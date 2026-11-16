//! Sizing keywords in a flex item's `min-width`/`min-height`/`max-width`/`max-height` (Taffy
//! `compute/flexbox.rs`, `resolve_size_keyword`): the item measured under the keyword's constraint.
//! Pixel checks after WPT's `css/css-flexbox/flex-item-max-width-min-content` and
//! `flex-item-min-height-min-content-overflow`.
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

/// `max-width: min-content` caps a 200px flex basis at the item's 100px min-content width.
#[test]
fn max_width_min_content_caps_a_flex_item() {
    let img = render(
        "<div style='display:flex;width:200px;height:50px'>\
           <div style='max-width:min-content;flex-basis:200px;background:green'><div style='width:100px;height:10px'></div></div>\
         </div>",
    );
    check(&img, &[(95, 25, GREEN), (105, 25, WHITE)]);
}

/// `min-height: min-content` keeps a scroll container in a 20px tall column at its 60px content.
#[test]
fn min_height_min_content_holds_a_column_item_at_its_content() {
    let img = render(
        "<div style='display:flex;flex-direction:column;height:20px;width:100px'>\
           <div style='min-height:min-content;overflow:hidden;background:green'><div style='height:60px'></div></div>\
         </div>",
    );
    check(&img, &[(10, 55, GREEN), (10, 65, WHITE)]);
}

/// `max-width: min-content` in a column's cross axis: the stretched item is only as wide as its
/// 100px min-content width.
#[test]
fn max_width_min_content_caps_a_stretched_column_item() {
    let img = render(
        "<div style='display:flex;flex-direction:column;width:200px'>\
           <div style='max-width:min-content;height:20px;background:green'><div style='width:100px'></div></div>\
         </div>",
    );
    check(&img, &[(95, 10, GREEN), (105, 10, WHITE)]);
}

/// `fit-content` in a minimum is the max-content size, at most the available size but at least the
/// min-content size, as for block items: a stretched grid item with `fr` columns stays as wide as its
/// 200px line, not its 800px max-content width (measured under the available width, a grid with `fr`
/// columns gives its max-content width).
#[test]
fn min_width_fit_content_is_at_most_the_available_width() {
    // Each grid item: max-content 400px (eight 50px boxes on a line), min-content 50px.
    let item = format!("<div style='font-size:0'>{}</div>", "<span style='display:inline-block;width:50px;height:20px'></span>".repeat(8));
    let img = render(&format!(
        "<div style='display:flex;flex-direction:column;width:200px'>\
           <div style='display:grid;grid-template-columns:1fr 1fr;min-width:fit-content;background:green'>{item}{item}</div>\
         </div>"
    ));
    check(&img, &[(195, 10, GREEN), (205, 10, WHITE)]);
}
