//! Lines of `vertical-lr` with `text-orientation: sideways` are inverted: the glyphs' tops (the
//! line-over side) face block-end, so the alphabetic baseline and the line-under side are toward
//! the left. Their inline boxes and the baselines their root exports are turned as their text is
//! painted (blitz-dom `sideways_inverted_lines`), and a horizontal grid synthesizes inline-axis
//! baselines at the left, where such lines' under side is (stylo_taffy `baseline_synthesis`).
//! Pixel checks after WPT css-flexbox `baseline-synthesis-003`, css-grid `grid-self-baseline-006`.
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
        width: 300,
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

const WHITE: [u8; 3] = [255, 255, 255];
const GREEN: [u8; 3] = [0, 128, 0];

#[track_caller]
fn check(img: &RgbaImage, points: &[(usize, usize, [u8; 3])]) {
    for &(x, y, want) in points {
        assert_eq!(pixel(img, x, y), want, "pixel at ({x}, {y})");
    }
}

/// An empty item synthesizes its baseline at its line-under (left) edge; a line of zero height
/// holding a 10px inline-block has its baseline at its left edge too (the box sits over it, to
/// the right), so both items start at the left and fill a 100px square with no red showing.
#[test]
fn sideways_vertical_lr_line_baseline_is_on_the_left() {
    let img = render(
        "<div style='display:flex;align-items:baseline;writing-mode:vertical-lr;text-orientation:sideways;background:red'>\
           <div style='height:50px;width:100px;background:green'></div>\
           <div style='height:50px;width:100px;background:green;line-height:0'>\
             <span style='width:10px;height:10px;display:inline-block'></span>\
           </div>\
         </div>",
    );
    check(&img, &[(1, 1, GREEN), (98, 25, GREEN), (1, 75, GREEN), (98, 98, GREEN), (5, 52, GREEN), (95, 52, GREEN)]);
}

/// `justify-self: baseline` in a horizontal grid aligns items without a vertical baseline at
/// their left edges, as inline-blocks on a sideways `vertical-lr` line are.
#[test]
fn horizontal_grid_synthesizes_inline_baselines_at_the_left() {
    let img = render(
        "<div style='display:grid;grid-template-columns:100px;justify-items:baseline'>\
           <div style='width:20px;height:10px;background:green'></div>\
           <div style='width:60px;height:10px;background:green'></div>\
         </div>",
    );
    check(&img, &[(1, 5, GREEN), (1, 15, GREEN), (19, 5, GREEN), (25, 5, WHITE), (59, 15, GREEN), (65, 15, WHITE)]);
}
