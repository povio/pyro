//! CSS Multi-column Layout (blitz-dom `layout::multicol`, blitz-paint `render/multicol.rs`): a
//! multi-column container's content is laid out in one column-wide strip, broken into columns
//! (balanced, or filled to a definite height with `column-fill: auto`) and painted once per
//! column, a box split by a break sliced. Column rules are painted between columns with content.
//! Pixel checks as in WPT's `slice-block-fragmentation-*`, `clip-path-columns-shape-*` and
//! `clip-text-fragmentation` reftests.
use std::sync::Arc;

use pyro_render::*;

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}

fn render(body: &str, scale: f64) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let snapshot = Snapshot {
        html: format!("<!doctype html><body style='margin:0;background:#fff'>{body}"),
        base_url: "http://test/".into(),
        width: 500,
        height: 300,
        device_scale_factor: scale,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

/// The pixel at CSS px (x, y) (the device pixel at its top-left corner, plus half a pixel).
fn pixel(img: &RgbaImage, scale: f64, x: f64, y: f64) -> [u8; 3] {
    let (x, y) = ((x * scale + 0.5) as usize, (y * scale + 0.5) as usize);
    let i = (y * img.width as usize + x) * 4;
    [img.data[i], img.data[i + 1], img.data[i + 2]]
}

const WHITE: [u8; 3] = [255, 255, 255];
const GREEN: [u8; 3] = [0, 128, 0];
const BLUE: [u8; 3] = [0, 0, 255];
const BLACK: [u8; 3] = [0, 0, 0];

#[test]
fn fractional_column_translation_uses_device_pixels() {
    let image = render(include_str!("fixtures/fractional-column-offset.html"), 3.0);
    assert_eq!(pixel(&image, 3.0, 30.0, 7.0 / 3.0), GREEN);
    assert_eq!(pixel(&image, 3.0, 30.0, 8.0 / 3.0), BLUE);
}

fn check(img: &RgbaImage, scale: f64, points: &[(f64, f64, [u8; 3])], what: &str) {
    let wrong: Vec<String> = points
        .iter()
        .filter(|&&(x, y, want)| pixel(img, scale, x, y) != want)
        .map(|&(x, y, want)| format!("({x}, {y}): {:?}, want {want:?}", pixel(img, scale, x, y)))
        .collect();
    assert!(wrong.is_empty(), "{what} at scale {scale}:\n{}", wrong.join("\n"));
}

/// A tall box in three columns of a definite height (`column-fill: auto`) is cut where each
/// column ends; the columns sit side by side, a gap apart, and nothing is left below them.
#[test]
fn a_tall_box_is_split_into_columns_side_by_side() {
    for scale in [1.0, 2.0] {
        let img = render(
            "<div style='columns:3;column-fill:auto;gap:20px;width:340px;height:100px'>\
             <div style='height:300px;background:green'></div></div>",
            scale,
        );
        check(
            &img,
            scale,
            &[
                (50.0, 50.0, GREEN),
                (110.0, 50.0, WHITE),
                (170.0, 50.0, GREEN),
                (230.0, 50.0, WHITE),
                (290.0, 99.0, GREEN),
                (50.0, 150.0, WHITE),
                (50.0, 250.0, WHITE),
            ],
            "three columns",
        );
    }
}

/// Columns are balanced: two 50px blocks in two columns of an auto height make one each.
#[test]
fn columns_are_balanced() {
    for scale in [1.0, 2.0] {
        let img = render(
            "<div style='columns:2;column-gap:0;width:200px'>\
             <div style='height:50px;background:blue'></div><div style='height:50px;background:green'></div></div>\
             <div style='height:10px;background:black'></div>",
            scale,
        );
        check(
            &img,
            scale,
            &[(50.0, 25.0, BLUE), (150.0, 25.0, GREEN), (50.0, 55.0, BLACK), (150.0, 75.0, WHITE)],
            "balanced columns",
        );
    }
}

/// A box shadow of a box split over columns is sliced with it (`box-decoration-break: slice`):
/// the shadow's right edge shows in every column, its bottom edge only in the last, its top edge
/// only in the first (WPT `slice-block-fragmentation-001`/`-002`).
#[test]
fn a_box_shadow_is_sliced_at_column_breaks() {
    for scale in [1.0, 2.0] {
        let img = render(
            "<div style='margin:50px;columns:3;column-fill:auto;gap:20px;width:340px;height:100px'>\
             <div style='box-shadow:5px 5px black;height:300px'></div></div>",
            scale,
        );
        check(
            &img,
            scale,
            &[
                // First column: the right edge from 5px down, no bottom edge.
                (152.0, 52.0, WHITE),
                (152.0, 60.0, BLACK),
                (152.0, 149.0, BLACK),
                (100.0, 152.0, WHITE),
                // Second column: the right edge all the way.
                (272.0, 51.0, BLACK),
                (220.0, 152.0, WHITE),
                // Third column: the right edge and the bottom edge below the column.
                (392.0, 51.0, BLACK),
                (340.0, 152.0, BLACK),
            ],
            "sliced shadow",
        );
        let img = render(
            "<div style='margin:50px;columns:3;column-fill:auto;gap:20px;width:340px;height:100px'>\
             <div style='box-shadow:-5px -5px black;height:300px'></div></div>",
            scale,
        );
        check(
            &img,
            scale,
            &[(100.0, 47.0, BLACK), (220.0, 47.0, WHITE), (47.0, 100.0, BLACK), (167.0, 100.0, BLACK)],
            "sliced shadow up and left",
        );
    }
}

/// A clip path moves with the box into its column (WPT `clip-path-columns-shape-001`).
#[test]
fn a_clip_path_moves_with_its_box_into_the_column() {
    for scale in [1.0, 2.0] {
        let img = render(
            "<div style='columns:2;column-gap:0;width:200px;height:100px'><div style='height:100px'></div>\
             <div style='height:90px;background:green;border:5px solid red;clip-path:inset(5px)'></div></div>",
            scale,
        );
        check(
            &img,
            scale,
            &[
                (110.0, 10.0, GREEN),
                (190.0, 90.0, GREEN),
                (102.0, 50.0, WHITE),
                (150.0, 98.0, WHITE),
                (50.0, 150.0, WHITE),
            ],
            "clipped box in the second column",
        );
    }
}

/// A column rule is painted in the middle of the gap between two columns with content, as tall
/// as the columns, and not past the last column.
#[test]
fn column_rules_are_painted_between_columns() {
    for scale in [1.0, 2.0] {
        let img = render(
            "<div style='columns:3;column-fill:auto;column-gap:20px;width:340px;height:100px;\
             column-rule:4px solid blue'><div style='height:200px;background:green'></div></div>",
            scale,
        );
        check(
            &img,
            scale,
            &[
                (110.0, 50.0, BLUE),
                (107.0, 50.0, WHITE),
                (112.5, 50.0, WHITE),
                (110.0, 99.0, BLUE),
                (110.0, 101.0, WHITE),
                // The third column has no content: no rule before it.
                (230.0, 50.0, WHITE),
            ],
            "column rule",
        );
    }
}

/// `min-height` makes the container taller, not its content: the content is balanced as if the
/// container had none, and the columns are as tall as the container.
#[test]
fn min_height_is_not_balanced_as_content() {
    for scale in [1.0, 2.0] {
        let img = render(
            "<div style='columns:2;column-gap:0;width:200px;min-height:200px;column-rule:2px solid black'>\
             <div style='height:50px;background:blue'></div><div style='height:50px;background:green'></div></div>\
             <div style='height:10px;background:black'></div>",
            scale,
        );
        check(
            &img,
            scale,
            &[
                (50.0, 25.0, BLUE),
                (150.0, 25.0, GREEN),
                (50.0, 75.0, WHITE),
                (150.0, 75.0, WHITE),
                (50.0, 205.0, BLACK),
            ],
            "balanced under min-height",
        );
    }
}

/// A block multi-column container establishes a block formatting context: next to a float it is
/// narrowed and moved past it, as a `flow-root` is, instead of laying its columns under it.
#[test]
fn a_block_multicol_container_avoids_floats() {
    for scale in [1.0, 2.0] {
        let img = render(
            "<div style='float:left;width:100px;height:100px;background:black'></div>\
             <div style='columns:2;column-gap:20px'><div style='height:100px;background:green'></div></div>",
            scale,
        );
        check(
            &img,
            scale,
            &[
                (50.0, 50.0, BLACK),
                (150.0, 25.0, GREEN),
                (250.0, 25.0, GREEN),
                (300.0, 25.0, WHITE),
                (350.0, 25.0, GREEN),
                (490.0, 25.0, GREEN),
                (150.0, 75.0, WHITE),
            ],
            "multicol beside a float",
        );
    }
}

/// The column rules of a container that clips its overflow are clipped with its content: no
/// rule is drawn between the overflow columns past its inline end.
#[test]
fn column_rules_are_clipped_with_the_overflow() {
    for scale in [1.0, 2.0] {
        let img = render(
            "<div style='columns:2;column-fill:auto;column-gap:20px;width:200px;height:100px;overflow:hidden;\
             column-rule:4px solid blue'><div style='height:400px;background:green'></div></div>",
            scale,
        );
        check(
            &img,
            scale,
            &[
                (100.0, 50.0, BLUE),
                (50.0, 50.0, GREEN),
                (210.0, 50.0, WHITE),
                (250.0, 50.0, WHITE),
                (320.0, 50.0, WHITE),
            ],
            "clipped column rules",
        );
    }
}

/// A line of atomic inlines alone (no text, as markup without white space between boxes makes)
/// is a line box like any other: a break goes between lines, not through a box.
#[test]
fn a_line_of_boxes_alone_is_not_cut() {
    for scale in [1.0, 2.0] {
        let img = render(
            "<div style='columns:2;column-gap:0;width:200px;font-size:0;orphans:1;widows:1'>\
             <span style='display:inline-block;width:100px;height:100px;background:blue'></span>\
             <span style='display:inline-block;width:100px;height:100px;background:green'></span>\
             <span style='display:inline-block;width:100px;height:100px;background:black'></span></div>",
            scale,
        );
        check(
            &img,
            scale,
            &[
                (50.0, 50.0, BLUE),
                (50.0, 150.0, GREEN),
                (150.0, 50.0, BLACK),
                (150.0, 150.0, WHITE),
                (150.0, 99.0, BLACK),
            ],
            "lines of boxes",
        );
    }
}

/// Not a check: the time a long ordinary page (no columns, about 4,000 block boxes) takes to be
/// built, styled, laid out and painted in a small viewport, where layout is most of the work.
/// Run with `--ignored --nocapture` on two builds to compare them (under `/usr/bin/time -l` for
/// instructions).
#[test]
#[ignore]
fn ordinary_page_cost_probe() {
    let para = "Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore. ";
    let cards: String = (0..400)
        .map(|i| {
            format!(
                "<section style='margin:8px;padding:8px;border:1px solid #ccc'><h3>Card {i}</h3>\
                 <div><p>{para}</p><p>{para}{para}</p></div><ul><li>a</li><li>b</li><li>c</li></ul>\
                 <div style='display:flex;gap:4px'><div>x</div><div>y</div></div></section>"
            )
        })
        .collect();
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let mut renderer = Renderer::new();
    let mut times = Vec::new();
    for i in 0..12 {
        // A new document each time (the comment differs).
        let snapshot = Snapshot {
            html: format!("<!doctype html><!--{i}--><body style='margin:0;font:14px sans-serif'><main>{cards}</main>"),
            base_url: "http://test/".into(),
            width: 800,
            height: 600,
            device_scale_factor: 1.0,
            ..Default::default()
        };
        let t = std::time::Instant::now();
        renderer.render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap();
        times.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    times.sort_by(f64::total_cmp);
    println!("ordinary page: min {:.2} ms, median {:.2} ms", times[0], times[times.len() / 2]);
}

/// pyro: a column out of sight is not painted and leaves its pass's state behind it: a nested
/// container whose second column is off to the right, in the first column of the outer one, and
/// after it the outer container's spanner and its second column, painted as ever (review: the
/// culled column skipped restoring the column and spanner selectors).
#[test]
fn a_culled_column_leaves_later_boxes_painted() {
    for scale in [1.0, 2.0] {
        let img = render(
            "<div style='columns:2;column-gap:0;width:400px'>\
             <div style='columns:2;column-gap:0;width:2000px'><div style='height:40px;background:green'></div></div>\
             <div style='height:40px;background:green'></div>\
             <div style='column-span:all;height:20px;background:blue'></div>\
             <div style='height:40px;background:green'></div><div style='height:40px;background:green'></div></div>",
            scale,
        );
        check(
            &img,
            scale,
            &[(10.0, 10.0, GREEN), (210.0, 10.0, GREEN), (10.0, 40.0, BLUE), (390.0, 40.0, BLUE), (10.0, 60.0, GREEN), (210.0, 60.0, GREEN), (10.0, 95.0, WHITE)],
            "the boxes after the nested container",
        );
    }
}

/// pyro: a stacking context hoisted out of a multi-column container that is none (a `z-index`
/// box) is painted in each column it is in, from its place in the strip: one that starts in the
/// second column is painted there (it was moved into that column twice and clipped away), one a
/// break goes through in both.
#[test]
fn a_hoisted_box_is_painted_in_its_columns() {
    for scale in [1.0, 2.0] {
        let img = render(
            "<div style='columns:2;column-gap:0;width:200px'><div style='height:100px'></div>\
             <div style='position:relative;z-index:1;height:50px;background:green'></div></div>\
             <div style='columns:2;column-gap:0;width:200px;margin-top:100px'><div style='height:50px'></div>\
             <div style='position:relative;z-index:1;height:100px;background:blue'></div></div>",
            scale,
        );
        check(
            &img,
            scale,
            &[(150.0, 50.0, GREEN), (50.0, 50.0, WHITE), (150.0, 15.0, WHITE), (50.0, 240.0, BLUE), (150.0, 190.0, BLUE), (50.0, 190.0, WHITE)],
            "the boxes in their columns",
        );
    }
}

/// pyro: a stacking context hoisted out of the columns is painted in each column inside the clips
/// of the boxes between it and the container (an `overflow: clip` box broken across the columns),
/// as Chromium paints it and hit testing finds it; it was painted unclipped in the column copies.
#[test]
fn a_hoisted_box_in_columns_keeps_the_clips_around_it() {
    for scale in [1.0, 2.0] {
        let img = render(
            "<div style='columns:2;column-gap:0;width:200px'><div style='height:50px'></div>\
             <div style='overflow:clip;height:100px'><div style='position:relative;z-index:1;height:100px;width:150px;background:green'></div></div></div>",
            scale,
        );
        check(&img, scale, &[(50.0, 60.0, GREEN), (190.0, 30.0, GREEN), (220.0, 30.0, WHITE), (240.0, 60.0, WHITE)], "clipped in its columns");
    }
}

/// pyro: an out-of-flow box the columns fragment (its containing block, of no height, in them) is
/// painted in the overflow columns that show it, past the container's box: its container's
/// overflow reaches them (a box carrying no spanner counts with what it holds; WPT css-break
/// `out-of-flow-in-multicolumn-003`).
#[test]
fn a_fragmented_out_of_flow_box_is_painted_in_its_overflow_columns() {
    for scale in [1.0, 2.0] {
        let img = render(
            "<div style='column-count:2;column-fill:auto;column-gap:0;height:100px;width:100px;position:relative;left:-150px'>\
             <div style='position:relative'><div style='position:absolute;top:300px;width:50px;height:200px;background:green'></div></div></div>",
            scale,
        );
        check(&img, scale, &[(25.0, 50.0, GREEN), (75.0, 50.0, GREEN)], "the box in columns 4 and 5");
    }
}
