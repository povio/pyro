//! WebSocket client connections against a loopback RFC 6455 server.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use lp_net::*;
use sha1::{Digest, Sha1};

/// A blocking WebSocket server: `/echo` echoes data messages and answers a
/// Close with the same payload; `/server-close` closes with 4001 "bye" first;
/// `/reject` answers 403; `/bad-accept` a wrong accept key; `/set-cookie` sets
/// `ws=1`. Each handshake is logged with its interesting headers.
pub(crate) struct WsServer {
    pub(crate) port: u16,
    pub(crate) log: Arc<Mutex<Vec<String>>>,
}

pub(crate) fn start_ws_server() -> WsServer {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let log: Arc<Mutex<Vec<String>>> = Arc::default();
    let log2 = log.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let log = log2.clone();
            std::thread::spawn(move || serve(stream, &log));
        }
    });
    WsServer { port, log }
}

fn read_frame(reader: &mut impl Read) -> Option<(u8, Vec<u8>)> {
    let mut head = [0u8; 2];
    reader.read_exact(&mut head).ok()?;
    let mut len = (head[1] & 0x7f) as u64;
    if len == 126 {
        let mut b = [0u8; 2];
        reader.read_exact(&mut b).ok()?;
        len = u16::from_be_bytes(b) as u64;
    } else if len == 127 {
        let mut b = [0u8; 8];
        reader.read_exact(&mut b).ok()?;
        len = u64::from_be_bytes(b);
    }
    assert!(head[1] & 0x80 != 0, "client frames are masked");
    let mut key = [0u8; 4];
    reader.read_exact(&mut key).ok()?;
    let mut payload = vec![0u8; len as usize];
    reader.read_exact(&mut payload).ok()?;
    for (i, b) in payload.iter_mut().enumerate() {
        *b ^= key[i % 4];
    }
    Some((head[0] & 0x0f, payload))
}

fn write_frame(stream: &mut TcpStream, opcode: u8, payload: &[u8]) {
    let mut out = vec![0x80 | opcode];
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
    let _ = stream.write_all(&out);
}

fn serve(mut stream: TcpStream, log: &Mutex<Vec<String>>) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    if reader.read_line(&mut line).unwrap_or(0) == 0 {
        return;
    }
    let path = line.split_whitespace().nth(1).unwrap_or("").to_owned();
    let mut headers = Vec::new();
    loop {
        let mut h = String::new();
        reader.read_line(&mut h).unwrap();
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_owned()));
        }
    }
    let hv = |k: &str| headers.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone()).unwrap_or_default();
    log.lock().unwrap().push(format!(
        "{path} origin={} cookie={} protocol={} version={} upgrade={} auth={}",
        hv("origin"),
        hv("cookie"),
        hv("sec-websocket-protocol"),
        hv("sec-websocket-version"),
        hv("upgrade"),
        hv("authorization"),
    ));
    if path == "/reject" {
        let _ = stream.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n");
        return;
    }
    let accept = if path == "/bad-accept" {
        "x".to_owned()
    } else {
        let digest = Sha1::digest(format!("{}258EAFA5-E914-47DA-95CA-C5AB0DC85B11", hv("sec-websocket-key")));
        base64::engine::general_purpose::STANDARD.encode(digest)
    };
    let mut response =
        format!("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n");
    if let Some(first) = hv("sec-websocket-protocol").split(',').next().filter(|p| !p.is_empty()) {
        response.push_str(&format!("Sec-WebSocket-Protocol: {}\r\n", first.trim()));
    }
    if path.starts_with("/set-cookie") {
        response.push_str("Set-Cookie: ws=1; Path=/\r\n");
    }
    response.push_str("\r\n");
    let _ = stream.write_all(response.as_bytes());
    if path == "/server-close" {
        let mut payload = 4001u16.to_be_bytes().to_vec();
        payload.extend_from_slice(b"bye");
        write_frame(&mut stream, 0x8, &payload);
    }
    while let Some((opcode, payload)) = read_frame(&mut reader) {
        match opcode {
            0x8 => {
                if path != "/server-close" {
                    write_frame(&mut stream, 0x8, &payload);
                }
                let _ = stream.shutdown(std::net::Shutdown::Both);
                return;
            }
            0x9 => write_frame(&mut stream, 0xA, &payload),
            // The client's answer to our ping, reported back as text.
            0xA => write_frame(&mut stream, 0x1, &[b"pong:".as_slice(), &payload].concat()),
            0x1 if payload == b"close-me" => {
                let mut close = 1000u16.to_be_bytes().to_vec();
                close.extend_from_slice(b"done");
                write_frame(&mut stream, 0x8, &close);
            }
            0x1 if payload == b"ping-me" => write_frame(&mut stream, 0x9, b"p"),
            0x1 if payload == b"bad-frame" => {
                // Reserved bits set: a protocol error.
                let _ = stream.write_all(&[0xF1, 0]);
            }
            0x1 | 0x2 => write_frame(&mut stream, opcode, &payload),
            _ => {}
        }
    }
}

fn request(port: u16, path: &str, protocols: &[&str]) -> WebSocketRequest {
    WebSocketRequest {
        url: format!("ws://127.0.0.1:{port}{path}").parse().unwrap(),
        protocols: protocols.iter().map(|p| p.to_string()).collect(),
        origin: format!("http://127.0.0.1:{port}"),
        frame_id: Some("F".into()),
        defaults: Default::default(),
    }
}

pub(crate) fn open(client: &NetClient, req: WebSocketRequest) -> (RequestId, WebSocketHandle, mpsc::Receiver<WsEvent>) {
    let (tx, rx) = mpsc::channel();
    let (id, handle) = client.websocket(
        req,
        Box::new(move |event| {
            let _ = tx.send(event);
        }),
    );
    (id, handle, rx)
}

pub(crate) fn next(rx: &mpsc::Receiver<WsEvent>) -> WsEvent {
    rx.recv_timeout(Duration::from_secs(5)).expect("websocket event")
}

/// The next event that is not a `Sent` count.
pub(crate) fn next_message(rx: &mpsc::Receiver<WsEvent>) -> WsEvent {
    loop {
        match next(rx) {
            WsEvent::Sent(_) => continue,
            event => return event,
        }
    }
}

#[test]
fn websocket_echoes_text_and_binary_and_closes_cleanly() {
    let server = start_ws_server();
    let client = NetClient::new(Default::default()).unwrap();
    let (_, ws, rx) = open(&client, request(server.port, "/echo?x=1", &["chat", "superchat"]));
    match next(&rx) {
        WsEvent::Open { protocol, extensions, .. } => {
            assert_eq!(protocol, "chat");
            assert_eq!(extensions, "");
        }
        other => panic!("expected open, got {other:?}"),
    }
    let log = server.log.lock().unwrap()[0].clone();
    assert!(log.starts_with("/echo?x=1 origin=http://127.0.0.1:"), "{log}");
    assert!(log.contains("protocol=chat, superchat version=13 upgrade=websocket"), "{log}");
    ws.send_text("hello".into());
    assert!(matches!(next(&rx), WsEvent::Sent(5)));
    assert!(matches!(next(&rx), WsEvent::Text(t) if t == "hello"));
    // 70 000 bytes: a 64-bit length frame.
    let big = vec![7u8; 70_000];
    ws.send_binary(big.clone().into());
    assert!(matches!(next_message(&rx), WsEvent::Binary(b) if b[..] == big[..]));
    ws.send_text("ping-me".into());
    assert!(matches!(next_message(&rx), WsEvent::Text(t) if t == "pong:p"));
    ws.close(Some(1000), "done".into());
    match next_message(&rx) {
        WsEvent::Closed { clean, code, reason, failed } => {
            assert!(clean);
            assert_eq!((code, reason.as_str(), failed), (1000, "done", false));
        }
        other => panic!("expected close, got {other:?}"),
    }
}

#[test]
fn websocket_server_close_and_handshake_failures() {
    let server = start_ws_server();
    let client = NetClient::new(Default::default()).unwrap();
    let (_, _ws, rx) = open(&client, request(server.port, "/server-close", &[]));
    assert!(matches!(next(&rx), WsEvent::Open { .. }));
    match next(&rx) {
        WsEvent::Closed { clean, code, reason, failed } => {
            assert!(clean && !failed);
            assert_eq!((code, reason.as_str()), (4001, "bye"));
        }
        other => panic!("expected close, got {other:?}"),
    }
    for path in ["/reject", "/bad-accept"] {
        let (_, _ws, rx) = open(&client, request(server.port, path, &[]));
        assert!(
            matches!(next(&rx), WsEvent::Closed { clean: false, code: 1006, failed: true, .. }),
            "{path} must fail the connection"
        );
    }
    // A requested subprotocol the server does not select fails too: the
    // server below answers with the first offered one, so offer none and
    // check a protocol error frame instead.
    let (_, ws, rx) = open(&client, request(server.port, "/echo", &[]));
    assert!(matches!(next(&rx), WsEvent::Open { .. }));
    ws.send_text("bad-frame".into());
    assert!(matches!(next_message(&rx), WsEvent::Closed { clean: false, code: 1006, failed: true, .. }));
    // A bad port fails without connecting.
    let mut req = request(server.port, "/echo", &[]);
    req.url = "ws://127.0.0.1:6667/".parse().unwrap();
    let (_, _ws, rx) = open(&client, req);
    assert!(matches!(next(&rx), WsEvent::Closed { failed: true, .. }));
}

#[test]
fn websocket_sends_cookies_and_credentials_and_reports_set_cookie() {
    let server = start_ws_server();
    let client = NetClient::new(Default::default()).unwrap();
    let origin: Url = format!("http://127.0.0.1:{}/", server.port).parse().unwrap();
    client.cookies().lock().unwrap().set_from_header(
        &origin,
        "sid=7; Path=/",
        cookies::Access::Http,
        cookies::now_secs(),
    );
    let mut req = request(server.port, "/set-cookie", &[]);
    req.url = format!("ws://user:p%40ss@127.0.0.1:{}/set-cookie", server.port).parse().unwrap();
    let (_, ws, rx) = open(&client, req.clone());
    let WsEvent::Open { headers, .. } = next(&rx) else { panic!("expected open") };
    let log = server.log.lock().unwrap()[0].clone();
    assert!(log.contains("cookie=sid=7"), "{log}");
    assert!(log.contains("auth=Basic dXNlcjpwQHNz"), "{log}");
    // Stored only when the owner handles the open event.
    assert!(!client
        .cookies()
        .lock()
        .unwrap()
        .header_for(&origin, cookies::SiteContext::SameSite, cookies::now_secs())
        .unwrap()
        .contains("ws=1"));
    client.store_websocket_cookies(&req.url, &headers);
    let jar = client.cookies().lock().unwrap().header_for(&origin, cookies::SiteContext::SameSite, cookies::now_secs());
    assert!(jar.unwrap().contains("ws=1"));
    drop(ws);
}

#[test]
fn websocket_activity_is_observed_in_order() {
    let server = start_ws_server();
    let client = NetClient::new(Default::default()).unwrap();
    let events = client.subscribe();
    let (id, ws, rx) = open(&client, request(server.port, "/echo", &[]));
    assert!(matches!(next(&rx), WsEvent::Open { .. }));
    ws.send_text("hi".into());
    assert!(matches!(next_message(&rx), WsEvent::Text(_)));
    ws.close(None, String::new());
    assert!(matches!(next_message(&rx), WsEvent::Closed { code: 1005, clean: true, .. }));
    let mut seen = Vec::new();
    while let Ok(event) = events.try_recv() {
        if let NetEvent::WebSocket { id: event_id, activity, .. } = event {
            assert_eq!(event_id, id);
            seen.push(match *activity {
                WebSocketActivity::Created { frame_id, .. } => format!("created {}", frame_id.unwrap_or_default()),
                WebSocketActivity::WillSendHandshakeRequest { .. } => "request".into(),
                WebSocketActivity::HandshakeResponseReceived { status, .. } => format!("response {status}"),
                WebSocketActivity::FrameSent { opcode, payload } => format!("sent {opcode} {}", payload.len()),
                WebSocketActivity::FrameReceived { opcode, payload } => format!("received {opcode} {}", payload.len()),
                WebSocketActivity::FrameError { message } => format!("error {message}"),
                WebSocketActivity::Closed => "closed".into(),
            });
        }
    }
    assert_eq!(seen, ["created F", "request", "response 101", "sent 1 2", "received 1 2", "closed"]);
}

#[test]
fn dropping_the_handle_aborts_without_events() {
    let server = start_ws_server();
    let client = NetClient::new(Default::default()).unwrap();
    let (_, ws, rx) = open(&client, request(server.port, "/echo", &[]));
    assert!(matches!(next(&rx), WsEvent::Open { .. }));
    drop(ws);
    // The sink is dropped without a terminal event.
    assert!(matches!(rx.recv_timeout(Duration::from_secs(5)), Err(mpsc::RecvTimeoutError::Disconnected)));
}
