//! Matched Chrome table-cell heights: /tmp/table-height-chrome-matrix.log.
//! A cell's author height is a minimum, including with overflow:hidden.
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

fn boxes(html: &str, ids: &[&str]) -> Vec<LayoutBox> {
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
                    // Measured in Chrome 156: a cell's height is a minimum of
                    // its border box per `box-sizing`, so a content-box cell
                    // adds its padding (4px) and borders (2px; half of each
                    // collapsed 1px border, 1px) to it. Collapsed 1px borders
                    // contribute half each to the content minimum too.
                    let expected = match (collapse, sizing, height) {
                        (_, "border-box", "40px") => 40.0,
                        ("separate", "content-box", "40px") => 46.0,
                        ("collapse", "content-box", "40px") => 45.0,
                        ("separate", "content-box", "26px") => 32.0,
                        ("collapse", "content-box", "26px") => 31.0,
                        ("collapse", _, _) => 29.0,
                        _ => 30.0,
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

#[test]
fn table_row_rectangles_use_tracks_instead_of_spanning_edge_cell_union() {
    let b = boxes(
        r#"<!doctype html><style>
        *{box-sizing:border-box}body{margin:0}table{table-layout:fixed;width:280px;border-spacing:0}
        td{height:26px;padding:2px;border:1px solid;overflow:hidden}
        input,button{display:block;height:20px;padding:0;border:0;width:100%}
        .drag{height:24px;width:16px}.left{position:sticky;left:0}.right{position:sticky;right:0}
        </style><table><colgroup><col style='width:40px'><col style='width:200px'><col style='width:40px'></colgroup>
        <tbody id=body><tr id=first><td id=left class=left rowspan=2><div class=drag></div></td><td id=ordinary1><input value=one></td><td id=right class=right rowspan=2><button>Act</button></td></tr>
        <tr id=second><td id=ordinary2><input value=two></td></tr></tbody></table>"#,
        &["first", "second", "ordinary1", "ordinary2", "left", "right", "body"],
    );
    assert_eq!((b[2].height, b[3].height), (26.0, 26.0), "ordinary cells establish true26px tracks");
    assert_eq!((b[4].height, b[5].height), (52.0, 52.0), "edge cells retain their real rowspan height");
    eprintln!(
        "rowspan projection: first={:?}, second={:?}, ordinary={:?}/{:?}, spans={:?}/{:?}",
        b[0], b[1], b[2], b[3], b[4], b[5]
    );
    assert_eq!((b[0].height, b[1].height), (26.0, 26.0), "TR rectangles must describe individual tracks");
    assert_eq!(b[1].y, b[0].y + 26.0, "the second row starts directly after the first track");
    assert_eq!(b[6].height, 52.0, "row group still covers both tracks");
}

fn row_projection_controls(collapse: &str, spacing: u32, second_height: u32) -> String {
    format!(
        r#"<!doctype html><style>
    *{{box-sizing:border-box}}body{{margin:0}}table{{table-layout:fixed;width:400px;border-collapse:{collapse};border-spacing:{spacing}px}}
    td{{height:26px;padding:2px;border:1px solid;overflow:hidden}}#ordinary2{{height:{second_height}px}}
    input,button{{display:block;height:20px;padding:0;border:0;width:100%}}.drag{{height:24px;width:16px}}
    .left{{position:sticky;left:0}}.right{{position:sticky;right:0}}
    </style><table><colgroup><col style='width:40px'><col style='width:200px'><col style='width:40px'></colgroup>
    <tbody id=body><tr id=first><td id=left class=left rowspan=2><div class=drag></div></td><td id=ordinary1><input></td><td id=right class=right rowspan=2><button></button></td></tr>
    <tr id=second><td id=ordinary2><input></td></tr></tbody></table>"#
    )
}

#[test]
fn row_track_projection_preserves_unequal_heights_and_border_spacing() {
    for (collapse, spacing) in [("separate", 0), ("separate", 4), ("collapse", 4)] {
        let b = boxes(
            &row_projection_controls(collapse, spacing, 42),
            &["first", "second", "ordinary1", "ordinary2", "left", "right", "body"],
        );
        let gap = if collapse == "separate" { spacing as f64 } else { 0.0 };
        assert_eq!((b[2].height, b[3].height), (26.0, 42.0), "true tracks: {collapse}/{spacing}");
        assert_eq!((b[0].height, b[1].height), (26.0, 42.0), "row projection: {collapse}/{spacing}");
        assert_eq!(b[1].y - b[0].y, 26.0 + gap, "row spacing: {collapse}/{spacing}");
        assert_eq!((b[4].height, b[5].height), (68.0 + gap, 68.0 + gap), "spanning cells stay unchanged");
        for row in &b[..2] {
            assert_eq!(row.x, b[4].x, "full grid row starts at the first column");
            // CSSOM snaps x and width independently to 1/64px. Their sum
            // can differ from the projected outer edge by one layout unit.
            assert!(
                ((row.x + row.width) - (b[5].x + b[5].width)).abs() <= 1.0 / 32.0,
                "full grid row reaches the last column within snapped layout units"
            );
        }
        assert_eq!(b[6].height, 68.0 + gap, "row group covers the actual tracks");
    }
}

#[test]
fn row_track_projection_handles_rows_without_nonspanning_cells() {
    for normal_sibling in [false, true] {
        let ordinary = if normal_sibling { "<td><input style='display:block;height:20px'></td>" } else { "" };
        let html=format!("<!doctype html><style>*{{box-sizing:border-box}}body{{margin:0}}table{{table-layout:fixed;width:400px;border-spacing:0}}td{{height:26px;padding:0;border:0}}</style><table><colgroup><col style='width:200px'><col style='width:200px'></colgroup><tbody id=body><tr id=first style='height:26px'><td id=span rowspan=2 style='position:sticky;left:0'><div style='height:24px'></div></td></tr><tr id=second style='height:42px'>{ordinary}</tr></tbody></table>");
        let b = boxes(&html, &["first", "second", "span", "body"]);
        assert_eq!(
            (b[0].height, b[1].height),
            (26.0, 42.0),
            "tracks without ordinary originating cells, sibling={normal_sibling}"
        );
        assert_eq!(b[1].y - b[0].y, 26.0);
        assert_eq!((b[0].width, b[1].width), (400.0, 400.0));
        assert_eq!((b[2].height, b[3].height), (68.0, 68.0));
    }
}

#[test]
fn row_track_projection_preserves_oversized_rowspan_cell_extent() {
    for rowspan in [2, 3] {
        let html = format!(
            "<!doctype html><style>body{{margin:0}}table{{table-layout:fixed;width:200px;border-spacing:0}}td{{padding:0;border:0}}</style><table><tbody id=body><tr id=row><td id=cell rowspan={rowspan}><div style='height:60px'></div></td></tr></tbody></table>"
        );
        let b = boxes(&html, &["row", "cell", "body"]);
        assert_eq!((b[1].width, b[1].height), (200.0, 60.0), "rowspan={rowspan}: {b:?}");
        assert_eq!(
            (b[0].x, b[0].y, b[0].width, b[0].height),
            (b[1].x, b[1].y, b[1].width, b[1].height),
            "implicit grid rows must not truncate the only DOM row, rowspan={rowspan}: {b:?}"
        );
        assert_eq!((b[2].y, b[2].height), (b[1].y, b[1].height));
    }
}

#[test]
fn row_track_projection_preserves_table_ancestor_translation_and_scale() {
    let html = row_projection_controls("separate", 0, 26)
        .replace("<table>", "<div style='transform:translate(10px,20px) scale(2);transform-origin:0 0'><table>")
        .replace("</table>", "</table></div>");
    let b = boxes(&html, &["first", "second", "left", "body"]);
    assert_eq!((b[0].x, b[0].y), (10.0, 20.0));
    assert_eq!((b[0].width, b[1].width), (800.0, 800.0));
    assert_eq!((b[0].height, b[1].height), (52.0, 52.0));
    assert_eq!(b[1].y - b[0].y, 52.0);
    assert_eq!((b[2].height, b[3].height), (104.0, 104.0));
}
