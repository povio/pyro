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
//! elements whose style names an `@keyframes` rule (`pyro_dom::timing`) are
//! compared with the animations already known. A new one fires
//! `animationstart` after its delay, `animationiteration` at each iteration
//! boundary still ahead and `animationend` after its last iteration; one
//! whose name left the style before that fires `animationcancel`. An
//! `infinite` animation fires `animationstart` only. `animation-play-state:
//! paused` holds an animation's events with its time; set running again,
//! the rest fire from there ([`plan_animation`]). Once script controls a
//! CSS animation's object (play, pause, finish, cancel, its time), the
//! events follow the object's state instead ([`scripted_state`]).
//!
//! Transitions need a before-change style. Once a transition listener
//! exists, the layout engine records the elements whose computed style a
//! restyle replaces, with the style before and after
//! (`StyleEngine::take_style_changes`: the traversal sees both, so this
//! costs the restyled elements, not a scan of the document). Every style
//! flush script forces (`getComputedStyle`, `offset*`, `client*`, `scroll*`,
//! `getBoundingClientRect`, `getClientRects`) and each frame after a change
//! is a style change event of those elements: a property of an element's
//! transition list whose computed value differs between the two styles
//! transitions: `transitionrun`, then `transitionstart` after the delay and
//! `transitionend` after the duration. So the usual
//! `el.offsetHeight; el.classList.add('open')` starts a transition, as does
//! a change made in a task after the element was first rendered. An element
//! styled for the first time has no before-change style and transitions
//! nothing.
//!
//! Once script called `getAnimations()` the timelines are kept the same way
//! without listeners, and `getAnimations()` lists them as `CSSAnimation` and
//! `CSSTransition` objects ([`runs`]).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use pyro_dom::timing::{displays_none, ElementTiming, StyleSnapshot, TimingRules};
use pyro_dom::style::PseudoElement;
use pyro_dom::NodeId;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::event::{EventCore, EventInterface};
use crate::page::PageDom;

/// How long after a change the document's animations are looked at again.
const FRAME: Duration = Duration::from_millis(16);
/// Iteration events one animation may schedule.
const MAX_ITERATIONS: usize = 1024;

type Stamp = (u64, u64, u32, u32);

/// An animation or transition whose events are (or were) scheduled.
pub(crate) struct Timeline {
    /// Its identity for `getAnimations()`.
    id: u64,
    /// The page clock when it started (its style change), in ms.
    start_ms: f64,
    /// The style change event that started it ([`State::style_events`]).
    generation: u64,
    pub(crate) delay_ms: f64,
    pub(crate) duration_ms: f64,
    /// 1 for a transition.
    pub(crate) iterations: f64,
    el: NodeId,
    /// Its target's `::before`/`::after` (`None`: the element itself).
    pseudo: Option<PseudoElement>,
    /// A CSS animation driven by a scroll or view progress timeline: no time-based events, and the
    /// layout engine takes its progress from the scroll position.
    pub(crate) driven: bool,
    /// The animation's name, or the transitioning longhand.
    name: String,
    /// Cleared to cancel the events not fired yet.
    live: Rc<Cell<bool>>,
    /// Set once its `end` event fired.
    ended: Rc<Cell<bool>>,
    /// An animation's timing as declared: the same name with another timing
    /// is another run (a finished `spin 2ms` restyled to `spin 1s infinite`
    /// starts again). `None` for transitions.
    timing: Option<pyro_dom::timing::AnimationTiming>,
    /// An animation held by `animation-play-state: paused`: the local time
    /// it holds (ms).
    paused_at: Option<f64>,
    /// An animation: the events of its current plan ([`plan_animation`]),
    /// cleared when it plays on another way (paused, resumed, or script set
    /// its time).
    epoch: Rc<Cell<bool>>,
    /// An animation: how far its events got: 0 none, 1 `animationstart`,
    /// `1 + k` the `animationiteration` of boundary `k`, [`ENDED`] its end.
    progress: Rc<Cell<u32>>,
    /// A transition: its easing (`None`: `ease`) and the keyframes it runs
    /// between (the property's value before and after the style change).
    easing: Option<String>,
    keyframes: Option<std::sync::Arc<pyro_dom::animation::TextKeyframes>>,
    /// A transition: the before-change and after-change styles it runs between
    /// (`getComputedStyle` interpolates them, `animation_frame::Source::transition_value`).
    pub(crate) styles: Option<(StyleSnapshot, StyleSnapshot)>,
    /// A transition: its reversing-adjusted start style (css-transitions-1 §3; `None`: its
    /// before-change style) and its reversing shortening factor (1 unless it reversed another).
    reversing: Option<(StyleSnapshot, f64)>,
    /// pyro: a CSS animation's `animation-timeline` and range as last handed to script (`None`:
    /// time-based), to tell its object of a change.
    timeline_text: Option<(String, String, String)>,
}

impl Timeline {
    /// The local time now: the time since it started, or the time it holds.
    pub(crate) fn local_ms(&self, now_ms: f64) -> f64 {
        self.paused_at.unwrap_or(now_ms - self.start_ms)
    }
    pub(crate) fn id(&self) -> u64 {
        self.id
    }
    pub(crate) fn el(&self) -> NodeId {
        self.el
    }
    pub(crate) fn pseudo(&self) -> Option<PseudoElement> {
        self.pseudo
    }
    pub(crate) fn name(&self) -> &str {
        &self.name
    }
    pub(crate) fn start_ms(&self) -> f64 {
        self.start_ms
    }
    pub(crate) fn is_live(&self) -> bool {
        self.live.get()
    }
    /// Its `end` event fired.
    pub(crate) fn has_ended(&self) -> bool {
        self.ended.get()
    }
    pub(crate) fn is_paused(&self) -> bool {
        self.paused_at.is_some()
    }
    pub(crate) fn timing(&self) -> Option<&pyro_dom::timing::AnimationTiming> {
        self.timing.as_ref()
    }
    pub(crate) fn easing(&self) -> Option<&str> {
        self.easing.as_deref()
    }
    pub(crate) fn keyframes(&self) -> Option<std::sync::Arc<pyro_dom::animation::TextKeyframes>> {
        self.keyframes.clone()
    }
}

/// Per-document state (`PageDom::animation_events`).
#[derive(Default)]
pub(crate) struct State {
    /// The running task's animation clock reading ([`State::task_now`]).
    task_clock: Cell<Option<(u64, f64)>>,
    /// The sheets' key and the rules read from them.
    rules: RefCell<Option<Rc<TimingRules>>>,
    /// The style stamp the rules' key was last computed at.
    rules_stamp: Cell<Option<Stamp>>,
    /// The style stamp `inline_animations` last looked at, and its answer.
    inline_stamp: Cell<Option<(Stamp, bool)>>,
    /// The element and style stamp `style_read_hidden_content` last handled: a computed style
    /// read property by property (a screenshot library copies ~350 per element) asks once.
    hidden_read: Cell<Option<(NodeId, Stamp)>>,
    /// The style stamp of the last frame update.
    stamp: Cell<Option<Stamp>>,
    queued: Cell<bool>,
    animations: RefCell<Vec<Timeline>>,
    transitions: RefCell<Vec<Timeline>>,
    /// Script called `getAnimations()`: timelines are kept without listeners.
    listed: Cell<bool>,
    /// The last timeline id handed out.
    ids: Cell<u64>,
    /// Style change events processed so far ([`update`]): a transition's generation, which orders
    /// `getAnimations()` (one event's transitions share it and list by property name).
    style_events: Cell<u64>,
    /// Content roots seen styled `display: none` where Chrome has
    /// `content-visibility: hidden` (see [`hides_content`]): their content
    /// keeps a before-change style while hidden.
    hidden_roots: RefCell<FxHashSet<NodeId>>,
    /// Bumped whenever the timelines, their play state or the script
    /// animations change (the animated values' stamp,
    /// `animation_frame::stamp`).
    pub(crate) generation: Cell<u64>,
    /// The animations of `Animation` objects (Web Animations, and CSS
    /// animations and transitions script controls), as script reported them
    /// (`animation_frame::report`).
    pub(crate) scripted: RefCell<FxHashMap<u64, crate::animation_frame::Scripted>>,
    /// The CSS animations and transitions whose `Animation` object lost its effect
    /// (`effect = null`): `getComputedStyle` reports no transition value for them.
    pub(crate) effect_removed: RefCell<FxHashSet<u64>>,
    /// The layout engine samples the page's animated values
    /// (`StyleEngine::set_animation_source` was called).
    source_installed: Cell<bool>,
    /// The last [`State::next_report_key`].
    report_keys: Cell<u64>,
    /// The document's SMIL `<animate>` elements (`smil.rs`).
    pub(crate) smil: crate::smil::Cache,
    /// The before-change styles of the elements under such a root that may
    /// transition, by element, with their root: the engine styles none of
    /// them while the root is hidden, and styles them afresh once shown.
    hidden_content: RefCell<FxHashMap<NodeId, (NodeId, StyleSnapshot)>>,
    /// `Dom::removal_count` when `hidden_roots` last dropped the roots that
    /// left the document: a removed subtree loses its styles (Chrome), so a
    /// root removed and inserted again has no before-change styles, and a
    /// removed root does not keep its content's styles until navigation.
    hidden_roots_removals: Cell<u64>,
    /// `Dom::removal_count` when [`cancel_orphaned_transitions`] last looked for running
    /// transitions whose element left its place in the tree.
    transitions_removals: Cell<u64>,
    /// A style change event canceled a running transition: the `CSSTransition` objects script
    /// holds are told ([`sync_canceled_runs`]), so that their `ready` rejects and they go idle.
    runs_canceled: Cell<bool>,
}

/// A CSS animation or transition `getAnimations()` lists.
pub(crate) struct Run {
    pub id: u64,
    pub el: NodeId,
    /// Its target's `::before`/`::after` (`None`: the element).
    pub pseudo: Option<PseudoElement>,
    /// A scroll-driven animation's `animation-timeline` (`scroll()`, `view()`, `--name`) and its
    /// `animation-range-start`/`-end`.
    pub timeline: Option<(String, String, String)>,
    pub transition: bool,
    /// The animation's name, or the transitioning property.
    pub name: String,
    pub start_ms: f64,
    /// The style change event that started it (orders simultaneous transitions).
    pub generation: u64,
    pub delay_ms: f64,
    pub duration_ms: f64,
    pub iterations: f64,
    pub direction: String,
    pub fill: String,
    /// The effect's easing (a transition's; `linear` for an animation, whose
    /// timing function applies per keyframe).
    pub easing: String,
    pub keyframe_easing: String,
    /// The local time an animation paused by its style holds.
    pub paused_at: Option<f64>,
    /// pyro: an animation's `animation-trigger` item, the element declaring the timeline trigger
    /// it names and that `timeline-trigger` item (`None`: no trigger, or none of that name).
    pub trigger: Option<(String, NodeId, String)>,
}

impl State {
    pub(crate) fn transition_frames(&self, id: u64) -> Option<std::sync::Arc<pyro_dom::animation::TextKeyframes>> {
        self.transitions.borrow().iter().find(|run| run.id == id)?.keyframes()
    }

    /// The animation clock (page time, ms) of the running task: its first reading, held for the
    /// rest of the task, so that the CSS animations and transitions a task starts start at the
    /// time the frame samples them (their time is 0 until the clock moves, as a browser's pending
    /// start time; a later reading in the task would put them before their start).
    pub(crate) fn task_now(&self, page: &PageDom) -> f64 {
        let Some(seq) = pyro_runtime::task_seq() else { return crate::event::page_now_ms(page) };
        match self.task_clock.get() {
            Some((at, now)) if at == seq => now,
            _ => {
                let now = page.timeline_now_ms();
                self.task_clock.set(Some((seq, now)));
                now
            }
        }
    }

    /// Whether timelines are kept: the sheets animate or transition (their
    /// values are sampled into the layout engine's cascade), a listener could
    /// observe their events, or `getAnimations()` lists them.
    fn tracking(&self, dom: &pyro_dom::Dom, style: &pyro_dom::style::StyleEngine, doc: NodeId) -> bool {
        dom.has_animation_listeners()
            || self.listed.get()
            || dom.smil_seen()
            || dom.inline_transitions_seen()
            || self.rules(dom, style).animates()
            || self.inline_animations(dom, style, doc)
    }
    /// Whether a `style` attribute asks for an animation of an existing
    /// `@keyframes` rule (no sheet rule declares one): the page times it too,
    /// as Stylo's own animations get negative and changed delays wrong.
    fn inline_animations(&self, dom: &pyro_dom::Dom, style: &pyro_dom::style::StyleEngine, doc: NodeId) -> bool {
        let stamp = style.observation_stamp(dom);
        if let Some((at, found)) = self.inline_stamp.get() {
            if at == stamp {
                return found;
            }
        }
        let found = !self.rules(dom, style).animation_candidates(dom, doc).is_empty();
        self.inline_stamp.set(Some((stamp, found)));
        found
    }
    fn transitions_tracked(&self, dom: &pyro_dom::Dom, style: &pyro_dom::style::StyleEngine) -> bool {
        dom.has_transition_listeners()
            || self.listed.get()
            || dom.inline_transitions_seen()
            || self.rules(dom, style).animates()
    }
    fn bump(&self) {
        self.generation.set(self.generation.get().wrapping_add(1));
    }
    pub(crate) fn animations(&self) -> std::cell::Ref<'_, Vec<Timeline>> {
        self.animations.borrow()
    }
    pub(crate) fn try_timelines(
        &self,
    ) -> Option<(std::cell::Ref<'_, Vec<Timeline>>, std::cell::Ref<'_, Vec<Timeline>>)> {
        Some((self.animations.try_borrow().ok()?, self.transitions.try_borrow().ok()?))
    }
    fn next_id(&self) -> u64 {
        self.ids.set(self.ids.get() + 1);
        self.ids.get()
    }
    /// A key for an `Animation` object's reports (`animation_frame::report`),
    /// in creation order.
    pub(crate) fn next_report_key(&self) -> u64 {
        self.report_keys.set(self.report_keys.get() + 1);
        self.report_keys.get()
    }

    pub(crate) fn clear(&self) {
        for timeline in self.animations.take().into_iter().chain(self.transitions.take()) {
            timeline.live.set(false);
        }
        self.rules.borrow_mut().take();
        self.hidden_roots.borrow_mut().clear();
        self.hidden_content.borrow_mut().clear();
        self.scripted.borrow_mut().clear();
        self.smil.clear();
        self.bump();
    }

    /// Makes the layout engine sample the page's animated values at each
    /// sync, once the page has any.
    pub(crate) fn install_source(&self, page: &Rc<PageDom>, style: &pyro_dom::style::StyleEngine) {
        if self.source_installed.replace(true) {
            return;
        }
        style.set_animation_source(Some(crate::animation_frame::source(page)));
    }

    /// The rules of the document's sheets (the style engine's, read again
    /// only when the sheets changed), looked up once per observation stamp.
    fn rules(&self, dom: &pyro_dom::Dom, style: &pyro_dom::style::StyleEngine) -> Rc<TimingRules> {
        let stamp = style.observation_stamp(dom);
        if self.rules_stamp.replace(Some(stamp)) == Some(stamp) {
            if let Some(rules) = &*self.rules.borrow() {
                return rules.clone();
            }
        }
        let rules = style.timing_rules(dom);
        *self.rules.borrow_mut() = Some(rules.clone());
        rules
    }
}

/// One event to fire, `after` the moment it was planned.
struct Planned {
    el: NodeId,
    /// `pseudoElement` (`""`: the element).
    pseudo: &'static str,
    ty: &'static str,
    transition: bool,
    name: String,
    elapsed_s: f64,
    after: Duration,
    live: Rc<Cell<bool>>,
    /// Set when the event fires (the timeline's `end`).
    ends: Option<Rc<Cell<bool>>>,
    /// An animation's plan the event belongs to ([`Timeline::epoch`]).
    epoch: Option<Rc<Cell<bool>>>,
    /// An animation's progress and the step this event takes it to.
    step: Option<(Rc<Cell<u32>>, u32)>,
}

/// [`Timeline::progress`] once `animationend` fired.
const ENDED: u32 = u32::MAX;

/// Plans the `transitionstart` and `transitionend` of transition `t` from its local time `local`
/// (ms) on, replacing those planned before, as [`plan_animation`] does for an animation: one its
/// time reached already fires now (a transition sought or finished by script, WPT
/// `inherit-background-color-transition`), a later one when its time gets there at `rate`.
fn plan_transition(t: &mut Timeline, local: f64, rate: f64, held: bool, planned: &mut Vec<Planned>) {
    t.epoch.set(false);
    t.epoch = Rc::new(Cell::new(true));
    let progress = t.progress.get();
    if !t.live.get() || progress == ENDED {
        return;
    }
    let (delay, duration) = (t.delay_ms, t.duration_ms);
    let moving = !held && rate > 0.0;
    let at = |time: f64| {
        if time <= local {
            Some(0.0)
        } else if moving {
            Some((time - local) / rate)
        } else {
            None
        }
    };
    let event = |ty: &'static str, at_ms: f64, elapsed_ms: f64, step: u32| {
        let due = at_ms <= 0.0;
        if due {
            t.progress.set(t.progress.get().max(step));
            if step == ENDED {
                t.ended.set(true);
            }
        }
        Planned {
            el: t.el,
            pseudo: pseudo_name(t.pseudo),
            ty,
            transition: true,
            name: t.name.clone(),
            elapsed_s: elapsed_ms / 1000.0,
            after: after(at_ms),
            live: t.live.clone(),
            ends: (step == ENDED && !due).then(|| t.ended.clone()),
            epoch: (!due).then(|| t.epoch.clone()),
            step: (!due).then(|| (t.progress.clone(), step)),
        }
    };
    if progress < 1 {
        if let Some(at_ms) = at(delay) {
            planned.push(event("transitionstart", at_ms, (-delay).max(0.0), 1));
        }
    }
    if let Some(at_ms) = at(delay + duration) {
        planned.push(event("transitionend", at_ms, duration, ENDED));
    }
}

/// Plans the events of animation `t` from its local time `local` (ms) on,
/// replacing those planned before: the ones its time reached already and
/// that did not fire yet fire now (a run resumed or finished by script past
/// them; `animationstart` before the rest), the later ones when it gets
/// there, playing at `rate` (none while `held`). Iteration boundaries it
/// passed without playing through fire nothing.
fn plan_animation(t: &mut Timeline, local: f64, rate: f64, held: bool, planned: &mut Vec<Planned>) {
    t.epoch.set(false);
    t.epoch = Rc::new(Cell::new(true));
    let progress = t.progress.get();
    if !t.live.get() || progress == ENDED {
        return;
    }
    let (delay, duration, iterations) = (t.delay_ms, t.duration_ms, t.iterations);
    let moving = !held && rate > 0.0;
    let at = |time: f64| {
        if time <= local {
            Some(0.0)
        } else if moving {
            Some((time - local) / rate)
        } else {
            None
        }
    };
    // An event due now is the animation's already: it fires (in its task)
    // whatever plan replaces this one in the meantime, and counts as fired.
    let event = |ty: &'static str, at_ms: f64, elapsed_ms: f64, step: u32| {
        let due = at_ms <= 0.0;
        if due {
            t.progress.set(t.progress.get().max(step));
            if step == ENDED {
                t.ended.set(true);
            }
        }
        Planned {
            el: t.el,
            pseudo: pseudo_name(t.pseudo),
            ty,
            transition: false,
            name: t.name.clone(),
            elapsed_s: elapsed_ms / 1000.0,
            after: after(at_ms),
            live: t.live.clone(),
            ends: (step == ENDED && !due).then(|| t.ended.clone()),
            epoch: (!due).then(|| t.epoch.clone()),
            step: (!due).then(|| (t.progress.clone(), step)),
        }
    };
    if progress < 1 {
        if let Some(at_ms) = at(delay) {
            planned.push(event("animationstart", at_ms, (-delay).max(0.0), 1));
        }
    }
    if !iterations.is_finite() {
        return;
    }
    if duration > 0.0 && moving {
        // Boundaries behind it fire nothing: a negative delay passed them,
        // or it was resumed or sought past them.
        let boundaries = (1..).map(|k| k as f64).take_while(|k| *k < iterations);
        let ahead = boundaries.filter(|k| delay + k * duration > local && *k as u32 + 1 > progress);
        for k in ahead.take(MAX_ITERATIONS) {
            let at_ms = (delay + k * duration - local) / rate;
            planned.push(event("animationiteration", at_ms, k * duration, k as u32 + 1));
        }
    }
    let active = duration * iterations;
    if let Some(at_ms) = at(delay + active) {
        planned.push(event("animationend", at_ms, active, ENDED));
    }
}

fn after(ms: f64) -> Duration {
    crate::util::delay_from_ms(ms)
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
        let Some(layout) = page.try_layout() else { return };
        if !state.tracking(&dom, layout.style(), page.document()) {
            return;
        }
        // The syncs before that update record their style changes too: a
        // read that asks for no style flush (`innerText`) does not lose them.
        track_style_changes(state, &dom, layout.style());
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

/// A frame: the document's animations and the transitions its style changes start.
/// `defer`: events due now fire in a task of their own.
fn update(page: &Rc<PageDom>, defer: bool) {
    if page.is_disposed() || page.scripting_disabled() {
        return;
    }
    let state = &page.animation_events;
    state.style_events.set(state.style_events.get() + 1);
    let mut planned = Vec::new();
    {
        let dom = page.dom.borrow();
        let layout = page.layout();
        let style = layout.style();
        track_style_changes(state, &dom, style);
        // Styles first: syncing them registers new `<style>` sheets.
        style.engine_styled(&dom);
        let rules = state.rules(&dom, style);
        state.stamp.set(Some(style.observation_stamp(&dom)));
        // Animations the styles ask for now.
        let mut wanted = Vec::new();
        let style_of = |e: NodeId| style.stylo_style(&dom, e);
        let pseudo_rules = rules.has_pseudo_rules();
        let display_effects = std::cell::OnceCell::new();
        let may_sample_display = |target| {
            display_effects.get_or_init(|| {
                crate::animation_frame::frame(page, state.task_now(page), false).elements.into_iter()
                    .filter(|element| element.animations.iter().any(|effect| rules.may_sample_display(effect)))
                    .map(|element| element.key).collect::<FxHashSet<_>>()
            }).contains(&pyro_dom::snapshot::render_key(target))
        };
        for el in rules.animation_candidates(&dom, page.document()) {
            if dom.is_connected(el).unwrap_or(false) && !animation_display_suppressed(&dom, style, el, &may_sample_display) {
                wanted.extend(rules.element(&dom, el, &style_of).animations.into_iter().map(|a| (el, None, a)));
                // Its `::before`/`::after`, while it generates a box.
                if pseudo_rules {
                    for pseudo in [PseudoElement::Before, PseudoElement::After, PseudoElement::Marker] {
                        let timing = rules.element_for(&dom, el, Some(pseudo), &style_of);
                        if !timing.animations.is_empty() && style.pseudo_style(&dom, el, pseudo, false).is_some() {
                            wanted.extend(timing.animations.into_iter().map(|a| (el, Some(pseudo), a)));
                        }
                    }
                }
            }
        }
        let now_ms = state.task_now(page);
        if !wanted.is_empty() || crate::smil::present(page, &dom) {
            state.install_source(page, style);
        }
        // The CSS animations script controls follow their objects' states.
        let scripted_ids: FxHashSet<u64> = match state.scripted.try_borrow() {
            Ok(scripted) => scripted.values().filter_map(|s| s.css_id()).collect(),
            Err(_) => FxHashSet::default(),
        };
        let mut animations = state.animations.borrow_mut();
        let mut previous: Vec<_> = animations.drain(..).map(Some).collect();
        let mut occurrences = FxHashMap::default();
        for (index, run) in previous.iter().flatten().enumerate() {
            occurrences.entry((run.el, run.pseudo, run.name.clone())).or_insert_with(Vec::new).push(index);
        }
        let matches: Vec<_> = wanted
            .into_iter()
            .rev()
            .map(|(el, pseudo, animation)| {
                let known = occurrences
                    .get_mut(&(el, pseudo, animation.name.clone()))
                    .and_then(Vec::pop)
                    .and_then(|index| previous[index].take());
                (el, pseudo, animation, known)
            })
            .collect();
        let mut changed = false;
        for known in previous.into_iter().flatten() {
            changed = true;
            known.live.set(false);
            if !known.ended.get() && dom.is_alive(known.el) && dom.is_connected(known.el).unwrap_or(false) {
                planned.push(Planned {
                    el: known.el,
                    pseudo: pseudo_name(known.pseudo),
                    ty: "animationcancel",
                    transition: false,
                    name: known.name.clone(),
                    elapsed_s: 0.0,
                    after: Duration::ZERO,
                    live: Rc::new(Cell::new(true)),
                    ends: None,
                    epoch: None,
                    step: None,
                });
            }
        }
        // pyro: a run script controls changed: its object takes the CSS properties script did not
        // override (`animations.js` `cssFollow`), in a task.
        let mut scripted_changed = false;
        let mut any_trigger = false;
        // (Each known run is one wanted animation's: two of one name are two runs.)
        for (el, pseudo, animation, known) in matches.into_iter().rev() {
            if let Some(mut known) = known {
                let timing = known.timing.as_mut().unwrap();
                if timing.index != animation.index {
                    timing.index = animation.index;
                    changed = true;
                }
                // pyro: new times change the run, which goes on from its start (CSS Animations 1
                // § 3), its events planned again.
                if !known.timing.as_ref().unwrap().same_times(&animation) {
                    known.delay_ms = animation.delay_ms;
                    known.duration_ms = animation.duration_ms;
                    known.iterations = animation.iterations;
                    let timing = known.timing.as_mut().unwrap();
                    timing.delay_ms = animation.delay_ms;
                    timing.duration_ms = animation.duration_ms;
                    timing.duration_auto = animation.duration_auto;
                    timing.iterations = animation.iterations;
                    changed = true;
                    let driven = timeline_driven_now(&style_of_target(style, &dom, el, pseudo), animation.index);
                    scripted_changed |= scripted_ids.contains(&known.id);
                    if !driven && !scripted_ids.contains(&known.id) {
                        // (One that ended and is before its end again plays it again: its events
                        // from `animationstart` on; WPT css-animations `animation-delay-010`.)
                        let end = known.delay_ms + known.duration_ms * known.iterations;
                        if known.progress.get() == ENDED && known.local_ms(now_ms) < end {
                            known.progress.set(0);
                            known.ended.set(false);
                        }
                        let (local, held) = (known.local_ms(now_ms), known.paused_at.is_some());
                        plan_animation(&mut known, local, 1.0, held, &mut planned);
                    }
                }
                // A change of how it plays: paused holds its local time,
                // running again goes on from there, and its events follow
                // (unless script controls it: then its object's state does).
                // (Its timeline may have changed: `animation-timeline` is not part of the run.)
                known.driven = timeline_driven_now(&style_of_target(style, &dom, el, pseudo), animation.index);
                if scripted_ids.contains(&known.id) {
                    let text = known.driven.then(|| timeline_text_of(style, &dom, el, pseudo, animation.index)).flatten();
                    if text != known.timeline_text {
                        known.timeline_text = text;
                        scripted_changed = true;
                    }
                }
                let timing = known.timing.as_mut().unwrap();
                if timing.style != animation.style {
                    let replan = match (known.paused_at, animation.style.paused) {
                        (None, true) => {
                            known.paused_at = Some(now_ms - known.start_ms);
                            true
                        }
                        (Some(held), false) => {
                            known.start_ms = now_ms - held;
                            known.paused_at = None;
                            true
                        }
                        _ => false,
                    };
                    timing.style = animation.style.clone();
                    changed = true;
                    scripted_changed |= scripted_ids.contains(&known.id);
                    if replan && !scripted_ids.contains(&known.id) {
                        let (local, held) = (known.local_ms(now_ms), known.paused_at.is_some());
                        plan_animation(&mut known, local, 1.0, held, &mut planned);
                    }
                }
                animations.push(known);
                continue;
            }
            changed = true;
            // pyro: an animation with an `animation-trigger` waits for it, held just before its
            // start (only its backwards fill shows) until its object (`animations.js`) runs the
            // trigger's actions.
            let triggered = animation.trigger.is_some();
            any_trigger |= triggered;
            let paused = animation.style.paused || triggered;
            let driven = timeline_driven_now(&style_of_target(style, &dom, el, pseudo), animation.index);
            let mut timeline = Timeline {
                timeline_text: driven.then(|| timeline_text_of(style, &dom, el, pseudo, animation.index)).flatten(),
                id: state.next_id(),
                start_ms: now_ms,
                generation: state.style_events.get(),
                delay_ms: animation.delay_ms,
                duration_ms: animation.duration_ms,
                iterations: animation.iterations,
                el,
                pseudo,
                driven: timeline_driven_now(&style_of_target(style, &dom, el, pseudo), animation.index),
                name: animation.name.clone(),
                live: Rc::new(Cell::new(true)),
                ended: Rc::new(Cell::new(false)),
                timing: Some(animation.clone()),
                // Paused from the start: held at its beginning (a triggered one just before it).
                paused_at: if triggered { Some(-1e-6) } else { paused.then_some(0.0) },
                epoch: Rc::new(Cell::new(true)),
                progress: Rc::new(Cell::new(0)),
                easing: None,
                keyframes: None,
                styles: None,
                reversing: None,
            };
            // Held at 0 when paused: it starts only where a negative delay
            // put it in its active phase already, and goes no further. (A scroll-driven one has
            // no time-based events.)
            if !timeline.driven {
                plan_animation(&mut timeline, 0.0, 1.0, paused, &mut planned);
            }
            animations.push(timeline);
        }
        drop(animations);
        if changed {
            state.bump();
        }
        if scripted_changed || any_trigger {
            crate::animations::push_css_runs(page, any_trigger);
        }
        // Style change events of the elements whose style changed.
        if state.transitions_tracked(&dom, style) {
            transitions_from_changes(state, &dom, style, now_ms, &mut planned);
        } else {
            // Untracked: the used `anchor()` px changes are dropped, so the first tracked style
            // change event does not transition from a long-stale value.
            let _ = style.take_anchor_evals(&dom);
        }
        let mut transitions = state.transitions.borrow_mut();
        let count = transitions.len();
        // Ended: its `transitionend` fired, or (nobody listening) its end passed.
        let over = |t: &Timeline| t.ended.get() || now_ms > t.start_ms + t.delay_ms + t.duration_ms;
        transitions.retain(|t| t.live.get() && !over(t) && dom.is_alive(t.el));
        if transitions.len() != count {
            state.bump();
        }
        // The states script reported for CSS animations and transitions that
        // are gone.
        if let Ok(mut scripted) = state.scripted.try_borrow_mut() {
            if scripted.values().any(|s| s.css_id().is_some()) {
                let animations = state.animations.borrow();
                let live: FxHashSet<u64> =
                    animations.iter().chain(transitions.iter()).filter(|t| t.live.get()).map(|t| t.id).collect();
                scripted.retain(|_, s| s.css_id().is_none_or(|id| live.contains(&id)));
            }
        }
        if !transitions.is_empty() {
            state.install_source(page, style);
        }
    }
    finish_overlay_exits(page);
    sync_canceled_runs(page);
    schedule(page, planned, defer);
}

/// The `CSSTransition` objects of the transitions a style change event canceled go idle (their
/// `ready` rejects with an `AbortError`), in a task, as `getAnimations()` would find them: Chrome
/// cancels them in the style change event (WPT `CSSTransition-ready`, `CSSTransition-canceling`).
/// Only once script called `getAnimations()` (no object exists before).
fn sync_canceled_runs(page: &Rc<PageDom>) {
    let state = &page.animation_events;
    if state.runs_canceled.replace(false) && state.listed.get() {
        crate::animations::push_css_runs(page, false);
    }
}

/// Script changed the state of CSS animation `id`'s object (play, pause,
/// finish, cancel, its current time or rate): its events follow the object
/// from now on. An idle object (canceled) fires `animationcancel` unless the
/// animation ended, and starts again if played.
pub(crate) fn scripted_state(
    page: &Rc<PageDom>,
    id: u64,
    start: Option<f64>,
    hold: Option<f64>,
    rate: f64,
    origin: Option<f64>,
) {
    if page.is_disposed() || page.scripting_disabled() {
        return;
    }
    let now_ms = page.animation_events.task_now(page);
    let mut planned = Vec::new();
    {
        let local = match (hold, start, origin) {
            (Some(hold), _, _) => Some(hold),
            (None, Some(start), Some(origin)) => Some((now_ms - origin - start) * rate),
            _ => None,
        };
        // A CSS transition's object: its `transitionstart` and `transitionend` follow it.
        if let Ok(mut transitions) = page.animation_events.transitions.try_borrow_mut() {
            if let Some(t) = transitions.iter_mut().find(|t| t.id == id) {
                if let Some(local) = local {
                    plan_transition(t, local, rate, hold.is_some(), &mut planned);
                }
                drop(transitions);
                schedule(page, planned, true);
                return;
            }
        }
        let Ok(mut animations) = page.animation_events.animations.try_borrow_mut() else { return };
        let Some(t) = animations.iter_mut().find(|t| t.id == id) else { return };
        match local {
            Some(local) => plan_animation(t, local, rate, hold.is_some(), &mut planned),
            None => {
                t.epoch.set(false);
                if t.progress.get() != ENDED {
                    t.progress.set(0);
                    planned.push(Planned {
                        el: t.el,
                        pseudo: pseudo_name(t.pseudo),
                        ty: "animationcancel",
                        transition: false,
                        name: t.name.clone(),
                        elapsed_s: 0.0,
                        after: Duration::ZERO,
                        live: t.live.clone(),
                        ends: None,
                        epoch: None,
                        step: None,
                    });
                }
            }
        }
    }
    schedule(page, planned, true);
}

/// A rendering opportunity (a screenshot or screencast frame): the style
/// changes since the last frame start their transitions and animations now,
/// as a frame would (events due now fire in a task of their own). One flag
/// and stamp check when nothing changed.
pub(crate) fn before_render(page: &Rc<PageDom>) {
    let state = &page.animation_events;
    if page.is_disposed() || page.scripting_disabled() {
        return;
    }
    {
        let Ok(dom) = page.dom.try_borrow() else { return };
        let Some(layout) = page.try_layout() else { return };
        let style = layout.style();
        if !state.tracking(&dom, style, page.document()) || state.stamp.get() == Some(style.observation_stamp(&dom)) {
            return;
        }
    }
    update(page, true);
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
        track_style_changes(state, &dom, style);
        style.engine_styled(&dom);
        state.stamp.get() == Some(style.observation_stamp(&dom))
    };
    if !current {
        update(page, true);
    }
    let dom = page.dom.borrow();
    let animations = state.animations.borrow();
    let transitions = state.transitions.borrow();
    let rules = page.try_layout().map(|layout| state.rules(&dom, layout.style()));
    transitions
        .iter()
        .map(|t| (t, true))
        .chain(animations.iter().map(|t| (t, false)))
        .filter(|(t, _)| t.live.get() && dom.is_alive(t.el) && dom.is_connected(t.el).unwrap_or(false))
        .map(|(t, transition)| {
            let style = t.timing.as_ref().map(|t| &t.style);
            Run {
                id: t.id,
                el: t.el,
                pseudo: t.pseudo,
                timeline: (t.driven && !transition)
                    .then(|| {
                        let layout = page.try_layout()?;
                        let style = style_of_target(layout.style(), &dom, t.el, t.pseudo)?;
                        let index = t.timing.as_ref().map_or(0, |x| x.index);
                        let (start, end) = pyro_dom::timing::range_text(&style, index);
                        Some((pyro_dom::timing::timeline_text(&style, index), start, end))
                    })
                    .flatten(),
                transition,
                name: if transition { shown_property(&t.name).to_owned() } else { t.name.clone() },
                start_ms: t.start_ms,
                generation: t.generation,
                delay_ms: t.delay_ms,
                duration_ms: t.duration_ms,
                iterations: t.iterations,
                direction: style.map_or("normal", |s| s.direction.as_str()).to_owned(),
                fill: match style {
                    Some(s) => s.fill.clone(),
                    None => "backwards".to_owned(),
                },
                easing: match transition {
                    // (Sampled at full precision; shown as it serializes.)
                    true => t.easing.as_deref().map_or_else(|| "ease".to_owned(), pyro_dom::timing::shown_easing),
                    false => "linear".to_owned(),
                },
                keyframe_easing: style.and_then(|s| s.easing.as_deref()).map_or_else(|| "ease".into(), pyro_dom::timing::shown_easing),
                paused_at: t.paused_at,
                trigger: t.timing.as_ref().and_then(|timing| timing.trigger.clone()).and_then(|item| {
                    let name = item.split_whitespace().next()?.to_owned();
                    let layout = page.try_layout()?;
                    let style = layout.style();
                    let style_of = |e: NodeId| style.stylo_style(&dom, e);
                    let (source, decl) = rules.as_ref()?.timeline_trigger(&dom, page.document(), t.el, &name, &style_of)?;
                    Some((item, source, decl))
                }),
            }
        })
        .collect()
}

/// Script read `el`'s computed style: a style flush, as any layout read.
pub(crate) fn observe(page: &Rc<PageDom>, el: NodeId) {
    style_flush(page, Some(el));
}

/// Script forced a style flush (a style or layout read, of `read` when
/// known): a style change event of the elements whose style changed. Costs
/// one flag check without a transition listener, and one stamp check while
/// the style is unchanged.
pub(crate) fn style_flush(page: &Rc<PageDom>, read: Option<NodeId>) {
    let state = &page.animation_events;
    // A style flush after a change is a style change event for animations
    // too: the animations the styles ask for now start now (their values are
    // in the styles this flush reads), as at a frame.
    let pending = {
        let (Ok(dom), Some(layout)) = (page.dom.try_borrow(), page.try_layout()) else { return };
        let style = layout.style();
        !page.is_disposed()
            && !page.scripting_disabled()
            // (Inline animations of a sheet's `@keyframes` too: `tracking`.)
            && (dom.smil_seen() || state.tracking(&dom, style, page.document()))
            && state.stamp.get() != Some(style.observation_stamp(&dom))
    };
    if pending {
        update(page, true);
    }
    let mut planned = Vec::new();
    {
        let Ok(dom) = page.dom.try_borrow() else { return };
        if page.is_disposed() || page.scripting_disabled() {
            return;
        }
        let Some(layout) = page.try_layout() else { return };
        let style = layout.style();
        if !state.transitions_tracked(&dom, style) {
            // (As `update`: untracked used `anchor()` px changes are dropped.)
            let _ = style.take_anchor_evals(&dom);
            return;
        }
        track_style_changes(state, &dom, style);
        // The flush itself: syncing styles registers new `<style>` sheets.
        style.engine_styled(&dom);
        let now_ms = state.task_now(page);
        transitions_from_changes(state, &dom, style, now_ms, &mut planned);
        if let Some(el) = read {
            style_read_hidden_content(state, &dom, style, el);
        }
        // The read that follows reports the transitions just started (`transition_value`).
        if state.transitions.try_borrow().is_ok_and(|t| !t.is_empty()) {
            state.install_source(page, style);
        }
    }
    finish_overlay_exits(page);
    sync_canceled_runs(page);
    schedule(page, planned, true);
}

/// Whether hiding `el` (a popover or modal dialog) may start an `overlay` transition, which keeps
/// it in the top layer until it ends: a transition of `overlay` (or `all`) with
/// `transition-behavior: allow-discrete` (css-position-4 §4.1).
pub(crate) fn may_transition_overlay(page: &Rc<PageDom>, el: NodeId) -> bool {
    let (Ok(dom), Some(layout)) = (page.dom.try_borrow(), page.try_layout()) else { return false };
    let style = layout.style();
    let rules = page.animation_events.rules(&dom, style);
    if !rules.animates() {
        return false;
    }
    let style_of = |e: NodeId| style.stylo_style(&dom, e);
    rules
        .element(&dom, el, &style_of)
        .transitions
        .iter()
        .any(|t| t.allow_discrete && matches!(t.property.as_str(), "overlay" | "all"))
}

/// Hiding `el` (kept in the top layer for its `overlay` transition) after its style flush
/// ([`style_flush`]): an `overlay` transition of `el` started in this task, its `transitionstart`
/// not fired (a pending transition, its time still 0: typically `none` → `auto` when it was shown
/// and hidden with no rendering in between, the show's flush styling it out of the top layer and
/// the hide's in it), is dropped without events, as if never started. Reversing it at the next
/// style change event would shorten the `auto` → `none` transition that keeps the element in the
/// top layer by its progress, 0, taking the element out at once (WPT css-position
/// `overlay/overlay-transition-backdrop`). One started in an earlier task (an opening that
/// rendered) is reversed as usual. (A `transitionrun` a listener already heard gets no
/// `transitioncancel`.)
pub(crate) fn drop_unrendered_overlay_transition(page: &Rc<PageDom>, el: NodeId) {
    let state = &page.animation_events;
    let now = state.task_now(page);
    let Ok(mut transitions) = state.transitions.try_borrow_mut() else { return };
    let count = transitions.len();
    transitions.retain(|t| {
        let unrendered = t.el == el
            && t.pseudo.is_none()
            && t.name == "overlay"
            && t.live.get()
            && !t.ended.get()
            && t.start_ms == now
            && t.progress.get() == 0;
        if unrendered {
            t.live.set(false);
        }
        !unrendered
    });
    if transitions.len() != count {
        drop(transitions);
        state.bump();
        // (The `CSSTransition` script may hold goes idle.)
        if state.listed.get() {
            crate::animations::push_css_runs(page, false);
        }
    }
}

/// The elements kept in the top layer after leaving it ([`pyro_dom::Dom::keep_in_top_layer`])
/// whose `overlay` transition did not start or is over leave it now.
fn finish_overlay_exits(page: &Rc<PageDom>) {
    let leaving = match page.dom.try_borrow() {
        Ok(dom) => dom.top_layer_leaving(),
        Err(_) => return,
    };
    if leaving.is_empty() {
        return;
    }
    let done: Vec<NodeId> = {
        let transitions = page.animation_events.transitions.borrow();
        leaving
            .into_iter()
            .filter(|&el| !transitions.iter().any(|t| t.el == el && t.name == "overlay" && t.live.get() && !t.ended.get()))
            .collect()
    };
    if let Ok(mut dom) = page.dom.try_borrow_mut() {
        for el in done {
            dom.finish_top_layer_exit(el);
        }
    }
}

/// Script read layout (a box, an offset, a scroll position) of `el`: the
/// read lays the document out anyway, so the engine is synced whole first
/// and the style flush finds its styles current, instead of a style-only
/// sync right before the read's full one (two syncs per read of a changed
/// document).
pub(crate) fn layout_flush(page: &Rc<PageDom>, el: NodeId) {
    // pyro: an iframe showing a responsively embedded document is read at its content's size.
    page.sync_responsive_frames();
    if let (Ok(dom), Some(layout)) = (page.dom.try_borrow(), page.try_layout()) {
        if !page.is_disposed() && !page.scripting_disabled() {
            track_style_changes(&page.animation_events, &dom, layout.style());
        }
        layout.synced_geometry_source(&dom);
    }
    style_flush(page, Some(el));
}

/// Once transitions are tracked, the engine keeps the elements whose style a
/// restyle replaces ([`pyro_dom::style::StyleEngine::take_style_changes`]).
/// Set before every style sync, so that the first sync after a transition
/// listener appeared records its changes.
/// pyro: style changes are tracked before a layout outside the animation frames (the load gate's,
/// `PageDom::css_images_delay_load`) applies them: the transitions they start are not lost.
pub(crate) fn track_before_layout(page: &PageDom) {
    if let (Ok(dom), Some(layout)) = (page.dom.try_borrow(), page.try_layout()) {
        track_style_changes(&page.animation_events, &dom, layout.style());
    }
}

fn track_style_changes(state: &State, dom: &pyro_dom::Dom, style: &pyro_dom::style::StyleEngine) {
    if state.transitions_tracked(dom, style) {
        style.set_track_style_changes(true);
    }
}

/// A style change event of the elements whose computed style the engine
/// replaced since the last one: the transitions their before-change and
/// after-change styles start. Nothing to read when nothing changed.
fn transitions_from_changes(
    state: &State,
    dom: &pyro_dom::Dom,
    style: &pyro_dom::style::StyleEngine,
    now_ms: f64,
    planned: &mut Vec<Planned>,
) {
    drop_removed_hidden_roots(state, dom);
    cancel_orphaned_transitions(state, dom, style, now_ms, planned);
    let changes = style.take_style_changes(dom);
    // The elements whose used `anchor()`/`anchor-size()` px changed since the last style change
    // event, with their px then and now.
    let mut evals: FxHashMap<NodeId, ([Option<f32>; 10], [Option<f32>; 10])> =
        style.take_anchor_evals(dom).into_iter().map(|(el, before, after)| (el, (before, after))).collect();
    if changes.is_empty() && evals.is_empty() {
        return;
    }
    let rules = state.rules(dom, style);
    let mut starting = Starting::default();
    let mut hidden = FxHashSet::default();
    for (el, pseudo, before, now) in changes {
        // The document's own tree only: the rules are the document's (a
        // document rule does not style a shadow tree's elements).
        if !dom.in_document_tree(el) {
            continue;
        }
        // A `::before`/`::after` whose style changed while both styles generate a box (its
        // element's style change event is its own; one that comes or goes has no before-change
        // style, css-transitions-1 §3).
        if let Some(pseudo) = pseudo {
            let generates = pyro_dom::timing::generates_pseudo_box;
            if generates(&before) && generates(&now) && !displays_none_or_hidden(dom, style, el) {
                start_transitions_of(state, dom, style, &rules, el, Some(pseudo), &before, &now, now_ms, &mut starting);
            }
            continue;
        }
        let (before, now) = anchor_px_styles(style, el, before, now, evals.remove(&el));
        let (was_none, is_none) = (displays_none(&before), displays_none(&now));
        if !was_none && !is_none {
            start_transitions(state, dom, style, &rules, el, &before, &now, now_ms, &mut starting);
            continue;
        }
        // A box going `display: none` with a discrete `display` transition stays rendered while it
        // runs (css-display-4 §2.1, `transition-behavior: allow-discrete`): its transitions start.
        // (One without its own `display` change, inside such a box, has no rendered box to animate.)
        if !was_none && is_none && transitions_display(dom, style, &rules, el, &now) {
            start_transitions(state, dom, style, &rules, el, &before, &now, now_ms, &mut starting);
            continue;
        }
        // `display: none` at either style change event: not rendered then, so
        // no transitions (and no before-change style once shown), unless the
        // engine's `display: none` is Chrome's `content-visibility: hidden`
        // (a closed `<details>`'s content, `hidden=until-found`), which keeps
        // the style.
        let content = is_none && hides_content(dom, el);
        let was_content = was_none && !is_none && state.hidden_roots.borrow_mut().remove(&el);
        if content {
            keep_hidden_content(state, dom, style, &rules, el);
        } else if is_none {
            drop_hidden_content(state, dom, el);
            if !was_none {
                hidden.insert(el);
            }
        }
        if was_content {
            reveal_hidden_content(state, dom, style, &rules, el, now_ms, &mut starting);
        }
        if (content || !is_none) && (was_content || content || !was_none) {
            start_transitions(state, dom, style, &rules, el, &before, &now, now_ms, &mut starting);
        } else if was_none && !is_none && !was_content {
            // Shown from `display: none`: no before-change style, but its starting style
            // (`@starting-style`) stands for one (css-transitions-2 §3.2).
            if let Some(start) = style.starting_style(dom, el) {
                start_transitions(state, dom, style, &rules, el, &start, &now, now_ms, &mut starting);
            }
        }
    }
    // An element whose `anchor()`/`anchor-size()` px changed with no style change (its anchor
    // moved or resized, another element took the anchor name): its computed values changed, as
    // Chrome computes them to px (WPT css-anchor-position `anchor-transition-eval`, `-name`).
    for (el, (px_before, px_now)) in evals {
        if !dom.in_document_tree(el) || hidden.contains(&el) {
            continue;
        }
        let Some(now) = style.stylo_style(dom, el) else { continue };
        if displays_none(&now) || displays_none_or_hidden(dom, style, el) {
            continue;
        }
        let (before, now) = anchor_px_pair(&now, &now, &px_before, &px_now);
        start_transitions(state, dom, style, &rules, el, &before, &now, now_ms, &mut starting);
    }
    starting.commit(state, planned);
    if !hidden.is_empty() {
        let mut running = state.transitions.borrow_mut();
        let previous = running.len();
        running.retain(|transition| {
            if !hidden.iter().any(|&root| root == transition.el || is_ancestor(dom, root, transition.el)) {
                return true;
            }
            cancel_running(state, transition, now_ms, planned);
            false
        });
        if running.len() != previous {
            state.bump();
        }
    }
}

/// The before-change and after-change styles `before` and `now` of `el` with their `anchor()` and
/// `anchor-size()` insets and sizes computed to the px layout used then and now (`eval`: the px of
/// an element whose px changed since the last style change event; otherwise its px now, for both),
/// as Chrome computes them before it transitions: a changed `position-anchor` or anchor
/// transitions though the text of `top: anchor(top)` stays the same.
fn anchor_px_styles(
    style: &pyro_dom::style::StyleEngine,
    el: NodeId,
    before: StyleSnapshot,
    now: StyleSnapshot,
    eval: Option<([Option<f32>; 10], [Option<f32>; 10])>,
) -> (StyleSnapshot, StyleSnapshot) {
    let queries = |s: &StyleSnapshot| (0..10).any(|slot| pyro_dom::animation::anchor_slot_queries(s, slot));
    if eval.is_none() && !queries(&before) && !queries(&now) {
        return (before, now);
    }
    let Some((px_before, px_now)) = eval.or_else(|| style.anchor_used(el).map(|px| (px, px))) else {
        return (before, now);
    };
    anchor_px_pair(&before, &now, &px_before, &px_now)
}

/// [`anchor_px_styles`] with the px known: a slot is replaced in both styles, or (one of them
/// without its px) in neither.
fn anchor_px_pair(
    before: &StyleSnapshot,
    now: &StyleSnapshot,
    px_before: &[Option<f32>; 10],
    px_now: &[Option<f32>; 10],
) -> (StyleSnapshot, StyleSnapshot) {
    let queries = pyro_dom::animation::anchor_slot_queries;
    let both = |slot: usize| {
        (!queries(before, slot) || px_before[slot].is_some()) && (!queries(now, slot) || px_now[slot].is_some())
    };
    (
        pyro_dom::animation::with_anchor_px(before, px_before, both).unwrap_or_else(|| before.clone()),
        pyro_dom::animation::with_anchor_px(now, px_now, both).unwrap_or_else(|| now.clone()),
    )
}

/// Cancels running transition `transition` (unless it ended or was canceled already): its
/// `transitioncancel`, whose `elapsedTime` is how far its active phase got.
fn cancel_running(state: &State, transition: &Timeline, now_ms: f64, planned: &mut Vec<Planned>) {
    if transition.live.replace(false) && !transition.ended.get() {
        state.runs_canceled.set(true);
        let local = state.scripted.borrow().values().find(|effect| effect.css_id() == Some(transition.id))
            .and_then(|effect| effect.current(now_ms)).unwrap_or(now_ms - transition.start_ms);
        let mut event = cancel_event(transition.el, transition.pseudo, &transition.name);
        event.elapsed_s = (local - transition.delay_ms).clamp(0.0, transition.duration_ms) / 1000.0;
        planned.push(event);
    }
}

/// The running transitions whose target is no longer rendered where it started are canceled
/// (css-transitions-1 §3: "if an element is no longer in the document, implementations must
/// cancel any running transitions on it"; Chrome cancels them too when the element is moved, even
/// back under the same parent, and when its `::before`/`::after` stops generating a box):
/// - those of the elements removed from the tree (a subtree's) since the last style change
///   event, whether or not they were inserted again since;
/// - those of a pseudo-element its element no longer generates (`content: none`).
///
/// One counter check while nothing was removed and no pseudo-element transitions.
fn cancel_orphaned_transitions(
    state: &State,
    dom: &pyro_dom::Dom,
    style: &pyro_dom::style::StyleEngine,
    now_ms: f64,
    planned: &mut Vec<Planned>,
) {
    let removals = dom.removal_count();
    let since = state.transitions_removals.replace(removals);
    let Ok(mut running) = state.transitions.try_borrow_mut() else { return };
    if running.is_empty() {
        return;
    }
    // `None`: the log does not go back far enough; every element out of the document then.
    let removed: Option<Option<FxHashSet<NodeId>>> =
        (since != removals).then(|| dom.nodes_removed_since(since).map(|nodes| nodes.into_iter().collect()));
    let has_pseudo = running.iter().any(|t| t.pseudo.is_some() && t.live.get());
    if removed.is_none() && !has_pseudo {
        return;
    }
    let previous = running.len();
    running.retain(|transition| {
        let gone = match &removed {
            Some(Some(nodes)) => nodes.contains(&transition.el),
            Some(None) => !dom.is_connected(transition.el).unwrap_or(false),
            None => false,
        };
        let pseudo_gone = !gone
            && transition.live.get()
            && !transition.ended.get()
            && transition.pseudo.is_some_and(|pseudo| {
                dom.is_alive(transition.el)
                    && style
                        .pseudo_style(dom, transition.el, pseudo, false)
                        .is_none_or(|s| !pyro_dom::timing::generates_pseudo_box(&s))
            });
        if !gone && !pseudo_gone {
            return true;
        }
        cancel_running(state, transition, now_ms, planned);
        false
    });
    if running.len() != previous {
        state.bump();
    }
}

/// Whether `el` (after-change style `now`) runs a discrete transition of `display` (or `all`).
fn transitions_display(
    dom: &pyro_dom::Dom,
    style: &pyro_dom::style::StyleEngine,
    rules: &TimingRules,
    el: NodeId,
    now: &StyleSnapshot,
) -> bool {
    let style_of = |e: NodeId| if e == el { Some(now.clone()) } else { style.stylo_style(dom, e) };
    rules
        .element(dom, el, &style_of)
        .transitions
        .iter()
        .any(|t| t.allow_discrete && matches!(t.property.as_str(), "display" | "all"))
}

fn animation_display_suppressed(
    dom: &pyro_dom::Dom, style: &pyro_dom::style::StyleEngine, el: NodeId, may_sample_display: &impl Fn(NodeId) -> bool,
) -> bool {
    if style.displayed_as_synced(dom, el) != Some(false) {
        return false;
    }
    let mut current = Some(el);
    while let Some(target) = current {
        if let Some(snapshot) = style.stylo_style(dom, target).filter(displays_none) {
            let sampled = snapshot.rules.as_ref().is_some_and(|rules| {
                rules.self_and_ancestors().any(|rule| rule.cascade_level().is_animation())
            });
            let ordinary = !sampled || !may_sample_display(target);
            if ordinary && !hides_content(dom, target)
                && !matches!(style.transition_value(target, "display"), Some(pyro_dom::animation::TransitionValue::Text(value)) if value != "none") {
                return true;
            }
        }
        current = dom.assigned_slot_any(target).or_else(|| {
            let parent = dom.parent(target).ok()??;
            if dom.is_shadow_root(parent).unwrap_or(false) { dom.fragment_host(parent).ok().flatten() }
            else { dom.element(parent).ok().map(|_| parent) }
        });
    }
    false
}

/// Whether the engine's `display: none` of `el` is where Chrome has
/// `content-visibility: hidden`: a child of a closed `<details>` other than
/// its first `<summary>`, or a `hidden=until-found` element. Two attribute
/// reads, asked only of elements styled `display: none`.
fn hides_content(dom: &pyro_dom::Dom, el: NodeId) -> bool {
    use markup5ever::local_name;
    let Ok(e) = dom.element(el) else { return false };
    if until_found(e) {
        return true;
    }
    let Some(parent) = dom.parent_element(el).ok().flatten() else { return false };
    let Ok(p) = dom.element(parent) else { return false };
    if !p.is_html() || p.name.local != local_name!("details") || p.attr_local(&local_name!("open")).is_some() {
        return false;
    }
    let is_summary =
        |id: &NodeId| dom.element(*id).is_ok_and(|c| c.is_html() && c.name.local == local_name!("summary"));
    let first_summary = dom.children(parent).ok().and_then(|mut c| c.find(is_summary));
    first_summary != Some(el)
}

/// A `hidden=until-found` element: its own box is rendered in Chrome
/// (`content-visibility: hidden` hides what it contains).
fn until_found(e: &pyro_dom::node::ElementData) -> bool {
    e.attr_local(&markup5ever::local_name!("hidden")).is_some_and(|v| v.eq_ignore_ascii_case("until-found"))
}

/// `root` became hidden content: the elements under it that may transition
/// keep their style as it is now as their before-change style (the engine
/// computes it on demand; it styles none of them while `root` is hidden).
/// An element already kept keeps its older style.
fn keep_hidden_content(
    state: &State,
    dom: &pyro_dom::Dom,
    style: &pyro_dom::style::StyleEngine,
    rules: &TimingRules,
    root: NodeId,
) {
    state.hidden_roots.borrow_mut().insert(root);
    let Ok(descendants) = dom.descendants(root) else { return };
    let mut kept = state.hidden_content.borrow_mut();
    for d in descendants.filter(|d| *d != root) {
        let style_of = |e: NodeId| style.stylo_style(dom, e);
        if kept.contains_key(&d) || dom.element(d).is_err() || rules.element(dom, d, &style_of).transitions.is_empty() {
            continue;
        }
        if let Some(snapshot) = style.stylo_style(dom, d) {
            kept.insert(d, (root, snapshot));
        }
    }
}

/// Script read `el` (its style or layout) while it is hidden content (under
/// a root of [`hides_content`] the engine styles `display: none`): Chrome
/// styles such content when script reads it (a forced update of the
/// `content-visibility: hidden` subtree), and that style is its
/// before-change style once the root is shown. So the root's content keeps
/// its style as now ([`keep_hidden_content`]), as when the root was seen
/// becoming hidden; on later reads, only `el` joins it. A read of a
/// `hidden=until-found` root itself styles none of its content (Chrome lays
/// the root out with its content skipped), while a closed `<details>`'s
/// content root is inside what Chrome hides. Content hidden from
/// its first style on is never seen becoming hidden (a first style is no
/// style change), so this is where it gets one. Under a real `display: none`
/// nothing is kept. One engine lookup for a rendered element; a walk up to
/// the hidden subtree's top for a hidden one.
fn style_read_hidden_content(state: &State, dom: &pyro_dom::Dom, style: &pyro_dom::style::StyleEngine, el: NodeId) {
    // Idempotent while the element and the styles stay the same.
    let read = (el, style.observation_stamp(dom));
    if state.hidden_read.replace(Some(read)) == Some(read) {
        return;
    }
    if style.engine_displayed(dom, el) != Some(false) {
        return;
    }
    let Some(top) = hidden_content_root(dom, style, el) else { return };
    if el == top && dom.element(top).is_ok_and(until_found) {
        return;
    }
    let rules = state.rules(dom, style);
    if !state.hidden_roots.borrow().contains(&top) {
        keep_hidden_content(state, dom, style, &rules, top);
        return;
    }
    let style_of = |e: NodeId| style.stylo_style(dom, e);
    if el == top
        || state.hidden_content.borrow().contains_key(&el)
        || rules.element(dom, el, &style_of).transitions.is_empty()
    {
        return;
    }
    if let Some(snapshot) = style.stylo_style(dom, el) {
        state.hidden_content.borrow_mut().insert(el, (top, snapshot));
    }
}

/// The top of the engine's `display: none` subtree holding `el` (not
/// displayed), if it is a content root ([`hides_content`]): the nearest
/// content root, by its attributes, whose parent the engine displays. The
/// engine is asked only at content roots, so for an element under a plain
/// `display: none` this is a walk over attributes.
fn hidden_content_root(dom: &pyro_dom::Dom, style: &pyro_dom::style::StyleEngine, el: NodeId) -> Option<NodeId> {
    let mut at = Some(el);
    let top = loop {
        let x = at?;
        let parent = dom.parent_element(x).ok().flatten();
        if hides_content(dom, x) {
            match parent.map_or(Some(true), |p| style.engine_displayed(dom, p)) {
                Some(true) => break x,
                Some(false) => {}
                None => return None,
            }
        }
        at = parent;
    };
    style.stylo_style(dom, top).is_some_and(|s| displays_none(&s)).then_some(top)
}

/// Drops the hidden roots that left the document since the last style
/// change event, and their content's kept styles. One counter check while
/// nothing was removed.
fn drop_removed_hidden_roots(state: &State, dom: &pyro_dom::Dom) {
    let removals = dom.removal_count();
    if state.hidden_roots_removals.replace(removals) == removals || state.hidden_roots.borrow().is_empty() {
        return;
    }
    let mut roots = state.hidden_roots.borrow_mut();
    let before = roots.len();
    roots.retain(|r| dom.in_document_tree(*r));
    if roots.len() != before {
        state.hidden_content.borrow_mut().retain(|d, (r, _)| dom.is_alive(*d) && roots.contains(r));
    }
}

/// `el` is now really `display: none`: the hidden content it holds (or is)
/// is no longer rendered and loses its before-change styles.
fn drop_hidden_content(state: &State, dom: &pyro_dom::Dom, el: NodeId) {
    state.hidden_roots.borrow_mut().remove(&el);
    let mut kept = state.hidden_content.borrow_mut();
    if !kept.is_empty() {
        kept.retain(|d, (root, _)| dom.is_alive(*d) && *root != el && !is_ancestor(dom, el, *root));
    }
}

/// The hidden content root `root` is shown: its elements' kept styles meet
/// the styles the engine gave them afresh.
fn reveal_hidden_content(
    state: &State,
    dom: &pyro_dom::Dom,
    style: &pyro_dom::style::StyleEngine,
    rules: &TimingRules,
    root: NodeId,
    now_ms: f64,
    starting: &mut Starting,
) {
    let revealed: Vec<(NodeId, StyleSnapshot)> = {
        let mut kept = state.hidden_content.borrow_mut();
        let mut out = Vec::new();
        kept.retain(|d, (r, snapshot)| {
            if *r != root {
                return dom.is_alive(*d);
            }
            out.push((*d, snapshot.clone()));
            false
        });
        out
    };
    for (d, before) in revealed {
        if !dom.in_document_tree(d) || !is_ancestor(dom, root, d) {
            continue;
        }
        // Not rendered yet: still hidden content of a root inside this one
        // (it keeps its style there), or under a real `display: none`.
        if style.engine_displayed(dom, d) != Some(true) {
            if let Some(inner) = hidden_content_root(dom, style, d).filter(|inner| *inner != d) {
                state.hidden_roots.borrow_mut().insert(inner);
                state.hidden_content.borrow_mut().entry(d).or_insert((inner, before));
            }
            continue;
        }
        if let Some(now) = style.stylo_style(dom, d).filter(|now| !displays_none(now)) {
            start_transitions(state, dom, style, rules, d, &before, &now, now_ms, starting);
        }
    }
}

/// Whether `ancestor` is a proper ancestor of `node`.
fn is_ancestor(dom: &pyro_dom::Dom, ancestor: NodeId, node: NodeId) -> bool {
    let mut at = dom.parent(node).ok().flatten();
    while let Some(p) = at {
        if p == ancestor {
            return true;
        }
        at = dom.parent(p).ok().flatten();
    }
    false
}

/// The transitions one style change event starts. The ones of an element
/// and property under way are replaced in one pass over the running
/// transitions ([`Starting::commit`]): a page whose rules transition starts
/// hundreds per frame, and a pass per start cost more than the restyle.
#[derive(Default)]
struct Starting {
    changed: FxHashMap<(NodeId, Option<PseudoElement>), (StyleSnapshot, Vec<String>)>,
    timelines: Vec<Timeline>,
    /// The properties started, by element, with their index in `timelines`.
    keys: FxHashMap<(NodeId, Option<PseudoElement>), Vec<(String, usize)>>,
    /// Their events, planned after the cancel events of the transitions
    /// they replace.
    events: Vec<Planned>,
    /// The running transitions canceled ([`start_transitions_of`]): those whose end value the
    /// after-change style no longer has, none replacing them, and those whose current value
    /// already is the after-change value.
    ended: FxHashSet<u64>,
}

fn cancel_event(el: NodeId, pseudo: Option<PseudoElement>, name: &str) -> Planned {
    Planned {
        el,
        pseudo: pseudo_name(pseudo),
        ty: "transitioncancel",
        transition: true,
        name: name.to_owned(),
        elapsed_s: 0.0,
        after: Duration::ZERO,
        live: Rc::new(Cell::new(true)),
        ends: None,
        epoch: None,
        step: None,
    }
}

impl Starting {
    /// Replaces the running transitions of the elements and properties that
    /// start again, and starts the new ones.
    fn commit(self, state: &State, planned: &mut Vec<Planned>) {
        if self.timelines.is_empty() && self.changed.is_empty() && self.ended.is_empty() {
            return;
        }
        let mut running = state.transitions.borrow_mut();
        let previous_count = running.len();
        running.retain(|known| {
            let same = self
                .keys
                .get(&(known.el, known.pseudo))
                .is_some_and(|started| started.iter().any(|(name, _)| *name == known.name));
            let unlisted = self.changed.get(&(known.el, known.pseudo))
                .is_some_and(|(style, properties)| !pyro_dom::timing::lists_transition(style, properties, &known.name));
            let ended = self.ended.contains(&known.id);
            if (same || unlisted || ended) && known.live.get() && !known.ended.get() {
                known.live.set(false);
                state.runs_canceled.set(true);
                planned.push(cancel_event(known.el, known.pseudo, &known.name));
            }
            !same && !unlisted && !ended
        });
        if running.len() == previous_count && self.timelines.is_empty() { return; }
        planned.extend(self.events);
        running.extend(self.timelines);
        state.bump();
    }
}

/// The transitions a style change of `el` from `before` to `now` starts: the
/// properties of its transition list whose computed value differs. A
/// transition of the property still under way is replaced.
#[allow(clippy::too_many_arguments)]
fn start_transitions(
    state: &State,
    dom: &pyro_dom::Dom,
    style: &pyro_dom::style::StyleEngine,
    rules: &TimingRules,
    el: NodeId,
    before: &StyleSnapshot,
    now: &StyleSnapshot,
    now_ms: f64,
    starting: &mut Starting,
) {
    start_transitions_of(state, dom, style, rules, el, None, before, now, now_ms, starting);
}

/// [`start_transitions`] of `el`'s `pseudo` (`None`: of `el`).
#[allow(clippy::too_many_arguments)]
fn start_transitions_of(
    state: &State,
    dom: &pyro_dom::Dom,
    style: &pyro_dom::style::StyleEngine,
    rules: &TimingRules,
    el: NodeId,
    pseudo: Option<PseudoElement>,
    before: &StyleSnapshot,
    now: &StyleSnapshot,
    now_ms: f64,
    starting: &mut Starting,
) {
    if StyleSnapshot::ptr_eq(before, now) {
        return;
    }
    // The after-change style's custom properties resolve `var()` (a pseudo-element's: the
    // element's).
    let style_of = |e: NodeId| if e == el && pseudo.is_none() { Some(now.clone()) } else { style.stylo_style(dom, e) };
    let ElementTiming { transitions, transition_properties, transition_claims, .. } =
        rules.element_for(dom, el, pseudo, &style_of);
    if !state.transitions.borrow().is_empty() {
        if let Some(properties) = transition_properties {
            starting.changed.insert((el, pseudo), (now.clone(), properties));
        }
    }
    // Whether an item of `transition-property` after `index` names `longhand` (changed): that item
    // decides its transition, even one that starts none (a zero combined duration, or `normal`
    // behavior for a value that does not interpolate).
    let claimed_later = |index: usize, longhand: &str| {
        transition_claims.iter().skip(index + 1).flatten().any(|item| {
            item == "all"
                || item == longhand
                || pyro_dom::timing::changed_longhands_with(before, now, item, true).contains(&longhand)
        })
    };
    let mut started = Vec::new();
    for transition in transitions.iter().rev() {
        let custom = if transition.property == "all" {
            pyro_dom::timing::changed_custom_properties(before, now, transition.allow_discrete)
        } else { Vec::new() };
        let changed =
            pyro_dom::timing::changed_longhands_with(before, now, &transition.property, transition.allow_discrete);
        for longhand in changed.into_iter().chain(custom.iter().map(String::as_str)) {
            if started.iter().any(|name| name == longhand) || claimed_later(transition.index, longhand) {
                continue;
            }
            started.push(longhand.to_owned());
            // A property a CSS or script animation drives does not
            // transition: its value changes with the animation's.
            if crate::animation_frame::animates(state, rules, el, pseudo, longhand) {
                continue;
            }
            // Interrupting a running transition of the property: the new one starts from its
            // current value, and none starts when that already is the after-change value (the
            // running one is canceled; css-transitions-1 §3).
            let mut from = before.clone();
            if let Some((id, sample)) = interrupted_value(state, el, pseudo, longhand, now_ms, style.app_units_per_device_pixel()) {
                if pyro_dom::animation::same_computed_value(longhand, &sample, now) {
                    starting.ended.insert(id);
                    continue;
                }
                from = sample;
            }
            // Started already in this style change event (an element of a
            // revealed root that changed too): that one is replaced.
            let index = starting.timelines.len();
            let keys = starting.keys.entry((el, pseudo)).or_default();
            match keys.iter_mut().find(|(name, _)| *name == longhand) {
                Some((_, first)) => {
                    starting.timelines[*first].live.set(false);
                    starting.events.push(cancel_event(el, pseudo, longhand));
                    *first = index;
                }
                None => keys.push((longhand.to_owned(), index)),
            }
            // Reversing a running transition of the property (the after-change value is its
            // reversing-adjusted start value): shortened by how far it got (css-transitions-1 §3;
            // WPT `CSSTransition-currentTime`, `CSSTransition-effect`).
            let (mut delay_ms, mut duration_ms, mut reversing) = (transition.delay_ms, transition.duration_ms, None);
            if let Some((factor, start)) = reversed_portion(state, el, pseudo, longhand, now, now_ms) {
                duration_ms *= factor;
                if delay_ms < 0.0 {
                    delay_ms *= factor;
                }
                reversing = Some((start, factor));
            }
            let timeline = Timeline {
                timeline_text: None,
                id: state.next_id(),
                start_ms: now_ms,
                generation: state.style_events.get(),
                delay_ms,
                duration_ms,
                iterations: 1.0,
                el,
                pseudo,
                driven: false,
                name: longhand.to_owned(),
                live: Rc::new(Cell::new(true)),
                ended: Rc::new(Cell::new(false)),
                timing: None,
                paused_at: None,
                epoch: Rc::new(Cell::new(true)),
                progress: Rc::new(Cell::new(0)),
                easing: transition.easing.clone(),
                keyframes: pyro_dom::timing::transition_keyframes(&from, now, longhand).map(std::sync::Arc::new),
                styles: Some((from, now.clone())),
                reversing,
            };
            let (delay, duration) = (delay_ms, duration_ms);
            // (`transitionstart` and `transitionend` belong to the transition's plan, which script
            // replaces through its `CSSTransition`: `scripted_state`.)
            let event = |ty: &'static str, at_ms: f64, elapsed_ms: f64, step: Option<u32>| Planned {
                el,
                pseudo: pseudo_name(pseudo),
                ty,
                transition: true,
                name: longhand.to_owned(),
                elapsed_s: elapsed_ms / 1000.0,
                after: after(at_ms),
                live: timeline.live.clone(),
                ends: (step == Some(ENDED)).then(|| timeline.ended.clone()),
                epoch: step.map(|_| timeline.epoch.clone()),
                step: step.map(|step| (timeline.progress.clone(), step)),
            };
            starting.events.push(event("transitionrun", 0.0, 0.0, None));
            starting.events.push(event("transitionstart", delay, (-delay).max(0.0), Some(1)));
            starting.events.push(event("transitionend", delay + duration, duration, Some(ENDED)));
            starting.timelines.push(timeline);
        }
    }
    // A running transition of a property no transition started for now whose after-change value
    // is no longer its end value is canceled (css-transitions-1 §3: a zero combined duration, a
    // value it cannot interpolate to, `transition-behavior` back to `normal` for a discrete one;
    // WPT `CSSTransition-canceling`, `transition-behavior`).
    if let Ok(running) = state.transitions.try_borrow() {
        for known in running.iter() {
            if known.el != el || known.pseudo != pseudo || !known.live.get() || known.ended.get() {
                continue;
            }
            if started.iter().any(|name| *name == known.name) {
                continue;
            }
            let Some((_, end)) = &known.styles else { continue };
            if pyro_dom::timing::end_value_differs(end, now, &known.name) {
                starting.ended.insert(known.id);
            }
        }
    }
}

/// When the after-change style `now` of `el`'s `pseudo` reverses its running transition of
/// `longhand` (gives it back that transition's reversing-adjusted start value), the new
/// transition's reversing shortening factor and its reversing-adjusted start style (the running
/// one's end): `|portion × old factor + 1 − old factor|`, at most 1, where `portion` is the running
/// transition's eased progress at its current time (script's, once it set one).
fn reversed_portion(
    state: &State,
    el: NodeId,
    pseudo: Option<PseudoElement>,
    longhand: &str,
    now: &StyleSnapshot,
    now_ms: f64,
) -> Option<(f64, StyleSnapshot)> {
    let running = state.transitions.try_borrow().ok()?;
    let known = running.iter().find(|t| {
        t.el == el && t.pseudo == pseudo && t.name == longhand && t.live.get() && !t.ended.get()
    })?;
    let (before, end) = known.styles.as_ref()?;
    let (start, old_factor) = match &known.reversing {
        Some((start, factor)) => (start, *factor),
        None => (before, 1.0),
    };
    if pyro_dom::timing::end_value_differs(start, now, longhand) {
        return None;
    }
    let local = state
        .scripted
        .try_borrow()
        .ok()
        .and_then(|s| s.values().find(|e| e.css_id() == Some(known.id)).and_then(|e| e.current(now_ms)))
        .unwrap_or(now_ms - known.start_ms);
    let progress = if known.duration_ms > 0.0 {
        ((local - known.delay_ms) / known.duration_ms).clamp(0.0, 1.0)
    } else {
        1.0
    };
    let portion = pyro_dom::animation::eased_progress(progress, false, known.easing.as_deref().unwrap_or("ease"));
    let factor = (portion * old_factor + 1.0 - old_factor).abs().min(1.0);
    Some((factor, end.clone()))
}

/// The running transition of `el`'s `pseudo`'s `longhand` that a new one interrupts and its
/// current value (`au_per_dev_px`: the engine's device pixel), when that value is the transition's
/// own ([`crate::animation_frame::interrupted_transition`]).
fn interrupted_value(
    state: &State,
    el: NodeId,
    pseudo: Option<PseudoElement>,
    longhand: &str,
    now_ms: f64,
    au_per_dev_px: i32,
) -> Option<(u64, StyleSnapshot)> {
    let running = state.transitions.try_borrow().ok()?;
    let known = running.iter().find(|t| {
        t.el == el && t.pseudo == pseudo && t.name == longhand && t.live.get() && !t.ended.get()
    })?;
    let (from, to) = known.styles.as_ref()?;
    let (portion, original) = crate::animation_frame::interrupted_transition(state, known, now_ms)?;
    if !original {
        return None;
    }
    match pyro_dom::animation::transition_value(longhand, from, to, portion, false, Some("linear"), au_per_dev_px)? {
        pyro_dom::animation::TransitionValue::Style(sample) => Some((known.id, sample)),
        _ => None,
    }
}

/// Fires the events due now (as a task of their own when `defer`) and sets a
/// timer for each later one; equal delays keep their order.
fn schedule(page: &Rc<PageDom>, planned: Vec<Planned>, defer: bool) {
    // Nobody can observe an event without a listener for its kind: the
    // timelines run for the values they sample, not for events nobody hears
    // (a page whose rules transition starts dozens per frame).
    let (animation_heard, transition_heard) = match page.dom.try_borrow() {
        Ok(dom) => (dom.has_animation_listeners(), dom.has_transition_listeners()),
        Err(_) => (true, true),
    };
    for event in planned {
        if !(if event.transition { transition_heard } else { animation_heard }) {
            continue;
        }
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
/// the element left the document meanwhile (a `transitioncancel` still reaches it: removing an
/// element is what cancels its transitions, and Chrome fires that event at it).
fn fire(page: &Rc<PageDom>, event: &Planned) {
    if !event.live.get() || event.epoch.as_ref().is_some_and(|e| !e.get()) || page.is_disposed() {
        return;
    }
    {
        let dom = page.dom.borrow();
        let detached_cancel = event.ty == "transitioncancel";
        if !dom.is_alive(event.el) || !(detached_cancel || dom.is_connected(event.el).unwrap_or(false)) {
            return;
        }
    }
    if let Some(ended) = &event.ends {
        ended.set(true);
    }
    // An `overlay` transition that ends lets its element leave the top layer.
    if event.transition && event.name == "overlay" && matches!(event.ty, "transitionend" | "transitioncancel") {
        if let Ok(mut dom) = page.dom.try_borrow_mut() {
            dom.finish_top_layer_exit(event.el);
        }
    }
    if let Some((progress, step)) = &event.step {
        progress.set(progress.get().max(*step));
    }
    let interface = if event.transition { EventInterface::TransitionEvent } else { EventInterface::AnimationEvent };
    let ev = EventCore::trusted(page, interface, event.ty, true, false);
    {
        let mut fields = ev.fields.borrow_mut();
        if event.transition {
            fields.property_name = shown_property(&event.name).to_owned();
        } else {
            fields.animation_name = event.name.clone();
        }
        fields.elapsed_time = event.elapsed_s;
        fields.pseudo_element = event.pseudo.to_owned();
    }
    crate::target::fire_at_node(page, event.el, &ev);
}

/// The name a transition of longhand `name` shows (`propertyName`, `transitionProperty`): Stylo's
/// `vertical-align` is a shorthand of `baseline-shift` (and two discrete longhands), Chrome's a
/// longhand, whose transitions it names `vertical-align` (WPT css-transitions
/// `properties-value-*` 'vertical-align … / events').
fn shown_property(name: &str) -> &str {
    match name {
        "baseline-shift" => "vertical-align",
        _ => name,
    }
}

/// pyro: a scroll-driven CSS animation's `animation-timeline` and range, serialized.
fn timeline_text_of(
    style: &pyro_dom::style::StyleEngine,
    dom: &pyro_dom::Dom,
    el: NodeId,
    pseudo: Option<PseudoElement>,
    index: usize,
) -> Option<(String, String, String)> {
    let style = style_of_target(style, dom, el, pseudo)?;
    let (start, end) = pyro_dom::timing::range_text(&style, index);
    Some((pyro_dom::timing::timeline_text(&style, index), start, end))
}

/// `pseudoElement` of an event of a timeline whose target is `pseudo`.
fn pseudo_name(pseudo: Option<PseudoElement>) -> &'static str {
    match pseudo {
        None => "",
        Some(PseudoElement::Before) => "::before",
        Some(PseudoElement::After) => "::after",
        Some(PseudoElement::Marker) => "::marker",
    }
}

/// Whether `el` is not rendered (`display: none` or no style): its pseudo-elements have no boxes.
fn displays_none_or_hidden(dom: &pyro_dom::Dom, style: &pyro_dom::style::StyleEngine, el: NodeId) -> bool {
    style.stylo_style(dom, el).is_none_or(|s| displays_none(&s))
}

/// The style of an animation's target: the element's, or its `pseudo`'s.
fn style_of_target(
    style: &pyro_dom::style::StyleEngine,
    dom: &pyro_dom::Dom,
    el: NodeId,
    pseudo: Option<PseudoElement>,
) -> Option<StyleSnapshot> {
    match pseudo {
        None => style.stylo_style(dom, el),
        Some(p) => style.pseudo_style(dom, el, p, false),
    }
}

fn timeline_driven_now(style: &Option<StyleSnapshot>, index: usize) -> bool {
    style.as_ref().is_some_and(|s| pyro_dom::timing::timeline_driven(s, index))
}
