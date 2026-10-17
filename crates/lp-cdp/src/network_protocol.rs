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
        "protocol": "http/1.1",
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

/// Per-request facts needed by later events of the same request.
#[derive(Debug, Clone)]
struct Inflight {
    loader_id: String,
    frame_id: Option<String>,
    resource_type: &'static str,
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
                self.inflight.insert(
                    *id,
                    Inflight { loader_id: loader_id.clone(), frame_id: request.frame_id.clone(), resource_type },
                );
                let mut req = json!({
                    "url": request.url.as_str(),
                    "method": request.method.as_str(),
                    "headers": headers_json(&request.headers),
                    "initialPriority": "High",
                    "referrerPolicy": "strict-origin-when-cross-origin",
                });
                if let Some(body) = &request.post_data {
                    req["hasPostData"] = json!(true);
                    if let Ok(text) = std::str::from_utf8(body) {
                        req["postData"] = json!(text);
                    }
                }
                let document_url =
                    if resource_type == "Document" { request.url.as_str().to_owned() } else { String::new() };
                let mut params = json!({
                    "requestId": id.to_string(),
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
                let inflight = self.inflight.get(id)?;
                let mut params = json!({
                    "requestId": id.to_string(),
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
                self.inflight.get(id)?;
                Some((
                    "Network.dataReceived",
                    json!({
                        "requestId": id.to_string(),
                        "timestamp": timestamp,
                        "dataLength": data.len(),
                        "encodedDataLength": data.len(),
                    }),
                ))
            }
            NetEvent::LoadingFinished { id, encoded_len, timestamp } => {
                self.inflight.remove(id)?;
                Some((
                    "Network.loadingFinished",
                    json!({ "requestId": id.to_string(), "timestamp": timestamp, "encodedDataLength": encoded_len }),
                ))
            }
            NetEvent::LoadingFailed { id, error, canceled, timestamp, .. } => {
                let inflight = self.inflight.remove(id)?;
                Some((
                    "Network.loadingFailed",
                    json!({
                        "requestId": id.to_string(),
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

/// Response bodies retained for `Network.getResponseBody`, bounded by total
/// bytes (oldest bodies evicted first) and by a per-response cap (larger
/// bodies are not retained; asking for them returns an explicit error).
pub struct BodyStore {
    bodies: HashMap<RequestId, Body>,
    order: VecDeque<RequestId>,
    total: usize,
    total_cap: usize,
    per_body_cap: usize,
}

struct Body {
    chunks: Vec<Bytes>,
    len: usize,
    mime: String,
    complete: bool,
    oversized: bool,
}

pub const DEFAULT_BODY_TOTAL_CAP: usize = 100 * 1024 * 1024;
pub const DEFAULT_BODY_CAP: usize = 10 * 1024 * 1024;

impl Default for BodyStore {
    fn default() -> Self {
        Self::new(DEFAULT_BODY_TOTAL_CAP, DEFAULT_BODY_CAP)
    }
}

impl BodyStore {
    pub fn new(total_cap: usize, per_body_cap: usize) -> Self {
        Self { bodies: HashMap::new(), order: VecDeque::new(), total: 0, total_cap, per_body_cap }
    }

    /// Feeds an event; only response/data/finish/fail events matter.
    pub fn observe(&mut self, event: &NetEvent) {
        match event {
            NetEvent::ResponseReceived { id, head, .. } => {
                self.remove(*id);
                self.bodies.insert(
                    *id,
                    Body { chunks: Vec::new(), len: 0, mime: head.mime.clone(), complete: false, oversized: false },
                );
                self.order.push_back(*id);
            }
            NetEvent::DataReceived { id, data, .. } => {
                let Some(body) = self.bodies.get_mut(id) else { return };
                if body.oversized {
                    return;
                }
                if body.len + data.len() > self.per_body_cap {
                    self.total -= body.len;
                    body.chunks = Vec::new();
                    body.len = 0;
                    body.oversized = true;
                    return;
                }
                // `Bytes` is reference counted: retaining it does not copy.
                body.chunks.push(data.clone());
                body.len += data.len();
                self.total += data.len();
                self.evict();
            }
            NetEvent::LoadingFinished { id, .. } => {
                if let Some(body) = self.bodies.get_mut(id) {
                    body.complete = true;
                }
            }
            NetEvent::LoadingFailed { id, .. } => self.remove(*id),
            NetEvent::RequestWillBeSent { .. } => {}
        }
    }

    fn evict(&mut self) {
        while self.total > self.total_cap {
            let Some(oldest) = self.order.pop_front() else { break };
            if let Some(body) = self.bodies.remove(&oldest) {
                self.total -= body.len;
            }
        }
    }

    fn remove(&mut self, id: RequestId) {
        if let Some(body) = self.bodies.remove(&id) {
            self.total -= body.len;
            self.order.retain(|r| *r != id);
        }
    }

    /// Drops every body (navigation of the owning page, like Chrome).
    pub fn clear(&mut self) {
        self.bodies.clear();
        self.order.clear();
        self.total = 0;
    }

    pub fn retained_bytes(&self) -> usize {
        self.total
    }

    /// `Network.getResponseBody` result: text bodies as UTF-8 strings,
    /// anything else base64 encoded.
    pub fn get(&self, id: RequestId) -> CdpResult<Value> {
        let body = self.bodies.get(&id).ok_or_else(|| CdpError::server("No resource with given identifier found"))?;
        if body.oversized {
            return Err(CdpError::server("Response body is too large to be retained"));
        }
        if !body.complete {
            return Err(CdpError::server("Response body is not available yet"));
        }
        let mut bytes = Vec::with_capacity(body.len);
        for chunk in &body.chunks {
            bytes.extend_from_slice(chunk);
        }
        let textual = body.mime.starts_with("text/")
            || body.mime.contains("json")
            || body.mime.contains("javascript")
            || body.mime.contains("xml")
            || body.mime.is_empty();
        match String::from_utf8(bytes) {
            Ok(text) if textual => Ok(json!({ "body": text, "base64Encoded": false })),
            Ok(text) => Ok(json!({ "body": base64_encode(text.as_bytes()), "base64Encoded": true })),
            Err(e) => Ok(json!({ "body": base64_encode(e.as_bytes()), "base64Encoded": true })),
        }
    }
}

fn base64_encode(input: &[u8]) -> String {
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

    #[test]
    fn bodies_are_bounded_and_text_or_base64() {
        let mut s = BodyStore::new(10, 6);
        s.observe(&response(1, "application/json"));
        s.observe(&data(1, b"{\"a\""));
        assert!(s.get(RequestId(1)).is_err(), "not complete yet");
        s.observe(&data(1, b":1}"));
        assert!(s.get(RequestId(1)).unwrap_err().message.contains("too large"), "per-body cap");
        assert_eq!(s.retained_bytes(), 0);

        s.observe(&response(2, "image/png"));
        s.observe(&data(2, &[0xff, 0x00]));
        s.observe(&finished(2));
        assert_eq!(s.get(RequestId(2)).unwrap(), json!({"body":"/wA=","base64Encoded":true}));

        s.observe(&response(3, "text/plain"));
        s.observe(&data(3, b"hello"));
        s.observe(&finished(3));
        assert_eq!(s.get(RequestId(3)).unwrap(), json!({"body":"hello","base64Encoded":false}));

        s.observe(&response(4, "text/plain"));
        s.observe(&data(4, b"world!"));
        assert!(s.retained_bytes() <= 10, "total cap evicts oldest");
        assert!(s.get(RequestId(2)).is_err(), "oldest evicted");
        s.clear();
        assert_eq!(s.retained_bytes(), 0);
    }
}
