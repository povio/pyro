//! Browser/page orchestration on the browser thread.
//!
//! Ownership (see docs/ARCHITECTURE.md): this crate owns the Browser →
//! Page → Frame tree, navigation state machine, realms per (frame, world),
//! init-script injection, window timers, and the ordered [`PageEvent`] stream
//! consumed by the automation layer. Document loading uses `lp-dom`/`lp-net`;
//! `lp-webapi` supplies bindings in the main and isolated document worlds.

mod bindings;
mod loader;
mod nav;
mod stylesheets;
mod window;

use lp_js::{Engine, EngineHooks, JsError, Realm, RealmId, RealmOptions};
use lp_runtime::{EventLoop, LoopHandle, LoopState, RunOutcome, Scheduler};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Instant;

pub use loader::{
    FetchDone, FetchRequest, NetLoader, Resource, ResourceCancellation, ResourceKind, ResourceLoader, StaticLoader,
};
pub use nav::{History, Location, PageNav, Performance};
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
}

/// Receives page events in emission order on the browser thread.
pub trait PageObserver {
    fn on_page_event(&self, page: PageId, event: &PageEvent);
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

pub struct FrameRealm {
    pub world: Option<String>,
    pub realm: Realm,
}

pub struct Frame {
    pub id: FrameId,
    pub url: String,
    pub loader_id: LoaderId,
    pub realms: Vec<FrameRealm>,
    /// DOM store of the page (shared by its documents) and this frame's document.
    pub dom: Option<lp_dom::parse::DomRef>,
    pub document: Option<lp_dom::NodeId>,
    /// Binding state shared by this document's main and isolated worlds.
    pub page_dom: Option<Rc<lp_webapi::PageDom>>,
}

impl Frame {
    pub fn main_realm(&self) -> Option<&Realm> {
        self.realms.iter().find(|r| r.world.is_none()).map(|r| &r.realm)
    }

    pub fn world(&self, name: &str) -> Option<&Realm> {
        self.realms.iter().find(|r| r.world.as_deref() == Some(name)).map(|r| &r.realm)
    }
}

pub struct Page {
    pub id: PageId,
    pub context: ContextId,
    pub main_frame: Frame,
    pub init_scripts: Vec<InitScript>,
    /// Navigation whose document is being fetched (superseded fetches are ignored).
    pub(crate) pending_loader: Option<LoaderId>,
    /// Whether the pending navigation replaces the current history entry.
    pub(crate) pending_replace: bool,
    /// Session history and current URL (shared with the page's windows).
    pub nav: Rc<PageNav>,
    pub(crate) load: Option<loader::DocumentLoad>,
    pub(crate) stylesheets: stylesheets::Stylesheets,
    session_storage: lp_webapi::StorageNamespace,
}

/// Browser-thread state: the loop state type for [`EventLoop`].
pub struct Browser {
    pub engine: Engine,
    sched: Scheduler<Browser>,
    pub pages: Vec<Page>,
    observer: Option<Rc<dyn PageObserver>>,
    next_id: Cell<u64>,
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
            self.drain_dom_resources(id);
            if let Some(page) = self.page(id).and_then(|p| p.main_frame.page_dom.as_ref()) {
                page.after_task();
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
        engine.register::<Window>();
        engine.register::<Location>();
        engine.register::<History>();
        engine.register::<Performance>();
        let event_loop = EventLoop::new();
        let browser = Browser {
            engine,
            sched: event_loop.scheduler(),
            pages: Vec::new(),
            observer: None,
            next_id: Cell::new(1),
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
        self.observer = observer;
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
        if let Some(o) = &self.observer {
            o.on_page_event(page, &event);
        }
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
    pub(crate) fn loader_for(&self, page: PageId) -> Option<Rc<dyn ResourceLoader>> {
        let ctx = self.page(page).map(|p| p.context).unwrap_or(DEFAULT_CONTEXT);
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
            url: "about:blank".into(),
            loader_id: LoaderId(format!("{:032X}", self.next())),
            realms: Vec::new(),
            dom: None,
            document: None,
            page_dom: None,
        };
        let nav = PageNav::new(id, self.sched.clone());
        self.pages.push(Page {
            id,
            context,
            main_frame: frame,
            init_scripts: Vec::new(),
            pending_loader: None,
            pending_replace: false,
            nav,
            load: None,
            stylesheets: Default::default(),
            session_storage: Default::default(),
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

    pub fn add_init_script(&mut self, page: PageId, script: InitScript) -> bool {
        match self.page_mut(page) {
            Some(p) => {
                p.init_scripts.push(script);
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
                before != p.init_scripts.len()
            }
            None => false,
        }
    }

    /// Starts a navigation. Only `about:blank` is supported until document
    /// loading (lp-dom/lp-net) is integrated; other URLs emit
    /// `FrameNavigateFailed` and return an error.
    /// Starts a navigation that replaces the current history entry when
    /// `replace` (location.replace/reload, history traversal).
    pub fn navigate_with_history(
        &mut self,
        page: PageId,
        opts: NavigateOpts,
        replace: bool,
    ) -> Result<LoaderId, JsError> {
        if let Some(p) = self.page_mut(page) {
            p.pending_replace = replace;
        }
        self.navigate(page, opts)
    }

    pub fn navigate(&mut self, page: PageId, opts: NavigateOpts) -> Result<LoaderId, JsError> {
        let loader_id = LoaderId(format!("{:032X}", self.next()));
        let frame_id = self.page(page).ok_or(JsError::Type("no such page".into()))?.main_frame.id.clone();
        self.emit(
            page,
            PageEvent::FrameNavigate {
                frame_id: frame_id.clone(),
                loader_id: loader_id.clone(),
                url: opts.url.clone(),
                reason: opts.reason,
                cdp_request_id: opts.cdp_request_id,
            },
        );
        if opts.url != "about:blank" && self.loader_for(page).is_some() {
            self.start_document_fetch(page, opts, loader_id.clone())?;
            return Ok(loader_id);
        }
        if opts.url != "about:blank" {
            let error = "net::ERR_NOT_IMPLEMENTED (no resource loader configured)".to_string();
            self.emit(
                page,
                PageEvent::FrameNavigateFailed {
                    frame_id,
                    loader_id,
                    url: opts.url,
                    error: error.clone(),
                    cdp_request_id: opts.cdp_request_id,
                },
            );
            return Err(JsError::Unsupported("navigation to non-blank URLs"));
        }
        self.commit_document(page, &opts, loader_id.clone(), "null")?;
        if let Some(dom) = self.page(page).and_then(|p| p.main_frame.page_dom.clone()) {
            lp_webapi::lifecycle::dom_content_loaded(&dom, dom.document());
            lp_webapi::lifecycle::load(&dom, dom.document());
        }
        let ts = self.timestamp();
        self.emit(
            page,
            PageEvent::FrameDomContentLoaded {
                frame_id: frame_id.clone(),
                loader_id: loader_id.clone(),
                timestamp: ts,
            },
        );
        self.emit(
            page,
            PageEvent::FrameLoaded { frame_id: frame_id.clone(), loader_id: loader_id.clone(), timestamp: ts },
        );
        self.emit(page, PageEvent::FrameNetworkAlmostIdle { frame_id: frame_id.clone(), loader_id: loader_id.clone() });
        self.emit(page, PageEvent::FrameNetworkIdle { frame_id, loader_id: loader_id.clone() });
        Ok(loader_id)
    }

    /// Replaces the frame's realms with fresh ones for the new document, runs
    /// init scripts, then emits `FrameNavigated`.
    pub(crate) fn commit_document(
        &mut self,
        page: PageId,
        opts: &NavigateOpts,
        loader_id: LoaderId,
        origin: &str,
    ) -> Result<(), JsError> {
        let window_class = self.engine.register::<Window>();
        let sched = self.sched.clone();
        let page_dom = self.create_document_bindings(page, &opts.url, &loader_id)?;
        let (frame_id, old, scripts, nav, old_dom) = {
            let p = self.page_mut(page).ok_or(JsError::Type("no such page".into()))?;
            let old = std::mem::take(&mut p.main_frame.realms);
            // The previous document (and its in-flight loads) is gone.
            p.load = None;
            p.stylesheets = Default::default();
            let old_dom = p.main_frame.page_dom.replace(page_dom.clone());
            p.main_frame.dom = Some(page_dom.dom.clone());
            p.main_frame.document = Some(page_dom.document());
            p.main_frame.url = opts.url.clone();
            p.main_frame.loader_id = loader_id.clone();
            let replace = std::mem::take(&mut p.pending_replace);
            p.nav.commit_document(&opts.url, replace, None);
            p.nav.set_document(Some((page_dom.dom.clone(), page_dom.document())));
            (p.main_frame.id.clone(), old, p.init_scripts.clone(), p.nav.clone(), old_dom)
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
            self.emit(page, PageEvent::RealmDestroyed { frame_id: frame_id.clone(), realm: id });
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
        let mut created = Vec::new();
        for world in worlds {
            let realm = self.engine.create_realm(RealmOptions {
                name: world.clone().unwrap_or_default(),
                global_class: Some(window_class),
                origin: (origin != "null").then(|| origin.to_string()),
            })?;
            realm.set_global_data::<Window>(WindowData::new(sched.clone(), nav.clone(), &page_dom))?;
            lp_webapi::install(&realm.scope()?, page_dom.clone())?;
            self.install_module_fetcher(page, &realm);
            for s in scripts.iter().filter(|s| s.world == world) {
                let scope = realm.scope()?;
                if let Err(e) = scope.eval(&s.source, Some(&format!("init-script:{}", s.id)), 1) {
                    scope.report_exception(&e);
                }
            }
            created.push(FrameRealm { world, realm });
        }
        let events: Vec<PageEvent> = created
            .iter()
            .map(|r| PageEvent::RealmCreated {
                frame_id: frame_id.clone(),
                realm: r.realm.id(),
                world: r.world.clone(),
                origin: origin.to_string(),
            })
            .collect();
        self.page_mut(page).expect("page").main_frame.realms = created;
        for e in events {
            self.emit(page, e);
        }
        self.emit(
            page,
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
        if let Some(r) = self.page(page).and_then(|p| p.main_frame.world(name)) {
            return Ok(r.clone());
        }
        let window_class = self.engine.register::<Window>();
        let (url, frame_id, page_dom) = {
            let p = self.page(page).ok_or(JsError::Type("no such page".into()))?;
            (
                p.main_frame.url.clone(),
                p.main_frame.id.clone(),
                p.main_frame.page_dom.clone().ok_or(JsError::ContextDestroyed)?,
            )
        };
        let origin =
            url::Url::parse(&url).ok().map(|u| u.origin().ascii_serialization()).unwrap_or_else(|| "null".into());
        let realm = self.engine.create_realm(RealmOptions {
            name: name.to_string(),
            global_class: Some(window_class),
            origin: (origin != "null").then(|| origin.clone()),
        })?;
        let nav = self.page(page).expect("page").nav.clone();
        realm.set_global_data::<Window>(WindowData::new(self.sched.clone(), nav, &page_dom))?;
        lp_webapi::install(&realm.scope()?, page_dom)?;
        self.install_module_fetcher(page, &realm);
        self.page_mut(page)
            .expect("page")
            .main_frame
            .realms
            .push(FrameRealm { world: Some(name.to_string()), realm: realm.clone() });
        self.emit(page, PageEvent::RealmCreated { frame_id, realm: realm.id(), world: Some(name.to_string()), origin });
        Ok(realm)
    }

    /// Closes a page, disposing its realms and cancelling timers.
    pub fn close_page(&mut self, page: PageId) -> bool {
        let Some(pos) = self.pages.iter().position(|p| p.id == page) else { return false };
        let p = self.pages.remove(pos);
        if let Some(dom) = &p.main_frame.page_dom {
            dom.dispose();
        }
        for r in p.main_frame.realms {
            window::cancel_timers(&r.realm);
            let id = r.realm.id();
            if let Some(dom) = &p.main_frame.page_dom {
                lp_webapi::lifecycle::realm_disposed(dom, id);
            }
            r.realm.dispose();
            self.emit(page, PageEvent::RealmDestroyed { frame_id: p.main_frame.id.clone(), realm: id });
        }
        self.emit(page, PageEvent::FrameDestroyed { frame_id: p.main_frame.id });
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
