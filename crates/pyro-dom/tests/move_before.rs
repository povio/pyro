//! DOM "move" (`moveBefore`): pre-move validity and the state a move keeps
//! where a removal followed by an insertion would lose it.

use std::cell::RefCell;
use std::rc::Rc;

use pyro_dom::custom::CustomReaction;
use pyro_dom::iframe::IframeChange;
use pyro_dom::mutation::{ObserveInit, RecordKind};
use pyro_dom::parse::{parse_document, DomRef};
use pyro_dom::script::{ResourceKind, ResourceReason};
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

#[test]
fn a_move_needs_one_shadow_including_root_not_connectedness() {
    let (dom, d) = doc("<div id=a></div>");
    let mut dom = dom.borrow_mut();
    let a = by_id(&dom, d, "a");
    // Disconnected to disconnected, inside one tree.
    let origin = dom.create_element(d, "div").unwrap();
    let dest = dom.create_element(d, "div").unwrap();
    let p = dom.create_element(d, "p").unwrap();
    dom.append_child(origin, dest).unwrap();
    dom.append_child(origin, p).unwrap();
    dom.move_before(dest, p, None).unwrap();
    assert_eq!(dom.parent(p).unwrap(), Some(dest));
    // Across a shadow root of that disconnected tree.
    let host = dom.create_element(d, "div").unwrap();
    dom.append_child(origin, host).unwrap();
    let shadow = dom.attach_shadow(host, ShadowRootMode::Closed).unwrap();
    let inner = dom.create_element(d, "div").unwrap();
    dom.append_child(shadow, inner).unwrap();
    dom.move_before(inner, p, None).unwrap();
    assert_eq!(dom.parent(p).unwrap(), Some(inner));
    // Two disconnected trees, and connected <-> disconnected, share no root.
    let other = dom.create_element(d, "div").unwrap();
    let e = dom.move_before(other, p, None).unwrap_err();
    assert_eq!(e.kind(), Some(DomErrorKind::HierarchyRequest));
    assert_eq!(dom.move_before(a, p, None).unwrap_err().kind(), Some(DomErrorKind::HierarchyRequest));
    assert_eq!(dom.move_before(other, a, None).unwrap_err().kind(), Some(DomErrorKind::HierarchyRequest));
    // A parentless node is its own root: moving it into itself is refused.
    assert_eq!(dom.move_before(dest, origin, None).unwrap_err().kind(), Some(DomErrorKind::HierarchyRequest));
}

#[test]
fn a_move_queues_a_removal_and_an_insertion_record() {
    let (dom, d) = doc("<div id=a><i id=x></i><p id=p></p><b id=y></b></div><div id=b><u id=z></u></div>");
    let mut dom = dom.borrow_mut();
    let (a, b, p) = (by_id(&dom, d, "a"), by_id(&dom, d, "b"), by_id(&dom, d, "p"));
    let (x, y, z) = (by_id(&dom, d, "x"), by_id(&dom, d, "y"), by_id(&dom, d, "z"));
    let obs = dom.create_observer();
    for target in [a, b, p] {
        dom.observe(obs, target, ObserveInit { child_list: true, ..Default::default() }).unwrap();
    }
    dom.move_before(b, p, None).unwrap();
    let records = dom.take_records(obs);
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].kind, RecordKind::ChildList);
    assert_eq!((records[0].target, records[0].removed.clone(), records[0].added.len()), (a, vec![p], 0));
    assert_eq!((records[0].previous_sibling, records[0].next_sibling), (Some(x), Some(y)));
    assert_eq!((records[1].target, records[1].added.clone(), records[1].removed.len()), (b, vec![p], 0));
    assert_eq!((records[1].previous_sibling, records[1].next_sibling), (Some(z), None));
}

#[test]
fn a_move_keeps_focus_and_a_removal_does_not() {
    let (dom, d) = doc("<div id=a><button id=btn></button></div><div id=b></div>");
    let mut dom = dom.borrow_mut();
    let (a, b, btn) = (by_id(&dom, d, "a"), by_id(&dom, d, "b"), by_id(&dom, d, "btn"));
    dom.set_focused(d, Some(btn)).unwrap();
    dom.move_before(b, btn, None).unwrap();
    assert_eq!(dom.focused(d).unwrap(), Some(btn), "moving the focused element");
    dom.move_before(a, b, None).unwrap();
    assert_eq!(dom.focused(d).unwrap(), Some(btn), "moving an ancestor of the focused element");
    let body = d_body(&dom, d);
    dom.append_child(body, btn).unwrap();
    assert_eq!(dom.focused(d).unwrap(), None, "an insertion removes the node first");
}

fn d_body(dom: &Dom, d: NodeId) -> NodeId {
    dom.body(d).unwrap().unwrap()
}

#[test]
fn a_move_keeps_iframes_and_tells_the_loader_only_about_styles_and_images() {
    let (dom, d) = doc(
        "<div id=a><iframe id=f></iframe><script id=s></script><style id=st></style><img id=img src=x.png></div><div id=b></div>",
    );
    let mut dom = dom.borrow_mut();
    dom.set_resource_tracking(true);
    dom.watch_iframes(d).unwrap();
    for event in dom.take_iframe_events(d) {
        dom.release(event.node);
    }
    dom.take_resource_events();
    let (a, b) = (by_id(&dom, d, "a"), by_id(&dom, d, "b"));
    let (f, st, img) = (by_id(&dom, d, "f"), by_id(&dom, d, "st"), by_id(&dom, d, "img"));
    dom.move_before(b, a, None).unwrap();
    assert!(dom.take_iframe_events(d).is_empty(), "no iframe is disconnected or connected");
    let events = dom.take_resource_events();
    let seen: Vec<_> = events.iter().map(|e| (e.node, e.kind, e.reason)).collect();
    assert_eq!(
        seen,
        vec![
            (st, ResourceKind::StyleText, ResourceReason::Removed),
            (st, ResourceKind::StyleText, ResourceReason::Connected),
            (img, ResourceKind::Img, ResourceReason::Connected),
        ]
    );
    // The same subtree through removal and insertion does reach the iframe.
    let body = d_body(&dom, d);
    dom.append_child(body, a).unwrap();
    let changes: Vec<_> = dom.take_iframe_events(d).into_iter().map(|e| (e.node, e.change)).collect();
    assert!(changes.contains(&(f, IframeChange::Disconnected)) && changes.contains(&(f, IframeChange::Connected)));
    for (node, _) in changes {
        dom.release(node);
    }
}

#[test]
fn a_source_moving_in_or_out_of_a_picture_is_a_relevant_mutation_for_its_images() {
    let (dom, d) = doc("<picture id=pic><source id=src srcset=a.png><img id=img src=b.png></picture><div id=out></div>");
    let mut dom = dom.borrow_mut();
    dom.set_resource_tracking(true);
    dom.take_resource_events();
    let (pic, src, img, out) = (by_id(&dom, d, "pic"), by_id(&dom, d, "src"), by_id(&dom, d, "img"), by_id(&dom, d, "out"));
    let image_events = |dom: &mut Dom| -> Vec<NodeId> {
        dom.take_resource_events().into_iter().filter(|e| e.kind == ResourceKind::Img).map(|e| e.node).collect()
    };
    dom.move_before(out, src, None).unwrap();
    assert_eq!(image_events(&mut dom), vec![img], "moved out");
    dom.move_before(pic, src, Some(img)).unwrap();
    assert_eq!(image_events(&mut dom), vec![img], "moved in");
    dom.remove_child(pic, src).unwrap();
    assert_eq!(image_events(&mut dom), vec![img], "removed");
    dom.insert_before(pic, src, Some(img)).unwrap();
    assert_eq!(image_events(&mut dom), vec![img], "inserted");
}

#[test]
fn picture_source_selection() {
    let (dom, d) = doc(
        "<picture><source srcset='no.png' media='(min-width: 5000px)'><source srcset='t.txt' type='text/plain'>\
         <source srcset='small.png 1x, big.png 2x' media='(min-width: 10px)'><img id=in src=fallback.png><source srcset=late.png></picture>\
         <img id=out src=plain.png>",
    );
    let dom = dom.borrow();
    let viewport = media::Viewport { width: 800, height: 600, ..Default::default() };
    assert_eq!(dom.picture_source(by_id(&dom, d, "in"), &viewport).as_deref(), Some("small.png"));
    assert_eq!(dom.picture_source(by_id(&dom, d, "out"), &viewport), None);
    let retina = media::Viewport { device_scale_factor: 2.0, ..viewport.clone() };
    assert_eq!(dom.picture_source(by_id(&dom, d, "in"), &retina).as_deref(), Some("big.png"));
    // Width descriptors are densities against the viewport width.
    let widths = picture::parse_srcset("a.png 400w, b.png 1600w, data:image/png;base64,AA== 800w, bad 1q, c.png,", 800.0);
    let urls: Vec<_> = widths.iter().map(|c| (c.url.as_str(), c.density)).collect();
    assert_eq!(urls, vec![("a.png", 0.5), ("b.png", 2.0), ("data:image/png;base64,AA==", 1.0), ("c.png", 1.0)]);
    assert_eq!(picture::pick_candidate(&widths, 1.0).unwrap().url, "data:image/png;base64,AA==");
    assert_eq!(picture::pick_candidate(&widths, 3.0).unwrap().url, "b.png");
}

#[test]
fn slots_are_signaled_by_moves() {
    let (dom, d) = doc("<div id=host><p id=lit slot=content></p></div><p id=free slot=content></p>");
    let mut dom = dom.borrow_mut();
    let (host, free) = (by_id(&dom, d, "host"), by_id(&dom, d, "free"));
    let body = d_body(&dom, d);
    let shadow = dom.attach_shadow(host, ShadowRootMode::Open).unwrap();
    let slot = dom.create_element(d, "slot").unwrap();
    dom.set_attribute(slot, "name", "content").unwrap();
    dom.append_child(shadow, slot).unwrap();
    let other = dom.create_element(d, "slot").unwrap();
    dom.set_attribute(other, "name", "empty").unwrap();
    dom.append_child(shadow, other).unwrap();
    dom.take_signaled_slots();
    // Fallback content moved into and out of a slot without assigned nodes.
    let fallback = dom.create_element(d, "span").unwrap();
    dom.append_child(shadow, fallback).unwrap();
    assert!(dom.take_signaled_slots().is_empty());
    dom.move_before(other, fallback, None).unwrap();
    assert_eq!(dom.take_signaled_slots(), vec![other], "fallback moved in");
    dom.move_before(shadow, fallback, None).unwrap();
    assert_eq!(dom.take_signaled_slots(), vec![other], "fallback moved out");
    // The same for an insertion and a removal.
    dom.append_child(other, fallback).unwrap();
    assert_eq!(dom.take_signaled_slots(), vec![other], "fallback inserted");
    dom.remove_child(other, fallback).unwrap();
    assert_eq!(dom.take_signaled_slots(), vec![other], "fallback removed");
    // A slottable moved into and out of the host.
    dom.move_before(host, free, None).unwrap();
    assert_eq!(dom.take_signaled_slots(), vec![slot]);
    dom.move_before(body, free, None).unwrap();
    assert_eq!(dom.take_signaled_slots(), vec![slot]);
    // A slot moved out of its shadow tree loses its assigned nodes, and gets
    // them back when it is moved in again.
    dom.move_before(body, slot, None).unwrap();
    assert_eq!(dom.take_signaled_slots(), vec![slot], "slot moved out");
    assert!(dom.assigned_nodes(slot, false).unwrap().is_empty());
    dom.move_before(shadow, slot, None).unwrap();
    assert_eq!(dom.take_signaled_slots(), vec![slot], "slot moved in");
    assert_eq!(dom.assigned_nodes(slot, false).unwrap().len(), 1);
}

#[test]
fn custom_element_reactions_are_queued_per_document_in_tree_order() {
    let (dom, d) = doc("<div id=a><x-outer id=o><x-inner id=i></x-inner><span id=plain></span></x-outer></div><div id=b></div>");
    let mut dom = dom.borrow_mut();
    let (a, b, o, i) = (by_id(&dom, d, "a"), by_id(&dom, d, "b"), by_id(&dom, d, "o"), by_id(&dom, d, "i"));
    // Nothing is queued while no element is custom.
    dom.move_before(b, o, None).unwrap();
    assert!(!dom.has_custom_reactions());
    dom.set_custom_defined(o).unwrap();
    dom.set_custom_defined(i).unwrap();
    // A custom element in a shadow tree of the moved subtree comes before
    // the light children of its host.
    let plain = by_id(&dom, d, "plain");
    let shadow = dom.attach_shadow(plain, ShadowRootMode::Open).unwrap();
    let deep = dom.create_element(d, "x-deep").unwrap();
    dom.set_custom_defined(deep).unwrap();
    dom.append_child(shadow, deep).unwrap();
    assert_eq!(dom.take_custom_reactions(d), vec![(deep, CustomReaction::Connected)]);
    dom.move_before(a, o, None).unwrap();
    assert!(dom.has_custom_reactions_for(d));
    assert_eq!(
        dom.take_custom_reactions(d),
        vec![(o, CustomReaction::Moved), (i, CustomReaction::Moved), (deep, CustomReaction::Moved)]
    );
    assert!(!dom.has_custom_reactions());
    // Removal and insertion: disconnected, then connected.
    dom.remove_child(a, o).unwrap();
    dom.append_child(b, o).unwrap();
    let reactions: Vec<_> = dom.take_custom_reactions(d).into_iter().filter(|(n, _)| *n == o).map(|(_, r)| r).collect();
    assert_eq!(reactions, vec![CustomReaction::Disconnected, CustomReaction::Connected]);
    // A move inside a disconnected tree queues nothing.
    let (x, y) = (dom.create_element(d, "div").unwrap(), dom.create_element(d, "div").unwrap());
    dom.append_child(x, y).unwrap();
    dom.append_child(x, o).unwrap();
    dom.take_custom_reactions(d);
    dom.move_before(y, o, None).unwrap();
    assert!(!dom.has_custom_reactions());
}

#[test]
fn a_move_keeps_modal_dialogs_and_popovers_and_a_removal_closes_them() {
    let (dom, d) = doc("<div id=a><dialog id=dlg open></dialog><div id=pop popover></div></div><div id=b></div>");
    let mut dom = dom.borrow_mut();
    let (a, b, dlg, pop) = (by_id(&dom, d, "a"), by_id(&dom, d, "b"), by_id(&dom, d, "dlg"), by_id(&dom, d, "pop"));
    let modal = select::parse_selectors(":modal").unwrap();
    let open = select::parse_selectors(":popover-open").unwrap();
    assert_eq!(dom.query_selector(d, &modal).unwrap(), None);
    assert_eq!(dom.query_selector(d, &open).unwrap(), None);
    dom.set_modal(dlg, true).unwrap();
    dom.set_popover_showing(pop, true).unwrap();
    assert_eq!(dom.query_selector(d, &modal).unwrap(), Some(dlg));
    assert_eq!(dom.query_selector(d, &open).unwrap(), Some(pop));
    dom.move_before(b, dlg, None).unwrap();
    dom.move_before(b, a, None).unwrap();
    assert!(dom.is_modal(dlg) && dom.is_popover_showing(pop));
    assert_eq!(dom.query_selector(d, &modal).unwrap(), Some(dlg));
    assert_eq!(dom.query_selector(d, &open).unwrap(), Some(pop));
    // `:modal` needs the dialog to be open still.
    dom.remove_attribute(dlg, "open").unwrap();
    assert_eq!(dom.query_selector(d, &modal).unwrap(), None);
    dom.set_attribute(dlg, "open", "").unwrap();
    dom.remove_child(b, a).unwrap();
    dom.remove_child(b, dlg).unwrap();
    assert!(!dom.is_modal(dlg) && !dom.is_popover_showing(pop));
}

#[test]
fn a_default_selected_option_stays_selected_when_it_leaves_its_select() {
    let (dom, d) = doc(
        "<select id=s><option id=oa>A</option><optgroup id=g><option id=ob>B</option></optgroup><option id=oc>C</option></select>",
    );
    let mut dom = dom.borrow_mut();
    let body = d_body(&dom, d);
    let (s, oa, ob, oc, g) =
        (by_id(&dom, d, "s"), by_id(&dom, d, "oa"), by_id(&dom, d, "ob"), by_id(&dom, d, "oc"), by_id(&dom, d, "g"));
    let selected = |dom: &Dom| [oa, ob, oc].map(|o| dom.option_selected(o).unwrap());
    assert_eq!(selected(&dom), [true, false, false]);
    dom.move_before(body, oa, None).unwrap();
    assert_eq!(selected(&dom), [true, true, false]);
    dom.move_before(body, ob, None).unwrap();
    assert_eq!(selected(&dom), [true, true, true]);
    dom.move_before(s, oa, Some(oc)).unwrap();
    assert_eq!(selected(&dom), [true, true, false]);
    // A removal, and an optgroup taking its options along.
    dom.move_before(g, ob, None).unwrap();
    dom.remove_child(s, oa).unwrap();
    assert_eq!(selected(&dom), [true, true, false]);
    dom.remove_child(s, g).unwrap();
    assert_eq!(selected(&dom), [true, true, true]);
}

#[test]
fn inertness_comes_from_an_ancestor() {
    let (dom, d) = doc("<div inert><p id=in></p></div><p id=out></p>");
    let dom = dom.borrow();
    assert!(dom.is_inert(by_id(&dom, d, "in")));
    assert!(!dom.is_inert(by_id(&dom, d, "out")));
}

#[test]
fn selection_direction_belongs_to_the_selection_it_was_set_for() {
    let (dom, d) = doc("<input id=i value=0123456789>");
    let mut dom = dom.borrow_mut();
    let i = by_id(&dom, d, "i");
    assert_eq!(dom.selection_direction(i).unwrap(), None);
    dom.set_selection_range(i, 2, 7).unwrap();
    dom.set_selection_direction(i, Some(true)).unwrap();
    assert_eq!(dom.selection_direction(i).unwrap(), Some(true));
    dom.set_selection_range(i, 3, 7).unwrap();
    assert_eq!(dom.selection_direction(i).unwrap(), None, "a new selection has no direction");
    dom.set_selection_direction(i, Some(false)).unwrap();
    assert_eq!(dom.selection_direction(i).unwrap(), Some(false));
}

#[test]
fn manual_assignment_updates_layout_and_signals_slots_removed_with_ancestors() {
    let (dom, document) = doc("<div id=host><span id=child></span></div>");
    let mut dom = dom.borrow_mut();
    let host = by_id(&dom, document, "host");
    let child = by_id(&dom, document, "child");
    let shadow = dom.attach_shadow_with(host, ShadowRootMode::Open, pyro_dom::shadow_flags::MANUAL_SLOTS).unwrap();
    let wrapper = dom.create_element(document, "div").unwrap();
    let slot = dom.create_element(document, "slot").unwrap();
    dom.append_child(wrapper, slot).unwrap();
    dom.append_child(shadow, wrapper).unwrap();
    let version = dom.version();
    dom.slot_assign(slot, &[child]).unwrap();
    assert_ne!(dom.version(), version);
    assert_eq!(dom.take_signaled_slots(), [slot]);

    let version = dom.version();
    dom.slot_assign(slot, &[child, child]).unwrap();
    assert_eq!(dom.version(), version);
    assert!(dom.take_signaled_slots().is_empty());

    dom.remove_child(shadow, wrapper).unwrap();
    assert!(dom.assigned_nodes(slot, false).unwrap().is_empty());
    assert_eq!(dom.take_signaled_slots(), [slot]);
    dom.append_child(shadow, wrapper).unwrap();
    assert_eq!(dom.assigned_nodes(slot, false).unwrap(), [child]);
    assert_eq!(dom.take_signaled_slots(), [slot]);
}
