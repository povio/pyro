//! `serve --fork`: copy-on-write page snapshots.
//!
//! A booted app costs its startup (parsing and running its JavaScript) once
//! per page. In fork mode one template process boots one page per prepared
//! profile, and every CDP connection is served by a `fork()` of it: the
//! connection starts with those pages already booted, sharing their memory
//! with the template copy-on-write, and exits when the connection closes.
//!
//! The server process accepts and upgrades sockets as usual, then hands each
//! one to the template over a Unix socket (`SCM_RIGHTS`), together with the
//! write end of a pipe the connection's process keeps open until it exits:
//! the server holds the connection slot until that pipe reports EOF.
//! Profiles registered at run time (`Lightningpanda.registerPreparedPage`)
//! travel the same way, from the connection's process to the template.
//!
//! `fork()` keeps only the calling thread, so the template forks only with
//! no other thread alive: network I/O threads are stopped and JavaScriptCore
//! parks its helper threads first (patched JavaScriptCore, see
//! `crates/lp-jsc-sys/webkit/patches/0002-fork-support.patch`), and both
//! restart afterwards in the parent and in the child.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use serde_json::Value;

use crate::engine::EngineHost;
use crate::outbox::{Outbound, OutboundRx, Outbox, DEFAULT_OUTBOUND_BUDGET};
use crate::serve::ServeOptions;
use crate::server::ServerConfig;
use crate::target::{Browser, ForkProfiles, Host, NamedProfile, PreparedProfile, ProfileRelease};
use crate::ws::OutFrame;

/// A connection waits at most this long for loaded pages to go quiet: a
/// request still in flight at the fork fails in the template (its network
/// threads stop), and polling apps keep timers due.
const QUIET_WAIT: Duration = Duration::from_secs(2);

/// A page that has not loaded by then is forked as it is.
const LOAD_WAIT: Duration = Duration::from_secs(30);

/// A profile's setup steps must finish within this.
const SETUP_TIMEOUT: Duration = Duration::from_secs(60);

/// A derived profile's template is forked off its base's once that one is
/// up (and set up): connections for it, and profiles derived from it, wait
/// at most this long for it.
const DERIVE_WAIT: Duration = Duration::from_secs(90);

/// Forked children each template keeps waiting for a connection
/// (`LIGHTNINGPANDA_FORK_SPARES`), so that a connection does not wait for its
/// fork.
fn spares_wanted() -> usize {
    std::env::var("LIGHTNINGPANDA_FORK_SPARES").ok().and_then(|v| v.parse().ok()).unwrap_or(2)
}

/// What the root template does with the module scripts of the static roots
/// before its first page (`LIGHTNINGPANDA_FORK_PRECOMPILE`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Precompile {
    /// `off`: nothing; every process parses and compiles what it loads.
    Off,
    /// `modules` (the default): each module is parsed and its top-level code
    /// compiled; its functions are compiled at their first call, in the
    /// process that calls.
    Modules,
    /// `functions`: the code of every function too. Measured on the ToS suite
    /// it saves a page 2% more instructions than `modules` and costs it a
    /// third more page faults and 50 MB: a page calls a few hundred of the
    /// 70,000 functions, each on a page of memory it then touches.
    Functions,
}

fn precompile_mode() -> Precompile {
    match std::env::var("LIGHTNINGPANDA_FORK_PRECOMPILE").as_deref() {
        Ok("off" | "0") => Precompile::Off,
        Ok("functions") => Precompile::Functions,
        _ => Precompile::Modules,
    }
}

/// Source bytes of module scripts the root template precompiles at most.
const PRECOMPILE_BUDGET: usize = 64 * 1024 * 1024;

/// Parses and compiles the static roots' module scripts in the root
/// template's VM before its first page (`Engine::precompile_module`). The
/// template and every process forked from it, at any depth, then load those
/// modules without parsing or compiling them: the code is part of the heap
/// they share. A file that does not parse as a module is skipped.
fn precompile_static_modules(engine: &lp_js::Engine, files: &lp_net::StaticFiles) {
    let mode = precompile_mode();
    if mode == Precompile::Off || files.is_empty() || !lp_js::Engine::has_module_precompile() {
        return;
    }
    let started = Instant::now();
    let (mut tried, mut kept, mut bytes) = (0usize, 0usize, 0usize);
    for (url, file) in files.memory_files() {
        // `.cjs` has the JavaScript MIME type and is never a module.
        let module = file.content_type.starts_with("text/javascript")
            && file.path.extension().is_none_or(|e| !e.eq_ignore_ascii_case("cjs"));
        if !module || bytes + file.body.len() > PRECOMPILE_BUDGET {
            continue;
        }
        tried += 1;
        // The text a page's fetch of the file hands to the engine.
        let source = lp_webapi::module_script::utf8_decode(&file.body);
        if engine.precompile_module(&source, url.as_str(), mode == Precompile::Functions) {
            kept += 1;
            bytes += file.body.len();
        }
    }
    crate::fork_stats::precompiled(tried, kept, bytes, engine.module_precompile_stats().functions, started.elapsed());
}

/// Whether a forked connection's process leaves its teardown to the exit
/// (`LIGHTNINGPANDA_FORK_FAST_EXIT=0` tears down as a shared process does).
fn fast_exit() -> bool {
    std::env::var("LIGHTNINGPANDA_FORK_FAST_EXIT").map_or(true, |v| v != "0")
}

/// Ends a forked child that is done. With the fast exit, without the C
/// library's exit handlers and destructors: nothing in it is needed any more.
fn exit_child(code: i32) -> ! {
    if fast_exit() {
        // SAFETY: ends the process at once; its files close with it.
        unsafe { libc::_exit(code) }
    }
    std::process::exit(code)
}

/// Most file descriptors one message carries.
const MAX_FDS: usize = 4;

/// Whether this JavaScriptCore can fork (patched build).
pub fn supported() -> bool {
    lp_js::Capabilities::detect().fork
}

/// A message to the template.
#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum Message {
    /// Serve a connection, from the snapshot of `profile` if given (else the
    /// root's): fds are the upgraded socket and the pipe's write end.
    Connect {
        buffered: Vec<u8>,
        profile: Option<usize>,
    },
    /// Boot a profile for later connections (root only); replies `Registered`.
    /// With a name, `lease` holds the profile under that name.
    Register {
        profile: PreparedProfile,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        lease: Option<u64>,
    },
    /// Hold the profile registered as `name` (root only); replies `Adopted`.
    Adopt {
        name: String,
        lease: u64,
    },
    /// Drop `lease`'s hold on `name` (root only); replies `Released`.
    Release {
        name: String,
        lease: u64,
    },
    /// Open a lease (root only); replies `Leased`. The connection's process
    /// keeps this stream open while it lives: its holds end with it.
    Lease,
    /// List the named profiles (root only); replies `Listed`.
    Profiles,
    Unregister {
        profile_id: usize,
    },
    RetireDerived {
        profile_id: usize,
    },
    /// Fork a template for profile `id` off this one's snapshot of `base`,
    /// listening on `socket` (root to the template holding `base`).
    Derive {
        base: usize,
        id: usize,
        profile: PreparedProfile,
        socket: PathBuf,
    },
    /// A spare took a connection: wakes its template to fork a replacement.
    Taken,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Registered {
    profile_id: Option<usize>,
    error: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct Unregistered {
    removed: bool,
    error: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Adopted {
    profile_id: Option<usize>,
    profile: Option<PreparedProfile>,
    error: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct Released {
    release: Option<ProfileRelease>,
    error: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct Leased {
    lease: u64,
}

#[derive(Serialize, Deserialize)]
struct Listed {
    profiles: Vec<NamedProfile>,
}

// ---- Server side ---------------------------------------------------------

/// The template process of a `serve --fork` server; killed (and its socket
/// removed) when dropped.
pub struct Template {
    child: std::process::Child,
    socket: PathBuf,
}

// The template's stdin is a pipe from the server, which only the server
// writes to: EOF there means the server is gone (even killed), and the
// template exits too.

/// JavaScriptCore options the template starts with unless the environment
/// sets them. JSC reads them once, before the template's first VM, and its
/// forks inherit them. A fork serves one short connection:
///
/// - `useDFGJIT=false`: code it tiers up to the DFG (and FTL) is compiled on
///   helper threads that compete with the other forks for cores, and the
///   connection usually ends before that code pays for itself. Baseline JIT
///   and LLInt stay on. `JSC_useDFGJIT=true` gets the default tiers back.
/// - `jitMemoryReservationSize` 32 MB instead of 512 MB: macOS's `fork()`
///   walks the whole `MAP_JIT` reservation and copies the code in it, so a
///   fork took 15 ms of the template's time with 512 MB, 3.5 ms with 32 MB.
///   Pages of a mid-size SPA use about 6 MB of it; at 16 MB JavaScriptCore
///   already compiles less (it raises its tier-up thresholds as the pool
///   fills: 2.6% more instructions per page).
/// - `heapGrowthMaxIncrease=3` (JavaScriptCore's own default; other
///   processes run at 1, see `lp_js::configure_process`): a fork may
///   allocate about three times the template's live heap before it collects
///   instead of once, so most pages never collect. A collection in a fork
///   marks memory it shares with the template (copying the pages it writes)
///   for a process that exits seconds later: pages of a mid-size SPA ran
///   2.5% fewer instructions for 9 MB more memory each on average. The
///   template still settles (one full collection) before it forks.
const FORK_JSC_DEFAULTS: [(&str, &str); 3] =
    [("JSC_useDFGJIT", "false"), ("JSC_jitMemoryReservationSize", "33554432"), ("JSC_heapGrowthMaxIncrease", "3")];

impl Template {
    /// Starts `lightningpanda serve --template <socket> <args>` and waits
    /// until it listens.
    pub fn spawn(args: &[String]) -> std::io::Result<Template> {
        // Servers that were killed could not remove theirs.
        remove_stale_sockets();
        remove_socket_files(std::process::id());
        let socket = socket_path(std::process::id());
        let exe = std::env::current_exe()?;
        let mut command = std::process::Command::new(exe);
        command
            .arg("serve")
            .arg("--template")
            .arg(&socket)
            .args(args.iter().filter(|a| a.as_str() != "--fork"))
            .stdin(std::process::Stdio::piped())
            // The template ends with this server (its stdin), not with our parent.
            .env_remove(crate::serve::PARENT_PID_ENV);
        // Unless the user set them (this process's own defaults do not count).
        for (name, value) in FORK_JSC_DEFAULTS {
            if std::env::var_os(name).is_none() || lp_js::is_process_default(name) {
                command.env(name, value);
            }
        }
        let child = command.spawn()?;
        TEMPLATE_PID.store(child.id() as i32, std::sync::atomic::Ordering::Relaxed);
        let mut template = Template { child, socket };
        let deadline = Instant::now() + Duration::from_secs(30);
        while UnixStream::connect(&template.socket).is_err() {
            if let Some(status) = template.child.try_wait()? {
                return Err(std::io::Error::other(format!("template process exited: {status}")));
            }
            if Instant::now() > deadline {
                return Err(std::io::Error::other("template process did not start"));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok(template)
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }
}

impl Drop for Template {
    fn drop(&mut self) {
        TEMPLATE_PID.store(0, std::sync::atomic::Ordering::Relaxed);
        let _ = self.child.kill();
        let _ = self.child.wait();
        // Derived templates' sockets too (`<socket>.<id>`, `.conn`).
        remove_socket_files(std::process::id());
    }
}

/// The template process of this server, for [`cleanup_for_exit`].
static TEMPLATE_PID: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

/// For a server process that exits without dropping its [`Template`] (a
/// signal, or the process it runs for exited): ends the template (the
/// templates derived from it and their spares follow) and removes every
/// socket file of this server. Callable from any thread.
pub fn cleanup_for_exit() {
    let pid = TEMPLATE_PID.swap(0, std::sync::atomic::Ordering::Relaxed);
    if pid > 0 {
        // SAFETY: our own child, not reaped yet (only the Template reaps it).
        unsafe { libc::kill(pid, libc::SIGKILL) };
    }
    remove_socket_files(std::process::id());
}

/// The root template's control socket of the server with pid `server`.
/// Derived templates listen beside it on `<socket>.<id>`, and every
/// template's connection socket is `<control>.conn`.
fn socket_path(server: u32) -> PathBuf {
    std::env::temp_dir().join(format!("lightningpanda-{server}.sock"))
}

/// The server pid in a socket file name of [`socket_path`]'s form.
fn socket_server(name: &str) -> Option<u32> {
    let (pid, rest) = name.strip_prefix("lightningpanda-")?.split_once(".sock")?;
    (rest.is_empty() || rest.starts_with('.')).then_some(())?;
    pid.parse().ok()
}

/// Socket files (by name) in the temporary directory.
fn socket_files() -> impl Iterator<Item = (String, PathBuf)> {
    use std::os::unix::fs::FileTypeExt;
    std::fs::read_dir(std::env::temp_dir()).into_iter().flatten().flatten().filter_map(|entry| {
        let socket = entry.file_type().is_ok_and(|t| t.is_socket());
        let name = entry.file_name().into_string().ok()?;
        socket.then(|| (name, entry.path()))
    })
}

/// Removes every socket file of the server with pid `server`.
fn remove_socket_files(server: u32) {
    for (name, path) in socket_files() {
        if socket_server(&name) == Some(server) {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Removes the socket files of servers that are gone (killed before they
/// could remove them).
fn remove_stale_sockets() {
    for (name, path) in socket_files() {
        let Some(pid) = socket_server(&name).and_then(|p| libc::pid_t::try_from(p).ok()) else { continue };
        // SAFETY: signal 0 only checks that the process exists.
        let gone =
            unsafe { libc::kill(pid, 0) } != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
        if gone {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Hands an upgraded connection to the template; the receiver resolves once
/// the process serving it has exited.
pub fn connect(
    template: &Path,
    socket: std::net::TcpStream,
    buffered: Vec<u8>,
    profile: Option<usize>,
) -> std::io::Result<tokio::sync::oneshot::Receiver<()>> {
    let (read_end, write_end) = pipe()?;
    // A derived snapshot's template takes its connections directly; anything
    // else goes to the root's (which forwards what is not up yet).
    // The root also takes one whose derived template is not listening
    // (yet): its socket file can exist a moment before it does, and a
    // connect then is refused.
    let derived = profile.map(|id| conn_socket(&PathBuf::from(format!("{}.{id}", template.display()))));
    let mut stream = match derived.filter(|p| p.exists()).map(UnixStream::connect) {
        Some(Ok(stream)) => stream,
        _ => UnixStream::connect(conn_socket(template))?,
    };
    send(&mut stream, &Message::Connect { buffered, profile }, &[socket.as_raw_fd(), write_end.as_raw_fd()])?;
    drop(write_end);
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::Builder::new().name("lp-fork-wait".into()).spawn(move || {
        // Kept open until the connection ends: closing it now lets the next
        // connection reuse its descriptor number while this one's task is
        // still winding down in the server.
        let _socket = socket;
        let mut read_end = std::fs::File::from(read_end);
        let mut byte = [0u8; 1];
        // EOF once the connection's process (the only holder of the write end) has exited.
        while matches!(read_end.read(&mut byte), Ok(n) if n > 0) {}
        let _ = tx.send(());
    })?;
    Ok(rx)
}

/// Where a template (or one of its spares) accepts connections, beside its
/// control socket.
fn conn_socket(control: &Path) -> PathBuf {
    PathBuf::from(format!("{}.conn", control.display()))
}

// ---- Template side -------------------------------------------------------

/// What the template boots and forks: the protocol state of a connection
/// that has no socket yet, with one prepared page per profile.
struct Snapshot {
    browser: Browser<EngineHost>,
    outbox: Outbox,
    outbound: OutboundRx,
    /// Indexed by profile id.
    profiles: Vec<ProfileState>,
    /// Bumped whenever the pages change (a profile booted or set up): spares
    /// forked before are out of date.
    generation: u64,
}

struct ProfileState {
    /// When its page started booting.
    started: Instant,
    /// Settled (one full collection) since it loaded or ran its setup.
    settled: bool,
    /// Its setup steps ran (or it has none).
    set_up: bool,
    /// A derived profile whose page is still labelled with its base's id:
    /// its setup turns that page into its own.
    derive_from: Option<usize>,
}

impl ProfileState {
    fn new(profile: &PreparedProfile) -> Self {
        ProfileState { started: Instant::now(), settled: false, set_up: profile.setup.is_empty(), derive_from: None }
    }
}

type Waiting = VecDeque<(Vec<u8>, Vec<OwnedFd>, Instant)>;

/// A connection for another template: its socket, bytes, profile, fds and
/// when it arrived.
type Forward = (PathBuf, Vec<u8>, Option<usize>, Vec<OwnedFd>, Instant);

/// A snapshot to fork off as a template of its own.
#[derive(Clone)]
struct Derive {
    base: usize,
    id: usize,
    profile: PreparedProfile,
    socket: PathBuf,
}

/// A profile shared under a name (root only).
struct Named {
    id: usize,
    /// Leases holding it. None while profiles derived from it keep it.
    holders: HashSet<u64>,
}

/// One template process: the root (spawned by the server) or a derived one
/// (forked from the template holding its base profile).
struct TemplateProcess {
    snapshot: Snapshot,
    /// The root's socket: registrations go there.
    root: PathBuf,
    /// This template's control socket (registrations, derives).
    socket: PathBuf,
    listener: UnixListener,
    /// Connections for this template's snapshot: accepted by its spares, or
    /// by the template itself when none is waiting.
    conns: UnixListener,
    watch: std::rc::Rc<lp_runtime::FdWatch>,
    /// Connections to fork from here.
    waiting: Waiting,
    /// Root only: connections for derived snapshots, until their template listens.
    forwards: VecDeque<Forward>,
    /// Templates to fork off once the pages are ready.
    derives: VecDeque<Derive>,
    /// Root only: where each profile's snapshot is served (`None`: here).
    holders: Vec<Option<PathBuf>>,
    derived_pids: HashMap<usize, libc::pid_t>,
    /// Retired derived templates told to exit, not reaped yet, with when
    /// they get SIGKILL instead. Nothing waits for them: a large process can
    /// take a while to exit, and this loop serves every other profile.
    terminating: Vec<(libc::pid_t, Instant, bool)>,
    is_root: bool,
    /// Root only: profiles shared by name, retired with their last holder.
    names: HashMap<String, Named>,
    /// Root only: profiles registered without a name (and `--prepare`'s),
    /// kept until unregistered by id.
    pinned: HashSet<usize>,
    /// Root only: open leases, the streams that connections' processes keep
    /// open while they live.
    leases: HashMap<u64, UnixStream>,
    next_lease: u64,
    /// Root only: derives for a base whose template is not listening yet
    /// (it is forked once its own base is ready), with where and since when.
    pending_derives: VecDeque<(PathBuf, Derive, Instant)>,
    /// Forked children waiting for a connection (accepting on `conns`), as
    /// our end of a pair: the spare closes its end once it has taken one,
    /// and we close ours to drop it. Forked at `spares_generation`.
    spares: VecDeque<UnixStream>,
    spares_generation: u64,
}

/// Entry point of the template process (`serve --template <socket>`).
pub fn template_main(options: &ServeOptions, socket: &Path) -> ExitCode {
    match run_template(options, socket) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("lightningpanda serve --fork: template: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run_template(options: &ServeOptions, socket: &Path) -> Result<(), String> {
    // A fork needs every other thread gone: a page's workers wait for the child.
    lp_webapi::hold_for_fork();
    // Before profile preload, VM and network threads, and every template fork.
    lp_js::prepare_process_for_fork();
    crate::fork_stats::init();
    let profile = crate::serve::browser_profile(options)?;
    if !supported() {
        return Err("this JavaScriptCore cannot fork (build the patched one: scripts/jsc/build.sh)".into());
    }
    // The static-file preload runs on a thread of its own: let it finish.
    let _ = profile.net.static_files.preloaded();
    #[cfg(feature = "render")]
    {
        crate::engine::set_fork_template(true);
        // Fonts without CoreText, loaded here before any fork (children
        // inherit them): see lp_render::use_fork_safe_fonts.
        lp_render::use_fork_safe_fonts();
    }

    let mut thread = EngineHost::prepare_thread_with(&profile).map_err(|e| e.message)?;
    precompile_static_modules(&thread.browser.engine, &profile.net.static_files);
    let profiles: Vec<PreparedProfile> = profile.prepared.iter().map(one_page).collect();
    let pages = EngineHost::preboot(&mut thread, &profiles);
    let (outbox, outbound) = Outbox::channel(DEFAULT_OUTBOUND_BUDGET);
    let mut host = EngineHost::with_thread(thread, outbox.clone(), EngineHost::body_budget()).map_err(|e| e.message)?;
    host.adopt_prebooted(pages);
    let states = profiles.iter().map(ProfileState::new).collect();
    let holders = vec![None; profiles.len()];
    // `--prepare`'s profiles stay for the server's lifetime.
    let pinned = (0..profiles.len()).collect();
    let browser = Browser::with_prepared(host, profiles);
    let snapshot = Snapshot { browser, outbox, outbound, profiles: states, generation: 0 };

    // The connection socket first: the server takes the control socket as the sign that both listen.
    let conns = bind_nonblocking(&conn_socket(socket))?;
    let listener = bind_nonblocking(socket)?;
    let watch = lp_runtime::FdWatch::new(listener.as_raw_fd());
    let mut t = TemplateProcess {
        snapshot,
        root: socket.to_path_buf(),
        socket: socket.to_path_buf(),
        listener,
        conns,
        watch,
        waiting: VecDeque::new(),
        forwards: VecDeque::new(),
        derives: VecDeque::new(),
        holders,
        derived_pids: HashMap::new(),
        terminating: Vec::new(),
        is_root: true,
        names: HashMap::new(),
        pinned,
        leases: HashMap::new(),
        next_lease: 1,
        pending_derives: VecDeque::new(),
        spares: VecDeque::new(),
        spares_generation: 0,
    };
    t.watch_listener();

    install_child_notifications();
    'serve: loop {
        t.reap_children();
        t.accept_all();
        t.end_leases();
        // Spares that took a connection have closed their end.
        t.spares.retain(|spare| !readable(spare.as_raw_fd()));
        if t.spares.is_empty() {
            t.accept_conns();
        }
        t.forward_waiting();
        t.deliver_pending_derives();
        let ready = ready_to_fork(&mut t.snapshot, t.waiting.front().map(|w| w.2));
        if ready {
            while let Some(derive) = t.derives.pop_front() {
                if t.fork_template(derive) {
                    continue 'serve; // this process is now the derived template
                }
            }
            while let Some((buffered, fds, _)) = t.waiting.pop_front() {
                if let Some(child) = t.fork_connection(&options.server, buffered, fds) {
                    // Only the forked child gets here, and it is done.
                    exit_child(child);
                }
            }
            if let Some(child) = t.refill_spares(&options.server) {
                exit_child(child);
            }
        }
        // Boot what is booting; wake for the next message.
        let busy = !ready || !t.forwards.is_empty() || !t.pending_derives.is_empty() || !t.terminating.is_empty();
        let deadline = Instant::now() + Duration::from_millis(if busy { 20 } else { 1000 });
        t.watch_listener();
        let watch = t.watch.clone();
        let outcome = t.snapshot.browser.host_mut().run_until_work(Some(deadline), || watch.take_ready());
        t.snapshot.browser.host_mut().flush();
        t.snapshot.drain();
        if matches!(outcome, lp_runtime::RunOutcome::Idle) {
            let mut fds = vec![t.listener.as_raw_fd(), t.conns.as_raw_fd(), 0];
            fds.extend(t.spares.iter().map(|s| s.as_raw_fd()));
            fds.extend(t.leases.values().map(|s| s.as_raw_fd()));
            poll_readable(&fds, if busy { 20 } else { -1 });
        }
        if server_gone() {
            let _ = std::fs::remove_file(&t.socket);
            let _ = std::fs::remove_file(conn_socket(&t.socket));
            return Ok(());
        }
    }
}

impl TemplateProcess {
    /// Wakes the loop for control messages, and for connections while no
    /// spare is there to take them.
    fn watch_listener(&mut self) {
        let fd = if self.spares.is_empty() { self.conns.as_raw_fd() } else { self.listener.as_raw_fd() };
        self.watch = lp_runtime::FdWatch::new(fd);
        self.watch.set_interest(true, false);
        self.snapshot.browser.host_mut().thread.event_loop.watch_fd(Some(self.watch.clone()));
    }

    /// Reads every pending message: registrations are handled now,
    /// connections and derived templates wait until the pages are ready.
    fn accept_all(&mut self) {
        while let Ok((mut stream, _)) = self.listener.accept() {
            let _ = stream.set_nonblocking(false);
            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
            let Ok((message, fds)) = recv(&mut stream) else { continue };
            match message {
                Message::Connect { buffered, profile } if fds.len() == 2 => {
                    self.take_connection(buffered, profile, fds)
                }
                Message::Connect { .. } => {}
                Message::Register { profile, name, lease } => {
                    let reply = match self.register(profile, name, lease) {
                        Ok(id) => Registered { profile_id: Some(id), error: None },
                        Err(e) => Registered { profile_id: None, error: Some(e) },
                    };
                    let _ = send(&mut stream, &reply, &[]);
                }
                Message::Adopt { name, lease } => {
                    let reply = match self.adopt(&name, lease) {
                        Ok((id, profile)) => Adopted { profile_id: Some(id), profile: Some(profile), error: None },
                        Err(e) => Adopted { profile_id: None, profile: None, error: Some(e) },
                    };
                    let _ = send(&mut stream, &reply, &[]);
                }
                Message::Release { name, lease } => {
                    let reply = match self.release(&name, lease) {
                        Ok(release) => Released { release: Some(release), error: None },
                        Err(e) => Released { release: None, error: Some(e) },
                    };
                    let _ = send(&mut stream, &reply, &[]);
                }
                Message::Lease if self.is_root => {
                    let lease = self.next_lease;
                    self.next_lease += 1;
                    if send(&mut stream, &Leased { lease }, &[]).is_ok() && stream.set_nonblocking(true).is_ok() {
                        self.leases.insert(lease, stream);
                    }
                }
                Message::Lease => {}
                Message::Profiles => {
                    let _ = send(&mut stream, &Listed { profiles: self.named_profiles() }, &[]);
                }
                Message::Unregister { profile_id } | Message::RetireDerived { profile_id } => {
                    let result = if matches!(message, Message::Unregister { .. }) {
                        self.unregister(profile_id)
                    } else {
                        self.retire_derived(profile_id)
                    };
                    let reply = match result {
                        Ok(removed) => Unregistered { removed, error: None },
                        Err(error) => Unregistered { removed: false, error: Some(error) },
                    };
                    let _ = send(&mut stream, &reply, &[]);
                }
                Message::Taken => {}
                Message::Derive { base, id, profile, socket } => {
                    self.derives.push_back(Derive { base, id, profile, socket });
                    let _ = send(&mut stream, &Registered { profile_id: Some(id), error: None }, &[]);
                }
            }
        }
    }

    /// Takes the connections no spare took.
    fn accept_conns(&mut self) {
        while let Ok((mut stream, _)) = self.conns.accept() {
            let _ = stream.set_nonblocking(false);
            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
            if let Ok((Message::Connect { buffered, profile }, fds)) = recv(&mut stream) {
                if fds.len() == 2 {
                    self.take_connection(buffered, profile, fds);
                }
            }
        }
    }

    /// Forks for a connection here, or passes it to the template holding
    /// the snapshot it asked for.
    fn take_connection(&mut self, buffered: Vec<u8>, profile: Option<usize>, fds: Vec<OwnedFd>) {
        if profile.is_some_and(|id| !self.snapshot.browser.prepared_profile_active(id)) {
            return;
        }
        match profile.and_then(|id| self.holders.get(id).cloned().flatten()) {
            Some(path) => self.forwards.push_back((path, buffered, profile, fds, Instant::now())),
            None => self.waiting.push_back((buffered, fds, Instant::now())),
        }
    }

    /// Passes connections on to the templates of derived snapshots (they
    /// start listening once forked, so a connection may wait a little).
    fn forward_waiting(&mut self) {
        for _ in 0..self.forwards.len() {
            let Some((path, buffered, profile, fds, since)) = self.forwards.pop_front() else { break };
            if profile.is_some_and(|id| !self.snapshot.browser.prepared_profile_active(id)) {
                continue; // retired meanwhile: dropping it closes the connection
            }
            let raw: Vec<RawFd> = fds.iter().map(|f| f.as_raw_fd()).collect();
            let sent = UnixStream::connect(conn_socket(&path))
                .and_then(|mut s| send(&mut s, &Message::Connect { buffered: buffered.clone(), profile }, &raw));
            if sent.is_err() && since.elapsed() < DERIVE_WAIT {
                self.forwards.push_back((path, buffered, profile, fds, since));
            }
        }
    }

    /// Passes on derives whose base's template has started listening.
    fn deliver_pending_derives(&mut self) {
        for _ in 0..self.pending_derives.len() {
            let Some((holder, derive, since)) = self.pending_derives.pop_front() else { break };
            if !self.snapshot.browser.prepared_profile_active(derive.id) {
                continue; // retired meanwhile
            }
            if deliver_derive(&holder, &derive) {
                continue;
            }
            if since.elapsed() < DERIVE_WAIT {
                self.pending_derives.push_back((holder, derive, since));
            } else {
                eprintln!(
                    "lightningpanda: template: profile {}: the template of its base {} did not start",
                    derive.id, derive.base
                );
            }
        }
    }

    /// Adds a profile, or finds the same one. With a name, `lease` holds it
    /// under that name, which every connection shares: registering the same
    /// definition again (from any client) finds the profile booted.
    fn register(
        &mut self,
        profile: PreparedProfile,
        name: Option<String>,
        lease: Option<u64>,
    ) -> Result<usize, String> {
        if !self.is_root {
            return Err("registrations go to the root template".into());
        }
        let profile = one_page(&profile);
        let Some(name) = name else {
            let id = self.boot(profile)?;
            self.pinned.insert(id);
            return Ok(id);
        };
        let lease = self.live_lease(lease)?;
        if let Some(named) = self.names.get_mut(&name) {
            if self.snapshot.browser.prepared_profiles()[named.id] == profile {
                named.holders.insert(lease);
                return Ok(named.id);
            }
            if !named.holders.is_empty() {
                return Err(format!("profile {name} is registered with a different definition (release it first)"));
            }
            // Nobody holds the name (profiles derived from it keep the old
            // profile): it moves to the new definition.
            let old = named.id;
            self.names.remove(&name);
            self.retire_unused(old);
        }
        let id = self.boot(profile)?;
        self.names.insert(name, Named { id, holders: HashSet::from([lease]) });
        Ok(id)
    }

    /// The active profile with this definition, else a new one: a plain one
    /// boots here, a derived one is forked off the template holding its base.
    fn boot(&mut self, profile: PreparedProfile) -> Result<usize, String> {
        let browser = &self.snapshot.browser;
        if let Some(id) = browser
            .prepared_profiles()
            .iter()
            .enumerate()
            .find_map(|(id, p)| (browser.prepared_profile_active(id) && *p == profile).then_some(id))
        {
            return Ok(id);
        }
        if let Some(base) = profile.base {
            if !self.snapshot.browser.prepared_profile_active(base) {
                return Err(format!("no profile {base} to start from"));
            }
        }
        self.snapshot.profiles.push(ProfileState::new(&profile));
        if profile.base.is_none() {
            // Before the reply: a spare forked earlier must not take a
            // connection that wants this profile. A derived profile's page is
            // not here: spares pass on connections for ids they do not know.
            self.snapshot.generation += 1;
            self.spares.clear();
        }
        let id = self.snapshot.browser.add_prepared_profile(profile.clone());
        self.holders.resize(id + 1, None);
        let Some(base) = profile.base else {
            if !self.snapshot.browser.prepared_pages().iter().any(|(p, _)| *p == id) {
                let _ = self.snapshot.browser.unregister_prepared_profile(id);
                return Err(format!("cannot boot a page at {}", profile.url));
            }
            return Ok(id);
        };
        let socket = PathBuf::from(format!("{}.{id}", self.root.display()));
        let _ = std::fs::remove_file(&socket);
        self.holders[id] = Some(socket.clone());
        let derive = Derive { base, id, profile, socket };
        match self.holders[base].clone() {
            None => self.derives.push_back(derive),
            // That template may not be listening yet: it is forked once its
            // own base is ready, and only then can it take derives.
            Some(holder) => {
                if !deliver_derive(&holder, &derive) {
                    self.pending_derives.push_back((holder, derive, Instant::now()));
                }
            }
        }
        Ok(id)
    }

    /// `lease` if it is open (its connection has not ended).
    fn live_lease(&self, lease: Option<u64>) -> Result<u64, String> {
        lease.filter(|l| self.leases.contains_key(l)).ok_or_else(|| "the connection's lease has ended".to_owned())
    }

    /// Holds the profile registered as `name`: its id and definition.
    fn adopt(&mut self, name: &str, lease: u64) -> Result<(usize, PreparedProfile), String> {
        if !self.is_root {
            return Err("adoptions go to the root template".into());
        }
        let lease = self.live_lease(Some(lease))?;
        let named = self.names.get_mut(name).ok_or_else(|| format!("no profile {name}"))?;
        named.holders.insert(lease);
        Ok((named.id, self.snapshot.browser.prepared_profiles()[named.id].clone()))
    }

    /// Drops `lease`'s hold on `name`; the profile is retired once nothing uses it.
    fn release(&mut self, name: &str, lease: u64) -> Result<ProfileRelease, String> {
        if !self.is_root {
            return Err("releases go to the root template".into());
        }
        let Some(named) = self.names.get_mut(name) else { return Ok(ProfileRelease::default()) };
        if !named.holders.remove(&lease) {
            return Ok(ProfileRelease::default());
        }
        let id = named.id;
        Ok(ProfileRelease { released: true, retired: self.retire_unused(id) })
    }

    /// Ends the holds of connections whose process has exited: their lease
    /// stream reads EOF (however the process ended).
    fn end_leases(&mut self) {
        let ended: Vec<u64> = self.leases.iter().filter(|(_, s)| lease_ended(s)).map(|(l, _)| *l).collect();
        for lease in ended {
            self.leases.remove(&lease);
            let held: Vec<String> =
                self.names.iter().filter(|(_, n)| n.holders.contains(&lease)).map(|(name, _)| name.clone()).collect();
            for name in held {
                let _ = self.release(&name, lease);
            }
        }
    }

    /// Retires profile `id` once nothing uses it: no connection holds it by
    /// name, it was not registered without one, and no profile derives from
    /// it. Its base is checked next. Whether it was retired.
    fn retire_unused(&mut self, id: usize) -> bool {
        let browser = &self.snapshot.browser;
        if !browser.prepared_profile_active(id) || self.pinned.contains(&id) {
            return false;
        }
        if self.names.values().any(|n| n.id == id && !n.holders.is_empty()) {
            return false;
        }
        if !matches!(browser.check_profile_retirement(id), Ok(true)) {
            return false; // a profile still derives from it
        }
        let base = browser.prepared_profiles()[id].base;
        match self.retire(id) {
            Ok(true) => {
                self.names.retain(|_, n| n.id != id);
                if let Some(base) = base {
                    self.retire_unused(base);
                }
                true
            }
            Ok(false) => false,
            Err(e) => {
                eprintln!("lightningpanda: template: retiring profile {id}: {e}");
                false
            }
        }
    }

    /// The named profiles, by name.
    fn named_profiles(&self) -> Vec<NamedProfile> {
        let profiles = self.snapshot.browser.prepared_profiles();
        let mut list: Vec<NamedProfile> = self
            .names
            .iter()
            .map(|(name, n)| NamedProfile {
                name: name.clone(),
                profile_id: n.id,
                holders: n.holders.len(),
                url: profiles[n.id].url.clone(),
                base: profiles[n.id].base,
            })
            .collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        list
    }

    fn reap_children(&mut self) {
        loop {
            let mut status = 0;
            let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
            let pid = unsafe { libc::wait4(-1, &mut status, libc::WNOHANG, &mut usage) };
            if pid < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            if pid <= 0 {
                break;
            }
            crate::fork_stats::reaped(pid, &usage);
            self.derived_pids.retain(|_, child| *child != pid);
            self.terminating.retain(|(child, _, _)| *child != pid);
        }
        let now = Instant::now();
        for (pid, deadline, killed) in &mut self.terminating {
            if !*killed && now >= *deadline {
                // SAFETY: our child, not reaped yet (reaping removes it from this list).
                unsafe { libc::kill(*pid, libc::SIGKILL) };
                *killed = true;
            }
        }
    }

    fn retire_derived(&mut self, id: usize) -> Result<bool, String> {
        let before = self.derives.len();
        self.derives.retain(|derive| derive.id != id);
        let mut removed = self.derives.len() != before;
        if let Some(pid) = self.derived_pids.remove(&id) {
            // SAFETY: our child, not reaped yet (reaping removes it from derived_pids).
            unsafe { libc::kill(pid, libc::SIGTERM) };
            self.terminating.push((pid, Instant::now() + Duration::from_millis(500), false));
            removed = true;
        }
        let path = PathBuf::from(format!("{}.{id}", self.root.display()));
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(conn_socket(&path));
        Ok(removed)
    }

    /// Retires profile `id` by id (`unregisterPreparedPage {profileId}`): a
    /// profile some connection still holds by name stays until it is released.
    fn unregister(&mut self, id: usize) -> Result<bool, String> {
        if !self.is_root {
            return Err("profile retirement goes to the root template".into());
        }
        self.pinned.remove(&id);
        if self.names.values().any(|n| n.id == id) {
            return Ok(self.retire_unused(id));
        }
        let base = self.snapshot.browser.prepared_profiles().get(id).and_then(|p| p.base);
        let retired = self.retire(id)?;
        if let (true, Some(base)) = (retired, base) {
            self.retire_unused(base);
        }
        Ok(retired)
    }

    /// Retires profile `id` (its derived template too) unless profiles derive from it.
    fn retire(&mut self, id: usize) -> Result<bool, String> {
        if !self.snapshot.browser.check_profile_retirement(id).map_err(|e| e.message)? {
            return Ok(false);
        }
        let derived = self.snapshot.browser.prepared_profiles()[id].base;
        if let Some(base) = derived {
            let pending = self.pending_derives.len();
            self.pending_derives.retain(|(_, derive, _)| derive.id != id);
            if self.pending_derives.len() == pending {
                match self.holders.get(base).cloned().flatten() {
                    // Not waiting for that template: it may be running a setup.
                    Some(holder) => notify(&holder, &Message::RetireDerived { profile_id: id }),
                    None => {
                        self.retire_derived(id)?;
                    }
                }
            }
        }
        self.snapshot.browser.unregister_prepared_profile(id).map_err(|e| e.message)?;
        self.holders[id] = None;
        if derived.is_none() {
            // Spares still have its page. (They pass connections for a derived
            // profile on to this template, which refuses a retired one.)
            self.snapshot.generation += 1;
            self.spares.clear();
        }
        Ok(true)
    }

    /// Forks a derived template: the child continues as the template of
    /// `derive.id`, listening on its own socket. Whether this process is now
    /// that child.
    fn fork_template(&mut self, derive: Derive) -> bool {
        let (conns, listener) = match bind_nonblocking(&conn_socket(&derive.socket))
            .and_then(|c| bind_nonblocking(&derive.socket).map(|l| (c, l)))
        {
            Ok(both) => both,
            Err(e) => {
                eprintln!("lightningpanda: template: {e}");
                return false;
            }
        };
        match fork_process(&self.snapshot, "template") {
            Forked::Parent(pid) => {
                self.derived_pids.insert(derive.id, pid);
                false
            }
            Forked::Failed => false,
            Forked::Child => {
                self.listener = listener; // closes this child's copies of the parent's
                self.conns = conns;
                self.socket = derive.socket;
                self.is_root = false;
                self.holders.clear();
                self.derived_pids.clear();
                self.terminating.clear();
                self.waiting.clear();
                self.forwards.clear();
                self.derives.clear();
                self.spares.clear(); // the parent's: closing our copies lets them see the parent go
                self.names.clear();
                self.pinned.clear();
                self.leases.clear(); // the root's ends: only the root watches them
                self.pending_derives.clear();
                self.watch_listener();
                let profiles = &mut self.snapshot.profiles;
                while profiles.len() <= derive.id {
                    profiles.push(ProfileState { set_up: true, ..ProfileState::new(&derive.profile) });
                }
                profiles[derive.id] = ProfileState {
                    started: Instant::now(),
                    settled: false,
                    set_up: false,
                    derive_from: Some(derive.base),
                };
                self.snapshot.browser.set_prepared_profile(derive.id, derive.profile);
                true
            }
        }
    }

    /// Forks a process for one connection. Returns in the parent with
    /// `None`; in the child, serves the connection and returns its exit code.
    fn fork_connection(&mut self, config: &ServerConfig, buffered: Vec<u8>, fds: Vec<OwnedFd>) -> Option<i32> {
        match fork_process(&self.snapshot, "conn") {
            Forked::Parent(_) | Forked::Failed => None, // the parent's copies of the fds close here
            Forked::Child => {
                self.leave_template();
                unsafe { libc::close(self.conns.as_raw_fd()) };
                crate::fork_stats::mark("taken");
                Some(self.serve_child(config, buffered, fds, "conn", None))
            }
        }
    }

    /// Forks spares up to the wanted count. Returns in the parent with
    /// `None`; in a spare, once it has served its connection, its exit code.
    fn refill_spares(&mut self, config: &ServerConfig) -> Option<i32> {
        if self.spares_generation != self.snapshot.generation {
            self.spares.clear(); // their pages are out of date: they exit
            self.spares_generation = self.snapshot.generation;
        }
        while self.spares.len() < spares_wanted() {
            let Ok((ours, theirs)) = UnixStream::pair() else { return None };
            match fork_process(&self.snapshot, "spare") {
                Forked::Failed => return None,
                Forked::Parent(_) => self.spares.push_back(ours),
                Forked::Child => {
                    drop(ours);
                    self.leave_template();
                    return Some(self.wait_as_spare(config, theirs));
                }
            }
        }
        None
    }

    /// A spare takes the next connection from its template's connection
    /// socket itself, so that a test never waits on the template (which may
    /// be forking). Meanwhile its pages keep running (timers, polling), as
    /// the template's do. EOF from the template means it dropped this spare
    /// (its pages are out of date) or died.
    fn wait_as_spare(&mut self, config: &ServerConfig, to_template: UnixStream) -> i32 {
        let watch = lp_runtime::FdWatch::new(self.conns.as_raw_fd());
        watch.set_interest(true, false);
        self.snapshot.browser.host_mut().thread.event_loop.watch_fd(Some(watch.clone()));
        loop {
            let deadline = Instant::now() + Duration::from_millis(250);
            let outcome = self.snapshot.browser.host_mut().run_until_work(Some(deadline), || watch.take_ready());
            self.snapshot.browser.host_mut().flush();
            self.snapshot.drain();
            if matches!(outcome, lp_runtime::RunOutcome::Idle) {
                poll_readable(&[self.conns.as_raw_fd(), to_template.as_raw_fd()], 250);
            }
            if readable(to_template.as_raw_fd()) {
                crate::fork_stats::child_done("spare-unused", None);
                return 0;
            }
            let Ok((mut stream, _)) = self.conns.accept() else { continue };
            let _ = stream.set_nonblocking(false);
            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
            let Ok((Message::Connect { buffered, profile }, fds)) = recv(&mut stream) else { continue };
            if fds.len() != 2 {
                continue;
            }
            // This spare cannot serve the connection when it is out of date (the
            // template dropped it for a newer profile while it was accepting;
            // its pages miss that profile) or when the profile is a derived
            // snapshot that was not up when the server looked. The template
            // gets it instead, and queues it until that snapshot's template
            // listens if need be.
            let stale = readable(to_template.as_raw_fd());
            let unknown = profile.is_some_and(|id| !self.snapshot.browser.prepared_profile_active(id));
            if stale || unknown || profile.is_some_and(|id| self.holders.get(id).is_some_and(|h| h.is_some())) {
                let raw: Vec<RawFd> = fds.iter().map(|f| f.as_raw_fd()).collect();
                if let Ok(mut s) = UnixStream::connect(&self.socket) {
                    let _ = send(&mut s, &Message::Connect { buffered, profile }, &raw);
                }
                if stale {
                    crate::fork_stats::child_done("spare-unused", None);
                    return 0;
                }
                continue;
            }
            drop(to_template); // this spare is gone from the template's list
            if let Ok(mut s) = UnixStream::connect(&self.socket) {
                let _ = send(&mut s, &Message::Taken, &[]); // and it wakes to fork a replacement
            }
            self.snapshot.browser.host_mut().thread.event_loop.watch_fd(None);
            unsafe { libc::close(self.conns.as_raw_fd()) };
            crate::fork_stats::mark("taken");
            return self.serve_child(config, buffered, fds, "spare", profile);
        }
    }

    /// In a forked child, right after the fork: lets go of everything that
    /// belongs to the template. Other connections' sockets and pipes, and the
    /// other spares, would stay open while this process holds them.
    fn leave_template(&mut self) {
        #[cfg(feature = "render")]
        crate::engine::set_fork_template(false);
        self.snapshot.browser.host_mut().thread.event_loop.watch_fd(None);
        self.waiting.clear();
        self.forwards.clear();
        self.spares.clear();
        self.derives.clear();
        self.leases.clear();
        self.pending_derives.clear();
        self.terminating.clear();
        // `holders` stays: a spare of the root passes on connections for derived snapshots.
        unsafe { libc::close(self.listener.as_raw_fd()) };
    }

    /// In a forked child (after `leave_template`): serves one connection with
    /// the snapshot; the exit code.
    fn serve_child(
        &mut self,
        config: &ServerConfig,
        buffered: Vec<u8>,
        mut fds: Vec<OwnedFd>,
        kind: &str,
        profile: Option<usize>,
    ) -> i32 {
        let finished = fds.pop().expect("two fds");
        let socket = std::net::TcpStream::from(fds.pop().expect("two fds"));
        let root = self.root.clone();
        // The child owns the template's protocol state from here; the
        // template's copy is never touched again in this process.
        let (mut browser, outbox, outbound) = unsafe {
            (
                std::ptr::read(&self.snapshot.browser),
                std::ptr::read(&self.snapshot.outbox),
                std::ptr::read(&self.snapshot.outbound),
            )
        };
        browser.set_fork_child(Box::new(RootRegistry { root, lease: RefCell::new(None) }));
        lp_webapi::release_after_fork();
        let after = if fast_exit() { crate::serve::AfterServe::Exit } else { crate::serve::AfterServe::TearDown };
        crate::serve::serve_browser(browser, outbox, outbound, socket, &buffered, config.clone(), after);
        crate::fork_stats::mark("dropped");
        crate::fork_stats::child_done(kind, profile);
        drop(finished);
        0
    }
}

enum Forked {
    Parent(libc::pid_t),
    Child,
    Failed,
}

/// `fork()` with JavaScriptCore and the network told before and after.
/// `kind` names the child for `LIGHTNINGPANDA_FORK_STATS`.
fn fork_process(snapshot: &Snapshot, kind: &str) -> Forked {
    let engine = &snapshot.browser.host().thread.browser.engine;
    lp_net::client::suspend_for_fork();
    if let Err(e) = engine.prepare_for_fork() {
        eprintln!("lightningpanda: template: {e}");
        let _ = lp_net::client::resume_after_fork();
        return Forked::Failed;
    }
    // JSC's VMTraps queue thread (it fires watchdog timers) is the one expected
    // survivor: JSPrepareForFork holds it where it has no lock (patch 0026),
    // and JSDidFork starts a new one in the child.
    let others: Vec<String> = other_threads().into_iter().filter(|n| n != "JSC VMTraps Signal Sender").collect();
    if !others.is_empty() {
        eprintln!("lightningpanda: template: threads alive at fork: {others:?}");
    }
    // SAFETY: only this thread is alive (checked above, best effort), and
    // each process continues on it after JSC and the network are told.
    let forking = Instant::now();
    let pid = unsafe { libc::fork() };
    let fork_us = forking.elapsed().as_micros() as u64;
    if pid == 0 {
        crate::fork_stats::child_started();
        // SAFETY: the network is stopped and this is the sole surviving thread.
        // Keep inherited handles, but isolate their pipe from sibling readers.
        if let Err(e) = unsafe { snapshot.browser.host().thread.event_loop.reset_after_fork() } {
            eprintln!("lightningpanda: child wake pipe reset failed: {e}");
            std::process::exit(1);
        }
    }
    engine.did_fork(pid == 0);
    if pid == 0 && crate::fork_stats::init() {
        crate::fork_stats::child_modules(engine.module_precompile_stats());
    }
    install_child_notifications();
    if let Err(e) = lp_net::client::resume_after_fork() {
        eprintln!("lightningpanda: network restart failed: {e}");
        if pid == 0 {
            std::process::exit(1);
        }
    }
    if pid > 0 {
        crate::fork_stats::forked(kind, pid, fork_us);
    }
    match pid {
        0 => Forked::Child,
        p if p < 0 => {
            eprintln!("lightningpanda: template: fork failed: {}", std::io::Error::last_os_error());
            Forked::Failed
        }
        p => Forked::Parent(p),
    }
}

impl Snapshot {
    /// Drops what the template's protocol state wrote: no client reads it.
    fn drain(&mut self) {
        while let Some(Outbound::Frame(_)) = self.outbound.try_recv() {}
    }

    /// Runs one CDP command on the template's protocol state and waits for
    /// its reply (`LP.run` replies once its steps are done).
    fn command(&mut self, session: Option<&str>, method: &str, params: Value) -> Result<Value, String> {
        static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1 << 40);
        let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut message = serde_json::json!({ "id": id, "method": method, "params": params });
        if let Some(session) = session {
            message["sessionId"] = session.into();
        }
        let mut sink: Vec<OutFrame> = Vec::new();
        self.browser.handle_text(&message.to_string(), &mut sink);
        let deadline = Instant::now() + SETUP_TIMEOUT;
        loop {
            self.browser.host_mut().flush();
            // Synchronous replies come through the sink, later ones through the outbox.
            let mut frames: Vec<Value> = sink.drain(..).filter_map(|f| frame_json(f.finish_text().as_ref())).collect();
            while let Some(Outbound::Frame(f)) = self.outbound.try_recv() {
                frames.extend(frame_json(f.as_ref()));
            }
            if let Some(reply) = frames.into_iter().find(|v| v["id"] == serde_json::json!(id)) {
                return match reply.get("error") {
                    Some(e) => Err(e["message"].as_str().unwrap_or("failed").to_owned()),
                    None => Ok(reply["result"].clone()),
                };
            }
            if Instant::now() > deadline {
                return Err(format!("{method} timed out"));
            }
            self.browser.host_mut().run_until_work(Some(Instant::now() + Duration::from_millis(20)), || false);
        }
    }

    /// Runs profile `id`'s setup steps on the waiting page of profile
    /// `page_of` (itself, or the base of a derived profile), then hands the
    /// page back to the pool, as profile `id`'s, so that forks get it in that
    /// state.
    fn set_up(&mut self, id: usize, page_of: usize) -> Result<(), String> {
        self.generation += 1;
        let steps = self.browser.prepared_profiles()[id].setup.clone();
        let page = self.command(
            None,
            "Lightningpanda.acquirePreparedPage",
            serde_json::json!({ "profileId": page_of, "attach": true }),
        )?;
        let (Some(target), Some(session)) = (page["targetId"].as_str(), page["sessionId"].as_str()) else {
            return Err("acquirePreparedPage: no target".into());
        };
        let (target, session) = (target.to_owned(), session.to_owned());
        let run = self.command(Some(&session), "LP.run", serde_json::json!({ "steps": steps }));
        self.browser.return_to_prepared(&target).map_err(|e| e.message)?;
        if page_of != id {
            self.browser.relabel_prepared(page_of, id);
        }
        self.drain();
        run.map(|_| ())
    }
}

/// The JSON payload of an outgoing (unmasked) WebSocket text frame.
fn frame_json(frame: &[u8]) -> Option<Value> {
    if frame.len() < 2 || frame[0] & 0x0f != 1 {
        return None;
    }
    let start = match frame[1] & 0x7f {
        126 => 4,
        127 => 10,
        _ => 2,
    };
    serde_json::from_slice(frame.get(start..)?).ok()
}

/// The server closed our stdin (see [`Template`]).
fn server_gone() -> bool {
    let mut pfd = libc::pollfd { fd: 0, events: libc::POLLIN, revents: 0 };
    if unsafe { libc::poll(&mut pfd, 1, 0) } <= 0 {
        return false;
    }
    let mut byte = 0u8;
    unsafe { libc::read(0, (&mut byte as *mut u8).cast(), 1) == 0 }
}

/// A profile as the template boots it: one page, which every fork copies.
fn one_page(p: &PreparedProfile) -> PreparedProfile {
    PreparedProfile { pool: 1, refill_after_close: false, ..p.clone() }
}

/// Every page has loaded, run its setup and gone quiet (or waited long
/// enough); pages are settled first, so forks start from their live size.
fn ready_to_fork(snapshot: &mut Snapshot, waiting_since: Option<Instant>) -> bool {
    let waited_enough = waiting_since.is_some_and(|t| t.elapsed() > QUIET_WAIT);
    let mut ready = true;
    for (id, page) in snapshot.browser.prepared_pages() {
        let host = snapshot.browser.host();
        let loaded = host.page_loaded(page);
        let quiet = host.page_work(page).is_none_or(|w| w.quiet(0));
        let waited = snapshot.profiles[id].started.elapsed();
        if !(loaded && (quiet || waited_enough) || waited > LOAD_WAIT) {
            ready = false;
            continue;
        }
        // A derived profile waiting for this page, else this profile's own setup.
        let derived = snapshot.profiles.iter().position(|s| s.derive_from == Some(id) && !s.set_up);
        let pending = derived.or((!snapshot.profiles[id].set_up).then_some(id));
        if let (true, Some(target)) = (loaded, pending) {
            snapshot.profiles[target].set_up = true;
            snapshot.profiles[target].derive_from = None;
            if let Err(e) = snapshot.set_up(target, id) {
                eprintln!("lightningpanda: template: setup of profile {target}: {e}");
            }
            // Whatever the steps started has to go quiet again.
            snapshot.profiles[target].settled = false;
            snapshot.profiles[target].started = Instant::now();
            ready = false;
            continue;
        }
        if loaded && !snapshot.profiles[id].settled {
            snapshot.profiles[id].settled = true;
            snapshot.browser.host().thread.browser.engine.settle();
        }
    }
    ready
}

extern "C" fn child_notification(_: libc::c_int) {}

fn install_child_notifications() {
    // Keep exited children waitable, and interrupt idle polling so templates reap promptly.
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = child_notification as *const () as usize;
        action.sa_flags = libc::SA_NOCLDSTOP;
        libc::sigemptyset(&mut action.sa_mask);
        libc::sigaction(libc::SIGCHLD, &action, std::ptr::null_mut());
    }
}

fn retire_with(template: &Path, message: Message) -> Result<bool, String> {
    let mut stream = UnixStream::connect(template).map_err(|e| format!("template: {e}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).map_err(|e| e.to_string())?;
    stream.set_write_timeout(Some(Duration::from_secs(5))).map_err(|e| e.to_string())?;
    send(&mut stream, &message, &[]).map_err(|e| e.to_string())?;
    let (reply, _): (Unregistered, _) = recv(&mut stream).map_err(|e| e.to_string())?;
    match reply.error {
        Some(error) => Err(error),
        None => Ok(reply.removed),
    }
}

/// Sends `derive` to the template holding its base, without waiting for a
/// reply (that template may be running a setup); false when that template is
/// not listening (yet).
fn deliver_derive(holder: &Path, derive: &Derive) -> bool {
    let Ok(mut stream) = UnixStream::connect(holder) else { return false };
    let message = Message::Derive {
        base: derive.base,
        id: derive.id,
        profile: derive.profile.clone(),
        socket: derive.socket.clone(),
    };
    send(&mut stream, &message, &[]).is_ok()
}

/// Sends `message` to a template without waiting for its reply; a template
/// that is gone has nothing left to do.
fn notify(template: &Path, message: &Message) {
    if let Ok(mut stream) = UnixStream::connect(template) {
        let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
        let _ = send(&mut stream, message, &[]);
    }
}

/// Whether a lease stream reads EOF: the process that opened it has exited.
fn lease_ended(stream: &UnixStream) -> bool {
    if !readable(stream.as_raw_fd()) {
        return false;
    }
    let mut bytes = [0u8; 16];
    match (&*stream).read(&mut bytes) {
        Ok(0) => true,
        Ok(_) => false,
        Err(e) => !matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted),
    }
}

/// A forked connection's way to the root template's profile registry.
struct RootRegistry {
    root: PathBuf,
    /// Opened with the first named profile and kept until this process
    /// exits: the root holds this connection's profiles while it is open.
    lease: RefCell<Option<(u64, UnixStream)>>,
}

impl RootRegistry {
    fn request<R: for<'de> Deserialize<'de>>(&self, message: &Message) -> Result<R, String> {
        let mut stream = UnixStream::connect(&self.root).map_err(|e| format!("template: {e}"))?;
        send(&mut stream, message, &[]).map_err(|e| e.to_string())?;
        recv(&mut stream).map(|(reply, _)| reply).map_err(|e| e.to_string())
    }

    fn lease(&self) -> Result<u64, String> {
        if let Some((lease, _)) = &*self.lease.borrow() {
            return Ok(*lease);
        }
        let mut stream = UnixStream::connect(&self.root).map_err(|e| format!("template: {e}"))?;
        send(&mut stream, &Message::Lease, &[]).map_err(|e| e.to_string())?;
        let (Leased { lease }, _) = recv(&mut stream).map_err(|e| e.to_string())?;
        *self.lease.borrow_mut() = Some((lease, stream));
        Ok(lease)
    }
}

impl ForkProfiles for RootRegistry {
    fn register(&self, profile: &PreparedProfile, name: Option<&str>) -> Result<usize, String> {
        let lease = name.map(|_| self.lease()).transpose()?;
        let message = Message::Register { profile: profile.clone(), name: name.map(str::to_owned), lease };
        let reply: Registered = self.request(&message)?;
        match (reply.profile_id, reply.error) {
            (Some(id), _) => Ok(id),
            (None, e) => Err(e.unwrap_or_else(|| "registration failed".into())),
        }
    }

    fn adopt(&self, name: &str) -> Result<(usize, PreparedProfile), String> {
        let lease = self.lease()?;
        let reply: Adopted = self.request(&Message::Adopt { name: name.to_owned(), lease })?;
        match (reply.profile_id, reply.profile, reply.error) {
            (Some(id), Some(profile), _) => Ok((id, profile)),
            (_, _, e) => Err(e.unwrap_or_else(|| "adoption failed".into())),
        }
    }

    fn release(&self, name: &str) -> Result<ProfileRelease, String> {
        let Some(lease) = self.lease.borrow().as_ref().map(|(lease, _)| *lease) else {
            return Ok(ProfileRelease::default()); // this connection never held a name
        };
        let reply: Released = self.request(&Message::Release { name: name.to_owned(), lease })?;
        match (reply.release, reply.error) {
            (Some(release), _) => Ok(release),
            (None, e) => Err(e.unwrap_or_else(|| "release failed".into())),
        }
    }

    fn unregister(&self, id: usize) -> Result<bool, String> {
        retire_with(&self.root, Message::Unregister { profile_id: id })
    }

    fn list(&self) -> Result<Vec<NamedProfile>, String> {
        let reply: Listed = self.request(&Message::Profiles)?;
        Ok(reply.profiles)
    }
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn mach_port_deallocate(task: libc::mach_port_t, name: libc::mach_port_t) -> libc::kern_return_t;
}

/// Names of this process's threads other than the calling one.
#[cfg(target_os = "linux")]
fn other_threads() -> Vec<String> {
    // SAFETY: gettid has no preconditions.
    let me = unsafe { libc::gettid() }.to_string();
    let Ok(tasks) = std::fs::read_dir("/proc/self/task") else { return Vec::new() };
    tasks
        .flatten()
        .filter(|t| t.file_name().to_str() != Some(me.as_str()))
        // A thread that exited while we listed is past its last code: harmless.
        .filter_map(|t| std::fs::read_to_string(t.path().join("comm")).ok())
        .map(|name| name.trim_end_matches('\n').to_owned())
        .collect()
}

/// Names of this process's threads other than the calling one.
#[cfg(target_os = "macos")]
fn other_threads() -> Vec<String> {
    let mut names = Vec::new();
    unsafe {
        let mut list: libc::thread_act_array_t = std::ptr::null_mut();
        let mut count: libc::mach_msg_type_number_t = 0;
        #[allow(deprecated)]
        let task = libc::mach_task_self();
        if libc::task_threads(task, &mut list, &mut count) != 0 {
            return names;
        }
        let me = libc::pthread_mach_thread_np(libc::pthread_self());
        for i in 0..count as usize {
            let thread = *list.add(i);
            if thread != me {
                let p = libc::pthread_from_mach_thread_np(thread);
                let mut buf = [0 as libc::c_char; 64];
                if p != 0 {
                    libc::pthread_getname_np(p, buf.as_mut_ptr(), buf.len());
                }
                // A thread with no pthread left is past its last code: harmless.
                if p != 0 {
                    names.push(std::ffi::CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned());
                }
            }
            mach_port_deallocate(task, thread);
        }
        libc::vm_deallocate(
            task,
            list as libc::vm_address_t,
            count as usize * std::mem::size_of::<libc::thread_act_t>(),
        );
    }
    names
}

/// Whether `fd` has data (or EOF) to read right now.
fn readable(fd: RawFd) -> bool {
    let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
    unsafe { libc::poll(&mut pfd, 1, 0) > 0 }
}

fn bind_nonblocking(path: &Path) -> Result<UnixListener, String> {
    let _ = std::fs::remove_file(path);
    let listener = UnixListener::bind(path).map_err(|e| format!("{}: {e}", path.display()))?;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    Ok(listener)
}

fn poll_readable(fds: &[RawFd], timeout_ms: i32) {
    let mut pfds: Vec<libc::pollfd> =
        fds.iter().map(|&fd| libc::pollfd { fd, events: libc::POLLIN, revents: 0 }).collect();
    unsafe { libc::poll(pfds.as_mut_ptr(), pfds.len() as libc::nfds_t, timeout_ms) };
}

fn pipe() -> std::io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let (r, w) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
    for fd in [&r, &w] {
        unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) };
    }
    Ok((r, w))
}

// ---- Framing: a u32 length, JSON, and fds attached to the first byte -----

fn retry_interrupted<T>(mut operation: impl FnMut() -> std::io::Result<T>) -> std::io::Result<T> {
    loop {
        match operation() {
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            result => return result,
        }
    }
}

fn send(stream: &mut UnixStream, message: &impl Serialize, fds: &[RawFd]) -> std::io::Result<()> {
    let body = serde_json::to_vec(message).map_err(std::io::Error::other)?;
    let mut bytes = (body.len() as u32).to_le_bytes().to_vec();
    bytes.extend_from_slice(&body);
    let mut iov = libc::iovec { iov_base: bytes.as_mut_ptr().cast(), iov_len: bytes.len() };
    let mut control = vec![0u8; unsafe { libc::CMSG_SPACE((MAX_FDS * std::mem::size_of::<RawFd>()) as u32) } as usize];
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    if !fds.is_empty() {
        let len = std::mem::size_of_val(fds) as u32;
        msg.msg_control = control.as_mut_ptr().cast();
        msg.msg_controllen = unsafe { libc::CMSG_SPACE(len) } as _;
        // SAFETY: control has room for one header with MAX_FDS descriptors.
        unsafe {
            let cmsg = libc::CMSG_FIRSTHDR(&msg);
            (*cmsg).cmsg_level = libc::SOL_SOCKET;
            (*cmsg).cmsg_type = libc::SCM_RIGHTS;
            (*cmsg).cmsg_len = libc::CMSG_LEN(len) as _;
            std::ptr::copy_nonoverlapping(fds.as_ptr(), libc::CMSG_DATA(cmsg).cast::<RawFd>(), fds.len());
        }
    }
    let n = retry_interrupted(|| {
        let n = unsafe { libc::sendmsg(stream.as_raw_fd(), &msg, 0) };
        if n < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(n as usize)
        }
    })?;
    stream.write_all(&bytes[n..])
}

fn recv<T: for<'de> Deserialize<'de>>(stream: &mut UnixStream) -> std::io::Result<(T, Vec<OwnedFd>)> {
    let mut first = vec![0u8; 64 * 1024];
    let mut iov = libc::iovec { iov_base: first.as_mut_ptr().cast(), iov_len: first.len() };
    let mut control = vec![0u8; unsafe { libc::CMSG_SPACE((MAX_FDS * std::mem::size_of::<RawFd>()) as u32) } as usize];
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = control.as_mut_ptr().cast();
    msg.msg_controllen = control.len() as _;
    let n = retry_interrupted(|| {
        msg.msg_controllen = control.len() as _;
        msg.msg_flags = 0;
        let n = unsafe { libc::recvmsg(stream.as_raw_fd(), &mut msg, 0) };
        if n < 0 {
            Err(std::io::Error::last_os_error())
        } else if n == 0 {
            Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "no message"))
        } else {
            Ok(n as usize)
        }
    })?;
    let mut fds = Vec::new();
    // SAFETY: walks the control headers recvmsg filled in.
    unsafe {
        let mut cmsg = libc::CMSG_FIRSTHDR(&msg);
        while !cmsg.is_null() {
            if (*cmsg).cmsg_level == libc::SOL_SOCKET && (*cmsg).cmsg_type == libc::SCM_RIGHTS {
                let count = ((*cmsg).cmsg_len as usize - libc::CMSG_LEN(0) as usize) / std::mem::size_of::<RawFd>();
                let data = libc::CMSG_DATA(cmsg).cast::<RawFd>();
                for i in 0..count {
                    let fd = OwnedFd::from_raw_fd(*data.add(i));
                    libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC);
                    fds.push(fd);
                }
            }
            cmsg = libc::CMSG_NXTHDR(&msg, cmsg);
        }
    }
    let mut bytes = first[..n as usize].to_vec();
    while bytes.len() < 4 {
        let mut more = [0u8; 4];
        let k = retry_interrupted(|| stream.read(&mut more))?;
        if k == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        bytes.extend_from_slice(&more[..k]);
    }
    let len = u32::from_le_bytes(bytes[..4].try_into().expect("4 bytes")) as usize;
    while bytes.len() < 4 + len {
        let mut more = vec![0u8; 4 + len - bytes.len()];
        let k = retry_interrupted(|| stream.read(&mut more))?;
        if k == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        bytes.extend_from_slice(&more[..k]);
    }
    let message = serde_json::from_slice(&bytes[4..4 + len]).map_err(std::io::Error::other)?;
    Ok((message, fds))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Held while a test spawns a process, and while one relies on a socket
    /// it closed being gone: macOS cannot create a socket close-on-exec
    /// atomically, so a child spawned meanwhile may inherit (and keep) it.
    static SPAWN: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn interrupted_framed_reads_preserve_message_and_descriptors() {
        const CHILD: &str = "LP_FORK_INTERRUPTED_RECV_TEST";
        if std::env::var_os(CHILD).is_none() {
            let _spawn = SPAWN.lock().unwrap_or_else(|e| e.into_inner());
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "fork::tests::interrupted_framed_reads_preserve_message_and_descriptors",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        install_child_notifications();
        for fragmented in [false, true] {
            let (mut reader, mut writer) = UnixStream::pair().unwrap();
            reader.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
            let target = unsafe { libc::pthread_self() } as usize;
            let (read_fd, _write_fd) = pipe().unwrap();
            let sender = std::thread::spawn(move || {
                let body = serde_json::to_vec(&Message::Taken).unwrap();
                let mut bytes = (body.len() as u32).to_le_bytes().to_vec();
                bytes.extend_from_slice(&body);
                if fragmented {
                    writer.write_all(&bytes[..2]).unwrap();
                }
                for _ in 0..10 {
                    std::thread::sleep(Duration::from_millis(5));
                    assert_eq!(unsafe { libc::pthread_kill(target as libc::pthread_t, libc::SIGCHLD) }, 0);
                }
                if fragmented {
                    writer.write_all(&bytes[2..]).unwrap();
                } else {
                    send(&mut writer, &Message::Taken, &[read_fd.as_raw_fd()]).unwrap();
                }
            });
            let result: std::io::Result<(Message, Vec<OwnedFd>)> = recv(&mut reader);
            sender.join().unwrap();
            let (message, descriptors) = result.expect("SIGCHLD must not truncate a framed message");
            assert!(matches!(message, Message::Taken));
            assert_eq!(descriptors.len(), usize::from(!fragmented));
            for fd in descriptors {
                assert_ne!(unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC, 0);
            }
        }
    }

    #[test]
    fn socket_file_names_carry_their_server_pid() {
        assert_eq!(socket_server("lightningpanda-123.sock"), Some(123));
        assert_eq!(socket_server("lightningpanda-123.sock.conn"), Some(123));
        assert_eq!(socket_server("lightningpanda-123.sock.7"), Some(123));
        assert_eq!(socket_server("lightningpanda-123.sock.7.conn"), Some(123));
        assert_eq!(socket_server("lightningpanda-123.socket"), None);
        assert_eq!(socket_server("lightningpanda-abc.sock"), None);
        assert_eq!(socket_server("other-123.sock"), None);
    }

    #[test]
    fn stale_sockets_of_dead_servers_are_removed() {
        // A pid no process has (spawning one here would let it inherit other
        // tests' sockets, which macOS cannot create close-on-exec atomically).
        let dead = (2..99_999u32)
            .rev()
            .find(|&pid| {
                let alive = unsafe { libc::kill(pid as libc::pid_t, 0) } == 0;
                !alive && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
            })
            .unwrap();
        let dir = std::env::temp_dir();
        let stale = dir.join(format!("lightningpanda-{dead}.sock.7.conn"));
        let live = dir.join(format!("lightningpanda-{}.sock.99", std::process::id()));
        let not_a_socket = dir.join(format!("lightningpanda-{dead}.sock.8"));
        drop(UnixListener::bind(&stale).unwrap()); // the file stays
        drop(UnixListener::bind(&live).unwrap());
        std::fs::write(&not_a_socket, b"").unwrap();
        remove_stale_sockets();
        assert!(!stale.exists());
        assert!(live.exists(), "a live server's socket is kept");
        assert!(not_a_socket.exists(), "only sockets are removed");
        remove_socket_files(std::process::id());
        assert!(!live.exists());
        std::fs::remove_file(not_a_socket).unwrap();
    }

    struct SocketFiles(PathBuf);

    impl Drop for SocketFiles {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn connection_fixture() -> (SocketFiles, PathBuf, UnixListener, std::net::TcpStream) {
        // Tests run in parallel, and the clock can read the same for two of them.
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let suffix = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = PathBuf::from(format!("/tmp/lp-fork-{}-{suffix}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let template = dir.join("root");
        let root = UnixListener::bind(conn_socket(&template)).unwrap();
        root.set_nonblocking(true).unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let peer = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (socket, _) = listener.accept().unwrap();
        drop(peer);
        (SocketFiles(dir), template, root, socket)
    }

    #[test]
    fn refused_derived_socket_forwards_connection_to_root() {
        let _spawn = SPAWN.lock().unwrap_or_else(|e| e.into_inner());
        let (_files, template, root, socket) = connection_fixture();
        let derived = conn_socket(&PathBuf::from(format!("{}.7", template.display())));
        drop(UnixListener::bind(&derived).unwrap());
        assert!(derived.exists());
        assert_eq!(UnixStream::connect(&derived).unwrap_err().kind(), std::io::ErrorKind::ConnectionRefused);
        let completion =
            connect(&template, socket, vec![1, 2, 3], Some(7)).expect("root accepts while derived is not listening");
        let (mut stream, _) = root.accept().unwrap();
        let (message, fds): (Message, _) = recv(&mut stream).unwrap();
        match message {
            Message::Connect { buffered, profile } => {
                assert_eq!(buffered, [1, 2, 3]);
                assert_eq!(profile, Some(7));
            }
            _ => panic!("expected forwarded connection"),
        }
        assert_eq!(fds.len(), 2);
        drop(fds);
        completion.blocking_recv().unwrap();
    }

    #[test]
    fn listening_derived_socket_receives_connection_directly() {
        let (_files, template, root, socket) = connection_fixture();
        let derived = UnixListener::bind(conn_socket(&PathBuf::from(format!("{}.7", template.display())))).unwrap();
        derived.set_nonblocking(true).unwrap();
        let completion = connect(&template, socket, Vec::new(), Some(7)).unwrap();
        let (mut stream, _) = derived.accept().unwrap();
        let (message, fds): (Message, _) = recv(&mut stream).unwrap();
        assert!(matches!(message, Message::Connect { profile: Some(7), .. }));
        assert_eq!(root.accept().unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
        drop(fds);
        completion.blocking_recv().unwrap();
    }
}
