//! Speculative preloading: the requests a document makes after a
//! parser-blocking script start while the script is fetched, the elements
//! take them when the parser gets there, nothing is fetched twice, and
//! scripts still run in document order.

use lp_browser::*;
use lp_js::{ConsoleLevel, EngineHooks, JsException, Realm, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Default)]
struct Recorder {
    log: RefCell<Vec<String>>,
}

impl PageObserver for Recorder {
    fn on_page_event(&self, _page: PageId, event: &PageEvent) {
        if let PageEvent::FrameLoaded { .. } = event {
            self.log.borrow_mut().push("event:load".into());
        }
    }
}

impl EngineHooks for Recorder {
    fn console_api(&self, _realm: &Realm, _level: ConsoleLevel, args: &[Value<'_>]) {
        let parts: Vec<String> = args.iter().map(|a| a.to_string().unwrap_or_default()).collect();
        self.log.borrow_mut().push(parts.join(" "));
    }
    fn exception_thrown(&self, _realm: &Realm, e: &JsException) {
        self.log.borrow_mut().push(format!("exception: {}", e.text()));
    }
}

const ORIGIN: &str = "http://site.test";

fn mime_of(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("js") => "text/javascript",
        Some("css") => "text/css",
        Some("png") => "image/png",
        _ => "text/html",
    }
}

/// A loader whose completions the test releases by hand: which requests
/// are outstanding at once is then observable, and their answer order is
/// the test's.
#[derive(Default)]
struct ManualLoader {
    files: RefCell<HashMap<String, String>>,
    /// Every request, in order.
    requests: RefCell<Vec<String>>,
    /// Requests not answered yet: (URL, completion id).
    pending: RefCell<Vec<(String, u64)>>,
    /// Requests whose transport was cancelled (`fetch_cancellable` handle
    /// dropped), in order.
    cancelled: Rc<RefCell<Vec<String>>>,
}

impl ManualLoader {
    fn resource(url: &str, body: &str) -> Resource {
        Resource {
            url: url.to_owned(),
            status: 200,
            mime: mime_of(url).into(),
            charset: None,
            body: body.as_bytes().to_vec(),
            content_disposition: None,
            header_csp: false,
            refresh: None,
            referrer_policy: None,
            timing: None,
            partial: None,
        }
    }

    fn pending_urls(&self) -> Vec<String> {
        self.pending.borrow().iter().map(|(u, _)| u.clone()).collect()
    }

    /// Answers the outstanding request for `path` with its file.
    fn complete(&self, browser: &mut Browser, path: &str) {
        let url = format!("{ORIGIN}{path}");
        let at = self.pending.borrow().iter().position(|(u, _)| *u == url).unwrap_or_else(|| {
            panic!("{path} is not pending; pending: {:?}, requested: {:?}", self.pending_urls(), self.requests.borrow())
        });
        let (_, id) = self.pending.borrow_mut().remove(at);
        let body = self.files.borrow().get(&url).cloned();
        let result = body.map(|b| Self::resource(&url, &b)).ok_or_else(|| format!("net::ERR_FILE_NOT_FOUND {url}"));
        browser.complete_fetch(id, result);
    }

    fn count(&self, path: &str) -> usize {
        let url = format!("{ORIGIN}{path}");
        self.requests.borrow().iter().filter(|u| **u == url).count()
    }
}

impl ResourceLoader for ManualLoader {
    fn fetch(&self, browser: &mut Browser, request: FetchRequest, done: FetchDone) {
        self.requests.borrow_mut().push(request.url.clone());
        let id = browser.register_completion(done);
        if request.kind == ResourceKind::Document {
            // The document itself arrives on its own.
            let body = self.files.borrow().get(&request.url).cloned();
            let url = request.url;
            browser.scheduler().post(lp_runtime::TaskSource::Networking, move |b| {
                let result =
                    body.map(|b| Self::resource(&url, &b)).ok_or_else(|| format!("net::ERR_FILE_NOT_FOUND {url}"));
                b.complete_fetch(id, result);
            });
        } else {
            self.pending.borrow_mut().push((request.url, id));
        }
    }

    fn fetch_cancellable(
        &self,
        browser: &mut Browser,
        request: FetchRequest,
        done: FetchDone,
    ) -> Option<ResourceCancellation> {
        let url = request.url.clone();
        self.fetch(browser, request, done);
        let cancelled = self.cancelled.clone();
        Some(ResourceCancellation::new(move || cancelled.borrow_mut().push(url)))
    }
}

struct Manual {
    thread: BrowserThread,
    page: PageId,
    rec: Rc<Recorder>,
    loader: Rc<ManualLoader>,
}

impl Manual {
    fn start(html: &str, files: &[(&str, &str)]) -> Manual {
        let mut thread = crate::browser_thread().unwrap();
        let rec = Rc::new(Recorder::default());
        thread.browser.set_observer(Some(rec.clone()));
        thread.browser.set_engine_hooks(Some(rec.clone()));
        let loader = Rc::new(ManualLoader::default());
        loader.files.borrow_mut().insert(format!("{ORIGIN}/"), html.to_owned());
        for (path, body) in files {
            loader.files.borrow_mut().insert(format!("{ORIGIN}{path}"), (*body).to_owned());
        }
        thread.browser.set_loader(Some(loader.clone()));
        let page = thread.browser.create_page().unwrap();
        // The initial about:blank document's load event is not the page's.
        rec.log.borrow_mut().clear();
        thread.browser.navigate(page, NavigateOpts::address_bar(format!("{ORIGIN}/"))).unwrap();
        Manual { thread, page, rec, loader }
    }

    /// Runs the loop until `paths` are all outstanding at once.
    fn wait_pending(&mut self, paths: &[&str]) {
        let loader = self.loader.clone();
        let urls: Vec<String> = paths.iter().map(|p| format!("{ORIGIN}{p}")).collect();
        let r = self.thread.run_until(Some(Instant::now() + Duration::from_secs(10)), |_| {
            let pending = loader.pending.borrow();
            urls.iter().all(|u| pending.iter().any(|(p, _)| p == u))
        });
        assert_eq!(
            r,
            lp_runtime::RunOutcome::Done,
            "waiting for {paths:?}; pending {:?}; requested {:?}; log {:?}",
            self.loader.pending_urls(),
            self.loader.requests.borrow(),
            self.rec.log.borrow()
        );
    }

    fn complete(&mut self, path: &str) {
        self.loader.complete(&mut self.thread.browser, path);
    }

    fn wait_load(&mut self) {
        let rec = self.rec.clone();
        let r = self.thread.run_until(Some(Instant::now() + Duration::from_secs(10)), |_| {
            rec.log.borrow().iter().any(|l| l == "event:load")
        });
        assert_eq!(
            r,
            lp_runtime::RunOutcome::Done,
            "load; pending {:?}; requested {:?}; log {:?}",
            self.loader.pending_urls(),
            self.loader.requests.borrow(),
            self.rec.log.borrow()
        );
        self.thread.run_until_idle(Some(Instant::now() + Duration::from_millis(100)));
    }

    fn console(&self) -> Vec<String> {
        self.rec.log.borrow().iter().filter(|l| !l.starts_with("event:")).cloned().collect()
    }
}

fn logs(names: &[&str]) -> Vec<(String, String)> {
    names.iter().map(|n| (format!("/{n}.js"), format!("console.log('{n}')"))).collect()
}

/// Five parser-blocking scripts: all five are requested while the first is
/// still being fetched, and whatever order their responses arrive in, they
/// run in document order, each requested once.
#[test]
fn blocking_scripts_are_requested_ahead_and_run_in_document_order() {
    let files = logs(&["s1", "s2", "s3", "s4", "s5"]);
    let files: Vec<(&str, &str)> = files.iter().map(|(p, b)| (p.as_str(), b.as_str())).collect();
    let mut m = Manual::start(
        r#"<!doctype html><body>
        <script src="/s1.js"></script><p>text</p><script src="/s2.js"></script>
        <script src="/s3.js"></script><script src="/s4.js"></script><script src="/s5.js"></script>
        <script>console.log('inline')</script></body>"#,
        &files,
    );
    m.wait_pending(&["/s1.js", "/s2.js", "/s3.js", "/s4.js", "/s5.js"]);
    assert_eq!(m.console(), Vec::<String>::new(), "nothing runs before s1 arrives");
    // Responses out of order: the later scripts are ready first.
    for path in ["/s5.js", "/s3.js", "/s4.js", "/s2.js"] {
        m.complete(path);
    }
    m.thread.run_until_idle(Some(Instant::now() + Duration::from_millis(100)));
    assert_eq!(m.console(), Vec::<String>::new(), "s1 blocks the parser; the ready ones wait");
    m.complete("/s1.js");
    m.wait_load();
    assert_eq!(m.console(), ["s1", "s2", "s3", "s4", "s5", "inline"]);
    for path in ["/s1.js", "/s2.js", "/s3.js", "/s4.js", "/s5.js"] {
        assert_eq!(m.loader.count(path), 1, "{path} requested once: {:?}", m.loader.requests.borrow());
    }
}

/// A script's `document.write` of an external script: the written script
/// runs right after it, before the scripts the scan requested ahead, and
/// every URL is requested once.
#[test]
fn written_scripts_run_in_order_with_speculated_ones() {
    let mut m = Manual::start(
        r#"<!doctype html><body><script src="/s1.js"></script><script src="/s2.js"></script>
        <script>console.log('inline')</script></body>"#,
        &[
            ("/s1.js", r#"console.log('s1'); document.write('<script src="/w.js"><\/script><p id=w>w</p>')"#),
            ("/w.js", "console.log('w', !!document.currentScript, document.getElementById('w') === null)"),
            ("/s2.js", "console.log('s2', !!document.getElementById('w'))"),
        ],
    );
    m.wait_pending(&["/s1.js", "/s2.js"]);
    m.complete("/s2.js");
    m.complete("/s1.js");
    m.wait_pending(&["/w.js"]);
    m.complete("/w.js");
    m.wait_load();
    assert_eq!(m.console(), ["s1", "w true true", "s2 true", "inline"]);
    for path in ["/s1.js", "/w.js", "/s2.js"] {
        assert_eq!(m.loader.count(path), 1, "{path}: {:?}", m.loader.requests.borrow());
    }
}

/// `<base href>` before the blocking script (in the DOM) and after it (only
/// in scanned markup) both resolve the scanned URLs the way the elements
/// will, so the speculative requests are the ones taken.
#[test]
fn base_href_is_honoured_in_scanned_markup() {
    let mut m = Manual::start(
        r#"<!doctype html><head><base href="/sub/dir/"></head><body>
        <script src="a.js"></script><base href="/ignored/"><script src="b.js"></script>
        <script>console.log('inline')</script></body>"#,
        &[("/sub/dir/a.js", "console.log('a')"), ("/sub/dir/b.js", "console.log('b')")],
    );
    m.wait_pending(&["/sub/dir/a.js", "/sub/dir/b.js"]);
    m.complete("/sub/dir/a.js");
    m.complete("/sub/dir/b.js");
    m.wait_load();
    assert_eq!(m.console(), ["a", "b", "inline"]);
    assert_eq!(m.loader.requests.borrow().len(), 3, "{:?}", m.loader.requests.borrow());
}

/// A `<base href>` the parser has not reached yet, in scanned markup: the
/// first one resolves what follows it (the second is ignored, as the
/// element will be).
#[test]
fn base_href_in_scanned_markup_resolves_later_urls() {
    let mut m = Manual::start(
        r#"<!doctype html><body><script src="/a.js"></script><base href="/late/"><base href="/second/">
        <script src="b.js"></script><script>console.log('inline')</script></body>"#,
        &[("/a.js", "console.log('a')"), ("/late/b.js", "console.log('b')")],
    );
    m.wait_pending(&["/a.js", "/late/b.js"]);
    m.complete("/late/b.js");
    m.complete("/a.js");
    m.wait_load();
    assert_eq!(m.console(), ["a", "b", "inline"]);
    assert_eq!(m.loader.requests.borrow().len(), 3, "{:?}", m.loader.requests.borrow());
}

/// Nonce, `crossorigin`, `type=module`, `async` and `defer` scripts after
/// a blocking one: each is requested once (the attributes that shape the
/// request are part of the match) and runs.
#[test]
fn script_attributes_match_the_speculative_request() {
    let mut m = Manual::start(
        r#"<!doctype html><body><script src="/s1.js"></script>
        <script nonce="n0nce" src="/nonce.js"></script>
        <script crossorigin="anonymous" src="/cors.js"></script>
        <script type="module" src="/mod.js"></script>
        <script async src="/async.js"></script>
        <script defer src="/defer.js"></script>
        <script type="text/plain" src="/data.js"></script>
        <script nomodule src="/nomodule.js"></script>
        <script>console.log('inline')</script></body>"#,
        &[
            ("/s1.js", "console.log('s1')"),
            ("/nonce.js", "console.log('nonce', document.currentScript.nonce)"),
            ("/cors.js", "console.log('cors')"),
            ("/mod.js", "console.log('mod', import.meta.url.endsWith('/mod.js'))"),
            ("/async.js", "console.log('async')"),
            ("/defer.js", "console.log('defer')"),
        ],
    );
    m.wait_pending(&["/s1.js", "/nonce.js", "/cors.js", "/mod.js", "/async.js", "/defer.js"]);
    for path in ["/s1.js", "/nonce.js", "/cors.js", "/mod.js", "/async.js", "/defer.js"] {
        m.complete(path);
    }
    m.wait_load();
    let console = m.console();
    assert_eq!(console[..3], ["s1", "nonce n0nce", "cors"], "{console:?}");
    assert!(console.contains(&"inline".to_string()) && console.contains(&"async".to_string()), "{console:?}");
    let pos = |s: &str| console.iter().position(|l| l == s).unwrap_or_else(|| panic!("{s} in {console:?}"));
    // Deferred scripts (the module among them) run in document order after the parser is done.
    assert!(pos("inline") < pos("mod true") && pos("mod true") < pos("defer"), "{console:?}");
    for path in ["/s1.js", "/nonce.js", "/cors.js", "/mod.js", "/async.js", "/defer.js"] {
        assert_eq!(m.loader.count(path), 1, "{path}: {:?}", m.loader.requests.borrow());
    }
    for path in ["/data.js", "/nomodule.js"] {
        assert_eq!(m.loader.count(path), 0, "{path}: {:?}", m.loader.requests.borrow());
    }
}

/// Stylesheets, images and preload links after the blocking script are
/// requested ahead and taken by their elements; markup the parser never
/// sees as tags (comments, script text, noscript, template) requests
/// nothing.
#[test]
fn other_resources_are_speculated_and_skipped_markup_is_not() {
    let mut m = Manual::start(
        r#"<!doctype html><head><script src="/s1.js"></script>
        <link rel="stylesheet" href="/a.css"><link rel="preload" as="script" href="/p.js">
        <link rel="stylesheet" href="/off.css" disabled>
        <!-- <script src="/comment.js"></script> -->
        <script>var markup = '<script src="/text.js"><\/script><img src="/text.png">';</script>
        <noscript><img src="/noscript.png"><link rel="stylesheet" href="/noscript.css"></noscript>
        <template><script src="/template.js"></script><img src="/template.png"></template>
        </head><body><img src="/i.png"><picture><source srcset="/pic.png"><img src="/fallback.png"></picture>
        <img src="/set.png" srcset="/set2x.png 2x">
        <script>console.log('inline')</script></body>"#,
        &[("/s1.js", "console.log('s1')"), ("/a.css", "body { color: red }"), ("/p.js", "")],
    );
    m.wait_pending(&["/s1.js", "/a.css", "/p.js", "/i.png"]);
    for path in [
        "/comment.js",
        "/text.js",
        "/text.png",
        "/noscript.png",
        "/noscript.css",
        "/template.js",
        "/template.png",
        "/off.css",
        "/pic.png",
        "/fallback.png",
        "/set.png",
        "/set2x.png",
    ] {
        assert_eq!(m.loader.count(path), 0, "{path} is not a request: {:?}", m.loader.requests.borrow());
    }
    m.complete("/s1.js");
    m.complete("/a.css");
    m.complete("/p.js");
    m.complete("/i.png");
    // The picture's and the srcset image's sources are the elements' to pick
    // (the 1x candidate at this scale factor).
    m.wait_pending(&["/pic.png", "/set.png"]);
    m.complete("/pic.png");
    m.complete("/set.png");
    m.wait_load();
    assert_eq!(m.console(), ["s1", "inline"]);
    assert_eq!(m.loader.count("/set2x.png"), 0);
    for path in ["/s1.js", "/a.css", "/p.js", "/i.png", "/pic.png", "/set.png"] {
        assert_eq!(m.loader.count(path), 1, "{path}: {:?}", m.loader.requests.borrow());
    }
}

/// A parser-blocking external script that waits for stylesheets is
/// fetched while they load, as are the resources after it.
#[test]
fn stylesheet_blocked_script_is_fetched_with_the_stylesheet() {
    let mut m = Manual::start(
        r#"<!doctype html><head><link rel="stylesheet" href="/slow.css"><script src="/a.js"></script>
        <script src="/b.js"></script></head><body><script>console.log('inline', getComputedStyle(document.body).color)</script></body>"#,
        &[("/slow.css", "body { color: rgb(1, 2, 3) }"), ("/a.js", "console.log('a')"), ("/b.js", "console.log('b')")],
    );
    m.wait_pending(&["/slow.css", "/a.js", "/b.js"]);
    m.complete("/a.js");
    m.complete("/b.js");
    m.thread.run_until_idle(Some(Instant::now() + Duration::from_millis(100)));
    assert_eq!(m.console(), Vec::<String>::new(), "a waits for the stylesheet");
    m.complete("/slow.css");
    m.wait_load();
    assert_eq!(m.console(), ["a", "b", "inline rgb(1, 2, 3)"]);
    for path in ["/slow.css", "/a.js", "/b.js"] {
        assert_eq!(m.loader.count(path), 1, "{path}: {:?}", m.loader.requests.borrow());
    }
}

/// A script that comments the rest of the document out: the scanned
/// script after it was requested in vain (once) and never runs; the
/// document still loads.
#[test]
fn speculation_invalidated_by_a_write_is_harmless() {
    let mut m = Manual::start(
        r#"<!doctype html><body><script src="/s1.js"></script><script src="/s2.js"></script></body>"#,
        &[("/s1.js", "console.log('s1'); document.write('<!--')"), ("/s2.js", "console.log('s2')")],
    );
    m.wait_pending(&["/s1.js", "/s2.js"]);
    m.complete("/s1.js");
    m.wait_load();
    assert_eq!(m.console(), ["s1"]);
    assert_eq!(m.loader.count("/s2.js"), 1);
    assert_eq!(*m.loader.cancelled.borrow(), [format!("{ORIGIN}/s2.js")], "unclaimed at end of parsing");
    // Parsing ended: the unclaimed response (or its cancellation) is dropped
    // without effect.
    m.complete("/s2.js");
    m.thread.run_until_idle(Some(Instant::now() + Duration::from_millis(100)));
    assert_eq!(m.console(), ["s1"]);
}

/// Navigating away while the parser is blocked cancels the speculative
/// requests with the document; their late answers do nothing, and the next
/// document loads (and speculates) on its own.
#[test]
fn navigation_cancels_speculative_requests() {
    let mut m = Manual::start(
        r#"<!doctype html><body><script src="/s1.js"></script><script src="/s2.js"></script>
        <link rel="stylesheet" href="/a.css"><img src="/i.png"></body>"#,
        &[
            ("/s1.js", "console.log('s1')"),
            ("/s2.js", "console.log('s2')"),
            ("/next.html", r#"<!doctype html><script src="/n1.js"></script><script src="/n2.js"></script>"#),
            ("/n1.js", "console.log('n1')"),
            ("/n2.js", "console.log('n2')"),
        ],
    );
    m.wait_pending(&["/s1.js", "/s2.js", "/a.css", "/i.png"]);
    assert!(m.loader.cancelled.borrow().is_empty());
    m.thread.browser.navigate(m.page, NavigateOpts::address_bar(format!("{ORIGIN}/next.html"))).unwrap();
    m.wait_pending(&["/n1.js", "/n2.js"]);
    let cancelled = m.loader.cancelled.borrow().clone();
    for path in ["/s2.js", "/a.css", "/i.png"] {
        assert!(cancelled.contains(&format!("{ORIGIN}{path}")), "{path} cancelled: {cancelled:?}");
    }
    // The old document's answers arrive late: ignored.
    for path in ["/s1.js", "/s2.js", "/a.css", "/i.png"] {
        m.complete(path);
    }
    m.complete("/n2.js");
    m.complete("/n1.js");
    m.wait_load();
    assert_eq!(m.console(), ["n1", "n2"]);
    for path in ["/n1.js", "/n2.js"] {
        assert_eq!(m.loader.count(path), 1, "{path}: {:?}", m.loader.requests.borrow());
    }
}

/// A `<meta name=referrer>` in unparsed markup stops the scan there: the
/// requests after it are the elements' own, with the policy in force then.
#[test]
fn referrer_meta_stops_the_scan() {
    let mut m = Manual::start(
        r#"<!doctype html><body><script src="/s1.js"></script><meta name="referrer" content="no-referrer">
        <script src="/s2.js"></script><script>console.log('inline')</script></body>"#,
        &[("/s1.js", "console.log('s1')"), ("/s2.js", "console.log('s2')")],
    );
    m.wait_pending(&["/s1.js"]);
    m.thread.run_until_idle(Some(Instant::now() + Duration::from_millis(100)));
    assert_eq!(m.loader.count("/s2.js"), 0, "{:?}", m.loader.requests.borrow());
    m.complete("/s1.js");
    m.wait_pending(&["/s2.js"]);
    m.complete("/s2.js");
    m.wait_load();
    assert_eq!(m.console(), ["s1", "s2", "inline"]);
}

/// The whole path over HTTP with the network loader: a server that counts
/// its requests sees each resource once, and the scripts run in order.
#[test]
fn over_http_each_resource_is_requested_once() {
    let counts: Arc<Mutex<HashMap<String, usize>>> = Arc::default();
    let mut files: Vec<(String, &'static str, String)> = Vec::new();
    let mut html = String::from("<!doctype html><head><link rel=stylesheet href=/a.css></head><body>");
    for i in 0..8 {
        html.push_str(&format!(r#"<script src="/s{i}.js"></script>"#));
        files.push((format!("/s{i}.js"), "text/javascript", format!("console.log('s{i}')")));
    }
    html.push_str(
        r#"<img src="/i.png"><script defer src="/d.js"></script><script>console.log('inline')</script></body>"#,
    );
    files.push(("/a.css".into(), "text/css", "body { color: red }".into()));
    files.push(("/d.js".into(), "text/javascript", "console.log('d')".into()));
    files.push(("/i.png".into(), "image/png", "not a png".into()));
    files.push(("/".into(), "text/html", html));
    let (origin, _server) = serve(files, counts.clone());
    let mut thread = crate::browser_thread().unwrap();
    let rec = Rc::new(Recorder::default());
    thread.browser.set_observer(Some(rec.clone()));
    thread.browser.set_engine_hooks(Some(rec.clone()));
    let client = lp_net::NetClient::new(lp_net::NetConfig::default()).unwrap();
    thread.browser.set_loader(Some(Rc::new(NetLoader::new(client))));
    let page = thread.browser.create_page().unwrap();
    rec.log.borrow_mut().clear();
    thread.browser.navigate(page, NavigateOpts::address_bar(format!("{origin}/"))).unwrap();
    let r = thread.run_until(Some(Instant::now() + Duration::from_secs(20)), |_| {
        rec.log.borrow().iter().any(|l| l == "event:load")
    });
    assert_eq!(r, lp_runtime::RunOutcome::Done, "{:?}", rec.log.borrow());
    thread.run_until_idle(Some(Instant::now() + Duration::from_millis(200)));
    let console: Vec<String> = rec.log.borrow().iter().filter(|l| !l.starts_with("event:")).cloned().collect();
    let counts = counts.lock().unwrap();
    assert_eq!(
        console,
        ["s0", "s1", "s2", "s3", "s4", "s5", "s6", "s7", "inline", "d"],
        "{counts:?} {:?}",
        rec.log.borrow()
    );
    let mut paths: Vec<String> = ["/", "/a.css", "/i.png", "/d.js"].iter().map(|p| p.to_string()).collect();
    paths.extend((0..8).map(|i| format!("/s{i}.js")));
    for path in &paths {
        assert_eq!(counts.get(path).copied().unwrap_or(0), 1, "{path}: {counts:?}");
    }
}

/// Minimal HTTP/1.1 server on 127.0.0.1 serving `files` (one thread per
/// connection), counting requests per path.
fn serve(
    files: Vec<(String, &'static str, String)>,
    counts: Arc<Mutex<HashMap<String, usize>>>,
) -> (String, std::thread::JoinHandle<()>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = format!("http://{}", listener.local_addr().unwrap());
    let files = Arc::new(files);
    let h = std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let (files, counts) = (files.clone(), counts.clone());
            std::thread::spawn(move || loop {
                let mut buf = Vec::new();
                let mut byte = [0u8; 1];
                while !buf.ends_with(b"\r\n\r\n") {
                    match stream.read(&mut byte) {
                        Ok(1) => buf.push(byte[0]),
                        _ => return,
                    }
                }
                let req = String::from_utf8_lossy(&buf);
                let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
                *counts.lock().unwrap().entry(path.clone()).or_default() += 1;
                let (status, mime, body) = match files.iter().find(|f| f.0 == path) {
                    Some((_, m, b)) => ("200 OK", *m, b.clone()),
                    None => ("404 Not Found", "text/plain", "nope".to_string()),
                };
                // Scripts answer slowly enough for the scan to have run.
                if path.ends_with(".js") {
                    std::thread::sleep(Duration::from_millis(20));
                }
                let head = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: {mime}; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\n\r\n",
                    body.len()
                );
                if stream.write_all(head.as_bytes()).and_then(|_| stream.write_all(body.as_bytes())).is_err() {
                    return;
                }
            });
        }
    });
    (addr, h)
}
