//! Document loading: fetch → parse → scripts → DOMContentLoaded → load.
//!
//! Script handling follows HTML "prepare the script element" for
//! parser-inserted scripts (classification by `lp_dom::Dom::script_info`):
//! * inline classic: runs immediately (parser-blocking);
//! * external classic, no async/defer: parser-blocking fetch;
//! * external classic `defer`, and non-async module scripts: run in document
//!   order after parsing, before `DOMContentLoaded`;
//! * `async` (classic or module): run when ready, before `load`;
//! * import maps: merged into the realm's map when reached;
//! * `nomodule` classic scripts and data blocks: not executed.
//!
//! Scripts the parser did not insert (`appendChild`, `cloneNode`, a `src` set
//! while connected) arrive as `ResourceKind::Script` resource events and go
//! through `Browser::script_event`: inline classic scripts run at the drain,
//! external classic ones join the set of scripts that execute as soon as
//! possible (independently of parsing and of the deferred list), and modules
//! fetch their graph and run when ready. External and module script elements
//! get `load`/`error` events.
//!
//! Deviations: a deferred module's evaluation promise (including top-level
//! await) is awaited before the next deferred script/`DOMContentLoaded`,
//! because JSC fetches the module graph during evaluation, not before it. A
//! script-inserted script is prepared when the inserting script returns (a
//! parser-run script) or at the end of the inserting task (a handler or a
//! timer), instead of inside the DOM call that inserted it. A module
//! element's `load` fires when its evaluation promise fulfills, `error` when
//! it rejects (also for an evaluation error).
//!
//! DOM lifecycle and currentScript are wired through lp-webapi. Iframe/image
//! resources are separate loader work; linked stylesheets are handled by
//! `stylesheets`.

use crate::frames::FrameKey;
use crate::{Browser, HistoryHandling, LoaderId, NavigateOpts, PageEvent, PageId};
use lp_dom::parse::{DocumentParser, DomRef, ParseStep};
use lp_dom::script::ScriptKind;
use lp_dom::NodeId;
use lp_js::{JsError, ModuleFetcher, ModuleRequest, Persistent, Realm};
use lp_runtime::TaskSource;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::Duration;

/// `LIGHTNINGPANDA_TRACE_MODULES=1`: one stderr line per step of every ES
/// module fetch (requested, task started, loaded, handed to the engine), to
/// find an `import()` that never settles in a real application.
pub(crate) fn trace_modules() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("LIGHTNINGPANDA_TRACE_MODULES").is_ok_and(|v| v == "1"))
}

macro_rules! module_trace {
    ($($arg:tt)*) => {
        if trace_modules() {
            eprintln!("lightningpanda: module {:.3} {}", trace_epoch().elapsed().as_secs_f64() * 1e3, format_args!($($arg)*));
        }
    };
}

fn trace_epoch() -> std::time::Instant {
    static T0: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    *T0.get_or_init(std::time::Instant::now)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    Document,
    Script,
    ModuleScript,
    Stylesheet,
    Iframe,
    Image,
    /// A web font (a renderer's `@font-face` source).
    Font,
    /// A hyperlink download (`<a download>`).
    Download,
}

#[derive(Debug, Clone)]
pub struct FetchRequest {
    /// Body and content type of a form POST (`None`: GET).
    pub post: Option<(Vec<u8>, String)>,
    pub url: String,
    pub kind: ResourceKind,
    pub page: PageId,
    pub referrer: Option<String>,
    pub frame_id: Option<String>,
    pub loader_id: Option<String>,
    /// HTML crossorigin attribute (absent uses the resource default).
    pub crossorigin: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Resource {
    /// Final URL after redirects.
    pub url: String,
    pub status: u16,
    /// Lowercased MIME essence (e.g. `text/html`).
    pub mime: String,
    pub charset: Option<String>,
    pub body: Vec<u8>,
    /// The `Content-Disposition` response header, if any.
    pub content_disposition: Option<String>,
    /// The response carried a `Content-Security-Policy` header (the policy
    /// itself is not enforced; a document delivered with one hides nonces).
    pub header_csp: bool,
    /// The response's `Referrer-Policy` header (a document's default policy).
    pub referrer_policy: Option<lp_net::ReferrerPolicy>,
}

impl Resource {
    /// Body decoded with the response charset (BOM wins), UTF-8 by default.
    pub fn text(&self) -> String {
        lp_net::decode_text(&self.body, self.charset.as_deref()).into_owned()
    }
}

/// Completion of a fetch, run as a task on the browser thread.
pub type FetchDone = Box<dyn FnOnce(&mut Browser, Result<Resource, String>)>;

/// Fetches resources for pages. Implementations must complete `done`
/// asynchronously (as a later task), never re-entrantly from `fetch`.
pub trait ResourceLoader {
    fn fetch(&self, browser: &mut Browser, request: FetchRequest, done: FetchDone);

    /// Optional cancellation for document-owned resources. Dropping the returned
    /// handle cancels transport; a late completion must still be safe to ignore.
    fn fetch_cancellable(
        &self,
        browser: &mut Browser,
        request: FetchRequest,
        done: FetchDone,
    ) -> Option<ResourceCancellation> {
        self.fetch(browser, request, done);
        None
    }
}

/// Browser-thread cancellation of an in-flight resource. Custom loaders may
/// return this from `fetch_cancellable`; ordinary `fetch` remains unchanged.
pub struct ResourceCancellation(Option<Box<dyn FnOnce()>>);
impl ResourceCancellation {
    pub fn new(cancel: impl FnOnce() + 'static) -> Self {
        Self(Some(Box::new(cancel)))
    }
    pub fn disarm(mut self) {
        self.0.take();
    }
}
impl Drop for ResourceCancellation {
    fn drop(&mut self) {
        if let Some(cancel) = self.0.take() {
            cancel();
        }
    }
}

impl Browser {
    /// Stores a completion for an asynchronous fetch; see [`Browser::complete_fetch`].
    pub fn register_completion(&mut self, done: FetchDone) -> u64 {
        let id = self.next_completion_id();
        self.completions.insert(id, done);
        id
    }

    /// Runs (once) the completion registered under `id`.
    pub fn complete_fetch(&mut self, id: u64, result: Result<Resource, String>) {
        if let Some(done) = self.completions.remove(&id) {
            done(self, result);
        }
    }
}

/// Network-backed loader over `lp_net::NetClient` (hyper on the I/O thread).
/// Bodies are accumulated on the I/O thread and delivered as one task.
pub struct NetLoader {
    client: lp_net::NetClient,
}

impl NetLoader {
    pub fn new(client: lp_net::NetClient) -> NetLoader {
        NetLoader { client }
    }

    pub fn client(&self) -> &lp_net::NetClient {
        &self.client
    }
}

impl ResourceLoader for NetLoader {
    fn fetch(&self, browser: &mut Browser, request: FetchRequest, done: FetchDone) {
        if let Some(cancel) = self.fetch_cancellable(browser, request, done) {
            cancel.disarm();
        }
    }

    fn fetch_cancellable(
        &self,
        browser: &mut Browser,
        request: FetchRequest,
        done: FetchDone,
    ) -> Option<ResourceCancellation> {
        let id = browser.register_completion(done);
        let handle = browser.loop_handle();
        let url = match url::Url::parse(&request.url) {
            Ok(u) => u,
            Err(e) => {
                let msg = format!("net::ERR_INVALID_URL {}: {e}", request.url);
                browser.scheduler().post(TaskSource::Networking, move |b| b.complete_fetch(id, Err(msg)));
                return None;
            }
        };
        let destination = match request.kind {
            ResourceKind::Document => lp_net::Destination::Document,
            ResourceKind::Script => lp_net::Destination::Script,
            ResourceKind::ModuleScript => lp_net::Destination::Script,
            ResourceKind::Stylesheet => lp_net::Destination::Style,
            ResourceKind::Iframe => lp_net::Destination::Iframe,
            ResourceKind::Image => lp_net::Destination::Image,
            ResourceKind::Font => lp_net::Destination::Font,
            ResourceKind::Download => lp_net::Destination::Other,
        };
        let mut req = lp_net::Request::get(url, destination);
        if let Some(page) = browser.page(request.page) {
            req.defaults = page.request_defaults.clone();
        }
        if request.kind == ResourceKind::ModuleScript || request.crossorigin.is_some() {
            req.mode = lp_net::RequestMode::Cors;
        }
        if request.crossorigin.as_deref().is_some_and(|v| v.eq_ignore_ascii_case("use-credentials")) {
            req.credentials = lp_net::Credentials::Include;
        }
        if !matches!(request.kind, ResourceKind::Document | ResourceKind::Iframe) {
            req.origin = Some(
                request
                    .frame_id
                    .as_ref()
                    .and_then(|id| browser.frame(request.page, &crate::FrameId(id.clone())))
                    .map_or_else(|| "null".into(), |f| f.origin),
            );
        }
        if request.kind == ResourceKind::Stylesheet {
            if request.crossorigin.is_none() {
                req.credentials = lp_net::Credentials::Include;
            }
            req.max_body = Some(crate::stylesheets::MAX_SHEET_BYTES.min(self.client.config().max_body));
        }
        if let Some((body, content_type)) = request.post {
            req.method = lp_net::http::Method::POST;
            req.body = Some(body.into());
            if let Ok(value) = lp_net::http::HeaderValue::from_str(&content_type) {
                req.headers.insert(lp_net::http::header::CONTENT_TYPE, value);
            }
            req.origin = request
                .frame_id
                .as_ref()
                .and_then(|id| browser.frame(request.page, &crate::FrameId(id.clone())))
                .map(|f| f.origin);
        }
        req.referrer = request.referrer.as_deref().and_then(|r| url::Url::parse(r).ok());
        // The requesting document's policy (for a navigation: the frame's
        // current document, which started it).
        if req.referrer.is_some() {
            if let Some(policy) = request
                .frame_id
                .as_deref()
                .and_then(|id| browser.page(request.page)?.frames.key_of(id))
                .and_then(|key| browser.page(request.page)?.frames.page_dom(key))
                .map(|page_dom| page_dom.referrer_policy())
            {
                req.referrer_policy = policy;
            }
        }
        req.frame_id = request.frame_id;
        req.loader_id = request.loader_id;
        // A preloaded static file: answered here, on the browser thread, with
        // no I/O-thread round trip. Observers still get the Network events.
        if let Some(hit) = self.client.static_direct(&req) {
            if request.kind == ResourceKind::ModuleScript {
                module_trace!("served from the static directory {}", req.url);
            }
            let head = self.client.report_local(&req, &hit);
            let resource = Resource {
                url: head.url.to_string(),
                status: 200,
                mime: head.mime,
                charset: head.charset,
                content_disposition: None,
                header_csp: false,
                referrer_policy: None,
                body: hit.file.body.to_vec(),
            };
            browser.scheduler().post(TaskSource::Networking, move |b| b.complete_fetch(id, Ok(resource)));
            return None;
        }
        if request.kind == ResourceKind::ModuleScript {
            module_trace!("sent to the network {}", req.url);
        }
        let mut head: Option<lp_net::ResponseHead> = None;
        let mut body: Vec<u8> = Vec::new();
        let native_id = self.client.fetch(
            req,
            Box::new(move |ev| match ev {
                lp_net::ResponseEvent::Redirect { .. } => {}
                lp_net::ResponseEvent::Response(h) => head = Some(h),
                lp_net::ResponseEvent::Data(chunk) => body.extend_from_slice(&chunk),
                lp_net::ResponseEvent::Finished { .. } => {
                    let result = match head.take() {
                        Some(h) => Ok(Resource {
                            url: h.url.to_string(),
                            status: h.status.as_u16(),
                            mime: h.mime,
                            charset: h.charset,
                            content_disposition: h
                                .headers
                                .get(http::header::CONTENT_DISPOSITION)
                                .and_then(|v| v.to_str().ok())
                                .map(str::to_owned),
                            header_csp: h.headers.contains_key(http::header::CONTENT_SECURITY_POLICY),
                            referrer_policy: lp_net::ReferrerPolicy::from_header(
                                h.headers.get_all("referrer-policy").iter().filter_map(|v| v.to_str().ok()),
                            ),
                            body: std::mem::take(&mut body),
                        }),
                        None => Err("net::ERR_EMPTY_RESPONSE".to_string()),
                    };
                    // Err: the browser thread has exited; nothing to deliver.
                    let _ = handle.post(move |b: &mut Browser| b.complete_fetch(id, result));
                }
                lp_net::ResponseEvent::Failed(e) => {
                    let msg = e.to_string();
                    let _ = handle.post(move |b: &mut Browser| b.complete_fetch(id, Err(msg)));
                }
            }),
        );
        let client = self.client.clone();
        Some(ResourceCancellation::new(move || client.cancel(native_id)))
    }
}

/// (MIME, body, response delay) per URL.
type StaticEntry = (String, Vec<u8>, Duration);

/// In-memory loader for tests and `Page.setDocumentContent`-style use.
#[derive(Default)]
pub struct StaticLoader {
    entries: RefCell<std::collections::HashMap<String, StaticEntry>>,
    pub log: RefCell<Vec<String>>,
}

impl StaticLoader {
    pub fn insert(&self, url: &str, mime: &str, body: &str, delay: Duration) {
        self.insert_bytes(url, mime, body.as_bytes(), delay);
    }
    pub fn insert_bytes(&self, url: &str, mime: &str, body: &[u8], delay: Duration) {
        self.entries.borrow_mut().insert(url.to_string(), (mime.to_string(), body.to_vec(), delay));
    }
}

impl ResourceLoader for StaticLoader {
    fn fetch(&self, browser: &mut Browser, request: FetchRequest, done: FetchDone) {
        self.log.borrow_mut().push(request.url.clone());
        let entry = self.entries.borrow().get(&request.url).cloned();
        let url = request.url;
        let delay = entry.as_ref().map_or(Duration::ZERO, |e| e.2);
        let id = browser.register_completion(done);
        browser.scheduler().set_timer(delay, false, true, move |b| {
            let result = match entry.clone() {
                Some((mime, body, _)) => {
                    Ok(Resource {
                        url: url.clone(),
                        status: 200,
                        mime,
                        charset: None,
                        body,
                        content_disposition: None,
                        header_csp: false,
                        referrer_policy: None,
                    })
                }
                None => Err(format!("net::ERR_FILE_NOT_FOUND {url}")),
            };
            b.complete_fetch(id, result);
        });
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScriptType {
    Classic,
    Module,
}

pub(crate) struct ResourceNode {
    pub(crate) dom: DomRef,
    pub(crate) node: NodeId,
}

impl Drop for ResourceNode {
    fn drop(&mut self) {
        self.dom.borrow_mut().release(self.node);
    }
}

struct PendingScript {
    element: Rc<ResourceNode>,
    ty: ScriptType,
    url: String,
    /// `None` until fetched (inline scripts start as `Some`).
    source: Option<Result<String, String>>,
}

enum ParsePhase {
    /// Whole body not yet fed.
    Feed(Vec<u8>),
    /// Body fed; continue the tokenizer.
    Resume,
    /// Flush the decoder at end of input.
    Finish,
    /// Flushed; continue until no more scripts, then end.
    FinishResume,
    Done,
}

/// Loading state of a frame's document.
pub(crate) struct DocumentLoad {
    pub loader_id: LoaderId,
    url: String,
    dom: DomRef,
    parser: Option<DocumentParser>,
    phase: ParsePhase,
    blocked: bool,
    stylesheet_blocked_script: Option<Rc<ResourceNode>>,
    deferred: VecDeque<Rc<RefCell<PendingScript>>>,
    /// A deferred module is evaluating; wait for its promise.
    awaiting_module: bool,
    parsing_done: bool,
    dcl_fired: bool,
    load_fired: bool,
    async_pending: usize,
    inline_module_seq: u32,
}

/// The realm's ES module source: fetches through the page's loader.
struct PageModuleFetcher {
    page: FrameKey,
    sched: lp_runtime::Scheduler<Browser>,
}

impl ModuleFetcher for PageModuleFetcher {
    fn document_base_url(&self, realm: &Realm) -> Option<String> {
        let page = lp_webapi::target::page_of(&realm.scope().ok()?).ok()?;
        let dom = page.dom.try_borrow().ok()?;
        dom.document_base_url(page.document()).ok().map(str::to_owned)
    }

    fn fetch(&self, realm: &Realm, url: String, request: ModuleRequest) {
        // Called from inside JS: defer the network request to a task, where
        // `&mut Browser` is available.
        let realm = realm.downgrade();
        let page = self.page;
        module_trace!("requested {url}");
        let request = RefCell::new(Some(request));
        self.sched.post(TaskSource::Networking, move |b| {
            let Some(request) = request.borrow_mut().take() else {
                module_trace!("task found no request {url}");
                return;
            };
            if realm.upgrade().is_none_or(|r| r.is_disposed()) {
                module_trace!("task skipped, realm gone {url}");
                return;
            }
            module_trace!("fetching {url} (document {})", b.doc(page).is_some());
            let traced = url.clone();
            let complete = move |_b: &mut Browser, res: Result<String, String>| {
                match &res {
                    Ok(text) => module_trace!("loaded {traced} ({} bytes)", text.len()),
                    Err(e) => module_trace!("failed {traced}: {e}"),
                }
                let Some(realm) = realm.upgrade() else {
                    module_trace!("dropped {traced}: realm gone");
                    return;
                };
                let Ok(scope) = realm.scope() else {
                    module_trace!("dropped {traced}: no scope");
                    return;
                };
                let r = match res {
                    Ok(text) => request.resolve(&scope, &text),
                    Err(e) => request.reject(&scope, &JsError::Type(e)),
                };
                match &r {
                    Ok(()) => module_trace!("handed to engine {traced}"),
                    Err(e) => module_trace!("engine rejected {traced}: {e}"),
                }
                if let Err(e) = r {
                    scope.report_exception(&e);
                }
            };
            // A modulepreload of this URL already fetched (or is fetching) it.
            if let Some(complete) = b.preloaded_module(page, &url, Box::new(complete)) {
                b.fetch_text(page, url, ResourceKind::ModuleScript, complete);
            }
        });
    }
}

/// `scheme://host[:port]` of an absolute URL, or `"null"`.
pub(crate) fn origin_of(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(u) => match u.origin() {
            o @ url::Origin::Tuple(..) => o.ascii_serialization(),
            url::Origin::Opaque(_) => "null".into(),
        },
        Err(_) => "null".into(),
    }
}

fn resolve_url(base: &str, href: &str) -> Option<String> {
    url::Url::parse(base).ok()?.join(href).ok().map(|u| u.to_string())
}

impl Browser {
    pub fn set_loader(&mut self, loader: Option<Rc<dyn ResourceLoader>>) {
        self.loader = loader;
    }

    /// Cancel and forget an uncommitted document independently of the active
    /// document's loader. Release the state guard before cancellation callbacks.
    pub(crate) fn cancel_pending_document(&mut self, page: FrameKey) {
        let Some(mut state) = self.doc_mut(page) else { return };
        let pending = state.pending_loader.take();
        let cancel = state.pending_cancel.take();
        state.pending_script = None;
        drop(state);
        drop(cancel);
        if let Some(loader) = pending {
            self.forget_network_activity(page, &loader);
        }
    }

    /// Cancels the uncommitted navigation when the frame's scripts started
    /// it (`Browser::document_reopened`), reported as an aborted one. Not
    /// `navigation_failed`: a child frame's opened document fires its own
    /// load events when it is closed.
    fn stop_script_navigation(&mut self, page: FrameKey) {
        let Some(state) = self.doc(page) else { return };
        let Some((loader_id, url)) = state.pending_script.clone().filter(|(id, _)| state.pending_loader() == Some(id))
        else {
            return;
        };
        let frame_id = state.main_frame.id.clone();
        drop(state);
        self.cancel_pending_document(page);
        self.emit(
            page.page,
            PageEvent::FrameNavigateFailed {
                frame_id,
                loader_id,
                url,
                error: "net::ERR_ABORTED".into(),
                cdp_request_id: None,
            },
        );
    }

    /// Starts fetching a document for a non-blank navigation.
    pub(crate) fn start_document_fetch(
        &mut self,
        page: FrameKey,
        opts: NavigateOpts,
        loader_id: LoaderId,
        history: HistoryHandling,
    ) -> Result<(), JsError> {
        let Some(loader) = self.loader_for(page) else {
            return Err(JsError::Unsupported("navigation without a resource loader"));
        };
        let frame_id = self.doc(page).map(|p| p.main_frame.id.0.clone());
        // Not an automation client's (`Page.navigate`) or a traversal, which
        // `document.open()` does not stop.
        let script = opts.cdp_request_id.is_none()
            && !matches!(opts.reason, crate::NavigationReason::AddressBar | crate::NavigationReason::History);
        if let Some(mut p) = self.doc_mut(page) {
            p.pending_loader = Some(loader_id.clone());
            p.pending_script = script.then(|| (loader_id.clone(), opts.url.clone()));
        }
        let req = FetchRequest {
            post: opts.post.clone(),
            url: opts.url.clone(),
            kind: if page.child.is_some() { ResourceKind::Iframe } else { ResourceKind::Document },
            page: page.page,
            referrer: opts.referrer.clone(),
            frame_id,
            loader_id: Some(loader_id.0.clone()),
            crossorigin: None,
        };
        let pending_id = loader_id.clone();
        let cancel = loader.fetch_cancellable(
            self,
            req,
            Box::new(move |b, res| {
                let current = b.doc(page).and_then(|p| p.pending_loader.clone());
                if current.as_ref() != Some(&loader_id) {
                    return; // superseded by another navigation
                }
                if let Some(mut p) = b.doc_mut(page) {
                    p.pending_loader = None;
                    if let Some(cancel) = p.pending_cancel.take() {
                        cancel.disarm();
                    }
                }
                match res {
                    Ok(resource)
                        if (200..300).contains(&resource.status)
                            && resource.content_disposition.as_deref().is_some_and(is_attachment) =>
                    {
                        // A Content-Disposition: attachment response turns the
                        // navigation into a download: nothing commits (Chrome
                        // reports net::ERR_ABORTED to Page.navigate).
                        b.navigation_failed(page, &opts, loader_id, "net::ERR_ABORTED".into());
                        if let Some(frame_id) = b.doc(page).map(|p| p.main_frame.id.clone()) {
                            let suggested = suggested_filename(
                                resource
                                    .content_disposition
                                    .as_deref()
                                    .and_then(disposition_filename)
                                    .as_deref()
                                    .unwrap_or(""),
                                &resource.url,
                            );
                            b.emit(page.page, PageEvent::DownloadStarted);
                            b.emit(
                                page.page,
                                PageEvent::Download {
                                    frame_id,
                                    url: resource.url,
                                    suggested_filename: suggested,
                                    body: Ok(bytes::Bytes::from(resource.body)),
                                },
                            );
                        }
                    }
                    Ok(resource) if (200..300).contains(&resource.status) || resource.status == 0 => {
                        let mut opts = opts;
                        opts.url = resource.url.clone();
                        if let Err(e) = b.begin_document(page, &opts, loader_id.clone(), resource, history) {
                            b.navigation_failed(page, &opts, loader_id, e.to_string());
                        }
                    }
                    Ok(resource) => {
                        // Error pages still commit in browsers; keep their body.
                        let mut opts = opts;
                        opts.url = resource.url.clone();
                        if let Err(e) = b.begin_document(page, &opts, loader_id.clone(), resource, history) {
                            b.navigation_failed(page, &opts, loader_id, e.to_string());
                        }
                    }
                    Err(e) => b.navigation_failed(page, &opts, loader_id, e),
                }
            }),
        );
        if let Some(mut d) = self.doc_mut(page).filter(|d| d.pending_loader.as_ref() == Some(&pending_id)) {
            d.pending_cancel = cancel;
        }
        Ok(())
    }

    pub(crate) fn navigation_failed(
        &mut self,
        page: FrameKey,
        opts: &NavigateOpts,
        loader_id: LoaderId,
        error: String,
    ) {
        let Some(frame_id) = self.doc(page).map(|p| p.main_frame.id.clone()) else { return };
        self.forget_network_activity(page, &loader_id);
        self.emit(
            page.page,
            PageEvent::FrameNavigateFailed {
                frame_id,
                loader_id,
                url: opts.url.clone(),
                error,
                cdp_request_id: opts.cdp_request_id,
            },
        );
        // A failed initial child navigation leaves its initial empty document
        // usable, but must release the parent's load-event blocker.
        if page.child.is_some() {
            self.finish_blank_document(page);
        }
    }

    pub(crate) fn begin_document(
        &mut self,
        page: FrameKey,
        opts: &NavigateOpts,
        loader_id: LoaderId,
        resource: Resource,
        history: HistoryHandling,
    ) -> Result<(), JsError> {
        let origin = origin_of(&resource.url);
        // The policy the navigation request was sent with (its initiator's).
        let policy = match &opts.referrer {
            Some(_) => self.doc(page).and_then(|p| p.main_frame.page_dom.as_ref().map(|d| d.referrer_policy())),
            None => None,
        };
        self.commit_document(page, opts, loader_id.clone(), &origin, history)?;
        let page_dom = self.doc(page).and_then(|p| p.main_frame.page_dom.clone()).ok_or(JsError::ContextDestroyed)?;
        page_dom.set_document_referrer(opts.referrer.as_deref(), policy.unwrap_or_default(), resource.referrer_policy);
        let dom = page_dom.dom.clone();
        let doc = page_dom.document();
        if resource.header_csp {
            let _ = dom.borrow_mut().set_header_csp(doc, true);
        }
        let mut parser = if resource.mime == "text/plain" {
            DocumentParser::new_plaintext(dom.clone(), doc)
        } else if lp_dom::parse::is_xml_mime(&resource.mime) {
            DocumentParser::new_xml(dom.clone(), doc, &resource.mime)
        } else {
            DocumentParser::new(dom.clone(), doc, !page_dom.scripting_disabled())
        }
        .map_err(|e| JsError::Type(format!("{e:?}")))?;
        parser.set_transport_charset(resource.charset.as_deref());
        let mut p = self.doc_mut(page).ok_or(JsError::Type("no such page".into()))?;
        p.main_frame.dom = Some(dom.clone());
        p.main_frame.document = Some(doc);
        p.nav.set_document(Some((dom.clone(), doc)));
        p.load = Some(DocumentLoad {
            loader_id,
            url: p.main_frame.base_url.clone(),
            dom,
            parser: Some(parser),
            phase: ParsePhase::Feed(resource.body),
            blocked: false,
            stylesheet_blocked_script: None,
            deferred: VecDeque::new(),
            awaiting_module: false,
            parsing_done: false,
            dcl_fired: false,
            load_fired: false,
            async_pending: 0,
            inline_module_seq: 0,
        });
        drop(p);
        self.scheduler().post(TaskSource::Networking, move |b| b.continue_parsing(page));
        Ok(())
    }

    fn load_mut(&mut self, page: FrameKey) -> Option<crate::frames::StateMut<'_, DocumentLoad>> {
        self.doc_mut(page)?.filter_map(|p| p.load.as_mut())
    }

    fn current_loader(&self, page: FrameKey) -> Option<LoaderId> {
        self.doc(page)?.load.as_ref().map(|l| l.loader_id.clone())
    }

    /// Runs the parser until it blocks on a script or finishes.
    fn continue_parsing(&mut self, page: FrameKey) {
        loop {
            let step = {
                let Some(mut guard) = self.load_mut(page) else { return };
                let load = &mut *guard;
                if load.blocked || load.parsing_done {
                    return;
                }
                let Some(parser) = load.parser.as_mut() else { return };
                match std::mem::replace(&mut load.phase, ParsePhase::Resume) {
                    ParsePhase::Feed(body) => parser.feed_bytes(&body),
                    ParsePhase::Resume => parser.resume(),
                    ParsePhase::Finish => {
                        load.phase = ParsePhase::FinishResume;
                        parser.finish_bytes()
                    }
                    ParsePhase::FinishResume => {
                        load.phase = ParsePhase::FinishResume;
                        parser.resume()
                    }
                    ParsePhase::Done => return,
                }
            };
            self.drain_dom_resources(page);
            match step {
                ParseStep::Script(node) => self.parser_reached_script(page, node),
                ParseStep::NeedInput => {
                    let mut load = self.load_mut(page).expect("load");
                    match load.phase {
                        ParsePhase::Resume => load.phase = ParsePhase::Finish,
                        _ => {
                            load.phase = ParsePhase::Done;
                            if let Some(mut p) = load.parser.take() {
                                p.end();
                            }
                            load.parsing_done = true;
                            drop(load);
                            if let Some(dom) = self.doc(page).and_then(|p| p.main_frame.page_dom.clone()) {
                                dom.set_parser_position(None);
                                lp_webapi::lifecycle::set_ready_state(&dom, dom.document(), "interactive");
                            }
                            self.run_deferred(page);
                            return;
                        }
                    }
                }
            }
        }
    }

    /// A script end tag, in the document or in markup one of its scripts
    /// wrote: it waits for blocking stylesheets, or is prepared.
    fn parser_reached_script(&mut self, page: FrameKey, node: NodeId) {
        let blocks = self
            .doc(page)
            .and_then(|p| p.main_frame.page_dom.clone())
            .and_then(|p| p.dom.borrow().script_info(node).ok())
            .is_some_and(|s| s.kind == ScriptKind::Classic && (s.src.is_none() || (!s.async_ && !s.defer)));
        // The parser is paused here: the script's ancestors are its open
        // elements, which an expect link's target must not be among.
        if let Some(bindings) = self.doc(page).and_then(|p| p.main_frame.page_dom.clone()) {
            bindings.set_parser_position(Some(node));
            bindings.update_render_blocking();
        }
        if blocks && self.stylesheets_block_scripts(page) {
            let mut load = self.load_mut(page).expect("load");
            if load.dom.borrow_mut().add_ref(node).is_ok() {
                load.stylesheet_blocked_script = Some(Rc::new(ResourceNode { dom: load.dom.clone(), node }));
                load.blocked = true;
                return;
            }
        }
        self.prepare_script(page, node);
    }

    /// "Prepare the script element" for a parser-inserted script.
    fn prepare_script(&mut self, page: FrameKey, node: NodeId) {
        let Some(load) = self.doc(page).and_then(|p| p.filter_map(|p| p.load.as_ref())) else { return };
        let dom = load.dom.clone();
        let base = load.url.clone();
        drop(load);
        // HTML "script end tag": a microtask checkpoint comes first, so
        // mutation observers are told about the parser's insertions up to
        // this script before it runs (no script is on the stack here, so the
        // queued delivery runs at once).
        if let Some(bindings) = self.doc(page).and_then(|p| p.main_frame.page_dom.clone()) {
            lp_webapi::mutation_observer::checkpoint(&bindings);
        }
        if self.doc(page).is_some_and(|d| d.main_frame.sandbox.scripts_disabled) {
            return;
        }
        let info = {
            let mut d = dom.borrow_mut();
            let Ok(info) = d.script_info(node) else { return };
            if info.already_started {
                return;
            }
            // Spec order: an element without source text, a disconnected one
            // (the script before it removed it) and a data block return
            // before "already started" is set, and are no longer
            // parser-inserted: a later insertion, or text inserted into the
            // element, prepares it like a script-created element.
            if (info.src.is_none() && info.inline_text.is_empty())
                || !d.is_connected(node).unwrap_or(false)
                || info.kind == ScriptKind::Data
            {
                let _ = d.clear_parser_inserted(node);
                return;
            }
            let _ = d.mark_script_started(node);
            info
        };
        if info.kind == ScriptKind::ImportMap {
            let map_base = {
                let d = dom.borrow();
                d.node_document(node).ok().and_then(|doc| d.document_base_url(doc).ok().map(str::to_owned))
            };
            self.add_import_map(page, &info, map_base.as_deref().unwrap_or(&base));
            return;
        }
        if dom.borrow_mut().add_ref(node).is_err() {
            return;
        }
        let element = Rc::new(ResourceNode { dom, node });
        let ty = match info.kind {
            ScriptKind::Classic if !info.nomodule => ScriptType::Classic,
            ScriptKind::Module => ScriptType::Module,
            // nomodule classic.
            _ => return,
        };
        let src_url = match info.src.as_deref() {
            Some(s) => match resolve_url(&base, s) {
                Some(u) => Some(u),
                None => return, // spec: error event (needs lp-webapi)
            },
            None => None,
        };
        if src_url.is_some() {
            // A parser-blocking script is implicitly render-blocking.
            self.block_rendering_on_script(page, node, ty == ScriptType::Classic && !info.async_ && !info.defer);
        }
        match (ty, src_url) {
            (ScriptType::Classic, None) => self.execute_at_insertion_point(page, &info.inline_text, &base, &element),
            (ScriptType::Classic, Some(url)) if info.async_ => self.fetch_async(page, ty, url, element),
            (ScriptType::Classic, Some(url)) if info.defer => self.fetch_deferred(page, ty, url, element),
            (ScriptType::Classic, Some(url)) => {
                // Parser-blocking: pause until fetched and executed.
                let loader_id = self.current_loader(page);
                if let Some(mut l) = self.load_mut(page) {
                    l.blocked = true;
                }
                let u = url.clone();
                self.fetch_text(page, url, ResourceKind::Script, move |b, res| {
                    if b.current_loader(page) != loader_id {
                        return;
                    }
                    if let Some(mut l) = b.load_mut(page) {
                        l.blocked = false;
                    }
                    match res {
                        Ok(text) => {
                            b.execute_at_insertion_point(page, &text, &u, &element);
                            b.script_loaded(page, &element, true);
                        }
                        Err(e) => {
                            b.report_load_error(page, &e);
                            b.script_loaded(page, &element, false);
                        }
                    }
                    b.continue_parsing(page);
                });
            }
            (ScriptType::Module, Some(url)) if info.async_ => self.fetch_async(page, ty, url, element),
            (ScriptType::Module, Some(url)) => self.fetch_deferred(page, ty, url, element),
            (ScriptType::Module, None) => {
                let url = {
                    let mut l = self.load_mut(page).expect("load");
                    l.inline_module_seq += 1;
                    // Distinct registry key per inline module (see lp_js::Scope::eval_module).
                    format!("{}#inline-module-{}", base.split('#').next().unwrap_or(&base), l.inline_module_seq)
                };
                if info.async_ {
                    self.execute_module(page, &info.inline_text, &url, false, Some(element));
                } else {
                    let ps = PendingScript { element, ty, url, source: Some(Ok(info.inline_text)) };
                    self.load_mut(page).expect("load").deferred.push_back(Rc::new(RefCell::new(ps)));
                }
            }
        }
    }

    /// Fetches `url` and decodes it as text; non-2xx statuses are errors.
    pub(crate) fn fetch_text(
        &mut self,
        page: FrameKey,
        url: String,
        kind: ResourceKind,
        done: impl FnOnce(&mut Browser, Result<String, String>) + 'static,
    ) {
        let Some(loader) = self.loader_for(page) else {
            self.scheduler().post(TaskSource::Networking, move |b| done(b, Err("no resource loader".into())));
            return;
        };
        let referrer = self.doc(page).map(|p| p.main_frame.url.clone());
        let frame_id = self.doc(page).map(|p| p.main_frame.id.0.clone());
        // Subresources carry the document's loader id (CDP Network.* routing).
        let loader_id = self.doc(page).map(|p| p.main_frame.loader_id.0.clone());
        let req =
            FetchRequest { post: None, url, kind, page: page.page, referrer, frame_id, loader_id, crossorigin: None };
        loader.fetch(
            self,
            req,
            Box::new(move |b, res| {
                let res = match res {
                    Ok(r) if (200..300).contains(&r.status) => Ok(r.text()),
                    Ok(r) => Err(format!("HTTP {} for {}", r.status, r.url)),
                    Err(e) => Err(e),
                };
                done(b, res)
            }),
        );
    }

    fn fetch_deferred(&mut self, page: FrameKey, ty: ScriptType, url: String, element: Rc<ResourceNode>) {
        let ps = Rc::new(RefCell::new(PendingScript { element, ty, url: url.clone(), source: None }));
        self.load_mut(page).expect("load").deferred.push_back(ps.clone());
        let kind = if ty == ScriptType::Module { ResourceKind::ModuleScript } else { ResourceKind::Script };
        let loader_id = self.current_loader(page);
        self.fetch_text(page, url, kind, move |b, res| {
            if b.current_loader(page) != loader_id {
                return;
            }
            ps.borrow_mut().source = Some(res);
            b.run_deferred(page);
        });
    }

    /// Fetches a script of the "set of scripts that will execute as soon as
    /// possible" (parser-inserted `async`, or script-inserted external) and
    /// runs it when it arrives. While it is in flight it delays the `load`
    /// event of a document that is still loading; a document without a
    /// `DocumentLoad` (about:blank, a written document) has no such gate.
    fn fetch_async(&mut self, page: FrameKey, ty: ScriptType, url: String, element: Rc<ResourceNode>) {
        let loader_id = self.frame_loader(page);
        if loader_id.is_none() {
            return;
        }
        let counted = match self.load_mut(page) {
            Some(mut l) => {
                l.async_pending += 1;
                true
            }
            None => false,
        };
        let kind = if ty == ScriptType::Module { ResourceKind::ModuleScript } else { ResourceKind::Script };
        let u = url.clone();
        self.fetch_text(page, url, kind, move |b, res| {
            if b.frame_loader(page) != loader_id {
                return;
            }
            match res {
                Ok(text) if ty == ScriptType::Classic => {
                    b.execute_external(page, &text, &u, &element);
                    b.script_loaded(page, &element, true);
                }
                Ok(text) => {
                    b.execute_module(page, &text, &u, false, Some(element));
                }
                Err(e) => {
                    b.report_load_error(page, &e);
                    b.script_loaded(page, &element, false);
                }
            }
            // The script or its `load`/`error` handler may have replaced the
            // document: its load is not the one counted.
            if b.frame_loader(page) != loader_id {
                return;
            }
            if counted {
                if let Some(mut l) = b.load_mut(page) {
                    l.async_pending -= 1;
                }
            }
            b.maybe_fire_load(page);
        });
    }

    /// The loader of the document `page` shows (`None` without a document).
    fn frame_loader(&self, page: FrameKey) -> Option<LoaderId> {
        self.doc(page).map(|p| p.main_frame.loader_id.clone())
    }

    /// `load` (`ok`) or `error` on a script element, after it executed or its
    /// fetch failed. Dispatched in the loader's task, as the spec's "execute
    /// the script element" does.
    fn script_loaded(&self, page: FrameKey, element: &ResourceNode, ok: bool) {
        if let Some(bindings) = self.doc(page).and_then(|p| p.main_frame.page_dom.clone()) {
            bindings.unblock_rendering(element.node);
            lp_webapi::lifecycle::resource_loaded(&bindings, element.node, ok);
        }
    }

    /// HTML "block rendering" on a script whose fetch starts, if it is
    /// potentially render-blocking: `implicit`ly (a parser-blocking script)
    /// or through `blocking=render`. `script_loaded` unblocks it.
    fn block_rendering_on_script(&self, page: FrameKey, node: NodeId, implicit: bool) {
        let Some(bindings) = self.doc(page).and_then(|p| p.main_frame.page_dom.clone()) else { return };
        let explicit = bindings
            .dom
            .borrow()
            .get_attribute(node, "blocking")
            .ok()
            .flatten()
            .is_some_and(|v| v.split_ascii_whitespace().any(|t| t.eq_ignore_ascii_case("render")));
        if implicit {
            bindings.block_rendering(node, lp_webapi::RenderBlocker::Implicit);
        } else if explicit {
            bindings.block_rendering(node, lp_webapi::RenderBlocker::Explicit);
        }
    }

    /// "Prepare the script element" for a script the parser did not insert:
    /// a connected `<script>` inserted by script (or moved, or cloned), or
    /// one whose `src` was set while connected. The DOM reports these as
    /// `ResourceKind::Script` resource events; parser-inserted scripts are
    /// driven by `ParseStep::Script` instead and never arrive here (one whose
    /// `src` changes after it was prepared is "already started").
    pub(crate) fn script_event(&mut self, page: FrameKey, node: NodeId) {
        let Some(state) = self.doc(page) else { return };
        let Some(bindings) = state.main_frame.page_dom.clone() else { return };
        if state.main_frame.sandbox.scripts_disabled {
            return;
        }
        let base = state.main_frame.base_url.clone();
        drop(state);
        let dom = bindings.dom.clone();
        let info = {
            let mut d = dom.borrow_mut();
            let Ok(info) = d.script_info(node) else { return };
            // Spec order: an already-started element, an inline element
            // without source text, a disconnected element, and an unknown
            // type return before "already started" is set.
            if info.already_started || !d.is_connected(node).unwrap_or(false) {
                return;
            }
            if info.src.is_none() && info.inline_text.is_empty() {
                return;
            }
            if info.kind == ScriptKind::Data {
                return;
            }
            let _ = d.mark_script_started(node);
            info
        };
        if info.kind == ScriptKind::ImportMap {
            let map_base = {
                let d = dom.borrow();
                d.node_document(node).ok().and_then(|doc| d.document_base_url(doc).ok().map(str::to_owned))
            };
            self.add_import_map(page, &info, map_base.as_deref().unwrap_or(&base));
            return;
        }
        if dom.borrow_mut().add_ref(node).is_err() {
            return;
        }
        let element = Rc::new(ResourceNode { dom, node });
        let ty = match info.kind {
            ScriptKind::Classic if !info.nomodule => ScriptType::Classic,
            ScriptKind::Module => ScriptType::Module,
            _ => return, // nomodule classic: already started, never run
        };
        let src_url = match info.src.as_deref() {
            None => None,
            Some(s) if s.is_empty() => {
                self.script_loaded(page, &element, false);
                return;
            }
            Some(s) => match resolve_url(&base, s) {
                Some(u) => Some(u),
                None => {
                    self.script_loaded(page, &element, false);
                    return;
                }
            },
        };
        match (ty, src_url) {
            // Spec: runs immediately, at insertion; here once the inserting
            // script (or task) returns. While the parser is parsing (not
            // paused on a script it waits for) the inserting script was one
            // it ran, so `document.write` has that insertion point; otherwise
            // there is none.
            (ScriptType::Classic, None) => {
                let parsing = self.load_mut(page).is_some_and(|l| l.parser.is_some() && !l.blocked);
                if parsing {
                    self.execute_at_insertion_point(page, &info.inline_text, &base, &element);
                } else {
                    self.execute_classic(page, &info.inline_text, &base, &element);
                }
                // Scripts it inserted are prepared before the drain returns.
                self.drain_dom_resources(page);
            }
            // External classic scripts inserted by script are "force async":
            // `defer` is ignored and they run as soon as they arrive.
            (ScriptType::Classic, Some(url)) | (ScriptType::Module, Some(url)) => {
                self.block_rendering_on_script(page, node, false);
                self.fetch_async(page, ty, url, element)
            }
            (ScriptType::Module, None) => {
                let url = dynamic_inline_module_url(&base);
                self.execute_module(page, &info.inline_text, &url, false, Some(element));
                self.drain_dom_resources(page);
            }
        }
    }

    /// Runs ready deferred scripts in order once parsing is done, then fires
    /// `DOMContentLoaded`.
    fn run_deferred(&mut self, page: FrameKey) {
        loop {
            self.drain_dom_resources(page);
            let css_blocked = self.stylesheets_block_scripts(page);
            let Some(mut load) = self.load_mut(page) else { return };
            if !load.parsing_done || load.awaiting_module || load.dcl_fired {
                return;
            }
            let Some(next) = load.deferred.front().cloned() else { break };
            if css_blocked {
                return;
            }
            let source = next.borrow().source.clone();
            let Some(source) = source else { return }; // still fetching
            load.deferred.pop_front();
            drop(load);
            let (ty, url, element) = {
                let n = next.borrow();
                (n.ty, n.url.clone(), n.element.clone())
            };
            match source {
                Ok(text) if ty == ScriptType::Classic => {
                    self.execute_external(page, &text, &url, &element);
                    self.script_loaded(page, &element, true);
                }
                Ok(text) => {
                    if self.execute_module(page, &text, &url, true, Some(element)) {
                        return; // resumes when the module's promise settles
                    }
                }
                Err(e) => {
                    self.report_load_error(page, &e);
                    self.script_loaded(page, &element, false);
                }
            }
        }
        self.fire_dom_content_loaded(page);
    }

    /// HTML "download the hyperlink": blob:/data: bytes arrive with the
    /// request; same-origin HTTP(S) URLs are fetched. The result is reported
    /// as `PageEvent::Download` for the embedder (CDP download behavior).
    pub(crate) fn start_download(&mut self, page: FrameKey, request: lp_webapi::DownloadRequest) {
        let Some(frame_id) = self.doc(page).map(|p| p.main_frame.id.clone()) else { return };
        let suggested = suggested_filename(&request.filename, &request.url);
        let attribute = request.filename.clone();
        let lp_webapi::DownloadRequest { url, data, .. } = request;
        self.emit(page.page, PageEvent::DownloadStarted);
        match data {
            Some((bytes, _mime)) => {
                self.emit(
                    page.page,
                    PageEvent::Download { frame_id, url, suggested_filename: suggested, body: Ok(bytes) },
                );
            }
            None => {
                let Some(loader) = self.loader_for(page) else { return };
                let referrer = self.doc(page).map(|p| p.main_frame.url.clone());
                let loader_id = self.doc(page).map(|p| p.main_frame.loader_id.0.clone());
                let req = FetchRequest {
                    post: None,
                    url: url.clone(),
                    kind: ResourceKind::Download,
                    page: page.page,
                    referrer,
                    frame_id: Some(frame_id.0.clone()),
                    loader_id,
                    crossorigin: None,
                };
                loader.fetch(
                    self,
                    req,
                    Box::new(move |b, res| {
                        // A Content-Disposition filename takes precedence over
                        // the download attribute.
                        let mut suggested = suggested;
                        let body = match res {
                            Ok(r) if (200..300).contains(&r.status) => {
                                if let Some(name) = r.content_disposition.as_deref().and_then(disposition_filename) {
                                    suggested = suggested_filename(&name, &r.url);
                                } else if attribute.is_empty() {
                                    suggested = suggested_filename("", &r.url);
                                }
                                Ok(bytes::Bytes::from(r.body))
                            }
                            Ok(r) => Err(format!("HTTP {} for {}", r.status, r.url)),
                            Err(e) => Err(e),
                        };
                        b.emit(page.page, PageEvent::Download { frame_id, url, suggested_filename: suggested, body });
                    }),
                );
            }
        }
    }

    /// `document.open()` on the loaded document: it is loading again, so
    /// DOMContentLoaded/load (and their CDP lifecycle events) can fire anew.
    /// A navigation the frame's scripts started before it, still fetching,
    /// is cancelled (HTML "stop loading"), as Chrome cancels one; one an
    /// automation client started commits. Runs as a task queued by the
    /// open, so a navigation a script queued after it has not started yet.
    pub(crate) fn document_reopened(&mut self, page: FrameKey) {
        self.stop_script_navigation(page);
        let Some(mut state) = self.doc_mut(page) else { return };
        state.main_frame.dom_content_loaded = false;
        state.main_frame.loaded = false;
        if let Some(load) = &mut state.load {
            load.dcl_fired = false;
            load.load_fired = false;
        }
        drop(state);
        self.sync_root(page.page);
    }

    /// `document.close()` ended the script-created parser: "the end" steps
    /// after readyState became interactive.
    pub(crate) fn finish_written_document(&mut self, page: FrameKey) {
        let Some(mut state) = self.doc_mut(page) else { return };
        if state.main_frame.dom_content_loaded {
            return;
        }
        if let Some(load) = &mut state.load {
            load.dcl_fired = true;
        }
        state.main_frame.dom_content_loaded = true;
        let frame_id = state.main_frame.id.clone();
        let loader_id = state.main_frame.loader_id.clone();
        let bindings = state.main_frame.page_dom.clone();
        drop(state);
        self.sync_root(page.page);
        if let Some(dom) = bindings {
            lp_webapi::lifecycle::dom_content_loaded(&dom, dom.document());
        }
        self.emit(page.page, PageEvent::FrameDomContentLoaded { frame_id, loader_id, timestamp: self.timestamp() });
        self.maybe_fire_load(page);
    }

    fn fire_dom_content_loaded(&mut self, page: FrameKey) {
        let Some(mut load) = self.load_mut(page) else { return };
        if load.dcl_fired {
            return;
        }
        load.dcl_fired = true;
        let loader_id = load.loader_id.clone();
        drop(load);
        let frame_id = self.doc(page).expect("page").main_frame.id.clone();
        self.doc_mut(page).expect("frame").main_frame.dom_content_loaded = true;
        self.sync_root(page.page);
        if let Some(dom) = self.doc(page).and_then(|p| p.main_frame.page_dom.clone()) {
            lp_webapi::lifecycle::dom_content_loaded(&dom, dom.document());
        }
        self.emit(page.page, PageEvent::FrameDomContentLoaded { frame_id, loader_id, timestamp: self.timestamp() });
        self.maybe_fire_load(page);
    }

    pub(crate) fn maybe_fire_load(&mut self, page: FrameKey) {
        self.drain_dom_resources(page);
        self.fire_load_if_ready(page);
    }

    /// `maybe_fire_load` for a caller that has already drained DOM resources
    /// (the checkpoint drains once per page, then visits every frame).
    pub(crate) fn fire_load_if_ready(&mut self, page: FrameKey) {
        // Runs at every checkpoint: a loaded frame has nothing left to fire.
        if self.doc(page).is_none_or(|d| d.main_frame.loaded) {
            return;
        }
        let Some(host) = self.page(page.page).map(|p| p.frames.clone()) else { return };
        if host
            .child_keys(page)
            .iter()
            .any(|k| self.doc(*k).is_some_and(|d| !d.main_frame.loaded || d.pending_loader.is_some()))
        {
            return;
        }
        let Some(mut state) = self.doc_mut(page) else { return };
        if state.main_frame.loaded
            || !state.main_frame.dom_content_loaded
            || state.stylesheets.pending() != 0
            || state.images.blocking_load() != 0
        {
            return;
        }
        if let Some(load) = &mut state.load {
            if load.async_pending > 0 || !load.dcl_fired {
                return;
            }
            load.load_fired = true;
        }
        state.main_frame.loaded = true;
        let frame = state.main_frame.clone();
        drop(state);
        self.sync_root(page.page);
        if let Some(dom) = frame.page_dom {
            lp_webapi::lifecycle::load(&dom, dom.document());
        }
        if !host.matches(page, &frame.loader_id) {
            return;
        }
        self.emit(
            page.page,
            PageEvent::FrameLoaded {
                frame_id: frame.id,
                loader_id: frame.loader_id.clone(),
                timestamp: self.timestamp(),
            },
        );
        self.watch_network_idle(page, frame.loader_id);
        if let Some(parent) = frame.parent.and_then(|id| host.key(&id)) {
            if let Some(owner) = frame.owner {
                if let Some(dom) = host.frame(parent).and_then(|f| f.page_dom) {
                    lp_webapi::lifecycle::resource_loaded(&dom, owner, true);
                }
            }
            self.maybe_fire_load(parent);
        }
    }

    /// A stylesheet completed or stopped blocking. Called from a later task,
    /// never recursively from the parser's resource-event drain.
    pub(crate) fn stylesheets_changed(&mut self, page: FrameKey) {
        self.drain_dom_resources(page);
        if !self.stylesheets_block_scripts(page) {
            let pending = self.load_mut(page).and_then(|mut l| l.stylesheet_blocked_script.take());
            if let Some(element) = pending {
                self.load_mut(page).expect("load").blocked = false;
                self.prepare_script(page, element.node);
                self.continue_parsing(page);
            }
            self.run_deferred(page);
        }
        self.maybe_fire_load(page);
    }

    fn main_realm(&self, page: FrameKey) -> Option<Realm> {
        self.doc(page)?.main_frame.main_realm().cloned()
    }

    fn execute_classic(&mut self, page: FrameKey, text: &str, url: &str, element: &ResourceNode) {
        let Some(realm) = self.main_realm(page) else { return };
        let Ok(scope) = realm.scope() else { return };
        let bindings = self.doc(page).and_then(|p| p.main_frame.page_dom.clone());
        // HTML "execute the script element": a script moved to another
        // document since it was prepared does not run.
        if let Some(dom) = &bindings {
            if dom.dom.borrow().node_document(element.node).ok() != Some(dom.document()) {
                return;
            }
        }
        let outer = bindings.as_ref().and_then(|dom| dom.current_script());
        if let Some(dom) = &bindings {
            dom.set_current_script(Some(element.node));
        }
        if let Err(e) = scope.eval(text, Some(url), 1) {
            scope.report_exception(&e);
        }
        if let Some(dom) = &bindings {
            dom.set_current_script(outer);
        }
    }

    /// Registers an import map; its problems go to the console or `error`
    /// reporting and leave the rest of the page unaffected.
    fn add_import_map(&mut self, page: FrameKey, info: &lp_dom::script::ScriptInfo, base: &str) {
        let Some(realm) = self.main_realm(page) else { return };
        let Ok(scope) = realm.scope() else { return };
        // Spec: an external import map is an error event; there is no fetch.
        if info.src.is_some() {
            scope.console_warn("External import maps are not supported: <script type=\"importmap\"> ignored its src.");
            return;
        }
        match scope.add_import_map(&info.inline_text, base) {
            Ok(warnings) => warnings.iter().for_each(|w| scope.console_warn(&format!("Import map: {w}"))),
            Err(JsError::Unsupported(what)) => {
                scope.console_warn(&format!("Ignored <script type=\"importmap\">: {what}"))
            }
            Err(e) => scope.report_exception(&e),
        }
    }

    /// Runs a script the network parser reached with the parser lent to the
    /// page, so `document.write` parses at the insertion point, then prepares
    /// the external and module scripts that markup contained.
    fn execute_at_insertion_point(&mut self, page: FrameKey, text: &str, url: &str, element: &ResourceNode) {
        let bindings = self.doc(page).and_then(|p| p.main_frame.page_dom.clone());
        let lent = self.load_mut(page).and_then(|mut l| {
            let parser = l.parser.take()?;
            Some((parser, l.loader_id.clone()))
        });
        let (Some(bindings), Some((parser, loader_id))) = (bindings, lent) else {
            self.execute_classic(page, text, url, element);
            return;
        };
        bindings.lend_network_parser(parser);
        self.execute_classic(page, text, url, element);
        let parser = bindings.reclaim_network_parser();
        // The script may have started another document's load.
        match self.load_mut(page) {
            Some(mut load) if load.loader_id == loader_id => load.parser = parser,
            _ => return,
        }
        for node in bindings.take_written_scripts() {
            self.parser_reached_script(page, node);
        }
        // Scripts this script inserted run before the parser resumes.
        self.drain_dom_resources(page);
    }

    /// Runs an external classic script outside the parser: it has no insertion
    /// point, so its `document.write`s are ignored.
    fn execute_external(&mut self, page: FrameKey, text: &str, url: &str, element: &ResourceNode) {
        match self.doc(page).and_then(|p| p.main_frame.page_dom.clone()) {
            Some(bindings) => {
                bindings.with_destructive_writes_ignored(|| self.execute_classic(page, text, url, element))
            }
            None => self.execute_classic(page, text, url, element),
        }
    }

    /// External and module scripts reached in markup written to the
    /// script-created parser (`PageHooks::written_script_queued`). There may
    /// be no `DocumentLoad`: about:blank has none.
    pub(crate) fn prepare_written_scripts(&mut self, page: FrameKey) {
        let Some(bindings) = self.doc(page).and_then(|p| p.main_frame.page_dom.clone()) else { return };
        for node in bindings.take_written_scripts() {
            self.prepare_written_script(page, &bindings, node);
        }
    }

    fn prepare_written_script(&mut self, page: FrameKey, bindings: &Rc<lp_webapi::PageDom>, node: NodeId) {
        let dom = bindings.dom.clone();
        let info = {
            let mut d = dom.borrow_mut();
            let Ok(info) = d.script_info(node) else { return };
            let _ = d.mark_script_started(node);
            info
        };
        let base = self.doc(page).map(|p| p.main_frame.base_url.clone()).unwrap_or_default();
        let src_url = info.src.as_deref().and_then(|s| resolve_url(&base, s));
        let blocking = bindings.written_script_pending() == Some(node);
        let Some(url) = src_url else {
            if blocking {
                lp_webapi::lifecycle::written_script_fetched(bindings, node, "", Err("invalid URL".into()));
            } else if info.src.is_none() && info.kind == ScriptKind::Module {
                let url = dynamic_inline_module_url(&base);
                let element =
                    dom.borrow_mut().add_ref(node).ok().map(|_| Rc::new(ResourceNode { dom: dom.clone(), node }));
                self.execute_module(page, &info.inline_text, &url, false, element);
            }
            return;
        };
        if blocking {
            let bindings = bindings.clone();
            let u = url.clone();
            self.fetch_text(page, url, ResourceKind::Script, move |_, res| {
                lp_webapi::lifecycle::written_script_fetched(&bindings, node, &u, res);
            });
            return;
        }
        if dom.borrow_mut().add_ref(node).is_err() {
            return;
        }
        let element = Rc::new(ResourceNode { dom, node });
        let module = info.kind == ScriptKind::Module;
        let kind = if module { ResourceKind::ModuleScript } else { ResourceKind::Script };
        let u = url.clone();
        let current = bindings.clone();
        self.fetch_text(page, url, kind, move |b, res| {
            if !b.doc(page).and_then(|p| p.main_frame.page_dom.clone()).is_some_and(|d| Rc::ptr_eq(&d, &current)) {
                return;
            }
            match res {
                Ok(text) if module => {
                    b.execute_module(page, &text, &u, false, Some(element));
                }
                Ok(text) => {
                    b.execute_external(page, &text, &u, &element);
                    b.script_loaded(page, &element, true);
                }
                Err(e) => {
                    b.report_load_error(page, &e);
                    b.script_loaded(page, &element, false);
                }
            }
        });
    }

    /// Starts a module; with `deferred`, parsing-completion work waits for its
    /// evaluation promise, otherwise the `load` event does. Returns whether
    /// such a wait was set up. `element` gets `load` when the promise
    /// fulfills and `error` when evaluation cannot start or rejects.
    fn execute_module(
        &mut self,
        page: FrameKey,
        text: &str,
        url: &str,
        deferred: bool,
        element: Option<Rc<ResourceNode>>,
    ) -> bool {
        let Some(realm) = self.main_realm(page) else { return false };
        let Ok(scope) = realm.scope() else { return false };
        let bindings = self.doc(page).and_then(|p| p.main_frame.page_dom.clone());
        let evaluated = match &bindings {
            Some(dom) => dom.with_destructive_writes_ignored(|| scope.eval_module(text, url)),
            None => scope.eval_module(text, url),
        };
        let promise = match evaluated {
            Ok(p) => p,
            Err(e) => {
                scope.report_exception(&e);
                if let Some(element) = &element {
                    self.script_loaded(page, element, false);
                }
                return false;
            }
        };
        let loader_id = self.frame_loader(page);
        let sched = self.scheduler();
        let counted = self.load_mut(page).is_some();
        let resume = move |ok: bool| {
            let loader_id = loader_id.clone();
            let element = element.clone();
            sched.post(TaskSource::Networking, move |b| {
                if b.frame_loader(page) != loader_id {
                    return;
                }
                if let Some(element) = &element {
                    b.script_loaded(page, element, ok);
                }
                // The handler may have replaced the document.
                if b.frame_loader(page) != loader_id {
                    return;
                }
                if counted {
                    if let Some(mut l) = b.load_mut(page) {
                        if deferred {
                            l.awaiting_module = false;
                        } else {
                            l.async_pending -= 1;
                        }
                    }
                }
                if deferred {
                    b.run_deferred(page);
                } else {
                    b.maybe_fire_load(page);
                }
            });
        };
        let resume = Rc::new(resume);
        let (ok, err) = (resume.clone(), resume);
        let r = scope.then(
            promise,
            move |_s, _v| ok(true),
            move |s, e| {
                s.report_exception(&JsError::Thrown(Persistent::new(s, e)));
                err(false)
            },
        );
        if let Err(e) = r {
            scope.report_exception(&e);
            return false;
        }
        if let Some(mut l) = self.load_mut(page) {
            if deferred {
                l.awaiting_module = true;
            } else {
                l.async_pending += 1;
            }
        }
        true
    }

    fn report_load_error(&mut self, page: FrameKey, error: &str) {
        let Some(realm) = self.main_realm(page) else { return };
        let Ok(scope) = realm.scope() else { return };
        lp_webapi::lifecycle::report_diagnostic(&scope, &JsError::Type(format!("Failed to load script: {error}")));
    }
}

/// The synchronous part of "prepare the script element" for a script the
/// parser did not insert (`PageHooks::script_connected`, a post-connection
/// step): an inline classic script executes here, before the DOM call that
/// connected it (or gave it its text) returns. External scripts, modules and
/// import maps need the loader and stay queued as `ResourceKind::Script`
/// resource events for `Browser::script_event`, which finds an element this
/// function ran "already started".
pub(crate) fn prepare_inserted_script(frame: &crate::Frame, node: NodeId) {
    if frame.sandbox.scripts_disabled {
        return;
    }
    let (Some(bindings), Some(realm)) = (frame.page_dom.clone(), frame.main_realm().cloned()) else { return };
    let text = {
        // Script never runs under a DOM borrow: a held one defers to the drain.
        let Ok(mut dom) = bindings.dom.try_borrow_mut() else { return };
        let Ok(info) = dom.script_info(node) else { return };
        if info.already_started || info.parser_inserted || !dom.is_connected(node).unwrap_or(false) {
            return;
        }
        if info.src.is_some() || info.inline_text.is_empty() || info.kind != ScriptKind::Classic || info.nomodule {
            return;
        }
        let _ = dom.mark_script_started(node);
        info.inline_text
    };
    let Ok(scope) = realm.scope() else { return };
    let outer = bindings.current_script();
    bindings.set_current_script(Some(node));
    realm.reenter_native_callback_hook(|| {
        if let Err(e) = scope.eval(&text, Some(&frame.base_url), 1) {
            scope.report_exception(&e);
        }
    });
    bindings.set_current_script(outer);
}

/// Registry key of an inline module the parser did not reach (script-inserted
/// or written). JSC never evaluates a URL twice and node indices are reused
/// after DOM garbage collection, so the key is a process-wide sequence under a
/// prefix the parser's `#inline-module-N` keys never use.
fn dynamic_inline_module_url(base: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    format!("{}#inline-module-dyn-{n}", base.split('#').next().unwrap_or(base))
}

pub(crate) fn install_module_fetcher(sched: lp_runtime::Scheduler<Browser>, page: FrameKey, realm: &Realm) {
    realm.set_module_fetcher(Some(Rc::new(PageModuleFetcher { page, sched })));
}

/// The download's file name: the `download` attribute without path
/// separators, else the URL's last path segment, else "download".
fn suggested_filename(attribute: &str, url: &str) -> String {
    let clean = |name: &str| -> String {
        let name: String = name.chars().map(|c| if matches!(c, '/' | '\\' | '\0') { '_' } else { c }).collect();
        let name = name.trim().trim_matches('.').to_owned();
        name
    };
    let from_attribute = clean(attribute);
    if !from_attribute.is_empty() {
        return from_attribute;
    }
    if !url.starts_with("blob:") && !url.starts_with("data:") {
        if let Some(segment) = url::Url::parse(url).ok().and_then(|u| u.path_segments()?.next_back().map(str::to_owned))
        {
            let segment = clean(&segment);
            if !segment.is_empty() {
                return segment;
            }
        }
    }
    "download".into()
}

/// Whether a Content-Disposition value has the `attachment` type.
fn is_attachment(value: &str) -> bool {
    value.split(';').next().is_some_and(|t| t.trim().eq_ignore_ascii_case("attachment"))
}

/// The file name of a Content-Disposition value (RFC 6266): `filename*`
/// (RFC 8187 `charset'lang'pct-encoded`, UTF-8 or ISO-8859-1) wins over
/// `filename` (token or quoted-string).
fn disposition_filename(value: &str) -> Option<String> {
    let mut plain = None;
    let mut extended = None;
    let mut rest = value.split_once(';').map(|(_, r)| r)?;
    while !rest.trim().is_empty() {
        let (name, after) = rest.split_once('=')?;
        let name = name.trim().to_ascii_lowercase();
        let after = after.trim_start();
        let (val, next) = if let Some(quoted) = after.strip_prefix('"') {
            let mut out = String::new();
            let mut chars = quoted.char_indices();
            let mut end = quoted.len();
            while let Some((i, c)) = chars.next() {
                match c {
                    '\\' => {
                        if let Some((_, n)) = chars.next() {
                            out.push(n);
                        }
                    }
                    '"' => {
                        end = i + 1;
                        break;
                    }
                    c => out.push(c),
                }
            }
            let next = quoted[end..].split_once(';').map_or("", |(_, r)| r);
            (out, next)
        } else {
            let (v, next) = after.split_once(';').unwrap_or((after, ""));
            (v.trim().to_owned(), next)
        };
        match name.as_str() {
            "filename*" => {
                let mut parts = val.splitn(3, '\'');
                if let (Some(charset), Some(_lang), Some(encoded)) = (parts.next(), parts.next(), parts.next()) {
                    let bytes: Vec<u8> = percent_encoding::percent_decode_str(encoded).collect();
                    extended = if charset.eq_ignore_ascii_case("utf-8") {
                        String::from_utf8(bytes).ok()
                    } else if charset.eq_ignore_ascii_case("iso-8859-1") {
                        Some(bytes.iter().map(|&b| b as char).collect())
                    } else {
                        None
                    };
                }
            }
            "filename" => plain = Some(val),
            _ => {}
        }
        rest = next;
    }
    extended.or(plain).filter(|n| !n.is_empty())
}

#[cfg(test)]
mod disposition_tests {
    use super::{disposition_filename, is_attachment, suggested_filename};

    #[test]
    fn content_disposition_names() {
        assert!(is_attachment("attachment"));
        assert!(is_attachment(" Attachment ; filename=a.txt"));
        assert!(!is_attachment("inline; filename=a.txt"));
        assert_eq!(disposition_filename("attachment"), None);
        assert_eq!(disposition_filename("attachment; filename=plain.csv").as_deref(), Some("plain.csv"));
        assert_eq!(
            disposition_filename(r#"attachment; filename="a \"q\" b;c.txt"; size=3"#).as_deref(),
            Some(r#"a "q" b;c.txt"#)
        );
        // filename* wins regardless of order.
        assert_eq!(
            disposition_filename("attachment; filename*=UTF-8''r%C3%A9port.xlsx; filename=\"report.xlsx\"").as_deref(),
            Some("réport.xlsx")
        );
        assert_eq!(disposition_filename("attachment; filename*=iso-8859-1'en'%E9t%E9.txt").as_deref(), Some("été.txt"));
        // Unknown charset: fall back to filename.
        assert_eq!(disposition_filename("attachment; filename*=x-weird''a; filename=b").as_deref(), Some("b"));
        assert_eq!(suggested_filename("../../etc/passwd", "http://h/x"), "_.._etc_passwd");
        assert_eq!(suggested_filename("", "http://h/dir/file.pdf?x=1"), "file.pdf");
        assert_eq!(suggested_filename("", "blob:http://h/uuid"), "download");
    }
}
