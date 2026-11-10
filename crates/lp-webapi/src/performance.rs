//! The Performance Timeline's entries and User Timing, shared by a Window's
//! `performance` (lp-browser `nav`) and a worker's ([`WorkerPerformance`]).
//!
//! The entry interfaces (`PerformanceEntry`, `PerformanceResourceTiming`,
//! `PerformanceNavigationTiming`, `PerformancePaintTiming`, `PerformanceMark`,
//! `PerformanceMeasure`) wrap a [`PerformanceEntryData`]; a `resource` entry
//! shares its values with the document's buffer (`resource_timing`).
//! [`UserTiming`] is a realm's marks and measures. `PerformanceObserver`
//! (performance_observer.js) is made on a realm's first use of it.
//!
//! A worker's global has its own timeline: [`WorkerPerformance`] reads the
//! worker's `PageDom` (its resource entries: the fetches the worker made)
//! and its own User Timing. Made on the first read of `performance`.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use lp_js::{ClassDef, JsClass, JsError, JsResult, NoParent, Persistent, Scope, Value};

use crate::page::PageDom;
use crate::resource_timing::ResourceEntry;
use crate::target::EventTargetData;

pub struct PerformanceEntryData {
    pub name: String,
    pub entry_type: &'static str,
    pub start: f64,
    pub duration: f64,
    /// `detail` of a mark or measure (a structured clone), null when absent.
    pub detail: Option<Persistent>,
    /// The document's Navigation Timing (`navigation` entries only).
    pub navigation: Option<Box<NavigationValues>>,
    /// A `resource` entry's values (shared with the document's buffer).
    pub resource: Option<Rc<ResourceEntry>>,
}

impl PerformanceEntryData {
    pub fn new(name: &str, entry_type: &'static str, start: f64, duration: f64) -> PerformanceEntryData {
        PerformanceEntryData {
            name: name.to_owned(),
            entry_type,
            start,
            duration,
            detail: None,
            navigation: None,
            resource: None,
        }
    }
    /// The fetch phases: a resource entry's, or the navigation entry's
    /// document fetch.
    pub fn fetch(&self) -> Option<&ResourceEntry> {
        self.resource.as_deref().or_else(|| self.navigation.as_ref()?.fetch.as_deref())
    }
}

/// The wrapper of a `resource` entry: one object per entry and realm while
/// script holds it (identity by the entry's sequence number).
pub fn resource_object<'s>(s: &Scope<'s>, entry: Rc<ResourceEntry>) -> JsResult<Value<'s>> {
    let key = lp_js::IdentityKey(KEY_RESOURCE, entry.seq);
    if let Some(o) = s.lookup_identity(key) {
        return Ok(o.value());
    }
    let mut data = PerformanceEntryData::new("", "resource", entry.start, entry.duration());
    data.name = entry.name.clone();
    data.resource = Some(entry);
    Ok(s.wrap::<PerformanceResourceTiming>(data, Some(key))?.value())
}

/// Identity key space of `resource` entry wrappers.
const KEY_RESOURCE: u64 = 0x5265_736f_7572_6300;

/// A document's Navigation Timing as its `navigation` entry reports it, in
/// milliseconds since the time origin (the start of the document's fetch);
/// 0 for a step that has not run. Unloading the previous document is not
/// timed (0).
#[derive(Clone)]
pub struct NavigationValues {
    pub dom_interactive: f64,
    pub dom_content_loaded_start: f64,
    pub dom_content_loaded_end: f64,
    pub dom_complete: f64,
    pub load_start: f64,
    pub load_end: f64,
    pub nav_type: &'static str,
    /// The document's fetch (`None`: none, as for a document without one).
    pub fetch: Option<Rc<ResourceEntry>>,
}

impl NavigationValues {
    pub const EMPTY: NavigationValues = NavigationValues {
        dom_interactive: 0.0,
        dom_content_loaded_start: 0.0,
        dom_content_loaded_end: 0.0,
        dom_complete: 0.0,
        load_start: 0.0,
        load_end: 0.0,
        nav_type: "navigate",
        fetch: None,
    };
}

fn entry_json<'s>(s: &Scope<'s>, e: &PerformanceEntryData, detail: bool) -> JsResult<Value<'s>> {
    let o = s.object();
    o.set("name", s.string(&e.name))?;
    o.set("entryType", s.string(e.entry_type))?;
    o.set("startTime", s.number(e.start))?;
    o.set("duration", s.number(e.duration))?;
    if detail {
        o.set("detail", e.detail.as_ref().map_or(s.null(), |d| d.get(s)))?;
    }
    if e.navigation.is_some() || e.resource.is_some() {
        let f = e.fetch();
        o.set("initiatorType", s.string(initiator_type(e)))?;
        o.set("deliveryType", s.string(f.map_or("", |f| f.delivery_type)))?;
        o.set("nextHopProtocol", s.string(f.map_or("", |f| f.next_hop_protocol)))?;
        o.set("renderBlockingStatus", s.string(render_blocking_status(e)))?;
        o.set("contentType", s.string(f.map_or("", |f| &f.content_type)))?;
        for (key, value) in RESOURCE_TIMES.iter().chain(&RESOURCE_SIZES) {
            o.set(key, s.number(f.map_or(0.0, value)))?;
        }
        o.set("serverTiming", s.array(&[])?.value())?;
    }
    if let Some(n) = &e.navigation {
        for (key, value) in navigation_fields(n) {
            o.set(key, s.number(value))?;
        }
        o.set("type", s.string(n.nav_type))?;
        o.set("redirectCount", s.number(redirect_count(n)))?;
    }
    Ok(o.value())
}

fn initiator_type(e: &PerformanceEntryData) -> &'static str {
    match (&e.navigation, &e.resource) {
        (Some(_), _) => "navigation",
        (None, Some(r)) => r.initiator,
        _ => "",
    }
}

fn render_blocking_status(e: &PerformanceEntryData) -> &'static str {
    if e.resource.as_ref().is_some_and(|r| r.render_blocking) {
        "blocking"
    } else {
        "non-blocking"
    }
}

pub fn redirect_count(n: &NavigationValues) -> f64 {
    n.fetch.as_ref().map_or(0.0, |f| f64::from(f.redirect_count))
}

pub type FetchValue = fn(&ResourceEntry) -> f64;

/// PerformanceResourceTiming's times (no service workers, no interim
/// responses: those are 0).
pub const RESOURCE_TIMES: [(&str, FetchValue); 14] = [
    ("workerStart", |_| 0.0),
    ("redirectStart", |f| f.redirect_start),
    ("redirectEnd", |f| f.redirect_end),
    ("fetchStart", |f| f.fetch_start),
    ("domainLookupStart", |f| f.domain_lookup_start),
    ("domainLookupEnd", |f| f.domain_lookup_end),
    ("connectStart", |f| f.connect_start),
    ("connectEnd", |f| f.connect_end),
    ("secureConnectionStart", |f| f.secure_connection_start),
    ("requestStart", |f| f.request_start),
    ("responseStart", |f| f.response_start),
    ("firstInterimResponseStart", |_| 0.0),
    ("finalResponseHeadersStart", |f| f.response_start),
    ("responseEnd", |f| f.response_end),
];
/// Sizes and status.
pub const RESOURCE_SIZES: [(&str, FetchValue); 4] = [
    ("transferSize", |f| f.transfer_size as f64),
    ("encodedBodySize", |f| f.encoded_body_size as f64),
    ("decodedBodySize", |f| f.decoded_body_size as f64),
    ("responseStatus", |f| f64::from(f.response_status)),
];

pub fn navigation_fields(n: &NavigationValues) -> [(&'static str, f64); 10] {
    [
        ("unloadEventStart", 0.0),
        ("unloadEventEnd", 0.0),
        ("domInteractive", n.dom_interactive),
        ("domContentLoadedEventStart", n.dom_content_loaded_start),
        ("domContentLoadedEventEnd", n.dom_content_loaded_end),
        ("domComplete", n.dom_complete),
        ("loadEventStart", n.load_start),
        ("loadEventEnd", n.load_end),
        ("activationStart", 0.0),
        ("criticalCHRestart", 0.0),
    ]
}

pub struct PerformanceEntry;

impl JsClass for PerformanceEntry {
    const NAME: &'static str = "PerformanceEntry";
    type Data = PerformanceEntryData;
    type Parent = NoParent;

    fn define(def: &mut ClassDef<Self>) {
        def.getter("name", |s, e| Ok(s.string(&e.name)));
        def.getter("entryType", |s, e| Ok(s.string(e.entry_type)));
        def.getter("startTime", |s, e| Ok(s.number(e.start)));
        def.getter("duration", |s, e| Ok(s.number(e.duration)));
        def.method("toJSON", 0, |s, e, _a| entry_json(s, e, false));
    }
}

/// Paint Timing: `first-paint` / `first-contentful-paint`.
pub struct PerformancePaintTiming;

impl JsClass for PerformancePaintTiming {
    const NAME: &'static str = "PerformancePaintTiming";
    type Data = PerformanceEntryData;
    type Parent = PerformanceEntry;

    fn define(_def: &mut ClassDef<Self>) {}
}

/// Resource Timing: the document's `resource` entries (lp-webapi
/// `resource_timing`) and its `navigation` entry.
pub struct PerformanceResourceTiming;

impl JsClass for PerformanceResourceTiming {
    const NAME: &'static str = "PerformanceResourceTiming";
    type Data = PerformanceEntryData;
    type Parent = PerformanceEntry;

    fn define(def: &mut ClassDef<Self>) {
        macro_rules! values {
            ($table:ident: $($i:literal),*) => {$(
                def.getter($table[$i].0, |s, e| Ok(s.number(e.fetch().map_or(0.0, $table[$i].1))));
            )*};
        }
        values!(RESOURCE_TIMES: 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13);
        values!(RESOURCE_SIZES: 0, 1, 2, 3);
        def.getter("initiatorType", |s, e| Ok(s.string(initiator_type(e))));
        def.getter("nextHopProtocol", |s, e| Ok(s.string(e.fetch().map_or("", |f| f.next_hop_protocol))));
        def.getter("deliveryType", |s, e| Ok(s.string(e.fetch().map_or("", |f| f.delivery_type))));
        def.getter("renderBlockingStatus", |s, e| Ok(s.string(render_blocking_status(e))));
        def.getter("contentType", |s, e| Ok(s.string(e.fetch().map_or("", |f| &f.content_type))));
        def.getter("serverTiming", |s, _e| Ok(s.array(&[])?.value()));
        def.method("toJSON", 0, |s, e, _a| entry_json(s, e, false));
    }
}

/// Navigation Timing Level 2: the document's `navigation` entry.
pub struct PerformanceNavigationTiming;

impl JsClass for PerformanceNavigationTiming {
    const NAME: &'static str = "PerformanceNavigationTiming";
    type Data = PerformanceEntryData;
    type Parent = PerformanceResourceTiming;

    fn define(def: &mut ClassDef<Self>) {
        macro_rules! fields {
            ($($i:literal),*) => {$(
                def.getter(navigation_fields(&NavigationValues::EMPTY)[$i].0, |s, e| {
                    Ok(s.number(e.navigation.as_ref().map_or(0.0, |n| navigation_fields(n)[$i].1)))
                });
            )*};
        }
        fields!(0, 1, 2, 3, 4, 5, 6, 7, 8, 9);
        def.getter("type", |s, e| Ok(s.string(e.navigation.as_ref().map_or("navigate", |n| n.nav_type))));
        def.getter("redirectCount", |s, e| Ok(s.number(e.navigation.as_deref().map_or(0.0, redirect_count))));
        def.getter("notRestoredReasons", |s, _e| Ok(s.null()));
        def.method("toJSON", 0, |s, e, _a| entry_json(s, e, false));
    }
}

/// Navigation Timing's `PerformanceTiming` attribute names: not usable as
/// mark names in a Window (User Timing).
pub const TIMING_ATTRIBUTES: &[&str] = &[
    "navigationStart",
    "unloadEventStart",
    "unloadEventEnd",
    "redirectStart",
    "redirectEnd",
    "fetchStart",
    "domainLookupStart",
    "domainLookupEnd",
    "connectStart",
    "connectEnd",
    "secureConnectionStart",
    "requestStart",
    "responseStart",
    "responseEnd",
    "domLoading",
    "domInteractive",
    "domContentLoadedEventStart",
    "domContentLoadedEventEnd",
    "domComplete",
    "loadEventStart",
    "loadEventEnd",
];

/// A member of an options dictionary, `None` when absent or undefined.
fn member<'s>(options: Option<lp_js::Object<'s>>, name: &str) -> JsResult<Option<Value<'s>>> {
    match options {
        Some(o) => Ok(Some(o.get(name)?).filter(|v| !v.is_undefined())),
        None => Ok(None),
    }
}

/// The structured clone of a `detail` member (null when absent).
fn clone_detail<'s>(s: &Scope<'s>, detail: Option<Value<'s>>) -> JsResult<Option<Persistent>> {
    let Some(detail) = detail.filter(|v| !v.is_null()) else { return Ok(None) };
    let clone = s.global().get("structuredClone")?;
    let clone = clone.as_object().ok_or_else(|| JsError::Type("structuredClone is unavailable".into()))?;
    Ok(Some(Persistent::new(s, clone.call(None, &[detail])?)))
}

/// User Timing "create a PerformanceMark": `now` is the current time.
pub fn mark_data<'s>(s: &Scope<'s>, name: &str, options: Value<'s>, now: f64) -> JsResult<PerformanceEntryData> {
    if TIMING_ATTRIBUTES.contains(&name) && !crate::worker_scope::is_worker(s) {
        return Err(JsError::dom(
            "SyntaxError",
            format!("'{name}' is part of the PerformanceTiming interface, and cannot be used as a mark name."),
        ));
    }
    if !options.is_nullish() && options.as_object().is_none() {
        return Err(JsError::Type("The mark options are not an object.".into()));
    }
    let options = options.as_object();
    let start = match member(options, "startTime")? {
        Some(v) => {
            let t = v.to_number()?;
            if !t.is_finite() {
                return Err(JsError::Type("startTime is not a finite number.".into()));
            }
            if t < 0.0 {
                return Err(JsError::Type(format!("'{name}' cannot have a negative start time.").into()));
            }
            t
        }
        None => now,
    };
    let detail = clone_detail(s, member(options, "detail")?)?;
    Ok(PerformanceEntryData {
        name: name.to_owned(),
        entry_type: "mark",
        start,
        duration: 0.0,
        detail,
        navigation: None,
        resource: None,
    })
}

pub struct PerformanceMark;

impl JsClass for PerformanceMark {
    const NAME: &'static str = "PerformanceMark";
    type Data = PerformanceEntryData;
    type Parent = PerformanceEntry;

    fn define(def: &mut ClassDef<Self>) {
        def.constructor(1, |s, nt, a| {
            a.require(1, "PerformanceMark")?;
            let name = a.get(0).to_string()?;
            let now = crate::event::page_now_ms(&*crate::target::page_of(s)?);
            s.wrap_new::<PerformanceMark>(mark_data(s, &name, a.get(1), now)?, nt, None)
        });
        def.getter("detail", |s, e| Ok(e.detail.as_ref().map_or(s.null(), |d| d.get(s))));
        def.method("toJSON", 0, |s, e, _a| entry_json(s, e, true));
    }
}

pub struct PerformanceMeasure;

impl JsClass for PerformanceMeasure {
    const NAME: &'static str = "PerformanceMeasure";
    type Data = PerformanceEntryData;
    type Parent = PerformanceEntry;

    fn define(def: &mut ClassDef<Self>) {
        def.getter("detail", |s, e| Ok(e.detail.as_ref().map_or(s.null(), |d| d.get(s))));
        def.method("toJSON", 0, |s, e, _a| entry_json(s, e, true));
    }
}

/// A realm's User Timing marks and measures (its performance entry
/// buffer's), in the order they were added.
#[derive(Default)]
pub struct UserTiming {
    entries: RefCell<Vec<UserEntry>>,
}

/// A `PerformanceMark` or `PerformanceMeasure` in the entry buffer.
struct UserEntry {
    name: String,
    mark: bool,
    start: f64,
    object: Persistent,
}

/// What a timeline does with a name that is not a mark (User Timing
/// "convert a name to a timestamp"): `Some` for a name it knows.
pub type NameTimestamp<'a> = &'a dyn Fn(&str) -> Option<JsResult<f64>>;

impl UserTiming {
    /// `performance.mark(name, options)` at `now`: the new mark.
    pub fn mark<'s>(&self, s: &Scope<'s>, a: &lp_js::Args<'s>, now: f64) -> JsResult<Value<'s>> {
        a.require(1, "Performance.mark")?;
        let name = a.get(0).to_string()?;
        let data = mark_data(s, &name, a.get(1), now)?;
        let start = data.start;
        let object = s.wrap::<PerformanceMark>(data, None)?.value();
        self.entries.borrow_mut().push(UserEntry { name, mark: true, start, object: Persistent::new(s, object) });
        Ok(object)
    }

    /// User Timing "convert a mark to a timestamp".
    fn timestamp(&self, mark: Value<'_>, names: NameTimestamp<'_>) -> JsResult<f64> {
        if mark.is_number() {
            let t = mark.to_number()?;
            if t < 0.0 {
                return Err(JsError::Type("A measure cannot start or end at a negative time.".into()));
            }
            return Ok(t);
        }
        let name = mark.to_string()?;
        if let Some(t) = names(&name) {
            return t;
        }
        let found = self.entries.borrow().iter().rev().find(|e| e.mark && e.name == name).map(|e| e.start);
        found.ok_or_else(|| JsError::dom("SyntaxError", format!("The mark '{name}' does not exist.")))
    }

    /// `performance.measure(name, startOrOptions, endMark)` at `now`: the
    /// new measure.
    pub fn measure<'s>(
        &self,
        s: &Scope<'s>,
        a: &lp_js::Args<'s>,
        now: f64,
        names: NameTimestamp<'_>,
    ) -> JsResult<Value<'s>> {
        a.require(1, "Performance.measure")?;
        let name = a.get(0).to_string()?;
        let end_mark = a.opt(2).filter(|v| !v.is_undefined());
        // A dictionary only when it is an object (a string or number is a mark).
        let options = a.get(1).as_object();
        let (start, end, duration) = (member(options, "start")?, member(options, "end")?, member(options, "duration")?);
        let detail = member(options, "detail")?;
        let is_options = start.is_some() || end.is_some() || duration.is_some() || detail.is_some();
        if is_options {
            if end_mark.is_some() {
                return Err(JsError::Type(
                    "If a non-empty measure options object is given, endMark must not be.".into(),
                ));
            }
            if start.is_none() && end.is_none() {
                return Err(JsError::Type("The measure options need at least one of start and end.".into()));
            }
            if start.is_some() && end.is_some() && duration.is_some() {
                return Err(JsError::Type("The measure options cannot have all of start, duration and end.".into()));
            }
        }
        let duration = duration.map(|d| d.to_number()).transpose()?;
        let end_time = match (end_mark, end, start, duration) {
            (Some(mark), ..) => self.timestamp(mark, names)?,
            (None, Some(end), ..) => self.timestamp(end, names)?,
            (None, None, Some(start), Some(duration)) => self.timestamp(start, names)? + duration,
            _ => now,
        };
        let start_time = match (start, duration, end) {
            (Some(start), ..) => self.timestamp(start, names)?,
            (None, Some(duration), Some(end)) => self.timestamp(end, names)? - duration,
            _ if !is_options && !a.get(1).is_nullish() => self.timestamp(a.get(1), names)?,
            _ => 0.0,
        };
        let data = PerformanceEntryData {
            name: name.clone(),
            entry_type: "measure",
            start: start_time,
            duration: end_time - start_time,
            detail: clone_detail(s, detail)?,
            navigation: None,
            resource: None,
        };
        let object = s.wrap::<PerformanceMeasure>(data, None)?.value();
        self.entries.borrow_mut().push(UserEntry {
            name,
            mark: false,
            start: start_time,
            object: Persistent::new(s, object),
        });
        Ok(object)
    }

    /// `clearMarks(name)` / `clearMeasures(name)` (`a.get(0)`).
    pub fn clear(&self, marks: bool, a: &lp_js::Args<'_>) -> JsResult<()> {
        let name = a.opt(0).filter(|v| !v.is_undefined()).map(|v| v.to_string()).transpose()?;
        let removed: Vec<UserEntry> = {
            let mut entries = self.entries.borrow_mut();
            let (gone, kept) = std::mem::take(&mut *entries)
                .into_iter()
                .partition(|e| e.mark == marks && name.as_ref().is_none_or(|n| *n == e.name));
            *entries = kept;
            gone
        };
        drop(removed);
        Ok(())
    }

    /// Adds the marks and measures matching the filters to `out`, with
    /// their start times.
    pub fn collect<'s>(
        &self,
        s: &Scope<'s>,
        name: Option<&str>,
        entry_type: Option<&str>,
        out: &mut Vec<(f64, Value<'s>)>,
    ) {
        for entry in self.entries.borrow().iter() {
            let ty = if entry.mark { "mark" } else { "measure" };
            if name.is_none_or(|n| n == entry.name) && entry_type.is_none_or(|t| t == ty) {
                out.push((entry.start, entry.object.get(s)));
            }
        }
    }
}

/// Adds the document's `resource` entries matching `name` to `out` (the
/// fetches that ended are added to the buffer first).
pub fn collect_resources<'s>(
    s: &Scope<'s>,
    page: &Rc<PageDom>,
    name: Option<&str>,
    out: &mut Vec<(f64, Value<'s>)>,
) -> JsResult<()> {
    page.add_finished_fetches();
    for entry in page.resource_timings().entries() {
        if name.is_none_or(|n| n == entry.name) {
            out.push((entry.start, resource_object(s, entry)?));
        }
    }
    Ok(())
}

/// `entries` sorted by start time as an array (stable: entries with equal
/// start times keep the order they were added in).
pub fn sorted_entries<'s>(s: &Scope<'s>, mut entries: Vec<(f64, Value<'s>)>) -> JsResult<Value<'s>> {
    entries.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let values: Vec<Value<'s>> = entries.into_iter().map(|(_, v)| v).collect();
    Ok(s.array(&values)?.value())
}

/// `setResourceTimingBufferSize(n)` of the document's buffer.
pub fn set_buffer_size(page: &Rc<PageDom>, a: &lp_js::Args<'_>) -> JsResult<()> {
    a.require(1, "Performance.setResourceTimingBufferSize")?;
    // WebIDL `unsigned long`.
    let n = a.get(0).to_number()?;
    let n = if n.is_finite() { n.trunc().rem_euclid(4_294_967_296.0) } else { 0.0 };
    page.add_finished_fetches();
    page.resource_timings().set_limit(n as usize);
    Ok(())
}

/// `clearResourceTimings()` of the document's buffer.
pub fn clear_resources(page: &Rc<PageDom>) {
    page.add_finished_fetches();
    page.resource_timings().clear();
}

/// performance_observer.js: `PerformanceObserver` and
/// `PerformanceObserverEntryList` for one realm.
pub const OBSERVER_SOURCE: &str = include_str!("performance_observer.js");
const OBSERVER_URL: &str = "lightningpanda://internal/performance-observer.js";
/// Hidden key of the observer module on the realm's `performance`.
const OBSERVER_KEY: &str = "lpPerformanceObserver";

/// The realm's performance_observer.js module, made on first use and kept on
/// its `performance` object. `watch(queue | null)` registers the realm's
/// queue function; `page` is the document whose resource buffer counts the
/// dropped entries; `supported` is `PerformanceObserver.supportedEntryTypes`.
pub fn observer_module<'s>(
    s: &Scope<'s>,
    performance: lp_js::Object<'s>,
    page: &Rc<PageDom>,
    watch: impl Fn(&Scope<'_>, Value<'_>) + 'static,
    supported: &[&str],
) -> JsResult<lp_js::Object<'s>> {
    if let Some(module) = s.get_hidden(performance, OBSERVER_KEY)?.and_then(|v| v.as_object()) {
        return Ok(module);
    }
    let unavailable = || JsError::Type("PerformanceObserver is unavailable".into());
    let natives = s.object();
    let watch = s.function("watch", move |s, _this, a| {
        watch(s, a.get(0));
        Ok(s.undefined())
    })?;
    natives.set_atom("watch", watch.value())?;
    let report = s.function("reportException", |s, _this, a| {
        s.report_exception(&JsError::thrown(Persistent::new(s, a.get(0))));
        Ok(s.undefined())
    })?;
    natives.set_atom("reportException", report.value())?;
    let page = Rc::downgrade(page);
    let dropped = s.function("droppedResources", move |s, _this, _a| {
        let Some(page) = page.upgrade() else { return Ok(s.number(0.0)) };
        page.add_finished_fetches();
        Ok(s.number(page.resource_timings().dropped() as f64))
    })?;
    natives.set_atom("droppedResources", dropped.value())?;
    let types: Vec<Value<'s>> = supported.iter().map(|t| s.string(t)).collect();
    let types = s.array(&types)?;
    let root = Persistent::new(s, natives.value());
    let make = s.eval(OBSERVER_SOURCE, Some(OBSERVER_URL), 1)?;
    let module =
        make.as_object().ok_or_else(unavailable)?.call(None, &[performance.value(), root.get(s), types.value()])?;
    s.set_hidden(performance, OBSERVER_KEY, module)?;
    module.as_object().ok_or_else(unavailable)
}

// ---- Workers --------------------------------------------------------------

/// A worker's `performance`: its time origin is the worker's start, its
/// timeline the worker's User Timing and the `resource` entries of the
/// fetches the worker made (its `PageDom`'s buffer, Timing-Allow-Origin
/// checked against the worker's origin).
pub struct WorkerPerformanceData {
    target: EventTargetData,
    started: Instant,
    origin_wall_ms: f64,
    user: UserTiming,
    /// The realm's observer queue (performance_observer.js), while some
    /// observer observes.
    watcher: RefCell<Option<Rc<Persistent>>>,
}

impl Drop for WorkerPerformanceData {
    fn drop(&mut self) {
        self.target.page.resource_timings().remove_target(self.target.key);
    }
}

impl WorkerPerformanceData {
    fn now_ms(&self) -> f64 {
        now_since(self.started)
    }

    fn notify(&self, s: &Scope<'_>, entry_type: &'static str, entry: Value<'_>) -> JsResult<()> {
        let watcher = self.watcher.borrow().clone();
        if let Some(f) = watcher.and_then(|w| w.get(s).as_object()) {
            f.call(None, &[s.string(entry_type), entry])?;
        }
        Ok(())
    }
}

/// Milliseconds since `origin`, in steps of 100 microseconds (the
/// resolution of `performance.now()` without cross-origin isolation).
pub(crate) fn now_since(origin: Instant) -> f64 {
    let ms = origin.elapsed().as_secs_f64() * 1000.0;
    (ms * 10.0).floor() / 10.0
}

/// `PerformanceObserver.supportedEntryTypes` in a worker.
const WORKER_ENTRY_TYPES: &[&str] = &["mark", "measure", "resource"];

fn worker_entries<'s>(
    s: &Scope<'s>,
    p: &WorkerPerformanceData,
    name: Option<&str>,
    entry_type: Option<&str>,
) -> JsResult<Value<'s>> {
    let mut entries = Vec::new();
    p.user.collect(s, name, entry_type, &mut entries);
    if entry_type.is_none_or(|t| t == "resource") {
        collect_resources(s, &p.target.page, name, &mut entries)?;
    }
    sorted_entries(s, entries)
}

/// In a worker, a `PerformanceTiming` attribute name is no timestamp.
fn worker_name_timestamp(name: &str) -> Option<JsResult<f64>> {
    TIMING_ATTRIBUTES.contains(&name).then(|| {
        Err(JsError::Type(format!("Failed to execute 'measure' on 'Performance': '{name}' is not a mark in a worker.").into()))
    })
}

pub struct WorkerPerformance;

impl JsClass for WorkerPerformance {
    const NAME: &'static str = "Performance";
    type Data = WorkerPerformanceData;
    type Parent = crate::target::EventTarget;

    fn define(def: &mut ClassDef<Self>) {
        def.upcast(|p: &WorkerPerformanceData| &p.target);
        def.pure_method("now", 0, |s, p, _a| Ok(s.number(p.now_ms())));
        def.getter("timeOrigin", |s, p| Ok(s.number(p.origin_wall_ms)));
        def.method("mark", 1, |s, p, a| {
            let mark = p.user.mark(s, &a, p.now_ms())?;
            p.notify(s, "mark", mark)?;
            Ok(mark)
        });
        def.method("measure", 1, |s, p, a| {
            let measure = p.user.measure(s, &a, p.now_ms(), &worker_name_timestamp)?;
            p.notify(s, "measure", measure)?;
            Ok(measure)
        });
        def.method("clearMarks", 0, |s, p, a| p.user.clear(true, &a).map(|()| s.undefined()));
        def.method("clearMeasures", 0, |s, p, a| p.user.clear(false, &a).map(|()| s.undefined()));
        def.pure_method("getEntries", 0, |s, p, _a| worker_entries(s, p, None, None));
        def.pure_method("getEntriesByName", 1, |s, p, a| {
            a.require(1, "Performance.getEntriesByName")?;
            let name = a.get(0).to_string()?;
            let ty = a.opt(1).filter(|v| !v.is_undefined()).map(|v| v.to_string()).transpose()?;
            worker_entries(s, p, Some(&name), ty.as_deref())
        });
        def.pure_method("getEntriesByType", 1, |s, p, a| {
            a.require(1, "Performance.getEntriesByType")?;
            let ty = a.get(0).to_string()?;
            worker_entries(s, p, None, Some(&ty))
        });
        def.method("setResourceTimingBufferSize", 1, |s, p, a| {
            set_buffer_size(&p.target.page, &a).map(|()| s.undefined())
        });
        def.method("clearResourceTimings", 0, |s, p, _a| {
            clear_resources(&p.target.page);
            Ok(s.undefined())
        });
        def.accessor(
            "onresourcetimingbufferfull",
            |s, p| crate::resource_timing::buffer_full_handler(s, &p.target),
            |s, p, v| crate::resource_timing::set_buffer_full_handler(s, &p.target, v),
        );
        def.method("toJSON", 0, |s, p, _a| {
            let o = s.object();
            o.set("timeOrigin", s.number(p.origin_wall_ms))?;
            Ok(o.value())
        });
    }
}

/// Identity of a worker realm's `performance` when its target has none.
const KEY_WORKER_PERFORMANCE: u64 = 0x5065_7257_6f72_6b00;

/// The worker realm's `performance` ([SameObject], an EventTarget whose
/// wrapper the realm keeps for its listeners), made on first read; `slot`
/// keeps its identity. `started` is the worker's time origin.
pub(crate) fn worker_performance<'s>(
    s: &Scope<'s>,
    page: &Rc<PageDom>,
    slot: &std::cell::Cell<Option<lp_js::IdentityKey>>,
    started: Instant,
    origin_wall_ms: f64,
) -> JsResult<lp_js::Object<'s>> {
    if let Some(o) = slot.get().and_then(|identity| s.lookup_identity(identity)) {
        return Ok(o);
    }
    let target = EventTargetData::new(page);
    let key = target.key;
    let identity = target.wrapper_identity().unwrap_or(lp_js::IdentityKey(KEY_WORKER_PERFORMANCE, 0));
    page.resource_timings().add_target(key);
    let data = WorkerPerformanceData {
        target,
        started,
        origin_wall_ms,
        user: UserTiming::default(),
        watcher: RefCell::new(None),
    };
    let performance = s.wrap::<WorkerPerformance>(data, Some(identity))?;
    crate::target::pin_target(s, page, key, performance);
    slot.set(Some(identity));
    Ok(performance)
}

/// The worker realm's observer module (`PerformanceObserver`,
/// `PerformanceObserverEntryList`).
pub(crate) fn worker_observer_module<'s>(s: &Scope<'s>, performance: lp_js::Object<'s>) -> JsResult<lp_js::Object<'s>> {
    let p = s
        .unwrap::<WorkerPerformance>(performance.value())
        .ok_or_else(|| JsError::Type("PerformanceObserver is unavailable".into()))?;
    let page = p.target.page.clone();
    let realm = s.realm().downgrade();
    let watch_page = Rc::downgrade(&page);
    let watch = move |s: &Scope<'_>, queue: Value<'_>| {
        let Some(page) = watch_page.upgrade() else { return };
        let Ok(performance) = worker_performance_of(s) else { return };
        let Some(p) = s.unwrap::<WorkerPerformance>(performance.value()) else { return };
        let queue = queue.as_object().filter(|f| f.is_function()).map(|f| Rc::new(Persistent::new(s, f.value())));
        let watched = queue.is_some();
        *p.watcher.borrow_mut() = queue;
        // New resource entries reach the observers only while some watch.
        let realm = realm.clone();
        let listener = watched.then(|| {
            Rc::new(move |entry: &Rc<ResourceEntry>| {
                let Some(realm) = realm.upgrade().filter(|r| !r.is_disposed()) else { return };
                let Ok(s) = realm.scope() else { return };
                let Ok(performance) = worker_performance_of(&s) else { return };
                let Some(p) = s.unwrap::<WorkerPerformance>(performance.value()) else { return };
                let queued = resource_object(&s, entry.clone()).and_then(|o| p.notify(&s, "resource", o));
                if let Err(e) = queued {
                    s.report_exception(&e);
                }
            }) as crate::resource_timing::EntryListener
        });
        page.resource_timings().set_listener(listener);
    };
    observer_module(s, performance, &page, watch, WORKER_ENTRY_TYPES)
}

/// The worker realm's `performance`.
fn worker_performance_of<'s>(s: &Scope<'s>) -> JsResult<lp_js::Object<'s>> {
    crate::worker_scope::performance(s)
}

pub(crate) fn register_for_worker(engine: &lp_js::Engine) {
    engine.register::<PerformanceEntry>();
    engine.register::<PerformanceResourceTiming>();
    engine.register::<PerformanceMark>();
    engine.register::<PerformanceMeasure>();
    engine.register::<WorkerPerformance>();
}
