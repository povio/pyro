//! Media resources over real HTTP: a live stream (a body that never ends)
//! is read until its headers are in, then the transfer stops, so it no
//! longer holds the document's load event.
use pyro_browser::{BrowserThread, NavigateOpts, PageId};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

/// One MPEG-1 Layer III frame: 128 kb/s, 44.1 kHz, stereo (417 bytes).
fn mp3_frame() -> Vec<u8> {
    let mut frame = vec![0xFF, 0xFB, 0x90, 0x00];
    frame.resize(417, 0);
    frame
}

struct Live {
    origin: String,
    /// Bytes of the stream written so far.
    sent: Arc<AtomicUsize>,
    /// The stream's writer gave up (the client went away).
    closed: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
}

impl Live {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let (sent, closed, stop) =
            (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
        let (s, c, halt) = (sent.clone(), closed.clone(), stop.clone());
        thread::spawn(move || {
            while !halt.load(Ordering::Relaxed) {
                let Ok((mut stream, _)) = listener.accept() else {
                    thread::sleep(Duration::from_millis(5));
                    continue;
                };
                stream.set_nonblocking(false).unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut first = String::new();
                if reader.read_line(&mut first).unwrap_or(0) == 0 {
                    continue;
                }
                let path = first.split_whitespace().nth(1).unwrap_or("/").to_string();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                }
                let (s, c, halt) = (s.clone(), c.clone(), halt.clone());
                thread::spawn(move || {
                    if path == "/" {
                        let html = "<audio id=a src=/live autoplay muted></audio>";
                        let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}", html.len());
                        return;
                    }
                    // No length: the stream goes on until the client leaves.
                    if write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: audio/mpeg\r\nConnection: close\r\n\r\n")
                        .is_err()
                    {
                        return;
                    }
                    let burst: Vec<u8> = (0..40).flat_map(|_| mp3_frame()).collect();
                    while !halt.load(Ordering::Relaxed) {
                        if stream.write_all(&burst).is_err() {
                            c.store(true, Ordering::Relaxed);
                            return;
                        }
                        s.fetch_add(burst.len(), Ordering::Relaxed);
                        thread::sleep(Duration::from_millis(5));
                    }
                });
            }
        });
        Live { origin, sent, closed, stop }
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn eval(browser: &pyro_browser::Browser, page: PageId, source: &str) -> String {
    let realm = browser.page(page).unwrap().main_frame.main_realm().unwrap();
    let scope = realm.scope().unwrap();
    scope.eval(source, None, 1).unwrap_or_else(|e| panic!("{}", e.details(&scope).text())).to_string().unwrap()
}

#[test]
fn a_live_stream_does_not_hold_the_load_event() {
    let server = Live::new();
    let mut thread: BrowserThread = crate::browser_thread().unwrap();
    thread.enable_network(Default::default()).unwrap();
    let page = thread.browser.create_page().unwrap();
    thread.browser.navigate(page, NavigateOpts::address_bar(format!("{}/", server.origin))).unwrap();
    let outcome = thread.run_until(Some(Instant::now() + Duration::from_secs(10)), |b| {
        eval(b, page, "document.readyState === 'complete' && document.getElementById('a')?.readyState === 4") == "true"
    });
    assert_eq!(outcome, pyro_runtime::RunOutcome::Done);
    assert_eq!(
        eval(&thread.browser, page, "var a = document.getElementById('a'); [a.duration, a.error].join()"),
        "Infinity,"
    );
    // The transfer stopped once the headers were in.
    let deadline = Instant::now() + Duration::from_secs(10);
    while !server.closed.load(Ordering::Relaxed) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(server.closed.load(Ordering::Relaxed), "the stream is still being read");
    assert!(server.sent.load(Ordering::Relaxed) < 64 << 20);
}
