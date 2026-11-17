//! WPT `css/css-align` details of round 3 lane CF: the static-position rectangle of a block-level
//! absolutely positioned box in an inline, `anchor-center` without an anchor, collapsed table rows
//! sized before they collapse, and `justify-items` skipping anonymous and split-inline boxes.
use std::sync::Arc;

use pyro_render::*;

const GREEN: [u8; 3] = [0, 128, 0];
const WHITE: [u8; 3] = [255, 255, 255];

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("404".into()));
    }
}

/// `body` on a white page without margins, 400x400 CSS px.
fn render(body: &str) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let snapshot = Snapshot {
        html: format!("<!doctype html><body style='margin:0;background:#fff'>{body}"),
        base_url: "http://test/".into(),
        width: 400,
        height: 400,
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

/// A block-level absolutely positioned box after text in an inline is aligned in the next line's
/// block extent, with no inline extent at the line start (WPT `align-self-static-position-003`):
/// `end` puts its bottom-right corner at (line start, end of the next line).
#[test]
fn block_level_abspos_in_an_inline_aligns_in_the_next_line() {
    let img = render(
        "<div style='position:relative;margin:100px;width:100px;height:100px'>\
         <span style='font:10px/25px monospace;color:transparent'>text\
         <div style='position:absolute;width:50px;height:50px;background:green;justify-self:end;align-self:end'></div>\
         <br>text</span></div>",
    );
    // Static position (100, 125); the box ends there in x and at the next line's end (150) in y.
    assert_color(&img, 75, 125, GREEN, "inside the box");
    assert_color(&img, 105, 125, WHITE, "right of the line start");
    assert_color(&img, 75, 95, WHITE, "above the box");
}

/// `anchor-center` parses, and without a default anchor centres as `center` does.
#[test]
fn anchor_center_without_anchor_is_center() {
    let img = render(
        "<div style='position:relative;width:200px;height:100px'>\
         <div style='position:absolute;inset:0;margin:auto;width:50px;height:50px;background:red'></div>\
         <div style='position:absolute;inset:0;width:50px;height:50px;background:green;justify-self:anchor-center;align-self:anchor-center'></div></div>",
    );
    assert_color(&img, 100, 50, GREEN, "centred box");
    assert_color(&img, 10, 10, WHITE, "start corner");
}

/// A `visibility: collapse` row a cell spans takes its share of the cell's content before it
/// collapses: the table is as tall as the open row (WPT `align-content-table-cell-002`).
#[test]
fn a_collapsed_row_is_sized_before_it_collapses() {
    let img = render(
        "<table style='overflow:hidden' cellspacing=0 cellpadding=0><tr style='height:100px'>\
         <td rowspan=2><div style='width:50px;height:200px;background:linear-gradient(green 50%, red 50%)'></div></td></tr>\
         <tr style='height:100px;visibility:collapse'></tr></table>",
    );
    assert_color(&img, 25, 50, GREEN, "open row");
    assert_color(&img, 25, 150, WHITE, "below the table");
}

/// `justify-items` does not reach the content of an anonymous block, nor the block inside an inline
/// (WPT `justify-items-anonymous`, `justify-self-block-in-inline-002`).
#[test]
fn justify_items_skips_anonymous_and_split_inline_boxes() {
    let img = render(
        "<div style='width:200px;justify-items:right'>\
         <span style='display:inline-block;width:100px;height:50px;background:green'></span><div></div></div>\
         <div style='width:200px;justify-items:right'><span style='justify-items:center'>\
         <div style='min-width:50px;height:50px;background:green'></div></span></div>",
    );
    assert_color(&img, 10, 25, GREEN, "inline-block at the start");
    assert_color(&img, 150, 25, WHITE, "not at the right");
    let y = (50..120).find(|&y| pixel(&img, 10, y) == GREEN).expect("second box");
    assert_color(&img, 190, y + 5, GREEN, "block in an inline stretches");
}

/// An empty cell with `empty-cells: hide` paints neither background nor border in the separated
/// borders model (WPT CSS2 `empty-cells-applies-to-014`); one with content does.
#[test]
fn empty_cells_hide_paints_no_box() {
    let img = render(
        "<table style='border-spacing:0;empty-cells:hide'><tr>\
         <td style='width:50px;height:50px;padding:0;background:red;border:5px solid red'> </td>\
         <td style='width:50px;height:50px;padding:0;background:green'><span></span>&nbsp;</td></tr></table>",
    );
    assert_color(&img, 30, 30, WHITE, "empty cell");
    assert_color(&img, 2, 2, WHITE, "empty cell border");
    assert_color(&img, 85, 30, GREEN, "cell with content");
}

/// A floated root element shrinks to fit and floats to its side of the initial containing block
/// (WPT CSS2 `floats/float-root`).
#[test]
fn a_floated_root_shrinks_to_its_side() {
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let snapshot = Snapshot {
        html: "<!doctype html><html style='float:right;background:#fff'><body style='margin:0'>\
               <div style='width:50px;height:50px;background:green'></div>"
            .into(),
        base_url: "http://test/".into(),
        width: 400,
        height: 400,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    let img = Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0;
    assert_color(&img, 375, 25, GREEN, "at the right edge");
    assert_color(&img, 25, 25, WHITE, "not at the left edge");
}

/// Margins do not collapse through a block whose line holds text, however short its line is
/// (`font-size: 0`; WPT css-fonts `font-size-zero-1`).
#[test]
fn margins_do_not_collapse_through_a_zero_height_text_line() {
    let img = render(
        "<div style='height:20px;background:green;margin-bottom:20px'></div>\
         <div style='font-size:0;line-height:0'>text</div>\
         <div style='height:20px;background:green;margin-top:20px'></div>",
    );
    assert_color(&img, 10, 50, WHITE, "the gap is both margins");
    assert_color(&img, 10, 65, GREEN, "second box");
}

/// A flex scroll container gives the inline-block holding it its contents' baseline; a block one
/// gives its bottom margin edge (WPT css-align `baseline-of-scrollable-1b`).
#[test]
fn a_flex_scroll_container_gives_its_contents_baseline() {
    let img = render(
        "<div style='font:20px/20px monospace'>\
         <span style='display:inline-block;width:10px;height:10px;background:green'></span>\
         <div style='display:inline-block'><div style='display:flex;overflow:hidden;height:100px;width:50px'>x</div></div>\
         </div>",
    );
    // The square sits on the text's baseline, in the first 20px line, not at the box's bottom.
    let y = (0..200).find(|&y| pixel(&img, 5, y) == GREEN).expect("square");
    assert!(y < 30, "square top at {y}");
}

/// A no-break space is text: a zero-height line of it keeps margins apart as a line of letters does
/// (WPT css-color `t32-opacity-offscreen-multiple-boxes-2-c`, whose reference is `&nbsp;` text).
#[test]
fn a_zero_height_line_of_no_break_spaces_is_text() {
    for text in ["XX", "&nbsp;&nbsp;"] {
        let img = render(&format!(
            "<div style='height:20px;background:green;margin-bottom:20px'></div>\
             <div style='line-height:0'>{text}</div>\
             <div style='height:20px;background:green;margin-top:20px'></div>"
        ));
        assert_color(&img, 10, 50, WHITE, "the gap is both margins");
        assert_color(&img, 10, 65, GREEN, "second box");
    }
}

