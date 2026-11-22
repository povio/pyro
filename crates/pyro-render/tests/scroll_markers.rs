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
fn targeted_marker_follows_the_fractional_column_break() {
    use pyro_dom::parse::parse_document;
    use pyro_dom::snapshot::{RenderHtml, RenderNodeKind, render_key};
    use std::{cell::RefCell, rc::Rc};
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    for (count, leading, active, scroll) in [(1, "", 1, 100.), (2, "", 1, 50.), (1, "<aside></aside>", 2, 200.)] {
        let html = format!("<!doctype html><style>*{{margin:0}}main{{width:100px;height:40px;columns:{count};column-fill:auto;gap:0;overflow:auto;scroll-snap-type:x mandatory;scroll-marker-group:after}}main::column::scroll-marker{{content:\"\";width:12px;height:12px;background:red}}main::column::scroll-marker:target-current{{background:lime}}main::scroll-marker-group{{display:flex}}header,aside{{height:10.75px}}header{{break-before:column}}section,footer{{break-before:column;height:20px;scroll-snap-align:start}}</style><main id=sc>{leading}<header></header><section id=t></section><footer></footer><footer></footer></main>");
        let dom = Rc::new(RefCell::new(pyro_dom::Dom::new()));
        let doc = parse_document(&dom, "http://test/", &html);
        let dom = dom.borrow();
        let key = |id| render_key(dom.get_element_by_id(doc, id).unwrap().unwrap());
        let tree = dom.render_tree(doc, &RenderHtml { style_text: &|_| None, stylesheet_href: &|_| None, mark: None, adopted: None }).unwrap();
        let nodes = tree.nodes.into_iter().map(|node| TreeNode { key: node.key, parent: node.parent, kind: match node.kind {
            RenderNodeKind::Element { name, attrs } => TreeKind::Element { name, attrs },
            RenderNodeKind::Text(text) => TreeKind::Text(text),
        }}).collect();
        let mut snapshot = Snapshot { base_url: "http://test/".into(), width: 300, height: 100, device_scale_factor: 1., live: Some(Live { page: 1, tree: Arc::new(Tree { nodes }), clip: None, interaction: None }), ..Default::default() };
        let mut renderer = Renderer::new();
        renderer.sync_layout(&snapshot, &fetcher).unwrap();
        snapshot.element_scroll = Some(vec![(key("sc"), (scroll, 0.))]);
        snapshot.scroll_targets = Some(vec![(key("sc"), key("t"))]);
        let img = renderer.render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0;
        for index in 0..3 {
            assert_eq!(pixel(&img, 12 * index + 5, 45), if index == active { [0, 255, 0] } else { RED });
        }
    }
}

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

#[test]
fn a_block_group_beside_an_inline_level_scroller_is_built_once() {
    // The block-level group splits the inline content around it once: the anonymous block
    // wrapping the scroller does not place its group again (box construction recursed without
    // end). The group (blue, 20px tall) comes before the line holding the scroller.
    let img = render(
        "<style>#t{display:inline-block;overflow:auto;width:40px;height:20px;background:rgb(0,128,0);
          scroll-marker-group:before;vertical-align:top}
          #t::scroll-marker-group{display:block;height:20px;background:rgb(0,0,255)}</style>
         <div style='font:20px/20px monospace'><span id=t>x</span></div>",
    );
    assert_eq!(pixel(&img, 10, 10), BLUE, "the group's box first");
    assert_eq!(pixel(&img, 10, 30), GREEN, "then the line with the scroller");
    assert_eq!(pixel(&img, 10, 50), WHITE, "and no second group");
}

#[test]
fn an_inline_box_is_no_scroll_container() {
    // `overflow` does not apply to a non-replaced inline box: no group (WPT
    // inline-with-scroll-marker-group-crash hung on its block group).
    let img = render(
        "<style>#t{overflow:auto;scroll-marker-group:before}
          #t::scroll-marker-group{display:block;height:20px;background:rgb(255,0,0)}</style>
         <span id=t></span>",
    );
    assert_eq!(pixel(&img, 10, 10), WHITE);
}

#[test]
fn markers_before_and_after_the_current_one() {
    // `:target-before` and `:target-after`: the markers of a group before and after its active
    // one (the first, at the start).
    let img = render(&format!(
        "{MARKERS}<style>.s>div::scroll-marker:target-after{{background:rgb(255,0,0)}}
          .s>div::scroll-marker:target-before{{background:rgb(255,255,255)}}</style>
         <div class=s style='scroll-marker-group:after'><div></div><div></div><div></div></div>"
    ));
    assert_eq!(pixel(&img, 10, 75), BLUE, "the current one");
    assert_eq!(pixel(&img, 30, 75), RED, "after it");
    assert_eq!(pixel(&img, 50, 75), RED, "after it");
}

/// A live renderer laid out for `html`, and the render keys of its elements by id.
fn live(html: &str, width: u32, height: u32) -> (Renderer, impl Fn(&str) -> u64) {
    use pyro_dom::parse::parse_document;
    use pyro_dom::snapshot::{RenderHtml, RenderNodeKind, render_key};
    use std::{cell::RefCell, rc::Rc};
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let dom = Rc::new(RefCell::new(pyro_dom::Dom::new()));
    let doc = parse_document(&dom, "http://test/", html);
    let dom = dom.borrow();
    let tree = dom.render_tree(doc, &RenderHtml { style_text: &|_| None, stylesheet_href: &|_| None, mark: None, adopted: None }).unwrap();
    let ids: Vec<(String, u64)> = dom
        .descendants(doc)
        .unwrap()
        .filter_map(|n| Some((dom.get_attribute(n, "id").ok().flatten()?.to_owned(), render_key(n))))
        .collect();
    let nodes = tree.nodes.into_iter().map(|node| TreeNode { key: node.key, parent: node.parent, kind: match node.kind {
        RenderNodeKind::Element { name, attrs } => TreeKind::Element { name, attrs },
        RenderNodeKind::Text(text) => TreeKind::Text(text),
    }}).collect();
    let snapshot = Snapshot { base_url: "http://test/".into(), width, height, device_scale_factor: 1., live: Some(Live { page: 1, tree: Arc::new(Tree { nodes }), clip: None, interaction: None }), ..Default::default() };
    let mut renderer = Renderer::new();
    renderer.sync_layout(&snapshot, &fetcher).unwrap();
    (renderer, move |id: &str| ids.iter().find(|(i, _)| i == id).map(|(_, k)| *k).unwrap())
}

#[test]
fn a_scroll_button_is_hit_with_its_direction_and_state() {
    // CSS Overflow 5 § 2.2: activating a ::scroll-button() scrolls its originating element in the
    // button's direction; one whose element cannot scroll that way is disabled (WPT css-overflow
    // scroll-markers/scroll-buttons-activation, scroll-button-link-activation).
    let html = "<!doctype html><style>*{margin:0}#sc,#plain{width:100px;height:50px;white-space:nowrap}#sc{overflow:auto}#sc span,#plain span{display:inline-block;width:300px;height:10px}#sc::scroll-button(inline-end){content:'';position:absolute;top:0;left:0;width:20px;height:20px}#sc::scroll-button(up){content:'';position:absolute;top:0;left:40px;width:20px;height:20px}#plain::scroll-button(right){content:'';position:absolute;top:80px;left:0;width:20px;height:20px}</style><div id=sc><span></span></div><div id=plain><span></span></div>";
    let (mut renderer, key) = live(html, 300, 200);
    // inline-end of a horizontal-tb ltr scroller: rightwards, enabled (it can scroll right).
    assert_eq!(renderer.scroll_button_hit(1, key("sc"), 5., 5.), Some(((1, 0), true)));
    // up: upwards, disabled (it is at the top and has no vertical overflow).
    assert_eq!(renderer.scroll_button_hit(1, key("sc"), 45., 5.), Some(((0, -1), false)));
    // The button of an element that is no scroll container is disabled.
    assert_eq!(renderer.scroll_button_hit(1, key("plain"), 5., 85.), Some(((1, 0), false)));
    // The scroller's own content between the buttons is no button.
    assert_eq!(renderer.scroll_button_hit(1, key("sc"), 30., 5.), None);
    // Another element's button is not this one's.
    assert_eq!(renderer.scroll_button_hit(1, key("plain"), 5., 5.), None);
}

#[test]
fn a_scroll_button_hit_is_the_painted_one() {
    // Overlapping buttons: the one painted on top is hit (z-index), not the first in tree or
    // direction order. A button that is `visibility: hidden`, `pointer-events: none` or clipped
    // out (by an `overflow: hidden` positioned ancestor) is not hit: the point falls through to
    // what is under it, here the scroller's own content.
    let html = "<!doctype html><style>*{margin:0}#sc{position:relative;width:200px;height:100px;overflow:auto;white-space:nowrap}#sc span{display:inline-block;width:600px;height:90px}#sc::scroll-button(*){content:'';position:absolute;width:40px;height:40px}#sc::scroll-button(inline-start){top:0;left:0;z-index:1}#sc::scroll-button(inline-end){top:0;left:0;z-index:2}#sc::scroll-button(block-start){top:0;left:60px;visibility:hidden}#sc::scroll-button(block-end){top:0;left:120px;pointer-events:none}#clip{position:relative;width:200px;height:60px;overflow:hidden;margin-top:20px}#c2{width:200px;height:50px;overflow:auto;white-space:nowrap}#c2 span{display:inline-block;width:600px;height:10px}#c2::scroll-button(inline-end){content:'';position:absolute;top:100px;left:0;width:40px;height:40px}</style><div id=sc><span></span></div><div id=clip><div id=c2><span></span></div></div>";
    let (mut renderer, key) = live(html, 300, 300);
    // inline-end (z-index 2) over inline-start (z-index 1).
    assert_eq!(renderer.scroll_button_hit(1, key("sc"), 10., 10.), Some(((1, 0), true)));
    // Hidden and pointer-events: none buttons are not hit.
    assert_eq!(renderer.scroll_button_hit(1, key("sc"), 70., 10.), None);
    assert_eq!(renderer.scroll_button_hit(1, key("sc"), 130., 10.), None);
    // c2's button sits 100px down in #clip (at y 120 + 100), past its 60px: clipped out.
    assert_eq!(renderer.scroll_button_hit(1, key("c2"), 10., 230.), None);
}
