//! CDP domains backed by the real engine (`lp-browser` + `lp-js` on
//! JavaScriptCore). [`EngineHost`] implements [`Host`] on the browser thread.
//!
//! Engine notifications (page events, console calls, uncaught exceptions)
//! are queued by [`Collector`] while engine code runs and translated into CDP
//! events by [`EngineHost::flush`] afterwards, so no CDP state is borrowed
//! while JavaScript is on the stack.

#[cfg(feature = "render")]
mod blitz_geometry;
#[cfg(feature = "render")]
pub(crate) use blitz_geometry::set_fork_template;
mod dom;
mod driver;
mod fetch;
mod input;
mod metrics;
mod mocks;
mod network;
mod network_delivery;
mod page;
mod remote;
mod runtime;
#[cfg(feature = "render")]
mod screenshot;
#[cfg(not(feature = "render"))]
mod screenshot {
    //! Built without the `render` feature: screenshots are refused.
    #[derive(Default)]
    pub(crate) struct Renders;

    impl Renders {
        pub(super) fn ready(&self) -> bool {
            false
        }
        pub(super) fn clear(&mut self) {}
        pub(super) fn forget(&mut self, _page: lp_browser::PageId) {}
    }

    impl super::EngineHost {
        pub(super) fn flush_renders(&mut self) {}
    }
}
mod storage;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::sync::{Arc, OnceLock, Weak};

use crate::network_protocol::{BodyBudget, BodyLimits};
use std::time::{SystemTime, UNIX_EPOCH};

use lp_browser::{BrowserThread, Frame, FrameId, PageEvent, PageId, PageObserver};
use lp_js::{ConsoleLevel, EngineHooks, JsException, Persistent, Realm, RealmId, Value};

use serde::Deserialize;

use crate::msg::{self, CdpError, CdpResult, Command, Empty};
use crate::outbox::Outbox;
use crate::runtime_protocol::{HandleBudget, HandleTable};
use crate::target::{ContextKey, Host, PageCommand, PageKey, Sink};
use crate::ws::OutFrame;

/// The longest jump virtual time makes at once. A run waiting on a page whose
/// next timer is further off sleeps as usual (or times out).
const VIRTUAL_JUMP_MAX: std::time::Duration = std::time::Duration::from_secs(10);

/// Engine notifications waiting to be translated into CDP events.
pub(crate) enum Queued {
    Page(PageId, PageEvent),
    /// A command reply produced while engine code ran (`Runtime.evaluate`,
    /// `callFunctionOn`, awaited promises). Queued with the events so that
    /// everything the evaluation caused (e.g. `Page.navigatedWithinDocument`
    /// from `history.pushState`) reaches the client before its reply.
    Frame(OutFrame),
    Console {
        realm: RealmId,
        level: ConsoleLevel,
        args: Vec<Persistent>,
        timestamp: f64,
    },
    Exception {
        realm: RealmId,
        exception: JsException,
        timestamp: f64,
    },
    BindingCalled {
        realm: RealmId,
        name: String,
        payload: String,
    },
    /// A request announcement or response delivered on the browser thread.
    Network(ContextKey, network_delivery::Observed),
}

/// Most engine notifications queued between flushes. A page producing more
/// within one task (e.g. a tight console.log loop) exceeds the connection's
/// budget: the connection is closed (1008) rather than growing without bound.
pub(crate) const MAX_QUEUED_EVENTS: usize = 100_000;

struct NetworkWatch {
    context: ContextKey,
    id: lp_browser::ContextId,
    inbox: Weak<network_delivery::Inbox>,
}

pub(crate) struct Collector {
    queue: RefCell<VecDeque<Queued>>,
    out: Outbox,
    /// Whether any session wants console/exception events; when none does,
    /// console arguments are not rooted at all.
    runtime_wanted: Cell<bool>,
    network: RefCell<Vec<NetworkWatch>>,
    /// Shared reservation covers staging and queued events across contexts.
    network_budget: Arc<network_delivery::Budget>,
}

impl Collector {
    fn new(out: Outbox) -> Self {
        Self {
            queue: RefCell::new(VecDeque::new()),
            out,
            runtime_wanted: Cell::new(false),
            network: RefCell::new(Vec::new()),
            network_budget: Arc::new(network_delivery::Budget::default()),
        }
    }

    pub(crate) fn watch_network(
        &self,
        context: ContextKey,
        id: lp_browser::ContextId,
        inbox: &Arc<network_delivery::Inbox>,
    ) {
        let mut network = self.network.borrow_mut();
        network.retain(|s| s.inbox.strong_count() > 0);
        network.push(NetworkWatch { context, id, inbox: Arc::downgrade(inbox) });
    }

    /// Only owner-delivered responses may precede a page event or command
    /// reply. Raw I/O responses stay staged while author JavaScript runs.
    fn push(&self, item: Queued) {
        self.drain_network();
        self.push_back(item);
    }

    fn push_back(&self, item: Queued) {
        let mut queue = self.queue.borrow_mut();
        if queue.len() >= MAX_QUEUED_EVENTS {
            drop(queue);
            self.out.exceed();
            return;
        }
        queue.push_back(item);
    }

    fn drain_network(&self) {
        if self.out.is_exceeded() {
            return;
        }
        let network = self.network.borrow();
        for watch in network.iter() {
            let Some(inbox) = watch.inbox.upgrade() else { continue };
            while let Some(event) = inbox.pop() {
                self.push_back(Queued::Network(watch.context, event));
            }
        }
    }

    fn pop(&self) -> Option<Queued> {
        self.queue.borrow_mut().pop_front()
    }

    fn clear(&self) {
        self.queue.borrow_mut().clear();
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.queue.borrow().is_empty()
    }
}

/// Milliseconds since the Unix epoch (CDP `Runtime.Timestamp`).
fn epoch_ms() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64() * 1000.0)
}

impl PageObserver for Collector {
    fn on_page_event(&self, page: PageId, event: &PageEvent) {
        self.push(Queued::Page(page, event.clone()));
    }

    /// A fetch/XHR consumer is about to let script observe `frontier` of
    /// `request`: the matching staged response events become deliverable and
    /// are queued now, ahead of anything that script causes (Chrome reports
    /// them before the script sees the response, not when I/O received it).
    fn on_network_delivery(
        &self,
        _page: PageId,
        context: lp_browser::ContextId,
        loader: &lp_browser::LoaderId,
        request: lp_net::RequestId,
        frontier: lp_net::NetworkDelivery,
    ) {
        let inbox = self.network.borrow().iter().find(|w| w.id == context).and_then(|w| w.inbox.upgrade());
        if let Some(inbox) = inbox {
            inbox.publish_through(request, &loader.0, frontier.response, frontier.data_bytes, frontier.terminal);
            self.drain_network();
        }
    }
}

impl EngineHooks for Collector {
    fn console_api(&self, realm: &Realm, level: ConsoleLevel, args: &[Value<'_>]) {
        if !self.runtime_wanted.get() || self.out.is_exceeded() {
            return;
        }
        let Ok(scope) = realm.scope() else { return };
        let args = args.iter().map(|v| Persistent::new(&scope, *v)).collect();
        self.push(Queued::Console { realm: realm.id(), level, args, timestamp: epoch_ms() });
    }

    fn exception_thrown(&self, realm: &Realm, exception: &JsException) {
        if !self.runtime_wanted.get() {
            return;
        }
        self.push(Queued::Exception { realm: realm.id(), exception: exception.clone(), timestamp: epoch_ms() });
    }

    // Only pages a session added a binding to call this.
    fn binding_called(&self, realm: &Realm, name: &str, payload: &str) {
        self.push(Queued::BindingCalled { realm: realm.id(), name: name.to_owned(), payload: payload.to_owned() });
    }

    fn unhandled_rejection(&self, realm: &Realm, exception: &JsException) {
        if !self.runtime_wanted.get() {
            return;
        }
        self.push(Queued::Exception { realm: realm.id(), exception: exception.clone(), timestamp: epoch_ms() });
    }
}

/// Per-session CDP state. Handles are shared with pending asynchronous
/// replies (`awaitPromise`), which run as engine microtasks.
pub(crate) struct Session {
    pub(crate) page_enabled: bool,
    pub(crate) dom_enabled: bool,
    pub(crate) lifecycle_events: bool,
    pub(crate) runtime_enabled: bool,
    pub(crate) network_enabled: bool,
    /// `Network.setExtraHTTPHeaders` of this session. Like Chrome, kept while
    /// Network is disabled but applied to the page only while it is enabled.
    pub(crate) extra_headers: http::HeaderMap,
    /// `Network.setCacheDisabled` of this session (page-scoped in Chrome).
    pub(crate) cache_disabled: bool,
    /// Page.setInterceptFileChooserDialog: file inputs report
    /// Page.fileChooserOpened to this session.
    pub(crate) intercept_file_chooser: bool,
    /// Runtime.addBinding names: their calls are reported to this session.
    pub(crate) bindings: Vec<String>,
    fetch: fetch::FetchSession,
    pub(crate) handles: Rc<RefCell<HandleTable<Persistent>>>,
}

pub(crate) struct PageState {
    pub(crate) id: PageId,
    pub(crate) context: ContextKey,
    pub(crate) sessions: Vec<String>,
    /// Ordered protocol notification state; native snapshots remain the source
    /// of truth for live documents, realms, and reached lifecycle milestones.
    pub(crate) frames: HashMap<FrameId, page::FrameEvents>,
    pub(crate) pending_navigations: Vec<page::PendingNavigation>,
    /// Bounds recorded by Browser.setWindowBounds, reported back by getWindowForTarget.
    pub(crate) window_bounds: Option<serde_json::Value>,
    pub(crate) emulated_media: Option<serde_json::Value>,
    /// Dialogs reported by Page.javascriptDialogOpening and not yet
    /// answered by Page.handleJavaScriptDialog (they never block).
    pub(crate) unanswered_dialogs: u8,
    /// Emulation.setUserAgentOverride as sent (the locale override fills in
    /// its Accept-Language when it has none).
    user_agent_override: Option<lp_webapi::UserAgentOverride>,
    /// Emulation.setLocaleOverride / setTimezoneOverride. Recorded only:
    /// JavaScriptCore's Intl and Date use the process locale and time zone.
    pub(crate) locale_override: Option<String>,
    pub(crate) timezone_override: Option<String>,
    /// Emulation.setDefaultBackgroundColorOverride (RGBA), for screenshots.
    pub(crate) background_override: Option<[u8; 4]>,
    pub(crate) next_script_id: u64,
    /// Paused popup before its first navigation (`Page.getFrameTree` url ":").
    pub(crate) initial_popup: bool,
    /// A raw keydown which changes documents suppresses one following char.
    /// Keyboard input is page-scoped, including across attached sessions.
    pub(crate) suppress_next_char: bool,
    /// Input.setInterceptDrags: native drags started by mouse input are
    /// reported through Input.dragIntercepted.
    pub(crate) intercept_drags: bool,
    network: network::NetworkPage,
    dom_nodes: dom::BackendNodes,
    /// Downloads the page started, newest last (`LP.run` `download` steps).
    pub(crate) downloads: Vec<DownloadRecord>,
    pub(crate) download_count: u64,
}

/// A download a page started, kept for `LP.run` whatever the
/// `Browser.setDownloadBehavior` setting.
#[derive(Clone, Debug)]
pub(crate) struct DownloadRecord {
    pub url: String,
    pub filename: String,
    pub body: Result<bytes::Bytes, String>,
}

/// Bytes of download bodies kept per page; older ones lose their body.
pub(crate) const DOWNLOAD_BUDGET: usize = 64 * 1024 * 1024;

/// A popup that became a target, for `LP.run` `popup` steps.
#[derive(Clone, Debug)]
pub(crate) struct PopupRecord {
    pub opener: PageKey,
    pub page: PageKey,
    pub target_id: String,
}

/// `Browser.setDownloadBehavior` settings.
#[derive(Clone, Debug)]
pub(crate) struct DownloadBehavior {
    /// "allow" (file named as suggested) or "allowAndName" (named by guid);
    /// "deny" and "default" store nothing.
    pub(crate) behavior: String,
    pub(crate) download_path: Option<String>,
    pub(crate) events_enabled: bool,
}

/// The engine side of one CDP connection, living on its browser thread.
pub struct EngineHost {
    pub thread: BrowserThread,
    pub(crate) out: Outbox,
    collector: Rc<Collector>,
    contexts: HashMap<ContextKey, network::NetworkContext>,
    /// Browser.setDownloadBehavior per browser context (None: no
    /// `browserContextId`, the default context).
    pub(crate) download_behavior: HashMap<Option<ContextKey>, DownloadBehavior>,
    body_budget: BodyBudget,
    network_wake: network::NetworkWake,
    /// Screenshot renders of this connection (`screenshot.rs`).
    renders: screenshot::Renders,
    pub(crate) pages: HashMap<PageKey, PageState>,
    pub(crate) sessions: HashMap<String, Session>,
    pub(crate) next_exception_id: i64,
    /// Remote-object handles all sessions of this connection may retain.
    pub(crate) handle_budget: HandleBudget,
    next_backend_node: u64,
    pub(crate) pending: runtime::PendingReplies,
    /// Popups requested by pages, taken by the protocol layer after `flush`.
    pub(crate) popup_requests: Vec<crate::target::PopupRequest>,
    /// Pages booted before the connection (`preboot`), handed to the
    /// protocol layer once by `Host::take_prebooted`.
    prebooted: Vec<crate::target::PrebootedEntry>,
    /// Popups that became targets, oldest first (`LP.run` `popup` steps).
    pub(crate) popups: Vec<PopupRecord>,
    /// `LP.act` / `LP.expect` commands waiting for their page to change.
    waiters: driver::Waiters,
}

/// A page booted on a thread before any connection, with its context.
#[derive(Debug, Clone, Copy)]
pub struct Prebooted {
    pub context: lp_browser::ContextId,
    pub page: lp_browser::PageId,
    pub profile: usize,
}

/// Startup configuration shared by every browser thread of a `serve`
/// process (built once from the command line).
#[derive(Clone, Debug, Default)]
pub struct BrowserProfile {
    /// Network client configuration of every browser context.
    pub net: lp_net::NetConfig,
    /// Local storage entries every browser context starts with.
    pub local_storage: lp_webapi::StorageSeed,
    /// Pages booted ahead of every connection (`serve --prepare URL`).
    pub prepared: Vec<crate::target::PreparedProfile>,
}

impl EngineHost {
    /// What can still change `page` now (`serve --fork` waits for quiet
    /// pages before forking); `None` once it is gone.
    pub fn page_work(&self, page: PageKey) -> Option<lp_browser::PageWork> {
        self.thread.browser.page_work(self.pages.get(&page)?.id)
    }

    /// Creates the engine for the current thread (at most one per thread).
    pub fn new(out: Outbox) -> CdpResult<EngineHost> {
        Self::with_body_budget(out, Self::body_budget())
    }

    /// The process-wide retained-body budget shared by every connection.
    pub fn body_budget() -> BodyBudget {
        static BODY_BUDGET: OnceLock<BodyBudget> = OnceLock::new();
        BODY_BUDGET.get_or_init(|| BodyBudget::new(BodyLimits::default())).clone()
    }

    /// Uses a caller-supplied process budget (shared by every connection).
    pub fn with_body_budget(out: Outbox, body_budget: BodyBudget) -> CdpResult<EngineHost> {
        Self::with_thread(Self::prepare_thread()?, out, body_budget)
    }

    /// The engine and network client of a connection, created ahead of the
    /// connection (`serve` keeps one spare per process so a client's first
    /// command does not wait for engine start). Must stay on this thread.
    pub fn prepare_thread() -> CdpResult<BrowserThread> {
        Self::prepare_thread_with(&BrowserProfile::default())
    }

    /// `prepare_thread` with the process's startup configuration.
    pub fn prepare_thread_with(profile: &BrowserProfile) -> CdpResult<BrowserThread> {
        let mut thread =
            BrowserThread::new().map_err(|e| CdpError::server(format!("JavaScript engine unavailable: {e}")))?;
        thread
            .enable_network(profile.net.clone())
            .map_err(|e| CdpError::server(format!("network unavailable: {e}")))?;
        #[cfg(feature = "render")]
        blitz_geometry::install(
            profile.net.static_files.clone(),
            thread.browser.context_net(lp_browser::DEFAULT_CONTEXT).cloned(),
        );
        if !profile.local_storage.is_empty() {
            thread.set_local_storage_seed(profile.local_storage.clone());
        }
        // Best effort: a failure here only costs the first page its head start.
        let _ = thread.browser.prepare_spare_realm();
        Ok(thread)
    }

    /// Starts the pages of `profiles` on a prepared thread, before any
    /// connection: each in its own context, with the profile's init scripts
    /// and viewport. The caller runs the thread's loop to let them boot and
    /// adopts them with `adopt_prebooted` once the host exists.
    pub fn preboot(thread: &mut BrowserThread, profiles: &[crate::target::PreparedProfile]) -> Vec<Prebooted> {
        let mut out = Vec::new();
        for (index, profile) in profiles.iter().enumerate() {
            for _ in 0..profile.pool.clamp(1, crate::target::MAX_POOL) {
                let Ok(context) = thread.browser.create_context() else { break };
                let Ok(page) = thread.browser.create_page_in(context) else {
                    thread.browser.close_context(context);
                    break;
                };
                for (n, source) in profile.init_scripts.iter().enumerate() {
                    thread.browser.add_context_init_script(
                        context,
                        lp_browser::InitScript { id: format!("prebooted-{n}"), source: source.clone(), world: None },
                    );
                }
                if let Some(v) = profile.viewport {
                    if let Some(p) = thread.browser.page(page) {
                        let mut viewport = p.viewport().clone();
                        viewport.width = v.width;
                        viewport.height = v.height;
                        let _ = thread.browser.set_viewport(page, viewport);
                    }
                }
                if thread.browser.navigate(page, lp_browser::NavigateOpts::address_bar(&profile.url)).is_err() {
                    thread.browser.close_page(page);
                    thread.browser.close_context(context);
                    break;
                }
                out.push(Prebooted { context, page, profile: index });
            }
        }
        out
    }

    /// Registers pages from `preboot` with this host (their events before
    /// now went nowhere, which is fine: no session existed).
    pub fn adopt_prebooted(&mut self, pages: Vec<Prebooted>) {
        for p in pages {
            if self.thread.browser.page(p.page).is_none() {
                continue;
            }
            let context = u64::from(p.context.0);
            self.contexts.entry(context).or_insert_with(|| network::NetworkContext::new(p.context));
            let (page, target_id) = self.register_page(p.page, context);
            self.prebooted.push(crate::target::PrebootedEntry { context, page, target_id, profile: p.profile });
        }
    }

    /// Protocol state for a page of the engine; its target id is the main frame's.
    fn register_page(&mut self, id: PageId, context: ContextKey) -> (PageKey, String) {
        let key = PageKey::from(id.0);
        self.pages.insert(
            key,
            PageState {
                id,
                context,
                sessions: Vec::new(),
                frames: HashMap::new(),
                pending_navigations: Vec::new(),
                window_bounds: None,
                emulated_media: None,
                unanswered_dialogs: 0,
                user_agent_override: None,
                locale_override: None,
                timezone_override: None,
                background_override: None,
                next_script_id: 1,
                initial_popup: false,
                suppress_next_char: false,
                intercept_drags: false,
                network: network::NetworkPage::new(self.body_budget.clone()),
                dom_nodes: dom::BackendNodes::default(),
                downloads: Vec::new(),
                download_count: 0,
            },
        );
        let target_id = self.thread.browser.page(id).expect("page exists").main_frame.id.0.clone();
        (key, target_id)
    }

    /// `with_body_budget` for a thread from `prepare_thread`.
    pub fn with_thread(mut thread: BrowserThread, out: Outbox, body_budget: BodyBudget) -> CdpResult<EngineHost> {
        let collector = Rc::new(Collector::new(out.clone()));
        thread.browser.set_observer(Some(collector.clone()));
        thread.browser.set_engine_hooks(Some(collector.clone()));
        Ok(EngineHost {
            thread,
            out,
            collector,
            contexts: HashMap::new(),
            download_behavior: HashMap::new(),
            body_budget,
            network_wake: network::NetworkWake::default(),
            renders: screenshot::Renders::default(),
            pages: HashMap::new(),
            sessions: HashMap::new(),
            next_exception_id: 1,
            handle_budget: HandleBudget::new(HandleBudget::DEFAULT_LIMIT),
            next_backend_node: 1,
            pending: runtime::PendingReplies::default(),
            popup_requests: Vec::new(),
            prebooted: Vec::new(),
            popups: Vec::new(),
            waiters: driver::Waiters::default(),
        })
    }

    pub fn thread_mut(&mut self) -> &mut BrowserThread {
        &mut self.thread
    }

    /// Runs engine tasks and timers until `wake()` is true or engine
    /// notifications are waiting for [`EngineHost::flush`].
    pub fn run_until_work(
        &mut self,
        deadline: Option<std::time::Instant>,
        mut wake: impl FnMut() -> bool,
    ) -> lp_runtime::RunOutcome {
        let collector = &self.collector;
        let thread = &mut self.thread;
        let network_wake = &self.network_wake;
        let renders = &self.renders;
        let waiters = &self.waiters;
        let deadline = [deadline, waiters.next_deadline()].into_iter().flatten().min();
        // One ready task or due timer first: `run_until` checks `wake` before
        // running anything, and with a client that always has a command
        // waiting (pipelined or concurrent requests) it would return at once
        // every time, so no timer ever ran between commands.
        thread.event_loop.run_one(&mut thread.browser);
        let mut ready = |b: &mut lp_browser::Browser| {
            wake() || !collector.is_empty() || network_wake.ready() || renders.ready() || waiters.ready(b)
        };
        // Virtual time: instead of sleeping until a timer is due while a run
        // waits on a page that has nothing else to do, jump the page clock to
        // it (lp_browser::clock).
        if lp_browser::clock::virtual_time() {
            loop {
                if ready(&mut thread.browser) {
                    return lp_runtime::RunOutcome::Done;
                }
                if thread.event_loop.run_one(&mut thread.browser) {
                    continue;
                }
                if !waiters.only_timers_left(&thread.browser) {
                    break;
                }
                let Some(at) = thread.browser.scheduler().next_timer_deadline() else { break };
                let by = at.saturating_duration_since(std::time::Instant::now());
                if by.is_zero() || by > VIRTUAL_JUMP_MAX {
                    break;
                }
                thread.browser.advance_time(by);
            }
        }
        thread.event_loop.run_until(&mut thread.browser, deadline, false, ready)
    }

    /// Disposes every page and realm (connection teardown).
    pub fn close_all(&mut self) {
        self.pending.cancel_all();
        self.waiters.clear();
        self.renders.clear();
        self.sessions.clear();
        self.pages.clear();
        self.thread.browser.close_all();
        for (_, context) in self.contexts.drain() {
            self.thread.browser.close_context(context.id);
        }
        self.collector.clear();
    }

    /// Recomputes whether console/exception notifications are collected.
    pub(crate) fn update_runtime_wanted(&self) {
        self.collector.runtime_wanted.set(self.sessions.values().any(|s| s.runtime_enabled));
    }

    /// Unanswered `awaitPromise` evaluations held for settlement, context
    /// destruction or session detach (diagnostics/tests).
    pub fn pending_awaited_replies(&self) -> usize {
        self.pending.len()
    }

    /// Replaces the bound on pending `awaitPromise` evaluations (tests).
    pub fn set_pending_reply_limit(&self, limit: usize) {
        self.pending.set_limit(limit);
    }

    /// Replaces the connection's handle budget (tests, embedders with other
    /// memory limits). Only allowed while no session's handle table holds the
    /// current budget; otherwise existing tables would keep counting against
    /// the old budget, so the call fails in every build and changes nothing.
    pub fn set_handle_limit(&mut self, limit: usize) -> CdpResult<()> {
        if !self.sessions.is_empty() || self.handle_budget.is_shared() {
            return Err(CdpError::server("The handle limit can only be set before sessions attach"));
        }
        self.handle_budget = HandleBudget::new(limit);
        Ok(())
    }

    /// Replaces the byte limit of the connection's network observations
    /// (tests, embedders with other memory limits). It bounds everything the
    /// shared budget charges: staged responses awaiting delivery, events
    /// queued for the next flush, and per-request bookkeeping.
    pub fn set_queued_network_byte_limit(&mut self, limit: usize) {
        self.collector.network_budget.set_limit(limit);
    }

    /// Bytes currently charged to the connection's network budget
    /// (diagnostics/tests): staged responses awaiting delivery, events queued
    /// for the next flush, and per-request bookkeeping.
    pub fn queued_network_bytes(&self) -> usize {
        self.collector.network_budget.used()
    }

    /// True when engine notifications are waiting for [`EngineHost::flush`].
    pub fn has_pending_events(&self) -> bool {
        !self.collector.is_empty() || self.network_wake.ready() || self.renders.ready()
    }

    pub(crate) fn page_key(&self, id: PageId) -> Option<PageKey> {
        self.pages.iter().find(|(_, p)| p.id == id).map(|(k, _)| *k)
    }

    pub(crate) fn realm(&self, page: PageKey, realm: RealmId) -> Option<Realm> {
        let id = self.pages.get(&page)?.id;
        self.thread.browser.realm_in_page(id, realm)
    }

    fn frame_for_realm(&self, page: PageKey, realm: RealmId) -> Option<Frame> {
        let id = self.pages.get(&page)?.id;
        self.thread.browser.frames(id).into_iter().find_map(|info| {
            let frame = self.thread.browser.frame(id, &info.id)?;
            frame.realms.iter().any(|r| r.realm.id() == realm).then_some(frame)
        })
    }

    fn frame_for_document(&self, page: PageKey, document: &Rc<lp_webapi::PageDom>) -> Option<Frame> {
        let id = self.pages.get(&page)?.id;
        self.thread.browser.frames(id).into_iter().find_map(|info| {
            let frame = self.thread.browser.frame(id, &info.id)?;
            frame.page_dom.as_ref().is_some_and(|dom| Rc::ptr_eq(dom, document)).then_some(frame)
        })
    }

    fn owns_frame(&self, page: PageId, frame: Option<&String>) -> bool {
        frame.is_some_and(|id| self.thread.browser.frame(page, &FrameId(id.clone())).is_some())
    }

    pub(crate) fn main_realm(&self, page: PageKey) -> Option<Realm> {
        let state = self.pages.get(&page)?;
        self.thread.browser.page(state.id)?.main_frame.main_realm().cloned()
    }

    /// Sessions of `page` with a predicate, in attach order.
    pub(crate) fn sessions_of(&self, page: PageKey, pred: impl Fn(&Session) -> bool) -> Vec<String> {
        let Some(state) = self.pages.get(&page) else { return Vec::new() };
        state.sessions.iter().filter(|s| self.sessions.get(*s).is_some_and(&pred)).cloned().collect()
    }

    pub(crate) fn event<T: serde::Serialize + ?Sized>(&self, method: &str, params: &T, session_id: &str) {
        self.out.push(msg::event_frame(method, params, Some(session_id)));
    }

    /// Translates queued engine notifications into CDP events, in order.
    pub fn flush(&mut self) {
        self.flush_renders();
        self.service_waiters();
        self.drain_collector();
        self.flush_network();
        self.flush_fetch();
        // `LP.run` request steps wait on the log these observations fill.
        if self.waiters.log_changed {
            self.service_waiters();
            self.drain_collector();
        }
    }

    fn drain_collector(&mut self) {
        while let Some(item) = self.collector.pop() {
            match item {
                // Owner-delivered network observations stay in order with
                // page notifications, including before document replacement.
                Queued::Page(page, event) => {
                    if let Some(key) = self.page_key(page) {
                        self.on_page_event(key, event);
                    }
                }
                Queued::Network(context, event) => self.on_network_event(context, event.event),
                Queued::Frame(frame) => self.out.push(frame),
                Queued::Console { realm, level, args, timestamp } => self.on_console(realm, level, args, timestamp),
                Queued::Exception { realm, exception, timestamp } => self.on_exception(realm, exception, timestamp),
                Queued::BindingCalled { realm, name, payload } => self.on_binding_called(realm, name, payload),
            }
        }
    }

    /// Page owning `realm`, if any.
    pub(crate) fn page_of_realm(&self, realm: RealmId) -> Option<PageKey> {
        self.pages.keys().copied().find(|key| self.frame_for_realm(*key, realm).is_some())
    }

    fn reply<T: serde::Serialize + ?Sized>(&self, cmd: &PageCommand<'_, '_>, result: &T) {
        self.out.push(msg::result_frame(cmd.command.id, result, Some(cmd.session_id)));
    }

    fn page_command_result(&mut self, cmd: &PageCommand<'_, '_>) -> CdpResult<Option<serde_json::Value>> {
        let (domain, action) =
            cmd.command.domain_and_action().ok_or_else(|| CdpError::method_not_found(&cmd.command.method))?;
        match domain {
            "Page" => self.page_domain(action, cmd),
            "DOM" => self.dom_domain(action, cmd),
            "Runtime" => self.runtime_domain(action, cmd),
            "Emulation" => self.emulation_domain(action, cmd),
            "Browser" => self.window_domain(action, cmd),
            // Console entries go through Runtime.consoleAPICalled; there
            // are no additional Log-domain entries yet.
            "Network" => self.network_domain(action, cmd),
            "Fetch" => self.fetch_domain(action, cmd),
            "Input" => self.input_domain(action, cmd),
            "Security" => self.security_domain(action, cmd),
            // Blitz-backed layout queries since the last call.
            #[cfg(feature = "render")]
            "LP" if action == "geometryStats" => {
                let (layouts, deltas, snapshot_ms, layout_ms, read_ms) = blitz_geometry::take_stats();
                let (mirror_ms, settle_ms, resolve_ms) = lp_render::sync_profile::take();
                // Blitz counters are cumulative on this browser thread. Unlike
                // the existing resettable fields, callers subtract snapshots.
                let phases = lp_render::blitz_phases();
                let (inline_rebuilds, inline_bytes) = lp_render::blitz_rebuilds();
                let style_stats = lp_render::blitz_style_stats();
                Ok(Some(serde_json::json!({
                    "processId": std::process::id(),
                    "layouts": layouts, "deltas": deltas, "snapshotMs": snapshot_ms, "layoutMs": layout_ms, "readMs": read_ms,
                    "mirrorMs": mirror_ms, "settleMs": settle_ms, "resolveMs": resolve_ms,
                    "blitzCumulative": {
                        "phasesMs": {
                            "style": phases[0], "damage": phases[1], "construct": phases[2],
                            "postConstruct": phases[3], "layout": phases[4], "transform": phases[5],
                            "paintTree": phases[6], "clearDamage": phases[7], "subdocs": phases[8],
                        },
                        "inlineRebuilds": inline_rebuilds, "inlineBytes": inline_bytes,
                        "restyledNodes": lp_render::blitz_restyles(),
                        "styleStats": {
                            "styled": style_stats[0], "matched": style_stats[1],
                            "shared": style_stats[2], "reused": style_stats[3],
                        },
                    },
                })))
            }
            "LP" => self.driver_domain(action, cmd),
            #[cfg(feature = "render")]
            "IO" => self.io_domain(action, cmd),
            "Log" if matches!(action, "enable" | "disable") => Ok(Some(serde_json::json!({}))),
            // puppeteer enables both on every page; there are no issues to
            // report.
            "Audits" | "Performance" if matches!(action, "enable" | "disable") => Ok(Some(serde_json::json!({}))),
            #[cfg(feature = "render")]
            "Performance" if action == "getMetrics" => self.performance_metrics(cmd).map(Some),
            _ => Err(CdpError::unsupported(&cmd.command.method)),
        }
    }

    /// `Performance.getMetrics`: Chrome's names for what the engine counts.
    /// Layout and style counts and durations are the process's Blitz syncs
    /// (each restyles and relays out what changed, so `RecalcStyleCount`
    /// equals `LayoutCount`); JS heap metrics are not reported.
    #[cfg(feature = "render")]
    fn performance_metrics(&mut self, cmd: &PageCommand<'_, '_>) -> CdpResult<serde_json::Value> {
        let pid = self.pages.get(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?.id;
        let page = self.thread.browser.page(pid).ok_or_else(|| CdpError::server("page is gone"))?;
        let nodes = page.main_frame.page_dom.as_ref().map_or(0, |d| d.dom.borrow().live_nodes());
        let frames = self.thread.browser.frames(pid).len();
        let layouts = lp_layout::peek_layout_count();
        let phases = lp_render::blitz_phases();
        let metrics = [
            ("Timestamp", self.thread.browser.timestamp()),
            ("Documents", frames as f64),
            ("Frames", frames as f64),
            ("Nodes", nodes as f64),
            ("LayoutCount", layouts as f64),
            ("RecalcStyleCount", layouts as f64),
            ("LayoutDuration", phases[4] / 1000.0),
            ("RecalcStyleDuration", phases[0] / 1000.0),
        ];
        let metrics: Vec<_> =
            metrics.iter().map(|(name, value)| serde_json::json!({ "name": name, "value": value })).collect();
        Ok(serde_json::json!({ "metrics": metrics }))
    }

    /// Headless window metadata. Explicit bounds are recorded separately from
    /// the native viewport controlled by Emulation device metrics.
    fn window_domain(&mut self, action: &str, cmd: &PageCommand<'_, '_>) -> CdpResult<Option<serde_json::Value>> {
        let state = self.pages.get_mut(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?;
        let viewport = self.thread.browser.page(state.id).ok_or_else(|| CdpError::server("page is gone"))?.viewport();
        let (width, height) = (viewport.width, viewport.height);
        match action {
            "getWindowForTarget" => {
                let bounds = state.window_bounds.clone().unwrap_or_else(|| {
                    serde_json::json!({ "left": 0, "top": 0, "width": width, "height": height, "windowState": "normal" })
                });
                Ok(Some(serde_json::json!({ "windowId": cmd.page, "bounds": bounds })))
            }
            "setWindowBounds" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct P {
                    window_id: u64,
                    bounds: serde_json::Map<String, serde_json::Value>,
                }
                let p: P = cmd.command.params()?;
                if p.window_id != cmd.page {
                    return Err(CdpError::server("Browser window not found"));
                }
                // Merge like Chrome: unspecified fields keep their values.
                let mut bounds = state.window_bounds.clone().unwrap_or_else(|| {
                    serde_json::json!({ "left": 0, "top": 0, "width": width, "height": height, "windowState": "normal" })
                });
                for (k, v) in p.bounds {
                    if matches!(k.as_str(), "left" | "top" | "width" | "height" | "windowState") {
                        bounds[k] = v;
                    }
                }
                state.window_bounds = Some(bounds);
                Ok(Some(serde_json::json!({})))
            }
            _ => Err(CdpError::unsupported(&cmd.command.method)),
        }
    }

    fn emulation_domain(&mut self, action: &str, cmd: &PageCommand<'_, '_>) -> CdpResult<Option<serde_json::Value>> {
        match action {
            // Headless pages always behave as focused; there is no window focus to emulate.
            "setFocusEmulationEnabled" => Ok(Some(serde_json::json!({}))),
            // puppeteer's setViewport sends it with every viewport. Pages have
            // no touch input, which is what `enabled: false` asks for.
            "setTouchEmulationEnabled" => {
                #[derive(Deserialize)]
                struct P {
                    enabled: bool,
                }
                if cmd.command.params::<P>()?.enabled {
                    return Err(CdpError::unsupported("Emulation.setTouchEmulationEnabled enabled: true"));
                }
                Ok(Some(serde_json::json!({})))
            }
            "setDeviceMetricsOverride" | "clearDeviceMetricsOverride" => self.device_metrics(action, cmd),
            // Each call replaces the whole emulation (as Chrome): the media
            // type and the `prefers-*` features it lists; the rest reset.
            "setEmulatedMedia" => {
                #[derive(Deserialize)]
                struct Feature {
                    name: String,
                    value: String,
                }
                #[derive(Deserialize)]
                struct P {
                    media: Option<String>,
                    #[serde(default)]
                    features: Vec<Feature>,
                }
                let raw: serde_json::Value = cmd.command.params()?;
                let p: P = serde_json::from_value(raw.clone()).map_err(|e| CdpError::invalid_params(e.to_string()))?;
                let mut emulation = metrics::MediaEmulation::default();
                match p.media.as_deref().map(str::trim) {
                    None | Some("" | "screen") => {}
                    Some("print") => emulation.print = true,
                    Some(other) => return Err(CdpError::invalid_params(format!("Invalid media type '{other}'"))),
                }
                for f in &p.features {
                    match (f.name.as_str(), f.value.trim()) {
                        ("prefers-color-scheme", "dark") => emulation.color_scheme = lp_dom::media::ColorScheme::Dark,
                        ("prefers-color-scheme", "light" | "no-preference" | "") => {}
                        ("prefers-reduced-motion", "reduce") => emulation.reduced_motion = true,
                        ("prefers-reduced-motion", "no-preference" | "") => {}
                        // Other features (forced-colors, contrast, ...) are
                        // not evaluated by the media engine; accept them.
                        _ => {}
                    }
                }
                self.apply_media_emulation(cmd.page, emulation)?;
                if let Some(state) = self.pages.get_mut(&cmd.page) {
                    state.emulated_media = Some(raw);
                }
                Ok(Some(serde_json::json!({})))
            }
            "setUserAgentOverride" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct P {
                    user_agent: String,
                    accept_language: Option<String>,
                    platform: Option<String>,
                    user_agent_metadata: Option<serde_json::Value>,
                }
                let p: P = cmd.command.params()?;
                if p.user_agent_metadata.is_some() {
                    return Err(CdpError::unsupported("Emulation user agent client hints"));
                }
                // Empty UA alone resets the override; with locale/platform it
                // retains the browser's default UA while applying those fields.
                let override_ = if p.user_agent.is_empty() && p.accept_language.is_none() && p.platform.is_none() {
                    None
                } else {
                    Some(lp_webapi::UserAgentOverride {
                        user_agent: p.user_agent,
                        accept_language: p.accept_language,
                        platform: p.platform,
                    })
                };
                let state = self.pages.get_mut(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?;
                let previous = std::mem::replace(&mut state.user_agent_override, override_);
                self.apply_identity(cmd.page).inspect_err(|_| {
                    if let Some(state) = self.pages.get_mut(&cmd.page) {
                        state.user_agent_override = previous;
                    }
                })?;
                Ok(Some(serde_json::json!({})))
            }
            // Recorded, not applied to Intl/Date: JavaScriptCore uses the
            // process locale and time zone. navigator.language(s) and
            // Accept-Language follow the locale unless an explicit
            // acceptLanguage override is active. Empty or absent resets.
            "setLocaleOverride" => {
                #[derive(Deserialize)]
                struct P {
                    locale: Option<String>,
                }
                let p: P = cmd.command.params()?;
                let state = self.pages.get_mut(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?;
                let locale = p.locale.filter(|l| !l.is_empty());
                if locale.as_deref().is_some_and(|l| {
                    lp_net::http::HeaderValue::from_str(l).is_err()
                        || !l.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
                }) {
                    return Err(CdpError::invalid_params("Invalid locale name"));
                }
                state.locale_override = locale;
                self.apply_identity(cmd.page)?;
                Ok(Some(serde_json::json!({})))
            }
            // Screenshots paint this under the document instead of white.
            "setDefaultBackgroundColorOverride" => {
                #[derive(Deserialize)]
                struct Rgba {
                    r: f64,
                    g: f64,
                    b: f64,
                    a: Option<f64>,
                }
                #[derive(Deserialize)]
                struct P {
                    color: Option<Rgba>,
                }
                let p = cmd.command.params::<Option<P>>()?;
                let byte = |v: f64| v.clamp(0.0, 255.0).round() as u8;
                let color = p
                    .and_then(|p| p.color)
                    .map(|c| [byte(c.r), byte(c.g), byte(c.b), byte(c.a.unwrap_or(1.0).clamp(0.0, 1.0) * 255.0)]);
                let state = self.pages.get_mut(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?;
                state.background_override = color;
                Ok(Some(serde_json::json!({})))
            }
            "setTimezoneOverride" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct P {
                    timezone_id: String,
                }
                let p: P = cmd.command.params()?;
                // Without a time zone database only obviously malformed IDs
                // are rejected (Chrome checks the ID against ICU).
                let valid = |id: &str| {
                    id.starts_with(|c: char| c.is_ascii_alphabetic())
                        && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-' | '+'))
                        && !id.contains("//")
                        && !id.ends_with('/')
                };
                if !p.timezone_id.is_empty() && !valid(&p.timezone_id) {
                    return Err(CdpError::invalid_params(format!("Invalid timezone id: {}", p.timezone_id)));
                }
                let state = self.pages.get_mut(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?;
                state.timezone_override = Some(p.timezone_id).filter(|t| !t.is_empty());
                Ok(Some(serde_json::json!({})))
            }
            _ => Err(CdpError::unsupported(&cmd.command.method)),
        }
    }
}

impl EngineHost {
    /// Hands lp-browser the page's navigator identity: the user agent
    /// override, with the locale override as Accept-Language when the
    /// override has none.
    fn apply_identity(&mut self, page: PageKey) -> CdpResult<()> {
        let state = self.pages.get(&page).ok_or_else(|| CdpError::server("page is gone"))?;
        let mut value = state.user_agent_override.clone();
        if let Some(locale) = &state.locale_override {
            let value = value.get_or_insert_with(|| lp_webapi::UserAgentOverride {
                user_agent: String::new(),
                accept_language: None,
                platform: None,
            });
            value.accept_language.get_or_insert_with(|| locale.clone());
        }
        self.thread.browser.set_user_agent_override(state.id, value).map_err(|error| match error {
            lp_js::JsError::Type(message) => CdpError::invalid_params(message),
            other => remote::js_to_cdp(other),
        })
    }
}

impl Host for EngineHost {
    fn create_context(&mut self) -> CdpResult<ContextKey> {
        let id = self.thread.browser.create_context().map_err(|e| CdpError::server(e.to_string()))?;
        let key = u64::from(id.0);
        self.contexts.insert(key, network::NetworkContext::new(id));
        Ok(key)
    }

    fn dispose_context(&mut self, context: ContextKey) {
        let pages: Vec<PageKey> = self.pages.iter().filter(|(_, p)| p.context == context).map(|(k, _)| *k).collect();
        for page in pages {
            self.close_page(page);
        }
        if let Some(context) = self.contexts.remove(&context) {
            self.thread.browser.close_context(context.id);
        }
    }

    fn set_initial_popup(&mut self, page: PageKey, initial: bool) {
        if let Some(state) = self.pages.get_mut(&page) {
            state.initial_popup = initial;
        }
    }

    fn take_popup_requests(&mut self) -> Vec<crate::target::PopupRequest> {
        std::mem::take(&mut self.popup_requests)
    }

    fn popup_opened(&mut self, opener: PageKey, page: PageKey, target_id: &str) {
        if self.pages.get(&opener).is_some_and(|state| state.network.log.is_some()) {
            if let Some(state) = self.pages.get_mut(&page) {
                state.network.log.get_or_insert_with(Default::default);
            }
            self.update_network_subscriptions();
        }
        self.popups.push(PopupRecord { opener, page, target_id: target_id.to_owned() });
        if self.popups.len() > 256 {
            self.popups.remove(0);
        }
        self.waiters.log_changed = true;
    }

    fn create_page(&mut self, context: ContextKey) -> CdpResult<(PageKey, String)> {
        let context_id = self
            .contexts
            .get(&context)
            .ok_or_else(|| CdpError::invalid_params("No browser context with the given id found"))?
            .id;
        let id = self
            .thread
            .browser
            .create_page_in(context_id)
            .map_err(|e| CdpError::server(format!("cannot create page: {e}")))?;
        let (key, target_id) = self.register_page(id, context);
        // Events of the initial about:blank load precede any session.
        self.flush();
        Ok((key, target_id))
    }

    fn take_prebooted(&mut self) -> Vec<crate::target::PrebootedEntry> {
        std::mem::take(&mut self.prebooted)
    }

    fn navigate(&mut self, page: PageKey, url: &str) -> CdpResult<()> {
        let id = self.pages.get(&page).ok_or_else(|| CdpError::server("page is gone"))?.id;
        let result = self.thread.browser.navigate(id, lp_browser::NavigateOpts::address_bar(url));
        self.thread.browser.checkpoint();
        self.flush();
        result.map(|_| ()).map_err(|e| CdpError::server(format!("navigation failed: {e}")))
    }

    fn close_page(&mut self, page: PageKey) {
        let Some(state) = self.pages.remove(&page) else { return };
        for session in &state.sessions {
            self.pending.cancel_session(session);
            if let Some(session) = self.sessions.remove(session) {
                session.handles.borrow_mut().clear();
            }
        }
        self.renders.forget(state.id);
        self.thread.browser.close_page(state.id);
        self.update_network_subscriptions();
        // Realm/frame teardown events have no session left to go to.
        self.flush();
    }

    fn page_url(&self, page: PageKey) -> String {
        self.pages
            .get(&page)
            .and_then(|s| self.thread.browser.page(s.id))
            .map_or_else(|| "about:blank".to_owned(), |p| p.main_frame.url.clone())
    }

    fn page_title(&self, page: PageKey) -> String {
        let Some(state) = self.pages.get(&page) else { return String::new() };
        let page = self.thread.browser.page(state.id);
        page.and_then(|p| p.main_frame.page_dom.as_ref().map(|d| d.document_title())).unwrap_or_default()
    }

    fn session_attached(&mut self, page: PageKey, session_id: &str) {
        if let Some(state) = self.pages.get_mut(&page) {
            state.sessions.push(session_id.to_owned());
        }
        self.sessions.insert(
            session_id.to_owned(),
            Session {
                page_enabled: false,
                dom_enabled: false,
                lifecycle_events: false,
                runtime_enabled: false,
                network_enabled: false,
                extra_headers: http::HeaderMap::new(),
                cache_disabled: false,
                intercept_file_chooser: false,
                bindings: Vec::new(),
                fetch: fetch::FetchSession::default(),
                handles: Rc::new(RefCell::new(HandleTable::new(self.handle_budget.clone()))),
            },
        );
    }

    fn page_loaded(&self, page: PageKey) -> bool {
        let Some(state) = self.pages.get(&page) else { return false };
        let Some(p) = self.thread.browser.page(state.id) else { return false };
        let navigating = state.frames.get(&p.main_frame.id).is_some_and(|f| f.navigating);
        // A prepared page always has a URL to go to: on `about:blank` it has
        // not got there (the navigation failed, or never started).
        p.main_frame.loaded && !navigating && p.pending_loader().is_none() && p.main_frame.url != "about:blank"
    }

    fn page_needs_navigation(&self, page: PageKey) -> bool {
        let Some(state) = self.pages.get(&page) else { return false };
        let Some(p) = self.thread.browser.page(state.id) else { return false };
        let navigating = state.frames.get(&p.main_frame.id).is_some_and(|f| f.navigating);
        p.main_frame.url == "about:blank" && !navigating && p.pending_loader().is_none()
    }

    fn add_init_script(&mut self, page: PageKey, source: &str) {
        let Some(state) = self.pages.get_mut(&page) else { return };
        let id = format!("prepared-{}", state.next_script_id);
        state.next_script_id += 1;
        let page_id = state.id;
        let Some(context) = self.thread.browser.page(page_id).map(|p| p.context) else { return };
        self.thread
            .browser
            .add_context_init_script(context, lp_browser::InitScript { id, source: source.to_owned(), world: None });
    }

    fn settle_page(&mut self, page: PageKey) {
        if self.pages.contains_key(&page) {
            self.thread.browser.engine.settle();
        }
    }

    fn set_viewport(&mut self, page: PageKey, width: u32, height: u32) {
        let Some(state) = self.pages.get(&page) else { return };
        let Some(p) = self.thread.browser.page(state.id) else { return };
        let mut viewport = p.viewport().clone();
        viewport.width = width;
        viewport.height = height;
        let _ = self.thread.browser.set_viewport(state.id, viewport);
    }

    fn session_detached(&mut self, page: PageKey, session_id: &str) {
        if let Some(state) = self.pages.get_mut(&page) {
            state.sessions.retain(|s| s != session_id);
        }
        // No reply may reach a detached session; its handles go away even if
        // pending promise reactions are still referenced by JavaScript.
        self.pending.cancel_session(session_id);
        self.disable_fetch(session_id);
        if let Some(session) = self.sessions.remove(session_id) {
            session.handles.borrow_mut().clear();
        }
        self.update_runtime_wanted();
        self.update_network_subscriptions();
        self.apply_network_overrides(page);
    }

    fn page_command(&mut self, cmd: PageCommand<'_, '_>, _out: &mut dyn Sink) {
        let result = self.page_command_result(&cmd);
        // Synchronous inspector JS runs outside loop tasks. Release command
        // borrows before starting DOM resources and dispatching native events.
        // Handle releases run no JavaScript; skip the checkpoint for them.
        if !matches!(cmd.command.method.as_ref(), "Runtime.releaseObject" | "Runtime.releaseObjectGroup") {
            self.thread.browser.checkpoint();
        }
        // Events produced while handling precede the response (Chrome order).
        self.flush();
        match result {
            Ok(Some(value)) => self.reply(&cmd, &value),
            // The response is sent later (navigation commit, awaited promise).
            Ok(None) => {}
            Err(error) => self.out.push(msg::error_frame(Some(cmd.command.id), &error, Some(cmd.session_id))),
        }
    }

    fn browser_command(&mut self, command: &Command<'_>, context: Option<ContextKey>, out: &mut dyn Sink) {
        match command.method.as_ref() {
            // Puppeteer's acceptInsecureCerts sends this on the browser
            // session before any target is attached; it covers every context.
            "Security.enable" | "Security.disable" => {
                out.send(msg::result_frame(command.id, &Empty {}, command.session_id()))
            }
            "Security.setIgnoreCertificateErrors" => match command.params::<network::IgnoreCertificateErrors>() {
                Ok(p) => {
                    self.thread.browser.set_ignore_certificate_errors(p.ignore);
                    out.send(msg::result_frame(command.id, &Empty {}, command.session_id()))
                }
                Err(e) => out.send(msg::error_frame(Some(command.id), &e, command.session_id())),
            },
            "Storage.getCookies" | "Storage.setCookies" | "Storage.clearCookies" => {
                match self.storage_command(command, context) {
                    Ok(result) => out.send(msg::result_frame(command.id, &result, command.session_id())),
                    Err(error) => out.send(msg::error_frame(Some(command.id), &error, command.session_id())),
                }
            }
            "Browser.setDownloadBehavior" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct P {
                    behavior: String,
                    download_path: Option<String>,
                    #[serde(default)]
                    events_enabled: bool,
                }
                let result = command.params::<P>().and_then(|p| match p.behavior.as_str() {
                    "deny" | "default" | "allow" | "allowAndName" => {
                        if matches!(p.behavior.as_str(), "allow" | "allowAndName") && p.download_path.is_none() {
                            return Err(CdpError::invalid_params("downloadPath is required for allow behaviors"));
                        }
                        self.download_behavior.insert(
                            context,
                            DownloadBehavior {
                                behavior: p.behavior,
                                download_path: p.download_path,
                                events_enabled: p.events_enabled,
                            },
                        );
                        Ok(())
                    }
                    _ => Err(CdpError::invalid_params("Unrecognized download behavior")),
                });
                match result {
                    Ok(()) => out.send(msg::result_frame(command.id, &Empty {}, command.session_id())),
                    Err(e) => out.send(msg::error_frame(Some(command.id), &e, command.session_id())),
                }
            }
            _ => out.send(msg::error_frame(
                Some(command.id),
                &CdpError::unsupported(&command.method),
                command.session_id(),
            )),
        }
    }
}
