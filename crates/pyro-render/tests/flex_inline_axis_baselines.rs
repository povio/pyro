//! Flex baseline alignment by baseline-sharing group (Taffy `compute/flexbox.rs`, css-align-3 §9.1, Blink
//! `DetermineBaselineGroup`): in a column the cross axis is the container's inline axis, where a `vertical-rl`
//! item's first baseline group aligns to the right (its block-start) and a `vertical-lr` or horizontal one's to
//! the left; in a row, an item whose block flow runs the other way from the container's (`vertical-lr` in
//! `vertical-rl`) aligns to the line's other side. Boxes without text synthesize their baselines (at the line-left
//! edge across a horizontal container's inline axis, in the middle across a vertical container's block axis).
//! Pixel checks after WPT css-flexbox `align-items-baseline-column-horz` and `align-items-baseline-row-vert`.
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

/// A 200px-wide column with `align-items: baseline`, 20px boxes stacked from the top. The `vertical-rl` ones form
/// the group on the right: their baselines are synthesized at their left edges, so the green one's 10px right
/// margin shifts the blue one 10px left too (both at 170..190). The `vertical-lr` and horizontal ones form the
/// group on the left, their left edges lined up after the green `vertical-lr` box's 15px left margin (15..35).
/// (Before, column baselines fell back to `flex-start`: every box sat at the left edge of its margin box.)
#[test]
fn column_baseline_groups_follow_the_items_block_flow() {
    let img = render(
        "<div style='display:flex;flex-direction:column;align-items:baseline;width:200px'>\
           <div style='writing-mode:vertical-rl;width:20px;height:20px;margin-right:10px;background:green'></div>\
           <div style='writing-mode:vertical-rl;width:20px;height:20px;background:blue'></div>\
           <div style='writing-mode:vertical-lr;width:20px;height:20px;margin-left:15px;background:green'></div>\
           <div style='writing-mode:vertical-lr;width:20px;height:20px;background:blue'></div>\
           <div style='width:20px;height:20px;background:green'></div>\
         </div>",
    );
    check(
        &img,
        &[
            // `vertical-rl`: 170..190 at y 0..20 and 20..40
            (180, 10, GREEN),
            (165, 10, WHITE),
            (195, 10, WHITE),
            (180, 30, BLUE),
            (165, 30, WHITE),
            (195, 30, WHITE),
            // `vertical-lr` and horizontal: 15..35 at y 40..60, 60..80 and 80..100
            (25, 50, GREEN),
            (10, 50, WHITE),
            (25, 70, BLUE),
            (10, 70, WHITE),
            (40, 70, WHITE),
            (25, 90, GREEN),
            (10, 90, WHITE),
        ],
    );
}

/// A `vertical-rl` row (200px wide, 20px boxes stacked from the top) with `align-items: baseline`: baselines are
/// synthesized in the middle of the boxes. The `vertical-rl` boxes and the horizontal one align from the right,
/// the container's block-start: the green box's 10px right margin shifts the others to 170..190. The
/// `vertical-lr` boxes' block flow starts on the left, so they form the group there: the green one's 10px left
/// margin puts both at 10..30. (Before, every item was in the right group and the `vertical-lr` boxes sat at
/// 180..200.)
#[test]
fn vertical_rl_row_puts_vertical_lr_items_in_the_other_group() {
    let img = render(
        "<div style='display:flex;writing-mode:vertical-rl;align-items:baseline;width:200px;height:100px'>\
           <div style='width:20px;height:20px;margin-right:10px;background:green'></div>\
           <div style='width:20px;height:20px;background:blue'></div>\
           <div style='writing-mode:vertical-lr;width:20px;height:20px;margin-left:10px;background:green'></div>\
           <div style='writing-mode:vertical-lr;width:20px;height:20px;background:blue'></div>\
           <div style='writing-mode:horizontal-tb;width:20px;height:20px;background:green'></div>\
         </div>",
    );
    check(
        &img,
        &[
            // `vertical-rl`: 170..190 at y 0..20 and 20..40
            (180, 10, GREEN),
            (195, 10, WHITE),
            (180, 30, BLUE),
            (195, 30, WHITE),
            // `vertical-lr`: 10..30 at y 40..60 and 60..80
            (20, 50, GREEN),
            (5, 50, WHITE),
            (35, 50, WHITE),
            (180, 50, WHITE),
            (20, 70, BLUE),
            (5, 70, WHITE),
            (180, 70, WHITE),
            // horizontal (orthogonal, synthesized in the container's block axis): 170..190 at y 80..100
            (180, 90, GREEN),
            (195, 90, WHITE),
        ],
    );
}
