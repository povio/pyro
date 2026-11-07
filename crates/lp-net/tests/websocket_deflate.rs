//! permessage-deflate (RFC 7692) against a loopback server that answers the
//! offer with a preset `Sec-WebSocket-Extensions` per path and compresses
//! with flate2 like a real server.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use flate2::{Compress, Compression, Decompress, FlushCompress, FlushDecompress};
use lp_net::*;
use sha1::{Digest, Sha1};

const TAIL: [u8; 4] = [0, 0, 0xff, 0xff];

/// `/<preset>`: the response's extension header comes from [`preset`]. Every
/// handshake logs its `Sec-WebSocket-Extensions`; every data message logs
/// `rsv1=<bool> wire=<len>`; then the message is echoed, compressed when
/// deflate was negotiated. Text commands: `garbage` (an RSV1 message that is
/// not DEFLATE data), `bomb` (2 MiB of zeros, compressed), `rsv1-ping`,
/// `rsv1-continuation`, `plain` (an uncompressed text message).
struct Server {
    port: u16,
    log: Arc<Mutex<Vec<String>>>,
}

fn preset(path: &str) -> Option<&'static str> {
    Some(match path {
        "/takeover" => "permessage-deflate; client_max_window_bits=15",
        "/no-context" => "permessage-deflate; server_no_context_takeover; client_no_context_takeover",
        "/window8" => "permessage-deflate; client_max_window_bits=8; server_max_window_bits=9",
        "/bad-param" => "permessage-deflate; foo=1",
        "/bad-bits" => "permessage-deflate; client_max_window_bits=16",
        "/unknown" => "x-webkit-deflate-frame",
        "/duplicate" => "permessage-deflate, permessage-deflate",
        _ => return None,
    })
}

fn start() -> Server {
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
    Server { port, log }
}

/// (fin, rsv1, opcode, unmasked payload)
fn read_frame(reader: &mut impl Read) -> Option<(bool, bool, u8, Vec<u8>)> {
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
    let mut key = [0u8; 4];
    reader.read_exact(&mut key).ok()?;
    let mut payload = vec![0u8; len as usize];
    reader.read_exact(&mut payload).ok()?;
    for (i, b) in payload.iter_mut().enumerate() {
        *b ^= key[i % 4];
    }
    Some((head[0] & 0x80 != 0, head[0] & 0x40 != 0, head[0] & 0x0f, payload))
}

fn write_frame(stream: &mut TcpStream, first: u8, payload: &[u8]) {
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
    let _ = stream.write_all(&out);
}

fn deflate(c: &mut Compress, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 1024);
    c.compress_vec(data, &mut out, FlushCompress::Sync).unwrap();
    if out.ends_with(&TAIL) {
        out.truncate(out.len() - 4);
    }
    // RFC 7692 §7.2.3.6: an empty message is sent as one empty block.
    if out.is_empty() {
        out.push(0);
    }
    out
}

fn inflate(d: &mut Decompress, data: &[u8]) -> Option<Vec<u8>> {
    let mut input = data.to_vec();
    input.extend_from_slice(&TAIL);
    let mut out = Vec::with_capacity(1 << 20);
    d.decompress_vec(&input, &mut out, FlushDecompress::Sync).ok()?;
    Some(out)
}

fn serve(mut stream: TcpStream, log: &Mutex<Vec<String>>) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    if reader.read_line(&mut line).unwrap_or(0) == 0 {
        return;
    }
    let path = line.split_whitespace().nth(1).unwrap_or("").to_owned();
    let (mut key, mut offer) = (String::new(), String::new());
    loop {
        let mut h = String::new();
        reader.read_line(&mut h).unwrap();
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            match k.trim().to_ascii_lowercase().as_str() {
                "sec-websocket-key" => key = v.trim().to_owned(),
                "sec-websocket-extensions" => offer = v.trim().to_owned(),
                _ => {}
            }
        }
    }
    log.lock().unwrap().push(format!("offer={offer}"));
    let digest = Sha1::digest(format!("{key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11"));
    let accept = base64::engine::general_purpose::STANDARD.encode(digest);
    let ext = preset(&path);
    let mut response =
        format!("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n");
    if let Some(ext) = ext {
        response.push_str(&format!("Sec-WebSocket-Extensions: {ext}\r\n"));
    }
    response.push_str("\r\n");
    let _ = stream.write_all(response.as_bytes());
    let negotiated = ext.is_some();
    let no_context = path == "/no-context";
    let mut compressor = Compress::new(Compression::default(), false);
    let mut decompressor = Decompress::new(false);
    while let Some((fin, rsv1, opcode, payload)) = read_frame(&mut reader) {
        assert!(fin, "the client does not fragment");
        match opcode {
            0x8 => {
                let code = payload.get(..2).map_or(0, |c| u16::from_be_bytes([c[0], c[1]]));
                log.lock().unwrap().push(format!("close={code}"));
                write_frame(&mut stream, 0x88, &payload);
                return;
            }
            0x1 | 0x2 => {}
            _ => continue,
        }
        log.lock().unwrap().push(format!("rsv1={rsv1} wire={}", payload.len()));
        let message = if rsv1 {
            if no_context || path == "/window8" {
                decompressor = Decompress::new(false);
            }
            match inflate(&mut decompressor, &payload) {
                Some(m) => m,
                None => {
                    log.lock().unwrap().push("inflate failed".into());
                    return;
                }
            }
        } else {
            payload
        };
        match &message[..] {
            b"garbage" => write_frame(&mut stream, 0xC1, &[0xff, 0xff, 0xff, 0xff, 0x00]),
            b"bomb" => {
                let zeros = vec![b'0'; 2 << 20];
                write_frame(&mut stream, 0xC1, &deflate(&mut compressor, &zeros));
            }
            b"rsv1-ping" => write_frame(&mut stream, 0xC9, b""),
            b"rsv1-continuation" => {
                write_frame(&mut stream, 0x01, b"a");
                write_frame(&mut stream, 0xC0, b"b");
            }
            b"plain" => write_frame(&mut stream, 0x81, b"plain"),
            b"fragmented" => {
                // One compressed message in three frames: RSV1 on the first.
                let wire = deflate(&mut compressor, b"fragmented fragmented fragmented");
                let (a, rest) = wire.split_at(wire.len() / 3);
                let (b, c) = rest.split_at(rest.len() / 2);
                write_frame(&mut stream, 0x41, a);
                write_frame(&mut stream, 0x00, b);
                write_frame(&mut stream, 0x80, c);
            }
            _ if negotiated => {
                if no_context {
                    compressor.reset();
                }
                write_frame(&mut stream, 0xC0 | opcode, &deflate(&mut compressor, &message));
            }
            _ => write_frame(&mut stream, 0x80 | opcode, &message),
        }
    }
}

fn request(port: u16, path: &str) -> WebSocketRequest {
    WebSocketRequest {
        url: format!("ws://127.0.0.1:{port}{path}").parse().unwrap(),
        protocols: Vec::new(),
        origin: format!("http://127.0.0.1:{port}"),
        frame_id: Some("F".into()),
        defaults: Default::default(),
    }
}

fn open(client: &NetClient, port: u16, path: &str) -> (WebSocketHandle, mpsc::Receiver<WsEvent>) {
    let (tx, rx) = mpsc::channel();
    let (_, handle) = client.websocket(
        request(port, path),
        Box::new(move |event| {
            let _ = tx.send(event);
        }),
    );
    (handle, rx)
}

/// The next event that is not a `Sent` count.
fn next(rx: &mpsc::Receiver<WsEvent>) -> WsEvent {
    loop {
        match rx.recv_timeout(Duration::from_secs(5)).expect("websocket event") {
            WsEvent::Sent(_) => continue,
            event => return event,
        }
    }
}

fn extensions(rx: &mpsc::Receiver<WsEvent>) -> String {
    match next(rx) {
        WsEvent::Open { extensions, .. } => extensions,
        other => panic!("expected open, got {other:?}"),
    }
}

fn wire_sizes(server: &Server) -> Vec<String> {
    server.log.lock().unwrap().iter().filter(|l| l.starts_with("rsv1=") || l.starts_with("inflate")).cloned().collect()
}

#[test]
fn deflate_is_offered_and_messages_round_trip_with_context_takeover() {
    let server = start();
    let client = NetClient::new(Default::default()).unwrap();
    let (ws, rx) = open(&client, server.port, "/takeover");
    assert_eq!(extensions(&rx), "permessage-deflate; client_max_window_bits=15");
    assert_eq!(server.log.lock().unwrap()[0], "offer=permessage-deflate; client_max_window_bits");
    let text = "compress me, compress me, compress me, compress me ".repeat(8);
    for _ in 0..3 {
        ws.send_text(text.clone());
        assert!(matches!(next(&rx), WsEvent::Text(t) if t == text));
    }
    let binary: Vec<u8> = (0..70_000u32).map(|i| (i % 251) as u8).collect();
    ws.send_binary(binary.clone().into());
    assert!(matches!(next(&rx), WsEvent::Binary(b) if b[..] == binary[..]));
    ws.send_text(String::new());
    assert!(matches!(next(&rx), WsEvent::Text(t) if t.is_empty()));
    // Uncompressed and fragmented compressed messages mix with the rest.
    ws.send_text("plain".into());
    assert!(matches!(next(&rx), WsEvent::Text(t) if t == "plain"));
    ws.send_text("fragmented".into());
    assert!(matches!(next(&rx), WsEvent::Text(t) if t == "fragmented fragmented fragmented"));
    ws.send_text(text.clone());
    assert!(matches!(next(&rx), WsEvent::Text(t) if t == text));
    ws.close(Some(1000), String::new());
    assert!(matches!(next(&rx), WsEvent::Closed { clean: true, code: 1000, failed: false, .. }));
    let log = wire_sizes(&server);
    assert!(log.iter().all(|l| l.starts_with("rsv1=true")), "{log:?}");
    let wire: Vec<usize> = log.iter().map(|l| l.rsplit('=').next().unwrap().parse().unwrap()).collect();
    assert!(wire[0] < text.len() / 4, "{wire:?}");
    // Context takeover: the repeat is a back-reference.
    assert!(wire[1] < wire[0] && wire[2] < wire[0], "{wire:?}");
}

#[test]
fn no_context_takeover_and_small_client_windows_are_honoured() {
    let server = start();
    let client = NetClient::new(Default::default()).unwrap();
    let (ws, rx) = open(&client, server.port, "/no-context");
    assert_eq!(extensions(&rx), "permessage-deflate; server_no_context_takeover; client_no_context_takeover");
    let text = "again and again and again ".repeat(4);
    for _ in 0..3 {
        ws.send_text(text.clone());
        assert!(matches!(next(&rx), WsEvent::Text(t) if t == text));
    }
    drop(ws);
    let log = wire_sizes(&server);
    // The server inflates each message with a fresh window.
    assert_eq!(log.len(), 3, "{log:?}");
    assert!(log.iter().all(|l| l == &log[0] && l.starts_with("rsv1=true")), "{log:?}");

    let server = start();
    let (ws, rx) = open(&client, server.port, "/window8");
    extensions(&rx);
    let small = "xyzxyzxyz".repeat(20);
    let large = "abc".repeat(200);
    ws.send_text(small.clone());
    assert!(matches!(next(&rx), WsEvent::Text(t) if t == small));
    ws.send_text(large.clone());
    assert!(matches!(next(&rx), WsEvent::Text(t) if t == large));
    drop(ws);
    let log = wire_sizes(&server);
    assert!(log[0].starts_with("rsv1=true"), "{log:?}");
    assert_eq!(log[1], "rsv1=false wire=600");
}

#[test]
fn bad_negotiation_fails_the_connection() {
    let server = start();
    let client = NetClient::new(Default::default()).unwrap();
    let events = client.subscribe();
    for path in ["/bad-param", "/bad-bits", "/unknown", "/duplicate"] {
        let (_ws, rx) = open(&client, server.port, path);
        assert!(
            matches!(next(&rx), WsEvent::Closed { clean: false, code: 1006, failed: true, .. }),
            "{path} must fail the connection"
        );
    }
    let mut errors = Vec::new();
    while let Ok(event) = events.try_recv() {
        if let NetEvent::WebSocket { activity, .. } = event {
            if let WebSocketActivity::FrameError { message } = *activity {
                errors.push(message);
            }
        }
    }
    assert_eq!(errors.len(), 4, "{errors:?}");
    assert!(errors[0].contains("Error in permessage-deflate: Received an unexpected"), "{errors:?}");
    assert!(errors[2].contains("unsupported extension 'x-webkit-deflate-frame'"), "{errors:?}");
    // A server that refuses deflate gets plain frames.
    let (ws, rx) = open(&client, server.port, "/refuse");
    assert_eq!(extensions(&rx), "");
    ws.send_text("hello".into());
    assert!(matches!(next(&rx), WsEvent::Text(t) if t == "hello"));
    assert_eq!(wire_sizes(&server), ["rsv1=false wire=5"]);
    // ...and RSV1 from it is a protocol error.
    ws.send_text("garbage".into());
    assert!(matches!(next(&rx), WsEvent::Closed { clean: false, code: 1006, failed: true, .. }));
}

#[test]
fn bad_compressed_data_and_bombs_fail_the_connection() {
    let config = NetConfig { max_body: 1 << 20, ..Default::default() };
    let client = NetClient::new(config).unwrap();
    for (command, code) in [("garbage", 1002), ("bomb", 1009), ("rsv1-ping", 1002), ("rsv1-continuation", 1002)] {
        let server = start();
        let (ws, rx) = open(&client, server.port, "/takeover");
        extensions(&rx);
        ws.send_text(command.into());
        assert!(
            matches!(next(&rx), WsEvent::Closed { clean: false, code: 1006, failed: true, .. }),
            "{command} must fail the connection"
        );
        // The failure's Close frame carries the code.
        for _ in 0..100 {
            if server.log.lock().unwrap().iter().any(|l| l.starts_with("close=")) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let closes: Vec<String> =
            server.log.lock().unwrap().iter().filter(|l| l.starts_with("close=")).cloned().collect();
        assert_eq!(closes, [format!("close={code}")], "{command}");
    }
}

#[test]
fn extra_headers_do_not_replace_the_offer() {
    let server = start();
    let client = NetClient::new(Default::default()).unwrap();
    let mut req = request(server.port, "/takeover");
    req.defaults.extra_headers.insert("sec-websocket-extensions", http::HeaderValue::from_static("x-other"));
    let (tx, rx) = mpsc::channel();
    let (_, _ws) = client.websocket(
        req,
        Box::new(move |event| {
            let _ = tx.send(event);
        }),
    );
    assert_eq!(extensions(&rx), "permessage-deflate; client_max_window_bits=15");
    assert_eq!(server.log.lock().unwrap()[0], "offer=permessage-deflate; client_max_window_bits");
}
