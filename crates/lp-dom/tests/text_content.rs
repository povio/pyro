//! `Node.textContent` without copying: borrowed from the one node that
//! holds all of it, concatenated in one walk otherwise.

use std::borrow::Cow;
use std::cell::RefCell;
use std::rc::Rc;

use lp_dom::parse::{parse_document, DomRef};
use lp_dom::select::parse_query;
use lp_dom::*;

#[test]
fn text_content_borrows_a_single_text_node() {
    let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
    let d = parse_document(&dom, "http://test/", "<!doctype html><body><p id=one>alone</p><p id=two>a<b>b</b><i></i>c</p><p id=none></p><p id=e><b></b>x</p></body>");
    let dom = dom.borrow();
    let one = |css: &str| dom.query_first(d, &parse_query(css).unwrap()).unwrap().unwrap();
    let t = dom.text_content_ref(one("#one")).unwrap().unwrap();
    assert!(matches!(t, Cow::Borrowed("alone")));
    assert_eq!(dom.text_content_ref(one("#two")).unwrap().unwrap(), "abc");
    assert!(matches!(dom.text_content_ref(one("#none")).unwrap().unwrap(), Cow::Borrowed("")));
    assert!(matches!(dom.text_content_ref(one("#e")).unwrap().unwrap(), Cow::Borrowed("x")));
    let first_text = dom.first_child(one("#one")).unwrap().unwrap();
    assert!(matches!(dom.text_content_ref(first_text).unwrap().unwrap(), Cow::Borrowed("alone")));
    assert_eq!(dom.text_content_ref(d).unwrap(), None);
    assert_eq!(dom.text_content(one("body")).unwrap().unwrap(), "aloneabcx");
}
