//! Real TCP/WebSocket round trips through the tokio server, with the session
//! state running on its own thread as the browser thread will.

use std::net::SocketAddr;
use std::thread;

use lp_cdp::msg::{self, CdpResult, Command};
use lp_cdp::outbox::{Outbox, DEFAULT_OUTBOUND_BUDGET};
use lp_cdp::server::{Inbound, Server, ServerConfig, SessionChannels, SessionFactory};
use lp_cdp::target::{Browser, ContextKey, Host, PageCommand, PageKey, Sink};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;

struct NoPages;

impl Host for NoPages {
    fn create_context(&mut self) -> CdpResult<ContextKey> {
        Ok(1)
    }
    fn dispose_context(&mut self, _: ContextKey) {}
    fn create_page(&mut self, _: ContextKey) -> CdpResult<(PageKey, String)> {
        Err(msg::CdpError::server("no engine in this test"))
    }
    fn navigate(&mut self, _: PageKey, _: &str) -> CdpResult<()> {
        unreachable!()
    }
    fn close_page(&mut self, _: PageKey) {}
    fn page_url(&self, _: PageKey) -> String {
        unreachable!()
    }
    fn page_title(&self, _: PageKey) -> String {
        unreachable!()
    }
    fn session_attached(&mut self, _: PageKey, _: &str) {}
    fn session_detached(&mut self, _: PageKey, _: &str) {}
    fn page_command(&mut self, _: PageCommand<'_, '_>, _: &mut dyn Sink) {
        unreachable!()
    }
    fn browser_command(&mut self, command: &Command<'_>, _: Option<ContextKey>, out: &mut dyn Sink) {
        out.send(msg::error_frame(Some(command.id), &msg::CdpError::unsupported(&command.method), None));
    }
}

struct ThreadFactory;

impl SessionFactory for ThreadFactory {
    fn start(&self, _peer: SocketAddr) -> std::io::Result<SessionChannels> {
        let (in_tx, mut in_rx) = mpsc::channel::<Inbound>(64);
        let (out_tx, out_rx) = Outbox::channel(DEFAULT_OUTBOUND_BUDGET);
        thread::spawn(move || {
            let mut browser = Browser::new(NoPages);
            let mut sink = out_tx;
            while let Some(message) = in_rx.blocking_recv() {
                match message {
                    Inbound::Text(text) => browser.handle_text(&text, &mut sink),
                    Inbound::Closed => break,
                }
            }
            browser.shutdown();
        });
        Ok(SessionChannels { inbound: in_tx, outbound: out_rx })
    }
}

async fn start() -> SocketAddr {
    start_with(ServerConfig::default(), ThreadFactory).await
}

async fn start_with<F: SessionFactory>(config: ServerConfig, factory: F) -> SocketAddr {
    let server = Server::bind("127.0.0.1:0".parse().unwrap(), config, factory).await.unwrap();
    let addr = server.local_addr().unwrap();
    tokio::spawn(server.run());
    addr
}

/// A session that ends immediately (e.g. engine start failure).
struct EndsImmediately;

impl SessionFactory for EndsImmediately {
    fn start(&self, _peer: SocketAddr) -> std::io::Result<SessionChannels> {
        let (in_tx, _in_rx) = mpsc::channel::<Inbound>(1);
        let (_out_tx, out_rx) = Outbox::channel(DEFAULT_OUTBOUND_BUDGET);
        Ok(SessionChannels { inbound: in_tx, outbound: out_rx })
    }
}

/// A session that never reads its inbound messages and never answers.
struct NeverReads;

impl SessionFactory for NeverReads {
    fn start(&self, _peer: SocketAddr) -> std::io::Result<SessionChannels> {
        let (in_tx, in_rx) = mpsc::channel::<Inbound>(1);
        let (out_tx, out_rx) = Outbox::channel(DEFAULT_OUTBOUND_BUDGET);
        // Keep both ends alive for the test's duration.
        std::mem::forget((in_rx, out_tx));
        Ok(SessionChannels { inbound: in_tx, outbound: out_rx })
    }
}

fn quick(max_connections: usize) -> ServerConfig {
    ServerConfig {
        max_connections,
        head_timeout: Duration::from_millis(300),
        frame_timeout: Duration::from_millis(300),
        write_timeout: Duration::from_millis(300),
        close_timeout: Duration::from_millis(300),
        ..ServerConfig::default()
    }
}

/// Reads until EOF or error, returning whether the server ended the connection in time.
async fn closed_within(s: &mut TcpStream, limit: Duration) -> bool {
    let mut buf = vec![0u8; 64 * 1024];
    let deadline = tokio::time::Instant::now() + limit;
    loop {
        match tokio::time::timeout_at(deadline, s.read(&mut buf)).await {
            Ok(Ok(0)) | Ok(Err(_)) => return true,
            Ok(Ok(_)) => continue,
            Err(_) => return false,
        }
    }
}

async fn http_get(addr: SocketAddr, path: &str, host: &str) -> String {
    let mut s = TcpStream::connect(addr).await.unwrap();
    s.write_all(format!("GET {path} HTTP/1.1\r\nHost: {host}\r\n\r\n").as_bytes()).await.unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).await.unwrap();
    out
}

fn masked(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let mask = [1, 2, 3, 4];
    let mut f = vec![0x80 | opcode];
    if payload.len() < 126 {
        f.push(0x80 | payload.len() as u8);
    } else {
        f.push(0x80 | 126);
        f.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    }
    f.extend_from_slice(&mask);
    f.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    f
}

async fn read_frame(s: &mut TcpStream) -> (u8, Vec<u8>) {
    let mut h = [0u8; 2];
    s.read_exact(&mut h).await.unwrap();
    let len = match h[1] & 0x7f {
        126 => {
            let mut b = [0u8; 2];
            s.read_exact(&mut b).await.unwrap();
            u16::from_be_bytes(b) as usize
        }
        127 => {
            let mut b = [0u8; 8];
            s.read_exact(&mut b).await.unwrap();
            u64::from_be_bytes(b) as usize
        }
        n => n as usize,
    };
    let mut p = vec![0u8; len];
    s.read_exact(&mut p).await.unwrap();
    (h[0] & 0x0f, p)
}

async fn upgrade(addr: SocketAddr) -> TcpStream {
    let mut s = TcpStream::connect(addr).await.unwrap();
    s.write_all(
        b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
          Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n",
    )
    .await
    .unwrap();
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        let mut b = [0u8; 1];
        s.read_exact(&mut b).await.unwrap();
        head.push(b[0]);
    }
    assert!(String::from_utf8(head).unwrap().starts_with("HTTP/1.1 101"));
    s
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn discovery_endpoint_advertises_bound_address() {
    let addr = start().await;
    let response = http_get(addr, "/json/version", "127.0.0.1").await;
    assert!(response.starts_with("HTTP/1.1 200"));
    let body: Value = serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(body["webSocketDebuggerUrl"], format!("ws://{addr}/"));
    assert!(http_get(addr, "/json/version", "attacker.example").await.starts_with("HTTP/1.1 403"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn websocket_commands_are_answered_in_order() {
    let addr = start().await;
    let mut s = upgrade(addr).await;
    for id in 1..=50u64 {
        let text = json!({"id": id, "method": "Target.getBrowserContexts"}).to_string();
        s.write_all(&masked(1, text.as_bytes())).await.unwrap();
    }
    s.write_all(&masked(9, b"hi")).await.unwrap();
    let mut ids = Vec::new();
    let mut pong = false;
    while ids.len() < 50 || !pong {
        match read_frame(&mut s).await {
            (1, payload) => ids.push(serde_json::from_slice::<Value>(&payload).unwrap()["id"].as_u64().unwrap()),
            (0xA, payload) => {
                assert_eq!(payload, b"hi");
                pong = true;
            }
            other => panic!("unexpected frame {other:?}"),
        }
    }
    assert_eq!(ids, (1..=50).collect::<Vec<_>>());

    s.write_all(&masked(8, &1000u16.to_be_bytes())).await.unwrap();
    let (opcode, payload) = read_frame(&mut s).await;
    assert_eq!((opcode, &payload[..]), (8, &1000u16.to_be_bytes()[..]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oversized_message_closes_with_1009() {
    let addr = start().await;
    let mut s = upgrade(addr).await;
    let mut frame = vec![0x81, 0x80 | 127];
    frame.extend_from_slice(&(2u64 * 1024 * 1024).to_be_bytes());
    s.write_all(&frame).await.unwrap();
    let (opcode, payload) = read_frame(&mut s).await;
    assert_eq!(opcode, 8);
    assert_eq!(u16::from_be_bytes([payload[0], payload[1]]), 1009);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_json_gets_parse_error_without_closing() {
    let addr = start().await;
    let mut s = upgrade(addr).await;
    s.write_all(&masked(1, b"{nope")).await.unwrap();
    let (_, payload) = read_frame(&mut s).await;
    let v: Value = serde_json::from_slice(&payload).unwrap();
    assert_eq!(v["error"]["code"], -32700);
    s.write_all(&masked(1, br#"{"id":2,"method":"Browser.getVersion"}"#)).await.unwrap();
    let (_, payload) = read_frame(&mut s).await;
    assert_eq!(serde_json::from_slice::<Value>(&payload).unwrap()["id"], 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn finished_session_releases_the_connection_slot() {
    let addr = start_with(quick(1), EndsImmediately).await;
    // The client never closes its side; the server must still free the slot.
    let mut held = upgrade(addr).await;
    let (opcode, _) = read_frame(&mut held).await;
    assert_eq!(opcode, 8, "session end is reported with a close frame");
    let response = tokio::time::timeout(Duration::from_secs(5), http_get(addr, "/json/version", "127.0.0.1")).await;
    assert!(response.expect("slot was released").starts_with("HTTP/1.1 200"));
    drop(held);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stalled_request_heads_are_dropped() {
    let addr = start_with(quick(1), ThreadFactory).await;
    let mut stalled = TcpStream::connect(addr).await.unwrap();
    stalled.write_all(b"GET /json/version HTTP/1.1\r\nHost: 127.0.0.1\r\n").await.unwrap();
    assert!(closed_within(&mut stalled, Duration::from_secs(5)).await, "head deadline closes the connection");
    let response = tokio::time::timeout(Duration::from_secs(5), http_get(addr, "/json/version", "127.0.0.1")).await;
    assert!(response.expect("slot was released").starts_with("HTTP/1.1 200"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn complete_heads_over_the_limit_are_rejected() {
    let config = ServerConfig { max_http_message_size: 128, ..quick(4) };
    let addr = start_with(config, ThreadFactory).await;
    let padding = "x".repeat(200);
    let response = http_get(addr, &format!("/json/version?{padding}"), "127.0.0.1").await;
    assert!(response.starts_with("HTTP/1.1 413"), "{response}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn incomplete_frames_time_out() {
    let addr = start_with(quick(1), ThreadFactory).await;
    let mut s = upgrade(addr).await;
    s.write_all(&[0x81, 0x85, 1, 2]).await.unwrap(); // header promises 5 bytes, mask incomplete
    let (opcode, payload) = read_frame(&mut s).await;
    assert_eq!(opcode, 8);
    assert_eq!(u16::from_be_bytes([payload[0], payload[1]]), 1008);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ping_flood_without_reading_is_disconnected() {
    let addr = start_with(quick(1), NeverReads).await;
    let mut s = upgrade(addr).await;
    let ping = masked(9, b"12345678");
    let mut burst = Vec::with_capacity(ping.len() * 4096);
    for _ in 0..4096 {
        burst.extend_from_slice(&ping);
    }
    // Keep pinging without reading pongs until the server gives up.
    let flood = async {
        for _ in 0..256 {
            if s.write_all(&burst).await.is_err() {
                return true;
            }
        }
        false
    };
    let disconnected = tokio::time::timeout(Duration::from_secs(20), flood).await.unwrap_or(false);
    assert!(disconnected, "server must drop a client that floods pings without reading");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unresponsive_session_still_releases_slot_on_client_close() {
    let addr = start_with(quick(1), NeverReads).await;
    let mut s = upgrade(addr).await;
    s.write_all(&masked(1, br#"{"id":1,"method":"Browser.getVersion"}"#)).await.unwrap();
    s.write_all(&masked(1, br#"{"id":2,"method":"Browser.getVersion"}"#)).await.unwrap();
    drop(s);
    let response = tokio::time::timeout(Duration::from_secs(5), http_get(addr, "/json/version", "127.0.0.1")).await;
    assert!(response.expect("slot was released").starts_with("HTTP/1.1 200"));
}
