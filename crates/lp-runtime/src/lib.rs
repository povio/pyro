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
use std::time::{Duration, Instant};

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

/// Cross-thread posting handle. Tasks are delivered in posting order.
pub struct LoopHandle<S> {
    tx: mpsc::Sender<RemoteTask<S>>,
}

impl<S> Clone for LoopHandle<S> {
    fn clone(&self) -> Self {
        LoopHandle { tx: self.tx.clone() }
    }
}

impl<S> LoopHandle<S> {
    pub fn post(&self, task: impl FnOnce(&mut S) + Send + 'static) -> Result<(), LoopClosed> {
        self.tx.send(Box::new(task)).map_err(|_| LoopClosed)
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
    tx: Option<mpsc::Sender<RemoteTask<S>>>,
    rx: mpsc::Receiver<RemoteTask<S>>,
    remote_open: bool,
}

impl<S: LoopState + 'static> Default for EventLoop<S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S: LoopState + 'static> EventLoop<S> {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        EventLoop {
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
            remote_open: true,
        }
    }

    pub fn scheduler(&self) -> Scheduler<S> {
        self.sched.clone()
    }

    /// A new cross-thread handle. Handles created before
    /// [`EventLoop::close_remote`] keep working until dropped.
    pub fn handle(&self) -> LoopHandle<S> {
        LoopHandle { tx: self.tx.clone().expect("remote posting closed") }
    }

    /// Drops the loop's own sender so `run_until_idle` can finish once all
    /// external handles are gone.
    pub fn close_remote(&mut self) {
        self.tx = None;
    }

    /// Runs one ready task or due timer. Returns false if nothing was ready.
    pub fn run_one(&mut self, state: &mut S) -> bool {
        // Remote messages first: they are external events in arrival order.
        match self.rx.try_recv() {
            Ok(task) => {
                task(state);
                state.after_task();
                return true;
            }
            Err(mpsc::TryRecvError::Disconnected) => self.remote_open = false,
            Err(mpsc::TryRecvError::Empty) => {}
        }
        // Local tasks and due timers run in readiness order (task queue time
        // vs timer due time), like a browser's equal-priority task sources.
        // Never letting timers run while local tasks are queued starved them
        // behind self-reposting work (e.g. a MessageChannel scheduler loop).
        let task_ready = self.sched.inner.tasks.borrow().front().map(|(at, _, _)| *at);
        if let Some(task_ready) = task_ready {
            if self.next_due_timer().is_some_and(|due| due <= task_ready) {
                return self.run_due_timer(state);
            }
            let next = self.sched.inner.tasks.borrow_mut().pop_front();
            if let Some((_, _, task)) = next {
                task(state);
                state.after_task();
                return true;
            }
        }
        self.run_due_timer(state)
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

    fn next_deadline(&self) -> Option<Instant> {
        self.sched.inner.clean_heap_top();
        self.sched.inner.heap.borrow().peek().map(|r| r.0 .0)
    }

    fn has_keepalive_work(&self) -> bool {
        !self.sched.inner.tasks.borrow().is_empty() || self.sched.active_timers() > 0
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
                // No sender left: recv would return immediately; just sleep.
                match wait {
                    Some(w) => std::thread::sleep(w),
                    None => return RunOutcome::Idle,
                }
                continue;
            }
            let msg = match wait {
                Some(w) => self.rx.recv_timeout(w).map_err(|e| matches!(e, mpsc::RecvTimeoutError::Disconnected)),
                None => self.rx.recv().map_err(|_| true),
            };
            match msg {
                Ok(task) => {
                    task(state);
                    state.after_task();
                }
                Err(true) => {
                    // Every sender is gone (including ours after close_remote).
                    self.remote_open = false;
                    if wake.is_none() {
                        return RunOutcome::Idle;
                    }
                }
                Err(false) => {}
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
        s.set_timer(Duration::from_millis(20), false, true, |st| st.log.push("t20".into()));
        s.set_timer(Duration::from_millis(5), false, true, |st| st.log.push("t5".into()));
        let cleared = s.set_timer(Duration::from_millis(1), false, true, |st| st.log.push("never".into()));
        assert!(s.clear_timer(cleared));
        s.post(TaskSource::DomManipulation, |st| st.log.push("task".into()));
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
            s2.set_timer(Duration::ZERO, false, true, |st| st.log.push("timer".into()));
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
