//! Natural image dimensions from file headers (no pixel decoding): PNG,
//! GIF, JPEG (SOFn markers) and WebP (VP8, VP8L, VP8X). Other formats,
//! including SVG, report `None`.

/// (width, height) in image pixels, or `None` when the bytes are not a
/// supported image or the header is truncated or invalid.
pub fn dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let dims =
        png(bytes).or_else(|| gif(bytes)).or_else(|| jpeg(bytes)).or_else(|| webp(bytes)).or_else(|| bmp(bytes))?;
    // An EXIF orientation that turns the image a quarter (5 to 8) shows it with its sides
    // exchanged (`image-orientation: from-image`, the initial value).
    let dims = if matches!(exif_orientation(bytes), Some(5..=8)) { (dims.1, dims.0) } else { dims };
    (dims.0 > 0 && dims.1 > 0).then_some(dims)
}

/// The EXIF Orientation tag (0x0112) of a JPEG (APP1 `Exif`) or a PNG (`eXIf` chunk).
fn exif_orientation(b: &[u8]) -> Option<u32> {
    let tiff = if b.starts_with(&[0xff, 0xd8]) {
        let mut i = 2;
        loop {
            if *b.get(i)? != 0xff {
                return None;
            }
            let marker = *b.get(i + 1)?;
            if marker == 0xff {
                i += 1;
                continue;
            }
            if marker == 0xd8 || marker == 0x01 || (0xd0..=0xd7).contains(&marker) {
                i += 2;
                continue;
            }
            let len = be16(b, i + 2)? as usize;
            if marker == 0xda || len < 2 {
                return None;
            }
            let segment = b.get(i + 4..i + 2 + len)?;
            if marker == 0xe1 && segment.starts_with(b"Exif\0\0") {
                break &segment[6..];
            }
            i += 2 + len;
        }
    } else if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        let mut i = 8;
        loop {
            let len = be32(b, i)? as usize;
            let kind = b.get(i + 4..i + 8)?;
            if kind == b"eXIf" {
                break b.get(i + 8..i + 8 + len)?;
            }
            if kind == b"IDAT" || kind == b"IEND" {
                return None;
            }
            i = i.checked_add(len)?.checked_add(12)?;
        }
    } else {
        return None;
    };
    let little = match tiff.get(0..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let u16_at = |at: usize| -> Option<u32> {
        let s: [u8; 2] = tiff.get(at..at + 2)?.try_into().ok()?;
        Some(if little { u16::from_le_bytes(s) } else { u16::from_be_bytes(s) } as u32)
    };
    let u32_at = |at: usize| -> Option<u32> {
        let s: [u8; 4] = tiff.get(at..at + 4)?.try_into().ok()?;
        Some(if little { u32::from_le_bytes(s) } else { u32::from_be_bytes(s) })
    };
    let ifd = u32_at(4)? as usize;
    let entries = u16_at(ifd)? as usize;
    (0..entries).find_map(|n| {
        let entry = ifd + 2 + n * 12;
        (u16_at(entry)? == 0x0112).then(|| u16_at(entry + 8)).flatten()
    })
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}
fn be16(b: &[u8], at: usize) -> Option<u32> {
    Some(u16::from_be_bytes(b.get(at..at + 2)?.try_into().ok()?) as u32)
}
fn le16(b: &[u8], at: usize) -> Option<u32> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?) as u32)
}
fn le24(b: &[u8], at: usize) -> Option<u32> {
    let s = b.get(at..at + 3)?;
    Some(s[0] as u32 | (s[1] as u32) << 8 | (s[2] as u32) << 16)
}

fn png(b: &[u8]) -> Option<(u32, u32)> {
    // Signature, then the IHDR chunk: length, "IHDR", width, height.
    if !b.starts_with(b"\x89PNG\r\n\x1a\n") || b.get(12..16)? != b"IHDR" {
        return None;
    }
    Some((be32(b, 16)?, be32(b, 20)?))
}

fn gif(b: &[u8]) -> Option<(u32, u32)> {
    if !(b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a")) {
        return None;
    }
    Some((le16(b, 6)?, le16(b, 8)?))
}

/// A Windows bitmap (`BM`, then a BITMAPINFOHEADER or later: width and
/// height as signed 32-bit values at 18 and 22, a negative height for rows
/// stored top down; the 12-byte BITMAPCOREHEADER has 16-bit ones).
fn bmp(b: &[u8]) -> Option<(u32, u32)> {
    if !b.starts_with(b"BM") {
        return None;
    }
    let le32 = |at: usize| Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?));
    if le32(14)? == 12 {
        return Some((le16(b, 18)?, le16(b, 20)?));
    }
    let (w, h) = (le32(18)? as i32, le32(22)? as i32);
    (w > 0 && h != 0 && h != i32::MIN).then(|| (w as u32, h.unsigned_abs()))
}

fn jpeg(b: &[u8]) -> Option<(u32, u32)> {
    if !b.starts_with(&[0xff, 0xd8]) {
        return None;
    }
    let mut i = 2;
    while i + 4 <= b.len() {
        if b[i] != 0xff {
            return None;
        }
        let marker = b[i + 1];
        // Fill bytes and standalone markers carry no length.
        if marker == 0xff {
            i += 1;
            continue;
        }
        if marker == 0xd8 || marker == 0x01 || (0xd0..=0xd7).contains(&marker) {
            i += 2;
            continue;
        }
        let len = be16(b, i + 2)? as usize;
        // SOF0-SOF15 except DHT (C4), JPG (C8) and DAC (CC).
        if (0xc0..=0xcf).contains(&marker) && !matches!(marker, 0xc4 | 0xc8 | 0xcc) {
            return Some((be16(b, i + 7)?, be16(b, i + 5)?));
        }
        if marker == 0xda || len < 2 {
            return None;
        }
        i += 2 + len;
    }
    None
}

fn webp(b: &[u8]) -> Option<(u32, u32)> {
    if b.get(0..4)? != b"RIFF" || b.get(8..12)? != b"WEBP" {
        return None;
    }
    match b.get(12..16)? {
        b"VP8 " => {
            // Frame tag (3), start code 9D 01 2A, then 14-bit sizes.
            if b.get(23..26)? != [0x9d, 0x01, 0x2a] {
                return None;
            }
            Some((le16(b, 26)? & 0x3fff, le16(b, 28)? & 0x3fff))
        }
        b"VP8L" => {
            if *b.get(20)? != 0x2f {
                return None;
            }
            let bits = u32::from_le_bytes(b.get(21..25)?.try_into().ok()?);
            Some(((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1))
        }
        b"VP8X" => Some((le24(b, 24)? + 1, le24(b, 27)? + 1)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::dimensions;

    #[test]
    fn reads_png_gif_jpeg_and_webp_headers() {
        // The 1x1 PNG TOS chat uploads.
        let png = [
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13, b'I', b'H', b'D', b'R', 0, 0, 0, 1, 0, 0, 0,
            1, 8, 0,
        ];
        assert_eq!(dimensions(&png), Some((1, 1)));
        let mut wide = png;
        wide[16..24].copy_from_slice(&[0, 0, 2, 0x80, 0, 0, 1, 0xe0]);
        assert_eq!(dimensions(&wide), Some((640, 480)));
        assert_eq!(dimensions(b"GIF89a\x20\x03\x58\x02rest"), Some((800, 600)));
        // SOI, APP0 (length 16), SOF2 (progressive): height 0x01e0, width 0x0280.
        let mut jpeg = vec![0xff, 0xd8, 0xff, 0xe0, 0, 16];
        jpeg.extend_from_slice(&[0; 14]);
        jpeg.extend_from_slice(&[0xff, 0xc2, 0, 17, 8, 0x01, 0xe0, 0x02, 0x80, 3]);
        assert_eq!(dimensions(&jpeg), Some((640, 480)));
        let mut vp8x = b"RIFF\0\0\0\0WEBPVP8X\x0a\0\0\0\0\0\0\0".to_vec();
        vp8x.extend_from_slice(&[0x7f, 0x02, 0x00, 0xdf, 0x01, 0x00]);
        assert_eq!(dimensions(&vp8x), Some((640, 480)));
        let mut vp8l = b"RIFF\0\0\0\0WEBPVP8L\0\0\0\0\x2f".to_vec();
        // 14-bit (width-1)=639, (height-1)=479.
        let bits: u32 = 639 | (479 << 14);
        vp8l.extend_from_slice(&bits.to_le_bytes());
        assert_eq!(dimensions(&vp8l), Some((640, 480)));
        let mut vp8 = b"RIFF\0\0\0\0WEBPVP8 \0\0\0\0\0\0\0\x9d\x01\x2a".to_vec();
        vp8.extend_from_slice(&[0x80, 0x02, 0xe0, 0x01]);
        assert_eq!(dimensions(&vp8), Some((640, 480)));
    }

    #[test]
    fn reads_bmp_headers() {
        let mut bmp = vec![0u8; 54];
        bmp[..2].copy_from_slice(b"BM");
        bmp[14] = 40;
        bmp[18..22].copy_from_slice(&300i32.to_le_bytes());
        bmp[22..26].copy_from_slice(&2i32.to_le_bytes());
        assert_eq!(dimensions(&bmp), Some((300, 2)));
        // Rows stored top down.
        bmp[22..26].copy_from_slice(&(-7i32).to_le_bytes());
        assert_eq!(dimensions(&bmp), Some((300, 7)));
        assert_eq!(dimensions(&bmp[..24]), None);
        bmp[18..22].copy_from_slice(&0i32.to_le_bytes());
        assert_eq!(dimensions(&bmp), None);
    }

    #[test]
    fn rejects_unknown_truncated_and_empty_images() {
        assert_eq!(dimensions(b""), None);
        assert_eq!(dimensions(b"<svg width='10' height='10'/>"), None);
        assert_eq!(dimensions(b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR\0\0"), None);
        assert_eq!(dimensions(b"GIF89a\0\0\0\0"), None, "zero-size images are broken");
        assert_eq!(dimensions(&[0xff, 0xd8, 0xff, 0xda, 0, 2]), None, "scan before any SOF");
    }
}
