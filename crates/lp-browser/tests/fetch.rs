//! Browser-thread fetch delivery, metadata and document teardown over real HTTP.
use lp_browser::{BrowserThread, NavigateOpts, PageId, DEFAULT_CONTEXT};
use lp_net::{Destination, NetEvent};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

struct Server {
    origin: String,
    release_slow: Arc<(Mutex<bool>, Condvar)>,
    thread: Option<JoinHandle<()>>,
}
impl Server {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let release_slow = Arc::new((Mutex::new(false), Condvar::new()));
        let gate = release_slow.clone();
        let thread = thread::spawn(move || {
            let mut workers = Vec::new();
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request = String::new();
                if reader.read_line(&mut request).unwrap_or(0) == 0 {
                    continue;
                }
                let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();
                if path == "/stop" {
                    break;
                }
                let gate = gate.clone();
                workers.push(thread::spawn(move || {
                    let (mime, body) = match path.as_str() {
                        "/" => ("text/html", "<script>fetch('/data').then(r => r.json()).then(v => globalThis.fetched = v.answer)</script><p>loaded</p>"),
                        "/slow" => ("text/plain", "late"),
                        _ => ("application/json", "{\"answer\":42}"),
                    };
                    if write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).is_err() { return; }
                    if path == "/slow" {
                        let (lock, wake) = &*gate;
                        let _guard = wake.wait_timeout_while(lock.lock().unwrap(), Duration::from_secs(5), |ready| !*ready).unwrap();
                    }
                    let _ = stream.write_all(body.as_bytes());
                }));
            }
            for worker in workers {
                worker.join().unwrap();
            }
        });
        Self { origin, release_slow, thread: Some(thread) }
    }
    fn release(&self) {
        *self.release_slow.0.lock().unwrap() = true;
        self.release_slow.1.notify_all();
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.release();
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
    let result = thread.run_until(Some(Instant::now() + Duration::from_secs(5)), |browser| {
        let realm = browser.page(page).unwrap().main_frame.main_realm().unwrap();
        let scope = realm.scope().unwrap();
        scope.eval(condition, None, 1).unwrap().to_bool()
    });
    assert_eq!(result, lp_runtime::RunOutcome::Done, "waiting for {condition}");
}
fn navigate(thread: &mut BrowserThread, page: PageId, server: &Server) {
    thread.browser.navigate(page, NavigateOpts::address_bar(format!("{}/", server.origin))).unwrap();
    until(thread, page, "globalThis.fetched === 42");
}

#[test]
fn document_fetch_uses_its_context_and_exact_frame_loader_metadata() {
    let server = Server::new();
    let mut thread = BrowserThread::new().unwrap();
    thread.enable_network(Default::default()).unwrap();
    let events = thread.browser.context_net(DEFAULT_CONTEXT).unwrap().subscribe();
    let page = thread.browser.create_page().unwrap();
    navigate(&mut thread, page, &server);
    assert_eq!(eval(&thread, page, "fetched + ':' + document.readyState"), "42:complete");
    let frame = &thread.browser.page(page).unwrap().main_frame;
    let mut observed = false;
    while let Ok(event) = events.try_recv() {
        if let NetEvent::RequestWillBeSent { request, .. } = event {
            if request.destination == Destination::Fetch {
                assert_eq!(request.url.path(), "/data");
                assert_eq!(request.frame_id.as_deref(), Some(frame.id.0.as_str()));
                assert_eq!(request.loader_id.as_deref(), Some(frame.loader_id.0.as_str()));
                observed = true;
            }
        }
    }
    assert!(observed, "fetch omitted context network event");
}

#[test]
fn navigation_cancels_old_fetch_without_closing_shared_context_network() {
    let server = Server::new();
    let mut thread = BrowserThread::new().unwrap();
    thread.enable_network(Default::default()).unwrap();
    let a = thread.browser.create_page().unwrap();
    let b = thread.browser.create_page().unwrap();
    navigate(&mut thread, a, &server);
    navigate(&mut thread, b, &server);
    eval(&thread, a, "fetch('/slow').then(r => { globalThis.headers = true; return r.text(); }).then(() => globalThis.late = true); 'ok'");
    until(&mut thread, a, "globalThis.headers === true");
    let old = thread.browser.page(a).unwrap().main_frame.page_dom.as_ref().unwrap().clone();
    assert!(old.fetch_usage().0 > 0);
    thread.browser.navigate(a, NavigateOpts::address_bar("about:blank")).unwrap();
    assert!(old.is_disposed());
    assert_eq!(old.fetch_usage(), (0, 0));
    server.release();
    eval(&thread, b, "fetch('/data').then(r => r.json()).then(v => globalThis.again = v.answer); 'ok'");
    until(&mut thread, b, "globalThis.again === 42");
    thread.run_until_idle(Some(Instant::now() + Duration::from_millis(100)));
    assert_eq!(eval(&thread, a, "typeof late"), "undefined");
    assert_eq!(eval(&thread, b, "again"), "42");
}
