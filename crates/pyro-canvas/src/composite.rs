//! Per-pixel compositing of a rendered source image onto the bitmap (HTML
//! canvas "drawing model"): the global alpha, the operator (Porter-Duff, or
//! source-over with a W3C blend mode) and the clipping region's coverage.
//! Pixels are premultiplied RGBA8.

use crate::{Composite, Shadow};

#[inline]
fn unit(v: u8) -> f32 {
    f32::from(v) * (1.0 / 255.0)
}

#[inline]
fn byte(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

/// Whether a transparent source pixel leaves the destination as it is.
/// `clear` is bounded by the source's shape, as Chromium does: the bitmap
/// outside what is drawn keeps its pixels.
pub(crate) fn bounded(op: Composite) -> bool {
    !matches!(
        op,
        Composite::SourceIn
            | Composite::SourceOut
            | Composite::DestinationIn
            | Composite::DestinationAtop
            | Composite::Copy
    )
}

/// Composites `src` onto `dst` (same size, premultiplied) with `alpha`, `op`
/// and the clip `mask` (coverage per pixel).
pub(crate) fn apply(dst: &mut [u8], src: &[u8], alpha: f32, op: Composite, mask: Option<&[u8]>) {
    let skip_transparent = bounded(op);
    for (i, (d, s)) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)).enumerate() {
        let m = mask.map_or(255, |m| m[i]);
        if m == 0 || skip_transparent && s[3] == 0 {
            continue;
        }
        let s = [unit(s[0]) * alpha, unit(s[1]) * alpha, unit(s[2]) * alpha, unit(s[3]) * alpha];
        let dv = [unit(d[0]), unit(d[1]), unit(d[2]), unit(d[3])];
        let out = pixel(s, dv, op);
        if m == 255 {
            for k in 0..4 {
                d[k] = byte(out[k]);
            }
        } else {
            let m = unit(m);
            for k in 0..4 {
                d[k] = byte(dv[k] + (out[k] - dv[k]) * m);
            }
        }
    }
}

/// `clearRect`: `dst` loses the rectangle's `coverage` within the clip.
pub(crate) fn clear(dst: &mut [u8], coverage: &[u8], mask: Option<&[u8]>) {
    for (i, (d, c)) in dst.chunks_exact_mut(4).zip(coverage).enumerate() {
        let c = u32::from(*c) * u32::from(mask.map_or(255, |m| m[i])) / 255;
        match c {
            0 => {}
            255 => d.fill(0),
            c => {
                for v in d.iter_mut() {
                    *v = ((u32::from(*v) * (255 - c) + 127) / 255) as u8;
                }
            }
        }
    }
}

/// One premultiplied pixel `s` (alpha applied) over `d` with `op`.
fn pixel(s: [f32; 4], d: [f32; 4], op: Composite) -> [f32; 4] {
    let (sa, da) = (s[3], d[3]);
    let porter_duff =
        |fa: f32, fb: f32| [s[0] * fa + d[0] * fb, s[1] * fa + d[1] * fb, s[2] * fa + d[2] * fb, sa * fa + da * fb];
    match op {
        Composite::SourceOver => porter_duff(1.0, 1.0 - sa),
        Composite::SourceIn => porter_duff(da, 0.0),
        Composite::SourceOut => porter_duff(1.0 - da, 0.0),
        Composite::SourceAtop => porter_duff(da, 1.0 - sa),
        Composite::DestinationOver => porter_duff(1.0 - da, 1.0),
        Composite::DestinationIn => porter_duff(0.0, sa),
        Composite::DestinationOut => porter_duff(0.0, 1.0 - sa),
        Composite::DestinationAtop => porter_duff(1.0 - da, sa),
        Composite::Copy => s,
        Composite::Clear => [0.0; 4],
        Composite::Xor => porter_duff(1.0 - da, 1.0 - sa),
        Composite::Lighter => {
            [(s[0] + d[0]).min(1.0), (s[1] + d[1]).min(1.0), (s[2] + d[2]).min(1.0), (sa + da).min(1.0)]
        }
        _ => blend(s, d, op),
    }
}

/// Source-over with a blend mode (W3C Compositing and Blending §5):
/// `co = cs (1 - ad) + cd (1 - as) + as ad B(Cd, Cs)`.
fn blend(s: [f32; 4], d: [f32; 4], op: Composite) -> [f32; 4] {
    let (sa, da) = (s[3], d[3]);
    let straight = |p: [f32; 4]| {
        if p[3] > 0.0 {
            [p[0] / p[3], p[1] / p[3], p[2] / p[3]]
        } else {
            [0.0; 3]
        }
    };
    let (cs, cd) = (straight(s), straight(d));
    let mixed: [f32; 3] = match op {
        Composite::Hue => set_lum(set_sat(cs, sat(cd)), lum(cd)),
        Composite::Saturation => set_lum(set_sat(cd, sat(cs)), lum(cd)),
        Composite::Color => set_lum(cs, lum(cd)),
        Composite::Luminosity => set_lum(cd, lum(cs)),
        _ => [0, 1, 2].map(|k| separable(cd[k], cs[k], op)),
    };
    let mut out = [0.0; 4];
    for k in 0..3 {
        out[k] = s[k] * (1.0 - da) + d[k] * (1.0 - sa) + sa * da * mixed[k];
    }
    out[3] = sa + da * (1.0 - sa);
    out
}

fn separable(b: f32, s: f32, op: Composite) -> f32 {
    match op {
        Composite::Multiply => b * s,
        Composite::Screen => b + s - b * s,
        Composite::Overlay => hard_light(s, b),
        Composite::Darken => b.min(s),
        Composite::Lighten => b.max(s),
        Composite::ColorDodge => {
            if b == 0.0 {
                0.0
            } else if s >= 1.0 {
                1.0
            } else {
                (b / (1.0 - s)).min(1.0)
            }
        }
        Composite::ColorBurn => {
            if b >= 1.0 {
                1.0
            } else if s <= 0.0 {
                0.0
            } else {
                1.0 - ((1.0 - b) / s).min(1.0)
            }
        }
        Composite::HardLight => hard_light(b, s),
        Composite::SoftLight => {
            if s <= 0.5 {
                b - (1.0 - 2.0 * s) * b * (1.0 - b)
            } else {
                let d = if b <= 0.25 { ((16.0 * b - 12.0) * b + 4.0) * b } else { b.sqrt() };
                b + (2.0 * s - 1.0) * (d - b)
            }
        }
        Composite::Difference => (b - s).abs(),
        Composite::Exclusion => b + s - 2.0 * b * s,
        _ => s,
    }
}

fn hard_light(b: f32, s: f32) -> f32 {
    if s <= 0.5 {
        b * 2.0 * s
    } else {
        let s = 2.0 * s - 1.0;
        b + s - b * s
    }
}

fn lum(c: [f32; 3]) -> f32 {
    0.3 * c[0] + 0.59 * c[1] + 0.11 * c[2]
}

fn clip_color(c: [f32; 3]) -> [f32; 3] {
    let l = lum(c);
    let n = c[0].min(c[1]).min(c[2]);
    let x = c[0].max(c[1]).max(c[2]);
    let mut c = c;
    if n < 0.0 {
        c = c.map(|v| l + (v - l) * l / (l - n));
    }
    if x > 1.0 {
        c = c.map(|v| l + (v - l) * (1.0 - l) / (x - l));
    }
    c
}

fn set_lum(c: [f32; 3], l: f32) -> [f32; 3] {
    let d = l - lum(c);
    clip_color(c.map(|v| v + d))
}

fn sat(c: [f32; 3]) -> f32 {
    c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2])
}

fn set_sat(c: [f32; 3], s: f32) -> [f32; 3] {
    let max = c[0].max(c[1]).max(c[2]);
    let min = c[0].min(c[1]).min(c[2]);
    if max > min {
        c.map(|v| (v - min) * s / (max - min))
    } else {
        [0.0; 3]
    }
}

/// A shadow of the premultiplied `src` (`w` by `h`): its alpha, offset,
/// blurred (Gaussian with sigma blur/2, as three box blurs) and painted with
/// the shadow color. Premultiplied.
pub(crate) fn shadow(src: &[u8], w: usize, h: usize, shadow: &Shadow) -> Vec<u8> {
    // An offset past the bitmap moves the shadow off it; clamped so the
    // arithmetic below cannot overflow.
    let (wf, hf) = (w as f64 + 1.0, h as f64 + 1.0);
    let ox = shadow.offset.x.round().clamp(-wf, wf) as i64;
    let oy = shadow.offset.y.round().clamp(-hf, hf) as i64;
    let mut alpha = vec![0f32; w * h];
    for y in 0..h as i64 {
        let sy = y - oy;
        if sy < 0 || sy >= h as i64 {
            continue;
        }
        for x in 0..w as i64 {
            let sx = x - ox;
            if sx < 0 || sx >= w as i64 {
                continue;
            }
            alpha[(y as usize) * w + x as usize] = unit(src[((sy as usize) * w + sx as usize) * 4 + 3]);
        }
    }
    let sigma = shadow.blur / 2.0;
    if sigma > 0.0 {
        // Box sizes approximating a Gaussian (W3C filter effects §feGaussianBlur).
        let d = ((sigma * 3.0 * (2.0 * std::f64::consts::PI).sqrt() / 4.0) + 0.5).floor().max(1.0) as usize;
        // Three passes; bounded so a huge blur costs what a full-size one does.
        let d = d.min(w.max(h) * 2);
        for _ in 0..3 {
            box_blur(&mut alpha, w, h, d, true);
            box_blur(&mut alpha, w, h, d, false);
        }
    }
    let c = shadow.color.premultiply().components;
    let mut out = vec![0u8; w * h * 4];
    for (o, a) in out.chunks_exact_mut(4).zip(&alpha) {
        if *a > 0.0 {
            o[0] = byte(c[0] * a);
            o[1] = byte(c[1] * a);
            o[2] = byte(c[2] * a);
            o[3] = byte(c[3] * a);
        }
    }
    out
}

/// A box blur of width `d` along rows (`horizontal`) or columns, with a
/// running sum: O(pixels) whatever `d`.
fn box_blur(data: &mut [f32], w: usize, h: usize, d: usize, horizontal: bool) {
    let (lines, len, stride, step) = if horizontal { (h, w, w, 1) } else { (w, h, 1, w) };
    let before = d / 2;
    let after = d - before - 1;
    let mut line = vec![0f32; len];
    for l in 0..lines {
        let base = l * stride;
        for (i, v) in line.iter_mut().enumerate() {
            *v = data[base + i * step];
        }
        let mut sum = 0f32;
        // Window [i - before, i + after].
        for v in line.iter().take(after.min(len)) {
            sum += *v;
        }
        for i in 0..len {
            if i + after < len {
                sum += line[i + after];
            }
            if i > before {
                sum -= line[i - before - 1];
            }
            data[base + i * step] = sum / d as f32;
        }
    }
}
