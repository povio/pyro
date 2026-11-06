//! Web Animations: `Element.animate()`, `Animation`, `KeyframeEffect`,
//! `document.timeline` and `getAnimations()`, made in JS (animations.js) on
//! a realm's first use of any of them, as streams.js is. Realms that never
//! animate pay one global accessor pair per interface name, and
//! `getAnimations()` answers `[]` without making the module unless CSS
//! animations or transitions run (`CSSAnimation`/`CSSTransition`, kept by
//! animation_events.rs from the first `getAnimations()` on).
//!
//! The script gets the page clock (`now`, which follows CDP virtual time),
//! one-shot timers on the page's event loop (`schedule`) and Stylo's
//! property names (`cssName`). Nothing runs per frame: a playing animation
//! has one timer, at its end.
use std::rc::Rc;
use std::time::Duration;

use lp_js::{Args, ClassDef, JsClass, JsError, JsResult, Object, Persistent, Scope, Value};

use crate::page::PageDom;
use crate::target::page_of;
use crate::window::WindowHost;

/// Kept on the realm's `navigator` (a hidden property), itself kept per realm.
const KEY: &str = "lpAnimations";

macro_rules! lazy_globals {
    ($def:ident, $($name:literal),+ $(,)?) => {
        $($def.accessor(
            $name,
            |s, w| global(s, w.page(), $name),
            |s, w, value| replace(s, w.page(), $name, value),
        );)+
    };
}

/// Adds the lazy interface names to lp-browser's `Window` class.
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

/// This realm's module, made on first use.
fn module<'s>(s: &Scope<'s>, page: &Rc<PageDom>) -> JsResult<Object<'s>> {
    if let Some(module) = existing(s, page)? {
        return Ok(module);
    }
    let navigator = crate::window::navigator(s, page)?.as_object().ok_or_else(unavailable)?;
    let make = s.eval(include_str!("animations.js"), Some("lightningpanda://internal/animations.js"), 1)?;
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
    // The document's clock: what `performance.now()` reads.
    let now = s.function("now", |s, _this, _a| {
        let page = page_of(s)?;
        Ok(s.number(crate::event::page_now_ms(&page)))
    })?;
    natives.set_atom("now", now.value())?;
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
        let delay = if ms.is_finite() && ms > 0.0 { Duration::from_secs_f64(ms / 1000.0) } else { Duration::ZERO };
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
        Ok(if lp_dom::stylo::is_property(&name) { s.string(&name) } else { s.null() })
    })?;
    natives.set_atom("cssName", css_name.value())?;
    Ok(natives)
}

/// `Element.animate(keyframes, options)`.
pub(crate) fn animate<'s>(s: &Scope<'s>, page: &Rc<PageDom>, el: Value<'s>, a: &Args<'s>) -> JsResult<Value<'s>> {
    let module = module(s, page)?;
    call(module, "animate", &[el, a.get(0), a.get(1)])
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
