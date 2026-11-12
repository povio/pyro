//! PNG/JPEG encoding, PNG decoding and pixel comparison.

use crate::RgbaImage;

/// PNG at `Compression::Fast` with the `Sub` row filter: encoding is the
/// largest cost of a classic screenshot after layout, and the `png` crate's
/// default `Adaptive` filter tries all five filters per row (three quarters
/// of the encode) for a few percent of size on rendered pages.
pub fn encode_png(img: &RgbaImage) -> Vec<u8> {
    let mut out = Vec::with_capacity(img.data.len() / 8);
    let mut enc = png::Encoder::new(&mut out, img.width, img.height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(png::Compression::Fast);
    enc.set_filter(png::Filter::Sub);
    // In-memory writes of a well-formed image cannot fail.
    let mut w = enc.write_header().expect("png header");
    w.write_image_data(&img.data).expect("png data");
    w.finish().expect("png finish");
    out
}

/// Baseline JPEG at `quality` (1-100, libjpeg's scale), alpha dropped, no
/// chroma subsampling (as the `image` crate's encoder it replaces).
pub fn encode_jpeg(img: &RgbaImage, quality: u8) -> Result<Vec<u8>, String> {
    let (Ok(width), Ok(height)) = (u16::try_from(img.width), u16::try_from(img.height)) else {
        return Err(format!("{}x{} exceeds the JPEG limit of 65535", img.width, img.height));
    };
    let mut out = Vec::with_capacity(img.data.len() / 16);
    let mut enc = jpeg_encoder::Encoder::new(&mut out, quality.clamp(1, 100));
    enc.set_sampling_factor(jpeg_encoder::SamplingFactor::R_4_4_4);
    enc.encode(&img.data, width, height, jpeg_encoder::ColorType::Rgba).map_err(|e| e.to_string())?;
    Ok(out)
}

/// Any PNG (palette, gray, 16-bit) to RGBA8.
pub fn decode_png(bytes: &[u8]) -> Result<RgbaImage, String> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
    dec.set_transformations(
        png::Transformations::EXPAND | png::Transformations::ALPHA | png::Transformations::STRIP_16,
    );
    let mut reader = dec.read_info().map_err(|e| format!("not a PNG: {e}"))?;
    let size = reader.output_buffer_size().ok_or("PNG too large")?;
    let mut data = vec![0; size];
    let info = reader.next_frame(&mut data).map_err(|e| format!("bad PNG: {e}"))?;
    data.truncate(info.buffer_size());
    let data = match info.color_type {
        png::ColorType::Rgba => data,
        png::ColorType::GrayscaleAlpha => data.chunks_exact(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        other => return Err(format!("unsupported PNG color type {other:?}")),
    };
    Ok(RgbaImage { width: info.width, height: info.height, data })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompareResult {
    /// No pixel differs by more than the tolerance (sizes equal).
    pub equal: bool,
    /// Pixels whose largest per-channel difference exceeds the tolerance.
    /// With `stop_after`, counting stops past that many. A size mismatch
    /// counts every pixel of the larger image.
    pub diff_pixels: u64,
}

/// Compares `a` with `b`. `tolerance`: largest allowed per-channel absolute
/// difference (0 = exact). `stop_after`: stop counting once more than this
/// many pixels differ (the verdict is known).
pub fn compare(a: &RgbaImage, b: &RgbaImage, tolerance: u8, stop_after: Option<u64>) -> CompareResult {
    if a.width != b.width || a.height != b.height {
        let n = (a.width as u64 * a.height as u64).max(b.width as u64 * b.height as u64);
        return CompareResult { equal: false, diff_pixels: n };
    }
    if a.data == b.data {
        return CompareResult { equal: true, diff_pixels: 0 };
    }
    let mut diff = 0u64;
    // 16 pixels at a time with a byte compare; per pixel only on a mismatch.
    for (ca, cb) in a.data.chunks(64).zip(b.data.chunks(64)) {
        if ca == cb {
            continue;
        }
        for (pa, pb) in ca.chunks_exact(4).zip(cb.chunks_exact(4)) {
            if channel_diff(pa, pb) > tolerance {
                diff += 1;
                if stop_after.is_some_and(|n| diff > n) {
                    return CompareResult { equal: false, diff_pixels: diff };
                }
            }
        }
    }
    CompareResult { equal: diff == 0, diff_pixels: diff }
}

fn channel_diff(a: &[u8], b: &[u8]) -> u8 {
    a.iter().zip(b).map(|(x, y)| x.abs_diff(*y)).max().unwrap_or(0)
}

/// Differing pixels red over a faded gray copy of `actual`. Sizes may
/// differ: the image covers both, area outside one of them counts as
/// differing.
pub fn diff_image(actual: &RgbaImage, expected: &RgbaImage, tolerance: u8) -> RgbaImage {
    let (w, h) = (actual.width.max(expected.width), actual.height.max(expected.height));
    let px = |img: &RgbaImage, x: u32, y: u32| -> Option<[u8; 4]> {
        (x < img.width && y < img.height).then(|| {
            let i = (y as usize * img.width as usize + x as usize) * 4;
            [img.data[i], img.data[i + 1], img.data[i + 2], img.data[i + 3]]
        })
    };
    let mut data = Vec::with_capacity(w as usize * h as usize * 4);
    for y in 0..h {
        for x in 0..w {
            match (px(actual, x, y), px(expected, x, y)) {
                (Some(a), Some(b)) if channel_diff(&a, &b) <= tolerance => {
                    let l = ((a[0] as u32 + a[1] as u32 + a[2] as u32) / 3) as u8;
                    let f = 192 + l / 4;
                    data.extend_from_slice(&[f, f, f, 255]);
                }
                _ => data.extend_from_slice(&[255, 0, 0, 255]),
            }
        }
    }
    RgbaImage { width: w, height: h, data }
}
