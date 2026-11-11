//! Whether a page has work left that can still change it: the "nothing
//! else is being done" signal `LP.run` settles on after an action and
//! `LP.idle` waits for.
//!
//! A page is quiet when the browser thread has no queued task (local, or
//! posted from another thread: a response counts as a request in flight
//! until its completion is posted, then as a task until that runs), none of
//! the page's frames has a finite request in flight (of those the caller
//! asks about: all of them, or the ones started after a point it marked,
//! e.g. before an action), a `setTimeout` due within [`TIMER_HORIZON`], an
//! animation frame callback that is not part of an animation loop, or a
//! navigation that has not reached DOMContentLoaded. Microtasks (promise
//! reactions, MutationObserver records) always drain before a task ends, so
//! they never show up here. Layout and styles are computed on demand; what
//! the next rendering update owes is pending work because its handlers can
//! change the page: `scroll` events (a virtualized list renders the rows
//! scrolled to) and ResizeObserver gathers and delivery (callbacks
//! reposition UI).
//!
//! Not pending work: intervals and later timeouts (polling, debounces),
//! requests started before the caller's mark (long polls), and animation
//! loops on the frame clock. A caller that needs them waits for what they do.

pub use crate::window::TIMER_HORIZON;
use crate::{Browser, PageId, Window};

/// What can still change a page, counted at one moment.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PageWork {
    /// Tasks queued on the page's browser thread (any page's), including
    /// ones posted from other threads that have not run yet.
    pub tasks: usize,
    /// Finite requests in flight across the page's frames (started after
    /// the mark the reading was taken with), excluding established streams.
    pub requests: usize,
    /// Established event streams, excluded from finite settling work.
    pub event_streams: usize,
    /// `setTimeout`s due within [`TIMER_HORIZON`].
    pub timers: usize,
    /// Animation frame callbacks waiting for a frame.
    pub frames: usize,
    /// Frames with queued ResizeObserver gathering or notification.
    pub observers: usize,
    /// Frames whose next rendering update has scroll steps to run: queued
    /// `scroll` events, or a re-clamp of scroll offsets after a DOM change.
    pub scrolls: usize,
    /// Frames whose navigation has not reached DOMContentLoaded.
    pub navigations: usize,
}

impl PageWork {
    /// Nothing left.
    pub fn quiet(&self) -> bool {
        self.tasks == 0
            && self.requests == 0
            && self.timers == 0
            && self.frames == 0
            && self.observers == 0
            && self.scrolls == 0
            && self.navigations == 0
    }

    /// Quiet but for what the next rendering update owes (scroll steps, a
    /// ResizeObserver gather or delivery): work that waits for the frame
    /// interval, so only the clock holds the page
    /// ([`Browser::update_rendering`]).
    pub fn only_rendering(&self) -> bool {
        (self.observers > 0 || self.scrolls > 0) && PageWork { observers: 0, scrolls: 0, ..*self }.quiet()
    }
}

/// A reading of every way script can leave a page work to do: tasks posted
/// and timers set on the browser thread (any page's), requests the page's
/// context started, timers and animation frames its windows asked for,
/// changes to its documents (DOM, focus and hover, style sheets, scroll
/// positions: what layout observers measure against), and its observer work
/// and navigations. Two equal readings, the second taken after the safe
/// point that follows a script ([`Browser::checkpoint`]), mean the script
/// (an `eval` step's) left nothing behind that an event-loop turn would run.
/// Work it handed to another thread without starting a request (a message
/// to a worker) is not seen: its answer is no part of the next turn either.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkStamp {
    scheduled: u64,
    requests: u64,
    handles: u64,
    documents: u64,
    observers: usize,
    navigations: usize,
}

impl Browser {
    /// See [`WorkStamp`]; `None` once the page is gone.
    pub fn work_stamp(&self, page: PageId) -> Option<WorkStamp> {
        let work = self.page_work(page)?;
        let p = self.page(page)?;
        let (mut handles, mut documents) = (0u64, 0u64);
        for key in p.frames.keys() {
            let Some(doc) = self.doc(key) else { continue };
            let frame = &doc.main_frame;
            if let Some(page) = &frame.page_dom {
                // Three counters that only grow, so their sum does too.
                let ((version, styles, ..), scrolls) = page.layout().observation_stamp(&page.dom.borrow());
                documents = documents.wrapping_add(version).wrapping_add(styles).wrapping_add(scrolls);
            }
            let Some(scope) = frame.main_realm().and_then(|r| r.scope().ok()) else { continue };
            if let Some(w) = scope.global_data::<Window>() {
                handles += u64::from(w.handles_issued());
            }
        }
        Some(WorkStamp {
            scheduled: self.scheduler().scheduled(),
            requests: self.context_net(p.context).map_or(0, |net| net.requests_started()),
            handles,
            documents,
            observers: work.observers,
            navigations: work.navigations,
        })
    }

    /// Has the page's frames run what their next rendering update owes now
    /// instead of at the frame interval: queued scroll steps (as a task),
    /// then a ResizeObserver gather (callbacks are still delivered by their
    /// own task). For a page that has nothing else left to do
    /// ([`PageWork::only_rendering`]): the interval spares a busy page a
    /// layout per task, and only delays a quiet one.
    pub fn update_rendering(&self, page: PageId) {
        let Some(p) = self.page(page) else { return };
        let pending: Vec<_> = p
            .frames
            .keys()
            .into_iter()
            .filter_map(|key| self.doc(key)?.main_frame.page_dom.clone())
            .filter(|page| page.pending_scroll_work() || page.pending_resize_observer_work())
            .collect();
        for page in pending {
            page.request_rendering_update();
        }
    }
}

impl Browser {
    /// A point in the history of the requests `page`'s context makes: the
    /// ones started from now on are those [`Self::page_work_since`] counts
    /// for this reading.
    pub fn request_mark(&self, page: PageId) -> u64 {
        self.page(page).and_then(|p| self.context_net(p.context)).map_or(0, |net| net.request_mark())
    }

    /// What can still change `page` now; `None` once the page is gone.
    pub fn page_work(&self, page: PageId) -> Option<PageWork> {
        self.page_work_since(page, 0)
    }

    /// [`Self::page_work`], counting only the requests started after `mark`
    /// ([`Self::request_mark`]).
    pub fn page_work_since(&self, page: PageId, mark: u64) -> Option<PageWork> {
        let p = self.page(page)?;
        let net = self.context_net(p.context);
        let now = crate::clock::now();
        let sched = self.scheduler();
        let tasks = sched.pending_tasks() + sched.pending_remote_tasks();
        let mut work = PageWork { tasks, ..Default::default() };
        for key in p.frames.keys() {
            let Some(doc) = self.doc(key) else { continue };
            let frame = &doc.main_frame;
            if let Some(page) = &frame.page_dom {
                work.observers += usize::from(page.pending_resize_observer_work());
                work.scrolls += usize::from(page.pending_scroll_work());
            }
            if doc.pending_loader.is_some() || !frame.dom_content_loaded {
                work.navigations += 1;
            }
            if let Some(net) = net {
                work.requests += net.inflight_after(&frame.loader_id.0, mark);
                work.event_streams += net.activity(&frame.loader_id.0).map_or(0, |a| a.event_streams);
            }
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
