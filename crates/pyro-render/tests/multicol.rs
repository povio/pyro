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
