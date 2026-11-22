//! Document loading: fetch → parse → scripts → DOMContentLoaded → load.
//!
//! Script handling follows HTML "prepare the script element" for
//! parser-inserted scripts (classification by `pyro_dom::Dom::script_info`):
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
//! Deviations: JSC fetches the module graph during evaluation, not before
//! it, so a module counts as run (for the next deferred script,
//! `DOMContentLoaded` and `load`) when its evaluation promise settles or,
//! pending on a top-level await, once the realm has no module fetch in flight
//! after a task (`Browser::execute_module`). A script-inserted script is prepared when the inserting script returns (a
//! parser-run script) or at the end of the inserting task (a handler or a
//! timer), instead of inside the DOM call that inserted it. A module
//! element's `load` fires when it counts as run and its evaluation promise
//! has not rejected by then, `error` when it rejected (also for an
//! evaluation error).
//!
//! DOM lifecycle and currentScript are wired through pyro-webapi. Iframe/image
//! resources are separate loader work; linked stylesheets are handled by
//! `stylesheets`.

use crate::frames::FrameKey;
use crate::{Browser, HistoryHandling, LoaderId, NavigateOpts, PageEvent, PageId};
use pyro_dom::parse::{DocumentParser, DomRef, ParseStep};
use pyro_dom::script::ScriptKind;
use pyro_dom::NodeId;
use pyro_js::{JsError, ModuleFetcher, ModuleRequest, ModuleType, Persistent, Realm};
use pyro_runtime::TaskSource;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::Duration;

/// `PYRO_TRACE_MODULES=1`: one stderr line per step of every ES
/// module fetch (requested, task started, loaded, handed to the engine), to
/// find an `import()` that never settles in a real application.
pub(crate) fn trace_modules() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("PYRO_TRACE_MODULES").is_ok_and(|v| v == "1"))
}

macro_rules! module_trace {
    ($($arg:tt)*) => {
        if trace_modules() {
            eprintln!("pyro: module {:.3} {}", trace_epoch().elapsed().as_secs_f64() * 1e3, format_args!($($arg)*));
        }
    };
}

fn trace_epoch() -> std::time::Instant {
    static T0: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    *T0.get_or_init(std::time::Instant::now)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResourceKind {
    Document,
    Script,
    ModuleScript,
    Stylesheet,
    Iframe,
    Image,
    /// A web font (a renderer's `@font-face` source).
    Font,
    /// An `<audio>` element's media resource.
    Audio,
    /// A `<video>` element's media resource.
    Video,
    /// An HTML text track resource.
    Track,
    /// A hyperlink download (`<a download>`).
    Download,
    /// An `<object>` element's data (destination "object").
    Object,
}

#[derive(Debug, Clone)]
pub struct FetchRequest {
    pub parser_priority: bool,
    /// Body and content type of a form POST (`None`: GET).
    pub post: Option<(Vec<u8>, String)>,
    pub url: String,
    pub kind: ResourceKind,
    pub page: PageId,
    pub referrer: Option<String>,
    /// The request's own referrer policy (a `referrerpolicy` attribute,
    /// `rel=noreferrer`, a navigation's initiator); `None`: the policy of
    /// the document of `frame_id`.
    pub referrer_policy: Option<pyro_net::ReferrerPolicy>,
    pub frame_id: Option<String>,
    pub loader_id: Option<String>,
    /// HTML crossorigin attribute (absent uses the resource default).
    pub crossorigin: Option<String>,
    /// The request's credentials mode when set by its initiator rather than an attribute (a
    /// worklet's module graph); wins over the one `crossorigin` implies.
    pub credentials: Option<pyro_net::Credentials>,
    /// Resource Timing: the `initiatorType` of the requesting document's
    /// entry for this fetch (`None`: no entry, as for a document's own
    /// navigation or a renderer's fetch). An iframe's navigation reports to
    /// the parent document.
    pub initiator: Option<pyro_webapi::resource_timing::Initiator>,
    /// The module type of a [`ResourceKind::ModuleScript`] fetch, which
    /// picks its destination: a CSS module is fetched as a style, a JSON
    /// one as `json` (`None`: JavaScript).
    pub module: Option<ModuleType>,
    /// Only part of the body is needed (a media element's headers):
    /// `Resource::partial` says when the body was cut.
    pub part: Option<pyro_webapi::BodyPart>,
}

impl FetchRequest {
    pub(crate) fn effective_credentials(&self) -> pyro_net::Credentials {
        if self.kind == ResourceKind::Stylesheet && self.crossorigin.is_none() { return pyro_net::Credentials::Include; }
        self.credentials.unwrap_or_else(|| if self.crossorigin.as_deref().is_some_and(|value| value.eq_ignore_ascii_case("use-credentials")) { pyro_net::Credentials::Include } else { pyro_net::Credentials::SameOrigin })
    }
}

/// Where a `BodyPart` fetch cuts `body` (a prefix of the response body of
/// `total` bytes, if known): the length to keep, if any. `checked` is the
/// size `enough` is next asked at; it doubles past `body`'s length.
fn body_cut(part: &pyro_webapi::BodyPart, body: &[u8], total: Option<u64>, checked: &mut usize) -> Option<usize> {
    if body.len() >= part.limit {
        return Some(part.limit);
    }
    if body.len() < *checked {
        return None;
    }
    while *checked <= body.len() {
        *checked = checked.saturating_mul(2);
    }
    (part.enough)(body, total).then_some(body.len())
}

#[derive(Debug, Clone)]
pub struct Resource {
    /// Final URL after redirects.
    pub url: String,
    pub status: u16,
    /// Lowercased MIME essence (e.g. `text/html`).
    pub mime: String,
    pub charset: Option<String>,
    pub body: std::sync::Arc<Vec<u8>>,
    pub immutable_local: bool,
    /// The `Content-Disposition` response header, if any.
    pub content_disposition: Option<String>,
    /// The response carried a `Content-Security-Policy` header (the policy
    /// itself is not enforced; a document delivered with one hides nonces).
    /// pyro: the values of its `Content-Security-Policy` headers joined by `,` (each a policy),
    /// a newline, and those of its `Content-Security-Policy-Report-Only` headers. Only the
    /// Trusted Types directives are acted on (`Dom::set_trusted_types_policies`); other
    /// directives are not enforced.
    pub header_csp: Option<String>,
    /// The `Refresh` response header (HTML declarative refresh), if any.
    pub refresh: Option<String>,
    /// A document's `Content-Language` response header, if any (its
    /// language when no element or pragma gives one).
    pub content_language: Option<String>,
    /// The response's `Referrer-Policy` header (a document's default policy).
    pub referrer_policy: Option<pyro_net::ReferrerPolicy>,
    /// A navigation's `Referer` as its last hop sent it (`document.referrer`
    /// of the document it commits; empty: none was sent). `None`: the
    /// loader does not say, and it is computed from the navigation.
    pub sent_referrer: Option<String>,
    /// A navigation's fetch as the network saw it (Navigation Timing of the
    /// document it commits); `None` for other kinds and other loaders.
    pub timing: Option<Box<pyro_webapi::resource_timing::FetchReport>>,
    /// `Some`: `body` is a prefix of the response body, cut as the
    /// request's `part` asked (the transfer stopped there).
    pub partial: Option<pyro_webapi::Partial>,
    /// The response said `X-Content-Type-Options: nosniff`: a style sheet must be `text/css`.
    pub nosniff: bool,
    /// pyro: the response's tainting is "opaque" (Fetch): a no-cors request that reached another
    /// origin at any hop of its redirect chain, even one that ended back at its own origin.
    pub opaque: bool,
}

impl Resource {
    /// Body decoded with the response charset (BOM wins), UTF-8 by default.
    pub fn text(&self) -> String {
        pyro_net::decode_text(&self.body, self.charset.as_deref()).into_owned()
    }

    /// The body as a renderer takes it: a style sheet (`text/css`) decoded as
    /// CSS Syntax decodes one (byte order mark, the response's charset,
    /// `@charset`, then `environment`, the importing sheet's or document's
    /// encoding, which the renderer cannot know) and handed over as UTF-8
    /// behind a byte order mark, which its parser takes over all of those;
    /// any other body as it came.
    pub fn render_body(self, environment: &[impl AsRef<str>]) -> Vec<u8> {
        if !self.mime.eq_ignore_ascii_case("text/css") {
            return std::sync::Arc::unwrap_or_clone(self.body);
        }
        let environment: Vec<&str> = environment.iter().map(AsRef::as_ref).collect();
        let (text, _) = pyro_dom::cssom::decode_stylesheet(&self.body, self.charset.as_deref(), &environment);
        pyro_dom::cssom::stylesheet_utf8_bytes(&text)
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
    fn report_resource_delivery(
        &self,
        page: PageId,
        loader: Option<&LoaderId>,
        request: pyro_net::RequestId,
        frontier: pyro_net::NetworkDelivery,
    ) {
        let (Some(page), Some(loader)) = (self.page(page), loader) else { return };
        let observer = self.observer.borrow().clone();
        if let Some(observer) = observer {
            observer.on_network_delivery(page.id, page.context, loader, request, frontier);
        }
    }

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

/// Network-backed loader over `pyro_net::NetClient` (hyper on the I/O thread).
/// Bodies are accumulated on the I/O thread and delivered as one task.
pub struct NetLoader {
    client: pyro_net::NetClient,
}

#[derive(Default)]
pub(crate) struct ReadyResponses(std::collections::HashMap<String, std::collections::BTreeMap<(bool, u64), Result<Resource, String>>>);

impl ReadyResponses {
    fn push(&mut self, document: &str, blocking: bool, id: u64, result: Result<Resource, String>) {
        self.0.entry(document.to_owned()).or_default().insert((!blocking, id), result);
    }

    fn take(&mut self, document: &str) -> Option<(u64, Result<Resource, String>)> {
        let queue = self.0.get_mut(document)?;
        let ((_, id), result) = queue.pop_first()?;
        if queue.is_empty() { self.0.remove(document); }
        Some((id, result))
    }
}

#[cfg(test)]
mod ready_tests {
    #[test]
    fn document_responses_deliver_by_priority_then_issue_order() {
        let mut ready = super::ReadyResponses::default();
        for (document, blocking, id) in [("first", false, 5), ("other", true, 9), ("first", true, 3), ("first", true, 2)] {
            ready.push(document, blocking, id, Err(String::new()));
        }
        let ids: Vec<_> = std::iter::from_fn(|| ready.take("first")).map(|entry| entry.0).collect();
        assert_eq!(ids, [2, 3, 5]);
        assert_eq!(ready.take("other").unwrap().0, 9);
        assert!(ready.0.is_empty());
    }
}

impl NetLoader {
    pub fn new(client: pyro_net::NetClient) -> NetLoader {
        NetLoader { client }
    }

    pub fn client(&self) -> &pyro_net::NetClient {
        &self.client
    }
}

impl Browser {
    fn fetch_ready(&mut self, priority: Option<(String, bool)>, id: u64, result: Result<Resource, String>) {
        let Some((document, blocking)) = priority else { return self.complete_fetch(id, result) };
        self.ready_responses.push(&document, blocking, id, result);
        self.scheduler().post(TaskSource::Networking, move |browser| {
            if let Some((id, result)) = browser.ready_responses.take(&document) {
                browser.complete_fetch(id, result);
            }
        });
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
            ResourceKind::Document => pyro_net::Destination::Document,
            ResourceKind::Script => pyro_net::Destination::Script,
            ResourceKind::ModuleScript => match request.module {
                Some(ModuleType::Css) => pyro_net::Destination::Style,
                Some(ModuleType::Json) => pyro_net::Destination::Json,
                Some(ModuleType::Text) => pyro_net::Destination::Text,
                _ => pyro_net::Destination::Script,
            },
            ResourceKind::Stylesheet => pyro_net::Destination::Style,
            ResourceKind::Iframe => pyro_net::Destination::Iframe,
            ResourceKind::Image => pyro_net::Destination::Image,
            ResourceKind::Font => pyro_net::Destination::Font,
            ResourceKind::Audio => pyro_net::Destination::Audio,
            ResourceKind::Video => pyro_net::Destination::Video,
            ResourceKind::Track => pyro_net::Destination::Track,
            ResourceKind::Download | ResourceKind::Object => pyro_net::Destination::Other,
        };
        // Resource Timing: the document that gets this fetch's entry.
        let document = requesting_document(browser, &request);
        let timed = request.initiator.zip(document.clone());
        let render_blocking = request.kind == ResourceKind::Stylesheet;
        let priority = request.loader_id.as_ref().filter(|_| request.parser_priority || request.kind == ResourceKind::Image)
            .map(|document| (document.clone(), request.parser_priority));
        let mut req = pyro_net::Request::get(url, destination);
        req.parser_priority = request.parser_priority;
        if let Some(page) = browser.page(request.page) {
            req.defaults = page.request_defaults.clone();
            if request.kind == ResourceKind::Document {
                req.top_level_site = pyro_net::cookies::partition_key(&req.url);
            } else {
                // Nested navigations use their embedding document. Other
                // resources use the initiating document's complete frame chain.
                req.top_level_site = document.as_ref().and_then(|d| d.cookie_partition_key());
                req.cross_site_ancestor = document.is_none_or(|d| d.cookie_site_context() == pyro_net::cookies::SiteContext::CrossSite);
            }
        }
        if request.kind == ResourceKind::ModuleScript || request.crossorigin.is_some() {
            req.mode = pyro_net::RequestMode::Cors;
        } else if request.kind == ResourceKind::Track {
            req.mode = pyro_net::RequestMode::SameOrigin;
        }
        req.credentials = request.effective_credentials();
        if req.url.scheme() == "blob" {
            req.blob = blob_body(browser, request.page, req.url.as_str());
        }
        if !matches!(request.kind, ResourceKind::Document | ResourceKind::Iframe) {
            req.origin = Some(
                request
                    .frame_id
                    .as_deref()
                    .and_then(|id| frame_origin(browser, request.page, id))
                    .unwrap_or_else(|| "null".into()),
            );
        }
        if request.kind == ResourceKind::Stylesheet {
            req.max_body = Some(crate::stylesheets::MAX_SHEET_BYTES.min(self.client.config().max_body));
        }
        let referrer = request_referrer(browser, &request);
        req.referrer = referrer.referrer;
        req.referrer_policy = referrer.policy;
        let part = request.part;
        if let Some(start) = part.map(|p| p.start).filter(|&s| s > 0) {
            if let Ok(value) = pyro_net::http::HeaderValue::from_str(&format!("bytes={start}-")) {
                req.headers.insert(pyro_net::http::header::RANGE, value);
            }
        }
        if let Some((body, content_type)) = request.post {
            req.method = pyro_net::http::Method::POST;
            req.body = Some(body.into());
            if let Ok(value) = pyro_net::http::HeaderValue::from_str(&content_type) {
                req.headers.insert(pyro_net::http::header::CONTENT_TYPE, value);
            }
            req.origin = request.frame_id.as_deref().and_then(|id| frame_origin(browser, request.page, id));
        }
        // Resource Timing: an iframe navigation's entry goes to the parent,
        // so its TAO check runs against the parent's origin, not `origin`
        // (a POST's: the frame's own document).
        if request.kind == ResourceKind::Iframe && timed.is_some() {
            req.timing_origin = Some(
                parent_origin(browser, request.page, request.frame_id.as_deref()).unwrap_or_else(|| "null".into()),
            );
        }
        req.frame_id = request.frame_id;
        req.loader_id = request.loader_id;
        let document = matches!(request.kind, ResourceKind::Document | ResourceKind::Iframe);
        // A service worker's `fetch` event first, in a context with
        // registrations; one emptiness test otherwise.
        let service_worker = service_worker_route(browser, request.page, &req, document);
        // A preloaded static file: answered here, on the browser thread, with
        // no I/O-thread round trip. Observers still get the Network events.
        if let Some(hit) = self.client.static_direct(&req).filter(|_| service_worker.is_none()) {
            if request.kind == ResourceKind::ModuleScript {
                module_trace!("served from the static directory {}", req.url);
            }
            let start = std::time::Instant::now();
            let (network_id, head) = self.client.report_local_with_id(&req, &hit);
            let delivery_loader = req.loader_id.clone().map(LoaderId);
            let delivery_page = request.page;
            let delivery_bytes = hit.file.body.len() as u64;
            // Cached loads emit on this thread. Let observers drain the request
            // announcement now, but keep responses behind their delivery frontier.
            browser.report_resource_delivery(
                delivery_page,
                delivery_loader.as_ref(),
                network_id,
                pyro_net::NetworkDelivery { response: false, data_bytes: 0, terminal: false },
            );
            let report = (timed.is_some() || document).then(|| {
                let mut response = pyro_webapi::resource_timing::ReportedResponse::of(&head);
                response.encoded = hit.file.body.len() as u64;
                response.decoded = response.encoded;
                pyro_webapi::resource_timing::FetchReport {
                    url: req.url.as_str().to_owned(),
                    initiator: timed.as_ref().map_or(pyro_webapi::resource_timing::Initiator::Other, |t| t.0),
                    start,
                    end: std::time::Instant::now(),
                    response: Some(response),
                    render_blocking,
                }
            });
            if let (Some((_, page)), Some(report)) = (&timed, &report) {
                page.queue_resource_timing(report.clone());
            }
            let resource = Resource {
                url: head.url.to_string(),
                status: 200,
                mime: head.mime,
                charset: head.charset,
                content_disposition: None,
                header_csp: None,
                refresh: None,
                content_language: None,
                referrer_policy: None,
                sent_referrer: document.then(|| {
                    pyro_net::referrer::ReferrerChain::new(req.referrer.clone(), req.referrer_policy)
                        .header(&req.url)
                        .unwrap_or_default()
                }),
                body: hit.file.body.to_vec().into(),
                immutable_local: false,
                timing: report.filter(|_| document).map(Box::new),
                partial: None,
                nosniff: false,
                opaque: false,
            };
            browser.scheduler().post(TaskSource::Networking, move |b| {
                b.report_resource_delivery(
                    delivery_page,
                    delivery_loader.as_ref(),
                    network_id,
                    pyro_net::NetworkDelivery { response: true, data_bytes: delivery_bytes, terminal: true },
                );
                b.fetch_ready(priority, id, Ok(resource));
            });
            return None;
        }
        if request.kind == ResourceKind::ModuleScript {
            module_trace!("sent to the network {}", req.url);
        }
        let mut head: Option<pyro_net::ResponseHead> = None;
        let mut body: Vec<u8> = Vec::new();
        // A navigation's referrer, followed across its redirects as the
        // client sends it: the last hop's is `document.referrer`.
        let mut chain =
            document.then(|| pyro_net::referrer::ReferrerChain::new(req.referrer.clone(), req.referrer_policy));
        // Navigation Timing of the document this fetch commits.
        let start = std::time::Instant::now();
        let navigation_url = document.then(|| req.url.as_str().to_owned());
        let url = req.url.clone();
        let native_id = self.client.next_request_id();
        // A `part` fetch: the length of the whole body, when known; when
        // `enough` is next asked; whether the body was already delivered.
        let mut total: Option<u64> = None;
        let mut checked = pyro_webapi::BodyPart::FIRST_CHECK;
        let mut cut = false;
        // Not the client itself: pyro-net owns the sink until the request ends,
        // and the client stops when its last handle goes.
        let net = self.client.canceller();
        let sink: pyro_net::ResponseSink = Box::new(move |ev| match ev {
            // A cut body was delivered: the rest (and the cancellation's
            // failure) is dropped.
            _ if cut => {}
            pyro_net::ResponseEvent::Redirect { from, headers, .. } => {
                if let Some(chain) = chain.as_mut() {
                    chain.redirect(&from, &headers);
                }
            }
            pyro_net::ResponseEvent::Response(h) => {
                if part.is_some() && !h.headers.contains_key(http::header::CONTENT_ENCODING) {
                    total =
                        h.headers.get(http::header::CONTENT_LENGTH).and_then(|v| v.to_str().ok()?.trim().parse().ok());
                }
                head = Some(h);
            }
            pyro_net::ResponseEvent::Data(chunk) => {
                body.extend_from_slice(&chunk);
                let Some(part) = part.as_ref() else { return };
                let Some(keep) = body_cut(part, &body, total, &mut checked) else { return };
                let Some(h) = head.take() else { return };
                body.truncate(keep);
                cut = true;
                let whole = total == Some(body.len() as u64);
                let resource = Resource {
                    url: h.url.to_string(),
                    status: h.status.as_u16(),
                    mime: h.mime,
                    charset: h.charset,
                    body: std::mem::take(&mut body).into(),
                    immutable_local: false,
                    content_disposition: None,
                    header_csp: None,
                    refresh: None,
                    content_language: None,
                    referrer_policy: None,
                    sent_referrer: None,
                    timing: None,
                    partial: (!whole).then_some(pyro_webapi::Partial { total }),
                    nosniff: false,
                    opaque: h.kind == pyro_net::ResponseKind::Opaque,
                };
                let priority = priority.clone();
                let _ = handle.post(move |b: &mut Browser| b.fetch_ready(priority, id, Ok(resource)));
                // Stop the transfer: a live stream never ends.
                net.cancel(native_id);
            }
            pyro_net::ResponseEvent::Finished { encoded_len, decoded_len } => {
                let timing = |h: &pyro_net::ResponseHead| {
                    let url = navigation_url.clone()?;
                    let mut response = pyro_webapi::resource_timing::ReportedResponse::for_navigation(h);
                    response.encoded = encoded_len;
                    response.decoded = decoded_len;
                    // The document's own timing is not cross-origin to
                    // it (an iframe's TAO check was for its parent).
                    response.timing.timing_allow_failed = false;
                    Some(Box::new(pyro_webapi::resource_timing::FetchReport {
                        url,
                        initiator: pyro_webapi::resource_timing::Initiator::Other,
                        start,
                        end: std::time::Instant::now(),
                        response: Some(response),
                        render_blocking: false,
                    }))
                };
                let result = match head.take() {
                    Some(h) => Ok(Resource {
                        timing: timing(&h),
                        partial: None,
                        opaque: h.kind == pyro_net::ResponseKind::Opaque,
                        nosniff: pyro_net::is_nosniff(&h.headers),
                        url: h.url.to_string(),
                        status: h.status.as_u16(),
                        mime: h.mime,
                        charset: h.charset,
                        content_disposition: h
                            .headers
                            .get(http::header::CONTENT_DISPOSITION)
                            .and_then(|v| v.to_str().ok())
                            .map(str::to_owned),
                        header_csp: {
                            let joined = |name: http::header::HeaderName| -> String {
                                let values: Vec<&str> =
                                    h.headers.get_all(name).iter().filter_map(|v| v.to_str().ok()).collect();
                                values.join(",")
                            };
                            let enforce = joined(http::header::CONTENT_SECURITY_POLICY);
                            let report_only = joined(http::header::CONTENT_SECURITY_POLICY_REPORT_ONLY);
                            (!enforce.is_empty() || !report_only.is_empty()).then(|| format!("{enforce}\n{report_only}"))
                        },
                        // Only a document honours it; other kinds skip the lookup.
                        refresh: document
                            .then(|| h.headers.get(http::header::REFRESH)?.to_str().ok().map(str::to_owned))
                            .flatten(),
                        content_language: document
                            .then(|| h.headers.get(http::header::CONTENT_LANGUAGE)?.to_str().ok().map(str::to_owned))
                            .flatten(),
                        referrer_policy: pyro_net::ReferrerPolicy::from_header(
                            h.headers.get_all("referrer-policy").iter().filter_map(|v| v.to_str().ok()),
                        ),
                        sent_referrer: chain.as_ref().map(|c| c.header(&h.url).unwrap_or_default()),
                        body: std::mem::take(&mut body).into(),
                        immutable_local: false,
                    }),
                    None => Err("net::ERR_EMPTY_RESPONSE".to_string()),
                };
                // Err: the browser thread has exited; nothing to deliver.
                let priority = priority.clone();
                let _ = handle.post(move |b: &mut Browser| b.fetch_ready(priority, id, result));
            }
            pyro_net::ResponseEvent::Failed(e) => {
                let msg = e.to_string();
                let priority = priority.clone();
                let _ = handle.post(move |b: &mut Browser| b.fetch_ready(priority, id, Err(msg)));
            }
        });
        let sink = match &timed {
            Some((initiator, page)) => {
                pyro_webapi::resource_timing::timed_sink(page, *initiator, &url, render_blocking, sink)
            }
            None => sink,
        };
        match service_worker {
            Some(route) => route.dispatch(&self.client, native_id, req, sink),
            None => self.client.fetch_with_id(native_id, req, sink),
        }
        let client = self.client.clone();
        Some(ResourceCancellation::new(move || client.cancel(native_id)))
    }
}

/// The service worker route of a loader fetch: a navigation goes to the
/// worker whose registration its URL matches (unless the page bypasses
/// service workers), any other request to the requesting document's
/// controller. `None` at the cost of one emptiness test when the context has
/// no registrations.
fn service_worker_route(
    browser: &Browser,
    page: PageId,
    req: &pyro_net::Request,
    document: bool,
) -> Option<pyro_webapi::LoadRoute> {
    let page = browser.page(page)?;
    let settings = page.frames.settings.borrow();
    if !settings.services.may_route_loads() {
        return None;
    }
    if document {
        if page.bypass_service_worker {
            return None;
        }
        return pyro_webapi::navigation_route(&settings.services, req);
    }
    let key = page.frames.key_of(req.frame_id.as_deref()?)?;
    pyro_webapi::subresource_route(&*page.frames.page_dom(key)?, req)
}

/// The document whose timeline gets the Resource Timing entry of `request`:
/// the requesting frame's current document, or for an iframe's navigation
/// the document of the frame's parent.
fn requesting_document(browser: &Browser, request: &FetchRequest) -> Option<Rc<pyro_webapi::PageDom>> {
    let frames = &browser.page(request.page)?.frames;
    let key = frames.key_of(request.frame_id.as_deref()?)?;
    let key = match request.kind {
        ResourceKind::Iframe => frames.key_of(&browser.doc(key)?.main_frame.parent.clone()?.0)?,
        _ => key,
    };
    frames.page_dom(key)
}

/// The referrer source and policy `request` is sent with: its own policy,
/// else the policy of the document of its frame (for a navigation without
/// one: the frame's current document, which started it).
pub(crate) fn request_referrer(browser: &Browser, request: &FetchRequest) -> pyro_net::referrer::ReferrerChain {
    let referrer = request.referrer.as_deref().and_then(|r| url::Url::parse(r).ok());
    // The document's policy is read only when a referrer could be sent.
    let policy = match (&referrer, request.referrer_policy) {
        (_, Some(policy)) => policy,
        (None, None) => Default::default(),
        (Some(_), None) => request
            .frame_id
            .as_deref()
            .and_then(|id| browser.page(request.page)?.frames.key_of(id))
            .and_then(|key| browser.page(request.page)?.frames.page_dom(key))
            .map(|page_dom| page_dom.referrer_policy())
            .unwrap_or_default(),
    };
    pyro_net::referrer::ReferrerChain::new(referrer, policy)
}

/// The state of `node`'s `referrerpolicy` attribute: the policy its request
/// is sent with (`None`: its document's).
pub(crate) fn element_referrer_policy(dom: &pyro_dom::Dom, node: NodeId) -> Option<pyro_net::ReferrerPolicy> {
    pyro_net::ReferrerPolicy::from_attribute(dom.get_attribute(node, "referrerpolicy").ok().flatten())
}

/// The policy a frame's container (`<iframe referrerpolicy>`) navigates it
/// with: its own state, else the policy of its document (`parent`), which
/// is the request's client (not the frame's current document).
pub(crate) fn container_referrer_policy(
    dom: &pyro_dom::Dom,
    owner: NodeId,
    parent: Option<&pyro_webapi::PageDom>,
) -> Option<pyro_net::ReferrerPolicy> {
    element_referrer_policy(dom, owner).or_else(|| parent.map(|p| p.referrer_policy()))
}

/// The referrer policy of the document that holds the frame of `key`: what
/// an `about:srcdoc` document starts with (its policy container is a clone
/// of its container document's), until a `<meta name=referrer>` of its own.
pub(crate) fn parent_referrer_policy(browser: &Browser, key: FrameKey) -> Option<pyro_net::ReferrerPolicy> {
    let parent = browser.doc(key)?.main_frame.parent.clone()?;
    let page = browser.page(key.page)?;
    let parent = page.frames.key_of(&parent.0)?;
    Some(page.frames.page_dom(parent)?.referrer_policy())
}

/// The serialized origin of the parent of frame `frame_id`.
fn parent_origin(browser: &Browser, page: PageId, frame_id: Option<&str>) -> Option<String> {
    let key = browser.page(page)?.frames.key_of(frame_id?)?;
    let parent = browser.doc(key)?.main_frame.parent.clone()?;
    frame_origin(browser, page, &parent.0)
}

/// The serialized origin of frame `id`'s document, read in place (the
/// loader asks for it on every subresource; `Browser::frame` would clone
/// the whole frame, realms included, to answer).
fn frame_origin(browser: &Browser, page: PageId, id: &str) -> Option<String> {
    let key = browser.page(page)?.frames.key_of(id)?;
    Some(browser.doc(key)?.main_frame.origin.clone())
}

/// (MIME, body, response delay) per URL.
type StaticEntry = (String, Vec<u8>, Duration);

/// In-memory loader for tests and `Page.setDocumentContent`-style use.
#[derive(Default)]
pub struct StaticLoader {
    entries: RefCell<std::collections::HashMap<String, StaticEntry>>,
    /// The `Content-Security-Policy` header value served with a URL.
    csp: RefCell<std::collections::HashMap<String, String>>,
    pub log: RefCell<Vec<String>>,
    /// The `Referrer-Policy` header value served with a URL.
    referrer_policy: RefCell<std::collections::HashMap<String, String>>,
    /// Each request's URL and the `Referer` the network loader would send
    /// with it (its referrer under its policy), in request order.
    pub referers: RefCell<Vec<(String, Option<String>)>>,
}

impl StaticLoader {
    pub fn insert(&self, url: &str, mime: &str, body: &str, delay: Duration) {
        self.insert_bytes(url, mime, body.as_bytes(), delay);
    }
    pub fn insert_bytes(&self, url: &str, mime: &str, body: &[u8], delay: Duration) {
        self.entries.borrow_mut().insert(url.to_string(), (mime.to_string(), body.to_vec(), delay));
    }
    /// Serves `url` with a `Content-Security-Policy: {policy}` header.
    pub fn set_csp(&self, url: &str, policy: &str) {
        self.csp.borrow_mut().insert(url.to_string(), policy.to_string());
    }
    /// Serves `url` with a `Referrer-Policy: {policy}` header.
    pub fn set_referrer_policy(&self, url: &str, policy: &str) {
        self.referrer_policy.borrow_mut().insert(url.to_string(), policy.to_string());
    }
    /// The `Referer` of the last request for `url`: `None` if it was not
    /// requested, `Some(None)` if it was sent without one.
    pub fn referer_of(&self, url: &str) -> Option<Option<String>> {
        self.referers.borrow().iter().rev().find(|(u, _)| u == url).map(|(_, r)| r.clone())
    }
}

impl ResourceLoader for StaticLoader {
    fn fetch(&self, browser: &mut Browser, request: FetchRequest, done: FetchDone) {
        let start = request.part.map_or(0, |p| p.start);
        self.log.borrow_mut().push(if start > 0 {
            format!("{} bytes={start}-", request.url)
        } else {
            request.url.clone()
        });
        let sent_referrer =
            url::Url::parse(&request.url).ok().and_then(|u| request_referrer(browser, &request).header(&u));
        self.referers.borrow_mut().push((request.url.clone(), sent_referrer.clone()));
        let document = matches!(request.kind, ResourceKind::Document | ResourceKind::Iframe);
        let sent_referrer = document.then(|| sent_referrer.unwrap_or_default());
        let referrer_policy = self
            .referrer_policy
            .borrow()
            .get(&request.url)
            .and_then(|v| pyro_net::ReferrerPolicy::from_header([v.as_str()]));
        let entry = self.entries.borrow().get(&request.url).cloned();
        let csp = self.csp.borrow().get(&request.url).cloned();
        let url = request.url;
        let part = request.part;
        let delay = entry.as_ref().map_or(Duration::ZERO, |e| e.2);
        let id = browser.register_completion(done);
        browser.scheduler().set_timer(delay, false, true, move |b| {
            let result = match entry.clone() {
                Some((mime, mut body, _)) => {
                    // A `part` request: the range, cut as the network
                    // loader cuts it (all bytes arrive at once here).
                    let status = if start > 0 && (start as usize) < body.len() {
                        body.drain(..start as usize);
                        206
                    } else {
                        200
                    };
                    let total = body.len() as u64;
                    let mut checked = pyro_webapi::BodyPart::FIRST_CHECK;
                    let mut keep = None;
                    if let Some(part) = &part {
                        while keep.is_none() && checked <= body.len().min(part.limit) {
                            let at = checked;
                            keep = body_cut(part, &body[..at], Some(total), &mut checked);
                        }
                        keep = keep.or_else(|| (body.len() > part.limit).then_some(part.limit));
                    }
                    let partial = keep.filter(|&k| k < body.len()).map(|k| {
                        body.truncate(k);
                        pyro_webapi::Partial { total: Some(total) }
                    });
                    Ok(Resource {
                        url: url.clone(),
                        status,
                        mime,
                        charset: None,
                        body: body.into(),
                        immutable_local: true,
                        content_disposition: None,
                        header_csp: csp.clone(),
                        refresh: None,
                        content_language: None,
                        referrer_policy,
                        sent_referrer: sent_referrer.clone(),
                        timing: None,
                        partial,
                        nosniff: false,
                        opaque: false,
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
    /// From an external file (a `src`): only such a script gets `load`.
    external: bool,
}

/// HTML "list of scripts that will execute in order as soon as possible" of
/// a frame's document: script-inserted external scripts that are neither
/// `async` nor force async (`script.async = false`). Each runs once it and
/// every script before it are fetched; each delays the document's `load`
/// (`counted`, when the document has a `DocumentLoad`). The list belongs to
/// the document of `loader`: another document's load starts a new one.
#[derive(Default)]
pub(crate) struct InOrderScripts {
    loader: Option<LoaderId>,
    list: VecDeque<(Rc<RefCell<PendingScript>>, bool)>,
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
    pub(crate) dom: DomRef,
    pub(crate) parser: Option<DocumentParser>,
    /// Requests started ahead of the parser (`speculate`).
    pub(crate) speculation: crate::speculate::Speculation,
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
    sched: pyro_runtime::Scheduler<Browser>,
    fetches: Rc<ModuleFetches>,
    /// For a worklet's module graph (`ModuleFetcher::for_worklet_graph`): the graph's credentials
    /// mode, with which every module is fetched, outside the document's module map (`preloads`).
    worklet_credentials: Option<pyro_net::Credentials>,
    /// That worklet's module map, where each module this fetcher loads is recorded.
    worklet_modules: Option<(pyro_js::WorkletModuleMap, u64)>,
}

/// A worklet module's entry in its worklet's module map while it is fetched. It is marked fetched
/// before its source goes to the engine (which may run the rest of the graph, and script waiting on
/// it, before returning) and unmarked if the engine rejects it; dropped with a fetch that never
/// completed, it takes the module out of the map. "Fetched" says nothing of the module's imports:
/// `addModule()` of it waits for its graph (paint_worklet.rs `await_graph`).
struct WorkletEntry {
    modules: pyro_js::WorkletModuleMap,
    url: String,
    graph: u64,
    loaded: std::cell::Cell<bool>,
}

impl WorkletEntry {
    fn new(modules: pyro_js::WorkletModuleMap, url: &str, graph: u64) -> WorkletEntry {
        modules.borrow_mut().insert(url.to_owned(), pyro_js::WorkletModule { fetched: false, graph });
        WorkletEntry { modules, url: url.to_owned(), graph, loaded: std::cell::Cell::new(false) }
    }

    fn set_loaded(&self, loaded: bool) {
        self.loaded.set(loaded);
        let mut modules = self.modules.borrow_mut();
        if loaded {
            modules.insert(self.url.clone(), pyro_js::WorkletModule { fetched: true, graph: self.graph });
        } else {
            modules.remove(&self.url);
        }
    }
}

impl Drop for WorkletEntry {
    fn drop(&mut self) {
        let mut modules = self.modules.borrow_mut();
        if !self.loaded.get() && modules.get(&self.url).is_some_and(|m| !m.fetched) {
            modules.remove(&self.url);
        }
    }
}

/// The module fetches of a realm in flight, and who waits for there to be
/// none ([`ModuleFetcher::when_idle`]).
#[derive(Default)]
struct ModuleFetches {
    in_flight: std::cell::Cell<usize>,
    idle: RefCell<Vec<Box<dyn FnOnce()>>>,
}

/// One fetch in flight: from the request until its result is handed to the
/// engine, or the request is dropped.
struct InFlight(Rc<ModuleFetches>);

impl InFlight {
    fn new(fetches: &Rc<ModuleFetches>) -> InFlight {
        fetches.in_flight.set(fetches.in_flight.get() + 1);
        InFlight(fetches.clone())
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        let left = self.0.in_flight.get() - 1;
        self.0.in_flight.set(left);
        if left == 0 {
            let waiters = std::mem::take(&mut *self.0.idle.borrow_mut());
            for then in waiters {
                then();
            }
        }
    }
}

impl ModuleFetcher for PageModuleFetcher {
    fn document_base_url(&self, realm: &Realm) -> Option<String> {
        let page = pyro_webapi::target::page_of(&realm.scope().ok()?).ok()?;
        let dom = page.dom.try_borrow().ok()?;
        dom.document_base_url(page.document()).ok().map(str::to_owned)
    }

    fn when_idle(&self, then: Box<dyn FnOnce()>) {
        if self.fetches.in_flight.get() == 0 {
            then();
        } else {
            self.fetches.idle.borrow_mut().push(then);
        }
    }

    fn busy(&self) -> bool {
        self.fetches.in_flight.get() > 0
    }

    fn for_worklet_graph(&self, credentials: &str, modules: pyro_js::WorkletModuleMap, graph: u64) -> Option<Rc<dyn ModuleFetcher>> {
        let credentials = match credentials {
            "omit" => pyro_net::Credentials::Omit,
            "include" => pyro_net::Credentials::Include,
            _ => pyro_net::Credentials::SameOrigin,
        };
        Some(Rc::new(PageModuleFetcher {
            page: self.page,
            sched: self.sched.clone(),
            fetches: Rc::default(),
            worklet_credentials: Some(credentials),
            worklet_modules: Some((modules, graph)),
        }))
    }

    fn fetch(&self, realm: &Realm, url: String, request: ModuleRequest) {
        let ty = request.module_type();
        if let Some(e) = pyro_webapi::module_script::disallowed(ty, true) {
            if let Ok(scope) = realm.scope() {
                if let Err(e) = request.reject(&scope, &e) {
                    scope.report_exception(&e);
                }
            }
            return;
        }
        // A blob: URL names the blob it named when the module was requested
        // (URL parsing takes the blob URL entry): one revoked right after
        // `import()` still loads.
        let blob = if url.starts_with("blob:") {
            realm.scope().ok().and_then(|s| pyro_webapi::target::page_of(&s).ok()).and_then(|page| page.blob_url(&url))
        } else {
            None
        };
        // Called from inside JS: defer the network request to a task, where
        // `&mut Browser` is available.
        let realm = realm.downgrade();
        let page = self.page;
        let worklet_credentials = self.worklet_credentials;
        // Recorded in the worklet's module map as being fetched from now on.
        let worklet_entry = self.worklet_modules.clone().map(|(modules, graph)| WorkletEntry::new(modules, &url, graph));
        module_trace!("requested {url}");
        let request = RefCell::new(Some(request));
        let in_flight = InFlight::new(&self.fetches);
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
                // Dropped when this returns, the result handed to the engine.
                let _in_flight = in_flight;
                let fetched = res.is_ok();
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
                if let Some(entry) = worklet_entry.as_ref().filter(|_| fetched) {
                    entry.set_loaded(true);
                }
                let r = match res {
                    Ok(text) => pyro_webapi::module_script::complete(&scope, request, &text),
                    Err(e) => request.reject(
                        &scope,
                        &JsError::type_error(format!("Failed to fetch dynamically imported module: {traced} ({e})")),
                    ),
                };
                if let Some(entry) = worklet_entry.as_ref().filter(|_| fetched && r.is_err()) {
                    entry.set_loaded(false);
                }
                match &r {
                    Ok(()) => module_trace!("handed to engine {traced}"),
                    Err(e) => module_trace!("engine rejected {traced}: {e}"),
                }
                if let Err(e) = r {
                    scope.report_exception(&e);
                }
            };
            if let Some((bytes, mime)) = blob {
                let essence = pyro_net::parse_content_type(&mime).0;
                let result = match pyro_webapi::module_script::mime_error(ty, &essence, &url) {
                    Some(e) => Err(e),
                    None => Ok(pyro_webapi::module_script::utf8_decode(&bytes)),
                };
                complete(b, result);
                return;
            }
            // A modulepreload of this URL already fetched (or is fetching) it
            // as a JavaScript module.
            if let Some(credentials) = worklet_credentials {
                // A worklet graph's module: its own credentials mode and module map.
                let opts = ScriptFetch { module: Some(ty), credentials: Some(credentials), ..ScriptFetch::default() };
                b.fetch_script(page, url, ResourceKind::ModuleScript, opts, complete);
            } else if ty != ModuleType::JavaScript {
                let opts = ScriptFetch { module: Some(ty), ..ScriptFetch::default() };
                b.fetch_script(page, url, ResourceKind::ModuleScript, opts, complete);
            } else {
                b.fetch_imported_module(page, url, Box::new(complete));
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

pub(crate) fn resolve_url(base: &str, href: &str) -> Option<String> {
    // `String::from` takes the serialization; `to_string` would copy it.
    url::Url::parse(base).ok()?.join(href).ok().map(String::from)
}

/// The object a `blob:` URL names, from the blob URL store of a document of
/// `page` with the URL's origin: the requesting document's own, or a parent's
/// or child's for a same-origin frame (a parent's blob as an iframe `src`).
fn blob_body(browser: &Browser, page: PageId, url: &str) -> Option<pyro_net::BlobBody> {
    let host = browser.page(page)?.frames.clone();
    let origin = origin_of(url);
    host.keys().into_iter().find_map(|key| {
        let page_dom = browser.doc(key).filter(|d| d.main_frame.origin == origin)?.main_frame.page_dom.clone()?;
        let (bytes, mime) = page_dom.blob_url(url)?;
        Some(pyro_net::BlobBody { bytes, mime })
    })
}

/// The base URL `node`'s document resolves its URLs against: its first
/// `<base href>`, else the document's fallback base URL.
pub(crate) fn node_base_url(dom: &pyro_dom::Dom, node: NodeId) -> Option<String> {
    node_base_str(dom, node).map(str::to_owned)
}

/// [`node_base_url`] borrowed from the DOM.
fn node_base_str(dom: &pyro_dom::Dom, node: NodeId) -> Option<&str> {
    let doc = dom.node_document(node).ok()?;
    dom.document_base_url(doc).ok()
}

/// The `integrity` and `crossorigin` attributes a script fetch honours.
#[derive(Clone, Default)]
pub(crate) struct ScriptFetch {
    parser_priority: bool,
    pub integrity: Option<String>,
    pub crossorigin: Option<String>,
    /// The module type of a [`ResourceKind::ModuleScript`] fetch, whose
    /// response MIME type must match it (`None`: JavaScript).
    pub module: Option<ModuleType>,
    /// The element's `referrerpolicy` state (`None`: the document's policy).
    pub referrer_policy: Option<pyro_net::ReferrerPolicy>,
    /// The fetch's credentials mode, set by its initiator (a worklet's module graph).
    pub credentials: Option<pyro_net::Credentials>,
}

impl ScriptFetch {
    fn of(info: &pyro_dom::script::ScriptInfo) -> ScriptFetch {
        ScriptFetch {
            parser_priority: false,
            integrity: info.integrity.clone().filter(|v| !pyro_webapi::sri::is_empty(v)),
            crossorigin: info.crossorigin.clone(),
            module: None,
            referrer_policy: pyro_net::ReferrerPolicy::from_attribute(info.referrerpolicy.as_deref()),
            credentials: None,
        }
    }
}

/// pyro: whether `resource` is CORS-cross-origin (its tainting "opaque", Fetch): the response's
/// own tainting (a no-cors redirect chain through another origin is opaque though it ends at this
/// one), else the final URL's origin for a loader that does not report it.
pub(crate) fn is_opaque(resource: &Resource, cors: bool, document_origin: &str) -> bool {
    resource.opaque
        || (!cors && {
            let origin = origin_of(&resource.url);
            origin != "null" && origin != document_origin
        })
}

/// SRI for a fetched response: `None` when it may be used, else the
/// console message. A cross-origin response fetched without CORS is opaque
/// and never matches (Fetch: integrity metadata on a no-cors request).
pub(crate) fn integrity_error(
    resource: &Resource,
    integrity: Option<&str>,
    cors: bool,
    document_origin: &str,
) -> Option<String> {
    let integrity = integrity.filter(|v| !pyro_webapi::sri::is_empty(v))?;
    if is_opaque(resource, cors, document_origin) {
        return Some(format!(
            "Subresource Integrity: The resource '{}' has an integrity attribute, but the resource requires the \
             request to be CORS enabled to check the integrity, and it is not. The resource has been blocked \
             because the integrity cannot be enforced.",
            resource.url
        ));
    }
    (!pyro_webapi::sri::matches(integrity, &resource.body)).then(|| {
        format!(
            "Failed to find a valid digest in the 'integrity' attribute for resource '{}' with computed SHA-256 \
             integrity '{}'. The resource has been blocked.",
            resource.url,
            pyro_webapi::sri::sha256_expression(&resource.body)
        )
    })
}

/// Whether a document request that failed with `error` (a loader error)
/// shows Chrome's error document: any network error (`net::ERR_...`) but an
/// aborted navigation (a download, a cancel) and the URLs Chrome hands
/// elsewhere or never navigates to (an unknown scheme, an invalid URL).
fn shows_error_document(error: &str) -> bool {
    let Some(code) = error.strip_prefix("net::ERR_") else { return false };
    let code = &code[..code.bytes().take_while(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || *b == b'_').count()];
    !code.is_empty() && !matches!(code, "ABORTED" | "UNKNOWN_URL_SCHEME" | "INVALID_URL")
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
                error_page: false,
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
            return Err(JsError::Unsupported(&"navigation without a resource loader"));
        };
        let frame_id = self.doc(page).map(|p| p.main_frame.id.0.clone());
        // Not an automation client's (`Page.navigate`) or a traversal, which
        // `document.open()` does not stop.
        let script = opts.cdp_request_id.is_none()
            && !matches!(opts.reason, crate::NavigationReason::AddressBar | crate::NavigationReason::History);
        // Chrome: web content cannot navigate a frame to a local file ("Not
        // allowed to load local resource"), whatever its referrer policy.
        // The initiator is the parent document for a child frame, else the
        // frame's current document.
        if script && opts.url.get(..5).is_some_and(|s| s.eq_ignore_ascii_case("file:")) {
            let initiator = match page.child {
                Some(_) => self
                    .doc(page)
                    .and_then(|p| p.main_frame.parent.clone())
                    .and_then(|parent| self.frame(page.page, &parent))
                    .map(|f| f.origin),
                None => self.doc(page).map(|p| p.main_frame.origin.clone()),
            };
            if initiator.is_some_and(|o| o.starts_with("http:") || o.starts_with("https:")) {
                self.navigation_failed(page, &opts, loader_id, "net::ERR_ACCESS_DENIED".into());
                return Ok(());
            }
        }
        if let Some(mut p) = self.doc_mut(page) {
            p.pending_loader = Some(loader_id.clone());
            p.pending_script = script.then(|| (loader_id.clone(), opts.url.clone()));
        }
        let req = FetchRequest {
            parser_priority: false,
            credentials: None,
            post: opts.post.clone(),
            url: opts.url.clone(),
            kind: if page.child.is_some() { ResourceKind::Iframe } else { ResourceKind::Document },
            page: page.page,
            referrer: opts.referrer.clone(),
            referrer_policy: opts.referrer_policy,
            frame_id,
            loader_id: Some(loader_id.0.clone()),
            crossorigin: None,
            // Resource Timing: only the container's navigation of its frame
            // (`src` processing) is reported, to the parent. One the frame's
            // document starts, a reload or a traversal stays the frame's own:
            // its URLs are not the parent's to read.
            initiator: (page.child.is_some() && opts.reason == crate::NavigationReason::InitialFrameNavigation)
                .then_some(pyro_webapi::resource_timing::Initiator::Iframe),
            module: None,
            part: None,
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
                                    body: Ok(bytes::Bytes::from(std::sync::Arc::unwrap_or_clone(resource.body))),
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
                    Err(e) => b.document_fetch_failed(page, opts, loader_id, e, history),
                }
            }),
        );
        if let Some(mut d) = self.doc_mut(page).filter(|d| d.pending_loader.as_ref() == Some(&pending_id)) {
            d.pending_cancel = cancel;
        }
        Ok(())
    }

    /// The document request of a navigation failed on the network. A new
    /// window's navigation (its top-level frame still shows its first
    /// about:blank document: a popup, a new target) then commits an error
    /// document ([`Browser::commit_error_document`]), as Chrome commits
    /// one, so the window has a document and clients waiting for its first
    /// commit (Playwright's `popup` event) go on. Any other navigation fails
    /// and leaves the document as it was (Chrome would show its error page
    /// there too).
    fn document_fetch_failed(
        &mut self,
        page: FrameKey,
        opts: NavigateOpts,
        loader_id: LoaderId,
        error: String,
        history: HistoryHandling,
    ) {
        let new_window = page.child.is_none() && self.doc(page).is_some_and(|d| d.nav.is_first_blank());
        if new_window && shows_error_document(&error) {
            self.commit_error_document(page, opts, loader_id, error, history);
        } else {
            self.navigation_failed(page, &opts, loader_id, error);
        }
    }

    /// Commits Chrome's error document for a navigation that failed with
    /// `error`: an empty HTML document at [`crate::ERROR_DOCUMENT_URL`]
    /// (chromium-headless-shell's is empty too; Chrome's full build fills
    /// it with its "This site can't be reached" page), opaque origin, under
    /// the failed navigation's loader, with the URL it could not reach as
    /// its frame's `unreachable` and its session history entry's URL (a
    /// reload fetches that URL again). As in Chrome, `Page.navigate` first
    /// gets the error (`FrameNavigateFailed` with `error_page`), then the
    /// document commits and loads.
    fn commit_error_document(
        &mut self,
        page: FrameKey,
        opts: NavigateOpts,
        loader_id: LoaderId,
        error: String,
        history: HistoryHandling,
    ) {
        let Some(frame_id) = self.doc(page).map(|p| p.main_frame.id.clone()) else { return };
        self.forget_network_activity(page, &loader_id);
        self.emit(
            page.page,
            PageEvent::FrameNavigateFailed {
                frame_id: frame_id.clone(),
                loader_id: loader_id.clone(),
                url: opts.url.clone(),
                error: error.clone(),
                cdp_request_id: opts.cdp_request_id,
                error_page: true,
            },
        );
        let unreachable = crate::NavigationError { url: opts.url.clone(), error };
        let document = NavigateOpts { post: None, url: crate::ERROR_DOCUMENT_URL.into(), cdp_request_id: None, ..opts };
        let resource = Resource {
            url: crate::ERROR_DOCUMENT_URL.into(),
            status: 200,
            mime: "text/html".into(),
            charset: None,
            body: Default::default(),
            immutable_local: false,
            content_disposition: None,
            header_csp: None,
            refresh: None,
            content_language: None,
            referrer_policy: None,
            sent_referrer: None,
            timing: None,
            partial: None,
                nosniff: false,
            opaque: false,
        };
        if let Err(e) =
            self.begin_document_with(page, &document, loader_id.clone(), resource, history, Some(unreachable))
        {
            // Nothing committed: the navigation ends as a plain failure.
            self.emit(
                page.page,
                PageEvent::FrameNavigateFailed {
                    frame_id,
                    loader_id,
                    url: document.url,
                    error: e.to_string(),
                    cdp_request_id: None,
                    error_page: false,
                },
            );
        }
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
                error_page: false,
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
        self.begin_document_with(page, opts, loader_id, resource, history, None)
    }

    /// [`Browser::begin_document`] of the error document of `unreachable`
    /// when it is set ([`Browser::commit_document`]).
    fn begin_document_with(
        &mut self,
        page: FrameKey,
        opts: &NavigateOpts,
        loader_id: LoaderId,
        mut resource: Resource,
        history: HistoryHandling,
        unreachable: Option<crate::NavigationError>,
    ) -> Result<(), JsError> {
        let origin = origin_of(&resource.url);
        // The policy the navigation request was sent with (its initiator's).
        let policy = match (&opts.referrer, opts.referrer_policy) {
            (Some(_), None) if resource.sent_referrer.is_none() => {
                self.doc(page).and_then(|p| p.main_frame.page_dom.as_ref().map(|d| d.referrer_policy()))
            }
            (_, policy) => policy,
        };
        // Navigation Timing: the commit takes the fetch's phases (and a
        // failed commit must not leave them for the next one).
        let nav = self.doc(page).map(|p| p.nav.clone());
        if let Some(nav) = &nav {
            nav.set_incoming_fetch(resource.timing.take());
        }
        let committed = self.commit_document(page, opts, loader_id.clone(), &origin, history, unreachable);
        if let Some(nav) = &nav {
            nav.set_incoming_fetch(None);
        }
        committed?;
        let page_dom = self.doc(page).and_then(|p| p.main_frame.page_dom.clone()).ok_or(JsError::ContextDestroyed)?;
        match resource.sent_referrer.take() {
            // What the request's last hop was sent (a redirect chain may have cut it).
            Some(sent) => page_dom.set_sent_referrer(sent, resource.referrer_policy),
            None => page_dom.set_document_referrer(
                opts.referrer.as_deref(),
                policy.unwrap_or_default(),
                resource.referrer_policy,
            ),
        }
        let dom = page_dom.dom.clone();
        let doc = page_dom.document();
        // A page that bypasses CSP (`Page.setBypassCSP`) records no policy.
        if let Some(csp) = resource.header_csp.as_deref().filter(|_| !self.page(page.page).is_some_and(|p| p.bypass_csp)) {
            let (enforce, report_only) = csp.split_once('\n').unwrap_or((csp, ""));
            if !enforce.is_empty() {
                let _ = dom.borrow_mut().set_header_csp(doc, true);
            }
            let _ = dom.borrow_mut().set_trusted_types_policies(doc, enforce, report_only);
        }
        if let Some(refresh) = resource.refresh.as_deref() {
            self.refresh_header(page, refresh);
        }
        if resource.content_language.is_some() {
            let _ = dom.borrow_mut().set_content_language(doc, resource.content_language.as_deref());
        }
        let json_document = pyro_net::is_json_mime(&resource.mime);
        let mut parser = if json_document {
            DocumentParser::new_json(dom.clone(), doc, &resource.mime)
        } else if resource.mime == "text/plain" {
            DocumentParser::new_plaintext(dom.clone(), doc)
        } else if pyro_dom::parse::is_xml_mime(&resource.mime) {
            DocumentParser::new_xml(dom.clone(), doc, &resource.mime)
        } else {
            DocumentParser::new(dom.clone(), doc, !page_dom.scripting_disabled())
        }
        .map_err(|e| JsError::type_error(format!("{e:?}")))?;
        parser.set_transport_charset(resource.charset.as_deref());
        // HTML encoding sniffing: a child document without a declared encoding
        // inherits its same-origin container document's.
        if page.child.is_some() {
            let parent = self.page(page.page).map(|p| p.frames.parent_key(page)).filter(|&k| k != page);
            let inherited = parent.and_then(|k| self.doc(k)).and_then(|d| d.main_frame.page_dom.clone()).and_then(|pd| {
                let dom = pd.dom.borrow();
                let data = dom.document_data(pd.document()).ok()?;
                (origin_of(&data.url.to_string()) == origin).then_some(data.charset)
            });
            parser.set_fallback_charset(inherited);
        }
        // An image is shown in an image document (HTML § 7.6.6 "Page load processing model for
        // media"): Chromium's markup, the image at the top left in a frame, centred on a dark
        // background in a window.
        let body = if resource.mime.starts_with("image/") && !json_document && !pyro_dom::parse::is_xml_mime(&resource.mime) {
            let src = resource.url.replace('&', "&amp;").replace('"', "&quot;");
            let (body, img) = match page.child {
                Some(_) => ("margin: 0px;", "display: block;"),
                None => (
                    "margin: 0px; height: 100%; background-color: rgb(14, 14, 14);",
                    "display: block;-webkit-user-select: none;margin: auto;background-color: hsl(0, 0%, 90%);",
                ),
            };
            format!("<html><head><meta name=\"viewport\" content=\"width=device-width, minimum-scale=0.1\"></head><body style=\"{body}\"><img style=\"{img}\" src=\"{src}\"></body></html>")
                .into_bytes()
        } else {
            std::sync::Arc::unwrap_or_clone(resource.body)
        };
        let mut p = self.doc_mut(page).ok_or(JsError::Type("no such page".into()))?;
        p.main_frame.dom = Some(dom.clone());
        p.main_frame.document = Some(doc);
        p.nav.set_document(Some((dom.clone(), doc)));
        p.load = Some(DocumentLoad {
            loader_id,
            url: p.main_frame.base_url.clone(),
            dom,
            parser: Some(parser),
            speculation: Default::default(),
            phase: ParsePhase::Feed(body),
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

    pub(crate) fn load_mut(&mut self, page: FrameKey) -> Option<crate::frames::StateMut<'_, DocumentLoad>> {
        self.doc_mut(page)?.filter_map(|p| p.load.as_mut())
    }

    pub(crate) fn current_loader(&self, page: FrameKey) -> Option<LoaderId> {
        self.doc(page)?.load.as_ref().map(|l| l.loader_id.clone())
    }

    /// Runs the parser until it blocks on a script or finishes.
    fn continue_parsing(&mut self, page: FrameKey) {
        loop {
            // The parser runs with no borrow of the frame state held: it
            // runs custom element constructors and callbacks (HTML "create
            // an element for a token"), which may reach the frame tree.
            let (mut parser, phase, loader_id) = {
                let Some(mut guard) = self.load_mut(page) else { return };
                let load = &mut *guard;
                if load.blocked || load.parsing_done || matches!(load.phase, ParsePhase::Done) {
                    return;
                }
                let Some(parser) = load.parser.take() else { return };
                let next = match load.phase {
                    ParsePhase::Finish | ParsePhase::FinishResume => ParsePhase::FinishResume,
                    _ => ParsePhase::Resume,
                };
                (parser, std::mem::replace(&mut load.phase, next), load.loader_id.clone())
            };
            let step = match phase {
                ParsePhase::Feed(body) => parser.feed_bytes(&body),
                ParsePhase::Resume | ParsePhase::FinishResume => parser.resume(),
                ParsePhase::Finish => parser.finish_bytes(),
                ParsePhase::Done => unreachable!("checked above"),
            };
            match self.load_mut(page) {
                // A constructor may have started another document's load.
                Some(mut load) if load.loader_id == loader_id => load.parser = Some(parser),
                _ => return,
            }
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
                            load.speculation.end_of_parsing();
                            drop(load);
                            if let Some(dom) = self.doc(page).and_then(|p| p.main_frame.page_dom.clone()) {
                                dom.set_parser_position(None);
                                pyro_webapi::lifecycle::set_ready_state(&dom, dom.document(), "interactive");
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
        // A blocked script is prepared once the stylesheets load, where the spec
        // prepares at the end tag and only waits to execute. A script in
        // another document now (its element was moved away) is prepared at
        // once, which returns: waiting would prepare it wherever it is later,
        // back in this document if it was moved back.
        let moved_away = || {
            self.doc(page)
                .and_then(|p| p.main_frame.page_dom.clone())
                .is_some_and(|p| p.dom.borrow().node_document(node).ok() != Some(p.document()))
        };
        if blocks && !moved_away() && self.stylesheets_block_scripts(page) {
            let mut load = self.load_mut(page).expect("load");
            if load.dom.borrow_mut().add_ref(node).is_ok() {
                load.stylesheet_blocked_script = Some(Rc::new(ResourceNode { dom: load.dom.clone(), node }));
                load.blocked = true;
                drop(load);
                // The script's fetch, and the requests after it, overlap the
                // stylesheets' (the script runs when they are done).
                if !self.doc(page).is_some_and(|d| d.main_frame.sandbox.scripts_disabled) {
                    self.speculate_script(page, node);
                    self.speculate_ahead(page);
                }
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
            pyro_webapi::mutation_observer::checkpoint(&bindings);
        }
        if self.doc(page).is_some_and(|d| d.main_frame.sandbox.scripts_disabled) {
            return;
        }
        // The script's parser document: the document this load parses.
        let parser_document = self.doc(page).and_then(|p| p.main_frame.document);
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
                // Prepare step 4: without an `async` attribute it is now
                // force async (`script.async` is true; reinserted, it runs as
                // soon as possible).
                if !info.async_ {
                    let _ = d.set_script_force_async(node, true);
                }
                return;
            }
            let _ = d.mark_script_started(node);
            // Prepare step 15: a script moved into another document before it
            // was prepared (the parser kept inserting into the moved element)
            // does not run there, nor after it is moved back. It stays
            // "already started", so it fires no events either. (A script moved
            // after this point is `execute_classic`'s check.)
            if parser_document.is_some_and(|parser| d.node_document(node).ok() != Some(parser)) {
                return;
            }
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
            Some(s) => {
                // `<base href>` aware; the inline-script URL stays `base`.
                let resolved = {
                    let dom = element.dom.borrow();
                    resolve_url(node_base_str(&dom, node).unwrap_or(&base), s)
                };
                match resolved {
                    Some(u) => Some(u),
                    None => return, // spec: error event (needs pyro-webapi)
                }
            }
            None => None,
        };
        if src_url.is_some() {
            // A parser-blocking script is implicitly render-blocking.
            self.block_rendering_on_script(page, node, ty == ScriptType::Classic && !info.async_ && !info.defer);
        }
        match (ty, src_url) {
            (ScriptType::Classic, None) => self.execute_at_insertion_point(page, &info.inline_text, &base, &element),
            (ScriptType::Classic, Some(url)) if info.async_ => {
                self.fetch_async(page, ty, url, element, ScriptFetch::of(&info))
            }
            (ScriptType::Classic, Some(url)) if info.defer => {
                self.fetch_deferred(page, ty, url, element, ScriptFetch::of(&info))
            }
            (ScriptType::Classic, Some(url)) => {
                // Parser-blocking: pause until fetched and executed.
                let loader_id = self.current_loader(page);
                if let Some(mut l) = self.load_mut(page) {
                    l.blocked = true;
                }
                let u = url.clone();
                let opts = ScriptFetch { parser_priority: true, ..ScriptFetch::of(&info) };
                self.fetch_script(page, url, ResourceKind::Script, opts, move |b, res| {
                    if b.current_loader(page) != loader_id {
                        return;
                    }
                    if let Some(mut l) = b.load_mut(page) {
                        l.blocked = false;
                    }
                    match res {
                        _ if b.script_moved(page, &element) => {}
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
                self.speculate_ahead(page);
            }
            (ScriptType::Module, Some(url)) if info.async_ => {
                self.fetch_async(page, ty, url, element, ScriptFetch::of(&info))
            }
            (ScriptType::Module, Some(url)) => self.fetch_deferred(page, ty, url, element, ScriptFetch::of(&info)),
            (ScriptType::Module, None) => {
                // The script's base URL is the document base URL when it is
                // prepared (a `<base>` before it counts).
                let script_base = node_base_url(&element.dom.borrow(), node).unwrap_or_else(|| base.clone());
                let seq = {
                    let mut l = self.load_mut(page).expect("load");
                    l.inline_module_seq += 1;
                    l.inline_module_seq
                };
                let url = self.inline_module_identity(page, &script_base, &seq.to_string());
                if info.async_ {
                    self.execute_module(page, &info.inline_text, &url, false, Some(element), false);
                } else {
                    let ps = PendingScript { element, ty, url, source: Some(Ok(info.inline_text)), external: false };
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
        self.fetch_script(page, url, kind, ScriptFetch::default(), done);
    }

    /// [`Browser::fetch_text`] honouring a script's `crossorigin` (CORS
    /// mode) and `integrity` (a mismatch is an error).
    pub(crate) fn fetch_script(
        &mut self,
        page: FrameKey,
        url: String,
        kind: ResourceKind,
        opts: ScriptFetch,
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
        let ScriptFetch { integrity, crossorigin, module, referrer_policy, credentials, parser_priority } = opts;
        let module = (kind == ResourceKind::ModuleScript).then(|| module.unwrap_or(ModuleType::JavaScript));
        let cors = kind == ResourceKind::ModuleScript || crossorigin.is_some();
        let initiator = Some(pyro_webapi::resource_timing::Initiator::Script);
        let req = FetchRequest {
            parser_priority,
            post: None,
            url,
            kind,
            page: page.page,
            referrer,
            referrer_policy,
            frame_id,
            loader_id,
            crossorigin,
            credentials,
            initiator,
            module,
            part: None,
        };
        let cancel = self.fetch_document_resource(
            page,
            &loader,
            req,
            Box::new(move |b, res| {
                let res = match res {
                    Ok(r) if (200..300).contains(&r.status) => {
                        let blocked = integrity
                            .as_deref()
                            .and_then(|i| {
                                let origin = b.doc(page).map(|p| p.main_frame.origin.clone()).unwrap_or_default();
                                integrity_error(&r, Some(i), cors, &origin)
                            })
                            .or_else(|| {
                                module.and_then(|ty| pyro_webapi::module_script::mime_error(ty, &r.mime, &r.url))
                            })
                            .or_else(|| {
                                // Fetch: a classic script is blocked by nosniff
                                // or an audio/image/video/CSV MIME type.
                                (kind == ResourceKind::Script && pyro_net::script_response_blocked(&r.mime, r.nosniff))
                                    .then(|| {
                                        format!(
                                            "Refused to execute script from '{}': MIME type ('{}') is not executable",
                                            r.url, r.mime
                                        )
                                    })
                            });
                        match blocked {
                            Some(e) => Err(e),
                            // A module script is UTF-8 whatever its charset
                            // says (HTML "fetch a single module script").
                            None if module.is_some() => Ok(pyro_webapi::module_script::utf8_decode(&r.body)),
                            None => Ok(r.text()),
                        }
                    }
                    Ok(r) => Err(format!("HTTP {} for {}", r.status, r.url)),
                    Err(e) => Err(e),
                };
                done(b, res)
            }),
        );
        if let Some(cancel) = cancel {
            cancel.disarm();
        }
    }

    fn fetch_deferred(
        &mut self,
        page: FrameKey,
        ty: ScriptType,
        url: String,
        element: Rc<ResourceNode>,
        opts: ScriptFetch,
    ) {
        let ps = Rc::new(RefCell::new(PendingScript { element, ty, url: url.clone(), source: None, external: true }));
        self.load_mut(page).expect("load").deferred.push_back(ps.clone());
        let kind = if ty == ScriptType::Module { ResourceKind::ModuleScript } else { ResourceKind::Script };
        let loader_id = self.current_loader(page);
        self.fetch_script(page, url, kind, opts, move |b, res| {
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
    fn fetch_async(
        &mut self,
        page: FrameKey,
        ty: ScriptType,
        url: String,
        element: Rc<ResourceNode>,
        opts: ScriptFetch,
    ) {
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
        self.fetch_script(page, url, kind, opts, move |b, res| {
            if b.frame_loader(page) != loader_id {
                return;
            }
            match res {
                _ if b.script_moved(page, &element) => {}
                Ok(text) if ty == ScriptType::Classic => {
                    b.execute_external(page, &text, &u, &element);
                    b.script_loaded(page, &element, true);
                }
                Ok(text) => {
                    b.execute_module(page, &text, &u, false, Some(element), true);
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

    /// Fetches a script of the document's in-order list ([`InOrderScripts`])
    /// and runs the ready scripts at the front of the list when it arrives.
    fn fetch_in_order(
        &mut self,
        page: FrameKey,
        ty: ScriptType,
        url: String,
        element: Rc<ResourceNode>,
        opts: ScriptFetch,
    ) {
        let Some(loader_id) = self.frame_loader(page) else { return };
        let counted = match self.load_mut(page) {
            Some(mut l) => {
                l.async_pending += 1;
                true
            }
            None => false,
        };
        let ps = Rc::new(RefCell::new(PendingScript { element, ty, url: url.clone(), source: None, external: true }));
        {
            let Some(mut doc) = self.doc_mut(page) else { return };
            let in_order = &mut doc.in_order_scripts;
            if in_order.loader.as_ref() != Some(&loader_id) {
                in_order.loader = Some(loader_id.clone());
                in_order.list.clear();
            }
            in_order.list.push_back((ps.clone(), counted));
        }
        let kind = if ty == ScriptType::Module { ResourceKind::ModuleScript } else { ResourceKind::Script };
        self.fetch_script(page, url, kind, opts, move |b, res| {
            if b.frame_loader(page).as_ref() != Some(&loader_id) {
                return;
            }
            ps.borrow_mut().source = Some(res);
            b.run_in_order(page);
        });
    }

    /// Runs the scripts at the front of the in-order list while they are
    /// fetched, in list order, then checks for the document's `load`.
    fn run_in_order(&mut self, page: FrameKey) {
        loop {
            let next = {
                let Some(mut doc) = self.doc_mut(page) else { return };
                let loader = doc.main_frame.loader_id.clone();
                let in_order = &mut doc.in_order_scripts;
                if in_order.loader.as_ref() != Some(&loader) {
                    return;
                }
                if in_order.list.front().is_none_or(|(ps, _)| ps.borrow().source.is_none()) {
                    break;
                }
                in_order.list.pop_front()
            };
            let Some((ps, counted)) = next else { break };
            let (ty, url, element, source) = {
                let mut p = ps.borrow_mut();
                (p.ty, p.url.clone(), p.element.clone(), p.source.take())
            };
            let Some(source) = source else { continue };
            let loader_id = self.frame_loader(page);
            match source {
                _ if self.script_moved(page, &element) => {}
                Ok(text) if ty == ScriptType::Classic => {
                    self.execute_external(page, &text, &url, &element);
                    self.script_loaded(page, &element, true);
                }
                Ok(text) => {
                    self.execute_module(page, &text, &url, false, Some(element), true);
                }
                Err(e) => {
                    self.report_load_error(page, &e);
                    self.script_loaded(page, &element, false);
                }
            }
            // The script or its `load`/`error` handler may have replaced the
            // document: the rest of the list went with it.
            if self.frame_loader(page) != loader_id {
                return;
            }
            if counted {
                if let Some(mut l) = self.load_mut(page) {
                    l.async_pending -= 1;
                }
            }
        }
        self.maybe_fire_load(page);
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
            pyro_webapi::lifecycle::resource_loaded(&bindings, element.node, ok);
        }
    }

    /// HTML "execute the script element" step 2: a script whose node document
    /// is no longer its preparation-time document (the document of the frame
    /// that prepared it, `page`) was moved to another document since it was
    /// prepared. It returns before anything else: the script does not run,
    /// its fetch or parse error is not reported, and it gets neither `load`
    /// nor `error`. Checked where a prepared script's fetch completes, before
    /// executing it; rendering stops waiting for it.
    fn script_moved(&self, page: FrameKey, element: &ResourceNode) -> bool {
        let Some(bindings) = self.doc(page).and_then(|p| p.main_frame.page_dom.clone()) else { return false };
        let moved = bindings.dom.borrow().node_document(element.node).ok() != Some(bindings.document());
        if moved {
            bindings.unblock_rendering(element.node);
        }
        moved
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
            bindings.block_rendering(node, pyro_webapi::RenderBlocker::Implicit);
        } else if explicit {
            bindings.block_rendering(node, pyro_webapi::RenderBlocker::Explicit);
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
        // Prepared at its insertion (`prepare_inserted_script`): start it as
        // prepared, unless it was moved to another document since (it would
        // not run there; HTML "execute the script element" step 2).
        let prepared = dom.borrow_mut().take_prepared_script(node);
        let info = if let Some((info, document)) = prepared {
            if document != bindings.document() {
                return;
            }
            info
        } else {
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
            Some(s) => {
                let resolved = {
                    let dom = element.dom.borrow();
                    resolve_url(node_base_str(&dom, node).unwrap_or(&base), s)
                };
                match resolved {
                    Some(u) => Some(u),
                    None => {
                        self.script_loaded(page, &element, false);
                        return;
                    }
                }
            }
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
            // External scripts inserted by script ignore `defer`. With an
            // `async` attribute or force async they run as soon as they
            // arrive; otherwise (`script.async = false`) in insertion order.
            (ScriptType::Classic, Some(url)) | (ScriptType::Module, Some(url)) => {
                self.block_rendering_on_script(page, node, false);
                if info.async_ || info.force_async {
                    self.fetch_async(page, ty, url, element, ScriptFetch::of(&info))
                } else {
                    self.fetch_in_order(page, ty, url, element, ScriptFetch::of(&info))
                }
            }
            (ScriptType::Module, None) => {
                let script_base = node_base_url(&element.dom.borrow(), node).unwrap_or(base);
                let url = self.dynamic_inline_module_identity(page, &script_base);
                self.execute_module(page, &info.inline_text, &url, false, Some(element), false);
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
            let (ty, url, element, external) = {
                let n = next.borrow();
                (n.ty, n.url.clone(), n.element.clone(), n.external)
            };
            match source {
                // Moved to another document while it waited: the next one runs.
                _ if self.script_moved(page, &element) => {}
                Ok(text) if ty == ScriptType::Classic => {
                    self.execute_external(page, &text, &url, &element);
                    self.script_loaded(page, &element, true);
                }
                Ok(text) => {
                    if self.execute_module(page, &text, &url, true, Some(element), external) {
                        return; // resumes when the module's promise settles
                    }
                }
                Err(e) => {
                    self.report_load_error(page, &e);
                    self.script_loaded(page, &element, false);
                }
            }
        }
        // HTML queues DOMContentLoaded as a task: `<style>` events queued
        // before it fire first (the last one comes back here).
        if self.doc(page).is_some_and(|d| d.stylesheets.style_events_queued()) {
            return;
        }
        self.fire_dom_content_loaded(page);
    }

    /// HTML "download the hyperlink": blob:/data: bytes arrive with the
    /// request; same-origin HTTP(S) URLs are fetched. The result is reported
    /// as `PageEvent::Download` for the embedder (CDP download behavior).
    pub(crate) fn start_download(&mut self, page: FrameKey, request: pyro_webapi::DownloadRequest) {
        let Some(frame_id) = self.doc(page).map(|p| p.main_frame.id.clone()) else { return };
        let suggested = suggested_filename(&request.filename, &request.url);
        let attribute = request.filename.clone();
        let pyro_webapi::DownloadRequest { url, data, .. } = request;
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
                    parser_priority: false,
                    credentials: None,
                    post: None,
                    url: url.clone(),
                    kind: ResourceKind::Download,
                    page: page.page,
                    referrer,
                    referrer_policy: None,
                    frame_id: Some(frame_id.0.clone()),
                    loader_id,
                    crossorigin: None,
                    initiator: None,
                    module: None,
                    part: None,
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
                                Ok(bytes::Bytes::from(std::sync::Arc::unwrap_or_clone(r.body)))
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
            pyro_webapi::lifecycle::dom_content_loaded(&dom, dom.document());
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
            pyro_webapi::lifecycle::dom_content_loaded(&dom, dom.document());
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
            .any(|k| self.doc(*k).is_some_and(|d| !d.main_frame.loaded || d.pending_loader.is_some() || d.pending_frame_attributes > 0))
        {
            return;
        }
        let Some(mut state) = self.doc_mut(page) else { return };
        if state.main_frame.loaded
            || !state.main_frame.dom_content_loaded
            || state.stylesheets.pending() != 0
            || state.stylesheets.styles_pending()
            || state.images.blocking_load() != 0
            || state.main_frame.page_dom.as_ref().is_some_and(|d| d.media_delays_load())
            || state.main_frame.page_dom.as_ref().is_some_and(|d| d.fonts_delay_load())
        {
            return;
        }
        // pyro: the document's CSS images, once everything else is in (Chrome's style resolution
        // starts them before `load`, which waits for them).
        if let Some(page_dom) = state.main_frame.page_dom.clone() {
            if state.load.as_ref().is_none_or(|load| load.async_pending == 0 && load.dcl_fired) {
                drop(state);
                if page_dom.css_images_delay_load() {
                    return;
                }
                // pyro: the gate's layout pass may have made a pending face applicable for the
                // first time (a style change before load): fonts are checked again after it.
                if page_dom.fonts_delay_load() {
                    return;
                }
                let Some(new_state) = self.doc_mut(page) else { return };
                state = new_state;
            }
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
            pyro_webapi::lifecycle::load(&dom, dom.document());
        }
        if let Some(nav) = self.doc(page).map(|d| d.nav.clone()) {
            nav.notify_timeline("navigation");
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
        self.start_pending_refresh(page, frame.loader_id.clone());
        self.watch_network_idle(page, frame.loader_id);
        if let Some(parent) = frame.parent.and_then(|id| host.key(&id)) {
            if let Some(owner) = frame.owner {
                if let Some(dom) = host.frame(parent).and_then(|f| f.page_dom) {
                    pyro_webapi::lifecycle::resource_loaded(&dom, owner, true);
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
            dom.set_executing_classic_script(element.node);
            pyro_webapi::trusted_types::sync_compile_strings(&scope, dom);
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
    fn add_import_map(&mut self, page: FrameKey, info: &pyro_dom::script::ScriptInfo, base: &str) {
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

    /// Start the fetch at the script end tag. The stream schedules the ready
    /// script immediately, after a blocking predecessor, or after its own EOF.
    pub(crate) fn fetch_streamed_script(
        &mut self, page: FrameKey, node: NodeId, live: Rc<dyn Fn() -> bool>, ready: pyro_webapi::page::StreamScriptReady,
    ) {
        let empty = || Box::new(|done: Box<dyn FnOnce()>| done()) as pyro_webapi::page::StreamScriptJob;
        let Some(state) = self.doc(page) else { ready(empty()); return };
        let Some(bindings) = state.main_frame.page_dom.clone() else { ready(empty()); return };
        if state.main_frame.sandbox.scripts_disabled { ready(empty()); return }
        let base = state.main_frame.base_url.clone();
        drop(state);
        let dom = bindings.dom.clone();
        let Ok(info) = dom.borrow().script_info(node) else { ready(empty()); return };
        if dom.borrow_mut().add_ref(node).is_err() { ready(empty()); return }
        let element = Rc::new(ResourceNode { dom, node });
        let module = info.kind == ScriptKind::Module;
        let external = info.src.is_some();
        let url = match info.src.as_deref() {
            Some(src) => (!src.is_empty()).then(|| resolve_url(node_base_str(&element.dom.borrow(), node).unwrap_or(&base), src)).flatten(),
            None if module => {
                let script_base = node_base_url(&element.dom.borrow(), node).unwrap_or_else(|| base.clone());
                Some(self.dynamic_inline_module_identity(page, &script_base))
            }
            None => None,
        };
        let Some(url) = url else { self.script_loaded(page, &element, false); ready(empty()); return };
        let source = url.clone();
        let on_fetched = move |browser: &mut Browser, result: Result<String, String>| {
            let scheduler = browser.scheduler();
            ready(Box::new(move |done| scheduler.post(TaskSource::Networking, move |browser| {
                if !live() || !browser.doc(page).and_then(|p| p.main_frame.page_dom.clone()).is_some_and(|p| Rc::ptr_eq(&bindings, &p)) {
                    done();
                    return;
                }
                if browser.script_moved(page, &element) {
                    done();
                    return;
                }
                match result {
                    Ok(text) if module => {
                        let once = std::cell::RefCell::new(Some(done));
                        let after: Rc<dyn Fn()> = Rc::new(move || {
                            if let Some(f) = once.borrow_mut().take() { f(); }
                        });
                        if !browser.execute_module_then(page, &text, &source, false, Some(element), external, Some(after.clone())) {
                            after();
                        }
                    }
                    Ok(text) => {
                        browser.execute_external(page, &text, &source, &element);
                        browser.script_loaded(page, &element, true);
                        done();
                    }
                    Err(e) => {
                        browser.report_load_error(page, &e);
                        browser.script_loaded(page, &element, false);
                        done();
                    }
                }
            })));
        };
        if external {
            let kind = if module { ResourceKind::ModuleScript } else { ResourceKind::Script };
            self.fetch_script(page, url, kind, ScriptFetch::of(&info), on_fetched);
        } else {
            on_fetched(self, Ok(info.inline_text));
        }
    }

    fn prepare_written_script(&mut self, page: FrameKey, bindings: &Rc<pyro_webapi::PageDom>, node: NodeId) {
        let dom = bindings.dom.clone();
        let info = {
            let mut d = dom.borrow_mut();
            let Ok(info) = d.script_info(node) else { return };
            let _ = d.mark_script_started(node);
            info
        };
        let base = self.doc(page).map(|p| p.main_frame.base_url.clone()).unwrap_or_default();
        let src_url =
            info.src.as_deref().and_then(|s| resolve_url(node_base_str(&dom.borrow(), node).unwrap_or(&base), s));
        let blocking = bindings.written_script_pending() == Some(node);
        let Some(url) = src_url else {
            if blocking {
                pyro_webapi::lifecycle::written_script_fetched(bindings, node, "", Err("invalid URL".into()));
            } else if info.src.is_none() && info.kind == ScriptKind::Module {
                let script_base = node_base_url(&dom.borrow(), node).unwrap_or_else(|| base.clone());
                let url = self.dynamic_inline_module_identity(page, &script_base);
                let element =
                    dom.borrow_mut().add_ref(node).ok().map(|_| Rc::new(ResourceNode { dom: dom.clone(), node }));
                self.execute_module(page, &info.inline_text, &url, false, element, false);
            }
            return;
        };
        if blocking {
            let bindings = bindings.clone();
            let u = url.clone();
            self.fetch_script(page, url, ResourceKind::Script, ScriptFetch::of(&info), move |_, res| {
                pyro_webapi::lifecycle::written_script_fetched(&bindings, node, &u, res);
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
        self.fetch_script(page, url, kind, ScriptFetch::of(&info), move |b, res| {
            if !b.doc(page).and_then(|p| p.main_frame.page_dom.clone()).is_some_and(|d| Rc::ptr_eq(&d, &current)) {
                return;
            }
            match res {
                _ if b.script_moved(page, &element) => {}
                Ok(text) if module => {
                    b.execute_module(page, &text, &u, false, Some(element), true);
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

    /// Starts a module script; with `deferred`, parsing-completion work waits
    /// for it to run, otherwise the `load` event does. Returns whether such a
    /// wait was set up.
    ///
    /// HTML fetches the script's module graph, then runs it without awaiting
    /// its evaluation: a top-level await holds up neither the scripts after
    /// it nor DOMContentLoaded and load. With the engine's two steps
    /// ([`pyro_js::Scope::has_module_phases`]) the graph is fetched
    /// ([`pyro_js::Scope::load_module_graph`]), then linked and evaluated as
    /// soon as it is ready, after which the script has run. A fetch failure of the graph
    /// (the script's result is null) fires `error` at the element; anything
    /// else (a parse, link or evaluation error is reported) fires `load` at
    /// an `external` one and no event at an inline one.
    fn execute_module(
        &mut self,
        page: FrameKey,
        text: &str,
        url: &str,
        deferred: bool,
        element: Option<Rc<ResourceNode>>,
        external: bool,
    ) -> bool {
        self.execute_module_then(page, text, url, deferred, element, external, None)
    }

    fn execute_module_then(
        &mut self,
        page: FrameKey,
        text: &str,
        url: &str,
        deferred: bool,
        element: Option<Rc<ResourceNode>>,
        external: bool,
        after: Option<Rc<dyn Fn()>>,
    ) -> bool {
        let Some(realm) = self.main_realm(page) else { return false };
        let Ok(scope) = realm.scope() else { return false };
        let loader_id = self.frame_loader(page);
        let sched = self.scheduler();
        let counted = self.load_mut(page).is_some();
        // Ends the script in a task, after it ran: the element's event
        // (`Some(true)` load, `Some(false)` error), then what waits for it.
        let finish = {
            let element = element.clone();
            Rc::new(move |event: Option<bool>| {
                let loader_id = loader_id.clone();
                let element = element.clone();
                let after = after.clone();
                sched.post(TaskSource::Networking, move |b| {
                    let complete = || {
                    if b.frame_loader(page) != loader_id {
                        return;
                    }
                    if let Some(element) = &element {
                        b.script_ran(page, element, event);
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
                    };
                    let mut complete = complete;
                    complete();
                    if let Some(after) = after { after(); }
                });
            })
        };
        let ran = external.then_some(true);
        let started = match scope.load_module_graph(text, url) {
            Ok(Some(loaded)) => {
                let (fetched, failed) = (finish.clone(), finish.clone());
                let url = url.to_owned();
                let bindings = self.doc(page).and_then(|p| p.main_frame.page_dom.clone());
                // The script runs as soon as its graph is ready, with no task
                // in between (one could deliver an event, an <object>'s load,
                // before the script's listener for it exists).
                scope.then(
                    loaded,
                    move |s, _v| {
                        run_module_graph(s, bindings.as_ref(), &url);
                        fetched(ran)
                    },
                    move |s, e| {
                        // A graph that does not parse still runs: running it
                        // reports the error; one a fetch failed has no result.
                        if module_graph_has_script(s, e) {
                            s.report_exception(&JsError::thrown(Persistent::new(s, e)));
                            failed(ran);
                        } else {
                            failed(Some(false));
                        }
                    },
                )
            }
            Ok(None) => return self.execute_module_whole(page, text, url, deferred, element, ran, finish),
            Err(e) => Err(e),
        };
        if let Err(e) = started {
            scope.report_exception(&e);
            if let Some(element) = &element {
                self.script_ran(page, element, Some(false));
            }
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

    /// `execute_module` without the engine's two steps (an older patched
    /// JavaScriptCore, the system framework): the graph's fetch and the
    /// evaluation are one promise. The script counts as run once the
    /// promise settles or, still pending, when no module fetch of the realm
    /// is in flight after a task (the engine requests imports from promise
    /// jobs, so then every import was loaded and the module evaluated up to
    /// its top-level await; an unrelated `import()` in flight delays it).
    /// The element gets `ran` on fulfillment and `error` on rejection.
    fn execute_module_whole(
        &mut self,
        page: FrameKey,
        text: &str,
        url: &str,
        deferred: bool,
        element: Option<Rc<ResourceNode>>,
        ran: Option<bool>,
        finish: Rc<dyn Fn(Option<bool>)>,
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
                    self.script_ran(page, element, Some(false));
                }
                return false;
            }
        };
        let done = Rc::new(std::cell::Cell::new(false));
        let resume = {
            let done = done.clone();
            Rc::new(move |ok: bool| {
                if !done.replace(true) {
                    finish(if ok { ran } else { Some(false) });
                }
            })
        };
        let (ok, err) = (resume.clone(), resume.clone());
        let r = scope.then(
            promise,
            move |_s, _v| ok(true),
            move |s, e| {
                s.report_exception(&JsError::thrown(Persistent::new(s, e)));
                err(false)
            },
        );
        if let Err(e) = r {
            scope.report_exception(&e);
            return false;
        }
        if let Some(fetcher) = realm.module_fetcher() {
            graph_loaded(self.scheduler(), Rc::downgrade(&fetcher), done, Box::new(move || resume(true)));
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

    /// A script element has run (or failed): rendering is unblocked on it
    /// and it gets `event` (`Some(true)` load, `Some(false)` error).
    fn script_ran(&self, page: FrameKey, element: &ResourceNode, event: Option<bool>) {
        if let Some(bindings) = self.doc(page).and_then(|p| p.main_frame.page_dom.clone()) {
            bindings.unblock_rendering(element.node);
            if let Some(ok) = event {
                pyro_webapi::lifecycle::resource_loaded(&bindings, element.node, ok);
            }
        }
    }

    fn report_load_error(&mut self, page: FrameKey, error: &str) {
        let Some(realm) = self.main_realm(page) else { return };
        let Ok(scope) = realm.scope() else { return };
        pyro_webapi::lifecycle::report_diagnostic(
            &scope,
            &JsError::type_error(format!("Failed to load script: {error}")),
        );
    }

    /// A subresource failure the console reports (no `error` event at the Window).
    pub(crate) fn report_console_error(&mut self, page: FrameKey, error: &str) {
        let Some(realm) = self.main_realm(page) else { return };
        let Ok(scope) = realm.scope() else { return };
        pyro_webapi::lifecycle::report_diagnostic(&scope, &JsError::type_error(error.to_owned()));
    }
}

/// HTML "run a module script" for the graph `Browser::execute_module`
/// fetched for `url`: links and evaluates it (destructive writes ignored);
/// a link error and an evaluation error are reported upon the evaluation
/// promise's rejection, so after the microtasks the evaluation queued (it
/// runs inside a microtask, where they do not drain on return).
fn run_module_graph(scope: &pyro_js::Scope<'_>, bindings: Option<&Rc<pyro_webapi::PageDom>>, url: &str) {
    let evaluated = match bindings {
        Some(dom) => dom.with_destructive_writes_ignored(|| scope.link_and_evaluate_module(url)),
        None => scope.link_and_evaluate_module(url),
    };
    let report = |s: &pyro_js::Scope<'_>, e: pyro_js::Value<'_>| {
        s.note_module_evaluation_error(e);
        s.report_exception(&JsError::thrown(Persistent::new(s, e)))
    };
    let reported = match evaluated {
        Ok(promise) => scope.then(promise, |_s, _v| {}, report),
        Err(JsError::Thrown(error)) => {
            // A microtask later, as a rejection would be reported.
            scope.then(
                scope.undefined(),
                move |s, _v| {
                    s.note_module_evaluation_error(error.get(s));
                    s.report_exception(&JsError::Thrown(error))
                },
                |_s, _v| {},
            )
        }
        Err(e) => Err(e),
    };
    if let Err(e) = reported {
        scope.report_exception(&e);
    }
}

/// Whether a module graph whose load rejected with `error` still has a module
/// script, which runs and reports the error (HTML: the script's "error to
/// rethrow"), rather than none (a fetch failed: the element gets `error`):
/// a parse error; a module specifier that does not resolve; and the cached
/// evaluation error of a module an earlier script's evaluation reached
/// (`Scope::note_module_evaluation_error`), which the engine rejects with
/// again. A fetch failure is always a TypeError object, so any other thrown
/// value is an evaluation error too.
fn module_graph_has_script(scope: &pyro_js::Scope<'_>, error: pyro_js::Value<'_>) -> bool {
    let Some(object) = error.as_object() else { return true };
    if scope.is_module_evaluation_error(error) {
        return true;
    }
    let name = object.get_atom("name").ok().and_then(|n| n.as_string());
    match name.as_deref() {
        Some("SyntaxError") => true,
        Some("TypeError") => object
            .get_atom("message")
            .ok()
            .and_then(|m| m.as_string())
            .is_some_and(|m| m.starts_with("Failed to resolve module specifier")),
        _ => false,
    }
}

/// Calls `then` in a later task in which `fetcher` has no fetch in flight,
/// unless `done` is set by then (see `Browser::execute_module`).
fn graph_loaded(
    sched: pyro_runtime::Scheduler<Browser>,
    fetcher: std::rc::Weak<dyn ModuleFetcher>,
    done: Rc<std::cell::Cell<bool>>,
    then: Box<dyn FnOnce()>,
) {
    let again = sched.clone();
    sched.post(TaskSource::Networking, move |_b| {
        let Some(f) = fetcher.upgrade() else { return };
        if done.get() {
            return;
        }
        if !f.busy() {
            then();
            return;
        }
        let weak = fetcher.clone();
        f.when_idle(Box::new(move || graph_loaded(again, weak, done, then)));
    });
}

/// "Prepare the script element" for a script the parser did not insert
/// (`PageHooks::script_connected`, a post-connection step), before the DOM
/// call that connected it (or gave it its text or `src`) returns: an inline
/// classic script executes here. External scripts, modules and import maps
/// need the loader: they are prepared here too (already started, with what
/// prepare saw stashed in the DOM) and their `ResourceKind::Script` resource
/// event has `Browser::script_event` start them, so a script removed, moved
/// or stripped of its `src` by the inserting script still runs as prepared.
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
        if (info.src.is_none() && info.inline_text.is_empty()) || info.kind == ScriptKind::Data {
            return;
        }
        let _ = dom.mark_script_started(node);
        if info.src.is_some() || info.kind != ScriptKind::Classic || info.nomodule {
            let document = bindings.document();
            let _ = dom.stash_prepared_script(node, info, document);
            return;
        }
        info.inline_text
    };
    let Ok(scope) = realm.scope() else { return };
    let outer = bindings.current_script();
    bindings.set_executing_classic_script(node);
    pyro_webapi::trusted_types::sync_compile_strings(&scope, &bindings);
    realm.reenter_native_callback_hook(|| {
        if let Err(e) = scope.eval(&text, Some(&frame.base_url), 1) {
            scope.report_exception(&e);
        }
    });
    bindings.set_current_script(outer);
}

impl Browser {
    /// Registry identity of an inline module script whose base URL is `base`,
    /// the document base URL when it was prepared (`pyro_js`
    /// `Scope::inline_module_identity`): its `import.meta.url` and the
    /// referrer of its imports are `base`, fragment and all.
    fn inline_module_identity(&self, page: FrameKey, base: &str, tag: &str) -> String {
        let realm = self.main_realm(page);
        match realm.as_ref().and_then(|r| r.scope().ok()) {
            Some(scope) => scope.inline_module_identity(base, tag),
            None => format!("{}#inline-module-{tag}", base.split('#').next().unwrap_or(base)),
        }
    }

    /// [`Browser::inline_module_identity`] for an inline module the parser
    /// did not reach (script-inserted or written). Node indices are reused
    /// after DOM garbage collection, so the tag is a process-wide sequence
    /// under a prefix the parser's tags (`N`) never use.
    fn dynamic_inline_module_identity(&self, page: FrameKey, base: &str) -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed) + 1;
        self.inline_module_identity(page, base, &format!("dyn-{n}"))
    }
}

pub(crate) fn install_module_fetcher(sched: pyro_runtime::Scheduler<Browser>, page: FrameKey, realm: &Realm) {
    realm.set_module_fetcher(Some(Rc::new(PageModuleFetcher { page, sched, fetches: Rc::default(), worklet_credentials: None, worklet_modules: None })));
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
    /// Subresource Integrity refuses an opaque response (a no-cors request whose redirect chain
    /// went through another origin) though it ends at the document's own origin and its digest
    /// matches: opacity was read off the final URL.
    #[test]
    fn integrity_refuses_an_opaque_response_that_ends_at_the_documents_origin() {
        let body = b"body".to_vec();
        let integrity = pyro_webapi::sri::sha256_expression(&body);
        use super::{integrity_error, Resource};
        let resource = |opaque| Resource {
            url: "http://site.test/a.js".into(),
            status: 200,
            mime: "text/javascript".into(),
            charset: None,
            body: body.clone().into(),
            immutable_local: false,
            content_disposition: None,
            header_csp: None,
            refresh: None,
            content_language: None,
            referrer_policy: None,
            sent_referrer: None,
            timing: None,
            partial: None,
            nosniff: false,
            opaque,
        };
        assert!(integrity_error(&resource(false), Some(&integrity), false, "http://site.test").is_none());
        assert!(integrity_error(&resource(true), Some(&integrity), false, "http://site.test").is_some());
    }

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
