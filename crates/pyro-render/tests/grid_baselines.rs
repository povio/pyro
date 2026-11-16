//! Grid baseline self-alignment (Taffy `compute/grid/baseline.rs`): `last baseline` groups align to the end of
//! their row, and `justify-self: baseline` groups to the side their block flow starts from (css-align-3 §9.1).
//! Pixel checks after WPT's `css/css-grid/alignment/grid-justify-baseline-001` and
//! `css/css-grid/alignment/self-baseline/grid-self-baseline-*`.
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

const WHITE: [u8; 3] = [255, 255, 255];
const GREEN: [u8; 3] = [0, 128, 0];
const BLUE: [u8; 3] = [0, 0, 255];

#[track_caller]
fn check(img: &RgbaImage, points: &[(usize, usize, [u8; 3])]) {
    for &(x, y, want) in points {
        assert_eq!(pixel(img, x, y), want, "pixel at ({x}, {y})");
    }
}

/// Two boxes without baselines (synthesized at their bottom border edges) in one row aligned on their last
/// baselines: the blue one's 20px bottom margin puts it at 20..40 beside the green 0..40, and the row is 60px tall
/// (the shims count in the row's size).
#[test]
fn last_baseline_aligns_a_rows_last_baselines() {
    let img = render(
        "<div style='display:grid;grid-template-columns:50px 50px;align-items:last baseline;width:200px'>\
           <div style='height:40px;background:green'></div>\
           <div style='height:20px;margin-bottom:20px;background:blue'></div>\
         </div>\
         <div style='width:10px;height:10px;background:green'></div>",
    );
    check(&img, &[(25, 10, GREEN), (25, 35, GREEN), (75, 15, WHITE), (75, 25, BLUE), (75, 39, BLUE), (5, 55, WHITE), (5, 65, GREEN)]);
}

/// `justify-self: baseline` in an `rtl` grid: a `vertical-rl` item's block flow starts on the right, the grid's
/// inline start, so it aligns there; a `vertical-lr` item's starts on the left, so its group aligns to the
/// inline end, the left side (with `start`, the old fallback, both sat on the right).
#[test]
fn justify_baseline_follows_the_items_block_flow() {
    let img = render(
        "<div style='display:grid;width:100px;direction:rtl;justify-items:baseline'>\
           <div style='writing-mode:vertical-rl;width:20px;height:20px;background:green'></div>\
           <div style='writing-mode:vertical-lr;width:20px;height:20px;background:blue'></div>\
         </div>",
    );
    check(&img, &[(90, 10, GREEN), (10, 10, WHITE), (10, 30, BLUE), (90, 30, WHITE)]);
}
