//! Document loading against real JavaScriptCore: in-memory loader and the real
//! lp-net HTTP client against a local server.

use lp_browser::*;
use lp_js::{ConsoleLevel, EngineHooks, JsException, Realm, Value};
use std::cell::RefCell;
use std::io::{Read, Write};
use std::rc::Rc;
use std::time::{Duration, Instant};

#[derive(Default)]
struct Recorder {
    log: RefCell<Vec<String>>,
}

impl PageObserver for Recorder {
    fn on_page_event(&self, _page: PageId, event: &PageEvent) {
        let e = match event {
            PageEvent::FrameNavigate { .. } => "event:navigate".to_string(),
            PageEvent::FrameNavigated { url, .. } => format!("event:navigated {url}"),
            PageEvent::FrameNavigateFailed { error, .. } => format!("event:failed {error}"),
            PageEvent::FrameDomContentLoaded { .. } => "event:dcl".to_string(),
            PageEvent::FrameLoaded { .. } => "event:load".to_string(),
            _ => return,
        };
        self.log.borrow_mut().push(e);
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
    fn unhandled_rejection(&self, _realm: &Realm, e: &JsException) {
        self.log.borrow_mut().push(format!("rejection: {}", e.text()));
    }
}

const PAGE: &str = r#"<!doctype html><html><head>
<script>console.log('inline1')</script>
<script src="/blocking.js"></script>
<script defer src="/defer.js"></script>
<script async src="/async.js"></script>
<script type="module">import { v } from '/dep.js'; console.log('inline-module', v)</script>
<script type="module" src="/mod.js"></script>
<script nomodule>console.log('nomodule must not run')</script>
<script type="text/template">console.log('data block must not run')</script>
</head><body><p>hi</p>
<script>console.log('inline2')</script>
</body></html>"#;

fn files() -> Vec<(&'static str, &'static str, &'static str, u64)> {
    vec![
        ("/", "text/html", PAGE, 0),
        ("/blocking.js", "text/javascript", "console.log('blocking')", 20),
        ("/defer.js", "text/javascript", "console.log('defer')", 0),
        ("/async.js", "text/javascript", "console.log('async')", 60),
        ("/dep.js", "text/javascript", "export const v = 42;", 0),
        (
            "/mod.js",
            "text/javascript",
            "import { v } from './dep.js'; await 0; console.log('ext-module', v, import.meta.url)",
            0,
        ),
    ]
}

fn run(thread: &mut BrowserThread, rec: &Recorder) {
    let r = thread.run_until(Some(Instant::now() + Duration::from_secs(10)), |_| {
        rec.log.borrow().iter().any(|l| l == "event:load")
    });
    assert_eq!(r, lp_runtime::RunOutcome::Done, "log: {:?}", rec.log.borrow());
    // Let late async work (if any) settle.
    thread.run_until_idle(Some(Instant::now() + Duration::from_millis(200)));
}

fn check_order(log: &[String], origin: &str) {
    let pos = |s: &str| log.iter().position(|l| l == s).unwrap_or_else(|| panic!("missing {s:?} in {log:?}"));
    let ext_module = format!("ext-module 42 {origin}/mod.js");
    for (a, b) in [
        ("event:navigate", "inline1"),
        ("inline1", "blocking"),
        ("blocking", "inline2"),
        ("inline2", "defer"),
        ("defer", "inline-module 42"),
        ("inline-module 42", ext_module.as_str()),
        (ext_module.as_str(), "event:dcl"),
        ("async", "event:load"),
        ("event:dcl", "event:load"),
    ] {
        assert!(pos(a) < pos(b), "{a:?} should precede {b:?}: {log:?}");
    }
    assert!(!log.iter().any(|l| l.contains("must not run")), "{log:?}");
    assert!(!log.iter().any(|l| l.starts_with("exception") || l.starts_with("rejection")), "{log:?}");
}

#[test]
fn script_ordering_with_static_loader() {
    let mut thread = crate::browser_thread().unwrap();
    let rec = Rc::new(Recorder::default());
    thread.browser.set_observer(Some(rec.clone()));
    thread.browser.set_engine_hooks(Some(rec.clone()));
    let loader = Rc::new(StaticLoader::default());
    for (path, mime, body, delay) in files() {
        loader.insert(&format!("http://site.test{path}"), mime, body, Duration::from_millis(delay));
    }
    thread.browser.set_loader(Some(loader.clone()));
    let page = thread.browser.create_page().unwrap();
    rec.log.borrow_mut().clear();
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/")).unwrap();
    run(&mut thread, &rec);
    check_order(&rec.log.borrow(), "http://site.test");
    // The document was parsed into the page's DOM.
    let p = thread.browser.page(page).unwrap();
    let dom = p.main_frame.dom.clone().unwrap();
    let doc = p.main_frame.document.unwrap();
    let d = dom.borrow();
    let body = d.body(doc).unwrap().unwrap();
    assert!(d.text_content(body).unwrap().unwrap().contains("hi"));
}

#[test]
fn network_errors_fail_navigation() {
    let mut thread = crate::browser_thread().unwrap();
    let rec = Rc::new(Recorder::default());
    thread.browser.set_observer(Some(rec.clone()));
    thread.browser.set_loader(Some(Rc::new(StaticLoader::default())));
    let page = thread.browser.create_page().unwrap();
    rec.log.borrow_mut().clear();
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/missing")).unwrap();
    thread.run_until_idle(Some(Instant::now() + Duration::from_secs(2)));
    let log = rec.log.borrow();
    assert_eq!(log[0], "event:navigate");
    assert!(log[1].starts_with("event:failed net::ERR_FILE_NOT_FOUND"), "{log:?}");
}

/// Minimal HTTP/1.1 server on 127.0.0.1 serving `files()` (one thread per connection).
fn serve() -> (String, std::thread::JoinHandle<()>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = format!("http://{}", listener.local_addr().unwrap());
    let h = std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
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
                if path == "/__stop" {
                    return;
                }
                let resp = files().into_iter().find(|f| f.0 == path);
                let (status, mime, body, delay) = match resp {
                    Some((_, m, b, d)) => ("200 OK", m, b, d),
                    None => ("404 Not Found", "text/plain", "nope", 0),
                };
                std::thread::sleep(Duration::from_millis(delay));
                let head = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: {mime}; charset=utf-8\r\nContent-Length: {}\r\n\r\n",
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

#[test]
fn script_ordering_over_real_http() {
    let (origin, _server) = serve();
    let mut thread = crate::browser_thread().unwrap();
    let rec = Rc::new(Recorder::default());
    thread.browser.set_observer(Some(rec.clone()));
    thread.browser.set_engine_hooks(Some(rec.clone()));
    let client = lp_net::NetClient::new(lp_net::NetConfig::default()).unwrap();
    thread.browser.set_loader(Some(Rc::new(NetLoader::new(client))));
    let page = thread.browser.create_page().unwrap();
    rec.log.borrow_mut().clear();
    thread.browser.navigate(page, NavigateOpts::address_bar(format!("{origin}/"))).unwrap();
    run(&mut thread, &rec);
    check_order(&rec.log.borrow(), &origin);
    assert!(rec.log.borrow().contains(&format!("event:navigated {origin}/")));
}

#[test]
fn contexts_have_isolated_network_state() {
    let mut thread = crate::browser_thread().unwrap();
    thread.enable_network(lp_net::NetConfig::default()).unwrap();
    let b = &mut thread.browser;
    let c1 = b.create_context().unwrap();
    let c2 = b.create_context().unwrap();
    let j0 = b.context_net(DEFAULT_CONTEXT).unwrap().cookies().clone();
    let j1 = b.context_net(c1).unwrap().cookies().clone();
    let j2 = b.context_net(c2).unwrap().cookies().clone();
    assert!(!std::sync::Arc::ptr_eq(&j0, &j1) && !std::sync::Arc::ptr_eq(&j1, &j2));
    let p1 = b.create_page_in(c1).unwrap();
    let p2 = b.create_page_in(c2).unwrap();
    assert_eq!(b.page(p1).unwrap().context, c1);
    assert!(b.close_context(c1));
    assert!(b.page(p1).is_none());
    assert!(b.page(p2).is_some());
    assert!(b.context_net(c1).is_none());
    assert!(!b.close_context(DEFAULT_CONTEXT));
}

#[derive(Default)]
struct NavRecorder {
    log: RefCell<Vec<String>>,
}

impl PageObserver for NavRecorder {
    fn on_page_event(&self, _page: PageId, event: &PageEvent) {
        let e = match event {
            PageEvent::FrameNavigated { url, .. } => format!("navigated {url}"),
            PageEvent::FrameNavigatedWithinDocument { url, kind, .. } => format!("within {kind:?} {url}"),
            PageEvent::FrameLoaded { .. } => "load".to_string(),
            PageEvent::FrameNavigateFailed { error, .. } => format!("failed {error}"),
            _ => return,
        };
        self.log.borrow_mut().push(e);
    }
}

impl EngineHooks for NavRecorder {
    fn exception_thrown(&self, _realm: &Realm, e: &JsException) {
        self.log.borrow_mut().push(format!("exception: {}", e.text()));
    }
}

fn nav_setup() -> (BrowserThread, Rc<NavRecorder>, PageId) {
    let mut thread = crate::browser_thread().unwrap();
    let rec = Rc::new(NavRecorder::default());
    thread.browser.set_observer(Some(rec.clone()));
    thread.browser.set_engine_hooks(Some(rec.clone()));
    let loader = Rc::new(StaticLoader::default());
    loader.insert("http://site.test/a/b?x=1#h", "text/html", "<p>a</p>", Duration::ZERO);
    loader.insert("http://site.test/other", "text/html", "<p>other</p>", Duration::ZERO);
    thread.browser.set_loader(Some(loader));
    let page = thread.browser.create_page().unwrap();
    rec.log.borrow_mut().clear();
    thread.browser.navigate(page, NavigateOpts::address_bar("http://site.test/a/b?x=1#h")).unwrap();
    let r = thread
        .run_until(Some(Instant::now() + Duration::from_secs(5)), |_| rec.log.borrow().iter().any(|l| l == "load"));
    assert_eq!(r, lp_runtime::RunOutcome::Done);
    rec.log.borrow_mut().clear();
    (thread, rec, page)
}

fn js(thread: &BrowserThread, page: PageId, src: &str) -> String {
    let realm = thread.browser.page(page).unwrap().main_frame.main_realm().unwrap().clone();
    let s = realm.scope().unwrap();
    match s.eval(src, None, 1) {
        Ok(v) => v.to_string().unwrap(),
        Err(e) => format!("THROW {}", e.details(&s).text()),
    }
}

#[test]
fn location_reflects_current_url() {
    let (thread, _rec, page) = nav_setup();
    assert_eq!(
        js(&thread, page, "[location.href, location.origin, location.protocol, location.host, location.pathname, location.search, location.hash, String(location), window.origin].join('|')"),
        "http://site.test/a/b?x=1#h|http://site.test|http:|site.test|/a/b|?x=1|#h|http://site.test/a/b?x=1#h|http://site.test"
    );
    assert_eq!(
        js(
            &thread,
            page,
            "location === window.location && history === window.history && performance === window.performance"
        ),
        "true"
    );
    assert_eq!(js(&thread, page, "location instanceof Location && history instanceof History"), "true");
}

#[test]
fn push_and_replace_state_are_same_document() {
    let (mut thread, rec, page) = nav_setup();
    let len0: f64 = js(&thread, page, "history.length").parse().unwrap();
    assert_eq!(
        js(&thread, page, "history.pushState({n: 1}, '', '/route/1?q=2'); [location.pathname, location.search, JSON.stringify(history.state)].join('|')"),
        "/route/1|?q=2|{\"n\":1}"
    );
    assert_eq!(js(&thread, page, "history.length").parse::<f64>().unwrap(), len0 + 1.0);
    assert_eq!(
        js(&thread, page, "history.replaceState(null, '', 'two'); location.pathname + ':' + history.state"),
        "/route/two:null"
    );
    assert_eq!(js(&thread, page, "history.length").parse::<f64>().unwrap(), len0 + 1.0);
    // document.URL in the DOM follows synchronously.
    {
        let p = thread.browser.page(page).unwrap();
        let dom = p.main_frame.dom.clone().unwrap();
        let doc = p.main_frame.document.unwrap();
        assert_eq!(&*dom.borrow().document_data(doc).unwrap().url, "http://site.test/route/two");
    }
    assert!(js(&thread, page, "history.pushState(null, '', 'https://evil.test/')").contains("SecurityError"));
    thread.run_until_idle(Some(Instant::now() + Duration::from_secs(2)));
    assert_eq!(
        *rec.log.borrow(),
        ["within Push http://site.test/route/1?q=2", "within Replace http://site.test/route/two"]
    );
    assert_eq!(thread.browser.page(page).unwrap().main_frame.url, "http://site.test/route/two");
}

#[test]
fn history_state_is_one_object_until_the_entry_changes_like_chromium() {
    // Chromium (Playwright 1.59.1): history.state is deserialized once per
    // pushState, replaceState or traversal and then the same object on every
    // read (mutations visible); popstate's event.state is that object. The
    // engine re-parsed it on every read, so identity checks in history and
    // router libraries never settled (TanStack History builds its locations
    // from history.state).
    let (mut thread, _rec, page) = nav_setup();
    assert_eq!(
        js(
            &thread,
            page,
            "globalThis.out = {}; out.initialNull = history.state === null; \
             globalThis.obj = { a: 1, nested: { b: 2 } }; history.pushState(obj, '', '?p1'); \
             globalThis.s1 = history.state; out.pushRepeat = s1 === history.state; out.pushIsClone = s1 !== obj; \
             out.nestedRepeat = s1.nested === history.state.nested; s1.a = 99; out.mutationVisible = history.state.a; \
             history.replaceState({ c: 3 }, '', '?r1'); globalThis.s2 = history.state; \
             out.replaceNew = s2 !== s1; out.replaceRepeat = s2 === history.state; \
             history.replaceState(history.state, ''); out.replaceSelfNew = history.state !== s2; \
             globalThis.s2b = history.state; history.pushState({ d: 4 }, '', '?p2'); \
             addEventListener('popstate', e => { out.popEventIsHistoryState = e.state === history.state; \
               out.popRepeat = history.state === history.state; out.popValue = JSON.stringify(history.state); \
               out.popNotOld = history.state !== s2b; }, { once: true }); \
             history.back(); JSON.stringify(out)"
        ),
        r#"{"initialNull":true,"pushRepeat":true,"pushIsClone":true,"nestedRepeat":true,"mutationVisible":99,"replaceNew":true,"replaceRepeat":true,"replaceSelfNew":true}"#
    );
    thread.run_until_idle(Some(Instant::now() + Duration::from_secs(2)));
    assert_eq!(
        js(&thread, page, "JSON.stringify([out.popEventIsHistoryState, out.popRepeat, out.popValue, out.popNotOld])"),
        r#"[true,true,"{\"c\":3}",true]"#
    );
    // Each realm (world) keeps its own object.
    assert_eq!(js(&thread, page, "history.state === history.state && history.state.c"), "3");
}

#[test]
fn back_traverses_same_document_and_hash_changes_are_fragments() {
    let (mut thread, rec, page) = nav_setup();
    js(&thread, page, "history.pushState(null, '', '/p2'); location.hash = 'frag'");
    assert_eq!(js(&thread, page, "location.href"), "http://site.test/p2#frag");
    js(&thread, page, "history.back()");
    thread.run_until_idle(Some(Instant::now() + Duration::from_secs(2)));
    assert_eq!(js(&thread, page, "location.href"), "http://site.test/p2");
    assert_eq!(
        *rec.log.borrow(),
        [
            "within Push http://site.test/p2",
            "within Fragment http://site.test/p2#frag",
            "within Push http://site.test/p2"
        ]
    );
}

#[test]
fn location_assignment_navigates_cross_document_and_back_returns() {
    let (mut thread, rec, page) = nav_setup();
    js(&thread, page, "location.href = '/other'");
    let r = thread
        .run_until(Some(Instant::now() + Duration::from_secs(5)), |_| rec.log.borrow().iter().any(|l| l == "load"));
    assert_eq!(r, lp_runtime::RunOutcome::Done, "{:?}", rec.log.borrow());
    assert_eq!(js(&thread, page, "location.href + ' ' + history.length"), "http://site.test/other 3");
    rec.log.borrow_mut().clear();
    js(&thread, page, "history.back()");
    let r = thread
        .run_until(Some(Instant::now() + Duration::from_secs(5)), |_| rec.log.borrow().iter().any(|l| l == "load"));
    assert_eq!(r, lp_runtime::RunOutcome::Done, "{:?}", rec.log.borrow());
    assert_eq!(js(&thread, page, "location.href + ' ' + history.length"), "http://site.test/a/b?x=1#h 3");
    assert_eq!(rec.log.borrow()[0], "navigated http://site.test/a/b?x=1#h");
    js(&thread, page, "location.replace('/other')");
    rec.log.borrow_mut().clear();
    thread.run_until(Some(Instant::now() + Duration::from_secs(5)), |_| rec.log.borrow().iter().any(|l| l == "load"));
    assert_eq!(js(&thread, page, "history.length"), "3", "replace keeps the entry count");
}

#[test]
fn cross_document_back_restores_the_entry_state() {
    let (mut thread, rec, page) = nav_setup();
    js(&thread, page, "history.replaceState({k: 1}, ''); location.href = '/other'");
    let r = thread
        .run_until(Some(Instant::now() + Duration::from_secs(5)), |_| rec.log.borrow().iter().any(|l| l == "load"));
    assert_eq!(r, lp_runtime::RunOutcome::Done, "{:?}", rec.log.borrow());
    rec.log.borrow_mut().clear();
    js(&thread, page, "history.back()");
    let r = thread
        .run_until(Some(Instant::now() + Duration::from_secs(5)), |_| rec.log.borrow().iter().any(|l| l == "load"));
    assert_eq!(r, lp_runtime::RunOutcome::Done, "{:?}", rec.log.borrow());
    assert_eq!(
        js(&thread, page, "location.href + ' ' + JSON.stringify(history.state)"),
        "http://site.test/a/b?x=1#h {\"k\":1}"
    );
}

/// History handling follows the navigation's intent, not URL equality: a
/// reload keeps the entry's state, while `location.replace` and a same-URL
/// push (converted to a replace) create a new entry with null state.
#[test]
fn same_url_navigations_keep_state_only_for_reload() {
    let (mut thread, rec, page) = nav_setup();
    let wait_load = |thread: &mut BrowserThread| {
        let r = thread
            .run_until(Some(Instant::now() + Duration::from_secs(5)), |_| rec.log.borrow().iter().any(|l| l == "load"));
        assert_eq!(r, lp_runtime::RunOutcome::Done, "{:?}", rec.log.borrow());
        rec.log.borrow_mut().clear();
    };
    // A fragment-free document, so same-URL navigations are cross-document.
    js(&thread, page, "location.href = '/other'");
    wait_load(&mut thread);
    let state = "location.href + ' ' + history.length + ' ' + JSON.stringify(history.state)";
    for (navigation, expected) in [
        ("location.reload()", r#"http://site.test/other 3 {"k":1}"#),
        ("location.replace(location.href)", "http://site.test/other 3 null"),
        ("location.href = location.href", "http://site.test/other 3 null"),
    ] {
        js(&thread, page, "history.replaceState({k: 1}, '')");
        js(&thread, page, navigation);
        wait_load(&mut thread);
        assert_eq!(js(&thread, page, state), expected, "{navigation}");
    }
}

#[test]
fn performance_now_is_monotonic_and_origin_is_wall_time() {
    let (thread, _rec, page) = nav_setup();
    assert_eq!(
        js(&thread, page, "const a = performance.now(); let b = a; for (let i = 0; i < 1e5; i++) b = performance.now(); [a >= 0, b >= a, Math.abs(performance.timeOrigin + b - Date.now()) < 1000, typeof performance.mark].join()"),
        "true,true,true,function"
    );
}

#[test]
fn history_methods_accept_an_unbound_receiver() {
    // TanStack History: `(push ? h.pushState : h.replaceState)(state, "", url)`.
    let (mut thread, rec, page) = nav_setup();
    assert_eq!(
        js(
            &thread,
            page,
            "const h = history; const len = h.length; \
             (true ? h.pushState : h.replaceState)({ a: 1 }, '', '/t1'); \
             (false ? h.pushState : h.replaceState)({ b: 2 }, '', '/t2'); \
             [location.pathname, JSON.stringify(history.state), history.length - len, \
              Object.getOwnPropertyDescriptor(History.prototype, 'length').get.call(undefined) === history.length].join('|')"
        ),
        "/t2|{\"b\":2}|1|true"
    );
    let back = js(&thread, page, "const b = history.back; b(); 'ok'");
    assert_eq!(back, "ok");
    thread.run_until_idle(Some(Instant::now() + Duration::from_secs(2)));
    assert_eq!(js(&thread, page, "location.href"), "http://site.test/a/b?x=1#h");
    // Foreign receivers are still rejected.
    assert!(js(&thread, page, "history.pushState.call({}, null, '', '/x')").contains("Illegal invocation"));
    assert!(js(&thread, page, "history.pushState.call(location, null, '', '/x')").contains("Illegal invocation"));
    assert!(!rec.log.borrow().iter().any(|l| l.starts_with("exception")), "{:?}", rec.log.borrow());
}

#[derive(Default)]
struct OrderRecorder {
    log: RefCell<Vec<String>>,
}

impl PageObserver for OrderRecorder {
    fn on_page_event(&self, _page: PageId, event: &PageEvent) {
        if let PageEvent::FrameNavigatedWithinDocument { url, kind, .. } = event {
            self.log.borrow_mut().push(format!("within {kind:?} {url}"));
        }
    }
}

impl EngineHooks for OrderRecorder {
    fn console_api(&self, _realm: &Realm, _level: ConsoleLevel, args: &[Value<'_>]) {
        let parts: Vec<String> = args.iter().map(|a| a.to_string().unwrap_or_default()).collect();
        self.log.borrow_mut().push(format!("console {}", parts.join(" ")));
    }
}

#[test]
fn same_document_commits_are_reported_within_the_causing_turn() {
    let (mut thread, _rec, page) = nav_setup();
    let order = Rc::new(OrderRecorder::default());
    thread.browser.set_observer(Some(order.clone()));
    thread.browser.set_engine_hooks(Some(order.clone()));
    // Directly from a script turn (CDP Runtime.evaluate shape): the event is
    // delivered before the call returns, before later output of the turn.
    js(&thread, page, "history.pushState(null, '', '/pushed-1'); console.log('after push')");
    assert_eq!(*order.log.borrow(), ["within Push http://site.test/pushed-1", "console after push"]);
    // Frame::url follows at the next checkpoint (before any CDP reply).
    thread.browser.checkpoint();
    assert_eq!(thread.browser.page(page).unwrap().main_frame.url, "http://site.test/pushed-1");
    // From a task (fetch continuation shape): replace and fragment are reported
    // inside the same task, ahead of what the next task does.
    order.log.borrow_mut().clear();
    js(
        &thread,
        page,
        "setTimeout(() => { history.replaceState(null, '', '/replaced'); location.hash = 'frag'; console.log('same task') }, 0);
         setTimeout(() => console.log('next task'), 0);",
    );
    thread.run_until_idle(Some(Instant::now() + Duration::from_secs(2)));
    assert_eq!(
        *order.log.borrow(),
        [
            "within Replace http://site.test/replaced",
            "within Fragment http://site.test/replaced#frag",
            "console same task",
            "console next task"
        ]
    );
    assert_eq!(thread.browser.page(page).unwrap().main_frame.url, "http://site.test/replaced#frag");
    // History traversal stays asynchronous (queued as a task, like browsers).
    order.log.borrow_mut().clear();
    js(&thread, page, "history.back(); console.log('after back call')");
    assert_eq!(*order.log.borrow(), ["console after back call"]);
    thread.run_until_idle(Some(Instant::now() + Duration::from_secs(2)));
    assert_eq!(order.log.borrow()[1], "within Push http://site.test/replaced");
}
