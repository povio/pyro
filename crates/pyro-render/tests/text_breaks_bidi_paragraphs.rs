//! Line breaks and bidi paragraphs (reftest lane T2): a preserved newline ends a bidi paragraph
//! but not the `unicode-bidi: bidi-override` of the block (WPT `pre-wrap-align-*-003`); a segment
//! break next to a zero-width space is removed with the white space around it
//! (`seg-break-transformation-018/019`); atomic inlines in `nowrap` spans wrap between the spans in
//! a block that wraps (`line-breaking-030/031/032`). Measured on empty inline-blocks (`b`) in a
//! monospace font.
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

const STYLE: &str = "<!doctype html><style>body{margin:0;font:20px/40px monospace}
  b{display:inline-block;width:20px;height:10px}</style>";

/// Each element's border box `[x, y, width, height]`.
fn boxes(body: &str, ids: &[&str]) -> Vec<[f64; 4]> {
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
fn bidi_override_holds_after_a_preserved_newline() {
    // Reversed, `ab<b>c` puts the box one character from the left; not reversed, two.
    let got = boxes(
        "<div dir=rtl style='white-space:pre;unicode-bidi:bidi-override;width:300px'>ab<b id=a></b>c\nab<b id=b></b>c</div>",
        &["a", "b"],
    );
    assert!(got[1][1] > got[0][1], "{got:?}");
    assert!((got[0][0] - got[1][0]).abs() < 0.5, "{got:?}");
}

#[test]
fn a_segment_break_after_a_zero_width_space_is_removed() {
    let got = boxes(
        "<div><span id=a>aa&#x200b;\n  bb</span></div><div><span id=b>aa<i>&#x200b;</i>\nbb</span></div>\
         <div><span id=c>aabb</span></div><div><span id=d>aa \n bb</span></div>",
        &["a", "b", "c", "d"],
    );
    assert!((got[0][2] - got[2][2]).abs() < 0.5 && (got[1][2] - got[2][2]).abs() < 0.5, "{got:?}");
    assert!(got[3][2] > got[2][2] + 5.0, "a segment break without one is a space: {got:?}");
}

#[test]
fn atomic_inlines_in_nowrap_spans_wrap_between_the_spans() {
    let got = boxes(
        "<div style='width:20px'><span style='white-space:pre'><b id=a></b></span><span style='white-space:pre'><b id=b></b></span></div>\
         <div style='width:20px'><span style='white-space:pre'><b id=c></b></span><span style='white-space:pre'>X<b id=d></b></span></div>\
         <div style='width:20px'><span style='white-space:pre'><b id=e></b><b id=f></b></span></div>",
        &["a", "b", "c", "d", "e", "f"],
    );
    assert!(got[1][1] > got[0][1], "{got:?}");
    assert!(got[3][1] > got[2][1], "{got:?}");
    assert!((got[5][1] - got[4][1]).abs() < 0.5, "no wrap inside one nowrap span: {got:?}");
}
