//! `Dom::set_touch_watch`: the nodes mutations made under an attribution touched (Soft Navigations).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use pyro_dom::parse::{parse_document, DomRef};
use pyro_dom::select::parse_query;
use pyro_dom::*;

thread_local! {
    static ATTRIBUTION: Cell<u64> = const { Cell::new(0) };
}

fn attribution() -> u64 {
    ATTRIBUTION.with(Cell::get)
}

fn doc() -> (DomRef, NodeId) {
    let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
    let d = parse_document(&dom, "http://test/", "<!doctype html><body><p id=p>text</p></body>");
    (dom, d)
}

#[test]
fn nothing_is_recorded_without_a_watch_or_an_attribution() {
    let (dom, d) = doc();
    let mut dom = dom.borrow_mut();
    let p = dom.query_first(d, &parse_query("#p").unwrap()).unwrap().unwrap();
    ATTRIBUTION.with(|a| a.set(7));
    dom.set_attribute(p, "class", "a").unwrap();
    assert!(dom.take_touched().is_empty(), "no watch: nothing is recorded");
    dom.set_touch_watch(Some(attribution));
    ATTRIBUTION.with(|a| a.set(0));
    dom.set_attribute(p, "class", "b").unwrap();
    assert!(dom.take_touched().is_empty(), "no attribution: nothing is recorded");
}

#[test]
fn attributes_character_data_and_added_children_are_touched() {
    let (dom, d) = doc();
    let mut dom = dom.borrow_mut();
    let p = dom.query_first(d, &parse_query("#p").unwrap()).unwrap().unwrap();
    let text = dom.first_child(p).unwrap().unwrap();
    dom.set_touch_watch(Some(attribution));
    ATTRIBUTION.with(|a| a.set(7));
    dom.set_attribute(p, "class", "a").unwrap();
    dom.set_char_data(text, "changed").unwrap();
    let div = dom.create_element(d, "div").unwrap();
    dom.append_child(p, div).unwrap();
    ATTRIBUTION.with(|a| a.set(8));
    dom.remove_child(p, div).unwrap();
    assert_eq!(dom.take_touched(), vec![(7, p), (7, text), (7, div)], "a removal touches nothing");
    assert!(dom.take_touched().is_empty());
    dom.set_touch_watch(None);
    assert!(!dom.touch_watch_on());
}
