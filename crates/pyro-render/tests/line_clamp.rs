//! The legacy line clamp: `-webkit-line-clamp` on a `display: -webkit-box` box with
//! `-webkit-box-orient: vertical` (the form pages use) keeps that many lines of the box's
//! block formatting context, as Chromium does: an auto-height box ends at the last line kept,
//! which ends with "…", and the lines after it are not painted. A box with no more lines than
//! the clamp, or without `-webkit-box` and the vertical orientation, is laid out as it is.
use std::sync::Arc;

use pyro_render::*;

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}

/// Lines 40px tall on yellow; a green block follows the box.
const STYLE: &str = "<style>body{margin:0;background:white;font:bold 32px/40px Arial;color:black}
  .c{display:-webkit-box;-webkit-box-orient:vertical;-webkit-line-clamp:2;background:yellow;width:300px}
  .after{height:20px;width:300px;background:rgb(0,128,0)}</style>";

fn render(body: &str) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let snapshot = Snapshot {
        html: format!("<!doctype html>{STYLE}{body}<div class=after></div>"),
        base_url: "http://test/".into(),
        width: 400,
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

/// The first row of the green block that follows the box: the box's height.
fn box_height(img: &RgbaImage) -> u32 {
    (0..img.height).find(|&y| pixel(img, 290, y) == [0, 128, 0]).expect("the block after the box")
}

/// Dark pixels in columns `x0..x1` of rows `y0..y1`.
fn ink(img: &RgbaImage, x0: u32, x1: u32, y0: u32, y1: u32) -> usize {
    (y0..y1).flat_map(|y| (x0..x1).map(move |x| (x, y))).filter(|&(x, y)| pixel(img, x, y).iter().all(|&c| c < 128)).count()
}

#[test]
fn a_clamped_box_ends_at_its_last_line_kept_which_ends_with_an_ellipsis() {
    let img = render("<div class=c>I<br>I<br>I<br>I</div>");
    assert_eq!(box_height(&img), 80, "two lines of 40px");
    // The I's stem is left of x = 12; the ellipsis follows it on the second line only.
    assert!(ink(&img, 0, 12, 48, 60) > 0, "the second line's I");
    assert!(ink(&img, 12, 60, 64, 76) > 10, "the ellipsis after it");
    assert_eq!(ink(&img, 12, 60, 24, 36), 0, "none on the first line");
    assert_eq!(ink(&img, 0, 300, 100, 300), 0, "the lines after the clamp are not painted");
}

#[test]
fn a_box_with_no_more_lines_than_the_clamp_keeps_them() {
    let clamped = render("<div class=c>I<br>I</div>");
    let plain = render("<div class=c style='-webkit-line-clamp:none'>I<br>I</div>");
    assert_eq!(clamped.data, plain.data);
}

#[test]
fn the_clamp_needs_the_legacy_box_and_its_vertical_orientation() {
    let block = render("<div class=c style='display:block'>I<br>I<br>I<br>I</div>");
    assert_eq!(box_height(&block), 160, "display: block: four lines");
    assert_eq!(ink(&block, 12, 60, 64, 76), 0, "no ellipsis");
    let horizontal = render("<div class=c style='-webkit-box-orient:horizontal'>I<br>I<br>I<br>I</div>");
    assert_eq!(box_height(&horizontal), 160, "horizontal: four lines");
}

#[test]
fn lines_of_block_descendants_count_and_those_of_other_formatting_contexts_do_not() {
    // Line 2 is the first line of the nested block.
    let nested = render("<div class=c><div>I<div>I<br>I</div>I</div></div>");
    assert_eq!(box_height(&nested), 80);
    assert!(ink(&nested, 12, 60, 64, 76) > 10, "the ellipsis ends the nested block's first line");
    // A scroll container's lines are skipped: the clamp is at the second line outside it.
    let skipped = render("<div class=c><div>I<div style='overflow:hidden'>I<br>I</div>I<br>I</div></div>");
    assert_eq!(box_height(&skipped), 160, "line, two skipped lines, the clamp line");
    assert!(ink(&skipped, 12, 60, 144, 156) > 10, "the ellipsis ends the fourth line drawn");
}

#[test]
fn a_clamp_set_later_applies_and_one_removed_goes() {
    // The same box laid out with and without the clamp gives each its own layout.
    let on = render("<div class=c>I<br>I<br>I</div>");
    let off = render("<div class=c style='-webkit-line-clamp:none'>I<br>I<br>I</div>");
    assert_eq!(box_height(&on), 80);
    assert_eq!(box_height(&off), 120);
}
