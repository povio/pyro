//! An empty `<input>` or `<textarea>` paints its `placeholder` in its
//! `::placeholder` style, as Chromium does: its UA colour is #757575, the text
//! sits where the value's would (a single line centred in the content box)
//! and follows `text-align`; a textarea's wraps; any value hides it.
use std::sync::Arc;

use pyro_render::*;

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}

/// Inputs 300x60 (10px side padding, no border), each 80px below the last.
const STYLE: &str = "<style>body{margin:0;background:white}
  input,textarea{display:block;box-sizing:border-box;width:300px;height:60px;margin:0 0 20px;border:0;
  padding:0 10px;font:bold 32px Arial;background:white;color:black;outline:none}</style>";

fn render(body: &str) -> RgbaImage {
    let s = Snapshot {
        html: format!("<!doctype html>{STYLE}{body}"),
        base_url: "http://test/".into(),
        width: 400,
        height: 400,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    Renderer::new().render(&s, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

/// The pixels in rows `y0..y1` that `keep` accepts: their count and bounds
/// (`[x0, y0, x1, y1]`, inclusive).
fn ink(img: &RgbaImage, y0: u32, y1: u32, keep: impl Fn([u8; 3]) -> bool) -> (usize, [u32; 4]) {
    let (mut n, mut b) = (0, [u32::MAX, u32::MAX, 0, 0]);
    for y in y0..y1 {
        for x in 0..img.width {
            let i = (y as usize * img.width as usize + x as usize) * 4;
            if keep([img.data[i], img.data[i + 1], img.data[i + 2]]) {
                n += 1;
                b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
            }
        }
    }
    (n, b)
}

const GREY: [u8; 3] = [0x75, 0x75, 0x75];

#[test]
fn an_empty_input_paints_its_placeholder_where_its_value_would_be() {
    let img = render(
        "<input placeholder=IIII><input value=IIII style=color:#757575><input placeholder=IIII value=x>
         <style>.red::placeholder{color:#ff0000}</style><input class=red placeholder=IIII>",
    );
    let (grey, placeholder) = ink(&img, 0, 60, |p| p == GREY);
    assert!(grey > 100, "the UA placeholder colour, #757575: {grey} pixels");
    // The value's glyphs in that colour, 80px lower: the same box, the same place.
    let (_, value) = ink(&img, 80, 140, |p| p == GREY);
    assert_eq!(placeholder, [value[0], value[1] - 80, value[2], value[3] - 80], "placed as the value");
    assert!((10..14).contains(&placeholder[0]), "at the content box's start: {placeholder:?}");
    assert_eq!(ink(&img, 160, 220, |p| p == GREY).0, 0, "a value hides it");
    assert!(ink(&img, 160, 220, |p| p == [0, 0, 0]).0 > 0, "the value is painted");
    assert!(ink(&img, 240, 300, |p| p == [255, 0, 0]).0 > 100, "its ::placeholder colour");
}

#[test]
fn a_placeholder_follows_text_align_opacity_and_loses_its_line_breaks() {
    let img = render(
        "<style>.end{text-align:right}.half::placeholder{color:black;opacity:.5}</style>
         <input class=end placeholder=IIII><input class=half placeholder=IIII><input placeholder='II&#10;II'>",
    );
    let (_, end) = ink(&img, 0, 60, |p| p == GREY);
    let (_, start) = ink(&render("<input placeholder=IIII>"), 0, 60, |p| p == GREY);
    // I's side bearings are equal: as far from the content box's end (x 290) as from its start.
    assert_eq!(290 - (end[2] + 1), start[0] - 10, "right-aligned: {end:?}, left-aligned {start:?}");
    let (half, _) = ink(&img, 80, 140, |p| p.iter().all(|&c| (126..=128).contains(&c)));
    assert!(half > 100, "black at opacity .5: {half} grey pixels");
    let (_, joined) = ink(&img, 160, 220, |p| p == GREY);
    let one = start;
    assert_eq!(joined[3] - joined[1], one[3] - one[1], "one line, as tall as IIII");
    assert_eq!(joined[2] - joined[0], one[2] - one[0], "as wide as IIII");
}

#[test]
fn a_textarea_placeholder_wraps_at_its_width() {
    // Two words (about 80px) to a line of the 100px content box.
    let img =
        render("<textarea placeholder='IIII IIII IIII IIII IIII IIII' style='width:120px;height:200px'></textarea>");
    let (n, b) = ink(&img, 0, 200, |p| p == GREY);
    assert!(n > 100, "{n}");
    assert!(b[3] - b[1] > 64, "three lines of 32px text: {b:?}");
    assert!(b[2] < 110, "within the content box: {b:?}");
}

/// A number field is a text field: as wide and tall as a text input, with its placeholder
/// (WPT css-pseudo `placeholder-input-number`).
#[test]
fn a_number_input_is_laid_out_and_painted_as_a_text_field() {
    let field = |ty: &str| {
        render(&format!(
            "<style>input{{all:revert;font:16px Arial}} input::placeholder{{color:#ff0000}}</style><input type={ty} placeholder=IIII>"
        ))
    };
    let red = |c: [u8; 3]| c[0] > 200 && c[1] < 80 && c[2] < 80;
    let (text, number) = (ink(&field("text"), 0, 400, red), ink(&field("number"), 0, 400, red));
    assert!(number.0 > 0, "the placeholder is painted");
    assert_eq!(number, text);
}
