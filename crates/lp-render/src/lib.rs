//! lp-render: renders a page [`Snapshot`] to pixels with Blitz (html5ever,
//! Stylo, Taffy, Parley; painted by Vello) and compares the result against a
//! baseline. No JS engine and no lightningpanda crate: the browser thread
//! builds the snapshot (`lp_webapi::PageDom::render_snapshot`), this crate
//! renders it on a [`worker::RenderWorker`] thread.
//!
//! Stages, timed separately ([`Timings`]):
//! 1. parse + style + layout, waiting for the sub-resources Blitz asks for
//!    (stylesheets, images, fonts) through a [`Fetcher`];
//! 2. paint into a backend-neutral display list and hash it ([`scene_hash`]);
//! 3. raster: replay the list into vello_cpu (default, deterministic) or
//!    vello on wgpu (`gpu` feature), read back RGBA.
//!
//! A match skips stage 3 when the display-list hash equals the hash recorded
//! for the baseline, and stops at the first differing pixel when no
//! difference is allowed.
//!
//! A [`Live`] snapshot carries the page as a keyed node list instead of
//! HTML: the renderer keeps the page's Blitz document and applies only what
//! changed since its previous render (`mirror` module).

mod compare;
mod hash;
mod mirror;
mod net;
pub mod pdf;
mod svg_use;
pub mod worker;

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use rustc_hash::{FxHashMap, FxHashSet};
use std::time::{Duration, Instant};

use anyrender::recording::Scene;
use anyrender::{ImageRenderer, PaintScene};
use anyrender_vello_cpu::VelloCpuImageRenderer;
use blitz_dom::{DocumentConfig, StyleThreading};
use blitz_traits::shell::{ColorScheme, Viewport as BlitzViewport};
use kurbo::{Affine, Rect};
use parley::fontique::{Blob, Collection, CollectionOptions, SourceCache};
use parley::FontContext;
use peniko::{Color, Fill};

pub use compare::{compare, decode_png, diff_image, encode_jpeg, encode_png, CompareResult};
pub use hash::scene_hash;
pub use mirror::{MirrorStats, Rebuild, Tree, TreeKind, TreeNode, TOP};
pub use net::Fetcher;

/// Content digest for caching baselines (xxh3).
pub fn digest(bytes: &[u8]) -> u64 {
    xxhash_rust::xxh3::xxh3_64(bytes)
}

/// Tallest image a full-page render produces (device px).
pub const MAX_HEIGHT: u32 = 16_384;
/// Widest image (device px).
pub const MAX_WIDTH: u32 = 16_384;

/// What the browser thread hands to the renderer: plain data, `Send`.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    /// The document as HTML: current DOM, form state written into
    /// attributes, `<style>` text from the style engine (CSSOM edits
    /// included), no scripts.
    pub html: String,
    /// Document URL; relative URLs resolve against it.
    pub base_url: String,
    /// Texts of `<link rel=stylesheet>` sheets by absolute URL, served to
    /// Blitz without a fetch (they include CSSOM edits).
    pub sheets: HashMap<String, String>,
    /// CSS px.
    pub width: u32,
    pub height: u32,
    pub device_scale_factor: f64,
    pub dark: bool,
    /// Window scroll position (CSS px).
    pub scroll_x: f64,
    pub scroll_y: f64,
    /// Sub-resource bytes fetched for an earlier snapshot of the same
    /// document are reused while this key is unchanged (the loader id).
    pub resource_scope: String,
    /// The page as a keyed tree, rendered with the page's persistent
    /// document instead of `html` (see [`Live`]).
    pub live: Option<Live>,
}

/// A snapshot of a page whose Blitz document the renderer keeps between
/// renders: each render applies only what changed since the page's last
/// one (`mirror` module). A new `resource_scope` (navigation) or base URL
/// starts a new document; [`Renderer::forget`] drops it.
#[derive(Clone, Debug)]
pub struct Live {
    /// Which page's document (unique per renderer).
    pub page: u64,
    /// The page now. The same `Arc` as the previous render's means nothing
    /// changed.
    pub tree: Arc<Tree>,
    /// Key of the element [`Area::Element`] captures.
    pub clip: Option<u64>,
}

/// Attribute the snapshot puts on the element a screenshot is clipped to.
pub const CLIP_ATTRIBUTE: &str = "data-lp-render-clip";

/// One element's boxes ([`Renderer::layout_boxes`]), CSS px.
#[derive(Clone, Copy, Debug, Default)]
pub struct LayoutBox {
    /// Border box, document coordinates.
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    /// Padding box size less scrollbars.
    pub client_width: f64,
    pub client_height: f64,
    pub scroll_width: f64,
    pub scroll_height: f64,
}

pub struct LayoutBoxes {
    /// By render key (`lp_dom::snapshot::render_key`).
    pub boxes: Vec<(u64, LayoutBox)>,
    pub document_width: f64,
    pub document_height: f64,
    /// `fetch`/`layout`: bringing the document up to date; `scene`: reading the boxes.
    pub timings: Timings,
}

/// `Page.printToPDF` options.
#[derive(Clone, Debug)]
pub struct PdfOptions {
    /// Elements that become one page each (Paprize: `.pz-page`).
    pub page_selector: String,
    /// Page size (CSS px) when no element matches.
    pub paper: (f64, f64),
}

/// One page of a [`PdfOutput`].
#[derive(Clone, Debug)]
pub struct PdfPage {
    /// The page box in the document (device px).
    pub rect: Rect,
    /// How far content inside the box reaches past its right and bottom
    /// edges (CSS px; 0 when it fits).
    pub overflow_right: f64,
    pub overflow_bottom: f64,
    /// The box reaching furthest past the page (`<tag class>` x,y wxh), when one does.
    pub culprit: Option<String>,
}

pub struct PdfOutput {
    pub bytes: Vec<u8>,
    pub pages: Vec<PdfPage>,
    /// Pages came from `page_selector` (else: the document cut by `paper`).
    pub by_selector: bool,
    pub stats: pdf::PdfStats,
    /// Display-list commands of the whole document.
    pub commands: usize,
    /// `scene`: paint; `raster`: PDF writing.
    pub timings: Timings,
}

/// Which part of the page to rasterize.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum Area {
    /// The viewport at the snapshot's scroll position.
    #[default]
    Viewport,
    /// The whole document from its top-left corner (`fullPage`,
    /// `captureBeyondViewport`).
    FullPage,
    /// A rectangle in CSS px, relative to the document (CDP `clip`), with
    /// its own scale factor.
    Clip { x: f64, y: f64, width: f64, height: f64, scale: f64 },
    /// The border box of the element marked with [`CLIP_ATTRIBUTE`].
    Element,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Backend {
    #[default]
    Cpu,
    /// vello on wgpu; falls back to the CPU when built without `gpu`.
    Gpu,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Timings {
    /// Sub-resources Blitz waited for.
    pub fetch: Duration,
    /// Parse, style and layout (fetch excluded).
    pub layout: Duration,
    /// Display list and its hash.
    pub scene: Duration,
    /// Raster and readback; zero on a hash hit.
    pub raster: Duration,
    /// Baseline PNG decode (first compare with a baseline only).
    pub decode: Duration,
    /// Pixel compare.
    pub compare: Duration,
    /// Style and layout passes while sub-resources arrived (1 when
    /// everything the page asked for came before the first layout).
    pub layout_passes: u32,
    /// How a [`Live`] snapshot used the page's document.
    pub mirror: Option<MirrorStats>,
    /// PNG encoding of the returned images.
    pub encode: Duration,
}

impl Timings {
    pub fn total(&self) -> Duration {
        self.fetch + self.layout + self.scene + self.raster + self.decode + self.compare + self.encode
    }
}

/// Straight RGBA8 (the canvas is opaque white, so premultiplied equals straight).
#[derive(Clone, Debug, PartialEq)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

/// Stages 1 and 2: the display list, not rasterized yet.
pub struct Painted {
    scene: Scene,
    pub hash: u64,
    pub width: u32,
    pub height: u32,
    pub timings: Timings,
}

#[derive(Debug)]
pub enum RenderError {
    /// No element carries [`CLIP_ATTRIBUTE`] or it has no box.
    NoElement,
    Resource(String),
    /// The area is empty or larger than [`MAX_WIDTH`] x [`MAX_HEIGHT`].
    BadArea(String),
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RenderError::Resource(m) => f.write_str(m),
            RenderError::NoElement => f.write_str("the element has no box to capture"),
            RenderError::BadArea(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for RenderError {}

/// The process's fonts: the system font scan (fontique, tens of ms) runs
/// once, and every [`Renderer`] works on a clone. Clones share the scanned
/// system fonts and the loaded font data; fonts a renderer registers stay its
/// own.
static FONTS: LazyLock<Mutex<FontContext>> = LazyLock::new(|| {
    let mut font_ctx = FontContext {
        source_cache: SourceCache::new_shared(),
        collection: Collection::new(CollectionOptions { shared: false, system_fonts: true }),
    };
    font_ctx.collection.register_fonts(Blob::new(Arc::new(blitz_dom::BULLET_FONT) as _), None);
    Mutex::new(font_ctx)
});

fn process_fonts() -> FontContext {
    FONTS.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Warms the process-wide state a first render would otherwise pay for, on a
/// background thread: the system font scan, the system font database usvg
/// loads for the first inline SVG, Stylo's global data and the platform's
/// font fallback. Call once at startup, before any `fork`.
pub fn prewarm() -> std::io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new().name("lp-render-warm".into()).spawn(|| {
        struct NoFetch;
        impl Fetcher for NoFetch {
            fn fetch(&self, _url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
                done(Err("prewarm".into()));
            }
        }
        let snap = Snapshot {
            html: PREWARM_HTML.into(),
            base_url: "about:blank".into(),
            width: 64,
            height: 64,
            device_scale_factor: 1.0,
            ..Default::default()
        };
        let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
        let _ = Renderer::new().paint(&snap, Area::Viewport, &fetcher);
    })
}

/// Text in a few scripts (font fallback), a form control (text editor
/// layout) and an inline SVG (usvg's font database).
const PREWARM_HTML: &str = r#"<!doctype html><html><body style="font-family: system-ui">
<p>Aa 0 €<b>b</b><i>i</i> Ωж 中文 العربية</p><input value="x"><button>b</button>
<svg width="8" height="8"><text x="0" y="8">s</text></svg></body></html>"#;

/// Fonts and raster backends, reused across renders on one thread.
pub struct Renderer {
    font_ctx: FontContext,
    font_hashes: HashMap<u64, u64>,
    cpu: Option<(VelloCpuImageRenderer, (u32, u32))>,
    #[cfg(feature = "gpu")]
    gpu: Option<(anyrender_vello::VelloImageRenderer, (u32, u32))>,
    resources: net::ResourceCache,
    /// Persistent documents of [`Live`] snapshots, by page.
    mirrors: HashMap<u64, mirror::Mirror>,
    pdf_cache: pdf::PdfCache,
    /// Longest wait for sub-resources per render.
    pub fetch_timeout: Duration,
    /// Longest wait, before the first layout, for the fonts and images the
    /// document asked for (a browser's font block period); sheets block it
    /// until `fetch_timeout`.
    pub first_layout_wait: Duration,
}

impl Default for Renderer {
    fn default() -> Self {
        Self::new()
    }
}

impl Renderer {
    pub fn new() -> Self {
        Self {
            font_ctx: process_fonts(),
            font_hashes: HashMap::new(),
            cpu: None,
            #[cfg(feature = "gpu")]
            gpu: None,
            resources: net::ResourceCache::default(),
            mirrors: HashMap::new(),
            pdf_cache: pdf::PdfCache::default(),
            fetch_timeout: Duration::from_secs(10),
            first_layout_wait: Duration::from_secs(3),
        }
    }

    /// Registers font bytes (TTF/OTF) for every later render, e.g. a font a
    /// test suite pins so baselines do not depend on the machine's fonts.
    pub fn register_font(&mut self, bytes: Vec<u8>) {
        self.font_ctx.collection.register_fonts(Blob::new(Arc::new(bytes)), None);
    }

    /// Drops the persistent document of `page` (closed, or gone).
    pub fn forget(&mut self, page: u64) {
        self.mirrors.remove(&page);
    }

    /// Pages with a persistent document.
    pub fn mirrored_pages(&self) -> usize {
        self.mirrors.len()
    }

    /// Every box of `page`'s persistent document, one line each (diagnostics).
    #[doc(hidden)]
    pub fn mirrored_layout(&self, page: u64) -> Vec<String> {
        let Some(m) = self.mirrors.get(&page) else { return Vec::new() };
        let mut out = Vec::new();
        let mut stack = vec![(m.doc.root_element().id, 0usize)];
        while let Some((id, depth)) = stack.pop() {
            let node = m.doc.get_node(id).expect("node");
            let line = match node.element_data() {
                Some(e) => {
                    let l = node.final_layout();
                    let class = e.attr(blitz_dom::local_name!("class")).unwrap_or("");
                    let (x, y, w, h) = (l.location.x, l.location.y, l.size.width, l.size.height);
                    format!("<{} {class:?}> {x:.1},{y:.1} {w:.1}x{h:.1}", e.name.local)
                }
                None => format!("{:?}", node.text_content().chars().take(20).collect::<String>()),
            };
            out.push(format!("{}{line}", " ".repeat(depth)));
            for &c in node.children.iter().rev() {
                stack.push((c, depth + 1));
            }
        }
        out
    }

    /// The markup of `page`'s persistent document (tests, diagnostics).
    #[doc(hidden)]
    pub fn mirrored_html(&self, page: u64) -> Option<String> {
        self.mirrors.get(&page).map(|m| m.doc.root_element().outer_html())
    }

    fn config(&self, snap: &Snapshot, provider: Arc<dyn blitz_traits::net::NetProvider>) -> DocumentConfig {
        DocumentConfig {
            base_url: Some(snap.base_url.clone()),
            net_provider: Some(provider),
            viewport: Some(viewport_of(snap)),
            font_ctx: Some(self.font_ctx.clone()),
            style_threading: StyleThreading::Sequential,
            ..DocumentConfig::default()
        }
    }

    /// The page's persistent document, brought up to `live.tree`.
    fn mirror(&mut self, snap: &Snapshot, live: &Live, fetcher: &Arc<dyn Fetcher>) -> MirrorStats {
        let key = viewport_key(snap);
        let reason = match self.mirrors.get_mut(&live.page) {
            None => Some(Rebuild::First),
            Some(m) if m.scope != snap.resource_scope || m.base_url != snap.base_url => Some(Rebuild::Document),
            Some(m) => {
                m.bridge.retarget(snap, fetcher.clone());
                if m.viewport != key {
                    m.doc.set_viewport(viewport_of(snap));
                    m.viewport = key;
                }
                match m.apply(&live.tree, &snap.sheets) {
                    Ok(mutations) => return MirrorStats { rebuilt: None, mutations, pages: self.mirrors.len() as u32 },
                    Err(reason) => Some(reason),
                }
            }
        };
        self.mirrors.remove(&live.page);
        let bridge = net::Bridge::new(snap, fetcher.clone(), net::new_cache());
        let config = self.config(snap, bridge.provider());
        let mutations = live.tree.nodes.len() as u32;
        match mirror::Mirror::build(
            live.tree.clone(),
            config,
            bridge,
            snap.resource_scope.clone(),
            snap.sheets.clone(),
            key,
        ) {
            Ok(m) => {
                self.mirrors.insert(live.page, m);
                MirrorStats { rebuilt: reason, mutations, pages: self.mirrors.len() as u32 }
            }
            Err(e) => MirrorStats { rebuilt: Some(e), mutations: 0, pages: self.mirrors.len() as u32 },
        }
    }

    /// Stage 1: the document styled and laid out, sub-resources settled.
    /// Returns it with the node [`Live::clip`] (or [`CLIP_ATTRIBUTE`]) names.
    fn prepare<'a>(
        &'a mut self,
        snap: &Snapshot,
        fetcher: &Arc<dyn Fetcher>,
        fresh: &'a mut Option<(blitz_dom::BaseDocument, net::Bridge)>,
        timings: &mut Timings,
    ) -> Result<(&'a mut blitz_dom::BaseDocument, Option<blitz_dom::NodeId>), RenderError> {
        let started = Instant::now();
        let (doc, bridge, clip) = match &snap.live {
            Some(live) => {
                let stats = self.mirror(snap, live, fetcher);
                timings.mirror = Some(stats);
                match self.mirrors.get_mut(&live.page) {
                    Some(m) => {
                        let clip = live.clip.and_then(|k| m.node(k));
                        (&mut m.doc, &m.bridge, clip)
                    }
                    None => return Err(RenderError::BadArea("the page's tree has a repeated key".into())),
                }
            }
            None => {
                let bridge = net::Bridge::new(snap, fetcher.clone(), self.resources.scoped(&snap.resource_scope));
                let doc = blitz_html::HtmlDocument::from_html(&snap.html, self.config(snap, bridge.provider()));
                let mut doc = doc.into_inner();
                svg_use::resolve(&mut doc, &snap.base_url, fetcher, self.fetch_timeout)
                    .map_err(RenderError::Resource)?;
                let (doc, bridge) = fresh.insert((doc, bridge));
                let clip = doc.query_selector(&format!("[{CLIP_ATTRIBUTE}]")).ok().flatten();
                (doc, &*bridge, clip)
            }
        };
        let (fetch, passes) = bridge.settle(doc, started + self.fetch_timeout, self.first_layout_wait);
        timings.layout_passes = passes;
        doc.resolve(0.0);
        timings.fetch = fetch;
        timings.layout = started.elapsed().saturating_sub(fetch);
        Ok((doc, clip))
    }

    /// Stages 1 and 2.
    pub fn paint(&mut self, snap: &Snapshot, area: Area, fetcher: &Arc<dyn Fetcher>) -> Result<Painted, RenderError> {
        let mut timings = Timings::default();
        let dpr = snap.device_scale_factor.clamp(0.1, 16.0);
        let (vw, vh) = (snap.width.max(1), snap.height.max(1));
        let physical = |css: f64| (css * dpr).round() as u32;

        let mut fresh = None;
        let (doc, clip) = self.prepare(snap, fetcher, &mut fresh, &mut timings)?;

        let t = Instant::now();
        // A persistent document keeps the scroll of its last capture; only
        // a viewport capture is scrolled.
        if area != Area::Viewport {
            doc.set_viewport_scroll(blitz_dom::Point { x: 0.0, y: 0.0 });
        }
        let root = {
            let root = doc.root_element();
            let (size, overflow) = (root.final_layout().size, root.scrollable_overflow());
            let clipped = root.primary_styles().is_some_and(|s| {
                matches!(
                    s.clone_overflow_y(),
                    style::values::computed::Overflow::Hidden | style::values::computed::Overflow::Clip
                )
            });
            (size.width as f64, if clipped { size.height as f64 } else { (size.height as f64).max(overflow.y1) })
        };
        // Where the image starts in the document (CSS px) and its size in device px.
        let (origin, scale, w, h) = match area {
            Area::Viewport => {
                doc.set_viewport_scroll(blitz_dom::Point { x: snap.scroll_x, y: snap.scroll_y });
                ((0.0, 0.0), dpr, physical(vw as f64), physical(vh as f64))
            }
            Area::FullPage => {
                // The viewport's width, as Chromium's `captureBeyondViewport`:
                // the document's scrollable overflow also counts content wider
                // than the scroll containers that clip it (a wide table).
                let height = root.1.max(vh as f64);
                ((0.0, 0.0), dpr, physical(vw as f64), physical(height))
            }
            Area::Clip { x, y, width, height, scale } => {
                let s = dpr * scale.clamp(0.01, 16.0);
                ((x, y), s, (width * s).round() as u32, (height * s).round() as u32)
            }
            Area::Element => {
                let id = clip.ok_or(RenderError::NoElement)?;
                let node = doc.get_node(id).ok_or(RenderError::NoElement)?;
                let size = node.final_layout().size;
                if size.width <= 0.0 || size.height <= 0.0 {
                    return Err(RenderError::NoElement);
                }
                let bounds = element_bounds(doc, id, dpr).ok_or(RenderError::NoElement)?;
                (
                    (bounds.x0 / dpr, bounds.y0 / dpr),
                    dpr,
                    (bounds.width().round() as u32).max(1),
                    (bounds.height().round() as u32).max(1),
                )
            }
        };
        if w == 0 || h == 0 || w > MAX_WIDTH || h > MAX_HEIGHT {
            return Err(RenderError::BadArea(format!(
                "capture area {w}x{h} device px is empty or larger than {MAX_WIDTH}x{MAX_HEIGHT}"
            )));
        }
        // Blitz paints the document at its own scale; the area's origin and
        // a clip scale are applied as a transform of the recorded scene.
        let mut scene = Scene::new();
        scene.fill(Fill::NonZero, Affine::IDENTITY, Color::WHITE, None, &Rect::new(0.0, 0.0, w as f64, h as f64));
        let mut page = Scene::new();
        let extent_w = ((origin.0 * dpr) as u32).saturating_add((w as f64 * dpr / scale).ceil() as u32);
        let extent_h = ((origin.1 * dpr) as u32).saturating_add((h as f64 * dpr / scale).ceil() as u32);
        blitz_paint::paint_scene(&mut page, doc, dpr, extent_w, extent_h, 0, 0);
        let transform = Affine::scale(scale / dpr) * Affine::translate((-origin.0 * dpr, -origin.1 * dpr));
        scene.append_scene(page, transform);
        let hash = scene_hash(&scene, w, h, &mut self.font_hashes);
        timings.scene = t.elapsed();
        Ok(Painted { scene, hash, width: w, height: h, timings })
    }

    /// Element boxes of a [`Live`] snapshot's document as Blitz lays it out
    /// (CSS px, document coordinates), for answering layout queries with
    /// the geometry that is painted. The page's persistent document is
    /// brought up to the snapshot first (only what changed is applied and
    /// relaid out).
    pub fn layout_boxes(&mut self, snap: &Snapshot, fetcher: &Arc<dyn Fetcher>) -> Result<LayoutBoxes, RenderError> {
        let live =
            snap.live.as_ref().ok_or_else(|| RenderError::BadArea("layout boxes need a live snapshot".into()))?;
        let mut timings = Timings::default();
        let mut fresh = None;
        self.prepare(snap, fetcher, &mut fresh, &mut timings)?;
        let t = Instant::now();
        let m = self.mirrors.get(&live.page).ok_or(RenderError::NoElement)?;
        let doc = &m.doc;
        let mut boxes = Vec::new();
        // Each node's transform to the document, memoized along the
        // containing-block chains (`element_bounds` walks each one whole).
        let mut transforms: FxHashMap<blitz_dom::NodeId, Affine> = FxHashMap::default();
        // Node to its entry in `boxes`.
        let mut index: FxHashMap<blitz_dom::NodeId, usize> = FxHashMap::default();
        for (key, id) in m.elements() {
            let Some(node) = doc.get_node(id) else { continue };
            let Some(t) = node_transform(doc, id, &mut transforms) else { continue };
            // Unrounded, as Chromium reports fractional boxes (painting uses the rounded layout).
            let l = node.unrounded_layout();
            let size = l.size;
            let rect = t.transform_rect_bbox(Rect::new(0.0, 0.0, size.width as f64, size.height as f64));
            let client_width = (l.size.width - l.border.left - l.border.right - l.scrollbar_size.width).max(0.0) as f64;
            let client_height =
                (l.size.height - l.border.top - l.border.bottom - l.scrollbar_size.height).max(0.0) as f64;
            let o = node.scrollable_overflow();
            index.insert(id, boxes.len());
            boxes.push((
                key,
                LayoutBox {
                    x: rect.x0,
                    y: rect.y0,
                    width: rect.width(),
                    height: rect.height(),
                    client_width,
                    client_height,
                    scroll_width: client_width.max(o.x1 - l.border.left as f64),
                    scroll_height: client_height.max(o.y1 - l.border.top as f64),
                },
            ));
        }
        // Inline elements have no box of their own in Blitz: their content
        // is in the inline root's text layout. Chromium's rect is the union
        // of their fragments: each glyph run's (content area: ascent to
        // descent) and atomic inline's, credited to every inline ancestor
        // up to the root.
        {
            // Only the inline roots holding an inline element without a box
            // (walking every text layout per query is the costly part).
            let mut roots: FxHashSet<blitz_dom::NodeId> = FxHashSet::default();
            for (&id, &i) in &index {
                let b = &boxes[i].1;
                if b.width != 0.0 || b.height != 0.0 {
                    continue;
                }
                let inline = doc
                    .get_node(id)
                    .and_then(|n| n.primary_styles())
                    .is_some_and(|s| s.clone_display().is_inline_flow());
                if !inline {
                    continue;
                }
                let mut cur = doc.get_node(id).and_then(|n| n.parent);
                while let Some(p) = cur {
                    let Some(n) = doc.get_node(p) else { break };
                    if n.flags.is_inline_root() {
                        roots.insert(p);
                        break;
                    }
                    cur = n.parent;
                }
            }
            let mut acc: FxHashMap<blitz_dom::NodeId, Rect> = FxHashMap::default();
            for root_id in roots {
                let Some(root) = doc.get_node(root_id) else { continue };
                let Some(text) = root.element_data().and_then(|e| e.inline_layout_data.as_ref()) else { continue };
                let Some(t) = transforms.get(&root_id).copied() else { continue };
                let l = root.unrounded_layout();
                let origin = t * Affine::translate((
                    (l.border.left + l.padding.left) as f64,
                    (l.border.top + l.padding.top) as f64 + text.block_offset as f64,
                ));
                let scale = text.layout.scale() as f64;
                let mut credit = |start: Option<blitz_dom::NodeId>, r: Rect| {
                    let r =
                        origin.transform_rect_bbox(Rect::new(r.x0 / scale, r.y0 / scale, r.x1 / scale, r.y1 / scale));
                    let mut cur = start;
                    while let Some(id) = cur {
                        if id == root_id {
                            break;
                        }
                        let Some(node) = doc.get_node(id) else { break };
                        let Some(styles) = node.primary_styles() else { break };
                        let display = styles.clone_display();
                        if !display.is_contents() {
                            if !display.is_inline_flow() {
                                break;
                            }
                            acc.entry(id).and_modify(|a| *a = a.union(r)).or_insert(r);
                        }
                        cur = node.parent;
                    }
                };
                for line in text.layout.lines() {
                    for item in line.items() {
                        match item {
                            parley::PositionedLayoutItem::GlyphRun(run) => {
                                let metrics = run.run().font_metrics();
                                let (x, baseline) = (run.offset() as f64, run.baseline() as f64);
                                let r = Rect::new(
                                    x,
                                    baseline - metrics.ascent as f64,
                                    x + run.advance() as f64,
                                    baseline + metrics.descent as f64,
                                );
                                credit(Some(run.style().brush.id), r);
                            }
                            parley::PositionedLayoutItem::InlineBox(b) => {
                                let id = blitz_dom::NodeId::from_u64(b.id);
                                let parent = doc.get_node(id).and_then(|n| n.parent);
                                let r =
                                    Rect::new(b.x as f64, b.y as f64, (b.x + b.width) as f64, (b.y + b.height) as f64);
                                credit(parent, r);
                            }
                        }
                    }
                }
            }
            for (id, r) in acc {
                let Some(&i) = index.get(&id) else { continue };
                let b = &mut boxes[i].1;
                if b.width == 0.0 && b.height == 0.0 {
                    *b = LayoutBox {
                        x: r.x0,
                        y: r.y0,
                        width: r.width(),
                        height: r.height(),
                        client_width: 0.0,
                        client_height: 0.0,
                        scroll_width: 0.0,
                        scroll_height: 0.0,
                    };
                }
            }
        }
        // Collapsed borders: Blitz lays them out as gutters between
        // borderless cells (the table's gap: the first cell's widest
        // horizontal / vertical border). Chromium's cell boxes take half of
        // each collapsed border, so a row measures a border taller.
        {
            fn is_cell(doc: &blitz_dom::BaseDocument, id: blitz_dom::NodeId) -> bool {
                doc.get_node(id)
                    .and_then(|n| n.element_data())
                    .is_some_and(|e| matches!(e.name.local.as_ref(), "td" | "th"))
            }
            fn first_cell(doc: &blitz_dom::BaseDocument, id: blitz_dom::NodeId) -> Option<blitz_dom::NodeId> {
                let node = doc.get_node(id)?;
                for &c in &node.children {
                    if is_cell(doc, c) {
                        return Some(c);
                    }
                    let nested_table =
                        doc.get_node(c).and_then(|n| n.element_data()).is_some_and(|e| &*e.name.local == "table");
                    if !nested_table {
                        if let Some(f) = first_cell(doc, c) {
                            return Some(f);
                        }
                    }
                }
                None
            }
            let side = |w: app_units::Au, s: style::values::computed::BorderStyle| {
                if s.none_or_hidden() {
                    0.0
                } else {
                    w.to_f64_px()
                }
            };
            let mut gutters: FxHashMap<blitz_dom::NodeId, Option<(f64, f64)>> = FxHashMap::default();
            for (_, id) in m.elements() {
                if !is_cell(doc, id) {
                    continue;
                }
                let mut table = doc.get_node(id).and_then(|n| n.parent);
                while let Some(t) = table {
                    let n = doc.get_node(t);
                    if n.and_then(|n| n.element_data()).is_some_and(|e| &*e.name.local == "table") {
                        break;
                    }
                    table = n.and_then(|n| n.parent);
                }
                let Some(table) = table else { continue };
                let gutter = *gutters.entry(table).or_insert_with(|| {
                    let styles = doc.get_node(table)?.primary_styles()?;
                    if styles.clone_border_collapse() != style::computed_values::border_collapse::T::Collapse {
                        return None;
                    }
                    let cell = doc.get_node(first_cell(doc, table)?)?.primary_styles()?;
                    let b = cell.get_border();
                    let x = side(b.border_left_width.0, b.border_left_style)
                        .max(side(b.border_right_width.0, b.border_right_style));
                    let y = side(b.border_top_width.0, b.border_top_style)
                        .max(side(b.border_bottom_width.0, b.border_bottom_style));
                    Some((x / 2.0, y / 2.0))
                });
                let Some((gx, gy)) = gutter else { continue };
                // Row borders collapse into the cells' too; Blitz's gutter
                // only has the first cell's.
                let (mut top, mut bottom) = (gy, gy);
                if let Some(row) = doc.get_node(id).and_then(|n| n.parent).and_then(|p| doc.get_node(p)) {
                    if let Some(rs) = row.primary_styles() {
                        let b = rs.get_border();
                        top = top.max(side(b.border_top_width.0, b.border_top_style) / 2.0);
                        bottom = bottom.max(side(b.border_bottom_width.0, b.border_bottom_style) / 2.0);
                    }
                }
                if let Some(&i) = index.get(&id) {
                    let b = &mut boxes[i].1;
                    b.x -= gx;
                    b.y -= top;
                    b.width += 2.0 * gx;
                    b.height += top + bottom;
                }
            }
        }
        // Blitz lays table cells out in the table's grid: rows and row
        // groups get no box of their own. Chromium's is the union of their
        // cells' (rows') boxes; Paprize measures rows.
        let by_node = &index;
        fn table_part(doc: &blitz_dom::BaseDocument, id: blitz_dom::NodeId) -> bool {
            doc.get_node(id)
                .and_then(|n| n.element_data())
                .is_some_and(|e| matches!(e.name.local.as_ref(), "tr" | "thead" | "tbody" | "tfoot"))
        }
        fn union(
            doc: &blitz_dom::BaseDocument,
            id: blitz_dom::NodeId,
            by_node: &FxHashMap<blitz_dom::NodeId, usize>,
            boxes: &mut [(u64, LayoutBox)],
        ) -> Option<Rect> {
            let node = doc.get_node(id)?;
            let mut acc: Option<Rect> = None;
            for &c in &node.children {
                let r = if table_part(doc, c) {
                    union(doc, c, by_node, boxes)
                } else {
                    by_node.get(&c).map(|&i| {
                        let b = boxes[i].1;
                        Rect::new(b.x, b.y, b.x + b.width, b.y + b.height)
                    })
                };
                if let Some(r) = r.filter(|r| r.width() > 0.0 || r.height() > 0.0) {
                    acc = Some(acc.map_or(r, |a| a.union(r)));
                }
            }
            let i = *by_node.get(&id)?;
            let b = &mut boxes[i].1;
            if b.width == 0.0 && b.height == 0.0 {
                if let Some(r) = acc {
                    *b = LayoutBox {
                        x: r.x0,
                        y: r.y0,
                        width: r.width(),
                        height: r.height(),
                        client_width: r.width(),
                        client_height: r.height(),
                        scroll_width: r.width(),
                        scroll_height: r.height(),
                    };
                }
            }
            Some(Rect::new(b.x, b.y, b.x + b.width, b.y + b.height))
        }
        let parts: Vec<blitz_dom::NodeId> = m
            .elements()
            .map(|(_, id)| id)
            .filter(|&id| {
                table_part(doc, id) && !doc.get_node(id).and_then(|n| n.parent).is_some_and(|p| table_part(doc, p))
            })
            .collect();
        for id in parts {
            union(doc, id, by_node, &mut boxes);
        }
        let root = doc.root_element();
        let (size, overflow) = (root.final_layout().size, *root.scrollable_overflow());
        timings.scene = t.elapsed();
        Ok(LayoutBoxes {
            boxes,
            document_width: (size.width as f64).max(overflow.x1).max(snap.width as f64),
            document_height: (size.height as f64).max(overflow.y1).max(snap.height as f64),
            timings,
        })
    }

    /// The document as a PDF: one page per element matching
    /// `opts.page_selector` (its border box, CSS px * 0.75 = pt), or the
    /// document cut into `opts.paper` sized pages when none matches.
    pub fn print_pdf(
        &mut self,
        snap: &Snapshot,
        fetcher: &Arc<dyn Fetcher>,
        opts: &PdfOptions,
    ) -> Result<PdfOutput, RenderError> {
        let mut timings = Timings::default();
        let dpr = snap.device_scale_factor.clamp(0.1, 16.0);
        let mut fresh = None;
        let (doc, _) = self.prepare(snap, fetcher, &mut fresh, &mut timings)?;
        let t = Instant::now();
        doc.set_viewport_scroll(blitz_dom::Point { x: 0.0, y: 0.0 });
        let root = doc.root_element();
        let (size, overflow) = (root.final_layout().size, *root.scrollable_overflow());
        let doc_w = (size.width as f64).max(overflow.x1).max(snap.width as f64) * dpr;
        let doc_h = (size.height as f64).max(overflow.y1).max(snap.height as f64) * dpr;
        let mut pages = Vec::new();
        for id in doc.query_selector_all(&opts.page_selector).unwrap_or_default() {
            let Some(rect) = element_bounds(doc, id, dpr) else { continue };
            // Off-screen boxes (Paprize's measuring page sits at -9999px) are
            // not printed, as in Chromium.
            if rect.width() < 1.0 || rect.height() < 1.0 || rect.x1 <= 0.0 || rect.y1 <= 0.0 {
                continue;
            }
            let (right, bottom, culprit) = content_extent(doc, id);
            let node_size = doc.get_node(id).map(|n| n.final_layout().size).unwrap_or_default();
            pages.push(PdfPage {
                rect,
                overflow_right: (right - node_size.width as f64).max(0.0),
                overflow_bottom: (bottom - node_size.height as f64).max(0.0),
                culprit,
            });
        }
        let by_selector = !pages.is_empty();
        if !by_selector {
            let (pw, ph) = (opts.paper.0 * dpr, opts.paper.1 * dpr);
            let mut y = 0.0;
            while y < doc_h - 0.5 || y == 0.0 {
                pages.push(PdfPage {
                    rect: Rect::new(0.0, y, pw, y + ph),
                    overflow_right: 0.0,
                    overflow_bottom: 0.0,
                    culprit: None,
                });
                y += ph;
            }
        }
        let extent_h = pages.iter().map(|p| p.rect.y1).fold(doc_h, f64::max);
        let extent_w = pages.iter().map(|p| p.rect.x1).fold(doc_w, f64::max);
        let mut scene = Scene::new();
        blitz_paint::paint_scene(&mut scene, doc, dpr, extent_w.ceil() as u32, extent_h.ceil() as u32, 0, 0);
        timings.scene = t.elapsed();
        let t = Instant::now();
        let rects: Vec<Rect> = pages.iter().map(|p| p.rect).collect();
        let (bytes, stats) =
            pdf::write(&scene, &rects, pdf::PX_TO_PT / dpr, &mut self.pdf_cache).map_err(RenderError::Resource)?;
        timings.raster = t.elapsed();
        Ok(PdfOutput { bytes, pages, by_selector, stats, commands: scene.commands.len(), timings })
    }

    /// Stage 3.
    pub fn raster(&mut self, painted: Painted, backend: Backend) -> (RgbaImage, Timings) {
        let Painted { scene, width, height, mut timings, .. } = painted;
        let mut data = vec![0u8; width as usize * height as usize * 4];
        let t = Instant::now();
        match backend {
            #[cfg(feature = "gpu")]
            Backend::Gpu => {
                let r = ensure(&mut self.gpu, width, height);
                r.render(
                    |p| {
                        p.reset();
                        p.append_scene(scene, Affine::IDENTITY);
                    },
                    &mut data,
                );
            }
            _ => {
                let r = ensure(&mut self.cpu, width, height);
                r.render(
                    |p| {
                        p.reset();
                        p.append_scene(scene, Affine::IDENTITY);
                    },
                    &mut data,
                );
            }
        }
        timings.raster = t.elapsed();
        (RgbaImage { width, height, data }, timings)
    }

    pub fn render(
        &mut self,
        snap: &Snapshot,
        area: Area,
        backend: Backend,
        fetcher: &Arc<dyn Fetcher>,
    ) -> Result<(RgbaImage, u64, Timings), RenderError> {
        let painted = self.paint(snap, area, fetcher)?;
        let hash = painted.hash;
        let (image, timings) = self.raster(painted, backend);
        Ok((image, hash, timings))
    }
}

/// How far the boxes inside `id` reach (CSS px, from its border box's
/// top-left corner): the largest right and bottom edge of any descendant
/// box or its content, transforms ignored, and the box that reaches
/// furthest past `id`'s own size.
fn content_extent(doc: &blitz_dom::BaseDocument, id: blitz_dom::NodeId) -> (f64, f64, Option<String>) {
    let size = doc.get_node(id).map(|n| n.final_layout().size).unwrap_or_default();
    let (mut right, mut bottom, mut worst, mut culprit) = (0.0f64, 0.0f64, 0.5f64, None);
    let mut stack = vec![(id, 0.0f64, 0.0f64)];
    while let Some((nid, x, y)) = stack.pop() {
        let Some(node) = doc.get_node(nid) else { continue };
        let l = node.final_layout();
        let (nx, ny) = if nid == id { (0.0, 0.0) } else { (x + l.location.x as f64, y + l.location.y as f64) };
        if nid != id {
            let o = node.scrollable_overflow();
            let r = nx + (l.size.width as f64).max(o.x1);
            let b = ny + (l.size.height as f64).max(o.y1);
            right = right.max(r);
            bottom = bottom.max(b);
            let past = (r - size.width as f64).max(b - size.height as f64);
            // `>=`: descendants come later, so the deepest box at that edge wins.
            if past >= worst {
                worst = past;
                culprit = Some(match node.element_data() {
                    Some(e) => format!(
                        "<{} {:?}> {nx:.0},{ny:.0} {:.0}x{:.0} overflow {:.0}x{:.0}",
                        e.name.local,
                        e.attr(blitz_dom::local_name!("class")).unwrap_or(""),
                        l.size.width,
                        l.size.height,
                        o.x1,
                        o.y1
                    ),
                    None => format!("text {:?}", node.text_content().chars().take(30).collect::<String>()),
                });
            }
        }
        if let Some(children) = node.layout_children.borrow().as_ref() {
            for &c in children.iter() {
                stack.push((c, nx, ny));
            }
        }
    }
    (right, bottom, culprit)
}

/// The transform from `id`'s border box to the document (CSS px), as
/// [`element_bounds`] computes it but from the unrounded layout, memoized
/// in `memo`.
fn node_transform(
    doc: &blitz_dom::BaseDocument,
    id: blitz_dom::NodeId,
    memo: &mut FxHashMap<blitz_dom::NodeId, Affine>,
) -> Option<Affine> {
    if let Some(t) = memo.get(&id) {
        return Some(*t);
    }
    let node = doc.get_node(id)?;
    let base = match node.containing_block() {
        Some(parent) => {
            let p = node_transform(doc, parent, memo)?;
            let scroll = doc.get_node(parent)?.scroll_offset();
            p * Affine::translate((-scroll.x, -scroll.y))
        }
        None => Affine::IDENTITY,
    };
    let location = node.unrounded_layout().location;
    let t = base
        * Affine::translate((location.x as f64, location.y as f64))
        * node.transform().as_deref().copied().unwrap_or_default();
    memo.insert(id, t);
    Some(t)
}

/// The border box of `id` in device px, document coordinates, through the
/// same translated/transformed positioning chain as painting (layout-only
/// coordinates crop the wrong region for translated bottom sheets and scaled
/// component ancestors).
pub(crate) fn element_bounds(doc: &blitz_dom::BaseDocument, id: blitz_dom::NodeId, dpr: f64) -> Option<Rect> {
    let size = doc.get_node(id)?.final_layout().size;
    let mut chain = Vec::new();
    let mut current = Some(id);
    while let Some(current_id) = current {
        let current_node = doc.get_node(current_id)?;
        chain.push(current_id);
        current = current_node.containing_block();
    }
    let mut transform = Affine::IDENTITY;
    for current_id in chain.into_iter().rev() {
        let current_node = doc.get_node(current_id)?;
        let location = current_node.final_layout().location;
        transform = transform
            * Affine::translate((location.x as f64 * dpr, location.y as f64 * dpr))
            * current_node.transform().as_deref().copied().unwrap_or_default();
        if current_id != id {
            let scroll = current_node.scroll_offset();
            transform *= Affine::translate((-scroll.x * dpr, -scroll.y * dpr));
        }
    }
    Some(transform.transform_rect_bbox(Rect::new(0.0, 0.0, size.width as f64 * dpr, size.height as f64 * dpr)))
}

fn viewport_of(snap: &Snapshot) -> BlitzViewport {
    let dpr = snap.device_scale_factor.clamp(0.1, 16.0);
    let physical = |css: u32| (css.max(1) as f64 * dpr).round() as u32;
    BlitzViewport::new(
        physical(snap.width),
        physical(snap.height),
        dpr as f32,
        if snap.dark { ColorScheme::Dark } else { ColorScheme::Light },
    )
}

fn viewport_key(snap: &Snapshot) -> (u32, u32, u64, bool) {
    (snap.width, snap.height, snap.device_scale_factor.to_bits(), snap.dark)
}

fn ensure<R: ImageRenderer>(slot: &mut Option<(R, (u32, u32))>, w: u32, h: u32) -> &mut R {
    match slot {
        Some((_, size)) if *size == (w, h) => {}
        Some((r, size)) => {
            r.resize(w, h);
            *size = (w, h);
        }
        None => *slot = Some((R::new(w, h), (w, h))),
    }
    &mut slot.as_mut().expect("renderer").0
}
