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
use std::time::{Duration, Instant};

/// Frame interval without rendering. Upstream Lightpanda schedules each
/// animation frame callback 5 ms out (`Window.requestAnimationFrame`).
const FRAME_MS: u64 = 5;
/// Frames run as soon as the thread has nothing else ready (an idle timer)
/// this many times after each client command. Nothing renders, so a frame
/// only paces script: clients that wait for frames (e.g. Bun.WebView's
/// "stable for two frames" click check) get them as soon as the page's work
/// is done instead of on a clock, while a page with a perpetual
/// `requestAnimationFrame` loop falls back to the 5 ms cadence after a few
/// frames instead of spinning.
const EAGER_FRAMES: u32 = 4;

thread_local! {
    /// Eager frames this browser thread may still run.
    static EAGER_FRAMES_LEFT: Cell<u32> = const { Cell::new(0) };
}

/// Notes that an automation client just sent a command (or an `LP.run` step
/// acted) on this thread's browser: the next few animation frames run as
/// soon as the page is otherwise idle.
pub fn note_client_activity() {
    EAGER_FRAMES_LEFT.with(|c| c.set(EAGER_FRAMES));
}

/// Takes one eager frame, if any is left.
fn take_eager_frame() -> bool {
    EAGER_FRAMES_LEFT.with(|c| {
        let left = c.get();
        c.set(left.saturating_sub(1));
        left > 0
    })
}

/// One-shot timers due within this window count as pending page work
/// ([`WindowData::pending_work`]); later ones and intervals are a page's
/// background schedule (polling, debounces, idle timeouts), not the
/// follow-up of what just happened.
pub const TIMER_HORIZON: Duration = Duration::from_millis(50);
/// A callback requested from a frame callback that was itself requested
/// from one (and so on) this many levels deep belongs to an animation loop,
/// not pending work; a single frame or the double-frame "after paint" idiom
/// still counts.
const LOOP_DEPTH: u32 = 2;

#[derive(Default)]
struct AnimationFrame {
    /// Handle, callback, and how many frame callbacks it was requested
    /// from in a chain (0: from outside a frame).
    callbacks: Vec<(u32, Persistent, u32)>,
    native_callbacks: Vec<Rc<Cell<bool>>>,
    timer: Option<TimerId>,
    /// Chain depth of the frame callback running now, if any.
    running: Option<u32>,
}

pub struct WindowData {
    sched: Scheduler<Browser>,
    nav: Rc<PageNav>,
    target: lp_webapi::EventTargetData,
    /// JS timer handle -> loop timer and, for `setTimeout`, when it is due.
    timers: RefCell<HashMap<u32, (TimerId, Option<Instant>)>>,
    /// Pending `requestAnimationFrame` callbacks, in registration order, and
    /// the loop timer of the frame that will run them.
    animation: RefCell<AnimationFrame>,
    next_handle: Cell<u32>,
    frames: std::rc::Weak<crate::frames::FrameHost>,
    frame: Option<crate::frames::FrameKey>,
    frame_definer: RefCell<Option<Persistent>>,
    frame_index_count: Cell<usize>,
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
            animation: RefCell::new(AnimationFrame::default()),
            next_handle: Cell::new(1),
            frames: std::rc::Weak::new(),
            frame: None,
            frame_definer: RefCell::new(None),
            frame_index_count: Cell::new(0),
        }
    }

    pub(crate) fn for_frame(
        sched: Scheduler<Browser>,
        nav: Rc<PageNav>,
        page: &Rc<lp_webapi::PageDom>,
        frames: std::rc::Weak<crate::frames::FrameHost>,
        frame: crate::frames::FrameKey,
    ) -> Self {
        let mut data = Self::new(sched, nav, page);
        data.frames = frames;
        data.frame = Some(frame);
        data
    }
    pub fn url(&self) -> String {
        self.nav.url()
    }

    /// Milliseconds since the document's time origin (`performance.now()`).
    pub fn now_ms(&self) -> f64 {
        self.nav.now_ms()
    }

    /// Script work this window has scheduled that can still change the page
    /// soon: `setTimeout`s due by `now + TIMER_HORIZON`, and animation frame
    /// callbacks unless they belong to an animation loop.
    pub fn pending_work(&self, now: Instant) -> (usize, usize) {
        let horizon = now + TIMER_HORIZON;
        let timers = self.timers.borrow().values().filter(|(_, due)| due.is_some_and(|d| d <= horizon)).count();
        let animation = self.animation.borrow();
        let frames = animation.callbacks.iter().filter(|(_, _, depth)| *depth < LOOP_DEPTH).count()
            + animation.native_callbacks.len();
        (timers, frames)
    }
}

/// Identity keys of the per-window singletons (one wrapper per realm).
const KEY_LOCATION: u64 = 0x4c6f_6361_7469_6f6e;
pub(crate) const KEY_HISTORY: u64 = 0x4869_7374_6f72_7900;
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
            let cb = a.get(0).as_object().filter(|f| f.is_function()).ok_or_else(|| {
                JsError::Type(
                    "Failed to execute 'requestAnimationFrame' on 'Window': parameter 1 is not of type 'Function'."
                        .into(),
                )
            })?;
            request_animation_frame(s, w, Persistent::new(s, cb.value()))
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
        def.getter("top", |s, w| match (w.frames.upgrade(), w.frame) {
            (Some(host), Some(key)) => host.relation(s, key, true),
            _ => Ok(s.global().value()),
        });
        def.accessor(
            "parent",
            |s, w| match (w.frames.upgrade(), w.frame) {
                (Some(host), Some(key)) => host.relation(s, key, false),
                _ => Ok(s.global().value()),
            },
            |s, w, v| lp_webapi::window::replace_window_attribute(s, &w.target.page, "parent", v),
        );
        def.getter("frameElement", |s, w| match (w.frames.upgrade(), w.frame) {
            (Some(host), Some(key)) => host.frame_element(s, key),
            _ => Ok(s.null()),
        });
        // `opener` is writable: assigning (e.g. null to disown) shadows it.
        def.accessor(
            "opener",
            |s, w| match (w.frames.upgrade(), w.frame) {
                (Some(host), Some(key)) => host.opener_window(s, key),
                _ => Ok(s.null()),
            },
            |s, w, v| lp_webapi::window::replace_window_attribute(s, &w.target.page, "opener", v),
        );
        def.accessor(
            "length",
            |s, w| {
                Ok(s.number(match (w.frames.upgrade(), w.frame) {
                    (Some(host), Some(key)) => host.child_keys(key).len() as f64,
                    _ => 0.0,
                }))
            },
            |s, w, v| lp_webapi::window::replace_window_attribute(s, &w.target.page, "length", v),
        );
        def.getter("origin", |s, _| Ok(s.string(s.realm().origin().as_deref().unwrap_or("null"))));
        def.indexed(|s, w, index| match (w.frames.upgrade(), w.frame) {
            (Some(host), Some(key)) => {
                host.child_keys(key).get(index as usize).map(|target| host.window_value(s, key, *target)).transpose()
            }
            _ => Ok(None),
        });
        def.getter("window", |s, _w| Ok(s.global().value()));
        def.accessor(
            "self",
            |s, _w| Ok(s.global().value()),
            |s, w, v| lp_webapi::window::replace_window_attribute(s, &w.target.page, "self", v),
        );
        def.accessor(
            "frames",
            |s, _w| Ok(s.global().value()),
            |s, w, v| lp_webapi::window::replace_window_attribute(s, &w.target.page, "frames", v),
        );
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
    if w.target.page.scripting_disabled() {
        return Ok(s.number(0.0));
    }
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
    schedule(s, w, cb, args, delay, repeat)
}

fn schedule<'s>(
    s: &Scope<'s>,
    w: &WindowData,
    cb: Persistent,
    args: Vec<Persistent>,
    delay_ms: u64,
    repeat: bool,
) -> JsResult<Value<'s>> {
    if w.target.page.scripting_disabled() {
        return Ok(s.number(0.0));
    }
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
        // Arguments are rooted by `args` (Persistent) while this call runs.
        let vals: Vec<Value<'_>> = args.iter().map(|p| p.get(&scope)).collect();
        if let Err(e) = f.call(None, &vals) {
            scope.report_exception(&e);
        }
    });
    let due = (!repeat).then(|| crate::clock::now() + Duration::from_millis(delay_ms));
    w.timers.borrow_mut().insert(handle, (id, due));
    Ok(s.number(handle as f64))
}

/// Queues `cb` for the next animation frame. Every callback queued before a
/// frame runs in it, in order, with the same timestamp; callbacks queued while
/// a frame runs wait for the next one.
fn request_animation_frame<'s>(s: &Scope<'s>, w: &WindowData, cb: Persistent) -> JsResult<Value<'s>> {
    if w.target.page.scripting_disabled() {
        return Ok(s.number(0.0));
    }
    let handle = w.next_handle.get();
    w.next_handle.set(handle.wrapping_add(1).max(1));
    let mut frame = w.animation.borrow_mut();
    let depth = frame.running.map_or(0, |d| d.saturating_add(1));
    frame.callbacks.push((handle, cb, depth));
    schedule_animation_frame(s, w, &mut frame);
    Ok(s.number(handle as f64))
}

fn schedule_animation_frame(s: &Scope<'_>, w: &WindowData, frame: &mut AnimationFrame) {
    if frame.timer.is_none() {
        // Timers must not keep a disposed realm alive: hold it weakly.
        let realm_weak = s.realm().downgrade();
        let run = move |_browser: &mut Browser| {
            let Some(realm) = realm_weak.upgrade() else { return };
            let Ok(scope) = realm.scope() else { return };
            run_animation_frame(&scope);
        };
        frame.timer = Some(if take_eager_frame() {
            w.sched.set_idle_timer(true, run)
        } else {
            w.sched.set_timer(Duration::from_millis(FRAME_MS), false, true, run)
        });
    }
}

/// A native actionability observer of the next real rendering update.
pub fn request_native_frame(realm: &Realm) -> JsResult<Rc<Cell<bool>>> {
    let scope = realm.scope()?;
    let window = scope.global_data::<Window>().ok_or_else(|| JsError::Type("page has no Window".into()))?;
    let ready = Rc::new(Cell::new(false));
    let mut frame = window.animation.borrow_mut();
    frame.native_callbacks.push(ready.clone());
    schedule_animation_frame(&scope, window, &mut frame);
    Ok(ready)
}

fn run_animation_frame(scope: &Scope<'_>) {
    let Some(w) = scope.global_data::<Window>() else { return };
    lp_webapi::layout_api::flush_pending_scrolls(&w.target.page);
    let (handles, native): (Vec<u32>, _) = {
        let mut frame = w.animation.borrow_mut();
        frame.timer = None;
        (frame.callbacks.iter().map(|(h, _, _)| *h).collect(), std::mem::take(&mut frame.native_callbacks))
    };
    let now = scope.number(w.now_ms());
    for handle in handles {
        // A callback may cancel a later one of the same frame.
        let cb = {
            let mut frame = w.animation.borrow_mut();
            let Some(i) = frame.callbacks.iter().position(|(h, _, _)| *h == handle) else { continue };
            let (_, cb, depth) = frame.callbacks.remove(i);
            frame.running = Some(depth);
            cb
        };
        let Some(f) = cb.get(scope).as_object() else { continue };
        if let Err(e) = f.call(None, &[now]) {
            scope.report_exception(&e);
        }
    }
    w.animation.borrow_mut().running = None;
    for ready in native {
        ready.set(true);
    }
}

fn clear_timer<'s>(s: &Scope<'s>, w: &WindowData, a: Args<'s>) -> JsResult<Value<'s>> {
    let n = a.get(0).to_number()?;
    if n.is_finite() && n >= 1.0 && n <= u32::MAX as f64 {
        let handle = n as u32;
        if let Some((id, _)) = w.timers.borrow_mut().remove(&handle) {
            w.sched.clear_timer(id);
        } else {
            // Timer and animation frame handles share one sequence, so either
            // clear function cancels either kind (as before).
            let mut frame = w.animation.borrow_mut();
            if let Some(i) = frame.callbacks.iter().position(|(h, _, _)| *h == handle) {
                frame.callbacks.remove(i);
                if frame.callbacks.is_empty() && frame.native_callbacks.is_empty() {
                    if let Some(id) = frame.timer.take() {
                        w.sched.clear_timer(id);
                    }
                }
            }
        }
    }
    Ok(s.undefined())
}

/// Cancels every pending timer of a realm's window (navigation/teardown).
pub(crate) fn cancel_timers(realm: &Realm) {
    let Ok(scope) = realm.scope() else { return };
    if let Some(w) = scope.global_data::<Window>() {
        for (_, (id, _)) in w.timers.borrow_mut().drain() {
            w.sched.clear_timer(id);
        }
        let mut frame = w.animation.borrow_mut();
        frame.callbacks.clear();
        for ready in frame.native_callbacks.drain(..) {
            ready.set(true);
        }
        if let Some(id) = frame.timer.take() {
            w.sched.clear_timer(id);
        }
    }
}

/// Capture the descriptor intrinsic before page init scripts can replace it.
pub(crate) fn init_frame_indices(realm: &Realm) -> JsResult<()> {
    let scope = realm.scope()?;
    let define=scope.eval("(()=>{const define=Object.defineProperty;return(g,k,get)=>define(g,k,{get,enumerable:true,configurable:true})})()",Some("lightpanda://frame-indices"),1)?;
    let w = scope.global_data::<Window>().ok_or(JsError::ContextDestroyed)?;
    *w.frame_definer.borrow_mut() = Some(Persistent::new(&scope, define));
    Ok(())
}
pub(crate) fn refresh_frame_indices(
    realm: &Realm,
    host: &Rc<crate::frames::FrameHost>,
    key: crate::frames::FrameKey,
) -> JsResult<()> {
    let scope = realm.scope()?;
    let w = scope.global_data::<Window>().ok_or(JsError::ContextDestroyed)?;
    let Some(define) = w.frame_definer.borrow().clone() else { return Ok(()) };
    let count = host.child_keys(key).len();
    let old = w.frame_index_count.replace(count);
    for i in count..old {
        scope.global().delete(&i.to_string())?;
    }
    for i in 0..count {
        let host = Rc::downgrade(host);
        let get = scope.function("get frame", move |s, _, _| {
            let Some(host) = host.upgrade() else { return Ok(s.undefined()) };
            let Some(target) = host.child_keys(key).get(i).copied() else { return Ok(s.undefined()) };
            host.window_value(s, key, target)
        })?;
        define
            .get(&scope)
            .as_object()
            .ok_or(JsError::ContextDestroyed)?
            .call(None, &[scope.global().value(), scope.string(&i.to_string()), get.value()])?;
    }
    Ok(())
}
