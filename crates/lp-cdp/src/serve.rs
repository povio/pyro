//! `lightningpanda serve`: the CDP server process.
//!
//! The main thread runs a single-threaded tokio runtime for sockets. Each
//! upgraded connection gets its own browser thread owning a JavaScriptCore
//! engine, an [`EngineHost`] and the protocol state (docs/ARCHITECTURE.md).

use std::cell::RefCell;
use std::collections::VecDeque;
use std::net::{IpAddr, SocketAddr};
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

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

Document loading is not integrated yet: pages can only show about:blank.
--experimental-features and --load-resources are recorded for the loader and
have no effect until it lands.
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
        let server = Server::bind(addr, options.server.clone(), ThreadPerConnection::default()).await?;
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

/// Starts one browser thread per upgraded connection.
#[derive(Default)]
pub struct ThreadPerConnection {
    next: AtomicU64,
}

impl SessionFactory for ThreadPerConnection {
    fn start(&self, _peer: SocketAddr) -> std::io::Result<SessionChannels> {
        let n = self.next.fetch_add(1, Ordering::Relaxed);
        let (inbound_tx, mut inbound_rx) = mpsc::channel::<Inbound>(INFLIGHT);
        let (outbox, outbound) = Outbox::channel(DEFAULT_OUTBOUND_BUDGET);
        let (handle_tx, handle_rx) = oneshot::channel();
        let permits = Arc::new(Semaphore::new(INFLIGHT));
        let thread_permits = permits.clone();
        std::thread::Builder::new()
            .name(format!("lp-browser-{n}"))
            .spawn(move || run_browser_thread(outbox, handle_tx, thread_permits))?;
        // Forwards socket messages to the browser thread. At most INFLIGHT
        // messages wait there; beyond that the socket reader stops.
        tokio::spawn(async move {
            let Ok(handle) = handle_rx.await else { return };
            while let Some(message) = inbound_rx.recv().await {
                let closed = matches!(message, Inbound::Closed);
                let Ok(permit) = permits.acquire().await else { return };
                permit.forget();
                if handle.post(move |_| INBOX.with(|q| q.borrow_mut().push_back(message))).is_err() || closed {
                    return;
                }
            }
            // Reader gone without an explicit close: tell the session.
            let _ = handle.post(|_| INBOX.with(|q| q.borrow_mut().push_back(Inbound::Closed)));
        });
        Ok(SessionChannels { inbound: inbound_tx, outbound })
    }
}

thread_local! {
    /// Messages delivered to this browser thread, in arrival order.
    static INBOX: RefCell<VecDeque<Inbound>> = const { RefCell::new(VecDeque::new()) };
}

fn run_browser_thread(
    outbox: Outbox,
    handle_tx: oneshot::Sender<lp_runtime::LoopHandle<lp_browser::Browser>>,
    permits: Arc<Semaphore>,
) {
    let host = match EngineHost::new(outbox.clone()) {
        Ok(host) => host,
        Err(e) => {
            eprintln!("lightningpanda: {}", e.message);
            return; // dropping outbox and handle_tx closes the connection
        }
    };
    let mut browser = Browser::new(host);
    if handle_tx.send(browser.host().thread.handle()).is_err() {
        return;
    }
    let mut sink = outbox;
    loop {
        let has_mail = || INBOX.with(|q| !q.borrow().is_empty());
        let outcome = browser.host_mut().run_until_work(None, has_mail);
        browser.host_mut().flush();
        let mut closed = matches!(outcome, RunOutcome::Idle | RunOutcome::Stopped) && !has_mail();
        while let Some(message) = INBOX.with(|q| q.borrow_mut().pop_front()) {
            permits.add_permits(1);
            match message {
                Inbound::Text(text) => {
                    browser.handle_text(&text, &mut sink);
                    browser.host_mut().flush();
                }
                Inbound::Closed => {
                    closed = true;
                    break;
                }
            }
        }
        if closed {
            break;
        }
    }
    browser.shutdown();
    browser.host_mut().close_all();
    INBOX.with(|q| q.borrow_mut().clear());
}
