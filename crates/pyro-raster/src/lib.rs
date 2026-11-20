mod icc;

use image::{metadata::Orientation, ImageDecoder, ImageError, ImageResult, Limits};
use std::{
    collections::VecDeque,
    io::Cursor,
    sync::{Arc, LazyLock, Mutex},
};

pub const BYTE_BUDGET: usize = 256 << 20;

fn limit() -> ImageError {
    ImageError::Limits(image::error::LimitError::from_kind(image::error::LimitErrorKind::InsufficientMemory))
}

fn rgba_bytes(width: u64, height: u64) -> ImageResult<usize> {
    let size = width.checked_mul(height).and_then(|n| n.checked_mul(4)).ok_or_else(limit)?;
    Limits::default().reserve(size)?;
    usize::try_from(size).map_err(|_| limit())
}

#[cfg(any(test, feature = "test-budgets"))]
thread_local! { static PIXEL_DECODES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }

#[cfg(feature = "test-budgets")]
thread_local! { static TEST_BUDGET: std::cell::Cell<usize> = const { std::cell::Cell::new(BYTE_BUDGET) }; }

#[cfg(feature = "test-budgets")]
pub fn with_test_budget<T>(bytes: usize, run: impl FnOnce() -> T) -> (T, usize) {
    struct Restore(usize);
    impl Drop for Restore {
        fn drop(&mut self) {
            TEST_BUDGET.set(self.0);
        }
    }
    let _restore = Restore(TEST_BUDGET.replace(bytes.min(BYTE_BUDGET)));
    let before = PIXEL_DECODES.get();
    (run(), PIXEL_DECODES.get() - before)
}

pub fn decode_jpeg(bytes: &[u8]) -> ImageResult<(image::DynamicImage, Orientation)> {
    decode_jpeg_with_budget(bytes, usize::MAX)
}

pub fn jpeg_geometry(bytes: &[u8]) -> ImageResult<(u32, u32, u8)> {
    let limits = libjpeg_turbo_rs::DecodeLimits { max_memory: Limits::default().max_alloc, ..Default::default() };
    let decoder = libjpeg_turbo_rs::Decoder::new_header_only(bytes, limits).map_err(|error| {
        ImageError::Decoding(image::error::DecodingError::new(image::ImageFormat::Jpeg.into(), error))
    })?;
    let (width, height) = (u32::from(decoder.header().width), u32::from(decoder.header().height));
    rgba_bytes(width.into(), height.into())?;
    Ok((width, height, decoder.exif_orientation().unwrap_or(1)))
}

pub fn unturn(orientation: u8, width: u32, height: u32) -> [f64; 6] {
    let (width, height) = (f64::from(width), f64::from(height));
    match orientation {
        2 => [-1., 0., 0., 1., width, 0.],
        3 => [-1., 0., 0., -1., width, height],
        4 => [1., 0., 0., -1., 0., height],
        5 => [0., 1., 1., 0., 0., 0.],
        6 => [0., -1., 1., 0., 0., height],
        7 => [0., -1., -1., 0., width, height],
        8 => [0., 1., -1., 0., width, 0.],
        _ => [1., 0., 0., 1., 0., 0.],
    }
}

fn decode_jpeg_with_budget(bytes: &[u8], available: usize) -> ImageResult<(image::DynamicImage, Orientation)> {
    use libjpeg_turbo_rs::{DecodeLimits, Decoder, PixelFormat};
    let limits = DecodeLimits { max_memory: Limits::default().max_alloc, ..Default::default() };
    let error = |err| ImageError::Decoding(image::error::DecodingError::new(image::ImageFormat::Jpeg.into(), err));
    let (width, height, _) = jpeg_geometry(bytes)?;
    if rgba_bytes(width.into(), height.into())? > available {
        return Err(limit());
    }
    let decoder = Decoder::new_with_limits(bytes, limits).map_err(error)?.with_output_format(PixelFormat::Rgba);
    #[cfg(any(test, feature = "test-budgets"))]
    PIXEL_DECODES.update(|count| count + 1);
    let decoded = decoder.decode_image().map_err(error)?;
    let orientation = decoded.exif_orientation().and_then(Orientation::from_exif).unwrap_or(Orientation::NoTransforms);
    let mut pixels =
        image::RgbaImage::from_raw(decoded.width as u32, decoded.height as u32, decoded.data).ok_or_else(limit)?;
    if let Some(profile) = decoded.icc_profile {
        icc::to_srgb(&profile, &mut pixels);
    }
    Ok((pixels.into(), orientation))
}

#[derive(Clone)]
pub struct Raster {
    pub width: u32,
    pub height: u32,
    pub pixels: Arc<Vec<u8>>,
    pub blob: linebender_resource_handle::Blob<u8>,
    pub orientation: u8,
}

#[derive(Default)]
pub struct DocumentBudget {
    used: usize,
    sources: std::collections::HashMap<usize, (std::sync::Weak<Vec<u8>>, Raster)>,
}

impl DocumentBudget {
    pub fn decode(&mut self, data: &Arc<Vec<u8>>) -> ImageResult<Raster> {
        let key = Arc::as_ptr(data) as usize;
        if let Some((_, raster)) = self.sources.get(&key) {
            return Ok(raster.clone());
        }
        let overhead = std::mem::size_of::<Raster>() + 128;
        let budget = BYTE_BUDGET;
        #[cfg(feature = "test-budgets")]
        let budget = budget.min(TEST_BUDGET.get());
        let available = budget.checked_sub(self.used + overhead).ok_or_else(limit)?;
        let raster = decode(data, available)?;
        if raster.pixels.capacity() > available {
            return Err(limit());
        }
        self.used += overhead + raster.pixels.capacity();
        self.sources.insert(key, (Arc::downgrade(data), raster.clone()));
        Ok(raster)
    }
}

type Entry = (Box<[u8]>, Raster);
static CACHE: LazyLock<Mutex<VecDeque<Entry>>> = LazyLock::new(Default::default);

fn entry_bytes(compressed: usize, pixels: usize) -> usize {
    compressed.saturating_add(pixels).saturating_add(std::mem::size_of::<Entry>() + 64)
}

fn retain(cache: &mut VecDeque<Entry>, bytes: &[u8], raster: &Raster, budget: usize) {
    let charge = entry_bytes(bytes.len(), raster.pixels.capacity());
    if charge > budget {
        return;
    }
    let mut used: usize = cache.iter().map(|(key, raster)| entry_bytes(key.len(), raster.pixels.capacity())).sum();
    while cache.len() >= 64 || used > budget - charge {
        let (key, raster) = cache.pop_front().unwrap();
        used -= entry_bytes(key.len(), raster.pixels.capacity());
    }
    cache.push_back((bytes.into(), raster.clone()));
}

pub fn decode(bytes: &[u8], available: usize) -> ImageResult<Raster> {
    image::guess_format(bytes)?;
    let mut cache = CACHE.lock().unwrap_or_else(|error| error.into_inner());
    if let Some(index) = cache.iter().position(|(key, _)| key.as_ref() == bytes) {
        let entry = cache.remove(index).unwrap();
        let raster = entry.1.clone();
        cache.push_back(entry);
        return if raster.pixels.len() <= available { Ok(raster) } else { Err(limit()) };
    }
    if let Ok(size) = imagesize::blob_size(bytes) {
        if rgba_bytes(size.width as u64, size.height as u64)? > available {
            return Err(limit());
        }
    }
    let (mut image, profile, orientation) = if bytes.starts_with(&[0xff, 0xd8]) {
        let (image, orientation) = decode_jpeg_with_budget(bytes, available)?;
        (image, None, orientation)
    } else {
        let mut decoder = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format()?.into_decoder()?;
        let (width, height) = decoder.dimensions();
        if rgba_bytes(width.into(), height.into())? > available {
            return Err(limit());
        }
        Limits::default().reserve(decoder.total_bytes())?;
        let profile = decoder.icc_profile().ok().flatten();
        let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
        #[cfg(any(test, feature = "test-budgets"))]
        PIXEL_DECODES.update(|count| count + 1);
        (image::DynamicImage::from_decoder(decoder)?, profile, orientation)
    };
    image.apply_orientation(orientation);
    let (width, height) = (image.width(), image.height());
    let mut pixels = image.into_rgba8().into_raw();
    if let Some(profile) = profile {
        icc::to_srgb(&profile, &mut pixels);
    }
    let pixels = Arc::new(pixels);
    let raster = Raster {
        width,
        height,
        blob: linebender_resource_handle::Blob::new(pixels.clone()),
        pixels,
        orientation: orientation.to_exif(),
    };
    retain(&mut cache, bytes, &raster, BYTE_BUDGET);
    Ok(raster)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eviction_charges_decoded_and_compressed_storage() {
        let mut cache = VecDeque::new();
        let budget = 2 << 20;
        for index in 0..12 {
            let pixels = Arc::new(vec![index; 512 * 512 * 4]);
            let raster = Raster {
                width: 512,
                height: 512,
                blob: linebender_resource_handle::Blob::new(pixels.clone()),
                pixels,
                orientation: 1,
            };
            retain(&mut cache, &[index; 512], &raster, budget);
            assert_eq!(cache.len(), 1);
            assert_eq!(cache[0].0[0], index);
        }
    }

    #[test]
    fn header_budget_precedes_every_pixel_decode() {
        for format in
            [image::ImageFormat::Jpeg, image::ImageFormat::Png, image::ImageFormat::Gif, image::ImageFormat::WebP]
        {
            let mut output = Cursor::new(Vec::new());
            image::DynamicImage::new_rgb8(17, 19).write_to(&mut output, format).unwrap();
            let before = PIXEL_DECODES.get();
            assert!(matches!(decode(output.get_ref(), 17 * 19 * 4 - 1), Err(ImageError::Limits(_))), "{format:?}");
            assert_eq!(PIXEL_DECODES.get(), before);
        }
    }

    #[test]
    fn repeated_photo_uses_one_decode_and_one_pixel_buffer() {
        let mut output = Cursor::new(Vec::new());
        image::DynamicImage::new_luma8(6000, 4000).write_to(&mut output, image::ImageFormat::Jpeg).unwrap();
        let data = Arc::new(output.into_inner());
        let before = PIXEL_DECODES.get();
        let mut budget = DocumentBudget::default();
        let expected = budget.decode(&data).unwrap();
        for _ in 0..32 {
            let actual = budget.decode(&data).unwrap();
            assert!(Arc::ptr_eq(&actual.pixels, &expected.pixels));
            assert_eq!(actual.blob.id(), expected.blob.id());
        }
        assert_eq!(PIXEL_DECODES.get() - before, 1);
        assert!(budget.used < 6000 * 4000 * 4 + 1024);
    }
}
