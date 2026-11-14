//! pyro: `backdrop-filter`.
//!
//! blitz-paint records two markers for an element with a backdrop filter, each an empty layer with
//! the filter and the element's border box (its radii too) as the clip:
//!
//! - a **capture** (opacity 0) where the element starts to paint, before its own layers (its
//!   `clip-path`, mask and blend mode are no backdrop root for it);
//! - a **draw** (opacity 1), first in the element's effect group, so that the element's opacity and
//!   `filter` apply to the backdrop it shows, as in Chromium.
//!
//! The capture renders what the element's backdrop root painted before it (the commands since the
//! innermost open layer that is a backdrop root: a filter, opacity, blend mode or backdrop filter,
//! or blitz-paint's empty filter for `clip-path`, a mask and `will-change`; else since the start
//! of the scene or of the filter layer replayed), cut to the border box, into a context of its own;
//! filters it as Chromium does, reading past the box's edges the box mirrored (a blur takes in no
//! colour from outside the box, and no transparency); and keeps the images. The draw draws them
//! inside the border box. Scenes without a backdrop filter skip all of it.

use std::sync::Arc;

use anyrender::recording::{LayerCommand, RenderCommand};
use anyrender::{Filter, PaintScene as _};
use kurbo::{Affine, Rect, Shape};
use peniko::{BlendMode, ImageBrush, ImageQuality, ImageSampler};
use vello_common::paint::ImageSource;
use vello_cpu::{PaintType, Pixmap};

use crate::VelloCpuScenePainter;
use crate::filters::{FilterPlan, PixelRect, mirrored};

/// The opacity of the isolation layers blitz-paint pushes (`isolation: isolate`, the groups of a
/// `mix-blend-mode`, masks, text clips), which are no backdrop root.
const ISOLATION_ALPHA: f32 = 1.0 - f32::EPSILON;

pub(crate) enum Marker {
    Capture,
    Draw,
}

/// The backdrop marker pushed at `i`, if one is: an empty layer with a backdrop filter.
pub(crate) fn marker(commands: &[RenderCommand], i: usize) -> Option<(Marker, &LayerCommand, &Filter)> {
    let RenderCommand::PushLayer(layer) = commands.get(i)? else {
        return None;
    };
    let filter = layer.backdrop_filter.as_deref()?;
    if !matches!(commands.get(i + 1), Some(RenderCommand::PopLayer)) {
        return None;
    }
    Some((if layer.alpha == 0.0 { Marker::Capture } else { Marker::Draw }, layer, filter))
}

/// Whether the layer pushed at `i` is a backdrop root (Filter Effects 2): a filter, an opacity
/// below 1, a blend mode, or an element with a backdrop filter (its group starts with the draw).
pub(crate) fn is_root(commands: &[RenderCommand], i: usize) -> bool {
    // An element with a backdrop filter: its group (a clip layer when opaque) starts with the draw.
    if matches!(marker(commands, i + 1), Some((Marker::Draw, ..))) {
        return true;
    }
    let RenderCommand::PushLayer(layer) = &commands[i] else {
        return false;
    };
    layer.filter.is_some()
        || layer.blend != BlendMode::default()
        || (layer.alpha < 1.0 && layer.alpha != ISOLATION_ALPHA)
}

/// A captured, filtered backdrop.
pub(crate) struct Captured {
    key: Key,
    /// The filter's images, placed in the frame's device pixels.
    images: Vec<(Arc<Pixmap>, Affine)>,
    /// The one colour of all of the filter's image, if it has one.
    solid: Option<peniko::Color>,
    /// Whether the images cover all they cover opaquely (then drawing them over what lies
    /// beneath is drawing them in its place).
    opaque: bool,
    alpha: f32,
}

/// The one colour of all of `pixels` (premultiplied), unpremultiplied, if they have one.
fn uniform(pixels: &Pixmap) -> Option<peniko::Color> {
    let data = pixels.data();
    let first = *data.first()?;
    data.iter().all(|&p| p == first).then(|| {
        let a = f32::from(first.a) / 255.0;
        let channel = |c: u8| if first.a == 0 { 0.0 } else { (f32::from(c) / 255.0 / a).min(1.0) };
        peniko::Color::new([channel(first.r), channel(first.g), channel(first.b), a])
    })
}

/// What ties a capture to its draw: the filter both markers hold (one per element), the element's
/// transform and border box.
type Key = (usize, [f64; 6], Rect);

fn key(layer: &LayerCommand) -> Key {
    let filter = layer.backdrop_filter.as_ref().map_or(0, |filter| std::sync::Arc::as_ptr(filter) as usize);
    (filter, layer.transform.as_coeffs(), layer.clip.bounding_box())
}

impl VelloCpuScenePainter {
    /// The capture at `i` of `commands` (replayed under `transform`, with the layers `open` that
    /// are pushed and not popped there, and whether each is a backdrop root).
    pub(crate) fn capture_backdrop(
        &mut self,
        commands: &[RenderCommand],
        i: usize,
        open: &[(usize, bool)],
        transform: Affine,
        layer: &LayerCommand,
        filter: &Filter,
    ) {
        let key = key(layer);
        if self.backdrops.iter().any(|captured| captured.key == key) {
            return;
        }
        let layer_transform = transform * layer.transform;
        // A filter that changes nothing (`invert(0)`) draws nothing over the backdrop.
        let Some(plan) = FilterPlan::new(filter, layer_transform) else {
            return;
        };
        let plan = plan.at_full_resolution();
        // The box's pixels the filter reads to make what shows of it, and around them what it reads
        // past the box's edges, mirrored. The backdrop root's image is cut to the root's surface:
        // the context drawn into for the scene's root (Chromium's root surface is the viewport) or
        // a filter layer; a root layer's holds all of the box (a box reaching past the frame
        // mirrors at its own edge).
        let root = open.iter().rev().find(|&&(_, root)| root);
        let start = root.map_or(0, |&(at, _)| at + 1);
        let mut boxed = PixelRect::around(layer_transform.transform_rect_bbox(layer.clip.bounding_box()));
        if root.is_none() {
            boxed = boxed.intersect(self.bounds());
        }
        let shown = boxed.intersect(self.bounds());
        if shown.is_empty() {
            return;
        }
        let region = plan.source_region(shown);
        let out = boxed.intersect(region);
        let backdrop = self.render_commands(&commands[start..i], out, transform);
        let input = if region == out { backdrop.clone() } else { mirrored(&backdrop, out, region) };
        let ctx_to_frame = self.frame_to_ctx.inverse();
        let mut images: Vec<(Arc<Pixmap>, Affine)> = plan
            .render(input, region, None)
            .into_iter()
            .map(|image| (Arc::new(image.pixels), ctx_to_frame * image.transform))
            .collect();
        // A backdrop of one colour (a flat background, filtered) is drawn as that colour: a fill of
        // one colour costs less than an image, and vello_cpu's u8 compositing rounds an image under
        // a clip a level apart from one without (a masked backdrop in an `overflow: hidden` box).
        let solid = match &images[..] {
            [(pixels, _)] if !plan.draws_content() => uniform(pixels),
            _ => None,
        };
        let opaque = images.iter().all(|(pixels, _)| pixels.data().iter().all(|p| p.a == 255));
        if plan.draws_content() {
            let origin = Affine::translate((f64::from(out.x0), f64::from(out.y0)));
            images.push((Arc::new(backdrop), ctx_to_frame * origin));
        }
        self.backdrops.push(Captured { key, images, solid, opaque, alpha: plan.alpha() });
    }

    /// The draw of a captured backdrop: its images inside the marker's clip (the border box). With
    /// `replace` they take the place of what lies there (Chromium draws a backdrop filter's output
    /// with a source copy: where the backdrop root painted nothing, the filtered image is
    /// transparent, and what the element's ancestors painted beneath must not show through); else
    /// they go over it (inside the element's group, which starts transparent).
    pub(crate) fn draw_backdrop(&mut self, layer: &LayerCommand, transform: Affine, replace: bool) {
        let key = key(layer);
        let Some(captured) = self.backdrops.iter().rev().find(|captured| captured.key == key) else {
            return;
        };
        let (images, solid, alpha) = (captured.images.clone(), captured.solid, captured.alpha);
        let replace = replace && !captured.opaque;
        let opaque = captured.opaque;
        // A border box that is a rectangle of whole device pixels needs no clip: the images are
        // drawn inside it (a clip path rounded what it clipped a level apart from a plain fill).
        let rect = crate::scene::pixel_rect(&layer.clip, transform * layer.transform).filter(|_| !replace || opaque);
        let bounds = rect.map_or(self.bounds(), |rect| rect.intersect(self.bounds()));
        self.render_ctx.set_transform(transform * layer.transform);
        self.render_ctx.set_fill_rule(layer.fill);
        if replace {
            let copy = BlendMode::new(peniko::Mix::Normal, peniko::Compose::Copy);
            self.render_ctx.push_layer(Some(&layer.clip), Some(copy), None, None, None);
        } else if rect.is_none() {
            self.render_ctx.push_clip_path(&layer.clip);
        }
        if alpha < 1.0 {
            self.render_ctx.push_layer(None, None, Some(alpha), None, None);
        }
        for (pixels, placement) in images {
            let image_transform = self.frame_to_ctx * placement;
            let size = Rect::new(0.0, 0.0, f64::from(pixels.width()), f64::from(pixels.height()));
            let area = PixelRect::around(image_transform.transform_rect_bbox(size)).intersect(bounds);
            if area.is_empty() {
                continue;
            }
            if let Some(color) = solid {
                self.render_ctx.set_transform(Affine::IDENTITY);
                self.render_ctx.set_paint(PaintType::Solid(color));
                self.render_ctx.fill_rect(&area.to_rect());
                continue;
            }
            let [a, b, c, d, e, f] = image_transform.as_coeffs();
            let whole = a == 1.0 && d == 1.0 && b == 0.0 && c == 0.0 && e.fract() == 0.0 && f.fract() == 0.0;
            self.render_ctx.set_transform(Affine::IDENTITY);
            self.render_ctx.set_paint(PaintType::Image(ImageBrush {
                image: ImageSource::Pixmap(pixels),
                sampler: ImageSampler::new().with_quality(if whole { ImageQuality::Low } else { ImageQuality::Medium }),
            }));
            self.render_ctx.set_paint_transform(image_transform);
            self.render_ctx.fill_rect(&area.to_rect());
            self.render_ctx.reset_paint_transform();
        }
        if alpha < 1.0 {
            self.render_ctx.pop_layer();
        }
        if replace {
            self.render_ctx.pop_layer();
        } else if rect.is_none() {
            self.render_ctx.pop_clip();
        }
    }

    /// Whether the group pushed at `i` (an element's, its draw first in it) is opaque and
    /// unfiltered and its backdrop keeps its alpha (no `opacity()`): the backdrop is then drawn
    /// before the group, in place of what lies there.
    pub(crate) fn replaces_backdrop(&self, commands: &[RenderCommand], i: usize) -> bool {
        let Some((Marker::Draw, marker, _)) = marker(commands, i + 1) else {
            return false;
        };
        let key = key(marker);
        self.backdrops.iter().rev().find(|captured| captured.key == key).is_some_and(|captured| captured.alpha == 1.0)
            && match &commands[i] {
                RenderCommand::PushClipLayer(_) => true,
                RenderCommand::PushLayer(group) => {
                    group.alpha == 1.0 && group.filter.is_none() && group.blend == BlendMode::default()
                }
                _ => false,
            }
    }

    /// `commands` (replayed under `transform`) rendered over `out` of the current context into a
    /// pixmap of their own, the layers they leave open closed.
    fn render_commands(&mut self, commands: &[RenderCommand], out: PixelRect, transform: Affine) -> Pixmap {
        let (width, height) = (out.width() as u16, out.height() as u16);
        let mut ctx = self.offscreen_context(width, height);
        std::mem::swap(&mut self.render_ctx, &mut ctx);
        let shift = Affine::translate((-f64::from(out.x0), -f64::from(out.y0)));
        let frame_to_ctx = self.frame_to_ctx;
        self.frame_to_ctx = shift * frame_to_ctx;
        let depth = self.clip_paths.len();
        self.replay(commands, shift * transform);
        while self.clip_paths.len() > depth {
            self.pop_layer();
        }
        self.frame_to_ctx = frame_to_ctx;
        std::mem::swap(&mut self.render_ctx, &mut ctx);
        let mut pixels = Pixmap::new(width, height);
        ctx.flush();
        ctx.render(&mut pixels, &mut self.resources);
        self.filter_contexts.push(ctx);
        pixels
    }

    /// The pixels of the context drawn into.
    pub(crate) fn bounds(&self) -> PixelRect {
        PixelRect { x0: 0, y0: 0, x1: i32::from(self.render_ctx.width()), y1: i32::from(self.render_ctx.height()) }
    }

    /// A single-threaded context of `width` by `height` (one kept from an earlier frame if there is
    /// one): filter layers and backdrops render apart in one.
    pub(crate) fn offscreen_context(&mut self, width: u16, height: u16) -> vello_cpu::RenderContext {
        match self.filter_contexts.pop() {
            Some(mut ctx) => {
                ctx.reset_and_resize(width, height);
                ctx
            }
            None => vello_cpu::RenderContext::new_with(
                width,
                height,
                vello_cpu::RenderSettings { num_threads: 0, ..*self.render_ctx.render_settings() },
            ),
        }
    }
}
