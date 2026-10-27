//! `lightningpanda serve`: the CDP server process.
//!
//! The main thread runs a single-threaded tokio runtime for sockets. Each
//! upgraded connection gets its own browser thread owning a JavaScriptCore
//! engine, an [`EngineHost`] and the protocol state (docs/ARCHITECTURE.md).

use std::cell::RefCell;
use std::collections::VecDeque;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use lp_runtime::RunOutcome;
use tokio::sync::{mpsc, oneshot, Semaphore};

use crate::engine::{BrowserProfile, EngineHost};
use crate::outbox::{Outbox, DEFAULT_OUTBOUND_BUDGET};
use crate::server::{Inbound, Server, ServerConfig, SessionChannels, SessionFactory};
use crate::target::Browser;
use crate::ws::WsConfig;

const USAGE: &str = "\
usage: lightningpanda serve [OPTIONS]

Starts a WebSocket CDP server (Chrome DevTools Protocol on ws://HOST:PORT/).

options:
  --host <HOST>                         default 127.0.0.1
  --port <INT>                          default 9222
  --advertise-host <HOST>               host advertised in /json/version (default: --host)
  --cdp-max-connections <INT>           default 16
  --cdp-max-message-size <INT>          default 1048576
  --cdp-max-http-message-size <INT>     default 1048576
  --log-level <LEVEL>                   debug|info|warn|error|fatal (default info)
  --experimental-features <NAME>        accepted: cors (repeatable)
  --load-resources <KIND>               accepted: stylesheet, iframe (repeatable)
  --local-storage <FILE>                JSON preloaded into every browser context's
                                        localStorage: {\"http://host:port\": {\"key\": \"value\"}}
                                        (env LIGHTNINGPANDA_LOCAL_STORAGE)
  --static <ORIGIN>=<DIR>               serve GET requests for ORIGIN from DIR, no server
                                        needed there (repeatable; env LIGHTNINGPANDA_STATIC,
                                        entries separated by ';')

HTTP and data: document loading are supported; browser API coverage is partial.
--experimental-features and --load-resources are accepted for CLI compatibility;
the current loader configuration is fixed.

--local-storage: top-level keys are origins (or URLs; only their origin is used),
values are objects of string entries. A non-string entry value is stored as its
JSON text, so {\"settings\": {\"theme\": \"dark\"}} reads back as the string
'{\"theme\":\"dark\"}'. Every connection and every Target.createBrowserContext
starts with a fresh copy; pages still write to their own context only.

--static: like a static file server for a front-end build: DIR/<path>, a
directory's index.html, and for navigations any path without a file falls
back to DIR/index.html (single-page-app routes). Requests the directory cannot
answer (no such file, POST, other origins) go to the network as usual, so API
calls on the same origin still reach their server. Playwright routes still
see these requests.
";

/// Inbound messages waiting on a browser thread.
const INFLIGHT: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServeOptions {
    pub host: IpAddr,
    pub port: u16,
    pub server: ServerConfig,
    pub log_level: LogLevel,
    pub experimental_features: Vec<String>,
    pub load_resources: Vec<String>,
    /// `--local-storage` (or `LIGHTNINGPANDA_LOCAL_STORAGE`): JSON file of
    /// per-origin localStorage entries preloaded into every context.
    pub local_storage: Option<PathBuf>,
    /// `--static ORIGIN=DIR` (or `LIGHTNINGPANDA_STATIC`): directories served
    /// in place of origins.
    pub static_roots: Vec<(String, PathBuf)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
    Fatal,
}

impl Default for ServeOptions {
    fn default() -> Self {
        Self {
            host: IpAddr::from([127, 0, 0, 1]),
            port: 9222,
            server: ServerConfig::default(),
            log_level: LogLevel::Info,
            experimental_features: Vec::new(),
            load_resources: Vec::new(),
            local_storage: std::env::var_os(LOCAL_STORAGE_ENV).filter(|v| !v.is_empty()).map(PathBuf::from),
            static_roots: std::env::var(STATIC_ENV)
                .ok()
                .map(|v| {
                    v.split(';').filter(|e| !e.trim().is_empty()).filter_map(|e| parse_static_root(e).ok()).collect()
                })
                .unwrap_or_default(),
        }
    }
}

/// Environment equivalent of `--local-storage` (the flag wins).
pub const LOCAL_STORAGE_ENV: &str = "LIGHTNINGPANDA_LOCAL_STORAGE";
/// Environment equivalent of `--static` entries, separated by `;` (flags add to it).
pub const STATIC_ENV: &str = "LIGHTNINGPANDA_STATIC";

/// `ORIGIN=DIR` for `--static`. The first `=` after the origin's `://` splits.
fn parse_static_root(entry: &str) -> Result<(String, PathBuf), String> {
    let entry = entry.trim();
    let after_scheme = entry.find("://").map_or(0, |i| i + 3);
    let Some(eq) = entry[after_scheme..].find('=').map(|i| i + after_scheme) else {
        return Err(format!("--static: '{entry}' is not ORIGIN=DIR"));
    };
    let (origin, dir) = (entry[..eq].trim(), entry[eq + 1..].trim());
    if origin.is_empty() || dir.is_empty() {
        return Err(format!("--static: '{entry}' is not ORIGIN=DIR"));
    }
    Ok((origin.to_owned(), PathBuf::from(dir)))
}

pub fn parse_args(args: &[String]) -> Result<ServeOptions, String> {
    let mut o = ServeOptions::default();
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        let mut value = || it.next().cloned().ok_or_else(|| format!("{flag} needs a value"));
        let number = |v: String| v.parse::<usize>().map_err(|_| format!("{flag}: '{v}' is not a number"));
        match flag.as_str() {
            "--host" => {
                let v = value()?;
                o.host = if v == "localhost" {
                    IpAddr::from([127, 0, 0, 1])
                } else {
                    v.parse().map_err(|_| format!("--host: '{v}' is not an IP address"))?
                };
            }
            "--port" => {
                let v = value()?;
                o.port = v.parse().map_err(|_| format!("--port: '{v}' is not a port"))?;
            }
            "--advertise-host" => o.server.advertise_host = Some(value()?),
            "--cdp-max-connections" => o.server.max_connections = number(value()?)?.max(1),
            "--cdp-max-message-size" => o.server.ws = WsConfig { max_message_size: number(value()?)? },
            "--cdp-max-http-message-size" => o.server.max_http_message_size = number(value()?)?,
            "--log-level" => {
                o.log_level = match value()?.as_str() {
                    "debug" => LogLevel::Debug,
                    "info" => LogLevel::Info,
                    "warn" => LogLevel::Warn,
                    "error" => LogLevel::Error,
                    "fatal" => LogLevel::Fatal,
                    other => return Err(format!("--log-level: unknown level '{other}'")),
                }
            }
            "--experimental-features" => match value()?.as_str() {
                v @ "cors" => o.experimental_features.push(v.to_owned()),
                other => return Err(format!("--experimental-features: unknown feature '{other}'")),
            },
            "--load-resources" => match value()?.as_str() {
                v @ ("stylesheet" | "iframe") => o.load_resources.push(v.to_owned()),
                other => return Err(format!("--load-resources: unsupported kind '{other}'")),
            },
            "--local-storage" => o.local_storage = Some(PathBuf::from(value()?)),
            "--static" => o.static_roots.push(parse_static_root(&value()?)?),
            other => return Err(format!("unknown option '{other}'")),
        }
    }
    Ok(o)
}

/// Parses the `--local-storage` document: `{origin: {key: value}}`. String
/// values are stored as they are; any other JSON value is stored as its
/// serialized text (apps commonly keep JSON objects in localStorage).
pub fn parse_local_storage(text: &str) -> Result<lp_webapi::StorageSeed, String> {
    let doc: serde_json::Value = serde_json::from_str(text).map_err(|e| format!("not valid JSON: {e}"))?;
    let Some(origins) = doc.as_object() else {
        return Err("top level must be an object keyed by origin".into());
    };
    let mut seed = Vec::with_capacity(origins.len());
    for (origin, entries) in origins {
        let Some(entries) = entries.as_object() else {
            return Err(format!("'{origin}': value must be an object of localStorage entries"));
        };
        let entries = entries
            .iter()
            .map(|(key, value)| {
                let value = match value {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                (key.clone(), value)
            })
            .collect();
        seed.push((origin.clone(), entries));
    }
    lp_webapi::StorageSeed::new(seed)
}

/// The browser profile for `options`: reads and validates the preload files.
pub fn browser_profile(options: &ServeOptions) -> Result<BrowserProfile, String> {
    let mut profile = BrowserProfile::default();
    if let Some(path) = &options.local_storage {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("--local-storage: cannot read {}: {e}", path.display()))?;
        profile.local_storage =
            parse_local_storage(&text).map_err(|e| format!("--local-storage: {}: {e}", path.display()))?;
    }
    if !options.static_roots.is_empty() {
        profile.net.static_files =
            lp_net::StaticFiles::new(options.static_roots.clone()).map_err(|e| format!("--static: {e}"))?;
    }
    Ok(profile)
}

/// Entry point for `lightningpanda serve`.
pub fn serve_main(args: &[String]) -> ExitCode {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let options = match parse_args(args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("lightningpanda serve: {e}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let profile = match browser_profile(&options) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("lightningpanda serve: {e}");
            return ExitCode::from(2);
        }
    };
    if options.log_level <= LogLevel::Info {
        for (origin, count) in profile.local_storage.summary() {
            eprintln!("lightningpanda: localStorage preload for {origin}: {count} entries");
        }
        for (origin, dir) in profile.net.static_files.summary() {
            eprintln!("lightningpanda: serving {origin} from {}", dir.display());
        }
    }
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_io().enable_time().build() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("lightningpanda serve: cannot start I/O runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    let result = runtime.block_on(async {
        let addr = SocketAddr::new(options.host, options.port);
        let factory = ThreadPerConnection::with_spare(profile)?;
        let server = Server::bind(addr, options.server.clone(), factory).await?;
        if options.log_level <= LogLevel::Info {
            eprintln!("lightningpanda: CDP server listening on ws://{}/", server.local_addr()?);
        }
        server.run().await
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("lightningpanda serve: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Starts one browser thread per upgraded connection. One thread with its
/// engine already created waits in `spare`; a connection takes it and a new
/// spare starts, so engine start (~8 ms) overlaps the client's own setup
/// instead of delaying its first command.
#[derive(Default)]
pub struct ThreadPerConnection {
    next: AtomicU64,
    spare: std::sync::Mutex<Option<std::sync::mpsc::Sender<Assignment>>>,
    profile: Arc<BrowserProfile>,
}

/// What a prepared browser thread needs to serve a connection.
struct Assignment {
    outbox: Outbox,
    state: Arc<CloseState>,
    ready: oneshot::Sender<()>,
    permits: Arc<Semaphore>,
    /// Dropped when the thread ends, however it ends: releases the slot.
    finished: oneshot::Sender<()>,
}

impl ThreadPerConnection {
    /// A factory whose first spare thread starts creating its engine now, so
    /// the first connection (TOS uses one per process) finds it ready.
    pub fn with_spare(profile: BrowserProfile) -> std::io::Result<Self> {
        let factory = Self { profile: Arc::new(profile), ..Self::default() };
        let spare = factory.spawn_spare()?;
        *factory.spare.lock().unwrap_or_else(|e| e.into_inner()) = Some(spare);
        Ok(factory)
    }

    /// Spawns a thread that creates its engine now and serves a later connection.
    fn spawn_spare(&self) -> std::io::Result<std::sync::mpsc::Sender<Assignment>> {
        let n = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = std::sync::mpsc::channel::<Assignment>();
        let profile = self.profile.clone();
        std::thread::Builder::new().name(format!("lp-browser-{n}")).spawn(move || {
            let prepared = EngineHost::prepare_thread_with(&profile);
            // No connection ever claims this thread when the sender is dropped.
            let Ok(a) = rx.recv() else { return };
            let _finished = a.finished;
            run_browser_thread(prepared, a.outbox, a.state, a.ready, a.permits);
        })?;
        Ok(tx)
    }

    /// The spare thread (started earlier) or a fresh one; then starts the next spare.
    fn claim(&self) -> std::io::Result<std::sync::mpsc::Sender<Assignment>> {
        let taken = self.spare.lock().unwrap_or_else(|e| e.into_inner()).take();
        let next = self.spawn_spare();
        match taken {
            Some(tx) => {
                if let Ok(next) = next {
                    *self.spare.lock().unwrap_or_else(|e| e.into_inner()) = Some(next);
                }
                Ok(tx)
            }
            None => next,
        }
    }
}

/// What the I/O side needs to reach a browser thread.
struct ThreadControl {
    handle: lp_runtime::LoopHandle<lp_browser::Browser>,
    /// Interrupts running JavaScript; `None` if this JSC lacks the capability.
    terminate: Option<lp_js::TerminateHandle>,
}

impl ThreadControl {
    /// Connection gone: stop any running script, then queue teardown ahead
    /// of the per-message permits (the thread may be busy).
    fn close(&self) {
        if let Some(t) = &self.terminate {
            t.terminate();
        }
        let _ = self.handle.post(|_| INBOX.with(|q| q.borrow_mut().push_back(Inbound::Closed)));
    }
}

/// Close coordination between the connection (I/O side) and its browser thread.
#[derive(Default)]
struct CloseState {
    control: OnceLock<ThreadControl>,
    closed: AtomicBool,
}

impl CloseState {
    /// Connection ended. Whichever of this and the thread's start-up comes
    /// second performs the close, so a close during engine start is not lost.
    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        if let Some(control) = self.control.get() {
            control.close();
        }
    }

    fn set_control(&self, control: ThreadControl) {
        if self.control.set(control).is_ok() && self.closed.load(Ordering::SeqCst) {
            self.control.get().expect("just set").close();
        }
    }
}

impl SessionFactory for ThreadPerConnection {
    fn start(&self, _peer: SocketAddr) -> std::io::Result<SessionChannels> {
        let (inbound_tx, mut inbound_rx) = mpsc::channel::<Inbound>(INFLIGHT);
        let (outbox, outbound) = Outbox::channel(DEFAULT_OUTBOUND_BUDGET);
        let state = Arc::new(CloseState::default());
        let (ready_tx, ready_rx) = oneshot::channel::<()>();
        let (finished_tx, finished_rx) = oneshot::channel::<()>();
        let (closed_tx, mut closed_rx) = tokio::sync::watch::channel(false);
        let permits = Arc::new(Semaphore::new(INFLIGHT));
        let thread = self.claim()?;
        // `finished_tx` travels with the assignment and drops when the thread
        // ends, however it ends: that releases the connection slot.
        let assignment = Assignment {
            outbox,
            state: state.clone(),
            ready: ready_tx,
            permits: permits.clone(),
            finished: finished_tx,
        };
        if thread.send(assignment).is_err() {
            return Err(std::io::Error::other("browser thread ended before the connection started"));
        }
        // Forwards socket messages, in order, to the browser thread. At most
        // INFLIGHT messages wait there; beyond that the socket reader stops.
        // It never consumes messages it does not forward.
        let forward_state = state.clone();
        tokio::spawn(async move {
            if ready_rx.await.is_err() {
                return; // engine failed to start; the thread already ended
            }
            let control = forward_state.control.get().expect("set before ready");
            while let Some(Inbound::Text(text)) = inbound_rx.recv().await {
                let permit = tokio::select! {
                    permit = permits.acquire() => permit,
                    // A busy session must not keep this task alive after close.
                    _ = closed_rx.wait_for(|closed| *closed) => return,
                };
                let Ok(permit) = permit else { return };
                permit.forget();
                if control.handle.post(move |_| INBOX.with(|q| q.borrow_mut().push_back(Inbound::Text(text)))).is_err()
                {
                    return;
                }
            }
        });
        let on_close: Box<dyn FnOnce() + Send> = Box::new(move || {
            let _ = closed_tx.send(true);
            state.close();
        });
        Ok(SessionChannels { inbound: inbound_tx, outbound, finished: Some(finished_rx), on_close: Some(on_close) })
    }
}

thread_local! {
    /// Messages delivered to this browser thread, in arrival order.
    static INBOX: RefCell<VecDeque<Inbound>> = const { RefCell::new(VecDeque::new()) };
}

fn run_browser_thread(
    prepared: Result<lp_browser::BrowserThread, crate::msg::CdpError>,
    outbox: Outbox,
    state: Arc<CloseState>,
    ready: oneshot::Sender<()>,
    permits: Arc<Semaphore>,
) {
    let host = match prepared.and_then(|t| EngineHost::with_thread(t, outbox.clone(), EngineHost::body_budget())) {
        Ok(host) => host,
        Err(e) => {
            eprintln!("lightningpanda: {}", e.message);
            return; // dropping outbox and `ready` closes the connection
        }
    };
    let mut browser = Browser::new(host);
    let terminate = browser.host().thread.browser.engine.terminate_handle().ok();
    state.set_control(ThreadControl { handle: browser.host().thread.handle(), terminate });
    if ready.send(()).is_err() {
        return;
    }
    let mut sink = outbox;
    let closing = || INBOX.with(|q| q.borrow().iter().any(|m| matches!(m, Inbound::Closed)));
    loop {
        let has_mail = || INBOX.with(|q| !q.borrow().is_empty());
        let outcome = browser.host_mut().run_until_work(None, has_mail);
        browser.host_mut().flush();
        browser.adopt_popups(&mut sink);
        if closing() || (matches!(outcome, RunOutcome::Idle | RunOutcome::Stopped) && !has_mail()) {
            break; // queued commands of a closed connection are not run
        }
        while let Some(message) = INBOX.with(|q| q.borrow_mut().pop_front()) {
            match message {
                Inbound::Text(text) => {
                    permits.add_permits(1);
                    browser.handle_text(&text, &mut sink);
                    browser.host_mut().flush();
                    browser.adopt_popups(&mut sink);
                    if closing() {
                        break;
                    }
                }
                Inbound::Closed => break,
            }
        }
        if closing() {
            break;
        }
    }
    browser.shutdown();
    browser.host_mut().close_all();
    INBOX.with(|q| q.borrow_mut().clear());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_storage_flag_and_document() {
        let o = parse_args(&["--local-storage".to_owned(), "seed.json".to_owned()]).unwrap();
        assert_eq!(o.local_storage, Some(PathBuf::from("seed.json")));
        assert!(parse_args(&["--local-storage".to_owned()]).is_err());

        let seed = parse_local_storage(
            r#"{"http://localhost:3000/": {"token": "abc", "settings": {"theme": "dark"}, "n": 1},
                "http://api.test": {}}"#,
        )
        .unwrap();
        let mut summary = seed.summary();
        summary.sort();
        assert_eq!(summary, vec![("http://api.test".to_string(), 0), ("http://localhost:3000".to_string(), 3)]);
        let ns = lp_webapi::StorageNamespace::seeded(&seed);
        assert_eq!(ns.get("http://localhost:3000", "settings").as_deref(), Some(r#"{"theme":"dark"}"#));
        assert_eq!(ns.get("http://localhost:3000", "n").as_deref(), Some("1"));

        assert!(parse_local_storage("[]").is_err());
        assert!(parse_local_storage(r#"{"http://a.test": "x"}"#).is_err());
        assert!(parse_local_storage(r#"{"data:text/html,x": {}}"#).is_err());
        assert!(parse_local_storage("{").is_err());

        let missing =
            ServeOptions { local_storage: Some(PathBuf::from("/nonexistent/seed.json")), ..Default::default() };
        assert!(browser_profile(&missing).is_err());
        assert!(browser_profile(&ServeOptions { local_storage: None, ..Default::default() })
            .unwrap()
            .local_storage
            .is_empty());
    }

    #[test]
    fn static_flag() {
        let o = parse_args(&["--static".to_owned(), "http://localhost:3000=/tmp/dist".to_owned()]).unwrap();
        assert_eq!(o.static_roots, vec![("http://localhost:3000".to_string(), PathBuf::from("/tmp/dist"))]);
        let o = parse_args(&[
            "--static".to_owned(),
            "http://a.test=/a".to_owned(),
            "--static".to_owned(),
            " http://b.test:8080/ = /b/with=equals ".to_owned(),
        ])
        .unwrap();
        assert_eq!(
            o.static_roots,
            vec![
                ("http://a.test".to_string(), PathBuf::from("/a")),
                ("http://b.test:8080/".to_string(), PathBuf::from("/b/with=equals"))
            ]
        );
        assert!(parse_args(&["--static".to_owned(), "http://a.test".to_owned()]).is_err());
        assert!(parse_args(&["--static".to_owned(), "=/dir".to_owned()]).is_err());
        assert!(parse_args(&["--static".to_owned()]).is_err());

        let dir = std::env::temp_dir();
        let o = ServeOptions { static_roots: vec![("http://a.test".into(), dir.clone())], ..Default::default() };
        assert_eq!(
            browser_profile(&o).unwrap().net.static_files.summary(),
            vec![("http://a.test".to_string(), dir.clone())]
        );
        let o = ServeOptions {
            static_roots: vec![("http://a.test".into(), dir.join("no-such-dir"))],
            ..Default::default()
        };
        assert!(browser_profile(&o).is_err());
    }
}
