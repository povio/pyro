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
