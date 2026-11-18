//! `Dom::only_detached_changes_since`: the changes layout and computed styles cannot see,
//! for which the layout engine skips its sync.

use std::cell::RefCell;
use std::rc::Rc;

use pyro_dom::parse::{parse_document, DomRef};
use pyro_dom::*;

fn doc(html: &str) -> (DomRef, NodeId) {
    let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
    let d = parse_document(&dom, "http://test/", html);
    dom.borrow_mut().set_document_active(d, true).unwrap();
    (dom, d)
}

#[test]
fn changes_outside_the_document_are_told_apart_from_changes_in_it() {
    let (dom, d) = doc("<!doctype html><body><div id=a><span id=s>x</span></div></body>");
    let mut dom = dom.borrow_mut();
    let a = dom.get_element_by_id(d, "a").unwrap().unwrap();
    let s = dom.get_element_by_id(d, "s").unwrap().unwrap();

    // A detached element styled and filled: outside the document only.
    let v = dom.version();
    let clone = dom.create_element(d, "div").unwrap();
    dom.set_attribute(clone, "style", "color: red").unwrap();
    let child = dom.create_element(d, "b").unwrap();
    dom.append_child(clone, child).unwrap();
    assert!(dom.only_detached_changes_since(v));

    // Inserting it into the document: not.
    let v = dom.version();
    dom.append_child(a, clone).unwrap();
    assert!(!dom.only_detached_changes_since(v));

    // A change to a connected element: not.
    let v = dom.version();
    dom.set_attribute(s, "class", "k").unwrap();
    assert!(!dom.only_detached_changes_since(v));

    // Removing a connected element (then changing it detached): not, the removal shows.
    let v = dom.version();
    dom.remove_child(a, s).unwrap();
    dom.set_attribute(s, "class", "j").unwrap();
    assert!(!dom.only_detached_changes_since(v));

    // Changed while detached, then inserted: not, the insertion shows.
    let v = dom.version();
    dom.set_attribute(s, "class", "i").unwrap();
    dom.append_child(a, s).unwrap();
    assert!(!dom.only_detached_changes_since(v));

    // No change at all.
    let v = dom.version();
    assert!(dom.only_detached_changes_since(v));
}
