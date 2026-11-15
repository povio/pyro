//! Direction and position of inline text, compared as reftests are: a page and its reference
//! render pixel for pixel alike in the same renderer (and unlike the page without the feature).
//!
//! - `:dir()` matches by the directionality of elements (the nearest valid `dir`).
//! - A `unicode-bidi: bidi-override` holds on the lines after a `<br>`.
//! - The decorations of a relatively positioned inline box move with its text.
use std::sync::Arc;

use pyro_render::*;

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}

fn render(body: &str) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let snapshot = Snapshot {
        html: format!(
            "<!doctype html><style>body{{margin:0;background:white;font:20px/30px sans-serif;white-space:nowrap}}</style>{body}"
        ),
        base_url: "http://test/".into(),
        width: 400,
        height: 200,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

/// How many pixels differ between two renders.
fn differing(a: &RgbaImage, b: &RgbaImage) -> usize {
    assert_eq!((a.width, a.height), (b.width, b.height));
    a.data.chunks(4).zip(b.data.chunks(4)).filter(|(p, q)| p[..3] != q[..3]).count()
}

/// `test` renders as `reference` does, and unlike `plain` (the page without the feature).
fn matches(test: &str, reference: &str, plain: &str) {
    let (t, r, p) = (render(test), render(reference), render(plain));
    assert_eq!(differing(&t, &r), 0, "{test}\n  differs from the reference\n{reference}");
    assert!(differing(&t, &p) > 0, "{test}\n  renders as the page without the feature\n{plain}");
}

#[test]
fn dir_matches_the_directionality_of_elements() {
    // `:dir()` follows the nearest `dir` attribute (here the element's parent's).
    matches(
        "<style>p:dir(rtl){color:#0a0} p:dir(ltr){color:#00f}</style><div dir=rtl><p>Right</p></div><p>Left</p>",
        "<div dir=rtl><p style='color:#0a0'>Right</p></div><p style='color:#00f'>Left</p>",
        "<div dir=rtl><p>Right</p></div><p>Left</p>",
    );
}

#[test]
fn bidi_override_holds_after_a_line_break() {
    // A forced break ends the bidi algorithm's paragraph; the override opens again after it, as
    // Blink does.
    matches(
        "<p dir=rtl style='unicode-bidi:bidi-override'>ab<br>cd</p>",
        "<p dir=rtl>ba<br>dc</p>",
        "<p dir=rtl>ba<br>cd</p>",
    );
}

#[test]
fn decorations_go_with_relatively_positioned_text() {
    matches(
        "<p style='margin:0'><span style='position:relative;top:10px;text-decoration:underline'>Text</span></p>",
        "<p style='margin:0;padding-top:10px'><span style='text-decoration:underline'>Text</span></p>",
        "<p style='margin:0;padding-top:10px'><span>Text</span></p>",
    );
}
