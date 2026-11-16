//! pyro: corner shapes (CSS Borders 4 `corner-shape`, `superellipse()`), drawn as Blink draws them
//! (`ContouredRect` and `ContouredRect::Corner` in `platform/geometry/contoured_rect.cc`,
//! `PathBuilder::AddContouredRect` and `CornerClipOutPath` in `platform/geometry/path_builder.cc`,
//! `ComputeContouredBorderFromStyle` in `core/paint/contoured_border_geometry.cc`).
//!
//! A corner is the quarter of a superellipse of exponent `2^k` in the box its radii make
//! (`k` = 1 is round, 0 a bevel, -1 a scoop, ±∞ square and notch). The border box's own contour
//! follows the corners as given. A box inset or outset from it (the padding box, a shadow, an
//! outline) keeps its curves aligned with the border box's, so that a border looks as thick along
//! the curve as along the sides: it is its rectangle less each corner's "clip-out" region, the
//! area outside a curve moved inwards along its normals by the inset (Blink takes the regions out
//! with Skia's path ops; [`boolean`] does it on flattened outlines here).

use std::f64::consts::{FRAC_1_SQRT_2, SQRT_2};

use kurbo::{BezPath, PathEl, Point, Rect, Shape as _, Vec2};

use super::NonUniformRoundedRectRadii;

/// The exponents of Blink's `ContouredRect::CornerCurvature`.
pub const ROUND: f64 = 2.0;
pub const BEVEL: f64 = 1.0;
pub const STRAIGHT: f64 = 1000.0;
pub const NOTCH: f64 = 1.0 / STRAIGHT;
/// `Superellipse::kHighCurvatureThreshold`: parameters beyond ±16 draw as ±16.
const HIGH_CURVATURE: f64 = 16.0;
/// How finely corner curves are flattened (device px). vello_cpu flattens the curves it fills to
/// within 0.25px (`vello_common::flatten::TOL`), which leaves a curve's antialiased pixels up
/// to a quarter of their range off where Skia draws them; a corner-shaped edge is drawn as the
/// polygon flattened here instead, as close to the curve as Chromium draws it.
const FLATTEN_TOLERANCE: f64 = 0.02;

/// The exponent of a corner whose `superellipse()` parameter is `k` (`Superellipse::Exponent`):
/// `2^k`, `k` clamped to ±16.
pub fn exponent(k: f64) -> f64 {
    if k.is_nan() {
        return ROUND;
    }
    // An interpolated keyword (`round` to `round`) comes back a hair off its value.
    let k = if (k - k.round()).abs() < 1e-4 { k.round() } else { k };
    2f64.powf(k.clamp(-HIGH_CURVATURE, HIGH_CURVATURE))
}

/// Whether a corner of parameter `k` has no radius at all (`Superellipse::IsDegenerate`: square,
/// or so close to it that nothing would show).
pub fn is_degenerate(k: f64) -> bool {
    k >= HIGH_CURVATURE
}

fn clamp_curvature(curvature: f64) -> f64 {
    curvature.clamp(NOTCH, STRAIGHT)
}

/// The exponent of each corner of a box (`ContouredRect::CornerCurvature`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CornerCurvature {
    pub top_left: f64,
    pub top_right: f64,
    pub bottom_right: f64,
    pub bottom_left: f64,
}

impl CornerCurvature {
    pub const ROUND: Self = Self {
        top_left: ROUND,
        top_right: ROUND,
        bottom_right: ROUND,
        bottom_left: ROUND,
    };

    pub fn is_round(&self) -> bool {
        *self == Self::ROUND
    }

    pub fn is_convex(&self) -> bool {
        [self.top_left, self.top_right, self.bottom_right, self.bottom_left]
            .iter()
            .all(|&c| c >= BEVEL)
    }
}

fn normalize(v: Vec2) -> Vec2 {
    let length = v.hypot();
    if length == 0.0 || !length.is_finite() {
        v
    } else {
        v / length
    }
}

fn cross(a: Vec2, b: Vec2) -> f64 {
    a.x * b.y - a.y * b.x
}

/// The intersection of the lines through `a0`, `a1` and through `b0`, `b1`.
fn line_intersection(a0: Point, a1: Point, b0: Point, b1: Point) -> Option<Point> {
    let (a, b) = (a1 - a0, b1 - b0);
    let denominator = cross(a, b);
    if denominator.abs() < 1e-6 {
        return None;
    }
    Some(a0 + a * (cross(b0 - a0, b) / denominator))
}

/// The intersection of the segment `a0`-`a1` with the line through `b0`, `b1`, or of the
/// segment `b0`-`b1` with the line through `a0`, `a1` (Blink's `SegmentLineIntersection`).
fn segment_line_intersection(a0: Point, a1: Point, b0: Point, b1: Point) -> Option<Point> {
    let (a, b) = (a1 - a0, b1 - b0);
    let denominator = cross(a, b);
    if denominator.abs() < 1e-6 {
        return None;
    }
    let offset = b0 - a0;
    let a_scale = cross(offset, b) / denominator;
    if (0.0..=1.0).contains(&a_scale) {
        return Some(a0 + a * a_scale);
    }
    let b_scale = cross(offset, a) / denominator;
    if (0.0..=1.0).contains(&b_scale) {
        return Some(b0 + b * b_scale);
    }
    None
}

/// One corner of a contour: an axis-aligned quad whose points are, clockwise, where the curve
/// starts, the box's corner, where it ends, and the corner's centre (Blink's
/// `ContouredRect::Corner`).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Corner {
    v: [Point; 4],
    curvature: f64,
}

impl Corner {
    /// The corner of `rect` (a corner's box) turned `rotation` quarters: 0 top right, 1 bottom
    /// right, 2 bottom left, 3 top left.
    fn from_rect(rect: Rect, rotation: usize, curvature: f64) -> Self {
        let mut v = [
            Point::new(rect.x0, rect.y0),
            Point::new(rect.x1, rect.y0),
            Point::new(rect.x1, rect.y1),
            Point::new(rect.x0, rect.y1),
        ];
        v.rotate_left(rotation);
        Self { v, curvature }
    }

    fn new(v: [Point; 4], curvature: f64) -> Self {
        Self {
            v,
            curvature: clamp_curvature(curvature),
        }
    }

    fn start(&self) -> Point {
        self.v[0]
    }
    fn outer(&self) -> Point {
        self.v[1]
    }
    fn end(&self) -> Point {
        self.v[2]
    }
    fn center(&self) -> Point {
        self.v[3]
    }
    fn v1(&self) -> Vec2 {
        self.outer() - self.start()
    }
    fn v2(&self) -> Vec2 {
        self.end() - self.outer()
    }
    fn v3(&self) -> Vec2 {
        self.center() - self.end()
    }
    fn v4(&self) -> Vec2 {
        self.start() - self.center()
    }
    fn is_straight(&self) -> bool {
        self.curvature == STRAIGHT
    }
    fn is_bevel(&self) -> bool {
        self.curvature == BEVEL
    }
    fn is_round(&self) -> bool {
        self.curvature == ROUND
    }
    fn is_notch(&self) -> bool {
        self.curvature == NOTCH
    }
    fn is_concave(&self) -> bool {
        self.curvature < 1.0
    }
    fn is_empty(&self) -> bool {
        self.v1().hypot() == 0.0 || self.v2().hypot() == 0.0
    }

    /// The corner bulging the other way (a scoop for a round corner).
    fn inverse(&self) -> Self {
        Self::new(
            [self.start(), self.center(), self.end(), self.outer()],
            1.0 / self.curvature,
        )
    }

    fn bounding_box(&self) -> Rect {
        Rect::from_points(self.start(), self.end())
    }

    /// A point of the unit corner (`(0, 1)` the start, `(1, 0)` the end, `(1, 1)` the outer
    /// corner) in this corner.
    fn map_point(&self, x: f64, y: f64) -> Point {
        self.center() + self.v1() * x + self.v4() * y
    }

    fn half_corner(&self) -> Point {
        let h = half_corner_for_curvature(self.curvature);
        self.map_point(h, h)
    }

    /// This corner (of an inset or outset box) moved so that its curve stays parallel to
    /// `origin`'s, the corner it was inset from: `ContouredRect::Corner::AlignedToOrigin`.
    fn aligned_to_origin(&self, origin: &Corner, edge_inset_start: f64, edge_inset_end: f64) -> Corner {
        if origin.is_empty() || self == origin {
            return *self;
        }
        let start_radius = origin.v2().hypot();
        let end_radius = origin.v3().hypot();
        // Insets beyond the radius are kept (the inset box's radii stopped at zero).
        let start_inset = if edge_inset_start >= 0.0 && self.v2().hypot() == 0.0 {
            edge_inset_start
        } else {
            start_radius - self.v2().hypot()
        };
        let end_inset = if edge_inset_end >= 0.0 && self.v3().hypot() == 0.0 {
            edge_inset_end
        } else {
            end_radius - self.v3().hypot()
        };
        let parameter = origin.curvature.log2();
        let exponent = 2f64.powf(parameter.abs());
        let convex_half_corner = 0.5f64.powf(1.0 / exponent);
        let half_corner = if parameter < 0.0 {
            1.0 - convex_half_corner
        } else {
            convex_half_corner
        };
        let control_point_x = (half_corner / (SQRT_2 - 1.0) - FRAC_1_SQRT_2).clamp(0.0, 1.0);
        let inset_difference = (end_inset - start_inset).clamp(-start_radius, end_radius);

        let mut start_control_point_x = control_point_x;
        let mut end_control_point_x = control_point_x;
        if inset_difference != 0.0 {
            let bevel_normal_delta = (start_radius * start_radius + end_radius * end_radius
                - inset_difference * inset_difference)
                .max(0.0)
                .sqrt();
            let bevel_normal_x = end_radius * inset_difference + start_radius * bevel_normal_delta;
            let bevel_normal_y = -start_radius * inset_difference + end_radius * bevel_normal_delta;
            let bevel_control_point_x = start_radius * bevel_normal_y
                / (start_radius * bevel_normal_y + end_radius * bevel_normal_x);
            start_control_point_x = if parameter < 0.0 {
                bevel_control_point_x * (2.0 * control_point_x)
            } else {
                1.0 - (1.0 - bevel_control_point_x) * (2.0 * (1.0 - control_point_x))
            };
            end_control_point_x = 2.0 * control_point_x - start_control_point_x;
        }

        let unmapped_start_normal = normalize(Vec2::new(
            (1.0 - start_control_point_x) * start_radius,
            start_control_point_x * end_radius,
        ));
        let unmapped_end_normal = normalize(Vec2::new(
            end_control_point_x * start_radius,
            (1.0 - end_control_point_x) * end_radius,
        ));
        let v3 = normalize(origin.v3());
        let v2 = normalize(origin.v2());
        let start_normal = v3 * unmapped_start_normal.x + v2 * unmapped_start_normal.y;
        let end_normal = v3 * unmapped_end_normal.x + v2 * unmapped_end_normal.y;

        let original_outer = self.outer() - v3 * end_inset - v2 * start_inset;
        let mut adjusted_start = original_outer + v3 * end_radius + start_normal * start_inset;
        let mut adjusted_end = original_outer + v2 * start_radius + end_normal * end_inset;

        let start_tangent = Vec2::new(-start_normal.y, start_normal.x);
        let end_tangent = Vec2::new(-end_normal.y, end_normal.x);
        if parameter >= 0.0 && start_inset < 0.0 {
            adjusted_start = line_intersection(
                adjusted_start,
                adjusted_start - start_tangent,
                self.outer() + v3,
                self.outer(),
            )
            .unwrap_or(adjusted_start);
        }
        if parameter >= 0.0 && end_inset < 0.0 {
            adjusted_end = line_intersection(
                adjusted_end,
                adjusted_end + end_tangent,
                self.outer() + v2,
                self.outer(),
            )
            .unwrap_or(adjusted_end);
        }

        let adjusted_height = (adjusted_end - adjusted_start).dot(v2);
        let adjusted_outer = adjusted_end - v2 * adjusted_height;
        let adjusted_center = adjusted_start + v2 * adjusted_height;
        if parameter <= -1.0 || parameter >= 0.0 {
            return Corner::new(
                [adjusted_start, adjusted_outer, adjusted_end, adjusted_center],
                origin.curvature,
            );
        }

        // Between a scoop and a bevel the aligned curve is the ellipse arc tangent to the
        // normals' tangents at both ends.
        let tangent_intersection = line_intersection(
            adjusted_start,
            adjusted_start - start_tangent,
            adjusted_end,
            adjusted_end + end_tangent,
        )
        .unwrap_or(adjusted_start);
        Corner::new(
            [
                adjusted_start,
                tangent_intersection,
                adjusted_end,
                adjusted_start + (adjusted_end - tangent_intersection),
            ],
            ROUND,
        )
    }
}

/// The x (and y) of a unit superellipse corner at its middle (`HalfCornerForCurvature`).
fn half_corner_for_curvature(curvature: f64) -> f64 {
    0.5f64.powf(1.0 / clamp_curvature(curvature))
}

/// The cubic Bézier that follows half of a convex superellipse corner of exponent `curvature`,
/// from its start `(0, 1)` to its middle (`ApproximateSuperellipseHalfCornerAsBezierCurve`):
/// the two control points and the end.
fn half_corner_cubic(curvature: f64) -> [Vec2; 3] {
    const P: [f64; 7] = [
        1.2430920942724248,
        2.010479023614843,
        0.32922901179443753,
        0.2823023142212073,
        1.3473704261055421,
        2.9149468637949814,
        0.9106507102917086,
    ];
    let s = curvature.log2();
    let slope = P[0] + (P[6] - P[0]) * 0.5 * (1.0 + (P[5] * (s - P[1])).tanh());
    let base = 1.0 / (1.0 + (-slope * (0.0 - P[1])).exp());
    let logistic = 1.0 / (1.0 + (-slope * (s - P[1])).exp());
    let a = (logistic - base) / (1.0 - base);
    let b = P[2] * (-P[3] * s.powf(P[4])).exp();
    let half_corner = half_corner_for_curvature(curvature);
    [
        Vec2::new(a, 1.0),
        Vec2::new(half_corner - b, half_corner + b),
        Vec2::new(half_corner, half_corner),
    ]
}

/// The kappa of a cubic Bézier quarter ellipse (the conic Skia draws a round corner with).
const KAPPA: f64 = 0.552_284_749_830_793_4;

/// Draws `corner`'s curve: a line to its start, then the curve to its end (`AddCurvedCorner`).
fn add_curved_corner(path: &mut BezPath, corner: &Corner) {
    if corner.is_concave() {
        add_curved_corner(path, &corner.inverse());
        return;
    }
    line_or_move_to(path, corner.start());
    if corner.is_straight() || corner.is_empty() {
        path.line_to(corner.outer());
        path.line_to(corner.end());
    } else if corner.is_bevel() {
        path.line_to(corner.end());
    } else if corner.is_round() {
        let (start, outer, end) = (corner.start(), corner.outer(), corner.end());
        path.curve_to(
            start + (outer - start) * KAPPA,
            end + (outer - end) * KAPPA,
            end,
        );
    } else {
        let [p1, p2, p3] = half_corner_cubic(corner.curvature);
        path.curve_to(
            corner.map_point(p1.x, p1.y),
            corner.map_point(p2.x, p2.y),
            corner.map_point(p3.x, p3.y),
        );
        path.curve_to(
            corner.map_point(p2.y, p2.x),
            corner.map_point(p1.y, p1.x),
            corner.end(),
        );
    }
}

fn line_or_move_to(path: &mut BezPath, point: Point) {
    if path.elements().is_empty() || matches!(path.elements().last(), Some(PathEl::ClosePath)) {
        path.move_to(point);
    } else {
        path.line_to(point);
    }
}

/// The region outside an inset corner's curve, aligned with its origin (`CornerClipOutPath`):
/// what is taken out of the inset box's rectangle at that corner. `None` when nothing is.
fn corner_clip_out_path(
    origin: &Corner,
    target: &Corner,
    edge_inset_start: f64,
    edge_inset_end: f64,
) -> Option<BezPath> {
    let start_radius = origin.v2().hypot();
    let end_radius = origin.v3().hypot();
    if start_radius == 0.0 || end_radius == 0.0 || origin.is_straight() {
        return None;
    }
    let start_inset = if edge_inset_start >= 0.0 && target.v2().hypot() == 0.0 {
        edge_inset_start
    } else {
        start_radius - target.v2().hypot()
    };
    let end_inset = if edge_inset_end >= 0.0 && target.v3().hypot() == 0.0 {
        edge_inset_end
    } else {
        end_radius - target.v3().hypot()
    };
    let parameter = origin.curvature.log2();
    let inset_difference = (end_inset - start_inset).clamp(-start_radius, end_radius);
    if parameter <= 0.0 && (inset_difference == -start_radius || inset_difference == end_radius) {
        return None;
    }

    let aligned = target.aligned_to_origin(origin, edge_inset_start, edge_inset_end);
    let v3 = normalize(origin.v3());
    let v2 = normalize(origin.v2());
    let original_outer = target.outer() - v3 * end_inset - v2 * start_inset;
    let original_start = original_outer + v3 * end_radius;
    let original_end = original_outer + v2 * start_radius;

    let mut miter_start = aligned.start();
    let mut miter_end = aligned.end();
    if parameter < 0.0 && start_inset < 0.0 {
        let start_normal = normalize((aligned.start() - original_start) * (1.0 / start_inset));
        let start_tangent = Vec2::new(-start_normal.y, start_normal.x);
        miter_start = line_intersection(
            aligned.start(),
            aligned.start() - start_tangent,
            target.outer() + v3,
            target.outer(),
        )
        .unwrap_or(aligned.start());
    }
    if parameter < 0.0 && end_inset < 0.0 {
        let end_normal = normalize((aligned.end() - original_end) * (1.0 / end_inset));
        let end_tangent = Vec2::new(-end_normal.y, end_normal.x);
        miter_end = line_intersection(
            aligned.end(),
            aligned.end() + end_tangent,
            target.outer() + v2,
            target.outer(),
        )
        .unwrap_or(aligned.end());
    }

    let self_intersection = if parameter < 0.0
        && start_inset < 0.0
        && end_inset < 0.0
        && (-end_inset >= start_radius || -start_inset >= end_radius)
    {
        segment_line_intersection(miter_start, aligned.start(), miter_end, aligned.end())
    } else {
        None
    };

    let mut path = BezPath::new();
    path.move_to(miter_start);
    if let Some(point) = self_intersection {
        path.line_to(point);
    } else if origin.is_notch() {
        path.line_to(aligned.center());
    } else {
        add_curved_corner(&mut path, &aligned);
    }
    path.line_to(miter_end);
    path.line_to(target.outer());
    path.close_path();
    Some(path)
}

/// The box of each corner of a rounded rect, as Blink's `FloatRoundedRect::*Corner()`, in the
/// order top right, bottom right, bottom left, top left.
fn corner_rects(rect: Rect, radii: &NonUniformRoundedRectRadii) -> [Rect; 4] {
    let (tl, tr, br, bl) = (radii.top_left, radii.top_right, radii.bottom_right, radii.bottom_left);
    [
        Rect::new(rect.x1 - tr.x, rect.y0, rect.x1, rect.y0 + tr.y),
        Rect::new(rect.x1 - br.x, rect.y1 - br.y, rect.x1, rect.y1),
        Rect::new(rect.x0, rect.y1 - bl.y, rect.x0 + bl.x, rect.y1),
        Rect::new(rect.x0, rect.y0, rect.x0 + tl.x, rect.y0 + tl.y),
    ]
}

/// A box with shaped corners, possibly inset or outset from another (`ContouredRect`).
#[derive(Clone, Debug, PartialEq)]
pub struct ContouredRect {
    pub rect: Rect,
    pub radii: NonUniformRoundedRectRadii,
    pub curvature: CornerCurvature,
    /// The box (usually the border box) this one was inset or outset from, with its radii:
    /// the corners' curves stay parallel to its.
    pub origin: Option<(Rect, NonUniformRoundedRectRadii)>,
}

impl ContouredRect {
    fn curvatures(&self) -> [f64; 4] {
        let c = &self.curvature;
        [c.top_right, c.bottom_right, c.bottom_left, c.top_left]
    }

    fn corners_of(&self, rect: Rect, radii: &NonUniformRoundedRectRadii) -> [Corner; 4] {
        let rects = corner_rects(rect, radii);
        let curvatures = self.curvatures();
        std::array::from_fn(|i| Corner::from_rect(rects[i], i, curvatures[i]))
    }

    /// The path of the contour.
    pub fn path(&self) -> BezPath {
        let rect = Rect::new(
            self.rect.x0,
            self.rect.y0,
            self.rect.x1.max(self.rect.x0),
            self.rect.y1.max(self.rect.y0),
        );
        let identity = match &self.origin {
            None => true,
            Some((origin, radii)) => *origin == rect && *radii == self.radii,
        };
        let targets = self.corners_of(rect, &self.radii);
        if identity {
            let mut path = BezPath::new();
            path.move_to(targets[0].start());
            for corner in &targets {
                add_curved_corner(&mut path, corner);
            }
            path.close_path();
            return polygons_path(&flatten(&path));
        }
        let (origin_rect, origin_radii) = self.origin.as_ref().unwrap();
        let origins = self.corners_of(*origin_rect, origin_radii);
        let insets = [
            (rect.y0 - origin_rect.y0, origin_rect.x1 - rect.x1),
            (origin_rect.x1 - rect.x1, origin_rect.y1 - rect.y1),
            (origin_rect.y1 - rect.y1, rect.x0 - origin_rect.x0),
            (rect.x0 - origin_rect.x0, rect.y0 - origin_rect.y0),
        ];
        let cuts: Vec<BezPath> = (0..4)
            .filter_map(|i| corner_clip_out_path(&origins[i], &targets[i], insets[i].0, insets[i].1))
            .collect();
        if cuts.is_empty() {
            return rect.to_path(0.1);
        }
        let mut shapes = vec![vec![rect_polygon(rect)]];
        shapes.extend(cuts.iter().map(flatten));
        boolean(&shapes, |inside| inside[0] && !inside[1..].iter().any(|&b| b))
    }
}

/// `rect`'s corners, clockwise from the top left.
pub fn rect_polygon(rect: Rect) -> Vec<Point> {
    vec![
        Point::new(rect.x0, rect.y0),
        Point::new(rect.x1, rect.y0),
        Point::new(rect.x1, rect.y1),
        Point::new(rect.x0, rect.y1),
    ]
}

/// A path of closed `polygons`.
pub fn polygons_path(polygons: &[Vec<Point>]) -> BezPath {
    let mut path = BezPath::new();
    for polygon in polygons {
        let Some((first, rest)) = polygon.split_first() else {
            continue;
        };
        path.move_to(*first);
        for p in rest {
            path.line_to(*p);
        }
        path.close_path();
    }
    path
}

/// Every contour of `path` as a polygon, its curves flattened.
pub fn flatten(path: &BezPath) -> Vec<Vec<Point>> {
    flatten_with(path, FLATTEN_TOLERANCE)
}

/// Every contour of `path` as a polygon, its curves flattened to within `tolerance`.
pub fn flatten_with(path: &BezPath, tolerance: f64) -> Vec<Vec<Point>> {
    let mut polygons = Vec::new();
    let mut current: Vec<Point> = Vec::new();
    kurbo::flatten(path.elements().iter().copied(), tolerance, |el| match el {
        PathEl::MoveTo(p) => {
            if current.len() > 2 {
                polygons.push(std::mem::take(&mut current));
            }
            current.clear();
            current.push(p);
        }
        PathEl::LineTo(p) => {
            if current.last() != Some(&p) {
                current.push(p);
            }
        }
        PathEl::ClosePath => {
            if current.len() > 2 {
                polygons.push(std::mem::take(&mut current));
            }
            current.clear();
        }
        _ => {}
    });
    if current.len() > 2 {
        polygons.push(current);
    }
    polygons
}

/// The opposite-corner constraint for boxes with concave corners
/// (`RadiiConstraintFactorForOppositeCorners`): the scale of the radii at which the hulls of two
/// opposite corners touch without overlapping, 1 when they do not overlap.
fn opposite_corners_constraint(a: &Corner, b: &Corner) -> f64 {
    if !rects_intersect(a.bounding_box(), b.bounding_box()) {
        return 1.0;
    }
    let (hull_a, hull_b) = (hull_quad(a), hull_quad(b));
    if !quads_intersect(&hull_a, &hull_b) {
        return 1.0;
    }
    const NEAR_TOUCHING: f64 = 1.0 - 5.0 * f32::EPSILON as f64;
    if !quads_intersect(
        &scale_quad(&hull_a, a.outer(), NEAR_TOUCHING),
        &scale_quad(&hull_b, b.outer(), NEAR_TOUCHING),
    ) {
        return 1.0;
    }
    let (mut min, mut max) = (0.0f64, 1.0f64);
    while max - min > 0.05 {
        let scale = (min + max) / 2.0;
        if quads_intersect(&scale_quad(&hull_a, a.outer(), scale), &scale_quad(&hull_b, b.outer(), scale)) {
            max = scale;
        } else {
            min = scale;
        }
    }
    min
}

/// Blink's `gfx::RectF::Intersects`: overlapping with a positive area.
fn rects_intersect(a: Rect, b: Rect) -> bool {
    a.x0 < b.x1 && b.x0 < a.x1 && a.y0 < b.y1 && b.y0 < a.y1
}

fn hull_quad(corner: &Corner) -> [Point; 4] {
    let half_corner = corner.half_corner();
    let d = half_corner - corner.outer();
    // `LineF::Normal()`: the direction turned a quarter.
    let normal = Vec2::new(-d.y, d.x);
    let perpendicular = half_corner + normal;
    let i1 = line_intersection(half_corner, perpendicular, corner.start(), corner.center())
        .unwrap_or(corner.center());
    let i2 = line_intersection(half_corner, perpendicular, corner.end(), corner.center())
        .unwrap_or(corner.center());
    [corner.start(), i1, i2, corner.end()]
}

fn scale_quad(quad: &[Point; 4], origin: Point, scale: f64) -> [Point; 4] {
    quad.map(|p| origin + (p - origin) * scale)
}

/// `gfx::QuadF::IntersectsQuad`: neither quad is wholly outside one edge of the other.
fn quads_intersect(a: &[Point; 4], b: &[Point; 4]) -> bool {
    !fully_outside_one_edge(a, b) && !fully_outside_one_edge(b, a)
}

fn fully_outside_one_edge(q: &[Point; 4], other: &[Point; 4]) -> bool {
    let [p1, p2, p3, p4] = *q;
    let counter_clockwise = {
        let p24 = p2.y - p4.y;
        let p31 = p3.y - p1.y;
        p1.x * p24 + p2.x * p31 < p3.x * p24 + p4.x * p31
    };
    let edges = if counter_clockwise {
        [(p1, p4 - p1), (p2, p1 - p2), (p3, p2 - p3), (p4, p3 - p4)]
    } else {
        [(p1, p2 - p1), (p2, p3 - p2), (p3, p4 - p3), (p4, p1 - p4)]
    };
    edges
        .iter()
        .any(|&(base, vector)| other.iter().all(|&p| cross(vector, p - base) < 0.0))
}

/// The radii of a box's corners scaled so that the hulls of opposite concave corners do not
/// overlap (`ComputeContouredBorderFromStyle`), for a box whose radii already fit it.
pub fn constrain_opposite_corners(
    rect: Rect,
    radii: NonUniformRoundedRectRadii,
    curvature: &CornerCurvature,
) -> NonUniformRoundedRectRadii {
    if curvature.is_convex() {
        return radii;
    }
    let contour = ContouredRect {
        rect,
        radii,
        curvature: *curvature,
        origin: None,
    };
    let [tr, br, bl, tl] = contour.corners_of(rect, &radii);
    let factor = opposite_corners_constraint(&tl, &br)
        .min(opposite_corners_constraint(&bl, &tr))
        .min(1.0);
    if factor >= 1.0 {
        return radii;
    }
    let scale = |r: Vec2| {
        let r = r * factor;
        if r.x == 0.0 || r.y == 0.0 { Vec2::ZERO } else { r }
    };
    NonUniformRoundedRectRadii {
        top_left: scale(radii.top_left),
        top_right: scale(radii.top_right),
        bottom_right: scale(radii.bottom_right),
        bottom_left: scale(radii.bottom_left),
    }
}

// ---------- Polygon boolean ----------

/// A point index, snapping points within `SNAP` of each other together.
struct Points {
    points: Vec<Point>,
    cells: std::collections::HashMap<(i64, i64), Vec<usize>>,
}

const SNAP: f64 = 1e-6;

impl Points {
    fn new() -> Self {
        Self {
            points: Vec::new(),
            cells: std::collections::HashMap::new(),
        }
    }

    fn cell(p: Point) -> (i64, i64) {
        ((p.x / SNAP).floor() as i64, (p.y / SNAP).floor() as i64)
    }

    fn id(&mut self, p: Point) -> usize {
        let (cx, cy) = Self::cell(p);
        for dx in -1..=1 {
            for dy in -1..=1 {
                if let Some(ids) = self.cells.get(&(cx + dx, cy + dy)) {
                    for &id in ids {
                        let q = self.points[id];
                        if (q.x - p.x).abs() <= SNAP && (q.y - p.y).abs() <= SNAP {
                            return id;
                        }
                    }
                }
            }
        }
        let id = self.points.len();
        self.points.push(p);
        self.cells.entry((cx, cy)).or_default().push(id);
        id
    }
}

/// The nonzero winding number of `polygon` around `p`.
fn winding(polygon: &[Point], p: Point) -> i32 {
    let mut w = 0;
    let n = polygon.len();
    for i in 0..n {
        let (a, b) = (polygon[i], polygon[(i + 1) % n]);
        if a.y <= p.y {
            if b.y > p.y && cross(b - a, p - a) > 0.0 {
                w += 1;
            }
        } else if b.y <= p.y && cross(b - a, p - a) < 0.0 {
            w -= 1;
        }
    }
    w
}

/// The region of the plane that `inside` selects, given whether a point is inside each of
/// `shapes` (each a set of polygons, filled by the nonzero rule), as a path of closed polygons
/// wound so that it fills with the nonzero rule. Edges are split where they cross, kept where
/// the selection differs on their two sides, and joined into loops.
pub fn boolean(shapes: &[Vec<Vec<Point>>], inside: impl Fn(&[bool]) -> bool) -> BezPath {
    struct Segment {
        a: Point,
        b: Point,
        bounds: Rect,
        splits: Vec<(f64, Point)>,
    }
    let mut segments: Vec<Segment> = Vec::new();
    for polygon in shapes.iter().flatten() {
        let n = polygon.len();
        for i in 0..n {
            let (a, b) = (polygon[i], polygon[(i + 1) % n]);
            if (b - a).hypot() <= SNAP {
                continue;
            }
            segments.push(Segment {
                a,
                b,
                bounds: Rect::from_points(a, b).inflate(SNAP, SNAP),
                splits: Vec::new(),
            });
        }
    }
    let bounds: Vec<Rect> = shapes
        .iter()
        .map(|shape| {
            shape.iter().flatten().fold(
                Rect::new(f64::MAX, f64::MAX, f64::MIN, f64::MIN),
                |r, &q| Rect::new(r.x0.min(q.x), r.y0.min(q.y), r.x1.max(q.x), r.y1.max(q.y)),
            )
        })
        .collect();

    // Split every segment where another crosses or touches it.
    for i in 0..segments.len() {
        for j in i + 1..segments.len() {
            let (s, t) = (&segments[i], &segments[j]);
            if s.bounds.x1 < t.bounds.x0
                || t.bounds.x1 < s.bounds.x0
                || s.bounds.y1 < t.bounds.y0
                || t.bounds.y1 < s.bounds.y0
            {
                continue;
            }
            let (a, b, c, d) = (s.a, s.b, t.a, t.b);
            let (r, q) = (b - a, d - c);
            let (rl, ql) = (r.hypot(), q.hypot());
            let denominator = cross(r, q);
            let mut on_s: Vec<(f64, Point)> = Vec::new();
            let mut on_t: Vec<(f64, Point)> = Vec::new();
            if denominator.abs() > 1e-12 * rl * ql {
                let u = cross(c - a, q) / denominator;
                let v = cross(c - a, r) / denominator;
                let (eu, ev) = (SNAP / rl, SNAP / ql);
                if u >= -eu && u <= 1.0 + eu && v >= -ev && v <= 1.0 + ev {
                    let p = a + r * u.clamp(0.0, 1.0);
                    on_s.push((u.clamp(0.0, 1.0), p));
                    on_t.push((v.clamp(0.0, 1.0), p));
                }
            } else if (cross(r, c - a) / rl).abs() <= SNAP {
                // Collinear: each splits the other at its ends.
                for p in [c, d] {
                    let u = (p - a).dot(r) / (rl * rl);
                    if u > 0.0 && u < 1.0 {
                        on_s.push((u, p));
                    }
                }
                for p in [a, b] {
                    let v = (p - c).dot(q) / (ql * ql);
                    if v > 0.0 && v < 1.0 {
                        on_t.push((v, p));
                    }
                }
            }
            segments[i].splits.extend(on_s);
            segments[j].splits.extend(on_t);
        }
    }

    // The pieces, each once.
    let mut points = Points::new();
    let mut pieces: Vec<(usize, usize)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for s in &mut segments {
        s.splits.push((0.0, s.a));
        s.splits.push((1.0, s.b));
        s.splits.sort_by(|x, y| x.0.total_cmp(&y.0));
        let ids: Vec<usize> = s.splits.iter().map(|&(_, p)| points.id(p)).collect();
        for w in ids.windows(2) {
            let (x, y) = (w[0], w[1]);
            if x != y && seen.insert((x.min(y), x.max(y))) {
                pieces.push((x, y));
            }
        }
    }

    // Keep the pieces with the selection on one side only, the selection on their left.
    let selected = |p: Point| {
        let memberships: Vec<bool> = shapes
            .iter()
            .zip(&bounds)
            .map(|(shape, b)| {
                p.x >= b.x0
                    && p.x <= b.x1
                    && p.y >= b.y0
                    && p.y <= b.y1
                    && shape.iter().map(|polygon| winding(polygon, p)).sum::<i32>() != 0
            })
            .collect();
        inside(&memberships)
    };
    let mut edges: Vec<(usize, usize)> = Vec::new();
    for &(x, y) in &pieces {
        let (a, b) = (points.points[x], points.points[y]);
        let direction = b - a;
        let length = direction.hypot();
        let normal = Vec2::new(-direction.y, direction.x) / length * (SNAP * 10.0);
        let middle = a.midpoint(b);
        let (left, right) = (selected(middle + normal), selected(middle - normal));
        if left != right {
            edges.push(if left { (x, y) } else { (y, x) });
        }
    }

    // Join them into loops.
    let mut from: std::collections::HashMap<usize, Vec<usize>> = std::collections::HashMap::new();
    for (i, &(x, _)) in edges.iter().enumerate() {
        from.entry(x).or_default().push(i);
    }
    let mut used = vec![false; edges.len()];
    let mut path = BezPath::new();
    for first in 0..edges.len() {
        if used[first] {
            continue;
        }
        used[first] = true;
        let start = edges[first].0;
        path.move_to(points.points[start]);
        let mut at = edges[first].1;
        let mut count = 1;
        while at != start && count <= edges.len() {
            path.line_to(points.points[at]);
            let Some(next) = from
                .get(&at)
                .and_then(|list| list.iter().copied().find(|&e| !used[e]))
            else {
                break;
            };
            used[next] = true;
            at = edges[next].1;
            count += 1;
        }
        path.close_path();
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uniform(r: f64) -> NonUniformRoundedRectRadii {
        let v = Vec2::new(r, r);
        NonUniformRoundedRectRadii {
            top_left: v,
            top_right: v,
            bottom_right: v,
            bottom_left: v,
        }
    }

    fn all(c: f64) -> CornerCurvature {
        CornerCurvature {
            top_left: c,
            top_right: c,
            bottom_right: c,
            bottom_left: c,
        }
    }

    /// WPT `corner-shape-squircle-ref.html` was drawn with Blink's curves: its top right
    /// corner is `C 86.001 0.000 92.009 -0.036 96.022 3.978 C 100.036 7.991 100.000 13.999
    /// 100.000 25.000`.
    #[test]
    fn squircle_corner_matches_blink() {
        let contour = ContouredRect {
            rect: Rect::new(0.0, 0.0, 100.0, 100.0),
            radii: uniform(25.0),
            curvature: all(exponent(2.0)),
            origin: None,
        };
        let path = contour.path();
        let curves: Vec<_> = path
            .elements()
            .iter()
            .filter_map(|el| match *el {
                PathEl::CurveTo(a, b, c) => Some([a, b, c]),
                _ => None,
            })
            .collect();
        let close = |p: Point, x: f64, y: f64| (p.x - x).abs() < 2e-3 && (p.y - y).abs() < 2e-3;
        assert!(close(curves[0][0], 86.001, 0.0), "{:?}", curves[0]);
        assert!(close(curves[0][1], 92.009, -0.036), "{:?}", curves[0]);
        assert!(close(curves[0][2], 96.022, 3.978), "{:?}", curves[0]);
        assert!(close(curves[1][0], 100.036, 7.991), "{:?}", curves[1]);
        assert!(close(curves[1][1], 100.0, 13.999), "{:?}", curves[1]);
        assert!(close(curves[1][2], 100.0, 25.0), "{:?}", curves[1]);
    }

    /// The padding edge of a bevelled 100px box with 25px radii and a 5px border is the bevel
    /// moved 5px along its normal (WPT `corner-shape-bevel-ref.html`: 27.071px 5px).
    #[test]
    fn inner_bevel_is_parallel() {
        let outer = Rect::new(0.0, 0.0, 100.0, 100.0);
        let contour = ContouredRect {
            rect: outer.inset(-5.0),
            radii: uniform(20.0),
            curvature: all(exponent(0.0)),
            origin: Some((outer, uniform(25.0))),
        };
        let path = contour.path();
        let area = path.area().abs();
        // The 90px square less four right triangles of legs 22.929 - 5 = 17.929... the cut
        // corners are triangles whose legs run from the corner to 27.071 - 5.
        let leg = 27.071_067_8 - 5.0;
        let expected = 90.0 * 90.0 - 4.0 * leg * leg / 2.0;
        assert!((area - expected).abs() < 0.05, "{area} vs {expected}");
        let bounds = path.bounding_box();
        assert!((bounds.x0 - 5.0).abs() < 1e-6 && (bounds.x1 - 95.0).abs() < 1e-6, "{bounds:?}");
    }

    /// Scoop corners of radius 75 on a 150px box with a 20px border leave the middle of the
    /// box: WPT `inner-corner-collapse002-ref.html` (circles of radius 95 around the corners).
    #[test]
    fn overlapping_scoops_leave_the_middle() {
        let outer = Rect::new(0.0, 0.0, 150.0, 150.0);
        let contour = ContouredRect {
            rect: outer.inset(-20.0),
            radii: uniform(55.0),
            curvature: all(exponent(-1.0)),
            origin: Some((outer, uniform(75.0))),
        };
        let path = contour.path();
        let bounds = path.bounding_box();
        assert!((bounds.x0 - 58.31).abs() < 0.05 && (bounds.x1 - 91.69).abs() < 0.05, "{bounds:?}");
        assert!((bounds.y0 - 58.31).abs() < 0.05 && (bounds.y1 - 91.69).abs() < 0.05, "{bounds:?}");
        // Nothing of the box outside the four circles is left out, nothing inside one kept.
        for (p, expected) in [((75.0, 75.0), true), ((60.0, 75.0), true), ((57.0, 75.0), false), ((30.0, 30.0), false)] {
            let p = Point::new(p.0, p.1);
            assert_eq!(path.winding(p) != 0, expected, "{p:?}");
        }
    }

    #[test]
    fn boolean_difference_of_rects() {
        let square = |x0: f64, y0: f64, x1: f64, y1: f64| {
            vec![Point::new(x0, y0), Point::new(x1, y0), Point::new(x1, y1), Point::new(x0, y1)]
        };
        let path = boolean(
            &[vec![square(0.0, 0.0, 10.0, 10.0)], vec![square(5.0, -5.0, 15.0, 5.0)]],
            |m| m[0] && !m[1],
        );
        assert!((path.area().abs() - 75.0).abs() < 1e-9);
        // A cut sharing an edge and a corner with the subject.
        let path = boolean(
            &[vec![square(0.0, 0.0, 10.0, 10.0)], vec![square(5.0, 0.0, 10.0, 5.0)]],
            |m| m[0] && !m[1],
        );
        assert!((path.area().abs() - 75.0).abs() < 1e-9);
        assert_eq!(path.winding(Point::new(7.0, 2.0)), 0);
        assert_ne!(path.winding(Point::new(2.0, 2.0)), 0);
    }

    /// Concave corners whose hulls meet are scaled down until they (nearly) touch: two 220px
    /// scoops on a 240px box have hulls meeting at a scale of 0.727, which Blink's binary
    /// search (steps down to 0.05) settles at 0.71875. Convex corners are left alone.
    #[test]
    fn opposite_scoops_are_constrained() {
        let rect = Rect::new(0.0, 0.0, 240.0, 240.0);
        let radii = NonUniformRoundedRectRadii {
            top_left: Vec2::new(220.0, 220.0),
            bottom_right: Vec2::new(220.0, 220.0),
            ..NonUniformRoundedRectRadii::default()
        };
        let scoops = CornerCurvature {
            top_left: exponent(-1.0),
            bottom_right: exponent(-1.0),
            ..CornerCurvature::ROUND
        };
        let constrained = constrain_opposite_corners(rect, radii, &scoops);
        assert!((constrained.top_left.x - 220.0 * 0.71875).abs() < 1e-9, "{constrained:?}");
        let squircles = CornerCurvature {
            top_left: exponent(2.0),
            bottom_right: exponent(2.0),
            ..CornerCurvature::ROUND
        };
        assert_eq!(constrain_opposite_corners(rect, radii, &squircles), radii);
    }
}
