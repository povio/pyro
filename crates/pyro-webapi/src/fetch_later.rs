//! `window.fetchLater(input, init)` and `FetchLaterResult` (Fetch, "deferred fetching"): a
//! request the document queues now and the browser sends later, when its `activateAfter`
//! timeout passes or when the document goes away (unloaded, its frame removed, discarded),
//! whichever comes first. An aborted signal drops it unsent.
//!
//! Queued requests count against the deferred-fetch quota of the document's control document (the
//! document itself, or for a frame of the same origin as its parent, the parent's control
//! document): [`QUOTA_TOP_LEVEL`] for a top-level control document, [`QUOTA_MINIMAL`] for a frame
//! of another origin than its parent, and at most [`QUOTA_PER_ORIGIN`] for the records to one
//! reporting origin (the request URL's origin), by each request's total length (URL, body, header
//! names and values). Pyro keeps no back/forward cache, so "deactivated" is "unloaded" here.

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
/// The quota of a frame whose origin is not its parent's: Fetch's minimal quota, which its
/// permissions policy (`deferred-fetch-minimal`, allowed everywhere by default) grants.
pub(crate) const QUOTA_MINIMAL: usize = 8 * 1024;

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
    /// The document that queued it ([`PageDom::id`]).
    owner: u64,
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

/// The pending deferred fetches of a control document and the documents that share its quota, in
/// the order they were queued, and that quota.
struct Pool {
    quota: usize,
    records: RefCell<Vec<Rc<DeferredRecord>>>,
}

impl Pool {
    fn new(quota: usize) -> Rc<Pool> {
        Rc::new(Pool { quota, records: RefCell::new(Vec::new()) })
    }
    /// Total length of the pending records, and of those to `origin`.
    fn used(&self, origin: &str) -> (usize, usize) {
        self.records.borrow().iter().filter(|r| r.pending()).fold((0, 0), |(all, same), r| {
            (all + r.length, same + if r.origin == origin { r.length } else { 0 })
        })
    }
    fn prune(&self) {
        self.records.borrow_mut().retain(|r| r.pending());
    }
}

/// A document's share of the deferred-fetch quota.
pub(crate) struct DeferredFetches {
    pool: RefCell<Rc<Pool>>,
}

impl Default for DeferredFetches {
    fn default() -> Self {
        DeferredFetches { pool: RefCell::new(Pool::new(QUOTA_TOP_LEVEL)) }
    }
}

impl DeferredFetches {
    fn pool(&self) -> Rc<Pool> {
        self.pool.borrow().clone()
    }
}

/// Sets the quota of `child`, a new document in a frame of `parent`'s document: `parent`'s
/// control document's when their origins are the same or the container reserved the normal
/// quota for it and "deferred-fetch" is enabled for its origin (`delegated`: `Some(true)`); none
/// when the container reserved the normal quota for the origin it navigated to but the document
/// ended up at another one (`Some(false)`: no minimal one either, as Fetch's "reserve
/// deferred-fetch quota" does); else (or with no parent document) a minimal quota of its own,
/// also when `allow` names "deferred-fetch" without enabling it (`'none'`). Called by the
/// embedder before the document runs script.
pub fn join_quota(child: &PageDom, parent: Option<&PageDom>, same_origin: bool, delegated: Option<bool>) {
    let pool = match parent {
        Some(parent) if same_origin || delegated == Some(true) => parent.fetch_state.deferred.pool(),
        Some(_) if delegated == Some(false) => Pool::new(0),
        _ => Pool::new(QUOTA_MINIMAL),
    };
    *child.fetch_state.deferred.pool.borrow_mut() = pool;
}

/// Fetch "total request length": the URL's serialization without its fragment, the referrer's
/// serialization when it is a URL, the body's length, and each header's name and value.
fn total_request_length(request: &Request) -> usize {
    let mut url = request.url.clone();
    url.set_fragment(None);
    url.as_str().len()
        + request.referrer.as_ref().map_or(0, |r| r.as_str().len())
        + request.body.as_ref().map_or(0, |b| b.len())
        + request.headers.iter().map(|(name, value)| name.as_str().len() + value.as_bytes().len()).sum::<usize>()
}

/// Fetch "available deferred-fetch quota" for a request to `origin` from `page`'s document.
fn available_quota(page: &PageDom, origin: &str) -> usize {
    let pool = page.fetch_state.deferred.pool();
    let (all, same) = pool.used(origin);
    pool.quota.saturating_sub(all).min(QUOTA_PER_ORIGIN.saturating_sub(same))
}

/// Fetch "process a deferred fetch": sends the record's request, once, outside the document's
/// fetch group (it outlives the document, as a keepalive request does), and sets `activated`.
fn process(page: &PageDom, record: &DeferredRecord) {
    let Some(mut request) = record.request.borrow_mut().take() else { return };
    record.abort.borrow_mut().take();
    record.activated.set(true);
    page.fetch_state.deferred.pool().prune();
    let Some(net) = page.net.borrow().clone() else { return };
    page.apply_request_defaults(&mut request);
    net.fetch(request, Box::new(|_| {}));
}

/// Sends every pending deferred fetch of `page`'s document: it was unloaded, its frame was
/// removed, or it is being discarded.
pub(crate) fn flush(page: &PageDom) {
    let id = page.id();
    let records: Vec<_> =
        page.fetch_state.deferred.pool().records.borrow().iter().filter(|r| r.owner == id).cloned().collect();
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
    // DeferredRequestInit conversion: `activateAfter` is a finite double (a TypeError before the
    // request is made); a negative one is a RangeError once the request is made.
    let activate_after = if init.is_object() {
        match crate::fetch::option(s, init, "activateAfter")? {
            v if v.is_undefined() => None,
            v => {
                let ms = v.to_number()?;
                if !ms.is_finite() {
                    return Err(JsError::Type("fetchLater: activateAfter is not a finite number.".into()));
                }
                Some(ms)
            }
        }
    } else {
        None
    };
    // The request as `new Request(input, init)` makes it; a ReadableStream body (its bytes are
    // not known now) is a TypeError there.
    let (request, signal, _extras) = crate::fetch::parse_request(s, &page, input, init, true)?;
    if let Some(signal) = &signal {
        let signal = s.unwrap::<AbortSignal>(signal.get(s)).expect("validated signal");
        if signal.aborted() {
            return Err(JsError::thrown(pyro_js::Persistent::new(s, signal.reason(s))));
        }
    }
    if !matches!(request.url.scheme(), "http" | "https") || !pyro_net::is_potentially_trustworthy_url(&request.url) {
        return Err(JsError::Type("fetchLater: the request URL must be a potentially trustworthy HTTP(S) URL.".into()));
    }
    if activate_after.is_some_and(|ms| ms < 0.0) {
        return Err(JsError::Range("fetchLater: activateAfter must not be negative.".into()));
    }
    // Fetch "queue a deferred fetch": a keepalive request. pyro-net's Request has no keepalive
    // flag; what it means is how `process` sends it: outside the document's fetch group, so it
    // outlives the document, and without the document's keepalive quota.
    let origin = request.url.origin().ascii_serialization();
    let length = total_request_length(&request);
    let available = available_quota(&page, &origin);
    if available < length {
        return Err(quota_exceeded(s, length, available));
    }
    let activated = Rc::new(Cell::new(false));
    let record = Rc::new(DeferredRecord {
        owner: page.id(),
        request: RefCell::new(Some(request)),
        length,
        origin,
        activated: activated.clone(),
        abort: RefCell::new(None),
    });
    page.fetch_state.deferred.pool().records.borrow_mut().push(record.clone());
    if let Some(signal) = &signal {
        let signal = s.unwrap::<AbortSignal>(signal.get(s)).expect("validated signal");
        let weak = Rc::downgrade(&record);
        let registration = signal.register_native(Box::new(move |page: &PageDom| {
            if let Some(record) = weak.upgrade() {
                record.request.borrow_mut().take();
                page.fetch_state.deferred.pool().prune();
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
    // WebIDL: the operation's `length` is its required argument count.
    let define = s.global().get_atom("Object")?.as_object().map(|o| o.get_atom("defineProperty")).transpose()?;
    if let Some(define) = define.and_then(|d| d.as_object()) {
        let descriptor = s.object();
        descriptor.set_atom("value", s.number(1.0))?;
        descriptor.set_atom("configurable", s.boolean(true))?;
        define.call(None, &[function.value(), s.string("length"), descriptor.value()])?;
    }
    s.global().set_atom("fetchLater", function.value())
}
