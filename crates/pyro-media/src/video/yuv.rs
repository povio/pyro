//! Y'CbCr planes to straight RGBA, as a browser's compositor converts a
//! decoded frame: the stream's matrix (BT.601, BT.709 or BT.2020) and range,
//! chroma upsampled by nearest sample.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Matrix {
    Bt601,
    Bt709,
    Bt2020,
}

impl Matrix {
    /// (Kr, Kb).
    fn coefficients(self) -> (f32, f32) {
        match self {
            Matrix::Bt601 => (0.299, 0.114),
            Matrix::Bt709 => (0.2126, 0.0722),
            Matrix::Bt2020 => (0.2627, 0.0593),
        }
    }
    /// Chrome's choice for a stream that does not say: BT.709 for HD, else BT.601.
    pub fn untagged(height: u32) -> Matrix {
        if height >= 720 { Matrix::Bt709 } else { Matrix::Bt601 }
    }
}

/// One plane: its samples (8-bit, or little-endian 16-bit when `wide`) and
/// its row stride in bytes.
pub struct Plane<'a> {
    pub data: &'a [u8],
    pub stride: usize,
}

pub struct Picture<'a> {
    pub width: usize,
    pub height: usize,
    pub y: Plane<'a>,
    /// Absent for monochrome.
    pub uv: Option<(Plane<'a>, Plane<'a>)>,
    /// Chroma subsampling shifts (1, 1 for 4:2:0).
    pub ss: (u32, u32),
    /// Bits per sample (8, 10 or 12); above 8 samples are 16-bit.
    pub depth: u32,
    pub full_range: bool,
    pub matrix: Matrix,
}

#[derive(Clone, Copy)]
pub enum OutputTransfer {
    Preserve,
    Bt709ToSrgb,
}

// Map nonlinear BT.709 R'G'B' through linear light to the sRGB transfer
// curve. Keep 12-bit precision through the YUV matrix and quantize once
// at the output; the per-frame conversion uses only lookups.
static BT709_TO_SRGB: std::sync::LazyLock<[u8; 4096]> = std::sync::LazyLock::new(|| {
    std::array::from_fn(|i| {
        let signal = i as f64 / 4095.;
        let linear = if signal < 0.081 { signal / 4.5 } else { ((signal + 0.099) / 1.099).powf(1. / 0.45) };
        let srgb = if linear <= 0.0031308 { 12.92 * linear } else { 1.055 * linear.powf(1. / 2.4) - 0.055 };
        (srgb * 255.).round().clamp(0., 255.) as u8
    })
});

/// Straight RGBA rows (opaque).
pub fn to_rgba(p: &Picture<'_>, transfer: OutputTransfer) -> Vec<u8> {
    let (kr, kb) = p.matrix.coefficients();
    let kg = 1.0 - kr - kb;
    let max = ((1u32 << p.depth) - 1) as f32;
    let scale = 255.0 / max;
    let lut = match transfer {
        OutputTransfer::Preserve => None,
        OutputTransfer::Bt709ToSrgb => Some(&*BT709_TO_SRGB),
    };
    let encode = |sample: f32| match lut {
        Some(lut) => lut[(sample * (4095. / max)).round().clamp(0., 4095.) as usize],
        None => (sample * scale).round().clamp(0., 255.) as u8,
    };
    let shift = p.depth.saturating_sub(8) as i32;
    let (y_off, y_scale, c_scale) = if p.full_range {
        (0.0, 1.0, 1.0)
    } else {
        let unit = (1 << shift) as f32;
        (16.0 * unit, 255.0 / 219.0, 255.0 / 224.0)
    };
    let half = (128 << shift) as f32;
    let wide = p.depth > 8;
    let read = |plane: &Plane<'_>, x: usize, y: usize| -> f32 {
        if wide {
            let at = y * plane.stride + x * 2;
            plane.data.get(at..at + 2).map_or(0.0, |b| f32::from(u16::from_le_bytes([b[0], b[1]])))
        } else {
            plane.data.get(y * plane.stride + x).map_or(0.0, |&b| f32::from(b))
        }
    };
    let mut out = vec![255u8; p.width * p.height * 4];
    for row in 0..p.height {
        let line = &mut out[row * p.width * 4..(row + 1) * p.width * 4];
        for col in 0..p.width {
            let luma = (read(&p.y, col, row) - y_off) * y_scale;
            let (cb, cr) = match &p.uv {
                Some((u, v)) => {
                    let (cx, cy) = (col >> p.ss.0, row >> p.ss.1);
                    ((read(u, cx, cy) - half) * c_scale, (read(v, cx, cy) - half) * c_scale)
                }
                None => (0.0, 0.0),
            };
            let r = luma + 2.0 * (1.0 - kr) * cr;
            let b = luma + 2.0 * (1.0 - kb) * cb;
            let g = (luma - kr * r - kb * b) / kg;
            let px = &mut line[col * 4..col * 4 + 3];
            px[0] = encode(r);
            px[1] = encode(g);
            px[2] = encode(b);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packet_bt709_gray_levels_are_encoded_as_srgb() {
        let picture = Picture {
            width: 4,
            height: 1,
            y: Plane { data: &[16, 38, 126, 235], stride: 4 },
            uv: None,
            ss: (1, 1),
            depth: 8,
            full_range: false,
            matrix: Matrix::Bt709,
        };
        let gray = |transfer| to_rgba(&picture, transfer).chunks_exact(4).map(|pixel| pixel[0]).collect::<Vec<_>>();
        assert_eq!(gray(OutputTransfer::Preserve), [0, 26, 128, 255]);
        assert_eq!(gray(OutputTransfer::Bt709ToSrgb), [0, 41, 140, 255]);
    }
}
