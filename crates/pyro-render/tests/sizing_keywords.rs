//! Sizing keywords in `min-width`/`max-width`/`min-height`/`max-height` of block-level boxes
//! (css-sizing-3 §5.1: `min-content`, `max-content`, `fit-content`; css-sizing-4: `stretch`;
//! Taffy `compute/block.rs` `clamp_width_by_keywords`), and percentages in `calc()` margins in
//! intrinsic widths. Box sizes are read off the painted pixels, as in WPT's `css/css-sizing`
//! reftests (`block-size-with-min-or-max-content-*`, `fit-content-min-inline-size`,
//! `stretch/min-width-1`, `calc-margins-block`).
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
        height: 300,
        device_scale_factor: 1.0,
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
fn check(body: &str, want: (usize, usize)) {
    assert_eq!(green_size(&render(body)), want, "{body}");
}

/// The content height bounds a given height from below (`min-height`) or above (`max-height`).
#[test]
fn content_keywords_bound_the_height() {
    let content = "<div style='height:100px'></div>";
    check(&format!("<div style='background:green;width:100px;min-height:max-content;height:0'>{content}</div>"), (100, 100));
    check(&format!("<div style='background:green;width:100px;max-height:max-content;height:200px'>{content}</div>"), (100, 100));
    check(&format!("<div style='background:green;width:100px;min-height:min-content;height:150px'>{content}</div>"), (100, 150));
    check(
        "<div style='background:green;display:grid;grid-template-rows:100px;width:100px;min-height:max-content;height:0'></div>",
        (100, 100),
    );
}

/// The content width under the keyword's constraint bounds a given width.
#[test]
fn content_keywords_bound_the_width() {
    let boxes = "<span style='display:inline-block;width:50px;height:20px'></span><span style='display:inline-block;width:50px;height:20px'></span>";
    check(&format!("<div style='font-size:0;background:green;min-width:max-content;width:10px'>{boxes}</div>"), (100, 20));
    check(&format!("<div style='font-size:0;background:green;min-width:min-content;width:10px'>{boxes}</div>"), (50, 40));
    check(&format!("<div style='font-size:0;background:green;max-width:max-content;width:300px'>{boxes}</div>"), (100, 20));
    // A parent sized by its content counts the keyword too.
    check(
        &format!("<div style='font-size:0;width:fit-content;background:green'><div style='min-width:fit-content;width:10px'>{boxes}</div></div>"),
        (100, 20),
    );
    // A width the aspect ratio gives is an intrinsic width: `max-width: max-content` keeps it.
    check("<div style='background:green;max-width:max-content;height:100px;aspect-ratio:1'></div>", (100, 100));
}

/// `stretch` as a minimum: the containing block's width, or its definite height.
#[test]
fn stretch_bounds_from_below() {
    check("<div style='width:100px'><div style='width:50px;min-width:stretch;height:100px;background:green'></div></div>", (100, 100));
    check("<div style='height:100px'><div style='width:100px;min-height:stretch;background:green'></div></div>", (100, 100));
}

/// A percentage in a `calc()` margin is zero in an intrinsic width; the rest of it counts.
#[test]
fn calc_margins_count_in_intrinsic_widths() {
    check(
        "<div style='width:min-content;height:100px;background:green'><div style='margin-left:calc(10% + 100px)'></div></div>",
        (100, 100),
    );
}
