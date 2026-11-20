//! `image-orientation: none` on a border image (CSS Images 3 §5.3): the image is sliced as stored,
//! before its EXIF orientation turns it. Pyro turned border images by their orientation always
//! (WPT css/css-images/image-orientation/image-orientation-border-image).
use std::sync::Arc;

use pyro_render::*;

/// Serves `image` at every URL.
struct Image(Vec<u8>);

#[test]
fn bilinear_scaling_rounds_only_after_both_axes() {
    let pixels = [0, 1, 0, 0].map(|level| [level, level, level, 255]).concat();
    let fetcher: Arc<dyn Fetcher> = Arc::new(Image(encode_png(&RgbaImage { width: 2, height: 2, data: pixels })));
    let snapshot = Snapshot {
        html: "<!doctype html><body style='margin:0'><img style='display:block;width:3px;height:3px' src='tile.png'>".into(),
        width: 100, height: 100, device_scale_factor: 1.0, base_url: "http://test/".into(), ..Default::default()
    };
    let img = Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0;
    let center = (img.width as usize + 1) * 4;
    assert_eq!(&img.data[center..center + 4], &[0, 0, 0, 255]);
    assert_eq!(&img.data[8..12], &[1, 1, 1, 255]);
}
impl Fetcher for Image {
    fn fetch(&self, _: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Ok(self.0.clone()));
    }
}

/// A 2x1 PNG, red then blue, whose `eXIf` chunk says orientation 2 (mirrored): shown blue then red.
fn mirrored_png() -> Vec<u8> {
    let mut info = png::Info::with_size(2, 1);
    info.color_type = png::ColorType::Rgb;
    info.bit_depth = png::BitDepth::Eight;
    let mut out = Vec::new();
    let mut writer = png::Encoder::with_info(&mut out, info).unwrap().write_header().unwrap();
    // TIFF, big-endian: one IFD entry, Orientation (0x0112), SHORT, 1 value: 2.
    let exif = [
        0x4D, 0x4D, 0x00, 0x2A, 0x00, 0x00, 0x00, 0x08, 0x00, 0x01, 0x01, 0x12, 0x00, 0x03, 0x00, 0x00, 0x00, 0x01,
        0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    ];
    writer.write_chunk(png::chunk::ChunkType(*b"eXIf"), &exif).unwrap();
    writer.write_image_data(&[255, 0, 0, 0, 0, 255]).unwrap();
    writer.finish().unwrap();
    out
}

#[test]
fn image_orientation_none_slices_the_border_image_as_stored() {
    let fetcher: Arc<dyn Fetcher> = Arc::new(Image(mirrored_png()));
    let snapshot = Snapshot {
        html: "<!doctype html><body style='margin:0;background:#fff'>\
               <div style='width:40px;height:20px;border-image:url(http://test/i.png) 0 fill'></div>\
               <div style='width:40px;height:20px;border-image:url(http://test/i.png) 0 fill;image-orientation:none'></div>"
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
    let (blue, red) = ([0, 0, 255], [255, 0, 0]);
    assert_eq!([at(3, 10), at(36, 10)], [blue, red], "turned by its orientation");
    assert_eq!([at(3, 30), at(36, 30)], [red, blue], "as stored");
}

/// Serves `image` at every URL as an opaque response (a no-cors redirect chain that went to
/// another origin and came back to the document's).
struct OpaqueImage(Vec<u8>);
impl Fetcher for OpaqueImage {
    fn fetch(&self, _: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Ok(self.0.clone()));
    }
    fn fetch_final(&self, url: String, _: &FetchOptions, done: Box<dyn FnOnce(Result<Fetched, String>) + Send>) {
        done(Ok(Fetched { body: self.0.clone(), url, opaque: true, referrer_policy: None }))
    }
}

/// An opaque image (CSS Images 3 § 5.1) is shown as its EXIF orientation says whatever
/// `image-orientation` says, even when its final URL is of the document's origin: the response's
/// tainting decides, not the final URL (security review cycle 1, HIGH 3).
#[test]
fn an_opaque_image_at_the_documents_origin_keeps_its_orientation() {
    let fetcher: Arc<dyn Fetcher> = Arc::new(OpaqueImage(mirrored_png()));
    let snapshot = Snapshot {
        html: "<!doctype html><body style='margin:0;background:#fff'>\
               <div style='width:40px;height:20px;border-image:url(http://test/i.png) 0 fill;image-orientation:none'></div>"
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
    assert_eq!([at(3, 10), at(36, 10)], [[0, 0, 255], [255, 0, 0]], "turned by its orientation");
}
