//! The top layer kept at mutation time (its order, the popover stacks, the
//! inertness a modal dialog causes) and the `<details>` steps pyro-dom runs on
//! attribute changes (queued toggles, the exclusive accordion).

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

fn by_id(dom: &Dom, d: NodeId, id: &str) -> NodeId {
    dom.get_element_by_id(d, id).unwrap().unwrap()
}

/// Many showing popovers: ranks (and the topmost modal dialog) follow
/// entering, leaving from the top and from below, and re-entering, without
/// scanning the top layer per element.
#[test]
fn many_popovers_keep_their_ranks_and_the_topmost_modal() {
    let html: String = (0..500).map(|i| format!("<div id=p{i} popover=manual></div>")).collect::<String>()
        + "<dialog id=m open></dialog>";
    let (dom, d) = doc(&html);
    let mut dom = dom.borrow_mut();
    let ps: Vec<_> = (0..500).map(|i| by_id(&dom, d, &format!("p{i}"))).collect();
    let m = by_id(&dom, d, "m");
    for &p in &ps[..250] {
        dom.set_popover_showing(p, true).unwrap();
    }
    dom.set_modal(m, true).unwrap();
    for &p in &ps[250..] {
        dom.set_popover_showing(p, true).unwrap();
    }
    assert_eq!(dom.topmost_modal(), Some(m));
    // From the top, then from below, then re-entering on top.
    dom.set_popover_showing(ps[499], false).unwrap();
    for &p in &ps[..100] {
        dom.set_popover_showing(p, false).unwrap();
    }
    dom.set_popover_showing(ps[0], true).unwrap();
    let mut expected: Vec<_> = ps[100..250].to_vec();
    expected.push(m);
    expected.extend(&ps[250..499]);
    expected.push(ps[0]);
    assert_eq!(dom.top_layer(), expected);
    assert_eq!(dom.topmost_modal(), Some(m));
    dom.set_modal(m, false).unwrap();
    assert_eq!(dom.topmost_modal(), None);
    dom.set_modal(m, true).unwrap();
    assert_eq!(dom.topmost_modal(), Some(m));
    assert_eq!(dom.top_layer().last(), Some(&m));
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
    let (dom, d) = doc("<body id=b></body>");
    let mut dom = dom.borrow_mut();
    let body = by_id(&dom, d, "b");
    dom.watch_iframes(d).unwrap();
    // Toggles of details in a document nobody watches (template contents)
    // are dropped when the page's document takes its own.
    pyro_dom::parse::set_inner_html(&mut dom, body, "<template><details open></details></template>").unwrap();
    assert!(dom.has_details_toggles());
    assert!(dom.take_details_toggles(d).is_empty());
    assert!(!dom.has_details_toggles());
    pyro_dom::parse::set_inner_html(
        &mut dom,
        body,
        "<details id=x name=g open></details><details id=y name=g></details><details id=z></details>",
    )
    .unwrap();
    let (x, y, z) = (by_id(&dom, d, "x"), by_id(&dom, d, "y"), by_id(&dom, d, "z"));
    let take = |dom: &mut Dom| {
        let t = dom.take_details_toggles(d);
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

#[test]
fn a_modal_dialog_in_a_shadow_tree_does_not_block_its_hosts() {
    let (dom, d) = doc("<div id=host><span id=light></span></div><p id=other></p>");
    let mut dom = dom.borrow_mut();
    let (host, light, other) = (by_id(&dom, d, "host"), by_id(&dom, d, "light"), by_id(&dom, d, "other"));
    let root = dom.attach_shadow(host, ShadowRootMode::Open).unwrap();
    let dialog = dom.create_element(d, "dialog").unwrap();
    dom.append_child(root, dialog).unwrap();
    assert!(!dom.blocked_by_modal(other));
    dom.set_modal(dialog, true).unwrap();
    let html = dom.parent(dom.parent(host).unwrap().unwrap()).unwrap().unwrap();
    assert!(!dom.blocked_by_modal(host) && !dom.blocked_by_modal(html) && !dom.blocked_by_modal(dialog));
    assert!(dom.blocked_by_modal(other) && dom.blocked_by_modal(light));
}

/// `:hover` and `:focus-within` stop at the innermost top-layer element: it matches, its
/// ancestors outside it do not, and they match again once it leaves the top layer (Chrome; WPT
/// css/selectors `hover-toplayer-001`, `focus-within-toplayer-001`, `toplayer-transition-001`).
#[test]
fn user_action_pseudo_classes_stop_at_the_top_layer() {
    // A no-quirks document: in quirks mode a bare `:hover` matches links only (the
    // :active and :hover quirk).
    let (dom, d) = doc(
        "<!doctype html><div id=outer><dialog id=dlg><div id=pop popover=manual><input id=inp></div></dialog></div>",
    );
    let mut dom = dom.borrow_mut();
    let (outer, dlg, pop, inp) =
        (by_id(&dom, d, "outer"), by_id(&dom, d, "dlg"), by_id(&dom, d, "pop"), by_id(&dom, d, "inp"));
    let m = |dom: &Dom, el: NodeId, s: &str| dom.matches(el, &pyro_dom::select::parse_selectors(s).unwrap()).unwrap();
    dom.set_focused(d, Some(inp)).unwrap();
    dom.set_hovered(d, Some(inp)).unwrap();
    assert!(m(&dom, outer, ":focus-within") && m(&dom, outer, ":hover"));
    dom.set_modal(dlg, true).unwrap();
    assert!(m(&dom, dlg, ":focus-within") && m(&dom, dlg, ":hover"));
    assert!(!m(&dom, outer, ":focus-within") && !m(&dom, outer, ":hover"));
    dom.set_popover_showing(pop, true).unwrap();
    assert!(m(&dom, pop, ":focus-within") && m(&dom, pop, ":hover"));
    assert!(!m(&dom, dlg, ":focus-within") && !m(&dom, dlg, ":hover"), "the innermost top-layer element ends the chain");
    dom.set_popover_showing(pop, false).unwrap();
    dom.set_modal(dlg, false).unwrap();
    assert!(m(&dom, outer, ":focus-within") && m(&dom, outer, ":hover"));
}
