//! Line boxes paint in order, each with its inline boxes and then its text (CSS 2.1 Appendix E):
//! an inline box whose padding reaches into the line above paints over that line's text.
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
        html: format!("<!doctype html><style>body{{margin:0;background:white}}</style>{body}"),
        base_url: "http://test/".into(),
        width: 400,
        height: 200,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

fn dark_pixels(img: &RgbaImage, x0: u32, x1: u32, y0: u32, y1: u32) -> usize {
    let mut n = 0;
    for y in y0..y1 {
        for x in x0..x1 {
            let i = (y as usize * img.width as usize + x as usize) * 4;
            if img.data[i] < 100 && img.data[i + 1] < 100 && img.data[i + 2] < 100 {
                n += 1;
            }
        }
    }
    n
}

#[test]
fn a_box_reaching_into_the_line_above_paints_over_its_text() {
    // Lines 30px apart; the second line's span is padded 30px up, over the first line, under
    // the span's own text (blue box, white text).
    let html = "<p style='margin:0;font:20px/30px monospace;color:black'>0000<br><span style='background:#00f;color:white;padding-top:30px'>0000</span></p>";
    let img = render(html);
    let without = render("<p style='margin:0;font:20px/30px monospace;color:black'>0000<br>0000</p>");
    assert!(dark_pixels(&without, 0, 40, 0, 30) > 0, "the first line has dark text");
    assert_eq!(dark_pixels(&img, 0, 40, 0, 30), 0, "the second line's box covers it");
}

#[test]
fn a_text_clipped_background_paints_line_by_line() {
    // `background-clip: text` on a box that reaches above its later lines: its mask is drawn
    // while the root paints line by line (that panicked: the root held the text context).
    let img = render(
        "<p style='width:120px;font:20px/20px monospace;margin:40px 0 0'><span style='padding-top:15px;\
         background:linear-gradient(#000,#000);background-clip:text;color:transparent'>00 00 00 00 00 00</span></p>",
    );
    assert!(dark_pixels(&img, 0, 120, 40, 100) > 0, "the glyphs show the background");
}
