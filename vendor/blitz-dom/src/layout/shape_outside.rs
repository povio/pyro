//! pyro: a float's float area from `shape-outside` (CSS Shapes 1 § 3), as rows of its margin box
//! with the shape's horizontal extent in each, for Taffy's float context
//! (`BlockContext::place_shaped_floated_box`): line boxes beside the float wrap around the shape.
//!
//! Basic shapes (`circle()`, `ellipse()`, `inset()`, `polygon()`) in their reference box and the
//! box keywords (the reference box itself, corners square) and `path()`, grown by `shape-margin`.
//! Images and `shape()` keep the margin box.

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

/// The float area as a path in margin-box coordinates, `None` for the margin box itself.
fn area(style: &ComputedValues, margin_box: Rect, margin: [f64; 4], border: [f64; 4], padding: [f64; 4]) -> Option<BezPath> {
    let reference = |b: ShapeBox| {
        let inset = |r: Rect, e: [f64; 4]| Rect::new(r.x0 + e[3], r.y0 + e[0], r.x1 - e[1], r.y1 - e[2]);
        let border_box = inset(margin_box, margin);
        match b {
            ShapeBox::MarginBox => margin_box,
            ShapeBox::BorderBox => border_box,
            ShapeBox::PaddingBox => inset(border_box, border),
            ShapeBox::ContentBox => inset(inset(border_box, border), padding),
        }
    };
    match &style.get_box().shape_outside {
        ShapeOutside::None | ShapeOutside::Image(_) => None,
        ShapeOutside::Box(ShapeBox::MarginBox) => None,
        ShapeOutside::Box(b) => Some(reference(*b).to_path(0.1)),
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
                    rect.to_path(0.1)
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
                    let mut d = String::new();
                    let _ = path.path.to_css(&mut style_traits::CssWriter::new(&mut d), false);
                    let mut bez = BezPath::from_svg(&d).ok()?;
                    bez.apply_affine(kurbo::Affine::translate((r.x0, r.y0)));
                    bez
                }
                // `shape()`: the margin box (not yet).
                GenericBasicShape::PathOrShape(_) => return None,
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
) -> Option<Vec<Band>> {
    let f = |e: [f32; 4]| e.map(f64::from);
    let margin_box = Rect::new(0.0, 0.0, size.0 as f64, size.1 as f64);
    let path = area(style, margin_box, f(margin), f(border), f(padding))?;
    let shape_margin = lp(&style.get_box().shape_margin.0, size.0 as f64).max(0.0);
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
        // The area's extent over the row grown by `shape-margin` vertically.
        let (lo, hi) = (y - shape_margin, y1 + shape_margin);
        let mut extent: Option<(f64, f64)> = None;
        for &(a, b) in &edges {
            let (top, bottom) = if a.y <= b.y { (a, b) } else { (b, a) };
            if bottom.y < lo || top.y > hi {
                continue;
            }
            // The edge's part within the row.
            let x_at = |yy: f64| {
                if (bottom.y - top.y).abs() < 1e-9 {
                    top.x
                } else {
                    top.x + (bottom.x - top.x) * ((yy - top.y) / (bottom.y - top.y))
                }
            };
            let y_from = top.y.max(lo);
            let y_to = bottom.y.min(hi);
            for x in [x_at(y_from), x_at(y_to)] {
                extent = Some(extent.map_or((x, x), |(l, r)| (l.min(x), r.max(x))));
            }
        }
        let extent = extent.map(|(l, r)| {
            let l = (l - shape_margin).max(0.0);
            let r = (r + shape_margin).min(size.0 as f64);
            (l as f32, r as f32)
        });
        bands.push((y as f32, y1 as f32, extent.filter(|(l, r)| r > l || shape_margin > 0.0)));
        y = y1;
    }
    Some(bands)
}

/// [`float_bands`] for a float's computed style, its margin box and margins: the border widths and
/// paddings (percentages of the margin box's width less the margins) come from the style.
pub(crate) fn float_bands_for(
    style: &ComputedValues,
    margin_box: taffy::Size<f32>,
    margin: taffy::Rect<f32>,
) -> Option<Vec<Band>> {
    if matches!(style.get_box().shape_outside, ShapeOutside::None) {
        return None;
    }
    let (b, p) = (style.get_border(), style.get_padding());
    let side = |w: app_units::Au, s: style::values::computed::BorderStyle| if s.none_or_hidden() { 0.0 } else { w.to_f32_px() };
    let border = [
        side(b.border_top_width.0, b.border_top_style),
        side(b.border_right_width.0, b.border_right_style),
        side(b.border_bottom_width.0, b.border_bottom_style),
        side(b.border_left_width.0, b.border_left_style),
    ];
    let basis = (margin_box.width - margin.left - margin.right).max(0.0) as f64;
    let pad = |v: &style::values::computed::NonNegativeLengthPercentage| lp(&v.0, basis) as f32;
    let padding = [pad(&p.padding_top), pad(&p.padding_right), pad(&p.padding_bottom), pad(&p.padding_left)];
    float_bands(style, (margin_box.width, margin_box.height), [margin.top, margin.right, margin.bottom, margin.left], border, padding)
}
