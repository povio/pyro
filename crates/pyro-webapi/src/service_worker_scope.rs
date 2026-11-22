//! `ServiceWorkerGlobalScope`: the part of a service worker that runs on its
//! thread. The thread, VM, loop and empty document are a dedicated worker's
//! (`worker_scope`); what differs is the global (`ServiceWorkerGlobalScope`,
//! with `clients`, `registration`, `serviceWorker`, `skipWaiting()` and the
//! `install`/`activate`/`fetch`/`message` handlers) and the script-level glue
//! (`service_worker_scope.js`): the event classes, `Clients`/`Client`, and the
//! dispatch of what the browser thread sends (`service_worker.rs`).
//!
//! Intercepted requests: the page's `fetch()`/XHR hand their `pyro_net::Request`
//! and response sink here ([`FetchJob`]). The `fetch` event gets a `Request`
//! made from it. `respondWith(response)` reads the response's body whole and
//! answers the sink as the network would (status, headers, one data chunk);
//! no `respondWith` sends the request on to the network with the sink, so
//! Playwright's routing and the Network domain see it as any other request;
//! a rejected or invalid response is a network error. A job the worker never
//! answers (it stopped) fails the request; one that never reached the worker
//! goes to the network.
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use pyro_js::{JsError, JsResult, Persistent, Scope, Value};
use pyro_net::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use pyro_net::{NetClient, NetError, Request, RequestId, ResponseEvent, ResponseHead, ResponseKind, ResponseSink};
use rustc_hash::FxHashMap;

use crate::page::{PageDom, RemotePoster};
use crate::service_worker::{deliver, Lifecycle, QueryKind, SwUp};
use crate::worker::{Shared, MAX_QUEUED_BYTES};

/// What a service worker's thread is started with.
#[derive(Clone)]
pub(crate) struct ServiceStart {
    pub version: u64,
    pub registration: u64,
    pub(crate) partition_key: Option<String>,
    pub(crate) cookie_site_context: pyro_net::cookies::SiteContext,
    pub(crate) cookies: Arc<crate::cookie_store::subscriptions::Subscriptions>,
    pub scope: String,
    pub script_url: String,
    pub update_via_cache: String,
}

/// A request a controlled client made, on its way through the worker.
pub(crate) struct FetchJob {
    pub id: RequestId,
    pub request: Option<Request>,
    pub sink: Option<ResponseSink>,
    pub net: NetClient,
    pub client_id: String,
    /// A navigation's: the id of the client it creates.
    pub resulting_client_id: String,
    /// A `fetch` event has it: dropping it unanswered is a network error.
    pub dispatched: bool,
    pub observed: bool,
}

impl Drop for FetchJob {
    fn drop(&mut self) {
        let (Some(request), Some(mut sink)) = (self.request.take(), self.sink.take()) else { return };
        if self.dispatched {
            sink(ResponseEvent::Failed(NetError::Other("The service worker stopped before it responded".into())));
        } else {
            self.net.fetch_with_id(self.id, request, sink);
        }
    }
}

thread_local! {
    /// This worker's glue object (service_worker_scope.js); cleared before the VM goes.
    static GLUE: RefCell<Option<Persistent>> = const { RefCell::new(None) };
    /// Fetch events waiting for their answer, by key.
    static PENDING: RefCell<FxHashMap<u64, FetchJob>> = RefCell::default();
    static NEXT_KEY: Cell<u64> = const { Cell::new(1) };
    /// Queries sent to the browser thread and not answered yet.
    static QUERIES: Cell<usize> = const { Cell::new(0) };
}

/// Queries (`clients.get`, `matchAll`, ...) a worker may have unanswered.
const MAX_PENDING_QUERIES: usize = 4096;

/// A message's bytes counted against the worker's upward budget
/// (`Shared::up_bytes`) until it was delivered or dropped.
pub(crate) struct Queued {
    shared: Arc<Shared>,
    size: usize,
}

impl Queued {
    fn new(shared: Arc<Shared>, size: usize) -> Self {
        shared.up_bytes.fetch_add(size, Ordering::AcqRel);
        Queued { shared, size }
    }
}

impl Drop for Queued {
    fn drop(&mut self) {
        self.shared.up_bytes.fetch_sub(self.size, Ordering::AcqRel);
    }
}

/// Drops what holds handles into the VM, and the unanswered fetches (each
/// fails its request).
pub(crate) fn tear_down() {
    drop(GLUE.with(|g| g.take()));
    drop(PENDING.with(|p| std::mem::take(&mut *p.borrow_mut())));
}

fn next_key() -> u64 {
    NEXT_KEY.with(|k| {
        let key = k.get();
        k.set(key + 1);
        key
    })
}

fn glue<'s>(s: &Scope<'s>) -> JsResult<pyro_js::Object<'s>> {
    let value = GLUE.with(|g| g.borrow().as_ref().map(|p| p.get(s)));
    value.and_then(|v| v.as_object()).ok_or(JsError::ContextDestroyed)
}

/// A member of the glue object (`clients`, `registration`, ...).
pub(crate) fn member<'s>(s: &Scope<'s>, name: &str) -> JsResult<Value<'s>> {
    glue(s)?.get(name)
}

fn call(s: &Scope<'_>, name: &str, args: &[Value<'_>]) {
    let result = (|| {
        let function = glue(s)?.get(name)?;
        let function = function.as_object().ok_or(JsError::ContextDestroyed)?;
        let args: Vec<Value<'_>> = args.iter().map(|v| s.rehome(*v)).collect();
        function.call(None, &args)
    })();
    if let Err(error) = result {
        if !matches!(error, JsError::Terminated | JsError::ContextDestroyed) {
            s.report_exception(&error);
        }
    }
}

/// Makes the glue in a new service worker realm, before its script runs.
pub(crate) fn install(s: &Scope<'_>, start: &ServiceStart, poster: RemotePoster, shared: Arc<Shared>) -> JsResult<()> {
    let version = start.version;
    let native = s.object();
    let self_post = s.function("postToSelf", |s, _, a| {
        let page = crate::target::page_of(s)?;
        let options = crate::structured_clone::transfer_options(s, a.opt(1))?;
        let payload = crate::structured_clone::encode_message(s, a.get(0), options)?;
        let origin = page.origin_key();
        let realm = s.realm().downgrade();
        let receiver = Rc::downgrade(&page);
        page.post_task(Box::new(move || {
            let (Some(realm), Some(page)) = (realm.upgrade(), receiver.upgrade()) else { return };
            if page.is_disposed() { return }
            if let Ok(s) = realm.scope() {
                message(&s, &payload, None, &origin);
            }
        }));
        Ok(s.undefined())
    })?;
    native.set_atom("postToSelf", self_post.value())?;
    let subscriptions = start.cookies.clone();
    let script_url = pyro_net::Url::parse(&start.script_url).map_err(|_| JsError::ContextDestroyed)?;
    let manager = s.function("cookieManager", move |s, _, _| {
        crate::cookie_store::subscriptions::manager(s, &crate::target::page_of(s)?, subscriptions.clone(), Some(script_url.clone()))
    })?;
    native.set_atom("cookieManager", manager.value())?;
    let trusted = s.function("dispatchTrustedEvent", |s, _, a| {
        if let Some(origin) = a.opt(1).filter(|origin| !origin.is_undefined()) {
            let event = s.unwrap::<crate::event::Event>(a.get(0)).ok_or(JsError::ContextDestroyed)?;
            event.fields.borrow_mut().set_sender_origin(origin.to_string()?);
        }
        crate::cookie_store::events::dispatch_trusted(s, s.global().value(), a.get(0))?;
        Ok(s.undefined())
    })?;
    native.set_atom("dispatchTrustedEvent", trusted.value())?;
    let up = move |poster: &RemotePoster, message: SwUp| {
        poster(Box::new(move || deliver(version, message)));
    };
    let (p, next_query) = (poster.clone(), Rc::new(Cell::new(1u64)));
    let query = s.function("query", move |s, _, a| {
        if QUERIES.with(Cell::get) >= MAX_PENDING_QUERIES {
            return Err(JsError::dom("QuotaExceededError", "Too many pending requests to the browser."));
        }
        let id = next_query.get();
        next_query.set(id + 1);
        let kind = match a.get(0).to_string()?.as_str() {
            "skipWaiting" => QueryKind::SkipWaiting,
            "claim" => QueryKind::Claim,
            "matchAll" => QueryKind::MatchAll { include_uncontrolled: a.get(1).to_bool(), kind: a.get(2).to_string()? },
            "get" => QueryKind::Get(a.get(1).to_string()?),
            "unregister" => QueryKind::Unregister,
            "update" => QueryKind::Update,
            other => return Err(JsError::Type(format!("unknown query {other}").into())),
        };
        QUERIES.with(|q| q.set(q.get() + 1));
        up(&p, SwUp::Query { id, kind });
        Ok(s.number(id as f64))
    })?;
    native.set_atom("query", query.value())?;
    let p = poster.clone();
    let post = s.function("postToClient", move |s, _, a| {
        let client = a.get(0).to_string()?;
        let options = crate::structured_clone::transfer_options(s, a.opt(2))?;
        let message = crate::structured_clone::encode_message(s, a.get(1), options)?;
        // As a dedicated worker's postMessage: past the budget, wait for the
        // browser thread to take what is queued. One message may exceed it alone.
        let size = message.size();
        loop {
            if shared.closing.load(Ordering::Acquire) {
                return Ok(s.undefined());
            }
            let queued = shared.up_bytes.load(Ordering::Acquire);
            if queued == 0 || queued.saturating_add(size) <= MAX_QUEUED_BYTES {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        let queued = Queued::new(shared.clone(), size);
        up(&p, SwUp::PostToClient { client, message, queued });
        Ok(s.undefined())
    })?;
    native.set_atom("postToClient", post.value())?;
    let p = poster.clone();
    let done = s.function("done", move |s, _, a| {
        let kind = if a.get(0).to_string()? == "install" { Lifecycle::Install } else { Lifecycle::Activate };
        up(&p, SwUp::Done { kind, ok: a.get(1).to_bool() });
        Ok(s.undefined())
    })?;
    native.set_atom("done", done.value())?;
    let observe = s.function("observeFetch", |s, _, a| {
        let key = a.get(0).to_number()? as u64;
        PENDING.with(|pending| {
            let mut pending = pending.borrow_mut();
            if let Some(job) = pending.get_mut(&key) {
                if !job.observed {
                    if let (Some(request), Some(sink)) = (job.request.as_ref(), job.sink.take()) {
                        job.sink = Some(job.net.observe_service_worker_response(job.id, request, sink));
                        job.observed = true;
                    }
                }
            }
        });
        Ok(s.undefined())
    })?;
    native.set_atom("observeFetch", observe.value())?;
    let respond = s.function("respond", |s, _, a| {
        let key = a.get(0).to_number()? as u64;
        let Some(job) = PENDING.with(|p| p.borrow_mut().remove(&key)) else { return Ok(s.undefined()) };
        respond(s, job, &a)?;
        Ok(s.undefined())
    })?;
    native.set_atom("respond", respond.value())?;
    let passthrough = s.function("passthrough", |s, _, a| {
        let key = a.get(0).to_number()? as u64;
        if let Some(mut job) = PENDING.with(|p| p.borrow_mut().remove(&key)) {
            // Dropped undispatched: the request goes to the network.
            job.dispatched = false;
        }
        Ok(s.undefined())
    })?;
    native.set_atom("passthrough", passthrough.value())?;
    let fail = s.function("failFetch", |s, _, a| {
        let key = a.get(0).to_number()? as u64;
        if let Some(mut job) = PENDING.with(|p| p.borrow_mut().remove(&key)) {
            let reason = a.get(1).to_string().unwrap_or_default();
            job.request.take();
            if let Some(mut sink) = job.sink.take() {
                sink(ResponseEvent::Failed(NetError::Other(format!("The service worker's response failed: {reason}"))));
            }
        }
        Ok(s.undefined())
    })?;
    native.set_atom("failFetch", fail.value())?;
    let info = s.object();
    info.set_atom("scope", s.string(&start.scope))?;
    info.set_atom("scriptURL", s.string(&start.script_url))?;
    info.set_atom("updateViaCache", s.string(&start.update_via_cache))?;
    info.set_atom("registrationId", s.number(start.registration as f64))?;
    info.set_atom("versionId", s.number(start.version as f64))?;
    let roots = (Persistent::new(s, native.value()), Persistent::new(s, info.value()));
    let factory =
        s.eval(include_str!("service_worker_scope.js"), Some("pyro://internal/service-worker-scope.js"), 1)?;
    let factory = factory.as_object().ok_or_else(|| JsError::Type("service worker glue".into()))?;
    let glue = factory.call(None, &[roots.0.get(s), roots.1.get(s)])?;
    GLUE.with(|g| *g.borrow_mut() = Some(Persistent::new(s, glue)));
    Ok(())
}

/// The `respondWith` response, as the network would have given it.
fn respond(s: &Scope<'_>, mut job: FetchJob, a: &pyro_js::Args<'_>) -> JsResult<()> {
    let (Some(request), Some(mut sink)) = (job.request.take(), job.sink.take()) else { return Ok(()) };
    let fail = |mut sink: ResponseSink, reason: &str| {
        sink(ResponseEvent::Failed(NetError::Other(format!("The service worker's response failed: {reason}"))));
    };
    let kind = match a.get(6).to_string()?.as_str() {
        "opaque" => ResponseKind::Opaque,
        "opaqueredirect" => ResponseKind::OpaqueRedirect,
        "cors" => ResponseKind::Cors,
        _ => ResponseKind::Basic,
    };
    // Fetch's "handle fetch" checks: an opaque response only answers a no-cors
    // request, a CORS one not a same-origin request, an opaque redirect only a
    // manual-redirect one.
    let unsuitable = match kind {
        ResponseKind::Opaque => request.mode != pyro_net::RequestMode::NoCors,
        ResponseKind::Cors => request.mode == pyro_net::RequestMode::SameOrigin,
        ResponseKind::OpaqueRedirect => request.redirect != pyro_net::RedirectMode::Manual,
        ResponseKind::Basic => false,
    };
    if unsuitable {
        fail(sink, "its type does not suit the request's mode");
        return Ok(());
    }
    let status = a.get(1).to_number()? as u16;
    let status = match (StatusCode::from_u16(status), kind) {
        (_, ResponseKind::Opaque | ResponseKind::OpaqueRedirect) => StatusCode::OK,
        (Ok(status), _) => status,
        (Err(_), _) => {
            fail(sink, "invalid status");
            return Ok(());
        }
    };
    let mut headers = HeaderMap::new();
    if let Some(list) = a.get(3).as_object() {
        let count = list.get_atom("length")?.to_number()? as u32;
        let mut i = 0;
        while i + 1 < count {
            let (name, value) = (list.get_index(i)?.to_string()?, list.get_index(i + 1)?.to_string()?);
            if let (Ok(name), Ok(value)) = (HeaderName::from_bytes(name.as_bytes()), HeaderValue::from_str(&value)) {
                headers.append(name, value);
            }
            i += 2;
        }
    }
    let body = crate::encoding::buffer_source_bytes(s, a.get(4))?.map(Bytes::from).unwrap_or_default();
    let url = a.get(5).to_string().ok().filter(|u| !u.is_empty()).and_then(|u| pyro_net::Url::parse(&u).ok());
    let (mime, charset) = headers
        .get(pyro_net::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(pyro_net::parse_content_type)
        .unwrap_or_default();
    let head = ResponseHead {
        url: url.unwrap_or_else(|| request.url.clone()),
        status,
        status_text: a.get(2).to_string()?,
        headers,
        mime,
        charset,
        redirected: false,
        from_cache: false,
        kind,
        remote_addr: None,
        protocol: pyro_net::http::Version::HTTP_11,
        // Made by the worker, not the network: no connection phases.
        timing: Default::default(),
    };
    let len = body.len() as u64;
    sink(ResponseEvent::Response(head));
    if !body.is_empty() {
        sink(ResponseEvent::Data(body));
    }
    sink(ResponseEvent::Finished { encoded_len: len, decoded_len: len });
    Ok(())
}

/// Moves this worker's own `state` (the browser thread's lifecycle step).
pub(crate) fn extendable_event<'s>(s: &Scope<'s>) -> JsResult<Value<'s>> {
    glue(s)?.get_atom("ExtendableEvent")
}

/// Fire a functional event in the active worker, using the same extension
/// and lifetime-promise machinery as its install/activate events.
pub(crate) fn cookie_change(s: &Scope<'_>, page: &Rc<PageDom>, changes: &[pyro_net::cookies::CookieChange]) {
    match crate::cookie_store::events::cookie_event(s, page, true, changes) {
        Ok(event) => call(s, "dispatchCookieChange", &[event]),
        Err(err) => s.report_exception(&err),
    }
}

pub(crate) fn set_state(s: &Scope<'_>, state: &str) {
    call(s, "state", &[s.string(state)]);
}

pub(crate) fn lifecycle(s: &Scope<'_>, kind: Lifecycle) {
    call(s, if kind == Lifecycle::Install { "install" } else { "activate" }, &[]);
}

pub(crate) fn reply(s: &Scope<'_>, id: u64, result: Result<String, (String, String)>) {
    QUERIES.with(|q| q.set(q.get().saturating_sub(1)));
    match result {
        Ok(json) => call(s, "reply", &[s.number(id as f64), s.boolean(true), s.string(&json)]),
        Err((name, message)) => call(
            s,
            "reply",
            &[s.number(id as f64), s.boolean(false), s.undefined(), s.string(&name), s.string(&message)],
        ),
    }
}

/// A message to this worker. An absent client description denotes a
/// message posted by the service worker to itself.
pub(crate) fn message(
    s: &Scope<'_>,
    message: &crate::structured_clone::WireMessage,
    client: Option<&str>,
    origin_key: &str,
) {
    let source = client.and_then(|json| s.parse_json(json)).unwrap_or_else(|| s.null());
    let origin = s.string(crate::event::serialize_message_origin(origin_key));
    let key = s.string(origin_key);
    match crate::structured_clone::decode_with_ports(s, message) {
        Ok((data, ports)) => call(
            s,
            "dispatchMessage",
            &[data, ports.map_or_else(|| s.undefined(), |p| p.value()), source, origin, key],
        ),
        Err(JsError::Terminated) => {}
        Err(_) => call(s, "dispatchMessageError", &[source, origin, key]),
    }
}

fn mode_name(mode: pyro_net::RequestMode) -> &'static str {
    match mode {
        pyro_net::RequestMode::Navigate => "navigate",
        pyro_net::RequestMode::SameOrigin => "same-origin",
        pyro_net::RequestMode::NoCors => "no-cors",
        pyro_net::RequestMode::Cors => "cors",
    }
}

fn credentials_name(credentials: pyro_net::Credentials) -> &'static str {
    match credentials {
        pyro_net::Credentials::Omit => "omit",
        pyro_net::Credentials::SameOrigin => "same-origin",
        pyro_net::Credentials::Include => "include",
    }
}

fn cache_name(cache: pyro_net::CacheMode) -> &'static str {
    match cache {
        pyro_net::CacheMode::Default => "default",
        pyro_net::CacheMode::NoStore => "no-store",
        pyro_net::CacheMode::Reload => "reload",
        pyro_net::CacheMode::NoCache => "no-cache",
        pyro_net::CacheMode::ForceCache => "force-cache",
        pyro_net::CacheMode::OnlyIfCached => "only-if-cached",
    }
}

fn redirect_name(redirect: pyro_net::RedirectMode) -> &'static str {
    match redirect {
        pyro_net::RedirectMode::Follow => "follow",
        pyro_net::RedirectMode::Error => "error",
        pyro_net::RedirectMode::Manual => "manual",
    }
}

fn destination_name(destination: pyro_net::Destination) -> &'static str {
    match pyro_net::referrer::fetch_dest(destination) {
        "empty" => "",
        other => other,
    }
}

/// Fires a `fetch` event for `job`.
pub(crate) fn fetch(s: &Scope<'_>, mut job: FetchJob) {
    let Some(request) = job.request.as_ref() else { return };
    let headers = request
        .headers
        .iter()
        .map(|(n, v)| (n.as_str().to_owned(), String::from_utf8_lossy(v.as_bytes()).into_owned()))
        .collect();
    let intercepted = crate::request::Intercepted {
        method: request.method.as_str().to_owned(),
        url: request.url.to_string(),
        mode: mode_name(request.mode),
        credentials: credentials_name(request.credentials),
        cache: cache_name(request.cache),
        redirect: redirect_name(request.redirect),
        referrer: request.referrer.as_ref().map_or_else(|| "no-referrer".to_owned(), |u| u.to_string()),
        referrer_policy: request.referrer_policy.as_str().to_owned(),
        integrity: String::new(),
        keepalive: false,
        destination: destination_name(request.destination),
        headers,
        body: request.body.clone(),
    };
    let value = match crate::request::intercepted(s, intercepted) {
        Ok(value) => value,
        // The job drops undispatched: the request goes to the network.
        Err(_) => return,
    };
    let key = next_key();
    let (client, resulting) = (job.client_id.clone(), job.resulting_client_id.clone());
    job.dispatched = true;
    PENDING.with(|p| p.borrow_mut().insert(key, job));
    call(s, "dispatchFetch", &[s.number(key as f64), value, s.string(&client), s.string(&resulting)]);
}

/// This module's part of [`crate::thread_state::reset_thread`]: what of its
/// per-thread state still holds something, if any does.
pub(crate) fn thread_leftover() -> Option<&'static str> {
    if GLUE.with(|g| g.borrow().is_some()) {
        return Some("service_worker_scope::GLUE");
    }
    if PENDING.with(|p| !p.borrow().is_empty()) {
        return Some("service_worker_scope::PENDING");
    }
    if QUERIES.get() != 0 {
        return Some("service_worker_scope::QUERIES");
    }
    NEXT_KEY.set(1);
    None
}
