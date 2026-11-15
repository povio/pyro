//! `unicode-bidi` and `direction` on inline boxes and block containers (CSS Writing Modes 3
//! § 2.4): an inline box's embedding, override or isolate applies to what it holds, as the bidi
//! formatting characters Blink inserts for it would; a block container's `bidi-override` overrides
//! its whole inline content. Atomic inlines are neutrals (U+FFFC), each paragraph is resolved on
//! its own, the white space that ends a line takes the paragraph's level (UAX #9 L1), a span's
//! sides go where its direction puts them, and the inserted characters take no letter spacing and
//! let white space collapse across them. Inline-blocks of 10px make the visual order measurable.
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

#[test]
fn an_override_span_reverses_its_content() {
    let x = x_of(
        "<p><b id=a></b><span style='unicode-bidi:bidi-override;direction:rtl'><b id=b></b><b id=c></b></span><b id=d></b></p>",
        &["a", "b", "c", "d"],
    );
    assert_eq!(x, vec![0.0, 20.0, 10.0, 30.0], "{x:?}");
}

#[test]
fn bdo_overrides_with_its_dir() {
    let x = x_of("<p><bdo dir=rtl><b id=a></b><b id=b></b></bdo></p>", &["a", "b"]);
    assert_eq!(x, vec![10.0, 0.0], "{x:?}");
    // Left-to-right letters too: `bdo[dir]` is an override, not only the isolate of `[dir]`.
    let x = x_of("<p><bdo dir=rtl><span id=a>a</span><span id=b>b</span></bdo></p>", &["a", "b"]);
    assert!(x[1] < x[0], "{x:?}");
}

#[test]
fn an_embedding_in_an_rtl_paragraph_keeps_its_ltr_runs_together() {
    // In a right-to-left paragraph, "a b" then the embedding "c d": the embedding is a level-2
    // run of its own, placed left of what precedes it; inside it c stays left of d.
    let x = x_of(
        "<p dir=rtl style='width:200px'><b id=a></b><span style='unicode-bidi:embed;direction:ltr'><b id=c></b><b id=d></b></span></p>",
        &["a", "c", "d"],
    );
    assert!(x[2] > x[1] && x[0] > x[2], "{x:?}");
}

#[test]
fn a_blocks_bidi_override_overrides_its_inline_content() {
    let x = x_of(
        "<p style='unicode-bidi:bidi-override;direction:rtl;width:100px'><b id=a></b><b id=b></b></p>",
        &["a", "b"],
    );
    assert_eq!(x, vec![90.0, 80.0], "{x:?}");
}

#[test]
fn an_isolate_does_not_change_an_ltr_paragraph_without_rtl_content() {
    let x = x_of("<p><b id=a></b><span dir=ltr><b id=b></b></span><b id=c></b></p>", &["a", "b", "c"]);
    assert_eq!(x, vec![0.0, 10.0, 20.0], "{x:?}");
}

#[test]
fn an_atomic_inline_is_a_neutral_between_its_neighbours() {
    // Resolved as U+FFFC between left-to-right "abc" and right-to-left text in a right-to-left
    // paragraph, the box takes the paragraph's direction and goes left of "abc" (Chrome: 154 and
    // 164). It took the level of the text before it, right of "abc".
    let x = x_of("<p dir=rtl style='width:200px'><span id=t>abc</span><b id=b></b>\u{5d0}\u{5d1}</p>", &["t", "b"]);
    assert!(x[1] < x[0], "{x:?}");
}

#[test]
fn a_lines_trailing_space_takes_the_paragraphs_level() {
    // UAX #9 L1 on each line: the space that ends the first line is right-to-left, at the line's
    // left end, so both lines' "aaa" are flush right (Chrome: 24 and 24). It was reordered with
    // the left-to-right text, right of the first "aaa". The space hangs at the left, so the first
    // "aaa" keeps its whole width (its box lost the space's width at its right).
    let r = rects("<p dir=rtl style='width:5ch'><span id=a>aaa</span> <span id=b>aaa</span></p>", &["a", "b"]);
    assert_eq!((r[0][0], r[0][2]), (r[1][0], r[1][2]), "{r:?}");
}

#[test]
fn each_paragraph_is_resolved_on_its_own() {
    // A preserved line feed ends a paragraph (UAX #9 P1): the "!" ending the first one is between
    // right-to-left text and the paragraph's end, so it is left-to-right, right of the Hebrew
    // (Chrome: 0 and 36). Resolved with the next paragraph's Hebrew, it went left of it.
    let x = x_of(
        "<p style='white-space:pre'><span id=a>\u{5d0}\u{5d1}</span> <span id=x>!</span>\n\u{5d0}\u{5d1}</p>",
        &["a", "x"],
    );
    assert!(x[1] > x[0], "{x:?}");
}

#[test]
fn bidi_formatting_characters_take_no_letter_spacing_and_no_white_space() {
    // The isolate's characters (RLI, PDI) add no letter spacing, and the spaces on both sides of
    // the first collapse into one, as without the isolate.
    let spaced = |span: &str| {
        x_of(&format!("<p dir=rtl style='width:300px;letter-spacing:10px'><b id=a></b><span{span}><b id=b></b></span></p>"), &["a", "b"])
    };
    assert_eq!(spaced(" style='unicode-bidi:isolate'"), spaced(""));
    let collapsed = |span: &str| x_of(&format!("<p dir=rtl style='width:300px'><b id=a></b> <span{span}> <b id=b></b></span></p>"), &["a", "b"]);
    assert_eq!(collapsed(" style='unicode-bidi:isolate'"), collapsed(""));
}

#[test]
fn a_right_to_left_spans_start_side_is_its_right() {
    // The span's right border is its start side (CSS 2.1 § 8.6): right of its left-to-right text,
    // at the line's right end (Chrome: the text ends at 290).
    let r = rects("<p dir=rtl style='width:300px'><span style='border-right:10px solid blue'><span id=t>abc</span></span></p>", &["t"]);
    assert_eq!(r[0][0] + r[0][2], 290.0, "{r:?}");
}

#[test]
fn an_absolutely_positioned_blocks_static_position_follows_the_logical_order() {
    // After the text in a right-to-left line, the block's static position is below the line; at
    // its start, the line's top (Chrome: 40 and 0).
    let after = rects("<div dir=rtl style='width:200px'>abc<div id=a style='position:absolute;width:10px;height:10px'></div></div>", &["a"]);
    assert_eq!(after[0][1], 40.0, "{after:?}");
    let before = rects("<div dir=rtl style='width:200px'><div id=a style='position:absolute;width:10px;height:10px'></div>abc</div>", &["a"]);
    assert_eq!(before[0][1], 0.0, "{before:?}");
}
