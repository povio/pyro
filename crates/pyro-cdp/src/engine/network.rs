//! Per-context observations routed to the page sessions that enabled Network.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use bytes::Bytes;
use pyro_browser::ContextId;
use pyro_net::{NetEvent, RequestId};
use serde::Deserialize;
use serde_json::{json, Value};

use super::EngineHost;
use crate::msg::{CdpError, CdpResult};
use crate::network_protocol::{
    base64_encode, headers_json, protocol_request_id, BodyBudget, BodyStore, NetworkTranslator, MAX_INFLIGHT,
};
use crate::target::{ContextKey, PageCommand, PageKey};

#[derive(Default)]
pub(super) struct NetworkWake {
    ready: Arc<AtomicBool>,
    posted: Arc<AtomicBool>,
}

impl NetworkWake {
    pub(super) fn ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
    }

    /// Fetch interception decisions must wake command processing even while
    /// network response observations wait for their separate delivery task.
    pub(super) fn callback(&self, handle: pyro_runtime::LoopHandle<pyro_browser::Browser>) -> pyro_net::NetWake {
        let ready = self.ready.clone();
        let posted = self.posted.clone();
        Arc::new(move || {
            ready.store(true, Ordering::Release);
            if !posted.swap(true, Ordering::AcqRel) {
                let posted = posted.clone();
                let _ = handle.post(move |_| posted.store(false, Ordering::Release));
            }
        })
    }
}

pub(super) struct NetworkContext {
    pub(super) id: ContextId,
    subscription: Option<Arc<super::network_delivery::Inbox>>,
    routes: HashMap<RequestId, PageKey>,
    pub(super) fetch: Option<Arc<super::fetch::FetchQueue>>,
    /// `Pyro.run` mock rules of this context (`mocks.rs`).
    pub(super) mocks: Arc<super::mocks::DriverMocks>,
    /// Whether the installed interceptor was built with mocks present.
    pub(super) mocks_installed: bool,
}

impl NetworkContext {
    pub(super) fn new(id: ContextId) -> Self {
        Self {
            id,
            subscription: None,
            routes: HashMap::new(),
            fetch: None,
            mocks: Default::default(),
            mocks_installed: false,
        }
    }
}

pub(super) struct NetworkPage {
    pub(super) bodies: BodyStore,
    /// Request bodies for `Network.getRequestPostData`.
    pub(super) post_data: PostDataStore,
    translator: NetworkTranslator,
    /// The page's requests, kept once an `Pyro.run` asked for them.
    pub(super) log: Option<RequestLog>,
}

impl NetworkPage {
    pub(super) fn new(budget: BodyBudget) -> Self {
        Self {
            bodies: BodyStore::new(budget),
            post_data: PostDataStore::default(),
            translator: NetworkTranslator::default(),
            log: None,
        }
    }
}

/// Request bodies of a page's routed requests (`Network.getRequestPostData`)
/// by protocol request id, each at most [`MAX_LOGGED_POST_DATA`]; the oldest
/// go first past [`PostDataStore::MAX_BYTES`]. The bodies are shared with
/// the network layer's request (no copy).
#[derive(Default)]
pub(super) struct PostDataStore {
    bodies: HashMap<String, Bytes>,
    order: std::collections::VecDeque<String>,
    bytes: usize,
}

impl PostDataStore {
    const MAX_BYTES: usize = 8 * 1024 * 1024;

    fn observe(&mut self, event: &NetEvent) {
        let NetEvent::RequestWillBeSent { id, request, .. } = event else { return };
        let Some(body) = request.post_data.as_ref().filter(|b| !b.is_empty()) else { return };
        let body = body.slice(..body.len().min(MAX_LOGGED_POST_DATA));
        let key = protocol_request_id(*id, request.destination, request.loader_id.as_deref());
        if let Some(old) = self.bodies.insert(key.clone(), body.clone()) {
            self.bytes -= old.len();
            self.order.retain(|k| *k != key);
        }
        self.bytes += body.len();
        self.order.push_back(key);
        while self.bytes > Self::MAX_BYTES {
            let Some(oldest) = self.order.pop_front() else { break };
            if let Some(b) = self.bodies.remove(&oldest) {
                self.bytes -= b.len();
            }
        }
    }

    fn get(&self, id: &str) -> Option<&Bytes> {
        self.bodies.get(id)
    }

    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }
}

/// Requests kept per page in the log; older ones are dropped in halves.
const MAX_LOGGED: usize = 2_000;

/// A page's requests in the order they started (`Pyro.run`'s `request` steps
/// and reply). A redirect ends one entry and starts the next.
#[derive(Default)]
pub(super) struct RequestLog {
    entries: Vec<LoggedRequest>,
    open: HashMap<RequestId, usize>,
    /// Entries dropped from the front: `seq` keeps counting across drops.
    dropped: usize,
    /// Keep response bodies (from the first run with a `responseBody`
    /// request check on): `request` steps hand them out.
    pub(super) capture_bodies: bool,
    /// Bytes of the kept response bodies.
    body_bytes: usize,
}

/// Longest request body a logged request keeps (`postDataTruncated` past it).
const MAX_LOGGED_POST_DATA: usize = 1024 * 1024;

#[derive(Clone, Debug)]
pub(super) struct LoggedRequest {
    pub(super) method: String,
    pub(super) url: String,
    pub(super) status: Option<u16>,
    pub(super) error: Option<String>,
    /// Finished or failed.
    pub(super) done: bool,
    start: f64,
    end: Option<f64>,
    /// The headers as sent (after cookies and extra headers).
    pub(super) request_headers: http::HeaderMap,
    /// The request body, at most `MAX_LOGGED_POST_DATA` bytes of it.
    pub(super) post_data: Option<Bytes>,
    post_data_truncated: bool,
    /// The response headers once the head arrived.
    pub(super) response_headers: Option<http::HeaderMap>,
    /// The response body, when the log keeps bodies (`capture_bodies`).
    response_body: Option<ResponseBody>,
}

/// A logged response's body: shared chunks, at most `MAX_LOGGED_RESPONSE_BODY`.
#[derive(Clone, Debug, Default)]
struct ResponseBody {
    chunks: Vec<Bytes>,
    len: usize,
    textual: bool,
    truncated: bool,
}

/// Longest response body a logged request keeps (`responseBodyTruncated` past it).
const MAX_LOGGED_RESPONSE_BODY: usize = 4 * 1024 * 1024;
/// Response bytes a page's log keeps in all: older bodies go first.
const MAX_LOGGED_RESPONSE_BYTES: usize = 32 * 1024 * 1024;

pub(super) fn redact_header_values(mut headers: Value) -> Value {
    if let Some(values) = headers.as_object_mut() {
        for (name, value) in values {
            if matches!(
                name.to_ascii_lowercase().as_str(),
                "authorization"
                    | "proxy-authorization"
                    | "cookie"
                    | "cookie2"
                    | "set-cookie"
                    | "set-cookie2"
                    | "x-api-key"
                    | "x-auth-token"
                    | "x-csrf-token"
                    | "x-xsrf-token"
            ) {
                *value = Value::String("[REDACTED]".into());
            }
        }
    }
    headers
}

fn diagnostic_headers(headers: &http::HeaderMap) -> Value {
    redact_header_values(headers_json(headers))
}

impl LoggedRequest {
    pub(super) fn path(&self) -> &str {
        let rest = self.url.split_once("://").map_or(self.url.as_str(), |(_, r)| r);
        let path = rest.find('/').map_or("/", |i| &rest[i..]);
        path.split(['?', '#']).next().unwrap_or(path)
    }

    /// The request body as text (`None` when absent or not UTF-8).
    pub(super) fn post_data_text(&self) -> Option<&str> {
        self.post_data.as_deref().and_then(|b| std::str::from_utf8(b).ok())
    }

    /// The request body parsed as JSON.
    pub(super) fn post_data_json(&self) -> Option<Value> {
        let body = self.post_data.as_deref().filter(|_| !self.post_data_truncated)?;
        serde_json::from_slice(body).ok()
    }

    /// The values of a request header (names are case-insensitive).
    pub(super) fn request_header_values(&self, name: &str) -> impl Iterator<Item = &str> {
        self.request_headers.get_all(name.to_ascii_lowercase()).iter().filter_map(|v| v.to_str().ok())
    }

    pub(super) fn to_json(&self) -> Value {
        let mut v = json!({
            "method": self.method,
            "url": self.url,
            "status": self.status,
            "error": self.error,
            "ms": self.end.map(|e| (e - self.start) * 1e3),
            "requestHeaders": diagnostic_headers(&self.request_headers),
            "responseHeaders": self.response_headers.as_ref().map(diagnostic_headers),
        });
        if let Some(body) = &self.post_data {
            match std::str::from_utf8(body) {
                Ok(text) => v["postData"] = json!(text),
                Err(_) => {
                    v["postData"] = json!(base64_encode(body));
                    v["postDataBase64"] = json!(true);
                }
            }
            if self.post_data_truncated {
                v["postDataTruncated"] = json!(true);
            }
        }
        v
    }

    pub(super) fn has_response_body(&self) -> bool {
        self.response_body.is_some()
    }

    /// [`Self::to_json`] with the response body, when the log kept it.
    pub(super) fn to_json_with_body(&self) -> Value {
        let mut v = self.to_json();
        if let Some(body) = &self.response_body {
            let mut bytes = Vec::with_capacity(body.len);
            for chunk in &body.chunks {
                bytes.extend_from_slice(chunk);
            }
            match String::from_utf8(bytes) {
                Ok(text) if body.textual => v["responseBody"] = json!(text),
                Ok(text) => {
                    v["responseBody"] = json!(base64_encode(text.as_bytes()));
                    v["responseBodyBase64"] = json!(true);
                }
                Err(e) => {
                    v["responseBody"] = json!(base64_encode(e.as_bytes()));
                    v["responseBodyBase64"] = json!(true);
                }
            }
            if body.truncated {
                v["responseBodyTruncated"] = json!(true);
            }
        }
        v
    }
}

impl RequestLog {
    /// Sequence number of the next request.
    pub(super) fn seq(&self) -> usize {
        self.dropped + self.entries.len()
    }

    pub(super) fn history_truncated(&self) -> bool {
        self.dropped != 0
    }

    /// Requests started at or after `seq`.
    pub(super) fn since(&self, seq: usize) -> &[LoggedRequest] {
        &self.entries[seq.saturating_sub(self.dropped).min(self.entries.len())..]
    }

    fn observe(&mut self, event: &NetEvent) {
        match event {
            NetEvent::RequestWillBeSent { id, request, redirect_response, timestamp, .. } => {
                if let (Some(head), Some(i)) = (redirect_response, self.open.remove(id)) {
                    let e = &mut self.entries[i];
                    (e.status, e.done, e.end) = (Some(head.status.as_u16()), true, Some(*timestamp));
                }
                if self.entries.len() >= MAX_LOGGED {
                    let cut = MAX_LOGGED / 2;
                    for gone in self.entries.drain(..cut) {
                        self.body_bytes -= gone.response_body.map_or(0, |b| b.len);
                    }
                    self.dropped += cut;
                    self.open.retain(|_, i| {
                        let keep = *i >= cut;
                        *i = i.saturating_sub(cut);
                        keep
                    });
                }
                let (post_data, post_data_truncated) = match &request.post_data {
                    Some(b) if b.len() > MAX_LOGGED_POST_DATA => (Some(b.slice(..MAX_LOGGED_POST_DATA)), true),
                    other => (other.clone(), false),
                };
                let entry = LoggedRequest {
                    method: request.method.to_string(),
                    url: request.url.to_string(),
                    status: None,
                    error: None,
                    done: false,
                    start: *timestamp,
                    end: None,
                    request_headers: request.headers.clone(),
                    post_data,
                    post_data_truncated,
                    response_headers: None,
                    response_body: None,
                };
                // The same hop sent again without a redirect (a cache
                // revalidation that had to refetch) is still one request.
                if let Some(&i) = self.open.get(id) {
                    self.body_bytes -= self.entries[i].response_body.as_ref().map_or(0, |b| b.len);
                    self.entries[i] = entry;
                    return;
                }
                self.open.insert(*id, self.entries.len());
                self.entries.push(entry);
            }
            NetEvent::ResponseReceived { id, head, .. } => {
                if let Some(&i) = self.open.get(id) {
                    self.entries[i].status = Some(head.status.as_u16());
                    self.entries[i].response_headers = Some(head.headers.clone());
                    if self.capture_bodies {
                        let mime = &head.mime;
                        let textual = mime.starts_with("text/")
                            || mime.contains("json")
                            || mime.contains("javascript")
                            || mime.contains("xml")
                            || mime.is_empty();
                        self.body_bytes -= self.entries[i].response_body.as_ref().map_or(0, |b| b.len);
                        self.entries[i].response_body = Some(ResponseBody { textual, ..Default::default() });
                    }
                }
            }
            NetEvent::DataReceived { id, data, .. } => {
                let Some(&i) = self.open.get(id) else { return };
                let Some(body) = &self.entries[i].response_body else { return };
                if body.truncated || data.is_empty() {
                    return;
                }
                if body.len + data.len() > MAX_LOGGED_RESPONSE_BODY {
                    self.entries[i].response_body.as_mut().expect("checked").truncated = true;
                    return;
                }
                // Room from the oldest kept bodies first (never this one's).
                let mut oldest = 0;
                while self.body_bytes + data.len() > MAX_LOGGED_RESPONSE_BYTES && oldest < self.entries.len() {
                    if oldest != i {
                        if let Some(gone) = self.entries[oldest].response_body.take() {
                            self.body_bytes -= gone.len;
                        }
                    }
                    oldest += 1;
                }
                if self.body_bytes + data.len() > MAX_LOGGED_RESPONSE_BYTES {
                    self.entries[i].response_body.as_mut().expect("checked").truncated = true;
                    return;
                }
                let body = self.entries[i].response_body.as_mut().expect("checked");
                // `Bytes` is reference counted: keeping it does not copy.
                body.chunks.push(data.clone());
                body.len += data.len();
                self.body_bytes += data.len();
            }
            NetEvent::LoadingFinished { id, timestamp, .. } => {
                if let Some(i) = self.open.remove(id) {
                    (self.entries[i].done, self.entries[i].end) = (true, Some(*timestamp));
                }
            }
            NetEvent::LoadingFailed { id, error, timestamp, .. } => {
                if let Some(i) = self.open.remove(id) {
                    let e = &mut self.entries[i];
                    (e.error, e.done, e.end) = (Some(error.to_string()), true, Some(*timestamp));
                }
            }
            NetEvent::WebSocket { .. } => {}
        }
    }
}

impl EngineHost {
    pub(super) fn network_domain(&mut self, action: &str, cmd: &PageCommand<'_, '_>) -> CdpResult<Option<Value>> {
        match action {
            // The same override as Emulation's (puppeteer sends this one).
            "setUserAgentOverride" => self.emulation_domain(action, cmd),
            "getCookies" | "setCookie" | "setCookies" | "deleteCookies" | "clearBrowserCookies" => {
                self.network_cookies(action, cmd)
            }
            // Offline only, for the page's browser context (Chrome scopes it
            // to the target); throttling fields are accepted and ignored.
            "emulateNetworkConditions" => {
                #[derive(Deserialize)]
                struct Params {
                    offline: bool,
                }
                let params: Params = cmd.command.params()?;
                let state = self.pages.get(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?;
                let context =
                    self.contexts.get(&state.context).ok_or_else(|| CdpError::server("Browser context is gone"))?;
                if let Some(net) = self.thread.browser.context_net(context.id) {
                    net.update_settings(|s| s.offline = params.offline);
                }
                Ok(Some(json!({})))
            }
            "enable" => {
                // The server's process budget is currently configured by its
                // embedder. Refuse per-session options rather than ignore them.
                let params: serde_json::Map<String, Value> = cmd.command.params()?;
                if !params.is_empty() {
                    return Err(CdpError::server("Network.enable buffer options are not supported"));
                }
                self.sessions
                    .get_mut(cmd.session_id)
                    .ok_or_else(|| CdpError::server("session is gone"))?
                    .network_enabled = true;
                self.update_network_subscriptions();
                self.apply_network_overrides(cmd.page);
                Ok(Some(json!({})))
            }
            "disable" => {
                if let Some(session) = self.sessions.get_mut(cmd.session_id) {
                    session.network_enabled = false;
                }
                self.update_network_subscriptions();
                self.apply_network_overrides(cmd.page);
                Ok(Some(json!({})))
            }
            "setExtraHTTPHeaders" => {
                #[derive(Deserialize)]
                struct Params {
                    headers: serde_json::Map<String, Value>,
                }
                let params: Params = cmd.command.params()?;
                let headers = parse_extra_headers(&params.headers)?;
                self.sessions
                    .get_mut(cmd.session_id)
                    .ok_or_else(|| CdpError::server("session is gone"))?
                    .extra_headers = headers;
                self.apply_network_overrides(cmd.page);
                Ok(Some(json!({})))
            }
            "getResponseBody" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct Params {
                    request_id: String,
                }
                let params: Params = cmd.command.params()?;
                if !self.sessions.get(cmd.session_id).is_some_and(|s| s.network_enabled) {
                    return Err(CdpError::server("Network domain is not enabled"));
                }
                let state = self.pages.get(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?;
                state.network.bodies.get_by_protocol_id(&params.request_id).map(Some)
            }
            "getRequestPostData" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct Params {
                    request_id: String,
                }
                let params: Params = cmd.command.params()?;
                let state = self.pages.get(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?;
                let body = state
                    .network
                    .post_data
                    .get(&params.request_id)
                    .ok_or_else(|| CdpError::server("No post data available for the request"))?;
                Ok(Some(json!({ "postData": String::from_utf8_lossy(body) })))
            }
            "setCacheDisabled" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct Params {
                    cache_disabled: bool,
                }
                let params: Params = cmd.command.params()?;
                self.sessions
                    .get_mut(cmd.session_id)
                    .ok_or_else(|| CdpError::server("session is gone"))?
                    .cache_disabled = params.cache_disabled;
                self.apply_network_overrides(cmd.page);
                Ok(Some(json!({})))
            }
            "setBypassServiceWorker" => {
                #[derive(Deserialize)]
                struct Params {
                    bypass: bool,
                }
                let params: Params = cmd.command.params()?;
                self.sessions.get_mut(cmd.session_id).ok_or_else(|| CdpError::server("session is gone"))?.bypass_service_worker =
                    params.bypass;
                self.apply_network_overrides(cmd.page);
                Ok(Some(json!({})))
            }
            "clearBrowserCache" => {
                // Chrome clears the cache shared by the target's browser
                // context: other pages of that context miss afterwards. With
                // the opt-in shared public-asset cache, its process-wide store
                // is part of what this context reads, so it is cleared too (a
                // no-op when sharing is off).
                let state = self.pages.get(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?;
                let context =
                    self.contexts.get(&state.context).ok_or_else(|| CdpError::server("Browser context is gone"))?;
                if let Some(net) = self.thread.browser.context_net(context.id) {
                    net.clear_http_cache();
                    net.clear_shared_asset_cache();
                }
                Ok(Some(json!({})))
            }
            _ => Err(CdpError::unsupported(&cmd.command.method)),
        }
    }

    /// Security domain on a page session: certificate errors are ignored for
    /// the page's browser context (no security state events are reported).
    pub(super) fn security_domain(&mut self, action: &str, cmd: &PageCommand<'_, '_>) -> CdpResult<Option<Value>> {
        match action {
            "enable" | "disable" => Ok(Some(json!({}))),
            "setIgnoreCertificateErrors" => {
                let params: IgnoreCertificateErrors = cmd.command.params()?;
                let state = self.pages.get(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?;
                let context =
                    self.contexts.get(&state.context).ok_or_else(|| CdpError::server("Browser context is gone"))?;
                if let Some(net) = self.thread.browser.context_net(context.id) {
                    net.set_ignore_certificate_errors(params.ignore);
                }
                Ok(Some(json!({})))
            }
            _ => Err(CdpError::unsupported(&cmd.command.method)),
        }
    }

    /// Hands the page the request overrides of its sessions that have Network
    /// enabled (Chrome applies a session's settings only while its Network
    /// domain is enabled): the merged extra headers (later sessions override
    /// earlier ones; an empty set clears) and cache bypass (any session).
    /// Sibling pages, even in the same browser context, are unaffected.
    pub(super) fn apply_network_overrides(&mut self, page: PageKey) {
        let Some(state) = self.pages.get(&page) else { return };
        let mut headers = http::HeaderMap::new();
        let mut cache_disabled = false;
        for session in state.sessions.iter().filter_map(|s| self.sessions.get(s)).filter(|s| s.network_enabled) {
            for (name, value) in &session.extra_headers {
                headers.insert(name.clone(), value.clone());
            }
            cache_disabled |= session.cache_disabled;
        }
        // False only for a page pyro-browser no longer has, which then has no
        // requests left to apply them to.
        let _ = self.thread.browser.set_extra_http_headers(state.id, headers);
        let _ = self.thread.browser.set_cache_disabled(state.id, cache_disabled);
        // Not tied to Network.enable, as in Chrome.
        let bypass = state.sessions.iter().filter_map(|s| self.sessions.get(s)).any(|s| s.bypass_service_worker);
        let _ = self.thread.browser.set_bypass_service_worker(state.id, bypass);
    }

    pub(super) fn update_network_subscriptions(&mut self) {
        for page in self.pages.values_mut() {
            if !page.sessions.iter().any(|s| self.sessions.get(s).is_some_and(|s| s.network_enabled)) {
                page.network.bodies.clear();
                page.network.post_data.clear();
                page.network.translator = NetworkTranslator::default();
            }
        }
        self.update_fetch_interceptors();
        for (key, context) in &mut self.contexts {
            let wanted = self.pages.values().any(|page| {
                page.context == *key
                    && (page.network.log.is_some()
                        || page.sessions.iter().any(|s| {
                            self.sessions.get(s).is_some_and(|s| s.network_enabled || s.fetch.needs_observation())
                        }))
            });
            if wanted && context.subscription.is_none() {
                if let Some(net) = self.thread.browser.context_net(context.id) {
                    let subscription = super::network_delivery::Inbox::new(
                        net,
                        self.thread.handle(),
                        self.network_wake.ready.clone(),
                        self.collector.network_budget.clone(),
                        self.out.clone(),
                    );
                    self.collector.watch_network(*key, context.id, &subscription);
                    context.subscription = Some(subscription);
                }
            } else if !wanted {
                context.subscription = None;
                context.routes.clear();
            }
            context.routes.retain(|_, page| {
                self.pages.get(page).is_some_and(|p| {
                    p.network.log.is_some()
                        || p.sessions.iter().any(|s| self.sessions.get(s).is_some_and(|s| s.network_enabled))
                })
            });
        }
    }

    pub(super) fn flush_network(&mut self) {
        self.network_wake.ready.store(false, Ordering::Release);
        let contexts: Vec<_> = self.contexts.keys().copied().collect();
        for key in contexts {
            // Bound work per flush so command handling is not starved by a
            // continuous response stream. Remaining events cause another wake.
            for index in 0..256 {
                if self.out.is_exceeded() {
                    if let Some(context) = self.contexts.get_mut(&key) {
                        context.subscription = None;
                        context.routes.clear();
                    }
                    break;
                }
                let result = self.contexts.get(&key).and_then(|c| c.subscription.as_ref()).and_then(|s| s.pop());
                match result {
                    Some(event) => self.on_network_event(key, event.event),
                    None => break,
                }
                if index == 255 {
                    self.network_wake.ready.store(true, Ordering::Release);
                }
            }
        }
    }

    pub(super) fn on_network_event(&mut self, context: ContextKey, event: NetEvent) {
        if let NetEvent::LoadingFinished { id, .. } | NetEvent::LoadingFailed { id, .. } = &event {
            self.finish_fetch(context, *id);
        }
        let id = match &event {
            NetEvent::RequestWillBeSent { id, .. }
            | NetEvent::ResponseReceived { id, .. }
            | NetEvent::DataReceived { id, .. }
            | NetEvent::LoadingFinished { id, .. }
            | NetEvent::LoadingFailed { id, .. }
            | NetEvent::WebSocket { id, .. } => *id,
        };
        // A connection is routed by the frame that created it, like a request.
        let starting_frame = match &event {
            NetEvent::RequestWillBeSent { request, .. } => Some(request.frame_id.as_ref()),
            NetEvent::WebSocket { activity, .. } => match &**activity {
                pyro_net::WebSocketActivity::Created { frame_id, .. } => Some(frame_id.as_ref()),
                _ => None,
            },
            _ => None,
        };
        if let Some(frame_id) = starting_frame {
            let page = self.pages.iter().find_map(|(key, page)| {
                (page.context == context && self.owns_frame(page.id, frame_id)).then_some(*key)
            });
            let Some(page) = page else { return };
            let wanted = self.pages.get(&page).is_some_and(|p| {
                p.network.log.is_some()
                    || p.sessions.iter().any(|s| self.sessions.get(s).is_some_and(|s| s.network_enabled))
            });
            if !wanted {
                return;
            }
            let context = self.contexts.get_mut(&context).expect("subscribed context");
            if !context.routes.contains_key(&id) && context.routes.len() >= MAX_INFLIGHT {
                self.out.exceed();
                return;
            }
            context.routes.insert(id, page);
        }
        let Some(page) = self.contexts.get(&context).and_then(|c| c.routes.get(&id)).copied() else { return };
        let ended = match &event {
            NetEvent::LoadingFinished { .. } | NetEvent::LoadingFailed { .. } => true,
            NetEvent::WebSocket { activity, .. } => matches!(**activity, pyro_net::WebSocketActivity::Closed),
            _ => false,
        };
        if ended {
            self.contexts.get_mut(&context).expect("subscribed context").routes.remove(&id);
        }
        let Some(state) = self.pages.get_mut(&page) else { return };
        if let Some(log) = &mut state.network.log {
            log.observe(&event);
            self.waiters.log_changed = true;
        }
        state.network.bodies.observe(&event);
        // Request bodies only for requests that have one, while a session
        // of the page has Network enabled.
        if matches!(&event, NetEvent::RequestWillBeSent { request, .. } if request.post_data.is_some())
            && state.sessions.iter().any(|s| self.sessions.get(s).is_some_and(|s| s.network_enabled))
        {
            state.network.post_data.observe(&event);
        }
        // The params borrow from the translator: the page's sessions are read
        // field by field next to it.
        if let Some((method, params)) = state.network.translator.translate(&event) {
            let sessions = &self.sessions;
            for session in state.sessions.iter().filter(|s| sessions.get(*s).is_some_and(|s| s.network_enabled)) {
                self.out.push(crate::msg::event_frame(method, &params, Some(session)));
            }
        }
    }
}

/// `Network.Headers` to a header map, rejecting what Chrome 147 rejects with
/// the same messages (non-string values, invalid names and values).
fn parse_extra_headers(headers: &serde_json::Map<String, Value>) -> CdpResult<http::HeaderMap> {
    let mut map = http::HeaderMap::new();
    for (name, value) in headers {
        let name = http::HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| CdpError::invalid_params("Invalid header name"))?;
        let value = value.as_str().ok_or_else(|| CdpError::invalid_params("Invalid header value, string expected"))?;
        let value = http::HeaderValue::from_str(value).map_err(|_| CdpError::invalid_params("Invalid header value"))?;
        map.insert(name, value);
    }
    Ok(map)
}

/// `Security.setIgnoreCertificateErrors` parameters.
#[derive(Deserialize)]
pub(super) struct IgnoreCertificateErrors {
    pub(super) ignore: bool,
}
