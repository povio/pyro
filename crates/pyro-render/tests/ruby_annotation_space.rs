//! A ruby annotation that overflows its line box takes the room the neighbouring line leaves
//! beyond its content on that side, its half-leading (CSS Ruby 1 § 4.4, as Blink's
//! `InlineLayoutAlgorithm` with `BlockStartAnnotationSpace`): an annotation under one line's text
//! the half-leading over the next line's, one over a line's text the half-leading under the
//! previous line's. Ahem at 20px on 24px lines (2px half-leading); a 10px annotation overflows
//! the line box by 8px.
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

/// `body` on white at 300x200 CSS px, in 20px Ahem on 24px lines.
fn render(css: &str, body: &str) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(AhemFetch);
    let snapshot = Snapshot {
        html: format!(
            "<!doctype html><style>@font-face {{ font-family: A; src: url(ahem.ttf) }}
             html, body {{ margin: 0; background: #fff }}
             div {{ font: 20px/24px A; width: 230px; color: black }} {css}</style>{body}"
        ),
        base_url: "http://test/".into(),
        width: 300,
        height: 200,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    let mut renderer = Renderer::new();
    renderer.text_antialiasing = false;
    renderer.render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

/// The rows `[y0, y1)` of the black runs starting at x 0 (the lines' first words).
fn first_words(img: &RgbaImage) -> Vec<[usize; 2]> {
    let mut out: Vec<[usize; 2]> = Vec::new();
    for y in 0..img.height as usize {
        let i = y * img.width as usize * 4;
        if img.data[i..i + 3] == [0, 0, 0] {
            match out.last_mut() {
                Some(last) if last[1] == y => last[1] = y + 1,
                _ => out.push([y, y + 1]),
            }
        }
    }
    out
}

#[test]
fn an_under_annotation_takes_the_next_lines_half_leading() {
    let img = render(
        "ruby { ruby-position: under } .b { background: lime; height: 10px }",
        "<div>abc <ruby>xyz<rt>XYZ</rt></ruby><br>def<br>ghi</div><div class=b></div>",
    );
    // The first line is 24 + 8 - 2 = 30px tall.
    assert_eq!(first_words(&img), vec![[2, 22], [32, 52], [56, 76]]);
    // The block ends below the third line's box: 30 + 24 + 24.
    let lime = (0..200).find(|&y| img.data[(y * 300 + 5) * 4 + 1] == 255 && img.data[(y * 300 + 5) * 4] == 0);
    assert_eq!(lime, Some(78));
}

#[test]
fn an_over_annotation_takes_the_previous_lines_half_leading() {
    let img = render("", "<div>abc<br>def <ruby>xyz<rt>XYZ</rt></ruby><br>ghi</div>");
    // The second line's text 8 - 2 = 6px further down than without the annotation.
    assert_eq!(first_words(&img), vec![[2, 22], [32, 52], [56, 76]]);
}

#[test]
fn the_first_line_has_no_line_to_take_room_from() {
    let img = render("", "<div><ruby>abc<rt>XYZ</rt></ruby><br>def</div>");
    assert_eq!(first_words(&img), vec![[10, 30], [34, 54]]);
}
