use image::{ExtendedColorType, ImageEncoder};
use pyro_canvas::image_decoder::{decode, metadata, ImageType};
use pyro_raster::PageRasters;

#[test]
fn png_headers_and_pixels_are_available_at_distinct_stream_boundaries() {
    let mut png = Vec::new();
    let rgba = [255, 0, 0, 255, 12, 34, 56, 128];
    image::codecs::png::PngEncoder::new(&mut png).write_image(&rgba, 2, 1, ExtendedColorType::Rgba8).unwrap();
    let page = PageRasters::default();
    assert!(metadata(ImageType::Png, &png[..7], false).unwrap().is_none());
    assert!(decode(ImageType::Png, &png[..png.len() - 12], false, 0, None, true, &page).unwrap().is_none());
    let meta = metadata(ImageType::Png, &png[..png.len() - 12], false).unwrap().unwrap();
    assert_eq!((meta.width, meta.height, meta.frame_count), (2, 1, 1));
    assert!(!meta.animated);
    let output = decode(ImageType::Png, &png, true, 0, Some((1, 1)), true, &page).unwrap().unwrap();
    assert_eq!(
        (output.width, output.height, output.orientation, output.timestamp, output.duration),
        (2, 1, 1, 0, None)
    );
    assert_eq!(output.rgba, rgba);
    assert_eq!(page.used(), rgba.len());
    drop(output);
    assert_eq!(page.used(), 0);
}

#[test]
fn jpeg_yields_unrotated_pixels_and_the_original_exif_transform() {
    let mut jpeg = Vec::new();
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 95);
    // Little-endian TIFF IFD0, one SHORT orientation tag (rotate 90 CW).
    encoder
        .set_exif_metadata(vec![73, 73, 42, 0, 8, 0, 0, 0, 1, 0, 18, 1, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0, 0, 0, 0, 0])
        .unwrap();
    let rgb = [224, 32, 16].repeat(16 * 8);
    encoder.encode(&rgb, 16, 8, ExtendedColorType::Rgb8).unwrap();
    let page = PageRasters::default();
    let meta = metadata(ImageType::Jpeg, &jpeg, true).unwrap().unwrap();
    assert_eq!((meta.width, meta.height, meta.orientation), (16, 8, 6));
    let output = decode(ImageType::Jpeg, &jpeg, true, 0, None, true, &page).unwrap().unwrap();
    assert_eq!((output.width, output.height, output.orientation, output.duration), (16, 8, 6, None));
    assert!(output
        .rgba
        .chunks_exact(4)
        .all(|p| p[0].abs_diff(224) <= 3 && p[1].abs_diff(32) <= 3 && p[2].abs_diff(16) <= 3 && p[3] == 255));
    // The preexisting whole-image entry point uses the identical decoder.
    let (whole, orientation) = pyro_raster::decode_jpeg(&jpeg).unwrap();
    assert_eq!(orientation.to_exif(), 6);
    assert_eq!(whole.into_rgba8().into_raw(), output.rgba);
}

/// A tiny valid GIF with partial rectangles, a transparent index and both
/// Restore-to-Previous and Restore-to-Background disposal modes.
fn animation() -> (Vec<u8>, usize) {
    let mut gif = b"GIF89a\x03\x00\x01\x00\x81\x00\x00".to_vec();
    gif.extend_from_slice(&[0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255]);
    gif.extend_from_slice(b"\x21\xff\x0bNETSCAPE2.0\x03\x01\x02\x00\x00");
    let mut first_end = 0;
    for (index, (left, indices, delay, disposal)) in
        [(0u8, &[1u8, 1, 1][..], 0u8, 1u8), (1, &[2][..], 2, 3), (2, &[3][..], 3, 2), (0, &[0][..], 4, 1)]
            .into_iter()
            .enumerate()
    {
        gif.extend_from_slice(&[0x21, 0xf9, 4, (disposal << 2) | 1, delay, 0, 0, 0]);
        gif.extend_from_slice(&[0x2c, left, 0, 0, 0, indices.len() as u8, 0, 1, 0, 0]);
        // A clear before each literal keeps the LZW codes three bits wide.
        let mut code_bits = 0u32;
        let mut nbits = 0;
        for &pixel in indices {
            code_bits |= (4 | (u32::from(pixel) << 3)) << nbits;
            nbits += 6;
        }
        code_bits |= 5 << nbits;
        let len = (nbits + 3 + 7) / 8;
        gif.extend_from_slice(&[2, len as u8]);
        gif.extend_from_slice(&code_bits.to_le_bytes()[..len]);
        gif.push(0);
        if index == 0 {
            first_end = gif.len();
        }
    }
    gif.push(0x3b);
    (gif, first_end)
}

#[test]
fn gif_partial_frames_keep_zero_delay_disposal_transparency_and_loop_count() {
    let (gif, first_end) = animation();
    let page = PageRasters::default();
    let partial = metadata(ImageType::Gif, &gif[..first_end], false).unwrap().unwrap();
    assert_eq!(partial.frame_count, 1);
    assert!(decode(ImageType::Gif, &gif[..first_end], false, 1, None, true, &page).unwrap().is_none());
    let first = decode(ImageType::Gif, &gif[..first_end], false, 0, None, true, &page).unwrap().unwrap();
    assert_eq!((first.timestamp, first.duration), (0, Some(0)));
    drop(first);
    let meta = metadata(ImageType::Gif, &gif, true).unwrap().unwrap();
    assert!(meta.animated);
    assert_eq!((meta.width, meta.height, meta.frame_count, meta.repetition_count), (3, 1, 4, 2.0));
    let red = [255, 0, 0, 255];
    let green = [0, 255, 0, 255];
    let blue = [0, 0, 255, 255];
    let transparent = [0, 0, 0, 0];
    for (index, timestamp, duration, pixels) in [
        (3, 50_000, 40_000, [red, red, transparent]),
        (1, 0, 20_000, [red, green, red]),
        (2, 20_000, 30_000, [red, red, blue]),
        (0, 0, 0, [red, red, red]),
    ] {
        let out = decode(ImageType::Gif, &gif, true, index, None, true, &page).unwrap().unwrap();
        assert_eq!((out.timestamp, out.duration), (timestamp, Some(duration)));
        assert_eq!(out.rgba, pixels.concat());
    }
    assert_eq!(page.used(), 0);
}
