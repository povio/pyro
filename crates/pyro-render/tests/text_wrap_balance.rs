//! `text-wrap: balance` (reftest lane T). A balanced block wraps
//! its lines at the least width that keeps their count, never by breaking a word that
//! `overflow-wrap` could, and a block with more lines than Blink balances (six) is left alone.
//! Inline-blocks of 40px (`b`) are the words, so no result depends on a font.
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
  div{width:300px} b{display:inline-block;width:40px;height:10px}</style>";

/// Each element's `[x, width]`.
fn boxes(body: &str, ids: &[&str]) -> Vec<[f64; 2]> {
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
            [b.x, b.width]
        })
        .collect()
}

#[test]
fn balanced_lines_share_the_words() {
    // Seven 40px words with spaces: unbalanced, six fit a 300px line and one wraps; balanced,
    // the first line holds four.
    let words = |id: &str| format!("<b></b> <b></b> <b></b> <b></b> <b id={id}></b> <b></b> <b></b>");
    let x = boxes(
        &format!("<div>{}</div><div style='text-wrap:balance'>{}</div>", words("a"), words("b")),
        &["a", "b"],
    );
    assert!(x[0][0] > 100.0, "{x:?}");
    assert_eq!(x[1][0], 0.0);
}

#[test]
fn balance_leaves_long_blocks_and_unbreakable_words_alone() {
    // Seven lines: more than is balanced.
    let lines: String = (0..7).map(|i| format!("<b style='width:200px'></b> <b id=w{i} style='width:60px'></b> ")).collect();
    let x = boxes(&format!("<div style='text-wrap:balance'>{lines}</div>"), &["w0", "w6"]);
    assert!(x[0][0] > 200.0 && x[1][0] > 200.0, "{x:?}");
    // One word and a short one: no narrower width keeps two lines without breaking the word.
    let x = boxes(
        "<div style='text-wrap:balance;overflow-wrap:break-word;font:20px monospace;width:15ch'>\
         <u id=a>CONTROLLING</u> YOUR BU</div><u id=c style='font:20px monospace'>CONTROLLING</u>",
        &["a", "c"],
    );
    assert!((x[0][1] - x[1][1]).abs() < 0.5, "{x:?}");
}
