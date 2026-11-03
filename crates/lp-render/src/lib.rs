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
mod geometry;
mod hash;
mod mirror;
mod net;
pub mod pdf;
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
pub use geometry::{BoxPosition, LayoutBox, StickyBox, TextWord};
pub use hash::scene_hash;
pub use mirror::{DeltaOp, MirrorStats, Rebuild, Tree, TreeKind, TreeNode, TOP};
pub use net::Fetcher;

/// Where document syncs spend their time (diagnostics, `LP.geometryStats`).
pub mod sync_profile {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    static MIRROR: AtomicU64 = AtomicU64::new(0);
    static SETTLE: AtomicU64 = AtomicU64::new(0);
    static RESOLVE: AtomicU64 = AtomicU64::new(0);

    pub(crate) fn record(mirror: Duration, settle: Duration, resolve: Duration) {
        MIRROR.fetch_add(mirror.as_nanos() as u64, Ordering::Relaxed);
        SETTLE.fetch_add(settle.as_nanos() as u64, Ordering::Relaxed);
        RESOLVE.fetch_add(resolve.as_nanos() as u64, Ordering::Relaxed);
    }

    /// Milliseconds applying node lists, settling sub-resources and resolving
    /// (style + layout) since the last call.
    /// As [`take`], without resetting.
    pub fn peek() -> (f64, f64, f64) {
        let ms = |a: &AtomicU64| a.load(Ordering::Relaxed) as f64 / 1e6;
        (ms(&MIRROR), ms(&SETTLE), ms(&RESOLVE))
    }

    pub fn take() -> (f64, f64, f64) {
        let ms = |a: &AtomicU64| a.swap(0, Ordering::Relaxed) as f64 / 1e6;
        (ms(&MIRROR), ms(&SETTLE), ms(&RESOLVE))
    }
}

/// Milliseconds per Blitz `resolve` phase on this thread (profiling).
/// Inline layouts Blitz built (shaped) on this thread, and their text bytes.
pub fn blitz_rebuilds() -> (u64, u64) {
    blitz_dom::phase_profile::rebuilds()
}

/// Nodes Blitz's style traversal visited on this thread.
pub fn blitz_restyles() -> u64 {
    blitz_dom::phase_profile::restyles()
}

/// Elements Blitz styled, matched, shared and reused on this thread.
pub fn blitz_style_stats() -> [u64; 4] {
    blitz_dom::phase_profile::style_stats_peek()
}

pub fn blitz_phases() -> [f64; 9] {
    blitz_dom::phase_profile::peek()
}

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
    /// `print` media (CDP `Emulation.setEmulatedMedia`): `@media print`
    /// rules apply instead of `screen` ones.
    pub print: bool,
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

pub struct LayoutBoxes {
    /// By render key (`lp_dom::snapshot::render_key`).
    pub boxes: Vec<(u64, LayoutBox)>,
    /// Each word of each text node (by its key), in layout order.
    pub words: Vec<(u64, TextWord)>,
    /// `position: sticky` elements (their boxes are laid out as if static).
    pub sticky: Vec<(u64, StickyBox)>,
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
    /// Paper margins when no element matches (CSS px: top, right, bottom,
    /// left): the document is cut into the content box's height.
    pub margins: [f64; 4],
    /// Content scale when no element matches (1 = 100%).
    pub scale: f64,
    /// Paint background colors and images.
    pub print_background: bool,
    /// Pages to keep, 1-based inclusive ranges in order; empty keeps all.
    pub page_ranges: Vec<(usize, usize)>,
}

impl PdfOptions {
    /// `page_selector` pages, else `paper` cuts without margins, at 100%,
    /// with backgrounds, every page.
    pub fn new(page_selector: impl Into<String>, paper: (f64, f64)) -> PdfOptions {
        PdfOptions {
            page_selector: page_selector.into(),
            paper,
            margins: [0.0; 4],
            scale: 1.0,
            print_background: true,
            page_ranges: Vec::new(),
        }
    }
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

/// A document's PDF pages painted but not written yet
/// ([`Renderer::paint_pdf_page`]): plain data, so another thread (the
/// render thread, [`Renderer::write_pdf`]) can write it.
pub struct PdfPaint {
    scene: Scene,
    pages: Vec<PdfPage>,
    by_selector: bool,
    /// Device px to points.
    scale: f64,
    placement: Option<pdf::Placement>,
    timings: Timings,
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
        collection: if FORK_SAFE_FONTS.load(std::sync::atomic::Ordering::Relaxed) {
            fork_safe_collection()
        } else {
            Collection::new(CollectionOptions { shared: false, system_fonts: true })
        },
    };
    font_ctx.collection.register_fonts(Blob::new(Arc::new(blitz_dom::BULLET_FONT) as _), None);
    Mutex::new(font_ctx)
});

static FORK_SAFE_FONTS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Loads system fonts without the platform font API, now, on this thread
/// (`serve --fork`). macOS's CoreText starts dispatch threads and Objective-C
/// initializers that a later `fork()` cannot survive ("+[NSFontCollection
/// initialize] may have been in progress"); a fork template, and children
/// that fork again, must not use it. Fonts are read from the system font
/// directories instead (memory-mapped), with the generic families and
/// script fallbacks the platform backend would pick. Call before any render.
pub fn use_fork_safe_fonts() {
    FORK_SAFE_FONTS.store(true, std::sync::atomic::Ordering::Relaxed);
    drop(process_fonts());
}

/// Whether [`use_fork_safe_fonts`] was called (fonts never touch CoreText).
pub fn fork_safe_fonts() -> bool {
    FORK_SAFE_FONTS.load(std::sync::atomic::Ordering::Relaxed)
}

/// The system fonts by directory scan: see [`use_fork_safe_fonts`].
fn fork_safe_collection() -> Collection {
    use parley::fontique::{GenericFamily, Script};
    let mut c = Collection::new(CollectionOptions { shared: false, system_fonts: false });
    let mut dirs: Vec<std::path::PathBuf> = if cfg!(target_os = "macos") {
        ["/System/Library/Fonts", "/Library/Fonts"].iter().map(Into::into).collect()
    } else {
        ["/usr/share/fonts", "/usr/local/share/fonts"].iter().map(Into::into).collect()
    };
    if let Some(home) = std::env::var_os("HOME") {
        let home = std::path::PathBuf::from(home);
        dirs.push(if cfg!(target_os = "macos") { home.join("Library/Fonts") } else { home.join(".local/share/fonts") });
    }
    c.load_fonts_from_paths(dirs.iter().filter(|d| d.is_dir()));
    let ids = |c: &mut Collection, names: &[&str]| -> Vec<parley::fontique::FamilyId> {
        names.iter().filter_map(|n| c.family_id(n)).collect()
    };
    // As fontique's CoreText backend (Chromium's macOS defaults), then
    // common Linux families.
    let generics: &[(GenericFamily, &[&str])] = &[
        (GenericFamily::Serif, &["Times", "Times New Roman", "DejaVu Serif", "Liberation Serif"]),
        (GenericFamily::SansSerif, &["Helvetica", "Arial", "DejaVu Sans", "Liberation Sans"]),
        (GenericFamily::Monospace, &["Courier", "Courier New", "Menlo", "DejaVu Sans Mono", "Liberation Mono"]),
        (GenericFamily::Cursive, &["Apple Chancery"]),
        (GenericFamily::Fantasy, &["Papyrus"]),
        (GenericFamily::SystemUi, &["System Font", ".SF NS", "SF Pro", "Helvetica Neue", "Helvetica", "DejaVu Sans"]),
        (GenericFamily::UiSansSerif, &["System Font", ".SF NS", "SF Pro", "Helvetica Neue", "Helvetica"]),
        (GenericFamily::UiSerif, &["New York", "Times"]),
        (GenericFamily::UiMonospace, &["SF Mono", "Menlo", "Courier"]),
        (GenericFamily::Emoji, &["Apple Color Emoji", "Noto Color Emoji"]),
        (GenericFamily::Math, &["STIX Two Math"]),
    ];
    for (generic, names) in generics {
        let families = ids(&mut c, names);
        c.set_generic_families(*generic, families.into_iter());
    }
    // Script fallbacks CoreText would answer for the usual scripts.
    let fallbacks: &[(&[u8; 4], &[&str])] = &[
        (b"Latn", &["Helvetica", "Lucida Grande", "Arial Unicode MS", "DejaVu Sans"]),
        (b"Grek", &["Helvetica", "Lucida Grande", "Arial Unicode MS", "DejaVu Sans"]),
        (b"Cyrl", &["Helvetica", "Lucida Grande", "Arial Unicode MS", "DejaVu Sans"]),
        (b"Hani", &["PingFang SC", "Heiti SC", "Hiragino Sans GB", "Noto Sans CJK SC"]),
        (b"Hans", &["PingFang SC", "Heiti SC", "Hiragino Sans GB", "Noto Sans CJK SC"]),
        (b"Hant", &["PingFang TC", "Heiti TC", "Noto Sans CJK TC"]),
        (b"Jpan", &["Hiragino Sans", "Hiragino Kaku Gothic ProN", "Noto Sans CJK JP"]),
        (b"Hira", &["Hiragino Sans", "Hiragino Kaku Gothic ProN", "Noto Sans CJK JP"]),
        (b"Kana", &["Hiragino Sans", "Hiragino Kaku Gothic ProN", "Noto Sans CJK JP"]),
        (b"Hang", &["Apple SD Gothic Neo", "Noto Sans CJK KR"]),
        (b"Kore", &["Apple SD Gothic Neo", "Noto Sans CJK KR"]),
        (b"Arab", &["Geeza Pro", "Noto Sans Arabic", "DejaVu Sans"]),
        (b"Hebr", &["Arial Hebrew", "Noto Sans Hebrew", "DejaVu Sans"]),
        (b"Thai", &["Thonburi", "Noto Sans Thai"]),
        (b"Deva", &["Kohinoor Devanagari", "Devanagari Sangam MN", "Noto Sans Devanagari"]),
        (b"Zsym", &["Apple Symbols", "Apple Color Emoji", "Noto Color Emoji"]),
        (b"Zsye", &["Apple Color Emoji", "Noto Color Emoji"]),
    ];
    for (script, names) in fallbacks {
        let families = ids(&mut c, names);
        c.set_fallbacks(Script::from_bytes(**script), families.into_iter());
    }
    c
}

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

/// UA rule that settles transitions and animations at their end values.
const NO_TRANSITIONS: &str = "*, *::before, *::after { transition: none !important; animation: none !important; }";

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
    /// Layout-query memos of persistent documents, by page.
    geometry: HashMap<u64, geometry::Geometry>,
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
            geometry: HashMap::new(),
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
        self.geometry.remove(&page);
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
        let mut config = DocumentConfig {
            base_url: Some(snap.base_url.clone()),
            net_provider: Some(provider),
            viewport: Some(viewport_of(snap)),
            font_ctx: Some(self.font_ctx.clone()),
            style_threading: StyleThreading::Sequential,
            media_type: Some(media_type(snap.print)),
            ..DocumentConfig::default()
        };
        // Documents are resolved at time 0: a CSS transition or animation
        // would stay at its first frame (a drawer sliding in stays off
        // screen). Layout queries, screenshots and PDFs take the end state.
        config.ua_stylesheets.push(blitz_dom::UserAgentStylesheet::parse(NO_TRANSITIONS));
        config
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
                    m.doc.set_media_type(media_type(snap.print));
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
        let mirrored = started.elapsed();
        let (fetch, passes, resolved_latest) =
            bridge.settle(doc, started + self.fetch_timeout, self.first_layout_wait, false);
        timings.layout_passes = passes;
        let resolving = Instant::now();
        if !resolved_latest {
            doc.resolve(0.0);
        }
        sync_profile::record(
            mirrored,
            started.elapsed().saturating_sub(mirrored + resolving.elapsed()),
            resolving.elapsed(),
        );
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

    /// Brings `snap`'s persistent document up to date for layout queries
    /// ([`Renderer::element_box`]): only what changed is applied and relaid
    /// out. `fetch`/`layout` time it.
    pub fn sync_layout(&mut self, snap: &Snapshot, fetcher: &Arc<dyn Fetcher>) -> Result<Timings, RenderError> {
        let live =
            snap.live.as_ref().ok_or_else(|| RenderError::BadArea("layout queries need a live snapshot".into()))?;
        let mut timings = Timings::default();
        let mut fresh = None;
        self.prepare(snap, fetcher, &mut fresh, &mut timings)?;
        self.geometry.entry(live.page).or_default().clear();
        Ok(timings)
    }

    /// [`Renderer::sync_layout`] from the changes since the page's last tree
    /// or delta (lp-dom's change journal) instead of the whole tree. `Err`
    /// (nothing applied) when the page has no document at this viewport or
    /// the delta does not fit it: sync the whole tree instead.
    pub fn sync_delta(
        &mut self,
        page: u64,
        ops: &[DeltaOp],
        viewport: (u32, u32, f64, bool, bool),
        style_only: bool,
    ) -> Result<Timings, RenderError> {
        let started = Instant::now();
        let mut timings = Timings::default();
        let m = self.mirrors.get_mut(&page).ok_or(RenderError::NoElement)?;
        let (w, h, dpr, dark, print) = viewport;
        if m.viewport != (w, h, dpr.to_bits(), dark, print) {
            return Err(RenderError::BadArea("the viewport changed".into()));
        }
        m.apply_delta(ops).map_err(|_| RenderError::BadArea("the delta does not fit the document".into()))?;
        let mirrored = started.elapsed();
        let mirror::Mirror { doc, bridge, .. } = m;
        let geometry_generation = doc.root_node().tree().geometry_generation();
        let (fetch, passes, resolved_latest) =
            bridge.settle(doc, started + self.fetch_timeout, self.first_layout_wait, style_only);
        let resolving = Instant::now();
        if !resolved_latest {
            match style_only {
                true => doc.resolve_style(0.0),
                false => doc.resolve(0.0),
            }
        }
        sync_profile::record(
            mirrored,
            started.elapsed().saturating_sub(mirrored + resolving.elapsed()),
            resolving.elapsed(),
        );
        timings.layout_passes = passes;
        timings.fetch = fetch;
        timings.layout = started.elapsed().saturating_sub(fetch);
        if !style_only || doc.root_node().tree().geometry_generation() != geometry_generation {
            self.geometry.entry(page).or_default().clear();
        }
        Ok(timings)
    }

    /// Puts `:hover` on the element keyed `hovered` and focus on `focused`
    /// in `page`'s persistent document (the page's pointer and focus, which
    /// a node list does not carry), restyling and relaying out what that
    /// changes.
    pub fn set_interaction(&mut self, page: u64, hovered: Option<u64>, focused: Option<u64>, style_only: bool) {
        let Some(m) = self.mirrors.get_mut(&page) else { return };
        let hover = hovered.and_then(|k| m.node(k));
        let focus = focused.and_then(|k| m.node(k));
        let mut changed = m.doc.set_hover_node(hover);
        if m.doc.get_focussed_node_id() != focus {
            match focus {
                Some(id) => {
                    m.doc.set_focus_to(id);
                }
                None => m.doc.clear_focus(),
            }
            changed = true;
        }
        if changed && style_only {
            let generation = m.doc.root_node().tree().geometry_generation();
            m.doc.resolve_style(0.0);
            if m.doc.root_node().tree().geometry_generation() != generation {
                self.geometry.entry(page).or_default().clear();
            }
        } else if changed {
            m.doc.resolve(0.0);
            self.geometry.entry(page).or_default().clear();
        }
    }

    /// The computed style of the element keyed `key` in `page`'s persistent
    /// document as of the last sync; one under `display: none` (which the
    /// traversal does not style) is resolved now. `None`: not mirrored.
    pub fn element_style(
        &self,
        page: u64,
        key: u64,
    ) -> Option<style::servo_arc::Arc<style::properties::ComputedValues>> {
        let m = self.mirrors.get(&page)?;
        let id = m.node(key)?;
        match m.doc.get_node(id)?.primary_styles() {
            Some(s) => Some((*s).clone()),
            None => m.doc.resolve_undisplayed_style(id),
        }
    }

    /// The box of the element keyed `key` in `page`'s persistent document,
    /// as of the last [`Renderer::sync_layout`] (see [`LayoutBox`]).
    pub fn element_box(&self, page: u64, key: u64) -> Option<LayoutBox> {
        let m = self.mirrors.get(&page)?;
        self.geometry.get(&page)?.element_box(&m.doc, m.node(key)?)
    }

    /// The element keyed `key` and its ancestors as Blitz laid them out
    /// (diagnostics).
    pub fn describe_element(&self, page: u64, key: u64) -> Option<String> {
        let m = self.mirrors.get(&page)?;
        Some(geometry::describe(&m.doc, m.node(key)?))
    }

    /// Every element's box ([`Renderer::element_box`]) after
    /// [`Renderer::sync_layout`], and the document's scroll size.
    pub fn layout_boxes(&mut self, snap: &Snapshot, fetcher: &Arc<dyn Fetcher>) -> Result<LayoutBoxes, RenderError> {
        let timings = self.sync_layout(snap, fetcher)?;
        let page = snap.live.as_ref().map(|l| l.page).unwrap_or_default();
        let mut out = self.page_layout(page).ok_or(RenderError::NoElement)?;
        out.timings = Timings { scene: out.timings.scene, ..timings };
        Ok(out)
    }

    /// `page`'s persistent document as last synced: every element's box
    /// ([`Renderer::element_box`]), every word, the sticky elements and the
    /// document's scroll size (at least the viewport).
    pub fn page_layout(&self, page: u64) -> Option<LayoutBoxes> {
        let t = Instant::now();
        let m = self.mirrors.get(&page)?;
        let g = self.geometry.get(&page)?;
        let doc = &m.doc;
        let keys = m.keys();
        let mut boxes = Vec::new();
        let mut sticky = Vec::new();
        // Tree order (paint order among equals for hit testing).
        let mut stack = vec![doc.root_element().id];
        while let Some(id) = stack.pop() {
            let Some(node) = doc.get_node(id) else { continue };
            stack.extend(node.children.iter().rev().copied());
            if !node.is_element() {
                continue;
            }
            // Not rendered (display: none, or inside it: never styled) has no box.
            let rendered = node.primary_styles().is_some_and(|s| !s.clone_display().is_none());
            if !rendered {
                continue;
            }
            let (Some(&key), Some(b)) = (keys.get(&id), g.element_box(doc, id)) else { continue };
            if b.position == BoxPosition::Sticky {
                sticky.push((key, g.sticky_box(doc, id)));
            }
            boxes.push((key, b));
        }
        let words = g.text_words(doc).into_iter().filter_map(|w| Some((*keys.get(&w.node)?, w))).collect();
        let (document_width, document_height) = self.document_size(page)?;
        Some(LayoutBoxes {
            boxes,
            words,
            sticky,
            document_width,
            document_height,
            timings: Timings { scene: t.elapsed(), ..Timings::default() },
        })
    }

    /// `page`'s document scroll size (at least the viewport), as last synced.
    pub fn document_size(&self, page: u64) -> Option<(f64, f64)> {
        let m = self.mirrors.get(&page)?;
        let doc = &m.doc;
        let root = doc.root_element();
        let (size, overflow) =
            (root.final_layout().size, root.scrollable_overflow().scale_from_origin(1.0 / doc.viewport().scale_f64()));
        let (vw, vh) = (m.viewport.0 as f64, m.viewport.1 as f64);
        Some(((size.width as f64).max(overflow.x1).max(vw), (size.height as f64).max(overflow.y1).max(vh)))
    }

    /// Whether any element of `page`'s document is `position: sticky`.
    pub fn has_sticky(&self, page: u64) -> bool {
        match (self.mirrors.get(&page), self.geometry.get(&page)) {
            (Some(m), Some(g)) => g.has_sticky(&m.doc),
            _ => false,
        }
    }

    /// The keys of the elements at viewport point (`x`, `y`) of `page`'s
    /// persistent document, topmost first (`document.elementsFromPoint`;
    /// the first is `elementFromPoint`'s).
    pub fn elements_from_point(&self, page: u64, x: f64, y: f64) -> Vec<u64> {
        let Some(m) = self.mirrors.get(&page) else { return Vec::new() };
        let keys = m.keys();
        let mut out = Vec::new();
        let mut cur = m.doc.element_from_point(x as f32, y as f32);
        while let Some(id) = cur {
            if let Some(&k) = keys.get(&id) {
                out.push(k);
            }
            cur = m.doc.get_node(id).and_then(|n| n.parent);
        }
        out
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
        let mut cache = std::mem::take(&mut self.pdf_cache);
        let out = match self.prepare(snap, fetcher, &mut fresh, &mut timings) {
            Ok((doc, _)) => print_document(doc, (snap.width, snap.height), dpr, opts, timings, &mut cache),
            Err(e) => Err(e),
        };
        self.pdf_cache = cache;
        out
    }

    /// [`Renderer::print_pdf`] of `page`'s persistent document as it is
    /// (brought up to date by [`Renderer::sync_layout`] or
    /// [`Renderer::sync_delta`]): the document layout queries were answered
    /// from is the one printed. `viewport`: CSS px; `dpr`: device scale.
    pub fn print_pdf_page(
        &mut self,
        page: u64,
        viewport: (u32, u32),
        dpr: f64,
        opts: &PdfOptions,
    ) -> Result<PdfOutput, RenderError> {
        let m = self.mirrors.get_mut(&page).ok_or(RenderError::NoElement)?;
        print_document(&mut m.doc, viewport, dpr.clamp(0.1, 16.0), opts, Timings::default(), &mut self.pdf_cache)
    }

    /// [`Renderer::print_pdf_page`] up to the PDF writing, which
    /// [`Renderer::write_pdf`] does (on any renderer).
    pub fn paint_pdf_page(
        &mut self,
        page: u64,
        viewport: (u32, u32),
        dpr: f64,
        opts: &PdfOptions,
    ) -> Result<PdfPaint, RenderError> {
        let m = self.mirrors.get_mut(&page).ok_or(RenderError::NoElement)?;
        paint_document(&mut m.doc, viewport, dpr.clamp(0.1, 16.0), opts, Timings::default())
    }

    /// The PDF of [`Renderer::paint_pdf_page`]'s pages.
    pub fn write_pdf(&mut self, paint: PdfPaint) -> Result<PdfOutput, RenderError> {
        write_pdf(paint, &mut self.pdf_cache)
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

/// Pages and PDF of a laid-out document ([`Renderer::print_pdf`]).
fn print_document(
    doc: &mut blitz_dom::BaseDocument,
    viewport: (u32, u32),
    dpr: f64,
    opts: &PdfOptions,
    timings: Timings,
    cache: &mut pdf::PdfCache,
) -> Result<PdfOutput, RenderError> {
    write_pdf(paint_document(doc, viewport, dpr, opts, timings)?, cache)
}

/// The pages of a laid-out document and its display list.
fn paint_document(
    doc: &mut blitz_dom::BaseDocument,
    viewport: (u32, u32),
    dpr: f64,
    opts: &PdfOptions,
    mut timings: Timings,
) -> Result<PdfPaint, RenderError> {
    let t = Instant::now();
    doc.set_viewport_scroll(blitz_dom::Point { x: 0.0, y: 0.0 });
    let root = doc.root_element();
    let (size, overflow) = (root.final_layout().size, *root.scrollable_overflow());
    let doc_w = (size.width as f64).max(overflow.x1).max(viewport.0 as f64) * dpr;
    let doc_h = (size.height as f64).max(overflow.y1).max(viewport.1 as f64) * dpr;
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
    let [top, right, bottom, left] = opts.margins;
    let scale = opts.scale.clamp(0.1, 2.0);
    // Paper cuts with margins or a scale put each cut into the paper's
    // content box (the page box stays the paper).
    let placement = (!by_selector && (opts.margins != [0.0; 4] || scale != 1.0)).then(|| pdf::Placement {
        paper: (opts.paper.0 * pdf::PX_TO_PT, opts.paper.1 * pdf::PX_TO_PT),
        offset: (left * pdf::PX_TO_PT, top * pdf::PX_TO_PT),
        scale,
    });
    if !by_selector {
        let content = ((opts.paper.0 - left - right).max(1.0), (opts.paper.1 - top - bottom).max(1.0));
        let (pw, ph) = (content.0 / scale * dpr, content.1 / scale * dpr);
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
    if !opts.page_ranges.is_empty() {
        let count = pages.len();
        if opts.page_ranges.iter().any(|&(from, _)| from > count) {
            return Err(RenderError::BadArea("Page range exceeds page count".into()));
        }
        let mut kept = Vec::new();
        for &(from, to) in &opts.page_ranges {
            kept.extend(pages[from - 1..to.min(count)].iter().cloned());
        }
        pages = kept;
    }
    let extent_h = pages.iter().map(|p| p.rect.y1).fold(doc_h, f64::max);
    let extent_w = pages.iter().map(|p| p.rect.x1).fold(doc_w, f64::max);
    let mut scene = Scene::new();
    let (w, h) = (extent_w.ceil() as u32, extent_h.ceil() as u32);
    blitz_paint::paint_scene_with(&mut scene, doc, dpr, w, h, 0, 0, opts.print_background);
    timings.scene = t.elapsed();
    Ok(PdfPaint { scene, pages, by_selector, scale: pdf::PX_TO_PT / dpr, placement, timings })
}

/// The PDF of painted pages.
fn write_pdf(paint: PdfPaint, cache: &mut pdf::PdfCache) -> Result<PdfOutput, RenderError> {
    let PdfPaint { scene, pages, by_selector, scale, placement, mut timings } = paint;
    let t = Instant::now();
    let rects: Vec<Rect> = pages.iter().map(|p| p.rect).collect();
    let (bytes, stats) = pdf::write(&scene, &rects, scale, placement, cache).map_err(RenderError::Resource)?;
    timings.raster = t.elapsed();
    Ok(PdfOutput { bytes, pages, by_selector, stats, commands: scene.commands.len(), timings })
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

fn viewport_key(snap: &Snapshot) -> (u32, u32, u64, bool, bool) {
    (snap.width, snap.height, snap.device_scale_factor.to_bits(), snap.dark, snap.print)
}

fn media_type(print: bool) -> style::media_queries::MediaType {
    match print {
        true => style::media_queries::MediaType::print(),
        false => style::media_queries::MediaType::screen(),
    }
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

#[cfg(test)]
mod settle_resolution_tests {
    use super::*;
    use lp_dom::parse::{parse_document, DomRef};
    use lp_dom::snapshot::{render_key, RenderHtml, RenderNodeKind};
    use std::{cell::RefCell, rc::Rc};

    struct NoFetch;
    impl Fetcher for NoFetch {
        fn fetch(&self, _url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
            done(Err("offline".into()));
        }
    }

    fn fixture(html: &str) -> (Snapshot, u64) {
        let dom: DomRef = Rc::new(RefCell::new(lp_dom::Dom::new()));
        let doc = parse_document(&dom, "http://test/", html);
        let d = dom.borrow();
        let target = render_key(d.get_element_by_id(doc, "box").unwrap().unwrap());
        let tree =
            d.render_tree(doc, &RenderHtml { style_text: &|_| None, stylesheet_href: &|_| None, mark: None }).unwrap();
        let tree = Tree {
            nodes: tree
                .nodes
                .into_iter()
                .map(|node| TreeNode {
                    key: node.key,
                    parent: node.parent,
                    kind: match node.kind {
                        RenderNodeKind::Element { name, attrs } => TreeKind::Element { name, attrs },
                        RenderNodeKind::Text(text) => TreeKind::Text(text),
                    },
                })
                .collect(),
        };
        (
            Snapshot {
                base_url: "http://test/".into(),
                width: 200,
                height: 100,
                device_scale_factor: 1.0,
                live: Some(Live { page: 1, tree: Arc::new(tree), clip: None }),
                ..Default::default()
            },
            target,
        )
    }

    #[test]
    fn settled_clean_prepare_resolves_once_with_identical_geometry_and_paint() {
        let (snap, key) = fixture("<!doctype html><style>body{margin:0}#box{width:50px;height:20px;background:blue}</style><div id=box></div>");
        let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
        let mut renderer = Renderer::new();
        let before = renderer.paint(&snap, Area::Viewport, &fetcher).unwrap().hash;
        let generation = renderer.mirrors[&1].doc.root_node().tree().geometry_generation();
        renderer.sync_layout(&snap, &fetcher).unwrap();
        let advances = renderer.mirrors[&1].doc.root_node().tree().geometry_generation() - generation;
        let bounds = renderer.element_box(1, key).unwrap();
        assert_eq!((bounds.x, bounds.y, bounds.width, bounds.height), (0.0, 0.0, 50.0, 20.0));
        assert_eq!(renderer.paint(&snap, Area::Viewport, &fetcher).unwrap().hash, before);
        eprintln!("clean prepare full resolve generation advances={advances}");
        assert_eq!(advances, 1, "settle already resolved the unchanged resource-free document");
    }

    #[test]
    fn rootless_live_prepare_and_delta_remain_safe() {
        let (snap, _) = fixture("<!doctype html><div id=box></div>");
        let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
        let mut renderer = Renderer::new();
        renderer.sync_layout(&snap, &fetcher).unwrap();
        let mut empty = snap.clone();
        empty.live.as_mut().unwrap().tree = Arc::new(Tree::default());
        renderer.sync_layout(&empty, &fetcher).unwrap();
        let mut renderer = Renderer::new();
        renderer.sync_layout(&snap, &fetcher).unwrap();
        renderer
            .sync_delta(
                1,
                &[DeltaOp::Children { parent: None, kids: Vec::new(), nodes: Vec::new() }],
                (200, 100, 1.0, false, false),
                false,
            )
            .unwrap();
    }

    type Delivery = Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>;
    #[derive(Default)]
    struct PartialState {
        trigger: bool,
        delivered: bool,
        image: Option<Delivery>,
    }
    #[derive(Default)]
    struct PartialResources {
        state: Mutex<PartialState>,
        held: Mutex<Vec<Delivery>>,
    }
    impl Fetcher for PartialResources {
        fn fetch(&self, url: String, done: Delivery) {
            let image = if url.ends_with("late.svg") {
                let mut state = self.state.lock().unwrap();
                if state.trigger {
                    state.delivered = true;
                    Some(done)
                } else {
                    state.image = Some(done);
                    None
                }
            } else if url.ends_with("trigger.ttf") {
                let image = {
                    let mut state = self.state.lock().unwrap();
                    state.trigger = true;
                    let image = state.image.take();
                    state.delivered |= image.is_some();
                    image
                };
                self.held.lock().unwrap().push(done);
                image
            } else {
                self.held.lock().unwrap().push(done);
                None
            };
            if let Some(done) = image {
                done(Ok(br#"<svg xmlns="http://www.w3.org/2000/svg" width="60" height="20"><rect width="60" height="20" fill="blue"/></svg>"#.to_vec()));
            }
        }
    }

    #[test]
    fn late_resource_after_fallback_is_resolved_when_another_resource_times_out() {
        // The font is first requested during fallback resolution. Deliver A
        // into Bridge's channel then, while B/font remain pending. Bridge
        // handles A on its next receive, then reaches the deadline with prior
        // positive passes: the final resolve must apply A's intrinsic size.
        let (snap,key) = fixture("<!doctype html><style>@font-face{font-family:SettlePartial;src:url(trigger.ttf)}body{margin:0;font-family:SettlePartial,serif}img{display:block}</style>font trigger<img id=box src=late.svg><img src=never.svg>");
        let partial = Arc::new(PartialResources::default());
        let fetcher: Arc<dyn Fetcher> = partial.clone();
        let mut renderer = Renderer::new();
        renderer.fetch_timeout = Duration::from_millis(80);
        renderer.first_layout_wait = Duration::ZERO;
        let timings = renderer.sync_layout(&snap, &fetcher).unwrap();
        let state = partial.state.lock().unwrap();
        assert!(state.trigger && state.delivered, "the late-resource control must actually deliver A during layout");
        assert!(timings.layout_passes > 0, "the timeout must follow a fallback pass");
        let bounds = renderer.element_box(1, key).unwrap();
        assert_eq!(
            (bounds.width, bounds.height),
            (60.0, 20.0),
            "newly handled image bytes need their final layout even with prior passes"
        );
    }

    #[test]
    fn style_only_color_delta_updates_style_without_full_resolve() {
        let source = "<!doctype html><style>body{margin:0}</style><div id=box style='width:50px;height:20px;color:blue'>text</div>";
        let (snap, key) = fixture(source);
        let (red, red_key) = fixture(&source.replace("color:blue", "color:red"));
        assert_eq!(key, red_key);
        let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
        let mut renderer = Renderer::new();
        renderer.sync_layout(&snap, &fetcher).unwrap();
        let old_color = renderer.element_style(1, key).unwrap().clone_color();
        let before = renderer.element_box(1, key).unwrap();
        let generation = renderer.mirrors[&1].doc.root_node().tree().geometry_generation();
        let kind = red.live.as_ref().unwrap().tree.nodes.iter().find(|n| n.key == key).unwrap().kind.clone();
        renderer.sync_delta(1, &[DeltaOp::Node { key, kind }], (200, 100, 1.0, false, false), true).unwrap();
        let advances = renderer.mirrors[&1].doc.root_node().tree().geometry_generation() - generation;
        let color = renderer.element_style(1, key).unwrap().clone_color();
        let mut whole = Renderer::new();
        whole.sync_layout(&red, &fetcher).unwrap();
        assert_ne!(color, old_color);
        assert_eq!(color, whole.element_style(1, key).unwrap().clone_color());
        assert_eq!(renderer.element_box(1, key).unwrap(), before);
        eprintln!("style-only color delta full resolve generation advances={advances}");
        assert_eq!(advances, 0, "a color-only style read must not perform a full layout");
    }

    #[test]
    fn style_only_width_delta_keeps_dirty_geometry_for_the_next_full_sync() {
        let source = "<!doctype html><div id=box style='width:50px;height:20px'></div>";
        let (snap, key) = fixture(source);
        let (wide, _) = fixture(&source.replace("width:50px", "width:130px"));
        let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
        let mut renderer = Renderer::new();
        renderer.sync_layout(&snap, &fetcher).unwrap();
        let kind = wide.live.as_ref().unwrap().tree.nodes.iter().find(|n| n.key == key).unwrap().kind.clone();
        renderer.sync_delta(1, &[DeltaOp::Node { key, kind }], (200, 100, 1.0, false, false), true).unwrap();
        let root = renderer.mirrors[&1].doc.root_element();
        assert!(
            root.has_damaged_descendants() || root.damage().is_some_and(|d| !d.is_empty()),
            "style-only sync must retain the width's layout damage"
        );
        renderer.sync_delta(1, &[], (200, 100, 1.0, false, false), false).unwrap();
        assert_eq!(renderer.element_box(1, key).unwrap().width, 130.0);
    }

    #[test]
    fn style_only_delta_still_resolves_used_container_query_geometry() {
        let source="<!doctype html><style>#box{container-type:inline-size;height:20px}#inner{width:10px;height:10px}@container(width>=150px){#inner{width:60px}}</style><div id=box style='width:200px'><div id=inner></div></div>";
        let (snap, key) = fixture(source);
        let (narrow, _) = fixture(&source.replace("width:200px", "width:100px"));
        let inner = snap
            .live
            .as_ref()
            .unwrap()
            .tree
            .nodes
            .iter()
            .find_map(|n| match &n.kind {
                TreeKind::Element { attrs, .. }
                    if attrs.iter().any(|(name, value)| &*name.local == "id" && value == "inner") =>
                {
                    Some(n.key)
                }
                _ => None,
            })
            .unwrap();
        let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
        let mut renderer = Renderer::new();
        renderer.sync_layout(&snap, &fetcher).unwrap();
        assert_eq!(renderer.element_box(1, inner).unwrap().width, 60.0);
        let kind = narrow.live.as_ref().unwrap().tree.nodes.iter().find(|n| n.key == key).unwrap().kind.clone();
        renderer.sync_delta(1, &[DeltaOp::Node { key, kind }], (200, 100, 1.0, false, false), true).unwrap();
        assert_eq!(renderer.element_box(1, key).unwrap().width, 100.0);
        assert_eq!(
            renderer.element_box(1, inner).unwrap().width,
            10.0,
            "query-dependent styles must use current container geometry"
        );
    }
    #[derive(Default)]
    struct RecordedResources(Mutex<Vec<String>>);
    impl Fetcher for RecordedResources {
        fn fetch(&self, url: String, done: Delivery) {
            self.0.lock().unwrap().push(url);
            done(Err("offline asset guard".into()));
        }
    }

    #[test]
    fn style_only_delta_discovers_background_and_mask_resources_without_layout() {
        let source = "<!doctype html><div id=box style='width:50px;height:20px'></div>";
        let (snap, key) = fixture(source);
        let (images, _) = fixture(
            &source.replace("height:20px", "height:20px;background-image:url(bg.svg);mask-image:url(mask.svg)"),
        );
        let recorded = Arc::new(RecordedResources::default());
        let fetcher: Arc<dyn Fetcher> = recorded.clone();
        let mut renderer = Renderer::new();
        renderer.sync_layout(&snap, &fetcher).unwrap();
        let before = renderer.element_box(1, key).unwrap();
        let generation = renderer.mirrors[&1].doc.root_node().tree().geometry_generation();
        let kind = images.live.as_ref().unwrap().tree.nodes.iter().find(|n| n.key == key).unwrap().kind.clone();
        renderer.sync_delta(1, &[DeltaOp::Node { key, kind }], (200, 100, 1.0, false, false), true).unwrap();
        let calls = recorded.0.lock().unwrap();
        assert!(calls.iter().any(|url| url.ends_with("bg.svg")), "style-only background resource must be discovered");
        assert!(calls.iter().any(|url| url.ends_with("mask.svg")), "style-only mask resource must be discovered");
        assert_eq!(renderer.mirrors[&1].doc.root_node().tree().geometry_generation() - generation, 0);
        assert_eq!(renderer.element_box(1, key).unwrap(), before);
    }
    #[test]
    fn settled_full_delta_resolves_once_with_current_geometry_and_paint() {
        let source="<!doctype html><style>body{margin:0}</style><div id=box style='width:50px;height:20px;background:blue'></div>";
        let (snap, key) = fixture(source);
        let (wide, _) = fixture(&source.replace("width:50px", "width:130px"));
        let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
        let mut renderer = Renderer::new();
        renderer.sync_layout(&snap, &fetcher).unwrap();
        let before = renderer.element_box(1, key).unwrap();
        let generation = renderer.mirrors[&1].doc.root_node().tree().geometry_generation();
        let kind = wide.live.as_ref().unwrap().tree.nodes.iter().find(|n| n.key == key).unwrap().kind.clone();
        renderer.sync_delta(1, &[DeltaOp::Node { key, kind }], (200, 100, 1.0, false, false), false).unwrap();
        let advances = renderer.mirrors[&1].doc.root_node().tree().geometry_generation() - generation;
        let after = renderer.element_box(1, key).unwrap();
        let mut whole = Renderer::new();
        whole.sync_layout(&wide, &fetcher).unwrap();
        assert_ne!(before, after, "real width mutation must be applied");
        assert_eq!(after, whole.element_box(1, key).unwrap());
        assert_eq!((after.width, after.height), (130.0, 20.0));
        assert_eq!(
            renderer.paint(&wide, Area::Viewport, &fetcher).unwrap().hash,
            whole.paint(&wide, Area::Viewport, &fetcher).unwrap().hash
        );
        eprintln!("clean full delta resolve generation advances={advances}");
        assert_eq!(advances, 1, "normal successful settling already resolved this mutation");
    }
}
