//! SVG `<script>` in HTML: the parser yields it at its end tag (or at `<script/>`) like an
//! HTML script, and its source is `href`, else `xlink:href`
//! (html/semantics/scripting-1/the-script-element/execution-timing/131.html–146-href.html).

use std::cell::RefCell;
use std::rc::Rc;

use pyro_dom::parse::{DocumentParser, DomRef, ParseStep};
use pyro_dom::*;

fn parser() -> (DomRef, NodeId, DocumentParser) {
    let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
    let d = dom.borrow_mut().create_document(DocumentKind::Html, "http://t/");
    let p = DocumentParser::new(dom.clone(), d, true).unwrap();
    (dom, d, p)
}

fn local(dom: &DomRef, node: NodeId) -> String {
    dom.borrow().element(node).unwrap().name.local.to_string()
}

#[test]
fn the_svg_script_end_tag_yields_the_script_before_the_content_after_it() {
    let (dom, _, mut p) = parser();
    let step = p.feed("<svg><script>log(1)</script><rect id=after /></svg><p>");
    let ParseStep::Script(script) = step else { panic!("no script: {step:?}") };
    assert_eq!(local(&dom, script), "script");
    assert_eq!(&*dom.borrow().element(script).unwrap().name.ns, "http://www.w3.org/2000/svg");
    let info = dom.borrow().script_info(script).unwrap();
    assert_eq!(info.inline_text, "log(1)");
    assert!(info.parser_inserted);
    // The parser stopped at the script: nothing after it is in the tree yet.
    assert_eq!(dom.borrow().next_sibling(script).unwrap(), None);
    assert_eq!(p.resume(), ParseStep::NeedInput);
    let after = dom.borrow().next_sibling(script).unwrap().expect("parsing resumed");
    assert_eq!(local(&dom, after), "rect");
}

#[test]
fn nested_svg_scripts_yield_the_inner_one_first() {
    let (dom, _, mut p) = parser();
    let ParseStep::Script(inner) = p.feed("<svg><script>a<script>b</script>c</script></svg>") else { panic!() };
    assert_eq!(dom.borrow().script_info(inner).unwrap().inline_text, "b");
    let ParseStep::Script(outer) = p.resume() else { panic!() };
    assert_ne!(inner, outer);
    assert_eq!(dom.borrow().script_info(outer).unwrap().inline_text, "ac");
}

#[test]
fn a_self_closing_svg_script_yields_at_once() {
    let (dom, _, mut p) = parser();
    let ParseStep::Script(script) = p.feed("<svg><script href=a.js /><g></g></svg>") else { panic!() };
    let info = dom.borrow().script_info(script).unwrap();
    assert_eq!(info.src.as_deref(), Some("a.js"));
    assert_eq!(dom.borrow().first_child(script).unwrap(), None);
    assert_eq!(p.resume(), ParseStep::NeedInput);
    let g = dom.borrow().next_sibling(script).unwrap().expect("the <g> is the script's sibling");
    assert_eq!(local(&dom, g), "g");
}

#[test]
fn an_svg_script_source_is_href_then_xlink_href_and_it_is_never_async() {
    let src = |html: &str| {
        let (dom, _, mut p) = parser();
        let ParseStep::Script(script) = p.feed(html) else { panic!("no script: {html}") };
        let info = dom.borrow().script_info(script).unwrap();
        assert!(!info.async_ && !info.defer && !info.nomodule, "{html}");
        info.src
    };
    assert_eq!(src("<svg><script href=a.js xlink:href=b.js async defer></script>").as_deref(), Some("a.js"));
    assert_eq!(src("<svg><script xlink:href=b.js nomodule></script>").as_deref(), Some("b.js"));
    // `src` is HTML's attribute: an SVG script with only `src` is inline.
    assert_eq!(src("<svg><script src=a.js>x</script>"), None);
}
