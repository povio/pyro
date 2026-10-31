//! Blitz-backed geometry (`LIGHTNINGPANDA_GEOMETRY=blitz`): layout queries
//! (`getBoundingClientRect`, `offset*`, `client*`, `scroll*`) report the
//! boxes Blitz lays out, the same layout `captureScreenshot` and
//! `printToPDF` paint, instead of lp-dom's flow layout.
//!
//! Layout queries are synchronous on the browser thread, and the render
//! thread's documents are its own, so the browser thread keeps a second
//! `Renderer` here, with one persistent Blitz document per page document.
//! When lp-dom recomputes its layout (the DOM, styles or viewport changed
//! since the last query), the source builds the keyed node list, the mirror
//! applies what changed and Blitz relays out incrementally; the element boxes
//! then replace the flow layout's (`lp_dom::layout::GeometrySource`).
//!
//! Sub-resources: only `data:` URLs are loaded (synchronously). Images and
//! fonts from the network are not waited for, so their boxes may differ from
//! a render's until a later layout.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use lp_dom::layout::{ExternalBox, ExternalGeometry, GeometrySource, Layout};
use lp_dom::Dom;
use lp_render::{DeltaOp, Fetcher, Live, Renderer, Snapshot, TreeKind, TreeNode};

thread_local! {
    static RENDERER: RefCell<Option<Renderer>> = const { RefCell::new(None) };
}

static NEXT_PAGE: AtomicU64 = AtomicU64::new(1);
static CALLS: AtomicU64 = AtomicU64::new(0);
/// Syncs from lp-dom's change journal (the rest sent the whole tree).
static DELTAS: AtomicU64 = AtomicU64::new(0);
static SNAPSHOT_NS: AtomicU64 = AtomicU64::new(0);
static LAYOUT_NS: AtomicU64 = AtomicU64::new(0);
static READ_NS: AtomicU64 = AtomicU64::new(0);

/// Base URL of the Blitz documents (relative sub-resources are not loaded).
const BASE_URL: &str = "about:blank";

/// Layouts since the last call: count and milliseconds building the node
/// list, applying it and laying out, and reading the boxes.
pub(crate) fn take_stats() -> (u64, u64, f64, f64, f64) {
    let ms = |a: &AtomicU64| a.swap(0, Ordering::Relaxed) as f64 / 1e6;
    (
        CALLS.swap(0, Ordering::Relaxed),
        DELTAS.swap(0, Ordering::Relaxed),
        ms(&SNAPSHOT_NS),
        ms(&LAYOUT_NS),
        ms(&READ_NS),
    )
}

/// Makes this thread's new documents take Blitz geometry when
/// `LIGHTNINGPANDA_GEOMETRY=blitz`.
pub(crate) fn install() {
    if lp_dom::layout::blitz_geometry_requested() {
        lp_dom::layout::set_geometry_source_factory(Some(Rc::new(|| {
            Rc::new(BlitzGeometry { page: NEXT_PAGE.fetch_add(1, Ordering::Relaxed), synced: Default::default() })
                as Rc<dyn GeometrySource>
        })));
    }
}

struct DataUrls;

impl Fetcher for DataUrls {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        let body = lp_net::Url::parse(&url).ok().and_then(|u| lp_net::data_url::parse(&u)).map(|(_, b)| b.to_vec());
        done(body.ok_or_else(|| format!("{url}: Blitz geometry loads data: URLs only")));
    }
}

/// The Blitz document of one page document.
struct BlitzGeometry {
    page: u64,
    /// The DOM version, style generation and viewport it was last synced at.
    synced: std::cell::Cell<Option<(u64, u64, (u32, u32, u64, bool))>>,
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

impl BlitzGeometry {
    /// The document as a keyed node list for this page's Blitz document.
    fn snapshot(&self, dom: &Dom, layout: &Layout) -> Option<Snapshot> {
        let t = Instant::now();
        let s = lp_webapi::render_snapshot::render_tree_snapshot(dom, layout, layout.document(), BASE_URL);
        let tree = Arc::new(super::screenshot::convert(s.tree?));
        let snap = Snapshot {
            html: String::new(),
            base_url: BASE_URL.into(),
            sheets: s.sheets,
            width: s.width,
            height: s.height,
            device_scale_factor: s.device_scale_factor,
            dark: s.dark,
            scroll_x: 0.0,
            scroll_y: 0.0,
            resource_scope: format!("geometry-{}", self.page),
            live: Some(Live { page: self.page, tree, clip: None }),
        };
        SNAPSHOT_NS.fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
        Some(snap)
    }
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
    }
}

/// lp-dom's delta as lp-render takes it.
fn convert_delta(d: lp_dom::snapshot::RenderDelta) -> Vec<DeltaOp> {
    use lp_dom::snapshot::{DeltaOp as D, RenderNodeKind};
    let kind = |k: RenderNodeKind| match k {
        RenderNodeKind::Element { name, attrs } => TreeKind::Element { name, attrs },
        RenderNodeKind::Text(t) => TreeKind::Text(t),
    };
    d.ops
        .into_iter()
        .map(|op| match op {
            D::Node { key, kind: k } => DeltaOp::Node { key, kind: kind(k) },
            D::Children { parent, kids, nodes } => DeltaOp::Children {
                parent,
                kids,
                nodes: nodes.into_iter().map(|n| TreeNode { key: n.key, parent: n.parent, kind: kind(n.kind) }).collect(),
            },
        })
        .collect()
}

impl GeometrySource for BlitzGeometry {
    fn sync(&self, dom: &Dom, layout: &Layout) {
        let v = layout.viewport();
        let viewport = (v.width, v.height, v.device_scale_factor.to_bits(), v.color_scheme == lp_dom::media::ColorScheme::Dark);
        let now = (dom.version(), layout.style().observation_stamp(dom).1, viewport);
        let last = self.synced.replace(Some(now));
        // The changes since the last sync, when the journal covers them and
        // neither sheets nor viewport changed: no whole-tree snapshot or diff.
        if let Some((version, generation, vp)) = last {
            if generation == now.1 && vp == viewport {
                let t = Instant::now();
                let delta = lp_webapi::render_snapshot::render_tree_delta(dom, layout, layout.document(), BASE_URL, version);
                SNAPSHOT_NS.fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
                if let Some(delta) = delta {
                    let ops = convert_delta(delta);
                    let dpr = v.device_scale_factor;
                    let timings = RENDERER.with(|r| {
                        let mut r = r.try_borrow_mut().ok()?;
                        r.as_mut()?.sync_delta(self.page, &ops, (viewport.0, viewport.1, dpr, viewport.3)).ok()
                    });
                    if let Some(t) = timings {
                        CALLS.fetch_add(1, Ordering::Relaxed);
                        DELTAS.fetch_add(1, Ordering::Relaxed);
                        LAYOUT_NS.fetch_add((t.fetch + t.layout).as_nanos() as u64, Ordering::Relaxed);
                        return;
                    }
                }
            }
        }
        let Some(snap) = self.snapshot(dom, layout) else { return };
        let fetcher: Arc<dyn Fetcher> = Arc::new(DataUrls);
        let timings = RENDERER.with(|r| {
            let mut r = r.try_borrow_mut().ok()?;
            r.get_or_insert_with(Renderer::new).sync_layout(&snap, &fetcher).ok()
        });
        if let Some(t) = timings {
            CALLS.fetch_add(1, Ordering::Relaxed);
            LAYOUT_NS.fetch_add((t.fetch + t.layout).as_nanos() as u64, Ordering::Relaxed);
        }
    }

    fn element_box(&self, key: u64) -> Option<ExternalBox> {
        let t = Instant::now();
        let b = RENDERER.with(|r| r.try_borrow().ok()?.as_ref()?.element_box(self.page, key));
        READ_NS.fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
        b.map(external)
    }

    fn geometry(&self, dom: &Dom, layout: &Layout) -> Option<ExternalGeometry> {
        let snap = self.snapshot(dom, layout)?;
        let fetcher: Arc<dyn Fetcher> = Arc::new(DataUrls);
        let out = RENDERER.with(|r| {
            let mut r = r.try_borrow_mut().ok()?;
            r.get_or_insert_with(Renderer::new).layout_boxes(&snap, &fetcher).ok()
        })?;
        CALLS.fetch_add(1, Ordering::Relaxed);
        LAYOUT_NS.fetch_add((out.timings.fetch + out.timings.layout).as_nanos() as u64, Ordering::Relaxed);
        READ_NS.fetch_add(out.timings.scene.as_nanos() as u64, Ordering::Relaxed);
        Some(ExternalGeometry {
            boxes: out.boxes.into_iter().map(|(k, b)| (k, external(b))).collect(),
            document_width: out.document_width,
            document_height: out.document_height,
        })
    }
}
