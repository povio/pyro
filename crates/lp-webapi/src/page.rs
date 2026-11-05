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
    /// Members of live collections by (root, kind), shared by every
    /// collection object over the same root and kind
    /// (`collections::CollectionData`).
    pub(crate) collection_members: RefCell<crate::collections::MemberCache>,
    /// Node references released by finalized wrappers while the DOM was
    /// borrowed; applied at the next safe point.
    pending_release: RefCell<Vec<NodeId>>,
    /// Attr wrappers an element adopted through `setAttributeNode` keyed by
    /// (element, namespace, local name): their wrapper identity replaces the
    /// default one for that attribute.
    pub(crate) attr_keys: RefCell<FxHashMap<(NodeId, String, String), lp_js::IdentityKey>>,
    /// (DOM version, removal count) seen by the last `after_task`.
    after_task_stamp: Cell<Option<(u64, u64)>>,
    current_script: Cell<Option<NodeId>>,
    pub(crate) realms: RefCell<FxHashMap<RealmId, WeakRealm>>,
    pub(crate) url_params_helpers: RefCell<FxHashMap<RealmId, Persistent>>,
    pub(crate) header_helpers: RefCell<FxHashMap<RealmId, Persistent>>,
    pub(crate) data_view_readers: RefCell<FxHashMap<RealmId, Persistent>>,
    /// Wrappers kept alive because they hold listeners for a connected
    /// node (or an active non-node target): (realm, target) -> wrapper.
    pub(crate) pinned: RefCell<FxHashMap<(RealmId, TargetKey), Persistent>>,
    /// Node wrappers pinned since the last task because script added a property to them, to
    /// unpin if the node is not connected and to mark for re-pinning when it connects.
    pub(crate) new_property_pins: RefCell<Vec<(RealmId, NodeId)>>,
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
    /// Event targets of each realm's `visualViewport` and `screen.orientation`
    /// (registered by their wrappers, removed when those are collected), for
    /// the embedder's resize and orientation change events.
    pub(crate) viewport_targets: RefCell<Vec<(crate::window::ViewportTarget, TargetKey)>>,
    /// Live `MediaQueryList` targets (one per realm and query text) with the
    /// last `matches` each reported, for the embedder's `change` events.
    pub(crate) media_queries: RefCell<Vec<crate::layout_api::MediaQueryEntry>>,
    /// Each realm's original `structuredClone` (MessagePort cloning).
    pub(crate) structured_clones: RefCell<FxHashMap<RealmId, Persistent>>,
    /// Each realm's message encoder and decoder for workers, once it has used one.
    pub(crate) worker_wire: RefCell<FxHashMap<RealmId, Persistent>>,
    /// Workers this document started that have not ended (`crate::worker`).
    pub(crate) workers: RefCell<Vec<Rc<crate::worker::WorkerInner>>>,
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
    /// Scroll event targets awaiting the next rendering update, per document
    /// (None: the document itself), each fired once ("pending scroll event
    /// targets"). Keyed by document: a task dropped by a navigation must not
    /// block the next document's events. The value: whether a `scrollend`
    /// follows (a scroll that ran to completion, not a layout clamp).
    pub(crate) pending_scrolls: RefCell<Vec<((NodeId, Option<NodeId>), bool, Rc<()>)>>,
    /// The DOM version scroll offsets were last re-clamped at, and whether a
    /// re-clamp is queued for the next frame (`layout_api::clamp_scrolls`).
    pub(crate) scroll_clamp: Cell<(Option<u64>, bool)>,
    pub(crate) mutations: crate::mutation_observer::BindingState,
    pub(crate) identity: RefCell<crate::identity::IdentityState>,
    /// The browser context's permissions and clipboard.
    context_services: RefCell<Rc<crate::navigator_extras::ContextServices>>,
    /// `navigator.maxTouchPoints` (Emulation.setTouchEmulationEnabled).
    max_touch_points: Cell<u32>,
    /// Per realm with PermissionStatus change listeners: the function that
    /// re-reads their states (navigator_extras).
    pub(crate) permission_watchers: RefCell<Vec<(lp_js::WeakRealm, lp_js::Persistent)>>,
    pub(crate) fetch_state: crate::fetch::FetchState,
    pub(crate) xhr_state: crate::xhr::XhrState,
    pub(crate) file_readers: crate::file_reader::ReaderState,
    pub(crate) local_storage: RefCell<crate::storage::StorageNamespace>,
    pub(crate) session_storage: RefCell<crate::storage::StorageNamespace>,
    pub(crate) opaque_storage: crate::storage::OpaqueStorage,
    /// Named popups this document opened (`window.open(url, name)`): a
    /// later open with the same name reuses the popup while it is open.
    pub(crate) popups: RefCell<Vec<(String, Rc<crate::popup::PopupState>)>>,
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
    /// A deferred hover update after the hovered element was removed is
    /// scheduled (`crate::input::update_hover_after_task`).
    pub(crate) hover_update_queued: Cell<bool>,
    press_target: Cell<Option<NodeId>>,
    /// The `pointerdown` of the press in progress was canceled: its
    /// compatibility mouse events (moves while down, `mouseup`) are not
    /// sent, as in Chromium; a canceled `pointerup` suppresses nothing.
    pub(crate) compat_mouse_suppressed: Cell<bool>,
    /// Native drag source under a left press that has not moved past the
    /// drag threshold yet, with the press position.
    pub(crate) drag_candidate: Cell<Option<(NodeId, f64, f64)>>,
    /// Native text selection under a primary press (`crate::input`).
    pub(crate) text_drag: Cell<Option<crate::input::TextDrag>>,
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
    /// The network parser, lent by the loader while a script it reached runs:
    /// that script's `document.write`s parse at its insertion point.
    pub(crate) network_parser: RefCell<Option<lp_dom::parse::DocumentParser>>,
    pub(crate) network_parser_lent: Cell<bool>,
    /// External and module scripts reached in written markup, for the embedder
    /// to fetch, and the written parser-blocking one tokenizing waits for.
    pub(crate) written_scripts: RefCell<Vec<NodeId>>,
    pub(crate) written_pending: Cell<Option<NodeId>>,
    /// HTML "ignore-destructive-writes counter": a script that is external or
    /// a module runs, so a write with no insertion point is ignored.
    pub(crate) ignore_destructive_writes: Cell<u32>,
    /// Control activated by a Space keydown, clicked on the matching keyup.
    pub(crate) space_target: Cell<Option<NodeId>>,
    dialog_returns: RefCell<FxHashMap<NodeId, String>>,
    /// `nonce` IDL values, keyed with the content attribute they were set
    /// over: a later attribute change wins, as it updates the internal slot.
    nonces: RefCell<FxHashMap<NodeId, (Option<String>, String)>>,
    pub(crate) sheet_models: RefCell<FxHashMap<NodeId, Rc<crate::css::SheetModel>>>,
    /// Files selected into `<input type=file>` (CDP DOM.setFileInputFiles).
    pub(crate) files: RefCell<FxHashMap<NodeId, Vec<crate::file::FileEntry>>>,
    /// Faux layout + style engine of the displayed document. Read it with
    /// [`PageDom::layout`], which first sizes a child frame's viewport.
    pub layout: RefCell<lp_dom::layout::Layout>,
    /// A child frame's viewport (its iframe's content box): `None` for top
    /// documents, which keep the emulated viewport.
    frame_viewport: RefCell<Option<FrameViewportSource>>,
    /// Child frames: the viewport was sized from the iframe once (later
    /// changes are resizes), a `resize` task is queued, and the earliest a
    /// listening window re-reads its iframe after a task (with a wake queued).
    frame_sized: Cell<bool>,
    frame_resize_queued: Cell<bool>,
    frame_resize_next: Cell<Option<std::time::Instant>>,
    frame_resize_wake: Cell<bool>,
    this: std::rc::Weak<PageDom>,
}

/// Computes a child frame's viewport from its iframe; `None`: unknown yet,
/// keep the current one.
pub type FrameViewportSource = Rc<dyn Fn() -> Option<lp_dom::media::Viewport>>;

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
    /// `window.open`'s returned object watches this popup.
    pub popup: Option<Rc<crate::popup::PopupTicket>>,
}

/// How a blocking `alert`/`confirm`/`prompt` was answered.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DialogAnswer {
    pub accept: bool,
    /// The text a `prompt` returns when accepted (empty when not given).
    pub prompt_text: String,
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
    /// The document's `window.open` result asked something of its popup,
    /// after (and in order with) the navigation that requested the popup.
    fn popup_command(&self, _popup: Rc<crate::popup::PopupState>, _command: crate::popup::PopupCommand) {}
    /// `window.close()`: the embedder closes the page if its top-level
    /// browsing context is script-closable (a popup). The default ignores it.
    fn close_window(&self) {}
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
    /// A dedicated worker of this document started or ended.
    fn worker_event(&self, _event: crate::worker::WorkerEvent) {}
    /// A hyperlink download. The default embedder performs none.
    fn download(&self, _request: DownloadRequest) {}
    /// An `<input type=file>` was activated: the embedder may report a file
    /// chooser (CDP Page.fileChooserOpened). There is no native picker.
    fn file_chooser(&self, _input: NodeId, _multiple: bool) {}
    /// `alert`, `confirm` or `prompt` (`kind`) was called from the document
    /// at `url`. An embedder that shows the dialog blocks here until it is
    /// answered and returns the answer; `None` reports it without blocking
    /// (the call then returns what a dismissed dialog gives).
    fn javascript_dialog(
        &self,
        _kind: &'static str,
        _message: String,
        _default_prompt: Option<String>,
        _url: String,
    ) -> Option<DialogAnswer> {
        None
    }
    /// `document.open()` replaced the loaded document's content: the frame is
    /// loading again, so DOMContentLoaded/load can be reported anew.
    fn document_reopened(&self) {}
    /// `document.close()` finished the script-created parser (readyState is
    /// already "interactive"): fire DOMContentLoaded, then load once nothing
    /// blocks it. Returns false when the embedder does not handle it.
    fn document_closed(&self) -> bool {
        false
    }
    /// An external or module script was reached in markup written to a
    /// script-created parser: schedule `PageDom::take_written_scripts` for the
    /// loader (a blocking one reports through
    /// `lifecycle::written_script_fetched`). False when the embedder cannot
    /// load scripts; the write then reports them as unsupported.
    fn written_script_queued(&self) -> bool {
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
    /// Live realms of `scope`'s script world in the other documents that share
    /// this page's `Dom`, so a node or event keeps one wrapper across them.
    fn sibling_realms(&self, _scope: &lp_js::Scope<'_>) -> Vec<lp_js::Realm> {
        Vec::new()
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
        Rc::new_cyclic(|this| PageDom {
            this: this.clone(),
            frame_sized: Cell::new(false),
            frame_resize_queued: Cell::new(false),
            frame_resize_next: Cell::new(None),
            frame_resize_wake: Cell::new(false),
            dom,
            id: NEXT_PAGE.fetch_add(1, Ordering::Relaxed),
            disposed: Cell::new(false),
            document: Cell::new(document),
            scripting_disabled: Cell::new(false),
            flushing_iframes: Cell::new(false),
            connected_iframes: RefCell::new(FxHashSet::default()),
            selectors: RefCell::new(SelectorCache::default()),
            collection_members: RefCell::default(),
            pending_release: RefCell::new(Vec::new()),
            attr_keys: RefCell::new(FxHashMap::default()),
            after_task_stamp: Cell::new(None),
            current_script: Cell::new(None),
            realms: RefCell::new(FxHashMap::default()),
            data_view_readers: RefCell::new(FxHashMap::default()),
            header_helpers: RefCell::new(FxHashMap::default()),
            url_params_helpers: RefCell::new(FxHashMap::default()),
            pinned: RefCell::new(FxHashMap::default()),
            new_property_pins: RefCell::new(Vec::new()),
            window_keys: RefCell::new(Vec::new()),
            window_holders: RefCell::new(FxHashMap::default()),
            same_objects: RefCell::new(FxHashMap::default()),
            viewport_targets: RefCell::new(Vec::new()),
            media_queries: RefCell::new(Vec::new()),
            structured_clones: RefCell::new(FxHashMap::default()),
            worker_wire: RefCell::new(FxHashMap::default()),
            workers: RefCell::new(Vec::new()),
            define_properties: RefCell::new(FxHashMap::default()),
            pending_port_reconcile: RefCell::new(Vec::new()),
            last_removals: Cell::new(u64::MAX),
            next_event: Cell::new(1),
            net: RefCell::new(None),
            identity: RefCell::new(Default::default()),
            context_services: Default::default(),
            max_touch_points: Cell::new(0),
            permission_watchers: RefCell::new(Vec::new()),
            mutations: Default::default(),
            intersections: Default::default(),
            resizes: Default::default(),
            pending_scrolls: Default::default(),
            scroll_clamp: Default::default(),
            fetch_state: Default::default(),
            xhr_state: Default::default(),
            file_readers: Default::default(),
            local_storage: RefCell::new(Default::default()),
            session_storage: RefCell::new(Default::default()),
            opaque_storage: Default::default(),
            popups: RefCell::new(Vec::new()),
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
            hover_update_queued: Cell::new(false),
            press_target: Cell::new(None),
            compat_mouse_suppressed: Cell::new(false),
            drag_candidate: Cell::new(None),
            text_drag: Cell::new(None),
            drag: RefCell::new(None),
            selection: RefCell::new(None),
            selectionchange_pending: Cell::new(false),
            script_parser: RefCell::new(None),
            script_parser_active: Cell::new(false),
            script_parser_close: Cell::new(false),
            script_nesting: Cell::new(0),
            network_parser: RefCell::new(None),
            network_parser_lent: Cell::new(false),
            written_scripts: RefCell::new(Vec::new()),
            written_pending: Cell::new(None),
            ignore_destructive_writes: Cell::new(0),
            space_target: Cell::new(None),
            dialog_returns: RefCell::new(FxHashMap::default()),
            nonces: RefCell::new(FxHashMap::default()),
            sheet_models: RefCell::new(FxHashMap::default()),
            files: RefCell::new(FxHashMap::default()),
            layout: RefCell::new(lp_dom::layout::Layout::new(document)),
            frame_viewport: RefCell::new(None),
        })
    }

    /// Set before installing realms. Share local storage per browser context
    /// and session storage per top-level page across document replacements.
    pub fn set_storage_namespaces(
        self: &Rc<Self>,
        local: crate::storage::StorageNamespace,
        session: crate::storage::StorageNamespace,
    ) {
        // Attached pages receive other pages' `storage` events.
        local.attach(self);
        session.attach(self);
        *self.local_storage.borrow_mut() = local;
        *self.session_storage.borrow_mut() = session;
    }

    /// Shares the browser context's permissions and clipboard.
    pub fn set_context_services(&self, services: Rc<crate::navigator_extras::ContextServices>) {
        *self.context_services.borrow_mut() = services;
    }
    pub fn context_services(&self) -> Rc<crate::navigator_extras::ContextServices> {
        self.context_services.borrow().clone()
    }
    /// `navigator.maxTouchPoints`; nonzero also means touch input is
    /// emulated (`Emulation.setTouchEmulationEnabled`).
    pub fn set_max_touch_points(&self, points: u32) {
        self.max_touch_points.set(points);
        if points > 0 {
            let realms: Vec<_> = self.realms.borrow().values().filter_map(|r| r.upgrade()).collect();
            for realm in realms {
                if let Ok(s) = realm.scope() {
                    if let Err(e) = crate::handlers::define_touch_handlers(&s) {
                        s.report_exception(&e);
                    }
                }
            }
        }
    }
    pub fn max_touch_points(&self) -> u32 {
        self.max_touch_points.get()
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
        crate::worker::dispose(self);
        self.realms.borrow_mut().clear();
        // Drop roots and embedder hooks outside RefCell borrows: destructors
        // can release wrappers or other resources owned by this page.
        drop(self.hooks.take());
        drop(self.net.take());
        drop(self.pinned.take());
        drop(self.window_holders.take());
        drop(self.same_objects.take());
        drop(self.permission_watchers.take());
        drop(self.structured_clones.take());
        drop(self.worker_wire.take());
        drop(self.define_properties.take());
        drop(self.pending_port_reconcile.take());
        drop(self.data_view_readers.take());
        drop(self.header_helpers.take());
        drop(self.url_params_helpers.take());
        // A drag in flight owns a Persistent to its DataTransfer wrapper.
        drop(self.drag.take());
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
        *self.layout.borrow_mut() = lp_dom::layout::Layout::with_viewport(doc, viewport);
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
        self.layout().viewport().clone()
    }

    /// Sizes this child frame's viewport from its iframe (see
    /// [`PageDom::layout`]); `None` for a top document.
    pub fn set_frame_viewport_source(&self, source: Option<FrameViewportSource>) {
        *self.frame_viewport.borrow_mut() = source;
    }

    /// The document's layout, after a child frame's viewport is brought up
    /// to its iframe's content box (which lays the parent out when it
    /// changed since). Top documents pay one empty check.
    pub fn layout(&self) -> std::cell::Ref<'_, lp_dom::layout::Layout> {
        self.sync_frame_viewport();
        self.layout.borrow()
    }

    /// [`PageDom::layout`] without panicking while the layout is borrowed
    /// mutably.
    pub fn try_layout(&self) -> Option<std::cell::Ref<'_, lp_dom::layout::Layout>> {
        self.sync_frame_viewport();
        self.layout.try_borrow().ok()
    }

    /// Re-reads a child frame's viewport (see [`PageDom::layout`]). A
    /// changed size after the first queues one `resize` task for the
    /// child's window, as Chrome fires it after the layout that resized the
    /// iframe.
    pub fn sync_frame_viewport(&self) {
        let Some(source) = self.frame_viewport.borrow().clone() else { return };
        let Some(viewport) = source() else { return };
        let old = match self.layout.try_borrow() {
            Ok(l) if *l.viewport() == viewport => return,
            Ok(l) => (l.viewport().width, l.viewport().height),
            Err(_) => return,
        };
        // A query already holding the layout keeps the old size; the next
        // one picks it up.
        let Ok(mut layout) = self.layout.try_borrow_mut() else { return };
        layout.set_viewport(viewport.clone());
        drop(layout);
        let resized = self.frame_sized.replace(true) && old != (viewport.width, viewport.height);
        if resized && !self.frame_resize_queued.replace(true) {
            let page = self.this.clone();
            self.post_task(Box::new(move || {
                if let Some(page) = page.upgrade() {
                    page.frame_resize_queued.set(false);
                    page.fire_frame_resize();
                }
            }));
        }
    }

    /// The child window's `resize` (and its visual viewport's), then the
    /// media query lists whose `matches` flipped.
    fn fire_frame_resize(self: &Rc<Self>) {
        if self.is_disposed() {
            return;
        }
        if let Some(window) = self.window_target() {
            let event =
                crate::event::EventCore::trusted(self, crate::event::EventInterface::Event, "resize", false, false);
            crate::target::fire(self, window, &event);
        }
        for target in self.viewport_targets(crate::window::ViewportTarget::VisualViewport) {
            let event =
                crate::event::EventCore::trusted(self, crate::event::EventInterface::Event, "resize", false, false);
            crate::target::fire(self, target, &event);
        }
        for (target, query, matches) in self.media_query_changes() {
            self.fire_media_query_change(target, &query, matches);
        }
    }

    /// After a task: a child frame whose window listens for events re-reads
    /// its iframe's box, so a parent restyle fires `resize` without a read
    /// in the child. At most every max(16 ms, twice the re-read's cost), as
    /// ResizeObserver gathers; top documents and windows without listeners
    /// return at once.
    fn frame_resize_after_task(self: &Rc<Self>) {
        if !self.frame_sized.get() {
            return;
        }
        let Some(window) = self.window_target() else { return };
        if !self.dom.try_borrow().is_ok_and(|d| d.has_listeners(window)) {
            return;
        }
        let now = std::time::Instant::now();
        if let Some(next) = self.frame_resize_next.get().filter(|&n| n > now) {
            if !self.frame_resize_wake.replace(true) {
                let page = self.this.clone();
                self.post_delayed_task(
                    next - now,
                    Box::new(move || {
                        if let Some(page) = page.upgrade() {
                            page.frame_resize_wake.set(false);
                            page.frame_resize_after_task();
                        }
                    }),
                );
            }
            return;
        }
        self.sync_frame_viewport();
        let cost = now.elapsed();
        self.frame_resize_next
            .set(Some(std::time::Instant::now() + (cost * 2).max(std::time::Duration::from_millis(16))));
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
            (ResourceKind::StyleText, _) if !self.sheet_models.borrow().get(&ev.node).is_some_and(|m| m.edited()) => {
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

    /// Lends the network parser for one script it reached: the script's
    /// `document.write`s parse at the insertion point.
    pub fn lend_network_parser(&self, mut parser: lp_dom::parse::DocumentParser) {
        parser.open_insertion_point();
        *self.network_parser.borrow_mut() = Some(parser);
        self.network_parser_lent.set(true);
        self.script_nesting.set(self.script_nesting.get() + 1);
    }

    /// Ends `lend_network_parser` after the script. Once the parser is back
    /// the loader blocks on written scripts itself (`take_written_scripts`).
    pub fn reclaim_network_parser(&self) -> Option<lp_dom::parse::DocumentParser> {
        self.script_nesting.set(self.script_nesting.get() - 1);
        self.network_parser_lent.set(false);
        self.written_pending.set(None);
        let mut parser = self.network_parser.borrow_mut().take()?;
        parser.close_insertion_point();
        Some(parser)
    }

    /// The written parser-blocking script tokenizing waits for.
    pub fn written_script_pending(&self) -> Option<NodeId> {
        self.written_pending.get()
    }

    /// External and module scripts reached in written markup, in order.
    pub fn take_written_scripts(&self) -> Vec<NodeId> {
        std::mem::take(&mut *self.written_scripts.borrow_mut())
    }

    /// Runs `f` as an external or module script: `document.write` without an
    /// insertion point is ignored meanwhile.
    pub fn with_destructive_writes_ignored<R>(&self, f: impl FnOnce() -> R) -> R {
        self.ignore_destructive_writes.set(self.ignore_destructive_writes.get() + 1);
        let result = f();
        self.ignore_destructive_writes.set(self.ignore_destructive_writes.get() - 1);
        result
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
    /// The live `visualViewport` (or `screen.orientation`) targets, one per
    /// realm that has used it: the embedder fires `resize` (`change`) on each.
    pub fn viewport_targets(&self, kind: crate::window::ViewportTarget) -> Vec<TargetKey> {
        self.viewport_targets.borrow().iter().filter(|(k, _)| *k == kind).map(|(_, t)| *t).collect()
    }

    /// Re-evaluates every live `MediaQueryList` against the current viewport:
    /// the targets whose `matches` flipped, with the query text and the new
    /// value, for the embedder to fire `change` at (`fire_media_query_change`).
    pub fn media_query_changes(&self) -> Vec<(TargetKey, String, bool)> {
        let layout = self.layout();
        let mut out = Vec::new();
        for entry in self.media_queries.borrow().iter() {
            let now = layout.style().match_media(&entry.query);
            if entry.last.replace(now) != now {
                out.push((entry.target, entry.query.clone(), now));
            }
        }
        out
    }

    /// Fires a trusted `change` (MediaQueryListEvent) at a `MediaQueryList`
    /// target from `media_query_changes`. Call outside DOM/layout borrows.
    pub fn fire_media_query_change(self: &Rc<Self>, target: TargetKey, query: &str, matches: bool) {
        let event = crate::event::EventCore::trusted(
            self,
            crate::event::EventInterface::MediaQueryListEvent,
            "change",
            false,
            false,
        );
        {
            let mut fields = event.fields.borrow_mut();
            fields.media_query = query.to_owned();
            fields.media_matches = matches;
        }
        crate::target::fire(self, target, &event);
    }

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

    /// A scheduled resize gather or callback can still change layout.
    pub fn pending_resize_observer_work(&self) -> bool {
        self.resizes.pending_work()
    }

    /// Request a current gather for input actionability without bypassing
    /// the observer callback task or changing ordinary background pacing.
    pub fn request_resize_observer_update_for_input(&self) {
        crate::resize_observer::request_update_for_input(self);
    }

    /// Failure-only input diagnostics: observers present, callback queued,
    /// paced gather queued, and time until its pacing deadline.
    pub fn resize_observer_work_state(&self) -> (bool, bool, bool, Option<std::time::Duration>) {
        self.resizes.pending_work_state()
    }

    /// Wrappers of active ResizeObservers kept alive (diagnostics/tests).
    pub fn resize_observer_pins(&self) -> usize {
        self.resizes.pins.borrow().len()
    }

    /// Safe point (after each task): applies deferred releases, unpins
    /// listener-holding wrappers of disconnected nodes, re-pins reconnected
    /// ones, and frees unreachable DOM nodes.
    pub fn after_task(self: &Rc<Self>) {
        if self.quiet_since_last_task() {
            return;
        }
        self.flush_iframe_events();
        crate::message_channel::reconcile_ports(self);
        crate::mutation_observer::after_task(self);
        crate::intersection_observer::after_task(self);
        crate::resize_observer::after_task(self);
        self.frame_resize_after_task();
        crate::layout_api::schedule_scroll_clamp(self);
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

    /// True when no step of `after_task` can have work: the DOM is unchanged
    /// since the last task (version covers tree, focus and hover), nothing is
    /// queued for release, reconciliation or observation, and no layout
    /// observer is registered (those deliver initial entries without a DOM
    /// change). Runs after every CDP command, so this is the common case.
    pub fn quiet_since_last_task(&self) -> bool {
        if self.is_disposed() {
            return false;
        }
        let Ok(dom) = self.dom.try_borrow() else { return false };
        let stamp = (dom.version(), dom.removal_count());
        if self.after_task_stamp.get() != Some(stamp) {
            self.after_task_stamp.set(Some(stamp));
            return false;
        }
        !dom.has_iframe_events()
            && !dom.has_pending_mutations()
            && !dom.has_gc_candidates()
            && self.mutations.pending_drops_is_empty()
            && self.pending_release.borrow().is_empty()
            && self.pending_port_reconcile.borrow().is_empty()
            && self.new_property_pins.borrow().is_empty()
            && !self.intersections.has_observers()
            && !self.resizes.has_observers()
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
    pub fn main_world_realm(&self) -> Option<lp_js::Realm> {
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
    pub(crate) fn post_delayed_task(&self, delay: std::time::Duration, task: Box<dyn FnOnce()>) {
        match self.hooks() {
            Some(h) => h.post_delayed_task(delay, task),
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
    pub(crate) fn popup_command(&self, popup: Rc<crate::popup::PopupState>, command: crate::popup::PopupCommand) {
        if let Some(h) = self.hooks() {
            h.popup_command(popup, command);
        }
    }
    pub(crate) fn close_window(&self) {
        if let Some(h) = self.hooks() {
            h.close_window();
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
    pub(crate) fn sibling_realms(&self, s: &lp_js::Scope<'_>) -> Vec<lp_js::Realm> {
        self.hooks().map(|h| h.sibling_realms(s)).unwrap_or_default()
    }
    /// The wrapper of `id` in a sibling realm of `s`, if one is alive.
    pub(crate) fn sibling_wrapper<'s>(&self, s: &lp_js::Scope<'s>, id: NodeId) -> Option<lp_js::Object<'s>> {
        self.sibling_realms(s).iter().find_map(|realm| {
            let scope = realm.scope().ok()?;
            let owner = crate::target::page_of(&scope).ok()?;
            let wrapper = scope.lookup_identity(owner.node_identity(id))?;
            lp_js::Persistent::new(&scope, wrapper.value()).get(s).as_object()
        })
    }
    /// A realm that may hold listeners of a target of this page: its own, or a
    /// sibling document's when the target's wrapper was made there.
    pub(crate) fn listener_realm(&self, id: lp_js::RealmId) -> Option<lp_js::Realm> {
        if self.is_disposed() {
            return None;
        }
        self.realms.borrow().get(&id).and_then(|w| w.upgrade()).or_else(|| lp_js::Realm::by_id(id))
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

    pub(crate) fn nonce(&self, el: NodeId, attr: Option<&str>) -> Option<String> {
        let nonces = self.nonces.borrow();
        nonces.get(&el).filter(|(over, _)| over.as_deref() == attr).map(|(_, value)| value.clone())
    }
    pub(crate) fn set_nonce(&self, el: NodeId, attr: Option<&str>, value: String) {
        self.nonces.borrow_mut().insert(el, (attr.map(str::to_owned), value));
    }

    /// A `<style>` element's sheet is read from the DOM again, after CSSOM edits.
    pub(crate) fn set_style_element_sheet(&self, owner: NodeId) {
        self.layout.borrow_mut().style_mut().add_sheet(owner, lp_dom::style::SheetSource::StyleElement);
    }

    /// `document.title`: the first `<title>`'s text, whitespace collapsed.
    pub fn document_title(&self) -> String {
        let dom = self.dom.borrow();
        let Ok(sel) = lp_dom::select::parse_selectors("title") else { return String::new() };
        let title = dom.query_selector(self.document(), &sel).ok().flatten();
        let text = title.and_then(|t| dom.text_content(t).ok().flatten()).unwrap_or_default();
        text.split_ascii_whitespace().collect::<Vec<_>>().join(" ")
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

    /// The document's URL, for a layout engine that loads sub-resources
    /// (`LIGHTNINGPANDA_GEOMETRY=blitz`).
    pub fn set_base_url(&self, url: &str) {
        self.layout.borrow().set_base_url(url);
    }

    /// Borrow helper for bindings.
    pub fn with_dom<R>(&self, f: impl FnOnce(&mut Dom) -> R) -> R {
        f(&mut self.dom.borrow_mut())
    }
}
