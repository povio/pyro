//! Inline-size containment (`contain: inline-size`, css-contain-2 §3.2; blitz-dom `layout/mod.rs`
//! `inline_size_contained_width`): a box whose width its parent leaves to it (shrink-to-fit, or
//! measured for its min-content or max-content contribution) is as wide as if it were empty with a
//! content of `contain-intrinsic-width`; its height still follows from its content laid out at that
//! width. Against Chrome 157.
//!
//! Danex's list cells (Pyro 0.0.38, fault 5): a column `inline-flex` holds a truncating line with
//! Tailwind's `[contain:inline-size]` and a hidden `::after` whose `min-width` sizes the cell. The
//! line gave the `inline-flex` its whole text width (320 px, up to `max-width`) instead of none, so
//! the value ran 56 px further over the next column than in Chrome (264 px).
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
    .c{contain:inline-size}.c::before{content:'';display:block;width:40px;height:20px}</style>";

/// WPT `css/css-sizing/contain-intrinsic-size/contain-intrinsic-size-028.html`, its horizontal
/// boxes: an `inline-block` is as wide as `contain-intrinsic-width` (none: 0), as tall as its
/// content, whatever `contain-intrinsic-height` says.
#[test]
fn a_shrink_to_fit_box_is_as_wide_as_its_intrinsic_width_override() {
    let html = format!(
        "{STYLE}<div><span id=none class=c style='display:inline-block;contain-intrinsic-size:none none'></span>\
         <span id=height class=c style='display:inline-block;contain-intrinsic-size:none 50px'></span>\
         <span id=width class=c style='display:inline-block;contain-intrinsic-size:100px none'></span>\
         <span id=both class=c style='display:inline-block;contain-intrinsic-size:100px 50px'></span></div>"
    );
    let b = boxes(&html, &["none", "height", "width", "both"]);
    let sizes: Vec<_> = b.iter().map(|b| (b.width, b.height)).collect();
    assert_eq!(sizes, [(0.0, 20.0), (0.0, 20.0), (100.0, 20.0), (100.0, 20.0)]);
}

/// Danex's list cell: the `inline-flex` is as wide as its `::after`'s `min-width` (31ch, about
/// 264 px), not its line's text (330 px, cut at `max-width: 20rem`).
#[test]
fn a_contained_line_gives_its_column_flex_container_none_of_its_width() {
    let html = format!(
        "{STYLE}<style>.m::after{{content:attr(data-measure);visibility:hidden;display:block;height:0;\
         min-width:31ch;overflow:hidden;overflow-wrap:anywhere}}</style>\
         <div style='width:200px'><span id=f style='display:inline-flex;flex-direction:column;min-width:0;max-width:20rem'>\
         <span class=m data-measure='Lakeshore marine certificate' style='display:flex;flex-direction:column;min-width:0'>\
         <span id=line style='display:block;overflow:hidden;white-space:nowrap;contain:inline-size'>\
         Lakeshore marine certificate · DNX-KCS-2608P.01C and some more words</span></span></span>\
         <span id=probe style='display:inline-block;width:31ch'></span></div>"
    );
    let [f, line, probe] = boxes(&html, &["f", "line", "probe"])[..] else { unreachable!() };
    assert!((f.width - probe.width).abs() < 0.6, "the inline-flex is 31ch wide ({f:?}, {probe:?})");
    assert!((line.width - f.width).abs() < 0.6, "the line is stretched to it ({line:?})");
    // With nothing else in it, the column flex container is empty-wide.
    let html = format!(
        "{STYLE}<div style='width:200px'><span id=f style='display:inline-flex;flex-direction:column'>\
         <span style='display:block;white-space:nowrap;contain:inline-size'>Decide</span></span></div>"
    );
    assert_eq!(boxes(&html, &["f"])[0].width, 0.0);
}

/// A box whose width its parent gives it is not shrunk: a block in flow, a stretched flex item.
/// A flex item sized by its content (`flex: none`) is as wide as its padding.
#[test]
fn boxes_given_their_width_keep_it() {
    let html = format!(
        "{STYLE}<div style='width:300px'><div id=block class=c></div></div>\
         <div style='display:flex;flex-direction:column;width:300px'><div id=stretched class=c></div></div>\
         <div style='display:flex;width:300px'><div id=none class=c style='flex:none;padding:0 5px'></div>\
         <div id=grows class=c style='flex:1'></div></div>"
    );
    let b = boxes(&html, &["block", "stretched", "none", "grows"]);
    assert_eq!((b[0].width, b[0].height), (300.0, 20.0), "block");
    assert_eq!((b[1].width, b[1].height), (300.0, 20.0), "stretched flex item");
    assert_eq!((b[2].width, b[2].height), (10.0, 20.0), "content-sized flex item");
    assert_eq!((b[3].width, b[3].height), (290.0, 20.0), "growing flex item");
}
