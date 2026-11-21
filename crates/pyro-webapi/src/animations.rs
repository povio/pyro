//! Web Animations: `Element.animate()`, `Animation`, `KeyframeEffect`,
//! `document.timeline` and `getAnimations()`, made in JS (animations.js) on
//! a realm's first use of any of them, as streams.js is. Realms that never
//! animate pay one global accessor pair per interface name, and
//! `getAnimations()` answers `[]` without making the module unless CSS
//! animations or transitions run (`CSSAnimation`/`CSSTransition`, kept by
//! animation_events.rs from the first `getAnimations()` on).
//!
//! The script gets the page clock (`now`, which follows CDP virtual time),
//! one-shot timers on the page's event loop (`schedule`), Stylo's property
//! names (`cssName`) and the engine's record of each animation's state
//! (`report`, which animation_frame.rs samples into the cascade). Nothing
//! runs per frame: a playing animation has one timer, at its end.
use std::rc::Rc;

use pyro_js::{Args, ClassDef, JsClass, JsError, JsResult, Object, Persistent, Scope, Value};

use crate::page::PageDom;
use crate::target::page_of;
use crate::window::WindowHost;

/// Kept on the realm's `navigator` (a hidden property), itself kept per realm.
const KEY: &str = "lpAnimations";

macro_rules! lazy_globals {
    ($def:ident, $($name:literal),+ $(,)?) => {
        $($def.global_value_accessor(
            $name,
            |s, w| global(s, w.page(), $name),
            |s, w, value| replace(s, w.page(), $name, value),
        );)+
    };
}

/// Adds the lazy interface names to pyro-browser's `Window` class.
pub(crate) fn define_globals<W>(def: &mut ClassDef<W>)
where
    W: JsClass,
    W::Data: WindowHost,
{
    lazy_globals!(
        def,
        "Animation",
        "AnimationEffect",
        "KeyframeEffect",
        "AnimationTimeline",
        "DocumentTimeline",
        "AnimationPlaybackEvent",
        "CSSAnimation",
        "CSSTransition",
        "ScrollTimeline",
        "ViewTimeline",
    );
}

fn global<'s>(s: &Scope<'s>, page: &Rc<PageDom>, name: &str) -> JsResult<Value<'s>> {
    let value = module(s, page)?.get(name)?;
    replace(s, page, name, value)?;
    Ok(value)
}

/// Shadows the accessor with the interface object's usual data property.
fn replace(s: &Scope<'_>, page: &Rc<PageDom>, name: &str, value: Value<'_>) -> JsResult<()> {
    let define = page.define_properties.borrow().get(&s.realm_id()).map(|p| p.get(s));
    let define = define.and_then(|v| v.as_object()).ok_or_else(unavailable)?;
    let desc = s.object();
    desc.set_atom("value", value)?;
    desc.set_atom("writable", s.boolean(true))?;
    desc.set_atom("enumerable", s.boolean(false))?;
    desc.set_atom("configurable", s.boolean(true))?;
    define.call(None, &[s.global().value(), s.string(name), desc.value()])?;
    Ok(())
}

fn unavailable() -> JsError {
    JsError::Type("Web Animations are unavailable".into())
}

pub(crate) fn validate_event_animation<'s>(s: &Scope<'s>, value: Value<'s>, transition: bool) -> JsResult<()> {
    call(module(s, &page_of(s)?)?, "validateEventAnimation", &[value, s.boolean(transition)])?;
    Ok(())
}

pub(crate) fn view_runs<'s>(s: &Scope<'s>, page: &Rc<PageDom>, target: Value<'s>, runs: Value<'s>) -> JsResult<Value<'s>> {
    call(module(s, page)?, "viewRuns", &[target, runs])
}

/// This realm's module, made on first use.
fn module<'s>(s: &Scope<'s>, page: &Rc<PageDom>) -> JsResult<Object<'s>> {
    if let Some(module) = existing(s, page)? {
        return Ok(module);
    }
    let navigator = crate::window::navigator(s, page)?.as_object().ok_or_else(unavailable)?;
    let make = s.eval(include_str!("animations.js"), Some("pyro://internal/animations.js"), 1)?;
    let module = make.as_object().ok_or_else(unavailable)?.call(None, &[natives(s)?.value()])?;
    s.set_hidden(navigator, KEY, module)?;
    module.as_object().ok_or_else(unavailable)
}

/// The module only when this realm already made it: without it, no
/// animation exists.
fn existing<'s>(s: &Scope<'s>, page: &Rc<PageDom>) -> JsResult<Option<Object<'s>>> {
    let navigator = crate::window::navigator(s, page)?.as_object().ok_or_else(unavailable)?;
    Ok(s.get_hidden(navigator, KEY)?.and_then(|v| v.as_object()))
}

fn call<'s>(module: Object<'s>, name: &str, args: &[Value<'s>]) -> JsResult<Value<'s>> {
    module.get(name)?.as_object().ok_or_else(unavailable)?.call(None, args)
}

fn natives<'s>(s: &Scope<'s>) -> JsResult<Object<'s>> {
    let natives = s.object();
    // The document timeline's clock: what `performance.now()` read when the running task
    // first asked (a timeline's time holds still within a task).
    let now = s.function("now", |s, _this, _a| {
        let page = page_of(s)?;
        Ok(s.number(page.timeline_now_ms()))
    })?;
    natives.set_atom("now", now.value())?;
    // Capture the native endpoints once when constructing the transition's script object.
    // The object can outlive its run or lose its target.
    let animation_frames = s.function("animationFrames", |s, _, args| {
        let page = page_of(s)?;
        let Some(el) = crate::node::opt_node_arg(s, args.get(0), "animationFrames")? else { return Ok(s.null()) };
        let name = args.get(1).to_string()?;
        let dom = page.dom.borrow();
        let layout = page.layout();
        let Some(steps) = layout.synced_geometry_source(&dom).and_then(|g| g.css_animation_frames(pyro_dom::snapshot::render_key(el), &name)) else {
            return Ok(s.null());
        };
        let mut frames = Vec::with_capacity(steps.len());
        for (offset, easing, composite, values) in steps {
            let mut properties = Vec::with_capacity(values.len());
            for (name, value, underlying) in values {
                properties.push(s.array(&[s.string(&name), s.string(&value), s.string(&underlying)])?.value());
            }
            frames.push(s.array(&[s.number(offset), easing.as_deref().map_or_else(|| s.null(), |v| s.string(v)),
                composite.as_deref().map_or_else(|| s.null(), |v| s.string(v)), s.array(&properties)?.value()])?.value());
        }
        Ok(s.array(&frames)?.value())
    })?;
    natives.set_atom("animationFrames", animation_frames.value())?;
    let endpoints = s.function("transitionFrames", |s, _, args| {
        let page = page_of(s)?;
        let id = args.get(0).to_number()? as u64;
        let mut frames = Vec::new();
        if let Some(keyframes) = page.animation_events.transition_frames(id) {
            for frame in &keyframes.frames {
                let block = pyro_dom::stylo::declarations::DeclarationBlock::parse(&frame.declarations);
                if let Some(property) = block.item(0) {
                    frames.push(s.array(&[s.number(frame.offset), s.string(&property), s.string(&block.value(&property))])?.value());
                }
            }
        }
        Ok(s.array(&frames)?.value())
    })?;
    natives.set_atom("transitionFrames", endpoints.value())?;
    let observed = std::cell::Cell::new(None);
    let flush = s.function("flushPlayback", move |s, _this, _a| {
        let page = page_of(s)?;
        crate::animation_events::before_render(&page);
        let generation = page.animation_events.generation.get();
        if observed.replace(Some(generation)) != Some(generation) {
            with_css_runs(s, &page)?;
        }
        Ok(s.undefined())
    })?;
    natives.set_atom("flushPlayback", flush.value())?;
    // `fn` as a task of the page's event loop, `ms` from now.
    let schedule = s.function("schedule", |s, _this, a| {
        let ms = a.get(0).to_number()?;
        let callback = a.get(1);
        if callback.as_object().is_none() {
            return Err(JsError::Type("schedule: not a function".into()));
        }
        let page = page_of(s)?;
        let callback = Persistent::new(s, callback);
        let realm = s.realm().downgrade();
        let delay = crate::util::delay_from_ms(ms);
        let task: Box<dyn FnOnce()> = Box::new(move || {
            let Some(realm) = realm.upgrade() else { return };
            let Ok(s) = realm.scope() else { return };
            if page_of(&s).is_ok_and(|p| p.is_disposed()) {
                return;
            }
            let Some(f) = callback.get(&s).as_object() else { return };
            if let Err(error) = f.call(None, &[]) {
                s.report_exception(&error);
            }
        });
        if delay.is_zero() {
            page.post_task(task);
        } else {
            page.post_delayed_task(delay, task);
        }
        Ok(s.undefined())
    })?;
    natives.set_atom("schedule", schedule.value())?;
    // A keyframe member (`backgroundColor`) to the CSS property it names
    // (`background-color`); null when the engine knows no such property.
    let css_name = s.function("cssName", |s, _this, a| {
        let member = a.get(0).to_string()?;
        let name = crate::css::camel_to_css(&member);
        Ok(if pyro_dom::stylo::is_property(&name) { s.string(&name) } else { s.null() })
    })?;
    natives.set_atom("cssName", css_name.value())?;
    let valid_pseudo = s.function("validViewPseudo", |s, _, args| {
        let text = args.get(0).to_string()?;
        Ok(s.boolean(text.len() <= 4096 && matches!(pyro_dom::stylo::parse_pseudo_arg(&text), pyro_dom::stylo::PseudoArg::Pseudo(_))))
    })?;
    natives.set_atom("validViewPseudo", valid_pseudo.value())?;
    // `report(key, state)`: an Animation object's state after a change, for
    // the values sampled into the cascade (animation_frame.rs); `state` null
    // once it has no effect.
    let report = s.function("report", |s, _this, a| {
        // A page going away has nothing left to sample.
        let Ok(page) = page_of(s) else { return Ok(s.undefined()) };
        let key = a.get(0).to_number()? as u64;
        let state = match a.get(1).as_object() {
            Some(o) => Some(reported(s, o)?),
            None => None,
        };
        crate::animation_frame::report(&page, key, state);
        Ok(s.undefined())
    })?;
    natives.set_atom("report", report.value())?;
    // A page-wide key for `report` (realms of one page share the engine).
    let report_key = s.function("reportKey", |s, _this, _a| {
        let key = page_of(s).map_or(0, |page| page.animation_events.next_report_key());
        Ok(s.number(key as f64))
    })?;
    natives.set_atom("reportKey", report_key.value())?;
    // `timelineState(source, subject, axis, insetStartKind, insetStart, insetEndKind, insetEnd)`: a
    // script ScrollTimeline's (`subject` null) or ViewTimeline's `[offset, start, end, source]` now
    // (`source` the scroll container, null for the viewport), or null when inactive.
    let timeline_state = s.function("timelineState", |s, _this, a| {
        let page = page_of(s)?;
        let source = crate::node::opt_node_arg(s, a.get(0), "source")?;
        let subject = crate::node::opt_node_arg(s, a.get(1), "subject")?;
        let axis = a.get(2).to_number()? as u8;
        let inset = [
            (a.get(3).to_number()? as u8, a.get(4).to_number()?),
            (a.get(5).to_number()? as u8, a.get(6).to_number()?),
        ];
        // (2 `::before`, 3 `::after`: the subject's pseudo-element.)
        let pseudo = if a.len() > 7 { a.get(7).to_number()? as u8 } else { 0 };
        let state = {
            let dom = page.dom.borrow();
            let layout = page.layout();
            // (A frame's time without a frame: the scroll offsets of now.)
            layout.push_scroll_if_due(page.timeline_now_ms());
            let Some(geometry) = layout.synced_geometry_source(&dom) else { return Ok(s.null()) };
            let key = |id: pyro_dom::NodeId| pyro_dom::snapshot::render_key(id);
            let state = geometry.timeline_state(source.map(key), subject.map(key), pseudo, axis, inset);
            state.map(|(offset, start, end, src, contain, size)| {
                (offset, start, end, src.and_then(|k| pyro_dom::geometry::node_of(&dom, k)), contain, size)
            })
        };
        let Some((offset, start, end, src, contain, size)) = state else { return Ok(s.null()) };
        let src = crate::node::node_value(s, &page, src)?;
        Ok(s.array(&[
            s.number(offset),
            s.number(start),
            s.number(end),
            src,
            s.number(contain.0),
            s.number(contain.1),
            s.number(size),
        ])?
        .value())
    })?;
    natives.set_atom("timelineState", timeline_state.value())?;
    // `pushScroll()`: scroll timelines take the scroll position of now (after a frame passed).
    let push_scroll = s.function("pushScroll", |s, _this, _a| {
        let page = page_of(s)?;
        if let Some(layout) = page.try_layout() {
            if layout.push_scroll() {
                layout.set_scroll_pushed_at(page.timeline_now_ms());
            }
        }
        Ok(s.undefined())
    })?;
    natives.set_atom("pushScroll", push_scroll.value())?;
    Ok(natives)
}

/// `report`'s state: `[target, cssId (-1: none), keyframes, startTime,
/// holdTime, playbackRate, timeline origin, delay, endDelay, fill,
/// iterationStart, iterations, duration, direction, easing, composite, gone
/// (1: shows nothing)]`, keyframes as `[offset, easing, composite,
/// declarations]` each.
fn reported(s: &Scope<'_>, o: Object<'_>) -> JsResult<crate::animation_frame::ReportedState> {
    let at = |i: u32| o.get_index(i);
    let num = |i: u32| -> JsResult<f64> { at(i)?.to_number() };
    let opt = |i: u32| -> JsResult<Option<f64>> {
        let v = at(i)?;
        if v.is_nullish() {
            Ok(None)
        } else {
            v.to_number().map(Some)
        }
    };
    let text = |v: Value<'_>| -> JsResult<Option<String>> {
        if v.is_nullish() {
            Ok(None)
        } else {
            v.to_string().map(Some)
        }
    };
    let el = crate::node::opt_node_arg(s, at(0)?, "report")?;
    let css = num(1)?;
    let mut frames = Vec::new();
    if let Some(list) = at(2)?.as_object() {
        let len = list.get("length")?.to_number()? as u32;
        for i in 0..len {
            let Some(f) = list.get_index(i)?.as_object() else { continue };
            frames.push((
                f.get_index(0)?.to_number()?,
                text(f.get_index(1)?)?,
                text(f.get_index(2)?)?,
                f.get_index(3)?.to_string()?,
            ));
        }
    }
    let timing = crate::animation_frame::ReportedState::timing(
        num(7)?,
        num(8)?,
        &at(9)?.to_string()?,
        num(10)?,
        num(11)?,
        num(12)?,
        &at(13)?.to_string()?,
    );
    Ok(crate::animation_frame::ReportedState {
        el,
        css: (css >= 0.0).then_some(css as u64),
        keyframes: crate::animation_frame::keyframes(frames),
        start: opt(3)?,
        hold: opt(4)?,
        rate: num(5)?,
        origin: opt(6)?,
        timing,
        easing: text(at(14)?)?.map(|e| std::sync::Arc::from(e.as_str())),
        composite: pyro_dom::animation::Composite::parse(&at(15)?.to_string()?).unwrap_or_default(),
        gone: num(16)? != 0.0,
        pseudo: crate::animation_frame::Pseudo::from_code(num(17)?),
        scroll: match at(18)?.as_object() {
            None => None,
            Some(r) => {
                let n = |i: u32| -> JsResult<f64> { r.get_index(i)?.to_number() };
                Some(crate::animation_frame::ScrollRef {
                    source: crate::node::opt_node_arg(s, r.get_index(0)?, "report")?,
                    subject: crate::node::opt_node_arg(s, r.get_index(1)?, "report")?,
                    axis: n(2)? as u8,
                    inset: [(n(3)? as u8, n(4)?), (n(5)? as u8, n(6)?)],
                    inactive: n(7)? != 0.0,
                    pseudo: if r.get_index(8)?.is_undefined() { 0 } else { n(8)? as u8 },
                })
            }
        },
    })
}

/// `Element.animate(keyframes, options)`.
pub(crate) fn animate<'s>(s: &Scope<'s>, page: &Rc<PageDom>, el: Value<'s>, a: &Args<'s>) -> JsResult<Value<'s>> {
    let module = module(s, page)?;
    call(module, "animate", &[el, a.get(0), a.get(1)])
}

/// pyro: in a task, hands the page's module (when it has one) the CSS runs again: a run script
/// controls changed, and its object takes the CSS properties script did not override.
/// `create`: made when the realm has none (an animation with a trigger needs its object).
pub(crate) fn push_css_runs(page: &Rc<PageDom>, create: bool) {
    let weak = Rc::downgrade(page);
    page.post_task(Box::new(move || {
        let Some(page) = weak.upgrade().filter(|p| !p.is_disposed()) else { return };
        let Some(realm) = page.main_world_realm() else { return };
        let Ok(s) = realm.scope() else { return };
        if create || matches!(existing(&s, &page), Ok(Some(_))) {
            if let Err(error) = with_css_runs(&s, &page) {
                s.report_exception(&error);
            }
        }
    }));
}

/// Hands this realm's module the CSS animations and transitions the engine
/// keeps; `None` when there are none and the module was never made.
fn with_css_runs<'s>(s: &Scope<'s>, page: &Rc<PageDom>) -> JsResult<Option<Object<'s>>> {
    let runs = crate::animation_events::runs(page);
    let module = match existing(s, page)? {
        Some(module) => module,
        None if runs.is_empty() => return Ok(None),
        None => module(s, page)?,
    };
    let mut list = Vec::with_capacity(runs.len());
    for run in runs {
        let target = crate::node::wrap_node(s, page, run.el)?.value();
        list.push(
            s.array(&[
                s.number(run.id as f64),
                target,
                s.boolean(run.transition),
                s.string(&run.name),
                s.number(run.start_ms),
                s.number(run.delay_ms),
                s.number(run.duration_ms),
                s.number(run.iterations),
                s.string(&run.direction),
                s.string(&run.fill),
                s.string(&run.easing),
                run.paused_at.map_or_else(|| s.null(), |t| s.number(t)),
                match run.pseudo {
                    None => s.null(),
                    Some(pyro_dom::style::PseudoElement::Before) => s.string("::before"),
                    Some(pyro_dom::style::PseudoElement::After) => s.string("::after"),
                    Some(pyro_dom::style::PseudoElement::Marker) => s.string("::marker"),
                },
                match &run.timeline {
                    Some((text, start, end)) => s.array(&[s.string(text), s.string(start), s.string(end)])?.value(),
                    None => s.null(),
                },
                match &run.trigger {
                    Some((actions, source, decl)) => s
                        .array(&[s.string(actions), crate::node::wrap_node(s, page, *source)?.value(), s.string(decl)])?
                        .value(),
                    None => s.null(),
                },
                s.string(&run.keyframe_easing),
            ])?
            .value(),
        );
    }
    call(module, "cssSync", &[s.array(&list)?.value()])?;
    Ok(Some(module))
}

/// `Element.getAnimations(options)`.
pub(crate) fn element_animations<'s>(
    s: &Scope<'s>,
    page: &Rc<PageDom>,
    el: Value<'s>,
    a: &Args<'s>,
) -> JsResult<Value<'s>> {
    match with_css_runs(s, page)? {
        Some(module) => call(module, "elementAnimations", &[el, a.get(0)]),
        None => Ok(s.array(&[])?.value()),
    }
}

/// `Document.getAnimations()` and `ShadowRoot.getAnimations()`.
pub(crate) fn root_animations<'s>(s: &Scope<'s>, page: &Rc<PageDom>, root: Value<'s>) -> JsResult<Value<'s>> {
    match with_css_runs(s, page)? {
        Some(module) => call(module, "rootAnimations", &[root]),
        None => Ok(s.array(&[])?.value()),
    }
}

/// `document.timeline`: the realm's default document timeline.
pub(crate) fn timeline<'s>(s: &Scope<'s>, page: &Rc<PageDom>) -> JsResult<Value<'s>> {
    call(module(s, page)?, "timeline", &[])
}
