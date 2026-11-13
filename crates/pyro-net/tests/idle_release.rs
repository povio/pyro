//! The I/O thread's idle hook ([`pyro_net::set_idle_release`]). The hook is
//! process-wide: it counts its runs per thread, and the test reads its own
//! client's I/O thread.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::sync::Mutex;
use std::thread::ThreadId;
use std::time::{Duration, Instant};

use pyro_net::*;

static RELEASES: Mutex<Option<HashMap<ThreadId, usize>>> = Mutex::new(None);

fn count_release() {
    let mut releases = RELEASES.lock().unwrap();
    *releases.get_or_insert_with(HashMap::new).entry(std::thread::current().id()).or_default() += 1;
}

fn releases(thread: ThreadId) -> usize {
    RELEASES.lock().unwrap().as_ref().and_then(|r| r.get(&thread).copied()).unwrap_or(0)
}

/// `/open` sends its head and a first chunk, then never ends; anything else
/// answers `ok`.
fn start_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        return;
                    }
                    let path = line.split_whitespace().nth(1).unwrap_or("/").to_owned();
                    loop {
                        let mut h = String::new();
                        if reader.read_line(&mut h).unwrap_or(0) == 0 || h.trim_end().is_empty() {
                            break;
                        }
                    }
                    if path == "/open" {
                        let _ =
                            stream.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n");
                        std::thread::sleep(Duration::from_secs(600));
                        return;
                    }
                    let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");
                }
            });
        }
    });
    port
}

fn wait_for_releases(thread: ThreadId, above: usize) -> usize {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let n = releases(thread);
        if n > above {
            return n;
        }
        assert!(Instant::now() < deadline, "no idle release after {above}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The hook runs once a second after the thread's last activity even while
/// a request stays open, then not again until there is new activity (a
/// finished request counts).
#[test]
fn idle_release_runs_once_per_quiet_spell_with_a_request_open() {
    set_idle_release(count_release);
    let port = start_server();
    let url = |path: &str| Url::parse(&format!("http://127.0.0.1:{port}{path}")).unwrap();
    let client = NetClient::new(NetConfig::default()).unwrap();

    // The sink runs on the client's I/O thread.
    let (tx, rx) = mpsc::channel();
    client.fetch(
        Request::get(url("/open"), Destination::Fetch),
        Box::new(move |ev| {
            let _ = tx.send((std::thread::current().id(), ev));
        }),
    );
    let io = loop {
        match rx.recv_timeout(Duration::from_secs(10)).expect("event") {
            (io, ResponseEvent::Data(d)) if !d.is_empty() => break io,
            (_, ResponseEvent::Failed(e)) => panic!("{e:?}"),
            _ => {}
        }
    };
    let first = wait_for_releases(io, 0);
    std::thread::sleep(Duration::from_millis(2500));
    assert_eq!(releases(io), first, "a release without new activity");

    let (tx, done) = mpsc::channel();
    client.fetch(
        Request::get(url("/done"), Destination::Fetch),
        Box::new(move |ev| {
            if matches!(ev, ResponseEvent::Finished { .. } | ResponseEvent::Failed(_)) {
                let _ = tx.send(ev);
            }
        }),
    );
    assert!(matches!(done.recv_timeout(Duration::from_secs(10)).expect("finished"), ResponseEvent::Finished { .. }));
    wait_for_releases(io, first);
}
