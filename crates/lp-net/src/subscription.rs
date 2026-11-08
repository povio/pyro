//! Bounded, nonblocking Network event observations. Overflow terminates the
//! subscription explicitly; it never leaves a partial lifecycle looking valid.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use crate::{NetEvent, ResponseHead};

pub type NetWake = Arc<dyn Fn() + Send + Sync>;

thread_local! {
    /// On an I/O thread ([`DeferredWakes`]): the observers whose wake is due
    /// at the end of the thread's turn. `None` elsewhere: wakes run at once.
    static DEFERRED: RefCell<Option<Vec<Arc<Observer>>>> = const { RefCell::new(None) };
}

/// While it lives, this thread's observer wakes are held until
/// [`flush_deferred_wakes`] (the I/O thread runs it before parking and after
/// each request task): a small response's events (head, body, finished) wake
/// the consumer once instead of once each, and a terminal event's wake lands
/// right after the completion the request's sink just posted, so the
/// consumer's loop is already awake for it. Order per subscription is
/// unchanged (the queue's). Dropping it flushes.
pub(crate) struct DeferredWakes(());

impl DeferredWakes {
    pub(crate) fn enter() -> Self {
        DEFERRED.with(|d| {
            let mut d = d.borrow_mut();
            debug_assert!(d.is_none(), "deferred wakes already entered on this thread");
            *d = Some(Vec::new());
        });
        DeferredWakes(())
    }
}

impl Drop for DeferredWakes {
    fn drop(&mut self) {
        let due = DEFERRED.with(|d| d.borrow_mut().take()).unwrap_or_default();
        wake_all(due);
    }
}

/// Runs the wakes this thread deferred since the last flush.
pub(crate) fn flush_deferred_wakes() {
    let due = DEFERRED.with(|d| d.borrow_mut().as_mut().map(std::mem::take)).unwrap_or_default();
    wake_all(due);
}

fn wake_all(due: Vec<Arc<Observer>>) {
    // Outside the thread-local borrow: a wake may publish or subscribe.
    for observer in due {
        observer.wake_due.store(false, Ordering::Release);
        observer.wake();
    }
}

/// While a deferred wake is queued, the consumer is also woken each time its
/// queue grows by another `1 / PRESSURE_STEPS` of either cap.
const PRESSURE_STEPS: usize = 16;

#[derive(Clone, Copy, Debug)]
pub struct SubscriptionLimits {
    pub max_events: usize,
    /// Logical retained bytes: payloads, strings, headers, and event storage.
    /// Shared Bytes allocations may have spare capacity owned by their producer.
    pub max_bytes: usize,
}

impl Default for SubscriptionLimits {
    fn default() -> Self {
        Self { max_events: 1024, max_bytes: 8 * 1024 * 1024 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubscriptionError {
    Empty,
    /// Queue was cleared and detached after exceeding either limit. Resubscribe
    /// and resynchronize protocol state; subsequent reads keep returning this.
    Overflow,
    /// I/O thread stopped; all queued events have already been consumed.
    Closed,
}

struct State {
    queue: VecDeque<(NetEvent, usize)>,
    bytes: usize,
    terminal: Option<SubscriptionError>,
}

struct Observer {
    state: Mutex<State>,
    limits: SubscriptionLimits,
    wake: Option<NetWake>,
    /// A deferred wake of this observer is queued on an I/O thread.
    wake_due: AtomicBool,
}

impl Observer {
    fn open(&self) -> bool {
        self.state.lock().unwrap().terminal.is_none()
    }

    /// Wakes the consumer now, or at the end of this thread's turn under
    /// [`DeferredWakes`] (one entry per observer however often it is due).
    fn notify(self: &Arc<Self>) {
        if self.wake.is_none() {
            return;
        }
        let deferred = DEFERRED.with(|d| match d.borrow_mut().as_mut() {
            Some(due) => {
                if !self.wake_due.swap(true, Ordering::AcqRel) {
                    due.push(self.clone());
                }
                true
            }
            None => false,
        });
        if !deferred {
            self.wake();
        }
    }

    fn send(self: &Arc<Self>, event: &NetEvent, bytes: usize) {
        let (notify, pressure) = {
            let mut state = self.state.lock().unwrap();
            if state.terminal.is_some() {
                return;
            }
            if state.queue.len() >= self.limits.max_events || bytes > self.limits.max_bytes.saturating_sub(state.bytes)
            {
                state.queue.clear();
                state.bytes = 0;
                state.terminal = Some(SubscriptionError::Overflow);
                (true, false)
            } else {
                let was_empty = state.queue.is_empty();
                let before = state.bytes;
                state.queue.push_back((event.clone(), bytes));
                state.bytes += bytes;
                // Each time the queue grows by another sixteenth of either
                // cap (64 events / 512 KB by default).
                let step_events = (self.limits.max_events / PRESSURE_STEPS).max(1);
                let step_bytes = (self.limits.max_bytes / PRESSURE_STEPS).max(1);
                let pressure = state.queue.len() % step_events == 0 || before / step_bytes < state.bytes / step_bytes;
                (was_empty, pressure)
            }
        };
        if notify {
            self.notify();
        } else if pressure && self.wake_due.load(Ordering::Acquire) {
            // The deferred wake waits for the I/O thread to park (or a request
            // to finish), and a body that keeps its socket readable never lets
            // a current-thread runtime park: coop yields skip the park hook.
            // Wake the consumer now so it drains well before either cap; the
            // queued deferred wake still runs (a spare wake is harmless). At
            // most PRESSURE_STEPS such wakes per undrained queue, so a slow
            // consumer is not woken once per event.
            self.wake();
        }
    }

    /// Terminal: wakes at once, never deferred.
    fn close(&self) {
        let notify = {
            let mut state = self.state.lock().unwrap();
            if state.terminal.is_some() {
                false
            } else {
                state.terminal = Some(SubscriptionError::Closed);
                true
            }
        };
        if notify {
            self.wake();
        }
    }

    fn wake(&self) {
        // No observer or registry lock is held while invoking embedder code.
        if let Some(wake) = &self.wake {
            wake();
        }
    }
}

/// Single-consumer observation of a network context. Send across threads;
/// dropping it releases all queued events without waiting for another request.
pub struct NetSubscription {
    observer: Arc<Observer>,
}

impl NetSubscription {
    pub fn try_recv(&self) -> Result<NetEvent, SubscriptionError> {
        let mut state = self.observer.state.lock().unwrap();
        if let Some((event, bytes)) = state.queue.pop_front() {
            state.bytes -= bytes;
            return Ok(event);
        }
        Err(state.terminal.unwrap_or(SubscriptionError::Empty))
    }

    pub fn queued_usage(&self) -> (usize, usize) {
        let state = self.observer.state.lock().unwrap();
        (state.queue.len(), state.bytes)
    }
}

#[derive(Default)]
struct Registry {
    observers: Vec<Weak<Observer>>,
    closed: bool,
}

#[derive(Default)]
pub(crate) struct Subscriptions(Mutex<Registry>);

impl Subscriptions {
    pub(crate) fn subscribe(&self, limits: SubscriptionLimits, wake: Option<NetWake>) -> NetSubscription {
        let observer = Arc::new(Observer {
            state: Mutex::new(State { queue: VecDeque::new(), bytes: 0, terminal: None }),
            limits,
            wake,
            wake_due: AtomicBool::new(false),
        });
        let closed = {
            let mut registry = self.0.lock().unwrap();
            registry.observers.retain(|o| o.upgrade().is_some_and(|o| o.open()));
            if !registry.closed {
                registry.observers.push(Arc::downgrade(&observer));
            }
            registry.closed
        };
        if closed {
            observer.close();
        }
        NetSubscription { observer }
    }

    fn active(&self) -> Vec<Arc<Observer>> {
        let mut registry = self.0.lock().unwrap();
        let mut active = Vec::with_capacity(registry.observers.len());
        registry.observers.retain(|weak| {
            let Some(observer) = weak.upgrade().filter(|o| o.open()) else { return false };
            active.push(observer);
            true
        });
        active
    }

    pub(crate) fn observed(&self) -> bool {
        let mut registry = self.0.lock().unwrap();
        registry.observers.retain(|o| o.upgrade().is_some_and(|o| o.open()));
        !registry.observers.is_empty()
    }

    pub(crate) fn emit(&self, event: NetEvent) {
        let active = self.active();
        if active.is_empty() {
            return;
        }
        let bytes = event_bytes(&event);
        for observer in active {
            observer.send(&event, bytes);
        }
    }

    pub(crate) fn close(&self) {
        let active = {
            let mut registry = self.0.lock().unwrap();
            registry.closed = true;
            std::mem::take(&mut registry.observers)
        };
        for observer in active.into_iter().filter_map(|o| o.upgrade()) {
            observer.close();
        }
    }
}

fn header_bytes(headers: &http::HeaderMap) -> usize {
    headers.iter().map(|(name, value)| name.as_str().len() + value.len() + 64).sum()
}

fn opt_bytes(value: &Option<String>) -> usize {
    value.as_ref().map_or(0, String::capacity)
}

fn head_bytes(head: &ResponseHead) -> usize {
    std::mem::size_of::<ResponseHead>()
        + head.url.as_str().len()
        + head.status_text.capacity()
        + header_bytes(&head.headers)
        + head.mime.capacity()
        + opt_bytes(&head.charset)
        + opt_bytes(&head.remote_addr)
}

impl NetEvent {
    /// Logical retained bytes of this observation: exactly the charge a
    /// [`NetSubscription`] applies against [`SubscriptionLimits::max_bytes`].
    /// Allocation-free and deterministic for an unchanged event, so an embedder
    /// that moves observations into its own bounded queue can charge and later
    /// release the identical amount.
    pub fn retained_bytes(&self) -> usize {
        event_bytes(self)
    }
}

fn event_bytes(event: &NetEvent) -> usize {
    std::mem::size_of::<NetEvent>()
        + match event {
            NetEvent::RequestWillBeSent { request, redirect_response, .. } => {
                std::mem::size_of::<crate::RequestInfo>()
                    + request.url.as_str().len()
                    + request.method.as_str().len()
                    + header_bytes(&request.headers)
                    + request.post_data.as_ref().map_or(0, bytes::Bytes::len)
                    + opt_bytes(&request.frame_id)
                    + opt_bytes(&request.loader_id)
                    + redirect_response.as_deref().map_or(0, head_bytes)
            }
            NetEvent::ResponseReceived { head, frame_id, loader_id, .. } => {
                head_bytes(head) + opt_bytes(frame_id) + opt_bytes(loader_id)
            }
            NetEvent::DataReceived { data, .. } => data.len(),
            NetEvent::LoadingFinished { .. } => 0,
            NetEvent::LoadingFailed { error, .. } => match error {
                crate::NetError::Cors(s)
                | crate::NetError::Decode(s)
                | crate::NetError::Other(s)
                | crate::NetError::Certificate(s)
                | crate::NetError::Blocked(s)
                | crate::NetError::UnsupportedScheme(s) => s.capacity(),
                _ => 0,
            },
            NetEvent::WebSocket { activity, .. } => {
                std::mem::size_of::<crate::WebSocketActivity>() + activity.retained_bytes()
            }
        }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn event(size: usize) -> NetEvent {
        NetEvent::DataReceived { id: crate::RequestId(1), data: bytes::Bytes::from(vec![0; size]), timestamp: 0.0 }
    }

    #[test]
    fn byte_and_event_overflow_are_terminal_and_release_payloads() {
        struct Body(Arc<AtomicUsize>, [u8; 128]);
        impl AsRef<[u8]> for Body {
            fn as_ref(&self) -> &[u8] {
                &self.1
            }
        }
        impl Drop for Body {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        for limits in [
            SubscriptionLimits { max_events: 1, max_bytes: usize::MAX },
            SubscriptionLimits { max_events: 100, max_bytes: event_bytes(&event(128)) },
        ] {
            let hub = Subscriptions::default();
            let sub = hub.subscribe(limits, None);
            let dropped = Arc::new(AtomicUsize::new(0));
            hub.emit(NetEvent::DataReceived {
                id: crate::RequestId(1),
                data: bytes::Bytes::from_owner(Body(dropped.clone(), [0; 128])),
                timestamp: 0.0,
            });
            assert_eq!(dropped.load(Ordering::SeqCst), 0);
            hub.emit(event(1));
            assert_eq!(dropped.load(Ordering::SeqCst), 1);
            assert_eq!(sub.queued_usage(), (0, 0));
            assert!(matches!(sub.try_recv(), Err(SubscriptionError::Overflow)));
            assert!(!hub.observed());
            hub.emit(event(1));
            hub.close();
            assert!(matches!(sub.try_recv(), Err(SubscriptionError::Overflow)));
        }
    }

    #[test]
    fn retained_bytes_is_the_subscription_charge() {
        let hub = Subscriptions::default();
        let sub = hub.subscribe(SubscriptionLimits::default(), None);
        let events = [
            event(7),
            NetEvent::LoadingFinished { id: crate::RequestId(1), encoded_len: 7, timestamp: 0.0 },
            NetEvent::LoadingFailed {
                id: crate::RequestId(2),
                error: crate::NetError::Other("boom".into()),
                canceled: false,
                destination: crate::request::Destination::Document,
                timestamp: 0.0,
            },
        ];
        let expected: usize = events.iter().map(NetEvent::retained_bytes).sum();
        assert!(expected > 0);
        for e in events {
            hub.emit(e);
        }
        assert_eq!(sub.queued_usage(), (3, expected));
        let mut released = 0;
        while let Ok(e) = sub.try_recv() {
            released += e.retained_bytes();
        }
        assert_eq!(released, expected);
        assert_eq!(sub.queued_usage(), (0, 0));
    }

    #[test]
    fn wake_runs_outside_locks_and_coalesces_until_drain() {
        let hub = Arc::new(Subscriptions::default());
        let slot: Arc<Mutex<Option<NetSubscription>>> = Arc::default();
        let count = Arc::new(AtomicUsize::new(0));
        let (h, s, c) = (Arc::downgrade(&hub), slot.clone(), count.clone());
        let sub = hub.subscribe(
            SubscriptionLimits::default(),
            Some(Arc::new(move || {
                c.fetch_add(1, Ordering::SeqCst);
                // Both methods acquire locks also used by the publisher.
                h.upgrade().unwrap().observed();
                if let Some(sub) = s.lock().unwrap().as_ref() {
                    sub.queued_usage();
                }
            })),
        );
        *slot.lock().unwrap() = Some(sub);
        hub.emit(event(2));
        hub.emit(event(3));
        assert_eq!(count.load(Ordering::SeqCst), 1);
        {
            let slot = slot.lock().unwrap();
            let sub = slot.as_ref().unwrap();
            for size in [2, 3] {
                assert!(matches!(sub.try_recv(), Ok(NetEvent::DataReceived { data, .. }) if data.len() == size));
            }
            assert!(matches!(sub.try_recv(), Err(SubscriptionError::Empty)));
        }
        hub.emit(event(4));
        hub.close();
        assert_eq!(count.load(Ordering::SeqCst), 3);
        let sub = slot.lock().unwrap().take().unwrap();
        assert!(sub.try_recv().is_ok());
        assert!(matches!(sub.try_recv(), Err(SubscriptionError::Closed)));
        let late = hub.subscribe(SubscriptionLimits::default(), None);
        assert!(matches!(late.try_recv(), Err(SubscriptionError::Closed)));
    }

    /// Under `DeferredWakes` a thread's emits wake each observer once per
    /// flush, however many empty-to-nonempty transitions its consumer saw;
    /// `close` still wakes at once, and other threads are not deferred.
    #[test]
    fn deferred_wakes_coalesce_per_flush_and_close_is_immediate() {
        let hub = Arc::new(Subscriptions::default());
        let slot: Arc<Mutex<Option<NetSubscription>>> = Arc::default();
        let count = Arc::new(AtomicUsize::new(0));
        let (s, c) = (slot.clone(), count.clone());
        // The consumer drains inside the wake, like a loop task would.
        let sub = hub.subscribe(
            SubscriptionLimits::default(),
            Some(Arc::new(move || {
                c.fetch_add(1, Ordering::SeqCst);
                if let Some(sub) = s.lock().unwrap().as_ref() {
                    while sub.try_recv().is_ok() {}
                }
            })),
        );
        *slot.lock().unwrap() = Some(sub);
        let deferred = DeferredWakes::enter();
        for size in 1..=4 {
            hub.emit(event(size));
        }
        assert_eq!(count.load(Ordering::SeqCst), 0, "held until the flush");
        assert_eq!(slot.lock().unwrap().as_ref().unwrap().queued_usage().0, 4);
        flush_deferred_wakes();
        assert_eq!(count.load(Ordering::SeqCst), 1, "one wake for four events");
        assert_eq!(slot.lock().unwrap().as_ref().unwrap().queued_usage(), (0, 0));
        flush_deferred_wakes();
        assert_eq!(count.load(Ordering::SeqCst), 1, "nothing due: no wake");
        // Another thread's emit is not deferred.
        let hub2 = hub.clone();
        std::thread::spawn(move || hub2.emit(event(5))).join().unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 2, "the other thread woke at once");
        hub.emit(event(6));
        hub.emit(event(7));
        assert_eq!(count.load(Ordering::SeqCst), 2);
        // The deferred wake of events 6-7 is still due; dropping the guard runs it.
        drop(deferred);
        assert_eq!(count.load(Ordering::SeqCst), 3);
        assert_eq!(slot.lock().unwrap().as_ref().unwrap().queued_usage(), (0, 0));
        let deferred = DeferredWakes::enter();
        hub.emit(event(8));
        hub.close();
        assert_eq!(count.load(Ordering::SeqCst), 4, "close wakes immediately");
        drop(deferred);
        assert_eq!(count.load(Ordering::SeqCst), 5, "the deferred wake of event 8 still runs");
        let sub = slot.lock().unwrap().take().unwrap();
        assert!(matches!(sub.try_recv(), Err(SubscriptionError::Closed)));
    }

    /// A deferred wake that never gets flushed (an I/O thread that never
    /// parks while a fast body streams) still wakes the consumer each time
    /// the queue grows by another sixteenth of a cap: a consumer that drains
    /// on wake is never overrun, and one that does not is woken sixteen
    /// times before the overflow, not once per event.
    #[test]
    fn deferred_wake_turns_immediate_under_queue_pressure() {
        for (limits, size) in [
            (SubscriptionLimits { max_events: 64, max_bytes: usize::MAX }, 1),
            (SubscriptionLimits { max_events: usize::MAX, max_bytes: 64 * event_bytes(&event(100)) }, 100),
        ] {
            // Draining consumer: thousands of events, never overrun.
            let hub = Arc::new(Subscriptions::default());
            let slot: Arc<Mutex<Option<NetSubscription>>> = Arc::default();
            let count = Arc::new(AtomicUsize::new(0));
            let (s, c) = (slot.clone(), count.clone());
            let sub = hub.subscribe(
                limits,
                Some(Arc::new(move || {
                    c.fetch_add(1, Ordering::SeqCst);
                    if let Some(sub) = s.lock().unwrap().as_ref() {
                        while sub.try_recv().is_ok() {}
                    }
                })),
            );
            *slot.lock().unwrap() = Some(sub);
            let deferred = DeferredWakes::enter();
            for _ in 0..4000 {
                hub.emit(event(size));
            }
            let sub = slot.lock().unwrap().take().unwrap();
            assert!(matches!(sub.try_recv(), Err(SubscriptionError::Empty)), "{limits:?}");
            // One wake per step: 4000 events / 4 per step.
            assert_eq!(count.load(Ordering::SeqCst), 1000, "{limits:?}");
            drop(deferred);
            assert_eq!(count.load(Ordering::SeqCst), 1001, "the queued deferred wake still runs");

            // Consumer that does not drain: a pressure wake at each step,
            // then the overflow's own (deferred) wake.
            let hub = Subscriptions::default();
            let count = Arc::new(AtomicUsize::new(0));
            let c = count.clone();
            let sub = hub.subscribe(limits, Some(Arc::new(move || _ = c.fetch_add(1, Ordering::SeqCst))));
            let deferred = DeferredWakes::enter();
            for _ in 0..65 {
                hub.emit(event(size));
            }
            assert_eq!(count.load(Ordering::SeqCst), PRESSURE_STEPS, "{limits:?}");
            assert!(matches!(sub.try_recv(), Err(SubscriptionError::Overflow)));
            drop(deferred);
            assert_eq!(count.load(Ordering::SeqCst), PRESSURE_STEPS + 1);
        }
    }

    #[test]
    fn dropped_consumer_detaches_and_metadata_counts_toward_cap() {
        let hub = Subscriptions::default();
        let sub = hub.subscribe(SubscriptionLimits::default(), None);
        hub.emit(event(4));
        let weak = Arc::downgrade(&sub.observer);
        drop(sub);
        assert!(weak.upgrade().is_none());
        assert!(!hub.observed());
        let sub = hub.subscribe(SubscriptionLimits { max_events: 10, max_bytes: 200 }, None);
        hub.emit(NetEvent::LoadingFailed {
            id: crate::RequestId(1),
            error: crate::NetError::Other("x".repeat(1000)),
            canceled: false,
            destination: crate::Destination::Fetch,
            timestamp: 0.0,
        });
        assert!(matches!(sub.try_recv(), Err(SubscriptionError::Overflow)));
    }
}
