//! An inline `<svg>` clips its content at its viewport, as Chromium's UA sheet makes it
//! (`svg:not(:root) { overflow: hidden }`, the overflow clip of a replaced element at its content
//! box): before, content past the box painted (WPT CSS2 `absolute-replaced-width-002` drew a
//! 200x100 rect under a 50 px high `<svg:svg>`). `overflow: visible`, in CSS or as the
//! presentation attribute, keeps the content. The `overflow` stays paint-only: the `<svg>` is no
//! scroll container in layout (a flex item's automatic minimum size stays its content size). An
//! SVG image is cut at its box whatever its root's `overflow`.
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

/// The `overflow` of an `<svg>` is paint-only: as a flex item it keeps its content size as its
/// automatic minimum size, and a narrow row does not shrink it (Chromium: a replaced element is no
/// scroll container).
#[test]
fn an_svg_flex_item_does_not_shrink_below_its_size() {
    let img = render(
        "<div style='display:flex; width:50px'>
           <svg width='40' height='20'><rect width='40' height='20' fill='blue'/></svg>
           <svg width='40' height='20'><rect width='40' height='20' fill='green'/></svg>
         </div>",
    );
    assert_eq!(pixel(&img, 38, 10), BLUE, "the first keeps 40px");
    assert_eq!(pixel(&img, 75, 10), GREEN, "the second starts at 40px");
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
