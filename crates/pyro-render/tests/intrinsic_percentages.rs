//! Replaced elements' content sizes in min/max sizes (`min-width: min-content` on a canvas is
//! the width its height gives through the ratio; WPT css-sizing `replaced-*-min-content`),
//! percentage heights in a float's intrinsic width and the quirks-mode percentage height quirk
//! (`intrinsic-percent-replaced-*`), and percentages in the children of a box whose width its
//! content gave (`intrinsic-percent-non-replaced-*`).
use std::sync::Arc;

use pyro_render::*;

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}

fn render(body: &str, quirks_mode: bool) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let doctype = if quirks_mode { "" } else { "<!doctype html>" };
    let snapshot = Snapshot {
        html: format!("{doctype}<body style='margin:0;background:#fff'>{body}"),
        base_url: "http://test/".into(),
        width: 400,
        height: 300,
        device_scale_factor: 1.0,
        quirks_mode,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

const GREEN: [u8; 3] = [0, 128, 0];

/// The width and height of the green area that starts at the top left corner.
fn green_size(img: &RgbaImage) -> (usize, usize) {
    let at = |x: usize, y: usize| {
        let i = (y * img.width as usize + x) * 4;
        [img.data[i], img.data[i + 1], img.data[i + 2]] == GREEN
    };
    ((0..img.width as usize).take_while(|&x| at(x, 0)).count(), (0..img.height as usize).take_while(|&y| at(0, y)).count())
}

#[track_caller]
fn check(body: &str, quirks_mode: bool, want: (usize, usize)) {
    assert_eq!(green_size(&render(body, quirks_mode)), want, "{body}");
}

#[test]
fn replaced_content_keywords_in_min_and_max_sizes() {
    let canvas = |style: &str| format!("<canvas width=10 height=10 style='display:block;background:green;{style}'></canvas>");
    check(&canvas("width:1000px;height:100px;max-width:min-content"), false, (100, 100));
    check(&canvas("width:10px;height:100px;min-width:min-content"), false, (100, 100));
    check(&canvas("width:100px;height:1000px;max-height:min-content"), false, (100, 100));
    check(&canvas("width:100px;height:10px;min-height:min-content"), false, (100, 100));
}

#[test]
fn percentage_heights_in_a_floats_intrinsic_width() {
    let page = "<div style='float:left;height:100px;background:green'><div><canvas width=10 height=10 \
                style='height:100%;vertical-align:top'></canvas></div></div>";
    // Quirks mode: the auto-height div hands the float's height on
    check(page, true, (100, 100));
    // Standards mode: the auto-height div makes the percentage `auto`
    check(page, false, (10, 100));
    // A table cell's child keeps its own basis even in quirks mode
    check(
        "<div style='display:table-cell;height:100px;background:green'><div style='width:100px'>\
         <div style='height:100%;background:red'></div></div></div>",
        true,
        (100, 100),
    );
}

#[test]
fn percentages_against_a_shrink_to_fit_width() {
    // The float is 50px wide (its content's); the child's `min-width: 200%` makes it 100px.
    check(
        "<div style='float:left;height:20px'><div style='width:50px'></div>\
         <div style='min-width:200%;height:30px;background:green'></div></div>",
        false,
        (100, 30),
    );
}
