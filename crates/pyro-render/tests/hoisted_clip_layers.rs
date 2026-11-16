//! Boxes with a `z-index` that an outer stacking context paints stay inside the overflow clips
//! of the boxes between the two, and boxes next to each other in paint order share those clip
//! layers: one clip layer per box cost a ToS table screen's capture half as much again (its
//! sticky cells and the buttons in its cells), and past the layer limit the boxes showed
//! unclipped.
use std::sync::Arc;

use pyro_render::*;

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}

/// A 100px box at (100, 100) that clips and makes no stacking context.
const STYLE: &str = "<style>body{margin:0;background:white}
  .box{position:absolute;left:100px;top:100px;width:100px;height:100px;overflow:hidden}
  .cell{position:relative;flex:none;width:50px;height:100px;overflow:hidden}
  .over{position:absolute;z-index:1;left:-100px;top:-100px;width:300px;height:300px}</style>";

fn render(body: &str) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let snapshot = Snapshot {
        html: format!("<!doctype html>{STYLE}{body}"),
        base_url: "http://test/".into(),
        width: 400,
        height: 400,
        device_scale_factor: 1.0,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

fn pixel(img: &RgbaImage, x: u32, y: u32) -> [u8; 3] {
    let i = (y as usize * img.width as usize + x as usize) * 4;
    [img.data[i], img.data[i + 1], img.data[i + 2]]
}

const GREEN: [u8; 3] = [0, 128, 0];
const BLUE: [u8; 3] = [0, 0, 255];
const WHITE: [u8; 3] = [255, 255, 255];

#[test]
fn more_z_indexed_boxes_in_one_clip_than_the_layer_limit_all_stay_inside_it() {
    // 1,500 boxes, each reaching 100px past every edge of the clipping box.
    let over = "<div class=over style='background:rgb(0,128,0)'></div>".repeat(1500);
    let img = render(&format!("<div class=box>{over}</div>"));
    for (x, y) in [(101, 101), (150, 150), (198, 198)] {
        assert_eq!(pixel(&img, x, y), GREEN, "inside at ({x}, {y})");
    }
    for (x, y) in [(50, 150), (250, 150), (150, 50), (150, 250), (5, 5), (295, 295)] {
        assert_eq!(pixel(&img, x, y), WHITE, "outside at ({x}, {y})");
    }
}

#[test]
fn boxes_in_different_inner_clips_keep_the_shared_outer_clip() {
    // Each box is clipped by its own cell and by the box around both cells.
    let cells = "<div style='display:flex'>
        <div class=cell><div class=over style='background:rgb(0,128,0)'></div></div>
        <div class=cell><div class=over style='background:rgb(0,0,255)'></div></div>
        <div class=cell><div class=over style='background:rgb(0,128,0)'></div></div></div>";
    let img = render(&format!("<div class=box>{cells}</div>"));
    assert_eq!(pixel(&img, 125, 150), GREEN, "first cell");
    assert_eq!(pixel(&img, 175, 150), BLUE, "second cell");
    // The third cell lies outside the clipping box.
    for (x, y) in [(90, 150), (210, 150), (230, 150), (125, 90), (175, 210)] {
        assert_eq!(pixel(&img, x, y), WHITE, "outside at ({x}, {y})");
    }
}

#[test]
fn a_box_without_a_clip_between_clipped_ones_is_not_clipped() {
    // Paint order: clipped (z 1), unclipped (z 2), clipped (z 3): the middle one is painted
    // with no clip layer open, above the first and below the last, and the last is clipped.
    let page = "<div class=box>
        <div class=over style='background:rgb(0,128,0)'></div>
        <div class=over style='z-index:3;left:50px;top:50px;width:100px;height:100px;background:rgb(0,0,255)'></div></div>
        <div style='position:absolute;z-index:2;left:20px;top:140px;width:160px;height:20px;background:rgb(255,0,0)'></div>";
    let img = render(page);
    assert_eq!(pixel(&img, 120, 120), GREEN);
    assert_eq!(pixel(&img, 175, 120), GREEN);
    assert_eq!(pixel(&img, 30, 150), [255, 0, 0], "outside the box, not clipped");
    assert_eq!(pixel(&img, 120, 150), [255, 0, 0], "above the first");
    assert_eq!(pixel(&img, 175, 155), BLUE, "below the last");
    assert_eq!(pixel(&img, 175, 175), BLUE);
    for (x, y) in [(220, 175), (175, 220)] {
        assert_eq!(pixel(&img, x, y), WHITE, "the last is clipped at ({x}, {y})");
    }
}
