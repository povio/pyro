//! Per-page binding state shared by every realm (main world + isolated
//! worlds) that exposes the page's DOM.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use lp_dom::events::TargetKey;
use lp_dom::parse::DomRef;
use lp_dom::select::SelectorCache;
use lp_dom::{Dom, NodeId};
use lp_js::{IdentityKey, Persistent, RealmId, WeakRealm};
use rustc_hash::FxHashMap;

static NEXT_PAGE: AtomicU64 = AtomicU64::new(1);

/// High bit marks identity keys of non-node event targets / events.
pub(crate) const OTHER_NS: u64 = 1 << 63;
pub(crate) const EVENT_NS: u64 = 1 << 62;

pub struct PageDom {
    pub dom: DomRef,
    id: u64,
    disposed: Cell<bool>,
    document: Cell<NodeId>,
    pub(crate) selectors: RefCell<SelectorCache>,
    /// Node references released by finalized wrappers while the DOM was
    /// borrowed; applied at the next safe point.
    pending_release: RefCell<Vec<NodeId>>,
    current_script: Cell<Option<NodeId>>,
    pub(crate) realms: RefCell<FxHashMap<RealmId, WeakRealm>>,
    pub(crate) url_params_helpers: RefCell<FxHashMap<RealmId, Persistent>>,
    pub(crate) header_helpers: RefCell<FxHashMap<RealmId, Persistent>>,
    pub(crate) data_view_readers: RefCell<FxHashMap<RealmId, Persistent>>,
    /// Wrappers kept alive because they hold listeners for a connected
    /// node (or an active non-node target): (realm, target) -> wrapper.
    pub(crate) pinned: RefCell<FxHashMap<(RealmId, TargetKey), Persistent>>,
    /// Window event-target keys (document -> window parent in event paths).
    /// (document, Window event-target key) pairs.
    pub(crate) window_keys: RefCell<Vec<(NodeId, TargetKey)>>,
    /// Per-realm plain object holding Window listeners (page realms use the
    /// default JSC global, which cannot carry private properties).
    pub(crate) window_holders: RefCell<FxHashMap<RealmId, Persistent>>,
    /// Per-realm private holder (null-prototype object, next free index)
    /// keeping Window [SameObject] wrappers (navigator, crypto, ...) alive:
    /// the identity map is weak, and script expandos / defineProperty
    /// overrides on them must survive collection.
    pub(crate) same_objects: RefCell<FxHashMap<RealmId, (Persistent, u32)>>,
    pub(crate) last_removals: Cell<u64>,
    next_event: Cell<u64>,
    pub net: RefCell<Option<lp_net::NetClient>>,
    pub(crate) intersections: crate::intersection_observer::BindingState,
    pub(crate) resizes: crate::resize_observer::BindingState,
    pub(crate) mutations: crate::mutation_observer::BindingState,
    pub(crate) identity: RefCell<crate::identity::IdentityState>,
    pub(crate) fetch_state: crate::fetch::FetchState,
    pub(crate) xhr_state: crate::xhr::XhrState,
    pub(crate) local_storage: RefCell<crate::storage::StorageNamespace>,
    pub(crate) session_storage: RefCell<crate::storage::StorageNamespace>,
    pub(crate) opaque_storage: crate::storage::OpaqueStorage,
    /// `URL.createObjectURL` store: blob URL -> (bytes, type). Owned by the
    /// creating document; revoked entries and disposal drop the bytes.
    pub(crate) blob_urls: RefCell<FxHashMap<String, (bytes::Bytes, String)>>,
    hooks: RefCell<Option<Rc<dyn PageHooks>>>,
    ready: RefCell<FxHashMap<NodeId, &'static str>>,
    focus_values: RefCell<FxHashMap<NodeId, String>>,
    clicking: RefCell<Vec<NodeId>>,
    submitting: RefCell<Vec<NodeId>>,
    hovered: Cell<Option<NodeId>>,
    press_target: Cell<Option<NodeId>>,
    /// Control activated by a Space keydown, clicked on the matching keyup.
    pub(crate) space_target: Cell<Option<NodeId>>,
    dialog_returns: RefCell<FxHashMap<NodeId, String>>,
    sheet_rules: RefCell<FxHashMap<NodeId, Vec<String>>>,
    /// Files selected into `<input type=file>` (CDP DOM.setFileInputFiles).
    pub(crate) files: RefCell<FxHashMap<NodeId, Vec<crate::file::FileEntry>>>,
    /// Faux layout + style engine of the displayed document.
    pub layout: RefCell<lp_dom::layout::Layout>,
}

/// A navigation requested by the document (link activation, form
/// submission, `location` assignment is lp-browser's own).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NavigationRequest {
    pub url: String,
    /// Browsing-context name (`target` attribute); None = same frame.
    pub target: Option<String>,
    pub method: String,
    pub body: Option<Vec<u8>>,
    pub content_type: Option<String>,
}

/// Posts a `Send` closure to run on the browser thread as a separate task.
pub type RemotePoster = std::sync::Arc<dyn Fn(Box<dyn FnOnce() + Send>) + Send + Sync>;

/// Embedder (lp-browser) services the bindings need.
pub trait PageHooks {
    /// Schedules `task` on the page's event loop as a separate task.
    fn post_task(&self, task: Box<dyn FnOnce()>);
    /// Starts a navigation of the frame showing the document.
    fn navigate(&self, request: NavigationRequest);
    /// Thread-safe poster that runs a closure as a task on the page's
    /// browser thread (lp-net sinks run on the I/O thread). `None` when the
    /// embedder cannot post across threads (network APIs then fail).
    fn remote_poster(&self) -> Option<RemotePoster> {
        None
    }
    /// Document of the child browsing context of `iframe`, if any.
    fn frame_document(&self, _iframe: NodeId) -> Option<NodeId> {
        None
    }
    /// WindowProxy of the child browsing context in `scope`'s realm.
    fn frame_window<'s>(&self, _scope: &lp_js::Scope<'s>, _iframe: NodeId) -> Option<lp_js::Value<'s>> {
        None
    }
}

impl PageDom {
    pub fn new(dom: DomRef, document: NodeId) -> Rc<PageDom> {
        Rc::new(PageDom {
            dom,
            id: NEXT_PAGE.fetch_add(1, Ordering::Relaxed),
            disposed: Cell::new(false),
            document: Cell::new(document),
            selectors: RefCell::new(SelectorCache::default()),
            pending_release: RefCell::new(Vec::new()),
            current_script: Cell::new(None),
            realms: RefCell::new(FxHashMap::default()),
            data_view_readers: RefCell::new(FxHashMap::default()),
            header_helpers: RefCell::new(FxHashMap::default()),
            url_params_helpers: RefCell::new(FxHashMap::default()),
            pinned: RefCell::new(FxHashMap::default()),
            window_keys: RefCell::new(Vec::new()),
            window_holders: RefCell::new(FxHashMap::default()),
            same_objects: RefCell::new(FxHashMap::default()),
            last_removals: Cell::new(u64::MAX),
            next_event: Cell::new(1),
            net: RefCell::new(None),
            identity: RefCell::new(Default::default()),
            mutations: Default::default(),
            intersections: Default::default(),
            resizes: Default::default(),
            fetch_state: Default::default(),
            xhr_state: Default::default(),
            local_storage: RefCell::new(Default::default()),
            session_storage: RefCell::new(Default::default()),
            opaque_storage: Default::default(),
            blob_urls: RefCell::new(FxHashMap::default()),
            hooks: RefCell::new(None),
            ready: RefCell::new(FxHashMap::default()),
            focus_values: RefCell::new(FxHashMap::default()),
            clicking: RefCell::new(Vec::new()),
            submitting: RefCell::new(Vec::new()),
            hovered: Cell::new(None),
            press_target: Cell::new(None),
            space_target: Cell::new(None),
            dialog_returns: RefCell::new(FxHashMap::default()),
            sheet_rules: RefCell::new(FxHashMap::default()),
            files: RefCell::new(FxHashMap::default()),
            layout: RefCell::new(lp_dom::layout::Layout::new(document)),
        })
    }

    /// Set before installing realms. Share local storage per browser context
    /// and session storage per top-level page across document replacements.
    pub fn set_storage_namespaces(
        &self,
        local: crate::storage::StorageNamespace,
        session: crate::storage::StorageNamespace,
    ) {
        *self.local_storage.borrow_mut() = local;
        *self.session_storage.borrow_mut() = session;
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    /// Bytes and type registered for a live `URL.createObjectURL` URL.
    pub fn blob_url(&self, url: &str) -> Option<(bytes::Bytes, String)> {
        self.blob_urls.borrow().get(url).cloned()
    }
    pub fn is_disposed(&self) -> bool {
        self.disposed.get()
    }

    /// Release document-owned binding state before disposing its realms.
    /// Call once on navigation/page teardown, on the browser thread at a safe
    /// point with no DOM borrow. Idempotent; a disposed PageDom cannot be reused.
    /// Shared context networking/storage handles remain valid for other pages.
    pub fn dispose(&self) {
        if self.disposed.replace(true) {
            return;
        }
        crate::fetch::dispose(self);
        crate::xhr::dispose(self);
        crate::mutation_observer::dispose(self);
        crate::intersection_observer::dispose(self);
        crate::resize_observer::dispose(self);
        self.realms.borrow_mut().clear();
        // Drop roots and embedder hooks outside RefCell borrows: destructors
        // can release wrappers or other resources owned by this page.
        drop(self.hooks.take());
        drop(self.net.take());
        drop(self.pinned.take());
        drop(self.window_holders.take());
        drop(self.same_objects.take());
        drop(self.data_view_readers.take());
        drop(self.header_helpers.take());
        drop(self.url_params_helpers.take());
        self.opaque_storage.clear();
        drop(self.blob_urls.take());
        crate::custom::forget_page(self.id);
        let windows = self.window_keys.take();
        let mut dom = self.dom.borrow_mut();
        for (doc, key) in windows {
            dom.set_document_window(doc, None);
            dom.forget_target(key);
        }
    }
    pub fn document(&self) -> NodeId {
        self.document.get()
    }
    /// Switches the displayed document (navigation commit).
    pub fn set_document(&self, doc: NodeId) {
        self.document.set(doc);
        self.opaque_storage.clear();
        let viewport = self.layout.borrow().viewport().clone();
        *self.layout.borrow_mut() = lp_dom::layout::Layout::with_viewport(doc, viewport);
    }

    /// Emulated viewport (CDP Emulation.setDeviceMetricsOverride etc.).
    pub fn set_viewport(&self, viewport: lp_dom::media::Viewport) {
        self.layout.borrow_mut().set_viewport(viewport);
    }
    pub fn viewport(&self) -> lp_dom::media::Viewport {
        self.layout.borrow().viewport().clone()
    }

    /// Applies a resource event to the style engine (`<style>` text and
    /// stylesheet removal). Linked stylesheet bodies are supplied with
    /// `set_stylesheet_text` once fetched.
    pub fn apply_style_event(&self, ev: &lp_dom::script::ResourceEvent) {
        use lp_dom::script::{ResourceKind, ResourceReason};
        let mut layout = self.layout.borrow_mut();
        let style = layout.style_mut();
        match (ev.kind, ev.reason) {
            (ResourceKind::StyleText | ResourceKind::Stylesheet, ResourceReason::Removed) => {
                style.remove_sheet(ev.node);
            }
            (ResourceKind::StyleText, _) if !self.sheet_rules.borrow().contains_key(&ev.node) => {
                style.add_sheet(ev.node, lp_dom::style::SheetSource::StyleElement);
            }
            _ => {}
        }
    }

    /// Text of a fetched `<link rel=stylesheet>`.
    pub fn set_stylesheet_text(&self, link: NodeId, text: &str) {
        self.layout.borrow_mut().style_mut().set_sheet_text(link, text);
    }
    pub fn current_script(&self) -> Option<NodeId> {
        self.current_script.get()
    }
    pub fn set_current_script(&self, script: Option<NodeId>) {
        self.current_script.set(script);
    }

    /// Allocates the Window event-target key for `document`.
    pub fn create_window_target(&self, document: NodeId) -> TargetKey {
        let mut dom = self.dom.borrow_mut();
        let key = dom.new_target_key();
        dom.set_document_window(document, Some(key));
        self.window_keys.borrow_mut().push((document, key));
        key
    }

    /// The Window event-target key created by `create_window_target`.
    pub fn window_target(&self) -> Option<TargetKey> {
        let doc = self.document();
        self.window_keys.borrow().iter().find(|(d, _)| *d == doc).map(|(_, k)| *k)
    }

    pub(crate) fn is_window_key(&self, key: TargetKey) -> bool {
        self.window_keys.borrow().iter().any(|(_, k)| *k == key)
    }

    pub(crate) fn node_identity(&self, id: NodeId) -> IdentityKey {
        IdentityKey(self.id, (u64::from(id.generation()) << 32) | u64::from(id.index()))
    }
    pub(crate) fn other_identity(&self, key: u64) -> IdentityKey {
        IdentityKey(self.id | OTHER_NS, key)
    }
    pub(crate) fn event_identity(&self, event: u64) -> IdentityKey {
        IdentityKey(self.id | EVENT_NS, event)
    }
    pub(crate) fn next_event_id(&self) -> u64 {
        let n = self.next_event.get();
        self.next_event.set(n + 1);
        n
    }

    /// Releases a wrapper's node reference (called from payload drop).
    pub(crate) fn release_node(&self, id: NodeId) {
        match self.dom.try_borrow_mut() {
            Ok(mut d) => d.release(id),
            Err(_) => self.pending_release.borrow_mut().push(id),
        }
    }

    /// Wrappers of active ResizeObservers kept alive (diagnostics/tests).
    pub fn resize_observer_pins(&self) -> usize {
        self.resizes.pins.borrow().len()
    }

    /// Safe point (after each task): applies deferred releases, unpins
    /// listener-holding wrappers of disconnected nodes, re-pins reconnected
    /// ones, and frees unreachable DOM nodes.
    pub fn after_task(&self) {
        crate::mutation_observer::after_task(self);
        crate::intersection_observer::after_task(self);
        crate::resize_observer::after_task(self);
        self.url_params_helpers.borrow_mut().retain(|realm, _| {
            self.realms.borrow().get(realm).and_then(|r| r.upgrade()).is_some_and(|r| !r.is_disposed())
        });
        self.header_helpers.borrow_mut().retain(|realm, _| {
            self.realms.borrow().get(realm).and_then(|r| r.upgrade()).is_some_and(|r| !r.is_disposed())
        });
        self.data_view_readers.borrow_mut().retain(|realm, _| {
            self.realms.borrow().get(realm).and_then(|r| r.upgrade()).is_some_and(|r| !r.is_disposed())
        });
        let pending = std::mem::take(&mut *self.pending_release.borrow_mut());
        let mut dom = self.dom.borrow_mut();
        for id in pending {
            dom.release(id);
        }
        crate::target::reconcile_pins(self, &mut dom);
        dom.collect_garbage();
        let released = dom.take_released_listeners();
        drop(dom);
        crate::target::release_listener_slots(self, released);
    }

    pub fn set_hooks(&self, hooks: Option<Rc<dyn PageHooks>>) {
        *self.hooks.borrow_mut() = hooks;
    }
    pub(crate) fn hooks(&self) -> Option<Rc<dyn PageHooks>> {
        self.hooks.borrow().clone()
    }
    pub(crate) fn post_task(&self, task: Box<dyn FnOnce()>) {
        match self.hooks() {
            Some(h) => h.post_task(task),
            None => task(),
        }
    }
    pub(crate) fn navigate(&self, req: NavigationRequest) {
        if let Some(h) = self.hooks() {
            h.navigate(req);
        }
    }
    pub fn remote_poster(&self) -> Option<RemotePoster> {
        self.hooks().and_then(|h| h.remote_poster())
    }
    pub(crate) fn frame_document(&self, iframe: NodeId) -> Option<NodeId> {
        self.hooks().and_then(|h| h.frame_document(iframe))
    }
    pub(crate) fn frame_window<'s>(
        &self,
        s: &lp_js::Scope<'s>,
        iframe: NodeId,
    ) -> lp_js::JsResult<Option<lp_js::Value<'s>>> {
        Ok(self.hooks().and_then(|h| h.frame_window(s, iframe)))
    }

    /// `document.readyState` maintained by the loader ("loading",
    /// "interactive", "complete").
    pub fn set_ready_state(&self, doc: NodeId, state: &'static str) {
        self.ready.borrow_mut().insert(doc, state);
    }
    pub fn ready_state(&self, doc: NodeId) -> &'static str {
        self.ready.borrow().get(&doc).copied().unwrap_or("complete")
    }

    pub(crate) fn remember_focus_value(&self, el: NodeId) {
        if let Ok(v) = self.dom.borrow().value(el) {
            self.focus_values.borrow_mut().insert(el, v);
        }
    }
    pub(crate) fn take_focus_value(&self, el: NodeId) -> Option<String> {
        self.focus_values.borrow_mut().remove(&el)
    }
    pub(crate) fn begin_click(&self, el: NodeId) -> bool {
        let mut c = self.clicking.borrow_mut();
        if c.contains(&el) {
            return false;
        }
        c.push(el);
        true
    }
    pub(crate) fn end_click(&self, el: NodeId) {
        self.clicking.borrow_mut().retain(|&x| x != el);
    }
    pub(crate) fn begin_submit(&self, form: NodeId) -> bool {
        let mut c = self.submitting.borrow_mut();
        if c.contains(&form) {
            return false;
        }
        c.push(form);
        true
    }
    pub(crate) fn end_submit(&self, form: NodeId) {
        self.submitting.borrow_mut().retain(|&x| x != form);
    }
    pub(crate) fn hovered(&self) -> Option<NodeId> {
        self.hovered.get().filter(|&h| self.dom.borrow().is_alive(h))
    }
    pub(crate) fn set_hovered(&self, el: Option<NodeId>) {
        self.hovered.set(el);
    }
    pub(crate) fn set_press_target(&self, el: Option<NodeId>) {
        self.press_target.set(el);
    }
    pub(crate) fn take_press_target(&self) -> Option<NodeId> {
        self.press_target.take().filter(|&p| self.dom.borrow().is_alive(p))
    }
    pub(crate) fn dialog_return(&self, el: NodeId) -> String {
        self.dialog_returns.borrow().get(&el).cloned().unwrap_or_default()
    }
    pub(crate) fn set_dialog_return(&self, el: NodeId, v: String) {
        self.dialog_returns.borrow_mut().insert(el, v);
    }

    /// CSSOM rules inserted via `insertRule` for a `<style>`/`<link>` owner.
    pub fn sheet_rules(&self, owner: NodeId) -> Vec<String> {
        self.sheet_rules.borrow().get(&owner).cloned().unwrap_or_default()
    }
    pub(crate) fn set_sheet_rules(&self, owner: NodeId, rules: Vec<String>) {
        // CSSOM edits replace the sheet text (as upstream does).
        self.layout.borrow_mut().style_mut().set_sheet_text(owner, &rules.join("\n"));
        self.sheet_rules.borrow_mut().insert(owner, rules);
    }

    /// Sets the files of an `<input type=file>` (CDP DOM.setFileInputFiles).
    pub fn set_input_files(&self, input: NodeId, files: Vec<crate::file::FileEntry>) -> Result<(), lp_dom::DomError> {
        let names: Vec<String> = files.iter().map(|f| f.name.clone()).collect();
        self.dom.borrow_mut().set_file_names(input, &names)?;
        self.files.borrow_mut().insert(input, files);
        Ok(())
    }

    /// Number of wrappers pinned for listeners (diagnostics/tests).
    pub fn pinned_count(&self) -> usize {
        self.pinned.borrow().len()
    }

    /// Borrow helper for bindings.
    pub fn with_dom<R>(&self, f: impl FnOnce(&mut Dom) -> R) -> R {
        f(&mut self.dom.borrow_mut())
    }
}
