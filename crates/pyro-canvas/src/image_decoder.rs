//! Raster tracks for WebCodecs ImageDecoder. Metadata is read without
//! decoding pixels; a GIF frame is composited by the image crate up to the
//! requested index. JPEG orientation is metadata, not a rotation of the bytes.

use std::io::Cursor;

use image::{AnimationDecoder, DynamicImage, ImageDecoder, Limits};
use pyro_raster::{PageRasters, Reservation};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageType {
    Png,
    Jpeg,
    Gif,
}

impl ImageType {
    pub fn from_mime(mime: &str) -> Option<Self> {
        if mime.eq_ignore_ascii_case("image/png") {
            Some(Self::Png)
        } else if mime.eq_ignore_ascii_case("image/jpeg") {
            Some(Self::Jpeg)
        } else if mime.eq_ignore_ascii_case("image/gif") {
            Some(Self::Gif)
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Metadata {
    pub width: u32,
    pub height: u32,
    pub orientation: u8,
    pub frame_count: u32,
    pub animated: bool,
    pub repetition_count: f64,
}

impl Metadata {
    fn still(width: u32, height: u32, orientation: u8) -> Result<Self, String> {
        if !crate::valid_size(width, height) {
            return Err("unsupported image dimensions".into());
        }
        Ok(Self { width, height, orientation, frame_count: 1, animated: false, repetition_count: 0.0 })
    }
}

pub struct DecodedFrame {
    pub width: u32,
    pub height: u32,
    /// Straight-alpha RGBA8 in the image's stored pixel order.
    pub rgba: Vec<u8>,
    pub orientation: u8,
    pub timestamp: i64,
    pub duration: Option<u64>,
    /// Travels with the output pixels while they are retained.
    pub reservation: Reservation,
}

struct Track {
    metadata: Metadata,
    // Frame timing is present when the requested frame's data is complete.
    ready: Option<(i64, Option<u64>)>,
}

fn incomplete<T>(finished: bool) -> Result<Option<T>, String> {
    if finished {
        Err("incomplete image data".into())
    } else {
        Ok(None)
    }
}

fn signature(bytes: &[u8], sig: &[u8]) -> Result<(), String> {
    let len = bytes.len().min(sig.len());
    if bytes[..len] != sig[..len] {
        return Err("image data does not match its declared type".into());
    }
    Ok(())
}

/// Discover a track and the known frame count from the supplied prefix.
/// An unfinished prefix without sufficient metadata returns None.
pub fn metadata(kind: ImageType, bytes: &[u8], finished: bool) -> Result<Option<Metadata>, String> {
    Ok(track(kind, bytes, finished, 0)?.map(|track| track.metadata))
}

fn track(kind: ImageType, bytes: &[u8], finished: bool, index: u32) -> Result<Option<Track>, String> {
    if !pyro_raster::capture::charge(bytes.len().div_ceil(64)) {
        return Err("image exceeds processing budget".into());
    }
    match kind {
        ImageType::Png => png_track(bytes, finished),
        ImageType::Gif => gif_track(bytes, finished, index),
        ImageType::Jpeg => {
            signature(bytes, &[0xff, 0xd8])?;
            let (w, h, orientation) = match pyro_raster::jpeg_geometry(bytes) {
                Ok(geometry) => geometry,
                Err(_) if !finished => return Ok(None),
                Err(e) => return Err(e.to_string()),
            };
            let metadata = Metadata::still(w, h, orientation)?;
            Ok(Some(Track { metadata, ready: jpeg_ended(bytes).then_some((0, None)) }))
        }
    }
}

// Walk lengths rather than searching for EOI inside arbitrary metadata or
// entropy-coded bytes. JPEG permits further scans and trailing data after EOI.
fn jpeg_ended(bytes: &[u8]) -> bool {
    let mut pos = 2;
    let mut entropy = false;
    while let Some(&b) = bytes.get(pos) {
        if b != 0xff {
            if !entropy {
                return false;
            }
            pos += 1;
            continue;
        }
        while bytes.get(pos) == Some(&0xff) {
            pos += 1;
        }
        let Some(&marker) = bytes.get(pos) else { return false };
        pos += 1;
        match marker {
            0xd9 => return true,
            0x00 | 0xd0..=0xd7 if entropy => continue,
            0x01 => continue,
            _ => {
                let Some(len) = bytes.get(pos..pos + 2) else { return false };
                let len = usize::from(u16::from_be_bytes([len[0], len[1]]));
                if len < 2 || len > bytes.len() - pos {
                    return false;
                }
                pos += len;
                entropy = marker == 0xda;
            }
        }
    }
    false
}

fn png_track(bytes: &[u8], finished: bool) -> Result<Option<Track>, String> {
    signature(bytes, b"\x89PNG\r\n\x1a\n")?;
    if bytes.len() < 33 {
        return incomplete(finished);
    }
    if &bytes[8..16] != b"\x00\x00\x00\x0dIHDR" {
        return Err("missing PNG image header".into());
    }
    let dim = |p: usize| u32::from_be_bytes(bytes[p..p + 4].try_into().expect("PNG header"));
    let mut metadata = Metadata::still(dim(16), dim(20), 1)?;
    let mut has_data = false;
    let mut pos = 33;
    while let Some(header) = bytes.get(pos..pos + 8) {
        let length = u32::from_be_bytes(header[..4].try_into().expect("PNG chunk length")) as usize;
        let kind = &header[4..8];
        if kind == b"acTL" {
            return Err("animated PNG decoding is unavailable".into());
        }
        has_data |= kind == b"IDAT";
        let Some(next) = pos.checked_add(12).and_then(|p| p.checked_add(length)) else {
            return Err("invalid PNG chunk length".into());
        };
        if next > bytes.len() {
            break;
        }
        if kind == b"eXIf" {
            if let Some(exif) = image::metadata::Orientation::from_exif_chunk(&bytes[pos + 8..next - 4]) {
                metadata.orientation = exif.to_exif();
            }
        }
        if kind == b"IEND" {
            if !has_data || length != 0 {
                return Err("invalid PNG image end".into());
            }
            return Ok(Some(Track { metadata, ready: Some((0, None)) }));
        }
        pos = next;
    }
    if !has_data {
        return incomplete(finished);
    }
    Ok(Some(Track { metadata, ready: None }))
}

fn le16(bytes: &[u8]) -> u32 {
    u32::from(u16::from_le_bytes([bytes[0], bytes[1]]))
}

/// Return the byte after the zero-sized terminator, or None if more bytes
/// are needed. No per-block storage is retained.
fn subblocks(bytes: &[u8], mut pos: usize) -> Option<usize> {
    loop {
        let n = usize::from(*bytes.get(pos)?);
        pos += 1;
        if n == 0 {
            return Some(pos);
        }
        if n > bytes.len() - pos {
            return None;
        }
        pos += n;
    }
}

fn gif_track(bytes: &[u8], finished: bool, index: u32) -> Result<Option<Track>, String> {
    if signature(bytes, b"GIF87a").is_err() {
        signature(bytes, b"GIF89a")?;
    }
    if bytes.len() < 13 {
        return incomplete(finished);
    }
    let (width, height) = (le16(&bytes[6..8]), le16(&bytes[8..10]));
    let mut metadata = Metadata::still(width, height, 1)?;
    let table = |flag: u8| if flag & 0x80 != 0 { 3usize << ((flag & 7) + 1) } else { 0 };
    let mut pos = 13 + table(bytes[10]);
    let mut count = 0u32;
    let mut repeat = 0.0;
    let mut timestamp = 0i64;
    let mut delay = 0u64;
    let mut ready = None;
    let mut ended = false;
    while let Some(&block) = bytes.get(pos) {
        pos += 1;
        match block {
            0x3b => {
                ended = true;
                break;
            }
            0x21 => {
                let Some(&label) = bytes.get(pos) else { break };
                pos += 1;
                let Some(end) = subblocks(bytes, pos) else { break };
                let extension = &bytes[pos..end];
                match label {
                    // Delay is in hundredths of a second. An explicit zero
                    // stays zero in WebCodecs, including for the first frame.
                    0xf9 if extension.len() == 6 && extension[0] == 4 => {
                        delay = u64::from(le16(&extension[2..4])) * 10_000;
                    }
                    0xff if extension.len() >= 17
                        && extension[0] == 11
                        && matches!(&extension[1..12], b"NETSCAPE2.0" | b"ANIMEXTS1.0")
                        && extension[12..14] == [3, 1] =>
                    {
                        let n = le16(&extension[14..16]);
                        repeat = if n == 0 { f64::INFINITY } else { f64::from(n) };
                    }
                    // A plain-text rendering block consumes its graphics control.
                    0x01 => delay = 0,
                    _ => {}
                }
                pos = end;
            }
            0x2c => {
                let Some(desc) = bytes.get(pos..pos + 9) else { break };
                let (left, top, w, h) = (le16(desc), le16(&desc[2..]), le16(&desc[4..]), le16(&desc[6..]));
                if w == 0 || h == 0 || left + w > width || top + h > height {
                    return Err("GIF image outside its logical screen".into());
                }
                pos += 9 + table(desc[8]);
                let frame = count;
                count = count.checked_add(1).ok_or("too many GIF frames")?;
                if bytes.get(pos).is_none() {
                    break;
                }
                let Some(end) = subblocks(bytes, pos + 1) else { break };
                if frame == index {
                    ready = Some((timestamp, Some(delay)));
                }
                timestamp += delay as i64;
                delay = 0;
                pos = end;
            }
            _ => return Err("unknown GIF block".into()),
        }
    }
    if count == 0 || (finished && !ended) {
        return incomplete(finished);
    }
    metadata.animated = count > 1 || (!ended && repeat != 0.0);
    metadata.frame_count = count;
    metadata.repetition_count = if metadata.animated { repeat } else { 0.0 };
    if ended && !metadata.animated {
        ready = ready.map(|_| (0, None));
    }
    Ok(Some(Track { metadata, ready }))
}

/// Decode one complete frame. A pending frame in an unfinished stream
/// returns None. The optional target size is a codec hint; these paths
/// produce the original coded dimensions, as permitted by WebCodecs.
pub fn decode(
    kind: ImageType,
    bytes: &[u8],
    finished: bool,
    index: u32,
    _desired_size: Option<(u32, u32)>,
    color_convert: bool,
    page: &PageRasters,
) -> Result<Option<DecodedFrame>, String> {
    let Some(track) = track(kind, bytes, finished, index)? else { return Ok(None) };
    if index >= track.metadata.frame_count {
        return if finished { Err("image frame index out of range".into()) } else { Ok(None) };
    }
    let Some((timestamp, duration)) = track.ready else { return incomplete(finished) };
    let Metadata { width, height, mut orientation, .. } = track.metadata;
    let size = width as usize * height as usize * 4;
    let scratch = size.checked_mul(4).and_then(|n| n.checked_add(64 << 10)).ok_or("image scratch size overflow")?;
    let work = size.div_ceil(64).checked_mul(index as usize + 1).ok_or("image work size overflow")?;
    if !pyro_raster::capture::charge(work) {
        return Err("image exceeds processing budget".into());
    }
    let reservation = page.reserve(size).map_err(|e| e.to_string())?;
    let _scratch = page.reserve(scratch).map_err(|e| e.to_string())?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(crate::MAX_SIDE);
    limits.max_image_height = Some(crate::MAX_SIDE);
    limits.max_alloc = Some(scratch as u64);

    let rgba = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<_, String> {
        Ok(match kind {
            ImageType::Jpeg => {
                let (image, exif) = pyro_raster::decode_jpeg_with_budget(bytes, size + scratch, color_convert)
                    .map_err(|e| e.to_string())?;
                orientation = exif.to_exif();
                image.into_rgba8()
            }
            ImageType::Png => {
                let mut decoder = image::codecs::png::PngDecoder::with_limits(Cursor::new(bytes), limits)
                    .map_err(|e| e.to_string())?;
                let profile = if color_convert { decoder.icc_profile().map_err(|e| e.to_string())? } else { None };
                orientation = decoder.orientation().map_err(|e| e.to_string())?.to_exif();
                let mut rgba = DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?.into_rgba8();
                if let Some(profile) = profile {
                    pyro_raster::apply_icc_to_srgb(&profile, &mut rgba);
                }
                rgba
            }
            ImageType::Gif => {
                let mut decoder = image::codecs::gif::GifDecoder::new(Cursor::new(bytes)).map_err(|e| e.to_string())?;
                decoder.set_limits(limits).map_err(|e| e.to_string())?;
                // The shipped GIF decoder applies palettes, transparency and the
                // previous frame's disposal before yielding a full canvas.
                let mut frames = decoder.into_frames();
                for _ in 0..index {
                    frames.next().ok_or("missing preceding GIF frame")?.map_err(|e| e.to_string())?;
                }
                frames.next().ok_or("missing GIF frame")?.map_err(|e| e.to_string())?.into_buffer()
            }
        })
    }))
    .map_err(|_| "image decoder failed")??;
    if rgba.dimensions() != (width, height) || rgba.as_raw().len() != size {
        return Err("decoded dimensions differ from image header".into());
    }
    Ok(Some(DecodedFrame { width, height, rgba: rgba.into_raw(), orientation, timestamp, duration, reservation }))
}
