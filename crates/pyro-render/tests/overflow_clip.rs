//! What a box that clips its overflow clips to, as Chromium does: `overflow: clip` on one axis
//! clips that axis only (with square corners); on both, `overflow-clip-margin` names the box
//! and how far outside it content still shows; `contain: paint` clips as `overflow: clip`
//! does. And a positioned descendant with a `z-index`, painted by an outer stacking context,
//! stays inside the clip of a box between the two.
use std::sync::Arc;

use pyro_render::*;

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}

/// A 100px box at (100, 100) whose 300px green child starts 100px above and left of it.
const STYLE: &str = "<style>body{margin:0;background:white}
  .box{position:absolute;left:100px;top:100px;width:100px;height:100px}
  .child{width:300px;height:300px;margin:-100px 0 0 -100px;background:rgb(0,128,0)}</style>";

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

fn green(img: &RgbaImage, x: u32, y: u32) -> bool {
    let i = (y as usize * img.width as usize + x as usize) * 4;
    img.data[i..i + 3] == [0, 128, 0]
}

/// The green pixels' extent on the row and the column through the box's middle.
fn extent(img: &RgbaImage) -> ((u32, u32), (u32, u32)) {
    let xs: Vec<u32> = (0..img.width).filter(|&x| green(img, x, 150)).collect();
    let ys: Vec<u32> = (0..img.height).filter(|&y| green(img, 150, y)).collect();
    ((xs[0], xs[xs.len() - 1] + 1), (ys[0], ys[ys.len() - 1] + 1))
}

#[test]
fn overflow_clip_on_one_axis_clips_that_axis_only() {
    let x = render("<div class=box style='overflow-x:clip'><div class=child></div></div>");
    assert_eq!(extent(&x), ((100, 200), (0, 300)));
    let y = render("<div class=box style='overflow-y:clip'><div class=child></div></div>");
    assert_eq!(extent(&y), ((0, 300), (100, 200)));
    // No rounding where one axis does not clip.
    let round = render("<div class=box style='overflow-x:clip;border-radius:50px'><div class=child></div></div>");
    assert!(green(&round, 101, 101) && green(&round, 198, 198), "square corners");
}

#[test]
fn overflow_clip_margin_moves_the_clip_edge() {
    let margin = render("<div class=box style='overflow:clip;overflow-clip-margin:10px'><div class=child></div></div>");
    assert_eq!(extent(&margin), ((90, 210), (90, 210)));
    let boxes = "border:10px solid transparent;padding:10px;width:60px;height:60px;overflow:clip";
    let content =
        render(&format!("<div class=box style='{boxes};overflow-clip-margin:content-box'><div class=child></div></div>"));
    assert_eq!(extent(&content), ((120, 180), (120, 180)));
    let border =
        render(&format!("<div class=box style='{boxes};overflow-clip-margin:border-box 5px'><div class=child></div></div>"));
    assert_eq!(extent(&border), ((95, 205), (95, 205)));
    // A negative length moves the edge inwards (Blink takes it).
    let inwards = render("<div class=box style='overflow:clip;overflow-clip-margin:-10px'><div class=child></div></div>");
    assert_eq!(extent(&inwards), ((110, 190), (110, 190)));
    // A scroll container clips at its padding box whatever the margin says.
    let hidden =
        render(&format!("<div class=box style='{boxes};overflow:hidden;overflow-clip-margin:border-box 5px'><div class=child></div></div>"));
    assert_eq!(extent(&hidden), ((110, 190), (110, 190)));
    let contain = render("<div class=box style='contain:paint;overflow-clip-margin:10px'><div class=child></div></div>");
    assert_eq!(extent(&contain), ((90, 210), (90, 210)));
}

#[test]
fn a_z_indexed_descendant_stays_inside_the_clip_of_a_box_that_makes_no_stacking_context() {
    // The clipping box is static: the child (relative, z-index 1) is painted by the root.
    let page = "<div style='margin:100px;width:100px;height:100px;overflow:hidden'>
        <div class=child style='position:relative;z-index:1'></div></div>";
    assert_eq!(extent(&render(page)), ((100, 200), (100, 200)));
    // Scrolled content moves under the clip, which stays.
    let nested = "<div style='margin:100px;width:100px;height:100px;overflow:hidden'><div style='padding:10px'>
        <div class=child style='position:relative;z-index:-1'></div></div></div>";
    assert_eq!(extent(&render(nested)), ((100, 200), (100, 200)));
    // An absolutely positioned child whose containing block is outside the clipping box is not clipped by it.
    let escapes = "<div style='margin:100px;width:100px;height:100px;overflow:hidden'>
        <div class=child style='position:absolute;z-index:1;left:0;top:0;margin:0'></div></div>";
    assert_eq!(extent(&render(escapes)), ((0, 300), (0, 300)));
}
