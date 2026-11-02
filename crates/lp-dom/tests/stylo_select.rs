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
