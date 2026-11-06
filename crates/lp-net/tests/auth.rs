//! Synthetic credentials and isolated loopback servers; no external services.
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc, Mutex,
};
use std::time::Duration;

use bytes::Bytes;
use lp_net::*;
use tokio::sync::oneshot;

type RequestLog = Arc<Mutex<Vec<(String, Vec<u8>)>>>;
type PendingAuth = (AuthChallenge, oneshot::Sender<AuthDecision>);

struct Server {
    url: Url,
    requests: RequestLog,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Server {
    fn new(challenge: &'static str, success: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (record, stopped) = (requests.clone(), stop.clone());
        let thread = std::thread::spawn(move || {
            for stream in listener.incoming() {
                if stopped.load(Ordering::Acquire) {
                    break;
                }
                let mut stream = stream.unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut headers = String::new();
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some((key, value)) = line.split_once(':') {
                        if key.eq_ignore_ascii_case("content-length") {
                            length = value.trim().parse().unwrap();
                        }
                    }
                    headers.push_str(&line);
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                let authorized =
                    headers.lines().any(|line| line.eq_ignore_ascii_case("authorization: Basic dXNlcjpwYXNz"));
                record.lock().unwrap().push((headers, body));
                let (status, extra, body) = if success && authorized {
                    ("200 OK", String::new(), "accepted")
                } else {
                    ("401 Unauthorized", format!("WWW-Authenticate: {challenge}\r\n"), "declined")
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\n{extra}Connection: close\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        Self { url: Url::parse(&format!("http://{address}/auth")).unwrap(), requests, stop, thread: Some(thread) }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = TcpStream::connect(("127.0.0.1", self.url.port().unwrap()));
        self.thread.take().unwrap().join().unwrap();
    }
}

struct Intercept {
    paused: Mutex<Vec<RequestId>>,
    auth: mpsc::Sender<PendingAuth>,
}
impl Interceptor for Intercept {
    fn paused(&self, req: PausedRequest, reply: oneshot::Sender<InterceptDecision>) {
        self.paused.lock().unwrap().push(req.id);
        reply.send(InterceptDecision::Continue { url: None, method: None, headers: None, body: None }).unwrap();
    }
    fn handles_auth(&self) -> bool {
        true
    }
    fn auth_required(&self, challenge: AuthChallenge, reply: oneshot::Sender<AuthDecision>) {
        self.auth.send((challenge, reply)).unwrap();
    }
}
fn setup(config: NetConfig) -> (NetClient, Arc<Intercept>, mpsc::Receiver<PendingAuth>) {
    let (tx, rx) = mpsc::channel();
    let intercept = Arc::new(Intercept { paused: Mutex::new(Vec::new()), auth: tx });
    let client = NetClient::new(config).unwrap();
    client.set_interceptor(Some(intercept.clone()));
    (client, intercept, rx)
}
fn start(client: &NetClient, server: &Server) -> (RequestId, mpsc::Receiver<ResponseEvent>) {
    let (tx, rx) = mpsc::channel();
    let mut req = Request::get(server.url.clone(), Destination::Fetch);
    req.method = http::Method::POST;
    req.body = Some(Bytes::from_static(b"payload"));
    (client.fetch(req, Box::new(move |event| tx.send(event).unwrap())), rx)
}
fn finish(rx: &mpsc::Receiver<ResponseEvent>) -> (Option<u16>, Vec<u8>, Option<NetError>) {
    let mut status = None;
    let mut body = Vec::new();
    let error = loop {
        match rx.recv_timeout(Duration::from_secs(5)).unwrap() {
            ResponseEvent::Response(head) => status = Some(head.status.as_u16()),
            ResponseEvent::Data(bytes) => body.extend(bytes),
            ResponseEvent::Finished { .. } => break None,
            ResponseEvent::Failed(error) => break Some(error),
            ResponseEvent::Redirect { .. } => panic!("unexpected redirect"),
        }
    };
    assert!(matches!(rx.recv_timeout(Duration::from_secs(5)), Err(mpsc::RecvTimeoutError::Disconnected)));
    (status, body, error)
}
fn credentials() -> AuthDecision {
    AuthDecision::ProvideCredentials { username: "user".into(), password: "pass".into() }
}

#[test]
fn basic_retries_preserve_body_id_and_single_lifecycle() {
    let server = Server::new(r#"Digest realm="other", Basic realm="test,realm""#, true);
    let (client, intercept, auth) = setup(NetConfig::default());
    let events = client.subscribe();
    let (id, rx) = start(&client, &server);
    let (challenge, reply) = auth.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(challenge.id, id);
    assert_eq!(challenge.origin, server.url.origin().ascii_serialization());
    assert_eq!(challenge.scheme, "basic");
    assert_eq!(challenge.realm, "test,realm");
    assert!(!challenge.proxy);
    assert_eq!(challenge.request.post_data, Some(Bytes::from_static(b"payload")));
    assert!(reply.send(credentials()).is_ok());
    assert_eq!(finish(&rx), (Some(200), b"accepted".to_vec(), None));
    assert_eq!(*intercept.paused.lock().unwrap(), [id]);
    let requests = server.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests.iter().all(|(headers, body)| headers.starts_with("POST /auth ") && body == b"payload"));
    let mut kinds = Vec::new();
    while let Ok(event) = events.try_recv() {
        let (event_id, kind) = match event {
            NetEvent::RequestWillBeSent { id, .. } => (id, "start"),
            NetEvent::ResponseReceived { id, .. } => (id, "response"),
            NetEvent::DataReceived { id, .. } => (id, "data"),
            NetEvent::LoadingFinished { id, .. } => (id, "finished"),
            NetEvent::LoadingFailed { .. } | NetEvent::WebSocket { .. } => panic!("unexpected event"),
        };
        assert_eq!(event_id, id);
        kinds.push(kind);
    }
    assert_eq!(kinds, ["start", "response", "data", "finished"]);
}

#[test]
fn declined_auth_returns_challenge_response() {
    for decision in [AuthDecision::Default, AuthDecision::CancelAuth] {
        let server = Server::new("Basic realm=test", false);
        let (client, _, auth) = setup(NetConfig::default());
        let (_, rx) = start(&client, &server);
        let (_, reply) = auth.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(reply.send(decision).is_ok());
        assert_eq!(finish(&rx), (Some(401), b"declined".to_vec(), None));
        assert_eq!(server.requests.lock().unwrap().len(), 1);
    }
}

#[test]
fn credential_retry_limit_and_unsupported_scheme_are_explicit() {
    let server = Server::new("Basic realm=test", false);
    let (client, intercept, auth) = setup(NetConfig { max_auth_retries: 2, ..NetConfig::default() });
    let (_, rx) = start(&client, &server);
    for _ in 0..2 {
        let (_, reply) = auth.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(reply.send(credentials()).is_ok());
    }
    assert_eq!(finish(&rx).2, Some(NetError::Blocked("net::ERR_INVALID_AUTH_CREDENTIALS".into())));
    assert_eq!(server.requests.lock().unwrap().len(), 3);
    assert_eq!(intercept.paused.lock().unwrap().len(), 1);
    let server = Server::new("Digest realm=test, nonce=x", false);
    let (client, _, auth) = setup(NetConfig::default());
    let (_, rx) = start(&client, &server);
    let (challenge, reply) = auth.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(challenge.scheme, "digest");
    assert!(reply.send(credentials()).is_ok());
    assert_eq!(finish(&rx).2, Some(NetError::Other("unsupported authentication scheme: digest".into())));
    assert_eq!(server.requests.lock().unwrap().len(), 1);
}

#[test]
fn pending_auth_cancellation_shutdown_and_dropped_reply_terminate_once() {
    for action in ["cancel", "shutdown", "drop-reply"] {
        let server = Server::new("Basic realm=test", false);
        let (client, _, auth) = setup(NetConfig::default());
        let events = client.subscribe();
        let (id, rx) = start(&client, &server);
        let (_, reply) = auth.recv_timeout(Duration::from_secs(5)).unwrap();
        let mut reply = Some(reply);
        match action {
            "cancel" => client.cancel(id),
            "shutdown" => drop(client),
            _ => drop(reply.take()),
        }
        let expected = if action == "drop-reply" {
            NetError::Other("authentication interception dropped".into())
        } else {
            NetError::Aborted
        };
        assert_eq!(finish(&rx), (None, vec![], Some(expected)));
        if let Some(reply) = reply {
            assert!(reply.is_closed());
        }
        assert!(matches!(events.try_recv(), Ok(NetEvent::RequestWillBeSent { .. })));
        assert!(matches!(events.try_recv(), Ok(NetEvent::LoadingFailed { .. })));
        assert!(events.try_recv().is_err());
    }
}

#[test]
fn authentication_without_opt_in_delivers_401_once() {
    let server = Server::new("Basic realm=test", false);
    let client = NetClient::new(NetConfig::default()).unwrap();
    let (_, rx) = start(&client, &server);
    assert_eq!(finish(&rx), (Some(401), b"declined".to_vec(), None));
    assert_eq!(server.requests.lock().unwrap().len(), 1);
}

#[test]
fn cancellation_releases_auth_reply_before_terminal_callback() {
    for shutdown in [false, true] {
        let server = Server::new("Basic realm=test", false);
        let (client, _, auth) = setup(NetConfig::default());
        let pending: Arc<Mutex<Option<oneshot::Sender<AuthDecision>>>> = Arc::new(Mutex::new(None));
        let in_callback = pending.clone();
        let (tx, rx) = mpsc::channel();
        let id = client.fetch(
            Request::get(server.url.clone(), Destination::Fetch),
            Box::new(move |event| {
                if let ResponseEvent::Failed(error) = event {
                    let closed = in_callback.lock().unwrap().as_ref().unwrap().is_closed();
                    tx.send((error, closed)).unwrap();
                }
            }),
        );
        let (_, reply) = auth.recv_timeout(Duration::from_secs(5)).unwrap();
        *pending.lock().unwrap() = Some(reply);
        if shutdown {
            drop(client);
        } else {
            client.cancel(id);
        }
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), (NetError::Aborted, true));
        assert!(matches!(rx.recv_timeout(Duration::from_secs(5)), Err(mpsc::RecvTimeoutError::Disconnected)));
    }
}
