//! `backdrop-filter`: what the element's backdrop root painted before it, filtered and shown in
//! the element's border box, under its content, as Chromium paints it. And `url()` filters naming
//! an SVG `<filter>` of colour primitives. Neither was painted before.
use std::sync::Arc;

use pyro_render::*;

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}

fn render(body: &str, scale: f64) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let snapshot = Snapshot {
        html: format!("<!doctype html><body style='margin:0;background:#fff;width:300px;height:200px'>{body}"),
        base_url: "http://test/".into(),
        width: 300,
        height: 200,
        device_scale_factor: scale,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

/// The pixel at CSS px (x, y).
fn pixel(img: &RgbaImage, scale: f64, x: u32, y: u32) -> [u8; 3] {
    let (x, y) = ((x as f64 * scale) as usize, (y as f64 * scale) as usize);
    let i = (y * img.width as usize + x) * 4;
    [img.data[i], img.data[i + 1], img.data[i + 2]]
}

const SQUARE: &str = "<div style='position:absolute;left:50px;top:50px;width:50px;height:50px;background:#000'></div>";

/// WPT backdrop-filters-invert: the backdrop is inverted in the element's box (the white page and
/// the black square), the element's own content drawn over it, and nothing outside the box.
#[test]
fn a_backdrop_filter_filters_what_lies_behind_the_box() {
    for scale in [1.0, 2.0] {
        let img = render(
            &format!(
                "{SQUARE}<div style='position:absolute;left:25px;top:25px;width:100px;height:100px;backdrop-filter:invert(1)'>
                 <div style='margin:10px;width:10px;height:10px;background:#0f0'></div></div>"
            ),
            scale,
        );
        assert_eq!(pixel(&img, scale, 75, 75), [255; 3], "the black square inverted, scale {scale}");
        assert_eq!(pixel(&img, scale, 110, 110), [0; 3], "the white page inverted, scale {scale}");
        assert_eq!(pixel(&img, scale, 40, 40), [0, 255, 0], "the content, over it, scale {scale}");
        assert_eq!(pixel(&img, scale, 150, 150), [255; 3], "outside the box, scale {scale}");
    }
}

/// A blur reads the backdrop mirrored at the box's edges (Chromium's edge mode): no colour from
/// outside the box comes in (WPT backdrop-filter-edge-clipping), no transparency darkens the edge.
#[test]
fn a_backdrop_blur_reads_no_colour_past_the_box() {
    for scale in [1.0, 2.0] {
        let img = render(
            "<div style='position:absolute;left:0;top:0;width:300px;height:50px;background:#0a0'></div>
             <div style='position:absolute;left:50px;top:50px;width:100px;height:100px;backdrop-filter:blur(10px)'></div>",
            scale,
        );
        assert_eq!(pixel(&img, scale, 100, 51), [255; 3], "the top edge, under the green, scale {scale}");
        assert_eq!(pixel(&img, scale, 51, 100), [255; 3], "the left edge, scale {scale}");
        assert_eq!(pixel(&img, scale, 100, 40), [0, 170, 0], "the green, scale {scale}");
    }
}

/// An element with opacity below 1 is a backdrop root: its child's backdrop is what the parent
/// painted (nothing under the child), not the page (WPT backdrop-filter-backdrop-root-opacity).
/// `will-change: opacity` makes one too.
#[test]
fn a_backdrop_stops_at_its_backdrop_root() {
    for root in ["opacity:.99", "will-change:opacity", "filter:invert(0)", "mix-blend-mode:darken"] {
        let img = render(
            &format!(
                "<div style='background:#0ff;width:200px;height:100px'>
                 <div style='{root};width:50px;height:50px;background:#080'>
                 <div style='position:relative;left:50px;width:50px;height:50px;backdrop-filter:invert(1)'></div></div></div>"
            ),
            1.0,
        );
        // The cyan page under the child, not inverted to red.
        assert_eq!(pixel(&img, 1.0, 75, 25), [0, 255, 255], "{root}");
    }
    let img = render(
        "<div style='background:#0ff;width:200px;height:100px'><div style='width:50px;height:50px;background:#080'>
         <div style='position:relative;left:50px;width:50px;height:50px;backdrop-filter:invert(1)'></div></div></div>",
        1.0,
    );
    assert_eq!(pixel(&img, 1.0, 75, 25), [255, 0, 0], "without a root, the page is inverted");
}

/// `url()` names an SVG `<filter>`: `feComponentTransfer` tables in sRGB (WPT backdrop-filter-svg),
/// and `feColorMatrix` in a `display: none` SVG, which Chromium evaluates in sRGB (WPT
/// backdrop-filter-reference-filter: it matches `invert(1)`).
#[test]
fn a_url_filter_names_an_svg_filter_of_colour_primitives() {
    let svg = "<svg style='position:absolute;width:0;height:0'><filter id='table' color-interpolation-filters='sRGB'>
        <feComponentTransfer><feFuncR type='table' tableValues='1 0'/><feFuncG type='table' tableValues='1 0'/>
        <feFuncB type='table' tableValues='1 0'/></feComponentTransfer></filter></svg>
        <svg style='display:none'><filter id='matrix'><feColorMatrix type='matrix'
        values='-1 0 0 0 1  0 -1 0 0 1  0 0 -1 0 1  0 0 0 1 0'/></filter></svg>";
    let fuchsia = "position:absolute;left:50px;top:50px;width:50px;height:50px;background:#f0f";
    for filter in ["backdrop-filter:url(#table)", "backdrop-filter:url(#matrix)"] {
        let img = render(
            &format!(
                "{svg}<div style='{fuchsia}'></div><div style='position:absolute;left:50px;top:50px;width:50px;height:50px;{filter}'></div>"
            ),
            1.0,
        );
        assert_eq!(pixel(&img, 1.0, 75, 75), [0, 255, 0], "{filter}");
    }
    let img = render(&format!("{svg}<div style='{fuchsia};filter:url(#matrix)'></div>"), 1.0);
    assert_eq!(pixel(&img, 1.0, 75, 75), [0, 255, 0], "filter:url(#matrix)");
    // linearRGB, the default elsewhere: the green of `#080` inverted there is not 255 - 136.
    let img = render(
        "<svg style='position:absolute;width:0;height:0'><filter id='linear'><feComponentTransfer>
         <feFuncG type='table' tableValues='1 0'/></feComponentTransfer></filter></svg>
         <div style='position:absolute;left:50px;top:50px;width:50px;height:50px;background:#080;filter:url(#linear)'></div>",
        1.0,
    );
    let g = pixel(&img, 1.0, 75, 75)[1];
    assert!(g > 200, "linearRGB: {g}");
}

/// An inline element's backdrop filter filters the backdrop in its fragments' border boxes, under
/// its own content (WPT `backdrop-filter-inline-positioning`). It was not painted.
#[test]
fn an_inline_elements_backdrop_filter_is_painted() {
    for scale in [1.0, 2.0] {
        let img = render(
            "<div style='background:#ffa500;height:200px'><span style='backdrop-filter:invert(1);padding:30px;color:#0f0;font:20px/20px monospace'>X</span></div>",
            scale,
        );
        // The span's border box: x 0..72, y 0 - 30 .. 20 + 30 (about), the page orange elsewhere.
        assert_eq!(pixel(&img, scale, 5, 10), [0, 90, 255], "the orange backdrop inverted, scale {scale}");
        assert_eq!(pixel(&img, scale, 200, 10), [255, 165, 0], "beside it, scale {scale}");
        assert_eq!(pixel(&img, scale, 5, 150), [255, 165, 0], "below it, scale {scale}");
    }
}
