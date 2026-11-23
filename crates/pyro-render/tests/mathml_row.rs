//! MathML rows (MathML Core § 3.3.1, Chromium's `MathMLRowLayoutAlgorithm`): a row lays its
//! in-flow element children out side by side on its baseline, never wrapped; white space between
//! them, out-of-flow and `display: none` children take no place. An invalid script or fraction is
//! laid out as a row, as are table cells, `<semantics>` and unknown MathML elements; a row's own
//! `width` applies, a float does not; display math is a block, its row centred. WPT mathml
//! `mrow/mrow-fallback-001`, `mpadded/mpadded-rendering-from-in-flow`,
//! `scripts/scripts-rendering-from-in-flow`, `tables/table-cell-mrow-layout`,
//! `relations/css-styling/width-height-005`, `relations/css-styling/ignored-properties-001`.
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

const STYLE: &str = "<!doctype html><style>body{margin:0;background:white;font:20px serif}</style>";

/// Each element's border box: `[x, y, width, height]`.
fn rects(body: &str, ids: &[&str]) -> Vec<[f64; 4]> {
    let dom: DomRef = Rc::new(RefCell::new(pyro_dom::Dom::new()));
    let doc = parse_document(&dom, "http://test/", &format!("{STYLE}{body}"));
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
            [b.x, b.y, b.width, b.height]
        })
        .collect()
}

const A: &str = "<mspace id=a width='10px' height='5px' depth='15px'></mspace>";
const B: &str = "<mspace id=b width='20px' height='30px' depth='10px'></mspace>";

#[test]
fn white_space_between_children_makes_no_space() {
    let r = rects(&format!("<div><math><mrow id=r>\n  {A}\n  {B}\n</mrow></math></div>"), &["r", "a", "b"]);
    assert_eq!(r[0][2], 30.0, "{r:?}");
    assert_eq!((r[1][0], r[2][0]), (r[0][0], r[0][0] + 10.0), "{r:?}");
}

#[test]
fn children_sit_on_the_baseline_and_the_row_holds_them() {
    let r = rects(&format!("<div><math><mrow id=r>{A}{B}</mrow></math></div>"), &["r", "a", "b"]);
    // Ascent 30 (b's height), descent 15 (a's depth).
    assert_eq!(r[0][3], 45.0, "{r:?}");
    assert_eq!((r[1][1] - r[0][1], r[2][1] - r[0][1]), (25.0, 0.0), "{r:?}");
}

#[test]
fn a_row_never_wraps() {
    let r = rects(&format!("<div style='width:25px'><math><mrow id=r>{A}{B}</mrow></math></div>"), &["r", "a", "b"]);
    assert_eq!(r[0][2], 30.0, "{r:?}");
    assert_eq!(r[2][0], r[1][0] + 10.0, "{r:?}");
}

#[test]
fn out_of_flow_and_hidden_children_take_no_place() {
    let r = rects(
        &format!(
            "<div><math><mrow id=r><mspace width='30px' style='position:absolute'></mspace>\
             <mspace width='8px' style='display:none'></mspace>{A}\
             <mspace width='16px' style='position:fixed'></mspace></mrow></math></div>"
        ),
        &["r", "a"],
    );
    assert_eq!((r[0][2], r[0][3]), (10.0, 20.0), "{r:?}");
    assert_eq!(r[1][0], r[0][0], "{r:?}");
}

#[test]
fn an_invalid_fraction_or_script_is_laid_out_as_a_row() {
    for tag in ["mfrac", "msub", "munderover", "mroot"] {
        let r = rects(
            &format!(
                "<div><math><{tag} id=e style='padding:0'>{A}</{tag}>\
                 <mrow id=r><mspace width='10px' height='5px' depth='15px'></mspace></mrow></math></div>"
            ),
            &["e", "r"],
        );
        assert_eq!((r[0][1], r[0][2], r[0][3]), (r[1][1], r[1][2], r[1][3]), "{tag}: {r:?}");
    }
}

#[test]
fn a_table_cell_semantics_and_an_unknown_element_are_rows() {
    for (tag, body) in [
        ("mtd", format!("<mtable><mtr><mtd id=e style='padding:0'>\n  {A}\n  {B}\n</mtd></mtr></mtable>")),
        ("mfenced", format!("<mfenced id=e>\n  {A}\n  {B}\n</mfenced>")),
        ("semantics", format!("<semantics id=e>\n  {B}\n</semantics>")),
    ] {
        let r = rects(&format!("<div><math>{body}</math></div>"), &["e", "b"]);
        let (width, height) = if tag == "semantics" { (20.0, 40.0) } else { (30.0, 45.0) };
        assert_eq!((r[0][2], r[0][3]), (width, height), "{tag}: {r:?}");
        assert_eq!(r[1][0] + 20.0, r[0][0] + width, "{tag}: {r:?}");
    }
}

#[test]
fn a_rows_width_applies_and_its_content_stays_at_the_start() {
    let r = rects(&format!("<div><math><mrow id=r style='width:100px'>{A}{B}</mrow></math></div>"), &["r", "a"]);
    assert_eq!((r[0][2], r[1][0]), (100.0, r[0][0]), "{r:?}");
    let r = rects(&format!("<div><math><mrow id=r style='width:5px'>{A}{B}</mrow></math></div>"), &["r", "a"]);
    assert_eq!((r[0][2], r[1][0]), (5.0, r[0][0]), "{r:?}");
}

#[test]
fn a_floated_row_is_a_row() {
    let r =
        rects(&format!("<div><math><mrow id=r style='float:right'>\n  {A}\n  {B}\n</mrow></math></div>"), &["r", "b"]);
    assert_eq!((r[0][2], r[0][3]), (30.0, 45.0), "{r:?}");
    assert_eq!(r[1][0], r[0][0] + 10.0, "{r:?}");
}

#[test]
fn display_math_is_a_block_with_its_row_centred() {
    let r = rects(&format!("<div style='width:200px'>x<math id=m display=block>{A}{B}</math>y</div>"), &["m", "a"]);
    assert_eq!((r[0][0], r[0][2], r[0][3]), (0.0, 200.0, 45.0), "{r:?}");
    assert_eq!(r[1][0], 85.0, "{r:?}");
}
