//! Boxes composed down containing-block chains: relative offsets, CSS
//! transforms (translation in device px in Blitz), out-of-flow boxes whose
//! containing block is not their parent, and fixed boxes, at device scale
//! factors 1 and 2.
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

fn boxes(html: &str, ids: &[&str], scale: f64) -> Vec<LayoutBox> {
    let dom: DomRef = Rc::new(RefCell::new(pyro_dom::Dom::new()));
    let doc = parse_document(&dom, "http://test/", html);
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
        width: 400,
        height: 300,
        device_scale_factor: scale,
        live: Some(Live { page: 1, tree: Arc::new(tree), clip: None, interaction: None }),
        ..Default::default()
    };
    let mut renderer = Renderer::new();
    renderer.sync_layout(&snap, &(Arc::new(NoFetch) as Arc<dyn Fetcher>)).unwrap();
    keys.into_iter().map(|key| renderer.element_box(1, key).unwrap()).collect()
}

const HTML: &str = r#"<!doctype html><style>body{margin:0}div{box-sizing:border-box}</style>
<div id=outer style="position:relative;left:10px;top:20px;width:100px;height:50px;transform:translate(5px,7px)">
  <div style="height:8px"><div id=abs style="position:absolute;left:3px;top:4px;width:10px;height:10px"><div id=deep style="margin-top:2px;height:3px"></div></div></div>
  <div id=fixed style="position:fixed;left:6px;top:5px;width:7px;height:9px"></div>
</div>
<div id=scaled style="width:20px;height:20px;transform:scale(2)"><div id=corner style="width:10px;height:10px"></div></div>"#;

#[test]
fn boxes_compose_down_the_containing_block_chain() {
    let ids = ["outer", "abs", "deep", "fixed", "scaled", "corner"];
    for scale in [1.0, 2.0] {
        let got: Vec<_> = boxes(HTML, &ids, scale).iter().map(|b| (b.x, b.y, b.width, b.height, b.fixed)).collect();
        assert_eq!(
            got,
            vec![
                (15.0, 27.0, 100.0, 50.0, false),
                (18.0, 31.0, 10.0, 10.0, false),
                (18.0, 33.0, 10.0, 3.0, false),
                // The transform makes `outer` the fixed box's containing block.
                (21.0, 32.0, 7.0, 9.0, true),
                (-10.0, 40.0, 40.0, 40.0, false),
                (-10.0, 40.0, 20.0, 20.0, false),
            ],
            "device scale factor {scale}"
        );
    }
}
