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
//! | `longtask::on_task(page, &TaskInfo)`, `loaf::on_task(page, &TaskInfo)` | [`task_finished`], from `Browser::task_finished` (`LoopState::task_finished`, `pyro-runtime`) | after every event-loop task (also frame timers, idle timers), before the checkpoint (`after_task`). Called for EVERY document (every frame of every page) of the browser, not only those whose script ran; times are in that document's clock. Tasks under 50 ms are skipped before any document is visited unless `loaf` is enabled (`TASK_BOOKKEEPING`); `loaf` filters for itself. Cost when disabled: two `Instant::now()` and one depth read per task, nothing else |
//! | `event::dispatch_begin(page, &EventData) -> Option<DispatchToken>`, `event::dispatch_end(page, &EventData, DispatchToken)`; `softnav::dispatch_begin/end` the same | [`dispatch_begin`] / [`dispatch_end`], from `target.rs dispatch_at` | around the listeners of every TRUSTED event (not synthetic `dispatchEvent` ones: `event.state.is_trusted`), every type: the module decides which types matter. The token is yours to carry between the two calls. Callers that skip dispatching for an event nobody listens to (`target::unheard`) never reach it: the module also gets them through `event::note_unheard(page, &UnheardEvent)` (see [`note_unheard`]), so `eventCounts` and the entries count every trusted input event, heard or not |
//! | `event::event_counts(s, page) -> JsResult<Value>`, `event::interaction_count(page) -> u32` | `performance.eventCounts` / `performance.interactionCount` getters of the window's `Performance` (`pyro-browser/src/nav.rs`), defined only when `event::ENABLED` | on property read |
//!
//! `FrameInfo` (document-clock ms, `performance.now()`'s clock): `start` is the
//! rAF timestamp, `paint_time` the time after the rAF callbacks and the style
//! update (layout is final from there), `presented` the time at the end of
//! the rendering update ("presentation": nothing is rasterized, the end of
//! Pyro's own update is the definition). `render_start <= start <= style_and_layout_start <= paint_time <= presented` (`render_start`
//! is where the update began, `style_and_layout_start` where the rAF callbacks ended; a frame's
//! `scripts` ran between them, the style and layout the callbacks forced inside them).
//! Tasks shorter than [`LONG_TASK_MS`] are only delivered while [`TASK_BOOKKEEPING`]
//! (`loaf`); `longtask::on_task` therefore never sees one, and need not filter.
//! `TaskInfo`: `start` / `end` of the task in the document clock, `depth`
//! and `nested_ms` (below).
//!
//! # Nested event loops
//!
//! `task_finished` is also called for a task a nested event loop runs inside
//! another task (a task that calls into `run_until`/`run_one`, e.g. a spin of
//! the event loop while a sync request or `alert` waits): its `depth` is > 1
//! and its span lies inside the outer task's, which is reported after it with
//! `nested_ms` = what the nested tasks took. What the modules do:
//! - `longtask`: ignore `task.is_nested()` tasks (a nested task is not a task of
//!   the document's event loop: "spin the event loop is not a long task",
//!   `spin-eventloop-not-longtask`); for the outer one use `end - start` (the
//!   document did not respond for all of it, `longtask-sync-xhr`).
//! - `loaf`: same, and attribute script time with `own_duration()` (the
//!   nested tasks' scripts are counted by themselves or not at all) so
//!   `blockingDuration` and the frame's `duration` do not count a span twice.
//! Both calls come in finish order: nested ones first.
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
//! # Painted candidates (`perf/scan.rs`)
//!
//! `scan::painted_candidates(page, frame) -> Rc<FrameScan>`, once per frame and shared
//! by every module that asks (memoized by `frame.presented`; a document whose layout
//! stamp, image loads and canvases did not change returns the previous result, so a
//! layout is not forced on pages that sit still). `FrameScan::candidates` is
//! everything painted now, in document order: `Candidate { node, kind (Text, Image,
//! VideoPoster, BackgroundImage, Svg, Canvas), url, natural, border_box, visible,
//! area, load_time, first_painted }`; `FrameScan::newly_painted(frame)` those first
//! painted in this frame. Who uses what: `paint` (FCP: any candidate; FP: its own
//! rule), `lcp` (`area`, `natural` for the upscaled-image rule, `url`, `load_time`,
//! `first_painted` = render time), `element` (the candidates whose node has an
//! `elementtiming` attribute: `visible` is the `intersectionRect`), `softnav` (the
//! candidates first painted after a soft navigation's start). Details, what counts and
//! what does not, and the identity rule are in the `scan.rs` docs. The scan walks
//! the document and reads computed styles: a module calls it only when it will use the
//! result (not while nothing observes and nothing is pending). `scan::walks(page)`
//! counts the walks. A lane that needs another fact adds a field to `Candidate`
//! in `scan.rs` (one small commit, named in its hand-back) and does not write a second
//! scan.
//!
//! # Soft navigations: who edits what
//!
//! Lane perf-8 owns every soft-navigation hook, behind `softnav::ENABLED`: the History
//! API / Navigation API URL-change notification (`pyro-browser/src/nav.rs`, the one file
//! outside `perf/` it edits besides registrations), the DOM-mutation notification
//! it needs to attribute "paints after the URL change" (it uses `scan` for the painted
//! side; a mutation hook on `PageDom` that is a no-op unless `softnav::ENABLED` is
//! perf-8's to add) and the user-interaction side (`softnav::dispatch_begin/end`,
//! called by core around trusted events). Other lanes do not touch these paths; `shift`,
//! `lcp` and `element` get soft-navigation attribution through
//! `TimelineEntry::soft_navigation` and `queue_entry` only.

use std::rc::Rc;

use crate::event::EventData;
use crate::page::PageDom;

pub mod element;
pub mod event;
pub mod lcp;
pub mod loaf;
pub mod longtask;
pub mod paint;
pub mod scan;
pub mod shift;
pub mod softnav;
pub mod timeline;

/// One rendering update of a document's window. See the module docs.
#[derive(Clone, Copy, Debug)]
pub struct FrameInfo {
    pub start: f64,
    pub paint_time: f64,
    pub presented: f64,
    /// The rendering update began (before reveal, autofocus, resize and scroll steps and the
    /// rAF timestamp): long animation frames' `renderStart`.
    pub render_start: f64,
    /// The frame's rAF callbacks are done and its style and layout update begins: LoAF's
    /// `styleAndLayoutStart`. `start`/`render_start` precede it; callbacks run between.
    pub style_and_layout_start: f64,
}

/// One event-loop task, in the document's clock. See the module docs.
#[derive(Clone, Copy, Debug)]
pub struct TaskInfo {
    pub start: f64,
    pub end: f64,
    /// 1 for an ordinary task, more for a task run inside another task by a
    /// nested event loop (`pyro_runtime::task_depth`). A nested task's
    /// span lies inside its outer task's.
    pub depth: u32,
    /// Milliseconds the tasks run inside this one by nested event loops took
    /// (their own `depth > 1` calls came before this one); 0 for a task that
    /// spun no loop.
    pub nested_ms: f64,
}

impl TaskInfo {
    /// Whether the task ran inside another one's nested event loop.
    pub fn is_nested(&self) -> bool {
        self.depth > 1
    }

    /// `end - start` less the time of the tasks nested in it: what the task's
    /// own code ran.
    pub fn own_duration(&self) -> f64 {
        (self.end - self.start - self.nested_ms).max(0.0)
    }
}

thread_local! {
    /// Per task depth (index 0 = depth 1): milliseconds finished tasks one depth
    /// deeper took since a task of this depth last finished.
    static NESTED_MS: std::cell::RefCell<Vec<f64>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Accounts a finished task of `depth` (`pyro_runtime::task_depth`, 1 = outermost) that took
/// `duration` ms, once per task before its documents are told: returns the ms
/// the tasks nested in it took ([`TaskInfo::nested_ms`]) and adds its own
/// duration to its outer task's. No allocation after the first deep task.
pub fn task_accounted(depth: u32, duration: f64) -> f64 {
    let depth = depth.max(1) as usize;
    NESTED_MS.with(|nested| {
        let mut nested = nested.borrow_mut();
        if nested.len() < depth {
            nested.resize(depth, 0.0);
        }
        let inside = std::mem::take(&mut nested[depth - 1]);
        if depth > 1 {
            nested[depth - 2] += duration;
        }
        inside
    })
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

/// The long-task threshold (Long Tasks, and LoAF's `blockingDuration` over it).
pub const LONG_TASK_MS: f64 = 50.0;

/// Whether some ENABLED module needs [`task_finished`] for tasks shorter than
/// [`LONG_TASK_MS`] too (a const, folded away while none does). `loaf` does: a
/// frame's scripts and duration are summed from every task between two
/// frames. `longtask` does not. `Browser::task_finished` returns before
/// touching any document for a shorter task when this is false, so
/// `longtask::on_task` only ever sees tasks of 50 ms or more and `loaf::on_task`
/// sees every task. (A module that needs more must flip this const in its own
/// commit.)
pub const TASK_BOOKKEEPING: bool = loaf::ENABLED;

/// A task of the event loop ended (`Browser::task_finished`).
pub fn task_finished(page: &Rc<PageDom>, task: &TaskInfo) {
    if longtask::ENABLED && task.end - task.start >= LONG_TASK_MS {
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

/// A trusted input event that was not made because nobody listens for its type
/// (`target::unheard`): what `input.rs` would have fired.
pub struct UnheardEvent<'a> {
    pub ty: &'a str,
    /// The `timeStamp` the event would have (document clock).
    pub time_stamp: f64,
    pub cancelable: bool,
    /// Where it would have been dispatched (the entry's `target`).
    pub target: pyro_dom::events::TargetKey,
    /// `KeyboardEvent.code` of key events (the `interactionId` key), else `None`.
    pub key_code: Option<&'a str>,
}

/// A trusted input event nobody listens for was skipped (`target::unheard_input`,
/// called from `input.rs` for mouse/pointer, key and input events): Event Timing
/// still counts it (`eventCounts`, `interactionCount`, entries over the duration
/// threshold, `first-input`), and the last-input time moves. Only called while
/// [`TRACK_INPUT`]; `event::note_unheard` only while `event::ENABLED`. The event
/// is not dispatched, so `dispatch_begin`/`dispatch_end` do not run for it: the
/// module makes `processingStart == processingEnd` (the time of the call).
pub(crate) fn note_unheard(page: &PageDom, event: &UnheardEvent<'_>) {
    note_input(page, event.ty, event.time_stamp);
    if event::ENABLED {
        event::note_unheard(page, event);
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
