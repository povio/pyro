//! End-to-end tests against a local HTTP/1.1 server on an ephemeral port.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use http::Method;
use lp_net::*;

struct Server {
    port: u16,
    log: Arc<Mutex<Vec<String>>>,
    /// `/gated` responds only after `open_gate()`: completion under test control.
    gate: Arc<(Mutex<bool>, std::sync::Condvar)>,
}

impl Server {
    fn open_gate(&self) {
        *self.gate.0.lock().unwrap() = true;
        self.gate.1.notify_all();
    }
}

type Parsed = (String, String, Vec<(String, String)>, Vec<u8>);

fn read_request(stream: &mut TcpStream) -> Option<Parsed> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut line = String::new();
    if reader.read_line(&mut line).ok()? == 0 {
        return None;
    }
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_owned();
    let path = parts.next()?.to_owned();
    let mut headers = Vec::new();
    loop {
        let mut h = String::new();
        reader.read_line(&mut h).ok()?;
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        let (k, v) = h.split_once(':')?;
        headers.push((k.trim().to_ascii_lowercase(), v.trim().to_owned()));
    }
    let len = headers.iter().find(|(k, _)| k == "content-length").and_then(|(_, v)| v.parse().ok()).unwrap_or(0);
    let mut body = vec![0; len];
    reader.read_exact(&mut body).ok()?;
    Some((method, path, headers, body))
}

fn respond(stream: &mut TcpStream, status: &str, headers: &[(&str, String)], body: &[u8]) {
    let mut out = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\n", body.len());
    for (k, v) in headers {
        out.push_str(&format!("{k}: {v}\r\n"));
    }
    out.push_str("\r\n");
    let _ = stream.write_all(out.as_bytes());
    let _ = stream.write_all(body);
}

fn start_server() -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let log = Arc::new(Mutex::new(Vec::new()));
    let log2 = log.clone();
    let gate: Arc<(Mutex<bool>, std::sync::Condvar)> = Arc::default();
    let gate2 = gate.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let log = log2.clone();
            let gate = gate2.clone();
            std::thread::spawn(move || {
                while let Some((method, path, headers, body)) = read_request(&mut stream) {
                    let hv = |k: &str| headers.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone()).unwrap_or_default();
                    log.lock().unwrap().push(format!(
                        "{method} {path} cookie={} origin={}",
                        hv("cookie"),
                        hv("origin")
                    ));
                    match (method.as_str(), path.as_str()) {
                        (_, "/auth-start") => respond(
                            &mut stream,
                            "302 Found",
                            &[("Location", format!("http://localhost:{port}/auth-end"))],
                            b"",
                        ),
                        ("OPTIONS", "/auth-end") => respond(
                            &mut stream,
                            "204 No Content",
                            &[
                                ("Access-Control-Allow-Origin", hv("origin")),
                                ("Access-Control-Allow-Headers", "authorization".into()),
                            ],
                            b"",
                        ),
                        (_, "/auth-end") => respond(
                            &mut stream,
                            "200 OK",
                            &[("Access-Control-Allow-Origin", hv("origin"))],
                            hv("authorization").as_bytes(),
                        ),

                        (_, "/plain") => respond(
                            &mut stream,
                            "200 OK",
                            &[("Content-Type", "text/plain; charset=utf-8".into())],
                            b"hello",
                        ),
                        (_, "/gzip") => {
                            let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
                            e.write_all(&b"zipped ".repeat(1000)).unwrap();
                            let z = e.finish().unwrap();
                            respond(&mut stream, "200 OK", &[("Content-Encoding", "gzip".into())], &z);
                        }
                        (_, "/redirect") => respond(&mut stream, "302 Found", &[("Location", "/plain".into())], b""),
                        (_, "/loop") => respond(&mut stream, "302 Found", &[("Location", "/loop".into())], b""),
                        ("POST", "/see-other") => {
                            respond(&mut stream, "303 See Other", &[("Location", "/echo".into())], b"")
                        }
                        (_, "/echo") => {
                            let b = format!("{method} {}", String::from_utf8_lossy(&body));
                            respond(&mut stream, "200 OK", &[], b.as_bytes())
                        }
                        (_, "/set-cookie") => respond(
                            &mut stream,
                            "200 OK",
                            &[("Set-Cookie", "sid=abc; Path=/; HttpOnly".into()), ("Set-Cookie", "theme=dark".into())],
                            b"ok",
                        ),
                        (_, "/cookie") => respond(&mut stream, "200 OK", &[], hv("cookie").as_bytes()),
                        (_, "/headers") => {
                            let all: String = headers.iter().map(|(k, v)| format!("{k}: {v}\n")).collect();
                            respond(&mut stream, "200 OK", &[], all.as_bytes())
                        }
                        ("OPTIONS", "/api") => respond(
                            &mut stream,
                            "204 No Content",
                            &[
                                ("Access-Control-Allow-Origin", hv("origin")),
                                ("Access-Control-Allow-Credentials", "true".into()),
                                ("Access-Control-Allow-Methods", "GET, PUT".into()),
                                ("Access-Control-Allow-Headers", "content-type, x-token".into()),
                                ("Access-Control-Max-Age", "60".into()),
                            ],
                            b"",
                        ),
                        (_, "/api") => respond(
                            &mut stream,
                            "200 OK",
                            &[
                                ("Access-Control-Allow-Origin", hv("origin")),
                                ("Access-Control-Allow-Credentials", "true".into()),
                            ],
                            format!("{method} {}", hv("x-token")).as_bytes(),
                        ),
                        (_, "/no-cors-headers") => respond(&mut stream, "200 OK", &[], b"secret"),
                        (_, "/gated") => {
                            let (open, cv) = &*gate;
                            let mut open = open.lock().unwrap();
                            while !*open {
                                open = cv.wait(open).unwrap();
                            }
                            drop(open);
                            respond(&mut stream, "200 OK", &[], b"released");
                        }
                        (_, "/slow") => {
                            std::thread::sleep(Duration::from_secs(5));
                            respond(&mut stream, "200 OK", &[], b"late");
                        }
                        _ => respond(&mut stream, "404 Not Found", &[], b"nope"),
                    }
                }
            });
        }
    });
    Server { port, log, gate }
}

fn url(s: &Server, path: &str) -> Url {
    Url::parse(&format!("http://127.0.0.1:{}{}", s.port, path)).unwrap()
}

fn collect(client: &NetClient, req: Request) -> (Vec<String>, Option<ResponseHead>, Vec<u8>, Option<NetError>) {
    let (tx, rx) = mpsc::channel();
    client.fetch(req, Box::new(move |ev| tx.send(ev).unwrap()));
    let mut kinds = Vec::new();
    let mut head = None;
    let mut body = Vec::new();
    loop {
        match rx.recv_timeout(Duration::from_secs(10)).expect("event") {
            ResponseEvent::Redirect { to, .. } => kinds.push(format!("redirect {}", to.path())),
            ResponseEvent::Response(h) => {
                kinds.push(format!("response {}", h.status.as_u16()));
                head = Some(h);
            }
            ResponseEvent::Data(d) => body.extend_from_slice(&d),
            ResponseEvent::Finished { .. } => {
                kinds.push("finished".into());
                return (kinds, head, body, None);
            }
            ResponseEvent::Failed(e) => {
                kinds.push("failed".into());
                return (kinds, head, body, Some(e));
            }
        }
    }
}

#[test]
fn plain_gzip_redirect_and_errors() {
    let s = start_server();
    let c = NetClient::new(NetConfig::default()).unwrap();
    let (k, h, b, _) = collect(&c, Request::get(url(&s, "/plain"), Destination::Fetch));
    assert_eq!(k, ["response 200", "finished"]);
    let h = h.unwrap();
    assert_eq!((h.mime.as_str(), h.charset.as_deref()), ("text/plain", Some("utf-8")));
    assert_eq!(b, b"hello");

    let (_, _, b, _) = collect(&c, Request::get(url(&s, "/gzip"), Destination::Fetch));
    assert_eq!(b, b"zipped ".repeat(1000));

    let (k, h, b, _) = collect(&c, Request::get(url(&s, "/redirect"), Destination::Document));
    assert_eq!(k, ["redirect /plain", "response 200", "finished"]);
    assert!(h.unwrap().redirected);
    assert_eq!(b, b"hello");

    let (_, _, _, e) = collect(&c, Request::get(url(&s, "/loop"), Destination::Fetch));
    assert_eq!(e, Some(NetError::TooManyRedirects));

    let mut post = Request::get(url(&s, "/see-other"), Destination::Fetch);
    post.method = Method::POST;
    post.body = Some(Bytes::from_static(b"payload"));
    let (_, _, b, _) = collect(&c, post);
    assert_eq!(b, b"GET ", "303 switches to GET without body");

    let (_, _, _, e) = collect(&c, Request::get(Url::parse("http://127.0.0.1:1/").unwrap(), Destination::Fetch));
    assert_eq!(e, Some(NetError::ConnectionRefused));

    let (k, _, b, _) = collect(&c, Request::get(Url::parse("data:text/plain,hi%20there").unwrap(), Destination::Fetch));
    assert_eq!(k, ["response 200", "finished"]);
    assert_eq!(b, b"hi there");
}

#[test]
fn cookies_roundtrip() {
    let s = start_server();
    let c = NetClient::new(NetConfig::default()).unwrap();
    collect(&c, Request::get(url(&s, "/set-cookie"), Destination::Document));
    let (_, _, b, _) = collect(&c, Request::get(url(&s, "/cookie"), Destination::Document));
    assert_eq!(String::from_utf8(b).unwrap(), "sid=abc; theme=dark");
    let doc_cookie = c.cookies().lock().unwrap().document_cookie(&url(&s, "/"), lp_net::cookies::now_secs());
    assert_eq!(doc_cookie, "theme=dark", "HttpOnly hidden from script");
    let mut omit = Request::get(url(&s, "/cookie"), Destination::Fetch);
    omit.credentials = Credentials::Omit;
    let (_, _, b, _) = collect(&c, omit);
    assert!(b.is_empty());
}

#[test]
fn cors_preflight_and_rejection() {
    let s = start_server();
    let c = NetClient::new(NetConfig::default()).unwrap();
    let origin = "http://localhost:5173".to_string();
    let mut req = Request::get(url(&s, "/api"), Destination::Fetch);
    req.method = Method::PUT;
    req.origin = Some(origin.clone());
    req.credentials = Credentials::Include;
    req.headers.insert("x-token", "t1".parse().unwrap());
    let (k, h, b, e) = collect(&c, req.clone());
    assert_eq!(e, None, "{k:?}");
    assert_eq!(h.unwrap().kind, ResponseKind::Cors);
    assert_eq!(b, b"PUT t1");
    // Second request uses the preflight cache (max-age 60).
    collect(&c, req.clone());
    let log = s.log.lock().unwrap().clone();
    assert_eq!(log.iter().filter(|l| l.starts_with("OPTIONS /api")).count(), 1, "{log:?}");
    assert!(log.iter().any(|l| l.starts_with("PUT /api") && l.contains("origin=http://localhost:5173")));

    // Disallowed method fails the preflight.
    let mut del = req.clone();
    del.method = Method::DELETE;
    let (_, _, _, e) = collect(&c, del);
    assert!(matches!(e, Some(NetError::Cors(_))), "{e:?}");

    // Missing ACAO fails the response check and exposes nothing.
    let mut nocors = Request::get(url(&s, "/no-cors-headers"), Destination::Fetch);
    nocors.origin = Some(origin);
    let (k, _, b, e) = collect(&c, nocors);
    assert!(matches!(e, Some(NetError::Cors(_))));
    assert_eq!(k, ["failed"]);
    assert!(b.is_empty());
}

#[test]
fn cancel_and_observer() {
    let s = start_server();
    let c = NetClient::new(NetConfig::default()).unwrap();
    let events = c.subscribe();
    let (tx, rx) = mpsc::channel();
    let id = c.fetch(Request::get(url(&s, "/slow"), Destination::Xhr), Box::new(move |ev| tx.send(ev).unwrap()));
    std::thread::sleep(Duration::from_millis(100));
    c.cancel(id);
    match rx.recv_timeout(Duration::from_secs(2)).unwrap() {
        ResponseEvent::Failed(NetError::Aborted) => {}
        other => panic!("unexpected {other:?}"),
    }
    let mut kinds = Vec::new();
    while let Ok(ev) = events.try_recv() {
        kinds.push(match ev {
            NetEvent::RequestWillBeSent { request, .. } => format!("will {}", request.url.path()),
            NetEvent::LoadingFailed { canceled, .. } => format!("failed canceled={canceled}"),
            other => format!("{other:?}"),
        });
    }
    assert_eq!(kinds, ["will /slow", "failed canceled=true"]);

    // Interceptor fulfills without touching the network.
    struct Fulfill;
    impl Interceptor for Fulfill {
        fn paused(&self, _req: PausedRequest, reply: tokio::sync::oneshot::Sender<InterceptDecision>) {
            let _ = reply.send(InterceptDecision::Fulfill {
                status: 201,
                headers: Default::default(),
                body: Bytes::from_static(b"mock"),
            });
        }
    }
    c.set_interceptor(Some(Arc::new(Fulfill)));
    let (k, _, b, _) = collect(&c, Request::get(url(&s, "/plain"), Destination::Fetch));
    assert_eq!(k, ["response 201", "finished"]);
    assert_eq!(b, b"mock");
}

#[test]
fn drop_fails_in_flight_requests() {
    let s = start_server();
    let c = NetClient::new(NetConfig::default()).unwrap();
    let events = c.subscribe();
    let (tx, rx) = mpsc::channel();
    c.fetch(Request::get(url(&s, "/slow"), Destination::Fetch), Box::new(move |ev| tx.send(ev).unwrap()));
    std::thread::sleep(Duration::from_millis(100));
    drop(c);
    match rx.recv_timeout(Duration::from_secs(2)).unwrap() {
        ResponseEvent::Failed(NetError::Aborted) => {}
        other => panic!("unexpected {other:?}"),
    }
    assert!(rx.recv_timeout(Duration::from_millis(200)).is_err(), "exactly one terminal event");
    let mut failures = 0;
    loop {
        match events.try_recv() {
            Ok(NetEvent::LoadingFailed {
                error: NetError::Aborted,
                canceled: true,
                destination: Destination::Fetch,
                ..
            }) => failures += 1,
            Ok(NetEvent::RequestWillBeSent { .. }) => {}
            Err(SubscriptionError::Closed) => break,
            other => panic!("unexpected shutdown observation {other:?}"),
        }
    }
    assert_eq!(failures, 1);
}

#[test]
fn regression_opaque_origin_requires_cors() {
    let s = start_server();
    let c = NetClient::new(NetConfig::default()).unwrap();
    let mut r = Request::get(url(&s, "/no-cors-headers"), Destination::Fetch);
    r.origin = Some("null".into());
    let (_, h, b, e) = collect(&c, r);
    assert!(matches!(e, Some(NetError::Cors(_))), "opaque origin exposed head={h:?} body={b:?} err={e:?}");
}
#[test]
fn regression_same_origin_mode_blocks_cross_origin() {
    let s = start_server();
    let c = NetClient::new(NetConfig::default()).unwrap();
    let mut r = Request::get(url(&s, "/no-cors-headers"), Destination::Fetch);
    r.origin = Some("http://localhost:1".into());
    r.mode = RequestMode::SameOrigin;
    let (_, h, b, e) = collect(&c, r);
    assert!(e.is_some(), "same-origin request exposed head={h:?} body={b:?}");
}
#[test]
fn regression_nocors_sameorigin_credentials() {
    let s = start_server();
    let c = NetClient::new(NetConfig::default()).unwrap();
    c.cookies().lock().unwrap().set_from_header(
        &url(&s, "/"),
        "sid=secret; Path=/",
        lp_net::cookies::Access::Http,
        lp_net::cookies::now_secs(),
    );
    let mut r = Request::get(url(&s, "/cookie"), Destination::Fetch);
    r.origin = Some("http://127.0.0.1:1".into());
    r.mode = RequestMode::NoCors;
    let (_, _, b, e) = collect(&c, r);
    assert_eq!(e, None);
    assert!(b.is_empty(), "cross-origin SameOrigin credentials sent {}", String::from_utf8_lossy(&b));
}
#[test]
fn regression_authorization_stripped_on_cross_origin_redirect() {
    let s = start_server();
    let c = NetClient::new(NetConfig::default()).unwrap();
    let mut r = Request::get(url(&s, "/auth-start"), Destination::Fetch);
    r.origin = Some(url(&s, "/").origin().ascii_serialization());
    r.headers.insert("authorization", "Bearer secret".parse().unwrap());
    let (_, _, b, e) = collect(&c, r);
    assert_eq!(e, None);
    assert!(b.is_empty(), "redirect leaked {}", String::from_utf8_lossy(&b));
}
#[test]
fn regression_data_body_cap() {
    let c = NetClient::new(NetConfig::default()).unwrap();
    let mut r = Request::get(Url::parse("data:,123456789").unwrap(), Destination::Fetch);
    r.max_body = Some(1);
    let (_, _, b, e) = collect(&c, r);
    assert_eq!(e, Some(NetError::TooLarge), "delivered {} bytes beyond max_body", b.len());
}
#[test]
fn regression_secure_cookie_overlay() {
    use lp_net::cookies::{Access, CookieJar, SiteContext};
    let mut jar = CookieJar::new();
    let https = Url::parse("https://example.com/").unwrap();
    let http = Url::parse("http://example.com/").unwrap();
    assert!(jar.set_from_header(&https, "sid=trusted; Secure; Path=/", Access::Http, 1.0));
    assert!(
        !jar.set_from_header(&http, "sid=attacker; Path=/", Access::Http, 1.0),
        "insecure response replaced Secure cookie; now {:?}",
        jar.header_for(&https, SiteContext::SameSite, 1.0)
    );
}
#[test]
fn regression_cookie_domain_bucket_cap() {
    use lp_net::cookies::{Access, CookieJar, SiteContext};
    let mut jar = CookieJar::new();
    for i in 0..10000 {
        let u = Url::parse(&format!("https://h{i}.example.com/")).unwrap();
        assert!(jar.set_from_header(&u, "x=1; Max-Age=1", Access::Http, 1.0));
        jar.header_for(&u, SiteContext::SameSite, 3.0);
    }
    assert!(jar.is_empty());
}
#[test]
fn regression_public_suffix_cookie_isolation() {
    use lp_net::cookies::{Access, CookieJar, SiteContext};
    let mut jar = CookieJar::new();
    let evil = Url::parse("https://evil.co.uk/").unwrap();
    let bank = Url::parse("https://bank.co.uk/").unwrap();
    let accepted = jar.set_from_header(&evil, "sid=attacker; Domain=co.uk; Path=/; SameSite=Strict", Access::Http, 1.0);
    let h = jar.header_for(&bank, SiteContext::SameSite, 1.0);
    assert!(!accepted, "public suffix cookie accepted and sent to bank: {h:?}");
}

#[test]
fn observer_starts_data_and_early_failure_requests() {
    for (url, offline, same_origin, expected) in [
        ("data:text/plain,hello", false, false, vec!["start", "response", "data", "finished"]),
        ("http://127.0.0.1:1/", true, false, vec!["start", "failed"]),
        ("https://example.test/", false, false, vec!["start", "failed"]),
        ("http://127.0.0.1:1/", false, true, vec!["start", "failed"]),
    ] {
        let client = NetClient::new(NetConfig::default()).unwrap();
        client.update_settings(|s| s.offline = offline);
        let events = client.subscribe();
        let mut request = Request::get(Url::parse(url).unwrap(), Destination::Fetch);
        if same_origin {
            request.mode = RequestMode::SameOrigin;
            request.origin = Some("http://elsewhere.test".into());
        }
        collect(&client, request);
        let mut observed = Vec::new();
        while let Ok(event) = events.try_recv() {
            observed.push(match event {
                NetEvent::RequestWillBeSent { .. } => "start",
                NetEvent::ResponseReceived { .. } => "response",
                NetEvent::DataReceived { .. } => "data",
                NetEvent::LoadingFinished { .. } => "finished",
                NetEvent::LoadingFailed { .. } => "failed",
            });
        }
        assert_eq!(observed, expected, "{url} offline={offline} same_origin={same_origin}");
    }
}

#[test]
fn page_extra_headers_are_per_request_and_take_precedence() {
    let s = start_server();
    let client = NetClient::new(NetConfig::default()).unwrap();
    client.update_settings(|st| {
        st.extra_headers.insert("x-context", "context".parse().unwrap());
        st.extra_headers.insert("x-shared", "context".parse().unwrap());
    });
    let send = |page: &[(&'static str, &'static str)]| {
        let mut req = Request::get(url(&s, "/headers"), Destination::Script);
        req.headers.insert("x-shared", "explicit".parse().unwrap());
        req.headers.insert("x-explicit", "explicit".parse().unwrap());
        req.headers.insert("user-agent", "explicit-agent".parse().unwrap());
        for (k, v) in page {
            req.defaults.extra_headers.insert(*k, v.parse().unwrap());
        }
        let (_, _, body, err) = collect(&client, req);
        assert!(err.is_none());
        let text = String::from_utf8(body).unwrap();
        let lines: Vec<String> = text.lines().map(str::to_owned).collect();
        lines
    };
    let page = send(&[("x-page", "page"), ("x-shared", "page"), ("user-agent", "page-agent")]);
    for expected in
        ["x-page: page", "x-shared: page", "x-context: context", "x-explicit: explicit", "user-agent: page-agent"]
    {
        assert!(page.iter().any(|l| l == expected), "missing {expected:?} in {page:?}");
    }
    assert_eq!(page.iter().filter(|l| l.starts_with("x-shared:")).count(), 1, "replaced, not appended: {page:?}");
    // Another request on the same client without page defaults: no page header leaks;
    // the context value still replaces the explicit one, as before this change.
    let other = send(&[]);
    assert!(!other.iter().any(|l| l.starts_with("x-page:")), "{other:?}");
    for expected in ["x-shared: context", "x-context: context", "user-agent: explicit-agent"] {
        assert!(other.iter().any(|l| l == expected), "missing {expected:?} in {other:?}");
    }
}

#[test]
fn loader_activity_counts_synchronously_and_each_terminal_once() {
    let s = start_server();
    let c = NetClient::new(NetConfig::default()).unwrap();
    let start = |path: &str, loader: Option<&str>| {
        let mut req = Request::get(url(&s, path), Destination::Script);
        req.loader_id = loader.map(str::to_owned);
        let (tx, rx) = mpsc::channel();
        let id = c.fetch(
            req,
            Box::new(move |ev| {
                let _ = tx.send(ev);
            }),
        );
        (id, rx)
    };
    let settle = |loader: &str, want: usize| {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while c.activity(loader).map(|a| a.inflight) != Some(want) {
            assert!(std::time::Instant::now() < deadline, "{loader}: {:?}", c.activity(loader));
            std::thread::sleep(Duration::from_millis(1));
        }
        c.activity(loader).unwrap()
    };
    // Counted on the caller's thread, before the I/O thread sees the request.
    let slow: Vec<_> = (0..3).map(|_| start("/slow", Some("L1"))).collect();
    let a = c.activity("L1").unwrap();
    assert_eq!((a.inflight, a.idle_since, a.almost_idle_since), (3, None, None));
    // Cancel is a terminal: exactly one decrement each, and the thresholds are stamped.
    c.cancel(slow[0].0);
    let a = settle("L1", 2);
    assert!(a.almost_idle_since.is_some() && a.idle_since.is_none());
    c.cancel(slow[1].0);
    c.cancel(slow[2].0);
    let a = settle("L1", 0);
    assert!(a.idle_since.is_some() && a.almost_idle_since.is_some());
    // A new request clears idle_since; held server-side until the gate opens, so the
    // assertion cannot race its completion. Finished is then a terminal too.
    let (_, rx) = start("/gated", Some("L1"));
    let a = c.activity("L1").unwrap();
    assert_eq!((a.inflight, a.idle_since), (1, None));
    s.open_gate();
    while !matches!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), ResponseEvent::Finished { .. }) {}
    let a = settle("L1", 0);
    assert!(a.idle_since.is_some(), "the finished request stamped idle again");
    // Requests without a loader are not counted.
    let (_, rx) = start("/plain", None);
    while !matches!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), ResponseEvent::Finished { .. }) {}
    assert!(c.activity("").is_none());
    // Forgetting a loader detaches its in-flight requests from any later entry.
    let (old, _rx_old) = start("/slow", Some("L2"));
    c.forget_activity("L2");
    assert!(c.activity("L2").is_none());
    let (new, _rx_new) = start("/slow", Some("L2"));
    c.cancel(old);
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(c.activity("L2").unwrap().inflight, 1, "the forgotten request does not decrement the new entry");
    c.cancel(new);
    settle("L2", 0);
}

#[test]
fn event_stream_activity_is_separate_and_cancellation_clears_both_counts() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (stop, stopped) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        read_request(&mut stream).unwrap();
        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n3\r\n:\n\n\r\n").unwrap();
        stream.flush().unwrap();
        let _ = stopped.recv_timeout(Duration::from_secs(5));
    });
    let client = NetClient::new(NetConfig::default()).unwrap();
    let mut req = Request::get(Url::parse(&format!("http://127.0.0.1:{port}/events")).unwrap(), Destination::Fetch);
    req.loader_id = Some("sse".into());
    let (tx, rx) = mpsc::channel();
    let id = client.fetch(
        req,
        Box::new(move |event| {
            let _ = tx.send(event);
        }),
    );
    loop {
        if matches!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), ResponseEvent::Response(_)) {
            break;
        }
    }
    let active = client.activity("sse").unwrap();
    assert_eq!((active.inflight, active.event_streams), (1, 1));
    assert!(active.idle_since.is_none());
    client.cancel(id);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while client.activity("sse").unwrap().inflight != 0 {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(client.activity("sse").unwrap().event_streams, 0);
    let _ = stop.send(());
    server.join().unwrap();
}
