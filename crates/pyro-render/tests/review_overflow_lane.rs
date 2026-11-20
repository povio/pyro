//! Round-6 overflow lane review: boxes a clipping stacking context paints whose containing block
//! lies outside it keep their place in its paint order (only the clip differs), in every phase;
//! line clamps nest, discard what follows them for paint, hit testing and scrollable overflow,
//! and `line-clamp: auto` fits the used block size.
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

/// `body` on white, 400px square; `.a` is 20px Ahem on 20px lines.
fn render(body: &str) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(AhemFetch);
    let snapshot = Snapshot {
        html: format!(
            "<!doctype html><style>@font-face {{ font-family: A; src: url(ahem.ttf) }}
             body{{margin:0;background:white}} .a{{font:20px/20px A;color:black}}</style>{body}"
        ),
        base_url: "http://test/".into(),
        width: 400,
        height: 400,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    let mut renderer = Renderer::new();
    renderer.text_antialiasing = false;
    renderer.render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

fn pixel(img: &RgbaImage, x: u32, y: u32) -> [u8; 3] {
    let i = (y as usize * img.width as usize + x as usize) * 4;
    [img.data[i], img.data[i + 1], img.data[i + 2]]
}

const GREEN: [u8; 3] = [0, 128, 0];
const RED: [u8; 3] = [255, 0, 0];
const BLUE: [u8; 3] = [0, 0, 255];
const SILVER: [u8; 3] = [192, 192, 192];
const WHITE: [u8; 3] = [255, 255, 255];
const BLACK: [u8; 3] = [0, 0, 0];

/// A static clipping stacking context at (0, 0), 100px square, inside a 300px containing block.
fn clipping_context(children: &str) -> String {
    format!(
        "<div style='position:relative;width:300px;height:300px'>\
         <div style='isolation:isolate;overflow:hidden;width:100px;height:100px;background:silver'>{children}</div></div>"
    )
}

#[test]
fn a_box_escaping_the_clip_keeps_its_z_order() {
    // `a` (z-index 1) is contained outside the clip, `b` (z-index 2) inside: `b` stays on top
    // where they overlap, clipped; `a` shows past the clip.
    let img = render(&clipping_context(
        "<div style='position:absolute;left:50px;top:50px;width:100px;height:100px;background:red;z-index:1'></div>\
         <div style='position:relative;left:50px;top:50px;width:100px;height:100px;background:rgb(0,128,0);z-index:2'></div>",
    ));
    assert_eq!(pixel(&img, 75, 75), GREEN, "z-index 2 over z-index 1 inside the clip");
    assert_eq!(pixel(&img, 125, 125), RED, "the escaping box past the clip");
    assert_eq!(pixel(&img, 25, 25), SILVER, "the context's own background");
}

#[test]
fn a_negative_z_index_box_escapes_the_clip_too() {
    let img = render(&clipping_context(
        "<div style='position:absolute;left:50px;top:50px;width:100px;height:100px;background:blue;z-index:-1'></div>",
    ));
    assert_eq!(pixel(&img, 75, 75), BLUE, "over the context's background");
    assert_eq!(pixel(&img, 125, 125), BLUE, "past the clip");
}

/// Four 20px lines, each a 50px green block (atomic inlines, which a clamp hides with their line).
fn four_lines() -> &'static str {
    "<span style='display:inline-block;vertical-align:top;width:50px;height:20px;background:rgb(0,128,0)'></span><br>\
     <span style='display:inline-block;vertical-align:top;width:50px;height:20px;background:rgb(0,128,0)'></span><br>\
     <span style='display:inline-block;vertical-align:top;width:50px;height:20px;background:rgb(0,128,0)'></span><br>\
     <span style='display:inline-block;vertical-align:top;width:50px;height:20px;background:rgb(0,128,0)'></span>"
}

#[test]
fn an_inner_line_clamp_survives_an_outer_one() {
    let img = render(&format!(
        "<div class=a style='line-clamp:99'><div style='display:block;line-clamp:1'>{}</div></div>",
        four_lines()
    ));
    assert_eq!(pixel(&img, 25, 10), GREEN, "the inner clamp's line");
    assert_eq!(pixel(&img, 25, 30), WHITE, "its second line stays discarded");
}

#[test]
fn line_clamp_auto_fits_the_used_height() {
    // `height: 100px` within `max-height: 40px` is 40px: two lines.
    let img = render(&format!(
        "<div class=a style='line-clamp:auto;height:100px;max-height:40px'>{}</div>",
        four_lines()
    ));
    assert_eq!(pixel(&img, 25, 30), GREEN, "the second line");
    assert_eq!(pixel(&img, 25, 50), WHITE, "the third line is discarded");
}

#[test]
fn a_right_to_left_clamp_line_drops_the_word_that_does_not_fit_beside_the_ellipsis() {
    // "XX XX XXXX" right to left in 100px: the first line is "XX XX"; the second word does not
    // fit beside the ellipsis, so it goes whole: "XX" at the right, the ellipsis before it.
    let img = render(
        "<div class=a style='width:100px;line-clamp:1;direction:rtl;unicode-bidi:bidi-override'>XX XX XXXX</div>",
    );
    assert_eq!(pixel(&img, 90, 10), BLACK, "the first word");
    assert_eq!(pixel(&img, 30, 10), WHITE, "nothing of the second word");
}

#[test]
fn an_atomic_inline_does_not_turn_the_clamp_ellipsis_into_a_character_cut() {
    // A 20px box, "X XX XXXX" in 100px: the first line is [box]"X XX"; "XX" goes whole.
    let img = render(
        "<div class=a style='width:100px;line-clamp:1'><span style='display:inline-block;width:20px;height:20px;background:blue'></span>X XX XXXX</div>",
    );
    assert_eq!(pixel(&img, 10, 10), BLUE, "the box");
    assert_eq!(pixel(&img, 30, 10), BLACK, "the first word");
    assert_eq!(pixel(&img, 70, 10), WHITE, "nothing of the second word");
}
