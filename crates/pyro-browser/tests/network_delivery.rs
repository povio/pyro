//! `PageObserver::on_network_delivery`: fetch/XHR transport progress reaches
//! the browser's observer, identified by page, context and loader, before the
//! script it lets run, and never for a replaced or closed document.
use pyro_browser::{
    BrowserThread, ContextId, LoaderId, NavigateOpts, PageEvent, PageId, PageObserver, DEFAULT_CONTEXT,
};
use pyro_js::{ConsoleLevel, EngineHooks, Realm, Value};
use pyro_net::{NetworkDelivery, RequestId};
use std::cell::RefCell;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::rc::Rc;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const GATED_BODY: &str = "late-body";

/// `/` is an empty document, `/data` a small body; `/gated` sends its headers
/// at once and its body only after `release()`.
struct Server {
    origin: String,
    gate: Arc<(Mutex<bool>, Condvar)>,
    thread: Option<JoinHandle<()>>,
}

impl Server {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let shared = gate.clone();
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
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                }
                if path == "/stop" {
                    break;
                }
                let gate = shared.clone();
                workers.push(thread::spawn(move || {
                    let (mime, body) = match path.as_str() {
                        "/" => ("text/html", "<p>doc</p>"),
                        "/gated" => ("text/plain", GATED_BODY),
                        _ => ("application/json", "{\"answer\":42}"),
                    };
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    if stream.write_all(head.as_bytes()).is_err() || stream.flush().is_err() {
                        return;
                    }
                    if path == "/gated" {
                        let (lock, wake) = &*gate;
                        let _open =
                            wake.wait_timeout_while(lock.lock().unwrap(), Duration::from_secs(5), |o| !*o).unwrap();
                    }
                    let _ = stream.write_all(body.as_bytes());
                }));
            }
            for worker in workers {
                let _ = worker.join();
            }
        });
        Self { origin, gate, thread: Some(thread) }
    }

    fn release(&self) {
        *self.gate.0.lock().unwrap() = true;
        self.gate.1.notify_all();
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.release();
        if let Ok(mut stream) = TcpStream::connect(self.origin.trim_start_matches("http://")) {
            let _ = stream.write_all(b"GET /stop HTTP/1.1\r\nHost: localhost\r\n\r\n");
        }
        let _ = self.thread.take().unwrap().join();
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Entry {
    Delivery { page: PageId, context: ContextId, loader: LoaderId, request: RequestId, frontier: NetworkDelivery },
    Script(String),
}

/// One log for observer deliveries and page `console.log` lines, so their
/// relative order is the order in which they happened on the browser thread.
#[derive(Default)]
struct Log(RefCell<Vec<Entry>>);

impl PageObserver for Log {
    fn on_page_event(&self, _page: PageId, _event: &PageEvent) {}

    fn on_network_delivery(
        &self,
        page: PageId,
        context: ContextId,
        loader: &LoaderId,
        request: RequestId,
        frontier: NetworkDelivery,
    ) {
        self.0.borrow_mut().push(Entry::Delivery { page, context, loader: loader.clone(), request, frontier });
    }
}

impl EngineHooks for Log {
    fn console_api(&self, _realm: &Realm, _level: ConsoleLevel, args: &[Value<'_>]) {
        let line = args.first().and_then(|a| a.to_string().ok()).unwrap_or_default();
        self.0.borrow_mut().push(Entry::Script(line));
    }
}

/// A delivery with its position in the log.
type Delivery = (usize, PageId, ContextId, LoaderId, RequestId, NetworkDelivery);

impl Log {
    fn script(&self, line: &str) -> Option<usize> {
        self.0.borrow().iter().position(|e| *e == Entry::Script(line.into()))
    }
    fn deliveries(&self) -> Vec<Delivery> {
        self.0
            .borrow()
            .iter()
            .enumerate()
            .filter_map(|(i, e)| match e {
                Entry::Delivery { page, context, loader, request, frontier } => {
                    Some((i, *page, *context, loader.clone(), *request, *frontier))
                }
                Entry::Script(_) => None,
            })
            .collect()
    }
}

fn setup() -> (BrowserThread, Rc<Log>) {
    let mut thread = crate::browser_thread().unwrap();
    thread.enable_network(Default::default()).unwrap();
    let log = Rc::new(Log::default());
    thread.browser.set_observer(Some(log.clone()));
    thread.browser.set_engine_hooks(Some(log.clone()));
    (thread, log)
}

fn eval(thread: &BrowserThread, page: PageId, script: &str) -> String {
    let realm = thread.browser.page(page).unwrap().main_frame.main_realm().unwrap();
    let scope = realm.scope().unwrap();
    scope.eval(script, None, 1).unwrap_or_else(|e| panic!("{}", e.details(&scope).text())).to_string().unwrap()
}

fn wait(thread: &mut BrowserThread, what: &str, mut done: impl FnMut() -> bool) {
    let result = thread.run_until(Some(Instant::now() + Duration::from_secs(5)), |_| done());
    assert_eq!(result, pyro_runtime::RunOutcome::Done, "waiting for {what}");
}

fn open(thread: &mut BrowserThread, page: PageId, server: &Server) -> LoaderId {
    thread.browser.navigate(page, NavigateOpts::address_bar(format!("{}/", server.origin))).unwrap();
    // The initial about:blank is already complete: wait for the new document.
    let loaded = format!("location.origin === '{}' && document.readyState === 'complete'", server.origin);
    let result = thread.run_until(Some(Instant::now() + Duration::from_secs(5)), |b| {
        let realm = b.page(page).unwrap().main_frame.main_realm().unwrap();
        let scope = realm.scope().unwrap();
        scope.eval(&loaded, None, 1).unwrap().to_bool()
    });
    assert_eq!(result, pyro_runtime::RunOutcome::Done, "load");
    thread.browser.page(page).unwrap().main_frame.loader_id.clone()
}

/// Checks the deliveries of one request: cumulative, identified as `page` /
/// `context` / `loader`, and each frontier reported before the script steps
/// it enables (`headers_seen`, `body_seen` are log positions).
fn assert_frontier_before_script(
    log: &Log,
    page: PageId,
    context: ContextId,
    loader: &LoaderId,
    headers_seen: usize,
    body_seen: usize,
) {
    let deliveries = log.deliveries();
    assert!(!deliveries.is_empty(), "no delivery reported: {:?}", log.0.borrow());
    let request = deliveries[0].4;
    let mut last = NetworkDelivery::default();
    for (_, p, c, l, r, frontier) in &deliveries {
        assert_eq!((*p, *c, l, *r), (page, context, loader, request), "identity: {:?}", log.0.borrow());
        assert!(frontier.data_bytes >= last.data_bytes && (frontier.response || !last.response), "not cumulative");
        last = *frontier;
    }
    let first_response = deliveries.iter().find(|d| d.5.response).expect("response delivery");
    assert!(first_response.0 < headers_seen, "response frontier after the script saw it: {:?}", log.0.borrow());
    let terminal = deliveries.iter().find(|d| d.5.terminal).expect("terminal delivery");
    assert_eq!(terminal.5, NetworkDelivery { response: true, data_bytes: GATED_BODY.len() as u64, terminal: true });
    assert!(terminal.0 < body_seen, "terminal frontier after the script saw the body: {:?}", log.0.borrow());
}

#[test]
fn fetch_delivery_reaches_the_observer_before_the_script_that_observes_it() {
    let server = Server::new();
    let (mut thread, log) = setup();
    let page = thread.browser.create_page().unwrap();
    let loader = open(&mut thread, page, &server);
    log.0.borrow_mut().clear();
    eval(
        &thread,
        page,
        "fetch('/gated').then(r => { console.log('headers'); return r.text(); }).then(t => console.log('body:' + t.length)); 0",
    );
    wait(&mut thread, "headers", || log.script("headers").is_some());
    server.release();
    let body = format!("body:{}", GATED_BODY.len());
    wait(&mut thread, "body", || log.script(&body).is_some());
    let (headers_seen, body_seen) = (log.script("headers").unwrap(), log.script(&body).unwrap());
    assert_frontier_before_script(&log, page, DEFAULT_CONTEXT, &loader, headers_seen, body_seen);
}

#[test]
fn xhr_delivery_reaches_the_observer_before_its_ready_state_changes() {
    let server = Server::new();
    let (mut thread, log) = setup();
    let page = thread.browser.create_page().unwrap();
    let loader = open(&mut thread, page, &server);
    log.0.borrow_mut().clear();
    eval(
        &thread,
        page,
        "const x = new XMLHttpRequest(); x.open('GET', '/gated');
         x.onreadystatechange = () => console.log('rs:' + x.readyState + ':' + x.responseText.length);
         x.send(); 0",
    );
    wait(&mut thread, "headers", || log.script("rs:2:0").is_some());
    server.release();
    let done = format!("rs:4:{}", GATED_BODY.len());
    wait(&mut thread, "done", || log.script(&done).is_some());
    let (headers_seen, body_seen) = (log.script("rs:2:0").unwrap(), log.script(&done).unwrap());
    assert_frontier_before_script(&log, page, DEFAULT_CONTEXT, &loader, headers_seen, body_seen);
}

#[test]
fn requests_with_the_same_native_id_are_told_apart_by_context_and_loader() {
    let server = Server::new();
    let (mut thread, log) = setup();
    let other = thread.browser.create_context().unwrap();
    let a = thread.browser.create_page().unwrap();
    let b = thread.browser.create_page_in(other).unwrap();
    let loader_a = open(&mut thread, a, &server);
    let loader_b = open(&mut thread, b, &server);
    log.0.borrow_mut().clear();
    for page in [a, b] {
        eval(&thread, page, "fetch('/data').then(r => r.text()).then(() => console.log('done')); 0");
    }
    wait(&mut thread, "both fetches", || {
        log.0.borrow().iter().filter(|e| **e == Entry::Script("done".into())).count() == 2
    });
    let terminal: Vec<_> = log.deliveries().into_iter().filter(|d| d.5.terminal).collect();
    assert_eq!(terminal.len(), 2, "{:?}", log.0.borrow());
    let of = |page| terminal.iter().find(|d| d.1 == page).cloned().expect("delivery for page");
    let (da, db) = (of(a), of(b));
    assert_eq!((da.2, &da.3), (DEFAULT_CONTEXT, &loader_a));
    assert_eq!((db.2, &db.3), (other, &loader_b));
    // Each context has its own NetClient, so the native ids collide: only
    // the context (and loader) keep the two requests apart.
    assert_eq!(da.4, db.4, "expected colliding per-client request ids");
}

#[test]
fn replaced_or_closed_documents_report_no_further_delivery() {
    let server = Server::new();
    let (mut thread, log) = setup();
    let replaced = thread.browser.create_page().unwrap();
    let closed = thread.browser.create_page().unwrap();
    let old_loader = open(&mut thread, replaced, &server);
    open(&mut thread, closed, &server);
    log.0.borrow_mut().clear();
    for page in [replaced, closed] {
        eval(&thread, page, "fetch('/gated').then(r => { console.log('headers'); return r.text(); }); 0");
    }
    wait(&mut thread, "headers", || {
        log.0.borrow().iter().filter(|e| **e == Entry::Script("headers".into())).count() == 2
    });
    // Both documents' requests were being reported until now.
    let reported: Vec<PageId> = log.deliveries().iter().filter(|d| d.5.response).map(|d| d.1).collect();
    assert!(reported.contains(&replaced) && reported.contains(&closed), "{:?}", log.0.borrow());
    thread.browser.navigate(replaced, NavigateOpts::address_bar("about:blank")).unwrap();
    assert!(thread.browser.close_page(closed));
    let before = log.0.borrow().len();
    server.release();
    thread.run_until_idle(Some(Instant::now() + Duration::from_millis(300)));
    let after: Vec<_> = log.0.borrow()[before..].to_vec();
    assert!(after.is_empty(), "delivery or script after replacement/close: {after:?}");
    let new_loader = thread.browser.page(replaced).unwrap().main_frame.loader_id.clone();
    assert_ne!(new_loader, old_loader);
}
