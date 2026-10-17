//! Callback-driven fetch bindings. Headers are delivered before body completion;
//! body readers buffer within explicit page/request limits. No stream API yet.
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

use crate::{
    abort::{AbortRegistration, AbortSignal},
    headers,
    page::{PageDom, RemotePoster},
    target::page_of,
    util::dom_string,
};
use bytes::Bytes;
use lp_js::{Deferred, JsError, JsResult, Persistent, RealmId, Scope, Value, WeakRealm};
use lp_net::http::{header, HeaderName, HeaderValue, Method};
use lp_net::{
    Credentials, Destination, NetError, RedirectMode, Request, RequestId, RequestMode, ResponseEvent, ResponseHead,
};
use rustc_hash::FxHashMap;

#[derive(Clone, Copy, Debug)]
pub struct FetchLimits {
    pub max_requests: usize,
    pub max_body_bytes: usize,
    /// Logical retained body bytes across live responses, including clones.
    pub max_total_body_bytes: usize,
    pub max_body_chunks: usize,
}
impl Default for FetchLimits {
    fn default() -> Self {
        Self {
            max_requests: 64,
            max_body_bytes: 16 * 1024 * 1024,
            max_total_body_bytes: 64 * 1024 * 1024,
            max_body_chunks: 8192,
        }
    }
}
#[derive(Clone, Debug, Default)]
pub struct RequestContext {
    pub frame_id: Option<String>,
    pub loader_id: Option<String>,
}

pub(crate) struct FetchState {
    limits: Cell<FetchLimits>,
    budget: Arc<Budget>,
    context: RefCell<RequestContext>,
    jobs: RefCell<FxHashMap<RequestId, Rc<Job>>>,
    bodies: RefCell<FxHashMap<RequestId, Weak<Body>>>,
}
impl Default for FetchState {
    fn default() -> Self {
        Self {
            limits: Cell::new(FetchLimits::default()),
            budget: Arc::new(Budget::default()),
            context: RefCell::new(RequestContext::default()),
            jobs: RefCell::new(FxHashMap::default()),
            bodies: RefCell::new(FxHashMap::default()),
        }
    }
}
struct Budget {
    used: AtomicUsize,
    limit: AtomicUsize,
}
impl Default for Budget {
    fn default() -> Self {
        Self { used: AtomicUsize::new(0), limit: AtomicUsize::new(FetchLimits::default().max_total_body_bytes) }
    }
}
impl Budget {
    fn reserve(&self, bytes: usize) -> bool {
        self.used
            .try_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                n.checked_add(bytes).filter(|n| *n <= self.limit.load(Ordering::Acquire))
            })
            .is_ok()
    }
}
struct Wire {
    head: Option<ResponseHead>,
    chunks: Vec<Bytes>,
    bytes: usize,
    terminal: Option<Result<(), NetError>>,
    closed: bool,
    scheduled: bool,
    null_body: bool,
    budget: Arc<Budget>,
}
impl Wire {
    fn clear_body(&mut self) {
        self.budget.used.fetch_sub(self.bytes, Ordering::AcqRel);
        self.bytes = 0;
        self.chunks = Vec::new();
    }
}
impl Drop for Wire {
    fn drop(&mut self) {
        self.clear_body();
    }
}

struct Job {
    realm: WeakRealm,
    realm_id: RealmId,
    promise: RefCell<Option<Deferred>>,
    signal: Option<Persistent>,
    credentials: Credentials,
    body: Rc<Body>,
}
struct Reader {
    realm: WeakRealm,
    promise: Deferred,
    kind: ReadKind,
}
#[derive(Clone, Copy)]
pub(crate) enum ReadKind {
    Text,
    Json,
    ArrayBuffer,
    Bytes,
    Blob,
}
pub(crate) struct Body {
    wire: Arc<Mutex<Wire>>,
    page: Weak<PageDom>,
    id: RequestId,
    realm_id: RealmId,
    readers: RefCell<Vec<Reader>>,
    consumers: Cell<usize>,
    abort_registration: RefCell<Option<AbortRegistration>>,
    abort_reason: RefCell<Weak<RefCell<Option<Persistent>>>>,
    aborted: Cell<bool>,
    disposed: Cell<bool>,
    pub(crate) mime: RefCell<String>,
}
impl Drop for Body {
    fn drop(&mut self) {
        if let Some(page) = self.page.upgrade() {
            page.fetch_state.bodies.borrow_mut().remove(&self.id);
        }
    }
}
thread_local! {
    // Only weak routing references cross tasks; no JS/Rc crosses the I/O thread.
    static PAGES: RefCell<FxHashMap<u64, Weak<PageDom>>> = RefCell::new(FxHashMap::default());
}

impl PageDom {
    /// Set before starting fetches. Returns false while bodies/jobs remain.
    pub fn set_fetch_limits(&self, limits: FetchLimits) -> bool {
        if !self.fetch_state.jobs.borrow().is_empty() || !self.fetch_state.bodies.borrow().is_empty() {
            return false;
        }
        self.fetch_state.limits.set(limits);
        self.fetch_state.budget.limit.store(limits.max_total_body_bytes, Ordering::Release);
        true
    }
    /// Embedder supplies exact frame/current document loader IDs for CDP routing.
    pub fn set_request_context(&self, context: RequestContext) {
        *self.fetch_state.context.borrow_mut() = context;
    }
    /// Active network jobs and logical retained body bytes (diagnostics).
    pub fn fetch_usage(&self) -> (usize, usize) {
        (self.fetch_state.jobs.borrow().len(), self.fetch_state.budget.used.load(Ordering::Acquire))
    }
}

fn option<'s>(s: &Scope<'s>, init: Value<'s>, key: &str) -> JsResult<Value<'s>> {
    match init.as_object() {
        Some(o) => o.get(key),
        None if init.is_null() || init.is_undefined() => Ok(s.undefined()),
        _ => Err(JsError::Type("fetch init must be an object".into())),
    }
}
fn string_option(s: &Scope<'_>, init: Value<'_>, key: &str, default: &str) -> JsResult<String> {
    let v = option(s, init, key)?;
    if v.is_undefined() {
        Ok(default.into())
    } else {
        dom_string(v)
    }
}
fn forbidden_header(name: &str) -> bool {
    matches!(
        name,
        "accept-charset"
            | "accept-encoding"
            | "access-control-request-headers"
            | "access-control-request-method"
            | "connection"
            | "content-length"
            | "cookie"
            | "cookie2"
            | "date"
            | "dnt"
            | "expect"
            | "host"
            | "keep-alive"
            | "origin"
            | "referer"
            | "set-cookie"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
            | "via"
    ) || name.starts_with("proxy-")
        || name.starts_with("sec-")
}
fn parse_request<'s>(
    s: &Scope<'s>,
    page: &PageDom,
    input: Value<'s>,
    init: Value<'s>,
) -> JsResult<(Request, Option<Persistent>)> {
    let input = dom_string(input)?;
    let resolved = crate::html::resolve_url(&page.dom.borrow(), page.document(), &input);
    let mut url = lp_net::Url::parse(&resolved).map_err(|_| JsError::Type("Invalid fetch URL".into()))?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err(JsError::Type("Fetch URL contains credentials".into()));
    }
    url.set_fragment(None);
    let mut request = Request::get(url, Destination::Fetch);
    let method = string_option(s, init, "method", "GET")?;
    let upper = method.to_ascii_uppercase();
    if matches!(upper.as_str(), "CONNECT" | "TRACE" | "TRACK") {
        return Err(JsError::Type("Forbidden fetch method".into()));
    }
    request.method =
        Method::from_bytes(if matches!(upper.as_str(), "DELETE" | "GET" | "HEAD" | "OPTIONS" | "POST" | "PUT") {
            upper.as_bytes()
        } else {
            method.as_bytes()
        })
        .map_err(|_| JsError::Type("Invalid fetch method".into()))?;
    request.mode = match string_option(s, init, "mode", "cors")?.as_str() {
        "cors" => RequestMode::Cors,
        "same-origin" => RequestMode::SameOrigin,
        "no-cors" => RequestMode::NoCors,
        _ => return Err(JsError::Type("Invalid fetch mode".into())),
    };
    request.credentials = match string_option(s, init, "credentials", "same-origin")?.as_str() {
        "omit" => Credentials::Omit,
        "same-origin" => Credentials::SameOrigin,
        "include" => Credentials::Include,
        _ => return Err(JsError::Type("Invalid fetch credentials".into())),
    };
    request.redirect = match string_option(s, init, "redirect", "follow")?.as_str() {
        "follow" => RedirectMode::Follow,
        "error" => RedirectMode::Error,
        "manual" => RedirectMode::Manual,
        _ => return Err(JsError::Type("Invalid fetch redirect mode".into())),
    };
    if request.mode == RequestMode::NoCors && !matches!(request.method, Method::GET | Method::HEAD | Method::POST) {
        return Err(JsError::Type("Invalid no-cors method".into()));
    }
    let headers = headers::from_init(s, option(s, init, "headers")?)?;
    for (name, value) in headers.pairs() {
        if forbidden_header(&name) {
            continue;
        }
        if matches!(name.as_str(), "x-http-method" | "x-http-method-override" | "x-method-override")
            && value.split(',').any(|m| matches!(m.trim().to_ascii_uppercase().as_str(), "CONNECT" | "TRACE" | "TRACK"))
        {
            continue;
        }
        let bytes: Vec<u8> = value.chars().map(|c| c as u8).collect();
        let key = HeaderName::from_bytes(name.as_bytes()).map_err(|_| JsError::Type("Invalid header".into()))?;
        let value = HeaderValue::from_bytes(&bytes).map_err(|_| JsError::Type("Invalid header value".into()))?;
        request.headers.append(key, value);
    }
    let body = option(s, init, "body")?;
    if !body.is_null() && !body.is_undefined() {
        if matches!(request.method, Method::GET | Method::HEAD) {
            return Err(JsError::Type("GET/HEAD cannot have a body".into()));
        }
        let (bytes, mime) = if let Some(blob) = s.unwrap::<crate::file::Blob>(body) {
            (blob.bytes.clone(), blob.mime.clone())
        } else if let Some(bytes) = crate::encoding::buffer_source_bytes(s, body)? {
            (Bytes::from(bytes), String::new())
        } else {
            (Bytes::from(dom_string(body)?), "text/plain;charset=UTF-8".into())
        };
        if !mime.is_empty() && !request.headers.contains_key(header::CONTENT_TYPE) {
            request.headers.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_str(&mime).map_err(|_| JsError::Type("Invalid body MIME type".into()))?,
            );
        }
        request.body = Some(bytes);
    }
    // No-cors guarding uses the same native safelist as preflight decisions.
    if request.mode == RequestMode::NoCors {
        let unsafe_names = lp_net::cors::unsafe_header_names(&request.headers);
        for name in unsafe_names {
            request.headers.remove(name);
        }
    }
    for key in ["integrity", "referrer", "referrerPolicy"] {
        let value = option(s, init, key)?;
        if !value.is_undefined() && !dom_string(value)?.is_empty() {
            return Err(JsError::Type(format!("fetch {key} is not implemented")));
        }
    }
    if option(s, init, "keepalive")?.to_bool() {
        return Err(JsError::Type("fetch keepalive is not implemented".into()));
    }
    let cache = string_option(s, init, "cache", "default")?;
    if cache != "default" && cache != "no-store" {
        return Err(JsError::Type("fetch cache mode is not implemented".into()));
    }
    let signal = option(s, init, "signal")?;
    let signal = if signal.is_null() || signal.is_undefined() {
        None
    } else {
        if s.unwrap::<AbortSignal>(signal).is_none() {
            return Err(JsError::Type("signal must be an AbortSignal".into()));
        }
        Some(Persistent::new(s, signal))
    };
    let document_url = page.dom.borrow().document_data(page.document()).map(|d| d.url.to_string()).unwrap_or_default();
    request.origin = Some(s.realm().origin().map(str::to_owned).unwrap_or_else(|| {
        lp_net::Url::parse(&document_url).map(|u| u.origin().ascii_serialization()).unwrap_or_else(|_| "null".into())
    }));
    let context = page.fetch_state.context.borrow();
    request.frame_id = context.frame_id.clone();
    request.loader_id = context.loader_id.clone();
    Ok((request, signal))
}

pub(crate) fn install(s: &Scope<'_>) -> JsResult<()> {
    let function = s.function("fetch", |s, _this, args| {
        let deferred = s.deferred()?;
        let promise = deferred.promise.get(s);
        let result = (|| {
            args.require(1, "fetch")?;
            let page = page_of(s)?;
            if page.is_disposed() {
                return Err(JsError::ContextDestroyed);
            }
            let (request, signal) = parse_request(s, &page, args.get(0), args.get(1))?;
            // Init conversion can execute script, including embedder teardown.
            if page.is_disposed() {
                return Err(JsError::ContextDestroyed);
            }
            if let Some(signal) = &signal {
                let signal = s.unwrap::<AbortSignal>(signal.get(s)).expect("validated signal");
                if signal.aborted() {
                    return Err(JsError::Thrown(Persistent::new(s, signal.reason(s))));
                }
            }
            let limits = page.fetch_state.limits.get();
            if page.fetch_state.jobs.borrow().len() >= limits.max_requests {
                return Err(JsError::Type("Too many pending fetches".into()));
            }
            let net =
                page.net.borrow().clone().ok_or_else(|| JsError::Type("Page networking is not installed".into()))?;
            let poster =
                page.remote_poster().ok_or_else(|| JsError::Type("Browser task poster is not installed".into()))?;
            Ok((page, request, signal, limits, net, poster))
        })();
        match result {
            Err(error) => deferred.reject(s, error.to_value(s))?,
            Ok((page, mut request, signal, limits, net, poster)) => {
                request.max_body = Some(limits.max_body_bytes);
                let id = net.next_request_id();
                let wire = Arc::new(Mutex::new(Wire {
                    head: None,
                    chunks: Vec::new(),
                    bytes: 0,
                    terminal: None,
                    closed: false,
                    scheduled: false,
                    null_body: request.method == Method::HEAD,
                    budget: page.fetch_state.budget.clone(),
                }));
                let body = Rc::new(Body {
                    wire: wire.clone(),
                    page: Rc::downgrade(&page),
                    id,
                    realm_id: s.realm_id(),
                    readers: RefCell::new(Vec::new()),
                    consumers: Cell::new(0),
                    abort_registration: RefCell::new(None),
                    abort_reason: RefCell::new(Weak::new()),
                    aborted: Cell::new(false),
                    disposed: Cell::new(false),
                    mime: RefCell::new(String::new()),
                });
                page.fetch_state.bodies.borrow_mut().insert(id, Rc::downgrade(&body));
                if let Some(signal) = &signal {
                    let signal = s.unwrap::<AbortSignal>(signal.get(s)).expect("validated signal");
                    *body.abort_reason.borrow_mut() = signal.reason_weak();
                    let weak = Rc::downgrade(&body);
                    *body.abort_registration.borrow_mut() = signal.register_native(Box::new(move |_| {
                        if let Some(body) = weak.upgrade() {
                            body.abort();
                        }
                    }));
                }
                let job = Rc::new(Job {
                    realm: s.realm().downgrade(),
                    realm_id: s.realm_id(),
                    promise: RefCell::new(Some(deferred)),
                    signal,
                    credentials: request.credentials,
                    body,
                });
                page.fetch_state.jobs.borrow_mut().insert(id, job);
                let page_id = page.id();
                PAGES.with(|pages| pages.borrow_mut().insert(page_id, Rc::downgrade(&page)));
                net.fetch_with_id(
                    id,
                    request,
                    Box::new(move |event| receive(&wire, page_id, id, &poster, limits, event)),
                );
            }
        }
        Ok(promise)
    })?;
    s.global().set("fetch", function.value())
}

fn receive(
    wire: &Arc<Mutex<Wire>>,
    page: u64,
    id: RequestId,
    poster: &RemotePoster,
    limits: FetchLimits,
    event: ResponseEvent,
) {
    let wake = {
        let mut wire = wire.lock().unwrap();
        if wire.closed {
            return;
        }
        let notify = match event {
            ResponseEvent::Redirect { .. } => false,
            ResponseEvent::Response(head) => {
                wire.null_body |=
                    matches!(head.kind, lp_net::ResponseKind::Opaque | lp_net::ResponseKind::OpaqueRedirect)
                        || matches!(head.status.as_u16(), 204 | 205 | 304);
                wire.head = Some(head);
                true
            }
            ResponseEvent::Data(bytes) => {
                if wire.null_body || bytes.is_empty() {
                    false
                } else if wire.chunks.len() >= limits.max_body_chunks
                    || wire.bytes.saturating_add(bytes.len()) > limits.max_body_bytes
                    || !wire.budget.reserve(bytes.len())
                {
                    wire.terminal = Some(Err(NetError::TooLarge));
                    wire.closed = true;
                    wire.clear_body();
                    true
                } else {
                    wire.bytes += bytes.len();
                    wire.chunks.push(bytes);
                    false
                }
            }
            ResponseEvent::Finished { .. } => {
                wire.terminal = Some(Ok(()));
                wire.closed = true;
                true
            }
            ResponseEvent::Failed(error) => {
                wire.terminal = Some(Err(error));
                wire.closed = true;
                wire.clear_body();
                true
            }
        };
        if notify && !wire.scheduled {
            wire.scheduled = true;
            true
        } else {
            false
        }
    };
    if wake {
        poster(Box::new(move || pump(page, id)));
    }
}

fn pump(page_id: u64, id: RequestId) {
    let page = PAGES.with(|pages| pages.borrow().get(&page_id).and_then(Weak::upgrade));
    let Some(page) = page else { return };
    let job = page.fetch_state.jobs.borrow().get(&id).cloned();
    let Some(job) = job else { return };
    let Some(realm) = job.realm.upgrade() else {
        cancel_realm(&page, job.realm_id);
        return;
    };
    let Ok(s) = realm.scope() else {
        cancel_realm(&page, job.realm_id);
        return;
    };
    let (head, terminal) = {
        let mut wire = job.body.wire.lock().unwrap();
        wire.scheduled = false;
        (wire.head.take(), wire.terminal.clone())
    };
    if let Some(head) = head {
        *job.body.mime.borrow_mut() =
            if matches!(head.kind, lp_net::ResponseKind::Opaque | lp_net::ResponseKind::OpaqueRedirect) {
                String::new()
            } else {
                head.headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("").to_owned()
            };
        let deferred = job.promise.borrow_mut().take();
        if let Some(deferred) = deferred {
            let result = crate::response::wrap(
                &s,
                &head,
                job.body.clone(),
                job.credentials,
                job.signal.as_ref().map(|v| v.get(&s)),
            );
            match result {
                Ok(value) => {
                    let _ = deferred.resolve(&s, value);
                }
                Err(error) => {
                    let _ = deferred.reject(&s, error.to_value(&s));
                }
            }
        }
    }
    if let Some(result) = terminal {
        if result.is_err() {
            if let Some(net) = page.net.borrow().as_ref() {
                net.cancel(id);
            }
        }
        let deferred = job.promise.borrow_mut().take();
        if let Some(deferred) = deferred {
            let error = result.err().unwrap_or_else(|| NetError::Other("Missing response headers".into()));
            let _ = deferred.reject(&s, JsError::Type(error.to_string()).to_value(&s));
        }
        page.fetch_state.jobs.borrow_mut().remove(&id);
        job.body.settle_readers();
        job.body.release_if_unused();
    }
}

impl Body {
    pub(crate) fn add_consumer(&self) {
        if !self.is_null() {
            self.consumers.set(self.consumers.get() + 1);
        }
    }
    pub(crate) fn release_consumer(&self) {
        if self.is_null() {
            self.release_if_unused();
            return;
        }
        let count = self.consumers.get();
        debug_assert!(count > 0);
        self.consumers.set(count.saturating_sub(1));
        self.release_if_unused();
    }
    fn release_if_unused(&self) {
        if self.consumers.get() != 0 {
            return;
        }
        let finished = {
            let mut wire = self.wire.lock().unwrap();
            if wire.terminal.is_some() {
                wire.clear_body();
                true
            } else {
                false
            }
        };
        if finished {
            drop(self.abort_registration.take());
        }
    }
    pub(crate) fn is_null(&self) -> bool {
        self.wire.lock().unwrap().null_body
    }
    fn abort_error<'s>(&self, s: &Scope<'s>) -> Value<'s> {
        let reason = self.abort_reason.borrow().upgrade();
        reason
            .as_ref()
            .and_then(|r| r.borrow().as_ref().map(|v| v.get(s)))
            .unwrap_or_else(|| JsError::dom("AbortError", "Fetch aborted").to_value(s))
    }
    fn error<'s>(&self, s: &Scope<'s>) -> Option<Value<'s>> {
        // A null body has no stream for abort or network failure to error.
        if self.is_null() {
            return None;
        }
        if self.aborted.get() {
            return Some(self.abort_error(s));
        }
        let error = self.wire.lock().unwrap().terminal.as_ref().and_then(|r| r.as_ref().err().cloned());
        if self.disposed.get() {
            Some(JsError::dom("AbortError", "Document disposed").to_value(s))
        } else {
            error.map(|e| JsError::Type(e.to_string()).to_value(s))
        }
    }
    pub(crate) fn read<'s>(&self, s: &Scope<'s>, kind: ReadKind, already_used: bool) -> JsResult<Value<'s>> {
        let deferred = s.deferred()?;
        let promise = deferred.promise.get(s);
        let ready = self.is_null() || self.wire.lock().unwrap().terminal.is_some();
        if already_used {
            deferred.reject(s, JsError::Type("Response body has already been consumed".into()).to_value(s))?;
        } else if let Some(error) = self.error(s) {
            deferred.reject(s, error)?;
            self.release_consumer();
        } else if ready {
            self.settle(s, deferred, kind);
        } else if self.readers.borrow().len() >= 1024 {
            deferred.reject(s, JsError::Type("Too many pending body readers".into()).to_value(s))?;
            self.release_consumer();
        } else {
            self.readers.borrow_mut().push(Reader { realm: s.realm().downgrade(), promise: deferred, kind });
        }
        Ok(promise)
    }
    fn settle(&self, s: &Scope<'_>, deferred: Deferred, kind: ReadKind) {
        if let Some(error) = self.error(s) {
            let _ = deferred.reject(s, error);
            self.release_consumer();
            return;
        }
        let bytes = {
            let wire = self.wire.lock().unwrap();
            let mut bytes = Vec::with_capacity(wire.bytes);
            for chunk in &wire.chunks {
                bytes.extend_from_slice(chunk);
            }
            bytes
        };
        let result = match kind {
            ReadKind::Text => Ok(
                s.string(String::from_utf8_lossy(bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes)).as_ref())
            ),
            ReadKind::Json => {
                let text = lp_js::JsString::new(
                    String::from_utf8_lossy(bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes)).as_ref(),
                );
                // SAFETY: local string/context are live; the returned value is
                // immediately wrapped and settled before any other JS call.
                let raw = unsafe { lp_js::sys::JSValueMakeFromJSONString(s.ctx(), text.as_raw()) };
                if raw.is_null() {
                    Err(JsError::Syntax("Invalid JSON response body".into()))
                } else {
                    Ok(unsafe { Value::from_raw(s.ctx(), raw) })
                }
            }
            ReadKind::ArrayBuffer => crate::encoding::array_buffer(s, &bytes),
            ReadKind::Bytes => crate::encoding::uint8_array(s, &bytes),
            ReadKind::Blob => s
                .wrap::<crate::file::Blob>(
                    crate::file::BlobData { bytes: Bytes::from(bytes), mime: self.mime.borrow().clone(), file: None },
                    None,
                )
                .map(|o| o.value()),
        };
        match result {
            Ok(value) => {
                let _ = deferred.resolve(s, value);
            }
            Err(error) => {
                let _ = deferred.reject(s, error.to_value(s));
            }
        }
        self.release_consumer();
    }
    fn settle_readers(&self) {
        let readers = self.readers.take();
        for reader in readers {
            if let Some(realm) = reader.realm.upgrade() {
                if let Ok(s) = realm.scope() {
                    self.settle(&s, reader.promise, reader.kind);
                    continue;
                }
            }
            self.release_consumer();
        }
    }
    fn abort(&self) {
        if self.aborted.replace(true) || self.disposed.get() {
            return;
        }
        {
            let mut wire = self.wire.lock().unwrap();
            wire.closed = true;
            wire.clear_body();
            wire.terminal = Some(Err(NetError::Aborted));
        }
        if let Some(page) = self.page.upgrade() {
            if let Some(net) = page.net.borrow().as_ref() {
                net.cancel(self.id);
            }
            let job = page.fetch_state.jobs.borrow_mut().remove(&self.id);
            if let Some(job) = job {
                let deferred = job.promise.borrow_mut().take();
                if let (Some(deferred), Some(realm)) = (deferred, job.realm.upgrade()) {
                    if let Ok(s) = realm.scope() {
                        // Fetch itself still rejects if aborted before headers,
                        // even when its response has no body stream to error.
                        let _ = deferred.reject(&s, self.abort_error(&s));
                    }
                }
            }
        }
        self.settle_readers();
    }
    fn dispose(&self) {
        self.disposed.set(true);
        {
            let mut wire = self.wire.lock().unwrap();
            wire.closed = true;
            wire.clear_body();
            wire.head = None;
            wire.terminal = Some(Err(NetError::Aborted));
        }
        drop(self.abort_registration.take());
        let readers = self.readers.take();
        for _ in &readers {
            self.release_consumer();
        }
        drop(readers);
    }
}

pub(crate) fn cancel_realm(page: &PageDom, realm: RealmId) {
    let bodies: Vec<_> =
        page.fetch_state.bodies.borrow().values().filter_map(Weak::upgrade).filter(|b| b.realm_id == realm).collect();
    for body in bodies {
        if let Some(net) = page.net.borrow().as_ref() {
            net.cancel(body.id);
        }
        let job = page.fetch_state.jobs.borrow_mut().remove(&body.id);
        body.dispose();
        drop(job);
    }
}
pub(crate) fn dispose(page: &PageDom) {
    PAGES.with(|pages| pages.borrow_mut().remove(&page.id()));
    let bodies: Vec<_> = page.fetch_state.bodies.borrow().values().filter_map(Weak::upgrade).collect();
    for body in bodies {
        if let Some(net) = page.net.borrow().as_ref() {
            net.cancel(body.id);
        }
        body.dispose();
    }
    drop(page.fetch_state.jobs.take());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_delivery_bounds_chunks_and_coalesces_wakes() {
        let budget = Arc::new(Budget::default());
        let wire = Arc::new(Mutex::new(Wire {
            head: None,
            chunks: Vec::new(),
            bytes: 0,
            terminal: None,
            closed: false,
            scheduled: false,
            null_body: false,
            budget: budget.clone(),
        }));
        let posts = Arc::new(AtomicUsize::new(0));
        let counted = posts.clone();
        let poster: RemotePoster = Arc::new(move |_| {
            counted.fetch_add(1, Ordering::Relaxed);
        });
        let limits = FetchLimits { max_body_chunks: 3, ..Default::default() };
        for _ in 0..3 {
            receive(&wire, 0, RequestId(1), &poster, limits, ResponseEvent::Data(Bytes::from_static(b"x")));
        }
        assert_eq!(posts.load(Ordering::Relaxed), 0, "data chunks must not post browser tasks");
        assert_eq!(budget.used.load(Ordering::Relaxed), 3);
        receive(&wire, 0, RequestId(1), &poster, limits, ResponseEvent::Data(Bytes::from_static(b"x")));
        assert_eq!(posts.load(Ordering::Relaxed), 1);
        assert_eq!(budget.used.load(Ordering::Relaxed), 0);
        assert!(matches!(wire.lock().unwrap().terminal, Some(Err(NetError::TooLarge))));
        for _ in 0..100 {
            receive(&wire, 0, RequestId(1), &poster, limits, ResponseEvent::Data(Bytes::from_static(b"x")));
        }
        assert_eq!(posts.load(Ordering::Relaxed), 1, "closed delivery must ignore late producer calls");
        assert_eq!(budget.used.load(Ordering::Relaxed), 0);
    }
}
