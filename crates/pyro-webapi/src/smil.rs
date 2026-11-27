//! SMIL `<animate>` and `<animateTransform>` (SVG animation elements).
//!
//! Two consumers read the animations. CSS properties are sampled with the
//! page's other animations ([`crate::animation_frame`]): each `<animate>` in
//! effect is a keyframe effect on its target, below the CSS animations of the
//! same element (Chrome cascades SMIL-animated properties under CSS
//! animations). Every other attribute is read through [`animated`]: the SVG
//! `animVal` getters (`collections.rs`) fold the animations of an
//! (element, attribute) over its base value at the document's SMIL time, as
//! the SMIL "sandwich" does (SMIL Animation 3.3 and 3.4).
//!
//! What is read: the target (`href`, `xlink:href`, else the parent),
//! `attributeName`, `values` or `from`, `to` and `by`, `keyTimes`, `calcMode`
//! (`discrete`, `linear`, `paced` and `spline`, `keySplines` giving each
//! interval's easing), `additive`, `accumulate`, `begin` (its offset values,
//! the earliest), `dur`, `repeatCount`, `repeatDur` and `fill="freeze"`. The
//! document's animation time starts at its load event, as Chrome starts the
//! SVG time containers of a document. `<set>` is an animation of its `to`
//! value, with an indefinite simple duration unless `dur` is given.
//!
//! Timing (SMIL Animation 3.6, "Interval timing"): every animation element
//! (`<animateMotion>` too) has a [`Timed`] reading of `begin`, `end`, `dur`,
//! `repeatCount`, `repeatDur`, `min`, `max` and `restart`. Its `begin` and `end`
//! lists are instance times (offsets, syncbase values `id.begin+1s`, `indefinite`,
//! and the times at which `beginElement()` or an event-base event happened), from
//! which [`resolve_all`] builds the element's list of [`Interval`]s. The intervals
//! do not depend on the time asked about, so one resolution serves every sample
//! until the document or an instance time changes. `walk` and `animated` sample
//! the animation function in the latest interval that began.
//!
//! Events: [`pump`] compares the document's SMIL time with the time it last ran
//! at and dispatches `beginEvent`, `repeatEvent` and `endEvent` at the elements
//! whose intervals began, repeated or ended in between. A seek (`setCurrentTime`)
//! dispatches only for the changes of which interval is active: intervals it
//! skipped dispatch nothing, and neither does a repeat it jumped over.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use markup5ever::ns;
use pyro_dom::animation::{Composite, Effect, Keyframes, TextKeyframe, TextKeyframes};
use pyro_dom::{Dom, NodeId};

use crate::event::{EventCore, EventInterface};
use crate::page::PageDom;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Animate,
    Transform,
    /// `<set>`: its `to` for the whole active duration (an indefinite simple duration without `dur`).
    Set,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CalcMode {
    Linear,
    Discrete,
    Paced,
    Spline,
}

/// What the CSS-property path samples: the values as declarations.
struct Css {
    values: Vec<String>,
    /// Each value's time, a fraction of the simple duration.
    key_times: Vec<f64>,
    /// Each interval's easing (`calcMode="spline"`).
    key_splines: Option<Vec<String>>,
    discrete: bool,
    /// The values add to the value below (`by` alone).
    additive: bool,
}

/// One animation element as its attributes describe it.
pub(crate) struct Animate {
    target: NodeId,
    /// The animated attribute or property, as `attributeName` names it.
    property: String,
    kind: Kind,
    /// The CSS-property reading (`<animate>` only; `None` where it is invalid).
    css: Option<Css>,
    /// The element's timing, an index into [`Read::timed`] (and [`Read`]'s resolved intervals).
    timed: usize,
    dur_ms: f64,
    freeze: bool,
    calc: CalcMode,
    additive_sum: bool,
    accumulate_sum: bool,
    /// `values`, split at `;` (one trailing empty item dropped).
    values: Option<Vec<String>>,
    from: Option<String>,
    to: Option<String>,
    by: Option<String>,
    key_times: Option<Vec<f64>>,
    key_splines: Option<Vec<[f64; 4]>>,
    /// `<animateTransform type>`.
    transform_type: String,
    /// False when the attributes are invalid (the animation has no effect).
    valid: bool,
}

impl Animate {
    /// (simple-duration fraction, iteration index) at the end of an active duration of `length` ms.
    fn end_state(&self, length: f64) -> (f64, f64) {
        if !self.dur_ms.is_finite() || length <= 0.0 {
            return (0.0, 0.0);
        }
        let iterations = length / self.dur_ms;
        let fraction = iterations.fract();
        if fraction == 0.0 {
            (1.0, iterations - 1.0)
        } else {
            (fraction, iterations.floor())
        }
    }

    /// (simple-duration fraction, iteration index, begin of the interval) at document time `time`
    /// (ms), while the animation is active or frozen: in the latest of its `intervals` that began.
    fn simple_state(&self, time: f64, intervals: &[Interval]) -> Option<(f64, f64, f64)> {
        let interval = intervals.iter().rev().find(|i| i.begin <= time)?;
        let active = time - interval.begin;
        if time < interval.end {
            Some(((active % self.dur_ms) / self.dur_ms, (active / self.dur_ms).floor(), interval.begin))
        } else if self.freeze {
            let (progress, iteration) = self.end_state(interval.end - interval.begin);
            Some((progress, iteration, interval.begin))
        } else {
            None
        }
    }
}

/// One begin or end value of an animation element.
#[derive(Clone, Debug, PartialEq)]
enum Spec {
    /// A clock value: a time of the document's SMIL clock.
    Offset(f64),
    /// `indefinite`: no instance time until the element is begun or ended by script.
    Indefinite,
    /// `id.begin+1s`, `id.end`: a time of an interval of another animation element.
    Sync { id: String, end: bool, offset: f64 },
    /// `click`, `id.beginEvent+1s`, `id.repeat(2)`: the time an event happened at the event base
    /// (the element `id`, else the animation's target), plus the offset.
    Event { id: Option<String>, name: String, offset: f64 },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Restart {
    Always,
    WhenNotActive,
    Never,
}

/// One animation element's timing attributes (`<animateMotion>` and animations without a valid
/// target or `attributeName` too: they have intervals and dispatch events all the same).
struct Timed {
    node: NodeId,
    id: Option<String>,
    /// The element an event value without an id listens at (SMIL: the animation's target).
    target: Option<NodeId>,
    begins: Vec<Spec>,
    ends: Vec<Spec>,
    /// The simple duration (`INFINITY`: `indefinite`, `media` and what does not parse).
    dur_ms: f64,
    /// The active duration before `min` and `max`: `dur` times `repeatCount`, cut by `repeatDur`.
    active_ms: f64,
    min_ms: f64,
    max_ms: f64,
    restart: Restart,
    /// An ancestor fails conditional processing (`systemLanguage`): the element never begins.
    disabled: bool,
}

/// One interval of an animation element: `[begin, end)` of the document's SMIL clock.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Interval {
    begin: f64,
    /// `INFINITY` while nothing ends it (an `end` that waits for an event, a simple duration
    /// that is indefinite).
    end: f64,
}

/// What [`pump`] dispatched for an animation element: the interval whose `beginEvent` was the
/// latest (by its begin), whether its `endEvent` followed, and its repeats so far.
#[derive(Clone, Copy, Default)]
struct Fired {
    begin: Option<f64>,
    ended: bool,
    iteration: u32,
}

/// Instance times script and events added to an animation element, in document ms.
#[derive(Default)]
struct Dynamic {
    begins: Vec<f64>,
    ends: Vec<f64>,
}

/// The animation elements of a document as one reading of its tree.
#[derive(Default)]
pub(crate) struct Read {
    /// The DOM version it was read at.
    version: u64,
    animates: Vec<Animate>,
    timed: Vec<Timed>,
    /// `id` of an animation element: its index in `timed` (the first of an id).
    ids: HashMap<String, usize>,
    /// Some value is a syncbase (the intervals then settle over rounds).
    has_sync: bool,
    /// Some value is an event (dispatch tells [`on_event`] what to match).
    has_events: bool,
}

/// The document's `<animate>` elements, read again when the DOM changed.
#[derive(Default)]
pub(crate) struct Cache {
    read: RefCell<Option<(u64, Rc<Read>)>>,
    /// `SVGSVGElement` time control (`setCurrentTime`, `pauseAnimations`), of
    /// the document's one SMIL clock (SVG gives each outermost `<svg>` its
    /// own; pyro's animations all run on the document's).
    seek: std::cell::Cell<Seek>,
    /// The instance times script and events added, by animation element.
    dynamic: RefCell<HashMap<NodeId, Dynamic>>,
    /// Bumped by each change of `dynamic`.
    dynamic_generation: std::cell::Cell<u64>,
    /// The intervals of the animations of `read` (its version, the `dynamic_generation`).
    resolved: RefCell<Option<(u64, u64, Rc<Vec<Vec<Interval>>>)>>,
    /// What [`pump`] dispatched for each animation element so far.
    fired: RefCell<HashMap<NodeId, Fired>>,
    /// A seek happened since [`pump`] last ran.
    seeked: std::cell::Cell<bool>,
    /// The latest plan of a [`pump`]: an earlier one that finds it changed does nothing.
    plan: std::cell::Cell<u64>,
    /// The DOM version [`pump`] last ran at.
    pumped: std::cell::Cell<Option<u64>>,
}

#[derive(Clone, Copy, Default)]
struct Seek {
    /// Added to the time since the load event.
    offset_ms: f64,
    /// The time the clock stopped at (`pauseAnimations`).
    paused_ms: Option<f64>,
    /// Bumped by each change, a change of the frame (`version`).
    generation: u64,
}

/// The document's SMIL time at page time `now` (ms): since the load event,
/// moved by `setCurrentTime`, held by `pauseAnimations`. `None` before the
/// load event (unless set while paused).
fn document_time(page: &PageDom, now: f64) -> Option<f64> {
    let seek = page.animation_events.smil.seek.get();
    if let Some(paused) = seek.paused_ms {
        return Some(paused);
    }
    page.timing().load_start.map(|load| now - load + seek.offset_ms)
}

/// `SVGSVGElement.getCurrentTime()`, in seconds.
pub(crate) fn current_time(page: &PageDom) -> f64 {
    let now = page.timeline_now_ms();
    let seek = page.animation_events.smil.seek.get();
    match document_time(page, now) {
        Some(t) => t / 1000.0,
        // Before the load event the clock is at its offset.
        None => seek.offset_ms / 1000.0,
    }
}

/// `SVGSVGElement.setCurrentTime(seconds)`.
pub(crate) fn set_current_time(page: &Rc<PageDom>, seconds: f64) {
    let target = seconds.max(0.0) * 1000.0;
    let now = page.timeline_now_ms();
    let cell = &page.animation_events.smil.seek;
    let mut seek = cell.get();
    match (seek.paused_ms, page.timing().load_start) {
        (Some(_), _) => seek.paused_ms = Some(target),
        (None, Some(load)) => seek.offset_ms = target - (now - load),
        (None, None) => seek.offset_ms = target,
    }
    seek.generation = seek.generation.wrapping_add(1);
    cell.set(seek);
    // The events of the move are dispatched in a task: a seek is looked at as a seek.
    page.animation_events.smil.seeked.set(true);
    schedule_pump(page, Duration::ZERO);
}

/// `pauseAnimations()` (`true`) and `unpauseAnimations()` (`false`).
pub(crate) fn set_paused(page: &Rc<PageDom>, paused: bool) {
    let now = page.timeline_now_ms();
    let cell = &page.animation_events.smil.seek;
    let current = document_time(page, now);
    let mut seek = cell.get();
    match (paused, seek.paused_ms) {
        (true, None) => seek.paused_ms = Some(current.unwrap_or(seek.offset_ms)),
        (false, Some(at)) => {
            seek.paused_ms = None;
            seek.offset_ms = match page.timing().load_start {
                Some(load) => at - (now - load),
                None => at,
            };
        }
        _ => return,
    }
    seek.generation = seek.generation.wrapping_add(1);
    cell.set(seek);
    // (Running again: the next begin, end or repeat is waited for from here.)
    schedule_pump(page, Duration::ZERO);
}

/// `animationsPaused()`.
pub(crate) fn paused(page: &PageDom) -> bool {
    page.animation_events.smil.seek.get().paused_ms.is_some()
}

impl Cache {
    pub(crate) fn clear(&self) {
        self.read.borrow_mut().take();
        self.resolved.borrow_mut().take();
        self.dynamic.borrow_mut().clear();
        self.fired.borrow_mut().clear();
        self.pumped.set(None);
    }

    /// The document's animations (none, and nothing read, until an SVG
    /// `<animate>` element was ever created: [`Dom::smil_seen`]).
    fn animations(&self, dom: &Dom, doc: NodeId) -> Option<Rc<Read>> {
        if !dom.smil_seen() {
            return None;
        }
        let version = dom.version();
        if let Some((at, read)) = &*self.read.borrow() {
            if *at == version {
                return Some(read.clone());
            }
        }
        let read = Rc::new(read_animations(dom, doc, version));
        *self.read.borrow_mut() = Some((version, read.clone()));
        Some(read)
    }

    /// The intervals of each of `read.timed`, resolved again when the document or an instance time changed.
    fn intervals(&self, read: &Read) -> Rc<Vec<Vec<Interval>>> {
        let generation = self.dynamic_generation.get();
        if let Some((version, at, found)) = &*self.resolved.borrow() {
            if *version == read.version && *at == generation {
                return found.clone();
            }
        }
        let found = Rc::new(resolve_all(read, &self.dynamic.borrow()));
        *self.resolved.borrow_mut() = Some((read.version, generation, found.clone()));
        found
    }

    /// Adds an instance time to the `begin` (or `end`) list of animation element `node`.
    fn add_instance(&self, node: NodeId, end: bool, at: f64) {
        let mut dynamic = self.dynamic.borrow_mut();
        let list = dynamic.entry(node).or_default();
        let times = if end { &mut list.ends } else { &mut list.begins };
        // (Script that begins an animation every frame does not grow the list without end.)
        if times.len() >= MAX_INSTANCES {
            times.remove(0);
        }
        times.push(at);
        drop(dynamic);

        self.dynamic_generation.set(self.dynamic_generation.get().wrapping_add(1));
        // A change of the frame (`version`).
        let mut seek = self.seek.get();
        seek.generation = seek.generation.wrapping_add(1);
        self.seek.set(seek);
    }
}

/// Instance times of one list kept for an element.
const MAX_INSTANCES: usize = 1024;
/// Intervals of one element.
const MAX_INTERVALS: usize = 256;
/// Rounds the intervals of animations with syncbase values settle in.
const MAX_ROUNDS: usize = 64;

/// Whether `systemLanguage` (SVG conditional processing) of an element of the tree passes: the
/// user's language is English.
fn language_passes(list: &str) -> bool {
    list.split(',').map(str::trim).any(|tag| {
        let primary = tag.split('-').next().unwrap_or("");
        primary.eq_ignore_ascii_case("en")
    })
}

/// Whether `id` or an ancestor fails conditional processing.
fn fails_conditions(dom: &Dom, id: NodeId) -> bool {
    let mut at = Some(id);
    while let Some(node) = at {
        if dom.get_attribute(node, "systemLanguage").ok().flatten().is_some_and(|list| !language_passes(list)) {
            return true;
        }
        at = dom.parent(node).ok().flatten();
    }
    false
}

/// The element animation element `id` animates: `href`, `xlink:href`, else its parent.
fn target_of(dom: &Dom, doc: NodeId, id: NodeId) -> Option<NodeId> {
    let attr = |name: &str| dom.get_attribute(id, name).ok().flatten();
    match attr("href").or_else(|| attr("xlink:href")) {
        Some(href) => dom.get_element_by_id(doc, href.trim().strip_prefix('#')?).ok().flatten(),
        None => dom.parent(id).ok().flatten().filter(|p| dom.element(*p).is_ok()),
    }
}

fn read_animations(dom: &Dom, doc: NodeId, version: u64) -> Read {
    let mut read = Read { version, ..Read::default() };
    let Ok(descendants) = dom.descendants(doc) else { return read };
    for id in descendants {
        let Ok(e) = dom.element(id) else { continue };
        if e.name.ns != ns!(svg) {
            continue;
        }
        let kind = match &*e.name.local {
            "animate" => Some(Kind::Animate),
            "animateTransform" => Some(Kind::Transform),
            "set" => Some(Kind::Set),
            // (Timed, not animated here.)
            "animateMotion" => None,
            _ => continue,
        };
        let index = read.timed.len();
        let timed = read_timed(dom, doc, id);
        read.has_sync |= timed.begins.iter().chain(&timed.ends).any(|s| matches!(s, Spec::Sync { .. }));
        read.has_events |= timed.begins.iter().chain(&timed.ends).any(|s| matches!(s, Spec::Event { .. }));
        if let Some(name) = &timed.id {
            read.ids.entry(name.clone()).or_insert(index);
        }
        read.timed.push(timed);
        if let Some(mut animate) = kind.and_then(|kind| read_animate(dom, doc, id, kind)) {
            animate.timed = index;
            read.animates.push(animate);
        }
    }
    read
}

/// The active duration of animation element `id` of simple duration `dur_ms`: the repeat duration
/// (`dur` times `repeatCount`), cut by `repeatDur` (SMIL 3.6.5). `None` for an invalid `repeatCount`;
/// one that is not finite (`1e+309`) is as good as none.
fn active_duration(dom: &Dom, id: NodeId, dur_ms: f64) -> Option<f64> {
    let attr = |name: &str| dom.get_attribute(id, name).ok().flatten();
    let repeat_count = match attr("repeatCount").map(str::trim) {
        Some("indefinite") => Some(f64::INFINITY),
        Some(count) => Some(count.parse::<f64>().ok().filter(|c| *c > 0.0)?).filter(|c| c.is_finite()),
        None => None,
    };
    let repeat_dur = match attr("repeatDur").map(str::trim) {
        Some("indefinite") => Some(f64::INFINITY),
        Some(text) => clock_ms(text).filter(|d| *d > 0.0),
        None => None,
    };
    Some(match (repeat_count, repeat_dur) {
        (None, None) => dur_ms,
        (Some(count), None) => dur_ms * count,
        (None, Some(duration)) => duration,
        (Some(count), Some(duration)) => (dur_ms * count).min(duration),
    })
}

/// The timing of animation element `id`.
fn read_timed(dom: &Dom, doc: NodeId, id: NodeId) -> Timed {
    let attr = |name: &str| dom.get_attribute(id, name).ok().flatten();
    // Without `begin` the element begins with its time container; a list of nothing valid never.
    let begins = match attr("begin") {
        Some(list) => parse_specs(list),
        None => vec![Spec::Offset(0.0)],
    };
    let ends = attr("end").map(parse_specs).unwrap_or_default();
    let dur_ms = match attr("dur").map(str::trim) {
        Some(text) => clock_ms(text).filter(|d| *d > 0.0).unwrap_or(f64::INFINITY),
        None => f64::INFINITY,
    };
    let active_ms = active_duration(dom, id, dur_ms).unwrap_or(dur_ms);
    let min_ms = attr("min")
        .map(str::trim)
        .and_then(|t| if t == "media" { Some(0.0) } else { clock_ms(t) })
        .filter(|m| *m >= 0.0)
        .unwrap_or(0.0);
    let max_ms = attr("max")
        .map(str::trim)
        .and_then(|t| if t == "indefinite" { Some(f64::INFINITY) } else { clock_ms(t) })
        .filter(|m| *m >= 0.0)
        .unwrap_or(f64::INFINITY);
    // (A `min` above `max` makes both void.)
    let (min_ms, max_ms) = if min_ms > max_ms { (0.0, f64::INFINITY) } else { (min_ms, max_ms) };
    let restart = match attr("restart").map(str::trim) {
        Some("whenNotActive") => Restart::WhenNotActive,
        Some("never") => Restart::Never,
        _ => Restart::Always,
    };
    Timed {
        node: id,
        id: attr("id").filter(|v| !v.is_empty()).map(str::to_owned),
        target: target_of(dom, doc, id),
        begins,
        ends,
        dur_ms,
        active_ms,
        min_ms,
        max_ms,
        restart,
        disabled: fails_conditions(dom, id),
    }
}

/// The values of a `begin` or `end` attribute, `;` separated; one that does not parse is ignored.
fn parse_specs(list: &str) -> Vec<Spec> {
    list.split(';').filter_map(parse_spec).collect()
}

/// One begin or end value (SMIL 3.6.1 `begin-value`): an offset, `indefinite`, a syncbase value
/// (`id.begin+1s`, a `.` in the id escaped with `\`) or an event value (`id.click`, `click-2s`,
/// `repeat(2)`). `wallclock` values are not read.
fn parse_spec(text: &str) -> Option<Spec> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if text == "indefinite" {
        return Some(Spec::Indefinite);
    }
    if let Some(ms) = clock_ms(text) {
        return Some(Spec::Offset(ms));
    }
    // The id ends at the first unescaped `.`; an event name has none before its offset's `+`.
    let mut id = String::new();
    let mut escaped = false;
    let mut after_dot = None;
    for (at, c) in text.char_indices() {
        if escaped {
            id.push(c);
            escaped = false;
            continue;
        }
        match c {
            '\\' => escaped = true,
            '.' => {
                after_dot = Some(at + 1);
                break;
            }
            '+' => break,
            c if c.is_whitespace() => break,
            c => id.push(c),
        }
    }
    let (id, rest) = match after_dot {
        Some(at) => (Some(id), &text[at..]),
        None => (None, text),
    };
    if id.as_deref() == Some("") {
        return None;
    }
    // The name ends at the offset's sign or a space (not inside `repeat(2)`).
    let mut depth = 0_u32;
    let mut name_end = rest.len();
    for (at, c) in rest.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            '+' | '-' if depth == 0 => {
                name_end = at;
                break;
            }
            c if c.is_whitespace() && depth == 0 => {
                name_end = at;
                break;
            }
            _ => {}
        }
    }
    let (name, offset_text) = rest.split_at(name_end);
    if name.is_empty() {
        return None;
    }
    let offset_text: String = offset_text.split_whitespace().collect();
    let offset = if offset_text.is_empty() {
        0.0
    } else if offset_text.starts_with('+') || offset_text.starts_with('-') {
        clock_ms(&offset_text)?
    } else {
        return None;
    };
    match (&id, name) {
        // (The events of an animation element happen at its interval's times: a syncbase.)
        (Some(id), "begin" | "beginEvent") => Some(Spec::Sync { id: id.clone(), end: false, offset }),
        (Some(id), "end" | "endEvent") => Some(Spec::Sync { id: id.clone(), end: true, offset }),
        _ => Some(Spec::Event { id: id.clone(), name: name.to_owned(), offset }),
    }
}

/// The instance times of the `begin` (or `end`) list of animation element `i`: its offsets, the
/// times of the intervals `current` has for its syncbases, and what script and events added.
fn instances(read: &Read, dynamic: &HashMap<NodeId, Dynamic>, current: &[Vec<Interval>], i: usize, ends: bool) -> Vec<f64> {
    let timed = &read.timed[i];
    let mut times = Vec::new();
    let specs = if ends { &timed.ends } else { &timed.begins };
    for spec in specs {
        match spec {
            Spec::Offset(ms) => times.push(*ms),
            // (An `end` that is indefinite ends nothing.)
            Spec::Indefinite => {
                if ends {
                    times.push(f64::INFINITY);
                }
            }
            Spec::Sync { id, end, offset } => {
                let Some(&other) = read.ids.get(id.as_str()) else { continue };
                for interval in &current[other] {
                    let at = if *end { interval.end } else { interval.begin };
                    if at.is_finite() {
                        times.push(at + offset);
                    }
                }
            }
            Spec::Event { .. } => {}
        }
    }
    if let Some(added) = dynamic.get(&timed.node) {
        times.extend(if ends { &added.ends } else { &added.begins });
    }
    times.retain(|t| !t.is_nan());
    times.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    times.dedup();
    times
}

/// SMIL 3.6.8 "Interval timing": the intervals of animation element `i`, from its instance times
/// (`current`: what its syncbases have so far).
fn resolve(read: &Read, dynamic: &HashMap<NodeId, Dynamic>, current: &[Vec<Interval>], i: usize) -> Vec<Interval> {
    let timed = &read.timed[i];
    let mut out: Vec<Interval> = Vec::new();
    if timed.disabled {
        return out;
    }
    let begins = instances(read, dynamic, current, i, false);
    let ends = instances(read, dynamic, current, i, true);
    // An `end` with a value that waits for an event or another animation leaves the interval
    // open (to its active duration) until an instance time arrives (so do end instances script
    // added, which need no `end` attribute); one of offsets alone, all before the begin, is no
    // interval at all.
    let has_end = !timed.ends.is_empty() || !ends.is_empty();
    let end_waits = timed.ends.is_empty() || timed.ends.iter().any(|s| !matches!(s, Spec::Offset(_)));
    // The next interval begins after this one (`after`) and not before the last ended (`floor`).
    let mut after = f64::NEG_INFINITY;
    let mut floor = f64::NEG_INFINITY;
    for _ in 0..MAX_INTERVALS {
        let Some(&begin) = begins.iter().find(|b| **b > after && **b >= floor) else { break };
        after = begin;
        let natural = begin + timed.active_ms;
        let mut end = if !has_end {
            natural
        } else {
            match ends.iter().find(|e| **e > begin) {
                Some(e) => (*e).min(natural),
                None if end_waits => natural,
                None => continue,
            }
        };
        // `min` and `max` bound the active duration.
        end = begin + (end - begin).min(timed.max_ms).max(timed.min_ms);
        // The first interval ends before the time container begins: it never was (SMIL 3.6.8,
        // "first interval in the past").
        if begin < 0.0 && end <= 0.0 {
            continue;
        }
        // `restart="always"`: a later begin instance inside the interval ends it.
        if timed.restart == Restart::Always {
            if let Some(&next) = begins.iter().find(|b| **b > begin && **b < end) {
                end = next;
            }
        }
        out.push(Interval { begin, end });
        if timed.restart == Restart::Never || !end.is_finite() {
            break;
        }
        floor = end;
    }
    out
}

/// The intervals of every animation element: each round resolves them all from the syncbase
/// times of the round before, until none changes (a cycle of syncbases settles over rounds).
fn resolve_all(read: &Read, dynamic: &HashMap<NodeId, Dynamic>) -> Vec<Vec<Interval>> {
    let count = read.timed.len();
    let mut current: Vec<Vec<Interval>> = vec![Vec::new(); count];
    let rounds = if read.has_sync { MAX_ROUNDS } else { 1 };
    for _ in 0..rounds {
        let next: Vec<Vec<Interval>> = (0..count).map(|i| resolve(read, dynamic, &current, i)).collect();
        let settled = next == current;
        current = next;
        if settled {
            break;
        }
    }
    current
}

/// A `;` separated list of numbers; one trailing empty item is allowed (`"0;1;"`).
fn parse_number_list(list: &str) -> Option<Vec<f64>> {
    let mut parts: Vec<&str> = list.split(';').map(str::trim).collect();
    if parts.last().is_some_and(|p| p.is_empty()) {
        parts.pop();
    }
    parts.into_iter().map(|p| p.parse::<f64>().ok().filter(|v| v.is_finite())).collect()
}

/// `keySplines`: each a cubic Bezier `x1 y1 x2 y2` with both x in 0..1 (y may leave it, as in Chrome).
fn parse_key_splines(list: &str) -> Option<Vec<[f64; 4]>> {
    let mut parts: Vec<&str> = list.split(';').map(str::trim).collect();
    if parts.last().is_some_and(|p| p.is_empty()) {
        parts.pop();
    }
    parts
        .into_iter()
        .map(|s| {
            let p: Vec<f64> = s
                .split(|c: char| c == ',' || c.is_whitespace())
                .filter(|t| !t.is_empty())
                .map(|t| t.parse::<f64>().ok().filter(|v| v.is_finite()))
                .collect::<Option<_>>()?;
            (p.len() == 4 && (0.0..=1.0).contains(&p[0]) && (0.0..=1.0).contains(&p[2])).then(|| [p[0], p[1], p[2], p[3]])
        })
        .collect()
}

fn read_animate(dom: &Dom, doc: NodeId, id: NodeId, kind: Kind) -> Option<Animate> {
    let attr = |name: &str| dom.get_attribute(id, name).ok().flatten();
    let target = match attr("href").or_else(|| attr("xlink:href")) {
        Some(href) => dom.get_element_by_id(doc, href.trim().strip_prefix('#')?).ok().flatten()?,
        None => dom.parent(id).ok().flatten().filter(|p| dom.element(*p).is_ok())?,
    };
    let property = attr("attributeName")?.trim();
    if property.is_empty() {
        return None;
    }
    let mode = attr("calcMode").map(str::trim).unwrap_or("linear");
    let discrete = mode == "discrete";
    // The CSS-property reading: the values as declarations (None where it is invalid).
    let css = if kind == Kind::Transform || property.contains(':') {
        None
    } else {
        (|| -> Option<Css> {
            if kind == Kind::Set {
                return Some(Css { values: vec![attr("to")?.trim().to_owned()], key_times: vec![0.0], key_splines: None, discrete: true, additive: false });
            }
            let mut additive = false;
            let values: Vec<String> = match attr("values") {
                Some(list) => list.split(';').map(str::trim).filter(|v| !v.is_empty()).map(str::to_owned).collect(),
                None => match (attr("from"), attr("to"), attr("by")) {
                    (Some(from), Some(to), _) => vec![from.trim().to_owned(), to.trim().to_owned()],
                    // `from` and `by` of a number with a unit (`10px`): from, and from plus by.
                    (Some(from), None, Some(by)) => {
                        let ((f, unit), (b, by_unit)) = (number_unit(from)?, number_unit(by)?);
                        if unit != by_unit {
                            return None;
                        }
                        vec![from.trim().to_owned(), format!("{}{unit}", f + b)]
                    }
                    // `by` alone adds to the underlying value, from zero.
                    (None, None, Some(by)) => {
                        let (_, unit) = number_unit(by)?;
                        additive = true;
                        vec![format!("0{unit}"), by.trim().to_owned()]
                    }
                    _ => return None,
                },
            };
            if values.is_empty() {
                return None;
            }
            let n = values.len();
            // Evenly spaced without `keyTimes`: `n` intervals of one value each when
            // discrete, `n - 1` between the values otherwise.
            let key_times = match attr("keyTimes") {
                Some(list) => {
                    // (One trailing empty item is allowed: `"0;1;"`.)
                    let times = parse_number_list(list)?;
                    // An invalid list disables the animation (SVG's error handling).
                    let ordered = times.windows(2).all(|w| w[0] <= w[1]);
                    let bounded = times.iter().all(|t| (0.0..=1.0).contains(t));
                    let ends = times.first() == Some(&0.0) && (discrete || times.last() == Some(&1.0));
                    if times.len() != n || !ordered || !bounded || !ends {
                        return None;
                    }
                    times
                }
                None if discrete => (0..n).map(|i| i as f64 / n as f64).collect(),
                None => (0..n).map(|i| if n == 1 { 0.0 } else { i as f64 / (n - 1) as f64 }).collect(),
            };
            let key_splines = match (mode, attr("keySplines")) {
                ("spline", Some(list)) => {
                    let splines = parse_key_splines(list)?;
                    if splines.len() + 1 != n {
                        return None;
                    }
                    Some(splines.iter().map(|p| format!("cubic-bezier({}, {}, {}, {})", p[0], p[1], p[2], p[3])).collect::<Vec<String>>())
                }
                ("spline", None) => return None,
                _ => None,
            };
            Some(Css { values, key_times, key_splines, discrete, additive })
        })()
    };
    let dur_ms = match attr("dur").map(str::trim) {
        None | Some("indefinite") if kind == Kind::Set => f64::INFINITY,
        Some(text) => clock_ms(text).filter(|d| *d > 0.0)?,
        None => return None,
    };
    // (An invalid `repeatCount` disables the animation's values; its timing is the element's.)
    active_duration(dom, id, dur_ms)?;
    let freeze = attr("fill").map(str::trim) == Some("freeze");

    let mut valid = kind != Kind::Set || attr("to").is_some();
    let calc = match mode {
        "discrete" => CalcMode::Discrete,
        "paced" => CalcMode::Paced,
        "spline" => CalcMode::Spline,
        _ => CalcMode::Linear,
    };
    let values = attr("values").map(|list| {
        let mut parts: Vec<String> = list.split(';').map(|v| v.trim().to_owned()).collect();
        if parts.last().is_some_and(|p| p.is_empty()) {
            parts.pop();
        }
        parts
    });
    if values.as_ref().is_some_and(|v| v.is_empty()) {
        valid = false;
    }
    let key_times = match attr("keyTimes") {
        Some(list) => match parse_number_list(list) {
            Some(t) if t.windows(2).all(|w| w[0] <= w[1]) && t.iter().all(|x| (0.0..=1.0).contains(x)) => Some(t),
            _ => {
                valid = false;
                None
            }
        },
        None => None,
    };
    let key_splines = match attr("keySplines").map(parse_key_splines) {
        Some(Some(splines)) => Some(splines),
        Some(None) => {
            if calc == CalcMode::Spline {
                valid = false;
            }
            None
        }
        None => None,
    };
    if calc == CalcMode::Spline && key_splines.is_none() {
        valid = false;
    }
    let text = |name: &str| attr(name).map(|v| v.trim().to_owned());
    Some(Animate {
        target,
        property: property.to_owned(),
        kind,
        css,
        timed: 0,
        dur_ms,
        freeze,
        calc,
        additive_sum: attr("additive").map(str::trim) == Some("sum"),
        accumulate_sum: attr("accumulate").map(str::trim) == Some("sum"),
        values,
        from: text("from"),
        to: text("to"),
        by: text("by"),
        key_times,
        key_splines,
        transform_type: attr("type").map_or("translate", str::trim).to_owned(),
        valid,
    })
}

/// `10px` as `(10, "px")`: a number and its unit (`None` for any other text).
fn number_unit(text: &str) -> Option<(f64, &str)> {
    let text = text.trim();
    let end = text.find(|c: char| c.is_ascii_alphabetic() || c == '%').unwrap_or(text.len());
    let (number, unit) = text.split_at(end);
    let number = number.trim().parse::<f64>().ok().filter(|v| v.is_finite())?;
    unit.chars().all(|c| c.is_ascii_alphabetic() || c == '%').then_some((number, unit))
}

/// A SMIL clock value in ms: `hh:mm:ss.f`, `mm:ss.f`, or a count with `h`,
/// `min`, `s` or `ms` (seconds without one).
fn clock_ms(text: &str) -> Option<f64> {
    let text = text.trim();
    if text.contains(':') {
        let parts: Vec<f64> = text.split(':').map(|p| p.trim().parse::<f64>().ok()).collect::<Option<_>>()?;
        let seconds = match parts.as_slice() {
            [h, m, s] => h * 3600.0 + m * 60.0 + s,
            [m, s] => m * 60.0 + s,
            _ => return None,
        };
        return Some(seconds * 1000.0);
    }
    let (count, scale) = if let Some(c) = text.strip_suffix("ms") {
        (c, 1.0)
    } else if let Some(c) = text.strip_suffix("min") {
        (c, 60_000.0)
    } else if let Some(c) = text.strip_suffix('h') {
        (c, 3_600_000.0)
    } else if let Some(c) = text.strip_suffix('s') {
        (c, 1000.0)
    } else {
        (text, 1000.0)
    };
    let count: f64 = count.trim().parse().ok()?;
    count.is_finite().then_some(count * scale)
}

/// Each `<animate>` of a CSS property in effect at page time `now`, in document order:
/// `visit(target, effect)`. Returns whether a value moves with time (one
/// running, or one yet to begin). `now` `None`: settled, as Playwright's
/// `animations: "disabled"` leaves CSS animations (a finite one at its end,
/// so only one that freezes shows; an infinite one gone).
pub(crate) fn walk(page: &PageDom, now: Option<f64>, visit: &mut dyn FnMut(NodeId, Effect)) -> bool {
    let Ok(dom) = page.dom.try_borrow() else { return false };
    let cache = &page.animation_events.smil;
    let Some(animations) = cache.animations(&dom, page.document()) else { return false };
    // The time containers start at the load event.
    let time = match now {
        Some(now) => match document_time(page, now) {
            Some(time) => Some(time),
            None => return animations.animates.iter().any(|a| a.css.is_some()),
        },
        None => None,
    };
    let intervals = cache.intervals(&animations);
    let paused = cache.seek.get().paused_ms.is_some();
    let mut moving = false;
    for a in animations.animates.iter() {
        let Some(css) = a.css.as_ref() else { continue };
        let ivs = &intervals[a.timed];
        // The simple time's fraction.
        let progress = match time {
            None => match ivs.last() {
                Some(last) if last.end.is_finite() && a.freeze => a.end_state(last.end - last.begin).0,
                // (A `set` without `dur` holds its value.)
                Some(_) if a.kind == Kind::Set && !a.dur_ms.is_finite() => 0.0,
                _ => continue,
            },
            Some(time) => {
                // An interval yet to begin: the value moves.
                if ivs.iter().any(|i| i.begin > time) {
                    moving |= !paused;
                }
                // (A value that does not change over its simple duration does not move.)
                if ivs.iter().any(|i| i.begin <= time && time < i.end) && a.dur_ms.is_finite() {
                    moving |= !paused;
                }
                match a.simple_state(time, ivs) {
                    Some((progress, _, _)) => progress,
                    None => continue,
                }
            }
        };
        let declaration = |v: &str| format!("{}: {}", a.property, v);
        let effect = |frames: Vec<TextKeyframe>, progress: f64| Effect {
            keyframes: Keyframes::Text(Arc::new(TextKeyframes { frames })),
            progress,
            before: false,
            easing: None,
            // `additive="sum"`: the values add to the value below (Web Animations' `add`).
            composite: if css.additive || (a.additive_sum && a.kind != Kind::Set) { Composite::Add } else { Composite::Replace },
            timeline: None,
        };
        let constant = |v: &str| {
            let frame = |offset| TextKeyframe { offset, easing: None, composite: None, declarations: declaration(v) };
            effect(vec![frame(0.0), frame(1.0)], 0.0)
        };
        let e = if css.discrete || css.values.len() == 1 {
            // The value whose time came last.
            let i = css.key_times.iter().rposition(|t| *t <= progress).unwrap_or(0);
            constant(&css.values[i])
        } else {
            let frames = css
                .values
                .iter()
                .zip(&css.key_times)
                .enumerate()
                .map(|(i, (v, t))| TextKeyframe {
                    offset: *t,
                    easing: css.key_splines.as_ref().and_then(|s| s.get(i).cloned()),
                    composite: None,
                    declarations: declaration(v),
                })
                .collect();
            effect(frames, progress)
        };
        if dom.is_alive(a.target) {
            visit(a.target, e);
        }
    }
    moving
}

/// One value of an animation function, before it is parsed as the attribute's type.
#[derive(Clone)]
pub(crate) enum Src {
    /// The underlying value (a `to` animation starts there).
    Underlying,
    /// The type's zero (a `by` animation starts there).
    Zero,
    Text(String),
    /// `from` plus `by`.
    Sum(String, String),
}

/// An animation in effect on an attribute at the current SMIL time: its animation function
/// (values, key times, easing), where in it the time is, and how it combines with the value below it.
#[derive(Clone)]
pub(crate) struct Contribution {
    values: Vec<Src>,
    /// Each value's time (a fraction of the simple duration); `None` evenly spaced.
    key_times: Option<Vec<f64>>,
    splines: Option<Vec<[f64; 4]>>,
    calc: CalcMode,
    progress: f64,
    iteration: f64,
    additive: bool,
    accumulate: bool,
    /// `<animateTransform type>`.
    transform_type: Option<String>,
}

/// The animations in effect on `attribute` of `target` at the document's current SMIL time, in
/// sandwich order (begin time, then document order); empty when none is.
pub(crate) fn animated(page: &PageDom, target: NodeId, attribute: &str) -> Vec<Contribution> {
    let Ok(dom) = page.dom.try_borrow() else { return Vec::new() };
    let cache = &page.animation_events.smil;
    let Some(animations) = cache.animations(&dom, page.document()) else { return Vec::new() };
    let Some(time) = document_time(page, page.timeline_now_ms()) else { return Vec::new() };
    let intervals = cache.intervals(&animations);
    let mut found: Vec<(f64, Contribution)> = Vec::new();
    for a in animations.animates.iter() {
        if a.target != target || a.property != attribute || !a.valid {
            continue;
        }
        let Some((progress, iteration, begin)) = a.simple_state(time, &intervals[a.timed]) else { continue };
        if !dom.is_alive(a.target) {
            continue;
        }
        if let Some(c) = contribution(a, progress, iteration) {
            found.push((begin, c));
        }
    }
    // A stable sort: animations that begin together stay in document order.
    found.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap_or(std::cmp::Ordering::Equal));
    found.into_iter().map(|(_, c)| c).collect()
}

/// SMIL 3.2: the animation function of `a`: `values`, else `from`/`to`, `from`/`by`, `to` or `by`.
fn contribution(a: &Animate, progress: f64, iteration: f64) -> Option<Contribution> {
    if a.kind == Kind::Set {
        // SVG Animations 4.3: `<set>` holds its `to` for the active duration (not additive).
        return Some(Contribution {
            values: vec![Src::Text(a.to.clone()?)],
            key_times: None,
            splines: None,
            calc: CalcMode::Discrete,
            progress,
            iteration,
            additive: false,
            accumulate: false,
            transform_type: None,
        });
    }
    let (values, by_only, to_only): (Vec<Src>, bool, bool) = if let Some(list) = &a.values {
        (list.iter().cloned().map(Src::Text).collect(), false, false)
    } else {
        match (&a.from, &a.to, &a.by) {
            (Some(from), Some(to), _) => (vec![Src::Text(from.clone()), Src::Text(to.clone())], false, false),
            (Some(from), None, Some(by)) => (vec![Src::Text(from.clone()), Src::Sum(from.clone(), by.clone())], false, false),
            // (A transform list is not underlying a `to` animation: it starts at the type's zero.)
            (None, Some(to), _) => {
                let first = if a.kind == Kind::Transform { Src::Zero } else { Src::Underlying };
                (vec![first, Src::Text(to.clone())], false, true)
            }
            (None, None, Some(by)) => (vec![Src::Zero, Src::Text(by.clone())], true, false),
            _ => return None,
        }
    };
    let n = values.len();
    let key_times = match (&a.key_times, a.calc) {
        // `paced` ignores `keyTimes`.
        (_, CalcMode::Paced) | (None, _) => None,
        (Some(times), calc) => {
            let ends = times.first() == Some(&0.0) && (calc == CalcMode::Discrete || times.last() == Some(&1.0));
            if times.len() != n || !ends {
                return None;
            }
            Some(times.clone())
        }
    };
    let splines = match (&a.key_splines, a.calc) {
        (Some(splines), CalcMode::Spline) if splines.len() + 1 == n => Some(splines.clone()),
        (_, CalcMode::Spline) => return None,
        _ => None,
    };
    Some(Contribution {
        values,
        key_times,
        splines,
        calc: a.calc,
        progress,
        iteration,
        // `by` alone is additive by definition; a `to` animation is not (it starts at the underlying value).
        additive: (a.additive_sum && !to_only) || by_only,
        accumulate: a.accumulate_sum && !to_only,
        transform_type: (a.kind == Kind::Transform).then(|| a.transform_type.clone()),
    })
}

impl Contribution {
    /// Whether the animation adds to the value below it.
    pub(crate) fn additive(&self) -> bool {
        self.additive
    }

    /// `<animateTransform type>`; `None` for `<animate>`.
    pub(crate) fn transform_type(&self) -> Option<&str> {
        self.transform_type.as_deref()
    }
}

/// How the values of one attribute type are parsed and combined.
pub(crate) trait Space {
    type V: Clone;
    /// The value of `text` (`kind`: an `<animateTransform>`'s type); `None` when it is not one.
    fn parse(&self, text: &str, kind: Option<&str>) -> Option<Self::V>;
    /// `a` at `t` of the way to `b` (a type that does not interpolate takes `a` before the middle).
    fn lerp(&self, a: &Self::V, b: &Self::V, t: f64) -> Self::V;
    /// The sum of two values.
    fn add(&self, a: &Self::V, b: &Self::V) -> Self::V;
    fn scale(&self, a: &Self::V, k: f64) -> Self::V;
    /// The zero of the type of `like`.
    fn zero(&self, like: &Self::V) -> Self::V;
    /// The distance between two values (`calcMode="paced"`); `None` for a type without one.
    fn distance(&self, _a: &Self::V, _b: &Self::V) -> Option<f64> {
        None
    }
    /// Whether values of the type can be added at all (`additive="sum"`).
    fn additive(&self) -> bool {
        true
    }
    /// Whether values of the type interpolate; one that does not (a string, a keyword, a boolean) is
    /// animated as `calcMode="discrete"`: `n` values take `n` equal parts of the simple duration.
    fn interpolates(&self) -> bool {
        true
    }
    /// An additive animation's value on top of the underlying one.
    fn combine(&self, under: &Self::V, value: &Self::V) -> Self::V {
        self.add(under, value)
    }
}

/// The cubic Bezier easing of a `keySplines` entry at `x`.
fn spline(p: &[f64; 4], x: f64) -> f64 {
    let bezier = |a: f64, b: f64, s: f64| {
        let u = 1.0 - s;
        3.0 * u * u * s * a + 3.0 * u * s * s * b + s * s * s
    };
    let (mut lo, mut hi) = (0.0_f64, 1.0_f64);
    for _ in 0..40 {
        let mid = (lo + hi) / 2.0;
        if bezier(p[0], p[2], mid) < x {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    bezier(p[1], p[3], (lo + hi) / 2.0)
}

/// The value of the animation function of `c` at its time, over the `under`lying value; `None`
/// when a value does not parse as the type (the animation then has no effect).
pub(crate) fn evaluate<S: Space>(space: &S, c: &Contribution, under: &S::V) -> Option<S::V> {
    // A `by` animation (alone, or with `from`) of a type whose values do not add (a string, a
    // keyword, a boolean) has no effect (WPT svg/animations `non-additive-type-by-animation`).
    if !space.additive() && c.values.iter().any(|v| matches!(v, Src::Zero | Src::Sum(..))) {
        return None;
    }
    let kind = c.transform_type.as_deref();
    let parsed: Vec<Option<S::V>> = c
        .values
        .iter()
        .map(|src| match src {
            Src::Underlying => Some(under.clone()),
            Src::Zero => None,
            Src::Text(text) => space.parse(text, kind),
            Src::Sum(a, b) => {
                let (a, b) = (space.parse(a, kind)?, space.parse(b, kind)?);
                Some(space.add(&a, &b))
            }
        })
        .collect();
    let like = parsed.iter().flatten().next()?.clone();
    let mut values: Vec<S::V> = Vec::with_capacity(parsed.len());
    for (src, value) in c.values.iter().zip(parsed) {
        match (src, value) {
            (_, Some(v)) => values.push(v),
            (Src::Zero, None) => values.push(space.zero(&like)),
            (_, None) => return None,
        }
    }
    let n = values.len();
    let p = c.progress;
    let mut result = if n == 1 {
        values[0].clone()
    } else if c.calc == CalcMode::Discrete || !space.interpolates() {
        // The value whose time came last (`n` intervals of one value each without `keyTimes`).
        let i = match &c.key_times {
            Some(times) => times.iter().rposition(|t| *t <= p).unwrap_or(0),
            None => ((p * n as f64).floor() as usize).min(n - 1),
        };
        values[i].clone()
    } else {
        let times: Vec<f64> = match &c.key_times {
            Some(times) => times.clone(),
            None if c.calc == CalcMode::Paced => {
                let distances: Option<Vec<f64>> = values.windows(2).map(|w| space.distance(&w[0], &w[1])).collect();
                match distances {
                    Some(d) if d.iter().sum::<f64>() > 0.0 => {
                        let total: f64 = d.iter().sum();
                        let mut at = 0.0;
                        let mut times = vec![0.0];
                        for step in d {
                            at += step;
                            times.push(at / total);
                        }
                        times
                    }
                    _ => (0..n).map(|i| i as f64 / (n - 1) as f64).collect(),
                }
            }
            None => (0..n).map(|i| i as f64 / (n - 1) as f64).collect(),
        };
        let i = times.iter().rposition(|t| *t <= p).unwrap_or(0).min(n - 2);
        let span = times[i + 1] - times[i];
        let mut local = if span > 0.0 { ((p - times[i]) / span).clamp(0.0, 1.0) } else { 1.0 };
        if let Some(p) = c.splines.as_ref().and_then(|s| s.get(i)) {
            local = spline(p, local);
        }
        space.lerp(&values[i], &values[i + 1], local)
    };
    // SMIL 3.3.2: each repeat adds the last value of the function once more.
    if c.accumulate && c.iteration > 0.0 && space.additive() {
        let last = space.scale(&values[n - 1], c.iteration);
        result = space.add(&result, &last);
    }
    Some(result)
}

/// The animated value: the animations of `contributions` (in sandwich order) applied over `base`.
pub(crate) fn fold<S: Space>(space: &S, contributions: &[Contribution], base: S::V) -> S::V {
    let mut current = base;
    for c in contributions {
        if let Some(value) = evaluate(space, c, &current) {
            current = if c.additive && space.additive() { space.combine(&current, &value) } else { value };
        }
    }
    current
}

/// Repeats of one interval one [`pump`] dispatches at most.
const MAX_REPEAT_EVENTS: usize = 1000;

/// Plans a [`pump`] after `delay`, replacing the plan made before.
fn schedule_pump(page: &Rc<PageDom>, delay: Duration) {
    // (A page without an embedder runs its tasks at once: nothing to wait for.)
    if page.hooks().is_none() || page.is_disposed() {
        return;
    }
    let cache = &page.animation_events.smil;
    let plan = cache.plan.get().wrapping_add(1);
    cache.plan.set(plan);
    let weak = Rc::downgrade(page);
    let task: Box<dyn FnOnce()> = Box::new(move || {
        if let Some(page) = weak.upgrade() {
            if page.animation_events.smil.plan.get() == plan {
                pump(&page);
            }
        }
    });
    if delay.is_zero() {
        page.post_task(task);
    } else {
        page.post_delayed_task(delay, task);
    }
}

/// The document finished loading: its SMIL clock starts, and the elements that begin with it
/// dispatch `beginEvent`.
pub(crate) fn start(page: &Rc<PageDom>) {
    if page.dom.try_borrow().is_ok_and(|dom| dom.smil_seen()) {
        schedule_pump(page, Duration::ZERO);
    }
}

/// Checkpoint after a task: a changed document (an animation element inserted, `begin`
/// mutated) is looked at again in a task of its own.
pub(crate) fn after_task(page: &Rc<PageDom>) {
    let cache = &page.animation_events.smil;
    {
        let Ok(dom) = page.dom.try_borrow() else { return };
        if !dom.smil_seen() || cache.pumped.get() == Some(dom.version()) {
            return;
        }
    }
    if page.is_disposed() || page.timing().load_start.is_none() {
        return;
    }
    schedule_pump(page, Duration::ZERO);
}

/// The iteration (0-based) of an interval of animation element `timed` at `time`.
fn iteration_at(timed: &Timed, interval: &Interval, time: f64) -> u32 {
    if !timed.dur_ms.is_finite() {
        return 0;
    }
    (((time.min(interval.end) - interval.begin) / timed.dur_ms).floor().max(0.0)).min(u32::MAX as f64) as u32
}

/// What the move of the document's SMIL clock to `time` dispatches for animation element `timed`
/// with intervals `ivs`, given what it had dispatched (`st`, updated): `(event type, repeat number)`
/// in order. After a `seeking` move only the change of which interval is active dispatches: the
/// intervals skipped dispatch nothing, a repeat jumped over neither.
fn events_for(timed: &Timed, ivs: &[Interval], time: f64, seeking: bool, st: &mut Fired) -> Vec<(&'static str, u32)> {
    let mut due: Vec<(&'static str, u32)> = Vec::new();
    if seeking {
        match ivs.iter().rev().find(|i| i.begin <= time) {
            Some(iv) if st.begin == Some(iv.begin) => {
                if time < iv.end {
                    // Back inside an interval that ended: it begins again.
                    if st.ended {
                        due.push(("beginEvent", 0));
                        st.ended = false;
                    }
                    st.iteration = iteration_at(timed, iv, time);
                } else if !st.ended {
                    due.push(("endEvent", 0));
                    st.ended = true;
                }
            }
            Some(iv) => {
                if st.begin.is_some() && !st.ended {
                    due.push(("endEvent", 0));
                }
                if time < iv.end {
                    due.push(("beginEvent", 0));
                    *st = Fired { begin: Some(iv.begin), ended: false, iteration: iteration_at(timed, iv, time) };
                } else {
                    // Over without having been seen.
                    *st = Fired { begin: Some(iv.begin), ended: true, iteration: 0 };
                }
            }
            None => {
                if st.begin.is_some() && !st.ended {
                    due.push(("endEvent", 0));
                }
                *st = Fired::default();
            }
        }
        return due;
    }
    for iv in ivs.iter() {
        if iv.begin > time {
            break;
        }
        let seen = st.begin;
        match seen {
            // (Older than what was dispatched: its time passed.)
            Some(begun) if iv.begin < begun => continue,
            Some(begun) if iv.begin == begun => {}
            _ => {
                if st.begin.is_some() && !st.ended {
                    due.push(("endEvent", 0));
                }
                due.push(("beginEvent", 0));
                *st = Fired { begin: Some(iv.begin), ended: false, iteration: 0 };
            }
        }
        if timed.dur_ms.is_finite() {
            // The repeats before the end (none at it), up to now.
            for _ in 0..MAX_REPEAT_EVENTS {
                let next = st.iteration.saturating_add(1);
                let at = iv.begin + f64::from(next) * timed.dur_ms;
                if at >= iv.end || at > time || next == u32::MAX {
                    break;
                }
                due.push(("repeatEvent", next));
                st.iteration = next;
            }
        }
        if iv.end <= time && !st.ended {
            due.push(("endEvent", 0));
            st.ended = true;
        }
    }
    // The interval it had begun is gone (its `begin` was changed, or the instance removed).
    if let Some(begun) = st.begin {
        if !st.ended && !ivs.iter().any(|iv| iv.begin == begun) {
            due.push(("endEvent", 0));
            st.ended = true;
        }
    }
    due
}

/// Dispatches the `beginEvent`, `repeatEvent` and `endEvent` the document's SMIL time reached, and
/// plans the run at the next one. See [`events_for`].
pub(crate) fn pump(page: &Rc<PageDom>) {
    if page.is_disposed() {
        return;
    }
    // (Handler attributes the parser made are installed before asking who listens.)
    crate::handlers::drain_inline(page);
    let cache = &page.animation_events.smil;
    let Some(time) = document_time(page, page.timeline_now_ms()) else { return };
    // The element, event type and repeat number of each event.
    let mut due: Vec<(NodeId, &'static str, u32)> = Vec::new();
    let mut next = f64::INFINITY;
    let observed;
    let read;
    {
        let Ok(dom) = page.dom.try_borrow() else {
            schedule_pump(page, Duration::from_millis(1));
            return;
        };
        let Some(found) = cache.animations(&dom, page.document()) else { return };
        read = found;
        cache.pumped.set(Some(dom.version()));
        let seeking = cache.seeked.replace(false);
        let intervals = cache.intervals(&read);
        observed = read.has_events || ["beginEvent", "endEvent", "repeatEvent"].iter().any(|ty| dom.listens_for(ty));
        let mut fired = cache.fired.borrow_mut();
        // An element that left the document starts afresh when it is inserted again.
        fired.retain(|node, _| read.timed.iter().any(|t| t.node == *node));
        for (i, timed) in read.timed.iter().enumerate() {
            let ivs = &intervals[i];
            if !dom.is_alive(timed.node) {
                continue;
            }
            // The next begin, end or (for a listener) repeat after now.
            for interval in ivs.iter() {
                for at in [interval.begin, interval.end] {
                    if at > time && at < next {
                        next = at;
                    }
                }
                if observed && interval.begin <= time && time < interval.end && timed.dur_ms.is_finite() {
                    let at = interval.begin + (((time - interval.begin) / timed.dur_ms).floor() + 1.0) * timed.dur_ms;
                    if at < interval.end && at < next {
                        next = at;
                    }
                }
            }
            let state = fired.entry(timed.node).or_default();
            for (ty, repeat) in events_for(timed, ivs, time, seeking, state) {
                due.push((timed.node, ty, repeat));
            }
        }
    }
    let plan = cache.plan.get();
    let mut added = false;
    if observed {
        for (node, ty, repeat) in due {
            // (The event values `beginEvent` and `endEvent` are syncbase times: `parse_spec`.)
            if ty == "repeatEvent" {
                added |= note_event(page, &read, node, "repeatEvent", false, time);
                added |= note_event(page, &read, node, &format!("repeat({repeat})"), false, time);
            }
            fire_event(page, node, ty);
        }
    }
    // Something a handler did planned the next run already.
    if cache.plan.get() != plan {
        return;
    }
    // An event value of a repeat began or ended an animation now: it dispatches in the next run.
    if added {
        schedule_pump(page, Duration::ZERO);
        return;
    }
    if next.is_finite() && cache.seek.get().paused_ms.is_none() {
        schedule_pump(page, crate::util::delay_from_ms((next - time).max(1.0)));
    }
}

/// Dispatches event `ty` (it does not bubble, and cannot be canceled) at animation element `node`.
fn fire_event(page: &Rc<PageDom>, node: NodeId, ty: &'static str) {
    {
        let Ok(dom) = page.dom.try_borrow() else { return };
        if !dom.is_alive(node) || !dom.is_connected(node).unwrap_or(false) {
            return;
        }
    }
    let event = EventCore::trusted(page, EventInterface::Event, ty, false, false);
    crate::target::fire_at_node(page, node, &event);
}

/// Whether `ancestor` is an ancestor of `node`.
fn is_ancestor(dom: &Dom, ancestor: NodeId, node: NodeId) -> bool {
    let mut at = dom.parent(node).ok().flatten();
    while let Some(parent) = at {
        if parent == ancestor {
            return true;
        }
        at = dom.parent(parent).ok().flatten();
    }
    false
}

/// Event `ty` happened at `target` (reaching its ancestors when it `bubbles`) at document time `at`:
/// adds an instance time to each `begin` or `end` list with an event value naming it. Whether any
/// was added.
fn note_event(page: &PageDom, read: &Read, target: NodeId, ty: &str, bubbles: bool, at: f64) -> bool {
    let cache = &page.animation_events.smil;
    let Ok(dom) = page.dom.try_borrow() else { return false };
    let doc = page.document();
    let mut added = false;
    for timed in &read.timed {
        for (ends, specs) in [(false, &timed.begins), (true, &timed.ends)] {
            for spec in specs {
                let Spec::Event { id, name, offset } = spec else { continue };
                if name.as_str() != ty {
                    continue;
                }
                let base = match id {
                    Some(id) => dom.get_element_by_id(doc, id).ok().flatten(),
                    None => timed.target,
                };
                let Some(base) = base else { continue };
                if base == target || (bubbles && is_ancestor(&dom, base, target)) {
                    cache.add_instance(timed.node, ends, at + offset);
                    added = true;
                }
            }
        }
    }
    added
}

/// Dispatch of event `ty` at `target` is about to run (`target::dispatch_at`): the animations with an
/// event value for it begin or end at its time (plus the offset).
pub(crate) fn on_event(page: &Rc<PageDom>, target: NodeId, ty: &str, bubbles: bool) {
    let cache = &page.animation_events.smil;
    let read = {
        let Ok(dom) = page.dom.try_borrow() else { return };
        match cache.animations(&dom, page.document()) {
            Some(read) if read.has_events => read,
            _ => return,
        }
    };
    // (An event before the document's load event happens at the clock's start.)
    let at = document_time(page, page.timeline_now_ms()).unwrap_or(cache.seek.get().offset_ms);
    if note_event(page, &read, target, ty, bubbles, at) {
        schedule_pump(page, Duration::ZERO);
    }
}

/// `beginElement()`, `beginElementAt(offset)` (`end`: false) and `endElement()`, `endElementAt(offset)`
/// (`end`: true): an instance time `offset_s` seconds from now in the element's `begin` or `end` list.
pub(crate) fn add_instance(page: &Rc<PageDom>, node: NodeId, end: bool, offset_s: f64) {
    let cache = &page.animation_events.smil;
    let now = document_time(page, page.timeline_now_ms()).unwrap_or(cache.seek.get().offset_ms);
    cache.add_instance(node, end, now + offset_s * 1000.0);
    schedule_pump(page, Duration::ZERO);
}

/// The first interval of animation element `node` that has not ended at the current time: the
/// current one, else the next. `None`: there is none.
fn current_interval(page: &PageDom, node: NodeId) -> Option<Interval> {
    let dom = page.dom.try_borrow().ok()?;
    let cache = &page.animation_events.smil;
    let read = cache.animations(&dom, page.document())?;
    let index = read.timed.iter().position(|t| t.node == node)?;
    let time = document_time(page, page.timeline_now_ms()).unwrap_or(cache.seek.get().offset_ms);
    let intervals = cache.intervals(&read);
    intervals[index].iter().find(|i| i.end > time).copied()
}

/// `getStartTime()` in seconds: the begin of the element's current (else next) interval; `None`
/// (InvalidStateError) when it has none.
pub(crate) fn start_time(page: &PageDom, node: NodeId) -> Option<f64> {
    current_interval(page, node).map(|i| (i.begin / 1000.0) as f32 as f64)
}

/// `getSimpleDuration()` in seconds; `None` (NotSupportedError) when it is indefinite.
pub(crate) fn simple_duration(page: &PageDom, node: NodeId) -> Option<f64> {
    let dom = page.dom.try_borrow().ok()?;
    let read = page.animation_events.smil.animations(&dom, page.document())?;
    let timed = read.timed.iter().find(|t| t.node == node)?;
    timed.dur_ms.is_finite().then_some((timed.dur_ms / 1000.0) as f32 as f64)
}

/// `SVGAnimationElement.targetElement`: `href`, `xlink:href`, else the parent.
pub(crate) fn target_element(page: &PageDom, node: NodeId) -> Option<NodeId> {
    let dom = page.dom.try_borrow().ok()?;
    target_of(&dom, page.document(), node)
}

/// Whether the document has `<animate>` elements to sample (the layout
/// engine then samples the page's animations at its syncs).
pub(crate) fn present(page: &PageDom, dom: &Dom) -> bool {
    page.animation_events.smil.animations(dom, page.document()).is_some_and(|a| a.animates.iter().any(|a| a.css.is_some()))
}

/// The DOM version the animations were read at: a change of the
/// `<animate>` elements is a change of the frame (`animation_frame::stamp`).
pub(crate) fn version(page: &PageDom) -> u64 {
    let read = page.animation_events.smil.read.borrow().as_ref().map_or(0, |(at, _)| *at);
    read ^ page.animation_events.smil.seek.get().generation.rotate_left(17)
}

#[cfg(test)]
mod tests {
    use super::{
        clock_ms, events_for, number_unit, parse_key_splines, parse_number_list, parse_spec, resolve_all, Dynamic, Fired,
        Interval, Read, Restart, Spec, Timed,
    };
    use pyro_dom::Dom;
    use std::collections::HashMap;

    fn timed(begins: Vec<Spec>, ends: Vec<Spec>, dur_ms: f64, active_ms: f64) -> Timed {
        Timed {
            node: node(),
            id: None,
            target: None,
            begins,
            ends,
            dur_ms,
            active_ms,
            min_ms: 0.0,
            max_ms: f64::INFINITY,
            restart: Restart::Always,
            disabled: false,
        }
    }

    fn node() -> pyro_dom::NodeId {
        Dom::new().create_document(pyro_dom::DocumentKind::Html, "about:blank")
    }

    fn read_of(timed: Vec<Timed>) -> Read {
        Read { timed, ..Read::default() }
    }

    #[test]
    fn begin_values() {
        assert_eq!(parse_spec("5s"), Some(Spec::Offset(5000.0)));
        assert_eq!(parse_spec(" indefinite "), Some(Spec::Indefinite));
        assert_eq!(parse_spec("a.end+1s"), Some(Spec::Sync { id: "a".into(), end: true, offset: 1000.0 }));
        assert_eq!(parse_spec("my-anim.begin - 250ms"), Some(Spec::Sync { id: "my-anim".into(), end: false, offset: -250.0 }));
        assert_eq!(parse_spec("a\\.b.end"), Some(Spec::Sync { id: "a.b".into(), end: true, offset: 0.0 }));
        assert_eq!(parse_spec("click"), Some(Spec::Event { id: None, name: "click".into(), offset: 0.0 }));
        assert_eq!(parse_spec("click+0.5s"), Some(Spec::Event { id: None, name: "click".into(), offset: 500.0 }));
        assert_eq!(parse_spec("e.test"), Some(Spec::Event { id: Some("e".into()), name: "test".into(), offset: 0.0 }));
        assert_eq!(parse_spec("anim.repeat(2)"), Some(Spec::Event { id: Some("anim".into()), name: "repeat(2)".into(), offset: 0.0 }));
        assert_eq!(parse_spec("a.end+"), None);
        assert_eq!(parse_spec(""), None);
    }

    #[test]
    fn intervals_restart_and_end() {
        // begin="0s; 10ms" end="20ms": the second begin ends the first interval.
        let read = read_of(vec![timed(vec![Spec::Offset(0.0), Spec::Offset(10.0)], vec![Spec::Offset(20.0)], f64::INFINITY, f64::INFINITY)]);
        let found = resolve_all(&read, &HashMap::new());
        assert_eq!(found[0], vec![Interval { begin: 0.0, end: 10.0 }, Interval { begin: 10.0, end: 20.0 }]);
        // begin="1s; 3s" dur="1s".
        let read = read_of(vec![timed(vec![Spec::Offset(1000.0), Spec::Offset(3000.0)], Vec::new(), 1000.0, 1000.0)]);
        let found = resolve_all(&read, &HashMap::new());
        assert_eq!(found[0], vec![Interval { begin: 1000.0, end: 2000.0 }, Interval { begin: 3000.0, end: 4000.0 }]);
        // An interval that ended before the time container began never was.
        let read = read_of(vec![timed(vec![Spec::Offset(-1000.0)], Vec::new(), 1000.0, 1000.0)]);
        assert!(resolve_all(&read, &HashMap::new())[0].is_empty());
        // `begin="indefinite"` waits for script.
        let mut read = read_of(vec![timed(vec![Spec::Indefinite], Vec::new(), 100.0, 100.0)]);
        assert!(resolve_all(&read, &HashMap::new())[0].is_empty());
        let node = read.timed[0].node;
        let mut dynamic = HashMap::new();
        dynamic.insert(node, Dynamic { begins: vec![50.0], ends: Vec::new() });
        read.has_sync = false;
        assert_eq!(resolve_all(&read, &dynamic)[0], vec![Interval { begin: 50.0, end: 150.0 }]);
    }

    #[test]
    fn syncbase_intervals() {
        // a: begin 0, dur 10ms; b: begin="a.end+5ms" dur 10ms.
        let a = timed(vec![Spec::Offset(0.0)], Vec::new(), 10.0, 10.0);
        let b = timed(vec![Spec::Sync { id: "a".into(), end: true, offset: 5.0 }], Vec::new(), 10.0, 10.0);
        let mut read = read_of(vec![a, b]);
        read.ids.insert("a".into(), 0);
        read.has_sync = true;
        let found = resolve_all(&read, &HashMap::new());
        assert_eq!(found[0], vec![Interval { begin: 0.0, end: 10.0 }]);
        assert_eq!(found[1], vec![Interval { begin: 15.0, end: 25.0 }]);
    }

    #[test]
    fn events_follow_the_clock() {
        // begin 5s, dur 1s, repeatCount 2: [5000, 7000].
        let t = timed(vec![Spec::Offset(5000.0)], Vec::new(), 1000.0, 2000.0);
        let ivs = [Interval { begin: 5000.0, end: 7000.0 }];
        let mut st = Fired::default();
        assert!(events_for(&t, &ivs, 4000.0, false, &mut st).is_empty());
        assert_eq!(events_for(&t, &ivs, 5000.0, false, &mut st), vec![("beginEvent", 0)]);
        assert_eq!(events_for(&t, &ivs, 6100.0, false, &mut st), vec![("repeatEvent", 1)]);
        assert_eq!(events_for(&t, &ivs, 7000.0, false, &mut st), vec![("endEvent", 0)]);
        // Seeking into an interval begins it; seeking within it, over a repeat, dispatches nothing.
        let mut st = Fired::default();
        assert_eq!(events_for(&t, &ivs, 5500.0, true, &mut st), vec![("beginEvent", 0)]);
        assert!(events_for(&t, &ivs, 6500.0, true, &mut st).is_empty());
        assert_eq!(events_for(&t, &ivs, 8000.0, true, &mut st), vec![("endEvent", 0)]);
        // Seeking past an interval that never began dispatches nothing for it.
        let mut st = Fired::default();
        assert!(events_for(&t, &ivs, 10_000.0, true, &mut st).is_empty());
    }

    #[test]
    fn lists() {
        assert_eq!(parse_number_list("0; 0.5;1;"), Some(vec![0.0, 0.5, 1.0]));
        assert_eq!(parse_number_list("0;;1"), None);
        assert_eq!(number_unit("10px"), Some((10.0, "px")));
        assert_eq!(number_unit("red"), None);
        // x of a control point stays in 0..1, y may leave it.
        assert_eq!(parse_key_splines("0.5 -1 0.5 2;"), Some(vec![[0.5, -1.0, 0.5, 2.0]]));
        assert_eq!(parse_key_splines("1.5 0 0.5 1"), None);
    }

    #[test]
    fn clock_values() {
        assert_eq!(clock_ms("10s"), Some(10_000.0));
        assert_eq!(clock_ms("0.1s"), Some(100.0));
        assert_eq!(clock_ms("250ms"), Some(250.0));
        assert_eq!(clock_ms("2"), Some(2000.0));
        assert_eq!(clock_ms("1min"), Some(60_000.0));
        assert_eq!(clock_ms("1h"), Some(3_600_000.0));
        assert_eq!(clock_ms("01:30"), Some(90_000.0));
        assert_eq!(clock_ms("00:00:02.5"), Some(2500.0));
        assert_eq!(clock_ms("-1s"), Some(-1000.0));
        assert_eq!(clock_ms("click"), None);
        assert_eq!(clock_ms("a.end+1s"), None);
    }
}
