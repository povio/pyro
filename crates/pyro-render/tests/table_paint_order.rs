//! Table paint details of WPT `css/css-tables` (round 3 lane C2T): collapsed borders over the block
//! backgrounds in cells and over positioned parts, no borders inside spanning cells, row
//! backgrounds over a whole spanning cell, angled gradients in tiles larger than the box,
//! collapsed rows without their spacing, row and row group outlines, and the min-content width a
//! table with a smaller `width` contributes.
use std::sync::Arc;

use pyro_render::*;

const GREEN: [u8; 3] = [0, 128, 0];
const RED: [u8; 3] = [255, 0, 0];
const LIME: [u8; 3] = [144, 238, 144];
const WHITE: [u8; 3] = [255, 255, 255];

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("404".into()));
    }
}

/// `body` on a white page without margins, 400x300 CSS px.
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

fn pixel(img: &RgbaImage, x: u32, y: u32) -> [u8; 3] {
    let i = ((y * img.width + x) * 4) as usize;
    [img.data[i], img.data[i + 1], img.data[i + 2]]
}

fn assert_color(img: &RgbaImage, x: u32, y: u32, want: [u8; 3], what: &str) {
    let got = pixel(img, x, y);
    assert!(
        got.iter().zip(want).all(|(&g, w)| (i32::from(g) - i32::from(w)).abs() <= 2),
        "{what} at ({x}, {y}): {got:?}, want {want:?}"
    );
}

/// A collapsed table's borders paint over the border of a block in a cell (Blink paints them
/// after the block backgrounds of its cells, WPT `collapsed-borders-painting-order-001`).
#[test]
fn collapsed_borders_paint_over_blocks_in_cells() {
    let img = render(
        "<table style='border-collapse:collapse'><td style='border:50px solid green;padding:0'>\
         <div style='border:50px solid red;margin:-50px'></div></td></table>",
    );
    assert_color(&img, 25, 25, GREEN, "border");
    assert_color(&img, 75, 75, GREEN, "border");
}

/// ...but under an inline-level table in a cell (inline content paints after them, WPT
/// `collapsed-borders-painting-order-012`).
#[test]
fn collapsed_borders_paint_under_inline_content() {
    let img = render(
        "<table style='border-collapse:collapse;line-height:0'><td style='border:50px solid red;padding:0'>\
         <table style='border-collapse:collapse;margin:-50px;display:inline-table;vertical-align:top'>\
         <td style='border:50px solid green;padding:0'></td></table></td></table>",
    );
    assert_color(&img, 25, 25, GREEN, "inner table");
}

/// A table with positioned cells paints its borders after them, once (WPT
/// `collapsed-border-positioned-tr-td`).
#[test]
fn collapsed_borders_paint_over_positioned_cells() {
    let img = render(
        "<table style='border-collapse:collapse'><tr><td style='position:relative;width:50px;height:50px;\
         padding:0;background:red;border:10px solid green'></td></tr></table>",
    );
    assert_color(&img, 3, 30, GREEN, "left border");
    assert_color(&img, 30, 30, RED, "cell");
}

/// The grid lines inside a spanning cell have no collapsed border: the columns' borders are not
/// drawn through it (WPT `border-collapse-spanning-cells-001`).
#[test]
fn spanning_cells_have_no_borders_inside() {
    let img = render(
        "<table style='border-collapse:collapse;background:green;font:20px/1 monospace'>\
         <col style='border:20px solid red'><col style='border:20px solid red'>\
         <tr><td style='padding:0;border:20px solid green;width:20px;height:20px'></td>\
         <td style='padding:0;border:20px solid green;width:20px;height:20px'></td></tr>\
         <tr><td colspan=2 style='padding:0;border:none;height:20px'></td></tr></table>",
    );
    // The middle of the second row, where the column line between the two columns runs.
    assert_color(&img, 60, 70, GREEN, "inside the spanning cell");
}

/// A row's background covers the whole of a cell that spans rows, its image positioned against the
/// row (WPT `background-image-row`).
#[test]
fn a_rows_background_covers_a_spanning_cell() {
    let img = render(
        "<table style='border-spacing:0'><tr style='background:rgb(144,238,144)'>\
         <td rowspan=2 style='padding:0;width:50px;height:100px'></td><td style='padding:0;width:50px;height:50px'></td></tr>\
         <tr><td style='padding:0;height:50px'></td></tr></table>",
    );
    assert_color(&img, 25, 75, LIME, "spanning cell, second row");
    assert_color(&img, 75, 75, WHITE, "cell of the second row");
}

/// An angled gradient in a tile larger than the box is centred on the tile, not the box: the box at
/// the tile's start shows its start.
#[test]
fn an_angled_gradient_is_centred_on_its_tile() {
    let img = render(
        "<div style='width:50px;height:50px;background:linear-gradient(90deg,green 50%,blue 50%) 0 0/200px 50px no-repeat'></div>",
    );
    assert_color(&img, 45, 25, GREEN, "start of the tile");
}

/// A collapsed row takes its border spacing with it (WPT `visibility-collapse-border-spacing-001`),
/// and a row group's outline is painted.
#[test]
fn a_collapsed_row_takes_its_spacing_and_row_group_outlines_paint() {
    let img = render(
        "<table style='width:100px;border-spacing:10px;background:red'><tbody style='outline:solid green 10px'>\
         <tr style='visibility:collapse'><td style='padding:0'></td></tr>\
         <tr><td style='padding:0'><div style='height:80px;background:green'></div></td></tr></tbody></table>",
    );
    for (x, y) in [(5, 5), (50, 5), (95, 95), (50, 95), (50, 50)] {
        assert_color(&img, x, y, GREEN, "square");
    }
    assert_color(&img, 50, 105, WHITE, "below the table");
}

/// A table is never narrower than its content: the min-content width it contributes is at least
/// that (WPT `table-intrinsic-size-001`).
#[test]
fn a_table_contributes_its_content_width() {
    let img = render(
        "<div style='width:min-content;background:green'><div style='display:table;width:30px;height:100px;\
         border-inline:10px solid green;margin-inline:10px'><div style='width:60px;height:100px'></div></div></div>",
    );
    assert_color(&img, 95, 50, GREEN, "right margin of the table");
    assert_color(&img, 105, 50, WHITE, "past the container");
}
