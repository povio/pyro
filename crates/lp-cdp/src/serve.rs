//! `lightningpanda serve`: the CDP server process.
//!
//! The main thread runs a single-threaded tokio runtime that accepts sockets
//! and upgrades them. Each upgraded connection is then served entirely by its
//! own browser thread, which owns a JavaScriptCore engine, an [`EngineHost`],
//! the protocol state and the socket (docs/ARCHITECTURE.md, [`crate::conn`]).

use std::cell::RefCell;
use std::collections::VecDeque;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::process::ExitCode;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use lp_runtime::RunOutcome;
use tokio::sync::oneshot;

use crate::conn::Conn;
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
  --cdp-max-message-size <INT>          default 67108864 (64 MiB)
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
  --static-fallback <spa|network>      missing navigation paths use index.html (default spa)
                                        or the normal network at the unchanged URL
  --prepare <URL>                       keep one page booted at URL per connection, ready
                                        for Lightningpanda.acquirePreparedPage (repeatable;
                                        env LIGHTNINGPANDA_PREPARE, ';'-separated)
  --prepare-refill <quiet|after-close>  when the replacement boots: when the thread is
                                        quiet (default, one extra booted app) or once the
                                        acquired page's context closes (no extra app;
                                        env LIGHTNINGPANDA_PREPARE_REFILL)
  --fork                                serve every connection from a fork() of a process
                                        that booted the prepared pages once (patched
                                        JavaScriptCore only; env LIGHTNINGPANDA_FORK=1)

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
see these requests. Use --static-fallback network for a files-only overlay:
existing assets stay local, while missing paths (including document navigation)
reach the server at their original URL. That server owns any SPA routing.

--prepare: every connection boots a page at URL in its own browser context
(with the --local-storage seed) before any command arrives, and boots the next
one each time a test takes it (once the connection has no open page, or after
15 ms without commands). A client takes one with the browser-level
command Lightningpanda.acquirePreparedPage {url?, profileId?, attach?} -> {targetId,
browserContextId, sessionId?, ready}; the page is announced like a new target
(Target.targetCreated / attachedToTarget), so Playwright sees it as a page of
its default context. Lightningpanda.registerPreparedPage {url, initScripts?,
viewport?, pool?} declares a profile at run time, with the init scripts a
test would otherwise install after the page exists. Lightningpanda.preparedPages
lists what is waiting.

--fork: one template process boots one page per prepared profile (--prepare,
and every Lightningpanda.registerPreparedPage, from any connection), and each
connection is served by a fork() of it. A connection therefore starts with
every profile's page booted, sharing its memory with the template until it
writes to it, and its process exits when it closes. Open one connection per
test (Lightningpanda.capabilities reports {fork: true}); registering a profile
boots it in the template for the connections that follow. A profile's setup
(LP.run steps, registerPreparedPage {setup}) runs once in the template after
the page loads, so every connection starts from the state the steps left.
Each template keeps LIGHTNINGPANDA_FORK_SPARES (default 2) forks waiting, so a
connection does not wait for its fork. Forks run without JavaScriptCore's
DFG/FTL tiers (connections are too short for them to pay off) unless
JSC_useDFGJIT is set.
";

/// How long a spare thread keeps booting prepared pages before it just waits.
const PREBOOT_LIMIT: std::time::Duration = std::time::Duration::from_secs(30);

/// No command for this long after a prepared page was taken: boot the next.
const REFILL_QUIET: std::time::Duration = std::time::Duration::from_millis(15);

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
    pub static_spa_fallback: bool,
    /// `--prepare URL` (or `LIGHTNINGPANDA_PREPARE`): pages booted ahead.
    pub prepare: Vec<String>,
    /// `--prepare-refill after-close`: replacements wait for the acquired
    /// page's context to close.
    pub prepare_refill_after_close: bool,
    /// `--fork` (or `LIGHTNINGPANDA_FORK=1`): connections are forks of a
    /// template process ([`crate::fork`]).
    pub fork: bool,
    /// `--template <socket>`: this process is that template (internal).
    pub template: Option<PathBuf>,
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
            static_spa_fallback: true,
            prepare: std::env::var(PREPARE_ENV)
                .ok()
                .map(|v| v.split(';').map(str::trim).filter(|e| !e.is_empty()).map(str::to_owned).collect())
                .unwrap_or_default(),
            prepare_refill_after_close: std::env::var(PREPARE_REFILL_ENV).is_ok_and(|v| v.trim() == "after-close"),
            fork: std::env::var(FORK_ENV).is_ok_and(|v| v.trim() == "1"),
            template: None,
        }
    }
}

/// Environment equivalent of `--fork` (`1`).
pub const FORK_ENV: &str = "LIGHTNINGPANDA_FORK";

/// Environment equivalent of `--prepare-refill`.
pub const PREPARE_REFILL_ENV: &str = "LIGHTNINGPANDA_PREPARE_REFILL";

/// Environment equivalent of `--prepare` entries, separated by `;` (flags add to it).
pub const PREPARE_ENV: &str = "LIGHTNINGPANDA_PREPARE";

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
            "--static-fallback" => match value()?.as_str() {
                "spa" => o.static_spa_fallback = true,
                "network" => o.static_spa_fallback = false,
                other => return Err(format!("--static-fallback: '{other}' is not spa or network")),
            },
            "--prepare" => o.prepare.push(value()?),
            "--prepare-refill" => match value()?.as_str() {
                "quiet" => o.prepare_refill_after_close = false,
                "after-close" => o.prepare_refill_after_close = true,
                other => return Err(format!("--prepare-refill: '{other}' is not quiet or after-close")),
            },
            "--fork" => o.fork = true,
            "--template" => o.template = Some(PathBuf::from(value()?)),
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
        profile.net.static_files = lp_net::StaticFiles::new(options.static_roots.clone())
            .map_err(|e| format!("--static: {e}"))?
            .with_spa_fallback(options.static_spa_fallback);
    }
    for url in &options.prepare {
        lp_net::Url::parse(url).map_err(|e| format!("--prepare: '{url}' is not an absolute URL ({e})"))?;
        profile.prepared.push(crate::target::PreparedProfile {
            url: url.clone(),
            init_scripts: Vec::new(),
            viewport: None,
            pool: 1,
            refill_after_close: options.prepare_refill_after_close,
            setup: Vec::new(),
            base: None,
        });
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
    if let Some(socket) = &options.template {
        return crate::fork::template_main(&options, socket);
    }
    let profile = match browser_profile(&options) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("lightningpanda serve: {e}");
            return ExitCode::from(2);
        }
    };
    let template = if options.fork {
        if !crate::fork::supported() {
            eprintln!("lightningpanda serve: --fork needs the patched JavaScriptCore (scripts/jsc/build.sh)");
            return ExitCode::from(2);
        }
        match crate::fork::Template::spawn(args) {
            Ok(t) => Some(Arc::new(t)),
            Err(e) => {
                eprintln!("lightningpanda serve: --fork: {e}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        None
    };
    if options.log_level <= LogLevel::Info {
        for (origin, count) in profile.local_storage.summary() {
            eprintln!("lightningpanda: localStorage preload for {origin}: {count} entries");
        }
        for (origin, dir) in profile.net.static_files.summary() {
            eprintln!("lightningpanda: serving {origin} from {}", dir.display());
        }
        for prepared in &profile.prepared {
            eprintln!("lightningpanda: preparing a page at {} per connection", prepared.url);
        }
    }
    // Fonts and the other process-wide render state load on a background
    // thread, so the first screenshot does not pay for them. Not in fork
    // mode: connections are served by forks of a separate template process,
    // which must not start threads before it forks.
    #[cfg(feature = "render")]
    if template.is_none() {
        let _ = lp_render::prewarm();
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
        if let Some(template) = template {
            let server = Server::bind(addr, options.server.clone(), ForkPerConnection { template }).await?;
            if options.log_level <= LogLevel::Info {
                eprintln!("lightningpanda: CDP server listening on ws://{}/ (fork mode)", server.local_addr()?);
            }
            return server.run().await;
        }
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
            let mut prepared = EngineHost::prepare_thread_with(&profile).map(|mut thread| {
                let prebooted = EngineHost::preboot(&mut thread, &profile.prepared);
                (thread, prebooted)
            });
            // Prepared pages boot while no connection exists yet; a
            // connection arriving mid-boot takes the thread at the next task.
            let claimed = RefCell::new(None);
            if let Ok((thread, prebooted)) = &mut prepared {
                if !prebooted.is_empty() {
                    let deadline = std::time::Instant::now() + PREBOOT_LIMIT;
                    let outcome = thread.run_until(Some(deadline), |_| {
                        if claimed.borrow().is_none() {
                            if let Ok(a) = rx.try_recv() {
                                *claimed.borrow_mut() = Some(a);
                            }
                        }
                        claimed.borrow().is_some()
                    });
                    // Booted with nobody waiting: shrink to the live size now.
                    if matches!(outcome, RunOutcome::Idle | RunOutcome::TimedOut) {
                        thread.browser.engine.settle();
                    }
                }
            }
            // No connection ever claims this thread when the sender is dropped.
            let a = match claimed.into_inner() {
                Some(a) => a,
                None => {
                    let Ok(a) = rx.recv() else { return };
                    a
                }
            };
            let _finished = a.finished;
            run_browser_thread(prepared, &profile, a.socket, &a.buffered, a.config, None);
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
        _path: &str,
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

/// `serve --fork`: every upgraded connection goes to a fork of the template.
pub struct ForkPerConnection {
    template: Arc<crate::fork::Template>,
}

impl SessionFactory for ForkPerConnection {
    fn start(&self, _peer: SocketAddr) -> std::io::Result<SessionChannels> {
        Err(std::io::Error::other("ForkPerConnection serves the upgraded socket itself (start_owned)"))
    }

    fn owns_sockets(&self) -> bool {
        true
    }

    fn start_owned(
        &self,
        _peer: SocketAddr,
        path: &str,
        socket: std::net::TcpStream,
        buffered: Vec<u8>,
        _config: &ServerConfig,
    ) -> Option<std::io::Result<oneshot::Receiver<()>>> {
        // `ws://host/?profile=N` asks for a fork of that profile's snapshot.
        let profile = path
            .split_once('?')
            .and_then(|(_, query)| query.split('&').find_map(|kv| kv.strip_prefix("profile=")))
            .and_then(|v| v.parse().ok());
        Some(crate::fork::connect(self.template.socket(), socket, buffered, profile))
    }
}

/// Replies to pipelined commands are written at least every this many commands.
const FLUSH_EVERY: usize = 16;

/// Serves one connection on this (browser) thread until it ends: the
/// connection's socket is read and written here (see [`crate::conn`]).
/// `fork_register` makes it a fork child of the template (`serve --fork`).
pub(crate) fn run_browser_thread(
    prepared: Result<(lp_browser::BrowserThread, Vec<crate::engine::Prebooted>), crate::msg::CdpError>,
    profile: &BrowserProfile,
    socket: std::net::TcpStream,
    buffered: &[u8],
    config: ServerConfig,
    fork_register: Option<crate::target::RegisterPrepared>,
) {
    let (outbox, outbound) = Outbox::channel(DEFAULT_OUTBOUND_BUDGET);
    let host = match prepared.and_then(|(t, prebooted)| {
        let mut host = EngineHost::with_thread(t, outbox.clone(), EngineHost::body_budget())?;
        host.adopt_prebooted(prebooted);
        Ok(host)
    }) {
        Ok(host) => host,
        Err(e) => {
            eprintln!("lightningpanda: {}", e.message);
            drop(outbox);
            if let Ok(conn) = Conn::new(socket, buffered, outbound, config) {
                conn.finish(); // a normal close, as when a session ends
            }
            return;
        }
    };
    let mut browser = Browser::with_prepared(host, profile.prepared.clone());
    if let Some(register) = fork_register {
        browser.set_fork_child(register);
    }
    serve_browser(browser, outbox, outbound, socket, buffered, config);
}

/// Serves one connection with protocol state that already exists: `browser`
/// writes to `outbox`, whose frames `outbound` carries to the socket.
pub(crate) fn serve_browser(
    mut browser: Browser<EngineHost>,
    outbox: Outbox,
    outbound: crate::outbox::OutboundRx,
    socket: std::net::TcpStream,
    buffered: &[u8],
    config: ServerConfig,
) {
    let conn = match Conn::new(socket, buffered, outbound, config) {
        Ok(conn) => conn,
        Err(e) => {
            eprintln!("lightningpanda: CDP connection: {e}");
            return;
        }
    };
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
    // A JavaScript dialog suspends its script while this connection is
    // served by a nested read loop until a session answers it (engine/dialog.rs).
    let paused = conn.clone();
    browser.host().set_dialog_pump(Some(Rc::new(move |take: &mut dyn FnMut(&str) -> crate::engine::PumpStep| {
        paused.try_borrow_mut().is_ok_and(|mut c| c.pause(take))
    })));

    let mut sink = outbox;
    let mut inbox: VecDeque<Inbound> = VecDeque::new();
    let closed = |inbox: &VecDeque<Inbound>| matches!(inbox.back(), Some(Inbound::Closed));
    // When the last command that was not a prepared-page status query ran.
    let mut quiet_since = std::time::Instant::now();
    'serve: loop {
        // A prepared page was taken: boot its replacement once no page is
        // open (the test closed its page), else only after a quiet moment
        // with no command, so a running test is not delayed. Loaded
        // prepared pages are settled at the same moments.
        let refill = browser.needs_refill();
        let quiet_at = (refill || browser.needs_settle()).then(|| {
            if refill && browser.no_open_pages() {
                quiet_since
            } else {
                quiet_since + REFILL_QUIET
            }
        });
        if quiet_at.is_some_and(|at| std::time::Instant::now() >= at) && inbox.is_empty() {
            browser.settle_prepared();
            browser.refill_prepared();
            continue;
        }
        let deadline = match (conn.borrow().next_deadline(), quiet_at) {
            (Some(d), Some(q)) => Some(d.min(q)),
            (d, q) => d.or(q),
        };
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
                    // Polling the pool's status is not test activity.
                    if !text.contains("\"Lightningpanda.preparedPages\"") {
                        quiet_since = std::time::Instant::now();
                    }
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
    browser.host().set_dialog_pump(None);
    browser.host_mut().thread.event_loop.watch_fd(None);
    browser.shutdown();
    browser.host_mut().close_all();
    lp_webapi::join_stopped(std::time::Duration::from_secs(1));
    drop(sink);
    match Rc::try_unwrap(conn) {
        Ok(conn) => conn.into_inner().finish(),
        Err(_) => debug_assert!(false, "connection still shared at teardown"),
    }
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
    fn prepare_refill_flag() {
        let o = parse_args(&["--prepare".to_owned(), "http://a.test/".to_owned()]).unwrap();
        assert!(!o.prepare_refill_after_close);
        let profile = browser_profile(&o).unwrap();
        assert!(!profile.prepared[0].refill_after_close);
        let o = parse_args(&[
            "--prepare-refill".to_owned(),
            "after-close".to_owned(),
            "--prepare".to_owned(),
            "http://a.test/".to_owned(),
        ])
        .unwrap();
        assert!(browser_profile(&o).unwrap().prepared[0].refill_after_close);
        assert!(parse_args(&["--prepare-refill".to_owned(), "later".to_owned()]).is_err());
        assert!(parse_args(&["--prepare".to_owned(), "/relative".to_owned()]).is_ok());
        assert!(browser_profile(&parse_args(&["--prepare".to_owned(), "/relative".to_owned()]).unwrap()).is_err());
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

#[cfg(test)]
mod static_fallback_tests {
    use super::*;

    #[test]
    fn static_fallback_cli_preserves_spa_default_and_configures_network_misses() {
        let dir = std::env::temp_dir().join(format!("lp-static-fallback-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("index.html"), "<p>SPA").unwrap();
        let base = vec!["--static".to_owned(), format!("http://overlay.test={}", dir.display())];
        let spa = parse_args(&base).unwrap();
        assert!(spa.static_spa_fallback);
        let mut args = base;
        args.extend(["--static-fallback".to_owned(), "network".to_owned()]);
        let network = parse_args(&args).unwrap();
        assert!(!network.static_spa_fallback);
        let missing = lp_net::Url::parse("http://overlay.test/api/media/proof?signature=value").unwrap();
        assert!(browser_profile(&spa).unwrap().net.static_files.lookup(&missing, true).unwrap().spa_fallback);
        assert!(browser_profile(&network).unwrap().net.static_files.lookup(&missing, true).is_none());
        assert!(parse_args(&["--static-fallback".into(), "unknown".into()]).is_err());
        assert!(parse_args(&["--static-fallback".into()]).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
