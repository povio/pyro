//! Preserved tabs go to tab stops (CSS Text 3 § 4.2, `tab-size`; reftest lane T). Stops count
//! from the start edge of the block container, every `tab-size` of the tab's own style, a number
//! in the advance of a space of the block container's font with its letter and word spacing; a
//! tab closer than 0.5ch to its stop goes on to the next; collapsed tabs are spaces; a justified
//! line keeps what is before its last tab in place. Inline-blocks of 10px (`b`) make positions
//! measurable; a line of non-breaking spaces is the ruler, so no result depends on the font.
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

const STYLE: &str = "<!doctype html><style>body{margin:0;font:20px/40px monospace}
  div{white-space:pre} b{display:inline-block;width:10px;height:10px}</style>";

/// The left edge of each element.
fn x_of(body: &str, ids: &[&str]) -> Vec<f64> {
    let dom: DomRef = Rc::new(RefCell::new(pyro_dom::Dom::new()));
    let doc = parse_document(&dom, "http://test/", &format!("{STYLE}{body}"));
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
        width: 600,
        height: 300,
        device_scale_factor: 1.0,
        live: Some(Live { page: 1, tree: Arc::new(tree), clip: None, interaction: None }),
        ..Default::default()
    };
    let mut renderer = Renderer::new();
    renderer.sync_layout(&snap, &(Arc::new(NoFetch) as Arc<dyn Fetcher>)).unwrap();
    keys.iter().map(|&key| renderer.element_box(1, key).unwrap().x).collect()
}

/// `count` non-breaking spaces.
fn spaces(count: usize) -> String {
    "&nbsp;".repeat(count)
}

#[track_caller]
fn assert_same(x: &[f64]) {
    assert!(x.windows(2).all(|pair| (pair[0] - pair[1]).abs() < 0.02), "{x:?}");
}

#[test]
fn a_preserved_tab_goes_to_the_next_stop() {
    let x = x_of(
        &format!(
            "<div>&#9;<b id=a></b></div><div>abc&#9;<b id=b></b></div><div>{}<b id=r></b></div>\
             <div>12345678&#9;<b id=c></b></div><div>{}<b id=r2></b></div>",
            spaces(8),
            spaces(16)
        ),
        &["a", "b", "r", "c", "r2"],
    );
    assert_same(&x[..3]);
    assert_same(&x[3..]);
    assert!(x[0] > 0.0 && x[3] > x[0]);
}

#[test]
fn tab_size_is_the_tabs_own_and_counts_in_the_blocks_spaces() {
    // The span's tab-size, in the block's spaces: neither the span's font size nor its spacing.
    let x = x_of(
        &format!(
            "<div style='tab-size:2'><span style='tab-size:4;font-size:2em;letter-spacing:5px'>&#9;</span><b id=a></b></div>\
             <div style='tab-size:3'>&#9;x&#9;<b id=b></b></div>\
             <div>{}<b id=r></b></div><div>{}<b id=r2></b></div>\
             <div style='tab-size:30px'>&#9;&#9;<b id=c></b></div>\
             <div style='tab-size:0'>&#9;&#9;<b id=d></b></div>",
            spaces(4),
            spaces(6)
        ),
        &["a", "r", "b", "r2", "c", "d"],
    );
    assert_same(&x[..2]);
    assert_same(&x[2..4]);
    assert_eq!(x[4], 60.0);
    assert_eq!(x[5], 0.0);
}

#[test]
fn letter_and_word_spacing_of_the_block_widen_the_stops() {
    let x = x_of(
        &format!(
            "<div style='letter-spacing:2px;word-spacing:10px;tab-size:4'>&#9;<b id=a></b></div>\
             <div>{}<b id=r style='margin-left:48px'></b></div>",
            spaces(4)
        ),
        &["a", "r"],
    );
    assert_same(&x);
}

#[test]
fn a_tab_closer_than_half_a_ch_to_its_stop_goes_to_the_next() {
    let x = x_of(
        "<div style='tab-size:100px'><b style='width:95px'></b>&#9;<b id=a></b></div>\
         <div style='tab-size:100px'><b style='width:80px'></b>&#9;<b id=b></b></div>",
        &["a", "b"],
    );
    assert_eq!(x, [200.0, 100.0]);
}

#[test]
fn stops_count_from_the_container_past_an_indent_and_a_float() {
    let x = x_of(
        "<div style='tab-size:100px;text-indent:30px'>a&#9;<b id=a></b></div>\
         <div style='tab-size:100px'><i style='float:left;width:30px;height:10px'></i>a&#9;<b id=b></b></div>\
         <div style='tab-size:100px;direction:rtl;width:400px'><i style='float:inline-start;width:30px;height:10px'></i>a&#9;<b id=c></b></div>\
         <div style='tab-size:100px;direction:rtl;width:400px'>a&#9;<b id=d></b></div>",
        &["a", "b", "c", "d"],
    );
    assert_eq!(&x[..2], [100.0, 100.0]);
    // Right to left: the float is on the right, and both lines' tabs end 100px from that edge.
    assert_same(&x[2..]);
}

#[test]
fn a_collapsed_tab_is_a_space_and_a_wrapping_line_breaks_after_its_tabs() {
    let x = x_of(
        "<div style='white-space:normal'>a&#9;<b id=a></b></div><div style='white-space:normal'>a <b id=r></b></div>\
         <div style='white-space:pre-wrap;width:200px;tab-size:100px'>a&#9;&#9;<b id=b></b></div>",
        &["a", "r", "b"],
    );
    assert_same(&x[..2]);
    // Both tabs fit (the second ends at the line's end); the box wraps to the next line.
    assert_eq!(x[2], 0.0);
}

#[test]
fn a_justified_line_moves_only_what_follows_its_last_tab() {
    let line = |align: &str, a: &str, b: &str| {
        format!(
            "<div style='white-space:pre-wrap;text-align:{align};width:300px;tab-size:100px'>\
             a b&#9;<b id={a}></b>c d <b id={b}></b> e <b style='width:290px'></b></div>"
        )
    };
    let x = x_of(&format!("{}{}", line("justify", "a", "b"), line("left", "c", "d")), &["a", "b", "c", "d"]);
    // The spaces before the tab keep their width; those after it take the line's free space.
    assert_eq!((x[0], x[2]), (100.0, 100.0));
    assert!(x[1] > x[3] + 10.0, "{x:?}");
}
