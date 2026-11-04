//! lp-dom's layout queries (`getBoundingClientRect`, `offset*`, `client*`,
//! `scroll*`, hit testing, text ranges) answered by Blitz: the same layout
//! `captureScreenshot` and `printToPDF` paint.
//!
//! Layout queries are synchronous on the browser thread, and the render
//! thread's documents are its own, so the browser thread keeps a second
//! `Renderer` here, with one persistent Blitz document per page document.
//! When a layout query finds the DOM, styles or viewport changed since the
//! last one, the source sends lp-dom's change journal (else the keyed node
//! list), the mirror applies what changed and Blitz relays out
//! incrementally (`lp_dom::layout::GeometrySource`).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use lp_dom::geometry::{BoxPosition, ExternalSticky, ExternalWord};
use lp_dom::layout::{ExternalBox, ExternalGeometry, GeometrySource, Layout};
use lp_dom::snapshot::RenderHtml;
use lp_dom::style::StyleEngine;
use lp_dom::{Dom, NodeId};
use lp_render::{DeltaOp, Fetcher, Live, Renderer, Snapshot, Tree, TreeKind, TreeNode};

thread_local! {
    static RENDERER: RefCell<Option<Renderer>> = const { RefCell::new(None) };
}

static NEXT_PAGE: AtomicU64 = AtomicU64::new(1);

/// In a `serve --fork` template: the template must not start threads
/// before it forks, which system font enumeration through CoreText does.
/// With fork-safe fonts (`lp_render::use_fork_safe_fonts`, which the
/// template loads first) Blitz geometry runs in the template too, and forked
/// children inherit its warm documents; otherwise the template boots
/// prepared pages without geometry and each child lays them out after the
/// fork.
static IN_TEMPLATE: AtomicBool = AtomicBool::new(false);

/// Marks this process as a fork template (`true`) or one of its children
/// leaving it (`false`).
pub fn set_fork_template(template: bool) {
    IN_TEMPLATE.store(template, Ordering::Relaxed);
}

static CALLS: AtomicU64 = AtomicU64::new(0);
/// Every layout sync of the process (not reset; CDP Performance.getMetrics).
static TOTAL_CALLS: AtomicU64 = AtomicU64::new(0);
/// Syncs from lp-dom's change journal (the rest sent the whole tree).
static DELTAS: AtomicU64 = AtomicU64::new(0);
static SNAPSHOT_NS: AtomicU64 = AtomicU64::new(0);
static LAYOUT_NS: AtomicU64 = AtomicU64::new(0);
static READ_NS: AtomicU64 = AtomicU64::new(0);

/// `LP_GEOMETRY_PROFILE=1`: time in syncs, whole-document reads and box
/// reads, printed to stderr every 250 syncs (diagnostic).
mod profile {
    use std::cell::Cell;
    use std::time::Duration;
    thread_local! {
        static T: Cell<[(u64, Duration); 4]> = const { Cell::new([(0, Duration::ZERO); 4]) };
    }
    pub(crate) fn on() -> bool {
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ON.get_or_init(|| std::env::var_os("LP_GEOMETRY_PROFILE").is_some())
    }
    /// 0 sync, 1 full rebuild, 2 whole document, 3 element box.
    pub(crate) fn add(i: usize, d: Duration) {
        T.with(|t| {
            let mut v = t.get();
            v[i].0 += 1;
            v[i].1 += d;
            t.set(v);
            if i == 0 && v[0].0 % 250 == 0 {
                let ms = |a: &std::sync::atomic::AtomicU64| a.load(std::sync::atomic::Ordering::Relaxed) as f64 / 1e6;
                eprintln!(
                    "lp-geometry-profile pid={} sync={}/{:?} rebuild={}/{:?} document={}/{:?} box={}/{:?} delta-build={:.0}ms blitz={:.0}ms mirror/settle/resolve={:?} phases(style,damage,construct,pconstruct,layout,transform,paint_tree,c_damage,subdocs)={:.0?} inline-rebuilds/bytes={:?} calls/deltas={}/{} restyled={} styled/matched/shared/reused={:?}",
                    std::process::id(), v[0].0, v[0].1, v[1].0, v[1].1, v[2].0, v[2].1, v[3].0, v[3].1,
                    ms(&super::SNAPSHOT_NS), ms(&super::LAYOUT_NS), lp_render::sync_profile::peek(), lp_render::blitz_phases(), lp_render::blitz_rebuilds(), super::CALLS.load(std::sync::atomic::Ordering::Relaxed), super::DELTAS.load(std::sync::atomic::Ordering::Relaxed), lp_render::blitz_restyles(), lp_render::blitz_style_stats()
                );
            }
        });
    }
}

/// `LP_GEOMETRY_DEBUG=1`: describe elements whose box is empty (diagnostic).
fn geometry_debug() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("LP_GEOMETRY_DEBUG").is_some())
}

/// Bumped when a sub-resource arrives that documents may have been laid out
/// without (a font or image fetched in the background): each page's
/// document is built again at its next layout query.
static RESOURCES: AtomicU64 = AtomicU64::new(0);

/// A sub-resource arrived: documents laid out before it lay out again.
pub fn resources_changed() {
    RESOURCES.fetch_add(1, Ordering::Relaxed);
}

/// Base URL of a document whose URL was not given.
const BLANK: &str = "about:blank";

/// Layouts since the last call: count, journal syncs, and milliseconds
/// building the node list, applying it and laying out, and reading boxes.
pub fn take_stats() -> (u64, u64, f64, f64, f64) {
    let ms = |a: &AtomicU64| a.swap(0, Ordering::Relaxed) as f64 / 1e6;
    (
        CALLS.swap(0, Ordering::Relaxed),
        DELTAS.swap(0, Ordering::Relaxed),
        ms(&SNAPSHOT_NS),
        ms(&LAYOUT_NS),
        ms(&READ_NS),
    )
}

/// Layout syncs (each a restyle and relayout of what changed) since the
/// process started; unlike [`take_stats`], nothing resets it.
pub fn peek_layout_count() -> u64 {
    TOTAL_CALLS.load(Ordering::Relaxed)
}

/// Makes this thread's new documents take their geometry from Blitz,
/// loading sub-resources through `fetcher`.
pub fn install(fetcher: Arc<dyn Fetcher>) {
    lp_dom::layout::set_geometry_source_factory(Some(Rc::new(move || {
        Rc::new(BlitzGeometry {
            page: NEXT_PAGE.fetch_add(1, Ordering::Relaxed),
            synced: Default::default(),
            sheets: Default::default(),
            resources: std::cell::Cell::new(RESOURCES.load(Ordering::Relaxed)),
            base_url: RefCell::new(BLANK.into()),
            fetcher: fetcher.clone(),
        }) as Rc<dyn GeometrySource>
    })));
}

/// The Blitz document of one page document.
/// DOM version, style generation, viewport (width, height, device pixel
/// ratio bits, dark) and style invalidations.
type SyncedAt = (u64, u64, (u32, u32, u64, bool, bool), u64);

pub struct BlitzGeometry {
    page: u64,
    base_url: RefCell<String>,
    fetcher: Arc<dyn Fetcher>,
    /// The DOM version, style generation, viewport and style invalidations
    /// it was last synced at.
    synced: std::cell::Cell<Option<SyncedAt>>,
    /// [`RESOURCES`] when its document was built.
    resources: std::cell::Cell<u64>,
    /// Each style sheet's owner, whether it is a `<style>`, and its text's
    /// hash, as last synced (see [`BlitzGeometry::changed_style_sheets`]).
    sheets: RefCell<Vec<(NodeId, bool, u64)>>,
}

impl Drop for BlitzGeometry {
    fn drop(&mut self) {
        let _ = RENDERER.try_with(|r| {
            if let Ok(mut r) = r.try_borrow_mut() {
                if let Some(r) = r.as_mut() {
                    r.forget(self.page);
                }
            }
        });
    }
}

/// lp-dom's render tree as lp-render takes it.
fn convert(t: lp_dom::snapshot::RenderTree) -> Tree {
    Tree { nodes: t.nodes.into_iter().map(|n| TreeNode { key: n.key, parent: n.parent, kind: kind(n.kind) }).collect() }
}

fn kind(k: lp_dom::snapshot::RenderNodeKind) -> TreeKind {
    match k {
        lp_dom::snapshot::RenderNodeKind::Element { name, attrs } => TreeKind::Element { name, attrs },
        lp_dom::snapshot::RenderNodeKind::Text(t) => TreeKind::Text(t),
    }
}

/// lp-dom's delta as lp-render takes it.
fn convert_delta(d: lp_dom::snapshot::RenderDelta) -> Vec<DeltaOp> {
    use lp_dom::snapshot::DeltaOp as D;
    d.ops
        .into_iter()
        .map(|op| match op {
            D::Node { key, kind: k } => DeltaOp::Node { key, kind: kind(k) },
            D::Children { parent, kids, nodes } => DeltaOp::Children {
                parent,
                kids,
                nodes: nodes
                    .into_iter()
                    .map(|n| TreeNode { key: n.key, parent: n.parent, kind: kind(n.kind) })
                    .collect(),
            },
        })
        .collect()
}

fn external(b: lp_render::LayoutBox) -> ExternalBox {
    ExternalBox {
        x: b.x,
        y: b.y,
        width: b.width,
        height: b.height,
        client_width: b.client_width,
        client_height: b.client_height,
        scroll_width: b.scroll_width,
        scroll_height: b.scroll_height,
        fixed: b.fixed,
        border: b.border,
        padding: b.padding,
        position: match b.position {
            lp_render::BoxPosition::Static => BoxPosition::Static,
            lp_render::BoxPosition::Relative => BoxPosition::Relative,
            lp_render::BoxPosition::Absolute => BoxPosition::Absolute,
            lp_render::BoxPosition::Fixed => BoxPosition::Fixed,
            lp_render::BoxPosition::Sticky => BoxPosition::Sticky,
        },
    }
}

impl BlitzGeometry {
    /// The page's Blitz document (`Renderer` page id on this thread).
    pub fn page(&self) -> u64 {
        self.page
    }

    /// Linked sheet texts by absolute URL, and the link-to-URL resolver the
    /// render tree uses.
    fn render_opts<'a>(
        dom: &'a Dom,
        style: &'a StyleEngine,
        base_url: &str,
        sheets: &'a RefCell<HashMap<String, String>>,
    ) -> (impl Fn(NodeId) -> Option<String> + 'a, impl Fn(NodeId) -> Option<String> + 'a) {
        let base = url::Url::parse(base_url).ok();
        let style_text = move |owner: NodeId| style.sheet_text(owner).map(str::to_owned);
        let stylesheet_href = move |link: NodeId| {
            let text = style.sheet_text(link)?;
            let href = dom.get_attribute(link, "href").ok().flatten()?;
            let url = base.as_ref()?.join(href.trim()).ok()?.to_string();
            sheets.borrow_mut().insert(url.clone(), text.to_owned());
            Some(url)
        };
        (style_text, stylesheet_href)
    }

    /// The document as a keyed node list for this page's Blitz document.
    fn snapshot(&self, dom: &Dom, style: &StyleEngine) -> Option<Snapshot> {
        let t = Instant::now();
        let base_url = self.base_url.borrow().clone();
        let sheets = RefCell::new(HashMap::new());
        let tree = {
            let (style_text, stylesheet_href) = Self::render_opts(dom, style, &base_url, &sheets);
            let opts = RenderHtml { style_text: &style_text, stylesheet_href: &stylesheet_href, mark: None };
            Arc::new(convert(dom.render_tree(style.document(), &opts).ok()?))
        };
        let v = style.viewport();
        let snap = Snapshot {
            html: String::new(),
            base_url,
            sheets: sheets.into_inner(),
            width: v.width,
            height: v.height,
            device_scale_factor: v.device_scale_factor,
            dark: v.color_scheme == lp_dom::media::ColorScheme::Dark,
            print: v.print,
            scroll_x: 0.0,
            scroll_y: 0.0,
            resource_scope: format!("geometry-{}", self.page),
            live: Some(Live { page: self.page, tree, clip: None, interaction: None }),
            background: None,
            element_scroll: None,
        };
        SNAPSHOT_NS.fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
        Some(snap)
    }

    /// The pointer's and focus's elements, which the node list does not carry.
    fn sync_interaction(&self, dom: &Dom, style: &StyleEngine, style_only: bool) {
        let doc = style.document();
        let hovered = dom.hovered(doc).map(lp_dom::snapshot::render_key);
        let focused = dom.focused(doc).ok().flatten().map(lp_dom::snapshot::render_key);
        RENDERER.with(|r| {
            if let Ok(mut r) = r.try_borrow_mut() {
                if let Some(r) = r.as_mut() {
                    r.set_interaction(self.page, hovered, focused, style_only);
                }
            }
        });
    }

    fn sync_document(&self, dom: &Dom, style: &StyleEngine, style_only: bool) {
        let v = style.viewport();
        let viewport = (
            v.width,
            v.height,
            v.device_scale_factor.to_bits(),
            v.color_scheme == lp_dom::media::ColorScheme::Dark,
            v.print,
        );
        let now = (dom.version(), style.observation_stamp(dom).1, viewport, style.invalidations());
        let last = self.synced.replace(Some(now));
        // The changes since the last sync, when the journal covers them and
        // neither sheets nor viewport changed: no whole-tree snapshot or diff.
        if let Some((version, generation, vp, invalidations)) = last {
            // Sheets changed: when only `<style>` texts did (a script
            // appending rules), their texts go with the delta.
            let restyled = if generation == now.1 {
                Some(Vec::new())
            } else if invalidations == now.3 {
                self.changed_style_sheets(dom, style)
            } else {
                None
            };
            if let (Some(restyled), true) = (restyled, vp == viewport) {
                let t = Instant::now();
                let base_url = self.base_url.borrow().clone();
                let sheets = RefCell::new(HashMap::new());
                let (style_text, stylesheet_href) = Self::render_opts(dom, style, &base_url, &sheets);
                let opts = RenderHtml { style_text: &style_text, stylesheet_href: &stylesheet_href, mark: None };
                let delta = dom.render_delta(style.document(), version, &opts).map(|mut delta| {
                    delta.ops.extend(restyled.iter().filter_map(|&owner| dom.render_style_text(owner, &opts)));
                    delta
                });
                SNAPSHOT_NS.fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
                if let Some(delta) = delta {
                    let ops = convert_delta(delta);
                    let dpr = v.device_scale_factor;
                    let timings = RENDERER.with(|r| {
                        let mut r = r.try_borrow_mut().ok()?;
                        r.as_mut()?
                            .sync_delta(
                                self.page,
                                &ops,
                                (viewport.0, viewport.1, dpr, viewport.3, viewport.4),
                                style_only,
                            )
                            .ok()
                    });
                    if let Some(t) = timings {
                        CALLS.fetch_add(1, Ordering::Relaxed);
                        TOTAL_CALLS.fetch_add(1, Ordering::Relaxed);
                        TOTAL_CALLS.fetch_add(1, Ordering::Relaxed);
                        DELTAS.fetch_add(1, Ordering::Relaxed);
                        LAYOUT_NS.fetch_add((t.fetch + t.layout).as_nanos() as u64, Ordering::Relaxed);
                        return;
                    }
                }
            }
        }
        *self.sheets.borrow_mut() = Self::sheet_hashes(dom, style);
        let Some(snap) = self.snapshot(dom, style) else { return };
        let fetcher = self.fetcher.clone();
        let timings = RENDERER.with(|r| {
            let mut r = r.try_borrow_mut().ok()?;
            r.get_or_insert_with(Renderer::new).sync_layout(&snap, &fetcher).ok()
        });
        if let Some(t) = timings {
            CALLS.fetch_add(1, Ordering::Relaxed);
            TOTAL_CALLS.fetch_add(1, Ordering::Relaxed);
            LAYOUT_NS.fetch_add((t.fetch + t.layout).as_nanos() as u64, Ordering::Relaxed);
        }
    }
}

impl BlitzGeometry {
    /// Each style sheet's owner, whether it is a `<style>`, and its text's hash.
    fn sheet_hashes(dom: &Dom, style: &StyleEngine) -> Vec<(NodeId, bool, u64)> {
        use std::hash::{Hash, Hasher};
        style
            .sheet_owners()
            .into_iter()
            .map(|owner| {
                let is_style = dom
                    .node(owner)
                    .ok()
                    .and_then(|n| n.element())
                    .is_some_and(|e| e.is_html() && &*e.name.local == "style");
                let mut h = std::hash::DefaultHasher::new();
                match style.sheet_text(owner) {
                    Some(text) => text.hash(&mut h),
                    None => dom.text_content(owner).ok().flatten().hash(&mut h),
                }
                (owner, is_style, h.finish())
            })
            .collect()
    }

    /// The `<style>` elements whose sheet text changed since the last sync,
    /// when nothing else about the sheets did (`None`: sync the whole tree).
    /// Sheets added or removed with their elements come with the delta.
    fn changed_style_sheets(&self, dom: &Dom, style: &StyleEngine) -> Option<Vec<NodeId>> {
        let now = Self::sheet_hashes(dom, style);
        let mut last = self.sheets.borrow_mut();
        let mut changed = Vec::new();
        for &(owner, is_style, hash) in &now {
            match last.iter().find(|s| s.0 == owner) {
                Some(&(_, _, old)) if old == hash => {}
                Some(_) if is_style => changed.push(owner),
                Some(_) => return None,
                None if is_style => {}
                None => return None,
            }
        }
        if last.iter().any(|s| !s.1 && !now.iter().any(|n| n.0 == s.0)) {
            return None;
        }
        *last = now;
        Some(changed)
    }

    fn sync_inner(&self, dom: &Dom, style: &StyleEngine, style_only: bool) {
        let resources = RESOURCES.load(Ordering::Relaxed);
        let rebuild = self.resources.replace(resources) != resources;
        if rebuild {
            // Build the document again: Blitz does not retry a resource
            // that failed, and the new one is in the fetcher's cache now.
            RENDERER.with(|r| {
                if let Ok(mut r) = r.try_borrow_mut() {
                    if let Some(r) = r.as_mut() {
                        r.forget(self.page);
                    }
                }
            });
            self.synced.set(None);
        }
        let t = (rebuild && profile::on()).then(Instant::now);
        self.sync_document(dom, style, style_only);
        if let Some(t) = t {
            profile::add(1, t.elapsed());
        }
        self.sync_interaction(dom, style, style_only);
    }

    fn element_box_inner(&self, key: u64) -> Option<ExternalBox> {
        let t = Instant::now();
        let b = RENDERER.with(|r| r.try_borrow().ok()?.as_ref()?.element_box(self.page, key));
        READ_NS.fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
        if b.is_some_and(|b| b.width == 0.0 || b.height == 0.0) && geometry_debug() {
            let what = RENDERER.with(|r| r.try_borrow().ok()?.as_ref()?.describe_element(self.page, key));
            eprintln!("lp-geometry: empty box {b:?}\n  {}", what.unwrap_or_default());
        }
        b.map(external)
    }

    fn geometry_inner(&self) -> Option<ExternalGeometry> {
        let out = RENDERER.with(|r| r.try_borrow().ok()?.as_ref()?.page_layout(self.page))?;
        READ_NS.fetch_add(out.timings.scene.as_nanos() as u64, Ordering::Relaxed);
        Some(ExternalGeometry {
            boxes: out.boxes.into_iter().map(|(k, b)| (k, external(b))).collect(),
            words: out
                .words
                .into_iter()
                .map(|(k, w)| {
                    (
                        k,
                        ExternalWord {
                            x: w.x,
                            y: w.y,
                            width: w.width,
                            height: w.height,
                            fixed: w.fixed,
                            edges: w.edges,
                        },
                    )
                })
                .collect(),
            sticky: out
                .sticky
                .into_iter()
                .map(|(k, s)| (k, ExternalSticky { insets: s.insets, limit: s.limit }))
                .collect(),
            document_width: out.document_width,
            document_height: out.document_height,
        })
    }
}

impl GeometrySource for BlitzGeometry {
    fn active(&self) -> bool {
        !IN_TEMPLATE.load(Ordering::Relaxed) || lp_render::fork_safe_fonts()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn set_base_url(&self, url: &str) {
        *self.base_url.borrow_mut() = url.to_owned();
    }

    fn stale(&self) -> bool {
        self.resources.get() != RESOURCES.load(Ordering::Relaxed)
    }

    fn sync(&self, dom: &Dom, layout: &Layout) {
        let t = profile::on().then(Instant::now);
        self.sync_inner(dom, layout.style(), false);
        if let Some(t) = t {
            profile::add(0, t.elapsed());
        }
    }

    fn sync_styles(&self, dom: &Dom, style: &StyleEngine) {
        let t = profile::on().then(Instant::now);
        self.sync_inner(dom, style, true);
        if let Some(t) = t {
            profile::add(0, t.elapsed());
        }
    }

    fn element_box(&self, key: u64) -> Option<ExternalBox> {
        let t = profile::on().then(Instant::now);
        let b = self.element_box_inner(key);
        if let Some(t) = t {
            profile::add(3, t.elapsed());
        }
        b
    }

    fn element_quads(&self, key: u64) -> Option<lp_dom::layout::ExternalQuads> {
        let q = RENDERER.with(|r| r.try_borrow().ok()?.as_ref()?.element_quads(self.page, key))?;
        Some(lp_dom::layout::ExternalQuads {
            border: q.border,
            transformed: q.transformed,
            border_widths: q.border_widths,
            padding: q.padding,
            margin: q.margin,
            width: q.width,
            height: q.height,
            fragments: q.fragments,
            fixed: q.fixed,
        })
    }

    fn geometry(&self) -> Option<ExternalGeometry> {
        let t = profile::on().then(Instant::now);
        let g = self.geometry_inner();
        if let Some(t) = t {
            profile::add(2, t.elapsed());
        }
        g
    }

    fn document_size(&self) -> Option<(f64, f64)> {
        RENDERER.with(|r| r.try_borrow().ok()?.as_ref()?.document_size(self.page))
    }

    fn element_style(&self, key: u64) -> Option<style::servo_arc::Arc<style::properties::ComputedValues>> {
        RENDERER.with(|r| r.try_borrow().ok()?.as_ref()?.element_style(self.page, key))
    }

    fn has_sticky(&self) -> bool {
        RENDERER.with(|r| r.try_borrow().ok().and_then(|r| Some(r.as_ref()?.has_sticky(self.page))).unwrap_or(false))
    }
}

/// `Page.printToPDF` from the page document's own Blitz document (the one
/// its layout queries were answered from), on this thread. `None`: the
/// document has no Blitz geometry.
pub fn print_pdf(
    dom: &Dom,
    layout: &Layout,
    opts: &lp_render::PdfOptions,
) -> Option<Result<lp_render::PdfOutput, String>> {
    let paint = match paint_pdf(dom, layout, opts)? {
        Ok(paint) => paint,
        Err(e) => return Some(Err(e)),
    };
    RENDERER.with(|r| {
        let mut r = r.try_borrow_mut().ok()?;
        Some(r.as_mut()?.write_pdf(paint).map_err(|e| e.to_string()))
    })
}

/// The page document's whole layout, brought up to date: only for
/// `DOMSnapshot.captureSnapshot`. `None`: the document has no Blitz geometry.
pub fn layout_snapshot(dom: &Dom, layout: &Layout) -> Option<lp_render::LayoutSnapshot> {
    let source = layout.synced_geometry_source(dom)?;
    let page = source.as_any().downcast_ref::<BlitzGeometry>()?.page;
    RENDERER.with(|r| r.try_borrow().ok()?.as_ref()?.layout_snapshot(page))
}

/// [`print_pdf`] up to the PDF writing ([`lp_render::Renderer::write_pdf`],
/// which another thread can do).
pub fn paint_pdf(
    dom: &Dom,
    layout: &Layout,
    opts: &lp_render::PdfOptions,
) -> Option<Result<lp_render::PdfPaint, String>> {
    let source = layout.synced_geometry_source(dom)?;
    let page = source.as_any().downcast_ref::<BlitzGeometry>()?.page;
    let v = layout.viewport();
    RENDERER.with(|r| {
        let mut r = r.try_borrow_mut().ok()?;
        Some(
            r.as_mut()?
                .paint_pdf_page(page, (v.width, v.height), v.device_scale_factor, opts)
                .map_err(|e| e.to_string()),
        )
    })
}
