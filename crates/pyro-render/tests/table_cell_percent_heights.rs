//! Lane R4E: percentage heights of a table cell's children (CSS Tables 3 § row layout, as Blink):
//! they resolve against the cell's own length `height`, against its stretched height only when the
//! table's (or the row's) height is definite, and are indefinite otherwise, where a percentage-height
//! scroll container counts as 0 (its `min-height` holds). After WPT css-tables
//! `percent-height-replaced-in-percent-cell-002/003`, `percent-height-table-cell-child`,
//! `table-cell-child-overflow-measure`, `percentage-sizing-of-table-cell-007`,
//! `percentage-sizing-of-table-cell-children-006`.
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

/// A cell's length `height` is the basis of a replaced child's `max-height: 100%`.
#[test]
fn replaced_max_height_resolves_against_the_cells_length_height() {
    let img = render(
        "<div style='display:table-cell;height:100px;line-height:0'>\
           <canvas width=200 height=200 style='background:green;max-height:100%'></canvas></div>",
    );
    check(&img, &[(50, 50, GREEN), (150, 50, WHITE), (50, 150, WHITE)]);
}

/// The cell's own `height` (50px), not its row's (100px), is the basis.
#[test]
fn the_cells_own_height_is_the_basis_not_its_rows() {
    let img = render(
        "<div style='display:table'>\
           <div style='display:table-cell;height:50px;vertical-align:top;line-height:0'>\
             <canvas width=1 height=1 style='display:block;height:200%;background:green'></canvas></div>\
           <div style='display:table-cell'><div style='height:100px'></div></div></div>",
    );
    check(&img, &[(50, 50, GREEN), (95, 95, GREEN), (150, 50, WHITE), (50, 150, WHITE)]);
}

/// In a table of indefinite height a cell's stretched height is no basis: `height: 50%` is auto.
#[test]
fn an_auto_cell_in_an_auto_table_gives_no_basis() {
    let img = render(
        "<div style='display:table;height:100%;width:100px;background:red'>\
           <div style='height:50%;background:green'><div style='height:100px'></div></div></div>",
    );
    check(&img, &[(50, 25, GREEN), (50, 75, GREEN), (50, 150, WHITE)]);
}

/// A percentage-height scroll container there does not stretch to its row: its `min-height` holds,
/// even when a sibling cell makes the row taller.
#[test]
fn a_percentage_height_scroller_keeps_its_min_height() {
    let img = render(
        "<div style='display:table'>\
           <div style='display:table-cell;height:100%;vertical-align:top'>\
             <div style='overflow-y:auto;height:100%;width:100px;min-height:100px;background:green'></div></div>\
           <div style='display:table-cell;height:150px'></div></div>",
    );
    check(&img, &[(50, 50, GREEN), (50, 125, WHITE)]);
}

/// With a definite table height the stretched cell is the basis.
#[test]
fn a_definite_table_height_makes_the_stretched_cell_the_basis() {
    let img = render(
        "<table style='height:200px;border-spacing:0'><td style='padding:0'>\
           <div style='height:100%;width:100px;background:green'></div></td></table>",
    );
    check(&img, &[(50, 10, GREEN), (50, 190, GREEN), (50, 210, WHITE)]);
}

/// Only in a table of non-`auto` height does such a scroller count as 0: in an `auto` one its
/// content sizes it.
#[test]
fn a_percentage_height_scroller_counts_as_zero_only_in_a_restricted_table() {
    let restricted = render(
        "<table style='height:100%;border-spacing:0'><td style='padding:0'>\
           <div style='width:100px;min-height:50px;height:100%;overflow-y:scroll;background:green'>\
             <div style='height:200px'></div></div></td></table>",
    );
    check(&restricted, &[(30, 25, GREEN), (30, 75, WHITE)]);
    let unrestricted = render(
        "<div style='display:table'><div style='overflow:auto;height:100%;background:green'>\
           <div style='width:100px;height:100px'></div></div></div>",
    );
    check(&unrestricted, &[(50, 50, GREEN), (50, 95, GREEN), (50, 105, WHITE)]);
}

/// Lane R4E: an inline box's outer shadow reaching back over the text before it on its line paints
/// over that text (CSS 2.1 Appendix E: a line's boxes and text in tree order; WPT css-backgrounds
/// `box-shadow-overlapping-001`).
#[test]
fn an_inline_shadow_paints_over_the_earlier_text_of_its_line() {
    let img = render(
        "<div style='float:left;font-size:40px;line-height:1;color:red'>XX\
           <span style='color:green;box-shadow:-60px 0 0 60px green'>Y</span></div>",
    );
    let red = (0..img.height as usize)
        .flat_map(|y| (0..img.width as usize).map(move |x| (x, y)))
        .filter(|&(x, y)| {
            let [r, g, b] = pixel(&img, x, y);
            r > 200 && g < 80 && b < 80
        })
        .count();
    assert_eq!(red, 0, "red text left uncovered");
}
