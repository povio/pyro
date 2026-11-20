//! An `<object>` whose `data` is an image (its `type` says so, or the URL's extension) shows the
//! image as replaced content, sized like `<img>` from the image (an SVG's natural size or ratio):
//! before, objects loaded nothing and always showed their fallback content (WPT CSS2
//! `replaced-intrinsic-003`..`005`, `margin-border-padding-003`). An object of another type keeps
//! its fallback, and so does one whose image fails to load.
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

/// `<i>` is a red 20 px square, the fallback content and the box after an object.
fn render(body: &str) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(DataUrls);
    let snapshot = Snapshot {
        html: format!(
            "<!doctype html><style>body{{margin:0;background:white}} i{{display:inline-block;width:20px;height:20px;background:red}}</style>{body}"
        ),
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

const SQUARE: &str = "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='80' height='40'%3E%3Crect width='80' height='40' fill='blue'/%3E%3C/svg%3E";
const RATIO: &str = "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 1 2'%3E%3Crect width='1' height='2' fill='blue'/%3E%3C/svg%3E";
const BLUE: [u8; 3] = [0, 0, 255];
const WHITE: [u8; 3] = [255; 3];
const RED: [u8; 3] = [255, 0, 0];

/// The image is the object's content, at its natural size; the fallback is not rendered.
#[test]
fn an_object_shows_its_svg_image() {
    let img = render(&format!(
        "<object data=\"{SQUARE}\" type='image/svg+xml' style='display:block'><i></i><i></i><i></i><i></i><i></i><i></i></object>"
    ));
    assert_eq!(pixel(&img, 75, 35), BLUE, "in the 80x40 image");
    assert_eq!(pixel(&img, 90, 10), WHITE, "right of it: no fallback");
    assert_eq!(pixel(&img, 40, 45), WHITE, "below it");
}

/// An image with only a ratio fills the object's width, its height from the ratio (as `<img>`).
#[test]
fn an_object_image_with_a_ratio_takes_its_width() {
    let img =
        render(&format!("<object data=\"{RATIO}\" type='image/svg+xml' style='display:block; width:50px'></object>"));
    assert_eq!(pixel(&img, 45, 95), BLUE, "50 wide, 100 high");
    assert_eq!(pixel(&img, 45, 105), WHITE);
}

/// A ratio-only image with an auto width fills the width less its padding (a stretch-fit size;
/// WPT CSS2 `replaced-intrinsic-003`): its content box was the whole width, and the box overflowed
/// it by its padding, its height twice that.
#[test]
fn a_ratio_only_image_fills_the_width_less_its_padding() {
    let img = render(&format!(
        "<div style='width:100px'><img src=\"{RATIO}\" style='display:block; padding-right:50px; background:red'></div>"
    ));
    assert_eq!(pixel(&img, 45, 95), BLUE, "50 wide, 100 high");
    assert_eq!(pixel(&img, 45, 105), WHITE, "not 200 high");
    assert_eq!(pixel(&img, 75, 50), RED, "the padding");
    assert_eq!(pixel(&img, 120, 50), WHITE, "the box ends at 100");
}

/// An inline object is an atomic inline box: the box after it follows the image on its line.
#[test]
fn an_inline_object_is_an_atomic_box() {
    let img = render(&format!("<object data=\"{SQUARE}\" type='image/svg+xml'><i></i></object><i></i>"));
    assert_eq!(pixel(&img, 40, 30), BLUE);
    assert_eq!(pixel(&img, 90, 35), RED, "the box after it starts at 80, on the image's baseline");
    assert_eq!(pixel(&img, 10, 45), WHITE, "no fallback below");
}

/// A block in the fallback of an inline image object does not split the object: its children are
/// not rendered, so it stays one atomic inline box (Chromium draws the image only, the next box
/// beside it).
#[test]
fn a_block_in_the_fallback_does_not_split_an_inline_object() {
    let img = render(&format!(
        "<div><object data=\"{SQUARE}\" type='image/svg+xml'><i></i><p><i></i></p><i></i></object><i></i></div>"
    ));
    assert_eq!(pixel(&img, 40, 30), BLUE, "the image starts the line");
    assert_eq!(pixel(&img, 10, 10), BLUE, "no fallback before it");
    assert_eq!(pixel(&img, 90, 35), RED, "the box after it is on its line, at 80");
    assert_eq!(pixel(&img, 110, 35), WHITE, "and nothing else");
    for y in [50, 70, 90, 110] {
        assert_eq!(pixel(&img, 10, y), WHITE, "no fallback below, at y {y}");
    }
}

/// An SVG with no natural size or ratio is shown at the default object size, 300x150, not the
/// object's fallback, as the installed Chrome draws this very page (blitz-dom `object_shows_image`;
/// WPT `replaced-element-021`).
#[test]
fn an_object_svg_without_a_size_shows_at_the_default_size() {
    let img = render(
        "<object data=\"data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg'%3E%3Crect width='300' height='200' fill='blue'/%3E%3C/svg%3E\" type='image/svg+xml' style='display:block'><i></i></object>",
    );
    assert_eq!(pixel(&img, 10, 10), BLUE, "the SVG, no fallback");
    assert_eq!(pixel(&img, 290, 140), BLUE, "300 by 150 (the viewport is 300 wide)");
    assert_eq!(pixel(&img, 10, 160), WHITE, "150 tall: the SVG's 200px rect is cut at its viewport");
}

/// An object that is no image (`text/html`), or whose image does not load, shows its fallback.
#[test]
fn other_objects_show_their_fallback() {
    let img = render(
        "<object data='page.html' type='text/html' style='display:block'><i></i></object>
         <object data='missing.svg' style='display:block'><i></i></object>",
    );
    assert_eq!(pixel(&img, 10, 10), RED, "the html object's fallback");
    assert_eq!(pixel(&img, 10, 30), RED, "the failed image's fallback");
}
