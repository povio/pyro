//! Subgrids (css-grid-2 §9; Taffy `compute/grid/subgrid.rs`): a grid item whose `grid-template-columns` or
//! `-rows` is `subgrid` takes its parent's tracks, whose sizes its own items help decide. Pixel checks after
//! WPT's `css/css-grid/subgrid` reftests.
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
        width: 400,
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
const RED: [u8; 3] = [255, 0, 0];

#[track_caller]
fn check(img: &RgbaImage, points: &[(usize, usize, [u8; 3])]) {
    for &(x, y, want) in points {
        assert_eq!(pixel(img, x, y), want, "pixel at ({x}, {y})");
    }
}

/// The subgrid's items size the parent's `auto` columns: the red item below fills the first
/// column, as wide as the subgrid's first item.
#[test]
fn a_subgrids_items_size_its_parents_columns() {
    let img = render(
        "<div style='display:grid;grid-template-columns:auto auto;justify-content:start'>\
           <div style='display:grid;grid-template-columns:subgrid;grid-column:span 2'>\
             <div style='width:30px;height:20px;background:green'></div>\
             <div style='width:50px;height:20px;background:blue'></div>\
           </div>\
           <div style='height:20px;background:red'></div>\
         </div>",
    );
    check(&img, &[(25, 10, GREEN), (35, 10, BLUE), (75, 10, BLUE), (85, 10, WHITE), (25, 30, RED), (35, 30, WHITE)]);
}

/// A subgrid's gutters are centred on its parent's: with no gap of its own in a parent with 20px
/// gaps, its first track reaches 10px into the parent's gutter.
#[test]
fn a_subgrids_gap_is_centred_on_its_parents_gutter() {
    let img = render(
        "<div style='display:grid;grid-template-columns:100px 100px;column-gap:20px'>\
           <div style='display:grid;grid-template-columns:subgrid;column-gap:0;grid-column:span 2'>\
             <div style='height:20px;background:green'></div>\
             <div style='height:20px;background:blue'></div>\
           </div>\
         </div>",
    );
    check(&img, &[(105, 10, GREEN), (109, 10, GREEN), (111, 10, BLUE), (219, 10, BLUE), (221, 10, WHITE)]);
}

/// A subgrid in its rows only: its item's height is measured at the width of its own column, so its
/// two inline blocks take two lines and the parent's first row is 40px tall.
#[test]
fn a_row_subgrids_items_are_as_wide_as_its_own_columns() {
    let img = render(
        "<div style='display:grid;width:100px'>\
           <div style='display:grid;grid-template-rows:subgrid'>\
             <div style='font-size:0'>\
               <div style='display:inline-block;width:60px;height:20px;background:green'></div>\
               <div style='display:inline-block;width:60px;height:20px;background:green'></div>\
             </div>\
           </div>\
           <div style='height:20px;background:red'></div>\
         </div>",
    );
    check(&img, &[(10, 10, GREEN), (10, 30, GREEN), (10, 45, RED), (10, 61, WHITE)]);
}

/// An empty subgrid nested in another still takes the room of its padding: 10px on each side of the
/// outer one and 15px of the inner one make the parent 50px square.
#[test]
fn an_empty_nested_subgrids_padding_takes_room() {
    let img = render(
        "<div style='display:inline-grid;grid:auto / auto;background:green;vertical-align:top'>\
           <div style='display:grid;grid:subgrid / subgrid;padding:10px'>\
             <div style='display:grid;grid:subgrid / subgrid;padding:15px'></div>\
           </div>\
         </div>",
    );
    check(&img, &[(5, 5, GREEN), (45, 45, GREEN), (55, 5, WHITE), (5, 55, WHITE)]);
}

/// A subgrid gap wider than a track: its tracks still start where the parent's lines put them (the
/// middle track ends before it starts), so the items in its last two tracks touch.
#[test]
fn subgrid_tracks_follow_the_parents_lines_when_the_gap_is_wider() {
    let img = render(
        "<div style='display:grid;grid-template-columns:100px 100px 100px;width:300px'>\
           <div style='display:grid;grid-template-columns:subgrid;gap:150px;grid-column:span 3'>\
             <div style='width:100px;height:20px;background:blue;grid-column:2'></div>\
             <div style='width:100px;height:20px;background:green;grid-column:3'></div>\
           </div>\
         </div>",
    );
    check(&img, &[(170, 10, WHITE), (180, 10, BLUE), (270, 10, BLUE), (280, 10, GREEN), (370, 10, GREEN), (380, 10, WHITE)]);
}

/// A `vertical-lr` subgrid in its rows inside a horizontal grid: its rows are the parent's columns,
/// running left to right, so its two items sit in the parent's 30px and 50px columns.
#[test]
fn an_orthogonal_subgrids_rows_are_its_parents_columns() {
    let img = render(
        "<div style='display:grid;grid-template-columns:30px 50px;justify-content:start'>\
           <div style='display:grid;writing-mode:vertical-lr;grid-template-rows:subgrid;grid-column:span 2'>\
             <div style='inline-size:20px;background:green'></div>\
             <div style='inline-size:20px;background:blue'></div>\
           </div>\
         </div>",
    );
    check(&img, &[(15, 10, GREEN), (29, 10, GREEN), (31, 10, BLUE), (79, 10, BLUE), (81, 10, WHITE)]);
}

/// An item whose line lies past a subgrid is clamped into it before it is auto-placed in a row: in
/// the subgrid's last column, which the green item already takes in the first row, the blue one
/// (`grid-column: 5` of two tracks) goes to the second row instead of over the green one.
#[test]
fn items_clamped_into_a_subgrid_take_their_own_cells() {
    let img = render(
        "<div style='display:inline-grid;grid-auto-columns:20px;vertical-align:top'>\
           <div style='display:grid;grid-template-columns:subgrid;grid-column:1 / span 2;grid-auto-rows:10px'>\
             <div style='grid-column:2;background:green'></div>\
             <div style='grid-column:5;background:blue'></div>\
           </div>\
         </div>",
    );
    check(&img, &[(30, 5, GREEN), (30, 15, BLUE), (10, 15, WHITE)]);
}
