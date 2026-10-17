//! The `Window` global: timers and scheduling primitives.
//!
//! DOM-facing members and EventTarget inheritance come from lp-webapi;
//! location/history and timer scheduling stay in lp-browser.

use crate::nav::{History, Location, PageNav, Performance, PerformanceData};
use crate::Browser;
use lp_js::{Args, ClassDef, IdentityKey, JsClass, JsError, JsResult, Persistent, Realm, Scope, Value};
use lp_runtime::{Scheduler, TimerId};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

pub struct WindowData {
    sched: Scheduler<Browser>,
    nav: Rc<PageNav>,
    target: lp_webapi::EventTargetData,
    /// JS timer handle -> loop timer.
    timers: RefCell<HashMap<u32, TimerId>>,
    next_handle: Cell<u32>,
}

impl WindowData {
    /// The page's session history shared with `window.history`.
    pub(crate) fn nav(&self) -> &Rc<PageNav> {
        &self.nav
    }

    pub fn new(sched: Scheduler<Browser>, nav: Rc<PageNav>, page: &Rc<lp_webapi::PageDom>) -> WindowData {
        let key = page.window_target().expect("document window target");
        WindowData {
            sched,
            nav,
            target: lp_webapi::EventTargetData::window(page, key),
            timers: RefCell::new(HashMap::new()),
            next_handle: Cell::new(1),
        }
    }

    pub fn url(&self) -> String {
        self.nav.url()
    }

    /// Milliseconds since the document's time origin (`performance.now()`).
    pub fn now_ms(&self) -> f64 {
        self.nav.now_ms()
    }
}

/// Identity keys of the per-window singletons (one wrapper per realm).
const KEY_LOCATION: u64 = 0x4c6f_6361_7469_6f6e;
const KEY_HISTORY: u64 = 0x4869_7374_6f72_7900;
const KEY_PERFORMANCE: u64 = 0x5065_7266_6f72_6d00;

impl lp_webapi::WindowHost for WindowData {
    fn page(&self) -> &Rc<lp_webapi::PageDom> {
        &self.target.page
    }
    fn event_target(&self) -> &lp_webapi::EventTargetData {
        &self.target
    }
}

pub struct Window;

impl JsClass for Window {
    const NAME: &'static str = "Window";
    type Data = WindowData;
    type Parent = lp_webapi::EventTarget;

    fn define(def: &mut ClassDef<Self>) {
        def.upcast(|w| &w.target);
        lp_webapi::define_members::<Window>(def);
        def.method("setTimeout", 1, |s, w, a| set_timer(s, w, a, false));
        def.method("setInterval", 1, |s, w, a| set_timer(s, w, a, true));
        def.method("clearTimeout", 0, clear_timer);
        def.method("clearInterval", 0, clear_timer);
        def.method("requestAnimationFrame", 1, |s, w, a| {
            // No rendering: frames tick at 60 Hz with the high-resolution time.
            let cb = a.get(0).as_object().filter(|f| f.is_function()).ok_or_else(|| {
                JsError::Type(
                    "Failed to execute 'requestAnimationFrame' on 'Window': parameter 1 is not of type 'Function'."
                        .into(),
                )
            })?;
            schedule(s, w, Persistent::new(s, cb.value()), Vec::new(), 16, false, true)
        });
        def.method("cancelAnimationFrame", 1, clear_timer);
        def.method("queueMicrotask", 1, |s, _w, a| {
            let cb = a.get(0).as_object().filter(|f| f.is_function()).ok_or_else(|| {
                JsError::Type(
                    "Failed to execute 'queueMicrotask' on 'Window': parameter 1 is not of type 'Function'.".into(),
                )
            })?;
            let cb = Persistent::new(s, cb.value());
            s.then(
                s.undefined(),
                move |s, _| {
                    if let Some(f) = cb.get(s).as_object() {
                        if let Err(e) = f.call(None, &[]) {
                            s.report_exception(&e);
                        }
                    }
                },
                |_, _| {},
            )?;
            Ok(s.undefined())
        });
        def.accessor(
            "location",
            |s, w| Ok(s.wrap::<Location>(w.nav.clone(), Some(IdentityKey(KEY_LOCATION, 0)))?.value()),
            |s, w, v| {
                // `window.location = url` assigns href.
                let loc = s.wrap::<Location>(w.nav.clone(), Some(IdentityKey(KEY_LOCATION, 0)))?;
                loc.set("href", v)
            },
        );
        def.getter("history", |s, w| Ok(s.wrap::<History>(w.nav.clone(), Some(IdentityKey(KEY_HISTORY, 0)))?.value()));
        def.getter("performance", |s, w| {
            if let Some(o) = s.lookup_identity(IdentityKey(KEY_PERFORMANCE, 0)) {
                return Ok(o.value());
            }
            Ok(s.wrap::<Performance>(PerformanceData::new(w.nav.clone()), Some(IdentityKey(KEY_PERFORMANCE, 0)))?
                .value())
        });
        def.getter("top", |s, _w| Ok(s.global().value()));
        def.getter("parent", |s, _w| Ok(s.global().value()));
        def.getter("window", |s, _w| Ok(s.global().value()));
        def.getter("self", |s, _w| Ok(s.global().value()));
        def.getter("frames", |s, _w| Ok(s.global().value()));
        def.getter("isSecureContext", |s, w| {
            let u = w.url();
            Ok(s.boolean(
                u.starts_with("https:")
                    || u.starts_with("http://localhost")
                    || u.starts_with("http://127.0.0.1")
                    || u == "about:blank",
            ))
        });
        def.method("__lpNow", 0, |s, w, _a| Ok(s.number(w.now_ms())));
    }
}

fn set_timer<'s>(s: &Scope<'s>, w: &WindowData, a: Args<'s>, repeat: bool) -> JsResult<Value<'s>> {
    let handler = a.get(0);
    // WebIDL `long timeout = 0`: conversion errors propagate.
    let delay = a.get(1).to_number()?;
    let delay = if delay.is_finite() && delay > 0.0 { delay.min(i32::MAX as f64) as u64 } else { 0 };
    let cb = if handler.is_function() {
        Persistent::new(s, handler)
    } else {
        // String handler: compiled as a function body in the global scope.
        let src = handler.to_string()?;
        let f = s.eval(&format!("(function () {{\n{src}\n}})"), Some(&w.url()), 1)?;
        Persistent::new(s, f)
    };
    let args: Vec<Persistent> = a.iter().skip(2).map(|v| Persistent::new(s, v)).collect();
    schedule(s, w, cb, args, delay, repeat, false)
}

fn schedule<'s>(
    s: &Scope<'s>,
    w: &WindowData,
    cb: Persistent,
    args: Vec<Persistent>,
    delay_ms: u64,
    repeat: bool,
    pass_time: bool,
) -> JsResult<Value<'s>> {
    let handle = w.next_handle.get();
    w.next_handle.set(handle.wrapping_add(1).max(1));
    // Timers must not keep a disposed realm alive: hold it weakly.
    let realm_weak = s.realm().downgrade();
    let id = w.sched.set_timer(Duration::from_millis(delay_ms), repeat, true, move |_browser| {
        let Some(realm) = realm_weak.upgrade() else { return };
        let Ok(scope) = realm.scope() else { return };
        if !repeat {
            if let Some(w) = scope.global_data::<Window>() {
                w.timers.borrow_mut().remove(&handle);
            }
        }
        let Some(f) = cb.get(&scope).as_object() else { return };
        let result = if pass_time {
            let now = scope.global_data::<Window>().map(|w| w.now_ms()).unwrap_or(0.0);
            f.call(None, &[scope.number(now)])
        } else {
            // Arguments are rooted by `args` (Persistent) while this call runs.
            let vals: Vec<Value<'_>> = args.iter().map(|p| p.get(&scope)).collect();
            f.call(None, &vals)
        };
        if let Err(e) = result {
            scope.report_exception(&e);
        }
    });
    w.timers.borrow_mut().insert(handle, id);
    Ok(s.number(handle as f64))
}

fn clear_timer<'s>(s: &Scope<'s>, w: &WindowData, a: Args<'s>) -> JsResult<Value<'s>> {
    let n = a.get(0).to_number()?;
    if n.is_finite() && n >= 1.0 && n <= u32::MAX as f64 {
        if let Some(id) = w.timers.borrow_mut().remove(&(n as u32)) {
            w.sched.clear_timer(id);
        }
    }
    Ok(s.undefined())
}

/// Cancels every pending timer of a realm's window (navigation/teardown).
pub(crate) fn cancel_timers(realm: &Realm) {
    let Ok(scope) = realm.scope() else { return };
    if let Some(w) = scope.global_data::<Window>() {
        for (_, id) in w.timers.borrow_mut().drain() {
            w.sched.clear_timer(id);
        }
    }
}
