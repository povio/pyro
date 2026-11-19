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
    margin_clip_path(frame, style, scale, clip_x, clip_y)
}

/// pyro: the clip of a box that clips on `clip_x` and/or `clip_y` as `overflow: clip` does, at
/// its `overflow-clip-margin` (see [`overflow_clip_path`]). Also the viewport clip of an inline
/// `<svg>` (a replaced element, which `overflow: hidden` clips as `clip` does: Chromium's
/// `LayoutReplaced` applies the margin to it; WPT css-overflow `overflow-clip-margin-svg`).
pub(super) fn margin_clip_path(
    frame: &CssBox,
    style: &style::properties::ComputedValues,
    scale: f64,
    clip_x: bool,
    clip_y: bool,
) -> kurbo::BezPath {
    use kurbo::Shape as _;
    use style::values::generics::box_::OverflowClipMarginBox;
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
    // pyro: as Chromium: the padding box's shape (the plain overflow clip) moved out to the edge
    // by the signed distance (the border widths for `border-box`, minus the padding for
    // `content-box`, plus the margin), its radii adjusted as Blink's
    // `FloatRoundedRect::OutsetWithCornerCorrection` (CSS Backgrounds 3 corner shaping: a radius
    // shrinks by an inset; past the outset it grows by it, a smaller one by the cubic easing
    // with its coverage). WPT css-overflow `overflow-clip-margin-border-radius-002` (a content
    // box without radius rounded by its margin), `overflow-clip-margin-visual-box-and-value-
    // with-border-radius` (15px corner, `border-box 5px`: 15.56px).
    let inner = |r: Vec2, dx: f64, dy: f64| {
        let r = Vec2::new(r.x - dx, r.y - dy);
        if r.x <= 0.0 || r.y <= 0.0 { Vec2::ZERO } else { r }
    };
    let b = frame.border_width;
    let padding = frame.padding_box;
    let outsets = Insets::new(
        b.x0 - insets.x0 + margin,
        b.y0 - insets.y0 + margin,
        b.x1 - insets.x1 + margin,
        b.y1 - insets.y1 + margin,
    );
    let size = padding.size();
    let adjusted = |r: Vec2, ox: f64, oy: f64| {
        if (r.x == 0.0 && r.y == 0.0) || (ox == 0.0 && oy == 0.0) || size.width <= 0.0 || size.height <= 0.0 {
            return r;
        }
        let coverage = 2.0 * (r.x / size.width).min(r.y / size.height);
        let dimension = |radius: f64, outset: f64| {
            if radius > outset || coverage > 1.0 {
                radius + outset
            } else {
                let ratio = radius / outset;
                radius + outset * (1.0 - (1.0 - ratio).powi(3) * (1.0 - coverage.powi(3)))
            }
        };
        let r = Vec2::new(dimension(r.x, ox), dimension(r.y, oy));
        if r.x <= 0.0 || r.y <= 0.0 { Vec2::ZERO } else { r }
    };
    let radii = NonUniformRoundedRectRadii {
        top_left: adjusted(inner(frame.border_radii.top_left, b.x0, b.y0), outsets.x0, outsets.y0),
        top_right: adjusted(inner(frame.border_radii.top_right, b.x1, b.y0), outsets.x1, outsets.y0),
        bottom_right: adjusted(inner(frame.border_radii.bottom_right, b.x1, b.y1), outsets.x1, outsets.y1),
        bottom_left: adjusted(inner(frame.border_radii.bottom_left, b.x0, b.y1), outsets.x0, outsets.y1),
    };
    CssBox::new(rect.inflate(margin, margin), Insets::ZERO, Insets::ZERO, 0.0, radii)
        .contoured_like(frame)
        .border_box_path()
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
    /// pyro: what this element clips its overflow to ([`overflow_clip_path`]): with a
    /// `border-shape`, the inner edge of its border.
    pub(super) fn overflow_clip_path(&self, style: &style::properties::ComputedValues) -> kurbo::BezPath {
        let mut path = self.overflow_clip_path_of_box(style);
        // pyro: a fieldset's `overflow` clips its anonymous content box, not its rendered legend
        // (HTML § 15.3.13; WPT the-fieldset-and-legend-elements `legend-position-relative-2`).
        if let Some(legend) = self.context.fieldset_legend_rect(self.node.id) {
            use kurbo::Shape as _;
            path.extend(legend.scale_from_origin(self.scale).path_elements(0.1));
        }
        path
    }

    fn overflow_clip_path_of_box(&self, style: &style::properties::ComputedValues) -> kurbo::BezPath {
        if let Some(shape) = self.border_shape() {
            return shape.inner_path();
        }
        // pyro: a table with captions clips its wrapper box, captions too (its frame is its grid
        // alone; Chromium's table fragment holds its captions; WPT css-contain
        // `contain-paint-table-002`).
        // (Only with captions: a grid as tall as the wrapper keeps its own frame, the collapsed
        // borders it was laid out with. And only for paint containment: `overflow` on a table
        // applies to the table box, the grid, not the wrapper (CSS 2 § 11.1.1; WPT
        // css21-errata `s-11-1-1b-001`).)
        let visible = {
            use style::values::computed::Overflow;
            let b = style.get_box();
            b.overflow_x == Overflow::Visible && b.overflow_y == Overflow::Visible
        };
        let contains_paint = style.get_box().contain.contains(style::values::computed::Contain::PAINT);
        if let blitz_dom::node::SpecialElementData::TableRoot(table) = &self.element.special_data
            && visible
            && contains_paint
            && table.grid_box.borrow().is_some_and(|(top, height)| {
                top != 0.0 || (height - self.node.final_layout().size.height).abs() > 0.01
            })
        {
            let wrapper = super::create_css_rect(style, self.node.final_layout(), self.scale);
            return overflow_clip_path(&wrapper, style, self.scale);
        }
        // pyro: classic scrollbars: a scroll container's content is cut to its scrollport, the
        // padding box less the scrollbar gutters (Chrome: WPT css-overflow
        // `overflow-scroll-resize-visibility-hidden`). A rounded padding box keeps its shape.
        if let Some(bars) = self.node.classic_scrollbars() {
            use kurbo::Shape as _;
            use style::values::computed::Overflow;
            let b = style.get_box();
            let scrolls = |o: Overflow| matches!(o, Overflow::Hidden | Overflow::Scroll | Overflow::Auto);
            let r = self.frame.border_radii;
            let square = [r.top_left, r.top_right, r.bottom_right, r.bottom_left].iter().all(|v| v.x <= 0.0 && v.y <= 0.0);
            if square && scrolls(b.overflow_x) && scrolls(b.overflow_y) {
                let (g, s) = (bars.gutters, self.scale);
                let p = self.frame.padding_box;
                let x0 = p.x0 + g.left as f64 * s;
                let y0 = p.y0 + g.top as f64 * s;
                return kurbo::Rect::new(x0, y0, (p.x1 - g.right as f64 * s).max(x0), (p.y1 - g.bottom as f64 * s).max(y0))
                    .to_path(0.1);
            }
        }
        overflow_clip_path(&self.frame, style, self.scale)
    }

    /// pyro: a shadow of `path` (this element's space, device px) filled in `color`, moved by
    /// `offset` and blurred by `blur` (the CSS blur radius, device px: a standard deviation of
    /// half of it, as Chromium blurs), drawn through a filter layer when blurred.
    fn fill_shadow_path(
        &self,
        scene: &mut impl PaintScene,
        path: &kurbo::BezPath,
        color: Color,
        offset: Vec2,
        blur: f64,
    ) {
        let transform = self.transform.then_translate(offset);
        if blur <= 0.0 {
            scene.fill(Fill::NonZero, transform, color, None, path);
            return;
        }
        use kurbo::Shape as _;
        let filter = std::sync::Arc::new(anyrender::Filter::single(anyrender::filters::FilterEffect::blur(
            (blur / 2.0) as f32,
        )));
        let bounds = path.bounding_box().inflate(blur * 2.0, blur * 2.0);
        let mut group = anyrender::Scene::new();
        group.push_layer(Fill::NonZero, Mix::Normal, 1.0, transform, &bounds, Some(filter), None);
        group.fill(Fill::NonZero, transform, color, None, path);
        group.pop_layer();
        scene.append_scene(group, kurbo::Affine::IDENTITY);
    }

    /// pyro: the shadows of an element with a `border-shape` (CSS Borders 4 §11.5.2): an outer
    /// shadow is its outer edge grown by the spread, blurred and cut out of the outer edge
    /// (`PaintNormalBoxShadow`); an inner one fills around the inner edge shrunk by the spread,
    /// inside the inner edge (`PaintInsetBoxShadowForBorderShape`).
    fn draw_border_shape_shadows(
        &self,
        scene: &mut impl PaintScene,
        shape: &super::border_shape::BorderShape,
        inset: bool,
    ) {
        use crate::kurbo_css::contoured::{boolean, flatten, rect_polygon};
        use kurbo::Shape as _;
        let current_color = self.style.clone_color();
        let box_shadow = &self.style.get_effects().box_shadow.0;
        let clip = if inset { shape.inner_path() } else { shape.outer_path() };
        // pyro: an outer shadow under a shape its opaque background fills (no inner shape: the
        // background is clipped to the shape) needs no cut, as a box's (`draw_outset_box_shadow`):
        // the shadow shows under the shape's anti-aliased edge as it does a `border-radius` box's
        // (WPT css-borders `border-shape-shadow-blur`).
        let covered = !inset && shape.inner.is_none() && {
            let background = self.style.get_background().background_color.resolve_to_absolute(&current_color).as_srgb_color();
            let canvas_background = self.context.root_element_id == Some(self.node.id)
                || self.context.canvas_background == Some(self.node.id);
            background.components[3] >= 1.0 && !canvas_background && self.style.get_effects().opacity >= 1.0
        };
        let mut clipped = covered;
        for shadow in box_shadow.iter().filter(|s| s.inset == inset).rev() {
            let color = shadow.base.color.resolve_to_absolute(&current_color).as_srgb_color();
            if color.components[3] <= 0.0 {
                continue;
            }
            let offset = Vec2::new(
                shadow.base.horizontal.px() as f64 * self.scale,
                shadow.base.vertical.px() as f64 * self.scale,
            );
            let spread = shadow.spread.px() as f64 * self.scale;
            let blur = shadow.base.blur.px() as f64 * self.scale;
            // pyro: a circle's blurred inset shadow clips itself to the circle (below): a second
            // clip would take its anti-aliased edge's coverage twice.
            let inset_circle = inset && blur > 0.0 && shape.inner_circle().is_some();
            if !clipped && !inset_circle {
                if inset {
                    scene.push_clip_layer(Fill::NonZero, self.transform, &clip);
                } else {
                    // Everything but the outer edge's area.
                    let far = clip.bounding_box().inflate(1e5, 1e5);
                    if shape.oval.is_some() {
                        // pyro: an oval's edge as its curve (one contour, even-odd), as a
                        // `border-radius` box's shadow is cut: a flattened edge took its
                        // anti-aliased pixels apart (WPT css-borders `border-shape-shadow-circle`).
                        let mut outside = far.to_path(0.1);
                        outside.extend(clip.iter());
                        scene.push_clip_layer(Fill::EvenOdd, self.transform, &outside);
                    } else {
                        let outside = boolean(
                            &[vec![rect_polygon(far)], flatten(&clip)],
                            |inside| inside[0] && !inside[1],
                        );
                        scene.push_clip_layer(Fill::NonZero, self.transform, &outside);
                    }
                }
                clipped = true;
            }
            if let Some(circle) = shape.inner_circle().filter(|_| inset_circle) {
                // pyro: a circle's blurred inset shadow as a `border-radius: 50%` box's: the
                // colour less the blurred hole (WPT css-borders `border-shape-shadow-inset-blur`).
                let clip_path = super::border_shape::oval_path(circle).unwrap_or_default();
                scene.push_layer(Fill::NonZero, Mix::Normal, 1.0, self.transform, &clip_path, None, None);
                scene.fill(Fill::NonZero, self.transform, color, None, &clip_path);
                scene.push_layer(Fill::NonZero, Compose::DestOut, 1.0, self.transform, &clip_path, None, None);
                let hole = circle.inflate(-spread, -spread);
                if hole.width() > 0.0 && hole.height() > 0.0 {
                    let transform = self.transform.then_translate(offset);
                    scene.draw_box_shadow(transform, hole, Color::WHITE, hole.width() / 2.0, blur / 2.0);
                }
                scene.pop_layer();
                scene.pop_layer();
            } else if inset {
                // The hole is the inner edge shrunk by the spread; the shadow is all around it.
                let hole = if spread == 0.0 {
                    clip.clone()
                } else {
                    shape.offset_path(&clip, -spread)
                };
                let far = clip.bounding_box().inflate(blur * 3.0 + spread.abs() + offset.hypot() + 10.0, 0.0);
                let far = far.inflate(0.0, blur * 3.0 + spread.abs() + offset.hypot() + 10.0);
                let around = boolean(
                    &[vec![rect_polygon(far)], flatten(&hole)],
                    |inside| inside[0] && !inside[1],
                );
                self.fill_shadow_path(scene, &around, color, offset, blur);
            } else if let Some(circle) = shape.circle_with_offset(spread).filter(|_| blur > 0.0) {
                // A circle's blurred shadow as a `border-radius: 50%` box's is drawn.
                let transform = self.transform.then_translate(offset);
                scene.draw_box_shadow(transform, circle, color, circle.width() / 2.0, blur / 2.0);
            } else {
                let path = shape.outer_path_with_offset(spread);
                self.fill_shadow_path(scene, &path, color, offset, blur);
            }
        }
        if clipped && !covered {
            scene.pop_layer();
        }
    }

    pub(super) fn draw_outset_box_shadow(&self, scene: &mut impl PaintScene) {
        let box_shadow = &self.style.get_effects().box_shadow.0;
        let has_outset_shadow = box_shadow.iter().any(|s| !s.inset);
        if !has_outset_shadow {
            return;
        }
        if let Some(shape) = self.border_shape() {
            self.draw_border_shape_shadows(scene, &shape, false);
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
                        let shape = CssBox::new(rect, Insets::ZERO, Insets::ZERO, 0.0, radii)
                            .contoured_like(&self.frame);

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
                        // radius (`BlurRadiusToStdDev`), in device px. vello's blurred rectangle
                        // has one circular radius: a shape with others (elliptical or differing
                        // corners, shaped corners) is its path, blurred.
                        let std_dev = blur * self.scale / 2.0;
                        match uniform_radius(&shape.border_radii).filter(|_| shape.corner_curvature.is_none()) {
                            Some(radius) => scene.draw_box_shadow(transform, rect, shadow_color, radius, std_dev),
                            None => {
                                let offset = Vec2::new(
                                    shadow.base.horizontal.px() as f64 * self.scale,
                                    shadow.base.vertical.px() as f64 * self.scale,
                                );
                                self.fill_shadow_path(scene, &shape.border_box_path(), shadow_color, offset, blur * self.scale);
                            }
                        }
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
        if let Some(shape) = self.border_shape() {
            self.draw_border_shape_shadows(scene, &shape, true);
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
                CssBox::new(hole, Insets::ZERO, Insets::ZERO, 0.0, radii).contoured_like(&self.frame)
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
