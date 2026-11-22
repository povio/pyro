//! Node adoption across documents (creation document, template contents) and
//! the NodeIterator pre-remove steps.

use std::cell::RefCell;
use std::rc::Rc;

use pyro_dom::parse::{parse_document, set_inner_html, DomRef};
use pyro_dom::*;

fn doc(html: &str) -> (DomRef, NodeId) {
    let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
    let d = parse_document(&dom, "http://test/", html);
    dom.borrow_mut().set_document_active(d, true).unwrap();
    (dom, d)
}

#[test]
fn a_node_keeps_the_document_it_was_created_in_across_adoptions() {
    let (dom, top) = doc("<body></body>");
    let frame = parse_document(&dom, "http://test/frame", "<body></body>");
    let other = parse_document(&dom, "http://test/other", "<body></body>");
    let mut dom = dom.borrow_mut();
    dom.set_document_active(frame, true).unwrap();
    dom.set_document_active(other, true).unwrap();
    let container = dom.create_element(frame, "div").unwrap();
    set_inner_html(&mut dom, container, "<p><b>x</b></p>").unwrap();
    let p = dom.first_child(container).unwrap().unwrap();
    let b = dom.first_child(p).unwrap().unwrap();
    assert_eq!(dom.creation_document(container).unwrap(), frame);
    // A document is its own, and a node that never moved has its node document.
    assert_eq!(dom.creation_document(top).unwrap(), top);
    let top_body = dom.body(top).unwrap().unwrap();
    assert_eq!(dom.creation_document(top_body).unwrap(), top);

    dom.append_child(top_body, container).unwrap();
    for n in [container, p, b] {
        assert_eq!(dom.owner_document(n).unwrap(), Some(top));
        assert_eq!(dom.creation_document(n).unwrap(), frame);
    }
    // A node parsed into the adopted container is created in the new document.
    set_inner_html(&mut dom, container, "<i></i>").unwrap();
    let i = dom.first_child(container).unwrap().unwrap();
    assert_eq!(dom.creation_document(i).unwrap(), top);
    // A second adoption keeps the first record.
    let other_body = dom.body(other).unwrap().unwrap();
    dom.append_child(other_body, container).unwrap();
    assert_eq!(dom.owner_document(i).unwrap(), Some(other));
    assert_eq!(dom.creation_document(container).unwrap(), frame);
    assert_eq!(dom.creation_document(i).unwrap(), top);
}

#[test]
fn a_document_without_a_window_gives_its_nodes_the_realm_of_its_creator() {
    let (dom, top) = doc("<body><template><i></i></template></body>");
    let frame = parse_document(&dom, "http://test/frame", "<body></body>");
    let mut dom = dom.borrow_mut();
    dom.set_document_active(frame, true).unwrap();
    // No creator known: the node counts as created where it ends up.
    let unknown = dom.create_document(DocumentKind::Html, "about:blank");
    let div = dom.create_element(unknown, "div").unwrap();
    assert_eq!(dom.creation_document(unknown).unwrap(), unknown);
    assert_eq!(dom.creation_document(div).unwrap(), unknown);
    dom.adopt_node(top, div).unwrap();
    assert_eq!(dom.creation_document(div).unwrap(), top);

    // Created by the frame's script: its nodes are the frame's, wherever they go.
    let frameless = dom.create_document(DocumentKind::Html, "about:blank");
    dom.set_document_creator(frameless, frame).unwrap();
    dom.set_document_creator(frameless, top).unwrap();
    assert_eq!(dom.creation_document(frameless).unwrap(), frame);
    let container = dom.create_element(frameless, "div").unwrap();
    set_inner_html(&mut dom, container, "<p></p>").unwrap();
    let p = dom.first_child(container).unwrap().unwrap();
    assert_eq!(dom.creation_document(p).unwrap(), frame);
    let top_body = dom.body(top).unwrap().unwrap();
    dom.append_child(top_body, container).unwrap();
    assert_eq!(dom.owner_document(p).unwrap(), Some(top));
    assert_eq!(dom.creation_document(p).unwrap(), frame);
    assert_eq!(dom.creation_document(container).unwrap(), frame);
    // One created by a document without a window inherits that document's realm.
    let nested = dom.create_document(DocumentKind::Html, "about:blank");
    dom.set_document_creator(nested, frameless).unwrap();
    assert_eq!(dom.creation_document(nested).unwrap(), frame);

    // Template contents belong to the window of the template's document.
    let template = dom.first_child(top_body).unwrap().unwrap();
    let contents = dom.template_contents(template).unwrap().unwrap();
    let i = dom.first_child(contents).unwrap().unwrap();
    assert_ne!(dom.owner_document(i).unwrap(), Some(top));
    assert_eq!(dom.creation_document(i).unwrap(), top);
    let frame_body = dom.body(frame).unwrap().unwrap();
    dom.append_child(frame_body, i).unwrap();
    assert_eq!(dom.creation_document(i).unwrap(), top);
}

#[test]
fn adopting_a_template_moves_its_contents_to_the_new_template_contents_owner() {
    let (dom, top) = doc("<body></body>");
    let mut dom = dom.borrow_mut();
    let source = dom.create_document(DocumentKind::Html, "about:blank");
    let target = dom.create_document(DocumentKind::Html, "about:blank");
    let outer = dom.create_element(source, "template").unwrap();
    let inner = dom.create_element(source, "template").unwrap();
    let span = dom.create_element(source, "span").unwrap();
    let outer_content = dom.template_contents(outer).unwrap().unwrap();
    let inner_content = dom.template_contents(inner).unwrap().unwrap();
    dom.append_child(inner_content, span).unwrap();
    dom.append_child(outer_content, inner).unwrap();
    let before = dom.owner_document(outer_content).unwrap().unwrap();
    assert_ne!(before, source);
    assert_eq!(dom.owner_document(inner).unwrap(), Some(before));

    dom.adopt_node(target, outer).unwrap();
    assert_eq!(dom.owner_document(outer).unwrap(), Some(target));
    let after = dom.owner_document(outer_content).unwrap().unwrap();
    assert_ne!(after, before);
    assert_ne!(after, target);
    for n in [inner, inner_content, span] {
        assert_eq!(dom.owner_document(n).unwrap(), Some(after), "nested template nodes follow the contents");
    }
    // Adopting the contents themselves puts them in the document directly.
    dom.adopt_node(top, outer_content).unwrap();
    assert_eq!(dom.owner_document(outer_content).unwrap(), Some(top));
    assert_eq!(dom.owner_document(outer).unwrap(), Some(target));
}

/// root[a[a1], b[b1], c]
fn scratch(dom: &mut Dom, d: NodeId, with_c: bool) -> [NodeId; 6] {
    let mk = |dom: &mut Dom, name: &str| dom.create_element(d, name).unwrap();
    let (root, a, a1, b, b1, c) =
        (mk(dom, "root"), mk(dom, "a"), mk(dom, "a1"), mk(dom, "b"), mk(dom, "b1"), mk(dom, "c"));
    dom.append_child(a, a1).unwrap();
    dom.append_child(b, b1).unwrap();
    dom.append_child(root, a).unwrap();
    dom.append_child(root, b).unwrap();
    if with_c {
        dom.append_child(root, c).unwrap();
    }
    [root, a, a1, b, b1, c]
}

#[test]
fn removing_an_ancestor_of_the_root_leaves_the_iterator_alone() {
    let (dom, d) = doc("<body></body>");
    let mut dom = dom.borrow_mut();
    let [root, a, a1, ..] = scratch(&mut dom, d, true);
    let it = dom.new_live_iterator(a1).unwrap();
    dom.remove_child(root, a).unwrap();
    assert_eq!((it.reference(), it.before()), (a1, true));
    let it = dom.new_live_iterator(a).unwrap();
    dom.set_iterator_reference(&it, a1, false).unwrap();
    dom.remove_child(a, a1).unwrap();
    assert_eq!((it.reference(), it.before()), (a, false));
}

#[test]
fn the_reference_and_the_position_in_flight_move_when_their_subtree_is_removed() {
    let (dom, d) = doc("<body></body>");
    let mut dom = dom.borrow_mut();
    let [root, a, a1, b, b1, c] = scratch(&mut dom, d, true);
    let it = dom.new_live_iterator(root).unwrap();
    // Pointer before the reference, a following node inside the root: go to it.
    dom.set_iterator_reference(&it, b1, true).unwrap();
    // The traversal in flight is after `a`, filtering `b`.
    dom.set_iterator_candidate(&it, Some((b, false))).unwrap();
    dom.remove_child(root, b).unwrap();
    assert_eq!((it.reference(), it.before()), (c, true));
    assert_eq!(it.candidate(), Some((a1, false)));
    dom.set_iterator_candidate(&it, None).unwrap();
    assert_eq!(it.candidate(), None);

    // No following node inside the root: the last node before, pointer after it.
    dom.remove_child(root, c).unwrap();
    assert_eq!((it.reference(), it.before()), (a1, false));
    dom.set_iterator_candidate(&it, Some((a1, true))).unwrap();
    dom.remove_child(root, a).unwrap();
    assert_eq!((it.reference(), it.before()), (root, false));
    assert_eq!(it.candidate(), Some((root, false)));
    dom.set_iterator_candidate(&it, None).unwrap();
}

#[test]
fn scoped_registry_clone_adopt_and_connection_order() {
    use RegistryAssociation::{Default as Global, Null, Scoped};

    let (dom, first) = doc("<main></main>");
    let second = parse_document(&dom, "http://test/second", "<main></main>");
    let mut d = dom.borrow_mut();
    d.set_document_active(second, true).unwrap();
    let root = d.create_element(first, "section").unwrap();
    let child = d.create_element(first, "span").unwrap();
    d.set_registry_association(root, Scoped(11)).unwrap();
    d.set_registry_association(child, Null).unwrap();
    d.append_child(root, child).unwrap();
    let imported = d.clone_node_with_registry(root, true, Some(second), Scoped(12)).unwrap();
    let leaf = d.first_child(imported).unwrap().unwrap();
    assert_eq!(d.registry_association(imported).unwrap(), Scoped(11));
    assert_eq!(d.registry_association(leaf).unwrap(), Scoped(12));
    assert!(d.scoped_registry_documents(11).is_empty());
    let second_host = d.document_element(second).unwrap().unwrap();
    d.append_child(second_host, imported).unwrap();
    let first_host = d.document_element(first).unwrap().unwrap();
    d.append_child(first_host, root).unwrap();
    let docs: Vec<_> = d.scoped_registry_documents(11).into_iter().map(|(_, doc)| doc).collect();
    assert_eq!(docs, [second, first]);
    d.adopt_node(second, root).unwrap();
    assert_eq!(d.registry_association(root).unwrap(), Scoped(11));
    assert_eq!(d.registry_association(child).unwrap(), Null);
    d.adopt_node(first, child).unwrap();
    assert_eq!(d.registry_association(child).unwrap(), Global);
}

#[test]
fn scoped_registry_fragment_and_declarative_shadow() {
    use RegistryAssociation::{Default as Global, Null, Scoped};

    let (dom, doc) = doc("<body></body>");
    let mut d = dom.borrow_mut();
    let host = d.create_element(doc, "div").unwrap();
    d.set_registry_association(host, Scoped(7)).unwrap();
    pyro_dom::parse::set_html_unsafe(&mut d, host,
        "<section><template shadowrootmode=open shadowrootclonable shadowrootcustomelementregistry><span></span></template></section>").unwrap();
    let section = d.first_child(host).unwrap().unwrap();
    let sr = d.shadow_root(section).unwrap().unwrap();
    assert_eq!(d.registry_association(section).unwrap(), Scoped(7));
    assert_eq!(d.registry_association(sr).unwrap(), Null);
    assert_eq!(d.registry_association(d.first_child(sr).unwrap().unwrap()).unwrap(), Null);
    let copy = d.clone_node(section, true, None).unwrap();
    let copy_sr = d.shadow_root(copy).unwrap().unwrap();
    let other = d.create_document(DocumentKind::Html, "about:blank");
    d.set_document_active(other, true).unwrap();
    d.adopt_node(other, copy).unwrap();
    assert_eq!(d.registry_association(copy_sr).unwrap(), Null);

    pyro_dom::parse::set_html_unsafe(&mut d, host,
        "<section><template shadowrootmode=open><span></span></template></section>").unwrap();
    let sr = d.shadow_root(d.first_child(host).unwrap().unwrap()).unwrap().unwrap();
    assert_eq!(d.registry_association(sr).unwrap(), Global);
    assert_eq!(d.registry_association(d.first_child(sr).unwrap().unwrap()).unwrap(), Global);
}

#[test]
fn shadow_disabled_definition_belongs_to_its_registry_and_local_name() {
    let (dom, first) = doc("");
    let second = parse_document(&dom, "http://test/second", "");
    let mut d = dom.borrow_mut();
    d.set_document_active(second, true).unwrap();
    let name = LocalName::from("x-blocked");
    let local = LocalName::from("div");
    d.disable_shadow_for_registry(first, RegistryAssociation::Default, &name, &local);
    for (doc, local_name, blocked) in [(first, "div", true), (second, "div", false), (first, "section", false)] {
        let el = d.create_element(doc, local_name).unwrap();
        d.set_is_value(el, Some(name.clone())).unwrap();
        assert_eq!(d.attach_shadow(el, ShadowRootMode::Open).is_err(), blocked);
    }
}
