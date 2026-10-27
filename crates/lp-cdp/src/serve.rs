//! `lightningpanda serve`: the CDP server process.
//!
//! The main thread runs a single-threaded tokio runtime that accepts sockets
//! and upgrades them. Each upgraded connection is then served entirely by its
//! own browser thread, which owns a JavaScriptCore engine, an [`EngineHost`],
//! the protocol state and the socket (docs/ARCHITECTURE.md, [`crate::conn`]).

use std::cell::RefCell;
use std::collections::VecDeque;
use std::net::{IpAddr, SocketAddr};
use std::process::ExitCode;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use lp_runtime::RunOutcome;
use tokio::sync::oneshot;

use crate::conn::Conn;
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
    socket: std::net::TcpStream,
    /// Bytes read past the upgrade request.
    buffered: Vec<u8>,
    config: ServerConfig,
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
            run_browser_thread(prepared, a.socket, &a.buffered, a.config);
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

impl SessionFactory for ThreadPerConnection {
    fn start(&self, _peer: SocketAddr) -> std::io::Result<SessionChannels> {
        Err(std::io::Error::other("ThreadPerConnection serves the upgraded socket itself (start_owned)"))
    }

    fn owns_sockets(&self) -> bool {
        true
    }

    fn start_owned(
        &self,
        _peer: SocketAddr,
        socket: std::net::TcpStream,
        buffered: Vec<u8>,
        config: &ServerConfig,
    ) -> Option<std::io::Result<oneshot::Receiver<()>>> {
        let (finished_tx, finished_rx) = oneshot::channel::<()>();
        let thread = match self.claim() {
            Ok(thread) => thread,
            Err(e) => return Some(Err(e)),
        };
        // `finished_tx` travels with the assignment and drops when the thread
        // ends, however it ends: that releases the connection slot.
        let assignment = Assignment { socket, buffered, config: config.clone(), finished: finished_tx };
        if thread.send(assignment).is_err() {
            return Some(Err(std::io::Error::other("browser thread ended before the connection started")));
        }
        Some(Ok(finished_rx))
    }
}

/// Replies to pipelined commands are written at least every this many commands.
const FLUSH_EVERY: usize = 16;

/// Serves one connection on this (browser) thread until it ends: the
/// connection's socket is read and written here (see [`crate::conn`]).
fn run_browser_thread(
    prepared: Result<lp_browser::BrowserThread, crate::msg::CdpError>,
    socket: std::net::TcpStream,
    buffered: &[u8],
    config: ServerConfig,
) {
    let (outbox, outbound) = Outbox::channel(DEFAULT_OUTBOUND_BUDGET);
    let conn = match Conn::new(socket, buffered, outbound, config) {
        Ok(conn) => conn,
        Err(e) => {
            eprintln!("lightningpanda: CDP connection: {e}");
            return;
        }
    };
    let host = match prepared.and_then(|t| EngineHost::with_thread(t, outbox.clone(), EngineHost::body_budget())) {
        Ok(host) => host,
        Err(e) => {
            eprintln!("lightningpanda: {}", e.message);
            drop(outbox);
            conn.finish(); // a normal close, as when a session ends
            return;
        }
    };
    let mut browser = Browser::new(host);
    // Network threads that exceed the budget wake this loop to close (1008).
    let handle = browser.host().thread.handle();
    outbox.set_waker(Box::new(move || {
        let _ = handle.post(|_| {});
    }));
    let conn = Rc::new(RefCell::new(conn));
    browser.host_mut().thread.event_loop.watch_fd(Some(conn.borrow().watch()));
    // While JavaScript runs nothing else reads the socket: the watchdog asks
    // the connection whether the client left (or stalled) and stops the script.
    let check = conn.clone();
    browser
        .host()
        .thread
        .browser
        .engine
        .set_interrupt_check(Some(Rc::new(move || check.try_borrow_mut().is_ok_and(|mut c| c.check_during_script()))));

    let mut sink = outbox;
    let mut inbox: VecDeque<Inbound> = VecDeque::new();
    let closed = |inbox: &VecDeque<Inbound>| matches!(inbox.back(), Some(Inbound::Closed));
    'serve: loop {
        let deadline = conn.borrow().next_deadline();
        let outcome = browser.host_mut().run_until_work(deadline, || {
            conn.borrow_mut().service(&mut inbox);
            !inbox.is_empty()
        });
        browser.host_mut().flush();
        browser.adopt_popups(&mut sink);
        conn.borrow_mut().flush(&mut inbox);
        if closed(&inbox) || (matches!(outcome, RunOutcome::Idle | RunOutcome::Stopped) && inbox.is_empty()) {
            break; // queued commands of a closed connection are not run
        }
        let mut unflushed = 0;
        while let Some(message) = inbox.pop_front() {
            match message {
                Inbound::Text(text) => {
                    lp_browser::note_client_activity();
                    let handle = crate::perf::span("cdp.handle_text");
                    browser.handle_text(&text, &mut sink);
                    drop(handle);
                    let flush = crate::perf::span("host.flush");
                    browser.host_mut().flush();
                    drop(flush);
                    browser.adopt_popups(&mut sink);
                    // With more commands already decoded (a pipelining client),
                    // their replies go out together in one write.
                    unflushed += 1;
                    if inbox.is_empty() || unflushed >= FLUSH_EVERY {
                        unflushed = 0;
                        conn.borrow_mut().flush(&mut inbox);
                    }
                    if closed(&inbox) {
                        break 'serve;
                    }
                }
                Inbound::Closed => break 'serve,
            }
        }
        if conn.borrow().ended() {
            break;
        }
    }
    browser.host().thread.browser.engine.set_interrupt_check(None);
    browser.host_mut().thread.event_loop.watch_fd(None);
    browser.shutdown();
    browser.host_mut().close_all();
    drop(sink);
    match Rc::try_unwrap(conn) {
        Ok(conn) => conn.into_inner().finish(),
        Err(_) => debug_assert!(false, "connection still shared at teardown"),
    }
}
