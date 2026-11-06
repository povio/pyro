//! WebSocket client (RFC 6455) on the context's I/O thread: the opening
//! handshake over a plain or TLS TCP stream, then message framing until
//! either side closes.
//!
//! The browser thread opens a socket with [`NetClient::websocket`] and drives
//! it through the returned [`WebSocketHandle`]; the socket reports to its
//! [`WsSink`] (`Open`, each `Message`, exactly one `Closed`). The context's
//! observers get the CDP `Network.webSocket*` facts as [`NetEvent`]s.
//!
//! Cookies of the context jar go with the handshake and `Set-Cookie` of the
//! response is stored, the page's User-Agent and Accept-Language apply, and
//! `Network.emulateNetworkConditions({offline})` fails new sockets. A socket
//! counts as a request of its loader while it connects and as an established
//! stream once open (see [`NetActivity::event_streams`]).
//!
//! Gaps (explicit): no extensions (permessage-deflate is not offered), no
//! HTTP proxy tunnelling (sockets connect directly) and no Fetch
//! interception of the handshake, as in Chrome.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::{Buf, Bytes, BytesMut};
use http::{header, HeaderMap, HeaderName, HeaderValue};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;
use url::Url;

use super::{wall_time, ActivityGuard, Cmd, NetClient, NetEvent, Shared};
use crate::cookies::{site_context, CookieJar};
use crate::request::{RequestDefaults, RequestId};

/// Opening a WebSocket: what the page's `new WebSocket(url, protocols)` asked.
#[derive(Clone, Debug)]
pub struct WsRequest {
    /// A `ws:` or `wss:` URL without a fragment.
    pub url: Url,
    /// Requested subprotocols (`Sec-WebSocket-Protocol`), in order.
    pub protocols: Vec<String>,
    /// Serialized origin of the document (the `Origin` header).
    pub origin: Option<String>,
    /// The document's URL, for the cookies' site context.
    pub document_url: Option<Url>,
    pub frame_id: Option<String>,
    pub loader_id: Option<String>,
    /// The page's User-Agent, Accept-Language and extra headers.
    pub defaults: RequestDefaults,
    /// Largest message accepted from the server; larger ones fail the
    /// connection with 1009.
    pub max_message: usize,
}

impl WsRequest {
    pub fn new(url: Url) -> Self {
        WsRequest {
            url,
            protocols: Vec::new(),
            origin: None,
            document_url: None,
            frame_id: None,
            loader_id: None,
            defaults: RequestDefaults::default(),
            max_message: 64 * 1024 * 1024,
        }
    }
}

/// One WebSocket message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WsMessage {
    Text(String),
    Binary(Bytes),
}

impl WsMessage {
    pub fn len(&self) -> usize {
        match self {
            WsMessage::Text(t) => t.len(),
            WsMessage::Binary(b) => b.len(),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// What a socket reports to its sink, in order: `Open` (only after a
/// successful handshake), any number of `Message`s, then exactly one `Closed`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WsEvent {
    /// The handshake completed: the subprotocol and extensions the server chose.
    Open {
        protocol: String,
        extensions: String,
    },
    Message(WsMessage),
    /// The connection is closed. `code` is the peer's close code (1005 when
    /// its close frame had none, 1006 when the connection ended without a
    /// closing handshake); `error` is set when the connection failed (the
    /// page fires `error` before `close`).
    Closed {
        code: u16,
        reason: String,
        clean: bool,
        error: Option<String>,
    },
}

/// Receives a socket's events on the I/O thread; wrap it to post to the
/// browser loop. It is called with `Closed` exactly once, then dropped.
pub type WsSink = Box<dyn FnMut(WsEvent) + Send>;

pub(super) enum WsCmd {
    Send(WsMessage),
    Close(Option<u16>, String),
}

/// The browser side of an open socket. Dropping it closes the socket (1001,
/// going away) if it is still open.
pub struct WebSocketHandle {
    id: RequestId,
    tx: mpsc::UnboundedSender<WsCmd>,
    buffered: Arc<AtomicU64>,
}

impl WebSocketHandle {
    /// The CDP `requestId` of the socket's `Network.webSocket*` events.
    pub fn id(&self) -> RequestId {
        self.id
    }
    /// Queues a message; its bytes count in [`Self::buffered_amount`] until
    /// written to the connection.
    pub fn send(&self, message: WsMessage) {
        self.buffered.fetch_add(message.len() as u64, Ordering::Relaxed);
        if let Err(mpsc::error::SendError(WsCmd::Send(m))) = self.tx.send(WsCmd::Send(message)) {
            self.buffered.fetch_sub(m.len() as u64, Ordering::Relaxed);
        }
    }
    /// Starts the closing handshake (`code` 1000 or 3000-4999, `reason` at
    /// most 123 bytes: the caller validates both).
    pub fn close(&self, code: Option<u16>, reason: String) {
        let _ = self.tx.send(WsCmd::Close(code, reason));
    }
    /// Bytes of queued messages not yet written.
    pub fn buffered_amount(&self) -> u64 {
        self.buffered.load(Ordering::Relaxed)
    }
}

pub(super) struct WsJob {
    pub(super) id: RequestId,
    pub(super) request: WsRequest,
    pub(super) sink: WsSink,
    pub(super) commands: mpsc::UnboundedReceiver<WsCmd>,
    pub(super) buffered: Arc<AtomicU64>,
}

impl NetClient {
    /// Opens a WebSocket. Events arrive on the I/O thread; if it has stopped,
    /// `sink` gets `Closed` (1006, with an error) on the caller's thread.
    pub fn websocket(&self, request: WsRequest, sink: WsSink) -> WebSocketHandle {
        let id = self.next_request_id();
        let (tx, commands) = mpsc::unbounded_channel();
        let buffered = Arc::new(AtomicU64::new(0));
        let sink = match request.loader_id.clone() {
            Some(loader) => {
                let mut guard = self.start_activity(loader);
                let mut sink = sink;
                Box::new(move |event: WsEvent| {
                    if matches!(event, WsEvent::Open { .. }) {
                        guard.mark_event_stream();
                    }
                    sink(event)
                }) as WsSink
            }
            None => sink,
        };
        let job = WsJob { id, request, sink, commands, buffered: buffered.clone() };
        if let Err(mpsc::error::SendError(Cmd::WebSocket(job))) =
            self.inner.tx.lock().unwrap_or_else(|e| e.into_inner()).send(Cmd::WebSocket(Box::new(job)))
        {
            let mut sink = job.sink;
            sink(WsEvent::Closed {
                code: 1006,
                reason: String::new(),
                clean: false,
                error: Some("network thread stopped".into()),
            });
        }
        WebSocketHandle { id, tx, buffered }
    }
}

impl ActivityGuard {
    /// Once a request became an established stream, it no longer counts as
    /// finite work of its loader.
    pub(super) fn mark_event_stream(&mut self) {
        if self.event_stream {
            return;
        }
        let mut map = self.map.lock().unwrap();
        if let Some((generation, activity)) = map.loaders.get_mut(&self.loader) {
            if *generation == self.generation {
                activity.event_streams += 1;
                self.event_stream = true;
            }
        }
    }
}

/// Delivers the one `Closed` (and CDP `webSocketClosed`) even when the task is
/// dropped mid-way (client shutdown, fork suspension).
struct Reporter {
    id: RequestId,
    shared: Arc<Shared>,
    sink: Option<WsSink>,
    /// `webSocketCreated` was emitted, so `webSocketClosed` is owed.
    announced: bool,
}

impl Reporter {
    fn emit(&self, event: NetEvent) {
        self.shared.emit(event);
    }
    fn now(&self) -> f64 {
        self.shared.now()
    }
    fn deliver(&mut self, event: WsEvent) {
        if let Some(sink) = self.sink.as_mut() {
            sink(event);
        }
    }
    fn frame_error(&self, message: &str) {
        self.emit(NetEvent::WebSocketFrameError { id: self.id, message: message.to_owned(), timestamp: self.now() });
    }
    /// Reports the end of the connection. `error` also goes to CDP as a frame error.
    fn closed(&mut self, code: u16, reason: String, clean: bool, error: Option<String>) {
        let Some(mut sink) = self.sink.take() else { return };
        if let Some(e) = &error {
            self.frame_error(e);
        }
        if self.announced {
            self.emit(NetEvent::WebSocketClosed { id: self.id, timestamp: self.now() });
        }
        sink(WsEvent::Closed { code, reason, clean, error });
    }
}

impl Drop for Reporter {
    fn drop(&mut self) {
        self.closed(1006, String::new(), false, Some("network thread stopped".into()));
    }
}

const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// `Sec-WebSocket-Accept` for a key (RFC 6455 §4.2.2).
pub fn accept_key(key: &str) -> String {
    use base64::Engine;
    let digest = ring::digest::digest(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY, format!("{key}{GUID}").as_bytes());
    base64::engine::general_purpose::STANDARD.encode(digest.as_ref())
}

fn random<const N: usize>() -> [u8; N] {
    use ring::rand::SecureRandom;
    let mut out = [0u8; N];
    let _ = ring::rand::SystemRandom::new().fill(&mut out);
    out
}

pub(crate) const OP_CONTINUATION: u8 = 0x0;
pub(crate) const OP_TEXT: u8 = 0x1;
pub(crate) const OP_BINARY: u8 = 0x2;
pub(crate) const OP_CLOSE: u8 = 0x8;
pub(crate) const OP_PING: u8 = 0x9;
pub(crate) const OP_PONG: u8 = 0xA;

/// One frame, masked with `mask` when given (client frames always are).
pub(crate) fn encode_frame(opcode: u8, payload: &[u8], mask: Option<[u8; 4]>) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 14);
    out.push(0x80 | opcode);
    let bit = if mask.is_some() { 0x80 } else { 0 };
    match payload.len() {
        n if n < 126 => out.push(bit | n as u8),
        n if n <= u16::MAX as usize => {
            out.push(bit | 126);
            out.extend_from_slice(&(n as u16).to_be_bytes());
        }
        n => {
            out.push(bit | 127);
            out.extend_from_slice(&(n as u64).to_be_bytes());
        }
    }
    match mask {
        Some(key) => {
            out.extend_from_slice(&key);
            out.extend(payload.iter().enumerate().map(|(i, b)| b ^ key[i % 4]));
        }
        None => out.extend_from_slice(payload),
    }
    out
}

/// A frame as received.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Frame {
    pub(crate) fin: bool,
    pub(crate) opcode: u8,
    pub(crate) payload: Bytes,
}

/// Why the connection must be failed: the close code to send and the message
/// (Chrome's wording) for the page and CDP.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Failure {
    pub(crate) code: u16,
    pub(crate) message: String,
}

fn fail(code: u16, message: impl Into<String>) -> Failure {
    Failure { code, message: message.into() }
}

/// Takes the next whole frame off `buf`, `Ok(None)` while it is incomplete.
/// Server frames must not be masked (§5.1) and use no reserved bits (no
/// extension was negotiated).
pub(crate) fn parse_frame(buf: &mut BytesMut, max_payload: usize) -> Result<Option<Frame>, Failure> {
    if buf.len() < 2 {
        return Ok(None);
    }
    let (b0, b1) = (buf[0], buf[1]);
    if b0 & 0x70 != 0 {
        return Err(fail(1002, "One or more reserved bits are on: reserved1 = 1"));
    }
    let fin = b0 & 0x80 != 0;
    let opcode = b0 & 0x0f;
    if !matches!(opcode, OP_CONTINUATION | OP_TEXT | OP_BINARY | OP_CLOSE | OP_PING | OP_PONG) {
        return Err(fail(1002, format!("Unrecognized frame opcode: {opcode}")));
    }
    if b1 & 0x80 != 0 {
        return Err(fail(1002, "A server must not mask any frames that it sends to the client."));
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
            let mut b = [0u8; 8];
            b.copy_from_slice(&buf[2..10]);
            (u64::from_be_bytes(b), 10)
        }
        n => (n as u64, 2),
    };
    if opcode >= OP_CLOSE {
        if !fin {
            return Err(fail(1002, "Received fragmented control frame: opcode = ".to_owned() + &opcode.to_string()));
        }
        if len > 125 {
            return Err(fail(1002, "Received control frame having too long payload: ".to_owned() + &len.to_string()));
        }
    }
    if len > max_payload as u64 {
        return Err(fail(1009, "WebSocket frame length too large"));
    }
    let total = header + len as usize;
    if buf.len() < total {
        buf.reserve(total - buf.len());
        return Ok(None);
    }
    buf.advance(header);
    let payload = buf.split_to(len as usize).freeze();
    Ok(Some(Frame { fin, opcode, payload }))
}

/// Joins fragments into messages and answers control frames.
pub(crate) struct Assembler {
    opcode: Option<u8>,
    parts: BytesMut,
    max_message: usize,
}

/// What a frame amounts to.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Incoming {
    Nothing,
    Message(WsMessage),
    Ping(Bytes),
    /// A close frame: its code (1005 without one) and reason.
    Close(u16, String),
}

impl Assembler {
    pub(crate) fn new(max_message: usize) -> Self {
        Assembler { opcode: None, parts: BytesMut::new(), max_message }
    }

    pub(crate) fn push(&mut self, frame: Frame) -> Result<Incoming, Failure> {
        match frame.opcode {
            OP_PING => return Ok(Incoming::Ping(frame.payload)),
            OP_PONG => return Ok(Incoming::Nothing),
            OP_CLOSE => {
                let p = &frame.payload;
                return match p.len() {
                    0 => Ok(Incoming::Close(1005, String::new())),
                    1 => Err(fail(1002, "Received a broken close frame containing an invalid size body.")),
                    _ => {
                        let code = u16::from_be_bytes([p[0], p[1]]);
                        if !valid_received_code(code) {
                            return Err(fail(
                                1002,
                                format!("Received a broken close frame containing an invalid close code: {code}"),
                            ));
                        }
                        let reason = std::str::from_utf8(&p[2..])
                            .map_err(|_| fail(1007, "Received a broken close frame containing invalid UTF-8."))?;
                        Ok(Incoming::Close(code, reason.to_owned()))
                    }
                };
            }
            _ => {}
        }
        let opcode = match (self.opcode, frame.opcode) {
            (None, OP_CONTINUATION) => return Err(fail(1002, "Received unexpected continuation frame.")),
            (Some(_), OP_TEXT | OP_BINARY) => {
                return Err(fail(1002, "Received start of new message but previous message is unfinished."))
            }
            (Some(op), _) | (None, op) => op,
        };
        if self.parts.len() + frame.payload.len() > self.max_message {
            return Err(fail(1009, "WebSocket message is too large"));
        }
        let payload = if self.opcode.is_none() && frame.fin {
            frame.payload
        } else {
            self.parts.extend_from_slice(&frame.payload);
            if !frame.fin {
                self.opcode = Some(opcode);
                return Ok(Incoming::Nothing);
            }
            self.opcode = None;
            std::mem::take(&mut self.parts).freeze()
        };
        Ok(Incoming::Message(match opcode {
            OP_TEXT => WsMessage::Text(
                String::from_utf8(payload.to_vec())
                    .map_err(|_| fail(1007, "Could not decode a text frame as UTF-8."))?,
            ),
            _ => WsMessage::Binary(payload),
        }))
    }
}

/// Close codes a peer may send (§7.4).
fn valid_received_code(code: u16) -> bool {
    matches!(code, 1000..=1003 | 1007..=1014 | 3000..=4999)
}

trait Stream: AsyncRead + AsyncWrite + Unpin {}
impl<T: AsyncRead + AsyncWrite + Unpin> Stream for T {}

/// The http(s) URL a ws(s) URL stands for (cookies and Origin rules).
pub(crate) fn http_url(url: &Url) -> Url {
    let mut u = url.clone();
    let _ = u.set_scheme(if url.scheme() == "wss" { "https" } else { "http" });
    u
}

/// Runs one socket to its end on the I/O thread.
pub(super) async fn run(shared: Arc<Shared>, cookies: Arc<Mutex<CookieJar>>, job: WsJob) {
    let WsJob { id, request, sink, mut commands, buffered } = job;
    let mut reporter = Reporter { id, shared: shared.clone(), sink: Some(sink), announced: false };
    reporter.emit(NetEvent::WebSocketCreated {
        id,
        url: request.url.to_string(),
        frame_id: request.frame_id.clone(),
        loader_id: request.loader_id.clone(),
        timestamp: reporter.now(),
    });
    reporter.announced = true;

    let key = {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(random::<16>())
    };
    let headers = handshake_headers(&shared, &cookies, &request, &key);
    reporter.emit(NetEvent::WebSocketWillSendHandshakeRequest {
        id,
        headers: Box::new(headers.clone()),
        timestamp: reporter.now(),
        wall_time: wall_time(),
    });
    if shared.settings.lock().unwrap().offline {
        return reporter.closed(
            1006,
            String::new(),
            false,
            Some("Error in connection establishment: net::ERR_INTERNET_DISCONNECTED".into()),
        );
    }
    let timeout = shared.config.connect_timeout;
    let stream = match tokio::time::timeout(timeout, connect(&shared, &request.url)).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            return reporter.closed(1006, String::new(), false, Some(format!("Error in connection establishment: {e}")))
        }
        Err(_) => {
            return reporter.closed(
                1006,
                String::new(),
                false,
                Some("Error in connection establishment: net::ERR_TIMED_OUT".into()),
            )
        }
    };
    let mut stream = stream;
    let mut buf = BytesMut::with_capacity(8 * 1024);
    let handshake = tokio::time::timeout(timeout, async {
        stream.write_all(&request_bytes(&request.url, &headers)).await?;
        read_head(&mut stream, &mut buf).await
    })
    .await;
    let head = match handshake {
        Ok(Ok(head)) => head,
        Ok(Err(e)) => {
            return reporter.closed(
                1006,
                String::new(),
                false,
                Some(format!("Error during WebSocket handshake: net::{}", io_error_text(&e))),
            )
        }
        Err(_) => {
            return reporter.closed(
                1006,
                String::new(),
                false,
                Some("Error during WebSocket handshake: net::ERR_TIMED_OUT".into()),
            )
        }
    };
    reporter.emit(NetEvent::WebSocketHandshakeResponseReceived {
        id,
        status: head.status,
        status_text: head.reason.clone(),
        headers: Box::new(head.headers.clone()),
        request_headers: Box::new(headers.clone()),
        timestamp: reporter.now(),
    });
    {
        let now = crate::cookies::now_secs();
        let url = http_url(&request.url);
        let mut jar = cookies.lock().unwrap();
        for v in head.headers.get_all(header::SET_COOKIE) {
            if let Ok(s) = v.to_str() {
                jar.set_from_header(&url, s, crate::cookies::Access::Http, now);
            }
        }
    }
    let protocol = match check_response(&head, &key, &request.protocols) {
        Ok(p) => p,
        Err(e) => {
            return reporter.closed(1006, String::new(), false, Some(format!("Error during WebSocket handshake: {e}")))
        }
    };
    reporter.deliver(WsEvent::Open { protocol, extensions: String::new() });

    let mut assembler = Assembler::new(request.max_message);
    // Set once our close frame is written: the closing handshake is under way.
    let mut sent_close: Option<tokio::time::Instant> = None;
    let mut commands_open = true;
    let mut read_buf = [0u8; 16 * 1024];
    let (mut rd, mut wr) = tokio::io::split(stream);
    let outcome: Result<(u16, String), Failure> = 'conn: loop {
        // Whole frames already buffered (handshake leftovers or a large read).
        loop {
            match parse_frame(&mut buf, request.max_message) {
                Ok(None) => break,
                Ok(Some(frame)) => match assembler.push(frame) {
                    Ok(Incoming::Nothing) => {}
                    Ok(Incoming::Ping(payload)) => {
                        if sent_close.is_none() {
                            let _ = wr.write_all(&encode_frame(OP_PONG, &payload, Some(random()))).await;
                        }
                    }
                    Ok(Incoming::Message(m)) => {
                        if sent_close.is_none() {
                            let (opcode, payload) = match &m {
                                WsMessage::Text(t) => (OP_TEXT, Bytes::copy_from_slice(t.as_bytes())),
                                WsMessage::Binary(b) => (OP_BINARY, b.clone()),
                            };
                            reporter.emit(NetEvent::WebSocketFrameReceived {
                                id,
                                opcode,
                                mask: false,
                                payload,
                                timestamp: reporter.now(),
                            });
                            reporter.deliver(WsEvent::Message(m));
                        }
                    }
                    Ok(Incoming::Close(code, reason)) => {
                        if sent_close.is_none() {
                            // Echo the code, as Chrome does.
                            let body = if code == 1005 { Vec::new() } else { code.to_be_bytes().to_vec() };
                            let _ = wr.write_all(&encode_frame(OP_CLOSE, &body, Some(random()))).await;
                        }
                        let _ = wr.shutdown().await;
                        break 'conn Ok((code, reason));
                    }
                    Err(f) => break 'conn Err(f),
                },
                Err(f) => break 'conn Err(f),
            }
        }
        let close_deadline = sent_close.map(|t| t + Duration::from_secs(10));
        tokio::select! {
            read = rd.read(&mut read_buf) => match read {
                Ok(0) | Err(_) => break 'conn Err(fail(0, "")),
                Ok(n) => buf.extend_from_slice(&read_buf[..n]),
            },
            command = commands.recv(), if commands_open => {
                let command = match command {
                    Some(c) => c,
                    // The page is gone: going away.
                    None => {
                        commands_open = false;
                        WsCmd::Close(Some(1001), String::new())
                    }
                };
                match command {
                    WsCmd::Send(m) => {
                        let len = m.len() as u64;
                        if sent_close.is_none() {
                            let (opcode, payload) = match m {
                                WsMessage::Text(t) => (OP_TEXT, Bytes::from(t.into_bytes())),
                                WsMessage::Binary(b) => (OP_BINARY, b),
                            };
                            let written = wr.write_all(&encode_frame(opcode, &payload, Some(random()))).await;
                            reporter.emit(NetEvent::WebSocketFrameSent {
                                id,
                                opcode,
                                mask: true,
                                payload,
                                timestamp: reporter.now(),
                            });
                            if written.is_err() {
                                buffered.fetch_sub(len, Ordering::Relaxed);
                                break 'conn Err(fail(0, ""));
                            }
                        }
                        buffered.fetch_sub(len, Ordering::Relaxed);
                    }
                    WsCmd::Close(code, reason) => {
                        if sent_close.is_none() {
                            let mut body = Vec::new();
                            if let Some(code) = code {
                                body.extend_from_slice(&code.to_be_bytes());
                                body.extend_from_slice(reason.as_bytes());
                            }
                            let _ = wr.write_all(&encode_frame(OP_CLOSE, &body, Some(random()))).await;
                            sent_close = Some(tokio::time::Instant::now());
                        }
                    }
                }
            },
            _ = async { tokio::time::sleep_until(close_deadline.unwrap()).await }, if close_deadline.is_some() => {
                break 'conn Err(fail(0, ""));
            }
        }
    };
    match outcome {
        Ok((code, reason)) => reporter.closed(code, reason, true, None),
        // The connection ended without a closing handshake.
        Err(Failure { code: 0, .. }) => reporter.closed(1006, String::new(), false, None),
        Err(Failure { code, message }) => {
            let _ = wr.write_all(&encode_frame(OP_CLOSE, &code.to_be_bytes(), Some(random()))).await;
            let _ = wr.shutdown().await;
            reporter.closed(1006, String::new(), false, Some(message))
        }
    }
}

fn io_error_text(e: &std::io::Error) -> &'static str {
    match e.kind() {
        std::io::ErrorKind::ConnectionRefused => "ERR_CONNECTION_REFUSED",
        std::io::ErrorKind::ConnectionReset => "ERR_CONNECTION_RESET",
        std::io::ErrorKind::UnexpectedEof => "ERR_CONNECTION_CLOSED",
        std::io::ErrorKind::TimedOut => "ERR_TIMED_OUT",
        _ => "ERR_FAILED",
    }
}

/// TCP (and TLS for `wss:`) to the socket's host.
async fn connect(shared: &Shared, url: &Url) -> Result<Box<dyn Stream>, String> {
    let host = url.host_str().ok_or("net::ERR_INVALID_URL")?;
    let port = url.port_or_known_default().unwrap_or(if url.scheme() == "wss" { 443 } else { 80 });
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    let tcp = tokio::net::TcpStream::connect((bare, port)).await.map_err(|e| {
        if e.kind() == std::io::ErrorKind::ConnectionRefused {
            "net::ERR_CONNECTION_REFUSED".to_owned()
        } else if e.to_string().contains("lookup") || e.to_string().contains("nodename") {
            "net::ERR_NAME_NOT_RESOLVED".to_owned()
        } else {
            format!("net::{}", io_error_text(&e))
        }
    })?;
    let _ = tcp.set_nodelay(true);
    if url.scheme() != "wss" {
        return Ok(Box::new(tcp));
    }
    let config = if shared.ignore_certificate_errors.load(Ordering::Relaxed) {
        super::tls_config_insecure()
    } else {
        super::tls_config()
    };
    let name = rustls::pki_types::ServerName::try_from(bare.to_owned()).map_err(|_| "net::ERR_INVALID_URL")?;
    let tls =
        tokio_rustls::TlsConnector::from(config).connect(name, tcp).await.map_err(
            |e| match super::certificate_error(&e) {
                Some(_) => "net::ERR_CERT_AUTHORITY_INVALID".to_owned(),
                None => "net::ERR_SSL_PROTOCOL_ERROR".to_owned(),
            },
        )?;
    Ok(Box::new(tls))
}

/// The opening handshake's headers, in Chrome's order.
fn handshake_headers(shared: &Shared, cookies: &Mutex<CookieJar>, req: &WsRequest, key: &str) -> HeaderMap {
    let settings = shared.settings.lock().unwrap().clone();
    let cfg = &shared.config;
    let mut h = HeaderMap::new();
    let host = match req.url.port() {
        Some(p) => format!("{}:{p}", req.url.host_str().unwrap_or_default()),
        None => req.url.host_str().unwrap_or_default().to_owned(),
    };
    let mut put = |name: HeaderName, value: &str| {
        if let Ok(v) = HeaderValue::from_str(value) {
            h.insert(name, v);
        }
    };
    put(header::HOST, &host);
    put(header::CONNECTION, "Upgrade");
    put(header::PRAGMA, "no-cache");
    put(header::CACHE_CONTROL, "no-cache");
    let ua = req
        .defaults
        .user_agent
        .as_ref()
        .and_then(|v| v.to_str().ok().map(str::to_owned))
        .unwrap_or_else(|| settings.user_agent.clone().unwrap_or_else(|| cfg.user_agent.clone()));
    put(header::USER_AGENT, &ua);
    put(header::UPGRADE, "websocket");
    if let Some(origin) = &req.origin {
        put(header::ORIGIN, origin);
    }
    put(header::SEC_WEBSOCKET_VERSION, "13");
    let lang = req
        .defaults
        .accept_language
        .as_ref()
        .and_then(|v| v.to_str().ok().map(str::to_owned))
        .unwrap_or_else(|| settings.accept_language.clone().unwrap_or_else(|| cfg.accept_language.clone()));
    put(header::ACCEPT_LANGUAGE, &lang);
    let target = http_url(&req.url);
    let ctx = site_context(req.document_url.as_ref(), &target, false, true);
    if let Some(cookie) = cookies.lock().unwrap().header_for(&target, ctx, crate::cookies::now_secs()) {
        put(header::COOKIE, &cookie);
    }
    put(header::SEC_WEBSOCKET_KEY, key);
    if !req.protocols.is_empty() {
        put(header::SEC_WEBSOCKET_PROTOCOL, &req.protocols.join(", "));
    }
    for (k, v) in settings.extra_headers.iter().chain(req.defaults.extra_headers.iter()) {
        h.insert(k.clone(), v.clone());
    }
    h
}

fn request_bytes(url: &Url, headers: &HeaderMap) -> Vec<u8> {
    let mut target = url.path().to_owned();
    if let Some(q) = url.query() {
        target.push('?');
        target.push_str(q);
    }
    let mut out = format!("GET {target} HTTP/1.1\r\n").into_bytes();
    for (name, value) in headers {
        out.extend_from_slice(name.as_str().as_bytes());
        out.extend_from_slice(b": ");
        out.extend_from_slice(value.as_bytes());
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(b"\r\n");
    out
}

pub(crate) struct Head {
    pub(crate) status: u16,
    pub(crate) reason: String,
    pub(crate) headers: HeaderMap,
}

/// Reads the response head; bytes after it stay in `buf`.
async fn read_head(stream: &mut (impl AsyncRead + Unpin), buf: &mut BytesMut) -> std::io::Result<Head> {
    let mut chunk = [0u8; 4096];
    loop {
        if let Some(head) = parse_head(buf)? {
            return Ok(head);
        }
        if buf.len() > 64 * 1024 {
            return Err(std::io::Error::other("response headers too large"));
        }
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

pub(crate) fn parse_head(buf: &mut BytesMut) -> std::io::Result<Option<Head>> {
    let mut slots = [httparse::EMPTY_HEADER; 64];
    let mut res = httparse::Response::new(&mut slots);
    let n = match res.parse(buf).map_err(std::io::Error::other)? {
        httparse::Status::Partial => return Ok(None),
        httparse::Status::Complete(n) => n,
    };
    let mut headers = HeaderMap::new();
    for h in res.headers.iter() {
        if let (Ok(name), Ok(value)) = (HeaderName::from_bytes(h.name.as_bytes()), HeaderValue::from_bytes(h.value)) {
            headers.append(name, value);
        }
    }
    let head = Head { status: res.code.unwrap_or(0), reason: res.reason.unwrap_or("").to_owned(), headers };
    buf.advance(n);
    Ok(Some(head))
}

/// Validates a handshake response (§4.1), returning the chosen subprotocol.
pub(crate) fn check_response(head: &Head, key: &str, protocols: &[String]) -> Result<String, String> {
    if head.status != 101 {
        return Err(format!("Unexpected response code: {}", head.status));
    }
    let get = |name: HeaderName| head.headers.get(name).and_then(|v| v.to_str().ok()).map(str::trim);
    if !get(header::UPGRADE).is_some_and(|v| v.eq_ignore_ascii_case("websocket")) {
        return Err(match get(header::UPGRADE) {
            None => "'Upgrade' header is missing".into(),
            Some(v) => format!("'Upgrade' header value is not 'WebSocket': {v}"),
        });
    }
    if !get(header::CONNECTION).is_some_and(|v| v.split(',').any(|t| t.trim().eq_ignore_ascii_case("upgrade"))) {
        return Err(match get(header::CONNECTION) {
            None => "'Connection' header is missing".into(),
            Some(v) => format!("'Connection' header value must contain 'Upgrade': {v}"),
        });
    }
    match get(header::SEC_WEBSOCKET_ACCEPT) {
        None => return Err("'Sec-WebSocket-Accept' header is missing".into()),
        Some(v) if v != accept_key(key) => return Err("Incorrect 'Sec-WebSocket-Accept' header value".into()),
        Some(_) => {}
    }
    if get(header::SEC_WEBSOCKET_EXTENSIONS).is_some_and(|v| !v.is_empty()) {
        return Err("'Sec-WebSocket-Extensions' header must not appear in response".into());
    }
    match get(header::SEC_WEBSOCKET_PROTOCOL) {
        None if !protocols.is_empty() => {
            Err("Sent non-empty 'Sec-WebSocket-Protocol' header but no response was received".into())
        }
        None => Ok(String::new()),
        Some(p) if protocols.iter().any(|r| r == p) => Ok(p.to_owned()),
        Some(_) if protocols.is_empty() => {
            Err("Response must not include 'Sec-WebSocket-Protocol' header if not present in request".into())
        }
        Some(_) => Err("'Sec-WebSocket-Protocol' header value doesn't match any of the requested values".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accept_key_matches_rfc_example() {
        assert_eq!(accept_key("dGhlIHNhbXBsZSBub25jZQ=="), "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    }

    #[test]
    fn frames_round_trip_through_lengths() {
        for len in [0usize, 5, 125, 126, 65535, 65536] {
            let payload = vec![7u8; len];
            let mut buf = BytesMut::from(&encode_frame(OP_BINARY, &payload, None)[..]);
            let frame = parse_frame(&mut buf, usize::MAX).unwrap().unwrap();
            assert_eq!((frame.fin, frame.opcode, frame.payload.len()), (true, OP_BINARY, len));
            assert!(buf.is_empty());
        }
    }

    #[test]
    fn partial_frames_wait_and_masked_server_frames_fail() {
        let whole = encode_frame(OP_TEXT, b"hello", None);
        let mut buf = BytesMut::from(&whole[..3]);
        assert_eq!(parse_frame(&mut buf, 100).unwrap(), None);
        buf.extend_from_slice(&whole[3..]);
        assert_eq!(parse_frame(&mut buf, 100).unwrap().unwrap().payload, Bytes::from_static(b"hello"));
        let mut masked = BytesMut::from(&encode_frame(OP_TEXT, b"x", Some([1, 2, 3, 4]))[..]);
        assert_eq!(parse_frame(&mut masked, 100).unwrap_err().code, 1002);
    }

    #[test]
    fn fragments_assemble_and_bad_utf8_fails() {
        let mut a = Assembler::new(1024);
        let frame = |fin, opcode, p: &'static [u8]| Frame { fin, opcode, payload: Bytes::from_static(p) };
        assert_eq!(a.push(frame(false, OP_TEXT, b"he")).unwrap(), Incoming::Nothing);
        assert_eq!(a.push(frame(false, OP_CONTINUATION, b"ll")).unwrap(), Incoming::Nothing);
        assert_eq!(
            a.push(frame(true, OP_CONTINUATION, b"o")).unwrap(),
            Incoming::Message(WsMessage::Text("hello".into()))
        );
        assert_eq!(a.push(frame(true, OP_TEXT, b"\xff")).unwrap_err().code, 1007);
        assert_eq!(a.push(frame(true, OP_CLOSE, b"\x03\xe8bye")).unwrap(), Incoming::Close(1000, "bye".into()));
        assert_eq!(a.push(frame(true, OP_CLOSE, b"")).unwrap(), Incoming::Close(1005, String::new()));
    }

    #[test]
    fn handshake_response_is_checked() {
        let key = "dGhlIHNhbXBsZSBub25jZQ==";
        let raw = "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\nSec-WebSocket-Protocol: chat\r\n\r\nrest";
        let mut buf = BytesMut::from(raw.as_bytes());
        let head = parse_head(&mut buf).unwrap().unwrap();
        assert_eq!(&buf[..], b"rest");
        assert_eq!(check_response(&head, key, &["chat".into()]).unwrap(), "chat");
        assert!(check_response(&head, key, &[]).is_err());
        assert!(check_response(&head, "other", &["chat".into()]).unwrap_err().contains("Accept"));
    }
}
