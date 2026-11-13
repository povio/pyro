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
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    Renderer::new().render(&snapshot(body), Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
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
    let out = Renderer::new().print_pdf(&snapshot(&html), &fetcher, &PdfOptions::new(".pz-page", (816.0, 1056.0))).unwrap();
    assert!(out.stats.runs_with_source >= 2, "the line's run and the ellipsis: {:?}", out.stats);
    assert_eq!(out.stats.glyphs_without_text, 0, "{:?}", out.stats);
}
