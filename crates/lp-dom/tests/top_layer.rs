//! The top layer kept at mutation time (its order, the popover stacks, the
//! inertness a modal dialog causes) and the `<details>` steps lp-dom runs on
//! attribute changes (queued toggles, the exclusive accordion).

use std::cell::RefCell;
use std::rc::Rc;

use lp_dom::parse::{parse_document, DomRef};
use lp_dom::*;

fn doc(html: &str) -> (DomRef, NodeId) {
    let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
    let d = parse_document(&dom, "http://test/", html);
    dom.borrow_mut().set_document_active(d, true).unwrap();
    (dom, d)
}

fn by_id(dom: &Dom, d: NodeId, id: &str) -> NodeId {
    dom.get_element_by_id(d, id).unwrap().unwrap()
}

#[test]
fn the_topmost_modal_dialog_makes_the_rest_inert() {
    let (dom, d) = doc("<button id=out></button><dialog id=a open><button id=ina></button></dialog><dialog id=b open><button id=inb></button></dialog><div id=p popover></div>");
    let mut dom = dom.borrow_mut();
    let (out, a, ina, b, inb, p) = (
        by_id(&dom, d, "out"),
        by_id(&dom, d, "a"),
        by_id(&dom, d, "ina"),
        by_id(&dom, d, "b"),
        by_id(&dom, d, "inb"),
        by_id(&dom, d, "p"),
    );
    assert_eq!(dom.topmost_modal(), None);
    assert!(!dom.is_inert(out));
    dom.set_modal(a, true).unwrap();
    dom.set_popover_showing(p, true).unwrap();
    dom.set_modal(b, true).unwrap();
    assert_eq!(dom.top_layer(), [a, p, b]);
    assert_eq!(dom.topmost_modal(), Some(b));
    assert!(dom.is_inert(out) && dom.is_inert(ina) && !dom.is_inert(inb) && !dom.is_inert(b));
    // A modal dialog whose `open` goes is not modal any more.
    dom.remove_attribute(b, "open").unwrap();
    assert_eq!(dom.top_layer(), [a, p]);
    assert!(dom.is_inert(out) && !dom.is_inert(ina));
    dom.set_popover_showing(p, false).unwrap();
    dom.set_modal(a, false).unwrap();
    assert!(dom.top_layer().is_empty() && !dom.is_inert(out));
}

#[test]
fn popover_stacks_follow_visibility_and_removal() {
    let (dom, d) = doc("<div id=a popover></div><div id=b popover></div><div id=h popover=hint></div>");
    let mut dom = dom.borrow_mut();
    let (a, b, h) = (by_id(&dom, d, "a"), by_id(&dom, d, "b"), by_id(&dom, d, "h"));
    for (el, stack) in [(a, PopoverStack::Auto), (b, PopoverStack::Auto), (h, PopoverStack::Hint)] {
        dom.set_popover_showing(el, true).unwrap();
        dom.push_popover_stack(el, stack);
    }
    dom.set_popover_invoker(b, Some(a));
    assert_eq!(dom.popover_stack(PopoverStack::Auto), [a, b]);
    assert_eq!(dom.popover_stack(PopoverStack::Hint), [h]);
    assert_eq!(dom.popover_invoker(b), Some(a));
    dom.set_popover_showing(b, false).unwrap();
    assert_eq!(dom.popover_stack(PopoverStack::Auto), [a]);
    assert_eq!(dom.popover_invoker(b), None);
    let parent = dom.parent(h).unwrap().unwrap();
    dom.remove_child(parent, h).unwrap();
    assert!(dom.popover_stack(PopoverStack::Hint).is_empty());
    assert_eq!(dom.top_layer(), [a]);
}

#[test]
fn details_open_changes_are_queued_and_close_the_name_group() {
    let (dom, d) = doc("<details id=x name=g open></details><details id=y name=g></details><details id=z></details>");
    let mut dom = dom.borrow_mut();
    let (x, y, z) = (by_id(&dom, d, "x"), by_id(&dom, d, "y"), by_id(&dom, d, "z"));
    let take = |dom: &mut Dom| {
        let t = dom.take_details_toggles();
        for (n, _) in &t {
            dom.release(*n);
        }
        t
    };
    // The parser's `open`.
    assert_eq!(take(&mut dom), [(x, false)]);
    dom.set_attribute(y, "open", "").unwrap();
    assert!(!dom.has_attribute(x, "open").unwrap());
    assert_eq!(take(&mut dom), [(y, false), (x, true)]);
    // A value change is no state change; another element is no details.
    dom.set_attribute(y, "open", "open").unwrap();
    dom.set_attribute(z, "title", "t").unwrap();
    assert!(!dom.has_details_toggles());
    // Joining the group while open closes the newcomer.
    dom.set_attribute(z, "open", "").unwrap();
    dom.set_attribute(z, "name", "g").unwrap();
    assert!(!dom.has_attribute(z, "open").unwrap());
    assert_eq!(take(&mut dom), [(z, false), (z, true)]);
}
