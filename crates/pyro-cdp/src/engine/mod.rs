//! CDP domains backed by the real engine (`pyro-browser` + `pyro-js` on
//! JavaScriptCore). [`EngineHost`] implements [`Host`] on the browser thread.
//!
//! Engine notifications (page events, console calls, uncaught exceptions)
//! are queued by [`Collector`] while engine code runs and translated into CDP
//! events by [`EngineHost::flush`] afterwards, so no CDP state is borrowed
//! while JavaScript is on the stack.

#[cfg(feature = "render")]
mod accessibility;
mod aria_snapshot;
mod blitz_geometry;
mod coverage;
mod fn_names;
#[cfg(feature = "render")]
pub(crate) use blitz_geometry::set_fork_template;
mod dialog;
mod dom;
mod driver;
mod fetch;
mod input;
mod metrics;
mod mocks;
mod network;
mod network_delivery;
mod page;
mod permissions;
mod remote;
mod runtime;
#[cfg(feature = "render")]
mod screenshot;
mod virtual_time;
mod worker;
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
        pub(super) fn forget(&mut self, _page: pyro_browser::PageId) {}
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

use pyro_browser::{BrowserThread, Frame, FrameId, PageEvent, PageId, PageObserver};
use pyro_js::{ConsoleLevel, EngineHooks, JsException, Persistent, Realm, RealmId, Value};

use serde::Deserialize;

use crate::msg::{self, CdpError, CdpResult, Command, Empty};
use crate::outbox::Outbox;
use crate::runtime_protocol::{HandleBudget, HandleTable};
use crate::target::{ContextKey, Host, PageCommand, PageKey, Sink};
use crate::ws::OutFrame;

pub use dialog::{DialogPump, PumpStep};
pub(crate) use driver::RunningCommand;

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
        /// The page the realm's window belongs to ([`realm_page`]).
        page: Option<PageId>,
        level: ConsoleLevel,
        args: Vec<Persistent>,
        timestamp: f64,
        /// The caller's stack (JavaScriptCore format).
        stack: Option<String>,
    },
    Exception {
        realm: RealmId,
        page: Option<PageId>,
        exception: JsException,
        timestamp: f64,
    },
    BindingCalled {
        realm: RealmId,
        page: Option<PageId>,
        name: String,
        payload: String,
    },
    /// A request announcement or response delivered on the browser thread.
    Network(ContextKey, network_delivery::Observed),
    Worker(PageId, pyro_webapi::WorkerEvent),
}

/// Most engine notifications queued between flushes. A page producing more
/// within one task (e.g. a tight console.log loop) exceeds the connection's
/// budget: the connection is closed (1008) rather than growing without bound.
pub(crate) const MAX_QUEUED_EVENTS: usize = 100_000;

struct NetworkWatch {
    context: ContextKey,
    id: pyro_browser::ContextId,
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
    /// Blocking JavaScript dialogs (`dialog.rs`).
    pub(crate) dialogs: dialog::DialogGate,
}

impl Collector {
    fn new(out: Outbox) -> Self {
        Self {
            queue: RefCell::new(VecDeque::new()),
            out,
            runtime_wanted: Cell::new(false),
            network: RefCell::new(Vec::new()),
            network_budget: Arc::new(network_delivery::Budget::default()),
            dialogs: dialog::DialogGate::default(),
        }
    }

    pub(crate) fn watch_network(
        &self,
        context: ContextKey,
        id: pyro_browser::ContextId,
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

    fn on_worker_event(&self, page: PageId, event: pyro_webapi::WorkerEvent) {
        self.push(Queued::Worker(page, event));
    }

    /// A fetch/XHR consumer is about to let script observe `frontier` of
    /// `request`: the matching staged response events become deliverable and
    /// are queued now, ahead of anything that script causes (Chrome reports
    /// them before the script sees the response, not when I/O received it).
    fn on_network_delivery(
        &self,
        _page: PageId,
        context: pyro_browser::ContextId,
        loader: &pyro_browser::LoaderId,
        request: pyro_net::RequestId,
        frontier: pyro_net::NetworkDelivery,
    ) {
        let inbox = self.network.borrow().iter().find(|w| w.id == context).and_then(|w| w.inbox.upgrade());
        if let Some(inbox) = inbox {
            inbox.publish_through(request, &loader.0, frontier.response, frontier.data_bytes, frontier.terminal);
            self.drain_network();
        }
    }

    fn javascript_dialog(
        &self,
        page: PageId,
        frame: &FrameId,
        url: &str,
        kind: &str,
        message: &str,
        default_prompt: Option<&str>,
    ) -> Option<pyro_webapi::DialogAnswer> {
        self.dialogs.show(&self.out, page, &frame.0, url, kind, message, default_prompt.unwrap_or_default())
    }
}

/// The page of a realm's window, read from the realm itself.
fn realm_page(realm: &Realm) -> Option<PageId> {
    pyro_browser::realm_page(&realm.scope().ok()?)
}

impl EngineHooks for Collector {
    fn console_api(&self, realm: &Realm, level: ConsoleLevel, args: &[Value<'_>]) {
        if !self.runtime_wanted.get() || self.out.is_exceeded() {
            return;
        }
        let Ok(scope) = realm.scope() else { return };
        let args = args.iter().map(|v| Persistent::new(&scope, *v)).collect();
        // Chrome reports the caller's stack with every console call
        // (Playwright's ConsoleMessage.location() reads it).
        let stack = scope.current_stack().ok();
        let page = pyro_browser::realm_page(&scope);
        self.push(Queued::Console { realm: realm.id(), page, level, args, timestamp: epoch_ms(), stack });
    }

    fn exception_thrown(&self, realm: &Realm, exception: &JsException) {
        if !self.runtime_wanted.get() {
            return;
        }
        let page = realm_page(realm);
        self.push(Queued::Exception { realm: realm.id(), page, exception: exception.clone(), timestamp: epoch_ms() });
    }

    // Only pages a session added a binding to call this.
    fn binding_called(&self, realm: &Realm, name: &str, payload: &str) {
        let page = realm_page(realm);
        self.push(Queued::BindingCalled {
            realm: realm.id(),
            page,
            name: name.to_owned(),
            payload: payload.to_owned(),
        });
    }

    fn unhandled_rejection(&self, realm: &Realm, exception: &JsException) {
        if !self.runtime_wanted.get() {
            return;
        }
        let page = realm_page(realm);
        self.push(Queued::Exception { realm: realm.id(), page, exception: exception.clone(), timestamp: epoch_ms() });
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
    /// `Network.setBypassServiceWorker` of this session (page-scoped in Chrome).
    pub(crate) bypass_service_worker: bool,
    /// Page.setInterceptFileChooserDialog: file inputs report
    /// Page.fileChooserOpened to this session.
    pub(crate) intercept_file_chooser: bool,
    /// Runtime.addBinding names: their calls are reported to this session.
    pub(crate) bindings: Vec<String>,
    fetch: fetch::FetchSession,
    pub(crate) handles: Rc<remote::Handles>,
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
    user_agent_override: Option<pyro_webapi::UserAgentOverride>,
    /// Emulation.setLocaleOverride. Intl and Date keep the process locale.
    pub(crate) locale_override: Option<String>,
    /// Emulation.setTimezoneOverride. It applies to the connection's whole VM
    /// (`EngineHost::timezone`), so all pages that set one agree.
    pub(crate) timezone_override: Option<String>,
    /// Emulation.setDefaultBackgroundColorOverride (RGBA), for screenshots.
    pub(crate) background_override: Option<[u8; 4]>,
    pub(crate) next_script_id: u64,
    /// Paused popup before its first navigation (`Page.getFrameTree` url ":").
    pub(crate) initial_popup: bool,
    /// This page is a `window.open` popup; dropped with the page, which tells
    /// the opener's object it closed.
    pub(crate) popup_ticket: Option<std::rc::Rc<pyro_webapi::PopupTicket>>,
    /// A raw keydown which changes documents suppresses one following char.
    /// Keyboard input is page-scoped, including across attached sessions.
    pub(crate) suppress_next_char: bool,
    /// Input.setInterceptDrags: native drags started by mouse input are
    /// reported through Input.dragIntercepted.
    pub(crate) intercept_drags: bool,
    /// Input.dispatchTouchEvent: the touches down, and the frame document
    /// the sequence's first touch landed in with its offset in the page's
    /// viewport (all touches of a sequence go to that document).
    pub(crate) touches: pyro_webapi::input::TouchSequence,
    pub(crate) touch_document: Option<(Rc<pyro_webapi::PageDom>, f64, f64)>,
    network: network::NetworkPage,
    dom_nodes: dom::BackendNodes,
    /// Downloads the page started, newest last (`Pyro.run` `download` steps).
    pub(crate) downloads: Vec<DownloadRecord>,
    pub(crate) download_count: u64,
}

/// A download a page started, kept for `Pyro.run` whatever the
/// `Browser.setDownloadBehavior` setting.
#[derive(Clone, Debug)]
pub(crate) struct DownloadRecord {
    pub url: String,
    pub filename: String,
    pub body: Result<bytes::Bytes, String>,
}

/// Bytes of download bodies kept per page; older ones lose their body.
pub(crate) const DOWNLOAD_BUDGET: usize = 64 * 1024 * 1024;

/// A popup that became a target, for `Pyro.run` `popup` steps.
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
    /// Commands from `window.open`'s returned objects, resolved to pages when
    /// the protocol layer takes them (after adopting the popups they name).
    pub(crate) popup_commands: Vec<(std::rc::Rc<pyro_webapi::PopupState>, pyro_webapi::PopupCommand)>,
    /// Live dedicated workers and what the target layer has not heard yet (`worker.rs`).
    workers: HashMap<crate::target::WorkerKey, pyro_webapi::WorkerHandle>,
    worker_changes: Vec<crate::target::WorkerChange>,
    /// Pages `Page.close` closes, taken by the protocol layer after `flush`.
    pub(crate) close_requests: Vec<PageKey>,
    /// `browserContextId`s of the contexts (a dialog's context disposed
    /// dismisses it).
    context_names: HashMap<ContextKey, String>,
    /// Pages booted before the connection (`preboot`), handed to the
    /// protocol layer once by `Host::take_prebooted`.
    prebooted: Vec<crate::target::PrebootedEntry>,
    /// Popups that became targets, oldest first (`Pyro.run` `popup` steps).
    pub(crate) popups: Vec<PopupRecord>,
    /// `Pyro.act` / `Pyro.expect` commands waiting for their page to change.
    waiters: driver::Waiters,
    /// The command running now, for `Pyro.cancelRun` while its script runs.
    running: Rc<driver::RunningCommand>,
    /// Emulation.setVirtualTimePolicy (none until a page sets one).
    virtual_time: Option<virtual_time::VirtualTime>,
    virtual_ticks_base: f64,
    /// The time zone in effect in this connection's VM: that of the pages
    /// which set one (None: the host's).
    timezone: Option<String>,
    /// JavaScript and CSS coverage of the pages (`coverage.rs`).
    coverage: coverage::Coverage,
}

/// A page booted on a thread before any connection, with its context.
#[derive(Debug, Clone, Copy)]
pub struct Prebooted {
    pub context: pyro_browser::ContextId,
    pub page: pyro_browser::PageId,
    pub profile: usize,
}

/// Startup configuration shared by every browser thread of a `serve`
/// process (built once from the command line).
#[derive(Clone, Debug, Default)]
pub struct BrowserProfile {
    /// Network client configuration of every browser context.
    pub net: pyro_net::NetConfig,
    /// Local storage entries every browser context starts with.
    pub local_storage: pyro_webapi::StorageSeed,
    /// Pages booted ahead of every connection (`serve --prepare URL`).
    pub prepared: Vec<crate::target::PreparedProfile>,
}

impl EngineHost {
    /// What can still change `page` now (`serve --fork` waits for quiet
    /// pages before forking); `None` once it is gone.
    pub fn page_work(&self, page: PageKey) -> Option<pyro_browser::PageWork> {
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
        Self::prepare_thread_with(&BrowserProfile::default(), true)
    }

    /// `prepare_thread` with the process's startup configuration.
    ///
    /// `spare_realm` also builds the first page's realm. That is over half of
    /// the work here, so only a thread prepared ahead of its connection asks
    /// for it: a thread started by its connection would do it before reading
    /// the first command, and a client that opens no page would pay for it.
    pub fn prepare_thread_with(profile: &BrowserProfile, spare_realm: bool) -> CdpResult<BrowserThread> {
        Self::prepare_thread_around(profile, spare_realm, None)
    }

    /// `prepare_thread_with` around the engine an earlier browser of this
    /// thread left ([`pyro_browser::BrowserThread::with_engine`]) when one is
    /// given. Everything but the engine is made anew: the browser, its loop,
    /// the network client with its cookie jar, cache and connections.
    pub fn prepare_thread_around(
        profile: &BrowserProfile,
        spare_realm: bool,
        engine: Option<pyro_js::Engine>,
    ) -> CdpResult<BrowserThread> {
        let mut thread = BrowserThread::with_engine(engine)
            .map_err(|e| CdpError::server(format!("JavaScript engine unavailable: {e}")))?;
        thread
            .enable_network(profile.net.clone())
            .map_err(|e| CdpError::server(format!("network unavailable: {e}")))?;
        #[cfg(feature = "render")]
        blitz_geometry::install(
            profile.net.static_files.clone(),
            thread.browser.context_net(pyro_browser::DEFAULT_CONTEXT).cloned(),
        );
        if !profile.local_storage.is_empty() {
            thread.set_local_storage_seed(profile.local_storage.clone());
        }
        if spare_realm {
            // Best effort: a failure here only costs the first page its head start.
            let _ = thread.browser.prepare_spare_realm();
        }
        Ok(thread)
    }

    /// After this thread's host and browser were dropped: forgets what the
    /// protocol layer kept per thread for their pages, and the renderer's
    /// fetcher.
    pub(crate) fn forget_thread_state() {
        aria_snapshot::forget_refs();
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
                        pyro_browser::InitScript { id: format!("prebooted-{n}"), source: source.clone(), world: None },
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
                if thread.browser.navigate(page, pyro_browser::NavigateOpts::address_bar(&profile.url)).is_err() {
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
                popup_ticket: None,
                suppress_next_char: false,
                intercept_drags: false,
                touches: Default::default(),
                touch_document: None,
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
            timezone: None,
            pages: HashMap::new(),
            sessions: HashMap::new(),
            next_exception_id: 1,
            handle_budget: HandleBudget::new(HandleBudget::DEFAULT_LIMIT),
            next_backend_node: 1,
            pending: runtime::PendingReplies::default(),
            popup_requests: Vec::new(),
            popup_commands: Vec::new(),
            workers: HashMap::new(),
            worker_changes: Vec::new(),
            close_requests: Vec::new(),
            context_names: HashMap::new(),
            prebooted: Vec::new(),
            popups: Vec::new(),
            waiters: driver::Waiters::default(),
            running: Rc::default(),
            virtual_time: None,
            virtual_ticks_base: 0.0,
            coverage: coverage::Coverage::default(),
        })
    }

    pub fn thread_mut(&mut self) -> &mut BrowserThread {
        &mut self.thread
    }

    /// The command running now, for the connection's watchdog check
    /// (`Pyro.cancelRun` while a script runs, conn.rs `check_during_script`).
    pub(crate) fn running_command(&self) -> Rc<RunningCommand> {
        self.running.clone()
    }

    /// Runs engine tasks and timers until `wake()` is true or engine
    /// notifications are waiting for [`EngineHost::flush`].
    pub fn run_until_work(
        &mut self,
        deadline: Option<std::time::Instant>,
        mut wake: impl FnMut() -> bool,
    ) -> pyro_runtime::RunOutcome {
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
        let mut ready = |b: &mut pyro_browser::Browser| {
            wake() || !collector.is_empty() || network_wake.ready() || renders.ready() || waiters.ready(b)
        };
        // Virtual time: instead of sleeping until a timer is due while a run
        // waits on a page that has nothing else to do, jump the page clock to
        // it (pyro_browser::clock).
        if pyro_browser::clock::virtual_time() {
            loop {
                if ready(&mut thread.browser) {
                    return pyro_runtime::RunOutcome::Done;
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
        // Emulation.setVirtualTimePolicy advancing: jump through timers
        // whenever nothing else is ready (virtual_time.rs).
        if self.virtual_time.as_ref().is_some_and(virtual_time::VirtualTime::advancing)
            && virtual_time::run(thread, &mut self.virtual_time, &self.out, &mut ready)
        {
            return pyro_runtime::RunOutcome::Done;
        }
        thread.event_loop.run_until(&mut thread.browser, deadline, false, ready)
    }

    /// Disposes every page and realm (connection teardown).
    pub fn close_all(&mut self) {
        self.pending.cancel_all();
        self.waiters.clear();
        self.collector.dialogs.driver_busy.set(false);
        self.renders.clear();
        self.sessions.clear();
        self.pages.clear();
        self.thread.browser.close_all();
        for (_, context) in self.contexts.drain() {
            self.thread.browser.close_context(context.id);
        }
        self.collector.clear();
    }

    /// Installs (or removes) the nested read loop blocking JavaScript dialogs
    /// run while they show (`serve` installs one per connection; without it
    /// dialogs never block). See `dialog.rs`.
    pub fn set_dialog_pump(&self, pump: Option<DialogPump>) {
        self.collector.dialogs.set_pump(pump);
    }

    /// Tells the dialog gate which sessions of `page` have Page enabled.
    pub(crate) fn sync_dialog_sessions(&self, page: PageKey) {
        let Some(state) = self.pages.get(&page) else { return };
        let sessions = self.sessions_of(page, |s| s.page_enabled);
        let target = self.thread.browser.page(state.id).map(|p| p.main_frame.id.0.clone()).unwrap_or_default();
        let context = self.context_names.get(&state.context).cloned().unwrap_or_default();
        self.collector.dialogs.set_sessions(state.id, target, context, sessions);
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

    /// The protocol key of an engine page: its id (`register_page`).
    pub(crate) fn page_key(&self, id: PageId) -> Option<PageKey> {
        let key = PageKey::from(id.0);
        debug_assert!(self.pages.get(&key).is_none_or(|p| p.id == id));
        self.pages.contains_key(&key).then_some(key)
    }

    pub(crate) fn realm(&self, page: PageKey, realm: RealmId) -> Option<Realm> {
        let id = self.pages.get(&page)?.id;
        self.thread.browser.realm_in_page(id, realm)
    }

    /// Reads the live frame of `page` showing `document`, without cloning it.
    fn with_frame_of_document<R>(
        &self,
        page: PageKey,
        document: &Rc<pyro_webapi::PageDom>,
        f: impl FnOnce(&Frame) -> R,
    ) -> Option<R> {
        let id = self.pages.get(&page)?.id;
        self.thread.browser.with_frame_of_document(id, document, f)
    }

    fn owns_frame(&self, page: PageId, frame: Option<&String>) -> bool {
        frame.is_some_and(|id| self.thread.browser.has_frame(page, id))
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

    /// One event to several sessions: `params` is serialized once and its
    /// JSON text copied into each session's frame.
    pub(crate) fn event_to<T: serde::Serialize + ?Sized>(&self, method: &str, params: &T, sessions: &[String]) {
        match sessions {
            [] => {}
            [one] => self.event(method, params, one),
            many => {
                let raw = serde_json::value::to_raw_value(params).expect("CDP params serialize");
                for session_id in many {
                    self.event(method, &*raw, session_id);
                }
            }
        }
    }

    /// Translates queued engine notifications into CDP events, in order.
    pub fn flush(&mut self) {
        self.flush_renders();
        self.service_waiters();
        self.drain_collector();
        self.flush_network();
        self.flush_fetch();
        self.poll_coverage();
        // `Pyro.run` request steps wait on the log these observations fill.
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
                Queued::Console { realm, page, level, args, timestamp, stack } => {
                    if let Some(page) = self.page_of_realm(realm, page) {
                        self.on_console(page, realm, level, args, timestamp, stack)
                    }
                }
                Queued::Exception { realm, page, exception, timestamp } => {
                    if let Some(page) = self.page_of_realm(realm, page) {
                        self.on_exception(page, realm, exception, timestamp)
                    }
                }
                Queued::BindingCalled { realm, page, name, payload } => {
                    if let Some(page) = self.page_of_realm(realm, page) {
                        self.on_binding_called(page, realm, name, payload)
                    }
                }
                Queued::Worker(page, event) => {
                    let key = self.page_key(page);
                    self.on_worker_event(key, event);
                }
            }
        }
    }

    /// Page owning `realm` (in a live frame), if any. `hint` is the page the
    /// realm's window named when the notification was queued; realms without
    /// one (none are expected) fall back to asking every page.
    pub(crate) fn page_of_realm(&self, realm: RealmId, hint: Option<PageId>) -> Option<PageKey> {
        let browser = &self.thread.browser;
        let owns = |key: &PageKey| self.pages.get(key).is_some_and(|p| browser.has_live_realm(p.id, realm));
        match hint {
            Some(page) => self.page_key(page).filter(owns),
            None => self.pages.keys().copied().find(owns),
        }
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
            "Accessibility" => self.accessibility_domain(action, cmd),
            "Profiler" => self.profiler_domain(action, cmd),
            "Debugger" => self.debugger_domain(action, cmd),
            "CSS" => self.css_domain(action, cmd),
            // Blitz-backed layout queries since the last call.
            #[cfg(feature = "render")]
            "Pyro" if action == "geometryStats" => {
                // The timers run from the first call on; its durations cover
                // only what ran since (callers reset with a first call).
                pyro_render::profiling::enable();
                let (layouts, deltas, snapshot_ms, layout_ms, read_ms) = blitz_geometry::take_stats();
                let (mirror_ms, settle_ms, resolve_ms) = pyro_render::sync_profile::take();
                // Blitz counters are cumulative on this browser thread. Unlike
                // the existing resettable fields, callers subtract snapshots.
                let phases = pyro_render::blitz_phases();
                let (inline_rebuilds, inline_bytes) = pyro_render::blitz_rebuilds();
                let style_stats = pyro_render::blitz_style_stats();
                let (resolves, layout_passes) = pyro_render::blitz_passes();
                let (rounded_nodes, rounding_skips) = pyro_render::blitz_rounding();
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
                        "restyledNodes": pyro_render::blitz_restyles(),
                        "resolves": resolves, "layoutPasses": layout_passes,
                        "roundedNodes": rounded_nodes, "roundingSkips": rounding_skips,
                        "styleStats": {
                            "styled": style_stats[0], "matched": style_stats[1],
                            "shared": style_stats[2], "reused": style_stats[3],
                        },
                    },
                })))
            }
            // What this connection's VM holds, for attributing the process's
            // "WebKit Malloc" from outside (`vmmap`, `footprint`, libpas's
            // status reporter). `action` first does one step of what the
            // server does on its own after closes, so that each step's share
            // can be read: `collect` (a full collection), `settle` (that, and
            // free pages returned), `deleteCode` (compiled and unlinked code
            // dropped first).
            "Pyro" if action == "jscMemory" => {
                #[derive(Deserialize)]
                struct P {
                    action: Option<String>,
                }
                let engine = &self.thread.browser.engine;
                match cmd.command.params::<P>()?.action.as_deref() {
                    None | Some("stats") => {}
                    Some("collect") => {
                        engine.collect_garbage_now();
                    }
                    Some("settle") => engine.settle(),
                    Some("deleteCode") => engine.settle_idle(),
                    Some(other) => return Err(CdpError::invalid_params(format!("unknown action: {other}"))),
                }
                let stats = engine
                    .memory_statistics_json()
                    .and_then(|json| serde_json::from_str::<serde_json::Value>(&json).ok())
                    .unwrap_or(serde_json::Value::Null);
                Ok(Some(serde_json::json!({
                    "processId": std::process::id(),
                    "footprint": pyro_js::process_footprint(),
                    "heap": stats,
                })))
            }
            "Pyro" => {
                // Dialogs opened by driver steps do not block (dialog.rs).
                self.collector.dialogs.driver_busy.set(true);
                let result = self.driver_domain(action, cmd);
                self.collector.dialogs.driver_busy.set(!self.waiters.is_empty());
                result
            }
            #[cfg(feature = "render")]
            "IO" => self.io_domain(action, cmd),
            "Log" if matches!(action, "enable" | "disable") => Ok(Some(serde_json::json!({}))),
            // puppeteer enables both on every page; there are no issues to
            // report.
            // `Performance.getMetrics` reports durations, timed from here on.
            #[cfg(feature = "render")]
            "Performance" if action == "enable" => {
                pyro_render::profiling::enable();
                Ok(Some(serde_json::json!({})))
            }
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
        let layouts = pyro_layout::peek_layout_count();
        let phases = pyro_render::blitz_phases();
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
            "setGeolocationOverride" | "clearGeolocationOverride" => self.geolocation_override(action, cmd),
            "setVirtualTimePolicy" => self.set_virtual_time_policy(cmd),
            // Touch input (Input.dispatchTouchEvent) is always accepted, as in
            // Chrome; this sets `navigator.maxTouchPoints` (default 1) and,
            // as Chrome does, makes the primary pointer coarse and unable to
            // hover (`(pointer: coarse)`, `(hover: none)`). puppeteer sends
            // `enabled: false` with every setViewport: a no-op unless enabled.
            "setTouchEmulationEnabled" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct P {
                    enabled: bool,
                    max_touch_points: Option<u32>,
                }
                let p = cmd.command.params::<P>()?;
                let points = if p.enabled { p.max_touch_points.unwrap_or(1) } else { 0 };
                if p.enabled && !(1..=16).contains(&points) {
                    return Err(CdpError::invalid_params("Touch points must be between 1 and 16"));
                }
                let id = self.pages.get(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?.id;
                self.thread.browser.set_max_touch_points(id, points);
                let mut viewport =
                    self.thread.browser.page(id).ok_or_else(|| CdpError::server("page is gone"))?.viewport().clone();
                let pointer = if p.enabled { pyro_dom::media::Pointer::Coarse } else { pyro_dom::media::Pointer::Fine };
                if viewport.pointer != pointer || viewport.hover == p.enabled {
                    viewport.pointer = pointer;
                    viewport.hover = !p.enabled;
                    self.thread.browser.set_viewport(id, viewport).map_err(remote::js_to_cdp)?;
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
                        ("prefers-color-scheme", "dark") => emulation.color_scheme = pyro_dom::media::ColorScheme::Dark,
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
                let metadata = p.user_agent_metadata.map(metrics::user_agent_metadata).transpose()?;
                if metadata.is_some() && p.user_agent.is_empty() {
                    return Err(CdpError::invalid_params("Empty userAgent invalid with userAgentMetadata provided"));
                }
                // Empty UA alone resets the override; with locale/platform it
                // retains the browser's default UA while applying those fields.
                let override_ = if p.user_agent.is_empty()
                    && p.accept_language.is_none()
                    && p.platform.is_none()
                    && metadata.is_none()
                {
                    None
                } else {
                    Some(pyro_webapi::UserAgentOverride {
                        user_agent: p.user_agent,
                        accept_language: p.accept_language,
                        platform: p.platform,
                        metadata,
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
                    pyro_net::http::HeaderValue::from_str(l).is_err()
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
                let requested = Some(p.timezone_id).filter(|t| !t.is_empty());
                self.pages.get(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?;
                // One VM per connection has one time zone. Not Chrome's "Timezone
                // override is already in effect": Playwright ignores that text, and
                // the page would silently run in the other page's zone.
                let others = self.pages.iter().filter(|(key, _)| **key != cmd.page);
                let other = others.filter_map(|(_, s)| s.timezone_override.clone()).next();
                if let (Some(id), Some(other)) = (&requested, &other) {
                    if id != other {
                        return Err(CdpError::server(format!(
                            "Cannot set timezone {id}: another page of this connection set {other}, \
                             and the time zone applies to the whole connection"
                        )));
                    }
                }
                let effective = requested.clone().or(other);
                self.apply_timezone(effective)?;
                let state = self.pages.get_mut(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?;
                state.timezone_override = requested;
                Ok(Some(serde_json::json!({})))
            }
            _ => Err(CdpError::unsupported(&cmd.command.method)),
        }
    }
}

impl EngineHost {
    /// Makes `id` the VM's time zone unless it already is: a change flushes
    /// JavaScriptCore's date caches and walks the heap's Date objects.
    fn apply_timezone(&mut self, id: Option<String>) -> CdpResult<()> {
        if self.timezone == id {
            return Ok(());
        }
        let known =
            self.thread.browser.engine.set_time_zone(id.as_deref()).map_err(|e| CdpError::server(e.to_string()))?;
        if !known {
            return Err(CdpError::invalid_params(format!("Invalid timezone id: {}", id.unwrap_or_default())));
        }
        self.timezone = id;
        Ok(())
    }

    /// Hands pyro-browser the page's navigator identity: the user agent
    /// override, with the locale override as Accept-Language when the
    /// override has none.
    fn apply_identity(&mut self, page: PageKey) -> CdpResult<()> {
        let state = self.pages.get(&page).ok_or_else(|| CdpError::server("page is gone"))?;
        let mut value = state.user_agent_override.clone();
        if let Some(locale) = &state.locale_override {
            let value = value.get_or_insert_with(|| pyro_webapi::UserAgentOverride {
                user_agent: String::new(),
                accept_language: None,
                platform: None,
                metadata: None,
            });
            value.accept_language.get_or_insert_with(|| locale.clone());
        }
        self.thread.browser.set_user_agent_override(state.id, value).map_err(|error| match error {
            pyro_js::JsError::Type(message) => CdpError::invalid_params(message),
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

    fn create_proxied_context(&mut self, proxy: crate::target::ContextProxy<'_>) -> CdpResult<ContextKey> {
        let config = pyro_net::ProxyConfig::parse(proxy.server, proxy.bypass_list).map_err(CdpError::server)?;
        let id = self.thread.browser.create_context_with_proxy(config).map_err(|e| CdpError::server(e.to_string()))?;
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

    fn take_close_requests(&mut self) -> Vec<PageKey> {
        std::mem::take(&mut self.close_requests)
    }

    fn take_popup_actions(&mut self) -> Vec<crate::target::PopupAction> {
        use crate::target::PopupAction;
        let commands = std::mem::take(&mut self.popup_commands);
        commands
            .into_iter()
            .filter_map(|(popup, command)| {
                let (&page, _) = self
                    .pages
                    .iter()
                    .find(|(_, state)| state.popup_ticket.as_ref().is_some_and(|t| t.is_for(&popup)))?;
                Some(match command {
                    pyro_webapi::PopupCommand::Close => PopupAction::Close(page),
                    pyro_webapi::PopupCommand::Navigate(url) => PopupAction::Navigate(page, url),
                })
            })
            .collect()
    }

    fn take_worker_changes(&mut self) -> Vec<crate::target::WorkerChange> {
        std::mem::take(&mut self.worker_changes)
    }

    fn worker_command(
        &mut self,
        worker: crate::target::WorkerKey,
        session_id: &str,
        command: &Command<'_>,
        _out: &mut dyn Sink,
    ) {
        self.worker_session_command(worker, session_id, command);
    }

    fn worker_session_detached(&mut self, worker: crate::target::WorkerKey, session_id: &str) {
        self.worker_session_gone(worker, session_id);
    }

    fn context_named(&mut self, key: ContextKey, id: &str) {
        self.context_names.insert(key, id.to_owned());
    }

    fn popup_opened(
        &mut self,
        opener: PageKey,
        page: PageKey,
        target_id: &str,
        handle: Option<std::rc::Rc<dyn std::any::Any>>,
    ) {
        let ticket: Option<std::rc::Rc<pyro_webapi::PopupTicket>> = handle.and_then(|h| h.downcast().ok());
        if let (Some(opener_id), Some(id)) =
            (self.pages.get(&opener).map(|s| s.id), self.pages.get(&page).map(|s| s.id))
        {
            self.thread.browser.adopt_popup(id, opener_id, ticket.clone());
        }
        if let Some(state) = self.pages.get_mut(&page) {
            state.popup_ticket = ticket;
        }
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
        let result = self.thread.browser.navigate(id, pyro_browser::NavigateOpts::address_bar(url));
        self.thread.browser.checkpoint();
        self.flush();
        result.map(|_| ()).map_err(|e| CdpError::server(format!("navigation failed: {e}")))
    }

    fn close_page(&mut self, page: PageKey) {
        self.coverage_page_closed(page);
        let Some(state) = self.pages.remove(&page) else { return };
        // The last page with an override gives the VM back its host zone.
        if state.timezone_override.is_some() && !self.pages.values().any(|s| s.timezone_override.is_some()) {
            let _ = self.apply_timezone(None);
        }
        for session in &state.sessions {
            self.pending.cancel_session(session);
            if let Some(session) = self.sessions.remove(session) {
                session.handles.borrow_mut().clear();
            }
        }
        self.renders.forget(state.id);
        self.collector.dialogs.forget(state.id);
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
                bypass_service_worker: false,
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
            .add_context_init_script(context, pyro_browser::InitScript { id, source: source.to_owned(), world: None });
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
        self.coverage_session_detached(page, session_id);
        if let Some(session) = self.sessions.remove(session_id) {
            session.handles.borrow_mut().clear();
        }
        self.update_runtime_wanted();
        self.update_network_subscriptions();
        self.apply_network_overrides(page);
        self.sync_dialog_sessions(page);
    }

    fn page_command(&mut self, cmd: PageCommand<'_, '_>, _out: &mut dyn Sink) {
        // A `Pyro.cancelRun` for this session stops a script it runs (or
        // the microtasks after it) that does not return.
        let _running = self.running.enter(cmd.session_id, cmd.command.id);
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
            "Browser.grantPermissions" | "Browser.resetPermissions" | "Browser.setPermission" => {
                let result = self.permissions_command(command, context);
                match result {
                    Ok(()) => out.send(msg::result_frame(command.id, &Empty {}, command.session_id())),
                    Err(e) => out.send(msg::error_frame(Some(command.id), &e, command.session_id())),
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
