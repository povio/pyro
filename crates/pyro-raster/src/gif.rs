//! The scratch memory image's GIF decoder takes for a file's first frame, read from the file's
//! headers before decoding: the gif crate's buffer for the frame, and, when the frame does not fill
//! the logical screen's width from its left edge (or runs past its bottom), a frame-sized RGBA
//! buffer of image's own; a frame may be far larger than the screen (each side up to 65535).

/// Scratch bytes for the first frame besides the screen-sized RGBA output; 0 for a file whose
/// headers do not parse (the decoder fails on it before allocating frame memory).
pub(crate) fn scratch(bytes: &[u8]) -> usize {
    first_frame(bytes).map_or(0, |(screen_w, screen_h, left, top, w, h)| {
        let pixels = usize::from(w) * usize::from(h);
        let direct = left == 0 && w == screen_w && u32::from(top) + u32::from(h) <= u32::from(screen_h);
        pixels + if direct { 0 } else { pixels * 4 }
    })
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

/// The logical screen's size and the first image descriptor's position and size.
fn first_frame(bytes: &[u8]) -> Option<(u16, u16, u16, u16, u16, u16)> {
    if !bytes.starts_with(b"GIF87a") && !bytes.starts_with(b"GIF89a") {
        return None;
    }
    let (screen_w, screen_h) = (u16_at(bytes, 6)?, u16_at(bytes, 8)?);
    let flags = *bytes.get(10)?;
    let mut at = 13;
    if flags & 0x80 != 0 {
        at += 3 << ((flags & 7) + 1);
    }
    loop {
        match *bytes.get(at)? {
            // Extension: label, then data sub-blocks up to an empty one.
            0x21 => {
                at += 2;
                loop {
                    let len = usize::from(*bytes.get(at)?);
                    at += 1 + len;
                    if len == 0 {
                        break;
                    }
                }
            }
            0x2c => {
                return Some((
                    screen_w,
                    screen_h,
                    u16_at(bytes, at + 1)?,
                    u16_at(bytes, at + 3)?,
                    u16_at(bytes, at + 5)?,
                    u16_at(bytes, at + 7)?,
                ));
            }
            _ => return None,
        }
    }
}
