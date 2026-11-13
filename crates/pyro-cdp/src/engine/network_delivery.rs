//! Bounded network observations at browser-thread delivery boundaries.
//!
//! I/O publication is not renderer delivery: author JavaScript can still be
//! running when a response arrives. Request announcements remain immediate;
//! responses become eligible in an owner task or at a binding's explicit
//! per-request delivery frontier. No callback here executes JavaScript.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use pyro_net::{NetClient, NetEvent, NetSubscription, NetWake, RequestId, SubscriptionError, SubscriptionLimits};

use crate::network_protocol::MAX_INFLIGHT;
use crate::outbox::Outbox;

// Service-worker precaching can publish thousands of completed cached requests
// before the renderer gets its next turn. Match the existing CDP transport's
// bounded capacity instead of closing after only 256 four-event requests.
const OBSERVATION_LIMITS: SubscriptionLimits =
    SubscriptionLimits { max_events: MAX_INFLIGHT, max_bytes: crate::outbox::DEFAULT_OUTBOUND_BUDGET };

pub(super) struct Budget {
    used: AtomicUsize,
    limit: AtomicUsize,
}

impl Default for Budget {
    fn default() -> Self {
        Self { used: AtomicUsize::new(0), limit: AtomicUsize::new(OBSERVATION_LIMITS.max_bytes) }
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
        | NetEvent::LoadingFailed { id, .. }
        | NetEvent::WebSocket { id, .. } => *id,
    }
}

impl Staged {
    fn stage(&mut self, event: NetEvent, budget: &Arc<Budget>, owner_task: bool) -> Option<()> {
        if self.failed || self.pending.len() + self.ready.len() >= OBSERVATION_LIMITS.max_events {
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
        // WebSocket activity has no delivery frontier to wait for.
        if owner_task || matches!(observed.event, NetEvent::RequestWillBeSent { .. } | NetEvent::WebSocket { .. }) {
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
                    NetEvent::RequestWillBeSent { .. } | NetEvent::WebSocket { .. } => false,
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
        handle: pyro_runtime::LoopHandle<pyro_browser::Browser>,
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
                subscription: net.subscribe_with(OBSERVATION_LIMITS, Some(notify.clone())),
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

    fn ingest(&self, staged: &mut Staged, owner_task: bool, stop_when_ready: bool) {
        if staged.failed || self.out.is_exceeded() {
            staged.fail();
            return;
        }
        // The retained event cap also bounds each drain. Owner delivery must
        // cover the whole bounded subscription, not leave responses stranded.
        for _ in 0..OBSERVATION_LIMITS.max_events {
            if stop_when_ready && !staged.ready.is_empty() {
                break;
            }
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
        self.ingest(&mut staged, true, false);
    }

    pub(super) fn publish_through(&self, id: RequestId, loader: &str, response: bool, bytes: u64, terminal: bool) {
        let mut staged = self.staged.lock().unwrap();
        self.ingest(&mut staged, false, false);
        staged.publish_through(id, loader, response, bytes, terminal);
    }

    pub(super) fn pop(&self) -> Option<Observed> {
        let mut staged = self.staged.lock().unwrap();
        // The caller consumes one observation at a time. Do not fill staging
        // with another burst while an eligible observation can be delivered.
        self.ingest(&mut staged, false, true);
        staged.ready.pop_front()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pyro_net::{Destination, RequestInfo, ResponseHead, ResponseKind};

    fn budget() -> Arc<Budget> {
        Arc::new(Budget::default())
    }

    fn request(id: u64, loader: &str) -> NetEvent {
        NetEvent::RequestWillBeSent {
            id: RequestId(id),
            request: Box::new(RequestInfo {
                url: "http://127.0.0.1/data".parse().unwrap(),
                method: pyro_net::http::Method::GET,
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
                status: pyro_net::http::StatusCode::OK,
                status_text: "OK".into(),
                headers: Default::default(),
                mime: "text/plain".into(),
                charset: None,
                redirected: false,
                from_cache: false,
                kind: ResponseKind::Basic,
                remote_addr: None,
                protocol: pyro_net::http::Version::HTTP_11,
                timing: Default::default(),
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
                NetEvent::WebSocket { id, .. } => format!("websocket {}", id.0),
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
    fn websocket_activity_is_ready_at_once() {
        let ws = |activity| NetEvent::WebSocket { id: RequestId(9), activity: Box::new(activity), timestamp: 0.0 };
        let (mut s, _budget) = staged(vec![
            ws(pyro_net::WebSocketActivity::Created { url: "ws://127.0.0.1/".parse().unwrap(), frame_id: None }),
            ws(pyro_net::WebSocketActivity::FrameReceived { opcode: 1, payload: "hi".into() }),
            ws(pyro_net::WebSocketActivity::Closed),
        ]);
        assert_eq!(ready(&mut s), ["websocket 9", "websocket 9", "websocket 9"]);
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
    #[test]
    fn a_full_staging_queue_drains_before_reading_more_subscription_events() {
        let net = NetClient::new(pyro_net::NetConfig::default()).unwrap();
        let subscription = net.subscribe_with(OBSERVATION_LIMITS, None);
        let request = pyro_net::Request::get("data:text/plain,ok".parse().unwrap(), Destination::Fetch);
        let (_, body) = net.fetch_blocking(request).unwrap();
        assert_eq!(body.as_ref(), b"ok");
        assert!(subscription.queued_usage().0 > 0);

        let budget = budget();
        let limit = OBSERVATION_LIMITS.max_events;
        let mut staged = Staged::default();
        for index in 0..limit {
            staged.stage(data(1000 + index as u64, 1), &budget, index < limit / 2).unwrap();
        }
        assert!(staged.stage(data(999999, 1), &budget, true).is_none(), "the retained event cap remains enforced");
        let (out, _rx) = Outbox::channel(crate::outbox::DEFAULT_OUTBOUND_BUDGET);
        let inbox = Inbox {
            subscription,
            staged: Mutex::new(staged),
            budget: budget.clone(),
            out: out.clone(),
            notify: Arc::new(|| {}),
        };
        let mut seen = Vec::new();
        while let Some(observed) = inbox.pop() {
            seen.push(request_id(&observed.event));
        }
        assert!(!out.is_exceeded(), "an active consumer must drain staged events instead of falsely overflowing");
        assert_eq!(
            seen.iter().filter(|id| id.0 >= 1000).count(),
            limit / 2,
            "responses still await an owner delivery boundary"
        );
        // More than one owner turn may be needed: events left in the bounded
        // subscription must not be lost or delivered ahead of their frontier.
        for _ in 0..3 {
            inbox.publish_all();
            while let Some(observed) = inbox.pop() {
                seen.push(request_id(&observed.event));
            }
        }
        assert!(!out.is_exceeded());
        assert_eq!(seen.iter().filter(|id| id.0 < 1000).count(), 4, "all four data-URL observations arrive");
        let original: Vec<_> = seen.into_iter().filter(|id| id.0 >= 1000).map(|id| id.0).collect();
        assert_eq!(original, (1000..1000 + limit as u64).collect::<Vec<_>>());
        assert_eq!(inbox.subscription.queued_usage(), (0, 0));
        drop(inbox);
        assert_eq!(budget.used(), 0, "every event reservation was released");
    }

    #[test]
    fn an_exceeded_connection_still_discards_ready_observations() {
        let net = NetClient::new(pyro_net::NetConfig::default()).unwrap();
        let budget = budget();
        let mut staged = Staged::default();
        staged.stage(data(1, 64), &budget, true).unwrap();
        let (out, _rx) = Outbox::channel(crate::outbox::DEFAULT_OUTBOUND_BUDGET);
        let inbox = Inbox {
            subscription: net.subscribe(),
            staged: Mutex::new(staged),
            budget: budget.clone(),
            out: out.clone(),
            notify: Arc::new(|| {}),
        };
        assert!(budget.used() > 0);
        out.exceed();
        assert!(inbox.pop().is_none());
        assert_eq!(budget.used(), 0);
        assert!(inbox.staged.lock().unwrap().failed);
    }
    #[test]
    fn an_incoming_burst_delivers_ready_requests_before_accumulating_more() {
        let net = NetClient::new(pyro_net::NetConfig::default()).unwrap();
        let subscription = net.subscribe_with(OBSERVATION_LIMITS, None);
        let count = OBSERVATION_LIMITS.max_events / 8 + 1;
        for _ in 0..count {
            net.fetch_blocking(pyro_net::Request::get("data:text/plain,ok".parse().unwrap(), Destination::Fetch))
                .unwrap();
        }
        let budget = budget();
        let mut staged = Staged::default();
        let pending = OBSERVATION_LIMITS.max_events / 2;
        for index in 0..pending {
            staged.stage(data(10000 + index as u64, 1), &budget, false).unwrap();
        }
        let (out, _rx) = Outbox::channel(crate::outbox::DEFAULT_OUTBOUND_BUDGET);
        let inbox = Inbox {
            subscription,
            staged: Mutex::new(staged),
            budget: budget.clone(),
            out: out.clone(),
            notify: Arc::new(|| {}),
        };
        let mut requests = 0;
        while let Some(observed) = inbox.pop() {
            assert!(matches!(observed.event, NetEvent::RequestWillBeSent { .. }));
            requests += 1;
        }
        assert!(!out.is_exceeded());
        assert_eq!(requests, count);
        inbox.publish_all();
        let mut responses = 0;
        while inbox.pop().is_some() {
            responses += 1;
        }
        assert_eq!(responses, pending + 3 * count);
        assert!(!out.is_exceeded());
        drop(inbox);
        assert_eq!(budget.used(), 0);
    }
    #[test]
    fn cached_body_burst_is_fully_delivered_within_the_connection_budget() {
        let net = NetClient::new(pyro_net::NetConfig::default()).unwrap();
        let subscription = net.subscribe_with(OBSERVATION_LIMITS, None);
        let body = bytes::Bytes::from(vec![b'x'; 32 * 1024]);
        let count = 384;
        for index in 0..count {
            let mut request = pyro_net::Request::get(
                format!("blob:http://cached-burst.test/{index}").parse().unwrap(),
                Destination::Fetch,
            );
            request.blob = Some(pyro_net::BlobBody { bytes: body.clone(), mime: "text/plain".into() });
            net.fetch_blocking(request).unwrap();
        }
        let (events, retained) = subscription.queued_usage();
        assert_eq!(events, count * 4);
        assert!(events > SubscriptionLimits::default().max_events);
        assert!(retained > SubscriptionLimits::default().max_bytes);
        assert!(retained <= OBSERVATION_LIMITS.max_bytes);
        let budget = budget();
        let (out, _rx) = Outbox::channel(crate::outbox::DEFAULT_OUTBOUND_BUDGET);
        let inbox = Inbox {
            subscription,
            staged: Mutex::default(),
            budget: budget.clone(),
            out: out.clone(),
            notify: Arc::new(|| {}),
        };
        inbox.publish_all();
        let mut delivered = 0;
        while inbox.pop().is_some() {
            delivered += 1;
        }
        assert_eq!(delivered, events, "one owner boundary publishes every observation already received");
        assert!(!out.is_exceeded());
        assert_eq!(inbox.subscription.queued_usage(), (0, 0));
        drop(inbox);
        assert_eq!(budget.used(), 0);
    }
}
