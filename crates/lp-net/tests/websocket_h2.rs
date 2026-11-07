//! WebSockets over HTTP/2 (RFC 8441) against a loopback TLS server that
//! speaks h2 (with or without `SETTINGS_ENABLE_CONNECT_PROTOCOL`) and
//! HTTP/1.1, so both the extended CONNECT path and the fallback run.

use std::net::TcpListener as StdListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};

use base64::Engine;
use bytes::Bytes;
use flate2::{Compress, Compression, Decompress, FlushCompress, FlushDecompress};
use http::{HeaderMap, Response};
use lp_net::*;
use sha1::{Digest, Sha1};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::client::collect;
use crate::websocket::{next, next_message, open};

const TAIL: [u8; 4] = [0, 0, 0xff, 0xff];
const NO_CONTEXT: &str = "permessage-deflate; server_no_context_takeover; client_no_context_takeover";

/// Paths: `/echo` echoes data messages (`ping-me` gets a ping, whose pong
/// comes back as text `pong:<payload>`; `close-me` gets Close 1000 "done")
/// and echoes a Close; `/server-close` closes with 4001 "bye" first;
/// `/deflate` negotiates permessage-deflate; `/reject` answers 403;
/// `/wrong-protocol` selects a subprotocol that was not offered;
/// `/set-cookie` sets `ws=1`. Every handshake logs `<h1|h2> <path>` and its
/// interesting headers; every data message logs `rsv1=<bool>`.
struct Server {
    port: u16,
    log: Arc<Mutex<Vec<String>>>,
    connections: Arc<AtomicUsize>,
}

impl Server {
    fn start(connect_protocol: bool) -> Server {
        use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
        let cert = CertificateDer::from(include_bytes!("data/self-signed.der").to_vec());
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(include_bytes!("data/self-signed.key.der").to_vec()));
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut config = rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![cert], key)
            .unwrap();
        config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
        let listener = StdListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let log: Arc<Mutex<Vec<String>>> = Arc::default();
        let connections = Arc::new(AtomicUsize::new(0));
        let (log2, count) = (log.clone(), connections.clone());
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            let local = tokio::task::LocalSet::new();
            local.block_on(&rt, async move {
                let listener = TcpListener::from_std(listener).unwrap();
                loop {
                    let Ok((tcp, _)) = listener.accept().await else { continue };
                    count.fetch_add(1, Ordering::SeqCst);
                    let (acceptor, log) = (acceptor.clone(), log2.clone());
                    tokio::task::spawn_local(async move {
                        let Ok(tls) = acceptor.accept(tcp).await else { return };
                        if tls.get_ref().1.alpn_protocol() == Some(b"h2") {
                            serve_h2(tls, connect_protocol, log).await;
                        } else {
                            serve_h1(tls, log).await;
                        }
                    });
                }
            });
        });
        Server { port, log, connections }
    }

    fn log(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
}

fn client() -> NetClient {
    let c = NetClient::new(NetConfig { shared_asset_cache: None, ..NetConfig::default() }).unwrap();
    c.set_ignore_certificate_errors(true);
    c
}

/// An ordinary GET, which leaves an HTTP/2 connection in the pool.
fn warm(c: &NetClient, port: u16) {
    let url = Url::parse(&format!("https://127.0.0.1:{port}/warm")).unwrap();
    let (_, head, body, error) = collect(c, Request::get(url, Destination::Fetch));
    assert_eq!(error, None);
    assert_eq!(head.unwrap().protocol, http::Version::HTTP_2);
    assert_eq!(body, b"h2 /warm");
}

fn request(port: u16, path: &str, protocols: &[&str]) -> WebSocketRequest {
    WebSocketRequest {
        url: format!("wss://127.0.0.1:{port}{path}").parse().unwrap(),
        protocols: protocols.iter().map(|p| p.to_string()).collect(),
        origin: format!("https://127.0.0.1:{port}"),
        frame_id: Some("F".into()),
        defaults: Default::default(),
    }
}

fn header(h: &HeaderMap, name: &str) -> String {
    h.get(name).and_then(|v| v.to_str().ok()).unwrap_or("").to_owned()
}

fn describe(kind: &str, path: &str, h: &HeaderMap) -> String {
    format!(
        "{kind} {path} origin={} cookie={} protocol={} version={} ext={} host={} key={}",
        header(h, "origin"),
        header(h, "cookie"),
        header(h, "sec-websocket-protocol"),
        header(h, "sec-websocket-version"),
        header(h, "sec-websocket-extensions"),
        h.contains_key("host"),
        h.contains_key("sec-websocket-key"),
    )
}

/// The handshake answer's extra headers for `path`, or `None` to refuse.
fn answer_headers(path: &str, h: &HeaderMap) -> Option<Vec<(&'static str, String)>> {
    let route = path.split('?').next().unwrap_or("");
    if route == "/reject" {
        return None;
    }
    let mut out = Vec::new();
    let offered = header(h, "sec-websocket-protocol");
    if route == "/wrong-protocol" {
        out.push(("sec-websocket-protocol", "nope".to_owned()));
    } else if let Some(first) = offered.split(',').next().filter(|p| !p.is_empty()) {
        out.push(("sec-websocket-protocol", first.trim().to_owned()));
    }
    if route == "/deflate" {
        out.push(("sec-websocket-extensions", NO_CONTEXT.to_owned()));
    }
    if route == "/set-cookie" {
        out.push(("set-cookie", "ws=1; Path=/".to_owned()));
    }
    Some(out)
}

async fn serve_h2<T>(tls: T, connect_protocol: bool, log: Arc<Mutex<Vec<String>>>)
where
    T: AsyncRead + AsyncWrite + Unpin + 'static,
{
    let mut builder = h2::server::Builder::new();
    if connect_protocol {
        builder.enable_connect_protocol();
    }
    let Ok(mut connection) = builder.handshake::<_, Bytes>(tls).await else { return };
    while let Some(Ok((request, mut respond))) = connection.accept().await {
        let log = log.clone();
        tokio::task::spawn_local(async move {
            let path = request.uri().path_and_query().map(|p| p.as_str().to_owned()).unwrap_or_default();
            let websocket = request.method() == http::Method::CONNECT
                && request.extensions().get::<h2::ext::Protocol>().is_some_and(|p| p.as_str() == "websocket");
            if !websocket {
                let response = Response::builder().status(200).body(()).unwrap();
                if let Ok(mut stream) = respond.send_response(response, false) {
                    let _ = stream.send_data(Bytes::from(format!("h2 {path}")), true);
                }
                return;
            }
            log.lock().unwrap().push(describe("h2", &path, request.headers()));
            let Some(extra) = answer_headers(&path, request.headers()) else {
                let _ = respond.send_response(Response::builder().status(403).body(()).unwrap(), true);
                return;
            };
            let mut response = Response::builder().status(200);
            for (k, v) in &extra {
                response = response.header(*k, v);
            }
            let Ok(send) = respond.send_response(response.body(()).unwrap(), false) else { return };
            let (near, far) = tokio::io::duplex(1 << 20);
            tokio::task::spawn_local(bridge(request.into_body(), send, near));
            let deflate = extra.iter().any(|(k, _)| *k == "sec-websocket-extensions");
            session(far, &path, deflate, &log).await;
        });
    }
}

/// Pumps an HTTP/2 stream's DATA to and from `io`.
async fn bridge(mut recv: h2::RecvStream, mut send: h2::SendStream<Bytes>, io: tokio::io::DuplexStream) {
    let (mut rd, mut wr) = tokio::io::split(io);
    let up = async move {
        while let Some(Ok(chunk)) = recv.data().await {
            let _ = recv.flow_control().release_capacity(chunk.len());
            if wr.write_all(&chunk).await.is_err() {
                break;
            }
        }
        let _ = wr.shutdown().await;
    };
    let down = async move {
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            let n = rd.read(&mut buf).await.unwrap_or(0);
            if n == 0 {
                let _ = send.send_data(Bytes::new(), true);
                break;
            }
            if send.send_data(Bytes::copy_from_slice(&buf[..n]), false).is_err() {
                break;
            }
        }
    };
    tokio::join!(up, down);
}

async fn serve_h1<T: AsyncRead + AsyncWrite + Unpin>(mut tls: T, log: Arc<Mutex<Vec<String>>>) {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if tls.read(&mut byte).await.unwrap_or(0) == 0 {
            return;
        }
        head.push(byte[0]);
    }
    let text = String::from_utf8_lossy(&head).into_owned();
    let mut lines = text.split("\r\n");
    let path = lines.next().unwrap_or("").split_whitespace().nth(1).unwrap_or("").to_owned();
    let mut headers = HeaderMap::new();
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            if let (Ok(k), Ok(v)) = (http::HeaderName::from_bytes(k.trim().as_bytes()), v.trim().parse()) {
                headers.append(k, v);
            }
        }
    }
    log.lock().unwrap().push(describe("h1", &path, &headers));
    let Some(extra) = answer_headers(&path, &headers) else {
        let _ = tls.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n").await;
        return;
    };
    let digest = Sha1::digest(format!("{}258EAFA5-E914-47DA-95CA-C5AB0DC85B11", header(&headers, "sec-websocket-key")));
    let accept = base64::engine::general_purpose::STANDARD.encode(digest);
    let mut response = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n"
    );
    for (k, v) in &extra {
        response.push_str(&format!("{k}: {v}\r\n"));
    }
    response.push_str("\r\n");
    let _ = tls.write_all(response.as_bytes()).await;
    let deflate = extra.iter().any(|(k, _)| *k == "sec-websocket-extensions");
    session(tls, &path, deflate, &log).await;
}

/// (rsv1, opcode, unmasked payload) of the next client frame.
async fn read_frame<T: AsyncRead + Unpin>(io: &mut T) -> Option<(bool, u8, Vec<u8>)> {
    let mut head = [0u8; 2];
    io.read_exact(&mut head).await.ok()?;
    let mut len = (head[1] & 0x7f) as u64;
    if len == 126 {
        len = io.read_u16().await.ok()? as u64;
    } else if len == 127 {
        len = io.read_u64().await.ok()?;
    }
    assert!(head[1] & 0x80 != 0, "client frames are masked");
    let mut key = [0u8; 4];
    io.read_exact(&mut key).await.ok()?;
    let mut payload = vec![0u8; len as usize];
    io.read_exact(&mut payload).await.ok()?;
    for (i, b) in payload.iter_mut().enumerate() {
        *b ^= key[i % 4];
    }
    Some((head[0] & 0x40 != 0, head[0] & 0x0f, payload))
}

async fn write_frame<T: AsyncWrite + Unpin>(io: &mut T, first: u8, payload: &[u8]) {
    let mut out = vec![first];
    match payload.len() {
        n @ 0..=125 => out.push(n as u8),
        n @ 126..=0xffff => {
            out.push(126);
            out.extend_from_slice(&(n as u16).to_be_bytes());
        }
        n => {
            out.push(127);
            out.extend_from_slice(&(n as u64).to_be_bytes());
        }
    }
    out.extend_from_slice(payload);
    let _ = io.write_all(&out).await;
    let _ = io.flush().await;
}

/// One WebSocket session (no context takeover when deflating).
async fn session<T: AsyncRead + AsyncWrite + Unpin>(mut io: T, path: &str, deflate: bool, log: &Mutex<Vec<String>>) {
    let route = path.split('?').next().unwrap_or("").to_owned();
    // The server sent its Close first: it does not echo the client's.
    let mut closing = route == "/server-close";
    if closing {
        write_frame(&mut io, 0x88, &[&4001u16.to_be_bytes()[..], b"bye"].concat()).await;
    }
    while let Some((rsv1, opcode, payload)) = read_frame(&mut io).await {
        match opcode {
            0x8 => {
                if !closing {
                    write_frame(&mut io, 0x88, &payload).await;
                }
                let _ = io.shutdown().await;
                return;
            }
            0x9 => write_frame(&mut io, 0x8A, &payload).await,
            0xA => write_frame(&mut io, 0x81, &[b"pong:".as_slice(), &payload].concat()).await,
            0x1 | 0x2 => {
                log.lock().unwrap().push(format!("rsv1={rsv1}"));
                let message = if rsv1 {
                    let mut input = payload;
                    input.extend_from_slice(&TAIL);
                    let mut out = Vec::with_capacity(1 << 20);
                    Decompress::new(false).decompress_vec(&input, &mut out, FlushDecompress::Sync).unwrap();
                    out
                } else {
                    payload
                };
                if message == b"ping-me" {
                    write_frame(&mut io, 0x89, b"p").await;
                } else if message == b"close-me" {
                    closing = true;
                    write_frame(&mut io, 0x88, &[&1000u16.to_be_bytes()[..], b"done"].concat()).await;
                } else if deflate {
                    let mut out = Vec::with_capacity(message.len() + 1024);
                    Compress::new(Compression::default(), false)
                        .compress_vec(&message, &mut out, FlushCompress::Sync)
                        .unwrap();
                    out.truncate(out.len() - 4);
                    write_frame(&mut io, 0xC0 | opcode, &out).await;
                } else {
                    write_frame(&mut io, 0x80 | opcode, &message).await;
                }
            }
            _ => {}
        }
    }
}

fn expect_open(rx: &mpsc::Receiver<WsEvent>) -> (String, String, HeaderMap) {
    match next(rx) {
        WsEvent::Open { protocol, extensions, headers } => (protocol, extensions, headers),
        other => panic!("expected open, got {other:?}"),
    }
}

fn expect_closed(rx: &mpsc::Receiver<WsEvent>) -> (bool, u16, String, bool) {
    match next_message(rx) {
        WsEvent::Closed { clean, code, reason, failed } => (clean, code, reason, failed),
        other => panic!("expected close, got {other:?}"),
    }
}

#[test]
fn h2_websocket_echoes_on_the_pooled_connection_and_closes_cleanly() {
    let server = Server::start(true);
    let c = client();
    warm(&c, server.port);
    let (_, ws, rx) = open(&c, request(server.port, "/echo?x=1", &["chat", "superchat"]));
    let (protocol, extensions, _) = expect_open(&rx);
    assert_eq!((protocol.as_str(), extensions.as_str()), ("chat", ""));
    let log = server.log()[0].clone();
    assert!(log.starts_with("h2 /echo?x=1 origin=https://127.0.0.1:"), "{log}");
    assert!(log.contains("protocol=chat, superchat version=13"), "{log}");
    assert!(log.contains("ext=permessage-deflate; client_max_window_bits"), "{log}");
    assert!(log.ends_with("host=false key=false"), "{log}");

    ws.send_text("hello".into());
    assert!(matches!(next(&rx), WsEvent::Sent(5)));
    assert!(matches!(next(&rx), WsEvent::Text(t) if t == "hello"));
    // 200 000 bytes each way: past the 64 KiB HTTP/2 stream window.
    let big = (0..200_000u32).map(|i| i as u8).collect::<Vec<_>>();
    ws.send_binary(big.clone().into());
    assert!(matches!(next_message(&rx), WsEvent::Binary(b) if b[..] == big[..]));
    ws.send_text("ping-me".into());
    assert!(matches!(next_message(&rx), WsEvent::Text(t) if t == "pong:p"));
    ws.close(Some(1000), "done".into());
    assert_eq!(expect_closed(&rx), (true, 1000, "done".to_owned(), false));
    assert_eq!(server.connections.load(Ordering::SeqCst), 1, "one connection for the GET and the WebSocket");

    // The connection stays usable for requests and further WebSockets.
    warm(&c, server.port);
    let (_, _ws, rx) = open(&c, request(server.port, "/echo", &[]));
    expect_open(&rx);
    assert_eq!(server.connections.load(Ordering::SeqCst), 1);
}

#[test]
fn h2_websocket_deflates_messages() {
    let server = Server::start(true);
    let c = client();
    warm(&c, server.port);
    let (_, ws, rx) = open(&c, request(server.port, "/deflate", &[]));
    let (_, extensions, _) = expect_open(&rx);
    assert_eq!(extensions, NO_CONTEXT);
    let text = "deflate me ".repeat(100);
    ws.send_text(text.clone());
    assert!(matches!(next_message(&rx), WsEvent::Text(t) if t == text));
    ws.send_binary(Bytes::from(vec![9u8; 5000]));
    assert!(matches!(next_message(&rx), WsEvent::Binary(b) if b[..] == [9u8; 5000][..]));
    assert_eq!(&server.log()[1..], ["rsv1=true", "rsv1=true"]);
    assert!(server.log()[0].starts_with("h2 /deflate"));
    ws.close(Some(1000), String::new());
    assert!(expect_closed(&rx).0);
}

#[test]
fn h2_websocket_close_codes() {
    let server = Server::start(true);
    let c = client();
    warm(&c, server.port);
    let (_, _ws, rx) = open(&c, request(server.port, "/server-close", &[]));
    expect_open(&rx);
    assert_eq!(expect_closed(&rx), (true, 4001, "bye".to_owned(), false));

    let (_, ws, rx) = open(&c, request(server.port, "/echo", &[]));
    expect_open(&rx);
    ws.send_text("close-me".into());
    assert_eq!(expect_closed(&rx), (true, 1000, "done".to_owned(), false));

    let (_, ws, rx) = open(&c, request(server.port, "/echo", &[]));
    expect_open(&rx);
    ws.close(Some(3001), "custom".into());
    assert_eq!(expect_closed(&rx), (true, 3001, "custom".to_owned(), false));

    let (_, ws, rx) = open(&c, request(server.port, "/echo", &[]));
    expect_open(&rx);
    ws.close(None, String::new());
    assert_eq!(expect_closed(&rx), (true, 1005, String::new(), false));
    let handshakes = server.log().into_iter().filter(|l| !l.starts_with("rsv1=")).collect::<Vec<_>>();
    assert!(handshakes.len() == 4 && handshakes.iter().all(|l| l.starts_with("h2 ")), "{handshakes:?}");
    assert_eq!(server.connections.load(Ordering::SeqCst), 1);
}

#[test]
fn h2_websocket_subprotocols_and_refusals() {
    let server = Server::start(true);
    let c = client();
    warm(&c, server.port);
    let events = c.subscribe();
    let (id, ws, rx) = open(&c, request(server.port, "/echo", &["v2", "v1"]));
    assert_eq!(expect_open(&rx).0, "v2");
    drop(ws);
    // No subprotocol offered, none selected.
    let (_, _ws, rx) = open(&c, request(server.port, "/echo", &[]));
    assert_eq!(expect_open(&rx).0, "");
    for (path, protocols) in [("/wrong-protocol", &["chat"][..]), ("/reject", &[][..])] {
        let (_, _ws, rx) = open(&c, request(server.port, path, protocols));
        assert!(
            matches!(next(&rx), WsEvent::Closed { clean: false, code: 1006, failed: true, .. }),
            "{path} must fail the connection"
        );
    }
    // Refusals are the server's answer: no HTTP/1.1 retry.
    assert!(server.log().iter().all(|l| l.starts_with("h2 ")), "{:?}", server.log());
    let mut statuses = Vec::new();
    while let Ok(event) = events.try_recv() {
        if let NetEvent::WebSocket { id: event_id, activity, .. } = event {
            if let WebSocketActivity::HandshakeResponseReceived { status, status_text, .. } = *activity {
                if event_id == id {
                    assert_eq!(status_text, "OK");
                }
                statuses.push(status);
            }
        }
    }
    assert_eq!(statuses, [200, 200, 200, 403]);
}

#[test]
fn h2_websocket_sends_cookies_and_reports_set_cookie() {
    let server = Server::start(true);
    let c = client();
    let origin: Url = format!("https://127.0.0.1:{}/", server.port).parse().unwrap();
    c.cookies().lock().unwrap().set_from_header(&origin, "sid=7; Path=/", cookies::Access::Http, cookies::now_secs());
    warm(&c, server.port);
    let req = request(server.port, "/set-cookie", &[]);
    let (_, _ws, rx) = open(&c, req.clone());
    let (_, _, headers) = expect_open(&rx);
    assert!(server.log()[0].starts_with("h2 /set-cookie"));
    assert!(server.log()[0].contains("cookie=sid=7"), "{:?}", server.log());
    c.store_websocket_cookies(&req.url, &headers);
    let jar = c.cookies().lock().unwrap().header_for(&origin, cookies::SiteContext::SameSite, cookies::now_secs());
    assert!(jar.unwrap().contains("ws=1"));
}

#[test]
fn without_the_setting_or_a_pooled_connection_websockets_use_http1() {
    // The server speaks h2 but does not allow the extended CONNECT.
    let server = Server::start(false);
    let c = client();
    warm(&c, server.port);
    let (_, ws, rx) = open(&c, request(server.port, "/deflate", &["chat"]));
    let (protocol, extensions, _) = expect_open(&rx);
    assert_eq!((protocol.as_str(), extensions.as_str()), ("chat", NO_CONTEXT));
    let log = server.log()[0].clone();
    assert!(log.starts_with("h1 /deflate") && log.ends_with("host=true key=true"), "{log}");
    ws.send_text("over http/1.1".into());
    assert!(matches!(next_message(&rx), WsEvent::Text(t) if t == "over http/1.1"));
    ws.close(Some(1000), String::new());
    assert!(expect_closed(&rx).0);
    assert_eq!(server.connections.load(Ordering::SeqCst), 2);

    // The setting is on but nothing is pooled yet: Chrome does not open an
    // HTTP/2 connection for a WebSocket either.
    let server = Server::start(true);
    let c = client();
    let (_, _ws, rx) = open(&c, request(server.port, "/echo", &[]));
    expect_open(&rx);
    assert!(server.log()[0].starts_with("h1 /echo"), "{:?}", server.log());
}

#[test]
fn h2_websocket_runs_inside_the_proxy_tunnel() {
    let server = Server::start(true);
    let proxy = crate::proxy::Proxy::start(crate::proxy::Mode::Open);
    let c = proxy.client("<-loopback>");
    c.set_ignore_certificate_errors(true);
    warm(&c, server.port);
    let (_, ws, rx) = open(&c, request(server.port, "/echo", &[]));
    expect_open(&rx);
    ws.send_text("tunneled".into());
    assert!(matches!(next_message(&rx), WsEvent::Text(t) if t == "tunneled"));
    assert!(server.log()[0].starts_with("h2 /echo"));
    let heads = proxy.heads();
    assert_eq!(heads.len(), 1, "one tunnel for the GET and the WebSocket: {heads:?}");
    assert_eq!(server.connections.load(Ordering::SeqCst), 1);
}
