//! CSS animation and transition events: `animationstart`,
//! `animationiteration`, `animationend`, `animationcancel`, `transitionrun`,
//! `transitionstart`, `transitionend` and `transitioncancel`.
//!
//! The layout engine settles every animation and transition at its end
//! state, so nothing is interpolated: this module only keeps the timeline
//! the events follow. It works when a listener for one of the events (or a
//! legacy `webkit*` type) was ever added, since without one nothing can
//! observe them.
//!
//! Animations: after a task changed the document, at the next frame, the
//! elements whose style names an `@keyframes` rule (`lp_dom::timing`) are
//! compared with the animations already known. A new one fires
//! `animationstart` after its delay, `animationiteration` at each iteration
//! boundary still ahead and `animationend` after its last iteration; one
//! whose name left the style before that fires `animationcancel`. An
//! `infinite` animation fires `animationstart` only.
//!
//! Transitions need a before-change style. Once a transition listener
//! exists, every style flush script forces (`getComputedStyle`, `offset*`,
//! `client*`, `scroll*`, `getBoundingClientRect`, `getClientRects`) and each
//! frame after a change is a style change event of the elements that may
//! transition (a rule or their `style` attribute declares a transition, see
//! `TimingRules::transition_candidates`): their style then becomes their
//! before-change style. A property of an element's transition list whose
//! computed value differs from the one at its previous style change
//! transitions: `transitionrun`, then `transitionstart` after the delay and
//! `transitionend` after the duration. So the usual
//! `el.offsetHeight; el.classList.add('open')` starts a transition, as does
//! a change made in a task after the element was first rendered.
//!
//! Once script called `getAnimations()` the timelines are kept the same way
//! without listeners, and `getAnimations()` lists them as `CSSAnimation` and
//! `CSSTransition` objects ([`runs`]).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use lp_dom::timing::{ElementTiming, StyleSnapshot, TimingRules};
use lp_dom::NodeId;
use rustc_hash::FxHashMap;

use crate::event::{EventCore, EventInterface};
use crate::page::PageDom;

/// How long after a change the document's animations are looked at again.
const FRAME: Duration = Duration::from_millis(16);
/// Iteration events one animation may schedule.
const MAX_ITERATIONS: usize = 1024;
/// Elements whose before-change style is kept.
const MAX_OBSERVED: usize = 1024;

type Stamp = (u64, u64, u32, u32);

/// An animation or transition whose events are (or were) scheduled.
struct Timeline {
    /// Its identity for `getAnimations()`.
    id: u64,
    /// The page clock when it started (its style change), in ms.
    start_ms: f64,
    delay_ms: f64,
    duration_ms: f64,
    /// 1 for a transition.
    iterations: f64,
    el: NodeId,
    /// The animation's name, or the transitioning longhand.
    name: String,
    /// Cleared to cancel the events not fired yet.
    live: Rc<Cell<bool>>,
    /// Set once its `end` event fired.
    ended: Rc<Cell<bool>>,
    /// An animation's timing as declared: the same name with another timing
    /// is another run (a finished `spin 2ms` restyled to `spin 1s infinite`
    /// starts again). `None` for transitions.
    timing: Option<lp_dom::timing::AnimationTiming>,
}

/// Per-document state (`PageDom::animation_events`).
#[derive(Default)]
pub(crate) struct State {
    /// The sheets' key and the rules read from them.
    rules: RefCell<Option<(lp_dom::style::AuthorSheetsKey, Rc<TimingRules>)>>,
    /// The style stamp the rules' key was last computed at.
    rules_stamp: Cell<Option<Stamp>>,
    /// The style stamp of the last frame update.
    stamp: Cell<Option<Stamp>>,
    queued: Cell<bool>,
    animations: RefCell<Vec<Timeline>>,
    transitions: RefCell<Vec<Timeline>>,
    /// Elements that may transition (and those script read the computed
    /// style of): their style at the last style change, and the stamp it was
    /// taken at.
    observed: RefCell<FxHashMap<NodeId, (StyleSnapshot, Stamp)>>,
    /// The style stamp of the last style change event of every element that
    /// may transition.
    flushed: Cell<Option<Stamp>>,
    /// Script called `getAnimations()`: timelines are kept without listeners.
    listed: Cell<bool>,
    /// The last timeline id handed out.
    ids: Cell<u64>,
}

/// A CSS animation or transition `getAnimations()` lists.
pub(crate) struct Run {
    pub id: u64,
    pub el: NodeId,
    pub transition: bool,
    /// The animation's name, or the transitioning property.
    pub name: String,
    pub start_ms: f64,
    pub delay_ms: f64,
    pub duration_ms: f64,
    pub iterations: f64,
}

impl State {
    /// Whether timelines are kept: a listener could observe their events, or
    /// `getAnimations()` lists them.
    fn tracking(&self, dom: &lp_dom::Dom) -> bool {
        dom.has_animation_listeners() || self.listed.get()
    }
    fn transitions_tracked(&self, dom: &lp_dom::Dom) -> bool {
        dom.has_transition_listeners() || self.listed.get()
    }
    fn next_id(&self) -> u64 {
        self.ids.set(self.ids.get() + 1);
        self.ids.get()
    }

    pub(crate) fn clear(&self) {
        for timeline in self.animations.take().into_iter().chain(self.transitions.take()) {
            timeline.live.set(false);
        }
        self.observed.borrow_mut().clear();
        self.flushed.set(None);
        self.rules.borrow_mut().take();
    }

    /// The rules of the document's sheets, read again only when the sheets changed.
    fn rules(&self, dom: &lp_dom::Dom, style: &lp_dom::style::StyleEngine) -> Rc<TimingRules> {
        let stamp = style.observation_stamp(dom);
        if self.rules_stamp.replace(Some(stamp)) == Some(stamp) {
            if let Some((_, rules)) = &*self.rules.borrow() {
                return rules.clone();
            }
        }
        let key = style.author_sheets_key(dom);
        if let Some((at, rules)) = &*self.rules.borrow() {
            if *at == key {
                return rules.clone();
            }
        }
        let mut texts = Vec::new();
        style.for_each_author_sheet(dom, &mut |text| texts.push(text.to_owned()));
        let rules = Rc::new(TimingRules::parse(texts.iter().map(String::as_str), style.viewport()));
        *self.rules.borrow_mut() = Some((key, rules.clone()));
        rules
    }
}

/// One event to fire, `after` the moment it was planned.
struct Planned {
    el: NodeId,
    ty: &'static str,
    transition: bool,
    name: String,
    elapsed_s: f64,
    after: Duration,
    live: Rc<Cell<bool>>,
    /// Set when the event fires (the timeline's `end`).
    ends: Option<Rc<Cell<bool>>>,
}

fn after(ms: f64) -> Duration {
    Duration::from_secs_f64(ms.max(0.0) / 1000.0)
}

/// Checkpoint from `PageDom::after_task`: a changed document is looked at
/// again one frame later.
pub(crate) fn after_task(page: &Rc<PageDom>) {
    let state = &page.animation_events;
    if state.queued.get() || page.is_disposed() || page.scripting_disabled() {
        return;
    }
    {
        let Ok(dom) = page.dom.try_borrow() else { return };
        if !state.tracking(&dom) {
            return;
        }
        let Some(layout) = page.try_layout() else { return };
        if state.stamp.get() == Some(layout.style().observation_stamp(&dom)) {
            return;
        }
    }
    state.queued.set(true);
    let weak = Rc::downgrade(page);
    page.post_delayed_task(
        FRAME,
        Box::new(move || {
            if let Some(page) = weak.upgrade() {
                page.animation_events.queued.set(false);
                update(&page, false);
            }
        }),
    );
}

/// A frame: the document's animations and the observed elements' transitions.
/// `defer`: events due now fire in a task of their own.
fn update(page: &Rc<PageDom>, defer: bool) {
    if page.is_disposed() || page.scripting_disabled() {
        return;
    }
    let state = &page.animation_events;
    let mut planned = Vec::new();
    {
        let dom = page.dom.borrow();
        let layout = page.layout();
        let style = layout.style();
        // Styles first: syncing them registers new `<style>` sheets.
        style.engine_styled(&dom);
        let rules = state.rules(&dom, style);
        state.stamp.set(Some(style.observation_stamp(&dom)));
        // Animations the styles ask for now.
        let mut wanted = Vec::new();
        for el in rules.animation_candidates(&dom, page.document()) {
            if dom.is_connected(el).unwrap_or(false) {
                wanted.extend(rules.element(&dom, el).animations.into_iter().map(|a| (el, a)));
            }
        }
        let now_ms = crate::event::page_now_ms(page);
        let mut animations = state.animations.borrow_mut();
        animations.retain(|known| {
            let keep = wanted.iter().any(|(el, a)| *el == known.el && known.timing.as_ref() == Some(a));
            if !keep {
                known.live.set(false);
                if !known.ended.get() && dom.is_alive(known.el) && dom.is_connected(known.el).unwrap_or(false) {
                    planned.push(Planned {
                        el: known.el,
                        ty: "animationcancel",
                        transition: false,
                        name: known.name.clone(),
                        elapsed_s: 0.0,
                        after: Duration::ZERO,
                        live: Rc::new(Cell::new(true)),
                        ends: None,
                    });
                }
            }
            keep
        });
        for (el, animation) in wanted {
            if animations.iter().any(|known| known.el == el && known.timing.as_ref() == Some(&animation)) {
                continue;
            }
            let timeline = Timeline {
                id: state.next_id(),
                start_ms: now_ms,
                delay_ms: animation.delay_ms,
                duration_ms: animation.duration_ms,
                iterations: animation.iterations,
                el,
                name: animation.name.clone(),
                live: Rc::new(Cell::new(true)),
                ended: Rc::new(Cell::new(false)),
                timing: Some(animation.clone()),
            };
            let event = |ty: &'static str, at_ms: f64, elapsed_ms: f64, ends: bool| Planned {
                el,
                ty,
                transition: false,
                name: animation.name.clone(),
                elapsed_s: elapsed_ms / 1000.0,
                after: after(at_ms),
                live: timeline.live.clone(),
                ends: ends.then(|| timeline.ended.clone()),
            };
            let (delay, duration) = (animation.delay_ms, animation.duration_ms);
            planned.push(event("animationstart", delay, (-delay).max(0.0), false));
            if animation.iterations.is_finite() {
                if duration > 0.0 {
                    // Boundaries a negative delay already passed fire nothing.
                    let boundaries = (1..).map(|k| k as f64).take_while(|k| *k < animation.iterations);
                    for k in boundaries.filter(|k| delay + k * duration > 0.0).take(MAX_ITERATIONS) {
                        planned.push(event("animationiteration", delay + k * duration, k * duration, false));
                    }
                }
                let active = duration * animation.iterations;
                planned.push(event("animationend", delay + active, active, true));
            }
            animations.push(timeline);
        }
        drop(animations);
        // Style change events of the observed elements, and of the others
        // that may transition now.
        let observed: Vec<NodeId> = state.observed.borrow().keys().copied().collect();
        for el in observed {
            if !dom.is_alive(el) || !dom.is_connected(el).unwrap_or(false) {
                state.observed.borrow_mut().remove(&el);
                continue;
            }
            style_change(state, &dom, style, Some(&rules), el, now_ms, &mut planned);
        }
        if state.transitions_tracked(&dom) {
            transition_candidates_changed(state, &dom, style, &rules, page.document(), now_ms, &mut planned);
        }
        state.transitions.borrow_mut().retain(|t| t.live.get() && !t.ended.get() && dom.is_alive(t.el));
    }
    schedule(page, planned, defer);
}

/// `getAnimations()`: the CSS animations and transitions of the document,
/// its style brought up to date first (as a style flush would; their
/// events due now fire in a task of their own). From the first call on,
/// they are kept without listeners.
pub(crate) fn runs(page: &Rc<PageDom>) -> Vec<Run> {
    let state = &page.animation_events;
    if page.is_disposed() || page.scripting_disabled() {
        return Vec::new();
    }
    state.listed.set(true);
    let current = {
        let Ok(dom) = page.dom.try_borrow() else { return Vec::new() };
        let Some(layout) = page.try_layout() else { return Vec::new() };
        let style = layout.style();
        style.engine_styled(&dom);
        state.stamp.get() == Some(style.observation_stamp(&dom))
    };
    if !current {
        update(page, true);
    }
    let dom = page.dom.borrow();
    let animations = state.animations.borrow();
    let transitions = state.transitions.borrow();
    transitions
        .iter()
        .map(|t| (t, true))
        .chain(animations.iter().map(|t| (t, false)))
        .filter(|(t, _)| t.live.get() && dom.is_alive(t.el) && dom.is_connected(t.el).unwrap_or(false))
        .map(|(t, transition)| Run {
            id: t.id,
            el: t.el,
            transition,
            name: t.name.clone(),
            start_ms: t.start_ms,
            delay_ms: t.delay_ms,
            duration_ms: t.duration_ms,
            iterations: t.iterations,
        })
        .collect()
}

/// Script read `el`'s computed style: a style change event of the element
/// (and, as for any style flush, of the elements that may transition).
pub(crate) fn observe(page: &Rc<PageDom>, el: NodeId) {
    let state = &page.animation_events;
    let mut planned = Vec::new();
    {
        let Ok(dom) = page.dom.try_borrow() else { return };
        if !state.tracking(&dom) || page.is_disposed() || page.scripting_disabled() {
            return;
        }
        let Some(layout) = page.try_layout() else { return };
        let style = layout.style();
        style.engine_styled(&dom);
        let stamp = style.observation_stamp(&dom);
        let seen = state.observed.borrow().get(&el).is_some_and(|(_, at)| *at == stamp);
        let flushed = state.flushed.get() == Some(stamp) || !state.transitions_tracked(&dom);
        if seen && flushed {
            return;
        }
        let now_ms = crate::event::page_now_ms(page);
        if !seen && dom.is_connected(el).unwrap_or(false) {
            // The rules are read only once the element's style is known to have
            // changed: a first observation never needs them.
            style_change(state, &dom, style, None, el, now_ms, &mut planned);
        }
        if !flushed {
            let rules = state.rules(&dom, style);
            transition_candidates_changed(state, &dom, style, &rules, page.document(), now_ms, &mut planned);
        }
    }
    // Events never fire inside the read itself.
    schedule(page, planned, true);
}

/// Script forced a style flush (a layout read): a style change event of
/// the elements that may transition. Costs one flag check without a
/// transition listener, and one stamp check while the style is unchanged.
pub(crate) fn style_flush(page: &Rc<PageDom>) {
    let state = &page.animation_events;
    let mut planned = Vec::new();
    {
        let Ok(dom) = page.dom.try_borrow() else { return };
        if !state.transitions_tracked(&dom) || page.is_disposed() || page.scripting_disabled() {
            return;
        }
        let Some(layout) = page.try_layout() else { return };
        let style = layout.style();
        // The flush itself: syncing styles registers new `<style>` sheets.
        style.engine_styled(&dom);
        if state.flushed.get() == Some(style.observation_stamp(&dom)) {
            return;
        }
        let rules = state.rules(&dom, style);
        let now_ms = crate::event::page_now_ms(page);
        transition_candidates_changed(state, &dom, style, &rules, page.document(), now_ms, &mut planned);
    }
    schedule(page, planned, true);
}

/// A style change event of every connected element of `doc` that may
/// transition, once per style stamp.
fn transition_candidates_changed(
    state: &State,
    dom: &lp_dom::Dom,
    style: &lp_dom::style::StyleEngine,
    rules: &TimingRules,
    doc: NodeId,
    now_ms: f64,
    planned: &mut Vec<Planned>,
) {
    let stamp = style.observation_stamp(dom);
    if state.flushed.replace(Some(stamp)) == Some(stamp) {
        return;
    }
    for el in rules.transition_candidates(dom, doc) {
        if state.observed.borrow().get(&el).is_some_and(|(_, at)| *at == stamp) {
            continue;
        }
        if dom.is_connected(el).unwrap_or(false) {
            style_change(state, dom, style, rules, el, now_ms, planned);
        }
    }
}

/// A style change event of `el`: the properties of its transition list whose
/// computed value differs from the before-change style start transitions,
/// and the current style becomes the before-change style. `rules` is read
/// from the sheets when not given, and only if the style changed.
fn style_change(
    state: &State,
    dom: &lp_dom::Dom,
    style: &lp_dom::style::StyleEngine,
    rules: Option<&TimingRules>,
    el: NodeId,
    now_ms: f64,
    planned: &mut Vec<Planned>,
) {
    let Some(now) = style.stylo_style(dom, el) else { return };
    let stamp = style.observation_stamp(dom);
    let before = {
        let mut observed = state.observed.borrow_mut();
        if observed.len() >= MAX_OBSERVED && !observed.contains_key(&el) {
            observed.retain(|id, _| dom.is_alive(*id) && dom.is_connected(*id).unwrap_or(false));
            if observed.len() >= MAX_OBSERVED {
                return;
            }
        }
        observed.insert(el, (now.clone(), stamp)).map(|(before, _)| before)
    };
    let Some(before) = before.filter(|before| !StyleSnapshot::ptr_eq(before, &now)) else { return };
    let read;
    let rules = match rules {
        Some(rules) => rules,
        None => {
            read = state.rules(dom, style);
            &*read
        }
    };
    let ElementTiming { transitions, .. } = rules.element(dom, el);
    let mut started: Vec<&'static str> = Vec::new();
    for transition in &transitions {
        for longhand in lp_dom::timing::changed_longhands(&before, &now, &transition.property) {
            if started.contains(&longhand) {
                continue;
            }
            started.push(longhand);
            let mut running = state.transitions.borrow_mut();
            // A transition of the property still under way is replaced.
            running.retain(|known| {
                let same = known.el == el && known.name == longhand;
                if same && known.live.get() && !known.ended.get() {
                    known.live.set(false);
                    planned.push(Planned {
                        el,
                        ty: "transitioncancel",
                        transition: true,
                        name: longhand.to_owned(),
                        elapsed_s: 0.0,
                        after: Duration::ZERO,
                        live: Rc::new(Cell::new(true)),
                        ends: None,
                    });
                }
                !same
            });
            let timeline = Timeline {
                id: state.next_id(),
                start_ms: now_ms,
                delay_ms: transition.delay_ms,
                duration_ms: transition.duration_ms,
                iterations: 1.0,
                el,
                name: longhand.to_owned(),
                live: Rc::new(Cell::new(true)),
                ended: Rc::new(Cell::new(false)),
                timing: None,
            };
            let (delay, duration) = (transition.delay_ms, transition.duration_ms);
            let event = |ty: &'static str, at_ms: f64, elapsed_ms: f64, ends: bool| Planned {
                el,
                ty,
                transition: true,
                name: longhand.to_owned(),
                elapsed_s: elapsed_ms / 1000.0,
                after: after(at_ms),
                live: timeline.live.clone(),
                ends: ends.then(|| timeline.ended.clone()),
            };
            planned.push(event("transitionrun", 0.0, 0.0, false));
            planned.push(event("transitionstart", delay, (-delay).max(0.0), false));
            planned.push(event("transitionend", delay + duration, duration, true));
            running.push(timeline);
        }
    }
}

/// Fires the events due now (as a task of their own when `defer`) and sets a
/// timer for each later one; equal delays keep their order.
fn schedule(page: &Rc<PageDom>, planned: Vec<Planned>, defer: bool) {
    for event in planned {
        let delay = event.after;
        if delay.is_zero() && !defer {
            fire(page, &event);
            continue;
        }
        let weak = Rc::downgrade(page);
        let task: Box<dyn FnOnce()> = Box::new(move || {
            if let Some(page) = weak.upgrade() {
                fire(&page, &event);
            }
        });
        if delay.is_zero() {
            page.post_task(task);
        } else {
            page.post_delayed_task(delay, task);
        }
    }
}

/// Dispatches one event at its element, unless its timeline was canceled or
/// the element left the document meanwhile.
fn fire(page: &Rc<PageDom>, event: &Planned) {
    if !event.live.get() || page.is_disposed() {
        return;
    }
    {
        let dom = page.dom.borrow();
        if !dom.is_alive(event.el) || !dom.is_connected(event.el).unwrap_or(false) {
            return;
        }
    }
    if let Some(ended) = &event.ends {
        ended.set(true);
    }
    let interface = if event.transition { EventInterface::TransitionEvent } else { EventInterface::AnimationEvent };
    let ev = EventCore::trusted(page, interface, event.ty, true, false);
    {
        let mut fields = ev.fields.borrow_mut();
        if event.transition {
            fields.property_name = event.name.clone();
        } else {
            fields.animation_name = event.name.clone();
        }
        fields.elapsed_time = event.elapsed_s;
    }
    crate::target::fire_at_node(page, event.el, &ev);
}
