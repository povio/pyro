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
//! Sub-resources resolve against the document's URL and load synchronously
//! when they are local: `data:` URLs, `file:` URLs and `serve --static`
//! roots. Network ones (a Google Fonts `@import` and its fonts) are fetched
//! through the browser's network client, blocking the layout up to
//! [`NETWORK_WAIT`], once per process: bodies are kept by URL, so later
//! documents lay out with them at once. The text is then measured in the
//! font it is painted in, as Chromium measures it after `document.fonts`.

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

/// Base URL of a document whose URL was not given.
const BLANK: &str = "about:blank";

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
/// Longest wait for one network sub-resource of a layout.
const NETWORK_WAIT: std::time::Duration = std::time::Duration::from_secs(3);

/// Network sub-resources loaded for layouts, by URL (process-wide).
static NETWORK_BODIES: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<String, Arc<Vec<u8>>>>> =
    std::sync::LazyLock::new(Default::default);

pub(crate) fn install(statics: lp_net::StaticFiles, net: Option<lp_net::NetClient>) {
    if lp_dom::layout::blitz_geometry_requested() {
        let fetcher: Arc<dyn Fetcher> = Arc::new(LocalFiles { statics, net });
        lp_dom::layout::set_geometry_source_factory(Some(Rc::new(move || {
            Rc::new(BlitzGeometry {
                page: NEXT_PAGE.fetch_add(1, Ordering::Relaxed),
                synced: Default::default(),
                base_url: std::cell::RefCell::new(BLANK.into()),
                fetcher: fetcher.clone(),
            }) as Rc<dyn GeometrySource>
        })));
    }
}

/// Loads sub-resources synchronously: `data:` and `file:` URLs and
/// `serve --static` roots from disk, the rest through `net` (cached).
struct LocalFiles {
    statics: lp_net::StaticFiles,
    net: Option<lp_net::NetClient>,
}

impl LocalFiles {
    /// A network body, from the process cache or fetched (bounded wait: a
    /// request held by interception the blocked thread would answer gives
    /// up after [`NETWORK_WAIT`]).
    fn network(&self, url: &lp_net::Url) -> Option<Vec<u8>> {
        let key = url.as_str().to_owned();
        if let Some(b) = NETWORK_BODIES.lock().ok()?.get(&key) {
            return Some(b.to_vec());
        }
        let net = self.net.as_ref()?;
        let path = url.path().to_ascii_lowercase();
        let destination = if [".woff2", ".woff", ".ttf", ".otf"].iter().any(|e| path.ends_with(e)) {
            lp_net::Destination::Font
        } else if path.ends_with(".css") || url.host_str() == Some("fonts.googleapis.com") {
            lp_net::Destination::Style
        } else {
            lp_net::Destination::Image
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let id = net.next_request_id();
        net.fetch_with_id(
            id,
            lp_net::Request::get(url.clone(), destination),
            Box::new(move |ev| {
                let _ = tx.send(ev);
            }),
        );
        let deadline = Instant::now() + NETWORK_WAIT;
        let (mut ok, mut body) = (false, Vec::new());
        loop {
            match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(lp_net::ResponseEvent::Response(h)) => ok = h.status.is_success(),
                Ok(lp_net::ResponseEvent::Data(d)) => body.extend_from_slice(&d),
                Ok(lp_net::ResponseEvent::Redirect { .. }) => {}
                Ok(lp_net::ResponseEvent::Finished { .. }) => break,
                Ok(lp_net::ResponseEvent::Failed(_)) | Err(_) => {
                    net.cancel(id);
                    return None;
                }
            }
        }
        if !ok {
            return None;
        }
        NETWORK_BODIES.lock().ok()?.insert(key, Arc::new(body.clone()));
        Some(body)
    }
}

impl Fetcher for LocalFiles {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        let body = (|| {
            let u = lp_net::Url::parse(&url).ok()?;
            match u.scheme() {
                "data" => lp_net::data_url::parse(&u).map(|(_, b)| b.to_vec()),
                "file" => std::fs::read(u.to_file_path().ok()?).ok(),
                _ => match self.statics.lookup(&u, false) {
                    Some(hit) => Some(hit.file.body.to_vec()),
                    None => match self.statics.resolve(&u, false) {
                        Some(hit) => std::fs::read(hit.path).ok(),
                        None if matches!(u.scheme(), "http" | "https") => self.network(&u),
                        None => None,
                    },
                },
            }
        })();
        done(body.ok_or_else(|| format!("{url}: not a local resource")));
    }
}

/// The Blitz document of one page document.
struct BlitzGeometry {
    page: u64,
    base_url: std::cell::RefCell<String>,
    fetcher: Arc<dyn Fetcher>,
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
        let base_url = self.base_url.borrow().clone();
        let s = lp_webapi::render_snapshot::render_tree_snapshot(dom, layout, layout.document(), &base_url);
        let tree = Arc::new(super::screenshot::convert(s.tree?));
        let snap = Snapshot {
            html: String::new(),
            base_url,
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
                let base_url = self.base_url.borrow().clone();
                let delta =
                    lp_webapi::render_snapshot::render_tree_delta(dom, layout, layout.document(), &base_url, version);
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
        let fetcher = self.fetcher.clone();
        let timings = RENDERER.with(|r| {
            let mut r = r.try_borrow_mut().ok()?;
            r.get_or_insert_with(Renderer::new).sync_layout(&snap, &fetcher).ok()
        });
        if let Some(t) = timings {
            CALLS.fetch_add(1, Ordering::Relaxed);
            LAYOUT_NS.fetch_add((t.fetch + t.layout).as_nanos() as u64, Ordering::Relaxed);
        }
    }

    fn set_base_url(&self, url: &str) {
        *self.base_url.borrow_mut() = url.to_owned();
    }

    fn element_box(&self, key: u64) -> Option<ExternalBox> {
        let t = Instant::now();
        let b = RENDERER.with(|r| r.try_borrow().ok()?.as_ref()?.element_box(self.page, key));
        READ_NS.fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
        b.map(external)
    }

    fn geometry(&self, dom: &Dom, layout: &Layout) -> Option<ExternalGeometry> {
        let snap = self.snapshot(dom, layout)?;
        let fetcher = self.fetcher.clone();
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
