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

/// A table's flex base size in a column is its rows' height, its `min-height` aside: with a
/// 1px row, `min-height: 80px` and an equal-growing 1px sibling in 200px, it ends 100px tall.
#[test]
fn a_column_table_item_bases_on_its_rows_not_its_min_height() {
    let img = render(
        "<div style='display:flex;flex-direction:column;height:200px'>\
           <div style='display:table;min-height:80px;flex:1 0 auto;width:100px;background:green'>\
             <div style='display:table-cell'><div style='height:1px'></div></div>\
           </div>\
           <div style='flex:1 0 1px'></div>\
         </div>",
    );
    check(&img, &[(50, 95, GREEN), (50, 105, WHITE)]);
}

/// A table in a row is laid out in the container's width: a 100% cell makes it as wide as the
/// 200px line.
#[test]
fn a_row_table_item_with_a_percentage_cell_fills_the_line() {
    let img = render(
        "<div style='display:flex;width:200px'>\
           <table cellspacing=0 cellpadding=0><tr><td style='width:100%;background:green;height:20px'><div style='width:10px'></div></td><td><div style='width:10px'></div></td></tr></table>\
         </div>",
    );
    check(&img, &[(150, 10, GREEN), (250, 10, WHITE)]);
}

/// An inflexible 200px basis with `max-width: 100px` counts 100px in its container's max-content
/// width.
#[test]
fn an_inflexible_basis_counts_its_max_width_in_the_container_size() {
    let img = render(
        "<div style='display:flex;width:max-content;background:green;height:50px'>\
           <div style='flex:0 0 200px;max-width:100px'></div>\
         </div>",
    );
    check(&img, &[(95, 25, GREEN), (105, 25, WHITE)]);
}

/// A cleared block's own floats force no clearance on it: `clear: both` blocks of a fixed height
/// holding floats stack, the second one not pulled up to the first one's float (WPT css-flexbox
/// `flexbox-flex-basis-content-004-ref`).
#[test]
fn a_cleared_block_holding_a_float_stays_below_the_block_before_it() {
    let img = render(
        "<style>.c{clear:both;height:50px}.f{float:left;width:20px;height:16px}</style>\
         <div class=c><div class=f style='background:blue'></div></div>\
         <div class=c><div class=f style='background:green'></div></div>",
    );
    check(&img, &[(10, 30, WHITE), (10, 55, GREEN), (10, 8, [0, 0, 255])]);
}

/// A collapsible space after an inline box's start margin, at the start of a line of atomic
/// inlines only, is removed: a flex item holding it is as wide as its 1px margin and 99px box
/// (WPT css-flexbox `grandchild-span-height`).
#[test]
fn a_leading_space_after_a_span_start_edge_is_removed() {
    let img = render(
        "<div style='display:flex'><div style='background:green'>\n  <span style='margin-left:1px'>\n    \
         <i style='display:inline-block;width:99px;height:50px'></i>\n  </span>\n</div></div>",
    );
    check(&img, &[(98, 25, GREEN), (102, 25, WHITE)]);
}

/// A wrapping column's unstretched item shrinks to fit its line once the line's cross size is
/// known: a 200px sibling widens the line, and the item's two 100px floats sit side by side
/// (WPT css-flexbox `align-self-016`).
#[test]
fn an_unstretched_wrapping_column_item_fits_its_line() {
    let img = render(
        "<div style='display:flex;flex-flow:column wrap;width:100px'>\
           <div style='align-self:start'>\
             <div style='float:left;width:100px;height:50px;background:green'></div>\
             <div style='float:left;width:100px;height:50px;background:green'></div>\
           </div>\
           <div style='width:200px'></div>\
         </div>",
    );
    check(&img, &[(150, 25, GREEN), (50, 75, WHITE)]);
}

/// A floated wrapping column fits its two 100px lines into the 150px it has.
#[test]
fn a_floated_wrapping_column_fits_its_lines_into_the_available_width() {
    let img = render(
        "<div style='width:150px'><div style='display:flex;flex-flow:column wrap;float:left;height:35px;background:green;align-content:flex-start'>\
           <div style='width:100px;height:20px;flex:none'></div><div style='width:100px;height:10px;flex:none'></div>\
           <div style='width:100px;height:10px;flex:none'></div><div style='width:100px;height:20px;flex:none'></div>\
         </div></div>",
    );
    check(&img, &[(145, 32, GREEN), (155, 32, WHITE)]);
}

/// Under `white-space: pre` the text after an inline-block too wide for the line stays on that
/// line (no wrap opportunity after the box; WPT css-overflow `text-overflow-009`).
#[test]
fn no_wrap_after_an_overflowing_inline_block_under_pre() {
    let img = render(
        "<div style='width:100px;white-space:pre;overflow:hidden;line-height:20px'>\
           <span style='display:inline-block;width:200px;height:50px;background:green'> </span><span style='display:inline-block;width:20px;height:20px;background:blue'></span>\
         </div>",
    );
    // The second box is on the first line, cut off at the right; nothing below the first line
    check(&img, &[(50, 25, GREEN), (10, 60, WHITE), (10, 70, WHITE)]);
}

/// The natural 13x13 of a checkbox or radio is its border box, as Chromium's: the UA `input`
/// padding and border do not grow it (a checkbox was 21x19 and a radio 17x15, so a force click at
/// the centre of a react-aria hidden checkbox missed the label drawn over it; Danex D5 B-05).
#[test]
fn checkboxes_and_radios_keep_a_13px_border_box() {
    for kind in ["checkbox", "radio"] {
        let img = render(&format!(
            "<div style='font-size:0;line-height:0'><input type={kind} style='margin:0;vertical-align:top'>\
             <span style='display:inline-block;width:20px;height:20px;background:green'></span></div>"
        ));
        // The green box starts right after a 13px control (a wider one covered x = 14 and 15).
        check(&img, &[(14, 5, GREEN), (15, 10, GREEN), (32, 10, GREEN), (34, 10, WHITE)]);
        // Nothing below the 13px row but the page (a 19px or 15px tall control reached y = 14).
        check(&img, &[(5, 14, WHITE)]);
    }
}
