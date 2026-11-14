//! A raster image's ICC profile converts its pixels to sRGB when it is decoded (CSS Color 4,
//! "Color spaces of tagged images"), as Chromium draws them for an sRGB screen (WPT
//! css/css-color/tagged-images-*): a #990000 image tagged with an sRGB profile whose red and green
//! are swapped is drawn #009900. An image tagged sRGB, or untagged, keeps its pixels exactly.
use std::sync::Arc;

use pyro_render::*;

/// Serves `image` at every URL.
struct Image(Vec<u8>);
impl Fetcher for Image {
    fn fetch(&self, _: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Ok(self.0.clone()));
    }
}

/// A 4x4 PNG of one colour, with `icc` as its iCCP chunk.
fn png(rgb: [u8; 3], icc: Option<Vec<u8>>) -> Vec<u8> {
    let mut info = png::Info::with_size(4, 4);
    info.color_type = png::ColorType::Rgb;
    info.bit_depth = png::BitDepth::Eight;
    info.icc_profile = icc.map(Into::into);
    let mut out = Vec::new();
    let mut writer = png::Encoder::with_info(&mut out, info).unwrap().write_header().unwrap();
    writer.write_image_data(&rgb.repeat(16)).unwrap();
    writer.finish().unwrap();
    out
}

/// The sRGB profile with its red and green colorants swapped (WPT's swapped.icc).
fn swapped_srgb() -> Vec<u8> {
    let mut profile = moxcms::ColorProfile::new_srgb();
    std::mem::swap(&mut profile.red_colorant, &mut profile.green_colorant);
    profile.encode().unwrap()
}

/// The colour `image` is drawn in, as an `<img>` and as a background.
fn drawn(image: Vec<u8>) -> [[u8; 3]; 2] {
    let fetcher: Arc<dyn Fetcher> = Arc::new(Image(image));
    let snapshot = Snapshot {
        html: "<!doctype html><body style='margin:0;background:#fff'>\
               <img src='http://test/i.png' style='display:block;width:40px;height:40px'>\
               <div style='width:40px;height:40px;background:url(http://test/i.png) 0 0 / 40px 40px'></div>"
            .into(),
        base_url: "http://test/".into(),
        width: 100,
        height: 100,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    let img = Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0;
    let at = |x: usize, y: usize| {
        let i = (y * img.width as usize + x) * 4;
        [img.data[i], img.data[i + 1], img.data[i + 2]]
    };
    [at(20, 20), at(20, 60)]
}

#[test]
fn tagged_images_are_drawn_in_srgb() {
    let near = |a: [u8; 3], b: [u8; 3]| a.iter().zip(b).all(|(a, b)| a.abs_diff(b) <= 1);
    for colour in drawn(png([0x99, 0, 0], Some(swapped_srgb()))) {
        assert!(near(colour, [0, 0x99, 0]), "swapped profile: {colour:?}");
    }
    let srgb = moxcms::ColorProfile::new_srgb().encode().unwrap();
    assert_eq!(drawn(png([0, 0x99, 0], Some(srgb))), [[0, 0x99, 0]; 2], "an sRGB profile changes nothing");
    assert_eq!(drawn(png([0, 0x99, 0], None)), [[0, 0x99, 0]; 2], "untagged");
}
