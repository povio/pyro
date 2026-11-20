//! Image decoding (for `drawImage`, patterns and `createImageBitmap`) and
//! encoding (`toDataURL`, `toBlob`, `convertToBlob`).

use std::io::Cursor;

use crate::{valid_size, Encoding, Pixmap};

/// Most memory one decode may allocate.
const MAX_DECODE_BYTES: u64 = 256 * 1024 * 1024;

/// The first frame of a PNG, JPEG, GIF or WebP image, premultiplied, or
/// `None` for anything else, a corrupt image or one larger than a canvas may
/// be (decoding stops at the header).
pub fn decode(bytes: &[u8]) -> Option<Pixmap> {
    decode_oriented(bytes, false)
}

/// Rasterizes an SVG image at its natural size: premultiplied RGBA rows, and the width and height.
pub type SvgDecoder = fn(&[u8], &pyro_raster::PageRasters) -> Option<(Vec<u8>, u32, u32, pyro_raster::Reservation)>;

static SVG_DECODER: std::sync::OnceLock<SvgDecoder> = std::sync::OnceLock::new();

/// Registers the SVG rasterizer [`decode`] uses for SVG images (`pyro-render` registers one: an
/// `<img>` of an SVG drawn by `drawImage` drew nothing).
pub fn set_svg_decoder(decoder: SvgDecoder) {
    let _ = SVG_DECODER.set(decoder);
}

/// Whether `bytes` look like an SVG document (XML whose root is `<svg>`, or SVGZ): an image
/// [`decode`] rasterizes (at no more than a fixed number of pixels, so smaller than its natural
/// size when that is huge).
pub fn is_svg(bytes: &[u8]) -> bool {
    looks_like_svg(bytes)
}

fn looks_like_svg(bytes: &[u8]) -> bool {
    if bytes.starts_with(&[0x1f, 0x8b]) {
        return true;
    }
    let head = &bytes[..bytes.len().min(4096)];
    let text = String::from_utf8_lossy(head);
    let text = text.trim_start_matches('\u{feff}').trim_start();
    text.starts_with('<') && text.contains("<svg")
}

/// [`decode`], the pixels turned as the image's EXIF orientation says when `oriented` (an
/// `<img>` drawn as `image-orientation: from-image` shows it, or an opaque one).
pub fn decode_oriented(bytes: &[u8], oriented: bool) -> Option<Pixmap> {
    // pyro: an SVG image, rasterized at its natural size by the registered decoder.
    if let Some(decoder) = SVG_DECODER.get().filter(|_| looks_like_svg(bytes)) {
        let (rgba, w, h, _charge) = decoder(bytes, &Default::default())?;
        return crate::pixmap_from_premultiplied(rgba, w, h);
    }
    let mut reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format().ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(crate::MAX_SIDE);
    limits.max_image_height = Some(crate::MAX_SIDE);
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    reader.limits(limits);
    let (w, h) = reader.into_dimensions().ok()?;
    if !valid_size(w, h) {
        return None;
    }
    let mut reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format().ok()?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    reader.limits(limits);
    let image = if oriented {
        use image::ImageDecoder;
        let mut decoder = reader.into_decoder().ok()?;
        let orientation = decoder.orientation().unwrap_or(image::metadata::Orientation::NoTransforms);
        let mut image = image::DynamicImage::from_decoder(decoder).ok()?;
        image.apply_orientation(orientation);
        image
    } else {
        reader.decode().ok()?
    };
    let rgba = image.into_rgba8();
    let (w, h) = rgba.dimensions();
    crate::pixmap_from_straight(rgba.into_raw(), w, h)
}

pub fn decode_bitmap(bytes: &[u8], oriented: bool, page: &pyro_raster::PageRasters) -> Option<crate::Bitmap> {
    let (pixmap, charge) = if looks_like_svg(bytes) {
        let (rgba, width, height, charge) = SVG_DECODER.get()?(bytes, page)?;
        (crate::pixmap_from_premultiplied(rgba, width, height)?, charge)
    } else {
        let reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format().ok()?;
        let (stored_width, stored_height) = reader.into_dimensions().ok()?;
        if !valid_size(stored_width, stored_height) { return None; }
        let raster = page.decode(bytes).ok()?;
        let input = raster.blob.data();
        let charge = page.reserve(input.len()).ok()?;
        let (pixels, width, height) = if oriented || raster.orientation == 1 {
            (input.to_vec(), raster.width, raster.height)
        } else {
            let width = stored_width as usize;
            let height = stored_height as usize;
            let mut pixels = vec![0; input.len()];
            for (index, pixel) in pixels.chunks_exact_mut(4).enumerate() {
                let (col, row) = (index % width, index / width);
                let (x, y) = match raster.orientation {
                    2 => (width - 1 - col, row),
                    3 => (width - 1 - col, height - 1 - row),
                    4 => (col, height - 1 - row),
                    5 => (row, col),
                    6 => (height - 1 - row, col),
                    7 => (height - 1 - row, width - 1 - col),
                    8 => (row, width - 1 - col),
                    _ => return None,
                };
                let offset = (y * raster.width as usize + x) * 4;
                pixel.copy_from_slice(input.get(offset..offset + 4)?);
            }
            (pixels, stored_width, stored_height)
        };
        (crate::pixmap_from_straight(pixels, width, height)?, charge)
    };
    let mut bitmap = crate::Bitmap::new(pixmap)?;
    bitmap.page_charge = Some(std::sync::Arc::new(charge));
    Some(bitmap)
}

/// Straight RGBA rows (`width` by `height`) in `encoding`.
pub fn encode(rgba: &[u8], width: u32, height: u32, encoding: Encoding) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    match encoding {
        Encoding::Png => {
            let mut enc = png::Encoder::new(&mut out, width, height);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            enc.set_compression(png::Compression::Fast);
            let mut w = enc.write_header().ok()?;
            w.write_image_data(rgba).ok()?;
            w.finish().ok()?;
        }
        Encoding::Jpeg(quality) => {
            // No alpha in JPEG: transparent pixels are composited onto
            // black, as browsers do.
            let rgb: Vec<u8> = rgba
                .chunks_exact(4)
                .flat_map(|p| {
                    let a = u32::from(p[3]);
                    [0, 1, 2].map(|k| ((u32::from(p[k]) * a + 127) / 255) as u8)
                })
                .collect();
            let quality = (quality.clamp(0.0, 1.0) * 100.0).round().max(1.0) as u8;
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality)
                .encode(&rgb, width, height, image::ExtendedColorType::Rgb8)
                .ok()?;
        }
        Encoding::Webp => {
            image::codecs::webp::WebPEncoder::new_lossless(&mut out)
                .encode(rgba, width, height, image::ExtendedColorType::Rgba8)
                .ok()?;
        }
    }
    Some(out)
}

/// `toDataURL`/`toBlob` type: PNG unless JPEG or WebP is asked for, with
/// the MIME type the result has.
pub fn encoding_for(mime: &str, quality: Option<f64>) -> (Encoding, &'static str) {
    let quality = quality.filter(|q| (0.0..=1.0).contains(q)).map_or(0.92, |q| q as f32);
    match mime.to_ascii_lowercase().as_str() {
        "image/jpeg" => (Encoding::Jpeg(quality), "image/jpeg"),
        "image/webp" => (Encoding::Webp, "image/webp"),
        _ => (Encoding::Png, "image/png"),
    }
}
