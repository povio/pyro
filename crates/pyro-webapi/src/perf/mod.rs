//! The performance-entry family beyond User Timing / Resource Timing /
//! Navigation Timing: Paint Timing, Largest Contentful Paint, Element
//! Timing, Layout Instability, Event Timing, Long Tasks, Long Animation
//! Frames, Soft Navigations. This file is the interface the lanes code
//! against; each lane edits only its own `perf/<module>.rs`.
//!
//! # What a module is
//!
//! Every module has the same four items (some have more, below):
//!
//! ```ignore
//! pub const ENTRY_TYPES: &[&str];   // entry types it makes real ("layout-shift")
//! pub const ENABLED: bool;          // flip to true when the module really works
//! pub fn register(engine: &pyro_js::Engine);          // its JsClass(es)
//! pub fn on_frame(page: &Rc<PageDom>, frame: &FrameInfo); // if it has frame work
//! ```
//!
//! While `ENABLED` is false the module is never called, its classes are not
//! registered and its types are not in `PerformanceObserver.supportedEntryTypes`
//! (nothing is advertised without behaviour). When it is true: `register` runs
//! once per engine (in `Browser::new_engine`), the module's `ENTRY_TYPES` are
//! listed in `supportedEntryTypes` (alphabetical), and its hooks below run.
//! Flipping `ENABLED` is the only edit outside the lane's file that is needed:
//! there is none. Every hook is called on the document's thread with no DOM
//! borrow held; a hook may take `page.dom.borrow()` itself.
//!
//! # Making entries (all modules but `paint`)
//!
//! Define the entry class(es) in the module (`JsClass`, parent
//! `crate::performance::PerformanceEntry` so `name`/`entryType`/`startTime`/
//! `duration` come from `PerformanceEntryData`, or your own getters), make an
//! `Rc<T>` of the entry data, implement [`timeline::TimelineEntry`] for it and
//! call `timeline::queue_entry(page, entry)`. That queues it for the observers
//! of its type in every realm of the document, and buffers it per the spec
//! limits for `observe({type, buffered: true})` and `droppedEntriesCount`.
//! Entries of these types are NOT in `performance.getEntries*()` (only paint is).
//! `TimelineEntry::wrap` must pass the key it is given to
//! `s.wrap::<Class>(data, Some(key))`.
//! `observe({type: "event", durationThreshold})` is filtered by core from the
//! entry's `duration()`; `includeSoftNavigationObservations` by core from
//! `soft_navigation()`; measuring and making the entries is the module's.
//! `page.timeline().is_observed("layout-shift")` tells whether some observer
//! watches a type now: skip work nobody can see (but `buffered` readers see
//! entries made before they observed: a module whose entries must exist for
//! late observers (lcp, layout-shift, element, event, longtask) keeps
//! producing while the page lives, observed or not, as the spec says).
//!
//! # Hooks, in the order they run
//!
//! | Hook | Called by | When |
//! |---|---|---|
//! | `<m>::on_frame(page, &FrameInfo)` | [`rendering_update`], from `run_animation_frame` (`pyro-browser/src/window.rs`) | at the end of every rendering update (animation frame) of the document's window, after rAF callbacks, the style updates, ResizeObserver step, focus fixup and the canvas paint step; order: shift, paint, element, lcp, event, loaf, softnav. Frames only run when something asked for one (rAF, an animation, a client command, or `timeline::request_rendering`); a render-blocked document runs none |
//! | `longtask::on_task(page, &TaskInfo)`, `loaf::on_task(page, &TaskInfo)` | [`task_finished`], from `Browser::task_finished` (`LoopState::task_finished`, `pyro-runtime`) | after every event-loop task (also frame timers, idle timers), before the checkpoint (`after_task`). Called for EVERY document (every frame of every page) of the browser, not only those whose script ran; times are in that document's clock. Filter on duration yourself (core does nothing but the `ENABLED` check). Cost when disabled: two `Instant::now()` per task |
//! | `event::dispatch_begin(page, &EventData) -> Option<DispatchToken>`, `event::dispatch_end(page, &EventData, DispatchToken)`; `softnav::dispatch_begin/end` the same | [`dispatch_begin`] / [`dispatch_end`], from `target.rs dispatch_at` | around the listeners of every TRUSTED event (not synthetic `dispatchEvent` ones: `event.state.is_trusted`), every type: the module decides which types matter. The token is yours to carry between the two calls. Callers that skip dispatching for an event nobody listens to (`target::unheard`) never reach it: `eventCounts` must not rely on them, see below |
//! | `event::event_counts(s, page) -> JsResult<Value>`, `event::interaction_count(page) -> u32` | `performance.eventCounts` / `performance.interactionCount` getters of the window's `Performance` (`pyro-browser/src/nav.rs`), defined only when `event::ENABLED` | on property read |
//!
//! `FrameInfo` (document-clock ms, `performance.now()`'s clock): `start` is the
//! rAF timestamp, `paint_time` the time after the rAF callbacks and the style
//! update (layout is final from there), `presented` the time at the end of
//! the rendering update ("presentation": nothing is rasterized, the end of
//! Pyro's own update is the definition). `start <= paint_time <= presented`.
//! `TaskInfo`: `start` / `end` of the task in the document clock.
//!
//! # Last user input
//!
//! `page.timeline().last_input_time()` / `last_wheel_time()` / `recent_input(now, window)`:
//! the `timeStamp` of the last trusted `mousedown`, `pointerdown`, `pointerup`,
//! `keydown`, `touchstart`, `touchend` (`wheel` separately), for layout-shift
//! (`hadRecentInput` = input within the last 500 ms of the shift, `lastInputTime`
//! = `last_input_time()` or 0) and LCP (stops considering candidates after the
//! first input or wheel). Set by [`dispatch_begin`] (and by [`note_unheard`] for
//! events without a listener), only while `shift`, `lcp` or `event` is ENABLED
//! ([`TRACK_INPUT`], a const): otherwise it costs nothing and reads `None`.
//!
//! # Asking for frames
//!
//! `timeline::request_rendering(page)` asks for one rendering update (a plain
//! frame timer, not an eager frame): [`rendering_update`] runs in it and
//! `page.timeline().wants_rendering()` is false at the start of every
//! `on_frame` (so a module that has work left calls `request_rendering` again
//! from `on_frame`; one that does not, stops; never request unconditionally
//! from `on_frame`: that is an animation loop). Core also requests one
//! when an observer of an ENABLED type starts observing. A page with no
//! rAF and no observer pays no frame.
//!
//! # Paint
//!
//! `paint` is a supported type already (`getEntries*()` lists it). Today
//! `PageNav::note_first_paint` makes `first-paint` and
//! `first-contentful-paint` at the first safe point with a body (called by
//! `Browser::checkpoint`). When `paint::ENABLED` is true that fallback is
//! skipped, and `paint::on_frame` owns both: it reports each with
//! `timeline::report_paint(page, PaintRecord { name, start, paint_time, presentation_time })`
//! (adds it to `getEntries*()` in `startTime` order and queues it for
//! observers; `presentation_time: Some(..)` -> `startTime` is it). The legacy
//! fallback gives `presentationTime` null and `paintTime == startTime`.
//!
//! # Timeline state (`page.timeline()`)
//!
//! `queue_entry`, `buffered`, `dropped`, `is_observed`, `paint_records`,
//! `wants_rendering` (see `timeline.rs`).
//!
//! # `PerformanceObserver` semantics (performance_observer.js)
//!
//! Core owns it: `observe({type, buffered, durationThreshold (event only: at
//! least 16, rounded to 8, default 104), includeSoftNavigationObservations})`,
//! `droppedEntriesCount`, sorting by `startTime`.
//!
//! Not in core (left for whoever needs it): the painted-candidate scan of the
//! plan (`perf/scan.rs`): `paint`, `lcp`, `element` and `softnav` each compute
//! what they need from layout themselves for now.

use std::rc::Rc;

use crate::event::EventData;
use crate::page::PageDom;

pub mod element;
pub mod event;
pub mod lcp;
pub mod loaf;
pub mod longtask;
pub mod paint;
pub mod shift;
pub mod softnav;
pub mod timeline;

/// One rendering update of a document's window. See the module docs.
#[derive(Clone, Copy, Debug)]
pub struct FrameInfo {
    pub start: f64,
    pub paint_time: f64,
    pub presented: f64,
}

/// One event-loop task, in the document's clock. See the module docs.
#[derive(Clone, Copy, Debug)]
pub struct TaskInfo {
    pub start: f64,
    pub end: f64,
}

/// Entry types every window has (User Timing, Resource Timing, Navigation
/// Timing, Paint Timing's baseline).
const BASE_TYPES: [&str; 5] = ["mark", "measure", "navigation", "paint", "resource"];

/// `(ENABLED, ENTRY_TYPES)` of every module.
fn modules() -> [(bool, &'static [&'static str]); 8] {
    [
        (paint::ENABLED, paint::ENTRY_TYPES),
        (lcp::ENABLED, lcp::ENTRY_TYPES),
        (element::ENABLED, element::ENTRY_TYPES),
        (shift::ENABLED, shift::ENTRY_TYPES),
        (event::ENABLED, event::ENTRY_TYPES),
        (longtask::ENABLED, longtask::ENTRY_TYPES),
        (loaf::ENABLED, loaf::ENTRY_TYPES),
        (softnav::ENABLED, softnav::ENTRY_TYPES),
    ]
}

/// `PerformanceObserver.supportedEntryTypes` of a Window, alphabetical: the
/// base types and the types of every ENABLED module.
pub fn supported_entry_types() -> Vec<&'static str> {
    let mut types: Vec<&'static str> = BASE_TYPES.to_vec();
    for (enabled, own) in modules() {
        if enabled {
            types.extend_from_slice(own);
        }
    }
    types.sort_unstable();
    types.dedup();
    types
}

/// Registers the classes of every ENABLED module (`Browser::new_engine`).
pub fn register(engine: &pyro_js::Engine) {
    if paint::ENABLED {
        paint::register(engine);
    }
    if lcp::ENABLED {
        lcp::register(engine);
    }
    if element::ENABLED {
        element::register(engine);
    }
    if shift::ENABLED {
        shift::register(engine);
    }
    if event::ENABLED {
        event::register(engine);
    }
    if longtask::ENABLED {
        longtask::register(engine);
    }
    if loaf::ENABLED {
        loaf::register(engine);
    }
    if softnav::ENABLED {
        softnav::register(engine);
    }
}

/// Whether a module is ENABLED for `entry_type` (an observer of it needs
/// frames).
fn needs_frames(entry_type: &str) -> bool {
    modules().iter().any(|(enabled, own)| *enabled && own.contains(&entry_type))
        || (paint::ENABLED && entry_type == "paint")
}

/// The observers of `s`'s realm observe `types` now
/// (performance_observer.js `native.observing`): the document gets a frame
/// when one of them belongs to an ENABLED module, so the module's first
/// `on_frame` runs soon after.
pub(crate) fn observing(page: &PageDom, s: &pyro_js::Scope<'_>, types: Vec<String>) {
    let wanted = types.iter().any(|t| needs_frames(t));
    page.timeline().set_observed(s, types);
    if wanted {
        timeline::request_rendering(page);
    }
}

/// The end of a rendering update of `page`'s document: runs the `on_frame`
/// of every ENABLED module (`window.rs run_animation_frame`).
pub fn rendering_update(page: &Rc<PageDom>, frame: &FrameInfo) {
    page.timeline().clear_wants_rendering();
    if page.is_disposed() {
        return;
    }
    if shift::ENABLED {
        shift::on_frame(page, frame);
    }
    if paint::ENABLED {
        paint::on_frame(page, frame);
    }
    if element::ENABLED {
        element::on_frame(page, frame);
    }
    if lcp::ENABLED {
        lcp::on_frame(page, frame);
    }
    if event::ENABLED {
        event::on_frame(page, frame);
    }
    if loaf::ENABLED {
        loaf::on_frame(page, frame);
    }
    if softnav::ENABLED {
        softnav::on_frame(page, frame);
    }
}

/// Whether some module wants [`task_finished`] calls (one branch per task
/// in `Browser`).
pub const TASK_HOOKS: bool = longtask::ENABLED || loaf::ENABLED;

/// A task of the event loop ended (`Browser::task_finished`).
pub fn task_finished(page: &Rc<PageDom>, task: &TaskInfo) {
    if longtask::ENABLED {
        longtask::on_task(page, task);
    }
    if loaf::ENABLED {
        loaf::on_task(page, task);
    }
}

/// Whether the document tracks the last trusted user input
/// (`TimelineBuffers::last_input_time`): only a module that reads it makes
/// it worth a branch. A const, so a build with none of them enabled
/// compiles the tracking away.
pub(crate) const TRACK_INPUT: bool = shift::ENABLED || lcp::ENABLED || event::ENABLED;

/// Records the time of a trusted user input of type `ty` (`time`: the event's
/// `timeStamp`, document clock). Called from [`dispatch_begin`] for dispatched
/// events and from [`note_unheard`] for the ones nobody listens for. Which
/// events count (Chrome `LayoutShiftTracker`/LCP): `mousedown`, `pointerdown`,
/// `pointerup`, `keydown`, `touchstart`, `touchend`; `wheel` goes to its own
/// slot. Pointer type is not distinguished (Chrome ignores a touch
/// `pointerdown`; a touch `pointerup` counts instead: a touch gesture's `pointerup`
/// follows its `pointerdown` within the 500 ms window, so the difference is
/// only visible for a long press).
pub(crate) fn note_input(page: &PageDom, ty: &str, time: f64) {
    match ty {
        "mousedown" | "pointerdown" | "pointerup" | "keydown" | "touchstart" | "touchend" => page.timeline().set_last_input(time),
        "wheel" => page.timeline().set_last_wheel(time),
        _ => {}
    }
}

/// What [`dispatch_begin`] hands [`dispatch_end`].
#[derive(Default)]
pub(crate) struct DispatchTokens {
    event: Option<event::DispatchToken>,
    soft: Option<softnav::DispatchToken>,
}

/// A trusted event is about to run its listeners (`target.rs dispatch_at`).
pub(crate) fn dispatch_begin(page: &PageDom, event: &EventData) -> DispatchTokens {
    if !(event::ENABLED || softnav::ENABLED || TRACK_INPUT) || !event.state.borrow().is_trusted {
        return DispatchTokens::default();
    }
    if TRACK_INPUT {
        let state = event.state.borrow();
        note_input(page, &state.ty, state.time_stamp);
    }
    DispatchTokens {
        event: if event::ENABLED { event::dispatch_begin(page, event) } else { None },
        soft: if softnav::ENABLED { softnav::dispatch_begin(page, event) } else { None },
    }
}

/// The listeners of the event [`dispatch_begin`] saw ran.
pub(crate) fn dispatch_end(page: &PageDom, event: &EventData, tokens: DispatchTokens) {
    if let Some(token) = tokens.event {
        event::dispatch_end(page, event, token);
    }
    if let Some(token) = tokens.soft {
        softnav::dispatch_end(page, event, token);
    }
}
