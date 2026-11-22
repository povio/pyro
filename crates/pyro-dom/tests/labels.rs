//! `element.labels` from the tree scope's index of `<label>` elements by
//! `for` (kept at mutation time) and the control's ancestors, checked
//! against the specification's definition walked over the whole tree:
//! the labels whose labeled control the element is, in tree order.

use std::cell::RefCell;
use std::rc::Rc;

use pyro_dom::parse::{parse_document, parse_fragment, DomRef};
use pyro_dom::select::parse_query;
use pyro_dom::*;

fn doc(html: &str) -> (DomRef, NodeId) {
    let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
    let d = parse_document(&dom, "http://test/", html);
    dom.borrow_mut().set_document_active(d, true).unwrap();
    (dom, d)
}

fn one(dom: &Dom, root: NodeId, css: &str) -> NodeId {
    dom.query_first(root, &parse_query(css).unwrap()).unwrap().unwrap()
}

fn all(dom: &Dom, root: NodeId, css: &str) -> Vec<NodeId> {
    dom.query_all(root, &parse_query(css).unwrap()).unwrap()
}

fn is_html_label(dom: &Dom, n: NodeId) -> bool {
    dom.element(n).is_ok_and(|e| e.is_html() && &*e.name.local == "label")
}

/// The labeled control by the specification's steps, through the public
/// API only (`getElementById` of the label's tree, or the first labelable
/// descendant).
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

/// Every labelable element of every tree scope reachable from `d` agrees
/// with the reference, and the indexes verify.
#[track_caller]
fn check(dom: &Dom, d: NodeId, scopes: &[NodeId]) {
    dom.verify_tree_state().unwrap();
    for &scope in std::iter::once(&d).chain(scopes) {
        for n in dom.descendants(scope).unwrap() {
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

#[test]
fn labels_by_for_wrapping_and_nested_in_tree_order() {
    let (dom, d) = doc(r#"<body>
        <label id=l1 for=a>A1</label>
        <label id=l2><span>wrap</span><input id=a></label>
        <label id=l3 for=a>A3</label>
        <label id=l4><label id=l5><input id=b></label></label>
        <label id=l6 for=b>B6</label>
        <label id=l7><input id=c><input id=c2></label>
        <label id=l8 for=h>hidden</label><input id=h type=hidden>
        <label id=l9 for=x>nothing</label>
        <label id=l10 for="">empty</label>
        <label id=l11 for=d>dup</label><div id=d>not a control</div><input id=d>
        <label id=l12><div>no control</div></label>
    </body>"#);
    let dom = dom.borrow();
    let ids = |nodes: Vec<NodeId>| -> Vec<String> {
        nodes.iter().map(|&n| dom.get_attribute(n, "id").unwrap().unwrap().to_owned()).collect()
    };
    assert_eq!(ids(dom.labels(one(&dom, d, "#a")).unwrap()), ["l1", "l2", "l3"]);
    assert_eq!(ids(dom.labels(one(&dom, d, "#b")).unwrap()), ["l4", "l5", "l6"]);
    assert_eq!(ids(dom.labels(one(&dom, d, "#c")).unwrap()), ["l7"]);
    assert!(dom.labels(one(&dom, d, "#c2")).unwrap().is_empty());
    // A hidden input is not labelable: no labels, and `l8` has no control.
    assert!(dom.labels(one(&dom, d, "#h")).unwrap().is_empty());
    assert_eq!(dom.labeled_control(one(&dom, d, "#l8")).unwrap(), None);
    assert_eq!(dom.labeled_control(one(&dom, d, "#l9")).unwrap(), None);
    assert_eq!(dom.labeled_control(one(&dom, d, "#l10")).unwrap(), None);
    // `for` names the first element with the id, a div here.
    assert_eq!(dom.labeled_control(one(&dom, d, "#l11")).unwrap(), None);
    assert!(dom.labels(one(&dom, d, "input#d")).unwrap().is_empty());
    assert_eq!(dom.labeled_control(one(&dom, d, "#l12")).unwrap(), None);
    assert_eq!(dom.labeled_control(one(&dom, d, "body")).unwrap(), None);
    check(&dom, d, &[]);
}

#[test]
fn for_and_id_changes_follow_the_index() {
    let (dom, d) = doc(r#"<body><label id=l1 for=a>A</label><label id=l2>B<input id=b></label><input id=a><input id=c></body>"#);
    let mut dom = dom.borrow_mut();
    let (l1, l2, a, b, c) =
        (one(&dom, d, "#l1"), one(&dom, d, "#l2"), one(&dom, d, "#a"), one(&dom, d, "#b"), one(&dom, d, "#c"));
    assert_eq!(dom.labels(a).unwrap(), [l1]);
    // Retargeted by `for`.
    dom.set_attribute(l1, "for", "c").unwrap();
    check(&dom, d, &[]);
    assert!(dom.labels(a).unwrap().is_empty());
    assert_eq!(dom.labels(c).unwrap(), [l1]);
    // A wrapping label given a `for` stops wrapping.
    dom.set_attribute(l2, "for", "c").unwrap();
    check(&dom, d, &[]);
    assert!(dom.labels(b).unwrap().is_empty());
    assert_eq!(dom.labels(c).unwrap(), [l1, l2]);
    // Same value again: nothing changes.
    dom.set_attribute(l2, "for", "c").unwrap();
    check(&dom, d, &[]);
    assert_eq!(dom.labels(c).unwrap(), [l1, l2]);
    // Emptied: no control.
    dom.set_attribute(l2, "for", "").unwrap();
    check(&dom, d, &[]);
    assert_eq!(dom.labels(c).unwrap(), [l1]);
    assert!(dom.labels(b).unwrap().is_empty());
    // Removed: wraps again.
    dom.remove_attribute(l2, "for").unwrap();
    check(&dom, d, &[]);
    assert_eq!(dom.labels(b).unwrap(), [l2]);
    // The control's id changes: labels by `for` follow the id, not the node.
    dom.set_attribute(c, "id", "z").unwrap();
    check(&dom, d, &[]);
    assert!(dom.labels(c).unwrap().is_empty());
    dom.set_attribute(a, "id", "c").unwrap();
    check(&dom, d, &[]);
    assert_eq!(dom.labels(a).unwrap(), [l1]);
    // A second element takes the id first in tree order.
    dom.set_attribute(l2, "id", "c").unwrap();
    check(&dom, d, &[]);
    assert!(dom.labels(a).unwrap().is_empty());
    dom.remove_attribute(l2, "id").unwrap();
    check(&dom, d, &[]);
    assert_eq!(dom.labels(a).unwrap(), [l1]);
    // The `for` attribute on something that is not a label is not indexed.
    dom.set_attribute(a, "for", "c").unwrap();
    check(&dom, d, &[]);
    // A label with `for` created detached, then linked and unlinked.
    let l3 = dom.create_element(d, "label").unwrap();
    dom.set_attribute(l3, "for", "c").unwrap();
    assert!(dom.labels(a).unwrap() == [l1]);
    let body = one(&dom, d, "body");
    let first = dom.first_child(body).unwrap();
    dom.insert_before(body, l3, first).unwrap();
    check(&dom, d, &[]);
    assert_eq!(dom.labels(a).unwrap(), [l3, l1]);
    dom.remove_child(body, l3).unwrap();
    check(&dom, d, &[]);
    assert_eq!(dom.labels(a).unwrap(), [l1]);
    // Detached, its `for` changes do not touch the document's index.
    dom.set_attribute(l3, "for", "q").unwrap();
    dom.remove_attribute(l3, "for").unwrap();
    check(&dom, d, &[]);
}

#[test]
fn label_subtrees_inserted_removed_and_moved() {
    let (dom, d) = doc(r#"<body><div id=box></div><input id=a><div id=other></div></body>"#);
    let mut dom = dom.borrow_mut();
    let (bx, a, other) = (one(&dom, d, "#box"), one(&dom, d, "#a"), one(&dom, d, "#other"));
    // A parsed fragment with labels at several depths.
    let frag = parse_fragment(&mut dom, bx, "<label for=a>1</label><p><label for=a>2</label><label>3<input id=q></label></p>").unwrap();
    dom.append_child(bx, frag).unwrap();
    check(&dom, d, &[]);
    assert_eq!(dom.labels(a).unwrap().len(), 2);
    let q = one(&dom, d, "#q");
    assert_eq!(dom.labels(q).unwrap().len(), 1);
    // The whole box moves after the control: order follows.
    let labels_before = dom.labels(a).unwrap();
    dom.append_child(other, bx).unwrap();
    check(&dom, d, &[]);
    assert_eq!(dom.labels(a).unwrap(), labels_before);
    let body = one(&dom, d, "body");
    let first = dom.first_child(body).unwrap();
    dom.insert_before(body, bx, first).unwrap();
    check(&dom, d, &[]);
    // A label removed with its subtree, then its clone inserted.
    let p = one(&dom, d, "#box p");
    let clone = dom.clone_node(p, true, None).unwrap();
    dom.remove_child(bx, p).unwrap();
    check(&dom, d, &[]);
    assert_eq!(dom.labels(a).unwrap().len(), 1);
    dom.append_child(body, clone).unwrap();
    check(&dom, d, &[]);
    assert_eq!(dom.labels(a).unwrap().len(), 2);
    // The control moves into a wrapping label.
    let wrap = one(&dom, d, "body > p > label:not([for])");
    dom.append_child(wrap, a).unwrap();
    check(&dom, d, &[]);
    // Not its first labelable descendant (the cloned `#q` is), so it is not
    // wrapped; still named by `for` twice.
    assert_eq!(dom.labels(a).unwrap().len(), 2);
    // Everything cleared.
    let kids: Vec<NodeId> = dom.descendants(body).unwrap().skip(1).filter(|&n| dom.parent(n).unwrap() == Some(body)).collect();
    for k in kids {
        dom.remove_child(body, k).unwrap();
    }
    check(&dom, d, &[]);
}

#[test]
fn shadow_trees_scope_labels_to_their_own_tree() {
    let (dom, d) = doc(r#"<body><div id=host></div><label id=outer for=a>light</label><input id=a></body>"#);
    let mut dom = dom.borrow_mut();
    let (host, outer, a) = (one(&dom, d, "#host"), one(&dom, d, "#outer"), one(&dom, d, "#a"));
    let sr = dom.attach_shadow(host, ShadowRootMode::Open).unwrap();
    let frag = parse_fragment(&mut dom, host, "<label id=inner for=a>shadow</label><label id=wrap><input id=a></label>").unwrap();
    dom.append_child(sr, frag).unwrap();
    check(&dom, d, &[sr]);
    let inner = one(&dom, sr, "#inner");
    let wrap = one(&dom, sr, "#wrap");
    let shadow_a = one(&dom, sr, "input");
    // Each `for` resolves in its own tree.
    assert_eq!(dom.labels(a).unwrap(), [outer]);
    assert_eq!(dom.labels(shadow_a).unwrap(), [inner, wrap]);
    assert_eq!(dom.labeled_control(inner).unwrap(), Some(shadow_a));
    // A label moved from the shadow tree into the document changes scope.
    let body = one(&dom, d, "body");
    dom.append_child(body, inner).unwrap();
    check(&dom, d, &[sr]);
    assert_eq!(dom.labels(a).unwrap(), [outer, inner]);
    assert_eq!(dom.labels(shadow_a).unwrap(), [wrap]);
    // And back.
    dom.append_child(sr, inner).unwrap();
    check(&dom, d, &[sr]);
    assert_eq!(dom.labels(a).unwrap(), [outer]);
    assert_eq!(dom.labels(shadow_a).unwrap(), [wrap, inner]);
    // The host (with its shadow tree) leaves and returns.
    dom.remove_child(body, host).unwrap();
    check(&dom, d, &[sr]);
    assert_eq!(dom.labels(shadow_a).unwrap(), [wrap, inner]);
    dom.append_child(body, host).unwrap();
    check(&dom, d, &[sr]);
    // A nested shadow tree inside the shadow tree is its own scope too.
    let deep_host = dom.create_element(d, "div").unwrap();
    dom.append_child(sr, deep_host).unwrap();
    let deep = dom.attach_shadow(deep_host, ShadowRootMode::Closed).unwrap();
    let frag = parse_fragment(&mut dom, deep_host, "<label for=a>deep</label>").unwrap();
    dom.append_child(deep, frag).unwrap();
    check(&dom, d, &[sr, deep]);
    assert_eq!(dom.labels(shadow_a).unwrap(), [wrap, inner]);
    assert_eq!(dom.labels(a).unwrap(), [outer]);
}

#[test]
fn detached_trees_are_walked() {
    let (dom, d) = doc("<body></body>");
    let mut dom = dom.borrow_mut();
    let div = dom.create_element(d, "div").unwrap();
    let frag = parse_fragment(&mut dom, div, "<label for=a>1</label><label><input id=a></label><label for=a>3</label>").unwrap();
    dom.append_child(div, frag).unwrap();
    let a = one(&dom, div, "input");
    assert_eq!(dom.labels(a).unwrap().len(), 3);
    assert_eq!(dom.labels(a).unwrap(), labels_walk(&dom, a));
    let labels = all(&dom, div, "label");
    assert_eq!(dom.labels(a).unwrap(), labels);
    // Connected: the same answer from the index.
    let body = one(&dom, d, "body");
    dom.append_child(body, div).unwrap();
    check(&dom, d, &[]);
    assert_eq!(dom.labels(a).unwrap(), labels);
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

#[test]
fn random_mutations_agree_with_the_walk() {
    let (dom, d) = doc(
        r#"<body><div id=root>
        <label for=a>1</label><label for=b>2</label><label><input id=a></label>
        <div><label for=c>3</label><label><label><button id=b>b</button></label></label></div>
        <input id=c><select id=d></select><label>4<div><textarea></textarea></div></label>
        <div id=host></div></div></body>"#,
    );
    let mut dom = dom.borrow_mut();
    let host = one(&dom, d, "#host");
    let sr = dom.attach_shadow(host, ShadowRootMode::Open).unwrap();
    let frag = parse_fragment(&mut dom, host, "<label for=a>s</label><label><input id=a></label>").unwrap();
    dom.append_child(sr, frag).unwrap();
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let elements = |dom: &Dom| -> Vec<NodeId> {
        let mut v: Vec<NodeId> = dom.descendants(d).unwrap().chain(dom.descendants(sr).unwrap()).filter(|&n| dom.node_type(n).unwrap() == NodeType::Element).collect();
        v.retain(|&n| !matches!(&*dom.element(n).unwrap().name.local, "html" | "head" | "body"));
        v
    };
    for step in 0..600 {
        let els = elements(&dom);
        let pick = els[rng.below(els.len())];
        match rng.below(7) {
            0 => {
                // `for` on a label (or any element: only labels index it).
                let f = IDS[rng.below(IDS.len())];
                dom.set_attribute(pick, "for", f).unwrap();
            }
            1 => {
                dom.remove_attribute(pick, "for").unwrap();
            }
            2 => {
                let id = IDS[rng.below(IDS.len())];
                dom.set_attribute(pick, "id", id).unwrap();
            }
            3 => {
                // Move a node (with its subtree) under another, within or
                // across the two scopes.
                let target = els[rng.below(els.len())];
                if target != pick && !dom.descendants(pick).unwrap().any(|n| n == target) && dom.parent(pick).unwrap().is_some() {
                    let _ = dom.append_child(target, pick);
                }
            }
            4 => {
                // Insert a fresh label subtree.
                let f = IDS[rng.below(IDS.len())];
                let html = match rng.below(3) {
                    0 => format!("<label for={f}>n</label>"),
                    1 => format!("<label>n<input id={f}></label>"),
                    _ => format!("<div><label for={f}><label><meter></meter></label></label></div>"),
                };
                let frag = parse_fragment(&mut dom, pick, &html).unwrap();
                let _ = dom.append_child(pick, frag);
            }
            5 => {
                // Remove a node, or an input's type flips to/from hidden.
                if rng.below(2) == 0 {
                    if let Some(p) = dom.parent(pick).unwrap() {
                        if p != d {
                            dom.remove_child(p, pick).unwrap();
                        }
                    }
                } else if &*dom.element(pick).unwrap().name.local == "input" {
                    let t = if rng.below(2) == 0 { "hidden" } else { "text" };
                    dom.set_attribute(pick, "type", t).unwrap();
                }
            }
            _ => {
                // Clone a subtree (labels included) next to the original.
                if let Some(p) = dom.parent(pick).unwrap() {
                    if p != d {
                        let c = dom.clone_node(pick, true, None).unwrap();
                        let _ = dom.insert_before(p, c, Some(pick));
                    }
                }
            }
        }
        if step % 5 == 0 || step > 550 {
            check(&dom, d, &[sr]);
        }
    }
    check(&dom, d, &[sr]);
}

#[test]
fn nested_reference_targets_associate_labels_in_shadow_including_order() {
    let (dom, document) = doc("<label id=a for=host></label><div id=host></div><label id=c for=host></label>");
    let mut dom = dom.borrow_mut();
    let host = one(&dom, document, "#host");
    let outer = dom.attach_shadow(host, ShadowRootMode::Closed).unwrap();
    let inner_host = dom.create_element(document, "div").unwrap();
    dom.set_attribute(inner_host, "id", "inner").unwrap();
    dom.append_child(outer, inner_host).unwrap();
    let inner = dom.attach_shadow(inner_host, ShadowRootMode::Open).unwrap();
    let b = dom.create_element(document, "label").unwrap();
    let input = dom.create_element(document, "input").unwrap();
    dom.set_attribute(input, "id", "control").unwrap();
    dom.append_child(b, input).unwrap();
    dom.append_child(inner, b).unwrap();
    let a = one(&dom, document, "#a");
    let c = one(&dom, document, "#c");
    dom.set_shadow_reference_target(outer, Some("inner")).unwrap();
    dom.set_shadow_reference_target(inner, Some("control")).unwrap();
    assert_eq!(dom.labels(input).unwrap(), [a, b, c]);
    assert_eq!(dom.labeled_control(a).unwrap(), Some(input));
    assert_eq!(dom.retarget_node(input, document), Some(host));
    dom.set_shadow_reference_target(inner, Some("missing")).unwrap();
    assert_eq!(dom.labels(input).unwrap(), [b]);
    assert_eq!(dom.resolve_reference_target(host).unwrap(), None);
}

#[test]
fn external_form_controls_require_connection_and_retarget_to_the_host() {
    let (dom, document) = doc("<section><button form=host></button><div id=host></div></section>");
    let mut dom = dom.borrow_mut();
    let section = one(&dom, document, "section");
    let parent = dom.parent(section).unwrap().unwrap();
    let button = one(&dom, section, "button");
    let host = one(&dom, section, "#host");
    let shadow = dom.attach_shadow(host, ShadowRootMode::Open).unwrap();
    let form = dom.create_element(document, "form").unwrap();
    let input = dom.create_element(document, "input").unwrap();
    dom.set_attribute(form, "id", "f").unwrap();
    dom.append_child(form, input).unwrap();
    dom.append_child(shadow, form).unwrap();
    dom.set_shadow_reference_target(shadow, Some("f")).unwrap();
    assert_eq!(dom.form_owner(button).unwrap(), Some(form));
    assert_eq!(dom.retarget_node(form, document), Some(host));
    assert_eq!(dom.owned_form_elements(form).unwrap(), [button, input]);
    dom.remove_child(parent, section).unwrap();
    assert_eq!(dom.form_owner(button).unwrap(), None);
    assert_eq!(dom.form_owner(input).unwrap(), Some(form));
    assert_eq!(dom.owned_form_elements(form).unwrap(), [input]);
    dom.set_attribute(input, "form", "f").unwrap();
    assert_eq!(dom.form_owner(input).unwrap(), None);
}
