//! HTTP client on a dedicated I/O thread (tokio current-thread runtime +
//! hyper). The browser thread submits requests through a channel and
//! receives ordered `ResponseEvent`s through the per-request sink, which the
//! browser wraps to post onto its event loop. No JS or DOM state is touched
//! here.
//!
//! Body chunks are `Bytes` (refcounted) end to end: the consumer, observers
//! and CDP body retention share the same allocation instead of upstream's
//! 2-3 copies per body (HttpClient.zig:4241 -> Fetch.zig:302 / captured
//! responses).

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use http::{header, HeaderMap, HeaderValue, Method, StatusCode, Version};
use http_body_util::{BodyExt, Full};
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use tokio::sync::{mpsc, oneshot, Semaphore};
use url::Url;

use crate::auth::auth_challenge;
use crate::cookies::{site_context, CookieJar, SiteContext};
use crate::cors::{self, PreflightCache};
use crate::h2_probe::{H2Sessions, ProbeConnector};
use crate::inflate::{Format as InflateFormat, Inflater};
use crate::proxy::{basic_authorization, ProxyConfig, ProxyConnector, ProxyError, ProxyState};
use crate::referrer::FetchSite;
use crate::request::*;
use crate::subscription::{
    flush_deferred_wakes, DeferredWakes, NetSubscription, NetWake, SubscriptionLimits, Subscriptions,
};

#[path = "websocket.rs"]
pub mod websocket;

/// Consumer of one request's events; wrap it to post to the browser loop.
///
/// Guarantees: every sink passed to [`NetClient::fetch`] receives exactly one
/// terminal event (`Finished` or `Failed`), after which it is dropped.
/// Events normally arrive on the I/O thread. If the I/O thread has already
/// stopped, `fetch` calls the sink synchronously on the caller's thread with
/// `Failed(Other("network thread stopped"))`. When the last `NetClient`
/// clone is dropped, in-flight requests receive `Failed(Aborted)` on the I/O
/// thread before it exits.
pub type ResponseSink = Box<dyn FnMut(ResponseEvent) + Send>;

#[derive(Clone, Debug)]
pub struct NetConfig {
    pub user_agent: String,
    pub accept_language: String,
    pub max_redirects: u32,
    pub connect_timeout: Duration,
    /// Default decoded-body cap when the request sets none.
    pub max_body: usize,
    /// Enforce CORS for cross-origin `cors`-mode requests (upstream: the
    /// `--experimental-features cors` flag, which TOS enables).
    pub enforce_cors: bool,
    /// Concurrent connections per origin (Chromium uses 6).
    pub max_per_origin: usize,
    /// Concurrent requests per origin once it has answered over HTTP/2, on
    /// its shared connection (Chromium's default stream limit is 100 too).
    pub max_per_h2_origin: usize,
    /// Maximum credential submissions for one HTTP hop. No unbounded auth loops.
    pub max_auth_retries: u32,
    /// Private in-memory HTTP cache budget for this client (one browser
    /// context), in logical bytes. 0 disables the cache.
    pub http_cache_max_bytes: usize,
    /// Largest single response body the HTTP cache stores.
    pub http_cache_max_entry_bytes: usize,
    /// Opt-in cross-context cache of public static assets (one handle per
    /// browser process). `None` (the default unless `PYRO_SHARED_ASSET_CACHE_MB`
    /// is set) keeps every context fully isolated.
    pub shared_asset_cache: Option<crate::cache::SharedAssetCache>,
    /// Directories answering GET/HEAD requests for whole origins from disk
    /// (`serve --static ORIGIN=DIR`); empty by default.
    pub static_files: crate::static_files::StaticFiles,
    /// Let embedders answer `static_files` hits on their own thread from the
    /// preloaded bytes (`NetClient::report_local`) instead of sending the
    /// request through the I/O thread. Default true; `PYRO_STATIC_DIRECT=0`
    /// turns it off (benchmark comparison).
    pub static_direct: bool,
    /// HTTP forward proxy for this client's requests (`serve --proxy`,
    /// `Target.createBrowserContext proxyServer`).
    pub proxy: Option<ProxyConfig>,
    /// Serve `file:` URLs from the local filesystem (navigations and the
    /// subresources of `file:` documents), as Chrome does. Default true;
    /// `PYRO_FILE_URLS=0` turns it off (`file:` then fails with
    /// `net::ERR_UNKNOWN_URL_SCHEME`).
    pub file_urls: bool,
}

impl Default for NetConfig {
    fn default() -> Self {
        NetConfig {
            user_agent: format!("Mozilla/5.0 (compatible; pyro/{})", env!("CARGO_PKG_VERSION")),
            accept_language: "en-US,en;q=0.9".into(),
            max_redirects: 20,
            connect_timeout: Duration::from_secs(10),
            max_body: 256 * 1024 * 1024,
            enforce_cors: true,
            max_per_origin: 6,
            max_per_h2_origin: 100,
            max_auth_retries: 3,
            http_cache_max_bytes: 64 * 1024 * 1024,
            http_cache_max_entry_bytes: 8 * 1024 * 1024,
            shared_asset_cache: crate::cache::SharedAssetCache::from_env(),
            static_files: Default::default(),
            static_direct: std::env::var("PYRO_STATIC_DIRECT").map_or(true, |v| v != "0"),
            proxy: None,
            file_urls: std::env::var("PYRO_FILE_URLS").map_or(true, |v| v != "0"),
        }
    }
}

/// Mutable network conditions set via CDP.
#[derive(Clone, Debug, Default)]
pub struct NetSettings {
    /// Network.setExtraHTTPHeaders.
    pub extra_headers: HeaderMap,
    /// Network.emulateNetworkConditions({offline}).
    pub offline: bool,
    /// Network.setUserAgentOverride / Emulation.setUserAgentOverride.
    pub user_agent: Option<String>,
    pub accept_language: Option<String>,
    /// Context-wide HTTP cache bypass (no lookup, no store; sends
    /// `Pragma: no-cache` / `Cache-Control: no-cache`, like DevTools). CDP
    /// `Network.setCacheDisabled` is page-scoped and uses
    /// `RequestDefaults::cache_disabled` instead; this is for embedders that
    /// want the whole context uncached.
    pub cache_disabled: bool,
}

/// Snapshot of a request as sent (after cookies and extra headers).
#[derive(Clone, Debug)]
pub struct RequestInfo {
    pub url: Url,
    pub method: Method,
    pub headers: HeaderMap,
    pub post_data: Option<Bytes>,
    pub destination: Destination,
    pub frame_id: Option<String>,
    pub loader_id: Option<String>,
}

/// Events for CDP Network.* (owned by the automation layer).
#[derive(Clone, Debug)]
pub enum NetEvent {
    RequestWillBeSent {
        id: RequestId,
        request: Box<RequestInfo>,
        redirect_response: Option<Box<ResponseHead>>,
        timestamp: f64,
        wall_time: f64,
    },
    ResponseReceived {
        id: RequestId,
        head: Box<ResponseHead>,
        destination: Destination,
        frame_id: Option<String>,
        loader_id: Option<String>,
        timestamp: f64,
    },
    DataReceived {
        id: RequestId,
        data: Bytes,
        timestamp: f64,
    },
    LoadingFinished {
        id: RequestId,
        encoded_len: u64,
        timestamp: f64,
    },
    LoadingFailed {
        id: RequestId,
        error: NetError,
        canceled: bool,
        destination: Destination,
        timestamp: f64,
    },
    /// A WebSocket connection's progress (`Network.webSocket*`).
    WebSocket {
        id: RequestId,
        /// Boxed: `NetEvent`s of every request stay their size.
        activity: Box<websocket::WebSocketActivity>,
        timestamp: f64,
    },
}

/// A request paused for CDP Fetch interception (request stage).
#[derive(Clone, Debug)]
pub struct PausedRequest {
    pub id: RequestId,
    pub request: RequestInfo,
}

/// A 401/407 challenge. Callbacks run on the I/O thread and must not block.
#[derive(Clone, Debug)]
pub struct AuthChallenge {
    pub id: RequestId,
    pub request: RequestInfo,
    pub origin: String,
    pub scheme: String,
    pub realm: String,
    pub proxy: bool,
}

/// Basic credentials use UTF-8; other schemes return explicit errors. Proxy
/// credentials answered once are kept for the context, like a browser's; there
/// is no server credential cache.
#[derive(Clone)]
pub enum AuthDecision {
    /// Return the challenge response; this headless client has no credential
    /// cache or interactive authentication prompt.
    Default,
    /// Decline credentials and deliver the original 401/407 response.
    CancelAuth,
    ProvideCredentials {
        username: String,
        password: String,
    },
}

#[derive(Clone, Debug)]
pub enum InterceptDecision {
    Continue {
        url: Option<Url>,
        method: Option<Method>,
        headers: Option<HeaderMap>,
        body: Option<Bytes>,
    },
    Fulfill {
        status: u16,
        headers: HeaderMap,
        body: Bytes,
    },
    /// CDP `Network.ErrorReason`, e.g. "Failed", "Aborted", "BlockedByClient".
    Fail(String),
    /// Continue unmodified, then pause the final response (after redirects,
    /// with its whole decoded body) at [`Interceptor::response`] before
    /// anything of it is delivered or observed. Such a response is not cached.
    PauseResponse,
}

/// A response paused at [`Interceptor::response`]: the real status, headers
/// and decoded body of the request's final hop.
#[derive(Clone, Debug)]
pub struct PausedResponse {
    pub id: RequestId,
    pub request: RequestInfo,
    pub status: u16,
    pub headers: HeaderMap,
    pub body: Bytes,
}

#[derive(Clone, Debug)]
pub enum ResponseDecision {
    /// Deliver the real response.
    Continue,
    /// Deliver this instead (with the response kind the real one had).
    Fulfill { status: u16, headers: HeaderMap, body: Bytes },
    /// CDP `Network.ErrorReason`.
    Fail(String),
}

/// Fetch-domain interception. `paused` must not block; answer through
/// `reply` whenever the CDP decision arrives. Dropping `reply` fails the
/// request with net::ERR_FAILED.
///
/// Authentication is opt-in. Request cancellation or client shutdown closes
/// pending reply receivers and emits exactly one terminal event. Replacing the
/// interceptor affects subsequent hops; embedders must resolve/cancel already
/// paused requests when disabling interception. Never retain a NetClient clone
/// inside its interceptor (that would prevent last-client shutdown).
pub trait Interceptor: Send + Sync {
    fn paused(&self, req: PausedRequest, reply: oneshot::Sender<InterceptDecision>);
    /// The response of a request `paused` answered with
    /// [`InterceptDecision::PauseResponse`]. Must not block; dropping `reply`
    /// fails the request.
    fn response(&self, _resp: PausedResponse, reply: oneshot::Sender<ResponseDecision>) {
        let _ = reply.send(ResponseDecision::Continue);
    }
    fn handles_auth(&self) -> bool {
        false
    }
    /// Opt-in: `paused` may be called for a request's first hop on the
    /// thread that starts the request, and the thread that answers then
    /// delivers a `Fulfill` or a failure itself (no I/O thread, no async
    /// runtime around either call). Asked once per request.
    fn pauses_on_caller(&self) -> bool {
        false
    }
    fn auth_required(&self, _challenge: AuthChallenge, reply: oneshot::Sender<AuthDecision>) {
        let _ = reply.send(AuthDecision::Default);
    }
}

enum Cmd {
    /// The request, its sink, when the caller started it, and what the
    /// caller's thread already did of its first hop ([`Staged`]).
    Fetch(RequestId, Box<Request>, ResponseSink, Instant, Option<Staged>),
    Cancel(RequestId),
    WebSocket(Box<websocket::Start>),
}

struct Inner {
    /// Replaced when the I/O thread restarts after a fork ([`suspend_for_fork`]).
    tx: Mutex<mpsc::UnboundedSender<Cmd>>,
    next_id: AtomicU64,
    cookies: Arc<Mutex<CookieJar>>,
    shared: Arc<Shared>,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
    activity: Arc<Mutex<ActivityMap>>,
    /// Requests paused at the request stage off the I/O thread
    /// ([`LocalPause`]), until their decision arrives.
    local: Mutex<HashMap<RequestId, Arc<LocalPause>>>,
}

/// Request activity of one loader (document) on this client: requests started
/// with that `Request::loader_id` and not yet terminal. Lets the browser detect
/// network idleness without racing the I/O thread's event publication.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NetActivity {
    pub inflight: usize,
    /// Established successful text/event-stream responses that remain in flight.
    pub event_streams: usize,
    /// When `inflight` last reached 0, if it is 0 now.
    pub idle_since: Option<Instant>,
    /// When `inflight` last reached at most 2, if it is at most 2 now.
    pub almost_idle_since: Option<Instant>,
}

#[derive(Default)]
struct ActivityMap {
    loaders: HashMap<String, LoaderActivity>,
    generation: u64,
    /// Requests tracked so far, over every loader. Each takes the count as
    /// its `order`, so one reading splits the requests in flight into those
    /// started before it and those started after ([`NetClient::request_mark`]).
    started: u64,
}

struct LoaderActivity {
    /// Tells this entry from an earlier one of the same loader that was
    /// forgotten: that one's requests must not touch it.
    generation: u64,
    counts: NetActivity,
    inflight: Vec<InFlight>,
}

/// A request in flight for a loader.
struct InFlight {
    id: RequestId,
    /// Its place among the requests the client tracked ([`ActivityMap::started`]).
    order: u64,
    /// An established event stream.
    stream: bool,
}

/// Decrements its loader's activity exactly once, when pyro-net drops the
/// request's sink after the terminal event (finished, failed, canceled, aborted
/// or shutdown). A forgotten loader's late terminals do not touch a newer entry.
struct ActivityGuard {
    event_stream: bool,
    map: Arc<Mutex<ActivityMap>>,
    loader: String,
    generation: u64,
    order: u64,
}

impl Drop for ActivityGuard {
    fn drop(&mut self) {
        let mut map = self.map.lock().unwrap();
        if let Some(l) = map.loaders.get_mut(&self.loader) {
            if l.generation == self.generation {
                l.inflight.retain(|r| r.order != self.order);
                let a = &mut l.counts;
                a.inflight = a.inflight.saturating_sub(1);
                if self.event_stream {
                    a.event_streams = a.event_streams.saturating_sub(1);
                }
                let now = Instant::now();
                if a.inflight == 0 && a.idle_since.is_none() {
                    a.idle_since = Some(now);
                }
                if a.inflight <= 2 && a.almost_idle_since.is_none() {
                    a.almost_idle_since = Some(now);
                }
            }
        }
    }
}

struct Shared {
    /// The I/O thread is stopping for a fork, not for good: keep observers open.
    suspending: std::sync::atomic::AtomicBool,
    config: NetConfig,
    settings: Mutex<NetSettings>,
    observers: Subscriptions,
    interceptor: Mutex<Option<Arc<dyn Interceptor>>>,
    cache: Mutex<crate::cache::HttpCache>,
    epoch: Instant,
    /// `Security.setIgnoreCertificateErrors`: HTTPS requests skip certificate
    /// verification (handshake signatures are still checked).
    ignore_certificate_errors: std::sync::atomic::AtomicBool,
    proxy: Option<Arc<ProxyState>>,
    /// The configured `User-Agent` and `Accept-Language` as header values,
    /// built once (a request without overrides sends them as they are).
    default_user_agent: Option<HeaderValue>,
    default_accept_language: Option<HeaderValue>,
}

impl Shared {
    fn now(&self) -> f64 {
        self.epoch.elapsed().as_secs_f64()
    }
    fn emit(&self, ev: NetEvent) {
        self.observers.emit(ev);
    }
    fn observed(&self) -> bool {
        self.observers.observed()
    }

    /// A whole response at once: its events to the observers and to `sink`.
    fn deliver_body(
        &self,
        id: RequestId,
        req: &Request,
        head: ResponseHead,
        body: Bytes,
        sink: &mut dyn FnMut(ResponseEvent),
    ) -> Result<(), NetError> {
        if body.len() > req.max_body.unwrap_or(self.config.max_body) {
            return Err(NetError::TooLarge);
        }
        self.emit(NetEvent::ResponseReceived {
            id,
            head: Box::new(head.clone()),
            destination: req.destination,
            frame_id: req.frame_id.clone(),
            loader_id: req.loader_id.clone(),
            timestamp: self.now(),
        });
        sink(ResponseEvent::Response(head));
        let len = body.len() as u64;
        if !body.is_empty() {
            if self.observed() {
                self.emit(NetEvent::DataReceived { id, data: body.clone(), timestamp: self.now() });
            }
            sink(ResponseEvent::Data(body));
        }
        self.emit(NetEvent::LoadingFinished { id, encoded_len: len, timestamp: self.now() });
        sink(ResponseEvent::Finished { encoded_len: len, decoded_len: len });
        Ok(())
    }

    /// An interceptor's `Fulfill`: the response it gave, served as it is.
    fn fulfill(
        &self,
        id: RequestId,
        req: &Request,
        status: u16,
        headers: HeaderMap,
        body: Bytes,
        sink: &mut dyn FnMut(ResponseEvent),
    ) -> Result<(), NetError> {
        let status = StatusCode::from_u16(status).map_err(|_| NetError::Other("invalid status".into()))?;
        crate::timing::served_locally();
        let head = response_head(&req.url, status, headers, ResponseKind::Basic, false);
        self.deliver_body(id, req, head, body, sink)
    }

    /// The request headers of one hop; `site` is its `Sec-Fetch-Site`,
    /// `same_origin` whether the hop is same-origin to the requester.
    fn build_headers(
        &self,
        cookies: &Mutex<CookieJar>,
        req: &Request,
        referrer: Option<&Url>,
        origin_url: Option<&Url>,
        cors_cross: bool,
        site: FetchSite,
        // pyro: the hop is same-origin and the response's tainting is "basic" (no hop of a no-cors
        // request went to another origin): Fetch includes credentials for "same-origin" then only.
        same_origin: bool,
    ) -> HeaderMap {
        // Read under the lock: the settings are a header map and strings a
        // request would otherwise copy to read them once.
        let settings = self.settings.lock().unwrap();
        let mut h = req.headers.clone();
        let ua = match &settings.user_agent {
            Some(ua) => HeaderValue::from_str(ua).ok(),
            None => self.default_user_agent.clone(),
        };
        if let Some(v) = req.defaults.user_agent.clone().or(ua) {
            h.entry(header::USER_AGENT).or_insert(v);
        }
        h.entry(header::ACCEPT).or_insert(HeaderValue::from_static(default_accept(req.destination)));
        let lang = match &settings.accept_language {
            Some(lang) => HeaderValue::from_str(lang).ok(),
            None => self.default_accept_language.clone(),
        };
        if let Some(v) = req.defaults.accept_language.clone().or(lang) {
            h.entry(header::ACCEPT_LANGUAGE).or_insert(v);
        }
        if let Some(hints) = &req.defaults.client_hints {
            if crate::cookies::is_trustworthy(&req.url) {
                for (name, value) in hints.iter() {
                    h.entry(name.clone()).or_insert_with(|| value.clone());
                }
            }
        }
        h.insert(header::ACCEPT_ENCODING, HeaderValue::from_static("gzip, deflate, br"));
        if let Some(origin) = &req.origin {
            if cors_cross || !matches!(req.method, Method::GET | Method::HEAD) {
                if let Ok(v) = HeaderValue::from_str(origin) {
                    h.insert(header::ORIGIN, v);
                }
            }
        }
        if let Some(r) = referrer {
            // A serialized URL is ASCII: the value takes the string as it is.
            if let Some(v) = crate::referrer::referrer_for(r, &req.url, req.referrer_policy)
                .and_then(|v| HeaderValue::from_maybe_shared(Bytes::from(v)).ok())
            {
                h.entry(header::REFERER).or_insert(v);
            }
        }
        // Fetch Metadata, to potentially trustworthy URLs as in Chrome.
        if crate::cookies::is_trustworthy(&req.url) {
            h.insert(SEC_FETCH_SITE, HeaderValue::from_static(site.as_str()));
            h.insert(SEC_FETCH_MODE, HeaderValue::from_static(crate::referrer::fetch_mode(req.mode, req.destination)));
            h.insert(SEC_FETCH_DEST, HeaderValue::from_static(crate::referrer::fetch_dest(req.destination)));
            if site == FetchSite::None && req.destination == Destination::Document {
                h.insert(SEC_FETCH_USER, HeaderValue::from_static("?1"));
            }
        }
        // Cookies.
        // (`same_origin` is the hop's response tainting being "basic": `includes_credentials`.)
        let send_cookies = includes_credentials(req.credentials, same_origin);
        if send_cookies && !h.contains_key(header::COOKIE) {
            let top = matches!(req.destination, Destination::Document);
            let safe = matches!(req.method, Method::GET | Method::HEAD);
            let ctx = if req.mode == RequestMode::Navigate && req.origin.is_none() {
                SiteContext::SameSite
            } else if req.origin.is_some() && origin_url.is_none() {
                if top && safe {
                    SiteContext::CrossSiteTopLevelSafe
                } else {
                    SiteContext::CrossSite
                }
            } else {
                site_context(origin_url, &req.url, top, safe)
            };
            let cookie = cookies.lock().unwrap().header_for(&req.url, ctx, crate::cookies::now_secs());
            if let Some(c) = cookie.and_then(|c| HeaderValue::from_str(&c).ok()) {
                h.insert(header::COOKIE, c);
            }
        }
        for (k, v) in settings.extra_headers.iter() {
            h.insert(k.clone(), v.clone());
        }
        for (k, v) in req.defaults.extra_headers.iter() {
            h.insert(k.clone(), v.clone());
        }
        h
    }

    /// Effective cache mode, applying the Fetch "HTTP-network-or-cache fetch"
    /// request header rules and Network.setCacheDisabled.
    fn cache_mode(&self, req: &Request, headers: &mut HeaderMap) -> CacheMode {
        let disabled = req.defaults.cache_disabled || self.settings.lock().unwrap().cache_disabled;
        let mut mode = if disabled { CacheMode::NoStore } else { req.cache };
        let conditional = [
            header::IF_MODIFIED_SINCE,
            header::IF_NONE_MATCH,
            header::IF_UNMODIFIED_SINCE,
            header::IF_MATCH,
            header::IF_RANGE,
        ];
        if mode == CacheMode::Default && conditional.iter().any(|h| headers.contains_key(h)) {
            mode = CacheMode::NoStore; // the page's own conditional request passes through untouched
        }
        if mode == CacheMode::NoCache && !headers.contains_key(header::CACHE_CONTROL) {
            headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("max-age=0"));
        }
        if matches!(mode, CacheMode::NoStore | CacheMode::Reload) && (disabled || req.cache != CacheMode::Default) {
            headers.entry(header::PRAGMA).or_insert(HeaderValue::from_static("no-cache"));
            headers.entry(header::CACHE_CONTROL).or_insert(HeaderValue::from_static("no-cache"));
        }
        mode
    }
}

/// A response's headers with the values in one allocation of their own.
/// hyper's HTTP/1 parser hands out values that are slices of the connection's
/// read buffer, which grows to 400 KB on a busy connection: a head that is
/// kept (by Cache Storage, by a `Response` a script holds, by the HTTP cache,
/// by the protocol's body store) would keep the buffer it arrived in.
fn own_headers(received: &HeaderMap) -> HeaderMap {
    let mut all = Vec::with_capacity(received.values().map(HeaderValue::len).sum());
    for value in received.values() {
        all.extend_from_slice(value.as_bytes());
    }
    let all = Bytes::from(all);
    let mut headers = received.clone();
    let mut at = 0;
    for value in headers.values_mut() {
        let own = all.slice(at..at + value.len());
        at += value.len();
        // The same map in the same order, so the same bytes.
        if own != value.as_bytes() {
            continue;
        }
        // SAFETY: `own` holds the bytes of `value`, a valid header value.
        let mut own = unsafe { HeaderValue::from_maybe_shared_unchecked(own) };
        own.set_sensitive(value.is_sensitive());
        *value = own;
    }
    headers
}

/// A response head as the current request task times it ([`crate::timing`]).
fn response_head(
    url: &Url,
    status: StatusCode,
    headers: HeaderMap,
    kind: ResponseKind,
    redirected: bool,
) -> ResponseHead {
    let (mime, charset) =
        headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).map(parse_content_type).unwrap_or_default();
    ResponseHead {
        url: url.clone(),
        status,
        status_text: status.canonical_reason().unwrap_or("").to_owned(),
        headers,
        mime,
        charset,
        redirected,
        from_cache: false,
        kind,
        remote_addr: None,
        protocol: Version::HTTP_11,
        timing: crate::timing::current(),
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.abort_local();
        // Closing the channel stops the I/O loop, which fails in-flight
        // requests with `Aborted` before exiting.
        let (dead, _) = mpsc::unbounded_channel();
        *self.tx.get_mut().unwrap_or_else(|e| e.into_inner()) = dead;
        if let Some(t) = self.thread.lock().unwrap().take() {
            // The last handle may go on the I/O thread itself (a sink or an
            // upgraded `RequestCanceller` there owned it). A thread cannot
            // join itself: its loop ends once it is back at the closed channel.
            if t.thread().id() != std::thread::current().id() {
                let _ = t.join();
            }
        }
    }
}

/// Handle to one network context (cookie jar + connection pool + I/O
/// thread). Cheap to clone and `Send`; one per browser context.
#[derive(Clone)]
pub struct NetClient {
    inner: Arc<Inner>,
}

/// Cancels a client's requests without keeping the client alive. For what a
/// request's sink owns: the I/O thread holds a sink until its request ends,
/// so a [`NetClient`] in it would keep that thread, and every request of the
/// client, running after the last handle elsewhere is gone.
#[derive(Clone)]
pub struct RequestCanceller {
    inner: std::sync::Weak<Inner>,
}

impl RequestCanceller {
    /// [`NetClient::cancel`]. Nothing once the client is gone: its requests
    /// failed with `Aborted` then.
    pub fn cancel(&self, id: RequestId) {
        if let Some(inner) = self.inner.upgrade() {
            NetClient { inner }.cancel(id);
        }
    }
}

impl NetClient {
    pub fn new(config: NetConfig) -> std::io::Result<Self> {
        let (tx, rx) = mpsc::unbounded_channel();
        let cache = crate::cache::HttpCache::new(config.http_cache_max_bytes, config.http_cache_max_entry_bytes);
        let proxy = config.proxy.clone().map(|p| Arc::new(ProxyState::new(p)));
        let shared = Arc::new(Shared {
            default_user_agent: HeaderValue::from_str(&config.user_agent).ok(),
            default_accept_language: HeaderValue::from_str(&config.accept_language).ok(),
            suspending: std::sync::atomic::AtomicBool::new(false),
            config,
            settings: Mutex::new(NetSettings::default()),
            observers: Subscriptions::default(),
            interceptor: Mutex::new(None),
            cache: Mutex::new(cache),
            epoch: Instant::now(),
            ignore_certificate_errors: std::sync::atomic::AtomicBool::new(false),
            proxy,
        });
        let cookies = Arc::new(Mutex::new(CookieJar::new()));
        let (s2, c2) = (shared.clone(), cookies.clone());
        let thread = std::thread::Builder::new().name("pyro-net-io".into()).spawn(move || io_main(rx, s2, c2))?;
        let inner = Arc::new(Inner {
            tx: Mutex::new(tx),
            next_id: AtomicU64::new(1),
            cookies,
            shared,
            thread: Mutex::new(Some(thread)),
            activity: Arc::default(),
            local: Mutex::default(),
        });
        let mut clients = CLIENTS.lock().unwrap_or_else(|e| e.into_inner());
        clients.retain(|c| c.strong_count() > 0);
        clients.push(Arc::downgrade(&inner));
        Ok(NetClient { inner })
    }

    pub fn cookies(&self) -> &Arc<Mutex<CookieJar>> {
        &self.inner.cookies
    }
    pub fn config(&self) -> &NetConfig {
        &self.inner.shared.config
    }
    /// Current network settings snapshot.
    pub fn settings(&self) -> NetSettings {
        self.inner.shared.settings.lock().unwrap().clone()
    }
    /// Effective User-Agent (override or configured default).
    pub fn user_agent(&self) -> String {
        let s = self.inner.shared.settings.lock().unwrap();
        s.user_agent.clone().unwrap_or_else(|| self.inner.shared.config.user_agent.clone())
    }
    pub fn update_settings(&self, f: impl FnOnce(&mut NetSettings)) {
        f(&mut self.inner.shared.settings.lock().unwrap());
    }
    /// Accepts any server certificate on later HTTPS connections (CDP
    /// `Security.setIgnoreCertificateErrors`). Connections opened while it
    /// was off stay pooled for requests made while it is off.
    pub fn set_ignore_certificate_errors(&self, ignore: bool) {
        self.inner.shared.ignore_certificate_errors.store(ignore, Ordering::Relaxed);
    }
    /// Subscribes with bounded defaults (1024 events / 8 MiB logical bytes).
    pub fn subscribe(&self) -> NetSubscription {
        self.subscribe_with(SubscriptionLimits::default(), None)
    }
    /// Wake runs on the publishing thread after all observer locks are released,
    /// on empty-to-nonempty and terminal transitions. It should post/coalesce a
    /// browser-loop task. Overflow is explicit and terminates the subscription.
    pub fn subscribe_with(&self, limits: SubscriptionLimits, wake: Option<NetWake>) -> NetSubscription {
        self.inner.shared.observers.subscribe(limits, wake)
    }
    pub fn set_interceptor(&self, i: Option<Arc<dyn Interceptor>>) {
        *self.inner.shared.interceptor.lock().unwrap() = i;
    }
    /// Network.clearBrowserCache: drops every HTTP cache entry of this context.
    pub fn clear_http_cache(&self) {
        self.inner.shared.cache.lock().unwrap().clear();
    }
    /// HTTP cache (entries, logical bytes).
    pub fn http_cache_usage(&self) -> (usize, usize) {
        self.inner.shared.cache.lock().unwrap().usage()
    }
    /// Clears the opt-in shared asset cache this client uses (all contexts
    /// sharing it are affected). No-op when sharing is off.
    pub fn clear_shared_asset_cache(&self) {
        if let Some(s) = &self.inner.shared.config.shared_asset_cache {
            s.clear();
        }
    }

    pub fn next_request_id(&self) -> RequestId {
        RequestId(self.inner.next_id.fetch_add(1, Ordering::Relaxed))
    }

    /// Grows with every request this client starts (WebSocket handshakes
    /// included): two equal readings mean none started between them.
    pub fn requests_started(&self) -> u64 {
        self.inner.next_id.load(Ordering::Relaxed)
    }

    /// Starts a request. `data:` URLs are answered on the I/O thread
    /// without network access so event ordering stays uniform.
    pub fn fetch(&self, req: Request, sink: ResponseSink) -> RequestId {
        let id = self.next_request_id();
        self.fetch_with_id(id, req, sink);
        id
    }

    pub fn fetch_with_id(&self, id: RequestId, req: Request, sink: ResponseSink) {
        let sink = match req.loader_id.clone() {
            Some(loader) => self.track(id, loader, sink),
            None => sink,
        };
        let start = Instant::now();
        let req = Box::new(req);
        match self.inner.local_interceptor(&req) {
            Some(interceptor) => LocalPause::start(&self.inner, interceptor, id, req, sink, start),
            None => self.inner.send_fetch(id, req, sink, start, None),
        }
    }

    /// Counts a started request for `loader` (synchronously, on the caller's
    /// thread) and ties the matching decrement to the sink's lifetime.
    fn track(&self, id: RequestId, loader: String, mut sink: ResponseSink) -> ResponseSink {
        let (generation, order) = {
            let mut map = self.inner.activity.lock().unwrap();
            let next = map.generation + 1;
            map.started += 1;
            let order = map.started;
            let l = map.loaders.entry(loader.clone()).or_insert_with(|| LoaderActivity {
                generation: next,
                counts: NetActivity { inflight: 0, event_streams: 0, idle_since: None, almost_idle_since: None },
                inflight: Vec::new(),
            });
            let generation = l.generation;
            l.inflight.push(InFlight { id, order, stream: false });
            let a = &mut l.counts;
            a.inflight += 1;
            a.idle_since = None;
            if a.inflight > 2 {
                a.almost_idle_since = None;
            }
            if generation == next {
                map.generation = next;
            }
            (generation, order)
        };
        let mut guard =
            ActivityGuard { event_stream: false, map: self.inner.activity.clone(), loader, generation, order };
        Box::new(move |ev| {
            if !guard.event_stream
                && matches!(&ev, ResponseEvent::Response(head) if head.status.is_success() && head.mime == "text/event-stream")
            {
                let mut map = guard.map.lock().unwrap();
                if let Some(l) = map.loaders.get_mut(&guard.loader) {
                    if l.generation == guard.generation {
                        l.counts.event_streams += 1;
                        if let Some(entry) = l.inflight.iter_mut().find(|r| r.order == guard.order) {
                            entry.stream = true;
                        }
                        guard.event_stream = true;
                    }
                }
            }
            sink(ev)
        })
    }

    /// Current activity of a loader, `None` if it never started a request on
    /// this client (or was forgotten since).
    pub fn activity(&self, loader_id: &str) -> Option<NetActivity> {
        self.inner.activity.lock().unwrap().loaders.get(loader_id).map(|l| l.counts)
    }

    /// A point in this client's request history: the requests it starts for
    /// a loader from now on are the ones [`Self::inflight_after`] counts for
    /// this reading. 0 comes before every request.
    pub fn request_mark(&self) -> u64 {
        self.inner.activity.lock().unwrap().started
    }

    /// A loader's finite requests (established event streams left out, as in
    /// [`NetActivity`]) that were started after `mark` and are still in flight.
    pub fn inflight_after(&self, loader_id: &str, mark: u64) -> usize {
        let map = self.inner.activity.lock().unwrap();
        map.loaders.get(loader_id).map_or(0, |l| l.inflight.iter().filter(|r| r.order > mark && !r.stream).count())
    }

    /// Whether request `id` is still in flight. It stops being so right after
    /// its terminal event (finished, failed, canceled) was handed to its
    /// sink, so a consumer that reads `false` has been given every event of
    /// the request. Also false for a request without a loader, which is not
    /// tracked, and for one whose loader was forgotten.
    pub fn in_flight(&self, id: RequestId) -> bool {
        let map = self.inner.activity.lock().unwrap();
        map.loaders.values().any(|l| l.inflight.iter().any(|r| r.id == id))
    }

    /// Drops a loader's activity entry (e.g. on navigation or page close).
    /// Requests still in flight for it no longer count anywhere.
    pub fn forget_activity(&self, loader_id: &str) {
        self.inner.activity.lock().unwrap().loaders.remove(loader_id);
    }

    /// Aborts a request; its sink receives `Failed(Aborted)` unless it
    /// already finished.
    pub fn cancel(&self, id: RequestId) {
        let local = self.inner.local.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
        if local.is_some_and(|pause| pause.abort(&self.inner.shared)) {
            return;
        }
        let _ = self.inner.tx.lock().unwrap_or_else(|e| e.into_inner()).send(Cmd::Cancel(id));
    }

    /// [`NetClient::cancel`] for a request's own sink (see [`RequestCanceller`]).
    pub fn canceller(&self) -> RequestCanceller {
        RequestCanceller { inner: Arc::downgrade(&self.inner) }
    }

    /// Blocking fetch for synchronous XHR. Must not be used while the
    /// caller's thread is needed to answer interception decisions.
    pub fn fetch_blocking(&self, req: Request) -> Result<(ResponseHead, Bytes), NetError> {
        let (tx, rx) = std::sync::mpsc::channel();
        self.fetch(
            req,
            Box::new(move |ev| {
                let _ = tx.send(ev);
            }),
        );
        let mut head = None;
        let mut body = bytes::BytesMut::new();
        loop {
            match rx.recv() {
                Ok(ResponseEvent::Response(h)) => head = Some(h),
                Ok(ResponseEvent::Data(d)) => body.extend_from_slice(&d),
                Ok(ResponseEvent::Redirect { .. }) => {}
                Ok(ResponseEvent::Finished { .. }) => {
                    return head.map(|h| (h, body.freeze())).ok_or(NetError::Other("no response".into()))
                }
                Ok(ResponseEvent::Failed(e)) => return Err(e),
                Err(_) => return Err(NetError::Other("network thread stopped".into())),
            }
        }
    }
}

/// The first hop of a request as its caller's thread left it: the headers
/// built and published (`RequestWillBeSent`), and the interceptor's
/// request-stage decision (`None`: it dropped its reply).
struct Staged {
    headers: HeaderMap,
    decision: Option<InterceptDecision>,
}

impl Inner {
    /// The interceptor to pause `req` with on the calling thread: one that
    /// allows it is set and the request's first hop reaches its request stage (an
    /// `http(s)` URL the checks before it let through). Any other request
    /// starts on the I/O thread.
    fn local_interceptor(&self, req: &Request) -> Option<Arc<dyn Interceptor>> {
        if req.skip_interception || !matches!(req.url.scheme(), "http" | "https") {
            return None;
        }
        let interceptor = self.shared.interceptor.lock().unwrap().clone()?;
        if !interceptor.pauses_on_caller() || self.shared.settings.lock().unwrap().offline {
            return None;
        }
        Some(interceptor)
    }

    fn send_fetch(&self, id: RequestId, req: Box<Request>, sink: ResponseSink, start: Instant, staged: Option<Staged>) {
        let sent = self.tx.lock().unwrap_or_else(|e| e.into_inner()).send(Cmd::Fetch(id, req, sink, start, staged));
        if let Err(mpsc::error::SendError(Cmd::Fetch(_, _, mut sink, _, _))) = sent {
            sink(ResponseEvent::Failed(NetError::Other("network thread stopped".into())));
        }
    }

    /// Fails every request paused off the I/O thread with `Aborted`.
    fn abort_local(&self) {
        let paused: Vec<Arc<LocalPause>> =
            self.local.lock().unwrap_or_else(|e| e.into_inner()).drain().map(|(_, pause)| pause).collect();
        for pause in paused {
            pause.abort(&self.shared);
        }
    }
}

/// A request paused at the request stage of its first hop without the I/O
/// thread: the thread that started it built its headers, published its
/// start and called [`Interceptor::paused`]; the thread that answers runs
/// what the answer asks for. A response the interceptor gives (`Fulfill`)
/// or a failure is delivered there and then; a request that goes on to the
/// network is handed to the I/O thread with its decision ([`Staged`]). An
/// embedder that pauses every request to match it against its patterns
/// would otherwise cross to the I/O thread and back for each one.
///
/// It is its reply's waker: the reply is polled where it is sent or dropped.
struct LocalPause {
    id: RequestId,
    client: std::sync::Weak<Inner>,
    /// Taken by whoever ends the pause: the decision, a cancel, or shutdown.
    state: Mutex<Option<LocalState>>,
}

struct LocalState {
    req: Box<Request>,
    sink: ResponseSink,
    headers: HeaderMap,
    start: Instant,
    reply: oneshot::Receiver<InterceptDecision>,
}

impl LocalPause {
    fn start(
        inner: &Arc<Inner>,
        interceptor: Arc<dyn Interceptor>,
        id: RequestId,
        req: Box<Request>,
        sink: ResponseSink,
        start: Instant,
    ) {
        let shared = &inner.shared;
        let origin_url = req.origin.as_deref().and_then(|o| Url::parse(o).ok());
        let same_origin = req.origin.is_none() || origin_url.as_ref().is_some_and(|o| cors::same_origin(o, &req.url));
        if req.mode == RequestMode::SameOrigin && !same_origin {
            // Refused before its request stage: the I/O thread reports it.
            return inner.send_fetch(id, req, sink, start, None);
        }
        let cors_cross = shared.config.enforce_cors && req.mode == RequestMode::Cors && !same_origin;
        let site = hop_site(&req, origin_url.as_ref());
        let mut headers = shared.build_headers(
            &inner.cookies,
            &req,
            req.referrer.as_ref(),
            origin_url.as_ref(),
            cors_cross,
            site,
            same_origin,
        );
        shared.cache_mode(&req, &mut headers);
        let info = || RequestInfo {
            url: req.url.clone(),
            method: req.method.clone(),
            headers: headers.clone(),
            post_data: req.body.clone(),
            destination: req.destination,
            frame_id: req.frame_id.clone(),
            loader_id: req.loader_id.clone(),
        };
        shared.emit(NetEvent::RequestWillBeSent {
            id,
            request: Box::new(info()),
            redirect_response: None,
            timestamp: shared.now(),
            wall_time: wall_time(),
        });
        let request = info();
        let (tx, reply) = oneshot::channel();
        let pause = Arc::new(LocalPause {
            id,
            client: Arc::downgrade(inner),
            state: Mutex::new(Some(LocalState { req, sink, headers, start, reply })),
        });
        inner.local.lock().unwrap_or_else(|e| e.into_inner()).insert(id, pause.clone());
        interceptor.paused(PausedRequest { id, request }, tx);
        // A decision made inside `paused` goes to the I/O thread whatever it
        // is: a sink is never called from inside `fetch`.
        pause.poll(true);
    }

    /// Polls the reply and, once it is there, acts on it. `starting`: the
    /// call is the one inside `fetch`.
    fn poll(self: &Arc<Self>, starting: bool) {
        // Without the client the request is already aborted, or about to be.
        let Some(inner) = self.client.upgrade() else { return };
        let mut slot = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let Some(mut state) = slot.take() else { return };
        let waker = std::task::Waker::from(self.clone());
        let mut cx = std::task::Context::from_waker(&waker);
        let decision = match std::future::Future::poll(std::pin::Pin::new(&mut state.reply), &mut cx) {
            std::task::Poll::Pending => {
                *slot = Some(state);
                return;
            }
            std::task::Poll::Ready(decision) => decision.ok(),
        };
        let LocalState { req, mut sink, headers, start, .. } = state;
        let on_io_thread =
            starting || matches!(decision, Some(InterceptDecision::Continue { .. } | InterceptDecision::PauseResponse));
        if on_io_thread {
            // Sent under the lock: a cancel that finds the pause over sends
            // its command after this one.
            inner.send_fetch(self.id, req, sink, start, Some(Staged { headers, decision }));
            drop(slot);
            inner.local.lock().unwrap_or_else(|e| e.into_inner()).remove(&self.id);
            return;
        }
        // The lock is released first: the sink may cancel requests.
        drop(slot);
        inner.local.lock().unwrap_or_else(|e| e.into_inner()).remove(&self.id);
        let shared = &inner.shared;
        let result = match decision {
            Some(InterceptDecision::Fulfill { status, headers, body }) => {
                let timing =
                    crate::timing::NetTiming { start: Some(start), fetch_start: Some(start), ..Default::default() };
                crate::timing::FETCH_TIMING.sync_scope(std::cell::Cell::new(timing), || {
                    shared.fulfill(self.id, &req, status, headers, body, &mut sink)
                })
            }
            Some(InterceptDecision::Fail(reason)) => Err(NetError::Blocked(cdp_reason(&reason))),
            _ => Err(NetError::Other("interception dropped".into())),
        };
        if let Err(e) = result {
            fail_local(shared, self.id, &req, &mut sink, e);
        }
    }

    /// Ends the pause with `Aborted`. False when it was over already (the
    /// I/O thread has the request, or it is finished).
    fn abort(&self, shared: &Shared) -> bool {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner()).take();
        let Some(LocalState { req, mut sink, reply, .. }) = state else { return false };
        // A terminal callback must not observe a live interception reply.
        drop(reply);
        fail_local(shared, self.id, &req, &mut sink, NetError::Aborted);
        true
    }
}

impl std::task::Wake for LocalPause {
    fn wake(self: Arc<Self>) {
        self.poll(false);
    }
}

/// The terminal events of a request that failed off the I/O thread.
fn fail_local(shared: &Shared, id: RequestId, req: &Request, sink: &mut ResponseSink, error: NetError) {
    shared.emit(NetEvent::LoadingFailed {
        id,
        canceled: error == NetError::Aborted,
        error: error.clone(),
        destination: req.destination,
        timestamp: shared.now(),
    });
    sink(ResponseEvent::Failed(error));
}

// ---------------------------------------------------------------------------
// I/O thread

type HttpClient = Client<ProbeConnector<hyper_rustls::HttpsConnector<ProxyConnector>>, Full<Bytes>>;

/// The TLS settings of every client: Mozilla's root certificates (webpki-roots),
/// rustls on ring. Plain data, built once per process (a fork child keeps it;
/// connections live on the I/O thread, which forks stop and restart).
fn tls_config() -> Arc<rustls::ClientConfig> {
    static CONFIG: std::sync::OnceLock<Arc<rustls::ClientConfig>> = std::sync::OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let roots = rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
            let provider = Arc::new(rustls::crypto::ring::default_provider());
            let config = rustls::ClientConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .expect("rustls protocol versions")
                .with_root_certificates(roots)
                .with_no_client_auth();
            Arc::new(config)
        })
        .clone()
}

/// [`tls_config`] without certificate verification, for
/// [`NetClient::set_ignore_certificate_errors`].
fn tls_config_insecure() -> Arc<rustls::ClientConfig> {
    static CONFIG: std::sync::OnceLock<Arc<rustls::ClientConfig>> = std::sync::OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let provider = Arc::new(rustls::crypto::ring::default_provider());
            let verifier = Arc::new(AnyCertificate(provider.clone()));
            let config = rustls::ClientConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .expect("rustls protocol versions")
                .dangerous()
                .with_custom_certificate_verifier(verifier)
                .with_no_client_auth();
            Arc::new(config)
        })
        .clone()
}

/// Accepts every server certificate, and the handshake signature made with it.
///
/// The signature is not checked either: rustls checks it through webpki, which refuses a certificate it cannot
/// parse (X.509 version 1, as LibreSSL's `openssl req -x509` writes), and with any certificate accepted a
/// valid signature says nothing about who the server is.
#[derive(Debug)]
struct AnyCertificate(Arc<rustls::crypto::CryptoProvider>);

impl rustls::client::danger::ServerCertVerifier for AnyCertificate {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// An HTTP(S) client over `tls`, with this context's connect settings. Its
/// HTTP/2 connections are registered in `sessions` (WebSockets over HTTP/2).
fn http_client(
    config: &NetConfig,
    tls: &rustls::ClientConfig,
    proxy: Option<Arc<ProxyState>>,
    sessions: Arc<H2Sessions>,
) -> HttpClient {
    let mut connector = HttpConnector::new_with_resolver(crate::timing::TimedResolver::new());
    connector.set_connect_timeout(Some(config.connect_timeout));
    connector.set_nodelay(true);
    connector.enforce_http(false);
    let connector = ProxyConnector::new(connector, proxy, config.connect_timeout);
    let connector = hyper_rustls::HttpsConnectorBuilder::new()
        .with_tls_config(tls.clone())
        .https_or_http()
        .enable_http1()
        .enable_http2()
        .wrap_connector(connector);
    let connector = ProbeConnector::new(connector, sessions);
    Client::builder(TokioExecutor::new()).pool_idle_timeout(Duration::from_secs(30)).build(connector)
}

/// Every client of the process, for [`suspend_for_fork`].
static CLIENTS: Mutex<Vec<std::sync::Weak<Inner>>> = Mutex::new(Vec::new());

/// Run on an I/O thread once it has been quiet for [`IO_IDLE_RELEASE`]
/// ([`set_idle_release`]).
static IDLE_RELEASE: std::sync::OnceLock<fn()> = std::sync::OnceLock::new();

/// How long an I/O thread stays quiet (no command, no request finishing)
/// before it runs the [`set_idle_release`] hook.
const IO_IDLE_RELEASE: Duration = Duration::from_secs(1);

/// Registers what an I/O thread runs once a second has passed since its
/// last command or finished request: an allocator with per-thread heaps
/// (the executable's) returns the pages of the response buffers the browser
/// freed, which only the thread that allocated them can (a loaded mid-size
/// SPA: 3 MB less).
pub fn set_idle_release(release: fn()) {
    let _ = IDLE_RELEASE.set(release);
}

/// Runs the [`set_idle_release`] hook once per quiet spell of an I/O
/// thread, [`IO_IDLE_RELEASE`] after its last activity, whether or not a
/// request is still open (a long poll or a stream may stay open for good;
/// the hook only returns what is already free). No timer runs while the
/// thread stays quiet after that.
struct IdleRelease {
    /// The thread's last command or finished request.
    last: std::cell::Cell<tokio::time::Instant>,
    /// Whether a release is due (activity since the last one).
    due: std::cell::Cell<bool>,
    wake: tokio::sync::Notify,
}

impl IdleRelease {
    /// Starts the releasing task on this thread's `LocalSet`.
    fn start(release: fn()) -> std::rc::Rc<IdleRelease> {
        let idle = std::rc::Rc::new(IdleRelease {
            last: std::cell::Cell::new(tokio::time::Instant::now()),
            due: std::cell::Cell::new(false),
            wake: tokio::sync::Notify::new(),
        });
        let task = idle.clone();
        tokio::task::spawn_local(async move {
            loop {
                task.wake.notified().await;
                loop {
                    let at = task.last.get() + IO_IDLE_RELEASE;
                    if at <= tokio::time::Instant::now() {
                        break;
                    }
                    tokio::time::sleep_until(at).await;
                }
                task.due.set(false);
                release();
            }
        });
        idle
    }

    /// A command came or a request finished.
    fn touch(&self) {
        self.last.set(tokio::time::Instant::now());
        if !self.due.replace(true) {
            self.wake.notify_one();
        }
    }
}

/// Stops the I/O thread of every client in the process, so that the process
/// can `fork()` (only the calling thread survives a fork). Requests still in
/// flight fail with `Aborted`; cookies, cache and observers are kept. Call
/// [`resume_after_fork`] afterwards, in the parent and in the child.
pub fn suspend_for_fork() {
    let clients: Vec<Arc<Inner>> =
        CLIENTS.lock().unwrap_or_else(|e| e.into_inner()).iter().filter_map(|c| c.upgrade()).collect();
    for inner in clients {
        inner.shared.suspending.store(true, Ordering::Relaxed);
        inner.abort_local();
        let (dead, _) = mpsc::unbounded_channel();
        *inner.tx.lock().unwrap_or_else(|e| e.into_inner()) = dead;
        if let Some(t) = inner.thread.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = t.join();
        }
    }
}

/// Restarts the I/O threads [`suspend_for_fork`] stopped.
pub fn resume_after_fork() -> std::io::Result<()> {
    let clients: Vec<Arc<Inner>> =
        CLIENTS.lock().unwrap_or_else(|e| e.into_inner()).iter().filter_map(|c| c.upgrade()).collect();
    for inner in clients {
        let mut thread = inner.thread.lock().unwrap_or_else(|e| e.into_inner());
        if thread.is_some() {
            continue;
        }
        let (tx, rx) = mpsc::unbounded_channel();
        let (s2, c2) = (inner.shared.clone(), inner.cookies.clone());
        *thread = Some(std::thread::Builder::new().name("pyro-net-io".into()).spawn(move || io_main(rx, s2, c2))?);
        *inner.tx.lock().unwrap_or_else(|e| e.into_inner()) = tx;
        inner.shared.suspending.store(false, Ordering::Relaxed);
    }
    Ok(())
}

struct Io {
    shared: Arc<Shared>,
    cookies: Arc<Mutex<CookieJar>>,
    client: HttpClient,
    /// Built at the first request made while certificate errors are ignored.
    insecure: std::cell::OnceCell<HttpClient>,
    /// The HTTP/2 connections of `client` and of `insecure`.
    sessions: Arc<H2Sessions>,
    insecure_sessions: Arc<H2Sessions>,
    preflight: Mutex<PreflightCache>,
    per_origin: Mutex<HashMap<url::Origin, Arc<Semaphore>>>,
    /// Origins whose semaphore grew to `max_per_h2_origin`.
    h2_origins: Mutex<HashSet<url::Origin>>,
}

fn io_main(mut rx: mpsc::UnboundedReceiver<Cmd>, shared: Arc<Shared>, cookies: Arc<Mutex<CookieJar>>) {
    struct CloseObservers(Arc<Shared>);
    impl Drop for CloseObservers {
        fn drop(&mut self) {
            if !self.0.suspending.load(Ordering::Relaxed) {
                self.0.observers.close();
            }
        }
    }
    let _close_observers = CloseObservers(shared.clone());
    // Observer wakes of this thread go out once per turn: when the runtime
    // is about to park (nothing left to run until I/O arrives) and before a
    // request's terminal event reaches its sink (`deliver`). Dropped before
    // the observers close.
    let _deferred_wakes = DeferredWakes::enter();
    let rt =
        match tokio::runtime::Builder::new_current_thread().enable_all().on_thread_park(flush_deferred_wakes).build() {
            Ok(rt) => rt,
            Err(_) => return,
        };
    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, async move {
        let sessions = Arc::new(H2Sessions::default());
        let client = http_client(&shared.config, &tls_config(), shared.proxy.clone(), sessions.clone());
        let io = std::rc::Rc::new(Io {
            shared,
            cookies,
            client,
            insecure: std::cell::OnceCell::new(),
            sessions,
            insecure_sessions: Arc::new(H2Sessions::default()),
            preflight: Mutex::new(PreflightCache::default()),
            per_origin: Mutex::new(HashMap::new()),
            h2_origins: Mutex::new(HashSet::new()),
        });
        let tasks: std::rc::Rc<std::cell::RefCell<HashMap<RequestId, Task>>> = Default::default();
        let priorities = ParserPriorities::default();
        let idle = IDLE_RELEASE.get().map(|release| IdleRelease::start(*release));
        while let Some(cmd) = rx.recv().await {
            if let Some(idle) = &idle {
                idle.touch();
            }
            match cmd {
                Cmd::Fetch(id, req, mut sink, start, staged) => {
                    let destination = req.destination;
                    let mut head_start = None;
                    if let Some(document) = req.loader_id.as_ref().filter(|_| matches!(req.url.scheme(), "http" | "https")) {
                        let origin = req.url.origin();
                        if req.parser_priority {
                            let mut signal = Some(priorities.start((document.clone(), origin)));
                            let mut downstream = sink;
                            sink = Box::new(move |event| {
                                if matches!(event, ResponseEvent::Response(_) | ResponseEvent::Finished { .. } | ResponseEvent::Failed(_)) {
                                    signal.take();
                                }
                                downstream(event);
                            });
                        } else if destination == Destination::Image {
                            head_start = priorities.pending(&(document.clone(), origin));
                        }
                    }
                    let sink = std::rc::Rc::new(std::cell::RefCell::new(Some(sink)));
                    let (io2, tasks2, sink2, idle2) = (io.clone(), tasks.clone(), sink.clone(), idle.clone());
                    let handle = tokio::task::spawn_local(async move {
                        if let Some(mut head) = head_start {
                            const MAX_HEAD_START: Duration = Duration::from_millis(100);
                            let _ = tokio::time::timeout_at((start + MAX_HEAD_START).into(), head.changed()).await;
                        }
                        let dest = req.destination;
                        let timing = crate::timing::NetTiming { start: Some(start), ..Default::default() };
                        let running = io2.run(id, *req, &sink2, staged);
                        let result = crate::timing::FETCH_TIMING.scope(std::cell::Cell::new(timing), running).await;
                        if let Err(e) = result {
                            io2.shared.emit(NetEvent::LoadingFailed {
                                id,
                                canceled: e == NetError::Aborted,
                                error: e.clone(),
                                destination: dest,
                                timestamp: io2.shared.now(),
                            });
                            deliver(&sink2, ResponseEvent::Failed(e));
                        }
                        sink2.borrow_mut().take();
                        tasks2.borrow_mut().remove(&id);
                        if let Some(idle) = idle2 {
                            idle.touch();
                        }
                    });
                    tasks.borrow_mut().insert(id, Task { handle, sink, destination });
                }
                Cmd::WebSocket(start) => {
                    tokio::task::spawn_local(io.clone().websocket(*start));
                }
                Cmd::Cancel(id) => {
                    // Release the map borrow before awaiting task destruction.
                    // A terminal callback must not observe a live auth/interception reply.
                    let task = tasks.borrow_mut().remove(&id);
                    if let Some(t) = task {
                        t.handle.abort();
                        let _ = t.handle.await;
                        io.shared.emit(NetEvent::LoadingFailed {
                            id,
                            error: NetError::Aborted,
                            canceled: true,
                            destination: t.destination,
                            timestamp: io.shared.now(),
                        });
                        deliver(&t.sink, ResponseEvent::Failed(NetError::Aborted));
                        t.sink.borrow_mut().take();
                    }
                }
            }
        }
        // Shutdown: every in-flight request still gets its terminal event.
        let pending: Vec<(RequestId, Task)> = tasks.borrow_mut().drain().collect();
        // Abort all tasks before yielding, so another pending request cannot
        // complete while shutdown waits for the first task to release its state.
        for (_, t) in &pending {
            t.handle.abort();
        }
        for (id, t) in pending {
            let _ = t.handle.await;
            io.shared.emit(NetEvent::LoadingFailed {
                id,
                error: NetError::Aborted,
                canceled: true,
                destination: t.destination,
                timestamp: io.shared.now(),
            });
            deliver(&t.sink, ResponseEvent::Failed(NetError::Aborted));
            t.sink.borrow_mut().take();
        }
        flush_deferred_wakes();
    });
}

type PriorityKey = (String, url::Origin);

#[derive(Default, Clone)]
struct ParserPriorities(Arc<Mutex<HashMap<PriorityKey, (usize, tokio::sync::watch::Sender<()>)>>>);

impl ParserPriorities {
    fn start(&self, key: PriorityKey) -> PendingHeaders {
        let mut table = self.0.lock().unwrap();
        table.entry(key.clone()).or_insert_with(|| (0, tokio::sync::watch::channel(()).0)).0 += 1;
        PendingHeaders(self.clone(), key)
    }

    fn pending(&self, key: &PriorityKey) -> Option<tokio::sync::watch::Receiver<()>> {
        self.0.lock().unwrap().get(key).map(|(_, signal)| signal.subscribe())
    }
}

struct PendingHeaders(ParserPriorities, PriorityKey);

#[cfg(test)]
mod priority_tests {
    use super::*;

    #[test]
    fn image_dispatch_tracks_only_its_documents_origin() {
        let priorities = ParserPriorities::default();
        let key = ("document".to_owned(), Url::parse("https://site.test/").unwrap().origin());
        assert!(priorities.pending(&key).is_none());
        let stylesheet = priorities.start(key.clone());
        let script = priorities.start(key.clone());
        let images: Vec<_> = (0..32).map(|_| priorities.pending(&key).unwrap()).collect();
        assert!(priorities.pending(&("other".into(), key.1.clone())).is_none());
        assert!(priorities.pending(&(key.0.clone(), Url::parse("https://other.test/").unwrap().origin())).is_none());
        drop(stylesheet);
        assert_eq!(images.iter().filter(|head| head.has_changed().is_err()).count(), 0);
        drop(script);
        assert_eq!(images.iter().filter(|head| head.has_changed().is_err()).count(), 32);
        assert!(priorities.pending(&key).is_none());
    }
}

impl Drop for PendingHeaders {
    fn drop(&mut self) {
        let mut table = self.0.0.lock().unwrap();
        let entry = table.get_mut(&self.1).unwrap();
        entry.0 -= 1;
        if entry.0 == 0 { table.remove(&self.1); }
    }
}

struct Task {
    destination: Destination,
    handle: tokio::task::JoinHandle<()>,
    sink: SharedSink,
}

type SharedSink = std::rc::Rc<std::cell::RefCell<Option<ResponseSink>>>;

fn deliver(sink: &SharedSink, ev: ResponseEvent) {
    // A terminal event completes the request on the consumer's loop: the
    // observers' wake (of this and the events before it, deferred since the
    // thread last parked) is posted first, so an owner task publishes them
    // ahead of what the completion causes, as when every event woke at once;
    // the completion's post then finds that loop awake (one wake-up for both).
    if matches!(ev, ResponseEvent::Finished { .. } | ResponseEvent::Failed(_)) {
        flush_deferred_wakes();
    }
    if let Some(s) = sink.borrow_mut().as_mut() {
        s(ev);
    }
}

fn wall_time() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

fn default_accept(d: Destination) -> &'static str {
    match d {
        Destination::Document | Destination::Iframe => {
            "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8"
        }
        Destination::Style => "text/css,*/*;q=0.1",
        Destination::Json => "application/json,*/*;q=0.5",
        Destination::Image => "image/avif,image/webp,image/apng,image/svg+xml,image/*,*/*;q=0.8",
        Destination::EventSource => "text/event-stream",
        _ => "*/*",
    }
}

/// One content coding of a response body.
enum Stage {
    /// `gzip`; `deflate` as a zlib stream (RFC 9110), or sent without its
    /// zlib wrapper (legacy IIS and others), which Chromium also accepts.
    Inflate(Inflater),
    /// `deflate` until its first two bytes show a zlib header or not: the
    /// bytes so far, and an output that stays empty.
    Sniff(Vec<u8>, Vec<u8>),
    Brotli(Box<brotli_decompressor::DecompressorWriter<Vec<u8>>>),
}

/// A zlib stream header (RFC 1950): method 8 with a window of at most
/// 32 KiB, and a check value that makes the pair a multiple of 31.
fn is_zlib_header(cmf: u8, flg: u8) -> bool {
    cmf & 0x0f == 8 && cmf >> 4 <= 7 && (u16::from(cmf) << 8 | u16::from(flg)) % 31 == 0
}

impl Stage {
    fn write_all(&mut self, chunk: &[u8]) -> std::io::Result<()> {
        match self {
            Stage::Inflate(d) => d.write_all(chunk),
            Stage::Brotli(d) => d.write_all(chunk),
            Stage::Sniff(head, _) => {
                head.extend_from_slice(chunk);
                if head.len() < 2 {
                    return Ok(());
                }
                let head = std::mem::take(head);
                *self = Stage::Inflate(Inflater::new(if is_zlib_header(head[0], head[1]) {
                    InflateFormat::Zlib
                } else {
                    InflateFormat::Raw
                }));
                self.write_all(&head)
            }
        }
    }

    /// The decoded output written so far, which the caller empties.
    fn out(&mut self) -> &mut Vec<u8> {
        match self {
            Stage::Inflate(d) => d.out(),
            Stage::Brotli(d) => d.get_mut(),
            Stage::Sniff(_, out) => out,
        }
    }

    fn finish(self) -> Result<Vec<u8>, NetError> {
        let err = |e: std::io::Error| NetError::Decode(e.to_string());
        match self {
            Stage::Inflate(d) => d.finish().map_err(err),
            Stage::Brotli(d) => d.into_inner().map_err(|_| NetError::Decode("truncated brotli stream".into())),
            Stage::Sniff(head, _) if head.is_empty() => Ok(Vec::new()),
            // A one-byte body: no zlib stream, so raw deflate decides.
            Stage::Sniff(head, _) => {
                let mut d = Inflater::new(InflateFormat::Raw);
                d.write_all(&head).map_err(err)?;
                d.finish().map_err(err)
            }
        }
    }
}

/// A response body's content decoding: its codings in decoding order (the
/// reverse of the header's), none for an identity body.
struct Decoder {
    stages: Vec<Stage>,
}

impl Decoder {
    const IDENTITY: Decoder = Decoder { stages: Vec::new() };

    /// The decoder for a `Content-Encoding` value (its header lines joined
    /// by commas). Like Chromium, a list decodes in reverse order, and a
    /// list naming `identity` or a coding not supported passes the body
    /// through undecoded instead of failing the request.
    fn for_encoding(enc: Option<&str>) -> Self {
        let Some(enc) = enc else { return Decoder::IDENTITY };
        let mut stages = Vec::new();
        for token in enc.split(',').map(str::trim).filter(|t| !t.is_empty()) {
            let stage = if token.eq_ignore_ascii_case("gzip") || token.eq_ignore_ascii_case("x-gzip") {
                Stage::Inflate(Inflater::new(InflateFormat::Gzip))
            } else if token.eq_ignore_ascii_case("deflate") {
                Stage::Sniff(Vec::new(), Vec::new())
            } else if token.eq_ignore_ascii_case("br") {
                Stage::Brotli(Box::new(brotli_decompressor::DecompressorWriter::new(Vec::new(), 4096)))
            } else {
                return Decoder::IDENTITY;
            };
            stages.push(stage);
        }
        stages.reverse();
        Decoder { stages }
    }

    /// Feeds an encoded chunk. Its decoded output is the chunk itself
    /// (`None`, identity) or what the last coding wrote to its output
    /// vector, which the caller empties.
    fn write(&mut self, chunk: &[u8]) -> Result<Option<&mut Vec<u8>>, NetError> {
        let err = |e: std::io::Error| NetError::Decode(e.to_string());
        let Some((first, rest)) = self.stages.split_first_mut() else { return Ok(None) };
        first.write_all(chunk).map_err(err)?;
        let mut prev = first;
        for next in rest {
            // The previous coding's output feeds the next; its vector keeps
            // its capacity.
            let mut buf = std::mem::take(prev.out());
            next.write_all(&buf).map_err(err)?;
            buf.clear();
            *prev.out() = buf;
            prev = next;
        }
        Ok(Some(prev.out()))
    }

    /// Feeds an encoded chunk, returning the decoded output in an
    /// allocation of exactly its size, which the response's readers then
    /// share (the sink, the Network body store, the HTTP caches). A network
    /// chunk is a slice of the connection's read buffer, all of which stays
    /// allocated while the slice lives (a SPA's 3.5 MB of bodies held
    /// 7.2 MB of read buffers); copied, the buffer is free for the next
    /// read at once. The decoder's output vector grows by doubling and
    /// stays the decoder's.
    fn push(&mut self, chunk: &[u8]) -> Result<Bytes, NetError> {
        Ok(match self.write(chunk)? {
            None => Bytes::copy_from_slice(chunk),
            Some(out) => {
                let decoded = Bytes::copy_from_slice(out);
                out.clear();
                decoded
            }
        })
    }

    /// Feeds an encoded chunk, appending the decoded output to `out`.
    fn push_into(&mut self, chunk: &[u8], out: &mut Vec<u8>) -> Result<(), NetError> {
        match self.write(chunk)? {
            None => out.extend_from_slice(chunk),
            Some(decoded) => out.append(decoded),
        }
        Ok(())
    }

    /// The output the decoder still held at the end of the stream: each
    /// coding's tail runs through the codings after it.
    fn finish(self) -> Result<Vec<u8>, NetError> {
        let err = |e: std::io::Error| NetError::Decode(e.to_string());
        let mut tail: Option<Vec<u8>> = None;
        for mut stage in self.stages {
            if let Some(t) = tail.take() {
                stage.write_all(&t).map_err(err)?;
            }
            tail = Some(stage.finish()?);
        }
        Ok(tail.unwrap_or_default())
    }
}

const SEC_FETCH_SITE: header::HeaderName = header::HeaderName::from_static("sec-fetch-site");
const SEC_FETCH_MODE: header::HeaderName = header::HeaderName::from_static("sec-fetch-mode");
const SEC_FETCH_DEST: header::HeaderName = header::HeaderName::from_static("sec-fetch-dest");
const SEC_FETCH_USER: header::HeaderName = header::HeaderName::from_static("sec-fetch-user");

/// The `Sec-Fetch-Site` relation of one hop: to the requesting document's
/// origin, else (a navigation a document started) its referrer, else none
/// (one the browser or automation started).
fn hop_site(req: &Request, origin_url: Option<&Url>) -> FetchSite {
    match (&req.origin, &req.referrer) {
        (Some(_), _) => FetchSite::of(origin_url, &req.url),
        (None, Some(referrer)) => FetchSite::of(Some(referrer), &req.url),
        (None, None) => FetchSite::None,
    }
}

/// The rustls certificate error inside `io` (hyper-rustls wraps the TLS
/// stream's `io::Error` in another one).
fn certificate_error(io: &std::io::Error) -> Option<String> {
    let inner = io.get_ref()?;
    if let Some(rustls::Error::InvalidCertificate(c)) = inner.downcast_ref::<rustls::Error>() {
        return Some(format!("{c:?}"));
    }
    certificate_error(inner.downcast_ref::<std::io::Error>()?)
}

/// Why a hop got no response.
enum SendError {
    Net(NetError),
    /// 407 to CONNECT. Without an answer it fails like a refused tunnel.
    ProxyChallenge {
        scheme: String,
        realm: String,
    },
}

impl From<SendError> for NetError {
    fn from(e: SendError) -> NetError {
        match e {
            SendError::Net(e) => e,
            SendError::ProxyChallenge { .. } => NetError::Proxy(TUNNEL_FAILED),
        }
    }
}

const TUNNEL_FAILED: &str = "net::ERR_TUNNEL_CONNECTION_FAILED";

fn classify(e: &hyper_util::client::legacy::Error) -> SendError {
    let mut src: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(e);
    let mut text = e.to_string();
    while let Some(s) = src {
        match s.downcast_ref::<ProxyError>() {
            Some(ProxyError::Unreachable(_)) => {
                return SendError::Net(NetError::Proxy("net::ERR_PROXY_CONNECTION_FAILED"))
            }
            Some(ProxyError::Tunnel(_)) => return SendError::Net(NetError::Proxy(TUNNEL_FAILED)),
            Some(ProxyError::Challenge { scheme, realm }) => {
                return SendError::ProxyChallenge { scheme: scheme.clone(), realm: realm.clone() }
            }
            None => {}
        }
        if let Some(io) = s.downcast_ref::<std::io::Error>() {
            if let Some(c) = certificate_error(io) {
                return SendError::Net(NetError::Certificate(c));
            }
            match io.kind() {
                std::io::ErrorKind::ConnectionRefused => return SendError::Net(NetError::ConnectionRefused),
                std::io::ErrorKind::TimedOut => return SendError::Net(NetError::Timeout),
                _ => {}
            }
        }
        text = format!("{text}: {s}");
        src = s.source();
    }
    if text.contains("dns error") || text.contains("failed to lookup") {
        return SendError::Net(NetError::NameNotResolved);
    }
    if e.is_connect() {
        return SendError::Net(NetError::ConnectionRefused);
    }
    SendError::Net(NetError::Other(text))
}

impl Io {
    fn origin_semaphore(&self, url: &Url) -> Arc<Semaphore> {
        // Keyed by the origin itself (`Hash + Eq`), not its serialization.
        let mut m = self.per_origin.lock().unwrap();
        m.entry(url.origin()).or_insert_with(|| Arc::new(Semaphore::new(self.shared.config.max_per_origin))).clone()
    }

    /// An origin that answered over HTTP/2 multiplexes one connection: the
    /// six-connection limit no longer applies to it.
    fn multiplexing(&self, url: &Url) {
        let key = url.origin();
        let mut h2_origins = self.h2_origins.lock().unwrap();
        if !h2_origins.contains(&key) {
            let cfg = &self.shared.config;
            if let Some(semaphore) = self.per_origin.lock().unwrap().get(&key) {
                semaphore.add_permits(cfg.max_per_h2_origin.saturating_sub(cfg.max_per_origin));
            }
            h2_origins.insert(key);
        }
    }

    fn store_cookies(&self, url: &Url, headers: &HeaderMap, allowed: bool) {
        if !allowed {
            return;
        }
        let now = crate::cookies::now_secs();
        let mut jar = self.cookies.lock().unwrap();
        for v in headers.get_all(header::SET_COOKIE) {
            if let Ok(s) = v.to_str() {
                jar.set_from_header(url, s, crate::cookies::Access::Http, now);
            }
        }
    }

    fn proxy_for(&self, url: &Url) -> Option<&ProxyState> {
        self.shared.proxy.as_deref().filter(|p| p.applies_to(url))
    }

    async fn send(
        &self,
        method: Method,
        url: &Url,
        mut headers: HeaderMap,
        body: Option<Bytes>,
    ) -> Result<http::Response<hyper::body::Incoming>, SendError> {
        // On a tunnel the proxy sees this header at CONNECT, and it must not
        // reach the target. An absolute-form request carries it itself.
        if url.scheme() == "http" {
            if let Some(value) = self.proxy_for(url).and_then(|p| p.authorization()) {
                headers.insert(header::PROXY_AUTHORIZATION, value);
            }
        }
        let mut b = http::Request::builder().method(method).uri(url.as_str());
        if let Some(h) = b.headers_mut() {
            *h = headers;
        }
        let r = b.body(Full::new(body.unwrap_or_default())).map_err(|_| SendError::Net(NetError::InvalidUrl))?;
        self.pooled().0.request(r).await.map_err(|e| classify(&e))
    }

    /// The pooled client for the context's current certificate mode, and the
    /// registry of its HTTP/2 connections.
    fn pooled(&self) -> (&HttpClient, &Arc<H2Sessions>) {
        if self.shared.ignore_certificate_errors.load(Ordering::Relaxed) {
            let client = self.insecure.get_or_init(|| {
                http_client(
                    &self.shared.config,
                    &tls_config_insecure(),
                    self.shared.proxy.clone(),
                    self.insecure_sessions.clone(),
                )
            });
            (client, &self.insecure_sessions)
        } else {
            (&self.client, &self.sessions)
        }
    }

    async fn run(
        &self,
        id: RequestId,
        mut req: Request,
        sink: &SharedSink,
        mut staged: Option<Staged>,
    ) -> Result<(), NetError> {
        let origin_url = req.origin.as_deref().and_then(|o| Url::parse(o).ok());
        let max_body = req.max_body.unwrap_or(self.shared.config.max_body);
        let mut redirects = 0u32;
        // The referrer of the hop being made. `req.referrer` stays the
        // request's initiator (Fetch Metadata, the `file:` check).
        let mut referrer = crate::referrer::ReferrerChain::new(req.referrer.clone(), req.referrer_policy);
        // `Sec-Fetch-Site`: the most cross-site relation of the chain so far.
        let mut site = FetchSite::None;
        let mut redirect_response: Option<ResponseHead> = None;
        let mut tainted = false; // cross-origin redirect in cors mode => Origin "null"
        // pyro: Fetch "response tainting" of a no-cors request: "opaque" from the first hop to
        // another origin on, a later same-origin hop included (a redirect back does not clear it).
        let mut no_cors_opaque = false;
        let mut unconditional = false; // a 304 no longer matched its entry: refetch without validators
        let mut pause_response: Option<RequestInfo> = None; // set by `PauseResponse`: the final hop's response is paused

        // Resource Timing's TAO check runs against the requesting document's
        // origin. An iframe's navigation is checked against its parent, as
        // the loader names it (else the referrer), never `req.origin`: that
        // is the Origin header's, for a form POST the frame's own document.
        // An iframe whose parent is unknown fails closed (an opaque origin).
        let mut timing_origin = if req.destination == Destination::Iframe {
            Some(req.timing_origin.clone().unwrap_or_else(|| {
                req.referrer.as_ref().map_or_else(|| "null".into(), |r| r.origin().ascii_serialization())
            }))
        } else {
            req.timing_origin.clone().or_else(|| req.origin.clone())
        };
        loop {
            crate::timing::note(|t| {
                // Authorization belongs to this response. Local or intercepted
                // responses must not inherit a grant from a preceding hop.
                t.timing_allow_checked = false;
                t.fetch_start = if redirects == 0 { t.start } else { Some(Instant::now()) };
                t.connection = None;
                t.request_start = None;
                t.response_start = None;
            });
            if tainted {
                req.origin = Some("null".into());
            }
            // Same-origin to the requester (`cors::is_same_origin_request`),
            // once per hop: a request without an initiator is browser-owned;
            // a tainted ("null") or unparsable one is never same-origin.
            let same_origin = req.origin.is_none()
                || (!tainted && origin_url.as_ref().is_some_and(|o| cors::same_origin(o, &req.url)));
            let cors_cross = self.shared.config.enforce_cors && req.mode == RequestMode::Cors && !same_origin;
            no_cors_opaque |= req.mode == RequestMode::NoCors && !same_origin;
            // Same-origin as far as the response's tainting goes (`no_cors_opaque`).
            let basic_tainting = same_origin && !no_cors_opaque;
            site = site.max(hop_site(&req, origin_url.as_ref()));
            // The first hop of a request its caller's thread paused comes
            // with its headers built, its start published and its decision.
            let (mut headers, mut decided) = match staged.take() {
                Some(staged) => (staged.headers, Some(staged.decision)),
                None => (
                    self.shared.build_headers(
                        &self.cookies,
                        &req,
                        referrer.referrer.as_ref(),
                        origin_url.as_ref(),
                        cors_cross,
                        site,
                        basic_tainting,
                    ),
                    None,
                ),
            };
            let cache_mode = self.shared.cache_mode(&req, &mut headers);
            // The request as sent, made for each consumer (the observers here,
            // an interceptor below) instead of copied for both up front.
            let info = |headers: &HeaderMap| RequestInfo {
                url: req.url.clone(),
                method: req.method.clone(),
                headers: headers.clone(),
                post_data: req.body.clone(),
                destination: req.destination,
                frame_id: req.frame_id.clone(),
                loader_id: req.loader_id.clone(),
            };
            if decided.is_none() {
                self.shared.emit(NetEvent::RequestWillBeSent {
                    id,
                    request: Box::new(info(&headers)),
                    redirect_response: redirect_response.take().map(Box::new),
                    timestamp: self.shared.now(),
                    wall_time: wall_time(),
                });
            }

            // Observers must see a start before every terminal/response path,
            // including local data URLs and requests rejected before I/O.
            if req.url.scheme() == "data" {
                return self.serve_data(id, &req, sink, None);
            }
            if req.url.scheme() == "blob" {
                return self.serve_blob(id, &req, sink);
            }
            if req.url.scheme() == "file" && self.shared.config.file_urls {
                // Chrome refuses redirects into local files.
                if redirects > 0 {
                    return Err(NetError::Blocked("net::ERR_UNSAFE_REDIRECT".into()));
                }
                return self.serve_file(id, &req, sink).await;
            }
            match req.url.scheme() {
                "http" | "https" => {}
                s => return Err(NetError::UnsupportedScheme(s.to_owned())),
            }
            if self.shared.settings.lock().unwrap().offline {
                return Err(NetError::Blocked("net::ERR_INTERNET_DISCONNECTED".into()));
            }
            if req.mode == RequestMode::SameOrigin && !same_origin {
                return Err(NetError::Cors("Cross-origin request in same-origin mode".into()));
            }

            // CDP Fetch interception (request stage).
            let interceptor =
                if req.skip_interception { None } else { self.shared.interceptor.lock().unwrap().clone() };
            let (mut method, mut url, mut headers, mut body) =
                (req.method.clone(), req.url.clone(), headers, req.body.clone());
            // `Some(None)`: the interceptor dropped its reply.
            let decision = match (decided.take(), &interceptor) {
                (Some(decision), _) => Some(decision),
                (None, Some(i)) => {
                    let (tx, rx) = oneshot::channel();
                    i.paused(PausedRequest { id, request: info(&headers) }, tx);
                    Some(rx.await.ok())
                }
                (None, None) => None,
            };
            match decision {
                None => {}
                Some(Some(InterceptDecision::PauseResponse)) => {
                    if pause_response.is_none() {
                        pause_response = Some(info(&headers));
                    }
                }
                Some(Some(InterceptDecision::Continue { url: u, method: m, headers: h, body: b })) => {
                    if let Some(u) = u {
                        url = u;
                    }
                    if let Some(m) = m {
                        method = m;
                    }
                    if let Some(h) = h {
                        headers = h;
                    }
                    if b.is_some() {
                        body = b;
                    }
                }
                Some(Some(InterceptDecision::Fulfill { status, headers: h, body: b })) => {
                    return self.fulfill(id, &req, sink, status, h, b);
                }
                Some(Some(InterceptDecision::Fail(reason))) => return Err(NetError::Blocked(cdp_reason(&reason))),
                Some(None) => return Err(NetError::Other("interception dropped".into())),
            }

            // Static files for an origin served from disk: after interception
            // (routes can still override) and before the cache and network.
            if (method == Method::GET || method == Method::HEAD) && !self.shared.config.static_files.is_empty() {
                let document = req.destination == Destination::Document;
                if let Some(hit) = self.shared.config.static_files.resolve(&url, document) {
                    return self.serve_static(id, &req, sink, &url, &method, hit, cors_cross, same_origin).await;
                }
            }

            // CORS preflight.
            if cors_cross && cors::needs_preflight(&req) {
                let origin = req.origin.clone().unwrap_or_else(|| "null".into());
                let covered = self.preflight.lock().unwrap().covers(&req, &origin);
                if !covered {
                    let mut ph = cors::preflight_request_headers(&req, &origin);
                    for name in [header::REFERER, SEC_FETCH_SITE, SEC_FETCH_MODE, SEC_FETCH_DEST] {
                        if let Some(v) = headers.get(&name) {
                            ph.insert(name, v.clone());
                        }
                    }
                    let _permit = self.origin_semaphore(&url).acquire_owned().await;
                    let resp = self.send(Method::OPTIONS, &url, ph, None).await?;
                    let status = resp.status().as_u16();
                    let rh = resp.headers().clone();
                    drop(resp.into_body().collect().await);
                    self.preflight.lock().unwrap().record(status, &rh, &req, &origin).map_err(NetError::Cors)?;
                }
            }

            // HTTP cache (RFC 9111), after request-stage interception and CORS
            // preflight so that both still apply to cached responses.
            // A response to be paused is neither served from nor stored in it.
            let cacheable = method == Method::GET
                && body.is_none()
                && !headers.contains_key(header::RANGE)
                && pause_response.is_none();
            let request_time = wall_time();
            let sent = if cacheable { Some(headers.clone()) } else { None };
            // pyro: whether this hop carries credentials (as the cookie decision above): the cache
            // serves a response only to a request of the same kind (`HttpCache::lookup`).
            let credentialed =
                includes_credentials(req.credentials, basic_tainting) || headers.contains_key(header::AUTHORIZATION);
            let mut revalidating = None;
            if cacheable {
                let lookup =
                    self.shared.cache.lock().unwrap().lookup(&url, &headers, credentialed, cache_mode, request_time);
                match lookup {
                    crate::cache::Lookup::Hit(cached) => {
                        // A stored response passes the TAO check a network one would.
                        crate::timing::note_timing_allow(&url, &cached.headers, timing_origin.as_deref());
                        let hop = Hop { cors_cross, same_origin: basic_tainting, redirected: redirects > 0 };
                        return self.serve_cached(id, &req, sink, &url, cached, hop, true, 0);
                    }
                    crate::cache::Lookup::Revalidate(cached) if !unconditional => {
                        for (name, value) in cached.validators() {
                            headers.insert(name, value);
                        }
                        revalidating = Some(cached);
                    }
                    _ => {}
                }
            }
            // Opt-in shared public assets: only after a private miss, only for
            // credential-free requests, fresh entries only (never revalidated).
            let shared_assets = self
                .shared
                .config
                .shared_asset_cache
                .as_ref()
                .filter(|_| cacheable && crate::cache::shared_request_eligible(&headers));
            if let (Some(store), None) = (shared_assets, revalidating.as_ref()) {
                let lookup = store.with(|c| c.lookup(&url, &headers, credentialed, cache_mode, request_time));
                if let crate::cache::Lookup::Hit(cached) = lookup {
                    crate::timing::note_timing_allow(&url, &cached.headers, timing_origin.as_deref());
                    let hop = Hop { cors_cross, same_origin: basic_tainting, redirected: redirects > 0 };
                    return self.serve_cached(id, &req, sink, &url, cached, hop, true, 0);
                }
            }
            if cache_mode == CacheMode::OnlyIfCached && revalidating.is_none() {
                return Err(NetError::Other("net::ERR_CACHE_MISS".into()));
            }

            let permit = self.origin_semaphore(&url).acquire_owned().await;
            let request_start = Instant::now();
            let mut auth_attempts = 0;
            let auth_interceptor = interceptor.as_ref().filter(|i| i.handles_auth());
            let resp = loop {
                // Ordinary traffic does not need retry copies of the header map.
                let sent = if auth_interceptor.is_some() {
                    self.send(method.clone(), &url, headers.clone(), body.clone()).await
                } else {
                    self.send(method.clone(), &url, std::mem::take(&mut headers), body.take()).await
                };
                // `resp` is None for a 407 to CONNECT: no response to fall back on.
                let (resp, proxy, scheme, realm) = match sent {
                    Ok(resp) => {
                        let proxy = resp.status() == StatusCode::PROXY_AUTHENTICATION_REQUIRED;
                        if (resp.status() != StatusCode::UNAUTHORIZED && !proxy) || auth_interceptor.is_none() {
                            break resp;
                        }
                        let challenge_header =
                            if proxy { header::PROXY_AUTHENTICATE } else { header::WWW_AUTHENTICATE };
                        let Some((scheme, realm)) = auth_challenge(resp.headers(), challenge_header) else {
                            break resp;
                        };
                        (Some(resp), proxy, scheme, realm)
                    }
                    Err(SendError::ProxyChallenge { scheme, realm }) if auth_interceptor.is_some() => {
                        (None, true, scheme, realm)
                    }
                    Err(e) => return Err(e.into()),
                };
                let Some(interceptor) = auth_interceptor else { unreachable!("no challenge without an interceptor") };
                if auth_attempts >= self.shared.config.max_auth_retries {
                    return Err(NetError::Blocked("net::ERR_INVALID_AUTH_CREDENTIALS".into()));
                }
                let proxy_state = if proxy { self.proxy_for(&url) } else { None };
                let (tx, rx) = oneshot::channel();
                interceptor.auth_required(
                    AuthChallenge {
                        id,
                        request: RequestInfo {
                            url: url.clone(),
                            method: method.clone(),
                            headers: headers.clone(),
                            post_data: body.clone(),
                            destination: req.destination,
                            frame_id: req.frame_id.clone(),
                            loader_id: req.loader_id.clone(),
                        },
                        origin: proxy_state
                            .map_or_else(|| url.origin().ascii_serialization(), |p| p.origin().to_owned()),
                        scheme: scheme.clone(),
                        realm,
                        proxy,
                    },
                    tx,
                );
                match rx.await {
                    Ok(AuthDecision::Default | AuthDecision::CancelAuth) => match resp {
                        Some(resp) => break resp,
                        None => return Err(NetError::Proxy(TUNNEL_FAILED)),
                    },
                    Ok(AuthDecision::ProvideCredentials { username, password }) => {
                        if proxy && proxy_state.is_none() {
                            return Err(NetError::Other("407 from a server that is not the proxy".into()));
                        }
                        if scheme != "basic" {
                            return Err(NetError::Other(format!("unsupported authentication scheme: {scheme}")));
                        }
                        let value = basic_authorization(&username, &password).map_err(NetError::Other)?;
                        // Proxy credentials belong to the connection, not to this
                        // request: `send` and the connector add them from now on.
                        match proxy_state {
                            Some(state) => state.set_authorization(value),
                            None => {
                                headers.insert(header::AUTHORIZATION, value);
                            }
                        }
                        auth_attempts += 1;
                        // Drop the challenge body without buffering it. Keep the same
                        // request ID and request-stage pause throughout auth retries.
                        drop(resp);
                    }
                    Err(_) => return Err(NetError::Other("authentication interception dropped".into())),
                }
            };
            // pyro: a response to an authentication retry answers the request as last sent, with
            // its `Authorization`: it is stored (if at all) under those headers and as credentialed,
            // never as the anonymous request that got the challenge (the shared asset store refuses
            // a request with credentials).
            let (sent, credentialed) = if auth_attempts > 0 && headers.contains_key(header::AUTHORIZATION) {
                (sent.map(|_| headers.clone()), true)
            } else {
                (sent, credentialed)
            };
            let status = resp.status();
            // The server's reason phrase when it is not the standard one (hyper keeps only those): XHR and fetch
            // report it as statusText, as Chromium does (`HTTP/1.1 204 UNICORNSWIN`).
            let reason = resp
                .extensions()
                .get::<hyper::ext::ReasonPhrase>()
                .map(|r| String::from_utf8_lossy(r.as_bytes()).into_owned());
            let resp_headers = own_headers(resp.headers());
            let protocol = resp.version();
            // A connection opened for this request reports its phases; the
            // request went out once it was ready.
            let connection = resp
                .extensions()
                .get::<crate::timing::ConnInfo>()
                .map(crate::timing::ConnInfo::timing)
                .filter(|c| c.start >= request_start);
            crate::timing::note(|t| {
                t.response_start = Some(Instant::now());
                t.connection = connection;
                t.request_start = Some(connection.map_or(request_start, |c| c.end.max(request_start)));
            });
            if protocol == Version::HTTP_2 {
                self.multiplexing(&url);
            }
            // pyro: the same decision as the cookies sent (`includes_credentials`): a tainted hop
            // (a no-cors chain that went through another origin) neither sends nor stores.
            self.store_cookies(&url, &resp_headers, includes_credentials(req.credentials, basic_tainting));
            let response_time = wall_time();
            if !matches!(method, Method::GET | Method::HEAD | Method::OPTIONS | Method::TRACE)
                && (status.is_success() || status.is_redirection())
            {
                self.invalidate_after_unsafe(&url, &resp_headers);
            }
            if status == StatusCode::NOT_MODIFIED {
                if let Some(cached) = revalidating.take() {
                    drop(resp);
                    drop(permit);
                    // pyro: into an entry of the same kind as the request as last sent only.
                    let updated = self.shared.cache.lock().unwrap().update_from_304(
                        &url,
                        cached.id,
                        &resp_headers,
                        request_time,
                        response_time,
                        credentialed,
                    );
                    match updated {
                        Some(updated) => {
                            crate::timing::note_timing_allow(&url, &updated.headers, timing_origin.as_deref());
                            let hop = Hop { cors_cross, same_origin: basic_tainting, redirected: redirects > 0 };
                            return self.serve_cached(id, &req, sink, &url, updated, hop, false, 0);
                        }
                        None => {
                            unconditional = true;
                            continue;
                        }
                    }
                }
            }

            // Redirects.
            if status.is_redirection() && status != StatusCode::NOT_MODIFIED {
                if let Some(loc) = resp_headers.get(header::LOCATION).and_then(|v| v.to_str().ok()) {
                    let target = url.join(loc).map_err(|_| NetError::InvalidUrl)?;
                    crate::timing::note_timing_allow(&url, &resp_headers, timing_origin.as_deref());
                    drop(resp.into_body().collect().await);
                    drop(permit);
                    match req.redirect {
                        RedirectMode::Error => return Err(NetError::RedirectNotAllowed),
                        RedirectMode::Manual => {
                            let head = self
                                .head(&url, status, resp_headers, ResponseKind::OpaqueRedirect, redirects > 0)
                                .over(protocol);
                            return self.finish_empty(id, &req, sink, head);
                        }
                        RedirectMode::Follow => {}
                    }
                    if cors_cross {
                        cors::cors_check(&resp_headers, req.origin.as_deref().unwrap_or("null"), req.credentials)
                            .map_err(NetError::Cors)?;
                    }
                    redirects += 1;
                    if redirects > self.shared.config.max_redirects {
                        return Err(NetError::TooManyRedirects);
                    }
                    let cross_origin = !cors::same_origin(&url, &target);
                    crate::timing::note(|t| {
                        t.redirect_end = Some(Instant::now());
                        t.redirect_count = t.redirect_count.saturating_add(1);
                        t.cross_origin_redirect |= cross_origin;
                    });
                    let head = self
                        .head(&url, status, resp_headers.clone(), ResponseKind::Basic, redirects > 1)
                        .over(protocol);
                    deliver(
                        sink,
                        ResponseEvent::Redirect {
                            from: url.clone(),
                            to: target.clone(),
                            status,
                            headers: resp_headers,
                        },
                    );
                    if (status == StatusCode::SEE_OTHER && req.method != Method::HEAD)
                        || (matches!(status.as_u16(), 301 | 302) && req.method == Method::POST)
                    {
                        req.method = Method::GET;
                        req.body = None;
                        for h in [
                            header::CONTENT_TYPE,
                            header::CONTENT_LENGTH,
                            header::CONTENT_ENCODING,
                            header::CONTENT_LANGUAGE,
                        ] {
                            req.headers.remove(h);
                        }
                    }
                    if !cors::same_origin(&url, &target) {
                        // Fetch serializes a redirect-tainted request origin
                        // as "null" for subsequent Timing-Allow-Origin checks.
                        if timing_origin.as_ref().is_some_and(|origin| *origin != url.origin().ascii_serialization()) {
                            timing_origin = Some("null".into());
                        }
                        req.headers.remove(header::AUTHORIZATION);
                        req.headers.remove(header::COOKIE);
                        if cors_cross {
                            tainted = true;
                        }
                    }
                    // Referrer Policy: the next hop's referrer is the one this
                    // hop sent, under the redirect's policy if it names one.
                    referrer.redirect(&url, &head.headers);
                    req.referrer_policy = referrer.policy;
                    req.url = target;
                    redirect_response = Some(head);
                    continue;
                }
            }

            let kind = if cors_cross {
                cors::cors_check(&resp_headers, req.origin.as_deref().unwrap_or("null"), req.credentials)
                    .map_err(NetError::Cors)?;
                ResponseKind::Cors
            } else if req.mode == RequestMode::NoCors && !basic_tainting {
                ResponseKind::Opaque
            } else {
                ResponseKind::Basic
            };
            let encoding = content_encoding(&resp_headers);
            if let (Some(request), Some(interceptor)) = (pause_response.take(), interceptor.as_ref()) {
                let body = read_whole_body(resp, encoding.as_deref(), max_body).await?;
                drop(permit);
                let (tx, rx) = oneshot::channel();
                let paused = PausedResponse {
                    id,
                    request,
                    status: status.as_u16(),
                    headers: resp_headers.clone(),
                    body: body.clone(),
                };
                interceptor.response(paused, tx);
                let net_status = status;
                let (status, resp_headers, body) = match rx.await {
                    Ok(ResponseDecision::Continue) => (status, resp_headers, body),
                    Ok(ResponseDecision::Fulfill { status, headers, body }) => (
                        StatusCode::from_u16(status).map_err(|_| NetError::Other("invalid status".into()))?,
                        headers,
                        body,
                    ),
                    Ok(ResponseDecision::Fail(reason)) => return Err(NetError::Blocked(cdp_reason(&reason))),
                    Err(_) => return Err(NetError::Other("response interception dropped".into())),
                };
                crate::timing::note_timing_allow(&url, &resp_headers, timing_origin.as_deref());
                let mut head = self.head(&url, status, resp_headers, kind, redirects > 0).over(protocol);
                if let Some(reason) = reason.filter(|_| head.status == net_status) {
                    head.status_text = reason;
                }
                return self.deliver_body(id, &req, sink, head, body);
            }
            // Before the head takes the headers: the coding borrows them.
            let mut decoder = Decoder::for_encoding(encoding.as_deref());
            let (store, mut max_entry) = {
                let cache = self.shared.cache.lock().unwrap();
                let store = sent.as_ref().is_some_and(|s| cache.storable(status, &resp_headers, s, cache_mode));
                (store.then(|| resp_headers.clone()), if store { cache.max_entry_bytes() } else { 0 })
            };
            let shared_store = shared_assets.filter(|s| {
                sent.as_ref().is_some_and(|sent| s.with(|c| c.storable(status, &resp_headers, sent, cache_mode)))
            });
            if let Some(s) = shared_store {
                max_entry = max_entry.max(s.with(|c| c.max_entry_bytes()));
            }
            let shared_response = shared_store.map(|_| resp_headers.clone());
            let mut stored: Option<Vec<Bytes>> = (store.is_some() || shared_store.is_some()).then(Vec::new);
            crate::timing::note_timing_allow(&url, &resp_headers, timing_origin.as_deref());
            let mut head = self.head(&url, status, resp_headers, kind, redirects > 0).over(protocol);
            if let Some(reason) = reason {
                head.status_text = reason;
            }
            self.shared.emit(NetEvent::ResponseReceived {
                id,
                head: Box::new(head.clone()),
                destination: req.destination,
                frame_id: req.frame_id.clone(),
                loader_id: req.loader_id.clone(),
                timestamp: self.shared.now(),
            });
            deliver(sink, ResponseEvent::Response(head));

            // Body.
            let mut body = resp.into_body();
            let (mut encoded, mut decoded) = (0u64, 0u64);
            let observed = self.shared.observed();
            while let Some(frame) = body.frame().await {
                let frame = frame.map_err(|e| NetError::Other(e.to_string()))?;
                let Ok(data) = frame.into_data() else { continue };
                encoded += data.len() as u64;
                let out = decoder.push(&data)?;
                drop(data);
                if !out.is_empty() {
                    decoded += out.len() as u64;
                    if decoded as usize > max_body {
                        return Err(NetError::TooLarge);
                    }
                    if observed {
                        self.shared.emit(NetEvent::DataReceived {
                            id,
                            data: out.clone(),
                            timestamp: self.shared.now(),
                        });
                    }
                    if decoded as usize > max_entry {
                        stored = None;
                    }
                    if let Some(s) = stored.as_mut() {
                        s.push(out.clone());
                    }
                    deliver(sink, ResponseEvent::Data(out));
                }
            }
            let tail = Bytes::copy_from_slice(&decoder.finish()?);
            if !tail.is_empty() {
                decoded += tail.len() as u64;
                if decoded as usize > max_body {
                    return Err(NetError::TooLarge);
                }
                if observed {
                    self.shared.emit(NetEvent::DataReceived { id, data: tail.clone(), timestamp: self.shared.now() });
                }
                if decoded as usize > max_entry {
                    stored = None;
                }
                if let Some(s) = stored.as_mut() {
                    s.push(tail.clone());
                }
                deliver(sink, ResponseEvent::Data(tail));
            }
            drop(permit);
            if let (Some(chunks), Some(sent)) = (stored, sent.as_ref()) {
                let body =
                    if chunks.len() == 1 { chunks.into_iter().next().unwrap() } else { Bytes::from(chunks.concat()) };
                if let Some(response) = store {
                    let mut cache = self.shared.cache.lock().unwrap();
                    cache.insert(&url, sent, credentialed, status, &response, body.clone(), request_time, response_time, protocol);
                }
                if let (Some(s), Some(response)) = (shared_store, shared_response) {
                    s.with(|c| c.insert(&url, sent, credentialed, status, &response, body, request_time, response_time, protocol));
                }
            }
            self.shared.emit(NetEvent::LoadingFinished { id, encoded_len: encoded, timestamp: self.shared.now() });
            deliver(sink, ResponseEvent::Finished { encoded_len: encoded, decoded_len: decoded });
            return Ok(());
        }
    }

    /// RFC 9111 §4.4: a non-error response to an unsafe method invalidates the
    /// target URI, and same-origin Location / Content-Location targets.
    fn invalidate_after_unsafe(&self, url: &Url, headers: &HeaderMap) {
        let mut targets = vec![url.clone()];
        for name in [header::LOCATION, header::CONTENT_LOCATION] {
            if let Some(target) = headers.get(name).and_then(|v| v.to_str().ok()).and_then(|v| url.join(v).ok()) {
                if cors::same_origin(url, &target) {
                    targets.push(target);
                }
            }
        }
        let mut cache = self.shared.cache.lock().unwrap();
        for target in &targets {
            cache.invalidate(target);
        }
        drop(cache);
        if let Some(s) = &self.shared.config.shared_asset_cache {
            s.with(|c| targets.iter().for_each(|t| c.invalidate(t)));
        }
    }

    /// Delivers a stored response: a fresh hit (`from_cache`, no network) or one
    /// just revalidated with a 304. CORS and response tainting are re-applied to
    /// the stored headers exactly as for a network response.
    #[allow(clippy::too_many_arguments)]
    fn serve_cached(
        &self,
        id: RequestId,
        req: &Request,
        sink: &SharedSink,
        url: &Url,
        cached: crate::cache::Cached,
        hop: Hop,
        from_cache: bool,
        encoded_len: u64,
    ) -> Result<(), NetError> {
        let kind = if hop.cors_cross {
            cors::cors_check(&cached.headers, req.origin.as_deref().unwrap_or("null"), req.credentials)
                .map_err(NetError::Cors)?;
            ResponseKind::Cors
        } else if req.mode == RequestMode::NoCors && !hop.same_origin {
            ResponseKind::Opaque
        } else {
            ResponseKind::Basic
        };
        let body = cached.body;
        if body.len() > req.max_body.unwrap_or(self.shared.config.max_body) {
            return Err(NetError::TooLarge);
        }
        crate::timing::served_locally();
        let mut head = self.head(url, cached.status, cached.headers, kind, hop.redirected).over(cached.protocol);
        head.from_cache = from_cache;
        self.shared.emit(NetEvent::ResponseReceived {
            id,
            head: Box::new(head.clone()),
            destination: req.destination,
            frame_id: req.frame_id.clone(),
            loader_id: req.loader_id.clone(),
            timestamp: self.shared.now(),
        });
        deliver(sink, ResponseEvent::Response(head));
        let decoded_len = body.len() as u64;
        if !body.is_empty() {
            if self.shared.observed() {
                self.shared.emit(NetEvent::DataReceived { id, data: body.clone(), timestamp: self.shared.now() });
            }
            deliver(sink, ResponseEvent::Data(body));
        }
        self.shared.emit(NetEvent::LoadingFinished { id, encoded_len, timestamp: self.shared.now() });
        deliver(sink, ResponseEvent::Finished { encoded_len, decoded_len });
        Ok(())
    }

    fn head(
        &self,
        url: &Url,
        status: StatusCode,
        headers: HeaderMap,
        kind: ResponseKind,
        redirected: bool,
    ) -> ResponseHead {
        response_head(url, status, headers, kind, redirected)
    }

    fn finish_empty(
        &self,
        id: RequestId,
        req: &Request,
        sink: &SharedSink,
        head: ResponseHead,
    ) -> Result<(), NetError> {
        self.shared.emit(NetEvent::ResponseReceived {
            id,
            head: Box::new(head.clone()),
            destination: req.destination,
            frame_id: req.frame_id.clone(),
            loader_id: req.loader_id.clone(),
            timestamp: self.shared.now(),
        });
        deliver(sink, ResponseEvent::Response(head));
        self.shared.emit(NetEvent::LoadingFinished { id, encoded_len: 0, timestamp: self.shared.now() });
        deliver(sink, ResponseEvent::Finished { encoded_len: 0, decoded_len: 0 });
        Ok(())
    }

    fn deliver_body(
        &self,
        id: RequestId,
        req: &Request,
        sink: &SharedSink,
        head: ResponseHead,
        body: Bytes,
    ) -> Result<(), NetError> {
        self.shared.deliver_body(id, req, head, body, &mut |ev| deliver(sink, ev))
    }

    /// Answers a request from a `static_files` root: 200 with the file,
    /// typed by extension, allowed for any origin so cross-origin scripts,
    /// fonts and `fetch` from other pages work like a permissive asset host.
    #[allow(clippy::too_many_arguments)]
    async fn serve_static(
        &self,
        id: RequestId,
        req: &Request,
        sink: &SharedSink,
        url: &Url,
        method: &Method,
        hit: crate::static_files::StaticHit,
        cors_cross: bool,
        same_origin: bool,
    ) -> Result<(), NetError> {
        crate::timing::served_locally();
        let document = req.destination == Destination::Document;
        let body = match self.shared.config.static_files.lookup(url, document) {
            Some(memory) if memory.file.path == hit.path => memory.file.body.clone(),
            _ => match tokio::fs::read(&hit.path).await {
                Ok(bytes) => Bytes::from(bytes),
                Err(e) => return Err(NetError::Other(format!("static file {}: {e}", hit.path.display()))),
            },
        };
        let mut headers = static_headers(crate::static_files::content_type(&hit.path), body.len(), hit.spa_fallback);
        if cors_cross && req.credentials == Credentials::Include {
            // `*` never satisfies a credentialed check: echo the requester.
            let origin = req.origin.clone().unwrap_or_else(|| "null".into());
            if let Ok(v) = HeaderValue::from_str(&origin) {
                headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, v);
                headers.insert(header::ACCESS_CONTROL_ALLOW_CREDENTIALS, HeaderValue::from_static("true"));
            }
        }
        let kind = if cors_cross {
            cors::cors_check(&headers, req.origin.as_deref().unwrap_or("null"), req.credentials)
                .map_err(NetError::Cors)?;
            ResponseKind::Cors
        } else if req.mode == RequestMode::NoCors && !same_origin {
            ResponseKind::Opaque
        } else {
            ResponseKind::Basic
        };
        let head = self.head(url, StatusCode::OK, headers, kind, false);
        let body = if *method == Method::HEAD { Bytes::new() } else { body };
        self.deliver_body(id, req, sink, head, body)
    }

    fn fulfill(
        &self,
        id: RequestId,
        req: &Request,
        sink: &SharedSink,
        status: u16,
        headers: HeaderMap,
        body: Bytes,
    ) -> Result<(), NetError> {
        self.shared.fulfill(id, req, status, headers, body, &mut |ev| deliver(sink, ev))
    }

    fn serve_data(
        &self,
        id: RequestId,
        req: &Request,
        sink: &SharedSink,
        _redirect: Option<ResponseHead>,
    ) -> Result<(), NetError> {
        let (mime, body) = crate::data_url::parse(&req.url).ok_or(NetError::InvalidUrl)?;
        let mut headers = HeaderMap::new();
        if let Ok(v) = HeaderValue::from_str(&mime) {
            headers.insert(header::CONTENT_TYPE, v);
        }
        let head = self.head(&req.url, StatusCode::OK, headers, ResponseKind::Basic, false);
        // A HEAD response has no body, as for any other scheme.
        let body = if req.method == Method::HEAD { Bytes::new() } else { body };
        self.deliver_body(id, req, sink, head, body)
    }

    /// `file:` from the local filesystem, as Chrome serves it: a 200 typed
    /// by the file's extension. Like Chrome, web pages cannot reach local
    /// files (a request made for an `http(s)` document is denied), `fetch`,
    /// XHR, EventSource and WebSocket do not support the scheme, and module
    /// scripts (CORS mode, opaque `null` origin) are refused. A missing file
    /// or a directory is `net::ERR_FILE_NOT_FOUND`.
    async fn serve_file(&self, id: RequestId, req: &Request, sink: &SharedSink) -> Result<(), NetError> {
        let web_requester = req
            .origin
            .as_deref()
            .and_then(|o| Url::parse(o).ok())
            .or_else(|| req.referrer.clone())
            .is_some_and(|u| matches!(u.scheme(), "http" | "https"));
        if web_requester {
            return Err(NetError::Blocked("net::ERR_ACCESS_DENIED".into()));
        }
        if matches!(
            req.destination,
            Destination::Fetch | Destination::Xhr | Destination::EventSource | Destination::WebSocket
        ) {
            return Err(NetError::UnsupportedScheme("file".into()));
        }
        if req.mode == RequestMode::Cors
            && matches!(req.destination, Destination::Script | Destination::Style | Destination::Json)
        {
            return Err(NetError::Cors("Cross origin requests are not supported for the file: scheme".into()));
        }
        if req.method != Method::GET && req.method != Method::HEAD {
            return Err(NetError::Other("file: URLs can only be fetched with GET".into()));
        }
        let not_found = || NetError::Blocked("net::ERR_FILE_NOT_FOUND".into());
        let path = req.url.to_file_path().map_err(|_| not_found())?;
        let meta = tokio::fs::metadata(&path).await.map_err(|_| not_found())?;
        if !meta.is_file() {
            return Err(not_found());
        }
        let max_body = req.max_body.unwrap_or(self.shared.config.max_body);
        if meta.len() > max_body as u64 {
            return Err(NetError::TooLarge);
        }
        let body = if req.method == Method::HEAD {
            Bytes::new()
        } else {
            Bytes::from(tokio::fs::read(&path).await.map_err(|_| not_found())?)
        };
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(crate::static_files::content_type(&path)));
        headers.insert(header::CONTENT_LENGTH, HeaderValue::from(meta.len()));
        let head = self.head(&req.url, StatusCode::OK, headers, ResponseKind::Basic, false);
        self.deliver_body(id, req, sink, head, body)
    }

    /// Fetch "scheme fetch" for `blob:`: the object the caller resolved from
    /// its blob URL store, whole (200) or the single byte range a `Range`
    /// header names (206 with `Content-Range`). Any other method, a revoked or
    /// unknown URL, an unparsable or unsatisfiable range is a network error.
    fn serve_blob(&self, id: RequestId, req: &Request, sink: &SharedSink) -> Result<(), NetError> {
        if req.method != Method::GET {
            return Err(NetError::Other("blob: URLs can only be fetched with GET".into()));
        }
        let blob = req.blob.as_ref().ok_or_else(|| NetError::Other("unknown or revoked blob: URL".into()))?;
        let len = blob.bytes.len() as u64;
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_str(&blob.mime).unwrap_or(HeaderValue::from_static("")));
        let (status, body) = match req.headers.get(header::RANGE) {
            None => (StatusCode::OK, blob.bytes.clone()),
            Some(value) => {
                let range = crate::range::parse_single_range(value.as_bytes())
                    .ok_or_else(|| NetError::Other("invalid Range header for a blob: URL".into()))?;
                let (start, end) = crate::range::resolve_range(range, len)
                    .ok_or_else(|| NetError::Other("unsatisfiable Range for a blob: URL".into()))?;
                let content_range = format!("bytes {start}-{}/{len}", end - 1);
                headers.insert(header::CONTENT_RANGE, HeaderValue::from_str(&content_range).expect("digits"));
                (StatusCode::PARTIAL_CONTENT, blob.bytes.slice(start as usize..end as usize))
            }
        };
        headers.insert(header::CONTENT_LENGTH, HeaderValue::from(body.len()));
        let head = self.head(&req.url, status, headers, ResponseKind::Basic, false);
        self.deliver_body(id, req, sink, head, body)
    }
}

/// The response's `Content-Encoding`, its header lines joined by commas
/// (borrowed when there is one line, as there nearly always is).
fn content_encoding(headers: &HeaderMap) -> Option<Cow<'_, str>> {
    let mut lines = headers.get_all(header::CONTENT_ENCODING).iter().filter_map(|v| v.to_str().ok());
    let first = lines.next()?;
    Some(lines.fold(Cow::Borrowed(first), |all, line| {
        let mut all = all.into_owned();
        all.push(',');
        all.push_str(line);
        Cow::Owned(all)
    }))
}

/// What of a hop a locally answered response (a cache hit, a static file)
/// is served under: its CORS mode, same-origin relation and redirect history.
#[derive(Clone, Copy)]
struct Hop {
    cors_cross: bool,
    same_origin: bool,
    redirected: bool,
}

/// Reads and decodes a whole response body (at most `max_body` bytes).
async fn read_whole_body(
    resp: http::Response<hyper::body::Incoming>,
    encoding: Option<&str>,
    max_body: usize,
) -> Result<Bytes, NetError> {
    let mut decoder = Decoder::for_encoding(encoding);
    let mut body = resp.into_body();
    let mut out = Vec::new();
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|e| NetError::Other(e.to_string()))?;
        let Ok(data) = frame.into_data() else { continue };
        decoder.push_into(&data, &mut out)?;
        if out.len() > max_body {
            return Err(NetError::TooLarge);
        }
    }
    out.append(&mut decoder.finish()?);
    if out.len() > max_body {
        return Err(NetError::TooLarge);
    }
    Ok(Bytes::from(out))
}

/// Response headers of a static-file answer (`serve_static`, `report_local`).
fn static_headers(content_type: &'static str, len: usize, spa_fallback: bool) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(len));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
    headers.insert(header::ACCESS_CONTROL_EXPOSE_HEADERS, HeaderValue::from_static("*"));
    headers.insert(http::header::HeaderName::from_static("x-pyro-static"), HeaderValue::from_static("1"));
    if spa_fallback {
        headers.insert(
            http::header::HeaderName::from_static("x-pyro-spa-fallback"),
            HeaderValue::from_static("1"),
        );
    }
    headers
}

impl NetClient {
    /// The in-memory static file for `req`, when the embedder may answer it
    /// itself (`NetConfig::static_direct`): GET only, same rules as the I/O
    /// path. The embedder then calls `report_local` so observers see the
    /// request like any other. Interception and the HTTP cache are skipped.
    pub fn static_direct(&self, req: &Request) -> Option<crate::static_files::StaticMemoryHit> {
        let config = &self.inner.shared.config;
        if !config.static_direct || config.static_files.is_empty() || req.method != Method::GET {
            return None;
        }
        if self.inner.shared.settings.lock().unwrap().offline {
            return None;
        }
        config.static_files.lookup(&req.url, req.destination == Destination::Document)
    }

    /// Observe a request handled by a service worker without issuing another network fetch.
    pub fn observe_service_worker_response(
        &self,
        id: RequestId,
        request: &Request,
        mut sink: ResponseSink,
    ) -> ResponseSink {
        let shared = self.inner.shared.clone();
        if !shared.observed() {
            return sink;
        }
        let destination = request.destination;
        let frame_id = request.frame_id.clone();
        let loader_id = request.loader_id.clone();
        shared.emit(NetEvent::RequestWillBeSent {
            id,
            request: Box::new(RequestInfo {
                url: request.url.clone(),
                method: request.method.clone(),
                headers: request.headers.clone(),
                post_data: request.body.clone(),
                destination,
                frame_id: frame_id.clone(),
                loader_id: loader_id.clone(),
            }),
            redirect_response: None,
            timestamp: shared.now(),
            wall_time: wall_time(),
        });
        Box::new(move |event| {
            let timestamp = shared.now();
            match &event {
                ResponseEvent::Response(head) => shared.emit(NetEvent::ResponseReceived {
                    id,
                    head: Box::new(head.clone()),
                    destination,
                    frame_id: frame_id.clone(),
                    loader_id: loader_id.clone(),
                    timestamp,
                }),
                ResponseEvent::Data(data) => shared.emit(NetEvent::DataReceived { id, data: data.clone(), timestamp }),
                ResponseEvent::Finished { encoded_len, .. } => {
                    shared.emit(NetEvent::LoadingFinished { id, encoded_len: *encoded_len, timestamp })
                }
                ResponseEvent::Failed(error) => shared.emit(NetEvent::LoadingFailed {
                    id,
                    error: error.clone(),
                    canceled: matches!(error, NetError::Aborted),
                    destination,
                    timestamp,
                }),
                ResponseEvent::Redirect { .. } => {}
            }
            sink(event);
        })
    }

    /// Publishes the `NetEvent`s of a request the embedder answered from a
    /// `static_direct` hit (on the caller's thread, in order, all at once)
    /// and returns the response head to build the resource from.
    pub fn report_local(&self, req: &Request, hit: &crate::static_files::StaticMemoryHit) -> ResponseHead {
        self.report_local_with_id(req, hit).1
    }

    /// Also returns the request identity so a same-thread loader can report
    /// observation and response-delivery boundaries without waiting for I/O.
    pub fn report_local_with_id(
        &self,
        req: &Request,
        hit: &crate::static_files::StaticMemoryHit,
    ) -> (RequestId, ResponseHead) {
        let shared = &self.inner.shared;
        let id = self.next_request_id();
        let body = &hit.file.body;
        let headers = static_headers(hit.file.content_type, body.len(), hit.spa_fallback);
        let (mime, charset) = parse_content_type(hit.file.content_type);
        let head = ResponseHead {
            url: req.url.clone(),
            status: StatusCode::OK,
            status_text: "OK".into(),
            headers,
            mime,
            charset,
            redirected: false,
            from_cache: false,
            kind: ResponseKind::Basic,
            remote_addr: None,
            protocol: Version::HTTP_11,
            timing: Default::default(),
        };
        if !shared.observed() {
            return (id, head);
        }
        let info = RequestInfo {
            url: req.url.clone(),
            method: req.method.clone(),
            headers: req.headers.clone(),
            post_data: None,
            destination: req.destination,
            frame_id: req.frame_id.clone(),
            loader_id: req.loader_id.clone(),
        };
        let now = shared.now();
        shared.emit(NetEvent::RequestWillBeSent {
            id,
            request: Box::new(info),
            redirect_response: None,
            timestamp: now,
            wall_time: wall_time(),
        });
        shared.emit(NetEvent::ResponseReceived {
            id,
            head: Box::new(head.clone()),
            destination: req.destination,
            frame_id: req.frame_id.clone(),
            loader_id: req.loader_id.clone(),
            timestamp: now,
        });
        if !body.is_empty() {
            shared.emit(NetEvent::DataReceived { id, data: body.clone(), timestamp: now });
        }
        shared.emit(NetEvent::LoadingFinished { id, encoded_len: body.len() as u64, timestamp: now });
        (id, head)
    }
}

fn cdp_reason(r: &str) -> String {
    match r {
        "Aborted" => "net::ERR_ABORTED",
        "TimedOut" => "net::ERR_TIMED_OUT",
        "AccessDenied" => "net::ERR_ACCESS_DENIED",
        "ConnectionClosed" => "net::ERR_CONNECTION_CLOSED",
        "ConnectionReset" => "net::ERR_CONNECTION_RESET",
        "ConnectionRefused" => "net::ERR_CONNECTION_REFUSED",
        "ConnectionAborted" => "net::ERR_CONNECTION_ABORTED",
        "ConnectionFailed" => "net::ERR_CONNECTION_FAILED",
        "NameNotResolved" => "net::ERR_NAME_NOT_RESOLVED",
        "InternetDisconnected" => "net::ERR_INTERNET_DISCONNECTED",
        "AddressUnreachable" => "net::ERR_ADDRESS_UNREACHABLE",
        "BlockedByClient" => "net::ERR_BLOCKED_BY_CLIENT",
        "BlockedByResponse" => "net::ERR_BLOCKED_BY_RESPONSE",
        _ => "net::ERR_FAILED",
    }
    .to_owned()
}

#[cfg(test)]
mod decoder_tests {
    use super::*;

    /// A network chunk leaves the decoder in an allocation of its own, so
    /// what keeps the output does not keep the connection's read buffer.
    #[test]
    fn decoded_output_does_not_share_the_chunk() {
        let read_buffer = Bytes::from(vec![7u8; 4096]);
        let chunk = read_buffer.slice(100..200);
        let out = Decoder::for_encoding(None).push(&chunk).unwrap();
        assert_eq!(out, chunk);
        assert!(!read_buffer.as_ptr_range().contains(&out.as_ptr()));

        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(&[b'a'; 100_000]).unwrap();
        let encoded = gz.finish().unwrap();
        let mut decoder = Decoder::for_encoding(Some("gzip"));
        let mut decoded = Vec::new();
        for part in encoded.chunks(1000) {
            decoded.extend_from_slice(&decoder.push(part).unwrap());
        }
        decoded.extend_from_slice(&decoder.finish().unwrap());
        assert_eq!(decoded, vec![b'a'; 100_000]);
    }

    /// A response's header values leave the connection's read buffer too.
    #[test]
    fn kept_headers_do_not_share_the_read_buffer() {
        let read_buffer = Bytes::from(vec![b'a'; 4096]);
        let mut received = HeaderMap::new();
        received.insert(header::CONTENT_TYPE, HeaderValue::from_maybe_shared(read_buffer.slice(10..19)).unwrap());
        received.append(header::SET_COOKIE, HeaderValue::from_maybe_shared(read_buffer.slice(30..40)).unwrap());
        let mut secret = HeaderValue::from_maybe_shared(read_buffer.slice(50..53)).unwrap();
        secret.set_sensitive(true);
        received.append(header::SET_COOKIE, secret);
        received.insert(header::ETAG, HeaderValue::from_static(""));
        let headers = own_headers(&received);
        assert_eq!(headers, received);
        for value in headers.values() {
            assert!(!read_buffer.as_ptr_range().contains(&value.as_bytes().as_ptr()));
        }
        assert_eq!(headers.get_all(header::SET_COOKIE).iter().map(HeaderValue::is_sensitive).collect::<Vec<_>>(), [false, true]);
    }

    fn decode(enc: &str, body: &[u8], chunk: usize) -> Result<Vec<u8>, NetError> {
        let mut decoder = Decoder::for_encoding(Some(enc));
        let mut out = Vec::new();
        for part in body.chunks(chunk.max(1)) {
            decoder.push_into(part, &mut out)?;
        }
        out.append(&mut decoder.finish()?);
        Ok(out)
    }

    fn zlib(data: &[u8]) -> Vec<u8> {
        let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    fn raw_deflate(data: &[u8]) -> Vec<u8> {
        let mut e = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    fn gzip(data: &[u8]) -> Vec<u8> {
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    /// `deflate` accepts both the zlib-wrapped stream (RFC 9110) and the
    /// raw deflate some servers send, in any chunking.
    #[test]
    fn deflate_accepts_zlib_and_raw_streams() {
        let text = b"hello deflate hello deflate hello deflate".repeat(50);
        for chunk in [1, 2, 7, 4096] {
            assert_eq!(decode("deflate", &zlib(&text), chunk).unwrap(), text);
            assert_eq!(decode("Deflate", &raw_deflate(&text), chunk).unwrap(), text);
        }
        assert_eq!(decode("deflate", b"", 1).unwrap(), b"");
    }

    /// Stacked codings decode in reverse order of the header list.
    #[test]
    fn stacked_codings_decode_in_reverse() {
        let text = b"stacked codings".repeat(100);
        let body = gzip(&zlib(&text)); // deflate applied first, then gzip
        for chunk in [1, 3, 64, 100_000] {
            assert_eq!(decode("deflate, gzip", &body, chunk).unwrap(), text);
            assert_eq!(decode("deflate,gzip", &body, chunk).unwrap(), text);
        }
        assert!(decode("gzip, deflate", &body, 64).is_err());
        // Header lines joined like one list.
        let mut headers = HeaderMap::new();
        headers.append(header::CONTENT_ENCODING, HeaderValue::from_static("deflate"));
        headers.append(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
        assert_eq!(content_encoding(&headers).as_deref(), Some("deflate,gzip"));
        assert_eq!(content_encoding(&HeaderMap::new()), None);
    }

    /// Like Chromium: a coding not supported (or `identity`) anywhere in
    /// the list passes the body through undecoded instead of failing.
    #[test]
    fn unknown_codings_pass_through() {
        let gz = gzip(b"abc");
        for enc in ["UTF-8", "none", "text", "zstd", "gzip, bogus", "identity", "identity, gzip", "", " , "] {
            assert_eq!(decode(enc, &gz, 5).unwrap(), gz, "{enc}");
        }
        assert_eq!(decode(" GZIP ", &gz, 5).unwrap(), b"abc");
        assert_eq!(decode("x-gzip", &gz, 5).unwrap(), b"abc");
    }
}

/// pyro: whether a hop includes credentials (Fetch § 4.4 "includes credentials"): its credentials
/// mode is "include", or "same-origin" and its response tainting is "basic" (`basic_tainting`:
/// same-origin, and no no-cors hop before it went to another origin). The ONE decision for the
/// cookies sent, the `Set-Cookie` stored and the cache entry's kind.
pub(crate) fn includes_credentials(credentials: Credentials, basic_tainting: bool) -> bool {
    match credentials {
        Credentials::Include => true,
        Credentials::SameOrigin => basic_tainting,
        Credentials::Omit => false,
    }
}
