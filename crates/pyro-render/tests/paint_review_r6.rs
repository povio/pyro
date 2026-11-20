//! Review findings of the round 6 paint lane: an inline element's `filter: blur()` with a
//! `clip-path` (blitz-paint `draw_inline_effects`), and a box's outline under its clipped
//! positioned children when another child escapes its clip.
use std::sync::Arc;

use pyro_render::*;

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _url: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        std::thread::spawn(move || done(Err("offline".into())));
    }
}

fn render(body: &str) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let snapshot = Snapshot {
        html: format!("<!doctype html><body style='margin:0;background:#fff;width:400px;height:300px'>{body}"),
        base_url: "http://test/".into(),
        width: 400,
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

/// A wrapping inline with `filter: blur(4px)` and `clip-path: inset(0)` is blurred inside its
/// clip: its background's edge, a pixel inside the box, is lighter than the background. The blur
/// was dropped (the spatial filter path left out elements with a clip path).
#[test]
fn an_inline_blur_with_a_clip_path_blurs() {
    let span = "<span style='background:#008000;filter:blur(4px);clip-path:inset(0);font:20px/40px monospace;line-height:40px'>xxxxxxxxxx</span>";
    let img = render(&format!("<div style='font:20px/40px monospace'>{span}</div>"));
    let unblurred = render(&format!("<div style='font:20px/40px monospace'>{}</div>", span.replace("filter:blur(4px);", "")));
    // The background's left edge: x = 0 is the box's first column.
    let (blurred_edge, sharp_edge) = (pixel(&img, 0, 20), pixel(&unblurred, 0, 20));
    assert_ne!(blurred_edge, sharp_edge, "the blur softens the edge: {blurred_edge:?} vs {sharp_edge:?}");
}

/// Renders `html` as a live page (its render tree from pyro-dom, as `pyro serve` paints pages).
fn render_live(html: &str) -> RgbaImage {
    use pyro_dom::snapshot::{RenderHtml, RenderNodeKind};
    let dom: pyro_dom::parse::DomRef = std::rc::Rc::new(std::cell::RefCell::new(pyro_dom::Dom::new()));
    let doc = pyro_dom::parse::parse_html_unsafe(&dom, html);
    let d = dom.borrow();
    let tree = d
        .render_tree(doc, &RenderHtml { style_text: &|_| None, stylesheet_href: &|_| None, mark: None, adopted: None })
        .unwrap();
    let tree = Tree {
        nodes: tree
            .nodes
            .into_iter()
            .map(|node| TreeNode {
                key: node.key,
                parent: node.parent,
                kind: match node.kind {
                    RenderNodeKind::Element { name, attrs } => TreeKind::Element { name, attrs },
                    RenderNodeKind::Text(text) => TreeKind::Text(text),
                },
            })
            .collect(),
    };
    let snap = Snapshot {
        base_url: "http://test/".into(),
        width: 400,
        height: 300,
        device_scale_factor: 1.0,
        live: Some(Live { page: 1, tree: Arc::new(tree), clip: None, interaction: None }),
        ..Default::default()
    };
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let mut renderer = Renderer::new();
    // Twice, as a live page is painted again: the second frame keeps the first's paint tree.
    let first = renderer.paint(&snap, Area::Viewport, &fetcher).unwrap();
    drop(renderer.raster(first, Backend::Cpu));
    let painted = renderer.paint(&snap, Area::Viewport, &fetcher).unwrap();
    renderer.raster(painted, Backend::Cpu).0
}

/// A box's inset outline (a negative `outline-offset`) goes under its positioned children, also
/// when another child (fixed, its containing block the viewport) escapes the clip of the box (a
/// stacking context): the escaping child turned the outline's deferral off for the whole box, so
/// the clipped positioned child painted under the outline.
#[test]
fn an_inset_outline_goes_under_clipped_positioned_children_beside_an_escaping_one() {
    let page = |fixed: &str| {
        format!(
            "<!doctype html><body style='margin:0'>\
             <div style='position:relative;z-index:0;overflow:hidden;width:100px;height:100px;outline:10px solid #00f;outline-offset:-10px'>\
             <div style='position:absolute;left:0;top:0;width:50px;height:50px;background:#008000'></div>{fixed}</div>"
        )
    };
    let alone = render_live(&page(""));
    assert_eq!(pixel(&alone, 5, 5), [0, 128, 0], "over the outline");
    let beside = render_live(&page("<div style='position:fixed;left:300px;top:200px;width:20px;height:20px;background:#f00'></div>"));
    assert_eq!(pixel(&beside, 5, 5), [0, 128, 0], "over the outline, beside a fixed child");
    assert_eq!(pixel(&beside, 310, 210), [255, 0, 0], "the fixed child, outside the clip");
    assert_eq!(pixel(&beside, 95, 95), [0, 0, 255], "the outline");
}


/// Taint from a `drop-shadow()` coloured from `currentcolor` on SVG content (Filter Effects,
/// "tainted filter primitives"): an `feDisplacementMap` whose map is tainted moves nothing, so the
/// green flood stays put over the red square. Untainted, the map (G = 1) moves the flood 50 px and
/// the red shows at the right.
const DISPLACE: &str = "<filter id=t color-interpolation-filters=sRGB>\
    <feFlood flood-color=green result=g /><feDisplacementMap in=g in2=SourceGraphic xChannelSelector=G yChannelSelector=B scale=100 />\
    </filter>";
const TAINTING: &str = "fill='rgb(0%, 100%, 50%)' style='color: rgb(0%, 100%, 50%); filter: drop-shadow(0 0 0 color-mix(in srgb, currentcolor 99.9%, black))";

fn displaced_or_not(svg: &str) -> [u8; 3] {
    pixel(&render(&format!("<svg width=100 height=100 style='display:block'>{svg}</svg>")), 90, 50)
}

/// The shadow's own filter list: tainted although an author filter has the id and the single
/// `feDropShadow` usvg gives the filters it makes of CSS functions (`filter1`).
#[test]
fn a_currentcolor_shadow_taints_beside_an_author_filter_like_a_generated_one() {
    let svg = format!(
        "{DISPLACE}<filter id=filter1><feDropShadow dx=0 dy=0 stdDeviation=0 /></filter>\
         <circle r=1 cx=200 filter=url(#filter1) />\
         <rect width=100 height=100 fill=red /><rect width=100 height=100 {TAINTING} url(#t)' />"
    );
    assert_eq!(displaced_or_not(&svg), [0, 128, 0]);
}

/// A tainted group inside a group whose filter displaces: the outer filter's source is tainted.
#[test]
fn taint_reaches_an_outer_groups_filter() {
    let svg = format!(
        "{DISPLACE}<rect width=100 height=100 fill=red />\
         <g filter=url(#t)><rect width=100 height=100 {TAINTING}' /></g>"
    );
    assert_eq!(displaced_or_not(&svg), [0, 128, 0]);
}

/// Without taint the map moves the flood: the tests above are not passing by a filter that does
/// nothing.
#[test]
fn an_untainted_map_moves_the_flood() {
    let svg = format!(
        "{DISPLACE}<rect width=100 height=100 fill=red />\
         <g filter=url(#t)><rect width=100 height=100 fill='rgb(0%, 100%, 50%)' /></g>"
    );
    assert_eq!(displaced_or_not(&svg), [255, 0, 0]);
}


/// An SVG image of a huge natural size (16000 by 16000) decoded for a canvas is rasterized at no
/// more than 16 Mi pixels, its aspect ratio kept: the limit is checked before allocating (it was
/// rasterized at its natural size, about 1 GB, before the canvas rejected it). Review cycle 2.
#[test]
fn a_huge_svg_image_is_rasterized_for_a_canvas_within_the_limit() {
    pyro_render::register_hooks();
    let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="16000" height="16000"><rect width="16000" height="16000" fill="green"/></svg>"#;
    let pixmap = pyro_canvas::codec::decode(svg).expect("an SVG image decodes");
    let (w, h) = (u64::from(pixmap.width()), u64::from(pixmap.height()));
    assert!(w * h <= 1 << 24, "{w} x {h} pixels");
    assert_eq!(w, h, "the aspect ratio is kept");
    // Drawn all over: the middle pixel is the rectangle's green.
    let i = ((h / 2) * w + w / 2) as usize * 4;
    let px = &pixmap.data_as_u8_slice()[i..i + 4];
    assert_eq!(px, &[0, 128, 0, 255], "the image fills its pixels");
}

/// A fixed box inside an `overflow: hidden` box that is no stacking context escapes its clip
/// (its containing block is the viewport) yet keeps its tree-order place among the positioned
/// boxes there: a later translucent white box over it covers it, as in Chromium. It was hoisted to
/// the root and painted over everything in the clipping box.
#[test]
fn a_fixed_box_escaping_a_clip_keeps_its_place_among_positioned_boxes() {
    let img = render(
        "<style>div{position:absolute;width:200px;height:200px}</style>\
         <div style='overflow:hidden;top:110px;left:10px'><div style='background:green'>\
         <div style='position:fixed;background:#f008;top:150px;left:50px'></div>\
         <div style='background:#fff8;top:90px;left:90px'></div></div></div>",
    );
    // Over all three: the white box over the red box over green.
    let [r, g, b] = pixel(&img, 150, 250);
    let close = |a: u8, e: u8| a.abs_diff(e) <= 2;
    assert!(close(r, 199) && close(g, 164) && close(b, 136), "white over red over green: {:?}", [r, g, b]);
    // Past the clipping box the fixed box still shows (red over the page's white).
    let [r, g, b] = pixel(&img, 230, 295);
    assert!(r > 240 && g < 140 && b < 140, "the fixed box past the clip: {:?}", [r, g, b]);
}

/// Positioned boxes whose containing block lies outside an `overflow: hidden` box (no stacking
/// context) escape its clip inside an opacity group between them, as Chromium clips by the
/// containing-block chain: the group's own content stays clipped, the boxes reach past it, and the
/// opacity still applies to all of it as one group. They were clipped with the group.
#[test]
fn boxes_escape_a_clip_inside_an_opacity_group_they_belong_to() {
    let img = render(
        "<div style='overflow:hidden;width:250px;height:250px;will-change:transform'>\
         <div style='overflow:hidden;width:300px;height:300px;transform:translateX(0)'>\
         <div style='overflow:hidden;width:200px;height:200px'>\
         <div style='width:300px;height:300px;background:red;opacity:0.5'>\
         <div style='width:400px;height:400px;background:yellow'>\
         <div style='position:absolute;width:400px;height:100px;background:blue'></div>\
         <div style='position:fixed;width:100px;height:400px;background:green'></div>\
         </div></div></div></div></div>",
    );
    // Past the 200px clip, inside the 250px one: the blue and green boxes at half opacity.
    let [r, g, b] = pixel(&img, 225, 50);
    assert!(r.abs_diff(127) <= 2 && g.abs_diff(127) <= 2 && b.abs_diff(255) <= 2, "blue past the inner clip: {:?}", [r, g, b]);
    let [r, g, b] = pixel(&img, 50, 225);
    assert!(r.abs_diff(127) <= 2 && g.abs_diff(191) <= 2 && b.abs_diff(127) <= 2, "green past the inner clip: {:?}", [r, g, b]);
    // The group's own content stays in the inner clip: nothing yellow past it.
    let [r, g, b] = pixel(&img, 225, 150);
    assert!(r > 250 && g > 250 && b > 250, "no yellow past the inner clip: {:?}", [r, g, b]);
}

/// An inline element that forms a stacking context (`will-change: opacity`) keeps its positioned
/// child: the child's `z-index: 3` orders it inside that context, under a `z-index: 1` box beside
/// it. It was hoisted to the root's context and painted over that box.
#[test]
fn an_inline_stacking_context_keeps_its_positioned_child() {
    let img = render(
        "<div style='position:absolute;width:100px;height:100px;background:green;z-index:1'></div>\
         <span style='will-change:opacity'><div style='position:absolute;width:100px;height:100px;background:red;z-index:3'></div></span>",
    );
    assert_eq!(pixel(&img, 50, 50), [0, 128, 0], "the z-index 1 box over the inline's child");
}

/// A box turned 45 degrees that clips a blending box is drawn in its own unturned space: the
/// overlay blend of red over its lime background is lime up to the clip's edge, and no red shows
/// where the edge is anti-aliased (once, as the turned surface is drawn). Red leaked there.
#[test]
fn a_turned_clip_over_a_blend_leaks_no_colour_at_its_edge() {
    let img = render(
        "<div style='width:100px;height:100px;background:lime;overflow:hidden'>\
         <div style='width:100px;height:100px;transform:rotate(45deg)'>\
         <div style='width:100px;height:100px;background:lime;overflow:hidden'>\
         <div style='width:100px;height:100px;background:red;transform:rotate(-45deg);mix-blend-mode:overlay;will-change:transform'></div>\
         </div></div></div>",
    );
    for y in 0..100 {
        for x in 0..100 {
            let [r, _, _] = pixel(&img, x, y);
            assert!(r <= 2, "no red at ({x}, {y}): {:?}", pixel(&img, x, y));
        }
    }
}

/// A relatively positioned inline with `clip-path: inset(0)` cuts the absolutely positioned box it
/// contains to its border box (a clip path clips the element's descendants): nothing of the box
/// shows past the inline. It reached out whole.
#[test]
fn an_inline_clip_path_cuts_the_positioned_box_it_keeps() {
    let img = render(
        "<div style='position:absolute;top:50px;left:50px;font:20px/20px monospace'>\
         <span style='position:relative;clip-path:inset(0)'>xxxx\
         <div style='position:absolute;top:-30px;left:-30px;width:200px;height:100px;background:green'></div></span></div>",
    );
    // Inside the inline's box: the green box.
    assert_eq!(pixel(&img, 60, 55), [0, 128, 0], "inside the inline");
    // Past it (the box reaches to x 220 and y 120): the page.
    assert_eq!(pixel(&img, 150, 100), [255, 255, 255], "past the inline");
}

/// A huge ellipse and a huge stroked circle of which only an arc is in view paint in bounded time:
/// the fine flattening (`anyrender_vello_cpu` `device_path`) is done only for paths near the
/// raster; one far past it is left to vello_cpu, which culls what is out of view. The whole curve
/// was tessellated at a fiftieth of a pixel first. Review cycle 3, finding 1.
#[test]
fn a_huge_curve_with_an_arc_in_view_paints_in_bounded_time() {
    let start = std::time::Instant::now();
    let img = render(
        "<svg width='400' height='300' style='display:block'>\
         <ellipse cx='200' cy='3000150' rx='3000000' ry='3000000' fill='green'/>\
         <circle cx='200' cy='-2999850' r='3000000' fill='none' stroke='blue' stroke-width='20'/></svg>",
    );
    assert!(start.elapsed() < std::time::Duration::from_secs(5), "took {:?}", start.elapsed());
    // The ellipse's top edge crosses the middle: green below, white above.
    assert_eq!(pixel(&img, 200, 200), [0, 128, 0], "inside the ellipse");
    assert_eq!(pixel(&img, 200, 100), [255, 255, 255], "above it");
}

/// Thousands of faded inline elements each keeping an absolutely positioned child, in a paragraph
/// of many lines, paint in work bounded by their own lines: each element's effect pass builds the
/// fragments of the lines its area reaches, not of the whole layout (it was quadratic: 9.7 s). The
/// work is counted (lines whose fragments were built), not timed. Review cycle 3, finding 2; cycle 4.
#[test]
fn many_inline_owners_paint_in_work_bounded_by_their_lines() {
    let spans: String = (0..3000).map(|_| "<span style='opacity:.9'>x<b style='position:absolute'></b></span> ").collect();
    let before = blitz_paint::inline_lines_visited();
    let _ = render(&format!("<div style='font:10px/10px monospace;width:390px'>{spans}</div>"));
    let visited = blitz_paint::inline_lines_visited() - before;
    assert!(visited > 0, "the paint ran on this thread");
    assert!(visited < 30_000, "{visited} lines built for 3000 faded elements");
}

/// An absolutely positioned box escaping the clip of an `overflow: hidden` box (no stacking
/// context) that paints no positioned box of its own keeps its tree-order place among its stacking
/// context's positioned boxes: over an earlier positioned box. It was painted with the clipping
/// box's in-flow content, under it (cycle-3 regression; WPT css/CSS2 `abspos/static-inside-inline-001`).
#[test]
fn a_box_escaping_a_clip_paints_over_an_earlier_positioned_box() {
    let img = render(
        "<style>body{margin:0}#w{overflow:hidden;width:100px;height:100px}\
         #a{position:absolute;background:#0f0;width:100px;height:100px}\
         #r{position:absolute;width:100px;height:100px;background:red}</style>\
         <div id=r></div><div id=w><span style='line-height:100px'><div id=a></div>X</span></div>",
    );
    let [r, g, b] = pixel(&img, 50, 50);
    assert!(r < 10 && g > 245 && b < 10, "the escaping box over the earlier one: {:?}", [r, g, b]);
}

/// A `perspective` on an inline that is not atomic (not transformable) does not make it keep its
/// absolutely positioned child: a child of negative z-index stays under the in-flow content
/// (cycle-3 regression; WPT css-transforms `perspective-untransformable-no-stacking-context`).
#[test]
fn an_untransformable_inline_keeps_no_positioned_child() {
    let img = render(
        "<style>*{margin:0}div,span{width:100px;height:100px}\
         #p{background:#0f0;padding-top:100px;perspective:100px}\
         #c{display:inline-block;z-index:-1;position:absolute;background:red}\
         #s{display:inline-block}</style>\
         <div style='overflow:hidden'><span id=p><div id=c></div><span id=s></span></span></div>",
    );
    let [r, g, b] = pixel(&img, 50, 50);
    assert!(r < 10 && g > 245 && b < 10, "the inline's background over the child: {:?}", [r, g, b]);
}

/// Two opacity groups in one `overflow: hidden` box (no stacking context), each holding a box whose
/// containing block lies outside the clip: both boxes escape it, in their groups. Only the first
/// group was handed the clip; the second was clipped whole. Review cycle 3, finding 3.
#[test]
fn boxes_escape_a_clip_in_each_of_two_opacity_groups() {
    let img = render(
        "<style>body{margin:0}.g{opacity:.5;height:100px}\
         .e{position:absolute;left:150px;width:50px;height:50px;background:blue}</style>\
         <div style='position:relative;width:300px;height:200px'>\
         <div style='overflow:hidden;width:100px;height:200px'>\
         <div class=g><div class=e style='top:0'></div></div>\
         <div class=g><div class=e style='top:100px'></div></div>\
         </div></div>",
    );
    for y in [25, 125] {
        let [r, g, b] = pixel(&img, 175, y);
        assert!(r.abs_diff(127) <= 2 && g.abs_diff(127) <= 2 && b > 250, "the box past the clip at y {y}: {:?}", [r, g, b]);
    }
}

/// The render tree of a `<details>` element with many children is written in linear time: each
/// child's index among its siblings was counted from the first. Review cycle 3, finding 5.
#[test]
fn a_details_with_many_children_renders_in_bounded_time() {
    let kids: String = (0..20000).map(|_| "<i></i>").collect();
    let start = std::time::Instant::now();
    let _ = render(&format!("<details open><summary>s</summary>{kids}</details>"));
    assert!(start.elapsed() < std::time::Duration::from_secs(20), "took {:?}", start.elapsed());
}

/// A box whose containing block lies outside an `overflow: hidden` box (no stacking context)
/// escapes its clip through two nested opacity groups: the inner group holds it, the outer group
/// holds the inner one. Only a group holding such a box itself took the clip over; the outer group
/// was clipped whole. Review cycle 4, finding 3.
#[test]
fn a_box_escapes_a_clip_through_nested_opacity_groups() {
    let img = render(
        "<style>body{margin:0}</style>\
         <div style='position:relative;width:300px;height:200px'>\
         <div style='overflow:hidden;width:100px;height:200px'>\
         <div style='opacity:.5;height:100px'><div style='opacity:.999;height:100px;background:yellow'>\
         <div style='position:absolute;left:150px;top:0;width:50px;height:50px;background:blue'></div>\
         </div></div></div></div>",
    );
    let [r, g, b] = pixel(&img, 175, 25);
    assert!(r.abs_diff(127) <= 3 && g.abs_diff(127) <= 3 && b > 250, "the box past the clip: {:?}", [r, g, b]);
    // The groups' own content stays clipped.
    let [r, g, b] = pixel(&img, 125, 50);
    assert!(r > 250 && g > 250 && b > 250, "no yellow past the clip: {:?}", [r, g, b]);
}

/// A huge stroked circle of which only an arc is in view costs the segments near the raster: its
/// segments far outside are left out before the stroke is expanded (the whole circle was expanded),
/// counted, not timed. Review cycle 4, finding 4.
#[test]
fn a_huge_stroke_with_an_arc_in_view_expands_what_is_near_the_raster() {
    let before = anyrender_vello_cpu::fine_segments();
    let img = render(
        "<svg width='400' height='300' style='display:block'>\
         <circle cx='200' cy='-2999850' r='3000000' fill='none' stroke='blue' stroke-width='20'/>\
         <circle cx='200' cy='690' r='550' fill='none' stroke='green' stroke-width='4'/></svg>",
    );
    let segments = anyrender_vello_cpu::fine_segments() - before;
    assert!(segments > 0, "the paint ran on this thread");
    // (15672 with the whole circles expanded.)
    assert!(segments < 2_000, "{segments} segments");
    // The arc crosses y 150 at x 200: blue there.
    let [r, g, b] = pixel(&img, 200, 150);
    assert!(b > 200 && r < 60 && g < 60, "the arc: {:?}", [r, g, b]);
}

/// A path of a great many tiny curves in view is flattened into at most a bounded number of
/// segments (past it, vello_cpu flattens it more coarsely): the fine flattening has a hard cap.
/// Review cycle 4, finding 4.
#[test]
fn a_path_of_countless_curves_is_flattened_within_a_cap() {
    let mut d = String::from("M0 150");
    for i in 0..100_000 {
        let x = i as f64 * 0.004;
        d.push_str(&format!(" C{:.3} 0 {:.3} 300 {:.3} 150", x + 0.001, x + 0.003, x + 0.004));
    }
    let before = anyrender_vello_cpu::fine_segments();
    let _ = render(&format!("<svg width='400' height='300' style='display:block'><path d='{d}' fill='green'/></svg>"));
    let segments = anyrender_vello_cpu::fine_segments() - before;
    assert!(segments <= 200_000, "{segments} segments");
}
