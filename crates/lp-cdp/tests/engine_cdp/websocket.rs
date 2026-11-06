//! `WebSocket` in pages against a local RFC 6455 server: the page API and
//! its events, CDP `Network.webSocket*` in Chrome's shapes, and `LP.run`
//! settling while a socket stays open.
use super::*;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};

/// A frame from the client (masked) or `None` at EOF.
fn read_frame(stream: &mut TcpStream) -> Option<(u8, Vec<u8>)> {
    let mut head = [0u8; 2];
    stream.read_exact(&mut head).ok()?;
    let opcode = head[0] & 0x0f;
    assert!(head[1] & 0x80 != 0, "client frames are masked");
    let len = match head[1] & 0x7f {
        126 => {
            let mut b = [0u8; 2];
            stream.read_exact(&mut b).ok()?;
            u16::from_be_bytes(b) as usize
        }
        127 => {
            let mut b = [0u8; 8];
            stream.read_exact(&mut b).ok()?;
            u64::from_be_bytes(b) as usize
        }
        n => n as usize,
    };
    let mut mask = [0u8; 4];
    stream.read_exact(&mut mask).ok()?;
    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload).ok()?;
    for (i, b) in payload.iter_mut().enumerate() {
        *b ^= mask[i % 4];
    }
    Some((opcode, payload))
}

fn write_frame(stream: &mut TcpStream, opcode: u8, payload: &[u8]) {
    let mut out = vec![0x80 | opcode];
    if payload.len() < 126 {
        out.push(payload.len() as u8);
    } else {
        out.push(126);
        out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    }
    out.extend_from_slice(payload);
    let _ = stream.write_all(&out);
}

/// A server for `connections` sockets: answers the handshake (choosing the
/// first offered subprotocol), greets with "hi", echoes every message, and
/// on "bye" closes with 4001 "server says bye". The handshake requests are
/// sent back through the channel.
fn serve_ws(connections: usize) -> (SocketAddr, std::sync::mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for _ in 0..connections {
            let Ok((mut stream, _)) = listener.accept() else { return };
            let tx = tx.clone();
            std::thread::spawn(move || {
                stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
                let mut request = Vec::new();
                let mut byte = [0u8; 1];
                while !request.ends_with(b"\r\n\r\n") {
                    if stream.read(&mut byte).unwrap_or(0) == 0 {
                        return;
                    }
                    request.push(byte[0]);
                }
                let request = String::from_utf8(request).unwrap();
                let header = |name: &str| {
                    request.lines().find_map(|l| {
                        let (k, v) = l.split_once(':')?;
                        k.trim().eq_ignore_ascii_case(name).then(|| v.trim().to_owned())
                    })
                };
                let key = header("sec-websocket-key").expect("key");
                let mut response = format!(
                    "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n",
                    lp_net::websocket_accept_key(&key)
                );
                if let Some(p) = header("sec-websocket-protocol") {
                    response += &format!("Sec-WebSocket-Protocol: {}\r\n", p.split(',').next().unwrap().trim());
                }
                response += "\r\n";
                let _ = tx.send(request);
                stream.write_all(response.as_bytes()).unwrap();
                write_frame(&mut stream, 1, b"hi");
                while let Some((opcode, payload)) = read_frame(&mut stream) {
                    match opcode {
                        8 => {
                            write_frame(&mut stream, 8, &payload[..payload.len().min(2)]);
                            return;
                        }
                        1 if payload == b"bye" => {
                            let mut body = 4001u16.to_be_bytes().to_vec();
                            body.extend_from_slice(b"server says bye");
                            write_frame(&mut stream, 8, &body);
                            let _ = read_frame(&mut stream);
                            return;
                        }
                        1 | 2 => write_frame(&mut stream, opcode, &payload),
                        _ => {}
                    }
                }
            });
        }
    });
    (address, rx)
}

/// Opens a socket, sends text and binary, closes, and resolves with what the
/// page saw.
const ECHO_SCRIPT: &str = r#"(url) => new Promise((resolve) => {
  const log = [];
  const ws = new WebSocket(url, ['chat', 'other']);
  ws.binaryType = 'arraybuffer';
  log.push('state ' + ws.readyState);
  ws.onopen = () => {
    log.push('open ' + ws.readyState + ' ' + ws.protocol);
    ws.send('hello');
    ws.send(new Uint8Array([1, 2, 3]));
  };
  ws.addEventListener('message', (e) => {
    const data = typeof e.data === 'string' ? e.data : 'bytes ' + Array.from(new Uint8Array(e.data)).join(',');
    log.push('message ' + data + ' ' + (e instanceof MessageEvent));
    if (data === 'bytes 1,2,3') ws.close(1000, 'done');
  });
  ws.onerror = () => log.push('error');
  ws.onclose = (e) => {
    log.push('close ' + e.code + ' ' + e.reason + ' ' + e.wasClean + ' ' + ws.readyState + ' ' + (e instanceof CloseEvent));
    resolve(log);
  };
})"#;

#[test]
fn websocket_sends_receives_and_closes() {
    let (address, handshakes) = serve_ws(1);
    let mut h = Harness::new();
    h.page("Network.enable", json!({}));
    let url = format!("ws://{address}/echo?x=1");
    let session = h.session.clone();
    let id = h.send(
        Some(&session),
        "LP.run",
        json!({"steps": [{"eval": {"expression": ECHO_SCRIPT, "call": true, "arg": url, "as": "log"}}]}),
    );
    let mut messages = h.until_response(id);
    messages.extend(h.run_for(100));
    let reply = messages.iter().find(|m| m["id"] == id).unwrap();
    assert_eq!(
        reply["result"]["reads"]["log"],
        json!([
            "state 0",
            "open 1 chat",
            "message hi true",
            "message hello true",
            "message bytes 1,2,3 true",
            "close 1000  true 3 true",
        ]),
        "{reply}"
    );
    let handshake = handshakes.recv_timeout(Duration::from_secs(1)).unwrap().to_ascii_lowercase();
    assert!(handshake.starts_with("get /echo?x=1 http/1.1\r\n"), "{handshake}");
    assert!(handshake.contains("\r\nsec-websocket-protocol: chat, other\r\n"), "{handshake}");
    assert!(handshake.contains("\r\norigin: null\r\n"), "{handshake}");
    assert!(handshake.contains("\r\nupgrade: websocket\r\n"), "{handshake}");

    let ws: Vec<&Value> =
        messages.iter().filter(|m| m["method"].as_str().is_some_and(|m| m.starts_with("Network.webSocket"))).collect();
    let names: Vec<&str> = ws.iter().map(|m| m["method"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        [
            "Network.webSocketCreated",
            "Network.webSocketWillSendHandshakeRequest",
            "Network.webSocketHandshakeResponseReceived",
            "Network.webSocketFrameReceived",
            "Network.webSocketFrameSent",
            "Network.webSocketFrameSent",
            "Network.webSocketFrameReceived",
            "Network.webSocketFrameReceived",
            "Network.webSocketClosed",
        ],
        "{ws:#?}"
    );
    let request_id = &ws[0]["params"]["requestId"];
    assert!(ws.iter().all(|m| &m["params"]["requestId"] == request_id), "{ws:#?}");
    assert_eq!(ws[0]["params"]["url"], url);
    assert_eq!(ws[1]["params"]["request"]["headers"]["sec-websocket-version"], "13");
    assert!(ws[1]["params"]["wallTime"].is_number());
    assert_eq!(ws[2]["params"]["response"]["status"], 101);
    assert_eq!(ws[2]["params"]["response"]["statusText"], "Switching Protocols");
    assert_eq!(ws[2]["params"]["response"]["headers"]["sec-websocket-protocol"], "chat");
    assert_eq!(ws[3]["params"]["response"], json!({"opcode": 1, "mask": false, "payloadData": "hi"}));
    assert_eq!(ws[4]["params"]["response"], json!({"opcode": 1, "mask": true, "payloadData": "hello"}));
    assert_eq!(ws[5]["params"]["response"], json!({"opcode": 2, "mask": true, "payloadData": "AQID"}));
    assert_eq!(ws[6]["params"]["response"]["payloadData"], "hello");
    assert_eq!(ws[7]["params"]["response"], json!({"opcode": 2, "mask": false, "payloadData": "AQID"}));
    assert!(ws.iter().all(|m| m["params"]["timestamp"].is_number() || m["method"] == "Network.webSocketCreated"));
}

/// An open socket is an established stream: `LP.run` settles while it stays
/// open, and the page keeps receiving on it afterwards.
#[test]
fn an_open_websocket_does_not_hold_settling() {
    let (address, _handshakes) = serve_ws(1);
    let mut h = Harness::new();
    let open = format!(
        "new Promise((resolve) => {{ window.got = []; window.ws = new WebSocket('ws://{address}/'); \
         ws.onmessage = (e) => got.push(e.data); ws.onopen = () => resolve(ws.readyState); }})"
    );
    let started = Instant::now();
    let r = h.page(
        "LP.run",
        json!({"steps": [
            {"eval": {"expression": open, "as": "state"}},
            {"idle": {"timeout": 2000}},
            // A reply is not owed: settling does not wait for one.
            {"eval": {"expression": "ws.send('ping'); got.join(',')", "as": "sent"}},
            {"eval": {"expression": "new Promise((resolve) => { const t = setInterval(() => { if (got.length > 1) { clearInterval(t); resolve(got.join(',')); } }, 5); })", "as": "got"}},
        ]}),
    );
    assert!(started.elapsed() < Duration::from_secs(2), "settling waited on the socket: {r}");
    assert_eq!(r["result"]["reads"]["state"], 1, "{r}");
    assert_eq!(r["result"]["reads"]["got"], "hi,ping", "{r}");
    assert_eq!(r["result"]["reads"]["sent"], "hi", "{r}");
    let idle = &r["result"]["results"][1];
    assert!(idle.get("settled").is_none(), "{r}");

    // The server closing is a task on the page, still delivered.
    let r = h.page(
        "LP.run",
        json!({"steps": [{"eval": {"expression":
            "new Promise((resolve) => { ws.onclose = (e) => resolve(e.code + ' ' + e.reason + ' ' + e.wasClean); ws.send('bye'); })",
            "as": "closed"}}]}),
    );
    assert_eq!(r["result"]["reads"]["closed"], "4001 server says bye true", "{r}");
}

#[test]
fn websocket_failures_fire_error_then_close() {
    // A port nothing listens on.
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let mut h = Harness::new();
    h.page("Network.enable", json!({}));
    let script = format!(
        "new Promise((resolve) => {{ const log = []; const ws = new WebSocket('ws://127.0.0.1:{port}/'); \
         ws.onerror = (e) => log.push('error ' + e.type); \
         ws.onclose = (e) => {{ log.push('close ' + e.code + ' ' + e.wasClean); resolve(log); }}; }})"
    );
    let session = h.session.clone();
    let id = h.send(Some(&session), "LP.run", json!({"steps": [{"eval": {"expression": script, "as": "log"}}]}));
    let messages = h.until_response(id);
    let reply = messages.iter().find(|m| m["id"] == id).unwrap();
    assert_eq!(reply["result"]["reads"]["log"], json!(["error error", "close 1006 false"]), "{reply}");
    let error = messages.iter().find(|m| m["method"] == "Network.webSocketFrameError").expect("frame error");
    assert_eq!(
        error["params"]["errorMessage"], "Error in connection establishment: net::ERR_CONNECTION_REFUSED",
        "{error}"
    );
    assert!(messages.iter().any(|m| m["method"] == "Network.webSocketClosed"));

    // Constructor and close() argument checks throw as in Chrome.
    let r = h.page(
        "LP.run",
        json!({"steps": [{"eval": {"expression": r#"(() => {
            const out = [];
            for (const f of [
              () => new WebSocket('ftp://x/'),
              () => new WebSocket('ws://x/#frag'),
              () => new WebSocket('ws://x/', ['a', 'a']),
              () => new WebSocket('ws://x/', 'bad proto'),
              () => new WebSocket('ws://127.0.0.1:1/').close(1001),
              () => new WebSocket('ws://127.0.0.1:1/').send('x'),
            ]) { try { f(); out.push('ok'); } catch (e) { out.push(e.name); } }
            out.push(typeof WebSocket.OPEN + WebSocket.CLOSED, new WebSocket('http://127.0.0.1:1/p').url);
            return out;
        })()"#, "as": "errors"}}]}),
    );
    assert_eq!(
        r["result"]["reads"]["errors"],
        json!([
            "SyntaxError",
            "SyntaxError",
            "SyntaxError",
            "SyntaxError",
            "InvalidAccessError",
            "InvalidStateError",
            "number3",
            "ws://127.0.0.1:1/p",
        ]),
        "{r}"
    );
}
