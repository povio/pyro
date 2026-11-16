//! A replaced flex item's natural aspect ratio is its preferred aspect ratio (css-sizing-4 §5.1;
//! Taffy `compute/flexbox.rs`): a definite cross size gives its flex base size and its main size
//! through the ratio, between content boxes (css-flexbox-1 §9.2 B, §4.5). Canvases have a natural
//! ratio from their `width` and `height` attributes. Pixel checks after WPT's
//! `css/css-flexbox/flex-aspect-ratio-img-row-012` and `relayout-intrinsic-block-size`.
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

/// A 100px height through the square ratio: 100px wide, not the 150px the `max-width` left of
/// its natural 200px width (its content size suggestion).
#[test]
fn a_definite_height_gives_a_flex_rows_item_its_width_through_its_ratio() {
    let img = render(
        "<div style='display:flex'>\
           <canvas width=200 height=200 style='height:100px;max-width:150px;background:green'></canvas>\
         </div>",
    );
    check(&img, &[(95, 50, GREEN), (105, 50, WHITE)]);
}

/// Stretched to the container's 100px height, the square canvas is 100px wide, not its natural
/// 200px (its flex base size and minimum through the ratio).
#[test]
fn a_stretched_items_width_follows_its_ratio() {
    let img = render(
        "<div style='display:flex;height:100px'>\
           <canvas width=200 height=200 style='background:green'></canvas>\
         </div>",
    );
    check(&img, &[(95, 50, GREEN), (105, 50, WHITE)]);
}

/// The ratio is between content boxes: a 50px content height and 10px borders make a 70px square.
#[test]
fn the_natural_ratio_is_the_content_boxes() {
    let img = render(
        "<div style='display:flex'>\
           <canvas width=100 height=100 style='height:50px;border:10px solid blue;background:green'></canvas>\
         </div>",
    );
    check(&img, &[(35, 35, GREEN), (65, 35, BLUE), (75, 35, WHITE), (35, 65, BLUE), (35, 75, WHITE)]);
}

/// In a column container sized to its content (a float), a stretched item with `aspect-ratio: 1/1`
/// and `min-height: 100px` is measured at its fit-content width, not the available width: the
/// container and the item are a 100px square, not a 300px one (§9.2 E; WPT
/// `css/css-flexbox/flex-item-transferred-sizes-padding-border-sizing`).
#[test]
fn a_stretched_ratio_item_in_a_content_sized_column_is_fit_content_wide() {
    let img = render(
        "<div style='display:flex;flex-direction:column;float:left'>\
           <div style='background:green;min-height:100px;aspect-ratio:1/1'></div>\
         </div>",
    );
    check(&img, &[(95, 95, GREEN), (105, 50, WHITE), (50, 105, WHITE)]);
}

/// The same for a canvas's natural ratio in an `inline-flex` column: it stays at its natural 100px
/// (WPT `css/css-flexbox/inline-flex-column-image-load`).
#[test]
fn a_stretched_canvas_in_a_content_sized_column_keeps_its_natural_size() {
    let img = render(
        "<div style='display:inline-flex;flex-direction:column;vertical-align:top'>\
           <canvas width=100 height=100 style='background:green'></canvas>\
         </div>",
    );
    check(&img, &[(95, 95, GREEN), (105, 50, WHITE), (50, 105, WHITE)]);
}

/// A canvas with a natural 16px square size and `height: 30px` in a flex container of no width: its
/// automatic minimum width is the 30px its height gives it through its ratio (Blink's min-content
/// width with a definite height), not its natural 16px (WPT `flexbox-min-width-auto-002a`).
#[test]
fn a_definite_heights_ratio_width_is_the_automatic_minimum() {
    let img = render(
        "<div style='display:flex;width:0'>\
           <canvas width=16 height=16 style='width:100px;height:30px;background:green'></canvas>\
         </div>",
    );
    check(&img, &[(28, 15, GREEN), (32, 15, WHITE)]);
}
