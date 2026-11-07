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
  } else if (url.pathname === '/client-url') {
    e.respondWith(self.clients.get(e.clientId).then((c) => new Response(c ? c.url : 'none')));
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
self.addEventListener('message', async (e) => {
  if (e.data === 'flood') {
    // More than the upward message budget, faster than the page takes it.
    for (let i = 0; i < 80; i++) e.source.postMessage(new ArrayBuffer(1 << 20));
    return;
  }
  if (e.data && e.data.kind === 'wire') {
    // A CryptoKey and a WritableStream in, a TransformStream back on the port.
    const { key, writable } = e.data;
    const raw = await crypto.subtle.exportKey('raw', key);
    const writer = writable.getWriter();
    await writer.write('sw wrote');
    await writer.close();
    const upper = new TransformStream({ transform(chunk, c) { c.enqueue(String(chunk).toUpperCase()); } });
    e.ports[0].postMessage({ key, bytes: raw.byteLength, upper }, [upper]);
    return;
  }
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
                    // Answered later, on its own thread: the page may navigate meanwhile.
                    "/slow-sw.js" => {
                        thread::spawn(move || {
                            thread::sleep(Duration::from_millis(500));
                            respond(&mut stream, "200 OK", &[("Content-Type", "text/javascript")], SW.as_bytes());
                        });
                    }
                    // A new script on every fetch, which never skips waiting.
                    "/wait.js" => {
                        let version = seen.lock().unwrap().iter().filter(|p| *p == "/wait.js").count();
                        let body = format!(
                            "self.addEventListener('fetch', (e) => {{ if (new URL(e.request.url).pathname === '/version') e.respondWith(new Response('{version}')); }});"
                        );
                        respond(&mut stream, "200 OK", &js, body.as_bytes());
                    }
                    "/go-sw.js" => respond(&mut stream, "302 Found", &[("Location", "/sub/sw.js")], b""),
                    "/sandboxed" => respond(
                        &mut stream,
                        "200 OK",
                        &[("Content-Type", "text/html")],
                        b"<!doctype html><script>
                          let access; try { access = typeof navigator.serviceWorker; } catch (e) { access = e.name; }
                          fetch('/api/sandboxed').then((r) => r.status, (e) => e.name)
                            .then((status) => parent.postMessage(access + '|' + status, '*'));
                          </script>",
                    ),
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
    // The shared cross-VM format: a CryptoKey, a transferred WritableStream
    // and a port go to the worker; the key and a TransformStream come back.
    eval(
        &thread,
        page,
        "globalThis.written = [];
         var writable = new WritableStream({ write(chunk) { written.push(chunk); } });
         var wireChannel = new MessageChannel();
         wireChannel.port1.onmessage = async (e) => {
           const { key, bytes, upper } = e.data;
           const w = upper.writable.getWriter();
           w.write('abc');
           const { value } = await upper.readable.getReader().read();
           globalThis.wire = [key instanceof CryptoKey, key.algorithm.name, bytes, value].join();
         };
         crypto.subtle.generateKey({ name: 'AES-GCM', length: 128 }, true, ['encrypt'])
           .then((key) => navigator.serviceWorker.controller.postMessage(
             { kind: 'wire', key, writable }, [wireChannel.port2, writable])); 'ok'",
    );
    until(&mut thread, page, "globalThis.wire !== undefined && written.length === 1");
    assert_eq!(eval(&thread, page, "wire + '|' + written.join()"), "true,AES-GCM,16,ABC|sw wrote");
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
         r('redirect', navigator.serviceWorker.register('/go-sw.js', {scope: '/'}));
         'ok'",
    );
    until(&mut thread, page, "Object.keys(results).length === 8");
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
    // A redirected script would escape the max scope of the URL it was registered with.
    assert!(get("redirect").starts_with("TypeError:"), "{}", get("redirect"));
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

/// A call from an isolated world settles in that world (Playwright's storage
/// state restore awaits `getRegistrations()` from its utility world).
#[test]
fn service_worker_promises_settle_in_an_isolated_world() {
    let (server, mut thread) = setup();
    let page = open(&mut thread, &server, "/page");
    let world = thread.browser.isolated_world(page, "utility").unwrap();
    {
        let scope = world.scope().unwrap();
        scope
            .eval(
                "globalThis.out = 'pending';
                 navigator.serviceWorker.getRegistrations().then((r) => out = 'regs:' + r.length, (e) => out = 'error:' + e)",
                None,
                1,
            )
            .unwrap();
    }
    let result = thread.run_until(Some(Instant::now() + Duration::from_secs(10)), |_| {
        let scope = world.scope().unwrap();
        scope.eval("out !== 'pending'", None, 1).is_ok_and(|v| v.to_bool())
    });
    assert_eq!(result, lp_runtime::RunOutcome::Done, "getRegistrations() never settled in the isolated world");
    let scope = world.scope().unwrap();
    assert_eq!(scope.eval("out", None, 1).unwrap().to_string().unwrap(), "regs:0");
}

fn navigate(thread: &mut BrowserThread, server: &Server, page: PageId, path: &str) {
    thread.browser.navigate(page, NavigateOpts::address_bar(format!("{}{path}", server.origin))).unwrap();
    until(thread, page, &format!("location.pathname === '{path}' && document.readyState === 'complete'"));
}

#[test]
fn sandboxed_frame_is_not_a_service_worker_client() {
    let (server, mut thread) = setup();
    let page = open(&mut thread, &server, "/page");
    register(&mut thread, page);
    // In scope of the active worker, but its origin is opaque: no access, not controlled.
    eval(
        &thread,
        page,
        "addEventListener('message', (e) => globalThis.sandboxed = e.data);
         const f = document.createElement('iframe');
         f.setAttribute('sandbox', 'allow-scripts');
         f.src = '/sandboxed';
         document.body.append(f); 'ok'",
    );
    until(&mut thread, page, "globalThis.sandboxed !== undefined");
    let result = eval(&thread, page, "sandboxed");
    let (access, status) = result.split_once('|').unwrap();
    assert_eq!(access, "SecurityError");
    assert_ne!(status, "201", "the worker answered the sandboxed frame's fetch");
    eval(&thread, page, "navigator.serviceWorker.getRegistrations().then((l) => globalThis.regs = l.length); 'ok'");
    until(&mut thread, page, "globalThis.regs !== undefined");
    assert_eq!(eval(&thread, page, "String(regs)"), "1");
}

#[test]
fn service_worker_keeps_working_after_the_registering_page_navigates() {
    let (server, mut thread) = setup();
    let page = open(&mut thread, &server, "/page");
    register(&mut thread, page);
    // The worker's clients.get() answers for the new document of the same page.
    navigate(&mut thread, &server, page, "/page2");
    assert_eq!(eval(&thread, page, "navigator.serviceWorker.controller.state"), "activated");
    eval(&thread, page, "fetch('/client-url').then((r) => r.text()).then((t) => globalThis.client = t); 'ok'");
    until(&mut thread, page, "globalThis.client !== undefined");
    assert_eq!(eval(&thread, page, "client"), format!("{}/page2", server.origin));
    // Client.postMessage reaches it too, past the upward message budget.
    eval(
        &thread,
        page,
        "globalThis.received = 0;
         navigator.serviceWorker.onmessage = (e) => { received += e.data.byteLength === 1 << 20 ? 1 : 0; };
         navigator.serviceWorker.controller.postMessage('flood'); 'ok'",
    );
    until(&mut thread, page, "received === 80");
}

#[test]
fn register_survives_a_navigation_during_the_script_fetch() {
    let (server, mut thread) = setup();
    let page = open(&mut thread, &server, "/page");
    eval(&thread, page, "navigator.serviceWorker.register('/slow-sw.js'); 'ok'");
    navigate(&mut thread, &server, page, "/page2");
    eval(
        &thread,
        page,
        "navigator.serviceWorker.register('/slow-sw.js').then((r) => globalThis.done = 'ok:' + r.scope, (e) => globalThis.done = e.name + ':' + e.message); 'ok'",
    );
    until(&mut thread, page, "globalThis.done !== undefined");
    assert_eq!(eval(&thread, page, "done"), format!("ok:{}/", server.origin));
    until(&mut thread, page, "navigator.serviceWorker.controller !== null");
}

#[test]
fn waiting_worker_activates_when_the_last_client_goes() {
    let (server, mut thread) = setup();
    let page = open(&mut thread, &server, "/page");
    eval(
        &thread,
        page,
        "navigator.serviceWorker.register('/wait.js').then((r) => globalThis.reg = r);
         navigator.serviceWorker.ready.then(() => globalThis.ready = true); 'ok'",
    );
    until(&mut thread, page, "globalThis.ready === true && reg.active.state === 'activated'");
    // A new page in scope is controlled by the first version.
    let second = open(&mut thread, &server, "/page2");
    eval(&thread, second, "fetch('/version').then((r) => r.text()).then((t) => globalThis.version = t); 'ok'");
    until(&mut thread, second, "globalThis.version !== undefined");
    assert_eq!(eval(&thread, second, "version"), "1");
    // The second version waits for that page.
    eval(&thread, page, "reg.update(); 'ok'");
    until(&mut thread, page, "reg.waiting !== null && reg.waiting.state === 'installed'");
    assert!(thread.browser.close_page(second));
    until(&mut thread, page, "reg.waiting === null && reg.active !== null && reg.active.state === 'activated'");
    let third = open(&mut thread, &server, "/page3");
    eval(&thread, third, "fetch('/version').then((r) => r.text()).then((t) => globalThis.version = t); 'ok'");
    until(&mut thread, third, "globalThis.version !== undefined");
    assert_eq!(eval(&thread, third, "version"), "2");
}

#[test]
fn a_failed_cross_thread_transfer_consumes_nothing() {
    let (server, mut thread) = setup();
    let page = open(&mut thread, &server, "/page");
    // A getter run by the clone transfers a listed port elsewhere: the post
    // throws, and the buffer and the port listed before it are intact.
    eval(
        &thread,
        page,
        "globalThis.log = [];
         const w = new Worker('/ports-worker.js');
         const x = new MessageChannel(), y = new MessageChannel(), other = new MessageChannel();
         x.port2.onmessage = (e) => log.push(e.data);
         const buf = new ArrayBuffer(8);
         const value = { get sneaky() { other.port1.postMessage(null, [y.port1]); return 1; } };
         let error;
         try { w.postMessage(value, [buf, x.port1, y.port1]); } catch (e) { error = e.name; }
         x.port1.postMessage('still entangled');
         globalThis.result = [error, buf.byteLength].join(':'); 'ok'",
    );
    until(&mut thread, page, "log.length === 1");
    assert_eq!(eval(&thread, page, "result + '|' + log[0]"), "DataCloneError:8|still entangled");
}

#[test]
fn a_full_port_queue_throws_before_the_transfer_list_is_consumed() {
    let (server, mut thread) = setup();
    let page = open(&mut thread, &server, "/page");
    // The worker holds port2 without starting it: port1's posts pile up.
    eval(
        &thread,
        page,
        "const w = new Worker('/ports-worker.js');
         const ch = new MessageChannel(), passenger = new MessageChannel();
         w.postMessage({ kind: 'hold' }, [ch.port2]);
         ch.port1.postMessage(new ArrayBuffer(64 * 1024 * 1024));
         globalThis.log = [];
         passenger.port2.onmessage = (e) => log.push(e.data);
         const buf = new ArrayBuffer(8);
         let error;
         try { ch.port1.postMessage(buf, [buf, passenger.port1]); } catch (e) { error = e.name; }
         passenger.port1.postMessage('kept');
         globalThis.result = [error, buf.byteLength].join(':'); 'ok'",
    );
    until(&mut thread, page, "log.length === 1");
    assert_eq!(eval(&thread, page, "result + '|' + log[0]"), "DataCloneError:8|kept");
}
