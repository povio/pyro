//! A connection that ends leaves its browser thread and JavaScript engine to
//! the next one (`lp_cdp::serve::Park`): nothing of the first client reaches
//! the second, and an engine that cannot be proven clean is dropped.

use std::io::{Read, Write};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lp_cdp::serve::{EngineReuse, EngineReuseStats, Park, ThreadPerConnection};
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

/// One client: its socket and the id of its next command.
struct Client {
    s: TcpStream,
    id: u64,
    session: Value,
}

impl Client {
    async fn new(addr: SocketAddr) -> Client {
        Client { s: connect(addr).await, id: 0, session: Value::Null }
    }

    async fn call(&mut self, method: &str, params: Value) -> Value {
        self.id += 1;
        let mut message = json!({"id": self.id, "method": method, "params": params});
        let of_browser = ["Target.", "Browser.", "Storage."].iter().any(|domain| method.starts_with(domain));
        if !self.session.is_null() && !of_browser {
            message["sessionId"] = self.session.clone();
        }
        self.s.write_all(&masked(message.to_string().as_bytes())).await.unwrap();
        loop {
            let m = tokio::time::timeout(Duration::from_secs(20), read_text(&mut self.s))
                .await
                .unwrap_or_else(|_| panic!("no reply to {method}"))
                .expect("frame");
            if m["id"] == json!(self.id) {
                return m;
            }
        }
    }

    /// A page at `url`, loaded, with a session attached. Its target id.
    async fn page(&mut self, url: &str) -> Value {
        let target = self.call("Target.createTarget", json!({"url": "about:blank"})).await;
        let target = target["result"]["targetId"].clone();
        let session = self.call("Target.attachToTarget", json!({"targetId": target, "flatten": true})).await;
        self.session = session["result"]["sessionId"].clone();
        assert!(self.session.is_string(), "attached: {session}");
        let navigated = self.call("Page.navigate", json!({"url": url})).await;
        assert!(navigated["error"].is_null(), "navigated: {navigated}");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let state = self.eval("location.href + ' ' + document.readyState").await;
            if state == json!(format!("{url} complete")) {
                return target;
            }
            assert!(Instant::now() < deadline, "the page never loaded: {state}");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    async fn eval(&mut self, expression: &str) -> Value {
        let reply = self
            .call("Runtime.evaluate", json!({"expression": expression, "returnByValue": true, "awaitPromise": true}))
            .await;
        assert!(reply["result"]["exceptionDetails"].is_null(), "{expression}: {reply}");
        reply["result"]["result"]["value"].clone()
    }

    async fn close(mut self) {
        let _ = self.s.shutdown().await;
    }
}

/// A site on a local port: every path answers a small page. `requests` holds
/// the `Cookie` header of each request (empty when there was none).
fn site() -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://127.0.0.1:{}/", listener.local_addr().unwrap().port());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = requests.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let seen = seen.clone();
            std::thread::spawn(move || {
                let mut head = Vec::new();
                let mut b = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") {
                    if stream.read(&mut b).unwrap_or(0) == 0 {
                        return;
                    }
                    head.push(b[0]);
                }
                let head = String::from_utf8_lossy(&head).to_string();
                let cookie = head
                    .lines()
                    .find_map(|l| l.strip_prefix("Cookie: ").or_else(|| l.strip_prefix("cookie: ")))
                    .unwrap_or("")
                    .to_string();
                seen.lock().unwrap().push(cookie);
                let body = "<!doctype html><title>site</title><p id=p>site</p>";
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nCache-Control: max-age=3600\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            });
        }
    });
    (url, requests)
}

async fn serve(factory: ThreadPerConnection) -> (SocketAddr, Arc<Park>) {
    let park = factory.park();
    let server = Server::bind("127.0.0.1:0".parse().unwrap(), ServerConfig::default(), factory).await.unwrap();
    let addr = server.local_addr().unwrap();
    tokio::spawn(server.run());
    (addr, park)
}

/// Waits until `ended` connections' ends have been decided: each either
/// parked its engine or dropped it.
async fn decided(park: &Park, ended: u64) -> EngineReuseStats {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let stats = park.stats();
        if stats.parked + stats.dropped >= ended {
            return stats;
        }
        assert!(Instant::now() < deadline, "connection ends undecided: {stats:?}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

const PROBE: &str = "JSON.stringify({
    global: typeof leaked,
    onWindow: Object.getOwnPropertyNames(globalThis).filter((n) => /leak/i.test(n)),
    prototype: Object.prototype.leakedProto === undefined && Array.prototype.leakedProto === undefined,
    local: localStorage.getItem('k'),
    session: sessionStorage.getItem('k'),
    cookie: document.cookie,
    binding: typeof leakedBinding,
    agent: navigator.userAgent,
    zone: Intl.DateTimeFormat().resolvedOptions().timeZone,
    symbol: Symbol.for('leaked').description,
    title: document.title,
})";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_next_connection_gets_the_engine_and_nothing_of_the_first_client() {
    let (url, requests) = site();
    let (addr, park) = serve(ThreadPerConnection::default().reuse(EngineReuse::Always)).await;

    let mut a = Client::new(addr).await;
    let host_agent = a.call("Browser.getVersion", json!({})).await["result"]["userAgent"].clone();
    let target_a = a.page(&url).await;
    let before: Value = serde_json::from_str(a.eval(PROBE).await.as_str().unwrap()).unwrap();
    let host_zone = before["zone"].clone();
    let other_zone = if host_zone == json!("Asia/Tokyo") { "America/Lima" } else { "Asia/Tokyo" };

    // Everything a client can leave behind.
    for (method, params) in [
        ("Runtime.enable", json!({})),
        ("Runtime.addBinding", json!({"name": "leakedBinding"})),
        ("Network.enable", json!({})),
        ("Network.setCookie", json!({"name": "viaCdp", "value": "A", "url": url})),
        ("Network.setExtraHTTPHeaders", json!({"headers": {"X-Leaked": "A"}})),
        ("Emulation.setUserAgentOverride", json!({"userAgent": "LeakedAgent/1"})),
        ("Emulation.setTimezoneOverride", json!({"timezoneId": other_zone})),
        ("Page.addScriptToEvaluateOnNewDocument", json!({"source": "globalThis.leakedInit = 1"})),
    ] {
        let reply = a.call(method, params).await;
        assert!(reply["error"].is_null(), "{method}: {reply}");
    }
    a.eval(
        "globalThis.leaked = 'A'; Object.prototype.leakedProto = 1; Array.prototype.leakedProto = 1;
         localStorage.setItem('k', 'A'); sessionStorage.setItem('k', 'A'); document.cookie = 'viaScript=A; path=/';
         globalThis.leakedTimer = setInterval(() => { globalThis.leakedTicks = (globalThis.leakedTicks || 0) + 1 }, 1);
         globalThis.leakedPromise = new Promise(() => {}).then(() => {});
         queueMicrotask(() => { globalThis.leakedMicrotask = 1 });
         document.title = 'leaked'; 1",
    )
    .await;
    let dirty: Value = serde_json::from_str(a.eval(PROBE).await.as_str().unwrap()).unwrap();
    assert_eq!(dirty["global"], "string");
    assert_eq!(dirty["local"], "A");
    assert_eq!(dirty["binding"], "function");
    assert_eq!(dirty["agent"], "LeakedAgent/1");
    assert_eq!(dirty["zone"], other_zone);
    assert!(dirty["cookie"].as_str().unwrap().contains("viaScript=A"), "{dirty}");
    assert!(dirty["cookie"].as_str().unwrap().contains("viaCdp=A"), "{dirty}");
    // Interception last: with it on, the page's requests wait for the client.
    let reply = a.call("Fetch.enable", json!({})).await;
    assert!(reply["error"].is_null(), "{reply}");
    let cookies = a.call("Storage.getCookies", json!({})).await;
    assert_eq!(cookies["result"]["cookies"].as_array().map(Vec::len), Some(2), "{cookies}");
    let session_a = a.session.clone();
    a.close().await;

    let stats = decided(&park, 1).await;
    assert_eq!(park.last_refusal(), None);
    assert_eq!(stats, EngineReuseStats { parked: 1, reused: 0, dropped: 0, expired: 0 });

    let mut b = Client::new(addr).await;
    let version = b.call("Browser.getVersion", json!({})).await;
    assert_eq!(version["result"]["userAgent"], host_agent);
    assert_eq!(park.stats().reused, 1, "the second connection runs in the first one's engine");
    let targets = b.call("Target.getTargets", json!({})).await;
    let targets = targets["result"]["targetInfos"].as_array().unwrap().clone();
    assert!(targets.is_empty(), "{targets:?}");
    // The first client's target and session mean nothing here.
    let stale = b.call("Target.attachToTarget", json!({"targetId": target_a, "flatten": true})).await;
    assert!(stale["error"].is_object(), "{stale}");
    b.session = session_a;
    let stale = b.call("Runtime.evaluate", json!({"expression": "1"})).await;
    assert!(stale["error"].is_object(), "{stale}");
    b.session = Value::Null;

    // Loads without anyone answering Fetch.requestPaused: no interception.
    let sent_before = requests.lock().unwrap().len();
    b.page(&url).await;
    let after: Value = serde_json::from_str(b.eval(PROBE).await.as_str().unwrap()).unwrap();
    assert_eq!(after, before, "the second client's page is a first page");
    assert_eq!(
        b.eval("typeof leakedInit + typeof leakedTicks + typeof leakedMicrotask").await,
        "undefinedundefinedundefined"
    );
    let cookies = b.call("Storage.getCookies", json!({})).await;
    assert_eq!(cookies["result"]["cookies"], json!([]), "{cookies}");
    let sent = requests.lock().unwrap()[sent_before..].to_vec();
    assert!(!sent.is_empty(), "the second client's page came from the network, not from the first one's cache");
    assert!(sent.iter().all(|cookie| cookie.is_empty()), "{sent:?}");
    b.close().await;

    // The second client's page made the engine dirty too; it is proven clean again.
    let stats = decided(&park, 2).await;
    assert_eq!(park.last_refusal(), None);
    assert_eq!(stats, EngineReuseStats { parked: 2, reused: 1, dropped: 0, expired: 0 });
    assert!(park.occupied());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_killed_mid_script_takes_its_engine_with_it() {
    let (addr, park) = serve(ThreadPerConnection::default().reuse(EngineReuse::Always)).await;

    let mut a = Client::new(addr).await;
    a.page("about:blank").await;
    a.eval("globalThis.leaked = 'A'; 1").await;
    a.id += 1;
    let spin = json!({"id": a.id, "method": "Runtime.evaluate", "sessionId": a.session,
        "params": {"expression": "while (true) {}"}});
    a.s.write_all(&masked(spin.to_string().as_bytes())).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    drop(a); // the client disappears while its script spins

    let stats = decided(&park, 1).await;
    assert_eq!(stats, EngineReuseStats { parked: 0, reused: 0, dropped: 1, expired: 0 });
    assert_eq!(park.last_refusal(), Some("a script was terminated"));
    assert!(!park.occupied());

    // The next client gets an engine of its own, which works.
    let mut b = Client::new(addr).await;
    b.page("about:blank").await;
    assert_eq!(b.eval("typeof leaked + ' ' + (6 * 7)").await, "undefined 42");
    assert_eq!(b.eval("new Promise((r) => setTimeout(() => r('timers run'), 5))").await, "timers run");
    assert_eq!(park.stats().reused, 0);
    b.close().await;
    let stats = decided(&park, 2).await;
    assert_eq!(stats, EngineReuseStats { parked: 1, reused: 0, dropped: 1, expired: 0 });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminate_execution_makes_the_engine_unfit_for_the_next_client() {
    let (addr, park) = serve(ThreadPerConnection::default().reuse(EngineReuse::Always)).await;
    let mut a = Client::new(addr).await;
    a.page("about:blank").await;
    a.id += 1;
    let spin = json!({"id": a.id, "method": "Runtime.evaluate", "sessionId": a.session,
        "params": {"expression": "while (true) {}"}});
    a.s.write_all(&masked(spin.to_string().as_bytes())).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let stopped = a.call("Runtime.terminateExecution", json!({})).await;
    assert!(stopped["error"].is_null(), "{stopped}");
    assert_eq!(a.eval("1 + 1").await, 2);
    a.close().await;
    let stats = decided(&park, 1).await;
    assert_eq!(stats.dropped, 1, "{stats:?}");
    assert_eq!(park.last_refusal(), Some("a script was terminated"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_connection_that_made_no_page_leaves_its_engine_too() {
    let (addr, park) = serve(ThreadPerConnection::default()).await;
    for round in 0..4u64 {
        let mut c = Client::new(addr).await;
        let version = c.call("Browser.getVersion", json!({})).await;
        assert!(version["result"]["product"].is_string());
        c.close().await;
        let stats = decided(&park, round + 1).await;
        assert_eq!(stats, EngineReuseStats { parked: round + 1, reused: round, dropped: 0, expired: 0 });
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn at_most_one_engine_waits() {
    let (addr, park) = serve(ThreadPerConnection::default().reuse(EngineReuse::Always)).await;
    let mut clients = Vec::new();
    for _ in 0..3 {
        let mut c = Client::new(addr).await;
        c.page("about:blank").await;
        c.eval("globalThis.leaked = 1").await;
        clients.push(c);
    }
    for c in clients {
        c.close().await;
    }
    let stats = decided(&park, 3).await;
    assert_eq!(stats, EngineReuseStats { parked: 1, reused: 0, dropped: 2, expired: 0 });
    assert_eq!(park.last_refusal(), Some("another engine already waits"));

    // Two clients at once: one gets the waiting engine, the other a new one.
    let mut first = Client::new(addr).await;
    let mut second = Client::new(addr).await;
    first.page("about:blank").await;
    second.page("about:blank").await;
    first.eval("globalThis.leaked = 'first'").await;
    assert_eq!(second.eval("typeof leaked").await, "undefined");
    assert_eq!(park.stats().reused, 1);
    first.close().await;
    second.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workers_timers_and_pending_requests_end_with_their_client() {
    let (url, _requests) = site();
    let (addr, park) = serve(ThreadPerConnection::default().reuse(EngineReuse::Always)).await;
    let mut a = Client::new(addr).await;
    a.page(&url).await;
    a.eval(
        "globalThis.w = new Worker(URL.createObjectURL(new Blob(['setInterval(() => postMessage(1), 1)'])));
         w.onmessage = () => { globalThis.leakedMessages = (globalThis.leakedMessages || 0) + 1 };
         globalThis.c = new MessageChannel(); c.port1.onmessage = () => {}; c.port2.postMessage(1);
         globalThis.bc = new BroadcastChannel('leaked'); bc.onmessage = () => {};
         setInterval(() => {}, 1); requestAnimationFrame(function again() { requestAnimationFrame(again) });
         globalThis.ac = new AbortController();
         globalThis.pending = fetch('http://127.0.0.1:9/never', {signal: ac.signal}).catch(() => {});
         new Promise((r) => setTimeout(r, 30))",
    )
    .await;
    a.close().await;
    let stats = decided(&park, 1).await;
    // Whatever the verdict, it is one of the two; a refusal names its cause.
    assert_eq!(stats.parked + stats.dropped, 1);
    assert_eq!((stats.parked, park.last_refusal()), (1, None), "the worker's client is gone and so is the worker");

    let mut b = Client::new(addr).await;
    b.page(&url).await;
    assert_eq!(
        b.eval(
            "new Promise((resolve) => {
                 const bc = new BroadcastChannel('leaked');
                 let heard = 0;
                 bc.onmessage = () => { heard++ };
                 setTimeout(() => resolve(typeof leakedMessages + ' ' + typeof w + ' ' + heard), 50);
             })"
        )
        .await,
        "undefined undefined 0"
    );
    b.close().await;
}

/// The default: a used engine is destroyed, its thread serves on; an
/// unused engine serves on too.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn by_default_an_engine_a_page_ran_in_is_not_kept_and_its_thread_is() {
    let (url, requests) = site();
    let (addr, park) = serve(ThreadPerConnection::default()).await;
    let mut a = Client::new(addr).await;
    a.page(&url).await;
    let before: Value = serde_json::from_str(a.eval(PROBE).await.as_str().unwrap()).unwrap();
    a.call("Runtime.addBinding", json!({"name": "leakedBinding"})).await;
    a.call("Emulation.setUserAgentOverride", json!({"userAgent": "LeakedAgent/1"})).await;
    a.eval(
        "globalThis.leaked = 'A'; Object.prototype.leakedProto = 1; localStorage.setItem('k', 'A');
         sessionStorage.setItem('k', 'A'); document.cookie = 'viaScript=A; path=/'; document.title = 'leaked'; 1",
    )
    .await;
    a.close().await;
    assert_eq!(decided(&park, 1).await, EngineReuseStats { parked: 1, reused: 0, dropped: 0, expired: 0 });

    let sent_before = requests.lock().unwrap().len();
    let mut b = Client::new(addr).await;
    b.page(&url).await;
    let after: Value = serde_json::from_str(b.eval(PROBE).await.as_str().unwrap()).unwrap();
    assert_eq!(after, before);
    let sent = requests.lock().unwrap()[sent_before..].to_vec();
    assert!(!sent.is_empty() && sent.iter().all(|cookie| cookie.is_empty()), "{sent:?}");
    // A client killed mid-script: its engine goes like any used one.
    b.id += 1;
    let spin = json!({"id": b.id, "method": "Runtime.evaluate", "sessionId": b.session,
        "params": {"expression": "while (true) {}"}});
    b.s.write_all(&masked(spin.to_string().as_bytes())).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    drop(b);
    assert_eq!(decided(&park, 2).await, EngineReuseStats { parked: 2, reused: 0, dropped: 0, expired: 0 });

    // The same thread, a new engine, nothing used: this one is kept.
    let mut c = Client::new(addr).await;
    assert!(c.call("Browser.getVersion", json!({})).await["result"]["product"].is_string());
    c.close().await;
    assert_eq!(decided(&park, 3).await, EngineReuseStats { parked: 3, reused: 0, dropped: 0, expired: 0 });
    let mut d = Client::new(addr).await;
    d.page("about:blank").await;
    assert_eq!(d.eval("typeof leaked + ' ' + (6 * 7)").await, "undefined 42");
    assert_eq!(park.stats().reused, 1);
    d.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn thread_reuse_keeps_the_thread_and_makes_a_new_engine() {
    let (addr, park) = serve(ThreadPerConnection::default().reuse(EngineReuse::Thread)).await;
    for round in 0..3u64 {
        let mut c = Client::new(addr).await;
        c.page("about:blank").await;
        assert_eq!(c.eval("typeof leaked").await, "undefined");
        c.eval("globalThis.leaked = 1").await;
        c.close().await;
        let stats = decided(&park, round + 1).await;
        assert_eq!(stats, EngineReuseStats { parked: round + 1, reused: 0, dropped: 0, expired: 0 });
        assert_eq!(park.last_refusal(), None);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reuse_turned_off_keeps_no_engine() {
    let (addr, park) = serve(ThreadPerConnection::default().engine_reuse(false)).await;
    for _ in 0..2 {
        let mut c = Client::new(addr).await;
        c.page("about:blank").await;
        assert_eq!(c.eval("typeof leaked").await, "undefined");
        c.eval("globalThis.leaked = 1").await;
        c.close().await;
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(park.stats(), EngineReuseStats::default());
    assert!(!park.occupied());
}
