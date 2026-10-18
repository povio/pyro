//! Bounded network observations at browser-thread delivery boundaries.
//!
//! I/O publication is not renderer delivery: author JavaScript can still be
//! running when a response arrives. Request announcements remain immediate;
//! responses become eligible in an owner task or at a binding's explicit
//! per-request delivery frontier. No callback here executes JavaScript.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use lp_net::{NetClient, NetEvent, NetSubscription, NetWake, RequestId, SubscriptionError, SubscriptionLimits};

use crate::network_protocol::MAX_INFLIGHT;
use crate::outbox::Outbox;

pub(super) struct Budget {
    used: AtomicUsize,
    limit: AtomicUsize,
}

impl Default for Budget {
    fn default() -> Self {
        Self { used: AtomicUsize::new(0), limit: AtomicUsize::new(SubscriptionLimits::default().max_bytes) }
    }
}

impl Budget {
    pub(super) fn used(&self) -> usize {
        self.used.load(Ordering::Relaxed)
    }
    pub(super) fn set_limit(&self, limit: usize) {
        self.limit.store(limit, Ordering::Relaxed);
    }
    fn reserve(self: &Arc<Self>, bytes: usize) -> Option<Charge> {
        // A compare-exchange loop: `fetch_update` is deprecated on the
        // toolchain, and its replacement is newer than the crate's MSRV.
        let mut used = self.used.load(Ordering::Relaxed);
        loop {
            let next = used.checked_add(bytes).filter(|n| *n <= self.limit.load(Ordering::Relaxed))?;
            match self.used.compare_exchange_weak(used, next, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => break,
                Err(actual) => used = actual,
            }
        }
        Some(Charge { budget: self.clone(), bytes })
    }
}

struct Charge {
    budget: Arc<Budget>,
    bytes: usize,
}
impl Drop for Charge {
    fn drop(&mut self) {
        self.budget.used.fetch_sub(self.bytes, Ordering::Relaxed);
    }
}

/// The reservation follows the event into Collector, and is released on
/// translation, overflow, context disposal, or connection teardown.
pub(crate) struct Observed {
    pub(super) event: NetEvent,
    _charge: Charge,
}

struct Identity {
    loader: Option<String>,
    _charge: Charge,
}
struct Progress {
    identity: Arc<Identity>,
    bytes: u64,
}
struct Pending {
    observed: Observed,
    identity: Option<Arc<Identity>>,
    bytes: u64,
}

#[derive(Default)]
struct Staged {
    pending: VecDeque<Pending>,
    ready: VecDeque<Observed>,
    requests: HashMap<RequestId, Progress>,
    failed: bool,
}

fn request_id(event: &NetEvent) -> RequestId {
    match event {
        NetEvent::RequestWillBeSent { id, .. }
        | NetEvent::ResponseReceived { id, .. }
        | NetEvent::DataReceived { id, .. }
        | NetEvent::LoadingFinished { id, .. }
        | NetEvent::LoadingFailed { id, .. } => *id,
    }
}

impl Staged {
    fn stage(&mut self, event: NetEvent, budget: &Arc<Budget>, owner_task: bool) -> Option<()> {
        if self.failed || self.pending.len() + self.ready.len() >= SubscriptionLimits::default().max_events {
            return None;
        }
        let id = request_id(&event);
        if let NetEvent::RequestWillBeSent { request, .. } = &event {
            if !self.requests.contains_key(&id) {
                if self.requests.len() >= MAX_INFLIGHT {
                    return None;
                }
                let charge = budget
                    .reserve(std::mem::size_of::<Progress>() + request.loader_id.as_ref().map_or(0, String::len))?;
                self.requests.insert(
                    id,
                    Progress {
                        identity: Arc::new(Identity { loader: request.loader_id.clone(), _charge: charge }),
                        bytes: 0,
                    },
                );
            }
        }
        let progress = self.requests.get_mut(&id);
        let (identity, bytes) = if let Some(progress) = progress {
            if let NetEvent::DataReceived { data, .. } = &event {
                progress.bytes = progress.bytes.saturating_add(data.len() as u64);
            }
            (Some(progress.identity.clone()), progress.bytes)
        } else {
            (None, 0)
        };
        if matches!(event, NetEvent::LoadingFinished { .. } | NetEvent::LoadingFailed { .. }) {
            self.requests.remove(&id);
        }
        let bytes_reserved = event.retained_bytes() + std::mem::size_of::<Pending>() - std::mem::size_of::<NetEvent>();
        let observed = Observed { event, _charge: budget.reserve(bytes_reserved)? };
        if owner_task || matches!(observed.event, NetEvent::RequestWillBeSent { .. }) {
            self.ready.push_back(observed);
        } else {
            self.pending.push_back(Pending { observed, identity, bytes });
        }
        Some(())
    }

    fn publish_all(&mut self) {
        self.ready.extend(self.pending.drain(..).map(|p| p.observed));
    }

    // The frontier refers to consumed transport events, not local aborts or
    // buffer errors. An earlier owner notification task may already have
    // published part/all of it; matching only remaining events is idempotent.
    fn publish_through(&mut self, id: RequestId, loader: &str, response: bool, bytes: u64, terminal: bool) {
        let mut pending = VecDeque::new();
        while let Some(item) = self.pending.pop_front() {
            let matches = request_id(&item.observed.event) == id
                && item.identity.as_ref().is_some_and(|i| i.loader.as_deref() == Some(loader));
            let eligible = matches
                && match &item.observed.event {
                    NetEvent::ResponseReceived { .. } => response,
                    NetEvent::DataReceived { .. } => item.bytes <= bytes,
                    NetEvent::LoadingFinished { .. } | NetEvent::LoadingFailed { .. } => terminal,
                    NetEvent::RequestWillBeSent { .. } => false,
                };
            if eligible {
                self.ready.push_back(item.observed);
            } else {
                pending.push_back(item);
            }
        }
        self.pending = pending;
    }

    fn fail(&mut self) {
        self.pending.clear();
        self.ready.clear();
        self.requests.clear();
        self.failed = true;
    }
}

pub(crate) struct Inbox {
    subscription: NetSubscription,
    staged: Mutex<Staged>,
    budget: Arc<Budget>,
    out: Outbox,
    notify: NetWake,
}

impl Inbox {
    pub(super) fn new(
        net: &NetClient,
        handle: lp_runtime::LoopHandle<lp_browser::Browser>,
        ready: Arc<AtomicBool>,
        budget: Arc<Budget>,
        out: Outbox,
    ) -> Arc<Self> {
        let inbox = Arc::new_cyclic(|weak: &std::sync::Weak<Self>| {
            let weak = weak.clone();
            let posted = Arc::new(AtomicBool::new(false));
            let notify: NetWake = Arc::new(move || {
                if !posted.swap(true, Ordering::AcqRel) {
                    let weak = weak.clone();
                    let posted = posted.clone();
                    let ready = ready.clone();
                    // A weak reference prevents queued tasks from retaining
                    // disposed contexts or their buffered response bodies.
                    let _ = handle.post(move |_| {
                        posted.store(false, Ordering::Release);
                        if let Some(inbox) = weak.upgrade() {
                            inbox.publish_all();
                            ready.store(true, Ordering::Release);
                        }
                    });
                }
            });
            Self {
                subscription: net.subscribe_with(SubscriptionLimits::default(), Some(notify.clone())),
                staged: Mutex::new(Staged::default()),
                budget,
                out,
                notify,
            }
        });
        // Also handles a subscription already closed during construction.
        (inbox.notify)();
        inbox
    }

    fn ingest(&self, staged: &mut Staged, owner_task: bool) {
        if staged.failed || self.out.is_exceeded() {
            staged.fail();
            return;
        }
        // The buffer's event cap also bounds each drain under a producer
        // continuously publishing while this owner task runs.
        for _ in 0..SubscriptionLimits::default().max_events {
            match self.subscription.try_recv() {
                Ok(event) => {
                    if staged.stage(event, &self.budget, owner_task).is_none() {
                        staged.fail();
                        self.out.exceed();
                        break;
                    }
                }
                Err(SubscriptionError::Empty) => break,
                _ => {
                    staged.fail();
                    self.out.exceed();
                    break;
                }
            }
        }
    }

    fn publish_all(&self) {
        let mut staged = self.staged.lock().unwrap();
        staged.publish_all();
        self.ingest(&mut staged, true);
    }

    pub(super) fn publish_through(&self, id: RequestId, loader: &str, response: bool, bytes: u64, terminal: bool) {
        let mut staged = self.staged.lock().unwrap();
        self.ingest(&mut staged, false);
        staged.publish_through(id, loader, response, bytes, terminal);
    }

    pub(super) fn pop(&self) -> Option<Observed> {
        let mut staged = self.staged.lock().unwrap();
        self.ingest(&mut staged, false);
        staged.ready.pop_front()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lp_net::{Destination, RequestInfo, ResponseHead, ResponseKind};

    fn budget() -> Arc<Budget> {
        Arc::new(Budget::default())
    }

    fn request(id: u64, loader: &str) -> NetEvent {
        NetEvent::RequestWillBeSent {
            id: RequestId(id),
            request: Box::new(RequestInfo {
                url: "http://127.0.0.1/data".parse().unwrap(),
                method: lp_net::http::Method::GET,
                headers: Default::default(),
                post_data: None,
                destination: Destination::Fetch,
                frame_id: Some("F".into()),
                loader_id: Some(loader.into()),
            }),
            redirect_response: None,
            timestamp: 0.0,
            wall_time: 0.0,
        }
    }

    fn response(id: u64) -> NetEvent {
        NetEvent::ResponseReceived {
            id: RequestId(id),
            head: Box::new(ResponseHead {
                url: "http://127.0.0.1/data".parse().unwrap(),
                status: lp_net::http::StatusCode::OK,
                status_text: "OK".into(),
                headers: Default::default(),
                mime: "text/plain".into(),
                charset: None,
                redirected: false,
                from_cache: false,
                kind: ResponseKind::Basic,
                remote_addr: None,
            }),
            destination: Destination::Fetch,
            frame_id: Some("F".into()),
            loader_id: None,
            timestamp: 0.0,
        }
    }

    fn data(id: u64, len: usize) -> NetEvent {
        NetEvent::DataReceived { id: RequestId(id), data: vec![b'x'; len].into(), timestamp: 0.0 }
    }

    fn finished(id: u64) -> NetEvent {
        NetEvent::LoadingFinished { id: RequestId(id), encoded_len: 0, timestamp: 0.0 }
    }

    fn ready(staged: &mut Staged) -> Vec<String> {
        staged
            .ready
            .drain(..)
            .map(|o| match o.event {
                NetEvent::RequestWillBeSent { id, .. } => format!("request {}", id.0),
                NetEvent::ResponseReceived { id, .. } => format!("response {}", id.0),
                NetEvent::DataReceived { id, data, .. } => format!("data {} {}", id.0, data.len()),
                NetEvent::LoadingFinished { id, .. } => format!("finished {}", id.0),
                NetEvent::LoadingFailed { id, .. } => format!("failed {}", id.0),
            })
            .collect()
    }

    fn staged(events: Vec<NetEvent>) -> (Staged, Arc<Budget>) {
        let budget = budget();
        let mut staged = Staged::default();
        for event in events {
            staged.stage(event, &budget, false).expect("within budget");
        }
        (staged, budget)
    }

    #[test]
    fn requests_are_immediate_and_responses_wait_for_their_frontier() {
        let (mut s, _budget) = staged(vec![request(1, "L"), response(1), data(1, 3), data(1, 4), finished(1)]);
        assert_eq!(ready(&mut s), ["request 1"]);
        s.publish_through(RequestId(1), "L", true, 0, false);
        assert_eq!(ready(&mut s), ["response 1"]);
        // Cumulative bytes: 3 covers only the first chunk.
        s.publish_through(RequestId(1), "L", true, 3, false);
        assert_eq!(ready(&mut s), ["data 1 3"]);
        // Idempotent: a repeated or partly published frontier adds nothing.
        s.publish_through(RequestId(1), "L", true, 3, false);
        assert!(ready(&mut s).is_empty());
        s.publish_through(RequestId(1), "L", true, 7, true);
        assert_eq!(ready(&mut s), ["data 1 4", "finished 1"]);
    }

    #[test]
    fn frontier_publishes_only_its_request_and_loader() {
        let (mut s, _budget) = staged(vec![request(1, "L1"), request(2, "L1"), response(1), response(2)]);
        assert_eq!(ready(&mut s), ["request 1", "request 2"]);
        // Wrong loader (a replaced document's id) publishes nothing.
        s.publish_through(RequestId(1), "L2", true, 0, true);
        assert!(ready(&mut s).is_empty());
        s.publish_through(RequestId(2), "L1", true, 0, false);
        assert_eq!(ready(&mut s), ["response 2"]);
        // The owner task still publishes everything left, in order.
        s.publish_all();
        assert_eq!(ready(&mut s), ["response 1"]);
    }

    #[test]
    fn charges_follow_events_and_failure_releases_them() {
        let (mut s, budget) = staged(vec![request(1, "L"), response(1), data(1, 1000)]);
        assert!(budget.used() > 1000);
        s.fail();
        assert_eq!(budget.used(), 0, "staged, ready and identity charges are released");
        assert!(s.stage(request(2, "L"), &budget, false).is_none(), "a failed inbox stays failed");
    }

    #[test]
    fn staging_beyond_the_budget_fails_instead_of_dropping() {
        let budget = budget();
        budget.set_limit(512);
        let mut s = Staged::default();
        assert!(s.stage(request(1, "L"), &budget, false).is_some());
        assert!(s.stage(data(1, 4096), &budget, false).is_none());
    }
}
