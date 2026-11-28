//! Prioritized Task Scheduling queues. A fixed priority or each mutable
//! TaskSignal has a FIFO task queue and a FIFO continuation queue
//! (`scheduler.yield()`) on the event loop. A continuation outranks tasks of
//! its own priority and is outranked by tasks of a higher one.
use super::{Cell, HashMap, Instant, Rc, RefCell, Scheduler, Task, VecDeque};
use std::time::Duration;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum WebTaskPriority {
    Background,
    UserVisible,
    UserBlocking,
}

#[derive(Clone)]
pub enum WebPrioritySource {
    Fixed(WebTaskPriority),
    Signal(Rc<Cell<WebTaskPriority>>),
}

#[derive(Clone, Copy, Hash, Eq, PartialEq)]
enum SourceKey {
    Fixed(WebTaskPriority),
    Signal(usize),
}

/// A source's task queue or its continuation queue.
type QueueKey = (SourceKey, bool);

impl WebPrioritySource {
    fn key(&self) -> SourceKey {
        match self {
            Self::Fixed(p) => SourceKey::Fixed(*p),
            Self::Signal(cell) => SourceKey::Signal(Rc::as_ptr(cell) as usize),
        }
    }

    fn priority(&self) -> WebTaskPriority {
        match self {
            Self::Fixed(p) => *p,
            Self::Signal(cell) => cell.get(),
        }
    }
}

struct Entry<S> {
    id: u64,
    ready: Instant,
    order: u64,
    task: Task<S>,
}
struct Queue<S> {
    source: WebPrioritySource,
    continuation: bool,
    entries: VecDeque<Entry<S>>,
}
pub(super) struct WebQueues<S> {
    queues: HashMap<QueueKey, Queue<S>>,
    next_order: u64,
}

impl<S> Default for WebQueues<S> {
    fn default() -> Self {
        Self { queues: HashMap::new(), next_order: 0 }
    }
}

impl<S> WebQueues<S> {
    fn push(&mut self, source: WebPrioritySource, continuation: bool, id: u64, task: Task<S>) {
        let order = self.next_order;
        self.next_order = order.wrapping_add(1);
        self.queues
            .entry((source.key(), continuation))
            .or_insert_with(|| Queue { source, continuation, entries: VecDeque::new() })
            .entries
            .push_back(Entry { id, ready: Instant::now(), order, task });
    }

    fn remove(&mut self, key: QueueKey, id: u64) {
        if let Some(q) = self.queues.get_mut(&key) {
            if let Some(i) = q.entries.iter().position(|j| j.id == id) {
                q.entries.remove(i);
            }
            if q.entries.is_empty() {
                self.queues.remove(&key);
            }
        }
    }

    pub(super) fn len(&self) -> usize {
        self.queues.values().map(|q| q.entries.len()).sum()
    }
    pub(super) fn is_empty(&self) -> bool {
        self.queues.is_empty()
    }

    /// Compare the actual selected entry's readiness against ordinary
    /// sources, so a stream of new high-priority jobs cannot jump ahead of
    /// an already ready timer, network completion, or CDP task. A
    /// continuation of user-visible or higher priority resumes the task that
    /// yielded before ordinary work (Chrome: `yield()` from a timer resumes
    /// before the next ready timer).
    pub(super) fn take_ready(&mut self, ordinary: Option<Instant>) -> Option<Task<S>> {
        let (&key, best) = self.queues.iter().max_by_key(|(_, q)| {
            (q.source.priority(), q.continuation, std::cmp::Reverse(q.entries.front().expect("nonempty web queue").order))
        })?;
        let resumes = best.continuation && best.source.priority() >= WebTaskPriority::UserVisible;
        if !resumes && ordinary.is_some_and(|at| at <= best.entries.front().expect("nonempty web queue").ready) {
            return None;
        }
        let q = self.queues.get_mut(&key)?;
        let task = q.entries.pop_front()?.task;
        if q.entries.is_empty() {
            self.queues.remove(&key);
        }
        Some(task)
    }
}

impl<S: 'static> Scheduler<S> {
    /// Returns cancellation of both a delayed admission timer and a ready task.
    pub fn post_web_task(&self, source: WebPrioritySource, delay: Duration, task: Task<S>) -> Box<dyn FnOnce()> {
        self.post_web(source, false, delay, task)
    }

    /// Queues a `scheduler.yield()` continuation; returns its cancellation.
    pub fn post_web_continuation(&self, source: WebPrioritySource, task: Task<S>) -> Box<dyn FnOnce()> {
        self.post_web(source, true, Duration::ZERO, task)
    }

    fn post_web(&self, source: WebPrioritySource, continuation: bool, delay: Duration, task: Task<S>) -> Box<dyn FnOnce()> {
        if self.inner.virtual_now.get().unwrap_or_else(Instant::now).checked_add(delay).is_none() {
            // A valid WebIDL duration beyond this platform's clock range
            // cannot become ready. The owner can still abort the task.
            return Box::new(|| {});
        }
        let task = super::attribute_once::<S>(task);
        let key = (source.key(), continuation);
        let id = self.inner.seq.get();
        self.inner.seq.set(id.wrapping_add(1));
        let weak = Rc::downgrade(&self.inner);
        let timer = if delay.is_zero() {
            self.inner.scheduled.set(self.inner.scheduled.get() + 1);
            self.inner.web.borrow_mut().push(source, continuation, id, task);
            None
        } else {
            let inner = weak.clone();
            let task = RefCell::new(Some(task));
            Some(self.set_timer(delay, false, true, move |_| {
                if let (Some(inner), Some(task)) = (inner.upgrade(), task.take()) {
                    inner.web.borrow_mut().push(source.clone(), continuation, id, task);
                }
            }))
        };
        Box::new(move || {
            if let Some(inner) = weak.upgrade() {
                if let Some(timer) = timer {
                    inner.remove_timer(timer);
                    inner.maybe_compact();
                }
                inner.web.borrow_mut().remove(key, id);
            }
        })
    }
}
