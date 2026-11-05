//! The compiled selector queries (`select_query`: type, id, class and
//! attribute tests under descendant and child combinators) agree with the
//! `selectors` crate matching every element (`Dom::matches`): tree order,
//! backtracking over combinators, scopes, duplicate ids and an id outside the
//! scope, shadow and detached trees, quirks mode, SVG names, attribute values.

use std::cell::RefCell;
use std::rc::Rc;

use lp_dom::parse::{parse_document, parse_fragment, DomRef};
use lp_dom::select::{parse_query, parse_selectors};
use lp_dom::*;

fn doc(html: &str) -> (DomRef, NodeId) {
    let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
    let d = parse_document(&dom, "http://test/", html);
    dom.borrow_mut().set_document_active(d, true).unwrap();
    (dom, d)
}

fn elements(dom: &Dom, root: NodeId) -> Vec<NodeId> {
    dom.descendants(root).unwrap().filter(|&n| dom.node_type(n).unwrap() == NodeType::Element).collect()
}

/// `querySelectorAll`, `querySelector`, `matches` and `closest` through the
/// compiled query, against `selectors` matching each element.
#[track_caller]
fn agree(dom: &Dom, root: NodeId, css: &str) {
    let q = parse_query(css).unwrap();
    assert!(q.is_compiled(), "{css} is compiled");
    let sel = parse_selectors(css).unwrap();
    let want: Vec<NodeId> =
        elements(dom, root).into_iter().filter(|&n| n != root && dom.matches(n, &sel).unwrap()).collect();
    assert_eq!(dom.query_all(root, &q).unwrap(), want, "querySelectorAll({css})");
    assert_eq!(dom.query_first(root, &q).unwrap(), want.first().copied(), "querySelector({css})");
    for n in elements(dom, root) {
        assert_eq!(dom.element_matches(n, &q).unwrap(), dom.matches(n, &sel).unwrap(), "matches({css})");
        assert_eq!(dom.element_closest(n, &q).unwrap(), dom.closest(n, &sel).unwrap(), "closest({css})");
    }
}

const SELECTORS: &[&str] = &[
    "*",
    "div",
    "DIV",
    "#a",
    "#missing",
    ".x",
    ".x.y",
    "div.x",
    "span#b.x",
    "[data-k]",
    "[data-k='1']",
    "[data-k^='1']",
    "[data-k$=a]",
    "[data-k*=b]",
    "[data-k~=c]",
    "[data-k|=d]",
    "[type=TEXT]",
    "[type=TEXT s]",
    "[data-k=AB i]",
    "div span",
    "div > span",
    ".x span",
    "#a span",
    "#a > span",
    "#s div",
    "#s > div > p",
    "body .x > span",
    "div div span",
    "div > div span",
    "div div > span",
    "section > div .x",
    "main #a span",
    "* > *",
    "p, span",
    "#a, #b",
    ".y span, section p",
    "html > body > div",
    "*|div",
];

const PAGE: &str = r#"<!doctype html><body>
<div id=a class="x y"><span id=b class=x data-k=1>one</span><span id=a class="X">dup a</span>
  <div><div class=x><span data-k="1ab c">deep</span></div></div></div>
<section id=s><div id=a>third a<p data-k=d-e>p</p></div><p class="y  x" data-k>p</p>
  <div class=x><input type=text data-k=ab><div><span>in s</span></div></div></section>
<main><div id=m><span data-k="x b">m</span></div></main>
<svg viewBox="0 0 1 1"><rect id=b class=x data-k=2/><foreignObject><div class=x><span>in svg</span></div></foreignObject></svg>
<a href=/x id=link>l</a></body>"#;

#[test]
fn compiled_queries_agree_with_matching_every_element() {
    let (dom, d) = doc(PAGE);
    let dom = dom.borrow();
    let one = |css: &str| dom.query_first(d, &parse_query(css).unwrap()).unwrap().unwrap();
    let section = one("#s");
    let first_a = one("div");
    let main = one("main");
    for css in SELECTORS {
        agree(&dom, d, css);
        // Scoped to an element: the first `#a` of the document is outside.
        agree(&dom, section, css);
        // Scoped to an element that has the id itself (an ancestor compound may match it).
        agree(&dom, first_a, css);
        agree(&dom, main, css);
    }
}

#[test]
fn an_id_in_an_ancestor_compound_bounds_the_walk() {
    let (dom, d) = doc(r#"<!doctype html><body><div id=u><p><span id=in>x</span></p></div>
        <div id=v><span id=out>y</span></div></body>"#);
    let all = |root: NodeId, css: &str| dom.borrow().query_all(root, &parse_query(css).unwrap()).unwrap();
    let one = |css: &str| dom.borrow().query_first(d, &parse_query(css).unwrap()).unwrap().unwrap();
    let (u, v, inner) = (one("#u"), one("#v"), one("#in"));
    assert_eq!(all(d, "#u span"), [inner]);
    assert_eq!(all(u, "#u span"), [inner], "the root itself has the id");
    assert_eq!(all(v, "#u span"), Vec::<NodeId>::new(), "the element with the id is elsewhere");
    let p = one("#u p");
    assert_eq!(all(p, "#u span"), [inner], "the element with the id is above the root");
    // Ids that change, and a second element with the id: the index follows.
    dom.borrow_mut().set_attribute(v, "id", "u").unwrap();
    let out = one("#out");
    assert_eq!(all(d, "#u span"), [inner, out]);
    dom.borrow_mut().remove_attribute(u, "id").unwrap();
    assert_eq!(all(d, "#u span"), [out]);
    assert_eq!(all(d, "#u > span#out"), [out]);
    assert_eq!(all(d, "#gone span"), Vec::<NodeId>::new());
}

#[test]
fn shadow_and_detached_trees() {
    let (dom, d) = doc(PAGE);
    let body = dom.borrow().body(d).unwrap().unwrap();
    let frag = parse_fragment(&mut dom.borrow_mut(), body, "<div id=a class=x><i id=c></i></div><b id=c></b>").unwrap();
    for css in ["#a", "#c", ".x i", "#a i", "div > i", "b, i"] {
        agree(&dom.borrow(), frag, css);
    }
    // Combinators stop at a shadow root: the host is no ancestor there.
    let host = dom.borrow().query_first(d, &parse_query("#s").unwrap()).unwrap().unwrap();
    let shadow = dom.borrow_mut().attach_shadow(host, ShadowRootMode::Open).unwrap();
    let inner = parse_fragment(&mut dom.borrow_mut(), body, "<div id=a class=x><em id=e>e</em></div>").unwrap();
    dom.borrow_mut().append_child(shadow, inner).unwrap();
    let dom = dom.borrow();
    for css in ["em", "#a em", "section em", "div > em", "#e", "#s em", ".x em"] {
        agree(&dom, shadow, css);
    }
    let em = dom.query_first(shadow, &parse_query("em").unwrap()).unwrap().unwrap();
    assert!(!dom.element_matches(em, &parse_query("section em").unwrap()).unwrap());
    assert_eq!(dom.query_first(d, &parse_query("#e").unwrap()).unwrap(), None);
}

#[test]
fn quirks_mode_and_xml_names() {
    // No doctype: quirks mode, ids and classes compare case-insensitively.
    let (dom, d) = doc(r#"<div id=Foo class=Bar><span id=foo class=bar>x</span></div>"#);
    for css in ["#foo", "#FOO span", ".BAR", ".bar > span", "DIV SPAN", "#Foo.bar"] {
        agree(&dom.borrow(), d, css);
    }
    // SVG elements keep their case; HTML ones compare in lowercase.
    let (dom, d) = doc(
        r#"<!doctype html><svg><foreignObject id=f viewBox="0 0 1 1"><div><linearGradient/></div></foreignObject></svg>"#,
    );
    for css in ["foreignObject", "foreignobject div", "[viewBox]", "[viewbox]", "linearGradient", "DIV lineargradient"]
    {
        agree(&dom.borrow(), d, css);
    }
}

#[test]
fn queries_outside_the_compiled_form_still_work() {
    let (dom, d) = doc(PAGE);
    let dom = dom.borrow();
    for css in
        [":scope > div", "div:first-child", "div + span", "div ~ p", "a:link", "|div", "span::before", ":is(div) span"]
    {
        let q = parse_query(css).unwrap();
        assert!(!q.is_compiled(), "{css} is not compiled");
        dom.query_all(d, &q).unwrap();
    }
    assert!(dom.element_matches(d, &parse_query("div").unwrap()).is_err(), "a document is no element");
}
