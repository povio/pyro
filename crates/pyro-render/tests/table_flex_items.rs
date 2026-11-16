//! A table's height is a minimum (CSS 2.1 §17.5.3; Taffy's table grid), and as a flex item a table
//! is never smaller than its min-content size in the main axis, whatever its `min-width`,
//! `min-height` or `max-width` (as in Blink). Pixel checks after WPT's
//! `css/css-flexbox/table-as-item-*` reftests.
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

/// A 20px `height` on a table whose row is 50px tall: the table is 50px tall, so the block after it
/// starts at 50px.
#[test]
fn a_tables_height_is_a_minimum() {
    let img = render(
        "<table style='height:20px;border-spacing:0;background:green'><td style='padding:0'><div style='width:30px;height:50px'></div></td></table>\
         <div style='width:30px;height:20px;background:blue'></div>",
    );
    check(&img, &[(10, 45, GREEN), (10, 55, BLUE), (10, 69, BLUE), (10, 71, WHITE)]);
}

/// A table flex item with `min-width: 50px` is still as wide as its 100px column: the next item
/// shrinks to nothing instead of taking 50px beside it.
#[test]
fn a_table_flex_item_is_as_wide_as_its_columns() {
    let img = render(
        "<div style='display:flex;width:100px'>\
           <div style='display:table;min-width:50px;background:green'>\
             <div style='display:table-cell'><div style='width:100px;height:50px'></div></div>\
           </div>\
           <div style='height:50px;flex-basis:100px;min-width:0;background:blue'></div>\
         </div>",
    );
    check(&img, &[(10, 25, GREEN), (95, 25, GREEN), (105, 25, WHITE)]);
}

/// A table's `max-width` does not make a flex item narrower than its columns either.
#[test]
fn a_tables_max_width_does_not_narrow_it_below_its_columns() {
    let img = render(
        "<div style='display:flex;width:200px'>\
           <div style='display:table;width:10px;max-width:10px;height:50px;background:green'>\
             <div style='width:100px;height:10px'></div>\
           </div>\
         </div>",
    );
    check(&img, &[(95, 25, GREEN), (105, 25, WHITE)]);
}

/// In a column flex container of no height, a table of `height: 0` is as tall as its 50px row, and
/// the item after it starts below it.
#[test]
fn a_table_in_a_column_is_as_tall_as_its_rows() {
    let img = render(
        "<div style='display:flex;flex-direction:column;height:0;width:100px'>\
           <div style='display:table;height:0;width:100px;background:green'>\
             <div style='display:table-cell'><div style='height:50px'></div></div>\
           </div>\
           <div style='flex:0 0 30px;background:blue'></div>\
         </div>",
    );
    check(&img, &[(10, 45, GREEN), (10, 55, BLUE), (10, 79, BLUE), (10, 81, WHITE)]);
}

/// A table stretched to a 100px tall flex line has its captions inside those 100px (they are part
/// of the table wrapper box, the flex item): the table grid gets what they leave.
#[test]
fn a_stretched_tables_captions_are_inside_its_height() {
    let img = render(
        "<div style='display:flex;width:100px;height:100px'>\
           <table style='border-spacing:0;flex-grow:1'>\
             <caption style='height:10px;background:green'></caption>\
             <caption style='height:20px;background:green;caption-side:bottom'></caption>\
             <td style='background:blue'></td>\
           </table>\
         </div>",
    );
    check(&img, &[(10, 5, GREEN), (10, 50, BLUE), (10, 95, GREEN), (10, 105, WHITE)]);
}

/// Measured in a flex container, a table's 100% column counts as an auto one (as in a cell, Blink's
/// `TableNode::AllowColumnPercentages`): its flex base size is its content's, and it grows to fill the
/// 100px line, instead of being as wide as `f32::MAX / 4`.
#[test]
fn a_tables_percentage_column_is_auto_when_measured_in_a_flex_container() {
    let img = render(
        "<div style='display:flex;width:100px;align-items:flex-start'>\
           <table style='flex-grow:1;flex-shrink:0;height:50px;border-spacing:0;background:green'>\
             <tr><td style='width:100%;padding:0'></td><td style='padding:0'><div style='width:10px'></div></td></tr>\
           </table>\
         </div>",
    );
    check(&img, &[(50, 25, GREEN), (95, 25, GREEN), (105, 25, WHITE)]);
}
