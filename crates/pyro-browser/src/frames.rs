//! Per-page frame registry. No registry borrow may cross JavaScript execution.
use crate::*;
use pyro_dom::NodeId;
use std::cell::{Ref, RefMut};
use std::collections::BTreeMap;
use std::ops::{Deref, DerefMut};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FrameKey {
    pub page: PageId,
    pub child: Option<u64>,
}
impl From<PageId> for FrameKey {
    fn from(page: PageId) -> Self {
        Self { page, child: None }
    }
}
impl From<FrameKey> for PageId {
    fn from(frame: FrameKey) -> Self {
        frame.page
    }
}

pub(crate) enum StateRef<'a, T> {
    Direct(&'a T),
    Shared(Ref<'a, T>),
}
impl<T> Deref for StateRef<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        match self {
            Self::Direct(v) => v,
            Self::Shared(v) => v,
        }
    }
}
impl<'a, T> StateRef<'a, T> {
    pub(crate) fn filter_map<U>(self, f: impl FnOnce(&T) -> Option<&U>) -> Option<StateRef<'a, U>> {
        match self {
            Self::Direct(v) => f(v).map(StateRef::Direct),
            Self::Shared(v) => Ref::filter_map(v, f).ok().map(StateRef::Shared),
        }
    }
}
pub(crate) enum StateMut<'a, T> {
    Direct(&'a mut T),
    Shared(RefMut<'a, T>),
}
impl<T> Deref for StateMut<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        match self {
            Self::Direct(v) => v,
            Self::Shared(v) => v,
        }
    }
}
impl<T> DerefMut for StateMut<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        match self {
            Self::Direct(v) => v,
            Self::Shared(v) => v,
        }
    }
}
impl<'a, T> StateMut<'a, T> {
    pub(crate) fn filter_map<U>(self, f: impl FnOnce(&mut T) -> Option<&mut U>) -> Option<StateMut<'a, U>> {
        match self {
            Self::Direct(v) => f(v).map(StateMut::Direct),
            Self::Shared(v) => RefMut::filter_map(v, f).ok().map(StateMut::Shared),
        }
    }
}
pub(crate) type DocumentRef<'a> = StateRef<'a, DocumentState>;
pub(crate) type DocumentMut<'a> = StateMut<'a, DocumentState>;

/// Owned frame metadata: safe to retain while commands execute JavaScript.
#[derive(Clone)]
pub struct FrameInfo {
    pub id: FrameId,
    pub parent: Option<FrameId>,
    pub owner: Option<NodeId>,
    pub url: String,
    pub loader_id: LoaderId,
    pub name: String,
    pub origin: String,
    pub network_idle: NetworkIdle,
    pub dom_content_loaded: bool,
    pub loaded: bool,
    /// The frame shows the error document of a navigation to this URL.
    pub unreachable_url: Option<String>,
}

/// A new top-level context starts with fresh target emulation settings; only
/// the opener's window and screen geometry supplies its initial dimensions.
pub(crate) fn popup_viewport(opener: &pyro_dom::media::Viewport) -> pyro_dom::media::Viewport {
    pyro_dom::media::Viewport {
        width: opener.width,
        height: opener.height,
        screen_width: opener.screen_width,
        screen_height: opener.screen_height,
        ..Default::default()
    }
}

/// Fit the requested desktop window to its screen's available area. Keep the
/// emulated screen independent of the smaller layout viewport of a popup.
pub(crate) fn apply_window_features(
    viewport: &mut pyro_dom::media::Viewport,
    features: pyro_webapi::window_features::WindowFeatures,
) -> (bool, i32, i32) {
    let screen_width = viewport.screen_width.unwrap_or(viewport.width);
    let screen_height = viewport.screen_height.unwrap_or(viewport.height);
    // A headless surface has no native window decoration. Its smallest
    // visible viewport is one CSS pixel, including under device emulation.
    let dimension = |requested: i32, available: u32| (requested.max(1) as u32).min(available);
    if let Some(width) = features.width {
        viewport.screen_width = Some(screen_width);
        viewport.width = dimension(width, screen_width);
    }
    if let Some(height) = features.height {
        viewport.screen_height = Some(screen_height);
        viewport.height = dimension(height, screen_height);
    }
    let position = |requested: Option<i32>, available: u32| {
        (requested.unwrap_or(0).max(0) as u32).min(available) as i32
    };
    (features.is_popup, position(features.left, screen_width.saturating_sub(viewport.width)),
        position(features.top, screen_height.saturating_sub(viewport.height)))
}

#[derive(Clone)]
pub(crate) struct Settings {
    pub local: pyro_webapi::StorageNamespace,
    pub services: Rc<pyro_webapi::ContextServices>,
    pub max_touch_points: u32,
    pub session: pyro_webapi::StorageNamespace,
    pub net: Option<pyro_net::NetClient>,
    pub viewport: pyro_dom::media::Viewport,
    pub window_chrome: (bool, i32, i32),
    pub user_agent: Option<pyro_webapi::UserAgentOverride>,
    pub extra_headers: pyro_net::http::HeaderMap,
    pub cache_disabled: bool,
    pub bypass_service_worker: bool,
    pub scripts: Vec<InitScript>,
    pub bindings: Vec<crate::Binding>,
}

thread_local! {
    /// A main-world realm created ahead of its page, by this thread's browser
    /// while idle. Realm creation installs ~170 interfaces (about 0.9 ms), so
    /// a connection's first page saves that when the spare is ready.
    static SPARE_REALM: RefCell<Option<Realm>> = const { RefCell::new(None) };
}

/// Creates the spare realm for this thread if none is waiting. `class` is
/// the Window class of the thread's engine.
pub(crate) fn prepare_spare_realm(class: pyro_js::ClassId) -> Result<(), JsError> {
    if SPARE_REALM.with(|s| s.borrow().as_ref().is_some_and(|r| !r.is_disposed())) {
        return Ok(());
    }
    let realm = Engine::create_realm_in_current_thread(RealmOptions {
        name: String::new(),
        global_class: Some(class),
        origin: None,
    })?;
    SPARE_REALM.with(|s| *s.borrow_mut() = Some(realm));
    Ok(())
}

/// Drops this thread's spare realm (engine teardown).
pub(crate) fn has_spare_realm() -> bool {
    SPARE_REALM.with(|s| s.borrow().is_some())
}

pub(crate) fn drop_spare_realm() {
    SPARE_REALM.with(|s| {
        if let Some(r) = s.borrow_mut().take() {
            r.dispose();
        }
    });
}

/// The child key a child frame's id spells (`{key:032X}`), if it is one.
fn child_of_frame_id(id: &str) -> Option<u64> {
    let hex = id.as_bytes();
    if hex.len() != 32 || !hex.iter().all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(b)) {
        return None;
    }
    u64::try_from(u128::from_str_radix(id, 16).ok()?).ok()
}

/// The page and frame a realm's `Window` belongs to, read from the realm's
/// own global data (every frame realm gets it in `make_realm`). Says nothing
/// about whether the frame is still live; see [`Browser::has_live_realm`].
pub fn realm_page(scope: &pyro_js::Scope<'_>) -> Option<PageId> {
    scope.global_data::<Window>()?.frame_key().map(|key| key.page)
}

/// Frame keys of one page without a heap allocation for up to four frames.
pub type FrameKeys = smallvec::SmallVec<[FrameKey; 4]>;

enum InitialIframeLoad {
    Navigate,
    Suppressed,
    Blank { url: url::Url, explicit: bool },
}

pub(crate) type LazyFrameStamp = (((u64, u64, u32, u32), u64), bool, bool);

pub(crate) struct FrameHost {
    pub children: RefCell<BTreeMap<u64, DocumentState>>,
    pub root: RefCell<Frame>,
    root_nav: Rc<PageNav>,
    pub dom: pyro_dom::parse::DomRef,
    pub(crate) cluster_group: Cell<pyro_webapi::AgentClusterGroup>,
    /// The browsing context group's historical agent cluster key map (HTML § 7.1.4 "obtain a
    /// similar-origin window agent"): per origin, whether its agent cluster is origin-keyed. Shared
    /// by the pages of the group (a popup takes its opener's), as `cluster_group` is.
    origin_keying: RefCell<OriginKeying>,
    pub settings: RefCell<Settings>,
    pub page: PageId,
    pub context: ContextId,
    /// The bodies its documents' layouts load, its context's.
    pub layout_bodies: std::sync::Arc<pyro_dom::layout::SubresourceBodies>,
    pub sched: Scheduler<Browser>,
    pub handle: LoopHandle<Browser>,
    pub observer: ObserverCell,
    pub next: Rc<Cell<u64>>,
    pub started: Instant,
    retired: RefCell<Vec<DocumentState>>,
    class: Cell<pyro_js::ClassId>,
    pub(crate) focus: RefCell<crate::focus::FocusState>,
    /// The page that opened this one as a popup with its opener kept
    /// (`Browser::adopt_popup`): the top-level window's `opener`.
    pub(crate) opener: RefCell<Option<std::rc::Weak<FrameHost>>>,
    /// For such a popup, the ticket through which the opener's document
    /// reaches it (its `window.open` result is `event.source` there).
    pub(crate) popup: RefCell<Option<Rc<pyro_webapi::PopupTicket>>>,
    /// For a popup made by `window.open` (`prebuild_popup`): its creator document's secure-context
    /// state when it was made, which its about:blank documents (the initial one, and later ones that
    /// inherit the creator's origin) take as theirs (HTML: a new top-level browsing context's
    /// initial document's environment comes from its creator's), not one computed as top-level.
    pub(crate) creator_secure: Cell<Option<bool>>,
    /// The other pages sharing this page's DOM store: the popups it opened, and its opener's group
    /// (a popup takes its opener's store, see `prebuild_popup`). Their realms are siblings of this
    /// page's for node wrappers (`sibling_realms`).
    pub(crate) dom_peers: RefCell<Vec<std::rc::Weak<FrameHost>>>,
    /// Containers whose frame was removed while that frame's document was dispatching its window
    /// `load` (Chrome: re-inserted during that dispatch, the iframe gets a frame that does not navigate).
    pub(crate) quiet_reinserts: RefCell<Vec<(NodeId, std::rc::Weak<pyro_webapi::PageDom>)>>,
    /// Each child frame's WindowProxy: its first main-world realm's global
    /// proxy, which every later document's realm adopts (`make_realm`), so
    /// that `iframe.contentWindow` stays one object that follows the frame.
    window_proxies: RefCell<BTreeMap<u64, pyro_js::Persistent>>,
    /// `<object>`/`<embed>` elements showing a document that were not being rendered when they
    /// would have got their child navigable (`display: none`): they get it once rendered
    /// ([`FrameHost::connect_rendered_objects`]).
    pending_objects: RefCell<Vec<(FrameKey, LoaderId, NodeId)>>,
    /// `<object>`/`<embed>` elements not fetched because they were not being rendered: fetched
    /// once rendered (`Browser::fetch_rendered_objects`).
    pub(crate) hidden_objects: RefCell<Vec<(FrameKey, NodeId)>>,
    /// Lazy iframes whose navigation waits for the lazy load check (`Browser::resume_lazy_frames`),
    /// each with the parent's observation stamp, `load` state and laziness at its last check.
    pub(crate) lazy_frames: RefCell<Vec<(FrameKey, Option<LazyFrameStamp>)>>,
    /// Lazy iframes the check let go: their next attribute processing navigates.
    pub(crate) lazy_approved: RefCell<Vec<FrameKey>>,
    /// Lazy iframes navigated by something else (a link, `location`, a reload) while their
    /// lazy load was pending: HTML clears their lazy load resumption steps, so it never runs.
    pub(crate) lazy_cancelled: RefCell<Vec<FrameKey>>,
    /// The frames whose attribute processing is navigating them now (such a navigation cancels
    /// nothing; another frame's navigation meanwhile still does).
    pub(crate) attribute_navigation: RefCell<Vec<FrameKey>>,
    /// The layout engine's sync stamp when the hidden objects were last checked.
    pub(crate) hidden_checked_at: Cell<Option<(u64, u64, u32, u32)>>,
}
type OriginKeying = Rc<RefCell<std::collections::HashMap<String, bool>>>;

/// A popup page made when `window.open` ran, before the embedder creates the
/// page: its frame tree with the initial about:blank document and realms
/// (HTML creates that document synchronously, so the opener can script it at
/// once). [`Browser::create_page_with`] turns it into the page.
pub(crate) struct PrebuiltPopup {
    pub id: PageId,
    pub host: Rc<FrameHost>,
    pub nav: Rc<PageNav>,
    pub frame: Frame,
}

impl FrameHost {
    /// HTML "obtain a similar-origin window agent": the first document of `origin` in the browsing
    /// context group decides whether its agent cluster is origin-keyed (`requested`: it asked with
    /// `Origin-Agent-Cluster: ?1` in a secure context); later documents of the origin keep it.
    pub(crate) fn record_origin_keying(&self, origin: &str, requested: bool) {
        let map = self.origin_keying.borrow().clone();
        map.borrow_mut().entry(origin.to_owned()).or_insert(requested);
    }

    /// Whether `origin`'s agent cluster in this browsing context group is origin-keyed (an origin
    /// no document recorded is site-keyed).
    pub(crate) fn origin_keyed(&self, origin: &str) -> bool {
        let map = self.origin_keying.borrow().clone();
        let keyed = map.borrow().get(origin).copied().unwrap_or(false);
        keyed
    }

    /// Joins `opener`'s browsing context group's key map (a popup adopted with its opener kept).
    pub(crate) fn share_origin_keying(&self, opener: &FrameHost) {
        let map = opener.origin_keying.borrow().clone();
        *self.origin_keying.borrow_mut() = map;
    }

    /// The popup page `window.open` asks for, made now in this page's
    /// context: a top-level frame whose initial about:blank document has the
    /// opener frame's origin and base URL and the session storage copied for
    /// it, with its main-world realm (and the worlds init scripts name).
    pub(crate) fn prebuild_popup(
        self: &Rc<Self>,
        opener: FrameKey,
        session: pyro_webapi::StorageNamespace,
        name: String,
        features: pyro_webapi::window_features::WindowFeatures,
    ) -> Result<PrebuiltPopup, JsError> {
        let from = self.frame(opener).ok_or(JsError::ContextDestroyed)?;
        let creator_secure = crate::window::frame_chain_secure(self, opener, &from.url);
        let id = PageId(self.next() as u32);
        let frame = Frame {
            id: FrameId(format!("{:032X}", self.next())),
            parent: None,
            owner: None,
            name,
            base_url: from.base_url.clone(),
            sandbox: Default::default(),
            fullscreen_container: Default::default(),
            url: "about:blank".into(),
            loader_id: LoaderId(format!("{:032X}", self.next())),
            realms: Vec::new(),
            dom: None,
            document: None,
            page_dom: None,
            network_idle: NetworkIdle::default(),
            origin: from.origin.clone(),
            opaque_origin: from.opaque_origin,
            dom_content_loaded: false,
            loaded: false,
            unreachable: None,
        };
        let nav = PageNav::new(id, frame.id.clone(), self.sched.clone(), self.observer.clone());
        let mut settings = self.settings.borrow().clone();
        settings.session = session;
        settings.viewport = popup_viewport(&settings.viewport);
        settings.window_chrome = apply_window_features(&mut settings.viewport, features);
        settings.user_agent = None;
        settings.extra_headers = Default::default();
        let host = Rc::new(Self {
            children: RefCell::new(BTreeMap::new()),
            root: RefCell::new(frame.clone()),
            root_nav: nav.clone(),
            // The opener's DOM store, as its frames share it: a node of the opener's documents can be
            // adopted into the popup's (`w.document.body.appendChild(node)`), which a store of its
            // own could not hold (HTML: one browsing context group, one event loop).
            dom: self.dom.clone(),
            cluster_group: Cell::new(self.cluster_group.get()),
            origin_keying: RefCell::new(self.origin_keying.borrow().clone()),
            settings: RefCell::new(settings),
            page: id,
            context: self.context,
            layout_bodies: self.layout_bodies.clone(),
            sched: self.sched.clone(),
            handle: self.handle.clone(),
            observer: self.observer.clone(),
            next: self.next.clone(),
            started: self.started,
            retired: RefCell::new(Vec::new()),
            class: Cell::new(self.class.get()),
            // Its `window.opener` from the start (`Browser::adopt_popup` sets it again).
            opener: RefCell::new(Some(Rc::downgrade(self))),
            popup: RefCell::new(None),
            creator_secure: Cell::new(None),
            dom_peers: RefCell::new(Vec::new()),
            quiet_reinserts: RefCell::new(Vec::new()),
            window_proxies: RefCell::new(BTreeMap::new()),
            pending_objects: RefCell::new(Vec::new()),
            hidden_objects: RefCell::new(Vec::new()),
            lazy_frames: RefCell::new(Vec::new()),
            lazy_approved: RefCell::new(Vec::new()),
            lazy_cancelled: RefCell::new(Vec::new()),
            attribute_navigation: RefCell::new(Vec::new()),
            hidden_checked_at: Cell::new(None),
            focus: RefCell::new(crate::focus::FocusState::new(id)),
        });
        host.creator_secure.set(Some(creator_secure));
        // One DOM store: each reaches the other's realms for node wrappers.
        self.dom_peers.borrow_mut().push(Rc::downgrade(&host));
        host.dom_peers.borrow_mut().push(Rc::downgrade(self));
        let key: FrameKey = id.into();
        let dom = bindings::create(&host, key, "about:blank", &frame.loader_id)?;
        // Nothing loads into it: it is complete as made (as in browsers).
        dom.set_ready_state(dom.document(), "complete");
        nav.commit_document("about:blank", HistoryHandling::Replace, Some((host.dom.clone(), dom.document())));
        nav.set_page_dom(&dom);
        {
            let mut root = host.root.borrow_mut();
            root.dom = Some(host.dom.clone());
            root.document = Some(dom.document());
            root.page_dom = Some(dom);
        }
        let scripts = host.settings.borrow().scripts.clone();
        let mut worlds = vec![None];
        for script in scripts {
            if script.world.is_some() && !worlds.contains(&script.world) {
                worlds.push(script.world);
            }
        }
        for world in worlds {
            host.make_realm(key, nav.clone(), world)?;
        }
        let frame = host.root.borrow().clone();
        Ok(PrebuiltPopup { id, host, nav, frame })
    }

    pub fn new(browser: &Browser, page: PageId, context: ContextId, root: Frame, nav: Rc<PageNav>) -> Rc<Self> {
        let state = browser.contexts.iter().find(|c| c.id == context).expect("context");
        let (local, services) = (state.local_storage.clone(), state.services.clone());
        let layout_bodies = state.layout_bodies.clone();
        Rc::new(Self {
            children: RefCell::new(BTreeMap::new()),
            root: RefCell::new(root),
            root_nav: nav,
            dom: Rc::new(RefCell::new(pyro_dom::Dom::new())),
            cluster_group: Cell::default(),
            origin_keying: RefCell::default(),
            settings: RefCell::new(Settings {
                local,
                services,
                max_touch_points: 0,
                session: Default::default(),
                net: browser.context_net(context).cloned(),
                viewport: Default::default(),
                window_chrome: Default::default(),
                user_agent: None,
                extra_headers: Default::default(),
                cache_disabled: false,
                bypass_service_worker: false,
                scripts: Vec::new(),
                bindings: Vec::new(),
            }),
            page,
            context,
            layout_bodies,
            sched: browser.sched.clone(),
            handle: browser.handle.clone(),
            observer: browser.observer.clone(),
            next: browser.next_id.clone(),
            started: browser.started,
            retired: RefCell::new(Vec::new()),
            class: Cell::new(browser.engine.register::<Window>()),
            opener: RefCell::new(None),
            popup: RefCell::new(None),
            creator_secure: Cell::new(None),
            dom_peers: RefCell::new(Vec::new()),
            quiet_reinserts: RefCell::new(Vec::new()),
            window_proxies: RefCell::new(BTreeMap::new()),
            pending_objects: RefCell::new(Vec::new()),
            hidden_objects: RefCell::new(Vec::new()),
            lazy_frames: RefCell::new(Vec::new()),
            lazy_approved: RefCell::new(Vec::new()),
            lazy_cancelled: RefCell::new(Vec::new()),
            attribute_navigation: RefCell::new(Vec::new()),
            hidden_checked_at: Cell::new(None),
            focus: RefCell::new(crate::focus::FocusState::new(page)),
        })
    }
    fn next(&self) -> u64 {
        let id = self.next.get();
        self.next.set(id + 1);
        id
    }
    pub fn frame(&self, key: FrameKey) -> Option<Frame> {
        self.with_frame(key, Frame::clone)
    }
    /// Reads a frame without cloning it. `f` must not touch this host.
    pub fn with_frame<R>(&self, key: FrameKey, f: impl FnOnce(&Frame) -> R) -> Option<R> {
        match key.child {
            None => Some(f(&self.root.borrow())),
            Some(id) => self.children.borrow().get(&id).map(|d| f(&d.main_frame)),
        }
    }
    /// The frame's document bindings, if any (an `Rc` clone, not a `Frame` clone).
    pub fn page_dom(&self, key: FrameKey) -> Option<Rc<pyro_webapi::PageDom>> {
        self.with_frame(key, |f| f.page_dom.clone()).flatten()
    }
    /// Whether this page has any child frames.
    pub fn has_children(&self) -> bool {
        !self.children.borrow().is_empty()
    }
    pub fn key(&self, id: &FrameId) -> Option<FrameKey> {
        self.key_of(&id.0)
    }
    /// [`FrameHost::key`] for an id given as text.
    pub fn key_of(&self, id: &str) -> Option<FrameKey> {
        if self.root.borrow().id.0 == id {
            return Some(self.page.into());
        }
        // A child frame's id is its key in hex (`attach`): no search.
        let child = child_of_frame_id(id)?;
        let found = self.children.borrow().get(&child).is_some_and(|d| d.main_frame.id.0 == id);
        found.then_some(FrameKey { page: self.page, child: Some(child) })
    }
    /// Root first, then child frames. Inline for the common single-frame
    /// page: this runs several times per checkpoint.
    pub fn keys(&self) -> FrameKeys {
        std::iter::once(self.page.into())
            .chain(self.children.borrow().keys().map(|id| FrameKey { page: self.page, child: Some(*id) }))
            .collect()
    }
    pub fn owner(&self, node: NodeId) -> Option<FrameKey> {
        self.children
            .borrow()
            .iter()
            .find(|(_, d)| d.main_frame.owner == Some(node))
            .map(|(id, _)| FrameKey { page: self.page, child: Some(*id) })
    }
    fn emit(&self, event: PageEvent) {
        emit_to(&self.observer, self.page, &event);
    }
    pub fn matches(&self, key: FrameKey, loader: &LoaderId) -> bool {
        self.with_frame(key, |f| f.loader_id == *loader).unwrap_or(false)
    }
    /// The session history of the frame `key`.
    pub(crate) fn nav(&self, key: FrameKey) -> Option<Rc<PageNav>> {
        match key.child {
            None => Some(self.root_nav.clone()),
            Some(id) => self.children.borrow().get(&id).map(|child| child.nav.clone()),
        }
    }
    pub fn make_realm(
        self: &Rc<Self>,
        key: FrameKey,
        nav: Rc<PageNav>,
        world: Option<String>,
    ) -> Result<Realm, JsError> {
        let frame = self.frame(key).ok_or(JsError::ContextDestroyed)?;
        let dom = frame.page_dom.ok_or(JsError::ContextDestroyed)?;
        let class = self.window_class();
        let (scripts, bindings) = {
            let settings = self.settings.borrow();
            let scripts: Vec<InitScript> = settings.scripts.iter().filter(|s| s.world == world).cloned().collect();
            let bindings: Vec<Binding> =
                settings.bindings.iter().filter(|b| b.applies_to(world.as_deref())).cloned().collect();
            (scripts, bindings)
        };
        let options = RealmOptions {
            name: world.clone().unwrap_or_default(),
            global_class: Some(class),
            origin: Some(frame.origin.clone()),
        };
        // An isolated world that nothing fills when its document is made
        // (no init script with any source, no binding) is made when first
        // entered: Playwright's utility world, announced for every document
        // (`Page.addScriptToEvaluateOnNewDocument` with an empty source,
        // then `Page.createIsolatedWorld`), is entered only by its actions,
        // and a realm costs about 1.3 ms of CPU (as much as the main world).
        // The realm exists now, with its id and origin, for the events and
        // lookups; its context, bindings and init scripts (all empty) come
        // with its first `scope()`, wherever that happens.
        let deferred = world.is_some()
            && bindings.is_empty()
            && scripts.iter().all(|script| script.source.trim().is_empty());
        if deferred {
            let host = Rc::downgrade(self);
            let (setup_nav, setup_dom, setup_world) = (nav, dom, world.clone());
            let setup: pyro_js::DeferredSetup = Box::new(move |realm: &Realm| {
                let host = host.upgrade().ok_or(JsError::ContextDestroyed)?;
                host.setup_realm(realm, key, setup_nav, &setup_dom)?;
                host.finish_realm(realm, key, &setup_world, &[], &[])
            });
            let realm = Engine::defer_realm_in_current_thread(options, setup)?;
            self.push_realm(key, world.clone(), &realm)?;
            self.emit(PageEvent::RealmCreated { frame_id: frame.id, realm: realm.id(), world, origin: frame.origin });
            return Ok(realm);
        }
        // A main-world realm prepared while this thread was idle (see
        // `prepare_spare_realm`) only needs its origin; otherwise build one.
        let spare = if world.is_none() { SPARE_REALM.with(|s| s.borrow_mut().take()) } else { None };
        let realm = match spare.filter(|r| !r.is_disposed()) {
            Some(realm) => {
                realm.set_origin(Some(frame.origin.clone()));
                realm
            }
            None => Engine::create_realm_in_current_thread(options)?,
        };
        if world.is_none() {
            if let Some(child) = key.child {
                self.keep_window_proxy(child, &realm)?;
                // An iframe's initial about:blank Window: its first
                // same-origin document reuses it (`reuse_realm`).
                if nav.is_initial_blank() {
                    dom.set_reusable_window(true);
                }
            }
        }
        self.setup_realm(&realm, key, nav, &dom)?;
        self.push_realm(key, world.clone(), &realm)?;
        self.finish_realm(&realm, key, &world, &bindings, &scripts)?;
        self.emit(PageEvent::RealmCreated { frame_id: frame.id, realm: realm.id(), world, origin: frame.origin });
        Ok(realm)
    }
    /// HTML Window reuse: the main-world `realm` of the frame's initial
    /// about:blank document becomes the realm of its new document (already
    /// committed, same origin), whose Window target it took over
    /// (`pyro_webapi::adopt_window`). What script put on the window (its
    /// expandos, listeners, `onload`) stays; the bindings, frame indices and
    /// init scripts are set up for the new document as for a new realm.
    pub(crate) fn reuse_realm(self: &Rc<Self>, key: FrameKey, nav: Rc<PageNav>, realm: &Realm) -> Result<(), JsError> {
        let frame = self.frame(key).ok_or(JsError::ContextDestroyed)?;
        let dom = frame.page_dom.ok_or(JsError::ContextDestroyed)?;
        let scripts: Vec<InitScript> =
            self.settings.borrow().scripts.iter().filter(|s| s.world.is_none()).cloned().collect();
        let data = WindowData::for_frame(self.sched.clone(), nav, &dom, Rc::downgrade(self), key);
        {
            let scope = realm.scope()?;
            let old = scope.global_data::<Window>().ok_or(JsError::ContextDestroyed)?;
            old.hand_over(&data);
        }
        realm.replace_global_data::<Window>(data)?;
        realm.set_origin(Some(frame.origin.clone()));
        {
            let _setup = realm.setup_without_watchdog();
            let scope = realm.scope()?;
            pyro_webapi::rebind_window(&scope, dom.clone())?;
            // Per document, not per Window: a new History object.
            scope.forget_identity(pyro_js::IdentityKey(window::KEY_HISTORY, 0));
        }
        loader::install_module_fetcher(self.sched.clone(), key, realm);
        self.push_realm(key, None, realm)?;
        // Bindings stay installed on the global; init scripts run again.
        self.finish_realm(realm, key, &None, &[], &scripts)
    }
    /// The frame's WindowProxy for its new main-world `realm`: the proxy of
    /// the frame's first document, which `realm` adopts, as browsers keep one
    /// WindowProxy per browsing context (HTML "WindowProxy"). The first
    /// realm's own proxy becomes the frame's.
    fn keep_window_proxy(&self, child: u64, realm: &Realm) -> Result<(), JsError> {
        if !Realm::can_adopt_global_proxy() {
            return Ok(());
        }
        let kept = self.window_proxies.borrow().get(&child).map(|p| realm.adopt_global_proxy(p));
        match kept {
            Some(Ok(true)) => Ok(()),
            Some(Err(e)) => Err(e),
            None | Some(Ok(false)) => {
                let scope = realm.scope()?;
                let proxy = pyro_js::Persistent::new(&scope, scope.global().value());
                self.window_proxies.borrow_mut().insert(child, proxy);
                Ok(())
            }
        }
    }
    /// A made realm's frame state and bindings: the Window's data, the
    /// page's bindings (pyro-webapi), the module fetcher.
    fn setup_realm(
        self: &Rc<Self>,
        realm: &Realm,
        key: FrameKey,
        nav: Rc<PageNav>,
        dom: &Rc<pyro_webapi::PageDom>,
    ) -> Result<(), JsError> {
        realm.set_global_data::<Window>(WindowData::for_frame(
            self.sched.clone(),
            nav,
            dom,
            Rc::downgrade(self),
            key,
        ))?;
        {
            // Engine code only, up to the bindings and init scripts below.
            let _setup = realm.setup_without_watchdog();
            let scope = realm.scope()?;
            let helpers = pyro_webapi::install_with_helpers(&scope, dom.clone())?;
            // Uncaught exceptions of the realm's callbacks fire `error` at the Window.
            realm.set_exception_reporter(Some(pyro_webapi::lifecycle::report_exception));
            if crate::clock::page_clock_enabled() {
                scope.eval(crate::clock::DATE_SCRIPT, Some("pyro://internal/clock.js"), 1)?;
            }
            window::init_frame_indices(&scope, helpers.get_atom("accessor")?)?;
            window::init_location_unforgeables(&scope, helpers)?;
        }
        loader::install_module_fetcher(self.sched.clone(), key, realm);
        Ok(())
    }
    /// Lists the realm among its frame's (before any script of it runs:
    /// `world_of` finds it there).
    fn push_realm(&self, key: FrameKey, world: Option<String>, realm: &Realm) -> Result<(), JsError> {
        let entry = FrameRealm { world, realm: realm.clone() };
        match key.child {
            None => self.root.borrow_mut().realms.push(entry),
            Some(id) => {
                self.children.borrow_mut().get_mut(&id).ok_or(JsError::ContextDestroyed)?.main_frame.realms.push(entry)
            }
        }
        Ok(())
    }
    /// What runs in a listed realm: frame indices, `bindings` (the ones
    /// that apply to the world) and `scripts` (the world's init scripts).
    fn finish_realm(
        self: &Rc<Self>,
        realm: &Realm,
        key: FrameKey,
        world: &Option<String>,
        bindings: &[Binding],
        scripts: &[InitScript],
    ) -> Result<(), JsError> {
        window::refresh_frame_indices(realm, self, key)?;
        for binding in bindings.iter().filter(|b| b.applies_to(world.as_deref())) {
            let scope = realm.scope()?;
            if let Err(e) = scope.install_binding(&binding.name) {
                scope.report_exception(&e);
            }
        }
        for script in scripts.iter().filter(|s| s.world == *world) {
            let scope = realm.scope()?;
            if let Err(e) = scope.eval(&script.source, Some(&format!("init-script:{}", script.id)), 1) {
                scope.report_exception(&e);
            }
        }
        Ok(())
    }
    fn window_class(&self) -> pyro_js::ClassId {
        self.class.get()
    }

    fn refresh_indices(self: &Rc<Self>, key: FrameKey) {
        let realms = self.frame(key).map(|f| f.realms).unwrap_or_default();
        // A deferred realm defines its indices when it is made.
        for world in realms.into_iter().filter(|w| !w.realm.is_deferred()) {
            if let Err(e) = window::refresh_frame_indices(&world.realm, self, key) {
                if let Ok(scope) = world.realm.scope() {
                    scope.report_exception(&e);
                }
            }
        }
    }
    /// The pending `<object>`/`<embed>` elements (not rendered when connected) that are rendered
    /// now get their child navigable; those no longer connected, or no longer showing a document,
    /// are dropped. HTML runs the element's steps again when its rendering changes.
    pub fn connect_rendered_objects(self: &Rc<Self>, mut before_connect: impl FnMut(FrameKey)) {
        if self.pending_objects.borrow().is_empty() {
            return;
        }
        let pending = std::mem::take(&mut *self.pending_objects.borrow_mut());
        for (parent, loader, node) in pending {
            let keep = {
                let dom = self.dom.borrow();
                dom.is_connected(node).unwrap_or(false) && dom.is_document_object(node)
            };
            if keep && self.owner(node).is_none() {
                // Puts it back when it is still not rendered.
                before_connect(parent);
                let _ = self.connected(parent, &loader, node);
            }
        }
    }
    pub fn connected(self: &Rc<Self>, parent: FrameKey, loader: &LoaderId, node: NodeId) -> Result<(), JsError> {
        if !self.matches(parent, loader) || self.owner(node).is_some() {
            return Ok(());
        }
        let ancestor = self.frame(parent).ok_or(JsError::ContextDestroyed)?;
        let (name, mut sandbox) = {
            let dom = self.dom.borrow();
            if !dom.is_connected(node).unwrap_or(false) || dom.owner_document(node).ok().flatten() != ancestor.document
            {
                return Ok(());
            }
            // An `<object>` or `<embed>` that is not being rendered (`display: none`) represents
            // nothing: no child navigable (HTML "the embed element", "the object element").
            if dom.is_document_object(node) {
                if let Some(page_dom) = ancestor.page_dom.as_ref() {
                    if !page_dom.layout().check_visibility(&dom, node, Default::default()) {
                        let mut pending = self.pending_objects.borrow_mut();
                        if !pending.iter().any(|(_, _, n)| *n == node) {
                            pending.push((parent, loader.clone(), node));
                        }
                        return Ok(());
                    }
                }
            }
            (
                dom.get_attribute(node, "name").ok().flatten().unwrap_or("").to_owned(),
                Sandbox::parse(dom.get_attribute(node, "sandbox").ok().flatten()),
            )
        };
        sandbox.opaque |= ancestor.sandbox.opaque;
        sandbox.scripts_disabled |= ancestor.sandbox.scripts_disabled;
        sandbox.automatic_features_blocked |= ancestor.sandbox.automatic_features_blocked;
        sandbox.popups_blocked |= ancestor.sandbox.popups_blocked;
        // Blink's Page::kMaxNumberOfFrames: a page holds at most 1000 subframes.
        if self.children.borrow().len() >= 1000 {
            return Err(JsError::Range("child frame limit reached".into()));
        }
        let mut depth = 0;
        let mut current = Some(ancestor.id.clone());
        while let Some(id) = current {
            depth += 1;
            if depth > 32 {
                return Err(JsError::Range("child frame depth limit reached".into()));
            }
            current = self.key(&id).and_then(|k| self.frame(k)).and_then(|f| f.parent);
        }
        let id = self.next();
        let key = FrameKey { page: self.page, child: Some(id) };
        let fid = FrameId(format!("{id:032X}"));
        debug_assert_eq!(child_of_frame_id(&fid.0), Some(id));
        let lid = LoaderId(format!("{:032X}", self.next()));
        let origin = if sandbox.opaque { "null".into() } else { ancestor.origin.clone() };
        let nav = PageNav::new_child(&self.root_nav, self.page, fid.clone(), self.sched.clone(), self.observer.clone());
        // The container's referrer policy when the initial about:blank document's navigation began.
        nav.snapshot_container_policy(crate::loader::element_referrer_policy(&self.dom.borrow(), node));
        let frame = Frame {
            id: fid.clone(),
            parent: Some(ancestor.id.clone()),
            owner: Some(node),
            name,
            url: "about:blank".into(),
            base_url: ancestor.base_url,
            loader_id: lid.clone(),
            origin,
            opaque_origin: if sandbox.opaque { self.next() } else { ancestor.opaque_origin },
            realms: Vec::new(),
            dom: None,
            document: None,
            page_dom: None,
            network_idle: Default::default(),
            dom_content_loaded: false,
            loaded: false,
            sandbox,
            fullscreen_container: fullscreen_container(&self.dom.borrow(), node, (ancestor.origin != "null").then_some(ancestor.origin.as_str())),
            unreachable: None,
        };
        self.dom.borrow_mut().add_ref(node).map_err(|e| JsError::type_error(format!("{e:?}")))?;
        self.children.borrow_mut().insert(
            id,
            DocumentState {
                main_frame: frame,
                nav: nav.clone(),
                pending_loader: None,
                pending_frame_attributes: 0,
                pending_cancel: None,
                pending_script: None,
                load: None,
                stylesheets: Default::default(),
                images: Default::default(),
                preloads: Default::default(),
                refresh: Default::default(),
                _owner_pin: Some(loader::ResourceNode { dom: self.dom.clone(), node }),
                in_order_scripts: Default::default(),
            },
        );
        self.refresh_indices(parent);
        self.emit(PageEvent::FrameChildFrameCreated { parent: ancestor.id, frame_id: fid.clone() });
        let result = (|| {
            let dom = bindings::create(self, key, "about:blank", &lid)?;
            if let Some(creator) = &ancestor.page_dom {
                dom.inherit_initial_referrer(creator);
            }
            // The initial empty Document is already complete, including during
            // the interval before a non-blank src commits.
            dom.set_ready_state(dom.document(), "complete");
            nav.commit_document("about:blank", HistoryHandling::Replace, Some((self.dom.clone(), dom.document())));
            nav.set_page_dom(&dom);
            {
                let mut children = self.children.borrow_mut();
                let child = children.get_mut(&id).ok_or(JsError::ContextDestroyed)?;
                child.main_frame.dom = Some(self.dom.clone());
                child.main_frame.document = Some(dom.document());
                child.main_frame.page_dom = Some(dom);
            }
            let scripts = self.settings.borrow().scripts.clone();
            let mut worlds = vec![None];
            for script in scripts {
                if script.world.is_some() && !worlds.contains(&script.world) {
                    worlds.push(script.world);
                }
            }
            for world in worlds {
                self.make_realm(key, nav.clone(), world)?;
            }
            self.emit(PageEvent::FrameNavigated {
                frame_id: fid,
                loader_id: lid.clone(),
                url: "about:blank".into(),
                reason: NavigationReason::InitialFrameNavigation,
                cdp_request_id: None,
                timestamp: self.started.elapsed().as_secs_f64(),
                unreachable_url: None,
            });
            // Chrome: an iframe re-inserted while the document of its previous frame is still
            // dispatching its window `load` (moved from that document's own load listener) gets a
            // frame that stays on its initial about:blank: it does not navigate, fires no `load` and
            // does not hold its parent's load (else a load listener that moves its iframe loops).
            let quiet = {
                let mut quiet = self.quiet_reinserts.borrow_mut();
                quiet.retain(|(_, page)| page.upgrade().is_some_and(|p| p.is_dispatching_load()));
                quiet.iter().position(|(n, _)| *n == node).map(|i| quiet.remove(i)).is_some()
            };
            let initial = self.initial_iframe_load(node, parent);
            if quiet || matches!(initial, InitialIframeLoad::Suppressed) {
                let mut children = self.children.borrow_mut();
                if let Some(child) = children.get_mut(&id) {
                    if let Some(dom) = &child.main_frame.page_dom {
                        dom.set_ready_state(dom.document(), "complete");
                    }
                    child.main_frame.dom_content_loaded = true;
                    child.main_frame.loaded = true;
                }
            } else if let InitialIframeLoad::Blank { url, explicit } = initial {
                // HTML "process the iframe attributes", initial insertion: run
                // the iframe load event steps before returning to the inserter.
                let (document, frame_id) = {
                    let mut children = self.children.borrow_mut();
                    let child = children.get_mut(&id).ok_or(JsError::ContextDestroyed)?;
                    child.main_frame.dom_content_loaded = true;
                    child.main_frame.loaded = true;
                    child.main_frame.url = url.to_string();
                    (child.main_frame.page_dom.clone().ok_or(JsError::ContextDestroyed)?, child.main_frame.id.clone())
                };
                if url.as_str() != "about:blank" { nav.update_initial_blank_url(url.as_str()); }
                if explicit { document.note_blank_requested(); }
                document.set_completely_loaded();
                let timestamp = self.started.elapsed().as_secs_f64();
                self.emit(PageEvent::FrameDomContentLoaded { frame_id: frame_id.clone(), loader_id: lid.clone(), timestamp });
                self.emit(PageEvent::FrameLoaded { frame_id, loader_id: lid.clone(), timestamp });
                nav.notify_timeline("navigation");
                if let Some(parent_dom) = ancestor.page_dom {
                    pyro_webapi::lifecycle::resource_loaded(&parent_dom, node, true);
                }
                self.sched.post(pyro_runtime::TaskSource::Navigation, move |b| b.watch_network_idle(key, lid));
            } else {
                self.queue_attributes(key, lid);
            }
            Ok(())
        })();
        if result.is_err() {
            self.detach(key);
        }
        result
    }

    /// HTML shared attribute processing, for an iframe's initial insertion.
    fn initial_iframe_load(&self, node: NodeId, parent: FrameKey) -> InitialIframeLoad {
        let dom = self.dom.borrow();
        let Ok(el) = dom.element(node) else { return InitialIframeLoad::Navigate };
        if !el.is_html() || &*el.name.local != "iframe" || el.attr("srcdoc").is_some() {
            return InitialIframeLoad::Navigate;
        }
        let src = el.attr("src");
        let url = src.filter(|raw| !raw.is_empty()).and_then(|raw| {
            let base = crate::loader::node_base_url(&dom, node).and_then(|url| url::Url::parse(&url).ok());
            url::Url::options().base_url(base.as_ref()).parse(raw).ok()
        }).unwrap_or_else(|| url::Url::parse("about:blank").expect("static URL"));
        let mut ancestor = parent;
        loop {
            let address = self.nav(ancestor).map(|nav| nav.url());
            if address.as_ref().is_some_and(|a| a.split('#').next() == url.as_str().split('#').next()) {
                return InitialIframeLoad::Suppressed;
            }
            let next = self.parent_key(ancestor);
            if next == ancestor { break; }
            ancestor = next;
        }
        if url.scheme() == "about" && url.cannot_be_a_base() && url.path() == "blank" {
            InitialIframeLoad::Blank { url, explicit: src.is_some() }
        } else {
            InitialIframeLoad::Navigate
        }
    }

    /// The documents of frame `key` and its descendants, the frame's first (pre-order).
    pub fn subtree_documents(&self, key: FrameKey) -> Vec<Rc<pyro_webapi::PageDom>> {
        let mut out = Vec::new();
        let mut stack = vec![key];
        while let Some(k) = stack.pop() {
            out.extend(self.page_dom(k));
            let mut children: Vec<FrameKey> = self.child_keys(k).into_iter().collect();
            children.reverse();
            stack.extend(children);
        }
        out
    }
    pub fn queue_attributes(&self, key: FrameKey, loader: LoaderId) {
        let Some(id) = key.child else { return };
        let parent = self.parent_key(key);
        let mut children = self.children.borrow_mut();
        let Some(child) = children.get_mut(&id) else { return };
        child.pending_frame_attributes += 1;
        // HTML "beginning navigation" snapshots the container's referrer policy now (its
        // `location.ancestorOrigins` masking), not when the queued navigation runs.
        let owner = child.main_frame.owner;
        let nav = child.nav.clone();
        drop(children);
        if let Some(owner) = owner {
            nav.snapshot_container_policy(crate::loader::element_referrer_policy(&self.dom.borrow(), owner));
        }
        self.sched.post(pyro_runtime::TaskSource::Navigation, move |b| {
            if let Some(mut doc) = b.doc_mut(key) {
                doc.pending_frame_attributes -= 1;
            }
            b.process_iframe_attributes(key, &loader);
            b.maybe_fire_load(parent);
        });
    }
    pub fn detach(self: &Rc<Self>, key: FrameKey) {
        self.invalidate_focus(key, true);
        let Some(frame) = self.frame(key) else { return };
        let descendants: Vec<_> = self
            .children
            .borrow()
            .iter()
            .filter(|(_, d)| d.main_frame.parent.as_ref() == Some(&frame.id))
            .map(|(id, _)| FrameKey { page: self.page, child: Some(*id) })
            .collect();
        for child in descendants {
            self.detach(child);
        }
        if let Some(id) = key.child {
            self.window_proxies.borrow_mut().remove(&id);
            let removed = self.children.borrow_mut().remove(&id);
            if let Some(state) = removed {
                self.retired.borrow_mut().push(state);
            }
            if let Some(parent) = frame.parent.and_then(|id| self.key(&id)) {
                self.refresh_indices(parent);
            }
        }
    }
    pub fn retire_children(self: &Rc<Self>, parent: FrameKey) {
        let Some(frame) = self.frame(parent) else { return };
        let keys: Vec<_> = self
            .children
            .borrow()
            .iter()
            .filter(|(_, d)| d.main_frame.parent.as_ref() == Some(&frame.id))
            .map(|(id, _)| FrameKey { page: self.page, child: Some(*id) })
            .collect();
        for key in keys {
            self.detach(key);
        }
    }
    pub fn has_retired(&self) -> bool {
        !self.retired.borrow().is_empty()
    }
    /// Ends the frames removed from this page (`detach`). Their windows linger while script
    /// still holds them, as a navigated child frame's do (`Realm::linger`): a removed iframe's
    /// window answers `parent`/`top` with null instead of throwing.
    pub fn drain_retired(&self) {
        self.drain_retired_with(true);
    }
    /// `drain_retired` for a page going away: the removed frames' realms end at once.
    pub fn drain_retired_disposing(&self) {
        self.drain_retired_with(false);
    }
    fn drain_retired_with(&self, linger: bool) {
        let retired = std::mem::take(&mut *self.retired.borrow_mut());
        for state in retired {
            let frame = state.main_frame;
            // pyro: what its layout loaded or waits for goes with it (`forget_document`).
            self.layout_bodies.forget_document(&crate::document_client(&frame));
            let linger = linger && frame.page_dom.is_some();
            if let Some(dom) = &frame.page_dom {
                if linger {
                    dom.detach();
                } else {
                    dom.dispose();
                }
            }
            let mut lingered = true;
            for world in frame.realms {
                if linger {
                    window::release_roots(&world.realm);
                } else {
                    window::cancel_timers(&world.realm);
                }
                if let Some(dom) = &frame.page_dom {
                    if linger {
                        pyro_webapi::lifecycle::realm_detached(dom, world.realm.id());
                    } else {
                        pyro_webapi::lifecycle::realm_disposed(dom, world.realm.id());
                    }
                }
                if !(linger && world.realm.linger()) {
                    lingered = false;
                    world.realm.dispose();
                }
                self.emit(PageEvent::RealmDestroyed { frame_id: frame.id.clone(), realm: world.realm.id() });
            }
            if linger && !lingered {
                if let Some(dom) = &frame.page_dom {
                    dom.dispose();
                }
            }
            if let Some(dom) = &frame.page_dom {
                let _ = dom.dom.borrow_mut().set_document_active(dom.document(), false);
            }
            if let Some(net) = self.settings.borrow().net.clone() {
                net.forget_activity(&frame.loader_id.0);
                if let Some(loader) = &state.pending_loader {
                    net.forget_activity(&loader.0);
                }
            }
            if let Some(parent) = frame.parent {
                self.emit(PageEvent::FrameChildFrameDetached { parent, frame_id: frame.id });
            }
        }
    }
}

/// A frame container's policy inputs for `fullscreen` as a navigation began
/// (`Frame::fullscreen_container`): `allowfullscreen`, the `allow` attribute, and the container's
/// declared origin, which the `'src'` allowlist token (the default of `allow`) stands for.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub(crate) struct FullscreenContainer {
    pub allowfullscreen: bool,
    pub allow: Option<String>,
    /// Permissions Policy "declared origin": opaque (`None`) for a `sandbox` without
    /// `allow-same-origin`; the container document's origin with `srcdoc` or without `src`; else the
    /// origin of the `src` URL, resolved against the container document.
    pub declared_origin: Option<String>,
}

/// `parent_origin`: the container document's origin as its frame has it (an about:blank or
/// about:srcdoc document's is the origin it inherited, which its URL does not tell), `None` if opaque.
pub(crate) fn fullscreen_container(dom: &pyro_dom::Dom, owner: NodeId, parent_origin: Option<&str>) -> FullscreenContainer {
    let attr = |name: &str| dom.get_attribute(owner, name).ok().flatten().map(str::to_owned);
    let tuple = |url: &url::Url| {
        let origin = url.origin();
        origin.is_tuple().then(|| origin.ascii_serialization())
    };
    let parent_origin = || parent_origin.map(str::to_owned);
    let sandboxed = attr("sandbox").is_some_and(|v| !v.split_ascii_whitespace().any(|t| t.eq_ignore_ascii_case("allow-same-origin")));
    let declared_origin = if sandboxed {
        None
    } else if attr("srcdoc").is_some() {
        parent_origin()
    } else {
        match attr("src").filter(|v| !v.trim().is_empty()) {
            Some(src) => url::Url::parse(&pyro_webapi::html::resolve_url(dom, owner, &src)).ok().as_ref().and_then(tuple),
            None => parent_origin(),
        }
    };
    FullscreenContainer { allowfullscreen: attr("allowfullscreen").is_some(), allow: attr("allow"), declared_origin }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Sandbox {
    pub opaque: bool,
    /// No script runs: the `sandbox` attribute without `allow-scripts`, or
    /// the page's `Emulation.setScriptExecutionDisabled`.
    pub scripts_disabled: bool,
    /// The sandboxed automatic features flag (meta refresh): the attribute
    /// without `allow-scripts` only. A page with script execution disabled
    /// still follows a refresh, as Chrome does.
    pub automatic_features_blocked: bool,
    pub popups_blocked: bool,
    /// The sandboxed navigation browsing context flag (any `sandbox` attribute): no navigating
    /// an ancestor below the top-level one.
    pub navigation_blocked: bool,
    /// The sandboxed top-level navigation flags: neither `allow-top-navigation` nor
    /// `allow-top-navigation-by-user-activation`.
    pub top_navigation_blocked: bool,
    /// Only `allow-top-navigation-by-user-activation`: the top-level one only with activation.
    pub top_navigation_needs_activation: bool,
}
impl Sandbox {
    pub fn parse(value: Option<&str>) -> Self {
        let Some(value) = value else { return Self::default() };
        let tokens: Vec<_> = value.split_ascii_whitespace().collect();
        let scripts_disabled = !tokens.iter().any(|t| t.eq_ignore_ascii_case("allow-scripts"));
        let top = tokens.iter().any(|t| t.eq_ignore_ascii_case("allow-top-navigation"));
        let top_by_activation =
            tokens.iter().any(|t| t.eq_ignore_ascii_case("allow-top-navigation-by-user-activation"));
        Self {
            opaque: !tokens.iter().any(|t| t.eq_ignore_ascii_case("allow-same-origin")),
            scripts_disabled,
            automatic_features_blocked: scripts_disabled,
            popups_blocked: !tokens.iter().any(|t| t.eq_ignore_ascii_case("allow-popups")),
            navigation_blocked: true,
            top_navigation_blocked: !top && !top_by_activation,
            top_navigation_needs_activation: !top && top_by_activation,
        }
    }
}
impl Browser {
    pub(crate) fn doc(&self, key: FrameKey) -> Option<DocumentRef<'_>> {
        let page = self.page(key.page)?;
        match key.child {
            None => Some(StateRef::Direct(&page.document)),
            Some(id) => Ref::filter_map(page.frames.children.borrow(), |c| c.get(&id)).ok().map(StateRef::Shared),
        }
    }
    pub(crate) fn doc_mut(&mut self, key: FrameKey) -> Option<DocumentMut<'_>> {
        let page = self.page_mut(key.page)?;
        match key.child {
            None => Some(StateMut::Direct(&mut page.document)),
            Some(id) => {
                RefMut::filter_map(page.frames.children.borrow_mut(), |c| c.get_mut(&id)).ok().map(StateMut::Shared)
            }
        }
    }
}

impl FrameHost {
    pub(crate) fn same_origin(&self, a: FrameKey, b: FrameKey) -> bool {
        let (Some(a), Some(b)) = (self.frame(a), self.frame(b)) else { return false };
        if a.origin == "null" || b.origin == "null" {
            a.origin == b.origin && a.opaque_origin == b.opaque_origin
        } else {
            a.origin == b.origin
        }
    }
    /// The iframe a navigation `target` from `source` names, if `source` may
    /// navigate it: its own subtree first, then same-origin frames of the page
    /// (a simplified "find a navigable by target name").
    /// HTML "the rules for choosing a navigable" for `_parent` and `_top` (ASCII
    /// case-insensitive) from frame `source`: `Some(Some(target))` when it may navigate that
    /// navigable, `Some(None)` when sandboxing forbids it (HTML "allowed by sandboxing to
    /// navigate"), None for any other name. A top-level frame is its own parent and top.
    pub fn ancestor_target(&self, source: FrameKey, name: &str, user_activation: bool) -> Option<Option<FrameKey>> {
        let top = name.eq_ignore_ascii_case("_top");
        if !top && !name.eq_ignore_ascii_case("_parent") {
            return None;
        }
        let target = if top { FrameKey::from(self.page) } else { self.parent_key(source) };
        if target == source {
            return Some(Some(source));
        }
        let Some(sandbox) = self.with_frame(source, |f| f.sandbox) else { return Some(None) };
        let allowed = if target.child.is_none() {
            !sandbox.top_navigation_blocked && (!sandbox.top_navigation_needs_activation || user_activation)
        } else {
            !sandbox.navigation_blocked
        };
        Some(allowed.then_some(target))
    }
    pub fn named_frame(&self, source: FrameKey, name: &str) -> Option<FrameKey> {
        if ["", "_blank", "_self", "_parent", "_top"].iter().any(|k| name.eq_ignore_ascii_case(k)) {
            return None;
        }
        let named: Vec<_> = self
            .children
            .borrow()
            .iter()
            .filter(|(_, d)| d.main_frame.name == name)
            .map(|(id, _)| FrameKey { page: self.page, child: Some(*id) })
            .collect();
        let in_subtree = |key: FrameKey| {
            let mut current = key;
            loop {
                if current == source {
                    return true;
                }
                let parent = self.parent_key(current);
                if parent == current {
                    return false;
                }
                current = parent;
            }
        };
        named
            .iter()
            .find(|&&key| in_subtree(key))
            .or_else(|| named.iter().find(|&&key| self.same_origin(source, key)))
            .copied()
    }
    pub(crate) fn parent_key(&self, key: FrameKey) -> FrameKey {
        self.frame(key).and_then(|f| f.parent).and_then(|id| self.key(&id)).unwrap_or(key)
    }
    pub fn child_keys(&self, key: FrameKey) -> FrameKeys {
        if !self.has_children() {
            return FrameKeys::new();
        }
        let Some(id) = self.with_frame(key, |f| f.id.clone()) else { return FrameKeys::new() };
        self.children
            .borrow()
            .iter()
            .filter(|(_, d)| d.main_frame.parent.as_ref() == Some(&id))
            .map(|(id, _)| FrameKey { page: self.page, child: Some(*id) })
            .collect()
    }
    pub fn realm(self: &Rc<Self>, key: FrameKey, world: Option<&str>) -> Result<Realm, JsError> {
        let frame = self.frame(key).ok_or(JsError::ContextDestroyed)?;
        if let Some(realm) = frame.realms.iter().find(|r| r.world.as_deref() == world) {
            return Ok(realm.realm.clone());
        }
        let nav = match key.child {
            None => self.root_nav.clone(),
            Some(id) => self.children.borrow().get(&id).ok_or(JsError::ContextDestroyed)?.nav.clone(),
        };
        self.make_realm(key, nav, world.map(str::to_owned))
    }
    fn world_of(&self, source: FrameKey, realm: pyro_js::RealmId) -> Result<Option<String>, JsError> {
        self.frame(source)
            .and_then(|f| f.realms.into_iter().find(|r| r.realm.id() == realm))
            .map(|r| r.world)
            .ok_or(JsError::ContextDestroyed)
    }
    /// Realms of `realm`'s world in the frames of this tree other than `source`.
    /// A removed frame's realms count until they are disposed (`drain_retired`):
    /// its script may still run, and the nodes it wrapped keep those wrappers.
    pub fn sibling_realms(&self, source: FrameKey, realm: pyro_js::RealmId) -> Vec<Realm> {
        let retired = self.retired.borrow();
        let world = match self.world_of(source, realm) {
            Ok(world) => world,
            Err(_) => {
                let of_retired =
                    retired.iter().flat_map(|d| d.main_frame.realms.iter()).find(|r| r.realm.id() == realm);
                match of_retired {
                    Some(r) => r.world.clone(),
                    None => return Vec::new(),
                }
            }
        };
        // A deferred realm has run no script: it holds no wrapper, no
        // listener and no event, which is what these lookups are after.
        let of = |frame: &Frame| {
            frame.realms.iter().find(|r| r.world == world && !r.realm.is_deferred()).map(|r| r.realm.clone())
        };
        let mut realms = Vec::new();
        if source.child.is_some() {
            realms.extend(of(&self.root.borrow()));
        }
        for (id, doc) in self.children.borrow().iter() {
            if source.child != Some(*id) {
                realms.extend(of(&doc.main_frame));
            }
        }
        for doc in retired.iter() {
            realms.extend(of(&doc.main_frame).filter(|r| r.id() != realm && !r.is_disposed()));
        }
        drop(retired);
        // The pages sharing the DOM store (opener and popups): all their frames' realms of the world.
        for peer in self.dom_group() {
            realms.extend(of(&peer.root.borrow()));
            for doc in peer.children.borrow().values() {
                realms.extend(of(&doc.main_frame));
            }
            for doc in peer.retired.borrow().iter() {
                realms.extend(of(&doc.main_frame).filter(|r| !r.is_disposed()));
            }
        }
        realms
    }

    /// The other live pages sharing this page's DOM store, through openers and popups.
    pub(crate) fn dom_group(&self) -> Vec<Rc<FrameHost>> {
        let mut out: Vec<Rc<FrameHost>> = Vec::new();
        let mut todo: Vec<Rc<FrameHost>> = self.dom_peers.borrow().iter().filter_map(std::rc::Weak::upgrade).collect();
        while let Some(host) = todo.pop() {
            if host.page == self.page || out.iter().any(|h| h.page == host.page) || !Rc::ptr_eq(&host.dom, &self.dom) {
                continue;
            }
            todo.extend(host.dom_peers.borrow().iter().filter_map(std::rc::Weak::upgrade));
            out.push(host);
        }
        out
    }
    pub fn document_value<'s>(
        self: &Rc<Self>,
        s: &pyro_js::Scope<'s>,
        source: FrameKey,
        target: FrameKey,
    ) -> pyro_js::JsResult<pyro_js::Value<'s>> {
        if !self.same_origin(source, target) {
            return Ok(s.null());
        }
        let name = self.world_of(source, s.realm().id())?;
        let realm = self.realm(target, name.as_deref())?;
        let scope = realm.scope()?;
        let doc = scope.global().get("document")?;
        let value = pyro_js::Persistent::new(&scope, doc);
        Ok(value.get(s))
    }
    pub fn window_value<'s>(
        self: &Rc<Self>,
        s: &pyro_js::Scope<'s>,
        source: FrameKey,
        target: FrameKey,
    ) -> pyro_js::JsResult<pyro_js::Value<'s>> {
        if source == target {
            return Ok(s.global().value());
        }
        if self.same_origin(source, target) {
            let name = self.world_of(source, s.realm().id())?;
            let realm = self.realm(target, name.as_deref())?;
            let scope = realm.scope()?;
            let value = pyro_js::Persistent::new(&scope, scope.global().value());
            return Ok(value.get(s));
        }
        let data = CrossWindowData { host: Rc::downgrade(self), source: Some(source), from: None, target, foreign: false };
        Ok(s.wrap::<CrossWindow>(data, Some(pyro_js::IdentityKey(0x4652414d45505258, target.child.unwrap_or(0))))?
            .value())
    }
    /// `window.opener` of frame `key`: the opener page's top-level window
    /// for the top-level frame of a popup that kept its opener (null for
    /// child frames, other pages, and a closed opener). Same-origin openers
    /// are reached directly; otherwise a cross-origin stand-in.
    pub fn opener_window<'s>(
        self: &Rc<Self>,
        s: &pyro_js::Scope<'s>,
        key: FrameKey,
    ) -> pyro_js::JsResult<pyro_js::Value<'s>> {
        // A popup whose opener set `popup.opener = null` before it existed.
        if key.child.is_some() || self.popup.borrow().as_ref().is_some_and(|t| t.opener_cleared()) {
            return Ok(s.null());
        }
        let Some(opener) = self.opener.borrow().as_ref().and_then(std::rc::Weak::upgrade) else { return Ok(s.null()) };
        let target = FrameKey::from(opener.page);
        let (Some(mine), Some(theirs)) = (self.frame(key), opener.frame(target)) else { return Ok(s.null()) };
        let same_origin = if mine.origin == "null" || theirs.origin == "null" {
            mine.origin == theirs.origin && mine.opaque_origin == theirs.opaque_origin
        } else {
            mine.origin == theirs.origin
        };
        if same_origin {
            let world = self.world_of(key, s.realm().id())?;
            let realm = opener.realm(target, world.as_deref())?;
            let scope = realm.scope()?;
            let value = pyro_js::Persistent::new(&scope, scope.global().value());
            return Ok(value.get(s));
        }
        opener_proxy(s, &opener, Some((Rc::downgrade(self), key)))
    }
    pub fn relation<'s>(
        self: &Rc<Self>,
        s: &pyro_js::Scope<'s>,
        key: FrameKey,
        top: bool,
    ) -> pyro_js::JsResult<pyro_js::Value<'s>> {
        // HTML `parent`/`top`: null once the window's navigable is gone (its container, or an
        // ancestor's, was removed: `detach` dropped the frame and its descendants).
        if self.with_frame(key, |_| ()).is_none() {
            return Ok(s.null());
        }
        self.window_value(s, key, if top { self.page.into() } else { self.parent_key(key) })
    }
    pub fn frame_element<'s>(
        self: &Rc<Self>,
        s: &pyro_js::Scope<'s>,
        key: FrameKey,
    ) -> pyro_js::JsResult<pyro_js::Value<'s>> {
        let parent = self.parent_key(key);
        if parent == key || !self.same_origin(key, parent) {
            return Ok(s.null());
        }
        let frame = self.frame(key).ok_or(JsError::ContextDestroyed)?;
        let owner = frame.owner.ok_or(JsError::ContextDestroyed)?;
        let dom = self.frame(parent).and_then(|f| f.page_dom).ok_or(JsError::ContextDestroyed)?;
        let world = self.world_of(key, s.realm().id())?;
        let realm = self.realm(parent, world.as_deref())?;
        let scope = realm.scope()?;
        let node = pyro_webapi::node::wrap_node(&scope, &dom, owner)?;
        Ok(pyro_js::Persistent::new(&scope, node.value()).get(s))
    }
}

/// The cross-origin stand-in for `opener`'s top-level window (one per
/// realm), as frame `from` (the popup's top-level frame) sees it.
/// Whether frames `a` and `b` (of any pages) are same origin.
fn frames_same_origin(a: &Frame, b: &Frame) -> bool {
    if a.origin == "null" || b.origin == "null" {
        a.origin == b.origin && a.opaque_origin == b.opaque_origin
    } else {
        a.origin == b.origin
    }
}

impl FrameHost {
    /// Frame `target` of this page as window `accessor` of another page (a
    /// popup's opener) sees it: its global when the two are same origin,
    /// else a cross-origin window (`CrossWindowData::foreign`), one object
    /// per frame and accessing realm.
    pub(crate) fn foreign_window<'s>(
        self: &Rc<Self>,
        s: &pyro_js::Scope<'s>,
        target: FrameKey,
        accessor: (&Rc<FrameHost>, FrameKey),
    ) -> pyro_js::JsResult<pyro_js::Value<'s>> {
        let (Some(theirs), Some(mine)) = (self.frame(target), accessor.0.frame(accessor.1)) else {
            return Err(JsError::ContextDestroyed);
        };
        if frames_same_origin(&theirs, &mine) {
            let realm = self.realm(target, None)?;
            let scope = realm.scope()?;
            let value = pyro_js::Persistent::new(&scope, scope.global().value());
            return Ok(value.get(s));
        }
        let key = pyro_js::IdentityKey(
            0x464f_5245_4947_4e00 ^ u64::from(self.page.0),
            target.child.unwrap_or(u64::MAX),
        );
        if let Some(o) = s.lookup_identity(key) {
            return Ok(o.value());
        }
        let data = CrossWindowData {
            host: Rc::downgrade(self),
            source: None,
            from: Some((Rc::downgrade(accessor.0), accessor.1)),
            target,
            foreign: true,
        };
        Ok(s.wrap::<CrossWindow>(data, Some(key))?.value())
    }

    /// The child window of frame `parent` with index `key` (`"0"`) or with
    /// the target name `key`, as `foreign_window` gives it; `None` for none.
    pub(crate) fn foreign_child<'s>(
        self: &Rc<Self>,
        s: &pyro_js::Scope<'s>,
        parent: FrameKey,
        key: &str,
        accessor: (&Rc<FrameHost>, FrameKey),
    ) -> pyro_js::JsResult<Option<pyro_js::Value<'s>>> {
        let children = self.child_keys(parent);
        let index = key.parse::<u32>().ok().filter(|i| i.to_string() == key);
        let child = match index {
            Some(i) => children.get(i as usize).copied(),
            None if key.is_empty() => None,
            None => children.iter().copied().find(|c| self.frame(*c).is_some_and(|f| f.name == key)),
        };
        child.map(|child| self.foreign_window(s, child, accessor)).transpose()
    }

    /// The `parent` or `top` of a `foreign` window, as its accessor sees it:
    /// the popup's WindowProxy for the popup's top-level frame.
    fn foreign_ancestor<'s>(
        self: &Rc<Self>,
        s: &pyro_js::Scope<'s>,
        w: &CrossWindowData,
        target: FrameKey,
    ) -> pyro_js::JsResult<pyro_js::Value<'s>> {
        let Some((from, key)) = w.from.as_ref().and_then(|(h, k)| Some((h.upgrade()?, *k))) else {
            return Err(JsError::ContextDestroyed);
        };
        if target != FrameKey::from(self.page) {
            return self.foreign_window(s, target, (&from, key));
        }
        let opener = from.frame(key).and_then(|f| f.page_dom);
        let popup = self.frame(target).and_then(|f| f.page_dom);
        match (opener, popup) {
            (Some(opener), Some(popup)) => {
                Ok(pyro_webapi::popup_window_proxy(s, &opener, &popup)?.unwrap_or_else(|| s.null()))
            }
            _ => Ok(s.null()),
        }
    }
}

fn opener_proxy<'s>(
    s: &pyro_js::Scope<'s>,
    opener: &Rc<FrameHost>,
    from: Option<(std::rc::Weak<FrameHost>, FrameKey)>,
) -> pyro_js::JsResult<pyro_js::Value<'s>> {
    let key = pyro_js::IdentityKey(0x4f50454e45525758, u64::from(opener.page.0));
    if let Some(o) = s.lookup_identity(key) {
        return Ok(o.value());
    }
    let data =
        CrossWindowData { host: Rc::downgrade(opener), source: None, from, target: opener.page.into(), foreign: false };
    Ok(s.wrap::<CrossWindow>(data, Some(key))?.value())
}

/// The window whose script is running (HTML's incumbent global, as
/// [`pyro_js::entry_realm`] approximates it): its frame host and key.
pub(crate) fn incumbent_window() -> Option<(Rc<FrameHost>, FrameKey)> {
    let realm = pyro_js::entry_realm()?;
    let scope = realm.scope().ok()?;
    let window = scope.global_data::<Window>()?;
    Some((window.frame_host()?, window.frame_key()?))
}

/// `event.source` of a message from window `source` to window `target`,
/// resolved in the realm reading it: the source's WindowProxy as the target
/// sees it (its global, a cross-origin stand-in, the popup's `opener`, or
/// the opener's `window.open` result). Null for a pair this cannot name (a
/// frame inside a popup or its opener), or once either side is gone.
fn message_source(
    target_host: &Rc<FrameHost>,
    target: FrameKey,
    source_host: &Rc<FrameHost>,
    source: FrameKey,
) -> pyro_webapi::event::MessageSource {
    let (th, sh) = (Rc::downgrade(target_host), Rc::downgrade(source_host));
    Rc::new(move |s| {
        let (Some(th), Some(sh)) = (th.upgrade(), sh.upgrade()) else { return Ok(s.null()) };
        let opener_of = |h: &FrameHost| h.opener.borrow().as_ref().and_then(std::rc::Weak::upgrade);
        let value = if Rc::ptr_eq(&th, &sh) {
            th.window_value(s, target, source)
        } else if source.child.is_none() && opener_of(&th).is_some_and(|o| Rc::ptr_eq(&o, &sh)) {
            th.opener_window(s, target)
        } else if target.child.is_none()
            && source.child.is_none()
            && opener_of(&sh).is_some_and(|o| Rc::ptr_eq(&o, &th))
        {
            let ticket = sh.popup.borrow().clone();
            match (ticket, th.page_dom(target)) {
                (Some(ticket), Some(dom)) => ticket.window_in(s, &dom),
                _ => Ok(s.null()),
            }
        } else {
            Ok(s.null())
        };
        Ok(value.unwrap_or_else(|_| s.null()))
    })
}

/// `postMessage` on window `target` of `host` (HTML "window post message
/// steps"), from window `source` (`None`: the target itself). The message
/// is cloned into the target's main-world realm now and delivered as a
/// task if the target's origin matches `targetOrigin`.
pub(crate) fn post_message<'s>(
    s: &pyro_js::Scope<'s>,
    a: &pyro_js::Args<'s>,
    source: Option<(Rc<FrameHost>, FrameKey)>,
    host: &Rc<FrameHost>,
    target: FrameKey,
) -> pyro_js::JsResult<()> {
    let (source_host, source) = source.unwrap_or_else(|| (host.clone(), target));
    // Origins compared by key: an opaque origin ("null" serialized) equals only itself.
    let incumbent_key = source_host.with_frame(source, frame_origin_key);
    let (message, target_origin, options) = pyro_webapi::window_message::post_message_args(s, a, incumbent_key.as_deref().unwrap_or("opaque:gone"))?;
    let found = host.with_frame(target, |f| Some((f.page_dom.clone()?, f.main_realm()?.clone(), frame_origin_key(f))));
    // A window whose document is gone receives nothing.
    let Some((page, realm, origin_key)) = found.flatten() else { return Ok(()) };
    let deliver = target_origin.allows(&origin_key);
    let window = (!Rc::ptr_eq(&source_host, host) || source != target)
        .then(|| message_source(host, target, &source_host, source));
    pyro_webapi::window_message::send_window_message(s, message, options, &page, &realm, incumbent_key, window, deliver)
}

/// A frame's origin key (`PageDom::origin_key`'s): its tuple origin, or `opaque:<token>`.
pub(crate) fn frame_origin_key(f: &Frame) -> String {
    if f.origin == "null" { format!("opaque:{}", f.opaque_origin) } else { f.origin.clone() }
}

pub(crate) struct CrossWindow;
pub(crate) struct CrossWindowData {
    host: std::rc::Weak<FrameHost>,
    /// The accessing frame, when it is in `host`'s page; `None` for a
    /// popup's view of its opener (another page), which is always
    /// cross-origin and whose target is its own top and parent.
    source: Option<FrameKey>,
    /// For a popup's view of its opener: the popup's frame host and frame.
    /// For a `foreign` window: the accessing window's.
    from: Option<(std::rc::Weak<FrameHost>, FrameKey)>,
    target: FrameKey,
    /// A child window of a popup as its opener (`from`, another page) sees
    /// it (`FrameHost::foreign_window`), rather than the popup's opener.
    foreign: bool,
}

impl CrossWindowData {
    /// The accessing window: its frame host and frame.
    fn accessor(&self, host: &Rc<FrameHost>) -> Option<(Rc<FrameHost>, FrameKey)> {
        match (self.source, &self.from) {
            (Some(source), _) => Some((host.clone(), source)),
            (None, Some((from, key))) => Some((from.upgrade()?, *key)),
            (None, None) => None,
        }
    }
}

/// `location` of a cross-origin window: only `href` (set) and `replace()`.
pub(crate) struct CrossLocation;
pub(crate) struct CrossLocationData {
    host: std::rc::Weak<FrameHost>,
    target: FrameKey,
    /// Base URL of the accessing document, which relative URLs resolve against.
    base: String,
}
impl CrossLocationData {
    fn navigate(&self, value: pyro_js::Value<'_>, replace: bool) -> pyro_js::JsResult<()> {
        let nav = self.host.upgrade().and_then(|h| h.nav(self.target)).ok_or(JsError::ContextDestroyed)?;
        crate::nav::navigate_cross_origin(&nav, &self.base, value, replace)
    }
}
impl pyro_js::JsClass for CrossLocation {
    const NAME: &'static str = "CrossOriginLocation";
    type Data = CrossLocationData;
    type Parent = pyro_js::NoParent;
    fn define(def: &mut pyro_js::ClassDef<Self>) {
        def.not_exposed();
        // Every name goes through here (no prototype reads cross-origin).
        def.named(true, |s, d, name| {
            if name != "replace" {
                return Err(JsError::dom(
                    "SecurityError",
                    "Blocked a frame from accessing a cross-origin frame's location.",
                ));
            }
            let data = CrossLocationData { host: d.host.clone(), target: d.target, base: d.base.clone() };
            let replace = s.function("replace", move |s, _, a| {
                a.require(1, "replace")?;
                data.navigate(a.get(0), true)?;
                Ok(s.undefined())
            })?;
            Ok(Some(replace.value()))
        });
        def.named_setter(|_, d, name, value| match name {
            "href" => d.navigate(value, false).map(|()| true),
            _ => Err(JsError::dom("SecurityError", "Blocked a cross-origin location write")),
        });
        def.named_deleter(|_, _, _| Err(JsError::dom("SecurityError", "Blocked a cross-origin location delete")));
    }
}

/// `w.location` of cross-origin window `w`, for the accessing window.
fn cross_location<'s>(
    s: &pyro_js::Scope<'s>,
    w: &CrossWindowData,
    host: &Rc<FrameHost>,
) -> pyro_js::JsResult<pyro_js::Value<'s>> {
    let base = w
        .accessor(host)
        .and_then(|(h, key)| h.with_frame(key, |f| f.base_url.clone()))
        .unwrap_or_else(|| "about:blank".into());
    let data = CrossLocationData { host: Rc::downgrade(host), target: w.target, base };
    Ok(s.wrap::<CrossLocation>(data, None)?.value())
}
impl pyro_js::JsClass for CrossWindow {
    const NAME: &'static str = "CrossOriginWindow";
    type Data = CrossWindowData;
    type Parent = pyro_js::NoParent;
    fn define(def: &mut pyro_js::ClassDef<Self>) {
        def.not_exposed();
        def.named(true, |s, w, name| {
            let Some(host) = w.host.upgrade() else {
                return if name == "closed" { Ok(Some(s.boolean(true))) } else { Err(JsError::ContextDestroyed) };
            };
            let value = match (name, w.source) {
                ("window" | "self" | "frames", None) if w.foreign => {
                    let Some((from, key)) = w.from.as_ref().and_then(|(h, k)| Some((h.upgrade()?, *k))) else {
                        return Err(JsError::ContextDestroyed);
                    };
                    host.foreign_window(s, w.target, (&from, key))?
                }
                ("parent", None) if w.foreign => host.foreign_ancestor(s, w, host.parent_key(w.target))?,
                ("top", None) if w.foreign => host.foreign_ancestor(s, w, host.page.into())?,
                ("window" | "self" | "frames" | "parent" | "top", None) => opener_proxy(s, &host, w.from.clone())?,
                ("window" | "self" | "frames", Some(source)) => host.window_value(s, source, w.target)?,
                ("parent", Some(source)) => host.window_value(s, source, host.parent_key(w.target))?,
                ("top", Some(source)) => host.window_value(s, source, host.page.into())?,
                ("length", _) => s.number(host.child_keys(w.target).len() as f64),
                ("closed", _) => s.boolean(host.frame(w.target).is_none()),
                ("focus", _) => {
                    let weak = w.host.clone();
                    let target = w.target;
                    s.function(name, move |s, _, _| {
                        if let Some(page) = weak.upgrade().and_then(|h| h.frame(target)).and_then(|f| f.page_dom) {
                            pyro_webapi::input::focus_window(&page);
                        }
                        Ok(s.undefined())
                    })?
                    .value()
                }
                ("close" | "blur", _) => s.function(name, |s, _, _| Ok(s.undefined()))?.value(),
                ("postMessage", _) => {
                    let (weak, target) = (w.host.clone(), w.target);
                    let source = w.accessor(&host).map(|(h, key)| (Rc::downgrade(&h), key));
                    s.function(name, move |s, _, a| {
                        let Some(host) = weak.upgrade() else { return Ok(s.undefined()) };
                        let source = source.as_ref().and_then(|(h, key)| Some((h.upgrade()?, *key)));
                        post_message(s, &a, source, &host, target)?;
                        Ok(s.undefined())
                    })?
                    .value()
                }
                ("location", _) => cross_location(s, w, &host)?,
                _ => return Err(JsError::dom("SecurityError", "Blocked cross-origin frame access")),
            };
            Ok(Some(value))
        });
        // `frames[i]` of a cross-origin window: its child windows, as the
        // accessing frame sees them.
        def.indexed(|s, w, index| {
            let Some(host) = w.host.upgrade() else { return Ok(None) };
            let child = host.child_keys(w.target).get(index as usize).copied();
            match (w.source, &w.from) {
                (Some(source), _) => child.map(|child| host.window_value(s, source, child)).transpose(),
                (None, Some((from, key))) if w.foreign => {
                    let Some(from) = from.upgrade() else { return Ok(None) };
                    child.map(|child| host.foreign_window(s, child, (&from, *key))).transpose()
                }
                _ => Ok(None),
            }
        });
        def.named_setter(|s, w, name, value| {
            // `w.location = url` navigates it, as `w.location.href = url` does.
            if name == "location" {
                if let Some(host) = w.host.upgrade() {
                    let location = cross_location(s, w, &host)?;
                    let data = s.unwrap::<CrossLocation>(location).ok_or(JsError::ContextDestroyed)?;
                    data.navigate(value, false)?;
                    return Ok(true);
                }
            }
            Err(JsError::dom("SecurityError", "Blocked cross-origin frame write"))
        });
        def.named_deleter(|_, _, _| Err(JsError::dom("SecurityError", "Blocked cross-origin frame delete")));
    }
}

impl Browser {
    pub(crate) fn sync_root(&self, page: PageId) {
        if let Some(p) = self.page(page) {
            // Runs at every checkpoint: compare only after a mutable access,
            // and clone only when the frames differ.
            if !p.root_dirty.replace(false) || p.frames.root.borrow().same_as(&p.main_frame) {
                return;
            }
            let mut frame = p.main_frame.clone();
            let mut root = p.frames.root.borrow_mut();
            if root.loader_id == frame.loader_id {
                for r in &root.realms {
                    if !frame.realms.iter().any(|v| v.realm.id() == r.realm.id()) {
                        frame.realms.push(r.clone());
                    }
                }
            }
            *root = frame;
        }
    }
    /// Snapshot of each live frame, including reached lifecycle milestones.
    pub fn frames(&self, page: PageId) -> Vec<FrameInfo> {
        let Some(p) = self.page(page) else { return Vec::new() };
        p.frames
            .keys()
            .into_iter()
            .filter_map(|key| {
                // Only the snapshot's fields are copied, not the whole frame.
                self.doc(key).map(|d| {
                    let f = &d.main_frame;
                    FrameInfo {
                        id: f.id.clone(),
                        parent: f.parent.clone(),
                        owner: f.owner,
                        url: f.url.clone(),
                        loader_id: f.loader_id.clone(),
                        name: f.name.clone(),
                        origin: f.origin.clone(),
                        network_idle: f.network_idle.clone(),
                        dom_content_loaded: f.dom_content_loaded,
                        loaded: f.loaded,
                        unreachable_url: f.unreachable.as_ref().map(|u| u.url.clone()),
                    }
                })
            })
            .collect()
    }
    /// Whether `realm` belongs to a live (not detached) frame of `page`. No
    /// frame is cloned.
    pub fn has_live_realm(&self, page: PageId, realm: RealmId) -> bool {
        let Some(p) = self.page(page) else { return false };
        self.sync_root(page);
        let holds = |f: &Frame| f.realms.iter().any(|r| r.realm.id() == realm);
        holds(&p.frames.root.borrow()) || p.frames.children.borrow().values().any(|d| holds(&d.main_frame))
    }
    /// Whether `page` has a live frame with this id.
    pub fn has_frame(&self, page: PageId, id: &str) -> bool {
        self.page(page).is_some_and(|p| p.frames.key_of(id).is_some())
    }
    /// Reads the live frame `id` of `page` without cloning it (the borrowed
    /// counterpart of [`Browser::frame`]). `f` must not mutate the browser.
    pub fn with_frame<R>(&self, page: PageId, id: &FrameId, f: impl FnOnce(&Frame) -> R) -> Option<R> {
        let host = &self.page(page)?.frames;
        let key = host.key(id)?;
        if key.child.is_none() {
            self.sync_root(page);
        }
        host.with_frame(key, f)
    }
    /// Reads the live frame of `page` whose document is `document`, root
    /// first, without cloning it. `f` must not mutate the browser.
    pub fn with_frame_of_document<R>(
        &self,
        page: PageId,
        document: &Rc<pyro_webapi::PageDom>,
        f: impl FnOnce(&Frame) -> R,
    ) -> Option<R> {
        let host = &self.page(page)?.frames;
        self.sync_root(page);
        let shows = |frame: &Frame| frame.page_dom.as_ref().is_some_and(|dom| Rc::ptr_eq(dom, document));
        {
            let root = host.root.borrow();
            if shows(&root) {
                return Some(f(&root));
            }
        }
        let children = host.children.borrow();
        children.values().map(|d| &d.main_frame).find(|frame| shows(frame)).map(f)
    }
    /// The page's realm with this id, searching the root frame first. A
    /// handle clone only; no frame is cloned (hot: every `Runtime.*` command).
    pub fn realm_in_page(&self, page: PageId, realm: RealmId) -> Option<Realm> {
        let p = self.page(page)?;
        let find = |f: &Frame| f.realms.iter().find(|r| r.realm.id() == realm).map(|r| r.realm.clone());
        if let Some(r) = find(&p.main_frame) {
            return Some(r);
        }
        p.frames.children.borrow().values().find_map(|d| find(&d.main_frame))
    }
    /// Owned snapshot with cloned realm and PageDom handles; never a live registry borrow.
    pub fn frame(&self, page: PageId, id: &FrameId) -> Option<Frame> {
        let key = self.page(page)?.frames.key(id)?;
        if key.child.is_none() {
            self.sync_root(page);
            self.page(page)?.frames.frame(key)
        } else {
            self.doc(key).map(|d| d.main_frame.clone())
        }
    }
    /// Whether `page` has any frame besides its main one.
    pub fn has_child_frames(&self, page: PageId) -> bool {
        self.page(page).is_some_and(|p| p.frames.has_children())
    }
    pub fn frame_of_owner(&self, page: PageId, owner: NodeId) -> Option<FrameId> {
        let host = &self.page(page)?.frames;
        host.owner(owner).and_then(|key| host.with_frame(key, |f| f.id.clone()))
    }
    pub fn isolated_world_in_frame(&mut self, page: PageId, id: &FrameId, name: &str) -> Result<Realm, JsError> {
        let host = self.page(page).ok_or(JsError::ContextDestroyed)?.frames.clone();
        let key = host.key(id).ok_or(JsError::ContextDestroyed)?;
        let realm = host.realm(key, Some(name))?;
        if key.child.is_none() {
            self.page_mut(page).expect("page").main_frame.realms = host.root.borrow().realms.clone();
        }
        Ok(realm)
    }
    pub fn navigate_frame(&mut self, page: PageId, id: &FrameId, opts: NavigateOpts) -> Result<LoaderId, JsError> {
        let key = self.page(page).and_then(|p| p.frames.key(id)).ok_or(JsError::ContextDestroyed)?;
        self.navigate_frame_with_handling(key, opts, HistoryHandling::Replace)
    }
    pub(crate) fn finish_blank_document(&mut self, key: FrameKey) {
        let Some(mut state) = self.doc_mut(key) else { return };
        if state.main_frame.dom_content_loaded {
            return;
        }
        state.main_frame.dom_content_loaded = true;
        let frame = state.main_frame.clone();
        drop(state);
        self.sync_root(key.page);
        if let Some(dom) = frame.page_dom {
            pyro_webapi::lifecycle::dom_content_loaded(&dom, dom.document());
        }
        self.emit(
            key.page,
            PageEvent::FrameDomContentLoaded {
                frame_id: frame.id,
                loader_id: frame.loader_id,
                timestamp: self.timestamp(),
            },
        );
        self.maybe_fire_load(key);
    }
    pub(crate) fn process_iframe_attributes(&mut self, key: FrameKey, loader: &LoaderId) {
        let Some(host) = self.page(key.page).map(|p| p.frames.clone()) else { return };
        // The cancel marker belongs to this queued processing: taken before any early return, so
        // a stale one cannot swallow a later lazy navigation of the frame.
        let lazy_cancelled = {
            let mut cancelled = host.lazy_cancelled.borrow_mut();
            cancelled.iter().position(|k| *k == key).map(|i| cancelled.remove(i)).is_some()
        };
        let Some(frame) = host.frame(key).filter(|f| &f.loader_id == loader) else { return };
        let Some(owner) = frame.owner else { return };
        let Some(parent) = frame.parent.as_ref().and_then(|id| host.key(id)).and_then(|key| host.frame(key)) else {
            return;
        };
        if !host.dom.borrow().is_connected(owner).unwrap_or(false) {
            return;
        }
        if self.defer_lazy_frame(&host, key, owner, parent.page_dom.as_deref(), lazy_cancelled) {
            return;
        }
        host.attribute_navigation.borrow_mut().push(key);
        let (srcdoc, src, src_base, encoded_src) = {
            let dom = host.dom.borrow();
            // An `<object>` hosting a document navigates to its `data`, an `<embed>` to its `src`
            // (neither has `srcdoc`).
            let embedded = dom.is_document_object(owner);
            let object = embedded && dom.element(owner).is_ok_and(|e| &*e.name.local == "object");
            (
                dom.get_attribute(owner, "srcdoc").ok().flatten().filter(|_| !embedded).map(str::to_owned),
                dom.get_attribute(owner, if object { "data" } else { "src" }).ok().flatten().map(str::to_owned),
                // `<base href>` aware.
                crate::loader::node_base_url(&dom, owner),
                // HTML "encoding-parse a URL": the query in the container document's encoding.
                dom.get_attribute(owner, if object { "data" } else { "src" })
                    .ok()
                    .flatten()
                    .map(|raw| pyro_webapi::html::resolve_url(&dom, owner, raw)),
            )
        };
        let result = if let Some(html) = srcdoc {
            self.begin_srcdoc(key, frame.id.clone(), html, parent.url, HistoryHandling::Replace).map(|_| ())
        } else {
            let has_src = src.is_some();
            let raw = src.unwrap_or_default();
            let url = if raw.trim().is_empty() {
                "about:blank".to_string()
            } else if let Some(encoded) = encoded_src.filter(|_| src_base.is_some()) {
                encoded
            } else {
                url::Url::parse(src_base.as_deref().unwrap_or(&parent.base_url))
                    .and_then(|u| u.join(&raw))
                    .map(|u| u.to_string())
                    .unwrap_or_else(|_| raw.clone())
            };
            if let Some(source) = crate::bindings::javascript_url_source(&url) {
                // HTML "navigate to a javascript: URL" with initialInsertion: a task runs the
                // Trusted Types pre-navigation check and the script in the frame's initial
                // document, which stays; its load follows. Author code in that check (a default
                // policy) may remove the container: then nothing runs and no load is fired.
                let parent_key = frame.parent.as_ref().and_then(|id| host.key(id));
                // CSP: the URL is checked against the inserting document's policies (the
                // navigation's initiator), snapshotted now.
                let initiator =
                    parent_key.and_then(|k| host.page_dom(k)).map(|dom| pyro_webapi::csp::NavigationInitiator::of(&dom));
                let js_url = url.clone();
                self.sched.post(pyro_runtime::TaskSource::Navigation, move |b| {
                    let connected = |b: &Browser| {
                        b.page(key.page).is_some_and(|p| p.frames.dom.borrow().is_connected(owner).unwrap_or(false))
                    };
                    let Some(doc) = b.doc(key) else { return };
                    let realm = doc.main_frame.main_realm().cloned();
                    drop(doc);
                    // The check runs under the inserting document's policies (the request's
                    // policy container is snapshotted from it).
                    let checker = parent_key.and_then(|k| b.doc(k)).and_then(|d| {
                        Some((d.main_frame.main_realm().cloned()?, d.main_frame.page_dom.clone()?))
                    });
                    // A string result replaces the initial document (HTML "evaluate a javascript: URL").
                    let mut result = None;
                    // CSP `script-src-elem` of the initiator: blocked, no script runs (the load
                    // of the initial document still follows).
                    let allowed = initiator.as_ref().is_none_or(|i| i.allows_javascript_url(&js_url));
                    if let (true, Some(realm), Some((parent_realm, parent_dom))) = (allowed, realm, checker) {
                        if let (Ok(scope), Ok(parent_scope)) = (realm.scope(), parent_realm.scope()) {
                            let script = pyro_webapi::trusted_types::javascript_url_script(&parent_scope, &parent_dom, &source);
                            if !connected(b) {
                                return;
                            }
                            if let Some(script) = script {
                                result = javascript_url_result(&scope, &script);
                            }
                        }
                    }
                    if connected(b) {
                        match result {
                            Some(html) => {
                                if b.begin_javascript_result(key, html).is_err() {
                                    b.finish_blank_document(key);
                                }
                            }
                            None => b.finish_blank_document(key),
                        }
                    }
                });
                Ok(())
            } else if url == "about:blank" && frame.url == "about:blank" && !frame.dom_content_loaded {
                // A script that opened the initial document since (append,
                // then `contentDocument.open()`) finishes it with `close()`.
                if host.nav(key).map_or(true, |nav| nav.is_initial_blank()) {
                    if let (true, Some(dom)) = (has_src, &frame.page_dom) {
                        dom.note_blank_requested();
                    }
                    self.finish_blank_document(key);
                }
                Ok(())
            } else {
                // The container's `referrerpolicy`, else its document's policy (the request's client).
                let referrer_policy =
                    crate::loader::container_referrer_policy(&host.dom.borrow(), owner, parent.page_dom.as_deref());
                self.navigate_frame_with_handling(
                    key,
                    NavigateOpts {
                        post: None,
                        url,
                        reason: NavigationReason::InitialFrameNavigation,
                        cdp_request_id: None,
                        referrer: parent.navigation_referrer(),
                        referrer_policy,
                    },
                    // HTML "navigate an iframe or frame": "replace" while the frame's document is
                    // not completely loaded (its `load` and `pageshow` handlers still run, after
                    // the frame counts as loaded), else "auto", a push (which leaving the initial
                    // about:blank document still makes a replace). An `<object>` or `<embed>`
                    // always replaces.
                    let completely_loaded =
                        frame.loaded && frame.page_dom.as_ref().is_none_or(|dom| dom.is_completely_loaded());
                    if completely_loaded && !host.dom.borrow().is_document_object(owner) {
                        HistoryHandling::Push
                    } else {
                        HistoryHandling::Replace
                    },
                )
                .map(|_| ())
            }
        };
        {
            let mut navigating = host.attribute_navigation.borrow_mut();
            if let Some(i) = navigating.iter().rposition(|k| *k == key) {
                navigating.remove(i);
            }
        }
        if let Err(e) = result {
            if let Some(realm) = host.frame(key).and_then(|f| f.main_realm().cloned()) {
                if let Ok(scope) = realm.scope() {
                    scope.report_exception(&e);
                }
            }
        }
    }
}

impl Browser {
    /// HTML "will lazy load element steps" for an iframe (`loading=lazy`, scripting enabled in
    /// its document): its navigation (`src`, `srcdoc` or a `javascript:` URL) waits for the lazy
    /// load check, the frame staying on its complete initial `about:blank`, which holds up no
    /// `load`. False when the check let it go (or it is not lazy): navigate now.
    fn defer_lazy_frame(
        &mut self,
        host: &Rc<FrameHost>,
        key: FrameKey,
        owner: NodeId,
        parent_dom: Option<&pyro_webapi::PageDom>,
        cancelled: bool,
    ) -> bool {
        // HTML "process the iframe attributes": only the navigation to a `src` URL (not srcdoc,
        // not about:blank) waits on the lazy loading steps.
        let lazy = {
            let dom = host.dom.borrow();
            let attr = |name| dom.get_attribute(owner, name).ok().flatten().map(str::trim);
            is_lazy_iframe(&dom, owner)
                && attr("srcdoc").is_none()
                && attr("src").is_some_and(|src| !src.is_empty() && !src.eq_ignore_ascii_case("about:blank"))
        } && parent_dom.is_some_and(|d| !d.scripting_disabled());
        if !lazy {
            return false;
        }
        // Navigated otherwise first (before this queued processing ran): no lazy load, and this
        // processing navigates nowhere.
        if cancelled {
            return true;
        }
        {
            let mut approved = host.lazy_approved.borrow_mut();
            if let Some(i) = approved.iter().position(|k| *k == key) {
                approved.remove(i);
                return false;
            }
        }
        {
            let mut pending = host.lazy_frames.borrow_mut();
            match pending.iter_mut().find(|(k, _)| *k == key) {
                // A new navigation while deferred: checked again.
                Some(entry) => entry.1 = None,
                None => pending.push((key, None)),
            }
        }
        if let Some(mut doc) = self.doc_mut(key) {
            doc.main_frame.dom_content_loaded = true;
            doc.main_frame.loaded = true;
        }
        true
    }

    /// HTML "navigate" clears the container's lazy load resumption steps: a lazy iframe navigated
    /// other than by its attribute processing (a link, `location`, a reload, `window.open`)
    /// loses its pending lazy load, including one whose queued processing has not run yet.
    pub(crate) fn cancel_lazy_frame(&mut self, key: FrameKey) {
        if key.child.is_none() {
            return;
        }
        let Some(host) = self.page(key.page).map(|p| p.frames.clone()) else { return };
        if host.attribute_navigation.borrow().contains(&key) {
            return;
        }
        let Some(owner) = host.frame(key).and_then(|f| f.owner) else { return };
        if !is_lazy_iframe(&host.dom.borrow(), owner) {
            return;
        }
        host.lazy_frames.borrow_mut().retain(|(k, _)| *k != key);
        host.lazy_approved.borrow_mut().retain(|k| *k != key);
        let pending = self.doc(key).is_some_and(|d| d.pending_frame_attributes > 0);
        if pending {
            let mut cancelled = host.lazy_cancelled.borrow_mut();
            if !cancelled.contains(&key) {
                cancelled.push(key);
            }
        }
    }

    /// The deferred lazy iframes that stopped being lazy, or that intersect the viewport expanded
    /// by the lazy load margin once their document's `load` has fired, navigate (their
    /// attributes are processed again, with what they say then).
    pub(crate) fn resume_lazy_frames(&mut self, host: &Rc<FrameHost>) {
        if host.lazy_frames.borrow().is_empty() {
            return;
        }
        let pending: Vec<(FrameKey, Option<LazyFrameStamp>)> = host.lazy_frames.borrow().clone();
        for (key, checked) in pending {
            let Some(frame) = host.frame(key) else {
                host.lazy_frames.borrow_mut().retain(|(k, _)| *k != key);
                continue;
            };
            let Some(owner) = frame.owner else { continue };
            let Some((parent_loaded, Some(parent_dom))) =
                self.doc(host.parent_key(key)).map(|d| (d.main_frame.loaded, d.main_frame.page_dom.clone()))
            else {
                continue;
            };
            let (connected, lazy) = {
                let dom = host.dom.borrow();
                (dom.is_connected(owner).unwrap_or(false), is_lazy_iframe(&dom, owner))
            };
            if !connected {
                continue;
            }
            // Checked again only when the parent's DOM, style, viewport or scroll offsets changed.
            let stamp = (parent_dom.layout().observation_stamp(&parent_dom.dom.borrow()), parent_loaded, lazy);
            if checked == Some(stamp) {
                continue;
            }
            let go = !lazy || (parent_loaded && pyro_webapi::intersection_observer::lazy_load_intersects(&parent_dom, owner));
            if !go {
                let stamp = (parent_dom.layout().observation_stamp(&parent_dom.dom.borrow()), parent_loaded, lazy);
                if let Some(entry) = host.lazy_frames.borrow_mut().iter_mut().find(|(k, _)| *k == key) {
                    entry.1 = Some(stamp);
                }
                continue;
            }
            host.lazy_frames.borrow_mut().retain(|(k, _)| *k != key);
            host.lazy_approved.borrow_mut().push(key);
            // In a task, as the attribute processing it resumes.
            host.queue_attributes(key, frame.loader_id.clone());
        }
    }

    /// Loads `html`, the `srcdoc` of the iframe of `key`, as its document
    /// `about:srcdoc` (the initial navigation, and a navigation to
    /// `about:srcdoc` such as `location.reload()` of that document).
    pub(crate) fn begin_srcdoc(
        &mut self,
        key: FrameKey,
        frame_id: FrameId,
        html: String,
        referrer: String,
        history: HistoryHandling,
    ) -> Result<LoaderId, JsError> {
        let loader = LoaderId(format!("{:032X}", self.next()));
        let opts = NavigateOpts {
            post: None,
            url: "about:srcdoc".into(),
            reason: NavigationReason::InitialFrameNavigation,
            cdp_request_id: None,
            referrer: Some(referrer),
            referrer_policy: None,
        };
        self.emit(
            key.page,
            PageEvent::FrameNavigate {
                frame_id,
                loader_id: loader.clone(),
                url: opts.url.clone(),
                reason: opts.reason,
                cdp_request_id: None,
            },
        );
        self.begin_document(
            key,
            &opts,
            loader.clone(),
            Resource {
                url: opts.url.clone(),
                status: 200,
                mime: "text/html".into(),
                charset: None,
                body: html.into_bytes().into(),
                immutable_local: false,
                content_disposition: None,
                header_csp: None,
                refresh: None,
                content_language: None,
                last_modified: None,
                referrer_policy: crate::loader::parent_referrer_policy(self, key),
                origin_agent_cluster: false,
                sent_referrer: None,
                timing: None,
                partial: None,
                nosniff: false, opaque: false,
            },
            history,
        )?;
        Ok(loader)
    }

    /// HTML "evaluate a javascript: URL" whose result is a string: the string is the frame's
    /// new document, HTML in UTF-8 at the URL of the document it replaces, which it replaces in
    /// session history.
    pub(crate) fn begin_javascript_result(&mut self, key: FrameKey, html: String) -> Result<LoaderId, JsError> {
        let host = self.page(key.page).map(|p| p.frames.clone()).ok_or(JsError::ContextDestroyed)?;
        let frame = host.frame(key).ok_or(JsError::ContextDestroyed)?;
        let loader = LoaderId(format!("{:032X}", self.next()));
        let opts = NavigateOpts {
            post: None,
            url: frame.url.clone(),
            reason: NavigationReason::Script,
            cdp_request_id: None,
            referrer: None,
            referrer_policy: None,
        };
        self.emit(
            key.page,
            PageEvent::FrameNavigate {
                frame_id: frame.id.clone(),
                loader_id: loader.clone(),
                url: opts.url.clone(),
                reason: opts.reason,
                cdp_request_id: None,
            },
        );
        self.begin_document(
            key,
            &opts,
            loader.clone(),
            Resource {
                url: opts.url.clone(),
                status: 200,
                mime: "text/html".into(),
                charset: Some("utf-8".into()),
                body: html.into_bytes().into(),
                immutable_local: false,
                content_disposition: None,
                header_csp: None,
                refresh: None,
                content_language: None,
                last_modified: None,
                referrer_policy: crate::loader::parent_referrer_policy(self, key),
                origin_agent_cluster: false,
                sent_referrer: None,
                timing: None,
                partial: None,
                nosniff: false,
                opaque: false,
            },
            HistoryHandling::Replace,
        )?;
        Ok(loader)
    }

    /// The `srcdoc` of the connected iframe that holds the frame of `key`, if it has one.
    pub(crate) fn srcdoc_of(&self, key: FrameKey) -> Option<(FrameId, String)> {
        let host = self.page(key.page).map(|p| p.frames.clone())?;
        let frame = host.frame(key)?;
        let owner = frame.owner?;
        let dom = host.dom.borrow();
        if !dom.is_connected(owner).unwrap_or(false) {
            return None;
        }
        let html = dom.get_attribute(owner, "srcdoc").ok().flatten()?.to_owned();
        Some((frame.id, html))
    }
}

impl Browser {
    pub(crate) fn refresh_frame_settings(&self, page: PageId) {
        let Some(p) = self.page(page) else { return };
        let host = &p.frames;
        {
            let mut settings = host.settings.borrow_mut();
            settings.user_agent = p.user_agent_override.clone();
            settings.extra_headers = p.extra_http_headers.clone();
            settings.cache_disabled = p.cache_disabled;
            settings.bypass_service_worker = p.bypass_service_worker;
            settings.viewport = p.viewport.clone();
            settings.max_touch_points = p.max_touch_points;
        }
        let settings = host.settings.borrow().clone();
        let bindings: Vec<_> = host.children.borrow().values().filter_map(|d| d.main_frame.page_dom.clone()).collect();
        for dom in bindings {
            let _ = dom.set_user_agent_override(settings.user_agent.clone());
            dom.set_extra_http_headers(settings.extra_headers.clone());
            dom.set_cache_disabled(settings.cache_disabled);
            dom.set_bypass_service_worker(settings.bypass_service_worker);
            dom.set_max_touch_points(settings.max_touch_points);
        }
    }
}

/// Runs the script of a `javascript:` URL in `scope`: its result when that is a string (which
/// becomes the navigated frame's document), else None. An exception is reported.
pub(crate) fn javascript_url_result(scope: &pyro_js::Scope<'_>, script: &str) -> Option<String> {
    match scope.eval(script, None, 1) {
        Ok(value) if value.is_string() => value.to_string().ok(),
        Ok(_) => None,
        Err(e) => {
            scope.report_exception(&e);
            None
        }
    }
}

#[cfg(test)]
mod frame_id_tests {
    use super::child_of_frame_id;

    #[test]
    fn child_frame_ids_spell_their_keys() {
        for key in [0u64, 1, 42, 0xDEAD_BEEF, u64::MAX] {
            assert_eq!(child_of_frame_id(&format!("{key:032X}")), Some(key));
        }
        assert_eq!(child_of_frame_id(&format!("{:032x}", 0xABu64)), None, "lowercase is not an id we made");
        assert_eq!(child_of_frame_id("2A"), None);
        assert_eq!(child_of_frame_id(&format!("{:032X}", u128::from(u64::MAX) + 1)), None);
    }
}

/// An HTML `<iframe>` whose `loading` attribute is in the Lazy state.
pub(crate) fn is_lazy_iframe(dom: &pyro_dom::Dom, node: NodeId) -> bool {
    dom.element(node).is_ok_and(|e| {
        e.is_html() && &*e.name.local == "iframe" && e.attr("loading").is_some_and(|l| l.eq_ignore_ascii_case("lazy"))
    })
}
