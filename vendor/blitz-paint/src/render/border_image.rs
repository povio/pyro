//! pyro: `border-image` (CSS Backgrounds 3 §6), painted as Blink's
//! `NinePieceImagePainter` and `NinePieceImageGrid` paint it.

use super::{ElementCx, to_image_quality};
use crate::gradient::to_peniko_gradient;
use anyrender::PaintScene;
use blitz_dom::CopyStyleGetters;
use blitz_dom::node::{ImageData, RasterImageData};
use kurbo::{Affine, Rect, Vec2};
use peniko::{Extend, Fill, ImageBrush, ImageQuality, ImageSampler};
use style::values::computed::{
    Gradient as StyloGradient, Length, NumberOrPercentage, border::BorderImageSideWidth,
};
use style::values::generics::{image::GenericImage, length::GenericLengthOrNumber};
use style::values::specified::border::BorderImageRepeatKeyword as Rule;

/// The most tiles one piece draws one by one (a raster piece without spacing is a single
/// repeating fill): a 1px tile over a 4K screen would otherwise be thousands of draws.
const MAX_TILES: usize = 4096;

/// The image a border image is cut from.
enum Source<'a> {
    Raster(&'a RasterImageData),
    #[cfg(feature = "svg")]
    Svg(&'a blitz_dom::node::SvgImageData),
    Gradient(&'a StyloGradient),
}

/// One side of the nine-piece grid: how much of the image it cuts off (image units) and how
/// wide it is drawn (device px, whole).
#[derive(Clone, Copy, Debug, Default)]
struct Edge {
    slice: f64,
    width: f64,
}

impl Edge {
    fn drawable(self) -> bool {
        self.slice > 0.0 && self.width > 0.0
    }

    /// Device px per image unit.
    fn scale(self) -> f64 {
        if self.drawable() {
            self.width / self.slice
        } else {
            1.0
        }
    }
}

/// How one piece covers its area along one axis (Blink's `TileParameters`).
#[derive(Clone, Copy, Debug)]
struct Tiling {
    /// The tile's length, device px.
    tile: f64,
    /// Where the first tile starts, from the area's start.
    phase: f64,
    /// The gap after each tile.
    spacing: f64,
}

/// Blink's `ComputeTileParameters`: `extent` is the area's length, `tile` the tile's length at
/// the piece's scale. `None` for `space` when not even one tile fits: the piece is not drawn.
fn tiling(rule: Rule, extent: f64, tile: f64) -> Option<Tiling> {
    match rule {
        Rule::Stretch => Some(Tiling {
            tile,
            phase: 0.0,
            spacing: 0.0,
        }),
        Rule::Round => {
            let repetitions = (extent / tile).round().max(1.0);
            Some(Tiling {
                tile: extent / repetitions,
                phase: 0.0,
                spacing: 0.0,
            })
        }
        // Centred: a tile in the middle of the area, and the others on both sides of it.
        Rule::Repeat => Some(Tiling {
            tile,
            phase: (extent - tile) / 2.0,
            spacing: 0.0,
        }),
        Rule::Space => {
            let repetitions = (extent / tile).floor();
            if repetitions < 1.0 {
                return None;
            }
            // The space left over goes around the tiles: before the first, between them and
            // after the last.
            let space = (extent - repetitions * tile) / (repetitions + 1.0);
            Some(Tiling {
                tile,
                phase: space,
                spacing: space,
            })
        }
    }
}

/// A piece of the grid: the part of the image it is cut from, where it goes, and how it
/// fills that area. `tile_scale` is in device px per image unit.
struct Piece {
    source: Rect,
    destination: Rect,
    tile_scale: Vec2,
    rules: (Rule, Rule),
    corner: bool,
}

/// A rect from its origin and size, with negative sizes taken as zero (gfx::SizeF's).
fn rect(x: f64, y: f64, width: f64, height: f64) -> Rect {
    Rect::new(x, y, x + width.max(0.0), y + height.max(0.0))
}

impl ElementCx<'_, '_> {
    /// Draw the element's `border-image` instead of its borders. Returns false when there is
    /// none to draw (`border-image-source: none`, or an image that is not loaded): the borders
    /// are then drawn as usual.
    pub(super) fn draw_border_image(&self, scene: &mut impl PaintScene) -> bool {
        let border = self.style.get_border();
        // pyro: a `filter()` source is the image it filters, drawn in a layer with the filters
        // over the border image area. Other sources (almost every element: `none`) cost nothing.
        let (source_image, filter) = match &border.border_image_source {
            GenericImage::Filter(_) => crate::filters::image_filter(
                &border.border_image_source,
                self.scale,
                &self.style.clone_color(),
                self.dom,
            ),
            source => (source, None),
        };
        // pyro: `image-orientation: none`: the pixels were turned by the EXIF orientation when
        // decoded; the border image slices the image as stored.
        let unturned;
        let source = match source_image {
            GenericImage::None => return false,
            GenericImage::Gradient(gradient) => Source::Gradient(gradient),
            GenericImage::Url(_) => match self.element.border_images.first() {
                Some(Some(data)) => match &data.image {
                    ImageData::Raster(raster)
                        if raster.orientation != 1 && blitz_dom::util::image_unoriented(&self.style) =>
                    {
                        unturned = unturned_raster(raster);
                        Source::Raster(&unturned)
                    }
                    ImageData::Raster(raster) => Source::Raster(raster),
                    #[cfg(feature = "svg")]
                    ImageData::Svg(svg) if !svg.intrinsic_dimensions.degenerate_view_box => {
                        Source::Svg(svg)
                    }
                    _ => return false,
                },
                _ => return false,
            },
            _ => return false,
        };
        let scale = self.scale;
        let bb = self.frame.border_box;
        let widths = self.frame.border_width;

        // The border image area: the border box grown by `border-image-outset`, on whole pixels.
        let outset =
            |value: &style::values::computed::NonNegativeLengthOrNumber, width: f64| match value {
                GenericLengthOrNumber::Number(n) => n.0 as f64 * width,
                GenericLengthOrNumber::Length(l) => l.0.px() as f64 * scale,
            };
        let outsets = &border.border_image_outset;
        let area = Rect::new(
            (bb.x0 - outset(&outsets.3, widths.x0)).round(),
            (bb.y0 - outset(&outsets.0, widths.y0)).round(),
            (bb.x1 + outset(&outsets.1, widths.x1)).round(),
            (bb.y1 + outset(&outsets.2, widths.y1)).round(),
        );
        if !(area.width() > 0.0 && area.height() > 0.0) {
            return true;
        }

        // The image's size in its own units: pixels for a raster image, CSS px for an SVG one,
        // device px for a gradient (sized as the border image area). `units` converts image
        // units to device px for `auto` widths, `numbers` slice numbers to image units.
        let (image_w, image_h, units, numbers) = match &source {
            Source::Raster(raster) => (raster.width as f64, raster.height as f64, scale, 1.0),
            #[cfg(feature = "svg")]
            Source::Svg(svg) => {
                let (w, h) = svg_size(svg, area.width() / scale, area.height() / scale);
                (w, h, scale, 1.0)
            }
            Source::Gradient(_) => (area.width(), area.height(), 1.0, scale),
        };
        if !(image_w > 0.0 && image_h > 0.0) {
            return true;
        }

        let slices = &border.border_image_slice;
        let slice = |value: &NumberOrPercentage, extent: f64| {
            let resolved = match value {
                NumberOrPercentage::Number(n) => *n as f64 * numbers,
                NumberOrPercentage::Percentage(p) => p.0 as f64 * extent,
            };
            resolved.min(extent)
        };
        let width = |value: &BorderImageSideWidth, border_width: f64, slice: f64, extent: f64| {
            match value {
                BorderImageSideWidth::Number(n) => (n.0 as f64 * border_width).round(),
                BorderImageSideWidth::Auto => (slice * units).round(),
                BorderImageSideWidth::LengthPercentage(lp) => {
                    (lp.0.resolve(Length::new((extent / scale) as f32)).px() as f64 * scale).round()
                }
            }
        };
        let widths_style = &border.border_image_width;
        let mut top = Edge {
            slice: slice(&slices.offsets.0.0, image_h),
            ..Edge::default()
        };
        let mut right = Edge {
            slice: slice(&slices.offsets.1.0, image_w),
            ..Edge::default()
        };
        let mut bottom = Edge {
            slice: slice(&slices.offsets.2.0, image_h),
            ..Edge::default()
        };
        let mut left = Edge {
            slice: slice(&slices.offsets.3.0, image_w),
            ..Edge::default()
        };
        top.width = width(&widths_style.0, widths.y0, top.slice, area.height());
        right.width = width(&widths_style.1, widths.x1, right.slice, area.width());
        bottom.width = width(&widths_style.2, widths.y1, bottom.slice, area.height());
        left.width = width(&widths_style.3, widths.x0, left.slice, area.width());

        // Opposite sides wider than the area together shrink, all four alike, to whole pixels
        // (rounded: truncating left a pixel's gap between them, WPT `border-image-calc.html`).
        let factor = (area.width() / (left.width + right.width).max(1.0))
            .min(area.height() / (top.width + bottom.width).max(1.0));
        if factor < 1.0 {
            for edge in [&mut top, &mut right, &mut bottom, &mut left] {
                edge.width = (edge.width * factor).round();
            }
        }

        let repeat = &border.border_image_repeat;
        let (horizontal, vertical_rule) = (repeat.0, repeat.1);
        let (iw, ih) = (image_w, image_h);
        let middle_source = (iw - left.slice - right.slice, ih - top.slice - bottom.slice);
        let middle_size = (
            area.width() - left.width - right.width,
            area.height() - top.width - bottom.width,
        );
        let (x0, y0, x1, y1) = (area.x0, area.y0, area.x1, area.y1);

        let mut pieces: Vec<Piece> = Vec::with_capacity(9);
        let corner = |source: Rect, destination: Rect| Piece {
            source,
            destination,
            tile_scale: Vec2::ZERO,
            rules: (Rule::Stretch, Rule::Stretch),
            corner: true,
        };
        if top.drawable() && left.drawable() {
            pieces.push(corner(
                rect(0.0, 0.0, left.slice, top.slice),
                rect(x0, y0, left.width, top.width),
            ));
        }
        if top.drawable() && right.drawable() {
            pieces.push(corner(
                rect(iw - right.slice, 0.0, right.slice, top.slice),
                rect(x1 - right.width, y0, right.width, top.width),
            ));
        }
        if bottom.drawable() && left.drawable() {
            pieces.push(corner(
                rect(0.0, ih - bottom.slice, left.slice, bottom.slice),
                rect(x0, y1 - bottom.width, left.width, bottom.width),
            ));
        }
        if bottom.drawable() && right.drawable() {
            pieces.push(corner(
                rect(
                    iw - right.slice,
                    ih - bottom.slice,
                    right.slice,
                    bottom.slice,
                ),
                rect(
                    x1 - right.width,
                    y1 - bottom.width,
                    right.width,
                    bottom.width,
                ),
            ));
        }
        // The sides: across, as wide as their edge; along, tiled by their rule. A left or
        // right side runs down (`vertical`), a top or bottom one across.
        let side = |edge: Edge, source: Rect, destination: Rect, vertical: bool| {
            let along = if vertical {
                (source.height(), destination.height())
            } else {
                (source.width(), destination.width())
            };
            let rules = if vertical {
                (Rule::Stretch, vertical_rule)
            } else {
                (horizontal, Rule::Stretch)
            };
            (edge.drawable() && along.0 > 0.0 && along.1 > 0.0).then(|| Piece {
                source,
                destination,
                tile_scale: Vec2::new(edge.scale(), edge.scale()),
                rules,
                corner: false,
            })
        };
        pieces.extend(side(
            left,
            rect(0.0, top.slice, left.slice, middle_source.1),
            rect(x0, y0 + top.width, left.width, middle_size.1),
            true,
        ));
        pieces.extend(side(
            right,
            rect(iw - right.slice, top.slice, right.slice, middle_source.1),
            rect(x1 - right.width, y0 + top.width, right.width, middle_size.1),
            true,
        ));
        pieces.extend(side(
            top,
            rect(left.slice, 0.0, middle_source.0, top.slice),
            rect(x0 + left.width, y0, middle_size.0, top.width),
            false,
        ));
        pieces.extend(side(
            bottom,
            rect(left.slice, ih - bottom.slice, middle_source.0, bottom.slice),
            rect(
                x0 + left.width,
                y1 - bottom.width,
                middle_size.0,
                bottom.width,
            ),
            false,
        ));
        // The middle, with `fill`: scaled as the top (or bottom) side across and the left (or
        // right) side down, or stretched along an axis whose rule is `stretch`.
        if slices.fill
            && middle_source.0 > 0.0
            && middle_source.1 > 0.0
            && middle_size.0 > 0.0
            && middle_size.1 > 0.0
        {
            let mut tile_scale = Vec2::new(units, units);
            if top.drawable() {
                tile_scale.x = top.scale();
            } else if bottom.drawable() {
                tile_scale.x = bottom.scale();
            }
            if left.drawable() {
                tile_scale.y = left.scale();
            } else if right.drawable() {
                tile_scale.y = right.scale();
            }
            if horizontal == Rule::Stretch {
                tile_scale.x = middle_size.0 / middle_source.0;
            }
            if vertical_rule == Rule::Stretch {
                tile_scale.y = middle_size.1 / middle_source.1;
            }
            pieces.push(Piece {
                source: rect(left.slice, top.slice, middle_source.0, middle_source.1),
                destination: rect(
                    x0 + left.width,
                    y0 + top.width,
                    middle_size.0,
                    middle_size.1,
                ),
                tile_scale,
                rules: (horizontal, vertical_rule),
                corner: false,
            });
        }

        let painter = PiecePainter {
            cx: self,
            source: &source,
            image: Rect::new(0.0, 0.0, iw, ih),
        };
        let filtered = filter.is_some();
        if filtered {
            scene.push_layer(
                Fill::NonZero,
                peniko::Mix::Normal,
                1.0,
                self.transform,
                &area,
                filter,
                None,
            );
        }
        for piece in &pieces {
            if piece.corner || piece.rules == (Rule::Stretch, Rule::Stretch) {
                painter.stretched(scene, piece.source, piece.destination);
                continue;
            }
            let (dst, src) = (piece.destination, piece.source);
            let Some(h) = tiling(piece.rules.0, dst.width(), src.width() * piece.tile_scale.x)
            else {
                continue;
            };
            let Some(v) = tiling(
                piece.rules.1,
                dst.height(),
                src.height() * piece.tile_scale.y,
            ) else {
                continue;
            };
            painter.tiled(scene, src, dst, h, v);
        }
        if filtered {
            scene.pop_layer();
        }
        true
    }
}

/// The size of an SVG border image in CSS px: its own, or the border image area's where it
/// has none (CSS Images' default sizing, against the area).
#[cfg(feature = "svg")]
fn svg_size(svg: &blitz_dom::node::SvgImageData, area_w: f64, area_h: f64) -> (f64, f64) {
    let width = svg
        .intrinsic_width()
        .filter(|w| w.is_finite() && *w > 0.0)
        .map(f64::from);
    let height = svg
        .intrinsic_height()
        .filter(|h| h.is_finite() && *h > 0.0)
        .map(f64::from);
    let ratio = match (width, height) {
        (Some(w), Some(h)) => Some(w / h),
        _ => svg.viewbox_aspect_ratio().map(f64::from),
    }
    .filter(|r| r.is_finite() && *r > 0.0);
    match (width, height) {
        (Some(w), Some(h)) => (w, h),
        (Some(w), None) => (w, ratio.map_or(area_h, |r| w / r)),
        (None, Some(h)) => (ratio.map_or(area_w, |r| h * r), h),
        (None, None) => match ratio {
            Some(r) => {
                let h = (area_w / r).min(area_h);
                (h * r, h)
            }
            None => (area_w, area_h),
        },
    }
}

/// Draws pieces of one border image.
struct PiecePainter<'a, 'c, 'd, 'e> {
    cx: &'a ElementCx<'c, 'd>,
    source: &'a Source<'e>,
    /// The whole image, in image units.
    image: Rect,
}

impl PiecePainter<'_, '_, '_, '_> {
    /// `source` (image units) drawn into `destination` (device px).
    fn stretched(&self, scene: &mut impl PaintScene, source: Rect, destination: Rect) {
        if source.is_zero_area() || destination.is_zero_area() {
            return;
        }
        if let Source::Raster(raster) = self.source {
            let Some((brush, crop)) = self.crop(raster, source, Extend::Pad) else {
                return;
            };
            let brush_transform = Affine::translate(destination.origin().to_vec2())
                * Affine::scale_non_uniform(
                    destination.width() / crop.0,
                    destination.height() / crop.1,
                );
            scene.fill(
                Fill::NonZero,
                self.cx.transform,
                brush.as_ref(),
                Some(brush_transform),
                &destination,
            );
            return;
        }
        self.vector(scene, source, destination, destination);
    }

    /// `source` tiled over `destination` by the two axes' tilings.
    fn tiled(
        &self,
        scene: &mut impl PaintScene,
        source: Rect,
        destination: Rect,
        h: Tiling,
        v: Tiling,
    ) {
        if source.is_zero_area() || destination.is_zero_area() || !(h.tile > 0.0 && v.tile > 0.0) {
            return;
        }
        let origin = destination.origin() + Vec2::new(h.phase, v.phase);
        if let Source::Raster(raster) = self.source
            && h.spacing == 0.0
            && v.spacing == 0.0
        {
            // One fill of the whole area with the piece repeating: no seams between tiles.
            let Some((brush, crop)) = self.crop(raster, source, Extend::Repeat) else {
                return;
            };
            let brush_transform = Affine::translate(origin.to_vec2())
                * Affine::scale_non_uniform(h.tile / crop.0, v.tile / crop.1);
            scene.fill(
                Fill::NonZero,
                self.cx.transform,
                brush.as_ref(),
                Some(brush_transform),
                &destination,
            );
            return;
        }
        // Each tile on its own: the tiles that overlap the area, cut to it.
        let positions = |start: f64, end: f64, origin: f64, t: Tiling| {
            let period = t.tile + t.spacing;
            let first = ((start - origin) / period).floor() as i64;
            let mut out = Vec::new();
            let mut k = first;
            loop {
                let at = origin + k as f64 * period;
                if at >= end - 1e-6 || out.len() > MAX_TILES {
                    break;
                }
                if at + t.tile > start + 1e-6 {
                    out.push(at);
                }
                k += 1;
            }
            out
        };
        let xs = positions(destination.x0, destination.x1, origin.x, h);
        let ys = positions(destination.y0, destination.y1, origin.y, v);
        if xs.len() * ys.len() > MAX_TILES {
            return;
        }
        let raster = match self.source {
            Source::Raster(raster) => self.crop(raster, source, Extend::Pad),
            _ => None,
        };
        for &y in &ys {
            for &x in &xs {
                let tile = Rect::new(x, y, x + h.tile, y + v.tile);
                let visible = tile.intersect(destination);
                if visible.is_zero_area() {
                    continue;
                }
                match &raster {
                    Some((brush, crop)) => {
                        let brush_transform = Affine::translate(tile.origin().to_vec2())
                            * Affine::scale_non_uniform(
                                tile.width() / crop.0,
                                tile.height() / crop.1,
                            );
                        scene.fill(
                            Fill::NonZero,
                            self.cx.transform,
                            brush.as_ref(),
                            Some(brush_transform),
                            &visible,
                        );
                    }
                    None => self.vector(scene, source, tile, visible),
                }
            }
        }
    }

    /// A gradient or SVG image drawn so that `source` covers `tile`, cut to `visible`.
    fn vector(&self, scene: &mut impl PaintScene, source: Rect, tile: Rect, visible: Rect) {
        // Image units to device px (the element's space).
        let placement = Affine::translate(tile.origin().to_vec2())
            * Affine::scale_non_uniform(
                tile.width() / source.width(),
                tile.height() / source.height(),
            )
            * Affine::translate(-source.origin().to_vec2());
        match self.source {
            Source::Raster(_) => {}
            Source::Gradient(gradient) => {
                let image = self.image;
                let current_color = self.cx.style.clone_color();
                let (gradient, gradient_transform) =
                    to_peniko_gradient(gradient, image, image, self.cx.scale, &current_color);
                if let Some(color) = super::background::uniform_gradient_color(&gradient) {
                    scene.fill(Fill::NonZero, self.cx.transform, color, None, &visible);
                    return;
                }
                let brush_transform = placement * gradient_transform.unwrap_or(Affine::IDENTITY);
                scene.fill(
                    Fill::NonZero,
                    self.cx.transform,
                    anyrender::Paint::Gradient(&gradient),
                    Some(brush_transform),
                    &visible,
                );
            }
            #[cfg(feature = "svg")]
            Source::Svg(svg) => {
                let scale = self.cx.scale;
                // The tree drawn at the image's size in device px, then placed.
                let viewport = super::svg_viewport_affine(
                    svg,
                    (self.image.width() * scale) as f32,
                    (self.image.height() * scale) as f32,
                );
                let transform =
                    self.cx.transform * placement * Affine::scale(1.0 / scale) * viewport;
                scene.push_clip_layer(Fill::NonZero, self.cx.transform, &visible);
                anyrender_svg::render_svg_tree(scene, &svg.tree, transform);
                scene.pop_layer();
            }
        }
    }

    /// The pixels of a raster image under `source` (rounded to whole pixels) as a brush of
    /// their own, so that sampling never reaches the pixels around them; with its size.
    fn crop(
        &self,
        raster: &RasterImageData,
        source: Rect,
        extend: Extend,
    ) -> Option<(ImageBrush, (f64, f64))> {
        let (w, h) = (raster.width as usize, raster.height as usize);
        let x0 = (source.x0.round().max(0.0) as usize).min(w);
        let y0 = (source.y0.round().max(0.0) as usize).min(h);
        let x1 = (source.x1.round().max(0.0) as usize)
            .min(w)
            .max(x0 + 1)
            .min(w);
        let y1 = (source.y1.round().max(0.0) as usize)
            .min(h)
            .max(y0 + 1)
            .min(h);
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        let (cw, ch) = (x1 - x0, y1 - y0);
        let data: &[u8] = raster.data.data();
        let mut pixels = Vec::with_capacity(cw * ch * 4);
        for row in y0..y1 {
            let start = (row * w + x0) * 4;
            pixels.extend_from_slice(data.get(start..start + cw * 4)?);
        }
        let quality: ImageQuality = to_image_quality(self.cx.style.clone_image_rendering());
        let brush = ImageBrush {
            image: peniko::ImageData {
                data: peniko::Blob::from(pixels),
                format: peniko::ImageFormat::Rgba8,
                width: cw as u32,
                height: ch as u32,
                alpha_type: peniko::ImageAlphaType::Alpha,
            },
            sampler: ImageSampler {
                x_extend: extend,
                y_extend: extend,
                quality,
                alpha: 1.0,
            },
        };
        Some((brush, (cw as f64, ch as f64)))
    }
}

/// pyro: `raster`'s pixels as stored, before its EXIF orientation turned them (`image-orientation:
/// none`).
fn unturned_raster(raster: &RasterImageData) -> RasterImageData {
    let (w, h) = (raster.width as usize, raster.height as usize);
    let (sw, sh) = raster.shown_size(true);
    let (sw, sh) = (sw as usize, sh as usize);
    let to_decoded = Affine::new(raster.unturn()).inverse();
    let data: &[u8] = raster.data.data();
    let mut pixels = vec![0u8; sw * sh * 4];
    for sy in 0..sh {
        for sx in 0..sw {
            let p = to_decoded * kurbo::Point::new(sx as f64 + 0.5, sy as f64 + 0.5);
            let (dx, dy) = (p.x.floor(), p.y.floor());
            if dx < 0.0 || dy < 0.0 || dx as usize >= w || dy as usize >= h {
                continue;
            }
            let from = (dy as usize * w + dx as usize) * 4;
            let to = (sy * sw + sx) * 4;
            if let Some(px) = data.get(from..from + 4) {
                pixels[to..to + 4].copy_from_slice(px);
            }
        }
    }
    RasterImageData::new(sw as u32, sh as u32, std::sync::Arc::new(pixels))
}
