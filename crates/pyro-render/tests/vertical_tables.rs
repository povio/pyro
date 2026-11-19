//! Tables in vertical writing modes and with `direction: rtl` (round 5): collapsed borders are
//! resolved over the grid's own rows and columns, whichever way they run; captions are laid out
//! on the block-start and block-end sides of a vertical table; a table narrower than its columns'
//! min-content widths grows to them when a spanning cell's max-content width is wider than its
//! columns'; and an absolutely positioned child of a row group is a box.
//! WPT css-writing-modes `border-conflict-element-v*`, `caption-side-v*`, `row-progression-v*`,
//! css-contain `contain-paint-015`..`017`.
use std::sync::Arc;

use pyro_render::*;

const GREEN: [u8; 3] = [0, 128, 0];
const BLUE: [u8; 3] = [0, 0, 255];
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
        html: format!("<!doctype html><style>td{{padding:0}}table{{border-spacing:0}}</style><body style='margin:0;background:#fff'>{body}"),
        base_url: "http://test/".into(),
        width: 400,
        height: 300,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

fn assert_color(img: &RgbaImage, x: u32, y: u32, want: [u8; 3], what: &str) {
    let i = ((y * img.width + x) * 4) as usize;
    let got = [img.data[i], img.data[i + 1], img.data[i + 2]];
    assert!(
        got.iter().zip(want).all(|(&g, w)| (i32::from(g) - i32::from(w)).abs() <= 2),
        "{what} at ({x}, {y}): {got:?}, want {want:?}"
    );
}

/// `vertical-lr`: the rows are columns of the page and a row's cells go down it. The border between
/// two cells of a row is their bottom and top borders; the first cell's wins among equals. The
/// table was 100x0 with nothing painted (collapsed borders were resolved in horizontal tables only).
#[test]
fn a_vertical_lr_table_collapses_the_borders_between_the_cells_of_a_row() {
    let img = render(
        "<table style='border-collapse:collapse;writing-mode:vertical-lr'><tr><td></td><td></td></tr>\
         <tr><td style='border-bottom:100px solid green'></td><td style='border-top:100px solid red;width:100px'></td></tr></table>",
    );
    for (x, y) in [(5, 5), (50, 50), (95, 95), (5, 95), (95, 5)] {
        assert_color(&img, x, y, GREEN, "the rule between the two cells");
    }
    assert_color(&img, 105, 50, WHITE, "right of the table");
    assert_color(&img, 50, 105, WHITE, "below the table");
}

/// `vertical-rl`: a row's block-start side is its right. The cell's right border beats its row's
/// there, and the table is as wide as the rule (half in the cell, half the table's own border).
#[test]
fn a_vertical_rl_table_collapses_the_borders_on_its_right() {
    let img = render(
        "<table style='border-collapse:collapse;writing-mode:vertical-rl'>\
         <tr style='border-right:100px solid red'><td style='border-right:100px solid green;height:100px'></td></tr></table>",
    );
    for (x, y) in [(5, 5), (50, 50), (95, 95)] {
        assert_color(&img, x, y, GREEN, "the cell's right border");
    }
    assert_color(&img, 105, 50, WHITE, "right of the table");
    assert_color(&img, 50, 105, WHITE, "below the table");
}

/// `direction: rtl`: the first cell is on the right, and its left border meets the second cell's
/// right one; the first cell's wins among equals (CSS 2.1 §17.6.2.1: the one further to the right
/// in an `rtl` table). The first cell's left border was offered to the table's right edge.
#[test]
fn an_rtl_table_collapses_a_cells_left_border_with_the_right_border_of_the_cell_after_it() {
    let cell = "<div style='width:50px;height:20px'></div>";
    let img = render(&format!(
        "<table style='border-collapse:collapse;direction:rtl'><tr>\
         <td style='border-left:10px solid green'>{cell}</td><td style='border-right:10px solid red'>{cell}</td></tr></table>"
    ));
    // The second cell 0..50, the rule 50..60, the first cell 60..110.
    for x in [51, 55, 59] {
        assert_color(&img, x, 10, GREEN, "the rule between the cells");
    }
    assert_color(&img, 45, 10, WHITE, "the second cell");
    assert_color(&img, 65, 10, WHITE, "the first cell");
    assert_color(&img, 105, 10, WHITE, "the first cell's right side");
    assert_color(&img, 115, 10, WHITE, "right of the table");
}

/// A vertical table's captions are on its block-start (`caption-side: top`) and block-end sides:
/// left and right of the grid in `vertical-lr`, right and left in `vertical-rl`. They were not laid out.
#[test]
fn a_vertical_tables_captions_are_beside_its_grid() {
    let table = |wm: &str, side: &str| {
        format!(
            "<table style='writing-mode:{wm}'><caption style='caption-side:{side}'><div style='width:50px;height:100px;background:green'></div></caption>\
             <tr><td><div style='width:50px;height:100px;background:blue'></div></td></tr></table>"
        )
    };
    for (wm, side, caption_x, cell_x) in [
        ("vertical-lr", "top", 25, 75),
        ("vertical-lr", "bottom", 75, 25),
        ("vertical-rl", "top", 75, 25),
        ("vertical-rl", "bottom", 25, 75),
    ] {
        let img = render(&table(wm, side));
        assert_color(&img, caption_x, 50, GREEN, &format!("{wm} {side}: the caption"));
        assert_color(&img, cell_x, 50, BLUE, &format!("{wm} {side}: the cell"));
        assert_color(&img, 105, 50, WHITE, &format!("{wm} {side}: right of the table"));
    }
}

/// A table whose `width` is less than its columns' min-content widths is as wide as those, also
/// when a cell spanning them has a max-content width wider than theirs (its min-content width
/// fits): the table kept its `width` and the cells overflowed it.
#[test]
fn a_narrow_table_with_a_spanning_cell_grows_to_its_columns_min_content_widths() {
    let cell = "<div style='width:50px;height:10px'></div>";
    let word = "<span style='display:inline-block;width:80px;height:10px'></span>";
    let img = render(&format!(
        "<table style='width:60px;background:green'><tr><td>{cell}</td><td>{cell}</td></tr><tr><td colspan=2>{word} {word}</td></tr></table>"
    ));
    assert_color(&img, 95, 5, GREEN, "the second column");
    assert_color(&img, 95, 25, GREEN, "the spanning cell");
    assert_color(&img, 105, 5, WHITE, "right of the table");
}

/// An absolutely positioned child of a row group (here one that is in no table: the anonymous
/// table has no rows) is a box of its own; its content was never built, so it was 0x0.
#[test]
fn an_absolutely_positioned_child_of_a_row_group_is_laid_out() {
    for display in ["table-row-group", "table-header-group", "table-footer-group"] {
        let img = render(&format!(
            "<div style='display:{display}'><span style='position:absolute'><div style='width:50px;height:50px;background:green'></div></span></div>"
        ));
        assert_color(&img, 25, 25, GREEN, display);
    }
}
