//! Containment and SVG images: style containment scopes counters and quotes to the element's
//! subtree (CSS Contain 2 § 3.3); an SVG image's root CSS background paints its canvas. (Layout
//! and paint containment keeping overflow out of an ancestor's scroll range has no scroll API here:
//! WPT css-contain `contain-layout-ink-overflow-*`, a gated directory, covers it.)
use std::sync::Arc;

use pyro_render::*;


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

/// A 20x20 green square.
const SQUARE: &str = "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='20' height='20'%3E%3Crect width='20' height='20' fill='%2300a000'/%3E%3C/svg%3E";

fn render(body: &str) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(DataUrls);
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

const LETTER: &str = "<style>div::first-letter{color:#0a0;font-size:36px;line-height:2}
  .ref{color:#0a0;font-size:36px;line-height:2}</style>";

#[test]

#[test]
fn style_containment_scopes_counters_and_quotes() {
    // WPT css-contain counter-scoping-001: the increment in the subtree makes a new counter.
    matches(
        "<style>div{contain:style;counter-increment:n} div::before,div::after{content:counters(n,'.') ' '} div::after{counter-increment:n 2}</style><div></div>",
        "<div>1 1.2</div>",
        "<style>div{counter-increment:n} div::before,div::after{content:counters(n,'.') ' '} div::after{counter-increment:n 2}</style><div></div>",
    );
    // quote-scoping-001: the depth inside starts at the outer one, and is restored after.
    matches(
        "<style>div{quotes:'A' 'Z' '1' '9'} div::before,span::before{content:open-quote} div::after{content:close-quote} span{contain:style}</style><div><span></span></div>",
        "<div>A1Z</div>",
        "<style>div{quotes:'A' 'Z' '1' '9'} div::before,span::before{content:open-quote} div::after{content:close-quote}</style><div><span></span></div>",
    );
}

#[test]
fn an_svg_image_paints_its_root_background() {
    let svg = "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='20' height='20' style='background: %2300a000'%3E%3Cg/%3E%3C/svg%3E";
    matches(&format!("<img src=\"{svg}\">"), "<span style='display:inline-block;width:20px;height:20px;background:#00a000'></span>", "");
}

