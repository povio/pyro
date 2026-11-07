//! `CanvasRenderingContext2D`, `OffscreenCanvas` 2D contexts, `ImageBitmap`
//! and canvas pixels, rasterized by lp-canvas (vello_cpu).
//!
//! - The drawing state (styles, line and shadow settings, text settings,
//!   the current transform, the clipping region) is kept, saved and
//!   restored like a browser's; `font` parses and serializes the CSS
//!   shorthand, defaulting to `10px sans-serif`.
//! - The current path is built with the transform applied (kurbo geometry,
//!   `canvas_path`), so `isPointInPath`/`isPointInStroke` answer for it and
//!   for a `Path2D`, and the rasterizer gets device-space paths.
//! - Each context owns an `lp_canvas::Surface`: its bitmap is allocated on
//!   the first draw or pixel write (reading a canvas nothing was drawn on
//!   allocates nothing) and released when the canvas is resized.
//! - Text is shaped with Parley on the system fonts (`lp_canvas::text`), for
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
//! The value types (`CanvasGradient`, `CanvasPattern`, `Path2D`, `ImageData`,
//! `TextMetrics`, `ImageBitmap`, `OffscreenCanvas`, `DOMMatrix`, `DOMPoint`)
//! and `createImageBitmap` are made in JS (canvas2d.js) on a realm's first
//! use of any of them.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::Arc;

use kurbo::{Affine, BezPath, Point, Rect};
use lp_canvas::{Bitmap, ClipPath, Color, Composite, DrawState, Paint, Pixmap, Surface};
use lp_dom::text_metrics::{self, FontSpec};
use lp_dom::NodeId;
use lp_js::sys;
use lp_js::{Args, ClassDef, JsClass, JsError, JsResult, NoParent, Object, Persistent, Scope, Value};

use crate::canvas_path::{closed, on_outline, point_in_fill, replay, stroke_outline, Builder, Path};
use crate::lazy_module::{lazy_globals, LazyModule};
use crate::node::node_value;
use crate::page::PageDom;
use crate::util::*;
use crate::window::WindowHost;

const CONTEXT_2D: &str = "__lpCanvas2d";
/// The OffscreenCanvas of an offscreen context (hidden on its wrapper).
const OFFSCREEN: &str = "__lpOffscreen";

static MODULE: LazyModule = LazyModule {
    key: "lpCanvas2d",
    source: include_str!("canvas2d.js"),
    url: "lightningpanda://internal/canvas2d.js",
    natives: Some(natives),
};

/// Adds the lazy interface names to lp-browser's `Window` class.
pub(crate) fn define_globals<W>(def: &mut ClassDef<W>)
where
    W: JsClass,
    W::Data: WindowHost,
{
    lazy_globals!(
        def,
        MODULE,
        "DOMPointReadOnly",
        "DOMPoint",
        "DOMMatrixReadOnly",
        "DOMMatrix",
        "WebKitCSSMatrix",
        "Path2D",
        "ImageData",
        "CanvasGradient",
        "CanvasPattern",
        "TextMetrics",
        "ImageBitmap",
        "OffscreenCanvas",
        "OffscreenCanvasRenderingContext2D",
        "createImageBitmap",
    );
}

fn natives<'s>(s: &Scope<'s>) -> JsResult<Object<'s>> {
    let natives = s.object();
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
        let context = new_context(s, &page, Target::Offscreen, size_u32(w), size_u32(h))?;
        s.set_hidden(context, OFFSCREEN, a.get(0))?;
        page.canvas_used.set(true);
        Ok(context.value())
    })?;
    natives.set_atom("offscreenContext", offscreen.value())?;
    let resize = s.function("resizeContext", |s, _this, a| {
        if let Some(c) = s.unwrap::<CanvasRenderingContext2D>(a.get(0)) {
            let (w, h) = (size_u32(a.get(1).to_number()?), size_u32(a.get(2).to_number()?));
            c.reset();
            c.surface.borrow_mut().resize(w, h);
        }
        Ok(s.undefined())
    })?;
    natives.set_atom("resizeContext", resize.value())?;
    let transfer = s.function("transferContext", |s, _this, a| {
        let c = s.unwrap::<CanvasRenderingContext2D>(a.get(0)).ok_or(JsError::ContextDestroyed)?;
        let handle = {
            let mut surface = c.surface.borrow_mut();
            let handle = ImageHandle {
                pixmap: surface.snapshot(),
                width: surface.width(),
                height: surface.height(),
                clean: c.clean.get(),
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
        let (encoding, mime) = lp_canvas::codec::encoding_for(&mime, quality);
        let bytes = match s.unwrap::<CanvasRenderingContext2D>(a.get(0)) {
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
    Ok(natives)
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

/// A CSS color as canvas serializes it: `#rrggbb` when opaque, else
/// `rgba(r, g, b, a)`. `currentcolor` is black (no element style applies).
pub(crate) fn canvas_color(value: &str) -> Option<String> {
    let css = lp_dom::computed::serialize_color(value, &mut || Some("rgb(0, 0, 0)".into()))?;
    let Some(inner) = css.strip_prefix("rgb(").and_then(|r| r.strip_suffix(')')) else { return Some(css) };
    let mut hex = String::from("#");
    for part in inner.split(',') {
        let v: u8 = part.trim().parse().ok()?;
        hex.push_str(&format!("{v:02x}"));
    }
    Some(hex)
}

/// The color of a [`canvas_color`] serialization.
fn color_of(serialized: &str) -> Color {
    if let Some(hex) = serialized.strip_prefix('#') {
        let v = u32::from_str_radix(hex, 16).unwrap_or(0);
        return Color::from_rgba8((v >> 16) as u8, (v >> 8) as u8, v as u8, 255);
    }
    let inner = serialized
        .strip_prefix("rgba(")
        .or_else(|| serialized.strip_prefix("rgb("))
        .and_then(|r| r.strip_suffix(')'))
        .unwrap_or("");
    let mut parts = inner.split(',').map(|p| p.trim().parse::<f32>().unwrap_or(0.0));
    let (r, g, b) = (parts.next().unwrap_or(0.0), parts.next().unwrap_or(0.0), parts.next().unwrap_or(0.0));
    let a = parts.next().unwrap_or(1.0);
    Color::new([r / 255.0, g / 255.0, b / 255.0, a.clamp(0.0, 1.0)])
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
    shadow_offset_x: f64,
    shadow_offset_y: f64,
    image_smoothing: bool,
    image_smoothing_quality: &'static str,
    filter: String,
    letter_spacing: String,
    word_spacing: String,
    font_kerning: &'static str,
    font_stretch: &'static str,
    font_variant_caps: &'static str,
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
            shadow_offset_x: 0.0,
            shadow_offset_y: 0.0,
            image_smoothing: true,
            image_smoothing_quality: "low",
            filter: "none".into(),
            letter_spacing: "0px".into(),
            word_spacing: "0px".into(),
            font_kerning: "auto",
            font_stretch: "normal",
            font_variant_caps: "normal",
            text_rendering: "auto",
            transform: Affine::IDENTITY,
            clip: Rc::new(Vec::new()),
        }
    }

    fn draw_state(&self) -> DrawState<'_> {
        let shadow_color = color_of(&self.shadow_color);
        DrawState {
            transform: self.transform,
            alpha: self.global_alpha as f32,
            composite: Composite::from_name(self.composite).unwrap_or(Composite::SourceOver),
            clip: &self.clip,
            shadow: (shadow_color.components[3] > 0.0).then(|| lp_canvas::Shadow {
                color: shadow_color,
                blur: self.shadow_blur,
                offset: kurbo::Vec2::new(self.shadow_offset_x, self.shadow_offset_y),
            }),
            smoothing: self.image_smoothing,
        }
    }

    fn stroke(&self) -> kurbo::Stroke {
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
    clean: Cell<bool>,
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
        }
    }
}

fn default_font() -> FontSpec {
    text_metrics::parse_font("10px sans-serif", 10.0).expect("default canvas font")
}

/// A canvas element's bitmap size: its `width`/`height` attributes.
fn canvas_size(page: &PageDom, canvas: NodeId) -> (u32, u32) {
    let dom = page.dom.borrow();
    let attr = |name: &str, default: u32| {
        dom.get_attribute(canvas, name).ok().flatten().and_then(|v| v.trim().parse::<u32>().ok()).unwrap_or(default)
    };
    (attr("width", 300), attr("height", 150))
}

fn new_context<'s>(s: &Scope<'s>, page: &Rc<PageDom>, target: Target, w: u32, h: u32) -> JsResult<Object<'s>> {
    let data = ContextData {
        page: page.clone(),
        target,
        state: RefCell::new(State::new()),
        stack: RefCell::new(Vec::new()),
        path: RefCell::new(Path::default()),
        surface: Rc::new(RefCell::new(Surface::new(w, h))),
        clean: Cell::new(true),
    };
    if let Target::Element(canvas) = target {
        page.register_canvas(canvas, Rc::downgrade(&data.surface));
    }
    s.wrap::<CanvasRenderingContext2D>(data, None)
}

/// `HTMLCanvasElement.getContext(type)`: one 2D context per canvas; other
/// context types are unsupported (null).
pub fn get_context<'s>(s: &Scope<'s>, page: &Rc<PageDom>, canvas: NodeId, kind: &str) -> JsResult<Value<'s>> {
    if kind != "2d" {
        return Ok(s.null());
    }
    let element = node_value(s, page, Some(canvas))?;
    let element = element.as_object().ok_or(JsError::ContextDestroyed)?;
    if let Some(existing) = s.get_hidden(element, CONTEXT_2D)? {
        return Ok(existing);
    }
    let (w, h) = canvas_size(page, canvas);
    let context = new_context(s, page, Target::Element(canvas), w, h)?;
    page.canvas_used.set(true);
    s.set_hidden(element, CONTEXT_2D, context.value())?;
    Ok(context.value())
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
    let (encoding, mime) = lp_canvas::codec::encoding_for(mime, quality);
    let bytes = match element_context(s, page, canvas)?.and_then(|v| s.unwrap::<CanvasRenderingContext2D>(v)) {
        Some(c) => {
            if !c.clean.get() {
                return Err(security_error());
            }
            c.sync_size();
            c.surface.borrow_mut().encode(encoding)
        }
        None => {
            let (w, h) = canvas_size(page, canvas);
            Surface::new(w, h).encode(encoding)
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
                crate::lifecycle::report_diagnostic(&s, &e);
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

/// An image argument's pixels: `Ok(None)` when it has none to give yet
/// (an image still loading, a video), errors as HTML specifies.
fn image_source(
    s: &Scope<'_>,
    page: &Rc<PageDom>,
    value: Value<'_>,
    what: &str,
    image_data: bool,
) -> JsResult<Option<ImageHandle>> {
    let not_source = || {
        JsError::Type(format!(
            "Failed to execute '{what}': The provided value is not of type '(CSSImageValue or HTMLCanvasElement or HTMLImageElement or HTMLVideoElement or ImageBitmap or OffscreenCanvas or SVGImageElement or VideoFrame)'."
        ))
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
                if source_page.dom.borrow().natural_size(n.id).is_none_or(|(w, h)| w == 0.0 || h == 0.0) {
                    return Err(JsError::dom(
                        "InvalidStateError",
                        "The HTMLImageElement provided is in the 'broken' state.",
                    ));
                }
                Ok(source_page.image_pixmap(n.id).map(|(pixmap, clean)| ImageHandle {
                    width: u32::from(pixmap.width()),
                    height: u32::from(pixmap.height()),
                    pixmap: Some(pixmap),
                    clean,
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
                        ImageHandle { pixmap, width: w, height: h, clean: c.clean.get() }
                    }
                    None => ImageHandle { pixmap: None, width: w, height: h, clean: true },
                };
                Ok(Some(handle))
            }
            // The frame shown now (lp-media); nothing before HAVE_CURRENT_DATA
            // or without a decoder for the video.
            "video" => Ok(crate::media::current_frame(&source_page, n.id).and_then(|(frame, clean)| {
                let pixmap = lp_canvas::pixmap_from_straight((*frame.rgba).clone(), frame.width, frame.height)?;
                Some(ImageHandle { pixmap: Bitmap::new(pixmap), width: frame.width, height: frame.height, clean })
            })),
            // No SVG images.
            "image" => Ok(None),
            _ => Err(not_source()),
        };
    }
    if !value.is_object() {
        return Err(not_source());
    }
    let source = MODULE.helper(s, page, "sourceOf", &[value, s.boolean(image_data)])?;
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
            Ok(Some(match s.unwrap::<CanvasRenderingContext2D>(desc.get_index(1)?) {
                Some(c) => {
                    let pixmap = c.surface.borrow_mut().snapshot();
                    ImageHandle { pixmap, width: w, height: h, clean: c.clean.get() }
                }
                None => ImageHandle { pixmap: None, width: w, height: h, clean: true },
            }))
        }
        2 => {
            let (w, h) = (size_u32(desc.get_index(2)?.to_number()?), size_u32(desc.get_index(3)?.to_number()?));
            let data = typed_bytes(s, desc.get_index(1)?)?;
            let pixmap = lp_canvas::pixmap_from_straight(data, w, h).and_then(Bitmap::new);
            Ok(Some(ImageHandle { pixmap, width: w, height: h, clean: true }))
        }
        3 => {
            let blob = s.unwrap::<crate::file::Blob>(desc.get_index(1)?).ok_or_else(not_source)?;
            let pixmap = lp_canvas::codec::decode(&blob.bytes)
                .and_then(Bitmap::new)
                .ok_or_else(|| JsError::dom("InvalidStateError", "The source image could not be decoded."))?;
            let (w, h) = (u32::from(pixmap.width()), u32::from(pixmap.height()));
            Ok(Some(ImageHandle { pixmap: Some(pixmap), width: w, height: h, clean: true }))
        }
        _ => Err(not_source()),
    }
}

/// `createImageBitmap` (canvas2d.js): `[handle, width, height]` of the
/// source's pixels, cropped, resized and flipped as asked.
fn make_bitmap<'s>(s: &Scope<'s>, a: Args<'s>) -> JsResult<Value<'s>> {
    let page = crate::target::page_of(s)?;
    page.canvas_used.set(true);
    let what = "Window.createImageBitmap";
    let Some(source) = image_source(s, &page, a.get(0), what, true)? else {
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
    if !lp_canvas::valid_size(w, h) {
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
    bitmap_triple(s, ImageHandle { pixmap, width: w, height: h, clean: source.clean })
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
        gradient_paint(record)?.map(|g| (Paint::Gradient(Arc::new(g)), true))
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

fn gradient_paint(record: Object<'_>) -> JsResult<Option<lp_canvas::Gradient>> {
    let kind = record.get_atom("kind")?.to_string()?;
    let Some(args) = record.get_atom("args")?.as_object() else { return Ok(None) };
    let v = numbers(args)?;
    let Some(stops) = record.get_atom("stops")?.as_object() else { return Ok(None) };
    let len = stops.get_atom("length")?.to_number()? as u32;
    let mut list = Vec::with_capacity(len as usize / 2);
    for i in (0..len).step_by(2) {
        let offset = stops.get_index(i)?.to_number()? as f32;
        let color = color_of(&stops.get_index(i + 1)?.to_string()?);
        list.push((offset, color));
    }
    let p = |i: usize| Point::new(v[i], v[i + 1]);
    let kind = match (kind.as_str(), v.len()) {
        ("linear", 4) => lp_canvas::GradientKind::Linear { start: p(0), end: p(2) },
        ("radial", 6) => {
            lp_canvas::GradientKind::Radial { start: p(0), start_radius: v[2], end: p(3), end_radius: v[5] }
        }
        ("conic", 3) => lp_canvas::GradientKind::Conic { center: p(1), angle: v[0] },
        _ => return Ok(None),
    };
    Ok(Some(lp_canvas::Gradient { kind, stops: list }))
}

fn pattern_paint(record: Object<'_>) -> JsResult<Option<(Paint, bool)>> {
    let Some(handle) = handle_of(record.get_atom("image")?)? else { return Ok(None) };
    let repetition = record.get_atom("repetition")?.to_string()?;
    let t = record.get_atom("transform")?.as_object().map(numbers).transpose()?.unwrap_or_default();
    let transform = if t.len() == 6 { Affine::new([t[0], t[1], t[2], t[3], t[4], t[5]]) } else { Affine::IDENTITY };
    // A transparent source paints nothing visible: a 1x1 transparent image
    // keeps the pattern's geometry rules simple.
    let Some(image) = handle.pixmap.or_else(|| Bitmap::new(Pixmap::new(1, 1))) else { return Ok(None) };
    let pattern = lp_canvas::Pattern {
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
    if !clean {
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
        ))),
    }
}

/// The path of `fill`/`stroke`/`clip` (a Path2D at `a[0]`, else the current
/// path) in device coordinates, and the index of the next argument.
fn path_arg(s: &Scope<'_>, c: &ContextData, a: &Args<'_>, what: &str) -> JsResult<(BezPath, usize)> {
    if a.get(0).is_object() {
        let Some(ops) = path2d_arg(s, c, a)? else {
            return Err(JsError::Type(format!("Failed to execute '{what}': parameter 1 is not of type 'Path2D'.")));
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
    let layout = layout_text(&text, &state);
    if layout.runs.is_empty() {
        return Ok(());
    }
    let scale = match max_width {
        Some(m) if layout.width > m => m / layout.width,
        _ => 1.0,
    };
    let width = layout.width * scale;
    let dx = match align_of(&state) {
        Align::Center => -width / 2.0,
        Align::Right => -width,
        Align::Left => 0.0,
    };
    let dy = baseline_shift(state.text_baseline, layout.ascent, layout.descent);
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

/// `textAlign` resolved for the direction (`inherit` is left-to-right).
fn align_of(state: &State) -> Align {
    let rtl = state.direction == "rtl";
    match state.text_align {
        "center" => Align::Center,
        "left" => Align::Left,
        "right" => Align::Right,
        "end" if !rtl => Align::Right,
        "start" if rtl => Align::Right,
        _ => Align::Left,
    }
}

fn spacing_px(value: &str, font_size: f64) -> f32 {
    let v = value.trim();
    let (number, unit) = v.find(|ch: char| ch.is_ascii_alphabetic()).map_or((v, ""), |i| v.split_at(i));
    let n: f64 = number.trim().parse().unwrap_or(0.0);
    (match unit {
        "em" => n * font_size,
        _ => n,
    }) as f32
}

fn layout_text(text: &str, state: &State) -> lp_canvas::text::TextLayout {
    let font = &state.font;
    let style = lp_canvas::text::TextStyle {
        family: &font.family,
        size: font.size as f32,
        weight: f32::from(font.weight),
        italic: font.style != "normal",
        letter_spacing: spacing_px(&state.letter_spacing, font.size),
        word_spacing: spacing_px(&state.word_spacing, font.size),
    };
    lp_canvas::text::layout(text, &style)
}

/// `drawImage` with 3, 5 or 9 arguments.
fn draw_image<'s>(s: &Scope<'s>, c: &ContextData, a: Args<'s>) -> JsResult<Value<'s>> {
    let what = "CanvasRenderingContext2D.drawImage";
    a.require(3, what)?;
    let n = a.len();
    if !matches!(n, 3 | 5) && n < 9 {
        return Err(JsError::Type(format!("Failed to execute 'drawImage' on 'CanvasRenderingContext2D': Valid arities are: [3, 5, 9], but {n} arguments provided.")));
    }
    let Some(source) = image_source(s, &c.page, a.get(0), what, false)? else { return Ok(s.undefined()) };
    let v = finite_args_from(&a, 1, if n >= 9 { 8 } else { n - 1 })?;
    let Some(v) = v else { return Ok(s.undefined()) };
    let (sw, sh) = (f64::from(source.width), f64::from(source.height));
    let (src, dst) = match v.len() {
        2 => (Rect::new(0.0, 0.0, sw, sh), Rect::new(v[0], v[1], v[0] + sw, v[1] + sh)),
        4 => (Rect::new(0.0, 0.0, sw, sh), Rect::new(v[0], v[1], v[0] + v[2], v[1] + v[3])),
        _ => (Rect::new(v[0], v[1], v[0] + v[2], v[1] + v[3]), Rect::new(v[4], v[5], v[4] + v[6], v[5] + v[7])),
    };
    if !source.clean {
        c.clean.set(false);
    }
    let Some(image) = source.pixmap else { return Ok(s.undefined()) };
    // The source's size may differ from its pixels' (a canvas snapshot is
    // its bitmap); map the source rectangle onto the pixels.
    let (px, py) = (f64::from(image.width()) / sw.max(1.0), f64::from(image.height()) / sh.max(1.0));
    let src = Rect::new(src.x0 * px, src.y0 * py, src.x1 * px, src.y1 * py);
    c.sync_size();
    let state = c.state.borrow().clone();
    c.surface.borrow_mut().draw_image(&image, src, dst, &state.draw_state());
    Ok(s.undefined())
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
    let space = if a.get(4).is_object() {
        let settings = a.get(4).as_object().expect("object").get_atom("colorSpace")?;
        if settings.is_undefined() {
            "srgb".to_owned()
        } else {
            dom_string(settings)?
        }
    } else {
        "srgb".to_owned()
    };
    if space != "srgb" && space != "display-p3" {
        return Err(JsError::Type(format!(
            "The provided value '{space}' is not a valid enum value of type PredefinedColorSpace."
        )));
    }
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
    if (w as u64) * (h as u64) > lp_canvas::MAX_AREA {
        return Err(JsError::Range("Out of memory at ImageData creation.".into()));
    }
    c.sync_size();
    let pixels = c.surface.borrow_mut().get_image_data(x, y, w as u32, h as u32);
    let data = clamped_array(s, &pixels)?;
    module_helper(s, c, "imageDataOf", &[data, s.number(w as f64), s.number(h as f64), s.string(&space)])
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
    let data = typed_bytes(s, info.get_index(2)?)?;
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
        return Err(JsError::Type(format!("Failed to execute '{what}': The provided double value is non-finite.")));
    }
    Ok(v)
}

/// Web IDL `long`: ToInt32.
fn long(v: Value<'_>) -> JsResult<i64> {
    let n = v.to_number()?;
    if !n.is_finite() {
        return Ok(0);
    }
    Ok(i64::from((n.trunc() % 4_294_967_296.0) as i64 as u32 as i32))
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
    let color = |text: String| canvas_color(&text).map(|c| Style::Color(c.clone(), color_of(&c)));
    let style = if v.is_object() {
        if !module_helper(s, c, "isFillStyle", &[v])?.to_bool() {
            // Any other object is converted to a string, as for a color.
            match color(dom_string(v)?) {
                Some(style) => style,
                None => return Ok(()),
            }
        } else {
            Style::Object(Persistent::new(s, v), Rc::default())
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
        return Err(JsError::Type(format!("Failed to execute '{what}': parameter 1 is not of type 'Path2D'.")));
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
            let (outline, k) = stroke_outline(path, &state);
            let p = Point::new(p.x * k, p.y * k);
            outline.winding(p) != 0 || on_outline(&outline, p)
        } else {
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
/// `textBaseline`, given the font's ascent and descent.
fn baseline_shift(baseline: &str, ascent: f64, descent: f64) -> f64 {
    match baseline {
        "top" => ascent,
        "hanging" => ascent * 0.8,
        "middle" => (ascent - descent) / 2.0,
        "ideographic" | "bottom" => -descent,
        _ => 0.0,
    }
}

use kurbo::Shape as _;

pub struct CanvasRenderingContext2D;
impl JsClass for CanvasRenderingContext2D {
    const NAME: &'static str = "CanvasRenderingContext2D";
    type Data = ContextData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.getter_this("canvas", |s, c, this| match c.target {
            Target::Element(canvas) => node_value(s, &c.page, Some(canvas)),
            Target::Offscreen => Ok(s.get_hidden(this, OFFSCREEN)?.unwrap_or_else(|| s.null())),
        });
        def.method("getContextAttributes", 0, |s, _c, _a| {
            let o = s.object();
            o.set_atom("alpha", s.boolean(true))?;
            o.set_atom("colorSpace", s.string("srgb"))?;
            o.set_atom("desynchronized", s.boolean(false))?;
            o.set_atom("willReadFrequently", s.boolean(false))?;
            Ok(o.value())
        });
        def.method("isContextLost", 0, |s, _c, _a| Ok(s.boolean(false)));
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
                // Invalid values are ignored; em/% resolve against 10px.
                if let Some(font) = text_metrics::parse_font(&dom_string(v)?, 10.0) {
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
        enum_state!(
            def,
            "fontStretch",
            font_stretch,
            [
                "ultra-condensed",
                "extra-condensed",
                "condensed",
                "semi-condensed",
                "normal",
                "semi-expanded",
                "expanded",
                "extra-expanded",
                "ultra-expanded"
            ]
        );
        enum_state!(
            def,
            "fontVariantCaps",
            font_variant_caps,
            ["normal", "small-caps", "all-small-caps", "petite-caps", "all-petite-caps", "unicase", "titling-caps"]
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
        string_state!(def, "filter", filter);
        string_state!(def, "letterSpacing", letter_spacing);
        string_state!(def, "wordSpacing", word_spacing);
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
                if let Some(color) = canvas_color(&dom_string(v)?) {
                    c.state.borrow_mut().shadow_color = color;
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
            let layout = layout_text(&text, &state);
            let (width, ascent, descent) = (layout.width, layout.ascent, layout.descent);
            // Where the text starts relative to the alignment point.
            let start = match align_of(&state) {
                Align::Center => -width / 2.0,
                Align::Right => -width,
                Align::Left => 0.0,
            };
            let shift = baseline_shift(state.text_baseline, ascent, descent);
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
                ascent * 0.8 - shift,
                -shift,
                -descent - shift,
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
        def.method("isPointInPath", 2, |s, c, a| is_point_in(s, c, a, false));
        def.method("isPointInStroke", 2, |s, c, a| is_point_in(s, c, a, true));

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
                // A line, with caps.
                b.move_to(v[0], v[1]);
                b.line_to(v[0] + v[2], v[1] + v[3]);
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
        // Focus rings are not drawn.
        def.method("drawFocusIfNeeded", 1, |s, _c, _a| Ok(s.undefined()));
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
        def.method("getLineDash", 0, |s, c, _a| {
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
            let source = image_source(s, &c.page, a.get(0), what, false)?;
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
            let handle = handle_object(s, source)?;
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
            module_helper(s, c, "imageData", &[s.number(w.abs() as f64), s.number(h.abs() as f64), a.get(2)])
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

pub(crate) fn register(engine: &lp_js::Engine) {
    engine.register::<CanvasRenderingContext2D>();
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
        out
    }
}
