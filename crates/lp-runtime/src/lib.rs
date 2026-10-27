//! Single-threaded event loop for a browser thread.
//!
//! * [`EventLoop<S>`] owns the queues and runs tasks with `&mut S` (the
//!   browser state owned by the thread).
//! * [`Scheduler<S>`] (`Rc`, `!Send`) posts tasks and timers from inside the
//!   thread, including from JavaScript callbacks that only have a realm.
//! * [`LoopHandle<S>`] (`Send + Clone`) posts tasks from other threads (network
//!   I/O, CDP socket) and wakes the loop; it fails cleanly once the loop is gone.
//!
//! Microtasks are drained by JavaScriptCore when the outermost API call
//! returns; after every task the loop calls [`LoopState::after_task`] (the
//! embedder's checkpoint: finalizer drain, observer delivery, ...).

use std::cell::{Cell, RefCell};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, VecDeque};
use std::rc::Rc;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Timed waits of the loop. `std`'s `recv_timeout`/`park_timeout`/`sleep`
/// are kernel-coalesced on macOS: measured on macOS 27 (M1 Pro), a 16 ms
/// wait returned after 100-145 ms and a 100 ms wait after ~240 ms, while a
/// `poll(2)`/`kevent(2)` timeout returned within 2 ms of the request. Every
/// delayed timer, `requestAnimationFrame` and the scroll/debounce cadences
/// built on them inherited that lateness. The loop therefore sleeps in
/// `poll(2)` on a self-pipe that cross-thread posters write to.
#[cfg(unix)]
mod wake {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    /// Write end, shared by every [`super::LoopHandle`], and whether the
    /// loop is (about to be) asleep: posts write to the pipe only then.
    pub(super) struct Notifier(OwnedFd, Arc<AtomicBool>);
    /// Read end, owned by the loop. Once every notifier is gone the pipe
    /// reports POLLHUP for good, so later waits sleep on no descriptor.
    pub(super) struct Sleeper(OwnedFd, std::cell::Cell<bool>, Arc<AtomicBool>);

    pub(super) fn pair() -> std::io::Result<(Sleeper, Arc<Notifier>)> {
        let mut fds = [0i32; 2];
        // SAFETY: `fds` is a valid 2-element buffer for pipe(2).
        if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: pipe(2) returned two fresh descriptors we now own.
        let (read, write) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
        for fd in [&read, &write] {
            // SAFETY: valid descriptor; flags are plain ints.
            unsafe {
                let flags = libc::fcntl(fd.as_raw_fd(), libc::F_GETFL);
                if flags < 0 || libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
            }
        }
        // A handle may post just as the loop (and the read end) goes away:
        // the write must fail with EPIPE, never raise SIGPIPE. Rust binaries
        // ignore SIGPIPE at startup; embedders may not, so pin it per fd.
        #[cfg(target_os = "macos")]
        // SAFETY: valid descriptor; F_SETNOSIGPIPE takes an int flag.
        if unsafe { libc::fcntl(write.as_raw_fd(), libc::F_SETNOSIGPIPE, 1) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let sleeping = Arc::new(AtomicBool::new(false));
        Ok((Sleeper(read, std::cell::Cell::new(false), sleeping.clone()), Arc::new(Notifier(write, sleeping))))
    }

    impl Notifier {
        /// Wakes the loop if it sleeps or is about to: called after the task
        /// is in the channel. Pairs with [`Sleeper::announce`] (both sides
        /// publish, fence, then read the other's state), so either this sees
        /// the flag or the loop sees the task before it blocks.
        pub(super) fn notify_if_sleeping(&self) {
            std::sync::atomic::fence(Ordering::SeqCst);
            if self.1.load(Ordering::SeqCst) {
                self.notify();
            }
        }

        /// Wakes the loop. A full pipe (EAGAIN) already holds a pending wake;
        /// EPIPE means the loop is gone and the channel send already failed
        /// or the task can never run; EINTR is retried. Anything else is a
        /// programming error (bad descriptor): surfaced in debug builds.
        pub(super) fn notify(&self) {
            loop {
                // SAFETY: one byte from a valid buffer to our descriptor.
                if unsafe { libc::write(self.0.as_raw_fd(), [1u8].as_ptr().cast(), 1) } == 1 {
                    return;
                }
                let err = std::io::Error::last_os_error();
                match err.raw_os_error() {
                    Some(libc::EINTR) => continue,
                    Some(libc::EAGAIN) | Some(libc::EPIPE) => return,
                    _ => {
                        debug_assert!(false, "event loop wake pipe write failed: {err}");
                        return;
                    }
                }
            }
        }
    }

    impl Sleeper {
        /// Marks the loop as going to sleep (`true`, before its last check of
        /// the task channel) or awake again (`false`).
        pub(super) fn announce(&self, sleeping: bool) {
            self.2.store(sleeping, Ordering::SeqCst);
            if sleeping {
                std::sync::atomic::fence(Ordering::SeqCst);
            }
        }

        /// Sleeps until `wait` elapses (forever when `None`) or a notifier
        /// writes or every notifier is gone. Returns whether a notifier
        /// still exists (`false` once the write end is closed everywhere).
        pub(super) fn sleep(&self, wait: Option<Duration>, watch: Option<&super::FdWatch>) -> bool {
            // poll(2) counts whole milliseconds: round up so the loop never
            // wakes before a deadline and spins.
            let timeout = match wait {
                None => -1,
                Some(w) => i32::try_from(w.as_millis() + u128::from(w.subsec_nanos() % 1_000_000 != 0)).unwrap_or(i32::MAX),
            };
            let watched = watch.and_then(|w| {
                let events = (if w.read.get() { libc::POLLIN } else { 0 }) | (if w.write.get() { libc::POLLOUT } else { 0 });
                (events != 0).then(|| libc::pollfd { fd: w.fd, events, revents: 0 })
            });
            let mut fds = [libc::pollfd { fd: self.0.as_raw_fd(), events: libc::POLLIN, revents: 0 }; 2];
            let mut nfds = 0;
            if !self.1.get() {
                nfds += 1;
            }
            if let Some(w) = watched {
                fds[nfds] = w;
                nfds += 1;
            }
            let pipe_watched = !self.1.get();
            if nfds == 0 {
                // No notifier and no descriptor: a plain timed wait (still
                // poll(2), which is not coalesced). `None` would block forever:
                // callers never pass it once every sender is gone.
                // SAFETY: zero descriptors; poll only sleeps.
                unsafe { libc::poll(std::ptr::null_mut(), 0, timeout.max(0)) };
                return false;
            }
            // SAFETY: `nfds` initialized pollfds.
            let n = unsafe { libc::poll(fds.as_mut_ptr(), nfds as libc::nfds_t, timeout) };
            if n < 0 {
                let err = std::io::Error::last_os_error();
                // EINTR: the caller recomputes its wait and polls again.
                debug_assert!(err.raw_os_error() == Some(libc::EINTR), "event loop wake pipe poll failed: {err}");
                return pipe_watched;
            }
            if let (Some(w), true) = (watch, watched.is_some()) {
                if fds[nfds - 1].revents != 0 {
                    w.ready.set(true);
                }
            }
            if !pipe_watched {
                return false;
            }
            let pipe = fds[0];
            if pipe.revents == 0 {
                return true;
            }
            // Drain every pending wake byte. EAGAIN ends the drain; EINTR
            // retries; anything else is a programming error (bad descriptor).
            let mut buf = [0u8; 64];
            loop {
                // SAFETY: valid buffer and descriptor; non-blocking read.
                let got = unsafe { libc::read(self.0.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
                if got > 0 {
                    continue;
                }
                if got == 0 {
                    break; // write end closed: POLLHUP below records it
                }
                let err = std::io::Error::last_os_error();
                match err.raw_os_error() {
                    Some(libc::EINTR) => continue,
                    Some(libc::EAGAIN) => break,
                    _ => {
                        debug_assert!(false, "event loop wake pipe read failed: {err}");
                        break;
                    }
                }
            }
            if pipe.revents & libc::POLLHUP != 0 {
                self.1.set(true);
                return false;
            }
            true
        }
    }
}

/// HTML task sources (informational today; kept for ordering policies).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskSource {
    DomManipulation,
    UserInteraction,
    Networking,
    Navigation,
    Timer,
    Remote,
    Internal,
}

pub type Task<S> = Box<dyn FnOnce(&mut S)>;
pub type RemoteTask<S> = Box<dyn FnOnce(&mut S) + Send>;

/// Hooks the loop calls on the state between tasks.
pub trait LoopState {
    /// Called after every task (checkpoint for embedder work).
    fn after_task(&mut self) {}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TimerId(pub u32);

struct Timer<S> {
    task: Rc<dyn Fn(&mut S)>,
    interval: Option<Duration>,
    /// Task-source bookkeeping for `blocks_completion` style queries.
    keeps_alive: bool,
}

struct Inner<S> {
    /// Local tasks with the time they were queued (their readiness time).
    tasks: RefCell<VecDeque<(Instant, TaskSource, Task<S>)>>,
    heap: RefCell<BinaryHeap<Reverse<(Instant, u64, TimerId)>>>,
    timers: RefCell<HashMap<TimerId, Timer<S>>>,
    next_timer: Cell<u32>,
    seq: Cell<u64>,
    stop: Cell<bool>,
    /// Live timers with `keeps_alive` (kept in sync with `timers`).
    keepalive: Cell<usize>,
}

impl<S> Inner<S> {
    fn insert_timer(&self, id: TimerId, timer: Timer<S>) {
        if timer.keeps_alive {
            self.keepalive.set(self.keepalive.get() + 1);
        }
        if let Some(old) = self.timers.borrow_mut().insert(id, timer) {
            if old.keeps_alive {
                self.keepalive.set(self.keepalive.get() - 1);
            }
        }
    }

    fn remove_timer(&self, id: TimerId) -> bool {
        match self.timers.borrow_mut().remove(&id) {
            Some(t) => {
                if t.keeps_alive {
                    self.keepalive.set(self.keepalive.get() - 1);
                }
                true
            }
            None => false,
        }
    }

    /// Drops heap heads whose timer was cleared, so `peek` is a live deadline.
    fn clean_heap_top(&self) {
        let timers = self.timers.borrow();
        let mut heap = self.heap.borrow_mut();
        while let Some(Reverse((_, _, id))) = heap.peek() {
            if timers.contains_key(id) {
                break;
            }
            heap.pop();
        }
    }

    /// Rebuilds the heap when cleared timers dominate it (bounded memory for
    /// pages that create and clear many long timers).
    fn maybe_compact(&self) {
        let live = self.timers.borrow().len();
        if self.heap.borrow().len() > 2 * live + 64 {
            let timers = self.timers.borrow();
            self.heap.borrow_mut().retain(|Reverse((_, _, id))| timers.contains_key(id));
        }
    }
}

/// In-thread scheduling handle.
pub struct Scheduler<S> {
    inner: Rc<Inner<S>>,
}

impl<S> Clone for Scheduler<S> {
    fn clone(&self) -> Self {
        Scheduler { inner: self.inner.clone() }
    }
}

impl<S: 'static> Scheduler<S> {
    pub fn post(&self, source: TaskSource, task: impl FnOnce(&mut S) + 'static) {
        self.inner.tasks.borrow_mut().push_back((Instant::now(), source, Box::new(task)));
    }

    /// Schedules `task` after `delay` (repeating every `delay` if `repeat`).
    /// `keeps_alive = false` marks timers that should not keep
    /// [`EventLoop::run_until_idle`] running (e.g. long polling intervals).
    pub fn set_timer(
        &self,
        delay: Duration,
        repeat: bool,
        keeps_alive: bool,
        task: impl Fn(&mut S) + 'static,
    ) -> TimerId {
        let id = TimerId(self.inner.next_timer.get());
        self.inner.next_timer.set(id.0.wrapping_add(1).max(1));
        self.inner.insert_timer(id, Timer { task: Rc::new(task), interval: repeat.then_some(delay), keeps_alive });
        self.push_deadline(Instant::now() + delay, id);
        id
    }

    pub fn clear_timer(&self, id: TimerId) -> bool {
        // The heap entry becomes stale: skipped at the top, compacted in bulk.
        let removed = self.inner.remove_timer(id);
        self.inner.maybe_compact();
        removed
    }

    fn push_deadline(&self, at: Instant, id: TimerId) {
        let seq = self.inner.seq.get();
        self.inner.seq.set(seq + 1);
        self.inner.heap.borrow_mut().push(Reverse((at, seq, id)));
    }

    /// Asks the loop to return from `run*` after the current task.
    pub fn stop(&self) {
        self.inner.stop.set(true);
    }

    pub fn pending_tasks(&self) -> usize {
        self.inner.tasks.borrow().len()
    }

    pub fn active_timers(&self) -> usize {
        self.inner.keepalive.get()
    }
}

/// Error returned when the loop that a [`LoopHandle`] targets has exited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoopClosed;

impl std::fmt::Display for LoopClosed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("event loop has exited")
    }
}

impl std::error::Error for LoopClosed {}

/// A remote task with the time it was posted (its readiness time).
type Stamped<S> = (Instant, RemoteTask<S>);

/// Cross-thread posting handle. Tasks are delivered in posting order.
pub struct LoopHandle<S> {
    tx: mpsc::Sender<Stamped<S>>,
    #[cfg(unix)]
    wake: Arc<wake::Notifier>,
}

impl<S> Clone for LoopHandle<S> {
    fn clone(&self) -> Self {
        LoopHandle {
            tx: self.tx.clone(),
            #[cfg(unix)]
            wake: self.wake.clone(),
        }
    }
}

impl<S> LoopHandle<S> {
    pub fn post(&self, task: impl FnOnce(&mut S) + Send + 'static) -> Result<(), LoopClosed> {
        self.tx.send((Instant::now(), Box::new(task))).map_err(|_| LoopClosed)?;
        #[cfg(unix)]
        self.wake.notify_if_sleeping();
        Ok(())
    }
}

/// Why a `run*` call returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutcome {
    /// The predicate became true.
    Done,
    /// No tasks, no keep-alive timers and no remote senders left.
    Idle,
    /// The deadline passed.
    TimedOut,
    /// [`Scheduler::stop`] was called.
    Stopped,
}

pub struct EventLoop<S> {
    sched: Scheduler<S>,
    tx: Option<mpsc::Sender<Stamped<S>>>,
    rx: mpsc::Receiver<Stamped<S>>,
    /// The loop's own notifier handle; dropped with `tx` by `close_remote`.
    #[cfg(unix)]
    wake_tx: Option<Arc<wake::Notifier>>,
    #[cfg(unix)]
    sleeper: wake::Sleeper,
    /// Remote tasks received from `rx`, in posting order, awaiting their turn.
    remote: VecDeque<Stamped<S>>,
    remote_open: bool,
    /// A descriptor the loop also waits on while it sleeps (see [`FdWatch`]).
    fd_watch: Option<Rc<FdWatch>>,
}

/// A descriptor (e.g. a client socket) the loop waits on next to its own wake
/// pipe, so the owner can do its I/O on the loop's thread without a thread of
/// its own. The owner sets which readiness it wants; when the loop wakes for
/// it, [`FdWatch::take_ready`] reports so. Interest must be dropped while the
/// owner will not act on it, or a ready descriptor keeps the loop awake.
#[derive(Debug)]
pub struct FdWatch {
    #[cfg_attr(not(unix), allow(dead_code))]
    fd: i32,
    read: Cell<bool>,
    write: Cell<bool>,
    ready: Cell<bool>,
}

impl FdWatch {
    pub fn new(fd: i32) -> Rc<FdWatch> {
        Rc::new(FdWatch { fd, read: Cell::new(false), write: Cell::new(false), ready: Cell::new(false) })
    }

    pub fn set_interest(&self, read: bool, write: bool) {
        self.read.set(read);
        self.write.set(write);
    }

    /// Whether the loop woke for this descriptor since the last call.
    pub fn take_ready(&self) -> bool {
        self.ready.replace(false)
    }
}

impl<S: LoopState + 'static> Default for EventLoop<S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S: LoopState + 'static> EventLoop<S> {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        #[cfg(unix)]
        let (sleeper, wake_tx) = wake::pair().expect("event loop wake pipe");
        EventLoop {
            #[cfg(unix)]
            wake_tx: Some(wake_tx),
            #[cfg(unix)]
            sleeper,
            sched: Scheduler {
                inner: Rc::new(Inner {
                    tasks: RefCell::new(VecDeque::new()),
                    heap: RefCell::new(BinaryHeap::new()),
                    timers: RefCell::new(HashMap::new()),
                    next_timer: Cell::new(1),
                    seq: Cell::new(0),
                    stop: Cell::new(false),
                    keepalive: Cell::new(0),
                }),
            },
            tx: Some(tx),
            rx,
            remote: VecDeque::new(),
            remote_open: true,
            fd_watch: None,
        }
    }

    /// Also waits on `watch`'s descriptor while sleeping (`None` stops).
    pub fn watch_fd(&mut self, watch: Option<Rc<FdWatch>>) {
        self.fd_watch = watch;
    }

    pub fn scheduler(&self) -> Scheduler<S> {
        self.sched.clone()
    }

    /// A new cross-thread handle. Handles created before
    /// [`EventLoop::close_remote`] keep working until dropped.
    pub fn handle(&self) -> LoopHandle<S> {
        LoopHandle {
            tx: self.tx.clone().expect("remote posting closed"),
            #[cfg(unix)]
            wake: self.wake_tx.clone().expect("remote posting closed"),
        }
    }

    /// Drops the loop's own sender so `run_until_idle` can finish once all
    /// external handles are gone.
    pub fn close_remote(&mut self) {
        self.tx = None;
        #[cfg(unix)]
        {
            self.wake_tx = None;
        }
    }

    /// Runs one ready task or due timer. Returns false if nothing was ready.
    pub fn run_one(&mut self, state: &mut S) -> bool {
        self.pull_remote();
        // Remote tasks (posted from other threads: network completions, CDP
        // deliveries), local tasks and due timers run in readiness order:
        // remote post time, local queue time, timer due time. Each source
        // stays FIFO. On equal instants: timer, then local, then remote.
        // Page work that became ready before a remote message was posted runs
        // first; anything posted after it runs after it, so no source can
        // starve another.
        let remote = self.remote.front().map(|(at, _)| *at);
        let local = self.sched.inner.tasks.borrow().front().map(|(at, _, _)| *at);
        let timer = self.next_due_timer();
        let earliest_page = match (local, timer) {
            (Some(l), Some(t)) => Some(l.min(t)),
            (l, t) => l.or(t),
        };
        if let Some(r) = remote {
            if earliest_page.is_none_or(|p| r < p) {
                let (_, task) = self.remote.pop_front().expect("remote front");
                task(state);
                state.after_task();
                return true;
            }
        }
        if let (Some(l), Some(t)) = (local, timer) {
            if t <= l {
                return self.run_due_timer(state);
            }
        }
        if local.is_some() {
            let next = self.sched.inner.tasks.borrow_mut().pop_front();
            if let Some((_, _, task)) = next {
                task(state);
                state.after_task();
                return true;
            }
        }
        self.run_due_timer(state)
    }

    /// Moves every remote task already posted into the ordered remote queue.
    fn pull_remote(&mut self) {
        loop {
            match self.rx.try_recv() {
                Ok(task) => self.remote.push_back(task),
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.remote_open = false;
                    return;
                }
                Err(mpsc::TryRecvError::Empty) => return,
            }
        }
    }

    /// Deadline of the earliest live timer if it is already due.
    fn next_due_timer(&self) -> Option<Instant> {
        self.sched.inner.clean_heap_top();
        let at = self.sched.inner.heap.borrow().peek().map(|r| r.0 .0)?;
        (at <= Instant::now()).then_some(at)
    }

    fn run_due_timer(&mut self, state: &mut S) -> bool {
        let now = Instant::now();
        loop {
            self.sched.inner.clean_heap_top();
            let top = self.sched.inner.heap.borrow().peek().map(|r| r.0);
            let Some((at, _, id)) = top else { return false };
            if at > now {
                return false;
            }
            self.sched.inner.heap.borrow_mut().pop();
            let timer = {
                let timers = self.sched.inner.timers.borrow();
                timers.get(&id).map(|t| (t.task.clone(), t.interval))
            };
            let Some((task, interval)) = timer else { continue }; // cleared
            match interval {
                Some(d) => self.sched.push_deadline(now + d.max(Duration::from_millis(1)), id),
                None => {
                    self.sched.inner.remove_timer(id);
                }
            }
            task(state);
            state.after_task();
            return true;
        }
    }

    /// Blocks until `wait` elapses, a remote task is posted, or every remote
    /// sender is gone. Returns false once no sender is left.
    #[cfg(unix)]
    fn sleep(&mut self, wait: Option<Duration>) -> bool {
        // Posters write to the wake pipe only while the loop is announced as
        // sleeping; a task posted before the announcement is caught here.
        self.sleeper.announce(true);
        let senders_left = match self.rx.try_recv() {
            Ok(task) => {
                self.remote.push_back(task);
                true
            }
            // No sender left: the sleeper still waits out `wait` (as before).
            Err(_) => self.sleeper.sleep(wait, self.fd_watch.as_deref()),
        };
        self.sleeper.announce(false);
        senders_left
    }

    #[cfg(not(unix))]
    fn sleep(&mut self, wait: Option<Duration>) -> bool {
        let msg = match wait {
            Some(w) => self.rx.recv_timeout(w).map_err(|e| matches!(e, mpsc::RecvTimeoutError::Disconnected)),
            None => self.rx.recv().map_err(|_| true),
        };
        match msg {
            Ok(task) => {
                self.remote.push_back(task);
                true
            }
            Err(disconnected) => !disconnected,
        }
    }

    fn next_deadline(&self) -> Option<Instant> {
        self.sched.inner.clean_heap_top();
        self.sched.inner.heap.borrow().peek().map(|r| r.0 .0)
    }

    fn has_keepalive_work(&self) -> bool {
        !self.remote.is_empty() || !self.sched.inner.tasks.borrow().is_empty() || self.sched.active_timers() > 0
    }

    /// Runs until `done(state)` is true, the deadline passes, the loop is
    /// stopped, or (when `stop_when_idle`) nothing can make progress.
    pub fn run_until(
        &mut self,
        state: &mut S,
        deadline: Option<Instant>,
        stop_when_idle: bool,
        mut done: impl FnMut(&mut S) -> bool,
    ) -> RunOutcome {
        loop {
            if done(state) {
                return RunOutcome::Done;
            }
            if self.sched.inner.stop.replace(false) {
                return RunOutcome::Stopped;
            }
            // Checked between tasks too: a busy queue cannot outrun the deadline.
            if deadline.is_some_and(|d| Instant::now() >= d) {
                return RunOutcome::TimedOut;
            }
            if self.run_one(state) {
                continue;
            }
            let now = Instant::now();
            // Idle mode does not wait for future remote messages: there is no
            // way to know whether any will come (queued ones ran in run_one).
            if stop_when_idle && !self.has_keepalive_work() {
                return RunOutcome::Idle;
            }
            let wake = [self.next_deadline(), deadline].into_iter().flatten().min();
            let wait = wake.map(|w| w.saturating_duration_since(now));
            if !self.remote_open {
                // No sender left: nothing can arrive; wait for the deadline.
                let Some(w) = wait else { return RunOutcome::Idle };
                self.sleep(Some(w));
                continue;
            }
            // Posted tasks are pulled by run_one on the next iteration, ordered
            // against page work (a timer may have become due before the post).
            let mut senders_left = self.sleep(wait);
            match self.rx.try_recv() {
                Ok(task) => self.remote.push_back(task),
                Err(mpsc::TryRecvError::Disconnected) => senders_left = false,
                Err(mpsc::TryRecvError::Empty) => {}
            }
            if !senders_left {
                // Every sender is gone (including ours after close_remote).
                self.remote_open = false;
                if wake.is_none() {
                    return RunOutcome::Idle;
                }
            }
        }
    }

    /// Runs until no task or keep-alive timer remains (does not wait for
    /// remote messages that are not queued yet) or the deadline passes.
    pub fn run_until_idle(&mut self, state: &mut S, deadline: Option<Instant>) -> RunOutcome {
        self.run_until(state, deadline, true, |_| false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thread_cpu() -> Duration {
        #[cfg(unix)]
        {
            let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
            // SAFETY: valid timespec out-pointer.
            unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) };
            Duration::new(ts.tv_sec as u64, ts.tv_nsec as u32)
        }
        #[cfg(not(unix))]
        Duration::ZERO
    }

    /// With every remote sender gone, the loop still waits for its deadline
    /// without spinning (the hung-up wake pipe must not be polled again).
    #[test]
    fn idle_wait_without_senders_sleeps_instead_of_spinning() {
        let mut lp = EventLoop::<St>::new();
        lp.close_remote();
        let mut st = St::default();
        let s = lp.scheduler();
        let fired = Rc::new(Cell::new(false));
        let f = fired.clone();
        s.set_timer(Duration::from_millis(60), false, true, move |_| f.set(true));
        let start = Instant::now();
        let cpu0 = thread_cpu();
        let r = lp.run_until(&mut st, Some(start + Duration::from_millis(500)), false, |_| fired.get());
        let cpu = thread_cpu() - cpu0;
        assert_eq!(r, RunOutcome::Done);
        assert!(start.elapsed() >= Duration::from_millis(60));
        assert!(start.elapsed() < Duration::from_millis(120), "late: {:?}", start.elapsed());
        assert!(cpu < Duration::from_millis(15), "spun for {cpu:?} of {:?}", start.elapsed());
        assert!(!lp.remote_open);
    }

    /// A remote post while the loop sleeps on a far deadline wakes it promptly
    /// and the task runs; nothing is lost when the post lands around the wait.
    #[test]
    fn remote_post_wakes_a_sleeping_loop_promptly() {
        let mut lp = EventLoop::<St>::new();
        let handle = lp.handle();
        let mut st = St::default();
        for round in 0..20 {
            let h = handle.clone();
            let delay = Duration::from_micros(200 * round);
            std::thread::spawn(move || {
                std::thread::sleep(delay);
                h.post(|st| st.log.push("remote".into())).unwrap();
            });
            let start = Instant::now();
            let before = st.log.len();
            let r = lp.run_until(&mut st, Some(start + Duration::from_secs(5)), false, |st| st.log.len() > before);
            assert_eq!(r, RunOutcome::Done);
            assert!(start.elapsed() < Duration::from_millis(100), "round {round}: {:?}", start.elapsed());
        }
        assert_eq!(st.log.len(), 20);
    }

    /// Posters skip the wake pipe while the loop is awake. Racing posts from
    /// another thread, with and without pauses that let the loop fall asleep,
    /// must never be missed: each one runs, in order, well before the deadline.
    #[test]
    fn posts_racing_the_loop_falling_asleep_are_never_missed() {
        let mut lp = EventLoop::<St>::new();
        let handle = lp.handle();
        let mut st = St::default();
        const N: u32 = 20_000;
        let poster = std::thread::spawn(move || {
            for i in 0..N {
                handle.post(move |st| st.log.push(i.to_string())).unwrap();
                match i % 97 {
                    0 => std::thread::sleep(Duration::from_micros(200)),
                    1..=3 => std::thread::yield_now(),
                    _ => {}
                }
            }
        });
        let r = lp.run_until(&mut st, Some(Instant::now() + Duration::from_secs(20)), false, |st| st.log.len() == N as usize);
        poster.join().unwrap();
        assert_eq!(r, RunOutcome::Done, "lost a wake-up after {} tasks", st.log.len());
        assert!(st.log.iter().enumerate().all(|(i, v)| v == &i.to_string()));
    }

    /// A watched descriptor wakes the sleeping loop when it becomes readable;
    /// without interest a ready descriptor does not keep the loop awake.
    #[cfg(unix)]
    #[test]
    fn watched_descriptor_wakes_the_loop_only_with_interest() {
        use std::io::Write;
        use std::os::fd::AsRawFd;
        let (mut tx, rx) = std::os::unix::net::UnixStream::pair().unwrap();
        let mut lp = EventLoop::<St>::new();
        let _handle = lp.handle();
        let mut st = St::default();
        let watch = FdWatch::new(rx.as_raw_fd());
        watch.set_interest(true, false);
        lp.watch_fd(Some(watch.clone()));
        let writer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            tx.write_all(b"x").unwrap();
            tx
        });
        let start = Instant::now();
        let r = lp.run_until(&mut st, Some(Instant::now() + Duration::from_secs(5)), false, |_| watch.take_ready());
        assert_eq!(r, RunOutcome::Done);
        assert!(start.elapsed() < Duration::from_secs(1), "woke after {:?}", start.elapsed());
        let _tx = writer.join().unwrap();
        // Still readable (nothing read), but no interest: the loop sleeps until the deadline.
        watch.set_interest(false, false);
        let cpu = thread_cpu();
        let r = lp.run_until(&mut st, Some(Instant::now() + Duration::from_millis(100)), false, |_| watch.take_ready());
        assert_eq!(r, RunOutcome::TimedOut);
        assert!(thread_cpu() - cpu < Duration::from_millis(30), "spun while the descriptor stayed ready");
    }

    /// Many posts without the loop running fill the wake pipe; every task still
    /// runs once, in order, and the loop does not block on the full pipe.
    #[test]
    fn posts_beyond_the_wake_pipe_capacity_all_run_in_order() {
        let mut lp = EventLoop::<St>::new();
        let handle = lp.handle();
        let mut st = St::default();
        for i in 0..70_000u32 {
            handle.post(move |st| st.log.push(i.to_string())).unwrap();
        }
        let r = lp.run_until(&mut st, Some(Instant::now() + Duration::from_secs(10)), false, |st| st.log.len() == 70_000);
        assert_eq!(r, RunOutcome::Done);
        assert!(st.log.iter().enumerate().all(|(i, v)| v == &i.to_string()));
        // The drained pipe still wakes for later posts.
        handle.post(|st| st.log.push("after".into())).unwrap();
        let r = lp.run_until(&mut st, Some(Instant::now() + Duration::from_secs(5)), false, |st| st.log.last().is_some_and(|v| v == "after"));
        assert_eq!(r, RunOutcome::Done);
    }

    /// Dropping the last handle while the loop sleeps with no deadline and no
    /// work ends the remote phase promptly with Idle (as recv() returning
    /// Disconnected did); with a deadline the loop keeps waiting for it.
    #[test]
    fn dropping_every_sender_during_the_wait_is_noticed() {
        let mut lp = EventLoop::<St>::new();
        let handle = lp.handle();
        lp.close_remote();
        let mut st = St::default();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            drop(handle);
        });
        let start = Instant::now();
        let r = lp.run_until(&mut st, None, false, |_| false);
        assert_eq!(r, RunOutcome::Idle);
        assert!(start.elapsed() < Duration::from_millis(500), "{:?}", start.elapsed());
        assert!(!lp.remote_open);
        // With a deadline and no senders, the loop waits (without the pipe) until it.
        let start = Instant::now();
        let r = lp.run_until(&mut st, Some(start + Duration::from_millis(40)), false, |_| false);
        assert_eq!(r, RunOutcome::TimedOut);
        assert!(start.elapsed() >= Duration::from_millis(40) && start.elapsed() < Duration::from_millis(100), "{:?}", start.elapsed());
    }

    /// Posting after the loop (and its wake pipe read end) is gone fails
    /// cleanly: no SIGPIPE, no panic, LoopClosed from the channel.
    #[test]
    fn post_after_the_loop_is_dropped_fails_cleanly() {
        let lp = EventLoop::<St>::new();
        let handle = lp.handle();
        drop(lp);
        assert!(handle.post(|_| {}).is_err());
        // Direct notifier write with the read end closed: EPIPE path, no signal.
        #[cfg(unix)]
        handle.wake.notify();
    }

    /// A delayed timer fires close to its deadline while the loop sleeps
    /// (macOS coalesces std sleeps by ~100-140 ms; the loop waits in poll).
    #[test]
    fn short_timers_fire_close_to_their_deadline_while_idle() {
        let mut lp = EventLoop::<St>::new();
        let _handle = lp.handle(); // a live remote sender: the loop sleeps in its wake wait, not in idle exit
        let mut st = St::default();
        let s = lp.scheduler();
        let mut lateness = Vec::new();
        for _ in 0..5 {
            let start = Instant::now();
            let fired = Rc::new(Cell::new(false));
            let f = fired.clone();
            s.set_timer(Duration::from_millis(16), false, true, move |_| f.set(true));
            let r = lp.run_until(&mut st, Some(start + Duration::from_millis(500)), false, |_| fired.get());
            assert_eq!(r, RunOutcome::Done);
            lateness.push(start.elapsed().saturating_sub(Duration::from_millis(16)));
        }
        lateness.sort();
        assert!(lateness[2] < Duration::from_millis(40), "median lateness {:?} (all {:?})", lateness[2], lateness);
    }

    #[derive(Default)]
    struct St {
        log: Vec<String>,
        checkpoints: u32,
    }
    impl LoopState for St {
        fn after_task(&mut self) {
            self.checkpoints += 1;
        }
    }

    #[test]
    fn tasks_then_timers_in_deadline_order() {
        let mut lp = EventLoop::<St>::new();
        let s = lp.scheduler();
        // Queued before any timer is registered, so it is ready before every
        // deadline regardless of scheduling pauses (readiness order).
        s.post(TaskSource::DomManipulation, |st| st.log.push("task".into()));
        // Registered shortest first: each later deadline is at least as late
        // as the earlier ones however long the gap between registrations.
        s.set_timer(Duration::from_millis(5), false, true, |st| st.log.push("t5".into()));
        s.set_timer(Duration::from_millis(20), false, true, |st| st.log.push("t20".into()));
        let cleared = s.set_timer(Duration::from_millis(1), false, true, |st| st.log.push("never".into()));
        assert!(s.clear_timer(cleared));
        let mut st = St::default();
        let r = lp.run_until_idle(&mut st, Some(Instant::now() + Duration::from_secs(2)));
        assert_eq!(r, RunOutcome::Idle);
        assert_eq!(st.log, ["task", "t5", "t20"]);
        assert_eq!(st.checkpoints, 3);
    }

    #[test]
    fn interval_repeats_until_cleared() {
        let mut lp = EventLoop::<St>::new();
        let s = lp.scheduler();
        let s2 = s.clone();
        let id = Rc::new(Cell::new(TimerId(0)));
        let id2 = id.clone();
        id.set(s.set_timer(Duration::from_millis(2), true, true, move |st| {
            st.log.push("i".into());
            if st.log.len() == 3 {
                s2.clear_timer(id2.get());
            }
        }));
        let mut st = St::default();
        lp.run_until_idle(&mut st, Some(Instant::now() + Duration::from_secs(2)));
        assert_eq!(st.log.len(), 3);
    }

    #[test]
    fn remote_posts_wake_the_loop_and_fail_after_exit() {
        let mut lp = EventLoop::<St>::new();
        let h = lp.handle();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            h.post(|st| st.log.push("remote".into())).unwrap();
            h
        });
        let mut st = St::default();
        let r = lp.run_until(&mut st, Some(Instant::now() + Duration::from_secs(5)), false, |st| !st.log.is_empty());
        assert_eq!(r, RunOutcome::Done);
        assert_eq!(st.log, ["remote"]);
        let h = t.join().unwrap();
        drop(lp);
        assert_eq!(h.post(|_| {}), Err(LoopClosed));
    }

    #[test]
    fn busy_queue_respects_deadline() {
        let mut lp = EventLoop::<St>::new();
        let s = lp.scheduler();
        fn repost(s: Scheduler<St>) {
            let s2 = s.clone();
            s.post(TaskSource::Internal, move |_| repost(s2));
        }
        repost(s.clone());
        let mut st = St::default();
        let start = Instant::now();
        let r = lp.run_until(&mut st, Some(start + Duration::from_millis(30)), false, |_| false);
        assert_eq!(r, RunOutcome::TimedOut);
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn cleared_timers_do_not_accumulate() {
        let lp = EventLoop::<St>::new();
        let s = lp.scheduler();
        for _ in 0..10_000 {
            let id = s.set_timer(Duration::from_secs(3600), false, true, |_| {});
            s.clear_timer(id);
        }
        assert!(lp.sched.inner.heap.borrow().len() <= 65, "heap {}", lp.sched.inner.heap.borrow().len());
        assert_eq!(s.active_timers(), 0);
        assert_eq!(lp.next_deadline(), None);
    }

    /// Posts a task that logs `name{n}` and re-posts itself until `n == last`
    /// (the shape of a MessageChannel scheduler loop that keeps working).
    fn chain(s: &Scheduler<St>, n: u32, last: u32) {
        let s2 = s.clone();
        s.post(TaskSource::DomManipulation, move |st| {
            st.log.push(format!("task{n}"));
            if n < last {
                chain(&s2, n + 1, last);
            }
        });
    }

    #[test]
    fn self_reposting_tasks_do_not_starve_due_timers() {
        let mut lp = EventLoop::<St>::new();
        let s = lp.scheduler();
        // A due timer registered before the chain is ready before every task.
        s.set_timer(Duration::ZERO, false, true, |st| st.log.push("timer".into()));
        chain(&s, 1, 20);
        let mut st = St::default();
        lp.run_until_idle(&mut st, Some(Instant::now() + Duration::from_secs(5)));
        assert_eq!(st.log.len(), 21);
        assert_eq!(st.log[0], "timer", "{:?}", st.log);
    }

    #[test]
    fn timer_registered_during_a_chain_runs_before_tasks_queued_after_it() {
        let mut lp = EventLoop::<St>::new();
        let s = lp.scheduler();
        let s2 = s.clone();
        s.post(TaskSource::DomManipulation, move |st| {
            st.log.push("task1".into());
            // Queued before the timer: runs first.
            chain(&s2, 2, 2);
            // Strictly separate the readiness instants (ties favor the timer).
            std::thread::sleep(Duration::from_micros(50));
            s2.set_timer(Duration::ZERO, false, true, |st| st.log.push("timer".into()));
            std::thread::sleep(Duration::from_micros(50));
            // Queued after the timer became ready: runs after it.
            chain(&s2, 3, 5);
        });
        let mut st = St::default();
        lp.run_until_idle(&mut st, Some(Instant::now() + Duration::from_secs(5)));
        assert_eq!(st.log, ["task1", "task2", "timer", "task3", "task4", "task5"]);
    }

    #[test]
    fn future_and_cleared_timers_do_not_preempt_queued_tasks() {
        let mut lp = EventLoop::<St>::new();
        let s = lp.scheduler();
        let later = s.set_timer(Duration::from_secs(60), false, true, |st| st.log.push("later".into()));
        let cleared = s.set_timer(Duration::ZERO, false, true, |st| st.log.push("cleared".into()));
        assert!(s.clear_timer(cleared));
        chain(&s, 1, 3);
        let mut st = St::default();
        let r = lp.run_until(&mut st, Some(Instant::now() + Duration::from_secs(5)), false, |st| st.log.len() == 3);
        assert_eq!(r, RunOutcome::Done);
        assert_eq!(st.log, ["task1", "task2", "task3"]);
        assert!(s.clear_timer(later));
    }

    #[test]
    fn interval_ticks_interleave_with_a_busy_chain() {
        let mut lp = EventLoop::<St>::new();
        let s = lp.scheduler();
        let s2 = s.clone();
        let id = Rc::new(Cell::new(TimerId(0)));
        let id2 = id.clone();
        id.set(s.set_timer(Duration::from_millis(1), true, true, move |st| {
            st.log.push("tick".into());
            if st.log.iter().filter(|l| *l == "tick").count() == 3 {
                s2.clear_timer(id2.get());
            }
        }));
        // Each task busy-waits 2 ms, so interval deadlines keep coming due.
        fn busy(s: &Scheduler<St>, n: u32) {
            let s2 = s.clone();
            s.post(TaskSource::DomManipulation, move |st| {
                let start = Instant::now();
                while start.elapsed() < Duration::from_millis(2) {}
                st.log.push(format!("task{n}"));
                if n < 10 {
                    busy(&s2, n + 1);
                }
            });
        }
        busy(&s, 1);
        let mut st = St::default();
        lp.run_until_idle(&mut st, Some(Instant::now() + Duration::from_secs(5)));
        let first_tick = st.log.iter().position(|l| l == "tick").unwrap();
        let last_task = st.log.iter().rposition(|l| l.starts_with("task")).unwrap();
        assert!(first_tick < last_task, "ticks starved behind the chain: {:?}", st.log);
        assert_eq!(st.log.iter().filter(|l| *l == "tick").count(), 3);
    }

    #[test]
    fn remote_posted_while_a_task_runs_waits_for_page_work_ready_before_it() {
        let mut lp = EventLoop::<St>::new();
        let s = lp.scheduler();
        let h = lp.handle();
        let s2 = s.clone();
        s.post(TaskSource::DomManipulation, move |st| {
            st.log.push("task".into());
            // Due-vs-CDP shape: a zero-delay timer, then a message from another
            // thread (a CDP command) arrives while this task is still running.
            s2.set_timer(Duration::ZERO, false, true, |st| st.log.push("timer".into()));
            std::thread::sleep(Duration::from_micros(50));
            let h = h.clone();
            std::thread::spawn(move || h.post(|st| st.log.push("remote".into())).unwrap()).join().unwrap();
            std::thread::sleep(Duration::from_micros(50));
            // Queued after the remote was posted: runs after it.
            chain(&s2, 1, 1);
        });
        let mut st = St::default();
        lp.run_until_idle(&mut st, Some(Instant::now() + Duration::from_secs(5)));
        assert_eq!(st.log, ["task", "timer", "remote", "task1"]);
        assert_eq!(st.checkpoints, 4);
    }

    #[test]
    fn remote_posted_before_page_work_runs_first_in_posting_order() {
        let mut lp = EventLoop::<St>::new();
        let s = lp.scheduler();
        let h = lp.handle();
        h.post(|st| st.log.push("remote1".into())).unwrap();
        h.post(|st| st.log.push("remote2".into())).unwrap();
        std::thread::sleep(Duration::from_micros(50));
        s.set_timer(Duration::ZERO, false, true, |st| st.log.push("timer".into()));
        chain(&s, 1, 1);
        let mut st = St::default();
        lp.run_until_idle(&mut st, Some(Instant::now() + Duration::from_secs(5)));
        assert_eq!(st.log, ["remote1", "remote2", "timer", "task1"]);
    }

    #[test]
    fn self_reposting_page_work_does_not_starve_remote_messages() {
        let mut lp = EventLoop::<St>::new();
        let s = lp.scheduler();
        let h = lp.handle();
        // An endless chain (a MessageChannel scheduler that never yields) and
        // an endless zero-delay timer loop: a remote message posted meanwhile
        // runs once the work that was ready before it is done.
        fn forever(s: &Scheduler<St>, h: LoopHandle<St>, n: u32) {
            let s2 = s.clone();
            s.post(TaskSource::DomManipulation, move |st| {
                st.log.push(format!("task{n}"));
                let h2 = h.clone();
                forever(&s2, h, n + 1);
                if n == 3 {
                    std::thread::sleep(Duration::from_micros(50));
                    h2.post(|st| st.log.push("remote".into())).unwrap();
                }
            });
        }
        fn ticks(s: &Scheduler<St>) {
            let s2 = s.clone();
            s.set_timer(Duration::ZERO, false, true, move |_| ticks(&s2));
        }
        forever(&s, lp.handle(), 1);
        ticks(&s);
        drop(h);
        let mut st = St::default();
        let r = lp.run_until(&mut st, Some(Instant::now() + Duration::from_secs(5)), false, |st| {
            st.log.iter().any(|l| l == "remote")
        });
        assert_eq!(r, RunOutcome::Done);
        // task4 was queued before the remote was posted, task5 after it; the
        // timer loop's deadlines before the post ran in between.
        assert_eq!(st.log, ["task1", "task2", "task3", "task4", "remote"]);
    }

    #[test]
    fn remote_and_local_chains_interleave_by_readiness() {
        let mut lp = EventLoop::<St>::new();
        let s = lp.scheduler();
        // Each task waits before re-posting, so every re-post is strictly
        // later than the other chain's pending entry.
        fn remote(h: LoopHandle<St>, n: u32) {
            let h2 = h.clone();
            h.post(move |st| {
                st.log.push(format!("r{n}"));
                std::thread::sleep(Duration::from_micros(50));
                if n < 3 {
                    remote(h2, n + 1);
                }
            })
            .unwrap();
        }
        fn local(s: &Scheduler<St>, n: u32) {
            let s2 = s.clone();
            s.post(TaskSource::DomManipulation, move |st| {
                st.log.push(format!("l{n}"));
                std::thread::sleep(Duration::from_micros(50));
                if n < 3 {
                    local(&s2, n + 1);
                }
            });
        }
        local(&s, 1);
        std::thread::sleep(Duration::from_micros(50));
        remote(lp.handle(), 1);
        let mut st = St::default();
        lp.run_until(&mut st, Some(Instant::now() + Duration::from_secs(5)), false, |st| st.log.len() == 6);
        assert_eq!(st.log, ["l1", "r1", "l2", "r2", "l3", "r3"]);
    }

    #[test]
    fn deadline_and_stop() {
        let mut lp = EventLoop::<St>::new();
        let s = lp.scheduler();
        s.set_timer(Duration::from_secs(60), false, true, |_| {});
        let mut st = St::default();
        let start = Instant::now();
        let r = lp.run_until(&mut st, Some(start + Duration::from_millis(30)), true, |_| false);
        assert_eq!(r, RunOutcome::TimedOut);
        assert!(start.elapsed() < Duration::from_secs(5));
        let s2 = s.clone();
        s.post(TaskSource::Internal, move |_| s2.stop());
        assert_eq!(lp.run_until(&mut st, None, false, |_| false), RunOutcome::Stopped);
    }
}
