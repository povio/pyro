//! Speculative preloading: the requests a document makes after a
//! parser-blocking script start while the script is fetched, the elements
//! take them when the parser gets there, nothing is fetched twice, and
//! scripts still run in document order.

use pyro_browser::*;
use pyro_js::{ConsoleLevel, EngineHooks, JsException, Realm, Value};
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
    image_policies: RefCell<Vec<Option<pyro_net::ReferrerPolicy>>>,
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
            body: body.as_bytes().to_vec().into(),
            immutable_local: false,
            content_disposition: None,
            header_csp: None,
            allow_csp_from: None,
            refresh: None,
            content_language: None,
            last_modified: None,
            reporting_endpoints: None, permissions_policy: None,
            connection_allowlist: None,
            link_fetches: Vec::new(),
            integrity_headers: None,
            x_frame_options: None,
            dictionary_links: Vec::new(),
            speculation_rules: Vec::new(),
            no_vary_search: None,
            referrer_policy: None,
            origin_agent_cluster: false, force_load_at_top: false, isolation_policy: 0, ephemeral_jar: false,
            embedder_policy: pyro_net::EmbedderPolicy::UnsafeNone,
            embedder_reporting: Default::default(),
            coop: Vec::new(),
            sent_referrer: None,
            timing: None,
            partial: None,
                nosniff: false, opaque: false,
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
        if request.kind == ResourceKind::Image { self.image_policies.borrow_mut().push(request.referrer_policy); }
        let id = browser.register_completion(done);
        if request.kind == ResourceKind::Document {
            // The document itself arrives on its own.
            let body = self.files.borrow().get(&request.url).cloned();
            let url = request.url;
            browser.scheduler().post(pyro_runtime::TaskSource::Networking, move |b| {
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
            pyro_runtime::RunOutcome::Done,
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
            pyro_runtime::RunOutcome::Done,
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

#[test]
fn image_layout_dispatch_joins_exact_request() {
    let mut manual = Manual::start("<img src='/asset.png'>", &[("/asset.png", "image")]);
    manual.wait_pending(&["/asset.png"]);
    manual.thread.browser.fetch_subresource_options(manual.page, format!("{ORIGIN}/asset.png"), ResourceKind::Image, Default::default(), Box::new(|_, _| {}));
    assert_eq!(manual.loader.count("/asset.png"), 1);
}

#[test]
fn image_dispatch_does_not_claim_a_different_policy() {
    let mut manual = Manual::start("<script src='/wait.js'></script><img src='/asset.png'>", &[("/wait.js", "document.write(\"<img referrerpolicy=no-referrer src='/asset.png'>\")"), ("/asset.png", "image")]);
    manual.wait_pending(&["/wait.js", "/asset.png"]);
    manual.complete("/wait.js");
    manual.complete("/asset.png");
    manual.thread.run_until_idle(Some(Instant::now() + Duration::from_millis(100)));
    assert_eq!(manual.loader.count("/asset.png"), 2);
    assert!(manual.loader.image_policies.borrow().contains(&Some(pyro_net::ReferrerPolicy::NoReferrer)));
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

/// Stylesheets, images (with the `srcset` / `<picture>` candidate the
/// element will pick at density 1) and preload links after the blocking
/// script are requested ahead and taken by their elements; markup the
/// parser never sees as tags (comments, script text, noscript, template)
/// requests nothing.
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
    m.wait_pending(&["/s1.js", "/a.css", "/p.js", "/i.png", "/pic.png", "/set.png"]);
    for path in [
        "/comment.js",
        "/text.js",
        "/text.png",
        "/noscript.png",
        "/noscript.css",
        "/template.js",
        "/template.png",
        "/off.css",
        "/fallback.png",
        "/set2x.png",
    ] {
        assert_eq!(m.loader.count(path), 0, "{path} is not a request: {:?}", m.loader.requests.borrow());
    }
    m.complete("/s1.js");
    m.complete("/a.css");
    m.complete("/p.js");
    m.complete("/i.png");
    // The picture's and the srcset image's 1x candidates were requested by
    // the scan too, and the elements take those requests.
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

/// A `<meta name=referrer>` in unparsed markup does not stop the scan: the
/// requests after it are made ahead with the policy in force there, and
/// the elements take them.
#[test]
fn referrer_meta_applies_to_the_requests_after_it() {
    let mut m = Manual::start(
        r#"<!doctype html><body><script src="/s1.js"></script><meta name="referrer" content="no-referrer">
        <script src="/s2.js"></script><script>console.log('inline')</script></body>"#,
        &[("/s1.js", "console.log('s1')"), ("/s2.js", "console.log('s2')")],
    );
    m.wait_pending(&["/s1.js", "/s2.js"]);
    m.complete("/s1.js");
    m.complete("/s2.js");
    m.wait_load();
    assert_eq!(m.console(), ["s1", "s2", "inline"]);
    assert_eq!(m.loader.count("/s2.js"), 1, "{:?}", m.loader.requests.borrow());
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
    let client = pyro_net::NetClient::new(pyro_net::NetConfig::default()).unwrap();
    thread.browser.set_loader(Some(Rc::new(NetLoader::new(client))));
    let page = thread.browser.create_page().unwrap();
    rec.log.borrow_mut().clear();
    thread.browser.navigate(page, NavigateOpts::address_bar(format!("{origin}/"))).unwrap();
    let r = thread.run_until(Some(Instant::now() + Duration::from_secs(20)), |_| {
        rec.log.borrow().iter().any(|l| l == "event:load")
    });
    assert_eq!(r, pyro_runtime::RunOutcome::Done, "{:?}", rec.log.borrow());
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

// ---------------------------------------------------------------------------
// Review (adversarial) cases.

impl Manual {
    /// Runs until `path` has been requested `n` times (or 3 s pass).
    fn wait_count(&mut self, path: &str, n: usize) -> bool {
        let loader = self.loader.clone();
        let url = format!("{ORIGIN}{path}");
        let r = self.thread.run_until(Some(Instant::now() + Duration::from_secs(3)), |_| {
            loader.requests.borrow().iter().filter(|u| **u == url).count() >= n
        });
        r == pyro_runtime::RunOutcome::Done
    }

    fn try_wait_load(&mut self, secs: u64) -> bool {
        let rec = self.rec.clone();
        let r = self.thread.run_until(Some(Instant::now() + Duration::from_secs(secs)), |_| {
            rec.log.borrow().iter().any(|l| l == "event:load")
        });
        r == pyro_runtime::RunOutcome::Done
    }
}

/// Two `<img>` with the same URL after a blocking script: the first claims
/// the pending speculative fetch, the second must not steal its completion
/// (both load, the load event fires).
#[test]
fn review_duplicate_image_urls_while_speculation_pending() {
    let mut m = Manual::start(
        r#"<!doctype html><body><script src="/s1.js"></script>
        <img id=a src="/i.png" onload="console.log('a load')" onerror="console.log('a error')">
        <img id=b src="/i.png" onload="console.log('b load')" onerror="console.log('b error')">
        <script>console.log('inline')</script></body>"#,
        &[("/s1.js", "console.log('s1')"), ("/i.png", "not a png")],
    );
    m.wait_pending(&["/s1.js", "/i.png"]);
    m.complete("/s1.js");
    m.thread.run_until_idle(Some(Instant::now() + Duration::from_millis(200)));
    assert_eq!(m.console(), ["s1", "inline"]);
    // Every request for /i.png still outstanding is answered.
    while m.loader.pending_urls().iter().any(|u| u.ends_with("/i.png")) {
        m.complete("/i.png");
        m.thread.run_until_idle(Some(Instant::now() + Duration::from_millis(50)));
    }
    assert!(m.try_wait_load(3), "load event: requested {:?}, log {:?}", m.loader.requests.borrow(), m.rec.log.borrow());
    // Each image completes once (an `image/png` body whose header does not
    // parse still fires `load`, as without speculation).
    let console = m.console();
    for id in ["a", "b"] {
        let n = console.iter().filter(|l| *l == &format!("{id} load") || *l == &format!("{id} error")).count();
        assert_eq!(n, 1, "{id}: {console:?}");
    }
}

/// Two `<link rel=stylesheet>` with the same URL after a blocking script,
/// then a script that waits for them: both sheets load and the script runs.
#[test]
fn review_duplicate_stylesheets_while_speculation_pending() {
    let mut m = Manual::start(
        r#"<!doctype html><head><script src="/s1.js"></script>
        <link rel=stylesheet href="/a.css"><link rel=stylesheet href="/a.css">
        <script>console.log('inline', getComputedStyle(document.documentElement).color)</script></head><body></body>"#,
        &[("/s1.js", "console.log('s1')"), ("/a.css", "html { color: rgb(1, 2, 3) }")],
    );
    m.wait_pending(&["/s1.js", "/a.css"]);
    m.complete("/s1.js");
    m.thread.run_until_idle(Some(Instant::now() + Duration::from_millis(200)));
    while m.loader.pending_urls().iter().any(|u| u.ends_with("/a.css")) {
        m.complete("/a.css");
        m.thread.run_until_idle(Some(Instant::now() + Duration::from_millis(50)));
    }
    assert!(m.try_wait_load(3), "load event: requested {:?}, log {:?}", m.loader.requests.borrow(), m.rec.log.borrow());
    assert_eq!(m.console(), ["s1", "inline rgb(1, 2, 3)"]);
}

/// `<link rel=preload as=image>` and an `<img>` of the same URL: both
/// claim the one speculative fetch; both must complete.
#[test]
fn review_preload_and_image_of_same_url() {
    let mut m = Manual::start(
        r#"<!doctype html><head><script src="/s1.js"></script>
        <link rel=preload as=image href="/i.png" onload="console.log('preload load')" onerror="console.log('preload error')">
        </head><body><img src="/i.png" onload="console.log('img load')" onerror="console.log('img error')"></body>"#,
        &[("/s1.js", "console.log('s1')"), ("/i.png", "not a png")],
    );
    m.wait_pending(&["/s1.js", "/i.png"]);
    m.complete("/s1.js");
    m.thread.run_until_idle(Some(Instant::now() + Duration::from_millis(200)));
    while m.loader.pending_urls().iter().any(|u| u.ends_with("/i.png")) {
        m.complete("/i.png");
        m.thread.run_until_idle(Some(Instant::now() + Duration::from_millis(50)));
    }
    assert!(m.try_wait_load(3), "load event: requested {:?}, log {:?}", m.loader.requests.borrow(), m.rec.log.borrow());
    let console = m.console();
    assert!(console.iter().any(|l| l.starts_with("preload ")), "{console:?}");
    assert!(console.iter().any(|l| l.starts_with("img ")), "{console:?}");
}

/// An image claims the pending speculative fetch, a script then points it
/// elsewhere (its request is cancelled), and a later `<img>` uses the first
/// URL again: that one must get a response (its own request), not wait on
/// the cancelled one forever.
#[test]
fn review_cancelled_claim_then_same_url_again() {
    let mut m = Manual::start(
        r#"<!doctype html><body><script src="/s1.js"></script>
        <img id=a src="/i.png">
        <script src="/s2.js"></script>
        <img id=b src="/i.png" onload="console.log('b load')" onerror="console.log('b error')">
        <script>console.log('inline')</script></body>"#,
        &[
            ("/s1.js", "console.log('s1')"),
            ("/s2.js", "document.getElementById('a').src = '/other.png'; console.log('s2')"),
            ("/i.png", "not a png"),
            ("/other.png", "not a png"),
        ],
    );
    m.wait_pending(&["/s1.js", "/i.png", "/s2.js"]);
    m.complete("/s1.js");
    m.thread.run_until_idle(Some(Instant::now() + Duration::from_millis(100)));
    m.complete("/s2.js");
    m.thread.run_until_idle(Some(Instant::now() + Duration::from_millis(200)));
    assert!(m.loader.cancelled.borrow().contains(&format!("{ORIGIN}/i.png")), "{:?}", m.loader.cancelled.borrow());
    // A real network client never answers a cancelled request: b needs its
    // own.
    assert!(m.wait_count("/i.png", 2), "b's request: {:?}", m.loader.requests.borrow());
}

/// A script writes a `<base href>`: the scripts after it resolve against
/// the new base; the speculative request for the old URL is wasted and
/// cancelled, never executed.
#[test]
fn review_written_base_changes_later_script_url() {
    let mut m = Manual::start(
        r#"<!doctype html><head><script src="/s1.js"></script><script src="b.js"></script>
        <script>console.log('inline', document.baseURI)</script></head><body></body>"#,
        &[
            ("/s1.js", r#"console.log('s1'); document.write('<base href="/other/">')"#),
            ("/b.js", "console.log('WRONG b')"),
            ("/other/b.js", "console.log('other b')"),
        ],
    );
    m.wait_pending(&["/s1.js", "/b.js"]);
    m.complete("/b.js");
    m.complete("/s1.js");
    m.wait_pending(&["/other/b.js"]);
    m.complete("/other/b.js");
    m.wait_load();
    assert_eq!(m.console(), ["s1", "other b", "inline http://site.test/other/"]);
    assert_eq!(m.loader.count("/b.js"), 1);
}

/// A written script duplicates a speculated URL: the written one takes the
/// speculation, the document's own element fetches again (as without
/// speculation, no cache) and both run, in order.
#[test]
fn review_written_script_duplicates_speculated_url() {
    let mut m = Manual::start(
        r#"<!doctype html><body><script src="/s1.js"></script><script src="/s2.js"></script>
        <script>console.log('inline')</script></body>"#,
        &[
            ("/s1.js", r#"console.log('s1'); document.write('<script src="/s2.js"><\/script>')"#),
            ("/s2.js", "console.log('s2', document.scripts.length)"),
        ],
    );
    m.wait_pending(&["/s1.js", "/s2.js"]);
    m.complete("/s1.js");
    m.complete("/s2.js");
    m.thread.run_until_idle(Some(Instant::now() + Duration::from_millis(100)));
    while m.loader.pending_urls().iter().any(|u| u.ends_with("/s2.js")) {
        m.complete("/s2.js");
        m.thread.run_until_idle(Some(Instant::now() + Duration::from_millis(50)));
    }
    m.wait_load();
    assert_eq!(m.console(), ["s1", "s2 2", "s2 3", "inline"]);
    assert_eq!(m.loader.count("/s2.js"), 2, "{:?}", m.loader.requests.borrow());
}

/// Scripts the document never executes are not requested: data-block
/// types, nomodule classics, `src` on SVG's script, script text inside
/// `<math>`/`<svg>` CDATA, and `<iframe srcdoc>` markup (the iframe makes
/// its own request, once).
#[test]
fn review_non_executable_scripts_are_not_requested() {
    let mut m = Manual::start(
        r#"<!doctype html><body><script src="/s1.js"></script>
        <script type="text/template" src="/tpl.js"></script>
        <script type="text/javascript; charset=utf-8" src="/param.js"></script>
        <script language="vbscript" src="/vb.js"></script>
        <script type="module" nomodule src="/modnomod.js"></script>
        <svg><script src="/svg.js"></script><![CDATA[ <script src="/cdata.js"></script> ]]></svg>
        <math><mtext><b>x</b></mtext><annotation-xml><script src="/mathx.js"></script></annotation-xml></math>
        <iframe srcdoc="<script src='/srcdoc.js'></script>"></iframe>
        <script>console.log('inline')</script></body>"#,
        &[
            ("/s1.js", "console.log('s1')"),
            ("/modnomod.js", "console.log('modnomod')"),
            ("/srcdoc.js", "console.log('srcdoc')"),
        ],
    );
    m.wait_pending(&["/s1.js"]);
    m.thread.run_until_idle(Some(Instant::now() + Duration::from_millis(100)));
    // What the scan requested while s1 blocks the parser (the document
    // URL and s1 are the parser's own).
    let ahead: Vec<String> = m.loader.requests.borrow()[2..].to_vec();
    assert_eq!(ahead, [format!("{ORIGIN}/modnomod.js")], "requested ahead");
    m.complete("/s1.js");
    // Answer whatever the document itself goes on to request.
    let rec = m.rec.clone();
    for _ in 0..50 {
        m.thread.run_until_idle(Some(Instant::now() + Duration::from_millis(50)));
        for u in m.loader.pending_urls() {
            let path = u.trim_start_matches(ORIGIN).to_owned();
            m.complete(&path);
        }
        if rec.log.borrow().iter().any(|l| l == "event:load") {
            break;
        }
    }
    assert_eq!(m.loader.count("/modnomod.js"), 1, "{:?}", m.loader.requests.borrow());
}

/// Script data that looks like a tag after an escaped `<script>` inside it
/// (`<!--<script>`): the real tokenizer never ends the element at the inner
/// `</script>`, so nothing in it is a request.
#[test]
fn review_double_escaped_script_text_is_not_scanned() {
    let mut m = Manual::start(
        r#"<!doctype html><body><script src="/s1.js"></script>
        <script><!--<script></script><img src="/esc.png">--></script>
        <script>console.log('inline')</script></body>"#,
        &[("/s1.js", "console.log('s1')")],
    );
    m.wait_pending(&["/s1.js"]);
    m.thread.run_until_idle(Some(Instant::now() + Duration::from_millis(100)));
    let ahead = m.loader.requests.borrow().clone();
    m.complete("/s1.js");
    m.wait_load();
    assert_eq!(m.loader.count("/esc.png"), 0, "ahead: {ahead:?}");
}

/// A classic `nomodule` script waiting for a stylesheet: the browser never
/// fetches it, so the early fetch for the stylesheet-blocked script must
/// not either.
#[test]
fn review_stylesheet_blocked_nomodule_script_is_not_fetched() {
    let mut m = Manual::start(
        r#"<!doctype html><head><link rel="stylesheet" href="/slow.css"><script nomodule src="/nm.js"></script>
        <script type="text/x-template" src="/tpl.js"></script>
        </head><body><script>console.log('inline')</script></body>"#,
        &[("/slow.css", "body { color: red }"), ("/nm.js", "console.log('nm')")],
    );
    m.wait_pending(&["/slow.css"]);
    m.thread.run_until_idle(Some(Instant::now() + Duration::from_millis(100)));
    let ahead = m.loader.requests.borrow().clone();
    m.complete("/slow.css");
    m.wait_load();
    assert_eq!(m.console(), ["inline"]);
    assert_eq!(m.loader.count("/nm.js"), 0, "ahead: {ahead:?}");
}

/// Markup an inline script writes after the parser consumed part of the
/// scanned input: the written scripts after the first written blocking one
/// are requested ahead too (the report: written markup is scanned at the
/// next block).
#[test]
fn review_written_markup_after_consumed_input_is_scanned() {
    let filler = "<p>filler paragraph text</p>".repeat(40);
    let html = format!(
        r#"<!doctype html><body><script src="/s1.js"></script>{filler}
        <script>document.write('<script src="/w1.js"><\/script><script src="/w2.js"><\/script>')</script>
        <script>console.log('inline')</script></body>"#
    );
    let mut m = Manual::start(
        &html,
        &[("/s1.js", "console.log('s1')"), ("/w1.js", "console.log('w1')"), ("/w2.js", "console.log('w2')")],
    );
    m.wait_pending(&["/s1.js"]);
    m.complete("/s1.js");
    m.wait_pending(&["/w1.js"]);
    m.thread.run_until_idle(Some(Instant::now() + Duration::from_millis(100)));
    let w2_ahead = m.loader.count("/w2.js");
    m.complete("/w1.js");
    if !m.loader.pending_urls().iter().any(|u| u.ends_with("/w2.js")) {
        m.wait_pending(&["/w2.js"]);
    }
    m.complete("/w2.js");
    m.wait_load();
    assert_eq!(m.console(), ["s1", "w1", "w2", "inline"]);
    assert_eq!(w2_ahead, 1, "w2 requested while w1 was pending: {:?}", m.loader.requests.borrow());
}

/// Like `serve`, with a per-path handler: (status, extra headers, body,
/// delay). Records each request's head.
#[allow(clippy::type_complexity)]
fn serve_with(
    handler: Arc<dyn Fn(&str) -> (u16, Vec<(String, String)>, String, u64) + Send + Sync>,
    heads: Arc<Mutex<Vec<(String, String)>>>,
) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let (handler, heads) = (handler.clone(), heads.clone());
            std::thread::spawn(move || loop {
                let mut buf = Vec::new();
                let mut byte = [0u8; 1];
                while !buf.ends_with(b"\r\n\r\n") {
                    match stream.read(&mut byte) {
                        Ok(1) => buf.push(byte[0]),
                        _ => return,
                    }
                }
                let req = String::from_utf8_lossy(&buf).to_string();
                let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
                heads.lock().unwrap().push((path.clone(), req));
                let (status, extra, body, delay) = handler(&path);
                std::thread::sleep(Duration::from_millis(delay));
                let mut head =
                    format!("HTTP/1.1 {status} X\r\nContent-Length: {}\r\nCache-Control: no-store\r\n", body.len());
                for (k, v) in extra {
                    head.push_str(&format!("{k}: {v}\r\n"));
                }
                head.push_str("\r\n");
                if stream.write_all(head.as_bytes()).and_then(|_| stream.write_all(body.as_bytes())).is_err() {
                    return;
                }
            });
        }
    });
    addr
}

fn http_load(origin: &str, secs: u64) -> (bool, Vec<String>) {
    let mut thread = crate::browser_thread().unwrap();
    let rec = Rc::new(Recorder::default());
    thread.browser.set_observer(Some(rec.clone()));
    thread.browser.set_engine_hooks(Some(rec.clone()));
    let client = pyro_net::NetClient::new(pyro_net::NetConfig::default()).unwrap();
    thread.browser.set_loader(Some(Rc::new(NetLoader::new(client))));
    let page = thread.browser.create_page().unwrap();
    rec.log.borrow_mut().clear();
    thread.browser.navigate(page, NavigateOpts::address_bar(format!("{origin}/"))).unwrap();
    let r = thread.run_until(Some(Instant::now() + Duration::from_secs(secs)), |_| {
        rec.log.borrow().iter().any(|l| l == "event:load")
    });
    thread.run_until_idle(Some(Instant::now() + Duration::from_millis(200)));
    let console = rec.log.borrow().iter().filter(|l| !l.starts_with("event:")).cloned().collect();
    (r == pyro_runtime::RunOutcome::Done, console)
}

fn js(body: &str) -> (u16, Vec<(String, String)>, String, u64) {
    (200, vec![("Content-Type".into(), "text/javascript".into())], body.into(), 30)
}

/// Speculated scripts answered 404, 500 and through a redirect: errors fire
/// `error` events (no execution of the error body), the redirect target
/// runs, each URL is requested once.
#[test]
fn review_http_errors_and_redirects_of_speculated_scripts() {
    let heads: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
    let handler: Arc<dyn Fn(&str) -> (u16, Vec<(String, String)>, String, u64) + Send + Sync> =
        Arc::new(|path: &str| match path {
            "/" => (
                200,
                vec![("Content-Type".into(), "text/html".into())],
                r#"<!doctype html><body><script src="/s1.js"></script>
                <script src="/missing.js" onerror="console.log('404 error')"></script>
                <script src="/boom.js" onerror="console.log('500 error')"></script>
                <script src="/redir.js"></script>
                <link rel=stylesheet href="/redir.css">
                <script>console.log('inline', getComputedStyle(document.body).color)</script></body>"#
                    .into(),
                0,
            ),
            "/s1.js" => (200, vec![("Content-Type".into(), "text/javascript".into())], "console.log('s1')".into(), 200),
            "/missing.js" => {
                (404, vec![("Content-Type".into(), "text/javascript".into())], "console.log('404 body ran')".into(), 0)
            }
            "/boom.js" => {
                (500, vec![("Content-Type".into(), "text/javascript".into())], "console.log('500 body ran')".into(), 0)
            }
            "/redir.js" => (302, vec![("Location".into(), "/target.js".into())], String::new(), 0),
            "/target.js" => js("console.log('target', document.currentScript.src)"),
            "/redir.css" => (302, vec![("Location".into(), "/target.css".into())], String::new(), 0),
            "/target.css" => {
                (200, vec![("Content-Type".into(), "text/css".into())], "body { color: rgb(4, 5, 6) }".into(), 0)
            }
            _ => (404, vec![], "nope".into(), 0),
        });
    let origin = serve_with(handler, heads.clone());
    let (loaded, console) = http_load(&origin, 20);
    assert!(loaded, "{console:?}");
    let console: Vec<String> =
        console.into_iter().filter(|l| !l.starts_with("exception: TypeError: Failed to load script")).collect();
    let target = format!("target {origin}/redir.js");
    assert_eq!(console, ["s1", "404 error", "500 error", target.as_str(), "inline rgb(4, 5, 6)"], "{console:?}");
    let heads = heads.lock().unwrap();
    for p in ["/s1.js", "/missing.js", "/boom.js", "/redir.js", "/target.js", "/redir.css", "/target.css"] {
        assert_eq!(
            heads.iter().filter(|(h, _)| h == p).count(),
            1,
            "{p}: {:?}",
            heads.iter().map(|h| &h.0).collect::<Vec<_>>()
        );
    }
}

/// A cookie an earlier blocking script sets: a later script's request
/// carried it before speculation (it was made after the cookie was set).
/// Recorded, not asserted either way: Chrome's preload scanner has the same
/// race. The test prints what the later request carried.
#[test]
fn review_cookie_set_by_earlier_script() {
    let heads: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
    let handler: Arc<dyn Fn(&str) -> (u16, Vec<(String, String)>, String, u64) + Send + Sync> =
        Arc::new(|path: &str| match path {
            "/" => (
                200,
                vec![("Content-Type".into(), "text/html".into())],
                r#"<!doctype html><body><script src="/s1.js"></script><script src="/s2.js"></script></body>"#.into(),
                0,
            ),
            "/s1.js" => js("document.cookie = 'k=v; path=/'"),
            "/s2.js" => js("console.log('s2')"),
            _ => (404, vec![], "nope".into(), 0),
        });
    let origin = serve_with(handler, heads.clone());
    let (loaded, console) = http_load(&origin, 20);
    assert!(loaded, "{console:?}");
    let heads = heads.lock().unwrap();
    let s2 = heads.iter().find(|(p, _)| p == "/s2.js").expect("s2 requested");
    let has_cookie = s2.1.to_ascii_lowercase().contains("cookie: k=v");
    eprintln!("REVIEW cookie on speculated /s2.js: {has_cookie}");
}
