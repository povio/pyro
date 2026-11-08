//! Bounded, nonblocking Network event observations. Overflow terminates the
//! subscription explicitly; it never leaves a partial lifecycle looking valid.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, Weak};

use crate::{NetEvent, ResponseHead};

pub type NetWake = Arc<dyn Fn() + Send + Sync>;

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
}

impl Observer {
    fn open(&self) -> bool {
        self.state.lock().unwrap().terminal.is_none()
    }

    fn send(&self, event: &NetEvent, bytes: usize) {
        let notify = {
            let mut state = self.state.lock().unwrap();
            if state.terminal.is_some() {
                return;
            }
            if state.queue.len() >= self.limits.max_events || bytes > self.limits.max_bytes.saturating_sub(state.bytes)
            {
                state.queue.clear();
                state.bytes = 0;
                state.terminal = Some(SubscriptionError::Overflow);
                true
            } else {
                let was_empty = state.queue.is_empty();
                state.queue.push_back((event.clone(), bytes));
                state.bytes += bytes;
                was_empty
            }
        };
        if notify {
            self.wake();
        }
    }

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
