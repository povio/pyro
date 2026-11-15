//! 3D transforms are flattened into the plane of the screen, as browsers composite an element
//! whose `transform-style` is `flat`: `rotateX`, `rotateY`, `rotate3d`, `translateZ`, `scaleZ`
//! and `matrix3d` paint their projection (they painted untransformed), under the perspective of
//! the containing block when it has one (a depth then scales the box about the perspective
//! origin, which does not scroll with the box's content). `backface-visibility: hidden` hides a
//! box that shows its back face, and a box behind the viewer or with a non-invertible matrix
//! is not drawn. The boxes layout queries report follow the same flattened transforms.
use std::{cell::RefCell, rc::Rc, sync::Arc};

use pyro_dom::{
    parse::{parse_document, DomRef},
    snapshot::{render_key, RenderHtml, RenderNodeKind},
};
use pyro_render::*;

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}

const GREEN: [u8; 3] = [0, 128, 0];
const WHITE: [u8; 3] = [255, 255, 255];

fn render(body: &str, scale: f64) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let snapshot = Snapshot {
        html: format!("<!doctype html><body style='margin:0;background:#fff'>{body}"),
        base_url: "http://test/".into(),
        width: 400,
        height: 300,
        device_scale_factor: scale,
        ..Default::default()
    };
    Renderer::new().render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
}

/// The pixel at CSS px (x, y).
fn pixel(img: &RgbaImage, scale: f64, x: f64, y: f64) -> [u8; 3] {
    let (x, y) = ((x * scale) as usize, (y * scale) as usize);
    let i = (y * img.width as usize + x) * 4;
    [img.data[i], img.data[i + 1], img.data[i + 2]]
}

/// The columns (CSS px, at row `y`) and rows (at column `x`) painted `color`: (first, last).
fn extent(img: &RgbaImage, scale: f64, color: [u8; 3], x: f64, y: f64) -> ((f64, f64), (f64, f64)) {
    let (w, h) = (img.width as f64 / scale, img.height as f64 / scale);
    let span = |hits: Vec<f64>| (hits.first().copied().unwrap_or(-1.0), hits.last().copied().unwrap_or(-1.0));
    let columns = (0..w as usize).map(|c| c as f64 + 0.5).filter(|&c| pixel(img, scale, c, y) == color).collect();
    let rows = (0..h as usize).map(|r| r as f64 + 0.5).filter(|&r| pixel(img, scale, x, r) == color).collect();
    (span(columns), span(rows))
}

/// A box 100px square at (100, 100).
fn square(style: &str) -> String {
    format!(
        "<div style='position:absolute;left:100px;top:100px;width:100px;height:100px;background:green;{style}'></div>"
    )
}

/// `rotateY(60deg)` paints the box at half its width about its centre, `rotateX(60deg)` at half
/// its height; `rotate3d` and the `rotate` property too, and a depth alone changes nothing.
/// They painted the whole box.
#[test]
fn a_rotation_out_of_the_plane_paints_its_projection() {
    for scale in [1.0, 2.0] {
        for (style, columns, rows) in [
            ("transform:rotateY(60deg)", (125.5, 174.5), (100.5, 199.5)),
            ("transform:rotateX(60deg)", (100.5, 199.5), (125.5, 174.5)),
            ("transform:rotate3d(0,1,0,-60deg)", (125.5, 174.5), (100.5, 199.5)),
            ("rotate:x 60deg", (100.5, 199.5), (125.5, 174.5)),
            ("transform:translateZ(50px) scaleZ(3)", (100.5, 199.5), (100.5, 199.5)),
            // matrix3d(1,0,0,0, 0,2,0,0, 0,0,3,0, 4,5,6,1): twice as high about the centre, moved by (4, 5).
            ("transform:matrix3d(1,0,0,0,0,2,0,0,0,0,3,0,4,5,6,1)", (104.5, 203.5), (55.5, 254.5)),
        ] {
            let img = render(&square(style), scale);
            assert_eq!(extent(&img, scale, GREEN, 150.0, 150.0), (columns, rows), "{style}, scale {scale}");
        }
    }
}

/// The perspective of the containing block scales a box moved towards the viewer by d / (d - z)
/// about the perspective origin: `perspective: 3px` and `translateZ(1px)` make a 100px box at
/// (25, 25) 150px at (0, 0) about the centre of a 150px container; `perspective-origin: 0 0`
/// scales it away from the corner (by 6 / 5 for `perspective: 6px`); `perspective: 0` is 1px.
/// It painted untransformed.
#[test]
fn a_depth_under_a_perspective_scales_about_the_perspective_origin() {
    let page = |container: &str, child: &str| {
        format!(
            "<div style='position:absolute;left:100px;top:100px;width:150px;height:150px;{container}'>\
             <div style='position:absolute;left:25px;top:25px;width:100px;height:100px;background:green;{child}'></div></div>"
        )
    };
    for scale in [1.0, 2.0] {
        // (container, box, probe inside the result, its columns and rows)
        for (container, child, probe, span) in [
            ("perspective:3px", "transform:translateZ(1px)", 175.0, (100.5, 249.5)),
            ("perspective:6px;perspective-origin:0 0", "transform:translateZ(1px)", 200.0, (130.5, 249.5)),
            ("perspective:2px;perspective-origin:0 0", "transform:translateZ(-3px)", 130.0, (110.5, 149.5)),
            // The `perspective()` function of the box itself, about its transform origin.
            ("", "transform:perspective(100px) translateZ(50px)", 175.0, (75.5, 274.5)),
        ] {
            let img = render(&page(container, child), scale);
            assert_eq!(extent(&img, scale, GREEN, probe, probe), (span, span), "{container} / {child}, scale {scale}");
        }
        // `perspective: 0` is 1px: twice the size about the corner, cut by the viewport's bottom.
        let img = render(&page("perspective:0;perspective-origin:0 0", "transform:translateZ(0.5px)"), scale);
        assert_eq!(
            extent(&img, scale, GREEN, 175.0, 175.0),
            ((150.5, 349.5), (150.5, 299.5)),
            "perspective: 0, scale {scale}"
        );
    }
}

/// A box at or beyond the viewer's distance is not drawn.
#[test]
fn a_box_behind_the_viewer_is_not_drawn() {
    let img = render(
        "<div style='perspective:100px'><div style='width:100px;height:100px;background:green;transform:translateZ(150px)'></div></div>",
        1.0,
    );
    assert_eq!(pixel(&img, 1.0, 50.0, 50.0), WHITE);
}

/// `backface-visibility: hidden` hides a box (and its content) that shows its back face:
/// `rotateY(180deg)`, `rotateX(180deg)`, `scaleZ(-1)` (which changes nothing in the plane). A 2D
/// mirror shows no back face, and a flipped box without it shows mirrored.
#[test]
fn backface_visibility_hidden_hides_a_box_showing_its_back() {
    let kid = "<div style='width:50px;height:100px;background:green'></div>";
    for scale in [1.0, 2.0] {
        // (style, probe column, whether the child shows there): mirrored, the child's half
        // is on the right.
        for (style, probe, visible) in [
            ("transform:rotateY(180deg);backface-visibility:hidden", 175.0, false),
            ("transform:rotateX(180deg);backface-visibility:hidden", 125.0, false),
            ("transform:scaleZ(-1);backface-visibility:hidden", 125.0, false),
            ("transform:rotateY(170deg);backface-visibility:hidden", 155.0, false),
            ("transform:rotateY(80deg);backface-visibility:hidden", 145.0, true),
            ("transform:scaleX(-1);backface-visibility:hidden", 175.0, true),
            ("transform:rotateY(180deg)", 175.0, true),
        ] {
            let img = render(
                &format!(
                    "<div style='position:absolute;left:100px;top:100px;width:100px;height:100px;{style}'>{kid}</div>"
                ),
                scale,
            );
            let want = if visible { GREEN } else { WHITE };
            assert_eq!(pixel(&img, scale, probe, 150.0), want, "{style}, scale {scale}");
        }
    }
}

/// In a 3D rendering context (`transform-style: preserve-3d`) the parent's matrix decides with
/// the child's which face shows: the card flip. A child turned 180deg in a parent turned 180deg
/// faces the viewer; an untransformed child of that parent shows its back. A `preserve-3d`
/// element is not hidden by its own `backface-visibility`: its children are planes of their own.
#[test]
fn a_3d_rendering_context_decides_which_face_its_children_show() {
    let card = "position:absolute;left:0;top:0;width:100px;height:100px;backface-visibility:hidden";
    let flip = |front: &str, back: &str, outer: &str| {
        format!(
            "<div style='position:absolute;left:100px;top:100px;width:100px;height:100px;transform:rotateY(180deg);{outer}'>\
             <div style='{card};background:{front}'></div>\
             <div style='{card};background:{back};transform:rotateY(180deg)'></div></div>"
        )
    };
    for scale in [1.0, 2.0] {
        let img = render(&flip("red", "green", "transform-style:preserve-3d"), scale);
        assert_eq!(pixel(&img, scale, 150.0, 150.0), GREEN, "card flip, scale {scale}");
        // Without `preserve-3d` each child is flattened into the parent's plane: only its own
        // matrix decides, and the front shows (mirrored), the back is hidden.
        let img = render(&flip("green", "red", ""), scale);
        assert_eq!(pixel(&img, scale, 150.0, 150.0), GREEN, "flat parent, scale {scale}");
        let img = render(
            &format!(
                "<div style='position:absolute;left:100px;top:100px;width:100px;transform:rotateY(180deg);backface-visibility:hidden;transform-style:preserve-3d'>\
                 <div style='height:100px;background:green'></div></div>"
            ),
            scale,
        );
        assert_eq!(pixel(&img, scale, 150.0, 150.0), GREEN, "preserve-3d with backface hidden, scale {scale}");
    }
}

/// A 3D rendering context composes its planes before projecting them: a child turned -60deg in a
/// `preserve-3d` parent turned 60deg faces the viewer at full width (flattened one by one it was a
/// quarter wide), and two turns of 90deg about x make a 180deg flip of a box that an edge-on
/// parent no longer swallows. A grouping property (here `isolation: isolate`) flattens the parent
/// first, as `transform-style: flat` does.
#[test]
fn a_3d_rendering_context_composes_its_planes() {
    let nest = |outer: &str, inner: &str| {
        format!(
            "<div style='position:absolute;left:100px;top:100px;width:200px;height:100px;{outer}'>\
             <div style='width:200px;height:100px;background:green;{inner}'></div></div>"
        )
    };
    for scale in [1.0, 2.0] {
        let img =
            render(&nest("transform-style:preserve-3d;transform:rotateY(60deg)", "transform:rotateY(-60deg)"), scale);
        assert_eq!(
            extent(&img, scale, GREEN, 200.0, 150.0),
            ((100.5, 299.5), (100.5, 199.5)),
            "composed, scale {scale}"
        );
        let img =
            render(&nest("transform-style:preserve-3d;transform:rotateX(90deg)", "transform:rotateX(90deg)"), scale);
        assert_eq!(
            extent(&img, scale, GREEN, 200.0, 150.0),
            ((100.5, 299.5), (100.5, 199.5)),
            "edge-on parent, scale {scale}"
        );
        // Flattened: a quarter of the width about the centre.
        for outer in
            ["transform:rotateY(60deg)", "transform-style:preserve-3d;transform:rotateY(60deg);isolation:isolate"]
        {
            let img = render(&nest(outer, "transform:rotateY(-60deg)"), scale);
            assert_eq!(
                extent(&img, scale, GREEN, 200.0, 150.0),
                ((175.5, 224.5), (100.5, 199.5)),
                "{outer}, scale {scale}"
            );
        }
    }
}

/// A flat box with a 3D transform is rasterized in its own plane and then drawn through its
/// flattened transform, as Chromium composites it: a child that blends with it (`difference`
/// of fuchsia over aqua) gives the same pixels as the blended colour painted without a blend.
/// Each drawn through the transform, the blended child's edge over the parent's edge came out a
/// few levels off the edge between the painted colours (WPT
/// `mix-blend-mode-parent-with-3D-transform`).
///
/// Across the bottom of the viewport too: the blend's group, clipped to the viewport, cut through
/// the pixels of the plane the screenshot's last rows sample, which came out a fifth lighter.
#[test]
fn a_blend_inside_a_flattened_box_matches_its_colour_painted_flat() {
    let parent = "background:aqua;width:140px;height:140px;padding-top:1px;transform:rotateX(20deg)";
    let blended = "background:fuchsia;margin:20px 0 0 20px;width:200px;height:200px";
    for top in [0, 180] {
        for scale in [1.0, 2.0] {
            let test = render(
                &format!(
                    "<div style='margin-top:{top}px;{parent}'>\
                     <div style='{blended};mix-blend-mode:difference'></div></div>"
                ),
                scale,
            );
            let reference = render(
                &format!(
                    "<div style='margin-top:{top}px;{parent}'><div style='{blended}'>\
                     <div style='background:yellow;width:120px;height:120px'></div></div></div>"
                ),
                scale,
            );
            let off = test.data.chunks(4).zip(reference.data.chunks(4)).filter(|(a, b)| a != b).count();
            assert_eq!(off, 0, "pixels off, top {top}, scale {scale}");
        }
    }
}

/// The out-of-flow boxes of a filtered inline (its stacking context) paint at the positioned
/// level, above the blocks after it, as a transformed block's do (WPT
/// `filtered-inline-is-container`): they were painted with the anonymous block that lays them
/// out, under the next block.
#[test]
fn the_positioned_boxes_of_a_filtered_inline_paint_above_later_blocks() {
    for filter in ["filter:blur(0px)", "position:relative"] {
        let img = render(
            &format!(
                "<div style='font:16px/20px sans-serif'><span style='{filter}'>a<div style='position:absolute;left:0;top:0;width:100px;height:100px;background:green'></div></span></div>\
                 <div style='height:300px;margin-top:-100px;background:red'></div>"
            ),
            1.0,
        );
        assert_eq!(pixel(&img, 1.0, 50.0, 50.0), GREEN, "{filter}");
    }
}

/// A fixed box whose containing block is a `backdrop-filter` box (a containing block that is no
/// stacking context in Blitz) is lifted into the root element's paint list and placed by its
/// offset there, not against the viewport (WPT `backdrop-filter-containing-block`).
#[test]
fn a_fixed_box_lifted_from_its_containing_block_keeps_its_place() {
    let img = render(
        "<div style='height:50px'></div><div style='width:200px;backdrop-filter:invert(1)'>\
         <div style='position:fixed;top:0;left:0;width:100px;height:100px;background:green'></div></div>",
        1.0,
    );
    assert_eq!(pixel(&img, 1.0, 50.0, 100.0), GREEN);
    assert_eq!(pixel(&img, 1.0, 50.0, 25.0), WHITE);
}

/// Only an element's children in the DOM take its perspective: an absolutely positioned
/// grandchild it contains does not (WPT `perspective-children-only-abspos`).
#[test]
fn a_perspective_applies_to_the_dom_children_only() {
    let img = render(
        "<div style='position:relative;width:100px;height:100px;background:red;perspective:100px'><div>\
         <div style='position:absolute;left:0;top:0;width:100px;height:100px;background:green;transform:translateZ(-100px)'></div>\
         </div></div>",
        1.0,
    );
    assert_eq!(extent(&img, 1.0, GREEN, 50.0, 50.0), ((0.5, 99.5), (0.5, 99.5)));
}

/// A box whose matrix has no inverse is not rendered, whatever it shows in the plane:
/// `scale: 1 1 0` flattens to the identity.
#[test]
fn a_box_with_a_non_invertible_matrix_is_not_drawn() {
    let img = render(&square("scale:1 1 0"), 1.0);
    assert_eq!(pixel(&img, 1.0, 150.0, 150.0), WHITE);
}

/// A live document (the mirror) and the element boxes layout queries read.
struct LivePage {
    renderer: Renderer,
    tree: Arc<Tree>,
    keys: Vec<u64>,
}

impl LivePage {
    fn new(html: &str, ids: &[&str]) -> LivePage {
        let dom: DomRef = Rc::new(RefCell::new(pyro_dom::Dom::new()));
        let doc = parse_document(&dom, "http://test/", html);
        let d = dom.borrow();
        let keys = ids.iter().map(|id| render_key(d.get_element_by_id(doc, id).unwrap().unwrap())).collect();
        let tree = d
            .render_tree(
                doc,
                &RenderHtml { style_text: &|_| None, stylesheet_href: &|_| None, mark: None, adopted: None },
            )
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
        LivePage { renderer: Renderer::new(), tree: Arc::new(tree), keys }
    }

    fn snapshot(&self, scroll: Option<Vec<(u64, (f64, f64))>>) -> Snapshot {
        Snapshot {
            base_url: "http://test/".into(),
            width: 400,
            height: 300,
            device_scale_factor: 1.0,
            live: Some(Live { page: 1, tree: self.tree.clone(), clip: None, interaction: None }),
            element_scroll: scroll,
            ..Default::default()
        }
    }

    fn render(&mut self, scroll: Option<Vec<(u64, (f64, f64))>>) -> RgbaImage {
        let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
        let snapshot = self.snapshot(scroll);
        self.renderer.render(&snapshot, Area::Viewport, Backend::Cpu, &fetcher).unwrap().0
    }

    fn boxes(&mut self) -> Vec<LayoutBox> {
        let snapshot = self.snapshot(None);
        self.renderer.sync_layout(&snapshot, &(Arc::new(NoFetch) as Arc<dyn Fetcher>)).unwrap();
        self.keys.clone().into_iter().map(|key| self.renderer.element_box(1, key).unwrap()).collect()
    }
}

/// The perspective origin of a scroll container stays put while its content scrolls: a box
/// scrolled into view scales about the container's corner, not the content's (WPT
/// perspective-origin-scroll). The transforms under it follow each new scroll offset.
#[test]
fn a_perspective_origin_does_not_scroll_with_the_content() {
    let html = "<!doctype html><style>body{margin:0} div{position:absolute}</style>\
        <div id=holder style='width:100px;height:100px;overflow:hidden;perspective:2px;perspective-origin:0 0'>\
        <div style='width:100px;height:300px'></div>\
        <div id=kid style='top:100px;width:50px;height:50px;background:green;transform:translateZ(1px)'></div></div>";
    let mut page = LivePage::new(html, &["holder", "kid"]);
    let holder = page.keys[0];
    // At scroll 0 the box (100..150 in the content) scales by 2 about (0, 0): 200..300, out of view.
    let img = page.render(None);
    assert_eq!(pixel(&img, 1.0, 25.0, 25.0), WHITE);
    // Scrolled by 100 it sits at 0..50 in the container and scales to 0..100.
    let img = page.render(Some(vec![(holder, (0.0, 100.0))]));
    assert_eq!(extent(&img, 1.0, GREEN, 50.0, 50.0), ((0.5, 99.5), (0.5, 99.5)));
    // Scrolled by 75: at 25..75, scaled to 50..150, cut at 100.
    let img = page.render(Some(vec![(holder, (0.0, 75.0))]));
    assert_eq!(extent(&img, 1.0, GREEN, 50.0, 75.0), ((0.5, 99.5), (50.5, 99.5)));
}

/// `getBoundingClientRect` and the boxes hit testing reads are the flattened transform's: a
/// `rotateY(60deg)` box half as wide, a box scaled by its container's perspective.
#[test]
fn layout_boxes_follow_the_flattened_transform() {
    let html = "<!doctype html><style>body{margin:0} div{position:absolute}</style>\
        <div id=rot style='left:100px;top:20px;width:100px;height:40px;transform:rotateY(60deg)'></div>\
        <div style='left:100px;top:100px;width:150px;height:150px;perspective:3px'>\
        <div id=near style='left:25px;top:25px;width:100px;height:100px;transform:translateZ(1px)'></div></div>";
    let mut page = LivePage::new(html, &["rot", "near"]);
    let boxes = page.boxes();
    let rect = |b: &LayoutBox| [b.x, b.y, b.width, b.height].map(|v| (v * 100.0).round() / 100.0);
    assert_eq!(rect(&boxes[0]), [125.0, 20.0, 50.0, 40.0]);
    assert_eq!(rect(&boxes[1]), [100.0, 100.0, 150.0, 150.0]);
}

/// A `display: table-cell` child of a `preserve-3d` box gets anonymous table, row group and row
/// boxes around it, which have no `transform-style` of their own: they pass the 3D rendering
/// context on (as Chromium's do), so two `rotateX(90deg)` across them add up to 180deg and the
/// box shows (flipped). Flat anonymous boxes broke the context: each 90deg turn was flattened
/// edge-on, and nothing showed (WPT css-transforms `transform-table-011`).
#[test]
fn anonymous_table_boxes_pass_a_3d_rendering_context_on() {
    let img = render(
        "<div style='margin:50px;transform:rotateX(90deg);transform-style:preserve-3d'>\
         <div style='display:table-cell;transform-style:preserve-3d'>\
         <div style='width:100px;height:100px;background:green;transform:rotateX(90deg)'></div></div></div>",
        1.0,
    );
    let ((x0, x1), _) = extent(&img, 1.0, GREEN, 0.0, 100.0);
    assert!(x0 >= 0.0 && x1 - x0 > 50.0, "the box shows: columns {x0}..{x1}");
}
