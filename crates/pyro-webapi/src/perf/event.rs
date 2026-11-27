//! Event Timing (lane perf-5): `PerformanceEventTiming` (`event` and `first-input` entries),
//! `performance.eventCounts` and `performance.interactionCount`.
//!
//! What is measured, and where it comes from:
//! - `startTime` is the event's `timeStamp`; `processingStart` is the document's clock when
//!   `target.rs dispatch_at` is about to run the listeners ([`dispatch_begin`]) and
//!   `processingEnd` when they (and the microtask checkpoints after each) are done
//!   ([`dispatch_end`]). An event nobody listens for is not dispatched at all
//!   ([`note_unheard`]): its two times are the time of that call.
//! - `duration` is the time from `startTime` to the presentation of the first rendering update
//!   after the event was processed (`FrameInfo::presented`: the end of Pyro's own update, nothing
//!   is rasterized), rounded to the nearest 8 ms. Every processed event asks for such an update
//!   (`timeline::request_rendering`), so an entry never waits for a paint nothing else wants
//!   (`keydown-without-paint`).
//! - An entry is exposed (queued for observers) when its duration is at least 16 ms (an observer's
//!   `durationThreshold` is applied by the observer); the timeline buffer keeps those of at least
//!   104 ms, and the first 150 (core: `timeline::policy`). A `first-input` entry is made for the
//!   first `pointerdown` (once its `pointerup` shows it was no scroll or cancel) or `keydown`,
//!   whatever its duration.
//! - `interactionId` follows Event Timing's interaction rules: a `pointerdown` entry is held until
//!   its `pointerup` (or `contextmenu`) gives the pair an id, or a `pointercancel` ends it without
//!   one; `click`/`auxclick` take the id of their pointer's `pointerup`; a `keydown` starts an
//!   interaction that its `keypress` and `keyup` (same `code`) share; a keyboard-made `click`
//!   (`pointerId` -1) has the last key interaction's id. `performance.interactionCount` counts the
//!   ids made.
//! - `eventCounts` counts every trusted event of the exposed types once processed, observed or not.
//!
//! Cost with the module enabled and no observer: per trusted event of any type one `match` on its
//! type name (no allocation for the types that are not timed); per timed event two reads of the
//! document clock, one counter increment, one `Vec` push of an 80-byte record and one request for
//! a rendering update (a flag, and the frame timer when none is waiting); per frame that follows
//! one pass over the records. Nothing is wrapped or queued unless an observer watches `event`, the
//! duration reaches the 104 ms buffer rule, or the entry is the `first-input`.
//!
//! Not covered (the engine has no source for them): touch interactions (`touchstart` ids and the
//! tap rules: touch input is turned into mouse input before it gets here), `pointerType`-specific
//! first-input rules, composition interactions, drag interaction ids, and the unheard events' real
//! `pointerId` (the mouse's, 1, is assumed; an event nobody listens for is not made).

use std::cell::RefCell;
use std::rc::Rc;

use pyro_dom::events::TargetKey;
use pyro_js::{ClassDef, IdentityKey, JsClass, JsError, JsResult, Scope, Value};

use crate::event::EventData;
use crate::page::PageDom;
use crate::performance::{PerformanceEntry, PerformanceEntryData};

use super::timeline::{self, TimelineEntry};
use super::FrameInfo;

/// Entry types this module makes real.
pub const ENTRY_TYPES: &[&str] = &["event", "first-input"];
/// Every entry type, event timing, `eventCounts` and the hooks are real.
pub const ENABLED: bool = true;

/// The event types Event Timing times and counts, in the order `eventCounts` lists them.
const EVENT_TYPES: [&str; 36] = [
    "auxclick",
    "click",
    "contextmenu",
    "dblclick",
    "mousedown",
    "mouseenter",
    "mouseleave",
    "mouseout",
    "mouseover",
    "mouseup",
    "pointerover",
    "pointerenter",
    "pointerdown",
    "pointerup",
    "pointercancel",
    "pointerout",
    "pointerleave",
    "gotpointercapture",
    "lostpointercapture",
    "touchstart",
    "touchend",
    "touchcancel",
    "keydown",
    "keypress",
    "keyup",
    "beforeinput",
    "input",
    "compositionstart",
    "compositionupdate",
    "compositionend",
    "dragstart",
    "dragend",
    "dragenter",
    "dragleave",
    "dragover",
    "drop",
];
const N: usize = EVENT_TYPES.len();

const AUXCLICK: u8 = 0;
const CLICK: u8 = 1;
const CONTEXTMENU: u8 = 2;
const POINTERDOWN: u8 = 12;
const POINTERUP: u8 = 13;
const POINTERCANCEL: u8 = 14;
const KEYDOWN: u8 = 22;
const KEYPRESS: u8 = 23;
const KEYUP: u8 = 24;

/// Index of `ty` in [`EVENT_TYPES`]; `None` for the (many) types that are not timed.
fn type_index(ty: &str) -> Option<u8> {
    Some(match ty {
        "auxclick" => 0,
        "click" => 1,
        "contextmenu" => 2,
        "dblclick" => 3,
        "mousedown" => 4,
        "mouseenter" => 5,
        "mouseleave" => 6,
        "mouseout" => 7,
        "mouseover" => 8,
        "mouseup" => 9,
        "pointerover" => 10,
        "pointerenter" => 11,
        "pointerdown" => 12,
        "pointerup" => 13,
        "pointercancel" => 14,
        "pointerout" => 15,
        "pointerleave" => 16,
        "gotpointercapture" => 17,
        "lostpointercapture" => 18,
        "touchstart" => 19,
        "touchend" => 20,
        "touchcancel" => 21,
        "keydown" => 22,
        "keypress" => 23,
        "keyup" => 24,
        "beforeinput" => 25,
        "input" => 26,
        "compositionstart" => 27,
        "compositionupdate" => 28,
        "compositionend" => 29,
        "dragstart" => 30,
        "dragend" => 31,
        "dragenter" => 32,
        "dragleave" => 33,
        "dragover" => 34,
        "drop" => 35,
        _ => return None,
    })
}

/// Entries shorter than this are never exposed (the least `durationThreshold`).
const MIN_EXPOSED_MS: f64 = 16.0;
/// Entries of at least this duration are kept for `buffered` observers.
const BUFFER_MS: f64 = 104.0;
/// A `pointerdown` whose `pointerup` has not come in this long is exposed without an
/// `interactionId` and is no first input (a held button: Chrome flushes such entries too, the
/// delay is a choice).
const HOLD_MS: f64 = 1000.0;
/// Records kept for the next frame (a document that renders no frames: render-blocked).
const MAX_PENDING: usize = 512;

fn round8(ms: f64) -> f64 {
    ((ms.max(0.0) / 8.0).round()) * 8.0
}

/// One processed event that waits for its presentation.
struct Pending {
    seq: u64,
    ty: u8,
    start: f64,
    processing_start: f64,
    processing_end: f64,
    cancelable: bool,
    target: Option<TargetKey>,
    interaction_id: u32,
    /// Set by the first frame after the event.
    duration: Option<f64>,
    /// A `pointerdown` waiting for its `pointerup`, `contextmenu` or `pointercancel`.
    held: bool,
    first_input_eligible: bool,
}

struct Inner {
    counts: [u32; N],
    pending: Vec<Pending>,
    next_seq: u64,
    /// Interactions made so far (`interactionCount`; the ids are 1, 2, ...).
    interactions: u32,
    /// `pointerdown`s waiting for their interaction: (pointer id, record).
    down: Vec<(i32, u64)>,
    /// `pointerdown`s whose interaction a `contextmenu` started: (pointer id, id).
    assigned: Vec<(i32, u32)>,
    /// The interaction the next `click` of a pointer belongs to.
    clicks: Vec<(i32, u32)>,
    /// Keys pressed and not released: (`code`, id).
    keys: Vec<(Box<str>, u32)>,
    last_key: u32,
    first_input_done: bool,
}

/// A document's Event Timing state (`TimelineBuffers::event`).
pub struct EventTimingState {
    inner: RefCell<Inner>,
}

impl Default for EventTimingState {
    fn default() -> Self {
        EventTimingState {
            inner: RefCell::new(Inner {
                counts: [0; N],
                pending: Vec::new(),
                next_seq: 0,
                interactions: 0,
                down: Vec::new(),
                assigned: Vec::new(),
                clicks: Vec::new(),
                keys: Vec::new(),
                last_key: 0,
                first_input_done: false,
            }),
        }
    }
}

fn bounded<T>(list: &mut Vec<T>) {
    if list.len() > 16 {
        list.remove(0);
    }
}

impl Inner {
    fn pending_mut(&mut self, seq: u64) -> Option<&mut Pending> {
        self.pending.iter_mut().find(|p| p.seq == seq)
    }

    fn new_interaction(&mut self) -> u32 {
        self.interactions += 1;
        self.interactions
    }

    /// Ends the wait of a held `pointerdown`.
    fn release(&mut self, seq: u64, id: u32, eligible: bool) {
        if let Some(p) = self.pending_mut(seq) {
            p.held = false;
            if id != 0 {
                p.interaction_id = id;
            }
            p.first_input_eligible &= eligible;
        }
    }

    /// The interaction rules for an event of type `ty` (index) recorded as `seq`: its
    /// `interactionId`, and whether it waits (a `pointerdown`).
    fn interaction(&mut self, ty: u8, pointer: i32, code: Option<&str>, seq: u64) -> (u32, bool) {
        match ty {
            POINTERDOWN => {
                if let Some(at) = self.down.iter().position(|d| d.0 == pointer) {
                    // A second `pointerdown` without a `pointerup`: the first one is no interaction.
                    let (_, old) = self.down.remove(at);
                    self.release(old, 0, false);
                }
                self.down.push((pointer, seq));
                (0, true)
            }
            POINTERUP => {
                let id = if let Some(at) = self.assigned.iter().position(|a| a.0 == pointer) {
                    self.assigned.remove(at).1
                } else if let Some(at) = self.down.iter().position(|d| d.0 == pointer) {
                    let (_, down) = self.down.remove(at);
                    let id = self.new_interaction();
                    self.release(down, id, true);
                    id
                } else {
                    // A `pointerup` without a `pointerdown` is no interaction.
                    0
                };
                if id != 0 {
                    self.clicks.push((pointer, id));
                    bounded(&mut self.clicks);
                }
                (id, false)
            }
            POINTERCANCEL => {
                if let Some(at) = self.down.iter().position(|d| d.0 == pointer) {
                    let (_, down) = self.down.remove(at);
                    self.release(down, 0, false);
                }
                self.clicks.retain(|c| c.0 != pointer);
                (0, false)
            }
            CONTEXTMENU => {
                if let Some(at) = self.down.iter().position(|d| d.0 == pointer) {
                    // The menu opened on the press: the interaction is the press so far.
                    let (_, down) = self.down.remove(at);
                    let id = self.new_interaction();
                    self.release(down, id, true);
                    self.assigned.push((pointer, id));
                    bounded(&mut self.assigned);
                    (id, false)
                } else {
                    (self.clicks.iter().find(|c| c.0 == pointer).map_or(0, |c| c.1), false)
                }
            }
            CLICK | AUXCLICK => {
                if pointer == -1 {
                    // A click a key made (Enter on a link, Space on a button).
                    (self.last_key, false)
                } else if let Some(at) = self.clicks.iter().position(|c| c.0 == pointer) {
                    (self.clicks.remove(at).1, false)
                } else {
                    (0, false)
                }
            }
            KEYDOWN => {
                let id = self.new_interaction();
                let code: Box<str> = code.unwrap_or("").into();
                self.keys.retain(|k| k.0 != code);
                self.keys.push((code, id));
                bounded(&mut self.keys);
                self.last_key = id;
                (id, false)
            }
            KEYPRESS => (if self.keys.is_empty() { 0 } else { self.last_key }, false),
            KEYUP => {
                let code = code.unwrap_or("");
                match self.keys.iter().position(|k| &*k.0 == code) {
                    Some(at) => (self.keys.remove(at).1, false),
                    None => (0, false),
                }
            }
            _ => (0, false),
        }
    }
}

/// An event as `record` takes it.
struct Raw<'a> {
    ty: u8,
    start: f64,
    processing_start: f64,
    processing_end: f64,
    cancelable: bool,
    target: Option<TargetKey>,
    pointer: i32,
    code: Option<&'a str>,
}

fn record(page: &PageDom, raw: Raw<'_>) {
    if super::loaf::ENABLED {
        super::loaf::note_ui_event(page, raw.start);
    }
    {
        let mut guard = page.timeline().event.inner.borrow_mut();
        let inner = &mut *guard;
        inner.counts[raw.ty as usize] += 1;
        let seq = inner.next_seq;
        inner.next_seq += 1;
        let (interaction_id, held) = inner.interaction(raw.ty, raw.pointer, raw.code, seq);
        if inner.pending.len() >= MAX_PENDING {
            inner.pending.remove(0);
        }
        inner.pending.push(Pending {
            seq,
            ty: raw.ty,
            start: raw.start,
            processing_start: raw.processing_start,
            processing_end: raw.processing_end,
            cancelable: raw.cancelable,
            target: raw.target,
            interaction_id,
            duration: None,
            held,
            first_input_eligible: true,
        });
    }
    // The entry's duration ends at the next presentation. Ask for one only when somebody can see the
    // entry (an observer, or the first-input entry that is always buffered); otherwise the next
    // frame the page renders anyway closes it, and an idle page is not made to render per input.
    let wanted = {
        let timeline = page.timeline();
        timeline.is_observed("event") || timeline.is_observed("first-input") || !timeline.event.inner.borrow().first_input_done
    };
    if wanted {
        timeline::request_rendering(page);
    }
}

/// What an exposed entry shares between its observers' wrappers.
pub struct EventEntry {
    name: &'static str,
    start: f64,
    duration: f64,
    processing_start: f64,
    processing_end: f64,
    cancelable: bool,
    interaction_id: u32,
    target: Option<TargetKey>,
}

/// An `event` or `first-input` entry on the timeline.
struct Timed {
    entry: Rc<EventEntry>,
    first_input: bool,
}

impl TimelineEntry for Timed {
    fn entry_type(&self) -> &'static str {
        if self.first_input {
            "first-input"
        } else {
            "event"
        }
    }
    fn start_time(&self) -> f64 {
        self.entry.start
    }
    fn duration(&self) -> f64 {
        self.entry.duration
    }
    fn wrap<'s>(&self, s: &Scope<'s>, key: IdentityKey) -> JsResult<Value<'s>> {
        let e = &self.entry;
        let base = PerformanceEntryData::new(e.name, self.entry_type(), e.start, e.duration);
        Ok(s.wrap::<PerformanceEventTiming>(EventTimingData { base, entry: e.clone() }, Some(key))?.value())
    }
}

/// The payload of a `PerformanceEventTiming` wrapper.
pub struct EventTimingData {
    base: PerformanceEntryData,
    entry: Rc<EventEntry>,
}

pub struct PerformanceEventTiming;

impl JsClass for PerformanceEventTiming {
    const NAME: &'static str = "PerformanceEventTiming";
    type Data = EventTimingData;
    type Parent = PerformanceEntry;

    fn define(def: &mut ClassDef<Self>) {
        def.upcast(|d: &EventTimingData| &d.base);
        def.getter("processingStart", |s, d| Ok(s.number(d.entry.processing_start)));
        def.getter("processingEnd", |s, d| Ok(s.number(d.entry.processing_end)));
        def.getter("cancelable", |s, d| Ok(s.boolean(d.entry.cancelable)));
        def.getter("interactionId", |s, d| Ok(s.number(f64::from(d.entry.interaction_id))));
        // The node the event was dispatched at, as the document sees it (a shadow tree's host);
        // null once the node is gone or no longer connected, and for a window target.
        def.getter("target", |s, d| {
            let Some(key) = d.entry.target else { return Ok(s.null()) };
            let page = crate::target::page_of(s)?;
            let live = {
                let dom = page.dom.borrow();
                dom.key_node(key).is_some_and(|id| dom.is_connected(id).unwrap_or(false))
            };
            if !live {
                return Ok(s.null());
            }
            crate::target::target_value(s, &page, Some(key))
        });
        def.method("toJSON", 0, |s, d, _a| {
            let e = &d.entry;
            let o = s.object();
            o.set("name", s.string(e.name))?;
            o.set("entryType", s.string(d.base.entry_type))?;
            o.set("startTime", s.number(e.start))?;
            o.set("duration", s.number(e.duration))?;
            o.set("processingStart", s.number(e.processing_start))?;
            o.set("processingEnd", s.number(e.processing_end))?;
            o.set("cancelable", s.boolean(e.cancelable))?;
            o.set("interactionId", s.number(f64::from(e.interaction_id)))?;
            Ok(o.value())
        });
    }
}

/// Identity of a realm's `performance.eventCounts` ([SameObject]).
const KEY_COUNTS: u64 = 0x5065_7245_7643_6e74;
/// Hidden key of the iterator factory on an `EventCounts` wrapper.
const ITER_KEY: &str = "lpEventCountsIter";

pub struct EventCountsData {
    page: Rc<PageDom>,
}

/// `performance.eventCounts`: a read-only maplike of the exposed event types to the number of
/// trusted events of each that were processed. Its iterators read the counts when stepped.
pub struct EventCounts;

impl JsClass for EventCounts {
    const NAME: &'static str = "EventCounts";
    type Data = EventCountsData;
    type Parent = pyro_js::NoParent;

    fn define(def: &mut ClassDef<Self>) {
        def.getter("size", |s, _d| Ok(s.number(N as f64)));
        def.method("get", 1, |s, d, a| {
            a.require(1, "EventCounts.get")?;
            let key = a.get(0).to_string()?;
            Ok(match type_index(&key) {
                Some(i) => s.number(f64::from(d.page.timeline().event.inner.borrow().counts[i as usize])),
                None => s.undefined(),
            })
        });
        def.method("has", 1, |s, _d, a| {
            a.require(1, "EventCounts.has")?;
            let key = a.get(0).to_string()?;
            Ok(s.boolean(type_index(&key).is_some()))
        });
        def.method_this("forEach", 1, |s, d, this, a| {
            a.require(1, "EventCounts.forEach")?;
            let Some(callback) = a.get(0).as_object().filter(|f| f.is_function()) else {
                return Err(JsError::Type("Failed to execute 'forEach' on 'EventCounts': The callback provided as parameter 1 is not a function.".into()));
            };
            let this_arg = a.get(1);
            for (i, name) in EVENT_TYPES.iter().enumerate() {
                let count = d.page.timeline().event.inner.borrow().counts[i];
                callback.call_with_this_value(this_arg, &[s.number(f64::from(count)), s.string(name), this.value()])?;
            }
            Ok(s.undefined())
        });
        def.method_this("entries", 0, |s, _d, this, _a| iterator(s, this, 2));
        def.method_this("keys", 0, |s, _d, this, _a| iterator(s, this, 0));
        def.method_this("values", 0, |s, _d, this, _a| iterator(s, this, 1));
    }
}

/// A live iterator over the map (`kind`: 0 keys, 1 values, 2 entries).
fn iterator<'s>(s: &Scope<'s>, this: pyro_js::Object<'s>, kind: u32) -> JsResult<Value<'s>> {
    let factory = s
        .get_hidden(this, ITER_KEY)?
        .and_then(|v| v.as_object())
        .ok_or_else(|| JsError::Type("Illegal invocation".into()))?;
    factory.call(None, &[s.number(f64::from(kind))])
}

/// Makes an iterator: `count(i)` reads the live count of type `i`; `map` is the `EventCounts`
/// object, whose prototype gets `@@iterator` (the `entries` function) on first use.
const ITERATOR_SOURCE: &str = r#"(function (count, keys, map) {
  'use strict';
  const O = Object;
  const IteratorPrototype = O.getPrototypeOf(O.getPrototypeOf([][Symbol.iterator]()));
  const state = new WeakMap();
  const proto = O.create(IteratorPrototype);
  O.defineProperty(proto, 'next', {
    value: function next() {
      const s = state.get(this);
      if (s === undefined) throw new TypeError('Illegal invocation');
      if (s.i >= keys.length) return { value: undefined, done: true };
      const i = s.i++;
      const value = s.kind === 0 ? keys[i] : s.kind === 1 ? count(i) : [keys[i], count(i)];
      return { value, done: false };
    },
    writable: true, configurable: true, enumerable: true,
  });
  O.defineProperty(proto, Symbol.toStringTag, { value: 'EventCounts Iterator', configurable: true });
  const prototype = O.getPrototypeOf(map);
  if (!O.hasOwn(prototype, Symbol.iterator)) {
    O.defineProperty(prototype, Symbol.iterator, { value: prototype.entries, writable: true, configurable: true });
  }
  return (kind) => {
    const it = O.create(proto);
    state.set(it, { i: 0, kind });
    return it;
  };
})"#;

/// Registers the module's classes (`JsClass`es); called once per engine when ENABLED.
pub fn register(engine: &pyro_js::Engine) {
    engine.register::<PerformanceEventTiming>();
    engine.register::<EventCounts>();
}

/// The end of a rendering update of `page`'s document: the entries of the events processed since
/// the last one get their duration (to this update's presentation) and are exposed.
pub fn on_frame(page: &Rc<PageDom>, frame: &FrameInfo) {
    let (ready, more) = {
        let mut guard = page.timeline().event.inner.borrow_mut();
        let inner = &mut *guard;
        if inner.pending.is_empty() {
            return;
        }
        for p in &mut inner.pending {
            if p.duration.is_none() {
                p.duration = Some(round8(frame.presented - p.start));
            }
            if p.held && frame.presented - p.start >= HOLD_MS {
                p.held = false;
                p.first_input_eligible = false;
            }
        }
        let pending = &inner.pending;
        inner.down.retain(|(_, seq)| pending.iter().any(|p| p.seq == *seq && p.held));
        let mut ready = Vec::new();
        let mut kept = Vec::new();
        for p in std::mem::take(&mut inner.pending) {
            if p.held {
                kept.push(p);
            } else {
                ready.push(p);
            }
        }
        inner.pending = kept;
        let more = !inner.pending.is_empty();
        let mut out = Vec::with_capacity(ready.len());
        for p in ready {
            let first = !inner.first_input_done && p.first_input_eligible && matches!(p.ty, POINTERDOWN | KEYDOWN);
            inner.first_input_done |= first;
            out.push((p, first));
        }
        (out, more)
    };
    // No state borrowed: queueing wraps entries and runs the observers' queue functions.
    for (p, first) in ready {
        expose(page, &p, first);
    }
    // Held `pointerdown`s still wait: look again (until their `pointerup` or the hold ends).
    if more {
        timeline::request_rendering(page);
    }
}

fn expose(page: &PageDom, p: &Pending, first: bool) {
    let duration = p.duration.unwrap_or(0.0);
    let entry = Rc::new(EventEntry {
        name: EVENT_TYPES[p.ty as usize],
        start: p.start,
        duration,
        processing_start: p.processing_start,
        processing_end: p.processing_end,
        cancelable: p.cancelable,
        interaction_id: p.interaction_id,
        target: p.target,
    });
    if first {
        timeline::queue_entry(page, Rc::new(Timed { entry: entry.clone(), first_input: true }));
    }
    if duration >= MIN_EXPOSED_MS && (duration >= BUFFER_MS || page.timeline().is_observed("event")) {
        timeline::queue_entry(page, Rc::new(Timed { entry, first_input: false }));
    }
}

/// What [`dispatch_begin`] hands [`dispatch_end`].
pub struct DispatchToken {
    ty: u8,
    start: f64,
    processing_start: f64,
}

/// A trusted event is about to run its listeners; called when ENABLED.
pub fn dispatch_begin(page: &PageDom, event: &EventData) -> Option<DispatchToken> {
    let (ty, start) = {
        let state = event.state.borrow();
        match type_index(&state.ty) {
            Some(ty) => (ty, state.time_stamp),
            None => {
                // Not timed, but a UI event a long animation frame notes (`firstUIEventTimestamp`).
                if super::loaf::ENABLED && super::loaf::is_untimed_ui_event(&state.ty) {
                    super::loaf::note_ui_event(page, state.time_stamp);
                }
                return None;
            }
        }
    };
    Some(DispatchToken { ty, start, processing_start: crate::event::page_now_ms(page) })
}

/// The listeners of the event ran; called when ENABLED.
pub fn dispatch_end(page: &PageDom, event: &EventData, token: DispatchToken) {
    let processing_end = crate::event::page_now_ms(page);
    // After the dispatch the event's target is what the document sees: a shadow tree's host, or
    // nothing when the event never left the shadow tree ("clearTargets").
    let (target, cancelable) = {
        let state = event.state.borrow();
        (state.target, state.cancelable)
    };
    let pointer = if event.iface == crate::event::EventInterface::PointerEvent { event.fields.borrow().pointer_id } else { 1 };
    let keyed = matches!(token.ty, KEYDOWN | KEYPRESS | KEYUP);
    let fields = keyed.then(|| event.fields.borrow());
    let code = fields.as_ref().map(|f| f.code.as_str());
    record(
        page,
        Raw { ty: token.ty, start: token.start, processing_start: token.processing_start, processing_end, cancelable, target, pointer, code },
    );
}

/// A trusted input event nobody listens for was not made (`perf::note_unheard`); called when
/// ENABLED. It counts like a dispatched one (`eventCounts`, entries, `interactionId`).
pub fn note_unheard(page: &PageDom, event: &super::UnheardEvent<'_>) {
    let Some(ty) = type_index(event.ty) else {
        if super::loaf::ENABLED && super::loaf::is_untimed_ui_event(event.ty) {
            super::loaf::note_ui_event(page, event.time_stamp);
        }
        return;
    };
    let now = crate::event::page_now_ms(page);
    // Nobody listens, so the event never leaves a shadow tree: report the host.
    let target = {
        let dom = page.dom.borrow();
        let mut key = event.target;
        while let Some(mut id) = dom.key_node(key) {
            let Ok(root) = dom.root_node(id, false) else { break };
            if !dom.is_shadow_root(root).unwrap_or(false) {
                break;
            }
            match dom.fragment_host(root) {
                Ok(Some(host)) => id = host,
                _ => break,
            }
            match dom.node_key(id) {
                Some(k) => key = k,
                None => break,
            }
        }
        Some(key)
    };
    record(
        page,
        Raw {
            ty,
            start: event.time_stamp,
            processing_start: now,
            processing_end: now,
            cancelable: event.cancelable,
            target,
            pointer: 1,
            code: event.key_code,
        },
    );
}

/// `performance.eventCounts` (an `EventCounts` object of `s`'s realm); read when ENABLED.
pub fn event_counts<'s>(s: &Scope<'s>, _page: &PageDom) -> JsResult<Value<'s>> {
    let key = IdentityKey(KEY_COUNTS, 0);
    if let Some(o) = s.lookup_identity(key) {
        return Ok(o.value());
    }
    let page = crate::target::page_of(s)?;
    let map = s.wrap::<EventCounts>(EventCountsData { page: page.clone() }, Some(key))?;
    let weak = Rc::downgrade(&page);
    let count = s.function("count", move |s, _this, a| {
        let Some(page) = weak.upgrade() else { return Ok(s.number(0.0)) };
        let i = a.get(0).to_number()? as usize;
        let n = page.timeline().event.inner.borrow().counts.get(i).copied().unwrap_or(0);
        Ok(s.number(f64::from(n)))
    })?;
    let names: Vec<Value<'s>> = EVENT_TYPES.iter().map(|t| s.string(t)).collect();
    let names = s.array(&names)?;
    let make = s.eval(ITERATOR_SOURCE, Some("pyro://internal/event-counts.js"), 1)?;
    let make = make.as_object().ok_or_else(|| JsError::Type("EventCounts is unavailable".into()))?;
    let factory = make.call(None, &[count.value(), names.value(), map.value()])?;
    s.set_hidden(map, ITER_KEY, factory)?;
    Ok(map.value())
}

/// `performance.interactionCount`; read when ENABLED.
pub fn interaction_count(page: &PageDom) -> u32 {
    page.timeline().event.inner.borrow().interactions
}

/// The entries of `entry_type` buffered in the document's timeline, as `(startTime, entry)` pairs:
/// what `getEntries*()` lists of the types that are on the performance timeline besides User
/// Timing, Resource Timing and Paint Timing (`first-input`, `long-animation-frame`).
pub(super) fn buffered_entries<'s>(s: &Scope<'s>, page: &PageDom, entry_type: &str) -> JsResult<Vec<(f64, Value<'s>)>> {
    let list = page.timeline().buffered(s, entry_type, false)?;
    let mut out = Vec::new();
    let Some(list) = list.as_object() else { return Ok(out) };
    let len = list.get("length")?.to_number()? as u32;
    for i in 0..len {
        let entry = list.get_index(i)?;
        let Some(object) = entry.as_object() else { continue };
        out.push((object.get("startTime")?.to_number()?, entry));
    }
    Ok(out)
}

/// The document's `first-input` entry as `getEntries*()` lists it (Event Timing: the one entry
/// of that type is on the performance timeline); `name` filters.
pub fn first_input_entries<'s>(s: &Scope<'s>, page: &PageDom, name: Option<&str>) -> JsResult<Vec<(f64, Value<'s>)>> {
    let mut entries = buffered_entries(s, page, "first-input")?;
    if let Some(name) = name {
        entries.retain(|(_, entry)| {
            entry.as_object().is_some_and(|o| o.get("name").and_then(|v| v.to_string()).is_ok_and(|n| n == name))
        });
    }
    Ok(entries)
}
