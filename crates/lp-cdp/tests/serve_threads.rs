//! The real per-connection browser threads (JavaScriptCore) behind the server:
//! disconnect interrupts running JavaScript and the connection slot is held
//! until the browser thread has finished.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use lp_cdp::serve::ThreadPerConnection;
use lp_cdp::server::{Server, ServerConfig};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

fn masked(payload: &[u8]) -> Vec<u8> {
    let mask = [9, 8, 7, 6];
    let mut f = vec![0x81];
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

async fn read_text(s: &mut TcpStream) -> Option<Value> {
    let mut h = [0u8; 2];
    s.read_exact(&mut h).await.ok()?;
    let len = match h[1] & 0x7f {
        126 => {
            let mut b = [0u8; 2];
            s.read_exact(&mut b).await.ok()?;
            u16::from_be_bytes(b) as usize
        }
        127 => {
            let mut b = [0u8; 8];
            s.read_exact(&mut b).await.ok()?;
            u64::from_be_bytes(b) as usize
        }
        n => n as usize,
    };
    let mut p = vec![0u8; len];
    s.read_exact(&mut p).await.ok()?;
    if h[0] & 0x0f != 1 {
        return Some(json!({"opcode": h[0] & 0x0f}));
    }
    serde_json::from_slice(&p).ok()
}

async fn connect(addr: SocketAddr) -> TcpStream {
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
    s
}

async fn call(s: &mut TcpStream, message: Value) -> Value {
    let id = message["id"].clone();
    s.write_all(&masked(message.to_string().as_bytes())).await.unwrap();
    loop {
        let m = tokio::time::timeout(Duration::from_secs(10), read_text(s)).await.expect("reply").expect("frame");
        if m["id"] == id {
            return m;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disconnect_interrupts_running_javascript_and_frees_the_slot() {
    let config = ServerConfig { max_connections: 1, ..ServerConfig::default() };
    let server = Server::bind("127.0.0.1:0".parse().unwrap(), config, ThreadPerConnection::default()).await.unwrap();
    let addr = server.local_addr().unwrap();
    tokio::spawn(server.run());

    let mut s = connect(addr).await;
    let target = call(&mut s, json!({"id":1,"method":"Target.createTarget","params":{"url":"about:blank"}})).await;
    let target = target["result"]["targetId"].clone();
    let session =
        call(&mut s, json!({"id":2,"method":"Target.attachToTarget","params":{"targetId":target,"flatten":true}}))
            .await;
    let session = session["result"]["sessionId"].clone();
    s.write_all(&masked(
        json!({"id":3,"method":"Runtime.evaluate","params":{"expression":"while (true) {}"},"sessionId":session})
            .to_string()
            .as_bytes(),
    ))
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    drop(s); // client disappears while the script spins

    // With one slot, a second connection is only served once the first
    // browser thread has stopped the script and torn down.
    let started = Instant::now();
    let mut second = tokio::time::timeout(Duration::from_secs(10), async {
        let mut s = connect(addr).await;
        let v = call(&mut s, json!({"id":1,"method":"Browser.getVersion"})).await;
        assert!(v["result"]["product"].is_string());
        s
    })
    .await
    .expect("runaway script was interrupted and the slot released");
    assert!(started.elapsed() < Duration::from_secs(10));
    let _ = second.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pipelined_commands_are_all_answered_in_order() {
    let server = Server::bind("127.0.0.1:0".parse().unwrap(), ServerConfig::default(), ThreadPerConnection::default())
        .await
        .unwrap();
    let addr = server.local_addr().unwrap();
    tokio::spawn(server.run());
    let mut s = connect(addr).await;
    let target = call(&mut s, json!({"id":1,"method":"Target.createTarget","params":{"url":"about:blank"}})).await;
    let target = target["result"]["targetId"].clone();
    let session =
        call(&mut s, json!({"id":2,"method":"Target.attachToTarget","params":{"targetId":target,"flatten":true}}))
            .await;
    let session = session["result"]["sessionId"].clone();
    // Far more than the in-flight window, written without waiting, with a
    // slow command first so the browser thread is busy while they queue.
    let mut burst = masked(
        json!({"id":100,"method":"Runtime.evaluate","params":{"expression":"const t = Date.now(); while (Date.now() - t < 200) {} 0"},"sessionId":session})
            .to_string()
            .as_bytes(),
    );
    for id in 101..400 {
        burst.extend(masked(
            json!({"id":id,"method":"Runtime.evaluate","params":{"expression":format!("{id}"),"returnByValue":true},"sessionId":session})
                .to_string()
                .as_bytes(),
        ));
    }
    s.write_all(&burst).await.unwrap();
    let mut ids = Vec::new();
    while ids.len() < 300 {
        let m = tokio::time::timeout(Duration::from_secs(10), read_text(&mut s))
            .await
            .expect("no reply lost")
            .expect("frame");
        if let Some(id) = m["id"].as_u64() {
            if id >= 101 {
                assert_eq!(m["result"]["result"]["value"], id, "{m}");
            }
            ids.push(id);
        }
    }
    assert_eq!(ids, (100..400).collect::<Vec<_>>());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn commands_are_answered_while_page_tasks_and_timers_never_go_idle() {
    let server = Server::bind("127.0.0.1:0".parse().unwrap(), ServerConfig::default(), ThreadPerConnection::default())
        .await
        .unwrap();
    let addr = server.local_addr().unwrap();
    tokio::spawn(server.run());
    let mut s = connect(addr).await;
    let target = call(&mut s, json!({"id":1,"method":"Target.createTarget","params":{"url":"about:blank"}})).await;
    let target = target["result"]["targetId"].clone();
    let session =
        call(&mut s, json!({"id":2,"method":"Target.attachToTarget","params":{"targetId":target,"flatten":true}}))
            .await;
    let session = session["result"]["sessionId"].clone();
    // Page work that is always ready: a self-reposting message task and a
    // zero-delay timer loop. Commands arrive between them and must still run
    // (readiness order bounds the page work that runs ahead of a command).
    let busy = "globalThis.n = 0; globalThis.ticks = 0;
                addEventListener('message', () => { n++; postMessage(0, '*'); });
                postMessage(0, '*');
                (function tick() { ticks++; setTimeout(tick, 0); })();
                0";
    call(&mut s, json!({"id":3,"method":"Runtime.evaluate","params":{"expression":busy},"sessionId":session})).await;
    let progress = "[n, ticks]";
    let mut last = (0, 0);
    for id in 4..24 {
        let v = call(
            &mut s,
            json!({"id":id,"method":"Runtime.evaluate","params":{"expression":progress,"returnByValue":true},"sessionId":session}),
        )
        .await;
        let value = &v["result"]["result"]["value"];
        let now = (value[0].as_u64().expect("n"), value[1].as_u64().expect("ticks"));
        assert!(now.0 >= last.0 && now.1 >= last.1, "{v}");
        last = now;
    }
    // Both kinds of page work kept running between the commands.
    assert!(last.0 > 0 && last.1 > 0, "{last:?}");
    let mut burst = Vec::new();
    for id in 100..200 {
        burst.extend(masked(
            json!({"id":id,"method":"Runtime.evaluate","params":{"expression":format!("{id}"),"returnByValue":true},"sessionId":session})
                .to_string()
                .as_bytes(),
        ));
    }
    s.write_all(&burst).await.unwrap();
    let mut ids = Vec::new();
    while ids.len() < 100 {
        let m = tokio::time::timeout(Duration::from_secs(10), read_text(&mut s))
            .await
            .expect("no reply lost")
            .expect("frame");
        if let Some(id) = m["id"].as_u64() {
            assert_eq!(m["result"]["result"]["value"], id, "{m}");
            ids.push(id);
        }
    }
    assert_eq!(ids, (100..200).collect::<Vec<_>>());
}
