//! An SVG image's natural width and height are its root's absolute `width` and `height`; one
//! without them (and without a `viewBox`) has no natural size and the default object size, 300x150,
//! stands in, as in browsers. usvg's resolved size of such an SVG (100x100) stood in.
use std::sync::Arc;

use pyro_render::*;

/// Serves `data:image/svg+xml,` URLs (percent-encoded SVG); anything else fails.
struct DataUrls;
impl Fetcher for DataUrls {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        std::thread::spawn(move || match url.strip_prefix("data:image/svg+xml,") {
            Some(svg) => done(Ok(svg.replace("%3C", "<").replace("%3E", ">").replace("%23", "#").replace("%25", "%").into_bytes())),
            None => done(Err("offline".into())),
        });
    }
}

fn render(body: &str) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(DataUrls);
    let snapshot = Snapshot {
        html: format!("<!doctype html><style>body{{margin:0;background:white}} img{{display:block}}</style>{body}"),
        base_url: "http://test/".into(),
        width: 400,
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

#[test]
fn an_svg_without_a_size_is_300_by_150() {
    // The SVG's rect fills its viewport, which is the image's box.
    let img = render(
        "<img src=\"data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg'%3E%3Crect width='100%25' height='100%25' fill='%2300f'/%3E%3C/svg%3E\">",
    );
    assert_eq!(pixel(&img, 295, 145), [0, 0, 255], "inside 300x150");
    assert_eq!(pixel(&img, 305, 5), [255, 255, 255], "right of 300");
    assert_eq!(pixel(&img, 5, 155), [255, 255, 255], "below 150");
}

/// pyro (lane G): a declared absolute `width` is the natural width, whatever the content: usvg's
/// resolved width of an SVG without a height took its content's bounds (46px here).
#[test]
fn an_svg_with_only_a_width_is_that_wide() {
    let img = render(
        "<img style='background:lime' src=\"data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='20px'%3E%3Ccircle cx='50%25' cy='50%25' r='50%25' fill='%2300f'/%3E%3C/svg%3E\">",
    );
    assert_ne!(pixel(&img, 10, 140), [255, 255, 255], "inside 20x150");
    assert_eq!(pixel(&img, 25, 75), [255, 255, 255], "right of 20");
}

/// pyro (lane G): a zero `width` is a natural width of zero, not an invalid image: the natural height
/// still sizes the box.
#[test]
fn an_svg_with_a_zero_width_keeps_its_natural_height() {
    let img = render(
        "<img style='width:50px;background:lime' src=\"data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='0' height='20'%3E%3C/svg%3E\">",
    );
    assert_eq!(pixel(&img, 25, 10), [0, 255, 0], "inside 50x20");
    assert_eq!(pixel(&img, 25, 25), [255, 255, 255], "below 20");
}
