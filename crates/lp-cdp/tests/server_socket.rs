//! Real TCP/WebSocket round trips through the tokio server, with the session
//! state running on its own thread as the browser thread will.

use std::net::SocketAddr;
use std::thread;

use lp_cdp::msg::{self, CdpResult, Command};
use lp_cdp::server::{Inbound, Outbound, Server, ServerConfig, SessionChannels, SessionFactory};
use lp_cdp::target::{Browser, ContextKey, Host, PageCommand, PageKey, Sink};
use lp_cdp::ws::OutFrame;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;

struct NoPages;

impl Host for NoPages {
    fn create_context(&mut self) -> CdpResult<ContextKey> {
        Ok(1)
    }
    fn dispose_context(&mut self, _: ContextKey) {}
    fn create_page(&mut self, _: ContextKey, _: &str) -> CdpResult<PageKey> {
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

struct ChannelSink(mpsc::UnboundedSender<Outbound>);

impl Sink for ChannelSink {
    fn send(&mut self, frame: OutFrame) {
        let _ = self.0.send(frame.finish_text());
    }
}

struct ThreadFactory;

impl SessionFactory for ThreadFactory {
    fn start(&self, _peer: SocketAddr) -> std::io::Result<SessionChannels> {
        let (in_tx, mut in_rx) = mpsc::channel::<Inbound>(64);
        let (out_tx, out_rx) = mpsc::unbounded_channel();
        thread::spawn(move || {
            let mut browser = Browser::new(NoPages);
            let mut sink = ChannelSink(out_tx);
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
    let server = Server::bind("127.0.0.1:0".parse().unwrap(), ServerConfig::default(), ThreadFactory).await.unwrap();
    let addr = server.local_addr().unwrap();
    tokio::spawn(server.run());
    addr
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
