//! Custom element reactions the tree queues besides connection: attribute
//! changes of observing elements only, adoption, failed elements, and the
//! form owner, disabled state and custom states of form-associated custom
//! elements.

use std::cell::RefCell;
use std::rc::Rc;

use lp_dom::custom::{AttributeChange, CustomReaction};
use lp_dom::parse::{parse_document, DomRef};
use lp_dom::select::parse_selectors;
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

fn changed(local: &str, old: Option<&str>, new: Option<&str>) -> CustomReaction {
    CustomReaction::AttributeChanged(Box::new(AttributeChange {
        local: LocalName::from(local),
        ns: Namespace::from(""),
        old: old.map(str::to_owned),
        new: new.map(str::to_owned),
    }))
}

#[test]
fn only_observing_elements_queue_attribute_reactions_for_observed_names() {
    let (dom, d) = doc("<x-a id=a></x-a><x-a id=other></x-a><x-b id=b></x-b>");
    let mut dom = dom.borrow_mut();
    let (a, other, b) = (by_id(&dom, d, "a"), by_id(&dom, d, "other"), by_id(&dom, d, "b"));
    dom.note_custom_definition(&LocalName::from("x-a"), vec![LocalName::from("title")]);
    dom.prepare_custom_element(a, true, false).unwrap();
    dom.set_custom_defined(a).unwrap();
    // Custom, but its definition observes nothing.
    dom.set_custom_defined(b).unwrap();
    dom.set_attribute(a, "title", "1").unwrap();
    dom.set_attribute(a, "title", "1").unwrap();
    dom.set_attribute(a, "lang", "en").unwrap();
    dom.set_attribute(b, "title", "x").unwrap();
    // Same name, not upgraded: nothing.
    dom.set_attribute(other, "title", "x").unwrap();
    dom.remove_attribute(a, "title").unwrap();
    assert_eq!(
        dom.take_custom_reactions(d),
        vec![
            (a, changed("title", None, Some("1"))),
            (a, changed("title", Some("1"), Some("1"))),
            (a, changed("title", Some("1"), None)),
        ]
    );
}

#[test]
fn adoption_queues_adopted_with_both_documents() {
    let (dom, d) = doc("<div id=host></div>");
    let mut dom = dom.borrow_mut();
    let other = dom.create_document(DocumentKind::Html, "about:blank");
    let el = dom.create_element(d, "x-a").unwrap();
    dom.set_custom_defined(el).unwrap();
    dom.adopt_node(other, el).unwrap();
    assert_eq!(dom.take_custom_reactions(d), vec![(el, CustomReaction::Adopted { old: d, new: other })]);
}

#[test]
fn a_failed_element_drops_its_reactions_and_is_no_upgrade_candidate() {
    let (dom, d) = doc("<div id=host></div>");
    let mut dom = dom.borrow_mut();
    let host = by_id(&dom, d, "host");
    dom.enable_custom_upgrades();
    let el = dom.create_element(d, "x-a").unwrap();
    dom.prepare_custom_element(el, true, false).unwrap();
    dom.note_custom_definition(&LocalName::from("x-a"), vec![LocalName::from("title")]);
    dom.set_attribute(el, "title", "t").unwrap();
    assert!(dom.has_custom_reactions());
    dom.set_custom_failed(el).unwrap();
    assert!(!dom.has_custom_reactions());
    dom.append_child(host, el).unwrap();
    assert!(!dom.has_upgrade_candidates());
}

#[test]
fn form_associated_elements_track_owner_and_disabled_state() {
    let (dom, d) = doc("<form id=f></form><fieldset id=fs disabled></fieldset>");
    let mut dom = dom.borrow_mut();
    let (f, fs) = (by_id(&dom, d, "f"), by_id(&dom, d, "fs"));
    let el = dom.create_element(d, "x-field").unwrap();
    dom.prepare_custom_element(el, false, true).unwrap();
    dom.set_custom_defined(el).unwrap();
    dom.set_form_associated(el).unwrap();
    assert!(!dom.has_custom_reactions());
    dom.append_child(f, el).unwrap();
    assert_eq!(dom.face_form(el), Some(f));
    dom.append_child(fs, el).unwrap();
    assert!(dom.is_disabled(el).unwrap());
    // Removing steps (form owner) come before disconnectedCallback, and
    // insertion steps before connectedCallback.
    assert_eq!(
        dom.take_custom_reactions(d),
        vec![
            (el, CustomReaction::FormAssociated(Some(f))),
            (el, CustomReaction::Connected),
            (el, CustomReaction::FormAssociated(None)),
            (el, CustomReaction::Disconnected),
            (el, CustomReaction::FormDisabled(true)),
            (el, CustomReaction::Connected),
        ]
    );
    // Custom states restyle the element and match :state().
    assert!(dom.set_custom_state(el, "open", true).unwrap());
    assert!(!dom.set_custom_state(el, "open", true).unwrap());
    assert_eq!(dom.query_selector(d, &parse_selectors("x-field:state(open)").unwrap()).unwrap(), Some(el));
    assert_eq!(dom.query_selector(d, &parse_selectors(":state(closed)").unwrap()).unwrap(), None);
    dom.clear_custom_states(el).unwrap();
    assert_eq!(dom.query_selector(d, &parse_selectors(":state(open)").unwrap()).unwrap(), None);
}
