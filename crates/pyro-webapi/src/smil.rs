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
//! SVG time containers of a document. Not read: event and syncbase `begin`
//! values (an animation with only those never begins), `end`, and the
//! `<animateMotion>` element. `<set>` is an animation of its `to` value, with
//! an indefinite simple duration unless `dur` is given.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use markup5ever::ns;
use pyro_dom::animation::{Composite, Effect, Keyframes, TextKeyframe, TextKeyframes};
use pyro_dom::{Dom, NodeId};

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
    begin_ms: f64,
    dur_ms: f64,
    /// The active duration: `dur` times `repeatCount`, cut by `repeatDur`.
    active_ms: f64,
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
    /// (simple-duration fraction, iteration index) at the end of the active duration.
    fn end_state(&self) -> (f64, f64) {
        if !self.dur_ms.is_finite() {
            return (0.0, 0.0);
        }
        let iterations = self.active_ms / self.dur_ms;
        let fraction = iterations.fract();
        if fraction == 0.0 {
            (1.0, iterations - 1.0)
        } else {
            (fraction, iterations.floor())
        }
    }

    /// (simple-duration fraction, iteration index) at document time `time` (ms), while the animation
    /// is active or frozen.
    fn simple_state(&self, time: f64) -> Option<(f64, f64)> {
        let active = time - self.begin_ms;
        if active < 0.0 {
            None
        } else if active < self.active_ms {
            Some(((active % self.dur_ms) / self.dur_ms, (active / self.dur_ms).floor()))
        } else if self.freeze {
            Some(self.end_state())
        } else {
            None
        }
    }
}

/// The document's `<animate>` elements, read again when the DOM changed.
#[derive(Default)]
pub(crate) struct Cache {
    read: RefCell<Option<(u64, Rc<Vec<Animate>>)>>,
    /// `SVGSVGElement` time control (`setCurrentTime`, `pauseAnimations`), of
    /// the document's one SMIL clock (SVG gives each outermost `<svg>` its
    /// own; pyro's animations all run on the document's).
    seek: std::cell::Cell<Seek>,
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
pub(crate) fn set_current_time(page: &PageDom, seconds: f64) {
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
}

/// `pauseAnimations()` (`true`) and `unpauseAnimations()` (`false`).
pub(crate) fn set_paused(page: &PageDom, paused: bool) {
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
}

/// `animationsPaused()`.
pub(crate) fn paused(page: &PageDom) -> bool {
    page.animation_events.smil.seek.get().paused_ms.is_some()
}

impl Cache {
    pub(crate) fn clear(&self) {
        self.read.borrow_mut().take();
    }

    /// The document's animations (none, and nothing read, until an SVG
    /// `<animate>` element was ever created: [`Dom::smil_seen`]).
    fn animations(&self, dom: &Dom, doc: NodeId) -> Option<Rc<Vec<Animate>>> {
        if !dom.smil_seen() {
            return None;
        }
        let version = dom.version();
        if let Some((at, read)) = &*self.read.borrow() {
            if *at == version {
                return Some(read.clone());
            }
        }
        let read = Rc::new(read_animations(dom, doc));
        *self.read.borrow_mut() = Some((version, read.clone()));
        Some(read)
    }
}

fn read_animations(dom: &Dom, doc: NodeId) -> Vec<Animate> {
    let Ok(descendants) = dom.descendants(doc) else { return Vec::new() };
    descendants
        .filter_map(|id| {
            let e = dom.element(id).ok()?;
            if e.name.ns != ns!(svg) {
                return None;
            }
            let kind = match &*e.name.local {
                "animate" => Kind::Animate,
                "animateTransform" => Kind::Transform,
                "set" => Kind::Set,
                _ => return None,
            };
            read_animate(dom, doc, id, kind)
        })
        .collect()
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
    // The earliest offset of the begin list; a list of none (only events or
    // syncbases) never begins.
    let begin_ms = match attr("begin") {
        Some(list) => list.split(';').filter_map(clock_ms).reduce(f64::min)?,
        None => 0.0,
    };
    let dur_ms = match attr("dur").map(str::trim) {
        None | Some("indefinite") if kind == Kind::Set => f64::INFINITY,
        Some(text) => clock_ms(text).filter(|d| *d > 0.0)?,
        None => return None,
    };
    let repeat_count = match attr("repeatCount").map(str::trim) {
        Some("indefinite") => Some(f64::INFINITY),
        Some(count) => Some(count.parse::<f64>().ok().filter(|c| *c > 0.0)?),
        None => None,
    };
    let repeat_dur = match attr("repeatDur").map(str::trim) {
        Some("indefinite") => Some(f64::INFINITY),
        Some(text) => clock_ms(text).filter(|d| *d > 0.0),
        None => None,
    };
    // SMIL 3.6.5: the active duration is the repeat duration cut by `repeatDur`.
    let active_ms = match (repeat_count, repeat_dur) {
        (None, None) => dur_ms,
        (Some(count), None) => dur_ms * count,
        (None, Some(duration)) => duration,
        (Some(count), Some(duration)) => (dur_ms * count).min(duration),
    };
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
        begin_ms,
        dur_ms,
        active_ms,
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
    let Some(animations) = page.animation_events.smil.animations(&dom, page.document()) else { return false };
    // The time containers start at the load event.
    let time = match now {
        Some(now) => match document_time(page, now) {
            Some(time) => Some(time),
            None => return animations.iter().any(|a| a.css.is_some()),
        },
        None => None,
    };
    let paused = page.animation_events.smil.seek.get().paused_ms.is_some();
    let mut moving = false;
    for a in animations.iter() {
        let Some(css) = a.css.as_ref() else { continue };
        // The simple time's fraction.
        let progress = match time {
            None if a.active_ms.is_finite() && a.freeze => a.end_state().0,
            // (A `set` without `dur` holds its value.)
            None if a.kind == Kind::Set && !a.dur_ms.is_finite() => 0.0,
            None => continue,
            Some(time) => {
                if time - a.begin_ms < 0.0 {
                    moving |= !paused;
                    continue;
                }
                // (A value that does not change over its simple duration does not move.)
                if time - a.begin_ms < a.active_ms && a.dur_ms.is_finite() {
                    moving |= !paused;
                }
                match a.simple_state(time) {
                    Some((progress, _)) => progress,
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
    let Some(animations) = page.animation_events.smil.animations(&dom, page.document()) else { return Vec::new() };
    let Some(time) = document_time(page, page.timeline_now_ms()) else { return Vec::new() };
    let mut found: Vec<(f64, Contribution)> = Vec::new();
    for a in animations.iter() {
        if a.target != target || a.property != attribute || !a.valid {
            continue;
        }
        let Some((progress, iteration)) = a.simple_state(time) else { continue };
        if !dom.is_alive(a.target) {
            continue;
        }
        if let Some(c) = contribution(a, progress, iteration) {
            found.push((a.begin_ms, c));
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

/// Whether the document has `<animate>` elements to sample (the layout
/// engine then samples the page's animations at its syncs).
pub(crate) fn present(page: &PageDom, dom: &Dom) -> bool {
    page.animation_events.smil.animations(dom, page.document()).is_some_and(|a| a.iter().any(|a| a.css.is_some()))
}

/// The DOM version the animations were read at: a change of the
/// `<animate>` elements is a change of the frame (`animation_frame::stamp`).
pub(crate) fn version(page: &PageDom) -> u64 {
    let read = page.animation_events.smil.read.borrow().as_ref().map_or(0, |(at, _)| *at);
    read ^ page.animation_events.smil.seek.get().generation.rotate_left(17)
}

#[cfg(test)]
mod tests {
    use super::{clock_ms, number_unit, parse_key_splines, parse_number_list};

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
