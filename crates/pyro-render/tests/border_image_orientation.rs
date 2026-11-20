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

#[test]
fn svg_jpeg_orientation_follows_the_fetch_response() {
    let pixels = (0..32).map(|index| if index < 16 { [255, 0, 0, 255] } else { [0, 0, 255, 255] }).collect::<Vec<_>>();
    let mut jpeg = encode_jpeg(&RgbaImage { width: 32, height: 1, data: pixels.concat() }, 100).unwrap();
    let marker = b"\xff\xe1\0\x22Exif\0\0MM\0\x2a\0\0\0\x08\0\x01\x01\x12\0\x03\0\0\0\x01\0\x02\0\0\0\0\0\0";
    jpeg.splice(2..2, marker.iter().copied());
    let snapshot = Snapshot {
        html: "<!doctype html><body style='margin:0'><svg width='32' height='1' style='display:block;image-orientation:none'><image href='photo.jpg' width='32' height='1'/></svg>".into(),
        width: 100, height: 100, device_scale_factor: 1.0, base_url: "http://test/".into(), ..Default::default()
    };
    let draw = |fetcher: Arc<dyn Fetcher>| Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0;
    let clean = draw(Arc::new(Image(jpeg.clone())));
    let opaque = draw(Arc::new(OpaqueImage(jpeg)));
    assert!(clean.data[0] > 240 && clean.data[2] < 10);
    assert!(opaque.data[0] < 10 && opaque.data[2] > 240);
}

#[test]
fn external_svg_jpeg_thumbnail_is_drawn() {
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new(&mut jpeg).encode(&vec![0; 6000 * 4000], 6000, 4000, image::ExtendedColorType::L8).unwrap();
    let thumbnails = "<image href='photo.jpg' width='100' height='67'/>".repeat(32);
    let snapshot = Snapshot {
        html: format!("<!doctype html><body style='margin:0;background:red'><svg width='100' height='67'>{thumbnails}</svg>"),
        width: 100, height: 67, device_scale_factor: 1.0, base_url: "http://test/".into(), ..Default::default()
    };
    let fetcher: Arc<dyn Fetcher> = Arc::new(Image(jpeg));
    let (image, decodes) = pyro_raster::with_test_budget(6000 * 4000 * 4 + 1024, || Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0);
    assert_eq!(decodes, 1);
    assert_eq!(&image.data[(33 * 100 + 50) * 4..][..4], &[0, 0, 0, 255]);
}
impl Fetcher for OpaqueImage {
    fn fetch(&self, _: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Ok(self.0.clone()));
    }
    fn fetch_final(&self, url: String, _: &FetchOptions, done: Box<dyn FnOnce(Result<Fetched, String>) + Send>) {
        done(Ok(Fetched { body: self.0.clone(), url, opaque: true, referrer_policy: None }))
    }
}

#[test]
fn svg_raster_routes_share_the_outer_budget() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use image::ImageFormat;
    let wrap = |href: &str| format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="23" height="27"><image href="{href}" width="23" height="27"/></svg>"#);
    let uri = |mime: &str, bytes: &[u8]| format!("data:{mime};base64,{}", STANDARD.encode(bytes));
    for format in [ImageFormat::Jpeg, ImageFormat::Png, ImageFormat::Gif, ImageFormat::WebP] {
        let mut output = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(23, 27).write_to(&mut output, format).unwrap();
        let bytes = output.into_inner();
        let image_uri = uri(format.to_mime_type(), &bytes);
        let embedded = wrap(&image_uri);
        let nested = wrap(&uri("image/svg+xml", embedded.as_bytes()));
        for (href, response) in [("photo", &bytes[..]), (&image_uri[..], &[][..]), ("drawing", embedded.as_bytes()), ("drawing", nested.as_bytes())] {
            let snapshot = Snapshot { html: format!("<!doctype html><body style='margin:0;background:red'>{}",wrap(href)), width: 32, height: 32, device_scale_factor: 1., base_url: "http://test/".into(), ..Default::default() };
            let render = || Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &(Arc::new(Image(response.to_vec())) as Arc<dyn Fetcher>)).unwrap().0;
            let (denied, decodes) = pyro_raster::with_test_budget(1024, render);
            assert_eq!(decodes, 0, "{format:?} {href}");
            assert_eq!(&denied.data[..4], &[255, 0, 0, 255]);
            assert_eq!(&render().data[..4], &[0, 0, 0, 255], "{format:?} {href}");
        }
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
