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
/// `m (1 + (r / m - 1)³)` where it is smaller.
fn grow_radius(r: f64, m: f64) -> f64 {
    if m <= 0.0 || r >= m {
        return r + m.max(0.0);
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
        ShapeOutside::Image(img) => image_area(style, img, reference(ShapeBox::ContentBox), image),
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
    let mut path = area(style, margin_box, f(margin), f(border), f(padding), image)?;
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
    kurbo::flatten(&path, 0.005, |el| match el {
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
fn image_area(style: &ComputedValues, image: &Image, content_box: Rect, loaded: Option<&ImageData>) -> Option<BezPath> {
    let threshold = style.get_box().shape_image_threshold.clamp(0.0, 1.0);
    let (w, h) = (content_box.width(), content_box.height());
    let mut rows: Vec<(f64, f64, f64, f64)> = Vec::new();
    let mut push = |y0: f64, y1: f64, extent: Option<(f64, f64)>| {
        let Some((x0, x1)) = extent else { return };
        if let Some(last) = rows.last_mut()
            && (last.1 - y0).abs() < 1e-9
            && (last.2 - x0).abs() < 1e-9
            && (last.3 - x1).abs() < 1e-9
        {
            last.1 = y1;
            return;
        }
        rows.push((y0, y1, x0, x1));
    };
    match crate::util::unfiltered_image(image) {
        Image::Url(_) => match loaded? {
            ImageData::Raster(raster) => {
                let (iw, ih) = (raster.width as usize, raster.height as usize);
                if iw == 0 || ih == 0 || w <= 0.0 || h <= 0.0 {
                    return Some(BezPath::new());
                }
                let data: &[u8] = raster.data.as_ref();
                if data.len() < iw * ih * 4 {
                    return None;
                }
                let limit = (threshold * 255.0) as f64;
                let (sx, sy) = (w / iw as f64, h / ih as f64);
                for row in 0..ih {
                    let line = &data[row * iw * 4..(row + 1) * iw * 4];
                    let above = |i: usize| f64::from(line[i * 4 + 3]) > limit;
                    let first = (0..iw).find(|&i| above(i));
                    let extent = first.map(|first| {
                        let last = (0..iw).rev().find(|&i| above(i)).unwrap_or(first);
                        (first as f64 * sx, (last + 1) as f64 * sx)
                    });
                    push(row as f64 * sy, (row + 1) as f64 * sy, extent);
                }
            }
            #[cfg(feature = "svg")]
            ImageData::Svg(svg) => {
                let tree = &svg.tree;
                let size = tree.size();
                let (tw, th) = (size.width() as f64, size.height() as f64);
                if !(tw > 0.0 && th > 0.0) {
                    return Some(BezPath::new());
                }
                let mut fills = Vec::new();
                svg_fills(tree.root(), 1.0, &mut fills);
                let to_image = kurbo::Affine::scale_non_uniform(w / tw, h / th);
                let fills: Vec<(BezPath, bool, f64)> =
                    fills.into_iter().map(|(path, even_odd, alpha)| (to_image * path, even_odd, alpha)).collect();
                sample_rows(w, h, |x, y| {
                    let p = Point::new(x, y);
                    fills.iter().fold(0.0, |a, (path, even_odd, alpha)| {
                        let winding = path.winding(p);
                        let inside = if *even_odd { winding % 2 != 0 } else { winding != 0 };
                        if inside { a + alpha * (1.0 - a) } else { a }
                    })
                }, threshold, &mut push);
            }
            _ => return None,
        },
        Image::Gradient(gradient) => {
            let current = style.get_inherited_text().slow_clone_color();
            let alpha = gradient_alpha(gradient, Rect::new(0.0, 0.0, w, h), &current)?;
            sample_rows(w, h, alpha, threshold, &mut push);
        }
        _ => return None,
    }
    let mut path = BezPath::new();
    for (y0, y1, x0, x1) in rows {
        let r = Rect::new(content_box.x0 + x0, content_box.y0 + y0, content_box.x0 + x1, content_box.y0 + y1);
        path.move_to((r.x0, r.y0));
        path.line_to((r.x1, r.y0));
        path.line_to((r.x1, r.y1));
        path.line_to((r.x0, r.y1));
        path.close_path();
    }
    Some(path)
}

/// Rows of 1px of a `w` by `h` box sampled at pixel centres: each row's extent of the samples
/// whose `alpha` is greater than `threshold`.
fn sample_rows(
    w: f64,
    h: f64,
    alpha: impl Fn(f64, f64) -> f64,
    threshold: f32,
    push: &mut impl FnMut(f64, f64, Option<(f64, f64)>),
) {
    let (cols, rows) = (w.ceil().clamp(0.0, 4000.0) as usize, h.ceil().clamp(0.0, 4000.0) as usize);
    let threshold = f64::from(threshold);
    for row in 0..rows {
        let (y0, y1) = (row as f64, ((row + 1) as f64).min(h));
        let y = (y0 + y1) / 2.0;
        let x_of = |i: usize| (i as f64 + ((i + 1) as f64).min(w)) / 2.0;
        let above = |i: usize| alpha(x_of(i), y) > threshold + 1e-6;
        let first = (0..cols).find(|&i| above(i));
        let extent = first.map(|first| {
            let last = (0..cols).rev().find(|&i| above(i)).unwrap_or(first);
            (first as f64, ((last + 1) as f64).min(w))
        });
        push(y0, y1, extent);
    }
}

/// The filled paths of an SVG image's tree, with their fill rule (`true`: even-odd) and alpha.
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
