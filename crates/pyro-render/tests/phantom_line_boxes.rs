//! Phantom line boxes (CSS Inline 3 § 2.1, csswg-drafts#9344): a line holding only empty inline
//! boxes is phantom, so margins collapse through its block, unless one of those boxes has a
//! margin, border or padding on an inline-axis side, also when they add up to zero.
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
  .w{display:flow-root;width:50px} .w>div{line-height:0}
  .w>div::after{content:'';display:flow-root;margin-top:50px}</style>";

/// Each element's border box: `[x, y, width, height]`.
fn rects(body: &str, ids: &[&str]) -> Vec<[f64; 4]> {
    let dom: DomRef = Rc::new(RefCell::new(pyro_dom::Dom::new()));
    let doc = parse_document(&dom, "http://test/", &format!("{STYLE}{body}"));
    let d = dom.borrow();
    let keys: Vec<_> = ids.iter().map(|id| render_key(d.get_element_by_id(doc, id).unwrap().unwrap())).collect();
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
        width: 600,
        height: 600,
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

/// The `[top within its wrapper, height]` of a `line-height: 0` block holding `span` and a
/// block-level `::after` with a 50px top margin: `[50, 0]` when the margin collapses through the
/// span's line (phantom), `[0, 50]` when the line keeps it inside the block.
fn block(span: &str) -> [f64; 2] {
    let body = format!("<div class=w id=w><div id=d>{span}</div></div>");
    let r = rects(&body, &["w", "d"]);
    [r[1][1] - r[0][1], r[1][3]]
}

#[test]
fn an_empty_span_without_inline_edges_is_a_phantom_line() {
    assert_eq!(block("<span></span>"), [50.0, 0.0]);
    // Block-axis sides do not count.
    assert_eq!(block("<span style='padding-top:1px'></span>"), [50.0, 0.0]);
    assert_eq!(block("<span style='border-bottom:1px solid transparent'></span>"), [50.0, 0.0]);
    assert_eq!(block("<span style='margin-top:1px;margin-bottom:-1px'></span>"), [50.0, 0.0]);
    // A zero side is no edge.
    assert_eq!(block("<span style='padding-left:0;margin-right:0'></span>"), [50.0, 0.0]);
}

#[test]
fn an_inline_axis_margin_border_or_padding_keeps_the_line() {
    assert_eq!(block("<span style='padding-left:1px'></span>"), [0.0, 50.0]);
    assert_eq!(block("<span style='border-right:1px solid transparent'></span>"), [0.0, 50.0]);
    assert_eq!(block("<span style='margin-left:-1px'></span>"), [0.0, 50.0]);
    // Sides that add up to zero still make edges.
    assert_eq!(block("<span style='margin-left:1px;margin-right:-1px'></span>"), [0.0, 50.0]);
    assert_eq!(block("<span style='padding-right:1px;margin-right:-1px'></span>"), [0.0, 50.0]);
    // A nested empty span's edge counts too.
    assert_eq!(block("<span><span style='padding-right:1px'></span></span>"), [0.0, 50.0]);
}
