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

/// [`decode`], the pixels turned as the image's EXIF orientation says when `oriented` (an
/// `<img>` drawn as `image-orientation: from-image` shows it, or an opaque one).
pub fn decode_oriented(bytes: &[u8], oriented: bool) -> Option<Pixmap> {
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
