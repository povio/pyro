//! An inline `<svg>` clips its content at its viewport, as Chromium's UA sheet makes it
//! (`svg:not(:root) { overflow: hidden }`, the overflow clip of a replaced element at its content
//! box): before, content past the box painted (WPT CSS2 `absolute-replaced-width-002` drew a
//! 200x100 rect under a 50 px high `<svg:svg>`). `overflow: visible`, in CSS or as the
//! presentation attribute, keeps the content. In layout that `overflow` counts as on any other
//! box, as in Chromium: a flex item's automatic minimum size is 0 with it, and a block-level
//! `<svg>` stays clear of floats. An SVG image is cut at its box whatever its root's `overflow`.
use std::sync::Arc;

use pyro_render::*;

/// Serves `data:image/svg+xml,` URLs (percent-encoded SVG); anything else fails.
struct DataUrls;
impl Fetcher for DataUrls {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        std::thread::spawn(move || match url.strip_prefix("data:image/svg+xml,") {
            Some(svg) => done(Ok(svg
                .replace("%3C", "<")
                .replace("%3E", ">")
                .replace("%23", "#")
                .replace("%25", "%")
                .into_bytes())),
            None => done(Err("offline".into())),
        });
    }
}

fn render(body: &str) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(DataUrls);
    let snapshot = Snapshot {
        html: format!("<!doctype html><style>body{{margin:0;background:white}} svg,img{{display:block}}</style>{body}"),
        base_url: "http://test/".into(),
        width: 300,
        height: 200,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

fn pixel(img: &RgbaImage, x: u32, y: u32) -> [u8; 3] {
    let i = (y as usize * img.width as usize + x as usize) * 4;
    [img.data[i], img.data[i + 1], img.data[i + 2]]
}

const BLUE: [u8; 3] = [0, 0, 255];
const GREEN: [u8; 3] = [0, 128, 0];
const WHITE: [u8; 3] = [255; 3];

/// Content past the `<svg>`'s box is cut off at the box (WPT CSS2 `*-replaced-*` with `svg:svg`).
#[test]
fn content_past_the_box_is_cut_off() {
    let img = render("<svg width='100' height='50'><rect width='200' height='100' fill='blue'/></svg>");
    assert_eq!(pixel(&img, 50, 25), BLUE, "in the box");
    assert_eq!(pixel(&img, 50, 75), WHITE, "below it");
    assert_eq!(pixel(&img, 150, 25), WHITE, "right of it");
}

/// The cut is the content box (`overflow-clip-margin: content-box`): the padding shows the
/// background, not the content.
#[test]
fn the_cut_is_at_the_content_box() {
    let img = render(
        "<svg width='100' height='50' style='padding:10px; background:green'>
           <rect x='-20' y='-20' width='200' height='100' fill='blue'/></svg>",
    );
    assert_eq!(pixel(&img, 15, 15), BLUE, "in the content box");
    assert_eq!(pixel(&img, 5, 5), GREEN, "the padding");
    assert_eq!(pixel(&img, 115, 30), GREEN, "the right padding");
}

/// `overflow: visible`, the property or the presentation attribute, keeps what reaches past the box.
#[test]
fn overflow_visible_keeps_the_content() {
    let img = render(
        "<svg width='100' height='50' style='overflow:visible'><rect width='200' height='100' fill='blue'/></svg>
         <svg width='100' height='50' overflow='visible'><rect width='200' height='100' fill='green'/></svg>",
    );
    assert_eq!(pixel(&img, 150, 25), BLUE, "the style's");
    assert_eq!(pixel(&img, 150, 75), GREEN, "the attribute's (drawn over the first's overflow)");
    assert_eq!(pixel(&img, 150, 125), GREEN, "below its box");
}

/// A rounded `<svg>` cuts its content at the rounded box, as Chromium cuts replaced content at the
/// content edge's curve; its corners show the page.
#[test]
fn a_rounded_svg_cuts_at_its_corners() {
    let img = render(
        "<svg width='100' height='100' style='border-radius:50%'><rect width='100' height='100' fill='blue'/></svg>",
    );
    assert_eq!(pixel(&img, 50, 50), BLUE);
    assert_eq!(pixel(&img, 3, 3), WHITE, "the corner");
}

/// An `<svg>` whose `overflow` is not `visible` (the UA sheet's `hidden`) has an automatic minimum
/// size of 0 as a flex item, as any replaced element with such an `overflow` has in Chromium: a
/// narrow row shrinks it. (Chrome 157: two 40px SVGs in a 50px row are 25px each.)
#[test]
fn an_svg_flex_item_shrinks_in_a_narrow_row() {
    let img = render(
        "<div style='display:flex; width:50px'>
           <svg width='40' height='20'><rect width='40' height='20' fill='blue'/></svg>
           <svg width='40' height='20'><rect width='40' height='20' fill='green'/></svg>
         </div>",
    );
    assert_eq!(pixel(&img, 22, 10), BLUE, "the first is 25px wide");
    assert_eq!(pixel(&img, 27, 10), GREEN, "the second starts at 25px");
    assert_eq!(pixel(&img, 47, 10), GREEN, "and is 25px wide");
    assert_eq!(pixel(&img, 53, 10), WHITE, "the row ends at 50px");
}

/// With `overflow: visible` the `<svg>` keeps its content size as its automatic minimum size, and
/// the other item takes the rest (Chrome 157: 40px and 10px).
#[test]
fn an_overflow_visible_svg_flex_item_keeps_its_size() {
    let img = render(
        "<div style='display:flex; width:50px'>
           <svg width='40' height='20' style='overflow:visible'><rect width='40' height='20' fill='blue'/></svg>
           <svg width='40' height='20'><rect width='40' height='20' fill='green'/></svg>
         </div>",
    );
    assert_eq!(pixel(&img, 38, 10), BLUE, "the first keeps 40px");
    assert_eq!(pixel(&img, 45, 10), GREEN, "the second has the 10px left");
    assert_eq!(pixel(&img, 53, 10), WHITE, "and is cut there");
}

/// The same for other replaced elements: a `<canvas>` with `overflow: hidden` shrinks, one without
/// keeps its size (Chrome 157: 10px and 40px).
#[test]
fn a_replaced_flex_item_with_hidden_overflow_shrinks() {
    let img = render(
        "<div style='display:flex; width:50px; align-items:flex-start'>
           <canvas width='40' height='20' style='overflow:hidden; background:blue'></canvas>
           <canvas width='40' height='20' style='background:green'></canvas>
         </div>",
    );
    assert_eq!(pixel(&img, 5, 2), BLUE, "the first is 10px wide");
    assert_eq!(pixel(&img, 12, 2), GREEN, "the second starts at 10px");
    assert_eq!(pixel(&img, 47, 2), GREEN, "and keeps 40px");
}

/// In a grid the SVGs' `width` is their minimum contribution, whatever their `overflow` (Chrome
/// 157: two 40px SVGs in a 50px grid of two `auto` columns stay 40px, at 0 and 40).
#[test]
fn svg_grid_items_with_a_width_keep_their_columns() {
    let img = render(
        "<div style='display:grid; width:50px; grid-template-columns:auto auto'>
           <svg width='40' height='20'><rect width='40' height='20' fill='blue'/></svg>
           <svg width='40' height='20'><rect width='40' height='20' fill='green'/></svg>
         </div>",
    );
    assert_eq!(pixel(&img, 38, 10), BLUE, "the first keeps 40px");
    assert_eq!(pixel(&img, 42, 10), GREEN, "the second starts at 40px");
    assert_eq!(pixel(&img, 78, 10), GREEN, "and keeps 40px");
}

/// A block-level replaced element stays clear of a float, with or without a clipping `overflow`
/// (Chrome 157: each sits at x=100 beside a 100px left float).
#[test]
fn block_level_replaced_elements_avoid_floats() {
    for replaced in [
        "<canvas width='40' height='20' style='display:block; background:blue'></canvas>",
        "<canvas width='40' height='20' style='display:block; overflow:hidden; background:blue'></canvas>",
        "<svg width='40' height='20'><rect width='40' height='20' fill='blue'/></svg>",
        "<svg width='40' height='20' style='overflow:visible'><rect width='40' height='20' fill='blue'/></svg>",
    ] {
        let img = render(&format!(
            "<div style='width:300px'><div style='float:left; width:100px; height:50px'></div>{replaced}</div>"
        ));
        assert_eq!(pixel(&img, 10, 10), WHITE, "not under the float: {replaced}");
        assert_eq!(pixel(&img, 105, 10), BLUE, "beside the float: {replaced}");
        assert_eq!(pixel(&img, 135, 10), BLUE, "40px wide: {replaced}");
    }
}

/// An SVG image is cut at its box, whatever its root element's `overflow`.
#[test]
fn an_svg_image_is_cut_at_its_box() {
    let img = render(
        "<img width='100' height='50' src=\"data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='100' height='50' overflow='visible'%3E%3Crect width='200' height='100' fill='blue'/%3E%3C/svg%3E\">",
    );
    assert_eq!(pixel(&img, 50, 25), BLUE);
    assert_eq!(pixel(&img, 50, 75), WHITE, "below the image");
    assert_eq!(pixel(&img, 150, 25), WHITE, "right of it");
}
