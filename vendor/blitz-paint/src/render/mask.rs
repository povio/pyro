//! Rendering for the CSS `mask` properties (`mask-image`, `mask-position`,
//! `mask-size`, `mask-repeat`, `mask-clip`, `mask-origin`, `mask-mode`,
//! `mask-composite`).
//!
//! Masks are applied by:
//!
//!  1. Pushing an isolation layer (clipped to the mask painting area) before the
//!     element is painted
//!  2. Painting the element (and its descendants) as normal
//!  3. Pushing a `Compose::DestIn` layer and drawing the mask image layers into it,
//!     which multiplies the alpha of the already-painted element by the alpha of
//!     the mask
//!  4. Popping both layers
//!
//! The mask image layers themselves are positioned/sized/repeated using the same
//! code as `background-image` layers (see `background.rs`), as the `mask-*` and
//! `background-*` properties share computed value types.
use crate::render::background::get_cyclic;

use super::background::{BoxModelBox, ImageLayerStyles};
use super::{ALMOST_OPAQUE, ElementCx};
use anyrender::recording::RenderCommand;
use anyrender::{Paint, PaintScene, Scene};
use kurbo::{Affine, BezPath};
#[cfg(feature = "svg")]
use kurbo::Rect;
use peniko::{BlendMode, Color, Compose, Fill, ImageBrush, ImageData, Mix};
use std::cell::RefCell;
use style::properties::generated::longhands::mask_composite::single_value::computed_value::T as StyloMaskComposite;
use style::properties::generated::longhands::mask_mode::single_value::computed_value::T as StyloMaskMode;
use style::values::generics::image::GenericImage;

impl ElementCx<'_, '_> {
    /// Whether the element has any CSS `mask-image` layers
    pub(super) fn has_css_mask(&self) -> bool {
        self.style
            .get_svg()
            .mask_image
            .0
            .iter()
            .any(|image| !matches!(image, GenericImage::None))
    }

    /// pyro: the mask painting area (CSS Masking's "mask painting area"), in device px from the
    /// border box's origin: the largest of the layers' `mask-clip` boxes (the boxes nest), rounded
    /// by the element's radii; everything visible for `mask-clip: no-clip`. Outside it the mask
    /// is transparent: nothing of the element is left.
    fn mask_painting_area(&self) -> BezPath {
        let svg_styles = self.style.get_svg();
        let rank = |clip: BoxModelBox| match clip {
            BoxModelBox::NoClip => 3,
            BoxModelBox::BorderBox => 2,
            BoxModelBox::PaddingBox => 1,
            BoxModelBox::ContentBox => 0,
        };
        let largest = (0..svg_styles.mask_image.0.len())
            .map(|idx| self.mask_layer_clip(idx))
            .max_by_key(|&clip| rank(clip))
            .unwrap_or(BoxModelBox::BorderBox);
        self.box_path(largest)
    }

    /// pyro: the box a mask layer is cut to: its `mask-clip`, but a layer that references an
    /// SVG `<mask>` is not cut (`mask-clip` positions and cuts mask *images*; a `<mask>` applies
    /// in its own region, which may reach past the border box: over the element's box shadow,
    /// WPT `mask-image-3f`).
    fn mask_layer_clip(&self, idx: usize) -> BoxModelBox {
        let svg_styles = self.style.get_svg();
        #[cfg(feature = "svg")]
        if let Some(GenericImage::Url(url)) = svg_styles.mask_image.0.get(idx) {
            if self.names_svg_mask(url, idx) {
                return BoxModelBox::NoClip;
            }
        }
        BoxModelBox::from(*get_cyclic(&svg_styles.mask_clip.0, idx))
    }

    /// If the element has a CSS mask, push an isolation layer for the masked content
    /// to be drawn into. Returns whether a layer was pushed, which should later be
    /// passed to [`maybe_pop_css_mask_layer`](Self::maybe_pop_css_mask_layer).
    pub(super) fn maybe_push_css_mask_layer(&self, scene: &mut impl PaintScene) -> bool {
        if !self.has_css_mask() && !self.has_mask_border() {
            return false;
        }

        // Content outside of the mask painting area has a mask alpha of 0, so the
        // isolation layer is clipped to it.
        scene.push_layer(
            Fill::NonZero,
            Mix::Normal,
            ALMOST_OPAQUE,
            self.transform,
            &self.mask_area(),
            None,
            None,
        );
        true
    }

    /// pyro: the area masks leave anything in: the mask painting area of the `mask-image`
    /// layers, else the `mask-border` image area.
    fn mask_area(&self) -> BezPath {
        if self.has_css_mask() {
            self.mask_painting_area()
        } else {
            kurbo::Shape::to_path(&self.mask_border_area(), 0.1)
        }
    }

    /// Apply the CSS mask to the content drawn since the corresponding
    /// [`maybe_push_css_mask_layer`](Self::maybe_push_css_mask_layer) by drawing the
    /// mask image layers with `Compose::DestIn`, then pop the isolation layer.
    pub(super) fn maybe_pop_css_mask_layer(&self, scene: &mut impl PaintScene, layer_pushed: bool) {
        if !layer_pushed {
            return;
        }

        // The `DestIn` layer covers the whole isolation layer: where a destructive layer
        // draws nothing, it leaves what is under it.
        let area = self.mask_area();
        if self.has_css_mask() {
            scene.push_layer(
                Fill::NonZero,
                BlendMode::new(Mix::Normal, Compose::DestIn),
                1.0,
                self.transform,
                &area,
                None,
                None,
            );
            self.draw_css_mask(scene, &area);
            scene.pop_layer(); // Mask (DestIn) layer
        }
        // pyro: `mask-border` (css-masking-1 §8): its nine-piece image is a mask of its own, by
        // its alpha or (`mask-border-mode: luminance`) its luminance; an image not loaded masks
        // nothing.
        if self.has_mask_border() {
            let mut recording = Scene::new();
            if self.draw_mask_border(&mut recording) {
                use style::values::specified::svg::MaskType;
                if self.style.get_svg().mask_border_mode == MaskType::Luminance {
                    luminance_to_alpha(&mut recording);
                }
                scene.push_layer(
                    Fill::NonZero,
                    BlendMode::new(Mix::Normal, Compose::DestIn),
                    1.0,
                    self.transform,
                    &area,
                    None,
                    None,
                );
                scene.append_scene(recording, Affine::IDENTITY);
                scene.pop_layer();
            }
        }
        scene.pop_layer(); // Isolation layer
    }

    /// Draw the mask image layers (analogous to `draw_background` for `background-image`)
    fn draw_css_mask(&self, scene: &mut impl PaintScene, area: &BezPath) {
        let svg_styles = self.style.get_svg();
        let image_data = &self.element.mask_images;
        let layer_count = svg_styles.mask_image.0.len();
        // pyro: while one of its images is loading the mask lets nothing through: the element
        // stays hidden, all of its layers waiting (as Chromium shows it; WPT css-masking
        // `mask-image-multi-layer-while-loading`).
        if image_data.iter().flatten().any(|image| image.status == blitz_dom::node::Status::Loading) {
            return;
        }

        for idx in (0..layer_count).rev() {
            let layer = ImageLayerStyles::from_svg(svg_styles, image_data, idx);

            // Each mask layer is composited with the (already drawn) layers below it
            // using the Porter-Duff operator given by its `mask-composite` value. The
            // bottommost layer has no layers below it and is always drawn with SrcOver.
            let compose = if idx == layer_count - 1 {
                Compose::SrcOver
            } else {
                let composite_list = &svg_styles.mask_composite.0;
                match get_cyclic(composite_list, idx) {
                    StyloMaskComposite::Add => Compose::SrcOver,
                    StyloMaskComposite::Subtract => Compose::SrcOut,
                    StyloMaskComposite::Intersect => Compose::SrcIn,
                    StyloMaskComposite::Exclude => Compose::Xor,
                }
            };

            // The layer is pushed unconditionally (even for `mask-image: none` layers,
            // which draw nothing) as compositing a transparent black layer with e.g.
            // `intersect` clears the mask built up so far. It covers the whole painting
            // area, and the layer's image is cut to its `mask-clip` box inside it: outside
            // that box the layer is transparent black, which its operator composites too.
            scene.push_layer(
                Fill::NonZero,
                BlendMode::new(Mix::Normal, compose),
                1.0,
                self.transform,
                area,
                None,
                None,
            );
            scene.push_clip_layer(Fill::NonZero, self.transform, &self.box_path(self.mask_layer_clip(idx)));
            let mode = *get_cyclic(&svg_styles.mask_mode.0, idx);
            // pyro: a reference to an SVG `<mask>` of the page.
            #[cfg(feature = "svg")]
            let drawn = match layer.stylo_image {
                GenericImage::Url(url) => self.draw_svg_mask(scene, url, mode, idx),
                _ => false,
            };
            #[cfg(not(feature = "svg"))]
            let drawn = false;
            // pyro: `mask-mode: luminance` uses the image's luminance as its alpha
            // (`match-source` is alpha for an image).
            if drawn {
                // The `<mask>` is the layer.
            } else if mode == StyloMaskMode::Luminance {
                let mut recording = Scene::new();
                self.draw_image_layer(&mut recording, &layer);
                luminance_to_alpha(&mut recording);
                scene.append_scene(recording, Affine::IDENTITY);
            } else {
                self.draw_image_layer(scene, &layer);
            }
            scene.pop_layer();
            scene.pop_layer();
        }
    }
}

#[cfg(feature = "svg")]
impl ElementCx<'_, '_> {
    /// pyro: `mask-image: url(#id)` naming a `<mask>` of this document: the mask's rendering is
    /// the layer's image (`mask-size`, `mask-position` and `mask-repeat` do not apply), its
    /// reference box the border box (`objectBoundingBox` fractions of it), user space at the
    /// border box's origin with the element's zoom. `mask-mode: match-source` is the mask's
    /// `mask-type`; `alpha` and `luminance` are used instead of it. Returns whether the URL named
    /// a `<mask>` (and the layer was drawn); a mask in a reference cycle masks everything.
    /// Whether a mask layer's URL names an SVG `<mask>`, of this document or of the document
    /// loaded as the layer's image.
    fn names_svg_mask(&self, url: &style::values::computed::url::ComputedUrl, layer: usize) -> bool {
        use super::clip_path::svg_reference::*;
        let dom = self.context.dom;
        match local_reference(dom, url) {
            Some(target) => dom.get_node(target).is_some_and(|node| is_svg_element(node, "mask")),
            None => self
                .element
                .mask_images
                .get(layer..layer + 1)
                .and_then(|image| external_reference(image, url))
                .is_some_and(|(source, id)| {
                    blitz_dom::node::svg_source_element(&source, &id, "id").is_some_and(|(name, _)| name == "mask")
                }),
        }
    }

    fn draw_svg_mask(
        &self,
        scene: &mut impl PaintScene,
        url: &style::values::computed::url::ComputedUrl,
        mode: StyloMaskMode,
        layer: usize,
    ) -> bool {
        use super::clip_path::svg_reference::*;
        let dom = self.context.dom;
        // pyro: a `<mask>` of this document, or of the document loaded as the layer's image
        // (`mask-image: url(masks.svg#mask)`: the image is the mask, not the document's drawing).
        let mut external: Option<std::sync::Arc<[u8]>> = None;
        let (target, id, user_space) = match local_reference(dom, url) {
            Some(target) => {
                // (One in contents `content-visibility: hidden` skips is no mask: not rendered, CSS
                // Contain 2 § 4.1; the layer is then transparent black.)
                let Some(node) = dom
                    .get_node(target)
                    .filter(|node| is_svg_element(node, "mask") && !dom.skipped_by_content_visibility(target))
                else {
                    return false;
                };
                if reference_cycle(dom, target) {
                    return true;
                }
                let Some(id) = node.attr(blitz_dom::local_name!("id")) else {
                    return true;
                };
                let user_space = node.attr(blitz_dom::local_name!("maskUnits")) == Some("userSpaceOnUse")
                    && node.attr(blitz_dom::local_name!("maskContentUnits")) != Some("objectBoundingBox");
                (Some(target), id.to_string(), user_space)
            }
            None => {
                let images = &self.element.mask_images;
                let Some((source, id)) = images.get(layer..layer + 1).and_then(|image| external_reference(image, url))
                else {
                    return false;
                };
                if !blitz_dom::node::svg_source_element(&source, &id, "id").is_some_and(|(name, _)| name == "mask") {
                    return false;
                }
                let attr = |name: &str| blitz_dom::node::svg_source_element(&source, &id, name).and_then(|(_, value)| value);
                let user_space = attr("maskUnits").as_deref() == Some("userSpaceOnUse")
                    && attr("maskContentUnits").as_deref() != Some("objectBoundingBox");
                external = Some(source);
                (None, id, user_space)
            }
        };
        let id = id.as_str();
        let zoom = self.style.effective_zoom.value() as f64;
        let to_user = 1.0 / (self.scale * zoom);
        // The shape the mask is taken from: the border box, whose box `objectBoundingBox` units
        // are fractions of; a mask in user units throughout may reach anything visible (the
        // element's box shadow, WPT `mask-image-3f`).
        let stand_in = if user_space { self.visible_area() } else { self.frame.border_box };
        let shape = Rect::new(
            stand_in.x0 * to_user,
            stand_in.y0 * to_user,
            stand_in.x1 * to_user,
            stand_in.y1 * to_user,
        );
        // `alpha` or `luminance` over the mask's `mask-type` (an id selector is all usvg needs).
        let simple_id = id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        let mask_type = match mode {
            StyloMaskMode::Alpha if simple_id => format!("<style>#{id}{{mask-type:alpha!important}}</style>"),
            StyloMaskMode::Luminance if simple_id => {
                format!("<style>#{id}{{mask-type:luminance!important}}</style>")
            }
            _ => String::new(),
        };
        let content = mask_type + &opaque_rect(shape, &format!("mask=\"url(#{})\"", escape_attribute(id)));
        let painter = self.context;
        let (width, height) = ((painter.width as f64 * to_user) as f32, (painter.height as f64 * to_user) as f32);
        let svg = match target {
            Some(target) => dom.svg_reference_document(target, &content, width, height),
            None => blitz_dom::node::SvgImageData::reference_document_from_source(external.as_deref().unwrap_or_default(), &content, width, height),
        };
        let Some(svg) = svg else {
            return true;
        };
        let transform = self.transform * Affine::scale(self.scale * zoom);
        if user_space {
            // Its content may lie above or left of the border box's corner, outside the tree's
            // viewport: what is visible bounds its raster.
            let visible = self.transform.transform_rect_bbox(self.visible_area());
            anyrender_svg::render_svg_tree_within(scene, &svg.tree, transform, visible);
        } else {
            anyrender_svg::render_svg_tree(scene, &svg.tree, transform);
        }
        true
    }
}

/// The luminance of an sRGB colour (unpremultiplied components), with `feColorMatrix`'s
/// `luminanceToAlpha` coefficients.
fn luminance([r, g, b]: [f32; 3]) -> f32 {
    0.2125 * r + 0.7154 * g + 0.0721 * b
}

/// A colour's luminance mask value as its alpha, over black: `alpha × luminance`.
fn luminance_color(color: Color) -> Color {
    let [r, g, b, a] = color.components;
    Color::new([0.0, 0.0, 0.0, (a * luminance([r, g, b])).clamp(0.0, 1.0)])
}

/// pyro: turns what `scene` paints into its luminance mask, as alpha over black: each solid
/// colour, gradient stop and image pixel becomes black with the alpha `alpha × luminance`. Exact
/// for content whose paints do not overlap translucently (an image, a gradient, a vector image's
/// shapes side by side); the luminance of overlapping translucent paints is approximated.
fn luminance_to_alpha(scene: &mut Scene) {
    for command in &mut scene.commands {
        match command {
            RenderCommand::Fill(fill) => luminance_paint(&mut fill.brush),
            RenderCommand::Stroke(stroke) => luminance_paint(&mut stroke.brush),
            RenderCommand::GlyphRun(run) => luminance_paint(&mut run.brush),
            RenderCommand::BoxShadow(shadow) => shadow.brush = luminance_color(shadow.brush),
            RenderCommand::PushLayer(_) | RenderCommand::PushClipLayer(_) | RenderCommand::PopLayer => {}
        }
    }
}

fn luminance_paint(paint: &mut Paint) {
    match paint {
        Paint::Solid(color) => *color = luminance_color(*color),
        Paint::Gradient(gradient) => {
            for stop in gradient.stops.iter_mut() {
                let color = stop.color.to_alpha_color::<color::Srgb>();
                stop.color = color::DynamicColor::from_alpha_color(luminance_color(color));
            }
        }
        Paint::Image(image) => *image = luminance_image(image),
        // Paints the backend owns are left as they are.
        Paint::Resource(_) | Paint::Custom(_) => {}
    }
}

thread_local! {
    /// The luminance masks of the last few mask images (keyed by their data's id), so an image
    /// is converted once, and the renderer sees the same image (and caches it) frame to frame.
    static LUMINANCE_IMAGES: RefCell<Vec<(u64, ImageData)>> = const { RefCell::new(Vec::new()) };
}

/// An image's luminance mask (see [`luminance_to_alpha`]), black with alpha.
fn luminance_image(image: &ImageBrush) -> ImageBrush {
    let source = &image.image;
    let id = source.data.id();
    let cached = LUMINANCE_IMAGES.with_borrow(|cache| {
        cache
            .iter()
            .find(|(key, _)| *key == id)
            .map(|(_, data)| data.clone())
    });
    let data = cached.unwrap_or_else(|| {
        let premultiplied = source.alpha_type == peniko::ImageAlphaType::AlphaPremultiplied;
        let bgra = source.format == peniko::ImageFormat::Bgra8;
        let mut out = Vec::with_capacity(source.data.data().len());
        for pixel in source.data.data().chunks_exact(4) {
            let (r, g, b) = if bgra {
                (pixel[2], pixel[1], pixel[0])
            } else {
                (pixel[0], pixel[1], pixel[2])
            };
            let a = pixel[3] as f32 / 255.0;
            let l = luminance([r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0]);
            // Premultiplied components are already scaled by alpha.
            let alpha = if premultiplied { l } else { l * a };
            out.extend_from_slice(&[0, 0, 0, (alpha * 255.0).round().clamp(0.0, 255.0) as u8]);
        }
        let data = ImageData {
            data: peniko::Blob::new(std::sync::Arc::new(out)),
            format: peniko::ImageFormat::Rgba8,
            alpha_type: peniko::ImageAlphaType::Alpha,
            width: source.width,
            height: source.height,
        };
        LUMINANCE_IMAGES.with_borrow_mut(|cache| {
            if cache.len() >= 8 {
                cache.remove(0);
            }
            cache.push((id, data.clone()));
        });
        data
    });
    ImageBrush {
        image: data,
        sampler: image.sampler,
    }
}
