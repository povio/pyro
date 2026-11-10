//! Chrome-backed18-row table minimums under a constrained column flexbox.
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

fn fixture(layout: &str, overflow: &str, row_style: &str, cell: &str) -> String {
    let rows = (0..18).map(|_| format!("<tr style='{row_style}'><td>{cell}</td></tr>")).collect::<String>();
    format!(
        r#"<!doctype html><style>body{{margin:0;font:14px/20px sans-serif}}#viewport{{display:flex;flex-direction:column;width:400px;height:100px;overflow:auto}}#inner{{display:flex;flex-direction:column;width:100%}}table{{table-layout:{layout};border-spacing:0;width:100%}}td{{box-sizing:border-box;height:26px;padding:2px 4px;border-bottom:1px solid black;overflow:{overflow}}}</style><div id=viewport><div id=inner><table id=table>{rows}</table></div></div>"#
    )
}

#[test]
fn eighteen_clipped_table_rows_keep_intrinsic_height_inside_column_flex() {
    for layout in ["auto", "fixed"] {
        for overflow in ["hidden", "visible"] {
            let b = boxes(&fixture(layout, overflow, "", "Invoice"), &["viewport", "inner", "table"]);
            assert_eq!(b[0].client_height, 100.0);
            assert_eq!(b[0].scroll_height, 468.0, "{layout}/{overflow}: {b:?}");
            assert_eq!(b[1].height, 468.0);
            assert_eq!(b[2].height, 468.0);
        }
    }
}

#[test]
fn explicit_row_height_and_taller_text_remain_minimums() {
    for layout in ["auto", "fixed"] {
        for (row_style, cell, expected) in [("height:40px", "Invoice", 720.0), ("", "First<br>Second", 810.0)] {
            let b = boxes(&fixture(layout, "hidden", row_style, cell), &["viewport", "table"]);
            assert_eq!(b[1].height, expected, "{layout}/{row_style}: {b:?}");
            assert_eq!(b[0].scroll_height, expected);
        }
    }
}

#[test]
fn rowspan_contributes_content_height_without_unclipping_cells() {
    for layout in ["auto", "fixed"] {
        let rows = (0..16).map(|_| "<tr><td>Invoice</td><td>EUR</td></tr>").collect::<String>();
        let html = format!(
            r#"<!doctype html><style>body{{margin:0;font:14px/20px sans-serif}}#viewport{{display:flex;flex-direction:column;width:400px;height:100px;overflow:auto}}#inner{{display:flex;flex-direction:column;width:100%}}table{{table-layout:{layout};border-spacing:0;width:400px}}td{{box-sizing:border-box;height:26px;padding:2px 4px;border-bottom:1px solid black;overflow:hidden}}#tall{{height:80px}}</style><div id=viewport><div id=inner><table id=table><tr><td id=span rowspan=2><div id=tall></div></td><td>EUR</td></tr><tr><td>EUR</td></tr>{rows}</table></div></div>"#
        );
        let b = boxes(&html, &["viewport", "table", "span", "tall"]);
        assert_eq!(b[1].height, 501.0, "{layout}: {b:?}");
        assert_eq!(b[0].scroll_height, 501.0);
        assert_eq!(b[2].height, 85.0);
        assert_eq!(b[3].height, 80.0);
    }
    let html = fixture("fixed", "hidden", "", "<div style='width:800px'>Real overflow</div>").replacen(
        "<td>",
        "<td id=clipped>",
        1,
    );
    let b = boxes(&html, &["viewport", "table", "clipped"]);
    assert_eq!(b[0].scroll_width, b[0].client_width, "clipped cell content leaked horizontally");
    assert_eq!(b[0].scroll_height, 468.0);
    assert!(b[2].scroll_width >= 800.0, "real overflow inside clipped cell was lost");
    assert!(b[2].scroll_width > b[2].client_width);
}

#[test]
fn formatting_root_alignment_does_not_shift_its_own_text_lines() {
    for alignment in ["baseline", "middle"] {
        let html = format!(
            r#"<!doctype html><style>body{{margin:0;font:14px/20px sans-serif}}#plain{{vertical-align:{alignment}}}table{{width:400px;border-spacing:0}}td{{box-sizing:border-box;height:26px;padding:2px 4px;border-bottom:1px solid black;vertical-align:{alignment}}}</style><div id=plain>First<br>Second</div><table><tr><td id=cell>First<br>Second</td></tr></table>"#
        );
        let b = boxes(&html, &["plain", "cell"]);
        assert_eq!(b[0].height, 40.0, "{alignment}: root strut shifted");
        assert_eq!(b[1].height, 45.0, "{alignment}: cell alignment changed line heights");
    }
}

#[test]
fn descendant_span_and_atomic_alignment_remain_independent_of_root_alignment() {
    let mut root_results = Vec::new();
    for root in ["baseline", "middle"] {
        let html = format!(
            r#"<!doctype html><style>body{{margin:0;font:14px/20px sans-serif}}.case{{width:400px;vertical-align:{root}}}.atomic{{display:inline-block;width:20px;height:30px}}.large{{font-size:30px;line-height:30px}}</style><div id=super class=case>A<span class=large style=vertical-align:super>X</span>B</div><div id=baseline class=case>A<span class=large style=vertical-align:baseline>X</span>B</div><div id=atomicMiddle class=case>A<i class=atomic style=vertical-align:middle></i>B</div><div id=atomicBaseline class=case>A<i class=atomic style=vertical-align:baseline></i>B</div>"#
        );
        let b = boxes(&html, &["super", "baseline", "atomicMiddle", "atomicBaseline"]);
        // Chrome: the descendant superscript expands the line, while an atomic
        // middle-aligned30px box is30px and baseline alignment adds5px descent.
        assert!(b[0].height > b[1].height, "descendant super alignment was lost");
        assert_eq!(b[1].height, 30.0);
        assert_eq!(b[2].height, 30.0);
        assert_eq!(b[3].height, 35.0);
        root_results.push(b.into_iter().map(|b| b.height).collect::<Vec<_>>());
    }
    assert_eq!(root_results[0], root_results[1], "root alignment leaked into descendant inline styles");
}
