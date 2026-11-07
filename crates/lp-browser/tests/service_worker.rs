//! Service workers over real HTTP: registration and lifecycle, interception of
//! a controlled page's fetch() and XHR (responses and passthrough), messages
//! with transferred ports both ways, and MSW's real `mockServiceWorker.js`
//! driven with the messages its browser library sends.
use lp_browser::{BrowserThread, NavigateOpts, PageId};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const MSW: &str = include_str!("fixtures/msw/mockServiceWorker.js");

/// The service worker of the plain tests: mocks `/api/*`, answers `/slow`
/// later, fails `/broken`, and passes everything else through.
const SW: &str = r#"
self.addEventListener('install', (e) => { e.waitUntil(Promise.resolve().then(() => self.skipWaiting())); });
self.addEventListener('activate', (e) => { e.waitUntil(self.clients.claim()); });
self.addEventListener('fetch', (e) => {
  const url = new URL(e.request.url);
  if (url.pathname.startsWith('/api/')) {
    e.respondWith(e.request.text().then((body) => new Response(JSON.stringify({
      mocked: url.pathname, method: e.request.method, body, mode: e.request.mode,
      client: typeof e.clientId, header: e.request.headers.get('x-test'),
    }), { status: 201, statusText: 'Made', headers: { 'content-type': 'application/json', 'x-sw': 'yes' } })));
  } else if (url.pathname === '/broken') {
    e.respondWith(Promise.reject(new Error('nope')));
  } else if (url.pathname === '/via-fetch') {
    e.respondWith(fetch('/real'));
  } else if (url.pathname === '/teed') {
    // The clone's body goes to the page as a transferred stream.
    e.respondWith(fetch('/real').then(async (r) => {
      const c = r.clone();
      const client = await self.clients.get(e.clientId);
      client.postMessage({ body: c.body }, [c.body]);
      return r;
    }));
  }
});
self.addEventListener('message', (e) => {
  const reply = { echo: e.data, source: typeof e.source.id, frameType: e.source.frameType, ports: e.ports.length };
  if (e.ports.length) e.ports[0].postMessage(reply);
  else e.source.postMessage(reply);
});
"#;

/// A dedicated worker that answers on transferred ports and streams.
const PORTS_WORKER: &str = r#"
onmessage = async (e) => {
  if (e.data.kind === 'port') {
    const port = e.ports[0];
    port.onmessage = (m) => port.postMessage('pong:' + m.data);
    port.postMessage('ready');
  } else if (e.data.kind === 'stream') {
    const text = await new Response(e.data.stream).text();
    const back = new ReadableStream({ start(c) { c.enqueue(new TextEncoder().encode('back:' + text)); c.close(); } });
    postMessage({ stream: back }, [back]);
  }
};
"#;

struct Server {
    origin: String,
    hits: Arc<Mutex<Vec<String>>>,
    thread: Option<JoinHandle<()>>,
}

fn respond(stream: &mut TcpStream, status: &str, headers: &[(&str, &str)], body: &[u8]) {
    let mut head = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n", body.len());
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
}

impl Server {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let hits = Arc::new(Mutex::new(Vec::new()));
        let seen = hits.clone();
        let thread = thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request = String::new();
                if reader.read_line(&mut request).unwrap_or(0) == 0 {
                    continue;
                }
                let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = v.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0; length];
                let _ = reader.read_exact(&mut body);
                if path == "/stop" {
                    break;
                }
                seen.lock().unwrap().push(path.clone());
                let js = [("Content-Type", "text/javascript")];
                match path.as_str() {
                    "/sw.js" => respond(&mut stream, "200 OK", &js, SW.as_bytes()),
                    "/mockServiceWorker.js" => respond(&mut stream, "200 OK", &js, MSW.as_bytes()),
                    "/sub/sw.js" => respond(&mut stream, "200 OK", &js, SW.as_bytes()),
                    "/sub/allowed.js" => respond(
                        &mut stream,
                        "200 OK",
                        &[("Content-Type", "text/javascript"), ("Service-Worker-Allowed", "/")],
                        SW.as_bytes(),
                    ),
                    "/html.js" => respond(&mut stream, "200 OK", &[("Content-Type", "text/html")], SW.as_bytes()),
                    "/throws.js" => respond(&mut stream, "200 OK", &js, b"throw new Error('top level')"),
                    "/ports-worker.js" => respond(&mut stream, "200 OK", &js, PORTS_WORKER.as_bytes()),
                    "/real" | "/api/passthrough" => {
                        respond(&mut stream, "200 OK", &[("Content-Type", "text/plain")], b"from network")
                    }
                    p if p.starts_with("/page") => respond(
                        &mut stream,
                        "200 OK",
                        &[("Content-Type", "text/html")],
                        b"<!doctype html><title>sw</title><p>page</p>",
                    ),
                    _ => respond(&mut stream, "404 Not Found", &[("Content-Type", "text/plain")], b"missing"),
                }
            }
        });
        Self { origin, hits, thread: Some(thread) }
    }

    fn hits(&self, path: &str) -> usize {
        self.hits.lock().unwrap().iter().filter(|p| *p == path).count()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Ok(mut stream) = TcpStream::connect(self.origin.trim_start_matches("http://")) {
            let _ = stream.write_all(b"GET /stop HTTP/1.1\r\nHost: localhost\r\n\r\n");
        }
        self.thread.take().unwrap().join().unwrap();
    }
}

fn eval(thread: &BrowserThread, page: PageId, script: &str) -> String {
    let realm = thread.browser.page(page).unwrap().main_frame.main_realm().unwrap();
    let scope = realm.scope().unwrap();
    scope.eval(script, None, 1).unwrap_or_else(|e| panic!("{}", e.details(&scope).text())).to_string().unwrap()
}

fn until(thread: &mut BrowserThread, page: PageId, condition: &str) {
    let result = thread.run_until(Some(Instant::now() + Duration::from_secs(10)), |browser| {
        let realm = browser.page(page).unwrap().main_frame.main_realm().unwrap();
        let scope = realm.scope().unwrap();
        scope.eval(condition, None, 1).is_ok_and(|v| v.to_bool())
    });
    if result != lp_runtime::RunOutcome::Done {
        let log = eval(thread, page, "JSON.stringify(globalThis.log || [])");
        panic!("waiting for {condition}: {log}");
    }
}

fn open(thread: &mut BrowserThread, server: &Server, path: &str) -> PageId {
    let page = thread.browser.create_page().unwrap();
    thread.browser.navigate(page, NavigateOpts::address_bar(format!("{}{path}", server.origin))).unwrap();
    until(thread, page, &format!("location.pathname === '{path}' && document.readyState === 'complete'"));
    page
}

fn setup() -> (Server, BrowserThread) {
    let server = Server::new();
    let mut thread = crate::browser_thread().unwrap();
    thread.enable_network(Default::default()).unwrap();
    (server, thread)
}

/// Registers /sw.js and waits until it controls the page.
fn register(thread: &mut BrowserThread, page: PageId) {
    eval(
        thread,
        page,
        "globalThis.log = [];
         navigator.serviceWorker.addEventListener('controllerchange', () => log.push('controllerchange'));
         navigator.serviceWorker.register('/sw.js').then((r) => {
           log.push('registered:' + r.scope + ':' + !!(r.installing || r.waiting || r.active));
           const w = r.installing || r.waiting || r.active;
           w.addEventListener('statechange', () => log.push('state:' + w.state));
         }, (e) => log.push('error:' + e.name + ':' + e.message)); 'ok'",
    );
    until(
        thread,
        page,
        "navigator.serviceWorker.controller !== null && navigator.serviceWorker.controller.state === 'activated'",
    );
}

#[test]
fn service_worker_lifecycle_and_fetch_interception() {
    let (server, mut thread) = setup();
    let page = open(&mut thread, &server, "/page");
    assert_eq!(
        eval(&thread, page, "typeof navigator.serviceWorker + ':' + navigator.serviceWorker.controller"),
        "object:null"
    );
    assert_eq!(eval(&thread, page, "String(navigator.serviceWorker === navigator.serviceWorker)"), "true");
    register(&mut thread, page);
    let log = eval(&thread, page, "log.join(',')");
    assert!(log.contains(&format!("registered:{}/:true", server.origin)), "{log}");
    assert!(log.contains("controllerchange"), "{log}");
    assert!(log.contains("state:activated"), "{log}");
    eval(
        &thread,
        page,
        "navigator.serviceWorker.ready.then((r) => { globalThis.ready = r.active.scriptURL; globalThis.same = r.active === navigator.serviceWorker.controller; }); 'ok'",
    );
    until(&mut thread, page, "globalThis.ready !== undefined");
    assert_eq!(eval(&thread, page, "ready + ':' + same"), format!("{}/sw.js:true", server.origin));

    // A mocked response, with the request as the worker saw it.
    eval(
        &thread,
        page,
        "fetch('/api/thing', {method: 'POST', body: 'hello', headers: {'x-test': '1'}})
           .then((r) => r.json().then((j) => { globalThis.mocked = [r.status, r.statusText, r.headers.get('x-sw'), j.mocked, j.method, j.body, j.mode, j.client, j.header].join('|'); })); 'ok'",
    );
    until(&mut thread, page, "globalThis.mocked !== undefined");
    assert_eq!(eval(&thread, page, "mocked"), "201|Made|yes|/api/thing|POST|hello|cors|string|1");
    assert_eq!(server.hits("/api/thing"), 0);

    // No respondWith: the network answers.
    eval(&thread, page, "fetch('/real').then((r) => r.text()).then((t) => globalThis.real = t); 'ok'");
    until(&mut thread, page, "globalThis.real !== undefined");
    assert_eq!(eval(&thread, page, "real"), "from network");
    assert_eq!(server.hits("/real"), 1);

    // A network body read whole while its clone's stream is read.
    eval(
        &thread,
        page,
        "fetch('/real').then((r) => { const c = r.clone(); const reader = c.body.getReader();
           return reader.read().then((x) => r.arrayBuffer().then((b) => [b.byteLength, x.value.length])); })
           .then((v) => globalThis.teed = v.join()); 'ok'",
    );
    until(&mut thread, page, "globalThis.teed !== undefined");
    assert_eq!(eval(&thread, page, "teed"), "12,12");
    eval(
        &thread,
        page,
        "navigator.serviceWorker.onmessage = (e) => new Response(e.data.body).text().then((t) => globalThis.streamed = t);
         fetch('/teed').then((r) => r.text()).then((t) => globalThis.teedBody = t); 'ok'",
    );
    until(&mut thread, page, "globalThis.teedBody !== undefined && globalThis.streamed !== undefined");
    assert_eq!(eval(&thread, page, "teedBody + '|' + streamed"), "from network|from network");
    eval(&thread, page, "navigator.serviceWorker.onmessage = null; 'ok'");

    // respondWith(fetch()) from the worker, and a rejected respondWith.
    eval(&thread, page, "fetch('/via-fetch').then((r) => r.text()).then((t) => globalThis.via = t); 'ok'");
    until(&mut thread, page, "globalThis.via !== undefined");
    assert_eq!(eval(&thread, page, "via"), "from network");
    eval(
        &thread,
        page,
        "fetch('/broken').then(() => globalThis.broken = 'resolved', (e) => globalThis.broken = e.name); 'ok'",
    );
    until(&mut thread, page, "globalThis.broken !== undefined");
    assert_eq!(eval(&thread, page, "broken"), "TypeError");
    assert_eq!(server.hits("/broken"), 0);

    // XHR goes through the worker too.
    eval(
        &thread,
        page,
        "const x = new XMLHttpRequest(); x.open('GET', '/api/xhr'); x.onload = () => globalThis.xhr = x.status + ':' + JSON.parse(x.responseText).mocked; x.send(); 'ok'",
    );
    until(&mut thread, page, "globalThis.xhr !== undefined");
    assert_eq!(eval(&thread, page, "xhr"), "201:/api/xhr");

    // A new page in scope is controlled from the start.
    let second = open(&mut thread, &server, "/page2");
    assert_eq!(eval(&thread, second, "navigator.serviceWorker.controller.state"), "activated");
    eval(&thread, second, "fetch('/api/two').then((r) => r.json()).then((j) => globalThis.two = j.mocked); 'ok'");
    until(&mut thread, second, "globalThis.two !== undefined");
    assert_eq!(eval(&thread, second, "two"), "/api/two");
    assert_eq!(server.hits("/sw.js"), 1);
}

#[test]
fn service_worker_messages_and_ports() {
    let (server, mut thread) = setup();
    let page = open(&mut thread, &server, "/page");
    register(&mut thread, page);
    eval(
        &thread,
        page,
        "navigator.serviceWorker.onmessage = (e) => { globalThis.direct = JSON.stringify(e.data) + ':' + (e.source === navigator.serviceWorker.controller); };
         navigator.serviceWorker.controller.postMessage({hello: 'sw'});
         const channel = new MessageChannel();
         channel.port1.onmessage = (e) => { globalThis.viaPort = JSON.stringify(e.data); };
         navigator.serviceWorker.controller.postMessage('with port', [channel.port2]); 'ok'",
    );
    until(&mut thread, page, "globalThis.direct !== undefined && globalThis.viaPort !== undefined");
    assert_eq!(
        eval(&thread, page, "direct"),
        r#"{"echo":{"hello":"sw"},"source":"string","frameType":"top-level","ports":0}:true"#
    );
    assert_eq!(
        eval(&thread, page, "viaPort"),
        r#"{"echo":"with port","source":"string","frameType":"top-level","ports":1}"#
    );
}

#[test]
fn service_worker_registration_errors() {
    let (server, mut thread) = setup();
    let page = open(&mut thread, &server, "/page");
    eval(
        &thread,
        page,
        "globalThis.results = {};
         const r = (k, p) => p.then((v) => results[k] = 'ok:' + v.scope, (e) => results[k] = e.name + ':' + e.message);
         r('missing', navigator.serviceWorker.register('/nope.js'));
         r('mime', navigator.serviceWorker.register('/html.js'));
         r('throws', navigator.serviceWorker.register('/throws.js'));
         r('scope', navigator.serviceWorker.register('/sub/sw.js', {scope: '/'}));
         r('allowed', navigator.serviceWorker.register('/sub/allowed.js', {scope: '/other/'}));
         r('cross', navigator.serviceWorker.register('http://example.invalid/sw.js'));
         r('slash', navigator.serviceWorker.register('/a%2fb/sw.js'));
         'ok'",
    );
    until(&mut thread, page, "Object.keys(results).length === 7");
    let get = |k: &str| eval(&thread, page, &format!("results.{k}"));
    assert!(get("missing").starts_with("TypeError:") && get("missing").contains("(404)"), "{}", get("missing"));
    assert!(get("mime").starts_with("SecurityError:") && get("mime").contains("MIME"), "{}", get("mime"));
    assert!(
        get("throws").starts_with("TypeError:") && get("throws").contains("evaluation failed"),
        "{}",
        get("throws")
    );
    assert!(get("scope").starts_with("SecurityError:") && get("scope").contains("max scope"), "{}", get("scope"));
    assert_eq!(get("allowed"), format!("ok:{}/other/", server.origin));
    assert!(get("cross").starts_with("SecurityError:"), "{}", get("cross"));
    assert!(get("slash").starts_with("TypeError:"), "{}", get("slash"));
    eval(
        &thread,
        page,
        "navigator.serviceWorker.getRegistrations().then((l) => globalThis.regs = l.map((r) => r.scope).join()); 'ok'",
    );
    until(&mut thread, page, "globalThis.regs !== undefined");
    assert_eq!(eval(&thread, page, "regs"), format!("{}/other/", server.origin));
    // The page is not in the registered scope: it stays uncontrolled.
    assert_eq!(eval(&thread, page, "String(navigator.serviceWorker.controller)"), "null");
    eval(&thread, page, "navigator.serviceWorker.getRegistration('/other/x').then((r) => r.unregister()).then((v) => globalThis.unregistered = v); 'ok'");
    until(&mut thread, page, "globalThis.unregistered !== undefined");
    assert_eq!(eval(&thread, page, "unregistered"), "true");
    eval(&thread, page, "navigator.serviceWorker.getRegistrations().then((l) => globalThis.after = l.length); 'ok'");
    until(&mut thread, page, "globalThis.after !== undefined");
    assert_eq!(eval(&thread, page, "after"), "0");
}

/// What msw's `setupWorker().start()` does with the worker, in page script:
/// register, wait for activation, MOCK_ACTIVATE, then answer each REQUEST on
/// the transferred port with MOCK_RESPONSE (or PASSTHROUGH).
const MSW_CLIENT: &str = r#"
globalThis.log = [];
const sw = navigator.serviceWorker;
sw.addEventListener('message', (event) => {
  const { type, payload } = event.data;
  log.push(type);
  if (type === 'MOCKING_ENABLED') globalThis.enabled = payload.client.frameType;
  if (type === 'INTEGRITY_CHECK_RESPONSE') globalThis.checksum = payload.checksum;
  if (type === 'REQUEST') {
    const port = event.ports[0];
    if (payload.url.endsWith('/api/passthrough')) { port.postMessage({ type: 'PASSTHROUGH' }); return; }
    const body = new TextEncoder().encode(JSON.stringify({ mocked: true, url: payload.url, method: payload.method })).buffer;
    port.postMessage({ type: 'MOCK_RESPONSE', data: { status: 200, statusText: 'OK', headers: { 'content-type': 'application/json', 'x-msw': '1' }, body } }, [body]);
  }
});
sw.register('/mockServiceWorker.js').then(async (registration) => {
  const worker = registration.installing || registration.waiting || registration.active;
  if (worker.state !== 'activated') {
    await new Promise((resolve) => worker.addEventListener('statechange', () => worker.state === 'activated' && resolve()));
  }
  globalThis.worker = worker;
  worker.postMessage('INTEGRITY_CHECK_REQUEST');
  worker.postMessage('MOCK_ACTIVATE');
}, (e) => log.push('error:' + e.message));
"#;

#[test]
fn msw_mock_service_worker_mocks_fetch() {
    let (server, mut thread) = setup();
    let page = open(&mut thread, &server, "/page");
    eval(&thread, page, MSW_CLIENT);
    until(&mut thread, page, "globalThis.enabled !== undefined && globalThis.checksum !== undefined");
    assert_eq!(eval(&thread, page, "enabled"), "top-level");
    assert_eq!(eval(&thread, page, "checksum"), "03cb67ac84128e63d7cd722a6e5b7f1e");
    assert_eq!(eval(&thread, page, "String(navigator.serviceWorker.controller === worker)"), "true");
    eval(
        &thread,
        page,
        "fetch('/api/user?id=1').then((r) => r.json().then((j) => globalThis.user = [r.status, r.headers.get('x-msw'), j.mocked, j.method, j.url].join('|'))); 'ok'",
    );
    until(&mut thread, page, "globalThis.user !== undefined");
    assert_eq!(eval(&thread, page, "user"), format!("200|1|true|GET|{}/api/user?id=1", server.origin));
    assert_eq!(server.hits("/api/user?id=1"), 0);
    eval(&thread, page, "fetch('/api/passthrough').then((r) => r.text()).then((t) => globalThis.pass = t); 'ok'");
    until(&mut thread, page, "globalThis.pass !== undefined");
    assert_eq!(eval(&thread, page, "pass"), "from network");
    assert_eq!(server.hits("/api/passthrough"), 1);
    assert!(eval(&thread, page, "log.join()").contains("REQUEST"));
}

#[test]
fn message_ports_and_streams_cross_to_a_dedicated_worker() {
    let (server, mut thread) = setup();
    let page = open(&mut thread, &server, "/page");
    eval(
        &thread,
        page,
        "globalThis.log = [];
         const w = new Worker('/ports-worker.js');
         const channel = new MessageChannel();
         channel.port1.onmessage = (e) => {
           log.push(e.data);
           if (e.data === 'ready') channel.port1.postMessage('ping');
         };
         w.postMessage({ kind: 'port', port: channel.port2 }, [channel.port2]);
         globalThis.detachedPost = (() => { try { w.postMessage(1, [channel.port2]); return 'ok'; } catch (e) { return e.name; } })();
         const body = new ReadableStream({ start(c) { c.enqueue(new TextEncoder().encode('hello')); c.close(); } });
         w.onmessage = (e) => new Response(e.data.stream).text().then((t) => log.push(t));
         w.postMessage({ kind: 'stream', stream: body }, [body]);
         globalThis.locked = body.locked; 'ok'",
    );
    until(&mut thread, page, "log.length === 3");
    let mut log: Vec<String> = eval(&thread, page, "log.join('|')").split('|').map(str::to_owned).collect();
    log.sort();
    assert_eq!(log, ["back:hello", "pong:ping", "ready"]);
    assert_eq!(eval(&thread, page, "detachedPost + ':' + locked"), "DataCloneError:true");
}
