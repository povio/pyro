//! `text-overflow: ellipsis` ends each line that overflows its clipping block
//! container with "…", as Chromium does: the atoms that do not fit beside it at
//! the content box's end edge are not drawn, and the ellipsis follows the last
//! one kept, in the block's own font and colour. Right-to-left lines end at the
//! left. A line that fits, or a box that does not clip, keeps its text.
use std::sync::Arc;

use pyro_render::*;

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}

/// Boxes 150px wide, each line 40px tall: I's stem fills rows 8-20 of a line,
/// an ellipsis's dots only the rows near the baseline (about 31).
const STYLE: &str = "<style>body{margin:0;background:white;font:bold 32px/40px Arial;color:black}
  .t{width:150px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
  .c{text-overflow:clip}.v{overflow:visible}</style>";
const LONG: &str = "IIIIIIIIIIIIIIIIIIII";

fn snapshot(body: &str) -> Snapshot {
    Snapshot {
        html: format!("<!doctype html>{STYLE}{body}"),
        base_url: "http://test/".into(),
        width: 400,
        height: 300,
        device_scale_factor: 1.0,
        ..Default::default()
    }
}

fn render(body: &str) -> RgbaImage {
    render_at(body, 1.0)
}

fn render_at(body: &str, device_scale_factor: f64) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let snapshot = Snapshot { device_scale_factor, ..snapshot(body) };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

/// The pixels in columns `x0..x1` and rows `y0..y1` that `keep` accepts: their
/// count and the largest and smallest x among them.
fn ink(img: &RgbaImage, x0: u32, x1: u32, y0: u32, y1: u32, keep: impl Fn([u8; 3]) -> bool) -> (usize, u32, u32) {
    let (mut n, mut min, mut max) = (0, u32::MAX, 0);
    for y in y0..y1 {
        for x in x0..x1 {
            let i = (y as usize * img.width as usize + x as usize) * 4;
            if keep([img.data[i], img.data[i + 1], img.data[i + 2]]) {
                n += 1;
                min = min.min(x);
                max = max.max(x);
            }
        }
    }
    (n, min, max)
}

fn dark(p: [u8; 3]) -> bool {
    p.iter().all(|&c| c < 128)
}

#[test]
fn an_overflowing_line_ends_with_an_ellipsis_after_the_last_atom_that_fits() {
    let img = render(&format!("<div class=t>{LONG}</div><div class='t c'>{LONG}</div>"));
    // Stems: the ellipsis line keeps whole I's, the clipped line runs to the box's edge.
    let (_, _, stems) = ink(&img, 0, 400, 8, 20, dark);
    let (_, _, clipped) = ink(&img, 0, 400, 48, 60, dark);
    assert!(clipped >= 145, "clipped at the edge: {clipped}");
    assert!(stems < 130, "no stem where the ellipsis goes: {stems}");
    // The dots, right of the last stem and within the box.
    let (dots, first, last) = ink(&img, stems + 1, 400, 24, 34, dark);
    assert!(dots > 10, "the ellipsis: {dots} dark pixels");
    // The gap is the I's and the ellipsis's side bearings.
    assert!(first <= stems + 8, "next to the last I kept: dots from {first}, stem at {stems}");
    assert!(last < 150, "within the box: {last}");
}

#[test]
fn a_line_that_fits_or_a_box_that_does_not_clip_keeps_its_text() {
    let fits = render("<div class=t>III</div>");
    let clip = render("<div class='t c'>III</div>");
    assert_eq!(fits.data, clip.data, "a line that fits is drawn as it is");
    let visible = render(&format!("<div class='t v'>{LONG}</div>"));
    let (_, _, right) = ink(&visible, 0, 400, 8, 20, dark);
    // Twenty I's are about 178px.
    assert!(right > 170, "overflow: visible draws the whole line: {right}");
}

#[test]
fn a_box_sized_to_its_text_keeps_it_at_a_fractional_position() {
    // Shrink-to-fit boxes as wide as their text, at fractional x: the box's painted (rounded)
    // edges must not make the text overflow it by a fraction of a pixel.
    for scale in [1.0, 2.0, 3.0] {
        for x in ["0.3px", "0.5px", "0.7px", "10.4px"] {
            let boxes = |overflow: &str| {
                format!(
                    "<div style='margin-left:{x};font:14px system-ui'><span class=t style='display:inline-block;width:auto;text-overflow:{overflow}'>EUR</span> \
                     <span class=t style='display:inline-block;width:auto;text-overflow:{overflow}'>Overview</span></div>"
                )
            };
            let ellipsis = render_at(&boxes("ellipsis"), scale);
            let clip = render_at(&boxes("clip"), scale);
            assert!(ellipsis.data == clip.data, "no ellipsis at scale {scale}, x {x}");
        }
    }
}

#[test]
fn a_right_to_left_line_ends_at_the_left() {
    let img = render(&format!("<div class=t dir=rtl>{LONG}</div>"));
    let (_, first_stem, _) = ink(&img, 0, 400, 8, 20, dark);
    assert!(first_stem > 20, "no stem where the ellipsis goes: {first_stem}");
    let (dots, _, last) = ink(&img, 0, first_stem, 24, 34, dark);
    assert!(dots > 10, "the ellipsis at the left: {dots}");
    assert!(last + 8 >= first_stem, "next to the last I kept: dots to {last}, stem at {first_stem}");
}

#[test]
fn the_ellipsis_has_the_block_containers_colour() {
    let img = render(&format!("<div class=t style=color:#0000ff><span style=color:#ff0000>{LONG}</span></div>"));
    let (_, _, stems) = ink(&img, 0, 400, 8, 20, |p| p == [255, 0, 0]);
    assert!(stems > 0, "red stems");
    assert!(ink(&img, stems + 1, 150, 24, 34, |p| p == [0, 0, 255]).0 > 0, "blue dots");
    assert_eq!(ink(&img, stems + 1, 150, 24, 34, |p| p == [255, 0, 0]).0, 0, "no red after the cut");
}

#[test]
fn a_printed_ellipsis_and_the_text_it_keeps_map_to_their_characters() {
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let html = format!("<div class=t>{LONG}</div>");
    let out =
        Renderer::new().print_pdf(&snapshot(&html), &fetcher, &PdfOptions::new(".pz-page", (816.0, 1056.0))).unwrap();
    assert!(out.stats.runs_with_source >= 2, "the line's run and the ellipsis: {:?}", out.stats);
    assert_eq!(out.stats.glyphs_without_text, 0, "{:?}", out.stats);
}

#[test]
fn a_string_value_ends_the_line_as_the_ellipsis_does() {
    let ellipsis = render(&format!("<div class=t>{LONG}</div>"));
    let string = render(&format!("<div class=t style='text-overflow:\"\\2026\"'>{LONG}</div>"));
    assert_eq!(ellipsis.data, string.data, "the string \"…\" is drawn as `ellipsis` is");
    // Line-breaking controls in the string are white space, collapsed: one line.
    let spaced = render(&format!("<div class=t style='text-overflow:\"I I\"'>{LONG}</div>"));
    let broken = render(&format!("<div class=t style='text-overflow:\"I\\A\\A\\2028 I\"'>{LONG}</div>"));
    assert_eq!(spaced.data, broken.data, "controls collapse to one space");
    assert_ne!(spaced.data, ellipsis.data, "the string is drawn");
    let (below, _, _) = ink(&broken, 0, 400, 40, 300, dark);
    assert_eq!(below, 0, "on the line it ends: {below} dark pixels below it");
}

#[test]
fn a_line_too_narrow_for_the_ellipsis_keeps_its_first_atom() {
    // 20px: an I (about 9px) fits, the ellipsis (32px) beside it does not.
    let img = render(&format!("<div class=t style='width:20px'>{LONG}</div>"));
    let (stem, _, last) = ink(&img, 0, 400, 8, 20, dark);
    assert!(stem > 0, "the first I is drawn");
    assert!(last < 10, "and only it: stems to {last}");
    let (dots, _, end) = ink(&img, last + 2, 400, 24, 34, dark);
    assert!(dots > 0 && end < 20, "the ellipsis follows it, clipped by the box: {dots} dark pixels to {end}");
}
