use kurbo::Rect;

pub const MIN_W: f64 = 1e-6;
pub const PROJECT_LIMIT: f64 = 1e5;

/// The projection of `rect` through the homography `h`, cut at the viewer's plane.
pub fn project(h: &[f64; 9], rect: Rect) -> Vec<(f64, f64)> {
    let corners = [(rect.x0, rect.y0), (rect.x1, rect.y0), (rect.x1, rect.y1), (rect.x0, rect.y1)];
    let homogeneous: Vec<[f64; 4]> = corners
        .iter()
        .map(|&(x, y)| {
            let [a, b, c, d, e, f, g, i, j] = *h;
            [a * x + b * y + c, d * x + e * y + f, 0.0, g * x + i * y + j]
        })
        .collect();
    clip_w(&homogeneous).into_iter().map(|p| (p[0], p[1])).collect()
}

/// A polygon of homogeneous points cut at the viewer's plane (W below [`MIN_W`]) and divided by W.
pub fn clip_w(points: &[[f64; 4]]) -> Vec<[f64; 3]> {
    let mut out = Vec::with_capacity(points.len() + 2);
    for i in 0..points.len() {
        let (a, b) = (points[i], points[(i + 1) % points.len()]);
        let (a_in, b_in) = (a[3] >= MIN_W, b[3] >= MIN_W);
        if a_in {
            out.push([a[0] / a[3], a[1] / a[3], a[2] / a[3]]);
        }
        if a_in != b_in {
            let t = (MIN_W - a[3]) / (b[3] - a[3]);
            let p: [f64; 4] = std::array::from_fn(|k| a[k] + (b[k] - a[k]) * t);
            out.push([p[0] / MIN_W, p[1] / MIN_W, p[2] / MIN_W]);
        }
    }
    out
}
