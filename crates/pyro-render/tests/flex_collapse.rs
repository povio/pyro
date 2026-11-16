//! `visibility: collapse` on flex items (css-flexbox-1 §4.4, §9.4 step 10; Taffy `compute/flexbox.rs`,
//! `measure_collapsed_item_struts`): a collapsed item is not rendered and takes no main size, but its line
//! keeps the cross size it had with the item in (its strut). Pixel checks after WPT's
//! `css/css-flexbox/flexbox-collapsed-item-horiz-*` and `gap-collapse`.
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
const BLUE: [u8; 3] = [0, 0, 255];

#[track_caller]
fn check(img: &RgbaImage, points: &[(usize, usize, [u8; 3])]) {
    for &(x, y, want) in points {
        assert_eq!(pixel(img, x, y), want, "pixel at ({x}, {y})");
    }
}

/// A collapsed 50px tall item beside a 10px tall one: it is not painted and takes no width, but the
/// line, and so the stretched-to container, stays 50px tall.
#[test]
fn a_collapsed_item_leaves_its_lines_cross_size() {
    let img = render(
        "<div style='display:inline-flex;background:green;vertical-align:top'>\
           <div style='width:40px;height:50px;background:blue;visibility:collapse'></div>\
           <div style='width:20px;height:10px;background:blue'></div>\
         </div>",
    );
    check(&img, &[(10, 5, BLUE), (10, 45, GREEN), (25, 25, WHITE), (10, 55, WHITE)]);
}

/// Gaps are only between the items that are not collapsed: two 20px items 10px apart.
#[test]
fn a_collapsed_item_has_no_gaps_around_it() {
    let img = render(
        "<div style='display:flex;gap:10px;height:20px'>\
           <div style='width:20px;background:blue'></div>\
           <div style='width:20px;background:green;visibility:collapse'></div>\
           <div style='width:20px;background:blue'></div>\
         </div>",
    );
    check(&img, &[(15, 10, BLUE), (25, 10, WHITE), (35, 10, BLUE), (45, 10, BLUE), (55, 10, WHITE)]);
}

/// A collapsed item in its own line (30px wide items in a 30px wide wrapping container) joins the line
/// before it once it has no width, and brings its 40px strut along: two lines, the first 40px tall.
#[test]
fn a_collapsed_item_joins_the_line_before_it_with_its_strut() {
    let img = render(
        "<div style='display:flex;flex-wrap:wrap;width:30px;align-items:flex-start'>\
           <div style='width:30px;height:10px;background:blue'></div>\
           <div style='width:30px;height:40px;background:green;visibility:collapse'></div>\
           <div style='width:30px;height:10px;background:green'></div>\
         </div>",
    );
    check(&img, &[(10, 5, BLUE), (10, 25, WHITE), (10, 45, GREEN), (10, 55, WHITE)]);
}
