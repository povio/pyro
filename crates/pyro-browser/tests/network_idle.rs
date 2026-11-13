//! `networkAlmostIdle` / `networkIdle` after `load`: reported once the
//! document's in-flight requests stay at most 2 / at 0 for 500 ms (Chrome's
//! IdlenessDetector windows), never while that many requests are in flight.
//! Scenarios follow W3's Chrome 147 probe (a document whose inline script
//! starts requests).
use pyro_browser::{BrowserThread, LoaderId, NavigateOpts, PageEvent, PageId, PageObserver};
use std::cell::RefCell;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::rc::Rc;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const QUIET: Duration = Duration::from_millis(500);

/// `/doc?n=<N>&fail=<0|1>` is a document whose inline script starts N fetches
/// of `/gated` (held until `release()`) and, with `fail=1`, one fetch of
/// `/fail` (the connection is closed without a response).
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
                    let resource = path.split('?').next().unwrap_or("/").to_string();
                    let body = match resource.as_str() {
                        "/doc" => {
                            let query = path.split_once('?').map_or("", |(_, q)| q);
                            let n: usize = query
                                .split('&')
                                .find_map(|kv| kv.strip_prefix("n="))
                                .and_then(|v| v.parse().ok())
                                .unwrap_or(0);
                            let fail = query.contains("fail=1");
                            format!(
                                "<script>for (let i = 0; i < {n}; i++) fetch('/gated?' + i).then(r => r.text());{}</script><p>doc</p>",
                                if fail { "fetch('/fail').catch(() => {});" } else { "" }
                            )
                        }
                        "/sse-doc" => "<script>fetch('/stream').then(r=>r.text())</script><p>stream</p>".to_string(),
                        "/stream" => {
                            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n3\r\n:\n\n\r\n").unwrap();
                            stream.flush().unwrap();
                            let (lock,wake)=&*gate;
                            let _open=wake.wait_timeout_while(lock.lock().unwrap(),Duration::from_secs(5),|o| !*o).unwrap();
                            let _=stream.write_all(b"0\r\n\r\n");
                            return;
                        }
                        "/fail" => return, // dropped: the request fails
                        _ => {
                            let (lock, wake) = &*gate;
                            let _open =
                                wake.wait_timeout_while(lock.lock().unwrap(), Duration::from_secs(5), |o| !*o).unwrap();
                            "ok".to_string()
                        }
                    };
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
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

/// Lifecycle events with the time they were observed.
#[derive(Default)]
struct Events(RefCell<Vec<(&'static str, LoaderId, Instant)>>);

impl PageObserver for Events {
    fn on_page_event(&self, _page: PageId, event: &PageEvent) {
        let (name, loader) = match event {
            PageEvent::FrameLoaded { loader_id, .. } => ("load", loader_id),
            PageEvent::FrameNetworkAlmostIdle { loader_id, .. } => ("almost", loader_id),
            PageEvent::FrameNetworkIdle { loader_id, .. } => ("idle", loader_id),
            _ => return,
        };
        self.0.borrow_mut().push((name, loader.clone(), Instant::now()));
    }
}

impl Events {
    fn at(&self, name: &str, loader: &LoaderId) -> Option<Instant> {
        self.0.borrow().iter().find(|(n, l, _)| *n == name && l == loader).map(|e| e.2)
    }
    fn count(&self, name: &str) -> usize {
        self.0.borrow().iter().filter(|(n, _, _)| *n == name).count()
    }
}

fn setup() -> (Server, BrowserThread, Rc<Events>, PageId) {
    let server = Server::new();
    let mut thread = crate::browser_thread().unwrap();
    thread.enable_network(Default::default()).unwrap();
    let events = Rc::new(Events::default());
    thread.browser.set_observer(Some(events.clone()));
    let page = thread.browser.create_page().unwrap();
    // The initial about:blank reports its own milestones; start clean.
    let first = thread.browser.page(page).unwrap().main_frame.loader_id.clone();
    let r = thread.run_until(Some(Instant::now() + Duration::from_secs(5)), |_| events.at("idle", &first).is_some());
    assert_eq!(r, pyro_runtime::RunOutcome::Done);
    events.0.borrow_mut().clear();
    (server, thread, events, page)
}

/// Navigates to `/doc?<query>` and waits for its `load`.
fn open(thread: &mut BrowserThread, events: &Events, page: PageId, server: &Server, query: &str) -> LoaderId {
    let loader =
        thread.browser.navigate(page, NavigateOpts::address_bar(format!("{}/doc?{query}", server.origin))).unwrap();
    let r = thread.run_until(Some(Instant::now() + Duration::from_secs(5)), |_| events.at("load", &loader).is_some());
    assert_eq!(r, pyro_runtime::RunOutcome::Done, "load");
    loader
}

/// Runs the loop for `d` (the detector's timers do not keep a run alive).
fn run_for(thread: &mut BrowserThread, d: Duration) {
    let end = Instant::now() + d;
    thread.run_until(Some(end), |_| false);
}

fn wait_for(thread: &mut BrowserThread, events: &Events, name: &str, loader: &LoaderId) -> Instant {
    let r = thread.run_until(Some(Instant::now() + Duration::from_secs(5)), |_| events.at(name, loader).is_some());
    assert_eq!(r, pyro_runtime::RunOutcome::Done, "waiting for {name}");
    events.at(name, loader).unwrap()
}

#[test]
fn without_requests_both_milestones_follow_load_after_the_quiet_window() {
    let (server, mut thread, events, page) = setup();
    let loader = open(&mut thread, &events, page, &server, "n=0");
    let load = events.at("load", &loader).unwrap();
    let idle = wait_for(&mut thread, &events, "idle", &loader);
    let almost = events.at("almost", &loader).expect("almost idle before idle");
    assert!(almost >= load + QUIET && idle >= load + QUIET, "reported inside the quiet window");
    assert!(almost <= idle);
    assert_eq!(
        thread.browser.page(page).unwrap().main_frame.network_idle,
        pyro_browser::NetworkIdle { almost_idle: true, idle: true }
    );
}

#[test]
fn a_request_in_flight_allows_almost_idle_but_never_idle() {
    let (server, mut thread, events, page) = setup();
    let loader = open(&mut thread, &events, page, &server, "n=1");
    let load = events.at("load", &loader).unwrap();
    let almost = wait_for(&mut thread, &events, "almost", &loader);
    assert!(almost >= load + QUIET);
    run_for(&mut thread, 3 * QUIET);
    assert!(events.at("idle", &loader).is_none(), "idle reported with a request in flight");
    let state = thread.browser.page(page).unwrap().main_frame.network_idle;
    assert_eq!(state, pyro_browser::NetworkIdle { almost_idle: true, idle: false });
    let released = Instant::now();
    server.release();
    let idle = wait_for(&mut thread, &events, "idle", &loader);
    assert!(idle >= released + QUIET, "idle inside the quiet window after the last request");
    assert_eq!((events.count("almost"), events.count("idle")), (1, 1));
}

#[test]
fn three_requests_in_flight_hold_back_both_milestones() {
    let (server, mut thread, events, page) = setup();
    let loader = open(&mut thread, &events, page, &server, "n=3");
    run_for(&mut thread, 3 * QUIET);
    assert!(
        events.at("almost", &loader).is_none() && events.at("idle", &loader).is_none(),
        "milestone with 3 in flight"
    );
    let released = Instant::now();
    server.release();
    let idle = wait_for(&mut thread, &events, "idle", &loader);
    let almost = events.at("almost", &loader).unwrap();
    assert!(almost >= released + QUIET, "almost idle inside the quiet window after the requests finished");
    assert!(idle >= released + QUIET);
}

#[test]
fn a_failed_request_counts_as_finished() {
    let (server, mut thread, events, page) = setup();
    let loader = open(&mut thread, &events, page, &server, "n=0&fail=1");
    let load = events.at("load", &loader).unwrap();
    let idle = wait_for(&mut thread, &events, "idle", &loader);
    assert!(idle >= load + QUIET);
}

#[test]
fn a_replaced_or_closed_document_reports_nothing_further() {
    let (server, mut thread, events, page) = setup();
    let old = open(&mut thread, &events, page, &server, "n=1");
    // Replaced while its request is in flight: the new document reports its
    // own milestones, the old one none.
    let new = thread.browser.navigate(page, NavigateOpts::address_bar("about:blank")).unwrap();
    wait_for(&mut thread, &events, "idle", &new);
    server.release();
    run_for(&mut thread, 3 * QUIET);
    assert!(events.at("idle", &old).is_none(), "replaced document reported idle");
    // Closed before its quiet window ends: nothing at all.
    let other = thread.browser.create_page().unwrap();
    let loader = thread.browser.page(other).unwrap().main_frame.loader_id.clone();
    assert!(thread.browser.close_page(other));
    run_for(&mut thread, 3 * QUIET);
    assert!(events.at("almost", &loader).is_none() && events.at("idle", &loader).is_none());
}

#[test]
fn established_event_stream_is_quiet_but_still_blocks_network_idle() {
    let (server, mut thread, events, page) = setup();
    let loader =
        thread.browser.navigate(page, NavigateOpts::address_bar(format!("{}/sse-doc", server.origin))).unwrap();
    let outcome = thread.run_until(Some(Instant::now() + Duration::from_secs(5)), |browser| {
        let context = browser.page(page).unwrap().context;
        browser.context_net(context).and_then(|net| net.activity(&loader.0)).is_some_and(|a| a.event_streams == 1)
    });
    assert_eq!(outcome, pyro_runtime::RunOutcome::Done);
    run_for(&mut thread, 2 * QUIET);
    let work = thread.browser.page_work(page).unwrap();
    assert_eq!((work.requests, work.event_streams), (0, 1));
    assert!(work.quiet(), "{work:?}");
    let context = thread.browser.page(page).unwrap().context;
    assert_eq!(thread.browser.context_net(context).unwrap().activity(&loader.0).unwrap().inflight, 1);
    assert!(events.at("idle", &loader).is_none());
    server.release();
    wait_for(&mut thread, &events, "idle", &loader);
}
