//! A tiny local forward proxy (CONNECT and absolute-form); no external services.
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use lp_net::*;
use tokio::sync::oneshot;

use crate::client::{collect, start_tls_server};

const USER_PASS: &str = "Proxy-Authorization: Basic dXNlcjpwYXNz";

/// hyper writes header names in lowercase, the CONNECT path as Chrome does.
fn authorized(head: &str) -> bool {
    head.lines().any(|l| l.eq_ignore_ascii_case(USER_PASS))
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Mode {
    Open,
    /// Answers 407 without `USER_PASS`.
    Auth,
    /// Refuses every CONNECT with 502.
    Refuse,
}

pub(crate) struct Proxy {
    port: u16,
    /// Request heads, as received.
    log: Arc<Mutex<Vec<String>>>,
}

impl Proxy {
    pub(crate) fn start(mode: Mode) -> Proxy {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let log = Arc::new(Mutex::new(Vec::new()));
        let record = log.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let (Ok(stream), record) = (stream, record.clone()) else { continue };
                std::thread::spawn(move || serve_proxied(stream, mode, record));
            }
        });
        Proxy { port, log }
    }
    fn server(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
    pub(crate) fn heads(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
    pub(crate) fn client(&self, bypass: &str) -> NetClient {
        client_for(&self.server(), bypass)
    }
}

fn client_for(server: &str, bypass: &str) -> NetClient {
    let proxy = ProxyConfig::parse(server, Some(bypass)).unwrap();
    NetClient::new(NetConfig { proxy, shared_asset_cache: None, ..NetConfig::default() }).unwrap()
}

fn serve_proxied(mut stream: TcpStream, mode: Mode, log: Arc<Mutex<Vec<String>>>) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    loop {
        let mut head = String::new();
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                return;
            }
            head.push_str(&line);
            if line == "\r\n" {
                break;
            }
        }
        log.lock().unwrap().push(head.clone());
        if mode == Mode::Auth && !authorized(&head) {
            let _ = stream.write_all(
                b"HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: Basic realm=\"local proxy\"\r\n\
                  Connection: close\r\nContent-Length: 6\r\n\r\ndenied",
            );
            return;
        }
        let Some(target) = head.strip_prefix("CONNECT ").and_then(|l| l.split_whitespace().next()) else {
            let target = head.split_whitespace().nth(1).unwrap_or("");
            let body = format!("proxied {target}");
            let reply = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}", body.len());
            if stream.write_all(reply.as_bytes()).is_err() {
                return;
            }
            continue;
        };
        if mode == Mode::Refuse {
            let _ = stream.write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n");
            return;
        }
        let Ok(upstream) = TcpStream::connect(target) else {
            let _ = stream.write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n");
            return;
        };
        stream.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n").unwrap();
        pipe(reader, upstream.try_clone().unwrap());
        let mut down = upstream;
        let _ = std::io::copy(&mut down, &mut stream);
        let _ = stream.shutdown(Shutdown::Both);
        return;
    }
}

/// Copies client bytes to `upstream` until the client closes.
fn pipe(mut from: impl Read + Send + 'static, mut upstream: TcpStream) {
    std::thread::spawn(move || {
        let _ = std::io::copy(&mut from, &mut upstream);
        let _ = upstream.shutdown(Shutdown::Write);
    });
}

fn fetch(client: &NetClient, url: &str) -> (Vec<String>, Option<ResponseHead>, Vec<u8>, Option<NetError>) {
    collect(client, Request::get(Url::parse(url).unwrap(), Destination::Fetch))
}

type PendingAuth = (AuthChallenge, oneshot::Sender<AuthDecision>);
struct Answers {
    tx: mpsc::Sender<PendingAuth>,
}
impl Interceptor for Answers {
    fn paused(&self, _req: PausedRequest, reply: oneshot::Sender<InterceptDecision>) {
        reply.send(InterceptDecision::Continue { url: None, method: None, headers: None, body: None }).unwrap();
    }
    fn handles_auth(&self) -> bool {
        true
    }
    fn auth_required(&self, challenge: AuthChallenge, reply: oneshot::Sender<AuthDecision>) {
        self.tx.send((challenge, reply)).unwrap();
    }
}
fn answering(client: &NetClient) -> mpsc::Receiver<PendingAuth> {
    let (tx, rx) = mpsc::channel();
    client.set_interceptor(Some(Arc::new(Answers { tx })));
    rx
}
fn credentials() -> AuthDecision {
    AuthDecision::ProvideCredentials { username: "user".into(), password: "pass".into() }
}
/// Starts a fetch in the background, answers `decisions` in order and returns
/// the result and the challenges seen.
fn fetch_answering(
    client: &NetClient,
    rx: &mpsc::Receiver<PendingAuth>,
    url: &str,
    decisions: Vec<AuthDecision>,
) -> ((Vec<String>, Option<ResponseHead>, Vec<u8>, Option<NetError>), Vec<AuthChallenge>) {
    let (c, u) = (client.clone(), url.to_owned());
    let running = std::thread::spawn(move || fetch(&c, &u));
    let mut seen = Vec::new();
    for decision in decisions {
        let (challenge, reply) = rx.recv_timeout(Duration::from_secs(5)).expect("challenge");
        seen.push(challenge);
        assert!(reply.send(decision).is_ok());
    }
    (running.join().unwrap(), seen)
}

#[test]
fn http_goes_absolute_form_and_https_through_connect() {
    let proxy = Proxy::start(Mode::Open);
    // Neither name resolves here: only the proxy can answer the first.
    let c = proxy.client("");
    let (_, head, body, e) = fetch(&c, "http://example.invalid/a?b=1");
    assert_eq!(e, None);
    assert_eq!(head.unwrap().status.as_u16(), 200);
    assert_eq!(body, b"proxied http://example.invalid/a?b=1");
    let heads = proxy.heads();
    assert!(heads[0].starts_with("GET http://example.invalid/a?b=1 HTTP/1.1\r\n"), "{heads:?}");
    assert!(heads[0].to_ascii_lowercase().contains("\r\nhost: example.invalid\r\n"), "{heads:?}");

    // The kept connection carries the next request, also absolute-form.
    let (_, _, body, _) = fetch(&c, "http://example.invalid/next");
    assert_eq!(body, b"proxied http://example.invalid/next");
    assert!(proxy.heads()[1].starts_with("GET http://example.invalid/next "));

    let tls = start_tls_server();
    let c = proxy.client("<-loopback>");
    c.set_ignore_certificate_errors(true);
    let (_, _, body, e) = fetch(&c, &format!("https://127.0.0.1:{tls}/"));
    assert_eq!((body.as_slice(), e), (&b"secure"[..], None));
    let last = proxy.heads().pop().unwrap();
    assert!(last.starts_with(&format!("CONNECT 127.0.0.1:{tls} HTTP/1.1\r\nHost: 127.0.0.1:{tls}\r\n")), "{last}");
    assert!(!last.contains("Proxy-Authorization"), "{last}");

    // A certificate is still checked end to end, through the tunnel.
    let c = proxy.client("<-loopback>");
    let (_, _, _, e) = fetch(&c, &format!("https://127.0.0.1:{tls}/"));
    assert_eq!(e.unwrap().error_text(), "net::ERR_CERT_AUTHORITY_INVALID");
}

#[test]
fn bypass_list_and_loopback_go_direct() {
    let proxy = Proxy::start(Mode::Open);
    let tls = start_tls_server();
    let url = format!("https://127.0.0.1:{tls}/");

    let implicit = proxy.client("");
    implicit.set_ignore_certificate_errors(true);
    assert_eq!(fetch(&implicit, &url).2, b"secure");
    assert_eq!(proxy.heads(), Vec::<String>::new(), "loopback skips the proxy by default");

    let listed = proxy.client("<-loopback>,127.0.0.1");
    listed.set_ignore_certificate_errors(true);
    assert_eq!(fetch(&listed, &url).2, b"secure");
    assert_eq!(proxy.heads(), Vec::<String>::new(), "listed host skips the proxy");

    let (_, _, _, e) = fetch(&proxy.client("*.invalid"), "http://example.invalid/");
    assert_eq!(e.unwrap().error_text(), "net::ERR_NAME_NOT_RESOLVED", "bypassed hosts resolve locally");
    assert_eq!(proxy.heads(), Vec::<String>::new());
}

#[test]
fn proxy_failures_have_their_own_errors() {
    let dead = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let c = client_for(&format!("http://127.0.0.1:{dead}"), "<-loopback>");
    for url in ["http://example.invalid/", "https://example.invalid/"] {
        let (_, _, _, e) = fetch(&c, url);
        assert_eq!(e.unwrap().error_text(), "net::ERR_PROXY_CONNECTION_FAILED", "{url}");
    }

    let refuse = Proxy::start(Mode::Refuse);
    let (_, _, _, e) = fetch(&refuse.client(""), "https://example.invalid/");
    assert_eq!(e.unwrap().error_text(), "net::ERR_TUNNEL_CONNECTION_FAILED");

    // Without an interceptor nothing can answer a CONNECT 407.
    let auth = Proxy::start(Mode::Auth);
    let (_, _, _, e) = fetch(&auth.client(""), "https://example.invalid/");
    assert_eq!(e.unwrap().error_text(), "net::ERR_TUNNEL_CONNECTION_FAILED");
    // A plain request has a real 407 response to deliver.
    let (_, head, body, e) = fetch(&auth.client(""), "http://example.invalid/");
    assert_eq!((head.unwrap().status.as_u16(), body.as_slice(), e), (407, &b"denied"[..], None));
}

#[test]
fn connect_challenge_goes_through_the_auth_path_and_is_kept_for_the_context() {
    let proxy = Proxy::start(Mode::Auth);
    let tls = start_tls_server();
    let url = format!("https://127.0.0.1:{tls}/");
    let c = proxy.client("<-loopback>");
    c.set_ignore_certificate_errors(true);
    let auth = answering(&c);

    let ((_, _, body, e), seen) = fetch_answering(&c, &auth, &url, vec![credentials()]);
    assert_eq!((body.as_slice(), e), (&b"secure"[..], None));
    let challenge = &seen[0];
    assert!(challenge.proxy);
    assert_eq!((challenge.scheme.as_str(), challenge.realm.as_str()), ("basic", "local proxy"));
    assert_eq!(challenge.origin, proxy.server(), "the origin of a proxy challenge is the proxy");
    let heads = proxy.heads();
    assert_eq!(heads.len(), 2, "{heads:?}");
    assert!(!heads[0].contains("Proxy-Authorization"), "{heads:?}");
    assert!(authorized(&heads[1]), "{heads:?}");

    // Later requests of the context send it unasked.
    let ((_, _, body, e), seen) = fetch_answering(&c, &auth, &format!("{url}again"), vec![]);
    assert_eq!((body.as_slice(), e, seen.len()), (&b"secure"[..], None, 0));

    // Another context on the same proxy starts without them.
    let other = proxy.client("<-loopback>");
    other.set_ignore_certificate_errors(true);
    let other_auth = answering(&other);
    let ((_, _, _, e), _) = fetch_answering(&other, &other_auth, &url, vec![AuthDecision::CancelAuth]);
    assert_eq!(e.unwrap().error_text(), "net::ERR_TUNNEL_CONNECTION_FAILED");

    let ((_, _, _, e), _) = fetch_answering(&other, &other_auth, &url, vec![AuthDecision::Default]);
    assert_eq!(e.unwrap().error_text(), "net::ERR_TUNNEL_CONNECTION_FAILED");

    // Wrong credentials stop at the retry limit.
    let wrong = proxy.client("<-loopback>");
    let wrong_auth = answering(&wrong);
    let bad = || AuthDecision::ProvideCredentials { username: "user".into(), password: "nope".into() };
    let ((_, _, _, e), seen) = fetch_answering(&wrong, &wrong_auth, &url, vec![bad(), bad(), bad()]);
    assert_eq!((e.unwrap().error_text(), seen.len()), ("net::ERR_INVALID_AUTH_CREDENTIALS", 3));
}

#[test]
fn plain_request_challenge_retries_with_the_proxy_header() {
    let proxy = Proxy::start(Mode::Auth);
    let c = proxy.client("");
    let auth = answering(&c);
    let events = c.subscribe();
    let url = "http://example.invalid/page";
    let ((_, head, body, e), seen) = fetch_answering(&c, &auth, url, vec![credentials()]);
    assert_eq!((head.unwrap().status.as_u16(), e), (200, None));
    assert_eq!(body, b"proxied http://example.invalid/page");
    assert!(seen[0].proxy);
    assert_eq!(seen[0].origin, proxy.server());
    assert!(authorized(proxy.heads().last().unwrap()));
    // The header is the connection's: it does not show as a page request header.
    let mut observed = false;
    while let Ok(event) = events.try_recv() {
        if let NetEvent::RequestWillBeSent { request, .. } = event {
            observed = true;
            assert!(!request.headers.contains_key("proxy-authorization"));
        }
    }
    assert!(observed);

    // Declined: the 407 itself is the response.
    let c = proxy.client("");
    let auth = answering(&c);
    let ((_, head, body, e), _) = fetch_answering(&c, &auth, url, vec![AuthDecision::CancelAuth]);
    assert_eq!((head.unwrap().status.as_u16(), body.as_slice(), e), (407, &b"denied"[..], None));
}

#[test]
fn credentials_in_the_proxy_url_are_sent_preemptively() {
    let proxy = Proxy::start(Mode::Auth);
    let c = client_for(&format!("http://user:pass@127.0.0.1:{}", proxy.port), "<-loopback>");
    let (_, _, body, e) = fetch(&c, "http://example.invalid/");
    assert_eq!((body.as_slice(), e), (&b"proxied http://example.invalid/"[..], None));
    let tls = start_tls_server();
    c.set_ignore_certificate_errors(true);
    let (_, _, body, e) = fetch(&c, &format!("https://127.0.0.1:{tls}/"));
    assert_eq!((body.as_slice(), e), (&b"secure"[..], None));
    let heads = proxy.heads();
    assert_eq!(heads.len(), 2, "{heads:?}");
    assert!(heads.iter().all(|h| authorized(h)), "{heads:?}");
}
