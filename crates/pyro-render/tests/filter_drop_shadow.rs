//! `filter: drop-shadow()` and `blur()` are painted, on frames rendered on the calling thread and on
//! vello_cpu's worker threads alike (vello_cpu 0.3 filters neither of them itself there). Chromium
//! takes drop-shadow's third length as the blur's standard deviation, and blurs with three boxes
//! (Skia on the CPU): the profiles below are Chromium 147's.
use std::sync::Arc;

use pyro_render::*;

/// Serves `white.png`, 40 by 40 white pixels.
struct WhitePng;
impl Fetcher for WhitePng {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        let png = encode_png(&RgbaImage { width: 40, height: 40, data: vec![255; 40 * 40 * 4] });
        std::thread::spawn(move || done(if url.ends_with("white.png") { Ok(png) } else { Err("404".into()) }));
    }
}

fn render(body: &str, width: u32, height: u32, scale: f64) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(WhitePng);
    let snapshot = Snapshot {
        html: format!("<!doctype html><body style='margin:0;background:#888;width:{width}px;height:{height}px'>{body}"),
        base_url: "http://test/".into(),
        width,
        height,
        device_scale_factor: scale,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

/// The red channel at CSS px (x, y).
fn red(img: &RgbaImage, scale: f64, x: u32, y: u32) -> u8 {
    let (x, y) = ((x as f64 * scale) as usize, (y as f64 * scale) as usize);
    img.data[(y * img.width as usize + x) * 4]
}

const IMG: &str =
    "<img src='white.png' style='position:absolute;left:40px;top:40px;filter:drop-shadow(0 40px 0 #000)'>";

#[test]
fn a_drop_shadow_is_painted_under_a_box_and_an_image() {
    for scale in [1.0, 2.0] {
        let img = render(
            "<div style='position:absolute;left:40px;top:40px;width:40px;height:40px;background:#fff;filter:drop-shadow(0 40px 0 #000)'></div>",
            200,
            160,
            scale,
        );
        assert_eq!(red(&img, scale, 60, 60), 255, "the box, scale {scale}");
        assert_eq!(red(&img, scale, 60, 100), 0, "its shadow, scale {scale}");
        assert_eq!(red(&img, scale, 60, 130), 0x88, "below the shadow, scale {scale}");
        let img = render(IMG, 200, 160, scale);
        assert_eq!(red(&img, scale, 60, 60), 255, "the image, scale {scale}");
        assert_eq!(red(&img, scale, 60, 100), 0, "its shadow, scale {scale}");
    }
}

/// A blurred drop shadow's edge, a row across it, against Chromium's: within two levels.
#[test]
fn a_blurred_drop_shadow_falls_off_as_chromiums() {
    // `drop-shadow(0 0 20px #000)` of a black box with its right edge at x = 200, on #888: the
    // red channel at x = 201, 204, ... 240 (CSS px), Chromium 147, device scales 1 and 2.
    const CHROMIUM: [(f64, [u8; 14]); 2] = [
        (1.0, [72, 80, 87, 95, 102, 108, 114, 119, 123, 126, 129, 131, 133, 134]),
        (2.0, [71, 79, 87, 95, 102, 108, 114, 119, 123, 126, 129, 131, 133, 134]),
    ];
    for (scale, chromium) in CHROMIUM {
        let img = render(
            "<div style='position:absolute;left:100px;top:50px;width:100px;height:200px;background:#000;filter:drop-shadow(0 0 20px #000)'></div>",
            400,
            300,
            scale,
        );
        let ours: Vec<u8> = (0..14).map(|i| red(&img, scale, 201 + 3 * i, 150)).collect();
        assert!(
            ours.iter().zip(chromium).all(|(&a, b)| a.abs_diff(b) <= 2),
            "scale {scale}: {ours:?} vs Chromium {chromium:?}"
        );
    }
    // `drop-shadow(0 18px 30px rgba(0,0,0,.45))` of a white card (y 40..140): a column below it,
    // at y = 150, 155, ... 220.
    const CARD: [(f64, [u8; 15]); 2] = [
        (1.0, [99, 103, 107, 111, 115, 119, 122, 125, 128, 130, 132, 133, 134, 135, 135]),
        (2.0, [99, 103, 107, 111, 115, 119, 122, 125, 128, 130, 132, 133, 134, 135, 135]),
    ];
    for (scale, chromium) in CARD {
        let img = render(
            "<div style='position:absolute;left:100px;top:40px;width:200px;height:100px;background:#fff;filter:drop-shadow(0 18px 30px rgba(0,0,0,.45))'></div>",
            400,
            300,
            scale,
        );
        let ours: Vec<u8> = (0..15).map(|i| red(&img, scale, 200, 150 + 5 * i)).collect();
        assert!(
            ours.iter().zip(chromium).all(|(&a, b)| a.abs_diff(b) <= 2),
            "card, scale {scale}: {ours:?} vs Chromium {chromium:?}"
        );
    }
}

/// `filter: blur()` takes its length as the standard deviation.
#[test]
fn a_blur_filter_blurs_the_element() {
    // `blur(10px)` of a black box with its right edge at x = 200, on #888: x = 176, 179, ... 221.
    const CHROMIUM: [(f64, [u8; 16]); 2] = [
        (1.0, [1, 2, 4, 9, 16, 26, 39, 54, 71, 86, 101, 114, 123, 129, 133, 135]),
        (2.0, [1, 2, 4, 9, 16, 26, 38, 53, 69, 85, 100, 112, 122, 128, 132, 134]),
    ];
    for (scale, chromium) in CHROMIUM {
        let img = render(
            "<div style='position:absolute;left:100px;top:50px;width:100px;height:200px;background:#000;filter:blur(10px)'></div>",
            400,
            300,
            scale,
        );
        let ours: Vec<u8> = (0..16).map(|i| red(&img, scale, 176 + 3 * i, 150)).collect();
        assert!(
            ours.iter().zip(chromium).all(|(&a, b)| a.abs_diff(b) <= 2),
            "scale {scale}: {ours:?} vs Chromium {chromium:?}"
        );
    }
}

/// A frame of 4 M device px or more is rasterized on worker threads, where vello_cpu cannot
/// filter: the filter is done before it, the same.
#[test]
fn a_drop_shadow_is_painted_on_a_frame_rendered_on_threads() {
    let body = "<div style='position:absolute;left:100px;top:50px;width:100px;height:200px;background:#000;filter:drop-shadow(0 0 20px #000)'></div>";
    let small = render(body, 400, 300, 2.0);
    let large = render(body, 1600, 1300, 2.0);
    assert!(1600 * 1300 * 4 >= 4_000_000);
    for x in [201, 210, 220, 240] {
        assert_eq!(red(&large, 2.0, x, 150), red(&small, 2.0, x, 150), "x {x}");
    }
    assert!(red(&large, 2.0, 210, 150) < 0x88 - 10);
}

/// A blurred box is blurred in all four channels, as Chromium's blur filter does, whatever else the
/// layer holds: WPT's filtered-inline-applies-to-block (a link's `blur()` over a block holding a
/// green inline-block) renders as its reference (the inline-block blurred itself). The link's
/// layer also held an invisible command in the link's colour, so its box was blurred as content of
/// several colours and the reference's as content of one, a level apart on 808 pixels.
#[test]
fn a_blurred_box_is_blurred_alike_beside_other_commands() {
    let page = |body: &str| {
        let html = format!("<!doctype html><style>.box{{display:inline-block;width:100px;height:100px;background:green}}</style>{body}");
        let fetcher: Arc<dyn Fetcher> = Arc::new(WhitePng);
        let snapshot = Snapshot {
            html,
            base_url: "http://test/".into(),
            width: 300,
            height: 200,
            device_scale_factor: 1.0,
            ..Default::default()
        };
        Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
    };
    let test = page("<a href='https://example.com/' style='filter:blur(2px)'><div><span class=box></span></div></a>");
    let reference = page("<div><span class=box style='filter:blur(2px)'></span></div>");
    let differ = test.data.chunks_exact(4).zip(reference.data.chunks_exact(4)).filter(|(a, b)| a != b).count();
    assert_eq!(differ, 0, "{differ} pixels differ");
    // Blurred: the box's edge is soft.
    let green = |img: &RgbaImage, x: usize| img.data[(50 * img.width as usize + x) * 4 + 1];
    assert!(green(&test, 8) > 128 && green(&test, 8) < 255, "{}", green(&test, 8));
}

/// A drop shadow's offset brings into view what lies outside it: an element out of the viewport
/// (WPT `filters-drop-shadow-003`), and content an ancestor's `overflow` clips away (WPT
/// `drop-shadow-clipped-001`). Culling left out the element, and the content the shadow is made of.
#[test]
fn a_drop_shadow_of_what_is_out_of_view_is_painted() {
    for scale in [1.0, 2.0] {
        let img = render(
            "<div style='position:relative;left:-1000px;top:-1000px;width:60px;height:60px;background:#fff;filter:drop-shadow(1040px 1040px 0 #000)'></div>",
            200,
            160,
            scale,
        );
        assert_eq!(red(&img, scale, 70, 70), 0, "the shadow of a box out of view, scale {scale}");
        assert_eq!(red(&img, scale, 30, 30), 0x88, "above its shadow, scale {scale}");
        let img = render(
            "<div style='overflow:hidden;width:100px;height:100px'><div style='filter:drop-shadow(-105px 0 0 #000)'>\
             <div style='position:relative;left:105px;width:50px;height:50px;background:#fff'></div></div></div>",
            200,
            160,
            scale,
        );
        assert_eq!(red(&img, scale, 25, 25), 0, "the shadow of clipped content, scale {scale}");
        assert_eq!(red(&img, scale, 25, 75), 0x88, "below it, scale {scale}");
        assert_eq!(red(&img, scale, 125, 25), 0x88, "the content, clipped, scale {scale}");
    }
}
