use std::sync::Arc;
pub(crate) use pyro_raster::capture::Budget;

use anyrender::{Filter, Glyph, NormalizedCoord, Paint, PaintRef, PaintScene, RenderContext, Scene};
use kurbo::{Affine, Rect, Shape, Stroke, Vec2};
use peniko::{BlendMode, Color, Fill, FontData, StyleRef};

pub(crate) struct Recording<'a>(pub &'a mut Scene, pub &'a Budget);

impl Recording<'_> {
    fn shape(&self, shape: &impl Shape) -> bool {
        if !self.1.take(1) {
            return false;
        }
        let count = shape.path_elements(self.0.tolerance).take(self.1.left.get() as usize + 1).count();
        self.1.take(count)
    }

    fn brush(&self, brush: &PaintRef<'_>) -> bool {
        self.1.take(match brush {
            Paint::Gradient(gradient) => gradient.stops.len(),
            _ => 1,
        })
    }
}

impl RenderContext for Recording<'_> {}
impl PaintScene for Recording<'_> {
    fn reset(&mut self) {
        self.0.reset();
    }
    fn push_layer(
        &mut self,
        fill: Fill,
        blend: impl Into<BlendMode>,
        alpha: f32,
        transform: Affine,
        clip: &impl Shape,
        filter: Option<Arc<Filter>>,
        backdrop: Option<Arc<Filter>>,
    ) {
        if !self.shape(clip) { return; }
        let layers = filter.iter().chain(&backdrop).map(|graph| graph.nodes().len()).sum::<usize>();
        if layers == 0 || {
            let bounds = transform.transform_rect_bbox(clip.bounding_box());
            pyro_raster::capture::surface(bounds.width().ceil() as u64, bounds.height().ceil() as u64, layers)
        } {
            self.0.push_layer(fill, blend, alpha, transform, clip, filter, backdrop);
        }
    }
    fn push_clip_layer(&mut self, fill: Fill, transform: Affine, clip: &impl Shape) {
        if self.shape(clip) {
            self.0.push_clip_layer(fill, transform, clip);
        }
    }
    fn pop_layer(&mut self) {
        if self.1.take(1) {
            self.0.pop_layer();
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
        let paint = paint.into();
        if self.shape(shape) && self.brush(&paint) && self.1.take(style.dash_pattern.len()) {
            self.0.stroke(style, transform, paint, brush_transform, shape);
        }
    }
    fn fill<'a>(
        &mut self,
        style: Fill,
        transform: Affine,
        paint: impl Into<PaintRef<'a>>,
        brush_transform: Option<Affine>,
        shape: &impl Shape,
    ) {
        let paint = paint.into();
        if self.shape(shape) && self.brush(&paint) {
            self.0.fill(style, transform, paint, brush_transform, shape);
        }
    }
    fn draw_glyphs<'a, 's: 'a>(
        &'s mut self,
        font: &'a FontData,
        size: f32,
        hint: bool,
        coords: &'a [NormalizedCoord],
        embolden: Vec2,
        style: impl Into<StyleRef<'a>>,
        brush: impl Into<PaintRef<'a>>,
        alpha: f32,
        transform: Affine,
        glyph_transform: Option<Affine>,
        glyphs: impl Iterator<Item = Glyph> + Clone,
    ) {
        let brush = brush.into();
        if !self.1.take(1 + coords.len()) || !self.brush(&brush) {
            return;
        }
        let count = glyphs.clone().take(self.1.left.get() as usize + 1).count();
        if self.1.take(count) {
            self.0.draw_glyphs(
                font,
                size,
                hint,
                coords,
                embolden,
                style,
                brush,
                alpha,
                transform,
                glyph_transform,
                glyphs,
            );
        }
    }
    fn draw_box_shadow(&mut self, transform: Affine, rect: Rect, brush: Color, radius: f64, std_dev: f64) {
        if self.1.take(1) {
            self.0.draw_box_shadow(transform, rect, brush, radius, std_dev);
        }
    }
}
