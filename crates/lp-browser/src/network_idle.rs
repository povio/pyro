//! `networkAlmostIdle` / `networkIdle` for a loaded document, after Chrome's
//! IdlenessDetector: once `load` has fired, a milestone is reported when the
//! document's in-flight requests (lp-net's per-loader activity: subresources,
//! fetch, XHR, ...) have stayed at most 2 / at 0 for [`NETWORK_QUIET`]. Each
//! is reported once per document; a request that fails or is aborted counts
//! as finished. A replaced or closed document reports nothing further.

use crate::frames::FrameKey;
use crate::{Browser, LoaderId, PageEvent};
use std::time::{Duration, Instant};

/// How long the in-flight count must stay at a threshold.
pub(crate) const NETWORK_QUIET: Duration = Duration::from_millis(500);
/// Re-check interval while a threshold is not met (there is no activity
/// notification; this only bounds how late a report can be).
const NETWORK_POLL: Duration = Duration::from_millis(100);

/// Network quiet milestones reached by a frame's current document.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NetworkIdle {
    pub almost_idle: bool,
    pub idle: bool,
}

impl Browser {
    /// Starts watching the document of `loader` after its `load` event.
    pub(crate) fn watch_network_idle(&mut self, page: FrameKey, loader: LoaderId) {
        self.check_network_idle(page, &loader, Instant::now());
    }

    fn check_network_idle(&mut self, page: FrameKey, loader: &LoaderId, loaded_at: Instant) {
        let Some(p) = self.doc(page) else { return };
        if &p.main_frame.loader_id != loader {
            return; // replaced
        }
        let mut reached = p.main_frame.network_idle;
        let frame_id = p.main_frame.id.clone();
        drop(p);
        let activity =
            self.page(page.page).and_then(|p| self.context_net(p.context)).and_then(|net| net.activity(&loader.0));
        let inflight = activity.map_or(0, |a| a.inflight);
        // Quiet windows start no earlier than `load`.
        let quiet_since = |since: Option<Instant>| since.map_or(loaded_at, |s| s.max(loaded_at));
        let idle_at = (inflight == 0).then(|| quiet_since(activity.and_then(|a| a.idle_since)) + NETWORK_QUIET);
        let almost_at =
            (inflight <= 2).then(|| quiet_since(activity.and_then(|a| a.almost_idle_since)) + NETWORK_QUIET);
        let now = Instant::now();
        let idle_due = idle_at.is_some_and(|t| t <= now);
        // Idle implies almost idle; report both in order.
        if !reached.almost_idle && (idle_due || almost_at.is_some_and(|t| t <= now)) {
            reached.almost_idle = true;
            self.set_network_idle(page, reached);
            let loader_id = loader.clone();
            self.emit(page.page, PageEvent::FrameNetworkAlmostIdle { frame_id: frame_id.clone(), loader_id });
        }
        if idle_due {
            reached.idle = true;
            self.set_network_idle(page, reached);
            self.emit(page.page, PageEvent::FrameNetworkIdle { frame_id, loader_id: loader.clone() });
            return;
        }
        let next = [(!reached.almost_idle).then_some(almost_at).flatten(), idle_at].into_iter().flatten().min();
        let delay = next.map_or(NETWORK_POLL, |t| t.saturating_duration_since(now)).max(Duration::from_millis(1));
        let loader = loader.clone();
        // Not a keep-alive timer: waiting for idle never keeps a run alive.
        self.scheduler().set_timer(delay, false, false, move |b| b.check_network_idle(page, &loader, loaded_at));
    }

    fn set_network_idle(&mut self, page: FrameKey, reached: NetworkIdle) {
        if let Some(mut p) = self.doc_mut(page) {
            p.main_frame.network_idle = reached;
        }
    }

    /// Forgets lp-net's activity entry of a document that is gone.
    pub(crate) fn forget_network_activity(&self, page: FrameKey, loader: &LoaderId) {
        if let Some(net) = self.page(page.page).and_then(|p| self.context_net(p.context)) {
            net.forget_activity(&loader.0);
        }
    }
}
