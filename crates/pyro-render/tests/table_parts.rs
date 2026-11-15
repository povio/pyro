//! Table layout and paint details of WPT `css/CSS2/tables` and friends (reftest lane P2): white
//! space between table parts in an inline box, cells aligned on their row's baseline, fixed-layout
//! cells narrower than their padding, column groups without columns, `max-width` on a cell and the
//! box shadows of rows.
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

/// White space between two cells in an inline box generates no box (CSS 2.1 §17.2.1): the cells
/// touch. A space was laid out between them.
#[test]
fn white_space_between_cells_in_an_inline_box_goes() {
    let square =
        |color: &str| format!("<span style='display:inline-block;width:20px;height:20px;background:{color}'></span>");
    let img = render(&format!(
        "<div><span><span style='display:table-cell'>{}</span>\n  \
           <!-- a comment --><span style='display:table-cell'>{}</span></span></div>",
        square("green"),
        square("blue")
    ));
    let y = (0..40).find(|&y| pixel(&img, 10, y) == GREEN).expect("the first cell's square");
    assert_color(&img, 21, y + 5, BLUE, "the second cell, right after the first");
    assert_color(&img, 41, y + 5, WHITE, "nothing after the second cell");
}

/// White space between a cell and other inline content in an inline box stays.
#[test]
fn white_space_between_a_cell_and_text_stays() {
    let img = render(
        "<div style='font-size:20px'><span><span style='display:table-cell'>\
           <span style='display:inline-block;width:20px;height:20px;background:green'></span></span> \
           <span style='display:inline-block;width:20px;height:20px;background:blue'></span></span></div>",
    );
    let y = (0..40).find(|&y| pixel(&img, 10, y) == GREEN).expect("the cell's square");
    assert_color(&img, 21, y + 5, WHITE, "the space after the cell");
}

/// Cells aligned on their row's baseline (`vertical-align: baseline`) have their first lines on
/// one baseline: a cell with less padding has its content lower in its box. Content was laid out at
/// the top of each cell.
#[test]
fn baseline_cells_share_their_rows_baseline() {
    let img = render(
        "<table style='border-spacing:0'><tr>\
           <td style='vertical-align:baseline;padding:40px 0 0'><div><span style='display:inline-block;width:20px;height:20px;background:green'></span></div></td>\
           <td style='vertical-align:baseline;padding:0;background:#fff'><div><span style='display:inline-block;width:20px;height:20px;background:blue'></span></div></td>\
         </tr></table>",
    );
    // The first cell's box is 20px wide; its square is at 40px down.
    let y = (0..120).find(|&y| pixel(&img, 10, y) == GREEN).expect("the first cell's square");
    assert!(y >= 40, "the first cell's square starts at {y}, under its padding");
    assert_color(&img, 30, y + 10, BLUE, "the second cell's square, on the same baseline");
    assert_color(&img, 30, 5, WHITE, "nothing at the top of the second cell");
}

/// A fixed table's cells are as wide as their columns, even where those are narrower than the
/// cells' padding: a 100px table whose middle cell takes 50% (and its padding) leaves nothing to
/// the others. The last cell stuck out of the table by its padding.
#[test]
fn fixed_table_cells_can_be_narrower_than_their_padding() {
    let img = render(
        "<table style='table-layout:fixed;width:100px;border-spacing:0'><tr>\
           <td style='padding:50px 25px;background:blue'></td>\
           <td style='padding:50px 25px;background:green;width:50%'></td>\
           <td style='padding:50px 25px;background:blue'></td>\
         </tr></table>",
    );
    assert_color(&img, 2, 50, GREEN, "the middle cell, from the table's left");
    assert_color(&img, 97, 50, GREEN, "the middle cell, to the table's right");
    assert_color(&img, 110, 50, WHITE, "nothing past the table");
}

/// A column group without columns stands for one column, with its background and its width.
/// It had no column.
#[test]
fn a_column_group_without_columns_is_a_column() {
    let img = render(
        "<div style='display:table'>\
           <span style='display:table-column-group;background:blue;width:80px'></span>\
           <div style='display:table-row'>\
             <div style='display:table-cell;height:50px'></div>\
             <div style='display:table-cell;width:50px;height:50px;background:green'></div>\
           </div>\
         </div>",
    );
    assert_color(&img, 40, 25, BLUE, "the first column, the group's background");
    assert_color(&img, 78, 25, BLUE, "the first column is the group's 80px");
    assert_color(&img, 100, 25, GREEN, "the second cell");
}

/// A cell's `max-width` caps the width it gives its column (as in Chromium).
#[test]
fn a_cells_max_width_caps_its_columns_width() {
    let img = render(
        "<div style='display:table'><div style='display:table-cell;width:150px;max-width:50px;height:50px;background:green'></div></div>",
    );
    assert_color(&img, 25, 25, GREEN, "the cell");
    assert_color(&img, 75, 25, WHITE, "nothing past its max-width");
}

/// A row's outer box shadow is painted around the row (under its background). It was not painted.
#[test]
fn a_rows_box_shadow_is_painted() {
    let img = render(
        "<div style='display:table;margin:60px'>\
           <div style='display:table-row;background:blue;box-shadow:0 0 0 10px green'>\
             <div style='display:table-cell;width:50px;height:50px'></div>\
           </div>\
         </div>",
    );
    assert_color(&img, 85, 85, BLUE, "the row's background");
    assert_color(&img, 55, 85, GREEN, "the shadow, left of the row");
    assert_color(&img, 85, 115, GREEN, "the shadow, below the row");
    assert_color(&img, 45, 85, WHITE, "nothing past the shadow");
}

/// A table in the collapsing border model has no padding (CSS 2.1 §17.6.2), for its container
/// too: it is as big as its `width` and `height`. Its padding was added.
#[test]
fn a_collapsing_tables_padding_is_ignored() {
    let img = render(
        "<table style='border-collapse:collapse;width:100px;height:100px;padding:100px;background:green'></table>",
    );
    assert_color(&img, 50, 50, GREEN, "the table");
    assert_color(&img, 150, 50, WHITE, "nothing right of its 100px");
    assert_color(&img, 50, 150, WHITE, "nothing below its 100px");
}

/// Collapsed borders laid out as one uniform grid are painted in the colour conflict resolution
/// gave them: here every vertical rule is a cell's green right (or left) border. They took the
/// first cell's top border's colour and style (none: nothing was painted).
#[test]
fn uniform_collapsed_borders_take_the_winning_colour() {
    let img = render(
        "<table style='border-collapse:collapse'><tr>\
           <td style='border-style:none solid;border-width:0 20px;border-color:red green;height:25px;padding:0'></td>\
           <td style='border-style:none solid;border-width:0 20px;border-color:red green red red;height:25px;padding:0'></td>\
         </tr></table>",
    );
    assert_color(&img, 10, 12, GREEN, "the left rule");
    assert_color(&img, 30, 12, GREEN, "the middle rule (the first cell's right border)");
    assert_color(&img, 50, 12, GREEN, "the right rule");
}

/// A `<col>`'s width has the halves of the collapsed borders beside it in it: a 165px column
/// whose cell has a 110px left border makes a 220px table (165px and half the border). The halves
/// were added to the column's width.
#[test]
fn a_columns_width_holds_half_the_collapsed_borders() {
    let img = render(
        "<table style='border-collapse:collapse;background:blue'><col style='width:165px'>\
           <tr><td style='width:110px;border-left:110px solid blue;padding:0;height:20px'></td></tr></table>",
    );
    assert_color(&img, 217, 10, BLUE, "the table, to 220px");
    assert_color(&img, 223, 10, WHITE, "nothing past 220px");
}

/// A fixed collapsed table's first-row cell with a percentage width gets the halves of the
/// collapsed borders beside it on top: its 25px borders and 50% of a 100px table fill the table,
/// leaving nothing to the other cells (whose backgrounds would show).
#[test]
fn a_fixed_collapsed_tables_percentage_cell_holds_its_half_borders() {
    let img = render(
        "<table style='border-collapse:collapse;table-layout:fixed;width:100px'><tr>\
           <td style='background:blue;border-right:24px solid blue;padding:50px 0'></td>\
           <td style='background:green;border-left:25px solid green;border-right:25px solid green;width:50%;padding:50px 0'></td>\
           <td style='background:blue;border-left:25px solid blue;padding:50px 0'></td>\
         </tr></table>",
    );
    for x in [2, 50, 97] {
        assert_color(&img, x, 50, GREEN, "the middle cell and its borders, all of the table");
    }
}

/// Where collapsed borders meet, the square the rules share is the colour of the edge that wins
/// there, among equals the top and left ones: the first cell's blue top border, not the second's
/// yellow one (the yellow top border painted over the top half of that square).
#[test]
fn a_junction_takes_the_winning_edges_colour() {
    let img = render(
        "<table style='border-collapse:collapse'><tr>\
           <td style='border:20px solid blue;width:40px;height:40px;padding:0'></td>\
           <td style='border:20px solid yellow;border-left-color:red;width:40px;height:40px;padding:0'></td>\
         </tr></table>",
    );
    // The rule between the cells, at the cells' middle height.
    let x = (25..200).find(|&x| pixel(&img, x, 45) == BLUE).expect("the middle rule");
    assert_color(&img, x + 2, 3, BLUE, "the junction with the top border, the first cell's");
}

/// A cell spanning columns of an automatic table gives none of them its width: its 100px span
/// three columns and the two 20px gaps between them. Its first column took the 100px.
#[test]
fn a_spanning_cells_width_covers_its_columns_and_gaps() {
    let img = render(
        "<table cellpadding=0 style='border-spacing:20px'><tr>\
           <td colspan=3 style='width:100px;height:20px;background:green'></td></tr></table>",
    );
    assert_color(&img, 118, 30, GREEN, "the cell, to 100px");
    assert_color(&img, 123, 30, WHITE, "nothing past 100px");
}

/// An absolutely positioned box in a caption is positioned against the positioned table it is
/// in. It was left out (a caption's out-of-flow boxes were not passed on).
#[test]
fn an_abspos_box_in_a_caption_is_the_tables() {
    let img = render(
        "<table style='position:relative;border-spacing:0'>\
           <caption style='width:50px;height:50px;background:green'>\
             <div style='position:absolute;left:50px;top:0;width:50px;height:50px;background:blue'></div>\
           </caption></table>",
    );
    assert_color(&img, 25, 25, GREEN, "the caption");
    assert_color(&img, 75, 25, BLUE, "the box, 50px into the table");
}
