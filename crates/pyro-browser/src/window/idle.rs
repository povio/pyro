//! `requestIdleCallback` / `cancelIdleCallback` (w3c/requestidlecallback),
//! Window only. A callback runs once the event loop has nothing else ready
//! (an idle timer, which never delays other work), or when its `timeout`
//! passes first. `IdleDeadline` is made in JS (idle.js) on a window's first
//! idle callback or first read of the interface: a page that never asks
//! for one pays nothing beyond the members.

use super::{Window, WindowData};
use pyro_js::{Args, JsError, JsResult, Object, Persistent, Scope, Value, WeakRealm};
use pyro_runtime::TimerId;
use std::time::{Duration, Instant};

/// The longest idle period (the spec's 50 ms cap on `timeRemaining()`).
const MAX_IDLE_MS: f64 = 50.0;

pub(super) struct IdleCallback {
    callback: Persistent,
    /// The idle timer that runs it and, with a `timeout`, the timer that
    /// runs it instead once that passed. Whichever fires first clears the
    /// other.
    idle: TimerId,
    timeout: Option<TimerId>,
}

/// WebIDL `unsigned long` (no [EnforceRange]): modulo 2^32.
fn unsigned_long(n: f64) -> u32 {
    if !n.is_finite() {
        return 0;
    }
    n.trunc().rem_euclid(4_294_967_296.0) as u32
}

pub(super) fn request<'s>(s: &Scope<'s>, w: &WindowData, a: Args<'s>) -> JsResult<Value<'s>> {
    let callback = a.get(0).as_object().filter(|f| f.is_function()).ok_or_else(|| {
        JsError::Type(
            "Failed to execute 'requestIdleCallback' on 'Window': parameter 1 is not of type 'IdleRequestCallback'."
                .into(),
        )
    })?;
    // IdleRequestOptions { unsigned long timeout = 0; }
    let options = a.get(1);
    let timeout = if options.is_nullish() {
        0
    } else if let Some(options) = options.as_object() {
        let timeout = options.get("timeout")?;
        if timeout.is_undefined() {
            0
        } else {
            unsigned_long(timeout.to_number()?)
        }
    } else {
        return Err(JsError::Type(
            "Failed to execute 'requestIdleCallback' on 'Window': The provided value is not of type 'IdleRequestOptions'."
                .into(),
        ));
    };
    if w.target.page.scripting_disabled() {
        return Ok(s.number(0.0));
    }
    let handle = w.next_handle.get();
    w.next_handle.set(handle.wrapping_add(1).max(1));
    // Timers must not keep a disposed realm alive: hold it weakly.
    let realm = s.realm().downgrade();
    // Requested during an idle period (from an idle callback or its
    // microtasks): it waits for the next period, which starts when this one
    // ends, as in Chrome. A self-rescheduling idle loop then runs about
    // every 50 ms instead of spinning the event loop.
    let wait = w.idle_period_end.get().map(|end| end.saturating_duration_since(Instant::now()));
    let idle = match wait.filter(|d| !d.is_zero()) {
        Some(wait) => {
            let on_period = realm.clone();
            w.sched.set_timer(wait, false, true, move |_browser| arm(&on_period, handle))
        }
        None => arm_idle(w, realm.clone(), handle),
    };
    let timeout = (timeout > 0).then(|| {
        w.sched.set_timer(Duration::from_millis(u64::from(timeout)), false, true, move |_browser| {
            run(&realm, handle, true)
        })
    });
    let callback = Persistent::new(s, callback.value());
    w.idle_callbacks.borrow_mut().insert(handle, IdleCallback { callback, idle, timeout });
    Ok(s.number(f64::from(handle)))
}

pub(super) fn cancel<'s>(s: &Scope<'s>, w: &WindowData, a: Args<'s>) -> JsResult<Value<'s>> {
    let handle = unsigned_long(a.get(0).to_number()?);
    if let Some(entry) = w.idle_callbacks.borrow_mut().remove(&handle) {
        clear(w, &entry);
    }
    Ok(s.undefined())
}

/// Cancels every pending idle callback (navigation/teardown).
pub(super) fn cancel_all(w: &WindowData) {
    for (_, entry) in w.idle_callbacks.borrow_mut().drain() {
        clear(w, &entry);
    }
}

fn clear(w: &WindowData, entry: &IdleCallback) {
    w.sched.clear_timer(entry.idle);
    if let Some(id) = entry.timeout {
        w.sched.clear_timer(id);
    }
}

fn arm_idle(w: &WindowData, realm: WeakRealm, handle: u32) -> TimerId {
    w.sched.set_idle_timer(true, move |_browser| run(&realm, handle, false))
}

/// The idle period a callback waited for began: it now waits for the loop
/// to be idle.
fn arm(realm: &WeakRealm, handle: u32) {
    let Some(strong) = realm.upgrade() else { return };
    let Ok(scope) = strong.scope() else { return };
    let Some(w) = scope.global_data::<Window>() else { return };
    let idle = arm_idle(w, realm.clone(), handle);
    let mut callbacks = w.idle_callbacks.borrow_mut();
    match callbacks.get_mut(&handle) {
        Some(entry) => entry.idle = idle,
        None => {
            w.sched.clear_timer(idle);
        }
    }
}

fn run(realm: &WeakRealm, handle: u32, did_timeout: bool) {
    let Some(realm) = realm.upgrade() else { return };
    let Ok(scope) = realm.scope() else { return };
    let Some(w) = scope.global_data::<Window>() else { return };
    let Some(entry) = w.idle_callbacks.borrow_mut().remove(&handle) else { return };
    // The timer that fired is spent; the other one goes.
    if did_timeout {
        w.sched.clear_timer(entry.idle);
    } else if let Some(id) = entry.timeout {
        w.sched.clear_timer(id);
    }
    if w.target.page.scripting_disabled() {
        return;
    }
    let now = w.now_ms();
    // A timed-out callback has no idle time; an idle one has until the
    // next timer is due, at most 50 ms.
    let length = if did_timeout {
        0.0
    } else {
        let until_timer = w
            .sched
            .next_timer_deadline()
            .map_or(MAX_IDLE_MS, |at| at.saturating_duration_since(Instant::now()).as_secs_f64() * 1000.0);
        until_timer.min(MAX_IDLE_MS)
    };
    // Idle callbacks requested from here on wait for the next idle period.
    let outer = w
        .idle_period_end
        .replace((!did_timeout).then(|| Instant::now() + Duration::from_secs_f64(length.max(0.0) / 1000.0)));
    let result = (|| {
        let make = module(&scope, w)?.get("make")?;
        let make = make.as_object().ok_or(JsError::ContextDestroyed)?;
        let deadline = make.call(None, &[scope.number(now), scope.number(length), scope.boolean(did_timeout)])?;
        let callback = entry.callback.get(&scope).as_object().ok_or(JsError::ContextDestroyed)?;
        // scheduler.yield() from an idle callback continues at background
        // priority (the callback's call ends with its microtask checkpoint).
        pyro_webapi::with_idle_scheduling_state(&w.target.page, || callback.call(None, &[deadline]))
    })();
    w.idle_period_end.set(outer);
    if let Err(e) = result {
        scope.report_exception(&e);
    }
}

/// This window's idle.js module, made on first use.
fn module<'s>(s: &Scope<'s>, w: &WindowData) -> JsResult<Object<'s>> {
    if let Some(module) = w.idle_module.borrow().as_ref().and_then(|m| m.get(s).as_object()) {
        return Ok(module);
    }
    let make = s.eval(include_str!("idle.js"), Some("pyro://internal/idle.js"), 1)?;
    let now = s.function("now", |s, _this, _a| Ok(s.number(s.global_data::<Window>().map_or(0.0, |w| w.now_ms()))))?;
    let module = make.as_object().ok_or(JsError::ContextDestroyed)?.call(None, &[now.value()])?;
    let module = module.as_object().ok_or(JsError::ContextDestroyed)?;
    *w.idle_module.borrow_mut() = Some(Persistent::new(s, module.value()));
    Ok(module)
}

/// `window.IdleDeadline`, made on its first read.
pub(super) fn interface<'s>(s: &Scope<'s>, w: &WindowData) -> JsResult<Value<'s>> {
    let value = module(s, w)?.get("IdleDeadline")?;
    pyro_webapi::window::define_global_interface(s, &w.target.page, "IdleDeadline", value)?;
    Ok(value)
}

pub(super) fn replace_interface<'s>(s: &Scope<'s>, w: &WindowData, value: Value<'s>) -> JsResult<()> {
    pyro_webapi::window::define_global_interface(s, &w.target.page, "IdleDeadline", value)
}
