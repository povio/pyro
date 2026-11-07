//! Browser/page orchestration on the browser thread.
//!
//! Ownership (see docs/ARCHITECTURE.md): this crate owns the Browser →
//! Page → Frame tree, navigation state machine, realms per (frame, world),
//! init-script injection, window timers, and the ordered [`PageEvent`] stream
//! consumed by the automation layer. Document loading uses `lp-dom`/`lp-net`;
//! `lp-webapi` supplies bindings in the main and isolated document worlds.

mod bindings;
pub mod clock;
mod focus;
mod frames;
mod images;
mod loader;
mod media;
mod nav;
mod network_idle;
mod preload;
mod quiet;
mod refresh;
mod stylesheets;
mod viewport;
mod window;

use lp_js::{Engine, EngineHooks, JsError, Realm, RealmId, RealmOptions};
use lp_runtime::{EventLoop, LoopHandle, LoopState, RunOutcome, Scheduler};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

pub use frames::{realm_page, FrameInfo};
use frames::{FrameHost, FrameKey};
pub use loader::{
    FetchDone, FetchRequest, NetLoader, Resource, ResourceCancellation, ResourceKind, ResourceLoader, StaticLoader,
};
pub use nav::{History, HistoryEntry, Location, PageNav, Performance};
pub use network_idle::NetworkIdle;
pub use quiet::{PageWork, TIMER_HORIZON};
pub use window::{note_client_activity, request_native_frame, Window, WindowData};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PageId(pub u32);

/// A browser context (CDP `browserContextId`): pages sharing network state
/// (own `lp_net::NetClient`, hence cookie jar). Context 0 is the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ContextId(pub u32);

pub const DEFAULT_CONTEXT: ContextId = ContextId(0);

struct ContextState {
    id: ContextId,
    loader: Option<Rc<dyn ResourceLoader>>,
    net: Option<lp_net::NetClient>,
    local_storage: lp_webapi::StorageNamespace,
    /// Permissions and clipboard its pages share.
    services: Rc<lp_webapi::ContextServices>,
    init_scripts: Vec<InitScript>,
}

/// CDP frame id (upstream uses a stable string per frame).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FrameId(pub String);

/// CDP loader id; changes for every cross-document navigation.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LoaderId(pub String);

/// Why a navigation started (CDP `Page.frameRequestedNavigation.reason` subset,
/// mirroring upstream `Notification.zig`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavigationReason {
    AddressBar,
    Anchor,
    FormGet,
    FormPost,
    Script,
    History,
    InitialFrameNavigation,
}

/// How a navigation's commit changes the session history (HTML "history
/// handling"), decided by the navigation's intent rather than its URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryHandling {
    /// A new entry after the current one (a same-URL push replaces instead).
    Push,
    /// A new entry in place of the current one (`location.replace`).
    Replace,
    /// Traversal to the current index, set by `history.go()`: the entry keeps
    /// its id and state.
    Traverse,
    /// Reload of the current entry, which keeps its id and state.
    Reload,
}

/// The document a commit replaces, as far as freeing it goes
/// (`Browser::commit_document`).
#[derive(Clone, Copy)]
enum Replaced {
    /// None, or the initial about:blank one as it was made.
    Nothing,
    /// The initial about:blank one, given content since.
    FilledBlank,
    /// A loaded one.
    Loaded,
}

#[derive(Debug, Clone)]
pub struct NavigateOpts {
    /// Body and content type of a form POST (`None`: GET).
    pub post: Option<(Vec<u8>, String)>,
    pub url: String,
    pub reason: NavigationReason,
    /// CDP request id of `Page.navigate`, so the response can be ordered after
    /// `frameStartedLoading` and before lifecycle events.
    pub cdp_request_id: Option<u64>,
    pub referrer: Option<String>,
}

impl NavigateOpts {
    pub fn address_bar(url: impl Into<String>) -> Self {
        NavigateOpts {
            post: None,
            url: url.into(),
            reason: NavigationReason::AddressBar,
            cdp_request_id: None,
            referrer: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SameDocumentKind {
    Push,
    Replace,
    Fragment,
}

/// Ordered page lifecycle events (upstream `Notification` equivalents).
#[derive(Debug, Clone)]
pub enum PageEvent {
    FrameNavigate {
        frame_id: FrameId,
        loader_id: LoaderId,
        url: String,
        reason: NavigationReason,
        cdp_request_id: Option<u64>,
    },
    FrameNavigated {
        frame_id: FrameId,
        loader_id: LoaderId,
        url: String,
        reason: NavigationReason,
        cdp_request_id: Option<u64>,
        timestamp: f64,
    },
    FrameNavigatedWithinDocument {
        frame_id: FrameId,
        url: String,
        kind: SameDocumentKind,
    },
    FrameNavigateFailed {
        frame_id: FrameId,
        loader_id: LoaderId,
        url: String,
        error: String,
        cdp_request_id: Option<u64>,
    },
    /// A realm became available for (frame, world); init scripts already ran.
    RealmCreated {
        frame_id: FrameId,
        realm: RealmId,
        world: Option<String>,
        origin: String,
    },
    RealmDestroyed {
        frame_id: FrameId,
        realm: RealmId,
    },
    FrameDomContentLoaded {
        frame_id: FrameId,
        loader_id: LoaderId,
        timestamp: f64,
    },
    FrameLoaded {
        frame_id: FrameId,
        loader_id: LoaderId,
        timestamp: f64,
    },
    FrameNetworkAlmostIdle {
        frame_id: FrameId,
        loader_id: LoaderId,
    },
    FrameNetworkIdle {
        frame_id: FrameId,
        loader_id: LoaderId,
    },
    FrameChildFrameCreated {
        parent: FrameId,
        frame_id: FrameId,
    },
    FrameChildFrameDetached {
        parent: FrameId,
        frame_id: FrameId,
    },
    FrameDestroyed {
        frame_id: FrameId,
    },
    JavascriptDialogOpening {
        frame_id: FrameId,
        kind: String,
        message: String,
        default_prompt: Option<String>,
    },
    /// The dialog [`PageEvent::JavascriptDialogOpening`] reported was
    /// dismissed (reported dialogs do not block; see
    /// [`PageObserver::javascript_dialog`]).
    JavascriptDialogClosed {
        frame_id: FrameId,
    },
    /// An `<input type=file>` was activated (no native picker opens).
    FileChooserOpened {
        frame_id: FrameId,
        input: lp_dom::NodeId,
        multiple: bool,
    },
    /// A link or `window.open` targets a new browsing context: the embedder
    /// opens `url` in a new page of the same browser context (no engine page
    /// exists yet). `opener` is whether the popup keeps its opener; `ticket`
    /// is set for `window.open`, and the embedder holds it as long as the
    /// popup page lives.
    PopupRequested {
        frame_id: FrameId,
        url: String,
        opener: bool,
        ticket: Option<std::rc::Rc<lp_webapi::PopupTicket>>,
    },
    /// The object `window.open` returned asked something of its popup. Comes
    /// after that popup's `PopupRequested`.
    PopupCommand {
        popup: std::rc::Rc<lp_webapi::PopupState>,
        command: lp_webapi::PopupCommand,
    },
    /// A popup page's own script called `window.close()`: the embedder
    /// closes the page. Only popups (pages adopted with [`Browser::adopt_popup`]
    /// and a ticket) are script-closable; other pages ignore the call.
    CloseRequested {
        frame_id: FrameId,
    },
    /// A native download was initiated, before its bytes finish fetching.
    DownloadStarted,
    /// A hyperlink download finished fetching (or failed).
    Download {
        frame_id: FrameId,
        url: String,
        suggested_filename: String,
        body: Result<bytes::Bytes, String>,
    },
}

/// Receives page events in emission order on the browser thread.
///
/// `FrameNavigatedWithinDocument` for `history.pushState`/`replaceState` and
/// fragment navigation is delivered synchronously from inside the JavaScript
/// call that caused it (as browsers do), so implementations must not run
/// JavaScript or re-enter the browser; queueing the event is the intent.
pub trait PageObserver {
    fn on_page_event(&self, page: PageId, event: &PageEvent);

    /// Transport progress of fetch/XHR request `request` became visible to
    /// the script of the document loaded by `loader`, before any script it
    /// causes runs (`lp_webapi::PageHooks::network_delivery`). `frontier` is
    /// cumulative for the request. Request ids are per browser context (each
    /// context has its own `lp_net::NetClient`), so `context` is part of the
    /// identity; `loader` tells an old document's requests from its
    /// successor's. Called synchronously on the browser thread: implementations
    /// may only record or publish, never run script or re-enter the browser.
    fn on_network_delivery(
        &self,
        _page: PageId,
        _context: ContextId,
        _loader: &LoaderId,
        _request: lp_net::RequestId,
        _frontier: lp_net::NetworkDelivery,
    ) {
    }

    /// A document of `page` started or ended a dedicated worker. May be called
    /// with script on the stack or while the page is being disposed: only record.
    fn on_worker_event(&self, _page: PageId, _event: lp_webapi::WorkerEvent) {}

    /// Page JavaScript in frame `frame` (document `url`) opened an `alert`,
    /// `confirm` or `prompt` dialog (`kind`). Called synchronously with the
    /// script on the stack, so like the other callbacks it must not run
    /// script or re-enter the browser. An embedder that shows the dialog
    /// (CDP with a session that has Page enabled) waits for its answer here
    /// and returns it: the script stays suspended meanwhile. `None` (the
    /// default) reports the dialog as [`PageEvent::JavascriptDialogOpening`]
    /// and the call returns what a dismissed dialog gives.
    fn javascript_dialog(
        &self,
        _page: PageId,
        _frame: &FrameId,
        _url: &str,
        _kind: &str,
        _message: &str,
        _default_prompt: Option<&str>,
    ) -> Option<lp_webapi::DialogAnswer> {
        None
    }
}

pub(crate) type ObserverCell = Rc<RefCell<Option<Rc<dyn PageObserver>>>>;

/// Delivers `event` without holding the cell borrow during the callback.
pub(crate) fn emit_to(cell: &ObserverCell, page: PageId, event: &PageEvent) {
    let observer = cell.borrow().clone();
    if let Some(o) = observer {
        o.on_page_event(page, event);
    }
}

/// Script evaluated in every new document before page scripts
/// (`Page.addScriptToEvaluateOnNewDocument`).
/// A global function reporting its calls to the engine hooks (CDP
/// `Runtime.addBinding`), defined in every new realm of a page before its
/// init scripts run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    pub name: String,
    /// `None`: every world; `Some(name)`: realms of that world ("" is the
    /// main world).
    pub world: Option<String>,
}

impl Binding {
    pub(crate) fn applies_to(&self, world: Option<&str>) -> bool {
        self.world.as_deref().is_none_or(|w| w == world.unwrap_or(""))
    }
}

#[derive(Debug, Clone)]
pub struct InitScript {
    pub id: String,
    pub source: String,
    /// `None` = main world; `Some(name)` = isolated world created on demand.
    pub world: Option<String>,
}

#[derive(Clone)]
pub struct FrameRealm {
    pub world: Option<String>,
    pub realm: Realm,
}

#[derive(Clone)]
pub struct Frame {
    pub id: FrameId,
    pub parent: Option<FrameId>,
    pub owner: Option<lp_dom::NodeId>,
    pub name: String,
    pub base_url: String,
    pub(crate) sandbox: frames::Sandbox,
    pub url: String,
    pub loader_id: LoaderId,
    pub realms: Vec<FrameRealm>,
    /// DOM store of the page (shared by its documents) and this frame's document.
    pub dom: Option<lp_dom::parse::DomRef>,
    pub document: Option<lp_dom::NodeId>,
    /// Binding state shared by this document's main and isolated worlds.
    pub page_dom: Option<Rc<lp_webapi::PageDom>>,
    /// Network quiet milestones the current document has reported.
    pub network_idle: NetworkIdle,
    pub origin: String,
    pub(crate) opaque_origin: u64,
    pub dom_content_loaded: bool,
    pub loaded: bool,
}

impl Frame {
    /// Field-wise equality by identity (realm ids, shared `Rc`s), without cloning.
    pub(crate) fn same_as(&self, other: &Frame) -> bool {
        self.id == other.id
            && self.parent == other.parent
            && self.owner == other.owner
            && self.name == other.name
            && self.base_url == other.base_url
            && self.sandbox == other.sandbox
            && self.url == other.url
            && self.loader_id == other.loader_id
            && self.realms.len() == other.realms.len()
            && self.realms.iter().zip(&other.realms).all(|(a, b)| a.world == b.world && a.realm.id() == b.realm.id())
            && match (&self.dom, &other.dom) {
                (Some(a), Some(b)) => Rc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            }
            && self.document == other.document
            && match (&self.page_dom, &other.page_dom) {
                (Some(a), Some(b)) => Rc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            }
            && self.network_idle == other.network_idle
            && self.origin == other.origin
            && self.opaque_origin == other.opaque_origin
            && self.dom_content_loaded == other.dom_content_loaded
            && self.loaded == other.loaded
    }
    pub fn main_realm(&self) -> Option<&Realm> {
        self.realms.iter().find(|r| r.world.is_none()).map(|r| &r.realm)
    }

    pub fn world(&self, name: &str) -> Option<&Realm> {
        self.realms.iter().find(|r| r.world.as_deref() == Some(name)).map(|r| &r.realm)
    }
}

/// Document state is shared by the main-frame and child-frame loading paths.
pub struct DocumentState {
    pub main_frame: Frame,
    pub nav: Rc<PageNav>,
    pub(crate) pending_loader: Option<LoaderId>,
    pub(crate) pending_cancel: Option<ResourceCancellation>,
    /// The loader and URL of `pending_loader` when the frame's scripts
    /// started it, which `document.open()` stops (HTML "stop loading").
    pub(crate) pending_script: Option<(LoaderId, String)>,
    pub(crate) load: Option<loader::DocumentLoad>,
    pub(crate) stylesheets: stylesheets::Stylesheets,
    pub(crate) images: images::Images,
    pub(crate) preloads: preload::Preloads,
    pub(crate) refresh: refresh::Refresh,
    pub(crate) _owner_pin: Option<loader::ResourceNode>,
}

impl DocumentState {
    /// The loader of a navigation whose document is still being fetched
    /// (not committed yet): the frame still shows the previous document.
    pub fn pending_loader(&self) -> Option<&LoaderId> {
        self.pending_loader.as_ref()
    }
}

pub struct Page {
    pub id: PageId,
    /// Set by every mutable access; `sync_root` compares only when set.
    pub(crate) root_dirty: Cell<bool>,
    pub context: ContextId,
    pub init_scripts: Vec<InitScript>,
    /// `Runtime.addBinding` functions, kept across navigation.
    pub bindings: Vec<Binding>,
    document: DocumentState,
    pub(crate) frames: Rc<FrameHost>,
    session_storage: lp_webapi::StorageNamespace,
    /// Set for a `window.open` popup: the opener's object reaches this
    /// page's documents through it, and dropping it tells the opener the
    /// popup closed.
    pub(crate) popup: Option<Rc<lp_webapi::PopupTicket>>,
    /// A popup's about:blank documents take the opener's origin and base
    /// URL (origin, opaque-origin token, base URL), as a child frame's do
    /// from its parent.
    opener_origin: Option<(String, u64, String)>,
    user_agent_override: Option<lp_webapi::UserAgentOverride>,
    /// `navigator.maxTouchPoints` of its documents (touch emulation).
    max_touch_points: u32,
    extra_http_headers: lp_net::http::HeaderMap,
    cache_disabled: bool,
    request_defaults: lp_net::RequestDefaults,
    viewport: lp_dom::media::Viewport,
    pending_resize: Option<LoaderId>,
}

impl std::ops::Deref for Page {
    type Target = DocumentState;
    fn deref(&self) -> &DocumentState {
        &self.document
    }
}
impl std::ops::DerefMut for Page {
    fn deref_mut(&mut self) -> &mut DocumentState {
        &mut self.document
    }
}

impl Page {
    /// Copies the page's own request options (extra headers, cache bypass)
    /// into `defaults`, e.g. those derived from a user agent override.
    fn apply_request_options(&self, defaults: &mut lp_net::RequestDefaults) {
        defaults.extra_headers = self.extra_http_headers.clone();
        defaults.cache_disabled = self.cache_disabled;
    }
}

/// Browser-thread state: the loop state type for [`EventLoop`].
pub struct Browser {
    pub engine: Engine,
    sched: Scheduler<Browser>,
    pub pages: Vec<Page>,
    /// Shared with every page's [`PageNav`], which reports same-document
    /// commits synchronously (see [`PageObserver`]).
    observer: ObserverCell,
    next_id: Rc<Cell<u64>>,
    started: Instant,
    /// Finalized-wrapper payloads released at checkpoints (diagnostics).
    pub released_payloads: RefCell<u64>,
    /// Cross-thread handle to this browser's own loop (network completions).
    handle: LoopHandle<Browser>,
    /// Loader for contexts without their own (tests, about:blank-only setups).
    pub(crate) loader: Option<Rc<dyn ResourceLoader>>,
    contexts: Vec<ContextState>,
    /// When set, every new context gets its own NetClient with this config.
    net_config: Option<lp_net::NetConfig>,
    /// `Security.setIgnoreCertificateErrors` sent browser-wide: applies to
    /// contexts created later too.
    ignore_certificate_errors: bool,
    /// Entries every context's local storage starts with.
    local_storage_seed: lp_webapi::StorageSeed,
    /// Completions of in-flight fetches, keyed by completion id.
    pub(crate) completions: std::collections::HashMap<u64, FetchDone>,
}

impl LoopState for Browser {
    fn after_task(&mut self) {
        self.checkpoint();
    }
}

impl Browser {
    /// A loaded single-frame page with nothing queued: every step of the
    /// checkpoint below would be a no-op. Checked first because the
    /// checkpoint runs after every CDP command.
    fn page_idle(p: &Page) -> bool {
        if p.root_dirty.get()
            || !p.main_frame.loaded
            || p.nav.has_pending_frame_url()
            || p.frames.has_children()
            || p.frames.has_retired()
        {
            return false;
        }
        {
            let Ok(dom) = p.frames.dom.try_borrow() else { return false };
            if dom.has_iframe_events() || dom.has_resource_events() {
                return false;
            }
        }
        p.main_frame.page_dom.as_ref().is_none_or(|d| d.quiet_since_last_task())
    }

    /// Paint Timing without a renderer: a document has its first paint at
    /// the first safe point at which it has a body and is not render-blocked
    /// (HTML: a render-blocked document has no rendering opportunity).
    fn note_first_paint(&mut self, key: frames::FrameKey) {
        let Some(state) = self.doc(key) else { return };
        if state.nav.first_paint().is_some() {
            return;
        }
        let (Some(bindings), nav) = (state.main_frame.page_dom.clone(), state.nav.clone()) else { return };
        drop(state);
        let has_body =
            bindings.dom.try_borrow().is_ok_and(|dom| dom.body(bindings.document()).ok().flatten().is_some());
        if has_body && !bindings.render_blocked() {
            nav.note_first_paint();
        }
    }

    /// Owner-thread safe point after synchronous host/inspector JavaScript.
    /// Call only after releasing DOM borrows. Event-loop tasks invoke it automatically.
    pub fn checkpoint(&mut self) {
        let n = self.engine.drain_finalized();
        *self.released_payloads.borrow_mut() += n as u64;
        let pages: Vec<_> = self.pages.iter().filter(|p| !Self::page_idle(p)).map(|p| p.id).collect();
        for id in pages {
            let Some(host) = self.page(id).map(|p| p.frames.clone()) else { continue };
            self.apply_same_document_url(id);
            self.sync_root(id);
            // Resource events are queued per page; one drain covers every frame.
            self.drain_dom_resources(id.into());
            for key in host.keys() {
                if let Some(bindings) = host.page_dom(key) {
                    bindings.after_task();
                }
                if key.child.is_some() {
                    if let Some(mut d) = self.doc_mut(key) {
                        if let Some(url) = d.nav.take_pending_frame_url() {
                            if !matches!(d.main_frame.url.as_str(), "about:blank" | "about:srcdoc") {
                                d.main_frame.base_url = url.clone();
                            }
                            d.main_frame.url = url;
                        }
                    }
                }
            }
            host.drain_retired();
            for key in host.keys() {
                self.fire_load_if_ready(key);
                self.note_first_paint(key);
            }
        }
    }
}

/// Maximum interval between services of the thread's native run loop while
/// the JavaScriptCore VM exists (macOS; see `lp_runtime` docs). Diagnostic-
/// backed initial policy: up to 4 idle wakes/s per browser thread.
#[cfg(target_os = "macos")]
const JSC_RUNLOOP_SERVICE_MAX_IDLE: std::time::Duration = std::time::Duration::from_millis(250);

/// A browser thread's loop plus its state.
pub struct BrowserThread {
    pub event_loop: EventLoop<Browser>,
    pub browser: Browser,
}

impl BrowserThread {
    /// Creates the engine for the current thread (one per thread).
    pub fn new() -> Result<BrowserThread, JsError> {
        let engine = Engine::new()?;
        lp_webapi::register(&engine);
        engine.register::<frames::CrossWindow>();
        engine.register::<frames::CrossLocation>();
        engine.register::<Window>();
        engine.register::<Location>();
        engine.register::<History>();
        engine.register::<Performance>();
        engine.register::<nav::PerformanceEntry>();
        engine.register::<nav::PerformancePaintTiming>();
        engine.register::<nav::PerformanceMark>();
        engine.register::<nav::PerformanceMeasure>();
        engine.register::<nav::PerformanceResourceTiming>();
        engine.register::<nav::PerformanceNavigationTiming>();
        engine.register::<nav::PerformanceTiming>();
        engine.register::<nav::PerformanceNavigation>();
        let event_loop = EventLoop::new();
        // The system JavaScriptCore binds this VM's timers to this thread's
        // run loop; the loop services it at a bounded interval for the VM's
        // whole lifetime (closed-page garbage lives in the VM, not in open
        // pages). The source build's timers use WTF's run loop instead, and
        // a fork child must not touch CoreFoundation's run loop at all.
        #[cfg(target_os = "macos")]
        let event_loop = {
            let mut event_loop = event_loop;
            if !lp_js::sys::SOURCE_BUILD {
                event_loop.enable_native_runloop_service(JSC_RUNLOOP_SERVICE_MAX_IDLE);
            }
            event_loop
        };
        let browser = Browser {
            engine,
            sched: event_loop.scheduler(),
            pages: Vec::new(),
            observer: Rc::new(RefCell::new(None)),
            next_id: Rc::new(Cell::new(1)),
            started: Instant::now(),
            released_payloads: RefCell::new(0),
            handle: event_loop.handle(),
            loader: None,
            contexts: vec![ContextState {
                id: DEFAULT_CONTEXT,
                loader: None,
                net: None,
                local_storage: Default::default(),
                services: Default::default(),
                init_scripts: Vec::new(),
            }],
            net_config: None,
            ignore_certificate_errors: false,
            local_storage_seed: Default::default(),
            completions: std::collections::HashMap::new(),
        };
        Ok(BrowserThread { event_loop, browser })
    }

    pub fn handle(&self) -> LoopHandle<Browser> {
        self.event_loop.handle()
    }

    /// Enables real document loading: every browser context (including the
    /// default one) gets its own lp-net client (I/O thread, cookie jar).
    pub fn enable_network(&mut self, config: lp_net::NetConfig) -> std::io::Result<()> {
        self.browser.net_config = Some(config);
        self.browser.attach_network(DEFAULT_CONTEXT)
    }

    /// Preloads local storage: every browser context (the existing ones and
    /// those created later) gets `seed`'s entries. Existing areas keep their
    /// other keys; no `storage` events fire.
    pub fn set_local_storage_seed(&mut self, seed: lp_webapi::StorageSeed) {
        for context in &self.browser.contexts {
            context.local_storage.seed(&seed);
        }
        self.browser.local_storage_seed = seed;
    }

    /// Runs tasks/timers until idle or the deadline.
    pub fn run_until_idle(&mut self, deadline: Option<Instant>) -> RunOutcome {
        self.browser.checkpoint();
        self.event_loop.run_until_idle(&mut self.browser, deadline)
    }

    pub fn run_until(&mut self, deadline: Option<Instant>, done: impl FnMut(&mut Browser) -> bool) -> RunOutcome {
        self.browser.checkpoint();
        self.event_loop.run_until(&mut self.browser, deadline, false, done)
    }
}

impl Browser {
    pub fn scheduler(&self) -> Scheduler<Browser> {
        self.sched.clone()
    }

    /// Virtual time ([`clock`]): moves the page clock and every pending
    /// timer `by` ahead.
    pub fn advance_time(&self, by: Duration) {
        self.sched.advance_timers(by);
        clock::skip(by);
    }

    /// Explicitly advances this browser thread's clock, without enabling automatic jumps.
    pub fn advance_time_explicit(&self, by: Duration) -> Result<(), JsError> {
        self.enable_page_clock()?;
        self.advance_time(by);
        Ok(())
    }

    /// Makes `Date` read the page clock in every realm (new realms do once
    /// it is on).
    fn enable_page_clock(&self) -> Result<(), JsError> {
        if !clock::page_clock_enabled() {
            for page in &self.pages {
                for info in self.frames(page.id) {
                    if let Some(frame) = self.frame(page.id, &info.id) {
                        for realm in &frame.realms {
                            realm.realm.scope()?.eval(
                                clock::DATE_SCRIPT,
                                Some("lightningpanda://internal/clock.js"),
                                1,
                            )?;
                        }
                    }
                }
            }
            clock::enable_manual();
        }
        Ok(())
    }

    /// Virtual time policy (`Emulation.setVirtualTimePolicy`), for this
    /// browser thread: freezes the page clock (`Date`, `performance.now()`)
    /// and holds every delayed timer; from then on time only moves through
    /// [`Browser::advance_virtual_time`]. `initial_wall_ms` sets `Date`'s
    /// time the first time. Returns false if it was already paused.
    pub fn pause_virtual_time(&self, initial_wall_ms: Option<f64>) -> Result<bool, JsError> {
        if clock::frozen() {
            return Ok(false);
        }
        self.enable_page_clock()?;
        clock::freeze(initial_wall_ms);
        // Held timers keep their distance to the virtual now, a year ahead
        // of the real time, so none comes due on its own.
        const HOLD: Duration = Duration::from_secs(365 * 24 * 3600);
        self.sched.delay_timers(HOLD);
        self.sched.set_virtual_now(Some(Instant::now() + HOLD));
        Ok(true)
    }

    /// Virtual time: how far ahead the next delayed timer is (zero when
    /// one is due); `None` without virtual time or timers.
    pub fn next_virtual_timer(&self) -> Option<Duration> {
        let now = self.sched.virtual_now()?;
        Some(self.sched.next_timer_deadline()?.saturating_duration_since(now))
    }

    /// Virtual time: moves the page clock `by` ahead. Returns the virtual
    /// now, by which timers are due ([`lp_runtime::EventLoop::run_timer_due_by`]).
    pub fn advance_virtual_time(&self, by: Duration) -> Option<Instant> {
        let now = self.sched.virtual_now()? + by;
        self.sched.set_virtual_now(Some(now));
        clock::skip(by);
        Some(now)
    }

    /// Cross-thread handle to this browser's loop.
    pub fn loop_handle(&self) -> LoopHandle<Browser> {
        self.handle.clone()
    }

    pub fn set_observer(&mut self, observer: Option<Rc<dyn PageObserver>>) {
        *self.observer.borrow_mut() = observer;
    }

    pub fn set_engine_hooks(&self, hooks: Option<Rc<dyn EngineHooks>>) {
        self.engine.set_hooks(hooks);
    }

    pub(crate) fn next_completion_id(&self) -> u64 {
        self.next()
    }

    fn next(&self) -> u64 {
        let n = self.next_id.get();
        self.next_id.set(n + 1);
        n
    }

    /// Seconds since the browser started (monotonic CDP event timestamps).
    pub fn timestamp(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }

    pub(crate) fn emit(&self, page: PageId, event: PageEvent) {
        emit_to(&self.observer, page, &event);
    }

    /// Creates a page showing `about:blank` (with an executable main realm,
    /// unlike the pinned upstream build TOS works around).
    /// Creates a page in the default context.
    pub fn create_page(&mut self) -> Result<PageId, JsError> {
        self.create_page_in(DEFAULT_CONTEXT)
    }

    /// Creates an isolated browser context with its own network state.
    pub fn create_context(&mut self) -> std::io::Result<ContextId> {
        self.create_context_inner(None)
    }

    /// `create_context` with its own proxy setting (`None`: no proxy) instead
    /// of the browser's.
    pub fn create_context_with_proxy(&mut self, proxy: Option<lp_net::ProxyConfig>) -> std::io::Result<ContextId> {
        self.create_context_inner(Some(proxy))
    }

    fn create_context_inner(&mut self, proxy: Option<Option<lp_net::ProxyConfig>>) -> std::io::Result<ContextId> {
        let id = ContextId(self.next() as u32);
        let local_storage = lp_webapi::StorageNamespace::seeded(&self.local_storage_seed);
        self.contexts.push(ContextState {
            id,
            loader: None,
            net: None,
            local_storage,
            services: Default::default(),
            init_scripts: Vec::new(),
        });
        if self.net_config.is_some() {
            self.attach_network_with(id, proxy)?;
        }
        Ok(id)
    }

    fn attach_network(&mut self, id: ContextId) -> std::io::Result<()> {
        self.attach_network_with(id, None)
    }

    fn attach_network_with(
        &mut self,
        id: ContextId,
        proxy: Option<Option<lp_net::ProxyConfig>>,
    ) -> std::io::Result<()> {
        let mut config = self.net_config.clone().unwrap_or_default();
        if let Some(proxy) = proxy {
            config.proxy = proxy;
        }
        let client = lp_net::NetClient::new(config)?;
        client.set_ignore_certificate_errors(self.ignore_certificate_errors);
        if let Some(c) = self.contexts.iter_mut().find(|c| c.id == id) {
            c.loader = Some(Rc::new(NetLoader::new(client.clone())));
            c.net = Some(client);
        }
        Ok(())
    }

    /// Closes a context: its pages are closed and its network client dropped.
    /// The default context cannot be closed.
    pub fn close_context(&mut self, id: ContextId) -> bool {
        if id == DEFAULT_CONTEXT {
            return false;
        }
        let pages: Vec<PageId> = self.pages.iter().filter(|p| p.context == id).map(|p| p.id).collect();
        for p in pages {
            self.close_page(p);
        }
        let before = self.contexts.len();
        self.contexts.retain(|c| c.id != id);
        before != self.contexts.len()
    }

    /// Accepts any HTTPS certificate in every context, including contexts
    /// created later (browser-wide `Security.setIgnoreCertificateErrors`).
    pub fn set_ignore_certificate_errors(&mut self, ignore: bool) {
        self.ignore_certificate_errors = ignore;
        for net in self.contexts.iter().filter_map(|c| c.net.as_ref()) {
            net.set_ignore_certificate_errors(ignore);
        }
    }

    /// The context's network client (cookies, interception, events).
    pub fn context_net(&self, id: ContextId) -> Option<&lp_net::NetClient> {
        self.contexts.iter().find(|c| c.id == id)?.net.as_ref()
    }

    /// Loader used by `page`: its context's, else the browser default.
    pub(crate) fn loader_for(&self, page: impl Into<PageId>) -> Option<Rc<dyn ResourceLoader>> {
        let ctx = self.page(page.into()).map(|p| p.context).unwrap_or(DEFAULT_CONTEXT);
        self.contexts.iter().find(|c| c.id == ctx).and_then(|c| c.loader.clone()).or_else(|| self.loader.clone())
    }

    /// Fetches a sub-resource of `page`'s current document through its
    /// context's loader (cookies, mocks, HTTP cache; CDP sees the request),
    /// for a renderer that needs bytes the DOM does not keep (screenshots:
    /// images, fonts, imported sheets). `done` runs on this thread.
    pub fn fetch_subresource(&mut self, page: PageId, url: String, kind: ResourceKind, done: FetchDone) {
        let Some(frame) = self.page(page).map(|p| p.main_frame.clone()) else {
            self.scheduler().post(lp_runtime::TaskSource::Networking, move |b| done(b, Err("page is gone".into())));
            return;
        };
        let Some(loader) = self.loader_for(page) else {
            self.scheduler()
                .post(lp_runtime::TaskSource::Networking, move |b| done(b, Err("no resource loader".into())));
            return;
        };
        let request = FetchRequest {
            post: None,
            url,
            kind,
            page,
            referrer: Some(frame.base_url.clone()),
            frame_id: Some(frame.id.0.clone()),
            loader_id: Some(frame.loader_id.0.clone()),
            crossorigin: None,
            initiator: None,
            part: None,
        };
        loader.fetch(self, request, done);
    }

    /// Creates a page in context `context`.
    pub fn create_page_in(&mut self, context: ContextId) -> Result<PageId, JsError> {
        if !self.contexts.iter().any(|c| c.id == context) {
            return Err(JsError::Type("no such browser context".into()));
        }
        let id = PageId(self.next() as u32);
        let frame = Frame {
            id: FrameId(format!("{:032X}", self.next())),
            parent: None,
            owner: None,
            name: String::new(),
            base_url: "about:blank".into(),
            sandbox: Default::default(),
            url: "about:blank".into(),
            loader_id: LoaderId(format!("{:032X}", self.next())),
            realms: Vec::new(),
            dom: None,
            document: None,
            page_dom: None,
            network_idle: NetworkIdle::default(),
            origin: "null".into(),
            opaque_origin: self.next(),
            dom_content_loaded: false,
            loaded: false,
        };
        let nav = PageNav::new(id, frame.id.clone(), self.sched.clone(), self.observer.clone());
        let frames = FrameHost::new(self, id, context, frame.clone(), nav.clone());
        let session_storage = frames.settings.borrow().session.clone();
        let init_scripts = self.contexts.iter().find(|c| c.id == context).unwrap().init_scripts.clone();
        frames.settings.borrow_mut().scripts = init_scripts.clone();
        debug_assert!(self.pages.last().is_none_or(|last| last.id < id), "page ids increase");
        self.pages.push(Page {
            id,
            context,
            root_dirty: Cell::new(true),
            init_scripts,
            bindings: Vec::new(),
            frames,
            document: DocumentState {
                main_frame: frame,
                pending_loader: None,
                pending_cancel: None,
                pending_script: None,
                nav,
                load: None,
                stylesheets: Default::default(),
                images: Default::default(),
                preloads: Default::default(),
                refresh: Default::default(),
                _owner_pin: None,
            },
            session_storage,
            popup: None,
            opener_origin: None,
            user_agent_override: None,
            max_touch_points: 0,
            extra_http_headers: Default::default(),
            cache_disabled: false,
            request_defaults: Default::default(),
            viewport: Default::default(),
            pending_resize: None,
        });
        self.navigate(
            id,
            NavigateOpts {
                post: None,
                url: "about:blank".into(),
                reason: NavigationReason::InitialFrameNavigation,
                cdp_request_id: None,
                referrer: None,
            },
        )?;
        Ok(id)
    }

    /// Makes `page` (just created in the opener's context) the popup that
    /// `opener`'s document requested with `ticket` (`PageEvent::PopupRequested`).
    /// With a ticket the popup keeps its opener: its documents see the
    /// opener's window as `window.opener`, its about:blank documents take
    /// the opener's origin and base URL, it uses the session storage copied
    /// for it (the current about:blank document switches to it), gets the
    /// requested browsing context name, and the opener's object reaches its
    /// documents. Without one (`noopener`) only the context is shared.
    pub fn adopt_popup(&mut self, page: PageId, opener: PageId, ticket: Option<Rc<lp_webapi::PopupTicket>>) {
        let Some(ticket) = ticket else { return };
        let opener_state = self.page(opener).map(|p| {
            let frame = &p.main_frame;
            (Rc::downgrade(&p.frames), (frame.origin.clone(), frame.opaque_origin, frame.base_url.clone()))
        });
        let local = self
            .page(page)
            .and_then(|p| self.contexts.iter().find(|c| c.id == p.context))
            .map(|c| c.local_storage.clone());
        let Some(p) = self.page_mut(page) else { return };
        let session = ticket.session_storage();
        p.session_storage = session.clone();
        p.frames.settings.borrow_mut().session = session.clone();
        p.main_frame.name = ticket.name().to_owned();
        if let Some((frames, origin)) = opener_state {
            *p.frames.opener.borrow_mut() = Some(frames);
            *p.frames.popup.borrow_mut() = Some(ticket.clone());
            // The initial about:blank document is the opener's creation.
            if p.main_frame.url == "about:blank" {
                p.main_frame.origin = origin.0.clone();
                p.main_frame.opaque_origin = origin.1;
                p.main_frame.base_url = origin.2.clone();
                for realm in &p.main_frame.realms {
                    realm.realm.set_origin(Some(origin.0.clone()));
                }
                if let Some(dom) = &p.main_frame.page_dom {
                    dom.set_base_url(&origin.2);
                    let doc = dom.document();
                    let _ = dom.dom.borrow_mut().set_document_base_url(doc, Some(&origin.2));
                }
            }
            p.opener_origin = Some(origin);
        }
        if let Some(dom) = &p.main_frame.page_dom {
            if let Some(local) = local {
                dom.set_storage_namespaces(local, session);
            }
            ticket.attach_page(dom);
        }
        p.popup = Some(ticket);
        self.sync_root(page);
    }

    /// The page `page` opened as a popup with its opener kept, if any.
    pub fn popup_opener(&self, page: PageId) -> Option<PageId> {
        self.page(page)?.frames.opener.borrow().as_ref().and_then(std::rc::Weak::upgrade).map(|h| h.page)
    }

    /// `pages` stays sorted by id: ids come from one increasing counter,
    /// pages are only appended at creation and removal keeps the order.
    fn page_index(&self, id: PageId) -> Option<usize> {
        debug_assert!(self.pages.windows(2).all(|w| w[0].id < w[1].id), "pages sorted by id");
        self.pages.binary_search_by_key(&id, |p| p.id).ok()
    }

    pub fn page(&self, id: PageId) -> Option<&Page> {
        self.page_index(id).map(|i| &self.pages[i])
    }

    pub(crate) fn page_mut(&mut self, id: PageId) -> Option<&mut Page> {
        let index = self.page_index(id)?;
        let page = &mut self.pages[index];
        page.root_dirty.set(true);
        Some(page)
    }

    /// Changes this page's navigator and native request defaults atomically.
    /// Persists across navigation and isolated worlds; sibling pages keep their defaults.
    /// An empty user agent uses the context default; `None` clears all overrides.
    pub fn set_user_agent_override(
        &mut self,
        page: PageId,
        value: Option<lp_webapi::UserAgentOverride>,
    ) -> Result<(), JsError> {
        let state = self.page_mut(page).ok_or_else(|| JsError::Type("no such page".into()))?;
        let mut defaults = value
            .as_ref()
            .map(lp_webapi::UserAgentOverride::request_defaults)
            .transpose()
            .map_err(|e| JsError::Type(format!("invalid user agent request defaults: {e}")))?
            .unwrap_or_default();
        // The user agent override owns none of the page's other request options.
        state.apply_request_options(&mut defaults);
        if let Some(bindings) = &state.main_frame.page_dom {
            bindings
                .set_user_agent_override(value.clone())
                .map_err(|e| JsError::Type(format!("invalid user agent request defaults: {e}")))?;
        }
        state.user_agent_override = value;
        state.request_defaults = defaults;
        self.refresh_frame_settings(page);
        Ok(())
    }

    /// The permissions and clipboard of `context`'s pages.
    pub fn context_services(&self, context: ContextId) -> Option<Rc<lp_webapi::ContextServices>> {
        self.contexts.iter().find(|c| c.id == context).map(|c| c.services.clone())
    }

    /// `navigator.maxTouchPoints` of this page's current and future
    /// documents (0: no touch input). Returns false if there is no such page.
    pub fn set_max_touch_points(&mut self, page: PageId, points: u32) -> bool {
        let Some(state) = self.page_mut(page) else { return false };
        state.max_touch_points = points;
        if let Some(bindings) = &state.main_frame.page_dom {
            bindings.set_max_touch_points(points);
        }
        self.refresh_frame_settings(page);
        true
    }

    /// Replaces this page's extra request headers (an empty map clears them).
    /// They apply to the page's document, subresource, fetch and XHR requests
    /// over context-wide settings, persist across navigation, and do not
    /// affect sibling pages. Returns false if there is no such page.
    pub fn set_extra_http_headers(&mut self, page: PageId, headers: lp_net::http::HeaderMap) -> bool {
        let Some(state) = self.page_mut(page) else { return false };
        if let Some(bindings) = &state.main_frame.page_dom {
            bindings.set_extra_http_headers(headers.clone());
        }
        state.request_defaults.extra_headers = headers.clone();
        state.extra_http_headers = headers;
        self.refresh_frame_settings(page);
        true
    }

    /// Bypasses the HTTP cache for this page's requests (document,
    /// subresources, fetch, XHR) while `disabled`: no lookup or store, plus
    /// no-cache request headers. Sibling pages keep using the context cache.
    /// Persists across navigation. Returns false if there is no such page.
    pub fn set_cache_disabled(&mut self, page: PageId, disabled: bool) -> bool {
        let Some(state) = self.page_mut(page) else { return false };
        if let Some(bindings) = &state.main_frame.page_dom {
            bindings.set_cache_disabled(disabled);
        }
        state.cache_disabled = disabled;
        state.request_defaults.cache_disabled = disabled;
        self.refresh_frame_settings(page);
        true
    }

    /// Installs a profile script in existing and future pages of one context.
    pub fn add_context_init_script(&mut self, context: ContextId, script: InitScript) -> bool {
        let Some(state) = self.contexts.iter_mut().find(|c| c.id == context) else { return false };
        state.init_scripts.push(script.clone());
        for page in self.pages.iter_mut().filter(|p| p.context == context) {
            page.init_scripts.push(script.clone());
            page.frames.settings.borrow_mut().scripts = page.init_scripts.clone();
        }
        true
    }

    pub fn add_init_script(&mut self, page: PageId, script: InitScript) -> bool {
        match self.page_mut(page) {
            Some(p) => {
                p.init_scripts.push(script);
                p.frames.settings.borrow_mut().scripts = p.init_scripts.clone();
                true
            }
            None => false,
        }
    }

    /// Adds `binding` to the page's future realms and defines it in its
    /// live ones it applies to. A binding of the same name and world is
    /// replaced. False if there is no such page.
    pub fn add_binding(&mut self, page: PageId, binding: Binding) -> bool {
        let Some(p) = self.page_mut(page) else { return false };
        p.bindings.retain(|b| *b != binding);
        p.bindings.push(binding.clone());
        p.frames.settings.borrow_mut().bindings = p.bindings.clone();
        for realm in self.page_realms(page) {
            if binding.applies_to(realm.world.as_deref()) {
                if let Ok(scope) = realm.realm.scope() {
                    if let Err(e) = scope.install_binding(&binding.name) {
                        scope.report_exception(&e);
                    }
                }
            }
        }
        true
    }

    /// Removes every binding named `name` from the page and deletes the
    /// global from its live realms. False if the page has no such binding.
    pub fn remove_binding(&mut self, page: PageId, name: &str) -> bool {
        let Some(p) = self.page_mut(page) else { return false };
        let before = p.bindings.len();
        p.bindings.retain(|b| b.name != name);
        if before == p.bindings.len() {
            return false;
        }
        p.frames.settings.borrow_mut().bindings = p.bindings.clone();
        for realm in self.page_realms(page) {
            if let Ok(scope) = realm.realm.scope() {
                let _ = scope.global().delete(name);
            }
        }
        true
    }

    /// Every live realm of every frame of `page`.
    fn page_realms(&self, page: PageId) -> Vec<FrameRealm> {
        self.frames(page).iter().filter_map(|info| self.frame(page, &info.id)).flat_map(|f| f.realms).collect()
    }

    pub fn remove_init_script(&mut self, page: PageId, id: &str) -> bool {
        match self.page_mut(page) {
            Some(p) => {
                let before = p.init_scripts.len();
                p.init_scripts.retain(|s| s.id != id);
                p.frames.settings.borrow_mut().scripts = p.init_scripts.clone();
                before != p.init_scripts.len()
            }
            None => false,
        }
    }

    /// Starts a navigation, replacing the current history entry when `replace`
    /// is true (`location.replace`).
    pub fn navigate_with_history(
        &mut self,
        page: PageId,
        opts: NavigateOpts,
        replace: bool,
    ) -> Result<LoaderId, JsError> {
        let history = if replace { HistoryHandling::Replace } else { HistoryHandling::Push };
        self.navigate_with_handling(page, opts, history)
    }

    /// Starts a navigation with explicit history handling. The handling belongs
    /// to this request, including while its document is being fetched.
    pub fn navigate_with_handling(
        &mut self,
        page: PageId,
        opts: NavigateOpts,
        history: HistoryHandling,
    ) -> Result<LoaderId, JsError> {
        self.navigate_frame_with_handling(page.into(), opts, history)
    }

    pub(crate) fn navigate_frame_with_handling(
        &mut self,
        page: FrameKey,
        opts: NavigateOpts,
        history: HistoryHandling,
    ) -> Result<LoaderId, JsError> {
        // HTML "navigate": the document being left is asked first
        // (`beforeunload` at its Window). The navigation goes on whatever its
        // listeners answer (`lp_webapi::lifecycle::before_unload`).
        if let Some(leaving) = self.doc(page).and_then(|doc| doc.main_frame.page_dom.clone()) {
            lp_webapi::lifecycle::before_unload(&leaving);
        }
        // "The navigation must be a replace" when it leaves the initial
        // about:blank document, decided as it starts: a document.open()
        // before it commits does not make it a push.
        let history = match history {
            HistoryHandling::Push if self.doc(page).is_some_and(|doc| doc.nav.is_initial_blank()) => {
                HistoryHandling::Replace
            }
            history => history,
        };
        let loader_id = LoaderId(format!("{:032X}", self.next()));
        // Invalidate the previous fetch even for blank or failed navigation.
        self.cancel_pending_document(page);
        let frame_id = self.doc(page).ok_or(JsError::Type("no such page".into()))?.main_frame.id.clone();
        self.emit(
            page.page,
            PageEvent::FrameNavigate {
                frame_id: frame_id.clone(),
                loader_id: loader_id.clone(),
                url: opts.url.clone(),
                reason: opts.reason,
                cdp_request_id: opts.cdp_request_id,
            },
        );
        if opts.url != "about:blank" && self.loader_for(page).is_some() {
            self.start_document_fetch(page, opts, loader_id.clone(), history)?;
            // The response arrives on a later task: build the realm the commit
            // will need while this thread would otherwise wait for it.
            let _ = self.prepare_spare_realm();
            return Ok(loader_id);
        }
        if opts.url != "about:blank" {
            let error = "net::ERR_NOT_IMPLEMENTED (no resource loader configured)".to_string();
            self.navigation_failed(page, &opts, loader_id, error);
            return Err(JsError::Unsupported("navigation to non-blank URLs"));
        }
        self.commit_document(page, &opts, loader_id.clone(), "null", history)?;
        self.finish_blank_document(page);
        Ok(loader_id)
    }

    /// Starts an ordinary navigation, adding a session-history entry on commit.
    pub fn navigate(&mut self, page: PageId, opts: NavigateOpts) -> Result<LoaderId, JsError> {
        self.navigate_with_history(page, opts, false)
    }

    /// HTML "unload a document and its descendants" for the active document
    /// of `frame`: its child frames' documents first, then its own
    /// (`lp_webapi::lifecycle::unload`). A frame tree without listeners
    /// costs a walk over its frames.
    fn unload_documents(&mut self, frame: FrameKey) {
        let Some(host) = self.page(frame.page).map(|p| p.frames.clone()) else { return };
        fn collect(b: &Browser, host: &frames::FrameHost, key: FrameKey, out: &mut Vec<Rc<lp_webapi::PageDom>>) {
            for child in host.child_keys(key) {
                collect(b, host, child, out);
            }
            if let Some(dom) = b.doc(key).and_then(|d| d.main_frame.page_dom.clone()) {
                out.push(dom);
            }
        }
        let mut documents = Vec::new();
        collect(self, &host, frame, &mut documents);
        for dom in documents {
            lp_webapi::lifecycle::unload(&dom);
        }
    }

    /// Replaces the frame's realms with fresh ones for the new document, runs
    /// init scripts, then emits `FrameNavigated`.
    pub(crate) fn commit_document(
        &mut self,
        page: FrameKey,
        opts: &NavigateOpts,
        loader_id: LoaderId,
        origin: &str,
        history: HistoryHandling,
    ) -> Result<(), JsError> {
        // The document being replaced (and its frames) unloads first.
        self.unload_documents(page);
        let host = self.page(page.page).ok_or(JsError::ContextDestroyed)?.frames.clone();
        self.cancel_pending_document(page);
        host.invalidate_focus(page, false);
        host.retire_children(page);
        host.drain_retired();
        if page.child.is_none() {
            self.page_mut(page.page).expect("page").main_frame.realms = host.root.borrow().realms.clone();
        }
        let inherited = self
            .doc(page)
            .and_then(|d| d.main_frame.parent.clone())
            .and_then(|id| host.key(&id))
            .and_then(|key| host.frame(key));
        // What an about:blank document inherits: its parent frame's origin
        // and base URL, or for a popup's top-level frame its opener's.
        let inherit_from: Option<(String, u64, String)> = match &inherited {
            Some(parent) => Some((parent.origin.clone(), parent.opaque_origin, parent.base_url.clone())),
            None if page.child.is_none() => self.page(page.page).and_then(|p| p.opener_origin.clone()),
            None => None,
        };
        // Attribute changes do not alter the active document's policy. Apply
        // the owner and ancestor restrictions when the next document commits.
        let sandbox = if let Some(owner) = self.doc(page).and_then(|d| d.main_frame.owner) {
            let mut flags = frames::Sandbox::parse(host.dom.borrow().get_attribute(owner, "sandbox").ok().flatten());
            if let Some(parent) = &inherited {
                flags.opaque |= parent.sandbox.opaque;
                flags.scripts_disabled |= parent.sandbox.scripts_disabled;
                flags.popups_blocked |= parent.sandbox.popups_blocked;
            }
            flags
        } else {
            frames::Sandbox::default()
        };
        self.doc_mut(page).ok_or(JsError::ContextDestroyed)?.main_frame.sandbox = sandbox;
        let inherit_url = matches!(opts.url.as_str(), "about:blank" | "about:srcdoc") && inherit_from.is_some();
        let actual_origin = if sandbox.opaque {
            "null".to_string()
        } else if inherit_url {
            inherit_from.as_ref().expect("creator").0.clone()
        } else {
            origin.to_string()
        };
        let token =
            if inherit_url && !sandbox.opaque { inherit_from.as_ref().expect("creator").1 } else { self.next() };
        let base_url = if inherit_url { inherit_from.as_ref().expect("creator").2.clone() } else { opts.url.clone() };
        {
            let mut state = self.doc_mut(page).ok_or(JsError::ContextDestroyed)?;
            state.main_frame.origin = actual_origin.clone();
            state.main_frame.opaque_origin = token;
            state.main_frame.base_url = base_url.clone();
        }
        self.sync_root(page.page);
        let page_dom = self.create_document_bindings(page, &opts.url, &loader_id)?;
        // Sub-resources of an external layout engine resolve against the
        // document's base (a srcdoc document inherits its parent's).
        page_dom.set_base_url(&base_url);
        if let Some(previous) = self.doc(page).map(|p| p.main_frame.loader_id.clone()) {
            self.forget_network_activity(page, &previous);
        }
        let scripts = self.page(page.page).ok_or(JsError::ContextDestroyed)?.init_scripts.clone();
        let (frame_id, old, scripts, nav, old_dom) = {
            let mut p = self.doc_mut(page).ok_or(JsError::Type("no such page".into()))?;
            p.main_frame.network_idle = NetworkIdle::default();
            p.main_frame.origin = actual_origin.clone();
            p.main_frame.dom_content_loaded = false;
            p.main_frame.loaded = false;
            let old = std::mem::take(&mut p.main_frame.realms);
            // The previous document (and its in-flight loads) is gone.
            p.pending_loader = None;
            p.pending_cancel = None;
            p.load = None;
            p.stylesheets = Default::default();
            p.images = Default::default();
            p.preloads = Default::default();
            p.refresh = Default::default();
            let old_dom = p.main_frame.page_dom.replace(page_dom.clone());
            // What the commit replaces: nothing (a new frame's first
            // document) or the initial about:blank one as it was made; that
            // one given content, written with document.open() (it is not
            // "initial" any more) or filled through DOM APIs; or a loaded
            // document.
            let replaced = match &old_dom {
                None => Replaced::Nothing,
                Some(old) if p.nav.is_first_blank() => {
                    if !p.nav.is_initial_blank() || bindings::blank_document_has_content(old) {
                        Replaced::FilledBlank
                    } else {
                        Replaced::Nothing
                    }
                }
                Some(_) => Replaced::Loaded,
            };
            p.main_frame.dom = Some(page_dom.dom.clone());
            p.main_frame.document = Some(page_dom.document());
            p.main_frame.url = opts.url.clone();
            p.main_frame.loader_id = loader_id.clone();
            p.nav.commit_document(&opts.url, history, None);
            p.nav.set_document(Some((page_dom.dom.clone(), page_dom.document())));
            p.nav.set_page_dom(&page_dom);
            (p.main_frame.id.clone(), old, scripts.clone(), p.nav.clone(), (old_dom, replaced))
        };
        let (old_dom, replaced) = old_dom;
        if let Some(dom) = &old_dom {
            dom.dispose();
        }
        for r in old {
            window::cancel_timers(&r.realm);
            let id = r.realm.id();
            if let Some(dom) = &old_dom {
                lp_webapi::lifecycle::realm_disposed(dom, id);
            }
            r.realm.dispose();
            self.emit(page.page, PageEvent::RealmDestroyed { frame_id: frame_id.clone(), realm: id });
        }
        if let Some(dom) = &old_dom {
            let _ = dom.dom.borrow_mut().set_document_active(dom.document(), false);
        }
        // The previous document is garbage now, as a closed page's is: collect
        // it before the next one boots instead of waiting for JSC's heuristics.
        // Measured on 80 navigations of a 2 MB React bundle: the process
        // plateaus at 218 MB instead of 629 MB, for about 5 ms per navigation.
        // A page's first document and its untouched initial about:blank one
        // leave nothing that pays for a full collection: a test's first `goto`
        // collected the whole heap for a blank page's two realms. Its
        // footprint still counts as a low point for later closes. An initial
        // document given content is collected only once the process has grown
        // past the gate, and then later (`Engine::defer_release_if_due`):
        // `page.setContent` of 40k nodes and a 1M-object array stayed after
        // the `goto` (81 MB, 26 MB released), while a `<style>` an init script
        // added had the idle loop drop the next page's compiled code. (What a
        // script kept on the blank document's window alone, with no node, is
        // left to the next release or JavaScriptCore's own collections.)
        drop(old_dom);
        match replaced {
            Replaced::Loaded => self.engine.release_closed(),
            Replaced::FilledBlank => self.engine.defer_release_if_due(),
            Replaced::Nothing => self.engine.note_release_point(),
        }
        // Worlds: main plus every world named by an init script.
        let mut worlds: Vec<Option<String>> = vec![None];
        for s in &scripts {
            if s.world.is_some() && !worlds.contains(&s.world) {
                worlds.push(s.world.clone());
            }
        }
        self.sync_root(page.page);
        for world in worlds {
            host.make_realm(page, nav.clone(), world)?;
        }
        if page.child.is_none() {
            self.page_mut(page.page).expect("page").main_frame.realms = host.root.borrow().realms.clone();
        }
        self.emit(
            page.page,
            PageEvent::FrameNavigated {
                frame_id,
                loader_id,
                url: opts.url.clone(),
                reason: opts.reason,
                cdp_request_id: opts.cdp_request_id,
                timestamp: self.timestamp(),
            },
        );
        Ok(())
    }

    /// Creates (or returns) an isolated world realm in the page's main frame
    /// (`Page.createIsolatedWorld`).
    pub fn isolated_world(&mut self, page: PageId, name: &str) -> Result<Realm, JsError> {
        let frame = self.page(page).ok_or(JsError::ContextDestroyed)?.main_frame.id.clone();
        self.isolated_world_in_frame(page, &frame, name)
    }

    /// Closes a page, disposing its realms and cancelling timers. Its
    /// documents unload first (`pagehide`, `visibilitychange`, `unload`), as
    /// a tab Chrome closes.
    pub fn close_page(&mut self, page: PageId) -> bool {
        if self.page_index(page).is_none() {
            return false;
        }
        self.unload_documents(page.into());
        self.dispose_page(page)
    }

    fn dispose_page(&mut self, page: PageId) -> bool {
        let Some(pos) = self.page_index(page) else { return false };
        self.cancel_pending_document(page.into());
        let host = self.pages[pos].frames.clone();
        host.invalidate_focus(page.into(), false);
        host.retire_children(page.into());
        host.drain_retired();
        let loader = self.pages[pos].main_frame.loader_id.clone();
        self.forget_network_activity(page.into(), &loader);
        self.pages[pos].main_frame.realms = host.root.borrow().realms.clone();
        self.pages[pos].root_dirty.set(true);
        let p = self.pages.remove(pos);
        if let Some(dom) = &p.main_frame.page_dom {
            dom.dispose();
        }
        for r in &p.document.main_frame.realms {
            window::cancel_timers(&r.realm);
            let id = r.realm.id();
            if let Some(dom) = &p.main_frame.page_dom {
                lp_webapi::lifecycle::realm_disposed(dom, id);
            }
            r.realm.dispose();
            self.emit(page, PageEvent::RealmDestroyed { frame_id: p.main_frame.id.clone(), realm: id });
        }
        self.emit(page, PageEvent::FrameDestroyed { frame_id: p.main_frame.id.clone() });
        drop(p);
        self.engine.release_closed();
        true
    }

    /// Disposes every page (connection teardown): no script runs.
    pub fn close_all(&mut self) {
        let ids: Vec<PageId> = self.pages.iter().map(|p| p.id).collect();
        for id in ids {
            self.dispose_page(id);
        }
        frames::drop_spare_realm();
        self.engine.drain_finalized();
    }

    /// Builds a main-world realm now for this thread's next page, so page
    /// creation skips interface installation. Call while idle (before the
    /// connection that will use this thread exists).
    pub fn prepare_spare_realm(&self) -> Result<(), JsError> {
        frames::prepare_spare_realm(self.engine.register::<Window>())
    }
}

#[cfg(test)]
mod tests;

impl Drop for Browser {
    fn drop(&mut self) {
        frames::drop_spare_realm();
        self.close_all();
    }
}
