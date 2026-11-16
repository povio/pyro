//! `initial-letter` on `::first-letter` (CSS Inline 3 § 7, laid out as Blink does): the letter's
//! box is as big as the ink of its glyph, its cap height spans `size` lines, its baseline sits on
//! the sink's line (raised and sunken letters move the text down), it starts the first line after
//! the text indent, and lines, later floats and the next paragraph's initial letter keep out of
//! it. Ahem (all glyphs full em boxes, cap height 0.8em) at 20px on 24px lines: a 3-line letter is
//! 80px square, 2px below the line's top.
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

/// `body` on white at 600x300 CSS px; `.p` paragraphs are 230px wide, 20px Ahem on 24px lines,
/// their `::first-letter` lime with the given `initial-letter` rules (`letter`).
fn render(css: &str, body: &str) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(AhemFetch);
    let snapshot = Snapshot {
        html: format!(
            "<!doctype html><style>@font-face {{ font-family: A; src: url(ahem.ttf) }}
             html, body {{ margin: 0; background: #fff }}
             .p {{ font: 20px/24px A; width: 230px; color: black }}
             .p::first-letter {{ color: lime }} {css}</style>{body}"
        ),
        base_url: "http://test/".into(),
        width: 600,
        height: 300,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    let mut renderer = Renderer::new();
    renderer.text_antialiasing = false;
    renderer.render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

/// The bounding box `[x0, y0, x1, y1)` of the pixels of `rgb` in `x0..x1` × `y0..y1`.
fn bbox(img: &RgbaImage, rgb: [u8; 3], area: [usize; 4]) -> Option<[usize; 4]> {
    let mut out: Option<[usize; 4]> = None;
    for y in area[1]..area[3].min(img.height as usize) {
        for x in area[0]..area[2].min(img.width as usize) {
            let i = (y * img.width as usize + x) * 4;
            if img.data[i..i + 3] == rgb {
                out = Some(match out {
                    None => [x, y, x + 1, y + 1],
                    Some([x0, y0, x1, y1]) => [x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1)],
                });
            }
        }
    }
    out
}

const LIME: [u8; 3] = [0, 255, 0];
const BLACK: [u8; 3] = [0, 0, 0];
const ALL: [usize; 4] = [0, 0, 600, 300];

/// The black ink of the line box `top..top + 24`.
fn line(img: &RgbaImage, top: usize) -> Option<[usize; 4]> {
    bbox(img, BLACK, [0, top, 600, top + 24])
}

#[test]
fn a_drop_initial_spans_three_lines() {
    let img = render(".p::first-letter { initial-letter: 3 }", "<div class=p>Abc<br>def<br>ghi<br>jkl<br>mno</div>");
    assert_eq!(bbox(&img, LIME, ALL), Some([0, 2, 80, 82]));
    // Lines 1 to 4 start beside the letter, the fifth below it.
    assert_eq!(line(&img, 0), Some([80, 2, 120, 22]));
    assert_eq!(line(&img, 72), Some([80, 74, 140, 94]));
    assert_eq!(line(&img, 96), Some([0, 98, 60, 118]));
    // `drop` is the same as no sink.
    let dropped = render(".p::first-letter { initial-letter: 3 drop }", "<div class=p>Abc<br>def</div>");
    assert_eq!(bbox(&dropped, LIME, ALL), Some([0, 2, 80, 82]));
}

#[test]
fn raised_and_sunken_initials_move_the_text_down() {
    // `raise`: the baseline on the first line's, which moves down two lines.
    let raised = render(".p::first-letter { initial-letter: 3 raise }", "<div class=p>Abc<br>def<br>ghi</div>");
    assert_eq!(bbox(&raised, LIME, ALL), Some([0, 2, 80, 82]));
    assert_eq!(line(&raised, 0), None);
    assert_eq!(line(&raised, 48), Some([80, 50, 120, 70]));
    assert_eq!(line(&raised, 96), Some([0, 98, 60, 118]));
    // Sunk two lines: the text moves down one.
    let sunk = render(".p::first-letter { initial-letter: 3 2 }", "<div class=p>Abc<br>def</div>");
    assert_eq!(bbox(&sunk, LIME, ALL), Some([0, 2, 80, 82]));
    assert_eq!(line(&sunk, 24), Some([80, 26, 120, 46]));
}

#[test]
fn the_box_takes_the_text_indent_and_its_margins() {
    // The indent is before the box and in it (Blink): the glyph is two indents in.
    let indented =
        render(".p { text-indent: 10px } .p::first-letter { initial-letter: 3 }", "<div class=p>Abc<br>def</div>");
    assert_eq!(bbox(&indented, LIME, ALL), Some([20, 2, 100, 82]));
    assert_eq!(line(&indented, 0), Some([100, 2, 140, 22]));
    assert_eq!(line(&indented, 24), Some([100, 26, 160, 46]));
    let margins = render(
        ".p::first-letter { initial-letter: 3; margin: 10px 45px 30px 15px }",
        "<div class=p>Abc<br>def<br>ghi<br>jkl<br>mno</div>",
    );
    assert_eq!(bbox(&margins, LIME, ALL), Some([15, 12, 95, 92]));
    assert_eq!(line(&margins, 0), Some([140, 2, 180, 22]));
    // The bottom margin keeps the fifth line beside the letter.
    assert_eq!(line(&margins, 96), Some([140, 98, 200, 118]));
}

#[test]
fn floats_and_the_next_initial_letter_keep_out_of_it() {
    // A left float after the first line is placed below the letter.
    let img = render(
        ".p::first-letter { initial-letter: 3 } i { float: left; width: 50px; height: 50px; background: blue }",
        "<div class=p>Abc<br><i></i>def<br>ghi<br>jkl<br>mno</div>",
    );
    assert_eq!(bbox(&img, [0, 0, 255], ALL), Some([0, 82, 50, 132]));
    // The next paragraph's first line starts below this one's letter.
    let two = render(".p::first-letter { initial-letter: 3 }", "<div class=p>Abc<br>Def</div><div class=p>WXY</div>");
    assert_eq!(bbox(&two, LIME, [0, 0, 600, 82]), Some([0, 2, 80, 82]));
    assert_eq!(bbox(&two, LIME, [0, 82, 600, 300]), Some([0, 84, 80, 164]));
    assert_eq!(line(&two, 82), Some([80, 84, 120, 104]));
}

#[test]
fn a_vertical_initial_is_centred_on_its_lines() {
    // `vertical-rl`: three 24px lines from the right edge, the 80px letter 4px past them.
    let img = render(
        ".p { writing-mode: vertical-rl; width: auto; height: 230px } .p::first-letter { initial-letter: 3 }",
        "<div class=p style='position: absolute; right: 100px'>Abc<br>def<br>ghi<br>jkl<br>mno</div>",
    );
    let block = bbox(&img, BLACK, ALL).unwrap();
    let letter = bbox(&img, LIME, ALL).unwrap();
    assert_eq!(letter[2] - letter[0], 80);
    assert_eq!(letter[3] - letter[1], 80);
    assert_eq!(letter[2], 500 + 4);
    assert_eq!(letter[1], 0);
    assert_eq!(block[2], 500 - 2);
}
