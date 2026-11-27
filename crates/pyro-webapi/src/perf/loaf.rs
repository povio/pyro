//! Long Animation Frames (lane perf-6): `PerformanceLongAnimationFrameTiming`.
//!
//! A frame of a document starts at the first task in which its script ran after the previous
//! frame ended (`on_task`, with the engine's own record of which documents' realms were entered:
//! `longtask::ran_script`) and ends with the paint time of the rendering update that follows
//! (`on_frame`). A frame that took more than 50 ms is an entry:
//! - `startTime`/`duration`: from that first task's start to the end of the update's style and
//!   layout (`FrameInfo::paint_time`), so `paintTime` is `startTime + duration`; `presentationTime`
//!   is the end of the update (`FrameInfo::presented`: nothing is rasterized);
//! - `renderStart` and `styleAndLayoutStart`: where the update began and where its animation frame
//!   callbacks ended (`FrameInfo`);
//! - `blockingDuration`: the sum of what each task before the update ran over 50 ms (its own time,
//!   less any nested event loop's), with the update's own time added to the longest task first
//!   (a task that ran the update blocked as long as the update did);
//! - `firstUIEventTimestamp`: the `timeStamp` of the first pointer, mouse, key or input event of
//!   the frame, 0 when none.
//! A long task asks for the update (`timeline::request_rendering`): a task that dirtied nothing
//! still ends its frame at the next rendering opportunity, as the spec has it. A frame whose
//! first task is more than [`GAP_MS`] before the update began is an earlier frame that never got
//! to render (it was short): the update starts a new one.
//!
//! `scripts` is always empty: the engine has no hook that tells which function a task, a
//! callback or a listener ran (`PerformanceScriptTiming` needs the invoker, source URL and
//! function), and an invoker or duration made up from the event target alone would be a guess.
//! `forcedStyleAndLayoutDuration` and `pauseDuration` hang off those entries, so they are absent
//! too. Frames of invisible documents are not told apart (the engine has no visibility notion
//! here), and an event loop spun by a nested task counts as a part of its outer task.
//!
//! Cost with the module enabled and no observer: per task, per document, one clock-free check of
//! whether the document's realms were entered (a thread-local list lookup); a task that ran script
//! then updates five numbers; nothing allocates until a frame is long.

use std::cell::RefCell;
use std::rc::Rc;

use pyro_js::{ClassDef, IdentityKey, JsClass, JsResult, Scope, Value};

use crate::page::PageDom;
use crate::performance::{PerformanceEntry, PerformanceEntryData};

use super::timeline::{self, TimelineEntry};
use super::{FrameInfo, LONG_TASK_MS};

/// Entry types this module makes real.
pub const ENTRY_TYPES: &[&str] = &["long-animation-frame"];
/// Entries come from the task hook, the frame hook and the engine's script entries.
pub const ENABLED: bool = true;

/// A first task this long (ms) before the update began started a frame that ended without one.
const GAP_MS: f64 = 10.0;

/// Hidden key of the cached empty `scripts` array on an entry's wrapper.
const SCRIPTS_KEY: &str = "lpLoafScripts";

#[derive(Default)]
struct Inner {
    /// Start of the first task of the open frame.
    frame_start: Option<f64>,
    /// End of the last task counted.
    last_end: f64,
    /// A frame closed here: the task that ran its update started before, and is no new frame.
    closed_until: f64,
    /// Sum over the open frame's tasks of what they ran over 50 ms, and the longest task.
    excess: f64,
    longest: f64,
    first_ui: Option<f64>,
    /// A rendering update was asked for the open frame.
    requested: bool,
}

impl Inner {
    fn discard(&mut self) {
        *self = Inner { closed_until: self.closed_until, last_end: self.last_end, ..Inner::default() };
    }
}

/// A document's Long Animation Frames state (`TimelineBuffers::loaf`).
#[derive(Default)]
pub struct LoafState {
    inner: RefCell<Inner>,
}

/// A trusted UI event (pointer, mouse, key, input) was processed at `time_stamp` (its `timeStamp`):
/// the open frame's `firstUIEventTimestamp` if it has none. Called from `perf/event.rs`.
pub(super) fn note_ui_event(page: &PageDom, time_stamp: f64) {
    page.timeline().loaf.inner.borrow_mut().first_ui.get_or_insert(time_stamp);
}

/// Whether `ty` is a UI event that Event Timing does not time but a frame notes.
pub(super) fn is_untimed_ui_event(ty: &str) -> bool {
    matches!(ty, "pointermove" | "mousemove")
}

#[derive(Clone, Copy)]
struct Frame {
    start: f64,
    duration: f64,
    render_start: f64,
    style_and_layout_start: f64,
    blocking: f64,
    first_ui: f64,
    presentation: f64,
}

impl TimelineEntry for Frame {
    fn entry_type(&self) -> &'static str {
        "long-animation-frame"
    }
    fn start_time(&self) -> f64 {
        self.start
    }
    fn duration(&self) -> f64 {
        self.duration
    }
    fn wrap<'s>(&self, s: &Scope<'s>, key: IdentityKey) -> JsResult<Value<'s>> {
        let base = PerformanceEntryData::new("long-animation-frame", "long-animation-frame", self.start, self.duration);
        Ok(s.wrap::<PerformanceLongAnimationFrameTiming>(FrameData { base, frame: *self }, Some(key))?.value())
    }
}

/// The payload of a `PerformanceLongAnimationFrameTiming` wrapper.
pub struct FrameData {
    base: PerformanceEntryData,
    frame: Frame,
}

pub struct PerformanceLongAnimationFrameTiming;

impl JsClass for PerformanceLongAnimationFrameTiming {
    const NAME: &'static str = "PerformanceLongAnimationFrameTiming";
    type Data = FrameData;
    type Parent = PerformanceEntry;

    fn define(def: &mut ClassDef<Self>) {
        def.upcast(|d: &FrameData| &d.base);
        def.getter("renderStart", |s, d| Ok(s.number(d.frame.render_start)));
        def.getter("styleAndLayoutStart", |s, d| Ok(s.number(d.frame.style_and_layout_start)));
        def.getter("blockingDuration", |s, d| Ok(s.number(d.frame.blocking)));
        def.getter("firstUIEventTimestamp", |s, d| Ok(s.number(d.frame.first_ui)));
        def.getter("paintTime", |s, d| Ok(s.number(d.frame.start + d.frame.duration)));
        def.getter("presentationTime", |s, d| Ok(s.number(d.frame.presentation)));
        def.getter_this("scripts", |s, _d, this| scripts(s, this));
        def.method_this("toJSON", 0, |s, d, this, _a| {
            let f = &d.frame;
            let o = s.object();
            o.set("name", s.string(&d.base.name))?;
            o.set("entryType", s.string(d.base.entry_type))?;
            o.set("startTime", s.number(f.start))?;
            o.set("duration", s.number(f.duration))?;
            o.set("renderStart", s.number(f.render_start))?;
            o.set("styleAndLayoutStart", s.number(f.style_and_layout_start))?;
            o.set("blockingDuration", s.number(f.blocking))?;
            o.set("firstUIEventTimestamp", s.number(f.first_ui))?;
            o.set("paintTime", s.number(f.start + f.duration))?;
            o.set("presentationTime", s.number(f.presentation))?;
            o.set("scripts", scripts(s, this)?)?;
            Ok(o.value())
        });
    }
}

/// `scripts`: the frozen empty array (one per wrapper, [SameObject]); see the module docs.
fn scripts<'s>(s: &Scope<'s>, this: pyro_js::Object<'s>) -> JsResult<Value<'s>> {
    if let Some(list) = s.get_hidden(this, SCRIPTS_KEY)?.filter(|v| !v.is_undefined()) {
        return Ok(list);
    }
    let list = s.frozen_array(&[])?;
    s.set_hidden(this, SCRIPTS_KEY, list)?;
    Ok(list)
}

/// Registers the module's classes (`JsClass`es); called once per engine when ENABLED.
pub fn register(engine: &pyro_js::Engine) {
    engine.register::<PerformanceLongAnimationFrameTiming>();
}

/// A task of the event loop ended; called when ENABLED, for every document.
pub fn on_task(page: &Rc<PageDom>, task: &super::TaskInfo) {
    // A nested task is inside its outer task, which carries its time (`own_duration`).
    if task.is_nested() || page.is_disposed() || !super::longtask::ran_script(page) {
        return;
    }
    let ask = {
        let mut st = page.timeline().loaf.inner.borrow_mut();
        // The task that ran a frame's update started before that frame closed.
        if task.start < st.closed_until {
            return;
        }
        if st.frame_start.is_some() && task.start - st.last_end > GAP_MS {
            st.discard();
        }
        let start = match st.frame_start {
            Some(start) => start,
            None => {
                // A UI event of this very task (noted while it ran) belongs to the new frame.
                st.first_ui = st.first_ui.filter(|t| *t >= task.start);
                st.frame_start = Some(task.start);
                task.start
            }
        };
        let own = task.own_duration();
        st.excess += (own - LONG_TASK_MS).max(0.0);
        st.longest = st.longest.max(own);
        st.last_end = task.end;
        let ask = !st.requested && task.end - start > LONG_TASK_MS;
        st.requested |= ask;
        ask
    };
    // A long frame ends at the next rendering update, whether or not anything else asks for one.
    if ask {
        timeline::request_rendering(page);
    }
}

/// The end of a rendering update of `page`'s document: closes the open frame.
pub fn on_frame(page: &Rc<PageDom>, frame: &FrameInfo) {
    let long = {
        let mut st = page.timeline().loaf.inner.borrow_mut();
        let carried = st.frame_start.filter(|_| frame.render_start - st.last_end <= GAP_MS);
        let render = (frame.paint_time - frame.render_start).max(0.0);
        let (start, blocking, first_ui) = match carried {
            Some(start) => {
                let longest_over = (st.longest - LONG_TASK_MS).max(0.0);
                let blocking = st.excess - longest_over + (st.longest + render - LONG_TASK_MS).max(0.0);
                (start, blocking, st.first_ui)
            }
            // No task before the update that ran script: the update alone is the frame.
            None => (frame.render_start, (render - LONG_TASK_MS).max(0.0), None),
        };
        st.discard();
        st.closed_until = frame.presented;
        let duration = (frame.paint_time - start).max(0.0);
        (duration > LONG_TASK_MS).then(|| Frame {
            start,
            duration,
            render_start: frame.render_start.max(start),
            style_and_layout_start: frame.style_and_layout_start.max(frame.render_start).max(start),
            blocking,
            first_ui: first_ui.unwrap_or(0.0),
            presentation: frame.presented,
        })
    };
    if let Some(long) = long {
        timeline::queue_entry(page, Rc::new(long));
    }
}

/// The document's `long-animation-frame` entries as `getEntries*()` lists them (Long Animation
/// Frames: they are on the performance timeline), as `(startTime, entry)` pairs; `name` filters.
pub fn timeline_entries<'s>(s: &Scope<'s>, page: &PageDom, name: Option<&str>) -> JsResult<Vec<(f64, Value<'s>)>> {
    if name.is_some_and(|n| n != "long-animation-frame") {
        return Ok(Vec::new());
    }
    super::event::buffered_entries(s, page, "long-animation-frame")
}
