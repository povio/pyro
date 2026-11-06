//! Per-page frame registry. No registry borrow may cross JavaScript execution.
use crate::*;
use lp_dom::NodeId;
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
}

#[derive(Clone)]
pub(crate) struct Settings {
    pub local: lp_webapi::StorageNamespace,
    pub services: Rc<lp_webapi::ContextServices>,
    pub max_touch_points: u32,
    pub session: lp_webapi::StorageNamespace,
    pub net: Option<lp_net::NetClient>,
    pub viewport: lp_dom::media::Viewport,
    pub user_agent: Option<lp_webapi::UserAgentOverride>,
    pub extra_headers: lp_net::http::HeaderMap,
    pub cache_disabled: bool,
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
pub(crate) fn prepare_spare_realm(class: lp_js::ClassId) -> Result<(), JsError> {
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
pub fn realm_page(scope: &lp_js::Scope<'_>) -> Option<PageId> {
    scope.global_data::<Window>()?.frame_key().map(|key| key.page)
}

/// Frame keys of one page without a heap allocation for up to four frames.
pub type FrameKeys = smallvec::SmallVec<[FrameKey; 4]>;

pub(crate) struct FrameHost {
    pub children: RefCell<BTreeMap<u64, DocumentState>>,
    pub root: RefCell<Frame>,
    root_nav: Rc<PageNav>,
    pub dom: lp_dom::parse::DomRef,
    pub settings: RefCell<Settings>,
    pub page: PageId,
    pub context: ContextId,
    pub sched: Scheduler<Browser>,
    pub handle: LoopHandle<Browser>,
    pub observer: ObserverCell,
    pub next: Rc<Cell<u64>>,
    pub started: Instant,
    retired: RefCell<Vec<DocumentState>>,
    class: Cell<lp_js::ClassId>,
    pub(crate) focus: RefCell<crate::focus::FocusState>,
    /// The page that opened this one as a popup with its opener kept
    /// (`Browser::adopt_popup`): the top-level window's `opener`.
    pub(crate) opener: RefCell<Option<std::rc::Weak<FrameHost>>>,
    /// For such a popup, the ticket through which the opener's document
    /// reaches it (its `window.open` result is `event.source` there).
    pub(crate) popup: RefCell<Option<Rc<lp_webapi::PopupTicket>>>,
}
impl FrameHost {
    pub fn new(browser: &Browser, page: PageId, context: ContextId, root: Frame, nav: Rc<PageNav>) -> Rc<Self> {
        let state = browser.contexts.iter().find(|c| c.id == context).expect("context");
        let (local, services) = (state.local_storage.clone(), state.services.clone());
        Rc::new(Self {
            children: RefCell::new(BTreeMap::new()),
            root: RefCell::new(root),
            root_nav: nav,
            dom: Rc::new(RefCell::new(lp_dom::Dom::new())),
            settings: RefCell::new(Settings {
                local,
                services,
                max_touch_points: 0,
                session: Default::default(),
                net: browser.context_net(context).cloned(),
                viewport: Default::default(),
                user_agent: None,
                extra_headers: Default::default(),
                cache_disabled: false,
                scripts: Vec::new(),
                bindings: Vec::new(),
            }),
            page,
            context,
            sched: browser.sched.clone(),
            handle: browser.handle.clone(),
            observer: browser.observer.clone(),
            next: browser.next_id.clone(),
            started: browser.started,
            retired: RefCell::new(Vec::new()),
            class: Cell::new(browser.engine.register::<Window>()),
            opener: RefCell::new(None),
            popup: RefCell::new(None),
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
    pub fn page_dom(&self, key: FrameKey) -> Option<Rc<lp_webapi::PageDom>> {
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
        // A main-world realm prepared while this thread was idle (see
        // `prepare_spare_realm`) only needs its origin; otherwise build one.
        let spare = if world.is_none() { SPARE_REALM.with(|s| s.borrow_mut().take()) } else { None };
        let realm = match spare.filter(|r| !r.is_disposed()) {
            Some(realm) => {
                realm.set_origin(Some(frame.origin.clone()));
                realm
            }
            None => Engine::create_realm_in_current_thread(RealmOptions {
                name: world.clone().unwrap_or_default(),
                global_class: Some(class),
                origin: Some(frame.origin.clone()),
            })?,
        };
        realm.set_global_data::<Window>(WindowData::for_frame(
            self.sched.clone(),
            nav,
            &dom,
            Rc::downgrade(self),
            key,
        ))?;
        {
            // Engine code only, up to the bindings and init scripts below.
            let _setup = realm.setup_without_watchdog();
            lp_webapi::install(&realm.scope()?, dom)?;
            // Uncaught exceptions of the realm's callbacks fire `error` at the Window.
            realm.set_exception_reporter(Some(lp_webapi::lifecycle::report_exception));
            if crate::clock::page_clock_enabled() {
                realm.scope()?.eval(crate::clock::DATE_SCRIPT, Some("lightningpanda://internal/clock.js"), 1)?;
            }
            window::init_frame_indices(&realm)?;
        }
        loader::install_module_fetcher(self.sched.clone(), key, &realm);
        let entry = FrameRealm { world: world.clone(), realm: realm.clone() };
        match key.child {
            None => self.root.borrow_mut().realms.push(entry),
            Some(id) => {
                self.children.borrow_mut().get_mut(&id).ok_or(JsError::ContextDestroyed)?.main_frame.realms.push(entry)
            }
        }
        window::refresh_frame_indices(&realm, self, key)?;
        let bindings = {
            let settings = self.settings.borrow();
            match settings.bindings.is_empty() {
                true => Vec::new(),
                false => settings.bindings.clone(),
            }
        };
        for binding in bindings.iter().filter(|b| b.applies_to(world.as_deref())) {
            let scope = realm.scope()?;
            if let Err(e) = scope.install_binding(&binding.name) {
                scope.report_exception(&e);
            }
        }
        let scripts = self.settings.borrow().scripts.clone();
        for script in scripts.iter().filter(|s| s.world == world) {
            let scope = realm.scope()?;
            if let Err(e) = scope.eval(&script.source, Some(&format!("init-script:{}", script.id)), 1) {
                scope.report_exception(&e);
            }
        }
        self.emit(PageEvent::RealmCreated { frame_id: frame.id, realm: realm.id(), world, origin: frame.origin });
        Ok(realm)
    }
    fn window_class(&self) -> lp_js::ClassId {
        self.class.get()
    }

    fn refresh_indices(self: &Rc<Self>, key: FrameKey) {
        let realms = self.frame(key).map(|f| f.realms).unwrap_or_default();
        for world in realms {
            if let Err(e) = window::refresh_frame_indices(&world.realm, self, key) {
                if let Ok(scope) = world.realm.scope() {
                    scope.report_exception(&e);
                }
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
            (
                dom.get_attribute(node, "name").ok().flatten().unwrap_or("").to_owned(),
                Sandbox::parse(dom.get_attribute(node, "sandbox").ok().flatten()),
            )
        };
        sandbox.opaque |= ancestor.sandbox.opaque;
        sandbox.scripts_disabled |= ancestor.sandbox.scripts_disabled;
        sandbox.popups_blocked |= ancestor.sandbox.popups_blocked;
        if self.children.borrow().len() >= 128 {
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
        let nav = PageNav::new(self.page, fid.clone(), self.sched.clone(), self.observer.clone());
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
        };
        self.dom.borrow_mut().add_ref(node).map_err(|e| JsError::Type(format!("{e:?}")))?;
        self.children.borrow_mut().insert(
            id,
            DocumentState {
                main_frame: frame,
                nav: nav.clone(),
                pending_loader: None,
                pending_cancel: None,
                pending_script: None,
                load: None,
                stylesheets: Default::default(),
                images: Default::default(),
                preloads: Default::default(),
                refresh: Default::default(),
                _owner_pin: Some(loader::ResourceNode { dom: self.dom.clone(), node }),
            },
        );
        self.refresh_indices(parent);
        self.emit(PageEvent::FrameChildFrameCreated { parent: ancestor.id, frame_id: fid.clone() });
        let result = (|| {
            let dom = bindings::create(self, key, "about:blank", &lid)?;
            nav.commit_document("about:blank", HistoryHandling::Replace, Some((self.dom.clone(), dom.document())));
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
            });
            self.queue_attributes(key, lid);
            Ok(())
        })();
        if result.is_err() {
            self.detach(key);
        }
        result
    }
    pub fn queue_attributes(&self, key: FrameKey, loader: LoaderId) {
        self.sched.post(lp_runtime::TaskSource::Navigation, move |b| b.process_iframe_attributes(key, &loader));
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
    pub fn drain_retired(&self) {
        let retired = std::mem::take(&mut *self.retired.borrow_mut());
        for state in retired {
            let frame = state.main_frame;
            if let Some(dom) = &frame.page_dom {
                dom.dispose();
            }
            for world in frame.realms {
                window::cancel_timers(&world.realm);
                if let Some(dom) = &frame.page_dom {
                    lp_webapi::lifecycle::realm_disposed(dom, world.realm.id());
                }
                world.realm.dispose();
                self.emit(PageEvent::RealmDestroyed { frame_id: frame.id.clone(), realm: world.realm.id() });
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

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Sandbox {
    pub opaque: bool,
    pub scripts_disabled: bool,
    pub popups_blocked: bool,
}
impl Sandbox {
    pub fn parse(value: Option<&str>) -> Self {
        let Some(value) = value else { return Self::default() };
        let tokens: Vec<_> = value.split_ascii_whitespace().collect();
        Self {
            opaque: !tokens.iter().any(|t| t.eq_ignore_ascii_case("allow-same-origin")),
            scripts_disabled: !tokens.iter().any(|t| t.eq_ignore_ascii_case("allow-scripts")),
            popups_blocked: !tokens.iter().any(|t| t.eq_ignore_ascii_case("allow-popups")),
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
    fn same_origin(&self, a: FrameKey, b: FrameKey) -> bool {
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
    fn world_of(&self, source: FrameKey, realm: lp_js::RealmId) -> Result<Option<String>, JsError> {
        self.frame(source)
            .and_then(|f| f.realms.into_iter().find(|r| r.realm.id() == realm))
            .map(|r| r.world)
            .ok_or(JsError::ContextDestroyed)
    }
    /// Realms of `realm`'s world in the frames of this tree other than `source`.
    /// A removed frame's realms count until they are disposed (`drain_retired`):
    /// its script may still run, and the nodes it wrapped keep those wrappers.
    pub fn sibling_realms(&self, source: FrameKey, realm: lp_js::RealmId) -> Vec<Realm> {
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
        let of = |frame: &Frame| frame.realms.iter().find(|r| r.world == world).map(|r| r.realm.clone());
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
        realms
    }
    pub fn document_value<'s>(
        self: &Rc<Self>,
        s: &lp_js::Scope<'s>,
        source: FrameKey,
        target: FrameKey,
    ) -> lp_js::JsResult<lp_js::Value<'s>> {
        if !self.same_origin(source, target) {
            return Ok(s.null());
        }
        let name = self.world_of(source, s.realm().id())?;
        let realm = self.realm(target, name.as_deref())?;
        let scope = realm.scope()?;
        let doc = scope.global().get("document")?;
        let value = lp_js::Persistent::new(&scope, doc);
        Ok(value.get(s))
    }
    pub fn window_value<'s>(
        self: &Rc<Self>,
        s: &lp_js::Scope<'s>,
        source: FrameKey,
        target: FrameKey,
    ) -> lp_js::JsResult<lp_js::Value<'s>> {
        if source == target {
            return Ok(s.global().value());
        }
        if self.same_origin(source, target) {
            let name = self.world_of(source, s.realm().id())?;
            let realm = self.realm(target, name.as_deref())?;
            let scope = realm.scope()?;
            let value = lp_js::Persistent::new(&scope, scope.global().value());
            return Ok(value.get(s));
        }
        let data = CrossWindowData { host: Rc::downgrade(self), source: Some(source), from: None, target };
        Ok(s.wrap::<CrossWindow>(data, Some(lp_js::IdentityKey(0x4652414d45505258, target.child.unwrap_or(0))))?
            .value())
    }
    /// `window.opener` of frame `key`: the opener page's top-level window
    /// for the top-level frame of a popup that kept its opener (null for
    /// child frames, other pages, and a closed opener). Same-origin openers
    /// are reached directly; otherwise a cross-origin stand-in.
    pub fn opener_window<'s>(
        self: &Rc<Self>,
        s: &lp_js::Scope<'s>,
        key: FrameKey,
    ) -> lp_js::JsResult<lp_js::Value<'s>> {
        if key.child.is_some() {
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
            let value = lp_js::Persistent::new(&scope, scope.global().value());
            return Ok(value.get(s));
        }
        opener_proxy(s, &opener, Some((Rc::downgrade(self), key)))
    }
    pub fn relation<'s>(
        self: &Rc<Self>,
        s: &lp_js::Scope<'s>,
        key: FrameKey,
        top: bool,
    ) -> lp_js::JsResult<lp_js::Value<'s>> {
        self.window_value(s, key, if top { self.page.into() } else { self.parent_key(key) })
    }
    pub fn frame_element<'s>(
        self: &Rc<Self>,
        s: &lp_js::Scope<'s>,
        key: FrameKey,
    ) -> lp_js::JsResult<lp_js::Value<'s>> {
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
        let node = lp_webapi::node::wrap_node(&scope, &dom, owner)?;
        Ok(lp_js::Persistent::new(&scope, node.value()).get(s))
    }
}

/// The cross-origin stand-in for `opener`'s top-level window (one per
/// realm), as frame `from` (the popup's top-level frame) sees it.
fn opener_proxy<'s>(
    s: &lp_js::Scope<'s>,
    opener: &Rc<FrameHost>,
    from: Option<(std::rc::Weak<FrameHost>, FrameKey)>,
) -> lp_js::JsResult<lp_js::Value<'s>> {
    let key = lp_js::IdentityKey(0x4f50454e45525758, u64::from(opener.page.0));
    if let Some(o) = s.lookup_identity(key) {
        return Ok(o.value());
    }
    let data = CrossWindowData { host: Rc::downgrade(opener), source: None, from, target: opener.page.into() };
    Ok(s.wrap::<CrossWindow>(data, Some(key))?.value())
}

/// The window whose script is running (HTML's incumbent global, as
/// [`lp_js::entry_realm`] approximates it): its frame host and key.
pub(crate) fn incumbent_window() -> Option<(Rc<FrameHost>, FrameKey)> {
    let realm = lp_js::entry_realm()?;
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
) -> lp_webapi::event::MessageSource {
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
    s: &lp_js::Scope<'s>,
    a: &lp_js::Args<'s>,
    source: Option<(Rc<FrameHost>, FrameKey)>,
    host: &Rc<FrameHost>,
    target: FrameKey,
) -> lp_js::JsResult<()> {
    let (source_host, source) = source.unwrap_or_else(|| (host.clone(), target));
    let incumbent = source_host.with_frame(source, |f| f.origin.clone()).unwrap_or_else(|| "null".into());
    let (message, target_origin, options) = lp_webapi::window_message::post_message_args(s, a, &incumbent)?;
    let found = host.with_frame(target, |f| Some((f.page_dom.clone()?, f.main_realm()?.clone(), f.origin.clone())));
    // A window whose document is gone receives nothing.
    let Some((page, realm, origin)) = found.flatten() else { return Ok(()) };
    let deliver = target_origin.allows(&origin);
    let window = (!Rc::ptr_eq(&source_host, host) || source != target)
        .then(|| message_source(host, target, &source_host, source));
    lp_webapi::window_message::send_window_message(message, options, &page, &realm, incumbent, window, deliver)
}

pub(crate) struct CrossWindow;
pub(crate) struct CrossWindowData {
    host: std::rc::Weak<FrameHost>,
    /// The accessing frame, when it is in `host`'s page; `None` for a
    /// popup's view of its opener (another page), which is always
    /// cross-origin and whose target is its own top and parent.
    source: Option<FrameKey>,
    /// For a popup's view of its opener: the popup's frame host and frame.
    from: Option<(std::rc::Weak<FrameHost>, FrameKey)>,
    target: FrameKey,
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
    fn navigate(&self, value: lp_js::Value<'_>, replace: bool) -> lp_js::JsResult<()> {
        let nav = self.host.upgrade().and_then(|h| h.nav(self.target)).ok_or(JsError::ContextDestroyed)?;
        crate::nav::navigate_cross_origin(&nav, &self.base, value, replace)
    }
}
impl lp_js::JsClass for CrossLocation {
    const NAME: &'static str = "CrossOriginLocation";
    type Data = CrossLocationData;
    type Parent = lp_js::NoParent;
    fn define(def: &mut lp_js::ClassDef<Self>) {
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
    s: &lp_js::Scope<'s>,
    w: &CrossWindowData,
    host: &Rc<FrameHost>,
) -> lp_js::JsResult<lp_js::Value<'s>> {
    let base = w
        .accessor(host)
        .and_then(|(h, key)| h.with_frame(key, |f| f.base_url.clone()))
        .unwrap_or_else(|| "about:blank".into());
    let data = CrossLocationData { host: Rc::downgrade(host), target: w.target, base };
    Ok(s.wrap::<CrossLocation>(data, None)?.value())
}
impl lp_js::JsClass for CrossWindow {
    const NAME: &'static str = "CrossOriginWindow";
    type Data = CrossWindowData;
    type Parent = lp_js::NoParent;
    fn define(def: &mut lp_js::ClassDef<Self>) {
        def.not_exposed();
        def.named(true, |s, w, name| {
            let Some(host) = w.host.upgrade() else {
                return if name == "closed" { Ok(Some(s.boolean(true))) } else { Err(JsError::ContextDestroyed) };
            };
            let value = match (name, w.source) {
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
                            lp_webapi::input::focus_window(&page);
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
            let (Some(host), Some(source)) = (w.host.upgrade(), w.source) else { return Ok(None) };
            let child = host.child_keys(w.target).get(index as usize).copied();
            child.map(|child| host.window_value(s, source, child)).transpose()
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
        document: &Rc<lp_webapi::PageDom>,
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
            lp_webapi::lifecycle::dom_content_loaded(&dom, dom.document());
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
        let Some(frame) = host.frame(key).filter(|f| &f.loader_id == loader) else { return };
        let Some(owner) = frame.owner else { return };
        let Some(parent) = frame.parent.as_ref().and_then(|id| host.key(id)).and_then(|key| host.frame(key)) else {
            return;
        };
        let (srcdoc, src, src_base) = {
            let dom = host.dom.borrow();
            if !dom.is_connected(owner).unwrap_or(false) {
                return;
            }
            (
                dom.get_attribute(owner, "srcdoc").ok().flatten().map(str::to_owned),
                dom.get_attribute(owner, "src").ok().flatten().map(str::to_owned),
                // `<base href>` aware.
                crate::loader::node_base_url(&dom, owner),
            )
        };
        let result = if let Some(html) = srcdoc {
            let loader = LoaderId(format!("{:032X}", self.next()));
            let opts = NavigateOpts {
                post: None,
                url: "about:srcdoc".into(),
                reason: NavigationReason::InitialFrameNavigation,
                cdp_request_id: None,
                referrer: Some(parent.url),
            };
            self.emit(
                key.page,
                PageEvent::FrameNavigate {
                    frame_id: frame.id,
                    loader_id: loader.clone(),
                    url: opts.url.clone(),
                    reason: opts.reason,
                    cdp_request_id: None,
                },
            );
            self.begin_document(
                key,
                &opts,
                loader,
                Resource {
                    url: opts.url.clone(),
                    status: 200,
                    mime: "text/html".into(),
                    charset: None,
                    body: html.into_bytes(),
                    content_disposition: None,
                    header_csp: false,
                    refresh: None,
                    referrer_policy: None,
                },
                HistoryHandling::Replace,
            )
        } else {
            let raw = src.unwrap_or_default();
            let url = if raw.trim().is_empty() {
                "about:blank".to_string()
            } else {
                url::Url::parse(src_base.as_deref().unwrap_or(&parent.base_url))
                    .and_then(|u| u.join(&raw))
                    .map(|u| u.to_string())
                    .unwrap_or_else(|_| raw.clone())
            };
            if url == "about:blank" && frame.url == "about:blank" && !frame.dom_content_loaded {
                // A script that opened the initial document since (append,
                // then `contentDocument.open()`) finishes it with `close()`.
                if host.nav(key).map_or(true, |nav| nav.is_initial_blank()) {
                    self.finish_blank_document(key);
                }
                Ok(())
            } else {
                self.navigate_frame_with_handling(
                    key,
                    NavigateOpts {
                        post: None,
                        url,
                        reason: NavigationReason::InitialFrameNavigation,
                        cdp_request_id: None,
                        referrer: Some(parent.url),
                    },
                    HistoryHandling::Replace,
                )
                .map(|_| ())
            }
        };
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
    pub(crate) fn refresh_frame_settings(&self, page: PageId) {
        let Some(p) = self.page(page) else { return };
        let host = &p.frames;
        {
            let mut settings = host.settings.borrow_mut();
            settings.user_agent = p.user_agent_override.clone();
            settings.extra_headers = p.extra_http_headers.clone();
            settings.cache_disabled = p.cache_disabled;
            settings.viewport = p.viewport.clone();
            settings.max_touch_points = p.max_touch_points;
        }
        let settings = host.settings.borrow().clone();
        let bindings: Vec<_> = host.children.borrow().values().filter_map(|d| d.main_frame.page_dom.clone()).collect();
        for dom in bindings {
            let _ = dom.set_user_agent_override(settings.user_agent.clone());
            dom.set_extra_http_headers(settings.extra_headers.clone());
            dom.set_cache_disabled(settings.cache_disabled);
            dom.set_max_touch_points(settings.max_touch_points);
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
