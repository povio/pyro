pub mod capture;
mod gif;
mod icc;
mod webp;

use image::{metadata::Orientation, ImageDecoder, ImageError, ImageResult, Limits};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, VecDeque},
    io::Cursor,
    sync::{Arc, Mutex, Weak, atomic::{AtomicUsize, Ordering}},
};

pub const BYTE_BUDGET: usize = 256 << 20;

fn limit() -> ImageError {
    ImageError::Limits(image::error::LimitError::from_kind(image::error::LimitErrorKind::InsufficientMemory))
}

/// Runs a decoder's pixel decode; a panic in it (a decoder bug on a malformed file) becomes a
/// decoding error rather than unwinding through the caller. The checks before decoding are what
/// keep malformed files away from decoders; this is the last line only.
fn contained<T>(format: image::ImageFormat, decode: impl FnOnce() -> ImageResult<T>) -> ImageResult<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(decode)).unwrap_or_else(|_| {
        Err(ImageError::Decoding(image::error::DecodingError::new(format.into(), "the decoder failed on this file")))
    })
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
    let limits = DecodeLimits { max_memory: Some((available as u64).min(Limits::default().max_alloc.unwrap_or(u64::MAX))), ..Default::default() };
    let error = |err| ImageError::Decoding(image::error::DecodingError::new(image::ImageFormat::Jpeg.into(), err));
    let (width, height, _) = jpeg_geometry(bytes)?;
    if !capture::surface(width.into(), height.into(), 5) { return Err(limit()); }
    if rgba_bytes(width.into(), height.into())? > available {
        return Err(limit());
    }
    let decoder = Decoder::new_with_limits(bytes, limits).map_err(error)?.with_output_format(PixelFormat::Rgba);
    if decoder.output_buffer_size().map_err(error)? != rgba_bytes(width.into(), height.into())? { return Err(limit()); }
    #[cfg(any(test, feature = "test-budgets"))]
    PIXEL_DECODES.update(|count| count + 1);
    let decoded = contained(image::ImageFormat::Jpeg, || decoder.decode_image().map_err(error))?;
    // The pixels are the size the header said (what the budget admitted).
    if (decoded.width as u64, decoded.height as u64) != (u64::from(width), u64::from(height)) {
        return Err(error(libjpeg_turbo_rs::JpegError::CorruptData("decoded size differs from the header".into())));
    }
    let orientation = decoded.exif_orientation().and_then(Orientation::from_exif).unwrap_or(Orientation::NoTransforms);
    let mut pixels =
        image::RgbaImage::from_raw(decoded.width as u32, decoded.height as u32, decoded.data).ok_or_else(limit)?;
    if let Some(profile) = decoded.icc_profile {
        icc::to_srgb(&profile, &mut pixels);
    }
    Ok((pixels.into(), orientation))
}

#[derive(Clone, Debug)]
pub struct Raster {
    pub width: u32,
    pub height: u32,
    pub blob: linebender_resource_handle::Blob<u8>,
    pub orientation: u8,
}

#[derive(Clone, Default, Debug)]
pub struct PageRasters {
    used: Arc<AtomicUsize>,
    cache: Arc<Mutex<VecDeque<Entry>>>,
    sources: Arc<SourceRegistry>,
}

type SourceRegistry = Mutex<BTreeMap<usize, Weak<SourceLease>>>;

#[derive(Debug)]
pub struct SourceLease {
    bytes: Arc<Vec<u8>>,
    registry: Weak<SourceRegistry>,
    reservation: Reservation,
}

impl SourceLease {
    pub fn belongs_to(&self, page: &PageRasters) -> bool { self.reservation.belongs_to(page) }
    pub fn bytes(&self) -> &Arc<Vec<u8>> { &self.bytes }
}

impl PartialEq for SourceLease {
    fn eq(&self, other: &Self) -> bool { std::ptr::eq(self, other) }
}
impl Eq for SourceLease {}

impl Drop for SourceLease {
    fn drop(&mut self) {
        if let Some(registry) = self.registry.upgrade() {
            let mut registry = registry.lock().unwrap_or_else(|err| err.into_inner());
            let key = Arc::as_ptr(&self.bytes) as usize;
            if registry.get(&key).is_some_and(|lease| std::ptr::eq(lease.as_ptr(), self)) {
                registry.remove(&key);
            }
        }
    }
}

#[derive(Debug)]
pub struct Reservation(Arc<AtomicUsize>, usize);

impl PartialEq for PageRasters {
    fn eq(&self, other: &Self) -> bool { self.same_page(other) }
}

impl Reservation {
    pub fn belongs_to(&self, page: &PageRasters) -> bool { Arc::ptr_eq(&self.0, &page.used) }
    pub fn grow_to(&mut self, page: &PageRasters, size: usize) -> ImageResult<()> {
        if !self.belongs_to(page) { return Err(limit()); }
        if size > self.1 {
            let mut extra = page.reserve(size - self.1)?;
            self.1 = size;
            extra.1 = 0;
        }
        Ok(())
    }
}

impl Drop for Reservation {
    fn drop(&mut self) { self.0.fetch_sub(self.1, Ordering::AcqRel); }
}

struct Pixels(Vec<u8>, #[allow(dead_code)] Reservation);

impl AsRef<[u8]> for Pixels {
    fn as_ref(&self) -> &[u8] { &self.0 }
}

type Entry = ([u8; 32], Reservation, Raster);

impl PageRasters {
    pub fn retain_source(&self, bytes: Arc<Vec<u8>>) -> ImageResult<Arc<SourceLease>> {
        let mut registry = self.sources.lock().unwrap_or_else(|err| err.into_inner());
        let key = Arc::as_ptr(&bytes) as usize;
        if let Some(lease) = registry.get(&key).and_then(Weak::upgrade) { return Ok(lease); }
        let reservation = self.reserve(bytes.capacity().checked_add(std::mem::size_of::<SourceLease>()).ok_or_else(limit)?)?;
        let lease = Arc::new(SourceLease { bytes, reservation, registry: Arc::downgrade(&self.sources) });
        registry.insert(key, Arc::downgrade(&lease));
        Ok(lease)
    }
    pub fn same_page(&self, other: &Self) -> bool { Arc::ptr_eq(&self.used, &other.used) }
    pub fn used(&self) -> usize { self.used.load(Ordering::Acquire) }

    pub fn reserve(&self, bytes: usize) -> ImageResult<Reservation> {
        let maximum = BYTE_BUDGET;
        #[cfg(feature = "test-budgets")]
        let maximum = maximum.min(TEST_BUDGET.get());
        self.used.fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
            used.checked_add(bytes).filter(|next| *next <= maximum)
        }).map_err(|_| limit())?;
        Ok(Reservation(self.used.clone(), bytes))
    }

    pub fn decode(&self, bytes: &[u8]) -> ImageResult<Raster> {
        if !capture::charge(bytes.len().div_ceil(64).saturating_mul(64)) { return Err(limit()); }
        let format = image::guess_format(bytes)?;
        let digest: [u8; 32] = Sha256::digest(bytes).into();
        let mut cache = self.cache.lock().unwrap_or_else(|err| err.into_inner());
        if let Some(index) = cache.iter().position(|entry| entry.0 == digest) {
            let entry = cache.remove(index).unwrap();
            let result = entry.2.clone();
            cache.push_back(entry);
            return Ok(result);
        }
        let dimensions = imagesize::blob_size(bytes).map_err(|_| limit())?;
        let size = rgba_bytes(dimensions.width as u64, dimensions.height as u64)?;
        let scratch_size = if bytes.starts_with(&[0xff, 0xd8]) {
            let header = libjpeg_turbo_rs::Decoder::new_header_only(bytes, Default::default()).map_err(|_| limit())?;
            if (usize::from(header.header().width), usize::from(header.header().height)) != (dimensions.width, dimensions.height) { return Err(limit()); }
            size.checked_mul(if header.header().is_progressive { 4 } else { 1 })
        } else {
            size.checked_mul(4)
        }.and_then(|scratch| scratch.checked_add(64 << 10)).ok_or_else(limit)?;
        let scratch_size = match format {
            image::ImageFormat::WebP => scratch_size.max(webp::check(bytes)?),
            image::ImageFormat::Gif => scratch_size.max(gif::scratch(bytes)),
            _ => scratch_size,
        };
        if !capture::charge(size.saturating_add(scratch_size).div_ceil(256)) { return Err(limit()); }
        let (charge, scratch) = loop {
            match self.reserve(size).and_then(|pixels| self.reserve(scratch_size).map(|scratch| (pixels, scratch))) {
                Ok(charges) => break charges,
                Err(_) if cache.pop_front().is_some() => continue,
                Err(error) => return Err(error),
            }
        };
        let raster = decode_reserved(format, bytes, charge, &scratch)?;
        drop(scratch);
        while cache.len() >= 64 { cache.pop_front(); }
        if let Ok(key) = self.reserve(std::mem::size_of::<Entry>()) {
            cache.push_back((digest, key, raster.clone()));
        }
        Ok(raster)
    }
}

fn decode_reserved(format: image::ImageFormat, bytes: &[u8], charge: Reservation, scratch: &Reservation) -> ImageResult<Raster> {
    let available = charge.1;
    let (mut image, profile, orientation) = if bytes.starts_with(&[0xff, 0xd8]) {
        let (image, orientation) = decode_jpeg_with_budget(bytes, available + scratch.1)?;
        if rgba_bytes(image.width().into(), image.height().into())? != available { return Err(limit()); }
        (image, None, orientation)
    } else {
        let mut reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
        let mut limits = Limits::default();
        limits.max_alloc = Some(scratch.1 as u64);
        reader.limits(limits);
        let mut decoder = reader.into_decoder()?;
        let (width, height) = decoder.dimensions();
        if decoder.total_bytes() > scratch.1 as u64 || rgba_bytes(width.into(), height.into())? != available {
            return Err(limit());
        }
        Limits::default().reserve(decoder.total_bytes())?;
        let profile = decoder.icc_profile().ok().flatten();
        let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
        #[cfg(any(test, feature = "test-budgets"))]
        PIXEL_DECODES.update(|count| count + 1);
        (contained(format, || image::DynamicImage::from_decoder(decoder))?, profile, orientation)
    };
    image.apply_orientation(orientation);
    let (width, height) = (image.width(), image.height());
    let mut pixels = image.into_rgba8().into_raw();
    if let Some(profile) = profile {
        icc::to_srgb(&profile, &mut pixels);
    }
    if pixels.capacity() != charge.1 { return Err(limit()); }
    let raster = Raster {
        width,
        height,
        blob: linebender_resource_handle::Blob::new(Arc::new(Pixels(pixels, charge))),
        orientation: orientation.to_exif(),
    };
    Ok(raster)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_decoder_panic_is_a_decoding_error() {
        let result: ImageResult<()> = contained(image::ImageFormat::WebP, || panic!("decoder bug"));
        assert!(matches!(result, Err(ImageError::Decoding(_))));
    }

    #[test]
    fn scene_blob_holds_page_charge_after_eviction() {
        let page = PageRasters::default();
        let mut encoded = Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(32, 32).write_to(&mut encoded, image::ImageFormat::Png).unwrap();
        let blob = page.decode(encoded.get_ref()).unwrap().blob;
        page.cache.lock().unwrap().clear();
        assert_eq!(page.used(), 32 * 32 * 4);
        drop(blob);
        assert_eq!(page.used(), 0);
    }

    #[test]
    fn shared_source_owners_charge_capacity_once_until_last_drop() {
        let page = PageRasters::default();
        let bytes = Arc::new(Vec::with_capacity(4096));
        let first = page.retain_source(bytes.clone()).unwrap();
        let cost = page.used();
        assert!(cost >= bytes.capacity());
        let second = page.retain_source(bytes).unwrap();
        assert_eq!(page.used(), cost);
        drop(first);
        assert_eq!(page.used(), cost);
        drop(second);
        assert_eq!(page.used(), 0);
        assert!(page.sources.lock().unwrap().is_empty());
    }

    #[test]
    fn eviction_charges_decoded_storage_and_key() {
        let page = PageRasters::default();
        let _occupied = page.reserve(BYTE_BUDGET - (5 << 20) - (64 << 10)).unwrap();
        for index in 0..12 {
            let mut output = Cursor::new(Vec::new());
            image::RgbaImage::from_pixel(512, 512, image::Rgba([index; 4])).write_to(&mut output, image::ImageFormat::Png).unwrap();
            let raster = page.decode(output.get_ref()).unwrap();
            assert_eq!(page.cache.lock().unwrap().len(), 1);
            assert_eq!(raster.blob.data()[0], index);
            assert!(page.used() <= BYTE_BUDGET);
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
            let page = PageRasters::default();
            let _occupied = page.reserve(BYTE_BUDGET - (17 * 19 * 4 - 1)).unwrap();
            assert!(matches!(page.decode(output.get_ref()), Err(ImageError::Limits(_))), "{format:?}");
            assert_eq!(PIXEL_DECODES.get(), before);
        }
    }

    #[test]
    fn repeated_photo_uses_one_decode_and_one_pixel_buffer() {
        let mut output = Cursor::new(Vec::new());
        image::DynamicImage::new_luma8(6000, 4000).write_to(&mut output, image::ImageFormat::Jpeg).unwrap();
        let data = Arc::new(output.into_inner());
        let before = PIXEL_DECODES.get();
        let budget = PageRasters::default();
        let expected = budget.decode(&data).unwrap();
        for _ in 0..32 {
            let actual = budget.decode(&data).unwrap();
            assert_eq!(actual.blob.id(), expected.blob.id());
        }
        assert_eq!(PIXEL_DECODES.get() - before, 1);
        assert_eq!(budget.used(), expected.blob.len() + std::mem::size_of::<Entry>());
    }
}
