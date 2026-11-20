use kurbo::Rect;

pub const MIN_W: f64 = 1e-6;
pub const PROJECT_LIMIT: f64 = 1e5;

/// The projection of `rect` through the homography `h`, cut at the viewer's plane.
pub fn project(h: &[f64; 9], rect: Rect) -> Vec<(f64, f64)> {
    let Some(points) = homogeneous_corners(h, rect) else { return Vec::new() };
    clip_w(&points).into_iter().map(|p| (p[0], p[1])).collect()
}

pub fn layout_bounds(h: &[f64; 9], rect: Rect) -> Option<Rect> {
    let points = homogeneous_corners(h, rect)?;
    let mut bounds: Option<Rect> = None;
    for [x, y, _, w] in points {
        if w <= 0.0 {
            continue;
        }
        let point = kurbo::Point::new(x / w, y / w);
        if !point.is_finite() {
            return None;
        }
        bounds = Some(bounds.map_or_else(|| Rect::from_points(point, point), |rect| rect.union_pt(point)));
    }
    bounds
}

fn homogeneous_corners(h: &[f64; 9], rect: Rect) -> Option<[[f64; 4]; 4]> {
    if !rect.is_finite() || !h.iter().all(|v| v.is_finite()) {
        return None;
    }
    let corners = [(rect.x0, rect.y0), (rect.x1, rect.y0), (rect.x1, rect.y1), (rect.x0, rect.y1)];
    let homogeneous = corners.map(|(x, y)| {
        let [a, b, c, d, e, f, g, i, j] = *h;
        [a * x + b * y + c, d * x + e * y + f, 0.0, g * x + i * y + j]
    });
    homogeneous.iter().flatten().all(|v| v.is_finite()).then_some(homogeneous)
}

/// A polygon of homogeneous points cut at the viewer's plane (W below [`MIN_W`]) and divided by W.
pub fn clip_w(points: &[[f64; 4]]) -> Vec<[f64; 3]> {
    if !points.iter().flatten().all(|v| v.is_finite()) {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(points.len() + 2);
    for i in 0..points.len() {
        let (a, b) = (points[i], points[(i + 1) % points.len()]);
        let (a_in, b_in) = (a[3] >= MIN_W, b[3] >= MIN_W);
        if a_in {
            out.push([a[0] / a[3], a[1] / a[3], a[2] / a[3]]);
        }
        if a_in != b_in {
            let span = b[3] - a[3];
            if !span.is_finite() {
                return Vec::new();
            }
            let t = (MIN_W - a[3]) / span;
            let p: [f64; 4] = std::array::from_fn(|k| a[k] + (b[k] - a[k]) * t);
            out.push([p[0] / MIN_W, p[1] / MIN_W, p[2] / MIN_W]);
        }
    }
    if out.iter().flatten().all(|v| v.is_finite()) { out } else { Vec::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_requires_finite_coordinates() {
        let rect = Rect::new(0.0, 0.0, 2.0, 2.0);
        let matrix = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.5, 1.0];
        assert_eq!(project(&matrix, rect).len(), 4);
        for coefficient in [1e200_f64 * 1e200, f64::NAN, f64::MAX] {
            let mut invalid = matrix;
            invalid[0] = coefficient;
            assert!(project(&invalid, rect).is_empty());
        }
        assert!(clip_w(&[[f64::MAX, 0.0, 0.0, MIN_W]]).is_empty());
        assert!(clip_w(&[[0.0, 1.0, 0.0, f64::MAX], [1.0, 0.0, 0.0, -f64::MAX]]).is_empty());
    }
}
