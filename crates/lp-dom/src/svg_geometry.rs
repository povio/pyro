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
            let points = dom.get_attribute(el, "points").ok().flatten()?;
            let nums = numbers(points);
            let mut b: Option<Bounds> = None;
            for p in nums.as_chunks::<2>().0 {
                b = Some(Bounds::union(b, Bounds::point(p[0], p[1])));
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
    let weight = style
        .used_value(dom, el, "font-weight")
        .and_then(|v| v.trim().parse::<f64>().ok())
        .map_or(400, |w| w.clamp(1.0, 1000.0) as u16);
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

fn multiply(m: [f64; 6], n: [f64; 6]) -> [f64; 6] {
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
