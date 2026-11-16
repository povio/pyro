//! What the CSS animation and transition events rest on in pyro-dom: the legacy
//! event types of DOM "invoke" (a trusted `animationend` nobody listens for
//! is retried as `webkitAnimationEnd`), and the author's animation and
//! transition declarations read from the sheets (`pyro_dom::timing`).

use std::cell::RefCell;
use std::rc::Rc;

use pyro_dom::events::{dispatch, AddOptions, EventState, ListenerHandle, ListenerInvoker, TargetKey};
use pyro_dom::media::Viewport;
use pyro_dom::parse::{parse_document, DomRef};
use pyro_dom::timing::TimingRules;
use pyro_dom::{Dom, NodeId};

fn doc(html: &str) -> (DomRef, NodeId) {
    let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
    let d = parse_document(&dom, "http://test/", html);
    (dom, d)
}

/// Records each invoked listener with the type the event reported meanwhile.
struct Recorder {
    log: Vec<String>,
    event: Rc<RefCell<EventState>>,
}
impl ListenerInvoker for Recorder {
    fn invoke(&mut self, cb: ListenerHandle, _current: TargetKey, _target: TargetKey) {
        self.log.push(format!("{}:{}", cb.0, self.event.borrow().ty));
    }
}

fn fire(dom: &DomRef, target: TargetKey, ty: &str, trusted: bool) -> Vec<String> {
    let mut state = EventState::new(ty, true, false, false);
    state.is_trusted = trusted;
    let event = Rc::new(RefCell::new(state));
    let mut recorder = Recorder { log: Vec::new(), event: event.clone() };
    dispatch(dom, target, &event, &mut recorder);
    assert_eq!(&*event.borrow().ty, ty, "the type is restored");
    recorder.log
}

#[test]
fn a_trusted_event_without_listeners_is_retried_under_its_legacy_type() {
    let (dom, d) = doc("<div id=parent><span id=child></span></div>");
    let options = AddOptions { capture: false, once: false, passive: None };
    let (parent, child) = {
        let mut dm = dom.borrow_mut();
        let parent = dm.get_element_by_id(d, "parent").unwrap().unwrap();
        let child = dm.get_element_by_id(d, "child").unwrap().unwrap();
        let (parent, child) = (dm.node_key(parent).unwrap(), dm.node_key(child).unwrap());
        assert!(!dm.has_animation_listeners());
        dm.add_listener(child, "webkitAnimationEnd", ListenerHandle(1), options);
        // Not the legacy type: the names are case-sensitive.
        dm.add_listener(child, "webkitanimationend", ListenerHandle(2), options);
        dm.add_listener(parent, "animationend", ListenerHandle(3), options);
        dm.add_listener(parent, "webkitAnimationEnd", ListenerHandle(4), options);
        assert!(dm.has_animation_listeners());
        (parent, child)
    };
    // The child has no `animationend` listener: its legacy one runs and sees
    // the legacy type. The parent has one, so its legacy listener stays out.
    assert_eq!(fire(&dom, child, "animationend", true), ["1:webkitAnimationEnd", "3:animationend"]);
    // Script-dispatched events are never rewritten.
    assert_eq!(fire(&dom, child, "animationend", false), ["3:animationend"]);
    // The legacy type itself is an ordinary type.
    assert_eq!(fire(&dom, child, "webkitAnimationEnd", false), ["1:webkitAnimationEnd", "4:webkitAnimationEnd"]);
    // The other three pairs.
    for (ty, legacy) in [
        ("animationstart", "webkitAnimationStart"),
        ("animationiteration", "webkitAnimationIteration"),
        ("transitionend", "webkitTransitionEnd"),
    ] {
        dom.borrow_mut().add_listener(parent, legacy, ListenerHandle(9), options);
        assert_eq!(fire(&dom, parent, ty, true), [format!("9:{legacy}")]);
        dom.borrow_mut().remove_listener(parent, legacy, ListenerHandle(9), false);
    }
    // No other type has a legacy alias.
    dom.borrow_mut().add_listener(parent, "webkitTransitionRun", ListenerHandle(9), options);
    assert!(fire(&dom, parent, "transitionrun", true).is_empty());
}

#[test]
fn animation_and_transition_declarations_cascade_from_rules_and_the_style_attribute() {
    let (dom, d) = doc(
        "<div id=a class='base spin'></div><div id=b class=base style='animation: fade 2s 3, missing 1s'></div>\
         <div id=c class='base off'></div><p id=d style='transition: color 30ms'></p>",
    );
    let sheet = "@keyframes spin {} @-webkit-keyframes fade { from {} to {} }
        .base { transition: width 1ms, height .5s 1s; }
        .spin { animation-name: spin; animation-duration: 100s; animation-delay: -99.9s; animation-iteration-count: infinite }
        @media (min-width: 100000px) { .base { animation: spin 1s } }
        @supports (display: grid) { .off { transition: none !important; } }
        #c { transition-duration: 5s }";
    let rules = TimingRules::parse([sheet].into_iter(), &Viewport::default());
    assert!(rules.has_keyframes("spin") && rules.has_keyframes("fade") && !rules.has_keyframes("missing"));
    let dm = dom.borrow();
    let el = |id: &str| dm.get_element_by_id(d, id).unwrap().unwrap();

    let a = rules.element(&dm, el("a"), &|_| None);
    assert_eq!(a.animations.len(), 1);
    let spin = &a.animations[0];
    assert_eq!((spin.name.as_str(), spin.duration_ms, spin.delay_ms), ("spin", 100_000.0, -99_900.0));
    assert!(spin.iterations.is_infinite());
    let transitions: Vec<_> = a.transitions.iter().map(|t| (t.property.as_str(), t.duration_ms, t.delay_ms)).collect();
    assert_eq!(transitions, [("width", 1.0, 0.0), ("height", 500.0, 1000.0)]);

    // The style attribute's shorthand; an animation without `@keyframes` does not run.
    let b = rules.element(&dm, el("b"), &|_| None);
    let animations: Vec<_> = b.animations.iter().map(|x| (x.name.as_str(), x.duration_ms, x.iterations)).collect();
    assert_eq!(animations, [("fade", 2000.0, 3.0)]);

    // `!important` beats the more specific rule: no transition.
    assert!(rules.element(&dm, el("c"), &|_| None).transitions.is_empty());

    let p = rules.element(&dm, el("d"), &|_| None);
    assert_eq!(p.transitions.len(), 1);
    assert_eq!((p.transitions[0].property.as_str(), p.transitions[0].duration_ms), ("color", 30.0));

    // Candidates: elements a rule naming an animation matches, and inline ones.
    let mut candidates = rules.animation_candidates(&dm, d);
    candidates.sort();
    let mut expected = vec![el("a"), el("b")];
    expected.sort();
    assert_eq!(candidates, expected);
}

#[test]
fn transition_listeners_are_noted_apart_from_animation_ones() {
    let (dom, d) = doc("<div id=a></div>");
    let options = AddOptions { capture: false, once: false, passive: None };
    let mut dm = dom.borrow_mut();
    let a = dm.get_element_by_id(d, "a").unwrap().unwrap();
    let a = dm.node_key(a).unwrap();
    dm.add_listener(a, "animationend", ListenerHandle(1), options);
    assert!(dm.has_animation_listeners() && !dm.has_transition_listeners());
    dm.add_listener(a, "webkitTransitionEnd", ListenerHandle(2), options);
    assert!(dm.has_transition_listeners());
}
