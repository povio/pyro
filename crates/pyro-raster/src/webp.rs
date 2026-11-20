//! WebP container-versus-bitstream checks, run before image-webp (0.2.4) sees a file.
//!
//! image-webp trusts the container for the output size but decodes each VP8 bitstream at the size
//! of its own frame header: it allocates the VP8 planes (up to 16383 x 16383, ~400 MB) before it
//! compares that size with the VP8X canvas or the ANMF frame, and in an animated frame with an
//! ALPH chunk it never compares them, writing a larger frame into the frame-sized buffer (an
//! out-of-bounds panic). [`check`] walks the chunks exactly as the decoder does and rejects a file
//! whose VP8 / VP8L frame headers disagree with the size they are decoded into, before any of it
//! is decoded; it also returns the scratch memory decoding the first frame takes (what `decode`
//! charges to the caller's budget besides the final RGBA pixels).

use image::{error::DecodingError, ImageError, ImageFormat, ImageResult};

fn invalid(message: &str) -> ImageError {
    ImageError::Decoding(DecodingError::new(ImageFormat::WebP.into(), message.to_owned()))
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn u24_at(bytes: &[u8], at: usize) -> Option<u32> {
    let b = bytes.get(at..at + 3)?;
    Some(u32::from(b[0]) | u32::from(b[1]) << 8 | u32::from(b[2]) << 16)
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

/// A chunk header at `at`: its fourcc, the offset of its payload and its declared size.
fn chunk_at(bytes: &[u8], at: usize) -> Option<([u8; 4], usize, usize)> {
    let fourcc: [u8; 4] = bytes.get(at..at + 4)?.try_into().ok()?;
    Some((fourcc, at + 8, u32_at(bytes, at + 4)? as usize))
}

/// The size of the chunk's data as the decoder steps over it (odd sizes padded).
fn rounded(size: usize) -> usize {
    size + (size & 1)
}

/// The size a VP8 bitstream (a "VP8 " chunk's payload at `at`) decodes at: its key frame header's.
/// A bitstream too short for the header is left to the decoder, which fails before allocating.
fn vp8_size(bytes: &[u8], at: usize) -> ImageResult<Option<(u32, u32)>> {
    let Some(tag) = u24_at(bytes, at) else { return Ok(None) };
    if tag & 1 != 0 {
        // Not a key frame: WebP holds key frames only (libwebp rejects the rest).
        return Err(invalid("VP8 frame is not a key frame"));
    }
    if bytes.get(at + 3..at + 6).is_some_and(|magic| magic != [0x9d, 0x01, 0x2a]) {
        return Err(invalid("VP8 frame has no start code"));
    }
    match (u16_at(bytes, at + 6), u16_at(bytes, at + 8)) {
        (Some(w), Some(h)) => Ok(Some((u32::from(w & 0x3fff), u32::from(h & 0x3fff)))),
        _ => Ok(None),
    }
}

/// The size a VP8L bitstream (a "VP8L" chunk's payload at `at`) declares.
fn vp8l_size(bytes: &[u8], at: usize) -> Option<(u32, u32)> {
    let header = u32_at(bytes, at + 1)?;
    Some(((header & 0x3fff) + 1, ((header >> 14) & 0x3fff) + 1))
}

/// Requires the bitstream of chunk `fourcc` (payload at `at`) to decode at `want`.
fn bitstream_matches(bytes: &[u8], fourcc: &[u8; 4], at: usize, want: (u32, u32)) -> ImageResult<()> {
    let size = match fourcc {
        b"VP8 " => vp8_size(bytes, at)?,
        b"VP8L" => vp8l_size(bytes, at),
        _ => return Ok(()),
    };
    match size {
        Some(size) if size != want => Err(invalid("frame bitstream size differs from its container")),
        _ => Ok(()),
    }
}

/// The VP8 decoder's planes for a `w` x `h` frame (Y, U and V over whole macroblocks).
fn vp8_planes(w: u32, h: u32) -> usize {
    let (mbw, mbh) = (w.div_ceil(16) as usize, h.div_ceil(16) as usize);
    mbw * 16 * mbh * 16 + 2 * (mbw * 8 * mbh * 8)
}

/// Checks the file and returns the scratch bytes decoding its first frame takes besides the final
/// RGBA pixels of the canvas.
pub(crate) fn check(bytes: &[u8]) -> ImageResult<usize> {
    if bytes.get(0..4) != Some(b"RIFF") || bytes.get(8..12) != Some(b"WEBP") {
        return Ok(0);
    }
    let Some((fourcc, start, size)) = chunk_at(bytes, 12) else { return Ok(0) };
    match &fourcc {
        // Simple formats: the container's size is the bitstream's own header; RGB output for
        // lossy (converted to RGBA after), and an RGBA scratch for lossless without alpha.
        b"VP8 " => {
            let Some((w, h)) = vp8_size(bytes, start)? else { return Ok(0) };
            Ok(vp8_planes(w, h) + w as usize * h as usize * 3)
        }
        b"VP8L" => {
            let Some((w, h)) = vp8l_size(bytes, start) else { return Ok(0) };
            let alpha = u32_at(bytes, start + 1).is_some_and(|header| header >> 28 & 1 != 0);
            let pixels = w as usize * h as usize;
            Ok(if alpha { 0 } else { pixels * 3 + pixels * 4 })
        }
        b"VP8X" => extended(bytes, start, size),
        _ => Ok(0),
    }
}

fn extended(bytes: &[u8], start: usize, size: usize) -> ImageResult<usize> {
    let (Some(flags), Some(w), Some(h)) = (bytes.get(start), u24_at(bytes, start + 4), u24_at(bytes, start + 7)) else {
        return Ok(0);
    };
    let (canvas_w, canvas_h) = (w + 1, h + 1);
    let (alpha, animated) = (flags & 0x10 != 0, flags & 0x02 != 0);
    let canvas = canvas_w as usize * canvas_h as usize;
    // The decoder's output (RGBA with alpha, else RGB converted to RGBA after).
    let output = if alpha { 0 } else { canvas * 3 };
    // Every chunk after VP8X, as the decoder walks them; a superset of what it reads (it stops at
    // the RIFF size): every VP8 / VP8L it could decode is checked.
    let mut first_frame = None;
    let mut at = start + rounded(size);
    let mut frames = 0usize;
    let mut lossy = false;
    while let Some((fourcc, payload, size)) = chunk_at(bytes, at) {
        match &fourcc {
            b"VP8 " | b"VP8L" => {
                lossy |= &fourcc == b"VP8 ";
                bitstream_matches(bytes, &fourcc, payload, (canvas_w, canvas_h))?;
            }
            b"ANMF" => {
                frames += 1;
                first_frame.get_or_insert(at);
            }
            _ => {}
        }
        at = payload + rounded(size);
    }
    if !animated {
        let alpha_scratch = if alpha { canvas * 5 } else { 0 };
        let planes = if lossy { vp8_planes(canvas_w, canvas_h) } else { canvas * 4 };
        return Ok(output + planes + alpha_scratch);
    }
    // The frames, as the decoder steps from one to the next (by the declared ANMF size, unpadded).
    let mut scratch = 0;
    let mut at = first_frame.unwrap_or(usize::MAX);
    for index in 0..frames {
        let Some((fourcc, payload, anmf_size)) = chunk_at(bytes, at) else { break };
        if &fourcc != b"ANMF" {
            break;
        }
        let frame = (|| {
            let (x, y) = (u24_at(bytes, payload)? * 2, u24_at(bytes, payload + 3)? * 2);
            Some((x, y, u24_at(bytes, payload + 6)? + 1, u24_at(bytes, payload + 9)? + 1))
        })();
        let Some((x, y, fw, fh)) = frame else { break };
        if fw > 16384 || fh > 16384 || x + fw > canvas_w || y + fh > canvas_h {
            return Err(invalid("animation frame outside its canvas"));
        }
        // Its bitstream: VP8 or VP8L, or ALPH followed by VP8 (read where the decoder reads it).
        if let Some((fourcc, inner, inner_size)) = chunk_at(bytes, payload + 16) {
            let mut frame_alpha = false;
            if &fourcc == b"ALPH" {
                frame_alpha = true;
                match chunk_at(bytes, inner + rounded(inner_size)) {
                    Some((next, vp8, _)) if &next == b"VP8 " => bitstream_matches(bytes, b"VP8 ", vp8, (fw, fh))?,
                    Some(_) => return Err(invalid("animation frame's ALPH is not followed by VP8")),
                    None => {}
                }
            } else {
                bitstream_matches(bytes, &fourcc, inner, (fw, fh))?;
            }
            if index == 0 {
                let pixels = fw as usize * fh as usize;
                let planes = if &fourcc == b"VP8L" { 0 } else { vp8_planes(fw, fh) };
                let alpha_scratch = if frame_alpha { pixels * 5 } else { 0 };
                // The canvas it is composited on, the frame's own pixels, and what decoding them takes.
                scratch = output + canvas * 4 + pixels * 4 + planes + alpha_scratch;
            }
        }
        at = payload + anmf_size;
    }
    Ok(scratch)
}
