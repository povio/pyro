//! Resource Timing: the document's `PerformanceResourceTiming` entries.
//!
//! A fetch of the document (fetch, XHR, EventSource, beacon here; scripts,
//! stylesheets, images, preloads and iframes in lp-browser's loader) is
//! reported once it ends as a [`FetchReport`]: plain `Instant`s from lp-net
//! and a few numbers. [`PageDom::add_resource_timing`] turns it into a
//! [`ResourceEntry`] (milliseconds since the time origin, Timing-Allow-Origin
//! applied) and pushes it into the document's buffer, [`ResourceTimings`].
//! No JS object is made then: lp-browser wraps entries when a script reads
//! the timeline, and hands each new one to observers only while some
//! `PerformanceObserver` watches (a listener installed here).
//!
//! The buffer follows Resource Timing: [`DEFAULT_BUFFER_SIZE`] entries, a
//! secondary buffer while it is full, and a `resourcetimingbufferfull` event
//! (queued once) at the document's `Performance` objects that may make room
//! (`setResourceTimingBufferSize`, `clearResourceTimings`) before the rest
//! is dropped.

use crate::event::{EventCore, EventInterface};
use crate::page::PageDom;
use lp_dom::events::TargetKey;
use lp_net::{ResponseEvent, ResponseSink};
use rustc_hash::FxHashMap;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::{Rc, Weak};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Entries the buffer holds until `setResourceTimingBufferSize`.
pub const DEFAULT_BUFFER_SIZE: usize = 250;
/// Entries waiting in the secondary buffer for the buffer full event; more
/// are dropped at once (a page can start any number of fetches in a task).
const MAX_SECONDARY: usize = 1024;
/// Resource Timing's stand-in for the response header size in
/// `transferSize`.
const HEADER_SIZE: u64 = 300;

/// `PerformanceResourceTiming.initiatorType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Initiator {
    Fetch,
    XmlHttpRequest,
    Beacon,
    Script,
    Link,
    Css,
    Img,
    Iframe,
    Other,
}

impl Initiator {
    pub fn as_str(self) -> &'static str {
        match self {
            Initiator::Fetch => "fetch",
            Initiator::XmlHttpRequest => "xmlhttprequest",
            Initiator::Beacon => "beacon",
            Initiator::Script => "script",
            Initiator::Link => "link",
            Initiator::Css => "css",
            Initiator::Img => "img",
            Initiator::Iframe => "iframe",
            Initiator::Other => "other",
        }
    }
}

/// A finished fetch as the network saw it (sent from the I/O thread).
#[derive(Clone, Debug)]
pub struct FetchReport {
    /// The request's URL (the entry's `name`; redirects do not change it).
    pub url: String,
    pub initiator: Initiator,
    /// When the document started the fetch (`startTime`).
    pub start: Instant,
    /// When the body was complete or the fetch failed (`responseEnd`).
    pub end: Instant,
    /// `None`: a network error.
    pub response: Option<ReportedResponse>,
    pub render_blocking: bool,
}

/// The response part of a [`FetchReport`].
#[derive(Clone, Debug, Default)]
pub struct ReportedResponse {
    pub timing: lp_net::NetTiming,
    pub status: u16,
    /// A `no-cors` cross-origin response: status and type are hidden.
    pub opaque: bool,
    pub from_cache: bool,
    pub protocol: Option<lp_net::http::Version>,
    pub mime: String,
    pub encoded: u64,
    pub decoded: u64,
}

impl ReportedResponse {
    pub fn of(head: &lp_net::ResponseHead) -> ReportedResponse {
        let opaque = matches!(head.kind, lp_net::ResponseKind::Opaque | lp_net::ResponseKind::OpaqueRedirect);
        ReportedResponse {
            timing: head.timing,
            status: head.status.as_u16(),
            opaque,
            from_cache: head.from_cache,
            protocol: Some(head.protocol),
            mime: if opaque { String::new() } else { head.mime.clone() },
            encoded: 0,
            decoded: 0,
        }
    }
}

/// One `PerformanceResourceTiming` entry; times in milliseconds since the
/// document's time origin, 0 where hidden or not applicable.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ResourceEntry {
    /// Unique among the document's entries (wrapper identity).
    pub seq: u64,
    pub name: String,
    pub initiator: &'static str,
    pub start: f64,
    pub redirect_start: f64,
    pub redirect_end: f64,
    pub fetch_start: f64,
    pub domain_lookup_start: f64,
    pub domain_lookup_end: f64,
    pub connect_start: f64,
    pub connect_end: f64,
    pub secure_connection_start: f64,
    pub request_start: f64,
    pub response_start: f64,
    pub response_end: f64,
    pub transfer_size: u64,
    pub encoded_body_size: u64,
    pub decoded_body_size: u64,
    pub response_status: u16,
    pub redirect_count: u16,
    pub next_hop_protocol: &'static str,
    pub content_type: String,
    pub render_blocking: bool,
    pub delivery_type: &'static str,
}

impl ResourceEntry {
    pub fn duration(&self) -> f64 {
        self.response_end - self.start
    }
}

fn protocol_name(v: lp_net::http::Version) -> &'static str {
    match v {
        lp_net::http::Version::HTTP_09 => "http/0.9",
        lp_net::http::Version::HTTP_10 => "http/1.0",
        lp_net::http::Version::HTTP_11 => "http/1.1",
        lp_net::http::Version::HTTP_2 => "h2",
        lp_net::http::Version::HTTP_3 => "h3",
        _ => "",
    }
}

/// The entry of `report`, with `base` (an `Instant`) at `base_ms` on the
/// document's clock. Times are floored to 100 microseconds as
/// `performance.now()` is, and never go backwards along the fetch.
/// A response that failed the Timing-Allow-Origin check keeps only its
/// start, `fetchStart` and end ("opaque timing info").
pub fn entry_of(report: &FetchReport, base: Instant, base_ms: f64) -> ResourceEntry {
    let ms = |t: Instant| {
        let at = match t.checked_duration_since(base) {
            Some(after) => base_ms + after.as_secs_f64() * 1000.0,
            None => base_ms - base.duration_since(t).as_secs_f64() * 1000.0,
        };
        (at.max(0.0) * 10.0).floor() / 10.0
    };
    let start = ms(report.start);
    let end = ms(report.end).max(start);
    let mut e = ResourceEntry {
        name: report.url.clone(),
        initiator: report.initiator.as_str(),
        start,
        fetch_start: start,
        response_end: end,
        render_blocking: report.render_blocking,
        ..Default::default()
    };
    let Some(r) = &report.response else { return e };
    let t = &r.timing;
    // A step never comes before the one it follows (clamped forward).
    let after = |floor: f64, t: Option<Instant>| t.map_or(floor, |t| ms(t).max(floor));
    if t.redirect_count > 0 {
        e.redirect_start = start;
        e.redirect_end = after(start, t.redirect_end);
        e.fetch_start = after(e.redirect_end, t.fetch_start);
        e.redirect_count = t.redirect_count;
    }
    e.response_status = if r.opaque { 0 } else { r.status };
    e.content_type = r.mime.clone();
    if t.timing_allow_failed {
        e.redirect_start = 0.0;
        e.redirect_end = 0.0;
        e.redirect_count = 0;
        return e;
    }
    let fetch_start = e.fetch_start;
    match t.connection {
        Some(c) => {
            let (lookup_start, lookup_end) = c.dns.unwrap_or((c.start, c.start));
            e.domain_lookup_start = after(fetch_start, Some(lookup_start));
            e.domain_lookup_end = after(e.domain_lookup_start, Some(lookup_end));
            e.connect_start = e.domain_lookup_end;
            e.connect_end = after(e.connect_start, Some(c.end));
            if c.secure {
                e.secure_connection_start = after(e.connect_start, Some(c.tcp_end)).min(e.connect_end);
            }
        }
        None => {
            e.domain_lookup_start = fetch_start;
            e.domain_lookup_end = fetch_start;
            e.connect_start = fetch_start;
            e.connect_end = fetch_start;
            if report.url.starts_with("https:") {
                e.secure_connection_start = fetch_start;
            }
        }
    }
    e.request_start = after(e.connect_end, t.request_start);
    e.response_start = after(e.request_start, t.response_start);
    e.response_end = e.response_end.max(e.response_start);
    e.encoded_body_size = r.encoded;
    e.decoded_body_size = r.decoded;
    e.transfer_size = if r.from_cache { 0 } else { r.encoded + HEADER_SIZE };
    e.next_hop_protocol = match (t.start, r.protocol) {
        (Some(_), Some(p)) => protocol_name(p),
        _ => "",
    };
    e.delivery_type = if r.from_cache { "cache" } else { "" };
    e
}

/// Called with each entry added while some observer watches.
pub type EntryListener = Rc<dyn Fn(&Rc<ResourceEntry>)>;

/// The document's resource timing buffer.
pub struct ResourceTimings {
    buffer: RefCell<Vec<Rc<ResourceEntry>>>,
    secondary: RefCell<VecDeque<Rc<ResourceEntry>>>,
    limit: Cell<usize>,
    /// "resource timing buffer full event pending flag".
    full_pending: Cell<bool>,
    /// Entries that did not make it into the buffer (observers'
    /// `droppedEntriesCount`), including those still waiting in the
    /// secondary buffer.
    dropped: Cell<u64>,
    /// The `Performance` objects of the document's realms (event targets of
    /// `resourcetimingbufferfull`).
    targets: RefCell<Vec<TargetKey>>,
    listener: RefCell<Option<EntryListener>>,
    /// The page is in [`PAGES`] (reports from the I/O thread find it).
    registered: Cell<bool>,
    next_seq: Cell<u64>,
    /// Fetches that ended on the I/O thread, not yet entries: a task adds
    /// them, and so does every read of the timeline first, so a fetch whose
    /// end its consumer has seen is always in the timeline.
    inbox: Inbox,
}

type Inbox = Arc<Mutex<Vec<FetchReport>>>;

impl Default for ResourceTimings {
    fn default() -> Self {
        ResourceTimings {
            buffer: RefCell::new(Vec::new()),
            secondary: RefCell::new(VecDeque::new()),
            limit: Cell::new(DEFAULT_BUFFER_SIZE),
            full_pending: Cell::new(false),
            dropped: Cell::new(0),
            targets: RefCell::new(Vec::new()),
            listener: RefCell::new(None),
            registered: Cell::new(false),
            next_seq: Cell::new(1),
            inbox: Arc::default(),
        }
    }
}

impl ResourceTimings {
    /// The buffered entries, in the order they were added (call
    /// [`PageDom::add_finished_fetches`] first).
    pub fn entries(&self) -> Vec<Rc<ResourceEntry>> {
        self.buffer.borrow().clone()
    }
    pub fn is_empty(&self) -> bool {
        self.buffer.borrow().is_empty()
    }
    /// The next entry's sequence number.
    pub fn next_seq(&self) -> u64 {
        self.next_seq.get()
    }
    /// `setResourceTimingBufferSize(n)`: entries over a smaller size stay.
    pub fn set_limit(&self, limit: usize) {
        self.limit.set(limit);
    }
    /// `clearResourceTimings()`.
    pub fn clear(&self) {
        let cleared = std::mem::take(&mut *self.buffer.borrow_mut());
        drop(cleared);
    }
    /// Entries dropped so far (`droppedEntriesCount` of `resource`).
    pub fn dropped(&self) -> u64 {
        self.dropped.get()
    }
    /// A `Performance` object of the document (an event target).
    pub fn add_target(&self, key: TargetKey) {
        self.targets.borrow_mut().push(key);
    }
    pub fn remove_target(&self, key: TargetKey) {
        self.targets.borrow_mut().retain(|k| *k != key);
    }
    /// Observers watch (`Some`) or stopped (`None`).
    pub fn set_listener(&self, listener: Option<EntryListener>) {
        drop(self.listener.replace(listener));
    }
    fn release(&self) {
        drop(self.buffer.take());
        drop(self.secondary.take());
        drop(self.listener.take());
        self.targets.borrow_mut().clear();
        drop(std::mem::take(&mut *self.inbox.lock().unwrap_or_else(|e| e.into_inner())));
    }
}

thread_local! {
    /// Pages whose fetches report from the I/O thread, by `PageDom::id`.
    static PAGES: RefCell<FxHashMap<u64, Weak<PageDom>>> = RefCell::new(FxHashMap::default());
}

impl PageDom {
    pub fn resource_timings(&self) -> &ResourceTimings {
        &self.resource_timings
    }

    /// Adds the entries of the fetches that ended on the I/O thread and are
    /// not in the timeline yet. Reads of the timeline call it first.
    pub fn add_finished_fetches(self: &Rc<Self>) {
        let reports = {
            let mut inbox = self.resource_timings.inbox.lock().unwrap_or_else(|e| e.into_inner());
            if inbox.is_empty() {
                return;
            }
            std::mem::take(&mut *inbox)
        };
        for report in &reports {
            self.add_resource_timing(report);
        }
    }

    /// Adds the entry of a finished fetch of this document (Resource Timing
    /// "mark resource timing"). Only HTTP(S) fetches have one.
    pub fn add_resource_timing(self: &Rc<Self>, report: &FetchReport) {
        if self.is_disposed() || !(report.url.starts_with("http:") || report.url.starts_with("https:")) {
            return;
        }
        let Some(now_ms) = self.event_time() else { return };
        let rt = &self.resource_timings;
        let mut entry = entry_of(report, Instant::now(), now_ms);
        entry.seq = rt.next_seq.replace(rt.next_seq.get() + 1);
        let entry = Rc::new(entry);
        if !rt.full_pending.get() && rt.buffer.borrow().len() < rt.limit.get() {
            rt.buffer.borrow_mut().push(entry.clone());
        } else {
            rt.dropped.set(rt.dropped.get() + 1);
            let mut secondary = rt.secondary.borrow_mut();
            if secondary.len() < MAX_SECONDARY {
                secondary.push_back(entry.clone());
            }
            drop(secondary);
            if !rt.full_pending.replace(true) {
                let page = Rc::downgrade(self);
                self.post_task(Box::new(move || {
                    if let Some(page) = page.upgrade() {
                        fire_buffer_full(&page);
                    }
                }));
            }
        }
        let listener = rt.listener.borrow().clone();
        if let Some(listener) = listener {
            listener(&entry);
        }
    }
}

/// Resource Timing "fire a buffer full event": while entries wait, tell the
/// `Performance` objects the buffer is full, then move what fits; the rest
/// is dropped once a round makes no room.
fn fire_buffer_full(page: &Rc<PageDom>) {
    let rt = &page.resource_timings;
    while !rt.secondary.borrow().is_empty() && !page.is_disposed() {
        let before = rt.secondary.borrow().len();
        if rt.buffer.borrow().len() >= rt.limit.get() {
            let targets = rt.targets.borrow().clone();
            for target in targets {
                let event = EventCore::trusted(page, EventInterface::Event, "resourcetimingbufferfull", false, false);
                crate::target::fire(page, target, &event);
            }
        }
        {
            let (mut buffer, mut secondary) = (rt.buffer.borrow_mut(), rt.secondary.borrow_mut());
            while buffer.len() < rt.limit.get() {
                let Some(entry) = secondary.pop_front() else { break };
                buffer.push(entry);
                rt.dropped.set(rt.dropped.get().saturating_sub(1));
            }
        }
        let after = rt.secondary.borrow().len();
        if before <= after {
            let dropped = std::mem::take(&mut *rt.secondary.borrow_mut());
            drop(dropped);
            break;
        }
    }
    rt.full_pending.set(false);
}

pub(crate) fn dispose(page: &PageDom) {
    if page.resource_timings.registered.get() {
        PAGES.with(|pages| pages.borrow_mut().remove(&page.id()));
    }
    page.resource_timings.release();
}

/// `sink` reporting the fetch it receives to `page`'s timeline when it ends
/// (an aborted fetch has no entry). The report is queued for the page before
/// the sink sees the end, so once the fetch's consumer has seen it, a read
/// of the timeline has the entry. Not an HTTP(S) URL, or a page without a
/// cross-thread poster: `sink` unchanged.
pub fn timed_sink(
    page: &Rc<PageDom>,
    initiator: Initiator,
    url: &lp_net::Url,
    render_blocking: bool,
    mut sink: ResponseSink,
) -> ResponseSink {
    if !matches!(url.scheme(), "http" | "https") || page.is_disposed() {
        return sink;
    }
    let Some(poster) = page.remote_poster() else { return sink };
    if !page.resource_timings.registered.replace(true) {
        PAGES.with(|pages| pages.borrow_mut().insert(page.id(), Rc::downgrade(page)));
    }
    let page_id = page.id();
    let inbox = page.resource_timings.inbox.clone();
    let start = Instant::now();
    let mut report = Some(FetchReport {
        url: url.as_str().to_owned(),
        initiator,
        start,
        end: start,
        response: None,
        render_blocking,
    });
    Box::new(move |event| {
        match &event {
            ResponseEvent::Response(head) => {
                if let Some(report) = report.as_mut() {
                    report.response = Some(ReportedResponse::of(head));
                }
            }
            ResponseEvent::Finished { encoded_len, decoded_len } => {
                if let Some(mut report) = report.take() {
                    report.end = Instant::now();
                    if let Some(r) = report.response.as_mut() {
                        r.encoded = *encoded_len;
                        r.decoded = *decoded_len;
                    }
                    post_report(&poster, &inbox, page_id, report);
                }
            }
            ResponseEvent::Failed(error) => {
                if let Some(mut report) = report.take() {
                    if *error != lp_net::NetError::Aborted {
                        report.end = Instant::now();
                        report.response = None;
                        post_report(&poster, &inbox, page_id, report);
                    }
                }
            }
            ResponseEvent::Redirect { .. } | ResponseEvent::Data(_) => {}
        }
        sink(event)
    })
}

/// Queues `report` for its page (I/O thread); the first one queued posts the
/// task that adds them.
fn post_report(poster: &crate::page::RemotePoster, inbox: &Inbox, page_id: u64, report: FetchReport) {
    let first = {
        let mut inbox = inbox.lock().unwrap_or_else(|e| e.into_inner());
        inbox.push(report);
        inbox.len() == 1
    };
    if first {
        poster(Box::new(move || {
            if let Some(page) = PAGES.with(|pages| pages.borrow().get(&page_id).and_then(Weak::upgrade)) {
                page.add_finished_fetches();
            }
        }));
    }
}

/// `onresourcetimingbufferfull` of a `Performance` object (`target`).
pub fn buffer_full_handler<'s>(
    s: &lp_js::Scope<'s>,
    target: &crate::EventTargetData,
) -> lp_js::JsResult<lp_js::Value<'s>> {
    crate::target::event_handler(s, target, "resourcetimingbufferfull")
}

pub fn set_buffer_full_handler(
    s: &lp_js::Scope<'_>,
    target: &crate::EventTargetData,
    value: lp_js::Value<'_>,
) -> lp_js::JsResult<()> {
    crate::target::set_event_handler(s, target, "resourcetimingbufferfull", value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn report(url: &str, response: Option<ReportedResponse>) -> (FetchReport, Instant) {
        let base = Instant::now();
        let r = FetchReport {
            url: url.into(),
            initiator: Initiator::Fetch,
            start: base + Duration::from_millis(10),
            end: base + Duration::from_millis(50),
            response,
            render_blocking: false,
        };
        (r, base)
    }

    #[test]
    fn network_phases_in_order() {
        let base = Instant::now();
        let at = |ms: u64| base + Duration::from_millis(ms);
        let timing = lp_net::NetTiming {
            start: Some(at(10)),
            fetch_start: Some(at(10)),
            connection: Some(lp_net::ConnectionTiming {
                start: at(11),
                dns: Some((at(11), at(13))),
                tcp_end: at(15),
                secure: true,
                end: at(20),
            }),
            request_start: Some(at(20)),
            response_start: Some(at(30)),
            ..Default::default()
        };
        let response = ReportedResponse {
            timing,
            status: 200,
            protocol: Some(lp_net::http::Version::HTTP_2),
            mime: "text/plain".into(),
            encoded: 10,
            decoded: 20,
            ..Default::default()
        };
        let (mut r, _) = report("https://a.test/x", Some(response));
        r.start = at(10);
        r.end = at(50);
        let e = entry_of(&r, base, 100.0);
        assert_eq!(e.start, 110.0);
        assert_eq!(e.fetch_start, 110.0);
        assert_eq!((e.domain_lookup_start, e.domain_lookup_end), (111.0, 113.0));
        assert_eq!((e.connect_start, e.secure_connection_start, e.connect_end), (113.0, 115.0, 120.0));
        assert_eq!((e.request_start, e.response_start, e.response_end), (120.0, 130.0, 150.0));
        assert_eq!((e.transfer_size, e.encoded_body_size, e.decoded_body_size), (310, 10, 20));
        assert_eq!((e.next_hop_protocol, e.response_status, e.content_type.as_str()), ("h2", 200, "text/plain"));
        assert_eq!(e.duration(), 40.0);
    }

    #[test]
    fn timing_allow_failure_hides_details() {
        let base = Instant::now();
        let timing = lp_net::NetTiming {
            start: Some(base),
            fetch_start: Some(base),
            request_start: Some(base + Duration::from_millis(20)),
            response_start: Some(base + Duration::from_millis(30)),
            timing_allow_failed: true,
            ..Default::default()
        };
        let response = ReportedResponse { timing, status: 200, encoded: 5, decoded: 5, ..Default::default() };
        let (r, base) = report("https://b.test/x", Some(response));
        let e = entry_of(&r, base, 0.0);
        assert_eq!((e.start, e.fetch_start, e.response_end), (10.0, 10.0, 50.0));
        assert_eq!((e.request_start, e.response_start, e.connect_start, e.domain_lookup_start), (0.0, 0.0, 0.0, 0.0));
        assert_eq!((e.transfer_size, e.encoded_body_size, e.decoded_body_size), (0, 0, 0));
        assert_eq!(e.next_hop_protocol, "");
    }

    #[test]
    fn network_error_keeps_start_and_end() {
        let (r, base) = report("http://a.test/x", None);
        let e = entry_of(&r, base, 0.0);
        assert_eq!((e.start, e.fetch_start, e.response_end, e.response_status), (10.0, 10.0, 50.0, 0));
    }
}
