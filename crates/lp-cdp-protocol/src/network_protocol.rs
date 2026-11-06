//! `Network` domain translation of `lp_net::NetEvent`s and the response-body
//! store behind `Network.getResponseBody`.
//!
//! Event shapes follow upstream `src/server/cdp/domains/network.zig` and what
//! Playwright 1.59's `crNetworkManager.js` reads. Wiring to a browser
//! context's `NetClient` lives in the engine host.

use std::collections::{HashMap, VecDeque};

use bytes::Bytes;
use http::HeaderMap;
use lp_net::{NetEvent, RequestId, ResponseHead};
use serde_json::{json, Map, Value};

use crate::fetch_protocol::ResourceType;
use crate::msg::{CdpError, CdpResult};

/// CDP `Network.Headers`: an object; repeated headers are joined with "\n"
/// as Chrome does.
pub fn headers_json(headers: &HeaderMap) -> Value {
    let mut map = Map::new();
    for (name, value) in headers {
        let value = String::from_utf8_lossy(value.as_bytes()).into_owned();
        match map.get_mut(name.as_str()) {
            Some(Value::String(existing)) => {
                existing.push('\n');
                existing.push_str(&value);
            }
            _ => {
                map.insert(name.as_str().to_owned(), Value::String(value));
            }
        }
    }
    Value::Object(map)
}

fn response_json(head: &ResponseHead, encoded_len: u64) -> Value {
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
        "protocol": if head.protocol == lp_net::http::Version::HTTP_2 { "h2" } else { "http/1.1" },
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

/// Chromium identifies a document navigation by requestId == loaderId.
/// Native network ids remain context-local and are used internally.
pub fn protocol_request_id(id: RequestId, destination: lp_net::Destination, loader_id: Option<&str>) -> String {
    if matches!(destination, lp_net::Destination::Document | lp_net::Destination::Iframe) {
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

fn is_attachment(head: &lp_net::ResponseHead) -> bool {
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
    pub fn translate(&mut self, event: &NetEvent) -> Option<(&'static str, Value)> {
        match event {
            NetEvent::RequestWillBeSent { id, request, redirect_response, timestamp, wall_time } => {
                let resource_type = request.destination.cdp_type();
                let loader_id = request.loader_id.clone().unwrap_or_default();
                if !self.inflight.contains_key(id) && self.inflight.len() >= MAX_INFLIGHT {
                    return None;
                }
                let request_id = protocol_request_id(*id, request.destination, request.loader_id.as_deref());
                self.inflight.insert(
                    *id,
                    Inflight {
                        request_id: request_id.clone(),
                        loader_id: loader_id.clone(),
                        frame_id: request.frame_id.clone(),
                        resource_type,
                        download: false,
                    },
                );
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
                    "requestId": request_id,
                    "loaderId": loader_id,
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
                Some(("Network.requestWillBeSent", params))
            }
            NetEvent::ResponseReceived { id, head, timestamp, .. } => {
                let inflight = self.inflight.get_mut(id)?;
                inflight.download = matches!(inflight.resource_type, "Document" | "Iframe")
                    && (200..300).contains(&head.status.as_u16())
                    && is_attachment(head);
                let inflight = &*inflight;
                let mut params = json!({
                    "requestId": inflight.request_id,
                    "loaderId": inflight.loader_id,
                    "timestamp": timestamp,
                    "type": inflight.resource_type,
                    "response": response_json(head, 0),
                    "hasExtraInfo": false,
                });
                if let Some(frame) = &inflight.frame_id {
                    params["frameId"] = json!(frame);
                }
                Some(("Network.responseReceived", params))
            }
            NetEvent::DataReceived { id, data, timestamp } => {
                let inflight = self.inflight.get(id).filter(|i| !i.download)?;
                Some((
                    "Network.dataReceived",
                    json!({
                        "requestId": inflight.request_id,
                        "timestamp": timestamp,
                        "dataLength": data.len(),
                        "encodedDataLength": data.len(),
                    }),
                ))
            }
            NetEvent::LoadingFinished { id, encoded_len, timestamp } => {
                let inflight = self.inflight.remove(id)?;
                if inflight.download {
                    return Some((
                        "Network.loadingFailed",
                        json!({
                            "requestId": inflight.request_id,
                            "timestamp": timestamp,
                            "type": inflight.resource_type,
                            "errorText": "net::ERR_ABORTED",
                            "canceled": true,
                        }),
                    ));
                }
                Some((
                    "Network.loadingFinished",
                    json!({ "requestId": inflight.request_id, "timestamp": timestamp, "encodedDataLength": encoded_len }),
                ))
            }
            NetEvent::LoadingFailed { id, error, canceled, timestamp, .. } => {
                let inflight = self.inflight.remove(id)?;
                Some((
                    "Network.loadingFailed",
                    json!({
                        "requestId": inflight.request_id,
                        "timestamp": timestamp,
                        "type": inflight.resource_type,
                        "errorText": error.error_text(),
                        "canceled": canceled,
                    }),
                ))
            }
        }
    }

    pub fn inflight(&self) -> usize {
        self.inflight.len()
    }
}

/// Largest request body included inline in `Network.requestWillBeSent` and
/// `Fetch.requestPaused` (upstream's default `maxPostDataSize`, 64 KiB).
/// Larger bodies report only `hasPostData: true`, as Chrome and upstream do.
pub const MAX_POST_DATA: usize = 64 * 1024;

/// Sets `hasPostData`, `postData` and `postDataEntries` on a CDP
/// `Network.Request` object.
///
/// Playwright builds `request.postData()` / `postDataJSON()` only from
/// `postDataEntries[].bytes` (base64), so the binary-safe entry must be
/// present. `postData` is the deprecated text form; it is omitted for bodies
/// that are not valid UTF-8 rather than sent lossily.
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

/// Fetch-domain resource type of a request destination.
pub fn fetch_resource_type(destination: lp_net::Destination) -> ResourceType {
    use lp_net::Destination as D;
    match destination {
        D::Document | D::Iframe => ResourceType::Document,
        D::Script => ResourceType::Script,
        D::Style => ResourceType::Stylesheet,
        D::Image => ResourceType::Image,
        D::Font => ResourceType::Font,
        D::Xhr | D::Fetch | D::EventSource => ResourceType::XHR,
        D::WebSocket => ResourceType::WebSocket,
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
///   lp-net hands out chunks of their own exact size (never slices of its
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
            NetEvent::RequestWillBeSent { .. } => {}
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
    use lp_net::{Destination, NetError, RequestInfo, ResponseKind, Url};

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
                protocol: lp_net::http::Version::HTTP_11,
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
        let (m, p) = t.translate(&request(7, Destination::Fetch)).unwrap();
        assert_eq!(m, "Network.requestWillBeSent");
        assert_eq!(p["requestId"], "7");
        assert_eq!(p["type"], "Fetch");
        assert_eq!(p["frameId"], "F");
        assert_eq!(p["request"]["headers"]["accept"], "a\nb");
        assert_eq!(p["request"]["postData"], "{\"a\":1}");
        let (m, p) = t.translate(&response(7, "application/json")).unwrap();
        assert_eq!(m, "Network.responseReceived");
        assert_eq!(p["response"]["status"], 200);
        assert_eq!(p["response"]["remotePort"], 8080);
        assert_eq!(p["loaderId"], "L");
        assert_eq!(t.translate(&data(7, b"x")).unwrap().0, "Network.dataReceived");
        assert_eq!(t.translate(&finished(7)).unwrap().0, "Network.loadingFinished");
        assert_eq!(t.inflight(), 0);
        assert!(t.translate(&finished(7)).is_none(), "unknown request ignored");
    }

    #[test]
    fn failures_carry_error_text() {
        let mut t = NetworkTranslator::default();
        t.translate(&request(1, Destination::Document));
        let failed = NetEvent::LoadingFailed {
            id: RequestId(1),
            error: NetError::ConnectionRefused,
            canceled: false,
            destination: Destination::Document,
            timestamp: 9.0,
        };
        let (m, p) = t.translate(&failed).unwrap();
        assert_eq!(m, "Network.loadingFailed");
        assert_eq!(p["errorText"], "net::ERR_CONNECTION_REFUSED");
        assert_eq!(p["type"], "Document");
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
        let write = |body: Option<Bytes>| {
            let mut request = json!({});
            write_post_data(&mut request, body.as_ref());
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
        let (_, p) = t.translate(&request(9, Destination::Fetch)).unwrap();
        assert_eq!(p["request"]["postDataEntries"], json!([{"bytes":"eyJhIjoxfQ=="}]));
        assert_eq!(p["request"]["postData"], "{\"a\":1}");
        assert_eq!(p["request"]["hasPostData"], true);
    }
}
