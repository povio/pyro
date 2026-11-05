//! The DOM's selector queries (`querySelector*`, `matches`, `closest`) go
//! through Stylo's selector matching over lp-dom nodes.

use std::cell::RefCell;
use std::rc::Rc;

use lp_dom::parse::{parse_document, DomRef};
use lp_dom::select::parse_selectors;
use lp_dom::*;

struct Page {
    dom: DomRef,
    doc: NodeId,
}

impl Page {
    fn new(html: &str) -> Page {
        let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
        let doc = parse_document(&dom, "http://test/", html);
        Page { dom, doc }
    }
    fn find(&self, css: &str) -> NodeId {
        self.dom.borrow().query_selector(self.doc, &parse_selectors(css).unwrap()).unwrap().unwrap()
    }
}

#[test]
fn dom_queries_go_through_stylo() {
    use lp_dom::select::parse_query;
    let p = Page::new(
        "<!doctype html><body><div id=a class='x y'><p id=p1>1</p><p id=p2 class=x>2</p></div>\
         <section id=s><div id=b><input id=i disabled></div></section></body>",
    );
    let dom = p.dom.borrow();
    let ids = |v: Vec<NodeId>| {
        v.into_iter().map(|n| dom.get_attribute(n, "id").unwrap().unwrap().to_owned()).collect::<Vec<_>>()
    };
    let all = |root: NodeId, s: &str| ids(dom.query_all(root, &parse_query(s).unwrap()).unwrap());
    assert_eq!(all(p.doc, ".x"), ["a", "p2"]);
    assert_eq!(all(p.doc, "div:has(> p.x) p"), ["p1", "p2"]);
    assert_eq!(all(p.doc, "p:nth-child(2 of .x, p)"), ["p2"]);
    assert_eq!(all(p.doc, "input:disabled, #p1"), ["p1", "i"]);
    let s = p.find("#s");
    assert_eq!(all(s, ":scope > div input"), ["i"]);
    assert_eq!(all(s, "body div"), ["b"], "ancestors outside the root still match");
    let first = dom.query_first(p.doc, &parse_query("p").unwrap()).unwrap();
    assert_eq!(first, Some(p.find("#p1")));
    let i = p.find("#i");
    assert!(dom.element_matches(i, &parse_query("section input:disabled").unwrap()).unwrap());
    assert_eq!(dom.element_closest(i, &parse_query("section, div").unwrap()).unwrap(), Some(p.find("#b")));
    assert!(parse_query("p:::nope").is_err());
}

#[test]
fn directionality_language_target_and_pseudo_element_parsing() {
    use lp_dom::select::parse_query;
    let p = Page::new(
        "<!doctype html><html lang=en-US><body>\
         <div id=ltr><span id=inherits></span></div><div id=rtl dir=rtl><span id=inrtl></span></div>\
         <div id=auto dir=auto>\u{5e9}\u{5dc}\u{5d5}\u{5dd}</div><bdi id=bdi>\u{627}</bdi>\
         <p id=fr lang=fr-x-standard><i id=frchild></i></p><p id=de lang=de></p>\
         <input id=tel type=tel dir=auto value=\"\u{5e9}\">\
         <a id=anchor></a></body></html>",
    );
    p.dom.borrow_mut().set_document_url(p.doc, "http://test/#anchor").unwrap();
    let dom = p.dom.borrow();
    let ids = |v: Vec<NodeId>| {
        v.into_iter().map(|n| dom.get_attribute(n, "id").unwrap().unwrap().to_owned()).collect::<Vec<_>>()
    };
    let all = |s: &str| ids(dom.query_all(p.doc, &parse_query(s).unwrap()).unwrap());
    assert_eq!(all("div:dir(rtl), span:dir(rtl), bdi:dir(rtl)"), ["rtl", "inrtl", "auto", "bdi"]);
    assert_eq!(all("span:dir(ltr)"), ["inherits"]);
    assert_eq!(all("input:dir(rtl)"), ["tel"], "dir=auto on a text control reads its value");
    assert_eq!(all("p:lang(fr)"), ["fr"], "fr-x-standard matches the range fr");
    assert_eq!(all("i:lang(fr-x)"), ["frchild"], "inherited, singleton subtag");
    assert_eq!(all("p:lang(fr-standard)"), Vec::<String>::new(), "a singleton stops extended filtering");
    assert_eq!(all("p:lang(de, it)"), ["de"]);
    assert_eq!(all("body :lang(en)").len(), 8, "the root's lang applies to every descendant without one");
    assert_eq!(all(":target"), ["anchor"]);
    assert!(parse_query("div::example").is_err(), "unknown pseudo-elements are invalid");
    assert!(parse_query("::-webkit-anything").is_ok(), "-webkit- pseudo-elements are accepted");
    assert!(parse_query("::slotted(.a)").is_ok());
    assert!(parse_query("::slotted()").is_err());
    assert!(parse_query("::slotted(0)").is_err());
    assert!(parse_query("::part(foo bar)").is_ok());
    assert!(parse_query("::part(0)").is_err());
}
