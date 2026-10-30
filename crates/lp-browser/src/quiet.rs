//! Whether a page has work left that can still change it: the "nothing
//! else is being done" signal `LP.run` settles on after an action and
//! `LP.idle` waits for.
//!
//! A page is quiet when the browser thread has no queued task (local, or
//! posted from another thread: a response counts as a request in flight
//! until its completion is posted, then as a task until that runs), none of
//! the page's frames has a request in flight (beyond an allowance the caller
//! sets, e.g. what was already in flight before an action), a `setTimeout`
//! due within [`TIMER_HORIZON`], an animation frame callback that is not
//! part of an animation loop, or a navigation that has not reached
//! DOMContentLoaded. Microtasks (promise reactions, MutationObserver
//! records) always drain before a task ends, so they never show up here.
//! Layout and styles are computed on demand, so they never pend either.
//!
//! Not pending work: intervals and later timeouts (polling, debounces),
//! requests that were in flight before (long polls), and animation loops
//! on the frame clock. A caller that needs them waits for what they do.

pub use crate::window::TIMER_HORIZON;
use crate::{Browser, PageId, Window};

/// What can still change a page, counted at one moment.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PageWork {
    /// Tasks queued on the page's browser thread (any page's), including
    /// ones posted from other threads that have not run yet.
    pub tasks: usize,
    /// Requests in flight across the page's frames.
    pub requests: usize,
    /// `setTimeout`s due within [`TIMER_HORIZON`].
    pub timers: usize,
    /// Animation frame callbacks waiting for a frame.
    pub frames: usize,
    /// Frames whose navigation has not reached DOMContentLoaded.
    pub navigations: usize,
}

impl PageWork {
    /// Nothing left but up to `requests` requests in flight.
    pub fn quiet(&self, requests: usize) -> bool {
        self.tasks == 0 && self.requests <= requests && self.timers == 0 && self.frames == 0 && self.navigations == 0
    }
}

impl Browser {
    /// What can still change `page` now; `None` once the page is gone.
    pub fn page_work(&self, page: PageId) -> Option<PageWork> {
        let p = self.page(page)?;
        let net = self.context_net(p.context);
        let now = crate::clock::now();
        let sched = self.scheduler();
        let tasks = sched.pending_tasks() + sched.pending_remote_tasks();
        let mut work = PageWork { tasks, ..Default::default() };
        for key in p.frames.keys() {
            let Some(doc) = self.doc(key) else { continue };
            let frame = &doc.main_frame;
            if doc.pending_loader.is_some() || !frame.dom_content_loaded {
                work.navigations += 1;
            }
            work.requests += net.and_then(|n| n.activity(&frame.loader_id.0)).map_or(0, |a| a.inflight);
            let Some(scope) = frame.main_realm().and_then(|r| r.scope().ok()) else { continue };
            if let Some(w) = scope.global_data::<Window>() {
                let (timers, frames) = w.pending_work(now);
                work.timers += timers;
                work.frames += frames;
            }
        }
        Some(work)
    }
}
