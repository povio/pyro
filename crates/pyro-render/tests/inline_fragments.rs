//! Boxes and decorations broken across lines are painted as Chromium paints their fragments:
//! `text-decoration-inset` applies at a box's ends (`box-decoration-break: slice`, the initial
//! value) or at each fragment's (`clone`, also as `-webkit-box-decoration-break`), its
//! percentages of the whole box, padding included; white space hanging at a line's end is not
//! decorated; a decoration casts the shadow of each run it is under and an inner box's line goes
//! over an outer box's; an inline box with `background-clip: text` shows its background in its own
//! glyphs; and a sliced inline box casts the shadows and shows the background images of the whole
//! box, cut at its fragments' open sides.
use std::sync::Arc;

use pyro_render::*;

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}

/// `0` is 1ch wide in any font: widths in `ch` make the lines break where the tests say. Each
/// line is 40px tall.
const STYLE: &str = "<!doctype html><style>body{margin:0;background:white;font:20px/40px monospace;color:black}
  p{margin:0 0 0 40px;width:6ch}</style>";

fn render(body: &str) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let snapshot = Snapshot {
        html: format!("{STYLE}{body}"),
        base_url: "http://test/".into(),
        width: 400,
        height: 200,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

fn pixel(img: &RgbaImage, x: u32, y: u32) -> [u8; 3] {
    let i = (y as usize * img.width as usize + x as usize) * 4;
    [img.data[i], img.data[i + 1], img.data[i + 2]]
}

/// The x extent (start, exclusive end) of the pixels in rows `y0..y1` that `keep` accepts.
fn extent(img: &RgbaImage, y0: u32, y1: u32, keep: impl Fn([u8; 3]) -> bool) -> Option<(u32, u32)> {
    let mut e: Option<(u32, u32)> = None;
    for y in y0..y1.min(img.height) {
        for x in 0..img.width {
            if keep(pixel(img, x, y)) {
                let e = e.get_or_insert((x, x + 1));
                *e = (e.0.min(x), e.1.max(x + 1));
            }
        }
    }
    e
}

fn blue(p: [u8; 3]) -> bool {
    p[2] > 200 && p[0] < 100 && p[1] < 100
}

fn red(p: [u8; 3]) -> bool {
    p[0] > 200 && p[1] < 100 && p[2] < 100
}

/// The extent of a blue underline on each of the two lines of `<u>0000 0000</u>` (transparent
/// glyphs), with `style` on the `u`.
fn underline(style: &str) -> [(u32, u32); 2] {
    let img = render(&format!(
        "<p><u style='color:transparent;text-decoration-color:#00f;text-underline-offset:6px;{style}'>0000 0000</u></p>"
    ));
    [0, 1].map(|line| extent(&img, line * 40, line * 40 + 40, blue).unwrap_or_else(|| panic!("{style}: line {line}")))
}

fn near(a: u32, b: f64) -> bool {
    (a as f64 - b).abs() <= 1.0
}

/// The underline of a wrapped box ends at its text on each line, not under the space the line
/// breaks after (it hangs, and Chromium removes it): "0000" is 4ch on both lines.
#[test]
fn hanging_white_space_is_not_decorated() {
    let [first, second] = underline("");
    let (w1, w2) = (first.1 - first.0, second.1 - second.0);
    assert!(w1.abs_diff(w2) <= 1, "{first:?} {second:?}");
    assert_eq!(first.0, second.0, "both lines start at the box's left: {first:?} {second:?}");
}

/// With `box-decoration-break: slice` (the initial value) an inset applies at the start of the
/// box's first fragment and the end of its last, not at the line break; with `clone` at both
/// ends of every fragment, as `-webkit-box-decoration-break: clone` says too.
#[test]
fn an_inset_applies_at_the_ends_of_the_box_or_of_each_cloned_fragment() {
    let [a, b] = underline("");
    let ch = (a.1 - a.0) as f64 / 4.0;
    let [s1, s2] = underline("text-decoration-inset:-1ch");
    assert!(near(s1.0, a.0 as f64 - ch) && s1.1 == a.1, "slice, first line: {s1:?} vs {a:?}");
    assert!(s2.0 == b.0 && near(s2.1, b.1 as f64 + ch), "slice, last line: {s2:?} vs {b:?}");
    let clone = underline("text-decoration-inset:-1ch;box-decoration-break:clone");
    for (line, (c, plain)) in clone.iter().zip([a, b]).enumerate() {
        assert!(
            near(c.0, plain.0 as f64 - ch) && near(c.1, plain.1 as f64 + ch),
            "clone, line {line}: {c:?} vs {plain:?}"
        );
    }
    assert_eq!(underline("text-decoration-inset:-1ch;-webkit-box-decoration-break:clone"), clone);
}

/// A percentage inset of a sliced box is of its whole decoration line, over all its fragments
/// and its inline padding: 25% of two lines of 4ch is 2ch, cut at the box's start and end; with
/// 2ch of padding at each side, 25% of 12ch is 3ch.
#[test]
fn a_percentage_inset_is_of_the_whole_box() {
    let [a, b] = underline("");
    let ch = (a.1 - a.0) as f64 / 4.0;
    let [p1, p2] = underline("text-decoration-inset:25%");
    assert!(near(p1.0, a.0 as f64 + 2.0 * ch) && p1.1 == a.1, "{p1:?} vs {a:?}");
    assert!(p2.0 == b.0 && near(p2.1, b.1 as f64 - 2.0 * ch), "{p2:?} vs {b:?}");

    let img = render(
        "<div style='margin-left:40px'><u style='padding:0 2ch;color:transparent;text-decoration-color:#00f;\
         text-underline-offset:6px;text-decoration-inset:25%'>0000<b style='font-weight:normal'>0000</b></u></div>",
    );
    let line = extent(&img, 0, 40, blue).unwrap();
    // The text starts after 2ch of padding; the line starts 3ch into it and ends 3ch before its end.
    assert!(near(line.1 - line.0, 2.0 * ch), "{line:?}, ch {ch}");
}

/// `text-decoration-inset: auto` insets each end by a twelfth and a half of the font size (at
/// least 1px): a line two adjacent boxes draw is broken between them.
#[test]
fn an_auto_inset_separates_adjacent_lines() {
    let page = |inset: &str| {
        render(&format!(
            "<div style='margin-left:40px'><u style='color:transparent;text-decoration-color:#00f;\
             text-underline-offset:6px;text-decoration-inset:{inset}'>00</u><u style='color:transparent;\
             text-decoration-color:#00f;text-underline-offset:6px;text-decoration-inset:{inset}'>00</u></div>"
        ))
    };
    let row = |img: &RgbaImage| {
        let (x0, x1) = extent(img, 0, 40, blue).unwrap();
        let y = (0..40).find(|&y| blue(pixel(img, x0, y))).unwrap();
        (x0..x1).filter(|&x| !blue(pixel(img, x, y))).count()
    };
    assert_eq!(row(&page("0")), 0, "one line");
    assert!(row(&page("auto")) >= 2, "a gap between the boxes' lines");
}

/// Each run a decoration is under casts its part of the line's shadow, in that run's shadow
/// colour (`currentColor`: the run's colour), and an inner box's line goes over an outer box's.
#[test]
fn a_decoration_casts_each_runs_shadow_and_inner_lines_go_on_top() {
    let img = render(
        "<div style='margin-left:40px;color:#00f;text-shadow:0 6px;text-decoration:underline;text-underline-offset:6px;\
         text-decoration-thickness:2px;text-decoration-skip-ink:none'><span style='color:transparent;text-shadow:0 6px #00f'>0000</span>\
         <span style='color:transparent;text-shadow:0 6px #f00'>0000</span></div>",
    );
    let line = extent(&img, 0, 40, blue).unwrap();
    // The shadow's rows: below the line by 6px.
    let y =
        (0..40).rev().find(|&y| (line.0..line.1).any(|x| blue(pixel(&img, x, y)) || red(pixel(&img, x, y)))).unwrap();
    let (left, right) = (pixel(&img, line.0 + 2, y), pixel(&img, line.1 - 3, y));
    assert!(blue(left) && red(right), "the shadow under each run is its colour: {left:?} {right:?}");

    // Two lines at the same place: the inner box's is seen.
    let img = render(
        "<div style='margin-left:40px'><u style='color:transparent;text-decoration-color:#00f;text-decoration-skip-ink:none'>00\
         <u style='text-decoration-color:#f00'>00</u></u></div>",
    );
    let line = extent(&img, 0, 40, |p| blue(p) || red(p)).unwrap();
    let y = (0..40).find(|&y| (line.0..line.1).any(|x| blue(pixel(&img, x, y)) || red(pixel(&img, x, y)))).unwrap();
    assert!(red(pixel(&img, line.1 - 3, y)), "{:?}", pixel(&img, line.1 - 3, y));
    assert!(blue(pixel(&img, line.0 + 2, y)), "{:?}", pixel(&img, line.0 + 2, y));
}

/// An inline box with `background-clip: text` shows its background through its own glyphs, as
/// text of that colour would be drawn, on every line it is on.
#[test]
fn an_inline_box_clips_its_background_to_its_glyphs() {
    let clipped =
        render("<p><span style='background:#00f;background-clip:text;color:transparent'>0000 0000</span></p>");
    let drawn = render("<p><span style='color:#00f'>0000 0000</span></p>");
    let ink = |img: &RgbaImage, line: u32| {
        (line * 40..line * 40 + 40)
            .flat_map(|y| (0..img.width).map(move |x| (x, y)))
            .filter(|&(x, y)| pixel(img, x, y)[0] < 128)
            .count()
    };
    for line in [0, 1] {
        let (a, b) = (ink(&clipped, line), ink(&drawn, line));
        assert!(a > 0 && a.abs_diff(b) <= b / 50, "line {line}: {a} px of background, {b} of text");
    }
    // Not the box: where the text leaves the page white, so does the background.
    let (w, h) = (clipped.width, clipped.height);
    let outside = (0..h)
        .flat_map(|y| (0..w).map(move |x| (x, y)))
        .filter(|&(x, y)| pixel(&drawn, x, y) == [255, 255, 255] && pixel(&clipped, x, y)[0] < 200)
        .count();
    assert_eq!(outside, 0);
}

/// A box sliced across lines casts the shadow of the whole box, its fragments side by side: the
/// first fragment's shadow stops at its open right side, the last fragment's reaches past its
/// closed one.
#[test]
fn a_sliced_box_casts_the_whole_boxs_shadow() {
    let img = render("<p><span style='box-shadow:8px 0 #f00;color:transparent'>0000 0000</span></p>");
    let (first, last) = (extent(&img, 0, 40, red), extent(&img, 40, 80, red));
    let ch = render("<p><span style='background:#f00;color:transparent'>0000 0000</span></p>");
    let (box1, box2) = (extent(&ch, 0, 40, red).unwrap(), extent(&ch, 40, 80, red).unwrap());
    assert_eq!(first, None, "no shadow right of the first fragment's open side: {box1:?}");
    let last = last.unwrap();
    assert_eq!(last, (box2.1, box2.1 + 8), "the last fragment's shadow past its right side");
}

/// The background image of a sliced box runs on from one fragment into the next: the gradient
/// is the whole box's.
#[test]
fn a_sliced_box_shows_the_whole_boxs_background_image() {
    let img = render(
        "<p><span style='background:linear-gradient(to right,#000,#fff);color:transparent'>0000 0000</span></p>",
    );
    let (first, second) = (extent(&img, 0, 40, |p| p[0] < 250).unwrap(), extent(&img, 40, 80, |p| p[0] < 250).unwrap());
    let y = 20;
    let (end_of_first, start_of_second) = (pixel(&img, first.1 - 1, y)[0], pixel(&img, second.0, y + 40)[0]);
    assert!(start_of_second > 100, "the second fragment starts mid-gradient: {start_of_second}");
    assert!(end_of_first.abs_diff(start_of_second) < 12, "{end_of_first} then {start_of_second}");
}
