//! SMIL `<animate>` of CSS properties (SVG animation elements), sampled with
//! the page's other animations ([`crate::animation_frame`]): each one in
//! effect is a keyframe effect on its target, below the CSS animations of
//! the same element (Chrome cascades SMIL-animated properties under CSS
//! animations).
//!
//! What is read: the target (`href`, `xlink:href`, else the parent),
//! `attributeName` (a CSS property; other attributes are not animated),
//! `values` or `from` and `to`, `keyTimes`, `calcMode` (`discrete`; `linear`,
//! `paced` and `spline` interpolate, `keySplines` giving each interval's
//! easing), `begin` (its offset values, the earliest), `dur`, `repeatCount`
//! and `fill="freeze"`. The document's animation time starts at its load
//! event, as Chrome starts the SVG time containers of a document. Not read:
//! event and syncbase `begin` values (an animation with only those never
//! begins), `end`, `repeatDur`, `by`, `additive`, `accumulate`, and the
//! `<set>`, `<animateTransform>` and `<animateMotion>` elements.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use markup5ever::{local_name, ns};
use pyro_dom::animation::{Composite, Effect, Keyframes, TextKeyframe, TextKeyframes};
use pyro_dom::{Dom, NodeId};

use crate::page::PageDom;

/// One `<animate>` as its attributes describe it.
pub(crate) struct Animate {
    target: NodeId,
    /// The animated property, as `attributeName` names it.
    property: String,
    values: Vec<String>,
    /// Each value's time, a fraction of the simple duration.
    key_times: Vec<f64>,
    /// Each interval's easing (`calcMode="spline"`).
    key_splines: Option<Vec<String>>,
    discrete: bool,
    begin_ms: f64,
    dur_ms: f64,
    repeat: f64,
    freeze: bool,
}

/// The document's `<animate>` elements, read again when the DOM changed.
#[derive(Default)]
pub(crate) struct Cache {
    read: RefCell<Option<(u64, Rc<Vec<Animate>>)>>,
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
        .filter(|id| dom.element(*id).is_ok_and(|e| e.name.ns == ns!(svg) && e.name.local == local_name!("animate")))
        .filter_map(|id| read_animate(dom, doc, id))
        .collect()
}

fn read_animate(dom: &Dom, doc: NodeId, id: NodeId) -> Option<Animate> {
    let attr = |name: &str| dom.get_attribute(id, name).ok().flatten();
    let target = match attr("href").or_else(|| attr("xlink:href")) {
        Some(href) => dom.get_element_by_id(doc, href.trim().strip_prefix('#')?).ok().flatten()?,
        None => dom.parent(id).ok().flatten().filter(|p| dom.element(*p).is_ok())?,
    };
    let property = attr("attributeName")?.trim();
    if property.is_empty() || property.contains(':') {
        return None;
    }
    let values: Vec<String> = match attr("values") {
        Some(list) => list.split(';').map(str::trim).filter(|v| !v.is_empty()).map(str::to_owned).collect(),
        None => vec![attr("from")?.trim().to_owned(), attr("to")?.trim().to_owned()],
    };
    if values.is_empty() {
        return None;
    }
    let mode = attr("calcMode").map(str::trim).unwrap_or("linear");
    let discrete = mode == "discrete";
    let n = values.len();
    // Evenly spaced without `keyTimes`: `n` intervals of one value each when
    // discrete, `n - 1` between the values otherwise.
    let key_times = match attr("keyTimes") {
        Some(list) => {
            let times: Vec<f64> = list.split(';').map(|t| t.trim().parse::<f64>().ok()).collect::<Option<_>>()?;
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
            let splines: Vec<String> = list
                .split(';')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| {
                    let p: Vec<f64> = s
                        .split(|c: char| c == ',' || c.is_whitespace())
                        .filter(|t| !t.is_empty())
                        .map(|t| t.parse::<f64>().ok())
                        .collect::<Option<_>>()?;
                    (p.len() == 4 && p.iter().all(|v| (0.0..=1.0).contains(v)))
                        .then(|| format!("cubic-bezier({}, {}, {}, {})", p[0], p[1], p[2], p[3]))
                })
                .collect::<Option<_>>()?;
            if splines.len() + 1 != n {
                return None;
            }
            Some(splines)
        }
        ("spline", None) => return None,
        _ => None,
    };
    // The earliest offset of the begin list; a list of none (only events or
    // syncbases) never begins.
    let begin_ms = match attr("begin") {
        Some(list) => list.split(';').filter_map(clock_ms).reduce(f64::min)?,
        None => 0.0,
    };
    let dur_ms = clock_ms(attr("dur")?).filter(|d| *d > 0.0)?;
    let repeat = match attr("repeatCount").map(str::trim) {
        Some("indefinite") => f64::INFINITY,
        Some(count) => count.parse::<f64>().ok().filter(|c| *c > 0.0)?,
        None => 1.0,
    };
    let freeze = attr("fill").map(str::trim) == Some("freeze");
    Some(Animate {
        target,
        property: property.to_owned(),
        values,
        key_times,
        key_splines,
        discrete,
        begin_ms,
        dur_ms,
        repeat,
        freeze,
    })
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

/// Each `<animate>` in effect at page time `now`, in document order:
/// `visit(target, effect)`. Returns whether a value moves with time (one
/// running, or one yet to begin). `now` `None`: settled, as Playwright's
/// `animations: "disabled"` leaves CSS animations (a finite one at its end,
/// so only one that freezes shows; an infinite one gone).
pub(crate) fn walk(page: &PageDom, now: Option<f64>, visit: &mut dyn FnMut(NodeId, Effect)) -> bool {
    let Ok(dom) = page.dom.try_borrow() else { return false };
    let Some(animations) = page.animation_events.smil.animations(&dom, page.document()) else { return false };
    // The time containers start at the load event.
    let time = match now {
        Some(now) => match page.timing().load_start {
            Some(load) => Some(now - load),
            None => return !animations.is_empty(),
        },
        None => None,
    };
    let mut moving = false;
    for a in animations.iter() {
        // Where the active duration ends: a whole number of iterations ends
        // on the last value.
        let end = || {
            let fraction = a.repeat.fract();
            if fraction == 0.0 {
                1.0
            } else {
                fraction
            }
        };
        // The simple time's fraction.
        let progress = match time {
            None if a.repeat.is_finite() && a.freeze => end(),
            None => continue,
            Some(time) => {
                let active = time - a.begin_ms;
                if active < 0.0 {
                    moving = true;
                    continue;
                }
                if active < a.dur_ms * a.repeat {
                    moving = true;
                    (active % a.dur_ms) / a.dur_ms
                } else if a.freeze {
                    end()
                } else {
                    continue;
                }
            }
        };
        let declaration = |v: &str| format!("{}: {}", a.property, v);
        let effect = |frames: Vec<TextKeyframe>, progress: f64| Effect {
            keyframes: Keyframes::Text(Arc::new(TextKeyframes { frames })),
            progress,
            before: false,
            easing: None,
            composite: Composite::Replace,
        };
        let constant = |v: &str| {
            let frame = |offset| TextKeyframe { offset, easing: None, composite: None, declarations: declaration(v) };
            effect(vec![frame(0.0), frame(1.0)], 0.0)
        };
        let e = if a.discrete || a.values.len() == 1 {
            // The value whose time came last.
            let i = a.key_times.iter().rposition(|t| *t <= progress).unwrap_or(0);
            constant(&a.values[i])
        } else {
            let frames = a
                .values
                .iter()
                .zip(&a.key_times)
                .enumerate()
                .map(|(i, (v, t))| TextKeyframe {
                    offset: *t,
                    easing: a.key_splines.as_ref().and_then(|s| s.get(i).cloned()),
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

/// Whether the document has `<animate>` elements to sample (the layout
/// engine then samples the page's animations at its syncs).
pub(crate) fn present(page: &PageDom, dom: &Dom) -> bool {
    page.animation_events.smil.animations(dom, page.document()).is_some_and(|a| !a.is_empty())
}

/// The DOM version the animations were read at: a change of the
/// `<animate>` elements is a change of the frame (`animation_frame::stamp`).
pub(crate) fn version(page: &PageDom) -> u64 {
    page.animation_events.smil.read.borrow().as_ref().map_or(0, |(at, _)| *at)
}

#[cfg(test)]
mod tests {
    use super::clock_ms;

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
