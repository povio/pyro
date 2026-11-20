//! Decoder hardening: files whose container (RIFF/VP8X canvas, ANMF frame, GIF logical screen) and
//! inner bitstream (VP8 / VP8L frame header, GIF image descriptor) disagree. `decode` must not
//! allocate more than the budget it was given admits, nor panic, for any of them; valid files of
//! the same shapes still decode.
//!
//! A counting allocator records each test thread's peak live allocation during a decode.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

struct Counting;

thread_local! {
    static LIVE: Cell<usize> = const { Cell::new(0) };
    static PEAK: Cell<usize> = const { Cell::new(0) };
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            grew(layout.size());
        }
        p
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() {
            grew(layout.size());
        }
        p
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        let _ = LIVE.try_with(|live| live.set(live.get().saturating_sub(layout.size())));
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if !p.is_null() {
            let _ = LIVE.try_with(|live| live.set(live.get().saturating_sub(layout.size())));
            grew(new_size);
        }
        p
    }
}

fn grew(size: usize) {
    let _ = LIVE.try_with(|live| {
        live.set(live.get() + size);
        let _ = PEAK.try_with(|peak| peak.set(peak.get().max(live.get())));
    });
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// What decoding `bytes` with `available` bytes of budget does: its result (Ok with the raster's
/// size, Err with the error, or "panic") and the peak bytes allocated above what was live before.
fn probe(bytes: &[u8], available: usize) -> (String, usize) {
    let base = LIVE.get();
    PEAK.set(base);
    let result = std::panic::catch_unwind(|| pyro_raster::decode(bytes, available));
    let peak = PEAK.get().saturating_sub(base);
    let outcome = match result {
        Ok(Ok(raster)) => format!("ok {}x{}", raster.width, raster.height),
        Ok(Err(error)) => format!("err {error}"),
        Err(_) => "panic".to_owned(),
    };
    (outcome, peak)
}

const VP8_20: &[u8] = include_bytes!("fixtures/vp8-20x20.bin");

fn chunk(fourcc: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = fourcc.to_vec();
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload);
    if payload.len() % 2 == 1 {
        out.push(0);
    }
    out
}

fn riff(chunks: &[Vec<u8>]) -> Vec<u8> {
    let body: Vec<u8> = chunks.concat();
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&(4 + body.len() as u32).to_le_bytes());
    out.extend_from_slice(b"WEBP");
    out.extend_from_slice(&body);
    out
}

fn u24(v: u32) -> [u8; 3] {
    [v as u8, (v >> 8) as u8, (v >> 16) as u8]
}

/// A VP8X chunk: `flags` (0x10 alpha, 0x02 animation) and the canvas size.
fn vp8x(flags: u8, width: u32, height: u32) -> Vec<u8> {
    let mut p = vec![flags, 0, 0, 0];
    p.extend_from_slice(&u24(width - 1));
    p.extend_from_slice(&u24(height - 1));
    chunk(b"VP8X", &p)
}

/// The 20x20 VP8 bitstream with its frame header claiming `width` x `height`.
fn vp8_claiming(width: u16, height: u16) -> Vec<u8> {
    let mut p = VP8_20.to_vec();
    p[6..8].copy_from_slice(&width.to_le_bytes());
    p[8..10].copy_from_slice(&height.to_le_bytes());
    chunk(b"VP8 ", &p)
}

/// An uncompressed ALPH chunk of `pixels` opaque samples.
fn alph(pixels: usize) -> Vec<u8> {
    let mut p = vec![0u8];
    p.extend(std::iter::repeat_n(0xff, pixels));
    chunk(b"ALPH", &p)
}

fn anim() -> Vec<u8> {
    chunk(b"ANIM", &[0, 0, 0, 0, 0, 0])
}

/// An ANMF chunk for a frame at (0, 0) of `width` x `height` holding `inner` chunks.
fn anmf(width: u32, height: u32, inner: &[Vec<u8>]) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&u24(0));
    p.extend_from_slice(&u24(0));
    p.extend_from_slice(&u24(width - 1));
    p.extend_from_slice(&u24(height - 1));
    p.extend_from_slice(&u24(100));
    p.push(0);
    for c in inner {
        p.extend_from_slice(c);
    }
    chunk(b"ANMF", &p)
}

/// A lossless VP8L chunk (from image's encoder) of a `width` x `height` image, its header
/// rewritten to claim `claim_w` x `claim_h`.
fn vp8l_claiming(width: u32, height: u32, claim_w: u32, claim_h: u32) -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgba8(width, height).write_to(&mut out, image::ImageFormat::WebP).unwrap();
    let file = out.into_inner();
    assert_eq!(&file[12..16], b"VP8L");
    let len = u32::from_le_bytes(file[16..20].try_into().unwrap()) as usize;
    let mut p = file[20..20 + len].to_vec();
    let header = u32::from_le_bytes(p[1..5].try_into().unwrap());
    let header = (header & !((1 << 28) - 1)) | (claim_w - 1) | ((claim_h - 1) << 14);
    p[1..5].copy_from_slice(&header.to_le_bytes());
    chunk(b"VP8L", &p)
}

/// Decodes an inconsistent file: it must be rejected (no panic) within `SMALL` bytes of allocation.
fn check(name: &str, file: &[u8], failures: &mut Vec<String>) {
    let (outcome, peak) = probe(file, BUDGET);
    eprintln!("{name}: {outcome}, peak {peak}");
    if !outcome.starts_with("err") || peak >= SMALL {
        failures.push(format!("{name}: {outcome}, allocated {peak} with a budget of {BUDGET}"));
    }
}

const BUDGET: usize = 1 << 20;
/// Allocation a rejected or tiny decode may take (parsing, small tables, a 20x20 frame).
const SMALL: usize = 256 << 10;

#[test]
fn valid_webp_shapes_decode() {
    // Simple lossy, extended lossy with alpha, and an animation whose frames match.
    for (name, file) in [
        ("simple", riff(&[chunk(b"VP8 ", VP8_20)])),
        ("vp8x+alph", riff(&[vp8x(0x10, 20, 20), alph(400), chunk(b"VP8 ", VP8_20)])),
        ("animated", riff(&[vp8x(0x12, 20, 20), anim(), anmf(20, 20, &[alph(400), chunk(b"VP8 ", VP8_20)])])),
        ("animated vp8", riff(&[vp8x(0x02, 20, 20), anim(), anmf(20, 20, &[chunk(b"VP8 ", VP8_20)])])),
    ] {
        let (outcome, peak) = probe(&file, BUDGET);
        assert_eq!(outcome, "ok 20x20", "{name}");
        assert!(peak < SMALL, "{name}: {peak}");
    }
}

#[test]
fn vp8_frame_larger_than_its_vp8x_canvas_is_rejected_before_allocating() {
    let mut failures = Vec::new();
    for (name, file) in [
        ("vp8x", riff(&[vp8x(0, 1, 1), vp8_claiming(16383, 16383)])),
        ("vp8x+alph", riff(&[vp8x(0x10, 20, 20), alph(400), vp8_claiming(16383, 16383)])),
        ("anmf vp8", riff(&[vp8x(0x02, 20, 20), anim(), anmf(20, 20, &[vp8_claiming(16383, 16383)])])),
        ("anmf alph", riff(&[vp8x(0x12, 20, 20), anim(), anmf(20, 20, &[alph(400), vp8_claiming(16383, 16383)])])),
    ] {
        check(name, &file, &mut failures);
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
fn animated_alpha_frame_whose_vp8_size_differs_is_rejected_without_panicking() {
    let mut failures = Vec::new();
    // ANMF says 1x1 (its ALPH has one sample); its VP8 is 20x20.
    for (name, file) in [
        ("larger vp8", riff(&[vp8x(0x12, 20, 20), anim(), anmf(1, 1, &[alph(1), chunk(b"VP8 ", VP8_20)])])),
        ("smaller frame, no alph", riff(&[vp8x(0x02, 20, 20), anim(), anmf(1, 1, &[chunk(b"VP8 ", VP8_20)])])),
    ] {
        check(name, &file, &mut failures);
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
fn vp8l_frame_whose_size_differs_from_its_container_is_rejected() {
    let mut failures = Vec::new();
    for (name, file) in [
        ("vp8x", riff(&[vp8x(0x10, 4, 4), vp8l_claiming(4, 4, 16384, 16384)])),
        ("anmf", riff(&[vp8x(0x12, 4, 4), anim(), anmf(4, 4, &[vp8l_claiming(4, 4, 16384, 16384)])])),
    ] {
        check(name, &file, &mut failures);
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// A GIF with a 2-color local table whose first frame is `width` x `height` on a `screen` x
/// `screen` logical screen, with no pixel data (the decode fails after setting up the frame).
fn gif(screen: u16, width: u16, height: u16) -> Vec<u8> {
    let mut g = b"GIF89a".to_vec();
    g.extend_from_slice(&screen.to_le_bytes());
    g.extend_from_slice(&screen.to_le_bytes());
    g.extend_from_slice(&[0, 0, 0]);
    g.push(0x2c);
    g.extend_from_slice(&[0, 0, 0, 0]);
    g.extend_from_slice(&width.to_le_bytes());
    g.extend_from_slice(&height.to_le_bytes());
    g.push(0x80);
    g.extend_from_slice(&[0, 0, 0, 255, 255, 255]);
    g.extend_from_slice(&[2, 2, 0x4c, 0x01, 0]);
    g.push(0x3b);
    g
}

#[test]
fn gif_frame_scratch_is_charged_to_the_budget() {
    // The frame does not fill the screen's width: image decodes it into a buffer of its own.
    let (outcome, peak) = probe(&gif(1, 8000, 8000), BUDGET);
    eprintln!("gif 8000x8000 frame on 1x1: {outcome}, peak {peak}");
    assert!(outcome.starts_with("err"), "{outcome}");
    assert!(peak < SMALL, "{peak}");
    // A frame inside the screen whose scratch fits: decodes (to the screen's size).
    let (outcome, _) = probe(&gif(64, 8, 8), BUDGET);
    assert!(outcome == "ok 64x64" || outcome.starts_with("err"), "{outcome}");
}

fn encoded(width: u32, height: u32, format: image::ImageFormat) -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgba8(width, height).write_to(&mut out, format).unwrap();
    out.into_inner()
}

#[test]
fn valid_files_of_every_format_decode() {
    for format in [image::ImageFormat::Png, image::ImageFormat::Gif, image::ImageFormat::WebP, image::ImageFormat::Jpeg] {
        let file = if format == image::ImageFormat::Jpeg {
            let mut out = std::io::Cursor::new(Vec::new());
            image::DynamicImage::new_rgb8(17, 19).write_to(&mut out, format).unwrap();
            out.into_inner()
        } else {
            encoded(17, 19, format)
        };
        let (outcome, _) = probe(&file, BUDGET);
        assert_eq!(outcome, "ok 17x19", "{format:?}");
    }
}

/// The JPEG's segments: (marker, offset of the marker, length of the segment including marker).
fn jpeg_segment(file: &[u8], marker: u8) -> (usize, usize) {
    let mut at = 2;
    while at + 4 <= file.len() {
        assert_eq!(file[at], 0xff);
        let len = usize::from(u16::from_be_bytes([file[at + 2], file[at + 3]]));
        if file[at + 1] == marker {
            return (at, len + 2);
        }
        at += len + 2;
    }
    panic!("no marker {marker:x}");
}

#[test]
fn jpeg_with_a_second_larger_frame_header_is_rejected() {
    // A second SOF0 claiming 16000 x 16000 after the first (the header check reads the first).
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(16, 16).write_to(&mut out, image::ImageFormat::Jpeg).unwrap();
    let file = out.into_inner();
    let (at, len) = jpeg_segment(&file, 0xc0);
    let mut sof = file[at..at + len].to_vec();
    sof[5..7].copy_from_slice(&16000u16.to_be_bytes());
    sof[7..9].copy_from_slice(&16000u16.to_be_bytes());
    let mut forged = file[..at + len].to_vec();
    forged.extend_from_slice(&sof);
    forged.extend_from_slice(&file[at + len..]);
    let mut failures = Vec::new();
    check("jpeg second SOF", &forged, &mut failures);
    assert!(failures.is_empty(), "{failures:#?}");
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 { 0xedb8_8320 ^ (crc >> 1) } else { crc >> 1 };
        }
    }
    !crc
}

fn png_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut out = (data.len() as u32).to_be_bytes().to_vec();
    let mut body = kind.to_vec();
    body.extend_from_slice(data);
    out.extend_from_slice(&body);
    out.extend_from_slice(&crc32(&body).to_be_bytes());
    out
}

#[test]
fn apng_frame_control_larger_than_its_header_is_rejected() {
    // A 1x1 PNG made an APNG whose first frame's fcTL (the default image) claims 16000 x 16000.
    let file = encoded(1, 1, image::ImageFormat::Png);
    let idat = file.windows(4).position(|w| w == b"IDAT").unwrap() - 4;
    let mut fctl = Vec::new();
    fctl.extend_from_slice(&0u32.to_be_bytes());
    fctl.extend_from_slice(&16000u32.to_be_bytes());
    fctl.extend_from_slice(&16000u32.to_be_bytes());
    fctl.extend_from_slice(&[0; 8]);
    fctl.extend_from_slice(&[0, 1, 0, 1, 0, 0]);
    let mut forged = file[..idat].to_vec();
    forged.extend_from_slice(&png_chunk(b"acTL", &[0, 0, 0, 1, 0, 0, 0, 0]));
    forged.extend_from_slice(&png_chunk(b"fcTL", &fctl));
    forged.extend_from_slice(&file[idat..]);
    let (outcome, peak) = probe(&forged, BUDGET);
    eprintln!("apng: {outcome}, peak {peak}");
    assert!(outcome.starts_with("err") || outcome == "ok 1x1", "{outcome}");
    assert!(peak < SMALL, "{peak}");
}
