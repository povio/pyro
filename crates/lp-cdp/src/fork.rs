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

use std::collections::VecDeque;
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
use crate::target::{Browser, Host, PreparedProfile};
use crate::ws::OutFrame;

/// A connection waits at most this long for loaded pages to go quiet: a
/// request still in flight at the fork fails in the template (its network
/// threads stop), and polling apps keep timers due.
const QUIET_WAIT: Duration = Duration::from_secs(2);

/// A page that has not loaded by then is forked as it is.
const LOAD_WAIT: Duration = Duration::from_secs(30);

/// A profile's setup steps must finish within this.
const SETUP_TIMEOUT: Duration = Duration::from_secs(60);

/// Forked children each template keeps waiting for a connection
/// (`LIGHTNINGPANDA_FORK_SPARES`), so that a connection does not wait for its
/// fork.
fn spares_wanted() -> usize {
    std::env::var("LIGHTNINGPANDA_FORK_SPARES").ok().and_then(|v| v.parse().ok()).unwrap_or(2)
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
    Connect { buffered: Vec<u8>, profile: Option<usize> },
    /// Boot a profile for later connections (root only); replies `{"profileId": n}`.
    Register { profile: PreparedProfile },
    /// Fork a template for profile `id` off this one's snapshot of `base`,
    /// listening on `socket` (root to the template holding `base`).
    Derive { base: usize, id: usize, profile: PreparedProfile, socket: PathBuf },
    /// A spare took a connection: wakes its template to fork a replacement.
    Taken,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Registered {
    profile_id: Option<usize>,
    error: Option<String>,
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
/// forks inherit them. A fork serves one short connection: code it tiers up
/// to the DFG (and FTL) is compiled on helper threads that compete with the
/// other forks for cores, and the connection usually ends before that code
/// pays for itself. Baseline JIT and LLInt stay on. Set `JSC_useDFGJIT=true`
/// to get the default tiers back.
const FORK_JSC_DEFAULTS: [(&str, &str); 1] = [("JSC_useDFGJIT", "false")];

impl Template {
    /// Starts `lightningpanda serve --template <socket> <args>` and waits
    /// until it listens.
    pub fn spawn(args: &[String]) -> std::io::Result<Template> {
        let socket = std::env::temp_dir().join(format!("lightningpanda-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&socket);
        let exe = std::env::current_exe()?;
        let mut command = std::process::Command::new(exe);
        command
            .arg("serve")
            .arg("--template")
            .arg(&socket)
            .args(args.iter().filter(|a| a.as_str() != "--fork"))
            .stdin(std::process::Stdio::piped());
        for (name, value) in FORK_JSC_DEFAULTS {
            if std::env::var_os(name).is_none() {
                command.env(name, value);
            }
        }
        let child = command.spawn()?;
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
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.socket);
        let _ = std::fs::remove_file(conn_socket(&self.socket));
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
    let derived = profile.map(|id| conn_socket(&PathBuf::from(format!("{}.{id}", template.display()))));
    let target = derived.filter(|p| p.exists()).unwrap_or_else(|| conn_socket(template));
    let mut stream = UnixStream::connect(target)?;
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
struct Derive {
    base: usize,
    id: usize,
    profile: PreparedProfile,
    socket: PathBuf,
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
    is_root: bool,
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
    let profile = crate::serve::browser_profile(options)?;
    if !supported() {
        return Err("this JavaScriptCore cannot fork (build the patched one: scripts/jsc/build.sh)".into());
    }
    // Children are never waited for: let the kernel reap them.
    unsafe { libc::signal(libc::SIGCHLD, libc::SIG_IGN) };
    // The static-file preload runs on a thread of its own: let it finish.
    let _ = profile.net.static_files.preloaded();

    let mut thread = EngineHost::prepare_thread_with(&profile).map_err(|e| e.message)?;
    let profiles: Vec<PreparedProfile> = profile.prepared.iter().map(one_page).collect();
    let pages = EngineHost::preboot(&mut thread, &profiles);
    let (outbox, outbound) = Outbox::channel(DEFAULT_OUTBOUND_BUDGET);
    let mut host = EngineHost::with_thread(thread, outbox.clone(), EngineHost::body_budget()).map_err(|e| e.message)?;
    host.adopt_prebooted(pages);
    let states = profiles.iter().map(ProfileState::new).collect();
    let holders = vec![None; profiles.len()];
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
        is_root: true,
        spares: VecDeque::new(),
        spares_generation: 0,
    };
    t.watch_listener();

    'serve: loop {
        t.accept_all();
        // Spares that took a connection have closed their end.
        t.spares.retain(|spare| !readable(spare.as_raw_fd()));
        if t.spares.is_empty() {
            t.accept_conns();
        }
        t.forward_waiting();
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
                    std::process::exit(child);
                }
            }
            if let Some(child) = t.refill_spares(&options.server) {
                std::process::exit(child);
            }
        }
        // Boot what is booting; wake for the next message.
        let busy = !ready || !t.forwards.is_empty();
        let deadline = Instant::now() + Duration::from_millis(if busy { 20 } else { 1000 });
        t.watch_listener();
        let watch = t.watch.clone();
        let outcome = t.snapshot.browser.host_mut().run_until_work(Some(deadline), || watch.take_ready());
        t.snapshot.browser.host_mut().flush();
        t.snapshot.drain();
        if matches!(outcome, lp_runtime::RunOutcome::Idle) {
            let mut fds = vec![t.listener.as_raw_fd(), t.conns.as_raw_fd(), 0];
            fds.extend(t.spares.iter().map(|s| s.as_raw_fd()));
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
                Message::Register { profile } => {
                    let reply = match self.register(profile) {
                        Ok(id) => Registered { profile_id: Some(id), error: None },
                        Err(e) => Registered { profile_id: None, error: Some(e) },
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
            let raw: Vec<RawFd> = fds.iter().map(|f| f.as_raw_fd()).collect();
            let sent = UnixStream::connect(conn_socket(&path))
                .and_then(|mut s| send(&mut s, &Message::Connect { buffered: buffered.clone(), profile }, &raw));
            if sent.is_err() && since.elapsed() < LOAD_WAIT {
                self.forwards.push_back((path, buffered, profile, fds, since));
            }
        }
    }

    /// Adds a profile (or finds the same one): a plain one boots here, a
    /// derived one is forked off the template holding its base.
    fn register(&mut self, profile: PreparedProfile) -> Result<usize, String> {
        if !self.is_root {
            return Err("registrations go to the root template".into());
        }
        let profile = one_page(&profile);
        if let Some(id) = self.snapshot.browser.prepared_profiles().iter().position(|p| *p == profile) {
            return Ok(id);
        }
        if let Some(base) = profile.base {
            if base >= self.snapshot.browser.prepared_profiles().len() {
                return Err(format!("no profile {base} to start from"));
            }
        }
        self.snapshot.profiles.push(ProfileState::new(&profile));
        self.snapshot.generation += 1;
        // Before the reply: a spare forked earlier must not take a connection that wants this profile.
        self.spares.clear();
        let id = self.snapshot.browser.add_prepared_profile(profile.clone());
        self.holders.resize(id + 1, None);
        let Some(base) = profile.base else {
            if !self.snapshot.browser.prepared_pages().iter().any(|(p, _)| *p == id) {
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
            Some(holder) => {
                let Derive { base, id, profile, socket } = derive;
                let mut stream = UnixStream::connect(&holder).map_err(|e| format!("template of {base}: {e}"))?;
                send(&mut stream, &Message::Derive { base, id, profile, socket }, &[]).map_err(|e| e.to_string())?;
                let _: (Registered, _) = recv(&mut stream).map_err(|e| e.to_string())?;
            }
        }
        Ok(id)
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
        match fork_process(&self.snapshot) {
            Forked::Parent | Forked::Failed => false, // the parent's copy of the listener closes here
            Forked::Child => {
                self.listener = listener; // closes this child's copies of the parent's
                self.conns = conns;
                self.socket = derive.socket;
                self.is_root = false;
                self.holders.clear();
                self.waiting.clear();
                self.forwards.clear();
                self.derives.clear();
                self.spares.clear(); // the parent's: closing our copies lets them see the parent go
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
        match fork_process(&self.snapshot) {
            Forked::Parent | Forked::Failed => None, // the parent's copies of the fds close here
            Forked::Child => {
                self.leave_template();
                unsafe { libc::close(self.conns.as_raw_fd()) };
                Some(self.serve_child(config, buffered, fds))
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
            match fork_process(&self.snapshot) {
                Forked::Failed => return None,
                Forked::Parent => self.spares.push_back(ours),
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
                return 0;
            }
            let Ok((mut stream, _)) = self.conns.accept() else { continue };
            let _ = stream.set_nonblocking(false);
            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
            let Ok((Message::Connect { buffered, profile }, fds)) = recv(&mut stream) else { continue };
            if fds.len() != 2 {
                continue;
            }
            if profile.is_some_and(|id| self.holders.get(id).is_some_and(|h| h.is_some())) {
                // A derived snapshot that was not up when the server looked: the
                // template queues it until that snapshot's template listens.
                let raw: Vec<RawFd> = fds.iter().map(|f| f.as_raw_fd()).collect();
                if let Ok(mut s) = UnixStream::connect(&self.socket) {
                    let _ = send(&mut s, &Message::Connect { buffered, profile }, &raw);
                }
                continue;
            }
            drop(to_template); // this spare is gone from the template's list
            if let Ok(mut s) = UnixStream::connect(&self.socket) {
                let _ = send(&mut s, &Message::Taken, &[]); // and it wakes to fork a replacement
            }
            self.snapshot.browser.host_mut().thread.event_loop.watch_fd(None);
            unsafe { libc::close(self.conns.as_raw_fd()) };
            return self.serve_child(config, buffered, fds);
        }
    }

    /// In a forked child, right after the fork: lets go of everything that
    /// belongs to the template. Other connections' sockets and pipes, and the
    /// other spares, would stay open while this process holds them.
    fn leave_template(&mut self) {
        self.snapshot.browser.host_mut().thread.event_loop.watch_fd(None);
        self.waiting.clear();
        self.forwards.clear();
        self.spares.clear();
        self.derives.clear();
        // `holders` stays: a spare of the root passes on connections for derived snapshots.
        unsafe { libc::close(self.listener.as_raw_fd()) };
    }

    /// In a forked child (after `leave_template`): serves one connection with
    /// the snapshot; the exit code.
    fn serve_child(&mut self, config: &ServerConfig, buffered: Vec<u8>, mut fds: Vec<OwnedFd>) -> i32 {
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
        browser.set_fork_child(Box::new(move |p: &PreparedProfile| register_with(&root, p)));
        crate::serve::serve_browser(browser, outbox, outbound, socket, &buffered, config.clone());
        drop(finished);
        0
    }
}

enum Forked {
    Parent,
    Child,
    Failed,
}

/// `fork()` with JavaScriptCore and the network told before and after.
fn fork_process(snapshot: &Snapshot) -> Forked {
    let engine = &snapshot.browser.host().thread.browser.engine;
    lp_net::client::suspend_for_fork();
    if let Err(e) = engine.prepare_for_fork() {
        eprintln!("lightningpanda: template: {e}");
        let _ = lp_net::client::resume_after_fork();
        return Forked::Failed;
    }
    // JSC's VMTraps queue thread is the one expected survivor: it holds no
    // lock while idle, and JSDidFork starts a new one in the child.
    let others: Vec<String> = other_threads().into_iter().filter(|n| n != "JSC VMTraps Signal Sender").collect();
    if !others.is_empty() {
        eprintln!("lightningpanda: template: threads alive at fork: {others:?}");
    }
    // SAFETY: only this thread is alive (checked above, best effort), and
    // each process continues on it after JSC and the network are told.
    let pid = unsafe { libc::fork() };
    if pid == 0 {
        // SAFETY: the network is stopped and this is the sole surviving thread.
        // Keep inherited handles, but isolate their pipe from sibling readers.
        if let Err(e) = unsafe { snapshot.browser.host().thread.event_loop.reset_after_fork() } {
            eprintln!("lightningpanda: child wake pipe reset failed: {e}");
            std::process::exit(1);
        }
    }
    engine.did_fork(pid == 0);
    if let Err(e) = lp_net::client::resume_after_fork() {
        eprintln!("lightningpanda: network restart failed: {e}");
        if pid == 0 {
            std::process::exit(1);
        }
    }
    match pid {
        0 => Forked::Child,
        p if p < 0 => {
            eprintln!("lightningpanda: template: fork failed: {}", std::io::Error::last_os_error());
            Forked::Failed
        }
        _ => Forked::Parent,
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

/// Sends a registration to the template; its profile id there.
fn register_with(template: &Path, profile: &PreparedProfile) -> Result<usize, String> {
    let mut stream = UnixStream::connect(template).map_err(|e| format!("template: {e}"))?;
    send(&mut stream, &Message::Register { profile: profile.clone() }, &[]).map_err(|e| e.to_string())?;
    let (reply, _): (Registered, _) = recv(&mut stream).map_err(|e| e.to_string())?;
    match (reply.profile_id, reply.error) {
        (Some(id), _) => Ok(id),
        (None, e) => Err(e.unwrap_or_else(|| "registration failed".into())),
    }
}

unsafe extern "C" {
    fn mach_port_deallocate(task: libc::mach_port_t, name: libc::mach_port_t) -> libc::kern_return_t;
}

/// Names of this process's threads other than the calling one.
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
    let n = unsafe { libc::sendmsg(stream.as_raw_fd(), &msg, 0) };
    if n < 0 {
        return Err(std::io::Error::last_os_error());
    }
    stream.write_all(&bytes[n as usize..])
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
    let n = unsafe { libc::recvmsg(stream.as_raw_fd(), &mut msg, 0) };
    if n <= 0 {
        return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "no message"));
    }
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
        let k = stream.read(&mut more)?;
        if k == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        bytes.extend_from_slice(&more[..k]);
    }
    let len = u32::from_le_bytes(bytes[..4].try_into().expect("4 bytes")) as usize;
    while bytes.len() < 4 + len {
        let mut more = vec![0u8; 4 + len - bytes.len()];
        let k = stream.read(&mut more)?;
        if k == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        bytes.extend_from_slice(&more[..k]);
    }
    let message = serde_json::from_slice(&bytes[4..4 + len]).map_err(std::io::Error::other)?;
    Ok((message, fds))
}
