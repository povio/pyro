//! Service workers over real HTTP: registration and lifecycle, interception of
//! a controlled page's fetch() and XHR (responses and passthrough), messages
//! with transferred ports both ways, and MSW's real `mockServiceWorker.js`
//! driven with the messages its browser library sends.
use pyro_browser::{BrowserThread, NavigateOpts, PageId};
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
  if (e.data === 'background-fetch') {
    e.source.postMessage({ kind: 'background-fetched', body: await (await fetch('/real')).text() });
    return;
  }
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

/// A worker for `/nav/`: answers the document `/nav/doc` and its script,
/// stylesheet and iframe, records every fetch event, and leaves the rest
/// (`/nav/net`, the image) to the network.
const NAV_SW: &str = r#"
self.addEventListener('install', (e) => e.waitUntil(self.skipWaiting()));
self.addEventListener('activate', (e) => e.waitUntil(self.clients.claim()));
const seen = [];
const html = { 'content-type': 'text/html' };
self.addEventListener('fetch', (e) => {
  const r = e.request;
  const path = new URL(r.url).pathname;
  if (path === '/nav/seen') {
    e.respondWith((async () => {
      const doc = seen.find((s) => s.path === '/nav/doc');
      const client = doc && await self.clients.get(doc.resulting);
      return new Response(JSON.stringify({ seen, client: client ? client.url : null }));
    })());
    return;
  }
  seen.push({ path, mode: r.mode, destination: r.destination, client: e.clientId, resulting: e.resultingClientId });
  if (path === '/nav/doc') {
    e.respondWith(new Response('<!doctype html><title>from sw</title><link rel=stylesheet href="/nav/style.css"><p id=p>doc</p><img src="/nav/img.png"><iframe src="/nav/frame"></iframe><script src="/nav/script.js"></script>', { headers: html }));
  } else if (path === '/nav/script.js') {
    e.respondWith(new Response('window.scripted = "sw script";', { headers: { 'content-type': 'text/javascript' } }));
  } else if (path === '/nav/style.css') {
    e.respondWith(new Response('#p { color: rgb(1, 2, 3) }', { headers: { 'content-type': 'text/css' } }));
  } else if (path === '/nav/frame') {
    e.respondWith(new Response('<!doctype html><p>framed by sw</p>', { headers: html }));
  } else if (path === '/nav/fetched') {
    e.respondWith(fetch(r));
  }
});
"#;

/// A worker that fills a cache while installing and answers from it.
const CACHE_SW: &str = r#"
self.addEventListener('install', (e) => e.waitUntil(caches.open('sw').then((c) => c.addAll(['/real'])).then(() => self.skipWaiting())));
self.addEventListener('activate', (e) => e.waitUntil(self.clients.claim()));
self.addEventListener('fetch', (e) => {
  const path = new URL(e.request.url).pathname;
  if (path === '/cached') {
    e.respondWith(caches.match('/real'));
  } else if (path === '/cache-put') {
    e.respondWith(caches.open('sw').then((c) => c.put('/from-sw', new Response('sw put', { headers: { 'x-sw': '1' } })))
      .then(() => new Response('stored')));
  } else if (path === '/offline-doc') {
    e.respondWith(caches.match('/offline-doc').then((r) => r || fetch(e.request)));
  }
});
"#;

/// A dedicated worker that stores a response in the shared caches.
const CACHES_WORKER: &str = r#"
caches.open('worker').then((c) => c.put('/from-worker', new Response('worker put')))
  .then(() => caches.keys()).then((names) => postMessage(names.join()), (e) => postMessage('error:' + e.name));
"#;

/// A dedicated worker that answers on transferred ports and streams.
/// Counts its connections; each port gets `<name>:<n>` and echoes messages,
/// and a port's `close` request ends the worker.
const SHARED_WORKER: &str = r#"
let connections = 0;
onconnect = (e) => {
  connections++;
  const port = e.ports[0];
  if (e.source !== port || !(self instanceof SharedWorkerGlobalScope)) return;
  port.onmessage = (m) => (m.data === 'close' ? close() : port.postMessage('echo:' + m.data));
  port.postMessage(self.name + ':' + connections);
};
"#;

/// A dedicated or shared worker whose `probe` replaces its [Replaceable]
/// `origin` and `name`, reads the descriptors, deletes and replaces them
/// again and checks a same-origin fetch still goes through; `read` reports
/// both.
const REPLACEABLE_WORKER: &str = r#"
const kind = (k) => {
  for (let o = self; o; o = Object.getPrototypeOf(o)) {
    const d = Object.getOwnPropertyDescriptor(o, k);
    if (d) return (o === self ? 'own-' : 'proto-') + ('get' in d ? 'accessor' : 'data');
  }
  return 'missing';
};
// Another realm's global prototype keeps its own `origin`; this one is per realm.
const owner = (k) => { for (let o = self; o; o = Object.getPrototypeOf(o)) if (Object.hasOwn(o, k)) return o === self ? 'self' : o === Object.getPrototypeOf(self) ? 'global-proto' : 'other'; return 'none'; };
const desc = (k) => { const d = Object.getOwnPropertyDescriptor(self, k); return [d.value, d.writable, d.enumerable, d.configurable].join(':'); };
const probe = async () => {
  const real = [self.origin, self.name];
  const layout = [owner('origin'), owner('name')].join();
  const before = [kind('origin'), kind('name')].join();
  self.origin = 'replaced-origin';
  self.name = 42;
  const after = [desc('origin'), desc('name'), typeof self.name, kind('origin'), kind('name')].join();
  self.origin = 'again';
  const again = self.origin;
  const gone = (k, v) => (kind(k) === 'missing' ? self[k] === undefined : self[k] === v);
  const deleted = [delete self.origin, delete self.name, gone('origin', real[0]), gone('name', real[1]), kind('origin'), kind('name')].join();
  self.origin = 'kept';
  self.name = 'kept-name';
  const body = await (await fetch('/real')).text();
  return [layout, real.join('|'), before, after, again, deleted, body, new URL(location.href).origin === real[0]].join(' / ');
};
const answer = (target, data) => {
  if (data === 'probe') probe().then((r) => target.postMessage(r), (e) => target.postMessage('error:' + e));
  else if (data === 'read') target.postMessage([self.origin, self.name].join('|'));
};
if (typeof SharedWorkerGlobalScope !== 'undefined' && self instanceof SharedWorkerGlobalScope) {
  let connections = 0;
  onconnect = (e) => { connections++; const port = e.ports[0]; port.onmessage = (m) => m.data === 'count' ? port.postMessage(connections) : answer(port, m.data); };
} else {
  onmessage = (m) => answer(self, m.data);
}
"#;

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
                    "/cache-sw.js" => respond(&mut stream, "200 OK", &js, CACHE_SW.as_bytes()),
                    "/caches-worker.js" => respond(&mut stream, "200 OK", &js, CACHES_WORKER.as_bytes()),
                    "/sandboxed-caches" => respond(
                        &mut stream,
                        "200 OK",
                        &[("Content-Type", "text/html")],
                        b"<!doctype html><script>
                          let access; try { access = typeof caches; } catch (e) { access = e.name; }
                          parent.postMessage(access, '*');
                          </script>",
                    ),
                    "/nav-sw.js" => respond(&mut stream, "200 OK", &js, NAV_SW.as_bytes()),
                    p if p.starts_with("/nav/") => respond(
                        &mut stream,
                        "200 OK",
                        &[("Content-Type", "text/html")],
                        b"<!doctype html><title>network</title><p>network</p>",
                    ),
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
                    "/shared-worker.js" => respond(&mut stream, "200 OK", &js, SHARED_WORKER.as_bytes()),
                    "/replaceable-worker.js" => respond(&mut stream, "200 OK", &js, REPLACEABLE_WORKER.as_bytes()),
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
    if result != pyro_runtime::RunOutcome::Done {
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

/// A blob worker uses its creator's lock manager; ending its event loop
/// releases even a lock whose callback returned an unresolved promise.
#[test]
fn terminated_blob_worker_releases_web_lock_to_window() {
    let (server, mut thread) = setup();
    let page = open(&mut thread, &server, "/page");
    eval(&thread, page, r#"
        globalThis.log = [];
        const url = URL.createObjectURL(new Blob([`
            navigator.locks.request('worker-held', lock => {
                postMessage([lock.name, lock.mode, lock instanceof Lock, navigator.locks instanceof LockManager]);
                return new Promise(() => {});
            });
        `], {type: 'text/javascript'}));
        const holder = new Worker(url);
        holder.onmessage = e => {
            log.push(e.data.join(','));
            navigator.locks.request('worker-held', {ifAvailable: true}, lock => {
                log.push(lock === null ? 'busy' : 'available');
            });
        };
        "ok"
    "#);
    until(&mut thread, page, "log.length === 2");
    assert_eq!(eval(&thread, page, "log.join('|')"), "worker-held,exclusive,true,true|busy");
    eval(&thread, page, r#"
        navigator.locks.request('worker-held', lock => log.push('granted:' + lock.name));
        holder.terminate();
        URL.revokeObjectURL(url);
        "ok"
    "#);
    until(&mut thread, page, "log.length === 3");
    assert_eq!(eval(&thread, page, "log[2]"), "granted:worker-held");
}

/// Runs the thread's loop for a moment, so tasks other threads posted run.
fn spin(thread: &mut BrowserThread, page: PageId) {
    eval(thread, page, "globalThis.spun = false; setTimeout(() => { spun = true; }, 30); 'ok'");
    until(thread, page, "spun");
}

/// The serving loop collects when a worker reports its end
/// (`pyro_webapi::take_worker_ended`): a full collection of every live page.
/// Only the workers a closed browser context ended report it. A page that
/// started and terminated 20 dedicated workers while holding 380 MB cost
/// 35.9 G instructions when each of them did, 0.4 G without.
#[test]
fn only_a_closed_contexts_worker_reports_its_end() {
    let (server, mut thread) = setup();
    let keep = open(&mut thread, &server, "/page");
    assert!(!pyro_webapi::take_worker_ended());
    eval(
        &thread,
        keep,
        "globalThis.ready = false; globalThis.w = new Worker('/ports-worker.js');
         const channel = new MessageChannel();
         channel.port1.onmessage = () => { ready = true; };
         w.postMessage({ kind: 'port', port: channel.port2 }, [channel.port2]); 'ok'",
    );
    until(&mut thread, keep, "ready");
    eval(&thread, keep, "w.terminate(); 'ok'");
    assert!(pyro_webapi::join_stopped(Duration::from_secs(5)), "the dedicated worker's thread ends");
    spin(&mut thread, keep);
    assert!(!pyro_webapi::worker_ended(), "a worker its live page terminated is not a close");

    let context = thread.browser.create_context().unwrap();
    let page = thread.browser.create_page_in(context).unwrap();
    thread.browser.navigate(page, NavigateOpts::address_bar(format!("{}/page", server.origin))).unwrap();
    until(&mut thread, page, "location.pathname === '/page' && document.readyState === 'complete'");
    register(&mut thread, page);
    assert!(thread.browser.close_context(context));
    let reported = thread.run_until(Some(Instant::now() + Duration::from_secs(10)), |_| pyro_webapi::worker_ended());
    assert_eq!(reported, pyro_runtime::RunOutcome::Done, "the closed context's service worker reports its end");
    assert!(pyro_webapi::take_worker_ended());
    assert!(!pyro_webapi::worker_ended());
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
    assert_eq!(result, pyro_runtime::RunOutcome::Done, "getRegistrations() never settled in the isolated world");
    let scope = world.scope().unwrap();
    assert_eq!(scope.eval("out", None, 1).unwrap().to_string().unwrap(), "regs:0");
}

fn navigate(thread: &mut BrowserThread, server: &Server, page: PageId, path: &str) {
    thread.browser.navigate(page, NavigateOpts::address_bar(format!("{}{path}", server.origin))).unwrap();
    until(thread, page, &format!("location.pathname === '{path}' && document.readyState === 'complete'"));
}

/// Playwright's `serviceWorkers: 'block'`: Chromium with Playwright's init
/// script resolves `register()` with undefined after a console warning and
/// registers nothing; here the context's switch covers the prototype's method
/// too, and with no worker the page's requests reach the network.
#[test]
fn a_context_that_blocks_service_workers_registers_none() {
    let (server, mut thread) = setup();
    let page = open(&mut thread, &server, "/page");
    let context = thread.browser.page(page).unwrap().context;
    let services = thread.browser.context_services(context).unwrap();
    services.set_service_workers_blocked(true);
    eval(
        &thread,
        page,
        "globalThis.warned = []; console.warn = (m) => warned.push(m);
         const container = navigator.serviceWorker;
         Promise.all([container.register('/sw.js'), ServiceWorkerContainer.prototype.register.call(container, '/sw.js'), container.register()])
           .then(async (r) => globalThis.blocked = [...r.map(String), (await container.getRegistrations()).length,
             String(container.controller), await fetch('/api/blocked').then((r) => r.status), ...warned].join('|')); 'ok'",
    );
    until(&mut thread, page, "globalThis.blocked !== undefined");
    let warning = "Service Worker registration blocked by Playwright";
    assert_eq!(
        eval(&thread, page, "blocked"),
        format!("undefined|undefined|undefined|0|null|404|{warning}|{warning}|{warning}")
    );
    assert_eq!(server.hits("/sw.js"), 0);
    // Lifted, registration works again.
    services.set_service_workers_blocked(false);
    register(&mut thread, page);
    assert_eq!(server.hits("/sw.js"), 1);
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
fn cookie_subscriptions_survive_update_and_allow_cleanup_after_unregister() {
    let (server, mut thread) = setup();
    let page = open(&mut thread, &server, "/page");
    eval(&thread, page, "navigator.serviceWorker.register('/wait.js', {scope:'/worker/'}).then(r => globalThis.reg = r); 'ok'");
    until(&mut thread, page, "globalThis.reg?.active?.state === 'activated'");
    eval(&thread, page, r#"
        globalThis.done = null;
        (async () => {
            const manager = reg.cookies, subscriptions = [{name:'unchanged'}];
            await manager.subscribe(subscriptions);
            await reg.update();
            const worker = await new Promise(resolve => reg.addEventListener('updatefound',
                () => resolve(reg.installing), {once:true}));
            if (worker.state !== 'activated') await new Promise(resolve => worker.addEventListener(
                'statechange', () => { if (worker.state === 'activated') resolve(); }));
            const saved = await manager.getSubscriptions();
            await reg.unregister();
            await manager.unsubscribe(subscriptions);
            return [saved.length, saved[0].name, (await manager.getSubscriptions()).length].join(',');
        })().then(value => done = value, error => done = error.name);
        'ok'
    "#);
    until(&mut thread, page, "done !== null");
    assert_eq!(eval(&thread, page, "done"), "1,unchanged,0");
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

/// Registers /nav-sw.js for /nav/ from `page` and waits for it to be active.
fn register_nav(thread: &mut BrowserThread, page: PageId) {
    eval(thread, page, "navigator.serviceWorker.register('/nav-sw.js', {scope: '/nav/'}).then((r) => globalThis.navReg = r); 'ok'");
    until(thread, page, "globalThis.navReg !== undefined && navReg.active !== null && navReg.active.state === 'activated'");
}

#[test]
fn service_worker_answers_navigations_and_subresources() {
    let (server, mut thread) = setup();
    let page = open(&mut thread, &server, "/page");
    register_nav(&mut thread, page);

    // The document, its stylesheet, script and iframe come from the worker.
    navigate(&mut thread, &server, page, "/nav/doc");
    assert_eq!(eval(&thread, page, "document.title"), "from sw");
    assert_eq!(server.hits("/nav/doc"), 0);
    until(&mut thread, page, "globalThis.scripted === 'sw script'");
    assert_eq!(eval(&thread, page, "getComputedStyle(document.getElementById('p')).color"), "rgb(1, 2, 3)");
    until(&mut thread, page, "document.querySelector('iframe').contentDocument?.body?.textContent === 'framed by sw'");
    assert_eq!(server.hits("/nav/script.js") + server.hits("/nav/style.css") + server.hits("/nav/frame"), 0);
    assert_eq!(eval(&thread, page, "String(navigator.serviceWorker.controller !== null)"), "true");
    assert_eq!(
        eval(&thread, page, "String(document.querySelector('iframe').contentWindow.navigator.serviceWorker.controller !== null)"),
        "true"
    );

    // What the worker saw: modes, destinations and client ids.
    eval(&thread, page, "fetch('/nav/seen').then((r) => r.json()).then((j) => globalThis.seen = j); 'ok'");
    until(&mut thread, page, "globalThis.seen !== undefined && seen.seen.some((s) => s.path === '/nav/img.png')");
    let by = |path: &str, field: &str| {
        eval(&thread, page, &format!("String(seen.seen.find((s) => s.path === '{path}').{field})"))
    };
    assert_eq!(by("/nav/doc", "mode"), "navigate");
    assert_eq!(by("/nav/doc", "destination"), "document");
    assert_eq!(by("/nav/doc", "client"), "");
    let resulting = by("/nav/doc", "resulting");
    assert_eq!(resulting.len(), 36, "{resulting}");
    assert_eq!(eval(&thread, page, "seen.client"), format!("{}/nav/doc", server.origin));
    assert_eq!(by("/nav/script.js", "destination"), "script");
    assert_eq!(by("/nav/script.js", "mode"), "no-cors");
    assert_eq!(by("/nav/script.js", "client"), resulting);
    assert_eq!(by("/nav/style.css", "destination"), "style");
    assert_eq!(by("/nav/style.css", "client"), resulting);
    assert_eq!(by("/nav/img.png", "destination"), "image");
    assert_eq!(by("/nav/frame", "mode"), "navigate");
    assert_eq!(by("/nav/frame", "destination"), "iframe");
    assert_ne!(by("/nav/frame", "resulting"), resulting);
    // The image was not answered: it went to the network.
    assert_eq!(server.hits("/nav/img.png"), 1);

    // A navigation the worker leaves alone, or answers with fetch(), goes to the network.
    navigate(&mut thread, &server, page, "/nav/net");
    assert_eq!(eval(&thread, page, "document.title"), "network");
    assert_eq!(eval(&thread, page, "String(navigator.serviceWorker.controller !== null)"), "true");
    navigate(&mut thread, &server, page, "/nav/fetched");
    assert_eq!(eval(&thread, page, "document.title"), "network");
    assert_eq!(server.hits("/nav/fetched"), 1);
}

#[test]
fn bypassing_service_workers_skips_navigations_and_subresources() {
    let (server, mut thread) = setup();
    let page = open(&mut thread, &server, "/page");
    register_nav(&mut thread, page);
    assert!(thread.browser.set_bypass_service_worker(page, true));
    navigate(&mut thread, &server, page, "/nav/doc");
    assert_eq!(eval(&thread, page, "document.title"), "network");
    assert_eq!(server.hits("/nav/doc"), 1);
    eval(
        &thread,
        page,
        "const s = document.createElement('script'); s.src = '/nav/script.js';
         s.onload = s.onerror = () => globalThis.loaded = String(globalThis.scripted);
         document.head.append(s); 'ok'",
    );
    until(&mut thread, page, "globalThis.loaded !== undefined");
    assert_eq!(eval(&thread, page, "loaded"), "undefined");
    assert_eq!(server.hits("/nav/script.js"), 1);
}

const CACHE_API: &str = r#"
globalThis.out = {};
(async () => {
  const c = await caches.open('v1');
  out.same = (await caches.open('v1')) !== c;
  await c.put('/a?x=1#frag', new Response('A', { headers: { 'content-type': 'text/plain', 'x-h': '1' } }));
  await c.put(new Request('/b', { headers: { accept: 'x' } }), new Response('B', { headers: { vary: 'Accept' } }));
  const r = await c.match('/a?x=1');
  out.text = await r.text() + '|' + r.type + '|' + r.headers.get('x-h') + '|' + r.status + '|' + r.url;
  out.blob = (await (await c.match('/a?x=1')).blob()).type;
  out.noSearch = String(await c.match('/a'));
  out.ignoreSearch = await (await c.match('/a', { ignoreSearch: true })).text();
  out.vary = String(await c.match('/b'));
  out.varyMatch = await (await c.match(new Request('/b', { headers: { accept: 'x' } }))).text();
  out.ignoreVary = await (await c.match('/b', { ignoreVary: true })).text();
  out.post = String(await c.match(new Request('/a?x=1', { method: 'POST' })));
  out.ignoreMethod = await (await c.match(new Request('/a?x=1', { method: 'POST' }), { ignoreMethod: true })).text();
  out.keys = (await c.keys()).map((k) => { const u = new URL(k.url); return u.pathname + u.search; }).join();
  out.keyHeader = (await c.keys('/b', { ignoreVary: true }))[0].headers.get('accept');
  out.all = (await c.matchAll()).length + ':' + (await c.matchAll('/a', { ignoreSearch: true })).length;
  await c.put('/a?x=1', new Response('A2'));
  out.replaced = (await c.matchAll()).length + ':' + await (await c.match('/a?x=1')).text();
  await c.add('/real');
  const network = await caches.match('/real');
  out.added = await network.text() + '|' + network.type + '|' + network.url.endsWith('/real');
  out.addFail = await c.add('/missing').then(() => 'ok', (e) => e.name);
  out.dup = await c.addAll(['/page', '/page']).then(() => 'ok', (e) => e.name);
  out.dupKept = String(await c.match('/page'));
  out.putPost = await c.put(new Request('/p', { method: 'POST', body: 'x' }), new Response('')).then(() => 'ok', (e) => e.name);
  out.putScheme = await c.put('data:text/plain,x', new Response('')).then(() => 'ok', (e) => e.name);
  out.putVary = await c.put('/v', new Response('', { headers: { vary: '*' } })).then(() => 'ok', (e) => e.name);
  const used = new Response('used'); await used.text();
  out.putUsed = await c.put('/u', used).then(() => 'ok', (e) => e.name);
  out.noArgs = await c.match().then(() => 'ok', (e) => e.name);
  out.del = await c.delete('/a?x=1') + ':' + await c.delete('/a?x=1');
  out.names = (await caches.keys()).join();
  await caches.open('v2');
  out.names2 = (await caches.keys()).join();
  out.has = await caches.has('v1') + ':' + await caches.has('nope');
  out.matchName = String(await caches.match('/real', { cacheName: 'nope' })) + ':' + await (await caches.match('/real', { cacheName: 'v1' })).text();
  out.deleted = await caches.delete('v1') + ':' + await caches.delete('v1');
  out.after = (await caches.keys()).join();
  out.orphan = await (await c.match('/real')).text();
  out.ifaces = [typeof Cache, typeof CacheStorage, caches instanceof CacheStorage, c instanceof Cache, caches === caches,
    Object.prototype.toString.call(c)].join();
  out.illegal = (() => { try { new Cache(); return 'ok'; } catch (e) { return e.name; } })();
  out.done = true;
})().catch((e) => { out.error = e.name + ':' + e.message; out.done = true; });
"#;

#[test]
fn cache_storage_api() {
    let (server, mut thread) = setup();
    let page = open(&mut thread, &server, "/page");
    eval(&thread, page, CACHE_API);
    until(&mut thread, page, "out.done === true");
    let get = |k: &str| eval(&thread, page, &format!("String(out.{k})"));
    assert_eq!(get("error"), "undefined");
    assert_eq!(get("same"), "true");
    assert_eq!(get("text"), "A|default|1|200|");
    assert_eq!(get("blob"), "text/plain");
    assert_eq!(get("noSearch"), "undefined");
    assert_eq!(get("ignoreSearch"), "A");
    assert_eq!(get("vary"), "undefined");
    assert_eq!(get("varyMatch"), "B");
    assert_eq!(get("ignoreVary"), "B");
    assert_eq!(get("post"), "undefined");
    assert_eq!(get("ignoreMethod"), "A");
    assert_eq!(get("keys"), "/a?x=1,/b");
    assert_eq!(get("keyHeader"), "x");
    assert_eq!(get("all"), "2:1");
    assert_eq!(get("replaced"), "2:A2");
    assert_eq!(get("added"), "from network|basic|true");
    assert_eq!(server.hits("/real"), 1);
    assert_eq!(get("addFail"), "TypeError");
    assert_eq!(get("dup"), "InvalidStateError");
    assert_eq!(get("dupKept"), "undefined");
    assert_eq!(get("putPost"), "TypeError");
    assert_eq!(get("putScheme"), "TypeError");
    assert_eq!(get("putVary"), "TypeError");
    assert_eq!(get("putUsed"), "TypeError");
    assert_eq!(get("noArgs"), "TypeError");
    assert_eq!(get("del"), "true:false");
    assert_eq!(get("names"), "v1");
    assert_eq!(get("names2"), "v1,v2");
    assert_eq!(get("has"), "true:false");
    assert_eq!(get("matchName"), "undefined:from network");
    assert_eq!(get("deleted"), "true:false");
    assert_eq!(get("after"), "v2");
    assert_eq!(get("orphan"), "from network");
    assert_eq!(get("ifaces"), "function,function,true,true,true,[object Cache]");
    assert_eq!(get("illegal"), "TypeError");

    // The context's documents share the origin's caches, across navigation.
    eval(&thread, page, "caches.open('v2').then((c) => c.put('/kept', new Response('kept'))).then(() => globalThis.kept = 1); 'ok'");
    until(&mut thread, page, "globalThis.kept === 1");
    navigate(&mut thread, &server, page, "/page2");
    eval(&thread, page, "caches.match('/kept').then((r) => r.text()).then((t) => globalThis.kept = t); 'ok'");
    until(&mut thread, page, "globalThis.kept !== undefined");
    assert_eq!(eval(&thread, page, "kept"), "kept");
    // So do the dedicated workers they start.
    eval(&thread, page, "new Worker('/caches-worker.js').onmessage = (e) => globalThis.fromWorker = e.data; 'ok'");
    until(&mut thread, page, "globalThis.fromWorker !== undefined");
    assert_eq!(eval(&thread, page, "fromWorker"), "v2,worker");
    eval(&thread, page, "caches.match('/from-worker').then((r) => r.text()).then((t) => globalThis.workerPut = t); 'ok'");
    until(&mut thread, page, "globalThis.workerPut !== undefined");
    assert_eq!(eval(&thread, page, "workerPut"), "worker put");
    // Another context starts empty.
    let other = thread.browser.create_context().unwrap();
    let isolated = thread.browser.create_page_in(other).unwrap();
    thread.browser.navigate(isolated, NavigateOpts::address_bar(format!("{}/page", server.origin))).unwrap();
    until(&mut thread, isolated, "location.pathname === '/page' && document.readyState === 'complete'");
    eval(&thread, isolated, "caches.keys().then((k) => globalThis.names = k.length); 'ok'");
    until(&mut thread, isolated, "globalThis.names !== undefined");
    assert_eq!(eval(&thread, isolated, "String(names)"), "0");
}

#[test]
fn service_worker_and_page_share_cache_storage() {
    let (server, mut thread) = setup();
    let page = open(&mut thread, &server, "/page");
    eval(&thread, page, "navigator.serviceWorker.register('/cache-sw.js'); 'ok'");
    until(&mut thread, page, "navigator.serviceWorker.controller !== null && navigator.serviceWorker.controller.state === 'activated'");
    // The worker's install filled a cache the page sees.
    eval(&thread, page, "caches.match('/real').then((r) => r.text()).then((t) => globalThis.pre = t); 'ok'");
    until(&mut thread, page, "globalThis.pre !== undefined");
    assert_eq!(eval(&thread, page, "pre"), "from network");
    assert_eq!(server.hits("/real"), 1);
    // The worker answers from it, and stores what the page then reads.
    eval(&thread, page, "fetch('/cached').then((r) => r.text()).then((t) => globalThis.cached = t); 'ok'");
    until(&mut thread, page, "globalThis.cached !== undefined");
    assert_eq!(eval(&thread, page, "cached"), "from network");
    assert_eq!(server.hits("/real"), 1);
    eval(
        &thread,
        page,
        "fetch('/cache-put').then(() => caches.open('sw')).then((c) => c.match('/from-sw'))
           .then((r) => r.text().then((t) => globalThis.swPut = t + '|' + r.headers.get('x-sw'))); 'ok'",
    );
    until(&mut thread, page, "globalThis.swPut !== undefined");
    assert_eq!(eval(&thread, page, "swPut"), "sw put|1");
    // A document the page cached is the worker's answer to its navigation.
    eval(
        &thread,
        page,
        "caches.open('sw').then((c) => c.put('/offline-doc', new Response('<!doctype html><title>offline</title>', { headers: { 'content-type': 'text/html' } })))
           .then(() => globalThis.stored = true); 'ok'",
    );
    until(&mut thread, page, "globalThis.stored === true");
    navigate(&mut thread, &server, page, "/offline-doc");
    assert_eq!(eval(&thread, page, "document.title"), "offline");
    assert_eq!(server.hits("/offline-doc"), 0);
}

#[test]
fn sandboxed_frame_has_no_cache_storage() {
    let (server, mut thread) = setup();
    let page = open(&mut thread, &server, "/page");
    assert_eq!(eval(&thread, page, "typeof caches"), "object");
    eval(
        &thread,
        page,
        "addEventListener('message', (e) => globalThis.sandboxed = e.data);
         const f = document.createElement('iframe');
         f.setAttribute('sandbox', 'allow-scripts');
         f.src = '/sandboxed-caches';
         document.body.append(f); 'ok'",
    );
    until(&mut thread, page, "globalThis.sandboxed !== undefined");
    assert_eq!(eval(&thread, page, "sandboxed"), "SecurityError");
}

#[test]
fn service_worker_background_fetches_are_not_client_document_requests() {
    let (server, mut thread) = setup();
    let page = open(&mut thread, &server, "/page");
    register(&mut thread, page);
    let context = thread.browser.page(page).unwrap().context;
    let events = thread.browser.context_net(context).unwrap().subscribe();
    eval(&thread, page, "globalThis.workerReply=null;navigator.serviceWorker.addEventListener('message',e=>{if(e.data.kind==='background-fetched')globalThis.workerReply=e.data.body});navigator.serviceWorker.controller.postMessage('background-fetch');");
    until(&mut thread, page, "globalThis.workerReply!==null");
    eval(&thread, page, "globalThis.clientReply=null;fetch('/real').then(r=>r.text()).then(v=>globalThis.clientReply=v);");
    until(&mut thread, page, "globalThis.clientReply!==null");
    let mut client_requests = Vec::new();
    let mut worker_requests = Vec::new();
    while let Ok(event) = events.try_recv() {
        if let pyro_net::NetEvent::RequestWillBeSent { request, .. } = event {
            if request.url.path() == "/real" {
                if request.loader_id.is_some() { client_requests.push(request); }
                else { worker_requests.push(request); }
            }
        }
    }
    assert_eq!(client_requests.len(), 1);
    assert_eq!(worker_requests.len(), 1);
    assert!(client_requests[0].frame_id.is_some());
    assert!(worker_requests[0].frame_id.is_none());
    assert_eq!(server.hits("/real"), 2);
}

#[test]
fn service_worker_intercepted_fetch_preserves_client_network_observations() {
    let (server, mut thread) = setup();
    let page = open(&mut thread, &server, "/page");
    register(&mut thread, page);
    let context = thread.browser.page(page).unwrap().context;
    let events = thread.browser.context_net(context).unwrap().subscribe();
    eval(&thread, page, "globalThis.clientDone=false;fetch('/api/observed',{method:'POST',body:'hello'}).then(r=>r.text()).then(()=>globalThis.clientDone=true);");
    until(&mut thread, page, "globalThis.clientDone");
    let mut starts = Vec::new();
    let mut responses = Vec::new();
    let mut finishes = 0;
    while let Ok(event) = events.try_recv() {
        match event {
            pyro_net::NetEvent::RequestWillBeSent { request, .. } if request.url.path() == "/api/observed" => starts.push(request),
            pyro_net::NetEvent::ResponseReceived { head, .. } if head.url.path() == "/api/observed" => responses.push(head),
            pyro_net::NetEvent::LoadingFinished { .. } => finishes += 1,
            _ => {},
        }
    }
    assert_eq!(starts.len(), 1);
    assert!(starts[0].frame_id.is_some());
    assert!(starts[0].loader_id.is_some());
    assert_eq!(starts[0].post_data.as_deref(), Some(b"hello".as_slice()));
    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0].status.as_u16(), 201);
    assert_eq!(finishes, 1);
    assert_eq!(server.hits("/api/observed"), 0);
}

/// `SharedWorker`: one worker per (origin, URL, name) that every document of
/// the browser thread connects to through its own port; a new name is a new
/// worker; `close()` ends it and the next construction starts it again.
#[test]
fn shared_workers_are_shared_between_documents_by_url_and_name() {
    let (server, mut thread) = setup();
    let a = open(&mut thread, &server, "/page");
    let b = open(&mut thread, &server, "/page2");
    let connect = "globalThis.got = []; globalThis.sw = new SharedWorker('/shared-worker.js', 'one'); \
        sw.port.onmessage = (e) => got.push(e.data); 'ok'";
    eval(&thread, a, connect);
    until(&mut thread, a, "got.length === 1");
    eval(&thread, b, connect);
    until(&mut thread, b, "got.length === 1");
    assert_eq!(eval(&thread, a, "got.join()"), "one:1");
    assert_eq!(eval(&thread, b, "got.join() + ':' + (sw instanceof EventTarget) + ':' + (sw.port instanceof MessagePort)"), "one:2:true:true");
    eval(&thread, b, "sw.port.postMessage('hi'); 'ok'");
    until(&mut thread, b, "got.length === 2");
    assert_eq!(eval(&thread, b, "got[1]"), "echo:hi");
    // Another name, another worker.
    eval(&thread, a, "globalThis.other = []; const o = new SharedWorker('/shared-worker.js', {name: 'two'}); o.port.onmessage = (e) => other.push(e.data); 'ok'");
    until(&mut thread, a, "other.length === 1");
    assert_eq!(eval(&thread, a, "other.join()"), "two:1");
    // close() ends it; the next one starts afresh.
    eval(&thread, a, "sw.port.postMessage('close'); 'ok'");
    spin(&mut thread, a);
    spin(&mut thread, a);
    eval(&thread, a, connect);
    until(&mut thread, a, "got.length === 1");
    assert_eq!(eval(&thread, a, "got.join()"), "one:1");
    assert_eq!(
        eval(&thread, a, "(() => { try { new SharedWorker('/shared-worker.js', {type: 'module'}); } catch (e) { return e.name } })()"),
        "NotSupportedError"
    );
}

/// [Replaceable] `origin` (Window and workers) and `name` (dedicated and
/// shared workers): assignment shadows the attribute with an own writable,
/// enumerable, configurable data property of that global only. Window
/// `origin` and worker `name` are the global's own accessors ([Global]), so
/// deleting the replacement leaves no property, as in Chrome; worker `origin`
/// is inherited (WorkerGlobalScope), so deleting it shows the original
/// again. The realm's origin (postMessage's origin,
/// same-origin fetch) and the worker's name (SharedWorker matching) stay.
#[test]
fn origin_and_worker_name_are_replaceable_per_global() {
    let (server, mut thread) = setup();
    let a = open(&mut thread, &server, "/page");
    let b = open(&mut thread, &server, "/page2");
    let origin = server.origin.clone();
    // Window.
    assert_eq!(
        eval(
            &thread,
            a,
            "globalThis.real = self.origin;
             const k = (n) => { for (let o = self; o; o = Object.getPrototypeOf(o)) { const d = Object.getOwnPropertyDescriptor(o, n); if (d) return (o === self ? 'own-' : 'proto-') + ('get' in d ? 'accessor' : 'data'); } return 'missing'; };
             const before = k('origin');
             self.origin = 'replaced';
             const d = Object.getOwnPropertyDescriptor(self, 'origin');
             const after = [d.value, d.writable, d.enumerable, d.configurable, k('origin'), origin].join(':');
             origin = 7;
             const again = typeof origin + ':' + origin;
             const deleted = [delete self.origin, before === 'own-accessor' ? self.origin === undefined : self.origin === real, k('origin')].join(':');
             self.origin = 'kept';
             globalThis.log = [];
             addEventListener('message', (e) => log.push(e.origin));
             postMessage('x', '*');
             [real, before, after, again, deleted].join(' / ')",
        ),
        format!("{origin} / own-accessor / replaced:true:true:true:own-data:replaced / number:7 / true:true:missing")
    );
    until(&mut thread, a, "log.length === 1");
    assert_eq!(eval(&thread, a, "log[0] === real && self.origin"), "kept");
    // Another window keeps its own attribute.
    assert_eq!(
        eval(&thread, b, "[self.origin, 'get' in Object.getOwnPropertyDescriptor(self, 'origin')].join()"),
        format!("{origin},true")
    );

    // Dedicated workers: one replaces, the other (same URL, other name) keeps its own.
    eval(
        &thread,
        a,
        "globalThis.got = { one: [], two: [] };
         globalThis.one = new Worker('/replaceable-worker.js', { name: 'dw-one' });
         globalThis.two = new Worker('/replaceable-worker.js', { name: 'dw-two' });
         one.onmessage = (e) => got.one.push(e.data);
         two.onmessage = (e) => got.two.push(e.data);
         one.postMessage('probe'); 'ok'",
    );
    until(&mut thread, a, "got.one.length === 1");
    let expected = |name: &str| {
        format!(
            "global-proto,self / {origin}|{name} / proto-accessor,own-accessor / \
             replaced-origin:true:true:true,42:true:true:true,number,own-data,own-data / again / \
             true,true,true,true,proto-accessor,missing / from network / true"
        )
    };
    assert_eq!(eval(&thread, a, "got.one[0]"), expected("dw-one"));
    eval(&thread, a, "one.postMessage('read'); two.postMessage('read'); 'ok'");
    until(&mut thread, a, "got.one.length === 2 && got.two.length === 1");
    assert_eq!(eval(&thread, a, "got.one[1] + ' ' + got.two[0]"), format!("kept|kept-name {origin}|dw-two"));
    eval(&thread, a, "one.terminate(); two.terminate(); 'ok'");

    // Shared worker: replacing its name leaves the name it is matched by.
    eval(
        &thread,
        a,
        "globalThis.shared = [];
         globalThis.sh = new SharedWorker('/replaceable-worker.js', 'sn');
         sh.port.onmessage = (e) => shared.push(e.data);
         sh.port.postMessage('probe'); 'ok'",
    );
    until(&mut thread, a, "shared.length === 1");
    assert_eq!(eval(&thread, a, "shared[0]"), expected("sn"));
    eval(
        &thread,
        b,
        "globalThis.shared = [];
         globalThis.sh = new SharedWorker('/replaceable-worker.js', { name: 'sn' });
         sh.port.onmessage = (e) => shared.push(e.data);
         sh.port.postMessage('count'); sh.port.postMessage('read'); 'ok'",
    );
    until(&mut thread, b, "shared.length === 2");
    assert_eq!(eval(&thread, b, "shared.join(' ')"), "2 kept|kept-name");
}
