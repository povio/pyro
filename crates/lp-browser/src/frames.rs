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
    pub session: lp_webapi::StorageNamespace,
    pub net: Option<lp_net::NetClient>,
    pub viewport: lp_dom::media::Viewport,
    pub user_agent: Option<lp_webapi::UserAgentOverride>,
    pub extra_headers: lp_net::http::HeaderMap,
    pub cache_disabled: bool,
    pub scripts: Vec<InitScript>,
}

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
}
impl FrameHost {
    pub fn new(browser: &Browser, page: PageId, context: ContextId, root: Frame, nav: Rc<PageNav>) -> Rc<Self> {
        let local = browser.contexts.iter().find(|c| c.id == context).expect("context").local_storage.clone();
        Rc::new(Self {
            children: RefCell::new(BTreeMap::new()),
            root: RefCell::new(root),
            root_nav: nav,
            dom: Rc::new(RefCell::new(lp_dom::Dom::new())),
            settings: RefCell::new(Settings {
                local,
                session: Default::default(),
                net: browser.context_net(context).cloned(),
                viewport: Default::default(),
                user_agent: None,
                extra_headers: Default::default(),
                cache_disabled: false,
                scripts: Vec::new(),
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
        })
    }
    fn next(&self) -> u64 {
        let id = self.next.get();
        self.next.set(id + 1);
        id
    }
    pub fn frame(&self, key: FrameKey) -> Option<Frame> {
        match key.child {
            None => Some(self.root.borrow().clone()),
            Some(id) => self.children.borrow().get(&id).map(|d| d.main_frame.clone()),
        }
    }
    pub fn key(&self, id: &FrameId) -> Option<FrameKey> {
        if self.root.borrow().id == *id {
            return Some(self.page.into());
        }
        self.children
            .borrow()
            .iter()
            .find(|(_, d)| d.main_frame.id == *id)
            .map(|(id, _)| FrameKey { page: self.page, child: Some(*id) })
    }
    pub fn keys(&self) -> Vec<FrameKey> {
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
        self.frame(key).is_some_and(|f| f.loader_id == *loader)
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
        let realm = Engine::create_realm_in_current_thread(RealmOptions {
            name: world.clone().unwrap_or_default(),
            global_class: Some(class),
            origin: Some(frame.origin.clone()),
        })?;
        realm.set_global_data::<Window>(WindowData::for_frame(
            self.sched.clone(),
            nav,
            &dom,
            Rc::downgrade(self),
            key,
        ))?;
        lp_webapi::install(&realm.scope()?, dom)?;
        window::init_frame_indices(&realm)?;
        loader::install_module_fetcher(self.sched.clone(), key, &realm);
        let entry = FrameRealm { world: world.clone(), realm: realm.clone() };
        match key.child {
            None => self.root.borrow_mut().realms.push(entry),
            Some(id) => {
                self.children.borrow_mut().get_mut(&id).ok_or(JsError::ContextDestroyed)?.main_frame.realms.push(entry)
            }
        }
        window::refresh_frame_indices(&realm, self, key)?;
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
                load: None,
                stylesheets: Default::default(),
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

#[derive(Clone, Copy, Default)]
pub(crate) struct Sandbox {
    pub opaque: bool,
    pub scripts_disabled: bool,
}
impl Sandbox {
    pub fn parse(value: Option<&str>) -> Self {
        let Some(value) = value else { return Self::default() };
        let tokens: Vec<_> = value.split_ascii_whitespace().collect();
        Self {
            opaque: !tokens.iter().any(|t| t.eq_ignore_ascii_case("allow-same-origin")),
            scripts_disabled: !tokens.iter().any(|t| t.eq_ignore_ascii_case("allow-scripts")),
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
    fn parent_key(&self, key: FrameKey) -> FrameKey {
        self.frame(key).and_then(|f| f.parent).and_then(|id| self.key(&id)).unwrap_or(key)
    }
    pub fn child_keys(&self, key: FrameKey) -> Vec<FrameKey> {
        let Some(frame) = self.frame(key) else { return Vec::new() };
        self.children
            .borrow()
            .iter()
            .filter(|(_, d)| d.main_frame.parent.as_ref() == Some(&frame.id))
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
        let data = CrossWindowData { host: Rc::downgrade(self), source, target };
        Ok(s.wrap::<CrossWindow>(data, Some(lp_js::IdentityKey(0x4652414d45505258, target.child.unwrap_or(0))))?
            .value())
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

pub(crate) struct CrossWindow;
pub(crate) struct CrossWindowData {
    host: std::rc::Weak<FrameHost>,
    source: FrameKey,
    target: FrameKey,
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
            let value = match name {
                "window" | "self" | "frames" => host.window_value(s, w.source, w.target)?,
                "parent" => host.window_value(s, w.source, host.parent_key(w.target))?,
                "top" => host.window_value(s, w.source, host.page.into())?,
                "length" => s.number(host.child_keys(w.target).len() as f64),
                "closed" => s.boolean(host.frame(w.target).is_none()),
                "close" | "focus" | "blur" => s.function(name, |s, _, _| Ok(s.undefined()))?.value(),
                _ => return Err(JsError::dom("SecurityError", "Blocked cross-origin frame access")),
            };
            Ok(Some(value))
        });
        def.named_setter(|_, _, _, _| Err(JsError::dom("SecurityError", "Blocked cross-origin frame write")));
        def.named_deleter(|_, _, _| Err(JsError::dom("SecurityError", "Blocked cross-origin frame delete")));
    }
}

impl Browser {
    pub(crate) fn sync_root(&self, page: PageId) {
        if let Some(p) = self.page(page) {
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
            .filter_map(|key| self.doc(key).map(|d| d.main_frame.clone()))
            .map(|f| FrameInfo {
                id: f.id,
                parent: f.parent,
                owner: f.owner,
                url: f.url,
                loader_id: f.loader_id,
                name: f.name,
                origin: f.origin,
                network_idle: f.network_idle,
                dom_content_loaded: f.dom_content_loaded,
                loaded: f.loaded,
            })
            .collect()
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
        host.owner(owner).and_then(|key| host.frame(key)).map(|f| f.id)
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
        let (srcdoc, src) = {
            let dom = host.dom.borrow();
            if !dom.is_connected(owner).unwrap_or(false) {
                return;
            }
            (
                dom.get_attribute(owner, "srcdoc").ok().flatten().map(str::to_owned),
                dom.get_attribute(owner, "src").ok().flatten().map(str::to_owned),
            )
        };
        let result = if let Some(html) = srcdoc {
            let loader = LoaderId(format!("{:032X}", self.next()));
            let opts = NavigateOpts {
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
                },
                HistoryHandling::Replace,
            )
        } else {
            let raw = src.unwrap_or_default();
            let url = if raw.trim().is_empty() {
                "about:blank".to_string()
            } else {
                url::Url::parse(&parent.base_url)
                    .and_then(|u| u.join(&raw))
                    .map(|u| u.to_string())
                    .unwrap_or_else(|_| raw.clone())
            };
            if url == "about:blank" && frame.url == "about:blank" && !frame.dom_content_loaded {
                self.finish_blank_document(key);
                Ok(())
            } else {
                self.navigate_frame_with_handling(
                    key,
                    NavigateOpts {
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
        }
        let settings = host.settings.borrow().clone();
        let bindings: Vec<_> = host.children.borrow().values().filter_map(|d| d.main_frame.page_dom.clone()).collect();
        for dom in bindings {
            let _ = dom.set_user_agent_override(settings.user_agent.clone());
            dom.set_extra_http_headers(settings.extra_headers.clone());
            dom.set_cache_disabled(settings.cache_disabled);
        }
    }
}
