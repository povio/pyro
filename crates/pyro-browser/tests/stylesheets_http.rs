//! Native stylesheet HTTP/CORS/cancellation checks, separate from synthetic loader ordering tests.
use pyro_browser::{BrowserThread, InitScript, NavigateOpts, PageId, DEFAULT_CONTEXT};
use pyro_net::{Destination, NetEvent};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::rc::Rc;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

struct Server {
    origin: String,
    html: Arc<Mutex<String>>,
    gate: Arc<(Mutex<bool>, Condvar)>,
    thread: Option<JoinHandle<()>>,
}
impl Server {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let html = Arc::new(Mutex::new("<p>HTTP</p>".to_string()));
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let (doc, wait) = (html.clone(), gate.clone());
        let thread = thread::spawn(move || {
            let mut workers = Vec::new();
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut first = String::new();
                if reader.read_line(&mut first).unwrap_or(0) == 0 {
                    continue;
                }
                let path = first.split_whitespace().nth(1).unwrap_or("/").to_string();
                let mut request_origin = None;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some((key, value)) = line.split_once(':') {
                        if key.eq_ignore_ascii_case("origin") {
                            request_origin = Some(value.trim().to_owned());
                        }
                    }
                }
                if path == "/stop" {
                    break;
                }
                let (doc, wait) = (doc.clone(), wait.clone());
                workers.push(thread::spawn(move || {
                    let (mime, body) = if path == "/" { ("text/html", doc.lock().unwrap().clone()) }
                        else { ("text/css", "html { display: none }".to_string()) };
                    let cors = if path == "/cors.css" { request_origin.map(|o| format!("Access-Control-Allow-Origin: {o}\r\n")).unwrap_or_default() } else { String::new() };
                    if write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {}\r\n{cors}Connection: close\r\n\r\n", body.len()).is_err() { return; }
                    if path == "/slow.css" {
                        let (lock, wake) = &*wait;
                        let _guard = wake.wait_timeout_while(lock.lock().unwrap(), Duration::from_secs(5), |ready| !*ready).unwrap();
                    }
                    let _ = stream.write_all(body.as_bytes());
                }));
            }
            for worker in workers {
                worker.join().unwrap();
            }
        });
        Self { origin, html, gate, thread: Some(thread) }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        *self.gate.0.lock().unwrap() = true;
        self.gate.1.notify_all();
        if let Ok(mut stream) = TcpStream::connect(self.origin.trim_start_matches("http://")) {
            let _ = stream.write_all(b"GET /stop HTTP/1.1\r\nHost: localhost\r\n\r\n");
        }
        self.thread.take().unwrap().join().unwrap();
    }
}
fn eval(browser: &pyro_browser::Browser, page: PageId, source: &str) -> String {
    let realm = browser.page(page).unwrap().main_frame.main_realm().unwrap();
    let scope = realm.scope().unwrap();
    scope.eval(source, None, 1).unwrap_or_else(|e| panic!("{}", e.details(&scope).text())).to_string().unwrap()
}
fn setup() -> (BrowserThread, PageId) {
    let mut thread = crate::browser_thread().unwrap();
    thread.enable_network(Default::default()).unwrap();
    let page = thread.browser.create_page().unwrap();
    thread.browser.add_init_script(page, InitScript { id: "css-events".into(), world: None, source: "globalThis.events=[]; for (const t of ['load','error']) document.addEventListener(t,e=>{if(e.target.tagName==='LINK')events.push(e.target.id+':'+t)},true)".into() });
    (thread, page)
}
#[track_caller]
fn until(thread: &mut BrowserThread, condition: impl FnMut(&mut pyro_browser::Browser) -> bool) {
    assert_eq!(
        thread.run_until(Some(Instant::now() + Duration::from_secs(5)), condition),
        pyro_runtime::RunOutcome::Done
    );
}

#[test]
fn stylesheet_cors_and_network_metadata_use_the_document_context() {
    let document = Server::new();
    let css = Server::new();
    *document.html.lock().unwrap() = format!("<link id=allow rel=stylesheet crossorigin href='{}/cors.css'><link id=deny rel=stylesheet crossorigin href='{}/denied.css'>", css.origin, css.origin);
    let (mut thread, page) = setup();
    let handle = thread.handle();
    let events = thread.browser.context_net(DEFAULT_CONTEXT).unwrap().subscribe_with(
        Default::default(),
        Some(Arc::new(move || {
            let _ = handle.post(|_| {});
        })),
    );
    thread.browser.navigate(page, NavigateOpts::address_bar(format!("{}/", document.origin))).unwrap();
    until(&mut thread, |b| {
        eval(b, page, "globalThis.events?.length === 2 && document.readyState === 'complete'") == "true"
    });
    assert_eq!(eval(&thread.browser, page, "events.sort().join('|')"), "allow:load|deny:error");
    assert_eq!(eval(&thread.browser, page, "getComputedStyle(document.documentElement).display"), "none");
    let frame = &thread.browser.page(page).unwrap().main_frame;
    let mut requests = 0;
    while let Ok(event) = events.try_recv() {
        if let NetEvent::RequestWillBeSent { request, .. } = event {
            if request.destination == Destination::Style {
                requests += 1;
                assert_eq!(request.frame_id.as_deref(), Some(frame.id.0.as_str()));
                assert_eq!(request.loader_id.as_deref(), Some(frame.loader_id.0.as_str()));
                assert_eq!(request.headers.get("origin").unwrap().to_str().unwrap(), document.origin);
            }
        }
    }
    assert_eq!(requests, 2);
}

#[test]
fn removing_or_navigating_cancels_native_stylesheet_requests_and_releases_state() {
    let server = Server::new();
    let (mut thread, page) = setup();
    let handle = thread.handle();
    let events = thread.browser.context_net(DEFAULT_CONTEXT).unwrap().subscribe_with(
        Default::default(),
        Some(Arc::new(move || {
            let _ = handle.post(|_| {});
        })),
    );
    thread.browser.navigate(page, NavigateOpts::address_bar(format!("{}/", server.origin))).unwrap();
    until(&mut thread, |b| {
        b.page(page).unwrap().main_frame.url.starts_with(&server.origin)
            && eval(b, page, "document.readyState") == "complete"
    });
    for navigation in [false, true] {
        eval(&thread.browser, page, "globalThis.link=document.createElement('link');link.rel='stylesheet';link.href='/slow.css';document.head.append(link);'ok'");
        until(&mut thread, |_| {
            while let Ok(event) = events.try_recv() {
                if let NetEvent::ResponseReceived { head, .. } = event {
                    if head.url.path() == "/slow.css" {
                        return true;
                    }
                }
            }
            false
        });
        assert_eq!(thread.browser.page(page).unwrap().stylesheet_usage().0, 1);
        let old = Rc::downgrade(thread.browser.page(page).unwrap().main_frame.page_dom.as_ref().unwrap());
        if navigation {
            thread.browser.navigate(page, NavigateOpts::address_bar("about:blank")).unwrap();
        } else {
            eval(&thread.browser, page, "link.remove();'ok'");
        }
        thread.browser.checkpoint();
        assert_eq!(thread.browser.page(page).unwrap().stylesheet_usage(), (0, 0));
        until(&mut thread, |_| {
            while let Ok(event) = events.try_recv() {
                if let NetEvent::LoadingFailed { canceled: true, destination: Destination::Style, .. } = event {
                    return true;
                }
            }
            false
        });
        if navigation {
            thread.browser.engine.collect_garbage_now();
            thread.browser.engine.drain_finalized();
            assert!(old.upgrade().is_none());
        } else {
            assert_eq!(eval(&thread.browser, page, "events.length"), "0");
        }
    }
}
