//! Flex content sizes and line sizing (Taffy `compute/block.rs`, `compute/flexbox.rs`): a block's
//! content size ignores its own `max-width`, keyword minimums bound automatic widths, the minimum
//! wins over a smaller maximum in a column's size, a column's min-content size is one line's, an
//! overflowing `space-between` falls back to `flex-start`. Pixel checks after WPT css-flexbox
//! `flex-base-size-ignores-max-width`, `flex-item-content-is-min-width-max-content`,
//! `flex-item-min-block-size-intrinsic-max-block-size`, `flex-minimum-height-flex-items-029`,
//! `justify-content_space-between-003.tentative`.
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

#[track_caller]
fn check(img: &RgbaImage, points: &[(usize, usize, [u8; 3])]) {
    for &(x, y, want) in points {
        assert_eq!(pixel(img, x, y), want, "pixel at ({x}, {y})");
    }
}

/// `min-width: min-content` (100px) wins over `max-width: 50px`: the item is 100px wide.
#[test]
fn min_content_minimum_wins_over_a_smaller_max_width() {
    let img = render(
        "<div style='display:flex;width:200px;height:50px'>\
           <div style='min-width:min-content;max-width:50px;background:green'><div style='width:100px'></div></div>\
         </div>",
    );
    check(&img, &[(95, 25, GREEN), (105, 25, WHITE)]);
}

/// A block's `min-width: max-content` counts in its flex item parent's minimum: the item in a 0px
/// wide container is as wide as the 100px line the block keeps.
#[test]
fn keyword_min_width_bounds_an_automatic_width_contribution() {
    let img = render(
        "<div style='display:flex;width:0'>\
           <div style='background:green'>\
             <div style='min-width:max-content;height:20px'><span style='display:inline-block;width:50px'></span> <span style='display:inline-block;width:50px'></span></div>\
           </div>\
         </div>",
    );
    check(&img, &[(5, 10, GREEN), (100, 10, GREEN), (115, 10, WHITE)]);
}

/// `min-height: 20px` beats `max-height: 10px` in a column container's automatic height: the next
/// box starts 20px down.
#[test]
fn column_size_keeps_a_minimum_over_a_smaller_maximum() {
    let img = render(
        "<div style='display:flex;flex-direction:column;width:100px'>\
           <div style='min-height:min-content;max-height:10px;background:green'><div style='height:20px'></div></div>\
         </div>\
         <div style='height:20px;width:100px;background:blue'></div>",
    );
    check(&img, &[(50, 15, GREEN), (50, 25, [0, 0, 255])]);
}

/// A wrapping column item of `flex: 1 0 0px` is at least its two 50px items tall (its min-content
/// height is one line's), so they stack instead of wrapping into two columns.
#[test]
fn wrapping_column_min_content_height_is_one_line() {
    let img = render(
        "<div style='display:flex;flex-direction:column'>\
           <div style='display:flex;flex-direction:column;flex-wrap:wrap;flex:1 0 0px;height:500px'>\
             <div style='flex:1 0 0px;width:100px;background:green'><div style='height:50px'></div></div>\
             <div style='flex:1 0 0px;width:100px;background:green'><div style='height:50px'></div></div>\
           </div>\
         </div>",
    );
    check(&img, &[(50, 25, GREEN), (50, 75, GREEN), (150, 25, WHITE)]);
}

/// An overflowing `space-between` row-reverse puts its item's end at the container's right edge
/// (`flex-start`), so the green end of the 300px item shows.
#[test]
fn overflowing_space_between_falls_back_to_flex_start() {
    let img = render(
        "<div style='display:flex;flex-direction:row-reverse;justify-content:space-between;overflow:hidden;width:100px;height:50px'>\
           <div style='display:flex;width:300px;flex-shrink:0'>\
             <div style='width:200px;height:50px;background:red;flex-shrink:0'></div>\
             <div style='width:100px;height:50px;background:green;flex-shrink:0'></div>\
           </div>\
         </div>",
    );
    check(&img, &[(10, 25, GREEN), (90, 25, GREEN)]);
}

/// A checkbox is 13x13 by nature, not by `width`/`height` in the UA sheet: a row flex container
/// stretches it to its 50px height (WPT css-flexbox `stretch-flex-item-checkbox-input`).
#[test]
fn a_checkbox_flex_item_stretches() {
    let img = render(
        "<div style='display:flex;width:50px;height:50px;background:green'><input type=checkbox style='width:100%;margin:0'></div>",
    );
    // The stretched checkbox covers the green container down to its bottom (a 13px one left it green)
    assert_ne!(pixel(&img, 25, 40), GREEN);
    check(&img, &[(60, 25, WHITE)]);
}

/// A fixed-layout table with a percentage width asks to be as wide as possible (Blink's
/// `kTableMaxWidth`): two flex items holding such tables share a 300px line evenly.
#[test]
fn fixed_percentage_tables_share_a_flex_line_evenly() {
    let img = render(
        "<div style='display:flex;width:300px;background:red'>\
           <div><div style='display:table;table-layout:fixed;width:100%;height:30px;background:green'></div></div>\
           <div><div style='display:table;table-layout:fixed;width:100%;height:30px;background:blue'></div></div>\
         </div>",
    );
    check(&img, &[(5, 15, GREEN), (145, 15, GREEN), (155, 15, [0, 0, 255]), (295, 15, [0, 0, 255])]);
}
