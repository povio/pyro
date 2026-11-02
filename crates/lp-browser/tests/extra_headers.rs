//! `Browser::set_extra_http_headers`: page-scoped extra request headers
//! (CDP `Network.setExtraHTTPHeaders`) on the page's document, subresource,
//! fetch and XHR requests, with no leak to sibling pages of the same context.
use lp_browser::{BrowserThread, NavigateOpts, PageId};
use lp_net::http::{HeaderMap, HeaderName, HeaderValue};
use lp_webapi::UserAgentOverride;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// One received request: its path (with the `?from=` tag) and the recorded
/// header values, lower-case names.
#[derive(Debug, Clone)]
struct Seen {
    path: String,
    headers: Vec<(String, String)>,
}

impl Seen {
    fn get(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }
}

/// `/` loads `/s.js` (a subresource); `/s.js` and `/data` are small bodies.
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
                let (mime, body) = match path.split('?').next().unwrap_or("/") {
                    "/" => ("text/html", "<script src=\"/s.js\"></script><p>doc</p>"),
                    "/s.js" => ("text/javascript", "globalThis.script = true;"),
                    _ => ("application/json", "{\"ok\":true}"),
                };
                log.lock().unwrap().push(Seen { path, headers });
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
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

fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
    let mut map = HeaderMap::new();
    for (name, value) in pairs {
        map.insert(HeaderName::from_bytes(name.as_bytes()).unwrap(), HeaderValue::from_str(value).unwrap());
    }
    map
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

/// Loads `/?from=<tag>` (and its `/s.js`), then one fetch and one XHR to
/// `/data?from=<tag>-fetch` / `-xhr`.
fn load_and_request(thread: &mut BrowserThread, page: PageId, server: &Server, tag: &str) {
    thread.browser.navigate(page, NavigateOpts::address_bar(format!("{}/?from={tag}", server.origin))).unwrap();
    // Wait for the new document (the previous one is already complete).
    let loaded = format!(
        "location.search === '?from={tag}' && globalThis.script === true && document.readyState === 'complete'"
    );
    until(thread, page, &loaded);
    eval(
        thread,
        page,
        &format!(
            "globalThis.done = 0;
             fetch('/data?from={tag}-fetch').then(r => r.text()).then(() => done++);
             {{ const x = new XMLHttpRequest(); x.open('GET', '/data?from={tag}-xhr');
                x.onloadend = () => done++; x.send(); }} 0"
        ),
    );
    until(thread, page, "globalThis.done === 2");
}

fn setup() -> (Server, BrowserThread) {
    let server = Server::new();
    let mut thread = crate::browser_thread().unwrap();
    thread.enable_network(Default::default()).unwrap();
    (server, thread)
}

/// `name` as received on every request whose path contains `from=<tag>`
/// (document, subresource script, fetch, XHR).
fn values(seen: &[Seen], tag: &str, name: &str) -> Vec<(String, Option<String>)> {
    seen.iter()
        .filter(|s| s.path.contains(&format!("from={tag}")) || s.path == "/s.js")
        .map(|s| (s.path.clone(), s.get(name).map(str::to_string)))
        .collect()
}

#[test]
fn page_headers_reach_every_request_kind_of_that_page_only() {
    let (server, mut thread) = setup();
    let a = thread.browser.create_page().unwrap();
    let b = thread.browser.create_page().unwrap();
    assert!(thread.browser.set_extra_http_headers(a, headers(&[("x-a", "1"), ("cache-control", "no-cache")])));
    load_and_request(&mut thread, a, &server, "a");
    let seen = server.take();
    let got = values(&seen, "a", "x-a");
    assert_eq!(got.len(), 4, "document, script, fetch, xhr: {seen:?}");
    assert!(got.iter().all(|(_, v)| v.as_deref() == Some("1")), "{got:?}");
    assert!(seen.iter().all(|s| s.get("cache-control") == Some("no-cache")), "{seen:?}");
    // The sibling page in the same context sends none of them.
    load_and_request(&mut thread, b, &server, "b");
    let seen = server.take();
    assert_eq!(seen.len(), 4, "{seen:?}");
    assert!(seen.iter().all(|s| s.get("x-a").is_none() && s.get("cache-control").is_none()), "{seen:?}");
}

#[test]
fn a_new_map_replaces_the_old_one_and_an_empty_map_clears() {
    let (server, mut thread) = setup();
    let page = thread.browser.create_page().unwrap();
    thread.browser.set_extra_http_headers(page, headers(&[("x-a", "1"), ("cache-control", "no-cache")]));
    load_and_request(&mut thread, page, &server, "first");
    server.take();
    // Replaced while the document is live: its fetch/XHR see the new map at once.
    thread.browser.set_extra_http_headers(page, headers(&[("x-b", "2")]));
    eval(&thread, page, "globalThis.again = 0; fetch('/data?from=live').then(() => again++); 0");
    until(&mut thread, page, "globalThis.again === 1");
    let seen = server.take();
    assert_eq!(seen.len(), 1);
    assert_eq!((seen[0].get("x-a"), seen[0].get("cache-control"), seen[0].get("x-b")), (None, None, Some("2")));
    // Replaced for later documents too.
    load_and_request(&mut thread, page, &server, "second");
    let seen = server.take();
    assert!(seen.iter().all(|s| s.get("x-b") == Some("2") && s.get("x-a").is_none()), "{seen:?}");
    // An empty map clears.
    thread.browser.set_extra_http_headers(page, HeaderMap::new());
    load_and_request(&mut thread, page, &server, "cleared");
    let seen = server.take();
    assert_eq!(seen.len(), 4);
    assert!(seen.iter().all(|s| s.get("x-b").is_none()), "{seen:?}");
}

#[test]
fn headers_persist_across_navigation_and_user_agent_changes() {
    let (server, mut thread) = setup();
    let page = thread.browser.create_page().unwrap();
    thread.browser.set_extra_http_headers(page, headers(&[("x-a", "kept")]));
    load_and_request(&mut thread, page, &server, "one");
    server.take();
    // Changing (and clearing) the user agent override keeps the extra headers.
    let ua = UserAgentOverride { user_agent: "ExtraUA/1".into(), accept_language: None, platform: None };
    thread.browser.set_user_agent_override(page, Some(ua)).unwrap();
    load_and_request(&mut thread, page, &server, "two");
    let seen = server.take();
    assert_eq!(seen.len(), 4);
    assert!(seen.iter().all(|s| s.get("x-a") == Some("kept") && s.get("user-agent") == Some("ExtraUA/1")), "{seen:?}");
    thread.browser.set_user_agent_override(page, None).unwrap();
    load_and_request(&mut thread, page, &server, "three");
    let seen = server.take();
    assert!(seen.iter().all(|s| s.get("x-a") == Some("kept") && s.get("user-agent") != Some("ExtraUA/1")), "{seen:?}");
    assert!(!thread.browser.set_extra_http_headers(PageId(9_999), HeaderMap::new()), "unknown page");
}
