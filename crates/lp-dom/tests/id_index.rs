//! `get_element_by_id` keeps its index across mutations that cannot change
//! ids and rebuilds after any that can. Checked differentially against a
//! full tree walk, plus a build counter showing the index is really reused.

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

/// Reference `getElementById`: the first light-tree descendant of `root`
/// in tree order whose null-namespace `id` attribute equals `id`.
fn walk(dom: &Dom, root: NodeId, id: &str) -> Option<NodeId> {
    fn visit(dom: &Dom, n: NodeId, id: &str) -> Option<NodeId> {
        let mut c = dom.first_child(n).unwrap();
        while let Some(k) = c {
            if dom.node_type(k).unwrap() == NodeType::Element
                && dom.get_attribute_ns(k, None, "id").unwrap() == Some(id)
            {
                return Some(k);
            }
            if let Some(f) = visit(dom, k, id) {
                return Some(f);
            }
            c = dom.next_sibling(k).unwrap();
        }
        None
    }
    if id.is_empty() {
        return None;
    }
    visit(dom, root, id)
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

const IDS: [&str; 6] = ["a", "b", "c", "d", "e", ""];

fn elements_under(dom: &Dom, root: NodeId, out: &mut Vec<NodeId>) {
    let mut c = dom.first_child(root).unwrap();
    while let Some(k) = c {
        if dom.node_type(k).unwrap() == NodeType::Element {
            out.push(k);
        }
        elements_under(dom, k, out);
        c = dom.next_sibling(k).unwrap();
    }
}

fn check_all(dom: &Dom, roots: &[NodeId], step: &str) {
    // Tree flags, attribute atoms and every tree scope's id map, against walks.
    if let Err(e) = dom.verify_tree_state() {
        panic!("{e} after {step}");
    }
    for &r in roots {
        for id in IDS.iter().chain(["missing"].iter()) {
            assert_eq!(dom.get_element_by_id(r, id).unwrap(), walk(dom, r, id), "root {r:?} id {id:?} after {step}");
        }
    }
}

#[test]
fn id_lookups_match_a_full_walk_through_mixed_mutations() {
    let (dom, d) = doc("<body><div id=a><span id=b>t</span><input id=c></div><div id=a><p id=d>x</p></div>\
         <section><b id=e></b><i id=b></i></section></body>");
    let mut dom = dom.borrow_mut();
    let body = dom.body(d).unwrap().unwrap();
    // Extra roots: a detached fragment, a detached element subtree, a shadow
    // root and another document (adoption source).
    let frag = dom.create_fragment(d).unwrap();
    let detached = dom.create_element(d, "div").unwrap();
    for (parent, id) in [(frag, "a"), (frag, "b"), (detached, "c"), (detached, "a")] {
        let e = dom.create_element(d, "em").unwrap();
        dom.set_attribute(e, "id", id).unwrap();
        dom.append_child(parent, e).unwrap();
    }
    let host = dom.create_element(d, "div").unwrap();
    dom.append_child(body, host).unwrap();
    let shadow = dom.attach_shadow(host, ShadowRootMode::Open).unwrap();
    let s1 = dom.create_element(d, "b").unwrap();
    dom.set_attribute(s1, "id", "a").unwrap();
    dom.append_child(shadow, s1).unwrap();
    let other = dom.create_document(DocumentKind::Html, "http://other/");
    let roots = [d, body, frag, detached, shadow];
    check_all(&dom, &roots, "setup");

    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    for batch in 0..400 {
        let ops = 1 + rng.below(12);
        let mut log = Vec::new();
        for _ in 0..ops {
            let mut els = Vec::new();
            for &r in &roots {
                elements_under(&dom, r, &mut els);
            }
            let el = els[rng.below(els.len())];
            let id = IDS[rng.below(IDS.len())];
            let op = rng.below(20);
            log.push(op);
            match op {
                // Changes the index must survive.
                0 => dom.set_attribute(el, "data-x", &batch.to_string()).unwrap(),
                1 => dom.set_attribute(el, "class", id).unwrap(),
                2 => {
                    if let Some(t) =
                        dom.first_child(el).unwrap().filter(|&t| dom.node_type(t).unwrap() == NodeType::Text)
                    {
                        dom.set_char_data(t, &format!("t{batch}")).unwrap();
                    }
                }
                3 => {
                    dom.set_focused(d, Some(el)).unwrap();
                }
                4 => dom.set_hovered(d, Some(el)).unwrap(),
                5 => {
                    if dom.tag_name(el).unwrap() == "INPUT" {
                        dom.set_value(el, &format!("v{batch}")).unwrap();
                    }
                }
                // Changes that must rebuild it.
                6 => dom.set_attribute(el, "id", id).unwrap(),
                7 => {
                    dom.remove_attribute(el, "id").unwrap();
                }
                // setAttributeNS with the null namespace sets the element's id.
                8 => dom.set_attribute_ns(el, None, "id", id).unwrap(),
                // An `id` attribute in another namespace is not the element's id
                // (it journals the same local-name hash, so it rebuilds).
                16 => dom.set_attribute_ns(el, Some("urn:test"), "id", id).unwrap(),
                17 => dom.set_attribute_ns(el, Some("urn:test"), "x:id", id).unwrap(),
                9 => {
                    let e = dom.create_element(d, "u").unwrap();
                    dom.set_attribute(e, "id", id).unwrap();
                    let target = els[rng.below(els.len())];
                    let first = dom.first_child(target).unwrap();
                    let _ = dom.insert_before(target, e, first);
                }
                10 => {
                    // Move a subtree, possibly across roots (hierarchy errors are fine).
                    let target = els[rng.below(els.len())];
                    let _ = dom.append_child(target, el);
                }
                11 => {
                    let target = roots[rng.below(roots.len())];
                    let first = dom.first_child(target).unwrap();
                    let _ = dom.insert_before(target, el, first);
                }
                12 => {
                    if el != host {
                        let _ = dom.remove_node(el);
                    }
                }
                13 => {
                    let c = dom.clone_node(el, true, None).unwrap();
                    let target = roots[1 + rng.below(roots.len() - 1)];
                    let _ = dom.append_child(target, c);
                }
                18 => {
                    // moveBefore within one shadow-including root (errors are fine).
                    let target = els[rng.below(els.len())];
                    let _ = dom.move_before(target, el, None);
                }
                19 => {
                    // A shadow tree with ids of its own, under any element that can host one.
                    let host = match dom.shadow_root(el).unwrap() {
                        Some(sr) => Some(sr),
                        None => dom.attach_shadow(el, ShadowRootMode::Open).ok(),
                    };
                    if let Some(sr) = host {
                        let e = dom.create_element(d, "span").unwrap();
                        dom.set_attribute(e, "id", id).unwrap();
                        dom.append_child(sr, e).unwrap();
                    }
                }
                14 => {
                    let e = dom.create_element(other, "q").unwrap();
                    dom.set_attribute(e, "id", id).unwrap();
                    let e = dom.adopt_node(d, e).unwrap();
                    let _ = dom.append_child(body, e);
                }
                _ => {
                    if el != host && dom.first_child(el).unwrap().is_some() {
                        dom.set_text_content(el, "gone").unwrap();
                    }
                }
            }
        }
        check_all(&dom, &roots, &format!("batch {batch} ops {log:?}"));
    }

    // Journal overflow between lookups hides an id change: every root must
    // still see it.
    let mut els = Vec::new();
    elements_under(&dom, d, &mut els);
    let (renamed, noisy) = (els[0], els[els.len() - 1]);
    dom.set_attribute(renamed, "id", "e").unwrap();
    for i in 0..(lp_dom::dom::JOURNAL_CAPACITY * 2) {
        dom.set_attribute(noisy, "data-z", &i.to_string()).unwrap();
    }
    check_all(&dom, &roots, "journal overflow");
}

#[test]
fn a_reused_root_slot_does_not_see_the_freed_roots_ids() {
    // Freeing nodes journals nothing and creating them bumps no version, so a
    // new root can take a freed root's slot with the index still current.
    let (dom, d) = doc("<body></body>");
    let mut dom = dom.borrow_mut();
    let slot = |id: NodeId| format!("{id:?}").split("gen").next().unwrap().to_owned();
    let frag = dom.create_fragment(d).unwrap();
    let e = dom.create_element(d, "i").unwrap();
    dom.set_attribute(e, "id", "gone").unwrap();
    dom.append_child(frag, e).unwrap();
    assert_eq!(dom.get_element_by_id(frag, "gone").unwrap(), Some(e));
    let version = dom.version();
    assert_eq!(dom.collect_garbage(), 2);
    // The free list hands back the element's slot first, then the fragment's.
    let reuse_element_slot = dom.create_element(d, "b").unwrap();
    let new_root = dom.create_fragment(d).unwrap();
    assert_eq!(slot(new_root), slot(frag), "precondition: the new fragment reuses the freed root's slot");
    assert_eq!(slot(reuse_element_slot), slot(e), "precondition: the freed element's slot is reused too");
    assert_eq!(dom.version(), version, "precondition: no version change since the lookup");
    assert_eq!(dom.get_element_by_id(new_root, "gone").unwrap(), None);
    assert_eq!(dom.get_element_by_id(new_root, "gone").unwrap(), walk(&dom, new_root, "gone"));
}

#[test]
fn an_id_attribute_in_another_namespace_is_not_an_element_id() {
    let (dom, d) = doc("<body><p id=keep></p><div></div></body>");
    let mut dom = dom.borrow_mut();
    let div = dom.body(d).unwrap().unwrap();
    let div = dom.children(div).unwrap().nth(1).unwrap();
    assert_eq!(dom.get_element_by_id(d, "ns").unwrap(), None);
    dom.set_attribute_ns(div, Some("urn:test"), "id", "ns").unwrap();
    dom.set_attribute_ns(div, Some("urn:test"), "x:id", "ns2").unwrap();
    assert_eq!(dom.get_element_by_id(d, "ns").unwrap(), None);
    assert_eq!(dom.get_element_by_id(d, "ns2").unwrap(), None);
    dom.set_attribute_ns(div, None, "id", "plain").unwrap();
    assert_eq!(dom.get_element_by_id(d, "plain").unwrap(), Some(div));
    assert!(dom.get_element_by_id(d, "keep").unwrap().is_some());
}
