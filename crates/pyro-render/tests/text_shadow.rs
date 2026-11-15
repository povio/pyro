//! `text-shadow` is painted as Chromium paints it: each shadow is the glyphs' shape moved by its
//! offset in its colour (transparent text casts one too), blurred by a Gaussian of half its blur
//! radius, the first shadow on top, all under the text and inside the element's opacity; text
//! decorations cast the shadow too. The checks compare renderings with each other, so they hold
//! with whatever font the system has.
use std::sync::Arc;

use pyro_render::*;

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}

const BACKGROUND: u8 = 0x88;

/// `text` in a 60px bold font at (40, 40) on #888, the div styled with `style`.
fn render(text: &str, style: &str, scale: f64) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let snapshot = Snapshot {
        html: format!(
            "<!doctype html><body style='margin:0;background:#888;width:400px;height:300px'>\
             <div style='position:absolute;left:40px;top:40px;font:bold 60px/60px sans-serif;{style}'>{text}</div>"
        ),
        base_url: "http://test/".into(),
        width: 400,
        height: 300,
        device_scale_factor: scale,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

fn rgb(img: &RgbaImage, x: usize, y: usize) -> [u8; 3] {
    let i = (y * img.width as usize + x) * 4;
    [img.data[i], img.data[i + 1], img.data[i + 2]]
}

/// The device pixels of `img` (rows `rows`) and their red channel.
fn reds(img: &RgbaImage, rows: std::ops::Range<usize>) -> impl Iterator<Item = (usize, usize, u8)> + '_ {
    rows.flat_map(move |y| (0..img.width as usize).map(move |x| (x, y, rgb(img, x, y)[0])))
}

/// A hard shadow is the glyphs moved by its offset: under white text, pixel for pixel what the
/// text itself paints in black, 80px lower.
#[test]
fn a_text_shadow_is_the_glyphs_moved_by_its_offset() {
    for scale in [1.0, 2.0] {
        let text = render("Hg", "color:#000", scale);
        let shadowed = render("Hg", "color:#fff;text-shadow:0 80px 0 #000", scale);
        let transparent = render("Hg", "color:transparent;text-shadow:0 80px 0 #000", scale);
        // The text's rows (its line is 40..100), and the shadow's, clear of the text.
        let (dy, rows) = ((80.0 * scale) as usize, (36.0 * scale) as usize..(112.0 * scale) as usize);
        let mut inked = 0;
        for (x, y, red) in reds(&text, rows) {
            inked += usize::from(red < 64);
            for (img, what) in [(&shadowed, "white text"), (&transparent, "transparent text")] {
                let shadow = rgb(img, x, y + dy)[0];
                assert!(shadow.abs_diff(red) <= 1, "{what}, scale {scale}: ({x}, {y}): shadow {shadow}, text {red}");
            }
            assert_eq!(rgb(&transparent, x, y), [BACKGROUND; 3], "transparent text is not drawn");
        }
        assert!(inked > 100, "scale {scale}: the text has ink");
    }
}

/// A blurred shadow keeps the hard shadow's ink and spreads it by a Gaussian of half the blur
/// radius: the variance of the ink along a row grows by the blur's (Chromium's three boxes have a
/// deviation of 0.94 times the one asked for).
#[test]
fn a_blurred_text_shadow_spreads_by_half_its_radius() {
    for scale in [1.0, 2.0] {
        let moments = |style: &str| {
            let img = render("I", &format!("color:transparent;{style}"), scale);
            let (mut ink, mut sum, mut sq) = (0.0, 0.0, 0.0);
            for (x, _, red) in reds(&img, 0..img.height as usize) {
                let w = f64::from(BACKGROUND - red.min(BACKGROUND));
                (ink, sum, sq) = (ink + w, sum + w * x as f64, sq + w * (x * x) as f64);
            }
            let mean = sum / ink;
            (ink, sq / ink - mean * mean)
        };
        let (hard_ink, hard_var) = moments("text-shadow:0 0 0 #000");
        let (blur_ink, blur_var) = moments("text-shadow:0 0 16px #000");
        assert!(hard_ink > 1000.0, "scale {scale}: no hard shadow");
        // Within 4%: the blur's faint tail is many pixels a level or two of alpha deep, and their ink
        // is each rounded to a whole level (to nearest, as Skia composites; vello_cpu rounded up, which
        // dropped some of it: 3.1% more ink at scale 2 since).
        assert!((blur_ink / hard_ink - 1.0).abs() < 0.04, "scale {scale}: ink {blur_ink} vs {hard_ink}");
        let sigma = 8.0 * scale;
        let spread = (blur_var - hard_var).max(0.0).sqrt() / sigma;
        assert!((0.85..1.05).contains(&spread), "scale {scale}: deviation {spread} of the one asked for");
    }
}

/// The first shadow is on top, the text over all of them, and the element's opacity fades them
/// with it.
#[test]
fn text_shadows_paint_in_order_under_the_text_and_inside_the_opacity() {
    let scale = 1.0;
    let text = render("H", "color:#000", scale);
    // A pixel the glyph covers fully.
    let (x, y, _) = reds(&text, 0..300).find(|&(_, _, red)| red == 0).expect("a fully covered pixel");
    let img = render("H", "color:#000;text-shadow:0 70px 0 rgb(255,0,0),0 70px 0 rgb(0,0,255)", scale);
    assert_eq!(rgb(&img, x, y + 70), [255, 0, 0], "the first shadow on top");
    let img = render("H", "color:#000;text-shadow:0 0 0 rgb(255,0,0)", scale);
    assert_eq!(rgb(&img, x, y), [0, 0, 0], "the text over its shadow");
    let img = render("H", "color:transparent;opacity:.5;text-shadow:0 70px 0 #000", scale);
    let faded = rgb(&img, x, y + 70)[0];
    assert!(faded.abs_diff(BACKGROUND / 2) <= 2, "faded with the element: {faded}");
}

/// An underline casts the text's shadow.
#[test]
fn an_underline_casts_the_text_shadow() {
    let scale = 1.0;
    let style = "color:transparent;text-decoration:underline;text-decoration-color:#000";
    let line = render("xxx", style, scale);
    let (x, y, _) = reds(&line, 0..300).find(|&(_, _, red)| red == 0).expect("the underline");
    let img = render("xxx", &format!("{style};text-shadow:0 12px 0 rgb(255,0,0)"), scale);
    assert_eq!(rgb(&img, x, y + 12), [255, 0, 0]);
}
