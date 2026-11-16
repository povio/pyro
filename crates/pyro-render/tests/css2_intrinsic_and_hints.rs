//! WPT `css/CSS2` details of round 3 lane C2T: `<br clear>`, text indents in min-content widths,
//! percentage margins of atomic inlines, `rowspan="0"` and tables without rows.
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

/// `<br clear="all">` clears the floats before it (HTML's `clear` presentational hint).
#[test]
fn br_clear_clears_floats() {
    let img = render(
        "<div style='float:left;width:50px;height:50px;background:green'></div><br clear=all>\
         <div style='height:20px;background:blue'></div>",
    );
    assert_color(&img, 100, 10, WHITE, "beside the float");
    assert_color(&img, 100, 60, BLUE, "below the float");
}

/// The text indent of the first line adds to the min-content width of an atomic inline at its
/// start (an empty text item before the box is no content to wrap after; WPT
/// `text-indent-intrinsic-001`).
#[test]
fn a_text_indent_adds_to_a_lone_atomic_inline() {
    let img = render(
        "<div style='width:min-content;text-indent:36px;background:blue;line-height:0'>\
         <span style='display:inline-block;width:12px;height:12px;background:green'></span></div>",
    );
    assert_color(&img, 42, 6, GREEN, "the box after the indent");
}

/// A line made negative by a negative text indent still wraps under a min-content constraint
/// (WPT `text-indent-intrinsic-003`): the second box, on a line of its own, sets the width.
#[test]
fn a_negative_indent_still_wraps_at_min_content() {
    let img = render(
        "<div style='width:min-content;text-indent:-36px;line-height:12px;background:blue;padding-left:40px'>\
         <span style='display:inline-block;width:12px;height:12px'></span>&#8203;\
         <span style='display:inline-block;width:36px;height:12px'></span></div>",
    );
    // 40px of padding and the 36px box (it was 12px wide: the line did not wrap).
    assert_color(&img, 70, 5, BLUE, "inside the box");
    assert_color(&img, 80, 5, WHITE, "past the box");
}

/// An atomic inline's percentage margins are of the line's root (zero while it is measured), not
/// of the root's parent (WPT `intrinsic-size-with-negative-margins`).
#[test]
fn atomic_inline_percentage_margins_are_of_their_root() {
    let img = render(
        "<div style='display:inline-block;border:solid green 25px;line-height:0'>\
         <div style='display:inline-block;width:50px;height:50px;margin-right:-100%;background:green'></div></div>",
    );
    assert_color(&img, 95, 50, GREEN, "right border");
    assert_color(&img, 105, 50, WHITE, "past the box");
}

/// `rowspan="0"` spans the rest of the row group (WPT `zero-rowspan-001`).
#[test]
fn rowspan_zero_spans_the_row_group() {
    let img = render(
        "<table style='border-spacing:0'><tr><td rowspan=0 style='padding:0;width:20px;background:green'></td>\
         <td style='padding:0;width:20px;height:20px'></td></tr><tr><td style='padding:0;height:20px'></td></tr></table>",
    );
    assert_color(&img, 10, 30, GREEN, "spanning cell, second row");
    assert_color(&img, 30, 30, WHITE, "second row's own cell");
}

/// A table without rows has no border spacing: a caption alone is the table (WPT
/// `caption-relative-positioning`).
#[test]
fn a_table_without_rows_has_no_border_spacing() {
    let img = render(
        "<table style='border-spacing:10px;background:blue'><caption style='height:20px;width:50px;background:green'></caption></table>\
         <div style='height:10px;background:green;width:50px'></div>",
    );
    assert_color(&img, 25, 25, GREEN, "the block right under the caption");
}
