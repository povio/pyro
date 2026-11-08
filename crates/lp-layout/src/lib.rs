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
/// Timers, running only while `lp_render::profiling` is on.
static SNAPSHOT_NS: AtomicU64 = AtomicU64::new(0);
static LAYOUT_NS: AtomicU64 = AtomicU64::new(0);
static READ_NS: AtomicU64 = AtomicU64::new(0);

/// `LP_GEOMETRY_PROFILE=1`: time in syncs, whole-document reads and box
/// reads, printed to stderr every 250 syncs (diagnostic; turns
/// `lp_render::profiling` on for the counters it prints).
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
/// building the node list, applying it and laying out, and reading boxes
/// (the milliseconds only while `lp_render::profiling` is on).
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
    if profile::on() {
        lp_render::profiling::enable();
    }
    lp_dom::layout::set_geometry_source_factory(Some(Rc::new(move || {
        Rc::new(BlitzGeometry {
            page: NEXT_PAGE.fetch_add(1, Ordering::Relaxed),
            synced: Default::default(),
            sheets: Default::default(),
            resources: std::cell::Cell::new(RESOURCES.load(Ordering::Relaxed)),
            base_url: RefCell::new(BaseUrl::new(BLANK)),
            fetcher: fetcher.clone(),
        }) as Rc<dyn GeometrySource>
    })));
}

/// The Blitz document of one page document.
/// DOM version, style generation, viewport (width, height, device pixel
/// ratio bits, dark) and style invalidations.
type SyncedAt = (u64, u64, (u32, u32, u64, bool, bool), u64);

/// A document's base URL as given, and parsed once when it is set: links
/// resolve against the parsed form on every sync, while the snapshot keeps
/// the original text (`Url` normalizes it).
struct BaseUrl {
    text: String,
    parsed: Option<url::Url>,
}

impl BaseUrl {
    fn new(text: &str) -> Self {
        Self { text: text.to_owned(), parsed: url::Url::parse(text).ok() }
    }
}

pub struct BlitzGeometry {
    page: u64,
    base_url: RefCell<BaseUrl>,
    fetcher: Arc<dyn Fetcher>,
    /// The DOM version, style generation, viewport and style invalidations
    /// it was last synced at.
    synced: std::cell::Cell<Option<SyncedAt>>,
    /// [`RESOURCES`] when its document was built.
    resources: std::cell::Cell<u64>,
    /// Each style sheet's owner, whether it is a `<style>`, and its text's
    /// revision (`StyleEngine::sheet_revision`), as last synced (see
    /// [`BlitzGeometry::changed_style_sheets`]).
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

    /// The `<style>` text and link-to-URL resolvers the render tree uses;
    /// with `sheets`, linked sheet texts are collected by absolute URL (a
    /// whole-tree snapshot carries them, a delta does not).
    fn render_opts<'a>(
        dom: &'a Dom,
        style: &'a StyleEngine,
        base: Option<&'a url::Url>,
        sheets: Option<&'a RefCell<HashMap<String, String>>>,
    ) -> (impl Fn(NodeId) -> Option<String> + 'a, impl Fn(NodeId) -> Option<String> + 'a) {
        let style_text = move |owner: NodeId| style.sheet_text(owner).map(str::to_owned);
        let stylesheet_href = move |link: NodeId| {
            let text = style.sheet_text(link)?;
            let href = dom.get_attribute(link, "href").ok().flatten()?;
            let url = base?.join(href.trim()).ok()?.to_string();
            if let Some(sheets) = sheets {
                sheets.borrow_mut().insert(url.clone(), text.to_owned());
            }
            Some(url)
        };
        (style_text, stylesheet_href)
    }

    /// The document as a keyed node list for this page's Blitz document.
    fn snapshot(&self, dom: &Dom, style: &StyleEngine) -> Option<Snapshot> {
        let t = lp_render::profiling::on().then(Instant::now);
        let base = self.base_url.borrow();
        let sheets = RefCell::new(HashMap::new());
        let tree = {
            let (style_text, stylesheet_href) = Self::render_opts(dom, style, base.parsed.as_ref(), Some(&sheets));
            let adopted = |scope: NodeId| style.adopted_sheets(scope);
            let opts = RenderHtml {
                style_text: &style_text,
                stylesheet_href: &stylesheet_href,
                mark: None,
                adopted: style.has_adopted().then_some(&adopted as _),
            };
            Arc::new(convert(dom.render_tree(style.document(), &opts).ok()?))
        };
        let v = style.viewport();
        let snap = Snapshot {
            html: String::new(),
            base_url: base.text.clone(),
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
            canvases: Vec::new(),
            element_scroll: None,
        };
        if let Some(t) = t {
            SNAPSHOT_NS.fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
        }
        Some(snap)
    }

    /// The pointer's and focus's elements, which the node list does not carry.
    fn sync_interaction(&self, dom: &Dom, style: &StyleEngine, style_only: bool) {
        let doc = style.document();
        let hovered = dom.hovered(doc).map(lp_dom::snapshot::render_key);
        let focused = dom.focused(doc).ok().flatten().map(lp_dom::snapshot::render_key);
        let active = dom.activated(doc).map(lp_dom::snapshot::render_key);
        let focus_visible = dom.focus_visible(doc);
        RENDERER.with(|r| {
            if let Ok(mut r) = r.try_borrow_mut() {
                if let Some(r) = r.as_mut() {
                    r.set_interaction_states(self.page, hovered, focused, active, focus_visible, style_only);
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
                let profiling = lp_render::profiling::on();
                let t = profiling.then(Instant::now);
                let delta = {
                    let base = self.base_url.borrow();
                    let (style_text, stylesheet_href) = Self::render_opts(dom, style, base.parsed.as_ref(), None);
                    let adopted = |scope: NodeId| style.adopted_sheets(scope);
                    let opts = RenderHtml {
                        style_text: &style_text,
                        stylesheet_href: &stylesheet_href,
                        mark: None,
                        adopted: style.has_adopted().then_some(&adopted as _),
                    };
                    dom.render_delta(style.document(), version, &opts).map(|mut delta| {
                        delta.ops.extend(restyled.iter().filter_map(|&owner| dom.render_style_text(owner, &opts)));
                        delta
                    })
                };
                if let Some(t) = t {
                    SNAPSHOT_NS.fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
                }
                if let Some(delta) = delta {
                    let ops = convert_delta(delta);
                    let dpr = v.device_scale_factor;
                    let timings = RENDERER.with(|r| {
                        let mut r = r.try_borrow_mut().ok()?;
                        let r = r.as_mut()?;
                        guarded(r, self.page, |r| {
                            r.sync_delta(
                                self.page,
                                &ops,
                                (viewport.0, viewport.1, dpr, viewport.3, viewport.4),
                                style_only,
                            )
                            .ok()
                        })
                    });
                    if let Some(t) = timings {
                        CALLS.fetch_add(1, Ordering::Relaxed);
                        TOTAL_CALLS.fetch_add(1, Ordering::Relaxed);
                        DELTAS.fetch_add(1, Ordering::Relaxed);
                        if profiling {
                            LAYOUT_NS.fetch_add((t.fetch + t.layout).as_nanos() as u64, Ordering::Relaxed);
                        }
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
            guarded(r.get_or_insert_with(Renderer::new), self.page, |r| r.sync_layout(&snap, &fetcher).ok())
        });
        if let Some(t) = timings {
            CALLS.fetch_add(1, Ordering::Relaxed);
            TOTAL_CALLS.fetch_add(1, Ordering::Relaxed);
            if lp_render::profiling::on() {
                LAYOUT_NS.fetch_add((t.fetch + t.layout).as_nanos() as u64, Ordering::Relaxed);
            }
        }
    }
}

/// Runs a sync of `page`'s persistent document; a panic drops that document (it may be half
/// updated, for example a table missing its context) so that the next sync builds it afresh,
/// instead of every later layout query of the page failing.
fn guarded<T>(r: &mut Renderer, page: u64, f: impl FnOnce(&mut Renderer) -> Option<T>) -> Option<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&mut *r))).unwrap_or_else(|_| {
        r.forget(page);
        None
    })
}

impl BlitzGeometry {
    /// Each style sheet's owner, whether it is a `<style>`, and its text's
    /// revision: loader and CSSOM texts are not copied or hashed again.
    fn sheet_hashes(dom: &Dom, style: &StyleEngine) -> Vec<(NodeId, bool, u64)> {
        style
            .sheet_owners()
            .into_iter()
            .map(|owner| {
                let is_style = dom
                    .node(owner)
                    .ok()
                    .and_then(|n| n.element())
                    .is_some_and(|e| e.is_html() && &*e.name.local == "style");
                (owner, is_style, style.sheet_revision(dom, owner).unwrap_or_default())
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
        // Told before every sync, so the first restyle after the engine
        // learns to track (and the first sync, which creates the document)
        // records what it replaces.
        if style.track_style_changes() {
            RENDERER.with(|r| {
                if let Some(r) = r.try_borrow_mut().ok().as_deref_mut().and_then(Option::as_mut) {
                    r.set_track_style_changes(self.page, true);
                }
            });
        }
        let t = (rebuild && profile::on()).then(Instant::now);
        self.sync_document(dom, style, style_only);
        if let Some(t) = t {
            profile::add(1, t.elapsed());
        }
        self.sync_interaction(dom, style, style_only);
    }

    fn element_box_inner(&self, key: u64) -> Option<ExternalBox> {
        let t = lp_render::profiling::on().then(Instant::now);
        let b = RENDERER.with(|r| r.try_borrow().ok()?.as_ref()?.element_box(self.page, key));
        if let Some(t) = t {
            READ_NS.fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
        }
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
            sticky: out.sticky.into_iter().map(|(k, s)| (k, external_sticky(s))).collect(),
            document_width: out.document_width,
            document_height: out.document_height,
        })
    }

    /// The page's words (`within`: only those below that element's key, with
    /// others; see `Renderer::subtree_words`).
    fn text_words_inner(&self, within: Option<u64>) -> Option<Vec<(u64, ExternalWord)>> {
        let t = lp_render::profiling::on().then(Instant::now);
        let words = RENDERER.with(|r| {
            let r = r.try_borrow().ok()?;
            let r = r.as_ref()?;
            match within {
                Some(key) => r.subtree_words(self.page, key),
                None => r.page_words(self.page),
            }
        })?;
        if let Some(t) = t {
            READ_NS.fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
        }
        Some(
            words
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
        )
    }
}

fn external_sticky(s: lp_render::StickyBox) -> ExternalSticky {
    ExternalSticky { insets: s.insets, limit: s.limit }
}

impl GeometrySource for BlitzGeometry {
    fn active(&self) -> bool {
        !IN_TEMPLATE.load(Ordering::Relaxed) || lp_render::fork_safe_fonts()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn set_base_url(&self, url: &str) {
        *self.base_url.borrow_mut() = BaseUrl::new(url);
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

    fn text_words(&self) -> Option<Vec<(u64, ExternalWord)>> {
        let t = profile::on().then(Instant::now);
        let w = self.text_words_inner(None);
        if let Some(t) = t {
            profile::add(2, t.elapsed());
        }
        w
    }

    fn subtree_words(&self, key: u64) -> Option<Vec<(u64, ExternalWord)>> {
        let t = profile::on().then(Instant::now);
        let w = self.text_words_inner(Some(key));
        if let Some(t) = t {
            profile::add(2, t.elapsed());
        }
        w
    }

    fn sticky_elements(&self) -> Vec<(u64, ExternalSticky)> {
        RENDERER
            .with(|r| r.try_borrow().ok()?.as_ref()?.sticky_elements(self.page))
            .unwrap_or_default()
            .into_iter()
            .map(|(k, s)| (k, external_sticky(s)))
            .collect()
    }

    fn hit_candidates(
        &self,
        probe: &lp_dom::layout::HitProbe,
        offset: &dyn Fn(u64) -> (f64, f64),
        shifted: &[u64],
    ) -> Option<Vec<lp_dom::layout::HitVisit>> {
        let t = lp_render::profiling::on().then(Instant::now);
        let probe = lp_render::HitProbe {
            x: probe.x,
            y: probe.y,
            ignore_x: probe.ignore_x,
            window_scroll: probe.window_scroll,
        };
        // (The hit bounds are brought up to date under a borrow of their own:
        // `offset` reads scroll offsets through the renderer.)
        RENDERER.with(|r| {
            if let Some(r) = r.try_borrow_mut().ok().as_mut().and_then(|r| r.as_mut()) {
                r.ensure_hit_bounds(self.page);
            }
        });
        let visits =
            RENDERER.with(|r| r.try_borrow().ok()?.as_ref()?.hit_candidates(self.page, &probe, offset, shifted))?;
        if let Some(t) = t {
            READ_NS.fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
        }
        Some(
            visits
                .into_iter()
                .map(|v| lp_dom::layout::HitVisit {
                    key: v.key,
                    order: v.order,
                    bounds: external(v.bounds),
                    hit: v.hit,
                })
                .collect(),
        )
    }

    fn document_size(&self) -> Option<(f64, f64)> {
        RENDERER.with(|r| r.try_borrow().ok()?.as_ref()?.document_size(self.page))
    }

    fn element_style(&self, key: u64) -> Option<style::servo_arc::Arc<style::properties::ComputedValues>> {
        RENDERER.with(|r| r.try_borrow().ok()?.as_ref()?.element_style(self.page, key))
    }

    fn element_displayed(&self, key: u64) -> Option<bool> {
        RENDERER.with(|r| r.try_borrow().ok()?.as_ref()?.element_displayed(self.page, key))
    }

    fn take_style_changes(
        &self,
    ) -> Vec<(u64, style::servo_arc::Arc<style::properties::ComputedValues>, style::servo_arc::Arc<style::properties::ComputedValues>)>
    {
        RENDERER.with(|r| {
            r.try_borrow_mut()
                .ok()
                .as_deref_mut()
                .and_then(Option::as_mut)
                .map(|r| r.take_style_changes(self.page))
                .unwrap_or_default()
        })
    }

    fn element_pseudo_style(
        &self,
        key: u64,
        pseudo: &style::selector_parser::PseudoElement,
        generated_only: bool,
    ) -> Option<style::servo_arc::Arc<style::properties::ComputedValues>> {
        RENDERER.with(|r| r.try_borrow().ok()?.as_ref()?.element_pseudo_style(self.page, key, pseudo, generated_only))
    }

    fn may_have_generated_before_after(&self) -> bool {
        RENDERER.with(|r| {
            r.try_borrow()
                .ok()
                .and_then(|r| Some(r.as_ref()?.may_have_generated_before_after(self.page)))
                .unwrap_or(true)
        })
    }

    fn has_sticky(&self) -> bool {
        RENDERER.with(|r| r.try_borrow().ok().and_then(|r| Some(r.as_ref()?.has_sticky(self.page))).unwrap_or(false))
    }

    fn styles_depend_on_active(&self) -> bool {
        RENDERER.with(|r| r.try_borrow().ok()?.as_ref()?.styles_depend_on_active(self.page)).unwrap_or(true)
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

#[cfg(test)]
mod tests {
    //! lp-dom reads hot computed values typed (`StyleEngine::computed`) and
    //! hit tests from per-box derived data: both checked here against the
    //! serialized values and the stacking order.

    use std::cell::RefCell;
    use std::rc::Rc;

    use lp_dom::layout::Layout;
    use lp_dom::parse::{parse_document, DomRef};
    use lp_dom::select::parse_selectors;
    use lp_dom::style::{Display, UserSelect};
    use lp_dom::{Dom, NodeId};

    struct NoFetch;
    impl lp_render::Fetcher for NoFetch {
        fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
            done(Err(format!("{url}: offline")));
        }
    }

    fn page(html: &str) -> (DomRef, NodeId, Layout) {
        super::install(std::sync::Arc::new(NoFetch));
        let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
        let doc = parse_document(&dom, "http://test/", html);
        dom.borrow_mut().set_document_active(doc, true).unwrap();
        let mut layout = Layout::new(doc);
        layout.style_mut().discover_style_elements(&dom.borrow());
        (dom, doc, layout)
    }

    fn find(dom: &Dom, doc: NodeId, css: &str) -> NodeId {
        dom.query_selector(doc, &parse_selectors(css).unwrap()).unwrap().unwrap()
    }

    #[test]
    fn base_url_is_parsed_when_set_and_keeps_its_text() {
        use lp_dom::layout::GeometrySource;
        let g = super::BlitzGeometry {
            page: 0,
            synced: Default::default(),
            sheets: Default::default(),
            resources: Default::default(),
            base_url: RefCell::new(super::BaseUrl::new(super::BLANK)),
            fetcher: std::sync::Arc::new(NoFetch),
        };
        assert_eq!(g.base_url.borrow().text, "about:blank");
        assert_eq!(g.base_url.borrow().parsed.as_ref().map(|u| u.as_str()), Some("about:blank"));
        // Navigation sets a new base; the text stays as given while links
        // resolve against the parsed (normalized) form.
        g.set_base_url("HTTP://Example.COM/a/b.html");
        let base = g.base_url.borrow();
        assert_eq!(base.text, "HTTP://Example.COM/a/b.html");
        let parsed = base.parsed.as_ref().unwrap();
        assert_eq!(parsed.join("s.css").unwrap().as_str(), "http://example.com/a/s.css");
        drop(base);
        g.set_base_url("not a url");
        assert!(g.base_url.borrow().parsed.is_none());
    }

    #[test]
    fn typed_computed_values_match_their_serialization() {
        let (dom, doc, layout) = page(
            r#"<!doctype html><html><head><style>
              .a { position: relative; z-index: 3; opacity: 0.5 }
              .b { position: absolute; transform: translateX(2px); isolation: isolate }
              .c { position: fixed; z-index: -2; user-select: none }
              .d { position: sticky; top: 0; -webkit-user-select: text }
              .e { display: contents; user-select: all }
              .f { display: inline-flex } .g { display: inline-grid } .h { display: inline-table }
              .i { display: inline-block; user-select: contain } .j { display: inline list-item }
              .k { display: grid } .l { display: table-header-group } .m { display: none }
              .n { opacity: 0.9999 } .o { z-index: 7 }
            </style></head><body>
            <div class=a><span class=b>b</span><p class=c>c</p></div>
            <div class=d><i class=e><b class=f>f</b></i><b class=g>g</b><b class=h>h</b></div>
            <ul><li id=li>x</li><li id=li2 style="display: block">y</li><li class=j>j</li></ul>
            <div class=k><span class=o>o</span><span class=i>i</span></div>
            <table><tbody class=l><tr id=tr><td>t</td></tr></tbody></table>
            <div class=m><div class=n>n</div></div>
            </body></html>"#,
        );
        let dom = dom.borrow();
        let style = layout.style();
        let mut checked = 0;
        for el in dom.query_selector_all(doc, &parse_selectors("*").unwrap()).unwrap() {
            let Some(c) = style.computed(&dom, el) else { continue };
            let value = |name: &str| style.stylo_value(&dom, el, name).unwrap();
            let display = value("display");
            assert_eq!(c.position.keyword(), value("position"), "{display}");
            assert_eq!(c.display_contents, display == "contents");
            assert_eq!(
                c.atomic_inline,
                matches!(display.as_str(), "inline-block" | "inline-flex" | "inline-grid" | "inline-table"),
                "{display}"
            );
            assert_eq!(
                c.table_row_or_group,
                matches!(
                    display.as_str(),
                    "table-row" | "table-row-group" | "table-header-group" | "table-footer-group"
                )
            );
            let expected = match display.as_str() {
                "none" => Display::None,
                "flex" | "inline-flex" => Display::Flex,
                "grid" | "inline-grid" => Display::Grid,
                _ => Display::Other,
            };
            assert_eq!(c.display, expected, "{display}");
            assert_eq!(c.block_flex_or_grid, matches!(display.as_str(), "flex" | "grid"), "{display}");
            assert_eq!(c.z_index, value("z-index").parse::<i32>().ok());
            assert_eq!(c.transformed, value("transform") != "none");
            assert_eq!(c.translucent, value("opacity").parse::<f64>().unwrap() < 1.0);
            assert_eq!(c.isolated, value("isolation") == "isolate");
            let select = match value("user-select").as_str() {
                "none" => UserSelect::None,
                "text" => UserSelect::Text,
                "all" => UserSelect::All,
                "contain" => UserSelect::Contain,
                _ => UserSelect::Auto,
            };
            assert_eq!(c.user_select, select);
            checked += 1;
        }
        assert!(checked > 20, "{checked} elements styled");
        // The UA display (read without allocating) against an author one.
        let c = |css: &str| style.computed(&dom, find(&dom, doc, css)).unwrap();
        assert!(!c("#li").author_display);
        assert!(c("#li2").author_display);
        assert_eq!(
            c(".j").author_display,
            style.stylo_value(&dom, find(&dom, doc, ".j"), "display").unwrap() != "list-item"
        );
        assert!(!c("#tr").author_display);
        assert!(!c("table").author_display);
        assert!(c(".l").author_display);
        assert!(c(".m").author_display);
    }

    #[test]
    fn hit_testing_follows_stacking_order_and_clips() {
        let (dom, doc, layout) = page(
            r#"<!doctype html><html><head><style>
              body { margin: 0; height: 600px }
              div { width: 100px; height: 100px }
              #low { position: absolute; left: 0; top: 0; z-index: 1 }
              #high { position: absolute; left: 50px; top: 0; z-index: 2 }
              #neg { position: absolute; left: 200px; top: 0; z-index: -1 }
              #flow { position: absolute; left: 200px; top: 0; z-index: 0; opacity: 0.5 }
              #clip { position: absolute; left: 0; top: 200px; overflow: hidden; width: 50px; height: 50px }
              #escape { position: fixed; left: 0; top: 200px; width: 100px; height: 20px }
              #ctx { position: absolute; left: 300px; top: 0; z-index: 5 }
              #deep { position: absolute; left: 0; top: 0; z-index: 100; width: 10px; height: 10px }
              #over { position: absolute; left: 300px; top: 0; z-index: 6; width: 50px; height: 50px }
            </style></head><body>
            <div id=low></div><div id=high></div><div id=neg></div><div id=flow></div>
            <div id=clip><div id=inner></div><div id=escape></div></div>
            <div id=ctx><div id=deep></div></div><div id=over></div>
            </body></html>"#,
        );
        let dom = dom.borrow();
        let at = |x: f64, y: f64| layout.element_from_point(&dom, x, y);
        let id = |css: &str| Some(find(&dom, doc, css));
        assert_eq!(at(25.0, 50.0), id("#low"));
        assert_eq!(at(75.0, 50.0), id("#high"), "z-index 2 over 1");
        assert_eq!(at(250.0, 50.0), id("#flow"), "z 0 over z -1");
        assert_eq!(at(25.0, 230.0), id("#inner"));
        assert_eq!(at(75.0, 230.0), id("body"), "clipped by #clip's overflow");
        assert_eq!(at(75.0, 210.0), id("#escape"), "fixed escapes the clip");
        assert_eq!(at(5.0, 205.0), id("#escape"), "a later fixed box paints above");
        assert_eq!(at(305.0, 5.0), id("#over"), "z 100 is atomic inside its z 5 context");
        // Hit tests of the same layout read the per-box data again.
        assert_eq!(at(305.0, 5.0), id("#over"));
        assert_eq!(at(75.0, 50.0), id("#high"));
        assert_eq!(at(75.0, 230.0), id("body"));
    }
}
