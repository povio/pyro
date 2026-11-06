//! Seven-element reproduction matched against Chrome150. Native before fix expands
//! a156.1875px flex item to181px by retaining a153px percentage-width input.
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
    let dom: DomRef = Rc::new(RefCell::new(lp_dom::Dom::new()));
    let doc = parse_document(&dom, "http://test/", html);
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
        width: 500,
        height: 200,
        device_scale_factor: 1.0,
        live: Some(Live { page: 1, tree: Arc::new(tree), clip: None, interaction: None }),
        ..Default::default()
    };
    let mut renderer = Renderer::new();
    renderer.sync_layout(&snap, &(Arc::new(NoFetch) as Arc<dyn Fetcher>)).unwrap();
    keys.into_iter().map(|key| renderer.element_box(1, key).unwrap()).collect()
}

fn fixture(extra: &str) -> String {
    format!(
        r#"<!doctype html><style>*{{box-sizing:border-box}}body{{font:500 14px/20px system-ui}}section{{display:flex;width:156.1875px;margin:20px}}.item{{flex:1;min-width:min-content}}.row{{display:flex;width:100%;gap:8px}}.frame{{display:flex;width:100%;min-width:0}}.inner{{display:flex;width:100%}}input{{width:100%;min-width:0;font:inherit;border:0;padding:0}}button{{width:20px;min-width:20px;height:20px;border:0;padding:0}}{extra}</style><section id=container><div id=item class=item><div class=row><div class=frame><div class=inner><input id=input></div></div><button id=button></button></div></div></section>"#
    )
}

#[test]
fn cyclic_percentage_input_does_not_widen_min_content_flex_item() {
    let b = boxes(&fixture(""), &["container", "item", "input", "button"]);
    assert_eq!(b[0].width, 156.1875);
    assert_eq!(b[1].width, 156.1875, "percentage input min-content contribution: {b:?}");
    assert_eq!(b[2].width, 128.1875, "remaining space after20px button and8px gap");
    assert_eq!(b[3].x + b[3].width, b[0].x + b[0].width);
}

#[test]
fn auto_and_max_content_inputs_keep_intrinsic_width_and_real_overflow() {
    // Compare intrinsic controls to each other, not to a font-dependent magic
    // number. Width:auto and max-content must continue to exceed this container.
    let auto = boxes(&fixture("input{width:auto}"), &["container", "item", "input"]);
    let max = boxes(&fixture(".item{min-width:max-content}"), &["container", "item", "input"]);
    let max_percent = boxes(&fixture("input{width:auto;max-width:100%}"), &["container", "item", "input"]);
    assert!(auto[1].width > auto[0].width, "true intrinsic overflow: {auto:?}");
    assert!(auto[2].width > 128.1875);
    assert_eq!(max[1].width, auto[1].width);
    assert_eq!(max[2].width, auto[2].width);
    assert_eq!(max_percent[1].width, auto[1].width);
}

#[test]
fn input_explicit_minimum_and_fixed_width_remain_authoritative() {
    let min = boxes(&fixture("input{min-width:200px}"), &["container", "item", "input", "button"]);
    assert_eq!(min[2].width, 200.0);
    assert_eq!(min[1].width, 228.0);
    assert!(min[3].x + min[3].width > min[0].x + min[0].width);
    let fixed = boxes(&fixture("input{width:100px}"), &["item", "input"]);
    assert_eq!(fixed[0].width, 156.1875);
    assert_eq!(fixed[1].width, 100.0);
}
