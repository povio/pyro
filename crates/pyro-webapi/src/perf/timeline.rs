//! Per-document performance timeline plumbing shared by the entry types that
//! only PerformanceObservers see (`layout-shift`, `largest-contentful-paint`,
//! `element`, `event`, `first-input`, `longtask`, `long-animation-frame`,
//! `soft-navigation`, ...), and the paint entries reported from frames.
//!
//! A module creates an entry (any type implementing [`TimelineEntry`]) and
//! calls [`queue_entry`]: that is the Performance Timeline "queue a
//! PerformanceEntry": every observer (in every realm of the document) that
//! observes the entry's type gets it queued, and the entry is added to the
//! type's buffer unless the buffer is full (then it counts as dropped). It
//! is never added to `performance.getEntries*()`: those types are not in it.
//! `observe({type, buffered: true})` reads the buffer through
//! [`TimelineBuffers::buffered`].

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use pyro_js::{IdentityKey, JsResult, Persistent, RealmId, Scope, Value, WeakRealm};
use rustc_hash::FxHashMap;

use crate::page::PageDom;
use crate::performance::{PerformanceEntryData, PerformancePaintTiming};

/// First half of the identity key of every wrapper [`queue_entry`] makes
/// (second half: the entry's sequence number in the document).
const ENTRY_KEY: u64 = 0x5065_7254_6c6e_0000;
/// First half of the identity key of a paint entry's wrapper (second half:
/// its slot, see [`wrap_paint`]).
const PAINT_KEY: u64 = 0x5065_7254_5061_6e74;

/// An entry a module queues. The object is made per observing realm (and per
/// `buffered` read), so it must be cheap to wrap from shared Rust data
/// (`Rc` the payload).
pub trait TimelineEntry {
    /// `entryType` (one of the module's `ENTRY_TYPES`).
    fn entry_type(&self) -> &'static str;
    /// `startTime`: observers sort their lists by it.
    fn start_time(&self) -> f64;
    /// `duration`: `durationThreshold` filtering of `event` entries and the
    /// buffer rule of `event` (>= 104 ms) read it.
    fn duration(&self) -> f64 {
        0.0
    }
    /// The entry belongs to a soft navigation interval: observers that did
    /// not ask for `includeSoftNavigationObservations` skip it.
    fn soft_navigation(&self) -> bool {
        false
    }
    /// The entry's JS object in `s`'s realm. Pass `Some(key)` to
    /// `s.wrap::<Class>(data, Some(key))`: the same entry is then one object
    /// per realm while script holds it (an entry from an observer callback
    /// is `===` the one `buffered: true` delivers).
    fn wrap<'s>(&self, s: &Scope<'s>, key: IdentityKey) -> JsResult<Value<'s>>;
}

/// Buffer rule of an entry type; `None`: not kept here (User Timing,
/// resource, navigation and paint entries live elsewhere).
struct Policy {
    limit: usize,
    /// Entries with a shorter duration are not buffered (not dropped).
    min_duration: f64,
}

fn policy(entry_type: &str) -> Option<Policy> {
    Some(match entry_type {
        "mark" | "measure" | "resource" | "navigation" | "paint" => return None,
        "first-input" => Policy { limit: 1, min_duration: 0.0 },
        "event" => Policy { limit: 150, min_duration: 104.0 },
        "longtask" | "long-animation-frame" => Policy { limit: 200, min_duration: 0.0 },
        // element, largest-contentful-paint, layout-shift, soft-navigation, ...
        _ => Policy { limit: 150, min_duration: 0.0 },
    })
}

#[derive(Default)]
struct TypeBuffer {
    entries: Vec<(u64, Rc<dyn TimelineEntry>)>,
    dropped: u32,
}

struct Watcher {
    realm: RealmId,
    weak: WeakRealm,
    queue: Rc<Persistent>,
}

/// A `first-paint` / `first-contentful-paint` entry.
#[derive(Clone, Copy)]
pub struct PaintRecord {
    pub name: &'static str,
    /// `startTime`: the presentation time when there is one, else the paint time.
    pub start: f64,
    /// `paintTime`.
    pub paint_time: f64,
    /// `presentationTime`; `None` reads as null (`paintTime` is then `startTime`).
    pub presentation_time: Option<f64>,
}

/// Per-document timeline state (`PageDom::timeline`).
#[derive(Default)]
pub struct TimelineBuffers {
    buffers: RefCell<FxHashMap<&'static str, TypeBuffer>>,
    next_seq: Cell<u64>,
    /// The observer queue function of each observing realm.
    watchers: RefCell<Vec<Watcher>>,
    /// Entry types observed in each realm (performance_observer.js reports them).
    observed: RefCell<FxHashMap<RealmId, Vec<String>>>,
    /// Paint entries modules reported (`report_paint`).
    paint: RefCell<Vec<PaintRecord>>,
    wants_rendering: Cell<bool>,
    /// Document-clock time of the last trusted user input that Layout
    /// Instability (`hadRecentInput`, `lastInputTime`) and LCP (stop at the
    /// first input) count; see `perf::note_input`.
    last_input: Cell<Option<f64>>,
    /// Document-clock time of the last trusted `wheel` (a scroll input:
    /// LCP stops at it, layout shifts do not count it as input).
    last_wheel: Cell<Option<f64>>,
    /// The shared painted-candidate scan's memo and identities (`perf/scan.rs`).
    pub(super) scan: RefCell<super::scan::ScanState>,
    /// Event Timing's counters, pending entries and interaction state (`perf/event.rs`).
    pub(super) event: super::event::EventTimingState,
    /// Long Animation Frames' open frame (`perf/loaf.rs`).
    pub(super) loaf: super::loaf::LoafState,
}

impl TimelineBuffers {
    /// `natives.watch(queue)` of performance_observer.js: `queue` (or null
    /// to stop) gets the entries `queue_entry` makes for `s`'s realm.
    pub(crate) fn watch(&self, s: &Scope<'_>, queue: Value<'_>) {
        let id = s.realm_id();
        let mut watchers = self.watchers.borrow_mut();
        watchers.retain(|w| w.realm != id);
        if queue.as_object().is_some_and(|f| f.is_function()) {
            watchers.push(Watcher { realm: id, weak: s.realm().downgrade(), queue: Rc::new(Persistent::new(s, queue)) });
        }
    }

    /// The entry types the observers of `s`'s realm observe now.
    pub(crate) fn set_observed(&self, s: &Scope<'_>, types: Vec<String>) {
        let id = s.realm_id();
        let mut observed = self.observed.borrow_mut();
        if types.is_empty() {
            observed.remove(&id);
        } else {
            observed.insert(id, types);
        }
    }

    /// Whether some observer of the document observes `entry_type` now.
    /// Modules ask it to skip work nobody can see.
    pub fn is_observed(&self, entry_type: &str) -> bool {
        self.observed.borrow().values().any(|types| types.iter().any(|t| t == entry_type))
    }

    /// The buffered entries of `entry_type` as `s`'s realm's objects, oldest
    /// first (`observe({type, buffered: true})`).
    pub fn buffered<'s>(&self, s: &Scope<'s>, entry_type: &str, include_soft_navigation: bool) -> JsResult<Value<'s>> {
        let entries = self.buffers.borrow().get(entry_type).map(|b| b.entries.clone()).unwrap_or_default();
        let mut values: Vec<Value<'s>> = Vec::new();
        for (seq, entry) in &entries {
            if include_soft_navigation || !entry.soft_navigation() {
                values.push(wrap_entry(s, *seq, entry)?);
            }
        }
        Ok(s.array(&values)?.value())
    }

    /// How many entries of `entry_type` did not fit the full buffer.
    pub fn dropped(&self, entry_type: &str) -> u32 {
        self.buffers.borrow().get(entry_type).map_or(0, |b| b.dropped)
    }

    /// Paint entries modules reported, in report order.
    pub fn paint_records(&self) -> Vec<PaintRecord> {
        self.paint.borrow().clone()
    }

    /// Whether a module asked for a rendering update that did not run yet.
    pub fn wants_rendering(&self) -> bool {
        self.wants_rendering.get()
    }

    /// The `timeStamp` of the last trusted user input (`mousedown`, `pointerdown`,
    /// `pointerup`, `keydown`, `touchstart`, `touchend`), `None` before the first.
    /// Maintained only while `layout-shift`, `largest-contentful-paint` or `event`
    /// is ENABLED (`perf::TRACK_INPUT`); always `None` otherwise.
    pub fn last_input_time(&self) -> Option<f64> {
        self.last_input.get()
    }

    /// The `timeStamp` of the last trusted `wheel` event (same gating).
    pub fn last_wheel_time(&self) -> Option<f64> {
        self.last_wheel.get()
    }

    /// Whether a trusted user input happened in `[now - window_ms, now]`
    /// (layout-shift `hadRecentInput` is `recent_input(shift_time, 500.0)`).
    pub fn recent_input(&self, now: f64, window_ms: f64) -> bool {
        self.last_input.get().is_some_and(|t| t <= now && now - t < window_ms)
    }

    pub(crate) fn set_last_input(&self, time: f64) {
        self.last_input.set(Some(time));
    }

    pub(crate) fn set_last_wheel(&self, time: f64) {
        self.last_wheel.set(Some(time));
    }

    pub(crate) fn clear_wants_rendering(&self) {
        self.wants_rendering.set(false);
    }
}

fn wrap_entry<'s>(s: &Scope<'s>, seq: u64, entry: &Rc<dyn TimelineEntry>) -> JsResult<Value<'s>> {
    let key = IdentityKey(ENTRY_KEY, seq);
    if let Some(o) = s.lookup_identity(key) {
        return Ok(o.value());
    }
    entry.wrap(s, key)
}

/// Performance Timeline "queue a PerformanceEntry" of `page`'s document:
/// queues `entry` for every observer of its type, then buffers it under the
/// type's rule (`first-input` 1 entry, `event` 150 entries of >= 104 ms,
/// `longtask` / `long-animation-frame` 200, the others 150; a full buffer
/// counts the entry as dropped). Observer callbacks run in a task later,
/// not here; what runs here is each realm's observer `queue` function
/// (performance_observer.js), which only appends to the observers' lists. So
/// the caller may hold `PageDom` cells (not the DOM's `borrow_mut`: wrapping
/// an entry can touch node wrappers).
pub fn queue_entry(page: &PageDom, entry: Rc<dyn TimelineEntry>) {
    let timeline = page.timeline();
    let entry_type = entry.entry_type();
    let seq = timeline.next_seq.get();
    timeline.next_seq.set(seq + 1);
    let soft = entry.soft_navigation();
    let targets: Vec<(WeakRealm, Rc<Persistent>)> = {
        let mut watchers = timeline.watchers.borrow_mut();
        watchers.retain(|w| w.weak.upgrade().is_some_and(|r| !r.is_disposed()));
        watchers.iter().map(|w| (w.weak.clone(), w.queue.clone())).collect()
    };
    for (realm, queue) in targets {
        let Some(realm) = realm.upgrade() else { continue };
        let Ok(s) = realm.scope() else { continue };
        let Some(f) = queue.get(&s).as_object() else { continue };
        let queued = wrap_entry(&s, seq, &entry)
            .and_then(|object| f.call(None, &[s.string(entry_type), object, s.boolean(soft)]));
        if let Err(e) = queued {
            s.report_exception(&e);
        }
    }
    if let Some(policy) = policy(entry_type) {
        if entry.duration() >= policy.min_duration {
            let mut buffers = timeline.buffers.borrow_mut();
            let buffer = buffers.entry(entry_type).or_default();
            if buffer.entries.len() < policy.limit {
                buffer.entries.push((seq, entry));
            } else {
                buffer.dropped = buffer.dropped.saturating_add(1);
            }
        }
    }
}

/// A paint entry's wrapper (`slot`: the entry's index among the document's
/// paint entries, so the observer's object is the `getEntries*()` one).
pub fn wrap_paint<'s>(s: &Scope<'s>, slot: u64, record: &PaintRecord) -> JsResult<Value<'s>> {
    let key = IdentityKey(PAINT_KEY, slot);
    if let Some(o) = s.lookup_identity(key) {
        return Ok(o.value());
    }
    let mut data = PerformanceEntryData::new(record.name, "paint", record.start, 0.0);
    data.paint_time = Some(record.paint_time);
    data.presentation_time = record.presentation_time;
    Ok(s.wrap::<PerformancePaintTiming>(data, Some(key))?.value())
}

struct PaintEntry {
    slot: u64,
    record: PaintRecord,
}

impl TimelineEntry for PaintEntry {
    fn entry_type(&self) -> &'static str {
        "paint"
    }
    fn start_time(&self) -> f64 {
        self.record.start
    }
    fn wrap<'s>(&self, s: &Scope<'s>, _key: IdentityKey) -> JsResult<Value<'s>> {
        wrap_paint(s, self.slot, &self.record)
    }
}

/// Adds a paint entry to the document's timeline (`getEntries*()` lists it,
/// sorted by `startTime`) and queues it for the observers of `paint`.
pub fn report_paint(page: &PageDom, record: PaintRecord) {
    let slot = {
        let mut paint = page.timeline().paint.borrow_mut();
        paint.push(record);
        paint.len() as u64 - 1
    };
    queue_entry(page, Rc::new(PaintEntry { slot, record }));
}

/// Asks for a rendering update (an animation frame of the document's
/// window that need not run rAF callbacks): [`super::rendering_update`]
/// runs at its end. One request is one frame; a module that still has work
/// after its `on_frame` asks again from it. Does not consume the
/// client-activity eager frames of `window.rs`.
pub fn request_rendering(page: &PageDom) {
    page.timeline().wants_rendering.set(true);
    if let Some(hooks) = page.hooks() {
        hooks.request_rendering_frame();
    }
}
