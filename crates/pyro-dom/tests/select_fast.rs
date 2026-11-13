//! `querySelector(All)` fast paths (`#id` through the id index, one class,
//! one tag, one attribute presence) agree with matching every element
//! (`Dom::matches`) in tree order: duplicate ids, scopes, detached trees,
//! shadow trees, quirks mode, SVG and namespaced attributes.

use std::cell::RefCell;
use std::rc::Rc;

use pyro_dom::parse::{parse_document, parse_fragment, DomRef};
use pyro_dom::select::parse_selectors;
use pyro_dom::*;

fn doc(html: &str) -> (DomRef, NodeId) {
    let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
    let d = parse_document(&dom, "http://test/", html);
    dom.borrow_mut().set_document_active(d, true).unwrap();
    (dom, d)
}

/// Every element under `root` (not `root`) matching `css`, in tree order.
fn reference(dom: &Dom, root: NodeId, css: &str) -> Vec<NodeId> {
    let sel = parse_selectors(css).unwrap();
    dom.descendants(root)
        .unwrap()
        .skip(1)
        .filter(|&n| dom.node_type(n).unwrap() == NodeType::Element && dom.matches(n, &sel).unwrap())
        .collect()
}

#[track_caller]
fn agree(dom: &Dom, root: NodeId, css: &str) {
    let sel = parse_selectors(css).unwrap();
    let all = dom.query_selector_all(root, &sel).unwrap();
    let want = reference(dom, root, css);
    assert_eq!(all, want, "querySelectorAll({css})");
    assert_eq!(dom.query_selector(root, &sel).unwrap(), want.first().copied(), "querySelector({css})");
}

const SELECTORS: &[&str] = &[
    "#a",
    "#b",
    "#A",
    "#missing",
    ".x",
    ".X",
    ".y",
    "div",
    "DIV",
    "span",
    "svg",
    "foreignObject",
    "foreignobject",
    "rect",
    "[data-k]",
    "[DATA-K]",
    "[viewBox]",
    "[viewbox]",
    "[href]",
    "[id]",
    "[class]",
];

const PAGE: &str = r#"<!doctype html><body>
<div id=a class="x y"><span id=b class=x data-k=1>one</span><span id=a class="X">dup a</span></div>
<section id=s><div id=a>third a</div><p class="y  x" data-k>p</p></section>
<svg viewBox="0 0 1 1"><rect id=b class=x data-k=2/><foreignObject><div class=x>in svg</div></foreignObject></svg>
<a href=/x id=link>l</a></body>"#;

#[test]
fn fast_paths_agree_with_matching_every_element() {
    let (dom, d) = doc(PAGE);
    let dom = dom.borrow();
    let section = dom.query_selector(d, &parse_selectors("#s").unwrap()).unwrap().unwrap();
    let first_a = dom.query_selector(d, &parse_selectors("div").unwrap()).unwrap().unwrap();
    for css in SELECTORS {
        agree(&dom, d, css);
        // Scoped to an element: the first `#a` of the document is outside.
        agree(&dom, section, css);
        // Scoped to an element that itself has the id (it is not a match).
        agree(&dom, first_a, css);
    }
}

#[test]
fn duplicate_ids_and_id_changes() {
    let (dom, d) = doc(PAGE);
    let sel = parse_selectors("#a").unwrap();
    assert_eq!(dom.borrow().query_selector_all(d, &sel).unwrap().len(), 3);
    // The first `#a` loses its id: the next one in tree order answers.
    let first = dom.borrow().query_selector(d, &sel).unwrap().unwrap();
    dom.borrow_mut().set_attribute(first, "id", "z").unwrap();
    agree(&dom.borrow(), d, "#a");
    agree(&dom.borrow(), d, "#z");
    // An element moved in front of the others takes over.
    let body = dom.borrow().body(d).unwrap().unwrap();
    let late = dom.borrow().query_selector(d, &parse_selectors("#link").unwrap()).unwrap().unwrap();
    dom.borrow_mut().set_attribute(late, "id", "a").unwrap();
    let firstchild = dom.borrow().first_child(body).unwrap();
    dom.borrow_mut().insert_before(body, late, firstchild).unwrap();
    assert_eq!(dom.borrow().query_selector(d, &sel).unwrap(), Some(late));
    agree(&dom.borrow(), d, "#a");
}

#[test]
fn detached_and_shadow_trees_keep_their_own_ids() {
    let (dom, d) = doc(PAGE);
    let body = dom.borrow().body(d).unwrap().unwrap();
    // A detached subtree.
    let frag = parse_fragment(&mut dom.borrow_mut(), body, "<div id=a class=x><i id=c></i></div><b id=c></b>").unwrap();
    {
        let dom = dom.borrow();
        for css in ["#a", "#c", ".x", "i", "b"] {
            agree(&dom, frag, css);
        }
        assert_eq!(dom.query_selector_all(d, &parse_selectors("#c").unwrap()).unwrap(), Vec::<NodeId>::new());
    }
    // A shadow tree: its ids are not the document's, and the other way round.
    let host = dom.borrow().query_selector(d, &parse_selectors("#s").unwrap()).unwrap().unwrap();
    let shadow = dom.borrow_mut().attach_shadow(host, ShadowRootMode::Open).unwrap();
    let inner = dom.borrow_mut().create_element(d, "em").unwrap();
    dom.borrow_mut().set_attribute(inner, "id", "shadowed").unwrap();
    dom.borrow_mut().append_child(shadow, inner).unwrap();
    let dom = dom.borrow();
    assert_eq!(dom.query_selector(d, &parse_selectors("#shadowed").unwrap()).unwrap(), None);
    assert_eq!(dom.query_selector(shadow, &parse_selectors("#shadowed").unwrap()).unwrap(), Some(inner));
    assert_eq!(dom.query_selector(shadow, &parse_selectors("#a").unwrap()).unwrap(), None);
    agree(&dom, shadow, "em");
}

#[test]
fn quirks_mode_matches_ids_and_classes_case_insensitively() {
    // No doctype: quirks mode.
    let (dom, d) = doc(r#"<div id=Foo class=Bar><span id=foo class=bar>x</span></div>"#);
    let dom = dom.borrow();
    for css in ["#foo", "#FOO", "#Foo", ".bar", ".BAR", ".Bar", "div", "SPAN"] {
        agree(&dom, d, css);
    }
    assert_eq!(dom.query_selector_all(d, &parse_selectors("#FOO").unwrap()).unwrap().len(), 2);
    assert_eq!(dom.query_selector_all(d, &parse_selectors(".bar").unwrap()).unwrap().len(), 2);
}
