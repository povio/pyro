//! An `<object>` hosting a document: the render snapshot gives it its frame document as `srcdoc`
//! (pyro-dom `snapshot.rs`), which is laid out and painted as an `<iframe>`'s, in place of the
//! object's fallback content.
use std::sync::Arc;

use pyro_render::*;

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}

fn render(body: &str) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let snapshot = Snapshot {
        html: format!("<!doctype html><body style='margin:0;background:#fff'>{body}"),
        base_url: "http://test/".into(),
        width: 400,
        height: 300,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

fn pixel(img: &RgbaImage, x: usize, y: usize) -> [u8; 3] {
    let i = (y * img.width as usize + x) * 4;
    [img.data[i], img.data[i + 1], img.data[i + 2]]
}

#[test]
fn an_object_with_a_frame_document_paints_it_instead_of_its_fallback() {
    let doc = "&lt;body style='margin:0;background:#008000'&gt;";
    let img = render(&format!(
        "<object type='text/html' srcdoc=\"{doc}\" data-pyro-frame-url='http://test/d.html' style='display:block;width:200px;height:100px;border:0'>\
         <div style='width:300px;height:200px;background:#f00'></div></object>"
    ));
    assert_eq!(pixel(&img, 10, 10), [0, 128, 0]);
    assert_eq!(pixel(&img, 199, 99), [0, 128, 0]);
    // The object's box, not its fallback's.
    assert_eq!(pixel(&img, 250, 150), [255, 255, 255]);
}

#[test]
fn an_inline_object_sized_by_its_attributes_paints_its_frame_document() {
    let doc = "&lt;body style='margin:0;background:#008000'&gt;";
    let img = render(&format!(
        "<object type='text/html' srcdoc=\"{doc}\" data-pyro-frame-url='http://test/d.html' width=200 height=100 style='border:0;vertical-align:top'>fallback</object>"
    ));
    assert_eq!(pixel(&img, 10, 10), [0, 128, 0]);
    assert_eq!(pixel(&img, 199, 99), [0, 128, 0]);
    assert_eq!(pixel(&img, 201, 50), [255, 255, 255]);
}

#[test]
fn an_embed_with_a_frame_document_paints_it() {
    let doc = "&lt;body style='margin:0;background:#008000'&gt;";
    let img = render(&format!(
        "<embed type='text/html' srcdoc=\"{doc}\" data-pyro-frame-url='http://test/d.html' width=200 height=100 style='vertical-align:top'>"
    ));
    assert_eq!(pixel(&img, 10, 10), [0, 128, 0]);
    assert_eq!(pixel(&img, 199, 99), [0, 128, 0]);
    assert_eq!(pixel(&img, 201, 50), [255, 255, 255]);
}
