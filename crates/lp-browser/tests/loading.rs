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
    let mut thread = BrowserThread::new().unwrap();
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
    let mut thread = BrowserThread::new().unwrap();
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
    let mut thread = BrowserThread::new().unwrap();
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
    let mut thread = BrowserThread::new().unwrap();
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
