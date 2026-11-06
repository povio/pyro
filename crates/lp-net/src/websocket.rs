//! WebSocket client connections (RFC 6455) on a context's I/O thread.
//!
//! A connection is one task on the I/O thread's `LocalSet`: it opens the
//! transport through the context's connector stack (TCP, the context proxy as
//! a CONNECT tunnel, rustls for `wss:`), sends the opening handshake with the
//! context's cookies, user agent and extra headers over hyper's HTTP/1.1
//! upgrade, then reads frames and writes the owner's messages until the
//! closing handshake ends. The owner (a page or a worker) only sees
//! [`WsEvent`]s through its sink, which posts them to its event loop, and
//! talks back through a [`WebSocketHandle`]; nothing polls.
//!
//! Not implemented: extensions (permessage-deflate is not offered, so
//! `extensions` is always empty) and HTTP/2 WebSockets (RFC 8441). Redirects
//! fail the connection, as in browsers.

use std::time::Duration;

use bytes::{Buf, Bytes, BytesMut};
use http::{header, HeaderMap, HeaderValue, Method};
use http_body_util::Empty;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use url::Url;

use super::{tls_config, tls_config_insecure, wall_time, Cmd, Io, NetClient, NetEvent};
use crate::cookies::{site_context, SiteContext};
use crate::proxy::{ProxyConnector, ProxyError};
use crate::request::{RequestDefaults, RequestId};

/// How long the client waits for the server's Close frame after sending its
/// own (Chromium's closing handshake timeout).
const CLOSING_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(60);
/// How long the client waits for the server to close the TCP connection once
/// both Close frames were exchanged (Chromium's underlying connection close
/// timeout).
const CONNECTION_CLOSE_TIMEOUT: Duration = Duration::from_secs(2);
const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// What [`NetClient::websocket`] opens.
#[derive(Clone, Debug)]
pub struct WebSocketRequest {
    /// A `ws:` or `wss:` URL without a fragment. User info, if any, becomes
    /// Basic credentials and is not sent in the URL.
    pub url: Url,
    /// Requested subprotocols, already validated by the caller.
    pub protocols: Vec<String>,
    /// Serialized origin of the client (`Origin`, cookie site checks).
    pub origin: String,
    /// Frame of the creating document, for CDP routing.
    pub frame_id: Option<String>,
    /// The owner's user agent, language and extra headers.
    pub defaults: RequestDefaults,
}

/// A connection's events, in order. `Closed` is always the last one, and
/// every connection gets exactly one unless its handle was dropped first.
#[derive(Debug)]
pub enum WsEvent {
    /// The opening handshake succeeded. `headers` are the handshake
    /// response's (the owner stores their cookies when it handles the event).
    Open {
        protocol: String,
        extensions: String,
        headers: HeaderMap,
    },
    Text(String),
    Binary(Bytes),
    /// Payload bytes of data messages written to the transport since the
    /// last such event (`bufferedAmount` decreases by them).
    Sent(usize),
    /// The connection is closed. `failed`: the client failed it (handshake or
    /// protocol error), which fires `error` before `close`.
    Closed {
        clean: bool,
        code: u16,
        reason: String,
        failed: bool,
    },
}

/// Consumer of one connection's events; wrap it to post to the owner's loop.
pub type WsSink = Box<dyn FnMut(WsEvent) + Send>;

enum WsCommand {
    Text(String),
    Binary(Bytes),
    Close(Option<u16>, String),
}

/// The owner's end of a connection. Dropping it aborts the connection at
/// once, without further events.
#[derive(Debug)]
pub struct WebSocketHandle {
    tx: mpsc::UnboundedSender<WsCommand>,
}

impl WebSocketHandle {
    pub fn send_text(&self, text: String) {
        let _ = self.tx.send(WsCommand::Text(text));
    }
    pub fn send_binary(&self, bytes: Bytes) {
        let _ = self.tx.send(WsCommand::Binary(bytes));
    }
    /// Starts the closing handshake: a Close frame with `code` (and
    /// `reason`, at most 123 bytes), or an empty one without a code.
    pub fn close(&self, code: Option<u16>, reason: String) {
        let _ = self.tx.send(WsCommand::Close(code, reason));
    }
}

/// What CDP's `Network.webSocket*` events report ([`NetEvent::WebSocket`]).
#[derive(Clone, Debug)]
pub enum WebSocketActivity {
    Created {
        url: Url,
        frame_id: Option<String>,
    },
    WillSendHandshakeRequest {
        headers: HeaderMap,
        wall_time: f64,
    },
    HandshakeResponseReceived {
        status: u16,
        status_text: String,
        headers: HeaderMap,
        request_headers: HeaderMap,
    },
    /// A data message: opcode 1 (text) or 2 (binary).
    FrameSent {
        opcode: u8,
        payload: Bytes,
    },
    FrameReceived {
        opcode: u8,
        payload: Bytes,
    },
    FrameError {
        message: String,
    },
    Closed,
}

impl WebSocketActivity {
    /// Heap bytes the event retains (subscription budgets).
    pub(crate) fn retained_bytes(&self) -> usize {
        let headers = |h: &HeaderMap| h.iter().map(|(k, v)| k.as_str().len() + v.len() + 16).sum::<usize>();
        match self {
            WebSocketActivity::Created { url, frame_id } => {
                url.as_str().len() + frame_id.as_ref().map_or(0, String::len)
            }
            WebSocketActivity::WillSendHandshakeRequest { headers: h, .. } => headers(h),
            WebSocketActivity::HandshakeResponseReceived { status_text, headers: h, request_headers, .. } => {
                status_text.len() + headers(h) + headers(request_headers)
            }
            WebSocketActivity::FrameSent { payload, .. } | WebSocketActivity::FrameReceived { payload, .. } => {
                payload.len()
            }
            WebSocketActivity::FrameError { message } => message.len(),
            WebSocketActivity::Closed => 0,
        }
    }
}

impl NetClient {
    /// Opens a WebSocket connection. The id is the connection's CDP request
    /// id (from this client's request counter). The sink gets the events on
    /// the I/O thread; if that thread has stopped it gets `Closed` at once,
    /// on the caller's thread.
    pub fn websocket(&self, req: WebSocketRequest, sink: WsSink) -> (RequestId, WebSocketHandle) {
        let id = self.next_request_id();
        let (tx, rx) = mpsc::unbounded_channel();
        let shared = &self.inner.shared;
        if shared.observed() {
            shared.emit(NetEvent::WebSocket {
                id,
                activity: Box::new(WebSocketActivity::Created { url: req.url.clone(), frame_id: req.frame_id.clone() }),
                timestamp: shared.now(),
            });
        }
        let start = Box::new(Start { id, req, rx, sink });
        if let Err(mpsc::error::SendError(Cmd::WebSocket(start))) =
            self.inner.tx.lock().unwrap_or_else(|e| e.into_inner()).send(Cmd::WebSocket(start))
        {
            let mut sink = start.sink;
            sink(WsEvent::Closed { clean: false, code: 1006, reason: String::new(), failed: true });
        }
        (id, WebSocketHandle { tx })
    }
}

impl NetClient {
    /// Stores the cookies of a WebSocket handshake response (`headers` of
    /// [`WsEvent::Open`]) for `url`, a `ws:`/`wss:` URL. The owner calls it
    /// when it handles the event, so script sees them from then on.
    pub fn store_websocket_cookies(&self, url: &Url, headers: &HeaderMap) {
        let mut http_url = url.clone();
        let _ = http_url.set_scheme(if url.scheme() == "wss" { "https" } else { "http" });
        let now = crate::cookies::now_secs();
        let mut jar = self.inner.cookies.lock().unwrap_or_else(|e| e.into_inner());
        for value in headers.get_all(header::SET_COOKIE) {
            if let Ok(value) = value.to_str() {
                jar.set_from_header(&http_url, value, crate::cookies::Access::Http, now);
            }
        }
    }
}

pub(crate) struct Start {
    id: RequestId,
    req: WebSocketRequest,
    rx: mpsc::UnboundedReceiver<WsCommand>,
    sink: WsSink,
}

/// Whether `port` is a Fetch "bad port": connections to it fail.
pub fn is_bad_port(port: u16) -> bool {
    const BAD: [u16; 81] = [
        0, 1, 7, 9, 11, 13, 15, 17, 19, 20, 21, 22, 23, 25, 37, 42, 43, 53, 69, 77, 79, 87, 95, 101, 102, 103, 104,
        109, 110, 111, 113, 115, 117, 119, 123, 135, 137, 139, 143, 161, 179, 389, 427, 465, 512, 513, 514, 515, 526,
        530, 531, 532, 540, 548, 554, 556, 563, 587, 601, 636, 989, 990, 993, 995, 1719, 1720, 1723, 2049, 3659, 4045,
        4190, 5060, 5061, 6000, 6566, 6665, 6666, 6667, 6668, 6669, 6679,
    ];
    BAD.contains(&port) || port == 6697 || port == 10080
}

/// A connection's CDP reporting: nothing is built unless a client observes.
struct Observer {
    shared: std::sync::Arc<super::Shared>,
    id: RequestId,
}

impl Observer {
    fn observe(&self, activity: impl FnOnce() -> WebSocketActivity) {
        if self.shared.observed() {
            self.shared.emit(NetEvent::WebSocket {
                id: self.id,
                activity: Box::new(activity()),
                timestamp: self.shared.now(),
            });
        }
    }
}

/// The connection's sink and its CDP reporting.
struct Delivery {
    sink: Option<WsSink>,
    observer: Observer,
    closed: bool,
}

impl Delivery {
    fn send(&mut self, event: WsEvent) {
        if let Some(sink) = self.sink.as_mut() {
            sink(event);
        }
    }
    fn observe(&self, activity: impl FnOnce() -> WebSocketActivity) {
        self.observer.observe(activity);
    }
    /// The terminal event (once).
    fn close(&mut self, clean: bool, code: u16, reason: String, failed: bool) {
        if std::mem::replace(&mut self.closed, true) {
            return;
        }
        self.send(WsEvent::Closed { clean, code, reason, failed });
        self.sink = None;
        self.observe(|| WebSocketActivity::Closed);
    }
    fn fail(&mut self, message: String) {
        if !self.closed {
            self.observe(|| WebSocketActivity::FrameError { message });
        }
        self.close(false, 1006, String::new(), true);
    }
}

impl Drop for Delivery {
    /// The I/O thread stopped (shutdown, fork) with the connection open.
    fn drop(&mut self) {
        self.close(false, 1006, String::new(), false);
    }
}

/// A protocol violation: the Close code sent to the server and the message.
struct Violation(u16, String);

struct Frame {
    fin: bool,
    opcode: u8,
    payload: Bytes,
}

/// The next complete frame at the front of `buf`, if there is one.
fn parse_frame(buf: &mut BytesMut, max: usize) -> Result<Option<Frame>, Violation> {
    if buf.len() < 2 {
        return Ok(None);
    }
    let (b0, b1) = (buf[0], buf[1]);
    if b0 & 0x70 != 0 {
        return Err(Violation(
            1002,
            format!(
                "One or more reserved bits are on: reserved1 = {}, reserved2 = {}, reserved3 = {}",
                (b0 >> 6) & 1,
                (b0 >> 5) & 1,
                (b0 >> 4) & 1
            ),
        ));
    }
    let fin = b0 & 0x80 != 0;
    let opcode = b0 & 0x0f;
    if b1 & 0x80 != 0 {
        return Err(Violation(1002, "A server must not mask any frames that it sends to the client.".into()));
    }
    let (len, header) = match b1 & 0x7f {
        126 => {
            if buf.len() < 4 {
                return Ok(None);
            }
            (u16::from_be_bytes([buf[2], buf[3]]) as u64, 4)
        }
        127 => {
            if buf.len() < 10 {
                return Ok(None);
            }
            let len = u64::from_be_bytes(buf[2..10].try_into().expect("8 bytes"));
            if len >> 63 != 0 {
                return Err(Violation(1002, "The most significant bit of a 64-bit length must be 0".into()));
            }
            (len, 10)
        }
        n => (n as u64, 2),
    };
    if opcode & 0x08 != 0 {
        if !fin {
            return Err(Violation(1002, format!("Received fragmented control frame: opcode = {opcode}")));
        }
        if len > 125 {
            return Err(Violation(1002, format!("Received control frame having too long payload: {len}")));
        }
    }
    if len > max as u64 {
        return Err(Violation(1009, format!("Message is too big: {len} bytes")));
    }
    let len = len as usize;
    if buf.len() < header + len {
        buf.reserve(header + len - buf.len());
        return Ok(None);
    }
    buf.advance(header);
    Ok(Some(Frame { fin, opcode, payload: buf.split_to(len).freeze() }))
}

/// A masked client frame.
fn encode_frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 14);
    out.push(0x80 | opcode);
    match payload.len() {
        n @ 0..=125 => out.push(0x80 | n as u8),
        n @ 126..=0xffff => {
            out.push(0x80 | 126);
            out.extend_from_slice(&(n as u16).to_be_bytes());
        }
        n => {
            out.push(0x80 | 127);
            out.extend_from_slice(&(n as u64).to_be_bytes());
        }
    }
    let mut key = [0u8; 4];
    // A key is needed for masking, not for secrecy of the payload; a failed
    // random source leaves the zero key (no masking effect).
    let _ = getrandom::fill(&mut key);
    out.extend_from_slice(&key);
    let start = out.len();
    out.extend_from_slice(payload);
    for (i, b) in out[start..].iter_mut().enumerate() {
        *b ^= key[i & 3];
    }
    out
}

fn close_payload(code: Option<u16>, reason: &str) -> Vec<u8> {
    match code {
        Some(code) => {
            let mut payload = code.to_be_bytes().to_vec();
            payload.extend_from_slice(reason.as_bytes());
            payload
        }
        None => Vec::new(),
    }
}

/// Chromium's check of a received Close code.
fn valid_close_code(code: u16) -> bool {
    !(code < 1000 || code == 1006 || (1014..3000).contains(&code) || code >= 5000)
}

/// `Sec-WebSocket-Accept` for a `Sec-WebSocket-Key`.
pub fn accept_key(key: &str) -> String {
    use base64::Engine;
    use sha1::{Digest, Sha1};
    let digest = Sha1::digest(format!("{key}{GUID}").as_bytes());
    base64::engine::general_purpose::STANDARD.encode(digest)
}

fn header_has_token(headers: &HeaderMap, name: header::HeaderName, token: &str) -> bool {
    headers
        .get_all(name)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .any(|t| t.trim().eq_ignore_ascii_case(token))
}

/// A connect failure's text, as Chromium words the common ones.
fn connect_error(e: &(dyn std::error::Error + 'static)) -> String {
    let mut src = Some(e);
    while let Some(s) = src {
        match s.downcast_ref::<ProxyError>() {
            Some(ProxyError::Unreachable(_)) => return "net::ERR_PROXY_CONNECTION_FAILED".into(),
            Some(ProxyError::Tunnel(_) | ProxyError::Challenge { .. }) => {
                return "net::ERR_TUNNEL_CONNECTION_FAILED".into()
            }
            None => {}
        }
        if let Some(io) = s.downcast_ref::<std::io::Error>() {
            if super::certificate_error(io).is_some() {
                return "net::ERR_CERT_AUTHORITY_INVALID".into();
            }
            match io.kind() {
                std::io::ErrorKind::ConnectionRefused => return "net::ERR_CONNECTION_REFUSED".into(),
                std::io::ErrorKind::TimedOut => return "net::ERR_TIMED_OUT".into(),
                _ => {}
            }
        }
        if s.to_string().contains("dns error") || s.to_string().contains("failed to lookup") {
            return "net::ERR_NAME_NOT_RESOLVED".into();
        }
        src = s.source();
    }
    format!("Error in connection establishment: {e}")
}

impl Io {
    fn ws_connector(&self) -> hyper_rustls::HttpsConnector<ProxyConnector> {
        let config = &self.shared.config;
        let mut tcp = HttpConnector::new();
        tcp.set_connect_timeout(Some(config.connect_timeout));
        tcp.set_nodelay(true);
        tcp.enforce_http(false);
        let proxy = ProxyConnector::new(tcp, self.shared.proxy.clone(), config.connect_timeout).tunneling();
        let tls = if self.shared.ignore_certificate_errors.load(std::sync::atomic::Ordering::Relaxed) {
            tls_config_insecure()
        } else {
            tls_config()
        };
        // HTTP/1.1 only in ALPN: the upgrade is an HTTP/1.1 mechanism.
        hyper_rustls::HttpsConnectorBuilder::new()
            .with_tls_config((*tls).clone())
            .https_or_http()
            .enable_http1()
            .wrap_connector(proxy)
    }

    /// The handshake request's headers, Chrome's set and order.
    fn ws_headers(&self, req: &WebSocketRequest, http_url: &Url, key: &str) -> HeaderMap {
        let settings = self.shared.settings.lock().unwrap().clone();
        let config = &self.shared.config;
        let mut h = HeaderMap::new();
        let host = match http_url.port() {
            Some(port) => format!("{}:{port}", http_url.host_str().unwrap_or("")),
            None => http_url.host_str().unwrap_or("").to_owned(),
        };
        if let Ok(v) = HeaderValue::from_str(&host) {
            h.insert(header::HOST, v);
        }
        h.insert(header::CONNECTION, HeaderValue::from_static("Upgrade"));
        h.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
        let ua = settings.user_agent.as_deref().unwrap_or(&config.user_agent);
        if let Some(v) = req.defaults.user_agent.clone().or_else(|| HeaderValue::from_str(ua).ok()) {
            h.insert(header::USER_AGENT, v);
        }
        h.insert(header::UPGRADE, HeaderValue::from_static("websocket"));
        if let Ok(v) = HeaderValue::from_str(&req.origin) {
            h.insert(header::ORIGIN, v);
        }
        h.insert(header::SEC_WEBSOCKET_VERSION, HeaderValue::from_static("13"));
        h.insert(header::ACCEPT_ENCODING, HeaderValue::from_static("gzip, deflate, br"));
        let lang = settings.accept_language.as_deref().unwrap_or(&config.accept_language);
        if let Some(v) = req.defaults.accept_language.clone().or_else(|| HeaderValue::from_str(lang).ok()) {
            h.insert(header::ACCEPT_LANGUAGE, v);
        }
        if let Some(hints) = &req.defaults.client_hints {
            if crate::cookies::is_trustworthy(http_url) {
                for (name, value) in hints.iter() {
                    h.entry(name.clone()).or_insert_with(|| value.clone());
                }
            }
        }
        // Credentials mode "include": the jar's cookies for the http(s) URL.
        let origin = Url::parse(&req.origin).ok().filter(|o| o.host_str().is_some());
        let context = match &origin {
            Some(origin) => site_context(Some(origin), http_url, false, true),
            None => SiteContext::CrossSite,
        };
        let cookie = self.cookies.lock().unwrap().header_for(http_url, context, crate::cookies::now_secs());
        if let Some(v) = cookie.and_then(|c| HeaderValue::from_str(&c).ok()) {
            h.insert(header::COOKIE, v);
        }
        if !req.url.username().is_empty() || req.url.password().is_some() {
            let decode = |s: &str| percent_encoding::percent_decode_str(s).decode_utf8_lossy().into_owned();
            let user = decode(req.url.username());
            let password = decode(req.url.password().unwrap_or(""));
            if let Ok(v) = crate::proxy::basic_authorization(&user, &password) {
                h.insert(header::AUTHORIZATION, v);
            }
        }
        if let Ok(v) = HeaderValue::from_str(key) {
            h.insert(header::SEC_WEBSOCKET_KEY, v);
        }
        if !req.protocols.is_empty() {
            if let Ok(v) = HeaderValue::from_str(&req.protocols.join(", ")) {
                h.insert(header::SEC_WEBSOCKET_PROTOCOL, v);
            }
        }
        for (k, v) in settings.extra_headers.iter().chain(req.defaults.extra_headers.iter()) {
            h.insert(k.clone(), v.clone());
        }
        h
    }

    /// Runs one connection to its end.
    pub(crate) async fn websocket(self: std::rc::Rc<Self>, start: Start) {
        let Start { id, req, mut rx, sink } = start;
        let observer = Observer { shared: self.shared.clone(), id };
        let opened = tokio::select! {
            opened = self.ws_open(&req, &observer) => Some(opened),
            // The owner dropped its handle: no more events.
            _ = wait_for_drop(&mut rx) => None,
        };
        let mut delivery = Delivery { sink: Some(sink), observer, closed: false };
        let Some(opened) = opened else {
            delivery.sink = None;
            return delivery.close(false, 1006, String::new(), false);
        };
        let (io, protocol, headers) = match opened {
            Ok(opened) => opened,
            Err(message) => return delivery.fail(message),
        };
        delivery.send(WsEvent::Open { protocol, extensions: String::new(), headers });
        self.ws_run(io, &mut rx, &mut delivery).await;
    }

    async fn ws_open(
        &self,
        req: &WebSocketRequest,
        delivery: &Observer,
    ) -> Result<(TokioIo<hyper::upgrade::Upgraded>, String, HeaderMap), String> {
        let secure = match req.url.scheme() {
            "ws" => false,
            "wss" => true,
            other => return Err(format!("The URL's scheme must be either 'ws' or 'wss'. '{other}' is not allowed.")),
        };
        let mut http_url = req.url.clone();
        let _ = http_url.set_scheme(if secure { "https" } else { "http" });
        let _ = http_url.set_username("");
        let _ = http_url.set_password(None);
        http_url.set_fragment(None);
        if http_url.port_or_known_default().is_some_and(is_bad_port) {
            return Err("net::ERR_UNSAFE_PORT".into());
        }
        if self.shared.settings.lock().unwrap().offline {
            return Err("net::ERR_INTERNET_DISCONNECTED".into());
        }
        let key = {
            use base64::Engine;
            let mut nonce = [0u8; 16];
            let _ = getrandom::fill(&mut nonce);
            base64::engine::general_purpose::STANDARD.encode(nonce)
        };
        let headers = self.ws_headers(req, &http_url, &key);
        delivery.observe(|| WebSocketActivity::WillSendHandshakeRequest {
            headers: headers.clone(),
            wall_time: wall_time(),
        });
        let mut target = http_url.clone();
        target.set_path("/");
        target.set_query(None);
        let uri: http::Uri = target.as_str().parse().map_err(|_| "Invalid URL".to_owned())?;
        let mut connector = self.ws_connector();
        let io = tower_service::Service::call(&mut connector, uri).await.map_err(|e| connect_error(&*e))?;
        let (mut sender, connection) = hyper::client::conn::http1::handshake::<_, Empty<Bytes>>(io)
            .await
            .map_err(|e| format!("Error in connection establishment: {e}"))?;
        tokio::task::spawn_local(async move {
            let _ = connection.with_upgrades().await;
        });
        let path = match http_url.query() {
            Some(query) => format!("{}?{query}", http_url.path()),
            None => http_url.path().to_owned(),
        };
        let mut request = http::Request::builder()
            .method(Method::GET)
            .uri(path)
            .body(Empty::new())
            .map_err(|_| "Invalid URL".to_owned())?;
        *request.headers_mut() = headers.clone();
        let response = sender.send_request(request).await.map_err(|e| {
            if e.is_incomplete_message() || e.is_closed() {
                "Connection closed before receiving a handshake response".to_owned()
            } else {
                format!("Error during WebSocket handshake: {e}")
            }
        })?;
        let status = response.status();
        delivery.observe(|| WebSocketActivity::HandshakeResponseReceived {
            status: status.as_u16(),
            status_text: status.canonical_reason().unwrap_or("").to_owned(),
            headers: response.headers().clone(),
            request_headers: headers.clone(),
        });
        if status != http::StatusCode::SWITCHING_PROTOCOLS {
            return Err(format!("Error during WebSocket handshake: Unexpected response code: {}", status.as_u16()));
        }
        let h = response.headers();
        if !header_has_token(h, header::UPGRADE, "websocket") {
            return Err("Error during WebSocket handshake: 'Upgrade' header is missing".into());
        }
        if !header_has_token(h, header::CONNECTION, "upgrade") {
            return Err("Error during WebSocket handshake: 'Connection' header is missing".into());
        }
        let accept = h.get(header::SEC_WEBSOCKET_ACCEPT).and_then(|v| v.to_str().ok());
        if accept != Some(accept_key(&key).as_str()) {
            return Err("Error during WebSocket handshake: Incorrect 'Sec-WebSocket-Accept' header value".into());
        }
        if h.contains_key(header::SEC_WEBSOCKET_EXTENSIONS) {
            return Err(
                "Error during WebSocket handshake: Response must not include 'Sec-WebSocket-Extensions' header if not present in request"
                    .into(),
            );
        }
        let mut protocols = h.get_all(header::SEC_WEBSOCKET_PROTOCOL).iter();
        let protocol = match (protocols.next(), protocols.next()) {
            (None, _) => String::new(),
            (Some(_), Some(_)) => {
                return Err("Error during WebSocket handshake: 'Sec-WebSocket-Protocol' header must not appear more than once in a response".into())
            }
            (Some(v), None) => v.to_str().unwrap_or("").to_owned(),
        };
        if protocol.is_empty() && !req.protocols.is_empty() {
            return Err(
                "Error during WebSocket handshake: Sent non-empty 'Sec-WebSocket-Protocol' header but no response was received"
                    .into(),
            );
        }
        if !protocol.is_empty() && !req.protocols.contains(&protocol) {
            return Err("Error during WebSocket handshake: 'Sec-WebSocket-Protocol' header value '".to_owned()
                + &protocol
                + "' in response does not match any of sent values");
        }
        let response_headers = h.clone();
        let upgraded =
            hyper::upgrade::on(response).await.map_err(|e| format!("Error during WebSocket handshake: {e}"))?;
        Ok((TokioIo::new(upgraded), protocol, response_headers))
    }

    /// Frames in and out until the connection ends.
    async fn ws_run(
        &self,
        mut io: TokioIo<hyper::upgrade::Upgraded>,
        rx: &mut mpsc::UnboundedReceiver<WsCommand>,
        delivery: &mut Delivery,
    ) {
        let max = self.shared.config.max_body;
        let mut inbuf = BytesMut::with_capacity(8 * 1024);
        // An unfinished fragmented message: its opcode and bytes so far.
        let mut fragment: Option<(u8, BytesMut)> = None;
        let mut close_sent = false;
        let mut close_received: Option<(u16, String)> = None;
        let mut deadline: Option<tokio::time::Instant> = None;
        loop {
            let timeout = async {
                match deadline {
                    Some(at) => tokio::time::sleep_until(at).await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                read = io.read_buf(&mut inbuf) => {
                    let n = match read {
                        Ok(n) => n,
                        Err(_) => 0,
                    };
                    if n == 0 {
                        match close_received.take() {
                            Some((code, reason)) => delivery.close(close_sent, code, reason, false),
                            None => delivery.close(false, 1006, String::new(), false),
                        }
                        return;
                    }
                    loop {
                        let frame = match parse_frame(&mut inbuf, max) {
                            Ok(Some(frame)) => frame,
                            Ok(None) => break,
                            Err(Violation(code, message)) => {
                                return self.ws_fail(&mut io, delivery, close_sent, code, message).await;
                            }
                        };
                        let complete = match frame.opcode {
                            0x0 => {
                                let Some((opcode, mut bytes)) = fragment.take() else {
                                    return self.ws_fail(&mut io, delivery, close_sent, 1002, "Received unexpected continuation frame.".into()).await;
                                };
                                if bytes.len() + frame.payload.len() > max {
                                    return self.ws_fail(&mut io, delivery, close_sent, 1009, "Message is too big".into()).await;
                                }
                                bytes.extend_from_slice(&frame.payload);
                                if frame.fin {
                                    Some((opcode, bytes.freeze()))
                                } else {
                                    fragment = Some((opcode, bytes));
                                    None
                                }
                            }
                            0x1 | 0x2 => {
                                if fragment.is_some() {
                                    return self.ws_fail(&mut io, delivery, close_sent, 1002, "Received start of new message but previous message is unfinished.".into()).await;
                                }
                                if frame.fin {
                                    Some((frame.opcode, frame.payload))
                                } else {
                                    fragment = Some((frame.opcode, BytesMut::from(&frame.payload[..])));
                                    None
                                }
                            }
                            0x8 => {
                                let payload = frame.payload;
                                let (code, reason) = match payload.len() {
                                    0 => (1005, String::new()),
                                    1 => return self.ws_fail(&mut io, delivery, close_sent, 1002, "Received a broken close frame containing an invalid size body.".into()).await,
                                    _ => {
                                        let code = u16::from_be_bytes([payload[0], payload[1]]);
                                        if !valid_close_code(code) {
                                            return self.ws_fail(&mut io, delivery, close_sent, 1002, format!("Received a broken close frame containing an invalid close code: {code}")).await;
                                        }
                                        match std::str::from_utf8(&payload[2..]) {
                                            Ok(reason) => (code, reason.to_owned()),
                                            Err(_) => return self.ws_fail(&mut io, delivery, close_sent, 1007, "Received a broken close frame containing invalid UTF-8.".into()).await,
                                        }
                                    }
                                };
                                if !close_sent {
                                    // Echo the code, as Chromium does.
                                    let echo = if code == 1005 { None } else { Some(code) };
                                    let _ = write_frame(&mut io, 0x8, &close_payload(echo, "")).await;
                                    close_sent = true;
                                }
                                close_received = Some((code, reason));
                                deadline = Some(tokio::time::Instant::now() + CONNECTION_CLOSE_TIMEOUT);
                                None
                            }
                            0x9 => {
                                if !close_sent {
                                    let _ = write_frame(&mut io, 0xA, &frame.payload).await;
                                }
                                None
                            }
                            0xA => None,
                            other => {
                                return self.ws_fail(&mut io, delivery, close_sent, 1002, format!("Unrecognized frame opcode: {other}")).await;
                            }
                        };
                        let Some((opcode, payload)) = complete else { continue };
                        if close_received.is_some() {
                            continue;
                        }
                        if opcode == 0x1 {
                            let Ok(text) = std::str::from_utf8(&payload) else {
                                return self.ws_fail(&mut io, delivery, close_sent, 1007, "Could not decode a text frame as UTF-8.".into()).await;
                            };
                            let text = text.to_owned();
                            delivery.observe(|| WebSocketActivity::FrameReceived { opcode, payload: payload.clone() });
                            delivery.send(WsEvent::Text(text));
                        } else {
                            delivery.observe(|| WebSocketActivity::FrameReceived { opcode, payload: payload.clone() });
                            delivery.send(WsEvent::Binary(payload));
                        }
                    }
                }
                command = rx.recv() => {
                    let Some(command) = command else {
                        // The owner dropped its handle: abort.
                        delivery.sink = None;
                        delivery.close(false, 1006, String::new(), false);
                        return;
                    };
                    let (opcode, payload, len) = match command {
                        WsCommand::Text(text) => {
                            let len = text.len();
                            (0x1, Bytes::from(text), len)
                        }
                        WsCommand::Binary(bytes) => {
                            let len = bytes.len();
                            (0x2, bytes, len)
                        }
                        WsCommand::Close(code, reason) => {
                            if !close_sent {
                                close_sent = true;
                                let _ = write_frame(&mut io, 0x8, &close_payload(code, &reason)).await;
                                if deadline.is_none() {
                                    deadline = Some(tokio::time::Instant::now() + CLOSING_HANDSHAKE_TIMEOUT);
                                }
                            }
                            continue;
                        }
                    };
                    if close_sent {
                        continue;
                    }
                    if write_frame(&mut io, opcode, &payload).await.is_err() {
                        delivery.close(false, 1006, String::new(), false);
                        return;
                    }
                    delivery.observe(|| WebSocketActivity::FrameSent { opcode, payload });
                    delivery.send(WsEvent::Sent(len));
                }
                () = timeout => {
                    match close_received.take() {
                        Some((code, reason)) => delivery.close(true, code, reason, false),
                        None => delivery.close(false, 1006, String::new(), false),
                    }
                    let _ = io.shutdown().await;
                    return;
                }
            }
        }
    }

    /// "Fail the WebSocket connection".
    async fn ws_fail(
        &self,
        io: &mut TokioIo<hyper::upgrade::Upgraded>,
        delivery: &mut Delivery,
        close_sent: bool,
        code: u16,
        message: String,
    ) {
        if !close_sent {
            let _ = write_frame(io, 0x8, &close_payload(Some(code), "")).await;
        }
        let _ = io.shutdown().await;
        delivery.fail(message);
    }
}

async fn write_frame(io: &mut TokioIo<hyper::upgrade::Upgraded>, opcode: u8, payload: &[u8]) -> std::io::Result<()> {
    io.write_all(&encode_frame(opcode, payload)).await?;
    io.flush().await
}

/// Resolves when the owner drops its handle (commands sent while the
/// connection opens wait in the channel).
async fn wait_for_drop(rx: &mut mpsc::UnboundedReceiver<WsCommand>) {
    // Commands cannot arrive before `Open` (the owner sends only once open,
    // except Close, which it turns into a drop while connecting).
    while rx.recv().await.is_some() {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accept_key_matches_rfc_example() {
        assert_eq!(accept_key("dGhlIHNhbXBsZSBub25jZQ=="), "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    }

    #[test]
    fn frames_round_trip_and_violations_are_reported() {
        let encoded = encode_frame(0x1, b"hello");
        assert_eq!(encoded[0], 0x81);
        assert_eq!(encoded[1], 0x80 | 5);
        // Server frames are unmasked.
        let mut buf = BytesMut::from(&[0x81u8, 3, b'a', b'b', b'c'][..]);
        let frame = parse_frame(&mut buf, 100).ok().flatten().unwrap();
        assert!(frame.fin);
        assert_eq!(frame.opcode, 1);
        assert_eq!(&frame.payload[..], b"abc");
        assert!(buf.is_empty());
        let mut partial = BytesMut::from(&[0x82u8, 126, 0x01][..]);
        assert!(parse_frame(&mut partial, 1000).ok().unwrap().is_none());
        let mut masked = BytesMut::from(&[0x81u8, 0x81, 0, 0, 0, 0, b'a'][..]);
        assert!(matches!(parse_frame(&mut masked, 100), Err(Violation(1002, _))));
        let mut rsv = BytesMut::from(&[0xC1u8, 0][..]);
        assert!(matches!(parse_frame(&mut rsv, 100), Err(Violation(1002, _))));
        let mut fragmented_ping = BytesMut::from(&[0x09u8, 0][..]);
        assert!(matches!(parse_frame(&mut fragmented_ping, 100), Err(Violation(1002, _))));
        let mut big = BytesMut::from(&[0x82u8, 126, 0x01, 0x00][..]);
        assert!(matches!(parse_frame(&mut big, 100), Err(Violation(1009, _))));
    }

    #[test]
    fn close_codes_and_bad_ports_follow_chromium_and_fetch() {
        assert!(valid_close_code(1000) && valid_close_code(1011) && valid_close_code(3000) && valid_close_code(4999));
        assert!(
            !valid_close_code(999) && !valid_close_code(1006) && !valid_close_code(1015) && !valid_close_code(5000)
        );
        assert!(is_bad_port(6667) && is_bad_port(10080) && is_bad_port(0) && !is_bad_port(8080) && !is_bad_port(443));
    }
}
