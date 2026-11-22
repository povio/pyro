//! `CanvasRenderingContext2D`, `OffscreenCanvas` 2D contexts, `ImageBitmap`
//! and canvas pixels, rasterized by pyro-canvas (vello_cpu).
//!
//! - The drawing state (styles, line and shadow settings, text settings,
//!   the current transform, the clipping region) is kept, saved and
//!   restored like a browser's; `font` parses and serializes the CSS
//!   shorthand, defaulting to `10px sans-serif`.
//! - The current path is built with the transform applied (kurbo geometry,
//!   `canvas_path`), so `isPointInPath`/`isPointInStroke` answer for it and
//!   for a `Path2D`, and the rasterizer gets device-space paths.
//! - Each context owns an `pyro_canvas::Surface`: its bitmap is allocated on
//!   the first draw or pixel write (reading a canvas nothing was drawn on
//!   allocates nothing) and released when the canvas is resized.
//! - Text is shaped with Parley on the system fonts (`pyro_canvas::text`), for
//!   `fillText`/`strokeText` and `measureText`.
//! - Images reach the canvas through `drawImage`, `createPattern` and
//!   `createImageBitmap`: `<img>` pixels are decoded on first use from the
//!   bytes the embedder kept ([`PageDom::set_image_bytes`]), `<video>`s give
//!   their current frame (`media::current_frame`), canvases and
//!   `OffscreenCanvas`es by snapshot, `ImageBitmap`s by their handle.
//! - Cross-origin images make the canvas origin-unclean: pixel reads
//!   (`getImageData`, `toDataURL`, `toBlob`, `convertToBlob`) then throw
//!   `SecurityError`.
//!
//! - `ImageBitmap`s are serializable and transferable, `OffscreenCanvas`es
//!   (without a context) transferable, within a thread and to and from
//!   workers ([`CanvasObject`]). `transferControlToOffscreen` makes an
//!   `OffscreenCanvas` whose frames show on its `<canvas>` ([`Placeholder`]):
//!   the thread drawing on it commits each changed frame after its task.
//!
//! The value types (`CanvasGradient`, `CanvasPattern`, `Path2D`, `ImageData`,
//! `TextMetrics`, `ImageBitmap`, `OffscreenCanvas`, `DOMMatrix`, `DOMPoint`)
//! and `createImageBitmap` are made in JS (canvas2d.js) on a realm's first
//! use of any of them, in a window or a worker.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::{Arc, Mutex};

use kurbo::{Affine, BezPath, Point, Rect};
use pyro_canvas::{Bitmap, ClipPath, Color, Composite, DrawState, Paint, Pixmap, Surface};
use pyro_dom::canvas_color::Space;
use pyro_dom::text_metrics::{self, FontSpec};
use pyro_dom::NodeId;
use pyro_js::sys;
use pyro_js::{Args, ClassDef, JsClass, JsError, JsResult, NoParent, Object, Persistent, Scope, Value};

use crate::canvas_path::{closed, on_outline, point_in_fill, replay, stroke_outline, Builder, Path};
use crate::lazy_module::{lazy_globals, LazyModule};
use crate::node::node_value;
use crate::page::PageDom;
use crate::util::*;
use crate::window::WindowHost;

const CONTEXT_2D: &str = "__lpCanvas2d";
/// The hidden slot of a `<canvas>` wrapper holding its `ImageBitmapRenderingContext`.
const BITMAP_RENDERER: &str = "__lpCanvasBitmapRenderer";
/// The OffscreenCanvas of an offscreen context (hidden on its wrapper).
const OFFSCREEN: &str = "__lpOffscreen";

pub(crate) static MODULE: LazyModule = LazyModule {
    key: "lpCanvas2d",
    source: include_str!("canvas2d.js"),
    url: "pyro://internal/canvas2d.js",
    natives: Some(natives),
};

pub(crate) static VIDEO_FRAMES: LazyModule = LazyModule {
    key: "lpVideoFrames",
    source: include_str!("video_frame.js"),
    url: "pyro://internal/video_frame.js",
    natives: Some(|s| {
        let limits = s.object();
        limits.set_atom("maxSide", s.number(f64::from(pyro_canvas::MAX_SIDE)))?;
        limits.set_atom("maxArea", s.number(pyro_canvas::MAX_AREA as f64))?;
        limits.set_atom("isCanvasSource", s.function("isCanvasSource", |s, _, a| {
            Ok(s.boolean(is_frame_canvas_source(s, a.get(0))?))
        })?.value())?;
        limits.set_atom("canvasSnapshot", s.function("canvasSnapshot", |s, _, a| frame_canvas_snapshot(s, a.get(0)))?.value())?;
        Ok(limits)
    }),
};

pub(crate) fn define_video_globals<W>(def: &mut ClassDef<W>)
where
    W: JsClass,
    W::Data: WindowHost,
{
    lazy_globals!(def, VIDEO_FRAMES, "VideoFrame", "VideoColorSpace");
    crate::video_decoder::define(def);
}

/// Adds the lazy interface names to pyro-browser's `Window` class.
pub(crate) fn define_globals<W>(def: &mut ClassDef<W>)
where
    W: JsClass,
    W::Data: WindowHost,
{
    define_video_globals(def);
    lazy_globals!(
        def,
        MODULE,
        "DOMRectReadOnly",
        "DOMRect",
        "DOMQuad",
        "DOMPointReadOnly",
        "DOMPoint",
        "DOMMatrixReadOnly",
        "DOMMatrix",
        "SVGRect",
        "DOMRectList",
        "WebKitCSSMatrix",
        "Path2D",
        "ImageData",
        "CanvasGradient",
        "CanvasPattern",
        "TextMetrics",
        "ImageBitmap",
        "ImageBitmapRenderingContext",
        "OffscreenCanvas",
        "createImageBitmap",
    );
}

/// The same names on a worker's global (no `WebKitCSSMatrix` there).
pub(crate) fn define_worker_globals<W>(def: &mut ClassDef<W>)
where
    W: JsClass,
    W::Data: WindowHost,
{
    lazy_globals!(
        def,
        MODULE,
        "DOMRectReadOnly",
        "DOMRect",
        "DOMQuad",
        "DOMPointReadOnly",
        "DOMPoint",
        "DOMMatrixReadOnly",
        "DOMMatrix",
        "Path2D",
        "ImageData",
        "CanvasGradient",
        "CanvasPattern",
        "TextMetrics",
        "ImageBitmap",
        "ImageBitmapRenderingContext",
        "OffscreenCanvas",
        "createImageBitmap",
    );
}

pub(crate) fn rect_value<'s>(s: &Scope<'s>, page: &Rc<PageDom>, readonly: bool, rect: pyro_dom::layout::Rect) -> JsResult<Value<'s>> {
    MODULE.helper(s, page, "rectValue", &[s.number(rect.x), s.number(rect.y), s.number(rect.width), s.number(rect.height), s.boolean(readonly)])
}

pub(crate) fn rect_list<'s>(s: &Scope<'s>, page: &Rc<PageDom>, rects: &[Value<'s>]) -> JsResult<Value<'s>> {
    MODULE.helper(s, page, "rectList", &[s.array(rects)?.value()])
}

fn natives<'s>(s: &Scope<'s>) -> JsResult<Object<'s>> {
    let natives = s.object();
    natives.set_atom("worker", s.boolean(crate::worker_scope::is_worker(s)))?;
    let transform = s.function("parseTransform", |s, _, a| {
        let Some((matrix, is_2d)) = pyro_dom::stylo::matrix_from_css_text(&dom_string(a.get(0))?) else { return Ok(s.null()) };
        Ok(s.array(&[s.array(&matrix.map(|v| s.number(v)))?.value(), s.boolean(is_2d)])?.value())
    })?;
    natives.set_atom("parseTransform", transform.value())?;
    let parse = s.function("parseColor", |s, _this, a| {
        Ok(match canvas_color(&dom_string(a.get(0))?) {
            Some(c) => s.string(&c),
            None => s.null(),
        })
    })?;
    natives.set_atom("parseColor", parse.value())?;
    let is_blob =
        s.function("isBlob", |s, _this, a| Ok(s.boolean(s.unwrap::<crate::file::Blob>(a.get(0)).is_some())))?;
    natives.set_atom("isBlob", is_blob.value())?;
    let make_bitmap = s.function("makeBitmap", |s, _this, a| make_bitmap(s, a))?;
    natives.set_atom("makeBitmap", make_bitmap.value())?;
    let offscreen = s.function("offscreenContext", |s, _this, a| {
        let page = crate::target::page_of(s)?;
        let (w, h) = (a.get(1).to_number()?, a.get(2).to_number()?);
        let space = Space::parse(&a.get(4).to_string()?).unwrap_or_default();
        let context = new_context(s, &page, Target::Offscreen, size_u32(w), size_u32(h), space)?;
        s.set_hidden(context, OFFSCREEN, a.get(0))?;
        page.canvas_used.set(true);
        // An OffscreenCanvas of `transferControlToOffscreen`: its frames go
        // to the placeholder `<canvas>`.
        if let (Some(placeholder), Some(c)) =
            (placeholder_of(a.get(3))?, s.unwrap::<OffscreenCanvasRenderingContext2D>(context.value()))
        {
            let mut commits = page.canvas_commits.borrow_mut();
            commits.retain(|c| c.surface.strong_count() > 0);
            commits.push(FrameCommit { surface: Rc::downgrade(&c.surface), clean: c.clean.clone(), placeholder, committed: Cell::new(None) });
        }
        Ok(context.value())
    })?;
    natives.set_atom("offscreenContext", offscreen.value())?;
    let resize = s.function("resizeContext", |s, _this, a| {
        if let Some(c) = s.unwrap::<OffscreenCanvasRenderingContext2D>(a.get(0)) {
            let (w, h) = (size_u32(a.get(1).to_number()?), size_u32(a.get(2).to_number()?));
            c.reset();
            c.clean.set(true);
            c.surface.borrow_mut().resize(w, h);
        }
        Ok(s.undefined())
    })?;
    natives.set_atom("resizeContext", resize.value())?;
    let transfer = s.function("transferContext", |s, _this, a| {
        let c = s.unwrap::<OffscreenCanvasRenderingContext2D>(a.get(0)).ok_or(JsError::ContextDestroyed)?;
        let handle = {
            let mut surface = c.surface.borrow_mut();
            let handle = ImageHandle {
                pixmap: surface.snapshot(),
                width: surface.width(),
                height: surface.height(),
                clean: c.clean.get(),
                space: c.space,
            };
            let (w, h) = (surface.width(), surface.height());
            surface.resize(w, h);
            handle
        };
        bitmap_triple(s, handle)
    })?;
    natives.set_atom("transferContext", transfer.value())?;
    let encode = s.function("encodeContext", |s, _this, a| {
        let mime = dom_string(a.get(3))?;
        let quality = if a.get(4).is_undefined() { None } else { Some(a.get(4).to_number()?) };
        let (encoding, mime) = pyro_canvas::codec::encoding_for(&mime, quality);
        let bytes = match s.unwrap::<OffscreenCanvasRenderingContext2D>(a.get(0)) {
            Some(c) => {
                if !c.clean.get() {
                    return Err(security_error());
                }
                c.surface.borrow_mut().encode(encoding)
            }
            None => Surface::new(size_u32(a.get(1).to_number()?), size_u32(a.get(2).to_number()?)).encode(encoding),
        };
        let bytes = bytes.ok_or_else(|| JsError::dom("EncodingError", "The canvas could not be encoded."))?;
        crate::file::make_blob(s, bytes.into(), mime.into(), None)
    })?;
    natives.set_atom("encodeContext", encode.value())?;
    // blankBitmap(width, height): a transparent black ImageBitmap of that size (what a
    // bitmaprenderer OffscreenCanvas without a bitmap gives transferToImageBitmap).
    let blank = s.function("blankBitmap", |s, _this, a| {
        let (w, h) = (size_u32(a.get(0).to_number()?), size_u32(a.get(1).to_number()?));
        bitmap_triple(s, ImageHandle { pixmap: None, width: w, height: h, clean: true, space: Space::Srgb })
    })?;
    natives.set_atom("blankBitmap", blank.value())?;
    // encodeImage(handle or null, width, height, type, quality): a bitmaprenderer OffscreenCanvas
    // as a Blob (convertToBlob): its bitmap, or transparent black of the canvas size.
    let encode_image = s.function("encodeImage", |s, _this, a| {
        let mime = dom_string(a.get(3))?;
        let quality = if a.get(4).is_undefined() { None } else { Some(a.get(4).to_number()?) };
        let (encoding, mime) = pyro_canvas::codec::encoding_for(&mime, quality);
        let handle = if a.get(0).is_null() { None } else { handle_of(a.get(0))? };
        let size = (size_u32(a.get(1).to_number()?), size_u32(a.get(2).to_number()?));
        let bytes = encode_handle(handle, size, encoding)?
            .ok_or_else(|| JsError::dom("EncodingError", "The canvas could not be encoded."))?;
        crate::file::make_blob(s, bytes.into(), mime.into(), None)
    })?;
    natives.set_atom("encodeImage", encode_image.value())?;
    Ok(natives)
}

/// A bitmaprenderer canvas's output encoded: `handle`'s pixels at its size, or transparent black
/// of `canvas` size without one. SecurityError for a bitmap that is not origin-clean.
fn encode_handle(
    handle: Option<ImageHandle>,
    canvas: (u32, u32),
    encoding: pyro_canvas::Encoding,
) -> JsResult<Option<Vec<u8>>> {
    let Some(handle) = handle else { return Ok(Surface::new(canvas.0, canvas.1).encode(encoding)) };
    if !handle.clean {
        return Err(security_error());
    }
    let handle = handle_in(handle, Space::Srgb);
    let mut surface = Surface::new(handle.width, handle.height);
    if let Some(image) = &handle.pixmap {
        let src = Rect::new(0.0, 0.0, f64::from(image.width()), f64::from(image.height()));
        let dst = Rect::new(0.0, 0.0, f64::from(handle.width), f64::from(handle.height));
        surface.draw_image(image, src, dst, &State::new().draw_state());
    }
    Ok(surface.encode(encoding))
}

/// What a `<canvas>`'s bitmaprenderer context shows (`element`: its wrapper): `None` without
/// such a context, `Some(None)` for transparent black of the canvas size (no bitmap, or null
/// transferred), else the bitmap at its own size.
fn renderer_output(s: &Scope<'_>, page: &Rc<PageDom>, element: Object<'_>) -> JsResult<Option<Option<ImageHandle>>> {
    let Some(context) = s.get_hidden(element, BITMAP_RENDERER)? else { return Ok(None) };
    let shown = MODULE.helper(s, page, "rendererImage", &[context])?;
    let Some(shown) = shown.as_object() else { return Ok(Some(None)) };
    Ok(Some(handle_of(shown.get_index(0)?)?))
}

fn size_u32(v: f64) -> u32 {
    if v.is_finite() && v > 0.0 {
        v.min(f64::from(u32::MAX)) as u32
    } else {
        0
    }
}

fn security_error() -> JsError {
    JsError::dom("SecurityError", "The canvas has been tainted by cross-origin data.")
}

/// A CSS color as canvas serializes it (`pyro_dom::canvas_color`): `#rrggbb`
/// or `rgba(r, g, b, a)` for legacy sRGB syntaxes, CSS serialization for the
/// rest. `currentcolor` is black (no element style applies). `None` when
/// `value` is not a CSS color: the caller keeps its previous value.
pub(crate) fn canvas_color(value: &str) -> Option<String> {
    parsed_color(value).map(|c| c.css)
}

fn parsed_color(value: &str) -> Option<pyro_dom::canvas_color::CanvasColor> {
    pyro_dom::canvas_color::parse(value, [0.0, 0.0, 0.0, 1.0])
}

/// `value` as a fill, stroke or shadow color of `c`: `currentcolor` is the
/// computed `color` of a connected `<canvas>` when the color is set, else
/// black (an OffscreenCanvas, a disconnected element).
fn context_color(c: &ContextData, value: &str) -> Option<pyro_dom::canvas_color::CanvasColor> {
    let black = [0.0, 0.0, 0.0, 1.0];
    let current = match c.target {
        Target::Element(canvas) if value.to_ascii_lowercase().contains("currentcolor") => {
            let color = c.page.try_layout().map(|layout| layout.computed_style_value(&c.page.dom.borrow(), canvas, "color"));
            color.and_then(|v| pyro_dom::canvas_color::parse(&v, black)).map_or(black, |v| v.rgba)
        }
        _ => black,
    };
    pyro_dom::canvas_color::parse_in(value, current, c.space)
}

/// The color of a [`canvas_color`] serialization.
fn color_of(serialized: &str) -> Color {
    if let Some(v) = serialized.strip_prefix('#').filter(|h| h.len() == 6).and_then(|h| u32::from_str_radix(h, 16).ok()) {
        return Color::from_rgba8((v >> 16) as u8, (v >> 8) as u8, v as u8, 255);
    }
    // `rgba(r, g, b, a)` as canvas_color prints it (the shadow color of every
    // draw): read directly; other serializations (`color(...)`, `lab(...)`)
    // go through the parser.
    if let Some(inner) = serialized.strip_prefix("rgba(").and_then(|r| r.strip_suffix(')')) {
        let parts: Vec<f32> = inner.split(',').filter_map(|p| p.trim().parse().ok()).collect();
        if let [r, g, b, a] = parts[..] {
            return Color::new([r / 255.0, g / 255.0, b / 255.0, a.clamp(0.0, 1.0)]);
        }
    }
    parsed_color(serialized).map_or(Color::BLACK, |c| Color::new(c.rgba))
}

/// A gradient or pattern's paint, made again when its record's version
/// changes (`addColorStop`, `setTransform`).
type PaintCache = Rc<RefCell<Option<(f64, Paint, bool)>>>;

#[derive(Clone)]
enum Style {
    Color(String, Color),
    /// A CanvasGradient or CanvasPattern of the context's realm.
    Object(Persistent, PaintCache),
}

#[derive(Clone)]
pub(crate) struct State {
    font: FontSpec,
    text_align: &'static str,
    text_baseline: &'static str,
    direction: &'static str,
    fill_style: Style,
    stroke_style: Style,
    global_alpha: f64,
    composite: &'static str,
    pub(crate) line_width: f64,
    pub(crate) line_cap: &'static str,
    pub(crate) line_join: &'static str,
    pub(crate) miter_limit: f64,
    line_dash: Vec<f64>,
    line_dash_offset: f64,
    shadow_blur: f64,
    shadow_color: String,
    /// `shadow_color` in the context's color space.
    shadow_rgba: Color,
    shadow_offset_x: f64,
    shadow_offset_y: f64,
    image_smoothing: bool,
    image_smoothing_quality: &'static str,
    filter: String,
    /// pyro: `filter` resolved for drawing (`None` for `none`): its value and the markup of the
    /// document's elements it references (`canvas_filter`).
    filter_spec: Option<pyro_canvas::CanvasFilter>,
    /// pyro: drawing with `filter` taints the canvas (a filter primitive
    /// colored from a link's visited-dependent `currentcolor`).
    filter_taints: bool,
    letter_spacing: String,
    word_spacing: String,
    /// pyro: `lang` (any string; `inherit` by default). Stored only: text is shaped without it.
    lang: String,
    font_kerning: &'static str,
    text_rendering: &'static str,
    pub(crate) transform: Affine,
    /// The clipping region: the intersection of these (device space).
    clip: Rc<Vec<ClipPath>>,
}

impl State {
    pub(crate) fn new() -> Self {
        let black = Style::Color("#000000".into(), Color::BLACK);
        State {
            font: default_font(),
            text_align: "start",
            text_baseline: "alphabetic",
            direction: "inherit",
            fill_style: black.clone(),
            stroke_style: black,
            global_alpha: 1.0,
            composite: "source-over",
            line_width: 1.0,
            line_cap: "butt",
            line_join: "miter",
            miter_limit: 10.0,
            line_dash: Vec::new(),
            line_dash_offset: 0.0,
            shadow_blur: 0.0,
            shadow_color: "rgba(0, 0, 0, 0)".into(),
            shadow_rgba: Color::TRANSPARENT,
            shadow_offset_x: 0.0,
            shadow_offset_y: 0.0,
            image_smoothing: true,
            image_smoothing_quality: "low",
            filter: "none".into(),
            filter_spec: None,
            filter_taints: false,
            letter_spacing: "0px".into(),
            word_spacing: "0px".into(),
            lang: "inherit".into(),
            font_kerning: "auto",
            text_rendering: "auto",
            transform: Affine::IDENTITY,
            clip: Rc::new(Vec::new()),
        }
    }

    fn draw_state(&self) -> DrawState<'_> {
        let shadow_color = self.shadow_rgba;
        DrawState {
            transform: self.transform,
            alpha: self.global_alpha as f32,
            composite: Composite::from_name(self.composite).unwrap_or(Composite::SourceOver),
            clip: &self.clip,
            shadow: (shadow_color.components[3] > 0.0).then(|| pyro_canvas::Shadow {
                color: shadow_color,
                blur: self.shadow_blur,
                offset: kurbo::Vec2::new(self.shadow_offset_x, self.shadow_offset_y),
            }),
            smoothing: self.image_smoothing,
            filter: self.filter_spec.as_ref(),
        }
    }

    pub(crate) fn stroke(&self) -> kurbo::Stroke {
        let cap = match self.line_cap {
            "round" => kurbo::Cap::Round,
            "square" => kurbo::Cap::Square,
            _ => kurbo::Cap::Butt,
        };
        let join = match self.line_join {
            "round" => kurbo::Join::Round,
            "bevel" => kurbo::Join::Bevel,
            _ => kurbo::Join::Miter,
        };
        let stroke =
            kurbo::Stroke::new(self.line_width).with_caps(cap).with_join(join).with_miter_limit(self.miter_limit);
        // A dash pattern of zeros draws a solid line, as no pattern does.
        if self.line_dash.iter().any(|d| *d > 0.0) {
            stroke.with_dashes(self.line_dash_offset, self.line_dash.iter().copied())
        } else {
            stroke
        }
    }
}

/// What a context draws into.
#[derive(Clone, Copy, PartialEq)]
enum Target {
    /// A `<canvas>`: its `width`/`height` attributes size the bitmap.
    Element(NodeId),
    /// An `OffscreenCanvas` (hidden on the wrapper as [`OFFSCREEN`]).
    Offscreen,
}

pub struct ContextData {
    page: Rc<PageDom>,
    target: Target,
    state: RefCell<State>,
    stack: RefCell<Vec<State>>,
    path: RefCell<Path>,
    surface: Rc<RefCell<Surface>>,
    /// HTML "origin-clean": nothing cross-origin was drawn.
    clean: Rc<Cell<bool>>,
    /// The `colorSpace` of the context's settings: the space the bitmap's
    /// values are in.
    space: Space,
}

impl ContextData {
    fn reset(&self) {
        *self.state.borrow_mut() = State::new();
        self.stack.borrow_mut().clear();
        *self.path.borrow_mut() = Path::default();
    }

    /// An element canvas whose size attributes changed without its IDL
    /// setters (`setAttribute`): reset as HTML does, before drawing or
    /// reading.
    fn sync_size(&self) {
        let Target::Element(canvas) = self.target else { return };
        let (w, h) = canvas_size(&self.page, canvas);
        let mut surface = self.surface.borrow_mut();
        if surface.width() != w || surface.height() != h {
            surface.resize(w, h);
            drop(surface);
            self.reset();
            self.clean.set(true);
        }
    }
}

fn default_font() -> FontSpec {
    text_metrics::parse_font("10px sans-serif", 10.0).expect("default canvas font")
}

/// The size relative canvas `font` sizes resolve against: the computed
/// `font-size` of a connected `<canvas>`, else 10px (an OffscreenCanvas, a
/// disconnected element).
fn font_parent_size(c: &ContextData) -> f64 {
    let Target::Element(canvas) = c.target else { return 10.0 };
    let size = c.page.try_layout().map(|layout| layout.computed_style_value(&c.page.dom.borrow(), canvas, "font-size"));
    size.as_deref()
        .and_then(|v| v.strip_suffix("px"))
        .and_then(|n| n.parse::<f64>().ok())
        .filter(|n| n.is_finite() && *n >= 0.0)
        .unwrap_or(10.0)
}

/// pyro: a canvas `filter` value for drawing: `None` for `none` (or nothing); else the value
/// and the markup of the `<filter>` elements of the document its `url(#id)`s name (an `id` the
/// document lacks is left out: the filter then names nothing, and draws nothing filtered).
///
/// The flag says whether drawing with it taints the canvas: as in Blink, a
/// referenced `<filter>` inside a link (`<a href>`) whose flood or lighting
/// color takes `currentcolor` paints with a color that `:visited` may decide,
/// which a page must not read back.
fn canvas_filter(page: &PageDom, value: &str) -> (Option<pyro_canvas::CanvasFilter>, bool) {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("none") {
        return (None, false);
    }
    let mut taints = false;
    let dom = page.dom.borrow();
    let doc = page.document();
    let mut defs = String::new();
    let mut rest = trimmed;
    while let Some(at) = rest.find("url(") {
        let after = &rest[at + 4..];
        let Some(end) = after.find(')') else { break };
        let reference = after[..end].trim().trim_matches(|c| c == '"' || c == '\'');
        if let Some(id) = reference.strip_prefix('#') {
            if let Some(el) = dom.get_element_by_id(doc, id).ok().flatten() {
                if let Ok(markup) = dom.xml_serialize(el) {
                    taints |= visited_dependent_filter(&dom, el, &markup);
                    defs.push_str(&markup);
                }
            }
        }
        rest = &after[end + 1..];
    }
    let Some(data) = dom.document_data(doc).ok() else { return (None, taints) };
    let spec = pyro_canvas::CanvasFilter { page: data.raster_budget.clone(), value: trimmed.to_string(), defs };
    (Some(spec), taints)
}

/// Whether the filter `el` (serialized as `markup`) has a flood or lighting
/// color taken from `currentcolor` while inside a link, whose `color` the
/// `:visited` state may set.
fn visited_dependent_filter(dom: &pyro_dom::Dom, el: NodeId, markup: &str) -> bool {
    let lower = markup.to_ascii_lowercase();
    let colored = ["<feflood", "<fedropshadow", "<fediffuselighting", "<fespecularlighting"]
        .iter()
        .any(|primitive| lower.contains(primitive));
    if !colored || !lower.contains("currentcolor") {
        return false;
    }
    let mut node = dom.parent(el).ok().flatten();
    let mut steps = 0;
    while let Some(id) = node {
        steps += 1;
        if steps > 4096 {
            break;
        }
        let link = dom.element(id).is_ok_and(|e| &*e.name.local == "a")
            && (dom.get_attribute(id, "href").ok().flatten().is_some()
                || dom.get_attribute_ns(id, Some("http://www.w3.org/1999/xlink"), "href").ok().flatten().is_some());
        if link {
            return true;
        }
        node = dom.parent(id).ok().flatten();
    }
    false
}

/// A canvas element's bitmap size: its `width`/`height` attributes.
fn canvas_size(page: &PageDom, canvas: NodeId) -> (u32, u32) {
    let dom = page.dom.borrow();
    let attr = |name: &str, default: u32| {
        crate::html::canvas_dimension(dom.get_attribute(canvas, name).ok().flatten().as_deref(), default)
    };
    (attr("width", 300), attr("height", 150))
}

fn new_context<'s>(
    s: &Scope<'s>,
    page: &Rc<PageDom>,
    target: Target,
    w: u32,
    h: u32,
    space: Space,
) -> JsResult<Object<'s>> {
    let data = ContextData {
        page: page.clone(),
        target,
        state: RefCell::new(State::new()),
        stack: RefCell::new(Vec::new()),
        path: RefCell::new(Path::default()),
        surface: Rc::new(RefCell::new(Surface::new(w, h))),
        clean: Rc::new(Cell::new(true)),
        space,
    };
    match target {
        Target::Element(canvas) => {
            page.register_canvas(canvas, Rc::downgrade(&data.surface));
            s.wrap::<CanvasRenderingContext2D>(data, None)
        }
        Target::Offscreen => s.wrap::<OffscreenCanvasRenderingContext2D>(data, None),
    }
}

/// `HTMLCanvasElement.getContext(type)`: one 2D context per canvas; other
/// context types are unsupported (null).
pub fn get_context<'s>(
    s: &Scope<'s>,
    page: &Rc<PageDom>,
    canvas: NodeId,
    kind: &str,
    settings: Value<'s>,
) -> JsResult<Value<'s>> {
    if placeholder_frame(page, canvas).is_some() {
        return Err(JsError::dom(
            "InvalidStateError",
            "Failed to execute 'getContext' on 'HTMLCanvasElement': Cannot get context from a canvas that has transferred its control to offscreen.",
        ));
    }
    // One context per canvas: a type other than the first one asked for is null.
    if kind == "bitmaprenderer" {
        let element = node_value(s, page, Some(canvas))?;
        let element = element.as_object().ok_or(JsError::ContextDestroyed)?;
        if s.get_hidden(element, CONTEXT_2D)?.is_some() {
            return Ok(s.null());
        }
        if let Some(existing) = s.get_hidden(element, BITMAP_RENDERER)? {
            return Ok(existing);
        }
        let context = MODULE.helper(s, page, "bitmapRenderer", &[element.value()])?;
        page.canvas_used.set(true);
        s.set_hidden(element, BITMAP_RENDERER, context)?;
        return Ok(context);
    }
    if kind != "2d" {
        return Ok(s.null());
    }
    // CanvasRenderingContext2DSettings, converted member by member in
    // dictionary order on every call (canvas2d.js `contextSettings`); a cached
    // context ignores the result.
    let space = MODULE.helper(s, page, "contextSettings", &[settings])?.to_string()?;
    let space = Space::parse(&space).unwrap_or_default();
    let element = node_value(s, page, Some(canvas))?;
    let element = element.as_object().ok_or(JsError::ContextDestroyed)?;
    if let Some(existing) = s.get_hidden(element, CONTEXT_2D)? {
        return Ok(existing);
    }
    if s.get_hidden(element, BITMAP_RENDERER)?.is_some() {
        return Ok(s.null());
    }
    let (w, h) = canvas_size(page, canvas);
    let context = new_context(s, page, Target::Element(canvas), w, h, space)?;
    page.canvas_used.set(true);
    s.set_hidden(element, CONTEXT_2D, context.value())?;
    Ok(context.value())
}

/// The `colorSpace` member of a settings dictionary (`default` when absent);
/// an unknown value is a TypeError (Web IDL enumeration).
fn space_member(settings: Value<'_>, default: Space) -> JsResult<Space> {
    let Some(settings) = settings.as_object() else { return Ok(default) };
    let value = settings.get_atom("colorSpace")?;
    if value.is_undefined() {
        return Ok(default);
    }
    let name = dom_string(value)?;
    Space::parse(&name).ok_or_else(|| {
        JsError::Type(
            format!("The provided value '{name}' is not a valid enum value of type PredefinedColorSpace.").into(),
        )
    })
}

/// `bitmap`'s values converted from `from` to `to` (the same bitmap when the
/// spaces are equal; `None` when there is no room for the copy).
fn convert_bitmap(bitmap: Bitmap, from: Space, to: Space) -> Option<Bitmap> {
    if from == to {
        return Some(bitmap);
    }
    let (w, h) = (u32::from(bitmap.width()), u32::from(bitmap.height()));
    let mut bytes = bitmap.data_as_u8_slice().to_vec();
    pyro_canvas::unpremultiply(&mut bytes);
    pyro_dom::canvas_color::convert_pixels(&mut bytes, from, to);
    pyro_canvas::pixmap_from_straight(bytes, w, h).and_then(Bitmap::new)
}

/// `handle` with its pixels in `space`.
fn handle_in(mut handle: ImageHandle, space: Space) -> ImageHandle {
    if handle.space != space {
        handle.pixmap = handle.pixmap.and_then(|p| convert_bitmap(p, handle.space, space));
        handle.space = space;
    }
    handle
}

/// The 2D context of a canvas element, if it has one.
fn element_context<'s>(s: &Scope<'s>, page: &Rc<PageDom>, canvas: NodeId) -> JsResult<Option<Value<'s>>> {
    let element = node_value(s, page, Some(canvas))?;
    let Some(element) = element.as_object() else { return Ok(None) };
    Ok(s.get_hidden(element, CONTEXT_2D)?.filter(|c| s.unwrap::<CanvasRenderingContext2D>(*c).is_some()))
}

/// HTML: setting a canvas's `width` or `height` clears its bitmap and resets
/// its context to the default state.
pub(crate) fn canvas_resized(s: &Scope<'_>, page: &Rc<PageDom>, canvas: NodeId) -> JsResult<()> {
    if let Some(c) = element_context(s, page, canvas)?.and_then(|v| s.unwrap::<CanvasRenderingContext2D>(v)) {
        c.reset();
        // HTML: setting the bitmap's dimensions resets origin-clean.
        c.clean.set(true);
        let (w, h) = canvas_size(page, canvas);
        c.surface.borrow_mut().resize(w, h);
    }
    Ok(())
}

/// The canvas's pixels in `type` (PNG unless JPEG or WebP is asked for), or
/// `None` for a canvas without pixels (zero-sized or too large).
fn encode_canvas(
    s: &Scope<'_>,
    page: &Rc<PageDom>,
    canvas: NodeId,
    mime: &str,
    quality: Option<f64>,
) -> JsResult<Option<(Vec<u8>, &'static str)>> {
    let (encoding, mime) = pyro_canvas::codec::encoding_for(mime, quality);
    let bytes = match element_context(s, page, canvas)?.and_then(|v| s.unwrap::<CanvasRenderingContext2D>(v)) {
        Some(c) => {
            if !c.clean.get() {
                return Err(security_error());
            }
            c.sync_size();
            c.surface.borrow_mut().encode(encoding)
        }
        None => {
            let element = node_value(s, page, Some(canvas))?;
            let renderer = match element.as_object() {
                Some(o) => renderer_output(s, page, o)?,
                None => None,
            };
            match renderer {
                // A bitmaprenderer canvas encodes its bitmap (transparent black without one).
                Some(bitmap) => encode_handle(bitmap, canvas_size(page, canvas), encoding)?,
                None => match placeholder_frame(page, canvas) {
                    Some(frame) => frame.surface().encode(encoding),
                    None => {
                        let (w, h) = canvas_size(page, canvas);
                        Surface::new(w, h).encode(encoding)
                    }
                },
            }
        }
    };
    Ok(bytes.map(|b| (b, mime)))
}

fn quality_arg(v: Value<'_>) -> JsResult<Option<f64>> {
    Ok(if v.is_number() { Some(v.to_number()?) } else { None })
}

/// `HTMLCanvasElement.toDataURL(type, quality)`: `data:,` for a canvas
/// without pixels.
pub(crate) fn to_data_url<'s>(s: &Scope<'s>, page: &Rc<PageDom>, canvas: NodeId, a: Args<'s>) -> JsResult<Value<'s>> {
    let mime = if a.get(0).is_undefined() { String::new() } else { dom_string(a.get(0))? };
    let quality = quality_arg(a.get(1))?;
    Ok(match encode_canvas(s, page, canvas, &mime, quality)? {
        Some((bytes, mime)) => {
            use base64::Engine as _;
            let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
            s.string(&format!("data:{mime};base64,{encoded}"))
        }
        None => s.string("data:,"),
    })
}

/// `HTMLCanvasElement.toBlob(callback, type, quality)`: the canvas is
/// encoded now; the callback gets the Blob (null for a canvas without
/// pixels) in a task.
pub(crate) fn to_blob<'s>(s: &Scope<'s>, page: &Rc<PageDom>, canvas: NodeId, a: Args<'s>) -> JsResult<Value<'s>> {
    a.require(1, "HTMLCanvasElement.toBlob")?;
    let callback = a.get(0);
    if !callback.is_function() {
        return Err(JsError::Type(
            "Failed to execute 'toBlob' on 'HTMLCanvasElement': parameter 1 is not of type 'Function'.".into(),
        ));
    }
    let mime = if a.get(1).is_undefined() { String::new() } else { dom_string(a.get(1))? };
    let encoded = encode_canvas(s, page, canvas, &mime, quality_arg(a.get(2))?)?;
    let callback = Persistent::new(s, callback);
    let realm = s.realm().downgrade();
    page.post_task(Box::new(move || {
        let Some(realm) = realm.upgrade() else { return };
        let Ok(s) = realm.scope() else { return };
        let blob = match encoded {
            Some((bytes, mime)) => match crate::file::make_blob(&s, bytes.into(), mime.into(), None) {
                Ok(b) => b,
                Err(e) => return crate::lifecycle::report_diagnostic(&s, &e),
            },
            None => s.null(),
        };
        if let Some(f) = callback.get(&s).as_object() {
            if let Err(e) = f.call(None, &[blob]) {
                // Web IDL: a callback's exception is reported in the callback's own realm.
                let callback_realm = s.object_realm(f).filter(|r| !r.is_disposed());
                match callback_realm.as_ref().and_then(|r| r.scope().ok()) {
                    Some(cs) => crate::lifecycle::report_diagnostic(&cs, &e),
                    None => crate::lifecycle::report_diagnostic(&s, &e),
                }
            }
        }
    }));
    Ok(s.undefined())
}

// ---- Image handles ---------------------------------------------------------

/// Pixels an `ImageBitmap` or `CanvasPattern` holds.
#[derive(Clone)]
struct ImageHandle {
    /// `None`: transparent black.
    pixmap: Option<Bitmap>,
    width: u32,
    height: u32,
    clean: bool,
    /// The color space of `pixmap`'s values (a canvas's, an ImageData's;
    /// decoded images are sRGB).
    space: Space,
}

thread_local! {
    static HANDED: RefCell<Option<ImageHandle>> = const { RefCell::new(None) };
}

/// A JS value holding `handle`: a native function whose call hands it over
/// ([`handle_of`]), so the pixels live exactly as long as the JS object
/// that keeps it (canvas2d.js keeps it private).
fn handle_object<'s>(s: &Scope<'s>, handle: ImageHandle) -> JsResult<Object<'s>> {
    s.function("", move |s, _this, _a| {
        HANDED.with(|h| *h.borrow_mut() = Some(handle.clone()));
        Ok(s.undefined())
    })
}

fn handle_of(v: Value<'_>) -> JsResult<Option<ImageHandle>> {
    let Some(f) = v.as_object().filter(|_| v.is_function()) else { return Ok(None) };
    HANDED.with(|h| h.borrow_mut().take());
    f.call(None, &[])?;
    Ok(HANDED.with(|h| h.borrow_mut().take()))
}

/// `[handle, width, height]` for canvas2d.js's `ImageBitmap`.
fn bitmap_triple<'s>(s: &Scope<'s>, handle: ImageHandle) -> JsResult<Value<'s>> {
    let (w, h) = (handle.width, handle.height);
    let object = handle_object(s, handle)?;
    Ok(s.array(&[object.value(), s.number(f64::from(w)), s.number(f64::from(h))])?.value())
}

fn is_frame_canvas_source(s: &Scope<'_>, value: Value<'_>) -> JsResult<bool> {
    if let Some(node) = s.unwrap::<crate::node::Node>(value) {
        let dom = node.page().dom.borrow();
        return Ok(dom.element(node.id).is_ok_and(|e| e.is_html() && &*e.name.local == "canvas"));
    }
    let Some(realm) = value.as_object().and_then(|o| s.object_realm(o)) else { return Ok(false) };
    let home = realm.scope()?;
    let page = crate::target::page_of(&home)?;
    let Some(module) = MODULE.existing(&home, &page)? else { return Ok(false) };
    let brand = module.get_atom("frameCanvasSource")?.as_object().ok_or(JsError::ContextDestroyed)?;
    Ok(brand.call(None, &[home.rehome(value)])?.to_bool())
}

fn frame_canvas_snapshot<'s>(s: &Scope<'s>, value: Value<'s>) -> JsResult<Value<'s>> {
    let invalid = || JsError::dom("InvalidStateError", "The canvas image source has no usable bitmap.");
    let exhausted = || JsError::dom("QuotaExceededError", "The VideoFrame exceeds the raster budget.");
    if !is_frame_canvas_source(s, value)? {
        return Err(JsError::Type("Expected a canvas image source.".into()));
    }
    let realm = value.as_object().and_then(|o| s.object_realm(o)).ok_or_else(invalid)?;
    let home = realm.scope()?;
    let source_page = crate::target::page_of(&home)?;
    let source = image_source(&home, &source_page, home.rehome(value), "VideoFrame", false, None)?.ok_or_else(invalid)?;
    // The frame's pixels are sRGB (its colorSpace says so).
    let source = handle_in(source, Space::Srgb);
    if !source.clean {
        return Err(JsError::dom("SecurityError", "The canvas image source is not origin-clean."));
    }
    if !pyro_canvas::valid_size(source.width, source.height) {
        return Err(invalid());
    }
    let len = source.width as usize * source.height as usize * 4;
    let budget = crate::target::page_of(s)?.raster_budget().ok_or_else(invalid)?;
    let held = budget.reserve(len).map_err(|_| exhausted())?;
    let scratch = budget.reserve(len).map_err(|_| exhausted())?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(len).map_err(|_| exhausted())?;
    if let Some(bitmap) = source.pixmap {
        if u32::from(bitmap.width()) != source.width || u32::from(bitmap.height()) != source.height {
            return Err(invalid());
        }
        bytes.extend_from_slice(bitmap.data_as_u8_slice());
        pyro_canvas::unpremultiply(&mut bytes);
    } else {
        bytes.resize(len, 0);
    }
    let result = s.object();
    result.set_prototype(s.null());
    result.set_atom("data", crate::encoding::uint8_array(s, &bytes)?)?;
    drop(bytes);
    drop(scratch);
    result.set_atom("width", s.number(f64::from(source.width)))?;
    result.set_atom("height", s.number(f64::from(source.height)))?;
    result.set_atom("opaque", s.boolean(false))?;
    result.set_atom("colorSpace", s.string("srgb"))?;
    let lease = crate::image_decoder::image_lease(s, Rc::new(held))?;
    if let Err(error) = result.set_atom("lease", lease.value()) {
        lease.call(None, &[s.boolean(false)])?;
        return Err(error);
    }
    Ok(result.value())
}

/// An image argument's pixels: `Ok(None)` when it has none to give yet
/// (an image still loading, a video), errors as HTML specifies.
fn image_source(
    s: &Scope<'_>,
    page: &Rc<PageDom>,
    value: Value<'_>,
    what: &str,
    image_data: bool,
    draw_target: Option<Target>,
) -> JsResult<Option<ImageHandle>> {
    let not_source = || {
        JsError::Type(format!(
            "Failed to execute '{what}': The provided value is not of type '(CSSImageValue or HTMLCanvasElement or HTMLImageElement or HTMLVideoElement or ImageBitmap or OffscreenCanvas or SVGImageElement or VideoFrame)'."
        ).into())
    };
    if let Some(n) = s.unwrap::<crate::node::Node>(value) {
        let source_page = n.page().clone();
        let local = {
            let dom = source_page.dom.borrow();
            let e = dom.element(n.id).map_err(|_| not_source())?;
            if !e.is_html() && &*e.name.local != "image" {
                return Err(not_source());
            }
            e.name.local.to_string()
        };
        return match local.as_str() {
            "img" => {
                if !source_page.image_complete(n.id) {
                    return Ok(None);
                }
                // pyro: HTML "check the usability of the image argument", as Chromium applies it:
                // only an image whose source failed to load or decode is broken (an error); one
                // without a source (no, empty or removed src) or with a zero natural dimension is
                // not usable and draws nothing (WPT 2d.drawImage.incomplete.*,
                // 2d.pattern.image.{nosrc,zeroheight,incomplete.*}, 2d.drawImage.zerosource.image).
                // An image whose source was emptied or removed keeps no data, even when the
                // element still holds what it showed before.
                let has_source = {
                    let dom = source_page.dom.borrow();
                    let in_picture = dom
                        .parent(n.id)
                        .ok()
                        .flatten()
                        .and_then(|p| dom.element(p).ok())
                        .is_some_and(|e| e.is_html() && &*e.name.local == "picture");
                    in_picture
                        || ["src", "srcset"].iter().any(|name| {
                            dom.get_attribute(n.id, name).ok().flatten().is_some_and(|v| !v.trim().is_empty())
                        })
                };
                if !has_source {
                    return Ok(None);
                }
                let natural = source_page.dom.borrow().natural_size(n.id);
                match natural {
                    Some((w, h)) if w > 0.0 && h > 0.0 => {}
                    Some(_) => return Ok(None),
                    None => {
                        return Err(JsError::dom(
                            "InvalidStateError",
                            "The HTMLImageElement provided is in the 'broken' state.",
                        ));
                    }
                }
                // An image that loaded before the document used a canvas API kept no bytes of its
                // own: the list of available images has them.
                source_page.adopt_available_image_bytes(n.id);
                // pyro: drawn as the image element's `image-orientation` shows it (Blink; the
                // source of the preference is not specified, w3c/csswg-drafts#4666).
                // createImageBitmap (`image_data`) takes the image's own media, as its EXIF
                // orientation shows it, whatever the element's style (WPT
                // imagebitmap-replication-exif-orientation).
                let from_image = image_data || {
                    let dom = source_page.dom.borrow();
                    source_page.layout.borrow().computed_style_value(&dom, n.id, "image-orientation") != "none"
                };
                // pyro: an SVG image's size is its natural size, whatever its bitmap's (rasterized
                // no larger than a fixed number of pixels): the source rectangle maps onto it.
                let natural = source_page
                    .image_is_svg(n.id)
                    .then(|| source_page.dom.borrow().natural_size(n.id))
                    .flatten()
                    .filter(|(w, h)| *w >= 1.0 && *h >= 1.0);
                Ok(source_page.image_pixmap(n.id, from_image).map(|(pixmap, clean)| ImageHandle {
                    width: natural.map_or(u32::from(pixmap.width()), |(w, _)| w.round() as u32),
                    height: natural.map_or(u32::from(pixmap.height()), |(_, h)| h.round() as u32),
                    pixmap: Some(pixmap),
                    clean,
                    space: Space::Srgb,
                }))
            }
            "canvas" => {
                let (w, h) = canvas_size(&source_page, n.id);
                if w == 0 || h == 0 {
                    return Err(JsError::dom(
                        "InvalidStateError",
                        "The image argument is a canvas element with a width or height of 0.",
                    ));
                }
                // The context is hidden on the wrapper passed in (the realm that
                // made it may be another same-origin window's).
                let context = match value.as_object() {
                    Some(o) => s.get_hidden(o, CONTEXT_2D)?,
                    None => None,
                };
                let handle = match context.and_then(|v| s.unwrap::<CanvasRenderingContext2D>(v)) {
                    Some(c) => {
                        c.sync_size();
                        let pixmap = c.surface.borrow_mut().snapshot();
                        ImageHandle { pixmap, width: w, height: h, clean: c.clean.get(), space: c.space }
                    }
                    None => {
                        let renderer = match value.as_object() {
                            Some(o) => renderer_output(s, page, o)?,
                            None => None,
                        };
                        match renderer {
                            // A bitmaprenderer canvas is its bitmap, at the bitmap's size.
                            Some(Some(bitmap)) => bitmap,
                            Some(None) => {
                                ImageHandle { pixmap: None, width: w, height: h, clean: true, space: Space::Srgb }
                            }
                            // A placeholder shows its OffscreenCanvas's last frame.
                            None => match placeholder_frame(&source_page, n.id) {
                                Some(frame) => ImageHandle {
                                    pixmap: frame.bitmap,
                                    width: frame.width,
                                    height: frame.height,
                                    clean: frame.clean,
                                    space: Space::Srgb,
                                },
                                None => {
                                    ImageHandle { pixmap: None, width: w, height: h, clean: true, space: Space::Srgb }
                                }
                            },
                        }
                    }
                };
                Ok(Some(handle))
            }
            // The frame shown now (pyro-media); nothing before HAVE_CURRENT_DATA
            // or without a decoder for the video.
            "video" => Ok(crate::media::current_frame(&source_page, n.id).and_then(|(frame, clean)| {
                let pixmap = pyro_canvas::pixmap_from_straight((*frame.rgba).clone(), frame.width, frame.height)?;
                Some(ImageHandle {
                    pixmap: Bitmap::new(pixmap),
                    width: frame.width,
                    height: frame.height,
                    clean,
                    space: Space::Srgb,
                })
            })),
            // pyro: an SVG `<image>`: the image of its `href` from the list of available images
            // (nothing while it is not loaded), shown as the element's `image-orientation` says
            // (an opaque image as its EXIF orientation says).
            "image" => {
                let (url, cors, from_image) = {
                    let dom = source_page.dom.borrow();
                    let href = dom
                        .get_attribute(n.id, "href")
                        .ok()
                        .flatten()
                        .or_else(|| dom.get_attribute_ns(n.id, Some("http://www.w3.org/1999/xlink"), "href").ok().flatten())
                        .map(str::to_owned);
                    let Some(href) = href.filter(|h| !h.trim().is_empty()) else { return Ok(None) };
                    let cors = match dom.get_attribute(n.id, "crossorigin").ok().flatten() {
                        None => 0,
                        Some(v) if v.eq_ignore_ascii_case("use-credentials") => 2,
                        Some(_) => 1,
                    };
                    let from_image =
                        source_page.layout.borrow().computed_style_value(&dom, n.id, "image-orientation") != "none";
                    (crate::html::resolve_url(&dom, n.id, href.trim()), cors, from_image)
                };
                let Some(image) = source_page.available_image(&url, cors) else { return Ok(None) };
                let Some((bytes, clean)) = image.bytes else { return Ok(None) };
                let oriented = from_image || !clean;
                let Some(budget) = source_page.raster_budget() else { return Ok(None) };
                let Some(pixmap) = pyro_canvas::codec::decode_bitmap(bytes.bytes(), oriented, &budget) else {
                    return Ok(None);
                };
                let (w, h) = (u32::from(pixmap.width()), u32::from(pixmap.height()));
                Ok(Some(ImageHandle { pixmap: Some(pixmap), width: w, height: h, clean, space: Space::Srgb }))
            }
            _ => Err(not_source()),
        };
    }
    if !value.is_object() {
        return Err(not_source());
    }
    let mut source = MODULE.helper(s, page, "sourceOf", &[value, s.boolean(image_data)])?;
    if source.is_null() {
        if let Some(module) = VIDEO_FRAMES.existing(s, page)? {
            if let Some(source_of) = module.get("canvasSource")?.as_object() {
                let use_orientation = match draw_target {
                    Some(Target::Element(canvas)) => {
                        page.layout().computed_style_value(&page.dom.borrow(), canvas, "image-orientation") != "none"
                    }
                    _ => true,
                };
                source = source_of.call(None, &[value, s.boolean(use_orientation)])?;
            }
        }
    }
    let Some(desc) = source.as_object().filter(|_| source.is_array()) else { return Err(not_source()) };
    let kind = desc.get_index(0)?.to_number()? as u32;
    match kind {
        0 => handle_of(desc.get_index(1)?),
        1 => {
            let (w, h) = (size_u32(desc.get_index(2)?.to_number()?), size_u32(desc.get_index(3)?.to_number()?));
            if w == 0 || h == 0 {
                return Err(JsError::dom(
                    "InvalidStateError",
                    "The image argument is an OffscreenCanvas with a width or height of 0.",
                ));
            }
            Ok(Some(match s.unwrap::<OffscreenCanvasRenderingContext2D>(desc.get_index(1)?) {
                Some(c) => {
                    let pixmap = c.surface.borrow_mut().snapshot();
                    ImageHandle { pixmap, width: w, height: h, clean: c.clean.get(), space: c.space }
                }
                None => ImageHandle { pixmap: None, width: w, height: h, clean: true, space: Space::Srgb },
            }))
        }
        // An ImageData or a buffer-backed VideoFrame, both with straight RGBA
        // pixels. A frame may specify a display size different from the bitmap.
        2 | 5 => {
            let (w, h) = (size_u32(desc.get_index(2)?.to_number()?), size_u32(desc.get_index(3)?.to_number()?));
            let data = typed_bytes(s, desc.get_index(1)?)?;
            let pixmap = pyro_canvas::pixmap_from_straight(data, w, h).and_then(Bitmap::new);
            let (width, height, space) = if kind == 5 {
                (size_u32(desc.get_index(4)?.to_number()?), size_u32(desc.get_index(5)?.to_number()?), Space::Srgb)
            } else {
                // An ImageData's values are in its colorSpace.
                let space = desc.get_index(4)?;
                let space = if space.is_string() { Space::parse(&space.to_string()?) } else { None };
                (w, h, space.unwrap_or_default())
            };
            Ok(Some(ImageHandle { pixmap, width, height, clean: true, space }))
        }
        3 => {
            let blob = s.unwrap::<crate::file::Blob>(desc.get_index(1)?).ok_or_else(not_source)?;
            let budget = page.raster_budget().ok_or_else(|| JsError::dom("InvalidStateError", "No active document."))?;
            // As its EXIF orientation shows it (imageOrientation "from-image").
            let pixmap = pyro_canvas::codec::decode_bitmap(&blob.bytes, true, &budget)
                .ok_or_else(|| JsError::dom("InvalidStateError", "The source image could not be decoded."))?;
            let (w, h) = (u32::from(pixmap.width()), u32::from(pixmap.height()));
            Ok(Some(ImageHandle { pixmap: Some(pixmap), width: w, height: h, clean: true, space: Space::Srgb }))
        }
        // pyro: a CSSImageValue: the image of its `url()` from the list of available images
        // (nothing while it is not loaded), shown as its EXIF orientation says.
        4 => {
            let text = desc.get_index(1)?.to_string()?;
            let Some(url) = css_url_of(&text) else { return Ok(None) };
            let Some(image) = page.available_image(&url, 0) else { return Ok(None) };
            let Some((bytes, clean)) = image.bytes else { return Ok(None) };
            let Some(budget) = page.raster_budget() else { return Ok(None) };
            let Some(pixmap) = pyro_canvas::codec::decode_bitmap(bytes.bytes(), true, &budget) else {
                return Ok(None);
            };
            let (w, h) = (u32::from(pixmap.width()), u32::from(pixmap.height()));
            Ok(Some(ImageHandle { pixmap: Some(pixmap), width: w, height: h, clean, space: Space::Srgb }))
        }
        _ => Err(not_source()),
    }
}

/// pyro: the URL of a computed `url("...")` (a CSSImageValue's text), unescaped.
fn css_url_of(text: &str) -> Option<String> {
    let inner = text.trim().strip_prefix("url(")?.strip_suffix(')')?.trim();
    let unquoted = match inner.chars().next()? {
        q @ ('"' | '\'') => inner.strip_prefix(q)?.strip_suffix(q)?,
        _ => inner,
    };
    let mut out = String::with_capacity(unquoted.len());
    let mut chars = unquoted.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            out.extend(chars.next());
        } else {
            out.push(c);
        }
    }
    (!out.is_empty()).then_some(out)
}

/// `createImageBitmap` (canvas2d.js): `[handle, width, height]` of the
/// source's pixels, cropped, resized and flipped as asked.
fn make_bitmap<'s>(s: &Scope<'s>, a: Args<'s>) -> JsResult<Value<'s>> {
    let page = crate::target::page_of(s)?;
    page.canvas_used.set(true);
    let what = "Window.createImageBitmap";
    let Some(source) = image_source(s, &page, a.get(0), what, true, None)? else {
        return Err(JsError::dom("InvalidStateError", "The source image could not be decoded."));
    };
    let crop = match a.get(1).as_object() {
        Some(c) => {
            let v: Vec<f64> = (0..4).map(|i| c.get_index(i).and_then(|v| v.to_number())).collect::<JsResult<_>>()?;
            Rect::new(v[0], v[1], v[0] + v[2], v[1] + v[3]).abs()
        }
        None => Rect::new(0.0, 0.0, f64::from(source.width), f64::from(source.height)),
    };
    let (rw, rh) = (size_u32(a.get(2).to_number()?), size_u32(a.get(3).to_number()?));
    let flip = a.get(4).to_bool();
    let smooth = a.get(5).to_bool();
    let (cw, ch) = (crop.width(), crop.height());
    let (w, h) = match (rw, rh) {
        (0, 0) => (cw.round() as u32, ch.round() as u32),
        (w, 0) => (w, ((f64::from(w) * ch / cw).round() as u32).max(1)),
        (0, h) => (((f64::from(h) * cw / ch).round() as u32).max(1), h),
        (w, h) => (w, h),
    };
    if !pyro_canvas::valid_size(w, h) {
        return Err(JsError::dom("InvalidStateError", "The ImageBitmap could not be allocated."));
    }
    let whole = crop == Rect::new(0.0, 0.0, f64::from(source.width), f64::from(source.height));
    let pixmap = if whole && (w, h) == (source.width, source.height) && !flip {
        source.pixmap
    } else if let Some(image) = source.pixmap {
        let mut surface = Surface::new(w, h);
        let mut state = State::new();
        state.image_smoothing = smooth;
        if flip {
            state.transform = Affine::new([1.0, 0.0, 0.0, -1.0, 0.0, f64::from(h)]);
        }
        surface.draw_image(&image, crop, Rect::new(0.0, 0.0, f64::from(w), f64::from(h)), &state.draw_state());
        surface.snapshot()
    } else {
        None
    };
    bitmap_triple(s, ImageHandle { pixmap, width: w, height: h, clean: source.clean, space: source.space })
}

/// A copy of the bytes of a typed array (an `ImageData`'s data).
fn typed_bytes(s: &Scope<'_>, v: Value<'_>) -> JsResult<Vec<u8>> {
    Ok(crate::encoding::buffer_source_bytes(s, v)?.unwrap_or_default())
}

/// A new `Uint8ClampedArray` holding `bytes`.
fn clamped_array<'s>(s: &Scope<'s>, bytes: &[u8]) -> JsResult<Value<'s>> {
    let ctx = s.ctx();
    let mut exc = std::ptr::null();
    // SAFETY: JSObjectMakeTypedArray allocates a zeroed array of `len` bytes
    // owned by JSC; the bytes are copied in before control returns to JS.
    unsafe {
        let obj = sys::JSObjectMakeTypedArray(ctx, sys::kJSTypedArrayTypeUint8ClampedArray, bytes.len(), &mut exc);
        if !exc.is_null() || obj.is_null() {
            return Err(JsError::Range("Out of memory at ImageData creation.".into()));
        }
        if !bytes.is_empty() {
            let ptr = sys::JSObjectGetTypedArrayBytesPtr(ctx, obj, &mut exc) as *mut u8;
            if !exc.is_null() {
                return Err(JsError::Range("Out of memory at ImageData creation.".into()));
            }
            if !ptr.is_null() {
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
            }
        }
        Ok(Value::from_raw(ctx, obj as sys::JSValueRef))
    }
}

// ---- Paints ------------------------------------------------------------------

/// The paint of a style, and whether it is origin-clean. `None`: it paints
/// nothing.
fn paint_of(s: &Scope<'_>, c: &ContextData, style: &Style) -> JsResult<Option<(Paint, bool)>> {
    let (object, cache) = match style {
        Style::Color(_, color) => return Ok(Some((Paint::Color(*color), true))),
        Style::Object(o, cache) => (o.get(s), cache),
    };
    let record = module_helper(s, c, "paintRecord", &[object])?;
    let Some(record) = record.as_object() else { return Ok(None) };
    let version = record.get_atom("version")?.to_number()?;
    if let Some((v, paint, clean)) = &*cache.borrow() {
        if *v == version {
            return Ok(Some((paint.clone(), *clean)));
        }
    }
    let made = if record.get_atom("stops")?.is_object() {
        gradient_paint(record, c.space)?.map(|g| (Paint::Gradient(Arc::new(g)), true))
    } else {
        pattern_paint(record)?
    };
    if let Some((paint, clean)) = &made {
        *cache.borrow_mut() = Some((version, paint.clone(), *clean));
    }
    Ok(made)
}

fn numbers(array: Object<'_>) -> JsResult<Vec<f64>> {
    let len = array.get_atom("length")?.to_number()? as u32;
    (0..len).map(|i| array.get_index(i)?.to_number()).collect()
}

fn gradient_paint(record: Object<'_>, space: Space) -> JsResult<Option<pyro_canvas::Gradient>> {
    let kind = record.get_atom("kind")?.to_string()?;
    let Some(args) = record.get_atom("args")?.as_object() else { return Ok(None) };
    let v = numbers(args)?;
    let Some(stops) = record.get_atom("stops")?.as_object() else { return Ok(None) };
    let len = stops.get_atom("length")?.to_number()? as u32;
    let mut list = Vec::with_capacity(len as usize / 2);
    for i in (0..len).step_by(2) {
        let offset = stops.get_index(i)?.to_number()? as f32;
        // Stops keep their serialization (currentcolor is black); painted
        // in the context's color space.
        let stop = stops.get_index(i + 1)?.to_string()?;
        let color = match space {
            Space::Srgb => color_of(&stop),
            _ => pyro_dom::canvas_color::parse_in(&stop, [0.0, 0.0, 0.0, 1.0], space)
                .map_or(Color::BLACK, |c| Color::new(c.rgba)),
        };
        list.push((offset, color));
    }
    let p = |i: usize| Point::new(v[i], v[i + 1]);
    let kind = match (kind.as_str(), v.len()) {
        ("linear", 4) => pyro_canvas::GradientKind::Linear { start: p(0), end: p(2) },
        ("radial", 6) => {
            pyro_canvas::GradientKind::Radial { start: p(0), start_radius: v[2], end: p(3), end_radius: v[5] }
        }
        ("conic", 3) => pyro_canvas::GradientKind::Conic { center: p(1), angle: v[0] },
        _ => return Ok(None),
    };
    Ok(Some(pyro_canvas::Gradient { kind, stops: list }))
}

fn pattern_paint(record: Object<'_>) -> JsResult<Option<(Paint, bool)>> {
    let Some(handle) = handle_of(record.get_atom("image")?)? else { return Ok(None) };
    let repetition = record.get_atom("repetition")?.to_string()?;
    let t = record.get_atom("transform")?.as_object().map(numbers).transpose()?.unwrap_or_default();
    let transform = if t.len() == 6 { Affine::new([t[0], t[1], t[2], t[3], t[4], t[5]]) } else { Affine::IDENTITY };
    // A transparent source paints nothing visible: a 1x1 transparent image
    // keeps the pattern's geometry rules simple.
    let Some(image) = handle.pixmap.or_else(|| Bitmap::new(Pixmap::new(1, 1))) else { return Ok(None) };
    let pattern = pyro_canvas::Pattern {
        image,
        repeat_x: matches!(repetition.as_str(), "repeat" | "repeat-x"),
        repeat_y: matches!(repetition.as_str(), "repeat" | "repeat-y"),
        transform,
    };
    Ok(Some((Paint::Pattern(Arc::new(pattern)), handle.clean)))
}

// ---- Drawing -------------------------------------------------------------------

/// Draws with the fill or stroke style: `draw` gets the surface, the paint
/// and the state.
fn with_paint(
    s: &Scope<'_>,
    c: &ContextData,
    fill: bool,
    draw: impl FnOnce(&mut Surface, &Paint, &State),
) -> JsResult<()> {
    c.sync_size();
    let style = {
        let state = c.state.borrow();
        if fill {
            state.fill_style.clone()
        } else {
            state.stroke_style.clone()
        }
    };
    // No page script runs while the state is borrowed: reading the style
    // object (canvas2d.js records) happens before.
    let Some((paint, clean)) = paint_of(s, c, &style)? else { return Ok(()) };
    if !clean || c.state.borrow().filter_taints {
        c.clean.set(false);
    }
    let state = c.state.borrow();
    draw(&mut c.surface.borrow_mut(), &paint, &state);
    Ok(())
}

/// The fill rule argument at `a[i]` (absent: nonzero).
fn fill_rule(a: &Args<'_>, i: usize, what: &str) -> JsResult<bool> {
    let rule = a.get(i);
    if rule.is_undefined() {
        return Ok(false);
    }
    match dom_string(rule)?.as_str() {
        "nonzero" => Ok(false),
        "evenodd" => Ok(true),
        other => Err(JsError::Type(format!(
            "Failed to execute '{what}': The provided value '{other}' is not a valid enum value of type CanvasFillRule."
        ).into())),
    }
}

/// The path of `fill`/`stroke`/`clip` (a Path2D at `a[0]`, else the current
/// path) in device coordinates, and the index of the next argument.
fn path_arg(s: &Scope<'_>, c: &ContextData, a: &Args<'_>, what: &str) -> JsResult<(BezPath, usize)> {
    if a.get(0).is_object() {
        let Some(ops) = path2d_arg(s, c, a)? else {
            return Err(JsError::Type(format!("Failed to execute '{what}': parameter 1 is not of type 'Path2D'.").into()));
        };
        let mut path = Path::default();
        replay(ops, &mut path, c.state.borrow().transform)?;
        return Ok((path.bez, 1));
    }
    Ok((c.path.borrow().bez.clone(), 0))
}

/// `fillText`/`strokeText`.
fn draw_text(s: &Scope<'_>, c: &ContextData, a: Args<'_>, fill: bool) -> JsResult<()> {
    let what = if fill { "CanvasRenderingContext2D.fillText" } else { "CanvasRenderingContext2D.strokeText" };
    a.require(3, what)?;
    let text = dom_string(a.get(0))?;
    let (x, y) = (a.get(1).to_number()?, a.get(2).to_number()?);
    let max_width = if a.get(3).is_undefined() { None } else { Some(a.get(3).to_number()?) };
    if !x.is_finite() || !y.is_finite() || max_width.is_some_and(|m| !m.is_finite() || m <= 0.0) {
        return Ok(());
    }
    let state = c.state.borrow();
    let layout = layout_text(c, &text, &state);
    if layout.runs.is_empty() {
        return Ok(());
    }
    let scale = match max_width {
        Some(m) if layout.width > m => m / layout.width,
        _ => 1.0,
    };
    let width = layout.width * scale;
    let dx = match align_of(c, &state) {
        Align::Center => -width / 2.0,
        Align::Right => -width,
        Align::Left => 0.0,
    };
    let dy = baseline_shift(state.text_baseline, &layout);
    let stroke = (!fill).then(|| state.stroke());
    drop(state);
    with_paint(s, c, fill, |surface, paint, state| {
        surface.text(&layout, Point::new(x + dx, y + dy), scale, stroke.as_ref(), paint, &state.draw_state());
    })
}

enum Align {
    Left,
    Center,
    Right,
}

/// Whether text is right-to-left: `direction`, where `inherit` is the computed `direction` of a
/// `<canvas>` (its `dir` or an ancestor's) and left-to-right for an OffscreenCanvas.
fn is_rtl(c: &ContextData, state: &State) -> bool {
    match state.direction {
        "rtl" => true,
        "ltr" => false,
        _ => match c.target {
            Target::Element(canvas) => c.page.try_layout().is_some_and(|layout| {
                c.page.dom.try_borrow().is_ok_and(|dom| layout.computed_style_value(&dom, canvas, "direction") == "rtl")
            }),
            Target::Offscreen => false,
        },
    }
}

/// `textAlign` resolved for the direction ([`is_rtl`]).
fn align_of(c: &ContextData, state: &State) -> Align {
    let rtl = is_rtl(c, state);
    match state.text_align {
        "center" => Align::Center,
        "left" => Align::Left,
        "right" => Align::Right,
        "end" if !rtl => Align::Right,
        "start" if rtl => Align::Right,
        _ => Align::Left,
    }
}

/// pyro: a CSS <length> as its number and lowercase unit (unitless zero is `px`), `None` for
/// anything else (other units, percentages, keywords, non-finite numbers).
fn spacing_parts(value: &str) -> Option<(f64, String)> {
    let v = value.trim_matches(|c: char| c.is_ascii_whitespace()).as_bytes();
    // A CSS number: sign, digits, fraction, then an exponent only when `e` is followed by
    // digits (so `1em` and `1ex` keep their unit).
    let digits = |from: usize| from + v[from..].iter().take_while(|b| b.is_ascii_digit()).count();
    let mut i = usize::from(matches!(v.first(), Some(b'+' | b'-')));
    let mut end = digits(i);
    if v.get(end) == Some(&b'.') && digits(end + 1) > end + 1 {
        end = digits(end + 1);
    }
    if end == i {
        return None;
    }
    if matches!(v.get(end), Some(b'e' | b'E')) {
        i = end + 1 + usize::from(matches!(v.get(end + 1), Some(b'+' | b'-')));
        if digits(i) > i {
            end = digits(i);
        }
    }
    let number = std::str::from_utf8(&v[..end]).ok()?;
    let unit = std::str::from_utf8(&v[end..]).ok()?;
    let n: f64 = number.parse().ok().filter(|n: &f64| n.is_finite())?;
    // -0 serializes as 0.
    let n = if n == 0.0 { 0.0 } else { n };
    let unit = unit.to_ascii_lowercase();
    if unit.is_empty() {
        return (n == 0.0).then(|| (0.0, "px".to_string()));
    }
    const UNITS: &[&str] = &[
        "px", "cm", "mm", "q", "in", "pt", "pc", "em", "rem", "ex", "rex", "ch", "rch", "ic", "ric", "cap", "rcap",
        "lh", "rlh",
    ];
    UNITS.contains(&unit.as_str()).then_some((n, unit))
}

/// The value `letterSpacing`/`wordSpacing` keep for `value`, `None` to ignore it.
fn spacing_length(value: &str) -> Option<String> {
    spacing_parts(value).map(|(n, unit)| format!("{n}{unit}"))
}

fn spacing_px(value: &str, font_size: f64) -> f32 {
    let Some((n, unit)) = spacing_parts(value) else { return 0.0 };
    // Font-relative units against the canvas font (16px for the root's), as approximations of
    // the font's own metrics where they differ (ex, cap, ch: half or 0.7 of the em).
    (n * match unit.as_str() {
        "cm" => 96.0 / 2.54,
        "mm" => 96.0 / 25.4,
        "q" => 96.0 / 101.6,
        "in" => 96.0,
        "pt" => 96.0 / 72.0,
        "pc" => 16.0,
        "em" | "ic" => font_size,
        "ex" | "ch" => font_size / 2.0,
        "cap" => font_size * 0.7,
        "lh" => font_size * 1.2,
        "rem" | "ric" => 16.0,
        "rex" | "rch" => 8.0,
        "rcap" => 16.0 * 0.7,
        "rlh" => 16.0 * 1.2,
        _ => 1.0,
    }) as f32
}

/// The language canvas text is shaped for: `lang`, where `inherit` is the language of a
/// `<canvas>` (its or an ancestor's `lang`, else the document's) and unknown for an
/// OffscreenCanvas; `None` when unknown (`""`).
fn text_lang(c: &ContextData, state: &State) -> Option<String> {
    let lang = match (state.lang.as_str(), c.target) {
        ("inherit", Target::Element(canvas)) => c.page.dom.try_borrow().ok()?.language(canvas).ok()??,
        ("inherit", Target::Offscreen) => return None,
        (lang, _) => lang.to_owned(),
    };
    (!lang.is_empty()).then_some(lang)
}

fn layout_text(c: &ContextData, text: &str, state: &State) -> pyro_canvas::text::TextLayout {
    let page = &*c.page;
    let lang = text_lang(c, state);
    let font = &state.font;
    let style = pyro_canvas::text::TextStyle {
        family: &font.family,
        size: font.size as f32,
        weight: f32::from(font.weight),
        italic: font.style != "normal",
        letter_spacing: spacing_px(&state.letter_spacing, font.size),
        word_spacing: spacing_px(&state.word_spacing, font.size),
        lang: lang.as_deref(),
        variant_caps: font.variant_caps,
    };
    let (key, faces) = canvas_web_faces(page, &font.family);
    pyro_canvas::text::layout_with_faces(text, &style, key, &faces)
}

/// pyro: the web faces canvas text in `page`'s realm draws `family` (a CSS family list) with: the
/// loaded faces of the realm's `FontFaceSet` (a document's or a worker's) and, in a document, the
/// `@font-face` rules whose `url()` source its layout loaded, each only when the list names its
/// family. The key names the set (no faces: plain layout); the faces are made once per key.
fn canvas_web_faces(page: &PageDom, family: &str) -> (u64, Rc<Vec<pyro_canvas::text::WebFace>>) {
    use std::hash::{Hash, Hasher};
    let wanted: Vec<String> = family
        .split(',')
        .map(|f| f.trim().trim_matches(|c| c == '"' || c == '\'').trim().to_ascii_lowercase())
        .filter(|f| !f.is_empty())
        .collect();
    // Generic families only (the common case): no face can be named by them.
    const GENERIC: [&str; 10] =
        ["serif", "sans-serif", "monospace", "cursive", "fantasy", "system-ui", "math", "emoji", "fangsong", "ui-monospace"];
    if wanted.iter().all(|w| GENERIC.contains(&w.as_str())) {
        return (0, Rc::default());
    }
    let named = |name: &str| {
        let name = name.trim().trim_matches(|c| c == '"' || c == '\'').trim().to_ascii_lowercase();
        wanted.iter().any(|w| *w == name)
    };
    let (generation, script) = page.canvas_faces();
    let script: Vec<&pyro_canvas::text::WebFace> = script.iter().filter(|f| named(&f.family)).collect();
    let mut css = Vec::new();
    for face in page.css_font_faces().iter().filter(|f| named(&f.family)) {
        let loaded = face.sources.iter().find_map(|source| match source {
            pyro_dom::font_face::FontSource::Url(url, _) => page.font_url_bytes(url).map(|bytes| (url.clone(), bytes)),
            pyro_dom::font_face::FontSource::Local(_) => None,
        });
        if let Some((url, bytes)) = loaded {
            css.push((face.family.clone(), url, bytes));
        }
    }
    if script.is_empty() && css.is_empty() {
        return (0, Rc::default());
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (page as *const PageDom as usize).hash(&mut hasher);
    generation.hash(&mut hasher);
    for face in &script {
        face.family.hash(&mut hasher);
        face.data.id().hash(&mut hasher);
    }
    for (family, url, bytes) in &css {
        family.hash(&mut hasher);
        url.hash(&mut hasher);
        (Arc::as_ptr(bytes) as usize, bytes.len()).hash(&mut hasher);
    }
    let key = hasher.finish().max(1);
    let faces = page.canvas_face_cache(key, || {
        let mut faces: Vec<pyro_canvas::text::WebFace> = script.iter().map(|f| (*f).clone()).collect();
        for (family, _, bytes) in css {
            let compressed = matches!(bytes.get(..4), Some(b"wOFF" | b"wOF2"));
            let face = if compressed {
                crate::font_loading::sfnt_bytes(bytes.to_vec()).map(|sfnt| pyro_canvas::text::WebFace::new(family, sfnt))
            } else {
                Some(pyro_canvas::text::WebFace::shared(family, bytes))
            };
            faces.extend(face);
        }
        faces
    });
    (key, faces)
}

/// `drawImage` with 3, 5 or 9 arguments.
fn draw_image<'s>(s: &Scope<'s>, c: &ContextData, a: Args<'s>) -> JsResult<Value<'s>> {
    let what = "CanvasRenderingContext2D.drawImage";
    a.require(3, what)?;
    let n = a.len();
    if !matches!(n, 3 | 5) && n < 9 {
        return Err(JsError::Type(format!("Failed to execute 'drawImage' on 'CanvasRenderingContext2D': Valid arities are: [3, 5, 9], but {n} arguments provided.").into()));
    }
    let Some(source) = image_source(s, &c.page, a.get(0), what, false, Some(c.target))? else { return Ok(s.undefined()) };
    let v = finite_args_from(&a, 1, if n >= 9 { 8 } else { n - 1 })?;
    let Some(v) = v else { return Ok(s.undefined()) };
    // HTML: the source is converted to the canvas's color space.
    let source = handle_in(source, c.space);
    let (sw, sh) = (f64::from(source.width), f64::from(source.height));
    let (src, dst) = match v.len() {
        2 => (Rect::new(0.0, 0.0, sw, sh), Rect::new(v[0], v[1], v[0] + sw, v[1] + sh)),
        4 => (Rect::new(0.0, 0.0, sw, sh), Rect::new(v[0], v[1], v[0] + v[2], v[1] + v[3])),
        _ => (Rect::new(v[0], v[1], v[0] + v[2], v[1] + v[3]), Rect::new(v[4], v[5], v[4] + v[6], v[5] + v[7])),
    };
    if !source.clean {
        c.clean.set(false);
    }
    c.sync_size();
    let state = c.state.borrow().clone();
    if state.filter_taints {
        c.clean.set(false);
    }
    // A source with no bitmap yet (a canvas without a context) is drawn as
    // transparent black, which `copy`, `source-in`, ... do not ignore.
    let Some(image) = source.pixmap else {
        c.surface.borrow_mut().draw_transparent_image(source.width, source.height, src, dst, &state.draw_state());
        return Ok(s.undefined());
    };
    // The source's size may differ from its pixels' (a canvas snapshot is
    // its bitmap); map the source rectangle onto the pixels.
    let (px, py) = (f64::from(image.width()) / sw.max(1.0), f64::from(image.height()) / sh.max(1.0));
    let src = Rect::new(src.x0 * px, src.y0 * py, src.x1 * px, src.y1 * py);
    c.surface.borrow_mut().draw_image(&image, src, dst, &state.draw_state());
    Ok(s.undefined())
}

/// HTML-in-Canvas `drawElementImage(element, dx, dy)`, `(element, dx, dy, dw, dh)` and
/// `(element, sx, sy, sw, sh, dx, dy, dw, dh)`: draws a drawable element of this context's
/// `<canvas content=drawable>`, painted alone at its CSS size (`crate::html_in_canvas::element_image`,
/// which throws InvalidStateError for an element it may not draw), as `drawImage` draws a bitmap
/// (the source rectangle in the element's CSS px). Returns the transform from the element's box to
/// the canvas grid: the current transform, then the destination's offset and scale. Only this
/// method reaches the element painting; nothing else in the context changes.
fn draw_element_image<'s>(s: &Scope<'s>, c: &ContextData, a: Args<'s>) -> JsResult<Value<'s>> {
    a.require(3, "CanvasRenderingContext2D.drawElementImage")?;
    let mut n = a.len();
    // A trailing DrawElementImageOptions dictionary ({preserveElementGeometry}).
    let options = (matches!(n, 4 | 6 | 10) && a.get(n - 1).is_object()).then(|| a.get(n - 1));
    if options.is_some() {
        n -= 1;
    }
    let preserve = match options.and_then(|o| o.as_object()) {
        Some(o) => o.get("preserveElementGeometry")?.to_bool(),
        None => false,
    };
    if !matches!(n, 3 | 5) && n < 9 {
        return Err(JsError::Type(format!("Failed to execute 'drawElementImage' on 'CanvasRenderingContext2D': Valid arities are: [3, 5, 9], but {n} arguments provided.").into()));
    }
    // An ElementImage (`captureElementImage`) draws what it captured.
    let snapshot = s.unwrap::<crate::html_in_canvas::ElementImageClass>(a.get(0)).map(|image| {
        crate::html_in_canvas::ElementImage {
            width: image.width,
            height: image.height,
            pixels: image.pixels.as_ref().map(|(w, h, data)| (*w, *h, (**data).clone())),
            taints: image.taints,
        }
    });
    let element = match snapshot {
        Some(_) => None,
        None => Some(
            s.unwrap::<crate::node::Node>(a.get(0))
                .filter(|node| Rc::ptr_eq(&node.page().dom, &c.page.dom) && node.page().dom.borrow().element(node.id).is_ok())
                .map(|node| node.id)
                .ok_or_else(|| JsError::Type("Failed to execute 'drawElementImage' on 'CanvasRenderingContext2D': parameter 1 is not of type '(Element or ElementImage)'.".into()))?,
        ),
    };
    let Target::Element(canvas) = c.target else {
        return Err(JsError::dom("InvalidStateError", "Failed to execute 'drawElementImage': The canvas has no drawable content."));
    };
    // WebIDL: the numbers are converted (their valueOf run) before the method's steps can throw.
    let v = finite_args_from(&a, 1, if n >= 9 { 8 } else { n - 1 })?;
    let image = match (snapshot, element) {
        (Some(image), _) => image,
        (None, Some(element)) => crate::html_in_canvas::element_image(&c.page, canvas, element)?,
        (None, None) => return Ok(s.undefined()),
    };
    let Some(v) = v else { return Ok(s.undefined()) };
    let (w, h) = (image.width, image.height);
    let (src, dst) = match v.len() {
        2 => (Rect::new(0.0, 0.0, w, h), Rect::new(v[0], v[1], v[0] + w, v[1] + h)),
        4 => (Rect::new(0.0, 0.0, w, h), Rect::new(v[0], v[1], v[0] + v[2], v[1] + v[3])),
        _ => (Rect::new(v[0], v[1], v[0] + v[2], v[1] + v[3]), Rect::new(v[4], v[5], v[4] + v[6], v[5] + v[7])),
    };
    c.sync_size();
    let state = c.state.borrow().clone();
    // Privacy: what the page must not read back (an image of unknown origin, a nested
    // document, a visited link) leaves the canvas not origin-clean, as a cross-origin drawImage;
    // so does a filter that taints (as drawImage's `filter_taints`).
    if image.taints || state.filter_taints {
        c.clean.set(false);
    }
    if let Some((pw, ph, rgba)) = image.pixels {
        // Painted in sRGB, converted to the canvas's color space; the source rectangle maps
        // onto the pixels (one per CSS px, rounded up).
        let bitmap = pyro_canvas::pixmap_from_straight(rgba, pw, ph)
            .and_then(Bitmap::new)
            .and_then(|bitmap| convert_bitmap(bitmap, Space::Srgb, c.space));
        if let Some(bitmap) = bitmap {
            let (px, py) = (f64::from(pw) / w.max(1.0), f64::from(ph) / h.max(1.0));
            let src = Rect::new(src.x0 * px, src.y0 * py, src.x1 * px, src.y1 * py);
            c.surface.borrow_mut().draw_image(&bitmap, src, dst, &state.draw_state());
        }
    }
    let scale = |to: f64, from: f64| if from != 0.0 { to / from } else { 1.0 };
    let place = Affine::translate((dst.x0, dst.y0))
        * Affine::scale_non_uniform(scale(dst.width(), src.width()), scale(dst.height(), src.height()))
        * Affine::translate((-src.x0, -src.y0));
    let [ma, mb, mc, md, me, mf] = (state.transform * place).as_coeffs();
    // The element's geometry follows where it was drawn, unless asked to keep it.
    if let (Some(element), false) = (element, preserve) {
        crate::html_in_canvas::set_drawn_transform(&c.page, canvas, element, vec![ma, mb, mc, md, me, mf]);
    }
    let args = [ma, mb, mc, md, me, mf].map(|v| s.number(v));
    module_helper(s, c, "matrix", &args)
}

fn finite_args_from(a: &Args<'_>, from: usize, n: usize) -> JsResult<Option<Vec<f64>>> {
    let mut v = Vec::with_capacity(n);
    for i in from..from + n {
        v.push(a.get(i).to_number()?);
    }
    Ok(v.iter().all(|x| x.is_finite()).then_some(v))
}

/// `getImageData(sx, sy, sw, sh, settings)`.
fn get_image_data<'s>(s: &Scope<'s>, c: &ContextData, a: Args<'s>) -> JsResult<Value<'s>> {
    a.require(4, "CanvasRenderingContext2D.getImageData")?;
    let (mut x, mut y, mut w, mut h) = (long(a.get(0))?, long(a.get(1))?, long(a.get(2))?, long(a.get(3))?);
    if w == 0 || h == 0 {
        return Err(JsError::dom(
            "IndexSizeError",
            format!("The source {} is 0.", if w == 0 { "width" } else { "height" }),
        ));
    }
    // ImageDataSettings.colorSpace: the context's when absent.
    let space = space_member(a.get(4), c.space)?;
    // ImageDataSettings.pixelFormat (validated by canvas2d.js): float data
    // is made from the 8-bit pixels.
    let format = module_helper(s, c, "pixelFormatOf", &[a.get(4)])?;
    if !c.clean.get() {
        return Err(security_error());
    }
    if w < 0 {
        x += w;
        w = -w;
    }
    if h < 0 {
        y += h;
        h = -h;
    }
    if (w as u64) * (h as u64) > pyro_canvas::MAX_AREA {
        return Err(JsError::Range("Out of memory at ImageData creation.".into()));
    }
    c.sync_size();
    let mut pixels = c.surface.borrow_mut().get_image_data(x, y, w as u32, h as u32);
    pyro_dom::canvas_color::convert_pixels(&mut pixels, c.space, space);
    let data = clamped_array(s, &pixels)?;
    module_helper(s, c, "imageDataOf", &[data, s.number(w as f64), s.number(h as f64), s.string(space.name()), format])
}

/// `putImageData(imagedata, dx, dy, dirtyX, dirtyY, dirtyWidth, dirtyHeight)`.
fn put_image_data<'s>(s: &Scope<'s>, c: &ContextData, a: Args<'s>) -> JsResult<Value<'s>> {
    a.require(3, "CanvasRenderingContext2D.putImageData")?;
    let info = module_helper(s, c, "imageInfo", &[a.get(0)])?;
    let Some(info) = info.as_object() else {
        return Err(JsError::Type(
            "Failed to execute 'putImageData' on 'CanvasRenderingContext2D': parameter 1 is not of type 'ImageData'."
                .into(),
        ));
    };
    let (iw, ih) = (info.get_index(0)?.to_number()? as i64, info.get_index(1)?.to_number()? as i64);
    let (dx, dy) = (long(a.get(1))?, long(a.get(2))?);
    let (mut bx, mut by, mut bw, mut bh) = (0, 0, iw, ih);
    if a.len() >= 7 {
        (bx, by, bw, bh) = (long(a.get(3))?, long(a.get(4))?, long(a.get(5))?, long(a.get(6))?);
        if bw < 0 {
            bx += bw;
            bw = -bw;
        }
        if bh < 0 {
            by += bh;
            bh = -bh;
        }
        if bx < 0 {
            bw += bx;
            bx = 0;
        }
        if by < 0 {
            bh += by;
            by = 0;
        }
        bw = bw.min(iw - bx);
        bh = bh.min(ih - by);
    }
    if bw <= 0 || bh <= 0 {
        return Ok(s.undefined());
    }
    // Float data is put as its 8-bit copy.
    let pixels = module_helper(s, c, "unorm8Of", &[info.get_index(2)?])?;
    let mut data = typed_bytes(s, pixels)?;
    // The ImageData's values are converted to the canvas's color space.
    let from = info.get_index(3)?;
    let from = if from.is_string() { Space::parse(&from.to_string()?) } else { None };
    pyro_dom::canvas_color::convert_pixels(&mut data, from.unwrap_or_default(), c.space);
    c.sync_size();
    c.surface.borrow_mut().put_image_data(
        &data,
        iw as u32,
        ih as u32,
        dx,
        dy,
        (bx as u32, by as u32, bw as u32, bh as u32),
    );
    Ok(s.undefined())
}

// ---- The context ----------------------------------------------------------

fn finite_args(a: &Args<'_>, n: usize) -> JsResult<Option<Vec<f64>>> {
    finite_args_from(a, 0, n)
}

/// Web IDL `double` arguments: non-finite values throw.
fn doubles(a: &Args<'_>, n: usize, what: &str) -> JsResult<Vec<f64>> {
    a.require(n, what)?;
    let v = (0..n).map(|i| a.get(i).to_number()).collect::<JsResult<Vec<_>>>()?;
    if v.iter().any(|x| !x.is_finite()) {
        return Err(JsError::Type(format!("Failed to execute '{what}': The provided double value is non-finite.").into()));
    }
    Ok(v)
}

/// Web IDL `[EnforceRange] long`.
fn long(v: Value<'_>) -> JsResult<i64> {
    // pyro: every caller's argument is `[EnforceRange] long` (createImageData, getImageData,
    // putImageData): NaN, infinities and values out of the long range throw a TypeError (WPT
    // 2d.imageData.{create2,get,put}.nonfinite).
    let n = v.to_number()?;
    let t = n.trunc();
    if !n.is_finite() || !(-2_147_483_648.0..=2_147_483_647.0).contains(&t) {
        return Err(JsError::Type("Value is outside the 'long' value range.".into()));
    }
    Ok(t as i64)
}

fn module_helper<'s>(s: &Scope<'s>, c: &ContextData, name: &str, args: &[Value<'s>]) -> JsResult<Value<'s>> {
    MODULE.helper(s, &c.page, name, args)
}

fn style_value<'s>(s: &Scope<'s>, style: &Style) -> Value<'s> {
    match style {
        Style::Color(c, _) => s.string(c),
        Style::Object(o, _) => o.get(s),
    }
}

fn set_style(s: &Scope<'_>, c: &ContextData, v: Value<'_>, fill: bool) -> JsResult<()> {
    let color = |text: String| context_color(c, &text).map(|c| Style::Color(c.css, Color::new(c.rgba)));
    let style = if v.is_object() {
        if !module_helper(s, c, "isFillStyle", &[v])?.to_bool() {
            // Any other object is converted to a string, as for a color.
            match color(dom_string(v)?) {
                Some(style) => style,
                None => return Ok(()),
            }
        } else {
            let style = Style::Object(Persistent::new(s, v), Rc::default());
            // HTML: setting a style to a pattern that is not origin-clean
            // makes the canvas origin-unclean at once (not when it paints).
            if let Some((_, false)) = paint_of(s, c, &style)? {
                c.clean.set(false);
            }
            style
        }
    } else {
        match color(dom_string(v)?) {
            Some(style) => style,
            None => return Ok(()),
        }
    };
    let mut state = c.state.borrow_mut();
    if fill {
        state.fill_style = style;
    } else {
        state.stroke_style = style;
    }
    Ok(())
}

/// A number attribute: non-finite values (and those `valid` rejects) are ignored.
macro_rules! number_state {
    ($def:ident, $name:literal, $field:ident, $valid:expr) => {
        $def.accessor(
            $name,
            |s, c| Ok(s.number(c.state.borrow().$field)),
            |_s, c, v| {
                let n = v.to_number()?;
                let valid: fn(f64) -> bool = $valid;
                if n.is_finite() && valid(n) {
                    c.state.borrow_mut().$field = n;
                }
                Ok(())
            },
        );
    };
}

/// An enumerated attribute: values outside `$values` are ignored.
macro_rules! enum_state {
    ($def:ident, $name:literal, $field:ident, [$($value:literal),+]) => {
        $def.accessor(
            $name,
            |s, c| Ok(s.string(c.state.borrow().$field)),
            |_s, c, v| {
                let v = dom_string(v)?;
                if let Some(&keyword) = [$($value),+].iter().find(|k| **k == v) {
                    c.state.borrow_mut().$field = keyword;
                }
                Ok(())
            },
        );
    };
}

/// A CSS-valued string attribute kept as given (`filter`, spacing).
macro_rules! string_state {
    ($def:ident, $name:literal, $field:ident) => {
        $def.accessor(
            $name,
            |s, c| Ok(s.string(&c.state.borrow().$field)),
            |_s, c, v| {
                c.state.borrow_mut().$field = dom_string(v)?;
                Ok(())
            },
        );
    };
}

/// pyro: `letterSpacing`/`wordSpacing`: a value that is not a CSS <length> is ignored, one that
/// is is kept serialized (WPT 2d.text.drawing.style.*.spacing).
macro_rules! spacing_state {
    ($def:ident, $name:literal, $field:ident) => {
        $def.accessor(
            $name,
            |s, c| Ok(s.string(&c.state.borrow().$field)),
            |_s, c, v| {
                if let Some(value) = spacing_length(&dom_string(v)?) {
                    c.state.borrow_mut().$field = value;
                }
                Ok(())
            },
        );
    };
}

/// `drawFocusIfNeeded([path,] element)`: when `element` is focused and is
/// fallback content of this context's `<canvas>`, the path (the current
/// one by default) is outlined with a focus ring: a 2 px ring in the
/// platform focus color (Chromium's auto outline, #101010), inside the
/// clipping region but without the shadow, global alpha, compositing or
/// filter.
fn draw_focus_if_needed<'s>(s: &Scope<'s>, c: &ContextData, a: Args<'s>) -> JsResult<Value<'s>> {
    let what = "CanvasRenderingContext2D.drawFocusIfNeeded";
    a.require(1, what)?;
    // One argument is the element: the current path is outlined.
    let (path, next) = if a.len() >= 2 { path_arg(s, c, &a, what)? } else { (c.path.borrow().bez.clone(), 0) };
    let not_element = || {
        JsError::Type(
            format!("Failed to execute 'drawFocusIfNeeded' on 'CanvasRenderingContext2D': parameter {} is not of type 'Element'.", next + 1)
                .into(),
        )
    };
    let node = s.unwrap::<crate::node::Node>(a.get(next)).ok_or_else(not_element)?;
    let Target::Element(canvas) = c.target else { return Ok(s.undefined()) };
    let shown = {
        let dom = c.page.dom.borrow();
        if dom.element(node.id).is_err() {
            return Err(not_element());
        }
        let focused = dom.node_document(node.id).ok().and_then(|doc| dom.focused(doc).ok().flatten());
        focused == Some(node.id) && node.id != canvas && dom.contains(canvas, node.id).unwrap_or(false)
    };
    if !shown || path.elements().is_empty() {
        return Ok(s.undefined());
    }
    c.sync_size();
    let state = c.state.borrow();
    let draw = DrawState {
        transform: Affine::IDENTITY,
        alpha: 1.0,
        composite: Composite::SourceOver,
        clip: &state.clip,
        shadow: None,
        smoothing: state.image_smoothing,
        filter: None,
    };
    let ring = Paint::Color(Color::from_rgba8(0x10, 0x10, 0x10, 255));
    c.surface.borrow_mut().stroke(&path, &kurbo::Stroke::new(2.0), &ring, &draw);
    Ok(s.undefined())
}

/// The path argument of `fill`/`stroke`/`clip`/`isPointIn*`: a Path2D at
/// `a[0]`, or None.
fn path2d_arg<'s>(s: &Scope<'s>, c: &ContextData, a: &Args<'s>) -> JsResult<Option<Object<'s>>> {
    if !a.get(0).is_object() {
        return Ok(None);
    }
    let ops = module_helper(s, c, "pathOps", &[a.get(0)])?;
    Ok(ops.as_object())
}

fn is_point_in<'s>(s: &Scope<'s>, c: &ContextData, a: Args<'s>, stroke: bool) -> JsResult<Value<'s>> {
    let what =
        if stroke { "CanvasRenderingContext2D.isPointInStroke" } else { "CanvasRenderingContext2D.isPointInPath" };
    let ops = path2d_arg(s, c, &a)?;
    if a.get(0).is_object() && ops.is_none() {
        return Err(JsError::Type(format!("Failed to execute '{what}': parameter 1 is not of type 'Path2D'.").into()));
    }
    let base = usize::from(ops.is_some());
    a.require(base + 2, what)?;
    let x = a.get(base).to_number()?;
    let y = a.get(base + 1).to_number()?;
    let evenodd = fill_rule(&a, base + 2, what)?;
    let p = Point::new(x, y);
    if !x.is_finite() || !y.is_finite() {
        return Ok(s.boolean(false));
    }
    let state = c.state.borrow().clone();
    let inside = |path: &BezPath| {
        if stroke {
            // The path and the point in the current user space, where the
            // line width and dashes apply (as drawing strokes); nothing is
            // inside under a transform that cannot be inverted.
            let t = state.transform;
            let det = t.determinant();
            if det == 0.0 || !det.is_finite() || !t.as_coeffs().iter().all(|v| v.is_finite()) {
                return false;
            }
            let user = t.inverse() * path.clone();
            let (outline, k) = stroke_outline(&user, &state);
            let q = t.inverse() * p;
            let q = Point::new(q.x * k, q.y * k);
            outline.winding(q) != 0 || on_outline(&outline, q)
        } else {
            // HTML: nothing is inside under a transform that cannot be
            // inverted (scale(0, 0)); an overflowing determinant
            // (scale(Number.MAX_VALUE)) still inverts.
            let t = state.transform;
            if t.determinant() == 0.0 || !t.as_coeffs().iter().all(|v| v.is_finite()) {
                return false;
            }
            point_in_fill(path, p, evenodd)
        }
    };
    let result = match ops {
        Some(ops) => {
            let mut path = Path::default();
            replay(ops, &mut path, state.transform)?;
            inside(&path.bez)
        }
        None => inside(&c.path.borrow().bez),
    };
    Ok(s.boolean(result))
}

/// Baseline offsets from the alphabetic baseline (up is positive) for
/// `textBaseline`, given the primary font's ascent, descent and `BASE` table.
fn baseline_shift(baseline: &str, layout: &pyro_canvas::text::TextLayout) -> f64 {
    let (ascent, descent) = (layout.ascent, layout.descent);
    match baseline {
        "top" => ascent,
        "hanging" => hanging_baseline(layout),
        "middle" => (ascent - descent) / 2.0,
        "ideographic" => ideographic_baseline(layout),
        "bottom" => -descent,
        _ => 0.0,
    }
}

/// The hanging baseline above the alphabetic one: the font's `BASE` table's, else 80% of the
/// ascent (Chromium).
fn hanging_baseline(layout: &pyro_canvas::text::TextLayout) -> f64 {
    layout.baselines.hanging.unwrap_or(layout.ascent * 0.8)
}

/// The ideographic baseline above the alphabetic one: the font's `BASE` table's, else the
/// descent below it (Chromium).
fn ideographic_baseline(layout: &pyro_canvas::text::TextLayout) -> f64 {
    layout.baselines.ideographic.unwrap_or(-layout.descent)
}

use kurbo::Shape as _;

pub struct CanvasRenderingContext2D;
impl JsClass for CanvasRenderingContext2D {
    const NAME: &'static str = "CanvasRenderingContext2D";
    type Data = ContextData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        ContextData::define_members(def);
        def.method("drawFocusIfNeeded", 1, draw_focus_if_needed);
    }
}

pub struct OffscreenCanvasRenderingContext2D;
impl JsClass for OffscreenCanvasRenderingContext2D {
    const NAME: &'static str = "OffscreenCanvasRenderingContext2D";
    type Data = ContextData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        ContextData::define_members(def);
    }
}

impl ContextData {
    /// HTML includes the same drawing mixins on two distinct interfaces.
    /// Register each operation against the brand of its owning prototype.
    fn define_members<C: JsClass<Data = ContextData>>(def: &mut ClassDef<C>) {
        def.getter_this("canvas", |s, c, this| match c.target {
            Target::Element(canvas) => node_value(s, &c.page, Some(canvas)),
            Target::Offscreen => Ok(s.get_hidden(this, OFFSCREEN)?.unwrap_or_else(|| s.null())),
        });
        def.method("getContextAttributes", 0, |s, c, _a| {
            let o = s.object();
            o.set_atom("alpha", s.boolean(true))?;
            o.set_atom("colorSpace", s.string(c.space.name()))?;
            o.set_atom("desynchronized", s.boolean(false))?;
            o.set_atom("willReadFrequently", s.boolean(false))?;
            Ok(o.value())
        });
        def.pure_method("isContextLost", 0, |s, _c, _a| Ok(s.boolean(false)));
        def.method("reset", 0, |s, c, _a| {
            c.reset();
            let mut surface = c.surface.borrow_mut();
            let (w, h) = (surface.width(), surface.height());
            surface.resize(w, h);
            Ok(s.undefined())
        });
        def.accessor(
            "font",
            |s, c| Ok(s.string(&c.state.borrow().font.serialize())),
            |_s, c, v| {
                // Invalid values are ignored; em/% resolve against the
                // canvas element's computed font-size, else 10px. The
                // shorthand also sets fontVariantCaps and fontStretch.
                let value = dom_string(v)?;
                if let Some(font) = text_metrics::parse_font(&value, font_parent_size(c)) {
                    c.state.borrow_mut().font = font;
                }
                Ok(())
            },
        );
        enum_state!(def, "textAlign", text_align, ["start", "end", "left", "right", "center"]);
        enum_state!(
            def,
            "textBaseline",
            text_baseline,
            ["top", "hanging", "middle", "alphabetic", "ideographic", "bottom"]
        );
        enum_state!(def, "direction", direction, ["ltr", "rtl", "inherit"]);
        enum_state!(def, "lineCap", line_cap, ["butt", "round", "square"]);
        enum_state!(def, "lineJoin", line_join, ["round", "bevel", "miter"]);
        enum_state!(def, "imageSmoothingQuality", image_smoothing_quality, ["low", "medium", "high"]);
        enum_state!(def, "fontKerning", font_kerning, ["auto", "normal", "none"]);
        def.accessor(
            "fontStretch",
            |s, c| Ok(s.string(c.state.borrow().font.stretch)),
            |_s, c, v| {
                let v = dom_string(v)?;
                if let Some(&keyword) = text_metrics::STRETCHES.iter().find(|k| **k == v) {
                    c.state.borrow_mut().font.stretch = keyword;
                }
                Ok(())
            },
        );
        def.accessor(
            "fontVariantCaps",
            |s, c| Ok(s.string(c.state.borrow().font.variant_caps)),
            |_s, c, v| {
                let v = dom_string(v)?;
                if let Some(&keyword) = text_metrics::VARIANT_CAPS.iter().find(|k| **k == v) {
                    c.state.borrow_mut().font.variant_caps = keyword;
                }
                Ok(())
            },
        );
        enum_state!(
            def,
            "textRendering",
            text_rendering,
            ["auto", "optimizeSpeed", "optimizeLegibility", "geometricPrecision"]
        );
        number_state!(def, "globalAlpha", global_alpha, |n| (0.0..=1.0).contains(&n));
        number_state!(def, "lineWidth", line_width, |n| n > 0.0);
        number_state!(def, "miterLimit", miter_limit, |n| n > 0.0);
        number_state!(def, "lineDashOffset", line_dash_offset, |_| true);
        number_state!(def, "shadowBlur", shadow_blur, |n| n >= 0.0);
        number_state!(def, "shadowOffsetX", shadow_offset_x, |_| true);
        number_state!(def, "shadowOffsetY", shadow_offset_y, |_| true);
        def.accessor(
            "filter",
            |s, c| Ok(s.string(&c.state.borrow().filter)),
            |_s, c, v| {
                let value = dom_string(v)?;
                // pyro: HTML ignores a value that does not parse as a <filter-value-list> or
                // `none`; CSS-wide keywords are not values here (WPT 2d.filter.value).
                let keyword = value.trim();
                if ["inherit", "initial", "unset", "revert", "revert-layer"]
                    .iter()
                    .any(|k| keyword.eq_ignore_ascii_case(k))
                    || !pyro_dom::stylo::supports_declaration("filter", &value)
                {
                    return Ok(());
                }
                let (spec, taints) = canvas_filter(&c.page, &value);
                let mut state = c.state.borrow_mut();
                state.filter = value;
                state.filter_spec = spec;
                state.filter_taints = taints;
                Ok(())
            },
        );
        spacing_state!(def, "letterSpacing", letter_spacing);
        spacing_state!(def, "wordSpacing", word_spacing);
        string_state!(def, "lang", lang);
        def.accessor(
            "globalCompositeOperation",
            |s, c| Ok(s.string(c.state.borrow().composite)),
            |_s, c, v| {
                let v = dom_string(v)?;
                if let Some((name, _)) = Composite::ALL.iter().find(|(k, _)| *k == v) {
                    c.state.borrow_mut().composite = name;
                }
                Ok(())
            },
        );
        def.accessor(
            "imageSmoothingEnabled",
            |s, c| Ok(s.boolean(c.state.borrow().image_smoothing)),
            |_s, c, v| {
                c.state.borrow_mut().image_smoothing = v.to_bool();
                Ok(())
            },
        );
        def.accessor(
            "shadowColor",
            |s, c| Ok(s.string(&c.state.borrow().shadow_color)),
            |_s, c, v| {
                if let Some(color) = context_color(c, &dom_string(v)?) {
                    let mut state = c.state.borrow_mut();
                    state.shadow_rgba = Color::new(color.rgba);
                    state.shadow_color = color.css;
                }
                Ok(())
            },
        );
        def.accessor(
            "fillStyle",
            |s, c| Ok(style_value(s, &c.state.borrow().fill_style)),
            |s, c, v| set_style(s, c, v, true),
        );
        def.accessor(
            "strokeStyle",
            |s, c| Ok(style_value(s, &c.state.borrow().stroke_style)),
            |s, c, v| set_style(s, c, v, false),
        );
        def.method("measureText", 1, |s, c, a| {
            let text = arg_string(&a, 0, "CanvasRenderingContext2D.measureText")?;
            let state = c.state.borrow();
            let layout = layout_text(c, &text, &state);
            let (width, ascent, descent) = (layout.width, layout.ascent, layout.descent);
            // Where the text starts relative to the alignment point.
            let start = match align_of(c, &state) {
                Align::Center => -width / 2.0,
                Align::Right => -width,
                Align::Left => 0.0,
            };
            let shift = baseline_shift(state.text_baseline, &layout);
            let ink = layout.ink.unwrap_or_default();
            let values = [
                width,
                -(start + ink.x0),
                start + ink.x1,
                ascent - shift,
                descent + shift,
                -ink.y0 - shift,
                ink.y1 + shift,
                ascent - shift,
                descent + shift,
                hanging_baseline(&layout) - shift,
                -shift,
                ideographic_baseline(&layout) - shift,
            ];
            drop(state);
            let values: Vec<Value<'_>> = values.iter().map(|v| s.number(*v)).collect();
            let array = s.array(&values)?;
            module_helper(s, c, "textMetrics", &[array.value()])
        });
        def.method("save", 0, |s, c, _a| {
            let state = c.state.borrow().clone();
            c.stack.borrow_mut().push(state);
            Ok(s.undefined())
        });
        def.method("restore", 0, |s, c, _a| {
            if let Some(state) = c.stack.borrow_mut().pop() {
                *c.state.borrow_mut() = state;
            }
            Ok(s.undefined())
        });

        // Transforms.
        def.method("getTransform", 0, |s, c, _a| {
            let [a, b, cc, d, e, f] = c.state.borrow().transform.as_coeffs();
            let args = [a, b, cc, d, e, f].map(|v| s.number(v));
            module_helper(s, c, "matrix", &args)
        });
        def.method("setTransform", 0, |s, c, a| {
            let m = if a.len() >= 6 {
                let Some(v) = finite_args(&a, 6)? else { return Ok(s.undefined()) };
                [v[0], v[1], v[2], v[3], v[4], v[5]]
            } else {
                let array = module_helper(s, c, "matrix2D", &[a.get(0)])?;
                let array = array.as_object().ok_or(JsError::ContextDestroyed)?;
                let mut m = [0.0; 6];
                for (i, slot) in m.iter_mut().enumerate() {
                    *slot = array.get_index(i as u32)?.to_number()?;
                }
                if m.iter().any(|v| !v.is_finite()) {
                    return Ok(s.undefined());
                }
                m
            };
            c.state.borrow_mut().transform = Affine::new(m);
            Ok(s.undefined())
        });
        def.method("resetTransform", 0, |s, c, _a| {
            c.state.borrow_mut().transform = Affine::IDENTITY;
            Ok(s.undefined())
        });
        def.method("transform", 6, |s, c, a| {
            a.require(6, "CanvasRenderingContext2D.transform")?;
            if let Some(v) = finite_args(&a, 6)? {
                let mut state = c.state.borrow_mut();
                state.transform *= Affine::new([v[0], v[1], v[2], v[3], v[4], v[5]]);
            }
            Ok(s.undefined())
        });
        def.method("translate", 2, |s, c, a| {
            a.require(2, "CanvasRenderingContext2D.translate")?;
            if let Some(v) = finite_args(&a, 2)? {
                let mut state = c.state.borrow_mut();
                state.transform *= Affine::translate((v[0], v[1]));
            }
            Ok(s.undefined())
        });
        def.method("rotate", 1, |s, c, a| {
            a.require(1, "CanvasRenderingContext2D.rotate")?;
            if let Some(v) = finite_args(&a, 1)? {
                let mut state = c.state.borrow_mut();
                state.transform *= Affine::rotate(v[0]);
            }
            Ok(s.undefined())
        });
        def.method("scale", 2, |s, c, a| {
            a.require(2, "CanvasRenderingContext2D.scale")?;
            if let Some(v) = finite_args(&a, 2)? {
                let mut state = c.state.borrow_mut();
                state.transform *= Affine::scale_non_uniform(v[0], v[1]);
            }
            Ok(s.undefined())
        });

        // The current path. Non-finite arguments add nothing.
        def.method("beginPath", 0, |s, c, _a| {
            *c.path.borrow_mut() = Path::default();
            Ok(s.undefined())
        });
        def.method("closePath", 0, |s, c, _a| {
            c.path.borrow_mut().close();
            Ok(s.undefined())
        });
        macro_rules! path_method {
            ($name:literal, $n:literal, |$b:ident, $v:ident| $body:expr) => {
                def.method($name, $n, |s, c, a| {
                    a.require($n, concat!("CanvasRenderingContext2D.", $name))?;
                    if let Some($v) = finite_args(&a, $n)? {
                        let t = c.state.borrow().transform;
                        let mut path = c.path.borrow_mut();
                        let mut $b = Builder { path: &mut path, t };
                        $body;
                    }
                    Ok(s.undefined())
                });
            };
        }
        path_method!("moveTo", 2, |b, v| b.move_to(v[0], v[1]));
        path_method!("lineTo", 2, |b, v| b.line_to(v[0], v[1]));
        path_method!("quadraticCurveTo", 4, |b, v| b.quad_to(v[0], v[1], v[2], v[3]));
        path_method!("bezierCurveTo", 6, |b, v| b.cubic_to((v[0], v[1]), (v[2], v[3]), v[4], v[5]));
        path_method!("rect", 4, |b, v| b.rect(v[0], v[1], v[2], v[3]));
        def.method("arcTo", 5, |s, c, a| {
            a.require(5, "CanvasRenderingContext2D.arcTo")?;
            let Some(v) = finite_args(&a, 5)? else { return Ok(s.undefined()) };
            if v[4] < 0.0 {
                return Err(JsError::dom("IndexSizeError", format!("The radius provided ({}) is negative.", v[4])));
            }
            let t = c.state.borrow().transform;
            Builder { path: &mut c.path.borrow_mut(), t }.arc_to(v[0], v[1], v[2], v[3], v[4]);
            Ok(s.undefined())
        });
        def.method("arc", 5, |s, c, a| {
            a.require(5, "CanvasRenderingContext2D.arc")?;
            let Some(v) = finite_args(&a, 5)? else { return Ok(s.undefined()) };
            if v[2] < 0.0 {
                return Err(JsError::dom("IndexSizeError", format!("The radius provided ({}) is negative.", v[2])));
            }
            let ccw = a.get(5).to_bool();
            let t = c.state.borrow().transform;
            Builder { path: &mut c.path.borrow_mut(), t }.ellipse(v[0], v[1], v[2], v[2], 0.0, v[3], v[4], ccw);
            Ok(s.undefined())
        });
        def.method("ellipse", 7, |s, c, a| {
            a.require(7, "CanvasRenderingContext2D.ellipse")?;
            let Some(v) = finite_args(&a, 7)? else { return Ok(s.undefined()) };
            if v[2] < 0.0 || v[3] < 0.0 {
                let which = if v[2] < 0.0 { "major" } else { "minor" };
                let r = if v[2] < 0.0 { v[2] } else { v[3] };
                return Err(JsError::dom(
                    "IndexSizeError",
                    format!("The {which}-axis radius provided ({r}) is negative."),
                ));
            }
            let ccw = a.get(7).to_bool();
            let t = c.state.borrow().transform;
            Builder { path: &mut c.path.borrow_mut(), t }.ellipse(v[0], v[1], v[2], v[3], v[4], v[5], v[6], ccw);
            Ok(s.undefined())
        });
        def.method("roundRect", 4, |s, c, a| {
            a.require(4, "CanvasRenderingContext2D.roundRect")?;
            let radii = if a.get(4).is_undefined() { s.number(0.0) } else { a.get(4) };
            let radii = module_helper(s, c, "roundRectRadii", &[radii])?;
            let (Some(v), Some(radii)) = (finite_args(&a, 4)?, radii.as_object()) else { return Ok(s.undefined()) };
            let mut r = [0.0; 8];
            for (i, slot) in r.iter_mut().enumerate() {
                *slot = radii.get_index(i as u32)?.to_number()?;
            }
            let t = c.state.borrow().transform;
            Builder { path: &mut c.path.borrow_mut(), t }.round_rect(v[0], v[1], v[2], v[3], r);
            Ok(s.undefined())
        });
        def.pure_method("isPointInPath", 2, |s, c, a| is_point_in(s, c, a, false));
        def.pure_method("isPointInStroke", 2, |s, c, a| is_point_in(s, c, a, true));

        // Drawing.
        def.method("fill", 0, |s, c, a| {
            let (path, next) = path_arg(s, c, &a, "CanvasRenderingContext2D.fill")?;
            let evenodd = fill_rule(&a, next, "CanvasRenderingContext2D.fill")?;
            let path = closed(&path);
            with_paint(s, c, true, |surface, paint, state| surface.fill(&path, evenodd, paint, &state.draw_state()))?;
            Ok(s.undefined())
        });
        def.method("stroke", 0, |s, c, a| {
            let (path, _) = path_arg(s, c, &a, "CanvasRenderingContext2D.stroke")?;
            with_paint(s, c, false, |surface, paint, state| {
                surface.stroke(&path, &state.stroke(), paint, &state.draw_state())
            })?;
            Ok(s.undefined())
        });
        def.method("clip", 0, |s, c, a| {
            let (path, next) = path_arg(s, c, &a, "CanvasRenderingContext2D.clip")?;
            let evenodd = fill_rule(&a, next, "CanvasRenderingContext2D.clip")?;
            let clip = ClipPath { path: closed(&path), evenodd };
            Rc::make_mut(&mut c.state.borrow_mut().clip).push(clip);
            Ok(s.undefined())
        });
        def.method("fillRect", 4, |s, c, a| {
            a.require(4, "CanvasRenderingContext2D.fillRect")?;
            let Some(v) = finite_args(&a, 4)? else { return Ok(s.undefined()) };
            if v[2] == 0.0 || v[3] == 0.0 {
                return Ok(s.undefined());
            }
            let t = c.state.borrow().transform;
            let mut path = Path::default();
            Builder { path: &mut path, t }.rect(v[0], v[1], v[2], v[3]);
            with_paint(s, c, true, |surface, paint, state| surface.fill(&path.bez, false, paint, &state.draw_state()))?;
            Ok(s.undefined())
        });
        def.method("strokeRect", 4, |s, c, a| {
            a.require(4, "CanvasRenderingContext2D.strokeRect")?;
            let Some(v) = finite_args(&a, 4)? else { return Ok(s.undefined()) };
            let t = c.state.borrow().transform;
            let mut path = Path::default();
            let mut b = Builder { path: &mut path, t };
            if v[2] == 0.0 && v[3] == 0.0 {
                return Ok(s.undefined());
            } else if v[2] == 0.0 || v[3] == 0.0 {
                // pyro: a closed subpath of the two corners, as Chromium strokes it (WPT
                // 2d.strokeRect.zero.4/5): joins at both ends, no caps.
                b.move_to(v[0], v[1]);
                b.line_to(v[0] + v[2], v[1] + v[3]);
                b.path.close();
            } else {
                b.rect(v[0], v[1], v[2], v[3]);
            }
            with_paint(s, c, false, |surface, paint, state| {
                surface.stroke(&path.bez, &state.stroke(), paint, &state.draw_state())
            })?;
            Ok(s.undefined())
        });
        def.method("clearRect", 4, |s, c, a| {
            a.require(4, "CanvasRenderingContext2D.clearRect")?;
            if let Some(v) = finite_args(&a, 4)? {
                c.sync_size();
                let state = c.state.borrow();
                let rect = Rect::new(v[0], v[1], v[0] + v[2], v[1] + v[3]);
                c.surface.borrow_mut().clear_rect(rect, state.transform, &state.clip);
            }
            Ok(s.undefined())
        });
        def.method("fillText", 3, |s, c, a| {
            draw_text(s, c, a, true)?;
            Ok(s.undefined())
        });
        def.method("strokeText", 3, |s, c, a| {
            draw_text(s, c, a, false)?;
            Ok(s.undefined())
        });
        def.method("drawImage", 3, draw_image);
        def.method("drawElementImage", 3, draw_element_image);
        // Focus rings are not drawn.
        def.method("setLineDash", 1, |s, c, a| {
            a.require(1, "CanvasRenderingContext2D.setLineDash")?;
            let list = a.get(0).as_object().ok_or_else(|| {
                JsError::Type(
                    "Failed to execute 'setLineDash' on 'CanvasRenderingContext2D': The provided value cannot be converted to a sequence."
                        .into(),
                )
            })?;
            let len = list.get_atom("length")?.to_number()? as u32;
            let mut dash = Vec::with_capacity(len.min(1024) as usize);
            for i in 0..len {
                let v = list.get_index(i)?.to_number()?;
                if !v.is_finite() || v < 0.0 {
                    return Ok(s.undefined());
                }
                dash.push(v);
            }
            if dash.len() % 2 == 1 {
                dash.extend_from_within(..);
            }
            c.state.borrow_mut().line_dash = dash;
            Ok(s.undefined())
        });
        def.pure_method("getLineDash", 0, |s, c, _a| {
            let values: Vec<Value<'_>> = c.state.borrow().line_dash.iter().map(|v| s.number(*v)).collect();
            Ok(s.array(&values)?.value())
        });

        // Styles.
        def.method("createLinearGradient", 4, |s, c, a| {
            let v = doubles(&a, 4, "CanvasRenderingContext2D.createLinearGradient")?;
            gradient(s, c, "linear", &v)
        });
        def.method("createRadialGradient", 6, |s, c, a| {
            let v = doubles(&a, 6, "CanvasRenderingContext2D.createRadialGradient")?;
            if v[2] < 0.0 || v[5] < 0.0 {
                let r = if v[2] < 0.0 { v[2] } else { v[5] };
                return Err(JsError::dom("IndexSizeError", format!("The radius provided ({r}) is negative.")));
            }
            gradient(s, c, "radial", &v)
        });
        def.method("createConicGradient", 3, |s, c, a| {
            let v = doubles(&a, 3, "CanvasRenderingContext2D.createConicGradient")?;
            gradient(s, c, "conic", &v)
        });
        def.method("createPattern", 2, |s, c, a| {
            let what = "CanvasRenderingContext2D.createPattern";
            a.require(2, what)?;
            let source = image_source(s, &c.page, a.get(0), what, false, None)?;
            let repetition = if a.get(1).is_null() { String::new() } else { dom_string(a.get(1))? };
            let repetition = match repetition.as_str() {
                "" | "repeat" => "repeat",
                "repeat-x" => "repeat-x",
                "repeat-y" => "repeat-y",
                "no-repeat" => "no-repeat",
                other => {
                    return Err(JsError::dom(
                        "SyntaxError",
                        format!(
                        "The provided type ('{other}') is not one of 'repeat', 'no-repeat', 'repeat-x', or 'repeat-y'."
                    ),
                    ))
                }
            };
            let Some(source) = source else { return Ok(s.null()) };
            let handle = handle_object(s, handle_in(source, c.space))?;
            module_helper(s, c, "pattern", &[s.string(repetition), handle.value()])
        });

        // Pixels.
        def.method("createImageData", 1, |s, c, a| {
            a.require(1, "CanvasRenderingContext2D.createImageData")?;
            if a.get(0).is_object() {
                let size = module_helper(s, c, "imageSize", &[a.get(0)])?;
                let Some(size) = size.as_object() else {
                    return Err(JsError::Type(
                        "Failed to execute 'createImageData' on 'CanvasRenderingContext2D': parameter 1 is not of type 'ImageData'."
                            .into(),
                    ));
                };
                let settings = s.object();
                settings.set_atom("colorSpace", size.get_index(2)?)?;
                settings.set_atom("pixelFormat", size.get_index(3)?)?;
                return module_helper(s, c, "imageData", &[size.get_index(0)?, size.get_index(1)?, settings.value()]);
            }
            a.require(2, "CanvasRenderingContext2D.createImageData")?;
            let (w, h) = (long(a.get(0))?, long(a.get(1))?);
            if w == 0 || h == 0 {
                return Err(JsError::dom(
                    "IndexSizeError",
                    format!("The source {} is 0.", if w == 0 { "width" } else { "height" }),
                ));
            }
            // ImageDataSettings with the context's color space when it names none.
            module_helper(
                s,
                c,
                "imageDataIn",
                &[s.number(w.abs() as f64), s.number(h.abs() as f64), a.get(2), s.string(c.space.name())],
            )
        });
        def.method("getImageData", 4, get_image_data);
        def.method("putImageData", 3, put_image_data);
    }
}

fn gradient<'s>(s: &Scope<'s>, c: &ContextData, kind: &str, args: &[f64]) -> JsResult<Value<'s>> {
    let values: Vec<Value<'_>> = args.iter().map(|v| s.number(*v)).collect();
    let array = s.array(&values)?;
    module_helper(s, c, "gradient", &[s.string(kind), array.value()])
}

pub(crate) fn register(engine: &pyro_js::Engine) {
    engine.register::<CanvasRenderingContext2D>();
    register_worker(engine);
}

pub(crate) fn register_worker(engine: &pyro_js::Engine) {
    engine.register::<OffscreenCanvasRenderingContext2D>();
}

// ---- Transfer and placeholders ----------------------------------------------

/// The last frame an `OffscreenCanvas` committed to its placeholder
/// `<canvas>` (HTML "update the rendering" of the offscreen canvas). Shared
/// between the thread drawing (a worker's, or the page's own) and the
/// page's, which paints it.
#[derive(Default)]
pub struct Placeholder {
    frame: Mutex<PlaceholderFrame>,
}

/// A committed frame, premultiplied; its straight-alpha rows are made once
/// per frame, when the page is painted.
#[derive(Clone, Default)]
pub(crate) struct PlaceholderFrame {
    bitmap: Option<Bitmap>,
    width: u32,
    height: u32,
    clean: bool,
    straight: Option<Arc<Vec<u8>>>,
}

impl PlaceholderFrame {
    /// A surface holding the frame (to encode it).
    fn surface(&self) -> Surface {
        let mut surface = Surface::new(self.width, self.height);
        if let Some(bitmap) = &self.bitmap {
            let rect = Rect::new(0.0, 0.0, f64::from(self.width), f64::from(self.height));
            surface.draw_image(bitmap, rect, rect, &State::new().draw_state());
        }
        surface
    }
}

impl Placeholder {
    fn frame(&self) -> std::sync::MutexGuard<'_, PlaceholderFrame> {
        self.frame.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn commit(&self, bitmap: Option<Bitmap>, width: u32, height: u32, clean: bool) {
        *self.frame() = PlaceholderFrame { bitmap, width, height, clean, straight: None };
    }

    /// The frame's straight RGBA rows, `None` while it has no pixels.
    fn rgba(&self) -> Option<(u32, u32, Arc<Vec<u8>>)> {
        let mut frame = self.frame();
        let bitmap = frame.bitmap.as_ref()?;
        if frame.straight.is_none() {
            let mut data = bitmap.data_as_u8_slice().to_vec();
            pyro_canvas::unpremultiply(&mut data);
            frame.straight = Some(Arc::new(data));
        }
        Some((frame.width, frame.height, frame.straight.clone()?))
    }
}

/// An offscreen context of this thread whose frames go to a placeholder.
pub(crate) struct FrameCommit {
    surface: Weak<RefCell<Surface>>,
    clean: Rc<Cell<bool>>,
    placeholder: Arc<Placeholder>,
    /// The surface's generation and size last committed.
    committed: Cell<Option<(u64, u32, u32, bool)>>,
}

/// After a task: each offscreen context drawing for a placeholder whose
/// bitmap changed commits it (the pixels are shared, not copied). Free on a
/// thread without one.
pub(crate) fn commit_frames(page: &PageDom) {
    let Ok(commits) = page.canvas_commits.try_borrow() else { return };
    if commits.is_empty() {
        return;
    }
    let mut gone = false;
    for commit in commits.iter() {
        let Some(surface) = commit.surface.upgrade() else {
            gone = true;
            continue;
        };
        let Ok(mut surface) = surface.try_borrow_mut() else { continue };
        surface.flush();
        let now = (surface.generation(), surface.width(), surface.height(), commit.clean.get());
        if commit.committed.get() != Some(now) {
            commit.committed.set(Some(now));
            commit.placeholder.commit(surface.snapshot(), now.1, now.2, now.3);
        }
    }
    drop(commits);
    if gone {
        page.canvas_commits.borrow_mut().retain(|c| c.surface.strong_count() > 0);
    }
}

/// The frame shown on `canvas` if its control was transferred.
fn placeholder_frame(page: &PageDom, canvas: NodeId) -> Option<PlaceholderFrame> {
    let list = page.canvas_placeholders.borrow();
    if list.is_empty() {
        return None;
    }
    list.iter().find(|(id, _)| *id == canvas).map(|(_, p)| p.frame().clone())
}

thread_local! {
    static PLACEHOLDER_HANDED: RefCell<Option<Arc<Placeholder>>> = const { RefCell::new(None) };
}

/// A JS value holding `placeholder` (canvas2d.js keeps it private on the
/// OffscreenCanvas), as [`handle_object`] holds pixels.
fn placeholder_object<'s>(s: &Scope<'s>, placeholder: Arc<Placeholder>) -> JsResult<Value<'s>> {
    Ok(s.function("", move |s, _this, _a| {
        PLACEHOLDER_HANDED.with(|h| *h.borrow_mut() = Some(placeholder.clone()));
        Ok(s.undefined())
    })?
    .value())
}

fn placeholder_of(v: Value<'_>) -> JsResult<Option<Arc<Placeholder>>> {
    let Some(f) = v.as_object().filter(|_| v.is_function()) else { return Ok(None) };
    PLACEHOLDER_HANDED.with(|h| h.borrow_mut().take());
    f.call(None, &[])?;
    Ok(PLACEHOLDER_HANDED.with(|h| h.borrow_mut().take()))
}

/// `HTMLCanvasElement.transferControlToOffscreen()`: an OffscreenCanvas of
/// the canvas's size whose committed frames the canvas shows.
pub(crate) fn transfer_control_to_offscreen<'s>(
    s: &Scope<'s>,
    page: &Rc<PageDom>,
    canvas: NodeId,
) -> JsResult<Value<'s>> {
    if element_context(s, page, canvas)?.is_some() {
        return Err(JsError::dom(
            "InvalidStateError",
            "Failed to execute 'transferControlToOffscreen' on 'HTMLCanvasElement': Cannot transfer control from a canvas that has a rendering context.",
        ));
    }
    if placeholder_frame(page, canvas).is_some() {
        return Err(JsError::dom(
            "InvalidStateError",
            "Failed to execute 'transferControlToOffscreen' on 'HTMLCanvasElement': Cannot transfer control from a canvas for more than one time.",
        ));
    }
    let (w, h) = canvas_size(page, canvas);
    let placeholder = Arc::new(Placeholder { frame: Mutex::new(PlaceholderFrame { width: w, height: h, clean: true, ..Default::default() }) });
    page.canvas_placeholders.borrow_mut().push((canvas, placeholder.clone()));
    page.canvas_used.set(true);
    let handle = placeholder_object(s, placeholder)?;
    MODULE.helper(s, page, "offscreenOf", &[s.number(f64::from(w)), s.number(f64::from(h)), handle])
}

/// An `ImageBitmap` or `OffscreenCanvas` on its way to another realm or
/// thread (StructuredSerialize, or its transfer).
#[derive(Clone)]
pub(crate) enum CanvasObject {
    Bitmap { pixmap: Option<Bitmap>, width: u32, height: u32 },
    Offscreen { width: f64, height: f64, placeholder: Option<Arc<Placeholder>> },
}

/// The [`CanvasObject`] of `value` if it is an ImageBitmap (or, being
/// transferred, an OffscreenCanvas) of `s`'s realm. DataCloneError for a
/// detached one or a bitmap that is not origin-clean; InvalidStateError for
/// an OffscreenCanvas with a context. A realm that never made a canvas
/// value costs one lookup.
pub(crate) fn canvas_object_of(s: &Scope<'_>, value: Value<'_>, transfer: bool) -> JsResult<Option<CanvasObject>> {
    if !value.is_object() {
        return Ok(None);
    }
    let page = crate::target::page_of(s)?;
    let Some(module) = MODULE.existing(s, &page)? else { return Ok(None) };
    let describe = module.get("describeTransferable")?;
    let describe = describe.as_object().ok_or(JsError::ContextDestroyed)?;
    let parts = describe.call(None, &[value, s.boolean(transfer)])?;
    let Some(parts) = parts.as_object().filter(|_| parts.is_array()) else { return Ok(None) };
    if parts.get_index(0)?.to_number()? == 0.0 {
        let handle =
            handle_of(parts.get_index(1)?)?.ok_or_else(|| JsError::dom("DataCloneError", "Invalid ImageBitmap"))?;
        if !handle.clean {
            return Err(JsError::dom("DataCloneError", "An ImageBitmap that is not origin-clean could not be cloned."));
        }
        return Ok(Some(CanvasObject::Bitmap { pixmap: handle.pixmap, width: handle.width, height: handle.height }));
    }
    Ok(Some(CanvasObject::Offscreen {
        width: parts.get_index(1)?.to_number()?,
        height: parts.get_index(2)?.to_number()?,
        placeholder: placeholder_of(parts.get_index(3)?)?,
    }))
}

/// Detaches a transferred ImageBitmap or OffscreenCanvas of `s`'s realm;
/// false when `value` is neither.
pub(crate) fn detach_canvas_object(s: &Scope<'_>, value: Value<'_>) -> JsResult<bool> {
    let page = crate::target::page_of(s)?;
    let Some(module) = MODULE.existing(s, &page)? else { return Ok(false) };
    let detach = module.get("detachTransferable")?;
    let detach = detach.as_object().ok_or(JsError::ContextDestroyed)?;
    Ok(detach.call(None, &[value])?.to_bool())
}

/// The ImageBitmap or OffscreenCanvas of `object` in `s`'s realm.
pub(crate) fn make_canvas_object<'s>(s: &Scope<'s>, object: &CanvasObject) -> JsResult<Value<'s>> {
    let page = crate::target::page_of(s)?;
    page.canvas_used.set(true);
    match object {
        CanvasObject::Bitmap { pixmap, width, height } => {
            let handle =
                ImageHandle { pixmap: pixmap.clone(), width: *width, height: *height, clean: true, space: Space::Srgb };
            let made = bitmap_triple(s, handle)?;
            MODULE.helper(s, &page, "bitmapOf", &[made])
        }
        CanvasObject::Offscreen { width, height, placeholder } => {
            let handle = match placeholder {
                Some(p) => placeholder_object(s, p.clone())?,
                None => s.null(),
            };
            MODULE.helper(s, &page, "offscreenOf", &[s.number(*width), s.number(*height), handle])
        }
    }
}

/// A canvas bitmap for rendering: its element, size and straight RGBA rows.
pub type CanvasBitmap = (NodeId, u32, u32, Arc<Vec<u8>>);

impl PageDom {
    pub(crate) fn register_canvas(&self, canvas: NodeId, surface: Weak<RefCell<Surface>>) {
        let mut list = self.canvas_surfaces.borrow_mut();
        list.retain(|(_, s)| s.strong_count() > 0);
        list.push((canvas, surface));
    }

    /// The bitmaps of this document's canvases that were drawn on, for
    /// painting them (a screenshot). Empty, at the cost of a borrow, on a
    /// page that never made a 2D context.
    pub fn canvas_bitmaps(&self) -> Vec<CanvasBitmap> {
        let list = self.canvas_surfaces.borrow();
        let mut out = Vec::new();
        for (canvas, surface) in list.iter() {
            let Some(surface) = surface.upgrade() else { continue };
            let mut surface = surface.borrow_mut();
            let (w, h) = canvas_size(self, *canvas);
            if (surface.width(), surface.height()) != (w, h) || !surface.is_allocated() {
                continue;
            }
            if let Some(rgba) = surface.straight_rgba() {
                out.push((*canvas, w, h, rgba));
            }
        }
        drop(list);
        // Canvases whose control went offscreen show the committed frame.
        for (canvas, placeholder) in self.canvas_placeholders.borrow().iter() {
            if let Some((w, h, rgba)) = placeholder.rgba() {
                out.push((*canvas, w, h, rgba));
            }
        }
        out
    }
}

/// This module's part of [`crate::thread_state::reset_thread`]: what of its
/// per-thread state still holds something, if any does.
pub(crate) fn thread_leftover() -> Option<&'static str> {
    if HANDED.with(|h| h.borrow().is_some()) {
        return Some("canvas::HANDED");
    }
    PLACEHOLDER_HANDED.with(|h| h.borrow().is_some()).then_some("canvas::PLACEHOLDER_HANDED")
}
