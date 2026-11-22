//! `window.fetchLater(input, init)` and `FetchLaterResult` (Fetch, "deferred fetching"): a
//! request the document queues now and the browser sends later, when its `activateAfter`
//! timeout passes or when the document goes away (unloaded, its frame removed, discarded),
//! whichever comes first. An aborted signal drops it unsent.
//!
//! Queued requests count against the deferred-fetch quota: [`QUOTA_TOP_LEVEL`] for all of a
//! top-level document's records and [`QUOTA_PER_ORIGIN`] for the records to one reporting
//! origin (the request URL's origin), by each request's total length (URL, body, header names
//! and values). Pyro keeps no back/forward cache, so "deactivated" is "unloaded" here.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use pyro_js::{ClassDef, JsClass, JsError, JsResult, NoParent, Scope, Value};
use pyro_net::Request;

use crate::abort::{AbortRegistration, AbortSignal};
use crate::page::PageDom;

/// The quota of a top-level document's deferred fetches: 640 KiB, less the 128 KiB Fetch
/// reserves for its frames' minimal quotas.
pub(crate) const QUOTA_TOP_LEVEL: usize = 512 * 1024;
/// The quota of the deferred fetches to one reporting origin (Fetch's `quotaForRequestOrigin`).
pub(crate) const QUOTA_PER_ORIGIN: usize = 64 * 1024;

pub struct FetchLaterResult;

pub struct FetchLaterResultData {
    activated: Rc<Cell<bool>>,
}

impl JsClass for FetchLaterResult {
    const NAME: &'static str = "FetchLaterResult";
    type Data = FetchLaterResultData;
    type Parent = NoParent;

    fn define(def: &mut ClassDef<Self>) {
        def.getter("activated", |s, d| Ok(s.boolean(d.activated.get())));
    }
}

pub(crate) fn register(engine: &pyro_js::Engine) {
    engine.register::<FetchLaterResult>();
}

/// A deferred fetch record: its request until it is sent or aborted.
pub(crate) struct DeferredRecord {
    request: RefCell<Option<Request>>,
    /// Fetch's "total request length".
    length: usize,
    /// The request URL's origin, its reporting origin.
    origin: String,
    activated: Rc<Cell<bool>>,
    abort: RefCell<Option<AbortRegistration>>,
}

impl DeferredRecord {
    fn pending(&self) -> bool {
        self.request.borrow().is_some()
    }
}

/// Deferred fetch records of a document, in the order they were queued.
#[derive(Default)]
pub(crate) struct DeferredFetches {
    records: RefCell<Vec<Rc<DeferredRecord>>>,
}

impl DeferredFetches {
    /// Total length of the pending records, and of those to `origin`.
    fn used(&self, origin: &str) -> (usize, usize) {
        self.records.borrow().iter().filter(|r| r.pending()).fold((0, 0), |(all, same), r| {
            (all + r.length, same + if r.origin == origin { r.length } else { 0 })
        })
    }
}

/// Fetch "total request length": the URL's serialization, the body's length, and each header's
/// name and value.
fn total_request_length(request: &Request) -> usize {
    request.url.as_str().len()
        + request.body.as_ref().map_or(0, |b| b.len())
        + request.headers.iter().map(|(name, value)| name.as_str().len() + value.as_bytes().len()).sum::<usize>()
}

/// Fetch "available deferred-fetch quota" for a request to `origin` from `page`'s document.
fn available_quota(page: &PageDom, origin: &str) -> usize {
    let (all, same) = page.fetch_state.deferred.used(origin);
    QUOTA_TOP_LEVEL.saturating_sub(all).min(QUOTA_PER_ORIGIN.saturating_sub(same))
}

/// Fetch "process a deferred fetch": sends the record's request, once, outside the document's
/// fetch group (it outlives the document, as a keepalive request does), and sets `activated`.
fn process(page: &PageDom, record: &DeferredRecord) {
    let Some(mut request) = record.request.borrow_mut().take() else { return };
    record.abort.borrow_mut().take();
    record.activated.set(true);
    page.fetch_state.deferred.records.borrow_mut().retain(|r| r.pending());
    let Some(net) = page.net.borrow().clone() else { return };
    page.apply_request_defaults(&mut request);
    net.fetch(request, Box::new(|_| {}));
}

/// Sends every pending deferred fetch of `page`'s document: it was unloaded, its frame was
/// removed, or it is being discarded.
pub(crate) fn flush(page: &PageDom) {
    let records: Vec<_> = page.fetch_state.deferred.records.borrow().clone();
    for record in records {
        process(page, &record);
    }
}

/// `fetchLater(input, init)`.
fn fetch_later<'s>(s: &Scope<'s>, input: Value<'s>, init: Value<'s>) -> JsResult<Value<'s>> {
    let page = crate::target::page_of(s)?;
    if page.is_disposed() {
        return Err(JsError::ContextDestroyed);
    }
    // A ReadableStream body cannot be deferred: its bytes are not known now.
    if init.is_object() {
        let body = crate::fetch::option(s, init, "body")?;
        if !body.is_undefined() && !body.is_null() && crate::streams::is_stream(s, body)? {
            return Err(JsError::Type("fetchLater: a ReadableStream request body cannot be deferred.".into()));
        }
    }
    let (mut request, signal, _extras) = crate::fetch::parse_request(s, &page, input, init)?;
    if let Some(signal) = &signal {
        let signal = s.unwrap::<AbortSignal>(signal.get(s)).expect("validated signal");
        if signal.aborted() {
            return Err(JsError::thrown(pyro_js::Persistent::new(s, signal.reason(s))));
        }
    }
    if !matches!(request.url.scheme(), "http" | "https") || !pyro_net::is_potentially_trustworthy_url(&request.url) {
        return Err(JsError::Type("fetchLater: the request URL must be a potentially trustworthy HTTP(S) URL.".into()));
    }
    let activate_after = if init.is_object() {
        match crate::fetch::option(s, init, "activateAfter")? {
            v if v.is_undefined() => None,
            v => {
                let ms = v.to_number()?;
                if !ms.is_finite() {
                    return Err(JsError::Type("fetchLater: activateAfter is not a finite number.".into()));
                }
                if ms < 0.0 {
                    return Err(JsError::Range("fetchLater: activateAfter must not be negative.".into()));
                }
                Some(ms)
            }
        }
    } else {
        None
    };
    // Fetch "queue a deferred fetch": a keepalive request (`process` sends it outside the
    // document's fetch group, straight to the network).
    let origin = request.url.origin().ascii_serialization();
    let length = total_request_length(&request);
    let available = available_quota(&page, &origin);
    if available < length {
        return Err(quota_exceeded(s, length, available));
    }
    let activated = Rc::new(Cell::new(false));
    let record = Rc::new(DeferredRecord {
        request: RefCell::new(Some(request)),
        length,
        origin,
        activated: activated.clone(),
        abort: RefCell::new(None),
    });
    page.fetch_state.deferred.records.borrow_mut().push(record.clone());
    if let Some(signal) = &signal {
        let signal = s.unwrap::<AbortSignal>(signal.get(s)).expect("validated signal");
        let weak = Rc::downgrade(&record);
        let registration = signal.register_native(Box::new(move |page: &PageDom| {
            if let Some(record) = weak.upgrade() {
                record.request.borrow_mut().take();
                page.fetch_state.deferred.records.borrow_mut().retain(|r| r.pending());
            }
        }));
        *record.abort.borrow_mut() = registration;
    }
    // A timeout no `Duration` holds never passes: the document's end sends the request.
    if let Some(delay) = activate_after.and_then(|ms| std::time::Duration::try_from_secs_f64(ms / 1000.0).ok()) {
        let (page_weak, record_weak): (Weak<PageDom>, Weak<DeferredRecord>) =
            (Rc::downgrade(&page), Rc::downgrade(&record));
        page.post_delayed_task(
            delay,
            Box::new(move || {
                if let (Some(page), Some(record)) = (page_weak.upgrade(), record_weak.upgrade()) {
                    process(&page, &record);
                }
            }),
        );
    }
    Ok(s.wrap::<FetchLaterResult>(FetchLaterResultData { activated }, None)?.value())
}

/// A `QuotaExceededError` whose `requested` is the request's total length and `quota` the quota
/// still available (a plain DOMException of that name if page script replaced the interface).
fn quota_exceeded(s: &Scope<'_>, requested: usize, available: usize) -> JsError {
    const MESSAGE: &str = "fetchLater: the deferred-fetch quota is exceeded.";
    let made = (|| {
        let ctor = s.global().get("QuotaExceededError").ok()?.as_object()?;
        let options = s.object();
        options.set_atom("requested", s.number(requested as f64)).ok()?;
        options.set_atom("quota", s.number(available as f64)).ok()?;
        ctor.construct(&[s.string(MESSAGE), options.value()]).ok()
    })();
    match made {
        Some(error) => JsError::thrown(pyro_js::Persistent::new(s, error.value())),
        None => JsError::dom("QuotaExceededError", MESSAGE),
    }
}

/// Defines `fetchLater` on a window global ([SecureContext]: only a secure context's).
pub(crate) fn install(s: &Scope<'_>) -> JsResult<()> {
    let function = s.function("fetchLater", |s, _this, args| {
        args.require(1, "fetchLater")?;
        fetch_later(s, args.get(0), args.get(1))
    })?;
    s.global().set_atom("fetchLater", function.value())
}
