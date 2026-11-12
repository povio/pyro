//! Hostile geometry: a path with points far outside the bitmap (relative to
//! its size) is flattened and clipped to a box that far out. What the canvas
//! shows does not change (a stroke along the box's edge is out of sight:
//! strokes are never wider than a few canvas sizes), and the rasterizer's
//! work, which grows with the size of curves and lines, stays bounded, so a
//! page's `bezierCurveTo(1e300, ...)` finishes. Other paths are drawn as
//! they are, at the cost of one scan of their points.

use kurbo::{BezPath, ParamCurve, PathEl, Point};

/// Points on a curve with a control point out of range (a visible part of
/// such a curve is then drawn as lines between them).
const STEPS: usize = 64;

pub(crate) fn guard_path(path: &BezPath, width: u32, height: u32) -> BezPath {
    let limit = 1e6 + 64.0 * f64::from(width.max(height));
    let out_of_range = |p: &Point| !(p.x.abs() <= limit && p.y.abs() <= limit);
    let huge = path.elements().iter().any(|el| match el {
        PathEl::MoveTo(p) | PathEl::LineTo(p) => out_of_range(p),
        PathEl::QuadTo(a, b) => out_of_range(a) || out_of_range(b),
        PathEl::CurveTo(a, b, c) => out_of_range(a) || out_of_range(b) || out_of_range(c),
        PathEl::ClosePath => false,
    });
    if !huge {
        return path.clone();
    }
    // Points beyond any useful range keep their direction from the origin.
    let tame = |p: Point| {
        let m = p.x.abs().max(p.y.abs());
        if m > 1e200 && m.is_finite() {
            Point::new(p.x / m * 1e200, p.y / m * 1e200)
        } else {
            p
        }
    };
    // Subpaths as polylines: curves within range flattened finely, a curve
    // with a point out of range through STEPS points on it.
    let mut subpaths: Vec<(Vec<Point>, bool)> = Vec::new();
    let mut last = Point::ZERO;
    let mut start = Point::ZERO;
    for el in path.elements() {
        match *el {
            PathEl::MoveTo(p) => {
                subpaths.push((vec![tame(p)], false));
                last = p;
                start = p;
            }
            PathEl::LineTo(p) => {
                if subpaths.is_empty() {
                    subpaths.push((vec![tame(last)], false));
                }
                subpaths.last_mut().expect("subpath").0.push(tame(p));
                last = p;
            }
            PathEl::QuadTo(a, b) => {
                if let Some((points, _)) = subpaths.last_mut() {
                    if out_of_range(&last) || out_of_range(&a) || out_of_range(&b) {
                        let q = kurbo::QuadBez::new(last, a, b);
                        points.extend((1..=STEPS).map(|i| tame(q.eval(i as f64 / STEPS as f64))));
                    } else {
                        flatten_into(points, [PathEl::MoveTo(last), PathEl::QuadTo(a, b)]);
                    }
                }
                last = b;
            }
            PathEl::CurveTo(a, b, c) => {
                if let Some((points, _)) = subpaths.last_mut() {
                    if out_of_range(&last) || out_of_range(&a) || out_of_range(&b) || out_of_range(&c) {
                        let cubic = kurbo::CubicBez::new(last, a, b, c);
                        points.extend((1..=STEPS).map(|i| tame(cubic.eval(i as f64 / STEPS as f64))));
                    } else {
                        flatten_into(points, [PathEl::MoveTo(last), PathEl::CurveTo(a, b, c)]);
                    }
                }
                last = c;
            }
            PathEl::ClosePath => {
                if let Some(s) = subpaths.last_mut() {
                    s.1 = true;
                }
                last = start;
            }
        }
    }
    let mut out = BezPath::new();
    for (points, closed) in subpaths {
        if points.iter().any(|p| !p.x.is_finite() || !p.y.is_finite()) {
            continue;
        }
        let clipped = clip(points, closed, limit);
        let Some((first, rest)) = clipped.split_first() else { continue };
        out.move_to(*first);
        for p in rest {
            out.line_to(*p);
        }
        if closed {
            out.close_path();
        }
    }
    out
}

fn flatten_into(points: &mut Vec<Point>, curve: [PathEl; 2]) {
    kurbo::flatten(curve, 0.05, |el| {
        if let PathEl::LineTo(p) = el {
            points.push(p);
        }
    });
}

/// Sutherland-Hodgman: `points` clipped to the box `[-limit, limit]`², as a
/// polygon (`closed`) or as a polyline whose runs outside follow the box's
/// edge.
fn clip(mut points: Vec<Point>, closed: bool, limit: f64) -> Vec<Point> {
    // x <= limit, x >= -limit, y <= limit, y >= -limit.
    for (axis, sign) in [(0, 1.0), (0, -1.0), (1, 1.0), (1, -1.0)] {
        if points.is_empty() {
            break;
        }
        let coord = |p: &Point| if axis == 0 { p.x } else { p.y };
        let inside = |p: &Point| sign * coord(p) <= limit;
        let cross = |a: Point, b: Point| {
            let t = (sign * limit - coord(&a)) / (coord(&b) - coord(&a));
            a.lerp(b, t)
        };
        let n = points.len();
        let mut out = Vec::with_capacity(n + 4);
        if !closed && inside(&points[0]) {
            out.push(points[0]);
        }
        let segments = if closed { n } else { n - 1 };
        for i in 0..segments {
            let (a, b) = if closed { (points[(i + n - 1) % n], points[i]) } else { (points[i], points[i + 1]) };
            match (inside(&a), inside(&b)) {
                (true, true) => out.push(b),
                (true, false) => out.push(cross(a, b)),
                (false, true) => {
                    out.push(cross(a, b));
                    out.push(b);
                }
                (false, false) => {}
            }
        }
        points = out;
    }
    points
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn far_lines_keep_their_visible_direction() {
        let mut path = BezPath::new();
        path.move_to((10.0, 10.0));
        path.line_to((1e9, 5e8 + 10.0));
        let out = guard_path(&path, 100, 100);
        let els = out.elements();
        let (PathEl::MoveTo(a), PathEl::LineTo(b)) = (els[0], els[1]) else { panic!("{els:?}") };
        assert_eq!(a, Point::new(10.0, 10.0));
        let slope = (b.y - a.y) / (b.x - a.x);
        assert!((slope - 5e8 / (1e9 - 10.0)).abs() < 1e-9, "{slope}");
        assert!(b.x <= 1e6 + 6400.0 + 1e-6);
    }
}
