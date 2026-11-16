//! `aspect-ratio` sizing (css-sizing-4 §5; Taffy `compute/block.rs` `RatioSizes`, grid
//! `alignment.rs`, stylo_taffy): the automatic minimum size of a box whose size follows its
//! ratio, limits transferred through the ratio only where they apply, `auto <ratio>`, and grid
//! items with an explicit `stretch`. Box sizes are read off the painted pixels, as in WPT's
//! `css/css-sizing/aspect-ratio` reftests.
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
    let width = (0..img.width as usize).take_while(|&x| at(x, 0)).count();
    let height = (0..img.height as usize).take_while(|&y| at(0, y)).count();
    (width, height)
}

#[track_caller]
fn check(body: &str, want: (usize, usize)) {
    assert_eq!(green_size(&render(body)), want, "{body}");
}

/// The plain cases: the `auto` axis follows the other one.
#[test]
fn the_auto_axis_follows_the_ratio() {
    check("<div style='background:green;width:100px;aspect-ratio:2/1'></div>", (100, 50));
    check("<div style='background:green;height:50px;aspect-ratio:2/1'></div>", (100, 50));
    check("<div style='background:green;width:200px;aspect-ratio:1/1;max-height:100px'></div>", (200, 100));
}

/// The automatic minimum size: content taller than the ratio's height makes the box taller (not
/// past `max-height`), content wider than the ratio's width makes it wider (not past `max-width`);
/// an explicit minimum or a scroll container turns it off.
#[test]
fn the_content_is_the_automatic_minimum() {
    check("<div style='background:green;width:100px;aspect-ratio:2/1'><div style='height:100px'></div></div>", (100, 100));
    check(
        "<div style='background:green;width:100px;aspect-ratio:1/2;max-height:100px'><div style='height:200px'></div></div>",
        (100, 100),
    );
    check("<div style='background:green;height:100px;aspect-ratio:1/2'><div style='width:100px'></div></div>", (100, 100));
    check(
        "<div style='background:green;height:100px;aspect-ratio:2/1;max-width:100px'><div style='width:200px'></div></div>",
        (100, 100),
    );
    check(
        "<div style='background:green;width:100px;aspect-ratio:2/1;min-height:0'><div style='height:100px'></div></div>",
        (100, 50),
    );
    check(
        "<div style='background:green;width:100px;aspect-ratio:2/1;overflow:hidden'><div style='height:100px'></div></div>",
        (100, 50),
    );
    // With padding and a border: the ratio is the content box's.
    check(
        "<div style='background:green;aspect-ratio:2/1;width:60px;border:10px solid green;padding:10px'>\
         <div style='height:60px'></div></div>",
        (100, 100),
    );
}

/// A limit in one axis does not override a size given in the other; the size the ratio gives
/// follows the limited one.
#[test]
fn limits_are_not_transferred_into_a_given_size() {
    check("<div style='background:green;aspect-ratio:2/1;min-height:100px;width:100px'></div>", (100, 100));
    check("<div style='background:green;width:100px;aspect-ratio:1/2;max-height:100px'></div>", (100, 100));
    check("<div style='background:green;width:50px;aspect-ratio:1/2;min-width:100px;max-height:100px'></div>", (100, 100));
    check("<div style='background:green;height:50px;aspect-ratio:2/1;min-height:100px;max-width:100px'></div>", (100, 100));
    check("<div style='background:green;height:100px;aspect-ratio:1/2;max-height:150px;min-width:100px'></div>", (100, 100));
    // Both sizes auto: the block axis' minimum goes through the ratio, no further than `max-width`.
    check(
        "<div style='min-height:200px;max-width:100px;aspect-ratio:1/1'><div style='height:100px;background:green'></div></div>",
        (100, 100),
    );
}

/// A float with only a ratio is as wide as its content and as tall as the ratio makes that.
#[test]
fn a_shrink_to_fit_box_takes_its_height_from_its_content_width() {
    check("<div style='background:green;float:left;aspect-ratio:1/1'><div style='width:100px'></div></div>", (100, 100));
}

/// `auto <ratio>` on a non-replaced box is the ratio of the content box, whatever `box-sizing`.
#[test]
fn auto_and_a_ratio_is_the_content_boxs() {
    check(
        "<div style='background:green;width:100px;aspect-ratio:auto 1/1;box-sizing:border-box;padding-left:50px'></div>",
        (100, 50),
    );
    check(
        "<div style='background:green;width:100px;aspect-ratio:2/1;box-sizing:border-box;padding-left:50px'></div>",
        (100, 50),
    );
}

/// A flex or grid container with a ratio is as tall as its content when that is taller; a
/// wrapping column flex container breaks its lines at the ratio's height instead.
#[test]
fn flex_and_grid_containers_have_the_automatic_minimum() {
    for display in ["flex", "grid"] {
        check(
            &format!(
                "<div style='display:{display};width:100px;aspect-ratio:2/1;background:green'>\
                 <div style='width:100%;height:100px'></div></div>"
            ),
            (100, 100),
        );
        check(
            &format!(
                "<div style='display:{display};width:100px;aspect-ratio:2/1;max-height:80px;background:green'>\
                 <div style='width:100%;height:200px'></div></div>"
            ),
            (100, 80),
        );
    }
    check(
        "<div style='display:flex;flex-flow:column wrap;width:100px;aspect-ratio:1/1'>\
         <div style='background:green;width:50px;height:100px'></div><div style='background:green;width:50px;height:100px'></div></div>",
        (100, 100),
    );
}

/// A grid item with a ratio: an explicit `stretch` fills the grid area in its axis, and the
/// other axis follows through the ratio unless it stretches or is sized too.
#[test]
fn grid_items_stretch_explicitly() {
    let grid = |area: &str, item: &str| {
        format!("<div style='display:grid;grid-template:{area}'><div style='background:green;{item}'></div></div>")
    };
    check(&grid("100px / 50px", "aspect-ratio:1/1;align-self:stretch"), (100, 100));
    check(&grid("100px / 100px", "aspect-ratio:2/1;align-self:stretch;justify-self:stretch"), (100, 100));
    check(&grid("100px / 100px", "aspect-ratio:2/1;align-self:stretch;width:100%"), (100, 100));
    check(&grid("100px / 100px", "aspect-ratio:2/1;height:100%;justify-self:stretch"), (100, 100));
    check(&grid("50px / 100px", "height:50px;aspect-ratio:1/2;justify-self:stretch"), (100, 50));
    // Without an explicit stretch the item is as wide as its area and the ratio gives its height.
    check(&grid("100px / 100px", "aspect-ratio:2/1"), (100, 50));
}

/// A grid container's `auto` height follows the ratio from its used width: `min-height: 60px`
/// through the ratio makes `repeat(auto-fill, 50px)` two columns, and the 100px width a 100px
/// height, not the 60px minimum (Taffy `compute/grid/mod.rs`; WPT
/// `css/css-grid/grid-definition/grid-auto-repeat-aspect-ratio-001`).
#[test]
fn a_grid_containers_auto_height_follows_the_ratio() {
    check(
        "<div style='display:inline-grid;background:green;aspect-ratio:1/1;min-height:60px;\
           grid-template-columns:repeat(auto-fill,50px);vertical-align:top'></div>",
        (100, 100),
    );
}

/// A non-replaced flex item of a preferred aspect ratio: its automatic minimum size is the larger
/// of its content and its definite cross size through the ratio; a preferred main size gives no
/// cross size (the flexed one does); a max-content flex base size follows the ratio from its
/// min cross size; `auto <ratio>` is between content boxes (WPT css-sizing `flex-aspect-ratio-*`).
#[test]
fn flex_items_with_a_ratio() {
    check(
        "<div style='display:flex'><div style='background:green;height:100px;aspect-ratio:1/2;flex-basis:0'>\
         <div style='width:100px'></div></div></div>",
        (100, 100),
    );
    check(
        "<div style='display:flex;width:0;height:100px'><div style='background:green;aspect-ratio:1'></div></div>",
        (100, 100),
    );
    check(
        "<div style='display:flex;width:100px'><div style='background:green;width:50px;aspect-ratio:1;min-width:0;flex:1'></div></div>",
        (100, 100),
    );
    check(
        "<div style='display:flex;flex-flow:column;align-items:start'>\
         <div style='aspect-ratio:1;flex:0 0 auto;min-height:0;min-width:100px;background:green'></div></div>",
        (100, 100),
    );
    check(
        "<div style='display:flex;width:1px'><div style='background:green;padding-top:15px;padding-left:10px;\
         box-sizing:border-box;height:25px;aspect-ratio:auto 19/1'></div></div>",
        (200, 25),
    );
}
