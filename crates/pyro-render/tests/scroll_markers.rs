//! CSS Overflow 5's scroll markers and scroll buttons, as Chromium generates and lays them out: a
//! scroll container's `::scroll-marker-group` is a box beside its own (before or after it in its
//! parent's box list), holding the `::scroll-marker`s of the elements it is the nearest scroll
//! container of (through their containing blocks), in tree order; the active marker matches
//! `:target-current`; `::scroll-button()`s are buttons after the element's box, disabled where it
//! cannot scroll. `content-visibility: auto` contains paint (its overflow is clipped).
use std::sync::Arc;

use pyro_render::*;

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}

fn render(body: &str) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let snapshot = Snapshot {
        html: format!("<!doctype html><style>body{{margin:0;background:white}}</style>{body}"),
        base_url: "http://test/".into(),
        width: 300,
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

const GREEN: [u8; 3] = [0, 128, 0];
const RED: [u8; 3] = [255, 0, 0];
const BLUE: [u8; 3] = [0, 0, 255];
const WHITE: [u8; 3] = [255, 255, 255];

/// Markers 20px wide, 50px tall in a flex group; the first one blue, the others green.
const MARKERS: &str = "<style>
  .s{overflow:hidden;width:100px;height:50px}
  .s::scroll-marker-group{display:flex;height:50px}
  .s>div{height:50px}
  .s>div::scroll-marker{content:'';width:20px;height:50px;background:rgb(0,128,0)}
  .s>div:first-child::scroll-marker{background:rgb(0,0,255)}
</style>";

#[test]
fn the_group_is_a_box_beside_the_scroll_container() {
    // After: the group's box follows the scroll container's (at y = 50).
    let after = render(&format!("{MARKERS}<div class=s style='scroll-marker-group:after'><div></div><div></div></div>"));
    assert_eq!(pixel(&after, 10, 75), BLUE);
    assert_eq!(pixel(&after, 30, 75), GREEN);
    assert_eq!(pixel(&after, 50, 75), WHITE, "two markers");
    assert_eq!(pixel(&after, 10, 25), WHITE, "the scroll container holds no marker");
    // Before: it comes first.
    let before = render(&format!("{MARKERS}<div class=s style='scroll-marker-group:before'><div></div><div></div></div>"));
    assert_eq!(pixel(&before, 10, 25), BLUE);
    assert_eq!(pixel(&before, 10, 75), WHITE);
    // No group without the property, nor on a box that does not scroll.
    let none = render(&format!("{MARKERS}<div class=s><div></div></div>"));
    assert_eq!(pixel(&none, 10, 75), WHITE);
    let visible =
        render(&format!("{MARKERS}<div class=s style='overflow:visible;scroll-marker-group:after'><div></div></div>"));
    assert_eq!(pixel(&visible, 10, 75), WHITE);
}

#[test]
fn the_group_is_a_flex_item_of_the_scroll_containers_parent() {
    // WPT scroll-marker-group-layout-parent: the group (before, z-index 2) and the scroll container
    // (z-index 1, moved back over it) are flex items side by side.
    let img = render(
        "<style>#sc{z-index:1;overflow:hidden;scroll-marker-group:before;margin-left:-100px;width:100px;height:100px;background:red}
          #sc::scroll-marker-group{z-index:2;width:100px;height:100px;background:rgb(0,128,0)}</style>
         <div style='display:flex;margin-left:100px'><div id=sc></div></div>",
    );
    assert_eq!(pixel(&img, 150, 50), GREEN);
}

#[test]
fn markers_belong_to_the_scroll_container_of_their_containing_block_chain() {
    // An absolutely positioned element whose containing block is outside the scroll container
    // has no marker in its group; one inside a positioned element of it has (WPT scroll-marker-005).
    let img = render(
        "<style>.s{overflow:hidden;width:100px;height:50px;scroll-marker-group:after}
          .s::scroll-marker-group{display:flex;height:50px}
          .m::scroll-marker{content:'';width:20px;height:50px;background:rgb(0,128,0)}
          .e::scroll-marker{content:'';width:20px;height:50px;background:red}</style>
         <div class=s><div class=m></div>
           <div style='position:absolute'><div class=e></div></div>
           <div class=m style='position:relative'><div class=m style='position:absolute'></div></div></div>",
    );
    assert_eq!(pixel(&img, 10, 75), GREEN);
    assert_eq!(pixel(&img, 30, 75), GREEN, "the relative box's own marker");
    assert_eq!(pixel(&img, 50, 75), GREEN, "the absolutely positioned child of the relative box");
    assert_eq!(pixel(&img, 70, 75), WHITE, "nothing more, and no red");
}

#[test]
fn the_first_marker_is_current_at_the_start() {
    let img = render(
        "<style>.s{overflow:hidden;height:50px;scroll-marker-group:after}
          .s::scroll-marker-group{display:flex}
          .s>div{height:100px}
          .s>div::scroll-marker{content:'';width:20px;height:20px;background:red}
          .s>div::scroll-marker:target-current{background:rgb(0,128,0)}</style>
         <div class=s><div></div><div></div></div>",
    );
    assert_eq!(pixel(&img, 10, 60), GREEN);
    assert_eq!(pixel(&img, 30, 60), RED);
}

#[test]
fn scroll_buttons_follow_the_box_and_disable_where_it_cannot_scroll() {
    // `down` selects the block-end button of a horizontal box; the up button of a box at its start
    // is disabled. Block-start comes before block-end.
    let img = render(
        "<style>.s{overflow:hidden;height:50px;width:100px}
          .s>div{height:200px}
          .s::scroll-button(up),.s::scroll-button(down){content:'';display:block;width:50px;height:20px;border:none;padding:0;background:rgb(0,128,0)}
          .s::scroll-button(up):disabled{background:rgb(0,0,255)}</style>
         <div class=s><div></div></div>",
    );
    assert_eq!(pixel(&img, 10, 60), BLUE, "block-start, disabled");
    assert_eq!(pixel(&img, 10, 80), GREEN, "block-end, enabled");
    assert_eq!(pixel(&img, 10, 100), WHITE);
}

#[test]
fn content_visibility_auto_clips_its_overflow() {
    let img = render(
        "<div style='content-visibility:auto;width:100px;height:100px'>
           <div style='width:200px;height:200px;background:rgb(0,128,0)'></div></div>",
    );
    assert_eq!(pixel(&img, 50, 50), GREEN);
    assert_eq!(pixel(&img, 150, 50), WHITE);
    assert_eq!(pixel(&img, 50, 150), WHITE);
}
