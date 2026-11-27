//! `SVGGraphicsElement.getBBox()`: the tight bounding box of an element's
//! geometry in its own user space (its `transform` excluded, its
//! descendants' transforms applied), as Blink's
//! `LayoutSVGShape::ObjectBoundingBox` and container union.
//!
//! Supported: `rect`, `circle`, `ellipse`, `line`, `polyline`, `polygon`,
//! `path` (all commands; curves and arcs use their exact extrema), the
//! containers `svg`, `g`, `a`, `switch` and `defs`, and `text`/`tspan`
//! (advance width from the text measure, height from the font's
//! ascent + descent). Lengths are user units; percentages and CSS-only
//! geometry properties are not resolved. Non-rendered children
//! (`display: none`) do not contribute.

use crate::layout::Rect;
use crate::style::StyleEngine;
use crate::{Dom, NodeId};

/// Axis-aligned bounds under construction.
#[derive(Copy, Clone, Debug)]
struct Bounds {
    min_x: f64,
    min_y: f64,
    max_x: f64,
    max_y: f64,
}

impl Bounds {
    fn point(x: f64, y: f64) -> Bounds {
        Bounds { min_x: x, min_y: y, max_x: x, max_y: y }
    }
    fn add(&mut self, x: f64, y: f64) {
        self.min_x = self.min_x.min(x);
        self.min_y = self.min_y.min(y);
        self.max_x = self.max_x.max(x);
        self.max_y = self.max_y.max(y);
    }
    fn union(a: Option<Bounds>, b: Bounds) -> Bounds {
        match a {
            Some(mut a) => {
                a.add(b.min_x, b.min_y);
                a.add(b.max_x, b.max_y);
                a
            }
            None => b,
        }
    }
    fn rect(self) -> Rect {
        Rect { x: self.min_x, y: self.min_y, width: self.max_x - self.min_x, height: self.max_y - self.min_y }
    }
}

/// `getBBox()` of `el`; an empty rect when it has no supported geometry.
pub fn bbox(dom: &Dom, style: &StyleEngine, el: NodeId) -> Rect {
    bounds(dom, style, el, 0).map_or(Rect { x: 0.0, y: 0.0, width: 0.0, height: 0.0 }, Bounds::rect)
}

/// `el`'s bbox mapped into the content box of its `svg` ancestor (CSS px
/// from the box's content origin): `el`'s own `transform` and those of the
/// `g`/`a`/`switch` ancestors below `svg` apply, then the svg's `viewBox`
/// and `preserveAspectRatio` scale into `viewport` (its content-box width
/// and height). None when `el` has no supported geometry.
pub fn client_bounds(dom: &Dom, style: &StyleEngine, el: NodeId, svg: NodeId, viewport: (f64, f64)) -> Option<Rect> {
    let mut b = bounds(dom, style, el, 0)?;
    let mut cur = Some(el);
    while let Some(n) = cur.filter(|&n| n != svg) {
        if let Some(m) = dom.get_attribute(n, "transform").ok().flatten().and_then(parse_transform) {
            b = transform_bounds(m, b);
        }
        cur = dom.parent_element(n).ok().flatten();
    }
    if let Some(m) = view_box_transform(dom, svg, viewport) {
        b = transform_bounds(m, b);
    }
    Some(b.rect())
}

/// The viewBox-to-viewport matrix of `svg` (SVG 2, 8.2), None without a
/// usable `viewBox`.
fn view_box_transform(dom: &Dom, svg: NodeId, viewport: (f64, f64)) -> Option<[f64; 6]> {
    let vb = dom.get_attribute(svg, "viewBox").ok().flatten()?;
    let [min_x, min_y, w, h] = numbers(vb)[..] else { return None };
    if w <= 0.0 || h <= 0.0 || viewport.0 <= 0.0 || viewport.1 <= 0.0 {
        return None;
    }
    let aspect = dom.svg_aspect_ratio(svg).ok()?;
    let (mut sx, mut sy) = (viewport.0 / w, viewport.1 / h);
    let (mut tx, mut ty) = (0.0, 0.0);
    if let Some((x, y)) = aspect.alignment() {
        let scale = if aspect.meet_or_slice() == 2 { sx.max(sy) } else { sx.min(sy) };
        (sx, sy) = (scale, scale);
        tx = x * (viewport.0 - w * scale);
        ty = y * (viewport.1 - h * scale);
    }
    Some([sx, 0.0, 0.0, sy, tx - min_x * sx, ty - min_y * sy])
}

fn attr(dom: &Dom, el: NodeId, name: &str) -> Option<f64> {
    let v = dom.get_attribute(el, name).ok().flatten()?.trim();
    let v = v.strip_suffix("px").unwrap_or(v);
    v.parse::<f64>().ok().filter(|n| n.is_finite())
}

fn num(dom: &Dom, el: NodeId, name: &str) -> f64 {
    attr(dom, el, name).unwrap_or(0.0)
}

fn local_name(dom: &Dom, el: NodeId) -> Option<String> {
    let e = dom.element(el).ok()?;
    (e.name.ns == markup5ever::ns!(svg)).then(|| e.name.local.to_string())
}

fn bounds(dom: &Dom, style: &StyleEngine, el: NodeId, depth: u8) -> Option<Bounds> {
    if depth > 64 {
        return None;
    }
    match local_name(dom, el)?.as_str() {
        "rect" | "image" | "foreignObject" | "use" => {
            let (w, h) = (num(dom, el, "width"), num(dom, el, "height"));
            if w < 0.0 || h < 0.0 {
                return None;
            }
            let (x, y) = (num(dom, el, "x"), num(dom, el, "y"));
            let mut b = Bounds::point(x, y);
            b.add(x + w, y + h);
            Some(b)
        }
        "circle" => {
            let (cx, cy, r) = (num(dom, el, "cx"), num(dom, el, "cy"), num(dom, el, "r"));
            let mut b = Bounds::point(cx - r, cy - r);
            b.add(cx + r, cy + r);
            Some(b)
        }
        "ellipse" => {
            let (cx, cy) = (num(dom, el, "cx"), num(dom, el, "cy"));
            let (rx, ry) = (num(dom, el, "rx"), num(dom, el, "ry"));
            let mut b = Bounds::point(cx - rx, cy - ry);
            b.add(cx + rx, cy + ry);
            Some(b)
        }
        "line" => {
            let mut b = Bounds::point(num(dom, el, "x1"), num(dom, el, "y1"));
            b.add(num(dom, el, "x2"), num(dom, el, "y2"));
            Some(b)
        }
        "polyline" | "polygon" => {
            let points = dom.get_attribute_ns(el, None, "points").ok().flatten()?;
            let mut b: Option<Bounds> = None;
            for (x, y) in crate::svg_point_list::parse_points(points) {
                b = Some(Bounds::union(b, Bounds::point(f64::from(x), f64::from(y))));
            }
            b
        }
        "path" => path_bounds(dom.get_attribute(el, "d").ok().flatten()?),
        "text" | "tspan" => text_bounds(dom, style, el),
        "svg" | "g" | "a" | "switch" | "defs" => {
            let mut b: Option<Bounds> = None;
            for c in dom.element_children(el).ok()? {
                if style.has_display_none(dom, c) {
                    continue;
                }
                let Some(cb) = bounds(dom, style, c, depth + 1) else { continue };
                let cb = match dom.get_attribute(c, "transform").ok().flatten().and_then(parse_transform) {
                    Some(m) => transform_bounds(m, cb),
                    None => cb,
                };
                b = Some(Bounds::union(b, cb));
            }
            b
        }
        _ => None,
    }
}

fn text_bounds(dom: &Dom, style: &StyleEngine, el: NodeId) -> Option<Bounds> {
    let text = dom.text_content(el).ok().flatten().unwrap_or_default();
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let size = style.computed_font_size(dom, Some(el));
    let weight =
        style.stylo_style(dom, el).map_or(400, |s| (s.get_font().font_weight.value() as f64).clamp(1.0, 1000.0) as u16);
    let width = crate::text_metrics::text_width(&text, size, weight);
    let (ascent, descent) = crate::text_metrics::vertical_metrics(size);
    let (x, y) = (num(dom, el, "x"), num(dom, el, "y"));
    let x = match style.used_value(dom, el, "text-anchor").as_deref().map(str::trim) {
        Some("middle") => x - width / 2.0,
        Some("end") => x - width,
        _ => x,
    };
    let mut b = Bounds::point(x, y - ascent);
    b.add(x + width, y + descent);
    Some(b)
}

/// Numbers in an SVG number list (`points`, path data arguments).
fn numbers(s: &str) -> Vec<f64> {
    let mut out = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_digit() || c == b'.' || c == b'-' || c == b'+' {
            let start = i;
            i += 1;
            let mut dot = c == b'.';
            let mut exp = false;
            while i < b.len() {
                let d = b[i];
                if d.is_ascii_digit() {
                    i += 1;
                } else if d == b'.' && !dot && !exp {
                    dot = true;
                    i += 1;
                } else if (d == b'e' || d == b'E') && !exp {
                    exp = true;
                    i += 1;
                    if i < b.len() && (b[i] == b'-' || b[i] == b'+') {
                        i += 1;
                    }
                } else {
                    break;
                }
            }
            if let Ok(n) = s[start..i].parse::<f64>() {
                out.push(n);
            }
        } else {
            i += 1;
        }
    }
    out
}

/// Path data split into (command, arguments). Arc flags may be written
/// without separators ("a4 4 0 014 4").
fn path_segments(d: &str) -> Vec<(char, Vec<f64>)> {
    let mut out = Vec::new();
    let mut cmd: Option<char> = None;
    let mut args = String::new();
    for c in d.chars() {
        if c.is_ascii_alphabetic() && c != 'e' && c != 'E' {
            if let Some(k) = cmd {
                out.push((k, arc_aware_numbers(k, &args)));
            }
            cmd = Some(c);
            args.clear();
        } else {
            args.push(c);
        }
    }
    if let Some(k) = cmd {
        out.push((k, arc_aware_numbers(k, &args)));
    }
    out
}

fn arc_aware_numbers(cmd: char, s: &str) -> Vec<f64> {
    if !matches!(cmd, 'A' | 'a') {
        return numbers(s);
    }
    // Every 7 arguments, the 4th and 5th are single-character flags.
    let mut out = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let slot = out.len() % 7;
        if b[i].is_ascii_whitespace() || b[i] == b',' {
            i += 1;
            continue;
        }
        if slot == 3 || slot == 4 {
            match b[i] {
                b'0' => out.push(0.0),
                b'1' => out.push(1.0),
                _ => return out,
            }
            i += 1;
            continue;
        }
        let rest = &s[i..];
        let first = numbers(rest);
        let Some(&n) = first.first() else { return out };
        // Advance past the number just read.
        let mut j = 0;
        let rb = rest.as_bytes();
        let mut seen_digit = false;
        let mut dot = false;
        let mut exp = false;
        while j < rb.len() {
            let d = rb[j];
            if d.is_ascii_digit() {
                seen_digit = true;
                j += 1;
            } else if (d == b'-' || d == b'+') && (j == 0 || matches!(rb[j - 1], b'e' | b'E')) {
                j += 1;
            } else if d == b'.' && !dot && !exp {
                dot = true;
                j += 1;
            } else if (d == b'e' || d == b'E') && seen_digit && !exp {
                exp = true;
                j += 1;
            } else {
                break;
            }
        }
        out.push(n);
        i += j.max(1);
    }
    out
}

fn path_bounds(d: &str) -> Option<Bounds> {
    let mut b: Option<Bounds> = None;
    let (mut x, mut y) = (0.0f64, 0.0f64);
    let (mut sx, mut sy) = (0.0f64, 0.0f64);
    // Last control point, for S/s and T/t reflection.
    let mut last_cubic: Option<(f64, f64)> = None;
    let mut last_quad: Option<(f64, f64)> = None;
    let add = |b: &mut Option<Bounds>, px: f64, py: f64| *b = Some(Bounds::union(*b, Bounds::point(px, py)));
    for (cmd, args) in path_segments(d) {
        let rel = cmd.is_ascii_lowercase();
        match cmd.to_ascii_uppercase() {
            'M' => {
                for (i, p) in args.as_chunks::<2>().0.iter().enumerate() {
                    let (px, py) = if rel { (x + p[0], y + p[1]) } else { (p[0], p[1]) };
                    x = px;
                    y = py;
                    if i == 0 {
                        sx = x;
                        sy = y;
                    }
                    add(&mut b, x, y);
                }
                last_cubic = None;
                last_quad = None;
                continue;
            }
            'L' => {
                for p in args.as_chunks::<2>().0 {
                    if rel {
                        x += p[0];
                        y += p[1];
                    } else {
                        x = p[0];
                        y = p[1];
                    }
                    add(&mut b, x, y);
                }
            }
            'H' => {
                for &v in &args {
                    x = if rel { x + v } else { v };
                    add(&mut b, x, y);
                }
            }
            'V' => {
                for &v in &args {
                    y = if rel { y + v } else { v };
                    add(&mut b, x, y);
                }
            }
            'C' | 'S' => {
                let smooth = cmd.eq_ignore_ascii_case(&'S');
                let n = if smooth { 4 } else { 6 };
                for p in args.chunks_exact(n) {
                    let (ox, oy) = if rel { (x, y) } else { (0.0, 0.0) };
                    let (c1x, c1y) = if smooth {
                        last_cubic.map_or((x, y), |(lx, ly)| (2.0 * x - lx, 2.0 * y - ly))
                    } else {
                        (ox + p[0], oy + p[1])
                    };
                    let k = if smooth { 0 } else { 2 };
                    let (c2x, c2y) = (ox + p[k], oy + p[k + 1]);
                    let (ex, ey) = (ox + p[k + 2], oy + p[k + 3]);
                    for t in cubic_extrema(x, c1x, c2x, ex).into_iter().chain(cubic_extrema(y, c1y, c2y, ey)) {
                        add(&mut b, cubic(x, c1x, c2x, ex, t), cubic(y, c1y, c2y, ey, t));
                    }
                    add(&mut b, ex, ey);
                    last_cubic = Some((c2x, c2y));
                    x = ex;
                    y = ey;
                }
                last_quad = None;
                continue;
            }
            'Q' | 'T' => {
                let smooth = cmd.eq_ignore_ascii_case(&'T');
                let n = if smooth { 2 } else { 4 };
                for p in args.chunks_exact(n) {
                    let (ox, oy) = if rel { (x, y) } else { (0.0, 0.0) };
                    let (cx, cy) = if smooth {
                        last_quad.map_or((x, y), |(lx, ly)| (2.0 * x - lx, 2.0 * y - ly))
                    } else {
                        (ox + p[0], oy + p[1])
                    };
                    let k = if smooth { 0 } else { 2 };
                    let (ex, ey) = (ox + p[k], oy + p[k + 1]);
                    for t in quad_extremum(x, cx, ex).into_iter().chain(quad_extremum(y, cy, ey)) {
                        add(&mut b, quad(x, cx, ex, t), quad(y, cy, ey, t));
                    }
                    add(&mut b, ex, ey);
                    last_quad = Some((cx, cy));
                    x = ex;
                    y = ey;
                }
                last_cubic = None;
                continue;
            }
            'A' => {
                for p in args.as_chunks::<7>().0 {
                    let (ex, ey) = if rel { (x + p[5], y + p[6]) } else { (p[5], p[6]) };
                    for (px, py) in arc_extrema(x, y, p[0], p[1], p[2], p[3] != 0.0, p[4] != 0.0, ex, ey) {
                        add(&mut b, px, py);
                    }
                    add(&mut b, ex, ey);
                    x = ex;
                    y = ey;
                }
            }
            'Z' => {
                x = sx;
                y = sy;
            }
            _ => return b,
        }
        last_cubic = None;
        last_quad = None;
    }
    b
}

fn cubic(p0: f64, p1: f64, p2: f64, p3: f64, t: f64) -> f64 {
    let u = 1.0 - t;
    u * u * u * p0 + 3.0 * u * u * t * p1 + 3.0 * u * t * t * p2 + t * t * t * p3
}

fn quad(p0: f64, p1: f64, p2: f64, t: f64) -> f64 {
    let u = 1.0 - t;
    u * u * p0 + 2.0 * u * t * p1 + t * t * p2
}

/// Parameters in (0, 1) where a cubic's derivative is zero on one axis.
fn cubic_extrema(p0: f64, p1: f64, p2: f64, p3: f64) -> Vec<f64> {
    let a = -p0 + 3.0 * p1 - 3.0 * p2 + p3;
    let b = 2.0 * (p0 - 2.0 * p1 + p2);
    let c = p1 - p0;
    let mut ts = Vec::new();
    if a.abs() < 1e-12 {
        if b.abs() > 1e-12 {
            ts.push(-c / b);
        }
    } else {
        let disc = b * b - 4.0 * a * c;
        if disc >= 0.0 {
            let r = disc.sqrt();
            ts.push((-b + r) / (2.0 * a));
            ts.push((-b - r) / (2.0 * a));
        }
    }
    ts.retain(|t| *t > 0.0 && *t < 1.0);
    ts
}

fn quad_extremum(p0: f64, p1: f64, p2: f64) -> Option<f64> {
    let den = p0 - 2.0 * p1 + p2;
    (den.abs() > 1e-12).then(|| (p0 - p1) / den).filter(|t| *t > 0.0 && *t < 1.0)
}

/// Axis extrema of an elliptical arc that lie on the drawn sweep
/// (SVG 2 implementation notes B.2.4/B.2.5: endpoint to center).
#[allow(clippy::too_many_arguments)]
fn arc_extrema(
    x1: f64,
    y1: f64,
    rx: f64,
    ry: f64,
    rotation: f64,
    large: bool,
    sweep: bool,
    x2: f64,
    y2: f64,
) -> Vec<(f64, f64)> {
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    if rx == 0.0 || ry == 0.0 || (x1 == x2 && y1 == y2) {
        return Vec::new();
    }
    let phi = rotation.to_radians();
    let (sin, cos) = phi.sin_cos();
    let dx = (x1 - x2) / 2.0;
    let dy = (y1 - y2) / 2.0;
    let x1p = cos * dx + sin * dy;
    let y1p = -sin * dx + cos * dy;
    let lambda = x1p * x1p / (rx * rx) + y1p * y1p / (ry * ry);
    if lambda > 1.0 {
        rx *= lambda.sqrt();
        ry *= lambda.sqrt();
    }
    let num = rx * rx * ry * ry - rx * rx * y1p * y1p - ry * ry * x1p * x1p;
    let den = rx * rx * y1p * y1p + ry * ry * x1p * x1p;
    let mut coef = if den == 0.0 { 0.0 } else { (num / den).max(0.0).sqrt() };
    if large == sweep {
        coef = -coef;
    }
    let cxp = coef * rx * y1p / ry;
    let cyp = -coef * ry * x1p / rx;
    let cx = cos * cxp - sin * cyp + (x1 + x2) / 2.0;
    let cy = sin * cxp + cos * cyp + (y1 + y2) / 2.0;
    let angle = |ux: f64, uy: f64| uy.atan2(ux);
    let theta1 = angle((x1p - cxp) / rx, (y1p - cyp) / ry);
    let mut delta = angle((-x1p - cxp) / rx, (-y1p - cyp) / ry) - theta1;
    let tau = std::f64::consts::TAU;
    if sweep && delta < 0.0 {
        delta += tau;
    } else if !sweep && delta > 0.0 {
        delta -= tau;
    }
    let point = |t: f64| {
        let (st, ct) = t.sin_cos();
        (cx + rx * cos * ct - ry * sin * st, cy + rx * sin * ct + ry * cos * st)
    };
    let on_sweep = |t: f64| {
        let mut d = (t - theta1).rem_euclid(tau);
        if delta < 0.0 {
            d = (theta1 - t).rem_euclid(tau);
        }
        d <= delta.abs()
    };
    let tx = (-ry * sin).atan2(rx * cos);
    let ty = (ry * cos).atan2(rx * sin);
    [tx, tx + std::f64::consts::PI, ty, ty + std::f64::consts::PI]
        .into_iter()
        .filter(|&t| on_sweep(t))
        .map(point)
        .collect()
}

/// Affine matrix [a b c d e f] from a `transform` attribute.
fn parse_transform(s: &str) -> Option<[f64; 6]> {
    let mut m = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
    let mut rest = s;
    while let Some(open) = rest.find('(') {
        let name = rest[..open].trim().trim_start_matches(',').trim();
        let close = rest[open..].find(')')? + open;
        let v = numbers(&rest[open + 1..close]);
        let t = match (name, v.as_slice()) {
            ("matrix", [a, b, c, d, e, f]) => [*a, *b, *c, *d, *e, *f],
            ("translate", [tx]) => [1.0, 0.0, 0.0, 1.0, *tx, 0.0],
            ("translate", [tx, ty]) => [1.0, 0.0, 0.0, 1.0, *tx, *ty],
            ("scale", [s]) => [*s, 0.0, 0.0, *s, 0.0, 0.0],
            ("scale", [sx, sy]) => [*sx, 0.0, 0.0, *sy, 0.0, 0.0],
            ("rotate", [a]) => rotate(*a, 0.0, 0.0),
            ("rotate", [a, cx, cy]) => rotate(*a, *cx, *cy),
            ("skewX", [a]) => [1.0, 0.0, a.to_radians().tan(), 1.0, 0.0, 0.0],
            ("skewY", [a]) => [1.0, a.to_radians().tan(), 0.0, 1.0, 0.0, 0.0],
            _ => return None,
        };
        m = multiply(m, t);
        rest = &rest[close + 1..];
    }
    Some(m)
}

fn rotate(deg: f64, cx: f64, cy: f64) -> [f64; 6] {
    let (s, c) = deg.to_radians().sin_cos();
    let r = [c, s, -s, c, 0.0, 0.0];
    multiply(multiply([1.0, 0.0, 0.0, 1.0, cx, cy], r), [1.0, 0.0, 0.0, 1.0, -cx, -cy])
}

pub fn multiply(m: [f64; 6], n: [f64; 6]) -> [f64; 6] {
    [
        m[0] * n[0] + m[2] * n[1],
        m[1] * n[0] + m[3] * n[1],
        m[0] * n[2] + m[2] * n[3],
        m[1] * n[2] + m[3] * n[3],
        m[0] * n[4] + m[2] * n[5] + m[4],
        m[1] * n[4] + m[3] * n[5] + m[5],
    ]
}

fn transform_bounds(m: [f64; 6], b: Bounds) -> Bounds {
    let map = |x: f64, y: f64| (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]);
    let (x0, y0) = map(b.min_x, b.min_y);
    let mut out = Bounds::point(x0, y0);
    for (x, y) in [(b.max_x, b.min_y), (b.min_x, b.max_y), (b.max_x, b.max_y)] {
        let (px, py) = map(x, y);
        out.add(px, py);
    }
    out
}

/// Hit-test SVG graphics in reverse paint order. The caller has resolved
/// the viewport's position among the surrounding CSS boxes.
pub(crate) fn hit_test(
    dom: &Dom,
    style: &StyleEngine,
    svg: NodeId,
    viewport: (f64, f64),
    x: f64,
    y: f64,
) -> Option<NodeId> {
    use kurbo::{Affine, Point};
    if x < 0. || y < 0. || x > viewport.0 || y > viewport.1 {
        return None;
    }
    let transform = view_box_transform(dom, svg, viewport).map(Affine::new).unwrap_or(Affine::IDENTITY);
    hit_children(dom, style, svg, Point::new(x, y), transform, 0)
}

fn hit_children(
    dom: &Dom,
    style: &StyleEngine,
    parent: NodeId,
    point: kurbo::Point,
    parent_transform: kurbo::Affine,
    depth: u8,
) -> Option<NodeId> {
    if depth >= 64 {
        return None;
    }
    let mut child = dom.last_child(parent).ok().flatten();
    while let Some(el) = child {
        child = dom.prev_sibling(el).ok().flatten();
        let Ok(element) = dom.element(el) else { continue };
        if element.name.ns != markup5ever::ns!(svg) {
            continue;
        }
        let tag = &*element.name.local;
        if !matches!(
            tag,
            "g" | "a" | "rect" | "circle" | "ellipse" | "line" | "polygon" | "polyline" | "path" | "text" | "image"
        ) {
            continue;
        }
        let Some(computed) = style.stylo_style(dom, el) else { continue };
        // A clipped subtree needs its clip geometry to select a target.
        // Leave it to the existing viewport hit until that geometry is available.
        if computed.get_box().display.is_none()
            || !matches!(computed.slow_clone_clip_path(), style::values::computed::basic_shape::ClipPath::None)
        {
            continue;
        }
        let transform = match dom.get_attribute(el, "transform").ok().flatten().and_then(parse_transform) {
            Some(matrix) => parent_transform * kurbo::Affine::new(matrix),
            None => parent_transform,
        };
        let p = transform.inverse() * point;
        if !p.x.is_finite() || !p.y.is_finite() {
            continue;
        }
        // Non-graphics elements such as defs, clipPath and mask do not
        // participate in the viewport's painting order.
        let container = matches!(tag, "g" | "a");
        if container {
            if let Some(hit) = hit_children(dom, style, el, point, transform, depth + 1) {
                return Some(hit);
            }
        }
        use style::computed_values::pointer_events::T as Pe;
        let pe = computed.get_inherited_ui().pointer_events;
        if pe == Pe::None
            || matches!(pe, Pe::Auto | Pe::Visiblepainted | Pe::Visiblefill | Pe::Visiblestroke | Pe::Visible)
                && computed.get_inherited_box().visibility != style::computed_values::visibility::T::Visible
        {
            continue;
        }
        let in_bounds = || {
            bounds(dom, style, el, 0)
                .is_some_and(|b| p.x >= b.min_x && p.x <= b.max_x && p.y >= b.min_y && p.y <= b.max_y)
        };
        if pe == Pe::BoundingBox {
            if in_bounds() {
                return Some(el);
            }
            continue;
        }
        if container {
            continue;
        }
        // Transparent paints and zero opacity still paint for pointer-events.
        use style::values::generics::svg::SVGPaintKind;
        let svg = computed.get_inherited_svg();
        let painted = matches!(pe, Pe::Auto | Pe::Visiblepainted | Pe::Painted);
        let fill =
            !matches!(pe, Pe::Stroke | Pe::Visiblestroke) && (!painted || !matches!(svg.fill.kind, SVGPaintKind::None));
        let stroke =
            !matches!(pe, Pe::Fill | Pe::Visiblefill) && (!painted || !matches!(svg.stroke.kind, SVGPaintKind::None));
        if !fill && !stroke {
            continue;
        }
        if tag == "text" {
            // SVG text hits character cells for both fill and stroke.
            if in_bounds() {
                return Some(el);
            }
            continue;
        }
        if let Some(path) = shape_path(dom, el, tag, false, |name| attr(dom, el, name)) {
            if fill
                && tag != "line"
                && fill_hit(&path, p, crate::stylo::serialize(&computed, "fill-rule").as_deref() == Some("evenodd"))
            {
                return Some(el);
            }
            if stroke {
                let Some(outline) = stroke_style(&computed, 1.0) else { continue };
                // Non-scaling strokes are constructed in host coordinates.
                use style::values::specified::svg::VectorEffect;
                let (path, stroke_point) =
                    if computed.get_svg().vector_effect.contains(VectorEffect::NON_SCALING_STROKE) {
                        (transform * path, point)
                    } else {
                        (path, p)
                    };
                if path_hit(&kurbo::stroke(path.iter(), &outline, &Default::default(), 0.01), stroke_point, false) {
                    return Some(el);
                }
            }
        }
    }
    None
}

/// SVG closes every open contour for fill. The stroke still follows the
/// original open path, including its end caps.
fn fill_hit(path: &kurbo::BezPath, p: kurbo::Point, evenodd: bool) -> bool {
    use kurbo::PathEl;
    let mut previous: Option<&PathEl> = None;
    let mut closed = None;
    for (i, el) in path.elements().iter().enumerate() {
        if matches!(el, PathEl::MoveTo(_)) && previous.is_some_and(|e| !matches!(e, PathEl::ClosePath)) {
            closed.get_or_insert_with(|| kurbo::BezPath::from_vec(path.elements()[..i].to_vec())).close_path();
        }
        if let Some(copy) = &mut closed {
            copy.push(*el);
        }
        previous = Some(el);
    }
    if previous.is_some_and(|e| !matches!(e, PathEl::ClosePath)) {
        closed.get_or_insert_with(|| path.clone()).close_path();
    }
    path_hit(closed.as_ref().unwrap_or(path), p, evenodd)
}

fn path_hit(path: &kurbo::BezPath, p: kurbo::Point, evenodd: bool) -> bool {
    use kurbo::{ParamCurveNearest, Shape};
    let bounds = path.bounding_box();
    if p.x < bounds.x0 - 1e-6 || p.x > bounds.x1 + 1e-6 || p.y < bounds.y0 - 1e-6 || p.y > bounds.y1 + 1e-6 {
        return false;
    }
    let winding = path.winding(p);
    (if evenodd { winding % 2 != 0 } else { winding != 0 })
        || path.segments().any(|s| s.nearest(p, 0.001).distance_sq < 1e-12)
}

/// The `stroke-*` properties of `computed` as a kurbo stroke; `dash_scale` converts `pathLength`
/// units to user units. None when the stroke is not wider than zero.
fn stroke_style(computed: &style::properties::ComputedValues, dash_scale: f64) -> Option<kurbo::Stroke> {
    use kurbo::{Cap, Join, Stroke};
    let property = |name: &str| crate::stylo::serialize(computed, name).unwrap_or_default();
    let length = |s: &str| s.trim_end_matches("px").parse::<f64>().ok();
    let width = length(&property("stroke-width")).unwrap_or(1.);
    if width <= 0. {
        return None;
    }
    let cap = match property("stroke-linecap").as_str() {
        "square" => Cap::Square,
        "round" => Cap::Round,
        _ => Cap::Butt,
    };
    let join = match property("stroke-linejoin").as_str() {
        "bevel" => Join::Bevel,
        "round" => Join::Round,
        _ => Join::Miter,
    };
    let mut outline = Stroke::new(width)
        .with_caps(cap)
        .with_join(join)
        .with_miter_limit(length(&property("stroke-miterlimit")).unwrap_or(4.));
    let dashes = property("stroke-dasharray");
    let dashes: Vec<f64> = dashes.split([',', ' ']).filter_map(length).map(|d| d * dash_scale).collect();
    if dashes.iter().any(|&n| n > 0.) {
        outline = outline.with_dashes(length(&property("stroke-dashoffset")).unwrap_or(0.) * dash_scale, dashes);
    }
    Some(outline)
}

/// User units per `pathLength` unit of `path` (1 without a positive `pathLength`).
fn path_length_scale(dom: &Dom, el: NodeId, path: &kurbo::BezPath) -> f64 {
    use kurbo::ParamCurveArclen;
    let Some(declared) = attr(dom, el, "pathLength").filter(|n| *n > 0.0) else { return 1.0 };
    let total: f64 = path.segments().map(|segment| segment.arclen(1e-3)).sum();
    if total > 0.0 { total / declared } else { 1.0 }
}

fn affine_of(dom: &Dom, el: NodeId) -> kurbo::Affine {
    dom.get_attribute(el, "transform").ok().flatten().and_then(parse_transform).map_or(kurbo::Affine::IDENTITY, kurbo::Affine::new)
}

/// The `transform`s of `el` and its SVG ancestors, up to the outermost: the matrix to the
/// outermost `svg`'s user space (no `viewBox`), what `vector-effect: non-scaling-stroke` measures in.
fn user_to_host(dom: &Dom, el: NodeId) -> kurbo::Affine {
    let mut matrix = kurbo::Affine::IDENTITY;
    let mut current = Some(el);
    while let Some(id) = current {
        if local_name(dom, id).is_none() {
            break;
        }
        matrix = affine_of(dom, id) * matrix;
        current = dom.parent_element(id).ok().flatten();
    }
    matrix
}

/// `SVGGeometryElement.isPointInFill`: (`x`, `y`) in the element's user space against the shape's
/// fill region (`fill-rule` decides; `pointer-events` and `visibility` do not apply).
pub fn is_point_in_fill(dom: &Dom, layout: &crate::layout::Layout, el: NodeId, x: f64, y: f64) -> bool {
    // A line encloses no area.
    if local_name(dom, el).as_deref() == Some("line") {
        return false;
    }
    let Some(path) = metric_path(dom, layout, el) else { return false };
    let evenodd = layout.style().stylo_style(dom, el)
        .is_some_and(|computed| crate::stylo::serialize(&computed, "fill-rule").as_deref() == Some("evenodd"));
    fill_hit(&path, kurbo::Point::new(x, y), evenodd)
}

/// `SVGGeometryElement.isPointInStroke`: (`x`, `y`) in the element's user space against the shape's
/// stroke outline (width, caps, joins, miter limit, dashes, `pathLength` and `non-scaling-stroke`).
pub fn is_point_in_stroke(dom: &Dom, layout: &crate::layout::Layout, el: NodeId, x: f64, y: f64) -> bool {
    let Some(path) = metric_path(dom, layout, el) else { return false };
    let Some(computed) = layout.style().stylo_style(dom, el) else { return false };
    let Some(outline) = stroke_style(&computed, path_length_scale(dom, el, &path)) else { return false };
    let point = kurbo::Point::new(x, y);
    use style::values::specified::svg::VectorEffect;
    let (path, point) = if computed.get_svg().vector_effect.contains(VectorEffect::NON_SCALING_STROKE) {
        let host = user_to_host(dom, el);
        (host * path, host * point)
    } else {
        (path, point)
    };
    path_hit(&kurbo::stroke(path.iter(), &outline, &Default::default(), 0.01), point, false)
}

/// Whether the filled or stroked region `path` has a point in `rect`: a corner of the rectangle
/// inside it, or its outline through the rectangle.
fn region_hits_rect(path: &kurbo::BezPath, rect: kurbo::Rect, evenodd: bool) -> bool {
    use kurbo::{PathEl, Point};
    for (x, y) in [(rect.x0, rect.y0), (rect.x1, rect.y0), (rect.x0, rect.y1), (rect.x1, rect.y1)] {
        if path_hit(path, Point::new(x, y), evenodd) {
            return true;
        }
    }
    let (mut hit, mut last, mut start) = (false, Point::ZERO, Point::ZERO);
    kurbo::flatten(path.iter(), 0.05, |el| match el {
        PathEl::MoveTo(p) => {
            start = p;
            last = p;
        }
        PathEl::LineTo(p) => {
            hit |= segment_hits_rect(last, p, rect);
            last = p;
        }
        PathEl::ClosePath => {
            hit |= segment_hits_rect(last, start, rect);
            last = start;
        }
        _ => {}
    });
    hit
}

/// Liang-Barsky: whether the segment `a`-`b` has a point in `r`.
fn segment_hits_rect(a: kurbo::Point, b: kurbo::Point, r: kurbo::Rect) -> bool {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let (mut t0, mut t1) = (0.0_f64, 1.0_f64);
    for (p, q) in [(-dx, a.x - r.x0), (dx, r.x1 - a.x), (-dy, a.y - r.y0), (dy, r.y1 - a.y)] {
        if p == 0.0 {
            if q < 0.0 {
                return false;
            }
            continue;
        }
        let t = q / p;
        if p < 0.0 {
            if t > t1 {
                return false;
            }
            t0 = t0.max(t);
        } else {
            if t < t0 {
                return false;
            }
            t1 = t1.min(t);
        }
    }
    true
}

/// Closes every contour of `path` (the region SVG fills).
fn closed_contours(path: &kurbo::BezPath) -> kurbo::BezPath {
    use kurbo::PathEl;
    let mut out = kurbo::BezPath::new();
    let mut open = false;
    for el in path.elements() {
        if matches!(el, PathEl::MoveTo(_)) && open {
            out.close_path();
        }
        open = !matches!(el, PathEl::ClosePath);
        out.push(*el);
    }
    if open {
        out.close_path();
    }
    out
}

/// `getIntersectionList` / `getEnclosureList` of `svg` (`rect` in its user space, its own `viewBox`
/// and nested viewports not applied): the rendered graphics elements below `reference` (default `svg`)
/// whose painted geometry, in document order, intersects `rect` (or lies inside it, `enclosure`).
pub fn intersection_list(
    dom: &Dom,
    layout: &crate::layout::Layout,
    svg: NodeId,
    reference: Option<NodeId>,
    rect: Rect,
    enclosure: bool,
) -> Vec<NodeId> {
    let query = kurbo::Rect::new(rect.x, rect.y, rect.x + rect.width, rect.y + rect.height).abs();
    let mut found = Vec::new();
    let root = reference.unwrap_or(svg);
    let mut emit = |id: NodeId| found.push(id);
    if root == svg {
        for child in dom.element_children(svg).into_iter().flatten() {
            visit_graphics(dom, layout, child, query, enclosure, kurbo::Affine::IDENTITY, 0, &mut emit);
        }
    } else {
        // The reference's ancestors below `svg` place it in `svg`'s user space.
        let mut to_svg = kurbo::Affine::IDENTITY;
        let mut ancestor = dom.parent_element(root).ok().flatten();
        while let Some(id) = ancestor.filter(|&id| id != svg) {
            to_svg = affine_of(dom, id) * to_svg;
            ancestor = dom.parent_element(id).ok().flatten();
        }
        visit_graphics(dom, layout, root, query, enclosure, to_svg, 0, &mut emit);
    }
    found
}

/// `checkIntersection` / `checkEnclosure`: whether `el`, or a graphics element below it, is in
/// that list for `rect`.
pub fn check_intersection(
    dom: &Dom,
    layout: &crate::layout::Layout,
    svg: NodeId,
    el: NodeId,
    rect: Rect,
    enclosure: bool,
) -> bool {
    let query = kurbo::Rect::new(rect.x, rect.y, rect.x + rect.width, rect.y + rect.height).abs();
    let mut to_svg = kurbo::Affine::IDENTITY;
    let mut ancestor = dom.parent_element(el).ok().flatten();
    while let Some(id) = ancestor.filter(|&id| id != svg) {
        to_svg = affine_of(dom, id) * to_svg;
        ancestor = dom.parent_element(id).ok().flatten();
    }
    let mut any = false;
    visit_graphics(dom, layout, el, query, enclosure, to_svg, 0, &mut |_| any = true);
    any
}

/// Reports `el` when it is a graphics element that matches, or the matching ones below a container.
/// `to_svg` maps the user space of `el`'s parent to the query's.
#[allow(clippy::too_many_arguments)]
fn visit_graphics(
    dom: &Dom,
    layout: &crate::layout::Layout,
    el: NodeId,
    query: kurbo::Rect,
    enclosure: bool,
    to_svg: kurbo::Affine,
    depth: u8,
    emit: &mut dyn FnMut(NodeId),
) {
    if depth > 32 {
        return;
    }
    let Some(tag) = local_name(dom, el) else { return };
    if matches!(
        tag.as_str(),
        "defs" | "clipPath" | "mask" | "marker" | "pattern" | "symbol" | "linearGradient" | "radialGradient"
            | "filter" | "style" | "script" | "title" | "desc" | "metadata"
    ) || layout.style().has_display_none(dom, el)
    {
        return;
    }
    let mut matrix = to_svg * affine_of(dom, el);
    match tag.as_str() {
        "g" | "a" | "switch" | "svg" => {
            if tag == "svg" {
                matrix = matrix * kurbo::Affine::translate((num(dom, el, "x"), num(dom, el, "y")));
            }
            for child in dom.element_children(el).into_iter().flatten() {
                visit_graphics(dom, layout, child, query, enclosure, matrix, depth + 1, emit);
            }
        }
        "use" => {
            let xlink = dom.get_attribute_ns(el, Some("http://www.w3.org/1999/xlink"), "href").ok().flatten();
            let href = dom.get_attribute(el, "href").ok().flatten().or(xlink);
            let target = href.and_then(|h| h.strip_prefix('#')).and_then(|id| {
                let doc = dom.node_document(el).ok()?;
                dom.get_element_by_id(doc, id).ok().flatten()
            });
            let Some(target) = target else { return };
            let placed = matrix * kurbo::Affine::translate((num(dom, el, "x"), num(dom, el, "y")));
            let mut any = false;
            // The reference's own `transform` applies inside the instance, below the use's.
            visit_graphics(dom, layout, target, query, enclosure, placed, depth + 1, &mut |_| any = true);
            if any {
                emit(el);
            }
        }
        "rect" | "circle" | "ellipse" | "line" | "polyline" | "polygon" | "path" | "text" | "image" | "foreignObject" => {
            if leaf_matches(dom, layout, el, &tag, query, enclosure, matrix) {
                emit(el);
            }
        }
        _ => {}
    }
}

/// The user-space box of the `clipPath` that `el`'s `clip-path` names (the union of its
/// children's boxes), None without one (or without any shape in it).
fn clip_box(dom: &Dom, layout: &crate::layout::Layout, el: NodeId) -> Option<Bounds> {
    let value = layout.style().used_value(dom, el, "clip-path").filter(|v| v.contains('#'))
        .or_else(|| dom.get_attribute(el, "clip-path").ok().flatten().map(str::to_owned))?;
    let start = value.find('#')? + 1;
    let end = value[start..].find(['"', '\'', ')']).map_or(value.len(), |n| start + n);
    let doc = dom.node_document(el).ok()?;
    let clip = dom.get_element_by_id(doc, &value[start..end]).ok().flatten()?;
    if local_name(dom, clip).as_deref() != Some("clipPath") {
        return None;
    }
    let mut union: Option<Bounds> = None;
    for child in dom.element_children(clip).ok()? {
        let Some(b) = bounds(dom, layout.style(), child, 0) else { continue };
        let b = match dom.get_attribute(child, "transform").ok().flatten().and_then(parse_transform) {
            Some(m) => transform_bounds(m, b),
            None => b,
        };
        union = Some(Bounds::union(union, b));
    }
    union
}

fn leaf_matches(
    dom: &Dom,
    layout: &crate::layout::Layout,
    el: NodeId,
    tag: &str,
    query: kurbo::Rect,
    enclosure: bool,
    to_svg: kurbo::Affine,
) -> bool {
    use kurbo::Shape;
    use style::computed_values::pointer_events::T as Pe;
    use style::values::generics::svg::SVGPaintKind;
    let Some(computed) = layout.style().stylo_style(dom, el) else { return false };
    let pe = computed.get_inherited_ui().pointer_events;
    if pe == Pe::None
        || matches!(pe, Pe::Auto | Pe::Visiblepainted | Pe::Visiblefill | Pe::Visiblestroke | Pe::Visible)
            && computed.get_inherited_box().visibility != style::computed_values::visibility::T::Visible
    {
        return false;
    }
    // The clip path's box limits the part of the element that counts.
    let mut query = query;
    let clipped = clip_box(dom, layout, el).map(|b| {
        let b = transform_bounds(to_svg.as_coeffs(), b);
        kurbo::Rect::new(b.min_x, b.min_y, b.max_x, b.max_y)
    });
    if let Some(clip) = clipped {
        if !enclosure {
            if query.x0 > clip.x1 || clip.x0 > query.x1 || query.y0 > clip.y1 || clip.y0 > query.y1 {
                return false;
            }
            query = query.intersect(clip);
        }
    }
    let svg_paint = computed.get_inherited_svg();
    let painted = matches!(pe, Pe::Auto | Pe::Visiblepainted | Pe::Painted);
    let fill = !matches!(pe, Pe::Stroke | Pe::Visiblestroke) && (!painted || !matches!(svg_paint.fill.kind, SVGPaintKind::None));
    let stroke = !matches!(pe, Pe::Fill | Pe::Visiblefill) && (!painted || !matches!(svg_paint.stroke.kind, SVGPaintKind::None));
    if matches!(tag, "text" | "image" | "foreignObject") {
        let Some(b) = bounds(dom, layout.style(), el, 0) else { return false };
        let b = transform_bounds(to_svg.as_coeffs(), b);
        let r = kurbo::Rect::new(b.min_x, b.min_y, b.max_x, b.max_y);
        return if enclosure {
            r.x0 >= query.x0 && r.y0 >= query.y0 && r.x1 <= query.x1 && r.y1 <= query.y1
        } else {
            r.x0 <= query.x1 && r.x1 >= query.x0 && r.y0 <= query.y1 && r.y1 >= query.y0
        };
    }
    let Some(path) = metric_path(dom, layout, el) else { return false };
    let evenodd = crate::stylo::serialize(&computed, "fill-rule").as_deref() == Some("evenodd");
    let mut regions: Vec<(kurbo::BezPath, bool)> = Vec::new();
    if fill && tag != "line" {
        regions.push((to_svg * closed_contours(&path), evenodd));
    }
    if stroke {
        if let Some(outline) = stroke_style(&computed, path_length_scale(dom, el, &path)) {
            use style::values::specified::svg::VectorEffect;
            let region = if computed.get_svg().vector_effect.contains(VectorEffect::NON_SCALING_STROKE) {
                kurbo::stroke((to_svg * path.clone()).iter(), &outline, &Default::default(), 0.01)
            } else {
                to_svg * kurbo::stroke(path.iter(), &outline, &Default::default(), 0.01)
            };
            regions.push((region, false));
        }
    }
    if enclosure {
        // Everything the element paints lies inside the rectangle (the clip box narrows nothing).
        let inside = |b: kurbo::Rect| b.x0 >= query.x0 && b.y0 >= query.y0 && b.x1 <= query.x1 && b.y1 <= query.y1;
        return !regions.is_empty() && regions.iter().all(|(region, _)| inside(region.bounding_box()));
    }
    regions.iter().any(|(region, evenodd)| region_hits_rect(region, query, *evenodd))
}

/// Path metrics use the element's equivalent path in local user coordinates.
/// Author calibration via pathLength affects rendering, not these DOM APIs.
pub fn total_length(dom: &Dom, layout: &crate::layout::Layout, el: NodeId) -> f64 {
    use kurbo::ParamCurveArclen;
    metric_path(dom, layout, el).map_or(0.0, |path| {
        path.segments().fold(0.0, |total, segment| total + segment.arclen(1e-5))
    })
}

pub fn point_at_length(dom: &Dom, layout: &crate::layout::Layout, el: NodeId, distance: f64) -> Option<kurbo::Point> {
    use kurbo::{ParamCurve, ParamCurveArclen, PathEl};
    let path = metric_path(dom, layout, el)?;
    let PathEl::MoveTo(mut endpoint) = *path.elements().first()? else { return None };
    if distance <= 0.0 {
        return Some(endpoint);
    }
    let mut remaining = distance;
    for segment in path.segments() {
        let length = segment.arclen(1e-5);
        if remaining < length {
            return Some(segment.eval(segment.inv_arclen(remaining, 1e-5)));
        }
        endpoint = segment.end();
        if remaining == length {
            break;
        }
        remaining -= length;
    }
    Some(endpoint)
}

fn metric_path(dom: &Dom, layout: &crate::layout::Layout, el: NodeId) -> Option<kurbo::BezPath> {
    let tag = local_name(dom, el)?;
    let css = geometry_properties(dom, layout, el);
    let property = |name: &str| css.iter().find(|(key, _)| *key == name).map(|(_, value)| value.as_str());
    if tag == "path" {
        if let Some(value) = property("d") {
            let data = value.strip_prefix("path(")?.strip_suffix(')')?.trim().trim_matches('"');
            return Some(parse_shape_path(data));
        }
    }
    let viewport = std::cell::OnceCell::new();
    let font_context = std::cell::OnceCell::new();
    let length = |name: &str| {
        let value = property(name).or_else(|| dom.get_attribute(el, name).ok().flatten())?.trim();
        if let Ok(n) = value.parse::<f64>() {
            return n.is_finite().then_some(n);
        }
        let cx = font_context.get_or_init(|| length_context(dom, layout, el));
        let (px, pct) = crate::computed::resolve_len(value, cx)?;
        if pct == 0.0 {
            return Some(px);
        }
        let (w, h) = (*viewport.get_or_init(|| user_viewport(dom, layout, el)))?;
        let axis = match name {
            "y" | "y1" | "y2" | "cy" | "ry" | "height" => h,
            "r" => w.hypot(h) / 2.0_f64.sqrt(),
            _ => w,
        };
        Some(px + pct * axis / 100.0)
    };
    shape_path(dom, el, &tag, true, length)
}

fn geometry_properties(dom: &Dom, layout: &crate::layout::Layout, el: NodeId) -> Vec<(&'static str, String)> {
    layout.synced_geometry_source(dom)
        .map(|source| source.svg_geometry_properties(crate::snapshot::render_key(el)))
        .unwrap_or_default()
}

fn length_context(dom: &Dom, layout: &crate::layout::Layout, el: NodeId) -> crate::computed::LengthContext {
    let root = dom.node_document(el).ok().and_then(|doc| dom.document_element(doc).ok().flatten());
    crate::computed::LengthContext {
        viewport_width: f64::from(layout.inner_width()),
        viewport_height: f64::from(layout.inner_height()),
        ..crate::computed::LengthContext::fonts(
            layout.style().computed_font_size(dom, Some(el)),
            layout.style().computed_font_size(dom, root),
        )
    }
}

fn user_viewport(dom: &Dom, layout: &crate::layout::Layout, el: NodeId) -> Option<(f64, f64)> {
    let mut ancestor = dom.parent_element(el).ok()?;
    let mut viewports = Vec::new();
    let mut basis = (f64::from(layout.inner_width()), f64::from(layout.inner_height()));
    while let Some(id) = ancestor {
        if local_name(dom, id).as_deref() == Some("svg") {
            if let Some(vb) = dom.get_attribute(id, "viewBox").ok()?.and_then(|v| v.parse::<svgtypes::ViewBox>().ok()) {
                basis = (vb.w, vb.h);
                break;
            }
            viewports.push(id);
        }
        ancestor = dom.parent_element(id).ok()?;
    }
    if ancestor.is_none() && viewports.is_empty() {
        return None;
    }
    for id in viewports.into_iter().rev() {
        // In HTML layout, an inline SVG's percentage width and height are
        // already resolved against its CSS containing block. The content box
        // is the resulting SVG viewport, before transforms or borders.
        if dom.parent_element(id).ok().flatten().is_some_and(|parent| dom.element(parent).is_ok_and(|e| e.is_html())) {
            if let Some(size) = layout.content_size(dom, id) {
                basis = size;
                continue;
            }
        }
        let css = geometry_properties(dom, layout, id);
        let cx = length_context(dom, layout, id);
        let (w, h) = layout.content_size(dom, id).unwrap_or(basis);
        let size = |name: &str, reference: f64, fallback| {
            let value = css.iter().find(|(k, _)| *k == name).map(|(_, v)| v.as_str())
                .or_else(|| dom.get_attribute(id, name).ok().flatten());
            let Some(value) = value else { return fallback };
            value.trim().parse::<f64>().ok().filter(|n| n.is_finite()).or_else(|| {
                crate::computed::resolve_len(value, &cx).map(|(px, pct)| px + pct * reference / 100.0)
            }).unwrap_or(fallback)
        };
        basis = (size("width", basis.0, w), size("height", basis.1, h));
    }
    Some(basis)
}

/// SVG path error handling keeps every complete segment before the first
/// malformed command. The parser expands relative and shorthand commands.
fn parse_shape_path(data: &str) -> kurbo::BezPath {
    use style::values::generics::basic_shape::{ArcSize, ArcSweep, ShapeCommand};
    use style::values::specified::svg_path::{CoordPair, SVGPathData};
    let (data, _) = SVGPathData::parse_bytes(data.as_bytes());
    let mut path = kurbo::BezPath::new();
    let coord = |p: CoordPair| kurbo::Point::new(f64::from(p.x), f64::from(p.y));
    for &command in data.normalize(true).commands() {
        match command {
            ShapeCommand::Move { point } => path.move_to(coord(point.into())),
            ShapeCommand::Line { point } => path.line_to(coord(point.into())),
            ShapeCommand::CubicCurve { point, control1, control2 } => {
                path.curve_to(coord(control1.into()), coord(control2.into()), coord(point.into()));
            }
            ShapeCommand::Arc { point, radii, arc_sweep, arc_size, rotate } => {
                let to = coord(point.into());
                let arc = kurbo::SvgArc {
                    from: path.current_position().unwrap_or_default(), to,
                    radii: kurbo::Vec2::new(f64::from(radii.rx), f64::from(radii.ry.as_ref().copied().unwrap_or(radii.rx))),
                    x_rotation: f64::from(rotate).to_radians(),
                    large_arc: arc_size == ArcSize::Large,
                    sweep: arc_sweep == ArcSweep::Cw,
                };
                if let Some(arc) = kurbo::Arc::from_svg_arc(&arc) {
                    path.extend(arc.append_iter(0.001));
                } else {
                    path.line_to(to);
                }
            }
            ShapeCommand::Close => path.close_path(),
            _ => unreachable!("normalized SVG path contains only M, L, C, A and Z"),
        }
    }
    path
}

fn shape_path(dom: &Dom, el: NodeId, tag: &str, allow_degenerate: bool, length: impl Fn(&str) -> Option<f64>) -> Option<kurbo::BezPath> {
    use kurbo::{Arc, BezPath, Circle, Line, Rect, Shape};
    let v = |name| length(name).unwrap_or(0.0);
    let path = match tag {
        "rect" => {
            let (w, h) = (v("width"), v("height"));
            if w < 0.0 || h < 0.0 || w == 0.0 && h == 0.0 || !allow_degenerate && (w == 0.0 || h == 0.0) {
                return None;
            }
            let rect = Rect::new(v("x"), v("y"), v("x") + w, v("y") + h);
            let rx = length("rx").unwrap_or(v("ry")).clamp(0., w / 2.);
            let ry = length("ry").unwrap_or(v("rx")).clamp(0., h / 2.);
            if rx > 0. && ry > 0. {
                // The SVG equivalent path starts on the top edge after the
                // upper-left arc. That origin matters for distance queries.
                use std::f64::consts::{FRAC_PI_2, PI};
                let mut path = BezPath::new();
                path.move_to((rect.x0 + rx, rect.y0));
                for (edge, center, angle) in [
                    ((rect.x1 - rx, rect.y0), (rect.x1 - rx, rect.y0 + ry), -FRAC_PI_2),
                    ((rect.x1, rect.y1 - ry), (rect.x1 - rx, rect.y1 - ry), 0.0),
                    ((rect.x0 + rx, rect.y1), (rect.x0 + rx, rect.y1 - ry), FRAC_PI_2),
                    ((rect.x0, rect.y0 + ry), (rect.x0 + rx, rect.y0 + ry), PI),
                ] {
                    path.line_to(edge);
                    path.extend(Arc::new(center, (rx, ry), angle, FRAC_PI_2, 0.0).append_iter(0.001));
                }
                path.close_path();
                path
            } else {
                rect.to_path(0.01)
            }
        }
        "circle" if v("r") > 0. => Circle::new((v("cx"), v("cy")), v("r")).to_path(0.01),
        "ellipse" => {
            // Hit testing only supplies the XML lengths. Inferring an auto
            // radius there would also override a CSS radius of zero. Metrics
            // receive the cascade, so can resolve SVG 2's auto radii.
            let (rx, ry) = if allow_degenerate {
                (length("rx").unwrap_or(v("ry")), length("ry").unwrap_or(v("rx")))
            } else {
                (v("rx"), v("ry"))
            };
            if rx < 0.0 || ry < 0.0 || rx == 0.0 && ry == 0.0 || !allow_degenerate && (rx == 0.0 || ry == 0.0) {
                return None;
            }
            if rx == 0.0 || ry == 0.0 {
                let (cx, cy) = (v("cx"), v("cy"));
                let mut path = BezPath::new();
                path.move_to((cx + rx, cy));
                path.line_to((cx, cy + ry));
                path.line_to((cx - rx, cy));
                path.line_to((cx, cy - ry));
                path.close_path();
                return Some(path);
            }
            // An ellipse begins on its positive x-axis even when ry > rx.
            let mut path = Arc::new((v("cx"), v("cy")), (rx, ry), 0.0, std::f64::consts::TAU, 0.0).to_path(0.001);
            path.close_path();
            path
        }
        "line" => Line::new((v("x1"), v("y1")), (v("x2"), v("y2"))).to_path(0.01),
        "path" => parse_shape_path(dom.get_attribute(el, "d").ok()??),
        "polygon" | "polyline" => {
            let mut coords = crate::svg_point_list::parse_points(dom.get_attribute_ns(el, None, "points").ok()??).map(|(x, y)| (f64::from(x), f64::from(y)));
            let start = coords.next()?;
            let mut path = BezPath::new();
            path.move_to(start);
            for pt in coords {
                path.line_to(pt);
            }
            if tag == "polygon" {
                path.close_path();
            }
            path
        }
        _ => return None,
    };
    Some(path)
}

#[cfg(test)]
mod aspect_ratio_tests {
    use super::*;

    #[test]
    fn none_slice_maps_each_axis_independently() {
        let mut dom = Dom::new();
        let doc = dom.create_document(crate::DocumentKind::Html, "about:blank");
        let svg = dom.create_element_ns(doc, Some("http://www.w3.org/2000/svg"), "svg").unwrap();
        dom.set_attribute(svg, "viewBox", "10 20 100 100").unwrap();
        dom.set_attribute(svg, "preserveAspectRatio", "none slice").unwrap();
        assert_eq!(view_box_transform(&dom, svg, (300., 100.)), Some([3., 0., 0., 1., -30., -20.]));
    }
}
