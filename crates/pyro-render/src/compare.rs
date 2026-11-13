//! PNG/JPEG/YUV encoding, PNG decoding and pixel comparison.

use std::cell::RefCell;

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

/// Baseline JPEG at `quality` (1-100, libjpeg's scale), alpha dropped, with
/// 4:2:0 chroma subsampling: Chromium's JPEG screenshots and screencast
/// frames (Skia's encoder) are 4:2:0 at every quality, 100 included.
///
/// libjpeg-turbo, one compressor per thread. A 3840x2160 render (M1 Max, CPU
/// time per encode at q95, photo / gradient / text page): 108 / 64 / 71 ms
/// with jpeg-encoder's scalar 4:4:4, 43 / 24 / 27 ms with libjpeg-turbo 4:4:4
/// (the same bytes), 25 / 13 / 16 ms with 4:2:0.
///
/// An image of [`BAND_PIXELS`] or more is encoded in bands of whole MCU rows
/// on up to [`BANDS`] threads, spliced into one JPEG with restart markers
/// between them ([`splice`]): the same pixels once decoded, for about the same
/// CPU time, in 0.35-0.55 of the time (3840x2160, 4 bands).
pub fn encode_jpeg(img: &RgbaImage, quality: u8) -> Result<Vec<u8>, String> {
    if img.width > 65535 || img.height > 65535 {
        return Err(format!("{}x{} exceeds the JPEG limit of 65535", img.width, img.height));
    }
    let bands = if img.width as usize * img.height as usize >= BAND_PIXELS {
        std::thread::available_parallelism().map_or(1, |n| n.get()).min(BANDS)
    } else {
        1
    };
    encode_jpeg_in(img, quality, bands)
}

/// [`encode_jpeg`] in up to `bands` bands.
fn encode_jpeg_in(img: &RgbaImage, quality: u8, bands: usize) -> Result<Vec<u8>, String> {
    let quality = quality.clamp(1, 100);
    let (width, height) = (img.width as usize, img.height as usize);
    let mcu_rows = height.div_ceil(16);
    let bands = bands.min(mcu_rows);
    let rows = mcu_rows.div_ceil(bands.max(1)) * 16;
    // The restart interval (MCUs per band) is a u16.
    if bands <= 1 || rows / 16 * width.div_ceil(16) > usize::from(u16::MAX) {
        return compress_jpeg(&img.data, width, height, quality);
    }
    let parts = std::thread::scope(|s| {
        let threads: Vec<_> = img
            .data
            .chunks(rows * width * 4)
            .map(|band| s.spawn(move || compress_jpeg(band, width, band.len() / (width * 4), quality)))
            .collect();
        threads
            .into_iter()
            .map(|t| t.join().unwrap_or_else(|_| Err("the JPEG encoder panicked".into())))
            .collect::<Vec<Result<Vec<u8>, String>>>()
    })
    .into_iter()
    .collect::<Result<Vec<_>, String>>()?;
    splice(&parts, height as u16, (rows / 16 * width.div_ceil(16)) as u16)
}

/// Images of at least this many pixels (3840x2160 is 8.3 M) are encoded in bands.
const BAND_PIXELS: usize = 4_000_000;
/// At most this many bands (threads): 8 encoded 3840x2160 no faster than 4.
const BANDS: usize = 4;

/// One JPEG of `height` rows of RGBA, on this thread's compressor.
fn compress_jpeg(rgba: &[u8], width: usize, height: usize, quality: u8) -> Result<Vec<u8>, String> {
    thread_local! {
        static COMPRESSOR: RefCell<Option<turbojpeg::Compressor>> = const { RefCell::new(None) };
    }
    COMPRESSOR.with(|slot| {
        let mut slot = slot.borrow_mut();
        let c = match slot.as_mut() {
            Some(c) => c,
            None => slot.insert(turbojpeg::Compressor::new().map_err(|e| e.to_string())?),
        };
        c.set_quality(i32::from(quality)).map_err(|e| e.to_string())?;
        c.set_subsamp(turbojpeg::Subsamp::Sub2x2).map_err(|e| e.to_string())?;
        let image =
            turbojpeg::Image { pixels: rgba, width, pitch: width * 4, height, format: turbojpeg::PixelFormat::RGBA };
        c.compress_to_vec(image).map_err(|e| e.to_string())
    })
}

/// The bands of one image, each a baseline JPEG of the same width, tables and
/// sampling, as one JPEG of `height` rows: the first band's headers with its
/// height replaced and a restart interval of `interval` MCUs (one band's)
/// added, then each band's entropy-coded data, a restart marker between two.
/// A restart resets the DC predictions, as each band's encoding started with.
fn splice(parts: &[Vec<u8>], height: u16, interval: u16) -> Result<Vec<u8>, String> {
    /// Offsets of the SOF0 and SOS segments, and where the entropy-coded data starts.
    fn segments(jpeg: &[u8]) -> Option<(usize, usize, usize)> {
        let (mut at, mut sof) = (2, None);
        while at + 4 <= jpeg.len() && jpeg[at] == 0xFF {
            let marker = jpeg[at + 1];
            let end = at + 2 + usize::from(u16::from_be_bytes([jpeg[at + 2], jpeg[at + 3]]));
            match marker {
                0xC0 => sof = Some(at),
                0xDA => return Some((sof?, at, end)),
                _ => {}
            }
            at = end;
        }
        None
    }
    let bad = || "the JPEG encoder wrote an unexpected stream".to_string();
    let (sof, sos, data) = segments(&parts[0]).ok_or_else(bad)?;
    let mut out = Vec::with_capacity(parts.iter().map(Vec::len).sum::<usize>() + 16);
    out.extend_from_slice(&parts[0][..sos]);
    out[sof + 5..sof + 7].copy_from_slice(&height.to_be_bytes());
    out.extend_from_slice(&[0xFF, 0xDD, 0, 4]);
    out.extend_from_slice(&interval.to_be_bytes());
    out.extend_from_slice(&parts[0][sos..data]);
    for (i, part) in parts.iter().enumerate() {
        let (_, _, data) = segments(part).ok_or_else(bad)?;
        let ecs =
            part.get(data..part.len().saturating_sub(2)).filter(|_| part.ends_with(&[0xFF, 0xD9])).ok_or_else(bad)?;
        out.extend_from_slice(ecs);
        if i + 1 < parts.len() {
            out.extend_from_slice(&[0xFF, 0xD0 + (i % 8) as u8]);
        }
    }
    out.extend_from_slice(&[0xFF, 0xD9]);
    Ok(out)
}

/// The image's RGB, 3 bytes a pixel (ffmpeg's `rgb24`), alpha dropped.
pub fn encode_rgb24(img: &RgbaImage) -> Vec<u8> {
    let mut out = vec![0u8; img.data.len() / 4 * 3];
    for (o, p) in out.chunks_exact_mut(3).zip(img.data.chunks_exact(4)) {
        o.copy_from_slice(&p[..3]);
    }
    out
}

/// Gray padding of [`encode_yuv420p`] frames (ffmpeg's `gray`, as Playwright
/// pads): RGB 128, 128, 128, and its luma.
const PAD_RGB: i32 = 128;
const PAD_Y: u8 = 126;

/// A video frame of `width` x `height` as ffmpeg's `yuv420p` (I420: the Y
/// plane, then U and V at half resolution rounded up), BT.601 limited range
/// as VP8 takes it: `img` at the top left and gray where it does not reach,
/// as Playwright's recorder pads frames to the video size. Alpha is dropped;
/// chroma is the mean of each 2x2 block. A frame of [`BAND_PIXELS`] or more
/// is converted in bands on up to [`BANDS`] threads: 3840x2160 took 6 ms on
/// one thread, 2 on four (M1 Max).
pub fn encode_yuv420p(img: &RgbaImage, width: u32, height: u32) -> Vec<u8> {
    let (w, h) = (width as usize, height as usize);
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let mut out = vec![0u8; w * h + 2 * cw * ch];
    if out.is_empty() {
        return out;
    }
    let bands = if w * h >= BAND_PIXELS {
        std::thread::available_parallelism().map_or(1, |n| n.get()).min(BANDS).min(ch)
    } else {
        1
    };
    // Chroma rows per band; each covers two luma rows.
    let rows = ch.div_ceil(bands);
    let (ys, uv) = out.split_at_mut(w * h);
    let (us, vs) = uv.split_at_mut(cw * ch);
    let parts = ys.chunks_mut(2 * rows * w).zip(us.chunks_mut(rows * cw)).zip(vs.chunks_mut(rows * cw));
    std::thread::scope(|s| {
        for (k, ((y, u), v)) in parts.enumerate() {
            if bands > 1 {
                s.spawn(move || yuv420p_band(img, w, h, k * rows, y, u, v));
            } else {
                yuv420p_band(img, w, h, k * rows, y, u, v);
            }
        }
    });
    out
}

/// The rows of an [`encode_yuv420p`] frame from chroma row `cy0` on: `ys`
/// holds its luma rows (from `2 * cy0`), `us` and `vs` its chroma rows.
fn yuv420p_band(img: &RgbaImage, w: usize, h: usize, cy0: usize, ys: &mut [u8], us: &mut [u8], vs: &mut [u8]) {
    let cw = w.div_ceil(2);
    let (iw, ih) = ((img.width as usize).min(w), (img.height as usize).min(h));
    let stride = img.width as usize * 4;
    for (i, yrow) in ys.chunks_exact_mut(w).enumerate() {
        let y = 2 * cy0 + i;
        if y < ih {
            let src = &img.data[y * stride..y * stride + iw * 4];
            for (p, out) in src.chunks_exact(4).zip(&mut yrow[..iw]) {
                *out = luma(p[0].into(), p[1].into(), p[2].into());
            }
            yrow[iw..].fill(PAD_Y);
        } else {
            yrow.fill(PAD_Y);
        }
    }
    // Pixels of a 2x2 block past the frame's last odd row or column repeat the edge.
    let px = |x: usize, y: usize| -> [i32; 3] {
        let (x, y) = (x.min(w - 1), y.min(h - 1));
        if x < iw && y < ih {
            let i = y * stride + x * 4;
            [img.data[i].into(), img.data[i + 1].into(), img.data[i + 2].into()]
        } else {
            [PAD_RGB; 3]
        }
    };
    for (i, (urow, vrow)) in us.chunks_exact_mut(cw).zip(vs.chunks_exact_mut(cw)).enumerate() {
        let (y0, y1) = (2 * (cy0 + i), 2 * (cy0 + i) + 1);
        // Whole blocks inside the image: the fast path.
        let inner = if y1 < ih { iw / 2 } else { 0 };
        if inner > 0 {
            let r0 = &img.data[y0 * stride..y0 * stride + inner * 8];
            let r1 = &img.data[y1 * stride..y1 * stride + inner * 8];
            for (((a, b), u), v) in
                r0.chunks_exact(8).zip(r1.chunks_exact(8)).zip(&mut urow[..inner]).zip(&mut vrow[..inner])
            {
                let r = i32::from(a[0]) + i32::from(a[4]) + i32::from(b[0]) + i32::from(b[4]);
                let g = i32::from(a[1]) + i32::from(a[5]) + i32::from(b[1]) + i32::from(b[5]);
                let bl = i32::from(a[2]) + i32::from(a[6]) + i32::from(b[2]) + i32::from(b[6]);
                (*u, *v) = chroma(r, g, bl);
            }
        }
        for cx in inner..cw {
            let (x0, x1) = (2 * cx, 2 * cx + 1);
            let [a, b, c, d] = [px(x0, y0), px(x1, y0), px(x0, y1), px(x1, y1)];
            let sum = |k: usize| a[k] + b[k] + c[k] + d[k];
            (urow[cx], vrow[cx]) = chroma(sum(0), sum(1), sum(2));
        }
    }
}

/// BT.601 limited-range luma of an RGB pixel.
#[inline]
fn luma(r: i32, g: i32, b: i32) -> u8 {
    (((66 * r + 129 * g + 25 * b + 128) >> 8) + 16) as u8
}

/// BT.601 limited-range chroma of the sums of four pixels' channels.
#[inline]
fn chroma(r: i32, g: i32, b: i32) -> (u8, u8) {
    ((((-38 * r - 74 * g + 112 * b + 512) >> 10) + 128) as u8, (((112 * r - 94 * g - 18 * b + 512) >> 10) + 128) as u8)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A test image with detail in every channel: gradients, a checkerboard and noise.
    fn busy(width: u32, height: u32) -> RgbaImage {
        let mut seed = 0x2545_f491_u32;
        let mut data = Vec::with_capacity(width as usize * height as usize * 4);
        for y in 0..height {
            for x in 0..width {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                let check = if (x / 7 + y / 5) % 2 == 0 { 60 } else { 0 };
                data.extend_from_slice(&[
                    (x * 255 / width) as u8,
                    (y * 255 / height) as u8 ^ check,
                    (seed & 0x3f) as u8 + 100,
                    255,
                ]);
            }
        }
        RgbaImage { width, height, data }
    }

    fn decode(jpeg: &[u8]) -> Vec<u8> {
        image::load_from_memory_with_format(jpeg, image::ImageFormat::Jpeg).unwrap().to_rgb8().into_raw()
    }

    /// Bands spliced with restart markers decode to exactly the pixels of one encode, whatever
    /// the band count, the last band's height (not a whole MCU row) and the width (not one either).
    #[test]
    fn jpeg_bands_decode_as_one_encode() {
        for (w, h) in [(333, 250), (64, 16), (1000, 49)] {
            let img = busy(w, h);
            let one = encode_jpeg_in(&img, 90, 1).unwrap();
            assert!(!one.windows(2).any(|m| m == [0xFF, 0xDD]), "one band: no restart interval");
            for bands in [2, 3, 4] {
                let banded = encode_jpeg_in(&img, 90, bands).unwrap();
                let restarts = banded.windows(2).filter(|m| m[0] == 0xFF && (0xD0..=0xD7).contains(&m[1])).count();
                let rows = (h as usize).div_ceil(16).div_ceil(bands) * 16;
                assert_eq!(
                    restarts,
                    (h as usize).div_ceil(rows) - 1,
                    "{w}x{h} in {bands}: a restart between two bands"
                );
                assert_eq!(decode(&banded), decode(&one), "{w}x{h} in {bands} bands");
            }
        }
    }

    /// Luma and chroma of known colors (BT.601 limited range, as ffmpeg's yuv420p is read),
    /// gray where the image does not reach, chroma the mean of each 2x2 block, odd sizes.
    #[test]
    fn yuv420p_frames() {
        assert_eq!(luma(128, 128, 128), PAD_Y);
        let px = |c: [u8; 3]| [c[0], c[1], c[2], 255];
        // 2x2: white, black / red, red.
        let img = RgbaImage {
            width: 2,
            height: 2,
            data: [px([255; 3]), px([0; 3]), px([255, 0, 0]), px([255, 0, 0])].concat(),
        };
        let f = encode_yuv420p(&img, 2, 2);
        assert_eq!(&f[..4], &[235, 16, 82, 82]);
        // Chroma of the block's mean color: (191, 64, 64).
        assert_eq!(&f[4..], &[chroma(765, 255, 255).0, chroma(765, 255, 255).1]);
        assert_eq!(chroma(4 * 255, 0, 0), (90, 240), "red");
        // A 1x1 white image in a 3x3 frame: gray around it; the odd edge's chroma repeats it.
        let one = RgbaImage { width: 1, height: 1, data: px([255; 3]).to_vec() };
        let f = encode_yuv420p(&one, 3, 3);
        assert_eq!(f.len(), 9 + 2 * 4);
        assert_eq!(&f[..9], &[235, PAD_Y, PAD_Y, PAD_Y, PAD_Y, PAD_Y, PAD_Y, PAD_Y, PAD_Y]);
        assert_eq!(&f[9..13], &[128; 4], "white and gray have no chroma");
        // A larger image than the frame is cut.
        let big = busy(10, 10);
        let f = encode_yuv420p(&big, 4, 2);
        assert_eq!(f[3], luma(big.data[12].into(), big.data[13].into(), big.data[14].into()));
    }

    /// A large frame converted in bands on threads is the frame converted in one, also with
    /// an odd height and a padded edge.
    #[test]
    fn yuv420p_bands_convert_as_one() {
        let img = busy(2601, 1599);
        let (w, h): (usize, usize) = (2602, 1601);
        let banded = encode_yuv420p(&img, w as u32, h as u32);
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        let mut one = vec![0u8; w * h + 2 * cw * ch];
        let (ys, uv) = one.split_at_mut(w * h);
        let (us, vs) = uv.split_at_mut(cw * ch);
        yuv420p_band(&img, w, h, 0, ys, us, vs);
        assert!(banded == one, "bands differ from one conversion");
    }
}
