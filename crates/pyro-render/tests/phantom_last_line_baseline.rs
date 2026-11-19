//! An inline-block's baseline is that of its last line box that is not a phantom one: a line after
//! a `<br>` holding only a float and collapsed white space has none (CSS Inline 3 § 4.1; blitz-dom
//! inline layout `last_line_index`). WPT css-flexbox `flexbox-baseline-multi-line-vert-001/-002`,
//! whose references are such inline-blocks.
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
        width: 200,
        height: 200,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

fn pixel(img: &RgbaImage, x: usize, y: usize) -> [u8; 3] {
    let i = (y * img.width as usize + x) * 4;
    [img.data[i], img.data[i + 1], img.data[i + 2]]
}

const GREEN: [u8; 3] = [0, 128, 0];
const BLUE: [u8; 3] = [0, 0, 255];

/// A 10px green box sits on the line's baseline beside an inline-block whose first line holds a
/// 20px-tall box of 10px text (baseline about 8px down). The float-only line after the `<br>`
/// would otherwise put the inline-block's baseline at the bottom of that first line, 20px down.
#[test]
fn phantom_line_after_br_gives_no_baseline() {
    let img = render(
        "<div style='font:10px/10px monospace'>\
           <span style='display:inline-block;width:10px;height:10px;background:green'></span>\
           <div style='display:inline-block;width:40px;height:40px'>\
             <span style='display:inline-block;width:10px;height:20px;background:blue;color:blue'>x</span><br>\
             <span style='float:left'></span>\
           </div>\
         </div>",
    );
    let blue_x = (0..100).find(|&x| (0..100).any(|y| pixel(&img, x, y) == BLUE)).expect("blue box");
    let top = (0..100).find(|&y| pixel(&img, blue_x, y) == BLUE).expect("blue top");
    assert_eq!(pixel(&img, 5, top + 3), GREEN, "green box ends at the first line's baseline");
    assert_ne!(pixel(&img, 5, top + 15), GREEN, "not at the bottom of the first line");
}
