//! Boxes of tables without uniform collapsed borders. Expected values measured on Chrome 156
//! (`bench/pdf/fixtures/table-collapse.mjs`, `chrome-156/table-collapse.json`).
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

fn fetcher() -> Arc<dyn Fetcher> {
    Arc::new(NoFetch)
}

fn boxes(html: &str, ids: &[&str]) -> Vec<[f64; 4]> {
    use_fork_safe_fonts();
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
        width: 800,
        height: 500,
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

const HEAD: &str = "<!doctype html><style>html,body{margin:0;font:13px/18px Arial}table{border-spacing:0;border-collapse:collapse}td{padding:0}div{height:18px}</style>";

fn assert_close(got: [f64; 4], want: [f64; 4]) {
    assert!(got.iter().zip(want).all(|(g, w)| (g - w).abs() < 0.05), "{got:?}, Chrome {want:?}");
}

/// The ToS CMR goods table: a spanning cell without borders, a left border on the cells after it. Half of the
/// border is inside each cell (0.5 px less content on the left of it, 0.5 px of inset after it).
#[test]
fn a_left_border_is_shared_with_the_cell_before_it() {
    let html = format!(
        "{HEAD}<table style='width:600px;table-layout:fixed'><colgroup><col style='width:100px'><col style='width:80px'><col style='width:60px'><col style='width:120px'></colgroup>\
         <tr><td><div></div></td><td><div></div></td><td><div></div></td><td><div></div></td></tr>\
         <tr><td id=a colspan=2><div id=b></div></td><td id=c style='border-left:1px solid #000;padding:0 3px'><div id=d></div></td><td style='border-left:1px solid #000;padding:0 3px'><div></div></td></tr></table>"
    );
    let got = boxes(&html, &["a", "b", "c", "d"]);
    assert_close(got[0], [0.0, 18.0, 299.984, 36.0]);
    assert_close(got[1], [0.0, 18.0, 299.484, 36.0]);
    assert_close(got[2], [299.984, 18.0, 399.969, 36.0]);
    assert_close(got[3], [303.484, 18.0, 396.484, 36.0]);
}

/// A row's top border adds half of itself to the rows on both sides of it.
#[test]
fn a_row_border_takes_height_from_both_rows() {
    let html = format!(
        "{HEAD}<table id=t style='width:400px'><tr><td><div id=a></div></td></tr><tr><td style='border-top:2px solid #000'><div id=b></div></td></tr></table>"
    );
    let got = boxes(&html, &["t", "a", "b"]);
    assert_close(got[0], [0.0, 0.0, 400.0, 38.0]);
    assert_close(got[1], [0.0, 0.0, 400.0, 18.0]);
    assert_close(got[2], [0.0, 20.0, 400.0, 38.0]);
}

/// A table's own border: half of it is the table's, half its cells'.
#[test]
fn a_table_border_is_shared_with_its_cells() {
    let html = format!(
        "{HEAD}<table id=t style='width:400px;border:3px solid #000'><tr><td id=c><div id=d></div></td><td></td></tr></table>"
    );
    let got = boxes(&html, &["t", "c", "d"]);
    assert_close(got[0], [0.0, 0.0, 400.0, 24.0]);
    assert_close(got[1], [1.5, 1.5, 200.0, 22.5]);
    assert_close(got[2], [3.0, 3.0, 200.0, 21.0]);
}
