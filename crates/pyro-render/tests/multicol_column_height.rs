//! css-multicol-2 `column-height` and `column-wrap`: a fixed column height, columns wrapping into
//! rows `row-gap` apart, the container as tall as its rows; `column-height` alone makes a
//! multi-column container, and the `columns` shorthand takes it after a slash.
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

fn fetcher() -> Arc<dyn Fetcher> {
    Arc::new(NoFetch)
}

fn boxes(html: &str, ids: &[&str]) -> Vec<[f64; 4]> {
    use_fork_safe_fonts();
    let dom: DomRef = Rc::new(RefCell::new(pyro_dom::Dom::new()));
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
        width: 800,
        height: 600,
        device_scale_factor: 1.0,
        live: Some(Live { page: 1, tree: Arc::new(tree), clip: None, interaction: None }),
        ..Default::default()
    };
    let mut renderer = Renderer::new();
    renderer.sync_layout(&snap, &fetcher()).unwrap();
    keys.into_iter()
        .map(|key| {
            let b = renderer.element_box(1, key).unwrap();
            [b.x, b.y, b.x + b.width, b.y + b.height]
        })
        .collect()
}


fn height(container: &str) -> f64 {
    let b = boxes(
        &format!("<!doctype html><style>body{{margin:0}}</style><div id=c style='width:100px;{container}'><div style='height:100px'></div></div>"),
        &["c"],
    );
    b[0][3] - b[0][1]
}

#[test]
fn columns_of_a_fixed_height_wrap_into_rows() {
    // 100px of content in 2 columns of 25px: 2 rows, 10px apart.
    assert_eq!(height("columns:2;gap:10px 0;column-height:25px;column-wrap:wrap"), 60.0);
    // `auto` wraps when the height is given; `nowrap` keeps one row (the container's own height
    // is then its content's, as without a column height: 0 here, since columns are not stacked).
    assert_eq!(height("columns:2;gap:10px 0;column-height:25px"), 60.0);
    assert_eq!(height("columns:2;column-height:25px;column-wrap:nowrap"), 25.0);
    // The shorthand, and `column-height` alone (one column per row).
    assert_eq!(height("columns:2 / 25px;row-gap:0"), 50.0);
    assert_eq!(height("column-height:40px;row-gap:20px"), 160.0); // 3 rows of 40px, 2 gaps of 20px
}

#[test]
fn without_a_column_height_nothing_changes() {
    assert_eq!(height("columns:2;gap:0"), 50.0);
    assert_eq!(height("column-wrap:wrap"), 100.0);
}
