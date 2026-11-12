//! The real per-connection browser threads (JavaScriptCore) behind the server:
//! disconnect interrupts running JavaScript and the connection slot is held
//! until the browser thread has finished.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use pyro_cdp::serve::ThreadPerConnection;
use pyro_cdp::server::{Server, ServerConfig};
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

/// A client frame (masked) with any opcode; payloads up to 64 KiB.
fn masked_frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let mut f = masked(payload);
    f[0] = 0x80 | opcode;
    f
}

/// Next server frame as (opcode, payload).
async fn read_frame(s: &mut TcpStream) -> Option<(u8, Vec<u8>)> {
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
    Some((h[0] & 0x0f, p))
}

/// Reads frames until a close frame; returns its status code.
async fn close_code(s: &mut TcpStream) -> u16 {
    loop {
        let (op, p) = tokio::time::timeout(Duration::from_secs(20), read_frame(s))
            .await
            .expect("close frame in time")
            .expect("close frame before EOF");
        if op == 8 {
            return u16::from_be_bytes([p[0], p[1]]);
        }
    }
}

async fn page_session(s: &mut TcpStream) -> Value {
    let target = call(s, json!({"id":1,"method":"Target.createTarget","params":{"url":"about:blank"}})).await;
    let target = target["result"]["targetId"].clone();
    let session =
        call(s, json!({"id":2,"method":"Target.attachToTarget","params":{"targetId":target,"flatten":true}})).await;
    session["result"]["sessionId"].clone()
}

async fn serve(config: ServerConfig) -> SocketAddr {
    let server = Server::bind("127.0.0.1:0".parse().unwrap(), config, ThreadPerConnection::default()).await.unwrap();
    let addr = server.local_addr().unwrap();
    tokio::spawn(server.run());
    addr
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn close_frame_during_a_script_stops_it_and_frees_the_slot() {
    let addr = serve(ServerConfig { max_connections: 1, ..ServerConfig::default() }).await;
    let mut s = connect(addr).await;
    let session = page_session(&mut s).await;
    s.write_all(&masked(
        json!({"id":3,"method":"Runtime.evaluate","params":{"expression":"while (true) {}"},"sessionId":session})
            .to_string()
            .as_bytes(),
    ))
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    // A close handshake (not just EOF) while the script spins.
    s.write_all(&masked_frame(8, &1000u16.to_be_bytes())).await.unwrap();
    assert_eq!(close_code(&mut s).await, 1000);
    let mut second = tokio::time::timeout(Duration::from_secs(10), async {
        let mut s = connect(addr).await;
        let v = call(&mut s, json!({"id":1,"method":"Browser.getVersion"})).await;
        assert!(v["result"]["product"].is_string());
        s
    })
    .await
    .expect("script stopped and slot released");
    let _ = second.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oversized_message_closes_with_1009() {
    let mut config = ServerConfig::default();
    config.ws.max_message_size = 1024;
    let addr = serve(config).await;
    let mut s = connect(addr).await;
    let v = call(&mut s, json!({"id":1,"method":"Browser.getVersion"})).await;
    assert!(v["result"]["product"].is_string());
    let big = json!({"id":2,"method":"Runtime.evaluate","params":{"expression":"x".repeat(2000)}}).to_string();
    s.write_all(&masked(big.as_bytes())).await.unwrap();
    assert_eq!(close_code(&mut s).await, 1009);
    let mut rest = Vec::new();
    let eof = tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut rest)).await;
    assert!(eof.is_ok(), "socket closed after the close frame");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_that_stops_reading_is_closed_with_1008_at_the_budget() {
    let addr = serve(ServerConfig::default()).await;
    let mut s = connect(addr).await;
    let session = page_session(&mut s).await;
    // ~96 MiB of by-value replies (budget: 64 MiB) while the client reads nothing.
    let mut burst = Vec::new();
    for id in 10..34 {
        burst.extend(masked(
            json!({"id":id,"method":"Runtime.evaluate","params":{"expression":"'x'.repeat(4 << 20)","returnByValue":true},"sessionId":session})
                .to_string()
                .as_bytes(),
        ));
    }
    s.write_all(&burst).await.unwrap();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(close_code(&mut s).await, 1008);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pings_are_answered_in_order_while_the_page_is_busy() {
    let addr = serve(ServerConfig::default()).await;
    let mut s = connect(addr).await;
    let session = page_session(&mut s).await;
    let busy = "(function tick() { const t = Date.now(); while (Date.now() - t < 2) {} setTimeout(tick, 0); })(); 0";
    call(&mut s, json!({"id":3,"method":"Runtime.evaluate","params":{"expression":busy},"sessionId":session})).await;
    let mut burst = Vec::new();
    for i in 0..20u8 {
        burst.extend(masked_frame(9, &[i; 4]));
        burst.extend(masked(
            json!({"id":100 + i as u64,"method":"Runtime.evaluate","params":{"expression":format!("{i}"),"returnByValue":true},"sessionId":session})
                .to_string()
                .as_bytes(),
        ));
    }
    s.write_all(&burst).await.unwrap();
    let (mut pongs, mut ids) = (Vec::new(), Vec::new());
    while pongs.len() < 20 || ids.len() < 20 {
        let (op, p) = tokio::time::timeout(Duration::from_secs(10), read_frame(&mut s))
            .await
            .expect("pong/reply in time")
            .expect("frame");
        match op {
            0xA => pongs.push(p[0]),
            1 => {
                let m: Value = serde_json::from_slice(&p).unwrap();
                if let Some(id) = m["id"].as_u64() {
                    ids.push(id);
                }
            }
            other => panic!("unexpected opcode {other}"),
        }
    }
    assert_eq!(pongs, (0..20).collect::<Vec<u8>>());
    assert_eq!(ids, (100..120).collect::<Vec<u64>>());
}

async fn send(s: &mut TcpStream, message: Value) {
    s.write_all(&masked(message.to_string().as_bytes())).await.unwrap();
}

/// Reads messages until `pred` matches one; returns everything read.
async fn read_until(s: &mut TcpStream, pred: impl Fn(&Value) -> bool) -> Vec<Value> {
    let mut seen = Vec::new();
    loop {
        let m = tokio::time::timeout(Duration::from_secs(10), read_text(s))
            .await
            .unwrap_or_else(|_| panic!("timed out; saw {seen:#?}"))
            .expect("frame");
        let done = pred(&m);
        seen.push(m);
        if done {
            return seen;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dialog_blocks_its_script_until_answered_over_the_socket() {
    let addr = serve(ServerConfig::default()).await;
    let mut s = connect(addr).await;
    let session = page_session(&mut s).await;
    call(&mut s, json!({"id":3,"method":"Page.enable","sessionId":session})).await;
    send(&mut s, json!({"id":4,"method":"Runtime.evaluate","params":{"expression":"confirm('sure?')","returnByValue":true},"sessionId":session})).await;
    // Pipelined behind the dialog: it waits for it.
    send(&mut s, json!({"id":5,"method":"Runtime.evaluate","params":{"expression":"'next'","returnByValue":true},"sessionId":session})).await;
    let seen = read_until(&mut s, |m| m["method"] == "Page.javascriptDialogOpening").await;
    assert!(
        seen.iter().all(|m| m.get("id").is_none() || m["id"] == 3),
        "nothing answered before the dialog: {seen:#?}"
    );
    assert_eq!(seen.last().unwrap()["params"]["message"], "sure?");
    tokio::time::sleep(Duration::from_millis(100)).await;
    send(&mut s, json!({"id":6,"method":"Page.handleJavaScriptDialog","params":{"accept":true},"sessionId":session}))
        .await;
    let seen = read_until(&mut s, |m| m["id"] == 5).await;
    let order: Vec<String> = seen
        .iter()
        .filter_map(|m| m["id"].as_u64().map(|id| format!("#{id}")).or_else(|| m["method"].as_str().map(str::to_owned)))
        .collect();
    assert_eq!(order, ["#6", "Page.javascriptDialogClosed", "#4", "#5"], "{seen:#?}");
    assert_eq!(seen[2]["result"]["result"]["value"], true);
    assert_eq!(seen[3]["result"]["result"]["value"], "next");
    let _ = s.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closing_the_socket_while_a_dialog_shows_frees_the_slot() {
    let addr = serve(ServerConfig { max_connections: 1, ..ServerConfig::default() }).await;
    let mut s = connect(addr).await;
    let session = page_session(&mut s).await;
    call(&mut s, json!({"id":3,"method":"Page.enable","sessionId":session})).await;
    send(&mut s, json!({"id":4,"method":"Runtime.evaluate","params":{"expression":"alert('x')"},"sessionId":session}))
        .await;
    read_until(&mut s, |m| m["method"] == "Page.javascriptDialogOpening").await;
    drop(s);
    let mut second = tokio::time::timeout(Duration::from_secs(10), async {
        let mut s = connect(addr).await;
        let v = call(&mut s, json!({"id":1,"method":"Browser.getVersion"})).await;
        assert!(v["result"]["product"].is_string());
        s
    })
    .await
    .expect("the dialog was dismissed and the connection torn down");
    let _ = second.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminate_execution_stops_a_running_script() {
    let addr = serve(ServerConfig::default()).await;
    let mut s = connect(addr).await;
    let session = page_session(&mut s).await;
    send(&mut s, json!({"id":3,"method":"Runtime.evaluate","params":{"expression":"for (;;) {}"},"sessionId":session}))
        .await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    send(&mut s, json!({"id":4,"method":"Runtime.terminateExecution","sessionId":session})).await;
    let seen = read_until(&mut s, |m| m["id"] == 4).await;
    let evaluated = seen.iter().find(|m| m["id"] == 3).expect("the evaluation was answered");
    assert_eq!(evaluated["error"]["message"], "Execution was terminated", "{evaluated}");
    assert!(evaluated.get("result").is_none(), "no exceptionDetails: {evaluated}");
    let next = call(&mut s, json!({"id":5,"method":"Runtime.evaluate","params":{"expression":"1 + 1","returnByValue":true},"sessionId":session})).await;
    assert_eq!(next["result"]["result"]["value"], 2, "only that script stopped: {next}");
    let _ = s.shutdown().await;
}
