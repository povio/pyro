//! Grid and flex sizing (Taffy `compute/grid`, `compute/flexbox.rs`): flexible tracks whose factors sum below
//! one are sized again at the container's definite size, an item spanning a flexible track gives it only its
//! minimum contribution under a content constraint, inputs are no scroll containers for layout, `stretch` as a
//! flex item's cross size is definite whatever `align-self`, and a ratio-only replaced item stretch-fits a row.
//! Pixel checks after WPT css-grid `grid-template-flexible-rerun-track-sizing`, `grid-flex-spanning-items-001`,
//! `stretch-grid-item-text-input-overflow`, css-sizing `stretch/flexbox-auto-minimum-002`, css-flexbox
//! `svg-root-as-flex-item-003`.
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

/// An inline grid with `minmax(0, .5fr)` tracks around a 200px item is 100px square (the factor sum counts as
/// one against its indefinite size), and its tracks are then half of that: an abspos box of 200% of the area
/// is 100px square.
#[test]
fn fractional_flex_tracks_are_sized_again_at_the_definite_size() {
    let img = render(
        "<div style='display:inline-grid;position:relative;grid-template-columns:minmax(0,.5fr);grid-template-rows:minmax(0,.5fr)'>\
           <div style='width:200px;height:200px'></div>\
           <div style='grid-area:1/1/2/2;position:absolute;width:200%;height:200%;background:green'></div>\
         </div>",
    );
    check(&img, &[(95, 95, GREEN), (105, 50, WHITE), (50, 105, WHITE)]);
}

/// A `min-content` grid with `1fr 30px` columns and a 300px item spanning both is 30px wide.
#[test]
fn spanning_items_give_flexible_tracks_their_minimum_contribution() {
    let img = render(
        "<div style='display:grid;grid-template-columns:1fr 30px;width:min-content;background:green;height:20px'>\
           <div style='grid-column:1/span 2'><div style='width:300px'></div></div>\
         </div>",
    );
    check(&img, &[(25, 10, GREEN), (35, 10, WHITE)]);
}

/// A 300px wide `<input>` with `overflow: hidden` in a 100px column keeps its width: it is no scroll
/// container, whose automatic minimum size would be zero.
#[test]
fn inputs_are_no_scroll_containers_for_layout() {
    let img = render(
        "<div style='display:grid;width:100px;grid-template-columns:auto'>\
           <input style='overflow:hidden;width:300px;appearance:none;border:0;padding:0;background:green;height:20px'>\
         </div>",
    );
    check(&img, &[(250, 10, GREEN)]);
}

/// A canvas with `height: stretch`, `align-self: start` and `flex-basis: 0` in a 200px tall row is at least
/// as wide as its stretched height gives through its ratio.
#[test]
fn stretch_cross_size_transfers_to_the_automatic_minimum() {
    let img = render(
        "<div style='display:flex;width:300px;height:200px'>\
           <canvas width=100 height=100 style='height:stretch;align-self:start;flex-basis:0;background:green'></canvas>\
         </div>",
    );
    check(&img, &[(195, 100, GREEN), (205, 100, WHITE)]);
}

/// An SVG with only a `viewBox` in a 150px row with a 50px right margin and no shrink is 100px wide.
#[test]
fn ratio_only_replaced_items_stretch_fit_a_row() {
    let img = render(
        "<div style='display:flex;width:150px'>\
           <svg viewBox='0 0 200 200' style='margin-right:50px;flex:1 0 auto;background:green'></svg>\
         </div>",
    );
    check(&img, &[(95, 50, GREEN), (105, 50, WHITE), (50, 95, GREEN), (50, 105, WHITE)]);
}
