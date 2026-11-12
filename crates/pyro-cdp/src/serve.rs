//! `pyro serve`: the CDP server process.
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

use pyro_runtime::RunOutcome;
use tokio::sync::oneshot;

use crate::conn::Conn;
use crate::engine::{BrowserProfile, EngineHost};
use crate::outbox::{Outbox, DEFAULT_OUTBOUND_BUDGET};
use crate::server::{Inbound, Server, ServerConfig, SessionChannels, SessionFactory};
use crate::target::Browser;
use crate::ws::WsConfig;

const USAGE: &str = "\
usage: pyro serve [OPTIONS]

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
                                        (env PYRO_LOCAL_STORAGE)
  --static <ORIGIN>=<DIR>               serve GET requests for ORIGIN from DIR, no server
                                        needed there (repeatable; env PYRO_STATIC,
                                        entries separated by ';')
  --static-fallback <spa|network>      missing navigation paths use index.html (default spa)
                                        or the normal network at the unchanged URL
  --proxy <URL>                         send requests through this HTTP proxy,
                                        http://[user:pass@]host:port: CONNECT for https,
                                        absolute-form for http; no SOCKS. A context's
                                        own proxyServer takes its place
  --proxy-bypass <LIST>                 hosts that skip the proxy, in Chrome's
                                        --proxy-bypass-list syntax. Loopback always
                                        skips it, unless the list has <-loopback>
  --prepare <URL>                       keep one page booted at URL per connection, ready
                                        for Pyro.acquirePreparedPage (repeatable;
                                        env PYRO_PREPARE, ';'-separated)
  --prepare-refill <quiet|after-close>  when the replacement boots: when the thread is
                                        quiet (default, one extra booted app) or once the
                                        acquired page's context closes (no extra app;
                                        env PYRO_PREPARE_REFILL)
  --fork                                serve every connection from a fork() of a process
                                        that booted the prepared pages once (patched
                                        JavaScriptCore only; env PYRO_FORK=1)
  --spare-engine <on|off>               keep a browser thread with its JavaScriptCore
                                        engine created ahead of the next connection
                                        (~8 ms off its first command, ~5 MB while idle).
                                        Default: on with --prepare (a test suite's
                                        prepared pages), off otherwise;
                                        env PYRO_SPARE_ENGINE=on|off
      --engine-reuse <on|off|thread|always>
                                       a connection that ends without having made
                                       a page leaves its browser thread and its
                                       JavaScript engine to the next one
                                       (default: on; at most one waits, for at
                                       most 10 s); thread: the thread after pages
                                       too, never the engine;
                                       always: the engine after pages too, once
                                       it is proven to hold nothing of them
                                       env PYRO_ENGINE_REUSE=...
  --render-prewarm <on|off>             scan the system fonts and warm the renderer at
                                        startup, so the first screenshot or PDF does not
                                        pay for it (~3 MB while idle). Default: on with
                                        --prepare, off otherwise;
                                        env PYRO_RENDER_PREWARM=on|off
  --hyphen-digit-breaks <on|off>        allow a line break between a hyphen and a digit
                                        when the hyphen follows a letter or digit
                                        (4b87-|916e, 10-|20), as Chrome does; a minus
                                        sign stays with its number (-120, A -5).
                                        Applies to line breaking and min-content
                                        widths (table columns). Default: off;
                                        env PYRO_HYPHEN_DIGIT_BREAKS=on|off
  --parent-pid <PID>                    exit when process PID exits, however it ends
                                        (env PYRO_PARENT_PID; the SDK's
                                        Pyro.launch() sets it to its own pid)

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
command Pyro.acquirePreparedPage {url?, profileId?, attach?} -> {targetId,
browserContextId, sessionId?, ready}; the page is announced like a new target
(Target.targetCreated / attachedToTarget), so Playwright sees it as a page of
its default context. Pyro.registerPreparedPage {url, initScripts?,
viewport?, pool?} declares a profile at run time, with the init scripts a
test would otherwise install after the page exists. Pyro.preparedPages
lists what is waiting.

--fork: one template process boots one page per prepared profile (--prepare,
and every Pyro.registerPreparedPage, from any connection), and each
connection is served by a fork() of it. A connection therefore starts with
every profile's page booted, sharing its memory with the template until it
writes to it, and its process exits when it closes. Open one connection per
test (Pyro.capabilities reports {fork: true}); registering a profile
boots it in the template for the connections that follow. A profile's setup
(Pyro.run steps, registerPreparedPage {setup}) runs once in the template after
the page loads, so every connection starts from the state the steps left.
Each template keeps PYRO_FORK_SPARES (default 2) forks waiting, so a
connection does not wait for its fork. Forks run without JavaScriptCore's
DFG/FTL tiers (connections are too short for them to pay off) unless
JSC_useDFGJIT is set.

Shared profiles (--fork): several clients (test workers in other processes)
can use one server. registerPreparedPage {name, ...} registers a profile
under a name every client of the server shares: the same definition under
the same name finds the profile already booted, a different one is refused
while another client holds it. Pyro.adoptPreparedPage {name} ->
{profileId, url, initScripts, viewport, setup, base} holds a profile another
client registered, unregisterPreparedPage {name} releases this connection's
hold, and Pyro.preparedProfiles lists the names with their holders.
A connection's holds end when it closes; a profile is retired once no
connection holds it and no other profile is derived from it.
";

/// How long a spare thread keeps booting prepared pages before it just waits.
const PREBOOT_LIMIT: std::time::Duration = std::time::Duration::from_secs(30);

/// The stack of a browser thread: a main thread's (8 MB on macOS and Linux),
/// as `pyro fetch`/`eval` run their page on, and as Chrome's
/// renderer main thread and our worker threads have. Layout recurses once
/// per tree level (Taffy's algorithms), about 2.7 KB per nested block in a
/// release build: a spawned thread's default 2 MB aborted the whole server
/// past about 770 nested blocks. Layout and paint now grow the stack on
/// demand (blitz-dom `util::ensure_stack`), so no depth aborts; 8 MB keeps
/// about 3,000 levels on the thread's own stack. JSC bounds its own
/// recursion by the stack it finds (`maxPerThreadStackUsage`, 5 MB), as on
/// those threads, and a layout read made near that limit grows the stack
/// too. Untouched stack pages cost no memory.
const BROWSER_STACK_BYTES: usize = 8 * 1024 * 1024;
/// No command for this long after a prepared page was taken: boot the next.
const REFILL_QUIET: std::time::Duration = std::time::Duration::from_millis(15);
/// With no command for this long, a closed page whose collection was put off
/// (`Engine::release_closed`) is collected (`Engine::settle`): a page close
/// itself only leaves the collection pending, so the closing command returns
/// at once and the next page does not re-commit the pages a purge on the
/// command's stack had just returned. A test suite's gap between tests is
/// shorter than this.
const RELEASE_QUIET: std::time::Duration = std::time::Duration::from_secs(1);
/// With no command for this long after a close, every realm's compiled code
/// is dropped too (`Engine::settle_idle`, the bytecode cache included): the
/// client may stay idle for good. Longer than a test's pause between
/// actions: dropped after every quiet second, Playwright's 293 KB injected
/// script was re-parsed by the next test (2-3 ms CPU), and a page waiting
/// on a timer lost its compiled code.
const CODE_QUIET: std::time::Duration = std::time::Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServeOptions {
    pub host: IpAddr,
    pub port: u16,
    pub server: ServerConfig,
    pub log_level: LogLevel,
    pub experimental_features: Vec<String>,
    pub load_resources: Vec<String>,
    /// `--local-storage` (or `PYRO_LOCAL_STORAGE`): JSON file of
    /// per-origin localStorage entries preloaded into every context.
    pub local_storage: Option<PathBuf>,
    /// `--static ORIGIN=DIR` (or `PYRO_STATIC`): directories served
    /// in place of origins.
    pub static_roots: Vec<(String, PathBuf)>,
    pub static_spa_fallback: bool,
    /// `--proxy URL`: forward proxy of every context without its own.
    pub proxy: Option<String>,
    /// `--proxy-bypass LIST`.
    pub proxy_bypass: Option<String>,
    /// `--prepare URL` (or `PYRO_PREPARE`): pages booted ahead.
    pub prepare: Vec<String>,
    /// `--prepare-refill after-close`: replacements wait for the acquired
    /// page's context to close.
    pub prepare_refill_after_close: bool,
    /// `--fork` (or `PYRO_FORK=1`): connections are forks of a
    /// template process ([`crate::fork`]).
    pub fork: bool,
    /// `--template <socket>`: this process is that template (internal).
    pub template: Option<PathBuf>,
    /// `--spare-engine` (or `PYRO_SPARE_ENGINE`): a browser thread
    /// with its engine waits for the next connection. `None` follows the
    /// mode: on with `--prepare` (a test suite, where the first command of a
    /// connection should not wait for an engine), off otherwise (the spare
    /// costs memory for as long as the server idles).
    pub spare_engine: Option<bool>,
    /// `--engine-reuse` (or `PYRO_ENGINE_REUSE`): a connection's
    /// browser thread and engine serve the next connection ([`Park`]).
    pub engine_reuse: EngineReuse,
    /// `--render-prewarm` (or `PYRO_RENDER_PREWARM`): warm the
    /// renderer's process-wide state at startup. `None` follows the mode
    /// like `spare_engine`.
    pub render_prewarm: Option<bool>,
    /// `--hyphen-digit-breaks` (or `PYRO_HYPHEN_DIGIT_BREAKS`):
    /// Chrome's line break between a hyphen and a digit
    /// (`pyro_render::set_hyphen_digit_breaks`). Off by default.
    pub hyphen_digit_breaks: bool,
    /// `--parent-pid` (or `PYRO_PARENT_PID`): exit with this
    /// process ([`crate::lifetime`]).
    pub parent_pid: Option<libc::pid_t>,
}

impl ServeOptions {
    /// Whether a test suite is being served: prepared pages are what the
    /// test client takes. The eager startup work pays off there.
    fn test_suite_mode(&self) -> bool {
        !self.prepare.is_empty()
    }

    pub fn spare_engine(&self) -> bool {
        self.spare_engine.unwrap_or_else(|| self.test_suite_mode())
    }

    pub fn render_prewarm(&self) -> bool {
        self.render_prewarm.unwrap_or_else(|| self.test_suite_mode())
    }
}

/// `on`/`off` (also `1`/`0`, `true`/`false`).
fn parse_switch(flag: &str, value: &str) -> Result<bool, String> {
    match value.trim() {
        "on" | "1" | "true" => Ok(true),
        "off" | "0" | "false" => Ok(false),
        other => Err(format!("{flag}: '{other}' is not on or off")),
    }
}

/// What a connection's end leaves to the next connection ([`Park`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EngineReuse {
    /// Nothing: every connection has a new thread and a new engine.
    Off,
    /// Its browser thread. The engine is destroyed and the next connection
    /// makes one on the thread.
    ///
    /// Not the default: a thread that keeps running pages competes worse
    /// for a processor than a new one. With twice as many busy processes
    /// as cores (macOS, 10 cores, 20 hogs), connections that load one page
    /// one after another: 20 to 25 of 450 took over three times the median
    /// on a kept thread, 3 to 5 on new threads (p90 17.7 ms against
    /// 12.8 ms, mean 10.6 to 12.6 ms against 9.3 to 10.2 ms). With idle
    /// cores the two are equal in time.
    Thread,
    /// Its browser thread and its engine when nothing used the engine
    /// (`pyro_js::Engine::untouched`: a client that connected and asked the
    /// browser things without ever making a page). A used engine is
    /// destroyed and its thread ends with it, for the reason above.
    ///
    /// Measured, instructions per connection that opens one page: 20.96 M
    /// with a new thread and engine each time, 20.24 M with the thread
    /// kept ([`EngineReuse::Thread`]), 21.26 M with the engine kept too
    /// ([`EngineReuse::Always`]):
    /// proving a used engine clean (delete all code, a full collection, a
    /// walk of the heap for its counts) costs a little more than destroying
    /// it and making another, and the next page finds no compiled code
    /// either way. An unused engine needs none of that: 3.7 M -> 0.7 M.
    #[default]
    On,
    /// Its browser thread and its engine, a used one once
    /// `pyro_js::Engine::recycle` has proven it clean: the next connection
    /// starts without waiting for an engine, at the price above.
    Always,
}

impl EngineReuse {
    fn parse(flag: &str, value: &str) -> Result<Self, String> {
        match value.trim() {
            "thread" => Ok(Self::Thread),
            "always" => Ok(Self::Always),
            other => parse_switch(flag, other)
                .map(|on| if on { Self::On } else { Self::Off })
                .map_err(|_| format!("{flag}: '{other}' is not on, off, thread or always")),
        }
    }
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
            proxy: None,
            proxy_bypass: None,
            prepare: std::env::var(PREPARE_ENV)
                .ok()
                .map(|v| v.split(';').map(str::trim).filter(|e| !e.is_empty()).map(str::to_owned).collect())
                .unwrap_or_default(),
            prepare_refill_after_close: std::env::var(PREPARE_REFILL_ENV).is_ok_and(|v| v.trim() == "after-close"),
            fork: std::env::var(FORK_ENV).is_ok_and(|v| v.trim() == "1"),
            template: None,
            spare_engine: std::env::var(SPARE_ENGINE_ENV).ok().and_then(|v| parse_switch(SPARE_ENGINE_ENV, &v).ok()),
            engine_reuse: std::env::var(ENGINE_REUSE_ENV)
                .ok()
                .and_then(|v| EngineReuse::parse(ENGINE_REUSE_ENV, &v).ok())
                .unwrap_or_default(),
            render_prewarm: std::env::var(RENDER_PREWARM_ENV)
                .ok()
                .and_then(|v| parse_switch(RENDER_PREWARM_ENV, &v).ok()),
            hyphen_digit_breaks: std::env::var(HYPHEN_DIGIT_BREAKS_ENV)
                .ok()
                .and_then(|v| parse_switch(HYPHEN_DIGIT_BREAKS_ENV, &v).ok())
                .unwrap_or(false),
            parent_pid: std::env::var(PARENT_PID_ENV).ok().and_then(|v| parse_pid(&v).ok()),
        }
    }
}

/// Environment equivalent of `--hyphen-digit-breaks` (`on`/`off`).
pub const HYPHEN_DIGIT_BREAKS_ENV: &str = "PYRO_HYPHEN_DIGIT_BREAKS";

/// Environment equivalent of `--parent-pid`.
pub const PARENT_PID_ENV: &str = "PYRO_PARENT_PID";

fn parse_pid(value: &str) -> Result<libc::pid_t, String> {
    match value.trim().parse::<libc::pid_t>() {
        Ok(pid) if pid > 0 => Ok(pid),
        _ => Err(format!("--parent-pid: '{value}' is not a process id")),
    }
}

/// Environment equivalent of `--fork` (`1`).
pub const FORK_ENV: &str = "PYRO_FORK";

/// Environment equivalent of `--spare-engine` (`on`/`off`).
pub const SPARE_ENGINE_ENV: &str = "PYRO_SPARE_ENGINE";

/// Environment equivalent of `--engine-reuse` (`on`/`off`).
pub const ENGINE_REUSE_ENV: &str = "PYRO_ENGINE_REUSE";

/// Environment equivalent of `--render-prewarm` (`on`/`off`).
pub const RENDER_PREWARM_ENV: &str = "PYRO_RENDER_PREWARM";

/// Environment equivalent of `--prepare-refill`.
pub const PREPARE_REFILL_ENV: &str = "PYRO_PREPARE_REFILL";

/// Environment equivalent of `--prepare` entries, separated by `;` (flags add to it).
pub const PREPARE_ENV: &str = "PYRO_PREPARE";

/// Environment equivalent of `--local-storage` (the flag wins).
pub const LOCAL_STORAGE_ENV: &str = "PYRO_LOCAL_STORAGE";
/// Environment equivalent of `--static` entries, separated by `;` (flags add to it).
pub const STATIC_ENV: &str = "PYRO_STATIC";

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
            "--proxy" => o.proxy = Some(value()?),
            "--proxy-bypass" => o.proxy_bypass = Some(value()?),
            "--prepare" => o.prepare.push(value()?),
            "--prepare-refill" => match value()?.as_str() {
                "quiet" => o.prepare_refill_after_close = false,
                "after-close" => o.prepare_refill_after_close = true,
                other => return Err(format!("--prepare-refill: '{other}' is not quiet or after-close")),
            },
            "--fork" => o.fork = true,
            "--spare-engine" => o.spare_engine = Some(parse_switch("--spare-engine", &value()?)?),
            "--engine-reuse" => o.engine_reuse = EngineReuse::parse("--engine-reuse", &value()?)?,
            "--render-prewarm" => o.render_prewarm = Some(parse_switch("--render-prewarm", &value()?)?),
            "--hyphen-digit-breaks" => o.hyphen_digit_breaks = parse_switch("--hyphen-digit-breaks", &value()?)?,
            "--parent-pid" => o.parent_pid = Some(parse_pid(&value()?)?),
            "--template" => o.template = Some(PathBuf::from(value()?)),
            other => return Err(format!("unknown option '{other}'")),
        }
    }
    Ok(o)
}

/// Parses the `--local-storage` document: `{origin: {key: value}}`. String
/// values are stored as they are; any other JSON value is stored as its
/// serialized text (apps commonly keep JSON objects in localStorage).
pub fn parse_local_storage(text: &str) -> Result<pyro_webapi::StorageSeed, String> {
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
    pyro_webapi::StorageSeed::new(seed)
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
        profile.net.static_files = pyro_net::StaticFiles::new(options.static_roots.clone())
            .map_err(|e| format!("--static: {e}"))?
            .with_spa_fallback(options.static_spa_fallback);
    }
    if let Some(server) = &options.proxy {
        profile.net.proxy =
            pyro_net::ProxyConfig::parse(server, options.proxy_bypass.as_deref()).map_err(|e| format!("--proxy: {e}"))?;
    } else if options.proxy_bypass.is_some() {
        return Err("--proxy-bypass needs --proxy".into());
    }
    for url in &options.prepare {
        pyro_net::Url::parse(url).map_err(|e| format!("--prepare: '{url}' is not an absolute URL ({e})"))?;
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

/// Entry point for `pyro serve`.
pub fn serve_main(args: &[String]) -> ExitCode {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let options = match parse_args(args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("pyro serve: {e}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    // Before any renderer exists, in the template process too (it gets the
    // same arguments): every renderer takes the default when it is made.
    #[cfg(feature = "render")]
    pyro_render::set_hyphen_digit_breaks(options.hyphen_digit_breaks);
    if let Some(socket) = &options.template {
        return crate::fork::template_main(&options, socket);
    }
    // The template first, while this process has one thread and no engine:
    // it is a fork of this process, and it starts (half of a fork-mode
    // start) while the server gets ready to listen.
    let template = if options.fork {
        if !crate::fork::supported() {
            eprintln!("pyro serve: --fork needs the patched JavaScriptCore (scripts/jsc/build.sh)");
            return ExitCode::from(2);
        }
        // SAFETY: nothing above starts a thread or JavaScriptCore.
        match unsafe { crate::fork::Template::spawn(&options) } {
            Ok(t) => Some(Arc::new(t)),
            Err(e) => {
                eprintln!("pyro serve: --fork: {e}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        None
    };
    // Before anything that can take a while: the process may already be gone.
    if let Some(pid) = options.parent_pid {
        if let Err(e) = crate::lifetime::exit_with(pid, crate::fork::cleanup_for_exit) {
            eprintln!("pyro serve: --parent-pid: {e}");
            return ExitCode::FAILURE;
        }
    }
    let profile = match browser_profile(&options) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("pyro serve: {e}");
            return ExitCode::from(2);
        }
    };
    if let Some(template) = &template {
        // Killed by a signal, the server still removes its templates' sockets.
        if let Err(e) = crate::lifetime::cleanup_on_signals(crate::fork::cleanup_for_exit) {
            eprintln!("pyro serve: --fork: {e}");
        }
        // The server listens while the template starts: the first connection
        // waits for it. A template that cannot start ends the server.
        let waiting = template.clone();
        let watch = std::thread::Builder::new().name("pyro-template-wait".into()).spawn(move || {
            crate::fork::remove_stale_sockets();
            if let Err(e) = waiting.wait_ready() {
                eprintln!("pyro serve: --fork: {e}");
                crate::fork::cleanup_for_exit();
                // SAFETY: ends the process without static destructors
                // while the server thread runs.
                unsafe { libc::_exit(1) };
            }
        });
        if let Err(e) = watch {
            eprintln!("pyro serve: --fork: {e}");
            return ExitCode::FAILURE;
        }
    }
    if options.log_level <= LogLevel::Info {
        for (origin, count) in profile.local_storage.summary() {
            eprintln!("pyro: localStorage preload for {origin}: {count} entries");
        }
        for (origin, dir) in profile.net.static_files.summary() {
            eprintln!("pyro: serving {origin} from {}", dir.display());
        }
        for prepared in &profile.prepared {
            eprintln!("pyro: preparing a page at {} per connection", prepared.url);
        }
    }
    // With --render-prewarm, fonts and the other process-wide render state
    // load on a background thread, so the first screenshot does not pay for
    // them. Not in fork mode: connections are served by forks of a separate
    // template process, which must not start threads before it forks.
    #[cfg(feature = "render")]
    if template.is_none() && options.render_prewarm() {
        let _ = pyro_render::prewarm();
    }
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_io().enable_time().build() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("pyro serve: cannot start I/O runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    let result = runtime.block_on(async {
        let addr = SocketAddr::new(options.host, options.port);
        if let Some(template) = template {
            let server = Server::bind(addr, options.server.clone(), ForkPerConnection { template }).await?;
            if options.log_level <= LogLevel::Info {
                eprintln!("pyro: CDP server listening on ws://{}/ (fork mode)", server.local_addr()?);
            }
            return server.run().await;
        }
        let factory = if options.spare_engine() {
            ThreadPerConnection::with_spare_and_reuse(profile, options.engine_reuse)?
        } else {
            ThreadPerConnection::without_spare(profile).reuse(options.engine_reuse)
        };
        let server = Server::bind(addr, options.server.clone(), factory).await?;
        if options.log_level <= LogLevel::Info {
            eprintln!("pyro: CDP server listening on ws://{}/", server.local_addr()?);
        }
        server.run().await
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("pyro serve: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Starts one browser thread per upgraded connection. One thread with its
/// engine already created waits in `spare`; a connection takes it and a new
/// spare starts, so engine start (~8 ms) overlaps the client's own setup
/// instead of delaying its first command.
///
/// A connection that ends leaves its thread and engine to the next one
/// ([`Park`]).
#[derive(Default)]
pub struct ThreadPerConnection {
    next: AtomicU64,
    spare: std::sync::Mutex<Option<std::sync::mpsc::Sender<Assignment>>>,
    profile: Arc<BrowserProfile>,
    park: Arc<Park>,
}

/// How long a browser thread waits with its engine for another connection.
/// After it the thread ends and the engine goes: a server nobody connects
/// to holds no engine, as before its first connection.
const PARK_LIMIT: std::time::Duration = std::time::Duration::from_secs(10);

/// A parked thread that no connection took for this long collects its
/// engine's heap and returns the allocators' free pages to the system. Not at once: a client that
/// reconnects right away (a test runner, one connection per test) would pay
/// for the purge and for the page faults after it at every connection.
const PARK_RELEASE_AFTER: std::time::Duration = std::time::Duration::from_millis(250);

/// What starts a browser thread's next turn: where its connection will
/// come from, and whether that connection exists yet.
struct Turn {
    assignment: std::sync::mpsc::Receiver<Assignment>,
    /// No connection waits yet (`ThreadPerConnection::spawn_thread`).
    ahead: bool,
}

/// The one browser thread that waits, with its engine, for the next
/// connection after its own ended.
///
/// A connection's end drops everything the connection had on its thread: the
/// protocol state (sessions, targets, interception, emulation, bindings,
/// permissions), the browser with its pages, contexts and storage, the
/// event loop with its timers and tasks, the network client with its cookie
/// jar, cache and sockets, the worker threads. Two things outlive it: the
/// thread, whose per-thread state is then proven empty or reset
/// (`pyro_webapi::thread_state`, `pyro_browser::reset_thread`), and the
/// engine, the JavaScriptCore VM with the classes registered in it, which
/// `pyro_js::Engine::recycle` proves to hold nothing of a script. An engine
/// or a thread that fails its proof is dropped and the thread ends, as
/// every thread did before; so does one that finds the place taken.
///
/// The engine never leaves its thread: a VM entered from two threads in
/// turn is where JavaScriptCore's lock takes a new thread for its owner.
pub struct Park {
    mode: std::sync::Mutex<EngineReuse>,
    waiting: std::sync::Mutex<Option<(u64, std::sync::mpsc::Sender<Turn>)>>,
    next: AtomicU64,
    parked: AtomicU64,
    reused: AtomicU64,
    dropped: AtomicU64,
    expired: AtomicU64,
    refusal: std::sync::Mutex<Option<&'static str>>,
}

impl Default for Park {
    fn default() -> Self {
        Self {
            mode: std::sync::Mutex::new(EngineReuse::On),
            waiting: std::sync::Mutex::new(None),
            next: AtomicU64::new(1),
            parked: AtomicU64::new(0),
            reused: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            expired: AtomicU64::new(0),
            refusal: std::sync::Mutex::new(None),
        }
    }
}

impl Park {
    fn mode(&self) -> EngineReuse {
        *self.mode.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// What connections' ends have done with their engines so far.
    pub fn stats(&self) -> EngineReuseStats {
        EngineReuseStats {
            parked: self.parked.load(Ordering::Relaxed),
            reused: self.reused.load(Ordering::Relaxed),
            dropped: self.dropped.load(Ordering::Relaxed),
            expired: self.expired.load(Ordering::Relaxed),
        }
    }

    /// Why the last engine that was not kept was dropped.
    pub fn last_refusal(&self) -> Option<&'static str> {
        *self.refusal.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Whether a thread waits with its engine now.
    pub fn occupied(&self) -> bool {
        self.waiting.lock().unwrap_or_else(|e| e.into_inner()).is_some()
    }

    fn refuse(&self, why: &'static str) {
        self.dropped.fetch_add(1, Ordering::Relaxed);
        *self.refusal.lock().unwrap_or_else(|e| e.into_inner()) = Some(why);
        reuse_log(why);
    }

    /// Takes the place if it is free: the ticket to give it up with, and
    /// where the thread's next turn arrives.
    fn enter(&self) -> Option<(u64, std::sync::mpsc::Receiver<Turn>)> {
        let mut waiting = self.waiting.lock().unwrap_or_else(|e| e.into_inner());
        if waiting.is_some() {
            return None;
        }
        let ticket = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = std::sync::mpsc::channel();
        *waiting = Some((ticket, tx));
        Some((ticket, rx))
    }

    /// Gives the place up. `false` when a connection took the thread
    /// meanwhile: its turn is on the way.
    fn leave(&self, ticket: u64) -> bool {
        let mut waiting = self.waiting.lock().unwrap_or_else(|e| e.into_inner());
        if waiting.as_ref().is_some_and(|(t, _)| *t == ticket) {
            *waiting = None;
            return true;
        }
        false
    }

    fn take(&self) -> Option<std::sync::mpsc::Sender<Turn>> {
        self.waiting.lock().unwrap_or_else(|e| e.into_inner()).take().map(|(_, tx)| tx)
    }
}

/// Counts of what connections' ends did with their engines
/// ([`Park::stats`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EngineReuseStats {
    /// Threads left waiting, with or without an engine.
    pub parked: u64,
    /// Connections served by an engine an earlier connection left.
    pub reused: u64,
    /// Threads that ended with their connection: the engine or the thread
    /// was not proven clean, or the place was taken.
    pub dropped: u64,
    /// Threads that ended after waiting [`PARK_LIMIT`] for nothing.
    pub expired: u64,
}

/// `PYRO_ENGINE_REUSE_LOG=1`: one line on stderr per connection
/// end, saying what became of its engine (diagnostic).
fn reuse_log(what: &str) {
    static ON: std::sync::LazyLock<bool> =
        std::sync::LazyLock::new(|| std::env::var("PYRO_ENGINE_REUSE_LOG").is_ok_and(|v| v.trim() == "1"));
    if *ON {
        eprintln!("pyro: engine reuse: {what}");
    }
}

/// Proves this thread and `engine` clean after their connection ended and
/// everything else of it was dropped, resetting what is theirs to reset.
/// The engine first: its collection is what frees the wrappers that keep
/// closed pages' state alive.
fn scrub(engine: &pyro_js::Engine, workers_joined: bool) -> Result<(), &'static str> {
    if !workers_joined {
        return Err("a worker thread is still running");
    }
    engine.recycle(pyro_browser::clean_heap())?;
    scrub_thread(workers_joined)
}

/// The thread's part of [`scrub`], after the engine's or after the engine
/// was dropped.
fn scrub_thread(workers_joined: bool) -> Result<(), &'static str> {
    if !workers_joined {
        return Err("a worker thread is still running");
    }
    EngineHost::forget_thread_state();
    pyro_webapi::thread_state::reset_thread()?;
    // Rendering state: every page's document was forgotten when it closed;
    // the renderer still holds the font collection with the web fonts pages
    // registered, and the fetcher of the old client's network.
    #[cfg(feature = "render")]
    pyro_layout::release_thread();
    pyro_browser::reset_thread()
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
        Self::with_spare_and_reuse(profile, EngineReuse::On)
    }

    /// [`Self::with_spare`], with [`Self::engine_reuse`] decided before the
    /// first thread starts.
    pub fn with_spare_and_reuse(profile: BrowserProfile, reuse: EngineReuse) -> std::io::Result<Self> {
        let factory = Self { profile: Arc::new(profile), ..Self::default() }.reuse(reuse);
        let spare = factory.spawn_thread(true)?;
        *factory.spare.lock().unwrap_or_else(|e| e.into_inner()) = Some(spare);
        Ok(factory)
    }

    /// A factory that starts each browser thread when its connection arrives:
    /// nothing runs, and no engine exists, while the server is idle.
    pub fn without_spare(profile: BrowserProfile) -> Self {
        Self { profile: Arc::new(profile), ..Self::default() }
    }

    /// Whether a connection's end leaves its thread and engine to the next
    /// connection (on unless turned off here).
    pub fn engine_reuse(self, on: bool) -> Self {
        self.reuse(if on { EngineReuse::On } else { EngineReuse::Off })
    }

    /// What a connection's end leaves to the next connection.
    pub fn reuse(self, mode: EngineReuse) -> Self {
        *self.park.mode.lock().unwrap_or_else(|e| e.into_inner()) = mode;
        self
    }

    /// Where this factory's threads wait between connections: its counts.
    pub fn park(&self) -> Arc<Park> {
        self.park.clone()
    }

    /// A browser thread that serves a later connection: the one that waits
    /// with its engine ([`Park`]) if there is one, else a new thread, which
    /// creates its engine now. `ahead` says no connection waits for it yet:
    /// only then is the first page's realm built here too
    /// (`prepare_thread_with`).
    fn spawn_thread(&self, ahead: bool) -> std::io::Result<std::sync::mpsc::Sender<Assignment>> {
        let (tx, rx) = std::sync::mpsc::channel::<Assignment>();
        let mut turn = Turn { assignment: rx, ahead };
        if let Some(parked) = self.park.take() {
            match parked.send(turn) {
                Ok(()) => return Ok(tx),
                Err(unsent) => turn = unsent.0, // the thread is gone (it panicked)
            }
        }
        let n = self.next.fetch_add(1, Ordering::Relaxed);
        let profile = self.profile.clone();
        let park = self.park.clone();
        let builder = std::thread::Builder::new().name(format!("pyro-browser-{n}")).stack_size(BROWSER_STACK_BYTES);
        builder.spawn(move || browser_thread(&profile, &park, turn))?;
        Ok(tx)
    }

    /// The spare thread (started earlier) or a fresh one; then starts the next spare.
    fn claim(&self) -> std::io::Result<std::sync::mpsc::Sender<Assignment>> {
        let taken = self.spare.lock().unwrap_or_else(|e| e.into_inner()).take();
        // With a spare taken, the new thread is the next spare; without one
        // it serves this connection, which is already waiting.
        let next = self.spawn_thread(taken.is_some());
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

/// A browser thread's life: one connection after another, each with a new
/// browser around the thread's engine, for as long as the engine and the
/// thread come out of a connection provably clean and [`Park`] has room.
fn browser_thread(profile: &Arc<BrowserProfile>, park: &Park, first: Turn) {
    let mut turn = first;
    let mut engine: Option<pyro_js::Engine> = None;
    loop {
        let Turn { assignment: rx, ahead } = turn;
        if engine.is_some() {
            park.reused.fetch_add(1, Ordering::Relaxed);
        }
        // Where the browser leaves the engine when it is dropped.
        let mode = park.mode();
        let left = (mode != EngineReuse::Off).then(pyro_browser::EngineSlot::default);
        {
            let mut prepared = EngineHost::prepare_thread_around(profile, ahead, engine.take()).map(|mut thread| {
                if let Some(left) = &left {
                    thread.return_engine(left.clone());
                }
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
            let finished = a.finished;
            let before = pyro_js::process_footprint();
            let workers_joined = run_browser_thread(prepared, profile, a.socket, &a.buffered, a.config);
            // The connection is over for its client and for the server's
            // count: what follows delays nobody.
            drop(finished);
            let Some(left) = left.and_then(|left| left.borrow_mut().take()) else {
                release_if_grown(before);
                return;
            };
            let keep = match mode {
                EngineReuse::Always => true,
                EngineReuse::On => left.untouched(),
                EngineReuse::Thread | EngineReuse::Off => false,
            };
            if !keep {
                // The engine goes; what its wrappers held goes with it.
                drop(left);
                release_if_grown(before);
                if mode == EngineReuse::On {
                    // A thread that ran pages is not kept (`EngineReuse::On`).
                    park.refuse("the engine was used");
                    return;
                }
                if let Err(why) = scrub_thread(workers_joined) {
                    park.refuse(why);
                    return;
                }
            } else {
                if let Err(why) = scrub(&left, workers_joined) {
                    park.refuse(why);
                    drop(left);
                    release_if_grown(before);
                    return;
                }
                engine = Some(left);
            }
        }
        let Some((ticket, turns)) = park.enter() else {
            park.refuse("another engine already waits");
            return;
        };
        park.parked.fetch_add(1, Ordering::Relaxed);
        reuse_log("parked");
        let waited = turns.recv_timeout(PARK_RELEASE_AFTER).or_else(|_| {
            // Collects, and returns the free pages of JavaScriptCore's
            // allocator and of the process's to the system.
            if let Some(engine) = &engine {
                engine.settle_idle();
            }
            turns.recv_timeout(PARK_LIMIT.saturating_sub(PARK_RELEASE_AFTER))
        });
        turn = match waited {
            Ok(turn) => turn,
            Err(_) => {
                if park.leave(ticket) {
                    park.expired.fetch_add(1, Ordering::Relaxed);
                    reuse_log("expired");
                    return;
                }
                // Taken at the last moment: the turn was or is being sent.
                let Ok(turn) = turns.recv() else { return };
                turn
            }
        };
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
        Some(self.template.socket().and_then(|template| crate::fork::connect(template, socket, buffered, profile)))
    }
}

/// Replies to pipelined commands are written at least every this many commands.
const FLUSH_EVERY: usize = 16;

/// Serves one connection on this (browser) thread until it ends: the
/// connection's socket is read and written here (see [`crate::conn`]).
pub(crate) fn run_browser_thread(
    prepared: Result<(pyro_browser::BrowserThread, Vec<crate::engine::Prebooted>), crate::msg::CdpError>,
    profile: &BrowserProfile,
    socket: std::net::TcpStream,
    buffered: &[u8],
    config: ServerConfig,
) -> bool {
    let (outbox, outbound) = Outbox::channel(DEFAULT_OUTBOUND_BUDGET);
    let host = match prepared.and_then(|(t, prebooted)| {
        let mut host = EngineHost::with_thread(t, outbox.clone(), EngineHost::body_budget())?;
        host.adopt_prebooted(prebooted);
        Ok(host)
    }) {
        Ok(host) => host,
        Err(e) => {
            eprintln!("pyro: {}", e.message);
            drop(outbox);
            if let Ok(conn) = Conn::new(socket, buffered, outbound, config) {
                conn.finish(); // a normal close, as when a session ends
            }
            return true;
        }
    };
    let browser = Browser::with_prepared(host, profile.prepared.clone());
    serve_browser(browser, outbox, outbound, socket, buffered, config, AfterServe::TearDown)
}

/// What a connection's end leaves to do in the process that served it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum AfterServe {
    /// The process lives on: every page is closed and its memory released.
    TearDown,
    /// The process exits right after (a forked connection's): its pages
    /// unload as when a tab closes, the connection closes cleanly, and the
    /// rest (disposing realms, collecting, dropping the engine) is left to
    /// the exit. The browser is leaked.
    Exit,
}

/// A connection's thread returns the free memory its engine leaves behind
/// when the process ends up this much larger than the connection found it
/// (`release_if_grown`).
const THREAD_END_RELEASE_GROWTH: u64 = 8 << 20;

/// For a browser thread whose connection ended and whose engine is gone
/// (not kept for the next connection): returns what the engine and its
/// pages left free, when the process grew by `THREAD_END_RELEASE_GROWTH`
/// since `before`. Nobody else returns it (the heap's pages of an exited
/// thread are purged only when another one collects). Measured after a run
/// of five SPA loads, the server idle again: 118 MB resident (64 MB
/// footprint) without this call, 83 MB (29 MB) with it. Only for a
/// connection that left the process larger: the release is 1.9M
/// instructions, half of what a connection that opens nothing costs. Where
/// the footprint cannot be read (not macOS) nothing is released, as before:
/// releasing at every connection's end is that cost with no measurement of
/// what it returns there. An engine that is kept releases while parked
/// (`Engine::settle_idle`).
fn release_if_grown(before: Option<u64>) {
    let grown = match (before, pyro_js::process_footprint()) {
        (Some(before), Some(after)) => after.saturating_sub(before) >= THREAD_END_RELEASE_GROWTH,
        _ => false,
    };
    if grown {
        pyro_js::release_thread_memory();
    }
}

/// Serves one connection with protocol state that already exists: `browser`
/// writes to `outbox`, whose frames `outbound` carries to the socket.
/// Whether every worker thread of its pages has ended (`TearDown` only;
/// `false` when one outlasted the wait for it).
pub(crate) fn serve_browser(
    mut browser: Browser<EngineHost>,
    outbox: Outbox,
    outbound: crate::outbox::OutboundRx,
    socket: std::net::TcpStream,
    buffered: &[u8],
    config: ServerConfig,
    after: AfterServe,
) -> bool {
    let conn = match Conn::new(socket, buffered, outbound, config) {
        Ok(conn) => conn,
        Err(e) => {
            eprintln!("pyro: CDP connection: {e}");
            return true;
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
        let engine = &browser.host().thread.browser.engine;
        // A closed context's service worker has ended: what it kept alive
        // on this thread is collectable only now, so its end counts as a
        // close. Measured closing a context whose page a service worker
        // controlled: 182 MB before this collection, 85 MB after; the one
        // inside the close itself had freed 71 of 317 MB. A worker that a
        // live page ended is not reported (`pyro_webapi::worker_ended`).
        if pyro_webapi::take_worker_ended() {
            engine.release_closed();
        }
        let code_at = engine.code_release_pending().then(|| quiet_since + CODE_QUIET);
        let release_at = engine.release_pending().then(|| quiet_since + RELEASE_QUIET).or(code_at);
        if release_at.is_some_and(|at| std::time::Instant::now() >= at) && inbox.is_empty() {
            if code_at.is_some_and(|at| std::time::Instant::now() >= at) {
                engine.settle_idle();
            } else {
                engine.settle();
            }
            continue;
        }
        let quiet_at = match (quiet_at, release_at) {
            (Some(q), Some(r)) => Some(q.min(r)),
            (q, r) => q.or(r),
        };
        let deadline = match (conn.borrow().next_deadline(), quiet_at) {
            (Some(d), Some(q)) => Some(d.min(q)),
            (d, q) => d.or(q),
        };
        let outcome = browser.host_mut().run_until_work(deadline, || {
            conn.borrow_mut().service(&mut inbox);
            !inbox.is_empty() || pyro_webapi::worker_ended()
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
                    pyro_browser::note_client_activity();
                    let received = std::time::Instant::now();
                    let handle = crate::perf::span("cdp.handle_text");
                    browser.handle_text(&text, &mut sink);
                    drop(handle);
                    // Polling the pool's status is not test activity. The
                    // parsed method says so (no scan of the message text).
                    if !browser.last_was_pool_poll() {
                        quiet_since = received;
                    }
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
    if crate::fork_stats::init() {
        crate::fork_stats::child_modules(browser.host().thread.browser.engine.module_precompile_stats());
    }
    crate::fork_stats::mark("served");
    browser.host().thread.browser.engine.set_interrupt_check(None);
    browser.host().set_dialog_pump(None);
    browser.host_mut().thread.event_loop.watch_fd(None);
    if after == AfterServe::Exit {
        browser.host().thread.browser.engine.skip_release_until_exit();
    }
    browser.shutdown();
    let mut workers_joined = true;
    if after == AfterServe::TearDown {
        browser.host_mut().close_all();
        workers_joined = pyro_webapi::join_stopped(std::time::Duration::from_secs(1));
    }
    drop(sink);
    match Rc::try_unwrap(conn) {
        Ok(conn) => conn.into_inner().finish(),
        Err(_) => debug_assert!(false, "connection still shared at teardown"),
    }
    crate::fork_stats::mark("closed");
    if after == AfterServe::Exit {
        std::mem::forget(browser);
    }
    workers_joined
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_reuse_flag() {
        assert_eq!(parse_args(&[]).unwrap().engine_reuse, EngineReuse::On);
        let with = |value: &str| parse_args(&["--engine-reuse".to_owned(), value.to_owned()]);
        assert_eq!(with("off").unwrap().engine_reuse, EngineReuse::Off);
        assert_eq!(with("on").unwrap().engine_reuse, EngineReuse::On);
        assert_eq!(with("thread").unwrap().engine_reuse, EngineReuse::Thread);
        assert_eq!(with("always").unwrap().engine_reuse, EngineReuse::Always);
        assert!(with("sometimes").is_err());
        assert!(parse_args(&["--engine-reuse".to_owned()]).is_err());
    }

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
        let ns = pyro_webapi::StorageNamespace::seeded(&seed);
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
    fn hyphen_digit_breaks_flag() {
        let args = |a: &[&str]| parse_args(&a.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>());
        if std::env::var_os(HYPHEN_DIGIT_BREAKS_ENV).is_none() {
            assert!(!args(&[]).unwrap().hyphen_digit_breaks);
        }
        assert!(args(&["--hyphen-digit-breaks", "on"]).unwrap().hyphen_digit_breaks);
        assert!(!args(&["--hyphen-digit-breaks", "off"]).unwrap().hyphen_digit_breaks);
        assert!(args(&["--hyphen-digit-breaks", "maybe"]).is_err());
        assert!(args(&["--hyphen-digit-breaks"]).is_err());
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
        let proxy = |args: &[&str]| {
            let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
            browser_profile(&parse_args(&args).unwrap()).map(|p| p.net.proxy.is_some())
        };
        assert_eq!(proxy(&[]), Ok(false));
        assert_eq!(proxy(&["--proxy", "http://127.0.0.1:3128", "--proxy-bypass", "<-loopback>,*.test"]), Ok(true));
        assert!(proxy(&["--proxy", "socks5://127.0.0.1:1080"]).unwrap_err().contains("SOCKS"));
        assert!(proxy(&["--proxy-bypass", "*.test"]).is_err());
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
        let dir = std::env::temp_dir().join(format!("pyro-static-fallback-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("index.html"), "<p>SPA").unwrap();
        let base = vec!["--static".to_owned(), format!("http://overlay.test={}", dir.display())];
        let spa = parse_args(&base).unwrap();
        assert!(spa.static_spa_fallback);
        let mut args = base;
        args.extend(["--static-fallback".to_owned(), "network".to_owned()]);
        let network = parse_args(&args).unwrap();
        assert!(!network.static_spa_fallback);
        let missing = pyro_net::Url::parse("http://overlay.test/api/media/proof?signature=value").unwrap();
        assert!(browser_profile(&spa).unwrap().net.static_files.lookup(&missing, true).unwrap().spa_fallback);
        assert!(browser_profile(&network).unwrap().net.static_files.lookup(&missing, true).is_none());
        assert!(parse_args(&["--static-fallback".into(), "unknown".into()]).is_err());
        assert!(parse_args(&["--static-fallback".into()]).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
