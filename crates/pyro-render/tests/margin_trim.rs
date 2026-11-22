//! `margin-trim` on block containers (css-box-4 §3.3; Taffy `compute/block.rs`): the margins of
//! the in-flow children at the container's block-start and block-end edges are truncated to zero,
//! with the margins that collapse with them. Pixel checks as in WPT's
//! `css/css-box/margin-trim/block-container-*` reftests.
use std::sync::Arc;

use pyro_render::*;

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}

fn render(body: &str) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let snapshot = Snapshot {
        html: format!("<!doctype html><body style='margin:0;background:#fff'>{body}"),
        base_url: "http://test/".into(),
        width: 300,
        height: 300,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

fn pixel(img: &RgbaImage, x: usize, y: usize) -> [u8; 3] {
    let i = (y * img.width as usize + x) * 4;
    [img.data[i], img.data[i + 1], img.data[i + 2]]
}

const WHITE: [u8; 3] = [255, 255, 255];
const GREEN: [u8; 3] = [0, 128, 0];
const BLUE: [u8; 3] = [0, 0, 255];

#[test]
fn max_height_separates_bottom_margin_only_when_it_shortens_the_block() {
    for (max_height, next_top) in [(49, 49), (50, 60), (51, 60)] {
        let img = render(&format!(
            "<div style='max-height:{max_height}px'><div style='height:50px;margin-bottom:10px'></div></div>\
             <div style='height:5px;background:green'></div>"
        ));
        assert_eq!(pixel(&img, 10, next_top - 1), WHITE);
        assert_eq!(pixel(&img, 10, next_top), GREEN);
    }
}

#[track_caller]
fn check(img: &RgbaImage, points: &[(usize, usize, [u8; 3])]) {
    for &(x, y, want) in points {
        assert_eq!(pixel(img, x, y), want, "pixel at ({x}, {y})");
    }
}

/// `margin-trim: block` removes the first child's top margin and the last child's bottom margin,
/// which would otherwise sit inside a container that does not collapse with them.
#[test]
fn block_trims_both_edges_of_a_flow_root() {
    let img = render(
        "<div style='display:flow-root;margin-trim:block;background:blue;width:100px'>\
           <div style='margin:50px 0;height:50px;background:green'></div>\
         </div>\
         <div style='width:100px;height:50px;background:green'></div>",
    );
    check(&img, &[(50, 10, GREEN), (50, 45, GREEN), (50, 60, GREEN), (50, 95, GREEN), (50, 105, WHITE)]);
}

/// Without the property the margins stay (the control of the test above).
#[test]
fn margins_stay_without_margin_trim() {
    let img = render(
        "<div style='display:flow-root;background:blue;width:100px'>\
           <div style='margin:50px 0;height:50px;background:green'></div>\
         </div>",
    );
    check(&img, &[(50, 10, BLUE), (50, 60, GREEN), (50, 110, BLUE), (50, 155, WHITE)]);
}

/// `block-start` trims only the start edge, and the margins a nested first child collapses
/// through its parent go with it.
#[test]
fn block_start_trims_nested_collapsed_margins() {
    let img = render(
        "<div style='display:flow-root;margin-trim:block-start;background:blue;width:100px'>\
           <div><div style='margin:30px 0;height:40px;background:green'></div></div>\
         </div>",
    );
    check(&img, &[(50, 5, GREEN), (50, 35, GREEN), (50, 45, BLUE), (50, 65, BLUE), (50, 75, WHITE)]);
}

/// `block-end` trims the last box's bottom margin and every margin of the collapsed-through boxes
/// after the last box with a height; a container whose margins collapse with its children keeps
/// its own bottom margin.
#[test]
fn block_end_trims_trailing_collapsed_through_boxes() {
    let img = render(
        "<div style='margin-trim:block-end;margin-bottom:10px;width:100px'>\
           <div style='margin-bottom:40px;height:50px;background:green'></div>\
           <div style='margin:100px 0 20px'></div>\
         </div>\
         <div style='width:100px;height:50px;background:blue'></div>",
    );
    check(&img, &[(50, 45, GREEN), (50, 55, WHITE), (50, 65, BLUE), (50, 105, BLUE), (50, 115, WHITE)]);
}

/// A container whose only children are collapsed through loses all their margins.
#[test]
fn block_end_trims_every_margin_of_collapsed_through_children() {
    let img = render(
        "<div style='display:flow-root;width:100px'>\
           <div style='margin-top:20px;margin-trim:block-end'><div style='margin-top:200px'></div></div>\
           <div style='height:30px;background:green'></div>\
         </div>",
    );
    check(&img, &[(50, 15, WHITE), (50, 25, GREEN), (50, 45, GREEN), (50, 55, WHITE)]);
}

/// The page of the two tests below: a zero-height `position: relative` box with 50px block margins
/// after a 50px green box, two blocks deep in a flow root; the blue box it holds marks where it is.
fn nested_self_collapsing_child(trim: &str) -> RgbaImage {
    render(&format!(
        "<div style='display:flow-root;width:100px;{trim}'><div><div>\
           <div style='height:50px;background:green'></div>\
           <div style='position:relative;margin:50px 0'>\
             <div style='position:absolute;top:0;width:100px;height:20px;background:blue'></div>\
           </div>\
         </div></div></div>"
    ))
}

/// `margin-trim: block-end` trims the margins that collapse through the bottom edges of the last
/// child and of its own last child, so the self-collapsing box among them moves up to the green
/// box's bottom (WPT `margin-trim/block-container-block-end-self-collapsing-children-nested-*`).
#[test]
fn block_end_trims_self_collapsing_boxes_nested_at_the_bottom() {
    let img = nested_self_collapsing_child("margin-trim:block-end");
    check(&img, &[(50, 45, GREEN), (50, 55, BLUE), (50, 65, BLUE), (50, 75, WHITE), (50, 105, WHITE)]);
}

/// Without the property the box stays after its top margin (the control of the test above).
#[test]
fn self_collapsing_boxes_nested_at_the_bottom_keep_margins_without_margin_trim() {
    let img = nested_self_collapsing_child("");
    check(&img, &[(50, 45, GREEN), (50, 55, WHITE), (50, 105, BLUE), (50, 115, BLUE), (50, 125, WHITE)]);
}
