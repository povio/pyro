//! `lightningpanda serve`: the CDP server process.
//!
//! The main thread runs a single-threaded tokio runtime for sockets. Each
//! upgraded connection gets its own browser thread owning a JavaScriptCore
//! engine, an [`EngineHost`] and the protocol state (docs/ARCHITECTURE.md).

use std::cell::RefCell;
use std::collections::VecDeque;
use std::net::{IpAddr, SocketAddr};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use lp_runtime::RunOutcome;
use tokio::sync::{mpsc, oneshot, Semaphore};

use crate::engine::EngineHost;
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

HTTP and data: document loading are supported; browser API coverage is partial.
--experimental-features and --load-resources are accepted for CLI compatibility;
the current loader configuration is fixed.
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
        }
    }
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
            other => return Err(format!("unknown option '{other}'")),
        }
    }
    Ok(o)
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
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_io().enable_time().build() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("lightningpanda serve: cannot start I/O runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    let result = runtime.block_on(async {
        let addr = SocketAddr::new(options.host, options.port);
        let server = Server::bind(addr, options.server.clone(), ThreadPerConnection::with_spare()?).await?;
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
    pub fn with_spare() -> std::io::Result<Self> {
        let factory = Self::default();
        let spare = factory.spawn_spare()?;
        *factory.spare.lock().unwrap_or_else(|e| e.into_inner()) = Some(spare);
        Ok(factory)
    }

    /// Spawns a thread that creates its engine now and serves a later connection.
    fn spawn_spare(&self) -> std::io::Result<std::sync::mpsc::Sender<Assignment>> {
        let n = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = std::sync::mpsc::channel::<Assignment>();
        std::thread::Builder::new().name(format!("lp-browser-{n}")).spawn(move || {
            let prepared = EngineHost::prepare_thread();
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
        let assignment =
            Assignment { outbox, state: state.clone(), ready: ready_tx, permits: permits.clone(), finished: finished_tx };
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
                let posted = crate::perf::now();
                let deliver = move |_: &mut lp_browser::Browser| {
                    INBOX.with(|q| q.borrow_mut().push_back(Inbound::Text(text)));
                    POSTED.with(|q| q.borrow_mut().push_back(posted));
                };
                if control.handle.post(deliver).is_err() {
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
    /// When each `Inbound::Text` in INBOX was posted by the I/O thread
    /// (zero-sized unless perf-trace is on).
    static POSTED: RefCell<VecDeque<crate::perf::Stamp>> = const { RefCell::new(VecDeque::new()) };
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
                    if let Some(posted) = POSTED.with(|q| q.borrow_mut().pop_front()) {
                        crate::perf::record_since("io.to_browser", posted);
                    }
                    permits.add_permits(1);
                    lp_browser::note_client_activity();
                    let handle = crate::perf::span("cdp.handle_text");
                    browser.handle_text(&text, &mut sink);
                    drop(handle);
                    let flush = crate::perf::span("host.flush");
                    browser.host_mut().flush();
                    drop(flush);
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
    POSTED.with(|q| q.borrow_mut().clear());
}
