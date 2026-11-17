//! pyro-render: renders a page [`Snapshot`] to pixels with Blitz (html5ever,
//! Stylo, Taffy, Parley; painted by Vello) and compares the result against a
//! baseline. No JS engine and no pyro crate but pyro-canvas (the
//! process's fonts, shared with canvas text): the browser thread
//! builds the snapshot (`pyro_webapi::PageDom::render_snapshot`), this crate
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
mod hit;
mod mirror;
mod net;
mod page_box;
pub mod pdf;
mod svg_use;
pub mod worker;

use std::collections::HashMap;
use std::sync::Arc;
#[cfg(test)]
use std::sync::Mutex;
use stylo_taffy::CopyStyleGetters;

use std::time::{Duration, Instant};

use anyrender::recording::Scene;
use anyrender::{ImageRenderer, PaintScene};
use anyrender_vello_cpu::VelloCpuImageRenderer;
use blitz_dom::{DocumentConfig, StyleThreading};
use blitz_traits::shell::{ColorScheme, Viewport as BlitzViewport};
use kurbo::{Affine, Rect};
use parley::fontique::Blob;
use parley::FontContext;
use peniko::{Color, Fill};

pub use compare::{
    compare, decode_png, diff_image, encode_jpeg, encode_png, encode_rgb24, encode_yuv420p, CompareResult,
};
pub use geometry::{BoxPosition, BoxQuads, LayoutBox, StickyBox, TextWord};
pub use hash::scene_hash;
pub use hit::{HitProbe, HitVisit};
pub use mirror::{DeltaOp, MirrorStats, Rebuild, Tree, TreeKind, TreeNode, TOP};
pub use net::Fetcher;

/// Whether the diagnostic timers run (`Pyro.geometryStats`,
/// `Performance.getMetrics` durations): off until a client asks for them, so
/// layout syncs read no clocks only to add their readings up. Counters, and
/// the clocks behaviour depends on (fetch deadlines), are not affected.
pub mod profiling {
    use std::sync::atomic::{AtomicBool, Ordering};

    static ON: AtomicBool = AtomicBool::new(false);

    /// Turns the timers on for the rest of the process, Blitz's phase timers
    /// included.
    pub fn enable() {
        ON.store(true, Ordering::Relaxed);
        blitz_dom::phase_profile::set_timing(true);
    }

    /// Whether the timers are on.
    #[inline]
    pub fn on() -> bool {
        ON.load(Ordering::Relaxed)
    }
}

/// Where document syncs spend their time (diagnostics, `Pyro.geometryStats`;
/// only while [`profiling`] is on).
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

    /// As [`take`], without resetting.
    pub fn peek() -> (f64, f64, f64) {
        let ms = |a: &AtomicU64| a.load(Ordering::Relaxed) as f64 / 1e6;
        (ms(&MIRROR), ms(&SETTLE), ms(&RESOLVE))
    }

    /// Milliseconds applying node lists, settling sub-resources and resolving
    /// (style + layout) since the last call.
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

/// Layout-children collections and table context builds Blitz made on this thread.
pub fn blitz_constructions() -> (u64, u64) {
    blitz_dom::phase_profile::constructions()
}

/// Inline layouts Blitz broke into lines on this thread, the lines it kept from their last
/// break (resumed after text changed past them), and the text bytes the line breaker went over.
pub fn blitz_line_breaks() -> (u64, u64, u64) {
    blitz_dom::phase_profile::line_breaks()
}

/// Layout algorithm runs Blitz made to place children on this thread (cache
/// misses, not size measurements), and how many were the root element's.
pub fn blitz_performed_layouts() -> (u64, u64) {
    blitz_dom::phase_profile::performed_layouts()
}

/// `resolve` calls Blitz made on this thread, and how many of their layout passes ran Taffy.
pub fn blitz_passes() -> (u64, u64) {
    blitz_dom::phase_profile::passes()
}

/// Layout nodes Blitz's rounding reached on this thread, and subtrees it skipped.
pub fn blitz_rounding() -> (u64, u64) {
    blitz_dom::phase_profile::rounding()
}

/// Tables Blitz built from scratch on this thread, and the table cells it read from their styles
/// (by those builds and by updates of the rows that changed).
pub fn blitz_tables() -> (u64, u64) {
    blitz_dom::phase_profile::tables()
}

/// Table grids Blitz laid out on this thread: whole, and again from their last layout in the rows
/// that changed.
pub fn blitz_table_grids() -> (u64, u64) {
    blitz_dom::phase_profile::table_grids()
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
    /// Scroll offsets (CSS px) of elements of a [`Live`] document by key;
    /// those of other elements are reset. `None` leaves the document's.
    pub element_scroll: Option<Vec<(u64, (f64, f64))>>,
    /// How far each `position: sticky` element of a [`Live`] document is
    /// moved from its laid-out place (CSS px, by key), as the page's scroll
    /// positions have it; those of other elements are reset. `None` leaves
    /// the document's.
    pub sticky_offsets: Option<Vec<(u64, (f64, f64))>>,
    /// The current scroll targets of a [`Live`] document (CSS Overflow 5 § 2.1, scroll marker
    /// selection): each scroll container's key (`u64::MAX` for the viewport) and its target's.
    /// `None` leaves the document's.
    pub scroll_targets: Option<Vec<(u64, u64)>>,
    /// Straight RGBA painted under the document where it has no background
    /// (CDP `Emulation.setDefaultBackgroundColorOverride`); `None`: white.
    pub background: Option<[u8; 4]>,
    /// Bitmaps of the drawn canvases of a [`Live`] document, painted as
    /// their elements' content (other canvases paint nothing).
    pub canvases: Vec<CanvasImage>,
    /// The animated values of a [`Live`] document's elements now
    /// ([`Renderer::set_animations`]); `None` leaves the document's.
    pub animations: Option<Arc<AnimationFrame>>,
    /// An XML document (XHTML, SVG, other XML): selectors match element and
    /// attribute names case-sensitively. A [`Live`] document whose value
    /// changes is built anew.
    pub xml_document: bool,
    /// A document in quirks mode: percentage heights look through `auto`-height blocks for
    /// their basis (Quirks Mode §3.5). A [`Live`] document whose value changes is built anew.
    pub quirks_mode: bool,
    /// A document in limited-quirks mode: with [`Self::quirks_mode`], a line without text has no
    /// strut (Quirks Mode §3.3, the line height calculation quirk). A [`Live`] document whose
    /// value changes is built anew.
    pub limited_quirks_mode: bool,
    /// The document's HTTP `Content-Language` (one language tag), the last
    /// fallback for `:lang()`. A [`Live`] document whose value changes is
    /// built anew.
    pub content_language: Option<String>,
    /// The page's selection, painted highlighted in screenshots of a [`Live`]
    /// document ([`Renderer::paint`]; PDFs show none).
    pub selection: Option<TextSelection>,
    /// The page's custom highlights and target text, bottom first, painted under the selection
    /// in screenshots of a [`Live`] document.
    pub highlights: Vec<TextHighlight>,
}

/// A highlight of a page ([`Snapshot::highlights`]): a custom highlight's name (`None`: the
/// target text, `::target-text`) and its ranges.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextHighlight {
    pub name: Option<String>,
    pub ranges: Vec<TextSelection>,
}

/// A page's selection ([`Snapshot::selection`]): from `start` to `end` in
/// tree order, each the render key of a text node and an offset in UTF-16
/// code units of its data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextSelection {
    pub start: (u64, u32),
    pub end: (u64, u32),
}

pub use blitz_dom::animation;

/// The animated values of a persistent document's elements at one moment,
/// by element key (`blitz_dom::animation`): elements left out have none.
pub type AnimationFrame = Vec<(u64, animation::ElementEffects)>;

/// A canvas element's bitmap ([`Snapshot::canvases`]).
#[derive(Clone, Debug)]
pub struct CanvasImage {
    /// The canvas element's render key.
    pub key: u64,
    pub width: u32,
    pub height: u32,
    /// Straight-alpha RGBA rows. The same `Arc` as the previous render's
    /// means the pixels did not change.
    pub rgba: Arc<Vec<u8>>,
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
    /// Keys of the hovered and focused elements, put on the document before
    /// it is laid out ([`Renderer::set_interaction`]); `None` leaves the
    /// document's pointer and focus as they are.
    pub interaction: Option<(Option<u64>, Option<u64>)>,
}

/// Attribute the snapshot puts on the element a screenshot is clipped to.
pub const CLIP_ATTRIBUTE: &str = "data-pyro-render-clip";

pub struct LayoutBoxes {
    /// By render key (`pyro_dom::snapshot::render_key`).
    pub boxes: Vec<(u64, LayoutBox)>,
    /// `position: sticky` elements (their boxes are laid out as if static).
    pub sticky: Vec<(u64, StickyBox)>,
    pub document_width: f64,
    pub document_height: f64,
    /// `fetch`/`layout`: bringing the document up to date; `scene`: reading the boxes.
    pub timings: Timings,
}

/// Chrome's page geometry for a paper and margins in inches (top, right,
/// bottom, left), as [`PdfOptions::paper`] and [`PdfOptions::margins`] in CSS px;
/// `None` when no content area is left. Measured on Chrome 156 (Letter, A4,
/// Legal and small papers; margins from 0 to 1 in; scales 0.8 to 1.5):
///
/// - The paper is rounded up to whole points and margins down to whole points.
///   The content box is what is left of those, in points (0.4 in on Letter:
///   612 - 28 - 28 = 556 pt = 741.33 px).
/// - The written page size is that paper on a 300 dpi grid (A4's 596 x 842 pt
///   gives 2483 x 3508 dots, a 595.92 x 841.92 pt MediaBox, and 596 x 843 pt, Playwright's A4,
///   2483 x 3512).
/// - The content box's origin is the top and left margin in whole CSS px, rounded
///   (28 pt = 37.33 px starts at 37 px, 29 pt = 38.67 px at 39 px).
///
/// Scale does not enter: it divides the content box when laying out.
pub fn chrome_page_geometry(paper_in: (f64, f64), margins_in: [f64; 4]) -> Option<((f64, f64), [f64; 4])> {
    const PT_PER_IN: f64 = 72.0;
    const PT_PER_DOT: f64 = PT_PER_IN / 300.0;
    let paper_pt = ((paper_in.0 * PT_PER_IN).ceil(), (paper_in.1 * PT_PER_IN).ceil());
    let [top, right, bottom, left] = margins_in.map(|m| (m * PT_PER_IN).floor());
    let content_pt = (paper_pt.0 - left - right, paper_pt.1 - top - bottom);
    if content_pt.0 < 1.0 || content_pt.1 < 1.0 {
        return None;
    }
    let to_px = |pt: f64| pt / pdf::PX_TO_PT;
    // In single precision: 843 pt, 3512.5 dots exactly, is 3512 and 1191 pt, 4962.5, is 4963.
    let page_px = |pt: f64| {
        let dots = pt as f32 * (1.0 / PT_PER_DOT) as f32;
        to_px((dots as f64 + 0.5).floor() * PT_PER_DOT)
    };
    let paper = (page_px(paper_pt.0), page_px(paper_pt.1));
    let (origin_top, origin_left) = (to_px(top).round(), to_px(left).round());
    let margins = [
        origin_top,
        paper.0 - origin_left - to_px(content_pt.0),
        paper.1 - origin_top - to_px(content_pt.1),
        origin_left,
    ];
    Some((paper, margins))
}

/// Blink's layout units per CSS px.
const UNITS_PER_PX: f64 = 64.0;

fn whole_px_up(units: i64) -> f64 {
    (units as f64 / UNITS_PER_PX).ceil()
}

/// Paper and margins as Chrome's print settings hold them, whole points, for
/// [`PageArea::layout_sizes`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageArea {
    paper: (f32, f32),
    /// Top, right, bottom, left.
    margins: [f32; 4],
}

/// The [`PageArea`] of a paper and margins in inches.
pub fn chrome_page_area(paper_in: (f64, f64), margins_in: [f64; 4]) -> PageArea {
    PageArea {
        paper: ((paper_in.0 * 72.0).ceil() as f32, (paper_in.1 * 72.0).ceil() as f32),
        margins: margins_in.map(|m| (m * 72.0).floor() as f32),
    }
}

impl PageArea {
    /// What layout gets at `factor` (CSS px): the page area rounded up, which
    /// is the whole CSS px a page holds and what block-level content fills and percentages of it
    /// resolve against, and rounded down, which viewport units and what the initial
    /// containing block holds see (`LayoutView::GetLayoutSize`). Measured on Chrome 156
    /// by markers at 50% of each, on 192 papers, margins, scales and shrinks: A4 with
    /// no margins is 794.67 px, so 795 and 794; with 0.4 in margins 720.02, so 721
    /// and 720. A full-width link still ends at the page's 741.33 px on Letter with
    /// 0.4 in margins: it is 742 px wide, and clipped at what the page shows.
    ///
    /// The page area, as Blink computes it (`PageBoxDefaultSize`,
    /// `ResolvePageBoxGeometry`, `CreateConstraintSpaceForPageArea`), is in `f32`
    /// and layout units of 1/64 px: the paper in CSS px rounds to a unit, then
    /// the paper and each margin are scaled and truncate to a unit, and what the
    /// margins leave rounds up. A4 with 0.4 in margins is 1048 px by the numbers,
    /// but 842 pt is 1122.671875 in units and 28 pt is 37.328125, which leave
    /// 1048.015625: a page holds 1049 px. Measured on Chrome 156 by what fits in a
    /// page: 400 of 400 sizes (5 papers, 10 margins, 8 scales), and 198 of 198
    /// overflow widths.
    fn layout_sizes(&self, factor: f32) -> ((f64, f64), (f64, f64)) {
        let (w, h) = self.units(factor);
        let down = |units: i64| (units as f64 / UNITS_PER_PX).floor();
        ((whole_px_up(w), whole_px_up(h)), (down(w), down(h)))
    }

    /// How far down a page shows, before [`PageArea::layout_sizes`] rounds up (CSS px).
    /// Less than a unit on paper shows nothing, and a link in it has no
    /// annotation: measured 1 row in 1310.015625 px at scale 0.8 (0.0125 px on
    /// paper) is dropped, in 1048.015625 at scale 1 (0.015625) it is kept.
    fn shown_height(&self, factor: f32) -> f64 {
        let (_, units) = self.units(factor);
        let px = units as f64 / UNITS_PER_PX;
        let on_paper = px.fract() / factor as f64;
        if on_paper > 0.0 && on_paper * UNITS_PER_PX < 1.0 - 1e-3 {
            px.floor()
        } else {
            px
        }
    }

    /// The page area's size in layout units.
    fn units(&self, factor: f32) -> (i64, i64) {
        const UNITS: f32 = UNITS_PER_PX as f32;
        let px = |pt: f32| pt * 96.0 / 72.0;
        let scaled = |v: f32| (v * factor * UNITS) as i64;
        let side = |paper: f32, a: f32, b: f32| {
            let paper = (px(paper) * UNITS).round() / UNITS;
            scaled(paper) - scaled(px(a)) - scaled(px(b))
        };
        let [top, right, bottom, left] = self.margins;
        (side(self.paper.0, left, right), side(self.paper.1, top, bottom))
    }
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
    /// [`chrome_page_area`] of `paper` and `margins`; `None`: their difference.
    pub page_area: Option<PageArea>,
    /// Content scale when no element matches (1 = 100%).
    pub scale: f64,
    /// Paint background colors and images.
    pub print_background: bool,
    /// Pages to keep, 1-based inclusive ranges in order; empty keeps all.
    pub page_ranges: Vec<(usize, usize)>,
    /// `page_selector` pages go on `paper` as Chrome places them ([`page_box`]), rather than
    /// each being a page of its own size.
    pub place_on_paper: bool,
    /// The document prints in `print` media: not under an emulated `screen`.
    pub print_media: bool,
    /// Write a tag tree (headings, paragraphs, links) when no element
    /// matches (`generateTaggedPDF`, Chrome's default).
    pub tagged: bool,
}

impl PdfOptions {
    /// `page_selector` pages, else `paper` cuts without margins, at 100%,
    /// with backgrounds, every page.
    pub fn new(page_selector: impl Into<String>, paper: (f64, f64)) -> PdfOptions {
        PdfOptions {
            page_selector: page_selector.into(),
            paper,
            margins: [0.0; 4],
            page_area: None,
            scale: 1.0,
            print_background: true,
            page_ranges: Vec::new(),
            place_on_paper: false,
            print_media: true,
            tagged: true,
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
    /// Where the page goes on paper; without one the page is its rectangle's size.
    pub placement: Option<pdf::Placement>,
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
    /// The text runs painted into `scene`, in paint order.
    runs: Vec<blitz_paint::TextRun>,
    /// Paper cuts: links and structure ([`pdf::Extras`]).
    extras: Option<Box<pdf::Extras>>,
    /// The document was laid out again at the paper's width (and back).
    relaid: bool,
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
    /// its own scale factor. Without `beyond_viewport` the page keeps the
    /// snapshot's scroll position (fixed elements stay where the viewport
    /// shows them); with it the document is painted from its top-left
    /// corner (`captureBeyondViewport`).
    Clip { x: f64, y: f64, width: f64, height: f64, scale: f64, beyond_viewport: bool },
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
    /// Encoding of the returned images (PNG, JPEG), or their conversion to
    /// pyro's raw formats.
    pub encode: Duration,
    /// Writing the image to the capture's file (`Job::Capture` `out`).
    pub write: Duration,
    /// CPU time of the process (all its threads) while the capture rendered,
    /// encoded and wrote, measured in `worker::capture` (zero elsewhere).
    pub cpu: Duration,
}

impl Timings {
    pub fn total(&self) -> Duration {
        self.fetch + self.layout + self.scene + self.raster + self.decode + self.compare + self.encode + self.write
    }
}

/// Straight (not premultiplied) RGBA8. Opaque unless the snapshot's
/// [`Snapshot::background`] is translucent.
#[derive(Clone, Debug, PartialEq)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

/// Stages 1 and 2: the display list, not rasterized yet.
pub struct Painted {
    scene: Scene,
    /// Every pixel is opaque (raster output needs no un-premultiplying).
    opaque: bool,
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

/// Loads system fonts without the platform font API, now, on this thread
/// (`serve --fork`): see [`pyro_canvas::fonts::use_fork_safe_fonts`]. Call
/// before any render.
pub fn use_fork_safe_fonts() {
    pyro_canvas::fonts::use_fork_safe_fonts();
    drop(process_fonts());
}

/// [`use_fork_safe_fonts`] on a thread of its own, so the caller can start its
/// engine meanwhile (the font scan is half of a fork template's start). From
/// this call on no font use touches the platform API; one that comes before
/// the scan is done waits for it. Join the thread before any `fork`. `None`:
/// the thread could not start and the fonts were loaded here.
pub fn load_fork_safe_fonts() -> Option<std::thread::JoinHandle<()>> {
    pyro_canvas::fonts::choose_fork_safe_fonts();
    let started = std::thread::Builder::new().name("pyro-fonts".into()).spawn(use_fork_safe_fonts);
    if started.is_err() {
        use_fork_safe_fonts();
    }
    started.ok()
}

/// Whether [`use_fork_safe_fonts`] was called (fonts never touch CoreText).
pub fn fork_safe_fonts() -> bool {
    pyro_canvas::fonts::fork_safe_fonts()
}

/// The process's fonts (`pyro_canvas::fonts`, shared with canvas text), with
/// Blitz's list bullet font added once: every [`Renderer`] works on a clone.
fn process_fonts() -> FontContext {
    static BULLET: std::sync::Once = std::sync::Once::new();
    BULLET.call_once(|| pyro_canvas::fonts::register_fonts(Blob::new(Arc::new(blitz_dom::BULLET_FONT) as _)));
    pyro_canvas::fonts::font_context()
}

/// Warms the process-wide state a first render would otherwise pay for, on a
/// background thread: the system font scan, the system font database usvg
/// loads for the first inline SVG, Stylo's global data and the platform's
/// font fallback. Call once at startup, before any `fork`.
pub fn prewarm() -> std::io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new().name("pyro-render-warm".into()).spawn(|| {
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
/// Chrome's `<input type=file>` box ("Choose File · No file chosen"): Blitz
/// lays the control out with no content to size it by, so it would be
/// zero-sized and never visible or clickable.
const FILE_INPUT: &str = r#"input[type="file" i] { box-sizing: border-box; width: 253px; height: 21px; }"#;

static HYPHEN_DIGIT_BREAKS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Sets the [`Renderer::hyphen_digit_breaks`] of every renderer made from now
/// on, in this process (`serve --hyphen-digit-breaks`). Off by default.
pub fn set_hyphen_digit_breaks(on: bool) {
    HYPHEN_DIGIT_BREAKS.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// The default [`set_hyphen_digit_breaks`] set.
pub fn hyphen_digit_breaks() -> bool {
    HYPHEN_DIGIT_BREAKS.load(std::sync::atomic::Ordering::Relaxed)
}

static TEXT_ANTIALIASING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// Sets the [`Renderer::text_antialiasing`] of every renderer made from now on,
/// in this process (`serve --text-antialiasing off`). On by default.
pub fn set_text_antialiasing(on: bool) {
    TEXT_ANTIALIASING.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// The default [`set_text_antialiasing`] set.
pub fn text_antialiasing() -> bool {
    TEXT_ANTIALIASING.load(std::sync::atomic::Ordering::Relaxed)
}

/// Adds the font file at `path` (TTF/OTF/TTC) to the fonts of every page and
/// inline SVG in this process, as if it were installed (`serve --font`; WPT's
/// runner installs Ahem). Reads the file now and loads nothing else: call it
/// at startup, before any font use (a fork template may be made afterwards).
pub fn add_font_file(path: &std::path::Path) -> std::io::Result<()> {
    let bytes = Arc::new(std::fs::read(path)?);
    if skrifa::raw::FileRef::new(&bytes).is_err() {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "not a TrueType or OpenType font"));
    }
    pyro_canvas::fonts::add_startup_font(Blob::new(bytes.clone() as _));
    blitz_dom::util::add_svg_font(bytes);
    Ok(())
}

/// The coverage (out of 255) from which a pixel of aliased text is painted: half
/// of the pixel, as Skia paints a pixel whose centre the outline covers.
const TEXT_ALIASING_THRESHOLD: u8 = 128;

/// Fonts and raster backends, reused across renders on one thread.
pub struct Renderer {
    font_ctx: FontContext,
    /// Content hashes of font and image blobs (`scene_hash`).
    blob_hashes: HashMap<u64, u64>,
    cpu: Option<(VelloCpuImageRenderer, (u32, u32))>,
    /// The CPU backend for frames of [`THREADED_RASTER_PIXELS`] or more, on
    /// [`raster_threads`] threads of its own.
    cpu_threaded: Option<(VelloCpuImageRenderer, (u32, u32))>,
    #[cfg(feature = "gpu")]
    gpu: Option<(anyrender_vello::VelloImageRenderer, (u32, u32))>,
    resources: net::ResourceCache,
    /// Persistent documents of [`Live`] snapshots, by page.
    mirrors: HashMap<u64, mirror::Mirror>,
    /// Animated values to sample into a page's persistent document at its
    /// next sync ([`Renderer::set_animations`]).
    animations: HashMap<u64, Arc<AnimationFrame>>,
    pdf_cache: pdf::PdfCache,
    /// Layout queries of persistent documents, by page (present once the
    /// page's document has been laid out for them).
    geometry: HashMap<u64, geometry::Geometry>,
    /// External SVG sprite files of persistent documents, by resource
    /// scope and URL.
    sprites: HashMap<(String, String), Arc<String>>,
    /// External SVG `<image>`/`<feImage>` resources as `data:` URLs (`svg_use::resolve_images`),
    /// by resource scope and URL.
    svg_images: HashMap<(String, String), Arc<(String, String)>>,
    /// Longest wait for sub-resources per render.
    pub fetch_timeout: Duration,
    /// Longest wait, before the first layout, for the fonts and images the
    /// document asked for (a browser's font block period); sheets block it
    /// until `fetch_timeout`.
    pub first_layout_wait: Duration,
    /// Chrome's soft wrap opportunity between a hyphen and a digit when the
    /// hyphen follows an ASCII letter or digit (`4b87-|916e`, `10-|20`; UAX
    /// #14 LB25 has none), while a minus sign stays with its number (`-120`,
    /// `A -5`): [`parley::HYPHEN_DIGIT_LINE_BREAK_OVERRIDE`]. Line breaking
    /// and min/max-content sizes (table columns, flex items) both use it.
    /// Read when a document is created: a page's persistent document keeps the
    /// value it was built with. Defaults to [`hyphen_digit_breaks`].
    pub hyphen_digit_breaks: bool,
    /// Glyphs rasterized antialiased (the default), or aliased: each pixel
    /// painted fully or not at all, as Chromium draws text when WPT's runner
    /// starts it without font antialiasing (`NoFontAntialiasing`), which the
    /// WPT reftests are judged with. Other drawing is antialiased either way,
    /// and PDFs are not rasterized. Defaults to [`text_antialiasing`].
    pub text_antialiasing: bool,
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
            blob_hashes: HashMap::new(),
            cpu: None,
            cpu_threaded: None,
            #[cfg(feature = "gpu")]
            gpu: None,
            resources: net::ResourceCache::default(),
            mirrors: HashMap::new(),
            animations: HashMap::new(),
            pdf_cache: pdf::PdfCache::default(),
            geometry: HashMap::new(),
            sprites: HashMap::new(),
            svg_images: HashMap::new(),
            fetch_timeout: Duration::from_secs(10),
            first_layout_wait: Duration::from_secs(3),
            hyphen_digit_breaks: hyphen_digit_breaks(),
            text_antialiasing: text_antialiasing(),
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
        self.animations.remove(&page);
    }

    /// Samples `frame` into `page`'s persistent document at its next sync or
    /// render (made now if the document exists): the animated values of every
    /// animated element, keyed as the page's tree. Elements left out lose
    /// theirs.
    pub fn set_animations(&mut self, page: u64, frame: Arc<AnimationFrame>) {
        self.animations.insert(page, frame);
    }

    /// Hands `page`'s pending animated values to its document, which restyles
    /// the elements whose values changed at its next resolve.
    fn apply_animations(&mut self, page: u64) {
        let Some(m) = self.mirrors.get_mut(&page) else { return };
        let Some(frame) = self.animations.remove(&page) else { return };
        if frame.is_empty() && !m.doc.has_sampled_animations() {
            return;
        }
        let effects = frame.iter().filter_map(|(key, effects)| Some((m.node(*key)?, effects.clone()))).collect();
        m.doc.set_sampled_animations(effects);
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
        // (A page can remove its document element: nothing is laid out then.)
        let mut stack: Vec<_> = m.doc.try_root_element().map(|root| (root.id, 0usize)).into_iter().collect();
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
        self.mirrors.get(&page).map(|m| m.doc.try_root_element().map(|root| root.outer_html()).unwrap_or_default())
    }

    fn config(&self, snap: &Snapshot, provider: Arc<dyn blitz_traits::net::NetProvider>) -> DocumentConfig {
        let mut config = DocumentConfig {
            base_url: Some(snap.base_url.clone()),
            net_provider: Some(provider),
            viewport: Some(viewport_of(snap)),
            font_ctx: Some(self.font_ctx.clone()),
            style_threading: StyleThreading::Sequential,
            media_type: Some(media_type(snap.print)),
            line_break_override: self.hyphen_digit_breaks.then_some(parley::HYPHEN_DIGIT_LINE_BREAK_OVERRIDE),
            xml_document: snap.xml_document,
            quirks_mode: snap.quirks_mode,
            line_height_quirk: snap.quirks_mode || snap.limited_quirks_mode,
            content_language: snap.content_language.clone(),
            ..DocumentConfig::default()
        };
        // Documents are resolved at time 0: a CSS transition or animation
        // would stay at its first frame (a drawer sliding in stays off
        // screen). Layout queries, screenshots and PDFs take the end state.
        config.ua_stylesheets.push(blitz_dom::UserAgentStylesheet::parse(NO_TRANSITIONS));
        config.ua_stylesheets.push(blitz_dom::UserAgentStylesheet::parse(FILE_INPUT));
        config
    }

    /// The page's persistent document, brought up to `live.tree`.
    fn mirror(&mut self, snap: &Snapshot, live: &Live, fetcher: &Arc<dyn Fetcher>) -> MirrorStats {
        let key = viewport_key(snap);
        let reason = match self.mirrors.get_mut(&live.page) {
            None => Some(Rebuild::First),
            Some(m)
                if m.scope != snap.resource_scope
                    || m.base_url != snap.base_url
                    || m.facts.0 != snap.xml_document
                    || m.facts.1 != snap.content_language
                    || m.facts.2 != snap.quirks_mode
                    || m.facts.3 != (snap.quirks_mode || snap.limited_quirks_mode)
                    || m.doc.line_break_override().is_some() != self.hyphen_digit_breaks =>
            {
                Some(Rebuild::Document)
            }
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
    /// With `selection`, a live document paints [`Snapshot::selection`] until
    /// it is prepared without (a paint takes it off again).
    fn prepare<'a>(
        &'a mut self,
        snap: &Snapshot,
        fetcher: &Arc<dyn Fetcher>,
        fresh: &'a mut Option<(blitz_dom::BaseDocument, net::Bridge)>,
        timings: &mut Timings,
        selection: bool,
    ) -> Result<(&'a mut blitz_dom::BaseDocument, Option<blitz_dom::NodeId>), RenderError> {
        let started = Instant::now();
        let (doc, bridge, clip) = match &snap.live {
            Some(live) => {
                let stats = self.mirror(snap, live, fetcher);
                timings.mirror = Some(stats);
                if let Some(frame) = &snap.animations {
                    self.animations.insert(live.page, frame.clone());
                }
                self.apply_animations(live.page);
                let images = mirror::take_svg_image_hrefs();
                if let (false, Some(m)) = (images.is_empty(), self.mirrors.get_mut(&live.page)) {
                    svg_use::resolve_images(
                        &mut m.doc,
                        images,
                        &snap.base_url,
                        &snap.resource_scope,
                        &mut self.svg_images,
                        fetcher,
                        self.fetch_timeout,
                    );
                }
                let hrefs = mirror::take_sprite_hrefs();
                if let (false, Some(m)) = (hrefs.is_empty(), self.mirrors.get_mut(&live.page)) {
                    svg_use::resolve_sprites(
                        &mut m.doc,
                        hrefs,
                        &snap.base_url,
                        &snap.resource_scope,
                        &mut self.sprites,
                        fetcher,
                        self.fetch_timeout,
                    );
                }
                if let Some((hovered, focused)) = live.interaction {
                    // Restyled and laid out by the settle below.
                    self.mark_interaction(live.page, hovered, focused);
                }
                match self.mirrors.get_mut(&live.page) {
                    Some(m) => {
                        if let Some(offsets) = &snap.element_scroll {
                            m.set_element_scroll(offsets);
                        }
                        if let Some(offsets) = &snap.sticky_offsets {
                            m.set_sticky_offsets(offsets);
                        }
                        if let Some(targets) = &snap.scroll_targets {
                            m.set_scroll_targets(targets);
                        }
                        m.set_canvases(&snap.canvases);
                        // The selection's text nodes, both in the document.
                        let selection_paint = selection;
                        let selection = snap.selection.filter(|_| selection).and_then(|s| {
                            Some(blitz_dom::DomSelection {
                                start: (m.node(s.start.0)?, s.start.1 as usize),
                                end: (m.node(s.end.0)?, s.end.1 as usize),
                            })
                        });
                        m.doc.set_dom_selection(selection);
                        let highlights = snap
                            .highlights
                            .iter()
                            .filter(|_| selection_paint)
                            .map(|highlight| {
                                let name = match &highlight.name {
                                    Some(name) => blitz_dom::HighlightName::Custom(name.clone()),
                                    None => blitz_dom::HighlightName::TargetText,
                                };
                                let ranges = highlight
                                    .ranges
                                    .iter()
                                    .filter_map(|s| {
                                        Some(blitz_dom::DomSelection {
                                            start: (m.node(s.start.0)?, s.start.1 as usize),
                                            end: (m.node(s.end.0)?, s.end.1 as usize),
                                        })
                                    })
                                    .collect();
                                (name, ranges)
                            })
                            .collect();
                        m.doc.set_dom_highlights(highlights);
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
                for id in doc.query_selector_all("[data-pyro-scroll-left],[data-pyro-scroll-top]").unwrap_or_default() {
                    if let Some(node) = doc.get_node_mut(id) {
                        let offset = |name| {
                            node.attr(name).and_then(|s| s.parse::<f64>().ok()).filter(|v| v.is_finite()).unwrap_or(0.0)
                        };
                        let point = blitz_dom::Point {
                            x: offset("data-pyro-scroll-left".into()),
                            y: offset("data-pyro-scroll-top".into()),
                        };
                        *node.scroll_offset_mut() = point;
                    }
                }

                svg_use::resolve(&mut doc, &snap.base_url, fetcher, self.fetch_timeout)
                    .map_err(RenderError::Resource)?;
                let images = svg_use::image_hrefs(&doc);
                if !images.is_empty() {
                    svg_use::resolve_images(
                        &mut doc,
                        images,
                        &snap.base_url,
                        &snap.resource_scope,
                        &mut self.svg_images,
                        fetcher,
                        self.fetch_timeout,
                    );
                }
                let (doc, bridge) = fresh.insert((doc, bridge));
                let clip = doc.query_selector(&format!("[{CLIP_ATTRIBUTE}]")).ok().flatten();
                (doc, &*bridge, clip)
            }
        };
        let profiling = profiling::on();
        let mirrored = profiling.then(|| started.elapsed());
        let (fetch, passes, resolved_latest) =
            bridge.settle(doc, started + self.fetch_timeout, self.first_layout_wait, false);
        timings.layout_passes = passes;
        let resolving = profiling.then(Instant::now);
        if !resolved_latest {
            doc.resolve(0.0);
        }
        if let (Some(mirrored), Some(resolving)) = (mirrored, resolving) {
            let resolved = resolving.elapsed();
            sync_profile::record(mirrored, started.elapsed().saturating_sub(mirrored + resolved), resolved);
        }
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
        let (doc, clip) = self.prepare(snap, fetcher, &mut fresh, &mut timings, true)?;

        let t = Instant::now();
        // A persistent document keeps the scroll of its last capture; only
        // a viewport capture and a clip within the viewport are scrolled.
        let scrolled = matches!(area, Area::Viewport | Area::Clip { beyond_viewport: false, .. });
        if !scrolled {
            doc.set_viewport_scroll(blitz_dom::Point { x: 0.0, y: 0.0 });
        }
        // A document without a document element (a page can remove it) lays out nothing: the
        // capture is the empty canvas. (`root_element()` panicked: WPT
        // dom/nodes/crashtests/documentElement-remove-descendant-query.html.)
        let root = match doc.try_root_element() {
            Some(root) => {
                // The layout size is in CSS px, the scrollable overflow in device
                // px (Blitz scales it): back to CSS px before they are compared.
                let (size, overflow) = (
                    root.final_layout().size,
                    root.scrollable_overflow().scale_from_origin(1.0 / doc.viewport().scale_f64()),
                );
                let clipped = root.primary_styles().is_some_and(|s| {
                    matches!(
                        s.clone_overflow_y(),
                        style::values::computed::Overflow::Hidden | style::values::computed::Overflow::Clip
                    )
                });
                (size.width as f64, if clipped { size.height as f64 } else { (size.height as f64).max(overflow.y1) })
            }
            None => (0.0, 0.0),
        };
        // Where the image starts in the document (CSS px) and its size in device px.
        let (origin, scale, w, h) = match area {
            Area::Viewport => {
                doc.set_viewport_scroll(blitz_dom::Point { x: snap.scroll_x, y: snap.scroll_y });
                // Scroll markers and buttons follow the viewport's scroll (`:target-current`).
                if doc.tracks_scroll_states() {
                    doc.resolve(0.0);
                }
                ((0.0, 0.0), dpr, physical(vw as f64), physical(vh as f64))
            }
            Area::FullPage => {
                // The viewport's width, as Chromium's `captureBeyondViewport`:
                // the document's scrollable overflow also counts content wider
                // than the scroll containers that clip it (a wide table).
                let height = root.1.max(vh as f64);
                ((0.0, 0.0), dpr, physical(vw as f64), physical(height))
            }
            Area::Clip { x, y, width, height, scale, beyond_viewport } => {
                let s = dpr * scale.clamp(0.01, 16.0);
                // Blitz paints a scrolled document shifted by the scroll.
                let origin = match beyond_viewport {
                    true => (x, y),
                    false => {
                        doc.set_viewport_scroll(blitz_dom::Point { x: snap.scroll_x, y: snap.scroll_y });
                        if doc.tracks_scroll_states() {
                            doc.resolve(0.0);
                        }
                        (x - snap.scroll_x, y - snap.scroll_y)
                    }
                };
                (origin, s, (width * s).round() as u32, (height * s).round() as u32)
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
        let [r, g, b, a] = snap.background.unwrap_or([255; 4]);
        if a > 0 {
            let color = Color::from_rgba8(r, g, b, a);
            scene.fill(Fill::NonZero, Affine::IDENTITY, color, None, &Rect::new(0.0, 0.0, w as f64, h as f64));
        }
        let mut page = Scene::new();
        let extent_w = ((origin.0 * dpr) as u32).saturating_add((w as f64 * dpr / scale).ceil() as u32);
        let extent_h = ((origin.1 * dpr) as u32).saturating_add((h as f64 * dpr / scale).ceil() as u32);
        let transform = Affine::scale(scale / dpr) * Affine::translate((-origin.0 * dpr, -origin.1 * dpr));
        // (Painting knows where the page lands in the image's pixels: see `paint_scene_for_device`.)
        blitz_paint::paint_scene_for_device(&mut page, doc, dpr, extent_w, extent_h, 0, 0, transform);
        doc.set_dom_selection(None);
        doc.set_dom_highlights(Vec::new());
        scene.append_scene(page, transform);
        let hash = scene_hash(&scene, w, h, &mut self.blob_hashes);
        timings.scene = t.elapsed();
        Ok(Painted { scene, opaque: a == 255, hash, width: w, height: h, timings })
    }

    /// Brings `snap`'s persistent document up to date for layout queries
    /// ([`Renderer::element_box`]): only what changed is applied and relaid
    /// out. `fetch`/`layout` time it.
    pub fn sync_layout(&mut self, snap: &Snapshot, fetcher: &Arc<dyn Fetcher>) -> Result<Timings, RenderError> {
        let live =
            snap.live.as_ref().ok_or_else(|| RenderError::BadArea("layout queries need a live snapshot".into()))?;
        let mut timings = Timings::default();
        let mut fresh = None;
        self.prepare(snap, fetcher, &mut fresh, &mut timings, false)?;
        self.geometry.entry(live.page).or_default();
        Ok(timings)
    }

    /// [`Renderer::sync_layout`] from the changes since the page's last tree
    /// or delta (pyro-dom's change journal) instead of the whole tree. `Err`
    /// (nothing applied) when the page has no document at this viewport or
    /// the delta does not fit it: sync the whole tree instead. The returned
    /// [`Timings::layout`] is measured only while [`profiling`] is on.
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
        self.apply_animations(page);
        let m = self.mirrors.get_mut(&page).ok_or(RenderError::NoElement)?;
        let profiling = profiling::on();
        let mirrored = profiling.then(|| started.elapsed());
        let mirror::Mirror { doc, bridge, .. } = m;
        let geometry_generation = doc.root_node().tree().geometry_generation();
        let (fetch, passes, resolved_latest) =
            bridge.settle(doc, started + self.fetch_timeout, self.first_layout_wait, style_only);
        let resolving = profiling.then(Instant::now);
        if !resolved_latest {
            match style_only {
                true => doc.resolve_style(0.0),
                false => doc.resolve(0.0),
            }
        }
        timings.layout_passes = passes;
        timings.fetch = fetch;
        // Diagnostics only (`Pyro.geometryStats`): zero while profiling is off.
        if let (Some(mirrored), Some(resolving)) = (mirrored, resolving) {
            let resolved = resolving.elapsed();
            let total = started.elapsed();
            sync_profile::record(mirrored, total.saturating_sub(mirrored + resolved), resolved);
            timings.layout = total.saturating_sub(fetch);
        }
        if !style_only || doc.root_node().tree().geometry_generation() != geometry_generation {
            self.geometry.entry(page).or_default();
        }
        Ok(timings)
    }

    /// Puts `:hover` on the element keyed `hovered` and focus on `focused`
    /// in `page`'s persistent document (the page's pointer and focus, which
    /// a node list does not carry), restyling and relaying out what that
    /// changes.
    pub fn set_interaction(&mut self, page: u64, hovered: Option<u64>, focused: Option<u64>, style_only: bool) {
        let changed = self.mark_interaction(page, hovered, focused);
        self.settle_interaction(page, changed, style_only);
    }

    /// [`Renderer::set_interaction`] with `:active` on the element keyed
    /// `active` (and its ancestors) and whether the focused element matches
    /// `:focus-visible`.
    pub fn set_interaction_states(
        &mut self,
        page: u64,
        hovered: Option<u64>,
        focused: Option<u64>,
        active: Option<u64>,
        focus_visible: bool,
        style_only: bool,
    ) {
        // The ring first, so a newly focused node gets it as it is focused.
        let changed = match self.mirrors.get_mut(&page) {
            Some(m) => {
                let active = active.and_then(|k| m.node(k));
                let ring = m.doc.set_focus_ring(focus_visible);
                m.doc.set_active_to(active) || ring
            }
            None => false,
        };
        let changed = self.mark_interaction(page, hovered, focused) || changed;
        self.settle_interaction(page, changed, style_only);
    }

    fn settle_interaction(&mut self, page: u64, changed: bool, style_only: bool) {
        let Some(m) = self.mirrors.get_mut(&page) else { return };
        // Called on every geometry read: without news a pass would change nothing.
        let changed = changed || m.doc.needs_resolve();
        if changed && style_only {
            let generation = m.doc.root_node().tree().geometry_generation();
            m.doc.resolve_style(0.0);
            if m.doc.root_node().tree().geometry_generation() != generation {
                self.geometry.entry(page).or_default();
            }
        } else if changed {
            m.doc.resolve(0.0);
            self.geometry.entry(page).or_default();
        }
    }

    /// [`Renderer::set_interaction`] without the restyle: whether anything changed.
    fn mark_interaction(&mut self, page: u64, hovered: Option<u64>, focused: Option<u64>) -> bool {
        let Some(m) = self.mirrors.get_mut(&page) else { return false };
        let hover = hovered.and_then(|k| m.node(k));
        let focus = focused.and_then(|k| m.node(k));
        let mut changed = m.doc.set_hover_node(hover);
        if m.doc.focused_node() != focus {
            match focus {
                Some(id) => {
                    m.doc.set_focus_to(id);
                }
                None => m.doc.clear_focus(),
            }
            changed = true;
        }
        changed
    }

    /// The resolved value of `property` of the element keyed `key` in `page`'s persistent document as of
    /// the last sync (Blink's `getComputedStyle` value: used track sizes, a subgrid's line names; blitz-dom's
    /// `resolved_style_value`). `None`: not mirrored.
    pub fn resolved_style_value(&self, page: u64, key: u64, property: &str) -> Option<String> {
        let m = self.mirrors.get(&page)?;
        let id = m.node(key)?;
        Some(m.doc.resolved_style_value(id, property))
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

    /// Whether the element keyed `key` in `page`'s persistent document is
    /// outside every `display: none` subtree of the rendered (flat) tree, as
    /// of the last style sync, read from the styles Stylo keeps on the nodes
    /// instead of deriving a computed style per ancestor.
    ///
    /// Stylo styles an element only when its parent is displayed, and drops
    /// the styles under an element it styles `display: none`; an element
    /// moved under a hidden parent keeps its old styles, which the traversal
    /// does not reach. So the walk goes all the way up: some element on the
    /// way with `display: none` hides it. When none is found but an element
    /// on the way has no styles, the styles cannot tell, and this answers
    /// `None`, as it does for elements that are not mirrored or not in the
    /// document.
    pub fn element_displayed(&self, page: u64, key: u64) -> Option<bool> {
        let m = self.mirrors.get(&page)?;
        let mut id = m.node(key)?;
        let mut unstyled = false;
        let start = id;
        loop {
            let node = m.doc.get_node(id)?;
            if !node.is_element() {
                break;
            }
            if !node.flags.is_in_document() {
                return None;
            }
            // A drop-down select's descendants (its options) generate no
            // boxes, as in Chromium's menu list (their `display` stays).
            if id != start
                && node.data.downcast_element().is_some_and(|el| {
                    &*el.name.local == "select"
                        && el.attr(blitz_dom::local_name!("multiple")).is_none()
                        && !el
                            .attr(blitz_dom::local_name!("size"))
                            .and_then(|s| s.trim().parse::<u32>().ok())
                            .is_some_and(|s| s > 1)
                })
            {
                return Some(false);
            }
            match node.primary_styles() {
                Some(s) if s.clone_display().is_none() => return Some(false),
                Some(_) => {}
                None => unstyled = true,
            }
            match node.parent {
                Some(p) => id = p,
                None => break,
            }
        }
        (!unstyled).then_some(true)
    }

    /// pyro: whether `page`'s document records the elements whose
    /// primary style a restyle replaces
    /// (`BaseDocument::set_track_style_changes`); no page, nothing.
    pub fn set_track_style_changes(&mut self, page: u64, on: bool) {
        if let Some(m) = self.mirrors.get_mut(&page) {
            m.doc.set_track_style_changes(on);
        }
    }

    /// The mirrored elements of `page` whose primary style was replaced since
    /// the last call (while tracked), by render key, with the styles before
    /// and after (`BaseDocument::take_style_changes`); elements no longer
    /// mirrored are left out.
    pub fn take_style_changes(
        &mut self,
        page: u64,
    ) -> Vec<(
        u64,
        style::servo_arc::Arc<style::properties::ComputedValues>,
        style::servo_arc::Arc<style::properties::ComputedValues>,
    )> {
        let Some(m) = self.mirrors.get_mut(&page) else { return Vec::new() };
        let changes = m.doc.take_style_changes();
        changes.into_iter().filter_map(|c| Some((m.key_of(c.node)?, c.before, c.after))).collect()
    }

    /// The computed style of pseudo-element `pseudo` of the element keyed
    /// `key` in `page`'s persistent document (`getComputedStyle(el,
    /// pseudo)`; see `BaseDocument::pseudo_element_style`, which
    /// `generated_only` is passed to). `None`: not mirrored.
    pub fn element_pseudo_style(
        &self,
        page: u64,
        key: u64,
        pseudo: &style::selector_parser::PseudoElement,
        generated_only: bool,
    ) -> Option<style::servo_arc::Arc<style::properties::ComputedValues>> {
        let m = self.mirrors.get(&page)?;
        m.doc.pseudo_element_style(m.node(key)?, pseudo, generated_only)
    }

    /// `false` while no element of `page`'s persistent document has been
    /// styled with a generated `::before` / `::after`: then
    /// [`Renderer::element_pseudo_style`] with `generated_only` answers
    /// `None` for every element.
    pub fn may_have_generated_before_after(&self, page: u64) -> bool {
        self.mirrors.get(&page).is_some_and(|m| m.doc.may_have_generated_before_after())
    }

    /// The box of the element keyed `key` in `page`'s persistent document,
    /// as of the last [`Renderer::sync_layout`] (see [`LayoutBox`]).
    pub fn element_box(&self, page: u64, key: u64) -> Option<LayoutBox> {
        let m = self.mirrors.get(&page)?;
        self.geometry.get(&page)?.element_box(&m.doc, m.node(key)?)
    }

    /// The CSS boxes of the element keyed `key` in `page`'s persistent
    /// document as quads, as of the last [`Renderer::sync_layout`] (see
    /// [`BoxQuads`]).
    pub fn element_quads(&self, page: u64, key: u64) -> Option<BoxQuads> {
        let m = self.mirrors.get(&page)?;
        self.geometry.get(&page)?.element_quads(&m.doc, m.node(key)?)
    }

    /// pyro: the pieces of the border box of the element keyed `key` in `page`'s persistent
    /// document when column breaks split it, each where its column shows it (document CSS px:
    /// x, y, width, height), as of the last [`Renderer::sync_layout`]; `None` when it is whole.
    pub fn element_fragments(&self, page: u64, key: u64) -> Option<Vec<[f64; 4]>> {
        let m = self.mirrors.get(&page)?;
        let (g, id) = (self.geometry.get(&page)?, m.node(key)?);
        // pyro (lane H2): a box in a plane drawn through a perspective is reported whole, where
        // paint draws it (`Geometry::laid_out`).
        if !g.in_layout_tree(&m.doc, id) || g.projective(&m.doc, id).is_some() {
            return None;
        }
        let pieces = g.columns_pieces(&m.doc, id)?;
        (pieces.len() > 1).then(|| pieces.iter().map(|r| [r.x0, r.y0, r.width(), r.height()]).collect())
    }

    /// pyro: whether `page`'s persistent document may have boxes split by column breaks (a
    /// multi-column container has been laid out in it).
    pub fn has_fragmented_boxes(&self, page: u64) -> bool {
        self.mirrors.get(&page).is_some_and(|m| m.doc.has_multicol())
    }

    /// The element keyed `key` and its ancestors as Blitz laid them out
    /// (diagnostics).
    pub fn describe_element(&self, page: u64, key: u64) -> Option<String> {
        let m = self.mirrors.get(&page)?;
        Some(geometry::describe(&m.doc, m.node(key)?))
    }

    /// Every element's box ([`Renderer::element_box`]) after
    /// [`Renderer::sync_layout`], and the document's scroll size (the words:
    /// [`Renderer::page_words`]).
    pub fn layout_boxes(&mut self, snap: &Snapshot, fetcher: &Arc<dyn Fetcher>) -> Result<LayoutBoxes, RenderError> {
        let timings = self.sync_layout(snap, fetcher)?;
        let page = snap.live.as_ref().map(|l| l.page).unwrap_or_default();
        let mut out = self.page_layout(page).ok_or(RenderError::NoElement)?;
        out.timings = Timings { scene: out.timings.scene, ..timings };
        Ok(out)
    }

    /// `page`'s persistent document as last synced: every element's box
    /// ([`Renderer::element_box`]), the sticky elements and the document's
    /// scroll size (at least the viewport). The words, which cost a pass
    /// over every line, are [`Renderer::page_words`]'s.
    pub fn page_layout(&self, page: u64) -> Option<LayoutBoxes> {
        let t = Instant::now();
        let m = self.mirrors.get(&page)?;
        let g = self.geometry.get(&page)?;
        let doc = &m.doc;
        let mut boxes = Vec::new();
        let mut sticky = Vec::new();
        // Tree order (paint order among equals for hit testing).
        let mut stack: Vec<_> = doc.try_root_element().map(|root| root.id).into_iter().collect();
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
            let (Some(key), Some(b)) = (m.key_of(id), g.element_box(doc, id)) else { continue };
            if b.position == BoxPosition::Sticky {
                let (mut s, cb) = g.sticky_box(doc, id);
                s.cb = cb.and_then(|cb| m.key_of(cb));
                sticky.push((key, s));
            }
            boxes.push((key, b));
        }
        let (document_width, document_height) = self.document_size(page)?;
        Some(LayoutBoxes {
            boxes,
            sticky,
            document_width,
            document_height,
            timings: Timings { scene: t.elapsed(), ..Timings::default() },
        })
    }

    /// Every word of every text node of `page`'s persistent document as
    /// last synced (by the node's key), in layout order.
    pub fn page_words(&self, page: u64) -> Option<Vec<(u64, TextWord)>> {
        let m = self.mirrors.get(&page)?;
        let g = self.geometry.get(&page)?;
        Some(g.text_words(&m.doc).into_iter().filter_map(|w| Some((m.key_of(w.node)?, w))).collect())
    }

    /// [`Renderer::page_words`] for the text nodes below the element `key`
    /// (with others: see `Geometry::subtree_words`), reading only the inline
    /// formatting contexts in and around it. `None`: not a node of the page.
    pub fn subtree_words(&self, page: u64, key: u64) -> Option<Vec<(u64, TextWord)>> {
        let m = self.mirrors.get(&page)?;
        let g = self.geometry.get(&page)?;
        let within = m.node(key)?;
        Some(g.subtree_words(&m.doc, within).into_iter().filter_map(|w| Some((m.key_of(w.node)?, w))).collect())
    }

    /// The `position: sticky` elements of `page`'s persistent document as
    /// last synced (by key, with their sticky inputs): the set Blitz's style
    /// traversal keeps, not a pass over the document.
    pub fn sticky_elements(&self, page: u64) -> Option<Vec<(u64, StickyBox)>> {
        let m = self.mirrors.get(&page)?;
        let g = self.geometry.get(&page)?;
        let doc = &m.doc;
        let mut out: Vec<(u64, StickyBox)> = doc
            .sticky_node_ids()
            .filter_map(|id| {
                let node = doc.get_node(id)?;
                if !node.is_element() || !node.primary_styles().is_some_and(|s| !s.clone_display().is_none()) {
                    return None;
                }
                let key = m.key_of(id)?;
                let b = g.element_box(doc, id)?;
                (b.position == BoxPosition::Sticky).then(|| {
                    let (mut s, cb) = g.sticky_box(doc, id);
                    s.cb = cb.and_then(|cb| m.key_of(cb));
                    (key, s)
                })
            })
            .collect();
        out.sort_by_key(|(key, _)| *key);
        Some(out)
    }

    /// The elements of `page`'s persistent document whose box may hold
    /// `probe`, and their ancestors, in tree order (see [`HitVisit`] and
    /// `hit`): what pyro-dom hit-tests instead of every element's box.
    /// `shifted`: the keys of the elements pyro-dom moves (scrolled
    /// containers, sticky boxes), `offset`: how far it moves any element.
    /// `None` while the document's hit bounds are stale: call
    /// [`Self::ensure_hit_bounds`] first.
    pub fn hit_candidates(
        &self,
        page: u64,
        probe: &HitProbe,
        offset: &dyn Fn(u64) -> (f64, f64),
        shifted: &[u64],
    ) -> Option<Vec<HitVisit>> {
        let m = self.mirrors.get(&page)?;
        if m.doc.hit_overflow_is_stale() {
            return None;
        }
        let g = self.geometry.get(&page)?;
        Some(hit::candidates(g, m, probe, offset, shifted))
    }

    /// Brings the hit bounds of `page`'s persistent document up to date with
    /// its last sync (`BaseDocument::ensure_hit_overflow`: a sync only marks
    /// them stale), for [`Self::hit_candidates`].
    pub fn ensure_hit_bounds(&mut self, page: u64) {
        if let Some(m) = self.mirrors.get_mut(&page) {
            m.doc.ensure_hit_overflow();
        }
    }

    /// `page`'s document scroll size (at least the viewport), as last synced.
    pub fn document_size(&self, page: u64) -> Option<(f64, f64)> {
        let m = self.mirrors.get(&page)?;
        let doc = &m.doc;
        let (vw, vh) = (m.viewport.0 as f64, m.viewport.1 as f64);
        // Without a document element the document is as large as the viewport.
        let Some(root) = doc.try_root_element() else { return Some((vw, vh)) };
        let (size, overflow) =
            (root.final_layout().size, root.scrollable_overflow().scale_from_origin(1.0 / doc.viewport().scale_f64()));
        Some(((size.width as f64).max(overflow.x1).max(vw), (size.height as f64).max(overflow.y1).max(vh)))
    }

    /// Whether a style rule of `page`'s document depends on `:active`
    /// (Stylo's state dependencies, kept as sheets are flushed). `None`:
    /// not mirrored.
    pub fn styles_depend_on_active(&self, page: u64) -> Option<bool> {
        Some(self.mirrors.get(&page)?.doc.style_depends_on_state(style_dom::ElementState::ACTIVE))
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
    pub fn elements_from_point(&mut self, page: u64, x: f64, y: f64) -> Vec<u64> {
        let Some(m) = self.mirrors.get_mut(&page) else { return Vec::new() };
        let mut out = Vec::new();
        let mut cur = m.doc.element_from_point(x as f32, y as f32);
        while let Some(id) = cur {
            if let Some(k) = m.key_of(id) {
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
        let out = match self.prepare(snap, fetcher, &mut fresh, &mut timings, false) {
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
        let paint = self.paint_pdf_page(page, viewport, dpr, opts)?;
        write_pdf(paint, &mut self.pdf_cache)
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
        let paint = paint_document(&mut m.doc, viewport, dpr.clamp(0.1, 16.0), opts, Timings::default())?;
        if paint.relaid {
            self.geometry.entry(page).or_default();
        }
        Ok(paint)
    }

    /// The PDF of [`Renderer::paint_pdf_page`]'s pages.
    pub fn write_pdf(&mut self, paint: PdfPaint) -> Result<PdfOutput, RenderError> {
        write_pdf(paint, &mut self.pdf_cache)
    }

    /// Stage 3.
    pub fn raster(&mut self, painted: Painted, backend: Backend) -> (RgbaImage, Timings) {
        let Painted { scene, opaque, width, height, mut timings, .. } = painted;
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
                let threads =
                    if width as usize * height as usize >= THREADED_RASTER_PIXELS { raster_threads() } else { 0 };
                let slot = if threads > 0 { &mut self.cpu_threaded } else { &mut self.cpu };
                let r = ensure_cpu(slot, width, height, threads);
                r.set_text_aliasing_threshold((!self.text_antialiasing).then_some(TEXT_ALIASING_THRESHOLD));
                r.render(
                    |p| {
                        p.reset();
                        p.append_scene(scene, Affine::IDENTITY);
                    },
                    &mut data,
                );
            }
        }
        if !opaque {
            unpremultiply(&mut data);
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

/// Premultiplied RGBA8 (the rasterizers' output) to straight alpha.
fn unpremultiply(data: &mut [u8]) {
    for p in data.chunks_exact_mut(4) {
        let a = p[3] as u32;
        if a != 0 && a != 255 {
            for c in &mut p[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
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
    let mut pages = Vec::new();
    let page_ids = doc.query_selector_all(&opts.page_selector).unwrap_or_default();
    let style_sheets = if opts.place_on_paper && !page_ids.is_empty() { page_rule_texts(doc) } else { Vec::new() };
    for id in page_ids {
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
            placement: opts.place_on_paper.then(|| {
                let source = page_box::fixed_page_size(
                    style_sheets.iter().map(String::as_str),
                    inline_page_name(doc, id),
                    opts.print_media,
                );
                let placement = page_box::placement(opts.paper, source);
                // Above 100% Chrome lays out at a smaller page, which this path does not do.
                pdf::Placement { scale: placement.scale * opts.scale.min(1.0), ..placement }
            }),
        });
    }
    if pages.is_empty() {
        return paint_paper(doc, viewport, dpr, opts, timings, t);
    }
    let root = doc.root_element();
    let (size, overflow) = (root.final_layout().size, *root.scrollable_overflow());
    let doc_w = (size.width as f64).max(overflow.x1).max(viewport.0 as f64) * dpr;
    let doc_h = (size.height as f64).max(overflow.y1).max(viewport.1 as f64) * dpr;
    let pages = keep_ranges(pages, &opts.page_ranges)?;
    let extent_h = pages.iter().map(|p| p.rect.y1).fold(doc_h, f64::max);
    let extent_w = pages.iter().map(|p| p.rect.x1).fold(doc_w, f64::max);
    let mut scene = Scene::new();
    let (w, h) = (extent_w.ceil() as u32, extent_h.ceil() as u32);
    let runs = blitz_paint::paint_scene_recording_text(&mut scene, doc, dpr, w, h, 0, 0, opts.print_background);
    timings.scene = t.elapsed();
    let scale = pdf::PX_TO_PT / dpr;
    Ok(PdfPaint { scene, pages, by_selector: true, scale, runs, extras: None, relaid: false, timings })
}

/// The CSS text with `@page` rules of the document's `<style>` elements and `<link>` sheets, in
/// tree order. A `<link media>` wraps its text in that `@media`.
fn page_rule_texts(doc: &blitz_dom::BaseDocument) -> Vec<String> {
    let sheets = doc.query_selector_all("style, link").unwrap_or_default();
    sheets
        .into_iter()
        .filter_map(|id| {
            if let Some(css) = doc.linked_page_css(id) {
                let media = doc.get_node(id)?.attr(blitz_dom::local_name!("media"));
                return Some(media.map_or_else(|| css.to_owned(), |media| format!("@media {media}{{{css}}}")));
            }
            let node = doc.get_node(id)?;
            let is_style = node.element_data().is_some_and(|e| &*e.name.local == "style");
            is_style.then(|| node.text_content()).filter(|css| css.contains("@page"))
        })
        .collect()
}

/// The `page` of the nearest `style` attribute that has one, on `id` or an ancestor.
fn inline_page_name(doc: &blitz_dom::BaseDocument, id: blitz_dom::NodeId) -> Option<&str> {
    let mut current = doc.get_node(id);
    while let Some(node) = current {
        if let Some(name) = node.attr(blitz_dom::local_name!("style")).and_then(page_box::page_name) {
            return Some(name);
        }
        current = node.parent.and_then(|parent| doc.get_node(parent));
    }
    None
}

/// `pages` restricted to `ranges` (1-based, inclusive; empty keeps all).
fn keep_ranges(pages: Vec<PdfPage>, ranges: &[(usize, usize)]) -> Result<Vec<PdfPage>, RenderError> {
    if ranges.is_empty() {
        return Ok(pages);
    }
    let count = pages.len();
    if ranges.iter().any(|&(from, _)| from > count) {
        return Err(RenderError::BadArea("Page range exceeds page count".into()));
    }
    let mut kept = Vec::new();
    for &(from, to) in ranges {
        kept.extend(pages[from - 1..to.min(count)].iter().cloned());
    }
    Ok(kept)
}

/// [`paint_document`] without page boxes: the document cut into the paper's
/// content box. When the content box is not the viewport's width, the
/// document is styled and laid out once at that width for printing (and back
/// afterwards), as Chrome lays out the print document at the paper's width,
/// and once more wider when it overflows ([`shrink_to_fit`]). [`Breaks`] says
/// where the cuts go.
fn paint_paper(
    doc: &mut blitz_dom::BaseDocument,
    viewport: (u32, u32),
    dpr: f64,
    opts: &PdfOptions,
    mut timings: Timings,
    t: Instant,
) -> Result<PdfPaint, RenderError> {
    let [top, right, bottom, left] = opts.margins;
    let scale = opts.scale.clamp(0.1, 2.0);
    let content = ((opts.paper.0 - left - right).max(1.0), (opts.paper.1 - top - bottom).max(1.0));
    // The print layout's viewport (CSS px).
    let (cw, ch) = (content.0 / scale, content.1 / scale);
    let mut saved = None;
    // What layout gets at a scale: the content fills the page area rounded up, and
    // viewport units and the initial containing block see it rounded down.
    let sizes = |factor: f32| match opts.page_area {
        Some(area) => area.layout_sizes(factor),
        None => {
            let (w, h) = (content.0 * factor as f64, content.1 * factor as f64);
            ((w.ceil(), h.ceil()), (w.floor(), h.floor()))
        }
    };
    let relayout = |doc: &mut blitz_dom::BaseDocument,
                    saved: &mut Option<BlitzViewport>,
                    (available, initial): ((f64, f64), (f64, f64))| {
        let screen = saved.get_or_insert_with(|| doc.viewport().clone());
        let mut print = screen.clone();
        print.window_size = ((initial.0 * dpr).round().max(1.0) as u32, (initial.1 * dpr).round().max(1.0) as u32);
        doc.set_viewport(print);
        let pair = |s: (f64, f64)| (s.0 as f32, s.1 as f32);
        doc.set_layout_size(Some((pair(initial), pair(available))));
        doc.resolve(0.0);
    };
    let factor = 1.0 / scale as f32;
    let (holds, initial) = sizes(factor);
    let (holds_w, holds_h) = holds;
    if holds_w != viewport.0 as f64 || initial.0 != viewport.0 as f64 {
        relayout(doc, &mut saved, (holds, initial));
    }
    let width = document_width(doc);
    let shrink = shrink_to_fit(width, holds_w);
    // The scale Blink lays out at: in `f32`, as a page's size depends on it.
    let shrunk_factor = factor * (width.min(holds_w * MAX_SHRINK) as f32 / holds_w as f32);
    if shrink < 1.0 {
        relayout(doc, &mut saved, sizes(shrunk_factor));
    }
    let (cw, ch) = (cw / shrink, ch / shrink);
    let width = if saved.is_some() { cw } else { viewport.0 as f64 };
    let root = doc.root_element();
    let (size, overflow) = (root.final_layout().size, *root.scrollable_overflow());
    let doc_w = (size.width as f64).max(overflow.x1).max(width) * dpr;
    // The content's height, not the viewport's: a short document on a page
    // smaller than the viewport is one page, as in Chrome.
    let doc_h = (size.height as f64).max(overflow.y1) * dpr;
    let breaks_h = unrounded_document_height(root, doc_h, dpr);
    // A page holds whole CSS px, more than the content box shows.
    let page_h = if shrink < 1.0 { sizes(shrunk_factor).0 .1 } else { holds_h };
    let (pw, ph) = (cw * dpr, page_h * dpr);
    let breaks = Breaks::of(doc, dpr);
    let mut pages = Vec::new();
    let mut y = 0.0;
    while y < breaks_h - TOUCHING || pages.is_empty() {
        let (end, restart) = breaks.page_end(y, ph, breaks_h);
        pages.push(PdfPage {
            rect: Rect::new(0.0, breaks.painted(y), pw, breaks.painted(end)),
            overflow_right: 0.0,
            overflow_bottom: 0.0,
            culprit: None,
            placement: None,
        });
        y = breaks.next_page_start(end, restart, breaks_h);
    }
    let mut pages = match keep_ranges(pages, &opts.page_ranges) {
        Ok(pages) => pages,
        Err(e) => {
            // The screen layout comes back whatever happens to the print.
            if let Some(v) = saved {
                doc.set_layout_size(None);
                doc.set_viewport(v);
                doc.resolve(0.0);
            }
            return Err(e);
        }
    };
    let extent_h = pages.iter().map(|p| p.rect.y1).fold(doc_h, f64::max);
    let extent_w = pages.iter().map(|p| p.rect.x1).fold(doc_w, f64::max);
    let mut scene = Scene::new();
    let (w, h) = (extent_w.ceil() as u32, extent_h.ceil() as u32);
    let runs = blitz_paint::paint_scene_recording_text(&mut scene, doc, dpr, w, h, 0, 0, opts.print_background);
    let mut extras = pdf_extras(doc, &runs, dpr, opts.tagged);
    extras.slice_height = ph;
    extras.shown_height = match opts.page_area {
        Some(area) => area.shown_height(if shrink < 1.0 { shrunk_factor } else { factor }) * dpr,
        None => ch * dpr,
    };
    extras.canvas = if opts.print_background { canvas_color(doc) } else { None };
    let relaid = saved.is_some();
    if let Some(v) = saved {
        doc.set_layout_size(None);
        doc.set_viewport(v);
        doc.resolve(0.0);
    }
    timings.scene = t.elapsed();
    let placement = pdf::Placement {
        paper: (opts.paper.0 * pdf::PX_TO_PT, opts.paper.1 * pdf::PX_TO_PT),
        offset: (left * pdf::PX_TO_PT, top * pdf::PX_TO_PT),
        scale: scale * shrink,
    };
    for page in &mut pages {
        page.placement = Some(placement);
    }
    Ok(PdfPaint {
        scene,
        pages,
        by_selector: false,
        scale: pdf::PX_TO_PT / dpr,
        runs,
        extras: Some(Box::new(extras)),
        relaid,
        timings,
    })
}

/// Blink's `printingMaximumShrinkFactor`.
const MAX_SHRINK: f64 = 1.5;

/// The height pages are cut against: `doc_h`, the painted document's height,
/// before pixel snapping, as a box or margin reaching a fraction of a px into a
/// page gets that page in Chrome. Overflow that is only known snapped counts
/// when it reaches past the rest.
fn unrounded_document_height(root: &blitz_dom::Node, doc_h: f64, dpr: f64) -> f64 {
    if root.layout_data().writing_mode.is_vertical() {
        return doc_h;
    }
    let l = root.unrounded_layout();
    let h = (l.size.height as f64).max(l.scrollable_overflow_rect.rect().bottom as f64) * dpr;
    if doc_h > (h / dpr).round() * dpr {
        doc_h
    } else {
        h
    }
}

/// How far down a print layout goes to take in the document's overflow, as a
/// scale: the document's width (CSS px) past `limit`, the whole CSS px the
/// page's content box rounds up to, is laid out in and scaled down from, at
/// most by 1.5. Measured on Chrome 156: a 500 px document on a 495 px limit
/// scales by 0.99, a 2000 px one by 2/3.
fn shrink_to_fit(width: f64, limit: f64) -> f64 {
    if width > limit {
        limit / width.min(limit * MAX_SHRINK)
    } else {
        1.0
    }
}

/// The width of what the document lays out, overflow included (CSS px).
fn document_width(doc: &blitz_dom::BaseDocument) -> f64 {
    let Some(root) = doc.try_root_element() else { return 0.0 };
    let overflow = root.scrollable_overflow().scale_from_origin(1.0 / doc.viewport().scale_f64());
    (root.final_layout().size.width as f64).max(overflow.x1)
}

/// A span down the document a cut should stay out of (device px).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Unbroken {
    top: f64,
    bottom: f64,
    /// Where `top` is painted.
    painted: f64,
    /// A line box: its inline formatting context in [`Breaks::contexts`] and
    /// its index there.
    line: Option<(u32, u32)>,
    /// Only a `break-inside: avoid`: it can be broken as a last resort.
    avoid_only: bool,
}

/// A break between two boxes that `break-*: avoid` is on.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Avoided {
    /// The end of the box before and the content of the box after.
    from: f64,
    to: f64,
    /// The top of the box before: a page that starts below it cannot go back
    /// into it (measured).
    before_top: f64,
}

/// An inline formatting context's line boxes in [`Breaks::lines`].
#[derive(Clone, Copy, Debug, PartialEq)]
struct LineContext {
    first: u32,
    count: u32,
    orphans: u32,
    widows: u32,
}

/// Where paged content may be cut, from one walk of the layout tree (device px
/// down the document), before pixel snapping: whether a line fits is decided
/// by its fractional position, and [`Breaks::painted`] gives the position a
/// decision is painted at. Vertical writing modes keep their snapped positions.
/// Measured on Chrome 156:
///
/// - A box that does not fit what is left of a page moves to the next one
///   whole when it is a line box, a replaced element or `break-inside: avoid`,
///   however tall. At the top of a page it has nowhere to go: it is cut, and
///   what it holds (lines, nested avoided boxes) keeps its own constraints.
///   Margins, borders and padding above the first content of the page do not
///   count as content, an empty box of any height does. For a monolithic box
///   they do: any margin or parent's padding above it moves it to the next page.
/// - An inline formatting context's lines split at the lowest line that leaves
///   `orphans` on this page's part and `widows` for the rest. When both cannot
///   be had, `orphans` wins: a first part with fewer lines than `orphans` moves
///   the whole context to the next page (unless at the top), and the rest gets
///   what is left. Past the first part, `widows` taller than a page is given up.
/// - A margin that a cut falls in is dropped, and the next page starts at the
///   content after it. Forced breaks keep the margin of what follows them
///   (the margin above a `break-after` is dropped, however the two collapsed),
///   and have no effect above the first content of a page.
/// - Monolithic boxes hold no break at all, nor what they hold (Blink's
///   `LayoutBox::IsMonolithic`): replaced elements and form controls,
///   inline-level blocks, size containment and a writing mode other than the
///   parent's. `overflow` does not make a box monolithic when printing.
/// - A cut that lands between two boxes with `break-after` or `break-before:
///   avoid` (or `avoid-page`) on the box before, its last children, the box
///   after or its first children goes up to the last break above with nothing
///   wrong with it. If there is none, to the last that only breaks `orphans`
///   or `widows`; if there is none of those either, it stays.
#[derive(Debug, Default, PartialEq)]
struct Breaks {
    /// Sorted by top.
    unbroken: Vec<Unbroken>,
    lines: Vec<(f64, f64)>,
    contexts: Vec<LineContext>,
    /// Forced breaks, sorted, and where the page after each starts.
    forced: Vec<(f64, f64)>,
    /// `break-after` positions while walking, to fill into `forced`.
    forced_after: Vec<f64>,
    /// Top margins of in-flow boxes (their top, margin), sorted.
    margin_tops: Vec<(f64, f64)>,
    /// Between which positions a cut breaks a `break-*: avoid`, sorted: from the
    /// end of the box before to the content of the box after.
    avoided: Vec<Avoided>,
    /// Tops of in-flow boxes, sorted. Empty without `avoided`.
    tops: Vec<f64>,
    /// Margins between block boxes, merged and sorted.
    margins: Vec<(f64, f64)>,
    /// What comes above the content of block boxes (their top margin, border
    /// and padding), merged and sorted.
    heads: Vec<(f64, f64)>,
    dpr: f64,
}

impl Breaks {
    fn of(doc: &blitz_dom::BaseDocument, dpr: f64) -> Self {
        use style::values::computed::{BreakBetween, BreakWithin};
        use style::values::specified::box_::{Contain, ContainerType, DisplayInside, DisplayOutside};
        let forces = |b: BreakBetween| {
            matches!(b, BreakBetween::Page | BreakBetween::Always | BreakBetween::Left | BreakBetween::Right)
        };
        let mut out = Breaks { dpr, ..Breaks::default() };
        // Node, its parent's top (unrounded and painted), whether it takes
        // part in its parent's block flow and whether its parent's axes are
        // physical (`unrounded_layout` is in the placer's writing mode).
        let root = doc.root_element();
        let mut stack = vec![(root.id, 0.0f64, 0.0f64, true, !root.layout_data().writing_mode.is_vertical())];
        while let Some((id, parent_y, parent_painted, flow, physical)) = stack.pop() {
            let Some(node) = doc.get_node(id) else { continue };
            let painted = parent_painted + node.final_layout().location.y as f64;
            let l = if physical { node.unrounded_layout() } else { node.final_layout() };
            let y = if physical { parent_y + l.location.y as f64 } else { painted };
            let (top, bottom) = (y * dpr, (y + l.size.height as f64) * dpr);
            let styles = node.primary_styles();
            let in_flow = flow && in_block_flow(node);
            let (margin_top, margin_bottom) =
                ((l.margin.top as f64).max(0.0) * dpr, (l.margin.bottom as f64).max(0.0) * dpr);
            let children = node.layout_children.borrow();
            let parent = children.as_ref().is_some_and(|c| !c.is_empty());
            let inline = node.flags.is_inline_root();
            if in_flow {
                if bottom > top {
                    out.tops.push(top);
                }
                if margin_top > 0.0 && bottom > top {
                    out.margins.push((top - margin_top, top));
                    out.margin_tops.push((top, margin_top));
                }
                // The margins of an empty box collapse through it: its place
                // says nothing about them.
                if margin_bottom > 0.0 && bottom > top {
                    out.margins.push((bottom, bottom + margin_bottom));
                }
                if parent || inline {
                    let head = (l.border.top + l.padding.top) as f64 * dpr;
                    if margin_top + head > 0.0 {
                        out.heads.push((top - margin_top, top + head));
                    }
                }
            }
            let mut monolithic = false;
            if let Some(styles) = &styles {
                let b = styles.get_box();
                let display = styles.clone_display();
                let atomic_inline =
                    display.outside() == DisplayOutside::Inline && display.inside() != DisplayInside::Flow;
                if !atomic_inline {
                    if forces(b.slow_clone_break_before()) {
                        let at = top - if in_flow { margin_top } else { 0.0 };
                        out.forced.push((at, at));
                    }
                    if forces(b.slow_clone_break_after()) {
                        out.forced_after.push(bottom);
                    }
                }
                let avoids = matches!(b.slow_clone_break_inside(), BreakWithin::Avoid | BreakWithin::AvoidPage);
                let replaced = node.element_data().is_some_and(|e| {
                    matches!(
                        &*e.name.local,
                        "img"
                            | "svg"
                            | "canvas"
                            | "video"
                            | "iframe"
                            | "object"
                            | "input"
                            | "textarea"
                            | "select"
                            | "button"
                    )
                });
                let writes_otherwise = node
                    .parent
                    .and_then(|p| doc.get_node(p))
                    .and_then(|p| p.primary_styles())
                    .is_some_and(|p| p.writing_mode != styles.writing_mode);
                monolithic = replaced
                    || atomic_inline
                    || writes_otherwise
                    || b.slow_clone_contain().contains(Contain::SIZE)
                    || b.slow_clone_container_type().contains(ContainerType::SIZE);
                // An inline-level box is in a line, which already holds it.
                if avoids || (monolithic && !atomic_inline) {
                    out.unbroken.push(Unbroken {
                        top,
                        bottom,
                        painted: painted * dpr,
                        line: None,
                        avoid_only: !monolithic,
                    });
                }
            }
            if monolithic {
                continue;
            }
            let text = node.element_data().and_then(|e| e.inline_layout_data.as_ref());
            // The lines of vertical text run across the page, and are not what a page cut splits.
            if let Some(text) = text.filter(|_| inline && node.vertical_block_axis().is_none()) {
                let origin = y + (l.border.top + l.padding.top) as f64 + text.block_offset as f64;
                let snapped = node.final_layout();
                let painted_origin =
                    painted + (snapped.border.top + snapped.padding.top) as f64 + text.block_offset as f64;
                let scale = text.layout.scale() as f64;
                let first = out.lines.len();
                for line in text.layout.lines() {
                    let m = line.metrics();
                    let (min, max) = if physical && !node.layout_data().writing_mode.is_vertical() {
                        (m.unquantized_block_min_coord, m.unquantized_block_max_coord)
                    } else {
                        (m.block_min_coord, m.block_max_coord)
                    };
                    let a = (origin + min as f64 / scale) * dpr;
                    let b = (origin + max as f64 / scale) * dpr;
                    if b > a {
                        let at = (out.contexts.len() as u32, (out.lines.len() - first) as u32);
                        // The painter puts a line's top at a whole CSS pixel, as Chromium does.
                        let painted = if physical && !node.layout_data().writing_mode.is_vertical() {
                            blitz_paint::snapped_line_top(origin, m.unquantized_block_min_coord, scale) * dpr
                        } else {
                            (painted_origin + m.block_min_coord as f64 / scale) * dpr
                        };
                        out.unbroken.push(Unbroken { top: a, bottom: b, painted, line: Some(at), avoid_only: false });
                        out.lines.push((a, b));
                    }
                }
                let text_style = styles.as_ref().map(|s| s.get_inherited_text());
                let limit = |v: Option<i32>| v.map_or(2, |v| v.max(1) as u32);
                out.contexts.push(LineContext {
                    first: first as u32,
                    count: (out.lines.len() - first) as u32,
                    orphans: limit(text_style.map(|t| t.slow_clone_orphans().0)),
                    widows: limit(text_style.map(|t| t.slow_clone_widows().0)),
                });
            }
            if let Some(children) = children.as_ref() {
                let block_flow = in_flow
                    && styles.as_ref().is_some_and(|s| {
                        matches!(s.clone_display().inside(), DisplayInside::Flow | DisplayInside::FlowRoot)
                    });
                let physical = physical && !node.layout_data().writing_mode.is_vertical();
                for &c in children.iter() {
                    stack.push((c, y, painted, block_flow, physical));
                }
                if block_flow && !inline {
                    out.avoided_between_children(doc, children, y, physical, dpr);
                }
            }
        }
        out.finish();
        out
    }

    /// Notes the `break-*: avoid` between neighbours of a block flow.
    fn avoided_between_children(
        &mut self,
        doc: &blitz_dom::BaseDocument,
        children: &[blitz_dom::NodeId],
        y: f64,
        physical: bool,
        dpr: f64,
    ) {
        let layout = |n: &'_ blitz_dom::Node| if physical { *n.unrounded_layout() } else { *n.final_layout() };
        let mut before = None;
        for &c in children {
            let Some(node) = doc.get_node(c) else { continue };
            if !in_block_flow(node) {
                continue;
            }
            // A break before an empty box and one after it are in one place,
            // and one of them has nothing to avoid.
            if layout(node).size.height == 0.0 {
                before = None;
                continue;
            }
            if let Some(before) = before.and_then(|b| doc.get_node(b)) {
                if avoids_break(doc, before, Edge::After) || avoids_break(doc, node, Edge::Before) {
                    let (above, below) = (layout(before), layout(node));
                    self.avoided.push(Avoided {
                        from: (y + above.location.y as f64 + above.size.height as f64) * dpr,
                        to: (y + below.location.y as f64) * dpr,
                        before_top: (y + above.location.y as f64) * dpr,
                    });
                }
            }
            before = Some(c);
        }
    }

    /// Sorts and merges what the walk collected.
    fn finish(&mut self) {
        fn merged(spans: &mut Vec<(f64, f64)>) {
            spans.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut out: Vec<(f64, f64)> = Vec::with_capacity(spans.len());
            for &(a, b) in spans.iter() {
                match out.last_mut() {
                    Some(last) if a <= last.1 => last.1 = last.1.max(b),
                    _ => out.push((a, b)),
                }
            }
            *spans = out;
        }
        self.unbroken.sort_by(|a, b| a.top.total_cmp(&b.top).then(a.painted.total_cmp(&b.painted)));
        merged(&mut self.margins);
        merged(&mut self.heads);
        self.margin_tops.sort_by(|a, b| a.0.total_cmp(&b.0));
        // The page after a `break-after` starts at what follows less that
        // box's own margin (the largest of the boxes starting there, which
        // collapse). The margin above the break is gone.
        for at in std::mem::take(&mut self.forced_after) {
            let content = Self::past(&self.margins, at);
            let from = self.margin_tops.partition_point(|m| m.0 < content - TOUCHING);
            let own = self.margin_tops[from..]
                .iter()
                .take_while(|m| m.0 <= content + TOUCHING)
                .map(|m| m.1)
                .fold(0.0, f64::max);
            self.forced.push((at, content - own));
        }
        self.forced.sort_by(|a, b| a.0.total_cmp(&b.0));
        if self.avoided.is_empty() {
            self.tops = Vec::new();
        } else {
            for span in &mut self.avoided {
                span.to = Self::past(&self.heads, span.to);
            }
            self.avoided.sort_by(|a, b| a.from.total_cmp(&b.from));
            self.tops.sort_by(f64::total_cmp);
        }
    }

    /// The end of the span of `spans` (merged and sorted) `y` is in, else `y`.
    fn past(spans: &[(f64, f64)], y: f64) -> f64 {
        let i = spans.partition_point(|&(a, _)| a <= y);
        match i.checked_sub(1).map(|i| spans[i]) {
            Some((_, b)) if b > y => b,
            _ => y,
        }
    }

    /// Where a cut at `at` falls on the painted document: the highest painted
    /// top of what starts there, else `at` snapped as boxes are.
    fn painted(&self, at: f64) -> f64 {
        match self.unbroken.get(self.unbroken.partition_point(|u| u.top < at)) {
            Some(u) if u.top == at => u.painted,
            _ => (at / self.dpr).round() * self.dpr,
        }
    }

    /// Where the page after a cut at `end` starts: past a margin the cut fell
    /// in (a negative margin can bring content into it), or where a forced
    /// break says. `doc_h` when only margins are left.
    fn next_page_start(&self, end: f64, forced: Option<f64>, doc_h: f64) -> f64 {
        let next = self.unbroken.partition_point(|u| u.top < end);
        let start = Self::past(&self.margins, end).min(self.unbroken.get(next).map_or(f64::MAX, |u| u.top));
        if start >= doc_h - TOUCHING {
            return doc_h;
        }
        forced.unwrap_or(start)
    }

    /// Where the page starting at `y` ends with `ph` of room, and where the
    /// next starts when a forced break ends it.
    fn page_end(&self, y: f64, ph: f64, doc_h: f64) -> (f64, Option<f64>) {
        // Below this a box is at the top of the page.
        let first_content = Self::past(&self.heads, y);
        let mut end = y + ph;
        let next_forced = self.forced.partition_point(|f| f.0 <= first_content + TOUCHING);
        if let Some(&(at, restart)) = self.forced.get(next_forced).filter(|f| f.0 < end) {
            return (at, Some(restart));
        }
        if end >= doc_h {
            return (end, None);
        }
        // Only what starts on this page can move the cut. Each round moves it
        // up, and few do, so all pages together look at each span a few times.
        let from = self.unbroken.partition_point(|u| u.top < y);
        let mut last_resort = false;
        loop {
            let upto = from + self.unbroken[from..].partition_point(|u| u.top < end);
            let mut cut = end;
            for u in &self.unbroken[from..upto] {
                // A line less than a px over a cut that moved up overflows a
                // box a fraction shorter than it, and stays above the cut.
                let reach = if u.line.is_some() && end < y + ph { self.dpr } else { TOUCHING };
                // A monolithic box below the top of the page, by however little,
                // moves. What has lines or children to split stays under a margin.
                let monolithic = u.line.is_none() && !u.avoid_only;
                let top = if monolithic { y } else { first_content };
                if u.bottom > end + reach && u.top > top + TOUCHING && !(last_resort && u.avoid_only) {
                    cut = cut.min(u.top);
                }
                if let Some((context, index)) = u.line.filter(|_| u.bottom <= end + TOUCHING) {
                    cut =
                        cut.min(self.split_context(self.contexts[context as usize], index, y, ph, first_content, end));
                }
            }
            // What comes above content goes with it.
            let i = self.heads.partition_point(|&(a, _)| a < cut);
            if let Some(&(a, b)) = i.checked_sub(1).map(|i| &self.heads[i]) {
                if cut <= b && a > first_content + TOUCHING {
                    cut = a;
                }
            }
            if cut >= end {
                if self.avoided_at(end).is_none() {
                    return (end, None);
                }
                if let Some(better) = self.better_than_avoided(end, y, first_content, from) {
                    end = better;
                    continue;
                }
                // Every break here breaks an `avoid`: the last one. That may
                // be inside a `break-inside: avoid` further down.
                if last_resort {
                    return (end, None);
                }
                last_resort = true;
                end = y + ph;
                continue;
            }
            end = cut;
        }
    }

    /// The break of `avoided` a cut at `at` is.
    fn avoided_at(&self, at: f64) -> Option<Avoided> {
        let i = self.avoided.partition_point(|s| s.from <= at + TOUCHING);
        i.checked_sub(1).map(|i| self.avoided[i]).filter(|s| at <= s.to + TOUCHING)
    }

    /// When a cut at `end` breaks an `avoid`, the last break above it on the
    /// page (starting at `y`) that does not: first without breaking `orphans`
    /// or `widows`, then breaking them. Blink's `EarlyBreak`: the last break of
    /// the best `BreakAppeal`.
    fn better_than_avoided(&self, end: f64, y: f64, first_content: f64, from: usize) -> Option<f64> {
        let avoided = self.avoided_at(end)?;
        let above = avoided.from;
        if above <= first_content + TOUCHING || avoided.before_top < y - TOUCHING {
            return None;
        }
        // Lines, and whether a break above them keeps `orphans` and `widows`.
        let upto = from + self.unbroken[from..].partition_point(|u| u.top < above);
        let lines = self.unbroken[from..upto].iter().filter_map(|u| {
            let (context, index) = u.line?;
            let context = self.contexts[context as usize];
            let lines = &self.lines[context.first as usize..(context.first + context.count) as usize];
            let index = index as usize;
            // A context that began on an earlier page is not gone back into.
            if lines[0].0 < y {
                return None;
            }
            let fine = index >= context.orphans as usize && lines.len() - index >= context.widows as usize;
            Some((u.top, fine))
        });
        let mut breaks: Vec<(f64, bool)> = lines.collect();
        let tops = self.tops.partition_point(|&t| t < y)..self.tops.partition_point(|&t| t < above);
        breaks.extend(self.tops[tops].iter().map(|&t| (t, true)));
        breaks.sort_by(|a, b| b.0.total_cmp(&a.0));
        let allowed = |at: f64| {
            at > first_content + TOUCHING
                && self.avoided_at(at).is_none()
                && !self.unbroken[from..upto]
                    .iter()
                    .any(|u| u.line.is_none() && u.top > first_content + TOUCHING && u.top < at && at < u.bottom)
        };
        let best = |want_fine: bool| breaks.iter().find(|b| (b.1 || !want_fine) && allowed(b.0)).map(|b| b.0);
        best(true).or_else(|| best(false))
    }

    /// Where a page ending at `end` cuts `context`, given that its line
    /// `index` is the last fitting (`end` when it is not the context's last
    /// fitting line, or nothing has to move).
    fn split_context(&self, context: LineContext, index: u32, y: f64, ph: f64, first_content: f64, end: f64) -> f64 {
        let lines = &self.lines[context.first as usize..(context.first + context.count) as usize];
        let fit = index as usize + 1;
        if lines.get(fit).is_none_or(|next| next.1 <= end + TOUCHING) {
            return end;
        }
        let start = lines.partition_point(|l| l.0 < y);
        let (orphans, widows) = (context.orphans as usize, context.widows as usize);
        if start == 0 && fit < orphans && lines[0].0 > first_content + TOUCHING {
            return lines[0].0;
        }
        let widows_taller_than_page =
            lines.len() < widows || lines[lines.len() - 1].1 - lines[lines.len() - widows].0 > ph;
        let keep = if start > 0 && widows_taller_than_page {
            fit
        } else {
            (start + orphans).max(lines.len().saturating_sub(widows))
        };
        if keep < fit {
            lines[keep].0
        } else {
            end
        }
    }
}

/// Which edge of a box a break is at.
#[derive(Clone, Copy)]
enum Edge {
    Before,
    After,
}

/// Whether a box takes part in its parent's block flow.
fn in_block_flow(node: &blitz_dom::Node) -> bool {
    use style::computed_values::float::T as Float;
    use style::computed_values::position::T as Position;
    node.primary_styles().is_none_or(|s| {
        !matches!(s.clone_position(), Position::Absolute | Position::Fixed) && s.clone_float() == Float::None
    })
}

/// Whether a box, or at that edge what it holds in its block flow, says
/// `break-*: avoid` for page breaks. Each box is on at most one chain of last
/// children and one of first children, so all the calls together stay linear.
fn avoids_break(doc: &blitz_dom::BaseDocument, node: &blitz_dom::Node, edge: Edge) -> bool {
    use style::values::computed::BreakBetween;
    use style::values::specified::box_::DisplayInside;
    let mut node = node;
    loop {
        let Some(styles) = node.primary_styles() else { return false };
        let b = styles.get_box();
        let value = match edge {
            Edge::Before => b.slow_clone_break_before(),
            Edge::After => b.slow_clone_break_after(),
        };
        if matches!(value, BreakBetween::Avoid | BreakBetween::AvoidPage) {
            return true;
        }
        if node.flags.is_inline_root()
            || !matches!(
                styles.clone_display().inside(),
                DisplayInside::Flow | DisplayInside::FlowRoot | DisplayInside::Flex
            )
        {
            return false;
        }
        let children = node.layout_children.borrow();
        let Some(children) = children.as_ref() else { return false };
        let in_flow = |id: &&blitz_dom::NodeId| doc.get_node(**id).is_some_and(in_block_flow);
        let next = match edge {
            Edge::Before => children.iter().find(in_flow),
            Edge::After => children.iter().rev().find(in_flow),
        };
        let Some(next) = next.and_then(|&id| doc.get_node(id)) else { return false };
        node = next;
    }
}

/// Positions this close are the same one: less than Blink's 1/64 px layout
/// unit, the smallest overflow that moves a line to the next page (measured).
const TOUCHING: f64 = 0.01;

/// The canvas background (the root's, else the body's), when not transparent.
fn canvas_color(doc: &blitz_dom::BaseDocument) -> Option<[u8; 4]> {
    use blitz_dom::util::ToColorColor;
    let color = |id: blitz_dom::NodeId| {
        let styles = doc.get_node(id)?.primary_styles()?;
        let c = styles.get_background().background_color.resolve_to_absolute(&styles.clone_color());
        let c = c.as_color_color().to_rgba8();
        (c.a > 0).then_some([c.r, c.g, c.b, c.a])
    };
    let root = doc.try_root_element()?;
    color(root.id).or_else(|| {
        let body = root
            .children
            .iter()
            .copied()
            .find(|&c| doc.get_node(c).and_then(|n| n.element_data()).is_some_and(|e| &*e.name.local == "body"))?;
        color(body)
    })
}

/// The structure (headings, paragraphs, links) and link targets behind the
/// painted text `runs`.
fn pdf_extras(doc: &blitz_dom::BaseDocument, runs: &[blitz_paint::TextRun], dpr: f64, tagged: bool) -> pdf::Extras {
    use blitz_dom::local_name;
    let mut ex = pdf::Extras { tagged, ..pdf::Extras::default() };
    let root = doc.root_element();
    ex.lang = root.element_data().and_then(|e| e.attr(local_name!("lang"))).map(str::to_owned);
    // Element to its structure element.
    let mut structs: HashMap<blitz_dom::NodeId, u32> = HashMap::new();
    let kind_of = |id: blitz_dom::NodeId| -> Option<pdf::StructKind> {
        let e = doc.get_node(id)?.element_data()?;
        match &*e.name.local {
            "p" => Some(pdf::StructKind::Paragraph),
            "a" if e.attr(local_name!("href")).is_some() => Some(pdf::StructKind::Link(0)),
            h @ ("h1" | "h2" | "h3" | "h4" | "h5" | "h6") => Some(pdf::StructKind::Heading(h.as_bytes()[1] - b'0')),
            _ => None,
        }
    };
    let link_target = |id: blitz_dom::NodeId| -> Option<pdf::LinkTarget> {
        let href = doc.get_node(id)?.element_data()?.attr(local_name!("href"))?.trim();
        if let Some(fragment) = href.strip_prefix('#') {
            if fragment.is_empty() || fragment.eq_ignore_ascii_case("top") {
                return Some(pdf::LinkTarget::Internal(0.0, 0.0));
            }
            let target = doc
                .tree()
                .iter()
                .find_map(|(tid, n)| (n.element_data()?.attr(local_name!("id"))? == fragment).then_some(tid))?;
            let r = element_bounds(doc, target, dpr)?;
            return Some(pdf::LinkTarget::Internal(r.x0, r.y0));
        }
        let url = doc.base_url().join(href).ok()?;
        matches!(url.scheme(), "http" | "https" | "mailto" | "file" | "tel" | "ftp")
            .then(|| pdf::LinkTarget::Uri(url.to_string()))
    };
    // The structure element of `id` (made with its ancestors' first).
    fn structure(
        id: blitz_dom::NodeId,
        doc: &blitz_dom::BaseDocument,
        ex: &mut pdf::Extras,
        structs: &mut HashMap<blitz_dom::NodeId, u32>,
        kind_of: &dyn Fn(blitz_dom::NodeId) -> Option<pdf::StructKind>,
        link_target: &dyn Fn(blitz_dom::NodeId) -> Option<pdf::LinkTarget>,
        dpr: f64,
    ) -> Option<u32> {
        let mut cur = Some(id);
        while let Some(c) = cur {
            if let Some(&s) = structs.get(&c) {
                return Some(s);
            }
            if let Some(mut kind) = kind_of(c) {
                let parent = doc.get_node(c).and_then(|n| n.parent).and_then(|p| {
                    blitz_dom::util::ensure_stack(|| structure(p, doc, ex, structs, kind_of, link_target, dpr))
                });
                let index = ex.structs.len() as u32;
                if let pdf::StructKind::Link(_) = kind {
                    let node = doc.get_node(c)?;
                    // Boxes of their own (block, inline-block): the border box;
                    // inline links get their text's.
                    let boxed = node.primary_styles().is_some_and(|s| !s.clone_display().is_inline_flow());
                    let rects: Vec<Rect> = boxed.then(|| unrounded_bounds(doc, c, dpr)).flatten().into_iter().collect();
                    let boxed = !rects.is_empty();
                    match link_target(c) {
                        Some(target) => {
                            kind = pdf::StructKind::Link(ex.links.len());
                            ex.links.push(pdf::Link { rects, target, structure: index, boxed });
                        }
                        // Not a link we can write: its content goes to its parent.
                        None => {
                            structs.insert(c, parent?);
                            return parent;
                        }
                    }
                }
                ex.structs.push(pdf::Struct { kind, parent });
                structs.insert(c, index);
                return Some(index);
            }
            cur = doc.get_node(c).and_then(|n| n.parent);
        }
        None
    }
    for run in runs {
        let s = structure(run.node, doc, &mut ex, &mut structs, &kind_of, &link_target, dpr);
        ex.run_struct.push(s);
        // Inline links cover their text, one area per line.
        let mut cur = s;
        while let Some(i) = cur {
            let st = &ex.structs[i as usize];
            if let pdf::StructKind::Link(l) = st.kind {
                if !ex.links[l].boxed && run.bounds.width() > 0.0 {
                    let rects = &mut ex.links[l].rects;
                    match rects.last_mut() {
                        Some(last)
                            if last.y0 < run.bounds.y1 && run.bounds.y0 < last.y1 && run.bounds.x0 <= last.x1 + 1.0 =>
                        {
                            *last = last.union(run.bounds)
                        }
                        _ => rects.push(run.bounds),
                    }
                }
                break;
            }
            cur = st.parent;
        }
    }
    // Links with no text (images, empty boxes) that have a box.
    for id in doc.query_selector_all("a[href]").unwrap_or_default() {
        if !structs.contains_key(&id) {
            let boxed =
                doc.get_node(id).and_then(|n| n.primary_styles()).is_some_and(|s| !s.clone_display().is_inline_flow());
            if boxed {
                structure(id, doc, &mut ex, &mut structs, &kind_of, &link_target, dpr);
            }
        }
    }
    // Chrome writes a link's area in whole CSS px, boxes too: a box at 397.5 px starts at 398. Only
    // what the page shows clips it, which is why a full-width link ends at 741.33 px on Letter
    // with 0.4 in margins.
    let snap = |v: f64| (v / dpr).round() * dpr;
    for rect in ex.links.iter_mut().flat_map(|l| l.rects.iter_mut()) {
        *rect = Rect::new(snap(rect.x0), snap(rect.y0), snap(rect.x1), snap(rect.y1));
    }
    ex.links.retain(|l| !l.rects.is_empty());
    ex
}

/// The PDF of painted pages.
fn write_pdf(paint: PdfPaint, cache: &mut pdf::PdfCache) -> Result<PdfOutput, RenderError> {
    let PdfPaint { scene, pages, by_selector, scale, runs, extras, mut timings, .. } = paint;
    let t = Instant::now();
    let placed: Vec<(Rect, Option<pdf::Placement>)> = pages.iter().map(|p| (p.rect, p.placement)).collect();
    let (bytes, stats) =
        pdf::write(&scene, &placed, scale, &runs, extras.as_deref(), cache).map_err(RenderError::Resource)?;
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
    bounds_of(doc, id, dpr, true)
}

/// [`element_bounds`] before pixel snapping (a link's area in a print layout,
/// as Chrome writes it).
fn unrounded_bounds(doc: &blitz_dom::BaseDocument, id: blitz_dom::NodeId, dpr: f64) -> Option<Rect> {
    bounds_of(doc, id, dpr, false)
}

fn bounds_of(doc: &blitz_dom::BaseDocument, id: blitz_dom::NodeId, dpr: f64, snapped: bool) -> Option<Rect> {
    let layout = |n: &blitz_dom::Node| if snapped { *n.final_layout() } else { doc.physical_unrounded_layout(n.id) };
    let size = layout(doc.get_node(id)?).size;
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
        let location = layout(current_node).location;
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

/// Frames of at least this many device px (3840x2160 is 8.3 M) are rasterized
/// on [`raster_threads`] threads. vello_cpu's threads cost CPU time for the
/// hand-offs: a 3840x2160 page took 20-60 % more of it, in 0.35-0.45 of the
/// time; at 1280x720 twice the CPU for little time saved (M1 Max).
const THREADED_RASTER_PIXELS: usize = 4_000_000;

/// Raster threads for large frames: `PYRO_RASTER_THREADS` (0: none, every
/// frame on the render thread), else vello_cpu's default, one less than the
/// cores, at most 8.
pub fn raster_threads() -> u16 {
    static THREADS: std::sync::OnceLock<u16> = std::sync::OnceLock::new();
    *THREADS.get_or_init(|| {
        match std::env::var("PYRO_RASTER_THREADS").ok().and_then(|v| v.trim().parse::<u16>().ok()) {
            Some(n) => n.min(64),
            None => std::thread::available_parallelism().map_or(0, |n| (n.get() - 1).min(8) as u16),
        }
    })
}

/// The renderer in `slot`, made with `threads` raster threads, or resized (keeping them).
fn ensure_cpu(
    slot: &mut Option<(VelloCpuImageRenderer, (u32, u32))>,
    w: u32,
    h: u32,
    threads: u16,
) -> &mut VelloCpuImageRenderer {
    match slot {
        Some((_, size)) if *size == (w, h) => {}
        Some((r, size)) => {
            r.resize(w, h);
            *size = (w, h);
        }
        None => *slot = Some((VelloCpuImageRenderer::with_threads(w, h, threads), (w, h))),
    }
    &mut slot.as_mut().expect("renderer").0
}

#[cfg_attr(not(feature = "gpu"), allow(dead_code))]
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
    use super::chrome_page_geometry;

    /// (paper in, margins in) -> page size pt, origin px, content size pt, measured on Chrome 156.
    #[test]
    fn page_geometry_matches_chrome() {
        for (paper, margins, page, origin, content) in [
            // Letter 0.4 in: margins are 28 pt, the origin 37 px, the content 556 x 736 pt.
            ((8.5, 11.0), [0.4; 4], (612.0, 792.0), (37.0, 37.0), (556.0, 736.0)),
            // 1 cm is 28.35 pt.
            ((8.5, 11.0), [1.0 / 2.54; 4], (612.0, 792.0), (37.0, 37.0), (556.0, 736.0)),
            ((8.5, 11.0), [0.5; 4], (612.0, 792.0), (48.0, 48.0), (540.0, 720.0)),
            ((8.5, 11.0), [0.0; 4], (612.0, 792.0), (0.0, 0.0), (612.0, 792.0)),
            // 29 pt = 38.67 px and 8 pt = 10.67 px round up, 28 pt = 37.33 px down.
            ((8.5, 11.0), [0.41; 4], (612.0, 792.0), (39.0, 39.0), (554.0, 734.0)),
            ((8.5, 11.0), [0.123; 4], (612.0, 792.0), (11.0, 11.0), (596.0, 776.0)),
            // Top, right, bottom, left round on their own.
            ((8.5, 11.0), [0.3, 0.2, 0.7, 0.9], (612.0, 792.0), (28.0, 85.0), (534.0, 721.0)),
            // A4: 596 x 842 pt on a 300 dpi grid.
            ((8.27, 11.69), [0.4; 4], (595.92, 841.92), (37.0, 37.0), (540.0, 786.0)),
            ((8.2677165, 11.6929134), [0.0; 4], (595.92, 841.92), (0.0, 0.0), (596.0, 842.0)),
            ((8.27, 11.69), [0.77, 0.13, 0.21, 0.55], (595.92, 841.92), (73.0, 52.0), (548.0, 772.0)),
            // Playwright's A4: 843 pt is 3512.5 dots, which lands on 3512 (and 1191 pt, 4962.5, on 4963).
            ((8.27, 11.7), [0.0; 4], (595.92, 842.88), (0.0, 0.0), (596.0, 843.0)),
            ((11.7, 16.54), [0.0; 4], (842.88, 1191.12), (0.0, 0.0), (843.0, 1191.0)),
            // 289 x 363 pt.
            ((4.01, 5.03), [0.4; 4], (288.96, 363.12), (37.0, 37.0), (233.0, 307.0)),
        ] {
            let ((w, h), [top, right, bottom, left]) = chrome_page_geometry(paper, margins).unwrap();
            let close = |a: f64, b: f64| (a - b).abs() < 1e-6;
            let at = format!("{paper:?} {margins:?}");
            assert!(close(w * 0.75, page.0) && close(h * 0.75, page.1), "page {at}: {w} {h}");
            assert!(close(top, origin.0) && close(left, origin.1), "origin {at}: {top} {left}");
            assert!(close((w - left - right) * 0.75, content.0), "content width {at}");
            assert!(close((h - top - bottom) * 0.75, content.1), "content height {at}");
        }
    }

    #[test]
    fn an_empty_content_area_has_no_geometry() {
        // Chrome: 0.5 + 0.5 in on 1 in fails; 0.497 in is 35 pt, which leaves 2 pt.
        assert!(chrome_page_geometry((1.0, 1.0), [0.5, 0.0, 0.5, 0.0]).is_none());
        assert!(chrome_page_geometry((1.0, 1.0), [0.497, 0.0, 0.497, 0.0]).is_some());
        assert!(chrome_page_geometry((1.0, 1.0), [0.0, 0.5, 0.0, 0.5]).is_none());
        // 1.001 in rounds up to 73 pt.
        assert!(chrome_page_geometry((1.001, 1.0), [0.0, 0.5, 0.0, 0.5]).is_some());
    }

    use super::*;
    use pyro_dom::parse::{parse_document, DomRef};
    use pyro_dom::snapshot::{render_key, RenderHtml, RenderNodeKind};
    use std::{cell::RefCell, rc::Rc};

    struct NoFetch;
    impl Fetcher for NoFetch {
        fn fetch(&self, _url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
            done(Err("offline".into()));
        }
    }

    fn fixture(html: &str) -> (Snapshot, u64) {
        let dom: DomRef = Rc::new(RefCell::new(pyro_dom::Dom::new()));
        let doc = parse_document(&dom, "http://test/", html);
        let d = dom.borrow();
        let target = render_key(d.get_element_by_id(doc, "box").unwrap().unwrap());
        let tree = d
            .render_tree(
                doc,
                &RenderHtml { style_text: &|_| None, stylesheet_href: &|_| None, mark: None, adopted: None },
            )
            .unwrap();
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
                live: Some(Live { page: 1, tree: Arc::new(tree), clip: None, interaction: None }),
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

    /// The blitz node of the element with `id` in `page`'s mirrored document.
    fn blitz_by_id(renderer: &Renderer, page: u64, id: &str) -> blitz_dom::NodeId {
        let doc = &renderer.mirrors[&page].doc;
        let attr = blitz_dom::local_name!("id");
        doc.root_node()
            .tree()
            .iter()
            .find(|(_, node)| node.element_data().is_some_and(|el| el.attr(attr.clone()) == Some(id)))
            .map(|(node_id, _)| node_id)
            .unwrap()
    }

    fn set_style(renderer: &mut Renderer, page: u64, node: blitz_dom::NodeId, style: &str) {
        let doc = &mut renderer.mirrors.get_mut(&page).unwrap().doc;
        let name = blitz_dom::QualName::new(None, blitz_dom::ns!(), blitz_dom::local_name!("style"));
        doc.mutate().set_attribute(node, name, style);
    }

    fn resolve(renderer: &mut Renderer, page: u64) {
        renderer.mirrors.get_mut(&page).unwrap().doc.resolve(0.0);
    }

    const STACKED: &str =
        "<!doctype html><style>body{margin:0}div{position:absolute;left:0;top:0;width:50px;height:50px}</style>\
        <div id=box style=z-index:1></div><div id=other style=z-index:2></div><p id=far style=margin-top:80px>x</p>";

    #[test]
    fn resolve_leaves_the_paint_tree_to_the_first_hit_test() {
        let (snap, _) = fixture(STACKED);
        let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
        let mut renderer = Renderer::new();
        renderer.sync_layout(&snap, &fetcher).unwrap();
        let other = blitz_by_id(&renderer, 1, "other");
        // `sync_layout` resolves but neither paints nor hit-tests.
        let doc = &mut renderer.mirrors.get_mut(&1).unwrap().doc;
        assert!(doc.get_node(other).unwrap().stacking_context.is_none(), "resolve built the paint tree");
        doc.ensure_paint_tree();
        assert!(doc.get_node(other).unwrap().stacking_context.is_some());
        assert!(doc.paint_tree_matches_fresh_build());
        // Further resolves leave the built tree as it was until the next build.
        set_style(&mut renderer, 1, other, "z-index:2;opacity:0.5");
        resolve(&mut renderer, 1);
        let doc = &mut renderer.mirrors.get_mut(&1).unwrap().doc;
        assert!(doc.get_node(other).unwrap().flags.contains(blitz_dom::node::NodeFlags::PAINT_TREE_DIRTY));
        doc.ensure_paint_tree();
        assert!(!doc.get_node(other).unwrap().flags.contains(blitz_dom::node::NodeFlags::PAINT_TREE_DIRTY));
        assert!(doc.paint_tree_matches_fresh_build());
    }

    /// A fixed box without a `z-index` inside an `opacity` (or `isolation`) wrapper paints with
    /// the wrapper (its stacking context), not with its containing block (the viewport): Blitz's
    /// hit test finds it where it is painted, under a window scroll too (Blitz hit-tests document
    /// coordinates: the box's viewport position plus the scroll). Without its paint offset it was
    /// hit as if its viewport position were the wrapper's.
    #[test]
    fn a_fixed_box_moved_into_a_stacking_context_is_hit_where_it_is_painted() {
        for wrapper in ["opacity:.5", "isolation:isolate"] {
            let (mut snap, key) = fixture(&format!(
                "<!doctype html><style>body{{margin:0}}</style><div style='height:30px'></div>\
                 <div style='{wrapper};margin-left:40px;height:20px'>\
                 <div id=box style='position:fixed;left:120px;top:50px;width:30px;height:30px;background:blue'></div></div>\
                 <div style='height:1000px'></div>"
            ));
            for scroll in [0.0, 25.0] {
                snap.scroll_y = scroll;
                let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
                let mut renderer = Renderer::new();
                renderer.paint(&snap, Area::Viewport, &fetcher).unwrap();
                let what = format!("{wrapper}, scrolled {scroll}");
                let hit = |renderer: &mut Renderer, x: f64, y: f64| {
                    renderer.elements_from_point(1, x, y + scroll).first().copied()
                };
                assert_eq!(hit(&mut renderer, 135.0, 65.0), Some(key), "{what}: where it is painted");
                assert_eq!(hit(&mut renderer, 121.0, 51.0), Some(key), "{what}: its top left corner");
                assert_ne!(hit(&mut renderer, 175.0, 90.0), Some(key), "{what}: moved by the wrapper");
                assert_ne!(hit(&mut renderer, 135.0, 40.0), Some(key), "{what}: moved by the scroll");
            }
        }
    }

    #[test]
    fn element_from_point_sees_every_resolve_since_the_last_paint() {
        let (snap, key) = fixture(STACKED);
        let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
        let mut renderer = Renderer::new();
        renderer.paint(&snap, Area::Viewport, &fetcher).unwrap();
        let (bx, other, far) =
            (blitz_by_id(&renderer, 1, "box"), blitz_by_id(&renderer, 1, "other"), blitz_by_id(&renderer, 1, "far"));
        let other_key = renderer.mirrors[&1].key_of(other).unwrap();
        assert_eq!(renderer.elements_from_point(1, 10.0, 10.0).first(), Some(&other_key));

        // Raise #box, then make an unrelated change in a second resolve: the
        // hit test (no paint in between) must still see the first one.
        set_style(&mut renderer, 1, bx, "z-index:3");
        resolve(&mut renderer, 1);
        set_style(&mut renderer, 1, far, "margin-top:81px");
        resolve(&mut renderer, 1);
        assert_eq!(renderer.elements_from_point(1, 10.0, 10.0).first(), Some(&key));
        assert!(renderer.mirrors.get_mut(&1).unwrap().doc.paint_tree_matches_fresh_build());

        // Lower it again across two resolves, then drop #other: #box is on top.
        set_style(&mut renderer, 1, bx, "z-index:1");
        resolve(&mut renderer, 1);
        resolve(&mut renderer, 1);
        assert_eq!(renderer.elements_from_point(1, 10.0, 10.0).first(), Some(&other_key));
        renderer.mirrors.get_mut(&1).unwrap().doc.mutate().remove_node(other);
        resolve(&mut renderer, 1);
        set_style(&mut renderer, 1, far, "margin-top:82px");
        resolve(&mut renderer, 1);
        assert_eq!(renderer.elements_from_point(1, 10.0, 10.0).first(), Some(&key));
        assert!(renderer.mirrors.get_mut(&1).unwrap().doc.paint_tree_matches_fresh_build());
    }

    /// Floats are lifted out of the blocks that paint in their painter's phases (blitz-dom
    /// `Node::paints_in_ancestor_phases`): the paint tree rebuilt after changes to those blocks
    /// (one becomes a layer root, a flex container, the float stops floating) equals a fresh build.
    #[test]
    fn floats_lifted_to_their_painter_match_a_fresh_build_after_changes() {
        let (snap, _) = fixture(
            "<!doctype html><style>body{margin:0}</style><div id=outer><div id=mid>0\
             <span id=box style='float:left;width:20px;height:20px'></span></div>\
             <span><div style='height:30px'></div></span></div>",
        );
        let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
        let mut renderer = Renderer::new();
        renderer.sync_layout(&snap, &fetcher).unwrap();
        let (outer, mid, float) =
            (blitz_by_id(&renderer, 1, "outer"), blitz_by_id(&renderer, 1, "mid"), blitz_by_id(&renderer, 1, "box"));
        let check = |renderer: &mut Renderer, what: &str| {
            let doc = &mut renderer.mirrors.get_mut(&1).unwrap().doc;
            doc.ensure_paint_tree();
            assert!(doc.paint_tree_matches_fresh_build(), "{what}");
        };
        check(&mut renderer, "first build");
        let lifted = |renderer: &Renderer| {
            renderer.mirrors[&1].doc.get_node(mid).unwrap().paint_children.borrow().as_ref().is_none_or(|c| !c.contains(&float))
        };
        assert!(lifted(&renderer), "the float is lifted out of the block");
        for (node, style, what) in [
            (mid, "position:relative", "the block becomes a layer root"),
            (mid, "", "and back"),
            (outer, "display:flex", "its parent becomes a flex container"),
            (outer, "", "and back"),
            (float, "width:20px;height:20px", "the float stops floating"),
            (float, "float:left;width:20px;height:20px", "and floats again"),
        ] {
            set_style(&mut renderer, 1, node, style);
            resolve(&mut renderer, 1);
            check(&mut renderer, what);
        }
        assert!(lifted(&renderer), "the float is lifted again");
    }
}

#[cfg(test)]
mod deferred_html_scroll_tests {
    use super::*;

    struct NoFetch;
    impl Fetcher for NoFetch {
        fn fetch(&self, _url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
            done(Err("unexpected resource".into()));
        }
    }
    fn pixels(html: &str) -> RgbaImage {
        let snapshot = Snapshot {
            html: format!("<!doctype html><style>body{{margin:0;background:white}}</style>{html}"),
            base_url: "http://test/".into(),
            width: 80,
            height: 80,
            device_scale_factor: 1.0,
            ..Default::default()
        };
        Renderer::new()
            .render(&snapshot, Area::FullPage, Backend::Cpu, &(Arc::new(NoFetch) as Arc<dyn Fetcher>))
            .unwrap()
            .0
    }
    fn at(image: &RgbaImage, x: usize, y: usize) -> &[u8] {
        let start = (y * image.width as usize + x) * 4;
        &image.data[start..start + 4]
    }
    #[test]
    fn full_page_is_the_document_height_at_any_device_scale_factor() {
        // A document 200 CSS px tall in an 80x80 viewport: 80x200 device px at
        // 1, 240x600 at 3 (the scrollable overflow is in device px).
        for (dpr, w, h) in [(1.0, 80, 200), (3.0, 240, 600)] {
            let snapshot = Snapshot {
                html: "<!doctype html><style>body{margin:0;background:white}</style><div style=\"height:200px;background:red\"></div>".into(),
                base_url: "http://test/".into(),
                width: 80,
                height: 80,
                device_scale_factor: dpr,
                ..Default::default()
            };
            let (image, ..) = Renderer::new()
                .render(&snapshot, Area::FullPage, Backend::Cpu, &(Arc::new(NoFetch) as Arc<dyn Fetcher>))
                .unwrap();
            assert_eq!((image.width, image.height), (w, h), "device scale factor {dpr}");
            assert_eq!(at(&image, 10, h as usize - 2), &[255, 0, 0, 255]);
        }
    }
    #[test]
    fn deferred_html_nested_scroll_preserves_both_axes_and_clipping() {
        let image = pixels(
            r#"<div data-pyro-scroll-left="40" data-pyro-scroll-top="40" style="width:40px;height:40px;overflow:hidden"><div style="position:relative;width:80px;height:80px;background:red"><div data-pyro-scroll-top="40" style="position:absolute;left:40px;top:40px;width:40px;height:40px;overflow:hidden"><div style="height:40px;background:blue"></div><div style="height:40px;background:green"></div></div></div></div>"#,
        );
        assert_eq!(at(&image, 20, 20), &[0, 128, 0, 255]);
        assert_eq!(at(&image, 60, 20), &[255, 255, 255, 255]);
        assert_eq!(at(&image, 20, 60), &[255, 255, 255, 255]);
    }
    #[test]
    fn deferred_html_zero_or_invalid_offsets_do_not_move_content() {
        let plain = pixels(
            r#"<div style="width:40px;height:40px;overflow:hidden"><div style="height:80px;background:red"></div></div>"#,
        );
        let invalid = pixels(
            r#"<div data-pyro-scroll-left="NaN" data-pyro-scroll-top="Infinity" style="width:40px;height:40px;overflow:hidden"><div style="height:80px;background:red"></div></div>"#,
        );
        assert_eq!(plain, invalid);
        assert_eq!(at(&plain, 20, 20), &[255, 0, 0, 255]);
    }
    #[test]
    fn deferred_html_time_input_values_are_visible() {
        let morning = pixels(r#"<input type="time" value="08:00" style="width:70px;height:24px;font:12px Arial">"#);
        let afternoon = pixels(r#"<input type="time" value="14:00" style="width:70px;height:24px;font:12px Arial">"#);
        let empty = pixels(r#"<input type="time" value="" style="width:70px;height:24px;font:12px Arial">"#);
        assert_ne!(morning, empty, "a time value must not paint as an empty field");
        assert_ne!(morning, afternoon, "changing the saved time must change the rendered pixels");
    }
}
