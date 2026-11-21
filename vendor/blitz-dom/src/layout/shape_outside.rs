//! pyro: a float's float area from `shape-outside` (CSS Shapes 1 § 3), as rows of its margin box
//! with the shape's horizontal extent in each, for Taffy's float context
//! (`BlockContext::place_shaped_floated_box`): line boxes beside the float wrap around the shape.
//!
//! Basic shapes (`circle()`, `ellipse()`, `inset()`, `polygon()`) in their reference box and the
//! box keywords (the reference box itself, corners square), `path()` and `shape()`, grown by
//! `shape-margin`. Images keep the margin box.

use kurbo::{BezPath, PathEl, Point, Rect, Shape as _};
use style::properties::ComputedValues;
use style::values::computed::basic_shape::{BasicShape, ShapeOutside};
use style::values::computed::{CSSPixelLength, Image, LengthPercentage};
use crate::node::ImageData;
use style::values::generics::basic_shape::{GenericBasicShape, GenericPathOrShapeFunction, GenericShapeRadius, ShapeBox};
use style::values::generics::position::GenericPositionOrAuto;

/// One row of a float's margin box: `(top, bottom, extent)`, `extent` the float area's
/// `(left, right)` in it, `None` where the float area has nothing.
pub(crate) type Band = (f32, f32, Option<(f32, f32)>);

fn lp(v: &LengthPercentage, basis: f64) -> f64 {
    v.resolve(CSSPixelLength::new(basis as f32)).px() as f64
}

fn position(p: &GenericPositionOrAuto<style::values::computed::Position>, r: Rect) -> Point {
    match p {
        GenericPositionOrAuto::Auto => r.center(),
        GenericPositionOrAuto::Position(pos) => {
            Point::new(r.x0 + lp(&pos.horizontal, r.width()), r.y0 + lp(&pos.vertical, r.height()))
        }
    }
}

fn axis_radius(radius: &GenericShapeRadius<LengthPercentage>, center: f64, start: f64, end: f64, basis: f64) -> f64 {
    let near = (center - start).abs().min((end - center).abs());
    let far = (center - start).abs().max((end - center).abs());
    match radius {
        GenericShapeRadius::Length(v) => lp(&v.0, basis),
        GenericShapeRadius::ClosestSide | GenericShapeRadius::ClosestCorner => near,
        GenericShapeRadius::FarthestSide | GenericShapeRadius::FarthestCorner => far,
    }
}

/// The border radii (top-left, top-right, bottom-right, bottom-left) of a border box `w` by `h`.
fn corner_radii(b: &style::properties::style_structs::Border, w: f64, h: f64) -> [kurbo::Vec2; 4] {
    let r = |c: &style::values::computed::BorderCornerRadius| kurbo::Vec2::new(lp(&c.0.width.0, w), lp(&c.0.height.0, h));
    [
        r(&b.border_top_left_radius),
        r(&b.border_top_right_radius),
        r(&b.border_bottom_right_radius),
        r(&b.border_bottom_left_radius),
    ]
}

/// A border radius grown by a margin `m` at the margin edge (CSS Shapes 1 § 3.1.1, as a box
/// shadow's spread in CSS Backgrounds 3): by all of `m` where the radius is at least `m`, by
/// `m (1 + (r / m - 1)³)` where it is smaller. A negative margin insets the edge: the radius
/// shrinks by it, to no less than 0 (the border contour path of an inset box).
fn grow_radius(r: f64, m: f64) -> f64 {
    if m <= 0.0 || r >= m {
        return (r + m).max(0.0);
    }
    r + m * (1.0 + (r / m - 1.0).powi(3))
}

/// The float area as a path in margin-box coordinates, `None` for the margin box itself.
fn area(
    style: &ComputedValues,
    margin_box: Rect,
    margin: [f64; 4],
    border: [f64; 4],
    padding: [f64; 4],
    image: Option<&ImageData>,
    columns: bool,
) -> Option<BezPath> {
    let inset = |r: Rect, e: [f64; 4]| Rect::new(r.x0 + e[3], r.y0 + e[0], r.x1 - e[1], r.y1 - e[2]);
    let border_box = inset(margin_box, margin);
    let reference = |b: ShapeBox| match b {
        ShapeBox::MarginBox => margin_box,
        ShapeBox::BorderBox => border_box,
        ShapeBox::PaddingBox => inset(border_box, border),
        ShapeBox::ContentBox => inset(inset(border_box, border), padding),
    };
    match &style.get_box().shape_outside {
        ShapeOutside::None => None,
        ShapeOutside::Image(img) => image_area(style, img, reference(ShapeBox::ContentBox), image, columns),
        ShapeOutside::Box(b) => {
            // The box with its border radii (CSS Shapes 1 § 3.1.1): grown by the margin at the
            // margin edge (as a shadow's spread), shrunk by the border and padding inside.
            let b = *b;
            let radii = corner_radii(&style.get_border(), border_box.width(), border_box.height());
            if b == ShapeBox::MarginBox && radii.iter().all(|r| r.x <= 0.0 || r.y <= 0.0) {
                return None;
            }
            // (horizontal, vertical) edge widths at each corner: top-left, top-right,
            // bottom-right, bottom-left.
            let corner_edges = |e: [f64; 4]| [(e[3], e[0]), (e[1], e[0]), (e[1], e[2]), (e[3], e[2])];
            let adjust: Box<dyn Fn(kurbo::Vec2, (f64, f64)) -> kurbo::Vec2> = match b {
                ShapeBox::MarginBox => Box::new(|r: kurbo::Vec2, (dx, dy): (f64, f64)| {
                    if r.x <= 0.0 || r.y <= 0.0 {
                        return kurbo::Vec2::ZERO;
                    }
                    kurbo::Vec2::new(grow_radius(r.x, dx), grow_radius(r.y, dy))
                }),
                _ => Box::new(|r: kurbo::Vec2, (dx, dy): (f64, f64)| {
                    kurbo::Vec2::new((r.x - dx).max(0.0), (r.y - dy).max(0.0))
                }),
            };
            let edges = match b {
                ShapeBox::MarginBox => corner_edges(margin),
                ShapeBox::BorderBox => [(0.0, 0.0); 4],
                ShapeBox::PaddingBox => corner_edges(border),
                ShapeBox::ContentBox => corner_edges(std::array::from_fn(|i| border[i] + padding[i])),
            };
            let radii: [kurbo::Vec2; 4] = std::array::from_fn(|i| adjust(radii[i], edges[i]));
            Some(crate::stylo_to_kurbo::rounded_rect_path(reference(b), radii))
        }
        ShapeOutside::Shape(shape, b) => {
            let r = reference(*b);
            let (w, h) = (r.width(), r.height());
            let shape: &BasicShape = shape;
            Some(match shape {
                GenericBasicShape::Circle(c) => {
                    let center = position(&c.position, r);
                    let radius = match &c.radius {
                        GenericShapeRadius::Length(v) => lp(&v.0, w.hypot(h) / std::f64::consts::SQRT_2),
                        other => {
                            let x = axis_radius(other, center.x, r.x0, r.x1, w);
                            let y = axis_radius(other, center.y, r.y0, r.y1, h);
                            match other {
                                GenericShapeRadius::ClosestSide => x.min(y),
                                GenericShapeRadius::FarthestSide => x.max(y),
                                _ => x.hypot(y),
                            }
                        }
                    };
                    kurbo::Circle::new(center, radius.max(0.0)).to_path(0.001)
                }
                GenericBasicShape::Ellipse(e) => {
                    let center = position(&e.position, r);
                    let rx = axis_radius(&e.semiaxis_x, center.x, r.x0, r.x1, w).max(0.0);
                    let ry = axis_radius(&e.semiaxis_y, center.y, r.y0, r.y1, h).max(0.0);
                    kurbo::Ellipse::new(center, (rx, ry), 0.0).to_path(0.001)
                }
                GenericBasicShape::Rect(inset) => {
                    // A pair of insets adding up to more than the box's size is reduced
                    // proportionally to add up to it (CSS Shapes 1 § 2.1.1): the shape is then a
                    // line, which still bounds the float area (a horizontal one only with a margin).
                    let fit = |a: f64, b: f64, size: f64| {
                        let sum = a + b;
                        if sum > size && sum > 0.0 { (a * size.max(0.0) / sum, b * size.max(0.0) / sum) } else { (a, b) }
                    };
                    let (top, bottom) = fit(lp(&inset.rect.0, h), lp(&inset.rect.2, h), h);
                    let (left, right) = fit(lp(&inset.rect.3, w), lp(&inset.rect.1, w), w);
                    let rect = Rect::new(r.x0 + left, r.y0 + top, r.x1 - right, r.y1 - bottom);
                    let corner = |c: &style::values::computed::BorderCornerRadius| {
                        kurbo::Vec2::new(lp(&c.0.width.0, w), lp(&c.0.height.0, h))
                    };
                    let round = &inset.round;
                    let radii = [
                        corner(&round.top_left),
                        corner(&round.top_right),
                        corner(&round.bottom_right),
                        corner(&round.bottom_left),
                    ];
                    crate::stylo_to_kurbo::rounded_rect_path(rect, radii)
                }
                GenericBasicShape::Polygon(poly) => {
                    crate::shape_path::polygon_path(poly.coordinates.iter().map(|coord|
                        Point::new(r.x0 + lp(&coord.0, w), r.y0 + lp(&coord.1, h))), lp(&poly.round.0, 0.0))
                }
                GenericBasicShape::PathOrShape(GenericPathOrShapeFunction::Path(path)) => {
                    // `path()`: SVG path data in CSS px from the reference box's origin.
                    let px = |v: &f32| *v as f64;
                    let mut bez = crate::shape_path::svg_path_to_bezpath(path.commands(), px, px, px, |a: &f32| *a as f64)?;
                    bez.apply_affine(kurbo::Affine::translate((r.x0, r.y0)));
                    bez
                }
                GenericBasicShape::PathOrShape(GenericPathOrShapeFunction::Shape(shape)) => {
                    // `shape()`: percentages of the reference box; a single arc radius of its
                    // direction-agnostic size (as `clip-path`).
                    let agnostic = w.hypot(h) / std::f64::consts::SQRT_2;
                    let mut bez = crate::shape_path::svg_path_to_bezpath(
                        &shape.commands,
                        |v: &LengthPercentage| lp(v, w),
                        |v: &LengthPercentage| lp(v, h),
                        |v: &LengthPercentage| lp(v, agnostic),
                        |a: &style::values::computed::Angle| a.degrees() as f64,
                    )?;
                    bez.apply_affine(kurbo::Affine::translate((r.x0, r.y0)));
                    bez
                }
            })
        }
    }
}

/// The float area of a float with the given margin box size and edges (top, right, bottom, left)
/// as [`Band`]s, `None` when it is the margin box.
pub(crate) fn float_bands(
    style: &ComputedValues,
    size: (f32, f32),
    margin: [f32; 4],
    border: [f32; 4],
    padding: [f32; 4],
    wm: stylo_taffy::WritingMode,
    containing_block_width: f32,
    image: Option<&ImageData>,
) -> Option<Vec<Band>> {
    use stylo_taffy::WritingModeExt as _;
    let f = |e: [f32; 4]| e.map(f64::from);
    // `size` is in the float context's axes (Taffy's: x inline, y block), the edges are physical
    // (top, right, bottom, left): the area is made physically, then mapped into those axes (the
    // physical y is the inline axis of a vertical writing mode, the x its block axis, mirrored
    // for `*-rl`).
    let vertical = wm.swaps_axes();
    let physical = if vertical { (size.1 as f64, size.0 as f64) } else { (size.0 as f64, size.1 as f64) };
    let margin_box = Rect::new(0.0, 0.0, physical.0, physical.1);
    let mut path = area(style, margin_box, f(margin), f(border), f(padding), image, vertical)?;
    if vertical {
        let affine = if wm.is_vertical_lr() {
            kurbo::Affine::new([0.0, 1.0, 1.0, 0.0, 0.0, 0.0])
        } else {
            kurbo::Affine::new([0.0, -1.0, 1.0, 0.0, 0.0, physical.0])
        };
        path.apply_affine(affine);
    }
    // Percentages of the containing block's inline size.
    let shape_margin = lp(&style.get_box().shape_margin.0, containing_block_width as f64).max(0.0);
    let height = size.1 as f64;
    if height <= 0.0 {
        return None;
    }
    Some(path_bands(&path, size.0 as f64, height, shape_margin, even_odd(style)))
}

/// Whether the float area's shape fills by the even-odd rule (`polygon()`, `path()`, `shape()`).
fn even_odd(style: &ComputedValues) -> bool {
    use style::values::generics::basic_shape::FillRule;
    let fill = match &style.get_box().shape_outside {
        ShapeOutside::Shape(shape, _) => match &**shape {
            GenericBasicShape::Polygon(poly) => poly.fill,
            GenericBasicShape::PathOrShape(GenericPathOrShapeFunction::Path(path)) => path.fill,
            GenericBasicShape::PathOrShape(GenericPathOrShapeFunction::Shape(shape)) => shape.fill,
            _ => FillRule::Nonzero,
        },
        _ => FillRule::Nonzero,
    };
    fill == FillRule::Evenodd
}

/// The most edges flattened curves give a float area: past it they are flattened more coarsely.
const MAX_CURVE_EDGES: usize = 20_000;

/// The edges of `path` (each subpath closed) for the float area in the box `region`: a curve
/// whose control points all lie outside it is its chord (which lies in their hull too, so the
/// area inside the region is the same), one far larger than it is split first, and curves are
/// flattened more coarsely while they would give more than [`MAX_CURVE_EDGES`] edges: a path
/// with control points far outside the box costs no more than one inside it.
fn path_edges(path: &BezPath, region: Rect) -> Vec<(Point, Point)> {
    let mut tolerance = 0.005;
    loop {
        let mut edges = Vec::new();
        let mut curve_edges = 0;
        let (mut start, mut last) = (Point::ZERO, Point::ZERO);
        let close = |edges: &mut Vec<(Point, Point)>, last: Point, start: Point| {
            if last != start {
                edges.push((last, start));
            }
        };
        for el in path.elements() {
            let seg = match *el {
                PathEl::MoveTo(p) => {
                    close(&mut edges, last, start);
                    start = p;
                    last = p;
                    continue;
                }
                PathEl::ClosePath => {
                    close(&mut edges, last, start);
                    last = start;
                    continue;
                }
                PathEl::LineTo(p) => kurbo::PathSeg::Line(kurbo::Line::new(last, p)),
                PathEl::QuadTo(c, p) => kurbo::PathSeg::Quad(kurbo::QuadBez::new(last, c, p)),
                PathEl::CurveTo(c0, c1, p) => kurbo::PathSeg::Cubic(kurbo::CubicBez::new(last, c0, c1, p)),
            };
            last = kurbo::ParamCurve::end(&seg);
            let before = edges.len();
            flatten_segment(seg, region, tolerance, 0, &mut edges);
            if !matches!(seg, kurbo::PathSeg::Line(_)) {
                curve_edges += edges.len() - before;
            }
        }
        close(&mut edges, last, start);
        if curve_edges <= MAX_CURVE_EDGES || tolerance > 1.0 {
            return edges;
        }
        tolerance *= 4.0;
    }
}

/// [`path_edges`] for one segment.
fn flatten_segment(seg: kurbo::PathSeg, region: Rect, tolerance: f64, depth: u32, out: &mut Vec<(Point, Point)>) {
    use kurbo::{ParamCurve as _, PathSeg};
    let (a, b) = (seg.start(), seg.end());
    let points: &[Point] = match &seg {
        PathSeg::Line(l) => &[l.p0, l.p1],
        PathSeg::Quad(q) => &[q.p0, q.p1, q.p2],
        PathSeg::Cubic(c) => &[c.p0, c.p1, c.p2, c.p3],
    };
    if points.iter().any(|p| !(p.x.is_finite() && p.y.is_finite())) {
        return;
    }
    let el = match seg {
        PathSeg::Line(_) => {
            out.push((a, b));
            return;
        }
        PathSeg::Quad(q) => PathEl::QuadTo(q.p1, q.p2),
        PathSeg::Cubic(c) => PathEl::CurveTo(c.p1, c.p2, c.p3),
    };
    let hull = points.iter().skip(1).fold(Rect::from_points(points[0], points[0]), |r, p| r.union_pt(*p));
    if hull.x1 < region.x0 || hull.x0 > region.x1 || hull.y1 < region.y0 || hull.y0 > region.y1 {
        out.push((a, b));
        return;
    }
    let size = hull.width().max(hull.height());
    if size > 4.0 * region.width().max(region.height()) + 64.0 && depth < 48 {
        let (first, second) = seg.subdivide();
        flatten_segment(first, region, tolerance, depth + 1, out);
        flatten_segment(second, region, tolerance, depth + 1, out);
        return;
    }
    let mut last = a;
    kurbo::flatten([PathEl::MoveTo(a), el], tolerance.max(size * 1e-6), |el| {
        if let PathEl::LineTo(p) = el {
            out.push((last, p));
            last = p;
        }
    });
}

/// The float area of the shape `path` (in the float context's axes, the margin box at the origin
/// `width` by `height`) grown by `shape_margin`, as [`Band`]s of 1px (at most 2,000 of them).
/// The area is clipped to the margin box before each row's extent is taken: pieces of it wholly
/// outside the box take nothing, even with pieces on both sides of it.
fn path_bands(path: &BezPath, width: f64, height: f64, shape_margin: f64, even_odd: bool) -> Vec<Band> {
    let m = shape_margin;
    let region = Rect::new(-m - 1.0, -m - 1.0, width + m + 1.0, height + m + 1.0);
    let edges = path_edges(path, region);
    let top = |e: &(Point, Point)| e.0.y.min(e.1.y);
    let bottom = |e: &(Point, Point)| e.0.y.max(e.1.y);
    // The edges by their top, swept down the rows: each row looks only at the edges in it.
    let mut order: Vec<usize> = (0..edges.len()).filter(|&i| bottom(&edges[i]) + m >= 0.0).collect();
    order.sort_by(|&i, &j| top(&edges[i]).total_cmp(&top(&edges[j])));
    let mut next = 0;
    let mut active: Vec<usize> = Vec::new();
    let mut crossings: Vec<(f64, i32)> = Vec::new();
    let step = (height / 2000.0).max(1.0);
    let mut bands = Vec::new();
    let mut y = 0.0;
    while y < height {
        let y1 = (y + step).min(height);
        while next < order.len() && top(&edges[order[next]]) - m <= y1 {
            active.push(order[next]);
            next += 1;
        }
        active.retain(|&i| bottom(&edges[i]) + m >= y);
        // Each piece of the row's area, clipped to the margin box: kept when something of it is
        // left (a line, the edge of a shape enclosing no area, only where it is in the box).
        let mut extent: Option<(f64, f64)> = None;
        let mut take = |l: f64, r: f64| {
            let (cl, cr) = (l.max(0.0), r.min(width));
            if cr > cl || (cr == cl && l >= 0.0 && r <= width) {
                extent = Some(extent.map_or((cl, cr), |(el, er): (f64, f64)| (el.min(cl), er.max(cr))));
            }
        };
        // The edges, grown by the margin (the points within that distance of them)...
        for &i in &active {
            let (a, b) = edges[i];
            let mirror = |p: Point| Point::new(-p.x, p.y);
            if let (Some(r), Some(l)) = (edge_max_x(a, b, y, y1, m), edge_max_x(mirror(a), mirror(b), y, y1, m)) {
                take(-l, r);
            }
        }
        // ...and the inside between them, at the row's top, middle and bottom.
        let e = (y1 - y) * 1e-3;
        for sample in [y + e, (y + y1) / 2.0, y1 - e] {
            crossings.clear();
            for &i in &active {
                let (a, b) = edges[i];
                if (a.y <= sample) != (b.y <= sample) {
                    let x = a.x + (sample - a.y) * (b.x - a.x) / (b.y - a.y);
                    crossings.push((x, if b.y > a.y { 1 } else { -1 }));
                }
            }
            crossings.sort_by(|p, q| p.0.total_cmp(&q.0));
            let mut winding = 0;
            let mut from = 0.0;
            for &(x, dir) in &crossings {
                let was = if even_odd { winding % 2 != 0 } else { winding != 0 };
                winding += dir;
                let is = if even_odd { winding % 2 != 0 } else { winding != 0 };
                if !was && is {
                    from = x;
                } else if was && !is {
                    take(from, x);
                }
            }
        }
        bands.push((y as f32, y1 as f32, extent.map(|(l, r)| (l as f32, r as f32))));
        y = y1;
    }
    bands
}

/// The rightmost x of the points within `m` of the edge `a`-`b` whose y is in the row `y0..y1`
/// (`None`: none is). With no margin an edge only touching the row's top or bottom is not in it,
/// nor is a horizontal edge (the area has no height there; the other edges bound it).
fn edge_max_x(a: Point, b: Point, y0: f64, y1: f64, m: f64) -> Option<f64> {
    let (top, bottom) = if a.y <= b.y { (a, b) } else { (b, a) };
    let dy = bottom.y - top.y;
    let horizontal = dy.abs() < 1e-9;
    let k = if horizontal { 0.0 } else { (bottom.x - top.x) / dy };
    let x_at = |y: f64| if horizontal { top.x.max(bottom.x) } else { top.x + k * (y - top.y) };
    let mut best: Option<f64> = None;
    let mut take = |v: f64| best = Some(best.map_or(v, |b: f64| b.max(v)));
    // The part of the edge in the row.
    let inside = if m > 0.0 { bottom.y >= y0 && top.y <= y1 } else { !horizontal && bottom.y > y0 && top.y < y1 };
    if inside {
        let (from, to) = (top.y.max(y0), bottom.y.min(y1));
        take(x_at(from).max(x_at(to)) + m.max(0.0));
    }
    if m <= 0.0 {
        return best;
    }
    // A part above or below the row reaches into it by `sqrt(m² - u²)` at distance `u` from it.
    // Along the part x is `c + s u`; `c + s u + sqrt(m² - u²)` is greatest at
    // `u = m s / sqrt(1 + s²)` (s > 0), clamped to the part.
    let mut part = |y_from: f64, y_to: f64, u_of: &dyn Fn(f64) -> f64, s: f64| {
        if y_from > y_to {
            return;
        }
        let (u_a, u_b) = { let (p, q) = (u_of(y_from), u_of(y_to)); (p.min(q), p.max(q)) };
        if u_a >= m - 1e-9 {
            return;
        }
        let u_b = u_b.min(m);
        let x_of_u = |u: f64| {
            // y at distance u, on the part.
            let y = if (u_of(y_from) - u).abs() < 1e-12 { y_from } else if (u_of(y_to) - u).abs() < 1e-12 { y_to } else {
                y_from + (y_to - y_from) * ((u - u_of(y_from)) / (u_of(y_to) - u_of(y_from)))
            };
            x_at(y)
        };
        let f = |u: f64| x_of_u(u) + (m * m - u * u).max(0.0).sqrt();
        let mut v = f(u_a).max(f(u_b));
        if s > 0.0 && !horizontal {
            let u = (m * s / (1.0 + s * s).sqrt()).clamp(u_a, u_b);
            v = v.max(f(u));
        }
        take(v);
    };
    // Above: u = y0 - y, x = c - k u.
    part(top.y.max(y0 - m), bottom.y.min(y0), &|y| y0 - y, -k);
    // Below: u = y - y1, x = c + k u.
    part(top.y.max(y1), bottom.y.min(y1 + m), &|y| y - y1, k);
    best
}

/// [`float_bands`] for a float's computed style, its margin box and margins: the border widths and
/// paddings (percentages of the margin box's width less the margins) come from the style.
pub(crate) fn float_bands_for(
    style: &ComputedValues,
    margin_box: taffy::Size<f32>,
    margin: taffy::Rect<f32>,
    wm: stylo_taffy::WritingMode,
    containing_block_width: f32,
    image: Option<&ImageData>,
) -> Option<Vec<Band>> {
    use stylo_taffy::WritingModeExt as _;
    if matches!(style.get_box().shape_outside, ShapeOutside::None) {
        return None;
    }
    // Percentage paddings: of the containing block's inline size.
    let basis = containing_block_width.max(0.0) as f64;
    let margin = wm.physical_rect(margin);
    let (b, p) = (style.get_border(), style.get_padding());
    let side = |w: app_units::Au, s: style::values::computed::BorderStyle| if s.none_or_hidden() { 0.0 } else { w.to_f32_px() };
    let border = [
        side(b.border_top_width.0, b.border_top_style),
        side(b.border_right_width.0, b.border_right_style),
        side(b.border_bottom_width.0, b.border_bottom_style),
        side(b.border_left_width.0, b.border_left_style),
    ];
    let pad = |v: &style::values::computed::NonNegativeLengthPercentage| lp(&v.0, basis) as f32;
    let padding = [pad(&p.padding_top), pad(&p.padding_right), pad(&p.padding_bottom), pad(&p.padding_left)];
    float_bands(style, (margin_box.width, margin_box.height), [margin.top, margin.right, margin.bottom, margin.left], border, padding, wm, containing_block_width, image)
}

// ---- pyro: float areas from images (CSS Shapes 1 § 3.2) ----

/// The float area of `shape-outside: <image>` in margin-box coordinates: the pixels whose alpha is
/// greater than `shape-image-threshold`, the image sized to the content box as a replaced element
/// of that size would be. `None` (the margin box) while the image is not loaded or failed to.
/// Made of the opaque extent of each physical row, or of each column with `columns` (a vertical
/// writing mode, whose lines run down the columns): a transparent gap across the float context's
/// lines is kept.
fn image_area(
    style: &ComputedValues,
    image: &Image,
    content_box: Rect,
    loaded: Option<&ImageData>,
    columns: bool,
) -> Option<BezPath> {
    let threshold = style.get_box().shape_image_threshold.clamp(0.0, 1.0);
    let (w, h) = (content_box.width(), content_box.height());
    if !(w > 0.0 && h > 0.0) {
        return match crate::util::unfiltered_image(image) {
            Image::Url(_) => loaded.map(|_| BezPath::new()),
            _ => Some(BezPath::new()),
        };
    }
    let rects = match crate::util::unfiltered_image(image) {
        Image::Url(_) => match loaded? {
            ImageData::Raster(raster) => {
                let (iw, ih) = (raster.width as usize, raster.height as usize);
                if iw == 0 || ih == 0 {
                    return Some(BezPath::new());
                }
                let data: &[u8] = raster.data.as_ref();
                if data.len() < iw * ih * 4 {
                    return None;
                }
                let limit = (threshold * 255.0) as f64;
                mask_rects(iw, ih, w, h, columns, |x, y| f64::from(data[(y * iw + x) * 4 + 3]) > limit)
            }
            #[cfg(feature = "svg")]
            ImageData::Svg(svg) => {
                let size = svg.tree.size();
                if !(size.width() > 0.0 && size.height() > 0.0) {
                    return Some(BezPath::new());
                }
                // The rendered image's alpha (strokes, clips, masks, opacity and paint alpha
                // included), at a pixel per CSS px up to 2,048 a side, scaled to the box.
                let (rw, rh) = (w.ceil().clamp(1.0, 2048.0) as u32, h.ceil().clamp(1.0, 2048.0) as u32);
                match rasterized_svg_alpha(&svg.tree, rw, rh) {
                    Some(alpha) => {
                        let limit = (threshold * 255.0) as f64;
                        mask_rects(rw as usize, rh as usize, w, h, columns, |x, y| {
                            f64::from(alpha.0[y * rw as usize + x]) > limit
                        })
                    }
                    None => {
                        // No rasterizer (a document without a renderer): the filled paths.
                        let tree = &svg.tree;
                        let mut fills = Vec::new();
                        svg_fills(tree.root(), 1.0, &mut fills);
                        let to_image =
                            kurbo::Affine::scale_non_uniform(w / size.width() as f64, h / size.height() as f64);
                        let fills: Vec<(BezPath, bool, f64)> =
                            fills.into_iter().map(|(path, even_odd, alpha)| (to_image * path, even_odd, alpha)).collect();
                        sampled_rects(w, h, columns, threshold, |x, y| {
                            let p = Point::new(x, y);
                            fills.iter().fold(0.0, |a, (path, even_odd, alpha)| {
                                let winding = path.winding(p);
                                let inside = if *even_odd { winding % 2 != 0 } else { winding != 0 };
                                if inside { a + alpha * (1.0 - a) } else { a }
                            })
                        })
                    }
                }
            }
            _ => return None,
        },
        Image::Gradient(gradient) => {
            let current = style.get_inherited_text().slow_clone_color();
            let alpha = gradient_alpha(gradient, Rect::new(0.0, 0.0, w, h), &current)?;
            sampled_rects(w, h, columns, threshold, alpha)
        }
        _ => return None,
    };
    let mut path = BezPath::new();
    for r in rects {
        let r = r + content_box.origin().to_vec2();
        path.move_to((r.x0, r.y0));
        path.line_to((r.x1, r.y0));
        path.line_to((r.x1, r.y1));
        path.line_to((r.x0, r.y1));
        path.close_path();
    }
    Some(path)
}

/// The float area of a mask of `cols` by `rows` cells (`opaque(x, y)`) scaled to a `w` by `h`
/// box, as rectangles: each row's (or with `columns` each column's) extent of opaque cells,
/// neighbours with the same extent merged.
fn mask_rects(cols: usize, rows: usize, w: f64, h: f64, columns: bool, opaque: impl Fn(usize, usize) -> bool) -> Vec<Rect> {
    let (sx, sy) = (w / cols as f64, h / rows as f64);
    // Lines across the mask (rows, or columns) and the cells along each.
    let (lines, cells) = if columns { (cols, rows) } else { (rows, cols) };
    let at = |line: usize, cell: usize| if columns { opaque(line, cell) } else { opaque(cell, line) };
    let mut runs: Vec<(usize, usize, usize, usize)> = Vec::new();
    for line in 0..lines {
        let Some(first) = (0..cells).find(|&c| at(line, c)) else { continue };
        let last = (0..cells).rev().find(|&c| at(line, c)).unwrap_or(first);
        if let Some(prev) = runs.last_mut()
            && prev.1 == line
            && prev.2 == first
            && prev.3 == last
        {
            prev.1 = line + 1;
            continue;
        }
        runs.push((line, line + 1, first, last));
    }
    runs.into_iter()
        .map(|(l0, l1, c0, c1)| {
            let (lo, hi) = (l0 as f64, l1 as f64);
            let (a, b) = (c0 as f64, (c1 + 1) as f64);
            if columns {
                Rect::new(lo * sx, a * sy, (hi * sx).min(w), (b * sy).min(h))
            } else {
                Rect::new(a * sx, lo * sy, (b * sx).min(w), (hi * sy).min(h))
            }
        })
        .collect()
}

/// [`mask_rects`] of `alpha` sampled at the centres of a grid over a `w` by `h` box: a cell per
/// px, at most 4,000 a side (a larger box gets larger cells, so all of it is sampled).
fn sampled_rects(w: f64, h: f64, columns: bool, threshold: f32, alpha: impl Fn(f64, f64) -> f64) -> Vec<Rect> {
    let (cols, rows) = (w.ceil().clamp(1.0, 4000.0) as usize, h.ceil().clamp(1.0, 4000.0) as usize);
    let (cw, ch) = (w / cols as f64, h / rows as f64);
    let threshold = f64::from(threshold);
    mask_rects(cols, rows, w, h, columns, |x, y| alpha((x as f64 + 0.5) * cw, (y as f64 + 0.5) * ch) > threshold + 1e-6)
}

/// pyro: renders an SVG image's tree scaled to `width` by `height` px and gives each pixel's alpha
/// (row by row): set by the embedder that has a rasterizer ([`set_svg_rasterizer`]).
#[cfg(feature = "svg")]
pub type SvgRasterizer = fn(&usvg::Tree, u32, u32, &pyro_raster::PageRasters) -> Option<Vec<u8>>;

#[cfg(feature = "svg")]
static SVG_RASTERIZER: std::sync::OnceLock<SvgRasterizer> = std::sync::OnceLock::new();

/// pyro: renders SVG images for `shape-outside` with `rasterizer` (the first one set stays).
#[cfg(feature = "svg")]
pub fn set_svg_rasterizer(rasterizer: SvgRasterizer) {
    let _ = SVG_RASTERIZER.set(rasterizer);
}

/// An SVG image's alpha at `width` by `height` from the rasterizer, the last few kept (a float is
/// laid out again and again with the same image).
#[cfg(feature = "svg")]
fn rasterized_svg_alpha(tree: &crate::node::SvgTree, width: u32, height: u32) -> Option<std::sync::Arc<(Vec<u8>, pyro_raster::Reservation)>> {
    let rasterize = SVG_RASTERIZER.get()?;
    let mut cache = tree.shape_alpha.lock().ok()?;
    if let Some((_, _, alpha)) = cache.as_ref().filter(|(w, h, _)| (*w, *h) == (width, height)) {
        return Some(alpha.clone());
    }
    cache.take();
    let pixels = (width as usize).checked_mul(height as usize)?;
    let charge = tree.page.reserve(pixels).ok()?;
    let alpha = rasterize(tree, width, height, &tree.page).filter(|alpha| alpha.len() == pixels && alpha.capacity() == pixels)?;
    let alpha = std::sync::Arc::new((alpha, charge));
    *cache = Some((width, height, alpha.clone()));
    Some(alpha)
}

/// The filled paths of an SVG image's tree, with their fill rule (`true`: even-odd) and alpha
/// (without a rasterizer, [`set_svg_rasterizer`]).
#[cfg(feature = "svg")]
fn svg_fills(group: &usvg::Group, opacity: f64, out: &mut Vec<(BezPath, bool, f64)>) {
    let opacity = opacity * group.opacity().get() as f64;
    for node in group.children() {
        match node {
            usvg::Node::Group(g) => svg_fills(g, opacity, out),
            usvg::Node::Path(p) if p.is_visible() => {
                let Some(fill) = p.fill() else { continue };
                let mut path = BezPath::new();
                let pt = |p: usvg::tiny_skia_path::Point| Point::new(p.x as f64, p.y as f64);
                for segment in p.data().segments() {
                    use usvg::tiny_skia_path::PathSegment;
                    match segment {
                        PathSegment::MoveTo(a) => path.move_to(pt(a)),
                        PathSegment::LineTo(a) => path.line_to(pt(a)),
                        PathSegment::QuadTo(a, b) => path.quad_to(pt(a), pt(b)),
                        PathSegment::CubicTo(a, b, c) => path.curve_to(pt(a), pt(b), pt(c)),
                        PathSegment::Close => path.close_path(),
                    }
                }
                let t = p.abs_transform();
                path.apply_affine(kurbo::Affine::new([
                    t.sx as f64, t.ky as f64, t.kx as f64, t.sy as f64, t.tx as f64, t.ty as f64,
                ]));
                let even_odd = matches!(fill.rule(), usvg::FillRule::EvenOdd);
                out.push((path, even_odd, opacity * fill.opacity().get() as f64));
            }
            _ => {}
        }
    }
}

/// A gradient's alpha at a point of the box `rect` it fills (CSS Images 3 § 3): the alpha of
/// its colour stops along its line or ray, interpolated (premultiplied, so linearly).
fn gradient_alpha<'a>(
    gradient: &'a style::values::computed::Gradient,
    rect: Rect,
    current: &style::color::AbsoluteColor,
) -> Option<Box<dyn Fn(f64, f64) -> f64 + 'a>> {
    use style::values::generics::image::{GenericCircle, GenericEllipse, GenericEndingShape, GenericGradient, GradientFlags, ShapeExtent};
    use style::values::computed::LineDirection;
    use style::values::specified::position::{HorizontalPositionKeyword, VerticalPositionKeyword};
    match gradient {
        GenericGradient::Linear { direction, items, flags, .. } => {
            let center = rect.center();
            let (w, h) = (rect.width(), rect.height());
            let dir = match direction {
                LineDirection::Angle(angle) => {
                    let a = angle.radians64();
                    kurbo::Vec2::new(a.sin(), -a.cos())
                }
                LineDirection::Horizontal(HorizontalPositionKeyword::Right) => kurbo::Vec2::new(1.0, 0.0),
                LineDirection::Horizontal(HorizontalPositionKeyword::Left) => kurbo::Vec2::new(-1.0, 0.0),
                LineDirection::Vertical(VerticalPositionKeyword::Top) => kurbo::Vec2::new(0.0, -1.0),
                LineDirection::Vertical(VerticalPositionKeyword::Bottom) => kurbo::Vec2::new(0.0, 1.0),
                LineDirection::Corner(horizontal, vertical) => {
                    let sx = if matches!(horizontal, HorizontalPositionKeyword::Right) { 1.0 } else { -1.0 };
                    let sy = if matches!(vertical, VerticalPositionKeyword::Bottom) { 1.0 } else { -1.0 };
                    let norm = w.hypot(h);
                    if norm > 0.0 { kurbo::Vec2::new(sx * h / norm, sy * w / norm) } else { kurbo::Vec2::new(0.0, 1.0) }
                }
            };
            let half = (w / 2.0) * dir.x.abs() + (h / 2.0) * dir.y.abs();
            let start = center - dir * half;
            let length = 2.0 * half;
            let ramp = AlphaRamp::new(items, length, current, flags.contains(GradientFlags::REPEATING));
            Some(Box::new(move |x, y| {
                let t = if length > 0.0 { (Point::new(x, y) - start).dot(dir) / length } else { 0.0 };
                ramp.at(t)
            }))
        }
        GenericGradient::Radial { shape, position, items, flags, .. } => {
            let cx = rect.x0 + lp(&position.horizontal, rect.width());
            let cy = rect.y0 + lp(&position.vertical, rect.height());
            let (dx_near, dx_far) = ((cx - rect.x0).abs().min((rect.x1 - cx).abs()), (cx - rect.x0).abs().max((rect.x1 - cx).abs()));
            let (dy_near, dy_far) = ((cy - rect.y0).abs().min((rect.y1 - cy).abs()), (cy - rect.y0).abs().max((rect.y1 - cy).abs()));
            let (rx, ry) = match shape {
                GenericEndingShape::Circle(GenericCircle::Radius(r)) => (r.0.px() as f64, r.0.px() as f64),
                GenericEndingShape::Circle(GenericCircle::Extent(extent)) => {
                    let r = match extent {
                        ShapeExtent::ClosestSide => dx_near.min(dy_near),
                        ShapeExtent::FarthestSide => dx_far.max(dy_far),
                        ShapeExtent::ClosestCorner => dx_near.hypot(dy_near),
                        _ => dx_far.hypot(dy_far),
                    };
                    (r, r)
                }
                GenericEndingShape::Ellipse(GenericEllipse::Radii(x, y)) => (lp(&x.0, rect.width()), lp(&y.0, rect.height())),
                GenericEndingShape::Ellipse(GenericEllipse::Extent(extent)) => match extent {
                    ShapeExtent::ClosestSide => (dx_near, dy_near),
                    ShapeExtent::FarthestSide => (dx_far, dy_far),
                    ShapeExtent::ClosestCorner => (dx_near * std::f64::consts::SQRT_2, dy_near * std::f64::consts::SQRT_2),
                    _ => (dx_far * std::f64::consts::SQRT_2, dy_far * std::f64::consts::SQRT_2),
                },
            };
            let ramp = AlphaRamp::new(items, rx, current, flags.contains(GradientFlags::REPEATING));
            if !(rx > 0.0 && ry > 0.0) {
                let last = ramp.last();
                return Some(Box::new(move |_, _| last));
            }
            Some(Box::new(move |x, y| ramp.at(((x - cx) / rx).hypot((y - cy) / ry))))
        }
        _ => None,
    }
}

/// A gradient's colour stops' alphas at their offsets along its line (in units of `length`),
/// with transition hints, fixed up as CSS Images 3 § 3.5.3 has it.
struct AlphaRamp {
    stops: Vec<(f64, f64, Option<f64>)>,
    repeating: bool,
}

impl AlphaRamp {
    fn new(
        items: &[style::values::generics::image::GenericGradientItem<style::values::computed::Color, LengthPercentage>],
        length: f64,
        current: &style::color::AbsoluteColor,
        repeating: bool,
    ) -> Self {
        use style::values::generics::image::GenericGradientItem;
        let offset = |p: &LengthPercentage| {
            if length > 0.0 {
                lp(p, length) / length
            } else {
                p.to_percentage().map_or(0.0, |p| p.0 as f64)
            }
        };
        let mut stops: Vec<(Option<f64>, f64, Option<f64>)> = Vec::new();
        let mut hint = None;
        for item in items {
            match item {
                GenericGradientItem::SimpleColorStop(color) => {
                    stops.push((None, color.resolve_to_absolute(current).alpha as f64, hint.take()));
                }
                GenericGradientItem::ComplexColorStop { color, position } => {
                    stops.push((Some(offset(position)), color.resolve_to_absolute(current).alpha as f64, hint.take()));
                }
                GenericGradientItem::InterpolationHint(position) => hint = Some(offset(position)),
            }
        }
        let n = stops.len();
        if n > 0 {
            if stops[0].0.is_none() {
                stops[0].0 = Some(0.0);
            }
            if stops[n - 1].0.is_none() {
                stops[n - 1].0 = Some(1.0);
            }
        }
        let mut max = f64::NEG_INFINITY;
        for stop in &mut stops {
            if let Some(p) = stop.0 {
                let p = p.max(max);
                stop.0 = Some(p);
                max = p;
            }
        }
        let mut i = 0;
        while i < n {
            if stops[i].0.is_none() {
                let start = i - 1;
                let mut end = i;
                while stops[end].0.is_none() {
                    end += 1;
                }
                let (a, b) = (stops[start].0.unwrap_or(0.0), stops[end].0.unwrap_or(1.0));
                for (k, stop) in stops[start + 1..end].iter_mut().enumerate() {
                    stop.0 = Some(a + (b - a) * (k + 1) as f64 / (end - start) as f64);
                }
                i = end;
            }
            i += 1;
        }
        Self { stops: stops.into_iter().map(|(p, a, h)| (p.unwrap_or(0.0), a, h)).collect(), repeating }
    }

    fn last(&self) -> f64 {
        self.stops.last().map_or(0.0, |s| s.1)
    }

    fn at(&self, t: f64) -> f64 {
        let (Some(first), Some(last)) = (self.stops.first(), self.stops.last()) else { return 0.0 };
        let mut t = t;
        if self.repeating {
            let period = last.0 - first.0;
            if !(period > 1e-9) {
                return last.1;
            }
            t = first.0 + (t - first.0).rem_euclid(period);
        }
        if t <= first.0 {
            return first.1;
        }
        // The stop after `t` (the last one at `t` at a hard stop: the colour after it).
        let Some(i) = self.stops.iter().position(|s| s.0 > t) else { return last.1 };
        let (a, b) = (&self.stops[i - 1], &self.stops[i]);
        let span = b.0 - a.0;
        let mut p = if span > 0.0 { (t - a.0) / span } else { 1.0 };
        if let Some(hint) = b.2 {
            let h = if span > 0.0 { ((hint - a.0) / span).clamp(0.0, 1.0) } else { 0.5 };
            p = if h <= 0.0 { 1.0 } else if h >= 1.0 { 0.0 } else { p.powf(0.5f64.ln() / h.ln()) };
        }
        a.1 + (b.1 - a.1) * p
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Attribute, BaseDocument, DocumentConfig, qual_name};
    use style::servo_arc::Arc as ServoArc;

    /// The computed style of a `<div>` with the `style` attribute `css`.
    fn style_of(css: &str) -> ServoArc<ComputedValues> {
        let mut doc = BaseDocument::new(DocumentConfig::default());
        let root = doc.root_node().id;
        let mut mutator = doc.mutate();
        let html = mutator.create_element(qual_name!("html"), vec![]);
        let body = mutator.create_element(qual_name!("body"), vec![]);
        let div = mutator.create_element(
            qual_name!("div"),
            vec![Attribute { name: qual_name!("style"), value: css.to_string() }],
        );
        mutator.append_children(body, &[div]);
        mutator.append_children(html, &[body]);
        mutator.append_children(root, &[html]);
        drop(mutator);
        doc.resolve(0.0);
        let style = doc.nodes[div].primary_styles().expect("styled").clone();
        style
    }

    /// The bands of a float with no margins, borders or paddings whose margin box is `w` by `h`.
    fn bands(css: &str, w: f32, h: f32) -> Vec<Band> {
        let style = style_of(css);
        float_bands(&style, (w, h), [0.0; 4], [0.0; 4], [0.0; 4], style.writing_mode, 1000.0, None).expect("a float area")
    }

    /// The extent of the band at `y`.
    fn at(bands: &[Band], y: f32) -> Option<(f32, f32)> {
        bands.iter().find(|b| b.0 <= y && y < b.1).and_then(|b| b.2)
    }

    #[test]
    fn negative_margin_shrinks_margin_box_radii() {
        assert_eq!(grow_radius(10.0, -4.0), 6.0);
        assert_eq!(grow_radius(3.0, -5.0), 0.0);
        assert_eq!(grow_radius(10.0, 4.0), 14.0);
    }

    #[test]
    fn zero_width_inset_is_a_vertical_edge() {
        let b = bands("float: left; shape-outside: inset(0 50% 0 50%)", 100.0, 100.0);
        for y in [0.5, 50.0, 99.5] {
            assert_eq!(at(&b, y), Some((50.0, 50.0)), "at {y}");
        }
    }

    #[test]
    fn zero_height_inset_needs_a_margin() {
        let b = bands("float: left; shape-outside: inset(50% 0 50% 0)", 100.0, 100.0);
        assert!(b.iter().all(|b| b.2.is_none()), "a horizontal line is an empty float area");
        let b = bands("float: left; shape-outside: inset(50% 0 50% 0); shape-margin: 10px", 100.0, 100.0);
        assert_eq!(at(&b, 45.0), Some((0.0, 100.0)));
        assert_eq!(at(&b, 55.0), Some((0.0, 100.0)));
        assert_eq!(at(&b, 35.0), None);
        assert_eq!(at(&b, 65.0), None);
    }

    #[test]
    fn overlapping_insets_are_reduced_proportionally() {
        // Left 50% and right 150% add up to 200%: reduced to 25% and 75%, an edge at 25px.
        let b = bands("float: left; shape-outside: inset(0 150% 0 50%)", 100.0, 100.0);
        assert_eq!(at(&b, 50.0), Some((25.0, 25.0)));
        // Top 75% and bottom 50% (CSS Shapes 1's example): as inset(60% 0 40% 0), a horizontal
        // line at 60px, which a margin makes a band around.
        let b = bands("float: left; shape-outside: inset(75% 0 50% 0); shape-margin: 5px", 100.0, 100.0);
        assert_eq!(at(&b, 58.0), Some((0.0, 100.0)));
        assert_eq!(at(&b, 50.0), None);
        assert_eq!(at(&b, 70.0), None);
    }

    #[test]
    fn pieces_outside_the_margin_box_take_nothing() {
        // One piece wholly left of the box, one wholly right: nothing of either is in it.
        let b = bands("float: left; shape-outside: path('M -50 0 H -10 V 100 H -50 Z M 110 0 H 150 V 100 H 110 Z')", 100.0, 100.0);
        assert!(b.iter().all(|b| b.2.is_none()), "{b:?}");
        // A piece covering the box with all its edges outside it takes all of it.
        let b = bands("float: left; shape-outside: polygon(-50px -50px, 150px -50px, 150px 150px, -50px 150px)", 100.0, 100.0);
        assert_eq!(at(&b, 50.0), Some((0.0, 100.0)));
        // A piece crossing the box's left edge, with one wholly right of it.
        let b = bands("float: left; shape-outside: path('M -50 0 H 30 V 100 H -50 Z M 110 0 H 150 V 100 H 110 Z')", 100.0, 100.0);
        assert_eq!(at(&b, 50.0), Some((0.0, 30.0)));
    }

    #[test]
    fn even_odd_hole_is_outside() {
        // An even-odd ring whose hole covers the box's right half and beyond: the right of the box
        // is not in the area.
        let b = bands(
            "float: left; shape-outside: polygon(evenodd, -50px 0, 300px 0, 300px 100px, -50px 100px, -50px 0, 40px 0, 40px 100px, 250px 100px, 250px 0, 40px 0)",
            100.0,
            100.0,
        );
        assert_eq!(at(&b, 50.0), Some((0.0, 40.0)));
    }

    #[test]
    fn far_control_points_cost_no_more_than_near_ones() {
        let region = Rect::new(-1.0, -1.0, 101.0, 101.0);
        let mut path = BezPath::new();
        path.move_to((0.0, 0.0));
        for i in 0..50 {
            let s = if i % 2 == 0 { 1e12 } else { -1e12 };
            path.curve_to((s, 0.0), (-s, 100.0), (0.0, 100.0));
            path.curve_to((s * 3.0, 1e11), (s, -1e11), (0.0, 0.0));
        }
        path.close_path();
        let start = std::time::Instant::now();
        let edges = path_edges(&path, region);
        assert!(edges.len() <= 2 * MAX_CURVE_EDGES, "{} edges", edges.len());
        let b = path_bands(&path, 100.0, 100.0, 0.0, false);
        assert_eq!(b.len(), 100);
        assert!(start.elapsed() < std::time::Duration::from_secs(5), "took {:?}", start.elapsed());
        // `path()` in a style, with points far out: done, and clipped to the box.
        let b = bands("float: left; shape-outside: path('M 0 0 C 1e9 0 -1e9 100 0 100 Z')", 100.0, 100.0);
        assert!(b.iter().all(|b| b.2.is_none_or(|(l, r)| l >= 0.0 && r <= 100.0)));
    }

    #[test]
    fn tall_image_shape_is_sampled_all_the_way_down() {
        let b = bands("float: left; shape-outside: linear-gradient(black, black)", 100.0, 5000.0);
        assert_eq!(at(&b, 10.0), Some((0.0, 100.0)));
        assert_eq!(at(&b, 4990.0), Some((0.0, 100.0)));
        // Sampling scales the mask: a grid of at most 4,000 cells still spans the 5,000px.
        let rects = sampled_rects(10.0, 5000.0, false, 0.0, |_, y| if y > 4500.0 { 1.0 } else { 0.0 });
        assert_eq!(rects.len(), 1);
        assert!((rects[0].y0 - 4500.0).abs() < 2.0 && rects[0].y1 == 5000.0, "{:?}", rects[0]);
    }

    #[test]
    fn vertical_image_shape_keeps_gaps_across_lines() {
        // vertical-rl: the float context's rows are the physical columns (mirrored). Opaque
        // physical columns 0..10 and 20..30 with a transparent one between: the rows through
        // the gap are not in the float area.
        let css = "float: left; writing-mode: vertical-rl; shape-outside: linear-gradient(to right, black 0 10px, transparent 10px 20px, black 20px 30px)";
        let b = bands(css, 40.0, 30.0);
        // (x 40 along the inline axis, 30 along the block axis: physical width 30.)
        assert!(at(&b, 5.0).is_some());
        assert_eq!(at(&b, 15.0), None);
        assert!(at(&b, 25.0).is_some());
        // The raster path: columns reduce along physical y.
        let opaque = |x: usize, y: usize| (x < 10 || x >= 20) && y < 5;
        let rects = mask_rects(30, 10, 30.0, 10.0, true, opaque);
        assert!(rects.iter().all(|r| r.x1 <= 10.0 || r.x0 >= 20.0), "{rects:?}");
    }
}
