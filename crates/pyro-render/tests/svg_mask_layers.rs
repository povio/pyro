//! CSS `mask-image` layers on SVG elements (blitz-dom `svg_css::css_mask_rewrite`) that name a
//! `<mask>` by `url(#id)`: one of another `<svg>` of the page masks, one in a shadow tree the
//! element is not in does not (ids are scoped to their tree), and nor does a local `<mask>` of
//! that id for the fragment of another document that failed to load.
use std::sync::Arc;

use pyro_render::*;

/// Every fetch is a 404.
struct Missing;
impl Fetcher for Missing {
    fn fetch(&self, _url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        std::thread::spawn(move || done(Err("404".into())));
    }
}

fn render(body: &str) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(Missing);
    let snapshot = Snapshot {
        html: format!("<!doctype html><body style='margin:0;background:#fff;width:400px;height:300px'>{body}"),
        base_url: "http://test/page.html".into(),
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
const PURPLE: [u8; 3] = [128, 0, 128];

/// A white circle in a `<mask id=m>` (in a shadow tree, or the document's), and a purple 100 px
/// square masked by `layers`: the circle (centred at 50, 50, radius 25) shows where the `<mask>`
/// masks, the 20 px corner where the second layer (a gradient) does.
fn page(mask_in_shadow: bool, layers: &str) -> String {
    let mask = "<svg width=0 height=0 style='position:absolute'><mask id=m x=0 y=0 width=1 height=1>\
                <circle cx=50 cy=50 r=25 fill=#fff /></mask></svg>";
    let (light, shadow) = if mask_in_shadow { ("", mask) } else { (mask, "") };
    format!(
        "{light}<div id=host><template shadowrootmode=open>{shadow}<slot></slot></template></div>\
         <svg width=100 height=100 style='display:block'>\
         <rect width=100 height=100 fill=purple style='mask-image: {layers}; mask-repeat: no-repeat; mask-size: auto, 20px 20px' />\
         </svg>"
    )
}

/// The `<mask>` of another `<svg>` of the page is the first layer.
#[test]
fn a_mask_of_another_svg_masks() {
    let img = render(&page(false, "url(#m), linear-gradient(#000, #000)"));
    assert_eq!(pixel(&img, 50, 50), PURPLE, "the circle");
    assert_eq!(pixel(&img, 5, 5), PURPLE, "the gradient's corner");
    assert_eq!(pixel(&img, 90, 90), WHITE, "outside both");
}

/// A `<mask>` only in a shadow tree is not found from the document's tree: that layer draws
/// nothing (the gradient's layer still masks).
#[test]
fn a_mask_in_a_shadow_tree_is_not_found_from_outside() {
    let img = render(&page(true, "url(#m), linear-gradient(#000, #000)"));
    assert_eq!(pixel(&img, 50, 50), WHITE, "no circle");
    assert_eq!(pixel(&img, 5, 5), PURPLE, "the gradient's corner");
}

/// `missing.svg#m` names a `<mask>` of another document, which failed to load: the page's own
/// `<mask id=m>` does not stand in for it.
#[test]
fn a_failed_external_mask_is_not_a_local_one() {
    let img = render(&page(false, "url(missing.svg#m), linear-gradient(#000, #000)"));
    assert_eq!(pixel(&img, 50, 50), WHITE, "no circle");
    assert_eq!(pixel(&img, 5, 5), PURPLE, "the gradient's corner");
}

