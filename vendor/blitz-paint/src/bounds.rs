//! pyro: where recorded drawing commands can paint, in the scene's coordinates: the bounds of an
//! element's opacity and filter layer (`BlitzDomPainter::paint_effect_group`). Bounds are
//! conservative (a glyph run's are its glyph origins with an em or two around them) and never
//! smaller than what the commands touch, so the layer does not cut anything off.

use anyrender::Paint;
use anyrender::recording::{FillCommand, RenderCommand, StrokeCommand};
use kurbo::{Join, Rect, Shape};

/// The area `commands` can touch: `None` when they draw nothing.
pub(crate) fn scene_bounds(commands: &[RenderCommand]) -> Option<Rect> {
    let mut at = 0;
    let mut bounds = None;
    // An unbalanced `PopLayer` ends a run: keep going after it.
    while at < commands.len() {
        bounds = union(bounds, layer_bounds(commands, &mut at));
    }
    bounds
}

/// The bounds of the commands from `*at` to the `PopLayer` that ends their layer (or the end),
/// leaving `*at` after that `PopLayer`.
fn layer_bounds(commands: &[RenderCommand], at: &mut usize) -> Option<Rect> {
    let mut bounds = None;
    while let Some(command) = commands.get(*at) {
        *at += 1;
        let painted = match command {
            RenderCommand::PopLayer => return bounds,
            // pyro (lane H2): a projective layer's commands are in its plane's own pixels: it paints
            // its clip (`render/three_d.rs`).
            RenderCommand::PushLayer(layer)
                if layer.filter.as_deref().is_some_and(crate::render::is_projective_marker) =>
            {
                layer_bounds(commands, at);
                Some(layer.transform.transform_rect_bbox(layer.clip.bounding_box()))
            }
            RenderCommand::PushLayer(layer) => {
                let clip = layer.transform.transform_rect_bbox(layer.clip.bounding_box());
                let inner = layer_bounds(commands, at);
                // A filter spreads the content (a blur, a shadow's offset); a backdrop filter,
                // and a filter that paints where its input is transparent (an SVG graph's flood,
                // turbulence, tile or colour matrix adding alpha: `stays_with_input`), paint the
                // whole clip, whatever the content (an empty element's filter region).
                let inner = match (&layer.filter, inner) {
                    (Some(filter), Some(inner)) => {
                        // The filter's reach in scene units: the layer transform's linear part (its
                        // translation would move the reach off the content).
                        let reach = filter.linear_bounds_expansion(&layer.transform);
                        Some(Rect::new(
                            inner.x0 + reach.x0,
                            inner.y0 + reach.y0,
                            inner.x1 + reach.x1,
                            inner.y1 + reach.y1,
                        ))
                    }
                    (_, inner) => inner,
                };
                let fills = layer.filter.as_deref().is_some_and(|filter| !stays_with_input(filter));
                let inner = if layer.backdrop_filter.is_some() || fills { Some(clip) } else { inner };
                inner.map(|inner| inner.intersect(clip))
            }
            RenderCommand::PushClipLayer(layer) => {
                let clip = layer.transform.transform_rect_bbox(layer.clip.bounding_box());
                layer_bounds(commands, at).map(|inner| inner.intersect(clip))
            }
            RenderCommand::Fill(fill) => Some(fill.transform.transform_rect_bbox(fill.shape.bounding_box())),
            RenderCommand::Stroke(stroke) => {
                let style = &stroke.style;
                // Half the width out, a miter's tip up to `miter_limit` times that.
                let miter = if style.join == Join::Miter { style.miter_limit.max(1.0) } else { 1.0 };
                let reach = style.width * 0.5 * miter.max(std::f64::consts::SQRT_2) + 1.0;
                Some(stroke.transform.transform_rect_bbox(stroke.shape.bounding_box().inflate(reach, reach)))
            }
            RenderCommand::GlyphRun(run) => {
                let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
                for glyph in &run.glyphs {
                    x0 = x0.min(f64::from(glyph.x));
                    y0 = y0.min(f64::from(glyph.y));
                    x1 = x1.max(f64::from(glyph.x));
                    y1 = y1.max(f64::from(glyph.y));
                }
                if run.glyphs.is_empty() {
                    None
                } else {
                    // Glyphs reach about an em above their baseline origin and a little below;
                    // a skewed or rotated glyph (synthetic italic, upright vertical text) further.
                    let em = f64::from(run.font_size).abs() * if run.glyph_transform.is_some() { 3.0 } else { 1.0 };
                    let embolden = run.embolden.x.abs().max(run.embolden.y.abs());
                    let ink = Rect::new(x0 - em, y0 - em * 1.5, x1 + em * 2.0, y1 + em).inflate(embolden, embolden);
                    Some(run.transform.transform_rect_bbox(ink))
                }
            }
            RenderCommand::BoxShadow(shadow) => {
                let reach = shadow.std_dev.abs() * 3.0 + 1.0;
                Some(shadow.transform.transform_rect_bbox(shadow.rect.inflate(reach, reach)))
            }
        };
        bounds = union(bounds, painted.filter(|r| r.width() > 0.0 && r.height() > 0.0));
    }
    bounds
}

/// pyro: whether `filter` draws only within its expansion of where its input draws: each node is
/// a blur, an offset, a drop shadow, or a colour function that keeps transparent black transparent
/// (a matrix whose alpha row adds nothing, an alpha transfer mapping 0 to 0). CSS filter functions
/// are; an SVG filter graph with a flood, turbulence, a tile, an image, a displacement, morphology
/// or a composite may draw anywhere in its region.
pub(crate) fn stays_with_input(filter: &anyrender::Filter) -> bool {
    use anyrender::filters::FilterEffect;
    use anyrender::filters::component_transfer::TransferFunction;
    filter.nodes().iter().all(|node| match &node.effect {
        FilterEffect::GaussianBlur(_) | FilterEffect::Offset(_) | FilterEffect::DropShadow(_) => true,
        FilterEffect::ColorMatrix(m) => m.0[19] <= 0.0,
        FilterEffect::ComponentTransfer(t) => match &t.alpha_function {
            TransferFunction::Identity => true,
            TransferFunction::Table(values) => values.first().is_none_or(|v| *v <= 0.0),
            TransferFunction::Discrete(values) => values.first().is_none_or(|v| *v <= 0.0),
            TransferFunction::Linear(f) => f.intercept <= 0.0,
            TransferFunction::Gamma(f) => f.offset <= 0.0,
        },
        _ => false,
    })
}

/// pyro: when `commands` draw a single shape (one fill, stroke or box shadow of a solid colour,
/// within clips), folds `opacity` into its colour and returns true: the opacity of a group of one
/// shape is its colour's alpha times it, so the group needs no layer (Skia folds a layer's alpha
/// into its single draw the same way). A glyph run is not one shape (its glyphs overlap).
pub(crate) fn fold_opacity(commands: &mut [RenderCommand], opacity: f32) -> bool {
    let mut draws = commands
        .iter_mut()
        .filter(|command| !matches!(command, RenderCommand::PushClipLayer(_) | RenderCommand::PopLayer));
    let (Some(draw), None) = (draws.next(), draws.next()) else {
        return false;
    };
    match draw {
        RenderCommand::Fill(FillCommand { brush: Paint::Solid(color), .. })
        | RenderCommand::Stroke(StrokeCommand { brush: Paint::Solid(color), .. }) => {
            *color = color.multiply_alpha(opacity);
            true
        }
        RenderCommand::BoxShadow(shadow) => {
            shadow.brush = shadow.brush.multiply_alpha(opacity);
            true
        }
        _ => false,
    }
}

fn union(a: Option<Rect>, b: Option<Rect>) -> Option<Rect> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.union(b)),
        (a, None) => a,
        (None, b) => b,
    }
}

/// pyro: whether a clip layer of `clip`, drawn with `transform` into device pixels, would leave
/// every device pixel of `ink` (a rectangle in device pixels) as it is: each pixel the rectangle
/// touches lies wholly inside the clip, where its coverage is full. The clip must be one contour
/// of straight edges along its bounding box with curves in the corners between them (a rectangle
/// or a rounded rectangle, as overflow clips are), and `transform` may only scale and translate;
/// otherwise this answers `false`. A pixel inside the box's whole pixels and off every corner's
/// curves is covered in whole. A coverage within 1/1024 of full or empty rounds to it in 8 bits,
/// so an edge that close to a pixel boundary counts as on it.
pub(crate) fn clip_covers_pixels(clip: &kurbo::BezPath, transform: kurbo::Affine, ink: Rect) -> bool {
    use kurbo::{PathEl, Point};
    const EPS: f64 = 1.0 / 1024.0;
    let [a, b, c, d, e, f] = transform.as_coeffs();
    if b.abs() > 1e-12 || c.abs() > 1e-12 || !ink.is_finite() || !transform.is_finite() {
        return false;
    }
    let map = |p: Point| Point::new(a * p.x + e, d * p.y + f);
    // The contour's segments in device pixels: each from the point before it, with its points
    // (a curve's control points and end point; a line's end point).
    let mut start: Option<Point> = None;
    let mut at = Point::ZERO;
    let mut segments: Vec<(Point, [Point; 3], usize)> = Vec::with_capacity(16);
    for element in clip.elements() {
        match *element {
            PathEl::MoveTo(p) => {
                if start.is_some() {
                    return false;
                }
                start = Some(map(p));
                at = map(p);
            }
            PathEl::LineTo(p) => {
                let p = map(p);
                segments.push((at, [p, p, p], 1));
                at = p;
            }
            PathEl::QuadTo(p1, p2) => {
                let (p1, p2) = (map(p1), map(p2));
                segments.push((at, [p1, p2, p2], 2));
                at = p2;
            }
            PathEl::CurveTo(p1, p2, p3) => {
                let (p1, p2, p3) = (map(p1), map(p2), map(p3));
                segments.push((at, [p1, p2, p3], 3));
                at = p3;
            }
            PathEl::ClosePath => {}
        }
    }
    let Some(start) = start else { return false };
    // The fill closes the contour with a line back to its start.
    if at != start {
        segments.push((at, [start, start, start], 1));
    }
    let mut bbox = Rect::from_points(start, start);
    let mut twice_area = 0.0;
    for (from, points, n) in &segments {
        for p in &points[..*n] {
            bbox = bbox.union_pt(*p);
        }
        let to = points[n - 1];
        twice_area += from.x * to.y - to.x * from.y;
    }
    // Straight edges lie on the box; each run of curves between them is a corner, which can cut
    // into nothing outside its own box.
    let near = |u: f64, v: f64| (u - v).abs() <= EPS;
    let mut corners: Vec<Rect> = Vec::new();
    let mut corner: Option<Rect> = None;
    for (from, points, n) in &segments {
        if *n == 1 {
            if let Some(done) = corner.take() {
                corners.push(done);
            }
            let to = points[0];
            let on_box = if near(from.x, to.x) {
                near(from.x, bbox.x0) || near(from.x, bbox.x1)
            } else if near(from.y, to.y) {
                near(from.y, bbox.y0) || near(from.y, bbox.y1)
            } else {
                false
            };
            if !on_box {
                return false;
            }
        } else {
            let mut piece = Rect::from_points(*from, *from);
            for p in &points[..*n] {
                piece = piece.union_pt(*p);
            }
            corner = Some(corner.map_or(piece, |c| c.union(piece)));
        }
    }
    if let Some(done) = corner {
        // (A run of curves at the end continues the one the contour starts with.)
        match segments.first() {
            Some((_, _, n)) if *n > 1 && !corners.is_empty() => corners[0] = corners[0].union(done),
            _ => corners.push(done),
        }
    }
    // The contour goes round the box once: its polygon holds at least the box less the corners
    // (a degenerate one doubling back on itself holds nothing).
    let corner_area: f64 = corners.iter().map(|c| c.area()).sum();
    if twice_area.abs() * 0.5 < bbox.area() - corner_area - EPS {
        return false;
    }
    // The pixels inside the box, and those the ink touches.
    let inside = Rect::new((bbox.x0 - EPS).ceil(), (bbox.y0 - EPS).ceil(), (bbox.x1 + EPS).floor(), (bbox.y1 + EPS).floor());
    let touched = Rect::new((ink.x0 + EPS).floor(), (ink.y0 + EPS).floor(), (ink.x1 - EPS).ceil(), (ink.y1 - EPS).ceil());
    if touched.x0 >= touched.x1 || touched.y0 >= touched.y1 {
        return true;
    }
    if touched.x0 < inside.x0 || touched.y0 < inside.y0 || touched.x1 > inside.x1 || touched.y1 > inside.y1 {
        return false;
    }
    !corners.iter().any(|c| {
        touched.x0 < c.x1 - EPS && touched.x1 > c.x0 + EPS && touched.y0 < c.y1 - EPS && touched.y1 > c.y0 + EPS
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyrender::{PaintScene, Scene};
    use kurbo::Affine;
    use peniko::{Fill, Mix, color::palette::css::BLACK};

    #[test]
    fn a_clip_bounds_what_it_holds_and_nothing_is_nothing() {
        let mut scene = Scene::new();
        assert_eq!(scene_bounds(&scene.commands), None);
        scene.fill(Fill::NonZero, Affine::translate((10.0, 0.0)), BLACK, None, &Rect::new(0.0, 0.0, 5.0, 5.0));
        scene.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &Rect::new(0.0, 0.0, 100.0, 100.0));
        scene.fill(Fill::NonZero, Affine::IDENTITY, BLACK, None, &Rect::new(50.0, 50.0, 500.0, 500.0));
        scene.pop_layer();
        scene.push_layer(Fill::NonZero, Mix::Normal, 0.5, Affine::IDENTITY, &Rect::new(0.0, 0.0, 9.0, 9.0), None, None);
        scene.pop_layer();
        assert_eq!(scene_bounds(&scene.commands), Some(Rect::new(10.0, 0.0, 100.0, 100.0)));
    }

    /// A filter layer's reach grows its content's bounds where the content is, however far the
    /// layer is moved.
    #[test]
    fn a_moved_filter_layer_bounds_its_content_and_the_reach() {
        use anyrender::filters::{Filter, FilterEffect};
        let mut scene = Scene::new();
        let moved = Affine::translate((110.0, 90.0));
        let shadow = FilterEffect::drop_shadow(10.0, 10.0, 0.0, BLACK);
        let filter = Some(std::sync::Arc::new(Filter::single(shadow)));
        let clip = Rect::new(-50.0, -50.0, 200.0, 200.0);
        scene.push_layer(Fill::NonZero, Mix::Normal, 1.0, moved, &clip, filter, None);
        scene.fill(Fill::NonZero, moved, BLACK, None, &Rect::new(0.0, 0.0, 100.0, 60.0));
        scene.pop_layer();
        assert_eq!(scene_bounds(&scene.commands), Some(Rect::new(110.0, 90.0, 220.0, 160.0)));
    }

    /// A filter that paints where its content is not (a flood, a colour matrix adding alpha)
    /// covers its whole clip, with no content at all too (an empty element's SVG filter region);
    /// a colour function that keeps transparent black transparent does not.
    #[test]
    fn a_filter_painting_past_its_content_bounds_its_clip() {
        use anyrender::filters::color_transformation::ColorMatrix;
        use anyrender::filters::{Filter, FilterEffect};
        let clip = Rect::new(-10.0, -10.0, 90.0, 90.0);
        let layer = |effect: FilterEffect, content: bool| {
            let mut scene = Scene::new();
            let filter = Some(std::sync::Arc::new(Filter::single(effect)));
            scene.push_layer(Fill::NonZero, Mix::Normal, 1.0, Affine::IDENTITY, &clip, filter, None);
            if content {
                scene.fill(Fill::NonZero, Affine::IDENTITY, BLACK, None, &Rect::new(0.0, 0.0, 10.0, 10.0));
            }
            scene.pop_layer();
            scene_bounds(&scene.commands)
        };
        assert_eq!(layer(FilterEffect::Flood(BLACK), false), Some(clip));
        assert_eq!(layer(FilterEffect::Flood(BLACK), true), Some(clip));
        let mut adds_alpha = ColorMatrix::opacity(1.0);
        adds_alpha.0[19] = 0.2;
        assert_eq!(layer(FilterEffect::ColorMatrix(adds_alpha), false), Some(clip));
        assert_eq!(layer(FilterEffect::ColorMatrix(ColorMatrix::opacity(0.5)), false), None);
        assert_eq!(layer(FilterEffect::blur(2.0), true), Some(Rect::new(-6.0, -6.0, 16.0, 16.0)));
    }

    /// A clip covers the whole pixels inside its box and off its corners' curves: an edge between
    /// pixels covers the pixel it cuts only in part, in device pixels after the transform.
    #[test]
    fn a_clip_covers_whole_pixels_off_its_curves() {
        use kurbo::{BezPath, RoundedRect};
        let id = Affine::IDENTITY;
        let rect = Rect::new(10.0, 20.0, 110.0, 70.0).to_path(0.1);
        assert!(clip_covers_pixels(&rect, id, Rect::new(30.0, 30.0, 60.0, 50.0)));
        assert!(clip_covers_pixels(&rect, id, Rect::new(10.0, 20.0, 110.0, 70.0)), "flush with whole pixels");
        assert!(!clip_covers_pixels(&rect, id, Rect::new(9.5, 30.0, 60.0, 50.0)));
        assert!(clip_covers_pixels(&rect, id, Rect::new(30.0, 30.0, 30.0, 50.0)), "nothing");
        let half = Rect::new(10.5, 20.0, 110.0, 70.0).to_path(0.1);
        assert!(!clip_covers_pixels(&half, id, Rect::new(10.5, 30.0, 60.0, 50.0)), "the cut pixel");
        assert!(clip_covers_pixels(&half, id, Rect::new(11.0, 30.0, 60.0, 50.0)));
        // At 3x the edge is at 31.5 device px.
        let three = Affine::scale(3.0);
        assert!(!clip_covers_pixels(&half, three, Rect::new(31.5, 90.0, 150.0, 150.0)));
        assert!(clip_covers_pixels(&half, three, Rect::new(32.0, 60.0, 330.0, 210.0)));
        let rounded = RoundedRect::new(10.0, 20.0, 110.0, 70.0, 8.0).to_path(0.1);
        assert!(clip_covers_pixels(&rounded, id, Rect::new(10.0, 28.0, 110.0, 62.0)), "between the corners");
        assert!(clip_covers_pixels(&rounded, id, Rect::new(18.0, 20.0, 102.0, 70.0)));
        assert!(!clip_covers_pixels(&rounded, id, Rect::new(12.0, 22.0, 30.0, 40.0)), "in a corner");
        assert!(!clip_covers_pixels(&rounded, id, Rect::new(100.0, 60.0, 109.0, 69.0)));
        // A rotated clip, or one that is no rounded rectangle, is not told.
        assert!(!clip_covers_pixels(&rect, Affine::rotate(0.1), Rect::new(30.0, 30.0, 40.0, 40.0)));
        let mut triangle = BezPath::new();
        triangle.move_to((0.0, 0.0));
        triangle.line_to((100.0, 0.0));
        triangle.line_to((0.0, 100.0));
        triangle.close_path();
        assert!(!clip_covers_pixels(&triangle, id, Rect::new(1.0, 1.0, 2.0, 2.0)));
        let mut doubled_back = BezPath::new();
        doubled_back.move_to((0.0, 0.0));
        doubled_back.line_to((100.0, 0.0));
        doubled_back.line_to((0.0, 0.0));
        doubled_back.line_to((0.0, 100.0));
        doubled_back.close_path();
        assert!(!clip_covers_pixels(&doubled_back, id, Rect::new(10.0, 10.0, 20.0, 20.0)));
    }

    #[test]
    fn only_a_single_solid_shape_takes_the_opacity_into_its_colour() {
        let shape = Rect::new(0.0, 0.0, 5.0, 5.0);
        let mut one = Scene::new();
        one.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &shape);
        one.fill(Fill::NonZero, Affine::IDENTITY, BLACK, None, &shape);
        one.pop_layer();
        assert!(fold_opacity(&mut one.commands, 0.5));
        assert!(
            matches!(&one.commands[1], RenderCommand::Fill(f) if f.brush == Paint::Solid(BLACK.multiply_alpha(0.5)))
        );

        let mut two = Scene::new();
        two.fill(Fill::NonZero, Affine::IDENTITY, BLACK, None, &shape);
        two.fill(Fill::NonZero, Affine::IDENTITY, BLACK, None, &shape);
        assert!(!fold_opacity(&mut two.commands, 0.5));
        assert!(matches!(&two.commands[0], RenderCommand::Fill(f) if f.brush == Paint::Solid(BLACK)));

        let mut layered = Scene::new();
        layered.push_layer(Fill::NonZero, Mix::Normal, 0.5, Affine::IDENTITY, &shape, None, None);
        layered.fill(Fill::NonZero, Affine::IDENTITY, BLACK, None, &shape);
        layered.pop_layer();
        assert!(!fold_opacity(&mut layered.commands, 0.5));
    }
}
