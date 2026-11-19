use pyro_browser::{BrowserThread, NavigateOpts, PageId};
use pyro_net::http::{HeaderMap, HeaderName, HeaderValue};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// One received request: its path (with the `?from=` tag) and the recorded
/// header values, lower-case names.
#[derive(Debug, Clone)]
struct Seen {
    path: String,
    headers: Vec<(String, String)>,
}

impl Seen {
    fn get(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }
}

/// Local documents, a srcdoc-relative script and a fetch body.
struct Server {
    origin: String,
    seen: Arc<Mutex<Vec<Seen>>>,
    thread: Option<JoinHandle<()>>,
}

impl Server {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        let thread = thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request = String::new();
                if reader.read_line(&mut request).unwrap_or(0) == 0 {
                    continue;
                }
                let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();
                let mut headers = Vec::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some((name, value)) = line.trim_end().split_once(':') {
                        headers.push((name.to_ascii_lowercase(), value.trim().to_string()));
                    }
                }
                if path == "/stop" {
                    break;
                }
                let (mime, body) = match path.split('?').next().unwrap_or("/") {
                    "/root" => ("text/html",r#"<iframe id=remote src='/child'></iframe><iframe id=inline srcdoc='<link rel=stylesheet href="/theme.css"><script src="/relative.js"></script><p>inline</p>'></iframe><script>globalThis.blankReady=document.getElementById("remote").contentDocument.URL==="about:blank"</script>"#),
                    "/child" => ("text/html", "<script>globalThis.done=false;fetch('/data').then(r=>r.text()).then(t=>done=t==='ok')</script><p>remote</p>"),
                    "/relative.js" => ("text/javascript", "globalThis.relative=42;globalThis.styleAtScript=getComputedStyle(document.documentElement).display"),
                    "/theme.css" => ("text/css", "html { display: none }"),
                    "/frames" => ("text/html", "<iframe id=sandboxed sandbox srcdoc='<p>s</p>'></iframe><iframe id=plain srcdoc='<p>p</p>'></iframe>"),
                    _ => ("text/plain", "ok"),
                };
                log.lock().unwrap().push(Seen { path, headers });
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        Self { origin, seen, thread: Some(thread) }
    }

    fn take(&self) -> Vec<Seen> {
        std::mem::take(&mut *self.seen.lock().unwrap())
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Ok(mut stream) = TcpStream::connect(self.origin.trim_start_matches("http://")) {
            let _ = stream.write_all(b"GET /stop HTTP/1.1\r\nHost: localhost\r\n\r\n");
        }
        let _ = self.thread.take().unwrap().join();
    }
}

fn eval(t: &BrowserThread, page: PageId, script: &str) -> String {
    let realm = t.browser.page(page).unwrap().main_frame.main_realm().unwrap();
    let scope = realm.scope().unwrap();
    scope.eval(script, None, 1).unwrap_or_else(|e| panic!("{}", e.details(&scope).text())).to_string().unwrap()
}
#[test]
fn native_child_requests_preserve_frame_loader_headers_origin_and_inherited_base() {
    let server = Server::new();
    let mut t = crate::browser_thread().unwrap();
    t.enable_network(Default::default()).unwrap();
    let page = t.browser.create_page().unwrap();
    let net = t.browser.context_net(pyro_browser::DEFAULT_CONTEXT).unwrap().clone();
    let events = net.subscribe();
    let mut headers = HeaderMap::new();
    headers.insert(HeaderName::from_static("x-frame-test"), HeaderValue::from_static("kept"));
    t.browser.set_extra_http_headers(page, headers);
    t.browser.navigate(page, NavigateOpts::address_bar(format!("{}/root", server.origin))).unwrap();
    let outcome = t.run_until(Some(Instant::now() + Duration::from_secs(5)), |b| {
        let frames = b.frames(page);
        frames.len() == 3
            && frames.iter().all(|f| f.loaded)
            && frames.iter().filter(|f| f.parent.is_some()).all(|f| {
                let frame = b.frame(page, &f.id).unwrap();
                let scope = frame.main_realm().unwrap().scope().unwrap();
                scope
                    .eval(
                        if f.url == "about:srcdoc" { "globalThis.relative===42" } else { "globalThis.done===true" },
                        None,
                        1,
                    )
                    .unwrap()
                    .to_bool()
            })
    });
    assert_eq!(outcome, pyro_runtime::RunOutcome::Done);
    assert_eq!(eval(&t, page, "blankReady"), "true");
    let frames = t.browser.frames(page);
    let remote = frames.iter().find(|f| f.url.ends_with("/child")).unwrap();
    let inline = frames.iter().find(|f| f.url == "about:srcdoc").unwrap();
    assert_eq!(remote.origin, server.origin);
    assert_eq!(inline.origin, server.origin);
    assert_eq!(eval(&t, page, "document.getElementById('inline').contentDocument.URL"), "about:srcdoc");
    assert_eq!(
        eval(&t, page, "document.getElementById('inline').contentDocument.baseURI"),
        format!("{}/root", server.origin)
    );
    assert_eq!(eval(&t, page, "document.getElementById('inline').contentWindow.styleAtScript"), "none");
    assert_ne!(eval(&t, page, "getComputedStyle(document.documentElement).display"), "none");
    let mut requests = Vec::new();
    while let Ok(event) = events.try_recv() {
        if let pyro_net::NetEvent::RequestWillBeSent { request, .. } = event {
            requests.push(request);
        }
    }
    for (path, frame) in [("/child", remote), ("/data", remote), ("/relative.js", inline), ("/theme.css", inline)] {
        let request = requests.iter().find(|r| r.url.path() == path).expect(path);
        assert_eq!(request.frame_id.as_deref(), Some(frame.id.0.as_str()));
        assert_eq!(request.loader_id.as_deref(), Some(frame.loader_id.0.as_str()));
    }
    let seen = server.take();
    assert_eq!(seen.len(), 5);
    assert!(seen.iter().any(|r| r.path == "/relative.js"));
    assert!(seen.iter().all(|r| r.get("x-frame-test") == Some("kept")), "{seen:?}");
    let outcome = t.run_until(Some(Instant::now() + Duration::from_secs(3)), |b| {
        b.frames(page).iter().all(|f| f.network_idle.idle)
    });
    assert_eq!(outcome, pyro_runtime::RunOutcome::Done);
    let loaders: Vec<_> = frames.iter().map(|f| f.loader_id.0.clone()).collect();
    t.browser.close_page(page);
    assert!(loaders.iter().all(|l| net.activity(l).is_none()));
}

#[test]
fn detaching_uncommitted_http_child_forgets_pending_loader_activity() {
    struct PendingServer {
        release: Option<std::sync::mpsc::Sender<()>>,
        thread: Option<JoinHandle<()>>,
    }
    impl Drop for PendingServer {
        fn drop(&mut self) {
            self.release.take();
            self.thread.take().unwrap().join().unwrap();
        }
    }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}/pending", listener.local_addr().unwrap());
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        let stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline => {
                    if rx
                        .recv_timeout(Duration::from_millis(1))
                        .is_err_and(|e| e == std::sync::mpsc::RecvTimeoutError::Disconnected)
                    {
                        return;
                    }
                }
                Err(_) => return,
            }
        };
        // Hold the connection without headers until the test releases it.
        let _stream = stream;
        let _ = rx.recv_timeout(Duration::from_secs(5));
    });
    let _server = PendingServer { release: Some(tx), thread: Some(worker) };
    let mut t = crate::browser_thread().unwrap();
    t.enable_network(Default::default()).unwrap();
    let net = t.browser.context_net(pyro_browser::DEFAULT_CONTEXT).unwrap().clone();
    let events = net.subscribe();
    let page = t.browser.create_page().unwrap();
    eval(
        &t,
        page,
        &format!("globalThis.f=document.createElement('iframe');f.src='{url}';document.body.appendChild(f)"),
    );
    let pending = std::cell::RefCell::new(None);
    assert_eq!(
        t.run_until(Some(Instant::now() + Duration::from_secs(5)), |_| {
            while let Ok(event) = events.try_recv() {
                if let pyro_net::NetEvent::RequestWillBeSent { request, .. } = event {
                    if request.url.as_str() == url {
                        *pending.borrow_mut() = request.loader_id;
                    }
                }
            }
            pending.borrow().as_ref().is_some_and(|id| net.activity(id).is_some_and(|a| a.inflight == 1))
        }),
        pyro_runtime::RunOutcome::Done
    );
    let pending = pending.into_inner().unwrap();
    eval(&t, page, "f.remove()");
    t.browser.checkpoint();
    assert!(net.activity(&pending).is_none());
    assert_eq!(t.browser.frames(page).len(), 1);
}

/// A renderer's sub-resource request for a frame document (a CSS `url()` with request URL
/// modifiers) is that frame's, named by its id: a sandboxed frame's goes out with its opaque
/// origin (`Origin: null`), never the top document's, though both frames' documents have the URL
/// `about:srcdoc`; one naming no frame of the page is not sent. The frame was looked up by its
/// document's URL (the first frame at it, or the top document when the URL was the top's).
#[test]
fn a_renderer_request_is_the_frame_its_client_names() {
    use std::cell::RefCell;
    use std::rc::Rc;
    let server = Server::new();
    let mut t = crate::browser_thread().unwrap();
    t.enable_network(Default::default()).unwrap();
    let page = t.browser.create_page().unwrap();
    t.browser.navigate(page, NavigateOpts::address_bar(format!("{}/frames", server.origin))).unwrap();
    let outcome = t.run_until(Some(Instant::now() + Duration::from_secs(5)), |b| {
        let frames = b.frames(page);
        frames.len() == 3 && frames.iter().all(|f| f.loaded)
    });
    assert_eq!(outcome, pyro_runtime::RunOutcome::Done);
    let frames = t.browser.frames(page);
    let children: Vec<_> = frames.iter().filter(|f| f.parent.is_some()).collect();
    let sandboxed = children.iter().find(|f| f.origin == "null").expect("a sandboxed frame with an opaque origin");
    let plain = children.iter().find(|f| f.origin != "null").expect("a frame of the page's origin");
    assert_eq!((sandboxed.url.as_str(), plain.url.as_str()), ("about:srcdoc", "about:srcdoc"));
    server.take();
    let fetch = |t: &mut BrowserThread, path: &str, client: Option<String>| -> Result<(), String> {
        let result: Rc<RefCell<Option<Result<(), String>>>> = Rc::default();
        let slot = result.clone();
        let options =
            pyro_browser::SubresourceOptions { crossorigin: Some("anonymous".into()), client, ..Default::default() };
        t.browser.fetch_subresource_options(
            page,
            format!("{}{path}", server.origin),
            pyro_browser::ResourceKind::Image,
            options,
            Box::new(move |_, r| *slot.borrow_mut() = Some(r.map(|_| ()))),
        );
        let done = result.clone();
        t.run_until(Some(Instant::now() + Duration::from_secs(5)), move |_| done.borrow().is_some());
        let r = result.borrow_mut().take().expect("the fetch completes");
        r
    };
    let client = |f: &&&pyro_browser::FrameInfo| format!("{}#{}", f.id.0, f.loader_id.0);
    let _ = fetch(&mut t, "/img?sandboxed", Some(client(&sandboxed)));
    let _ = fetch(&mut t, "/img?plain", Some(client(&plain)));
    let unknown = fetch(&mut t, "/img?unknown", Some("not-a-frame#x".into()));
    assert!(unknown.is_err(), "a client naming no frame sends nothing");
    // A document the frame has since navigated away from (another loader id) sends nothing.
    let stale = fetch(&mut t, "/img?stale", Some(format!("{}#not-its-document", plain.id.0)));
    assert!(stale.is_err(), "a client of a replaced document sends nothing");
    let seen = server.take();
    let origin_of =
        |query: &str| seen.iter().find(|r| r.path.ends_with(query)).map(|r| r.get("origin").map(str::to_owned));
    assert_eq!(origin_of("?sandboxed"), Some(Some("null".to_owned())), "{seen:?}");
    assert_ne!(origin_of("?plain"), Some(Some("null".to_owned())), "{seen:?}");
    assert_eq!(origin_of("?unknown"), None, "{seen:?}");
    assert_eq!(origin_of("?stale"), None, "{seen:?}");
}
