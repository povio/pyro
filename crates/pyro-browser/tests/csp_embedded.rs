//! CSP Embedded Enforcement (`<iframe csp>`): the required policy a child frame's navigation
//! snapshots, sends (`Sec-Required-CSP`, across redirects) and checks the response against; the
//! opaque error document of a blocked response; the requirement a committed document keeps for
//! its own frames (fetched, srcdoc and about:blank documents alike).

use pyro_browser::*;
use pyro_js::Realm;
use std::cell::RefCell;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const POLICY: &str = "script-src 'none'";

fn eval(realm: &Realm, source: &str) -> String {
    let scope = realm.scope().unwrap();
    scope
        .eval(source, Some("csp-embedded-test.js"), 1)
        .unwrap_or_else(|e| panic!("{}: {}", source, e.details(&scope).text()))
        .to_string()
        .unwrap()
}

fn main_realm(thread: &BrowserThread, page: PageId) -> Realm {
    thread.browser.page(page).unwrap().main_frame.main_realm().unwrap().clone()
}

fn settle(thread: &mut BrowserThread) {
    thread.run_until_idle(Some(Instant::now() + Duration::from_secs(3)));
}

#[derive(Default)]
struct Events(RefCell<Vec<PageEvent>>);
impl PageObserver for Events {
    fn on_page_event(&self, _: PageId, event: &PageEvent) {
        self.0.borrow_mut().push(event.clone());
    }
}

/// A page at `http://site.test/` whose body is `body`, served by `loader`, loaded and settled.
fn load(loader: &Rc<StaticLoader>, body: &str) -> (BrowserThread, PageId) {
    let mut t = crate::browser_thread().unwrap();
    loader.insert("http://site.test/", "text/html", body, Duration::ZERO);
    t.browser.set_loader(Some(loader.clone()));
    let page = t.browser.create_page().unwrap();
    t.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut t);
    (t, page)
}

fn child_frames(t: &BrowserThread, page: PageId) -> Vec<FrameInfo> {
    t.browser.frames(page).into_iter().filter(|f| f.parent.is_some()).collect()
}

#[test]
fn a_response_whose_policy_subsumes_the_required_one_loads_with_the_header_sent() {
    let loader = Rc::new(StaticLoader::default());
    loader.insert("http://other.test/ok", "text/html", "<p>ok</p>", Duration::ZERO);
    loader.set_csp("http://other.test/ok", "script-src 'none'; style-src 'none'");
    let (t, page) = load(&loader, &format!("<iframe csp=\"{POLICY}\" src='http://other.test/ok'></iframe>"));
    assert_eq!(loader.required_csp_of("http://other.test/ok"), Some(Some(POLICY.to_owned())));
    let children = child_frames(&t, page);
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].url, "http://other.test/ok");
    assert!(children[0].loaded);
}

#[test]
fn no_header_without_a_csp_attribute_or_with_an_invalid_one() {
    let loader = Rc::new(StaticLoader::default());
    for path in ["plain", "comma", "report", "crlf"] {
        loader.insert(&format!("http://other.test/{path}"), "text/html", "<p>x</p>", Duration::ZERO);
    }
    let (t, page) = load(
        &loader,
        "<iframe src='http://other.test/plain'></iframe>\
         <iframe csp=\"script-src 'none', img-src 'none'\" src='http://other.test/comma'></iframe>\
         <iframe csp=\"script-src 'none'; report-uri /r\" src='http://other.test/report'></iframe>\
         <script>const f = document.createElement('iframe'); f.csp = \"style-src\\r\\n'none'\"; \
         f.src = 'http://other.test/crlf'; document.body.appendChild(f);</script>",
    );
    for path in ["plain", "comma", "report", "crlf"] {
        assert_eq!(loader.required_csp_of(&format!("http://other.test/{path}")), Some(None), "{path}");
    }
    // Without a requirement nothing is checked: all four load.
    assert!(child_frames(&t, page).iter().all(|f| f.loaded && f.url.starts_with("http://other.test/")));
}

#[test]
fn only_an_html_iframe_takes_the_csp_attribute() {
    let loader = Rc::new(StaticLoader::default());
    loader.insert("http://other.test/object", "text/html", "<p>object</p>", Duration::ZERO);
    let (t, page) = load(
        &loader,
        &format!("<object type='text/html' csp=\"{POLICY}\" data='http://other.test/object'></object>"),
    );
    assert_eq!(loader.required_csp_of("http://other.test/object"), Some(None));
    let children = child_frames(&t, page);
    let urls: Vec<&str> = children.iter().map(|f| f.url.as_str()).collect();
    assert!(urls.iter().all(|u| *u != ERROR_DOCUMENT_URL), "{urls:?}");
    assert_eq!(eval(&main_realm(&t, page), "'csp' in document.createElement('object')"), "false");
    assert_eq!(eval(&main_realm(&t, page), "'csp' in document.createElement('iframe')"), "true");
}

#[test]
fn a_response_that_neither_accepts_nor_subsumes_commits_an_opaque_error_document_that_loads() {
    let loader = Rc::new(StaticLoader::default());
    let leak = "<script>parent.postMessage('leaked', '*')</script>";
    loader.insert("http://other.test/plain", "text/html", leak, Duration::ZERO);
    loader.insert("http://other.test/elsewhere", "text/html", "<p>x</p>", Duration::ZERO);
    loader.set_allow_csp_from("http://other.test/elsewhere", "http://not-the-parent.test");
    let events = Rc::new(Events::default());
    let mut t = crate::browser_thread().unwrap();
    t.browser.set_observer(Some(events.clone()));
    loader.insert(
        "http://site.test/",
        "text/html",
        &format!(
            "<script>globalThis.loads = 0; globalThis.messages = []; onmessage = e => messages.push(e.data);</script>\
             <iframe id=a csp=\"{POLICY}\" src='http://other.test/plain' onload='loads++'></iframe>\
             <iframe id=b csp=\"{POLICY}\" src='http://other.test/elsewhere' onload='loads++'></iframe>"
        ),
        Duration::ZERO,
    );
    t.browser.set_loader(Some(loader.clone()));
    let page = t.browser.create_page().unwrap();
    t.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    settle(&mut t);
    let children = child_frames(&t, page);
    assert_eq!(children.len(), 2);
    for child in &children {
        assert_eq!(child.url, ERROR_DOCUMENT_URL);
        assert_eq!(child.origin, "null");
        assert!(child.loaded);
    }
    let root = main_realm(&t, page);
    assert_eq!(eval(&root, "loads >= 2"), "true");
    assert_eq!(eval(&root, "messages.length"), "0");
    for id in ["a", "b"] {
        assert_eq!(
            eval(
                &root,
                &format!(
                    "(() => {{ try {{ document.getElementById('{id}').contentWindow.location.href; \
                     return 'readable'; }} catch (e) {{ return e.name; }} }})()"
                )
            ),
            "SecurityError"
        );
        assert_eq!(eval(&root, &format!("document.getElementById('{id}').contentDocument === null")), "true");
    }
    // The two error documents are not same-origin with each other either (each its own opaque
    // origin), and each says which URL it stands for.
    assert!(children.iter().all(|f| f.unreachable_url.as_deref().is_some_and(|u| u.starts_with("http://other.test/"))));
    let a = t.browser.frame(page, &children[0].id).unwrap().main_realm().unwrap().clone();
    assert_eq!(
        eval(&a, "(() => { try { parent.frames[1].document; return 'readable'; } catch (e) { return e.name; } })()"),
        "SecurityError"
    );
    let blocked = events
        .0
        .borrow()
        .iter()
        .filter(|e| matches!(e, PageEvent::FrameNavigateFailed { error, .. } if error == "net::ERR_BLOCKED_BY_CSP"))
        .count();
    assert_eq!(blocked, 2);
}

#[test]
fn allow_csp_from_accepts_the_required_policy_and_the_document_enforces_it() {
    let loader = Rc::new(StaticLoader::default());
    let body = "<script>globalThis.out = 'none'; \
                try { document.createElement('div').innerHTML = '<b>x</b>'; out = 'assigned'; } \
                catch (e) { out = e.name; }</script>";
    for path in ["star", "origin"] {
        loader.insert(&format!("http://other.test/{path}"), "text/html", body, Duration::ZERO);
    }
    loader.set_allow_csp_from("http://other.test/star", "*");
    loader.set_allow_csp_from("http://other.test/origin", "http://site.test");
    let (t, page) = load(
        &loader,
        "<iframe csp=\"require-trusted-types-for 'script'\" src='http://other.test/star'></iframe>\
         <iframe csp=\"require-trusted-types-for 'script'\" src='http://other.test/origin'></iframe>",
    );
    let children = child_frames(&t, page);
    assert_eq!(children.len(), 2);
    for child in &children {
        assert!(child.url.starts_with("http://other.test/"), "{}", child.url);
        let realm = t.browser.frame(page, &child.id).unwrap().main_realm().unwrap().clone();
        assert_eq!(eval(&realm, "out"), "TypeError", "{}", child.url);
    }
}

#[test]
fn an_accepted_required_policy_blocks_what_it_forbids() {
    let loader = Rc::new(StaticLoader::default());
    let body = "<script>globalThis.ran = 'plain'</script>\
                <script nonce=abc>globalThis.nonced = 'yes'</script>";
    for path in ["star", "subsumed"] {
        loader.insert(&format!("http://other.test/{path}"), "text/html", body, Duration::ZERO);
    }
    loader.set_allow_csp_from("http://other.test/star", "*");
    // Its own policy subsumes the requirement: it loads under its own policy alone.
    loader.set_csp("http://other.test/subsumed", "script-src 'nonce-abc' 'unsafe-inline'");
    let (t, page) = load(
        &loader,
        "<iframe csp=\"script-src 'nonce-abc'\" src='http://other.test/star'></iframe>\
         <iframe csp=\"script-src 'nonce-abc' 'unsafe-inline'\" src='http://other.test/subsumed'></iframe>",
    );
    let children = child_frames(&t, page);
    assert_eq!(children.len(), 2);
    let realm_of = |url: &str| {
        let child = children.iter().find(|f| f.url == url).unwrap_or_else(|| panic!("{url}"));
        t.browser.frame(page, &child.id).unwrap().main_realm().unwrap().clone()
    };
    let star = realm_of("http://other.test/star");
    assert_eq!(eval(&star, "typeof ran"), "undefined");
    assert_eq!(eval(&star, "nonced"), "yes");
    let subsumed = realm_of("http://other.test/subsumed");
    // A nonce-source turns 'unsafe-inline' off (CSP3): only the nonced script runs here too.
    assert_eq!(eval(&subsumed, "typeof ran"), "undefined");
    assert_eq!(eval(&subsumed, "nonced"), "yes");
}

#[test]
fn a_local_scheme_document_enforces_the_required_policy_besides_what_it_inherits() {
    let loader = Rc::new(StaticLoader::default());
    // The parent's own policy, which its about:srcdoc document inherits.
    loader.set_csp("http://site.test/", "script-src 'nonce-abc' 'nonce-def'");
    let (t, page) = load(
        &loader,
        "<iframe csp=\"script-src 'nonce-abc' 'nonce-xyz'\" srcdoc=\"\
         <script nonce=abc>window.both = 'ran'</script>\
         <script nonce=xyz>window.required_only = 'ran'</script>\
         <script nonce=def>window.inherited_only = 'ran'</script>\"></iframe>",
    );
    let children = child_frames(&t, page);
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].url, "about:srcdoc");
    let realm = t.browser.frame(page, &children[0].id).unwrap().main_realm().unwrap().clone();
    // Each script must satisfy both policies: only the one both allow runs.
    assert_eq!(eval(&realm, "typeof both"), "string");
    assert_eq!(eval(&realm, "typeof required_only"), "undefined");
    assert_eq!(eval(&realm, "typeof inherited_only"), "undefined");
}

#[test]
fn the_required_policy_is_snapshot_when_the_navigation_starts() {
    #[derive(Default)]
    struct Deferred {
        pending: RefCell<Vec<(FetchRequest, FetchDone)>>,
    }
    impl ResourceLoader for Deferred {
        fn fetch(&self, _: &mut Browser, request: FetchRequest, done: FetchDone) {
            self.pending.borrow_mut().push((request, done));
        }
    }
    let loader = Rc::new(Deferred::default());
    let mut t = crate::browser_thread().unwrap();
    t.browser.set_loader(Some(loader.clone()));
    let page = t.browser.create_page().unwrap();
    let root = main_realm(&t, page);
    eval(
        &root,
        &format!(
            "globalThis.f = document.createElement('iframe'); f.csp = \"{POLICY}\"; \
             f.src = 'http://other.test/slow'; document.body.appendChild(f)"
        ),
    );
    assert_eq!(
        t.run_until(Some(Instant::now() + Duration::from_secs(2)), |_| !loader.pending.borrow().is_empty()),
        pyro_runtime::RunOutcome::Done
    );
    // Changing (or dropping) the attribute meanwhile changes nothing for this navigation.
    eval(&root, "f.removeAttribute('csp')");
    let (request, done) = loader.pending.borrow_mut().pop().unwrap();
    assert_eq!(request.required_csp.as_deref(), Some(POLICY));
    done(
        &mut t.browser,
        Ok(Resource {
            url: "http://other.test/slow".into(),
            status: 200,
            mime: "text/html".into(),
            charset: None,
            body: b"<p>no policy</p>".to_vec().into(),
            immutable_local: false,
            content_disposition: None,
            header_csp: None,
            allow_csp_from: None,
            refresh: None,
            content_language: None,
            last_modified: None,
            reporting_endpoints: None,
            dictionary_links: Vec::new(),
            referrer_policy: None,
            origin_agent_cluster: false,
            embedder_policy: pyro_net::EmbedderPolicy::UnsafeNone,
            coop: Vec::new(),
            sent_referrer: None,
            timing: None,
            partial: None,
            nosniff: false,
            opaque: false,
        }),
    );
    settle(&mut t);
    let children = child_frames(&t, page);
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].url, ERROR_DOCUMENT_URL);
    assert_eq!(children[0].origin, "null");
}

#[test]
fn a_document_keeps_its_requirement_for_its_own_frames() {
    let loader = Rc::new(StaticLoader::default());
    let stricter = "script-src 'none'; style-src 'none'";
    loader.insert(
        "http://other.test/child",
        "text/html",
        &format!(
            "<iframe src='http://other.test/inherit'></iframe>\
             <iframe csp=\"{stricter}\" src='http://other.test/stricter'></iframe>\
             <iframe csp=\"img-src *\" src='http://other.test/looser'></iframe>"
        ),
        Duration::ZERO,
    );
    loader.set_allow_csp_from("http://other.test/child", "*");
    for path in ["inherit", "stricter", "looser"] {
        let url = format!("http://other.test/{path}");
        loader.insert(&url, "text/html", "<p>x</p>", Duration::ZERO);
        loader.set_allow_csp_from(&url, "*");
    }
    let (t, page) = load(&loader, &format!("<iframe csp=\"{POLICY}\" src='http://other.test/child'></iframe>"));
    assert_eq!(loader.required_csp_of("http://other.test/child"), Some(Some(POLICY.to_owned())));
    // No attribute: the parent's requirement.
    assert_eq!(loader.required_csp_of("http://other.test/inherit"), Some(Some(POLICY.to_owned())));
    // A stricter attribute (subsumed by the parent's): its own.
    assert_eq!(loader.required_csp_of("http://other.test/stricter"), Some(Some(stricter.to_owned())));
    // A looser one: the parent's.
    assert_eq!(loader.required_csp_of("http://other.test/looser"), Some(Some(POLICY.to_owned())));
    assert_eq!(t.browser.frames(page).len(), 5);
}

#[test]
fn srcdoc_and_about_blank_documents_keep_the_requirement_for_their_frames() {
    let loader = Rc::new(StaticLoader::default());
    for path in ["from-srcdoc", "second", "from-blank"] {
        let url = format!("http://site.test/{path}");
        loader.insert(&url, "text/html", "<p>x</p>", Duration::ZERO);
        loader.set_allow_csp_from(&url, "*");
    }
    let (mut t, page) = load(
        &loader,
        &format!(
            "<iframe csp=\"{POLICY}\" srcdoc=\"<iframe src='http://site.test/from-srcdoc'></iframe>\"></iframe>\
             <iframe id=blank csp=\"{POLICY}\" src='http://site.test/second'></iframe>"
        ),
    );
    // Only the srcdoc document's own frame requests this URL: it inherited the srcdoc's requirement.
    assert_eq!(loader.required_csp_of("http://site.test/from-srcdoc"), Some(Some(POLICY.to_owned())));
    assert_eq!(loader.required_csp_of("http://site.test/second"), Some(Some(POLICY.to_owned())));
    // Navigate the second frame to about:blank (a local commit), then give it a frame.
    let root_id = t.browser.page(page).unwrap().main_frame.id.clone();
    let blank = t
        .browser
        .frames(page)
        .into_iter()
        .find(|f| f.parent.as_ref() == Some(&root_id) && f.url == "http://site.test/second")
        .unwrap();
    t.browser.navigate_frame(page, &blank.id, NavigateOpts::address_bar("about:blank")).unwrap();
    settle(&mut t);
    let root = main_realm(&t, page);
    eval(
        &root,
        "const d = document.getElementById('blank').contentDocument; \
         const f = d.createElement('iframe'); f.src = 'http://site.test/from-blank'; d.body.appendChild(f)",
    );
    settle(&mut t);
    assert_eq!(loader.required_csp_of("http://site.test/from-blank"), Some(Some(POLICY.to_owned())));
}

// ----- over the network ----------------------------------------------------------------------

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
                let (status, extra, body) = match path.as_str() {
                    "/root" => (
                        "200 OK",
                        String::new(),
                        "<iframe id=redirected csp=\"script-src 'none'\" src='/redirect'></iframe>\
                         <iframe id=blocked csp=\"script-src 'none'\" src='/plain'></iframe>",
                    ),
                    "/redirect" => ("302 Found", "Location: /target\r\n".to_string(), ""),
                    "/target" => {
                        ("200 OK", "Content-Security-Policy: script-src 'none'\r\n".to_string(), "<p>target</p>")
                    }
                    _ => ("200 OK", String::new(), "<p>plain</p>"),
                };
                log.lock().unwrap().push(Seen { path, headers });
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Type: text/html\r\n{extra}Content-Length: {}\r\n\
                     Connection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        Self { origin, seen, thread: Some(thread) }
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

#[test]
fn every_redirect_hop_sends_the_required_policy_and_the_final_response_is_judged() {
    let server = Server::new();
    let mut t = crate::browser_thread().unwrap();
    t.enable_network(Default::default()).unwrap();
    let page = t.browser.create_page().unwrap();
    t.browser.navigate(page, NavigateOpts::address_bar(format!("{}/root", server.origin))).unwrap();
    let outcome = t.run_until(Some(Instant::now() + Duration::from_secs(5)), |b| {
        let frames = b.frames(page);
        frames.len() == 3 && frames.iter().all(|f| f.loaded)
    });
    assert_eq!(outcome, pyro_runtime::RunOutcome::Done);
    let seen = server.seen.lock().unwrap().clone();
    for path in ["/redirect", "/target", "/plain"] {
        let request = seen.iter().find(|r| r.path == path).expect(path);
        assert_eq!(request.get("sec-required-csp"), Some("script-src 'none'"), "{path}");
    }
    assert_eq!(seen.iter().find(|r| r.path == "/root").unwrap().get("sec-required-csp"), None);
    let children = child_frames(&t, page);
    // The redirect's target subsumes the requirement; the plain page does not.
    assert!(children.iter().any(|f| f.url == format!("{}/target", server.origin)));
    let blocked = children.iter().find(|f| f.url == ERROR_DOCUMENT_URL).expect("blocked frame");
    assert_eq!(blocked.origin, "null");
}
