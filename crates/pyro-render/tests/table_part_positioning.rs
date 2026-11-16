//! Positioned table parts. A relatively positioned (or sticky) row or row group has no box of
//! its own: it moves its cells, paints as a positioned box (CSS 2.1 Appendix E step 8) with its
//! cells over its background, and is the containing block of the absolutely positioned boxes in
//! it. A percentage `top` resolves against the length height of the part's parent, else is `auto`
//! (Chromium). A relatively positioned caption moves; a table's last baseline is its last row's
//! (css-align-3 § 9.1). Pixel checks as WPT's `css/css-position/position-relative-table-*`.
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

/// `body` on white at 600x300 CSS px; collapsed tables with cells holding 50px squares, a red
/// `.indicator` 50px square at its `top`/`left` in the positioned `.group`.
fn render(css: &str, body: &str) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(AhemFetch);
    let snapshot = Snapshot {
        html: format!(
            "<!doctype html><style>@font-face {{ font-family: A; src: url(ahem.ttf) }}
             html, body {{ margin: 0; background: #fff }}
             table {{ border-collapse: collapse }} td {{ padding: 0 }} td > div {{ width: 50px; height: 50px }}
             .group {{ position: relative; width: 150px; height: 200px }}
             .indicator {{ position: absolute; background: red; width: 50px; height: 50px }}
             .rel {{ position: relative; background: green }} {css}</style>{body}"
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

fn count(img: &RgbaImage, color: [u8; 3]) -> usize {
    img.data.chunks_exact(4).filter(|px| px[..3] == color).count()
}

const RED: [u8; 3] = [255, 0, 0];
const GREEN: [u8; 3] = [0, 128, 0];

#[test]
fn a_relatively_positioned_row_moves_its_cells_and_paints_over_earlier_positioned_boxes() {
    for (part, offset) in [("tr", "top: 100px"), ("tbody", "top: 100px"), ("tr", "left: 100px")] {
        let (row, group) = if part == "tr" { (" class=rel", "") } else { ("", " class=rel") };
        let indicator = if offset.starts_with("top") { "top: 100px; left: 0" } else { "top: 0; left: 100px" };
        let img = render(
            &format!(".rel {{ {offset} }}"),
            &format!(
                "<div class=group><div class=indicator style='{indicator}'></div><table><tbody{group}>
                 <tr{row}><td><div></div></td></tr></tbody></table></div>"
            ),
        );
        assert_eq!(count(&img, RED), 0, "{part} {offset}");
        assert_eq!(count(&img, GREEN), 2500, "{part} {offset}: the row's background, moved");
    }
}

#[test]
fn a_percentage_top_resolves_against_the_parents_given_height_only() {
    // A row at `top: 100%` in a row group 10px high (as given; it is 50px used) moves 10px; the
    // green square in it moves back 10px. A cell at `top: 100%` in a row of auto height stays.
    for body in [
        "<table><tbody style='height: 10px'><tr style='position: relative; top: 100%'><td>
         <div style='position: relative; top: -10px; background: green'></div></td></tr></tbody></table>",
        "<table><tr><td style='position: relative; top: 100%'><div style='background: green'></div></td></tr></table>",
    ] {
        let img = render("", body);
        assert_eq!(pixel(&img, 25, 2), GREEN, "{body}");
        assert_eq!(pixel(&img, 25, 48), GREEN, "{body}");
        assert_eq!(count(&img, GREEN), 2500, "{body}");
    }
}

#[test]
fn a_relatively_positioned_caption_moves() {
    let img = render(
        "caption { width: 50px; height: 50px; position: relative; top: 100px; background: green }",
        "<div class=group><div class=indicator style='top: 100px; left: 0'></div><table><caption></caption></table></div>",
    );
    assert_eq!(count(&img, RED), 0);
}

#[test]
fn a_positioned_row_group_is_the_containing_block_of_absolute_boxes_in_it() {
    // The row group (one empty row: 0px high) is moved 50px down; the box 50px below its top.
    let img = render(
        ".rel { top: 50px } .abs { position: absolute; top: 50px; background: green }",
        "<div class=group><div class=indicator style='top: 100px; left: 0'></div><table>
         <tbody class=rel><tr><td><div class=abs></div></td></tr></tbody></table></div>",
    );
    assert_eq!(count(&img, RED), 0);
}

#[test]
fn an_inline_block_holding_a_table_sits_on_its_bottom_edge() {
    // A 50px row, then a row of 20px Ahem text (its baseline 66px down): the table gives the
    // inline-block no baseline, so the text beside it sits on its bottom edge (70px), as in
    // Chromium. (A `<div>`: a `<table>` start tag closes an open `<p>`.)
    let img = render(
        ".line { font: 20px/1 A; color: black } .ib { display: inline-block } .t { font: 20px/1 A; color: white }",
        "<div class=line><span class=ib><table><tr><td><div></div></td></tr><tr><td class=t>a</td></tr></table></span>X</div>",
    );
    assert_eq!(pixel(&img, 60, 72), [0, 0, 0], "the text is 16px above and 4px below the bottom edge");
    assert_eq!(pixel(&img, 60, 52), [255, 255, 255]);
}
