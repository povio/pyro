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
    /// Serve a connection: fds are the upgraded socket and the pipe's write end.
    Connect { buffered: Vec<u8> },
    /// Boot a profile for later connections; replies `{"profileId": n}`.
    Register { profile: PreparedProfile },
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

impl Template {
    /// Starts `lightningpanda serve --template <socket> <args>` and waits
    /// until it listens.
    pub fn spawn(args: &[String]) -> std::io::Result<Template> {
        let socket = std::env::temp_dir().join(format!("lightningpanda-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&socket);
        let exe = std::env::current_exe()?;
        let child = std::process::Command::new(exe)
            .arg("serve")
            .arg("--template")
            .arg(&socket)
            .args(args.iter().filter(|a| a.as_str() != "--fork"))
            .stdin(std::process::Stdio::piped())
            .spawn()?;
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
    }
}

/// Hands an upgraded connection to the template; the receiver resolves once
/// the process serving it has exited.
pub fn connect(
    template: &Path,
    socket: std::net::TcpStream,
    buffered: Vec<u8>,
) -> std::io::Result<tokio::sync::oneshot::Receiver<()>> {
    let (read_end, write_end) = pipe()?;
    let mut stream = UnixStream::connect(template)?;
    send(&mut stream, &Message::Connect { buffered }, &[socket.as_raw_fd(), write_end.as_raw_fd()])?;
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

// ---- Template side -------------------------------------------------------

/// What the template boots and forks: the protocol state of a connection
/// that has no socket yet, with one prepared page per profile.
struct Snapshot {
    browser: Browser<EngineHost>,
    outbox: Outbox,
    outbound: OutboundRx,
    /// Indexed by profile id.
    profiles: Vec<ProfileState>,
}

struct ProfileState {
    /// When its page started booting.
    started: Instant,
    /// Settled (one full collection) since it loaded or ran its setup.
    settled: bool,
    /// Its setup steps ran (or it has none).
    set_up: bool,
}

impl ProfileState {
    fn new(profile: &PreparedProfile) -> Self {
        ProfileState { started: Instant::now(), settled: false, set_up: profile.setup.is_empty() }
    }
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
    let browser = Browser::with_prepared(host, profiles);
    let mut snapshot = Snapshot { browser, outbox, outbound, profiles: states };

    let listener = UnixListener::bind(socket).map_err(|e| format!("{}: {e}", socket.display()))?;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let watch = lp_runtime::FdWatch::new(listener.as_raw_fd());
    watch.set_interest(true, false);
    snapshot.browser.host_mut().thread.event_loop.watch_fd(Some(watch.clone()));

    let mut waiting: VecDeque<(Vec<u8>, Vec<OwnedFd>, Instant)> = VecDeque::new();
    loop {
        accept_all(&listener, &mut snapshot, &mut waiting);
        let ready = ready_to_fork(&mut snapshot, waiting.front().map(|w| w.2));
        if ready {
            while let Some((buffered, fds, _)) = waiting.pop_front() {
                if let Some(child) =
                    fork_connection(&mut snapshot, &options.server, (&listener, socket), &mut waiting, buffered, fds)
                {
                    // Only the forked child gets here, and it is done.
                    std::process::exit(child);
                }
            }
        }
        // Boot what is booting; wake for the next message.
        let deadline = Instant::now() + Duration::from_millis(if ready { 1000 } else { 20 });
        let outcome = snapshot.browser.host_mut().run_until_work(Some(deadline), || watch.take_ready());
        snapshot.browser.host_mut().flush();
        snapshot.drain();
        if matches!(outcome, lp_runtime::RunOutcome::Idle) {
            poll_readable(&[listener.as_raw_fd(), 0], if ready { -1 } else { 20 });
        }
        if server_gone() {
            let _ = std::fs::remove_file(socket);
            return Ok(());
        }
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

    /// Runs profile `id`'s setup steps on its page, then hands the page back
    /// to the pool so that forks get it in that state.
    fn set_up(&mut self, id: usize) -> Result<(), String> {
        let steps = self.browser.prepared_profiles()[id].setup.clone();
        let page = self.command(
            None,
            "Lightningpanda.acquirePreparedPage",
            serde_json::json!({ "profileId": id, "attach": true }),
        )?;
        let (Some(target), Some(session)) = (page["targetId"].as_str(), page["sessionId"].as_str()) else {
            return Err("acquirePreparedPage: no target".into());
        };
        let (target, session) = (target.to_owned(), session.to_owned());
        let run = self.command(Some(&session), "LP.run", serde_json::json!({ "steps": steps }));
        self.browser.return_to_prepared(&target).map_err(|e| e.message)?;
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

/// Reads every pending message: registrations are handled now, connections
/// wait until the pages are ready.
fn accept_all(
    listener: &UnixListener,
    snapshot: &mut Snapshot,
    waiting: &mut VecDeque<(Vec<u8>, Vec<OwnedFd>, Instant)>,
) {
    while let Ok((mut stream, _)) = listener.accept() {
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let Ok((message, fds)) = recv(&mut stream) else { continue };
        match message {
            Message::Connect { buffered } if fds.len() == 2 => waiting.push_back((buffered, fds, Instant::now())),
            Message::Connect { .. } => {}
            Message::Register { profile } => {
                let reply = match register(snapshot, profile) {
                    Ok(id) => Registered { profile_id: Some(id), error: None },
                    Err(e) => Registered { profile_id: None, error: Some(e) },
                };
                let _ = send(&mut stream, &reply, &[]);
            }
        }
    }
}

/// Adds a profile (or finds the same one) and starts booting its page.
fn register(snapshot: &mut Snapshot, profile: PreparedProfile) -> Result<usize, String> {
    let profile = one_page(&profile);
    if let Some(id) = snapshot.browser.prepared_profiles().iter().position(|p| *p == profile) {
        return Ok(id);
    }
    snapshot.profiles.push(ProfileState::new(&profile));
    let id = snapshot.browser.add_prepared_profile(profile);
    if !snapshot.browser.prepared_pages().iter().any(|(p, _)| *p == id) {
        return Err(format!("cannot boot a page at {}", snapshot.browser.prepared_profiles()[id].url));
    }
    Ok(id)
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
        let state = &snapshot.profiles[id];
        let waited = state.started.elapsed();
        if !(loaded && (quiet || waited_enough) || waited > LOAD_WAIT) {
            ready = false;
            continue;
        }
        if loaded && !state.set_up {
            snapshot.profiles[id].set_up = true;
            if let Err(e) = snapshot.set_up(id) {
                eprintln!("lightningpanda: template: setup of {}: {e}", snapshot.browser.prepared_profiles()[id].url);
            }
            // Whatever the steps started has to go quiet again.
            snapshot.profiles[id].settled = false;
            ready = false;
            continue;
        }
        if loaded && !state.settled {
            snapshot.profiles[id].settled = true;
            snapshot.browser.host().thread.browser.engine.settle();
        }
    }
    ready
}

/// Forks a process for one connection. Returns in the parent with `None`;
/// in the child, serves the connection and returns its exit code.
fn fork_connection(
    snapshot: &mut Snapshot,
    config: &ServerConfig,
    (listener, template): (&UnixListener, &Path),
    waiting: &mut VecDeque<(Vec<u8>, Vec<OwnedFd>, Instant)>,
    buffered: Vec<u8>,
    mut fds: Vec<OwnedFd>,
) -> Option<i32> {
    snapshot.drain();
    let engine = &snapshot.browser.host().thread.browser.engine;
    lp_net::client::suspend_for_fork();
    if let Err(e) = engine.prepare_for_fork() {
        eprintln!("lightningpanda: template: {e}");
        let _ = lp_net::client::resume_after_fork();
        return None;
    }
    // JSC's VMTraps queue thread is the one expected survivor: it holds no
    // lock while idle, and JSDidFork starts a new one in the child.
    let others: Vec<String> = other_threads().into_iter().filter(|n| n != "JSC VMTraps Signal Sender").collect();
    if !others.is_empty() {
        eprintln!("lightningpanda: template: threads alive at fork: {others:?}");
    }
    // SAFETY: only this thread is alive (checked above, best effort), and
    // the child continues on it after JSC and the network are told.
    let pid = unsafe { libc::fork() };
    if pid != 0 {
        engine.did_fork(false);
        if let Err(e) = lp_net::client::resume_after_fork() {
            eprintln!("lightningpanda: template: network restart failed: {e}");
        }
        if pid < 0 {
            eprintln!("lightningpanda: template: fork failed: {}", std::io::Error::last_os_error());
        }
        return None; // the parent's copies of the fds close here
    }

    // Child: one connection, then exit.
    engine.did_fork(true);
    if let Err(e) = lp_net::client::resume_after_fork() {
        eprintln!("lightningpanda: network restart failed: {e}");
        return Some(1);
    }
    snapshot.browser.host_mut().thread.event_loop.watch_fd(None);
    // Other connections' sockets and pipes are theirs: holding them would keep them open.
    waiting.clear();
    let template = template.to_path_buf();
    unsafe { libc::close(listener.as_raw_fd()) };
    let finished = fds.pop().expect("two fds");
    let socket = std::net::TcpStream::from(fds.pop().expect("two fds"));
    // The child owns the template's protocol state from here; the template's
    // copy of `snapshot` is never touched again in this process.
    let (mut browser, outbox, outbound) = unsafe {
        (std::ptr::read(&snapshot.browser), std::ptr::read(&snapshot.outbox), std::ptr::read(&snapshot.outbound))
    };
    browser.set_fork_child(Box::new(move |p: &PreparedProfile| register_with(&template, p)));
    crate::serve::serve_browser(browser, outbox, outbound, socket, &buffered, config.clone());
    drop(finished);
    Some(0)
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
