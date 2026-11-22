//! `Network` domain translation of `pyro_net::NetEvent`s and the response-body
//! store behind `Network.getResponseBody`.
//!
//! Event shapes follow upstream `src/server/cdp/domains/network.zig` and what
//! Playwright 1.59's `crNetworkManager.js` reads. Wiring to a browser
//! context's `NetClient` lives in the engine host.

use std::collections::{HashMap, VecDeque};

use bytes::Bytes;
use http::HeaderMap;
use pyro_net::{NetEvent, RequestId, RequestInfo, ResponseHead};
use serde::ser::{SerializeMap, Serializer};
use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::fetch_protocol::ResourceType;
use crate::msg::{CdpError, CdpResult};

/// CDP `Network.Headers`: an object; repeated headers are joined with "\n"
/// as Chrome does.
pub fn headers_json(headers: &HeaderMap) -> Value {
    // A header map yields a name's values one after another, so repeated
    // lines join the entry just pushed; the map is built once, from the
    // list (every request and response event: no lookup per header).
    let mut list: Vec<(String, Value)> = Vec::with_capacity(headers.keys_len());
    let mut last: Option<&str> = None;
    for (name, value) in headers {
        let value = String::from_utf8_lossy(value.as_bytes());
        if last == Some(name.as_str()) {
            if let Some((_, Value::String(existing))) = list.last_mut() {
                existing.push('\n');
                existing.push_str(&value);
            }
            continue;
        }
        last = Some(name.as_str());
        list.push((name.as_str().to_owned(), Value::String(value.into_owned())));
    }
    Value::Object(list.into_iter().collect::<Map<String, Value>>())
}

/// CDP `Network.Headers` written straight to the serializer: the same text
/// as serializing [`headers_json`] (names sorted, as a JSON object map keeps
/// them; a name's lines joined with "\n"), without building the map, a
/// `String` per name or one per value.
pub struct Headers<'a>(pub &'a HeaderMap);

impl Serialize for Headers<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut lines: Vec<(&str, &[u8])> = Vec::with_capacity(self.0.len());
        lines.extend(self.0.iter().map(|(name, value)| (name.as_str(), value.as_bytes())));
        // Stable, and a header map yields a name's values one after another:
        // a name's lines stay together and in order.
        lines.sort_by(|a, b| a.0.cmp(b.0));
        let mut map = serializer.serialize_map(None)?;
        let mut rest = &lines[..];
        while let [(name, first), tail @ ..] = rest {
            let repeated = tail.iter().take_while(|(n, _)| n == name).count();
            if repeated == 0 {
                map.serialize_entry(name, &*String::from_utf8_lossy(first))?;
            } else {
                let mut joined = String::from_utf8_lossy(first).into_owned();
                for (_, value) in &tail[..repeated] {
                    joined.push('\n');
                    joined.push_str(&String::from_utf8_lossy(value));
                }
                map.serialize_entry(name, &joined)?;
            }
            rest = &tail[repeated..];
        }
        map.end()
    }
}

/// CDP `Network.Response`. Fields are declared in sorted order, the order a
/// JSON object map writes them, so the text is that of the `Value` it
/// replaced (`translation_text_matches_value_translation` checks it).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ResponseJson<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    charset: Option<&'a str>,
    connection_id: u8,
    connection_reused: bool,
    encoded_data_length: u64,
    from_disk_cache: bool,
    from_service_worker: bool,
    headers: Headers<'a>,
    mime_type: &'a str,
    protocol: &'static str,
    #[serde(rename = "remoteIPAddress", skip_serializing_if = "Option::is_none")]
    remote_ip_address: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    remote_port: Option<u16>,
    security_state: &'static str,
    status: u16,
    status_text: &'a str,
    url: &'a str,
}

fn response_json(head: &ResponseHead, encoded_len: u64) -> ResponseJson<'_> {
    let remote = head.remote_addr.as_deref().and_then(|addr| addr.rsplit_once(':'));
    ResponseJson {
        charset: head.charset.as_deref(),
        connection_id: 0,
        connection_reused: false,
        encoded_data_length: encoded_len,
        from_disk_cache: head.from_cache,
        from_service_worker: false,
        headers: Headers(&head.headers),
        mime_type: &head.mime,
        protocol: if head.protocol == pyro_net::http::Version::HTTP_2 { "h2" } else { "http/1.1" },
        remote_ip_address: remote.map(|(ip, _)| ip.trim_start_matches('[').trim_end_matches(']')),
        remote_port: remote.map(|(_, port)| port.parse::<u16>().unwrap_or(0)),
        security_state: if head.url.scheme() == "https" { "secure" } else { "neutral" },
        status: head.status.as_u16(),
        status_text: &head.status_text,
        url: head.url.as_str(),
    }
}

/// CDP `Network.Request` of `Network.requestWillBeSent` (fields in sorted
/// order, see [`ResponseJson`]); `hasPostData` only with a body.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RequestJson<'a> {
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    has_post_data: bool,
    headers: Headers<'a>,
    initial_priority: &'static str,
    method: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    post_data: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    post_data_entries: Option<[PostDataEntry; 1]>,
    referrer_policy: &'static str,
    url: &'a str,
}

fn request_json(request: &RequestInfo) -> RequestJson<'_> {
    let post = PostData::of(request.post_data.as_ref());
    RequestJson {
        has_post_data: post.has,
        headers: Headers(&request.headers),
        initial_priority: "High",
        method: request.method.as_str(),
        post_data: post.text,
        post_data_entries: post.entries,
        referrer_policy: "strict-origin-when-cross-origin",
        url: request.url.as_str(),
    }
}

/// CDP `Network.Request` of `Fetch.requestPaused` (fields in sorted order,
/// see [`ResponseJson`]); `hasPostData` always.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PausedRequestJson<'a> {
    has_post_data: bool,
    headers: Headers<'a>,
    method: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    post_data: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    post_data_entries: Option<[PostDataEntry; 1]>,
    url: &'a str,
}

impl<'a> PausedRequestJson<'a> {
    pub fn new(request: &'a RequestInfo) -> Self {
        let post = PostData::of(request.post_data.as_ref());
        PausedRequestJson {
            has_post_data: post.has,
            headers: Headers(&request.headers),
            method: request.method.as_str(),
            post_data: post.text,
            post_data_entries: post.entries,
            url: request.url.as_str(),
        }
    }
}

/// `hasPostData`, `postData` and `postDataEntries` of a CDP
/// `Network.Request`.
///
/// Playwright builds `request.postData()` / `postDataJSON()` only from
/// `postDataEntries[].bytes` (base64), so the binary-safe entry must be
/// present. `postData` is the deprecated text form; it is omitted for bodies
/// that are not valid UTF-8 rather than sent lossily. Bodies over
/// [`MAX_POST_DATA`] report only `hasPostData`.
struct PostData<'a> {
    has: bool,
    text: Option<&'a str>,
    entries: Option<[PostDataEntry; 1]>,
}

impl<'a> PostData<'a> {
    fn of(body: Option<&'a Bytes>) -> Self {
        let inline = body.filter(|body| body.len() <= MAX_POST_DATA);
        PostData {
            has: body.is_some(),
            text: inline.and_then(|body| std::str::from_utf8(body).ok()),
            entries: inline.map(|body| [PostDataEntry { bytes: base64_encode(body) }]),
        }
    }
}

#[derive(Serialize)]
struct PostDataEntry {
    bytes: String,
}

#[derive(Serialize)]
struct Initiator {
    #[serde(rename = "type")]
    kind: &'static str,
}

/// `Network.requestWillBeSent` (fields in sorted order, see [`ResponseJson`]).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestWillBeSent<'a> {
    #[serde(rename = "documentURL")]
    document_url: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    frame_id: Option<&'a str>,
    initiator: Initiator,
    loader_id: &'a str,
    redirect_has_extra_info: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    redirect_response: Option<ResponseJson<'a>>,
    request: RequestJson<'a>,
    request_id: &'a str,
    timestamp: f64,
    #[serde(rename = "type")]
    resource_type: &'static str,
    wall_time: f64,
}

/// `Network.responseReceived` (fields in sorted order, see [`ResponseJson`]).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponseReceived<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    frame_id: Option<&'a str>,
    has_extra_info: bool,
    loader_id: &'a str,
    request_id: &'a str,
    response: ResponseJson<'a>,
    timestamp: f64,
    #[serde(rename = "type")]
    resource_type: &'static str,
}

/// `Network.dataReceived`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DataReceived<'a> {
    data_length: usize,
    encoded_data_length: usize,
    request_id: &'a str,
    timestamp: f64,
}

/// `Network.loadingFinished`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadingFinished {
    encoded_data_length: u64,
    request_id: String,
    timestamp: f64,
}

/// `Network.loadingFailed` (fields in sorted order, see [`ResponseJson`]).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadingFailed<'a> {
    canceled: bool,
    error_text: &'a str,
    request_id: String,
    timestamp: f64,
    #[serde(rename = "type")]
    resource_type: &'static str,
}

/// Params of a translated `Network.*` event. The events every request has
/// are written straight from the event they translate; the others go
/// through a `Value`.
#[derive(Serialize)]
#[serde(untagged)]
pub enum NetworkParams<'a> {
    RequestWillBeSent(RequestWillBeSent<'a>),
    ResponseReceived(ResponseReceived<'a>),
    DataReceived(DataReceived<'a>),
    LoadingFinished(LoadingFinished),
    LoadingFailed(LoadingFailed<'a>),
    Value(Value),
}

impl NetworkParams<'_> {
    /// The params as a JSON value.
    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).expect("CDP params serialize")
    }
}

/// Chromium identifies a document navigation by requestId == loaderId.
/// Native network ids remain context-local and are used internally.
pub fn protocol_request_id(id: RequestId, destination: pyro_net::Destination, loader_id: Option<&str>) -> String {
    if matches!(destination, pyro_net::Destination::Document | pyro_net::Destination::Iframe) {
        if let Some(loader) = loader_id.filter(|s| !s.is_empty()) {
            return loader.to_owned();
        }
    }
    id.to_string()
}

/// Per-request facts needed by later events of the same request.
#[derive(Debug, Clone)]
struct Inflight {
    request_id: String,
    loader_id: String,
    frame_id: Option<String>,
    resource_type: &'static str,
    /// A document response with Content-Disposition: attachment: the
    /// navigation became a download, and Chrome reports its request as
    /// aborted (no dataReceived, loadingFailed net::ERR_ABORTED).
    download: bool,
}

fn is_attachment(head: &pyro_net::ResponseHead) -> bool {
    head.headers
        .get(http::header::CONTENT_DISPOSITION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .is_some_and(|t| t.trim().eq_ignore_ascii_case("attachment"))
}

/// Translates network events for one page into CDP `Network.*` events.
#[derive(Default)]
pub struct NetworkTranslator {
    inflight: HashMap<RequestId, Inflight>,
}

/// Most requests tracked between `requestWillBeSent` and completion.
pub const MAX_INFLIGHT: usize = 16 * 1024;

impl NetworkTranslator {
    /// Returns `(method, params)` for the CDP event, or `None` for events of
    /// requests this translator did not see start (or over budget).
    ///
    /// The per-request events borrow from `event` and the translator rather
    /// than copying into a `Value` (several header maps' worth of `String`s
    /// per request).
    pub fn translate<'a>(&'a mut self, event: &'a NetEvent) -> Option<(&'static str, NetworkParams<'a>)> {
        match event {
            NetEvent::RequestWillBeSent { id, request, redirect_response, timestamp, wall_time } => {
                let resource_type = request.destination.cdp_type();
                if !self.inflight.contains_key(id) && self.inflight.len() >= MAX_INFLIGHT {
                    return None;
                }
                let request_id = protocol_request_id(*id, request.destination, request.loader_id.as_deref());
                let inflight = self
                    .inflight
                    .entry(*id)
                    .insert_entry(Inflight {
                        request_id,
                        loader_id: request.loader_id.clone().unwrap_or_default(),
                        frame_id: request.frame_id.clone(),
                        resource_type,
                        download: false,
                    })
                    .into_mut();
                let params = RequestWillBeSent {
                    document_url: if resource_type == "Document" { request.url.as_str() } else { "" },
                    frame_id: request.frame_id.as_deref(),
                    initiator: Initiator { kind: "other" },
                    loader_id: request.loader_id.as_deref().unwrap_or_default(),
                    redirect_has_extra_info: false,
                    redirect_response: redirect_response.as_deref().map(|redirect| response_json(redirect, 0)),
                    request: request_json(request),
                    request_id: &inflight.request_id,
                    timestamp: *timestamp,
                    resource_type,
                    wall_time: *wall_time,
                };
                Some(("Network.requestWillBeSent", NetworkParams::RequestWillBeSent(params)))
            }
            NetEvent::ResponseReceived { id, head, timestamp, .. } => {
                let inflight = self.inflight.get_mut(id)?;
                inflight.download = matches!(inflight.resource_type, "Document" | "Iframe")
                    && (200..300).contains(&head.status.as_u16())
                    && is_attachment(head);
                let inflight = &*inflight;
                let params = ResponseReceived {
                    frame_id: inflight.frame_id.as_deref(),
                    has_extra_info: false,
                    loader_id: &inflight.loader_id,
                    request_id: &inflight.request_id,
                    response: response_json(head, 0),
                    timestamp: *timestamp,
                    resource_type: inflight.resource_type,
                };
                Some(("Network.responseReceived", NetworkParams::ResponseReceived(params)))
            }
            NetEvent::DataReceived { id, data, timestamp } => {
                let inflight = self.inflight.get(id).filter(|i| !i.download)?;
                let params = DataReceived {
                    data_length: data.len(),
                    encoded_data_length: data.len(),
                    request_id: &inflight.request_id,
                    timestamp: *timestamp,
                };
                Some(("Network.dataReceived", NetworkParams::DataReceived(params)))
            }
            NetEvent::LoadingFinished { id, encoded_len, timestamp } => {
                let inflight = self.inflight.remove(id)?;
                if inflight.download {
                    let params = LoadingFailed {
                        canceled: true,
                        error_text: "net::ERR_ABORTED",
                        request_id: inflight.request_id,
                        timestamp: *timestamp,
                        resource_type: inflight.resource_type,
                    };
                    return Some(("Network.loadingFailed", NetworkParams::LoadingFailed(params)));
                }
                let params = LoadingFinished {
                    encoded_data_length: *encoded_len,
                    request_id: inflight.request_id,
                    timestamp: *timestamp,
                };
                Some(("Network.loadingFinished", NetworkParams::LoadingFinished(params)))
            }
            NetEvent::LoadingFailed { id, error, canceled, timestamp, .. } => {
                let inflight = self.inflight.remove(id)?;
                let params = LoadingFailed {
                    canceled: *canceled,
                    error_text: error.error_text(),
                    request_id: inflight.request_id,
                    timestamp: *timestamp,
                    resource_type: inflight.resource_type,
                };
                Some(("Network.loadingFailed", NetworkParams::LoadingFailed(params)))
            }
            NetEvent::WebSocket { id, activity, timestamp } => {
                let (method, params) = websocket_event(*id, activity, *timestamp);
                Some((method, NetworkParams::Value(params)))
            }
        }
    }

    pub fn inflight(&self) -> usize {
        self.inflight.len()
    }
}

/// `Network.webSocket*` of a connection's activity, in Chrome's shapes (what
/// Playwright's `crPage.js` reads for `page.on('websocket')`). Stateless: the
/// request id is the connection's.
fn websocket_event(id: RequestId, activity: &pyro_net::WebSocketActivity, timestamp: f64) -> (&'static str, Value) {
    use pyro_net::WebSocketActivity as A;
    let request_id = id.to_string();
    let frame = |opcode: u8, payload: &Bytes, mask: bool| {
        let data = if opcode == 1 { String::from_utf8_lossy(payload).into_owned() } else { base64_encode(payload) };
        json!({ "opcode": opcode, "mask": mask, "payloadData": data })
    };
    match activity {
        A::Created { url, .. } => (
            "Network.webSocketCreated",
            json!({ "requestId": request_id, "url": url.as_str(), "initiator": { "type": "script" } }),
        ),
        A::WillSendHandshakeRequest { headers, wall_time } => (
            "Network.webSocketWillSendHandshakeRequest",
            json!({
                "requestId": request_id,
                "timestamp": timestamp,
                "wallTime": wall_time,
                "request": { "headers": headers_json(headers) },
            }),
        ),
        A::HandshakeResponseReceived { status, status_text, headers, request_headers } => (
            "Network.webSocketHandshakeResponseReceived",
            json!({
                "requestId": request_id,
                "timestamp": timestamp,
                "response": {
                    "status": status,
                    "statusText": status_text,
                    "headers": headers_json(headers),
                    "requestHeaders": headers_json(request_headers),
                },
            }),
        ),
        A::FrameSent { opcode, payload } => (
            "Network.webSocketFrameSent",
            json!({ "requestId": request_id, "timestamp": timestamp, "response": frame(*opcode, payload, true) }),
        ),
        A::FrameReceived { opcode, payload } => (
            "Network.webSocketFrameReceived",
            json!({ "requestId": request_id, "timestamp": timestamp, "response": frame(*opcode, payload, false) }),
        ),
        A::FrameError { message } => (
            "Network.webSocketFrameError",
            json!({ "requestId": request_id, "timestamp": timestamp, "errorMessage": message }),
        ),
        A::Closed => ("Network.webSocketClosed", json!({ "requestId": request_id, "timestamp": timestamp })),
    }
}

/// Largest request body included inline in `Network.requestWillBeSent` and
/// `Fetch.requestPaused` (upstream's default `maxPostDataSize`, 64 KiB).
/// Larger bodies report only `hasPostData: true`, as Chrome and upstream do.
pub const MAX_POST_DATA: usize = 64 * 1024;

/// Fetch-domain resource type of a request destination.
pub fn fetch_resource_type(destination: pyro_net::Destination) -> ResourceType {
    use pyro_net::Destination as D;
    match destination {
        D::Document | D::Iframe => ResourceType::Document,
        // Worker main scripts were Destination::Script before they got their own
        // destinations (Sec-Fetch-Dest); CDP keeps reporting them as scripts.
        D::Script | D::Json | D::Text | D::Worker | D::SharedWorker | D::ServiceWorker => ResourceType::Script,
        D::Style => ResourceType::Stylesheet,
        D::Image => ResourceType::Image,
        D::Audio | D::Video => ResourceType::Media,
        D::Track => ResourceType::TextTrack,
        D::Font => ResourceType::Font,
        D::Xhr | D::Fetch | D::EventSource => ResourceType::XHR,
        D::WebSocket => ResourceType::WebSocket,
        D::Beacon => ResourceType::Ping,
        D::Other => ResourceType::Other,
    }
}

/// Limits for response bodies retained for `Network.getResponseBody`.
///
/// The defaults are provisional and need memory measurements on real TOS
/// workloads before they are relied on (see docs/automation/OPTIMIZATION.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BodyLimits {
    /// Bytes retained across every [`BodyStore`] sharing one [`BodyBudget`]
    /// (one budget per server process covers all connections and contexts).
    pub total: usize,
    /// Largest single body retained; larger bodies are not kept.
    pub per_body: usize,
}

impl Default for BodyLimits {
    fn default() -> Self {
        Self { total: 64 * 1024 * 1024, per_body: 8 * 1024 * 1024 }
    }
}

/// Shared byte budget for retained bodies. Cheap to clone; `Send`, so one
/// budget can span every browser thread of the process.
#[derive(Clone)]
pub struct BodyBudget(std::sync::Arc<BudgetInner>);

struct BudgetInner {
    limits: BodyLimits,
    used: std::sync::atomic::AtomicUsize,
}

impl BodyBudget {
    pub fn new(limits: BodyLimits) -> Self {
        Self(std::sync::Arc::new(BudgetInner { limits, used: std::sync::atomic::AtomicUsize::new(0) }))
    }

    pub fn limits(&self) -> BodyLimits {
        self.0.limits
    }

    /// Bytes currently retained by all stores of this budget.
    pub fn used(&self) -> usize {
        self.0.used.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn try_charge(&self, bytes: usize) -> bool {
        use std::sync::atomic::Ordering::Relaxed;
        let mut used = self.0.used.load(Relaxed);
        loop {
            let Some(next) = used.checked_add(bytes).filter(|n| *n <= self.0.limits.total) else { return false };
            match self.0.used.compare_exchange_weak(used, next, Relaxed, Relaxed) {
                Ok(_) => return true,
                Err(actual) => used = actual,
            }
        }
    }

    fn release(&self, bytes: usize) {
        self.0.used.fetch_sub(bytes, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Maximum response records per page, including empty and discarded bodies.
/// Payload byte limits alone cannot bound this bookkeeping.
pub const MAX_RETAINED_BODIES: usize = 4096;

/// Response bodies of one page for `Network.getResponseBody`.
///
/// - Capture is lazy: the engine only feeds events while a session of the page
///   has `Network` enabled, and nothing is retained before a response arrives.
/// - Bodies are shared `Bytes` chunks (no copy) charged to a [`BodyBudget`].
///   pyro-net hands out chunks of their own exact size (never slices of its
///   read buffers), and a cache hit shares the cache's body.
///   When the budget is full the store evicts its own oldest bodies; if that is
///   not enough, the new body is not retained and asking for it returns an
///   explicit error instead of stale or partial data.
/// - At most [`MAX_RETAINED_BODIES`] records are retained per page. Older
///   records are evicted even when they contain no payload bytes.
/// - Dropping or clearing the store returns its bytes to the budget.
pub struct BodyStore {
    budget: BodyBudget,
    bodies: HashMap<RequestId, Body>,
    protocol_ids: HashMap<String, RequestId>,
    order: VecDeque<RequestId>,
    retained: usize,
}

struct Body {
    protocol_id: String,
    frame_id: Option<String>,
    chunks: Vec<Bytes>,
    len: usize,
    textual: bool,
    complete: bool,
    dropped: Option<&'static str>,
}

impl BodyStore {
    pub fn new(budget: BodyBudget) -> Self {
        Self { budget, bodies: HashMap::new(), protocol_ids: HashMap::new(), order: VecDeque::new(), retained: 0 }
    }

    /// Feeds an event; only response/data/finish/fail events matter.
    pub fn observe(&mut self, event: &NetEvent) {
        match event {
            NetEvent::ResponseReceived { id, head, destination, loader_id, frame_id, .. } => {
                self.remove(*id);
                if self.bodies.len() >= MAX_RETAINED_BODIES {
                    if let Some(oldest) = self.order.front().copied() {
                        self.remove(oldest);
                    }
                }
                let textual = head.mime.starts_with("text/")
                    || head.mime.contains("json")
                    || head.mime.contains("javascript")
                    || head.mime.contains("xml")
                    || head.mime.is_empty();
                let protocol_id = protocol_request_id(*id, *destination, loader_id.as_deref());
                // Replaced captures (e.g. redirects) must not leave stale aliases.
                if let Some(previous) = self.protocol_ids.get(&protocol_id).copied() {
                    self.remove(previous);
                }
                self.protocol_ids.insert(protocol_id.clone(), *id);
                self.bodies.insert(
                    *id,
                    Body {
                        protocol_id,
                        frame_id: frame_id.clone(),
                        chunks: Vec::new(),
                        len: 0,
                        textual,
                        complete: false,
                        dropped: None,
                    },
                );
                self.order.push_back(*id);
            }
            NetEvent::DataReceived { id, data, .. } => self.append(*id, data),
            NetEvent::LoadingFinished { id, .. } => {
                if let Some(body) = self.bodies.get_mut(id) {
                    body.complete = true;
                }
            }
            NetEvent::LoadingFailed { id, .. } => self.remove(*id),
            NetEvent::RequestWillBeSent { .. } | NetEvent::WebSocket { .. } => {}
        }
    }

    fn append(&mut self, id: RequestId, data: &Bytes) {
        // Empty notifications must not allocate uncharged chunk metadata.
        if data.is_empty() {
            return;
        }
        let per_body = self.budget.limits().per_body;
        let Some(body) = self.bodies.get(&id) else { return };
        if body.dropped.is_some() {
            return;
        }
        if data.len() > per_body.saturating_sub(body.len) {
            self.drop_body(id, "Response body is too large to be retained");
            return;
        }
        while !self.budget.try_charge(data.len()) {
            // Make room from this page's oldest other bodies first.
            let victim = self.order.iter().copied().find(|r| *r != id && self.bodies.get(r).is_some_and(|b| b.len > 0));
            match victim {
                Some(victim) => self.remove(victim),
                None => {
                    self.drop_body(id, "Response body was not retained: body memory budget exhausted");
                    return;
                }
            }
        }
        let body = self.bodies.get_mut(&id).expect("checked above");
        // `Bytes` is reference counted: retaining it does not copy.
        body.chunks.push(data.clone());
        body.len += data.len();
        self.retained += data.len();
    }

    fn drop_body(&mut self, id: RequestId, reason: &'static str) {
        if let Some(body) = self.bodies.get_mut(&id) {
            self.budget.release(body.len);
            self.retained -= body.len;
            body.chunks = Vec::new();
            body.len = 0;
            body.dropped = Some(reason);
        }
    }

    fn remove(&mut self, id: RequestId) {
        if let Some(body) = self.bodies.remove(&id) {
            self.protocol_ids.remove(&body.protocol_id);
            self.budget.release(body.len);
            self.retained -= body.len;
            self.order.retain(|r| *r != id);
        }
    }

    /// A child navigation must not evict captures from its siblings or parent.
    pub fn clear_frame(&mut self, frame: &str) {
        let ids: Vec<_> = self
            .bodies
            .iter()
            .filter_map(|(id, body)| (body.frame_id.as_deref() == Some(frame)).then_some(*id))
            .collect();
        for id in ids {
            self.remove(id);
        }
    }

    /// Drops every body (navigation of the owning page, like Chrome).
    pub fn clear(&mut self) {
        self.budget.release(self.retained);
        self.retained = 0;
        self.bodies.clear();
        self.protocol_ids.clear();
        self.order.clear();
    }

    /// Bytes this store holds.
    pub fn retained_bytes(&self) -> usize {
        self.retained
    }

    /// Looks up the externally exposed request id, which is the loader id
    /// for document navigations and the native id for subresources.
    pub fn get_by_protocol_id(&self, id: &str) -> CdpResult<Value> {
        let id =
            self.protocol_ids.get(id).ok_or_else(|| CdpError::server("No resource with given identifier found"))?;
        self.get(*id)
    }

    /// `Network.getResponseBody` result: text bodies as UTF-8 strings,
    /// anything else base64 encoded.
    pub fn get(&self, id: RequestId) -> CdpResult<Value> {
        let body = self.bodies.get(&id).ok_or_else(|| CdpError::server("No resource with given identifier found"))?;
        if let Some(reason) = body.dropped {
            return Err(CdpError::server(reason));
        }
        if !body.complete {
            return Err(CdpError::server("Response body is not available yet"));
        }
        let mut bytes = Vec::with_capacity(body.len);
        for chunk in &body.chunks {
            bytes.extend_from_slice(chunk);
        }
        match String::from_utf8(bytes) {
            Ok(text) if body.textual => Ok(json!({ "body": text, "base64Encoded": false })),
            Ok(text) => Ok(json!({ "body": base64_encode(text.as_bytes()), "base64Encoded": true })),
            Err(e) => Ok(json!({ "body": base64_encode(e.as_bytes()), "base64Encoded": true })),
        }
    }
}

impl Drop for BodyStore {
    fn drop(&mut self) {
        self.budget.release(self.retained);
    }
}

/// Standard base64 with padding, as CDP carries binary bodies.
pub fn base64_encode(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { ALPHABET[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { ALPHABET[n as usize & 63] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::{Method, StatusCode};
    use pyro_net::{Destination, NetError, RequestInfo, ResponseKind, Url};

    fn tr(t: &mut NetworkTranslator, event: &NetEvent) -> Option<(&'static str, Value)> {
        t.translate(event).map(|(method, params)| (method, params.to_value()))
    }

    /// The `Value` translation the written-out params replaced, kept to
    /// check that the text is unchanged.
    mod reference {
        use super::*;

        pub fn response_json(head: &ResponseHead, encoded_len: u64) -> Value {
            let mut response = json!({
                "url": head.url.as_str(),
                "status": head.status.as_u16(),
                "statusText": head.status_text,
                "headers": headers_json(&head.headers),
                "mimeType": head.mime,
                "connectionReused": false,
                "connectionId": 0,
                "encodedDataLength": encoded_len,
                "securityState": if head.url.scheme() == "https" { "secure" } else { "neutral" },
                "fromDiskCache": head.from_cache,
                "fromServiceWorker": false,
                "protocol": if head.protocol == pyro_net::http::Version::HTTP_2 { "h2" } else { "http/1.1" },
            });
            if let Some(charset) = &head.charset {
                response["charset"] = json!(charset);
            }
            if let Some(addr) = &head.remote_addr {
                if let Some((ip, port)) = addr.rsplit_once(':') {
                    response["remoteIPAddress"] = json!(ip.trim_start_matches('[').trim_end_matches(']'));
                    response["remotePort"] = json!(port.parse::<u16>().unwrap_or(0));
                }
            }
            response
        }

        pub fn write_post_data(request: &mut Value, body: Option<&Bytes>) {
            let Some(body) = body else { return };
            request["hasPostData"] = json!(true);
            if body.len() > MAX_POST_DATA {
                return;
            }
            if let Ok(text) = std::str::from_utf8(body) {
                request["postData"] = json!(text);
            }
            request["postDataEntries"] = json!([{ "bytes": base64_encode(body) }]);
        }

        pub fn request_will_be_sent(event: &NetEvent) -> Value {
            let NetEvent::RequestWillBeSent { id, request, redirect_response, timestamp, wall_time } = event else {
                unreachable!()
            };
            let resource_type = request.destination.cdp_type();
            let mut req = json!({
                "url": request.url.as_str(),
                "method": request.method.as_str(),
                "headers": headers_json(&request.headers),
                "initialPriority": "High",
                "referrerPolicy": "strict-origin-when-cross-origin",
            });
            write_post_data(&mut req, request.post_data.as_ref());
            let document_url =
                if resource_type == "Document" { request.url.as_str().to_owned() } else { String::new() };
            let mut params = json!({
                "requestId": protocol_request_id(*id, request.destination, request.loader_id.as_deref()),
                "loaderId": request.loader_id.clone().unwrap_or_default(),
                "documentURL": document_url,
                "request": req,
                "timestamp": timestamp,
                "wallTime": wall_time,
                "initiator": { "type": "other" },
                "redirectHasExtraInfo": false,
                "type": resource_type,
            });
            if let Some(frame) = &request.frame_id {
                params["frameId"] = json!(frame);
            }
            if let Some(redirect) = redirect_response {
                params["redirectResponse"] = response_json(redirect, 0);
            }
            params
        }
    }

    /// The written-out params serialize to the very text of the `Value`s
    /// they replaced: same keys, values and key order.
    #[test]
    fn translation_text_matches_value_translation() {
        let mut headers = HeaderMap::new();
        headers.append("user-agent", "UA".parse().unwrap());
        headers.append("accept", "a".parse().unwrap());
        headers.append("sec-fetch-site", "same-origin".parse().unwrap());
        headers.append("x-multi", "1".parse().unwrap());
        headers.append("x-multi", "2".parse().unwrap());
        headers.append("x-multi", "3".parse().unwrap());
        headers.append("referer", "http://h/".parse().unwrap());
        headers.append("x-obs", http::HeaderValue::from_bytes(b"caf\xe9").unwrap());
        headers.append("accept-language", "en".parse().unwrap());
        let head = |headers: HeaderMap, charset: Option<&str>, remote: Option<&str>, h2: bool| ResponseHead {
            url: Url::parse("https://h/r").unwrap(),
            status: StatusCode::FOUND,
            status_text: "Found \"x\"".into(),
            headers,
            mime: "text/html".into(),
            charset: charset.map(Into::into),
            redirected: false,
            from_cache: true,
            kind: ResponseKind::Basic,
            remote_addr: remote.map(Into::into),
            protocol: if h2 { pyro_net::http::Version::HTTP_2 } else { pyro_net::http::Version::HTTP_11 },
            timing: Default::default(),
        };
        let heads = [
            head(headers.clone(), Some("utf-8"), Some("[::1]:8443"), true),
            head(HeaderMap::new(), None, Some("no-port"), false),
            head(headers.clone(), None, None, false),
        ];
        let bodies = [
            None,
            Some(Bytes::new()),
            Some(Bytes::from_static(b"{\"a\":\"\\u00e9\"}")),
            Some(Bytes::from_static(&[0xff, 0])),
            Some(Bytes::from(vec![b'a'; MAX_POST_DATA + 1])),
        ];
        let mut n = 0;
        for destination in [Destination::Document, Destination::Fetch] {
            for loader_id in [None, Some("L")] {
                for frame_id in [None, Some("F")] {
                    for body in &bodies {
                        for redirect in [None, Some(&heads[0]), Some(&heads[1])] {
                            n += 1;
                            let event = NetEvent::RequestWillBeSent {
                                id: RequestId(n),
                                request: Box::new(RequestInfo {
                                    url: Url::parse("http://h/api?x=1#f").unwrap(),
                                    method: Method::POST,
                                    headers: headers.clone(),
                                    post_data: body.clone(),
                                    destination,
                                    frame_id: frame_id.map(Into::into),
                                    loader_id: loader_id.map(Into::into),
                                }),
                                redirect_response: redirect.cloned().map(Box::new),
                                timestamp: 1.25,
                                wall_time: 1_700_000_000.5,
                            };
                            let mut t = NetworkTranslator::default();
                            let (_, params) = t.translate(&event).unwrap();
                            assert_eq!(
                                serde_json::to_string(&params).unwrap(),
                                serde_json::to_string(&reference::request_will_be_sent(&event)).unwrap()
                            );
                            for head in &heads {
                                assert_eq!(
                                    serde_json::to_string(&response_json(head, 7)).unwrap(),
                                    serde_json::to_string(&reference::response_json(head, 7)).unwrap()
                                );
                            }
                            let received = NetEvent::ResponseReceived {
                                id: RequestId(n),
                                head: Box::new(heads[0].clone()),
                                destination,
                                frame_id: None,
                                loader_id: None,
                                timestamp: f64::NAN,
                            };
                            let (_, params) = t.translate(&received).unwrap();
                            let mut expected = json!({
                                "requestId": protocol_request_id(RequestId(n), destination, loader_id),
                                "loaderId": loader_id.unwrap_or_default(),
                                "timestamp": f64::NAN,
                                "type": destination.cdp_type(),
                                "response": reference::response_json(&heads[0], 0),
                                "hasExtraInfo": false,
                            });
                            if let Some(frame) = frame_id {
                                expected["frameId"] = json!(frame);
                            }
                            assert_eq!(
                                serde_json::to_string(&params).unwrap(),
                                serde_json::to_string(&expected).unwrap()
                            );
                            let event = data(n, b"xyz");
                            let (_, params) = t.translate(&event).unwrap();
                            let expected = json!({
                                "requestId": protocol_request_id(RequestId(n), destination, loader_id),
                                "timestamp": 4.0,
                                "dataLength": 3,
                                "encodedDataLength": 3,
                            });
                            assert_eq!(
                                serde_json::to_string(&params).unwrap(),
                                serde_json::to_string(&expected).unwrap()
                            );
                            let event = finished(n);
                            let (_, params) = t.translate(&event).unwrap();
                            let expected = json!({
                                "requestId": protocol_request_id(RequestId(n), destination, loader_id),
                                "timestamp": 5.0,
                                "encodedDataLength": 7,
                            });
                            assert_eq!(
                                serde_json::to_string(&params).unwrap(),
                                serde_json::to_string(&expected).unwrap()
                            );
                        }
                    }
                }
            }
        }
    }

    /// `Fetch.requestPaused`'s request: `hasPostData` always, the rest as in
    /// `Network.requestWillBeSent`.
    #[test]
    fn paused_request_text_matches_value_translation() {
        let mut headers = HeaderMap::new();
        headers.append("b", "1".parse().unwrap());
        headers.append("a", "2".parse().unwrap());
        headers.append("b", "3".parse().unwrap());
        for body in [None, Some(Bytes::from_static(b"x=1")), Some(Bytes::from(vec![0u8; MAX_POST_DATA + 1]))] {
            let info = RequestInfo {
                url: Url::parse("http://h/p").unwrap(),
                method: Method::PUT,
                headers: headers.clone(),
                post_data: body.clone(),
                destination: Destination::Fetch,
                frame_id: None,
                loader_id: None,
            };
            let mut expected = json!({"url":info.url.as_str(),"method":info.method.as_str(),"headers":headers_json(&info.headers),
                "hasPostData":false});
            reference::write_post_data(&mut expected, body.as_ref());
            assert_eq!(
                serde_json::to_string(&PausedRequestJson::new(&info)).unwrap(),
                serde_json::to_string(&expected).unwrap()
            );
        }
    }

    fn request(id: u64, destination: Destination) -> NetEvent {
        let mut headers = HeaderMap::new();
        headers.append("accept", "a".parse().unwrap());
        headers.append("accept", "b".parse().unwrap());
        NetEvent::RequestWillBeSent {
            id: RequestId(id),
            request: Box::new(RequestInfo {
                url: Url::parse("http://h/api?x=1").unwrap(),
                method: Method::POST,
                headers,
                post_data: Some(Bytes::from_static(b"{\"a\":1}")),
                destination,
                frame_id: Some("F".into()),
                loader_id: Some("L".into()),
            }),
            redirect_response: None,
            timestamp: 1.0,
            wall_time: 2.0,
        }
    }

    fn response(id: u64, mime: &str) -> NetEvent {
        NetEvent::ResponseReceived {
            id: RequestId(id),
            head: Box::new(ResponseHead {
                url: Url::parse("http://h/api?x=1").unwrap(),
                status: StatusCode::OK,
                status_text: "OK".into(),
                headers: HeaderMap::new(),
                mime: mime.into(),
                charset: None,
                redirected: false,
                from_cache: false,
                kind: ResponseKind::Basic,
                remote_addr: Some("127.0.0.1:8080".into()),
                protocol: pyro_net::http::Version::HTTP_11,
                timing: Default::default(),
            }),
            destination: Destination::Fetch,
            frame_id: Some("F".into()),
            loader_id: Some("L".into()),
            timestamp: 3.0,
        }
    }

    fn data(id: u64, bytes: &'static [u8]) -> NetEvent {
        NetEvent::DataReceived { id: RequestId(id), data: Bytes::from_static(bytes), timestamp: 4.0 }
    }

    fn finished(id: u64) -> NetEvent {
        NetEvent::LoadingFinished { id: RequestId(id), encoded_len: 7, timestamp: 5.0 }
    }

    #[test]
    fn translates_a_request_lifecycle() {
        let mut t = NetworkTranslator::default();
        let (m, p) = tr(&mut t, &request(7, Destination::Fetch)).unwrap();
        assert_eq!(m, "Network.requestWillBeSent");
        assert_eq!(p["requestId"], "7");
        assert_eq!(p["type"], "Fetch");
        assert_eq!(p["frameId"], "F");
        assert_eq!(p["request"]["headers"]["accept"], "a\nb");
        assert_eq!(p["request"]["postData"], "{\"a\":1}");
        let (m, p) = tr(&mut t, &response(7, "application/json")).unwrap();
        assert_eq!(m, "Network.responseReceived");
        assert_eq!(p["response"]["status"], 200);
        assert_eq!(p["response"]["remotePort"], 8080);
        assert_eq!(p["loaderId"], "L");
        assert_eq!(tr(&mut t, &data(7, b"x")).unwrap().0, "Network.dataReceived");
        assert_eq!(tr(&mut t, &finished(7)).unwrap().0, "Network.loadingFinished");
        assert_eq!(t.inflight(), 0);
        assert!(tr(&mut t, &finished(7)).is_none(), "unknown request ignored");
    }

    #[test]
    fn websocket_activity_translates_to_chrome_events() {
        use pyro_net::WebSocketActivity as A;
        let mut t = NetworkTranslator::default();
        let ws = |activity| NetEvent::WebSocket { id: RequestId(4), activity: Box::new(activity), timestamp: 2.0 };
        let (m, p) =
            tr(&mut t, &ws(A::Created { url: "wss://h/s".parse().unwrap(), frame_id: Some("F".into()) })).unwrap();
        assert_eq!(
            (m, p["requestId"].as_str(), p["url"].as_str()),
            ("Network.webSocketCreated", Some("4"), Some("wss://h/s"))
        );
        let mut headers = HeaderMap::new();
        headers.insert("upgrade", "websocket".parse().unwrap());
        let (m, p) = tr(&mut t, &ws(A::WillSendHandshakeRequest { headers: headers.clone(), wall_time: 9.0 })).unwrap();
        assert_eq!(m, "Network.webSocketWillSendHandshakeRequest");
        assert_eq!(p["request"]["headers"]["upgrade"], "websocket");
        let (m, p) = tr(
            &mut t,
            &ws(A::HandshakeResponseReceived {
                status: 101,
                status_text: "Switching Protocols".into(),
                headers,
                request_headers: HeaderMap::new(),
            }),
        )
        .unwrap();
        assert_eq!((m, p["response"]["status"].as_u64()), ("Network.webSocketHandshakeResponseReceived", Some(101)));
        let (m, p) = tr(&mut t, &ws(A::FrameSent { opcode: 1, payload: "hi".into() })).unwrap();
        assert_eq!(m, "Network.webSocketFrameSent");
        assert_eq!(p["response"], json!({"opcode": 1, "mask": true, "payloadData": "hi"}));
        let (m, p) = tr(&mut t, &ws(A::FrameReceived { opcode: 2, payload: vec![0xff, 0].into() })).unwrap();
        assert_eq!(m, "Network.webSocketFrameReceived");
        assert_eq!(p["response"], json!({"opcode": 2, "mask": false, "payloadData": "/wA="}));
        let (m, p) = tr(&mut t, &ws(A::FrameError { message: "boom".into() })).unwrap();
        assert_eq!((m, p["errorMessage"].as_str()), ("Network.webSocketFrameError", Some("boom")));
        let (m, p) = tr(&mut t, &ws(A::Closed)).unwrap();
        assert_eq!((m, p["timestamp"].as_f64()), ("Network.webSocketClosed", Some(2.0)));
        assert_eq!(t.inflight(), 0, "connections keep no translator state");
    }

    #[test]
    fn failures_carry_error_text() {
        let mut t = NetworkTranslator::default();
        tr(&mut t, &request(1, Destination::Document));
        let failed = NetEvent::LoadingFailed {
            id: RequestId(1),
            error: NetError::ConnectionRefused,
            canceled: false,
            destination: Destination::Document,
            timestamp: 9.0,
        };
        let (m, p) = tr(&mut t, &failed).unwrap();
        assert_eq!(m, "Network.loadingFailed");
        assert_eq!(p["errorText"], "net::ERR_CONNECTION_REFUSED");
        assert_eq!(p["type"], "Document");
    }

    /// `loadingFailed` is written out like the other per-request events:
    /// the very text of the `Value` it replaced, for a failure and for a
    /// download (a finished attachment navigation reported as aborted).
    #[test]
    fn loading_failed_text_matches_value_translation() {
        let mut t = NetworkTranslator::default();
        tr(&mut t, &request(1, Destination::Fetch));
        let failed = NetEvent::LoadingFailed {
            id: RequestId(1),
            error: NetError::Blocked("net::ERR_BLOCKED_BY_CLIENT".into()),
            canceled: true,
            destination: Destination::Fetch,
            timestamp: 9.5,
        };
        let text = serde_json::to_string(&t.translate(&failed).unwrap().1).unwrap();
        let expected = json!({
            "requestId": "1",
            "timestamp": 9.5,
            "type": "Fetch",
            "errorText": "net::ERR_BLOCKED_BY_CLIENT",
            "canceled": true,
        });
        assert_eq!(text, serde_json::to_string(&expected).unwrap());

        tr(&mut t, &request(2, Destination::Document));
        let mut attachment = response(2, "application/octet-stream");
        if let NetEvent::ResponseReceived { head, .. } = &mut attachment {
            head.headers.insert("content-disposition", "attachment; filename=x.bin".parse().unwrap());
        }
        tr(&mut t, &attachment).unwrap();
        assert!(tr(&mut t, &data(2, b"x")).is_none(), "a download has no dataReceived");
        let finished = finished(2);
        let (m, text) = {
            let (m, params) = t.translate(&finished).unwrap();
            (m, serde_json::to_string(&params).unwrap())
        };
        assert_eq!(m, "Network.loadingFailed");
        let expected = json!({
            "requestId": "L",
            "timestamp": 5.0,
            "type": "Document",
            "errorText": "net::ERR_ABORTED",
            "canceled": true,
        });
        assert_eq!(text, serde_json::to_string(&expected).unwrap());
    }

    fn store(total: usize, per_body: usize) -> (BodyBudget, BodyStore) {
        let budget = BodyBudget::new(BodyLimits { total, per_body });
        (budget.clone(), BodyStore::new(budget))
    }

    #[test]
    fn bodies_are_text_or_base64_and_complete_only() {
        let (_, mut s) = store(100, 100);
        s.observe(&response(2, "image/png"));
        s.observe(&data(2, &[0xff, 0x00]));
        assert!(s.get(RequestId(2)).is_err(), "not complete yet");
        s.observe(&finished(2));
        assert_eq!(s.get(RequestId(2)).unwrap(), json!({"body":"/wA=","base64Encoded":true}));
        s.observe(&response(3, "text/plain"));
        s.observe(&data(3, b"hello"));
        s.observe(&finished(3));
        assert_eq!(s.get(RequestId(3)).unwrap(), json!({"body":"hello","base64Encoded":false}));
        assert!(s.get(RequestId(9)).is_err(), "unknown id");
    }

    #[test]
    fn oversized_bodies_are_not_retained_and_release_budget() {
        let (budget, mut s) = store(100, 6);
        s.observe(&response(1, "application/json"));
        s.observe(&data(1, b"{\"a\""));
        assert_eq!(budget.used(), 4);
        s.observe(&data(1, b":1}"));
        s.observe(&finished(1));
        assert!(s.get(RequestId(1)).unwrap_err().message.contains("too large"));
        assert_eq!(budget.used(), 0);
        assert_eq!(s.retained_bytes(), 0);
    }

    #[test]
    fn budget_evicts_own_oldest_then_refuses() {
        let (budget, mut s) = store(10, 10);
        s.observe(&response(1, "text/plain"));
        s.observe(&data(1, b"aaaaaa"));
        s.observe(&finished(1));
        s.observe(&response(2, "text/plain"));
        s.observe(&data(2, b"bbbbbb"));
        s.observe(&finished(2));
        assert!(s.get(RequestId(1)).is_err(), "oldest evicted to make room");
        assert_eq!(s.get(RequestId(2)).unwrap()["body"], "bbbbbb");
        assert!(budget.used() <= 10);
    }

    #[test]
    fn budget_is_shared_across_contexts_and_isolated_per_store() {
        let budget = BodyBudget::new(BodyLimits { total: 10, per_body: 10 });
        let mut a = BodyStore::new(budget.clone());
        let mut b = BodyStore::new(budget.clone());
        a.observe(&response(1, "text/plain"));
        a.observe(&data(1, b"aaaaaaaa"));
        a.observe(&finished(1));
        // b cannot evict a's bodies: its new body is refused with an explicit error.
        b.observe(&response(1, "text/plain"));
        b.observe(&data(1, b"bbbbbbbb"));
        b.observe(&finished(1));
        assert_eq!(a.get(RequestId(1)).unwrap()["body"], "aaaaaaaa", "same request id, different store");
        assert!(b.get(RequestId(1)).unwrap_err().message.contains("budget exhausted"));
        assert_eq!(budget.used(), 8, "aggregate stays bounded");
        drop(a);
        assert_eq!(budget.used(), 0, "teardown returns bytes");
        b.observe(&response(2, "text/plain"));
        b.observe(&data(2, b"bbbbbbbb"));
        b.observe(&finished(2));
        assert_eq!(b.get(RequestId(2)).unwrap()["body"], "bbbbbbbb");
        b.clear();
        assert_eq!(budget.used(), 0);
    }

    #[test]
    fn empty_and_discarded_response_records_are_bounded() {
        let (budget, mut s) = store(10, 0);
        for id in 0..4098 {
            s.observe(&response(id, "text/plain"));
            if id % 2 == 0 {
                s.observe(&data(id, b"x"));
            }
            s.observe(&finished(id));
        }
        assert_eq!(budget.used(), 0);
        assert!(s.bodies.len() <= 4096, "zero-byte records need a separate bound");
        assert!(s.get(RequestId(1)).is_err(), "oldest empty response evicted");
        assert_eq!(s.get(RequestId(4095)).unwrap()["body"], "");
        assert!(s.get(RequestId(4096)).unwrap_err().message.contains("too large"));
    }

    #[test]
    fn empty_data_events_do_not_retain_chunks() {
        let (budget, mut s) = store(10, 10);
        s.observe(&response(1, "text/plain"));
        s.observe(&data(1, b""));
        s.observe(&data(1, b"hello"));
        s.observe(&data(1, b""));
        s.observe(&finished(1));
        assert_eq!(s.bodies[&RequestId(1)].chunks.len(), 1);
        assert_eq!(budget.used(), 5);
        assert_eq!(s.get(RequestId(1)).unwrap()["body"], "hello");
    }

    #[test]
    fn failed_requests_release_their_bytes() {
        let (budget, mut s) = store(100, 100);
        s.observe(&response(1, "text/plain"));
        s.observe(&data(1, b"partial"));
        s.observe(&NetEvent::LoadingFailed {
            id: RequestId(1),
            error: NetError::Aborted,
            canceled: true,
            destination: Destination::Fetch,
            timestamp: 1.0,
        });
        assert_eq!(budget.used(), 0);
        assert!(s.get(RequestId(1)).is_err());
    }

    #[test]
    fn post_data_entries_carry_exact_bytes() {
        // The post data fields of a `Network.requestWillBeSent` request.
        let write = |body: Option<Bytes>| {
            let info = RequestInfo {
                url: Url::parse("http://h/").unwrap(),
                method: Method::POST,
                headers: HeaderMap::new(),
                post_data: body,
                destination: Destination::Fetch,
                frame_id: None,
                loader_id: None,
            };
            let mut request = serde_json::to_value(request_json(&info)).unwrap();
            let fields = request.as_object_mut().unwrap();
            fields.retain(|name, _| name.contains("PostData") || name.starts_with("postData"));
            request
        };
        assert_eq!(write(None), json!({}), "no body: nothing added");
        assert_eq!(
            write(Some(Bytes::from_static(br#"{"credentialType":"CLIENT"}"#))),
            json!({"hasPostData":true,"postData":"{\"credentialType\":\"CLIENT\"}","postDataEntries":[{"bytes":"eyJjcmVkZW50aWFsVHlwZSI6IkNMSUVOVCJ9"}]})
        );
        assert_eq!(
            write(Some(Bytes::from_static(&[0xff, 0x00, 0xfe]))),
            json!({"hasPostData":true,"postDataEntries":[{"bytes":"/wD+"}]}),
            "non-UTF-8 body: no lossy text, exact bytes in the entry"
        );
        assert_eq!(
            write(Some(Bytes::new())),
            json!({"hasPostData":true,"postData":"","postDataEntries":[{"bytes":""}]}),
            "empty body"
        );
        let at_limit = write(Some(Bytes::from(vec![b'a'; MAX_POST_DATA])));
        assert_eq!(at_limit["postDataEntries"][0]["bytes"].as_str().unwrap().len(), MAX_POST_DATA.div_ceil(3) * 4);
        assert_eq!(
            write(Some(Bytes::from(vec![b'a'; MAX_POST_DATA + 1]))),
            json!({"hasPostData":true}),
            "over the bound: only hasPostData, like upstream and Chrome"
        );
    }

    #[test]
    fn request_will_be_sent_includes_post_data_entries() {
        let mut t = NetworkTranslator::default();
        let (_, p) = tr(&mut t, &request(9, Destination::Fetch)).unwrap();
        assert_eq!(p["request"]["postDataEntries"], json!([{"bytes":"eyJhIjoxfQ=="}]));
        assert_eq!(p["request"]["postData"], "{\"a\":1}");
        assert_eq!(p["request"]["hasPostData"], true);
    }
}
