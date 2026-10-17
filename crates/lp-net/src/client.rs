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

use std::collections::HashMap;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use http_body_util::{BodyExt, Full};
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use tokio::sync::{mpsc, oneshot, Semaphore};
use url::Url;

use crate::cookies::{site_context, CookieJar, SiteContext};
use crate::cors::{self, PreflightCache};
use crate::request::*;

/// Consumer of one request's events. Called on the I/O thread; wrap it to
/// post to the browser loop. Receives exactly one terminal event.
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
}

impl Default for NetConfig {
    fn default() -> Self {
        NetConfig {
            user_agent: format!("Mozilla/5.0 (compatible; lightningpanda/{})", env!("CARGO_PKG_VERSION")),
            accept_language: "en-US,en;q=0.9".into(),
            max_redirects: 20,
            connect_timeout: Duration::from_secs(10),
            max_body: 256 * 1024 * 1024,
            enforce_cors: true,
            max_per_origin: 6,
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
}

/// A request paused for CDP Fetch interception (request stage).
#[derive(Clone, Debug)]
pub struct PausedRequest {
    pub id: RequestId,
    pub request: RequestInfo,
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
}

/// Fetch-domain interception. `paused` must not block; answer through
/// `reply` whenever the CDP decision arrives. Dropping `reply` fails the
/// request with net::ERR_FAILED.
pub trait Interceptor: Send + Sync {
    fn paused(&self, req: PausedRequest, reply: oneshot::Sender<InterceptDecision>);
}

enum Cmd {
    Fetch(RequestId, Box<Request>, ResponseSink),
    Cancel(RequestId),
}

struct Inner {
    tx: mpsc::UnboundedSender<Cmd>,
    next_id: AtomicU64,
    cookies: Arc<Mutex<CookieJar>>,
    shared: Arc<Shared>,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}

struct Shared {
    config: NetConfig,
    settings: Mutex<NetSettings>,
    observers: Mutex<Vec<mpsc::UnboundedSender<NetEvent>>>,
    interceptor: Mutex<Option<Arc<dyn Interceptor>>>,
    epoch: Instant,
}

impl Shared {
    fn now(&self) -> f64 {
        self.epoch.elapsed().as_secs_f64()
    }
    fn emit(&self, ev: NetEvent) {
        let mut obs = self.observers.lock().unwrap();
        if obs.is_empty() {
            return;
        }
        obs.retain(|tx| tx.send(ev.clone()).is_ok());
    }
    fn observed(&self) -> bool {
        !self.observers.lock().unwrap().is_empty()
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        // Closing the channel stops the I/O loop; in-flight requests are
        // dropped (their sinks receive nothing further).
        let (dead, _) = mpsc::unbounded_channel();
        self.tx = dead;
        if let Some(t) = self.thread.lock().unwrap().take() {
            let _ = t.join();
        }
    }
}

/// Handle to one network context (cookie jar + connection pool + I/O
/// thread). Cheap to clone and `Send`; one per browser context.
#[derive(Clone)]
pub struct NetClient {
    inner: Arc<Inner>,
}

impl NetClient {
    pub fn new(config: NetConfig) -> std::io::Result<Self> {
        let (tx, rx) = mpsc::unbounded_channel();
        let shared = Arc::new(Shared {
            config,
            settings: Mutex::new(NetSettings::default()),
            observers: Mutex::new(Vec::new()),
            interceptor: Mutex::new(None),
            epoch: Instant::now(),
        });
        let cookies = Arc::new(Mutex::new(CookieJar::new()));
        let (s2, c2) = (shared.clone(), cookies.clone());
        let thread = std::thread::Builder::new().name("lp-net-io".into()).spawn(move || io_main(rx, s2, c2))?;
        Ok(NetClient {
            inner: Arc::new(Inner {
                tx,
                next_id: AtomicU64::new(1),
                cookies,
                shared,
                thread: Mutex::new(Some(thread)),
            }),
        })
    }

    pub fn cookies(&self) -> &Arc<Mutex<CookieJar>> {
        &self.inner.cookies
    }
    pub fn config(&self) -> &NetConfig {
        &self.inner.shared.config
    }
    pub fn update_settings(&self, f: impl FnOnce(&mut NetSettings)) {
        f(&mut self.inner.shared.settings.lock().unwrap());
    }
    /// Subscribes to Network events. Unbounded: events are small; bodies
    /// are refcounted `Bytes` and retention policy belongs to the consumer.
    pub fn subscribe(&self) -> mpsc::UnboundedReceiver<NetEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.inner.shared.observers.lock().unwrap().push(tx);
        rx
    }
    pub fn set_interceptor(&self, i: Option<Arc<dyn Interceptor>>) {
        *self.inner.shared.interceptor.lock().unwrap() = i;
    }

    pub fn next_request_id(&self) -> RequestId {
        RequestId(self.inner.next_id.fetch_add(1, Ordering::Relaxed))
    }

    /// Starts a request. `data:` URLs are answered on the I/O thread
    /// without network access so event ordering stays uniform.
    pub fn fetch(&self, req: Request, sink: ResponseSink) -> RequestId {
        let id = self.next_request_id();
        self.fetch_with_id(id, req, sink);
        id
    }

    pub fn fetch_with_id(&self, id: RequestId, req: Request, sink: ResponseSink) {
        if let Err(mpsc::error::SendError(Cmd::Fetch(_, _, mut sink))) =
            self.inner.tx.send(Cmd::Fetch(id, Box::new(req), sink))
        {
            sink(ResponseEvent::Failed(NetError::Other("network thread stopped".into())));
        }
    }

    /// Aborts a request; its sink receives `Failed(Aborted)` unless it
    /// already finished.
    pub fn cancel(&self, id: RequestId) {
        let _ = self.inner.tx.send(Cmd::Cancel(id));
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

// ---------------------------------------------------------------------------
// I/O thread

type HttpClient = Client<HttpConnector, Full<Bytes>>;

struct Io {
    shared: Arc<Shared>,
    cookies: Arc<Mutex<CookieJar>>,
    client: HttpClient,
    preflight: Mutex<PreflightCache>,
    per_origin: Mutex<HashMap<String, Arc<Semaphore>>>,
}

fn io_main(mut rx: mpsc::UnboundedReceiver<Cmd>, shared: Arc<Shared>, cookies: Arc<Mutex<CookieJar>>) {
    let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(_) => return,
    };
    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, async move {
        let mut connector = HttpConnector::new();
        connector.set_connect_timeout(Some(shared.config.connect_timeout));
        connector.set_nodelay(true);
        connector.enforce_http(false);
        let client = Client::builder(TokioExecutor::new()).pool_idle_timeout(Duration::from_secs(30)).build(connector);
        let io = std::rc::Rc::new(Io {
            shared,
            cookies,
            client,
            preflight: Mutex::new(PreflightCache::default()),
            per_origin: Mutex::new(HashMap::new()),
        });
        let tasks: std::rc::Rc<std::cell::RefCell<HashMap<RequestId, Task>>> = Default::default();
        while let Some(cmd) = rx.recv().await {
            match cmd {
                Cmd::Fetch(id, req, sink) => {
                    let sink = std::rc::Rc::new(std::cell::RefCell::new(Some(sink)));
                    let (io2, tasks2, sink2) = (io.clone(), tasks.clone(), sink.clone());
                    let handle = tokio::task::spawn_local(async move {
                        let dest = req.destination;
                        let result = io2.run(id, *req, &sink2).await;
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
                    });
                    tasks.borrow_mut().insert(id, Task { abort: handle.abort_handle(), sink });
                }
                Cmd::Cancel(id) => {
                    if let Some(t) = tasks.borrow_mut().remove(&id) {
                        t.abort.abort();
                        io.shared.emit(NetEvent::LoadingFailed {
                            id,
                            error: NetError::Aborted,
                            canceled: true,
                            destination: Destination::Other,
                            timestamp: io.shared.now(),
                        });
                        deliver(&t.sink, ResponseEvent::Failed(NetError::Aborted));
                        t.sink.borrow_mut().take();
                    }
                }
            }
        }
        for (_, t) in tasks.borrow_mut().drain() {
            t.abort.abort();
        }
    });
}

struct Task {
    abort: tokio::task::AbortHandle,
    sink: SharedSink,
}

type SharedSink = std::rc::Rc<std::cell::RefCell<Option<ResponseSink>>>;

fn deliver(sink: &SharedSink, ev: ResponseEvent) {
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
        Destination::Image => "image/avif,image/webp,image/apng,image/svg+xml,image/*,*/*;q=0.8",
        Destination::EventSource => "text/event-stream",
        _ => "*/*",
    }
}

enum Decoder {
    Identity,
    Gzip(flate2::write::GzDecoder<Vec<u8>>),
    Deflate(flate2::write::ZlibDecoder<Vec<u8>>),
    Brotli(Box<brotli_decompressor::DecompressorWriter<Vec<u8>>>),
}

impl Decoder {
    fn for_encoding(enc: Option<&str>) -> Result<Self, NetError> {
        Ok(match enc.map(|e| e.trim().to_ascii_lowercase()).as_deref() {
            None | Some("") | Some("identity") => Decoder::Identity,
            Some("gzip") | Some("x-gzip") => Decoder::Gzip(flate2::write::GzDecoder::new(Vec::new())),
            Some("deflate") => Decoder::Deflate(flate2::write::ZlibDecoder::new(Vec::new())),
            Some("br") => Decoder::Brotli(Box::new(brotli_decompressor::DecompressorWriter::new(Vec::new(), 4096))),
            Some(other) => return Err(NetError::Decode(format!("unsupported content-encoding {other}"))),
        })
    }
    /// Feeds an encoded chunk, returning any decoded output.
    fn push(&mut self, chunk: Bytes) -> Result<Bytes, NetError> {
        let err = |e: std::io::Error| NetError::Decode(e.to_string());
        let out = match self {
            Decoder::Identity => return Ok(chunk),
            Decoder::Gzip(d) => {
                d.write_all(&chunk).map_err(err)?;
                std::mem::take(d.get_mut())
            }
            Decoder::Deflate(d) => {
                d.write_all(&chunk).map_err(err)?;
                std::mem::take(d.get_mut())
            }
            Decoder::Brotli(d) => {
                d.write_all(&chunk).map_err(err)?;
                std::mem::take(d.get_mut())
            }
        };
        Ok(Bytes::from(out))
    }
    fn finish(self) -> Result<Bytes, NetError> {
        let err = |e: std::io::Error| NetError::Decode(e.to_string());
        Ok(Bytes::from(match self {
            Decoder::Identity => Vec::new(),
            Decoder::Gzip(d) => d.finish().map_err(err)?,
            Decoder::Deflate(d) => d.finish().map_err(err)?,
            Decoder::Brotli(d) => d.into_inner().map_err(|_| NetError::Decode("truncated brotli stream".into()))?,
        }))
    }
}

fn classify(e: &hyper_util::client::legacy::Error) -> NetError {
    let mut src: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(e);
    let mut text = e.to_string();
    while let Some(s) = src {
        if let Some(io) = s.downcast_ref::<std::io::Error>() {
            match io.kind() {
                std::io::ErrorKind::ConnectionRefused => return NetError::ConnectionRefused,
                std::io::ErrorKind::TimedOut => return NetError::Timeout,
                _ => {}
            }
        }
        text = format!("{text}: {s}");
        src = s.source();
    }
    if text.contains("dns error") || text.contains("failed to lookup") {
        return NetError::NameNotResolved;
    }
    if e.is_connect() {
        return NetError::ConnectionRefused;
    }
    NetError::Other(text)
}

impl Io {
    fn origin_semaphore(&self, url: &Url) -> Arc<Semaphore> {
        let key = url.origin().ascii_serialization();
        let mut m = self.per_origin.lock().unwrap();
        m.entry(key).or_insert_with(|| Arc::new(Semaphore::new(self.shared.config.max_per_origin))).clone()
    }

    fn build_headers(&self, req: &Request, origin_url: Option<&Url>, cors_cross: bool) -> HeaderMap {
        let settings = self.shared.settings.lock().unwrap().clone();
        let cfg = &self.shared.config;
        let mut h = req.headers.clone();
        let ua = settings.user_agent.as_deref().unwrap_or(&cfg.user_agent);
        if let Ok(v) = HeaderValue::from_str(ua) {
            h.entry(header::USER_AGENT).or_insert(v);
        }
        h.entry(header::ACCEPT).or_insert(HeaderValue::from_static(default_accept(req.destination)));
        let lang = settings.accept_language.as_deref().unwrap_or(&cfg.accept_language);
        if let Ok(v) = HeaderValue::from_str(lang) {
            h.entry(header::ACCEPT_LANGUAGE).or_insert(v);
        }
        h.insert(header::ACCEPT_ENCODING, HeaderValue::from_static("gzip, deflate, br"));
        if let Some(origin) = &req.origin {
            if cors_cross || !matches!(req.method, Method::GET | Method::HEAD) {
                if let Ok(v) = HeaderValue::from_str(origin) {
                    h.insert(header::ORIGIN, v);
                }
            }
        }
        if let Some(r) = &req.referrer {
            let mut r = r.clone();
            r.set_fragment(None);
            let _ = r.set_username("");
            let _ = r.set_password(None);
            // strict-origin-when-cross-origin.
            let value = if origin_url.is_some_and(|o| !cors::same_origin(o, &req.url)) || r.origin() != req.url.origin()
            {
                r.origin().ascii_serialization() + "/"
            } else {
                r.to_string()
            };
            if let Ok(v) = HeaderValue::from_str(&value) {
                h.entry(header::REFERER).or_insert(v);
            }
        }
        // Cookies.
        let send_cookies = match req.credentials {
            Credentials::Include => true,
            Credentials::SameOrigin => !cors_cross,
            Credentials::Omit => false,
        };
        if send_cookies && !h.contains_key(header::COOKIE) {
            let top = matches!(req.destination, Destination::Document);
            let safe = matches!(req.method, Method::GET | Method::HEAD);
            let ctx = if req.mode == RequestMode::Navigate && req.origin.is_none() {
                SiteContext::SameSite
            } else {
                site_context(origin_url, &req.url, top, safe)
            };
            let cookie = self.cookies.lock().unwrap().header_for(&req.url, ctx, crate::cookies::now_secs());
            if let Some(c) = cookie.and_then(|c| HeaderValue::from_str(&c).ok()) {
                h.insert(header::COOKIE, c);
            }
        }
        for (k, v) in settings.extra_headers.iter() {
            h.insert(k.clone(), v.clone());
        }
        h
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

    async fn send(
        &self,
        method: Method,
        url: &Url,
        headers: HeaderMap,
        body: Option<Bytes>,
    ) -> Result<http::Response<hyper::body::Incoming>, NetError> {
        let mut b = http::Request::builder().method(method).uri(url.as_str());
        if let Some(h) = b.headers_mut() {
            *h = headers;
        }
        let r = b.body(Full::new(body.unwrap_or_default())).map_err(|_| NetError::InvalidUrl)?;
        self.client.request(r).await.map_err(|e| classify(&e))
    }

    async fn run(&self, id: RequestId, mut req: Request, sink: &SharedSink) -> Result<(), NetError> {
        let origin_url = req.origin.as_deref().and_then(|o| Url::parse(o).ok());
        let max_body = req.max_body.unwrap_or(self.shared.config.max_body);
        let mut redirects = 0u32;
        let mut redirect_response: Option<ResponseHead> = None;
        let mut tainted = false; // cross-origin redirect in cors mode => Origin "null"
        loop {
            if req.url.scheme() == "data" {
                return self.serve_data(id, &req, sink, redirect_response.take());
            }
            match req.url.scheme() {
                "http" => {}
                "https" => return Err(NetError::UnsupportedScheme("https (built without TLS support)".into())),
                s => return Err(NetError::UnsupportedScheme(s.to_owned())),
            }
            if self.shared.settings.lock().unwrap().offline {
                return Err(NetError::Blocked("net::ERR_INTERNET_DISCONNECTED".into()));
            }
            let cors_cross = self.shared.config.enforce_cors && cors::is_cors_request(&req, origin_url.as_ref());
            if tainted {
                req.origin = Some("null".into());
            }
            let headers = self.build_headers(&req, origin_url.as_ref(), cors_cross);
            let info = RequestInfo {
                url: req.url.clone(),
                method: req.method.clone(),
                headers: headers.clone(),
                post_data: req.body.clone(),
                destination: req.destination,
                frame_id: req.frame_id.clone(),
                loader_id: req.loader_id.clone(),
            };
            self.shared.emit(NetEvent::RequestWillBeSent {
                id,
                request: Box::new(info.clone()),
                redirect_response: redirect_response.take().map(Box::new),
                timestamp: self.shared.now(),
                wall_time: wall_time(),
            });

            // CDP Fetch interception (request stage).
            let interceptor = self.shared.interceptor.lock().unwrap().clone();
            let (mut method, mut url, mut headers, mut body) =
                (req.method.clone(), req.url.clone(), headers, req.body.clone());
            if let Some(i) = interceptor {
                let (tx, rx) = oneshot::channel();
                i.paused(PausedRequest { id, request: info }, tx);
                match rx.await {
                    Ok(InterceptDecision::Continue { url: u, method: m, headers: h, body: b }) => {
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
                    Ok(InterceptDecision::Fulfill { status, headers: h, body: b }) => {
                        return self.fulfill(id, &req, sink, status, h, b);
                    }
                    Ok(InterceptDecision::Fail(reason)) => return Err(NetError::Blocked(cdp_reason(&reason))),
                    Err(_) => return Err(NetError::Other("interception dropped".into())),
                }
            }

            // CORS preflight.
            if cors_cross && cors::needs_preflight(&req) {
                let origin = req.origin.clone().unwrap_or_else(|| "null".into());
                let covered = self.preflight.lock().unwrap().covers(&req, &origin);
                if !covered {
                    let ph = cors::preflight_request_headers(&req, &origin);
                    let _permit = self.origin_semaphore(&url).acquire_owned().await;
                    let resp = self.send(Method::OPTIONS, &url, ph, None).await?;
                    let status = resp.status().as_u16();
                    let rh = resp.headers().clone();
                    drop(resp.into_body().collect().await);
                    self.preflight.lock().unwrap().record(status, &rh, &req, &origin).map_err(NetError::Cors)?;
                }
            }

            let permit = self.origin_semaphore(&url).acquire_owned().await;
            let resp = self.send(method.clone(), &url, headers, body).await?;
            let status = resp.status();
            let resp_headers = resp.headers().clone();
            let cookies_allowed = match req.credentials {
                Credentials::Include => true,
                Credentials::SameOrigin => !cors_cross,
                Credentials::Omit => false,
            };
            self.store_cookies(&url, &resp_headers, cookies_allowed);

            // Redirects.
            if status.is_redirection() && status != StatusCode::NOT_MODIFIED {
                if let Some(loc) = resp_headers.get(header::LOCATION).and_then(|v| v.to_str().ok()) {
                    let target = url.join(loc).map_err(|_| NetError::InvalidUrl)?;
                    drop(resp.into_body().collect().await);
                    drop(permit);
                    match req.redirect {
                        RedirectMode::Error => return Err(NetError::RedirectNotAllowed),
                        RedirectMode::Manual => {
                            let head =
                                self.head(&url, status, resp_headers, ResponseKind::OpaqueRedirect, redirects > 0);
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
                    let head = self.head(&url, status, resp_headers.clone(), ResponseKind::Basic, redirects > 1);
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
                    if cors_cross && !cors::same_origin(&url, &target) {
                        tainted = true;
                    }
                    req.url = target;
                    redirect_response = Some(head);
                    continue;
                }
            }

            let kind = if cors_cross {
                cors::cors_check(&resp_headers, req.origin.as_deref().unwrap_or("null"), req.credentials)
                    .map_err(NetError::Cors)?;
                ResponseKind::Cors
            } else if req.mode == RequestMode::NoCors
                && origin_url.as_ref().is_some_and(|o| !cors::same_origin(o, &url))
            {
                ResponseKind::Opaque
            } else {
                ResponseKind::Basic
            };
            let encoding = resp_headers.get(header::CONTENT_ENCODING).and_then(|v| v.to_str().ok()).map(str::to_owned);
            let head = self.head(&url, status, resp_headers, kind, redirects > 0);
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
            let mut decoder = Decoder::for_encoding(encoding.as_deref())?;
            let mut body = resp.into_body();
            let (mut encoded, mut decoded) = (0u64, 0u64);
            let observed = self.shared.observed();
            while let Some(frame) = body.frame().await {
                let frame = frame.map_err(|e| NetError::Other(e.to_string()))?;
                let Ok(data) = frame.into_data() else { continue };
                encoded += data.len() as u64;
                let out = decoder.push(data)?;
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
                    deliver(sink, ResponseEvent::Data(out));
                }
            }
            let tail = decoder.finish()?;
            if !tail.is_empty() {
                decoded += tail.len() as u64;
                if decoded as usize > max_body {
                    return Err(NetError::TooLarge);
                }
                if observed {
                    self.shared.emit(NetEvent::DataReceived { id, data: tail.clone(), timestamp: self.shared.now() });
                }
                deliver(sink, ResponseEvent::Data(tail));
            }
            drop(permit);
            self.shared.emit(NetEvent::LoadingFinished { id, encoded_len: encoded, timestamp: self.shared.now() });
            deliver(sink, ResponseEvent::Finished { encoded_len: encoded, decoded_len: decoded });
            return Ok(());
        }
    }

    fn head(
        &self,
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
        }
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
        self.shared.emit(NetEvent::ResponseReceived {
            id,
            head: Box::new(head.clone()),
            destination: req.destination,
            frame_id: req.frame_id.clone(),
            loader_id: req.loader_id.clone(),
            timestamp: self.shared.now(),
        });
        deliver(sink, ResponseEvent::Response(head));
        let len = body.len() as u64;
        if !body.is_empty() {
            if self.shared.observed() {
                self.shared.emit(NetEvent::DataReceived { id, data: body.clone(), timestamp: self.shared.now() });
            }
            deliver(sink, ResponseEvent::Data(body));
        }
        self.shared.emit(NetEvent::LoadingFinished { id, encoded_len: len, timestamp: self.shared.now() });
        deliver(sink, ResponseEvent::Finished { encoded_len: len, decoded_len: len });
        Ok(())
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
        let status = StatusCode::from_u16(status).map_err(|_| NetError::Other("invalid status".into()))?;
        let head = self.head(&req.url, status, headers, ResponseKind::Basic, false);
        self.deliver_body(id, req, sink, head, body)
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
        self.deliver_body(id, req, sink, head, body)
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
