//! Browser/page orchestration on the browser thread.
//!
//! Ownership (see docs/ARCHITECTURE.md): this crate owns the Browser →
//! Page → Frame tree, navigation state machine, realms per (frame, world),
//! init-script injection, window timers, and the ordered [`PageEvent`] stream
//! consumed by the automation layer. Document loading uses `lp-dom`/`lp-net`;
//! `lp-webapi` supplies bindings in the main and isolated document worlds.

mod bindings;
mod focus;
mod frames;
mod images;
mod loader;
mod nav;
mod network_idle;
mod stylesheets;
mod viewport;
mod window;

use lp_js::{Engine, EngineHooks, JsError, Realm, RealmId, RealmOptions};
use lp_runtime::{EventLoop, LoopHandle, LoopState, RunOutcome, Scheduler};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Instant;

pub use frames::FrameInfo;
use frames::{FrameHost, FrameKey};
pub use loader::{
    FetchDone, FetchRequest, NetLoader, Resource, ResourceCancellation, ResourceKind, ResourceLoader, StaticLoader,
};
pub use nav::{History, HistoryEntry, Location, PageNav, Performance};
pub use network_idle::NetworkIdle;
pub use window::{Window, WindowData};

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

#[derive(Debug, Clone)]
pub struct NavigateOpts {
    pub url: String,
    pub reason: NavigationReason,
    /// CDP request id of `Page.navigate`, so the response can be ordered after
    /// `frameStartedLoading` and before lifecycle events.
    pub cdp_request_id: Option<u64>,
    pub referrer: Option<String>,
}

impl NavigateOpts {
    pub fn address_bar(url: impl Into<String>) -> Self {
        NavigateOpts { url: url.into(), reason: NavigationReason::AddressBar, cdp_request_id: None, referrer: None }
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
    /// An `<input type=file>` was activated (no native picker opens).
    FileChooserOpened {
        frame_id: FrameId,
        input: lp_dom::NodeId,
        multiple: bool,
    },
    /// A link or `window.open` targets a new browsing context: the embedder
    /// opens `url` in a new page of the same browser context (no engine page
    /// exists yet). `opener` is whether the popup keeps its opener.
    PopupRequested {
        frame_id: FrameId,
        url: String,
        opener: bool,
    },
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
    pub(crate) load: Option<loader::DocumentLoad>,
    pub(crate) stylesheets: stylesheets::Stylesheets,
    pub(crate) images: images::Images,
    pub(crate) _owner_pin: Option<loader::ResourceNode>,
}

pub struct Page {
    pub id: PageId,
    pub context: ContextId,
    pub init_scripts: Vec<InitScript>,
    document: DocumentState,
    pub(crate) frames: Rc<FrameHost>,
    session_storage: lp_webapi::StorageNamespace,
    user_agent_override: Option<lp_webapi::UserAgentOverride>,
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
    /// Completions of in-flight fetches, keyed by completion id.
    pub(crate) completions: std::collections::HashMap<u64, FetchDone>,
}

impl LoopState for Browser {
    fn after_task(&mut self) {
        self.checkpoint();
    }
}

impl Browser {
    /// Owner-thread safe point after synchronous host/inspector JavaScript.
    /// Call only after releasing DOM borrows. Event-loop tasks invoke it automatically.
    pub fn checkpoint(&mut self) {
        let n = self.engine.drain_finalized();
        *self.released_payloads.borrow_mut() += n as u64;
        let pages: Vec<_> = self.pages.iter().map(|p| p.id).collect();
        for id in pages {
            let Some(host) = self.page(id).map(|p| p.frames.clone()) else { continue };
            self.apply_same_document_url(id);
            self.sync_root(id);
            for key in host.keys() {
                self.drain_dom_resources(key);
                if let Some(bindings) = self.doc(key).and_then(|p| p.main_frame.page_dom.clone()) {
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
                self.maybe_fire_load(key);
            }
        }
    }
}

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
        engine.register::<Window>();
        engine.register::<Location>();
        engine.register::<History>();
        engine.register::<Performance>();
        let event_loop = EventLoop::new();
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
            }],
            net_config: None,
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
        let id = ContextId(self.next() as u32);
        self.contexts.push(ContextState { id, loader: None, net: None, local_storage: Default::default() });
        if self.net_config.is_some() {
            self.attach_network(id)?;
        }
        Ok(id)
    }

    fn attach_network(&mut self, id: ContextId) -> std::io::Result<()> {
        let config = self.net_config.clone().unwrap_or_default();
        let client = lp_net::NetClient::new(config)?;
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

    /// The context's network client (cookies, interception, events).
    pub fn context_net(&self, id: ContextId) -> Option<&lp_net::NetClient> {
        self.contexts.iter().find(|c| c.id == id)?.net.as_ref()
    }

    /// Loader used by `page`: its context's, else the browser default.
    pub(crate) fn loader_for(&self, page: impl Into<PageId>) -> Option<Rc<dyn ResourceLoader>> {
        let ctx = self.page(page.into()).map(|p| p.context).unwrap_or(DEFAULT_CONTEXT);
        self.contexts.iter().find(|c| c.id == ctx).and_then(|c| c.loader.clone()).or_else(|| self.loader.clone())
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
        self.pages.push(Page {
            id,
            context,
            init_scripts: Vec::new(),
            frames,
            document: DocumentState {
                main_frame: frame,
                pending_loader: None,
                pending_cancel: None,
                nav,
                load: None,
                stylesheets: Default::default(),
                images: Default::default(),
                _owner_pin: None,
            },
            session_storage,
            user_agent_override: None,
            extra_http_headers: Default::default(),
            cache_disabled: false,
            request_defaults: Default::default(),
            viewport: Default::default(),
            pending_resize: None,
        });
        self.navigate(
            id,
            NavigateOpts {
                url: "about:blank".into(),
                reason: NavigationReason::InitialFrameNavigation,
                cdp_request_id: None,
                referrer: None,
            },
        )?;
        Ok(id)
    }

    pub fn page(&self, id: PageId) -> Option<&Page> {
        self.pages.iter().find(|p| p.id == id)
    }

    pub(crate) fn page_mut(&mut self, id: PageId) -> Option<&mut Page> {
        self.pages.iter_mut().find(|p| p.id == id)
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
        // Attribute changes do not alter the active document's policy. Apply
        // the owner and ancestor restrictions when the next document commits.
        let sandbox = if let Some(owner) = self.doc(page).and_then(|d| d.main_frame.owner) {
            let mut flags = frames::Sandbox::parse(host.dom.borrow().get_attribute(owner, "sandbox").ok().flatten());
            if let Some(parent) = &inherited {
                flags.opaque |= parent.sandbox.opaque;
                flags.scripts_disabled |= parent.sandbox.scripts_disabled;
            }
            flags
        } else {
            frames::Sandbox::default()
        };
        self.doc_mut(page).ok_or(JsError::ContextDestroyed)?.main_frame.sandbox = sandbox;
        let inherit_url = matches!(opts.url.as_str(), "about:blank" | "about:srcdoc") && inherited.is_some();
        let actual_origin = if sandbox.opaque {
            "null".to_string()
        } else if inherit_url {
            inherited.as_ref().expect("parent").origin.clone()
        } else {
            origin.to_string()
        };
        let token = if inherit_url && !sandbox.opaque {
            inherited.as_ref().expect("parent").opaque_origin
        } else {
            self.next()
        };
        {
            let mut state = self.doc_mut(page).ok_or(JsError::ContextDestroyed)?;
            state.main_frame.origin = actual_origin.clone();
            state.main_frame.opaque_origin = token;
            state.main_frame.base_url =
                if inherit_url { inherited.as_ref().expect("parent").base_url.clone() } else { opts.url.clone() };
        }
        self.sync_root(page.page);
        let page_dom = self.create_document_bindings(page, &opts.url, &loader_id)?;
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
            let old_dom = p.main_frame.page_dom.replace(page_dom.clone());
            p.main_frame.dom = Some(page_dom.dom.clone());
            p.main_frame.document = Some(page_dom.document());
            p.main_frame.url = opts.url.clone();
            p.main_frame.loader_id = loader_id.clone();
            p.nav.commit_document(&opts.url, history, None);
            p.nav.set_document(Some((page_dom.dom.clone(), page_dom.document())));
            (p.main_frame.id.clone(), old, scripts.clone(), p.nav.clone(), old_dom)
        };
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

    /// Closes a page, disposing its realms and cancelling timers.
    pub fn close_page(&mut self, page: PageId) -> bool {
        let Some(pos) = self.pages.iter().position(|p| p.id == page) else { return false };
        self.cancel_pending_document(page.into());
        let host = self.pages[pos].frames.clone();
        host.invalidate_focus(page.into(), false);
        host.retire_children(page.into());
        host.drain_retired();
        let loader = self.pages[pos].main_frame.loader_id.clone();
        self.forget_network_activity(page.into(), &loader);
        self.pages[pos].main_frame.realms = host.root.borrow().realms.clone();
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
        true
    }

    /// Disposes every page (connection teardown).
    pub fn close_all(&mut self) {
        let ids: Vec<PageId> = self.pages.iter().map(|p| p.id).collect();
        for id in ids {
            self.close_page(id);
        }
        self.engine.drain_finalized();
    }
}

#[cfg(test)]
mod tests;

impl Drop for Browser {
    fn drop(&mut self) {
        self.close_all();
    }
}
