//! pyro: the CSS filters `blur()` and `drop-shadow()` on layers.
//!
//! vello_cpu 0.3 filters a layer only when it renders on one thread (its multi-threaded dispatcher
//! panics on a filter layer), takes a single primitive per layer and panics on most of them (colour
//! matrices). So [`crate::VelloCpuScenePainter::append_scene`] does filter layers itself: it renders
//! the layer's commands into a single-threaded context of their own, sized to what the filters read,
//! applies the steps below to those pixels and draws the result into the frame as an image, with the
//! layer's clip, blend mode and opacity. A frame rendered on worker threads gets the same pixels.
//!
//! The blur is Skia's on the CPU, which Chromium's filters and text shadows use: three box blurs
//! (SVG's `feGaussianBlur` approximation) from a standard deviation of 2, a Gaussian kernel below.
//! Other effects (the colour matrices: `grayscale()`, `opacity()`, ...) are left out, as before.

use std::f64::consts::PI;

use anyrender::Filter;
use anyrender::filters::{FilterEffect, FilterInput};
use kurbo::{Affine, Rect};
use peniko::color::{AlphaColor, Srgb};
use vello_cpu::Pixmap;

/// The farthest (device px) a blur reads from: what a larger one would reach beyond it is left
/// out, which keeps the pixels a huge blur renders bounded.
const MAX_REACH: i32 = 1024;

/// A rectangle of whole device pixels: `x0..x1` by `y0..y1`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PixelRect {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl PixelRect {
    /// The pixels `rect` touches.
    pub(crate) fn around(rect: Rect) -> Self {
        let clamp = |v: f64| v.clamp(-(1 << 28) as f64, (1 << 28) as f64) as i32;
        if !(rect.x0 <= rect.x1 && rect.y0 <= rect.y1) {
            return Self::EMPTY;
        }
        Self {
            x0: clamp(rect.x0.floor()),
            y0: clamp(rect.y0.floor()),
            x1: clamp(rect.x1.ceil()),
            y1: clamp(rect.y1.ceil()),
        }
    }

    const EMPTY: Self = Self { x0: 0, y0: 0, x1: 0, y1: 0 };

    pub(crate) fn width(self) -> i32 {
        self.x1 - self.x0
    }

    pub(crate) fn height(self) -> i32 {
        self.y1 - self.y0
    }

    pub(crate) fn is_empty(self) -> bool {
        self.x1 <= self.x0 || self.y1 <= self.y0
    }

    pub(crate) fn intersect(self, other: Self) -> Self {
        Self {
            x0: self.x0.max(other.x0),
            y0: self.y0.max(other.y0),
            x1: self.x1.min(other.x1),
            y1: self.y1.min(other.y1),
        }
    }

    fn union(self, other: Self) -> Self {
        if self.is_empty() {
            return other;
        }
        if other.is_empty() {
            return self;
        }
        Self {
            x0: self.x0.min(other.x0),
            y0: self.y0.min(other.y0),
            x1: self.x1.max(other.x1),
            y1: self.y1.max(other.y1),
        }
    }

    fn inflate(self, by: i32) -> Self {
        Self { x0: self.x0 - by, y0: self.y0 - by, x1: self.x1 + by, y1: self.y1 + by }
    }

    pub(crate) fn to_rect(self) -> Rect {
        Rect::new(self.x0 as f64, self.y0 as f64, self.x1 as f64, self.y1 as f64)
    }
}

/// One filter step, in device pixels.
#[derive(Clone, Copy, Debug)]
enum Step {
    /// `blur()`: every channel blurred.
    Blur { sigma: f64 },
    /// `drop-shadow()`: the layer over its alpha, blurred, moved and coloured.
    DropShadow { dx: f64, dy: f64, sigma: f64, color: AlphaColor<Srgb> },
}

/// The steps of a layer's filter this backend does, in order.
pub(crate) struct FilterPlan {
    steps: Vec<Step>,
}

impl FilterPlan {
    /// The steps of `filter` (a chain, as CSS `filter` lists make) in device pixels under the layer's
    /// `transform`; `None` when it has none this backend does, or is not a chain.
    pub(crate) fn new(filter: &Filter, transform: Affine) -> Option<Self> {
        let [a, b, c, d, _, _] = transform.as_coeffs();
        let scale = (a * d - b * c).abs().sqrt();
        if !scale.is_finite() || scale == 0.0 {
            return None;
        }
        let mut steps = Vec::new();
        for (i, node) in filter.nodes().iter().enumerate() {
            let chained = node.inputs.secondary.is_none()
                && match node.inputs.primary {
                    None => i == 0,
                    Some(FilterInput::Result(id)) => usize::from(id.0) + 1 == i,
                    Some(_) => false,
                };
            if !chained {
                return None;
            }
            match &node.effect {
                FilterEffect::GaussianBlur(blur) if blur.std_deviation > 0.0 => {
                    steps.push(Step::Blur { sigma: f64::from(blur.std_deviation) * scale });
                }
                FilterEffect::DropShadow(shadow) => {
                    let (dx, dy) = (f64::from(shadow.dx), f64::from(shadow.dy));
                    steps.push(Step::DropShadow {
                        dx: a * dx + c * dy,
                        dy: b * dx + d * dy,
                        sigma: f64::from(shadow.std_deviation.max(0.0)) * scale,
                        color: shadow.color,
                    });
                }
                _ => {}
            }
        }
        (!steps.is_empty()).then_some(Self { steps })
    }

    /// The pixels of the layer's unfiltered content the steps read to produce `out`.
    pub(crate) fn source_region(&self, out: PixelRect) -> PixelRect {
        self.steps.iter().rev().fold(out, |needed, step| match *step {
            Step::Blur { sigma } => needed.inflate(reach(sigma)),
            Step::DropShadow { dx, dy, sigma, .. } => {
                let moved = PixelRect {
                    x0: (f64::from(needed.x0) - dx).floor() as i32,
                    y0: (f64::from(needed.y0) - dy).floor() as i32,
                    x1: (f64::from(needed.x1) - dx).ceil() as i32,
                    y1: (f64::from(needed.y1) - dy).ceil() as i32,
                };
                needed.union(moved.inflate(reach(sigma)))
            }
        })
    }

    /// How many times smaller than the frame the layer's content is rendered for the filter: for a
    /// single blur or drop shadow of a standard deviation of 8 device px or more, a power of two
    /// that leaves the deviation at 4 to 8 reduced pixels (as Skia blurs on the GPU), else 1. The
    /// blur then costs as much at any size, and the frame's raster scales the result back up
    /// bilinearly, within two levels of the full-resolution blur.
    pub(crate) fn factor(&self) -> usize {
        match self.steps[..] {
            [Step::Blur { sigma }] | [Step::DropShadow { sigma, .. }] => reduction(sigma),
            _ => 1,
        }
    }

    /// Whether the layer's content is drawn as it is, after the filter's images: the layer over its
    /// drop shadow.
    pub(crate) fn draws_content(&self) -> bool {
        matches!(self.steps[..], [Step::DropShadow { .. }])
    }

    /// The filter's images, to draw in order (see [`FilteredImage`]), from `pixels`: the layer's
    /// content (premultiplied RGBA) over `region`, rendered [`Self::factor`] times smaller.
    /// `uniform` is the one colour all of the content is drawn in, if it is (a text shadow's
    /// glyphs): its blur then blurs the alpha alone.
    pub(crate) fn render(
        &self,
        mut pixels: Pixmap,
        region: PixelRect,
        uniform: Option<AlphaColor<Srgb>>,
    ) -> Vec<FilteredImage> {
        let factor = self.factor();
        let (w, h) = (usize::from(pixels.width()), usize::from(pixels.height()));
        let origin = Affine::translate((f64::from(region.x0), f64::from(region.y0)));
        if w == 0 || h == 0 {
            return Vec::new();
        }
        let mut scratch = Vec::new();
        match self.steps[..] {
            [Step::Blur { sigma }] => {
                let image = match uniform {
                    Some(color) => {
                        let mut plane = alpha_plane(&pixels);
                        blur_reduced(&mut plane, &mut scratch, w, h, 1, sigma, factor);
                        let color = color.with_alpha(1.0).to_rgba8();
                        tinted(&plane, w, h, [color.r, color.g, color.b, 255])
                    }
                    None => {
                        blur_reduced(pixels.data_as_u8_slice_mut(), &mut scratch, w, h, 4, sigma, factor);
                        pixels
                    }
                };
                vec![FilteredImage::scaled(image, origin, factor)]
            }
            [Step::DropShadow { dx, dy, sigma, color }] => {
                let premul = color.premultiply().to_rgba8();
                if premul.a == 0 {
                    return Vec::new();
                }
                let mut plane = alpha_plane(&pixels);
                blur_reduced(&mut plane, &mut scratch, w, h, 1, sigma, factor);
                let shadow = tinted(&plane, w, h, [premul.r, premul.g, premul.b, premul.a]);
                vec![FilteredImage::scaled(shadow, Affine::translate((dx, dy)) * origin, factor)]
            }
            _ => {
                let data = pixels.data_as_u8_slice_mut();
                for step in &self.steps {
                    match *step {
                        Step::Blur { sigma } => blur(data, &mut scratch, w, h, 4, sigma),
                        Step::DropShadow { dx, dy, sigma, color } => {
                            drop_shadow(data, &mut scratch, w, h, dx, dy, sigma, color)
                        }
                    }
                }
                vec![FilteredImage::scaled(pixels, origin, 1)]
            }
        }
    }
}

/// The factor a blur of `sigma` (device px) is computed smaller by: see [`FilterPlan::factor`].
fn reduction(sigma: f64) -> usize {
    let mut factor = 1usize;
    while sigma / (factor * 2) as f64 >= 4.0 && factor < 64 {
        factor *= 2;
    }
    factor
}

/// One image of a filtered layer.
pub(crate) struct FilteredImage {
    pub pixels: Pixmap,
    /// From the image's pixels to the frame's device pixels.
    pub transform: Affine,
    /// Sampled bilinearly (scaled up, or moved by a fraction of a pixel), else nearest.
    pub smooth: bool,
}

impl FilteredImage {
    /// `pixels` at `factor` device px each, placed by `transform`.
    fn scaled(pixels: Pixmap, transform: Affine, factor: usize) -> Self {
        let transform = transform * Affine::scale(factor as f64);
        let [.., e, f] = transform.as_coeffs();
        let smooth = factor > 1 || e.fract() != 0.0 || f.fract() != 0.0;
        Self { pixels, transform, smooth }
    }

    /// The device pixels the image covers.
    pub(crate) fn bounds(&self) -> Rect {
        self.transform.transform_rect_bbox(Rect::new(
            0.0,
            0.0,
            f64::from(self.pixels.width()),
            f64::from(self.pixels.height()),
        ))
    }
}

/// The alpha bytes of premultiplied RGBA pixels.
fn alpha_plane(pixels: &Pixmap) -> Vec<u8> {
    pixels.data().iter().map(|p| p.a).collect()
}

/// `color` (premultiplied RGBA bytes) times each byte of `plane`, as a pixmap.
fn tinted(plane: &[u8], w: usize, h: usize, color: [u8; 4]) -> Pixmap {
    let mut pixmap = Pixmap::new(w as u16, h as u16);
    for (px, &a) in pixmap.data_as_u8_slice_mut().chunks_exact_mut(4).zip(plane) {
        let a = u32::from(a);
        for c in 0..4 {
            px[c] = div255(u32::from(color[c]) * a) as u8;
        }
    }
    pixmap
}

/// `data` (`channels` bytes a pixel, `w` by `h`): content rendered `factor` times smaller, blurred
/// in place as the three boxes of [`blur`] blur it at full resolution: the boxes here (of
/// fractional widths) spread as far as those, less what the rendering at a reduced size (a mean
/// over squares of `factor`) and the bilinear scaling back up spread.
fn blur_reduced(
    data: &mut [u8],
    scratch: &mut Vec<u8>,
    w: usize,
    h: usize,
    channels: usize,
    sigma: f64,
    factor: usize,
) {
    if factor == 1 {
        blur(data, scratch, w, h, channels, sigma);
        return;
    }
    // The variance (device px^2) of the full resolution's boxes, less the reduction's (a box
    // of `factor`: (f^2 - 1) / 12) and the bilinear scaling's (a triangle: f^2 / 6).
    let full: f64 = box_windows(sigma)
        .iter()
        .map(|&(lo, hi)| {
            let d = (lo + 1 + hi) as f64;
            (d * d - 1.0) / 12.0
        })
        .sum();
    let f = factor as f64;
    let variance = ((full - (3.0 * f * f - 1.0) / 12.0) / (f * f)).max(0.0);
    scratch.clear();
    scratch.resize(data.len(), 0);
    for pass in 0..3 {
        let (src, dst) = if pass % 2 == 0 { (&*data, &mut scratch[..]) } else { (&scratch[..], &mut *data) };
        extended_box_rows(src, dst, w, h, channels, variance / 3.0);
    }
    for pass in 0..3 {
        let (src, dst) = if pass % 2 == 0 { (&scratch[..], &mut *data) } else { (&*data, &mut scratch[..]) };
        extended_box_cols(src, dst, w, h, channels, variance / 3.0);
    }
    // Three row passes left the pixels in `scratch`, three column passes in `data`.
}

/// The weights of a box of a fractional width with `variance` (px^2), Gwosdek et al.'s extended
/// box: `r` pixels to each side at full weight, the next at `alpha` of it. Returns `r`, and the
/// two weights in 1/65536ths.
fn extended_box(variance: f64) -> (usize, u32, u32) {
    // A box of radius r has a variance of r (r + 1) / 3.
    let r = (((12.0 * variance + 1.0).sqrt() - 1.0) / 2.0).floor().max(0.0);
    let base = r * (r + 1.0) / 3.0;
    let alpha = ((2.0 * r + 1.0) * (variance - base) / (2.0 * ((r + 1.0) * (r + 1.0) - variance))).clamp(0.0, 1.0);
    let total = 2.0 * r + 1.0 + 2.0 * alpha;
    let full = (65536.0 / total).round() as u32;
    (r as usize, full, (65536.0 * alpha / total).round() as u32)
}

fn extended_box_rows(src: &[u8], dst: &mut [u8], w: usize, h: usize, channels: usize, variance: f64) {
    match channels {
        1 => extended_box_rows_of::<1>(src, dst, w, h, variance),
        _ => extended_box_rows_of::<4>(src, dst, w, h, variance),
    }
}

/// The rows of `C`-byte pixels through a box of `variance`, each row copied between transparent
/// pixels first, so that the loop needs no checks at its ends.
fn extended_box_rows_of<const C: usize>(src: &[u8], dst: &mut [u8], w: usize, h: usize, variance: f64) {
    let (r, full, edge) = extended_box(variance);
    // Output x's window is padded pixels x + 1 ..= x + 2r + 1, the pixels by it x and x + 2r + 2.
    let pad = r + 1;
    let stride = w * C;
    let mut padded = vec![0u8; (w + 2 * pad) * C];
    for (row, out) in src.chunks_exact(stride).zip(dst.chunks_exact_mut(stride)).take(h) {
        padded[pad * C..(pad + w) * C].copy_from_slice(row);
        let mut sums = [0u32; C];
        for k in 1..=2 * r + 1 {
            for c in 0..C {
                sums[c] += u32::from(padded[k * C + c]);
            }
        }
        for x in 0..w {
            let (before, after, first) = (x * C, (x + 2 * r + 2) * C, (x + 1) * C);
            for c in 0..C {
                let (b, a) = (u32::from(padded[before + c]), u32::from(padded[after + c]));
                out[x * C + c] = ((sums[c] * full + (a + b) * edge + 32768) >> 16).min(255) as u8;
                sums[c] = sums[c] + a - u32::from(padded[first + c]);
            }
        }
    }
}

fn extended_box_cols(src: &[u8], dst: &mut [u8], w: usize, h: usize, channels: usize, variance: f64) {
    let (r, full, edge) = extended_box(variance);
    let stride = w * channels;
    let zero = vec![0u8; stride];
    let row = |y: isize| -> &[u8] {
        if y < 0 || y as usize >= h { &zero } else { &src[y as usize * stride..(y as usize + 1) * stride] }
    };
    let mut sums = vec![0u32; stride];
    let r = r as isize;
    for y in -r..=r {
        for (s, &v) in sums.iter_mut().zip(row(y)) {
            *s += u32::from(v);
        }
    }
    for (y, out) in dst.chunks_exact_mut(stride).take(h).enumerate() {
        let y = y as isize;
        let (above, below) = (row(y - r - 1), row(y + r + 1));
        for (((o, &s), &a), &b) in out.iter_mut().zip(&sums).zip(above).zip(below) {
            *o = ((s * full + (u32::from(a) + u32::from(b)) * edge + 32768) >> 16).min(255) as u8;
        }
        for ((s, &add), &sub) in sums.iter_mut().zip(below).zip(row(y - r)) {
            *s = *s + u32::from(add) - u32::from(sub);
        }
    }
}

/// How far (device px) a blur of `sigma` reads.
fn reach(sigma: f64) -> i32 {
    if sigma <= 0.0 {
        return 0;
    }
    ((3.0 * sigma).ceil() as i32 + 1).min(MAX_REACH)
}

/// `drop-shadow()` on premultiplied RGBA `data`: its alpha blurred by `sigma`, moved by `(dx, dy)`
/// and filled with `color`, under the pixels.
#[allow(clippy::too_many_arguments)]
fn drop_shadow(
    data: &mut [u8],
    scratch: &mut Vec<u8>,
    w: usize,
    h: usize,
    dx: f64,
    dy: f64,
    sigma: f64,
    color: AlphaColor<Srgb>,
) {
    let premul = color.premultiply().to_rgba8();
    if premul.a == 0 {
        return;
    }
    let mut alpha: Vec<u8> = data.chunks_exact(4).map(|p| p[3]).collect();
    blur(&mut alpha, scratch, w, h, 1, sigma);
    // The shadow at pixel (x, y) is the blurred alpha at (x - dx, y - dy), interpolated between
    // the four pixels around it when the offset has a fraction (a transformed layer's).
    let (ix, fx) = (dx.floor(), dx - dx.floor());
    let (iy, fy) = (dy.floor(), dy - dy.floor());
    let (ix, iy) = (ix as isize, iy as isize);
    let weights = [
        ((1.0 - fx) * (1.0 - fy) * 65536.0).round() as u32,
        (fx * (1.0 - fy) * 65536.0).round() as u32,
        ((1.0 - fx) * fy * 65536.0).round() as u32,
        (fx * fy * 65536.0).round() as u32,
    ];
    let at = |x: isize, y: isize| -> u32 {
        if x < 0 || y < 0 || x as usize >= w || y as usize >= h {
            0
        } else {
            u32::from(alpha[y as usize * w + x as usize])
        }
    };
    let color = [u32::from(premul.r), u32::from(premul.g), u32::from(premul.b), u32::from(premul.a)];
    for y in 0..h {
        let sy = y as isize - iy;
        for x in 0..w {
            let sx = x as isize - ix;
            // `sx`/`sy` is the sample right of / below the shadow's position.
            let coverage = if weights[0] == 65536 {
                at(sx, sy)
            } else {
                (at(sx - 1, sy - 1) * weights[3]
                    + at(sx, sy - 1) * weights[2]
                    + at(sx - 1, sy) * weights[1]
                    + at(sx, sy) * weights[0]
                    + 32768)
                    >> 16
            };
            if coverage == 0 {
                continue;
            }
            let px = &mut data[(y * w + x) * 4..(y * w + x) * 4 + 4];
            let under = 255 - u32::from(px[3]);
            if under == 0 {
                continue;
            }
            for c in 0..4 {
                // shadow = color * coverage; result = pixel + shadow * (1 - pixel alpha)
                let shadow = color[c] * coverage;
                px[c] = (u32::from(px[c]) + div255(div255(shadow) * under)).min(255) as u8;
            }
        }
    }
}

/// `v / 255` rounded, for `v` up to 255 * 255.
fn div255(v: u32) -> u32 {
    (v + 128 + ((v + 128) >> 8)) >> 8
}

/// Blurs `data` (`w` by `h` pixels of `channels` interleaved bytes) by a Gaussian of `sigma`, with
/// transparent black outside, as Skia does on the CPU: three box blurs from a standard deviation of
/// 2 (SVG's approximation: windows of `d = floor(sigma * 3 * sqrt(2 * pi) / 4 + 0.5)` pixels), a
/// Gaussian kernel below.
pub(crate) fn blur(data: &mut [u8], scratch: &mut Vec<u8>, w: usize, h: usize, channels: usize, sigma: f64) {
    if sigma <= 0.0 || w == 0 || h == 0 {
        return;
    }
    scratch.clear();
    scratch.resize(data.len(), 0);
    if sigma < 2.0 {
        let kernel = gaussian_kernel(sigma);
        convolve_rows(data, scratch, w, h, channels, &kernel);
        convolve_cols(scratch, data, w, h, channels, &kernel);
        return;
    }
    let passes = box_windows(sigma);
    for (i, &(lo, hi)) in passes.iter().enumerate() {
        let (src, dst) = if i % 2 == 0 { (&*data, &mut **scratch) } else { (&**scratch, &mut *data) };
        box_rows(src, dst, w, h, channels, lo, hi);
    }
    // Three passes left the rows in `scratch`.
    for (i, &(lo, hi)) in passes.iter().enumerate() {
        let (src, dst) = if i % 2 == 0 { (&**scratch, &mut *data) } else { (&*data, &mut **scratch) };
        box_cols(src, dst, w, h, channels, lo, hi);
    }
    // ... and the columns in `data`.
}

/// The three box windows (pixels before, pixels after) approximating a Gaussian of `sigma`: of an
/// odd `d`, three centred on the pixel; of an even one, one centred on the edge before the pixel,
/// one on the edge after it, and one of `d + 1` centred on it.
fn box_windows(sigma: f64) -> [(usize, usize); 3] {
    let d = ((sigma * 3.0 * (2.0 * PI).sqrt() / 4.0 + 0.5).floor() as usize).max(1);
    if d % 2 == 1 {
        let r = (d - 1) / 2;
        [(r, r); 3]
    } else {
        let r = d / 2;
        [(r, r - 1), (r - 1, r), (r, r)]
    }
}

/// One box blur along the rows: each output byte is the mean of the `lo + 1 + hi` inputs around it.
fn box_rows(src: &[u8], dst: &mut [u8], w: usize, h: usize, channels: usize, lo: usize, hi: usize) {
    match channels {
        1 => box_rows_of::<1>(src, dst, w, h, lo, hi),
        _ => box_rows_of::<4>(src, dst, w, h, lo, hi),
    }
}

/// The rows of `C`-byte pixels through a box of the `lo + 1 + hi` pixels around each, each row
/// copied between transparent pixels first, so that the loop needs no checks at its ends.
fn box_rows_of<const C: usize>(src: &[u8], dst: &mut [u8], w: usize, h: usize, lo: usize, hi: usize) {
    let size = (lo + 1 + hi) as u64;
    let mul = (1u64 << 32) / size;
    // Output x's window is padded pixels x + pad - lo ..= x + pad + hi.
    let pad = lo.max(hi) + 1;
    let stride = w * C;
    let mut padded = vec![0u8; (w + 2 * pad) * C];
    for (row, out) in src.chunks_exact(stride).zip(dst.chunks_exact_mut(stride)).take(h) {
        padded[pad * C..(pad + w) * C].copy_from_slice(row);
        let mut sums = [0u64; C];
        for k in pad - lo..=pad + hi {
            for c in 0..C {
                sums[c] += u64::from(padded[k * C + c]);
            }
        }
        for x in 0..w {
            let (add, sub) = ((x + pad + hi + 1) * C, (x + pad - lo) * C);
            for c in 0..C {
                out[x * C + c] = ((sums[c] * mul + (1 << 31)) >> 32) as u8;
                sums[c] = sums[c] + u64::from(padded[add + c]) - u64::from(padded[sub + c]);
            }
        }
    }
}

/// One box blur along the columns, a row at a time (all columns' sums kept).
fn box_cols(src: &[u8], dst: &mut [u8], w: usize, h: usize, channels: usize, lo: usize, hi: usize) {
    let size = (lo + 1 + hi) as u64;
    let mul = (1u64 << 32) / size;
    let stride = w * channels;
    let mut sums = vec![0u32; stride];
    for y in 0..=hi.min(h - 1) {
        for (s, &v) in sums.iter_mut().zip(&src[y * stride..(y + 1) * stride]) {
            *s += u32::from(v);
        }
    }
    for y in 0..h {
        for (o, &s) in dst[y * stride..(y + 1) * stride].iter_mut().zip(&sums) {
            *o = ((u64::from(s) * mul + (1 << 31)) >> 32) as u8;
        }
        if y + hi + 1 < h {
            let add = &src[(y + hi + 1) * stride..(y + hi + 2) * stride];
            for (s, &v) in sums.iter_mut().zip(add) {
                *s += u32::from(v);
            }
        }
        if y >= lo {
            let sub = &src[(y - lo) * stride..(y - lo + 1) * stride];
            for (s, &v) in sums.iter_mut().zip(sub) {
                *s -= u32::from(v);
            }
        }
    }
}

/// A normalized Gaussian kernel of `sigma`, `3 * sigma` (rounded up) to each side, in 1/65536ths.
fn gaussian_kernel(sigma: f64) -> Vec<u32> {
    let r = (3.0 * sigma).ceil().max(1.0) as i32;
    let weights: Vec<f64> = (-r..=r).map(|i| (-(f64::from(i) * f64::from(i)) / (2.0 * sigma * sigma)).exp()).collect();
    let total: f64 = weights.iter().sum();
    weights.iter().map(|v| (v / total * 65536.0).round() as u32).collect()
}

fn convolve_rows(src: &[u8], dst: &mut [u8], w: usize, h: usize, channels: usize, kernel: &[u32]) {
    let r = kernel.len() / 2;
    let stride = w * channels;
    for y in 0..h {
        let row = &src[y * stride..(y + 1) * stride];
        let out = &mut dst[y * stride..(y + 1) * stride];
        for x in 0..w {
            for c in 0..channels {
                let mut sum = 0u32;
                for (k, &weight) in kernel.iter().enumerate() {
                    let sx = x + k;
                    if sx >= r && sx - r < w {
                        sum += weight * u32::from(row[(sx - r) * channels + c]);
                    }
                }
                out[x * channels + c] = ((sum + 32768) >> 16).min(255) as u8;
            }
        }
    }
}

fn convolve_cols(src: &[u8], dst: &mut [u8], w: usize, h: usize, channels: usize, kernel: &[u32]) {
    let r = kernel.len() / 2;
    let stride = w * channels;
    let mut sums = vec![0u32; stride];
    for y in 0..h {
        sums.fill(0);
        for (k, &weight) in kernel.iter().enumerate() {
            let sy = y + k;
            if sy < r || sy - r >= h {
                continue;
            }
            for (s, &v) in sums.iter_mut().zip(&src[(sy - r) * stride..(sy - r + 1) * stride]) {
                *s += weight * u32::from(v);
            }
        }
        for (o, &s) in dst[y * stride..(y + 1) * stride].iter_mut().zip(&sums) {
            *o = ((s + 32768) >> 16).min(255) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The blurred edge of a half plane: a Gaussian's cumulative distribution, within the box
    /// approximation's error.
    #[test]
    fn a_blurred_edge_follows_the_gaussian() {
        for sigma in [1.0, 3.0, 10.0, 30.0] {
            let w = (sigma * 12.0) as usize + 2;
            let h = w;
            let mut plane: Vec<u8> = (0..w * h).map(|i| if i % w < w / 2 { 255 } else { 0 }).collect();
            let mut scratch = Vec::new();
            blur(&mut plane, &mut scratch, w, h, 1, sigma);
            let row = &plane[h / 2 * w..(h / 2 + 1) * w];
            for offset in [-2.0, -1.0, 0.0, 1.0, 2.0] {
                // Pixel centres: the edge is at w / 2.
                let x = (w as f64 / 2.0 + offset * sigma - 0.5).round() as usize;
                let d = (x as f64 + 0.5 - w as f64 / 2.0) / sigma;
                let expected = 255.0 * 0.5 * libm_erfc(d / std::f64::consts::SQRT_2);
                let got = f64::from(row[x]);
                assert!((got - expected).abs() <= 6.0, "sigma {sigma} at {offset} sigma: {got} vs {expected}");
            }
        }
    }

    fn libm_erfc(x: f64) -> f64 {
        // Abramowitz and Stegun 7.1.26, |error| < 1.5e-7.
        let t = 1.0 / (1.0 + 0.3275911 * x.abs());
        let y = t * (0.254829592 + t * (-0.284496736 + t * (1.421413741 + t * (-1.453152027 + t * 1.061405429))));
        let erfc = y * (-x * x).exp();
        if x >= 0.0 { erfc } else { 2.0 - erfc }
    }

    /// A blur keeps what it spreads: the sum of a channel stays (to rounding), and nothing leaks
    /// across the buffer's edge from outside.
    #[test]
    fn blurring_keeps_the_total() {
        let (w, h) = (64, 48);
        let mut data = vec![0u8; w * h * 4];
        for y in 20..28 {
            for x in 24..40 {
                data[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(&[200, 100, 50, 255]);
            }
        }
        let before: u64 = data.iter().skip(3).step_by(4).map(|&v| u64::from(v)).sum();
        blur(&mut data, &mut Vec::new(), w, h, 4, 3.0);
        let after: u64 = data.iter().skip(3).step_by(4).map(|&v| u64::from(v)).sum();
        assert!((before as i64 - after as i64).abs() < (before / 50) as i64, "{before} vs {after}");
        // Premultiplied stays premultiplied.
        for p in data.chunks_exact(4) {
            assert!(p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3], "{p:?}");
        }
    }

    fn square(w: usize, h: usize, x: std::ops::Range<usize>, y: std::ops::Range<usize>) -> Pixmap {
        let mut pixmap = Pixmap::new(w as u16, h as u16);
        let data = pixmap.data_as_u8_slice_mut();
        for y in y {
            for x in x.clone() {
                data[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(&[255, 255, 255, 255]);
            }
        }
        pixmap
    }

    fn px(pixmap: &Pixmap, x: usize, y: usize) -> [u8; 4] {
        let w = usize::from(pixmap.width());
        pixmap.data_as_u8_slice()[(y * w + x) * 4..(y * w + x) * 4 + 4].try_into().unwrap()
    }

    #[test]
    fn a_drop_shadow_is_the_alpha_coloured_and_moved_under_the_pixels() {
        let filter = Filter::linear_list(std::iter::once(FilterEffect::drop_shadow(
            4.0,
            8.0,
            0.0,
            AlphaColor::from_rgba8(0, 0, 255, 255),
        )));
        // Device offsets are the layer's times its scale.
        let plan = FilterPlan::new(&filter, Affine::scale(2.0)).unwrap();
        let out = PixelRect { x0: 0, y0: 0, x1: 20, y1: 20 };
        assert_eq!(plan.source_region(out), PixelRect { x0: -8, y0: -16, x1: 20, y1: 20 });
        assert_eq!(plan.factor(), 1);
        let images = plan.render(square(20, 20, 2..6, 2..6), out, None);
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].transform, Affine::translate((8.0, 16.0)));
        assert!(!images[0].smooth);
        assert_eq!(px(&images[0].pixels, 3, 3), [0, 0, 255, 255]);
        assert_eq!(px(&images[0].pixels, 1, 1), [0, 0, 0, 0]);
        // The layer itself is drawn over it as it is.
        assert!(plan.draws_content());
    }

    #[test]
    fn a_chain_of_filters_is_applied_in_order() {
        let filter = Filter::linear_list(
            [
                FilterEffect::drop_shadow(4.0, 0.0, 0.0, AlphaColor::from_rgba8(0, 0, 255, 255)),
                FilterEffect::drop_shadow(0.0, 4.0, 0.0, AlphaColor::from_rgba8(255, 0, 0, 255)),
            ]
            .into_iter(),
        );
        let plan = FilterPlan::new(&filter, Affine::IDENTITY).unwrap();
        let out = PixelRect { x0: 0, y0: 0, x1: 20, y1: 20 };
        let images = plan.render(square(20, 20, 2..6, 2..6), out, None);
        assert_eq!(images.len(), 1);
        let image = &images[0].pixels;
        assert_eq!(px(image, 3, 3), [255, 255, 255, 255]);
        assert_eq!(px(image, 7, 3), [0, 0, 255, 255], "the first shadow");
        assert_eq!(px(image, 7, 7), [255, 0, 0, 255], "the second shadow, of the first's alpha too");
    }

    /// A blur of content rendered at a reduced resolution (each pixel the mean of a square of the
    /// full resolution's) and scaled back up bilinearly is within two levels of the full one.
    #[test]
    fn a_reduced_blur_matches_the_full_one() {
        for sigma in [8.0, 12.0, 20.0, 40.0] {
            // A rectangle with room for the blur around it, as a filter layer's region has.
            let margin = (3.0 * sigma) as usize + 16;
            let (w, h) = (120 + 2 * margin, 60 + 2 * margin);
            let plane: Vec<u8> = (0..w * h)
                .map(|i| {
                    let (x, y) = (i % w, i / w);
                    if (margin..margin + 120).contains(&x) && (margin..margin + 60).contains(&y) { 255 } else { 0 }
                })
                .collect();
            let mut full = plane.clone();
            blur(&mut full, &mut Vec::new(), w, h, 1, sigma);
            let factor = reduction(sigma);
            assert!(factor > 1);
            let (lw, lh) = (w.div_ceil(factor), h.div_ceil(factor));
            let mut low = vec![0u8; lw * lh];
            downsample(&plane, w, h, 1, factor, &mut low, lw);
            blur_reduced(&mut low, &mut Vec::new(), lw, lh, 1, sigma, factor);
            let f = factor as f64;
            let at = |x: isize, y: isize| -> f64 {
                f64::from(low[y.clamp(0, lh as isize - 1) as usize * lw + x.clamp(0, lw as isize - 1) as usize])
            };
            let mut worst = 0.0f64;
            for y in 0..h {
                for x in 0..w {
                    // The pixel centre in reduced pixels, less half a pixel: bilinear sampling.
                    let (u, v) = ((x as f64 + 0.5) / f - 0.5, (y as f64 + 0.5) / f - 0.5);
                    let (u0, v0) = (u.floor(), v.floor());
                    let (fu, fv) = (u - u0, v - v0);
                    let (u0, v0) = (u0 as isize, v0 as isize);
                    let value = (at(u0, v0) * (1.0 - fu) + at(u0 + 1, v0) * fu) * (1.0 - fv)
                        + (at(u0, v0 + 1) * (1.0 - fu) + at(u0 + 1, v0 + 1) * fu) * fv;
                    worst = worst.max((value - f64::from(full[y * w + x])).abs());
                }
            }
            assert!(worst <= 2.5, "sigma {sigma}: off by {worst}");
        }
    }

    /// Each `factor` by `factor` square of `src` (pixels past its edge transparent) averaged into a
    /// pixel of `dst` (`lw` wide).
    fn downsample(src: &[u8], w: usize, h: usize, channels: usize, factor: usize, dst: &mut [u8], lw: usize) {
        let area = (factor * factor) as u32;
        let mut sums = vec![0u32; lw * channels];
        for (ly, out) in dst.chunks_exact_mut(lw * channels).enumerate() {
            sums.fill(0);
            for y in ly * factor..((ly + 1) * factor).min(h) {
                let row = &src[y * w * channels..(y + 1) * w * channels];
                for (x, px) in row.chunks_exact(channels).enumerate() {
                    let at = x / factor * channels;
                    for c in 0..channels {
                        sums[at + c] += u32::from(px[c]);
                    }
                }
            }
            for (o, &s) in out.iter_mut().zip(&sums) {
                *o = ((s + area / 2) / area) as u8;
            }
        }
    }
}
