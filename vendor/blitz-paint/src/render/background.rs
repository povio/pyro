use super::{ALMOST_OPAQUE, ElementCx, to_image_quality, to_peniko_image};
use crate::color::{Color, ToColorColor};
use crate::gradient::to_peniko_gradient;
use crate::kurbo_css::{CssBox, NonUniformRoundedRectRadii};
use anyrender::PaintScene;
use blitz_dom::CopyStyleGetters;
use blitz_dom::NodeId;
use blitz_dom::node::{ImageData, ImageResourceData, SpecialElementData};
use kurbo::{self, Affine, BezPath, Point, Rect, Shape, Size, Vec2};
use peniko::{self, BlendMode, Compose, Fill, Mix};
use style::{
    computed_values::border_collapse::T as BorderCollapse,
    properties::{
        ComputedValues,
        generated::longhands::{
            background_attachment::single_value::computed_value::T as StyloBackgroundAttachment,
            background_clip::single_value::computed_value::T as StyloBackgroundClip,
            background_origin::single_value::computed_value::T as StyloBackgroundOrigin,
        },
        style_structs::{Background, SVG},
    },
    values::{
        computed::{
            BackgroundRepeat, Gradient as StyloGradient, Image as ComputedImage, LengthPercentage,
            Overflow, background::BackgroundSize,
        },
        generics::image::GenericImage,
        specified::background::BackgroundRepeatKeyword,
    },
};

#[cfg(feature = "tracing")]
use tracing::warn;

/// A box from the CSS box model. Abstracts over the (structurally identical)
/// computed value types of the `background-clip`/`background-origin` and
/// `mask-clip`/`mask-origin` properties.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)] // The variants are named after the CSS keywords
pub(super) enum BoxModelBox {
    BorderBox,
    PaddingBox,
    ContentBox,
    /// pyro: `mask-clip: no-clip`: everything visible.
    NoClip,
}

// Also covers MaskClip as the type is the same
impl From<StyloBackgroundClip> for BoxModelBox {
    fn from(value: StyloBackgroundClip) -> Self {
        match value {
            StyloBackgroundClip::BorderBox => Self::BorderBox,
            StyloBackgroundClip::PaddingBox => Self::PaddingBox,
            StyloBackgroundClip::ContentBox => Self::ContentBox,

            // TODO: support BorderArea
            StyloBackgroundClip::BorderArea => Self::BorderBox,
            StyloBackgroundClip::Text => Self::BorderBox,
            // pyro: the union of the text and the border area, cut by the text mask.
            StyloBackgroundClip::BorderAreaText => Self::BorderBox,

            // pyro: `mask-clip`'s SVG boxes, for a box: fill-box is the content box, stroke-box
            // and view-box the border box.
            StyloBackgroundClip::FillBox => Self::ContentBox,
            StyloBackgroundClip::StrokeBox | StyloBackgroundClip::ViewBox => Self::BorderBox,
            StyloBackgroundClip::NoClip => Self::NoClip,
        }
    }
}

// Also covers MaskOrigin as the type is the same
impl From<StyloBackgroundOrigin> for BoxModelBox {
    fn from(value: StyloBackgroundOrigin) -> Self {
        match value {
            StyloBackgroundOrigin::BorderBox => Self::BorderBox,
            StyloBackgroundOrigin::PaddingBox => Self::PaddingBox,
            StyloBackgroundOrigin::ContentBox => Self::ContentBox,
            // pyro: `mask-origin`'s SVG boxes, for a box (as `mask-clip`'s above).
            StyloBackgroundOrigin::FillBox => Self::ContentBox,
            StyloBackgroundOrigin::StrokeBox | StyloBackgroundOrigin::ViewBox => Self::BorderBox,
        }
    }
}

/// The styles and image data for a single layer of a CSS image layer list
/// (`background-image` or `mask-image`). The `background-*` and `mask-*`
/// properties share computed value types, which allows the layer painting code
/// to be shared.
pub(super) struct ImageLayerStyles<'a> {
    /// The computed value of the `background-image`/`mask-image` layer
    pub stylo_image: &'a ComputedImage,
    /// The loaded image resource if `stylo_image` is a `url()` image
    pub image_data: Option<&'a ImageResourceData>,
    pub position_x: &'a LengthPercentage,
    pub position_y: &'a LengthPercentage,
    pub repeat: &'a BackgroundRepeat,
    pub size: &'a BackgroundSize,
    pub clip: BoxModelBox,
    pub text_clip: bool,
    /// pyro: `background-clip: border-area text`: the text mask takes in the border area too.
    pub text_border_area: bool,
    /// pyro: `background-clip: border-area`: the area the border paints.
    pub border_area: bool,
    pub origin: BoxModelBox,
    pub attachment: StyloBackgroundAttachment,
    /// pyro: image pixels per CSS px (the `<resolution>` of an `image-set()` option; 1
    /// otherwise): a raster image's natural size is its pixel size divided by it.
    pub resolution: f32,
}

impl<'a> ImageLayerStyles<'a> {
    pub(super) fn from_background(
        bg_styles: &'a Background,
        image_data: &'a [Option<ImageResourceData>],
        idx: usize,
    ) -> Self {
        Self {
            stylo_image: &bg_styles.background_image.0[idx],
            image_data: image_data.get(idx).and_then(Option::as_ref),
            resolution: 1.0,
            position_x: get_cyclic(&bg_styles.background_position_x.0, idx),
            position_y: get_cyclic(&bg_styles.background_position_y.0, idx),
            repeat: get_cyclic(&bg_styles.background_repeat.0, idx),
            size: get_cyclic(&bg_styles.background_size.0, idx),
            clip: (*get_cyclic(&bg_styles.background_clip.0, idx)).into(),
            text_clip: matches!(
                *get_cyclic(&bg_styles.background_clip.0, idx),
                StyloBackgroundClip::Text | StyloBackgroundClip::BorderAreaText
            ),
            text_border_area: *get_cyclic(&bg_styles.background_clip.0, idx)
                == StyloBackgroundClip::BorderAreaText,
            border_area: *get_cyclic(&bg_styles.background_clip.0, idx)
                == StyloBackgroundClip::BorderArea,
            origin: (*get_cyclic(&bg_styles.background_origin.0, idx)).into(),
            attachment: *get_cyclic(&bg_styles.background_attachment.0, idx),
        }
    }

    pub(super) fn from_svg(
        svg_styles: &'a SVG,
        image_data: &'a [Option<ImageResourceData>],
        idx: usize,
    ) -> Self {
        Self {
            stylo_image: &svg_styles.mask_image.0[idx],
            image_data: image_data.get(idx).and_then(Option::as_ref),
            resolution: 1.0,
            position_x: get_cyclic(&svg_styles.mask_position_x.0, idx),
            position_y: get_cyclic(&svg_styles.mask_position_y.0, idx),
            repeat: get_cyclic(&svg_styles.mask_repeat.0, idx),
            size: get_cyclic(&svg_styles.mask_size.0, idx),
            clip: (*get_cyclic(&svg_styles.mask_clip.0, idx)).into(),
            text_clip: false,
            text_border_area: false,
            border_area: false,
            origin: (*get_cyclic(&svg_styles.mask_origin.0, idx)).into(),
            // There is no `mask-attachment` property
            attachment: StyloBackgroundAttachment::Scroll,
        }
    }
}

impl ElementCx<'_, '_> {
    pub(super) fn background_inside_overflow(&self, clip: &BezPath) -> bool {
        use style::dom::TElement as _;
        if !self.frame.has_border_radius() || self.frame.border_width != kurbo::Insets::ZERO
            || self.devtools.show_layout || self.node.local_name() == "input"
            || !matches!(self.style.clone_display().inside(), style::values::specified::box_::DisplayInside::Flow | style::values::specified::box_::DisplayInside::FlowRoot)
            || self.style.clone_overflow_x() != Overflow::Clip || self.style.clone_overflow_y() != Overflow::Clip
            || !matches!(self.element.special_data, SpecialElementData::None)
            || !self.style.get_effects().box_shadow.0.is_empty() || self.border_shape().is_some()
            || !matches!(self.style.get_border().border_image_source, GenericImage::None)
            || *clip != self.frame.border_box_path()
        { return false; }
        true
    }

    pub(super) fn draw_background_inside_overflow(&self, scene: &mut impl PaintScene, clip: &BezPath) {
        self.draw_background_layers(scene, &self.style, &self.element.background_images, None, None, Some(clip));
    }

    pub(super) fn draw_background(&self, scene: &mut impl PaintScene) {
        // pyro: a checkbox, radio button or range input in its native appearance paints no author
        // background (Chromium draws the widget alone; `appearance: none` draws the box).
        use blitz_dom::local_name;
        use style::dom::TElement as _;
        if self.node.local_name() == "input"
            && self.node.attr(local_name!("type")).is_some_and(|kind| {
                ["checkbox", "radio", "range"].iter().any(|native| kind.trim().eq_ignore_ascii_case(native))
            })
            && !self.native_appearance_none()
        {
            return;
        }
        let style = self.style.clone();
        self.draw_background_layers(scene, &style, &self.element.background_images, None, None, None);
    }

    /// pyro: the canvas background (CSS Backgrounds 3 §2.11.2), drawn by the root element in place
    /// of its own background: the background of `source` (the root element, or the body whose
    /// background propagates to it) over the whole of `canvas` (scene space). Its images are
    /// positioned against the root element's boxes, as if painted for it alone. Drawn in the root
    /// element's effect group (its opacity, filters and blend mode apply), not under its transform.
    pub(super) fn draw_canvas_background(
        &self,
        scene: &mut impl PaintScene,
        source: NodeId,
        canvas: Rect,
    ) {
        let Some(node) = self.context.dom.get_node(source) else {
            return;
        };
        let (Some(style), Some(element)) = (node.primary_styles(), node.element_data()) else {
            return;
        };
        if self.transform.determinant().abs() < 1e-12 {
            return;
        }
        let canvas = self.transform.inverse().transform_rect_bbox(canvas);
        let style = (*style).clone();
        self.draw_background_layers(scene, &style, &element.background_images, Some(canvas), None, None);
    }

    /// The background of `style` with the images `image_data`, over this element's background
    /// painting area, or over `canvas` (this element's space) for the canvas background.
    /// pyro: `text_mask`, for a table part's background drawn in one of its cells: that cell and
    /// its transform, whose text `background-clip: text` layers show in (with a canvas, text
    /// clips are otherwise border-box).
    fn draw_background_layers(
        &self,
        scene: &mut impl PaintScene,
        style: &ComputedValues,
        image_data: &[Option<ImageResourceData>],
        canvas: Option<Rect>,
        text_mask: Option<(NodeId, Affine)>,
        overflow_clip: Option<&BezPath>,
    ) {
        let text_clips = canvas.is_none() || text_mask.is_some();
        let bg_styles = &style.get_background();
        let layer_count = bg_styles.background_image.0.len();

        // pyro: `background-attachment: local` layers of a scroll container see the boxes of its
        // scrolled content, cut to its padding box.
        let local = canvas
            .is_none()
            .then(|| self.local_frame())
            .flatten()
            .map(|frame| ElementCx {
                frame,
                style: self.style.clone(),
                ..*self
            });
        let local_cx = |attachment: StyloBackgroundAttachment| {
            local
                .as_ref()
                .filter(|_| attachment == StyloBackgroundAttachment::Local)
        };

        // pyro: with a `border-shape`, the background is clipped to its inner shape (to the shape,
        // with one) instead of the rounded box (CSS Borders 4 §11).
        let border_shape = canvas.is_none().then(|| self.border_shape()).flatten();
        let shape_clip = border_shape.as_ref().map(|shape| shape.background_clip());
        let shape_bounds = border_shape.as_ref().map(|shape| {
            use kurbo::Shape as _;
            shape.outer_path().bounding_box()
        });

        // The background color is clipped by the clip of the last layer in the list
        let last_clip = *get_cyclic(&bg_styles.background_clip.0, layer_count - 1);
        let last_attachment = *get_cyclic(&bg_styles.background_attachment.0, layer_count - 1);
        let background_clip: BoxModelBox = last_clip.into();
        // pyro: Chromium's bleed avoidance for a rounded box whose border hides its edge, worked
        // out when a layer cut to the border box paints.
        let shrunk_border_box = std::cell::OnceCell::new();
        let clip_path = |clip: BoxModelBox,
                         border_area: bool,
                         attachment: StyloBackgroundAttachment| {
            let path = match (canvas, local_cx(attachment)) {
                (Some(canvas), _) => canvas.to_path(0.1),
                (None, Some(local)) => local.box_path(clip),
                (None, None) if border_area => self.border_area_path(),
                (None, None) if shape_clip.is_some() => {
                    let shape = shape_clip.clone().unwrap();
                    match clip {
                        BoxModelBox::BorderBox => shape,
                        BoxModelBox::NoClip => self.box_path(clip),
                        _ => {
                            use crate::kurbo_css::contoured::{boolean, flatten, rect_polygon};
                            let shapes = [flatten(&shape), vec![rect_polygon(self.box_rect(clip))]];
                            boolean(&shapes, |inside| inside[0] && inside[1])
                        }
                    }
                }
                (None, None) => match clip {
                    BoxModelBox::BorderBox => shrunk_border_box
                        .get_or_init(|| self.bleed_avoiding_border_box(style))
                        .clone()
                        .unwrap_or_else(|| self.box_path(clip)),
                    _ => self.box_path(clip),
                },
            };
            if overflow_clip == Some(&path) { self.frame.border_box.to_path(0.1) } else { path }
        };

        // pyro: `background-blend-mode`: each layer blends with the layers and the colour under it,
        // in an isolated group (Compositing 1 §8.1: not with what lies behind the element).
        // Modes past the image layers are not used.
        let blends = (0..layer_count).any(|idx| {
            *get_cyclic(&bg_styles.background_blend_mode.0, idx) != style::values::computed::effects::Blend::Normal
        });
        let group = canvas.unwrap_or(self.frame.border_box);
        let blend_group = self.context.layer_manager.maybe_push_layer(
            scene,
            blends,
            BlendMode::default(),
            ALMOST_OPAQUE,
            self.transform,
            &group,
            None,
            None,
        );

        // Draw background color (if any)
        // pyro: the run of layers cut to the text, and whether its mask takes in the border area.
        let mut text_run = matches!(last_clip, StyloBackgroundClip::Text | StyloBackgroundClip::BorderAreaText)
            && text_clips;
        let mut run_border_area = last_clip == StyloBackgroundClip::BorderAreaText;
        if text_run {
            self.begin_text_clip_group(scene, canvas);
        }
        let current_color = style.clone_color();
        let bg_color = bg_styles
            .background_color
            .resolve_to_absolute(&current_color)
            .as_srgb_color();
        let color_with_image = bg_color != Color::TRANSPARENT
            && canvas.is_none()
            && !text_run
            && !blends
            && self.frame.has_border_radius()
            && local_cx(last_attachment).is_none()
            && !layer_paints_nothing(&ImageLayerStyles::from_background(bg_styles, image_data, layer_count - 1));
        if bg_color != Color::TRANSPARENT && !color_with_image {
            // pyro: the colour's clip only when there is a colour.
            let background_clip_path = clip_path(
                background_clip,
                last_clip == StyloBackgroundClip::BorderArea,
                last_attachment,
            );
            let local_color = local_cx(last_attachment).is_some();
            if local_color {
                scene.push_clip_layer(
                    Fill::NonZero,
                    self.transform,
                    &self.frame.padding_box_path(),
                );
            }
            let color_transform = if canvas.is_none()
                && last_clip == StyloBackgroundClip::BorderBox
                && !local_color
                && shape_clip.is_none()
                && shrunk_border_box.get().is_some_and(Option::is_none)
                && self.frame.border_box.width() == self.frame.border_box.height()
                && self.frame.is_elliptical_border()
            {
                let [xx, yx, xy, yy, ..] = self.transform.as_coeffs();
                if xx == yy && xy == -yx && yx != 0.0 {
                    let center = self.frame.border_box.center();
                    let scale = xx.hypot(yx);
                    Affine::translate((self.transform * center).to_vec2())
                        * Affine::scale(scale)
                        * Affine::translate(-center.to_vec2())
                } else {
                    self.transform
                }
            } else {
                self.transform
            };
            scene.fill(
                Fill::NonZero,
                color_transform,
                bg_color,
                None,
                &background_clip_path,
            );
            if local_color {
                scene.pop_layer();
            }
        }

        for idx in (0..layer_count).rev() {
            let layer = ImageLayerStyles::from_background(bg_styles, image_data, idx);
            let layer_text_clip = layer.text_clip && text_clips;
            let layer_border_area = layer_text_clip && layer.text_border_area;
            if layer_text_clip != text_run || (text_run && layer_border_area != run_border_area) {
                if text_run {
                    self.end_text_clip_group(scene, canvas, text_mask, run_border_area);
                }
                if layer_text_clip {
                    self.begin_text_clip_group(scene, canvas);
                }
                text_run = layer_text_clip;
                run_border_area = layer_border_area;
            }
            if layer_paints_nothing(&layer) {
                continue;
            }
            let background_clip_path = clip_path(layer.clip, layer.border_area, layer.attachment);
            let local = local_cx(layer.attachment);
            let cx = local.unwrap_or(self);
            let area = canvas.unwrap_or_else(|| cx.box_rect(layer.clip));
            // pyro: a `border-shape` reaching out of the box takes the painting area with it, to
            // the whole pixels its edge reaches into (what is painted is cut to the shape: the
            // image covers its partly covered pixels too; WPT css-backgrounds
            // `clip-border-area-border-shape`).
            let area = match &shape_bounds {
                Some(bounds) if canvas.is_none() => area.union(bounds.expand()),
                _ => area,
            };

            if local.is_some() {
                scene.push_clip_layer(
                    Fill::NonZero,
                    self.transform,
                    &self.frame.padding_box_path(),
                );
            }
            // pyro: no clip layer where nothing reaches past the clip: one image, not repeated,
            // inside a rectangular clip box (Chromium clips a background to a pixel rectangle
            // without a layer; a vello_cpu layer rounds what it holds a level apart from the
            // same image drawn directly, as an `<img>` is: WPT css-images `object-position-svg-*`).
            let blend = *get_cyclic(&bg_styles.background_blend_mode.0, idx);
            let unclipped = blend == style::values::computed::effects::Blend::Normal
                && canvas.is_none()
                && local.is_none()
                && !layer.border_area
                && shape_clip.is_none()
                && !self.frame.has_border_radius()
                && self.frame.corner_curvature.is_none()
                && !(layer.clip == BoxModelBox::BorderBox && shrunk_border_box.get().is_some_and(Option::is_some))
                && self.image_tile_within(&layer, area);
            let clipped = self.context.layer_manager.maybe_push_layer(
                scene,
                !unclipped,
                crate::blend::convert_blend_mode(blend),
                1.0,
                self.transform,
                &background_clip_path,
                None,
                None,
            );
            if color_with_image && idx == layer_count - 1 {
                if clipped {
                    let bounds = self.transform.transform_rect_bbox(background_clip_path.bounding_box()).expand();
                    scene.fill(Fill::NonZero, Affine::IDENTITY, bg_color, None, &bounds);
                } else {
                    scene.fill(Fill::NonZero, self.transform, bg_color, None, &background_clip_path);
                }
            }
            cx.draw_image_layer_in(scene, &layer, area, &current_color);
            self.context.layer_manager.maybe_pop_layer(scene, clipped);
            if local.is_some() {
                scene.pop_layer();
            }
        }
        if text_run {
            self.end_text_clip_group(scene, canvas, text_mask, run_border_area);
        }
        self.context.layer_manager.maybe_pop_layer(scene, blend_group);
    }

    /// pyro: the border box that `background-clip: border-box` layers are cut to under a rounded
    /// border that hides the background's edge, as Chromium paints it
    /// (`BoxDecorationData::ComputeBleedAvoidance`, `kBackgroundBleedShrinkBackground`): every
    /// side's colour opaque and its style neither dotted, dashed nor hidden, some border drawn,
    /// no border image. The box is inset by half of each side's width (a sixth with a `double`
    /// side) and its radii shrink with it, so the border covers the background's antialiased
    /// edge: drawn to the outer edge, the background showed through the border's own
    /// antialiasing there, a dark seam around a light border. `None` when the background is cut
    /// to the border box as it is (also under a side narrower than two device pixels).
    fn bleed_avoiding_border_box(&self, style: &ComputedValues) -> Option<BezPath> {
        use style::values::specified::BorderStyle;
        if !self.frame.has_border_radius() {
            return None;
        }
        let border = style.get_border();
        if !matches!(border.border_image_source, GenericImage::None) {
            return None;
        }
        let current_color = style.clone_color();
        let widths = self.frame.border_width;
        let sides = [
            (
                &border.border_top_color,
                border.border_top_style,
                widths.y0,
            ),
            (
                &border.border_right_color,
                border.border_right_style,
                widths.x1,
            ),
            (
                &border.border_bottom_color,
                border.border_bottom_style,
                widths.y1,
            ),
            (
                &border.border_left_color,
                border.border_left_style,
                widths.x0,
            ),
        ];
        if !sides.iter().any(|(_, _, width)| *width > 0.0) {
            return None;
        }
        let mut double = false;
        for (color, side_style, width) in sides {
            let opaque = color.resolve_to_absolute(&current_color).alpha >= 1.0;
            if !opaque
                || matches!(
                    side_style,
                    BorderStyle::Hidden | BorderStyle::Dotted | BorderStyle::Dashed
                )
            {
                return None;
            }
            // A side under two device pixels keeps the background to the outer edge: the
            // background's edge, half a pixel in, and the border's two antialiased edges share
            // its pixels, and what is under the box showed through where their coverages met
            // (WPT `filter-effects/blur-clip-border-radius`, a 1px border).
            if width > 0.0 && width < 2.0 {
                return None;
            }
            // A double border under 3px is drawn solid (Blink's `BorderEdge::EffectiveStyle`).
            double |= side_style == BorderStyle::Double && width >= 3.0 * self.scale;
        }
        let fraction = if double { 1.0 / 6.0 } else { 0.5 };
        let insets = kurbo::Insets::new(
            widths.x0 * fraction,
            widths.y0 * fraction,
            widths.x1 * fraction,
            widths.y1 * fraction,
        );
        let radii = self.frame.border_radii;
        let shrink = |r: Vec2, dx: f64, dy: f64| Vec2::new((r.x - dx).max(0.0), (r.y - dy).max(0.0));
        let radii = NonUniformRoundedRectRadii {
            top_left: shrink(radii.top_left, insets.x0, insets.y0),
            top_right: shrink(radii.top_right, insets.x1, insets.y0),
            bottom_right: shrink(radii.bottom_right, insets.x1, insets.y1),
            bottom_left: shrink(radii.bottom_left, insets.x0, insets.y1),
        };
        let shrunk = CssBox::new(
            self.frame.border_box - insets,
            kurbo::Insets::ZERO,
            kurbo::Insets::ZERO,
            0.0,
            radii,
        )
        .contoured_like(&self.frame);
        Some(shrunk.border_box_path())
    }

    /// pyro: this element's boxes as a `background-attachment: local` layer sees them on a scroll
    /// container: those of its scrolled content, the padding box grown to the scrollable overflow
    /// and moved by the scroll offset, without corner radii (the padding box, which clips the
    /// layer, has them). `None` for an element that does not scroll: `local` is then `scroll`.
    fn local_frame(&self) -> Option<CssBox> {
        let box_style = self.style.get_box();
        let scrolls = |overflow: Overflow| !matches!(overflow, Overflow::Visible | Overflow::Clip);
        if !(scrolls(box_style.overflow_x) || scrolls(box_style.overflow_y)) {
            return None;
        }
        let scale = self.scale;
        let padding_box = self.frame.padding_box;
        // The scrollable overflow, relative to the padding box's top left (CSS px), with the
        // padding box itself: the area the content scrolls in.
        let overflow = self.node.final_layout().scrollable_overflow_rect.rect();
        let area = Rect::new(
            0.0,
            0.0,
            padding_box.width() / scale,
            padding_box.height() / scale,
        )
        .union(Rect::new(
            overflow.left as f64,
            overflow.top as f64,
            overflow.right as f64,
            overflow.bottom as f64,
        ));
        let scroll = self.node.scroll_offset();
        let area = area - Vec2::new(scroll.x, scroll.y);
        let scrolled_padding_box = Rect::new(
            padding_box.x0 + area.x0 * scale,
            padding_box.y0 + area.y0 * scale,
            padding_box.x0 + area.x1 * scale,
            padding_box.y0 + area.y1 * scale,
        );
        Some(CssBox::new(
            scrolled_padding_box + self.frame.border_width,
            self.frame.border_width,
            self.frame.padding_width,
            0.0,
            NonUniformRoundedRectRadii::default(),
        ))
    }

    /// Starts an isolated group for `background-clip: text` layers; layers drawn
    /// until [`end_text_clip_group`](Self::end_text_clip_group) are clipped to text.
    /// pyro: over `canvas` when there is one (a table part's background in a cell).
    fn begin_text_clip_group(&self, scene: &mut impl PaintScene, canvas: Option<Rect>) {
        let area = canvas.map_or_else(|| self.frame.border_box_path(), |canvas| canvas.to_path(0.1));
        scene.push_layer(
            Fill::NonZero,
            Mix::Normal,
            ALMOST_OPAQUE,
            self.transform,
            &area,
            None,
            None,
        );
    }

    /// Ends the group started by [`begin_text_clip_group`](Self::begin_text_clip_group):
    /// draws the text mask in a `DestIn` layer, then pops both layers.
    fn end_text_clip_group(
        &self,
        scene: &mut impl PaintScene,
        canvas: Option<Rect>,
        text_mask: Option<(NodeId, Affine)>,
        border_area: bool,
    ) {
        let area = canvas.map_or_else(|| self.frame.border_box_path(), |canvas| canvas.to_path(0.1));
        scene.push_layer(
            Fill::NonZero,
            BlendMode::new(Mix::Normal, Compose::DestIn),
            1.0,
            self.transform,
            &area,
            None,
            None,
        );
        let (node, transform) = text_mask.unwrap_or((self.node.id, self.transform));
        self.context.draw_text_mask(scene, node, transform, true);
        if border_area {
            scene.fill(Fill::NonZero, self.transform, Color::BLACK, None, &self.border_area_path());
        }
        scene.pop_layer();
        scene.pop_layer();
    }

    /// The path of the given CSS box model box for this element
    pub(super) fn box_path(&self, css_box: BoxModelBox) -> BezPath {
        match css_box {
            BoxModelBox::BorderBox => self.frame.border_box_path(),
            BoxModelBox::PaddingBox => self.frame.padding_box_path(),
            BoxModelBox::ContentBox => self.frame.content_box_path(),
            BoxModelBox::NoClip => self.visible_area().to_path(0.1),
        }
    }

    /// The rect of the given CSS box model box for this element
    fn box_rect(&self, css_box: BoxModelBox) -> Rect {
        match css_box {
            BoxModelBox::BorderBox => self.frame.border_box,
            BoxModelBox::PaddingBox => self.frame.padding_box,
            BoxModelBox::ContentBox => self.frame.content_box,
            BoxModelBox::NoClip => self.visible_area(),
        }
    }

    /// Draw a single layer of a CSS image layer list (`background-image` or `mask-image`)
    pub(super) fn draw_image_layer(&self, scene: &mut impl PaintScene, layer: &ImageLayerStyles) {
        let current_color = self.style.clone_color();
        self.draw_image_layer_in(scene, layer, self.box_rect(layer.clip), &current_color);
    }

    /// Draw a single layer over `area`, its background painting area (this element's space, device
    /// px): the clip box, or the canvas. `current_color` resolves `currentcolor` in gradients.
    fn draw_image_layer_in(
        &self,
        scene: &mut impl PaintScene,
        layer: &ImageLayerStyles,
        area: Rect,
        current_color: &style::color::AbsoluteColor,
    ) {
        match layer.stylo_image {
            GenericImage::None => {
                // Do nothing
            }
            GenericImage::Gradient(gradient) => {
                self.draw_gradient_layer(scene, gradient, layer, area, current_color)
            }
            GenericImage::Url(_) => {
                self.draw_raster_image_layer(scene, layer, area);
                #[cfg(feature = "svg")]
                self.draw_svg_image_layer(scene, layer, area);
            }
            GenericImage::LightDark(_) => {
                #[cfg(feature = "tracing")]
                warn!("Implement image layer drawing for ImageLightDark")
            }
            GenericImage::PaintWorklet(_) => {
                #[cfg(feature = "tracing")]
                warn!("Implement image layer drawing for Image::PaintWorklet")
            }
            // pyro: `cross-fade()` (CSS Images 4 § 2.6): its images (and colours, as images of
            // that colour) each drawn as the layer would draw it, weighted by its percentage
            // (those left out share what the others leave of 100%, a sum past 100% is scaled
            // down to it) and added up premultiplied (`plus-lighter`) in a group.
            GenericImage::CrossFade(cross_fade) => {
                use style::values::generics::image::GenericCrossFadeImage;
                use style::values::generics::Optional;
                let elements = &cross_fade.elements;
                let given: Vec<Option<f32>> = elements
                    .iter()
                    .map(|element| match &element.percent {
                        Optional::Some(percent) => Some(percent.0.clamp(0.0, 1.0)),
                        Optional::None => None,
                    })
                    .collect();
                let sum: f32 = given.iter().flatten().sum();
                let left_out = given.iter().filter(|weight| weight.is_none()).count();
                let share = if left_out > 0 { (1.0 - sum).max(0.0) / left_out as f32 } else { 0.0 };
                let weights: Vec<f32> = given.iter().map(|weight| weight.unwrap_or(share)).collect();
                let total: f32 = weights.iter().sum();
                let scale = if total > 1.0 { 1.0 / total } else { 1.0 };
                // A `url()` image: its slot among the element's stored images
                // (`blitz_dom::util::image_slots`), found by its URL.
                let data_of = |image| match blitz_dom::util::unfiltered_image(image) {
                    // pyro: and its request (a slot of the same URL fetched with other
                    // modifiers is not its image).
                    GenericImage::Url(style::values::computed::url::ComputedUrl::Valid(url, modifiers)) => self
                        .element
                        .background_images
                        .iter()
                        .chain(&self.element.mask_images)
                        .flatten()
                        .find(|data| {
                            *data.url == **url && blitz_dom::net::asks_same_request(&data.options, modifiers.as_deref())
                        }),
                    _ => layer.image_data,
                };
                // pyro: the natural size of the cross-fade: the weighted average of those of its
                // images that have one (CSS Images 4 § 2.6), and each image drawn at the size
                // the cross-fade gets, so a smaller image is stretched to it (it was drawn at
                // its own size).
                let mut natural = (0.0f32, 0.0f32, 0.0f32);
                for (element, weight) in elements.iter().zip(&weights) {
                    let GenericCrossFadeImage::Image(image) = &element.image else { continue };
                    let size = data_of(image).and_then(|data| match &data.image {
                        ImageData::Raster(raster) => {
                            let unoriented = blitz_dom::util::image_unoriented(&self.style) && raster.orientation != 1;
                            let (w, h) = raster.shown_size(unoriented);
                            Some((w as f32 / layer.resolution, h as f32 / layer.resolution))
                        }
                        #[cfg(feature = "svg")]
                        ImageData::Svg(svg) => svg.intrinsic_width().zip(svg.intrinsic_height()),
                        #[allow(unreachable_patterns)]
                        _ => None,
                    });
                    if let Some((w, h)) = size.filter(|(w, h)| *w > 0.0 && *h > 0.0) {
                        natural = (natural.0 + w * weight, natural.1 + h * weight, natural.2 + weight);
                    }
                }
                let sized = (natural.2 > 0.0).then(|| {
                    let (w, h) = (natural.0 / natural.2, natural.1 / natural.2);
                    let (origin_rect, _, _) = self.positioning_area(layer, area);
                    let size = compute_layer_size(
                        layer,
                        (origin_rect.width() / self.scale) as f32,
                        (origin_rect.height() / self.scale) as f32,
                        BackgroundSizeComputeMode::Intrinsic { width: Some(w), height: Some(h), ratio: Some(w / h) },
                    );
                    use style::values::generics::length::GenericLengthPercentageOrAuto as Lpa;
                    use style::values::generics::NonNegative;
                    let px = |v: f64| {
                        Lpa::LengthPercentage(NonNegative(style::values::computed::LengthPercentage::new_length(
                            style::values::computed::Length::new(v as f32),
                        )))
                    };
                    BackgroundSize::ExplicitSize { width: px(size.width), height: px(size.height) }
                });
                let size = sized.as_ref().unwrap_or(layer.size);
                let clip = area.to_path(0.1);
                scene.push_layer(Fill::NonZero, BlendMode::default(), 1.0, self.transform, &clip, None, None);
                for (element, weight) in elements.iter().zip(weights) {
                    let weight = weight * scale;
                    if weight <= 0.0 {
                        continue;
                    }
                    scene.push_layer(
                        Fill::NonZero,
                        BlendMode::new(Mix::Normal, Compose::PlusLighter),
                        weight,
                        self.transform,
                        &clip,
                        None,
                        None,
                    );
                    match &element.image {
                        GenericCrossFadeImage::Image(image) => {
                            let inner = ImageLayerStyles { stylo_image: image, image_data: data_of(image), size, ..*layer };
                            self.draw_image_layer_in(scene, &inner, area, current_color);
                        }
                        // A colour is an image of that colour without a natural size: in the
                        // cross-fade's tiles when it has a size.
                        GenericCrossFadeImage::Color(color) if sized.is_some() => {
                            let color = color.resolve_to_absolute(current_color).as_dynamic_color();
                            let inner = ImageLayerStyles { size, ..*layer };
                            self.draw_generated_layer(scene, &inner, area, |_, _| {
                                let mut solid = peniko::Gradient::new_linear((0.0, 0.0), (1.0, 0.0));
                                for offset in [0.0, 1.0] {
                                    solid.stops.push(peniko::ColorStop { color, offset });
                                }
                                (solid, None)
                            })
                        }
                        GenericCrossFadeImage::Color(color) => {
                            let color = color.resolve_to_absolute(current_color).as_srgb_color();
                            scene.fill(Fill::NonZero, self.transform, color, None, &clip);
                        }
                    }
                    scene.pop_layer();
                }
                scene.pop_layer();
            }
            // pyro: `image(<color>)`: an image of that colour without a natural size, sized,
            // positioned and tiled as a gradient is.
            GenericImage::Image(color) => {
                let color = color.resolve_to_absolute(current_color).as_dynamic_color();
                self.draw_generated_layer(scene, layer, area, |_, _| {
                    let mut solid = peniko::Gradient::new_linear((0.0, 0.0), (1.0, 0.0));
                    for offset in [0.0, 1.0] {
                        solid.stops.push(peniko::ColorStop { color, offset });
                    }
                    (solid, None)
                })
            }
            // pyro: `image-set()`: the option Stylo selected for the device's resolution (none
            // when every option was dropped: an unsupported `type()`, a resolution of 0).
            GenericImage::ImageSet(set) => {
                if let Some(item) = set.items.get(set.selected_index) {
                    let resolution = item.resolution.dppx();
                    let inner = ImageLayerStyles {
                        stylo_image: &item.image,
                        resolution: if resolution > 0.0 { layer.resolution * resolution } else { layer.resolution },
                        ..*layer
                    };
                    self.draw_image_layer_in(scene, &inner, area, current_color);
                }
            }
            // pyro: `filter()`: the image it filters, drawn in a layer with the filters over the
            // painting area (so a `drop-shadow()` stays inside it).
            GenericImage::Filter(_) => {
                let (image, filter) = crate::filters::image_filter(
                    layer.stylo_image,
                    self.scale,
                    current_color,
                    self.dom,
                );
                let inner = ImageLayerStyles {
                    stylo_image: image,
                    ..*layer
                };
                let filtered = filter.is_some();
                if filtered {
                    scene.push_layer(
                        Fill::NonZero,
                        Mix::Normal,
                        1.0,
                        self.transform,
                        &area,
                        filter,
                        None,
                    );
                }
                self.draw_image_layer_in(scene, &inner, area, current_color);
                if filtered {
                    scene.pop_layer();
                }
            }
        }
    }

    /// Paint the backgrounds of the table's column groups, columns, row groups and rows
    /// (in that order, bottom to top: CSS 2.1 §17.5.1).
    ///
    /// These backgrounds are only painted behind the table's cells (not in the
    /// `border-spacing` between them, nor where a row has no cell), so they are
    /// painted cell by cell.
    pub(super) fn draw_table_row_backgrounds(&self, scene: &mut impl PaintScene) {
        let SpecialElementData::TableRoot(table) = &self.element.special_data else {
            return;
        };

        // pyro: a positioned row group or row paints its background with its cells, in the
        // positioned layer (`draw_positioned_table_part_backgrounds`), not here.
        let positioned = |node_id: NodeId| -> bool {
            self.context.dom.get_node(node_id).and_then(|node| node.primary_styles()).is_some_and(|style| {
                use style::computed_values::position::T as Position;
                matches!(style.clone_position(), Position::Relative | Position::Sticky)
            })
        };
        let background_color = |node_id: NodeId| -> Option<Color> {
            if positioned(node_id) {
                return None;
            }
            let node = self.context.dom.get_node(node_id)?;
            let style = node.primary_styles()?;
            let current_color = style.clone_color();
            let color = style
                .get_background()
                .background_color
                .resolve_to_absolute(&current_color)
                .as_srgb_color();
            (color != Color::TRANSPARENT).then_some(color)
        };

        // Resolve the (group, track) background colors once per track rather than once per cell
        let column_colors: Vec<[Option<Color>; 2]> = table
            .columns
            .iter()
            .map(|col| {
                [
                    col.group.and_then(background_color),
                    background_color(col.node_id),
                ]
            })
            .collect();
        let row_colors: Vec<[Option<Color>; 2]> = table
            .rows
            .iter()
            .map(|row| {
                [
                    row.group.and_then(background_color),
                    background_color(row.node_id),
                ]
            })
            .collect();
        // pyro: a part with a background image paints it (with its colour) cell by cell too,
        // positioned against the part's own box (CSS 2.1 §17.5.1). So does one whose colour is
        // cut to the text (`background-clip: text`): the cells' text masks it.
        let has_image = |node_id: NodeId| -> bool {
            !positioned(node_id) && self.context.dom.get_node(node_id).and_then(|node| node.primary_styles()).is_some_and(|style| {
                let background = style.get_background();
                background.background_image.0.iter().any(|image| !matches!(image, ComputedImage::None))
                    || matches!(
                        background.background_clip.0.last(),
                        Some(StyloBackgroundClip::Text | StyloBackgroundClip::BorderAreaText)
                    )
            })
        };
        let column_images: Vec<[bool; 2]> =
            table.columns.iter().map(|col| [col.group.is_some_and(has_image), has_image(col.node_id)]).collect();
        let row_images: Vec<[bool; 2]> =
            table.rows.iter().map(|row| [row.group.is_some_and(has_image), has_image(row.node_id)]).collect();
        let has_color =
            |colors: &[[Option<Color>; 2]]| colors.iter().flatten().any(Option::is_some);
        let any_image = |images: &[[bool; 2]]| images.iter().flatten().any(|image| *image);
        // pyro: the rows' outer box shadows, each painted before its row's background (as
        // Chromium paints a row's shadow, then its background, row after row).
        let row_shadows: Vec<bool> = table
            .rows
            .iter()
            .map(|row| {
                !positioned(row.node_id)
                    && self.context.dom.get_node(row.node_id).and_then(|node| node.primary_styles()).is_some_and(|style| {
                        style.get_effects().box_shadow.0.iter().any(|shadow| !shadow.inset)
                    })
            })
            .collect();
        let any_shadow = row_shadows.iter().any(|shadow| *shadow);
        if !has_color(&column_colors)
            && !has_color(&row_colors)
            && !any_image(&column_images)
            && !any_image(&row_images)
            && !any_shadow
        {
            return;
        }
        let part_rects = (any_image(&column_images) || any_image(&row_images) || any_shadow)
            .then(|| self.table_part_rects(table))
            .flatten();
        let mut shadowed_row = None;

        let cell_rect = |cell_id: NodeId| -> Option<Rect> {
            let layout = self.context.dom.get_node(cell_id)?.final_layout();
            Some(Rect::new(
                layout.location.x as f64,
                layout.location.y as f64,
                (layout.location.x + layout.size.width) as f64,
                (layout.location.y + layout.size.height) as f64,
            ))
        };

        // In the collapsed borders model the borders are laid out as gutters between the
        // cells, but a cell's border box (and so its background) extends to the middle of
        // the borders around it. The gutter is split on a whole pixel so that the
        // backgrounds of adjacent cells tile exactly.
        let collapsed = table.border_collapse == BorderCollapse::Collapse;
        let (gutter, bounds) = if collapsed {
            let gap = table.style.gap.map(|gap| gap.into_raw().value() as f64);
            let bounds = table
                .cells
                .iter()
                .filter_map(|cell| cell_rect(cell.node_id))
                .reduce(|a, b| a.union(b));
            (Size::new(gap.width, gap.height), bounds)
        } else {
            (Size::ZERO, None)
        };
        let before = Size::new((gutter.width / 2.0).ceil(), (gutter.height / 2.0).ceil());
        let after = gutter - before;

        for cell in &table.cells {
            // A collapsed row (`visibility: collapse`) paints nothing.
            if table.rows.get(cell.row as usize).is_some_and(|row| row.collapsed) {
                continue;
            }
            // pyro: a row's shadow at its first cell (a row's cells follow one another).
            if any_shadow && row_shadows.get(cell.row as usize) == Some(&true) && shadowed_row != Some(cell.row) {
                shadowed_row = Some(cell.row);
                if let (Some(rects), Some(row)) = (part_rects.as_ref(), table.rows.get(cell.row as usize)) {
                    let part = rects.rect(table, row.node_id, PartKind::Row, cell);
                    self.draw_table_part_shadow(scene, row.node_id, part);
                }
            }
            let no_colors = [None, None];
            let no_images = [false, false];
            let column = column_colors
                .get(cell.column as usize)
                .unwrap_or(&no_colors);
            let row = row_colors.get(cell.row as usize).unwrap_or(&no_colors);
            let column_image = column_images.get(cell.column as usize).unwrap_or(&no_images);
            let row_image = row_images.get(cell.row as usize).unwrap_or(&no_images);
            let images = column_image.iter().chain(row_image).any(|image| *image);
            let mut colors = column.iter().chain(row).flatten().peekable();
            if colors.peek().is_none() && !images {
                continue;
            }
            let Some(mut rect) = cell_rect(cell.node_id) else {
                continue;
            };
            if let Some(bounds) = bounds {
                rect = Rect::new(
                    rect.x0 - before.width,
                    rect.y0 - before.height,
                    rect.x1 + after.width,
                    rect.y1 + after.height,
                )
                .intersect(bounds);
            }

            let shape = rect.scale_from_origin(self.scale);
            // pyro: a `position: sticky` row group or row is painted where its scroll container
            // holds it (`Node::sticky_offset`), as its cells are.
            let mut sticky = Vec2::ZERO;
            if let Some(row) = table.rows.get(cell.row as usize) {
                for id in [row.group, Some(row.node_id)].into_iter().flatten() {
                    if let Some(part) = self.context.dom.get_node(id) {
                        let offset = part.sticky_offset();
                        sticky += Vec2::new(offset.x, offset.y);
                    }
                }
            }
            let transform = self.transform
                * Affine::translate(Vec2::new((sticky.x * self.scale).round(), (sticky.y * self.scale).round()));
            // pyro: in the cell's `border-shape`, which the backgrounds of its table parts take as
            // the cell's own (CSS Borders 4 § 11; WPT css-backgrounds
            // `clip-border-shape-table-part-background`).
            let cell_shape = self.context.dom.get_node(cell.node_id).and_then(|cell_node| {
                let layout = *cell_node.final_layout();
                let location = Vec2::new(layout.location.x as f64, layout.location.y as f64);
                let cell_transform = transform * Affine::translate(location * self.scale);
                let cell_cx = self.context.element_cx(cell_node, layout, cell_transform, None);
                cell_cx.border_shape().map(|shape| (cell_transform, shape.background_clip()))
            });
            if let Some((cell_transform, path)) = &cell_shape {
                scene.push_clip_layer(Fill::NonZero, *cell_transform, path);
            }
            if !images {
                for color in colors {
                    scene.fill(Fill::NonZero, transform, *color, None, &shape);
                }
                if cell_shape.is_some() {
                    scene.pop_layer();
                }
                continue;
            }
            // Bottom to top: column group, column, row group, row.
            let column_node = table.columns.get(cell.column as usize);
            let row_node = table.rows.get(cell.row as usize);
            let parts = [
                (column_node.and_then(|col| col.group), column[0], column_image[0], PartKind::ColumnGroup),
                (column_node.map(|col| col.node_id), column[1], column_image[1], PartKind::Column),
                (row_node.and_then(|row| row.group), row[0], row_image[0], PartKind::RowGroup),
                (row_node.map(|row| row.node_id), row[1], row_image[1], PartKind::Row),
            ];
            for (node_id, color, image, kind) in parts {
                let Some(node_id) = node_id else { continue };
                match (image, part_rects.as_ref()) {
                    (true, Some(rects)) => {
                        let part = rects.rect(table, node_id, kind, cell);
                        let location = Vec2::new(rect.x0, rect.y0);
                        let cell_location = self
                            .context
                            .dom
                            .get_node(cell.node_id)
                            .map_or(location, |node| {
                                let location = node.final_layout().location;
                                Vec2::new(location.x as f64, location.y as f64)
                            });
                        let cell_transform = transform * Affine::translate(cell_location * self.scale);
                        self.draw_table_part_background(
                            scene,
                            node_id,
                            part,
                            &shape,
                            transform,
                            (cell.node_id, cell_transform),
                        );
                    }
                    _ => {
                        if let Some(color) = color {
                            scene.fill(Fill::NonZero, transform, color, None, &shape);
                        }
                    }
                }
            }
            if cell_shape.is_some() {
                scene.pop_layer();
            }
        }
    }

    /// pyro: the backgrounds of the positioned row group and row of the table cell this is (see
    /// `BaseDocument::table_cell_positioned_parts`), group first, in the cell's border box: a
    /// positioned part paints with its cells, in the positioned layer, under each cell's own
    /// background (WPT `position-relative-table-*`, `position-sticky-table-*`).
    pub(super) fn draw_positioned_table_part_backgrounds(&self, scene: &mut impl PaintScene) {
        let dom = self.context.dom;
        let parts = dom.table_cell_positioned_parts(self.node.id);
        if parts.iter().all(Option::is_none) {
            return;
        }
        let Some(table_node) = self.node.layout_parent.get().and_then(|id| dom.get_node(id)) else { return };
        let Some(SpecialElementData::TableRoot(table)) = table_node.element_data().map(|e| &e.special_data) else {
            return;
        };
        let index = self.node.layout_data().table_cell_index as usize;
        let Some(cell) = table.cells.get(index).filter(|cell| cell.node_id == self.node.id) else { return };
        let layout = self.node.final_layout();
        let shape = Rect::new(0.0, 0.0, layout.size.width as f64, layout.size.height as f64).scale_from_origin(self.scale);
        let mut part_rects = None;
        for (part, kind) in parts.into_iter().zip([PartKind::RowGroup, PartKind::Row]) {
            let Some(part) = part else { continue };
            let Some(node) = dom.get_node(part) else { continue };
            let Some(style) = node.primary_styles() else { continue };
            let current_color = style.clone_color();
            let background = style.get_background();
            let color = background.background_color.resolve_to_absolute(&current_color).as_srgb_color();
            let image = background.background_image.0.iter().any(|image| !matches!(image, ComputedImage::None));
            if image {
                let rects = part_rects.get_or_insert_with(|| self.table_part_rects(table));
                if let Some(rects) = rects.as_ref() {
                    // The part's box in the cell's space.
                    let location = layout.location;
                    let rect = rects.rect(table, part, kind, cell) - Vec2::new(location.x as f64, location.y as f64);
                    self.draw_table_part_background(scene, part, rect, &shape, self.transform, (self.node.id, self.transform));
                    continue;
                }
            }
            if color != Color::TRANSPARENT {
                scene.fill(Fill::NonZero, self.transform, color, None, &shape);
            }
        }
    }

    /// pyro: the outlines of a table's row groups and rows (they paint no box of their own: the
    /// cells paint their backgrounds), each around the tracks it spans, groups first. A row with
    /// `visibility` other than `visible` (a collapsed one) has none.
    pub(crate) fn draw_table_part_outlines(&self, scene: &mut impl PaintScene) {
        let SpecialElementData::TableRoot(table) = &self.element.special_data else {
            return;
        };
        let dom = self.context.dom;
        let has_outline = |id: NodeId| {
            dom.get_node(id).and_then(|node| node.primary_styles()).is_some_and(|style| {
                let outline = style.get_outline();
                outline.outline_style != style::values::computed::OutlineStyle::none()
                    && outline.outline_width.0.to_f32_px() > 0.0
                    && style.get_inherited_box().visibility == style::computed_values::visibility::T::Visible
                    // (A positioned part paints as a layer of its own, its outline with it.)
                    && style.clone_position() == style::computed_values::position::T::Static
            })
        };
        let mut parts: Vec<(NodeId, Vec<usize>)> = Vec::new();
        for (at, row) in table.rows.iter().enumerate() {
            if let Some(group) = row.group.filter(|group| has_outline(*group)) {
                match parts.iter_mut().find(|(id, _)| *id == group) {
                    Some((_, rows)) => rows.push(at),
                    None => parts.push((group, vec![at])),
                }
            }
        }
        for (at, row) in table.rows.iter().enumerate() {
            if !row.collapsed && has_outline(row.node_id) {
                parts.push((row.node_id, vec![at]));
            }
        }
        if parts.is_empty() {
            return;
        }
        let Some(rects) = self.table_part_rects(table) else { return };
        let x0 = rects.columns.iter().map(|c| c.0).fold(f64::INFINITY, f64::min);
        let x1 = rects.columns.iter().map(|c| c.1).fold(f64::NEG_INFINITY, f64::max);
        for (id, rows) in parts {
            let open: Vec<usize> = rows.iter().copied().filter(|&at| !table.rows[at].collapsed).collect();
            let (Some(&first), Some(&last)) = (open.first(), open.last()) else { continue };
            let (Some(top), Some(bottom)) = (rects.rows.get(first), rects.rows.get(last)) else { continue };
            let Some(node) = dom.get_node(id) else { continue };
            if node.element_data().is_none() {
                continue;
            }
            let mut layout = taffy::Layout::new();
            layout.size = taffy::Size { width: (x1 - x0) as f32, height: (bottom.1 - top.0) as f32 };
            let transform = self.transform * Affine::translate(Vec2::new(x0, top.0) * self.scale);
            self.context.element_cx(node, layout, transform, None).draw_outline(scene);
        }
    }

    /// pyro: where a table's rows and columns are (CSS px, the table's space), for the boxes of its
    /// parts whose backgrounds are positioned against them.
    fn table_part_rects(&self, table: &blitz_dom::TableContext) -> Option<TablePartRects> {
        let grid_info = table.computed_grid_info.borrow();
        let grid_info = grid_info.as_ref()?;
        let top = table.grid_box.borrow().map_or(0.0, |(top, _)| top as f64);
        // (In logical order, as `TableContext::columns` index them: right to left in an `rtl` table.)
        let columns: Vec<(f64, f64)> =
            grid_info.columns.positions.iter().map(|track| (track.start as f64, track.end as f64)).collect();
        let rows: Vec<(f64, f64)> = super::PhysicalTracks::from_tracks(&grid_info.rows)
            .iter()
            .map(|track| (track.start as f64 + top, track.end as f64 + top))
            .collect();
        // pyro: a vertical table's grid runs its columns down the page and its rows across it, from
        // the right in `vertical-rl` (WPT css-tables `paint/col-paint-vrl-rtl`).
        let vertical = self.node.primary_styles().and_then(|style| {
            let wm = style.writing_mode;
            wm.is_vertical().then(|| (!wm.is_vertical_lr(), self.frame.border_box.width() / self.scale))
        });
        (!columns.is_empty() && !rows.is_empty()).then_some(TablePartRects { columns, rows, vertical })
    }

    /// pyro: the background of a table part (`part`, the box its images are positioned against,
    /// CSS px in the table's space) painted in `clip` (device px, the table's space): one cell.
    fn draw_table_part_background(
        &self,
        scene: &mut impl PaintScene,
        node_id: NodeId,
        part: Rect,
        clip: &Rect,
        base: Affine,
        (cell, cell_transform): (NodeId, Affine),
    ) {
        let Some(node) = self.context.dom.get_node(node_id) else { return };
        if node.element_data().is_none() || node.primary_styles().is_none() {
            return;
        }
        let mut layout = taffy::Layout::new();
        layout.size = taffy::Size { width: part.width() as f32, height: part.height() as f32 };
        let transform = base * Affine::translate(Vec2::new(part.x0, part.y0) * self.scale);
        let part_cx = self.context.element_cx(node, layout, transform, None);
        scene.push_clip_layer(Fill::NonZero, base, clip);
        // pyro: over the whole cell (`clip`, which a cell spanning rows or columns makes larger
        // than the part's box): the colour and the tiles cover it, the images positioned against
        // the part's box (Blink paints a part's background in the cell's rect).
        let canvas = *clip - Vec2::new(part.x0, part.y0) * self.scale;
        if let (Some(style), Some(element)) = (node.primary_styles(), node.element_data()) {
            let style = (*style).clone();
            part_cx.draw_background_layers(scene, &style, &element.background_images, Some(canvas), Some((cell, cell_transform)), None);
        }
        scene.pop_layer();
    }

    /// pyro: the outer box shadow of a table part (a row) whose box is `part` (CSS px, the table's
    /// space).
    fn draw_table_part_shadow(&self, scene: &mut impl PaintScene, node_id: NodeId, part: Rect) {
        let Some(node) = self.context.dom.get_node(node_id) else { return };
        if node.element_data().is_none() || node.primary_styles().is_none() {
            return;
        }
        let mut layout = taffy::Layout::new();
        layout.size = taffy::Size { width: part.width() as f32, height: part.height() as f32 };
        let transform = self.transform * Affine::translate(Vec2::new(part.x0, part.y0) * self.scale);
        self.context.element_cx(node, layout, transform, None).draw_outset_box_shadow(scene);
    }

    /// Whether the layer is positioned against the viewport
    /// (`background-attachment: fixed`) rather than the element's origin box.
    /// `fixed` behaves as `scroll` on elements affected by a CSS transform
    /// (the transformed element acts as the layer's containing block).
    fn layer_is_fixed(&self, layer: &ImageLayerStyles) -> bool {
        layer.attachment == StyloBackgroundAttachment::Fixed && !self.is_transformed()
    }

    /// The background positioning area and the transform from its coordinate
    /// space to the scene for a fixed layer: the viewport, unaffected by any
    /// scrolling.
    fn fixed_positioning_area(&self) -> (Rect, Affine) {
        let viewport_rect = Rect::new(
            0.0,
            0.0,
            self.context.width as f64,
            self.context.height as f64,
        );
        let transform = Affine::translate((self.context.initial_x, self.context.initial_y));
        (viewport_rect, transform)
    }

    /// Whether this element or any of its ancestors has a CSS transform
    ///
    /// pyro: a transform that resolves to the identity (`transform: translate(0)`), an
    /// individual transform property and `will-change: transform` count too: each degrades
    /// `fixed` to `scroll`, as in Chromium (WPT css/css-transforms/transform-fixed-bg-005/008).
    fn is_transformed(&self) -> bool {
        use style::values::generics::transform::{GenericRotate, GenericScale, GenericTranslate};
        use style::values::specified::box_::WillChangeBits;
        let mut current = Some(self.node.id);
        while let Some(node) = current.and_then(|id| self.context.dom.get_node(id)) {
            if node.transform().is_some() {
                return true;
            }
            if let Some(style) = node.primary_styles() {
                let box_style = style.get_box();
                if !box_style.transform.0.is_empty()
                    || !matches!(box_style.rotate, GenericRotate::None)
                    || !matches!(box_style.translate, GenericTranslate::None)
                    || !matches!(box_style.scale, GenericScale::None)
                    || box_style
                        .will_change
                        .bits
                        .contains(WillChangeBits::TRANSFORM)
                {
                    return true;
                }
            }
            current = node.parent;
        }
        false
    }

    #[cfg(feature = "svg")]
    fn draw_svg_image_layer(
        &self,
        scene: &mut impl PaintScene,
        layer: &ImageLayerStyles,
        area: Rect,
    ) {
        let Some(bg_image) = layer.image_data else {
            return;
        };
        let ImageData::Svg(svg) = &bg_image.image else {
            return;
        };
        // pyro: in this element's used colour scheme (`SvgImageData::for_color_scheme`).
        let schemed;
        let svg = match svg.for_color_scheme(self.dark_color_scheme()) {
            Some(dark) => {
                schemed = dark;
                &schemed
            }
            None => svg,
        };

        // A zero-sized `viewBox` disables rendering of the SVG
        if svg.intrinsic_dimensions.degenerate_view_box {
            return;
        }

        let (origin_rect, base_transform, area) = self.positioning_area(layer, area);

        let frame_w = origin_rect.width() / self.scale;
        let frame_h = origin_rect.height() / self.scale;

        // Size the SVG per the CSS default sizing algorithm
        // (https://drafts.csswg.org/css-images/#default-sizing). An SVG image
        // may lack an intrinsic width, height, and/or aspect ratio, so each is
        // passed separately (usvg's resolved `Tree::size` is only used as the
        // source coordinate space of the rendered tree).
        let intrinsic_width = svg.intrinsic_width().filter(|w| w.is_finite() && *w > 0.0);
        let intrinsic_height = svg.intrinsic_height().filter(|h| h.is_finite() && *h > 0.0);
        let aspect_ratio = match (intrinsic_width, intrinsic_height) {
            (Some(w), Some(h)) => Some(w / h),
            _ => svg.viewbox_aspect_ratio(),
        }
        .filter(|r| r.is_finite() && *r > 0.0);

        let (bg_pos, bg_size) = compute_layer_position_and_size(
            layer,
            frame_w,
            frame_h,
            BackgroundSizeComputeMode::Intrinsic {
                width: intrinsic_width,
                height: intrinsic_height,
                ratio: aspect_ratio,
            },
        );

        if bg_size.width <= 0.0 || bg_size.height <= 0.0 {
            return;
        }

        // pyro: an image without a `viewBox` or a size lays its content out in the size it is
        // drawn at (`SvgImageData::for_url`); it was stretched from usvg's 100x100.
        let sized = svg.sized(bg_size.width as f32, bg_size.height as f32);
        let svg = sized.as_ref().unwrap_or(svg);

        let viewport = super::svg_viewport_affine(
            svg,
            (bg_size.width * self.scale) as f32,
            (bg_size.height * self.scale) as f32,
        );

        // pyro: the image repeats as a raster one does, over the painting area (it was drawn
        // once, at its position).
        let bg_pos = (bg_pos.to_vec2() * self.scale).to_point();
        let bg_size = bg_size * self.scale;
        let BackgroundRepeat(repeat_x, repeat_y) = layer.repeat;
        let x = raster_axis_tiles(
            *repeat_x,
            origin_rect.x0,
            origin_rect.width(),
            area.x0,
            area.x1,
            bg_pos.x,
            bg_size.width,
        );
        let y = raster_axis_tiles(
            *repeat_y,
            origin_rect.y0,
            origin_rect.height(),
            area.y0,
            area.y1,
            bg_pos.y,
            bg_size.height,
        );
        if x.count as u64 * y.count as u64 > 2_000 {
            return;
        }
        // pyro: an image ends at its viewport: content past it (a stroke on the root's edges) is
        // cut off, tile by tile, when there is any (it was drawn over the neighbouring tiles and
        // the area around them).
        let size = svg.tree.size();
        let ink = svg.tree.root().abs_layer_bounding_box();
        let overflows = ink.left() < -0.01
            || ink.top() < -0.01
            || ink.right() > size.width() + 0.01
            || ink.bottom() > size.height() + 0.01;
        for hc in 0..y.count {
            for wc in 0..x.count {
                let origin = Vec2::new(
                    x.start + wc as f64 * x.stride,
                    y.start + hc as f64 * y.stride,
                );
                let transform = base_transform * kurbo::Affine::translate(origin) * viewport;
                // pyro: the root's background (an SVG document's canvas) fills the tile.
                if let Some(color) = svg.intrinsic_dimensions.root_background {
                    let tile = Rect::from_origin_size(origin.to_point(), bg_size);
                    let color = peniko::Color::from_rgba8(color.red, color.green, color.blue, color.alpha);
                    scene.fill(Fill::NonZero, base_transform, color, None, &tile);
                }
                if overflows {
                    let tile = Rect::from_origin_size(origin.to_point(), bg_size);
                    scene.push_clip_layer(Fill::NonZero, base_transform, &tile);
                    anyrender_svg::render_svg_tree(scene, &svg.tree, transform);
                    scene.pop_layer();
                } else {
                    anyrender_svg::render_svg_tree(scene, &svg.tree, transform);
                }
            }
        }
    }

    /// pyro: a background image's rectangle (`pos`, device px from `origin_rect`'s corner, and its size) as
    /// Chromium places it on a box laid out at a fraction of a pixel: from the box's unrounded
    /// place, the image's rectangle snapped to whole device pixels (`BackgroundImageGeometry`), so
    /// that two boxes rounded apart place images centred on one point alike (WPT css-backgrounds
    /// `subpixel-position-center`, `background-position-subpixel-at-border`). A box at whole
    /// pixels, or under a transform that is not a translation, keeps the position as it is.
    fn subpixel_image_rect(&self, pos: Point, size: Size, origin_rect: Rect, base: Affine) -> (Point, Size) {
        let [a, b, c, d, tx, ty] = (self.transform * base).as_coeffs();
        // A zero-sized image paints nothing: it stays zero-sized, not snapped to a pixel.
        if a != 1.0 || b != 0.0 || c != 0.0 || d != 1.0 || !(size.width > 0.0 && size.height > 0.0) {
            return (pos, size);
        }
        let (unrounded, _) = self.context.dom.physical_unrounded_geometry(self.node.id);
        let fraction = |v: f32| f64::from(v) - f64::from(v).round();
        let (fx, fy) = (fraction(unrounded.x), fraction(unrounded.y));
        if fx.abs() < 1e-6 && fy.abs() < 1e-6 {
            return (pos, size);
        }
        // Each edge at the whole device pixel nearest its unrounded place.
        let snap = |v: f64, length: f64, f: f64, origin: f64, t: f64| {
            let start = t + origin + v + f * self.scale;
            let (x0, x1) = (start.round(), (start + length).round());
            (x0 - t - origin, (x1 - x0).max(1.0))
        };
        let (x, w) = snap(pos.x, size.width, fx, origin_rect.x0, tx);
        let (y, h) = snap(pos.y, size.height, fy, origin_rect.y0, ty);
        (Point::new(x, y), Size::new(w, h))
    }

    fn draw_raster_image_layer(
        &self,
        scene: &mut impl PaintScene,
        layer: &ImageLayerStyles,
        area: Rect,
    ) {
        let Some(bg_image) = layer.image_data else {
            return;
        };
        let ImageData::Raster(image_data) = &bg_image.image else {
            return;
        };

        let image_rendering = self.style.clone_image_rendering();
        let quality = to_image_quality(image_rendering);

        let (origin_rect, base_transform, area) = self.positioning_area(layer, area);

        // pyro: `image-orientation: none`: the image as stored, not as its EXIF orientation
        // turned the decoded pixels.
        let unoriented = blitz_dom::util::image_unoriented(&self.style) && image_data.orientation != 1;
        let (image_width, image_height) = image_data.shown_size(unoriented);
        let (image_width, image_height) = (image_width as f64, image_height as f64);

        let (bg_pos, bg_size) = compute_layer_position_and_size(
            layer,
            origin_rect.width() / self.scale,
            origin_rect.height() / self.scale,
            BackgroundSizeComputeMode::Size(
                image_width as f32 / layer.resolution,
                image_height as f32 / layer.resolution,
            ),
        );

        let bg_pos = (bg_pos.to_vec2() * self.scale).to_point();
        let (bg_pos, bg_size) = self.subpixel_image_rect(bg_pos, bg_size * self.scale, origin_rect, base_transform);
        if !(bg_size.width > 0.0 && bg_size.height > 0.0) {
            return;
        }

        let BackgroundRepeat(repeat_x, repeat_y) = layer.repeat;
        // pyro: the tiles cover the whole painting area (the clip box, or the canvas), not just
        // the positioning area: a repeating image fills the border area of a `border-box` clip
        // and, for the root element, the canvas.
        let x = raster_axis_tiles(
            *repeat_x,
            origin_rect.x0,
            origin_rect.width(),
            area.x0,
            area.x1,
            bg_pos.x,
            bg_size.width,
        );
        let y = raster_axis_tiles(
            *repeat_y,
            origin_rect.y0,
            origin_rect.height(),
            area.y0,
            area.y1,
            bg_pos.y,
            bg_size.height,
        );
        if x.count == 0 || y.count == 0 {
            return;
        }

        let mut brush = to_peniko_image(image_data, quality);
        // pyro: an axis that does not repeat stops at the image's edge, as in Chromium: sampling
        // past it (a scaled image's edge pixels) wrapped the opposite edge's pixels in. `repeat`
        // and `round` tiles are one repeating fill, their seams sampled across (Chromium's image
        // shader).
        let extend = |repeat: BackgroundRepeatKeyword| match repeat {
            BackgroundRepeatKeyword::Repeat | BackgroundRepeatKeyword::Round => {
                peniko::Extend::Repeat
            }
            BackgroundRepeatKeyword::Space | BackgroundRepeatKeyword::NoRepeat => {
                peniko::Extend::Pad
            }
        };
        brush.sampler.x_extend = extend(*repeat_x);
        brush.sampler.y_extend = extend(*repeat_y);
        let scale =
            Affine::scale_non_uniform(bg_size.width / image_width, bg_size.height / image_height);
        let scale = if unoriented { scale * Affine::new(image_data.unturn()) } else { scale };
        // Tiles that touch are one fill with the image repeating in it.
        let contiguous = |t: &AxisTiles, extend: peniko::Extend| {
            t.count == 1 || (t.stride == t.len && extend == peniko::Extend::Repeat)
        };
        if contiguous(&x, brush.sampler.x_extend) && contiguous(&y, brush.sampler.y_extend) {
            let rect = Rect::new(
                x.start,
                y.start,
                x.start + x.len + (x.count - 1) as f64 * x.stride,
                y.start + y.len + (y.count - 1) as f64 * y.stride,
            );
            let brush_transform = Affine::translate((x.start, y.start)) * scale;
            scene.fill(
                peniko::Fill::NonZero,
                base_transform,
                brush.as_ref(),
                Some(brush_transform),
                &rect,
            );
            return;
        }
        if x.count as u64 * y.count as u64 > 10_000 {
            return;
        }
        // Spaced tiles are each the image on its own, clamped in both axes (Chromium draws them
        // through a picture shader of one image per cell): a row of `round` tiles with spaced
        // rows matches no-repeat copies laid side by side (WPT `background-repeat-space-6`/`-7`).
        brush.sampler.x_extend = peniko::Extend::Pad;
        brush.sampler.y_extend = peniko::Extend::Pad;
        for hc in 0..y.count {
            for wc in 0..x.count {
                let origin = Point::new(
                    x.start + wc as f64 * x.stride,
                    y.start + hc as f64 * y.stride,
                );
                let rect = Rect::from_origin_size(origin, (x.len, y.len));
                let brush_transform = Affine::translate(origin.to_vec2()) * scale;
                scene.fill(
                    peniko::Fill::NonZero,
                    base_transform,
                    brush.as_ref(),
                    Some(brush_transform),
                    &rect,
                );
            }
        }
    }

    /// The background positioning area of `layer` and the transform from its space to the scene,
    /// with `area` (this element's space) taken to that space: the viewport for a fixed layer,
    /// else the origin box in this element's space.
    /// pyro: whether `layer` draws one SVG image tile (a `url()` image, `no-repeat` on both axes,
    /// not fixed) that lies inside `clip` (this element's space, device px), its drawing too (it
    /// stays in its viewport).
    fn image_tile_within(&self, layer: &ImageLayerStyles, clip: Rect) -> bool {
        use BackgroundRepeatKeyword::NoRepeat;
        let BackgroundRepeat(repeat_x, repeat_y) = layer.repeat;
        if !matches!((repeat_x, repeat_y), (NoRepeat, NoRepeat))
            || self.layer_is_fixed(layer)
            || !matches!(layer.stylo_image, GenericImage::Url(_))
        {
            return false;
        }
        let Some(data) = layer.image_data else { return false };
        let origin_rect = self.box_rect(layer.origin);
        let (w, h) = (origin_rect.width() / self.scale, origin_rect.height() / self.scale);
        let (pos, size) = match &data.image {
            // (A raster `<img>` is drawn in its element's clip layer, `should_clip_node`: its
            // background twin keeps its layer too.)
            #[cfg(feature = "svg")]
            ImageData::Svg(svg) => {
                let size = svg.tree.size();
                let ink = svg.tree.root().abs_layer_bounding_box();
                if ink.left() < -0.01
                    || ink.top() < -0.01
                    || ink.right() > size.width() + 0.01
                    || ink.bottom() > size.height() + 0.01
                    || svg.intrinsic_dimensions.degenerate_view_box
                {
                    return false;
                }
                let width = svg.intrinsic_width().filter(|w| w.is_finite() && *w > 0.0);
                let height = svg.intrinsic_height().filter(|h| h.is_finite() && *h > 0.0);
                let ratio = match (width, height) {
                    (Some(w), Some(h)) => Some(w / h),
                    _ => svg.viewbox_aspect_ratio(),
                }
                .filter(|r| r.is_finite() && *r > 0.0);
                let mode = BackgroundSizeComputeMode::Intrinsic { width, height, ratio };
                let (pos, size) = compute_layer_position_and_size(layer, w, h, mode);
                ((pos.to_vec2() * self.scale).to_point(), size * self.scale)
            }
            #[allow(unreachable_patterns)]
            _ => return false,
        };
        let tile = Rect::from_origin_size(origin_rect.origin() + pos.to_vec2(), size);
        let e = 1e-3;
        tile.x0 >= clip.x0 - e && tile.y0 >= clip.y0 - e && tile.x1 <= clip.x1 + e && tile.y1 <= clip.y1 + e
    }

    fn positioning_area(&self, layer: &ImageLayerStyles, area: Rect) -> (Rect, Affine, Rect) {
        if self.layer_is_fixed(layer) {
            let (viewport_rect, transform) = self.fixed_positioning_area();
            let area = (transform.inverse() * self.transform).transform_rect_bbox(area);
            (viewport_rect, transform, area)
        } else {
            (self.box_rect(layer.origin), self.transform, area)
        }
    }

    fn draw_gradient_layer(
        &self,
        scene: &mut impl PaintScene,
        gradient: &StyloGradient,
        layer: &ImageLayerStyles,
        area: Rect,
        current_color: &style::color::AbsoluteColor,
    ) {
        let scale = self.scale;
        self.draw_generated_layer(scene, layer, area, |tile_rect, bounding_box| {
            to_peniko_gradient(gradient, tile_rect, bounding_box, scale, current_color)
        })
    }

    /// pyro: a layer whose image has no natural size and is made for its tile: a gradient
    /// (`make` gets the tile and the border box's bounds), or `image(<color>)` as a gradient of
    /// one colour.
    fn draw_generated_layer(
        &self,
        scene: &mut impl PaintScene,
        layer: &ImageLayerStyles,
        area: Rect,
        make: impl FnOnce(Rect, Rect) -> (peniko::Gradient, Option<Affine>),
    ) {
        // The tiles cover the painting area (`area`: the clip box, or the canvas), in the
        // positioning area's space.
        let (origin_rect, base_transform, clip_rect) = self.positioning_area(layer, area);

        let (bg_pos, bg_size) = compute_layer_position_and_size(
            layer,
            origin_rect.width() / self.scale,
            origin_rect.height() / self.scale,
            BackgroundSizeComputeMode::Auto,
        );

        let bg_pos = (bg_pos.to_vec2() * self.scale).to_point();
        let bg_size = bg_size * self.scale;

        let BackgroundRepeat(repeat_x, repeat_y) = layer.repeat;

        let x = gradient_axis_tiling(
            *repeat_x,
            origin_rect.x0,
            origin_rect.width(),
            clip_rect.x0,
            clip_rect.width(),
            bg_pos.x,
            bg_size.width,
        );
        let y = gradient_axis_tiling(
            *repeat_y,
            origin_rect.y0,
            origin_rect.height(),
            clip_rect.y0,
            clip_rect.height(),
            bg_pos.y,
            bg_size.height,
        );

        let tile_rect = Rect::new(0.0, 0.0, x.rect_len, y.rect_len);
        let bounding_box = self.frame.border_box.bounding_box();

        let (gradient, gradient_transform) = make(tile_rect, bounding_box);
        // pyro: a gradient of one colour is that colour (`linear-gradient(white, white)`): the
        // ramp came out a level short of it in places. Repeating in both axes, its tiles cover
        // the painting area: one fill, however small they are (`background-size: 0.2px`).
        let solid = uniform_gradient_color(&gradient);
        let repeats = |repeat: &BackgroundRepeatKeyword| {
            matches!(
                repeat,
                BackgroundRepeatKeyword::Repeat | BackgroundRepeatKeyword::Round
            )
        };
        if let Some(color) = solid
            && repeats(repeat_x)
            && repeats(repeat_y)
            && bg_size.width > 0.0
            && bg_size.height > 0.0
        {
            scene.fill(
                peniko::Fill::NonZero,
                base_transform,
                color,
                None,
                &clip_rect,
            );
            return;
        }

        // FIXME: https://wpt.live/css/css-backgrounds/background-size/background-size-near-zero-gradient.html
        if x.count as u64 * y.count as u64 > 500 {
            return;
        }
        let brush = match &solid {
            Some(color) => anyrender::Paint::Solid(*color),
            None => anyrender::Paint::Gradient(&gradient),
        };
        let gradient_transform = if solid.is_some() {
            None
        } else {
            gradient_transform
        };

        let transform = base_transform.then_translate(Vec2 {
            x: x.translate,
            y: y.translate,
        });

        for hc in 0..y.count {
            for wc in 0..x.count {
                let transform = transform.then_translate(Vec2 {
                    x: wc as f64 * x.stride,
                    y: hc as f64 * y.stride,
                });

                scene.fill(
                    peniko::Fill::NonZero,
                    transform,
                    brush.clone(),
                    gradient_transform,
                    &tile_rect,
                );
            }
        }
    }
}

fn compute_layer_position_and_size(
    layer: &ImageLayerStyles,
    container_w: f64,
    container_h: f64,
    size_mode: BackgroundSizeComputeMode,
) -> (Point, Size) {
    use BackgroundRepeatKeyword::*;

    let bg_size = compute_layer_size(layer, container_w as f32, container_h as f32, size_mode);

    let BackgroundRepeat(repeat_x, repeat_y) = layer.repeat;
    let bg_size = round_layer_size(
        bg_size,
        container_w,
        container_h,
        matches!(repeat_x, Round),
        matches!(repeat_y, Round),
        size_is_auto(layer.size),
    );

    // pyro: the image is positioned at its final size, after `round` scaled it (CSS Backgrounds
    // 3 §3.9: sizing, the round step, then positioning; it was positioned at its unrounded size).
    let bg_pos = compute_layer_position(
        layer,
        (container_w - bg_size.width) as f32,
        (container_h - bg_size.height) as f32,
    );

    (bg_pos, bg_size)
}

/// Which axes of a `background-size` are `auto` (x, y).
fn size_is_auto(size: &BackgroundSize) -> (bool, bool) {
    use style::values::generics::length::GenericLengthPercentageOrAuto as Lpa;
    match size {
        BackgroundSize::ExplicitSize { width, height } => {
            (matches!(width, Lpa::Auto), matches!(height, Lpa::Auto))
        }
        BackgroundSize::Cover | BackgroundSize::Contain => (false, false),
    }
}

/// pyro: the round step of CSS Backgrounds 3 §3.9 on a layer sized `size` in a positioning area
/// `container_w` x `container_h`: a `round` axis is scaled to fit a whole number of copies (at
/// least one), and when only one axis rounds and `background-size` is `auto` in the other (`auto`
/// in `auto_axes`), the other is scaled with it, so the image keeps the aspect ratio it had.
/// Upstream left the other axis alone, and divided by zero copies for an image more than twice
/// the area's size.
fn round_layer_size(
    size: Size,
    container_w: f64,
    container_h: f64,
    round_x: bool,
    round_y: bool,
    auto_axes: (bool, bool),
) -> Size {
    let round = |container: f64, len: f64| {
        if len > 0.0 && container > 0.0 {
            container / (container / len).round().max(1.0)
        } else {
            len
        }
    };
    match (round_x, round_y) {
        (true, true) => Size::new(
            round(container_w, size.width),
            round(container_h, size.height),
        ),
        (true, false) => {
            let width = round(container_w, size.width);
            let height = if auto_axes.1 && size.width > 0.0 {
                size.height * width / size.width
            } else {
                size.height
            };
            Size::new(width, height)
        }
        (false, true) => {
            let height = round(container_h, size.height);
            let width = if auto_axes.0 && size.height > 0.0 {
                size.width * height / size.height
            } else {
                size.width
            };
            Size::new(width, height)
        }
        (false, false) => size,
    }
}

#[inline]
fn compute_layer_position(layer: &ImageLayerStyles, width: f32, height: f32) -> Point {
    use style::values::computed::Length;

    let bg_pos_x = layer.position_x.resolve(Length::new(width)).px() as f64;
    let bg_pos_y = layer.position_y.resolve(Length::new(height)).px() as f64;

    Point::new(bg_pos_x, bg_pos_y)
}

fn compute_layer_size(
    layer: &ImageLayerStyles,
    container_w: f32,
    container_h: f32,
    mode: BackgroundSizeComputeMode,
) -> kurbo::Size {
    use style::values::computed::Length;
    use style::values::generics::length::GenericLengthPercentageOrAuto as Lpa;

    let (width, height): (f32, f32) = match layer.size {
        BackgroundSize::ExplicitSize { width, height } => {
            let width = width.map(|w| w.0.resolve(Length::new(container_w)));
            let height = height.map(|h| h.0.resolve(Length::new(container_h)));

            match (width, height) {
                (Lpa::LengthPercentage(width), Lpa::LengthPercentage(height)) => {
                    let width = width.px();
                    let height = height.px();
                    match mode {
                        BackgroundSizeComputeMode::Auto => (width, height),
                        BackgroundSizeComputeMode::Size(_, _) => (width, height),
                        BackgroundSizeComputeMode::Intrinsic { .. } => (width, height),
                    }
                }
                (Lpa::LengthPercentage(width), Lpa::Auto) => {
                    let width = width.px();
                    let height = match mode {
                        BackgroundSizeComputeMode::Auto => container_h,
                        BackgroundSizeComputeMode::Size(bg_w, bg_h) => bg_h / bg_w * width,
                        BackgroundSizeComputeMode::Intrinsic { height, ratio, .. } => ratio
                            .map(|ratio| width / ratio)
                            .or(height)
                            .unwrap_or(container_h),
                    };
                    (width, height)
                }
                (Lpa::Auto, Lpa::LengthPercentage(height)) => {
                    let height = height.px();
                    let width = match mode {
                        BackgroundSizeComputeMode::Auto => container_w,
                        BackgroundSizeComputeMode::Size(bg_w, bg_h) => bg_w / bg_h * height,
                        BackgroundSizeComputeMode::Intrinsic { width, ratio, .. } => ratio
                            .map(|ratio| height * ratio)
                            .or(width)
                            .unwrap_or(container_w),
                    };
                    (width, height)
                }
                (Lpa::Auto, Lpa::Auto) => match mode {
                    BackgroundSizeComputeMode::Auto => (container_w, container_h),
                    BackgroundSizeComputeMode::Size(bg_w, bg_h) => (bg_w, bg_h),
                    BackgroundSizeComputeMode::Intrinsic {
                        width,
                        height,
                        ratio,
                    } => default_sizing(width, height, ratio, container_w, container_h),
                },
            }
        }
        BackgroundSize::Cover => match mode {
            BackgroundSizeComputeMode::Auto => (container_w, container_h),
            BackgroundSizeComputeMode::Size(bg_w, bg_h) => {
                // Scale to the smallest size that covers both axes
                let ratio = (container_w / bg_w).max(container_h / bg_h);
                (bg_w * ratio, bg_h * ratio)
            }
            BackgroundSizeComputeMode::Intrinsic { ratio, .. } => match ratio {
                // Scale the aspect ratio to the smallest size that covers both axes
                Some(ratio) => {
                    let scale = (container_w / ratio).max(container_h);
                    (scale * ratio, scale)
                }
                // No intrinsic aspect ratio: fill the positioning area
                None => (container_w, container_h),
            },
        },
        BackgroundSize::Contain => match mode {
            BackgroundSizeComputeMode::Auto => (container_w, container_h),
            BackgroundSizeComputeMode::Size(bg_w, bg_h) => {
                // Scale to the largest size contained by both axes
                let ratio = (container_w / bg_w).min(container_h / bg_h);
                (bg_w * ratio, bg_h * ratio)
            }
            BackgroundSizeComputeMode::Intrinsic { ratio, .. } => match ratio {
                // Scale the aspect ratio to the largest size contained by both axes
                Some(ratio) => {
                    let scale = (container_w / ratio).min(container_h);
                    (scale * ratio, scale)
                }
                // No intrinsic aspect ratio: fill the positioning area
                None => (container_w, container_h),
            },
        },
    };

    kurbo::Size {
        width: width as f64,
        height: height as f64,
    }
}

enum BackgroundSizeComputeMode {
    Auto,
    Size(f32, f32),
    /// Intrinsic dimensions of an image which may lack an intrinsic width,
    /// height, and/or aspect ratio (e.g. SVG), sized per the CSS default
    /// sizing algorithm (https://drafts.csswg.org/css-images/#default-sizing)
    Intrinsic {
        width: Option<f32>,
        height: Option<f32>,
        ratio: Option<f32>,
    },
}

/// The CSS default sizing algorithm for the unconstrained (`auto auto`) case:
/// resolve the concrete object size from whichever intrinsic dimensions exist,
/// falling back to the default object size (the background positioning area).
fn default_sizing(
    width: Option<f32>,
    height: Option<f32>,
    ratio: Option<f32>,
    container_w: f32,
    container_h: f32,
) -> (f32, f32) {
    match (width, height) {
        (Some(w), Some(h)) => (w, h),
        (Some(w), None) => (w, ratio.map(|r| w / r).unwrap_or(container_h)),
        (None, Some(h)) => (ratio.map(|r| h * r).unwrap_or(container_w), h),
        (None, None) => match ratio {
            // Intrinsic aspect ratio only: size as if `contain` were specified
            Some(ratio) => {
                let scale = (container_w / ratio).min(container_h);
                (scale * ratio, scale)
            }
            None => (container_w, container_h),
        },
    }
}

/// The placement and tiling of a background layer along one axis: a
/// translation applied to the layer as a whole, the length of each filled
/// rect, and the number of explicit tiles with the stride between them.
struct AxisTiling {
    /// Translation (in device pixels) positioning the first tile
    translate: f64,
    /// Length of each filled rect, in the coordinate space of the fill's transform
    rect_len: f64,
    /// Number of explicitly drawn tiles
    count: u32,
    /// Stride (in device pixels) between the starts of consecutive tiles
    stride: f64,
}

/// The tiles of a raster image layer along one axis: where the first starts, their length and
/// the distance between their starts, in device px.
struct AxisTiles {
    start: f64,
    len: f64,
    stride: f64,
    count: u32,
}

/// pyro: the tiles of a raster image layer along one axis that cover the painting area
/// `area_start..area_end`, for a positioning area at `origin_start` (`origin_len` long), the image
/// at `bg_pos` in it and `tile_len` long. `repeat` and `round` (already sized to fit) tile the
/// whole painting area; `space` spaces the copies that fit in the positioning area so that the
/// first and last touch its edges and repeats that pattern over the painting area, or places one
/// copy at `bg_pos` when two do not fit; `no-repeat` places one copy.
fn raster_axis_tiles(
    repeat: BackgroundRepeatKeyword,
    origin_start: f64,
    origin_len: f64,
    area_start: f64,
    area_end: f64,
    bg_pos: f64,
    tile_len: f64,
) -> AxisTiles {
    use BackgroundRepeatKeyword::*;

    let cover = |anchor: f64, stride: f64| {
        let first = anchor + ((area_start - anchor) / stride).floor() * stride;
        let count = ((area_end - first) / stride)
            .ceil()
            .clamp(0.0, u32::MAX as f64) as u32;
        AxisTiles {
            start: first,
            len: tile_len,
            stride,
            count,
        }
    };
    match repeat {
        Repeat | Round => cover(origin_start + bg_pos, tile_len),
        Space => {
            let (count, stride) = compute_space_count_and_stride(origin_len, tile_len);
            if count > 1 {
                cover(origin_start, stride)
            } else {
                AxisTiles {
                    start: origin_start + bg_pos,
                    len: tile_len,
                    stride: 0.0,
                    count: 1,
                }
            }
        }
        NoRepeat => AxisTiles {
            start: origin_start + bg_pos,
            len: tile_len,
            stride: 0.0,
            count: 1,
        },
    }
}

/// Per-axis placement and tiling for a gradient layer. Unlike raster images,
/// gradients cannot rely on brush repetition, so `Repeat`/`Round` also produce
/// explicit tiles. When the clip box extends beyond the origin box, tiling
/// starts from the clip box edge so the pattern covers the whole clipped area.
fn gradient_axis_tiling(
    repeat: BackgroundRepeatKeyword,
    origin_start: f64,
    origin_len: f64,
    clip_start: f64,
    clip_len: f64,
    bg_pos: f64,
    tile_len: f64,
) -> AxisTiling {
    use BackgroundRepeatKeyword::*;

    match repeat {
        Repeat | Round => {
            // The clip and origin boxes are nested, so the clip box extends
            // beyond the origin box iff it does so at either end
            let clip_is_outer =
                clip_start < origin_start || clip_start + clip_len > origin_start + origin_len;
            let (area_start, area_len) = if clip_is_outer {
                (clip_start, clip_len)
            } else {
                (origin_start, origin_len)
            };
            let extend_len = extend((origin_start - area_start) + bg_pos, tile_len);
            let count = ((area_len + extend_len) / tile_len).ceil() as u32;
            AxisTiling {
                translate: area_start - extend_len,
                rect_len: tile_len,
                count,
                stride: tile_len,
            }
        }
        Space => {
            let (count, stride) = compute_space_count_and_stride(origin_len, tile_len);
            if count > 1 {
                // pyro: the spaced pattern repeats over the painting area (a `border-box` clip
                // shows it in the border too), as a raster image's does.
                let first = origin_start + ((clip_start - origin_start) / stride).floor() * stride;
                let count = ((clip_start + clip_len - first) / stride).ceil().max(0.0) as u32;
                AxisTiling {
                    translate: first,
                    rect_len: tile_len,
                    count,
                    stride,
                }
            } else {
                AxisTiling {
                    translate: origin_start + bg_pos,
                    rect_len: tile_len,
                    count,
                    stride,
                }
            }
        }
        NoRepeat => AxisTiling {
            translate: origin_start + bg_pos,
            rect_len: tile_len,
            count: 1,
            stride: 0.0,
        },
    }
}

fn compute_space_count_and_stride(bg_size: f64, size: f64) -> (u32, f64) {
    let modulo = bg_size % size;
    let count = (((bg_size - modulo) / size) as u32).max(1);
    let stride = if count > 1 {
        modulo / (count - 1) as f64
    } else {
        0.0
    } + size;

    (count, stride)
}

/// pyro: the colour of a gradient whose stops all have one colour, which paints as that colour.
pub(super) fn uniform_gradient_color(gradient: &peniko::Gradient) -> Option<Color> {
    let first = gradient.stops.first()?;
    gradient
        .stops
        .iter()
        .all(|stop| stop.color == first.color)
        .then(|| first.color.to_alpha_color::<peniko::color::Srgb>())
}

/// Whether drawing the layer is guaranteed to paint nothing, making its clip
/// layer unnecessary
fn layer_paints_nothing(layer: &ImageLayerStyles) -> bool {
    match layer.stylo_image {
        GenericImage::None => true,
        GenericImage::Url(_) => layer.image_data.is_none(),
        _ => false,
    }
}

#[inline]
pub(super) fn get_cyclic<T>(values: &[T], layer_index: usize) -> &T {
    &values[layer_index % values.len()]
}

fn extend(offset: f64, length: f64) -> f64 {
    let extend_length = offset % length;
    if extend_length > 0.0 {
        length - extend_length
    } else {
        -extend_length
    }
}

/// pyro: a table part whose background is painted in its cells.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PartKind {
    ColumnGroup,
    Column,
    RowGroup,
    Row,
}

/// pyro: a table's column and row tracks (CSS px, the table's space): the boxes of its parts.
struct TablePartRects {
    columns: Vec<(f64, f64)>,
    rows: Vec<(f64, f64)>,
    /// A vertical table's: whether its block axis runs right to left, and its border box width.
    vertical: Option<(bool, f64)>,
}

impl TablePartRects {
    /// The box of a part of `table`: a row or row group spans every column, a column or column
    /// group every row, each over its own tracks.
    fn rect(
        &self,
        table: &blitz_dom::TableContext,
        node_id: NodeId,
        kind: PartKind,
        cell: &blitz_dom::TableCell,
    ) -> Rect {
        // From the start of the first track at `indices` to the end of the last.
        // (Tracks in logical order: the columns of an `rtl` table run right to left.)
        fn span(tracks: &[(f64, f64)], mut indices: impl Iterator<Item = usize>) -> Option<(f64, f64)> {
            let first = indices.next()?;
            let last = indices.last().unwrap_or(first);
            let (a, b) = (tracks.get(first)?, tracks.get(last.min(tracks.len() - 1))?);
            Some((a.0.min(b.0), a.1.max(b.1)))
        }
        let all_columns = span(&self.columns, [0, self.columns.len() - 1].into_iter()).unwrap();
        let all_rows = (self.rows[0].0, self.rows[self.rows.len() - 1].1);
        let (xs, ys) = match kind {
            PartKind::Row => (all_columns, span(&self.rows, std::iter::once(cell.row as usize)).unwrap_or(all_rows)),
            PartKind::RowGroup => {
                let rows = table.rows.iter().enumerate().filter(|(_, row)| row.group == Some(node_id));
                (all_columns, span(&self.rows, rows.map(|(at, _)| at)).unwrap_or(all_rows))
            }
            PartKind::Column => {
                let columns = table.columns.iter().enumerate().filter(|(_, column)| column.node_id == node_id);
                (span(&self.columns, columns.map(|(at, _)| at)).unwrap_or(all_columns), all_rows)
            }
            PartKind::ColumnGroup => {
                let columns = table.columns.iter().enumerate().filter(|(_, column)| column.group == Some(node_id));
                (span(&self.columns, columns.map(|(at, _)| at)).unwrap_or(all_columns), all_rows)
            }
        };
        match self.vertical {
            None => Rect::new(xs.0, ys.0, xs.1, ys.1),
            Some((false, _)) => Rect::new(ys.0, xs.0, ys.1, xs.1),
            Some((true, width)) => Rect::new(width - ys.1, xs.0, width - ys.0, xs.1),
        }
    }
}
