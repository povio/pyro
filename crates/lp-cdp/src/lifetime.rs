//! How a `serve` process ends: with the process it runs for, and cleanly on
//! a termination signal.
//!
//! `--parent-pid <PID>` (`LIGHTNINGPANDA_PARENT_PID`, which
//! `Lightningpanda.launch()` sets to its own pid): the server exits once that
//! process has, however it ended. A test runner that stops its workers
//! without running their exit handlers (`bun test --parallel`) would
//! otherwise leave every browser they launched running, in a process group
//! of its own. The wait is a kernel event (kqueue `NOTE_EXIT` on macOS, a
//! pidfd on Linux), not polling.
//!
//! SIGTERM, SIGINT and SIGHUP (`serve --fork`): the server removes its
//! template's socket files before it dies of the signal.

use std::io::Read;
use std::os::fd::{FromRawFd, OwnedFd};
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::Duration;

/// Runs `cleanup` and exits once process `pid` has exited (now, if it
/// already has). A thread of its own waits.
pub fn exit_with(pid: libc::pid_t, cleanup: fn()) -> std::io::Result<()> {
    std::thread::Builder::new().name("lp-parent-watch".into()).spawn(move || {
        wait_for_exit(pid);
        cleanup();
        // SAFETY: ends the process without running atexit handlers or static
        // destructors while other threads (browser threads) still run.
        unsafe { libc::_exit(0) };
    })?;
    Ok(())
}

/// Write end of the self-pipe the signal handler writes the signal to.
static SIGNAL_PIPE: AtomicI32 = AtomicI32::new(-1);

extern "C" fn on_signal(signal: libc::c_int) {
    let fd = SIGNAL_PIPE.load(Ordering::Relaxed);
    let byte = signal as u8;
    // SAFETY: write(2) is async-signal-safe; one byte to our own pipe.
    unsafe { libc::write(fd, (&byte as *const u8).cast(), 1) };
}

/// On SIGTERM, SIGINT or SIGHUP, runs `cleanup` (on a thread of its own,
/// not in the handler), then dies of the signal as it would have.
pub fn cleanup_on_signals(cleanup: fn()) -> std::io::Result<()> {
    let mut fds = [0; 2];
    // SAFETY: pipe(2) fills both descriptors, which are then owned here.
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let (read, write) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
    for fd in &fds {
        // SAFETY: our own descriptors; the template and forks must not inherit them.
        unsafe { libc::fcntl(*fd, libc::F_SETFD, libc::FD_CLOEXEC) };
    }
    std::thread::Builder::new().name("lp-signal-watch".into()).spawn(move || {
        let mut read = std::fs::File::from(read);
        let mut byte = [0u8; 1];
        loop {
            match read.read(&mut byte) {
                Ok(1) => break,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                _ => return,
            }
        }
        cleanup();
        let signal = libc::c_int::from(byte[0]);
        // SAFETY: back to the default action, then the same signal again.
        unsafe {
            libc::signal(signal, libc::SIG_DFL);
            libc::kill(libc::getpid(), signal);
            libc::_exit(128 + signal);
        }
    })?;
    // The thread owns the read end; the handler writes to the write end for
    // the rest of the process.
    SIGNAL_PIPE.store(std::os::fd::IntoRawFd::into_raw_fd(write), Ordering::Relaxed);
    for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
        // SAFETY: installs a handler that only calls write(2).
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = on_signal as *const () as usize;
            action.sa_flags = libc::SA_RESTART;
            libc::sigemptyset(&mut action.sa_mask);
            libc::sigaction(signal, &action, std::ptr::null_mut());
        }
    }
    Ok(())
}

fn errno() -> Option<i32> {
    std::io::Error::last_os_error().raw_os_error()
}

/// Returns once process `pid` has exited.
#[cfg(target_os = "macos")]
fn wait_for_exit(pid: libc::pid_t) {
    // SAFETY: kqueue calls on a descriptor this function owns and closes.
    unsafe {
        let kq = libc::kqueue();
        if kq < 0 {
            return poll_for_exit(pid);
        }
        let mut change: libc::kevent = std::mem::zeroed();
        change.ident = pid as libc::uintptr_t;
        change.filter = libc::EVFILT_PROC;
        change.flags = libc::EV_ADD | libc::EV_ONESHOT;
        change.fflags = libc::NOTE_EXIT;
        if libc::kevent(kq, &change, 1, std::ptr::null_mut(), 0, std::ptr::null()) < 0 {
            let gone = errno() == Some(libc::ESRCH);
            libc::close(kq);
            if !gone {
                poll_for_exit(pid);
            }
            return;
        }
        let mut event: libc::kevent = std::mem::zeroed();
        loop {
            let n = libc::kevent(kq, std::ptr::null(), 0, &mut event, 1, std::ptr::null());
            if n > 0 {
                break;
            }
            if n < 0 && errno() != Some(libc::EINTR) {
                libc::close(kq);
                return poll_for_exit(pid);
            }
        }
        libc::close(kq);
    }
}

/// Returns once process `pid` has exited.
#[cfg(target_os = "linux")]
fn wait_for_exit(pid: libc::pid_t) {
    // SAFETY: pidfd_open takes no pointers; the descriptor is ours to close.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) } as libc::c_int;
    if fd < 0 {
        if errno() == Some(libc::ESRCH) {
            return;
        }
        return poll_for_exit(pid); // a kernel before 5.3
    }
    let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
    loop {
        // SAFETY: one valid pollfd.
        let n = unsafe { libc::poll(&mut pfd, 1, -1) };
        if n > 0 {
            break;
        }
        if n < 0 && errno() != Some(libc::EINTR) {
            // SAFETY: our descriptor.
            unsafe { libc::close(fd) };
            return poll_for_exit(pid);
        }
    }
    // SAFETY: our descriptor.
    unsafe { libc::close(fd) };
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn wait_for_exit(pid: libc::pid_t) {
    poll_for_exit(pid)
}

/// The fallback: checks twice a second (our parent counts as gone once we
/// were reparented).
fn poll_for_exit(pid: libc::pid_t) {
    // SAFETY: getppid and kill(pid, 0) have no preconditions.
    let parent = unsafe { libc::getppid() } == pid;
    loop {
        if parent && unsafe { libc::getppid() } != pid {
            return;
        }
        if unsafe { libc::kill(pid, 0) } != 0 && errno() == Some(libc::ESRCH) {
            return;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}
