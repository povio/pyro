//! The page's animated values at one moment
//! ([`pyro_dom::animation::AnimationFrame`]): what the layout engine samples
//! into its cascade, so that layout, computed styles, screenshots, PDFs and
//! screencast frames show CSS animations, CSS transitions and Web Animations
//! where they are now.
//!
//! The timelines are the page's own: CSS animations and transitions as
//! `animation_events.rs` keeps them (started at the style change that started
//! them), and the `Animation` objects of Web Animations as animations.js
//! reports them on every change of their state ([`report`]). A CSS animation
//! or transition that script paused, seeked or otherwise controlled through
//! its `CSSAnimation`/`CSSTransition` object follows that object's state
//! instead of its own. Each effect's iteration progress is computed here with
//! the Web Animations timing model ([`progress`]); the layout engine resolves
//! the keyframes and interpolates (blitz-dom's `animation` module).
//!
//! Layout queries and computed styles see CSS and script animations;
//! transitions only reach captures: a transition's own value would hide the
//! property's base value from the style change events that start and
//! retarget transitions (`animation_events.rs` reads them from the layout
//! engine's styles).
//!
//! Nothing ticks: a sync asks for the frame when its stamp changed ([`stamp`]:
//! the timelines' state, and the clock in frames of [`FRAME_MS`] while a value
//! moves), and a screenshot or screencast frame asks at the moment it is
//! taken.

use std::rc::{Rc, Weak};
use std::sync::Arc;

use pyro_dom::animation::{AnimationFrame, Composite, Effect, ElementAnimations, Keyframes, TextKeyframe, TextKeyframes};
use pyro_dom::NodeId;
use rustc_hash::FxHashMap;

use crate::page::PageDom;

/// The clock's step for layout syncs: within one, layout queries and
/// computed styles read the same animated values (Chrome's animation time is
/// the frame's).
pub(crate) const FRAME_MS: f64 = 16.0;

/// How an effect fills outside its active interval.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Fill {
    None,
    Forwards,
    Backwards,
    Both,
}

impl Fill {
    fn parse(text: &str) -> Fill {
        match text {
            "forwards" => Fill::Forwards,
            "backwards" => Fill::Backwards,
            "both" => Fill::Both,
            _ => Fill::None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Direction {
    Normal,
    Reverse,
    Alternate,
    AlternateReverse,
}

impl Direction {
    fn parse(text: &str) -> Direction {
        match text {
            "reverse" => Direction::Reverse,
            "alternate" => Direction::Alternate,
            "alternate-reverse" => Direction::AlternateReverse,
            _ => Direction::Normal,
        }
    }
}

/// An effect's timing (Web Animations `EffectTiming`, `auto` resolved).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Timing {
    pub delay: f64,
    pub end_delay: f64,
    pub fill: Fill,
    pub iteration_start: f64,
    pub iterations: f64,
    pub duration: f64,
    pub direction: Direction,
}

/// Where an effect is at one local time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Progress {
    /// The directed progress (before the effect's easing).
    pub progress: f64,
    /// The before flag its easing takes.
    pub before: bool,
    /// In the before or active phase: the value moves as time goes on.
    pub moving: bool,
}

/// The iteration progress of an effect with `timing` at `local` time, for
/// playback `rate` (Web Animations, "calculating the iteration progress";
/// animations.js's `computeTiming` without the easing). `None`: not in
/// effect.
pub(crate) fn progress(timing: &Timing, local: f64, rate: f64) -> Option<Progress> {
    let duration = timing.duration.max(0.0);
    let active = if duration == 0.0 || timing.iterations == 0.0 { 0.0 } else { duration * timing.iterations };
    let end = (timing.delay + active + timing.end_delay).max(0.0);
    let backwards = rate < 0.0;
    let before_active = timing.delay.min(end).max(0.0);
    let active_after = (timing.delay + active).min(end).max(0.0);
    let phase_before = local < before_active || (backwards && local == before_active);
    let phase_after = !phase_before && (local > active_after || (!backwards && local == active_after));
    let active_time = if phase_before {
        match timing.fill {
            Fill::Backwards | Fill::Both => (local - timing.delay).max(0.0),
            _ => return None,
        }
    } else if !phase_after {
        local - timing.delay
    } else {
        match timing.fill {
            Fill::Forwards | Fill::Both => (local - timing.delay).min(active).max(0.0),
            _ => return None,
        }
    };
    let overall = if duration == 0.0 {
        if phase_before { timing.iteration_start } else { timing.iteration_start + timing.iterations }
    } else {
        active_time / duration + timing.iteration_start
    };
    let mut simple = if overall.is_infinite() { timing.iteration_start % 1.0 } else { overall % 1.0 };
    if simple == 0.0 && !phase_before && active_time == active && timing.iterations != 0.0 && overall != 0.0 {
        simple = 1.0;
    }
    let iteration = if phase_after && timing.iterations.is_infinite() {
        f64::INFINITY
    } else if simple == 1.0 {
        overall.floor() - 1.0
    } else {
        overall.floor()
    };
    let forwards = match timing.direction {
        Direction::Normal => true,
        Direction::Reverse => false,
        Direction::Alternate | Direction::AlternateReverse => {
            let d = if timing.direction == Direction::AlternateReverse { iteration + 1.0 } else { iteration };
            d.is_infinite() || d % 2.0 == 0.0
        }
    };
    let directed = if forwards { simple } else { 1.0 - simple };
    let before = (phase_before && !backwards) || (phase_after && backwards);
    Some(Progress { progress: directed, before, moving: !phase_after })
}

/// An `Animation` object's state, as animations.js reports it.
pub(crate) struct Scripted {
    /// Its target element (`None`: no target, or a pseudo-element's).
    el: Option<NodeId>,
    /// The CSS animation or transition (`animation_events` id) this object
    /// is, or `None` for a script animation.
    css: Option<u64>,
    keyframes: Arc<TextKeyframes>,
    start: Option<f64>,
    hold: Option<f64>,
    rate: f64,
    /// The timeline's origin on the page clock (`None`: no timeline).
    origin: Option<f64>,
    timing: Timing,
    easing: Option<Arc<str>>,
    composite: Composite,
    /// Its place in the composite order (creation order).
    seq: u64,
    /// It shows nothing (a CSS animation's or transition's object canceled,
    /// finished without fill, or without a target).
    gone: bool,
}

impl Scripted {
    /// The CSS animation or transition this object is.
    pub(crate) fn css_id(&self) -> Option<u64> {
        self.css
    }

    /// The current time at `now` (`None`: idle, gone, or no timeline time).
    fn current(&self, now: f64) -> Option<f64> {
        if self.gone {
            return None;
        }
        if let Some(hold) = self.hold {
            return Some(hold);
        }
        Some((now - self.origin? - self.start?) * self.rate)
    }
}

/// `report(seq, state)` from animations.js: an `Animation` object's state
/// after a change (`state` `None`: it has no effect any more).
pub(crate) fn report(page: &Rc<PageDom>, seq: u64, state: Option<ReportedState>) {
    let events = &page.animation_events;
    {
        let Ok(mut scripted) = events.scripted.try_borrow_mut() else { return };
        match state {
            Some(s) => {
                scripted.insert(
                    seq,
                    Scripted {
                        el: s.el,
                        css: s.css,
                        keyframes: s.keyframes,
                        start: s.start,
                        hold: s.hold,
                        rate: s.rate,
                        origin: s.origin,
                        timing: s.timing,
                        easing: s.easing,
                        composite: s.composite,
                        seq,
                        gone: s.gone,
                    },
                );
            }
            None => {
                scripted.remove(&seq);
            }
        }
    }
    events.generation.set(events.generation.get().wrapping_add(1));
    if let Some(layout) = page.try_layout() {
        events.install_source(page, layout.style());
    }
}

/// What [`report`] takes.
pub(crate) struct ReportedState {
    pub el: Option<NodeId>,
    pub css: Option<u64>,
    pub keyframes: Arc<TextKeyframes>,
    pub start: Option<f64>,
    pub hold: Option<f64>,
    pub rate: f64,
    pub origin: Option<f64>,
    pub timing: Timing,
    pub easing: Option<Arc<str>>,
    pub composite: Composite,
    pub gone: bool,
}

impl ReportedState {
    /// The timing from animations.js's strings: `fill` (`auto` is `none`)
    /// and `direction`.
    pub fn timing(
        delay: f64,
        end_delay: f64,
        fill: &str,
        iteration_start: f64,
        iterations: f64,
        duration: f64,
        direction: &str,
    ) -> Timing {
        Timing { delay, end_delay, fill: Fill::parse(fill), iteration_start, iterations, duration, direction: Direction::parse(direction) }
    }
}

/// Keyframes from animations.js: `[offset, easing|null, composite|null,
/// declarations]` each.
pub(crate) fn keyframes(frames: Vec<(f64, Option<String>, Option<String>, String)>) -> Arc<TextKeyframes> {
    Arc::new(TextKeyframes {
        frames: frames
            .into_iter()
            .map(|(offset, easing, composite, declarations)| TextKeyframe {
                offset,
                easing: easing.filter(|e| e != "linear"),
                composite: composite.as_deref().and_then(Composite::parse),
                declarations,
            })
            .collect(),
    })
}

/// A CSS animation's timing from its style.
fn css_animation_timing(t: &crate::animation_events::Timeline) -> Option<(Timing, &pyro_dom::timing::AnimationStyle)> {
    let timing = t.timing()?;
    Some((
        Timing {
            delay: t.delay_ms,
            end_delay: 0.0,
            fill: Fill::parse(&timing.style.fill),
            iteration_start: 0.0,
            iterations: t.iterations,
            duration: t.duration_ms,
            direction: Direction::parse(&timing.style.direction),
        },
        &timing.style,
    ))
}

/// A CSS transition's timing (a `CSSTransition`'s effect fills backwards).
fn transition_timing(t: &crate::animation_events::Timeline) -> Timing {
    Timing {
        delay: t.delay_ms,
        end_delay: 0.0,
        fill: Fill::Backwards,
        iteration_start: 0.0,
        iterations: 1.0,
        duration: t.duration_ms,
        direction: Direction::Normal,
    }
}

/// The animated values at page time `now` (`transitions`: with the CSS
/// transitions, for a capture).
pub(crate) fn frame(page: &PageDom, now: f64, transitions: bool) -> AnimationFrame {
    let mut out = Builder::default();
    walk(page, now, transitions, &mut |el, transition, effect| out.push(el, transition, effect));
    let Ok(dom) = page.dom.try_borrow() else { return AnimationFrame::default() };
    AnimationFrame {
        elements: out
            .elements
            .into_iter()
            .filter(|(el, _)| dom.is_alive(*el) && dom.is_connected(*el).unwrap_or(false))
            .map(|(el, mut e)| {
                e.key = pyro_dom::snapshot::render_key(el);
                e
            })
            .collect(),
    }
}

#[derive(Default)]
struct Builder {
    elements: Vec<(NodeId, ElementAnimations)>,
    index: FxHashMap<NodeId, usize>,
}

impl Builder {
    fn push(&mut self, el: NodeId, transition: bool, effect: Effect) {
        let i = *self.index.entry(el).or_insert_with(|| {
            self.elements.push((el, ElementAnimations::default()));
            self.elements.len() - 1
        });
        let e = &mut self.elements[i].1;
        if transition { e.transitions.push(effect) } else { e.animations.push(effect) }
    }
}

/// Every effect in effect at `now`, in composite order per level: `visit(el,
/// transition, effect)`. Returns whether a value moves with time.
fn walk(page: &PageDom, now: f64, transitions: bool, visit: &mut dyn FnMut(NodeId, bool, Effect)) -> bool {
    let state = &page.animation_events;
    let Ok(scripted) = state.scripted.try_borrow() else { return false };
    let Some((animations, transition_list)) = state.try_timelines() else { return false };
    let overrides: FxHashMap<u64, &Scripted> =
        scripted.values().filter_map(|s| Some((s.css?, s))).collect();
    let mut moving = false;
    // Transitions, in the order they started.
    for t in transition_list.iter().filter(|t| t.is_live()) {
        let Some(keyframes) = t.keyframes() else { continue };
        let (local, rate, timing, easing) = match overrides.get(&t.id()) {
            Some(s) => match s.current(now) {
                Some(local) => (local, s.rate, s.timing.clone(), s.easing.clone()),
                None => continue,
            },
            None => (now - t.start_ms(), 1.0, transition_timing(t), Some(Arc::from(t.easing().unwrap_or("ease")))),
        };
        let Some(p) = progress(&timing, local, rate) else { continue };
        if transitions {
            moving |= p.moving && overrides.get(&t.id()).is_none_or(|s| s.hold.is_none() && s.rate != 0.0);
            // An effect script gave the object replaces the transition's.
            let (el, keyframes) = match overrides.get(&t.id()) {
                Some(s) if !s.keyframes.frames.is_empty() => (s.el.unwrap_or(t.el()), s.keyframes.clone()),
                _ => (t.el(), keyframes),
            };
            let effect = Effect {
                keyframes: Keyframes::Text(keyframes),
                progress: p.progress,
                before: p.before,
                easing,
                composite: Composite::Replace,
            };
            visit(el, true, effect);
        }
    }
    // CSS animations, in each element's `animation-name` order.
    for t in animations.iter().filter(|t| t.is_live()) {
        let Some((own_timing, style)) = css_animation_timing(t) else { continue };
        let (local, rate, timing, held) = match overrides.get(&t.id()) {
            Some(s) => match s.current(now) {
                Some(local) => (local, s.rate, s.timing.clone(), s.hold.is_some() || s.rate == 0.0),
                None => continue,
            },
            None => (t.local_ms(now), 1.0, own_timing, t.is_paused()),
        };
        let Some(p) = progress(&timing, local, rate) else { continue };
        moving |= p.moving && !held;
        let effect = match overrides.get(&t.id()) {
            // An effect script gave the object replaces the `@keyframes`.
            Some(s) if !s.keyframes.frames.is_empty() => Effect {
                keyframes: Keyframes::Text(s.keyframes.clone()),
                progress: p.progress,
                before: p.before,
                easing: s.easing.clone(),
                composite: s.composite,
            },
            _ => Effect {
                keyframes: Keyframes::Css {
                    name: Arc::from(t.name()),
                    easing: style.easing.as_deref().map(Arc::from),
                    composite: Composite::parse(&style.composition).unwrap_or_default(),
                },
                progress: p.progress,
                before: p.before,
                // The object's effect easing (`linear` unless script set one).
                easing: overrides.get(&t.id()).and_then(|s| s.easing.clone()),
                composite: Composite::Replace,
            },
        };
        let el = overrides.get(&t.id()).and_then(|s| s.el).unwrap_or(t.el());
        visit(el, false, effect);
    }
    // Script animations, in the order they were made.
    let mut script: Vec<&Scripted> = scripted.values().filter(|s| s.css.is_none()).collect();
    script.sort_by_key(|s| s.seq);
    for s in script {
        let Some(el) = s.el else { continue };
        let Some(local) = s.current(now) else { continue };
        let Some(p) = progress(&s.timing, local, s.rate) else { continue };
        moving |= p.moving && s.hold.is_none() && s.rate != 0.0;
        let effect = Effect {
            keyframes: Keyframes::Text(s.keyframes.clone()),
            progress: p.progress,
            before: p.before,
            easing: s.easing.clone(),
            composite: s.composite,
        };
        visit(el, false, effect);
    }
    moving
}

/// Changes whenever the frame at `now` could differ from an earlier one: the
/// timelines' state, and while a value moves with time, the clock in steps of
/// [`FRAME_MS`]. 0 while nothing animates.
pub(crate) fn stamp(page: &PageDom, now: f64) -> u64 {
    let generation = page.animation_events.generation.get();
    let moving = walk(page, now, false, &mut |_, _, _| {});
    let generation = generation.wrapping_mul(0x9E37_79B9_7F4A_7C15).rotate_left(17);
    if moving {
        generation ^ ((now / FRAME_MS).floor() as i64 as u64).wrapping_add(1)
    } else {
        generation
    }
}

/// Whether a CSS or script animation of `el` animates `longhand` (no
/// transition starts for it: its value changes with the animation's).
pub(crate) fn animates(
    state: &crate::animation_events::State,
    rules: &pyro_dom::timing::TimingRules,
    el: NodeId,
    longhand: &str,
) -> bool {
    let css = state
        .animations()
        .iter()
        .any(|t| t.is_live() && t.el() == el && rules.keyframes_declare(t.name(), longhand));
    if css {
        return true;
    }
    let Ok(scripted) = state.scripted.try_borrow() else { return false };
    scripted.values().any(|s| {
        s.css.is_none()
            && s.el == Some(el)
            && s.keyframes.frames.iter().any(|f| {
                f.declarations.split(';').any(|d| d.split(':').next().is_some_and(|p| p.trim() == longhand))
            })
    })
}

/// The page's [`pyro_dom::animation::AnimationSource`]: what the layout
/// engine samples at each sync (no transitions).
pub(crate) fn source(page: &Rc<PageDom>) -> pyro_dom::animation::SharedAnimationSource {
    Rc::new(Source { page: Rc::downgrade(page) })
}

struct Source {
    page: Weak<PageDom>,
}

impl pyro_dom::animation::AnimationSource for Source {
    fn stamp(&self) -> u64 {
        match self.page.upgrade() {
            Some(page) if !page.is_disposed() => stamp(&page, crate::event::page_now_ms(&page)),
            _ => 0,
        }
    }

    fn frame(&self) -> AnimationFrame {
        match self.page.upgrade() {
            Some(page) if !page.is_disposed() => frame(&page, crate::event::page_now_ms(&page), false),
            _ => AnimationFrame::default(),
        }
    }
}

impl PageDom {
    /// The style update of a rendering opportunity (the end of an animation
    /// frame, after its callbacks): the style changes since the last one start
    /// their transitions and animations now, as in a browser's frame. One
    /// flag and stamp check for a document that neither animates nor
    /// transitions, or did not change.
    pub fn update_animations(self: &Rc<Self>) {
        crate::animation_events::before_render(self);
    }

    /// The animated values now, transitions included: what a screenshot or
    /// screencast frame shows ([`pyro_dom::animation::AnimationFrame`]). The
    /// style changes since the last frame start their transitions and
    /// animations first, as at a rendering opportunity.
    pub fn animation_frame(self: &Rc<Self>) -> AnimationFrame {
        crate::animation_events::before_render(self);
        frame(self, crate::event::page_now_ms(self), true)
    }

    /// Changes whenever [`PageDom::animation_frame`] could show something
    /// else: the animations' state, and the clock while a value moves (in
    /// [`FRAME_MS`] steps). 0 while nothing animates.
    pub fn animation_stamp(&self) -> u64 {
        let now = crate::event::page_now_ms(self);
        let generation = self.animation_events.generation.get();
        let moving = walk(self, now, true, &mut |_, _, _| {});
        let generation = generation.wrapping_mul(0x9E37_79B9_7F4A_7C15).rotate_left(17);
        if moving {
            generation ^ ((now / FRAME_MS).floor() as i64 as u64).wrapping_add(1)
        } else {
            generation
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timing(delay: f64, duration: f64, iterations: f64, fill: Fill, direction: Direction) -> Timing {
        Timing { delay, end_delay: 0.0, fill, iteration_start: 0.0, iterations, duration, direction }
    }

    #[test]
    fn progress_follows_the_web_animations_timing_model() {
        // A negative delay starts the effect partway (paused CSS animations).
        let t = timing(-2000.0, 4000.0, 1.0, Fill::None, Direction::Normal);
        assert_eq!(progress(&t, 0.0, 1.0).map(|p| p.progress), Some(0.5));
        // Before the delay: only a backwards fill gives a value.
        let t = timing(100.0, 1000.0, 1.0, Fill::None, Direction::Normal);
        assert_eq!(progress(&t, 50.0, 1.0), None);
        let t = timing(100.0, 1000.0, 1.0, Fill::Backwards, Direction::Normal);
        let p = progress(&t, 50.0, 1.0).unwrap();
        assert_eq!((p.progress, p.before, p.moving), (0.0, true, true));
        // After the end: forwards fill holds the end, and the value stops moving.
        let t = timing(0.0, 1000.0, 2.0, Fill::Forwards, Direction::Alternate);
        let p = progress(&t, 5000.0, 1.0).unwrap();
        assert_eq!((p.progress, p.moving), (0.0, false));
        assert_eq!(progress(&timing(0.0, 1000.0, 2.0, Fill::None, Direction::Normal), 5000.0, 1.0), None);
        // Directions: the second iteration of `alternate` runs backwards.
        let t = timing(0.0, 1000.0, f64::INFINITY, Fill::None, Direction::Alternate);
        assert_eq!(progress(&t, 1250.0, 1.0).map(|p| p.progress), Some(0.75));
        let t = timing(0.0, 1000.0, 1.0, Fill::None, Direction::Reverse);
        assert_eq!(progress(&t, 250.0, 1.0).map(|p| p.progress), Some(0.75));
        // A zero duration with `both` fills at its end once started.
        let t = timing(0.0, 0.0, 1.0, Fill::Both, Direction::Normal);
        assert_eq!(progress(&t, 0.0, 1.0).map(|p| p.progress), Some(1.0));
    }
}
