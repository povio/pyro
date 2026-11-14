//! `Renderer::text_antialiasing` (`serve --text-antialiasing off`): off, glyphs are drawn aliased,
//! as Chromium draws text when WPT's runner starts it with `NoFontAntialiasing`, so every pixel of
//! black text on white is black or white; on (the default), glyph edges are blended. Other drawing
//! stays antialiased either way. The checks hold with whatever font the system has.
use std::sync::Arc;

use pyro_render::*;

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}

/// Black text at a fractional position, and a black circle, on white.
fn render(antialiased: bool, scale: f64) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let snapshot = Snapshot {
        html: "<!doctype html><body style='margin:0;background:#fff;width:300px;height:200px'>\
               <div style='position:absolute;left:10.3px;top:10.6px;font:30px/40px sans-serif;color:#000'>Hgw</div>\
               <div style='position:absolute;left:150px;top:100px;width:60px;height:60px;border-radius:50%;background:#000'></div>"
            .into(),
        base_url: "http://test/".into(),
        width: 300,
        height: 200,
        device_scale_factor: scale,
        ..Default::default()
    };
    let mut renderer = Renderer::new();
    renderer.text_antialiasing = antialiased;
    renderer.render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

/// Pixels of `img` inside the device-pixel box (`x0..x1`, `y0..y1`) that are black, and that are
/// neither black nor white.
fn census(img: &RgbaImage, x: std::ops::Range<usize>, y: std::ops::Range<usize>) -> (usize, usize) {
    let (mut black, mut blended) = (0, 0);
    for y in y {
        for x in x.clone() {
            let i = (y * img.width as usize + x) * 4;
            match img.data[i] {
                0 => black += 1,
                255 => {}
                _ => blended += 1,
            }
        }
    }
    (black, blended)
}

#[test]
fn text_is_aliased_with_antialiasing_off_and_nothing_else_is() {
    for scale in [1.0, 2.0] {
        let px = |v: f64| (v * scale) as usize;
        let text = |img: &RgbaImage| census(img, px(0.0)..px(140.0), px(0.0)..px(60.0));
        let circle = |img: &RgbaImage| census(img, px(140.0)..px(220.0), px(90.0)..px(170.0));

        let aliased = render(false, scale);
        let (ink, blended) = text(&aliased);
        assert!(ink > px(100.0), "scale {scale}: the text has ink ({ink} black pixels)");
        assert_eq!(blended, 0, "scale {scale}: aliased text has only black and white pixels");
        let (_, edge) = circle(&aliased);
        assert!(edge > 0, "scale {scale}: the circle's edge stays antialiased");

        let antialiased = render(true, scale);
        let (_, blended) = text(&antialiased);
        assert!(blended > 0, "scale {scale}: antialiased text has blended edges");
        assert_eq!(circle(&antialiased), circle(&aliased), "scale {scale}: the circle does not change");
    }
}

/// `rotate(360deg)` and other turns by multiples of 90 degrees are exact, as in Chromium: the text
/// of a box turned all the way round draws as untransformed text does (WPT
/// css-transforms/transform-input-*, whose few differing pixels fall outside their fuzzy limits
/// once text is aliased).
#[test]
fn text_turned_by_a_whole_turn_draws_as_untransformed_text() {
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let render = |transform: &str, antialiased: bool| {
        let snapshot = Snapshot {
            html: format!(
                "<!doctype html><body style='margin:0;background:#fff'>\
                 <div style='margin:10.3px;width:120px;font:15px/20px sans-serif;transform:{transform}'>abc 555-5555</div>"
            ),
            base_url: "http://test/".into(),
            width: 200,
            height: 60,
            device_scale_factor: 1.0,
            ..Default::default()
        };
        let mut renderer = Renderer::new();
        renderer.text_antialiasing = antialiased;
        renderer.render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0.data
    };
    for antialiased in [true, false] {
        let plain = render("none", antialiased);
        for turn in ["rotate(360deg)", "rotate(-720deg)", "rotate(180deg) rotate(180deg)"] {
            assert!(render(turn, antialiased) == plain, "{turn}, antialiased {antialiased}");
        }
    }
}
