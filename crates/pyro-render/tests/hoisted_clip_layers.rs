//! Boxes with a `z-index` that an outer stacking context paints stay inside the overflow clips
//! of the boxes between the two, and boxes next to each other in paint order share those clip
//! layers: one clip layer per box cost a ToS table screen's capture half as much again (its
//! sticky cells and the buttons in its cells), and past the layer limit the boxes showed
//! unclipped. A box its clips cut nothing of is painted in none of them: one layer per icon
//! button in its `overflow: hidden` cell still cost a ToS finances capture a tenth more.
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

/// The clip layers a page's scene pushes, painted as a screenshot paints it (the device pixels
/// known: `blitz_paint::paint_scene`).
fn clip_layer_count(body: &str) -> usize {
    use anyrender::recording::{RenderCommand, Scene};
    let html = format!("<!doctype html>{STYLE}{CELLS}{body}");
    let mut doc = blitz_html::HtmlDocument::from_html(&html, blitz_dom::DocumentConfig::default());
    doc.viewport_mut().window_size = (400, 400);
    doc.resolve(0.0);
    let mut scene = Scene::new();
    blitz_paint::paint_scene(&mut scene, &mut doc, 1.0, 400, 400, 0, 0);
    scene.commands.iter().filter(|command| matches!(command, RenderCommand::PushClipLayer(_))).count()
}

/// Cells that clip, each holding a button-sized box with room around it (the ToS finances
/// table: an icon button in its `overflow: hidden` cell).
const CELLS: &str = "<style>.pad{position:relative;flex:none;width:30px;height:30px;padding:5px;overflow:hidden}
  .button{width:20px;height:20px;background:rgb(0,0,255);border-radius:3px}</style>";

#[test]
fn a_z_indexed_box_well_inside_its_clip_pushes_no_clip_layer() {
    // 100 cells: each clips its own content (one layer each, as before round 3). A z-index on the
    // boxes inside put a second layer per cell around each box (the cell's clip again, as the box
    // is painted by the root's stacking context); it cuts nothing, and is left out.
    let cells = |style: &str| {
        let cell = format!("<div class=pad><div class=button style='{style}'></div></div>");
        format!("<div style='display:flex;flex-wrap:wrap'>{}</div>", cell.repeat(100))
    };
    let unpositioned = clip_layer_count(&cells(""));
    assert!(unpositioned >= 100, "each cell clips: {unpositioned}");
    assert_eq!(clip_layer_count(&cells("position:relative;z-index:1")), unpositioned);
    // A box reaching past its cell keeps the cell's clip.
    assert_eq!(clip_layer_count(&cells("position:relative;z-index:1;left:20px")), unpositioned + 100);
}

/// `body` at 200x200 CSS px and `dsf`, as a screenshot.
fn render_at(body: &str, dsf: f64) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let snapshot = Snapshot {
        html: format!("<!doctype html><style>body{{margin:0;background:white}}</style>{body}"),
        base_url: "http://test/".into(),
        width: 200,
        height: 200,
        device_scale_factor: dsf,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

/// The pixels that differ between two images, and by how many levels at most.
fn difference(a: &RgbaImage, b: &RgbaImage) -> (usize, u8) {
    let differing = a.data.chunks(4).zip(b.data.chunks(4)).filter(|(a, b)| a != b).count();
    (differing, a.data.iter().zip(&b.data).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0))
}

/// A clip box between device pixels, with rounded corners.
const CLIP: &str = "position:absolute;left:20.25px;top:30.5px;width:120.5px;height:80.75px;\
    border-radius:12px;background:rgb(240,240,200)";

#[test]
fn a_hoisted_box_its_clips_cut_is_painted_in_them_as_without_a_z_index() {
    // Flush with an edge between pixels (antialiased by the clip as well), in a rounded corner, and
    // reaching out: with a z-index the box is painted by the root's stacking context in its clips;
    // without one, in its clipping box's own clip layer. The pixels are the same.
    let boxes = [
        "margin:0 0 0 30px;width:40px;height:20.5px;background:rgb(0,128,0)",
        "margin:0;width:10px;height:10px;background:rgb(0,0,200)",
        "margin:-2.25px 0 0 -3.5px;width:200px;height:12px;background:rgba(0,90,0,.6)",
    ];
    for dsf in [1.0, 3.0] {
        for (i, style) in boxes.iter().enumerate() {
            let page = |extra: &str| format!("<div style='{CLIP};overflow:hidden'><div style='{extra}{style}'></div></div>");
            let flat = render_at(&page(""), dsf);
            let hoisted = render_at(&page("position:relative;z-index:1;"), dsf);
            assert_eq!(difference(&flat, &hoisted), (0, 0), "box {i} at {dsf}x");
        }
    }
}

#[test]
fn a_hoisted_box_well_inside_its_clips_is_painted_as_with_no_clip() {
    // The clips leave out a box they cut nothing of: its pixels are those of the same box in a box
    // that does not clip, translucent edges over each other included. In a clip layer they round
    // a level apart in places (vello_cpu composites the layer's 8-bit pixels once more).
    let boxes = [
        "margin:10.25px 0 0 20.5px;width:40.5px;height:30.25px;border-radius:6px;\
         border:1.5px solid rgba(0,0,90,.5);background:rgba(200,0,0,.45)",
        "margin:10px 0 0 20px;width:40px;height:30px;background:rgb(0,128,0)",
    ];
    for dsf in [1.0, 3.0] {
        for (i, style) in boxes.iter().enumerate() {
            let page = |overflow: &str, extra: &str| {
                format!("<div style='{CLIP};overflow:{overflow}'><div style='{extra}{style}'></div></div>")
            };
            let unclipped = render_at(&page("visible", "position:relative;z-index:1;"), dsf);
            let hoisted = render_at(&page("hidden", "position:relative;z-index:1;"), dsf);
            assert_eq!(difference(&unclipped, &hoisted), (0, 0), "box {i} at {dsf}x");
            let (_, levels) = difference(&render_at(&page("hidden", ""), dsf), &hoisted);
            assert!(levels <= 1, "box {i} at {dsf}x: {levels} levels from the clip layer's rounding");
        }
    }
}
