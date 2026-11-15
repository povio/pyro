//! The CSS `filter()` image function (Filter Effects 1, `<filter-function>` applied to an
//! `<image>`): the image painted with the filters applied, as a background layer or a border
//! image (WPT css/filter-effects/filter-function/filter-function-002, -003, -*-gradient).
use std::sync::Arc;

use pyro_render::*;

/// Serves `data:image/svg+xml,` URLs (percent-encoded SVG), on another thread as a page's loads
/// arrive; anything else fails.
struct DataUrls;
impl Fetcher for DataUrls {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        std::thread::spawn(move || match url.strip_prefix("data:image/svg+xml,") {
            Some(svg) => done(Ok(percent_decode(svg))),
            None => done(Err("offline".into())),
        });
    }
}

fn percent_decode(s: &str) -> Vec<u8> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

/// A 20x20 square of `fill` (percent-encoded, `%23` for `#`).
fn square(fill: &str) -> String {
    format!(
        "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='20' height='20'%3E\
         %3Crect width='20' height='20' fill='{fill}'/%3E%3C/svg%3E"
    )
}

fn render(body: &str, width: u32, height: u32) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(DataUrls);
    let snapshot = Snapshot {
        html: format!("<!doctype html><body style='margin:0;background:#fff'>{body}"),
        base_url: "http://test/".into(),
        width,
        height,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

fn pixel(img: &RgbaImage, x: usize, y: usize) -> [u8; 3] {
    let i = (y * img.width as usize + x) * 4;
    [img.data[i], img.data[i + 1], img.data[i + 2]]
}

#[test]
fn a_filtered_gradient_background_is_painted_filtered() {
    let img = render(
        "<div style='width:40px;height:40px;background-image:filter(linear-gradient(red, red), invert(1))'></div>\
         <div style='width:40px;height:40px;background:filter(linear-gradient(red, red), invert(1) grayscale(1)) no-repeat, blue'></div>",
        40,
        80,
    );
    assert_eq!(pixel(&img, 20, 20), [0, 255, 255]);
    // Filters apply in order: cyan, then its luminance.
    let [r, g, b] = pixel(&img, 20, 60);
    assert!(r == g && g == b && r > 150, "{:?}", [r, g, b]);
}

#[test]
fn a_drop_shadow_of_a_filtered_image_is_painted_in_the_box() {
    // The green left half's shadow fills the right half; the layer's background clip keeps it
    // inside the box.
    let img = render(
        "<div style='width:40px;height:20px;background:filter(linear-gradient(to right, green 50%, transparent 50%), \
         drop-shadow(20px 0 0 green)) no-repeat, red'></div>",
        80,
        20,
    );
    assert_eq!(pixel(&img, 10, 10), [0, 128, 0]);
    assert_eq!(pixel(&img, 30, 10), [0, 128, 0]);
    assert_eq!(pixel(&img, 50, 10), [255, 255, 255]);
}

#[test]
fn a_filtered_border_image_is_painted_filtered() {
    let img = render(
        "<div style='width:0;height:0;border:20px solid black;border-image:filter(linear-gradient(red, red), invert(1)) 1'></div>",
        40,
        40,
    );
    assert_eq!(pixel(&img, 5, 5), [0, 255, 255]);
    assert_eq!(pixel(&img, 35, 35), [0, 255, 255]);
}

#[test]
fn an_invalid_filter_image_is_dropped_at_parse_time() {
    // No filter function: the declaration is invalid, so the earlier one stays.
    let img = render(
        "<style>div{background-image:linear-gradient(lime, lime)}div{background-image:filter(linear-gradient(red, red),)}</style>\
         <div style='width:20px;height:20px'></div>",
        20,
        20,
    );
    assert_eq!(pixel(&img, 10, 10), [0, 255, 0]);
}

#[test]
fn mask_images_take_no_filter_image() {
    // As in Chromium and Gecko: the declaration is dropped, and the earlier mask (all of it) stays.
    let img = render(
        "<style>div{mask-image:linear-gradient(black, black)}\
         div{mask-image:filter(linear-gradient(transparent, transparent), opacity(1))}</style>\
         <div style='width:20px;height:20px;background:lime'></div>",
        20,
        20,
    );
    assert_eq!(pixel(&img, 10, 10), [0, 255, 0]);
}

#[test]
fn list_style_images_take_no_filter_image() {
    // As in Chromium and Gecko: the declaration (also as `list-style`) is dropped and the earlier
    // marker stays. A marker draws its image as it is, so a `filter()` marker drew the red square
    // unfiltered.
    let (green, red) = (square("%2300a000"), square("%23ff0000"));
    let list = |css: &str| {
        render(
            &format!(
                "<style>li{{list-style-image:url(\"{green}\")}} {css}</style>\
                 <ul style='margin:0;padding-left:40px;font:20px/20px sans-serif'><li>x</li></ul>"
            ),
            80,
            20,
        )
    };
    let reference = list("");
    let marker = |img: &RgbaImage| (0..40).find(|&x| pixel(img, x, 10) != [255, 255, 255]).map(|x| pixel(img, x, 10));
    assert_eq!(marker(&reference), Some([0, 160, 0]));
    for css in [
        format!("li{{list-style-image:filter(url(\"{red}\"), grayscale(1))}}"),
        format!("li{{list-style:filter(url(\"{red}\"), grayscale(1)) inside}}"),
    ] {
        let img = list(&css);
        assert_eq!(marker(&img), marker(&reference), "{css}");
        assert_eq!(img.data, reference.data, "{css}");
    }
}
