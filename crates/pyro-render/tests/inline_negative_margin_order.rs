//! CSS 2.1 Appendix E paints a line's text and inline boxes in tree order: an inline box pulled
//! back over the text before it by a negative left margin paints its background over that text,
//! and its own text over its background (WPT CSS2 `css1/c5504-imrgn-l-005`). At 20px Ahem every
//! glyph is a 20px square and a space shows the background behind it.
use std::sync::Arc;

use pyro_render::*;

/// Serves Ahem (`tests/fonts`) as `ahem.ttf`.
struct AhemFetch;
impl Fetcher for AhemFetch {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(if url.ends_with("/ahem.ttf") {
            Ok(include_bytes!("fonts/Ahem.ttf").to_vec())
        } else {
            Err("offline".into())
        });
    }
}

fn render(body: &str) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(AhemFetch);
    let snapshot = Snapshot {
        html: format!(
            "<!doctype html><style>@font-face {{ font-family: A; src: url(ahem.ttf) }}
             html, body {{ margin: 0; background: #fff }} p {{ margin: 0; font: 20px/20px A; color: #0f0 }}</style>{body}"
        ),
        base_url: "http://test/".into(),
        width: 400,
        height: 100,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    let mut renderer = Renderer::new();
    renderer.text_antialiasing = false;
    renderer.render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

fn pixel(img: &RgbaImage, x: usize, y: usize) -> [u8; 3] {
    let i = (y * img.width as usize + x) * 4;
    [img.data[i], img.data[i + 1], img.data[i + 2]]
}

#[test]
fn a_box_pulled_back_paints_over_the_text_before_it() {
    // "xx xx " is 0..120; the span is pulled back 60px to start at 60: its orange "x" (60..80)
    // and its blue space (80..100) land on the paragraph's second "xx" (60..100).
    let img = render("<p>xx xx <span style='margin-left:-60px;background:#00f;color:#f80'>x x</span></p>");
    assert_eq!(pixel(&img, 50, 10), [255, 255, 255], "the paragraph's space");
    assert_eq!(pixel(&img, 70, 10), [255, 136, 0], "the span's first x over the paragraph's text");
    assert_eq!(pixel(&img, 90, 10), [0, 0, 255], "the span's space over the paragraph's text");
}
