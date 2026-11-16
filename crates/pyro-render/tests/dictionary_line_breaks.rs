//! Line breaks inside Thai words (reftest lane T): ICU4X's dictionaries find the word boundaries
//! of Thai, Lao, Khmer and Burmese text, where a line may break; `word-break: manual` keeps such
//! text together. A block's height counts its 40px lines, whatever the machine's fonts.
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
  div{width:0}</style>";

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
fn thai_text_breaks_at_dictionary_words_unless_word_break_is_manual() {
    // "Bangkok is beautiful": several words, no spaces.
    let text = "\u{e01}\u{e23}\u{e38}\u{e07}\u{e40}\u{e17}\u{e1e}\u{e04}\u{e37}\u{e2d}\u{e2a}\u{e27}\u{e22}\u{e07}\u{e32}\u{e21}";
    let s = sizes(
        &format!("<div id=a lang=th>{text}</div><div id=b lang=th style='word-break:manual'>{text}</div>"),
        &["a", "b"],
    );
    assert!(s[0][1] >= 80.0, "{s:?}");
    assert_eq!(s[1][1], 40.0);
}
