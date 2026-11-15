//! Table structure as CSS 2.1 §17 and Chromium build and paint it (WPT `css/CSS2/tables`): the
//! anonymous table, row and cell boxes around table parts that are missing them, captions, the
//! collapsed borders a collapsed table draws around its cells, the width a fixed table's column
//! gets from a cell with collapsed borders, and the background images of rows, row groups,
//! columns and column groups.
use std::sync::Arc;

use pyro_render::*;

const GREEN: [u8; 3] = [0, 128, 0];
const BLUE: [u8; 3] = [0, 0, 255];
const ORANGE: [u8; 3] = [255, 165, 0];
const WHITE: [u8; 3] = [255, 255, 255];

fn solid_png(n: u32, color: [u8; 3]) -> Vec<u8> {
    let data = (0..n * n).flat_map(|_| [color[0], color[1], color[2], 255]).collect();
    encode_png(&RgbaImage { width: n, height: n, data })
}

struct Images;
impl Fetcher for Images {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        let name = url.rsplit('/').next().unwrap_or_default().to_string();
        std::thread::spawn(move || {
            done(match name.as_str() {
                "blue.png" => Ok(solid_png(10, BLUE)),
                _ => Err("404".into()),
            })
        });
    }
}

/// `body` on a white page without margins, 400x300 CSS px.
fn render(body: &str) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(Images);
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

/// Cells in a block are wrapped in an anonymous row of an anonymous table: side by side. They
/// were laid out as blocks, one below the other.
#[test]
fn cells_in_a_block_are_one_row_of_an_anonymous_table() {
    let img = render(
        "<div><span style='display:table-cell;width:50px;height:50px;background:green'></span> \
         <span style='display:table-cell;width:50px;height:50px;background:blue'></span></div>",
    );
    assert_color(&img, 25, 25, GREEN, "the first cell");
    assert_color(&img, 75, 25, BLUE, "the second cell, beside the first");
    assert_color(&img, 25, 75, WHITE, "nothing below the row");
}

/// Cells of a table that come before its row group are its first row (an anonymous one), with
/// the same columns as the group's rows. They were placed over the group's last row.
#[test]
fn cells_before_a_row_group_are_the_first_row() {
    let img = render(
        "<div style='display:table'>\
           <div style='display:table-cell;width:40px;height:30px;background:green'></div>\
           <div style='display:table-cell;width:60px;height:30px;background:green'></div>\
           <div style='display:table-row-group'>\
             <div style='display:table-row'>\
               <div style='display:table-cell;height:30px;background:blue'></div>\
               <div style='display:table-cell;height:30px;background:orange'></div>\
             </div>\
           </div>\
         </div>",
    );
    assert_color(&img, 20, 15, GREEN, "the first row's first cell");
    assert_color(&img, 70, 15, GREEN, "the first row's second cell");
    assert_color(&img, 20, 45, BLUE, "the group's row, first column");
    assert_color(&img, 45, 45, ORANGE, "the group's row, second column (60px wide, at 40px)");
}

/// What a row holds that is not a cell is wrapped in an anonymous cell. It was not laid out.
#[test]
fn a_rows_other_content_is_an_anonymous_cell() {
    let img = render(
        "<div style='display:table'><div style='display:table-row'>\
           <span style='display:inline-block;width:40px;height:40px;background:green'></span>\
           <div style='display:table-cell'><div style='width:40px;height:40px;background:blue'></div></div>\
         </div></div>",
    );
    assert_color(&img, 20, 20, GREEN, "the inline block, in an anonymous cell");
    assert_color(&img, 60, 20, BLUE, "the cell after it");
}

/// A caption is laid out above the table (below it with `caption-side: bottom`), outside the
/// table's box: the table's background is not under it. Captions were not laid out or painted.
#[test]
fn a_caption_is_outside_the_tables_box() {
    for (side, caption_y, table_y) in [("top", 15, 55), ("bottom", 65, 25)] {
        let img = render(&format!(
            "<table style='border-spacing:0;background:blue'>\
               <caption style='caption-side:{side}'><div style='height:30px;background:green'></div></caption>\
               <tr><td style='padding:0;width:100px;height:50px'></td></tr>\
             </table>"
        ));
        assert_color(&img, 50, caption_y, GREEN, &format!("the caption ({side})"));
        assert_color(&img, 50, table_y, BLUE, &format!("the table's background ({side} caption)"));
        // The caption (30px) and the table (50px) end at 80px.
        assert_color(&img, 50, 85, WHITE, &format!("nothing below both ({side} caption)"));
    }
}

/// A collapsed table draws the borders that won around its cells, its own outer border only where
/// it won: a cell's wider border beats it. The table's own red border was drawn over them.
#[test]
fn a_collapsed_tables_outer_border_is_the_one_that_won() {
    let img = render(
        "<table style='border:10px solid red;border-collapse:collapse'>\
           <tr><td style='border:20px solid blue;padding:0;width:60px;height:60px'></td></tr>\
         </table>",
    );
    assert_color(&img, 5, 5, BLUE, "the cell's border, outside the table's half");
    assert_color(&img, 15, 50, BLUE, "the cell's border, left side");
    assert_color(&img, 50, 15, BLUE, "the cell's border, top side");
}

/// In a fixed collapsed table, a first-row cell's `width` gives its column the cell's border box
/// with half of the collapsed borders beside it: the content box stays as wide as `width`. The
/// column was as wide as `width`, the halves of the borders inside it.
#[test]
fn a_fixed_collapsed_tables_column_holds_the_cells_borders() {
    let img = render(
        "<table style='border-collapse:collapse;table-layout:fixed;width:300px'><tr>\
           <td style='padding:0;height:20px'></td>\
           <td style='padding:0;height:20px;width:80px;background:blue;border-left:20px solid orange;border-right:20px solid orange'></td>\
           <td style='padding:0;height:20px'></td>\
         </tr></table>",
    );
    // Columns of 100px: the middle one from 100 to 200, its borders across its lines.
    assert_color(&img, 95, 10, ORANGE, "the left border, outside the column");
    assert_color(&img, 105, 10, ORANGE, "the left border, inside the column");
    assert_color(&img, 115, 10, BLUE, "the cell's content");
    assert_color(&img, 185, 10, BLUE, "the cell's content, its end");
    assert_color(&img, 195, 10, ORANGE, "the right border");
}

/// A row group's background image is positioned against the group's box and painted in its
/// cells. Only background colours of table parts were painted.
#[test]
fn a_row_groups_background_image_is_positioned_against_the_group() {
    let img = render(
        "<table style='border-spacing:0'>\
           <tbody style='background:url(blue.png) no-repeat right bottom'>\
             <tr><td style='padding:0;width:50px;height:50px'></td><td style='padding:0;width:50px;height:50px'></td></tr>\
             <tr><td style='padding:0;width:50px;height:50px'></td><td style='padding:0;width:50px;height:50px'></td></tr>\
           </tbody>\
         </table>",
    );
    // The image is drawn once, in the group's bottom right corner (the last cell's).
    assert_color(&img, 95, 95, BLUE, "the image in the group's bottom right corner");
    assert_color(&img, 85, 95, WHITE, "left of the image");
    assert_color(&img, 45, 45, WHITE, "the first cell, where the image is not");
}

/// A row's `::before` and `::after` that are table cells are cells of the row.
#[test]
fn a_rows_pseudo_element_cells() {
    let img = render(
        "<style>.r::before{content:'';display:table-cell;width:30px;height:30px;background:green}\
                 .r::after{content:'';display:table-cell;width:30px;height:30px;background:blue}</style>\
         <div style='display:table'><div class=r style='display:table-row'>\
           <div style='display:table-cell;width:30px;height:30px;background:orange'></div>\
         </div></div>",
    );
    assert_color(&img, 15, 15, GREEN, "the row's ::before cell");
    assert_color(&img, 45, 15, ORANGE, "the row's cell");
    assert_color(&img, 75, 15, BLUE, "the row's ::after cell");
}

/// A table in the collapsing border model has no padding (CSS 2.1 §17.6.2). Its padding moved the
/// cells in.
#[test]
fn a_collapsed_tables_padding_is_ignored() {
    let img = render(
        "<table style='border-collapse:collapse;padding:10px'>\
           <tr><td style='padding:0;width:50px;height:50px;background:green'></td></tr></table>",
    );
    assert_color(&img, 5, 5, GREEN, "the cell, at the table's corner");
}

/// Column groups' and columns' borders take part in collapsed border conflict resolution
/// (css-tables-3 §11.3). They were not offered.
#[test]
fn column_borders_take_part_in_collapsing() {
    let img = render(
        "<table style='border-collapse:collapse'><colgroup style='border:4px solid green'><col><col></colgroup>\
           <tr><td style='padding:0;width:40px;height:40px'></td><td style='padding:0;width:40px;height:40px'></td></tr>\
         </table>",
    );
    assert_color(&img, 1, 20, GREEN, "the column group's left border");
    assert_color(&img, 40, 1, GREEN, "the column group's top border");
    assert_color(&img, 40, 20, WHITE, "no border between the group's columns");
}

/// A row's transform moves its cells (rows are transformable boxes, and their cells are painted
/// by the table). It was ignored.
#[test]
fn a_transformed_rows_cells_move_with_it() {
    let img = render(
        "<table style='border-spacing:0'><tr style='transform:translate(50px,0)'>\
           <td style='padding:0;width:30px;height:30px;background:green'></td></tr></table>",
    );
    assert_color(&img, 65, 15, GREEN, "the cell, moved with its row");
    assert_color(&img, 15, 15, WHITE, "where the cell was");
}

/// A row with `visibility: collapse` takes no height (CSS 2.1 §17.5.5). It was hidden but kept
/// its height.
#[test]
fn a_collapsed_row_takes_no_height() {
    let img = render(
        "<table style='border-spacing:0'>\
           <tr style='visibility:collapse'><td style='padding:0;width:30px;height:30px;background:red'></td></tr>\
           <tr><td style='padding:0;height:30px;background:green'></td></tr></table>",
    );
    assert_color(&img, 15, 15, GREEN, "the second row, at the top");
}

/// A content-box table's `width` holds its border spacing (the spacing is in its content box).
/// It was added around it.
#[test]
fn a_content_box_tables_width_holds_its_spacing() {
    let img = render(
        "<div style='display:table;width:100px;border-spacing:10px;background:green'>\
           <div style='display:table-row'><div style='display:table-cell;height:20px'></div></div></div>",
    );
    assert_color(&img, 95, 5, GREEN, "inside the table's 100px");
    assert_color(&img, 105, 5, WHITE, "past them");
}

/// A table is at least as wide as its caption's min-content width. The caption was not laid out.
#[test]
fn a_table_is_as_wide_as_its_caption() {
    let img = render(
        "<div style='display:table;background:blue'>\
           <div style='display:table-caption;width:200px;height:10px;background:green'></div></div>",
    );
    assert_color(&img, 150, 5, GREEN, "the caption, 200px wide");
}
