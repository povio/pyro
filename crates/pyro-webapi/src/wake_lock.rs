//! Screen Wake Lock (https://w3c.github.io/screen-wake-lock/): `navigator.wakeLock`, `WakeLock` and
//! `WakeLockSentinel` on a window, made in JS (wake_lock.js) on a realm's first use and secure-context only.
//! A headless browser has no screen to keep awake, so a lock is the spec's state machine: the document's
//! active locks, the `release` event, and their loss when the document is hidden or no longer fully active.
//! Nothing runs while no lock exists: the visibility listener is added with the first lock.

use std::rc::Rc;

use pyro_js::{ClassDef, JsClass, JsError, JsResult, Object, Scope, Value};

use crate::lazy_module::LazyModule;
use crate::page::PageDom;

static MODULE: LazyModule = LazyModule {
    key: "lpWakeLock",
    source: include_str!("wake_lock.js"),
    url: "pyro://internal/wake-lock.js",
    natives: Some(natives),
};

/// `WakeLock` and `WakeLockSentinel` on a window global (`[SecureContext]`: absent elsewhere).
pub(crate) fn define_globals<W: JsClass>(def: &mut ClassDef<W>)
where
    W::Data: crate::window::WindowHost,
{
    macro_rules! interface {
        ($($name:literal),+) => {$(
            def.global_value_accessor(
                $name,
                |s, w| {
                    let page = crate::window::WindowHost::page(w);
                    if !crate::secure_context::secure(s, page) {
                        return Ok(s.undefined());
                    }
                    MODULE.global(s, page, $name)
                },
                |s, w, value| crate::lazy_module::replace(s, crate::window::WindowHost::page(w), $name, value),
            );
        )+};
    }
    interface!("WakeLock", "WakeLockSentinel");
}

/// `navigator.wakeLock`: the realm's `WakeLock` in a secure context.
pub(crate) fn wake_lock<'s>(s: &Scope<'s>, page: &Rc<PageDom>) -> JsResult<Value<'s>> {
    if page.is_disposed() {
        return Err(JsError::ContextDestroyed);
    }
    if !crate::secure_context::secure(s, page) {
        return Ok(s.undefined());
    }
    MODULE.module(s, page)?.get("wakeLock")
}

fn natives<'s>(s: &Scope<'s>) -> JsResult<Object<'s>> {
    let native = s.object();
    native.set_atom("active", s.function("active", |s, _, _| Ok(s.boolean(crate::target::page_of(s)?.is_fully_active())))?.value())?;
    native.set_atom("hidden", s.function("hidden", |s, _, _| Ok(s.boolean(crate::target::page_of(s)?.is_hidden())))?.value())?;
    // The engine's gate (`check_feature` also queues the `permissions-policy-violation` report).
    native.set_atom(
        "allowed",
        s.function("allowed", |s, _, _| {
            let page = crate::target::page_of(s)?;
            Ok(s.boolean(crate::permissions_policy::check_feature(s, &page, "screen-wake-lock")))
        })?
        .value(),
    )?;
    native.set_atom(
        "permission",
        s.function("permission", |s, _, _| {
            let page = crate::target::page_of(s)?;
            Ok(s.string(crate::navigator_extras::permission_state(&page, "screen-wake-lock").as_str()))
        })?
        .value(),
    )?;
    native.set_atom(
        "dispatchTrusted",
        s.function("dispatchTrusted", |s, _, a| {
            crate::cookie_store::events::dispatch_trusted(s, a.get(0), a.get(1))?;
            Ok(s.undefined())
        })?
        .value(),
    )?;
    Ok(native)
}
