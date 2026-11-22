//! Custom element reactions the tree queues besides connection: attribute
//! changes of observing elements only, adoption, failed elements, and the
//! form owner, disabled state and custom states of form-associated custom
//! elements.

use std::cell::RefCell;
use std::rc::Rc;

use pyro_dom::custom::{AttributeChange, CustomReaction};
use pyro_dom::parse::{parse_document, DomRef};
use pyro_dom::select::parse_selectors;
use pyro_dom::*;

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

#[test]
fn defined_selector_respects_the_namespace_and_custom_element_state() {
    let mut dom = Dom::new();
    let inert = dom.create_document(DocumentKind::Html, "about:blank");
    let svg = dom.create_element_ns(inert, Some("http://www.w3.org/2000/svg"), "a-a").unwrap();
    let reserved = dom.create_element(inert, "font-face").unwrap();
    let candidate = dom.create_element(inert, "a-a").unwrap();
    let builtin = dom.create_element(inert, "button").unwrap();
    dom.set_is_value(builtin, Some(LocalName::from("a-a"))).unwrap();
    let defined = parse_selectors(":defined").unwrap();
    for (element, expected) in [(svg, true), (reserved, true), (candidate, false), (builtin, false)] {
        assert_eq!(dom.matches(element, &defined).unwrap(), expected);
    }
    dom.set_custom_defined(candidate).unwrap();
    assert!(dom.matches(candidate, &defined).unwrap());
}

#[test]
fn detached_fragment_creation_queues_only_its_registry_definition() {
    use pyro_dom::upgrade::UpgradeCandidate;
    let (dom, doc) = doc("<body></body>");
    let mut dom = dom.borrow_mut();
    dom.enable_custom_upgrades();
    let name = LocalName::from("x-badge");
    dom.note_registry_definition(doc, RegistryAssociation::Scoped(7), &name, &name, vec![]);
    let host = dom.create_element(doc, "div").unwrap();
    let root = dom.attach_shadow(host, ShadowRootMode::Open).unwrap();
    for (registry, expected) in [(RegistryAssociation::Scoped(7), true), (RegistryAssociation::Default, false), (RegistryAssociation::Null, false)] {
        dom.set_registry_association(root, registry).unwrap();
        let frag = pyro_dom::parse::parse_setter_fragment_in(&mut dom, host, root, "<x-badge></x-badge>").unwrap();
        let child = dom.first_child(frag).unwrap().unwrap();
        assert_eq!(dom.registry_association(child).unwrap(), registry);
        let queued = dom.take_upgrade_reactions();
        if expected { assert_eq!(queued, [UpgradeCandidate::Created(child)]); } else { assert!(queued.is_empty()); }
    }
}
