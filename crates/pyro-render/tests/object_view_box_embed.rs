//! `object-view-box` (CSS Images 5): the view box stands for a replaced element's content where
//! it is sized (the natural size) and where `object-fit` and `object-position` place it (WPT
//! css/css-images/object-view-box-*). And an `<embed>` shows the image its `src` names
//! (css/css-images/object-fit-*-png-*e, object-position-png-*e).
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
        html: format!("<!doctype html><style>body{{margin:0;background:white}} img,embed{{display:block}}</style>{body}"),
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

/// 100x100: the left half blue, the right half green (`#008000`).
const HALVES: &str = "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='100' height='100'%3E%3Crect width='50' height='100' fill='blue'/%3E%3Crect x='50' width='50' height='100' fill='green'/%3E%3C/svg%3E";
const BLUE: [u8; 3] = [0, 0, 255];
const GREEN: [u8; 3] = [0, 128, 0];
const WHITE: [u8; 3] = [255; 3];

#[test]
fn the_view_box_is_the_natural_size() {
    // The right half is the view: the image is 50x100 and green.
    let img = render(&format!("<img src=\"{HALVES}\" style='object-view-box: inset(0 0 0 50px)'>"));
    assert_eq!(pixel(&img, 10, 50), GREEN);
    assert_eq!(pixel(&img, 45, 95), GREEN);
    assert_eq!(pixel(&img, 60, 50), WHITE, "the box is 50px wide");
}

#[test]
fn the_view_box_is_fitted_to_the_content_box() {
    // `xywh(0 0 50px 50px)` (the top of the blue half) fills a 100x100 box at twice the size.
    let img = render(&format!(
        "<img src=\"{HALVES}\" style='width:100px;height:100px;object-view-box: xywh(0 0 50px 50px)'>"
    ));
    assert_eq!(pixel(&img, 10, 10), BLUE);
    assert_eq!(pixel(&img, 90, 90), BLUE);
}

#[test]
fn a_view_box_without_an_area_is_ignored() {
    let img = render(&format!("<img src=\"{HALVES}\" style='object-view-box: inset(0 60px 0 60px)'>"));
    assert_eq!(pixel(&img, 10, 50), BLUE);
    assert_eq!(pixel(&img, 90, 50), GREEN);
}

#[test]
fn an_embed_shows_its_image() {
    let img = render(&format!("<embed src=\"{HALVES}\">"));
    assert_eq!(pixel(&img, 10, 50), BLUE);
    assert_eq!(pixel(&img, 90, 50), GREEN);
    assert_eq!(pixel(&img, 110, 50), WHITE);
}
