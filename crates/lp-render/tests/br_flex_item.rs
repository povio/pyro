//! `<br>` in flex and grid containers, against Chrome 156 (see bench/pdf/fixtures/generators/fx-br.mjs for more).
//!
//! A `<br>` is a piece of text there: it joins neighbouring text and `<br>`s in one anonymous item, one line each, and
//! all of its CSS is ignored except `display: none` and `display: contents`, which remove it. White space after a
//! `<br>` or text stays when it's preserved, and makes a line of its own.
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
        live: Some(Live { page: 1, tree: Arc::new(tree), clip: None, interaction: None }),
        ..Default::default()
    };
    let mut renderer = Renderer::new();
    renderer.sync_layout(&snap, &(Arc::new(NoFetch) as Arc<dyn Fetcher>)).unwrap();
    keys.into_iter().map(|key| renderer.element_box(1, key).unwrap()).collect()
}

const COL: &str = "display:flex;flex-direction:column;align-items:flex-start";
const ROW: &str = "display:flex";
const GRID: &str = "display:grid";

/// Checks each case: container, extra style, content (`A` and `B` are 10x5 boxes), and in Chrome: x and y of B, y of
/// what follows the container. x is None where it depends on the width of a font's text.
fn check(container: &str, cases: &[(&str, &str, Option<f64>, f64, f64)]) {
    let mut failures = Vec::new();
    for (style, inner, x, y, after) in cases {
        let inner = inner
            .replace('A', "<div id=a style='width:10px;height:5px'></div>")
            .replace('B', "<div id=b style='width:10px;height:5px'></div>");
        let html = format!(
            "<!doctype html><body style='margin:0;font:16px/20px Arial'><div style='width:300px;{container};{style}'>{inner}</div><div id=z style='height:1px'></div>"
        );
        let b = boxes(&html, &["b", "z"]);
        let close = |p: f64, q: f64| (p - q).abs() < 0.05;
        if !(x.is_none_or(|x| close(x, b[0].x)) && close(*y, b[0].y) && close(*after, b[1].y)) {
            failures.push(format!("{style} {inner}: {x:?},{y} {after}, got {},{} {}", b[0].x, b[0].y, b[1].y));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn br_in_a_column_flex_container_matches_chrome() {
    check(
        COL,
        &[
            ("", "A<br>B", Some(0.0), 25.0, 30.0),
            ("", "A<br><br>B", Some(0.0), 45.0, 50.0),
            ("", "A<br><br><br>B", Some(0.0), 65.0, 70.0),
            ("", "A<br><!--c--><br>B", Some(0.0), 45.0, 50.0),
            ("", "A<br style=\"line-height:40px\">B", Some(0.0), 25.0, 30.0),
            ("", "A<br style=\"font-size:40px\">B", Some(0.0), 25.0, 30.0),
            ("", "A<br style=\"display:block;height:50px;width:30px\">B", Some(0.0), 25.0, 30.0),
            ("", "A<br style=\"display:flex\">B", Some(0.0), 25.0, 30.0),
            ("", "A<br style=\"display:inline-block;width:30px\">B", Some(0.0), 25.0, 30.0),
            ("", "A<br style=\"display:none\">B", Some(0.0), 5.0, 10.0),
            ("", "A<br style=\"display:contents\">B", Some(0.0), 5.0, 10.0),
            ("", "A<br style=\"margin:10px;padding:7px;border:3px solid #000\">B", Some(0.0), 25.0, 30.0),
            ("", "A<br style=\"position:absolute\">B", Some(0.0), 25.0, 30.0),
            ("", "A<br style=\"float:left\">B", Some(0.0), 25.0, 30.0),
            ("", "A<br style=\"order:5\">B", Some(0.0), 25.0, 30.0),
            ("", "A<br style=\"flex:1 0 100px\">B", Some(0.0), 25.0, 30.0),
            ("", "A<br><span style=\"display:none\"></span><br>B", Some(0.0), 45.0, 50.0),
            ("", "A<br><span style=\"position:absolute\"></span><br>B", Some(0.0), 45.0, 50.0),
            ("", "A<div style=\"display:contents\"><br></div><br>B", Some(0.0), 45.0, 50.0),
            ("", "A<span><br></span>B", Some(0.0), 25.0, 30.0),
            ("", "Atx<br>tyB", Some(0.0), 45.0, 50.0),
            ("", "Atx<br>B", Some(0.0), 25.0, 30.0),
            ("", "Atx<br><br>tyB", Some(0.0), 65.0, 70.0),
            ("", "Atx<br style=\"display:none\">tyB", Some(0.0), 25.0, 30.0),
            ("", "A<br> B", Some(0.0), 25.0, 30.0),
            ("", "A<br>\nB", Some(0.0), 25.0, 30.0),
            ("", "A <br>B", Some(0.0), 25.0, 30.0),
            ("", "A<br> <br>B", Some(0.0), 45.0, 50.0),
            ("", "A<br>\n<br>\nB", Some(0.0), 45.0, 50.0),
            ("white-space:pre-wrap", "A<br> B", Some(0.0), 45.0, 50.0),
            ("white-space:pre-wrap", "A<br>\nB", Some(0.0), 45.0, 50.0),
            ("white-space:pre-wrap", "A <br>B", Some(0.0), 25.0, 30.0),
            ("white-space:pre-wrap", "A<br> <br>B", Some(0.0), 45.0, 50.0),
            ("white-space:pre-wrap", "A<br>\n<br>\nB", Some(0.0), 85.0, 90.0),
            ("white-space:pre-line", "A<br> B", Some(0.0), 25.0, 30.0),
            ("white-space:pre-line", "A<br>\nB", Some(0.0), 45.0, 50.0),
            ("white-space:pre-line", "A <br>B", Some(0.0), 25.0, 30.0),
            ("white-space:pre-line", "A<br> <br>B", Some(0.0), 45.0, 50.0),
            ("white-space:pre-line", "A<br>\n<br>\nB", Some(0.0), 85.0, 90.0),
            ("font:15px/15px Arial", "A<br>B", Some(0.0), 20.0, 25.0),
            ("font:11.2px/1.4 Arial", "A<br><br>B", Some(0.0), 36.34375, 41.34375),
        ],
    );
}

#[test]
fn br_in_a_row_flex_container_matches_chrome() {
    check(
        ROW,
        &[
            ("", "A<br>B", Some(10.0), 0.0, 20.0),
            ("", "A<br><br>B", Some(10.0), 0.0, 40.0),
            ("", "A<br><br><br>B", Some(10.0), 0.0, 60.0),
            ("", "A<br><!--c--><br>B", Some(10.0), 0.0, 40.0),
            ("", "A<br style=\"line-height:40px\">B", None, 0.0, 20.0),
            ("", "A<br style=\"font-size:40px\">B", None, 0.0, 20.0),
            ("", "A<br style=\"display:block;height:50px;width:30px\">B", None, 0.0, 20.0),
            ("", "A<br style=\"display:flex\">B", None, 0.0, 20.0),
            ("", "A<br style=\"display:inline-block;width:30px\">B", None, 0.0, 20.0),
            ("", "A<br style=\"display:none\">B", None, 0.0, 5.0),
            ("", "A<br style=\"display:contents\">B", None, 0.0, 5.0),
            ("", "A<br style=\"margin:10px;padding:7px;border:3px solid #000\">B", None, 0.0, 20.0),
            ("", "A<br style=\"position:absolute\">B", None, 0.0, 20.0),
            ("", "A<br style=\"float:left\">B", None, 0.0, 20.0),
            ("", "A<br style=\"order:5\">B", None, 0.0, 20.0),
            ("", "A<br style=\"flex:1 0 100px\">B", None, 0.0, 20.0),
            ("", "A<br><span style=\"display:none\"></span><br>B", None, 0.0, 40.0),
            ("", "A<br><span style=\"position:absolute\"></span><br>B", None, 0.0, 20.0),
            ("", "A<div style=\"display:contents\"><br></div><br>B", None, 0.0, 40.0),
            ("", "A<span><br></span>B", Some(10.0), 0.0, 20.0),
            ("", "Atx<br>tyB", None, 0.0, 40.0),
            ("", "Atx<br>B", None, 0.0, 20.0),
            ("", "Atx<br><br>tyB", None, 0.0, 60.0),
            ("", "Atx<br style=\"display:none\">tyB", None, 0.0, 20.0),
            ("", "A<br> B", Some(10.0), 0.0, 20.0),
            ("", "A<br>\nB", Some(10.0), 0.0, 20.0),
            ("", "A <br>B", Some(10.0), 0.0, 20.0),
            ("", "A<br> <br>B", Some(10.0), 0.0, 40.0),
            ("", "A<br>\n<br>\nB", Some(10.0), 0.0, 40.0),
            ("white-space:pre-wrap", "A<br> B", Some(14.453125), 0.0, 40.0),
            ("white-space:pre-wrap", "A<br>\nB", Some(10.0), 0.0, 40.0),
            ("white-space:pre-wrap", "A <br>B", Some(10.0), 0.0, 20.0),
            ("white-space:pre-wrap", "A<br> <br>B", Some(14.453125), 0.0, 40.0),
            ("white-space:pre-wrap", "A<br>\n<br>\nB", Some(10.0), 0.0, 80.0),
            ("white-space:pre-line", "A<br> B", Some(10.0), 0.0, 20.0),
            ("white-space:pre-line", "A<br>\nB", Some(10.0), 0.0, 40.0),
            ("white-space:pre-line", "A <br>B", Some(10.0), 0.0, 20.0),
            ("white-space:pre-line", "A<br> <br>B", Some(10.0), 0.0, 40.0),
            ("white-space:pre-line", "A<br>\n<br>\nB", Some(10.0), 0.0, 80.0),
            ("font:15px/15px Arial", "A<br>B", Some(10.0), 0.0, 15.0),
            ("font:11.2px/1.4 Arial", "A<br><br>B", Some(10.0), 0.0, 31.34375),
        ],
    );
}

#[test]
fn br_in_a_grid_container_matches_chrome() {
    check(
        GRID,
        &[
            ("", "A<br>B", Some(0.0), 25.0, 30.0),
            ("", "A<br><br>B", Some(0.0), 45.0, 50.0),
            ("", "A<br><br><br>B", Some(0.0), 65.0, 70.0),
            ("", "A<br><!--c--><br>B", Some(0.0), 45.0, 50.0),
            ("", "A<br style=\"line-height:40px\">B", Some(0.0), 25.0, 30.0),
            ("", "A<br style=\"font-size:40px\">B", Some(0.0), 25.0, 30.0),
            ("", "A<br style=\"display:block;height:50px;width:30px\">B", Some(0.0), 25.0, 30.0),
            ("", "A<br style=\"display:flex\">B", Some(0.0), 25.0, 30.0),
            ("", "A<br style=\"display:inline-block;width:30px\">B", Some(0.0), 25.0, 30.0),
            ("", "A<br style=\"display:none\">B", Some(0.0), 5.0, 10.0),
            ("", "A<br style=\"display:contents\">B", Some(0.0), 5.0, 10.0),
            ("", "A<br style=\"margin:10px;padding:7px;border:3px solid #000\">B", Some(0.0), 25.0, 30.0),
            ("", "A<br style=\"position:absolute\">B", Some(0.0), 25.0, 30.0),
            ("", "A<br style=\"float:left\">B", Some(0.0), 25.0, 30.0),
            ("", "A<br style=\"order:5\">B", Some(0.0), 25.0, 30.0),
            ("", "A<br style=\"flex:1 0 100px\">B", Some(0.0), 25.0, 30.0),
            ("", "A<br><span style=\"display:none\"></span><br>B", Some(0.0), 45.0, 50.0),
            ("", "A<br><span style=\"position:absolute\"></span><br>B", Some(0.0), 45.0, 50.0),
            ("", "A<div style=\"display:contents\"><br></div><br>B", Some(0.0), 45.0, 50.0),
            ("", "A<span><br></span>B", Some(0.0), 25.0, 30.0),
            ("", "Atx<br>tyB", Some(0.0), 45.0, 50.0),
            ("", "Atx<br>B", Some(0.0), 25.0, 30.0),
            ("", "Atx<br><br>tyB", Some(0.0), 65.0, 70.0),
            ("", "Atx<br style=\"display:none\">tyB", Some(0.0), 25.0, 30.0),
            ("", "A<br> B", Some(0.0), 25.0, 30.0),
            ("", "A<br>\nB", Some(0.0), 25.0, 30.0),
            ("", "A <br>B", Some(0.0), 25.0, 30.0),
            ("", "A<br> <br>B", Some(0.0), 45.0, 50.0),
            ("", "A<br>\n<br>\nB", Some(0.0), 45.0, 50.0),
            ("white-space:pre-wrap", "A<br> B", Some(0.0), 45.0, 50.0),
            ("white-space:pre-wrap", "A<br>\nB", Some(0.0), 45.0, 50.0),
            ("white-space:pre-wrap", "A <br>B", Some(0.0), 25.0, 30.0),
            ("white-space:pre-wrap", "A<br> <br>B", Some(0.0), 45.0, 50.0),
            ("white-space:pre-wrap", "A<br>\n<br>\nB", Some(0.0), 85.0, 90.0),
            ("white-space:pre-line", "A<br> B", Some(0.0), 25.0, 30.0),
            ("white-space:pre-line", "A<br>\nB", Some(0.0), 45.0, 50.0),
            ("white-space:pre-line", "A <br>B", Some(0.0), 25.0, 30.0),
            ("white-space:pre-line", "A<br> <br>B", Some(0.0), 45.0, 50.0),
            ("white-space:pre-line", "A<br>\n<br>\nB", Some(0.0), 85.0, 90.0),
            ("font:15px/15px Arial", "A<br>B", Some(0.0), 20.0, 25.0),
            ("font:11.2px/1.4 Arial", "A<br><br>B", Some(0.0), 36.34375, 41.34375),
        ],
    );
}
