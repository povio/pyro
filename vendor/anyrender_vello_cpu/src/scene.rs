use std::sync::Arc;

use anyrender::recording::{LayerCommand, RenderCommand};
use anyrender::{Filter, NormalizedCoord, Paint, PaintRef, PaintScene, RenderContext};
use glifo::FontEmbolden;
use kurbo::{Affine, BezPath, Diagonal2, PathEl, Rect, Shape, Stroke};
use peniko::{BlendMode, Color, Fill, FontData, ImageBrush, ImageQuality, ImageSampler, StyleRef};
use vello_common::paint::ImageSource;
use vello_cpu::{PaintType, Pixmap};

use crate::filters::{FilterPlan, PixelRect};
use crate::image_cache::{ImageCache, ImageCacheConfig};

const DEFAULT_TOLERANCE: f64 = 0.1;

pub struct VelloCpuScenePainter {
    pub(crate) render_ctx: vello_cpu::RenderContext,
    pub(crate) resources: vello_cpu::Resources,
    pub(crate) image_cache: ImageCache,
    /// pyro: single-threaded contexts that filter layers are rendered in (one per level of
    /// nesting), kept from frame to frame. See [`crate::filters`].
    pub(crate) filter_contexts: Vec<vello_cpu::RenderContext>,
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

impl RenderContext for VelloCpuScenePainter {}
impl PaintScene for VelloCpuScenePainter {
    fn reset(&mut self) {
        self.render_ctx.reset();
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
    }

    fn push_clip_layer(&mut self, fill: Fill, transform: Affine, clip: &impl Shape) {
        self.render_ctx.set_transform(transform);
        self.render_ctx.set_fill_rule(fill);
        self.render_ctx
            .push_clip_layer(&clip.into_path(DEFAULT_TOLERANCE));
    }

    fn pop_layer(&mut self) {
        self.render_ctx.pop_layer();
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
    /// does (`blur()`, `drop-shadow()`) is rendered by [`Self::draw_filtered_layer`].
    fn append_scene(&mut self, scene: anyrender::recording::Scene, scene_transform: Affine) {
        self.replay(&scene.commands, scene_transform);
    }
}

impl VelloCpuScenePainter {
    fn replay(&mut self, commands: &[RenderCommand], transform: Affine) {
        let mut i = 0;
        while i < commands.len() {
            if let RenderCommand::PushLayer(layer) = &commands[i]
                && let Some(plan) =
                    layer.filter.as_deref().and_then(|filter| FilterPlan::new(filter, transform * layer.transform))
            {
                let end = matching_pop(commands, i);
                self.draw_filtered_layer(layer, &plan, &commands[i + 1..end], transform);
                i = end + 1;
                continue;
            }
            self.replay_command(&commands[i], transform);
            i += 1;
        }
    }

    fn replay_command(&mut self, command: &RenderCommand, transform: Affine) {
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
                paint_ref(&c.brush),
                c.brush_transform,
                &c.shape,
            ),
            RenderCommand::Fill(c) => self.fill(
                c.fill,
                transform * c.transform,
                paint_ref(&c.brush),
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
                paint_ref(&c.brush),
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
        let out = PixelRect::around(layer_transform.transform_rect_bbox(layer.clip.bounding_box())).intersect(frame);
        if out.is_empty() {
            return;
        }
        // Whole reduced pixels.
        let factor = plan.factor() as i32;
        let region = plan.source_region(out);
        let region = PixelRect {
            x0: region.x0.div_euclid(factor) * factor,
            y0: region.y0.div_euclid(factor) * factor,
            x1: -(-region.x1).div_euclid(factor) * factor,
            y1: -(-region.y1).div_euclid(factor) * factor,
        };
        let (Ok(width), Ok(height)) = (
            u16::try_from(region.width() / factor),
            u16::try_from(region.height() / factor),
        ) else {
            return;
        };
        let mut ctx = match self.filter_contexts.pop() {
            Some(mut ctx) => {
                ctx.reset_and_resize(width, height);
                ctx
            }
            None => vello_cpu::RenderContext::new_with(
                width,
                height,
                vello_cpu::RenderSettings {
                    num_threads: 0,
                    ..*self.render_ctx.render_settings()
                },
            ),
        };
        std::mem::swap(&mut self.render_ctx, &mut ctx);
        self.replay(
            inner,
            Affine::scale(1.0 / f64::from(factor))
                * Affine::translate((-f64::from(region.x0), -f64::from(region.y0)))
                * transform,
        );
        std::mem::swap(&mut self.render_ctx, &mut ctx);
        let mut pixels = Pixmap::new(width, height);
        ctx.flush();
        ctx.render(&mut pixels, &mut self.resources);
        self.filter_contexts.push(ctx);
        let images = plan.render(pixels, region, uniform_color(inner));

        // An opaque layer of the normal blend mode whose clip is a rectangle of whole device pixels
        // needs no layer of its own: the images are drawn inside `out`, the clip there, and the
        // content (a drop shadow's) only when it lies inside the clip too. The layer cost a 4K
        // title card's shadow 3 to 5 ms of CPU time.
        let clip = out_rect(&layer.clip, layer_transform);
        let push = layer.alpha < 1.0
            || layer.blend != BlendMode::default()
            || pixel_rect(&layer.clip, layer_transform) != Some(clip)
            || (plan.draws_content()
                && !content_bounds(inner, transform).is_some_and(|bounds| contains(clip, bounds, frame)));
        if push {
            self.render_ctx.set_transform(layer_transform);
            self.render_ctx.set_fill_rule(layer.fill);
            self.render_ctx
                .push_layer(Some(&layer.clip), Some(layer.blend), Some(layer.alpha), None, None);
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

/// pyro: `clip` under `transform` as a rectangle of whole device pixels, if it is one: a
/// rectangle path (four axis-aligned sides) under a transform without rotation or skew, its
/// edges on whole pixels.
fn pixel_rect(clip: &BezPath, transform: Affine) -> Option<PixelRect> {
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
/// one colour (a text shadow's glyphs): their pixels are then that colour times their alpha.
fn uniform_color(commands: &[RenderCommand]) -> Option<peniko::Color> {
    let mut color = None;
    for command in commands {
        let paint = match command {
            RenderCommand::Fill(c) => &c.brush,
            RenderCommand::Stroke(c) => &c.brush,
            RenderCommand::GlyphRun(c) => &c.brush,
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
