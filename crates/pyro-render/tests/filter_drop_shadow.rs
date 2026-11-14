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
