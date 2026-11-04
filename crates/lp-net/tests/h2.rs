//! A local HTTP/2 server over TLS, negotiated by ALPN; no external services.
use std::net::TcpListener as StdListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use bytes::Bytes;
use http::{Method, Response, Version};
use lp_net::*;
use tokio::net::TcpListener;
use tokio::sync::Notify;

use crate::client::{collect, start_tls_server};

/// Requests to `/held` wait until this many are in flight, more than the
/// six-connection limit of HTTP/1.1 allows.
const HELD: usize = 12;

struct H2Server {
    port: u16,
    connections: Arc<AtomicUsize>,
}

impl H2Server {
    fn start() -> H2Server {
        use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
        let cert = CertificateDer::from(include_bytes!("data/self-signed.der").to_vec());
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(include_bytes!("data/self-signed.key.der").to_vec()));
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut config = rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![cert], key)
            .unwrap();
        config.alpn_protocols = vec![b"h2".to_vec()];
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
        let listener = StdListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let connections = Arc::new(AtomicUsize::new(0));
        let count = connections.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            let local = tokio::task::LocalSet::new();
            local.block_on(&rt, async move {
                let listener = TcpListener::from_std(listener).unwrap();
                let held = Arc::new((AtomicUsize::new(0), Notify::new()));
                loop {
                    let Ok((tcp, _)) = listener.accept().await else { continue };
                    count.fetch_add(1, Ordering::SeqCst);
                    let (acceptor, held) = (acceptor.clone(), held.clone());
                    tokio::task::spawn_local(async move {
                        let Ok(tls) = acceptor.accept(tcp).await else { return };
                        let Ok(mut connection) = h2::server::handshake(tls).await else { return };
                        while let Some(Ok((request, respond))) = connection.accept().await {
                            tokio::task::spawn_local(answer(request, respond, held.clone()));
                        }
                    });
                }
            });
        });
        H2Server { port, connections }
    }

    fn url(&self, path: &str) -> String {
        format!("https://127.0.0.1:{}{path}", self.port)
    }
}

async fn answer(
    request: http::Request<h2::RecvStream>,
    mut respond: h2::server::SendResponse<Bytes>,
    held: Arc<(AtomicUsize, Notify)>,
) {
    let path = request.uri().path().to_owned();
    let mut received = Vec::new();
    let mut body = request.into_body();
    while let Some(Ok(chunk)) = body.data().await {
        let _ = body.flow_control().release_capacity(chunk.len());
        received.extend_from_slice(&chunk);
    }
    if path == "/held" {
        let (count, all_here) = &*held;
        if count.fetch_add(1, Ordering::SeqCst) + 1 >= HELD {
            all_here.notify_waiters();
        }
        while count.load(Ordering::SeqCst) < HELD {
            let waiting = all_here.notified();
            if count.load(Ordering::SeqCst) >= HELD {
                break;
            }
            waiting.await;
        }
    }
    let mut response = Response::builder().status(200).header("content-type", "text/plain");
    if path == "/cached" {
        response = response.header("cache-control", "max-age=600");
    }
    let Ok(mut stream) = respond.send_response(response.body(()).unwrap(), false) else { return };
    let out = if received.is_empty() { format!("h2 {path}") } else { String::from_utf8_lossy(&received).into_owned() };
    let _ = stream.send_data(Bytes::from(out), true);
}

fn client() -> NetClient {
    let c = NetClient::new(NetConfig { shared_asset_cache: None, ..NetConfig::default() }).unwrap();
    c.set_ignore_certificate_errors(true);
    c
}

fn get(c: &NetClient, url: &str) -> (Option<ResponseHead>, Vec<u8>, Option<NetError>) {
    let (_, head, body, error) = collect(c, Request::get(Url::parse(url).unwrap(), Destination::Fetch));
    (head, body, error)
}

#[test]
fn alpn_selects_h2_and_requests_share_one_connection() {
    let server = H2Server::start();
    let c = client();
    let (head, body, error) = get(&c, &server.url("/first"));
    assert_eq!(error, None);
    let head = head.unwrap();
    assert_eq!((head.protocol, head.status.as_u16(), head.status_text.as_str()), (Version::HTTP_2, 200, ""));
    assert_eq!(head.mime, "text/plain");
    assert_eq!(body, b"h2 /first");

    let mut post = Request::get(Url::parse(&server.url("/post")).unwrap(), Destination::Fetch);
    post.method = Method::POST;
    post.body = Some(Bytes::from_static(b"payload"));
    let (_, _, body, error) = collect(&c, post);
    assert_eq!((body.as_slice(), error), (&b"payload"[..], None));

    let (_, body, _) = get(&c, &server.url("/third"));
    assert_eq!(body, b"h2 /third");
    assert_eq!(server.connections.load(Ordering::SeqCst), 1, "kept and reused");

    // Another context has its own connection.
    let other = client();
    assert_eq!(get(&other, &server.url("/other")).1, b"h2 /other");
    assert_eq!(server.connections.load(Ordering::SeqCst), 2);
}

#[test]
fn an_h2_origin_multiplexes_past_the_connection_limit() {
    let server = H2Server::start();
    let c = client();
    assert_eq!(get(&c, &server.url("/warm")).1, b"h2 /warm");
    let (tx, rx) = mpsc::channel();
    for _ in 0..HELD {
        let tx = tx.clone();
        let request = Request::get(Url::parse(&server.url("/held")).unwrap(), Destination::Fetch);
        c.fetch(
            request,
            Box::new(move |event| {
                if matches!(event, ResponseEvent::Finished { .. } | ResponseEvent::Failed(_)) {
                    let _ = tx.send(matches!(event, ResponseEvent::Finished { .. }));
                }
            }),
        );
    }
    // The server answers only when all of them are in flight together.
    for _ in 0..HELD {
        assert!(rx.recv_timeout(Duration::from_secs(10)).expect("all streams in flight"));
    }
    assert_eq!(server.connections.load(Ordering::SeqCst), 1);
}

#[test]
fn http1_servers_still_work_and_cached_responses_keep_their_protocol() {
    let tls = start_tls_server();
    let c = client();
    let (head, body, error) = get(&c, &format!("https://127.0.0.1:{tls}/"));
    let head = head.unwrap();
    assert_eq!((head.protocol, head.status_text.as_str(), error), (Version::HTTP_11, "OK", None));
    assert_eq!(body, b"secure");

    let server = H2Server::start();
    assert_eq!(get(&c, &server.url("/cached")).0.unwrap().protocol, Version::HTTP_2);
    let (head, body, _) = get(&c, &server.url("/cached"));
    let head = head.unwrap();
    assert!(head.from_cache);
    assert_eq!((head.protocol, head.status_text.as_str(), body.as_slice()), (Version::HTTP_2, "", &b"h2 /cached"[..]));
}

#[test]
fn h2_runs_inside_a_connect_tunnel() {
    let server = H2Server::start();
    let proxy = crate::proxy::Proxy::start(crate::proxy::Mode::Open);
    let c = proxy.client("<-loopback>");
    c.set_ignore_certificate_errors(true);
    for path in ["/one", "/two"] {
        let (head, body, error) = get(&c, &server.url(path));
        assert_eq!((head.unwrap().protocol, error), (Version::HTTP_2, None));
        assert_eq!(body, format!("h2 {path}").as_bytes());
    }
    assert_eq!(server.connections.load(Ordering::SeqCst), 1);
    let heads = proxy.heads();
    assert_eq!(heads.len(), 1, "{heads:?}");
    assert!(heads[0].starts_with(&format!("CONNECT 127.0.0.1:{} ", server.port)), "{heads:?}");
}
