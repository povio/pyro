//! Text decorations and sideways text in vertical writing modes, as Chrome 157 paints them (each expected value
//! measured in a Chrome screenshot of the same markup; the Inter subset is the only font).
//!
//! - On the central baseline of `vertical-rl`/`vertical-lr` text (`text-orientation: mixed`), an underline goes 1px
//!   under the em box and an overline on the font's whole-pixel ascent, from the text fragment's line-over (right)
//!   edge; `text-underline-position: right`, and Japanese, Korean or Mongolian text, trade their sides (a `double`
//!   line's second line still goes away from the text). A fallback font places nothing: the style's first available
//!   font does.
//! - `sideways-lr` lines run bottom to top from the content box's bottom, their line-over side on the left, where an
//!   emphasis mark's `right` side is under the line.
//! - `text-decoration-skip-ink` leaves gaps where turned glyphs cross a vertical line's underline.
use std::sync::Arc;

use pyro_render::*;

/// Serves the Inter subset and Ahem (`tests/fonts`) as `inter.ttf` and `ahem.ttf`.
struct InterFetch;
impl Fetcher for InterFetch {
    fn fetch(&self, url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(if url.ends_with("/inter.ttf") {
            Ok(include_bytes!("fonts/Inter-subset.ttf").to_vec())
        } else if url.ends_with("/ahem.ttf") {
            Ok(include_bytes!("fonts/Ahem.ttf").to_vec())
        } else {
            Err("offline".into())
        });
    }
}

/// `body` (absolutely positioned divs at `top: 20px`, in Inter) on white, 800x400 CSS px; `style` is added to the
/// divs' rules. Text is aliased, as Chrome drew it (`NoFontAntialiasing`, as WPT's runner starts it).
fn render(style: &str, body: &str, scale: f64) -> RgbaImage {
    render_with("", style, body, scale)
}

/// [`render`] with the rules `css` added.
fn render_with(css: &str, style: &str, body: &str, scale: f64) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(InterFetch);
    let snapshot = Snapshot {
        html: format!(
            "<!doctype html><style>@font-face {{ font-family: Inter; src: url(inter.ttf) }} {css}
             html, body {{ margin: 0; background: #fff }}
             div {{ position: absolute; top: 20px; font-family: Inter; {style} }}</style>{body}"
        ),
        base_url: "http://test/".into(),
        width: 800,
        height: 400,
        device_scale_factor: scale,
        ..Default::default()
    };
    let mut renderer = Renderer::new();
    renderer.text_antialiasing = false;
    renderer.render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

fn red(img: &RgbaImage, x: usize, y: usize) -> u8 {
    img.data[(y * img.width as usize + x) * 4]
}

/// The columns of `x0..x1` with ink in row `y` (CSS px, at `scale`), as `[start, end)` runs.
fn inked_runs(img: &RgbaImage, y: usize, x0: usize, x1: usize, scale: f64) -> Vec<(f64, f64)> {
    let s = scale as usize;
    let mut runs: Vec<(f64, f64)> = Vec::new();
    for x in x0 * s..x1 * s {
        if red(img, x, y * s) < 128 {
            let x = x as f64 / scale;
            match runs.last_mut() {
                Some(run) if run.1 == x => run.1 = x + 1.0 / scale,
                _ => runs.push((x, x + 1.0 / scale)),
            }
        }
    }
    runs
}

/// The rows of column `x` with ink, top to bottom, as `[start, end)` runs.
fn inked_rows(img: &RgbaImage, x: usize, y0: usize, y1: usize) -> Vec<(usize, usize)> {
    let mut runs: Vec<(usize, usize)> = Vec::new();
    for y in y0..y1 {
        if red(img, x, y) < 128 {
            match runs.last_mut() {
                Some(run) if run.1 == y => run.1 = y + 1,
                _ => runs.push((y, y + 1)),
            }
        }
    }
    runs
}

/// Underlines and overlines of transparent vertical text, 2px thick, 20px Inter on 20px lines: where Chrome draws
/// each (CSS px), at every device scale.
#[test]
fn vertical_decorations_sit_on_the_em_box() {
    let cases: [(&str, (f64, f64)); 10] = [
        // An underline 1px under the em box: 23px from the line-over edge (Inter's 19 + 5 px, halved, and half an em).
        ("writing-mode:vertical-rl", (97.0, 99.0)),
        // `right`: 1px over the em box.
        ("writing-mode:vertical-lr;text-underline-position:right", (171.0, 173.0)),
        // An overline on the whole-pixel ascent.
        ("writing-mode:vertical-rl;text-decoration-line:overline", (222.0, 224.0)),
        // Japanese: the underline is over the text.
        ("writing-mode:vertical-rl' lang='ja", (271.0, 273.0)),
        // An offset moves an underline away from the text, under it or over it.
        ("writing-mode:vertical-rl;text-underline-offset:3px", (294.0, 296.0)),
        ("writing-mode:vertical-lr;text-underline-position:right;text-underline-offset:3px", (374.0, 376.0)),
        // `right` puts an overline where the underline would be.
        ("writing-mode:vertical-rl;text-decoration-line:overline;text-underline-position:right", (397.0, 399.0)),
        // Sideways text keeps the alphabetic baseline; `sideways-lr` has its line-over side on the left.
        ("writing-mode:sideways-rl", (450.0, 452.0)),
        ("writing-mode:sideways-lr", (518.0, 520.0)),
        // Mongolian: over the text, as Japanese (css-text-decor-3's `auto`, Gecko and WPT's
        // `text-underline-position-auto-001`; Chrome leaves Mongolian out and draws it at 547-549).
        ("writing-mode:vertical-rl' lang='mn", (571.0, 573.0)),
    ];
    let body: String = cases
        .iter()
        .enumerate()
        .map(|(i, (css, _))| format!("<div style='left:{}px;{css}'>MONITOR</div>", 100 + 50 * i))
        .collect();
    for scale in [1.0, 2.0] {
        let img = render(
            "font-size:20px;line-height:20px;color:transparent;text-decoration:underline #000;\
             text-decoration-skip-ink:none;text-decoration-thickness:2px",
            &body,
            scale,
        );
        for (i, (css, chrome)) in cases.iter().enumerate() {
            let left = 100 + 50 * i;
            let got = inked_runs(&img, 60, left - 20, left + 30, scale);
            assert_eq!(got, vec![*chrome], "{css} at {scale}: Chrome draws it at {chrome:?}");
        }
    }
}

/// At 37px on 40px lines (Inter's 36 + 9 px are odd), 3px thick (3.7 cut): the fragment's line-over edge is half a
/// line's leading, floored, from the line's; the overline is 1px further out than with an even height.
#[test]
fn vertical_decorations_of_an_odd_height_font() {
    let cases: [(&str, (f64, f64)); 4] = [
        ("writing-mode:vertical-rl;text-decoration-line:overline", (74.0, 77.0)),
        ("writing-mode:vertical-rl", (124.0, 127.0)),
        ("writing-mode:vertical-lr;text-decoration-line:overline", (266.0, 269.0)),
        ("writing-mode:vertical-lr", (316.0, 319.0)),
    ];
    let body: String = cases
        .iter()
        .enumerate()
        .map(|(i, (css, _))| format!("<div style='left:{}px;{css}'>MONITOR</div>", 30 + 96 * i))
        .collect();
    let img = render(
        "font-size:37px;line-height:40px;color:transparent;text-decoration:underline #000;text-decoration-skip-ink:none",
        &body,
        1.0,
    );
    for (i, (css, chrome)) in cases.iter().enumerate() {
        let left = 30 + 96 * i;
        let got = inked_runs(&img, 60, left - 20, left + 76, 1.0);
        assert_eq!(got, vec![*chrome], "{css}: Chrome draws it at {chrome:?}");
    }
}

/// `sideways-lr` text runs bottom to top from the bottom of its 300px box (`text-align: end` and `direction: rtl`
/// take it to the top), its first line on the left: the rows and columns of its ink in Chrome.
#[test]
fn sideways_lr_runs_up_from_the_bottom() {
    let img = render(
        "height:300px;font-size:20px;line-height:20px;color:#000",
        "<div style='left:100px;writing-mode:sideways-lr'>MONITOR x-y</div>\
         <div style='left:300px;writing-mode:sideways-lr;width:60px'>MONITOR OPEN MONITOR OPEN MONITOR OPEN MONITOR</div>\
         <div style='left:400px;writing-mode:sideways-lr;text-align:end'>MONITOR</div>\
         <div style='left:500px;writing-mode:sideways-lr;direction:rtl'>MONITOR x-y</div>",
        1.0,
    );
    // The "M" stems: the line's baseline side is on the right (x 120 is the stems' foot).
    let rows = |x: usize| inked_rows(&img, x, 0, 400);
    assert_eq!(rows(110).last().map(|r| r.1), Some(318), "the text ends at the box's bottom");
    assert!(rows(110).first().is_some_and(|r| r.0 > 150), "and starts above it: {:?}", rows(110));
    assert!(rows(101).is_empty() && !rows(102).is_empty(), "the line's ascent side is on the left");
    // Two lines, left to right, each from the bottom (Chrome ends them a pixel lower: its glyphs, rotated, cover
    // another row there).
    let ends_near = |x: usize, chrome: usize| rows(x).last().is_some_and(|r| r.1.abs_diff(chrome) <= 1);
    assert!(ends_near(310, 319), "first line: {:?}", rows(310));
    assert!(ends_near(330, 320), "second line: {:?}", rows(330));
    // `text-align: end` and `direction: rtl`: at the top.
    assert_eq!(rows(410).first().map(|r| r.0), Some(23));
    assert!(rows(510).first().is_some_and(|r| r.0 < 30), "rtl starts at the top: {:?}", rows(510));
}

/// `text-decoration-skip-ink: auto` cuts a vertical line's underline where the turned glyphs cross it (here every
/// glyph but the hyphen: the underline is moved into their x-height), and `none` does not.
#[test]
fn skip_ink_cuts_the_underline_of_vertical_text() {
    let img = render(
        "font-size:40px;line-height:40px;color:transparent;text-decoration:underline #000;\
         text-underline-offset:-0.25em;text-decoration-thickness:3px",
        "<div style='left:100px;writing-mode:vertical-rl'>xqx-xqx</div>\
         <div style='left:200px;writing-mode:vertical-rl;text-decoration-skip-ink:none'>xqx-xqx</div>\
         <div style='left:500px;writing-mode:vertical-lr'>xqx-xqx</div>",
        1.0,
    );
    // Chrome: what is left of the underline is beside the hyphen (rows 85 to 98).
    for x in [108, 508] {
        let rows = inked_rows(&img, x, 0, 400);
        assert_eq!(rows.len(), 1, "column {x}: one piece left: {rows:?}");
        assert!(rows[0].0 >= 80 && rows[0].1 <= 104, "column {x}: beside the hyphen: {rows:?}");
    }
    assert_eq!(inked_rows(&img, 208, 0, 400), vec![(20, 165)], "skip-ink: none");
}

/// A `double` line's second line goes away from the text, also where the underline and overline trade sides
/// (Japanese, `text-underline-position: right`): the two lines of each, as Chrome draws them, 2px thick, 1px apart.
#[test]
fn double_vertical_decorations_go_away_from_the_text() {
    let cases: [(&str, [(f64, f64); 2]); 6] = [
        // Under the em box: the second line further left, away from the text.
        ("writing-mode:vertical-rl", [(94.0, 96.0), (97.0, 99.0)]),
        // Over it: the second line further right.
        ("writing-mode:vertical-rl' lang='ja", [(201.0, 203.0), (204.0, 206.0)]),
        ("writing-mode:vertical-rl;text-decoration-line:overline", [(282.0, 284.0), (285.0, 287.0)]),
        // An overline under the em box (`right`): the second line further left.
        (
            "writing-mode:vertical-rl;text-decoration-line:overline;text-underline-position:right",
            [(334.0, 336.0), (337.0, 339.0)],
        ),
        ("writing-mode:vertical-lr;text-underline-position:right", [(521.0, 523.0), (524.0, 526.0)]),
        ("writing-mode:vertical-lr;text-decoration-line:overline' lang='ja", [(574.0, 576.0), (577.0, 579.0)]),
    ];
    let lefts = [100, 180, 260, 340, 500, 580];
    let body: String = cases
        .iter()
        .zip(lefts)
        .map(|((css, _), left)| format!("<div style='left:{left}px;{css}'>MONITOR</div>"))
        .collect();
    let img = render(
        "font-size:20px;line-height:20px;color:transparent;text-decoration:underline double #000;\
         text-decoration-skip-ink:none;text-decoration-thickness:2px",
        &body,
        1.0,
    );
    for ((css, chrome), left) in cases.iter().zip(lefts) {
        let got = inked_runs(&img, 60, left - 30, left + 50, 1.0);
        assert_eq!(got, chrome.to_vec(), "{css}: Chrome draws it at {chrome:?}");
    }
}

/// A run a fallback font draws is decorated from the style's first available font, as Blink places decorations
/// (`PrimaryFont()`): "x" falls back to Ahem (0.8 + 0.2 em, 4px shorter than Inter at 20px), and each line's
/// overline and underline are where Chrome draws them, whichever font ends the line, as for text all in Inter.
#[test]
fn vertical_decorations_of_a_fallback_font_follow_the_first_font() {
    let css = "@font-face { font-family: P; src: url(inter.ttf); unicode-range: U+20-5A }
               @font-face { font-family: A; src: url(ahem.ttf) }";
    // Two 40px lines each, the first on the right.
    let cases: [(&str, &str, [(f64, f64); 2]); 4] = [
        ("overline", "MONxxx<br>xxxMON", [(132.0, 134.0), (172.0, 174.0)]),
        ("underline", "MONxxx<br>xxxMON", [(227.0, 229.0), (267.0, 269.0)]),
        ("overline", "MONMON<br>MONMON", [(372.0, 374.0), (412.0, 414.0)]),
        ("underline", "MONMON<br>MONMON", [(467.0, 469.0), (507.0, 509.0)]),
    ];
    let body: String = cases
        .iter()
        .enumerate()
        .map(|(i, (line, text, _))| {
            format!("<div style='left:{}px;text-decoration-line:{line}'>{text}</div>", 100 + 120 * i)
        })
        .collect();
    let img = render_with(
        css,
        "writing-mode:vertical-rl;font-family:P,A;font-size:20px;line-height:40px;color:transparent;\
         text-decoration:underline #000;text-decoration-skip-ink:none;text-decoration-thickness:2px",
        &body,
        1.0,
    );
    for (i, (line, text, chrome)) in cases.iter().enumerate() {
        let left = 100 + 120 * i;
        for y in [40, 110] {
            let got = inked_runs(&img, y, left - 20, left + 100, 1.0);
            assert_eq!(got, chrome.to_vec(), "{line} of {text:?} at row {y}: Chrome draws it at {chrome:?}");
        }
    }
}

/// `text-emphasis-position: over right` (the initial value) puts the marks on the right of a vertical line, which in
/// `sideways-lr` is under it (its line-over side is on the left), and `left` on the left. Transparent text with black
/// marks on 60px lines: the marks are on the side of the line's middle (x + 30) where Chrome draws them (columns 42 to
/// 48 of the line; 11 to 17 for `left`). pyro draws them 2 or 3 px further out, in `sideways-rl` too.
#[test]
fn emphasis_marks_of_sideways_lr_text() {
    let cases: [(&str, bool); 4] = [
        ("writing-mode:sideways-lr", true),
        ("writing-mode:sideways-lr;text-emphasis-position:over left", false),
        ("writing-mode:sideways-rl", true),
        ("writing-mode:vertical-rl;text-orientation:sideways", true),
    ];
    let body: String = cases
        .iter()
        .enumerate()
        .map(|(i, (css, _))| format!("<div style='left:{}px;{css}'>MONITOR</div>", 100 + 100 * i))
        .collect();
    let img = render("font-size:20px;line-height:60px;color:transparent;text-emphasis:'M' #000", &body, 1.0);
    for (i, (css, right)) in cases.iter().enumerate() {
        let left = 100 + 100 * i;
        let columns: Vec<usize> = (left..left + 60).filter(|&x| !inked_rows(&img, x, 0, 400).is_empty()).collect();
        let middle = left + 30;
        let on_side = |x: &usize| if *right { *x > middle + 5 } else { *x + 5 < middle };
        assert!(
            !columns.is_empty() && columns.iter().all(on_side),
            "{css}: marks in columns {columns:?}, Chrome draws them on the {} of {middle}",
            if *right { "right" } else { "left" }
        );
    }
}
