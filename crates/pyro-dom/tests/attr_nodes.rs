//! Attr nodes (`Dom::attribute_node`), attribute lookups by namespace and
//! the node comparisons that involve attributes.

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

fn name(local: &str) -> QualName {
    QualName::new(None, Namespace::from(""), LocalName::from(local))
}

#[test]
fn an_attribute_has_one_node_until_it_leaves_its_element() {
    let (dom, d) = doc("<!doctype html><div id=a x=abc></div>");
    let mut dom = dom.borrow_mut();
    let a = dom.get_element_by_id(d, "a").unwrap().unwrap();
    let x = dom.attribute_node(a, None, "x").unwrap().unwrap();
    assert_eq!(dom.attribute_node(a, None, "x").unwrap(), Some(x), "same node on every lookup");
    assert_eq!(dom.attribute_node(a, None, "nope").unwrap(), None);
    assert_eq!(dom.node_type(x).unwrap(), NodeType::Attribute);
    assert_eq!(dom.node_name(x).unwrap(), "x");
    assert_eq!(dom.attr_node_value(x).unwrap(), "abc");
    assert_eq!(dom.text_content(x).unwrap().as_deref(), Some("abc"));
    assert_eq!(dom.attr_owner_element(x).unwrap(), Some(a));
    // No parent, no children, its own root, never connected.
    assert_eq!(dom.parent(x).unwrap(), None);
    assert_eq!(dom.child_count(x).unwrap(), 0);
    assert_eq!(dom.root_node(x, false).unwrap(), x);
    assert!(!dom.is_connected(x).unwrap());
    assert_eq!(dom.owner_document(x).unwrap(), Some(d));
    // The value is the element's attribute value, both ways.
    dom.set_attribute(a, "x", "new").unwrap();
    assert_eq!(dom.attr_node_value(x).unwrap(), "new");
    dom.set_text_content(x, "via node").unwrap();
    assert_eq!(dom.get_attribute(a, "x").unwrap(), Some("via node"));
    // Removed: the node keeps the last value and loses its element; setting
    // the attribute again makes a different node.
    dom.add_ref(x).unwrap();
    dom.remove_attribute(a, "x").unwrap();
    assert_eq!(dom.attr_owner_element(x).unwrap(), None);
    assert_eq!(dom.attr_node_value(x).unwrap(), "via node");
    dom.set_attribute(a, "x", "again").unwrap();
    let again = dom.attribute_node(a, None, "x").unwrap().unwrap();
    assert_ne!(again, x);
    assert_eq!(dom.attr_node_value(x).unwrap(), "via node");
    dom.set_attr_node_value(x, "detached").unwrap();
    assert_eq!(dom.get_attribute(a, "x").unwrap(), Some("again"));
    dom.release(x);
}

#[test]
fn set_attribute_node_appends_replaces_and_refuses_one_in_use() {
    let (dom, d) = doc("<!doctype html><div id=a x=first></div><div id=b></div>");
    let mut dom = dom.borrow_mut();
    let a = dom.get_element_by_id(d, "a").unwrap().unwrap();
    let b = dom.get_element_by_id(d, "b").unwrap().unwrap();
    let old = dom.attribute_node(a, None, "x").unwrap().unwrap();
    dom.add_ref(old).unwrap();
    // Its own attribute: returned as is.
    assert_eq!(dom.set_attribute_node(a, old).unwrap(), Some(old));
    // Another element's: InUseAttributeError.
    let err = dom.set_attribute_node(b, old).unwrap_err();
    assert_eq!(err.kind(), Some(DomErrorKind::InUseAttribute));
    // A new one of the same name replaces it in place and returns it.
    let fresh = dom.create_attribute(d, name("x")).unwrap();
    dom.add_ref(fresh).unwrap();
    dom.set_attr_node_value(fresh, "second").unwrap();
    assert_eq!(dom.attr_owner_element(fresh).unwrap(), None);
    assert_eq!(dom.set_attribute_node(a, fresh).unwrap(), Some(old));
    assert_eq!(dom.get_attribute(a, "x").unwrap(), Some("second"));
    assert_eq!(dom.attributes(a).unwrap().len(), 2, "id and x");
    assert_eq!(dom.attr_owner_element(fresh).unwrap(), Some(a));
    assert_eq!(dom.attr_owner_element(old).unwrap(), None);
    assert_eq!(dom.attr_node_value(old).unwrap(), "first");
    assert_eq!(dom.attribute_node(a, None, "x").unwrap(), Some(fresh));
    // The freed one can go to the other element.
    assert_eq!(dom.set_attribute_node(b, old).unwrap(), None);
    assert_eq!(dom.get_attribute(b, "x").unwrap(), Some("first"));
    // removeAttributeNode only takes the element's own.
    assert_eq!(dom.remove_attribute_node(a, old).unwrap_err().kind(), Some(DomErrorKind::NotFound));
    dom.remove_attribute_node(b, old).unwrap();
    assert!(!dom.has_attribute(b, "x").unwrap());
    dom.release(old);
    dom.release(fresh);
}

#[test]
fn an_attr_node_follows_its_element_between_documents() {
    let (dom, d) = doc("<!doctype html><div id=a x=1></div>");
    let mut dom = dom.borrow_mut();
    let other = dom.create_document(DocumentKind::Xml, "about:blank");
    dom.add_ref(other).unwrap();
    let a = dom.get_element_by_id(d, "a").unwrap().unwrap();
    let x = dom.attribute_node(a, None, "x").unwrap().unwrap();
    dom.add_ref(x).unwrap();
    assert_eq!(dom.owner_document(x).unwrap(), Some(d));
    // Adopting the element moves its attributes' node document.
    dom.adopt_node(other, a).unwrap();
    assert_eq!(dom.owner_document(x).unwrap(), Some(other));
    // A detached attribute takes the document of the element it is set on.
    let el = dom.create_element(d, "p").unwrap();
    dom.add_ref(el).unwrap();
    dom.remove_attribute(a, "x").unwrap();
    assert_eq!(dom.owner_document(x).unwrap(), Some(other));
    dom.set_attribute_node(el, x).unwrap();
    assert_eq!(dom.owner_document(x).unwrap(), Some(d));
    // adoptNode on an attribute takes it off its element.
    dom.adopt_node(other, x).unwrap();
    assert_eq!(dom.attr_owner_element(x).unwrap(), None);
    assert!(!dom.has_attribute(el, "x").unwrap());
    assert_eq!(dom.owner_document(x).unwrap(), Some(other));
    // Clones are detached copies, in the requested document.
    let copy = dom.clone_node(x, true, Some(d)).unwrap();
    assert_eq!(dom.attr_owner_element(copy).unwrap(), None);
    assert_eq!(dom.attr_node_value(copy).unwrap(), "1");
    assert_eq!(dom.owner_document(copy).unwrap(), Some(d));
    assert!(dom.is_equal_node(copy, x).unwrap());
    dom.release(x);
    dom.release(el);
    dom.release(other);
}

#[test]
fn attr_nodes_are_collected_with_their_element_and_keep_it_alive() {
    let (dom, d) = doc("<!doctype html><body></body>");
    let mut dom = dom.borrow_mut();
    dom.collect_garbage();
    let base = dom.live_nodes();
    // Unreferenced node of an attribute of a live element: dropped, remade on demand.
    let body = dom.body(d).unwrap().unwrap();
    dom.set_attribute(body, "x", "1").unwrap();
    let x = dom.attribute_node(body, None, "x").unwrap().unwrap();
    assert_eq!(dom.live_nodes(), base + 1);
    dom.collect_garbage();
    assert_eq!(dom.live_nodes(), base);
    assert!(!dom.is_alive(x));
    assert_eq!(dom.get_attribute(body, "x").unwrap(), Some("1"));
    // A referenced one keeps its detached element.
    let el = dom.create_element(d, "p").unwrap();
    dom.set_attribute(el, "y", "2").unwrap();
    let y = dom.attribute_node(el, None, "y").unwrap().unwrap();
    dom.add_ref(y).unwrap();
    dom.collect_garbage();
    assert!(dom.is_alive(el) && dom.is_alive(y));
    assert_eq!(dom.attr_owner_element(y).unwrap(), Some(el));
    // Released: element and node go together.
    dom.release(y);
    dom.collect_garbage();
    assert!(!dom.is_alive(el) && !dom.is_alive(y));
    assert_eq!(dom.live_nodes(), base);
    // A detached attribute is collected once released.
    let z = dom.create_attribute(d, name("z")).unwrap();
    dom.add_ref(z).unwrap();
    dom.collect_garbage();
    assert!(dom.is_alive(z));
    dom.release(z);
    dom.collect_garbage();
    assert!(!dom.is_alive(z));
}

#[test]
fn attributes_are_not_children() {
    let (dom, d) = doc("<!doctype html><div id=a x=1 y=2><span id=s></span></div><p id=p></p>");
    let mut dom = dom.borrow_mut();
    let a = dom.get_element_by_id(d, "a").unwrap().unwrap();
    let s = dom.get_element_by_id(d, "s").unwrap().unwrap();
    let p = dom.get_element_by_id(d, "p").unwrap().unwrap();
    let x = dom.attribute_node(a, None, "x").unwrap().unwrap();
    let y = dom.attribute_node(a, None, "y").unwrap().unwrap();
    for (parent, node) in [(a, x), (x, s), (x, y)] {
        let err = dom.append_child(parent, node).unwrap_err();
        assert_eq!(err.kind(), Some(DomErrorKind::HierarchyRequest));
    }
    assert_eq!(dom.replace_child(a, x, s).unwrap_err().kind(), Some(DomErrorKind::HierarchyRequest));
    assert!(dom.contains(x, x).unwrap());
    assert!(!dom.contains(a, x).unwrap());
    use position::*;
    // Two attributes of one element: implementation-specific, in list order.
    assert_eq!(dom.compare_document_position(x, y).unwrap(), IMPLEMENTATION_SPECIFIC | FOLLOWING);
    assert_eq!(dom.compare_document_position(y, x).unwrap(), IMPLEMENTATION_SPECIFIC | PRECEDING);
    // An attribute is contained by its element and its ancestors...
    assert_eq!(dom.compare_document_position(a, x).unwrap(), CONTAINED_BY | FOLLOWING);
    assert_eq!(dom.compare_document_position(x, a).unwrap(), CONTAINS | PRECEDING);
    assert_eq!(dom.compare_document_position(d, x).unwrap(), CONTAINED_BY | FOLLOWING);
    // ...precedes the element's children and what follows it.
    assert_eq!(dom.compare_document_position(x, s).unwrap(), FOLLOWING);
    assert_eq!(dom.compare_document_position(s, x).unwrap(), PRECEDING);
    assert_eq!(dom.compare_document_position(x, p).unwrap(), FOLLOWING);
    // A detached attribute is disconnected from everything, consistently.
    let lone = dom.create_attribute(d, name("z")).unwrap();
    let there = dom.compare_document_position(lone, a).unwrap();
    let back = dom.compare_document_position(a, lone).unwrap();
    assert_eq!(there & (DISCONNECTED | IMPLEMENTATION_SPECIFIC), DISCONNECTED | IMPLEMENTATION_SPECIFIC);
    assert_eq!((there ^ back) & (PRECEDING | FOLLOWING), PRECEDING | FOLLOWING);
    // Namespace lookups go through the element.
    assert_eq!(dom.lookup_namespace_uri(x, None).unwrap(), Some("http://www.w3.org/1999/xhtml"));
    assert_eq!(dom.lookup_namespace_uri(lone, None).unwrap(), None);
}

#[test]
fn is_equal_node_compares_what_the_dom_standard_lists() {
    let (dom, d) = doc("<!doctype html><body></body>");
    let mut dom = dom.borrow_mut();
    // Doctypes: name, public ID, system ID.
    let dt = |dom: &mut Dom, n: &str, p: &str, s: &str| dom.create_doctype(d, n, p, s).unwrap();
    let base = dt(&mut dom, "a", "p", "s");
    assert!({ let o = dt(&mut dom, "a", "p", "s"); dom.is_equal_node(base, o).unwrap() });
    for (n, p, s) in [("b", "p", "s"), ("a", "q", "s"), ("a", "p", "t")] {
        assert!(!{ let o = dt(&mut dom, n, p, s); dom.is_equal_node(base, o).unwrap() }, "{n} {p} {s}");
    }
    // Elements: namespace, prefix, local name.
    let el = |dom: &mut Dom, ns: &str, q: &str| dom.create_element_ns(d, Some(ns), q).unwrap();
    let e = el(&mut dom, "urn:a", "p:x");
    assert!({ let o = el(&mut dom, "urn:a", "p:x"); dom.is_equal_node(e, o).unwrap() });
    assert!(!{ let o = el(&mut dom, "urn:b", "p:x"); dom.is_equal_node(e, o).unwrap() });
    assert!(!{ let o = el(&mut dom, "urn:a", "q:x"); dom.is_equal_node(e, o).unwrap() });
    assert!(!{ let o = el(&mut dom, "urn:a", "p:y"); dom.is_equal_node(e, o).unwrap() });
    // Attributes: namespace, local name and value; neither order nor prefix.
    let with = |dom: &mut Dom, attrs: &[(Option<&str>, &str, &str)]| {
        let e = dom.create_element(d, "i").unwrap();
        for (ns, q, v) in attrs {
            dom.set_attribute_ns(e, *ns, q, v).unwrap();
        }
        e
    };
    let one = with(&mut dom, &[(None, "a", "1"), (Some("urn:n"), "p:b", "2")]);
    assert!({ let o = with(&mut dom, &[(Some("urn:n"), "q:b", "2"), (None, "a", "1")]); dom.is_equal_node(one, o).unwrap() });
    assert!(!{ let o = with(&mut dom, &[(None, "a", "1"), (Some("urn:m"), "p:b", "2")]); dom.is_equal_node(one, o).unwrap() });
    assert!(!{ let o = with(&mut dom, &[(None, "a", "1"), (Some("urn:n"), "p:b", "3")]); dom.is_equal_node(one, o).unwrap() });
    assert!(!{ let o = with(&mut dom, &[(None, "a", "1")]); dom.is_equal_node(one, o).unwrap() });
    // Children, in order; text against CDATA or comment is a different interface.
    let tree = |dom: &mut Dom, texts: &[&str]| {
        let e = dom.create_element(d, "u").unwrap();
        for t in texts {
            let t = dom.create_text(d, t).unwrap();
            dom.append_child(e, t).unwrap();
        }
        e
    };
    let t = tree(&mut dom, &["a", "b"]);
    assert!({ let o = tree(&mut dom, &["a", "b"]); dom.is_equal_node(t, o).unwrap() });
    assert!(!{ let o = tree(&mut dom, &["b", "a"]); dom.is_equal_node(t, o).unwrap() });
    assert!(!{ let o = tree(&mut dom, &["a"]); dom.is_equal_node(t, o).unwrap() });
    let text = dom.create_text(d, "x").unwrap();
    let comment = dom.create_comment(d, "x").unwrap();
    assert!(!dom.is_equal_node(text, comment).unwrap());
}

#[test]
fn a_namespaced_attribute_is_not_the_elements_own() {
    let (dom, d) = doc("<!doctype html><iframe id=f></iframe><div id=a></div>");
    let mut dom = dom.borrow_mut();
    let a = dom.get_element_by_id(d, "a").unwrap().unwrap();
    // Same qualified name, another namespace, set first.
    dom.set_attribute_ns(a, Some("urn:n"), "style", "before").unwrap();
    dom.set_attribute_ns(a, None, "style", "color: green").unwrap();
    // Script's getAttribute sees the first by qualified name...
    assert_eq!(dom.get_attribute_qualified(a, "style").unwrap(), Some("before"));
    assert!(dom.has_attribute_qualified(a, "style").unwrap());
    // ...the element's own state reads the one in no namespace.
    assert_eq!(dom.get_attribute(a, "style").unwrap(), Some("color: green"));
    dom.remove_attribute_ns(a, None, "style").unwrap();
    assert_eq!(dom.get_attribute(a, "style").unwrap(), None);
    assert!(!dom.has_attribute(a, "style").unwrap());
    assert_eq!(dom.get_attribute_qualified(a, "style").unwrap(), Some("before"));
    // Names are still lowercased for HTML elements, and a prefixed name is a
    // qualified name.
    dom.set_attribute(a, "title", "t").unwrap();
    assert_eq!(dom.get_attribute(a, "TITLE").unwrap(), Some("t"));
    dom.set_attribute_ns(a, Some("http://www.w3.org/1999/xlink"), "xlink:href", "#x").unwrap();
    assert_eq!(dom.get_attribute(a, "xlink:href").unwrap(), Some("#x"));
    assert_eq!(dom.get_attribute(a, "href").unwrap(), None);
    // A namespaced `src` does not load the frame.
    let f = dom.get_element_by_id(d, "f").unwrap().unwrap();
    dom.watch_iframes(d).unwrap();
    let _ = dom.take_iframe_events(d);
    dom.set_attribute_ns(f, Some("urn:n"), "src", "data:text/html,x").unwrap();
    assert!(dom.take_iframe_events(d).is_empty());
    assert_eq!(dom.get_attribute(f, "src").unwrap(), None);
    // `setAttribute("src")` finds that attribute by qualified name and changes it.
    dom.set_attribute(f, "src", "data:text/html,z").unwrap();
    assert_eq!(dom.get_attribute_ns(f, Some("urn:n"), "src").unwrap(), Some("data:text/html,z"));
    assert!(dom.take_iframe_events(d).is_empty());
    dom.set_attribute_ns(f, None, "src", "data:text/html,y").unwrap();
    let events = dom.take_iframe_events(d);
    assert_eq!(events.len(), 1);
    for e in events {
        dom.release(e.node);
    }
}

#[test]
fn deselecting_every_option_sticks_until_the_options_change() {
    let (dom, d) = doc("<!doctype html><select id=s><option selected>one<option>two</select>");
    let mut dom = dom.borrow_mut();
    let s = dom.get_element_by_id(d, "s").unwrap().unwrap();
    assert_eq!(dom.selected_index(s).unwrap(), 0);
    dom.set_selected_index(s, -1).unwrap();
    assert_eq!(dom.selected_index(s).unwrap(), -1);
    // Attributes in another namespace change nothing.
    let first = dom.select_options(s).unwrap()[0];
    dom.set_attribute_ns(first, Some("urn:n"), "selected", "").unwrap();
    dom.set_attribute_ns(s, Some("urn:n"), "multiple", "").unwrap();
    dom.set_attribute_ns(s, Some("urn:n"), "size", "4").unwrap();
    assert_eq!(dom.selected_index(s).unwrap(), -1);
    // A new option runs the selectedness setting algorithm again.
    let o = dom.create_element(d, "option").unwrap();
    dom.append_child(s, o).unwrap();
    assert_eq!(dom.selected_index(s).unwrap(), 0);
    // So does the `selected` setter.
    dom.set_selected_index(s, 7).unwrap();
    assert_eq!(dom.selected_index(s).unwrap(), -1);
    dom.set_option_selected(o, false).unwrap();
    assert_eq!(dom.selected_index(s).unwrap(), 0);
    dom.set_selected_index(s, 1).unwrap();
    assert_eq!(dom.selected_index(s).unwrap(), 1);
}

#[test]
fn doctype_names_reject_whitespace_nul_and_greater_than() {
    use pyro_dom::dom::validate_doctype_name;
    for ok in ["html", "", "edi:<", "1a", "a=b", "é"] {
        assert!(validate_doctype_name(ok).is_ok(), "{ok:?}");
    }
    for bad in ["edi:>", "edi:a ", "a\tb", "a\nb", "a\u{c}b", "a\rb", "a\0b"] {
        let err = validate_doctype_name(bad).unwrap_err();
        assert_eq!(err.kind(), Some(DomErrorKind::InvalidCharacter), "{bad:?}");
    }
}

#[test]
fn a_navigated_xml_document_runs_the_insertion_steps() {
    use pyro_dom::iframe::IframeChange;
    use pyro_dom::parse::{DocumentParser, ParseStep};
    // The XML parser attaches a tree without scripts at once; iframes still
    // get their post-connection steps and `on*` attributes are compiled, as
    // for a document the HTML parser built.
    let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
    let doc = dom.borrow_mut().create_document(DocumentKind::Html, "https://example.test/page.xhtml");
    dom.borrow_mut().set_document_active(doc, true).unwrap();
    dom.borrow_mut().watch_iframes(doc).unwrap();
    let source = "<html xmlns='http://www.w3.org/1999/xhtml'><body onload='a()'>\
                  <iframe src='a.xml' onload='b()'/><p/><template><iframe/></template></body></html>";
    let mut parser = DocumentParser::new_xml(dom.clone(), doc, "application/xhtml+xml").unwrap();
    assert_eq!(parser.feed_bytes(source.as_bytes()), ParseStep::NeedInput);
    assert!(dom.borrow().document_element(doc).unwrap().is_none(), "nothing is built before the end of input");
    assert_eq!(parser.finish_bytes(), ParseStep::NeedInput, "no script to stop at");
    parser.end();
    let mut dom = dom.borrow_mut();
    let events = dom.take_iframe_events(doc);
    assert_eq!(events.len(), 1, "the iframe in the template contents is inert");
    assert_eq!(events[0].change, IframeChange::Connected);
    assert_eq!(dom.tag_name(events[0].node).unwrap(), "iframe");
    for e in events {
        dom.release(e.node);
    }
    let pending: Vec<String> =
        dom.take_pending_inline_handlers().into_iter().map(|n| dom.tag_name(n).unwrap()).collect();
    assert_eq!(pending, ["body", "iframe"]);
    // A DOMParser document stays inert.
    let inert = pyro_dom::xml::parse_document(&mut dom, "about:blank", source, "application/xhtml+xml");
    assert!(dom.take_iframe_events(inert).is_empty());
    assert!(!dom.has_pending_inline_handlers());
}
