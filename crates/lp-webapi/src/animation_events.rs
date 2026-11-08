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

use lp_dom::timing::{displays_none, ElementTiming, StyleSnapshot, TimingRules};
use lp_dom::NodeId;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::event::{EventCore, EventInterface};
use crate::page::PageDom;

/// How long after a change the document's animations are looked at again.
const FRAME: Duration = Duration::from_millis(16);
/// Iteration events one animation may schedule.
const MAX_ITERATIONS: usize = 1024;

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
    /// Script called `getAnimations()`: timelines are kept without listeners.
    listed: Cell<bool>,
    /// The last timeline id handed out.
    ids: Cell<u64>,
    /// Content roots seen styled `display: none` where Chrome has
    /// `content-visibility: hidden` (see [`hides_content`]): their content
    /// keeps a before-change style while hidden.
    hidden_roots: RefCell<FxHashSet<NodeId>>,
    /// The before-change styles of the elements under such a root that may
    /// transition, by element, with their root: the engine styles none of
    /// them while the root is hidden, and styles them afresh once shown.
    hidden_content: RefCell<FxHashMap<NodeId, (NodeId, StyleSnapshot)>>,
    /// `Dom::removal_count` when `hidden_roots` last dropped the roots that
    /// left the document: a removed subtree loses its styles (Chrome), so a
    /// root removed and inserted again has no before-change styles, and a
    /// removed root does not keep its content's styles until navigation.
    hidden_roots_removals: Cell<u64>,
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
        self.rules.borrow_mut().take();
        self.hidden_roots.borrow_mut().clear();
        self.hidden_content.borrow_mut().clear();
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

/// A frame: the document's animations and the transitions its style changes start.
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
        track_style_changes(state, &dom, style);
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
        // Style change events of the elements whose style changed.
        if state.transitions_tracked(&dom) {
            transitions_from_changes(state, &dom, style, now_ms, &mut planned);
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
    let mut planned = Vec::new();
    {
        let Ok(dom) = page.dom.try_borrow() else { return };
        if !state.transitions_tracked(&dom) || page.is_disposed() || page.scripting_disabled() {
            return;
        }
        let Some(layout) = page.try_layout() else { return };
        let style = layout.style();
        track_style_changes(state, &dom, style);
        // The flush itself: syncing styles registers new `<style>` sheets.
        style.engine_styled(&dom);
        let now_ms = crate::event::page_now_ms(page);
        transitions_from_changes(state, &dom, style, now_ms, &mut planned);
        if let Some(el) = read {
            style_read_hidden_content(state, &dom, style, el);
        }
    }
    schedule(page, planned, true);
}

/// Script read layout (a box, an offset, a scroll position) of `el`: the
/// read lays the document out anyway, so the engine is synced whole first
/// and the style flush finds its styles current, instead of a style-only
/// sync right before the read's full one (two syncs per read of a changed
/// document).
pub(crate) fn layout_flush(page: &Rc<PageDom>, el: NodeId) {
    if let (Ok(dom), Some(layout)) = (page.dom.try_borrow(), page.try_layout()) {
        if !page.is_disposed() && !page.scripting_disabled() {
            track_style_changes(&page.animation_events, &dom, layout.style());
        }
        layout.synced_geometry_source(&dom);
    }
    style_flush(page, Some(el));
}

/// Once transitions are tracked, the engine keeps the elements whose style a
/// restyle replaces ([`lp_dom::style::StyleEngine::take_style_changes`]).
/// Set before every style sync, so that the first sync after a transition
/// listener appeared records its changes.
fn track_style_changes(state: &State, dom: &lp_dom::Dom, style: &lp_dom::style::StyleEngine) {
    if state.transitions_tracked(dom) {
        style.set_track_style_changes(true);
    }
}

/// A style change event of the elements whose computed style the engine
/// replaced since the last one: the transitions their before-change and
/// after-change styles start. Nothing to read when nothing changed.
fn transitions_from_changes(
    state: &State,
    dom: &lp_dom::Dom,
    style: &lp_dom::style::StyleEngine,
    now_ms: f64,
    planned: &mut Vec<Planned>,
) {
    drop_removed_hidden_roots(state, dom);
    let changes = style.take_style_changes(dom);
    if changes.is_empty() {
        return;
    }
    let rules = state.rules(dom, style);
    for (el, before, now) in changes {
        // The document's own tree only: the rules are the document's (a
        // document rule does not style a shadow tree's elements).
        if !dom.in_document_tree(el) {
            continue;
        }
        let (was_none, is_none) = (displays_none(&before), displays_none(&now));
        if !was_none && !is_none {
            start_transitions(state, dom, &rules, el, &before, &now, now_ms, planned);
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
        }
        if was_content {
            reveal_hidden_content(state, dom, style, &rules, el, now_ms, planned);
        }
        if (content || !is_none) && (was_content || content || !was_none) {
            start_transitions(state, dom, &rules, el, &before, &now, now_ms, planned);
        }
    }
}

/// Whether the engine's `display: none` of `el` is where Chrome has
/// `content-visibility: hidden`: a child of a closed `<details>` other than
/// its first `<summary>`, or a `hidden=until-found` element. Two attribute
/// reads, asked only of elements styled `display: none`.
fn hides_content(dom: &lp_dom::Dom, el: NodeId) -> bool {
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
fn until_found(e: &lp_dom::node::ElementData) -> bool {
    e.attr_local(&markup5ever::local_name!("hidden")).is_some_and(|v| v.eq_ignore_ascii_case("until-found"))
}

/// `root` became hidden content: the elements under it that may transition
/// keep their style as it is now as their before-change style (the engine
/// computes it on demand; it styles none of them while `root` is hidden).
/// An element already kept keeps its older style.
fn keep_hidden_content(
    state: &State,
    dom: &lp_dom::Dom,
    style: &lp_dom::style::StyleEngine,
    rules: &TimingRules,
    root: NodeId,
) {
    state.hidden_roots.borrow_mut().insert(root);
    let Ok(descendants) = dom.descendants(root) else { return };
    let mut kept = state.hidden_content.borrow_mut();
    for d in descendants.filter(|d| *d != root) {
        if kept.contains_key(&d) || dom.element(d).is_err() || rules.element(dom, d).transitions.is_empty() {
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
fn style_read_hidden_content(state: &State, dom: &lp_dom::Dom, style: &lp_dom::style::StyleEngine, el: NodeId) {
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
    if el == top || state.hidden_content.borrow().contains_key(&el) || rules.element(dom, el).transitions.is_empty() {
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
fn hidden_content_root(dom: &lp_dom::Dom, style: &lp_dom::style::StyleEngine, el: NodeId) -> Option<NodeId> {
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
fn drop_removed_hidden_roots(state: &State, dom: &lp_dom::Dom) {
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
fn drop_hidden_content(state: &State, dom: &lp_dom::Dom, el: NodeId) {
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
    dom: &lp_dom::Dom,
    style: &lp_dom::style::StyleEngine,
    rules: &TimingRules,
    root: NodeId,
    now_ms: f64,
    planned: &mut Vec<Planned>,
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
            start_transitions(state, dom, rules, d, &before, &now, now_ms, planned);
        }
    }
}

/// Whether `ancestor` is a proper ancestor of `node`.
fn is_ancestor(dom: &lp_dom::Dom, ancestor: NodeId, node: NodeId) -> bool {
    let mut at = dom.parent(node).ok().flatten();
    while let Some(p) = at {
        if p == ancestor {
            return true;
        }
        at = dom.parent(p).ok().flatten();
    }
    false
}

/// The transitions a style change of `el` from `before` to `now` starts: the
/// properties of its transition list whose computed value differs. A
/// transition of the property still under way is replaced.
#[allow(clippy::too_many_arguments)]
fn start_transitions(
    state: &State,
    dom: &lp_dom::Dom,
    rules: &TimingRules,
    el: NodeId,
    before: &StyleSnapshot,
    now: &StyleSnapshot,
    now_ms: f64,
    planned: &mut Vec<Planned>,
) {
    if StyleSnapshot::ptr_eq(before, now) {
        return;
    }
    let ElementTiming { transitions, .. } = rules.element(dom, el);
    let mut started: Vec<&'static str> = Vec::new();
    for transition in &transitions {
        for longhand in lp_dom::timing::changed_longhands(before, now, &transition.property) {
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
