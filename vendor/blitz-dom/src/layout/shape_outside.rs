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
use style::values::computed::{CSSPixelLength, LengthPercentage};
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
/// `m (1 + (r / m - 1)³)` where it is smaller.
fn grow_radius(r: f64, m: f64) -> f64 {
    if m <= 0.0 || r >= m {
        return r + m.max(0.0);
    }
    r + m * (1.0 + (r / m - 1.0).powi(3))
}

/// The float area as a path in margin-box coordinates, `None` for the margin box itself.
fn area(style: &ComputedValues, margin_box: Rect, margin: [f64; 4], border: [f64; 4], padding: [f64; 4]) -> Option<BezPath> {
    let inset = |r: Rect, e: [f64; 4]| Rect::new(r.x0 + e[3], r.y0 + e[0], r.x1 - e[1], r.y1 - e[2]);
    let border_box = inset(margin_box, margin);
    let reference = |b: ShapeBox| match b {
        ShapeBox::MarginBox => margin_box,
        ShapeBox::BorderBox => border_box,
        ShapeBox::PaddingBox => inset(border_box, border),
        ShapeBox::ContentBox => inset(inset(border_box, border), padding),
    };
    match &style.get_box().shape_outside {
        ShapeOutside::None | ShapeOutside::Image(_) => None,
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
                    kurbo::Circle::new(center, radius.max(0.0)).to_path(0.1)
                }
                GenericBasicShape::Ellipse(e) => {
                    let center = position(&e.position, r);
                    let rx = axis_radius(&e.semiaxis_x, center.x, r.x0, r.x1, w).max(0.0);
                    let ry = axis_radius(&e.semiaxis_y, center.y, r.y0, r.y1, h).max(0.0);
                    kurbo::Ellipse::new(center, (rx, ry), 0.0).to_path(0.1)
                }
                GenericBasicShape::Rect(inset) => {
                    let rect = Rect::new(
                        r.x0 + lp(&inset.rect.3, w),
                        r.y0 + lp(&inset.rect.0, h),
                        r.x1 - lp(&inset.rect.1, w),
                        r.y1 - lp(&inset.rect.2, h),
                    );
                    if !(rect.x1 > rect.x0 && rect.y1 > rect.y0) {
                        return Some(BezPath::new());
                    }
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
                    let mut path = BezPath::new();
                    for (i, c) in poly.coordinates.iter().enumerate() {
                        let p = Point::new(r.x0 + lp(&c.0, w), r.y0 + lp(&c.1, h));
                        if i == 0 {
                            path.move_to(p);
                        } else {
                            path.line_to(p);
                        }
                    }
                    path.close_path();
                    path
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
    let mut path = area(style, margin_box, f(margin), f(border), f(padding))?;
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
    // The path's edges, flattened.
    let mut edges = Vec::new();
    let (mut start, mut last) = (Point::ZERO, Point::ZERO);
    kurbo::flatten(&path, 0.25, |el| match el {
        PathEl::MoveTo(p) => {
            start = p;
            last = p;
        }
        PathEl::LineTo(p) => {
            edges.push((last, p));
            last = p;
        }
        PathEl::ClosePath => {
            edges.push((last, start));
            last = start;
        }
        _ => {}
    });
    let height = size.1 as f64;
    if height <= 0.0 {
        return None;
    }
    // Rows of 1px (at most 2,000 of them).
    let step = (height / 2000.0).max(1.0);
    let mut bands = Vec::new();
    let mut y = 0.0;
    while y < height {
        let y1 = (y + step).min(height);
        // The extent of the area grown by `shape-margin` (the points within that distance of
        // it) over the row: the extremes of the edges' grown extents.
        let mut extent: Option<(f64, f64)> = None;
        for &(a, b) in &edges {
            let mirror = |p: Point| Point::new(-p.x, p.y);
            let (Some(r), Some(l)) = (edge_max_x(a, b, y, y1, shape_margin), edge_max_x(mirror(a), mirror(b), y, y1, shape_margin))
            else {
                continue;
            };
            let l = -l;
            extent = Some(extent.map_or((l, r), |(el, er)| (el.min(l), er.max(r))));
        }
        let extent = extent.map(|(l, r)| (l.max(0.0) as f32, r.min(size.0 as f64) as f32));
        bands.push((y as f32, y1 as f32, extent.filter(|(l, r)| r > l || shape_margin > 0.0)));
        y = y1;
    }
    Some(bands)
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
    float_bands(style, (margin_box.width, margin_box.height), [margin.top, margin.right, margin.bottom, margin.left], border, padding, wm, containing_block_width)
}
