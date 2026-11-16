//! White space hanging at a line's end and the boxes around it (reftest lane T): preserved
//! spaces and ideographic spaces that hang are content of their inline boxes (a background shows
//! behind them, WPT `trailing-ideographic-space-*`); collapsible spaces that hang are in no box.
//! Measured on an inline box (`u`) in a monospace font.
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
  div{width:2.5ch}</style>";

/// Each element's `[width, height]`.
fn sizes(body: &str, ids: &[&str]) -> Vec<[f64; 2]> {
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
            [b.width, b.height]
        })
        .collect()
}

#[test]
fn preserved_and_ideographic_spaces_that_hang_are_in_their_boxes() {
    let s = sizes(
        "<div style='white-space:pre-wrap'>ab<u id=a>  </u>cd</div>\
         <div>ab<u id=b>&#x3000;</u>cd</div>\
         <div>ab<u id=c> </u>cd</div>\
         <div>ab<u id=d>x </u>cd</div><u id=x>x</u>",
        &["a", "b", "c", "d", "x"],
    );
    let ch = s[4][0];
    // Two preserved spaces past the end of a 2.5ch line: both in the box.
    assert!((s[0][0] - 2.0 * ch).abs() < 0.2, "{s:?}");
    // An ideographic space is not collapsible: it hangs and keeps its box.
    assert!(s[1][0] > 0.5 * ch, "{s:?}");
    // A collapsible space at the end of a line is in no box.
    assert_eq!(s[2][0], 0.0);
    assert!((s[3][0] - ch).abs() < 0.2, "{s:?}");
}
