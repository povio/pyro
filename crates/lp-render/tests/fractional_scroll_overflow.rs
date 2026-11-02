//! A fractional HS chip label must not gain overflow from paint-box rounding.
//! Matched native/Chrome proof: /tmp/hs-fractional-overflow-*-before.log.
use std::{cell::RefCell, rc::Rc, sync::Arc};

use lp_dom::{
    parse::{parse_document, DomRef},
    snapshot::{render_key, RenderHtml, RenderNodeKind},
};
use lp_render::*;

struct NoFetch;
impl Fetcher for NoFetch {
    fn fetch(&self, _: String, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        done(Err("offline".into()));
    }
}

fn boxes(html: &str, ids: &[&str]) -> Vec<LayoutBox> {
    use_fork_safe_fonts();
    let dom: DomRef = Rc::new(RefCell::new(lp_dom::Dom::new()));
    let doc = parse_document(&dom, "http://test/", html);
    let d = dom.borrow();
    let keys: Vec<_> = ids.iter().map(|id| render_key(d.get_element_by_id(doc, id).unwrap().unwrap())).collect();
    let tree =
        d.render_tree(doc, &RenderHtml { style_text: &|_| None, stylesheet_href: &|_| None, mark: None }).unwrap();
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
        width: 500,
        height: 200,
        device_scale_factor: 1.0,
        live: Some(Live { page: 1, tree: Arc::new(tree), clip: None }),
        ..Default::default()
    };
    let mut renderer = Renderer::new();
    renderer.sync_layout(&snap, &(Arc::new(NoFetch) as Arc<dyn Fetcher>)).unwrap();
    keys.into_iter().map(|key| renderer.element_box(1, key).unwrap()).collect()
}

#[test]
fn fitting_fractional_inline_boxes_have_equal_client_and_scroll_extents() {
    let html = r#"<!doctype html><style>
      body{margin:0}.group{position:relative;left:100.140625px;display:flex;gap:4px}
      .chip{display:inline-flex;gap:2px;padding-left:4px}.label{overflow:hidden;white-space:nowrap;line-height:20px}
      i{display:inline-block;width:46.453125px;height:14px}
      </style><div class=group><div class=chip><span id=fractional class=label><i></i></span></div></div>
      <div class=group style=left:0><div class=chip><span id=integer class=label><i></i></span></div></div>"#;
    for node in boxes(html, &["fractional", "integer"]) {
        assert_eq!(node.width, 46.453125);
        assert_eq!(node.scroll_width, node.client_width, "fitting box gained rounded paint overflow: {node:?}");
    }
}

#[test]
fn real_descendant_overflow_is_retained_and_clipped_before_reaching_ancestors() {
    let html = r#"<!doctype html><style>
      body{margin:0}.outer{position:relative;left:100.140625px;width:80px}
      .label{width:46.453125px;height:20px;overflow:hidden;white-space:nowrap}
      i{display:inline-block;width:120px;height:14px}
      </style><div id=outer class=outer><div id=clipped class=label><i></i></div></div>
      <div class=outer><div id=visible class=label style=overflow:visible><i></i></div></div>"#;
    let result = boxes(html, &["outer", "clipped", "visible"]);
    assert_eq!(result[0].scroll_width, result[0].client_width, "clipped child overflow leaked to ancestor");
    for node in &result[1..] {
        assert!(node.scroll_width >= 120.0, "real descendant overflow lost: {node:?}");
        assert!(node.scroll_width > node.client_width);
    }
}

#[test]
fn real_nowrap_text_overflow_remains_observable() {
    let html = r#"<!doctype html><style>
      body{margin:0}.label{position:relative;left:100.140625px;width:46.453125px;overflow:hidden;white-space:nowrap;font:13px/20px Arial}
      </style><div id=text class=label>990291990291990291990291</div>"#;
    let result = boxes(html, &["text"]);
    assert!(result[0].scroll_width > 100.0, "real text overflow lost: {:?}", result[0]);
    assert!(result[0].scroll_width > result[0].client_width);
}
