//! Long Tasks (lane perf-6): `PerformanceLongTaskTiming` and `TaskAttributionTiming`.
//!
//! A task of the event loop that took 50 ms or more (the task hook's own start and end, the
//! document's clock; `perf::task_finished` filters the threshold) is an entry of every document
//! whose script ran in it. Which documents those are comes from the engine: a document's realm
//! was entered from native code since the task began ([`ran_script`]: `pyro_js::reset_entered` at
//! each outermost task's boundaries). The task is the document's own: `name` is `self` and the one
//! `TaskAttributionTiming` has container type `window` and no container id, name or source (what
//! the spec gives a task attributed to the observer's own window). A document that did not run
//! script in the task gets nothing: whether a script of another frame (the
//! `same-origin-descendant` and `-ancestor` names, with their container) or of another window ran
//! is not something the engine tells at this hook, so those entries are not made.
//!
//! A task a nested event loop ran inside another task (a sync request, `alert`) is no task of its
//! own: only the outer one counts, with the whole time it blocked the document
//! (`spin-eventloop-not-longtask`, `longtask-sync-xhr`).
//!
//! The entry's `duration` is the task's length in whole milliseconds (Chrome rounds it so). Cost
//! with the module enabled and no observer: nothing per task shorter than 50 ms (the hook returns
//! before any document is visited unless `loaf` keeps books); per long task and document that ran
//! script one small allocation and the buffer push (`buffered` observers read them later).

use std::rc::Rc;

use pyro_js::{ClassDef, IdentityKey, JsClass, JsResult, Scope, Value};

use crate::page::PageDom;
use crate::performance::{PerformanceEntry, PerformanceEntryData};

use super::timeline::{self, TimelineEntry};

/// Entry types this module makes real.
pub const ENTRY_TYPES: &[&str] = &["longtask"];
/// Entries come from the event loop's task hook and the engine's script entries.
pub const ENABLED: bool = true;

/// Hidden key of the cached `attribution` array on an entry's wrapper.
const ATTRIBUTION_KEY: &str = "lpLongTaskAttribution";

/// Whether script of `page`'s document ran in the task being reported (some realm of the document
/// was entered from native code since the task began).
pub(super) fn ran_script(page: &PageDom) -> bool {
    page.realms.borrow().values().any(|realm| realm.upgrade().is_some_and(|realm| realm.entered_since_reset()))
}

struct LongTask {
    start: f64,
    duration: f64,
}

impl TimelineEntry for LongTask {
    fn entry_type(&self) -> &'static str {
        "longtask"
    }
    fn start_time(&self) -> f64 {
        self.start
    }
    fn duration(&self) -> f64 {
        self.duration
    }
    fn wrap<'s>(&self, s: &Scope<'s>, key: IdentityKey) -> JsResult<Value<'s>> {
        let data = PerformanceEntryData::new("self", "longtask", self.start, self.duration);
        Ok(s.wrap::<PerformanceLongTaskTiming>(data, Some(key))?.value())
    }
}

pub struct PerformanceLongTaskTiming;

impl JsClass for PerformanceLongTaskTiming {
    const NAME: &'static str = "PerformanceLongTaskTiming";
    type Data = PerformanceEntryData;
    type Parent = PerformanceEntry;

    fn define(def: &mut ClassDef<Self>) {
        def.getter_this("attribution", |s, _d, this| attribution(s, this));
        def.method_this("toJSON", 0, |s, d, this, _a| {
            let o = s.object();
            o.set("name", s.string(&d.name))?;
            o.set("entryType", s.string(d.entry_type))?;
            o.set("startTime", s.number(d.start))?;
            o.set("duration", s.number(d.duration))?;
            let list = attribution(s, this)?;
            let item = list.as_object().map(|a| a.get_index(0)).transpose()?;
            let json = match item.and_then(|v| v.as_object()) {
                Some(item) => item.get("toJSON")?.as_object().map(|f| f.call(Some(item), &[])).transpose()?,
                None => None,
            };
            o.set("attribution", s.array(&json.into_iter().collect::<Vec<_>>())?.value())?;
            Ok(o.value())
        });
    }
}

/// `attribution`: the frozen array of the one `TaskAttributionTiming` (one array per wrapper, as
/// a [SameObject] FrozenArray is).
fn attribution<'s>(s: &Scope<'s>, this: pyro_js::Object<'s>) -> JsResult<Value<'s>> {
    if let Some(list) = s.get_hidden(this, ATTRIBUTION_KEY)?.filter(|v| !v.is_undefined()) {
        return Ok(list);
    }
    let item = s.wrap::<TaskAttributionTiming>(PerformanceEntryData::new("unknown", "taskattribution", 0.0, 0.0), None)?;
    let list = s.frozen_array(&[item.value()])?;
    s.set_hidden(this, ATTRIBUTION_KEY, list)?;
    Ok(list)
}

/// The culprit of a long task. Only the own-window attribution is made (see the module docs).
pub struct TaskAttributionTiming;

impl JsClass for TaskAttributionTiming {
    const NAME: &'static str = "TaskAttributionTiming";
    type Data = PerformanceEntryData;
    type Parent = PerformanceEntry;

    fn define(def: &mut ClassDef<Self>) {
        def.getter("containerType", |s, _d| Ok(s.string("window")));
        def.getter("containerSrc", |s, _d| Ok(s.string("")));
        def.getter("containerId", |s, _d| Ok(s.string("")));
        def.getter("containerName", |s, _d| Ok(s.string("")));
        def.method("toJSON", 0, |s, d, _a| {
            let o = s.object();
            o.set("name", s.string(&d.name))?;
            o.set("entryType", s.string(d.entry_type))?;
            o.set("startTime", s.number(d.start))?;
            o.set("duration", s.number(d.duration))?;
            o.set("containerType", s.string("window"))?;
            o.set("containerSrc", s.string(""))?;
            o.set("containerId", s.string(""))?;
            o.set("containerName", s.string(""))?;
            Ok(o.value())
        });
    }
}

/// Registers the module's classes (`JsClass`es); called once per engine when ENABLED.
pub fn register(engine: &pyro_js::Engine) {
    engine.register::<PerformanceLongTaskTiming>();
    engine.register::<TaskAttributionTiming>();
}

/// A task of the event loop ended; called when ENABLED, for every document.
pub fn on_task(page: &Rc<PageDom>, task: &super::TaskInfo) {
    // A nested task is part of the task it ran in, which is reported whole.
    if task.is_nested() || page.is_disposed() || !ran_script(page) {
        return;
    }
    let duration = (task.end - task.start).round();
    timeline::queue_entry(page, Rc::new(LongTask { start: task.start, duration }));
}
