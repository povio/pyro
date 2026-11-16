//! Static positions of absolutely positioned boxes (CSS Positioned Layout 3 § 4.1, the
//! static-position rectangle), as Chromium computes them:
//! - an inline-level box in a line is put on the start side of its place in the direction of
//!   the text there (its bidi level), not of the containing block: Latin text in a right-to-left
//!   paragraph runs left to right, and so does the box after it;
//! - an inline-level box among block-level siblings has the static position of the start of a
//!   line at its place: beside the floats there, where `text-align` puts the line's content;
//! - a box in the top layer has the initial containing block as its rectangle.
//! Pixel checks with Ahem, as WPT's `css/css-position/static-position` reftests.
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

/// `body` on white at 600x300 CSS px, with Ahem as family `A`.
fn render(css: &str, body: &str) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(AhemFetch);
    let snapshot = Snapshot {
        html: format!(
            "<!doctype html><style>@font-face {{ font-family: A; src: url(ahem.ttf) }}
             html, body {{ margin: 0; background: #fff }} {css}</style>{body}"
        ),
        base_url: "http://test/".into(),
        width: 600,
        height: 300,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

fn pixel(img: &RgbaImage, x: u32, y: u32) -> [u8; 3] {
    let i = (y as usize * img.width as usize + x as usize) * 4;
    [img.data[i], img.data[i + 1], img.data[i + 2]]
}

/// How many pixels of `color` the image has.
fn count(img: &RgbaImage, color: [u8; 3]) -> usize {
    img.data.chunks_exact(4).filter(|px| px[..3] == color).count()
}

const RED: [u8; 3] = [255, 0, 0];
const GREEN: [u8; 3] = [0, 128, 0];

#[test]
fn an_inline_level_box_after_latin_text_in_a_right_to_left_paragraph_runs_left_to_right() {
    // The line is one left-to-right run ("XXX", "XX", the box's place, red "XXXXX") set at the
    // right: the green box starts at its place and covers the red text after it.
    let img = render(
        ".c { direction: rtl; width: 400px; font: 16px/1 A; color: green; background: green }
         .abs { position: absolute; display: inline; color: green } .red { color: red }",
        "<div class=c>XXX<span>XX<div class=abs>XXXXX</div><span class=red>XXXXX</span></span></div>",
    );
    assert_eq!(count(&img, RED), 0, "the box covers the text after its place");
    assert_eq!(pixel(&img, 330, 8), GREEN);
}

#[test]
fn an_inline_level_box_among_blocks_starts_its_line_beside_a_float() {
    // A 50px float at the left of the second half: the box's line starts after it.
    let img = render(
        "#c { position: relative; background: red; width: 100px; height: 100px }
         #c > div { background: green } #inflow { height: 50px }
         #float { float: left; width: 50px; height: 50px }
         #abs { display: inline; position: absolute; width: 50px; height: 50px }",
        "<div id=c><div id=inflow></div><div id=float></div><div id=abs></div></div>",
    );
    assert_eq!(count(&img, RED), 0);
}

#[test]
fn an_inline_level_box_among_blocks_is_where_text_align_puts_the_line() {
    // `text-align: center` in the 50px beside a right float: the box's left edge is at 25px, a
    // right-to-left one's right edge.
    for (direction, shift) in [("ltr", "-50%"), ("rtl", "50%")] {
        let img = render(
            "#c { position: relative; background: red; width: 100px; height: 100px; text-align: center }
             #c > div { background: green } #inflow { height: 50px }
             #float { float: right; width: 50px; height: 50px }
             #abs { display: inline; position: absolute; width: 50px; height: 50px }",
            &format!(
                "<div id=c style='direction: {direction}'><div id=inflow></div><div id=float></div>
                 <div id=abs style='transform: translateX({shift})'></div></div>"
            ),
        );
        assert_eq!(count(&img, RED), 0, "{direction}");
    }
}
