use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

fn fetch(body: &'static str, args: &[&str]) -> std::process::Output {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    listener.set_nonblocking(true).unwrap();
    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    // An accepted socket inherits the listener's non-blocking mode on macOS: a read
                    // before the request arrives would fail, and the reply would go out ahead of it.
                    stream.set_nonblocking(false).unwrap();
                    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                            break;
                        }
                    }
                    let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(5));
                }
                Err(e) => panic!("fixture failed: {e}"),
            }
        }
    });
    let mut child = Command::new(env!("CARGO_BIN_EXE_pyro"))
        .arg("fetch")
        .args(args)
        .arg(url)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            server.join().unwrap();
            panic!("fetch failed to enforce its JavaScript timeout");
        }
        thread::sleep(Duration::from_millis(10));
    }
    server.join().unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn fetch_deadline_interrupts_parser_script_and_timer_script() {
    for body in ["<script>while (true) {}</script>", "<script>setTimeout(() => { while (true) {} }, 0)</script>"] {
        let output = fetch(body, &["--timeout-ms", "200", "--wait-ms", "1000"]);
        assert_eq!(output.status.code(), Some(124), "{}", String::from_utf8_lossy(&output.stderr));
        assert!(output.stdout.is_empty(), "timeout should not dump an incomplete document");
    }
}

#[test]
fn fetch_dumps_script_modified_document() {
    let output = fetch("<body><script>document.body.setAttribute('data-loaded', 'yes')</script></body>", &[]);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(String::from_utf8_lossy(&output.stdout).contains("data-loaded=\"yes\""));
}

/// A response of [`Server`]: sent `delay` after the request arrived.
struct Reply {
    delay: Duration,
    content_type: &'static str,
    body: String,
}

impl Reply {
    fn html(body: &str) -> Option<Reply> {
        Some(Reply { delay: Duration::ZERO, content_type: "text/html", body: body.into() })
    }
    fn json(body: &str, delay_ms: u64) -> Option<Reply> {
        Some(Reply { delay: Duration::from_millis(delay_ms), content_type: "application/json", body: body.into() })
    }
}

/// A local HTTP server answering each request with `routes(path)` (404 for
/// `None`), one connection per request, each on a thread of its own so that
/// a delayed reply holds up no other. Every response allows any origin:
/// `eval`'s page is about:blank, whose origin is opaque.
struct Server {
    origin: String,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Server {
    fn start(routes: impl Fn(&str) -> Option<Reply> + Send + Sync + 'static) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let routes = Arc::new(routes);
        let stopped = stop.clone();
        let thread = thread::spawn(move || {
            for stream in listener.incoming() {
                if stopped.load(Ordering::Acquire) {
                    break;
                }
                let Ok(stream) = stream else { continue };
                let routes = routes.clone();
                thread::spawn(move || answer(stream, &*routes));
            }
        });
        Server { origin, stop, thread: Some(thread) }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.origin)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = TcpStream::connect(self.origin.trim_start_matches("http://"));
        let _ = self.thread.take().unwrap().join();
    }
}

fn answer(mut stream: TcpStream, routes: &dyn Fn(&str) -> Option<Reply>) {
    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut request = String::new();
    if reader.read_line(&mut request).unwrap_or(0) == 0 {
        return;
    }
    let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
            break;
        }
    }
    let (status, content_type, body) = match routes(&path) {
        Some(reply) => {
            thread::sleep(reply.delay);
            ("200 OK", reply.content_type, reply.body)
        }
        None => ("404 Not Found", "text/plain", String::new()),
    };
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

struct Run {
    status: ExitStatus,
    stdout: String,
    stderr: String,
    elapsed: Duration,
}

fn spawn(args: &[&str]) -> Child {
    Command::new(env!("CARGO_BIN_EXE_pyro"))
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

fn read_all(mut from: impl Read + Send + 'static) -> JoinHandle<String> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = from.read_to_end(&mut bytes);
        String::from_utf8_lossy(&bytes).into_owned()
    })
}

/// Waits for `child`, killing it (and failing) once it has run for `limit`.
fn wait(child: &mut Child, started: Instant, limit: Duration) -> ExitStatus {
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if started.elapsed() > limit {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("pyro still ran after {limit:?}");
        }
        thread::sleep(Duration::from_millis(5));
    }
}

/// Runs `pyro <args>` to its end (at most 20 s).
fn pyro(args: &[&str]) -> Run {
    // A freshly linked binary's first start is checked by the system first (a second on macOS).
    Command::new(env!("CARGO_BIN_EXE_pyro")).arg("version").output().unwrap();
    let started = Instant::now();
    let mut child = spawn(args);
    let stdout = read_all(child.stdout.take().unwrap());
    let stderr = read_all(child.stderr.take().unwrap());
    let status = wait(&mut child, started, Duration::from_secs(20));
    let elapsed = started.elapsed();
    Run { status, stdout: stdout.join().unwrap(), stderr: stderr.join().unwrap(), elapsed }
}

/// After `load`, the page fetches its data (answered 100 ms later) and
/// inserts it.
const DATA_AFTER_LOAD: &str = r#"<body><ul id="list"></ul><script>
addEventListener('load', () => {
  fetch('/data.json').then(r => r.json()).then(items => {
    for (const item of items) list.append(Object.assign(document.createElement('li'), { textContent: item }));
  });
});
</script></body>"#;

fn data_after_load(path: &str) -> Option<Reply> {
    match path {
        "/" => Reply::html(DATA_AFTER_LOAD),
        "/data.json" => Reply::json(r#"["alpha", "beta"]"#, 100),
        _ => None,
    }
}

#[test]
fn fetch_wait_runs_until_data_fetched_after_load_is_in() {
    let server = Server::start(data_after_load);
    let run = pyro(&["fetch", "--wait-ms", "3000", &server.url("/")]);
    assert!(run.status.success(), "{}", run.stderr);
    assert!(run.stdout.contains("<li>alpha</li><li>beta</li>"), "{}", run.stdout);
    // Done once nothing is left, not at the end of the wait.
    assert!(run.elapsed < Duration::from_millis(2000), "took {:?}", run.elapsed);
}

#[test]
fn fetch_without_wait_dumps_at_load() {
    let server = Server::start(data_after_load);
    let run = pyro(&["fetch", &server.url("/")]);
    assert!(run.status.success(), "{}", run.stderr);
    assert!(run.stdout.contains(r#"<ul id="list"></ul>"#), "{}", run.stdout);
}

/// A timer 200 ms out is more than `Pyro.idle` counts as due soon (50 ms);
/// `--wait-ms` waited for it before and still does.
#[test]
fn fetch_wait_runs_a_timer_set_for_later() {
    let server = Server::start(|path| match path {
        "/" => Reply::html(
            "<body><script>setTimeout(() => { document.body.dataset.timer = 'fired' }, 200)</script></body>",
        ),
        _ => None,
    });
    let run = pyro(&["fetch", "--wait-ms", "3000", &server.url("/")]);
    assert!(run.status.success(), "{}", run.stderr);
    assert!(run.stdout.contains(r#"data-timer="fired""#), "{}", run.stdout);
    assert!(run.elapsed < Duration::from_millis(2000), "took {:?}", run.elapsed);
}

/// The second request starts in the first one's callback (a fetch, then an
/// XHR), each answered 100 ms after it arrives.
#[test]
fn fetch_wait_follows_requests_started_by_responses() {
    let server = Server::start(|path| match path {
        "/" => Reply::html(
            r#"<body><script>
addEventListener('load', () => {
  fetch('/first.json').then(r => r.json()).then(first => {
    document.body.dataset.first = first.value;
    const xhr = new XMLHttpRequest();
    xhr.open('GET', '/second.json');
    xhr.onload = () => { document.body.dataset.second = JSON.parse(xhr.responseText).value; };
    xhr.send();
  });
});
</script></body>"#,
        ),
        "/first.json" => Reply::json(r#"{"value": "one"}"#, 100),
        "/second.json" => Reply::json(r#"{"value": "two"}"#, 100),
        _ => None,
    });
    let run = pyro(&["fetch", "--wait-ms", "3000", &server.url("/")]);
    assert!(run.status.success(), "{}", run.stderr);
    assert!(run.stdout.contains(r#"data-first="one""#), "{}", run.stdout);
    assert!(run.stdout.contains(r#"data-second="two""#), "{}", run.stdout);
    assert!(run.elapsed < Duration::from_millis(2000), "took {:?}", run.elapsed);
}

/// A page that never goes quiet is dumped when the wait is over.
#[test]
fn fetch_wait_ends_for_a_page_that_polls_forever() {
    let server = Server::start(|path| match path {
        "/" => Reply::html(
            r#"<body><script>
let polls = 0;
setInterval(() => fetch('/poll').then(r => r.text()).then(() => { document.body.dataset.polls = ++polls; }), 50);
</script></body>"#,
        ),
        "/poll" => Reply::json("{}", 20),
        _ => None,
    });
    let run = pyro(&["fetch", "--wait-ms", "800", &server.url("/")]);
    assert!(run.status.success(), "{}", run.stderr);
    assert!(run.stdout.contains("data-polls="), "{}", run.stdout);
    assert!(!run.stderr.contains("panicked"), "{}", run.stderr);
    assert!(
        run.elapsed >= Duration::from_millis(800) && run.elapsed < Duration::from_millis(3000),
        "took {:?}",
        run.elapsed
    );
}

#[test]
fn eval_runs_until_its_fetch_is_answered() {
    let server = Server::start(|path| match path {
        "/text" => Some(Reply {
            delay: Duration::from_millis(100),
            content_type: "text/plain",
            body: "from the server".into(),
        }),
        _ => None,
    });
    let code = format!("fetch('{}').then(r => r.text()).then(console.log)", server.url("/text"));
    let run = pyro(&["eval", "-e", &code]);
    assert!(run.status.success(), "{}", run.stderr);
    assert_eq!(run.stdout, "from the server\n");
}

/// Reads the first line of `pyro <args>`'s output, closes the pipe and
/// returns how the process ended and what it wrote on stderr.
fn first_line_then_close(args: &[&str]) -> (String, ExitStatus, String, Duration) {
    let started = Instant::now();
    let mut child = spawn(args);
    let stderr = read_all(child.stderr.take().unwrap());
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut first = String::new();
    stdout.read_line(&mut first).unwrap();
    drop(stdout);
    let status = wait(&mut child, started, Duration::from_secs(20));
    (first, status, stderr.join().unwrap(), started.elapsed())
}

/// `pyro fetch URL | head -1` and `pyro eval ... | head -1` end quietly, as
/// Unix tools do on a closed pipe.
#[test]
fn a_closed_stdout_ends_fetch_and_eval_quietly() {
    // More than a pipe holds: the dump is still being written when the reader goes.
    let page = format!("<body>{}</body>", "<p>a line of the page</p>\n".repeat(20_000));
    let server = Server::start(move |path| (path == "/").then(|| Reply::html(&page)).flatten());
    let (first, status, stderr, _) = first_line_then_close(&["fetch", &server.url("/")]);
    assert!(first.starts_with("<html>"), "{first}");
    assert!(status.success(), "{status:?}: {stderr}");
    assert!(stderr.is_empty(), "{stderr}");

    let (first, status, stderr, elapsed) =
        first_line_then_close(&["eval", "--timeout-ms", "15000", "-e", "for (let i = 0; ; i++) console.log(i)"]);
    assert_eq!(first, "0\n");
    assert!(status.success(), "{status:?}: {stderr}");
    assert!(stderr.is_empty(), "{stderr}");
    assert!(elapsed < Duration::from_secs(10), "took {elapsed:?}: the script ran on");
}
