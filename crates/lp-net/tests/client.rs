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
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let log = log2.clone();
            std::thread::spawn(move || {
                while let Some((method, path, headers, body)) = read_request(&mut stream) {
                    let hv = |k: &str| headers.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone()).unwrap_or_default();
                    log.lock().unwrap().push(format!(
                        "{method} {path} cookie={} origin={}",
                        hv("cookie"),
                        hv("origin")
                    ));
                    match (method.as_str(), path.as_str()) {
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
    Server { port, log }
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
