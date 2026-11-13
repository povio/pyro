//! Closing a browser context ends its network client at once, whatever its
//! pages were still loading.
use pyro_browser::NavigateOpts;
use pyro_net::{NetError, NetEvent, SubscriptionError};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// `/` is a document with a script, `/held.js`, whose response the server
/// holds until `release()`. Nothing cancels a script's request when its page
/// closes (an image's is cancelled then).
struct Server {
    origin: String,
    held: Arc<AtomicBool>,
    gate: Arc<(Mutex<bool>, Condvar)>,
    thread: Option<JoinHandle<()>>,
}

impl Server {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let held = Arc::new(AtomicBool::new(false));
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let (held2, gate2) = (held.clone(), gate.clone());
        let thread = thread::spawn(move || {
            let mut workers = Vec::new();
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
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
                let (held, gate) = (held2.clone(), gate2.clone());
                workers.push(thread::spawn(move || {
                    let body = if path == "/held.js" {
                        held.store(true, Ordering::Release);
                        let (open, wake) = &*gate;
                        drop(wake.wait_while(open.lock().unwrap(), |open| !*open).unwrap());
                        ""
                    } else {
                        "<p>page</p><script src=/held.js></script>"
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
        Self { origin, held, gate, thread: Some(thread) }
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

#[test]
fn closing_a_context_aborts_the_loads_in_flight_and_stops_its_client() {
    let server = Server::new();
    let mut thread = crate::browser_thread().unwrap();
    thread.enable_network(Default::default()).unwrap();
    let context = thread.browser.create_context().unwrap();
    let page = thread.browser.create_page_in(context).unwrap();
    let events = thread.browser.context_net(context).unwrap().subscribe();
    thread.browser.navigate(page, NavigateOpts::address_bar(format!("{}/", server.origin))).unwrap();
    let held = server.held.clone();
    let r = thread.run_until(Some(Instant::now() + Duration::from_secs(5)), |_| held.load(Ordering::Acquire));
    assert_eq!(r, pyro_runtime::RunOutcome::Done, "the script request reached the server");

    assert!(thread.browser.close_context(context));
    // The server still holds the script: the client stops because its last
    // handle is gone, not because the request ended. The request's sink
    // (which pyro-net owns until then) must not be such a handle.
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut aborted = 0;
    loop {
        match events.try_recv() {
            Ok(NetEvent::LoadingFailed { error: NetError::Aborted, canceled: true, .. }) => aborted += 1,
            Ok(_) => {}
            Err(SubscriptionError::Closed) => break,
            Err(SubscriptionError::Empty) => {
                assert!(Instant::now() < deadline, "the closed context's network client is still running");
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(e) => panic!("{e:?}"),
        }
    }
    assert_eq!(aborted, 1, "the held script request was aborted");
}
