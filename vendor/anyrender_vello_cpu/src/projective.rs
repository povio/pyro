//! pyro (lane H2): projective layers. A plane that a 3D transform maps to the screen through a
//! perspective (a rotation out of the plane under `perspective`) becomes a trapezoid, which no 2D
//! affine draws. blitz-paint draws such a plane as a layer marked with its homography: the layer's
//! commands are in the plane's own pixels, the homography maps those to the layer's space (which
//! the layer's transform maps on as usual), and its clip bounds what it paints there.
//!
//! The content is rasterized in the plane, at its own resolution, and each frame pixel the plane
//! covers samples it bilinearly through the inverse homography, as a compositor draws a layer
//! through a perspective transform. A frame point whose plane point lies behind the viewer (W <= 0)
//! shows nothing: the plane is clipped at the viewer's plane before projecting (Chromium splits
//! such a quad at W = 0 as well).

use std::sync::Arc;

use anyrender::Filter;
use anyrender::filters::FilterEffect;
use anyrender::recording::{LayerCommand, RenderCommand};
use kurbo::{Affine, Rect, Shape};
use peniko::{Fill, ImageBrush, ImageQuality, ImageSampler};
use vello_common::paint::ImageSource;
use vello_cpu::{PaintType, Pixmap};

use crate::filters::PixelRect;
use crate::scene::VelloCpuScenePainter;

/// A homography of the plane: a point (x, y) maps to (X / W, Y / W) with
/// `(X, Y, W) = H (x, y, 1)`, `H` row-major.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Homography(pub [f64; 9]);

impl Homography {
    /// The homography of a 2D affine.
    pub fn from_affine(affine: Affine) -> Self {
        let [a, b, c, d, e, f] = affine.as_coeffs();
        Self([a, c, e, b, d, f, 0.0, 0.0, 1.0])
    }

    /// `self` after `first`: `self * first`.
    pub fn after(&self, first: &Homography) -> Self {
        let (a, b) = (&self.0, &first.0);
        let mut out = [0.0; 9];
        for row in 0..3 {
            for col in 0..3 {
                out[row * 3 + col] = (0..3).map(|k| a[row * 3 + k] * b[k * 3 + col]).sum();
            }
        }
        Self(out)
    }

    /// `(X, Y, W)` for the point (x, y).
    pub fn apply(&self, x: f64, y: f64) -> [f64; 3] {
        let m = &self.0;
        [m[0] * x + m[1] * y + m[2], m[3] * x + m[4] * y + m[5], m[6] * x + m[7] * y + m[8]]
    }

    /// The inverse, `None` for a singular homography.
    pub fn inverse(&self) -> Option<Self> {
        let m = &self.0;
        let cofactor = |r0: usize, r1: usize, c0: usize, c1: usize| {
            m[r0 * 3 + c0] * m[r1 * 3 + c1] - m[r0 * 3 + c1] * m[r1 * 3 + c0]
        };
        let adjugate = [
            cofactor(1, 2, 1, 2),
            -cofactor(0, 2, 1, 2),
            cofactor(0, 1, 1, 2),
            -cofactor(1, 2, 0, 2),
            cofactor(0, 2, 0, 2),
            -cofactor(0, 1, 0, 2),
            cofactor(1, 2, 0, 1),
            -cofactor(0, 2, 0, 1),
            cofactor(0, 1, 0, 1),
        ];
        let determinant = m[0] * adjugate[0] + m[1] * adjugate[3] + m[2] * adjugate[6];
        if !(determinant.abs() > 1e-300) || !determinant.is_finite() {
            return None;
        }
        Some(Self(adjugate.map(|v| v / determinant)))
    }

    /// The projection of `rect`, its part behind the viewer (W below `min_w`) cut away: a convex
    /// polygon, empty when nothing of it is in front.
    pub fn project_rect(&self, rect: Rect, min_w: f64) -> Vec<(f64, f64)> {
        let corners = [(rect.x0, rect.y0), (rect.x1, rect.y0), (rect.x1, rect.y1), (rect.x0, rect.y1)];
        let points: Vec<[f64; 3]> = corners.iter().map(|&(x, y)| self.apply(x, y)).collect();
        let mut out = Vec::with_capacity(6);
        for i in 0..points.len() {
            let (a, b) = (points[i], points[(i + 1) % points.len()]);
            let (a_in, b_in) = (a[2] >= min_w, b[2] >= min_w);
            if a_in {
                out.push((a[0] / a[2], a[1] / a[2]));
            }
            if a_in != b_in {
                let t = (min_w - a[2]) / (b[2] - a[2]);
                out.push(((a[0] + (b[0] - a[0]) * t) / min_w, (a[1] + (b[1] - a[1]) * t) / min_w));
            }
        }
        out
    }
}

/// The smallest W a plane point is drawn at.
const MIN_W: f64 = 1e-6;

/// pyro: the homography a projective layer's filter marks (blitz-paint's `three_d::projective_marker`):
/// six offsets, the first by negative zero, then the homography's columns' pairs
/// `(h11, h21), (h12, h22), (h13, h23)`, then `(h31, h32)`, `(h33, -0.0)`.
pub(crate) fn homography(filter: &Filter) -> Option<Homography> {
    let nodes = filter.nodes();
    if nodes.len() != 6 {
        return None;
    }
    let offset = |i: usize| match nodes[i].effect {
        FilterEffect::Offset(v) => Some(v),
        _ => None,
    };
    let marker = offset(0)?;
    if !(marker.x == 0.0 && marker.y == 0.0 && marker.x.is_sign_negative() && marker.y.is_sign_negative()) {
        return None;
    }
    let (c0, c1, c2, r3, last) = (offset(1)?, offset(2)?, offset(3)?, offset(4)?, offset(5)?);
    if !(last.y == 0.0 && last.y.is_sign_negative()) {
        return None;
    }
    Some(Homography([c0.x, c1.x, c2.x, c0.y, c1.y, c2.y, r3.x, r3.y, last.x]))
}

/// pyro: whether `filter` is blitz-paint's mark for a clip rasterized aliased (the cut of a split
/// plane): a single offset by zero and negative zero, which moves nothing.
pub(crate) fn is_aliased_clip_marker(filter: &Filter) -> bool {
    matches!(
        filter.nodes(),
        [node] if matches!(
            node.effect,
            FilterEffect::Offset(v)
                if v.x == 0.0 && v.y == 0.0 && v.x.is_sign_positive() && v.y.is_sign_negative()
        )
    )
}

/// The filter that marks a projective layer with `homography` (see [`homography`]).
pub fn projective_marker(homography: &Homography) -> Arc<Filter> {
    let m = &homography.0;
    let v = |x: f64, y: f64| FilterEffect::Offset(kurbo::Vec2::new(x, y));
    Arc::new(Filter::linear_list(
        [v(-0.0, -0.0), v(m[0], m[3]), v(m[1], m[4]), v(m[2], m[5]), v(m[6], m[7]), v(m[8], -0.0)].into_iter(),
    ))
}

impl VelloCpuScenePainter {
    /// pyro: a projective layer (see the module docs): its commands, in the plane's pixels, are
    /// rasterized there, and the frame pixels inside the layer's clip that the plane covers sample
    /// that raster through the inverse of `homography` (followed by the layer's transform and
    /// `transform`).
    pub(crate) fn draw_projective_layer(
        &mut self,
        layer: &LayerCommand,
        homography: Homography,
        inner: &[RenderCommand],
        transform: Affine,
    ) {
        /// The most pixels a plane is rasterized with; a larger one is rasterized at a lower scale.
        const MAX_PIXELS: f64 = (16 << 20) as f64;
        let to_frame = Homography::from_affine(transform * layer.transform).after(&homography);
        let Some(from_frame) = to_frame.inverse() else { return };
        let Some(bounds) = crate::scene::painted_bounds(inner, Affine::IDENTITY) else { return };
        let frame =
            PixelRect { x0: 0, y0: 0, x1: i32::from(self.render_ctx.width()), y1: i32::from(self.render_ctx.height()) };
        // The frame pixels: what the plane's projection and the layer's clip cover.
        let projected = to_frame.project_rect(bounds, MIN_W);
        if projected.len() < 3 {
            return;
        }
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for &(x, y) in &projected {
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
        }
        let clip = (transform * layer.transform).transform_rect_bbox(layer.clip.bounding_box());
        let target = PixelRect::around(Rect::new(x0, y0, x1, y1).intersect(clip)).intersect(frame);
        if target.is_empty() {
            return;
        }
        // The plane's pixels: its content's whole pixels, at a lower scale when there are too many.
        let region =
            PixelRect::around(bounds.inflate(-crate::scene::WHOLE_PIXEL_SLACK, -crate::scene::WHOLE_PIXEL_SLACK));
        if region.is_empty() {
            return;
        }
        let area = f64::from(region.width()) * f64::from(region.height());
        let scale = if area > MAX_PIXELS { (MAX_PIXELS / area).sqrt() } else { 1.0 };
        let (width, height) = (
            (f64::from(region.width()) * scale).ceil().clamp(1.0, 65535.0) as u16,
            (f64::from(region.height()) * scale).ceil().clamp(1.0, 65535.0) as u16,
        );
        let to_pixels = Affine::scale(scale) * Affine::translate((-f64::from(region.x0), -f64::from(region.y0)));
        let mut ctx = self.offscreen_context(width, height);
        std::mem::swap(&mut self.render_ctx, &mut ctx);
        let frame_to_ctx = self.frame_to_ctx;
        // A backdrop read in the plane reads the plane's own pixels (no affine maps the frame there).
        self.frame_to_ctx = to_pixels;
        self.replay(inner, to_pixels);
        self.frame_to_ctx = frame_to_ctx;
        std::mem::swap(&mut self.render_ctx, &mut ctx);
        let mut pixels = Pixmap::new(width, height);
        ctx.flush();
        ctx.render(&mut pixels, &mut self.resources);
        self.filter_contexts.push(ctx);

        let warped = warp(&pixels, &from_frame, target, (f64::from(region.x0), f64::from(region.y0)), scale);
        self.render_ctx.set_transform(Affine::IDENTITY);
        self.render_ctx.set_fill_rule(Fill::NonZero);
        self.render_ctx.set_paint(PaintType::Image(ImageBrush {
            image: ImageSource::Pixmap(Arc::new(warped)),
            sampler: ImageSampler::new().with_quality(ImageQuality::Low),
        }));
        self.render_ctx.set_paint_transform(Affine::translate((f64::from(target.x0), f64::from(target.y0))));
        self.render_ctx.fill_rect(&target.to_rect());
        self.render_ctx.reset_paint_transform();
    }
}

/// The frame pixels of `target`, each the plane raster `pixels` (the plane's pixels from
/// `origin`, at `scale`) sampled bilinearly at the plane point `from_frame` maps its centre to:
/// transparent where that point is behind the viewer or off the raster.
fn warp(pixels: &Pixmap, from_frame: &Homography, target: PixelRect, origin: (f64, f64), scale: f64) -> Pixmap {
    let (w, h) = (target.width() as usize, target.height() as usize);
    let mut out = Pixmap::new(w as u16, h as u16);
    let (sw, sh) = (i64::from(pixels.width()), i64::from(pixels.height()));
    let src = pixels.data_as_u8_slice();
    let dst = out.data_as_u8_slice_mut();
    // From frame pixels straight to the raster's texel coordinates (texel centres on whole numbers):
    // the raster's placement folded into the inverse homography, which leaves its third row as it
    // is, so the plane point's own W stays 1 / z.
    let to_texels =
        Homography([scale, 0.0, -origin.0 * scale - 0.5, 0.0, scale, -origin.1 * scale - 0.5, 0.0, 0.0, 1.0])
            .after(from_frame);
    let m = to_texels.0;
    let texel = |x: i64, y: i64| -> u32 {
        if x < 0 || y < 0 || x >= sw || y >= sh {
            return 0;
        }
        let i = ((y * sw + x) * 4) as usize;
        u32::from_le_bytes([src[i], src[i + 1], src[i + 2], src[i + 3]])
    };
    for row in 0..h {
        let fy = f64::from(target.y0) + row as f64 + 0.5;
        let fx = f64::from(target.x0) + 0.5;
        // Each step right adds the homography's first column.
        let (mut x, mut y, mut z) =
            (m[0] * fx + m[1] * fy + m[2], m[3] * fx + m[4] * fy + m[5], m[6] * fx + m[7] * fy + m[8]);
        let line = &mut dst[row * w * 4..(row + 1) * w * 4];
        for px_out in line.chunks_exact_mut(4) {
            let (cx, cy, cz) = (x, y, z);
            x += m[0];
            y += m[3];
            z += m[6];
            if !(cz > 0.0) {
                continue;
            }
            let inv = 1.0 / cz;
            let (px, py) = (cx * inv, cy * inv);
            if !(px > -1.0 && py > -1.0 && px < sw as f64 && py < sh as f64) {
                continue;
            }
            let (fx0, fy0) = (px.floor(), py.floor());
            // Weights in 1/256ths.
            let (tx, ty) = (((px - fx0) * 256.0) as u32, ((py - fy0) * 256.0) as u32);
            let (ix, iy) = (fx0 as i64, fy0 as i64);
            let (a, b, c, d) = if ix >= 0 && iy >= 0 && ix + 1 < sw && iy + 1 < sh {
                let i = ((iy * sw + ix) * 4) as usize;
                let j = i + (sw * 4) as usize;
                (
                    u32::from_le_bytes([src[i], src[i + 1], src[i + 2], src[i + 3]]),
                    u32::from_le_bytes([src[i + 4], src[i + 5], src[i + 6], src[i + 7]]),
                    u32::from_le_bytes([src[j], src[j + 1], src[j + 2], src[j + 3]]),
                    u32::from_le_bytes([src[j + 4], src[j + 5], src[j + 6], src[j + 7]]),
                )
            } else {
                (texel(ix, iy), texel(ix + 1, iy), texel(ix, iy + 1), texel(ix + 1, iy + 1))
            };
            if (a | b | c | d) == 0 {
                continue;
            }
            let (wa, wb, wc, wd) = ((256 - tx) * (256 - ty), tx * (256 - ty), (256 - tx) * ty, tx * ty);
            for (k, out) in px_out.iter_mut().enumerate() {
                let shift = 8 * k;
                let channel = |v: u32| (v >> shift) & 0xff;
                let sum = channel(a) * wa + channel(b) * wb + channel(c) * wc + channel(d) * wd;
                *out = ((sum + (1 << 15)) >> 16).min(255) as u8;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyrender::{PaintScene, recording::Scene, render_to_buffer};
    use peniko::{Mix, color::palette::css::LIME};

    use crate::VelloCpuImageRenderer;

    #[test]
    fn the_marker_carries_the_homography() {
        let h = Homography([1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 0.001, -0.002, 1.5]);
        assert_eq!(homography(&projective_marker(&h)), Some(h));
        let inverse = h.inverse().unwrap();
        let identity = h.after(&inverse);
        for (i, v) in identity.0.iter().enumerate() {
            assert!((v - if i % 4 == 0 { 1.0 } else { 0.0 }).abs() < 1e-9, "{identity:?}");
        }
    }

    /// A 40 px square under a homography that keeps its top edge and halves its bottom edge
    /// about the centre: a trapezoid, its slanted sides through the frame's pixels.
    #[test]
    fn a_projective_layer_draws_its_plane_as_a_trapezoid() {
        // W = 1 + y / 40: the bottom edge (y = 40) at W = 2. X shifted to keep the centre.
        let h = Homography([1.0, 0.5, 0.0, 0.0, 2.0, 0.0, 0.0, 1.0 / 40.0, 1.0]);
        let place = Affine::translate((30.0, 10.0));
        let mut scene = Scene::new();
        let clip = Rect::new(0.0, 0.0, 100.0, 100.0);
        scene.push_layer(Fill::NonZero, Mix::Normal, 1.0, place, &clip, Some(projective_marker(&h)), None);
        scene.fill(Fill::NonZero, Affine::IDENTITY, LIME, None, &Rect::new(0.0, 0.0, 40.0, 40.0));
        scene.pop_layer();
        let buffer =
            render_to_buffer::<VelloCpuImageRenderer, _>(|p| p.append_scene(scene, Affine::IDENTITY), 100, 100);
        let alpha = |x: usize, y: usize| buffer[(y * 100 + x) * 4 + 3];
        // The top edge spans x = 30..70 at y = 10; the bottom (y = 40 in the plane) maps to
        // ((x + 20) / 2, 40) + place: x = 40..60, y = 50.
        assert_eq!(alpha(32, 12), 255, "inside near the top-left corner");
        assert_eq!(alpha(33, 47), 0, "outside the slanted side near the bottom");
        assert_eq!(alpha(50, 48), 255, "inside near the bottom");
        assert_eq!(alpha(50, 52), 0, "below the bottom edge");
        // Under W = 1 + y / 40 the plane's middle row (y = 20) maps to Y = 40 / 1.5 = 26.7.
        assert_eq!(alpha(31, 10 + 26), 0, "the side has moved in by a third of the way");
    }
}
