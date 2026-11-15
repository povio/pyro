use std::sync::Arc;

use anyrender::recording::{LayerCommand, RenderCommand};
use anyrender::{Filter, NormalizedCoord, Paint, PaintRef, PaintScene, RenderContext};
use glifo::FontEmbolden;
use kurbo::{Affine, BezPath, Diagonal2, PathEl, Rect, Shape, Stroke};
use peniko::{BlendMode, Color, Fill, FontData, ImageBrush, ImageQuality, ImageSampler, StyleRef};
use vello_common::paint::ImageSource;
use vello_cpu::{PaintType, Pixmap};

use crate::filters::{FilterPlan, PixelRect, map_color};
use crate::image_cache::{ImageCache, ImageCacheConfig};

const DEFAULT_TOLERANCE: f64 = 0.1;

pub struct VelloCpuScenePainter {
    pub(crate) render_ctx: vello_cpu::RenderContext,
    pub(crate) resources: vello_cpu::Resources,
    pub(crate) image_cache: ImageCache,
    /// pyro: single-threaded contexts that filter layers are rendered in (one per level of
    /// nesting), kept from frame to frame. See [`crate::filters`].
    pub(crate) filter_contexts: Vec<vello_cpu::RenderContext>,
    /// pyro: what each push not popped yet was (see [`Self::replay`]).
    pub(crate) clip_paths: Vec<Pushed>,
    /// pyro: whether the scene being replayed has a layer with a blend mode.
    pub(crate) blending: bool,
    /// pyro: whether the scene being replayed has a backdrop filter (see [`crate::backdrop`]).
    pub(crate) backdrop_filters: bool,
    /// pyro: the backdrops captured in the scene being replayed.
    pub(crate) backdrops: Vec<crate::backdrop::Captured>,
    /// pyro: from the frame's device pixels to those of the context drawn into (a filter layer's
    /// or a backdrop's, offscreen).
    pub(crate) frame_to_ctx: Affine,
    /// pyro: glyphs are drawn aliased, a pixel painted when the glyph covers at least this
    /// much of it (out of 255), as Chromium draws text when WPT runs it without font
    /// antialiasing. `None` (the default): antialiased.
    pub(crate) text_aliasing_threshold: Option<u8>,
}

impl VelloCpuScenePainter {
    pub fn new(width: u16, height: u16) -> Self {
        Self::with_image_cache_config(width, height, ImageCacheConfig::default())
    }

    pub fn with_image_cache_config(width: u16, height: u16, config: ImageCacheConfig) -> Self {
        Self::with_settings(width, height, vello_cpu::RenderSettings::default(), config)
    }

    /// A painter whose context renders with `settings` (worker threads, SIMD level).
    pub fn with_settings(
        width: u16,
        height: u16,
        settings: vello_cpu::RenderSettings,
        config: ImageCacheConfig,
    ) -> Self {
        Self {
            render_ctx: vello_cpu::RenderContext::new_with(width, height, settings),
            resources: vello_cpu::Resources::new(),
            image_cache: ImageCache::new(config),
            filter_contexts: Vec::new(),
            clip_paths: Vec::new(),
            blending: false,
            backdrop_filters: false,
            backdrops: Vec::new(),
            frame_to_ctx: Affine::IDENTITY,
            text_aliasing_threshold: None,
        }
    }

    /// pyro: draws glyphs aliased from now on, a pixel painted when the glyph covers at least
    /// `threshold` of it (out of 255; vello_cpu's aliasing threshold), or antialiased (`None`,
    /// the default). Other drawing stays antialiased.
    pub fn set_text_aliasing_threshold(&mut self, threshold: Option<u8>) {
        self.text_aliasing_threshold = threshold;
    }

    fn convert_paint(&mut self, paint: PaintRef<'_>) -> PaintType {
        match paint {
            Paint::Solid(alpha_color) => PaintType::Solid(alpha_color),
            Paint::Gradient(gradient) => PaintType::Gradient(gradient.clone()),
            Paint::Image(image) => PaintType::Image(ImageBrush {
                image: self
                    .image_cache
                    .get_or_register(&mut self.resources, image.image),
                sampler: image.sampler,
            }),
            // TODO: custom paint
            Paint::Resource(_) => PaintType::Solid(peniko::color::palette::css::TRANSPARENT),
            Paint::Custom(_) => PaintType::Solid(peniko::color::palette::css::TRANSPARENT),
        }
    }

    /// Advance the image cache's frame counter and evict stale entries.
    ///
    /// Should be called once per frame, after rendering.
    pub fn maintain(&mut self) {
        self.image_cache.maintain(&mut self.resources);
    }

    /// Drop all cached image conversions.
    pub fn clear_image_cache(&mut self) {
        self.image_cache.clear(&mut self.resources);
    }

    pub fn finish(mut self) -> Pixmap {
        let mut pixmap = Pixmap::new(self.render_ctx.width(), self.render_ctx.height());
        self.render_ctx.render(pixmap.as_mut(), &mut self.resources);
        pixmap
    }
}

/// pyro: what a push of the painter pushed onto vello_cpu's context, for its pop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pushed {
    /// A layer (`push_layer`, `push_clip_layer`).
    Layer,
    /// A clip path (non-isolated: see [`VelloCpuScenePainter::replay`]).
    ClipPath,
    /// Nothing (an opaque backdrop filter's group: see [`crate::backdrop`]).
    Nothing,
}

impl RenderContext for VelloCpuScenePainter {}
impl PaintScene for VelloCpuScenePainter {
    fn reset(&mut self) {
        self.render_ctx.reset();
        self.clip_paths.clear();
    }

    fn push_layer(
        &mut self,
        fill: Fill,
        blend: impl Into<BlendMode>,
        alpha: f32,
        transform: Affine,
        clip: &impl Shape,
        _filter: Option<Arc<Filter>>,
        _backdrop_filter: Option<Arc<Filter>>,
    ) {
        // pyro: a layer's filter is applied by `append_scene` (see `crate::filters`); a layer
        // pushed here directly is drawn unfiltered, as vello_cpu cannot filter on worker threads.
        self.render_ctx.set_transform(transform);
        self.render_ctx.set_fill_rule(fill);
        self.render_ctx.push_layer(
            Some(&clip.into_path(DEFAULT_TOLERANCE)),
            Some(blend.into()),
            Some(alpha),
            None,
            None,
        );
        self.clip_paths.push(Pushed::Layer);
    }

    fn push_clip_layer(&mut self, fill: Fill, transform: Affine, clip: &impl Shape) {
        self.render_ctx.set_transform(transform);
        self.render_ctx.set_fill_rule(fill);
        self.render_ctx
            .push_clip_layer(&clip.into_path(DEFAULT_TOLERANCE));
        self.clip_paths.push(Pushed::Layer);
    }

    fn pop_layer(&mut self) {
        match self.clip_paths.pop() {
            Some(Pushed::ClipPath) => self.render_ctx.pop_clip(),
            Some(Pushed::Nothing) => {}
            Some(Pushed::Layer) | None => self.render_ctx.pop_layer(),
        }
    }

    fn stroke<'a>(
        &mut self,
        style: &Stroke,
        transform: Affine,
        paint: impl Into<PaintRef<'a>>,
        brush_transform: Option<Affine>,
        shape: &impl Shape,
    ) {
        self.render_ctx.set_transform(transform);
        self.render_ctx.set_stroke(style.clone());
        let paint = self.convert_paint(paint.into());
        self.render_ctx.set_paint(paint);
        self.render_ctx
            .set_paint_transform(brush_transform.unwrap_or(Affine::IDENTITY));
        self.render_ctx
            .stroke_path(&shape.into_path(DEFAULT_TOLERANCE));
    }

    fn fill<'a>(
        &mut self,
        style: Fill,
        transform: Affine,
        paint: impl Into<PaintRef<'a>>,
        brush_transform: Option<Affine>,
        shape: &impl Shape,
    ) {
        self.render_ctx.set_transform(transform);
        self.render_ctx.set_fill_rule(style);
        let paint = self.convert_paint(paint.into());
        self.render_ctx.set_paint(paint);
        self.render_ctx
            .set_paint_transform(brush_transform.unwrap_or(Affine::IDENTITY));
        self.render_ctx
            .fill_path(&shape.into_path(DEFAULT_TOLERANCE));
    }

    fn draw_glyphs<'a, 's: 'a>(
        &'a mut self,
        font: &'a FontData,
        font_size: f32,
        hint: bool,
        normalized_coords: &'a [NormalizedCoord],
        embolden: kurbo::Vec2,
        style: impl Into<StyleRef<'a>>,
        paint: impl Into<PaintRef<'a>>,
        _brush_alpha: f32,
        transform: Affine,
        glyph_transform: Option<Affine>,
        glyphs: impl Iterator<Item = anyrender::Glyph> + Clone,
    ) {
        // pyro: aliased glyphs, for this run only (`set_text_aliasing_threshold`), moved a
        // 512th of a device pixel right and down: a pixel half covered at a glyph's left or top
        // edge is then left out and one at its right or bottom edge painted, as Skia paints
        // aliased paths (rounding a pixel centre on an edge down and right).
        let transform = match self.text_aliasing_threshold {
            Some(_) => Affine::translate((1.0 / 512.0, 1.0 / 512.0)) * transform,
            None => transform,
        };
        self.render_ctx
            .set_aliasing_threshold(self.text_aliasing_threshold);
        self.render_ctx.set_transform(transform);
        let paint = self.convert_paint(paint.into());
        self.render_ctx.set_paint(paint);

        let style: StyleRef<'a> = style.into();
        match style {
            StyleRef::Fill(fill) => {
                self.render_ctx.set_fill_rule(fill);
                let _ = self
                    .render_ctx
                    .glyph_run(&mut self.resources, font)
                    .font_size(font_size)
                    .hint(hint)
                    .normalized_coords(normalized_coords)
                    .font_embolden(embolden_in_cache_units(
                        embolden,
                        font,
                        font_size,
                        hint,
                        transform * glyph_transform.unwrap_or_default(),
                    ))
                    .glyph_transform(glyph_transform.unwrap_or_default())
                    .fill_glyphs(glyphs.map(|g| vello_cpu::Glyph {
                        id: g.id,
                        x: g.x,
                        y: g.y,
                    }));
            }
            StyleRef::Stroke(stroke) => {
                self.render_ctx.set_stroke(stroke.clone());
                let _ = self
                    .render_ctx
                    .glyph_run(&mut self.resources, font)
                    .font_size(font_size)
                    .hint(hint)
                    .normalized_coords(normalized_coords)
                    .glyph_transform(glyph_transform.unwrap_or_default())
                    .stroke_glyphs(glyphs.map(|g| vello_cpu::Glyph {
                        id: g.id,
                        x: g.x,
                        y: g.y,
                    }));
            }
        }
        self.render_ctx.set_aliasing_threshold(None);
    }
    fn draw_box_shadow(
        &mut self,
        transform: Affine,
        rect: Rect,
        color: Color,
        radius: f64,
        std_dev: f64,
    ) {
        self.render_ctx.set_transform(transform);
        self.render_ctx.set_paint(PaintType::Solid(color));
        self.render_ctx.reset_paint_transform();
        self.render_ctx
            .fill_blurred_rounded_rect(&rect, radius as f32, std_dev as f32, false);
    }

    /// pyro: replays `scene` as anyrender does, except that a layer with a filter this backend
    /// does (`blur()`, `drop-shadow()`, the colour functions) is filtered: see [`Self::replay`].
    fn append_scene(&mut self, scene: anyrender::recording::Scene, scene_transform: Affine) {
        self.blending = scene.commands.iter().any(blends);
        self.backdrop_filters = scene.commands.iter().any(
            |command| matches!(command, RenderCommand::PushLayer(layer) if layer.backdrop_filter.is_some()),
        );
        self.backdrops.clear();
        self.frame_to_ctx = Affine::IDENTITY;
        self.replay(&scene.commands, scene_transform);
        self.backdrops.clear();
    }
}

impl VelloCpuScenePainter {
    /// pyro: replays `commands`; a layer with a filter is drawn by [`Self::draw_filtered_layer`],
    /// unless its filter only fades it (`opacity()`: a layer of that opacity) or only maps colours
    /// and its content is in one colour (then drawn in the mapped colour, in a layer as it is).
    ///
    /// A clip with a blend mode in what it clips is a clip path (vello_cpu's non-isolated clip),
    /// not a clip layer: a CSS clip (`overflow`) makes no isolated group, so a `mix-blend-mode`
    /// inside an `overflow: hidden` box blends with the box's background, as in Chromium (Skia's
    /// clips are no layers either); in a clip layer it blended with nothing. What is a group pushes
    /// a layer: a stacking context with blending descendants, a mask, a text clip. Other clips stay
    /// clip layers, which draw what they clip as one: a clip path clips each fill on its own, and
    /// where fills overlap under an antialiased edge their coverage adds up (`clip-path` circles
    /// over a box and its border came out up to 97 levels off).
    ///
    /// A backdrop filter's markers capture and draw the element's backdrop: see
    /// [`crate::backdrop`]. The layers pushed and not popped yet are kept for it (`open`), each
    /// with whether it is a backdrop root.
    pub(crate) fn replay(&mut self, commands: &[RenderCommand], transform: Affine) {
        let mut open: Vec<(usize, bool)> = Vec::new();
        // The pops of local-raster layers replayed as they are (their content captures a backdrop).
        let mut unwrapped: Vec<usize> = Vec::new();
        let mut i = 0;
        while i < commands.len() {
            if unwrapped.last() == Some(&i) {
                unwrapped.pop();
                i += 1;
                continue;
            }
            if self.backdrop_filters
                && let Some((kind, layer, filter)) = crate::backdrop::marker(commands, i)
            {
                match kind {
                    crate::backdrop::Marker::Capture => {
                        self.capture_backdrop(commands, i, &open, transform, layer, filter)
                    }
                    crate::backdrop::Marker::Draw => self.draw_backdrop(layer, transform, false),
                }
                i += 2;
                continue;
            }
            if let RenderCommand::PushClipLayer(clip) = &commands[i]
                && self.blending
                && blends_inside(commands, i)
            {
                self.render_ctx.set_transform(transform * clip.transform);
                self.render_ctx.set_fill_rule(clip.fill);
                self.render_ctx.push_clip_path(&clip.clip);
                self.clip_paths.push(Pushed::ClipPath);
                open.push((i, false));
                i += 1;
                continue;
            }
            // pyro: an `feImage`'s content, which only its filter layer's graph reads
            // (`draw_filtered_layer`): drawn nowhere else.
            if let RenderCommand::PushLayer(layer) = &commands[i]
                && layer.filter.as_deref().and_then(crate::graph::image_source_marker).is_some()
            {
                i = matching_pop(commands, i) + 1;
                continue;
            }
            // pyro (lane H2): a clip rasterized aliased: where a split cut a plane of a 3D
            // rendering context, so that its pieces share each pixel along the cut without a seam.
            if let RenderCommand::PushLayer(layer) = &commands[i]
                && layer.filter.as_deref().is_some_and(crate::projective::is_aliased_clip_marker)
            {
                self.render_ctx.set_transform(transform * layer.transform);
                self.render_ctx.set_fill_rule(layer.fill);
                self.render_ctx.set_aliasing_threshold(Some(128));
                self.render_ctx.push_clip_layer(&layer.clip);
                self.render_ctx.set_aliasing_threshold(None);
                self.clip_paths.push(Pushed::Layer);
                open.push((i, false));
                i += 1;
                continue;
            }
            // pyro (lane H2): a plane drawn through a perspective (`crate::projective`).
            if let RenderCommand::PushLayer(layer) = &commands[i]
                && let Some(homography) = layer.filter.as_deref().and_then(crate::projective::homography)
            {
                let end = matching_pop(commands, i);
                self.draw_projective_layer(layer, homography, &commands[i + 1..end], transform);
                i = end + 1;
                continue;
            }
            if let RenderCommand::PushLayer(layer) = &commands[i]
                && layer.filter.as_deref().is_some_and(is_local_raster_marker)
            {
                let end = matching_pop(commands, i);
                let inner = &commands[i + 1..end];
                // A backdrop filter inside reads what lies beneath the element too (a 3D transform
                // makes no backdrop root), which a context of the element's own does not hold: the
                // content is replayed here, as it is.
                if self.backdrop_filters
                    && (0..inner.len()).any(|j| {
                        matches!(crate::backdrop::marker(inner, j), Some((crate::backdrop::Marker::Capture, ..)))
                    })
                {
                    unwrapped.push(end);
                    i += 1;
                } else {
                    self.draw_local_layer(layer, inner, transform);
                    i = end + 1;
                }
                continue;
            }
            if let RenderCommand::PushLayer(layer) = &commands[i]
                && let Some(plan) =
                    layer.filter.as_deref().and_then(|filter| FilterPlan::new(filter, transform * layer.transform))
            {
                let end = matching_pop(commands, i);
                let inner = &commands[i + 1..end];
                let recolor = match plan.recolors() {
                    Some(matrices) => uniform_color(inner, false).map(|color| map_color(color, matrices)),
                    None => None,
                };
                if plan.is_alpha_only() || recolor.is_some() {
                    self.push_layer(
                        layer.fill,
                        layer.blend,
                        layer.alpha * plan.alpha(),
                        transform * layer.transform,
                        &layer.clip,
                        None,
                        None,
                    );
                    match recolor {
                        Some(_) => inner.iter().for_each(|command| self.replay_command(command, transform, recolor)),
                        None => self.replay(inner, transform),
                    }
                    self.pop_layer();
                } else {
                    self.draw_filtered_layer(layer, &plan, inner, transform);
                }
                i = end + 1;
                continue;
            }
            // An opaque group whose backdrop replaces what lies beneath: the backdrop is drawn
            // first, outside the group, and the group's draw is skipped.
            if self.backdrop_filters
                && self.replaces_backdrop(commands, i)
                && let Some((_, marker, _)) = crate::backdrop::marker(commands, i + 1)
            {
                self.draw_backdrop(marker, transform, true);
                open.push((i, true));
                // The group's clip layer (an opaque group) pushes nothing: it would only round
                // what it holds a second time.
                match &commands[i] {
                    RenderCommand::PushClipLayer(_) => self.clip_paths.push(Pushed::Nothing),
                    command => self.replay_command(command, transform, None),
                }
                i += 3;
                continue;
            }
            match &commands[i] {
                RenderCommand::PushLayer(_) => {
                    open.push((i, self.backdrop_filters && crate::backdrop::is_root(commands, i)))
                }
                RenderCommand::PushClipLayer(_) => {
                    open.push((i, self.backdrop_filters && crate::backdrop::is_root(commands, i)))
                }
                RenderCommand::PopLayer => {
                    open.pop();
                }
                _ => {}
            }
            self.replay_command(&commands[i], transform, None);
            i += 1;
        }
    }

    /// One command, its solid paint replaced by `recolor` if given (a colour filter's).
    fn replay_command(&mut self, command: &RenderCommand, transform: Affine, recolor: Option<Color>) {
        let paint = |brush| recolor.map_or_else(|| paint_ref(brush), Paint::Solid);
        match command {
            RenderCommand::PushLayer(c) => self.push_layer(
                c.fill,
                c.blend,
                c.alpha,
                transform * c.transform,
                &c.clip,
                c.filter.clone(),
                c.backdrop_filter.clone(),
            ),
            RenderCommand::PushClipLayer(c) => self.push_clip_layer(c.fill, transform * c.transform, &c.clip),
            RenderCommand::PopLayer => self.pop_layer(),
            RenderCommand::Stroke(c) => self.stroke(
                &c.style,
                transform * c.transform,
                paint(&c.brush),
                c.brush_transform,
                &c.shape,
            ),
            RenderCommand::Fill(c) => self.fill(
                c.fill,
                transform * c.transform,
                paint(&c.brush),
                c.brush_transform,
                &c.shape,
            ),
            RenderCommand::GlyphRun(c) => self.draw_glyphs(
                &c.font_data,
                c.font_size,
                c.hint,
                &c.normalized_coords,
                c.embolden,
                &c.style,
                paint(&c.brush),
                c.brush_alpha,
                transform * c.transform,
                c.glyph_transform,
                c.glyphs.iter().copied(),
            ),
            RenderCommand::BoxShadow(c) => {
                self.draw_box_shadow(transform * c.transform, c.rect, c.brush, c.radius, c.std_dev)
            }
        }
    }

    /// pyro: the layer of a flat element with a 3D transform (blitz-paint's
    /// `local_raster_marker`), drawn as Chromium composites the element: its commands are
    /// rasterized in the element's plane (the layer's transform maps it to the frame) and the result
    /// is drawn into the frame through that transform, sampled bilinearly. Edges its content shares
    /// are resolved in its plane, on the whole pixels its layout puts them on, not each where the
    /// transform puts it: a blended child's edge over its flattened parent's background came out
    /// a few levels off a parent and child drawn there, where Chromium's layer has the same pixels.
    /// A plane seen edge-on, or too large a one, is drawn as it is.
    fn draw_local_layer(&mut self, layer: &LayerCommand, inner: &[RenderCommand], transform: Affine) {
        const MAX_SIDE: i32 = 8192;
        const MAX_PIXELS: i64 = 16 << 20;
        let Some(to_frame) = local_raster_transform(transform * layer.transform) else {
            self.replay(inner, transform);
            return;
        };
        let from_frame = to_frame.inverse();
        let local = from_frame * transform;
        let Some(bounds) = painted_bounds(inner, local) else { return };
        // What the frame shows of the plane, and a pixel around it that the sampling reads.
        let frame = Rect::new(0.0, 0.0, f64::from(self.render_ctx.width()), f64::from(self.render_ctx.height()));
        let visible = from_frame.transform_rect_bbox(frame.inflate(2.0, 2.0));
        // A bound on a whole pixel stays there through the inverse's rounding (a column the
        // content covers a 1e-13th of was sampled into its edge).
        let region = PixelRect::around(bounds.intersect(visible).inflate(-WHOLE_PIXEL_SLACK, -WHOLE_PIXEL_SLACK));
        if region.is_empty() {
            return;
        }
        if region.width() > MAX_SIDE
            || region.height() > MAX_SIDE
            || i64::from(region.width()) * i64::from(region.height()) > MAX_PIXELS
        {
            self.replay(inner, transform);
            return;
        }
        let (Ok(width), Ok(height)) = (u16::try_from(region.width()), u16::try_from(region.height())) else {
            self.replay(inner, transform);
            return;
        };
        let to_pixels = Affine::translate((-f64::from(region.x0), -f64::from(region.y0)));
        let mut ctx = self.offscreen_context(width, height);
        std::mem::swap(&mut self.render_ctx, &mut ctx);
        let frame_to_ctx = self.frame_to_ctx;
        self.frame_to_ctx = to_pixels * from_frame * frame_to_ctx;
        self.replay(inner, to_pixels * local);
        self.frame_to_ctx = frame_to_ctx;
        std::mem::swap(&mut self.render_ctx, &mut ctx);
        let mut pixels = Pixmap::new(width, height);
        ctx.flush();
        ctx.render(&mut pixels, &mut self.resources);
        self.filter_contexts.push(ctx);

        self.render_ctx.set_transform(to_frame);
        self.render_ctx.set_fill_rule(Fill::NonZero);
        self.render_ctx.set_paint(PaintType::Image(ImageBrush {
            image: ImageSource::Pixmap(Arc::new(pixels)),
            sampler: ImageSampler::new().with_quality(ImageQuality::Medium),
        }));
        self.render_ctx.set_paint_transform(Affine::translate((f64::from(region.x0), f64::from(region.y0))));
        self.render_ctx.fill_rect(&region.to_rect());
        self.render_ctx.reset_paint_transform();
    }

    /// pyro: `commands` rendered into a single-threaded context of `width` by `height` of their own,
    /// `to_offscreen` mapping the frame's device pixels to its pixels.
    fn render_offscreen(
        &mut self,
        commands: &[RenderCommand],
        width: u16,
        height: u16,
        to_offscreen: Affine,
        transform: Affine,
    ) -> Pixmap {
        let mut ctx = self.offscreen_context(width, height);
        std::mem::swap(&mut self.render_ctx, &mut ctx);
        let frame_to_ctx = self.frame_to_ctx;
        self.frame_to_ctx = to_offscreen * frame_to_ctx;
        self.replay(commands, to_offscreen * transform);
        self.frame_to_ctx = frame_to_ctx;
        std::mem::swap(&mut self.render_ctx, &mut ctx);
        let mut pixels = Pixmap::new(width, height);
        ctx.flush();
        ctx.render(&mut pixels, &mut self.resources);
        self.filter_contexts.push(ctx);
        pixels
    }

    /// pyro: a filter layer. Its commands are rendered into a single-threaded context of their own,
    /// over the pixels its filter reads to produce what the layer's clip shows of the frame (at a
    /// reduced size for a large blur: [`FilterPlan::factor`]), the filter makes images of them
    /// ([`FilterPlan::render`]), and those are drawn into the frame inside a layer with the clip,
    /// blend mode and opacity, followed by the commands themselves for a drop shadow.
    fn draw_filtered_layer(
        &mut self,
        layer: &LayerCommand,
        plan: &FilterPlan,
        inner: &[RenderCommand],
        transform: Affine,
    ) {
        let layer_transform = transform * layer.transform;
        let frame = PixelRect {
            x0: 0,
            y0: 0,
            x1: i32::from(self.render_ctx.width()),
            y1: i32::from(self.render_ctx.height()),
        };
        let bounds = PixelRect::around(layer_transform.transform_rect_bbox(layer.clip.bounding_box()));
        let out = bounds.intersect(frame);
        if out.is_empty() {
            return;
        }
        // Whole reduced pixels.
        let factor = plan.factor() as i32;
        let region = plan.source_region(out);
        // pyro: an SVG filter's graph sees nothing outside its filter region (the clip).
        let region = if plan.bounded() { region.intersect(bounds) } else { region };
        // pyro: a reduced drop shadow's pixels are drawn moved by its offset, so the region is
        // aligned to whole reduced pixels where they land: the same blur of the same content, as
        // a drop shadow or as `blur()`, then comes out on the same grid of the frame, not on
        // grids `factor / 2` apart (up to two levels apart everywhere: WPT drop-shadow-scale).
        let (sx, sy) = plan.grid_shift();
        let region = PixelRect {
            x0: (region.x0 + sx).div_euclid(factor) * factor - sx,
            y0: (region.y0 + sy).div_euclid(factor) * factor - sy,
            x1: -(-(region.x1 + sx)).div_euclid(factor) * factor - sx,
            y1: -(-(region.y1 + sy)).div_euclid(factor) * factor - sy,
        };
        let (Ok(width), Ok(height)) = (
            u16::try_from(region.width() / factor),
            u16::try_from(region.height() / factor),
        ) else {
            return;
        };
        let to_offscreen = Affine::scale(1.0 / f64::from(factor))
            * Affine::translate((-f64::from(region.x0), -f64::from(region.y0)));
        let pixels = self.render_offscreen(inner, width, height, to_offscreen, transform);
        // pyro: the content of the filter's `feImage`s, the layers in it that hold them (see
        // `crate::graph::IMAGE_SOURCE`), each rendered over the same pixels.
        let mut sources: Vec<Option<Pixmap>> = Vec::new();
        if plan.reads_images() {
            let mut j = 0;
            while j < inner.len() {
                let end = match &inner[j] {
                    RenderCommand::PushLayer(_) | RenderCommand::PushClipLayer(_) => matching_pop(inner, j),
                    _ => j,
                };
                if let RenderCommand::PushLayer(marker) = &inner[j]
                    && let Some(k) = marker.filter.as_deref().and_then(crate::graph::image_source_marker)
                    && k < 64
                {
                    if sources.len() <= k {
                        sources.resize_with(k + 1, || None);
                    }
                    let content = &inner[j + 1..end];
                    sources[k] = Some(self.render_offscreen(content, width, height, to_offscreen, transform));
                }
                j = end + 1;
            }
        }
        // A blur of glyphs in one colour (a text shadow's) blurs their alpha alone, as the blur mask
        // filter Chromium draws text shadows with does; anything else is blurred in all four
        // channels, as its blur filter does (a box was a level off with an invisible command in
        // another colour beside it: WPT's filtered-inline-applies-to-block).
        let images = plan.render(pixels, &sources, region, uniform_color(inner, true));

        // An opaque layer of the normal blend mode whose clip is a rectangle of whole device pixels
        // needs no layer of its own: the images are drawn inside `out`, the clip there, and the
        // content (a drop shadow's) only when it lies inside the clip too. The layer cost a 4K
        // title card's shadow 3 to 5 ms of CPU time.
        let clip = out_rect(&layer.clip, layer_transform);
        let alpha = layer.alpha * plan.alpha();
        let push = alpha < 1.0
            || layer.blend != BlendMode::default()
            || pixel_rect(&layer.clip, layer_transform) != Some(clip)
            || (plan.draws_content()
                && !content_bounds(inner, transform).is_some_and(|bounds| contains(clip, bounds, frame)));
        if push {
            self.render_ctx.set_transform(layer_transform);
            self.render_ctx.set_fill_rule(layer.fill);
            self.render_ctx
                .push_layer(Some(&layer.clip), Some(layer.blend), Some(alpha), None, None);
        }
        for image in images {
            let area = PixelRect::around(image.bounds()).intersect(out);
            if area.is_empty() {
                continue;
            }
            let quality = if image.smooth { ImageQuality::Medium } else { ImageQuality::Low };
            self.render_ctx.set_transform(Affine::IDENTITY);
            self.render_ctx.set_fill_rule(Fill::NonZero);
            self.render_ctx.set_paint(PaintType::Image(ImageBrush {
                image: ImageSource::Pixmap(Arc::new(image.pixels)),
                sampler: ImageSampler::new().with_quality(quality),
            }));
            self.render_ctx.set_paint_transform(image.transform);
            self.render_ctx.fill_rect(&area.to_rect());
            self.render_ctx.reset_paint_transform();
        }
        if plan.draws_content() {
            self.replay(inner, transform);
        }
        if push {
            self.render_ctx.pop_layer();
        }
    }
}

/// pyro: the transform from the pixels a local-raster layer is rasterized in to the frame, given
/// the one from its plane (`None` for a plane seen edge-on). The plane is rasterized at the
/// larger of the scales the transform applies to its axes, as Chromium picks a layer's raster
/// scale (its ideal contents scale): `scale3d(2, 2, 1)` is drawn as sharp as `scale(2)`, and a
/// rotation out of the plane at the plane's own resolution.
fn local_raster_transform(to_frame: Affine) -> Option<Affine> {
    if !(to_frame.determinant().abs() > 1e-6) {
        return None;
    }
    let [a, b, c, d, _, _] = to_frame.as_coeffs();
    let scale = a.hypot(b).max(c.hypot(d));
    // An interpolated rotation's scale is 1 give or take its rounding.
    let scale = if (scale - 1.0).abs() < 1e-4 { 1.0 } else { scale };
    (scale.is_finite() && scale > 1e-3).then(|| to_frame * Affine::scale(1.0 / scale))
}

/// pyro: how far past a whole pixel a local-raster layer's bound may lie and still be on it.
pub(crate) const WHOLE_PIXEL_SLACK: f64 = 1e-4;

/// pyro: the device pixels `commands` (under `transform`) paint, as [`content_bounds`] has them but
/// for a layer, cut to what it holds: a group's clip is often the viewport (a `mix-blend-mode`'s
/// isolation), which made a local-raster layer's region all of what shows of the plane where the
/// same content without the group had its own bounds. A layer that filters or draws a backdrop
/// keeps its clip (it paints where its content does not), and a local-raster layer in it adds
/// the whole pixels of its plane it fills.
pub(crate) fn painted_bounds(commands: &[RenderCommand], transform: Affine) -> Option<Rect> {
    let mut bounds: Option<Rect> = None;
    let mut i = 0;
    while i < commands.len() {
        let painted = match &commands[i] {
            RenderCommand::PushLayer(layer) if layer.filter.as_deref().is_some_and(is_local_raster_marker) => {
                let end = matching_pop(commands, i);
                let inner = &commands[i + 1..end];
                i = end;
                match local_raster_transform(transform * layer.transform) {
                    Some(to_frame) => painted_bounds(inner, to_frame.inverse() * transform).map(|plane| {
                        let plane = plane.inflate(-WHOLE_PIXEL_SLACK, -WHOLE_PIXEL_SLACK);
                        to_frame.transform_rect_bbox(PixelRect::around(plane).to_rect())
                    }),
                    None => painted_bounds(inner, transform),
                }
            }
            // A filter layer, a backdrop and a projective layer (`crate::projective`, its content in
            // the plane's pixels) paint their clip. A layer with no filter effects (blitz-paint's
            // backdrop-root mark, pyro lane H2) or an aliased clip paints only what it holds.
            RenderCommand::PushLayer(layer)
                if layer.filter.as_deref().is_some_and(|filter| {
                    !filter.nodes().is_empty() && !crate::projective::is_aliased_clip_marker(filter)
                }) || layer.backdrop_filter.is_some() =>
            {
                let clip = (transform * layer.transform).transform_rect_bbox(layer.clip.bounding_box());
                i = matching_pop(commands, i);
                Some(clip)
            }
            command @ (RenderCommand::PushLayer(_) | RenderCommand::PushClipLayer(_)) => {
                let (clip_transform, clip) = match command {
                    RenderCommand::PushLayer(layer) => (layer.transform, layer.clip.bounding_box()),
                    RenderCommand::PushClipLayer(layer) => (layer.transform, layer.clip.bounding_box()),
                    _ => unreachable!(),
                };
                let clip = (transform * clip_transform).transform_rect_bbox(clip);
                let end = matching_pop(commands, i);
                let inside = painted_bounds(&commands[i + 1..end], transform);
                i = end;
                inside.map(|inside| inside.intersect(clip)).filter(|r| r.width() > 0.0 && r.height() > 0.0)
            }
            command => content_bounds(std::slice::from_ref(command), transform),
        };
        if let Some(painted) = painted {
            bounds = Some(bounds.map_or(painted, |b| b.union(painted)));
        }
        i += 1;
    }
    bounds
}

/// pyro: whether `filter` is blitz-paint's `local_raster_marker`: a single offset by negative
/// zero, which moves nothing (see [`VelloCpuScenePainter::draw_local_layer`]).
pub(crate) fn is_local_raster_marker(filter: &Filter) -> bool {
    matches!(
        filter.nodes(),
        [node] if matches!(
            node.effect,
            anyrender::filters::FilterEffect::Offset(v)
                if v.x == 0.0 && v.y == 0.0 && v.x.is_sign_negative() && v.y.is_sign_negative()
        )
    )
}

/// pyro: `clip` under `transform` as a rectangle of whole device pixels, if it is one: a
/// rectangle path (four axis-aligned sides) under a transform without rotation or skew, its
/// edges on whole pixels.
pub(crate) fn pixel_rect(clip: &BezPath, transform: Affine) -> Option<PixelRect> {
    let [_, b, c, _, _, _] = transform.as_coeffs();
    if b != 0.0 || c != 0.0 {
        return None;
    }
    let mut points = Vec::with_capacity(5);
    for element in clip.elements() {
        match *element {
            PathEl::MoveTo(p) if points.is_empty() => points.push(p),
            PathEl::LineTo(p) if !points.is_empty() => points.push(p),
            PathEl::ClosePath if !points.is_empty() => {}
            _ => return None,
        }
    }
    if points.len() == 5 && points[4] == points[0] {
        points.pop();
    }
    if points.len() != 4 {
        return None;
    }
    // Each side horizontal or vertical.
    for i in 0..4 {
        let (p, q) = (points[i], points[(i + 1) % 4]);
        if p.x != q.x && p.y != q.y {
            return None;
        }
    }
    let rect = transform.transform_rect_bbox(clip.bounding_box());
    let whole = |v: f64| (v - v.round()).abs() < 1e-6;
    (whole(rect.x0) && whole(rect.y0) && whole(rect.x1) && whole(rect.y1)).then(|| PixelRect::around(rect))
}

/// The pixels `clip` under `transform` touches.
fn out_rect(clip: &BezPath, transform: Affine) -> PixelRect {
    PixelRect::around(transform.transform_rect_bbox(clip.bounding_box()))
}

/// Whether `bounds` (device px) lies inside `rect` where it is inside `frame`.
fn contains(rect: PixelRect, bounds: Rect, frame: PixelRect) -> bool {
    let inside = PixelRect::around(bounds).intersect(frame);
    inside.is_empty() || inside.intersect(rect) == inside
}

/// pyro: the device pixels `commands` (under `transform`) can paint, conservatively: a layer's by
/// its clip, a glyph run's by its glyph origins with ems around them; `None` when they paint none.
fn content_bounds(commands: &[RenderCommand], transform: Affine) -> Option<Rect> {
    let mut bounds: Option<Rect> = None;
    let mut i = 0;
    while i < commands.len() {
        let painted = match &commands[i] {
            RenderCommand::PushLayer(layer) => {
                let clip = (transform * layer.transform).transform_rect_bbox(layer.clip.bounding_box());
                i = matching_pop(commands, i);
                Some(clip)
            }
            RenderCommand::PushClipLayer(layer) => {
                let clip = (transform * layer.transform).transform_rect_bbox(layer.clip.bounding_box());
                i = matching_pop(commands, i);
                Some(clip)
            }
            RenderCommand::PopLayer => None,
            RenderCommand::Fill(c) => Some((transform * c.transform).transform_rect_bbox(c.shape.bounding_box())),
            RenderCommand::Stroke(c) => {
                let reach = c.style.width * c.style.miter_limit.max(std::f64::consts::SQRT_2) + 1.0;
                Some((transform * c.transform).transform_rect_bbox(c.shape.bounding_box().inflate(reach, reach)))
            }
            RenderCommand::GlyphRun(c) => {
                let em = f64::from(c.font_size).abs() * if c.glyph_transform.is_some() { 3.0 } else { 2.0 }
                    + c.embolden.x.abs().max(c.embolden.y.abs());
                c.glyphs
                    .iter()
                    .map(|g| Rect::new(f64::from(g.x), f64::from(g.y), f64::from(g.x), f64::from(g.y)))
                    .reduce(|a, b| a.union(b))
                    .map(|r| (transform * c.transform).transform_rect_bbox(r.inflate(em, em)))
            }
            RenderCommand::BoxShadow(c) => {
                let reach = c.std_dev.abs() * 3.0 + c.radius.abs() + 1.0;
                Some((transform * c.transform).transform_rect_bbox(c.rect.inflate(reach, reach)))
            }
        };
        if let Some(painted) = painted {
            bounds = Some(bounds.map_or(painted, |b| b.union(painted)));
        }
        i += 1;
    }
    bounds
}

/// The one solid colour `commands` paint in, if they are plain fills, strokes and glyph runs of
/// one colour: their pixels are then that colour times their alpha. `glyphs_only` asks for glyph
/// runs alone (a text shadow's).
fn uniform_color(commands: &[RenderCommand], glyphs_only: bool) -> Option<peniko::Color> {
    let mut color = None;
    for command in commands {
        let paint = match command {
            RenderCommand::GlyphRun(c) => &c.brush,
            RenderCommand::Fill(c) if !glyphs_only => &c.brush,
            RenderCommand::Stroke(c) if !glyphs_only => &c.brush,
            _ => return None,
        };
        let Paint::Solid(c) = paint else { return None };
        match color {
            None => color = Some(*c),
            Some(seen) if seen == *c => {}
            Some(_) => return None,
        }
    }
    color
}

/// pyro: whether a command pushes a layer with a blend mode (`mix-blend-mode`).
fn blends(command: &RenderCommand) -> bool {
    matches!(command, RenderCommand::PushLayer(layer)
        if layer.blend.mix != peniko::Mix::Normal || layer.blend.compose == peniko::Compose::PlusLighter)
}

/// pyro: whether what the clip pushed at `push` clips has a layer with a blend mode that blends
/// with what lies under the clip: in it, or in clips in it, but not in another layer (a group).
fn blends_inside(commands: &[RenderCommand], push: usize) -> bool {
    let mut depth = 0usize;
    let mut i = push + 1;
    while i < commands.len() {
        match &commands[i] {
            command @ RenderCommand::PushLayer(_) => {
                if blends(command) {
                    return true;
                }
                i = matching_pop(commands, i);
            }
            RenderCommand::PushClipLayer(_) => depth += 1,
            RenderCommand::PopLayer if depth == 0 => return false,
            RenderCommand::PopLayer => depth -= 1,
            _ => {}
        }
        i += 1;
    }
    false
}

/// The index of the `PopLayer` that closes the layer pushed at `push` (the end when none does).
fn matching_pop(commands: &[RenderCommand], push: usize) -> usize {
    let mut depth = 0usize;
    for (i, command) in commands.iter().enumerate().skip(push) {
        match command {
            RenderCommand::PushLayer(_) | RenderCommand::PushClipLayer(_) => depth += 1,
            RenderCommand::PopLayer => {
                depth -= 1;
                if depth == 0 {
                    return i;
                }
            }
            _ => {}
        }
    }
    commands.len()
}

fn paint_ref(paint: &Paint) -> PaintRef<'_> {
    match paint {
        Paint::Solid(color) => Paint::Solid(*color),
        Paint::Gradient(gradient) => Paint::Gradient(gradient),
        Paint::Image(image) => Paint::Image(image.as_ref()),
        Paint::Resource(id) => Paint::Resource(*id),
        Paint::Custom(custom) => Paint::Custom(custom.as_ref()),
    }
}

/// pyro: `embolden` (how far each side of a glyph's outline grows, in pixels at the run's font
/// size) as glifo 0.4 takes it. glifo expands the outline it caches, and that outline is in font
/// design units unless the run is hinted (then it is at the size drawn), so an amount passed as
/// it is came out `font_size / units_per_em` times too small without hinting: no synthetic bold
/// at all. glifo's main branch converts the amount itself (`embolden_in_cache_units`); this is
/// the same factor, the cached outline's size over the run's, from the outside.
fn embolden_in_cache_units(
    embolden: kurbo::Vec2,
    font: &FontData,
    font_size: f32,
    hint: bool,
    full_transform: Affine,
) -> FontEmbolden {
    let default = FontEmbolden::new(Diagonal2::new(embolden.x, embolden.y));
    if embolden == kurbo::Vec2::ZERO || !(font_size > 0.0) {
        return default;
    }
    const NEARLY_ZERO: f64 = 1.0 / 4096.0;
    // glifo hints a run only under a positive uniform scale without vertical skew, and then
    // caches outlines at the font size times that scale.
    let [a, b, _, d, _, _] = full_transform.as_coeffs();
    let hinted = hint && (a - d).abs() <= NEARLY_ZERO && a > 0.0 && d > 0.0 && b.abs() <= NEARLY_ZERO;
    let factor = if hinted {
        d
    } else {
        let Some(upem) = units_per_em(font) else {
            return default;
        };
        f64::from(upem) / f64::from(font_size)
    };
    FontEmbolden::new(Diagonal2::new(embolden.x * factor, embolden.y * factor))
        .with_tolerance(default.tolerance * factor)
}

/// The `unitsPerEm` of a font's `head` table (of the collection member `font.index`).
fn units_per_em(font: &FontData) -> Option<u16> {
    let data = font.data.data();
    let u16_at = |at: usize| Some(u16::from_be_bytes(data.get(at..at + 2)?.try_into().ok()?));
    let u32_at = |at: usize| Some(u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?));
    let directory = if data.get(..4)? == b"ttcf" {
        u32_at(12 + 4 * font.index as usize)? as usize
    } else {
        0
    };
    (0..u16_at(directory + 4)? as usize)
        .map(|table| directory + 12 + 16 * table)
        .find(|&record| data.get(record..record + 4) == Some(b"head".as_slice()))
        .and_then(|record| u16_at(u32_at(record + 8)? as usize + 18))
        .filter(|&upem| upem > 0)
}

#[cfg(test)]
mod clip_rule_tests {
    use anyrender::{PaintScene, recording::Scene, render_to_buffer};
    use kurbo::{Affine, BezPath, Rect};
    use peniko::{Fill, Mix, color::palette::css::RED};

    use crate::VelloCpuImageRenderer;

    fn render_clip(fill: Fill, compositing_layer: bool, replay: bool) -> Vec<u8> {
        let path = BezPath::from_svg("M0 0H100V100H0Z M25 25H75V75H25Z").unwrap();
        let draw = |scene: &mut crate::VelloCpuScenePainter| {
            if replay {
                let mut recording = Scene::new();
                draw_clipped(&mut recording, fill, compositing_layer, &path);
                scene.append_scene(recording, Affine::IDENTITY);
            } else {
                draw_clipped(scene, fill, compositing_layer, &path);
            }
        };
        render_to_buffer::<VelloCpuImageRenderer, _>(draw, 100, 100)
    }

    fn draw_clipped(
        scene: &mut impl PaintScene,
        fill: Fill,
        compositing_layer: bool,
        path: &BezPath,
    ) {
        if compositing_layer {
            scene.push_layer(fill, Mix::Normal, 1.0, Affine::IDENTITY, path, None, None);
        } else {
            scene.push_clip_layer(fill, Affine::IDENTITY, path);
        }
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            RED,
            None,
            &Rect::new(0.0, 0.0, 100.0, 100.0),
        );
        scene.pop_layer();
    }

    fn assert_pixels(compositing_layer: bool, replay: bool) {
        for fill in [Fill::NonZero, Fill::EvenOdd] {
            let buffer = render_clip(fill, compositing_layer, replay);
            let pixel = |x: usize, y: usize| &buffer[(y * 100 + x) * 4..(y * 100 + x) * 4 + 4];
            assert_eq!(pixel(10, 10), &[255, 0, 0, 255]);
            assert_eq!(
                pixel(50, 50),
                if fill == Fill::EvenOdd {
                    &[0, 0, 0, 0]
                } else {
                    &[255, 0, 0, 255]
                }
            );
        }
    }

    #[test]
    fn clip_layers_respect_fill_rule() {
        assert_pixels(false, false);
    }

    #[test]
    fn compositing_layers_respect_fill_rule() {
        assert_pixels(true, false);
    }

    #[test]
    fn scene_replay_preserves_clip_rules() {
        assert_pixels(false, true);
        assert_pixels(true, true);
    }

    /// pyro: a clip is no group: a blend mode inside it blends with what lies under it (red in
    /// `difference` over yellow is lime), as CSS clips do. A clip layer isolated it (red).
    #[test]
    fn a_blend_mode_inside_a_clip_blends_with_what_lies_under_it() {
        let yellow = peniko::Color::from_rgba8(255, 255, 0, 255);
        let all = Rect::new(0.0, 0.0, 100.0, 100.0);
        let mut recording = Scene::new();
        recording.fill(Fill::NonZero, Affine::IDENTITY, yellow, None, &all);
        recording.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &Rect::new(0.0, 0.0, 50.0, 100.0));
        recording.push_layer(Fill::NonZero, Mix::Difference, 1.0, Affine::IDENTITY, &all, None, None);
        recording.fill(Fill::NonZero, Affine::IDENTITY, RED, None, &all);
        recording.pop_layer();
        recording.pop_layer();
        let buffer = render_to_buffer::<VelloCpuImageRenderer, _>(|painter| painter.append_scene(recording, Affine::IDENTITY), 100, 100);
        let pixel = |x: usize, y: usize| &buffer[(y * 100 + x) * 4..(y * 100 + x) * 4 + 4];
        assert_eq!(pixel(25, 50), &[0, 255, 0, 255], "blended under the clip");
        assert_eq!(pixel(75, 50), &[255, 255, 0, 255], "outside the clip");
    }

    /// pyro: a clip with no blend mode in what it clips draws that as one: under its antialiased
    /// edge a fill covered by another does not show through, as it does through a clip path
    /// (each fill clipped on its own).
    #[test]
    fn a_clip_without_blending_clips_its_content_as_one() {
        let all = Rect::new(0.0, 0.0, 100.0, 100.0);
        let mut recording = Scene::new();
        recording.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &kurbo::Circle::new((50.0, 50.0), 40.3));
        recording.fill(Fill::NonZero, Affine::IDENTITY, RED, None, &all);
        recording.fill(Fill::NonZero, Affine::IDENTITY, peniko::Color::from_rgba8(0, 0, 255, 255), None, &all);
        recording.pop_layer();
        let buffer = render_to_buffer::<VelloCpuImageRenderer, _>(|painter| painter.append_scene(recording, Affine::IDENTITY), 100, 100);
        assert!(buffer.chunks_exact(4).any(|p| p[3] > 0 && p[3] < 255), "an antialiased edge");
        assert!(buffer.chunks_exact(4).all(|p| p[0] == 0), "no red under the blue");
    }
}

#[cfg(test)]
mod filter_layer_tests {
    use std::sync::Arc;

    use anyrender::filters::{Filter, FilterEffect};
    use anyrender::{PaintScene, recording::Scene, render_to_buffer};
    use kurbo::{Affine, Rect};
    use peniko::{Fill, Mix, color::palette::css::RED};

    use crate::VelloCpuImageRenderer;

    /// A red square under a layer clipped to `clip` with a blue drop shadow 5 px down and right.
    fn render(clip: Rect, square: Rect, alpha: f32) -> Vec<u8> {
        let mut scene = Scene::new();
        let shadow = FilterEffect::drop_shadow(5.0, 5.0, 0.0, peniko::Color::from_rgba8(0, 0, 255, 255));
        let filter = Some(Arc::new(Filter::single(shadow)));
        scene.push_layer(Fill::NonZero, Mix::Normal, alpha, Affine::IDENTITY, &clip, filter, None);
        scene.fill(Fill::NonZero, Affine::IDENTITY, RED, None, &square);
        scene.pop_layer();
        render_to_buffer::<VelloCpuImageRenderer, _>(|painter| painter.append_scene(scene, Affine::IDENTITY), 100, 100)
    }

    fn pixel(buffer: &[u8], x: usize, y: usize) -> &[u8] {
        &buffer[(y * 100 + x) * 4..(y * 100 + x) * 4 + 4]
    }

    /// An opaque filter layer with a clip on whole pixels holding its content is drawn without a
    /// layer of its own: the same pixels.
    #[test]
    fn a_filter_layer_draws_its_content_over_its_shadow() {
        let buffer = render(Rect::new(10.0, 10.0, 90.0, 90.0), Rect::new(20.0, 20.0, 60.0, 60.0), 1.0);
        assert_eq!(pixel(&buffer, 22, 22), [255, 0, 0, 255]);
        assert_eq!(pixel(&buffer, 62, 62), [0, 0, 255, 255]);
        assert_eq!(pixel(&buffer, 5, 5), [0, 0, 0, 0]);
        // At half opacity, in a layer.
        let buffer = render(Rect::new(10.0, 10.0, 90.0, 90.0), Rect::new(20.0, 20.0, 60.0, 60.0), 0.5);
        assert_eq!(pixel(&buffer, 22, 22)[3], 128);
        assert_eq!(pixel(&buffer, 62, 62), [0, 0, 128, 128]);
    }

    /// Content reaching past the clip keeps the layer, which clips it.
    #[test]
    fn content_past_the_clip_is_clipped() {
        let buffer = render(Rect::new(0.0, 0.0, 50.0, 50.0), Rect::new(40.0, 40.0, 80.0, 80.0), 1.0);
        assert_eq!(pixel(&buffer, 45, 45), [255, 0, 0, 255]);
        assert_eq!(pixel(&buffer, 70, 70), [0, 0, 0, 0]);
        // A clip off whole pixels too.
        let buffer = render(Rect::new(0.0, 0.0, 50.5, 50.5), Rect::new(40.0, 40.0, 80.0, 80.0), 1.0);
        assert_eq!(pixel(&buffer, 70, 70), [0, 0, 0, 0]);
    }
}

#[cfg(test)]
mod color_filter_tests {
    use std::sync::Arc;

    use anyrender::filters::{Filter, FilterEffect};
    use anyrender::{PaintScene, recording::Scene, render_to_buffer};
    use kurbo::{Affine, Rect};
    use peniko::{Color, Fill, Mix};

    use crate::VelloCpuImageRenderer;

    /// Squares of `colors` side by side (20 px each from x = 10) under a layer with `filter`.
    fn render(filter: Filter, colors: &[Color], alpha: f32) -> Vec<u8> {
        let mut scene = Scene::new();
        let clip = Rect::new(0.0, 0.0, 100.0, 40.0);
        scene.push_layer(Fill::NonZero, Mix::Normal, alpha, Affine::IDENTITY, &clip, Some(Arc::new(filter)), None);
        for (i, &color) in colors.iter().enumerate() {
            let x = 10.0 + 20.0 * i as f64;
            scene.fill(Fill::NonZero, Affine::IDENTITY, color, None, &Rect::new(x, 10.0, x + 20.0, 30.0));
        }
        scene.pop_layer();
        render_to_buffer::<VelloCpuImageRenderer, _>(|painter| painter.append_scene(scene, Affine::IDENTITY), 100, 40)
    }

    fn pixel(buffer: &[u8], x: usize, y: usize) -> &[u8] {
        &buffer[(y * 100 + x) * 4..(y * 100 + x) * 4 + 4]
    }

    const YELLOW: Color = Color::from_rgba8(255, 255, 0, 255);
    const RED: Color = Color::from_rgba8(255, 0, 0, 255);

    /// Content in one colour is drawn in the mapped colour, content in several is filtered as
    /// pixels: the same colours either way.
    #[test]
    fn a_colour_filter_maps_content_in_one_colour_and_in_several() {
        let grayscale = || Filter::linear_list(std::iter::once(FilterEffect::grayscale(1.0)));
        let one = render(grayscale(), &[YELLOW], 1.0);
        let several = render(grayscale(), &[YELLOW, RED], 1.0);
        for buffer in [&one, &several] {
            // 0.2126 + 0.7152 of 255.
            assert_eq!(pixel(buffer, 20, 20), [237, 237, 237, 255]);
            assert_eq!(pixel(buffer, 5, 5), [0, 0, 0, 0]);
        }
        assert_eq!(pixel(&several, 40, 20), [54, 54, 54, 255]);
    }

    /// `opacity()` alone fades the layer, the layer's own opacity too.
    #[test]
    fn the_opacity_function_fades_the_layer() {
        let filter = Filter::linear_list(std::iter::once(FilterEffect::opacity(0.5)));
        let buffer = render(filter, &[RED, YELLOW], 0.5);
        let near = |got: &[u8], want: [u8; 4]| got.iter().zip(want).all(|(&g, w)| g.abs_diff(w) <= 1);
        assert!(near(pixel(&buffer, 20, 20), [64, 0, 0, 64]), "{:?}", pixel(&buffer, 20, 20));
        assert!(near(pixel(&buffer, 40, 20), [64, 64, 0, 64]), "{:?}", pixel(&buffer, 40, 20));
    }
}

#[cfg(test)]
mod local_raster_tests {
    use std::sync::Arc;

    use anyrender::filters::{Filter, FilterEffect};
    use anyrender::{PaintScene, recording::Scene, render_to_buffer};
    use kurbo::{Affine, Rect, Vec2};
    use peniko::{Color, Fill, Mix};

    use crate::VelloCpuImageRenderer;

    /// pyro: a local-raster layer (blitz-paint's mark) under a rotation: a box that blends in
    /// `difference` over its parent's box, in a group whose clip is the viewport, gives the same
    /// pixels as the colours the blend makes painted without one. The content is rasterized in its
    /// plane over what it paints (not the group's clip, which moved the layer's outer edges) and
    /// drawn through the rotation; each fill drawn through it, the blended edge over the parent's
    /// edge took in what the parent's edge let through.
    #[test]
    fn a_local_raster_layer_resolves_a_blend_in_its_plane() {
        let marker = || Some(Arc::new(Filter::single(FilterEffect::Offset(Vec2::new(-0.0, -0.0)))));
        // The content lies inside the frame (a group clipped to the frame cuts through the pixels
        // its edges sample: blitz-paint widens such a clip in a local-raster layer).
        let plane = Affine::translate((50.0, 10.0)) * Affine::rotate(0.35);
        let rgb = |r, g, b| Color::from_rgba8(r, g, b, 255);
        let viewport = Rect::new(0.0, 0.0, 160.0, 160.0);
        let parent = Rect::new(0.0, 0.0, 60.0, 61.0);
        let child = Rect::new(10.0, 11.0, 90.0, 91.0);
        let render = |blend: bool| {
            let mut scene = Scene::new();
            scene.push_layer(Fill::NonZero, Mix::Normal, 1.0, plane, &Rect::new(-1e7, -1e7, 1e7, 1e7), marker(), None);
            scene.fill(Fill::NonZero, plane, rgb(0, 255, 255), None, &parent);
            if blend {
                scene.push_layer(Fill::NonZero, Mix::Difference, 1.0, Affine::IDENTITY, &viewport, None, None);
                scene.fill(Fill::NonZero, plane, rgb(255, 0, 255), None, &child);
                scene.pop_layer();
            } else {
                scene.fill(Fill::NonZero, plane, rgb(255, 0, 255), None, &child);
                scene.fill(Fill::NonZero, plane, rgb(255, 255, 0), None, &child.intersect(parent));
            }
            scene.pop_layer();
            render_to_buffer::<VelloCpuImageRenderer, _>(|painter| painter.append_scene(scene, Affine::IDENTITY), 160, 160)
        };
        let (blended, painted) = (render(true), render(false));
        assert!(blended.chunks_exact(4).any(|p| p[3] > 0 && p[3] < 255), "antialiased edges");
        let off = blended.chunks_exact(4).zip(painted.chunks_exact(4)).filter(|(a, b)| a != b).count();
        assert_eq!(off, 0, "pixels off");
    }
}
