//! The CSS colour filter functions (`grayscale()`, `sepia()`, `saturate()`, `hue-rotate()`,
//! `invert()`, `opacity()`, `brightness()`, `contrast()`) are applied, as Chromium applies them: the
//! filter effects spec's `feColorMatrix` matrices on sRGB values (not linearRGB). The colours below
//! are the ones WPT's references give, which are Chromium's. They were not painted at all before:
//! the boxes kept their colours.
use std::sync::Arc;

use pyro_render::*;

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}

fn render(body: &str, width: u32, height: u32, scale: f64) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let snapshot = Snapshot {
        html: format!("<!doctype html><body style='margin:0;background:#fff;width:{width}px;height:{height}px'>{body}"),
        base_url: "http://test/".into(),
        width,
        height,
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

const BOX: &str = "position:absolute;left:50px;top:50px;width:100px;height:100px;";

/// Each function on a box of one colour gives Chromium's colour.
#[test]
fn each_colour_function_gives_chromiums_colour() {
    let cases: [(&str, &str, [u8; 3]); 11] = [
        // WPT filter-grayscale-001: blue is #121212.
        ("blue", "grayscale(100%)", [0x12; 3]),
        // filter-contrast-002: red is #808080.
        ("red", "contrast(0%)", [0x80; 3]),
        // filter-hue_rotate-002: red is #007100.
        ("red", "hue-rotate(120deg)", [0, 0x71, 0]),
        // filter-saturate-001: yellow is #EDEDED.
        ("yellow", "saturate(0%)", [0xED; 3]),
        // composited-filters-under-opacity: yellow is blue.
        ("yellow", "invert(100%)", [0, 0, 255]),
        // feColorMatrix's sepia rows on white: 0.393 + 0.769 + 0.189 > 1, 0.349 + 0.686 + 0.168 > 1,
        // 0.272 + 0.534 + 0.131 = 0.937.
        ("white", "sepia(1)", [255, 255, 239]),
        ("rgb(100,150,200)", "brightness(50%)", [50, 75, 100]),
        ("rgb(100,150,200)", "brightness(3)", [255, 255, 255]),
        ("rgb(100,150,200)", "invert(25%)", [114, 139, 164]),
        // A chain, in order, each result clamped: red twice as bright stays red, then inverted.
        ("red", "brightness(2) invert(1)", [0, 255, 255]),
        // A function at its neutral amount changes nothing.
        ("rgb(100,150,200)", "grayscale(0) saturate(1)", [100, 150, 200]),
    ];
    for scale in [1.0, 2.0] {
        // Content in two colours is filtered as pixels, content in one by its colour: the same.
        let img = render(
            &format!("<div style='{BOX}background:blue;border:10px solid red;filter:grayscale(1)'></div>"),
            200,
            200,
            scale,
        );
        assert_eq!(pixel(&img, scale, 100, 100), [0x12; 3], "the background, scale {scale}");
        assert_eq!(pixel(&img, scale, 55, 100), [54; 3], "the border, scale {scale}");
        for (color, filter, want) in cases {
            let img = render(&format!("<div style='{BOX}background:{color};filter:{filter}'></div>"), 200, 200, scale);
            assert_eq!(pixel(&img, scale, 100, 100), want, "{color} through {filter}, scale {scale}");
            assert_eq!(pixel(&img, scale, 20, 20), [255; 3], "outside the box, {filter}, scale {scale}");
        }
    }
}

/// A filter on a parent applies to everything it paints: its children in several colours (not one
/// colour that the filter can map), text, and a child outside the parent's box.
#[test]
fn a_parents_colour_filter_applies_to_its_descendants() {
    for scale in [1.0, 2.0] {
        let img = render(
            &format!(
                "<div style='filter:invert(1)'><div style='{BOX}background:#ff0'></div>
                 <div style='position:absolute;left:150px;top:150px;width:40px;height:40px;background:#0f0'></div></div>"
            ),
            200,
            200,
            scale,
        );
        assert_eq!(pixel(&img, scale, 100, 100), [0, 0, 255], "first child, scale {scale}");
        assert_eq!(pixel(&img, scale, 170, 170), [255, 0, 255], "second child, scale {scale}");
        assert_eq!(pixel(&img, scale, 20, 20), [255; 3], "around them, scale {scale}");
    }
}

/// Boxes with a colour filter under a faded parent are filtered, then faded with the group: WPT's
/// composited-filters-under-opacity, two inverted yellow boxes overlapping under opacity .5, renders
/// as its reference, the same boxes in blue.
#[test]
fn a_colour_filter_under_opacity_is_faded_once() {
    let boxes = |style: &str| {
        let content = format!("width:100px;height:150px;position:absolute;top:0;{style}");
        format!(
            "<div style='opacity:.5'><div style='{content};left:0'></div><div style='{content};left:50px'></div></div>"
        )
    };
    for scale in [1.0, 2.0] {
        let test = render(&boxes("background:yellow;filter:invert(100%)"), 200, 200, scale);
        let reference = render(&boxes("background:blue"), 200, 200, scale);
        assert!(
            test.data == reference.data,
            "scale {scale}: {:?} vs {:?}",
            pixel(&test, scale, 75, 75),
            pixel(&reference, scale, 75, 75)
        );
        assert_ne!(pixel(&test, scale, 75, 75), [255; 3]);
    }
}

/// `opacity()` fades the element as `opacity` does, on its own and after other functions.
#[test]
fn the_opacity_function_fades_the_element() {
    for (filter, want) in
        [("opacity(0)", [255; 3]), ("opacity(.5)", [255, 128, 128]), ("invert(1) opacity(.5)", [128, 255, 255])]
    {
        let img = render(&format!("<div style='{BOX}background:red;filter:{filter}'></div>"), 200, 200, 1.0);
        let got = pixel(&img, 1.0, 100, 100);
        assert!(got.iter().zip(want).all(|(&g, w)| g.abs_diff(w) <= 1), "{filter}: {got:?}, want {want:?}");
    }
}

/// A frame of 4 M device px or more is rasterized on worker threads, where vello_cpu cannot
/// filter: the colours are mapped before it, the same.
#[test]
fn a_colour_filter_is_applied_on_a_frame_rendered_on_threads() {
    let body = format!(
        "<div style='filter:hue-rotate(120deg)'><div style='{BOX}background:red'></div><div style='{BOX}left:160px;background:#ff0'></div></div>"
    );
    let small = render(&body, 400, 300, 2.0);
    let large = render(&body, 1600, 1300, 2.0);
    assert!(1600 * 1300 * 4 >= 4_000_000);
    for x in [100, 210] {
        assert_eq!(pixel(&large, 2.0, x, 100), pixel(&small, 2.0, x, 100), "x {x}");
    }
    assert_eq!(pixel(&large, 2.0, 100, 100), [0, 0x71, 0]);
}
