//! Matched Chrome table-cell heights: /tmp/table-height-chrome-matrix.log.
//! A cell's author height is a minimum, including with overflow:hidden.
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
        live: Some(Live { page: 1, tree: Arc::new(tree), clip: None }),
        ..Default::default()
    };
    let mut renderer = Renderer::new();
    renderer.sync_layout(&snap, &(Arc::new(NoFetch) as Arc<dyn Fetcher>)).unwrap();
    keys.into_iter().map(|key| renderer.element_box(1, key).unwrap()).collect()
}

fn cell_html(collapse: &str, layout: &str, sizing: &str, height: &str) -> String {
    format!(
        r#"<!doctype html><style>body{{margin:0}}table{{table-layout:{layout};border-collapse:{collapse};border-spacing:0;width:400px}}td{{box-sizing:{sizing};height:{height};padding:2px 4px;border-top:1px solid transparent;border-bottom:1px solid gray;overflow:hidden;white-space:nowrap;font:13px/16.25px Arial}}.amount{{display:inline-flex;gap:4px;white-space:nowrap}}.amount span{{line-height:24px}}</style><table><tbody><tr id=row><td id=cell><div id=child class=amount><span>120.00</span><span>EUR</span></div></td></tr></tbody></table>"#
    )
}

#[test]
fn table_cell_height_is_a_content_minimum_in_fixed_and_auto_tables() {
    let mut failures = Vec::new();
    for collapse in ["separate", "collapse"] {
        for layout in ["fixed", "auto"] {
            for sizing in ["border-box", "content-box"] {
                for height in ["26px", "auto", "40px"] {
                    let b = boxes(&cell_html(collapse, layout, sizing, height), &["cell", "row", "child"]);
                    // Chrome treats table-cell height as a border-box minimum even
                    // for content-box. Collapsed 1px borders contribute half each.
                    let expected = if height == "40px" {
                        40.0
                    } else if collapse == "collapse" {
                        29.0
                    } else {
                        30.0
                    };
                    if b[0].height != expected
                        || b[1].height != expected
                        || b[2].height != 24.0
                        || b[2].y + b[2].height > b[0].y + b[0].height
                    {
                        failures.push(format!("{collapse}/{layout}/{sizing}/{height}: cell={:?}, row={:?}, child={:?}, expected={expected}", b[0], b[1], b[2]));
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn table_rowspan_content_minimum_fits_the_combined_rows() {
    for layout in ["fixed", "auto"] {
        let html = format!(
            r#"<!doctype html><style>body{{margin:0}}table{{table-layout:{layout};border-spacing:0;width:400px}}td{{height:26px;box-sizing:border-box;padding:2px 4px;border:1px solid gray;overflow:hidden}}#child{{display:flex;height:60px}}.small{{height:20px}}</style><table><tbody><tr id=first><td id=cell rowspan=2><div id=child></div></td><td><div class=small></div></td></tr><tr id=second><td><div class=small></div></td></tr></tbody></table>"#
        );
        let b = boxes(&html, &["cell", "child", "first", "second"]);
        assert_eq!(b[0].height, 66.0, "{layout}: spanning content plus padding/border");
        assert!(b[1].y >= b[0].y && b[1].y + b[1].height <= b[0].y + b[0].height, "{layout}: {b:?}");
        assert_eq!(b[3].y + b[3].height - b[2].y, 66.0, "{layout}: row allocation");
    }
}

#[test]
fn table_cell_overflow_clip_retains_the_complete_intrinsic_child() {
    let snap = Snapshot { html: "<!doctype html><style>body{margin:0;background:white}table{border-spacing:0;table-layout:fixed;width:100px}td{height:26px;box-sizing:border-box;padding:2px 4px;border-top:1px solid black;border-bottom:1px solid black;overflow:hidden}div{height:24px;background:rgb(255,0,0)}</style><table><tr><td><div></div></td></tr></table>".into(), base_url:"http://test/".into(), width:120, height:60, device_scale_factor:1.0, ..Default::default() };
    let (image, _, _) =
        Renderer::new().render(&snap, Area::Viewport, Backend::Cpu, &(Arc::new(NoFetch) as Arc<dyn Fetcher>)).unwrap();
    let pixel =
        |x: usize, y: usize| &image.data[(y * image.width as usize + x) * 4..(y * image.width as usize + x) * 4 + 4];
    assert_eq!(pixel(10, 26), &[255, 0, 0, 255], "last child row must not be clipped by the old26px cell");
    assert_eq!(pixel(10, 29), &[0, 0, 0, 255], "cell bottom border follows the complete24px child");
    assert_eq!(pixel(10, 30), &[255, 255, 255, 255], "overflow still clips at the expanded cell boundary");
}

#[test]
fn inline_flex_content_baseline_sizes_a_line_outside_tables() {
    let b = boxes(
        "<!doctype html><style>body{margin:0;font:13px/16.25px Arial}.amount{display:inline-flex;gap:4px}.amount span{line-height:24px}</style><div id=line>adjacent <div class=amount id=child><span>120.00</span><span>EUR</span></div> text</div>",
        &["line", "child"],
    );
    assert_eq!(
        (b[0].y, b[0].height, b[1].y, b[1].height),
        (0.0, 24.0, 0.0, 24.0),
        "Chrome aligns the text baseline, without adding bottom-edge descent: {b:?}"
    );
}

#[test]
fn nested_tables_keep_each_tables_border_model_and_intrinsic_height() {
    for outer in ["separate", "collapse"] {
        for inner in ["separate", "collapse"] {
            let html = format!(
                r#"<!doctype html><style>body{{margin:0;font:13px/16.25px Arial}}table{{table-layout:fixed;border-spacing:0;width:400px}}td{{height:26px;box-sizing:border-box;padding:2px 4px;border-top:1px solid transparent;border-bottom:1px solid gray;overflow:hidden;white-space:nowrap}}.amount{{display:inline-flex;gap:4px}}.amount span{{line-height:24px}}</style><table style="border-collapse:{outer}"><tbody><tr><td id=outer><table style="border-collapse:{inner};width:100%"><tbody><tr><td id=inner><div class=amount id=child><span>120.00</span><span>EUR</span></div></td></tr></tbody></table></td></tr></tbody></table>"#
            );
            let b = boxes(&html, &["outer", "inner", "child"]);
            let expected_outer = if outer == "collapse" { (0.5, 35.0) } else { (0.0, 36.0) };
            let expected_inner = if inner == "collapse" { (3.5, 29.0) } else { (3.0, 30.0) };
            assert_eq!((b[0].y, b[0].height), expected_outer, "outer={outer}, inner={inner}: {b:?}");
            assert_eq!((b[1].y, b[1].height), expected_inner, "outer={outer}, inner={inner}: {b:?}");
            assert_eq!((b[2].y, b[2].height), (6.0, 24.0), "outer={outer}, inner={inner}: {b:?}");
        }
    }
}
