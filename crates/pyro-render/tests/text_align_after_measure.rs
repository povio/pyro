//! Text aligned in a box that a measuring run laid out at another width first.
//!
//! Danex's home triage (Pyro 0.0.38, fault 5): each table cell holds a column `inline-flex` whose
//! block child shows one line. Taffy measures that stretched flex item at the available width
//! (the cell's), then lays it out at its container's width. The cell is measured before it is
//! laid out, so the item's final layout came from the cache, and its text kept the alignment of
//! the measuring run: `text-align: right` put each action link about a cell's width to the
//! right, past the table and the viewport. Chrome 157 aligns the text to the item.
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
        width: 800,
        height: 600,
        device_scale_factor: 1.0,
        live: Some(Live { page: 1, tree: Arc::new(tree), clip: None, interaction: None }),
        ..Default::default()
    };
    let mut renderer = Renderer::new();
    renderer.sync_layout(&snap, &(Arc::new(NoFetch) as Arc<dyn Fetcher>)).unwrap();
    keys.into_iter().map(|key| renderer.element_box(1, key).unwrap()).collect()
}

const STYLE: &str = "<style>body{margin:0;font:14px/20px sans-serif}\
    .f{display:inline-flex;flex-direction:column;min-width:6em;vertical-align:middle}\
    .b{display:block;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}</style>";

/// The column flex item `#b` (wider than its text, by the container's `min-width`) and its text `#a`.
const ITEM: &str = "<span class=f><span id=b class=b><span><a id=a href=#>Decide</a></span></span></span>";

/// `#a` sits in `#b` where `text-align` puts it: at its end (`right`) or in its middle (`center`).
fn check(case: &str, html: &str, align: &str) {
    let [cell, b, a] = boxes(html, &["cell", "b", "a"])[..] else { unreachable!() };
    assert!(b.width > a.width + 10.0, "{case}: the item is wider than its text ({b:?}, {a:?})");
    assert!(
        b.x >= cell.x && b.x + b.width <= cell.x + cell.width + 0.02,
        "{case}: the item is in its cell ({cell:?}, {b:?})"
    );
    let (expected, got) = match align {
        "right" => (b.x + b.width, a.x + a.width),
        _ => (b.x + b.width / 2.0, a.x + a.width / 2.0),
    };
    assert!(
        (got - expected).abs() < 0.6,
        "{case}: text {align}-aligned in its item at {expected}, not {got} ({b:?}, {a:?})"
    );
}

#[test]
fn text_aligned_in_a_stretched_column_flex_item_of_a_table_cell() {
    for align in ["right", "center"] {
        let html = format!(
            "{STYLE}<table style='width:400px;border-collapse:collapse'><tr><td style='width:50%'>Step</td>\
             <td id=cell style='text-align:{align};padding:0'>{ITEM}</td></tr></table>"
        );
        check("auto table", &html, align);
    }
}

/// Danex's triage: a fixed table with a colgroup and a minimum width in a scroller, cells holding
/// the column `inline-flex` with a `::after` that sets its minimum width.
#[test]
fn danex_triage_action_cell() {
    let html = format!(
        "{STYLE}<style>.m::after{{content:attr(data-measure);visibility:hidden;display:block;height:0;\
         min-width:6ch;overflow:hidden;overflow-wrap:anywhere}}td{{padding:10px 12px;vertical-align:top}}</style>\
         <div style='width:700px;overflow-x:auto'><table style='width:100%;min-width:40rem;table-layout:fixed;\
         border-collapse:collapse'><colgroup><col><col style='width:14rem'><col style='width:11rem'></colgroup><tbody><tr>\
         <td>Decide 2 screening matches</td><td>CP-HKF</td><td id=cell style='text-align:right;padding-right:20px'>\
         <span class=f style='min-width:0;max-width:20rem'><span class='m' data-measure=Decide \
         style='display:flex;flex-direction:column;min-width:0'><span id=b class=b style='contain:inline-size'><span>\
         <span style='display:contents'><a id=a href=#>Decide</a></span></span></span></span></span></td></tr></tbody>\
         </table></div>"
    );
    let [cell, b, a] = boxes(&html, &["cell", "b", "a"])[..] else { unreachable!() };
    let content_end = cell.x + cell.width - 20.0;
    assert!((b.x + b.width - content_end).abs() < 0.6, "the item ends at the cell's content edge ({cell:?}, {b:?})");
    assert!((a.x + a.width - content_end).abs() < 0.6, "the link ends at the cell's content edge ({cell:?}, {a:?})");
}

#[test]
fn text_aligned_in_a_stretched_column_flex_item_of_a_flex_item() {
    let html = format!(
        "{STYLE}<div style='display:flex;width:300px'><div id=cell style='flex:1;text-align:right'>{ITEM}</div></div>"
    );
    check("flex item", &html, "right");
}
