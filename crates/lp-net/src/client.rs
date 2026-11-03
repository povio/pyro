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

use crate::auth::auth_challenge;
use crate::cookies::{site_context, CookieJar, SiteContext};
use crate::cors::{self, PreflightCache};
use crate::request::*;
use crate::subscription::{NetSubscription, NetWake, SubscriptionLimits, Subscriptions};

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
    /// Maximum credential submissions for one HTTP hop. No unbounded auth loops.
    pub max_auth_retries: u32,
    /// Private in-memory HTTP cache budget for this client (one browser
    /// context), in logical bytes. 0 disables the cache.
    pub http_cache_max_bytes: usize,
    /// Largest single response body the HTTP cache stores.
    pub http_cache_max_entry_bytes: usize,
    /// Opt-in cross-context cache of public static assets (one handle per
    /// browser process). `None` (the default unless `LIGHTNINGPANDA_SHARED_ASSET_CACHE_MB`
    /// is set) keeps every context fully isolated.
    pub shared_asset_cache: Option<crate::cache::SharedAssetCache>,
    /// Directories answering GET/HEAD requests for whole origins from disk
    /// (`serve --static ORIGIN=DIR`); empty by default.
    pub static_files: crate::static_files::StaticFiles,
    /// Let embedders answer `static_files` hits on their own thread from the
    /// preloaded bytes (`NetClient::report_local`) instead of sending the
    /// request through the I/O thread. Default true; `LIGHTNINGPANDA_STATIC_DIRECT=0`
    /// turns it off (benchmark comparison).
    pub static_direct: bool,
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
            max_auth_retries: 3,
            http_cache_max_bytes: 64 * 1024 * 1024,
            http_cache_max_entry_bytes: 8 * 1024 * 1024,
            shared_asset_cache: crate::cache::SharedAssetCache::from_env(),
            static_files: Default::default(),
            static_direct: std::env::var("LIGHTNINGPANDA_STATIC_DIRECT").map_or(true, |v| v != "0"),
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

/// Server Basic credentials use UTF-8. Other schemes and proxy credentials
/// return explicit errors; this build has no proxy transport or auth cache.
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
    fn auth_required(&self, _challenge: AuthChallenge, reply: oneshot::Sender<AuthDecision>) {
        let _ = reply.send(AuthDecision::Default);
    }
}

enum Cmd {
    Fetch(RequestId, Box<Request>, ResponseSink),
    Cancel(RequestId),
}

struct Inner {
    /// Replaced when the I/O thread restarts after a fork ([`suspend_for_fork`]).
    tx: Mutex<mpsc::UnboundedSender<Cmd>>,
    next_id: AtomicU64,
    cookies: Arc<Mutex<CookieJar>>,
    shared: Arc<Shared>,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
    activity: Arc<Mutex<ActivityMap>>,
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
    loaders: HashMap<String, (u64, NetActivity)>,
    generation: u64,
}

/// Decrements its loader's activity exactly once, when lp-net drops the
/// request's sink after the terminal event (finished, failed, canceled, aborted
/// or shutdown). A forgotten loader's late terminals do not touch a newer entry.
struct ActivityGuard {
    event_stream: bool,
    map: Arc<Mutex<ActivityMap>>,
    loader: String,
    generation: u64,
}

impl Drop for ActivityGuard {
    fn drop(&mut self) {
        let mut map = self.map.lock().unwrap();
        if let Some((generation, a)) = map.loaders.get_mut(&self.loader) {
            if *generation == self.generation {
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
}

impl Drop for Inner {
    fn drop(&mut self) {
        // Closing the channel stops the I/O loop, which fails in-flight
        // requests with `Aborted` before exiting.
        let (dead, _) = mpsc::unbounded_channel();
        *self.tx.get_mut().unwrap_or_else(|e| e.into_inner()) = dead;
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
        let cache = crate::cache::HttpCache::new(config.http_cache_max_bytes, config.http_cache_max_entry_bytes);
        let shared = Arc::new(Shared {
            suspending: std::sync::atomic::AtomicBool::new(false),
            config,
            settings: Mutex::new(NetSettings::default()),
            observers: Subscriptions::default(),
            interceptor: Mutex::new(None),
            cache: Mutex::new(cache),
            epoch: Instant::now(),
            ignore_certificate_errors: std::sync::atomic::AtomicBool::new(false),
        });
        let cookies = Arc::new(Mutex::new(CookieJar::new()));
        let (s2, c2) = (shared.clone(), cookies.clone());
        let thread = std::thread::Builder::new().name("lp-net-io".into()).spawn(move || io_main(rx, s2, c2))?;
        let inner = Arc::new(Inner {
            tx: Mutex::new(tx),
            next_id: AtomicU64::new(1),
            cookies,
            shared,
            thread: Mutex::new(Some(thread)),
            activity: Arc::default(),
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

    /// Starts a request. `data:` URLs are answered on the I/O thread
    /// without network access so event ordering stays uniform.
    pub fn fetch(&self, req: Request, sink: ResponseSink) -> RequestId {
        let id = self.next_request_id();
        self.fetch_with_id(id, req, sink);
        id
    }

    pub fn fetch_with_id(&self, id: RequestId, req: Request, sink: ResponseSink) {
        let sink = match req.loader_id.clone() {
            Some(loader) => self.track(loader, sink),
            None => sink,
        };
        if let Err(mpsc::error::SendError(Cmd::Fetch(_, _, mut sink))) =
            self.inner.tx.lock().unwrap_or_else(|e| e.into_inner()).send(Cmd::Fetch(id, Box::new(req), sink))
        {
            sink(ResponseEvent::Failed(NetError::Other("network thread stopped".into())));
        }
    }

    /// Counts a started request for `loader` (synchronously, on the caller's
    /// thread) and ties the matching decrement to the sink's lifetime.
    fn track(&self, loader: String, mut sink: ResponseSink) -> ResponseSink {
        let generation = {
            let mut map = self.inner.activity.lock().unwrap();
            let next = map.generation + 1;
            let (generation, a) = map.loaders.entry(loader.clone()).or_insert_with(|| {
                (next, NetActivity { inflight: 0, event_streams: 0, idle_since: None, almost_idle_since: None })
            });
            let generation = *generation;
            a.inflight += 1;
            a.idle_since = None;
            if a.inflight > 2 {
                a.almost_idle_since = None;
            }
            if generation == next {
                map.generation = next;
            }
            generation
        };
        let mut guard = ActivityGuard { event_stream: false, map: self.inner.activity.clone(), loader, generation };
        Box::new(move |ev| {
            if !guard.event_stream
                && matches!(&ev, ResponseEvent::Response(head) if head.status.is_success() && head.mime == "text/event-stream")
            {
                let mut map = guard.map.lock().unwrap();
                if let Some((generation, activity)) = map.loaders.get_mut(&guard.loader) {
                    if *generation == guard.generation {
                        activity.event_streams += 1;
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
        self.inner.activity.lock().unwrap().loaders.get(loader_id).map(|(_, a)| *a)
    }

    /// Drops a loader's activity entry (e.g. on navigation or page close).
    /// Requests still in flight for it no longer count anywhere.
    pub fn forget_activity(&self, loader_id: &str) {
        self.inner.activity.lock().unwrap().loaders.remove(loader_id);
    }

    /// Aborts a request; its sink receives `Failed(Aborted)` unless it
    /// already finished.
    pub fn cancel(&self, id: RequestId) {
        let _ = self.inner.tx.lock().unwrap_or_else(|e| e.into_inner()).send(Cmd::Cancel(id));
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

type HttpClient = Client<hyper_rustls::HttpsConnector<HttpConnector>, Full<Bytes>>;

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

/// Accepts every server certificate; handshake signatures are still verified
/// with the provider's algorithms.
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
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// An HTTP(S) client over `tls`, with this context's connect settings.
fn http_client(config: &NetConfig, tls: &rustls::ClientConfig) -> HttpClient {
    let mut connector = HttpConnector::new();
    connector.set_connect_timeout(Some(config.connect_timeout));
    connector.set_nodelay(true);
    connector.enforce_http(false);
    let connector = hyper_rustls::HttpsConnectorBuilder::new()
        .with_tls_config(tls.clone())
        .https_or_http()
        .enable_http1()
        .wrap_connector(connector);
    Client::builder(TokioExecutor::new()).pool_idle_timeout(Duration::from_secs(30)).build(connector)
}

/// Every client of the process, for [`suspend_for_fork`].
static CLIENTS: Mutex<Vec<std::sync::Weak<Inner>>> = Mutex::new(Vec::new());

/// Stops the I/O thread of every client in the process, so that the process
/// can `fork()` (only the calling thread survives a fork). Requests still in
/// flight fail with `Aborted`; cookies, cache and observers are kept. Call
/// [`resume_after_fork`] afterwards, in the parent and in the child.
pub fn suspend_for_fork() {
    let clients: Vec<Arc<Inner>> =
        CLIENTS.lock().unwrap_or_else(|e| e.into_inner()).iter().filter_map(|c| c.upgrade()).collect();
    for inner in clients {
        inner.shared.suspending.store(true, Ordering::Relaxed);
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
        *thread = Some(std::thread::Builder::new().name("lp-net-io".into()).spawn(move || io_main(rx, s2, c2))?);
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
    preflight: Mutex<PreflightCache>,
    per_origin: Mutex<HashMap<String, Arc<Semaphore>>>,
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
    let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(_) => return,
    };
    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, async move {
        let client = http_client(&shared.config, &tls_config());
        let io = std::rc::Rc::new(Io {
            shared,
            cookies,
            client,
            insecure: std::cell::OnceCell::new(),
            preflight: Mutex::new(PreflightCache::default()),
            per_origin: Mutex::new(HashMap::new()),
        });
        let tasks: std::rc::Rc<std::cell::RefCell<HashMap<RequestId, Task>>> = Default::default();
        while let Some(cmd) = rx.recv().await {
            match cmd {
                Cmd::Fetch(id, req, sink) => {
                    let destination = req.destination;
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
                    tasks.borrow_mut().insert(id, Task { handle, sink, destination });
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
    });
}

struct Task {
    destination: Destination,
    handle: tokio::task::JoinHandle<()>,
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

/// The rustls certificate error inside `io` (hyper-rustls wraps the TLS
/// stream's `io::Error` in another one).
fn certificate_error(io: &std::io::Error) -> Option<String> {
    let inner = io.get_ref()?;
    if let Some(rustls::Error::InvalidCertificate(c)) = inner.downcast_ref::<rustls::Error>() {
        return Some(format!("{c:?}"));
    }
    certificate_error(inner.downcast_ref::<std::io::Error>()?)
}

fn classify(e: &hyper_util::client::legacy::Error) -> NetError {
    let mut src: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(e);
    let mut text = e.to_string();
    while let Some(s) = src {
        if let Some(io) = s.downcast_ref::<std::io::Error>() {
            if let Some(c) = certificate_error(io) {
                return NetError::Certificate(c);
            }
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
        if let Some(v) = req.defaults.user_agent.clone().or_else(|| HeaderValue::from_str(ua).ok()) {
            h.entry(header::USER_AGENT).or_insert(v);
        }
        h.entry(header::ACCEPT).or_insert(HeaderValue::from_static(default_accept(req.destination)));
        let lang = settings.accept_language.as_deref().unwrap_or(&cfg.accept_language);
        if let Some(v) = req.defaults.accept_language.clone().or_else(|| HeaderValue::from_str(lang).ok()) {
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
            Credentials::SameOrigin => cors::is_same_origin_request(req),
            Credentials::Omit => false,
        };
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
            let cookie = self.cookies.lock().unwrap().header_for(&req.url, ctx, crate::cookies::now_secs());
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
        let client = if self.shared.ignore_certificate_errors.load(Ordering::Relaxed) {
            self.insecure.get_or_init(|| http_client(&self.shared.config, &tls_config_insecure()))
        } else {
            &self.client
        };
        client.request(r).await.map_err(|e| classify(&e))
    }

    async fn run(&self, id: RequestId, mut req: Request, sink: &SharedSink) -> Result<(), NetError> {
        let origin_url = req.origin.as_deref().and_then(|o| Url::parse(o).ok());
        let max_body = req.max_body.unwrap_or(self.shared.config.max_body);
        let mut redirects = 0u32;
        let mut redirect_response: Option<ResponseHead> = None;
        let mut tainted = false; // cross-origin redirect in cors mode => Origin "null"
        let mut unconditional = false; // a 304 no longer matched its entry: refetch without validators
        let mut pause_response: Option<RequestInfo> = None; // set by `PauseResponse`: the final hop's response is paused
        loop {
            if tainted {
                req.origin = Some("null".into());
            }
            let cors_cross = self.shared.config.enforce_cors && cors::is_cors_request(&req, origin_url.as_ref());
            let mut headers = self.build_headers(&req, origin_url.as_ref(), cors_cross);
            let cache_mode = self.cache_mode(&req, &mut headers);
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

            // Observers must see a start before every terminal/response path,
            // including local data URLs and requests rejected before I/O.
            if req.url.scheme() == "data" {
                return self.serve_data(id, &req, sink, None);
            }
            match req.url.scheme() {
                "http" | "https" => {}
                s => return Err(NetError::UnsupportedScheme(s.to_owned())),
            }
            if self.shared.settings.lock().unwrap().offline {
                return Err(NetError::Blocked("net::ERR_INTERNET_DISCONNECTED".into()));
            }
            if req.mode == RequestMode::SameOrigin && !cors::is_same_origin_request(&req) {
                return Err(NetError::Cors("Cross-origin request in same-origin mode".into()));
            }

            // CDP Fetch interception (request stage).
            let interceptor = self.shared.interceptor.lock().unwrap().clone();
            let (mut method, mut url, mut headers, mut body) =
                (req.method.clone(), req.url.clone(), headers, req.body.clone());
            if let Some(i) = &interceptor {
                let (tx, rx) = oneshot::channel();
                let paused_info = info.clone();
                i.paused(PausedRequest { id, request: info }, tx);
                match rx.await {
                    Ok(InterceptDecision::PauseResponse) => {
                        pause_response.get_or_insert(paused_info);
                    }
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

            // Static files for an origin served from disk: after interception
            // (routes can still override) and before the cache and network.
            if (method == Method::GET || method == Method::HEAD) && !self.shared.config.static_files.is_empty() {
                let document = req.destination == Destination::Document;
                if let Some(hit) = self.shared.config.static_files.resolve(&url, document) {
                    return self.serve_static(id, &req, sink, &url, &method, hit, cors_cross).await;
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

            // HTTP cache (RFC 9111), after request-stage interception and CORS
            // preflight so that both still apply to cached responses.
            // A response to be paused is neither served from nor stored in it.
            let cacheable = method == Method::GET
                && body.is_none()
                && !headers.contains_key(header::RANGE)
                && pause_response.is_none();
            let request_time = wall_time();
            let sent = if cacheable { Some(headers.clone()) } else { None };
            let mut revalidating = None;
            if cacheable {
                let lookup = self.shared.cache.lock().unwrap().lookup(&url, &headers, cache_mode, request_time);
                match lookup {
                    crate::cache::Lookup::Hit(cached) => {
                        return self.serve_cached(id, &req, sink, &url, cached, cors_cross, redirects > 0, true, 0);
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
                let lookup = store.with(|c| c.lookup(&url, &headers, cache_mode, request_time));
                if let crate::cache::Lookup::Hit(cached) = lookup {
                    return self.serve_cached(id, &req, sink, &url, cached, cors_cross, redirects > 0, true, 0);
                }
            }
            if cache_mode == CacheMode::OnlyIfCached && revalidating.is_none() {
                return Err(NetError::Other("net::ERR_CACHE_MISS".into()));
            }

            let permit = self.origin_semaphore(&url).acquire_owned().await;
            let mut auth_attempts = 0;
            let auth_interceptor = interceptor.as_ref().filter(|i| i.handles_auth());
            let resp = loop {
                // Ordinary traffic does not need retry copies of the header map.
                let resp = if auth_interceptor.is_some() {
                    self.send(method.clone(), &url, headers.clone(), body.clone()).await?
                } else {
                    self.send(method.clone(), &url, std::mem::take(&mut headers), body.take()).await?
                };
                let proxy = resp.status() == StatusCode::PROXY_AUTHENTICATION_REQUIRED;
                if resp.status() != StatusCode::UNAUTHORIZED && !proxy {
                    break resp;
                }
                let Some(interceptor) = auth_interceptor else { break resp };
                let challenge_header = if proxy { header::PROXY_AUTHENTICATE } else { header::WWW_AUTHENTICATE };
                let Some((scheme, realm)) = auth_challenge(resp.headers(), challenge_header) else { break resp };
                if auth_attempts >= self.shared.config.max_auth_retries {
                    return Err(NetError::Blocked("net::ERR_INVALID_AUTH_CREDENTIALS".into()));
                }
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
                        origin: url.origin().ascii_serialization(),
                        scheme: scheme.clone(),
                        realm,
                        proxy,
                    },
                    tx,
                );
                match rx.await {
                    Ok(AuthDecision::Default | AuthDecision::CancelAuth) => break resp,
                    Ok(AuthDecision::ProvideCredentials { username, password }) => {
                        if proxy {
                            return Err(NetError::Other(
                                "proxy authentication requires proxy transport support".into(),
                            ));
                        }
                        if scheme != "basic" {
                            return Err(NetError::Other(format!("unsupported authentication scheme: {scheme}")));
                        }
                        if username.contains(':') {
                            return Err(NetError::Other("Basic authentication username contains ':'".into()));
                        }
                        use base64::Engine;
                        let value = base64::engine::general_purpose::STANDARD.encode(format!("{username}:{password}"));
                        let name = if proxy { header::PROXY_AUTHORIZATION } else { header::AUTHORIZATION };
                        headers.insert(
                            name,
                            HeaderValue::from_str(&format!("Basic {value}"))
                                .map_err(|_| NetError::Other("invalid credentials".into()))?,
                        );
                        auth_attempts += 1;
                        // Drop the challenge body without buffering it. Keep the same
                        // request ID and request-stage pause throughout auth retries.
                        drop(resp);
                    }
                    Err(_) => return Err(NetError::Other("authentication interception dropped".into())),
                }
            };
            let status = resp.status();
            let resp_headers = resp.headers().clone();
            let cookies_allowed = match req.credentials {
                Credentials::Include => true,
                Credentials::SameOrigin => cors::is_same_origin_request(&req),
                Credentials::Omit => false,
            };
            self.store_cookies(&url, &resp_headers, cookies_allowed);
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
                    let updated = self.shared.cache.lock().unwrap().update_from_304(
                        &url,
                        cached.id,
                        &resp_headers,
                        request_time,
                        response_time,
                    );
                    match updated {
                        Some(updated) => {
                            return self.serve_cached(
                                id,
                                &req,
                                sink,
                                &url,
                                updated,
                                cors_cross,
                                redirects > 0,
                                false,
                                0,
                            )
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
                    if !cors::same_origin(&url, &target) {
                        req.headers.remove(header::AUTHORIZATION);
                        req.headers.remove(header::COOKIE);
                        if cors_cross {
                            tainted = true;
                        }
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
            } else if req.mode == RequestMode::NoCors && !cors::is_same_origin_request(&req) {
                ResponseKind::Opaque
            } else {
                ResponseKind::Basic
            };
            let encoding = resp_headers.get(header::CONTENT_ENCODING).and_then(|v| v.to_str().ok()).map(str::to_owned);
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
                let head = self.head(&url, status, resp_headers, kind, redirects > 0);
                return self.deliver_body(id, &req, sink, head, body);
            }
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
                    if decoded as usize > max_entry {
                        stored = None;
                    }
                    if let Some(s) = stored.as_mut() {
                        s.push(out.clone());
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
                    cache.insert(&url, sent, status, &response, body.clone(), request_time, response_time);
                }
                if let (Some(s), Some(response)) = (shared_store, shared_response) {
                    s.with(|c| c.insert(&url, sent, status, &response, body, request_time, response_time));
                }
            }
            self.shared.emit(NetEvent::LoadingFinished { id, encoded_len: encoded, timestamp: self.shared.now() });
            deliver(sink, ResponseEvent::Finished { encoded_len: encoded, decoded_len: decoded });
            return Ok(());
        }
    }

    /// Effective cache mode, applying the Fetch "HTTP-network-or-cache fetch"
    /// request header rules and Network.setCacheDisabled.
    fn cache_mode(&self, req: &Request, headers: &mut HeaderMap) -> CacheMode {
        let disabled = req.defaults.cache_disabled || self.shared.settings.lock().unwrap().cache_disabled;
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
        cors_cross: bool,
        redirected: bool,
        from_cache: bool,
        encoded_len: u64,
    ) -> Result<(), NetError> {
        let kind = if cors_cross {
            cors::cors_check(&cached.headers, req.origin.as_deref().unwrap_or("null"), req.credentials)
                .map_err(NetError::Cors)?;
            ResponseKind::Cors
        } else if req.mode == RequestMode::NoCors && !cors::is_same_origin_request(req) {
            ResponseKind::Opaque
        } else {
            ResponseKind::Basic
        };
        let body = cached.body;
        if body.len() > req.max_body.unwrap_or(self.shared.config.max_body) {
            return Err(NetError::TooLarge);
        }
        let mut head = self.head(url, cached.status, cached.headers, kind, redirected);
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
        if body.len() > req.max_body.unwrap_or(self.shared.config.max_body) {
            return Err(NetError::TooLarge);
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
    ) -> Result<(), NetError> {
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
        } else if req.mode == RequestMode::NoCors && !cors::is_same_origin_request(req) {
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

/// Reads and decodes a whole response body (at most `max_body` bytes).
async fn read_whole_body(
    resp: http::Response<hyper::body::Incoming>,
    encoding: Option<&str>,
    max_body: usize,
) -> Result<Bytes, NetError> {
    let mut decoder = Decoder::for_encoding(encoding)?;
    let mut body = resp.into_body();
    let mut out = Vec::new();
    let push = |out: &mut Vec<u8>, chunk: Bytes| {
        out.extend_from_slice(&chunk);
        if out.len() > max_body {
            return Err(NetError::TooLarge);
        }
        Ok(())
    };
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|e| NetError::Other(e.to_string()))?;
        let Ok(data) = frame.into_data() else { continue };
        let decoded = decoder.push(data)?;
        push(&mut out, decoded)?;
    }
    let tail = decoder.finish()?;
    push(&mut out, tail)?;
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
    headers.insert(http::header::HeaderName::from_static("x-lightningpanda-static"), HeaderValue::from_static("1"));
    if spa_fallback {
        headers.insert(
            http::header::HeaderName::from_static("x-lightningpanda-spa-fallback"),
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

    /// Publishes the `NetEvent`s of a request the embedder answered from a
    /// `static_direct` hit (on the caller's thread, in order, all at once)
    /// and returns the response head to build the resource from.
    pub fn report_local(&self, req: &Request, hit: &crate::static_files::StaticMemoryHit) -> ResponseHead {
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
        };
        if !shared.observed() {
            return head;
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
        head
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
