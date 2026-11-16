use super::ElementCx;
use crate::color::{Color, ToColorColor as _};
use crate::kurbo_css::{CssBox, NonUniformRoundedRectRadii};
use anyrender::PaintScene;
use blitz_dom::CopyStyleGetters;
use kurbo::{Insets, Rect, Vec2};
use peniko::{Compose, Fill, Mix};

/// pyro: a corner radius of a shadow shape grown by `spread` (shrunk when negative), as CSS
/// Backgrounds 3 §7.1.1 has it (and WPT's `box-shadow-radius-generated-ref.html` computes it): a
/// radius larger than the spread, or of a corner that covers more than its box (`coverage`, the
/// smaller of its two radii's share of half the box), grows by the spread; a smaller one by the
/// spread times `1 - (1 - r / spread)³ (1 - coverage³)`, so that a square corner stays square and a
/// small radius grows smoothly into a large one, while a corner that is all of the box's side (a
/// circle's) stays round. A shrinking radius stops at zero.
fn spread_radius(radius: f64, spread: f64, coverage: f64) -> f64 {
    if radius <= 0.0 {
        return 0.0;
    }
    if spread <= 0.0 {
        return (radius + spread).max(0.0);
    }
    if radius > spread || coverage > 1.0 {
        return radius + spread;
    }
    radius + spread * (1.0 - (1.0 - radius / spread).powi(3) * (1.0 - coverage.powi(3)))
}

/// pyro: the radii of a shadow shape whose box (`size`, before the spread) was grown by `spread`
/// on every side. A corner that is square in either direction stays square.
fn spread_radii(
    radii: &NonUniformRoundedRectRadii,
    size: kurbo::Size,
    spread: f64,
) -> NonUniformRoundedRectRadii {
    let corner = |r: Vec2| {
        if r.x <= 0.0 || r.y <= 0.0 {
            return Vec2::ZERO;
        }
        let coverage = (2.0 * r.x / size.width).min(2.0 * r.y / size.height);
        let coverage = if coverage.is_finite() { coverage } else { 0.0 };
        Vec2::new(
            spread_radius(r.x, spread, coverage),
            spread_radius(r.y, spread, coverage),
        )
    };
    NonUniformRoundedRectRadii {
        top_left: corner(radii.top_left),
        top_right: corner(radii.top_right),
        bottom_right: corner(radii.bottom_right),
        bottom_left: corner(radii.bottom_left),
    }
}

/// pyro: what a box that clips its overflow (`style`'s `overflow` is not `visible` on some
/// axis; `frame` is its box) clips its content to, as Chromium does:
/// - a scroll container (`hidden`, `scroll`, `auto`): its padding box, along the padding edge's
///   curve;
/// - `overflow: clip` on both axes (or `contain: paint`): the box `overflow-clip-margin` names (the padding box
///   unless it says otherwise) along its edge's curve, grown by the margin's length as a
///   shadow shape is by its spread;
/// - `clip` on one axis and `visible` on the other: that box's extent (and margin) on the
///   clipped axis only, with square corners (CSS Overflow 3 §3.1: no rounding when one axis
///   does not clip).
pub(super) fn overflow_clip_path(frame: &CssBox, style: &style::properties::ComputedValues, scale: f64) -> kurbo::BezPath {
    use kurbo::Shape as _;
    use style::values::computed::Overflow;
    use style::values::generics::box_::OverflowClipMarginBox;
    let box_style = style.get_box();
    let (visible_x, visible_y) = (box_style.overflow_x == Overflow::Visible, box_style.overflow_y == Overflow::Visible);
    // Paint containment clips as `overflow: clip` does, on both axes.
    let contains = box_style.contain.contains(style::values::computed::Contain::PAINT);
    let (clip_x, clip_y) = (
        box_style.overflow_x == Overflow::Clip || contains && visible_x,
        box_style.overflow_y == Overflow::Clip || contains && visible_y,
    );
    if !(clip_x || clip_y) || !(clip_x || visible_x) || !(clip_y || visible_y) {
        return frame.padding_box_path();
    }
    let clip_margin = &style.get_margin().overflow_clip_margin;
    // (A negative margin moves the edge inwards, to the box's middle at most.)
    let margin = f64::from(clip_margin.offset.px()) * scale;
    let (rect, insets) = match clip_margin.visual_box {
        OverflowClipMarginBox::BorderBox => (frame.border_box, Insets::ZERO),
        OverflowClipMarginBox::PaddingBox => (frame.padding_box, frame.border_width),
        OverflowClipMarginBox::ContentBox => {
            let (b, p) = (frame.border_width, frame.padding_width);
            (frame.content_box, Insets::new(b.x0 + p.x0, b.y0 + p.y0, b.x1 + p.x1, b.y1 + p.y1))
        }
    };
    let margin = margin.max(-rect.width() / 2.0).max(-rect.height() / 2.0);
    if !(clip_x && clip_y) {
        const FAR: f64 = 1e7;
        let (x0, x1) = if clip_x { (rect.x0 - margin, rect.x1 + margin) } else { (-FAR, FAR) };
        let (y0, y1) = if clip_y { (rect.y0 - margin, rect.y1 + margin) } else { (-FAR, FAR) };
        return Rect::new(x0, y0, x1, y1).to_path(0.1);
    }
    if margin == 0.0 {
        return match clip_margin.visual_box {
            OverflowClipMarginBox::BorderBox => frame.border_box_path(),
            OverflowClipMarginBox::PaddingBox => frame.padding_box_path(),
            OverflowClipMarginBox::ContentBox => frame.content_box_path(),
        };
    }
    // The box's own radii: the border radii less the widths between the border edge and it.
    let inner = |r: Vec2, dx: f64, dy: f64| {
        let r = Vec2::new(r.x - dx, r.y - dy);
        if r.x <= 0.0 || r.y <= 0.0 { Vec2::ZERO } else { r }
    };
    let radii = NonUniformRoundedRectRadii {
        top_left: inner(frame.border_radii.top_left, insets.x0, insets.y0),
        top_right: inner(frame.border_radii.top_right, insets.x1, insets.y0),
        bottom_right: inner(frame.border_radii.bottom_right, insets.x1, insets.y1),
        bottom_left: inner(frame.border_radii.bottom_left, insets.x0, insets.y1),
    };
    let radii = spread_radii(&radii, rect.size(), margin);
    CssBox::new(rect.inflate(margin, margin), Insets::ZERO, Insets::ZERO, 0.0, radii).border_box_path()
}

/// pyro: the single radius of a shadow shape whose corners are all circular arcs of one radius
/// (a rectangle, or a rounded rectangle), which vello's blurred rounded rectangle can draw.
fn uniform_radius(radii: &NonUniformRoundedRectRadii) -> Option<f64> {
    let r = radii.top_left.x;
    [
        radii.top_left,
        radii.top_right,
        radii.bottom_right,
        radii.bottom_left,
    ]
    .iter()
    .all(|c| (c.x - r).abs() < 1e-6 && (c.y - r).abs() < 1e-6)
    .then_some(r)
}

/// pyro: the inner radii of a box's padding edge: each border radius less the border widths
/// beside it (CSS Backgrounds 3 §5.2).
fn padding_radii(frame: &CssBox) -> NonUniformRoundedRectRadii {
    let r = &frame.border_radii;
    let w = frame.border_width;
    let inner = |r: Vec2, x: f64, y: f64| {
        let r = Vec2::new((r.x - x).max(0.0), (r.y - y).max(0.0));
        if r.x == 0.0 || r.y == 0.0 {
            Vec2::ZERO
        } else {
            r
        }
    };
    NonUniformRoundedRectRadii {
        top_left: inner(r.top_left, w.x0, w.y0),
        top_right: inner(r.top_right, w.x1, w.y0),
        bottom_right: inner(r.bottom_right, w.x1, w.y1),
        bottom_left: inner(r.bottom_left, w.x0, w.y1),
    }
}

impl ElementCx<'_, '_> {
    pub(super) fn draw_outset_box_shadow(&self, scene: &mut impl PaintScene) {
        let box_shadow = &self.style.get_effects().box_shadow.0;
        let has_outset_shadow = box_shadow.iter().any(|s| !s.inset);
        if !has_outset_shadow {
            return;
        }

        let current_color = self.style.clone_color();
        let opacity = self.style.get_effects().opacity;
        let bg_color = self
            .style
            .get_background()
            .background_color
            .resolve_to_absolute(&current_color)
            .as_srgb_color();
        // pyro: a root or body whose background is the canvas's paints none of its own over
        // the shadow, which must then be cut out of its box.
        let canvas_background = self.context.root_element_id == Some(self.node.id)
            || self.context.canvas_background == Some(self.node.id);
        let bg_is_opaque = bg_color.components[3] >= 1.0 && !canvas_background;
        let needs_clip = opacity < 1.0 || !bg_is_opaque;

        let max_shadow_rect = box_shadow.iter().fold(Rect::ZERO, |prev, shadow| {
            let x = shadow.base.horizontal.px() as f64 * self.scale;
            let y = shadow.base.vertical.px() as f64 * self.scale;
            let blur = shadow.base.blur.px() as f64 * self.scale;
            let spread = shadow.spread.px() as f64 * self.scale;
            let offset = spread + blur * 2.5;

            let rect = self.frame.border_box.inflate(offset, offset) + Vec2::new(x, y);

            prev.union(rect)
        });

        self.context.layer_manager.maybe_with_layer(
            scene,
            needs_clip,
            Mix::Normal.into(),
            1.0,
            self.transform,
            &self.frame.shadow_clip(max_shadow_rect),
            None,
            None,
            |scene| {
                for shadow in box_shadow.iter().filter(|s| !s.inset).rev() {
                    let shadow_color = shadow
                        .base
                        .color
                        .resolve_to_absolute(&current_color)
                        .as_srgb_color();

                    let alpha = shadow_color.components[3];
                    if alpha != 0.0 {
                        let transform = self.transform.then_translate(Vec2 {
                            x: shadow.base.horizontal.px() as f64 * self.scale,
                            y: shadow.base.vertical.px() as f64 * self.scale,
                        });

                        let spread = shadow.spread.px() as f64 * self.scale;
                        let blur = shadow.base.blur.px() as f64;

                        // Without blur or spread the shadow is exactly the element's shape,
                        // so fill the border box path directly (preserving individual radii)
                        if blur == 0.0 && spread == 0.0 {
                            scene.fill(
                                Fill::NonZero,
                                transform,
                                shadow_color,
                                None,
                                &self.frame.border_box_path(),
                            );
                            continue;
                        }

                        // pyro: the shadow shape is the border box grown by the spread, each
                        // corner's radii grown by the spread formula (CSS Backgrounds 3 §7.1.1).
                        let rect = self.frame.border_box.inflate(spread, spread);
                        if rect.width() <= 0.0 || rect.height() <= 0.0 {
                            continue;
                        }
                        let radii = spread_radii(
                            &self.frame.border_radii,
                            self.frame.border_box.size(),
                            spread,
                        );
                        let shape = CssBox::new(rect, Insets::ZERO, Insets::ZERO, 0.0, radii);

                        // Without blur the shadow is a solid rounded rectangle: fill it as a
                        // path (antialiased edges) rather than with vello's blurred-rectangle
                        // function, which evaluates powf for every pixel it covers. A spread
                        // ring (`box-shadow: 0 0 0 6000px`) covered a whole 3840x2160 frame:
                        // 60 of the frame's 88 ms of CPU time.
                        if blur == 0.0 {
                            scene.fill(
                                Fill::NonZero,
                                transform,
                                shadow_color,
                                None,
                                &shape.border_box_path(),
                            );
                            continue;
                        }

                        // pyro: Chromium blurs with a standard deviation of half the blur
                        // radius (`BlurRadiusToStdDev`), in device px. Corners of differing radii
                        // are drawn with their average (vello's blurred rectangle has one).
                        let radius = uniform_radius(&shape.border_radii)
                            .unwrap_or_else(|| shape.border_radii.average());
                        let std_dev = blur * self.scale / 2.0;
                        scene.draw_box_shadow(transform, rect, shadow_color, radius, std_dev);
                    }
                }
            },
        )
    }

    pub(super) fn draw_inset_box_shadow(&self, scene: &mut impl PaintScene) {
        let current_color = self.style.clone_color();
        let box_shadow = &self.style.get_effects().box_shadow.0;
        let has_inset_shadow = box_shadow.iter().any(|s| s.inset);
        if !has_inset_shadow {
            return;
        }

        let padding_box = self.frame.padding_box_path();
        let inner_radii = padding_radii(&self.frame);

        for shadow in box_shadow.iter().filter(|s| s.inset) {
            let shadow_color = shadow
                .base
                .color
                .resolve_to_absolute(&current_color)
                .as_srgb_color();
            if shadow_color.components[3] <= 0.0 || shadow_color == Color::TRANSPARENT {
                continue;
            }

            let offset = Vec2 {
                x: shadow.base.horizontal.px() as f64 * self.scale,
                y: shadow.base.vertical.px() as f64 * self.scale,
            };
            let transform = self.transform.then_translate(offset);
            // The unshadowed area is the padding box shrunk by the spread radius, its radii the
            // padding edge's shrunk (or, for a negative spread, grown) by the spread formula.
            let spread = shadow.spread.px() as f64 * self.scale;
            let blur = shadow.base.blur.px() as f64 * self.scale;
            let hole = self.frame.padding_box.inflate(-spread, -spread);
            let hole_shape = (hole.width() > 0.0 && hole.height() > 0.0).then(|| {
                let radii = spread_radii(&inner_radii, self.frame.padding_box.size(), -spread);
                CssBox::new(hole, Insets::ZERO, Insets::ZERO, 0.0, radii)
            });

            // pyro: without blur the shadow is the padding box less the moved hole, one fill in
            // a clip to the padding box (no layers: the colour blends as a box's does).
            if blur == 0.0 {
                let mut path = padding_box.clone();
                if let Some(shape) = &hole_shape {
                    let mut hole_path = shape.border_box_path();
                    hole_path.apply_affine(kurbo::Affine::translate(offset));
                    path.extend(&hole_path);
                }
                scene.push_clip_layer(Fill::NonZero, self.transform, &padding_box);
                scene.fill(Fill::EvenOdd, self.transform, shadow_color, None, &path);
                scene.pop_layer();
                continue;
            }

            scene.push_layer(
                Fill::NonZero,
                Mix::Normal,
                1.0,
                self.transform,
                &padding_box,
                None,
                None,
            );
            scene.fill(
                Fill::NonZero,
                self.transform,
                shadow_color,
                None,
                &padding_box,
            );

            scene.push_layer(
                Fill::NonZero,
                Compose::DestOut,
                1.0,
                self.transform,
                &padding_box,
                None,
                None,
            );
            if let Some(shape) = &hole_shape {
                let radius = uniform_radius(&shape.border_radii)
                    .unwrap_or_else(|| shape.border_radii.average());
                scene.draw_box_shadow(transform, hole, Color::WHITE, radius, blur / 2.0);
            }

            scene.pop_layer();
            scene.pop_layer();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The spread formula of CSS Backgrounds 3 §7.1.1, against WPT's
    /// `box-shadow-radius-000.html` (a 5px radius spread by 20px is 16.56px) and
    /// `box-shadow-border-radius-001.html` (a 2px circle spread by 149px is a 300px circle).
    #[test]
    fn spread_radius_follows_the_spec_formula() {
        assert_eq!(spread_radius(10.0, 10.0, 0.5), 20.0);
        assert_eq!(spread_radius(10.0, 5.0, 0.5), 15.0);
        assert!((spread_radius(8.0, 16.0, 0.0) - 22.0).abs() < 1e-9);
        assert!((spread_radius(5.0, 20.0, 0.0) - 16.5625).abs() < 1e-9);
        assert_eq!(spread_radius(1.0, 149.0, 1.0), 150.0);
        assert_eq!(spread_radius(0.0, 20.0, 0.0), 0.0);
        // A negative spread (inset shadows) shrinks, and stops at zero.
        assert_eq!(spread_radius(10.0, -4.0, 0.0), 6.0);
        assert_eq!(spread_radius(3.0, -4.0, 0.0), 0.0);
    }
}
