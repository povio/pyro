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
        if height >= 720 {
            Matrix::Bt709
        } else {
            Matrix::Bt601
        }
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

/// Straight RGBA rows (opaque).
pub fn to_rgba(p: &Picture<'_>) -> Vec<u8> {
    let (kr, kb) = p.matrix.coefficients();
    let kg = 1.0 - kr - kb;
    let max = ((1u32 << p.depth) - 1) as f32;
    let scale = 255.0 / max;
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
            px[0] = (r * scale).round().clamp(0.0, 255.0) as u8;
            px[1] = (g * scale).round().clamp(0.0, 255.0) as u8;
            px[2] = (b * scale).round().clamp(0.0, 255.0) as u8;
        }
    }
    out
}
