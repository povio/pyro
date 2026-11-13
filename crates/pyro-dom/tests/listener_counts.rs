//! `Dom::listens_for`: the per-type listener count a dispatch nobody can
//! hear is told apart by. It follows every way a listener comes and goes
//! (add, duplicate add, remove, `once`, an AbortSignal's removal, a target
//! erased or forgotten), counts every kind of target, and an unheard
//! dispatch leaves the event as a heard one that ran nothing would.

use std::cell::RefCell;
use std::rc::Rc;

use pyro_dom::events::{dispatch, AddOptions, EventState, ListenerHandle, ListenerInvoker, TargetKey};
use pyro_dom::parse::{parse_document, DomRef};
use pyro_dom::*;

const BUBBLE: AddOptions = AddOptions { capture: false, once: false, passive: None };
const CAPTURE: AddOptions = AddOptions { capture: true, once: false, passive: None };
const ONCE: AddOptions = AddOptions { capture: false, once: true, passive: None };

struct Tree {
    dom: DomRef,
    div: TargetKey,
    inner: TargetKey,
    document: TargetKey,
    window: TargetKey,
}

fn tree() -> Tree {
    let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
    let d = parse_document(&dom, "http://test/", "<body><div id=a></div><div id=host></div></body>");
    let (div, inner, document, window) = {
        let mut dm = dom.borrow_mut();
        dm.set_document_active(d, true).unwrap();
        let div = dm.get_element_by_id(d, "a").unwrap().unwrap();
        let host = dm.get_element_by_id(d, "host").unwrap().unwrap();
        let shadow = dm.attach_shadow(host, ShadowRootMode::Closed).unwrap();
        let inner = dm.create_element(d, "b").unwrap();
        dm.append_child(shadow, inner).unwrap();
        let window = dm.new_target_key();
        dm.set_document_window(d, Some(window));
        (dm.node_key(div).unwrap(), dm.node_key(inner).unwrap(), dm.node_key(d).unwrap(), window)
    };
    Tree { dom, div, inner, document, window }
}

#[derive(Default)]
struct Calls(Vec<(u64, TargetKey)>);
impl ListenerInvoker for Calls {
    fn invoke(&mut self, cb: ListenerHandle, current: TargetKey, _target: TargetKey) {
        self.0.push((cb.0, current));
    }
}

fn fire(t: &Tree, target: TargetKey, ty: &str) -> Vec<(u64, TargetKey)> {
    let event = RefCell::new(EventState::new(ty, true, true, true));
    let mut calls = Calls::default();
    dispatch(&t.dom, target, &event, &mut calls);
    calls.0
}

#[test]
fn counts_follow_add_and_remove_on_every_kind_of_target() {
    let t = tree();
    let heard = |ty: &str| t.dom.borrow().listens_for(ty);
    assert!(!heard("mousedown"));
    // Each kind of target counts: node, shadow tree node, document, window,
    // a target outside any tree; capture as well as bubble.
    for (n, (target, opts)) in
        [(t.div, BUBBLE), (t.inner, BUBBLE), (t.document, CAPTURE), (t.window, BUBBLE)].into_iter().enumerate()
    {
        assert!(t.dom.borrow_mut().add_listener(target, "mousedown", ListenerHandle(n as u64), opts));
        assert!(heard("mousedown"));
        assert!(!heard("mouseup"));
        assert!(t.dom.borrow_mut().remove_listener(target, "mousedown", ListenerHandle(n as u64), opts.capture).is_some());
        assert!(!heard("mousedown"), "target {n}");
    }
    let other = t.dom.borrow_mut().new_target_key();
    t.dom.borrow_mut().add_listener(other, "message", ListenerHandle(9), BUBBLE);
    assert!(heard("message"));
    t.dom.borrow_mut().erase_listeners(other);
    assert!(!heard("message"));

    // A duplicate registration is not a second listener.
    assert!(t.dom.borrow_mut().add_listener(t.div, "click", ListenerHandle(1), BUBBLE));
    assert!(!t.dom.borrow_mut().add_listener(t.div, "click", ListenerHandle(1), BUBBLE));
    t.dom.borrow_mut().remove_listener(t.div, "click", ListenerHandle(1), false);
    assert!(!heard("click"));
    // Removing what was never added changes nothing.
    t.dom.borrow_mut().add_listener(t.div, "click", ListenerHandle(1), BUBBLE);
    assert!(t.dom.borrow_mut().remove_listener(t.div, "click", ListenerHandle(1), true).is_none());
    assert!(t.dom.borrow_mut().remove_listener(t.window, "click", ListenerHandle(1), false).is_none());
    assert!(heard("click"));

    // Two listeners for one type on two targets: the type is heard until
    // both are gone.
    t.dom.borrow_mut().add_listener(t.window, "click", ListenerHandle(2), CAPTURE);
    t.dom.borrow_mut().remove_listener(t.div, "click", ListenerHandle(1), false);
    assert!(heard("click"));
    assert_eq!(fire(&t, t.div, "click"), [(2, t.window)]);
    t.dom.borrow_mut().remove_listener(t.window, "click", ListenerHandle(2), true);
    assert!(!heard("click"));
    assert_eq!(fire(&t, t.div, "click"), []);
}

#[test]
fn once_abort_and_erased_targets_take_their_listeners_off_the_count() {
    let t = tree();
    let heard = |ty: &str| t.dom.borrow().listens_for(ty);
    // `once`: gone after its one call.
    t.dom.borrow_mut().add_listener(t.window, "mouseup", ListenerHandle(1), ONCE);
    assert!(heard("mouseup"));
    assert_eq!(fire(&t, t.div, "mouseup"), [(1, t.window)]);
    assert!(!heard("mouseup"));
    assert_eq!(fire(&t, t.div, "mouseup"), []);

    // An AbortSignal removes one registration.
    t.dom.borrow_mut().add_listener(t.div, "keydown", ListenerHandle(2), BUBBLE);
    t.dom.borrow_mut().add_listener(t.div, "keydown", ListenerHandle(3), CAPTURE);
    let registration = t.dom.borrow().listener_registration(t.div, "keydown", ListenerHandle(2), false).unwrap();
    t.dom.borrow_mut().abort_listener(t.div, registration);
    assert!(heard("keydown"));
    t.dom.borrow_mut().abort_listener(t.div, registration);
    assert!(heard("keydown"));
    let registration = t.dom.borrow().listener_registration(t.div, "keydown", ListenerHandle(3), true).unwrap();
    t.dom.borrow_mut().abort_listener(t.div, registration);
    assert!(!heard("keydown"));

    // A target erased with listeners of several types.
    t.dom.borrow_mut().add_listener(t.inner, "pointerdown", ListenerHandle(4), BUBBLE);
    t.dom.borrow_mut().add_listener(t.inner, "pointerup", ListenerHandle(5), BUBBLE);
    t.dom.borrow_mut().add_listener(t.div, "pointerup", ListenerHandle(6), BUBBLE);
    t.dom.borrow_mut().erase_listeners(t.inner);
    assert!(!heard("pointerdown"));
    assert!(heard("pointerup"));
    t.dom.borrow_mut().erase_listeners(t.div);
    assert!(!heard("pointerup"));
}

#[test]
fn a_legacy_type_listener_hears_the_modern_event() {
    let t = tree();
    let heard = |ty: &str| t.dom.borrow().listens_for(ty);
    t.dom.borrow_mut().add_listener(t.div, "webkitAnimationEnd", ListenerHandle(1), BUBBLE);
    assert!(heard("animationend"));
    assert!(heard("webkitAnimationEnd"));
    assert!(!heard("animationstart"));
    let event = RefCell::new({
        let mut state = EventState::new("animationend", true, false, false);
        state.is_trusted = true;
        state
    });
    let mut calls = Calls::default();
    dispatch(&t.dom, t.div, &event, &mut calls);
    assert_eq!(calls.0, [(1, t.div)]);
    assert_eq!(&*event.borrow().ty, "animationend");
}

#[test]
fn an_unheard_dispatch_leaves_the_event_as_a_heard_one_with_no_matching_listener() {
    // The same dispatch twice: with no listener for the type anywhere, and
    // with one on a target the event's path does not cross.
    let state_after = |with_listener: bool, in_shadow: bool| {
        let t = tree();
        if with_listener {
            let other = t.dom.borrow_mut().new_target_key();
            t.dom.borrow_mut().add_listener(other, "mousedown", ListenerHandle(1), BUBBLE);
        }
        assert_eq!(t.dom.borrow().listens_for("mousedown"), with_listener);
        let mut state = EventState::new("mousedown", true, true, true);
        state.related_target = Some(t.div);
        state.stop_propagation = true;
        let event = RefCell::new(state);
        let mut calls = Calls::default();
        let target = if in_shadow { t.inner } else { t.div };
        let not_canceled = dispatch(&t.dom, target, &event, &mut calls);
        assert!(calls.0.is_empty());
        let ev = event.borrow();
        let window_is_last = ev.target.is_some();
        format!(
            "{not_canceled} {:?} {:?} {:?} {} {} {} {} {window_is_last}",
            ev.target.map(|k| k == target),
            ev.related_target.map(|k| k == t.div),
            ev.current_target,
            ev.dispatching,
            ev.stop_propagation,
            ev.stop_immediate,
            ev.path.len(),
        )
    };
    for in_shadow in [false, true] {
        assert_eq!(state_after(false, in_shadow), state_after(true, in_shadow), "in shadow: {in_shadow}");
    }
}
