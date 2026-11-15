//! Inline layout leftovers (reftest lane P5): out-of-flow boxes whose containing block is a
//! positioned inline box, the edges of inline boxes (a right-to-left box's sides, sides whose
//! margin, border and padding add up to nothing, percentages), inline boxes split around blocks
//! (percentage heights, the containing block's direction, a block-level `::after`), `text-indent`
//! in anonymous blocks, white space after a `<br>`, the baseline a scroll container gives its
//! inline-block, `visibility` on descendants, and in-flow blocks that overlap (CSS 2.1 Appendix E).
//! Inline-blocks of 10px (`b`) make positions measurable; lines are 40px, the font 20px monospace.
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

const STYLE: &str = "<!doctype html><style>body{margin:0;background:white;font:20px/40px monospace;color:black}
  p{margin:0} b{display:inline-block;width:10px;height:10px}</style>";

fn x_of(body: &str, ids: &[&str]) -> Vec<f64> {
    rects(body, ids).iter().map(|r| r[0]).collect()
}

/// Each element's border box: `[x, y, width, height]`.
fn rects(body: &str, ids: &[&str]) -> Vec<[f64; 4]> {
    let dom: DomRef = Rc::new(RefCell::new(pyro_dom::Dom::new()));
    let doc = parse_document(&dom, "http://test/", &format!("{STYLE}{body}"));
    let d = dom.borrow();
    let keys: Vec<_> = ids.iter().map(|id| render_key(d.get_element_by_id(doc, id).unwrap().unwrap())).collect();
    let tree =
        d.render_tree(doc, &RenderHtml { style_text: &|_| None, stylesheet_href: &|_| None, mark: None, adopted: None }).unwrap();
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
        width: 600,
        height: 300,
        device_scale_factor: 1.0,
        live: Some(Live { page: 1, tree: Arc::new(tree), clip: None, interaction: None }),
        ..Default::default()
    };
    let mut renderer = Renderer::new();
    renderer.sync_layout(&snap, &(Arc::new(NoFetch) as Arc<dyn Fetcher>)).unwrap();
    keys.iter()
        .map(|&key| {
            let b = renderer.element_box(1, key).unwrap();
            [b.x, b.y, b.width, b.height]
        })
        .collect()
}


fn render(body: &str) -> RgbaImage {
    let fetcher: Arc<dyn Fetcher> = Arc::new(NoFetch);
    let snapshot = Snapshot {
        html: format!("<!doctype html><style>body{{margin:0;background:white}}</style>{body}"),
        base_url: "http://test/".into(),
        width: 300,
        height: 200,
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
const WHITE: [u8; 3] = [255, 255, 255];

#[test]
fn an_rtl_spans_only_edge_goes_to_its_side_of_the_content() {
    // The span's left margin is its end edge (it is right-to-left): left of its content, also
    // when the text before it and its own are shaped as one run (Chrome: 50; it was 10, the
    // margin after the content).
    let x = x_of("<p><b id=a></b><span style='direction:rtl;margin-left:40px'><b id=b></b></span></p>", &["a", "b"]);
    assert_eq!(x, vec![0.0, 50.0], "{x:?}");
    let x = x_of("<p>ab<span style='direction:rtl;padding-left:40px'>cd<b id=b></b></span></p>", &["b"]);
    // "ab", the padding, "cd" (12px a character).
    assert_eq!(x, vec![88.0], "{x:?}");
}

#[test]
fn an_abspos_box_in_a_positioned_span_is_placed_against_the_spans_box() {
    // The span's padding box starts at its first content (x 10) moved by its relative offset
    // (30): `left: 0` is there, `right: 0` at its end (Chrome: 40 and 45). The inline root's
    // box was the containing block (0 and 195).
    let r = rects(
        "<p style='width:200px'><b></b><span style='position:relative;left:30px'><b></b>\
         <i id=l style='position:absolute;left:0;top:0;width:5px;height:5px'></i>\
         <i id=r style='position:absolute;right:0;bottom:0;width:5px;height:5px'></i></span></p>",
        &["l", "r"],
    );
    assert_eq!((r[0][0], r[1][0]), (40.0, 45.0), "{r:?}");
    assert!(r[1][1] > r[0][1], "bottom: 0 is below top: 0 in the span's box: {r:?}");
}

#[test]
fn an_abspos_box_deeper_in_a_positioned_span_is_placed_against_the_span() {
    // Not a child of the span: in an inline-block inside it (WPT `abspos-inline-007`).
    let x = x_of(
        "<p style='width:200px'><b></b><span style='position:relative'><b></b>\
         <b style='width:30px'><i id=r style='position:absolute;right:0;top:0;width:5px;height:5px'></i></b></span><b></b></p>",
        &["r"],
    );
    assert_eq!(x, vec![45.0], "{x:?}");
}

#[test]
fn a_static_position_moves_with_its_relatively_positioned_span() {
    // WPT `abspos-inline-008`: the box is where it would be in the span, which is moved.
    let r = rects(
        "<p style='width:200px;margin-left:100px'><span style='position:relative;left:-100px;top:7px'>\
         <i id=x style='position:absolute;display:inline-block;width:5px;height:5px'></i></span></p>",
        &["x"],
    );
    assert_eq!((r[0][0], r[0][1]), (0.0, 7.0), "{r:?}");
}

#[test]
fn a_side_that_adds_up_to_nothing_is_still_an_edge() {
    // A border and a negative margin as wide: the line is no empty line (CSS 2.1 § 9.4.2), so
    // a block-level box after the edge would start below it (WPT `static-inside-inline-002`),
    // and a line of only such an edge has its height (WPT `margin-right-114`).
    let r = rects(
        "<div style='width:200px'><span style='border-left:10px solid;margin-left:-10px'>\
         <div id=x style='position:absolute;width:5px;height:5px'></div>X</span></div><div id=n></div>",
        &["x", "n"],
    );
    assert_eq!((r[0][1], r[1][1]), (40.0, 40.0), "{r:?}");
    let r = rects(
        "<div style='width:200px'><span style='border-right:10px solid;margin-right:-10px'><div></div></span></div><div id=n></div>",
        &["n"],
    );
    assert_eq!(r[0][1], 40.0, "the tail's line: {r:?}");
}

#[test]
fn a_spans_percentage_padding_is_of_its_containing_blocks_width() {
    // WPT `c547-indent-001`'s reference (Chrome: 100). It resolved against zero.
    let x = x_of("<div style='width:200px'><span style='padding-left:50%'><b id=b></b></span></div>", &["b"]);
    assert_eq!(x, vec![100.0], "{x:?}");
    // Not in the intrinsic width: the container is as wide as the content alone.
    let r = rects("<div id=d style='display:inline-block'><span style='padding-left:50%'><b></b></span></div>", &["d"]);
    assert_eq!(r[0][2], 10.0, "{r:?}");
}

#[test]
fn a_block_in_an_inline_resolves_against_the_inlines_containing_block() {
    // Percentage heights (WPT `block-in-inline-percents-001`) and the direction that decides
    // which margin an over-constrained block drops (WPT `block-in-inline-margins-001b`).
    let r = rects("<div style='height:200px'><span><span id=k style='display:block;height:50%'></span></span></div>", &["k"]);
    assert_eq!(r[0][3], 100.0, "{r:?}");
    let r = rects(
        "<div style='width:100px'><span style='direction:rtl'>\
         <span id=k style='display:block;width:80px;height:5px;margin:10px'></span></span></div>",
        &["k"],
    );
    assert_eq!(r[0][0], 10.0, "{r:?}");
}

#[test]
fn a_block_level_after_splits_its_inline() {
    // WPT `block-in-inline-insert-016b`: "Two" is a block after "One", so two lines.
    let r = rects("<style>#i:after{display:block;content:'Two'}</style><div><span id=i>One</span></div><div id=n></div>", &["n"]);
    assert_eq!(r[0][1], 80.0, "{r:?}");
}

#[test]
fn text_indent_skips_anonymous_blocks_that_are_not_first() {
    // CSS Text 3 § 8.1 (WPT `text-indent-014`): the text after a block child is not indented.
    let x = x_of("<div style='text-indent:60px;width:300px'><b id=a></b><div></div><b id=c></b></div>", &["a", "c"]);
    assert_eq!(x, vec![60.0, 0.0], "{x:?}");
}

#[test]
fn white_space_after_an_inline_block_that_follows_a_br_is_kept() {
    // WPT `vertical-align-122`: the space between the boxes of the second line (12px) was
    // dropped as white space after a line break.
    let x = x_of("<p><b></b> <b></b><br> <b id=c></b> <b id=d></b></p>", &["c", "d"]);
    assert_eq!(x, vec![0.0, 22.0], "{x:?}");
}

#[test]
fn a_scroll_container_gives_its_inline_block_its_bottom_margin_edge() {
    // WPT `baseline-block-with-overflow-001`: the inline-block's baseline is the bottom margin
    // edge of its last block when that clips, not the baseline of the text in it.
    let r = rects(
        "<p><span style='display:inline-block;padding-bottom:20px'>\
         <span id=in style='display:block;width:30px;height:30px;margin-bottom:6px;overflow:hidden'>XX</span></span><b id=m></b></p>",
        &["in", "m"],
    );
    assert_eq!(r[1][1] + r[1][3], r[0][1] + r[0][3] + 6.0, "{r:?}");
}

#[test]
fn an_inline_level_abspos_box_after_inline_content_is_on_its_line() {
    // WPT `remove-block-between-inline-and-abspos`: it joins the anonymous block of the
    // inline content before it, so its static position is after that content.
    let r = rects(
        "<div style='width:200px'><div style='height:10px'></div><b></b>\
         <span id=x style='position:absolute;width:5px;height:5px'></span></div>",
        &["x"],
    );
    assert_eq!(r[0][0], 10.0, "{r:?}");
    assert!(r[0][1] >= 10.0 && r[0][1] < 50.0, "{r:?}");
}

#[test]
fn attr_names_an_html_attribute_in_any_case() {
    // WPT `content-attr-case-001`.
    let r = rects("<style>#s:before{content:attr(Title)}</style><div id=s title='PASS' style='display:inline-block'></div>", &["s"]);
    assert!(r[0][2] > 0.0, "{r:?}");
}

#[test]
fn a_visible_descendant_of_a_hidden_element_paints() {
    // CSS 2.1 § 11.2 (WPT `visibility-005`): the hidden block's background and its own text do
    // not paint; the visible span's and the visible child block's do.
    let img = render(
        "<div style='visibility:hidden;background:red;font:20px/20px monospace;color:red'>\
         <span style='visibility:visible;background:green;color:green'>XX</span> XX\
         <div style='visibility:visible;background:green;height:20px;width:20px'></div></div>",
    );
    assert_eq!(pixel(&img, 5, 10), GREEN);
    assert_eq!(pixel(&img, 5, 30), GREEN);
    assert_eq!(pixel(&img, 60, 10), WHITE, "the hidden block's background and text");
    assert_eq!(pixel(&img, 100, 30), WHITE);
    // A hidden span in a visible block (WPT `position-relative-032`'s reference).
    let img = render("<div style='font:20px/20px monospace'><span style='visibility:hidden;background:red;color:red'>XX</span></div>");
    assert_eq!(pixel(&img, 5, 10), WHITE);
}

#[test]
fn an_inline_block_paints_over_the_background_of_a_later_block() {
    // CSS 2.1 Appendix E (WPT `inline-block-zorder-001`, `position-absolute-007`): block
    // backgrounds first, then inline content, also where blocks overlap without floats.
    let img = render(
        "<div style='height:20px'><span style='display:inline-block;vertical-align:top;width:40px;height:20px;background:green'></span></div>\
         <div style='margin-top:-20px;height:20px;width:40px;background:red'></div>",
    );
    assert_eq!(pixel(&img, 10, 10), GREEN);
    // Text overflowing a block of fixed height paints over the next block's background.
    let img = render(
        "<div style='height:10px;font:20px/20px monospace;color:green'>\u{2588}\u{2588}</div>\
         <div style='height:20px;width:40px;background:red'></div>",
    );
    assert_eq!(pixel(&img, 6, 15), GREEN);
    assert_eq!(pixel(&img, 35, 28), RED);
}

#[test]
fn a_spans_edges_go_beside_content_an_override_in_its_text_reorders() {
    // WPT `bidi-011`: U+202E written in the span reverses "TSET", and "ST" after the span (in a
    // U+202D inside that override) goes left of it. The span's start edge goes left of "TSET",
    // not where the zero-width U+202E itself was left (before "ST").
    let x = x_of(
        "<div style='width:600px'>TE<span style='border:2px solid;padding:0 10px;margin:0 10px'>\u{202E}\
         <i id=c>TSET</i></span>\u{202D}<i id=d>ST</i> </div>",
        &["c", "d"],
    );
    assert_eq!(x, vec![70.0, 24.0], "{x:?}");
}

#[test]
fn display_math_computes_to_flow() {
    // CSS Display 4 (WPT `display-math-on-pseudo-elements-002`): `block math` on an element that
    // is not MathML is a block, `math` an inline. The declaration was dropped (both inline).
    let r = rects(
        "<style>#i:after{display:block math;content:'Two'} #j:after{display:math;content:'Two'}</style>\
         <div><span id=i>One</span></div><div id=n><span id=j>One</span></div><div id=m></div>",
        &["n", "m"],
    );
    assert_eq!((r[0][1], r[1][1]), (80.0, 120.0), "{r:?}");
}

#[test]
fn a_static_position_in_vertical_lines_is_on_its_line() {
    // WPT css-position `static-position/vlr-ltr-ltr`: in a vertical writing mode the box stays
    // where its line puts it, also in a relatively positioned span (which the turned lines do
    // not move): 3 + 2 characters of 20px down the line.
    let r = rects(
        "<div style='writing-mode:vertical-lr;height:300px;font:20px/20px monospace'>XXX<span style='position:relative;inset-block-start:2px;inset-inline-start:2px'>XX\
         <i id=x style='position:absolute;width:5px;height:5px'></i>XX</span></div>",
        &["x"],
    );
    assert_eq!(r[0][1], 60.0, "{r:?}");
}
