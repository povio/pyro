//! Per-page binding state shared by every realm (main world + isolated
//! worlds) that exposes the page's DOM.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use lp_dom::events::TargetKey;
pub use lp_dom::iframe::IframeAttribute;
use lp_dom::parse::DomRef;
use lp_dom::select::SelectorCache;
use lp_dom::{Dom, NodeId};
use lp_js::{IdentityKey, Persistent, RealmId, WeakRealm};
use rustc_hash::{FxHashMap, FxHashSet};

static NEXT_PAGE: AtomicU64 = AtomicU64::new(1);

/// High bit marks identity keys of non-node event targets / events.
pub(crate) const OTHER_NS: u64 = 1 << 63;
pub(crate) const EVENT_NS: u64 = 1 << 62;

pub struct PageDom {
    pub dom: DomRef,
    id: u64,
    disposed: Cell<bool>,
    document: Cell<NodeId>,
    scripting_disabled: Cell<bool>,
    flushing_iframes: Cell<bool>,
    connected_iframes: RefCell<FxHashSet<NodeId>>,
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
    /// Each realm's original `structuredClone` (MessagePort cloning).
    pub(crate) structured_clones: RefCell<FxHashMap<RealmId, Persistent>>,
    /// Each realm's original `Object.defineProperty` ([Replaceable] setters).
    pub(crate) define_properties: RefCell<FxHashMap<RealmId, Persistent>>,
    /// MessagePorts whose peer was collected; their pins are reconciled at
    /// the next `after_task` (recorded from `Drop`, which must not run JS).
    pub(crate) pending_port_reconcile: RefCell<Vec<std::rc::Weak<crate::message_channel::PortInner>>>,
    pub(crate) last_removals: Cell<u64>,
    next_event: Cell<u64>,
    pub net: RefCell<Option<lp_net::NetClient>>,
    pub(crate) intersections: crate::intersection_observer::BindingState,
    pub(crate) resizes: crate::resize_observer::BindingState,
    pub(crate) mutations: crate::mutation_observer::BindingState,
    pub(crate) identity: RefCell<crate::identity::IdentityState>,
    pub(crate) fetch_state: crate::fetch::FetchState,
    pub(crate) xhr_state: crate::xhr::XhrState,
    pub(crate) file_readers: crate::file_reader::ReaderState,
    pub(crate) local_storage: RefCell<crate::storage::StorageNamespace>,
    pub(crate) session_storage: RefCell<crate::storage::StorageNamespace>,
    pub(crate) opaque_storage: crate::storage::OpaqueStorage,
    /// `URL.createObjectURL` store: blob URL -> (bytes, type). Owned by the
    /// creating document; revocation, document replacement and disposal drop
    /// the entries (and their bytes).
    pub(crate) blob_urls: RefCell<FxHashMap<String, (bytes::Bytes, String)>>,
    pub(crate) reflected_url_base: RefCell<Option<crate::html::ReflectedUrlBase>>,
    hooks: RefCell<Option<Rc<dyn PageHooks>>>,
    ready: RefCell<FxHashMap<NodeId, &'static str>>,
    pub(crate) focus_values: RefCell<FxHashMap<NodeId, String>>,
    pub(crate) focus_transition: RefCell<Option<FocusTransition>>,
    pub(crate) editing_target: Cell<Option<NodeId>>,
    clicking: RefCell<Vec<NodeId>>,
    submitting: RefCell<Vec<NodeId>>,
    hovered: Cell<Option<NodeId>>,
    /// `<img>` resource state set by the embedder: the `src` value the
    /// current load is for, and whether it is still loading.
    images: RefCell<FxHashMap<NodeId, (String, bool)>>,
    /// The last mouse input's position and state, for boundary events when
    /// content under a stationary pointer changes.
    pub(crate) pointer: RefCell<Option<crate::input::MouseInput>>,
    press_target: Cell<Option<NodeId>>,
    /// Native drag source under a left press that has not moved past the
    /// drag threshold yet, with the press position.
    pub(crate) drag_candidate: Cell<Option<(NodeId, f64, f64)>>,
    /// The document selection (`crate::editing`), shared by every world.
    pub(crate) selection: RefCell<Option<crate::editing::DocSelection>>,
    pub(crate) selectionchange_pending: Cell<bool>,
    /// The native drag-and-drop session in progress, if any.
    pub(crate) drag: RefCell<Option<crate::input::DragSession>>,
    /// `document.open()`'s script-created parser (`crate::html` document
    /// open/write/close), a close() deferred from a script it runs, and that
    /// script nesting level.
    pub(crate) script_parser: RefCell<Option<lp_dom::parse::DocumentParser>>,
    pub(crate) script_parser_active: Cell<bool>,
    pub(crate) script_parser_close: Cell<bool>,
    pub(crate) script_nesting: Cell<u32>,
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
    /// A new browsing context (popup) keeps its opener (`window.opener`):
    /// `window.open` without `noopener`, or a link with `rel=opener`.
    pub opener: bool,
}

/// HTML "download the hyperlink" for a `download` link.
#[derive(Clone, Debug)]
pub struct DownloadRequest {
    pub url: String,
    /// The `download` attribute value (may be empty).
    pub filename: String,
    /// `blob:`/`data:` bytes and MIME type, captured at activation: the page
    /// may revoke a blob URL right after `click()`. None for HTTP(S) URLs,
    /// which the embedder fetches.
    pub data: Option<(bytes::Bytes, String)>,
}

/// Posts a `Send` closure to run on the browser thread as a separate task.
pub type RemotePoster = std::sync::Arc<dyn Fn(Box<dyn FnOnce() + Send>) + Send + Sync>;

/// Cancellation shared by native focus steps and the browsing-context owner.
/// A nested transition cancels the interrupted one across document boundaries.
#[derive(Clone, Debug)]
pub struct FocusTransition {
    current: Rc<Cell<bool>>,
    prevent_scroll: bool,
}

impl Default for FocusTransition {
    fn default() -> Self {
        Self::new()
    }
}
impl FocusTransition {
    pub fn new() -> Self {
        Self::with_prevent_scroll(false)
    }
    pub(crate) fn with_prevent_scroll(prevent_scroll: bool) -> Self {
        Self { current: Rc::new(Cell::new(true)), prevent_scroll }
    }
    pub(crate) fn prevents_scroll(&self) -> bool {
        self.prevent_scroll
    }
    pub fn is_current(&self) -> bool {
        self.current.get()
    }
    pub fn cancel(&self) {
        self.current.set(false);
    }
    pub fn ptr_eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.current, &other.current)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusTarget {
    Element(NodeId),
    Window,
    Blur(NodeId),
}

/// Embedder (lp-browser) services the bindings need.
pub trait PageHooks {
    /// Observation-only (no JS): this document still belongs to a live owner
    /// frame/loader. Unlike is_focused, unselected frames also return true.
    /// Checked before a request can cancel a shared transition token.
    fn focus_document_is_live(&self) -> bool {
        true
    }
    /// Called outside binding borrows, including same-local-target requests.
    /// False means the owner handled or rejected the request. Owners applying
    /// native helpers must pass this same token and check it after callbacks.
    fn request_focus(&self, _target: FocusTarget, _transition: &FocusTransition) -> bool {
        true
    }
    /// Whether this live document is the owner's selected keyboard frame.
    fn is_focused(&self) -> bool {
        true
    }

    /// Schedules `task` on the page's event loop as a separate task.
    fn post_task(&self, task: Box<dyn FnOnce()>);
    /// Schedules `task` as a separate task after `delay` (default: at once).
    fn post_delayed_task(&self, _delay: std::time::Duration, task: Box<dyn FnOnce()>) {
        self.post_task(task)
    }
    /// Starts a navigation of the frame showing the document.
    fn navigate(&self, request: NavigationRequest);
    /// Thread-safe poster that runs a closure as a task on the page's
    /// browser thread (lp-net sinks run on the I/O thread). `None` when the
    /// embedder cannot post across threads (network APIs then fail).
    fn remote_poster(&self) -> Option<RemotePoster> {
        None
    }
    /// Transport progress made visible by fetch/XHR on the owner thread.
    /// Called after the delivery snapshot and before any resulting JS, with
    /// binding locks/borrows released. Default embedders need no observer.
    fn network_delivery(&self, _id: lp_net::RequestId, _frontier: lp_net::NetworkDelivery) {}
    /// A hyperlink download. The default embedder performs none.
    fn download(&self, _request: DownloadRequest) {}
    /// An `<input type=file>` was activated: the embedder may report a file
    /// chooser (CDP Page.fileChooserOpened). There is no native picker.
    fn file_chooser(&self, _input: NodeId, _multiple: bool) {}
    /// `document.open()` replaced the loaded document's content: the frame is
    /// loading again, so DOMContentLoaded/load can be reported anew.
    fn document_reopened(&self) {}
    /// `document.close()` finished the script-created parser (readyState is
    /// already "interactive"): fire DOMContentLoaded, then load once nothing
    /// blocks it. Returns false when the embedder does not handle it.
    fn document_closed(&self) -> bool {
        false
    }
    /// Called without DOM/hook borrows before the inserting native call returns.
    fn iframe_connected(&self, _iframe: NodeId) {}
    /// Removed iframe owners, descendants before ancestors.
    fn iframe_disconnected(&self, _iframe: NodeId) {}
    fn iframe_attribute_changed(&self, _iframe: NodeId, _attribute: IframeAttribute) {}
    /// Actual child document wrapper, bound to the child's PageDom/world.
    /// `Some(null)` denies access; `None` opts into the legacy NodeId fallback.
    fn frame_document_value<'s>(
        &self,
        _scope: &lp_js::Scope<'s>,
        _iframe: NodeId,
    ) -> lp_js::JsResult<Option<lp_js::Value<'s>>> {
        Ok(None)
    }
    /// Legacy document lookup for embedders sharing the parent's binding state.
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
        dom.borrow_mut().watch_iframes(document).expect("page document");
        Rc::new(PageDom {
            dom,
            id: NEXT_PAGE.fetch_add(1, Ordering::Relaxed),
            disposed: Cell::new(false),
            document: Cell::new(document),
            scripting_disabled: Cell::new(false),
            flushing_iframes: Cell::new(false),
            connected_iframes: RefCell::new(FxHashSet::default()),
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
            structured_clones: RefCell::new(FxHashMap::default()),
            define_properties: RefCell::new(FxHashMap::default()),
            pending_port_reconcile: RefCell::new(Vec::new()),
            last_removals: Cell::new(u64::MAX),
            next_event: Cell::new(1),
            net: RefCell::new(None),
            identity: RefCell::new(Default::default()),
            mutations: Default::default(),
            intersections: Default::default(),
            resizes: Default::default(),
            fetch_state: Default::default(),
            xhr_state: Default::default(),
            file_readers: Default::default(),
            local_storage: RefCell::new(Default::default()),
            session_storage: RefCell::new(Default::default()),
            opaque_storage: Default::default(),
            blob_urls: RefCell::new(FxHashMap::default()),
            reflected_url_base: RefCell::new(None),
            hooks: RefCell::new(None),
            ready: RefCell::new(FxHashMap::default()),
            focus_values: RefCell::new(FxHashMap::default()),
            focus_transition: RefCell::new(None),
            editing_target: Cell::new(None),
            clicking: RefCell::new(Vec::new()),
            submitting: RefCell::new(Vec::new()),
            hovered: Cell::new(None),
            images: RefCell::new(FxHashMap::default()),
            pointer: RefCell::new(None),
            press_target: Cell::new(None),
            drag_candidate: Cell::new(None),
            drag: RefCell::new(None),
            selection: RefCell::new(None),
            selectionchange_pending: Cell::new(false),
            script_parser: RefCell::new(None),
            script_parser_active: Cell::new(false),
            script_parser_close: Cell::new(false),
            script_nesting: Cell::new(0),
            space_target: Cell::new(None),
            dialog_returns: RefCell::new(FxHashMap::default()),
            sheet_rules: RefCell::new(FxHashMap::default()),
            files: RefCell::new(FxHashMap::default()),
            layout: RefCell::new(new_layout(lp_dom::layout::Layout::new(document))),
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

    /// Number of live object URLs registered by the current document.
    pub fn blob_url_count(&self) -> usize {
        self.blob_urls.borrow().len()
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
        crate::input::clear_document_focus(self);
        self.dom.borrow_mut().unwatch_iframes(self.document());
        self.connected_iframes.borrow_mut().clear();
        crate::fetch::dispose(self);
        crate::xhr::dispose(self);
        crate::file_reader::dispose(self);
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
        drop(self.structured_clones.take());
        drop(self.define_properties.take());
        drop(self.pending_port_reconcile.take());
        drop(self.data_view_readers.take());
        drop(self.header_helpers.take());
        drop(self.url_params_helpers.take());
        self.opaque_storage.clear();
        drop(self.blob_urls.take());
        drop(self.reflected_url_base.take());
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
        crate::input::clear_document_focus(self);
        {
            let mut dom = self.dom.borrow_mut();
            dom.unwatch_iframes(self.document());
            dom.watch_iframes(doc).expect("replacement document");
        }
        self.connected_iframes.borrow_mut().clear();
        self.document.set(doc);
        self.opaque_storage.clear();
        // Object URLs belong to the replaced document.
        drop(self.blob_urls.take());
        drop(self.reflected_url_base.take());
        let viewport = self.layout.borrow().viewport().clone();
        *self.layout.borrow_mut() = new_layout(lp_dom::layout::Layout::with_viewport(doc, viewport));
    }

    /// Browser-owned sandbox policy. Explicit inspector/Realm evaluation is
    /// unaffected; parser, dynamic-script and timer checks belong to the embedder.
    pub fn set_scripting_disabled(&self, disabled: bool) {
        self.scripting_disabled.set(disabled);
    }
    pub fn scripting_disabled(&self) -> bool {
        self.scripting_disabled.get()
    }

    /// Flush after parser/native mutation before entering page script. Native
    /// binding callbacks call this automatically after releasing their borrows.
    /// Reentrant mutations are drained by the outer flush; a held DOM borrow
    /// defers work until the next safe point.
    pub fn flush_iframe_events(&self) {
        use lp_dom::iframe::IframeChange;
        if self.is_disposed() || self.flushing_iframes.replace(true) {
            return;
        }
        struct Reset<'a>(&'a Cell<bool>);
        impl Drop for Reset<'_> {
            fn drop(&mut self) {
                self.0.set(false);
            }
        }
        let _reset = Reset(&self.flushing_iframes);
        loop {
            let events = {
                let Ok(mut dom) = self.dom.try_borrow_mut() else { return };
                dom.take_iframe_events(self.document())
            };
            if events.is_empty() {
                break;
            }
            struct Pins<'a> {
                page: &'a PageDom,
                events: Vec<lp_dom::iframe::IframeEvent>,
            }
            impl Drop for Pins<'_> {
                fn drop(&mut self) {
                    let mut dom = self.page.dom.borrow_mut();
                    for event in &self.events {
                        dom.release(event.node);
                    }
                }
            }
            let pins = Pins { page: self, events };
            for event in &pins.events {
                if self.is_disposed() {
                    return;
                }
                let Some(hooks) = self.hooks() else { continue };
                let connected = {
                    let dom = self.dom.borrow();
                    dom.is_connected(event.node).unwrap_or(false)
                        && dom.node_document(event.node).ok() == Some(self.document())
                };
                match event.change {
                    IframeChange::Connected if connected => {
                        let new = self.connected_iframes.borrow_mut().insert(event.node);
                        if new {
                            hooks.iframe_connected(event.node);
                        }
                    }
                    IframeChange::Disconnected => {
                        let removed = self.connected_iframes.borrow_mut().remove(&event.node);
                        if removed {
                            hooks.iframe_disconnected(event.node);
                        }
                    }
                    IframeChange::AttributeChanged(attribute) if connected => {
                        hooks.iframe_attribute_changed(event.node, attribute);
                    }
                    _ => (),
                }
            }
        }
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
    pub fn after_task(self: &Rc<Self>) {
        self.flush_iframe_events();
        crate::message_channel::reconcile_ports(self);
        crate::mutation_observer::after_task(self);
        crate::intersection_observer::after_task(self);
        crate::resize_observer::after_task(self);
        crate::input::update_hover_after_task(self);
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
        let seed = self.hooks.borrow().is_none() && hooks.is_some();
        drop(self.hooks.replace(hooks));
        if seed && !self.is_disposed() {
            let mut dom = self.dom.borrow_mut();
            dom.unwatch_iframes(self.document());
            dom.watch_iframes(self.document()).expect("page document");
            self.connected_iframes.borrow_mut().clear();
        }
    }
    pub(crate) fn hooks(&self) -> Option<Rc<dyn PageHooks>> {
        self.hooks.borrow().clone()
    }
    /// The page's main-world realm: the earliest live realm without a world
    /// name (isolated worlds are named).
    pub(crate) fn main_world_realm(&self) -> Option<lp_js::Realm> {
        let realms = self.realms.borrow();
        let mut live: Vec<_> = realms.iter().filter_map(|(id, r)| Some((id.0, r.upgrade()?))).collect();
        live.retain(|(_, r)| !r.is_disposed() && r.name().is_empty());
        live.into_iter().min_by_key(|(id, _)| *id).map(|(_, r)| r)
    }

    pub(crate) fn post_task(&self, task: Box<dyn FnOnce()>) {
        match self.hooks() {
            Some(h) => h.post_task(task),
            None => task(),
        }
    }
    pub(crate) fn download(&self, request: DownloadRequest) {
        if let Some(h) = self.hooks() {
            h.download(request);
        }
    }
    pub(crate) fn navigate(&self, req: NavigationRequest) {
        if self.scripting_disabled() && lp_net::Url::parse(&req.url).is_ok_and(|url| url.scheme() == "javascript") {
            return;
        }
        if let Some(h) = self.hooks() {
            h.navigate(req);
        }
    }
    pub(crate) fn network_delivery(&self, id: lp_net::RequestId, frontier: lp_net::NetworkDelivery) {
        if !self.is_disposed() {
            let hooks = self.hooks();
            if let Some(hooks) = hooks {
                hooks.network_delivery(id, frontier);
            }
        }
    }
    pub fn remote_poster(&self) -> Option<RemotePoster> {
        self.hooks().and_then(|h| h.remote_poster())
    }
    pub(crate) fn frame_document(&self, iframe: NodeId) -> Option<NodeId> {
        self.hooks().and_then(|h| h.frame_document(iframe))
    }
    pub(crate) fn frame_document_value<'s>(
        &self,
        s: &lp_js::Scope<'s>,
        iframe: NodeId,
    ) -> lp_js::JsResult<Option<lp_js::Value<'s>>> {
        match self.hooks() {
            Some(hooks) => hooks.frame_document_value(s, iframe),
            None => Ok(None),
        }
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
    /// Embedder: an `<img>` started (`pending`) or finished loading the
    /// resource for `src` (the attribute value); `None` forgets the image.
    pub fn set_image_state(&self, el: NodeId, src: Option<&str>, pending: bool) {
        match src {
            Some(src) => {
                self.images.borrow_mut().insert(el, (src.to_owned(), pending));
            }
            None => {
                self.images.borrow_mut().remove(&el);
            }
        }
    }
    /// HTMLImageElement.complete: no `src`, or the current `src` has
    /// finished loading (successfully or not). A `src` set since the last
    /// load started is not complete yet, as in Chromium.
    pub(crate) fn image_complete(&self, el: NodeId) -> bool {
        let src = self.dom.borrow().get_attribute(el, "src").ok().flatten().map(str::to_owned);
        let Some(src) = src else { return true };
        match self.images.borrow().get(&el) {
            Some((loading, pending)) if *loading == src => !pending,
            // Empty or unresolvable sources are complete (broken).
            _ => src.trim().is_empty(),
        }
    }
    pub(crate) fn hovered(&self) -> Option<NodeId> {
        self.hovered.get().filter(|&h| self.dom.borrow().is_alive(h))
    }
    /// The hovered element even when it is no longer connected.
    pub(crate) fn hovered_raw(&self) -> Option<NodeId> {
        self.hovered.get()
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

/// A document layout with the process's geometry selection
/// (`LIGHTNINGPANDA_GEOMETRY=flow` opts into the flow layout).
fn new_layout(mut layout: lp_dom::layout::Layout) -> lp_dom::layout::Layout {
    layout.set_geometry(lp_dom::layout::Geometry::from_env());
    layout
}
