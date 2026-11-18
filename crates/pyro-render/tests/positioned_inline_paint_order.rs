//! A relative or sticky inline box that is not atomic paints its content in the positioned layer,
//! in tree order with the other positioned boxes (CSS 2.1 Appendix E step 8), not with its block's
//! in-flow content: an absolutely positioned box before it in the tree is painted under it, one
//! after it over it, and `z-index` sorts it. Its block leaves it to its paint-tree entry (blitz-dom
//! `NodeFlags::INLINE_PASS_ENTRY`, blitz-paint `draw_inline_pass_entry`). WPT css-position
//! `position-sticky-inline`, `position-sticky-nested-inline`.
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
        html: format!(
            "<!doctype html><style>body{{margin:0;background:#fff}}\
             .t{{font:20px/20px monospace}}</style>{body}"
        ),
        base_url: "http://test/".into(),
        width: 200,
        height: 100,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

fn pixel(img: &RgbaImage, x: usize, y: usize) -> [u8; 3] {
    let i = (y * img.width as usize + x) * 4;
    [img.data[i], img.data[i + 1], img.data[i + 2]]
}

const RED: [u8; 3] = [255, 0, 0];
const GREEN: [u8; 3] = [0, 128, 0];

/// The relative span's green background and text cover the absolutely positioned red box that comes before it in
/// the tree, though the span is part of its block's in-flow content.
#[test]
fn relative_span_paints_over_an_earlier_absolute_box() {
    let img = render(
        "<div style='position:relative'>\
           <div style='position:absolute;top:0;left:0;width:40px;height:20px;background:red'></div>\
           <span class=t style='position:relative;background:green;color:green'>XXXX</span>\
         </div>",
    );
    assert_eq!(pixel(&img, 20, 10), GREEN);
}

/// An absolutely positioned box after the span in the tree covers it.
#[test]
fn later_absolute_box_paints_over_a_relative_span() {
    let img = render(
        "<div style='position:relative'>\
           <span class=t style='position:relative;background:red;color:red'>XXXX</span>\
           <div style='position:absolute;top:0;left:0;width:40px;height:20px;background:green'></div>\
         </div>",
    );
    assert_eq!(pixel(&img, 20, 10), GREEN);
}

/// A negative `z-index` puts the span under a later absolute box, a positive one over it.
#[test]
fn z_index_sorts_a_relative_span() {
    let img = render(
        "<div style='position:relative;z-index:0'>\
           <span class=t style='position:relative;z-index:1;background:green;color:green'>XXXX</span>\
           <div style='position:absolute;top:0;left:0;width:40px;height:20px;background:red'></div>\
         </div>",
    );
    assert_eq!(pixel(&img, 20, 10), GREEN, "z-index 1 over the absolute box");
    let img = render(
        "<div style='position:relative;z-index:0'>\
           <div style='position:absolute;top:0;left:0;width:40px;height:20px;background:red'></div>\
           <span class=t style='position:relative;z-index:-1;background:green;color:green'>XXXX</span>\
         </div>",
    );
    assert_eq!(pixel(&img, 20, 10), RED, "z-index -1 under the absolute box");
}
