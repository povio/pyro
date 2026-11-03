//! `Browser::set_cache_disabled`: a page-scoped HTTP cache bypass (CDP
//! `Network.setCacheDisabled`). The context cache stays shared: a sibling
//! page keeps hitting it while the disabled page goes to the network.
use lp_browser::{BrowserThread, NavigateOpts, PageId};
use lp_net::http::{HeaderMap, HeaderValue};
use lp_webapi::UserAgentOverride;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// A request the server received: path and lower-case headers.
#[derive(Debug, Clone)]
struct Seen {
    path: String,
    headers: Vec<(String, String)>,
}

impl Seen {
    fn get(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }
    fn resource(&self) -> &str {
        self.path.split('?').next().unwrap_or("/")
    }
}

/// `/` is an uncacheable document loading the cacheable `/s.js`; `/data` is
/// cacheable too (`public, max-age=3600`).
struct Server {
    origin: String,
    seen: Arc<Mutex<Vec<Seen>>>,
    thread: Option<JoinHandle<()>>,
}

impl Server {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
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
                let mut headers = Vec::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some((name, value)) = line.trim_end().split_once(':') {
                        headers.push((name.to_ascii_lowercase(), value.trim().to_string()));
                    }
                }
                if path == "/stop" {
                    break;
                }
                let (mime, cache, body) = match path.split('?').next().unwrap_or("/") {
                    "/" => ("text/html", "no-store", "<script src=\"/s.js\"></script><p>doc</p>"),
                    "/s.js" => ("text/javascript", "public, max-age=3600", "globalThis.script = true;"),
                    _ => ("application/json", "public, max-age=3600", "{\"ok\":true}"),
                };
                log.lock().unwrap().push(Seen { path, headers });
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nCache-Control: {cache}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        Self { origin, seen, thread: Some(thread) }
    }

    fn take(&self) -> Vec<Seen> {
        std::mem::take(&mut *self.seen.lock().unwrap())
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Ok(mut stream) = TcpStream::connect(self.origin.trim_start_matches("http://")) {
            let _ = stream.write_all(b"GET /stop HTTP/1.1\r\nHost: localhost\r\n\r\n");
        }
        let _ = self.thread.take().unwrap().join();
    }
}

fn eval(thread: &BrowserThread, page: PageId, script: &str) -> String {
    let realm = thread.browser.page(page).unwrap().main_frame.main_realm().unwrap();
    let scope = realm.scope().unwrap();
    scope.eval(script, None, 1).unwrap_or_else(|e| panic!("{}", e.details(&scope).text())).to_string().unwrap()
}

fn until(thread: &mut BrowserThread, page: PageId, condition: &str) {
    let result = thread.run_until(Some(Instant::now() + Duration::from_secs(5)), |b| {
        let realm = b.page(page).unwrap().main_frame.main_realm().unwrap();
        let scope = realm.scope().unwrap();
        scope.eval(condition, None, 1).unwrap().to_bool()
    });
    assert_eq!(result, lp_runtime::RunOutcome::Done, "waiting for {condition}");
}

/// Loads `/?from=<tag>` (and `/s.js`), then a fetch and an XHR of `/data`.
fn load_and_request(thread: &mut BrowserThread, page: PageId, server: &Server, tag: &str) {
    thread.browser.navigate(page, NavigateOpts::address_bar(format!("{}/?from={tag}", server.origin))).unwrap();
    let loaded = format!(
        "location.search === '?from={tag}' && globalThis.script === true && document.readyState === 'complete'"
    );
    until(thread, page, &loaded);
    eval(
        thread,
        page,
        "globalThis.done = 0;
         fetch('/data').then(r => r.text()).then(() => {
           done++;
           const x = new XMLHttpRequest(); x.open('GET', '/data'); x.onloadend = () => done++; x.send();
         }); 0",
    );
    until(thread, page, "globalThis.done === 2");
}

/// Server requests for the cacheable resources (`/s.js`, `/data`).
fn cacheable(seen: &[Seen]) -> Vec<&Seen> {
    seen.iter().filter(|s| s.resource() != "/").collect()
}

fn setup() -> (Server, BrowserThread) {
    let server = Server::new();
    let mut thread = crate::browser_thread().unwrap();
    thread.enable_network(Default::default()).unwrap();
    (server, thread)
}

#[test]
fn a_disabled_page_bypasses_the_shared_cache_while_its_sibling_hits_it() {
    let (server, mut thread) = setup();
    let a = thread.browser.create_page().unwrap();
    let b = thread.browser.create_page().unwrap();
    // Warm the context cache: /s.js once, /data once (the XHR then hits).
    load_and_request(&mut thread, a, &server, "warm");
    let warm = server.take();
    assert_eq!(cacheable(&warm).len(), 2, "{warm:?}");
    assert!(thread.browser.set_cache_disabled(a, true));
    load_and_request(&mut thread, a, &server, "a-disabled");
    let bypass = server.take();
    let requests = cacheable(&bypass);
    assert_eq!(requests.len(), 3, "script, fetch and XHR go to the network: {bypass:?}");
    for seen in &requests {
        assert_eq!((seen.get("cache-control"), seen.get("pragma")), (Some("no-cache"), Some("no-cache")), "{seen:?}");
    }
    // The sibling page of the same context still hits the cache.
    load_and_request(&mut thread, b, &server, "b");
    let sibling = server.take();
    assert!(cacheable(&sibling).is_empty(), "sibling bypassed the cache: {sibling:?}");
    // Re-enabled: the page hits the cache again.
    assert!(thread.browser.set_cache_disabled(a, false));
    load_and_request(&mut thread, a, &server, "a-enabled");
    let again = server.take();
    assert!(cacheable(&again).is_empty(), "re-enabled page still bypassed: {again:?}");
}

#[test]
fn the_bypass_applies_to_the_live_document_and_survives_other_page_options() {
    let (server, mut thread) = setup();
    let page = thread.browser.create_page().unwrap();
    load_and_request(&mut thread, page, &server, "warm");
    server.take();
    // Set on the live document: its next fetch bypasses the warm entry.
    thread.browser.set_cache_disabled(page, true);
    eval(&thread, page, "globalThis.live = 0; fetch('/data').then(() => live++); 0");
    until(&mut thread, page, "globalThis.live === 1");
    let live = server.take();
    assert_eq!(cacheable(&live).len(), 1, "{live:?}");
    // Changing the extra headers or the user agent keeps the bypass, and vice versa.
    let mut headers = HeaderMap::new();
    headers.insert("x-a", HeaderValue::from_static("1"));
    thread.browser.set_extra_http_headers(page, headers);
    let ua =
        UserAgentOverride { user_agent: "CacheUA/1".into(), accept_language: None, platform: None, metadata: None };
    thread.browser.set_user_agent_override(page, Some(ua)).unwrap();
    load_and_request(&mut thread, page, &server, "after-options");
    let seen = server.take();
    let requests = cacheable(&seen);
    assert_eq!(requests.len(), 3, "{seen:?}");
    for s in &requests {
        assert_eq!(
            (s.get("pragma"), s.get("x-a"), s.get("user-agent")),
            (Some("no-cache"), Some("1"), Some("CacheUA/1")),
            "{s:?}"
        );
    }
    assert!(!thread.browser.set_cache_disabled(PageId(9_999), true), "unknown page");
}
