//! HTTP cache against a local HTTP/1.1 server that counts and records every
//! request it sees. Cold, warm and revalidated paths, isolation and bounds.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use http::Method;
use lp_net::*;

type Seen = Arc<Mutex<Vec<(String, String, Vec<(String, String)>)>>>;

struct Server {
    port: u16,
    seen: Seen,
}

impl Server {
    fn hits(&self, path: &str) -> usize {
        self.seen.lock().unwrap().iter().filter(|(_, p, _)| p == path).count()
    }
    /// Header value of the n-th request to `path`.
    fn header(&self, path: &str, n: usize, name: &str) -> Option<String> {
        let seen = self.seen.lock().unwrap();
        let (_, _, h) = seen.iter().filter(|(_, p, _)| p == path).nth(n)?;
        h.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone())
    }
    fn url(&self, path: &str) -> Url {
        Url::parse(&format!("http://127.0.0.1:{}{}", self.port, path)).unwrap()
    }
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

fn gzip(data: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

fn start() -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen: Seen = Arc::default();
    let changed = Arc::new(AtomicUsize::new(0));
    let (log, changed2) = (seen.clone(), changed.clone());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let (log, changed) = (log.clone(), changed2.clone());
            std::thread::spawn(move || loop {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    return;
                }
                let mut parts = line.split_whitespace();
                let (method, target) = (parts.next().unwrap().to_owned(), parts.next().unwrap().to_owned());
                let path = target.split('?').next().unwrap().to_owned();
                let mut headers = Vec::new();
                loop {
                    let mut h = String::new();
                    reader.read_line(&mut h).unwrap();
                    let h = h.trim_end();
                    if h.is_empty() {
                        break;
                    }
                    let (k, v) = h.split_once(':').unwrap();
                    headers.push((k.trim().to_ascii_lowercase(), v.trim().to_owned()));
                }
                let hv = |k: &str| headers.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
                let len = hv("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
                let mut body = vec![0; len];
                reader.read_exact(&mut body).unwrap();
                log.lock().unwrap().push((method.clone(), path.clone(), headers.clone()));
                let fresh = ("Cache-Control", "max-age=3600".to_owned());
                let s = &mut stream;
                match (method.as_str(), path.as_str()) {
                    ("POST", _) => respond(s, "200 OK", &[], b"posted"),
                    (_, "/fresh") => respond(s, "200 OK", &[fresh], b"fresh-body"),
                    (_, "/etag") if hv("if-none-match").as_deref() == Some("\"v1\"") => respond(
                        s,
                        "304 Not Modified",
                        &[("ETag", "\"v1\"".into()), ("X-Revalidated", "yes".into())],
                        b"",
                    ),
                    (_, "/etag") => respond(
                        s,
                        "200 OK",
                        &[("ETag", "\"v1\"".into()), ("Cache-Control", "no-cache".into())],
                        b"etag-body",
                    ),
                    (_, "/lm") if hv("if-modified-since").is_some() => respond(s, "304 Not Modified", &[], b""),
                    (_, "/lm") => respond(
                        s,
                        "200 OK",
                        &[
                            ("Last-Modified", "Sun, 06 Nov 1994 08:49:37 GMT".into()),
                            ("Cache-Control", "max-age=0".into()),
                        ],
                        b"lm-body",
                    ),
                    (_, "/changed") => match changed.fetch_add(1, Ordering::SeqCst) {
                        0 => respond(
                            s,
                            "200 OK",
                            &[("ETag", "\"v1\"".into()), ("Cache-Control", "no-cache".into())],
                            b"v1-body",
                        ),
                        1 => respond(s, "304 Not Modified", &[("ETag", "\"v2\"".into())], b""),
                        _ => respond(
                            s,
                            "200 OK",
                            &[("ETag", "\"v2\"".into()), ("Cache-Control", "no-cache".into())],
                            b"v2-body",
                        ),
                    },
                    (_, "/nostore") => {
                        respond(s, "200 OK", &[("Cache-Control", "no-store, max-age=3600".into())], b"x")
                    }
                    (_, "/cookie") => respond(s, "200 OK", &[fresh, ("Set-Cookie", "c=1".into())], b"x"),
                    (_, "/none") => respond(s, "200 OK", &[], b"x"),
                    (_, "/vary") => respond(
                        s,
                        "200 OK",
                        &[fresh, ("Vary", "Accept-Language".into())],
                        hv("accept-language").unwrap_or_default().as_bytes(),
                    ),
                    (_, "/auth") => respond(s, "200 OK", &[fresh], hv("authorization").unwrap_or_default().as_bytes()),
                    (_, "/big") => respond(s, "200 OK", &[fresh], &[b'b'; 4096]),
                    (_, "/gzip") => {
                        respond(s, "200 OK", &[fresh, ("Content-Encoding", "gzip".into())], &gzip(b"inflated"))
                    }
                    _ => respond(s, "200 OK", &[fresh], b"other"),
                }
            });
        }
    });
    Server { port, seen }
}

struct Got {
    head: Option<ResponseHead>,
    body: Vec<u8>,
    encoded: u64,
    error: Option<NetError>,
}

fn fetch(client: &NetClient, req: Request) -> Got {
    let (tx, rx) = mpsc::channel();
    client.fetch(req, Box::new(move |ev| tx.send(ev).unwrap()));
    let mut got = Got { head: None, body: Vec::new(), encoded: 0, error: None };
    loop {
        match rx.recv_timeout(Duration::from_secs(10)).expect("event") {
            ResponseEvent::Response(h) => got.head = Some(h),
            ResponseEvent::Data(d) => got.body.extend_from_slice(&d),
            ResponseEvent::Finished { encoded_len, .. } => {
                got.encoded = encoded_len;
                return got;
            }
            ResponseEvent::Failed(e) => {
                got.error = Some(e);
                return got;
            }
            ResponseEvent::Redirect { .. } => {}
        }
    }
}

fn get(s: &Server, client: &NetClient, path: &str) -> Got {
    fetch(client, Request::get(s.url(path), Destination::Script))
}

fn with(s: &Server, path: &str, f: impl FnOnce(&mut Request)) -> Request {
    let mut r = Request::get(s.url(path), Destination::Script);
    f(&mut r);
    r
}

fn client() -> NetClient {
    NetClient::new(NetConfig::default()).unwrap()
}

#[test]
fn cold_then_warm_fresh_hit_skips_the_network() {
    let s = start();
    let c = client();
    let cold = get(&s, &c, "/fresh");
    assert_eq!((cold.body.as_slice(), cold.head.as_ref().unwrap().from_cache), (&b"fresh-body"[..], false));
    assert!(cold.encoded > 0);
    let warm = get(&s, &c, "/fresh#ignored-fragment");
    let head = warm.head.unwrap();
    assert_eq!((warm.body.as_slice(), head.from_cache, head.status.as_u16()), (&b"fresh-body"[..], true, 200));
    assert_eq!(warm.encoded, 0, "no network bytes for a hit");
    assert!(head.headers.contains_key("age"));
    assert_eq!(s.hits("/fresh"), 1, "one network request for two loads");
    assert_eq!(c.http_cache_usage().0, 1);

    // Decoded bodies are stored; a hit needs no content decoding.
    assert_eq!(get(&s, &c, "/gzip").body, b"inflated");
    assert_eq!(get(&s, &c, "/gzip").body, b"inflated");
    assert_eq!(s.hits("/gzip"), 1);
}

#[test]
fn revalidation_sends_validators_and_serves_the_stored_body_on_304() {
    let s = start();
    let c = client();
    assert_eq!(get(&s, &c, "/etag").body, b"etag-body");
    let r = get(&s, &c, "/etag");
    let head = r.head.unwrap();
    assert_eq!((r.body.as_slice(), head.status.as_u16(), head.from_cache), (&b"etag-body"[..], 200, false));
    assert_eq!(head.headers["x-revalidated"], "yes", "304 header fields are merged");
    assert_eq!(s.hits("/etag"), 2);
    assert_eq!(s.header("/etag", 0, "if-none-match"), None);
    assert_eq!(s.header("/etag", 1, "if-none-match").as_deref(), Some("\"v1\""));

    assert_eq!(get(&s, &c, "/lm").body, b"lm-body");
    assert_eq!(get(&s, &c, "/lm").body, b"lm-body");
    assert_eq!(s.header("/lm", 1, "if-modified-since").as_deref(), Some("Sun, 06 Nov 1994 08:49:37 GMT"));

    // A 304 whose strong ETag no longer matches the entry: refetch unconditionally.
    assert_eq!(get(&s, &c, "/changed").body, b"v1-body");
    assert_eq!(get(&s, &c, "/changed").body, b"v2-body");
    assert_eq!(s.hits("/changed"), 3);
    assert_eq!(s.header("/changed", 2, "if-none-match"), None);
}

#[test]
fn unstorable_responses_always_go_to_the_network() {
    let s = start();
    let c = client();
    for path in ["/nostore", "/cookie", "/none"] {
        get(&s, &c, path);
        get(&s, &c, path);
        assert_eq!(s.hits(path), 2, "{path}");
    }
    assert_eq!(c.http_cache_usage(), (0, 0));
}

#[test]
fn vary_selects_the_matching_variant() {
    let s = start();
    let c = client();
    let lang = |l: &'static str| with(&s, "/vary", |r| r.defaults.accept_language = Some(l.parse().unwrap()));
    assert_eq!(fetch(&c, lang("en")).body, b"en");
    assert_eq!(fetch(&c, lang("de")).body, b"de");
    let again = fetch(&c, lang("en"));
    assert_eq!((again.body.as_slice(), again.head.unwrap().from_cache), (&b"en"[..], true));
    assert_eq!(s.hits("/vary"), 2);
}

#[test]
fn request_directives_and_cache_modes() {
    let s = start();
    let c = client();
    get(&s, &c, "/fresh");
    let cc = |v: &'static str| {
        with(&s, "/fresh", |r| {
            r.headers.insert("cache-control", v.parse().unwrap());
        })
    };
    // no-cache / max-age=0 / Pragma force the network; no validators on /fresh, so unconditional.
    fetch(&c, cc("no-cache"));
    fetch(&c, cc("max-age=0"));
    fetch(
        &c,
        with(&s, "/fresh", |r| {
            r.headers.insert("pragma", "no-cache".parse().unwrap());
        }),
    );
    assert_eq!(s.hits("/fresh"), 4);
    assert!(get(&s, &c, "/fresh").head.unwrap().from_cache, "responses fetched under no-cache are stored");

    // no-store: neither used nor stored.
    c.clear_http_cache();
    fetch(&c, cc("no-store"));
    assert_eq!(c.http_cache_usage().0, 0);

    // Fetch modes.
    let mode = |path: &str, m: CacheMode| with(&s, path, |r| r.cache = m);
    get(&s, &c, "/fresh");
    let before = s.hits("/fresh");
    fetch(&c, mode("/fresh", CacheMode::NoStore));
    assert_eq!(s.header("/fresh", before, "pragma").as_deref(), Some("no-cache"));
    assert_eq!(s.header("/fresh", before, "cache-control").as_deref(), Some("no-cache"));
    fetch(&c, mode("/fresh", CacheMode::Reload));
    assert_eq!(s.hits("/fresh"), before + 2);
    assert!(fetch(&c, mode("/fresh", CacheMode::ForceCache)).head.unwrap().from_cache);
    assert!(fetch(&c, mode("/fresh", CacheMode::OnlyIfCached)).head.unwrap().from_cache);
    assert_eq!(s.hits("/fresh"), before + 2);
    let miss = fetch(&c, mode("/never", CacheMode::OnlyIfCached));
    assert_eq!(miss.error, Some(NetError::Other("net::ERR_CACHE_MISS".into())));
    assert_eq!(s.hits("/never"), 0);
    // no-cache mode revalidates, with Cache-Control: max-age=0 added.
    get(&s, &c, "/etag");
    fetch(&c, mode("/etag", CacheMode::NoCache));
    assert_eq!(s.header("/etag", 1, "cache-control").as_deref(), Some("max-age=0"));
    assert_eq!(s.header("/etag", 1, "if-none-match").as_deref(), Some("\"v1\""));

    // A page's own conditional request passes through, and its 304 is delivered as such.
    let own = fetch(
        &c,
        with(&s, "/etag", |r| {
            r.headers.insert("if-none-match", "\"v1\"".parse().unwrap());
        }),
    );
    assert_eq!(own.head.unwrap().status.as_u16(), 304);
}

#[test]
fn unsafe_methods_invalidate_the_target() {
    let s = start();
    let c = client();
    get(&s, &c, "/fresh");
    assert!(get(&s, &c, "/fresh").head.unwrap().from_cache);
    let mut post = Request::get(s.url("/fresh"), Destination::Fetch);
    post.method = Method::POST;
    post.body = Some(Bytes::from_static(b"x"));
    fetch(&c, post);
    assert!(!get(&s, &c, "/fresh").head.unwrap().from_cache);
    let methods: Vec<String> =
        s.seen.lock().unwrap().iter().filter(|(_, p, _)| p == "/fresh").map(|(m, _, _)| m.clone()).collect();
    assert_eq!(methods, ["GET", "POST", "GET"], "the GET after the POST went to the network");
}

#[test]
fn caches_are_per_context_and_clearable() {
    let s = start();
    let a = client();
    let b = client();
    let auth = || {
        with(&s, "/auth", |r| {
            r.headers.insert("authorization", "Bearer secret-a".parse().unwrap());
        })
    };
    assert_eq!(fetch(&a, auth()).body, b"Bearer secret-a");
    assert!(fetch(&a, auth()).head.unwrap().from_cache, "a private cache may store authenticated responses");
    // Another context never sees it: its request goes to the network without credentials.
    let other = get(&s, &b, "/auth");
    assert_eq!((other.body.as_slice(), other.head.unwrap().from_cache), (&b""[..], false));
    assert_eq!(s.hits("/auth"), 2);
    a.clear_http_cache();
    assert_eq!(a.http_cache_usage(), (0, 0));
    assert!(!fetch(&a, auth()).head.unwrap().from_cache);
    // Dropping the context's client frees everything; nothing is persisted.
    drop(a);
    let fresh_context = client();
    assert!(!fetch(&fresh_context, auth()).head.unwrap().from_cache);
}

#[test]
fn bounded_bytes_and_entry_size() {
    let s = start();
    let c = NetClient::new(NetConfig {
        http_cache_max_bytes: 4096,
        http_cache_max_entry_bytes: 1024,
        ..NetConfig::default()
    })
    .unwrap();
    get(&s, &c, "/big");
    get(&s, &c, "/big");
    assert_eq!(s.hits("/big"), 2, "a body over the entry cap is not stored");
    for i in 0..40 {
        get(&s, &c, &format!("/other?{i}"));
    }
    let (entries, bytes) = c.http_cache_usage();
    assert!(bytes <= 4096 && entries < 40, "({entries}, {bytes})");
    assert!(get(&s, &c, "/other?39").head.unwrap().from_cache, "most recent kept");
    assert!(!get(&s, &c, "/other?0").head.unwrap().from_cache, "least recent evicted");

    let off = NetClient::new(NetConfig { http_cache_max_bytes: 0, ..NetConfig::default() }).unwrap();
    get(&s, &off, "/fresh");
    assert!(!get(&s, &off, "/fresh").head.unwrap().from_cache, "0 bytes disables the cache");
}

#[test]
fn cache_disabled_setting_bypasses_lookup_and_store() {
    let s = start();
    let c = client();
    get(&s, &c, "/fresh");
    c.update_settings(|st| st.cache_disabled = true);
    let r = get(&s, &c, "/fresh");
    assert!(!r.head.unwrap().from_cache);
    assert_eq!(s.header("/fresh", 1, "pragma").as_deref(), Some("no-cache"));
    assert_eq!(s.header("/fresh", 1, "cache-control").as_deref(), Some("no-cache"));
    c.update_settings(|st| st.cache_disabled = false);
    assert!(get(&s, &c, "/fresh").head.unwrap().from_cache, "the earlier entry is still valid");
    assert_eq!(s.hits("/fresh"), 2);
}

#[test]
fn hits_are_reported_to_network_observers() {
    let s = start();
    let c = client();
    let sub = c.subscribe();
    get(&s, &c, "/fresh");
    get(&s, &c, "/fresh");
    let mut responses = Vec::new();
    let mut finished = Vec::new();
    while let Ok(ev) = sub.try_recv() {
        match ev {
            NetEvent::ResponseReceived { head, .. } => responses.push(head.from_cache),
            NetEvent::LoadingFinished { encoded_len, .. } => finished.push(encoded_len),
            _ => {}
        }
    }
    assert_eq!(responses, vec![false, true]);
    assert!(finished[0] > 0 && finished[1] == 0, "{finished:?}");
}

#[test]
fn page_scoped_cache_disabled_bypasses_only_that_request() {
    let s = start();
    let c = client();
    let disabled = || with(&s, "/fresh", |r| r.defaults.cache_disabled = true);
    get(&s, &c, "/fresh");
    // The disabled page's request goes to the network with the DevTools headers...
    let r = fetch(&c, disabled());
    assert!(!r.head.unwrap().from_cache);
    assert_eq!(s.header("/fresh", 1, "pragma").as_deref(), Some("no-cache"));
    assert_eq!(s.header("/fresh", 1, "cache-control").as_deref(), Some("no-cache"));
    // ...while a sibling page on the same context still hits the entry, which the bypass neither replaced nor dropped.
    assert!(get(&s, &c, "/fresh").head.unwrap().from_cache);
    assert_eq!(s.hits("/fresh"), 2);
    // A disabled page does not store.
    c.clear_http_cache();
    fetch(&c, disabled());
    assert_eq!(c.http_cache_usage().0, 0);
    assert_eq!(s.header("/fresh", 2, "cache-control").as_deref(), Some("no-cache"));
}
