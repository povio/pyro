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
mod svg_use;
pub mod worker;

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
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

    /// Stages 1 and 2.
    pub fn paint(&mut self, snap: &Snapshot, area: Area, fetcher: &Arc<dyn Fetcher>) -> Result<Painted, RenderError> {
        let mut timings = Timings::default();
        let dpr = snap.device_scale_factor.clamp(0.1, 16.0);
        let (vw, vh) = (snap.width.max(1), snap.height.max(1));
        let physical = |css: f64| (css * dpr).round() as u32;

        let started = Instant::now();
        let mut fresh = None;
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
                // Use the same translated/transformed positioning chain as
                // painting. Layout-only coordinates crop the wrong region for
                // translated bottom sheets and scaled component ancestors.
                let mut chain = Vec::new();
                let mut current = Some(id);
                while let Some(current_id) = current {
                    let current_node = doc.get_node(current_id).ok_or(RenderError::NoElement)?;
                    chain.push(current_id);
                    current = current_node.containing_block();
                }
                let mut transform = Affine::IDENTITY;
                for current_id in chain.into_iter().rev() {
                    let current_node = doc.get_node(current_id).ok_or(RenderError::NoElement)?;
                    let location = current_node.final_layout().location;
                    transform = transform
                        * Affine::translate((location.x as f64 * dpr, location.y as f64 * dpr))
                        * current_node.transform().as_deref().copied().unwrap_or_default();
                    if current_id != id {
                        let scroll = current_node.scroll_offset();
                        transform *= Affine::translate((-scroll.x * dpr, -scroll.y * dpr));
                    }
                }
                let bounds = transform.transform_rect_bbox(Rect::new(
                    0.0,
                    0.0,
                    size.width as f64 * dpr,
                    size.height as f64 * dpr,
                ));
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
