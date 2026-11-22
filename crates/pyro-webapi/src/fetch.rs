//! Callback-driven fetch bindings. Headers are delivered before body completion;
//! body readers buffer within explicit page/request limits. No stream API yet.
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

use crate::{
    abort::{AbortRegistration, AbortSignal, AbortSignalData},
    headers,
    page::{PageDom, RemotePoster},
    target::page_of,
    util::{dom_string, WrapperRef},
};
use bytes::Bytes;
use data_url::mime::Mime;
use pyro_js::{Deferred, JsError, JsResult, Persistent, RealmId, Scope, Value, WeakRealm};
use pyro_net::http::{header, HeaderName, HeaderValue, Method};
use pyro_net::{
    Credentials, Destination, NetError, NetworkDelivery, RedirectMode, Request, RequestId, RequestMode, ResponseEvent,
    ResponseHead,
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
            // Chrome throws for no number of pending requests; 64 failed pages that
            // start one per encoding label (encoding/single-byte-decoder.any.html).
            max_requests: 1024,
            // Chrome caps no response body; one is as large as pyro-net
            // takes (`NetConfig::max_body`, 256 MiB): fetch/content-encoding's
            // big-*-body pages decode to 27 MB.
            max_body_bytes: 256 * 1024 * 1024,
            // Room for one largest body and its clone (counted twice).
            max_total_body_bytes: 512 * 1024 * 1024,
            // A largest body in 4 KiB network chunks (a `Bytes` handle each,
            // 2 MiB of handles).
            max_body_chunks: 65_536,
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
    /// Body bytes of this document's keepalive fetches and beacons still in
    /// flight (released by their sinks), against [`KEEPALIVE_QUOTA`].
    pub(crate) keepalive_bytes: Arc<AtomicUsize>,
    context: RefCell<RequestContext>,
    jobs: RefCell<FxHashMap<RequestId, Rc<Job>>>,
    bodies: RefCell<FxHashMap<RequestId, Weak<Body>>>,
    /// The document's `fetchLater` requests not yet sent.
    pub(crate) deferred: crate::fetch_later::DeferredFetches,
}
impl Default for FetchState {
    fn default() -> Self {
        Self {
            limits: Cell::new(FetchLimits::default()),
            budget: Arc::new(Budget::default()),
            keepalive_bytes: Arc::default(),
            context: RefCell::new(RequestContext::default()),
            jobs: RefCell::new(FxHashMap::default()),
            bodies: RefCell::new(FxHashMap::default()),
            deferred: Default::default(),
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
    frontier: NetworkDelivery,
    /// Boxed: most bodies never get a head (every `Request`'s, a script's
    /// `Response`), and inline it would be most of the wire's size.
    head: Option<Box<ResponseHead>>,
    chunks: Vec<Bytes>,
    bytes: usize,
    terminal: Option<Result<(), NetError>>,
    closed: bool,
    scheduled: bool,
    null_body: bool,
    /// A `response.body` reader exists: data chunks wake the page so reads
    /// resolve as bytes arrive.
    streaming: bool,
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
    /// `integrity` metadata: the response is held until the whole body has
    /// arrived and matched (SRI), then delivered.
    integrity: Option<crate::integrity::Integrity>,
    held_head: RefCell<Option<ResponseHead>>,
}

/// Fetch's keepalive quota: the body bytes of a document's keepalive
/// requests in flight at once (Chrome's 64 KiB).
pub(crate) const KEEPALIVE_QUOTA: usize = 64 * 1024;

/// Reserves `len` keepalive body bytes; the returned guard releases them when
/// the request ends (its sink sees the terminal event or is dropped).
pub(crate) fn reserve_keepalive(bytes: &Arc<AtomicUsize>, len: usize) -> Option<KeepaliveGuard> {
    bytes
        .try_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_add(len).filter(|n| *n <= KEEPALIVE_QUOTA))
        .ok()?;
    Some(KeepaliveGuard { bytes: bytes.clone(), len })
}

pub(crate) struct KeepaliveGuard {
    bytes: Arc<AtomicUsize>,
    len: usize,
}
impl Drop for KeepaliveGuard {
    fn drop(&mut self) {
        self.bytes.fetch_sub(self.len, Ordering::AcqRel);
    }
}

/// A document's referrer state (`PageDom::referrer`).
#[derive(Default)]
pub(crate) struct DocumentReferrer {
    referrer: RefCell<String>,
}
struct Reader {
    realm: WeakRealm,
    promise: Deferred,
    kind: ReadKind,
    headers: Option<headers::HeadersData>,
}
#[derive(Clone, Copy)]
pub(crate) enum ReadKind {
    Text,
    Json,
    ArrayBuffer,
    Bytes,
    Blob,
    FormData,
    /// The body's pieces as they are, in a `cache_storage::StoredBody`: what
    /// `cache.put` keeps, without a copy.
    Chunks,
}
/// Package fully read bytes using the consuming Request or Response's own
/// current headers. Clones can share body bytes but have separate header lists.
pub(crate) fn package_body<'s>(
    s: &Scope<'s>,
    kind: ReadKind,
    bytes: Vec<u8>,
    headers: Option<&headers::HeadersData>,
) -> JsResult<Value<'s>> {
    let mime = headers.and_then(headers::HeadersData::mime_type);
    match kind {
        ReadKind::Blob => {
            let mime = mime.map(|m| m.to_string()).unwrap_or_default();
            Ok(s.wrap::<crate::file::Blob>(
                crate::file::BlobData { bytes: Bytes::from(bytes), mime, file: None },
                None,
            )?
            .value())
        }
        ReadKind::FormData => match mime {
            Some(m) if m.matches("application", "x-www-form-urlencoded") => {
                crate::form_data::from_urlencoded(s, &bytes)
            }
            Some(m) if m.matches("multipart", "form-data") => {
                let boundary = m
                    .get_parameter("boundary")
                    .ok_or_else(|| JsError::Type("Multipart body has no boundary".into()))?;
                crate::form_data::from_multipart(s, &bytes, boundary)
            }
            _ => Err(JsError::Type("Cannot parse this MIME type as FormData".into())),
        },
        _ => unreachable!("only Blob and FormData consume MIME metadata"),
    }
}
/// A native body read's `{value, done}`, with no prototype: resolving the
/// read's promise with it cannot find a `then` that script put on
/// `Object.prototype` (the body stream's chunks stay the body's bytes).
fn read_result<'s>(s: &Scope<'s>, value: Value<'s>, done: bool) -> JsResult<Value<'s>> {
    let result = s.object();
    result.set_prototype(s.null());
    result.set_atom("value", value)?;
    result.set_atom("done", s.boolean(done))?;
    Ok(result.value())
}
pub(crate) struct Body {
    wire: Arc<Mutex<Wire>>,
    visible_terminal: RefCell<Option<Result<(), NetError>>>,
    page: Weak<PageDom>,
    id: RequestId,
    realm_id: RealmId,
    readers: RefCell<Vec<Reader>>,
    /// Pending `ReadableStreamDefaultReader.read()` promises, in order.
    stream_reads: RefCell<Vec<(WeakRealm, Deferred)>>,
    /// The stream holds a consumer until it closes, errors or is canceled.
    stream_open: Cell<bool>,
    /// Bytes the stream delivered while another consumer (a clone that will
    /// read the whole body) kept the chunks: they stay buffered for it.
    stream_cursor: Cell<usize>,
    consumers: Cell<usize>,
    abort_registration: RefCell<Option<AbortRegistration>>,
    /// The aborting signal's own wrapper reference, for its reason. None without a signal.
    abort_signal: RefCell<Option<WrapperRef>>,
    aborted: Cell<bool>,
    disposed: Cell<bool>,
    /// A keepalive fetch: document teardown leaves its transport running.
    keepalive: bool,
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
    pub(crate) fn request_context(&self) -> RequestContext {
        self.fetch_state.context.borrow().clone()
    }
    /// Sets `document.referrer` (the navigation request's referrer source,
    /// before policy) and the document response's `Referrer-Policy` header.
    /// `policy` is the one the navigation request was sent with.
    pub fn set_document_referrer(
        &self,
        referrer: Option<&str>,
        policy: pyro_net::ReferrerPolicy,
        header: Option<pyro_net::ReferrerPolicy>,
    ) {
        let url = self.document_url();
        let value = referrer
            .and_then(|r| pyro_net::Url::parse(r).ok())
            .zip(pyro_net::Url::parse(&url).ok())
            .and_then(|(r, target)| pyro_net::referrer::referrer_for(&r, &target, policy));
        *self.referrer.referrer.borrow_mut() = value.unwrap_or_default();
        self.set_referrer_policy_header(header);
    }
    /// An initial about:blank document takes its creator's URL verbatim and
    /// snapshots the creator's referrer policy before any script can access it.
    pub fn inherit_initial_referrer(&self, creator: &PageDom) {
        *self.referrer.referrer.borrow_mut() = creator.document_url();
        self.set_referrer_policy_header(Some(creator.referrer_policy()));
    }

    /// Sets `document.referrer` to the `Referer` the navigation request's
    /// last hop sent (empty: none) and the document response's
    /// `Referrer-Policy` header.
    pub fn set_sent_referrer(&self, referrer: String, header: Option<pyro_net::ReferrerPolicy>) {
        *self.referrer.referrer.borrow_mut() = referrer;
        self.set_referrer_policy_header(header);
    }
    /// The `Referrer-Policy` header of a worker's main script response.
    pub(crate) fn set_referrer_policy_header(&self, header: Option<pyro_net::ReferrerPolicy>) {
        self.dom.borrow_mut().set_header_referrer_policy(self.document(), header.map(|policy| policy.as_str()));
    }
    /// `document.referrer`.
    pub fn document_referrer(&self) -> String {
        self.referrer.referrer.borrow().clone()
    }
    /// The document's referrer policy: its last `<meta name=referrer>` in
    /// `<head>` with a known value, else its `Referrer-Policy` header, else
    /// strict-origin-when-cross-origin. Read when a request is made (the head
    /// is found in constant time and scanned over its children only).
    pub fn referrer_policy(&self) -> pyro_net::ReferrerPolicy {
        pyro_net::ReferrerPolicy::parse(self.dom.borrow().effective_referrer_policy(self.document())).unwrap_or_default()
    }
    /// The document's URL, parsed (see `Dom::document_url_parsed`).
    pub(crate) fn document_url_parsed(&self) -> Option<pyro_net::Url> {
        self.dom.borrow().document_url_parsed(self.document()).ok().flatten().cloned()
    }
    /// Determine a client referrer from the document, before the network layer
    /// applies its referrer policy. An iframe srcdoc document uses the URL
    /// of the first enclosing document that is not itself srcdoc.
    pub fn client_referrer(&self) -> Option<pyro_net::Url> {
        if self.has_opaque_origin() {
            return None;
        }
        let url = self.document_url_parsed()?;
        if url.scheme() == "about" && url.path() == "srcdoc" {
            return match self.container() {
                crate::page::FrameContainer::Frame(parent, _) => parent.client_referrer(),
                _ => None,
            };
        }
        Some(url)
    }
    pub(crate) fn document_url(&self) -> String {
        self.dom.borrow().document_data(self.document()).map(|d| d.url.to_string()).unwrap_or_default()
    }
    /// Active network jobs and logical retained body bytes (diagnostics).
    pub fn fetch_usage(&self) -> (usize, usize) {
        (self.fetch_state.jobs.borrow().len(), self.fetch_state.budget.used.load(Ordering::Acquire))
    }
}

pub(crate) fn option<'s>(s: &Scope<'s>, init: Value<'s>, key: &str) -> JsResult<Value<'s>> {
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
pub(crate) fn forbidden_header(name: &str) -> bool {
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
/// Fetch "forbidden request-header": a forbidden name, or a method override
/// header whose value names a forbidden method (CONNECT, TRACE, TRACK).
pub(crate) fn forbidden_request_header(name: &str, value: &str) -> bool {
    if forbidden_header(name) {
        return true;
    }
    if !matches!(name, "x-http-method-override" | "x-http-method" | "x-method-override") {
        return false;
    }
    split_header_values(value)
        .any(|method| ["CONNECT", "TRACE", "TRACK"].iter().any(|m| method.eq_ignore_ascii_case(m)))
}

/// Fetch "get, decode, and split": preserve commas and escaped characters
/// in HTTP quoted strings, trimming HTTP tab/space around each value.
fn split_header_values(value: &str) -> impl Iterator<Item = &str> {
    let mut quoted = false;
    let mut escaped = false;
    value
        .split(move |ch| {
            if escaped {
                escaped = false;
            } else if quoted && ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                quoted = !quoted;
            } else if ch == ',' && !quoted {
                return true;
            }
            false
        })
        .map(|part| part.trim_matches([' ', '\t']))
}

/// MIME Sniffing "parse a MIME type". Use data-url for essence parsing and
/// serialization. Collect the complete parameter value before validating it:
/// a quoted string may span semicolons and have ignored text after its quote.
fn parse_body_mime(value: &str) -> Option<Mime> {
    let (essence, mut rest) = value.trim_matches([' ', '\t', '\r', '\n']).split_once(';').unwrap_or((value, ""));
    let mut mime: Mime = essence.parse().ok()?;
    while !rest.is_empty() {
        rest = rest.trim_start_matches([' ', '\t', '\r', '\n']);
        let Some(end) = rest.find(['=', ';']) else { break };
        let name = &rest[..end];
        let separator = rest.as_bytes()[end];
        rest = &rest[end + 1..];
        if separator == b';' {
            continue;
        }
        let value = if let Some(quoted) = rest.strip_prefix('"') {
            let mut output = String::new();
            let mut chars = quoted.chars();
            while let Some(ch) = chars.next() {
                match ch {
                    '"' => break,
                    '\\' => output.push(chars.next().unwrap_or('\\')),
                    _ => output.push(ch),
                }
            }
            rest = chars.as_str().split_once(';').map_or("", |(_, tail)| tail);
            output
        } else {
            let (value, tail) = rest.split_once(';').unwrap_or((rest, ""));
            rest = tail;
            let value = value.trim_end_matches([' ', '\t', '\r', '\n']);
            if value.is_empty() {
                continue;
            }
            value.to_owned()
        };
        if !value.chars().all(|ch| matches!(ch, '\t' | ' '..='~' | '\u{80}'..='\u{ff}')) {
            continue;
        }
        let Ok(name) = headers::validate_name(name.to_owned()) else { continue };
        if mime.get_parameter(name.as_str()).is_none() {
            mime.parameters.push((name, value));
        }
    }
    Some(mime)
}

pub(crate) fn extract_mime(value: &str) -> Option<Mime> {
    let mut result: Option<Mime> = None;
    let mut charset: Option<String> = None;
    for value in split_header_values(value) {
        let Some(mut mime) = parse_body_mime(value) else { continue };
        if mime.matches("*", "*") {
            continue;
        }
        if result.as_ref().is_some_and(|prev| prev.matches(&mime.type_, &mime.subtype)) {
            if mime.get_parameter("charset").is_none() {
                if let Some(inherited) = &charset {
                    mime.parameters.push(("charset".into(), inherited.clone()));
                }
            }
        } else {
            charset = mime.get_parameter("charset").map(str::to_owned);
        }
        result = Some(mime);
    }
    result
}
/// Gives a `blob:` request the object its URL names, from this document's
/// blob URL store (the network client cannot see it); a URL the store does
/// not hold, which includes every other document's, stays unresolved and the
/// client fails it like a revoked one.
pub(crate) fn attach_blob(page: &PageDom, request: &mut Request) {
    if request.url.scheme() == "blob" {
        request.blob = page.blob_url(request.url.as_str()).map(|(bytes, mime)| pyro_net::BlobBody { bytes, mime });
    }
}
/// What a fetch does beyond its `pyro_net::Request`.
pub(crate) struct FetchExtras {
    integrity: Option<crate::integrity::Integrity>,
    /// Keepalive: the reserved body bytes, held until the request ends.
    keepalive: Option<KeepaliveGuard>,
}

/// `deferred`: for `fetchLater` (Fetch "queue a deferred fetch"), whose request may not have a
/// ReadableStream body (from `init` or a Request input) and takes no keepalive quota of its own.
pub(crate) fn parse_request<'s>(
    s: &Scope<'s>,
    page: &PageDom,
    input: Value<'s>,
    init: Value<'s>,
    deferred: bool,
) -> JsResult<(Request, Option<Persistent>, FetchExtras)> {
    // A Request input: its fields, overridden by `init`.
    let request_input = s.unwrap::<crate::request::Request>(input);
    let input = match &request_input {
        Some(r) => r.url.clone(),
        None => dom_string(input)?,
    };
    let default_string = |key: &str, default: &str| -> String {
        match &request_input {
            Some(r) => match key {
                "method" => r.method.clone(),
                "mode" => r.mode.clone(),
                "credentials" => r.credentials.clone(),
                "redirect" => r.redirect.clone(),
                "cache" => r.cache.clone(),
                _ => default.to_owned(),
            },
            None => default.to_owned(),
        }
    };
    let resolved = crate::html::resolve_url_utf8(&page.dom.borrow(), page.document(), &input);
    let mut url = pyro_net::Url::parse(&resolved).map_err(|_| JsError::Type("Invalid fetch URL".into()))?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err(JsError::Type("Fetch URL contains credentials".into()));
    }
    url.set_fragment(None);
    let mut request = Request::get(url, Destination::Fetch);
    let method = string_option(s, init, "method", &default_string("method", "GET"))?;
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
    request.mode = match string_option(s, init, "mode", &default_string("mode", "cors"))?.as_str() {
        "cors" => RequestMode::Cors,
        "same-origin" => RequestMode::SameOrigin,
        "no-cors" => RequestMode::NoCors,
        // A navigation request a service worker got (`fetch(event.request)`):
        // it stays one with no init, and becomes same-origin with one.
        "navigate" if request_input.as_ref().is_some_and(|r| r.mode == "navigate")
            && string_option(s, init, "mode", "")?.is_empty() =>
        {
            if init.is_nullish() {
                request.destination = match request_input.as_ref().map(|r| r.destination.as_str()) {
                    Some("iframe") => Destination::Iframe,
                    _ => Destination::Document,
                };
                RequestMode::Navigate
            } else {
                RequestMode::SameOrigin
            }
        }
        _ => return Err(JsError::Type("Invalid fetch mode".into())),
    };
    request.credentials =
        match string_option(s, init, "credentials", &default_string("credentials", "same-origin"))?.as_str() {
            "omit" => Credentials::Omit,
            "same-origin" => Credentials::SameOrigin,
            "include" => Credentials::Include,
            _ => return Err(JsError::Type("Invalid fetch credentials".into())),
        };
    request.redirect = match string_option(s, init, "redirect", &default_string("redirect", "follow"))?.as_str() {
        "follow" => RedirectMode::Follow,
        "error" => RedirectMode::Error,
        "manual" => RedirectMode::Manual,
        _ => return Err(JsError::Type("Invalid fetch redirect mode".into())),
    };
    // RequestInit `priority` is a RequestPriority enum; fetch has no use for it.
    if !matches!(string_option(s, init, "priority", "auto")?.as_str(), "high" | "low" | "auto") {
        return Err(JsError::Type("Invalid fetch priority".into()));
    }
    if request.mode == RequestMode::NoCors && !matches!(request.method, Method::GET | Method::HEAD | Method::POST) {
        return Err(JsError::Type("Invalid no-cors method".into()));
    }
    let headers_init = option(s, init, "headers")?;
    let header_pairs = match (&request_input, headers_init.is_undefined()) {
        (Some(r), true) => r.header_pairs(s),
        _ => headers::from_init(s, headers_init)?.pairs(),
    };
    for (name, value) in header_pairs {
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
    if deferred {
        let init_stream = !body.is_undefined() && !body.is_null() && crate::streams::is_stream(s, body)?;
        let input_stream = body.is_undefined() && request_input.as_ref().is_some_and(|r| r.script_stream.get());
        if init_stream || input_stream {
            return Err(JsError::Type("fetchLater: a ReadableStream request body cannot be deferred.".into()));
        }
    }
    let input_body = match (&request_input, body.is_undefined()) {
        (Some(r), true) => r.take_body()?.map(|bytes| (bytes, r.body.mime.borrow().clone())),
        _ => None,
    };
    if let Some((bytes, mime)) = input_body {
        if matches!(request.method, Method::GET | Method::HEAD) {
            return Err(JsError::Type("GET/HEAD cannot have a body".into()));
        }
        if !mime.is_empty() && !request.headers.contains_key(header::CONTENT_TYPE) {
            request.headers.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_str(&mime).map_err(|_| JsError::Type("Invalid body MIME type".into()))?,
            );
        }
        request.body = Some(bytes);
    } else if !body.is_null() && !body.is_undefined() {
        if matches!(request.method, Method::GET | Method::HEAD) {
            return Err(JsError::Type("GET/HEAD cannot have a body".into()));
        }
        let (bytes, mime) = extract_body(s, body)?;
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
        let unsafe_names = pyro_net::cors::unsafe_header_names(&request.headers);
        for name in unsafe_names {
            request.headers.remove(name);
        }
    }
    // Referrer: init, else the Request input's, else "client" (the document).
    let referrer = match option(s, init, "referrer")? {
        v if v.is_undefined() => request_input.as_ref().map_or_else(|| "client".to_owned(), |r| r.referrer.clone()),
        v => crate::request::parse_referrer(page, &dom_string(v)?).map_err(JsError::type_error)?,
    };
    request.referrer = match referrer.as_str() {
        "no-referrer" => None,
        "client" => page.client_referrer(),
        url => pyro_net::Url::parse(url).ok(),
    };
    let policy = match option(s, init, "referrerPolicy")? {
        v if v.is_undefined() => request_input.as_ref().map(|r| r.referrer_policy.clone()).unwrap_or_default(),
        v => {
            let policy = dom_string(v)?;
            crate::request::check_referrer_policy(&policy).map_err(JsError::type_error)?;
            policy
        }
    };
    // The document's policy is looked up only when a referrer is sent.
    request.referrer_policy = match pyro_net::ReferrerPolicy::parse(&policy) {
        Some(policy) => policy,
        None if request.referrer.is_some() => page.referrer_policy(),
        None => Default::default(),
    };
    let integrity = match option(s, init, "integrity")? {
        v if v.is_undefined() => request_input.as_ref().map(|r| r.integrity.clone()).unwrap_or_default(),
        v => dom_string(v)?,
    };
    if !integrity.is_empty() && request.mode == RequestMode::NoCors {
        // An opaque response can never match its metadata.
        if crate::integrity::Integrity::parse(&integrity).is_some() {
            return Err(JsError::Type("Failed to fetch: integrity metadata on a no-cors request".into()));
        }
    }
    let keepalive = match option(s, init, "keepalive")? {
        v if v.is_undefined() => request_input.as_ref().is_some_and(|r| r.keepalive),
        v => v.to_bool(),
    };
    let keepalive = if keepalive && !deferred {
        let len = request.body.as_ref().map_or(0, Bytes::len);
        Some(
            reserve_keepalive(&page.fetch_state.keepalive_bytes, len)
                .ok_or_else(|| JsError::Type("Failed to fetch: keepalive request body exceeds its quota".into()))?,
        )
    } else {
        None
    };
    let extras = FetchExtras { integrity: crate::integrity::Integrity::parse(&integrity), keepalive };
    request.cache = match string_option(s, init, "cache", &default_string("cache", "default"))?.as_str() {
        "default" => pyro_net::CacheMode::Default,
        "no-store" => pyro_net::CacheMode::NoStore,
        "reload" => pyro_net::CacheMode::Reload,
        "no-cache" => pyro_net::CacheMode::NoCache,
        "force-cache" => pyro_net::CacheMode::ForceCache,
        "only-if-cached" => pyro_net::CacheMode::OnlyIfCached,
        _ => return Err(JsError::Type("Invalid fetch cache mode".into())),
    };
    if request.cache == pyro_net::CacheMode::OnlyIfCached && request.mode != RequestMode::SameOrigin {
        return Err(JsError::Type("'only-if-cached' can be set only with 'same-origin' mode".into()));
    }
    let signal = option(s, init, "signal")?;
    let signal = match (&request_input, signal.is_undefined()) {
        (Some(r), true) => r.signal(s).unwrap_or_else(|| s.undefined()),
        _ => signal,
    };
    let signal = if signal.is_null() || signal.is_undefined() {
        None
    } else {
        if s.unwrap::<AbortSignal>(signal).is_none() {
            return Err(JsError::Type("signal must be an AbortSignal".into()));
        }
        Some(Persistent::new(s, signal))
    };
    request.origin = Some(s.realm().origin().unwrap_or_else(|| {
        page.document_url_parsed().map(|u| u.origin().ascii_serialization()).unwrap_or_else(|| "null".into())
    }));
    let context = page.fetch_state.context.borrow();
    request.frame_id = context.frame_id.clone();
    request.loader_id = context.loader_id.clone();
    Ok((request, signal, extras))
}

/// Body mixin `textStream()` once the body is known usable: `body` (the
/// object's body stream, or null) piped through a UTF-8 TextDecoderStream.
/// A null body gives a fresh empty stream on each call and stays unused.
pub(crate) fn text_stream<'s>(s: &Scope<'s>, body: Value<'s>) -> JsResult<Value<'s>> {
    let page = crate::target::page_of(s)?;
    if body.is_null() {
        let empty = crate::encoding::uint8_array(s, &[])?;
        return crate::streams::helper(s, &page, "textFromBytes", &[empty]);
    }
    crate::streams::helper(s, &page, "textFromStream", &[body])
}

/// Fetch "extract a body" for the supported BodyInit types: FormData
/// (multipart/form-data with a generated boundary), Blob, URLSearchParams,
/// BufferSource, else ToString. Returns (bytes, Content-Type or "" when the
/// type sets none). ReadableStream bodies are handled by the callers, which
/// drain them first (`Response` constructor, `fetch`); other objects are
/// stringified as the Web IDL union's last member, USVString.
pub(crate) fn extract_body(s: &Scope<'_>, body: Value<'_>) -> JsResult<(Bytes, String)> {
    if let Some((bytes, mime)) = crate::form_data::multipart_body(s, body)? {
        return Ok((bytes, mime));
    }
    Ok(if let Some(blob) = s.unwrap::<crate::file::Blob>(body) {
        (blob.bytes.clone(), blob.mime.clone())
    } else if let Some(params) = s.unwrap::<crate::url_search_params::URLSearchParams>(body) {
        (Bytes::from(params.serialize()), "application/x-www-form-urlencoded;charset=UTF-8".into())
    } else if let Some(bytes) = crate::encoding::buffer_source_bytes(s, body)? {
        (Bytes::from(bytes), String::new())
    } else {
        (Bytes::from(dom_string(body)?), "text/plain;charset=UTF-8".into())
    })
}

/// `navigator.sendBeacon(url, data)` (Beacon spec): a keepalive POST with
/// credentials, `no-cors` unless the body's type is not CORS-safelisted.
/// It is not one of the document's fetches, so it outlives the document.
/// Returns false when the body exceeds the keepalive quota or the page has
/// no network.
pub(crate) fn send_beacon(s: &Scope<'_>, page: &Rc<PageDom>, url: pyro_net::Url, data: Value<'_>) -> JsResult<bool> {
    let (body, mime) = if data.is_null() || data.is_undefined() {
        (Bytes::new(), String::new())
    } else if crate::streams::is_stream(s, data)? {
        return Err(JsError::Type("Failed to execute 'sendBeacon': a ReadableStream body is not supported.".into()));
    } else {
        extract_body(s, data)?
    };
    let Some(net) = page.net.borrow().clone() else { return Ok(false) };
    let Some(guard) = reserve_keepalive(&page.fetch_state.keepalive_bytes, body.len()) else { return Ok(false) };
    let mut request = Request::get(url, Destination::Beacon);
    request.method = Method::POST;
    request.credentials = Credentials::Include;
    request.mode = RequestMode::NoCors;
    if !mime.is_empty() {
        let value = HeaderValue::from_str(&mime).map_err(|_| JsError::Type("Invalid body MIME type".into()))?;
        if !pyro_net::cors::is_safelisted_header("content-type", &value) {
            request.mode = RequestMode::Cors;
        }
        request.headers.insert(header::CONTENT_TYPE, value);
    }
    request.body = Some(body);
    let document_url = page.document_url_parsed();
    request.origin = Some(s.realm().origin().unwrap_or_else(|| {
        document_url.as_ref().map(|u| u.origin().ascii_serialization()).unwrap_or_else(|| "null".into())
    }));
    request.referrer = page.client_referrer();
    request.referrer_policy = page.referrer_policy();
    let context = page.request_context();
    request.frame_id = context.frame_id;
    request.loader_id = context.loader_id;
    page.apply_request_defaults(&mut request);
    let mut guard = Some(guard);
    let sink: pyro_net::ResponseSink = Box::new(move |event| {
        if matches!(event, ResponseEvent::Finished { .. } | ResponseEvent::Failed(_)) {
            guard.take();
        }
    });
    let sink =
        crate::resource_timing::timed_sink(page, crate::resource_timing::Initiator::Beacon, &request.url, false, sink);
    net.fetch(request, sink);
    Ok(true)
}

/// HTML hyperlink auditing: a `PING` POST to each of a followed `<a>` or
/// `<area>`'s `ping` URLs. It holds nothing up and, like a beacon, is not one
/// of the document's fetches. `document` is the link's document URL,
/// `target` the URL followed, `policy` the link's referrer policy.
pub(crate) fn send_pings(
    page: &Rc<PageDom>,
    pings: Vec<pyro_net::Url>,
    document: Option<pyro_net::Url>,
    target: &str,
    policy: pyro_net::ReferrerPolicy,
) {
    use pyro_net::ReferrerPolicy as P;
    let Some(net) = page.net.borrow().clone() else { return };
    let context = page.request_context();
    let document_origin = document.as_ref().map(|u| u.origin()).filter(|o| o.is_tuple());
    let encrypted = document.as_ref().is_some_and(|u| u.scheme() == "https");
    for url in pings {
        let Some(guard) = reserve_keepalive(&page.fetch_state.keepalive_bytes, 4) else { return };
        let same_origin = document_origin.as_ref().is_some_and(|o| *o == url.origin());
        let downgrade = encrypted && url.scheme() == "http";
        let mut request = Request::get(url, Destination::Beacon);
        request.method = Method::POST;
        request.credentials = Credentials::Include;
        request.mode = RequestMode::NoCors;
        request.headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/ping"));
        if let Ok(v) = HeaderValue::from_str(target) {
            request.headers.insert(HeaderName::from_static("ping-to"), v);
        }
        // Ping-From for a same-origin ping, or any from a document not
        // fetched over an encrypted connection.
        if same_origin || !encrypted {
            if let Some(v) = document.as_ref().and_then(|u| HeaderValue::from_str(u.as_str()).ok()) {
                request.headers.insert(HeaderName::from_static("ping-from"), v);
            }
        }
        request.body = Some(Bytes::from_static(b"PING"));
        // Fetch "serializing a request origin" under the link's policy.
        let null_origin = match policy {
            P::NoReferrer => true,
            P::SameOrigin => !same_origin,
            P::NoReferrerWhenDowngrade | P::StrictOrigin | P::StrictOriginWhenCrossOrigin => downgrade,
            P::Origin | P::OriginWhenCrossOrigin | P::UnsafeUrl => false,
        };
        request.origin = Some(match (&document_origin, null_origin) {
            (Some(origin), false) => origin.ascii_serialization(),
            _ => "null".into(),
        });
        // The request's referrer is no-referrer: Ping-From names the document.
        request.referrer = None;
        request.referrer_policy = P::NoReferrer;
        request.frame_id = context.frame_id.clone();
        request.loader_id = context.loader_id.clone();
        page.apply_request_defaults(&mut request);
        let mut guard = Some(guard);
        net.fetch(
            request,
            Box::new(move |event| {
                if matches!(event, ResponseEvent::Finished { .. } | ResponseEvent::Failed(_)) {
                    guard.take();
                }
            }),
        );
    }
}

pub(crate) fn install(s: &Scope<'_>) -> JsResult<()> {
    let function = s.function("fetch", |s, _this, args| fetch_call(s, &args, None))?;
    s.global().set_atom("fetch", function.value())
}

/// The fetch streams.js `fetchBuffered` makes once it has settled a stream body (its
/// `natives.fetchSettled`, not reachable from page script): `fromStream` says whether the
/// body is the stream's bytes. It never takes the stream path again.
pub(crate) fn settled_fetch_function<'s>(s: &Scope<'s>) -> JsResult<Value<'s>> {
    let function = s.function("fetch", |s, _this, args| {
        let from_stream = args.get(2).to_bool();
        fetch_call(s, &args, Some(from_stream))
    })?;
    Ok(function.value())
}

/// `fetch(input, init)`; `settled`: a call from `fetchBuffered` (see [`settled_fetch_function`]).
fn fetch_call<'s>(s: &Scope<'s>, args: &pyro_js::Args<'s>, settled: Option<bool>) -> JsResult<Value<'s>> {
    let deferred = s.deferred()?;
    let promise = deferred.promise.get(s);
    let streamed = if settled.is_some() {
        Ok(None)
    } else {
        crate::streams::fetch_stream_body(s, args.get(0), args.get(1))
    };
    match streamed {
        Ok(Some(sent)) => {
            deferred.resolve(s, sent)?;
            return Ok(promise);
        }
        Ok(None) => {}
        Err(error) => {
            deferred.reject(s, error.to_value(s))?;
            return Ok(promise);
        }
    }
    let result = (|| {
        args.require(1, "fetch")?;
        let page = page_of(s)?;
        if page.is_disposed() {
            return Err(JsError::ContextDestroyed);
        }
        let (mut request, signal, extras) = parse_request(s, &page, args.get(0), args.get(1), false)?;
        request.body_from_stream = request.body.is_some() && settled == Some(true);
        // Init conversion can execute script, including embedder teardown.
        if page.is_disposed() {
            return Err(JsError::ContextDestroyed);
        }
        if let Some(signal) = &signal {
            let signal = s.unwrap::<AbortSignal>(signal.get(s)).expect("validated signal");
            if signal.aborted() {
                return Err(JsError::thrown(Persistent::new(s, signal.reason(s))));
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
        Ok((page, request, signal, extras, limits, net, poster))
    })();
    match result {
        Err(error) => deferred.reject(s, error.to_value(s))?,
        Ok((page, mut request, signal, extras, limits, net, poster)) => {
            page.apply_request_defaults(&mut request);
            attach_blob(&page, &mut request);
            request.max_body = Some(limits.max_body_bytes);
            let id = net.next_request_id();
            let wire = Arc::new(Mutex::new(Wire {
                frontier: NetworkDelivery::default(),
                head: None,
                chunks: Vec::new(),
                bytes: 0,
                terminal: None,
                closed: false,
                scheduled: false,
                null_body: request.method == Method::HEAD,
                streaming: false,
                budget: page.fetch_state.budget.clone(),
            }));
            let body = Rc::new(Body {
                wire: wire.clone(),
                visible_terminal: RefCell::new(None),
                page: Rc::downgrade(&page),
                id,
                realm_id: s.realm_id(),
                readers: RefCell::new(Vec::new()),
                stream_reads: RefCell::new(Vec::new()),
                stream_open: Cell::new(false),
                stream_cursor: Cell::new(0),
                consumers: Cell::new(0),
                abort_registration: RefCell::new(None),
                abort_signal: RefCell::new(None),
                aborted: Cell::new(false),
                disposed: Cell::new(false),
                keepalive: extras.keepalive.is_some(),
                mime: RefCell::new(String::new()),
            });
            page.fetch_state.bodies.borrow_mut().insert(id, Rc::downgrade(&body));
            if let Some(signal) = &signal {
                let signal = s.unwrap::<AbortSignal>(signal.get(s)).expect("validated signal");
                *body.abort_signal.borrow_mut() = Some(signal.wrapper.clone());
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
                integrity: extras.integrity,
                held_head: RefCell::new(None),
            });
            if job.integrity.is_some() {
                // Nothing is delivered before the whole body is checked.
                job.body.wire.lock().unwrap().streaming = false;
            }
            page.fetch_state.jobs.borrow_mut().insert(id, job);
            let page_id = page.id();
            PAGES.with(|pages| pages.borrow_mut().insert(page_id, Rc::downgrade(&page)));
            let mut keepalive = extras.keepalive;
            let sink: pyro_net::ResponseSink = Box::new(move |event| {
                if matches!(event, ResponseEvent::Finished { .. } | ResponseEvent::Failed(_)) {
                    keepalive.take();
                }
                receive(&wire, page_id, id, &poster, limits, event)
            });
            let sink = crate::resource_timing::timed_sink_carried(
                &page,
                crate::resource_timing::Initiator::Fetch,
                &request.url,
                sink,
            );
            // A controlled page's request goes to its service worker
            // first; any other is one flag test away from the network.
            crate::service_worker::route(&page, &net, id, request, sink);
        }
    }
    Ok(promise)
}

/// WebAssembly.compileStreaming and instantiateStreaming on the realm's `WebAssembly` namespace
/// (install.js `wasmStreaming`, given as `factory`), with [`crate::response::read_for_wasm`] as
/// the step that takes a Response's bytes.
pub(crate) fn install_wasm_streaming(s: &Scope<'_>, factory: Value<'_>) -> JsResult<()> {
    let read = s.function("readWasmResponse", |s, _, a| crate::response::read_for_wasm(s, a.get(0), a.get(1)))?;
    let factory = factory.as_object().ok_or_else(|| JsError::Type("install.js wasmStreaming".into()))?;
    factory.call(None, &[read.value()])?;
    Ok(())
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
            // The body closed early (cancelled, too large, failed its
            // integrity check) and no `pump` follows: the end's Resource
            // Timing report (`timed_sink_carried`) needs a task of its own.
            let reported = match &event {
                ResponseEvent::Finished { .. } => true,
                ResponseEvent::Failed(error) => *error != NetError::Aborted,
                _ => false,
            };
            drop(wire);
            if reported {
                poster(Box::new(move || add_finished_fetches(page)));
            }
            return;
        }
        let notify = match event {
            ResponseEvent::Redirect { .. } => false,
            ResponseEvent::Response(head) => {
                wire.frontier.response = true;
                wire.null_body |=
                    matches!(head.kind, pyro_net::ResponseKind::Opaque | pyro_net::ResponseKind::OpaqueRedirect)
                        || matches!(head.status.as_u16(), 204 | 205 | 304);
                wire.head = Some(Box::new(head));
                true
            }
            ResponseEvent::Data(bytes) => {
                wire.frontier.data_bytes = wire.frontier.data_bytes.saturating_add(bytes.len() as u64);
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
                    wire.streaming
                }
            }
            ResponseEvent::Finished { .. } => {
                wire.frontier.terminal = true;
                wire.terminal = Some(Ok(()));
                wire.closed = true;
                true
            }
            ResponseEvent::Failed(error) => {
                wire.frontier.terminal = true;
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

/// Adds the finished fetches' Resource Timing entries of page `page_id`.
fn add_finished_fetches(page_id: u64) {
    if let Some(page) = PAGES.with(|pages| pages.borrow().get(&page_id).and_then(Weak::upgrade)) {
        page.add_finished_fetches();
    }
}

fn pump(page_id: u64, id: RequestId) {
    let page = PAGES.with(|pages| pages.borrow().get(&page_id).and_then(Weak::upgrade));
    let Some(page) = page else { return };
    let job = page.fetch_state.jobs.borrow().get(&id).cloned();
    // A fetch gone since its end was queued (aborted, its realm destroyed)
    // still leaves the end's Resource Timing report, which rides this task
    // (`timed_sink_carried`): left queued, it would hold back every later one.
    let Some(job) = job else {
        page.add_finished_fetches();
        return;
    };
    let Some(realm) = job.realm.upgrade() else {
        page.add_finished_fetches();
        cancel_realm(&page, job.realm_id);
        return;
    };
    let Ok(s) = realm.scope() else {
        page.add_finished_fetches();
        cancel_realm(&page, job.realm_id);
        return;
    };
    let (head, terminal, frontier) = {
        let mut wire = job.body.wire.lock().unwrap();
        wire.scheduled = false;
        (wire.head.take().map(|head| *head), wire.terminal.clone(), wire.frontier)
    };
    if terminal.is_some() {
        // The fetch's Resource Timing entry rides this task (`timed_sink_carried`).
        page.add_finished_fetches();
    }
    *job.body.visible_terminal.borrow_mut() = terminal.clone();
    page.network_delivery(id, frontier);
    if page.is_disposed() || job.body.disposed.get() || job.body.aborted.get() {
        return;
    }
    let head = match &job.integrity {
        None => head,
        Some(integrity) => {
            if let Some(head) = head {
                if matches!(head.kind, pyro_net::ResponseKind::Opaque | pyro_net::ResponseKind::OpaqueRedirect) {
                    fail_integrity(&s, &page, &job, id);
                    return;
                }
                *job.held_head.borrow_mut() = Some(head);
            }
            if matches!(terminal, Some(Ok(()))) {
                let held = job.held_head.borrow_mut().take();
                if held.is_some() && !integrity.matches(&job.body.wire.lock().unwrap().chunks) {
                    fail_integrity(&s, &page, &job, id);
                    return;
                }
                held
            } else {
                None
            }
        }
    };
    if let Some(head) = head {
        *job.body.mime.borrow_mut() =
            if matches!(head.kind, pyro_net::ResponseKind::Opaque | pyro_net::ResponseKind::OpaqueRedirect) {
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
    job.body.settle_stream_reads();
    if let Some(result) = terminal {
        if result.is_err() {
            if let Some(net) = page.net.borrow().as_ref() {
                net.cancel(id);
            }
        }
        let deferred = job.promise.borrow_mut().take();
        if let Some(deferred) = deferred {
            let error = result.err().unwrap_or_else(|| NetError::Other("Missing response headers".into()));
            let _ = deferred.reject(&s, JsError::Type(error.to_string().into()).to_value(&s));
        }
        page.fetch_state.jobs.borrow_mut().remove(&id);
        job.body.settle_readers();
        job.body.release_if_unused();
    }
}

/// The body did not match the request's integrity metadata (or the response
/// is opaque): a network error, as if it never arrived.
fn fail_integrity(s: &Scope<'_>, page: &PageDom, job: &Job, id: RequestId) {
    if let Some(net) = page.net.borrow().as_ref() {
        net.cancel(id);
    }
    {
        let mut wire = job.body.wire.lock().unwrap();
        wire.terminal = Some(Err(NetError::Other("integrity mismatch".into())));
        wire.closed = true;
        wire.clear_body();
    }
    *job.body.visible_terminal.borrow_mut() = Some(Err(NetError::Other("integrity mismatch".into())));
    job.held_head.borrow_mut().take();
    if let Some(deferred) = job.promise.borrow_mut().take() {
        let error = JsError::Type("Failed to fetch: the response does not match its integrity metadata".into());
        let _ = deferred.reject(s, error.to_value(s));
    }
    page.fetch_state.jobs.borrow_mut().remove(&id);
    job.body.settle_readers();
    job.body.release_if_unused();
}

impl Body {
    /// A complete in-memory body for script-constructed Requests and Responses.
    pub(crate) fn constructed(s: &Scope<'_>, bytes: Option<Bytes>, mime: String) -> Rc<Body> {
        Body::constructed_chunks(s, bytes.map(|b| vec![b]), mime)
    }

    /// [`Body::constructed`] of a body in pieces, which it keeps as they are
    /// (a response read back from a cache shares the stored bytes).
    pub(crate) fn constructed_chunks(s: &Scope<'_>, bytes: Option<Vec<Bytes>>, mime: String) -> Rc<Body> {
        let null_body = bytes.is_none();
        let mut chunks = bytes.unwrap_or_default();
        chunks.retain(|b| !b.is_empty());
        let len = chunks.iter().map(Bytes::len).sum();
        let wire = Wire {
            frontier: NetworkDelivery::default(),
            head: None,
            chunks,
            bytes: len,
            terminal: Some(Ok(())),
            closed: true,
            scheduled: false,
            null_body,
            streaming: false,
            // Private budget pre-charged with this body so clear_body balances.
            budget: Arc::new(Budget { used: AtomicUsize::new(len), limit: AtomicUsize::new(usize::MAX) }),
        };
        Rc::new(Body {
            wire: Arc::new(Mutex::new(wire)),
            visible_terminal: RefCell::new(Some(Ok(()))),
            page: Weak::new(),
            id: RequestId(0),
            realm_id: s.realm_id(),
            readers: RefCell::new(Vec::new()),
            stream_reads: RefCell::new(Vec::new()),
            stream_open: Cell::new(false),
            stream_cursor: Cell::new(0),
            consumers: Cell::new(0),
            abort_registration: RefCell::new(None),
            abort_signal: RefCell::new(None),
            aborted: Cell::new(false),
            disposed: Cell::new(false),
            keepalive: false,
            mime: RefCell::new(mime),
        })
    }

    /// The bytes of a script-constructed body (`Body::constructed`).
    pub(crate) fn constructed_bytes(&self) -> Bytes {
        let wire = self.wire.lock().expect("body wire");
        match wire.chunks.as_slice() {
            [one] => one.clone(),
            chunks => Bytes::from(chunks.concat()),
        }
    }
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
        // The signal's reason when its wrapper is still alive; otherwise (no signal, or the
        // signal was collected before the body was read) the default AbortError.
        let reason = self.abort_signal.borrow().as_ref().and_then(|w| AbortSignalData::stored_reason(w, s));
        reason.unwrap_or_else(|| JsError::dom("AbortError", "Fetch aborted").to_value(s))
    }
    fn error<'s>(&self, s: &Scope<'s>) -> Option<Value<'s>> {
        // A null body has no stream for abort or network failure to error.
        if self.is_null() {
            return None;
        }
        if self.aborted.get() {
            return Some(self.abort_error(s));
        }
        let error = self.visible_terminal.borrow().as_ref().and_then(|r| r.as_ref().err().cloned());
        if self.disposed.get() {
            Some(JsError::dom("AbortError", "Document disposed").to_value(s))
        } else {
            error.map(|e| JsError::Type(e.to_string().into()).to_value(s))
        }
    }
    pub(crate) fn read<'s>(
        self: &Rc<Self>,
        s: &Scope<'s>,
        kind: ReadKind,
        already_used: bool,
        headers: Option<headers::HeadersData>,
    ) -> JsResult<Value<'s>> {
        let deferred = s.deferred()?;
        let promise = deferred.promise.get(s);
        let ready = self.is_null() || self.visible_terminal.borrow().is_some();
        if already_used {
            deferred.reject(s, JsError::Type("Response body has already been consumed".into()).to_value(s))?;
        } else if let Some(error) = self.error(s) {
            deferred.reject(s, error)?;
            self.release_consumer();
        } else if ready && !self.is_null() && matches!(kind, ReadKind::Blob | ReadKind::FormData) {
            // Even an already-buffered stream finishes through a fetch task.
            // Read the container's MIME metadata when packaging its bytes.
            let body = self.clone();
            let realm = s.realm().downgrade();
            page_of(s)?.post_task(Box::new(move || {
                if let Some(realm) = realm.upgrade() {
                    if let Ok(scope) = realm.scope() {
                        body.settle(&scope, deferred, kind, headers.as_ref());
                        return;
                    }
                }
                body.release_consumer();
            }));
        } else if ready {
            self.settle(s, deferred, kind, headers.as_ref());
        } else if self.readers.borrow().len() >= 1024 {
            deferred.reject(s, JsError::Type("Too many pending body readers".into()).to_value(s))?;
            self.release_consumer();
        } else {
            self.readers.borrow_mut().push(Reader { realm: s.realm().downgrade(), promise: deferred, kind, headers });
        }
        Ok(promise)
    }
    fn settle(&self, s: &Scope<'_>, deferred: Deferred, kind: ReadKind, headers: Option<&headers::HeadersData>) {
        if let Some(error) = self.error(s) {
            let _ = deferred.reject(s, error);
            self.release_consumer();
            return;
        }
        let bytes = if matches!(kind, ReadKind::Chunks) {
            Vec::new()
        } else {
            let wire = self.wire.lock().unwrap();
            let mut bytes = Vec::with_capacity(wire.bytes);
            for chunk in &wire.chunks {
                bytes.extend_from_slice(chunk);
            }
            bytes
        };
        let result = match kind {
            ReadKind::Chunks => {
                let chunks = self.wire.lock().unwrap().chunks.clone();
                s.wrap::<crate::cache_storage::StoredBody>(chunks, None).map(|o| o.value())
            }
            ReadKind::Text => Ok(
                s.string(String::from_utf8_lossy(bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes)).as_ref())
            ),
            ReadKind::Json => {
                let text = pyro_js::JsString::new(
                    String::from_utf8_lossy(bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes)).as_ref(),
                );
                // SAFETY: local string/context are live; the returned value is
                // immediately wrapped and settled before any other JS call.
                let raw = unsafe { pyro_js::sys::JSValueMakeFromJSONString(s.ctx(), text.as_raw()) };
                if raw.is_null() {
                    Err(JsError::Syntax("Invalid JSON response body".into()))
                } else {
                    Ok(unsafe { Value::from_raw(s.ctx(), raw) })
                }
            }
            ReadKind::ArrayBuffer => crate::encoding::array_buffer(s, &bytes),
            ReadKind::Bytes => crate::encoding::uint8_array(s, &bytes),
            ReadKind::Blob | ReadKind::FormData => package_body(s, kind, bytes, headers),
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
                    self.settle(&s, reader.promise, reader.kind, reader.headers.as_ref());
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
        self.settle_stream_reads();
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
        drop(self.stream_reads.take());
        self.close_stream();
    }

    // ----- response.body (ReadableStream) ------------------------------------

    /// Starts streaming delivery for a `response.body` reader. The stream
    /// takes over the Response's body consumer.
    pub(crate) fn open_stream(&self) {
        self.stream_open.set(true);
        self.wire.lock().unwrap().streaming = true;
    }
    fn close_stream(&self) {
        if self.stream_open.replace(false) {
            self.wire.lock().unwrap().streaming = false;
            self.release_consumer();
        }
    }
    /// `ReadableStreamDefaultReader.read()`: the bytes received since the
    /// previous read, `done` once the body completed, or the body's error.
    pub(crate) fn stream_read<'s>(&self, s: &Scope<'s>) -> JsResult<Value<'s>> {
        let deferred = s.deferred()?;
        let promise = deferred.promise.get(s);
        if !self.stream_open.get() {
            deferred.resolve(s, read_result(s, s.undefined(), true)?)?;
            return Ok(promise);
        }
        self.stream_reads.borrow_mut().push((s.realm().downgrade(), deferred));
        self.settle_stream_reads();
        Ok(promise)
    }
    /// Resolves pending reads from buffered chunks, then completion or error.
    pub(crate) fn settle_stream_reads(&self) {
        loop {
            if self.stream_reads.borrow().is_empty() {
                return;
            }
            let (weak, deferred) = self.stream_reads.borrow_mut().remove(0);
            let Some(realm) = weak.upgrade() else { continue };
            let Ok(s) = realm.scope() else { continue };
            // A clone of the Response still reads the whole body: the stream
            // copies what it has not delivered and leaves the chunks buffered.
            let shared = self.consumers.get() > 1;
            let chunk = {
                let mut wire = self.wire.lock().unwrap();
                let skip = self.stream_cursor.get();
                if wire.chunks.is_empty() || wire.bytes <= skip {
                    None
                } else if shared {
                    let mut bytes = Vec::with_capacity(wire.bytes - skip);
                    let mut offset = 0;
                    for c in &wire.chunks {
                        let end = offset + c.len();
                        if end > skip {
                            bytes.extend_from_slice(&c[skip.saturating_sub(offset)..]);
                        }
                        offset = end;
                    }
                    self.stream_cursor.set(wire.bytes);
                    Some(bytes)
                } else {
                    let chunks = std::mem::take(&mut wire.chunks);
                    let len: usize = chunks.iter().map(Bytes::len).sum();
                    wire.bytes = wire.bytes.saturating_sub(len);
                    wire.budget.used.fetch_sub(len, Ordering::AcqRel);
                    self.stream_cursor.set(0);
                    let mut bytes = Vec::with_capacity(len);
                    for c in &chunks {
                        bytes.extend_from_slice(c);
                    }
                    bytes.drain(..skip.min(bytes.len()));
                    (!bytes.is_empty()).then_some(bytes)
                }
            };
            let error = self.error(&s);
            let done = self.is_null() || matches!(*self.visible_terminal.borrow(), Some(Ok(())));
            if chunk.is_none() && error.is_none() && !done {
                self.stream_reads.borrow_mut().insert(0, (weak, deferred));
                return;
            }
            let settled = (|| -> JsResult<()> {
                if let Some(bytes) = chunk.filter(|_| error.is_none()) {
                    let value = crate::encoding::uint8_array(&s, &bytes)?;
                    deferred.resolve(&s, read_result(&s, value, false)?)?;
                } else if let Some(error) = error {
                    deferred.reject(&s, error)?;
                    self.close_stream();
                } else {
                    deferred.resolve(&s, read_result(&s, s.undefined(), true)?)?;
                    self.close_stream();
                }
                Ok(())
            })();
            if settled.is_err() {
                self.close_stream();
            }
        }
    }
    /// One body stream was canceled: the transfer stops unless another
    /// consumer (a clone of the Response) still reads the whole body, which
    /// keeps the bytes; this stream just closes, its pending reads dropped
    /// (the canceled stream ignores them) and its cursor reset.
    pub(crate) fn cancel_consumer_stream(&self) {
        if self.consumers.get() > 1 {
            drop(self.stream_reads.take());
            self.stream_cursor.set(0);
            self.close_stream();
            return;
        }
        self.cancel_stream();
    }
    /// `ReadableStream.cancel()` / reader `cancel()`: stops the request,
    /// drops buffered bytes and finishes pending reads as done.
    pub(crate) fn cancel_stream(&self) {
        {
            let mut wire = self.wire.lock().unwrap();
            wire.closed = true;
            wire.clear_body();
            if wire.terminal.is_none() {
                wire.terminal = Some(Ok(()));
            }
        }
        if self.visible_terminal.borrow().is_none() {
            *self.visible_terminal.borrow_mut() = Some(Ok(()));
        }
        if let Some(page) = self.page.upgrade() {
            if let Some(net) = page.net.borrow().as_ref() {
                net.cancel(self.id);
            }
            page.fetch_state.jobs.borrow_mut().remove(&self.id);
        }
        self.settle_stream_reads();
        self.close_stream();
    }
}

pub(crate) fn cancel_realm(page: &PageDom, realm: RealmId) {
    crate::event_source::cancel_realm(page, realm);
    let bodies: Vec<_> =
        page.fetch_state.bodies.borrow().values().filter_map(Weak::upgrade).filter(|b| b.realm_id == realm).collect();
    for body in bodies {
        if !body.keepalive {
            if let Some(net) = page.net.borrow().as_ref() {
                net.cancel(body.id);
            }
        }
        let job = page.fetch_state.jobs.borrow_mut().remove(&body.id);
        body.dispose();
        drop(job);
    }
}
pub(crate) fn dispose(page: &PageDom) {
    // A discarded document sends its deferred fetches (Fetch "process deferred fetches").
    crate::fetch_later::flush(page);
    crate::event_source::dispose(page);
    PAGES.with(|pages| pages.borrow_mut().remove(&page.id()));
    let bodies: Vec<_> = page.fetch_state.bodies.borrow().values().filter_map(Weak::upgrade).collect();
    for body in bodies {
        // A keepalive fetch outlives its document (Fetch "keepalive"): its
        // transport runs on and the page-side state goes.
        if !body.keepalive {
            if let Some(net) = page.net.borrow().as_ref() {
                net.cancel(body.id);
            }
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
            frontier: NetworkDelivery::default(),
            head: None,
            chunks: Vec::new(),
            bytes: 0,
            terminal: None,
            closed: false,
            scheduled: false,
            null_body: false,
            streaming: false,
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
        let frontier = wire.lock().unwrap().frontier;
        assert_eq!(
            frontier,
            NetworkDelivery { response: false, data_bytes: 4, terminal: false },
            "quota is not transport terminal"
        );
        for _ in 0..100 {
            receive(&wire, 0, RequestId(1), &poster, limits, ResponseEvent::Data(Bytes::from_static(b"x")));
        }
        assert_eq!(posts.load(Ordering::Relaxed), 1, "closed delivery must ignore late producer calls");
        assert_eq!(budget.used.load(Ordering::Relaxed), 0);
    }
}

/// This module's part of [`crate::thread_state::reset_thread`]: what of its
/// per-thread state still holds something, if any does.
pub(crate) fn thread_leftover() -> Option<&'static str> {
    PAGES.with(|p| p.borrow_mut().retain(|_, page| page.strong_count() > 0));
    PAGES.with(|p| !p.borrow().is_empty()).then_some("fetch::PAGES")
}
