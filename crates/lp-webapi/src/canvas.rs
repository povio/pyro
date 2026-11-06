//! `CanvasRenderingContext2D` without a rasterizer: drawing state, paths and
//! text measurement, but no pixels.
//!
//! - The drawing state (styles, line and shadow settings, text settings,
//!   the current transform) is kept, saved and restored like a browser's.
//! - `font` parses and serializes the CSS shorthand, defaulting to `10px sans-serif`.
//! - `measureText` uses the same text measure as the flow layout
//!   (`lp_dom::text_metrics`) and returns a `TextMetrics`.
//! - The current path is built (with the transform applied, kurbo geometry),
//!   so `isPointInPath`/`isPointInStroke` answer for it and for a `Path2D`.
//! - Drawing calls draw nothing. Pixel readback (`getImageData`) returns the
//!   transparent pixels of a canvas nothing was drawn on since it was last
//!   cleared, and throws `NotSupportedError` once something was, so no page
//!   receives invented pixels.
//!
//! The value types (`CanvasGradient`, `CanvasPattern`, `Path2D`, `ImageData`,
//! `TextMetrics`, `DOMMatrix`, `DOMPoint`) are made in JS (canvas2d.js) on a
//! realm's first use of any of them. Glyph bounding boxes are the font's
//! ascent/descent, not per-glyph ink.

use std::cell::{Cell, RefCell};
use std::f64::consts::{FRAC_PI_2, PI, TAU};
use std::rc::Rc;

use kurbo::{Affine, BezPath, ParamCurveNearest, PathEl, Point, Shape, Vec2};
use lp_dom::text_metrics::{self, FontSpec};
use lp_dom::NodeId;
use lp_js::{Args, ClassDef, JsClass, JsError, JsResult, NoParent, Object, Persistent, Scope, Value};

use crate::lazy_module::{lazy_globals, LazyModule};
use crate::node::node_value;
use crate::page::PageDom;
use crate::util::*;
use crate::window::WindowHost;

const CONTEXT_2D: &str = "__lpCanvas2d";

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
    Ok(natives)
}

/// A CSS color as canvas serializes it: `#rrggbb` when opaque, else
/// `rgba(r, g, b, a)`. `currentcolor` is black (no element style applies).
fn canvas_color(value: &str) -> Option<String> {
    let css = lp_dom::computed::serialize_color(value, &mut || Some("rgb(0, 0, 0)".into()))?;
    let Some(inner) = css.strip_prefix("rgb(").and_then(|r| r.strip_suffix(')')) else { return Some(css) };
    let mut hex = String::from("#");
    for part in inner.split(',') {
        let v: u8 = part.trim().parse().ok()?;
        hex.push_str(&format!("{v:02x}"));
    }
    Some(hex)
}

#[derive(Clone)]
enum Style {
    Color(String),
    /// A CanvasGradient or CanvasPattern of the context's realm.
    Object(Persistent),
}

#[derive(Clone)]
struct State {
    font: FontSpec,
    text_align: &'static str,
    text_baseline: &'static str,
    direction: &'static str,
    fill_style: Style,
    stroke_style: Style,
    global_alpha: f64,
    composite: &'static str,
    line_width: f64,
    line_cap: &'static str,
    line_join: &'static str,
    miter_limit: f64,
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
    transform: Affine,
}

impl State {
    fn new() -> Self {
        State {
            font: default_font(),
            text_align: "start",
            text_baseline: "alphabetic",
            direction: "inherit",
            fill_style: Style::Color("#000000".into()),
            stroke_style: Style::Color("#000000".into()),
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
        }
    }
}

pub struct ContextData {
    page: Rc<PageDom>,
    canvas: NodeId,
    state: RefCell<State>,
    stack: RefCell<Vec<State>>,
    path: RefCell<Path>,
    /// Something was drawn since the bitmap was last cleared: its pixels
    /// are unknown here.
    drawn: Cell<bool>,
}

impl ContextData {
    fn reset(&self) {
        *self.state.borrow_mut() = State::new();
        self.stack.borrow_mut().clear();
        *self.path.borrow_mut() = Path::default();
        self.drawn.set(false);
    }
}

fn default_font() -> FontSpec {
    text_metrics::parse_font("10px sans-serif", 10.0).expect("default canvas font")
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
    let data = ContextData {
        page: page.clone(),
        canvas,
        state: RefCell::new(State::new()),
        stack: RefCell::new(Vec::new()),
        path: RefCell::new(Path::default()),
        drawn: Cell::new(false),
    };
    let context = s.wrap::<CanvasRenderingContext2D>(data, None)?;
    s.set_hidden(element, CONTEXT_2D, context.value())?;
    Ok(context.value())
}

/// HTML: setting a canvas's `width` or `height` clears its bitmap and resets
/// its context to the default state.
pub(crate) fn canvas_resized(s: &Scope<'_>, page: &Rc<PageDom>, canvas: NodeId) -> JsResult<()> {
    let element = node_value(s, page, Some(canvas))?;
    let Some(element) = element.as_object() else { return Ok(()) };
    if let Some(context) = s.get_hidden(element, CONTEXT_2D)? {
        if let Some(c) = s.unwrap::<CanvasRenderingContext2D>(context) {
            c.reset();
        }
    }
    Ok(())
}

/// `HTMLCanvasElement.toBlob(callback)`: the callback gets null (the
/// serialization fails: there are no pixels to encode), in a task.
pub(crate) fn to_blob<'s>(s: &Scope<'s>, page: &Rc<PageDom>, a: Args<'s>) -> JsResult<Value<'s>> {
    a.require(1, "HTMLCanvasElement.toBlob")?;
    let callback = a.get(0);
    if !callback.is_function() {
        return Err(JsError::Type(
            "Failed to execute 'toBlob' on 'HTMLCanvasElement': parameter 1 is not of type 'Function'.".into(),
        ));
    }
    let callback = Persistent::new(s, callback);
    let realm = s.realm().downgrade();
    page.post_task(Box::new(move || {
        let Some(realm) = realm.upgrade() else { return };
        let Ok(s) = realm.scope() else { return };
        if let Some(f) = callback.get(&s).as_object() {
            if let Err(e) = f.call(None, &[s.null()]) {
                crate::lifecycle::report_diagnostic(&s, &e);
            }
        }
    }));
    Ok(s.undefined())
}

// ---- Paths --------------------------------------------------------------

/// A path in canvas (device) coordinates, as HTML builds it: each point is
/// transformed by the transform current when it is added.
#[derive(Default)]
struct Path {
    bez: BezPath,
    /// First point of the last subpath.
    start: Option<Point>,
    /// Last point of the last subpath.
    last: Option<Point>,
    /// The last subpath was closed: the next segment starts a new subpath at
    /// its first point.
    closed: bool,
}

const TOLERANCE: f64 = 0.01;

/// The flattening tolerance for an arc of `radius` (user units) drawn through
/// `t`: [`TOLERANCE`] in device pixels, but never below a millionth of the
/// radius. kurbo's segment count grows with radius / tolerance, so a page's
/// `arc(0, 0, 1e300, 0, 6)` would otherwise never finish.
fn arc_tolerance(radius: f64, t: Affine) -> f64 {
    let scale = t.determinant().abs().sqrt();
    let device = if scale.is_finite() && scale > 0.0 { TOLERANCE / scale } else { TOLERANCE };
    device.max(radius * 1e-6)
}

impl Path {
    fn move_to(&mut self, p: Point) {
        self.bez.move_to(p);
        self.start = Some(p);
        self.last = Some(p);
        self.closed = false;
    }
    /// HTML "ensure there is a subpath" for `p` (device coordinates).
    fn ensure(&mut self, p: Point) {
        match self.last {
            None => self.move_to(p),
            Some(last) if self.closed => self.move_to(last),
            Some(_) => {}
        }
    }
    fn line_to(&mut self, p: Point) {
        if self.last.is_none() {
            return self.move_to(p);
        }
        self.ensure(p);
        self.bez.line_to(p);
        self.last = Some(p);
    }
    fn close(&mut self) {
        if self.last.is_some() && !self.closed {
            self.bez.close_path();
            self.last = self.start;
            self.closed = true;
        }
    }
}

/// Adds segments to a [`Path`] in user coordinates, through `t`.
struct Builder<'a> {
    path: &'a mut Path,
    t: Affine,
}

impl Builder<'_> {
    fn move_to(&mut self, x: f64, y: f64) {
        self.path.move_to(self.t * Point::new(x, y));
    }
    fn line_to(&mut self, x: f64, y: f64) {
        self.path.line_to(self.t * Point::new(x, y));
    }
    fn quad_to(&mut self, cx: f64, cy: f64, x: f64, y: f64) {
        let c = self.t * Point::new(cx, cy);
        self.path.ensure(c);
        let p = self.t * Point::new(x, y);
        self.path.bez.quad_to(c, p);
        self.path.last = Some(p);
    }
    fn cubic_to(&mut self, c1: (f64, f64), c2: (f64, f64), x: f64, y: f64) {
        let c1 = self.t * Point::new(c1.0, c1.1);
        self.path.ensure(c1);
        let p = self.t * Point::new(x, y);
        self.path.bez.curve_to(c1, self.t * Point::new(c2.0, c2.1), p);
        self.path.last = Some(p);
    }
    #[allow(clippy::too_many_arguments)]
    fn ellipse(&mut self, cx: f64, cy: f64, rx: f64, ry: f64, rotation: f64, start: f64, end: f64, ccw: bool) {
        let sweep = if !ccw && end - start >= TAU {
            TAU
        } else if ccw && start - end >= TAU {
            -TAU
        } else if ccw {
            -(start - end).rem_euclid(TAU)
        } else {
            (end - start).rem_euclid(TAU)
        };
        let center = Point::new(cx, cy);
        let radii = Vec2::new(rx, ry);
        let rot = Affine::rotate(rotation);
        let first = center + (rot * Point::new(rx * start.cos(), ry * start.sin())).to_vec2();
        let p = self.t * first;
        if self.path.last.is_some() {
            self.path.line_to(p);
        } else {
            self.path.move_to(p);
        }
        if rx == 0.0 || ry == 0.0 || sweep == 0.0 {
            let last = center + (rot * Point::new(rx * (start + sweep).cos(), ry * (start + sweep).sin())).to_vec2();
            return self.path.line_to(self.t * last);
        }
        let arc = kurbo::Arc { center, radii, start_angle: start, sweep_angle: sweep, x_rotation: rotation };
        let mut end_point = p;
        for el in arc.append_iter(arc_tolerance(rx.max(ry), self.t)) {
            if let PathEl::CurveTo(a, b, c) = el {
                end_point = self.t * c;
                self.path.bez.curve_to(self.t * a, self.t * b, end_point);
            }
        }
        self.path.last = Some(end_point);
    }
    fn arc_to(&mut self, x1: f64, y1: f64, x2: f64, y2: f64, r: f64) {
        let p1 = Point::new(x1, y1);
        self.path.ensure(self.t * p1);
        let Some(last) = self.path.last else { return };
        // The last point, back in user coordinates.
        let p0 = if self.t.determinant() == 0.0 { last } else { self.t.inverse() * last };
        let p2 = Point::new(x2, y2);
        let (v1, v2) = (p0 - p1, p2 - p1);
        let cross = v1.x * v2.y - v1.y * v2.x;
        if p0 == p1 || p1 == p2 || r == 0.0 || cross.abs() < 1e-12 {
            return self.line_to(x1, y1);
        }
        let (u1, u2) = (v1.normalize(), v2.normalize());
        let angle = u1.dot(u2).clamp(-1.0, 1.0).acos();
        let distance = r / (angle / 2.0).tan();
        let t1 = p1 + u1 * distance;
        let t2 = p1 + u2 * distance;
        let bisector = (u1 + u2).normalize();
        let center = p1 + bisector * (r / (angle / 2.0).sin());
        let a1 = (t1.y - center.y).atan2(t1.x - center.x);
        let a2 = (t2.y - center.y).atan2(t2.x - center.x);
        // The arc runs the short way from t1 to t2: anticlockwise when p0,
        // p1, p2 turn clockwise.
        self.ellipse(center.x, center.y, r, r, 0.0, a1, a2, cross > 0.0);
    }
    fn rect(&mut self, x: f64, y: f64, w: f64, h: f64) {
        self.move_to(x, y);
        self.line_to(x + w, y);
        self.line_to(x + w, y + h);
        self.line_to(x, y + h);
        self.path.close();
        self.move_to(x, y);
    }
    /// `roundRect` with radii [tl.x, tl.y, tr.x, tr.y, br.x, br.y, bl.x, bl.y].
    fn round_rect(&mut self, mut x: f64, mut y: f64, mut w: f64, mut h: f64, r: [f64; 8]) {
        let (mut tl, mut tr, mut br, mut bl) = ((r[0], r[1]), (r[2], r[3]), (r[4], r[5]), (r[6], r[7]));
        if w < 0.0 {
            x += w;
            w = -w;
            std::mem::swap(&mut tl, &mut tr);
            std::mem::swap(&mut bl, &mut br);
        }
        if h < 0.0 {
            y += h;
            h = -h;
            std::mem::swap(&mut tl, &mut bl);
            std::mem::swap(&mut tr, &mut br);
        }
        let mut scale: f64 = 1.0;
        for (sum, side) in [(tl.0 + tr.0, w), (tr.1 + br.1, h), (br.0 + bl.0, w), (tl.1 + bl.1, h)] {
            if sum > 0.0 {
                scale = scale.min(side / sum);
            }
        }
        let s = |c: (f64, f64)| (c.0 * scale, c.1 * scale);
        let (tl, tr, br, bl) = (s(tl), s(tr), s(br), s(bl));
        self.move_to(x + tl.0, y);
        self.line_to(x + w - tr.0, y);
        self.ellipse(x + w - tr.0, y + tr.1, tr.0, tr.1, 0.0, -FRAC_PI_2, 0.0, false);
        self.line_to(x + w, y + h - br.1);
        self.ellipse(x + w - br.0, y + h - br.1, br.0, br.1, 0.0, 0.0, FRAC_PI_2, false);
        self.line_to(x + bl.0, y + h);
        self.ellipse(x + bl.0, y + h - bl.1, bl.0, bl.1, 0.0, FRAC_PI_2, PI, false);
        self.line_to(x, y + tl.1);
        self.ellipse(x + tl.0, y + tl.1, tl.0, tl.1, 0.0, PI, PI + FRAC_PI_2, false);
        self.path.close();
        self.move_to(x, y);
    }
    fn svg(&mut self, data: &str) {
        // A path whose data has an error draws nothing here (HTML keeps the
        // part before the error). kurbo flattens SVG arcs at a fixed
        // tolerance, so arcs with huge numbers would take forever: such
        // data draws nothing either.
        if svg_arc_too_large(data) {
            return;
        }
        let Ok(bez) = BezPath::from_svg(data) else { return };
        for el in bez.elements() {
            match *el {
                PathEl::MoveTo(p) => self.path.move_to(self.t * p),
                PathEl::LineTo(p) => self.path.line_to(self.t * p),
                PathEl::QuadTo(c, p) => self.quad_to(c.x, c.y, p.x, p.y),
                PathEl::CurveTo(a, b, p) => self.cubic_to((a.x, a.y), (b.x, b.y), p.x, p.y),
                PathEl::ClosePath => self.path.close(),
            }
        }
    }
}

/// Whether SVG path `data` has an arc command and a number of magnitude
/// 1e12 or more (an integer part of 13+ digits or a positive exponent of 12+):
/// kurbo would cut such an arc into millions of curves.
fn svg_arc_too_large(data: &str) -> bool {
    if !data.bytes().any(|b| b == b'A' || b == b'a') {
        return false;
    }
    let bytes = data.as_bytes();
    let mut i = 0;
    let mut digits = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'0'..=b'9' => {
                digits += 1;
                if digits >= 13 {
                    return true;
                }
            }
            b'e' | b'E' => {
                digits = 0;
                let mut j = i + 1;
                let negative = bytes.get(j) == Some(&b'-');
                if matches!(bytes.get(j), Some(b'-' | b'+')) {
                    j += 1;
                }
                let mut exponent = 0u32;
                while let Some(d @ b'0'..=b'9') = bytes.get(j) {
                    exponent = exponent.saturating_mul(10).saturating_add(u32::from(d - b'0'));
                    j += 1;
                }
                if !negative && exponent >= 12 {
                    return true;
                }
                i = j;
                continue;
            }
            _ => digits = 0,
        }
        i += 1;
    }
    false
}

/// Replays the ops of a Path2D (canvas2d.js) into `path` through `t`.
fn replay(ops: Object<'_>, path: &mut Path, t: Affine) -> JsResult<()> {
    let len = ops.get_atom("length")?.to_number()? as u32;
    let mut stack = vec![t];
    let mut i = 0;
    let num = |k: u32| -> JsResult<f64> { ops.get_index(k)?.to_number() };
    let args = |k: u32, n: u32| -> JsResult<Vec<f64>> { (k..k + n).map(num).collect() };
    while i < len {
        let op = num(i)? as u32;
        let t = *stack.last().expect("transform stack");
        let mut b = Builder { path, t };
        let n = match op {
            0 | 1 => 2,
            2 | 7 => 4,
            3 | 10 => 6,
            4 => 5,
            5 => 6,
            6 => 8,
            9 => 12,
            8 | 11 => 0,
            12 => 1,
            _ => return Err(JsError::Type("corrupt Path2D".into())),
        };
        if op == 12 {
            b.svg(&ops.get_index(i + 1)?.to_string()?);
            i += 2;
            continue;
        }
        let v = args(i + 1, n)?;
        match op {
            0 => b.move_to(v[0], v[1]),
            1 => b.line_to(v[0], v[1]),
            2 => b.quad_to(v[0], v[1], v[2], v[3]),
            3 => b.cubic_to((v[0], v[1]), (v[2], v[3]), v[4], v[5]),
            4 => b.arc_to(v[0], v[1], v[2], v[3], v[4]),
            5 => b.ellipse(v[0], v[1], v[2], v[2], 0.0, v[3], v[4], v[5] != 0.0),
            6 => b.ellipse(v[0], v[1], v[2], v[3], v[4], v[5], v[6], v[7] != 0.0),
            7 => b.rect(v[0], v[1], v[2], v[3]),
            8 => b.path.close(),
            9 => b.round_rect(v[0], v[1], v[2], v[3], v[4..12].try_into().expect("8 radii")),
            10 => stack.push(t * Affine::new([v[0], v[1], v[2], v[3], v[4], v[5]])),
            _ => {
                if stack.len() > 1 {
                    stack.pop();
                }
            }
        }
        i += 1 + n;
    }
    Ok(())
}

/// `path` with every subpath closed (filling closes them implicitly).
fn closed(path: &BezPath) -> BezPath {
    let mut out = BezPath::new();
    let mut open = false;
    for el in path.elements() {
        match el {
            PathEl::MoveTo(_) => {
                if open {
                    out.close_path();
                }
                open = true;
            }
            PathEl::ClosePath => open = false,
            _ => {}
        }
        out.push(*el);
    }
    if open {
        out.close_path();
    }
    out
}

/// Whether `p` is on one of `path`'s segments (counts as inside).
fn on_outline(path: &BezPath, p: Point) -> bool {
    path.segments().any(|seg| seg.nearest(p, 1e-9).distance_sq < 1e-12)
}

fn point_in_fill(path: &BezPath, p: Point, evenodd: bool) -> bool {
    if !p.x.is_finite() || !p.y.is_finite() {
        return false;
    }
    let path = closed(path);
    let winding = path.winding(p);
    (if evenodd { winding % 2 != 0 } else { winding != 0 }) || on_outline(&path, p)
}

/// The outline of `path` stroked with `state`'s line settings, and the
/// factor it was scaled by: wide strokes are computed scaled down to a width
/// of 1 (scale the query point by the same factor), because kurbo flattens
/// round joins and caps at a fixed tolerance and a page's
/// `lineWidth = 1e300` would otherwise never finish.
fn stroke_outline(path: &BezPath, state: &State) -> (BezPath, f64) {
    let t = state.transform;
    let scale = t.determinant().abs().sqrt();
    let width = state.line_width * scale;
    let k = if width > 1.0 { 1.0 / width } else { 1.0 };
    let cap = match state.line_cap {
        "round" => kurbo::Cap::Round,
        "square" => kurbo::Cap::Square,
        _ => kurbo::Cap::Butt,
    };
    let join = match state.line_join {
        "round" => kurbo::Join::Round,
        "bevel" => kurbo::Join::Bevel,
        _ => kurbo::Join::Miter,
    };
    let style = kurbo::Stroke::new(width * k).with_caps(cap).with_join(join).with_miter_limit(state.miter_limit);
    let scaled = Affine::scale(k);
    let outline = kurbo::stroke(
        path.elements().iter().map(|el| scaled * *el),
        &style,
        &kurbo::StrokeOpts::default(),
        (TOLERANCE * k).max(1e-6),
    );
    (outline, k)
}

// ---- The context ----------------------------------------------------------

fn finite_args(a: &Args<'_>, n: usize) -> JsResult<Option<Vec<f64>>> {
    let mut v = Vec::with_capacity(n);
    for i in 0..n {
        v.push(a.get(i).to_number()?);
    }
    Ok(v.iter().all(|x| x.is_finite()).then_some(v))
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
        Style::Color(c) => s.string(c),
        Style::Object(o) => o.get(s),
    }
}

fn set_style(s: &Scope<'_>, c: &ContextData, v: Value<'_>, fill: bool) -> JsResult<()> {
    let style = if v.is_object() {
        if !module_helper(s, c, "isFillStyle", &[v])?.to_bool() {
            // Any other object is converted to a string, as for a color.
            match canvas_color(&dom_string(v)?) {
                Some(color) => Style::Color(color),
                None => return Ok(()),
            }
        } else {
            Style::Object(Persistent::new(s, v))
        }
    } else {
        match canvas_color(&dom_string(v)?) {
            Some(color) => Style::Color(color),
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

const COMPOSITE: [&str; 26] = [
    "source-over",
    "source-in",
    "source-out",
    "source-atop",
    "destination-over",
    "destination-in",
    "destination-out",
    "destination-atop",
    "lighter",
    "copy",
    "xor",
    "multiply",
    "screen",
    "overlay",
    "darken",
    "lighten",
    "color-dodge",
    "color-burn",
    "hard-light",
    "soft-light",
    "difference",
    "exclusion",
    "hue",
    "saturation",
    "color",
    "luminosity",
];

fn canvas_size(c: &ContextData) -> (f64, f64) {
    let dom = c.page.dom.borrow();
    let attr = |name: &str, default: f64| {
        dom.get_attribute(c.canvas, name)
            .ok()
            .flatten()
            .and_then(|v| v.trim().parse::<u32>().ok())
            .map_or(default, f64::from)
    };
    (attr("width", 300.0), attr("height", 150.0))
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
    let rule = a.get(base + 2);
    let evenodd = if rule.is_undefined() {
        false
    } else {
        match dom_string(rule)?.as_str() {
            "nonzero" => false,
            "evenodd" => true,
            other => {
                return Err(JsError::Type(format!(
                    "Failed to execute '{what}': The provided value '{other}' is not a valid enum value of type CanvasFillRule."
                )));
            }
        }
    };
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

fn image_source_kind(s: &Scope<'_>, v: Value<'_>) -> Option<(Rc<PageDom>, NodeId, String)> {
    let n = s.unwrap::<crate::node::Node>(v)?;
    let page = n.page().clone();
    let local = {
        let dom = page.dom.borrow();
        let e = dom.element(n.id).ok()?;
        if !e.is_html() && &*e.name.local != "image" {
            return None;
        }
        e.name.local.to_string()
    };
    Some((page, n.id, local))
}

pub struct CanvasRenderingContext2D;
impl JsClass for CanvasRenderingContext2D {
    const NAME: &'static str = "CanvasRenderingContext2D";
    type Data = ContextData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.getter("canvas", |s, c| node_value(s, &c.page, Some(c.canvas)));
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
                if let Some(&op) = COMPOSITE.iter().find(|k| **k == v) {
                    c.state.borrow_mut().composite = op;
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
            let state = c.state.borrow().clone();
            let font = &state.font;
            let width = if font.is_monospace() {
                text_metrics::monospace_width(&text, font.size)
            } else {
                text_metrics::text_width(&text, font.size, font.weight)
            };
            let (ascent, descent) = text_metrics::vertical_metrics(font.size);
            // Left-to-right: start/left align at x, end/right end at x.
            let left = match state.text_align {
                "center" => width / 2.0,
                "end" | "right" => width,
                _ => 0.0,
            };
            let shift = baseline_shift(state.text_baseline, ascent, descent);
            let values = [
                width,
                left,
                width - left,
                ascent - shift,
                descent + shift,
                ascent - shift,
                descent + shift,
                ascent - shift,
                descent + shift,
                ascent * 0.8 - shift,
                -shift,
                -descent - shift,
            ];
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

        // Drawing: no pixels. The bitmap's content is unknown afterwards.
        for name in
            ["fillRect", "strokeRect", "fillText", "strokeText", "fill", "stroke", "drawImage", "drawFocusIfNeeded"]
        {
            def.method(name, 0, |s, c, _a| {
                c.drawn.set(true);
                Ok(s.undefined())
            });
        }
        def.method("clip", 0, |s, _c, _a| Ok(s.undefined()));
        def.method("clearRect", 4, |s, c, a| {
            a.require(4, "CanvasRenderingContext2D.clearRect")?;
            // Clearing the whole bitmap makes its pixels known again.
            if let Some(v) = finite_args(&a, 4)? {
                let (w, h) = canvas_size(c);
                let (x0, x1) = (v[0].min(v[0] + v[2]), v[0].max(v[0] + v[2]));
                let (y0, y1) = (v[1].min(v[1] + v[3]), v[1].max(v[1] + v[3]));
                let t = c.state.borrow().transform;
                if t == Affine::IDENTITY && x0 <= 0.0 && y0 <= 0.0 && x1 >= w && y1 >= h {
                    c.drawn.set(false);
                }
            }
            Ok(s.undefined())
        });
        def.method("setLineDash", 1, |s, c, a| {
            a.require(1, "CanvasRenderingContext2D.setLineDash")?;
            let list = a.get(0).as_object().ok_or_else(|| {
                JsError::Type(
                    "Failed to execute 'setLineDash' on 'CanvasRenderingContext2D': The provided value cannot be converted to a sequence."
                        .into(),
                )
            })?;
            let len = list.get_atom("length")?.to_number()? as u32;
            let mut dash = Vec::with_capacity(len as usize);
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
            a.require(2, "CanvasRenderingContext2D.createPattern")?;
            let not_source = || {
                JsError::Type(
                    "Failed to execute 'createPattern' on 'CanvasRenderingContext2D': The provided value is not of type '(CSSImageValue or HTMLCanvasElement or HTMLImageElement or HTMLVideoElement or ImageBitmap or OffscreenCanvas or SVGImageElement or VideoFrame)'."
                        .into(),
                )
            };
            let (page, node, local) = image_source_kind(s, a.get(0)).ok_or_else(not_source)?;
            let repetition = if a.get(1).is_null() { String::new() } else { dom_string(a.get(1))? };
            let repetition = match repetition.as_str() {
                "" | "repeat" => "repeat",
                "repeat-x" => "repeat-x",
                "repeat-y" => "repeat-y",
                "no-repeat" => "no-repeat",
                other => {
                    return Err(JsError::dom(
                        "SyntaxError",
                        format!("The provided type ('{other}') is not one of 'repeat', 'no-repeat', 'repeat-x', or 'repeat-y'."),
                    ))
                }
            };
            match local.as_str() {
                "canvas" => {
                    let size = {
                        let dom = page.dom.borrow();
                        let attr = |name: &str, default: u32| {
                            dom.get_attribute(node, name)
                                .ok()
                                .flatten()
                                .and_then(|v| v.trim().parse::<u32>().ok())
                                .unwrap_or(default)
                        };
                        (attr("width", 300), attr("height", 150))
                    };
                    if size.0 == 0 || size.1 == 0 {
                        return Err(JsError::dom("InvalidStateError", "The image argument is a canvas element with a width or height of 0."));
                    }
                }
                "img" => {
                    if !page.image_complete(node) {
                        return Ok(s.null());
                    }
                    if page.dom.borrow().natural_size(node).is_none_or(|(w, h)| w == 0.0 || h == 0.0) {
                        return Err(JsError::dom("InvalidStateError", "The HTMLImageElement provided is in the 'broken' state."));
                    }
                }
                // No video frames (no media decoding) and no SVG images.
                "video" | "image" => return Ok(s.null()),
                _ => return Err(not_source()),
            }
            module_helper(s, c, "pattern", &[s.string(repetition)])
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
        def.method("getImageData", 4, |s, c, a| {
            a.require(4, "CanvasRenderingContext2D.getImageData")?;
            let (w, h) = (long(a.get(2))?, long(a.get(3))?);
            if w == 0 || h == 0 {
                return Err(JsError::dom(
                    "IndexSizeError",
                    format!("The source {} is 0.", if w == 0 { "width" } else { "height" }),
                ));
            }
            if c.drawn.get() {
                return Err(JsError::dom(
                    "NotSupportedError",
                    "Canvas pixels are not available: there is no rasterizer for what was drawn.",
                ));
            }
            // Nothing drawn since the bitmap was cleared: transparent black.
            module_helper(s, c, "imageData", &[s.number(w.abs() as f64), s.number(h.abs() as f64), a.get(4)])
        });
        def.method("putImageData", 3, |s, c, a| {
            a.require(3, "CanvasRenderingContext2D.putImageData")?;
            if module_helper(s, c, "imageSize", &[a.get(0)])?.is_null() {
                return Err(JsError::Type(
                    "Failed to execute 'putImageData' on 'CanvasRenderingContext2D': parameter 1 is not of type 'ImageData'."
                        .into(),
                ));
            }
            c.drawn.set(true);
            Ok(s.undefined())
        });
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_serialize_like_canvas() {
        assert_eq!(canvas_color("red").as_deref(), Some("#ff0000"));
        assert_eq!(canvas_color("rgba(0, 128, 255, 0.5)").as_deref(), Some("rgba(0, 128, 255, 0.5)"));
        assert_eq!(canvas_color("not a color"), None);
    }

    #[test]
    fn paths_answer_point_queries() {
        let mut path = Path::default();
        let mut b = Builder { path: &mut path, t: Affine::IDENTITY };
        b.rect(0.0, 0.0, 10.0, 10.0);
        assert!(point_in_fill(&path.bez, Point::new(5.0, 5.0), false));
        assert!(point_in_fill(&path.bez, Point::new(10.0, 5.0), false), "edges are inside");
        assert!(!point_in_fill(&path.bez, Point::new(11.0, 5.0), false));
        // An open subpath is closed for filling; arcs bulge.
        let mut path = Path::default();
        let mut b = Builder { path: &mut path, t: Affine::translate((100.0, 0.0)) };
        b.ellipse(0.0, 50.0, 20.0, 20.0, 0.0, 0.0, TAU, false);
        assert!(point_in_fill(&path.bez, Point::new(100.0, 50.0), false));
        assert!(point_in_fill(&path.bez, Point::new(118.0, 50.0), false));
        assert!(!point_in_fill(&path.bez, Point::new(0.0, 50.0), false));
    }

    #[test]
    fn huge_arcs_and_strokes_finish() {
        let mut path = Path::default();
        let mut b = Builder { path: &mut path, t: Affine::IDENTITY };
        b.ellipse(0.0, 0.0, 1e300, 1e300, 0.0, 0.0, TAU, false);
        assert!(path.bez.elements().len() < 100);
        assert!(point_in_fill(&path.bez, Point::new(5.0, 5.0), false));

        let mut path = Path::default();
        let mut b = Builder { path: &mut path, t: Affine::IDENTITY };
        b.move_to(0.0, 0.0);
        b.line_to(100.0, 0.0);
        b.line_to(100.0, 100.0);
        let mut state = State::new();
        state.line_join = "round";
        state.line_cap = "round";
        state.line_width = 1e300;
        stroke_outline(&path.bez, &state);
        state.line_width = 1e6;
        let (outline, k) = stroke_outline(&path.bez, &state);
        assert!(outline.winding(Point::new(50.0 * k, 50.0 * k)) != 0);
        assert!(outline.winding(Point::new(0.0, 600_000.0 * k)) == 0);
        state.line_width = 4.0;
        let (outline, k) = stroke_outline(&path.bez, &state);
        assert!(outline.winding(Point::new(50.0 * k, 1.0 * k)) != 0);
        assert!(outline.winding(Point::new(50.0 * k, 3.0 * k)) == 0);

        assert!(svg_arc_too_large("M0 0 A1e300 1e300 0 1 1 1 1"));
        assert!(svg_arc_too_large("M0 0a9999999999999 1 0 1 1 1 1"));
        assert!(!svg_arc_too_large("M0 0 A10 10 0 1 1 1e-20 1"));
        assert!(!svg_arc_too_large("M0 0 L1e300 0"));
        let mut path = Path::default();
        Builder { path: &mut path, t: Affine::IDENTITY }.svg("M0 0 A1e300 1e300 0 1 1 1 1");
        assert!(path.bez.elements().is_empty());
    }
}
