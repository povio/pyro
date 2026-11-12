//! Referrers end to end: `document.referrer` from the navigation, Referer on
//! script navigations, fetch and subresources under the document's policy
//! (`Referrer-Policy` header, `<meta name=referrer>`), and Fetch Metadata.
use pyro_browser::{BrowserThread, NavigateOpts, PageId};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

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
                // Content negotiation as a dev server does it: raw CSS only for
                // an Accept naming text/css, a JavaScript wrapper otherwise.
                let accepts_css = headers.iter().any(|(n, v)| n == "accept" && v.contains("text/css"));
                let (mime, extra, body) = match path.split('?').next().unwrap_or("/") {
                    "/m.css" if accepts_css => ("text/css", "", "p { color: red }"),
                    "/m.css" => ("text/javascript", "", "export default 'p { color: red }';"),
                    "/a" => ("text/html", "", "<p>a</p>"),
                    "/b" => ("text/html", "Referrer-Policy: bogus, origin\r\n", "<p>b</p>"),
                    "/c" => (
                        "text/html",
                        "",
                        "<head><meta name=referrer content=no-referrer><script src=\"/s.js\"></script></head><p>c</p>",
                    ),
                    "/s.js" => ("text/javascript", "", "globalThis.script = true;"),
                    _ => ("application/json", "", "{\"ok\":true}"),
                };
                log.lock().unwrap().push(Seen { path, headers });
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        Self { origin, seen, thread: Some(thread) }
    }

    fn request(&self, path: &str) -> Seen {
        self.seen.lock().unwrap().iter().rev().find(|s| s.path == path).cloned().unwrap_or_else(|| panic!("{path}"))
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
        let Some(realm) = b.page(page).and_then(|p| p.main_frame.main_realm().cloned()) else { return false };
        let Ok(scope) = realm.scope() else { return false };
        scope.eval(condition, None, 1).is_ok_and(|v| v.to_bool())
    });
    assert_eq!(result, pyro_runtime::RunOutcome::Done, "waiting for {condition}");
}

#[test]
fn document_referrer_policy_and_fetch_metadata() {
    let server = Server::new();
    let mut thread = crate::browser_thread().unwrap();
    thread.enable_network(Default::default()).unwrap();
    let page = thread.browser.create_page().unwrap();
    let origin = server.origin.clone();

    // Browser-initiated: no referrer, Sec-Fetch-Site none.
    thread.browser.navigate(page, NavigateOpts::address_bar(format!("{origin}/a"))).unwrap();
    until(&mut thread, page, "location.pathname === '/a' && document.readyState === 'complete'");
    assert_eq!(eval(&thread, page, "document.referrer"), "");
    let a = server.request("/a");
    assert_eq!((a.get("referer"), a.get("sec-fetch-site"), a.get("sec-fetch-user")), (None, Some("none"), Some("?1")));
    assert_eq!((a.get("sec-fetch-mode"), a.get("sec-fetch-dest")), (Some("navigate"), Some("document")));

    // A script navigation sends the document as its Referer, which becomes
    // the next document's `document.referrer`.
    eval(&thread, page, "location.href = '/b#frag'; 0");
    until(&mut thread, page, "location.pathname === '/b' && document.readyState === 'complete'");
    let a_url = format!("{origin}/a");
    assert_eq!(eval(&thread, page, "document.referrer"), a_url);
    let b = server.request("/b");
    assert_eq!((b.get("referer"), b.get("sec-fetch-site")), (Some(a_url.as_str()), Some("same-origin")));
    assert_eq!(b.get("sec-fetch-user"), None);

    // /b's Referrer-Policy header (origin) applies to its fetches.
    eval(&thread, page, "globalThis.done = false; fetch('/data?b').then(() => done = true); 0");
    until(&mut thread, page, "globalThis.done === true");
    assert_eq!(server.request("/data?b").get("referer"), Some(format!("{origin}/").as_str()));

    // /c's <meta name=referrer content=no-referrer> applies to its subresources.
    eval(&thread, page, "location.assign('/c'); 0");
    until(&mut thread, page, "location.pathname === '/c' && globalThis.script === true");
    // /b's origin policy cut the navigation's referrer to its origin.
    assert_eq!(eval(&thread, page, "document.referrer"), format!("{origin}/"));
    let script = server.request("/s.js");
    assert_eq!(script.get("referer"), None, "{script:?}");
    assert_eq!((script.get("sec-fetch-dest"), script.get("sec-fetch-mode")), (Some("script"), Some("no-cors")));
}

/// CSS and JSON module scripts are fetched with destinations "style" and
/// "json": their Accept and Sec-Fetch-Dest headers say so, and a server that
/// negotiates on Accept returns the stylesheet rather than JavaScript.
#[test]
fn module_script_fetch_destinations() {
    let server = Server::new();
    let mut thread = crate::browser_thread().unwrap();
    thread.enable_network(Default::default()).unwrap();
    let page = thread.browser.create_page().unwrap();
    let origin = server.origin.clone();
    thread.browser.navigate(page, NavigateOpts::address_bar(format!("{origin}/a"))).unwrap();
    until(&mut thread, page, "location.pathname === '/a' && document.readyState === 'complete'");
    eval(
        &thread,
        page,
        "globalThis.out = null; Promise.all([\
           import('/m.css', { with: { type: 'css' } }),\
           import('/m.json', { with: { type: 'json' } })])\
         .then(([css, json]) => out = css.default.cssRules[0].selectorText + ' ' + json.default.ok,\
               e => out = 'error: ' + e); 0",
    );
    until(&mut thread, page, "globalThis.out !== null");
    assert_eq!(eval(&thread, page, "out"), "p true");
    let css = server.request("/m.css");
    assert_eq!(
        (css.get("accept"), css.get("sec-fetch-dest"), css.get("sec-fetch-mode")),
        (Some("text/css,*/*;q=0.1"), Some("style"), Some("cors"))
    );
    let json = server.request("/m.json");
    assert_eq!(
        (json.get("accept"), json.get("sec-fetch-dest"), json.get("sec-fetch-mode")),
        (Some("application/json,*/*;q=0.5"), Some("json"), Some("cors"))
    );
}
