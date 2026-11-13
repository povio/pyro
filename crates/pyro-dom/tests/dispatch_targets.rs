//! DOM "dispatch": the event path's relatedTarget retargeting, the reset of
//! targets that point into a shadow tree ("clearTargets"), the hook the
//! bindings pick the activation target in, and the `submit`/`reset` events of
//! nested forms (dom/events/relatedTarget.window.js, shadow-relatedTarget,
//! Event-dispatch-single-activation-behavior).

use std::cell::RefCell;
use std::rc::Rc;

use pyro_dom::events::{dispatch, dispatch_with, AddOptions, EventState, ListenerHandle, ListenerInvoker, TargetKey};
use pyro_dom::parse::{parse_document, DomRef};
use pyro_dom::*;

fn doc(html: &str) -> (DomRef, NodeId) {
    let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
    let d = parse_document(&dom, "http://test/", html);
    dom.borrow_mut().set_document_active(d, true).unwrap();
    (dom, d)
}

const BUBBLE: AddOptions = AddOptions { capture: false, once: false, passive: None };

/// Records `(listener, current target, target, relatedTarget)` per call and
/// runs `during` (which may move nodes) on the first one.
struct Recorder<'a> {
    event: Rc<RefCell<EventState>>,
    seen: Vec<(u64, TargetKey, Option<TargetKey>, Option<TargetKey>)>,
    during: Option<Box<dyn FnOnce() + 'a>>,
}
impl ListenerInvoker for Recorder<'_> {
    fn invoke(&mut self, cb: ListenerHandle, current: TargetKey, _target: TargetKey) {
        let (target, related) = {
            let ev = self.event.borrow();
            (ev.target, ev.related_target)
        };
        self.seen.push((cb.0, current, target, related));
        if let Some(during) = self.during.take() {
            during();
        }
    }
}

fn event(ty: &str, bubbles: bool, related: Option<TargetKey>) -> Rc<RefCell<EventState>> {
    let mut state = EventState::new(ty, bubbles, true, false);
    state.related_target = related;
    Rc::new(RefCell::new(state))
}

/// `<div id=host>` with a closed shadow root holding `<b>`, and `<i id=light>`.
struct Tree {
    dom: DomRef,
    host: TargetKey,
    shadow: TargetKey,
    inner: TargetKey,
    inner_id: NodeId,
    light: TargetKey,
    body: NodeId,
    window: TargetKey,
}

fn tree() -> Tree {
    let (dom, d) = doc("<body><div id=host></div><i id=light></i></body>");
    let (host, shadow, inner, inner_id, light, body, window) = {
        let mut dm = dom.borrow_mut();
        let host = dm.get_element_by_id(d, "host").unwrap().unwrap();
        let light = dm.get_element_by_id(d, "light").unwrap().unwrap();
        let shadow = dm.attach_shadow(host, ShadowRootMode::Closed).unwrap();
        let inner = dm.create_element(d, "b").unwrap();
        dm.append_child(shadow, inner).unwrap();
        let window = dm.new_target_key();
        dm.set_document_window(d, Some(window));
        let body = dm.body(d).unwrap().unwrap();
        let key = |n| dm.node_key(n).unwrap();
        (key(host), key(shadow), key(inner), inner, key(light), body, window)
    };
    Tree { dom, host, shadow, inner, inner_id, light, body, window }
}

#[test]
fn related_target_is_retargeted_for_each_invocation_target() {
    let t = tree();
    {
        let mut dm = t.dom.borrow_mut();
        dm.add_listener(t.light, "focus", ListenerHandle(1), BUBBLE);
        dm.add_listener(t.window, "focus", ListenerHandle(2), AddOptions { capture: true, ..BUBBLE });
    }
    // Focus moves from the shadow tree's node to a light-tree one: listeners
    // outside the shadow tree see the host, never the node inside it.
    let ev = event("focus", false, Some(t.inner));
    let mut rec = Recorder { event: ev.clone(), seen: Vec::new(), during: None };
    assert!(dispatch(&t.dom, t.light, &ev, &mut rec));
    assert_eq!(
        rec.seen,
        vec![(2, t.window, Some(t.light), Some(t.host)), (1, t.light, Some(t.light), Some(t.host))]
    );
    // The event keeps what the outermost invocation target saw.
    assert_eq!((ev.borrow().target, ev.borrow().related_target), (Some(t.light), Some(t.host)));

    // A relatedTarget that is not a node is never retargeted.
    let other = t.dom.borrow_mut().new_target_key();
    let ev = event("focus", false, Some(other));
    let mut rec = Recorder { event: ev.clone(), seen: Vec::new(), during: None };
    dispatch(&t.dom, t.light, &ev, &mut rec);
    assert_eq!(rec.seen[1], (1, t.light, Some(t.light), Some(other)));
    assert_eq!(ev.borrow().related_target, Some(other));
}

#[test]
fn targets_in_a_shadow_tree_are_reset_after_dispatch() {
    let t = tree();
    // The target is a shadow root, or a node of one: both targets are reset,
    // whatever the relatedTarget is.
    for target in [t.shadow, t.inner] {
        for related in [t.window, t.host, t.light] {
            let ev = event("demo", false, Some(related));
            let mut rec = Recorder { event: ev.clone(), seen: Vec::new(), during: None };
            dispatch(&t.dom, target, &ev, &mut rec);
            assert_eq!((ev.borrow().target, ev.borrow().related_target), (None, None), "{target:?} {related:?}");
        }
    }
    // Decided when the path is built: a listener that moves the target out of
    // the shadow tree does not keep it exposed.
    t.dom.borrow_mut().add_listener(t.inner, "demo", ListenerHandle(1), BUBBLE);
    let ev = event("demo", false, Some(t.window));
    let (dom, inner, body) = (t.dom.clone(), t.inner_id, t.body);
    let during = Box::new(move || dom.borrow_mut().append_child(body, inner).map(|_| ()).unwrap());
    let mut rec = Recorder { event: ev.clone(), seen: Vec::new(), during: Some(during) };
    dispatch(&t.dom, t.inner, &ev, &mut rec);
    assert_eq!(rec.seen.len(), 1);
    assert_eq!(t.dom.borrow().parent(t.inner_id).unwrap(), Some(t.body));
    assert_eq!((ev.borrow().target, ev.borrow().related_target), (None, None));
}

#[test]
fn related_target_retargeting_is_fixed_before_listeners_run() {
    let t = tree();
    let body = t.dom.borrow().node_key(t.body).unwrap();
    t.dom.borrow_mut().add_listener(body, "demo", ListenerHandle(1), BUBBLE);
    let ev = event("demo", false, Some(t.inner));
    let (dom, inner, body_id) = (t.dom.clone(), t.inner_id, t.body);
    let during = Box::new(move || dom.borrow_mut().append_child(body_id, inner).map(|_| ()).unwrap());
    let mut rec = Recorder { event: ev.clone(), seen: Vec::new(), during: Some(during) };
    dispatch(&t.dom, body, &ev, &mut rec);
    assert_eq!(rec.seen, vec![(1, body, Some(body), Some(t.host))]);
    assert_eq!((ev.borrow().target, ev.borrow().related_target), (Some(body), Some(t.host)));
}

#[test]
fn an_event_whose_related_target_retargets_to_its_target_is_not_dispatched() {
    let t = tree();
    t.dom.borrow_mut().add_listener(t.host, "heya", ListenerHandle(1), BUBBLE);
    let ev = event("heya", false, Some(t.shadow));
    ev.borrow_mut().prevent_default();
    let mut rec = Recorder { event: ev.clone(), seen: Vec::new(), during: None };
    let mut hook_calls = 0;
    assert!(!dispatch_with(&t.dom, t.host, &ev, &mut rec, &mut |_| hook_calls += 1), "the canceled flag is reported");
    assert!(rec.seen.is_empty());
    assert_eq!(hook_calls, 0);
    let state = ev.borrow();
    assert_eq!((state.target, state.related_target), (None, None));
    assert!(!state.dispatching);

    // An event whose relatedTarget is its target is dispatched as usual.
    drop(state);
    let ev = event("heya", false, Some(t.host));
    let mut rec = Recorder { event: ev.clone(), seen: Vec::new(), during: None };
    assert!(dispatch(&t.dom, t.host, &ev, &mut rec));
    assert_eq!(rec.seen, vec![(1, t.host, Some(t.host), Some(t.host))]);
}

#[test]
fn the_path_ends_below_a_host_that_is_the_related_target() {
    let t = tree();
    {
        let mut dm = t.dom.borrow_mut();
        dm.add_listener(t.inner, "over", ListenerHandle(1), BUBBLE);
        dm.add_listener(t.host, "over", ListenerHandle(2), BUBBLE);
    }
    // A composed event moving from the host into its shadow tree stays in
    // the shadow tree: outside it, target and relatedTarget would be one.
    let mut state = EventState::new("over", true, false, true);
    state.related_target = Some(t.host);
    let ev = Rc::new(RefCell::new(state));
    let mut rec = Recorder { event: ev.clone(), seen: Vec::new(), during: None };
    let mut path = Vec::new();
    dispatch_with(&t.dom, t.inner, &ev, &mut rec, &mut |items| {
        path = items.iter().map(|item| (item.invocation_target, item.at_target())).collect();
    });
    assert_eq!(path, vec![(t.inner, true), (t.shadow, false)]);
    assert_eq!(rec.seen, vec![(1, t.inner, Some(t.inner), Some(t.host))]);
}

#[test]
fn the_path_hook_sees_shadow_hosts_as_targets() {
    let t = tree();
    let ev = Rc::new(RefCell::new(EventState::new("click", false, true, true)));
    let mut rec = Recorder { event: ev.clone(), seen: Vec::new(), during: None };
    let mut path = Vec::new();
    dispatch_with(&t.dom, t.inner, &ev, &mut rec, &mut |items| {
        path = items.iter().map(|item| (item.invocation_target, item.target, item.at_target())).collect();
    });
    assert_eq!(path[0], (t.inner, t.inner, true));
    assert_eq!(path[1], (t.shadow, t.inner, false));
    assert_eq!(path[2], (t.host, t.host, true), "retargeted: the host is a target of the event");
    assert_eq!(path.last().unwrap().0, t.window);
    assert!(!path.last().unwrap().2);
}

#[test]
fn submit_and_reset_of_a_nested_form_stop_at_the_outer_form() {
    let (dom, d) = doc("<form id=outer><div id=mid><span id=leaf></span></div></form>");
    let (outer, mid, inner, leaf, document) = {
        let mut dm = dom.borrow_mut();
        let mid_id = dm.get_element_by_id(d, "mid").unwrap().unwrap();
        // The parser never nests forms; script can.
        let inner_id = dm.create_element(d, "form").unwrap();
        dm.append_child(mid_id, inner_id).unwrap();
        let key = |dm: &Dom, id: &str| dm.node_key(dm.get_element_by_id(d, id).unwrap().unwrap()).unwrap();
        (key(&dm, "outer"), key(&dm, "mid"), dm.node_key(inner_id).unwrap(), key(&dm, "leaf"), dm.node_key(d).unwrap())
    };
    for ty in ["submit", "reset", "change"] {
        let mut dm = dom.borrow_mut();
        dm.add_listener(inner, ty, ListenerHandle(1), BUBBLE);
        dm.add_listener(mid, ty, ListenerHandle(2), BUBBLE);
        dm.add_listener(outer, ty, ListenerHandle(3), BUBBLE);
        dm.add_listener(outer, ty, ListenerHandle(4), AddOptions { capture: true, ..BUBBLE });
        dm.add_listener(document, ty, ListenerHandle(5), BUBBLE);
        dm.add_listener(leaf, ty, ListenerHandle(6), BUBBLE);
    }
    let run = |ty: &str, target: TargetKey| {
        let ev = event(ty, true, None);
        let mut rec = Recorder { event: ev.clone(), seen: Vec::new(), during: None };
        dispatch(&dom, target, &ev, &mut rec);
        assert!(!ev.borrow().stop_propagation, "the flag is unset after dispatch");
        rec.seen.iter().map(|call| call.0).collect::<Vec<_>>()
    };
    // The outer form's capturing listener runs; its bubbling one and the
    // document's do not.
    assert_eq!(run("submit", inner), vec![4, 1, 2]);
    assert_eq!(run("reset", inner), vec![4, 1, 2]);
    // A form's own event reaches the document.
    assert_eq!(run("submit", outer), vec![4, 3, 5]);
    // So does a script's `submit` at some descendant up to the form, and no further.
    assert_eq!(run("submit", leaf), vec![4, 6, 2]);
    // Other events bubble through forms.
    assert_eq!(run("change", inner), vec![4, 1, 2, 3, 5]);
}
