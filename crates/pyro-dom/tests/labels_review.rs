//! Adversarial checks of the tree scope's `<label>`-by-`for` index
//! (`element.labels`, `label.control`) on every path that links, unlinks
//! or moves a label or rewrites its `for`: innerHTML / setHTMLUnsafe,
//! declarative shadow roots, adoption across documents, template
//! contents, shadow roots attached after the labels exist, controls that
//! move between scopes, duplicate ids, namespaces, attribute case, Attr
//! nodes, `moveBefore`, parser foster parenting and the adoption agency,
//! XML documents; each against the specification's definition walked over
//! the tree through the public API, with `verify_tree_state` throughout.

use std::cell::RefCell;
use std::rc::Rc;

use pyro_dom::parse::{parse_document, parse_fragment, set_html_unsafe, set_inner_html, DomRef};
use pyro_dom::select::parse_query;
use pyro_dom::*;

const XHTML: &str = "http://www.w3.org/1999/xhtml";
const SVG: &str = "http://www.w3.org/2000/svg";

fn doc(html: &str) -> (DomRef, NodeId) {
    let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
    let d = parse_document(&dom, "http://test/", html);
    dom.borrow_mut().set_document_active(d, true).unwrap();
    (dom, d)
}

fn one(dom: &Dom, root: NodeId, css: &str) -> NodeId {
    dom.query_first(root, &parse_query(css).unwrap()).unwrap().unwrap()
}

fn is_html_label(dom: &Dom, n: NodeId) -> bool {
    dom.element(n).is_ok_and(|e| e.is_html() && &*e.name.local == "label")
}

/// The labeled control by the specification's steps, through the public
/// API only.
fn control_walk(dom: &Dom, label: NodeId) -> Option<NodeId> {
    if let Some(f) = dom.get_attribute(label, "for").unwrap() {
        let root = dom.root_node(label, false).unwrap();
        let target = dom.get_element_by_id(root, f).unwrap()?;
        return dom.is_labelable(target).unwrap().then_some(target);
    }
    dom.descendants(label).unwrap().skip(1).find(|&d| dom.is_labelable(d).unwrap())
}

/// Reference `labels`: every `<label>` of the control's tree whose labeled
/// control it is, in tree order.
fn labels_walk(dom: &Dom, control: NodeId) -> Vec<NodeId> {
    if !dom.is_labelable(control).unwrap() {
        return Vec::new();
    }
    let root = dom.root_node(control, false).unwrap();
    dom.descendants(root)
        .unwrap()
        .filter(|&n| is_html_label(dom, n))
        .filter(|&l| control_walk(dom, l) == Some(control))
        .collect()
}

/// Every element of every tree under `roots` agrees with the reference,
/// and the indexes verify.
#[track_caller]
fn check(dom: &Dom, roots: &[NodeId]) {
    dom.verify_tree_state().unwrap();
    for &root in roots {
        for n in dom.descendants(root).unwrap() {
            if dom.node_type(n).unwrap() != NodeType::Element {
                continue;
            }
            assert_eq!(dom.labels(n).unwrap(), labels_walk(dom, n), "labels of {n:?}");
            if is_html_label(dom, n) {
                assert_eq!(dom.labeled_control(n).unwrap(), control_walk(dom, n), "control of {n:?}");
            }
        }
    }
}

fn ids(dom: &Dom, nodes: &[NodeId]) -> Vec<String> {
    nodes.iter().map(|&n| dom.get_attribute(n, "id").unwrap().unwrap_or("").to_owned()).collect()
}

#[test]
fn inner_html_replaces_indexed_labels_and_label_contents() {
    let (dom, d) = doc(r#"<body><div id=box><label id=l1 for=a>1</label><label id=l2><input id=q></label></div><input id=a></body>"#);
    let mut dom = dom.borrow_mut();
    let (bx, a, l2) = (one(&dom, d, "#box"), one(&dom, d, "#a"), one(&dom, d, "#l2"));
    assert_eq!(ids(&dom, &dom.labels(a).unwrap()), ["l1"]);
    // The label's own innerHTML: it stays indexed, its wrapped control goes.
    let q = one(&dom, d, "#q");
    set_inner_html(&mut dom, l2, "<span>t</span><select id=s></select>").unwrap();
    check(&dom, &[d]);
    assert!(dom.labels(q).unwrap().is_empty());
    assert_eq!(dom.labels(one(&dom, d, "#s")).unwrap(), [l2]);
    // The container's innerHTML: the old labels leave the index, new enter.
    set_inner_html(&mut dom, bx, "<label id=n1 for=a>n</label><p><label id=n2 for=a>n</label></p>").unwrap();
    check(&dom, &[d]);
    assert_eq!(ids(&dom, &dom.labels(a).unwrap()), ["n1", "n2"]);
    // Emptied.
    set_inner_html(&mut dom, bx, "").unwrap();
    check(&dom, &[d]);
    assert!(dom.labels(a).unwrap().is_empty());
    // "outerHTML": the label replaced by a parsed label with another `for`.
    set_inner_html(&mut dom, bx, "<label id=o for=a>o</label>").unwrap();
    let o = one(&dom, d, "#o");
    assert_eq!(dom.labels(a).unwrap(), [o]);
    let frag = parse_fragment(&mut dom, bx, "<label id=o2 for=a>o2</label><label id=o3 for=zz>x</label>").unwrap();
    dom.replace_child(bx, frag, o).unwrap();
    check(&dom, &[d]);
    assert_eq!(ids(&dom, &dom.labels(a).unwrap()), ["o2"]);
    // insertAdjacentHTML around the control: before and after it.
    let body = one(&dom, d, "body");
    let frag = parse_fragment(&mut dom, body, "<label id=b1 for=a>b</label>").unwrap();
    dom.insert_before(body, frag, Some(a)).unwrap();
    let frag = parse_fragment(&mut dom, body, "<label id=b2 for=a>b</label>").unwrap();
    let next = dom.next_sibling(a).unwrap();
    dom.insert_before(body, frag, next).unwrap();
    check(&dom, &[d]);
    assert_eq!(ids(&dom, &dom.labels(a).unwrap()), ["o2", "b1", "b2"]);
    // setHTMLUnsafe with a declarative shadow root holding labels.
    let host = dom.create_element(d, "div").unwrap();
    dom.append_child(body, host).unwrap();
    set_html_unsafe(
        &mut dom,
        host,
        // A declarative shadow root attaches to the template's parent, which
        // must be an element of the fragment (not the context element).
        "<div id=dsd><template shadowrootmode=open><label id=s1 for=a>s</label><label id=s2><input id=a></label></template></div><label id=light for=a>l</label>",
    )
    .unwrap();
    let dsd = one(&dom, d, "#dsd");
    let sr = dom.shadow_root(dsd).unwrap().expect("declarative shadow root");
    check(&dom, &[d, sr]);
    let sa = one(&dom, sr, "input");
    assert_eq!(ids(&dom, &dom.labels(sa).unwrap()), ["s1", "s2"]);
    assert_eq!(ids(&dom, &dom.labels(a).unwrap()), ["o2", "b1", "b2", "light"]);
    // setHTMLUnsafe on the shadow root itself replaces its labels.
    set_html_unsafe(&mut dom, sr, "<label id=s3 for=a>s</label><input id=a>").unwrap();
    check(&dom, &[d, sr]);
    assert_eq!(ids(&dom, &dom.labels(one(&dom, sr, "input")).unwrap()), ["s3"]);
}

#[test]
fn adoption_across_documents_moves_labels_between_the_documents_indexes() {
    let (dom, d) = doc(r#"<body><label id=l1 for=a>1</label><label id=l2><input id=a></label></body>"#);
    let mut dom = dom.borrow_mut();
    let (l1, l2, a) = (one(&dom, d, "#l1"), one(&dom, d, "#l2"), one(&dom, d, "#a"));
    let d2 = dom.create_document(DocumentKind::Html, "http://other/");
    let html2 = dom.create_element(d2, "html").unwrap();
    dom.append_child(d2, html2).unwrap();
    let body2 = dom.create_element(d2, "body").unwrap();
    dom.append_child(html2, body2).unwrap();
    let a2 = dom.create_element(d2, "input").unwrap();
    dom.set_attribute(a2, "id", "a").unwrap();
    dom.append_child(body2, a2).unwrap();
    check(&dom, &[d, d2]);
    // The label with `for` adopted (detached by the adoption) and appended
    // in the other document: it labels that document's `#a`.
    dom.adopt_node(d2, l1).unwrap();
    check(&dom, &[d, d2]);
    assert!(dom.labels(a).unwrap().len() == 1, "only the wrapping label");
    dom.append_child(body2, l1).unwrap();
    check(&dom, &[d, d2]);
    assert_eq!(dom.labels(a2).unwrap(), [l1]);
    assert_eq!(dom.labels(a).unwrap(), [l2]);
    assert_eq!(dom.owner_document(l1).unwrap(), Some(d2));
    // The wrapping label with its control adopted the other way round.
    dom.adopt_node(d2, l2).unwrap();
    dom.insert_before(body2, l2, Some(a2)).unwrap();
    check(&dom, &[d, d2]);
    // `#a` of the second document is now the second `a` in tree order:
    // `l1` names the wrapped one, which is first.
    assert!(dom.labels(a2).unwrap().is_empty());
    assert_eq!(dom.labels(a).unwrap(), [l2, l1]);
    // Back home, imported as a clone instead (no `#a` there yet).
    let body = one(&dom, d, "body");
    let clone = dom.clone_node(l1, true, Some(d)).unwrap();
    dom.append_child(body, clone).unwrap();
    check(&dom, &[d, d2]);
    assert_eq!(dom.labeled_control(clone).unwrap(), None);
    assert_eq!(dom.labels(a).unwrap(), [l2, l1]);
    dom.adopt_node(d, l2).unwrap();
    dom.append_child(body, l2).unwrap();
    check(&dom, &[d, d2]);
    assert_eq!(dom.labels(a).unwrap(), [clone, l2]);
    assert_eq!(dom.labels(a2).unwrap(), [l1]);
}

#[test]
fn template_contents_are_walked_and_index_once_cloned_in() {
    let (dom, d) = doc(r#"<body><template id=t><label id=tl for=a>t</label><label id=tw><input id=a></label></template><input id=a></body>"#);
    let mut dom = dom.borrow_mut();
    let (t, a) = (one(&dom, d, "#t"), one(&dom, d, "body > input"));
    let contents = dom.template_contents(t).unwrap().unwrap();
    check(&dom, &[d, contents]);
    // Nothing in the template labels the document's control...
    assert!(dom.labels(a).unwrap().is_empty());
    // ...and inside the contents (a detached tree, walked) both do.
    let ta = one(&dom, contents, "input");
    assert_eq!(ids(&dom, &dom.labels(ta).unwrap()), ["tl", "tw"]);
    // The contents cloned into the document index there.
    let body = one(&dom, d, "body");
    let clone = dom.clone_node(contents, true, Some(d)).unwrap();
    dom.append_child(body, clone).unwrap();
    check(&dom, &[d, contents]);
    // The template's `#a` clone comes after the document's `#a`.
    assert_eq!(ids(&dom, &dom.labels(a).unwrap()), ["tl"]);
    let ca = one(&dom, d, "body > label > input");
    assert_eq!(ids(&dom, &dom.labels(ca).unwrap()), ["tw"]);
    // The template's innerHTML replaces its contents (not indexed either).
    set_inner_html(&mut dom, t, "<label for=a>again</label>").unwrap();
    let contents = dom.template_contents(t).unwrap().unwrap();
    check(&dom, &[d, contents]);
    assert_eq!(ids(&dom, &dom.labels(a).unwrap()), ["tl"]);
    // A label moved out of the contents into the document.
    let again = one(&dom, contents, "label");
    dom.append_child(body, again).unwrap();
    check(&dom, &[d, contents]);
    assert_eq!(dom.labels(a).unwrap().len(), 2);
    // And a document label moved into the contents.
    let tl = one(&dom, d, "#tl");
    dom.append_child(contents, tl).unwrap();
    check(&dom, &[d, contents]);
    assert_eq!(dom.labels(a).unwrap(), [again]);
}

#[test]
fn shadow_root_attached_after_labels_exist_and_controls_moving_between_scopes() {
    let (dom, d) = doc(r#"<body><div id=host><label id=l1 for=a>light</label></div><label id=l2 for=a>2</label><input id=a></body>"#);
    let mut dom = dom.borrow_mut();
    let (host, l1, l2, a) = (one(&dom, d, "#host"), one(&dom, d, "#l1"), one(&dom, d, "#l2"), one(&dom, d, "#a"));
    assert_eq!(dom.labels(a).unwrap(), [l1, l2]);
    // Attaching a shadow root leaves the light children in the document.
    let sr = dom.attach_shadow(host, ShadowRootMode::Open).unwrap();
    check(&dom, &[d, sr]);
    assert_eq!(dom.labels(a).unwrap(), [l1, l2]);
    // The control moves into the shadow tree: no document label names it.
    dom.append_child(sr, a).unwrap();
    check(&dom, &[d, sr]);
    assert!(dom.labels(a).unwrap().is_empty());
    assert_eq!(dom.labeled_control(l1).unwrap(), None);
    // A label follows it in.
    dom.append_child(sr, l1).unwrap();
    check(&dom, &[d, sr]);
    assert_eq!(dom.labels(a).unwrap(), [l1]);
    // `moveBefore` from the light tree into the shadow tree, and back.
    dom.move_before(sr, l2, Some(a)).unwrap();
    check(&dom, &[d, sr]);
    assert_eq!(dom.labels(a).unwrap(), [l2, l1]);
    let body = one(&dom, d, "body");
    dom.move_before(body, a, None).unwrap();
    check(&dom, &[d, sr]);
    assert!(dom.labels(a).unwrap().is_empty());
    dom.move_before(body, l2, Some(a)).unwrap();
    check(&dom, &[d, sr]);
    assert_eq!(dom.labels(a).unwrap(), [l2]);
    // A wrapping label whose control is moved out and a sibling control in.
    let wrap = dom.create_element(d, "label").unwrap();
    dom.append_child(body, wrap).unwrap();
    dom.append_child(wrap, a).unwrap();
    check(&dom, &[d, sr]);
    assert_eq!(dom.labels(a).unwrap(), [l2, wrap]);
    let other = dom.create_element(d, "button").unwrap();
    dom.insert_before(wrap, other, Some(a)).unwrap();
    check(&dom, &[d, sr]);
    assert_eq!(dom.labels(a).unwrap(), [l2]);
    assert_eq!(dom.labels(other).unwrap(), [wrap]);
    // The host (its shadow tree with `l1`) adopted away and back.
    let d2 = dom.create_document(DocumentKind::Html, "http://other/");
    dom.adopt_node(d2, host).unwrap();
    check(&dom, &[d, d2, sr]);
    dom.append_child(d2, host).unwrap();
    check(&dom, &[d, d2, sr]);
    let a2 = dom.create_element(d2, "input").unwrap();
    dom.set_attribute(a2, "id", "a").unwrap();
    dom.append_child(sr, a2).unwrap();
    check(&dom, &[d, d2, sr]);
    assert_eq!(dom.labels(a2).unwrap(), [l1]);
}

#[test]
fn for_targets_inserted_later_and_duplicate_ids_whose_first_changes() {
    let (dom, d) = doc(r#"<body><label id=l for=x>x</label><div id=slot></div></body>"#);
    let mut dom = dom.borrow_mut();
    let (l, slot) = (one(&dom, d, "#l"), one(&dom, d, "#slot"));
    assert_eq!(dom.labeled_control(l).unwrap(), None);
    // The control appears later.
    let x1 = dom.create_element(d, "input").unwrap();
    dom.set_attribute(x1, "id", "x").unwrap();
    dom.append_child(slot, x1).unwrap();
    check(&dom, &[d]);
    assert_eq!(dom.labels(x1).unwrap(), [l]);
    // A second `#x` after it: still the first.
    let x2 = dom.create_element(d, "textarea").unwrap();
    dom.set_attribute(x2, "id", "x").unwrap();
    dom.append_child(slot, x2).unwrap();
    check(&dom, &[d]);
    assert_eq!(dom.labels(x1).unwrap(), [l]);
    assert!(dom.labels(x2).unwrap().is_empty());
    // The first leaves: the second is first now.
    dom.remove_child(slot, x1).unwrap();
    check(&dom, &[d]);
    assert_eq!(dom.labels(x2).unwrap(), [l]);
    // A new first that is not labelable: nobody has labels.
    let div = dom.create_element(d, "div").unwrap();
    dom.set_attribute(div, "id", "x").unwrap();
    dom.insert_before(slot, div, Some(x2)).unwrap();
    check(&dom, &[d]);
    assert!(dom.labels(x2).unwrap().is_empty());
    assert_eq!(dom.labeled_control(l).unwrap(), None);
    // Its id changes: the second is first again.
    dom.set_attribute(div, "id", "y").unwrap();
    check(&dom, &[d]);
    assert_eq!(dom.labels(x2).unwrap(), [l]);
    // The former first comes back before everything.
    let body = one(&dom, d, "body");
    dom.insert_before(body, x1, Some(l)).unwrap();
    check(&dom, &[d]);
    assert_eq!(dom.labels(x1).unwrap(), [l]);
    assert!(dom.labels(x2).unwrap().is_empty());
    // A hidden input first in tree order is still "the first element with
    // the id" (not labelable: no control for the label).
    dom.set_attribute(x1, "type", "hidden").unwrap();
    check(&dom, &[d]);
    assert!(dom.labels(x1).unwrap().is_empty());
    assert!(dom.labels(x2).unwrap().is_empty());
    assert_eq!(dom.labeled_control(l).unwrap(), None);
    dom.set_attribute(x1, "type", "checkbox").unwrap();
    check(&dom, &[d]);
    assert_eq!(dom.labels(x1).unwrap(), [l]);
}

#[test]
fn namespaces_attribute_case_and_attr_nodes() {
    let (dom, d) = doc(r#"<!doctype html><body><input id=a><input id=A></body>"#);
    let mut dom = dom.borrow_mut();
    let body = one(&dom, d, "body");
    let (a, upper) = (one(&dom, d, "#a"), one(&dom, d, "#A"));
    // An SVG `label` is not an HTML label: not indexed, no control.
    let svg_label = dom.create_element_ns(d, Some(SVG), "label").unwrap();
    dom.set_attribute(svg_label, "for", "a").unwrap();
    dom.append_child(body, svg_label).unwrap();
    check(&dom, &[d]);
    assert!(dom.labels(a).unwrap().is_empty());
    assert_eq!(dom.labeled_control(svg_label).unwrap(), None);
    dom.set_attribute(svg_label, "for", "A").unwrap();
    dom.remove_attribute(svg_label, "for").unwrap();
    check(&dom, &[d]);
    // An XHTML-namespaced label made with createElementNS is one.
    let ns_label = dom.create_element_ns(d, Some(XHTML), "label").unwrap();
    dom.set_attribute_ns(ns_label, None, "for", "a").unwrap();
    dom.append_child(body, ns_label).unwrap();
    check(&dom, &[d]);
    assert_eq!(dom.labels(a).unwrap(), [ns_label]);
    // `for` values are case-sensitive: `A` names the other input.
    dom.set_attribute(ns_label, "for", "A").unwrap();
    check(&dom, &[d]);
    assert!(dom.labels(a).unwrap().is_empty());
    assert_eq!(dom.labels(upper).unwrap(), [ns_label]);
    // `setAttribute("FOR")` in an HTML document lowercases the name.
    let l2 = dom.create_element(d, "label").unwrap();
    dom.append_child(body, l2).unwrap();
    dom.set_attribute(l2, "FOR", "a").unwrap();
    check(&dom, &[d]);
    assert_eq!(dom.labels(a).unwrap(), [l2]);
    dom.remove_attribute(l2, "FoR").unwrap();
    check(&dom, &[d]);
    assert!(dom.labels(a).unwrap().is_empty());
    // A namespaced `for` is not the `for` attribute.
    dom.set_attribute_ns(l2, Some("urn:x"), "x:for", "a").unwrap();
    check(&dom, &[d]);
    assert!(dom.labels(a).unwrap().is_empty());
    assert_eq!(dom.labeled_control(l2).unwrap(), None, "no `for`, no labelable descendant");
    let inner = dom.create_element(d, "meter").unwrap();
    dom.append_child(l2, inner).unwrap();
    check(&dom, &[d]);
    assert_eq!(dom.labels(inner).unwrap(), [l2], "wraps despite the namespaced `for`");
    dom.set_attribute_ns(l2, None, "for", "a").unwrap();
    check(&dom, &[d]);
    assert_eq!(dom.labels(a).unwrap(), [l2]);
    assert!(dom.labels(inner).unwrap().is_empty());
    dom.remove_attribute_ns(l2, Some("urn:x"), "for").unwrap();
    check(&dom, &[d]);
    assert_eq!(dom.labels(a).unwrap(), [l2]);
    dom.remove_attribute_ns(l2, None, "for").unwrap();
    check(&dom, &[d]);
    assert_eq!(dom.labels(inner).unwrap(), [l2]);
    // toggleAttribute adds an empty `for`: no control at all.
    assert!(dom.toggle_attribute(l2, "for", None).unwrap());
    check(&dom, &[d]);
    assert!(dom.labels(inner).unwrap().is_empty());
    assert!(dom.labels(a).unwrap().is_empty());
    assert!(!dom.toggle_attribute(l2, "for", None).unwrap());
    check(&dom, &[d]);
    assert_eq!(dom.labels(inner).unwrap(), [l2]);
    // Attr nodes: `label.attributes.for.value = ..`, setAttributeNode,
    // removeAttributeNode.
    dom.set_attribute(l2, "for", "A").unwrap();
    let attr = dom.attribute_node(l2, None, "for").unwrap().unwrap();
    assert_eq!(dom.labels(upper).unwrap(), [ns_label, l2]);
    dom.set_attr_node_value(attr, "a").unwrap();
    check(&dom, &[d]);
    assert_eq!(dom.labels(a).unwrap(), [l2]);
    assert_eq!(dom.labels(upper).unwrap(), [ns_label]);
    dom.remove_attribute_node(l2, attr).unwrap();
    check(&dom, &[d]);
    assert!(dom.labels(a).unwrap().is_empty());
    assert_eq!(dom.labels(inner).unwrap(), [l2]);
    dom.set_attr_node_value(attr, "A").unwrap();
    dom.set_attribute_node(l2, attr).unwrap();
    check(&dom, &[d]);
    assert_eq!(dom.labels(upper).unwrap(), [ns_label, l2]);
    // Replacing the Attr node of an existing `for` with another one.
    let other = dom.attribute_node(ns_label, None, "for").unwrap().unwrap();
    dom.remove_attribute_node(ns_label, other).unwrap();
    dom.set_attr_node_value(other, "a").unwrap();
    assert_eq!(dom.set_attribute_node(l2, other).unwrap(), Some(attr));
    check(&dom, &[d]);
    assert_eq!(dom.labels(a).unwrap(), [l2]);
    assert!(dom.labels(upper).unwrap().is_empty());
}

#[test]
fn xml_documents_index_only_xhtml_labels() {
    let mut dom = Dom::new();
    let d = xml::parse_document(
        &mut dom,
        "http://test/x.xml",
        r#"<r><label for="a">plain</label><input id="a"/><h:label xmlns:h="http://www.w3.org/1999/xhtml" for="a">x</h:label><h:input xmlns:h="http://www.w3.org/1999/xhtml" id="a"/></r>"#,
        "application/xml",
    );
    dom.verify_tree_state().unwrap();
    let r = dom.document_element(d).unwrap().unwrap();
    let kids: Vec<NodeId> = dom.children(r).unwrap().collect();
    let (plain_label, plain_input, h_label, h_input) = (kids[0], kids[1], kids[2], kids[3]);
    assert_eq!(dom.labeled_control(plain_label).unwrap(), None, "not an HTML element");
    assert!(dom.labels(plain_input).unwrap().is_empty(), "not an HTML input");
    // The XHTML label names the first `#a`, which is no labelable element.
    assert_eq!(dom.labeled_control(h_label).unwrap(), None);
    assert!(dom.labels(h_input).unwrap().is_empty());
    dom.set_attribute(plain_input, "id", "b").unwrap();
    dom.verify_tree_state().unwrap();
    assert_eq!(dom.labels(h_input).unwrap(), [h_label]);
    check(&dom, &[d]);
    // Adopted into an HTML document, namespaces stay: the plain label is
    // still no label.
    let (hd, hdoc) = doc("<body></body>");
    let mut hd = hd.borrow_mut();
    let body = one(&hd, hdoc, "body");
    let plain = hd.create_element_ns(hdoc, None, "label").unwrap();
    hd.set_attribute(plain, "for", "a").unwrap();
    hd.append_child(body, plain).unwrap();
    let a = hd.create_element(hdoc, "input").unwrap();
    hd.set_attribute(a, "id", "a").unwrap();
    hd.append_child(body, a).unwrap();
    check(&hd, &[hdoc]);
    assert!(hd.labels(a).unwrap().is_empty());
    hd.set_attribute(plain, "for", "b").unwrap();
    hd.remove_attribute(plain, "for").unwrap();
    check(&hd, &[hdoc]);
}

#[test]
fn parser_foster_parenting_and_adoption_agency_keep_the_index() {
    let (dom, d) = doc(concat!(
        "<body>",
        "<table><label id=f1 for=a>foster</label><tr><td><label id=f2 for=a>cell</label></td></tr><label id=f3><input id=b></label></table>",
        "<b><label id=m1 for=a>x<p id=p>y</b>z</p>",
        "<input id=a>",
        "</body>"
    ));
    let dom = dom.borrow();
    check(&dom, &[d]);
    let a = one(&dom, d, "#a");
    assert_eq!(ids(&dom, &dom.labels(a).unwrap()), ["f1", "f2", "m1"]);
    let b = one(&dom, d, "#b");
    assert_eq!(ids(&dom, &dom.labels(b).unwrap()), ["f3"]);
}

#[test]
fn fragment_parsers_host_documents_are_not_indexes() {
    // While a fragment is parsed, its elements live in a parser-only
    // document; the index is only read for real tree scopes.
    let (dom, d) = doc("<body><div id=c></div></body>");
    let mut dom = dom.borrow_mut();
    let c = one(&dom, d, "#c");
    for _ in 0..3 {
        let frag = parse_fragment(&mut dom, c, "<label for=a>1</label><label><input id=a></label><label for=a>3</label>").unwrap();
        let a = one(&dom, frag, "input");
        // The fragment is a detached tree: walked, three labels.
        assert_eq!(dom.labels(a).unwrap().len(), 3);
        assert_eq!(dom.labels(a).unwrap(), labels_walk(&dom, a));
        dom.append_child(c, frag).unwrap();
        check(&dom, &[d]);
    }
    let first = one(&dom, d, "#c input");
    assert_eq!(dom.labels(first).unwrap().len(), 7, "every `for=a` names the first #a; one wraps it");
    set_inner_html(&mut dom, c, "").unwrap();
    check(&dom, &[d]);
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

const IDS: [&str; 5] = ["a", "b", "c", "d", ""];

/// Random mutations through the paths the first random test does not take:
/// innerHTML, setHTMLUnsafe (declarative shadow roots), `moveBefore`,
/// adoption between two documents, template contents cloned in, Attr node
/// writes, replaceChild and namespaced `for`; verified after every step.
#[test]
fn random_mutations_through_every_path_agree_with_the_walk() {
    let (dom, d) = doc(
        r#"<body><div id=root>
        <label for=a>1</label><label for=b>2</label><label><input id=a></label>
        <div><label for=c>3</label><label><label><button id=b>b</button></label></label></div>
        <input id=c><select id=d></select><label>4<div><textarea></textarea></div></label>
        <template><label for=a>t</label><label><input id=a></label></template>
        <div id=host></div></div></body>"#,
    );
    let mut dom = dom.borrow_mut();
    let host = one(&dom, d, "#host");
    let sr = dom.attach_shadow(host, ShadowRootMode::Open).unwrap();
    set_inner_html(&mut dom, host, "<label for=a>s</label><label><input id=a></label>").unwrap();
    let frag = parse_fragment(&mut dom, host, "<label for=b>s</label><output id=b></output>").unwrap();
    dom.append_child(sr, frag).unwrap();
    let d2 = dom.create_document(DocumentKind::Html, "http://other/");
    let root2 = dom.create_element(d2, "html").unwrap();
    dom.append_child(d2, root2).unwrap();
    set_inner_html(&mut dom, root2, "<body><label for=a>o</label><input id=a><label><input id=b></label></body>").unwrap();
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let scopes = |dom: &Dom| -> Vec<NodeId> {
        let mut v = vec![d, sr, d2];
        for n in dom.descendants(d).unwrap().chain(dom.descendants(d2).unwrap()) {
            if let Ok(Some(s)) = dom.shadow_root(n) {
                if s != sr {
                    v.push(s);
                }
            }
        }
        v
    };
    let elements = |dom: &Dom| -> Vec<NodeId> {
        let mut v: Vec<NodeId> = scopes(dom)
            .into_iter()
            .flat_map(|s| dom.descendants(s).unwrap().collect::<Vec<_>>())
            .filter(|&n| dom.node_type(n).unwrap() == NodeType::Element)
            .collect();
        v.retain(|&n| !matches!(&*dom.element(n).unwrap().name.local, "html" | "head" | "body"));
        v
    };
    let mut verify_at = 0;
    for step in 0..500 {
        let els = elements(&dom);
        let pick = els[rng.below(els.len())];
        match rng.below(9) {
            0 => {
                let f = IDS[rng.below(IDS.len())];
                set_inner_html(
                    &mut dom,
                    pick,
                    &match rng.below(3) {
                        0 => format!("<label for={f}>n</label><input id={f}>"),
                        1 => format!("<label>n<input id={f}></label><label for={f}>m</label>"),
                        _ => String::new(),
                    },
                )
                .unwrap();
            }
            1 => {
                // Declarative shadow root (when the element has none yet),
                // or plain setHTMLUnsafe content.
                let f = IDS[rng.below(IDS.len())];
                let _ = set_html_unsafe(
                    &mut dom,
                    pick,
                    &format!("<template shadowrootmode=open><label for={f}>d</label><input id={f}></template><label for={f}>l</label>"),
                );
            }
            2 => {
                // moveBefore within the shadow-including root (fails
                // harmlessly otherwise).
                let target = els[rng.below(els.len())];
                if target != pick && !dom.descendants(pick).unwrap().any(|n| n == target) && dom.parent(pick).unwrap().is_some() {
                    let first = dom.first_child(target).unwrap();
                    let _ = dom.move_before(target, pick, first);
                }
            }
            3 => {
                // Adopt into the other document and append somewhere there.
                let (from, to) = if dom.owner_document(pick).unwrap() == Some(d2) { (d2, d) } else { (d, d2) };
                let _ = from;
                let targets: Vec<NodeId> = dom
                    .descendants(to)
                    .unwrap()
                    .filter(|&n| dom.node_type(n).unwrap() == NodeType::Element && n != pick && !dom.descendants(pick).unwrap().any(|x| x == n))
                    .collect();
                if !targets.is_empty() && dom.parent(pick).unwrap().map_or(false, |p| p != d && p != d2) {
                    let target = targets[rng.below(targets.len())];
                    dom.adopt_node(to, pick).unwrap();
                    let _ = dom.append_child(target, pick);
                }
            }
            4 => {
                // Template contents cloned next to the template; or a node
                // moved into the contents.
                let templates: Vec<NodeId> = els.iter().copied().filter(|&n| &*dom.element(n).unwrap().name.local == "template").collect();
                if let Some(&t) = templates.first() {
                    let contents = dom.template_contents(t).unwrap().unwrap();
                    if rng.below(2) == 0 {
                        let owner = dom.owner_document(t).unwrap().unwrap();
                        let clone = dom.clone_node(contents, true, Some(owner)).unwrap();
                        if let Some(p) = dom.parent(t).unwrap() {
                            let _ = dom.insert_before(p, clone, Some(t));
                        }
                    } else if pick != t && dom.parent(pick).unwrap().is_some() && !dom.descendants(pick).unwrap().any(|n| n == t) {
                        let owner = dom.owner_document(contents).unwrap().unwrap();
                        let _ = dom.adopt_node(owner, pick);
                        let _ = dom.append_child(contents, pick);
                    }
                }
            }
            5 => {
                // `for` through an Attr node, or namespaced.
                let f = IDS[rng.below(IDS.len())];
                match rng.below(3) {
                    0 => {
                        if let Some(attr) = dom.attribute_node(pick, None, "for").unwrap() {
                            dom.set_attr_node_value(attr, f).unwrap();
                        } else {
                            dom.set_attribute(pick, "for", f).unwrap();
                        }
                    }
                    1 => {
                        if let Some(attr) = dom.attribute_node(pick, None, "for").unwrap() {
                            dom.remove_attribute_node(pick, attr).unwrap();
                        }
                    }
                    _ => {
                        dom.set_attribute_ns(pick, Some("urn:x"), "x:for", f).unwrap();
                    }
                }
            }
            6 => {
                let id = IDS[rng.below(IDS.len())];
                if rng.below(3) == 0 {
                    dom.remove_attribute(pick, "id").unwrap();
                } else {
                    dom.set_attribute(pick, "id", id).unwrap();
                }
            }
            7 => {
                // replaceChild with a fresh label, or remove.
                if let Some(p) = dom.parent(pick).unwrap() {
                    if p != d && p != d2 && dom.node_type(p).unwrap() == NodeType::Element {
                        if rng.below(2) == 0 {
                            let f = IDS[rng.below(IDS.len())];
                            let frag = parse_fragment(&mut dom, p, &format!("<label for={f}><input id={f}></label>")).unwrap();
                            dom.replace_child(p, frag, pick).unwrap();
                        } else {
                            dom.remove_child(p, pick).unwrap();
                        }
                    }
                }
            }
            _ => {
                if &*dom.element(pick).unwrap().name.local == "input" {
                    let t = ["hidden", "text", "checkbox"][rng.below(3)];
                    dom.set_attribute(pick, "type", t).unwrap();
                } else if dom.parent(pick).unwrap().is_some_and(|p| p != d && p != d2) {
                    let c = dom.clone_node(pick, true, None).unwrap();
                    let p = dom.parent(pick).unwrap().unwrap();
                    let _ = dom.insert_before(p, c, Some(pick));
                }
            }
        }
        if step >= verify_at {
            check(&dom, &scopes(&dom));
            verify_at = step + 1 + (step / 50);
        }
    }
    check(&dom, &scopes(&dom));
}
