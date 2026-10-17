//! Document loading: fetch → parse → scripts → DOMContentLoaded → load.
//!
//! Script handling follows HTML "prepare the script element" for
//! parser-inserted scripts (classification by `lp_dom::Dom::script_info`):
//! * inline classic: runs immediately (parser-blocking);
//! * external classic, no async/defer: parser-blocking fetch;
//! * external classic `defer`, and non-async module scripts: run in document
//!   order after parsing, before `DOMContentLoaded`;
//! * `async` (classic or module): run when ready, before `load`;
//! * `nomodule` classic scripts, import maps and data blocks: not executed
//!   (import maps are unsupported, see docs/PARITY.md).
//!
//! Deviation: a deferred module's evaluation promise (including top-level
//! await) is awaited before the next deferred script/`DOMContentLoaded`,
//! because JSC fetches the module graph during evaluation, not before it.
//!
//! DOM lifecycle and currentScript are wired through lp-webapi. Script element
//! load/error events, script-inserted scripts, and external stylesheet/iframe/
//! image resources remain separate loader work.

use crate::{Browser, LoaderId, NavigateOpts, PageEvent, PageId};
use lp_dom::parse::{DocumentParser, DomRef, ParseStep};
use lp_dom::script::ScriptKind;
use lp_dom::NodeId;
use lp_js::{JsError, ModuleFetcher, ModuleRequest, Persistent, Realm};
use lp_runtime::TaskSource;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    Document,
    Script,
    ModuleScript,
    Stylesheet,
    Iframe,
    Image,
}

#[derive(Debug, Clone)]
pub struct FetchRequest {
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
        };
        let mut req = lp_net::Request::get(url, destination);
        if request.kind == ResourceKind::ModuleScript || request.crossorigin.is_some() {
            req.mode = lp_net::RequestMode::Cors;
        }
        if request.crossorigin.as_deref().is_some_and(|v| v.eq_ignore_ascii_case("use-credentials")) {
            req.credentials = lp_net::Credentials::Include;
        }
        if !matches!(request.kind, ResourceKind::Document | ResourceKind::Iframe) {
            req.origin =
                Some(browser.page(request.page).map_or_else(|| "null".into(), |p| origin_of(&p.main_frame.url)));
        }
        if request.kind == ResourceKind::Stylesheet {
            if request.crossorigin.is_none() {
                req.credentials = lp_net::Credentials::Include;
            }
            req.max_body = Some(crate::stylesheets::MAX_SHEET_BYTES.min(self.client.config().max_body));
        }
        req.referrer = request.referrer.as_deref().and_then(|r| url::Url::parse(r).ok());
        req.frame_id = request.frame_id;
        req.loader_id = request.loader_id;
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
        self.entries.borrow_mut().insert(url.to_string(), (mime.to_string(), body.as_bytes().to_vec(), delay));
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
                Some((mime, body, _)) => Ok(Resource { url: url.clone(), status: 200, mime, charset: None, body }),
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
    page: PageId,
    sched: lp_runtime::Scheduler<Browser>,
}

impl ModuleFetcher for PageModuleFetcher {
    fn fetch(&self, realm: &Realm, url: String, request: ModuleRequest) {
        // Called from inside JS: defer the network request to a task, where
        // `&mut Browser` is available.
        let realm = realm.downgrade();
        let page = self.page;
        let request = RefCell::new(Some(request));
        self.sched.post(TaskSource::Networking, move |b| {
            let Some(request) = request.borrow_mut().take() else { return };
            b.fetch_text(page, url, ResourceKind::ModuleScript, move |_b, res| {
                let Some(realm) = realm.upgrade() else { return };
                let Ok(scope) = realm.scope() else { return };
                let r = match res {
                    Ok(text) => request.resolve(&scope, &text),
                    Err(e) => request.reject(&scope, &JsError::Type(e)),
                };
                if let Err(e) = r {
                    scope.report_exception(&e);
                }
            });
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

    /// Starts fetching a document for a non-blank navigation.
    pub(crate) fn start_document_fetch(
        &mut self,
        page: PageId,
        opts: NavigateOpts,
        loader_id: LoaderId,
    ) -> Result<(), JsError> {
        let Some(loader) = self.loader_for(page) else {
            return Err(JsError::Unsupported("navigation without a resource loader"));
        };
        let frame_id = self.page(page).map(|p| p.main_frame.id.0.clone());
        if let Some(p) = self.page_mut(page) {
            p.pending_loader = Some(loader_id.clone());
        }
        let req = FetchRequest {
            url: opts.url.clone(),
            kind: ResourceKind::Document,
            page,
            referrer: opts.referrer.clone(),
            frame_id,
            loader_id: Some(loader_id.0.clone()),
            crossorigin: None,
        };
        loader.fetch(
            self,
            req,
            Box::new(move |b, res| {
                let current = b.page(page).and_then(|p| p.pending_loader.clone());
                if current.as_ref() != Some(&loader_id) {
                    return; // superseded by another navigation
                }
                if let Some(p) = b.page_mut(page) {
                    p.pending_loader = None;
                }
                match res {
                    Ok(resource) if (200..300).contains(&resource.status) || resource.status == 0 => {
                        let mut opts = opts;
                        opts.url = resource.url.clone();
                        if let Err(e) = b.begin_document(page, &opts, loader_id.clone(), resource) {
                            b.navigation_failed(page, &opts, loader_id, e.to_string());
                        }
                    }
                    Ok(resource) => {
                        // Error pages still commit in browsers; keep their body.
                        let mut opts = opts;
                        opts.url = resource.url.clone();
                        if let Err(e) = b.begin_document(page, &opts, loader_id.clone(), resource) {
                            b.navigation_failed(page, &opts, loader_id, e.to_string());
                        }
                    }
                    Err(e) => b.navigation_failed(page, &opts, loader_id, e),
                }
            }),
        );
        Ok(())
    }

    pub(crate) fn navigation_failed(&mut self, page: PageId, opts: &NavigateOpts, loader_id: LoaderId, error: String) {
        let Some(frame_id) = self.page(page).map(|p| p.main_frame.id.clone()) else { return };
        self.emit(
            page,
            PageEvent::FrameNavigateFailed {
                frame_id,
                loader_id,
                url: opts.url.clone(),
                error,
                cdp_request_id: opts.cdp_request_id,
            },
        );
    }

    pub(crate) fn install_module_fetcher(&self, page: PageId, realm: &Realm) {
        realm.set_module_fetcher(Some(Rc::new(PageModuleFetcher { page, sched: self.scheduler() })));
    }

    fn begin_document(
        &mut self,
        page: PageId,
        opts: &NavigateOpts,
        loader_id: LoaderId,
        resource: Resource,
    ) -> Result<(), JsError> {
        let origin = origin_of(&resource.url);
        self.commit_document(page, opts, loader_id.clone(), &origin)?;
        let page_dom = self.page(page).and_then(|p| p.main_frame.page_dom.clone()).ok_or(JsError::ContextDestroyed)?;
        let dom = page_dom.dom.clone();
        let doc = page_dom.document();
        let mut parser = DocumentParser::new(dom.clone(), doc, true).map_err(|e| JsError::Type(format!("{e:?}")))?;
        parser.set_transport_charset(resource.charset.as_deref());
        let p = self.page_mut(page).ok_or(JsError::Type("no such page".into()))?;
        p.main_frame.dom = Some(dom.clone());
        p.main_frame.document = Some(doc);
        p.nav.set_document(Some((dom.clone(), doc)));
        p.load = Some(DocumentLoad {
            loader_id,
            url: resource.url.clone(),
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
        self.scheduler().post(TaskSource::Networking, move |b| b.continue_parsing(page));
        Ok(())
    }

    fn load_mut(&mut self, page: PageId) -> Option<&mut DocumentLoad> {
        self.page_mut(page)?.load.as_mut()
    }

    fn current_loader(&self, page: PageId) -> Option<LoaderId> {
        self.page(page)?.load.as_ref().map(|l| l.loader_id.clone())
    }

    /// Runs the parser until it blocks on a script or finishes.
    fn continue_parsing(&mut self, page: PageId) {
        loop {
            let Some(load) = self.load_mut(page) else { return };
            if load.blocked || load.parsing_done {
                return;
            }
            let Some(parser) = load.parser.as_mut() else { return };
            let step = match std::mem::replace(&mut load.phase, ParsePhase::Resume) {
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
            };
            self.drain_dom_resources(page);
            match step {
                ParseStep::Script(node) => {
                    let blocks = self
                        .page(page)
                        .and_then(|p| p.main_frame.page_dom.as_ref())
                        .and_then(|p| p.dom.borrow().script_info(node).ok())
                        .is_some_and(|s| s.kind == ScriptKind::Classic && (s.src.is_none() || (!s.async_ && !s.defer)));
                    if blocks && self.stylesheets_block_scripts(page) {
                        let load = self.load_mut(page).expect("load");
                        if load.dom.borrow_mut().add_ref(node).is_ok() {
                            load.stylesheet_blocked_script =
                                Some(Rc::new(ResourceNode { dom: load.dom.clone(), node }));
                            load.blocked = true;
                            return;
                        }
                    }
                    self.prepare_script(page, node);
                }
                ParseStep::NeedInput => {
                    let load = self.load_mut(page).expect("load");
                    match load.phase {
                        ParsePhase::Resume => load.phase = ParsePhase::Finish,
                        _ => {
                            load.phase = ParsePhase::Done;
                            if let Some(mut p) = load.parser.take() {
                                p.end();
                            }
                            load.parsing_done = true;
                            if let Some(dom) = self.page(page).and_then(|p| p.main_frame.page_dom.clone()) {
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

    /// "Prepare the script element" for a parser-inserted script.
    fn prepare_script(&mut self, page: PageId, node: NodeId) {
        let Some(load) = self.page(page).and_then(|p| p.load.as_ref()) else { return };
        let dom = load.dom.clone();
        let base = load.url.clone();
        let info = {
            let mut d = dom.borrow_mut();
            let Ok(info) = d.script_info(node) else { return };
            if info.already_started {
                return;
            }
            let _ = d.mark_script_started(node);
            info
        };
        if dom.borrow_mut().add_ref(node).is_err() {
            return;
        }
        let element = Rc::new(ResourceNode { dom, node });
        let ty = match info.kind {
            ScriptKind::Classic if !info.nomodule => ScriptType::Classic,
            ScriptKind::Module => ScriptType::Module,
            // nomodule classic, import maps (unsupported) and data blocks.
            _ => return,
        };
        let src_url = match info.src.as_deref() {
            Some(s) => match resolve_url(&base, s) {
                Some(u) => Some(u),
                None => return, // spec: error event (needs lp-webapi)
            },
            None => None,
        };
        match (ty, src_url) {
            (ScriptType::Classic, None) => self.execute_classic(page, &info.inline_text, &base, &element),
            (ScriptType::Classic, Some(url)) if info.async_ => self.fetch_async(page, ty, url, element),
            (ScriptType::Classic, Some(url)) if info.defer => self.fetch_deferred(page, ty, url, element),
            (ScriptType::Classic, Some(url)) => {
                // Parser-blocking: pause until fetched and executed.
                let loader_id = self.current_loader(page);
                if let Some(l) = self.load_mut(page) {
                    l.blocked = true;
                }
                let u = url.clone();
                self.fetch_text(page, url, ResourceKind::Script, move |b, res| {
                    if b.current_loader(page) != loader_id {
                        return;
                    }
                    match res {
                        Ok(text) => b.execute_classic(page, &text, &u, &element),
                        Err(e) => b.report_load_error(page, &e),
                    }
                    if let Some(l) = b.load_mut(page) {
                        l.blocked = false;
                    }
                    b.continue_parsing(page);
                });
            }
            (ScriptType::Module, Some(url)) if info.async_ => self.fetch_async(page, ty, url, element),
            (ScriptType::Module, Some(url)) => self.fetch_deferred(page, ty, url, element),
            (ScriptType::Module, None) => {
                let url = {
                    let l = self.load_mut(page).expect("load");
                    l.inline_module_seq += 1;
                    // Distinct registry key per inline module (see lp_js::Scope::eval_module).
                    format!("{}#inline-module-{}", base.split('#').next().unwrap_or(&base), l.inline_module_seq)
                };
                if info.async_ {
                    self.execute_module(page, &info.inline_text, &url, false);
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
        page: PageId,
        url: String,
        kind: ResourceKind,
        done: impl FnOnce(&mut Browser, Result<String, String>) + 'static,
    ) {
        let Some(loader) = self.loader_for(page) else {
            self.scheduler().post(TaskSource::Networking, move |b| done(b, Err("no resource loader".into())));
            return;
        };
        let referrer = self.page(page).map(|p| p.main_frame.url.clone());
        let frame_id = self.page(page).map(|p| p.main_frame.id.0.clone());
        // Subresources carry the document's loader id (CDP Network.* routing).
        let loader_id = self.page(page).map(|p| p.main_frame.loader_id.0.clone());
        let req = FetchRequest { url, kind, page, referrer, frame_id, loader_id, crossorigin: None };
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

    fn fetch_deferred(&mut self, page: PageId, ty: ScriptType, url: String, element: Rc<ResourceNode>) {
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

    fn fetch_async(&mut self, page: PageId, ty: ScriptType, url: String, element: Rc<ResourceNode>) {
        let loader_id = {
            let l = self.load_mut(page).expect("load");
            l.async_pending += 1;
            l.loader_id.clone()
        };
        let kind = if ty == ScriptType::Module { ResourceKind::ModuleScript } else { ResourceKind::Script };
        let u = url.clone();
        self.fetch_text(page, url, kind, move |b, res| {
            if b.current_loader(page) != Some(loader_id) {
                return;
            }
            match res {
                Ok(text) if ty == ScriptType::Classic => b.execute_classic(page, &text, &u, &element),
                Ok(text) => {
                    b.execute_module(page, &text, &u, false);
                }
                Err(e) => b.report_load_error(page, &e),
            }
            if let Some(l) = b.load_mut(page) {
                l.async_pending -= 1;
            }
            b.maybe_fire_load(page);
        });
    }

    /// Runs ready deferred scripts in order once parsing is done, then fires
    /// `DOMContentLoaded`.
    fn run_deferred(&mut self, page: PageId) {
        loop {
            self.drain_dom_resources(page);
            let css_blocked = self.stylesheets_block_scripts(page);
            let Some(load) = self.load_mut(page) else { return };
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
            let (ty, url, element) = {
                let n = next.borrow();
                (n.ty, n.url.clone(), n.element.clone())
            };
            match source {
                Ok(text) if ty == ScriptType::Classic => self.execute_classic(page, &text, &url, &element),
                Ok(text) => {
                    if self.execute_module(page, &text, &url, true) {
                        return; // resumes when the module's promise settles
                    }
                }
                Err(e) => self.report_load_error(page, &e),
            }
        }
        self.fire_dom_content_loaded(page);
    }

    fn fire_dom_content_loaded(&mut self, page: PageId) {
        let Some(load) = self.load_mut(page) else { return };
        if load.dcl_fired {
            return;
        }
        load.dcl_fired = true;
        let loader_id = load.loader_id.clone();
        let frame_id = self.page(page).expect("page").main_frame.id.clone();
        if let Some(dom) = self.page(page).and_then(|p| p.main_frame.page_dom.clone()) {
            lp_webapi::lifecycle::dom_content_loaded(&dom, dom.document());
        }
        self.emit(page, PageEvent::FrameDomContentLoaded { frame_id, loader_id, timestamp: self.timestamp() });
        self.maybe_fire_load(page);
    }

    fn maybe_fire_load(&mut self, page: PageId) {
        self.drain_dom_resources(page);
        if self.page(page).is_some_and(|p| p.stylesheets.pending() != 0) {
            return;
        }
        let Some(load) = self.load_mut(page) else { return };
        if !load.dcl_fired || load.load_fired || load.async_pending > 0 {
            return;
        }
        load.load_fired = true;
        let loader_id = load.loader_id.clone();
        let frame_id = self.page(page).expect("page").main_frame.id.clone();
        if let Some(dom) = self.page(page).and_then(|p| p.main_frame.page_dom.clone()) {
            lp_webapi::lifecycle::load(&dom, dom.document());
        }
        let ts = self.timestamp();
        self.emit(
            page,
            PageEvent::FrameLoaded { frame_id: frame_id.clone(), loader_id: loader_id.clone(), timestamp: ts },
        );
        self.emit(page, PageEvent::FrameNetworkAlmostIdle { frame_id: frame_id.clone(), loader_id: loader_id.clone() });
        self.emit(page, PageEvent::FrameNetworkIdle { frame_id, loader_id });
    }

    /// A stylesheet completed or stopped blocking. Called from a later task,
    /// never recursively from the parser's resource-event drain.
    pub(crate) fn stylesheets_changed(&mut self, page: PageId) {
        self.drain_dom_resources(page);
        if !self.stylesheets_block_scripts(page) {
            let pending = self.load_mut(page).and_then(|l| l.stylesheet_blocked_script.take());
            if let Some(element) = pending {
                self.load_mut(page).expect("load").blocked = false;
                self.prepare_script(page, element.node);
                self.continue_parsing(page);
            }
            self.run_deferred(page);
        }
        self.maybe_fire_load(page);
    }

    fn main_realm(&self, page: PageId) -> Option<Realm> {
        self.page(page)?.main_frame.main_realm().cloned()
    }

    fn execute_classic(&mut self, page: PageId, text: &str, url: &str, element: &ResourceNode) {
        let Some(realm) = self.main_realm(page) else { return };
        let Ok(scope) = realm.scope() else { return };
        let bindings = self.page(page).and_then(|p| p.main_frame.page_dom.clone());
        if let Some(dom) = &bindings {
            dom.set_current_script(Some(element.node));
        }
        if let Err(e) = scope.eval(text, Some(url), 1) {
            scope.report_exception(&e);
        }
        if let Some(dom) = &bindings {
            dom.set_current_script(None);
        }
    }

    /// Starts a module; with `deferred`, parsing-completion work waits for its
    /// evaluation promise. Returns whether such a wait was set up.
    fn execute_module(&mut self, page: PageId, text: &str, url: &str, deferred: bool) -> bool {
        let Some(realm) = self.main_realm(page) else { return false };
        let Ok(scope) = realm.scope() else { return false };
        let promise = match scope.eval_module(text, url) {
            Ok(p) => p,
            Err(e) => {
                scope.report_exception(&e);
                return false;
            }
        };
        let loader_id = self.current_loader(page);
        let sched = self.scheduler();
        let resume = move || {
            let loader_id = loader_id.clone();
            sched.post(TaskSource::Networking, move |b| {
                if b.current_loader(page) != loader_id {
                    return;
                }
                if let Some(l) = b.load_mut(page) {
                    if deferred {
                        l.awaiting_module = false;
                    } else {
                        l.async_pending -= 1;
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
            move |_s, _v| ok(),
            move |s, e| {
                s.report_exception(&JsError::Thrown(Persistent::new(s, e)));
                err()
            },
        );
        if let Err(e) = r {
            scope.report_exception(&e);
            return false;
        }
        if let Some(l) = self.load_mut(page) {
            if deferred {
                l.awaiting_module = true;
            } else {
                l.async_pending += 1;
            }
        }
        true
    }

    fn report_load_error(&mut self, page: PageId, error: &str) {
        let Some(realm) = self.main_realm(page) else { return };
        let Ok(scope) = realm.scope() else { return };
        scope.report_exception(&JsError::Type(format!("Failed to load script: {error}")));
    }
}
