//! `display: contents` and the boxes it leaves (WPT css-display, lane ID): white space between hoisted texts stays in
//! their run (flex items and block lines), a contents element's `::before` and `::after` are its parent's inline
//! content, a `<fieldset>` lays out as a block or inline-block whatever table `display` it has, and `flow-root
//! list-item` parses (servo had it behind the gecko feature).
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
        height: 400,
        device_scale_factor: 1.0,
        live: Some(Live { page: 1, tree: Arc::new(tree), clip: None, interaction: None }),
        ..Default::default()
    };
    let mut renderer = Renderer::new();
    renderer.sync_layout(&snap, &(Arc::new(NoFetch) as Arc<dyn Fetcher>)).unwrap();
    keys.into_iter().map(|key| renderer.element_box(1, key).unwrap()).collect()
}


const BODY: &str = "<!doctype html><body style='margin:0;font:16px/20px Arial'>";

/// The x of `#m` after `content` in a flex row or a block, with the white space between the hoisted texts and without.
fn mark_x(container: &str, content: &str) -> f64 {
    let html = format!("{BODY}<div style='{container}'>{content}<span id=m style='display:inline-block;width:1px;height:1px'></span></div>");
    boxes(&html, &["m"])[0].x
}

#[test]
fn white_space_between_contents_texts_stays_in_their_run() {
    // In a flex row `<b>` is an item of its own, so the space before it ends the anonymous item and goes.
    for (container, spaces) in [("display:flex", 1.0), ("display:block", 2.0)] {
        let with = mark_x(container, "x<div style='display:contents'>y</div> <div style='display:contents'>z</div><div style='display:contents'>a <b>b</b></div>");
        let without = mark_x(container, "x<div style='display:contents'>y</div><div style='display:contents'>z</div><div style='display:contents'>a<b>b</b></div>");
        assert!(with > without + 3.0 * spaces, "{container}: {with} vs {without} ({spaces} spaces)");
    }
    // A run of only white space makes no flex item: `#m` starts the row.
    assert_eq!(mark_x("display:flex", " <div style='display:contents'> </div> "), 0.0);
}

#[test]
fn contents_pseudo_elements_are_inline_content_of_the_parent() {
    let style = "<style>.c{display:contents}.p::before{content:'AAA'}.p::after{content:'SSS'}</style>";
    for inner in ["<div class='c p'>x</div>", "<span class='c p'></span>", "y<div class='c p'></div>"] {
        let plain = inner.replace(" p'", "'");
        let html = |inner: &str| format!("{BODY}{style}<div id=d>{inner}<span id=m>|</span></div>");
        let with = boxes(&html(inner), &["m", "d"]);
        let without = boxes(&html(&plain), &["m", "d"]);
        assert!(with[0].x > without[0].x + 20.0, "{inner}: generated text before #m ({} vs {})", with[0].x, without[0].x);
        assert_eq!(with[1].height, 20.0, "{inner}: one line");
    }
    // Alone in a block: one line, not a block each.
    let html = format!("{BODY}{style}<div id=d><div class='c p'></div></div>");
    assert_eq!(boxes(&html, &["d"])[0].height, 20.0);
}

#[test]
fn fieldset_lays_out_as_a_block_or_an_inline_block() {
    let field = "width:40px;height:20px;margin:0;padding:0;border:none";
    for display in ["table-cell", "table-caption", "table-row", "table"] {
        let html = format!("{BODY}<div><span><fieldset id=f style='display:{display};{field}'></fieldset></span><i id=i style='display:inline-block;width:20px;height:20px'></i></div>");
        let b = boxes(&html, &["f", "i"]);
        assert_eq!((b[0].width, b[0].height), (40.0, 20.0), "{display}");
        assert_eq!(b[1].y, 20.0, "{display}: a block, the next box on the next line");
    }
    let html = format!("{BODY}<div style='font-size:0;line-height:0'><i style='display:inline-block;width:20px;height:20px'></i><fieldset id=f style='display:inline;{field}'></fieldset></div>");
    let b = boxes(&html, &["f"]);
    assert_eq!((b[0].x, b[0].width, b[0].height), (20.0, 40.0, 20.0), "inline: an inline-block");
}

#[test]
fn flow_root_list_item_is_a_formatting_context() {
    let html = format!("{BODY}<div style='border:1px solid'><span id=l style='display:flow-root list-item;margin:20px 0'><div style='margin:20px 0'>x</div></span></div>");
    let b = boxes(&html, &["l"]);
    assert_eq!((b[0].y, b[0].height), (21.0, 60.0), "its child's margins stay inside it");
}
