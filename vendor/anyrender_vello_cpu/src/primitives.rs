// Copyright 2020 the Resvg Authors (turbulence and the lighting normals, from resvg 0.48 `filter/`)
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! pyro: the pixel work of the SVG filter primitives the graph ([`crate::graph`]) does besides the
//! colour, blur, offset and compositing ones: `feDisplacementMap`, `feTile`, `feMorphology`,
//! `feConvolveMatrix`, `feTurbulence` and the two lighting primitives, on premultiplied RGBA
//! pixmaps of one region of device pixels. Each follows what Chromium's (Skia's) filter does where
//! the two differ from the spec's reference code: `feTurbulence` and the lighting normals are
//! resvg's ports of the spec's code (resvg is Apache-2.0 OR MIT, as this crate).

use vello_cpu::Pixmap;

use crate::filters::{PixelRect, div255};

/// A pixmap's size as `usize`s.
fn size(p: &Pixmap) -> (usize, usize) {
    (usize::from(p.width()), usize::from(p.height()))
}

/// The unpremultiplied channels of a premultiplied pixel (Skia's `SkUnPreMultiply`: rounded, 0
/// where it is transparent).
pub(crate) fn unpremultiply(px: [u8; 4]) -> [u8; 4] {
    let a = u32::from(px[3]);
    match a {
        0 => [0; 4],
        255 => px,
        _ => {
            let c = |v: u8| ((u32::from(v) * 255 + a / 2) / a).min(255) as u8;
            [c(px[0]), c(px[1]), c(px[2]), px[3]]
        }
    }
}

/// `feDisplacementMap`: each pixel of the result is the pixel of `src` its displacement lands on
/// (nearest, transparent beyond `src`), as Skia's displacement filter samples it. The
/// displacement is `scale * (C / 255 - 0.5)` of the map's unpremultiplied channels `x` and `y`
/// (0 red, 1 green, 2 blue, 3 alpha), in the filter's user space, which `m` (`[a, b, c, d]`, the
/// linear part of the transform to device pixels) maps to device pixels.
pub(crate) fn displace(src: &Pixmap, map: &Pixmap, m: [f64; 4], scale: f64, x: usize, y: usize) -> Pixmap {
    let (w, h) = size(src);
    let mut out = Pixmap::new(src.width(), src.height());
    let s = src.data_as_u8_slice();
    let d = out.data_as_u8_slice_mut();
    let shift = |v: u8| scale * (f64::from(v) / 255.0 - 0.5);
    for (i, px) in map.data_as_u8_slice().chunks_exact(4).enumerate() {
        let c = unpremultiply([px[0], px[1], px[2], px[3]]);
        let (ux, uy) = (shift(c[x]), shift(c[y]));
        let (dx, dy) = (m[0] * ux + m[2] * uy, m[1] * ux + m[3] * uy);
        let (px_x, px_y) = ((i % w) as f64, (i / w) as f64);
        // The nearest pixel to the displaced centre.
        let sx = (px_x + 0.5 + dx).floor();
        let sy = (px_y + 0.5 + dy).floor();
        if sx < 0.0 || sy < 0.0 || sx >= w as f64 || sy >= h as f64 {
            continue;
        }
        let j = (sy as usize * w + sx as usize) * 4;
        d[i * 4..i * 4 + 4].copy_from_slice(&s[j..j + 4]);
    }
    out
}

/// `feTile`: `rect` (pixels of `src`, the input's subregion) repeated over the whole pixmap, a
/// copy of it starting at its own position.
pub(crate) fn tile(src: &Pixmap, rect: PixelRect) -> Pixmap {
    let (w, h) = size(src);
    let mut out = Pixmap::new(src.width(), src.height());
    let (tw, th) = (i64::from(rect.width()), i64::from(rect.height()));
    if tw <= 0 || th <= 0 {
        return out;
    }
    let s = src.data_as_u8_slice();
    let d = out.data_as_u8_slice_mut();
    let columns: Vec<Option<usize>> = (0..w as i64)
        .map(|x| {
            let sx = i64::from(rect.x0) + (x - i64::from(rect.x0)).rem_euclid(tw);
            (0..w as i64).contains(&sx).then_some(sx as usize)
        })
        .collect();
    for y in 0..h as i64 {
        let sy = i64::from(rect.y0) + (y - i64::from(rect.y0)).rem_euclid(th);
        if !(0..h as i64).contains(&sy) {
            continue;
        }
        let (row, line) = (y as usize * w * 4, sy as usize * w * 4);
        for (x, sx) in columns.iter().enumerate() {
            if let Some(sx) = sx {
                d[row + x * 4..row + x * 4 + 4].copy_from_slice(&s[line + sx * 4..line + sx * 4 + 4]);
            }
        }
    }
    out
}

/// `feMorphology`: each channel's minimum (erode) or maximum (dilate) over the `2 rx + 1` by
/// `2 ry + 1` pixels around each pixel, the window cut to the pixmap (as Skia's morphology filter
/// cuts it to its input), in a pass along the rows then one along the columns.
pub(crate) fn morphology(pixels: &mut Pixmap, rx: usize, ry: usize, dilate: bool) {
    let (w, h) = size(pixels);
    if w == 0 || h == 0 {
        return;
    }
    let pick = |a: u8, b: u8| if dilate { a.max(b) } else { a.min(b) };
    let data = pixels.data_as_u8_slice_mut();
    let mut line = Vec::new();
    if rx > 0 {
        for row in data.chunks_exact_mut(w * 4) {
            line.clear();
            line.extend_from_slice(row);
            for x in 0..w {
                let (lo, hi) = (x.saturating_sub(rx), (x + rx).min(w - 1));
                for c in 0..4 {
                    let mut v = line[lo * 4 + c];
                    for k in lo + 1..=hi {
                        v = pick(v, line[k * 4 + c]);
                    }
                    row[x * 4 + c] = v;
                }
            }
        }
    }
    if ry > 0 {
        let source = data.to_vec();
        for y in 0..h {
            let (lo, hi) = (y.saturating_sub(ry), (y + ry).min(h - 1));
            let out = &mut data[y * w * 4..(y + 1) * w * 4];
            out.copy_from_slice(&source[lo * w * 4..(lo + 1) * w * 4]);
            for k in lo + 1..=hi {
                for (o, &v) in out.iter_mut().zip(&source[k * w * 4..(k + 1) * w * 4]) {
                    *o = pick(*o, v);
                }
            }
        }
    }
}

/// How `feConvolveMatrix` reads past the pixmap's edges.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Edge {
    /// The nearest edge pixel.
    Duplicate,
    /// The pixels of the opposite edge.
    Wrap,
    /// Transparent black.
    None,
}

/// An `feConvolveMatrix` in device pixels.
#[derive(Clone, Debug)]
pub(crate) struct Convolution {
    pub columns: usize,
    pub rows: usize,
    /// Row by row, as the element lists it.
    pub kernel: Vec<f32>,
    pub target_x: usize,
    pub target_y: usize,
    pub divisor: f32,
    pub bias: f32,
    pub edge: Edge,
    pub preserve_alpha: bool,
}

/// `feConvolveMatrix`, as Chromium has Skia's matrix convolution do it: the kernel turned half a
/// turn (the spec's formula), sums over bytes (0..255) times `1 / divisor` plus `bias * 255`,
/// clamped (the colour to the alpha) and rounded; with `preserveAlpha` the colours are convolved
/// unpremultiplied and the pixel keeps its alpha.
pub(crate) fn convolve(src: &Pixmap, k: &Convolution) -> Pixmap {
    let (w, h) = size(src);
    let mut out = Pixmap::new(src.width(), src.height());
    if w == 0 || h == 0 || k.kernel.len() != k.columns * k.rows || k.divisor == 0.0 {
        return out;
    }
    let pixels: Vec<[u8; 4]> = src
        .data_as_u8_slice()
        .chunks_exact(4)
        .map(|p| {
            let p = [p[0], p[1], p[2], p[3]];
            if k.preserve_alpha { unpremultiply(p) } else { p }
        })
        .collect();
    let gain = 1.0 / k.divisor;
    let bias = k.bias * 255.0;
    let at = |x: isize, y: isize| -> Option<[u8; 4]> {
        let (x, y) = match k.edge {
            Edge::None if x < 0 || y < 0 || x >= w as isize || y >= h as isize => return None,
            Edge::None => (x, y),
            Edge::Duplicate => (x.clamp(0, w as isize - 1), y.clamp(0, h as isize - 1)),
            Edge::Wrap => (x.rem_euclid(w as isize), y.rem_euclid(h as isize)),
        };
        Some(pixels[y as usize * w + x as usize])
    };
    let d = out.data_as_u8_slice_mut();
    for y in 0..h {
        for x in 0..w {
            let mut sums = [0.0f32; 4];
            for j in 0..k.rows {
                for i in 0..k.columns {
                    // The spec's kernel[orderX - i - 1][orderY - j - 1] at the source pixel
                    // (x - targetX + i, y - targetY + j).
                    let weight = k.kernel[(k.rows - 1 - j) * k.columns + (k.columns - 1 - i)];
                    if weight == 0.0 {
                        continue;
                    }
                    let sx = x as isize - k.target_x as isize + i as isize;
                    let sy = y as isize - k.target_y as isize + j as isize;
                    if let Some(p) = at(sx, sy) {
                        for c in 0..4 {
                            sums[c] += weight * f32::from(p[c]);
                        }
                    }
                }
            }
            // Computed in floats and rounded once, as Skia's convolution shader stores it (WPT
            // feconvolve-region: a bias of 0.5 over blue is 128, not 127).
            let value = |sum: f32, max: f32| (sum * gain + bias).clamp(0.0, max);
            let px = &mut d[(y * w + x) * 4..(y * w + x) * 4 + 4];
            if k.preserve_alpha {
                let a = pixels[y * w + x][3];
                for c in 0..3 {
                    px[c] = (value(sums[c], 255.0) * f32::from(a) / 255.0 + 0.5) as u8;
                }
                px[3] = a;
            } else {
                let a = value(sums[3], 255.0);
                for c in 0..3 {
                    px[c] = (value(sums[c], a) + 0.5) as u8;
                }
                px[3] = (a + 0.5) as u8;
            }
        }
    }
    out
}

/// `feTurbulence`'s parameters.
#[derive(Clone, Debug)]
pub(crate) struct Turbulence {
    pub base_frequency: (f64, f64),
    pub octaves: u32,
    pub seed: i32,
    pub fractal_noise: bool,
}

/// `feTurbulence` over a pixmap of `w` by `h`: the pixel `(x, y)` is the noise at the user-space
/// point `to_user` maps `(x, y)` to (the pixel's corner, as resvg evaluates it), the spec's
/// reference code (resvg's port). The result is premultiplied.
pub(crate) fn turbulence(w: u16, h: u16, t: &Turbulence, to_user: kurbo::Affine) -> Pixmap {
    let mut out = Pixmap::new(w, h);
    let (lattice, gradient) = noise::init(t.seed);
    let d = out.data_as_u8_slice_mut();
    for (i, px) in d.chunks_exact_mut(4).enumerate() {
        let (x, y) = ((i % usize::from(w)) as f64, (i / usize::from(w)) as f64);
        let p = to_user * kurbo::Point::new(x, y);
        let mut rgba = [0u8; 4];
        for (channel, v) in rgba.iter_mut().enumerate() {
            let n = noise::turbulence(channel, p.x, p.y, t, &lattice, &gradient);
            let n = if t.fractal_noise { (n * 255.0 + 255.0) / 2.0 } else { n * 255.0 };
            *v = (n.clamp(0.0, 255.0) + 0.5) as u8;
        }
        let a = u32::from(rgba[3]);
        for c in 0..3 {
            px[c] = div255(u32::from(rgba[c]) * a) as u8;
        }
        px[3] = rgba[3];
    }
    out
}

/// The spec's (and resvg's) noise.
mod noise {
    use super::Turbulence;

    const RAND_M: i32 = 2147483647; // 2**31 - 1
    const RAND_A: i32 = 16807; // 7**5; primitive root of m
    const RAND_Q: i32 = 127773; // m / a
    const RAND_R: i32 = 2836; // m % a
    const B_SIZE: usize = 0x100;
    const B_SIZE_32: i32 = 0x100;
    const B_LEN: usize = B_SIZE + B_SIZE + 2;
    const BM: i32 = 0xff;
    const PERLIN_N: i32 = 0x1000;

    pub(super) fn init(mut seed: i32) -> (Vec<usize>, Vec<Vec<[f64; 2]>>) {
        let mut lattice = vec![0; B_LEN];
        let mut gradient = vec![vec![[0.0; 2]; B_LEN]; 4];
        if seed <= 0 {
            seed = -(seed % (RAND_M - 1)) + 1;
        }
        if seed > RAND_M - 1 {
            seed = RAND_M - 1;
        }
        for g in gradient.iter_mut() {
            for (i, l) in lattice.iter_mut().enumerate().take(B_SIZE) {
                *l = i;
                for j in 0..2 {
                    seed = random(seed);
                    g[i][j] = f64::from((seed % (B_SIZE_32 + B_SIZE_32)) - B_SIZE_32) / f64::from(B_SIZE_32);
                }
                let s = (g[i][0] * g[i][0] + g[i][1] * g[i][1]).sqrt();
                if s != 0.0 {
                    g[i][0] /= s;
                    g[i][1] /= s;
                }
            }
        }
        for i in (1..B_SIZE).rev() {
            let k = lattice[i];
            seed = random(seed);
            let j = (seed % B_SIZE_32) as usize;
            lattice[i] = lattice[j];
            lattice[j] = k;
        }
        for i in 0..B_SIZE + 2 {
            lattice[B_SIZE + i] = lattice[i];
            for g in gradient.iter_mut() {
                g[B_SIZE + i] = g[i];
            }
        }
        (lattice, gradient)
    }

    fn random(seed: i32) -> i32 {
        let mut result = RAND_A * (seed % RAND_Q) - RAND_R * (seed / RAND_Q);
        if result <= 0 {
            result += RAND_M;
        }
        result
    }

    pub(super) fn turbulence(
        channel: usize,
        x: f64,
        y: f64,
        t: &Turbulence,
        lattice: &[usize],
        gradient: &[Vec<[f64; 2]>],
    ) -> f64 {
        let (mut x, mut y) = (x * t.base_frequency.0, y * t.base_frequency.1);
        let mut sum = 0.0;
        let mut ratio = 1.0;
        for _ in 0..t.octaves {
            let n = noise2(channel, x, y, lattice, gradient);
            sum += if t.fractal_noise { n } else { n.abs() } / ratio;
            x *= 2.0;
            y *= 2.0;
            ratio *= 2.0;
        }
        sum
    }

    fn noise2(channel: usize, x: f64, y: f64, lattice: &[usize], gradient: &[Vec<[f64; 2]>]) -> f64 {
        let t = x + f64::from(PERLIN_N);
        let bx0 = (t as i32) & BM;
        let bx1 = (bx0 + 1) & BM;
        let rx0 = t - t as i64 as f64;
        let rx1 = rx0 - 1.0;
        let t = y + f64::from(PERLIN_N);
        let by0 = (t as i32) & BM;
        let by1 = (by0 + 1) & BM;
        let ry0 = t - t as i64 as f64;
        let ry1 = ry0 - 1.0;
        let i = lattice[bx0 as usize];
        let j = lattice[bx1 as usize];
        let b00 = lattice[i + by0 as usize];
        let b10 = lattice[j + by0 as usize];
        let b01 = lattice[i + by1 as usize];
        let b11 = lattice[j + by1 as usize];
        let sx = s_curve(rx0);
        let sy = s_curve(ry0);
        let g = &gradient[channel];
        let u = rx0 * g[b00][0] + ry0 * g[b00][1];
        let v = rx1 * g[b10][0] + ry0 * g[b10][1];
        let a = lerp(sx, u, v);
        let u = rx0 * g[b01][0] + ry1 * g[b01][1];
        let v = rx1 * g[b11][0] + ry1 * g[b11][1];
        let b = lerp(sx, u, v);
        lerp(sy, a, b)
    }

    fn s_curve(t: f64) -> f64 {
        t * t * (3.0 - 2.0 * t)
    }

    fn lerp(t: f64, a: f64, b: f64) -> f64 {
        a + t * (b - a)
    }
}

/// A light source in device pixels of the pixmap lit (`z` scaled as the plane is).
#[derive(Clone, Debug)]
pub(crate) enum Light {
    /// The unit vector towards the light.
    Distant([f64; 3]),
    Point([f64; 3]),
    Spot {
        at: [f64; 3],
        /// The unit vector the light points in.
        direction: [f64; 3],
        exponent: f64,
        /// The cosine of the limiting cone's angle, if it has one.
        cone: Option<f64>,
    },
}

/// `feDiffuseLighting` or `feSpecularLighting` in device pixels.
#[derive(Clone, Debug)]
pub(crate) struct Lighting {
    pub light: Light,
    pub surface_scale: f64,
    /// `kd` or `ks`.
    pub constant: f64,
    /// `None` for diffuse lighting, else the specular exponent.
    pub specular_exponent: Option<f64>,
    /// The lighting colour (bytes, in the primitive's colour space).
    pub color: [u8; 3],
}

/// The lighting primitives over the alpha of `src` as a height map: the surface normal from Sobel
/// kernels (the spec's, its edge ones at the pixmap's edges; resvg's port), the light's colour times
/// `N.L` (diffuse, opaque) or `N.H ^ exponent` (specular, its alpha the largest of its colours).
pub(crate) fn lighting(src: &Pixmap, l: &Lighting) -> Pixmap {
    let (w, h) = size(src);
    let mut out = Pixmap::new(src.width(), src.height());
    if w == 0 || h == 0 {
        return out;
    }
    let alpha: Vec<i32> = src.data_as_u8_slice().chunks_exact(4).map(|p| i32::from(p[3])).collect();
    let a = |x: usize, y: usize| alpha[y * w + x];
    let d = out.data_as_u8_slice_mut();
    for y in 0..h {
        for x in 0..w {
            let ((fx, fy), (nx, ny)) = normal(&a, x, y, w, h);
            let surface =
                [-(nx as f64) * l.surface_scale / 255.0 * fx, -(ny as f64) * l.surface_scale / 255.0 * fy, 1.0];
            let z = f64::from(a(x, y)) / 255.0 * l.surface_scale;
            let to_light = match &l.light {
                Light::Distant(v) => *v,
                Light::Point(at) | Light::Spot { at, .. } => {
                    normalized([at[0] - x as f64, at[1] - y as f64, at[2] - z])
                }
            };
            let color = light_color(l, to_light);
            let n = normalized(surface);
            let factor = match l.specular_exponent {
                None => l.constant * dot(n, to_light),
                Some(exponent) => {
                    let half = normalized([to_light[0], to_light[1], to_light[2] + 1.0]);
                    let n_dot_h = dot(n, half);
                    l.constant * if exponent == 1.0 { n_dot_h } else { n_dot_h.max(0.0).powf(exponent) }
                }
            };
            let c = |v: f64| (v * factor).clamp(0.0, 255.0).round() as u8;
            let rgb = [c(color[0]), c(color[1]), c(color[2])];
            let px = &mut d[(y * w + x) * 4..(y * w + x) * 4 + 4];
            match l.specular_exponent {
                None => px.copy_from_slice(&[rgb[0], rgb[1], rgb[2], 255]),
                Some(_) => px.copy_from_slice(&[rgb[0], rgb[1], rgb[2], rgb[0].max(rgb[1]).max(rgb[2])]),
            }
        }
    }
    out
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn normalized(v: [f64; 3]) -> [f64; 3] {
    let length = dot(v, v).sqrt();
    if length == 0.0 { v } else { [v[0] / length, v[1] / length, v[2] / length] }
}

/// The light's colour reaching a point in the direction `to_light` (unit) from it: a spot light's
/// falls off with the angle to its direction, and past its cone (smoothly over 0.016 of the cosine,
/// as Skia's spot light) it is black.
fn light_color(l: &Lighting, to_light: [f64; 3]) -> [f64; 3] {
    let color = l.color.map(f64::from);
    let Light::Spot { direction, exponent, cone, .. } = &l.light else {
        return color;
    };
    let cos = -dot(to_light, *direction);
    if cos <= 0.0 {
        return [0.0; 3];
    }
    const ANTI_ALIAS: f64 = 0.016;
    let mut scale = cos.powf(*exponent);
    if let Some(outer) = *cone {
        if cos < outer {
            return [0.0; 3];
        }
        if cos < outer + ANTI_ALIAS {
            scale *= (cos - outer) / ANTI_ALIAS;
        }
    }
    color.map(|c| c * scale)
}

/// The Sobel normal at `(x, y)` of a `w` by `h` height map: its factors and its two sums (the
/// spec's kernels, its edge and corner ones where the map ends). A map of fewer than two pixels
/// along a side is flat along it.
fn normal(a: &impl Fn(usize, usize) -> i32, x: usize, y: usize, w: usize, h: usize) -> ((f64, f64), (i32, i32)) {
    if w < 2 || h < 2 {
        return ((0.0, 0.0), (0, 0));
    }
    // The kernels at the edges are the interior one with the missing row or column replaced by
    // the centre's, and factors that make up for it.
    let (left, right) = (x > 0, x + 1 < w);
    let (top, bottom) = (y > 0, y + 1 < h);
    let at = |dx: isize, dy: isize| a((x as isize + dx) as usize, (y as isize + dy) as usize);
    // Horizontal: the column right minus the column left (the centre where one is missing),
    // weighted 1, 2, 1 down the rows present.
    let column = |dx: isize| {
        let mut sum = 2 * at(dx, 0);
        if top {
            sum += at(dx, -1);
        }
        if bottom {
            sum += at(dx, 1);
        }
        sum
    };
    let row = |dy: isize| {
        let mut sum = 2 * at(0, dy);
        if left {
            sum += at(-1, dy);
        }
        if right {
            sum += at(1, dy);
        }
        sum
    };
    let nx = column(if right { 1 } else { 0 }) - column(if left { -1 } else { 0 });
    let ny = row(if bottom { 1 } else { 0 }) - row(if top { -1 } else { 0 });
    // The spec's factors: 1/4 inside, 1/3 along an edge (2/3 at a corner) across the edge's
    // direction, 1/2 along it.
    let sobel = |both: bool, other_both: bool| match (both, other_both) {
        (true, true) => 1.0 / 4.0,
        (true, false) => 1.0 / 3.0,
        (false, true) => 1.0 / 2.0,
        (false, false) => 2.0 / 3.0,
    };
    ((sobel(left && right, top && bottom), sobel(top && bottom, left && right)), (nx, ny))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixmap(w: u16, h: u16, f: impl Fn(usize, usize) -> [u8; 4]) -> Pixmap {
        let mut p = Pixmap::new(w, h);
        for (i, px) in p.data_as_u8_slice_mut().chunks_exact_mut(4).enumerate() {
            px.copy_from_slice(&f(i % usize::from(w), i / usize::from(w)));
        }
        p
    }

    fn at(p: &Pixmap, x: usize, y: usize) -> [u8; 4] {
        let i = (y * usize::from(p.width()) + x) * 4;
        p.data_as_u8_slice()[i..i + 4].try_into().unwrap()
    }

    /// A map of green 255, blue 128 at scale 10 moves each pixel by (+5, ~0): the pixel 5 right.
    #[test]
    fn displacement_reads_the_pixel_the_map_points_at() {
        let src = pixmap(10, 2, |x, _| [x as u8, 0, 0, 255]);
        let map = pixmap(10, 2, |_, _| [0, 255, 128, 255]);
        let out = displace(&src, &map, [1.0, 0.0, 0.0, 1.0], 10.0, 1, 2);
        assert_eq!(at(&out, 0, 0), [5, 0, 0, 255]);
        assert_eq!(at(&out, 4, 1), [9, 0, 0, 255]);
        assert_eq!(at(&out, 5, 0), [0; 4], "past the source: transparent");
        // A mirrored user space moves the other way.
        let out = displace(&src, &map, [-1.0, 0.0, 0.0, 1.0], 10.0, 1, 2);
        assert_eq!(at(&out, 9, 0), [4, 0, 0, 255]);
    }

    #[test]
    fn a_tile_repeats_its_rectangle_from_its_own_position() {
        let src = pixmap(6, 1, |x, _| [x as u8, 0, 0, 255]);
        let out = tile(&src, PixelRect { x0: 2, y0: 0, x1: 4, y1: 1 });
        let row: Vec<u8> = (0..6).map(|x| at(&out, x, 0)[0]).collect();
        assert_eq!(row, [2, 3, 2, 3, 2, 3]);
    }

    #[test]
    fn dilation_and_erosion_by_a_radius() {
        let mut p = pixmap(7, 7, |x, y| if (x, y) == (3, 3) { [0, 0, 255, 255] } else { [0; 4] });
        morphology(&mut p, 1, 2, true);
        assert_eq!(at(&p, 4, 5), [0, 0, 255, 255]);
        assert_eq!(at(&p, 5, 3), [0; 4]);
        morphology(&mut p, 1, 1, false);
        assert_eq!(at(&p, 3, 3), [0, 0, 255, 255]);
        assert_eq!(at(&p, 4, 3), [0; 4]);
    }

    /// WPT feconvolve-region: a box of nine ones over blue with a bias of 0.5: blue plus half grey
    /// inside, half-transparent white (premultiplied 128s) outside.
    #[test]
    fn a_convolution_with_a_bias() {
        let src =
            pixmap(9, 9, |x, y| if (3..6).contains(&x) && (3..6).contains(&y) { [0, 0, 255, 255] } else { [0; 4] });
        let k = Convolution {
            columns: 3,
            rows: 3,
            kernel: vec![1.0; 9],
            target_x: 1,
            target_y: 1,
            divisor: 9.0,
            bias: 0.5,
            edge: Edge::Duplicate,
            preserve_alpha: false,
        };
        let out = convolve(&src, &k);
        assert_eq!(at(&out, 4, 4), [128, 128, 255, 255]);
        assert_eq!(at(&out, 0, 0), [128, 128, 128, 128]);
    }

    /// A flat surface under a light straight above: the lighting colour itself.
    #[test]
    fn a_flat_surface_lit_from_above() {
        let src = pixmap(4, 4, |_, _| [0, 0, 0, 255]);
        let mut l = Lighting {
            light: Light::Distant([0.0, 0.0, 1.0]),
            surface_scale: 1.0,
            constant: 1.0,
            specular_exponent: None,
            color: [0, 255, 128],
        };
        assert_eq!(at(&lighting(&src, &l), 0, 3), [0, 255, 128, 255]);
        l.specular_exponent = Some(1.0);
        assert_eq!(at(&lighting(&src, &l), 2, 2), [0, 255, 128, 255]);
    }

    /// Turbulence of no frequency is transparent; fractal noise of none is half grey.
    #[test]
    fn noise_of_no_frequency() {
        let mut t = Turbulence { base_frequency: (0.0, 0.0), octaves: 1, seed: 0, fractal_noise: false };
        assert_eq!(at(&turbulence(2, 2, &t, kurbo::Affine::IDENTITY), 1, 1), [0; 4]);
        t.fractal_noise = true;
        assert_eq!(at(&turbulence(2, 2, &t, kurbo::Affine::IDENTITY), 1, 1), [64, 64, 64, 128]);
    }
}
