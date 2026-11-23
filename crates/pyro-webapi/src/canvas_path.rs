//! Canvas paths: the current path and `Path2D` built in device coordinates
//! (kurbo geometry), and the point queries `isPointInPath`/`isPointInStroke`.

use std::f64::consts::{FRAC_PI_2, PI, TAU};

use kurbo::{Affine, BezPath, ParamCurveNearest, PathEl, Point, Shape, Vec2};
use pyro_js::{JsError, JsResult, Object};

use crate::canvas::State;

// ---- Paths --------------------------------------------------------------

/// A path in canvas (device) coordinates, as HTML builds it: each point is
/// transformed by the transform current when it is added.
#[derive(Default)]
pub(crate) struct Path {
    pub(crate) bez: BezPath,
    /// First point of the last subpath.
    start: Option<Point>,
    /// Last point of the last subpath.
    last: Option<Point>,
    /// The last subpath was closed: the next segment starts a new subpath at
    /// its first point.
    closed: bool,
}

pub(crate) const TOLERANCE: f64 = 0.01;

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
    pub(crate) fn move_to(&mut self, p: Point) {
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
    pub(crate) fn line_to(&mut self, p: Point) {
        if self.last.is_none() {
            return self.move_to(p);
        }
        self.ensure(p);
        self.bez.line_to(p);
        self.last = Some(p);
    }
    pub(crate) fn close(&mut self) {
        if self.last.is_some() && !self.closed {
            self.bez.close_path();
            self.last = self.start;
            self.closed = true;
        }
    }
}

/// Adds segments to a [`Path`] in user coordinates, through `t`.
pub(crate) struct Builder<'a> {
    pub(crate) path: &'a mut Path,
    pub(crate) t: Affine,
}

impl Builder<'_> {
    pub(crate) fn move_to(&mut self, x: f64, y: f64) {
        self.path.move_to(self.t * Point::new(x, y));
    }
    pub(crate) fn line_to(&mut self, x: f64, y: f64) {
        self.path.line_to(self.t * Point::new(x, y));
    }
    pub(crate) fn quad_to(&mut self, cx: f64, cy: f64, x: f64, y: f64) {
        let c = self.t * Point::new(cx, cy);
        self.path.ensure(c);
        let p = self.t * Point::new(x, y);
        self.path.bez.quad_to(c, p);
        self.path.last = Some(p);
    }
    pub(crate) fn cubic_to(&mut self, c1: (f64, f64), c2: (f64, f64), x: f64, y: f64) {
        let c1 = self.t * Point::new(c1.0, c1.1);
        self.path.ensure(c1);
        let p = self.t * Point::new(x, y);
        self.path.bez.curve_to(c1, self.t * Point::new(c2.0, c2.1), p);
        self.path.last = Some(p);
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn ellipse(
        &mut self,
        cx: f64,
        cy: f64,
        rx: f64,
        ry: f64,
        rotation: f64,
        start: f64,
        end: f64,
        ccw: bool,
    ) {
        // HTML: the whole circumference when the angles are 2π or more apart
        // in the drawing direction; otherwise from start to end that way.
        // pyro: as Blink (AdjustEndAngle), angles apart by a multiple of 2π
        // against the drawing direction (`ellipse(..., 2π, 0)` clockwise)
        // also draw the whole circumference, not nothing (WPT
        // 2d.line.join.round, 2d.path.isPointInStroke.scaleddashes).
        let sweep = if !ccw && end - start >= TAU {
            TAU
        } else if ccw && start - end >= TAU {
            -TAU
        } else if !ccw && start > end {
            TAU - (start - end).rem_euclid(TAU)
        } else if ccw && start < end {
            -(TAU - (end - start).rem_euclid(TAU))
        } else if ccw {
            -(start - end)
        } else {
            end - start
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
    pub(crate) fn arc_to(&mut self, x1: f64, y1: f64, x2: f64, y2: f64, r: f64) {
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
    pub(crate) fn rect(&mut self, x: f64, y: f64, w: f64, h: f64) {
        self.move_to(x, y);
        self.line_to(x + w, y);
        self.line_to(x + w, y + h);
        self.line_to(x, y + h);
        self.path.close();
        self.move_to(x, y);
    }
    /// `roundRect` with radii [tl.x, tl.y, tr.x, tr.y, br.x, br.y, bl.x, bl.y].
    pub(crate) fn round_rect(&mut self, mut x: f64, mut y: f64, mut w: f64, mut h: f64, r: [f64; 8]) {
        let (mut tl, mut tr, mut br, mut bl) = ((r[0], r[1]), (r[2], r[3]), (r[4], r[5]), (r[6], r[7]));
        // pyro: HTML traces the subpath with the signed width and height, so one negative side
        // mirrors it and turns it counter-clockwise (WPT 2d.path.roundrect.winding); the new
        // subpath after it starts at the (x, y) given.
        let (x0, y0) = (x, y);
        let reversed = (w < 0.0) != (h < 0.0);
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
        if reversed {
            self.ellipse(x + tl.0, y + tl.1, tl.0, tl.1, 0.0, PI + FRAC_PI_2, PI, true);
            self.line_to(x, y + h - bl.1);
            self.ellipse(x + bl.0, y + h - bl.1, bl.0, bl.1, 0.0, PI, FRAC_PI_2, true);
            self.line_to(x + w - br.0, y + h);
            self.ellipse(x + w - br.0, y + h - br.1, br.0, br.1, 0.0, FRAC_PI_2, 0.0, true);
            self.line_to(x + w, y + tr.1);
            self.ellipse(x + w - tr.0, y + tr.1, tr.0, tr.1, 0.0, 0.0, -FRAC_PI_2, true);
        } else {
            self.line_to(x + w - tr.0, y);
            self.ellipse(x + w - tr.0, y + tr.1, tr.0, tr.1, 0.0, -FRAC_PI_2, 0.0, false);
            self.line_to(x + w, y + h - br.1);
            self.ellipse(x + w - br.0, y + h - br.1, br.0, br.1, 0.0, 0.0, FRAC_PI_2, false);
            self.line_to(x + bl.0, y + h);
            self.ellipse(x + bl.0, y + h - bl.1, bl.0, bl.1, 0.0, FRAC_PI_2, PI, false);
            self.line_to(x, y + tl.1);
            self.ellipse(x + tl.0, y + tl.1, tl.0, tl.1, 0.0, PI, PI + FRAC_PI_2, false);
        }
        self.path.close();
        self.move_to(x0, y0);
    }
    pub(crate) fn svg(&mut self, data: &str) {
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
pub(crate) fn svg_arc_too_large(data: &str) -> bool {
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
pub(crate) fn replay(ops: Object<'_>, path: &mut Path, t: Affine) -> JsResult<()> {
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
pub(crate) fn closed(path: &BezPath) -> BezPath {
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
pub(crate) fn on_outline(path: &BezPath, p: Point) -> bool {
    path.segments().any(|seg| seg.nearest(p, 1e-9).distance_sq < 1e-12)
}

pub(crate) fn point_in_fill(path: &BezPath, p: Point, evenodd: bool) -> bool {
    if !p.x.is_finite() || !p.y.is_finite() {
        return false;
    }
    // Points the transform carried past the range of f64 (a rect under
    // `scale(Number.MAX_VALUE)`) are kept at a finite distance far past any
    // canvas, so the winding still sees the shape around the point; the
    // bound leaves products of two coordinates finite.
    let far = |q: &mut Point| {
        let bound = |v: f64| if v.is_infinite() { v.signum() * 1e150 } else { v };
        *q = Point::new(bound(q.x), bound(q.y));
    };
    let mut path = path.clone();
    for el in path.elements_mut() {
        match el {
            PathEl::MoveTo(a) | PathEl::LineTo(a) => far(a),
            PathEl::QuadTo(a, b) => {
                far(a);
                far(b);
            }
            PathEl::CurveTo(a, b, c) => {
                far(a);
                far(b);
                far(c);
            }
            PathEl::ClosePath => {}
        }
    }
    let path = closed(&path);
    let winding = path.winding(p);
    (if evenodd { winding % 2 != 0 } else { winding != 0 }) || on_outline(&path, p)
}

/// Dash periods along one stroke past which a query ignores the dashes (as
/// drawing does, pyro-canvas `MAX_DASHES`).
const MAX_DASHES: f64 = 100_000.0;

/// The length of `path`'s control polygon: at least its length.
fn control_length(path: &BezPath) -> f64 {
    let (mut last, mut start, mut length) = (Point::ZERO, Point::ZERO, 0.0);
    let mut to = |p: Point, last: &mut Point| {
        length += last.distance(p);
        *last = p;
    };
    for el in path.elements() {
        match *el {
            PathEl::MoveTo(p) => (last, start) = (p, p),
            PathEl::LineTo(p) => to(p, &mut last),
            PathEl::QuadTo(a, b) => {
                to(a, &mut last);
                to(b, &mut last);
            }
            PathEl::CurveTo(a, b, c) => {
                to(a, &mut last);
                to(b, &mut last);
                to(c, &mut last);
            }
            PathEl::ClosePath => to(start, &mut last),
        }
    }
    length
}

/// The outline of `path` (user space: the space the line settings apply
/// in) stroked with `state`'s line settings and dashes, and the factor it was
/// scaled by: wide strokes are computed scaled down to a width of 1 (scale
/// the query point by the same factor), because kurbo flattens round joins
/// and caps at a fixed tolerance and a page's `lineWidth = 1e300` would
/// otherwise never finish.
pub(crate) fn stroke_outline(path: &BezPath, state: &State) -> (BezPath, f64) {
    let width = state.line_width;
    let k = if width > 1.0 { 1.0 / width } else { 1.0 };
    let mut style = state.stroke();
    if !style.dash_pattern.is_empty() {
        let period: f64 = style.dash_pattern.iter().sum();
        if period.is_nan() || period <= 0.0 || control_length(path) / period > MAX_DASHES {
            style.dash_pattern.clear();
        }
    }
    style.width *= k;
    style.dash_offset *= k;
    for dash in style.dash_pattern.iter_mut() {
        *dash *= k;
    }
    let scaled = Affine::scale(k);
    let outline = kurbo::stroke(
        path.elements().iter().map(|el| scaled * *el),
        &style,
        &kurbo::StrokeOpts::default(),
        (TOLERANCE * k).max(1e-6),
    );
    (outline, k)
}

#[cfg(test)]
mod tests {
    use super::*;

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
