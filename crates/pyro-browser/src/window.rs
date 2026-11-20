//! The `Window` global: timers and scheduling primitives.
//!
//! DOM-facing members and EventTarget inheritance come from pyro-webapi;
//! location/history and timer scheduling stay in pyro-browser.

use crate::nav::{History, Location, PageNav, Performance, PerformanceData};
use crate::Browser;
use pyro_js::{Args, ClassDef, IdentityKey, JsClass, JsError, JsResult, Object, Persistent, Realm, Scope, Value};
use pyro_runtime::{Scheduler, TimerId};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

mod idle;

/// Frame interval without rendering. Upstream Lightpanda schedules each
/// animation frame callback 5 ms out (`Window.requestAnimationFrame`).
const FRAME_MS: u64 = 5;
/// Frames run as soon as the thread has nothing else ready (an idle timer)
/// this many times after each client command when no animation moves.
/// Clients that wait for frames (e.g. Bun.WebView's
/// "stable for two frames" click check) get them as soon as the page's work
/// is done instead of on a clock, while a page with a perpetual
/// `requestAnimationFrame` loop falls back to the 5 ms cadence after a few
/// frames instead of spinning.
const EAGER_FRAMES: u32 = 4;

thread_local! {
    /// Eager frames this browser thread may still run.
    static EAGER_FRAMES_LEFT: Cell<u32> = const { Cell::new(0) };
}

/// Notes that an automation client just sent a command (or an `Pyro.run` step
/// acted) on this thread's browser: the next few animation frames run as
/// soon as the page is otherwise idle.
pub fn note_client_activity() {
    EAGER_FRAMES_LEFT.with(|c| c.set(EAGER_FRAMES));
}

/// No eager frame is left ([`crate::reset_thread`]).
pub(crate) fn reset_eager_frames() {
    EAGER_FRAMES_LEFT.with(|c| c.set(0));
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
    /// With an eager frame (an idle timer, run when nothing else is ready): the frame interval's
    /// timer that runs the frame anyway when tasks keep coming (HTML updates the rendering
    /// between tasks whatever their number), so a page whose tasks never stop still gets frames.
    backup: Option<TimerId>,
    /// Chain depth of the frame callback running now, if any.
    running: Option<u32>,
}

pub struct WindowData {
    sched: Scheduler<Browser>,
    nav: Rc<PageNav>,
    target: pyro_webapi::EventTargetData,
    /// A Window owns its associated Document while the realm is reachable,
    /// including after a child frame navigates and the Document becomes inactive.
    /// A reused initial about:blank Window receives new WindowData at commit.
    _associated_document: pyro_webapi::collections::NodeHold,
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
    /// Identity of this realm's `performance` wrapper once it was made.
    performance: Cell<Option<IdentityKey>>,
    /// HTML timer nesting level of the timer task running now (0: none).
    timer_nesting: Cell<u32>,
    /// Pending `requestIdleCallback`s by handle.
    idle_callbacks: RefCell<HashMap<u32, idle::IdleCallback>>,
    /// idle.js (`IdleDeadline`), once made.
    idle_module: RefCell<Option<Persistent>>,
    /// End of the idle period whose callback is running now, if any.
    idle_period_end: Cell<Option<Instant>>,
    /// Calls a timer callback whose nesting resets for microtasks, made on
    /// the first deeply nested timer ([`call_nested_timer`]).
    nested_timer_call: RefCell<Option<Persistent>>,
}

impl WindowData {
    /// The page's session history shared with `window.history`.
    pub(crate) fn nav(&self) -> &Rc<PageNav> {
        &self.nav
    }

    /// The frame this window belongs to (`None` for a window made outside
    /// a page's frame tree).
    pub(crate) fn frame_key(&self) -> Option<crate::frames::FrameKey> {
        self.frame
    }

    /// The frame tree this window belongs to, while it exists.
    pub(crate) fn frame_host(&self) -> Option<Rc<crate::frames::FrameHost>> {
        self.frames.upgrade()
    }

    pub fn new(sched: Scheduler<Browser>, nav: Rc<PageNav>, page: &Rc<pyro_webapi::PageDom>) -> WindowData {
        let key = page.window_target().expect("document window target");
        // `Event.timeStamp` reads the clock `performance.now()` reads.
        let clock = nav.clone();
        page.set_event_clock(Rc::new(move || clock.now_ms()));
        WindowData {
            sched,
            nav,
            target: pyro_webapi::EventTargetData::window(page, key),
            _associated_document: pyro_webapi::collections::NodeHold::new(page, page.document())
                .expect("live Window document"),
            timers: RefCell::new(HashMap::new()),
            animation: RefCell::new(AnimationFrame::default()),
            next_handle: Cell::new(1),
            frames: std::rc::Weak::new(),
            frame: None,
            frame_definer: RefCell::new(None),
            frame_index_count: Cell::new(0),
            performance: Cell::new(None),
            timer_nesting: Cell::new(0),
            idle_callbacks: RefCell::new(HashMap::new()),
            idle_module: RefCell::new(None),
            idle_period_end: Cell::new(None),
            nested_timer_call: RefCell::new(None),
        }
    }

    /// HTML Window reuse: this window's state that the realm keeps for its
    /// next document's Window data (`FrameHost::reuse_realm`). Its event
    /// target's listeners stay with the target key, which the new data has.
    pub(crate) fn hand_over(&self, to: &WindowData) {
        self.target.hand_over(&to.target);
        *to.frame_definer.borrow_mut() = self.frame_definer.take();
        to.frame_index_count.set(self.frame_index_count.get());
        *to.idle_module.borrow_mut() = self.idle_module.take();
        *to.nested_timer_call.borrow_mut() = self.nested_timer_call.take();
        to.next_handle.set(self.next_handle.get());
    }
    pub(crate) fn for_frame(
        sched: Scheduler<Browser>,
        nav: Rc<PageNav>,
        page: &Rc<pyro_webapi::PageDom>,
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

    /// `setTimeout`s (not intervals) due by `deadline`: a redirect or a
    /// render the page has scheduled, for a caller that waits longer than
    /// [`TIMER_HORIZON`] (a fork template before it runs a profile's setup).
    pub fn timeouts_due_by(&self, deadline: Instant) -> usize {
        self.timers.borrow().values().filter(|(_, due)| due.is_some_and(|d| d <= deadline)).count()
    }

    /// Grows with every timer this window's script sets and every animation
    /// frame it requests (the handles given out): two equal readings mean it
    /// asked for neither between them.
    pub fn handles_issued(&self) -> u32 {
        self.next_handle.get()
    }
}

/// Identity keys of the per-window singletons (one wrapper per realm).
const KEY_LOCATION: u64 = 0x4c6f_6361_7469_6f6e;
pub(crate) const KEY_HISTORY: u64 = 0x4869_7374_6f72_7900;
const KEY_PERFORMANCE: u64 = 0x5065_7266_6f72_6d00;

impl pyro_webapi::WindowHost for WindowData {
    fn page(&self) -> &Rc<pyro_webapi::PageDom> {
        &self.target.page
    }
    fn event_target(&self) -> &pyro_webapi::EventTargetData {
        &self.target
    }
}

/// The realm's `performance` ([SameObject] and an EventTarget: the wrapper
/// carries its event listeners, so the realm keeps it).
fn performance_object<'s>(s: &pyro_js::Scope<'s>, w: &WindowData) -> JsResult<pyro_js::Object<'s>> {
    if let Some(o) = w.performance.get().and_then(|identity| s.lookup_identity(identity)) {
        return Ok(o);
    }
    let data = PerformanceData::new(w.nav.clone(), &w.target.page);
    let identity = data.wrapper_identity().unwrap_or(IdentityKey(KEY_PERFORMANCE, 0));
    let key = data.event_target().key;
    let performance = s.wrap::<Performance>(data, Some(identity))?;
    pyro_webapi::target::pin_target(s, &w.target.page, key, performance);
    w.performance.set(Some(identity));
    Ok(performance)
}

pub struct Window;

impl JsClass for Window {
    const NAME: &'static str = "Window";
    type Data = WindowData;
    type Parent = pyro_webapi::EventTarget;

    fn define(def: &mut ClassDef<Self>) {
        def.upcast(|w| &w.target);
        pyro_webapi::define_members::<Window>(def);
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
        def.method("requestIdleCallback", 1, idle::request);
        def.method("cancelIdleCallback", 1, idle::cancel);
        def.accessor("IdleDeadline", idle::interface, idle::replace_interface);
        def.method_this("reportError", 1, |s, _w, this, a| {
            pyro_webapi::report_error::report_error(s, this, &a, "Window", true)
        });
        // No COOP/COEP isolation here.
        def.getter("crossOriginIsolated", |s, _w| Ok(s.boolean(false)));
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
        // Navigation API, made on the realm's first use.
        def.getter("navigation", |s, w| crate::nav::navigation_module(s, &w.nav)?.get("navigation"));
        macro_rules! navigation_interfaces {
            ($($name:literal),*) => {$(
                def.accessor(
                    $name,
                    |s, w| crate::nav::navigation_module(s, &w.nav)?.get($name),
                    |s, w, v| pyro_webapi::window::replace_window_attribute(s, &w.target.page, $name, v),
                );
            )*};
        }
        navigation_interfaces!(
            "Navigation",
            "NavigateEvent",
            "NavigationHistoryEntry",
            "NavigationDestination",
            "NavigationTransition",
            "NavigationCurrentEntryChangeEvent"
        );
        // [Replaceable], as in Chrome: assignment shadows it on the window.
        def.accessor(
            "performance",
            |s, w| Ok(performance_object(s, w)?.value()),
            |s, w, v| pyro_webapi::window::replace_window_attribute(s, &w.target.page, "performance", v),
        );
        // Performance Timeline observers, made on the realm's first use.
        def.accessor(
            "PerformanceObserver",
            |s, w| crate::nav::observer_module(s, performance_object(s, w)?, &w.nav)?.get("PerformanceObserver"),
            |s, w, v| pyro_webapi::window::replace_window_attribute(s, &w.target.page, "PerformanceObserver", v),
        );
        def.accessor(
            "PerformanceObserverEntryList",
            |s, w| {
                crate::nav::observer_module(s, performance_object(s, w)?, &w.nav)?.get("PerformanceObserverEntryList")
            },
            |s, w, v| pyro_webapi::window::replace_window_attribute(s, &w.target.page, "PerformanceObserverEntryList", v),
        );
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
            |s, w, v| pyro_webapi::window::replace_window_attribute(s, &w.target.page, "parent", v),
        );
        def.getter("frameElement", |s, w| match (w.frames.upgrade(), w.frame) {
            (Some(host), Some(key)) => host.frame_element(s, key),
            _ => Ok(s.null()),
        });
        // `window.name`: the name of the window's navigable. A child frame
        // takes it from its iframe's `name` attribute when it is created (and
        // when the attribute changes), a popup from `window.open`.
        def.accessor(
            "name",
            |s, w| {
                let name = match (w.frames.upgrade(), w.frame) {
                    (Some(host), Some(key)) => host.with_frame(key, |f| f.name.clone()),
                    _ => None,
                };
                Ok(s.string(&name.unwrap_or_default()))
            },
            |_s, w, v| {
                let name = v.to_string()?;
                let (Some(host), Some(key)) = (w.frames.upgrade(), w.frame) else { return Ok(()) };
                match key.child {
                    Some(id) => {
                        if let Some(child) = host.children.borrow_mut().get_mut(&id) {
                            child.main_frame.name = name;
                        }
                    }
                    None => {
                        // The root frame mirrors the page's main frame: name both.
                        host.root.borrow_mut().name.clone_from(&name);
                        let page = host.page;
                        w.sched.post(pyro_runtime::TaskSource::DomManipulation, move |browser: &mut Browser| {
                            if let Some(p) = browser.page_mut(page) {
                                p.main_frame.name = name;
                            }
                        });
                    }
                }
                Ok(())
            },
        );
        // `opener` is writable: assigning (e.g. null to disown) shadows it.
        def.accessor(
            "opener",
            |s, w| match (w.frames.upgrade(), w.frame) {
                (Some(host), Some(key)) => host.opener_window(s, key),
                _ => Ok(s.null()),
            },
            |s, w, v| pyro_webapi::window::replace_window_attribute(s, &w.target.page, "opener", v),
        );
        def.accessor(
            "length",
            |s, w| {
                Ok(s.number(match (w.frames.upgrade(), w.frame) {
                    (Some(host), Some(key)) => host.child_keys(key).len() as f64,
                    _ => 0.0,
                }))
            },
            |s, w, v| pyro_webapi::window::replace_window_attribute(s, &w.target.page, "length", v),
        );
        def.getter("origin", |s, _| Ok(s.string(s.realm().origin().as_deref().unwrap_or("null"))));
        def.indexed(|s, w, index| match (w.frames.upgrade(), w.frame) {
            (Some(host), Some(key)) => {
                host.child_keys(key).get(index as usize).map(|target| host.window_value(s, key, *target)).transpose()
            }
            _ => Ok(None),
        });
        // [LegacyUnforgeable]: the global's own constant where the
        // JavaScriptCore supports it (`pyro_webapi::install`).
        if !pyro_webapi::window::has_window_constants() {
            def.getter("window", |s, _w| Ok(s.global().value()));
        }
        def.accessor(
            "self",
            |s, _w| Ok(s.global().value()),
            |s, w, v| pyro_webapi::window::replace_window_attribute(s, &w.target.page, "self", v),
        );
        def.accessor(
            "frames",
            |s, _w| Ok(s.global().value()),
            |s, w, v| pyro_webapi::window::replace_window_attribute(s, &w.target.page, "frames", v),
        );
        def.getter("isSecureContext", |s, w| Ok(s.boolean(is_secure_context(w))));
        def.method("__lpNow", 0, |s, w, _a| Ok(s.number(w.now_ms())));
        // Window post message steps: from the window whose script is running.
        def.method("postMessage", 1, |s, w, a| {
            match (w.frames.upgrade(), w.frame) {
                (Some(host), Some(key)) => {
                    crate::frames::post_message(s, &a, crate::frames::incumbent_window(), &host, key)?
                }
                _ => pyro_webapi::window::post_message_to_self(s, &w.target.page, &a)?,
            }
            Ok(s.undefined())
        });
    }
}

/// HTML "secure context" for a window: its document's URL and those of its
/// ancestor frames are potentially trustworthy (Chrome checks every
/// ancestor; an about:blank or about:srcdoc document counts as its parent).
fn is_secure_context(w: &WindowData) -> bool {
    match (w.frames.upgrade(), w.frame) {
        (Some(host), Some(key)) => frame_chain_secure(&host, key, &w.url()),
        _ => trustworthy_url(&w.url()),
    }
}

fn trustworthy_url(url: &str) -> bool {
    pyro_net::Url::parse(url).is_ok_and(|u| pyro_net::is_potentially_trustworthy_url(&u))
}

/// HTML "secure context" for the document of frame `key` at `url`: it and every ancestor frame's
/// document are potentially trustworthy (also the page's `[SecureContext]` members,
/// `PageHooks::is_secure_context`).
pub(crate) fn frame_chain_secure(host: &crate::frames::FrameHost, mut key: crate::frames::FrameKey, url: &str) -> bool {
    let mut url = url.to_owned();
    loop {
        // A popup's about:blank / about:srcdoc document: its creator's state (`FrameHost::creator_secure`),
        // which already covers the creator's ancestors; it ends the walk, for the popup itself and for
        // the frames inside it.
        if key.child.is_none() && matches!(url.as_str(), "about:blank" | "about:srcdoc") {
            if let Some(secure) = host.creator_secure.get() {
                return secure;
            }
        }
        if !document_trustworthy(host, key, &url) {
            return false;
        }
        if key.child.is_none() {
            return true;
        }
        let parent = host.parent_key(key);
        if parent == key {
            return true;
        }
        key = parent;
        let Some(nav) = host.nav(key) else { return false };
        url = nav.url();
    }
}

/// Whether the document of frame `key` at `url` is potentially trustworthy: by its tuple origin
/// (HTML "secure context" asks of the environment's origin), which an about:blank document inherits
/// from its creator (a popup's initial document from an http page is not trustworthy, though
/// about:blank URLs are); an opaque origin by its URL (data:, a sandboxed frame).
fn document_trustworthy(host: &crate::frames::FrameHost, key: crate::frames::FrameKey, url: &str) -> bool {
    let origin = host
        .with_frame(key, |f| pyro_net::Url::parse(&f.origin).ok().map(|u| u.origin()))
        .flatten()
        .filter(|o| o.is_tuple());
    match origin {
        Some(origin) => pyro_net::is_potentially_trustworthy_origin(&origin),
        None => trustworthy_url(url),
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

/// HTML timer nesting: a timer set from a timer task more than this many
/// levels deep waits at least [`NESTED_TIMER_MIN_MS`] (Chrome's clamp of
/// `setTimeout(f, 0)` polling chains and `setInterval(f, 0)`).
const TIMER_NESTING_LIMIT: u32 = 5;
const NESTED_TIMER_MIN_MS: u64 = 4;

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
    // The running task's nesting level (0 outside a timer task).
    let nesting = w.timer_nesting.get();
    let delay_ms = if nesting > TIMER_NESTING_LIMIT { delay_ms.max(NESTED_TIMER_MIN_MS) } else { delay_ms };
    let timer = JsTimer { handle, cb, args, delay_ms, repeat };
    let id = arm_timer(s.realm().downgrade(), w, timer, nesting.saturating_add(1));
    let due = (!repeat).then(|| crate::clock::now() + Duration::from_millis(delay_ms));
    w.timers.borrow_mut().insert(handle, (id, due));
    Ok(s.number(handle as f64))
}

/// A `setTimeout`/`setInterval` as its loop timer runs it.
struct JsTimer {
    handle: u32,
    cb: Persistent,
    args: Vec<Persistent>,
    delay_ms: u64,
    repeat: bool,
}

/// Sets the loop timer of `timer`, whose task runs at nesting level `level`
/// (an interval's repetitions one deeper each).
fn arm_timer(realm_weak: pyro_js::WeakRealm, w: &WindowData, timer: JsTimer, level: u32) -> TimerId {
    let level = Cell::new(level);
    let (delay_ms, repeat) = (timer.delay_ms, timer.repeat);
    // Timers must not keep a disposed realm alive: hold it weakly.
    w.sched.set_timer(Duration::from_millis(delay_ms), repeat, true, move |_browser| {
        let Some(realm) = realm_weak.upgrade() else { return };
        let Ok(scope) = realm.scope() else { return };
        let Some(w) = scope.global_data::<Window>() else { return };
        let handle = timer.handle;
        if !repeat {
            w.timers.borrow_mut().remove(&handle);
        }
        let Some(f) = timer.cb.get(&scope).as_object() else { return };
        let run_level = level.get();
        let outer = w.timer_nesting.replace(run_level);
        // Arguments are rooted by `args` (Persistent) while this call runs.
        let vals: Vec<Value<'_>> = timer.args.iter().map(|p| p.get(&scope)).collect();
        let result = if run_level > TIMER_NESTING_LIMIT {
            call_nested_timer(&scope, w, f, &vals)
        } else {
            f.call(None, &vals).map(|_| ())
        };
        w.timer_nesting.set(outer);
        if let Err(e) = result {
            scope.report_exception(&e);
        }
        if !repeat {
            return;
        }
        level.set(run_level.saturating_add(1));
        // The repetition is set from this task: an interval below 4 ms this
        // deep is clamped from now on (once; later repetitions keep 4 ms).
        if delay_ms < NESTED_TIMER_MIN_MS && run_level > TIMER_NESTING_LIMIT {
            let mut timers = w.timers.borrow_mut();
            if let Some(entry) = timers.get_mut(&handle) {
                let old = entry.0;
                let clamped = JsTimer {
                    handle,
                    cb: timer.cb.clone(),
                    args: timer.args.clone(),
                    delay_ms: NESTED_TIMER_MIN_MS,
                    repeat: true,
                };
                entry.0 = arm_timer(realm.downgrade(), w, clamped, run_level.saturating_add(1));
                drop(timers);
                w.sched.clear_timer(old);
            }
        }
    })
}

/// Calls a timer callback above the nesting limit so that timers set by the
/// microtasks after it are not nested in it (HTML: the microtask checkpoint
/// is no timer task): a microtask queued ahead of the callback's own resets
/// the level. JavaScriptCore drains microtasks when the outermost API call
/// returns, so the callback runs inside a native function that queues the
/// reset first (made once per window, on its first such timer). Only
/// callbacks already clamped to 4 ms come here.
fn call_nested_timer(scope: &Scope<'_>, w: &WindowData, f: Object<'_>, args: &[Value<'_>]) -> JsResult<()> {
    let cached = w.nested_timer_call.borrow().as_ref().and_then(|p| p.get(scope).as_object());
    let call = match cached {
        Some(call) => call,
        None => {
            let call = scope.function("", |s, _this, a| {
                s.then(
                    s.undefined(),
                    |s, _| {
                        if let Some(w) = s.global_data::<Window>() {
                            w.timer_nesting.set(0);
                        }
                    },
                    |_, _| {},
                )?;
                let f = a.get(0).as_object().ok_or(JsError::ContextDestroyed)?;
                let args: Vec<Value<'_>> = a.iter().skip(1).collect();
                f.call(None, &args)?;
                Ok(s.undefined())
            })?;
            *w.nested_timer_call.borrow_mut() = Some(Persistent::new(scope, call.value()));
            call
        }
    };
    let mut call_args = Vec::with_capacity(args.len() + 1);
    call_args.push(f.value());
    call_args.extend_from_slice(args);
    call.call(None, &call_args)?;
    Ok(())
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
        // Moving animations must advance their layout sample between callbacks,
        // including after a client command replenishes the eager-frame budget.
        let animation_interval = w.target.page.moving_animation_frame_interval();
        frame.timer = Some(if animation_interval.is_none() && take_eager_frame() {
            let realm_weak = s.realm().downgrade();
            frame.backup = Some(w.sched.set_frame_timer(Duration::from_millis(FRAME_MS), true, move |_browser: &mut Browser| {
                let Some(realm) = realm_weak.upgrade() else { return };
                let Ok(scope) = realm.scope() else { return };
                run_animation_frame(&scope);
            }));
            w.sched.set_idle_timer(true, run)
        } else {
            // A frame timer: once due the frame runs before the tasks and timers that are
            // waiting, however many, so a page that keeps its thread busy (timers always
            // behind) renders on time and a client waiting for frames (a click's "stable for
            // two frames" check) is not held behind every round of the page's backlog.
            w.sched.set_frame_timer(animation_interval.unwrap_or(Duration::from_millis(FRAME_MS)), true, run)
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

/// Runs an animation frame of the realm's window soon, for native work done
/// in it (`requestVideoFrameCallback`).
pub(crate) fn request_frame(realm: &Realm) {
    let Ok(scope) = realm.scope() else { return };
    let Some(window) = scope.global_data::<Window>() else { return };
    let mut frame = window.animation.borrow_mut();
    schedule_animation_frame(&scope, window, &mut frame);
}

/// Forgets the frame's timers (the one that fired, and the other of an eager frame's pair).
fn clear_frame_timers(w: &WindowData) {
    let mut frame = w.animation.borrow_mut();
    for id in [frame.timer.take(), frame.backup.take()].into_iter().flatten() {
        w.sched.clear_timer(id);
    }
}

fn run_animation_frame(scope: &Scope<'_>) {
    let Some(w) = scope.global_data::<Window>() else { return };
    // A render-blocked document (a pending render-blocking stylesheet or
    // script, an unresolved `<link rel=expect>`) has no rendering
    // opportunity: the frame runs as soon as rendering is unblocked.
    if w.target.page.render_blocked() {
        clear_frame_timers(w);
        let realm_weak = scope.realm().downgrade();
        w.target.page.when_render_unblocked(Box::new(move || {
            let Some(realm) = realm_weak.upgrade() else { return };
            let Ok(scope) = realm.scope() else { return };
            let Some(w) = scope.global_data::<Window>() else { return };
            let mut frame = w.animation.borrow_mut();
            if frame.timer.is_none() && !(frame.callbacks.is_empty() && frame.native_callbacks.is_empty()) {
                let realm_weak = realm.downgrade();
                frame.timer = Some(w.sched.set_timer(Duration::ZERO, false, true, move |_browser: &mut Browser| {
                    let Some(realm) = realm_weak.upgrade() else { return };
                    let Ok(scope) = realm.scope() else { return };
                    run_animation_frame(&scope);
                }));
            }
        }));
        return;
    }
    pyro_webapi::layout_api::flush_pending_scrolls(&w.target.page);
    // pyro: the frame determines `content-visibility: auto` relevance anew (CSS Contain 2 § 4.1).
    pyro_webapi::layout_api::content_visibility_frame(&w.target.page);
    // The style update of the frame before this one, when the document changed since and none
    // ran yet (the first frame after a load, whose update is a task of its own that this
    // frame's timer can beat): the styles a callback of this frame changes then have their
    // before-change values, and the change starts its transitions. A browser has rendered
    // the document before its first animation frame callback runs. One flag and stamp check
    // for a document that neither animates nor transitions, or did not change.
    w.target.page.update_animations();
    let (handles, native): (Vec<u32>, _) = {
        clear_frame_timers(w);
        let mut frame = w.animation.borrow_mut();
        (frame.callbacks.iter().map(|(h, _, _)| *h).collect(), std::mem::take(&mut frame.native_callbacks))
    };
    let now_ms = w.now_ms();
    // HTML video frame callbacks run before the animation frame callbacks.
    pyro_webapi::media::run_video_frame_callbacks(&w.target.page, now_ms);
    let now = scope.number(now_ms);
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
    // The frame's style update: what its callbacks changed starts transitions
    // and animations now (the next frame's callbacks see them running).
    w.target.page.update_animations();
    w.target.page.sync_frame_resources();
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
                    for id in [frame.timer.take(), frame.backup.take()].into_iter().flatten() {
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
    if realm.is_deferred() {
        return;
    }
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
        for id in [frame.timer.take(), frame.backup.take()].into_iter().flatten() {
            w.sched.clear_timer(id);
        }
        drop(frame);
        idle::cancel_all(w);
    }
}

/// Drops what the window of a lingering realm (`Realm::linger`) roots in its
/// own realm, besides its timers: the frame index definer, idle.js and the
/// nested timer caller. Each would keep the realm's global alive from the
/// global's own payload, which the collector cannot see through.
pub(crate) fn release_roots(realm: &Realm) {
    // A deferred realm was never made: entering it would make it now.
    if realm.is_deferred() {
        return;
    }
    cancel_timers(realm);
    let Ok(scope) = realm.scope() else { return };
    if let Some(w) = scope.global_data::<Window>() {
        drop(w.frame_definer.borrow_mut().take());
        drop(w.idle_module.borrow_mut().take());
        drop(w.nested_timer_call.borrow_mut().take());
    }
}

/// Keeps `define`, the step defining a frame index getter on the global
/// (pyro-webapi's install.js `accessor`, which captured the descriptor
/// intrinsic before page init scripts can replace it).
pub(crate) fn init_frame_indices(scope: &pyro_js::Scope<'_>, define: pyro_js::Value<'_>) -> JsResult<()> {
    let w = scope.global_data::<Window>().ok_or(JsError::ContextDestroyed)?;
    *w.frame_definer.borrow_mut() = Some(Persistent::new(scope, define));
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
