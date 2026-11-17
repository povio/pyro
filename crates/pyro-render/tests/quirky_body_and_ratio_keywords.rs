//! The quirky html and body fill the viewport (Quirks Mode §3.6/§3.7, blitz-dom `quirky_fill_height`),
//! percentages in a shrink-to-fit box's children's margins count as zero in its width, a ratio-given
//! size is the box's intrinsic size for `min-*: min-content`, and `aspect-ratio` skips internal table
//! boxes. Pixel checks after WPT css-sizing `stretch/stretch-quirk-001`, css-flexbox
//! `flexbox_quirks_body`, css-sizing `abspos-auto-sizing-fit-content-percentage-001`,
//! `aspect-ratio/intrinsic-size-010`, `aspect-ratio/table-element-001`.
use std::sync::Arc;

use pyro_render::*;

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}

fn render(html: String, quirks_mode: bool) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let snapshot = Snapshot {
        html,
        base_url: "http://test/".into(),
        width: 300,
        height: 200,
        device_scale_factor: 1.0,
        quirks_mode,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

fn standards(body: &str) -> RgbaImage {
    render(format!("<!doctype html><body style='margin:0;background:#fff'>{body}"), false)
}

fn pixel(img: &RgbaImage, x: usize, y: usize) -> [u8; 3] {
    let i = (y * img.width as usize + x) * 4;
    [img.data[i], img.data[i + 1], img.data[i + 2]]
}

const WHITE: [u8; 3] = [255, 255, 255];
const GREEN: [u8; 3] = [0, 128, 0];
const YELLOW: [u8; 3] = [255, 255, 0];

#[track_caller]
fn check(img: &RgbaImage, points: &[(usize, usize, [u8; 3])]) {
    for &(x, y, want) in points {
        assert_eq!(pixel(img, x, y), want, "pixel at ({x}, {y})");
    }
}

/// In quirks mode `height: stretch` on an inline-block in the body fills the viewport: the body's
/// auto height is the viewport's less its margins, and definite.
#[test]
fn quirky_body_is_definite_for_stretch() {
    let img = render(
        "<body style='margin:0;background:#fff'>\
           <div style='display:inline-block;width:40px;height:stretch;background:green'></div>"
            .into(),
        true,
    );
    check(&img, &[(20, 10, GREEN), (20, 190, GREEN), (60, 100, WHITE)]);
}

/// A flex body in quirks mode fills the viewport less its margins (and the html element's).
#[test]
fn quirky_flex_body_fills_the_viewport() {
    let img = render(
        "<html style='margin:0;background:#fff'><body style='display:flex;margin:10px;background:yellow'>x".into(),
        true,
    );
    check(&img, &[(150, 180, YELLOW), (150, 195, WHITE)]);
    // Not in standards mode.
    let img = render(
        "<!doctype html><html style='margin:0;background:#fff'><body style='display:flex;margin:10px;background:yellow'>x"
            .into(),
        false,
    );
    check(&img, &[(150, 180, WHITE)]);
}

/// An abspos box's width counts its child's `margin-left: -50%` as zero: the box is as wide as
/// the 100px child, which then sits 50px to the left.
#[test]
fn percentage_margins_count_as_zero_in_a_shrink_to_fit_width() {
    let img = standards(
        "<div style='position:relative;width:100px;height:100px;margin-left:100px'>\
           <div style='position:absolute;background:green'><div style='width:100px;height:100px;margin-left:-50%'></div></div>\
         </div>",
    );
    check(&img, &[(105, 50, GREEN), (195, 50, GREEN), (205, 50, WHITE)]);
}

/// `min-width: min-content` on a box whose width comes from its ratio and height is that width:
/// the 150px content does not widen it. Same for `min-height: min-content` and a 50px content.
#[test]
fn ratio_sizes_are_the_min_content_size() {
    let img = standards(
        "<div style='height:25px;min-width:min-content;aspect-ratio:4/1;width:auto;display:inline-block;background:green'>\
           <div style='width:150px'></div></div>",
    );
    check(&img, &[(95, 10, GREEN), (105, 10, WHITE)]);
    let img = standards(
        "<div style='width:100px;min-height:min-content;aspect-ratio:4/1;background:green'><div style='height:50px'></div></div>",
    );
    check(&img, &[(50, 20, GREEN), (50, 30, WHITE)]);
}

/// `aspect-ratio` does not apply to table cells: a 100px wide cell with `aspect-ratio: 1/1` is as
/// tall as its row (50px), not 100px.
#[test]
fn aspect_ratio_skips_table_cells() {
    let img = standards(
        "<table style='border-collapse:collapse'><tr>\
           <td style='padding:0;width:100px;aspect-ratio:1/1;background:green'></td>\
           <td style='padding:0;height:50px;width:10px'></td></tr></table>",
    );
    check(&img, &[(50, 45, GREEN), (50, 55, WHITE)]);
}
