//! Browser/page orchestration on the browser thread.
//!
//! Ownership (see docs/ARCHITECTURE.md): this crate owns the Browser →
//! Page → Frame tree, navigation state machine, realms per (frame, world),
//! init-script injection, window timers, and the ordered [`PageEvent`] stream
//! consumed by the automation layer. Document loading uses `pyro-dom`/`pyro-net`;
//! `pyro-webapi` supplies bindings in the main and isolated document worlds.

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
mod responsive_frames;
mod speculate;
mod stylesheets;
mod viewport;
mod window;

use pyro_js::{Engine, EngineHooks, JsError, Realm, RealmId, RealmOptions};
use pyro_runtime::{EventLoop, LoopHandle, LoopState, RunOutcome, Scheduler};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

pub use frames::{realm_page, FrameInfo};
pub use images::LazyStarted;
use frames::{FrameHost, FrameKey};
pub use loader::{
    FetchDone, FetchRequest, NetLoader, Resource, ResourceCancellation, ResourceKind, ResourceLoader, StaticLoader,
};
pub use nav::{set_navigation_rate_limit, History, HistoryEntry, Location, PageNav, Performance};
pub use network_idle::NetworkIdle;
pub use quiet::{PageWork, WorkStamp, TIMER_HORIZON};
pub use window::{note_client_activity, request_native_frame, Window, WindowData};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PageId(pub u32);

/// A browser context (CDP `browserContextId`): pages sharing network state
/// (own `pyro_net::NetClient`, hence cookie jar). Context 0 is the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ContextId(pub u32);

pub const DEFAULT_CONTEXT: ContextId = ContextId(0);

/// Network settings a browser context has of its own instead of the
/// browser's ([`Browser::create_context_with`]).
#[derive(Debug, Clone, Default)]
pub struct ContextNet {
    /// Its proxy (`Some(None)`: none), `Target.createBrowserContext proxyServer`.
    pub proxy: Option<Option<pyro_net::ProxyConfig>>,
    /// The `User-Agent` of its requests and its pages' `navigator.userAgent`
    /// (a page's `Emulation.setUserAgentOverride` still takes its place).
    pub user_agent: Option<String>,
}

struct ContextState {
    id: ContextId,
    loader: Option<Rc<dyn ResourceLoader>>,
    net: Option<pyro_net::NetClient>,
    /// The bodies its pages' layouts loaded, shared with the contexts that
    /// go out with the same network identity ([`Browser::layout_bodies_for`]).
    layout_bodies: std::sync::Arc<pyro_dom::layout::SubresourceBodies>,
    local_storage: pyro_webapi::StorageNamespace,
    /// Permissions and clipboard its pages share.
    services: Rc<pyro_webapi::ContextServices>,
    init_scripts: Vec<InitScript>,
}

/// A request for a sub-resource of `frame`'s document that the engine needs
/// itself (a renderer's or layout's image, font or imported sheet): the
/// document's frame and loader, its URL as referrer.
/// With `cors`, in CORS mode with credentials only to the same origin (`crossorigin="anonymous"`).
fn subresource_request(frame: &Frame, page: PageId, url: String, kind: ResourceKind, cors: bool) -> FetchRequest {
    let options = SubresourceOptions { crossorigin: cors.then(|| "anonymous".to_owned()), ..Default::default() };
    subresource_request_with(frame, page, url, kind, &options)
}

/// pyro: the client id of `frame`'s current document (`SubresourceOptions::client`).
pub fn document_client(frame: &Frame) -> String {
    format!("{}#{}", frame.id.0, frame.loader_id.0)
}

/// pyro: the layout body key of `request` as `frame`'s document sends it (the ONE place a page
/// writer's key is built: from the request actually sent, destination included): its default
/// referrer (the document's) and policy count as the defaults layout sends.
pub(crate) fn layout_key_of(frame: &Frame, request: &FetchRequest, integrity: Option<&str>) -> pyro_dom::layout::LayoutBodyKey {
    // (`integrity`: the integrity metadata the request was checked against, part of it.)
    let mut options = pyro_dom::layout::SubresourceOptions {
        crossorigin: request.crossorigin.clone(),
        integrity: integrity.map(str::to_owned),
        referrer: request.referrer.clone().filter(|r| *r != frame.base_url),
        referrer_policy: request.referrer_policy.map(|p| p.as_str().to_owned()),
    };
    let destination = match request.kind {
        ResourceKind::Stylesheet => pyro_dom::layout::SubresourceKind::Style,
        ResourceKind::Font => pyro_dom::layout::SubresourceKind::Font,
        _ => {
            options.referrer.clone_from(&request.referrer);
            options.referrer_policy = Some(request.referrer_policy.or_else(|| frame.page_dom.as_ref().map(|dom| dom.referrer_policy())).unwrap_or_default().as_str().to_owned());
            pyro_dom::layout::SubresourceKind::Image
        }
    };
    pyro_dom::layout::LayoutBodyKey::new(&document_client(frame), &request.url, &options, destination)
}

/// pyro: what a capture knows of one document's images ([`Browser::document_images`]).
#[derive(Clone, Debug, Default)]
pub struct DocumentImages {
    /// The URLs of its list of available images.
    pub available: std::collections::HashSet<String>,
    /// The URLs its elements requested as images (`<img>`, SVG `<image>`, `<object>`).
    pub requested: std::collections::HashSet<String>,
    /// It had fired `load`.
    pub loaded: bool,
}

/// pyro: what a renderer's sub-resource request carries beyond its URL, as a CSS `url()`'s
/// request URL modifiers ask (CSS Values 5 § 4.5.4). The defaults are a sub-resource's: no-cors,
/// no integrity metadata, the document as referrer with its policy.
#[derive(Clone, Debug, Default)]
pub struct SubresourceOptions {
    /// `anonymous` or `use-credentials`: the request's mode is "cors", with those credentials.
    pub crossorigin: Option<String>,
    /// Integrity metadata: a response whose body matches none of it fails, as does a
    /// cross-origin one fetched without CORS.
    pub integrity: Option<String>,
    /// The request's referrer (the style sheet a `url()` was written in); `None`: the document.
    pub referrer: Option<String>,
    /// The request's referrer policy; `None`: the document's.
    pub referrer_policy: Option<pyro_net::ReferrerPolicy>,
    /// The document the request is made for, as `"<frame id>#<loader id>"` (`None`: the page's
    /// current document): the request is that frame's, with its origin (opaque in a sandboxed
    /// frame), and only while that document is the frame's (a request of a document since
    /// navigated away from is not sent).
    pub client: Option<String>,
}

fn subresource_request_with(
    frame: &Frame,
    page: PageId,
    url: String,
    kind: ResourceKind,
    options: &SubresourceOptions,
) -> FetchRequest {
    FetchRequest {
        parser_priority: false,
        credentials: None,
        post: None,
        url,
        kind,
        page,
        referrer: Some(options.referrer.clone().unwrap_or_else(|| frame.base_url.clone())),
        referrer_policy: options.referrer_policy,
        frame_id: Some(frame.id.0.clone()),
        loader_id: Some(frame.loader_id.0.clone()),
        crossorigin: options.crossorigin.clone(),
        initiator: None,
        module: None,
        part: None,
    }
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
    /// The policy the request is sent with (its initiator's: a link's or
    /// frame's `referrerpolicy`, `rel=noreferrer`, the document that
    /// navigates another frame); `None`: the navigated frame's current
    /// document's.
    pub referrer_policy: Option<pyro_net::ReferrerPolicy>,
}

impl NavigateOpts {
    pub fn address_bar(url: impl Into<String>) -> Self {
        NavigateOpts {
            post: None,
            url: url.into(),
            reason: NavigationReason::AddressBar,
            cdp_request_id: None,
            referrer: None,
            referrer_policy: None,
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
        /// The committed document is the error document of a navigation
        /// to this URL ([`ERROR_DOCUMENT_URL`]; CDP `Frame.unreachableUrl`).
        unreachable_url: Option<String>,
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
        /// An error document for the failure commits next, under the same
        /// loader (`FrameNavigated` with `unreachable_url`): loading goes on.
        error_page: bool,
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
        input: pyro_dom::NodeId,
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
        ticket: Option<std::rc::Rc<pyro_webapi::PopupTicket>>,
        window_features: pyro_webapi::window_features::WindowFeatures,
        window_name: String,
        user_gesture: bool,
        referrer: Option<String>,
        referrer_policy: Option<pyro_net::ReferrerPolicy>,
    },
    /// The object `window.open` returned asked something of its popup. Comes
    /// after that popup's `PopupRequested`.
    PopupCommand {
        popup: std::rc::Rc<pyro_webapi::PopupState>,
        command: pyro_webapi::PopupCommand,
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

    /// Transport progress of fetch/XHR or cached resource `request` became visible to
    /// the script of the document loaded by `loader`, before any script it
    /// causes runs (`pyro_webapi::PageHooks::network_delivery`). `frontier` is
    /// cumulative for the request. A zero frontier records the request announcement
    /// without publishing its response. Request ids are per browser context (each
    /// context has its own `pyro_net::NetClient`), so `context` is part of the
    /// identity; `loader` tells an old document's requests from its
    /// successor's. Called synchronously on the browser thread: implementations
    /// may only record or publish, never run script or re-enter the browser.
    fn on_network_delivery(
        &self,
        _page: PageId,
        _context: ContextId,
        _loader: &LoaderId,
        _request: pyro_net::RequestId,
        _frontier: pyro_net::NetworkDelivery,
    ) {
    }

    /// A document of `page` started or ended a dedicated worker. May be called
    /// with script on the stack or while the page is being disposed: only record.
    fn on_worker_event(&self, _page: PageId, _event: pyro_webapi::WorkerEvent) {}

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
    ) -> Option<pyro_webapi::DialogAnswer> {
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
    pub owner: Option<pyro_dom::NodeId>,
    pub name: String,
    pub base_url: String,
    pub(crate) sandbox: frames::Sandbox,
    /// The container's policy for `fullscreen` when the frame's navigation began (HTML takes the
    /// container policy then; a later attribute change applies to the next navigation):
    /// whether it had `allowfullscreen`, and its `allow` attribute. Top-level frames: none.
    pub(crate) fullscreen_container: frames::FullscreenContainer,
    pub url: String,
    pub loader_id: LoaderId,
    pub realms: Vec<FrameRealm>,
    /// DOM store of the page (shared by its documents) and this frame's document.
    pub dom: Option<pyro_dom::parse::DomRef>,
    pub document: Option<pyro_dom::NodeId>,
    /// Binding state shared by this document's main and isolated worlds.
    pub page_dom: Option<Rc<pyro_webapi::PageDom>>,
    /// Network quiet milestones the current document has reported.
    pub network_idle: NetworkIdle,
    pub origin: String,
    pub(crate) opaque_origin: u64,
    pub dom_content_loaded: bool,
    pub loaded: bool,
    /// The frame shows the error document of a navigation that failed
    /// ([`ERROR_DOCUMENT_URL`]): the URL it could not reach, and why.
    pub unreachable: Option<NavigationError>,
}

/// URL of the document a new window shows when its navigation fails on the
/// network, as in Chrome (its `Frame.unreachableUrl` is the URL that failed).
pub const ERROR_DOCUMENT_URL: &str = "chrome-error://chromewebdata/";

/// A navigation that failed with a network error, which a frame's error
/// document stands for ([`Frame::unreachable`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavigationError {
    /// The URL the navigation could not reach.
    pub url: String,
    /// The network error (`net::ERR_CONNECTION_REFUSED`, ...).
    pub error: String,
}

impl Frame {
    /// The document providing the navigation's client referrer; a srcdoc
    /// document obtains it from its nearest non-srcdoc container document.
    pub(crate) fn navigation_referrer(&self) -> Option<String> {
        self.page_dom.as_ref()?.client_referrer().map(Into::into)
    }

    /// Field-wise equality by identity (realm ids, shared `Rc`s), without cloning.
    pub(crate) fn same_as(&self, other: &Frame) -> bool {
        self.id == other.id
            && self.parent == other.parent
            && self.owner == other.owner
            && self.name == other.name
            && self.base_url == other.base_url
            && self.sandbox == other.sandbox
            && self.fullscreen_container == other.fullscreen_container
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
            && self.unreachable == other.unreachable
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
    /// Container attribute processing queued before its navigation has a loader.
    pub(crate) pending_frame_attributes: usize,
    pub(crate) pending_cancel: Option<ResourceCancellation>,
    /// The loader and URL of `pending_loader` when the frame's scripts
    /// started it, which `document.open()` stops (HTML "stop loading").
    pub(crate) pending_script: Option<(LoaderId, String)>,
    pub(crate) load: Option<loader::DocumentLoad>,
    /// HTML "list of scripts that will execute in order as soon as possible".
    pub(crate) in_order_scripts: loader::InOrderScripts,
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
    session_storage: pyro_webapi::StorageNamespace,
    /// Set for a `window.open` popup: the opener's object reaches this
    /// page's documents through it, and dropping it tells the opener the
    /// popup closed.
    pub(crate) popup: Option<Rc<pyro_webapi::PopupTicket>>,
    pub(crate) script_closable: bool,
    /// A popup's about:blank documents take the opener's origin and base
    /// URL (origin, opaque-origin token, base URL), as a child frame's do
    /// from its parent.
    opener_origin: Option<(String, u64, String)>,
    user_agent_override: Option<pyro_webapi::UserAgentOverride>,
    /// `navigator.maxTouchPoints` of its documents (touch emulation).
    max_touch_points: u32,
    /// `Emulation.setScriptExecutionDisabled`: documents committed from now
    /// on, and their frames, run no script of their own.
    script_execution_disabled: bool,
    /// `Page.setBypassCSP`: documents committed from now on, in all its
    /// frames, take no Content-Security-Policy.
    bypass_csp: bool,
    extra_http_headers: pyro_net::http::HeaderMap,
    cache_disabled: bool,
    bypass_service_worker: bool,
    request_defaults: pyro_net::RequestDefaults,
    viewport: pyro_dom::media::Viewport,
    /// Unemulated viewport selected when this desktop window was created.
    native_viewport: pyro_dom::media::Viewport,
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
    /// pyro: the content sizes of the responsively embedded documents its iframes show (css-sizing-4
    /// responsive iframes: a `<meta name="responsive-embedded-sizing">` before the body), handed to the
    /// render snapshot (`Dom::set_frame_content_size`): an iframe of `frame-sizing: content-height` is
    /// as tall as its document's content. Each document updates its own iframes
    /// (`PageDom::sync_responsive_frames`, which script reads of its layout run too). Call before
    /// taking a render snapshot.
    pub fn update_responsive_frames(&self) {
        if !self.frames.has_children() {
            return;
        }
        // Innermost documents first: a responsively embedded document's own frames size it.
        let depth = |mut key: frames::FrameKey| {
            let mut depth = 0;
            while key.child.is_some() && depth < 64 {
                key = self.frames.parent_key(key);
                depth += 1;
            }
            depth
        };
        let mut keys: Vec<(usize, frames::FrameKey)> = self.frames.keys().into_iter().map(|k| (depth(k), k)).collect();
        keys.sort_by(|a, b| b.0.cmp(&a.0));
        for (_, key) in keys {
            if let Some(page_dom) = self.frames.page_dom(key) {
                page_dom.sync_responsive_frames();
            }
        }
    }

    /// The documents of this page's frames (the store may hold other pages' too).
    fn frame_tree_documents(&self) -> Vec<pyro_dom::NodeId> {
        let mut docs: Vec<pyro_dom::NodeId> =
            self.frames.keys().into_iter().filter_map(|k| self.frames.with_frame(k, |f| f.document).flatten()).collect();
        if let Some(d) = self.main_frame.page_dom.as_ref().map(|p| p.document()) {
            if !docs.contains(&d) {
                docs.push(d);
            }
        }
        docs
    }

    /// Gives each iframe of the page the document it shows as the renderer paints it (its render
    /// HTML, URL and scroll position; see [`pyro_dom::FrameDocument`]), innermost frames first so a
    /// frame's HTML holds its own frames' documents. pyro: each frame document carries its own
    /// sheets ([`pyro_dom::FrameDocument::sheets`]), never merged with the page's by URL.
    pub fn update_frame_documents(&self) -> Vec<pyro_dom::geometry::TransitionFrame> {
        let mut transitions = Vec::new();
        if !self.frames.has_children() {
            if let Some(page_dom) = &self.main_frame.page_dom {
                if let Ok(mut dom) = page_dom.dom.try_borrow_mut() {
                    dom.set_frame_documents(&self.frame_tree_documents(), Vec::new());
                    dom.frame_animations.clear();
                }
            }
            return transitions;
        }
        let depth = |mut key: frames::FrameKey| {
            let mut depth = 0;
            while key.child.is_some() && depth < 64 {
                key = self.frames.parent_key(key);
                depth += 1;
            }
            depth
        };
        let mut keys: Vec<(usize, frames::FrameKey)> =
            self.frames.keys().into_iter().filter(|k| k.child.is_some()).map(|k| (depth(k), k)).collect();
        keys.sort_by(|a, b| b.0.cmp(&a.0));
        let mut documents = Vec::new();
        let mut frame_animations = Vec::new();
        for (_, key) in keys {
            let Some((owner, page_dom, base_url, url, client)) = self.frames.with_frame(key, |f| {
                (f.owner, f.page_dom.clone(), f.base_url.clone(), f.url.clone(), document_client(f))
            })
            else {
                continue;
            };
            let (Some(owner), Some(page_dom)) = (owner, page_dom) else { continue };
            // Each frame's HTML is taken after its own frames' documents are set (deeper first).
            // (Its animated elements keyed, its animations kept for the renderer.)
            // pyro: at the frame document's own base (its `<base href>`), as the top document's
            // capture takes it (`render_base_url`).
            let frame_base = if base_url.is_empty() { &url } else { &base_url };
            let base = page_dom.render_base_url(frame_base);
            // pyro: with its own frames' table (set before, deeper first), which its markup names.
            let frames = page_dom.dom.try_borrow().map(|dom| dom.frame_table(page_dom.document())).unwrap_or_default();
            let start = transitions.len();
            page_dom.transition_frames(&frames.nonce, &mut transitions);
            let (snapshot, animations) = page_dom.render_snapshot_frame(&base, &transitions[start..]);
            let document = pyro_dom::FrameDocument {
                html: snapshot.html,
                document_url: (url != base).then_some(url),
                url: base,
                scroll: (snapshot.scroll_x, snapshot.scroll_y),
                client,
                frames,
                // pyro: its own sheets, with it (never merged into the page's by URL).
                sheets: snapshot.sheets.into_iter().collect(),
            };
            if let Ok(mut dom) = page_dom.dom.try_borrow_mut() {
                dom.set_frame_document(owner, Some(document.clone()));
            }
            frame_animations.push((document.client.clone(), animations));
            documents.push((owner, document));
        }
        let store = self.main_frame.page_dom.as_ref().map(|p| p.dom.clone());
        if let Some(store) = store {
            if let Ok(mut dom) = store.try_borrow_mut() {
                dom.set_frame_documents(&self.frame_tree_documents(), documents);
                dom.frame_animations = frame_animations
                    .into_iter()
                    .filter_map(|(client, frame)| Some((client, std::sync::Arc::new(frame?))))
                    .collect();
            }
        }
        transitions
    }


    /// Copies the page's own request options (extra headers, cache bypass)
    /// into `defaults`, e.g. those derived from a user agent override.
    fn apply_request_options(&self, defaults: &mut pyro_net::RequestDefaults) {
        defaults.extra_headers = self.extra_http_headers.clone();
        defaults.cache_disabled = self.cache_disabled;
    }
}

/// Browser-thread state: the loop state type for [`EventLoop`].
pub struct Browser {
    /// Dropped (or handed to [`BrowserThread::return_engine`]'s slot) by
    /// this struct's `Drop`, before every other field as when it was a plain
    /// first field.
    pub engine: std::mem::ManuallyDrop<Engine>,
    /// Where the engine goes when this browser is dropped, instead of being
    /// dropped with it.
    engine_return: Option<EngineSlot>,
    sched: Scheduler<Browser>,
    pub pages: Vec<Page>,
    /// Shared with every page's [`PageNav`], which reports same-document
    /// commits synchronously (see [`PageObserver`]).
    observer: ObserverCell,
    next_id: Rc<Cell<u64>>,
    started: Instant,
    /// pyro: the layout body stores of this browser's network identities (`layout_bodies_for`):
    /// this browser's own, never shared with another browser of the process, whose document ids
    /// (frame and loader counters) may be the same.
    layout_stores: Vec<((Option<pyro_net::ProxyConfig>, String), std::sync::Arc<pyro_dom::layout::SubresourceBodies>)>,
    /// Finalized-wrapper payloads released at checkpoints (diagnostics).
    pub released_payloads: RefCell<u64>,
    /// Cross-thread handle to this browser's own loop (network completions).
    handle: LoopHandle<Browser>,
    /// Loader for contexts without their own (tests, about:blank-only setups).
    pub(crate) loader: Option<Rc<dyn ResourceLoader>>,
    contexts: Vec<ContextState>,
    /// When set, every new context gets its own NetClient with this config.
    net_config: Option<pyro_net::NetConfig>,
    /// `Security.setIgnoreCertificateErrors` sent browser-wide: applies to
    /// contexts created later too.
    ignore_certificate_errors: bool,
    /// Entries every context's local storage starts with.
    local_storage_seed: pyro_webapi::StorageSeed,
    /// Completions of in-flight fetches, keyed by completion id.
    pub(crate) completions: std::collections::HashMap<u64, FetchDone>,
    ready_responses: loader::ReadyResponses,
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
        // The task is over for every page, idle ones too (skipped below): their document
        // timelines read the clock again in the next task.
        for page in &self.pages {
            if let Some(dom) = page.main_frame.page_dom.as_ref() {
                dom.end_task_time();
            }
        }
        let pages: Vec<_> = self.pages.iter().filter(|p| !Self::page_idle(p)).map(|p| p.id).collect();
        for id in pages {
            let Some(host) = self.page(id).map(|p| p.frames.clone()) else { continue };
            self.sync_root(id);
            // Resource events are queued per page; one drain covers every frame.
            self.drain_dom_resources(id.into());
            for key in host.keys() {
                if let Some(bindings) = host.page_dom(key) {
                    bindings.after_task();
                }
                if key.child.is_some() {
                    self.apply_same_document_url(key);
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
/// the JavaScriptCore VM exists (macOS; see `pyro_runtime` docs). Diagnostic-
/// backed initial policy: up to 4 idle wakes/s per browser thread.
#[cfg(target_os = "macos")]
const JSC_RUNLOOP_SERVICE_MAX_IDLE: std::time::Duration = std::time::Duration::from_millis(250);

/// Where a dropped [`Browser`] leaves its engine ([`BrowserThread::return_engine`]).
pub type EngineSlot = Rc<RefCell<Option<Engine>>>;

/// What [`Engine::heap_counts`] read on the first engine a browser thread of
/// this process made, right after its classes were registered: what
/// [`Engine::recycle`] compares a used engine with. Every browser thread
/// builds its engine the same way, so one reading stands for all.
static CLEAN_HEAP: std::sync::OnceLock<Option<pyro_js::HeapCounts>> = std::sync::OnceLock::new();

/// The heap of an engine [`BrowserThread::new`] just made; `None` before
/// the first one, and when the JavaScriptCore in use cannot tell.
pub fn clean_heap() -> Option<pyro_js::HeapCounts> {
    CLEAN_HEAP.get().copied().flatten()
}

/// Returns this thread's browser state to what a new thread has, after its
/// [`BrowserThread`] was dropped and before another is made on it: the page
/// clock (virtual time, a frozen clock) and the eager animation frames.
/// `Err` names state that is not this function's to clear and is still there.
pub fn reset_thread() -> Result<(), &'static str> {
    if frames::has_spare_realm() {
        return Err("a spare realm is still held");
    }
    clock::reset();
    window::reset_eager_frames();
    Ok(())
}

/// A browser thread's loop plus its state.
pub struct BrowserThread {
    pub event_loop: EventLoop<Browser>,
    pub browser: Browser,
}

impl BrowserThread {
    /// Creates the engine for the current thread (one per thread).
    pub fn new() -> Result<BrowserThread, JsError> {
        Self::with_engine(None)
    }

    /// [`BrowserThread::new`] around `engine` when given: an engine an
    /// earlier `BrowserThread` of this thread made and left
    /// ([`BrowserThread::return_engine`]), which [`Engine::recycle`] has
    /// accepted since. Its classes are registered already.
    pub fn with_engine(engine: Option<Engine>) -> Result<BrowserThread, JsError> {
        let engine = match engine {
            Some(engine) => engine,
            None => {
                let engine = Self::new_engine()?;
                CLEAN_HEAP.get_or_init(|| engine.heap_counts());
                engine
            }
        };
        Ok(Self::around(engine))
    }

    fn new_engine() -> Result<Engine, JsError> {
        let engine = Engine::new()?;
        pyro_webapi::register(&engine);
        engine.register::<frames::CrossWindow>();
        engine.register::<frames::CrossLocation>();
        engine.register::<Window>();
        engine.register::<Location>();
        engine.register::<History>();
        engine.register::<Performance>();
        engine.register::<pyro_webapi::performance::PerformanceEntry>();
        engine.register::<pyro_webapi::performance::PerformancePaintTiming>();
        engine.register::<pyro_webapi::performance::PerformanceMark>();
        engine.register::<pyro_webapi::performance::PerformanceMeasure>();
        engine.register::<pyro_webapi::performance::PerformanceResourceTiming>();
        engine.register::<pyro_webapi::server_timing::PerformanceServerTiming>();
        engine.register::<pyro_webapi::performance::PerformanceNavigationTiming>();
        engine.register::<nav::PerformanceTiming>();
        engine.register::<nav::PerformanceNavigation>();
        Ok(engine)
    }

    fn around(engine: Engine) -> BrowserThread {
        let event_loop = EventLoop::new();
        // The system JavaScriptCore binds this VM's timers to this thread's
        // run loop; the loop services it at a bounded interval for the VM's
        // whole lifetime (closed-page garbage lives in the VM, not in open
        // pages). The source build's timers use WTF's run loop instead, and
        // a fork child must not touch CoreFoundation's run loop at all.
        #[cfg(target_os = "macos")]
        let event_loop = {
            let mut event_loop = event_loop;
            if !pyro_js::sys::SOURCE_BUILD {
                event_loop.enable_native_runloop_service(JSC_RUNLOOP_SERVICE_MAX_IDLE);
            }
            event_loop
        };
        let browser = Browser {
            engine: std::mem::ManuallyDrop::new(engine),
            engine_return: None,
            sched: event_loop.scheduler(),
            pages: Vec::new(),
            observer: Rc::new(RefCell::new(None)),
            next_id: Rc::new(Cell::new(1)),
            started: Instant::now(),
            layout_stores: Vec::new(),
            released_payloads: RefCell::new(0),
            handle: event_loop.handle(),
            loader: None,
            contexts: vec![ContextState {
                id: DEFAULT_CONTEXT,
                loader: None,
                net: None,
                layout_bodies: Default::default(),
                local_storage: Default::default(),
                services: Default::default(),
                init_scripts: Vec::new(),
            }],
            net_config: None,
            ignore_certificate_errors: false,
            local_storage_seed: Default::default(),
            completions: std::collections::HashMap::new(),
            ready_responses: Default::default(),
        };
        // The VM's deferred work (WebAssembly.compile/instantiate settling, finished on a
        // compilation thread) runs in a task of this loop: JavaScriptCore would run it from a WTF
        // run loop nothing runs here. One task per wake while none is queued.
        let handle = event_loop.handle();
        let queued = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        browser.engine.set_deferred_work_waker(move || {
            if queued.swap(true, std::sync::atomic::Ordering::AcqRel) {
                return;
            }
            let queued = queued.clone();
            let _ = handle.post(move |b: &mut Browser| {
                queued.store(false, std::sync::atomic::Ordering::Release);
                b.engine.run_deferred_work();
            });
        });
        BrowserThread { event_loop, browser }
    }

    /// When this thread's browser is dropped, its engine is left in `slot`
    /// instead of being destroyed. The taker calls [`Engine::recycle`] once
    /// everything else of the thread is gone, and drops an engine it refuses.
    pub fn return_engine(&mut self, slot: EngineSlot) {
        self.browser.engine_return = Some(slot);
    }

    pub fn handle(&self) -> LoopHandle<Browser> {
        self.event_loop.handle()
    }

    /// Enables real document loading: every browser context (including the
    /// default one) gets its own pyro-net client (I/O thread, cookie jar).
    pub fn enable_network(&mut self, config: pyro_net::NetConfig) -> std::io::Result<()> {
        self.browser.net_config = Some(config);
        self.browser.attach_network(DEFAULT_CONTEXT)
    }

    /// Preloads local storage: every browser context (the existing ones and
    /// those created later) gets `seed`'s entries. Existing areas keep their
    /// other keys; no `storage` events fire.
    pub fn set_local_storage_seed(&mut self, seed: pyro_webapi::StorageSeed) {
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
                        // A deferred realm's setup reads the clock's state when it is made.
                        for realm in frame.realms.iter().filter(|r| !r.realm.is_deferred()) {
                            realm.realm.scope()?.eval(
                                clock::DATE_SCRIPT,
                                Some("pyro://internal/clock.js"),
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
    /// now, by which timers are due ([`pyro_runtime::EventLoop::run_timer_due_by`]).
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
        self.create_context_inner(ContextNet::default())
    }

    /// `create_context` with its own proxy setting (`None`: no proxy) instead
    /// of the browser's.
    pub fn create_context_with_proxy(&mut self, proxy: Option<pyro_net::ProxyConfig>) -> std::io::Result<ContextId> {
        self.create_context_inner(ContextNet { proxy: Some(proxy), user_agent: None })
    }

    /// `create_context` with network settings of its own instead of the
    /// browser's (see [`ContextNet`]).
    pub fn create_context_with(&mut self, net: ContextNet) -> std::io::Result<ContextId> {
        self.create_context_inner(net)
    }

    fn create_context_inner(&mut self, net: ContextNet) -> std::io::Result<ContextId> {
        let id = ContextId(self.next() as u32);
        let local_storage = pyro_webapi::StorageNamespace::seeded(&self.local_storage_seed);
        self.contexts.push(ContextState {
            id,
            loader: None,
            net: None,
            layout_bodies: Default::default(),
            local_storage,
            services: Default::default(),
            init_scripts: Vec::new(),
        });
        if self.net_config.is_some() {
            self.attach_network_with(id, net)?;
        }
        Ok(id)
    }

    fn attach_network(&mut self, id: ContextId) -> std::io::Result<()> {
        self.attach_network_with(id, ContextNet::default())
    }

    fn attach_network_with(&mut self, id: ContextId, net: ContextNet) -> std::io::Result<()> {
        let mut config = self.net_config.clone().unwrap_or_default();
        let own_identity = net.proxy.is_some() || net.user_agent.is_some();
        if let Some(proxy) = net.proxy {
            config.proxy = proxy;
        }
        if let Some(user_agent) = net.user_agent {
            config.user_agent = user_agent;
        }
        if own_identity {
            // The opt-in cross-context asset cache would answer its requests
            // with what other identities fetched, through their proxies.
            config.shared_asset_cache = None;
        }
        let layout_bodies = self.layout_bodies_for(own_identity, &config);
        let client = pyro_net::NetClient::new(config)?;
        client.set_ignore_certificate_errors(self.ignore_certificate_errors);
        if let Some(c) = self.contexts.iter_mut().find(|c| c.id == id) {
            c.loader = Some(Rc::new(NetLoader::new(client.clone())));
            c.net = Some(client);
            c.layout_bodies = layout_bodies;
        }
        Ok(())
    }

    /// The layout bodies of a context going out with `config` (see
    /// `pyro_dom::layout::SubresourceBodies`). One with a proxy or user agent
    /// of its own (`own_identity`) gets bodies of its own, gone with it: a
    /// scraper's identities share nothing, not even two through the same
    /// proxy (each may be a session of its own). The contexts that go out as
    /// the browser does share one set per process and browser-wide proxy and
    /// user agent, kept for the process's life, so that a later context,
    /// connection or fork lays out with them at once, as before contexts had
    /// identities of their own.
    fn layout_bodies_for(
        &mut self,
        own_identity: bool,
        config: &pyro_net::NetConfig,
    ) -> std::sync::Arc<pyro_dom::layout::SubresourceBodies> {
        // pyro: this browser's stores only: a process-wide store let another browser's documents,
        // whose ids come from their own counters and may equal these, write, fail or forget this
        // browser's entries (security review cycle 5).
        if own_identity {
            return Default::default();
        }
        let identity = (config.proxy.clone(), config.user_agent.clone());
        if let Some((_, bodies)) = self.layout_stores.iter().find(|(i, _)| *i == identity) {
            return bodies.clone();
        }
        let bodies = std::sync::Arc::<pyro_dom::layout::SubresourceBodies>::default();
        self.layout_stores.push((identity, bodies.clone()));
        bodies
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
        self.contexts.retain(|c| {
            if c.id == id {
                c.services.close();
            }
            c.id != id
        });
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
    pub fn context_net(&self, id: ContextId) -> Option<&pyro_net::NetClient> {
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
        self.fetch_subresource_with(page, url, kind, false, done)
    }

    /// [`Browser::fetch_subresource`], with `cors` in CORS mode with credentials only to the same
    /// origin (`crossorigin="anonymous"`, as `shape-outside` images load).
    pub fn fetch_subresource_with(&mut self, page: PageId, url: String, kind: ResourceKind, cors: bool, done: FetchDone) {
        let options = SubresourceOptions { crossorigin: cors.then(|| "anonymous".to_owned()), ..Default::default() };
        self.fetch_subresource_options(page, url, kind, options, done)
    }

    /// pyro: [`Browser::fetch_subresource`] made as `options` say (a CSS `url()`'s request URL
    /// modifiers): a response that fails the CORS check or the integrity check is an error.
    pub fn fetch_subresource_options(
        &mut self,
        page: PageId,
        url: String,
        kind: ResourceKind,
        options: SubresourceOptions,
        done: FetchDone,
    ) {
        // pyro: (a style sheet's request comes with its sheet's `Referrer-Policy` already: the
        // renderer takes it from the sheet object that made it, never looked up here by URL.)
        // A frame document's request is that frame's (by its id, never by a URL, which several
        // frames can share), the page's otherwise; a client that names no frame of the page
        // sends nothing.
        let built = self.page(page).and_then(|p| match options.client.as_deref() {
            None => Some((
                subresource_request_with(&p.main_frame, page, url.clone(), kind, &options),
                p.main_frame.origin.clone(),
            )),
            Some(client) => {
                let (frame_id, loader) = client.split_once('#')?;
                let key = p.frames.key_of(frame_id)?;
                p.frames
                    .with_frame(key, |f| {
                        (f.loader_id.0 == loader).then(|| {
                            (subresource_request_with(f, page, url.clone(), kind, &options), f.origin.clone())
                        })
                    })
                    .flatten()
            }
        });
        let Some((request, origin)) = built else {
            self.scheduler().post(pyro_runtime::TaskSource::Networking, move |b| done(b, Err("page is gone".into())));
            return;
        };
        let done: FetchDone = match options.integrity {
            None => done,
            Some(integrity) => {
                let cors = options.crossorigin.is_some();
                Box::new(move |b, result| {
                    let result = result.and_then(|r| match loader::integrity_error(&r, Some(&integrity), cors, &origin) {
                        Some(message) => Err(message),
                        None => Ok(r),
                    });
                    done(b, result)
                })
            }
        };
        let Some(loader) = self.loader_for(page) else {
            self.scheduler()
                .post(pyro_runtime::TaskSource::Networking, move |b| done(b, Err("no resource loader".into())));
            return;
        };
        if kind == ResourceKind::Image {
            let frame = options.client.as_deref().and_then(|client| self.live_client_frame(page, client)).map_or_else(|| page.into(), |pair| pair.0);
            if let Some(cancel) = self.dispatch_image(frame, &loader, request, done, true) { cancel.disarm(); }
        } else {
            loader.fetch(self, request, done);
        }
    }

    /// Loads what the layout of `frame`'s document is laid out with (an
    /// image, a font, an imported sheet) as [`Browser::fetch_subresource`]
    /// does for the renderer: through the page's browser context (its proxy,
    /// user agent, cookies, headers, offline state and interception),
    /// attributed to that document's frame. Only while that document (the one
    /// `loader` loaded) is the frame's; otherwise `done` is dropped uncalled.
    /// `done` gets the body of a 2xx response.
    pub(crate) fn fetch_layout_subresource(
        &mut self,
        frame: FrameKey,
        loader: &LoaderId,
        url: String,
        kind: ResourceKind,
        options: SubresourceOptions,
        done: Box<dyn FnOnce(Result<pyro_dom::layout::LayoutBody, String>) + Send>,
    ) {
        let (request, origin) = {
            let Some(state) = self.doc(frame) else { return };
            if state.main_frame.loader_id != *loader {
                return;
            }
            (
                subresource_request_with(&state.main_frame, frame.page, url, kind, &options),
                state.main_frame.origin.clone(),
            )
        };
        let cors = options.crossorigin.is_some();
        if kind == ResourceKind::Image && options.integrity.is_none() {
            let shape = speculate::Shape::of(self, &request).expect("whole image");
            if let (Some(state), Some(bodies)) = (self.doc(frame), self.layout_bodies(frame.page)) {
                let raw = pyro_dom::layout::SubresourceOptions {
                    crossorigin: options.crossorigin.clone(), integrity: None, referrer: options.referrer.clone(),
                    referrer_policy: options.referrer_policy.map(|policy| policy.as_str().to_owned()),
                };
                let slot = pyro_dom::layout::LayoutBodyKey::new(&document_client(&state.main_frame), &request.url, &raw, pyro_dom::layout::SubresourceKind::Image);
                bodies.image_dispatched(&slot, shape.referrer_source.map(Into::into), shape.policy.as_str().to_owned());
            }
        }
        let integrity = options.integrity;
        let Some(resource_loader) = self.loader_for(frame) else {
            done(Err("no resource loader".into()));
            return;
        };
        let environment = self.frame_document_encoding(frame);
        let completion: FetchDone = Box::new(move |_, result| {
                done(result.and_then(|r| {
                    // pyro: a response that fails its integrity check (`integrity()`) is an error.
                    if let Some(message) = loader::integrity_error(&r, integrity.as_deref(), cors, &origin) {
                        return Err(message);
                    }
                    match r.status {
                        // pyro: the body with its URL after redirects (a sheet resolves its URLs
                        // against it), its tainting and its status, together, after the checks.
                        200..=299 => {
                            let (final_url, opaque, status) = (r.url.clone(), loader::is_opaque(&r, cors, &origin), r.status);
                            let referrer_policy = r.referrer_policy.map(|p| p.as_str().to_owned());
                            let bytes = if kind == ResourceKind::Image { r.body } else { std::sync::Arc::new(r.render_body(environment.as_slice())) };
                            Ok(pyro_dom::layout::LayoutBody { image_lease: None, bytes, final_url, opaque, status, referrer_policy })
                        }
                        status => Err(format!("HTTP {status}")),
                    }
                }))
            });
        if kind == ResourceKind::Image {
            if let Some(cancel) = self.dispatch_image(frame, &resource_loader, request, completion, true) { cancel.disarm(); }
        } else {
            resource_loader.fetch(self, request, completion);
        }
    }

    /// pyro: the sub-resource bodies `page`'s documents' layouts fetched (their own fetches of
    /// CSS images, imported sheets and fonts), which a renderer of the page uses as they are.
    pub fn layout_bodies(&self, page: PageId) -> Option<std::sync::Arc<pyro_dom::layout::SubresourceBodies>> {
        self.page(page).map(|p| p.frames.layout_bodies.clone())
    }

    /// pyro: the frame and loader `client` ([`document_client`]) names in `page`, while that document
    /// is still its frame's.
    pub(crate) fn live_client_frame(&self, page: PageId, client: &str) -> Option<(FrameKey, LoaderId)> {
        let (frame_id, loader) = client.split_once('#')?;
        let p = self.page(page)?;
        let key = p.frames.key_of(frame_id)?;
        p.frames.with_frame(key, |f| (f.loader_id.0 == loader).then(|| (key, f.loader_id.clone()))).flatten()
    }

    /// pyro: whether `client` ([`document_client`]) names a document that is still its frame's
    /// in `page`.
    pub fn is_live_client(&self, page: PageId, client: &str) -> bool {
        let Some((frame_id, loader)) = client.split_once('#') else { return false };
        let Some(p) = self.page(page) else { return false };
        p.frames.key_of(frame_id).and_then(|key| p.frames.with_frame(key, |f| f.loader_id.0 == loader)).unwrap_or(false)
    }

    /// pyro: a body `frame`'s document fetched itself for `url` (a no-cors request with the
    /// defaults: an imported sheet, an `<object>`'s image) that its layout would otherwise fetch
    /// again: that document's alone ([`pyro_dom::layout::LayoutBodyKey`]), written with its URL
    /// after redirects, tainting and status once its checks passed.
    pub(crate) fn provide_layout_body(&self, frame: FrameKey, key: pyro_dom::layout::LayoutBodyKey, body: pyro_dom::layout::LayoutBody) {
        // Only for the document that is still the frame's (the key names it).
        let live = self.doc(frame).is_some_and(|d| document_client(&d.main_frame) == key.document());
        let Some(p) = self.page(frame.page).filter(|_| live) else { return };
        if p.frames.layout_bodies.start(&key) {
            p.frames.layout_bodies.finish(key, body);
        }
    }

    /// pyro: each document of `page` by its client: its list of available images and the URLs its
    /// elements requested as images (a capture asks about the document a request is for, a
    /// child frame's included, never the top document's).
    pub fn document_images(&self, page: PageId) -> std::collections::HashMap<String, DocumentImages> {
        let Some(p) = self.page(page) else { return Default::default() };
        p.frames
            .keys()
            .into_iter()
            .filter_map(|key| {
                let client = p.frames.with_frame(key, document_client)?;
                let available = p.frames.page_dom(key).map(|d| d.available_image_urls().into_iter().collect()).unwrap_or_default();
                let requested = self.doc(key).map(|d| d.images.urls().into_iter().collect()).unwrap_or_default();
                let loaded = p.frames.with_frame(key, |f| f.loaded).unwrap_or(false);
                Some((client, DocumentImages { available, requested, loaded }))
            })
            .collect()
    }



    /// The encoding `page`'s document was decoded with, the environment
    /// encoding of the style sheets it imports ([`Resource::render_body`]).
    pub fn document_encoding(&self, page: PageId) -> Option<String> {
        self.frame_document_encoding(page.into())
    }

    /// [`Browser::document_encoding`] of `frame`'s document.
    pub(crate) fn frame_document_encoding(&self, frame: FrameKey) -> Option<String> {
        let page_dom = self.doc(frame)?.main_frame.page_dom.clone()?;
        let dom = page_dom.dom.borrow();
        dom.document_data(page_dom.document()).ok().map(|d| d.charset.to_owned())
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
            fullscreen_container: Default::default(),
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
            unreachable: None,
        };
        let nav = PageNav::new(id, frame.id.clone(), self.sched.clone(), self.observer.clone());
        let frames = FrameHost::new(self, id, context, frame.clone(), nav.clone());
        self.push_page(id, context, frame, frames, nav);
        self.navigate(
            id,
            NavigateOpts {
                post: None,
                url: "about:blank".into(),
                reason: NavigationReason::InitialFrameNavigation,
                cdp_request_id: None,
                referrer: None,
                referrer_policy: None,
            },
        )?;
        Ok(id)
    }

    fn push_page(&mut self, id: PageId, context: ContextId, frame: Frame, frames: Rc<FrameHost>, nav: Rc<PageNav>) {
        let session_storage = frames.settings.borrow().session.clone();
        let viewport = frames.settings.borrow().viewport.clone();
        let init_scripts = self.contexts.iter().find(|c| c.id == context).unwrap().init_scripts.clone();
        frames.settings.borrow_mut().scripts = init_scripts.clone();
        // A popup made ahead (`create_page_with`) took its id earlier.
        let at = self.pages.partition_point(|p| p.id < id);
        self.pages.insert(at, Page {
            id,
            context,
            root_dirty: Cell::new(true),
            init_scripts,
            bindings: Vec::new(),
            frames,
            document: DocumentState {
                main_frame: frame,
                pending_loader: None,
                pending_frame_attributes: 0,
                pending_cancel: None,
                pending_script: None,
                nav,
                load: None,
                stylesheets: Default::default(),
                images: Default::default(),
                preloads: Default::default(),
                refresh: Default::default(),
                _owner_pin: None,
                in_order_scripts: Default::default(),
            },
            session_storage,
            popup: None,
            script_closable: false,
            opener_origin: None,
            user_agent_override: None,
            max_touch_points: 0,
            script_execution_disabled: false,
            bypass_csp: false,
            extra_http_headers: Default::default(),
            cache_disabled: false,
            bypass_service_worker: false,
            request_defaults: Default::default(),
            native_viewport: viewport.clone(),
            viewport,
            pending_resize: None,
        });
    }

    /// The popup page the opener's `window.open` made ahead (its ticket's
    /// prebuilt page, `PopupTicket::take_prebuilt`): it becomes a page of
    /// `context` with its initial about:blank document as it is now, which
    /// the opener may have scripted already. `None` when `prebuilt` is not
    /// such a page or the context is gone.
    pub fn create_page_with(&mut self, context: ContextId, prebuilt: Box<dyn std::any::Any>) -> Option<PageId> {
        let prebuilt = prebuilt.downcast::<frames::PrebuiltPopup>().ok()?;
        if !self.contexts.iter().any(|c| c.id == context) || prebuilt.host.context != context {
            return None;
        }
        let frames::PrebuiltPopup { id, host, nav, frame } = *prebuilt;
        self.push_page(id, context, frame, host, nav);
        let p = self.page_mut(id)?;
        let realms = p.frames.root.borrow().realms.clone();
        p.main_frame.realms = realms;
        p.main_frame.dom_content_loaded = true;
        p.main_frame.loaded = true;
        Some(id)
    }

    /// Reports the commit and load of `page`'s initial about:blank document
    /// as it is now (a popup's, made by `window.open`; CDP clients wait for a
    /// commit). False when the page shows another document.
    pub fn announce_initial_document(&mut self, id: PageId) -> bool {
        let Some(p) = self.page(id) else { return false };
        if !p.nav.is_initial_blank() || p.main_frame.url != "about:blank" {
            return false;
        }
        let (frame_id, loader_id) = (p.main_frame.id.clone(), p.main_frame.loader_id.clone());
        let timestamp = self.timestamp();
        self.emit(id, PageEvent::FrameNavigated {
            frame_id: frame_id.clone(),
            loader_id: loader_id.clone(),
            url: "about:blank".into(),
            reason: NavigationReason::InitialFrameNavigation,
            cdp_request_id: None,
            timestamp,
            unreachable_url: None,
        });
        self.emit(id, PageEvent::FrameDomContentLoaded { frame_id: frame_id.clone(), loader_id: loader_id.clone(), timestamp });
        self.emit(id, PageEvent::FrameLoaded { frame_id, loader_id, timestamp });
        true
    }

    /// Initialize the browser chrome and viewport of a newly created top-level
    /// context even when the initiating script requested no opener handle.
    pub fn configure_popup(&mut self, page: PageId, opener: PageId, window_name: &str, features: pyro_webapi::window_features::WindowFeatures) {
        let Some(mut viewport) = self.page(opener).map(|p| frames::popup_viewport(&p.viewport)) else { return };
        let chrome = frames::apply_window_features(&mut viewport, features);
        if let Some(p) = self.page_mut(page) {
            // The target name belongs to the new navigable independently of
            // whether it has an opener. Only "_blank" denotes an unnamed one.
            if !window_name.eq_ignore_ascii_case("_blank") {
                p.main_frame.name = window_name.to_owned();
            }
            let mut settings = p.frames.settings.borrow_mut();
            p.native_viewport = viewport.clone();
            settings.window_chrome = chrome;
            settings.viewport = viewport.clone();
            if let Some(dom) = &p.main_frame.page_dom {
                dom.set_window_chrome(chrome.0, chrome.1, chrome.2);
            }
        }
        self.sync_root(page);
        let _ = self.set_viewport(page, viewport);
    }

    /// Makes `page` (just created in the opener's context) the popup that
    /// `opener`'s document requested with `ticket` (`PageEvent::PopupRequested`).
    /// With a ticket the popup keeps its opener: its documents see the
    /// opener's window as `window.opener`, its about:blank documents take
    /// the opener's origin and base URL, it uses the session storage copied
    /// for it (the current about:blank document switches to it), gets the
    /// requested browsing context name, and the opener's object reaches its
    /// documents. Without one (`noopener`) only the context is shared.
    pub fn adopt_popup(&mut self, page: PageId, opener: PageId, ticket: Option<Rc<pyro_webapi::PopupTicket>>) {
        if let Some(p) = self.page_mut(page) { p.script_closable = true; }
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
        if let Some((frames, opener_main)) = opener_state {
            // The popup's creator is the document that called `window.open`, which may be a frame of
            // the opener page (a sandboxed child has an opaque origin of its own): a popup made in that
            // document's context (`FrameHost::prebuild_popup`, sharing its DOM store) already has its
            // origin, which later about:blank documents of the popup inherit. Otherwise the initial
            // about:blank document takes the opener page's.
            let prebuilt = frames.upgrade().is_some_and(|f| Rc::ptr_eq(&f.dom, &p.frames.dom))
                && p.main_frame.url == "about:blank";
            let origin = if prebuilt {
                (p.main_frame.origin.clone(), p.main_frame.opaque_origin, p.main_frame.base_url.clone())
            } else {
                opener_main
            };
            if let Some(opener_host) = frames.upgrade() {
                p.frames.cluster_group.set(opener_host.cluster_group.get());
                p.frames.share_origin_keying(&opener_host);
            }
            *p.frames.opener.borrow_mut() = Some(frames);
            *p.frames.popup.borrow_mut() = Some(ticket.clone());
            // The initial about:blank document is the opener's creation.
            if p.main_frame.url == "about:blank" && !prebuilt {
                p.main_frame.origin = origin.0.clone();
                p.main_frame.opaque_origin = origin.1;
                p.main_frame.base_url = origin.2.clone();
                for realm in &p.main_frame.realms {
                    realm.realm.set_origin(Some(origin.0.clone()));
                }
                if let Some(dom) = &p.main_frame.page_dom {
                    dom.set_window_agent_cluster(p.frames.cluster_group.get(), frames::frame_origin_key(&p.main_frame));
                    dom.set_base_url(&origin.2);
                    let site = pyro_net::Url::parse(&origin.0).ok().and_then(|url| pyro_net::cookies::partition_key(&url));
                    let context = if site.is_some() { pyro_net::cookies::SiteContext::SameSite } else { pyro_net::cookies::SiteContext::CrossSite };
                    dom.set_cookie_context(site, context);
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
        value: Option<pyro_webapi::UserAgentOverride>,
    ) -> Result<(), JsError> {
        let state = self.page_mut(page).ok_or_else(|| JsError::Type("no such page".into()))?;
        let mut defaults = value
            .as_ref()
            .map(pyro_webapi::UserAgentOverride::request_defaults)
            .transpose()
            .map_err(|e| JsError::type_error(format!("invalid user agent request defaults: {e}")))?
            .unwrap_or_default();
        // The user agent override owns none of the page's other request options.
        state.apply_request_options(&mut defaults);
        if let Some(bindings) = &state.main_frame.page_dom {
            bindings
                .set_user_agent_override(value.clone())
                .map_err(|e| JsError::type_error(format!("invalid user agent request defaults: {e}")))?;
        }
        state.user_agent_override = value;
        state.request_defaults = defaults;
        self.refresh_frame_settings(page);
        Ok(())
    }

    /// The permissions and clipboard of `context`'s pages.
    pub fn context_services(&self, context: ContextId) -> Option<Rc<pyro_webapi::ContextServices>> {
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

    /// Whether this page's documents run script (`Emulation.setScriptExecutionDisabled`).
    /// Applies from the next document the page or one of its frames commits:
    /// its script elements, event handlers, timers and `javascript:` URLs do
    /// not run, `<noscript>` is markup, and meta refresh still works, as in
    /// Chrome with JavaScript off. Script evaluated over the protocol still
    /// runs. Returns false if there is no such page.
    pub fn set_script_execution_disabled(&mut self, page: PageId, disabled: bool) -> bool {
        let Some(state) = self.page_mut(page) else { return false };
        state.script_execution_disabled = disabled;
        true
    }

    /// Whether this page's documents ignore their Content-Security-Policy
    /// (`Page.setBypassCSP`). Applies from the next document the page or one
    /// of its frames commits; the document already loaded keeps what it had,
    /// as in Chrome. The engine enforces no directive, so what this lifts is
    /// the one effect a policy has: nonce hiding, which a header-delivered
    /// policy turns on (see `pyro_dom::Dom::set_header_csp`). Returns false
    /// if there is no such page.
    pub fn set_bypass_csp(&mut self, page: PageId, enabled: bool) -> bool {
        let Some(state) = self.page_mut(page) else { return false };
        state.bypass_csp = enabled;
        true
    }

    /// Replaces this page's extra request headers (an empty map clears them).
    /// They apply to the page's document, subresource, fetch and XHR requests
    /// over context-wide settings, persist across navigation, and do not
    /// affect sibling pages. Returns false if there is no such page.
    pub fn set_extra_http_headers(&mut self, page: PageId, headers: pyro_net::http::HeaderMap) -> bool {
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

    /// Makes this page's fetch/XHR skip the controlling service worker
    /// (`Network.setBypassServiceWorker`), in all its frames and across
    /// navigation. Returns false if there is no such page.
    pub fn set_bypass_service_worker(&mut self, page: PageId, bypass: bool) -> bool {
        let Some(state) = self.page_mut(page) else { return false };
        if let Some(bindings) = &state.main_frame.page_dom {
            bindings.set_bypass_service_worker(bypass);
        }
        state.bypass_service_worker = bypass;
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
        // A deferred realm defines nothing yet (`add_binding` makes it to
        // define one): nothing to delete, and not worth making it for.
        for realm in self.page_realms(page).iter().filter(|r| !r.realm.is_deferred()) {
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
        // HTML "navigate" ("checking if unloading is canceled"): the document being left and the
        // documents of its descendant frames are asked first, in tree order (`beforeunload` at
        // their Windows). The navigation goes on whatever their listeners answer
        // (`pyro_webapi::lifecycle::before_unload`).
        let own = self.doc(page).and_then(|doc| doc.main_frame.page_dom.clone());
        let descendants = self.page(page.page).map(|p| p.frames.subtree_documents(page)).unwrap_or_default();
        let leaving = own.iter().cloned().chain(
            descendants.into_iter().filter(|d| !own.as_ref().is_some_and(|own| Rc::ptr_eq(own, d))),
        );
        for document in leaving {
            pyro_webapi::lifecycle::before_unload(&document);
        }
        self.cancel_lazy_frame(page);
        // The container policy (`fullscreen`) is taken as the navigation begins: an `allowfullscreen`
        // set while the response is on its way applies to the next navigation, not this one.
        // (The document now shown keeps the policy it was made with.)
        let container = self.doc(page).and_then(|d| d.main_frame.owner).and_then(|owner| {
            let host = self.page(page.page).map(|p| p.frames.clone())?;
            // The container document's origin from its frame: an about:blank/srcdoc parent's
            // inherited one.
            let parent_origin = host.frame(host.parent_key(page)).map(|f| f.origin).filter(|o| o != "null");
            let dom = host.dom.borrow();
            Some(frames::fullscreen_container(&dom, owner, parent_origin.as_deref()))
        });
        if let (Some(container), Some(mut state)) = (container, self.doc_mut(page)) {
            state.main_frame.fullscreen_container = container;
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
        // Reloading an error document (its URL is Chrome's error page)
        // fetches the URL it could not reach, as Chrome does.
        let mut opts = opts;
        if opts.url == ERROR_DOCUMENT_URL {
            if let Some(unreachable) = self.doc(page).and_then(|doc| doc.main_frame.unreachable.clone()) {
                opts.url = unreachable.url;
            }
        }
        // `about:srcdoc` is the iframe's `srcdoc`, never fetched: navigating
        // to it (a srcdoc document's `location.reload()`) loads it again.
        if opts.url == "about:srcdoc" {
            if let Some((frame_id, html)) = self.srcdoc_of(page) {
                self.cancel_pending_document(page);
                let referrer = opts.referrer.clone().unwrap_or_default();
                return self.begin_srcdoc(page, frame_id, html, referrer, history);
            }
        }
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
            return Err(JsError::Unsupported(&"navigation to non-blank URLs"));
        }
        self.commit_document(page, &opts, loader_id.clone(), "null", history, None)?;
        self.finish_blank_document(page);
        Ok(loader_id)
    }

    /// Starts an ordinary navigation, adding a session-history entry on commit.
    pub fn navigate(&mut self, page: PageId, opts: NavigateOpts) -> Result<LoaderId, JsError> {
        self.navigate_with_history(page, opts, false)
    }

    /// HTML `pageswap` at the active document of `frame`, which a cross-document navigation to
    /// `url` (of origin `origin`) is about to replace. Its activation is null for a navigation to
    /// another origin; an about:blank or srcdoc document takes its creator's origin, as the one it
    /// replaces did. (Pyro does not track the origins of a navigation's redirects: a cross-origin
    /// redirect back to the same origin still gets an activation.)
    fn fire_page_swap(&mut self, frame: FrameKey, url: &str, history: HistoryHandling, origin: &str) {
        let Some((dom, old_origin, realm)) = self.doc(frame).and_then(|d| {
            Some((d.main_frame.page_dom.clone()?, d.main_frame.origin.clone(), d.main_frame.main_realm().cloned()?))
        }) else {
            return;
        };
        let same_origin = matches!(url, "about:blank" | "about:srcdoc") || (old_origin != "null" && old_origin == origin);
        let ty = match history {
            HistoryHandling::Push => "push",
            HistoryHandling::Replace => "replace",
            HistoryHandling::Traverse => "traverse",
            HistoryHandling::Reload => "reload",
        };
        pyro_webapi::lifecycle::page_swap(&dom, || {
            if !same_origin {
                return None;
            }
            let scope = realm.scope().ok()?;
            window::navigation_activation(&scope, ty, url)
        });
    }

    /// HTML "unload a document and its descendants" for the active document
    /// of `frame`: its child frames' documents first, then its own
    /// (`pyro_webapi::lifecycle::unload`). A frame tree without listeners
    /// costs a walk over its frames.
    fn unload_documents(&mut self, frame: FrameKey) {
        let Some(host) = self.page(frame.page).map(|p| p.frames.clone()) else { return };
        fn collect(b: &Browser, host: &frames::FrameHost, key: FrameKey, out: &mut Vec<Rc<pyro_webapi::PageDom>>) {
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
            pyro_webapi::lifecycle::unload(&dom);
        }
    }

    /// Replaces the frame's realms with fresh ones for the new document, runs
    /// init scripts, then emits `FrameNavigated`. `unreachable`: the document
    /// is the error document of that failed navigation, whose URL its
    /// session history entry keeps (Chrome's, which a reload or a traversal
    /// fetches again).
    pub(crate) fn commit_document(
        &mut self,
        page: FrameKey,
        opts: &NavigateOpts,
        loader_id: LoaderId,
        origin: &str,
        history: HistoryHandling,
        unreachable: Option<NavigationError>,
    ) -> Result<(), JsError> {
        // `pageswap` at the document being replaced, then it (and its frames) unloads.
        self.fire_page_swap(page, &opts.url, history, origin);
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
                flags.automatic_features_blocked |= parent.sandbox.automatic_features_blocked;
                flags.popups_blocked |= parent.sandbox.popups_blocked;
                flags.navigation_blocked |= parent.sandbox.navigation_blocked;
                flags.top_navigation_blocked |= parent.sandbox.top_navigation_blocked;
                flags.top_navigation_needs_activation |= parent.sandbox.top_navigation_needs_activation;
                flags.document_domain_blocked |= parent.sandbox.document_domain_blocked;
            }
            flags
        } else {
            // A top-level document: the page's script switch, which its
            // frames inherit like a sandbox without `allow-scripts`.
            let disabled = self.page(page.page).is_some_and(|p| p.script_execution_disabled);
            frames::Sandbox { scripts_disabled: disabled, ..Default::default() }
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
        let previous_origin = self.doc(page).map(|d| d.main_frame.origin.clone());
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
        // pyro: what the previous document's layout loaded, or still waits for, is its alone: gone
        // with it (`SubresourceBodies::forget_document`).
        if let (Some(previous), Some(p)) = (self.doc(page).map(|d| document_client(&d.main_frame)), self.page(page.page)) {
            if !previous.ends_with(&format!("#{}", loader_id.0)) {
                p.frames.layout_bodies.forget_document(&previous);
            }
        }
        let scripts = self.page(page.page).ok_or(JsError::ContextDestroyed)?.init_scripts.clone();
        let (frame_id, mut old, scripts, nav, old_dom, initial_same_origin) = {
            let mut p = self.doc_mut(page).ok_or(JsError::Type("no such page".into()))?;
            p.main_frame.network_idle = NetworkIdle::default();
            // HTML Window reuse: the initial about:blank document of a frame,
            // replaced by a document of its own origin.
            let blank_loaded = p.main_frame.loaded && p.main_frame.page_dom.as_ref().is_some_and(|d| d.blank_requested());
            let initial_same_origin = !blank_loaded
                && p.nav.is_initial_blank()
                && previous_origin.as_deref() == Some(actual_origin.as_str())
                && actual_origin != "null";
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
            p.main_frame.unreachable = unreachable.clone();
            let entry_url = unreachable.as_ref().map_or(opts.url.as_str(), |u| u.url.as_str());
            p.nav.commit_document_at(&opts.url, entry_url, history, None);
            p.nav.set_document(Some((page_dom.dom.clone(), page_dom.document())));
            p.nav.set_page_dom(&page_dom);
            (p.main_frame.id.clone(), old, scripts.clone(), p.nav.clone(), (old_dom, replaced), initial_same_origin)
        };
        if let Some(host) = self.page(page.page).map(|p| p.frames.clone()) {
            host.store_ancestor_origins(page);
        }
        let (old_dom, replaced) = old_dom;
        // A child frame's initial about:blank Window made reusable
        // (`FrameHost::reuse_realm`) becomes the new document's: it keeps its
        // realm, event target and what script put on it, as browsers keep
        // the Window object.
        let reused = match &old_dom {
            Some(dom) if page.child.is_some() && initial_same_origin && dom.reusable_window() => {
                let at = old.iter().position(|r| {
                    r.world.is_none() && !r.realm.is_deferred() && !r.realm.is_disposed() && !r.realm.is_lingering()
                });
                match at {
                    Some(at) if pyro_webapi::adopt_window(dom, &page_dom, old[at].realm.id()) => {
                        let r = old.remove(at);
                        window::cancel_timers(&r.realm);
                        pyro_webapi::lifecycle::realm_disposed(dom, r.realm.id());
                        Some(r.realm)
                    }
                    _ => None,
                }
            }
            _ => None,
        };
        // A child frame's previous document stays usable while script of
        // other documents holds it (its parent's, through a saved
        // `contentDocument`): HTML only makes it not fully active. Its realms
        // linger until nothing reaches them (`Realm::linger`); the engine
        // disposes them after a collection finds them dead.
        let linger = page.child.is_some() && old_dom.is_some();
        if let Some(dom) = &old_dom {
            if linger {
                dom.detach();
            } else {
                dom.dispose();
            }
        }
        let mut lingered = true;
        for r in old {
            if linger {
                window::release_roots(&r.realm);
            } else {
                window::cancel_timers(&r.realm);
            }
            let id = r.realm.id();
            if let Some(dom) = &old_dom {
                if linger {
                    pyro_webapi::lifecycle::realm_detached(dom, id);
                } else {
                    pyro_webapi::lifecycle::realm_disposed(dom, id);
                }
            }
            if !(linger && r.realm.linger()) {
                lingered = false;
                r.realm.dispose();
            }
            self.emit(page.page, PageEvent::RealmDestroyed { frame_id: frame_id.clone(), realm: id });
        }
        if linger && !lingered {
            if let Some(dom) = &old_dom {
                dom.dispose();
            }
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
            match (&world, &reused) {
                (None, Some(realm)) => host.reuse_realm(page, nav.clone(), realm)?,
                _ => {
                    host.make_realm(page, nav.clone(), world)?;
                }
            }
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
                unreachable_url: unreachable.map(|u| u.url),
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
        host.drain_retired_disposing();
        let loader = self.pages[pos].main_frame.loader_id.clone();
        self.forget_network_activity(page.into(), &loader);
        self.pages[pos].main_frame.realms = host.root.borrow().realms.clone();
        self.pages[pos].root_dirty.set(true);
        let p = self.pages.remove(pos);
        if let Some(dom) = &p.main_frame.page_dom {
            dom.dispose();
            // The document leaves its DOM store, as a navigated-away or removed frame's does: a
            // popup shares its opener's store (and the opener its popups'), which outlives this page,
            // so its tree is freed by the store's collection rather than with the store.
            let _ = dom.dom.borrow_mut().set_document_active(dom.document(), false);
        }
        for r in &p.document.main_frame.realms {
            window::cancel_timers(&r.realm);
            let id = r.realm.id();
            if let Some(dom) = &p.main_frame.page_dom {
                pyro_webapi::lifecycle::realm_disposed(dom, id);
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
        // SAFETY: taken once, here; nothing reads the field afterwards (the
        // remaining fields drop without it).
        let engine = unsafe { std::mem::ManuallyDrop::take(&mut self.engine) };
        if let Some(slot) = self.engine_return.take() {
            *slot.borrow_mut() = Some(engine);
        }
    }
}
