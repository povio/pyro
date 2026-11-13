//! `Network.webSocket*` for a page's connection, as Playwright's
//! `page.on('websocket')` consumes them.
use super::*;
use std::io::{BufRead, BufReader, Read, Write};

/// Serves a blank page over HTTP and echoes WebSocket messages of up to 125
/// bytes; a Close is answered with its payload.
fn start_server() -> std::net::SocketAddr {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut key = None;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        return;
                    }
                    if line == "\r\n" {
                        break;
                    }
                    if let Some((name, value)) = line.split_once(':') {
                        if name.eq_ignore_ascii_case("sec-websocket-key") {
                            key = Some(value.trim().to_owned());
                        }
                    }
                }
                let Some(key) = key else {
                    let _ = stream.write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 13\r\nConnection: close\r\n\r\n<p>blank</p>\n",
                    );
                    return;
                };
                let accept = pyro_net::websocket_accept_key(&key);
                let _ = write!(
                    stream,
                    "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
                );
                loop {
                    let mut head = [0u8; 2];
                    if reader.read_exact(&mut head).is_err() {
                        return;
                    }
                    let len = (head[1] & 0x7f) as usize;
                    let mut mask = [0u8; 4];
                    let mut payload = vec![0u8; len];
                    if reader.read_exact(&mut mask).is_err() || reader.read_exact(&mut payload).is_err() {
                        return;
                    }
                    for (i, b) in payload.iter_mut().enumerate() {
                        *b ^= mask[i % 4];
                    }
                    let mut frame = vec![head[0], len as u8];
                    frame.extend_from_slice(&payload);
                    let _ = stream.write_all(&frame);
                    if head[0] & 0x0f == 0x8 {
                        let _ = stream.shutdown(std::net::Shutdown::Both);
                        return;
                    }
                }
            });
        }
    });
    address
}

#[test]
fn page_websocket_reports_its_handshake_frames_and_close() {
    let address = start_server();
    let mut h = Harness::new();
    h.page("Network.enable", json!({}));
    let mut events = h.page_with_events("Page.navigate", json!({"url":format!("http://{address}/")}));
    wait_for_network_finish(&mut h, &mut events);
    let expression = format!(
        "const ws = new WebSocket('ws://{address}/echo'); ws.onopen = () => ws.send('hi'); ws.onmessage = () => ws.close(1000);"
    );
    let mut events = h.page_with_events("Runtime.evaluate", json!({ "expression": expression }));
    if !events.iter().any(|e| e["method"] == "Network.webSocketClosed") {
        events.extend(until_event(&mut h, "Network.webSocketClosed"));
    }
    let socket: Vec<&Value> =
        events.iter().filter(|e| e["method"].as_str().is_some_and(|m| m.starts_with("Network.webSocket"))).collect();
    let methods: Vec<&str> = socket.iter().map(|e| e["method"].as_str().unwrap()).collect();
    assert_eq!(
        methods,
        [
            "Network.webSocketCreated",
            "Network.webSocketWillSendHandshakeRequest",
            "Network.webSocketHandshakeResponseReceived",
            "Network.webSocketFrameSent",
            "Network.webSocketFrameReceived",
            "Network.webSocketClosed",
        ],
        "{events:#?}"
    );
    let id = &socket[0]["params"]["requestId"];
    assert!(socket.iter().all(|e| &e["params"]["requestId"] == id));
    assert_eq!(socket[0]["params"]["url"], format!("ws://{address}/echo"));
    assert_eq!(socket[1]["params"]["request"]["headers"]["upgrade"], "websocket");
    assert_eq!(socket[2]["params"]["response"]["status"], 101);
    assert_eq!(socket[3]["params"]["response"], json!({"opcode": 1, "mask": true, "payloadData": "hi"}));
    assert_eq!(socket[4]["params"]["response"]["payloadData"], "hi");
}
