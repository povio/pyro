//! Lane SP: transformed table rows and row groups contain absolutely positioned and fixed boxes
//! (laid out against the part, painted in its transform), `calc()` margins against an indefinite
//! basis, a replaced keyword minimum over a length maximum, `background-clip: text` of a table
//! part. Pixel checks after WPT css-transforms `transform-transformed-tr-percent-height-child`,
//! `transform-transformed-tbody-contains-fixed-position`, css-sizing `calc-margins-flex`,
//! `replaced-aspect-ratio-stretch-fit-003`, css-backgrounds `clip-text-background-table-cell`.
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

const RED: [u8; 3] = [255, 0, 0];

/// A translated row is the containing block of an abspos box in its cell: `100%` is the row's
/// height (50px), and the box moves with the row's transform (10px right).
#[test]
fn transformed_row_contains_abspos_and_moves_it() {
    let img = render(
        "<table style='border-spacing:0'><tr style='transform:translateX(10px)'>\
           <td style='width:50px;height:50px;padding:0'><div style='position:absolute;left:0;top:0;width:20px;height:100%;background:green'></div></td>\
         </tr></table>",
    );
    check(&img, &[(15, 45, GREEN), (5, 25, WHITE), (15, 55, WHITE)]);
}

/// A translated row group contains a `position: fixed` box: it sits at the group's corner and
/// moves with it.
#[test]
fn transformed_row_group_contains_fixed() {
    let img = render(
        "<div style='height:50px'></div><table style='border-spacing:0'><tbody style='transform:translateX(20px)'><tr>\
           <td style='width:50px;height:50px;padding:0'><div style='position:fixed;left:0;top:0;width:20px;height:20px;background:green'></div></td>\
         </tr></tbody></table>",
    );
    check(&img, &[(25, 55, GREEN), (5, 5, WHITE), (5, 55, WHITE)]);
}

/// A `calc(10% + 100px)` margin is 100px in a `min-content` flex container's width.
#[test]
fn calc_margin_counts_its_length_in_an_intrinsic_width() {
    let img = render(
        "<div style='display:flex;width:min-content;height:50px;background:green'><div style='margin-left:calc(10% + 100px)'></div></div>",
    );
    check(&img, &[(95, 25, GREEN), (105, 25, WHITE)]);
}

/// `min-width: max-content` (the 100px height through the 1:1 ratio) wins over `max-width: 50px`.
#[test]
fn replaced_keyword_minimum_wins_over_max_width() {
    let img = render(
        "<div style='width:100px;height:100px'><svg viewBox='0 0 1 1' style='display:block;background:green;min-width:max-content;max-width:50px;height:100%'></svg></div>",
    );
    check(&img, &[(95, 95, GREEN), (105, 50, WHITE)]);
}

/// A row's `background-clip: text` shows in its cell's glyphs only, not over the cell.
#[test]
fn table_part_background_clips_to_cell_text() {
    let img = render(
        "<table style='border-spacing:0'><tr style='background:red;background-clip:text;color:transparent'>\
           <td style='width:100px;height:60px;padding:0;vertical-align:top;font:20px monospace'>.</td>\
         </tr></table>",
    );
    assert_ne!(pixel(&img, 90, 50), RED);
}

fn render_quirks(body: &str, quirks_mode: bool, limited_quirks_mode: bool) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let snapshot = Snapshot {
        html: format!("<body style='margin:0;background:#fff'>{body}"),
        base_url: "http://test/".into(),
        width: 300,
        height: 200,
        device_scale_factor: 1.0,
        quirks_mode,
        limited_quirks_mode,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

/// The line height calculation quirk: in quirks and limited-quirks mode a line holding only an
/// inline-block has no strut, so no gap under it; in standards mode the strut's descent shows.
#[test]
fn line_without_text_has_no_strut_in_quirks_modes() {
    let body = "<div style='background:red;font:20px serif'><span style='display:inline-block;width:50px;height:50px;background:green'></span></div>";
    for (quirks, limited) in [(true, false), (false, true)] {
        let img = render_quirks(body, quirks, limited);
        check(&img, &[(25, 49, GREEN), (25, 52, WHITE)]);
    }
    let img = render_quirks(body, false, false);
    assert_eq!(pixel(&img, 25, 52), RED);
}

/// `height: stretch` on an inline-block and a button fills their container's definite height.
#[test]
fn stretch_height_fills_definite_container_for_atomic_inlines() {
    let img = render(
        "<div style='height:120px;font:10px monospace'>\
           <div style='display:inline-block;height:stretch;width:40px;background:green;vertical-align:top'></div>\
           <button style='height:stretch;width:40px;background:green;border:0;vertical-align:top'></button>\
         </div>",
    );
    check(&img, &[(20, 110, GREEN)]);
    assert_eq!(pixel(&img, 20, 125), WHITE);
}

/// `min-height: min-content` grows a 10px inline-block to its content; `max-height:
/// max-content` shrinks a 120px one to it.
#[test]
fn content_keywords_in_min_and_max_height_of_inline_blocks() {
    let img = render(
        "<div style='font:20px/40px monospace;color:transparent'>\
           <div style='display:inline-block;vertical-align:top;width:40px;height:10px;min-height:min-content;background:green'>x</div>\
           <div style='display:inline-block;vertical-align:top;width:40px;height:120px;max-height:max-content;background:green'>x</div>\
         </div>",
    );
    check(&img, &[(20, 35, GREEN), (70, 35, GREEN), (70, 60, WHITE)]);
}
