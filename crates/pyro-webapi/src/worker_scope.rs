//! `DedicatedWorkerGlobalScope`: the side of a `Worker` that runs on its thread.
//!
//! The thread owns one `Engine` (its own VM), an `EventLoop` and a `PageDom` over
//! an empty document at the worker's URL. The document is never exposed: it
//! gives the realm what the bindings look up through `page_of` (events,
//! `fetch`, `URL`, base URL and origin), so those run unchanged, each on this
//! thread with this thread's state. Only the classes in `register` exist here,
//! which is what makes `document` undefined in a worker. A dedicated worker
//! has `Worker`: its nested workers belong to its `PageDom` (`worker::dispose`
//! ends them with it), and it waits a bounded time for their threads before
//! its VM goes (`worker::join_nested`).
//!
//! API subset: `self`, `location`, `navigator`, `name`, `postMessage`, `close`,
//! `onmessage`/`onmessageerror`/`onerror`, `importScripts`, timers,
//! `queueMicrotask`, `fetch` and its classes, `WebSocket`, `XMLHttpRequest` (without
//! `responseXML`; relative URLs resolve against the script URL), `console`,
//! `TextEncoder`/`Decoder`, `URL`, `atob`/`btoa`, `structuredClone`, `crypto`
//! (with `CryptoKey`), `performance` (the worker's own timeline: User Timing,
//! its fetches' Resource Timing, `PerformanceObserver`), `FileReader`, `navigator.storage`
//! and, made on first use as on a window, the streams (`ReadableStream`,
//! `TransformStream`, `CompressionStream`, ...).
//!
//! Scripts are fetched here, blocking this thread only, through the page's
//! context `NetClient`: cookies, interception and Network events are the
//! context's. Requests carry the owner's frame id and no loader id, so they are
//! reported on the owner's page and do not count toward its network idle.
//!
//! An uncaught exception fires `error` at the global and, if not canceled,
//! `error` at the `Worker`; it does not continue to the owner's window.
use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::sync::atomic::Ordering;
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use pyro_dom::parse::DomRef;
use pyro_dom::Dom;
use pyro_js::{
    Args, ClassDef, ConsoleLevel, Engine, EngineHooks, IdentityKey, JsClass, JsError, JsException, JsResult,
    Persistent, Realm, RealmOptions, Scope, Value,
};
use pyro_net::http::HeaderMap;
use pyro_net::{Credentials, Destination, NetClient, Request, RequestMode, ResponseEvent, ResponseKind};
use pyro_runtime::{EventLoop, IdleWatch, LoopHandle, LoopState, RunOutcome, Scheduler, TaskSource, TimerId};

use crate::event::{EventCore, EventInterface};
use crate::identity::UserAgentOverride;
use crate::page::{NavigationRequest, PageDom, PageHooks, RemotePoster};
use crate::structured_clone::{encode_message, WireMessage};
use crate::target::{fire, EventTarget, EventTargetData};
use crate::util::arg_string;
use crate::worker::{deliver_up, fire_message, message_taken, Early, InspectTask, Shared, Up, MAX_QUEUED_BYTES};

/// Largest script `new Worker` or `importScripts` loads.
const MAX_SCRIPT_BYTES: usize = 64 * 1024 * 1024;
/// How often a blocked fetch checks that the worker should still run.
const CLOSING_POLL: Duration = Duration::from_millis(50);
/// Console calls and exceptions kept until a client enables `Runtime`: V8's
/// inspector replays those a client missed, and Playwright enables it only after
/// attaching, while the initial script may already have run.
const MAX_BUFFERED_EVENTS: usize = 256;
/// As on the browser thread: JSC itself bounds its recursion by the stack it finds.
const STACK_BYTES: usize = 8 * 1024 * 1024;
/// How long a worker that ends waits for the threads of its nested workers;
/// each level waits its own, so a chain of them can take this once per level.
const NESTED_JOIN: Duration = Duration::from_secs(1);

/// The embedder policies of a dedicated worker's owner, which its script's response must be
/// compatible with (HTML "run a worker": "check a global object's embedder policy"). `Send`.
pub(crate) struct OwnerEmbedder {
    policy: pyro_net::EmbedderPolicy,
    report_only: pyro_net::EmbedderPolicy,
    /// Where the owner's reports of a refused worker go (its `report-to` endpoints, by disposition).
    report: Option<pyro_net::CorpReport>,
}

impl OwnerEmbedder {
    /// `page`'s policies, when its worker's script has to be compatible with them: none for an owner without
    /// a (report-only) policy or one with a Document-Isolation-Policy (its workers inherit it).
    ///
    /// `shared`: the owner of a shared worker is enforced only when its policy is `require-corp`. The spec
    /// would refuse a `credentialless` owner a worker without a policy too, but Chrome does not
    /// (html/cross-origin-embedder-policy/credentialless/shared-worker.https.window.js expects a
    /// credentialless window's `unsafe-none` shared worker to run), and the page is the proxy for Chrome.
    pub(crate) fn of(page: &PageDom, shared: bool) -> Option<OwnerEmbedder> {
        let owner_reporting = page.embedder_reporting();
        if owner_reporting.dip || owner_reporting.report_only_dip {
            return None;
        }
        let mut owner = pyro_net::RequestDefaults::default();
        page.apply_embedder_defaults(&mut owner);
        let enforced = if shared && owner.embedder_policy != pyro_net::EmbedderPolicy::RequireCorp {
            pyro_net::EmbedderPolicy::UnsafeNone
        } else {
            owner.embedder_policy
        };
        owner.corp_report.map(|report| OwnerEmbedder { policy: enforced, report_only: report.report_only, report: Some(report) })
    }
}

/// A document's connection to a shared worker, checked on the worker's thread, where its embedder policy is
/// known once its script has loaded (HTML "check a global object's embedder policy" for every connecting
/// document, not only the one that started it). `Send`.
pub(crate) struct ConnectCheck {
    /// The connecting document's policies (`None`: nothing to check).
    pub(crate) owner: Option<OwnerEmbedder>,
    /// Identifies the connecting `SharedWorker` object to the document side if it is refused.
    pub(crate) client: u64,
    /// The worker's script URL, the blocked URL of a report.
    pub(crate) url: String,
}

/// What a worker's thread needs from the page side. `Send`.
pub(crate) struct WorkerStart {
    id: u64,
    url: pyro_net::Url,
    name: String,
    /// The bytes of a `blob:` URL, captured at construction like a download's:
    /// the store is the owner document's and the page may revoke the URL at once.
    script: Option<Bytes>,
    net: NetClient,
    poster: RemotePoster,
    shared: Arc<Shared>,
    frame_id: Option<String>,
    origin: Option<String>,
    creator_cluster: Option<crate::page::AgentCluster>,
    creator_secure: bool,
    /// The creator's Mixed Content / Upgrade Insecure Requests state: the worker's requests keep it.
    insecure: pyro_net::InsecurePolicy,
    /// The creator's cross-origin embedder policy (HTML § 7.1.4): a worker has its script response's, which an
    /// owner whose policy is not `unsafe-none` requires to be the same or stronger, and a `blob:` or
    /// `data:` worker inherits the owner's. The worker's requests are subject to it.
    pub(crate) embedder_policy: pyro_net::EmbedderPolicy,
    /// The owner's policies when a dedicated worker's script must be compatible with them: none
    /// for a service worker, a shared worker, or an owner without a (report-only) policy.
    pub(crate) owner_embedder: Option<OwnerEmbedder>,
    /// The creator's reporting parts of it: a `blob:` or `data:` worker inherits them with the policy.
    pub(crate) embedder_reporting: pyro_net::EmbedderReporting,
    cookie_partition_key: Option<String>,
    cookie_site_context: pyro_net::cookies::SiteContext,
    /// The creator's credentialless partition (`crate::credentialless`): the worker's cookies and
    /// storage keys are the partition's.
    anonymous: Option<crate::credentialless::Anonymous>,
    user_agent: Option<UserAgentOverride>,
    extra_headers: HeaderMap,
    cache_disabled: bool,
    /// The owner's context partition: the worker's IndexedDB shares its data.
    indexed_db: crate::indexeddb::Namespace,
    /// Its Cache Storage, likewise.
    caches: crate::cache_storage::Namespace,
    opfs: crate::opfs::Namespace,
    buckets: crate::storage_buckets::catalog::Catalog,
    permissions: crate::navigator_extras::PermissionTable,
    web_locks: crate::web_locks::LockService,
    pressure: crate::pressure::PressureHub,
    /// `new Worker(url, {type: 'module'})`: the script is a module.
    pub(crate) module: bool,
    /// `WorkerOptions.credentials`: a module worker's main script and every
    /// module of its realm are fetched with it (HTML "fetch a module worker
    /// script graph"). A classic worker's fetches stay same-origin.
    pub(crate) credentials: Credentials,
    /// A dedicated worker's main script request: its creator's referrer
    /// source and policy (HTML "fetch a classic/module worker script" uses
    /// the outside settings). None: no `Referer` is sent.
    pub(crate) referrer: Option<(pyro_net::Url, pyro_net::ReferrerPolicy)>,
    /// A worker started by a worker: its place under
    /// `worker::MAX_NESTED_WORKERS`, held by the worker's thread until it
    /// exits (`run`), or given back with this if the thread never starts.
    pub(crate) nested: Option<crate::worker::NestedSlot>,
    pub(crate) early: Vec<Early>,
    /// A service worker version: its global is a `ServiceWorkerGlobalScope`.
    pub(crate) service: Option<crate::service_worker_scope::ServiceStart>,
    /// A shared worker: its global is a `SharedWorkerGlobalScope`, and
    /// documents connect to it (`WorkerLoop::connect`).
    pub(crate) shared_worker: bool,
    /// The owner document's CSP: its script's redirects and its module graph's static imports
    /// are checked against it ([`crate::worker::csp_check`]).
    pub(crate) csp: crate::worker::WorkerCsp,
    /// A dedicated or shared worker that is a service worker client: the link its requests take to
    /// the worker that controls it (a nested worker shares its creator's).
    pub(crate) control: Option<Arc<crate::service_worker::ControlLink>>,
}

impl WorkerStart {
    /// `None` when the embedder gives no network or no way back to this thread.
    pub(crate) fn new(
        id: u64,
        page: &Rc<PageDom>,
        s: &Scope<'_>,
        url: pyro_net::Url,
        name: String,
        shared: Arc<Shared>,
    ) -> Option<WorkerStart> {
        let net = page.net.borrow().clone()?;
        let poster = page.remote_poster()?;
        let script = (url.scheme() == "blob").then(|| page.blob_url(url.as_str()).map(|(bytes, _)| bytes)).flatten();
        // (A Document-Isolation-Policy's workers inherit it: nothing to check.)
        let owner_embedder = OwnerEmbedder::of(page, false);
        Some(WorkerStart {
            id,
            url,
            name,
            script,
            net,
            poster,
            shared,
            frame_id: page.request_context().frame_id,
            origin: s.realm().origin(),
            creator_cluster: Some(page.agent_cluster()),
            cookie_partition_key: page.cookie_partition_key(),
            cookie_site_context: page.cookie_site_context(),
            anonymous: page.anonymous(),
            insecure: page.insecure_policy(),
            embedder_policy: page.embedder_policy(),
            owner_embedder,
            embedder_reporting: page.embedder_reporting(),
            creator_secure: s.global_data::<WorkerGlobalScope>()
                .map_or_else(|| crate::secure_context::secure(s, page), |parent| parent.secure),
            user_agent: page.identity.borrow().value.clone(),
            extra_headers: page.extra_http_headers(),
            cache_disabled: page.cache_disabled(),
            indexed_db: page.local_storage.borrow().indexed_db(),
            caches: page.local_storage.borrow().caches(),
            opfs: page.local_storage.borrow().opfs(),
            buckets: page.local_storage.borrow().buckets(),
            permissions: page.context_services().permissions.clone(),
            web_locks: page.context_services().web_locks.clone(),
            pressure: page.context_services().pressure().clone(),
            module: false,
            credentials: Credentials::SameOrigin,
            referrer: None,
            nested: None,
            early: Vec::new(),
            service: None,
            shared_worker: false,
            csp: Default::default(),
            control: None,
        })
    }

    /// A service worker version whose script `page` fetched. Its own network
    /// requests belong to the worker, not to the registering document's frame.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn for_service(
        id: u64,
        page: &Rc<PageDom>,
        url: pyro_net::Url,
        origin: String,
        shared: Arc<Shared>,
        script: Bytes,
        service: crate::service_worker_scope::ServiceStart,
        module: bool,
    ) -> Option<WorkerStart> {
        let net = page.net.borrow().clone()?;
        // The version outlives `page`'s document: its messages must not be
        // dropped when that page navigates or closes.
        let poster = page.thread_poster()?;
        Some(WorkerStart {
            id,
            url,
            name: String::new(),
            script: Some(script),
            net,
            poster,
            shared,
            frame_id: None,
            origin: Some(origin),
            creator_cluster: None,
            cookie_partition_key: service.partition_key.clone(),
            cookie_site_context: service.cookie_site_context,
            anonymous: None,
            creator_secure: true,
            insecure: page.insecure_policy(),
            embedder_policy: pyro_net::EmbedderPolicy::UnsafeNone,
            owner_embedder: None,
            embedder_reporting: Default::default(),
            user_agent: page.identity.borrow().value.clone(),
            extra_headers: page.extra_http_headers(),
            cache_disabled: page.cache_disabled(),
            indexed_db: page.local_storage.borrow().indexed_db(),
            caches: page.local_storage.borrow().caches(),
            opfs: page.local_storage.borrow().opfs(),
            buckets: page.local_storage.borrow().buckets(),
            permissions: page.context_services().permissions.clone(),
            web_locks: page.context_services().web_locks.clone(),
            pressure: page.context_services().pressure().clone(),
            module,
            credentials: Credentials::SameOrigin,
            referrer: None,
            nested: None,
            early: Vec::new(),
            service: Some(service),
            shared_worker: false,
            csp: Default::default(),
            control: None,
        })
    }
}

/// Starts the thread; returns once its loop can take tasks, with what was
/// queued for it. The VM and everything else are created on the thread, after that.
pub(crate) fn spawn(
    mut start: WorkerStart,
) -> std::io::Result<(LoopHandle<WorkerLoop>, std::thread::JoinHandle<()>, Vec<Early>)> {
    let early = std::mem::take(&mut start.early);
    let (ready, handle) = mpsc::sync_channel(1);
    let thread = std::thread::Builder::new()
        .name(format!("pyro-worker-{}", start.id))
        .stack_size(STACK_BYTES)
        .spawn(move || run(start, ready))?;
    match handle.recv() {
        Ok(handle) => Ok((handle, thread, early)),
        Err(_) => Err(std::io::Error::other("worker thread ended at startup")),
    }
}

fn epoch_ms() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64() * 1000.0)
}

/// A console call or exception no client has seen yet.
pub enum BufferedEvent {
    Console { level: ConsoleLevel, args: Vec<Persistent>, timestamp: f64, stack: Option<String> },
    Exception { exception: JsException, timestamp: f64 },
}

/// The worker engine's observer: buffers what happens until an inspector
/// attaches, then forwards to it. Uncaught exceptions also become `error` events.
pub struct WorkerEvents {
    buffer: RefCell<VecDeque<BufferedEvent>>,
    inspector: RefCell<Option<Rc<dyn EngineHooks>>>,
    state: RefCell<Option<Rc<dyn Any>>>,
    sched: Scheduler<WorkerLoop>,
}

impl WorkerEvents {
    /// Makes `hooks` the receiver of later events and `state` what
    /// [`WorkerEvents::inspector_state`] returns (dropped before the VM);
    /// returns the events so far, oldest first.
    pub fn attach_inspector(&self, hooks: Rc<dyn EngineHooks>, state: Rc<dyn Any>) -> Vec<BufferedEvent> {
        *self.inspector.borrow_mut() = Some(hooks);
        *self.state.borrow_mut() = Some(state);
        self.buffer.take().into()
    }

    pub fn inspector_state(&self) -> Option<Rc<dyn Any>> {
        self.state.borrow().clone()
    }

    fn push(&self, event: BufferedEvent) {
        let mut buffer = self.buffer.borrow_mut();
        if buffer.len() == MAX_BUFFERED_EVENTS {
            buffer.pop_front();
        }
        buffer.push_back(event);
    }

    fn inspector(&self) -> Option<Rc<dyn EngineHooks>> {
        self.inspector.borrow().clone()
    }

    fn release(&self) {
        drop(self.inspector.take());
        drop(self.state.take());
        drop(self.buffer.take());
    }
}

impl EngineHooks for WorkerEvents {
    fn console_api(&self, realm: &Realm, level: ConsoleLevel, args: &[Value<'_>]) {
        if let Some(inspector) = self.inspector() {
            return inspector.console_api(realm, level, args);
        }
        let Ok(scope) = realm.scope() else { return };
        let args = args.iter().map(|v| Persistent::new(&scope, *v)).collect();
        self.push(BufferedEvent::Console { level, args, timestamp: epoch_ms(), stack: scope.current_stack().ok() });
    }

    fn exception_thrown(&self, realm: &Realm, exception: &JsException) {
        match self.inspector() {
            Some(inspector) => inspector.exception_thrown(realm, exception),
            None => self.push(BufferedEvent::Exception { exception: exception.clone(), timestamp: epoch_ms() }),
        }
        let exception = exception.clone();
        self.sched.post(TaskSource::DomManipulation, move |state| state.dispatch_error(exception));
    }

    fn unhandled_rejection(&self, realm: &Realm, exception: &JsException) {
        match self.inspector() {
            Some(inspector) => inspector.unhandled_rejection(realm, exception),
            None => self.push(BufferedEvent::Exception { exception: exception.clone(), timestamp: epoch_ms() }),
        }
    }
}

/// What an inspector task can reach on the worker's thread.
pub struct WorkerContext<'a> {
    pub engine: &'a Engine,
    pub realm: &'a Realm,
    pub events: &'a Rc<WorkerEvents>,
}

/// The worker thread's loop state.
pub struct WorkerLoop {
    engine: Engine,
    page: Rc<PageDom>,
    realm: Realm,
    events: Rc<WorkerEvents>,
    shared: Arc<Shared>,
    id: u64,
    poster: RemotePoster,
    /// An `error` listener is running: an exception it throws goes to the page only.
    in_error: bool,
    /// A module worker whose script has not finished evaluating (its graph
    /// is still loading, or it awaits at its top level): the messages and
    /// connections that arrive wait in `held`, as the worker's port message
    /// queue is enabled once its script ran (HTML "run a worker").
    starting: Rc<Cell<bool>>,
    /// Which of the realm's module requests are its top-level graph's static imports.
    graph: Rc<GraphPhase>,
    held: Vec<Held>,
}

/// Which module requests of a worker's realm belong to its top-level module graph, fetched
/// with the owner as the fetch client (HTML "fetch a module worker script graph": the owner's
/// CSP decides, for the worker's destination), and which are `import()` of the running script
/// (the worker's own fetches, under its own CSP).
#[derive(Default)]
pub(crate) struct GraphPhase {
    /// The engine's first step (`load_module_graph`) runs: the graph is being fetched and none
    /// of the worker's script has run, so every request is a static import.
    loading: Cell<bool>,
    /// The engine fetches and evaluates the graph in one step (`eval_module`): until it settles
    /// a request may be a static import or an `import()` of the evaluating top level, and the
    /// engine does not say which, so both policies are checked.
    mixed: Cell<bool>,
}

/// What a worker's module request is, as far as [`GraphPhase`] can tell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GraphRequest {
    Static,
    Mixed,
    Dynamic,
}

impl GraphPhase {
    fn request(&self) -> GraphRequest {
        if self.loading.get() {
            GraphRequest::Static
        } else if self.mixed.get() {
            GraphRequest::Mixed
        } else {
            GraphRequest::Dynamic
        }
    }
}

/// What a worker that is still starting was sent.
enum Held {
    Message(WireMessage),
    Connect(WireMessage, ConnectCheck),
}

impl LoopState for WorkerLoop {
    fn after_task(&mut self) {
        self.engine.drain_finalized();
        self.page.after_task();
        // The script's evaluation settled in this task (or in its microtasks,
        // just run): what was sent meanwhile is delivered, in order, each as
        // the task it would have been.
        if !self.held.is_empty() && !self.starting.get() {
            for held in std::mem::take(&mut self.held) {
                match held {
                    Held::Message(message) => self.receive(message),
                    Held::Connect(message, check) => self.connect(message, check),
                }
                self.engine.drain_finalized();
                self.page.after_task();
            }
        }
    }
}

impl WorkerLoop {
    /// A message from the page, as its own task.
    pub(crate) fn receive(&mut self, message: WireMessage) {
        if self.starting.get() {
            return self.held.push(Held::Message(message));
        }
        message_taken(&self.shared, message.size());
        if self.shared.closing.load(Ordering::Acquire) {
            return;
        }
        let Ok(s) = self.realm.scope() else { return };
        let Some(target) = self.page.window_target() else { return };
        fire_message(&s, &self.page, target, &message);
    }

    /// A document connected to this shared worker: `connect` at the global,
    /// with the port `message` transfers as `ports[0]` and `source`.
    pub(crate) fn connect(&mut self, message: WireMessage, check: ConnectCheck) {
        if self.starting.get() {
            return self.held.push(Held::Connect(message, check));
        }
        if self.shared.closing.load(Ordering::Acquire) {
            return;
        }
        // HTML "check a global object's embedder policy": a connecting document whose policy is not
        // `unsafe-none` needs the worker's to be one too (the worker's is its script's, whichever document
        // started it). A refused connection gets no `connect`, and its SharedWorker object an `error`.
        if let Some(owner) = &check.owner {
            let url = pyro_net::Url::parse(&check.url).ok();
            if url.is_some_and(|url| !owner_accepts_worker(owner, &url, self.page.embedder_policy())) {
                let (id, client) = (self.id, check.client);
                (self.poster)(Box::new(move || deliver_up(id, Up::ConnectRefused(client))));
                return;
            }
        }
        let Ok(s) = self.realm.scope() else { return };
        let Some(target) = self.page.window_target() else { return };
        let Ok((_, ports)) = crate::structured_clone::decode_with_ports(&s, &message) else { return };
        let Some(ports) = ports else { return };
        let port = ports.get_index(0).ok().map(|p| Persistent::new(&s, p));
        let ev = crate::event::EventCore::trusted(&self.page, crate::event::EventInterface::MessageEvent, "connect", false, false);
        {
            let mut fields = ev.fields.borrow_mut();
            fields.message_data = Some(Persistent::new(&s, s.string("")));
            fields.message_ports = Some(Persistent::new(&s, ports.value()));
            if let Some(port) = port {
                fields.message_source_window = Some(Rc::new(move |s| Ok(s.rehome(port.get(s)))));
            }
        }
        crate::target::fire(&self.page, target, &ev);
    }

    /// Runs `f` with the realm (a service worker's browser-thread request).
    pub(crate) fn service(&mut self, f: impl FnOnce(&Scope<'_>, &Rc<PageDom>)) {
        if self.shared.closing.load(Ordering::Acquire) {
            return;
        }
        let Ok(s) = self.realm.scope() else { return };
        f(&s, &self.page);
    }

    pub(crate) fn inspect(&mut self, task: InspectTask) {
        task(&WorkerContext { engine: &self.engine, realm: &self.realm, events: &self.events });
    }

    fn dispatch_error(&mut self, exception: JsException) {
        if self.shared.closing.load(Ordering::Acquire) {
            return;
        }
        let message = format!("Uncaught {}", exception.text());
        let filename = exception.url.clone().unwrap_or_default();
        let (line, column) = (exception.line.unwrap_or(0), exception.column.unwrap_or(0));
        let mut report = true;
        // The exception reporter fired the global's event already (uncanceled).
        let fired = self.page.reported_errors.get();
        if fired > 0 {
            self.page.reported_errors.set(fired - 1);
        } else if !self.in_error {
            if let Some(target) = self.page.window_target() {
                let ev = EventCore::trusted(&self.page, EventInterface::ErrorEvent, "error", false, true);
                {
                    let mut fields = ev.fields.borrow_mut();
                    fields.message = message.clone();
                    fields.filename = filename.clone();
                    fields.lineno = line;
                    fields.colno = column;
                    fields.error = exception.value.clone();
                }
                self.in_error = true;
                report = fire(&self.page, target, &ev);
                // An exception from a listener is reported by a task after this one.
                self.events.sched.post(TaskSource::DomManipulation, |state| state.in_error = false);
            }
        }
        if report {
            let id = self.id;
            (self.poster)(Box::new(move || deliver_up(id, Up::Error { message, filename, line, column })));
        }
    }
}

struct ScopeHooks {
    sched: Scheduler<WorkerLoop>,
    handle: LoopHandle<WorkerLoop>,
}

impl PageHooks for ScopeHooks {
    fn post_task(&self, task: Box<dyn FnOnce()>) {
        self.sched.post(TaskSource::DomManipulation, move |_| task());
    }

    fn post_web_task(
        &self,
        priority: pyro_runtime::WebPrioritySource,
        delay: std::time::Duration,
        task: Box<dyn FnOnce()>,
    ) -> Option<Box<dyn FnOnce()>> {
        Some(self.sched.post_web_task(priority, delay, Box::new(move |_| task())))
    }

    fn post_web_continuation(
        &self,
        priority: pyro_runtime::WebPrioritySource,
        task: Box<dyn FnOnce()>,
    ) -> Option<Box<dyn FnOnce()>> {
        Some(self.sched.post_web_continuation(priority, Box::new(move |_| task())))
    }

    fn post_delayed_task(&self, delay: Duration, task: Box<dyn FnOnce()>) {
        let task = RefCell::new(Some(task));
        self.sched.set_timer(delay, false, true, move |_| {
            if let Some(task) = task.borrow_mut().take() {
                task();
            }
        });
    }

    fn set_media_timer(&self, delay: Duration, task: Box<dyn FnOnce()>) -> Option<u64> {
        let task = RefCell::new(Some(task));
        // A host timer for internal polling (Compute Pressure): it does not keep the worker's loop busy.
        let id = self.sched.set_timer(delay, false, false, move |_| {
            if let Some(task) = task.borrow_mut().take() {
                task();
            }
        });
        Some(u64::from(id.0))
    }

    fn clear_media_timer(&self, id: u64) {
        if let Ok(id) = u32::try_from(id) {
            self.sched.clear_timer(TimerId(id));
        }
    }

    fn navigate(&self, _request: NavigationRequest) {}

    fn remote_poster(&self) -> Option<RemotePoster> {
        let handle = self.handle.clone();
        Some(Arc::new(move |task| {
            let _ = handle.post(move |_| task());
        }))
    }
}

fn run(mut start: WorkerStart, ready: mpsc::SyncSender<LoopHandle<WorkerLoop>>) {
    // A nested worker's place under the cap lasts as long as this thread,
    // whatever ends it: given back on the way out, after the teardown.
    let _nested = start.nested.take();
    let event_loop = EventLoop::<WorkerLoop>::new();
    #[cfg(target_os = "macos")]
    let event_loop = {
        // As BrowserThread: the system JavaScriptCore's timers need this run loop.
        let mut event_loop = event_loop;
        if !pyro_js::sys::SOURCE_BUILD {
            event_loop.enable_native_runloop_service(Duration::from_millis(250));
        }
        event_loop
    };
    let mut event_loop = event_loop;
    if ready.send(event_loop.handle()).is_err() {
        return;
    }
    let (id, shared, poster, module) = (start.id, start.shared.clone(), start.poster.clone(), start.module);
    // A module shared worker's graph is fetched and linked before it runs:
    // a failure there is a failure to load, not an exception of the worker.
    let phased = module && start.shared_worker;
    let service = start.service.as_ref().map(|s| s.version);
    // A service worker's version learns whether its script ran.
    let evaluated = |ok: bool| {
        if let Some(version) = service {
            poster(Box::new(move || {
                crate::service_worker::deliver(version, crate::service_worker::SwUp::Evaluated(ok))
            }));
        }
    };
    match set_up(&event_loop, start) {
        Ok((mut state, script)) => {
            if let Some(script) = script {
                let ok = !shared.closing.load(Ordering::Acquire)
                    && if phased { start_module_graph(&state, &script) } else { run_script(&state, &script, module) };
                evaluated(ok);
                run_collecting_when_quiet(&mut event_loop, &mut state, &shared);
            } else {
                evaluated(false);
            }
            tear_down(event_loop, state);
            // The VM is gone. When a closed browser context ended the
            // worker, its pages go back now and the thread that started it
            // learns that what the worker kept alive there can be collected.
            // Not for a worker its live page ended: the release is 2.8M
            // instructions per worker, and the collection walks that page's
            // whole heap (measured with 20 workers started and terminated
            // by a page holding 380 MB: 0.4 G instructions without either,
            // 35.9 G with both).
            if shared.report_end.load(Ordering::Acquire) {
                pyro_js::release_thread_memory();
                poster(Box::new(crate::worker::note_ended));
            }
        }
        Err(_) if service.is_some() => evaluated(false),
        Err(_) => poster(Box::new(move || deliver_up(id, Up::LoadFailed))),
    }
}

/// Runs the worker's loop until it is closing. A worker that has gone quiet
/// (awake for nothing, next to no CPU for a second) has its VM collected if
/// the process grew since it last was (`Engine::settle_if_grown`): what its
/// script left behind, a service worker's precache run for one, otherwise
/// stays for as long as the worker waits for its next event. A worker that
/// sleeps is not woken for this: the watch starts with its next task.
fn run_collecting_when_quiet(event_loop: &mut EventLoop<WorkerLoop>, state: &mut WorkerLoop, shared: &Shared) {
    let mut idle = IdleWatch::new();
    loop {
        let watching = idle.deadline();
        // Not watching: come back after the first turn of the loop that
        // follows a wait, with its task still queued or just run.
        let mut turns = 0u32;
        let outcome = event_loop.run_until(state, watching, false, |_| {
            turns += 1;
            shared.closing.load(Ordering::Acquire) || (watching.is_none() && turns > 1)
        });
        if shared.closing.load(Ordering::Acquire) {
            return;
        }
        match outcome {
            RunOutcome::Done => idle.note_work(Instant::now()),
            RunOutcome::TimedOut => {
                if idle.quiet(Instant::now()) {
                    state.engine.settle_if_grown();
                }
            }
            RunOutcome::Idle | RunOutcome::Stopped => return,
        }
    }
}

/// The realm and, when it loaded, the script. A failed load was reported.
fn set_up(event_loop: &EventLoop<WorkerLoop>, start: WorkerStart) -> JsResult<(WorkerLoop, Option<(String, String)>)> {
    let engine = Engine::new()?;
    // The VM's deferred work (WebAssembly.compile/instantiate settling, finished on a compilation
    // thread) runs in a task of this loop, as on the browser thread (JSC patch 0040). One task per
    // wake while none is queued.
    let handle = event_loop.handle();
    let queued = Arc::new(std::sync::atomic::AtomicBool::new(false));
    engine.set_deferred_work_waker(move || {
        if queued.swap(true, Ordering::AcqRel) {
            return;
        }
        let queued = queued.clone();
        let _ = handle.post(move |state: &mut WorkerLoop| {
            queued.store(false, Ordering::Release);
            state.engine.run_deferred_work();
        });
    });
    *start.shared.terminate.lock().unwrap_or_else(|e| e.into_inner()) = Some(engine.terminate_handle()?);
    // `terminate()` may have run before the handle was there to act on it.
    if start.shared.closing.load(Ordering::Acquire) {
        if let Some(terminate) = start.shared.terminate.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            terminate.terminate();
        }
    }
    let class = register(&engine, start.service.is_some(), start.shared_worker);
    let events = Rc::new(WorkerEvents {
        buffer: RefCell::new(VecDeque::new()),
        inspector: RefCell::new(None),
        state: RefCell::new(None),
        sched: event_loop.scheduler(),
    });
    engine.set_hooks(Some(events.clone()));
    let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
    let document = dom.borrow_mut().create_document(pyro_dom::DocumentKind::Html, start.url.as_str());
    dom.borrow_mut().set_document_active(document, true).map_err(|e| JsError::Type(format!("{e:?}").into()))?;
    // CSP: a `data:` or `blob:` script's worker has its owner's policies, each with its
    // delivery (header or `<meta>`) and disposition; a fetched script's are its response's,
    // recorded by `fetch_script`.
    if matches!(start.url.scheme(), "data" | "blob") && !start.csp.inherited.is_empty() {
        let _ = dom.borrow_mut().inherit_csp_policies(document, &start.csp.inherited);
    }
    let page = PageDom::new(dom, document);
    // The same for the owner's `Connection-Allowlist`; a fetched script's own is its response's
    // (`fetch_script`; a service worker's, which its registration fetched, comes in `inherited_allowlist`).
    if matches!(start.url.scheme(), "data" | "blob") || start.service.is_some() {
        page.set_connection_allowlist(start.csp.inherited_allowlist.clone());
    }
    // Dedicated workers, including data/blob workers and nested workers, use
    // their creator's cluster. Shared and service workers own a distinct cluster.
    if !start.shared_worker {
        if let Some(cluster) = &start.creator_cluster {
            page.inherit_agent_cluster(cluster.clone());
        }
    }
    let cookie_site = if start.url.scheme() == "data" { pyro_net::cookies::SiteContext::CrossSite } else { start.cookie_site_context };
    page.set_cookie_context(start.cookie_partition_key.clone(), cookie_site);
    page.set_anonymous(start.anonymous.clone());
    page.set_context_services(Rc::new(crate::navigator_extras::ContextServices::for_worker(
        start.permissions.clone(), start.web_locks.clone(), start.pressure.clone(),
    )));
    // A controlled worker's requests go through its controller (`service_worker::route`).
    crate::service_worker::set_worker_control(&page, start.control.clone());
    // The worker's time origin: `performance.now()`, `Event.timeStamp` and
    // its Resource Timing entries count from here.
    let started = Instant::now();
    let origin_wall_ms = epoch_ms();
    page.set_event_clock(Rc::new(move |at| crate::performance::time_since(started, at)));
    page.set_user_agent_override(start.user_agent.clone()).map_err(|e| JsError::Type(e.to_string().into()))?;
    page.set_extra_http_headers(start.extra_headers.clone());
    page.set_no_accept_ch(start.service.is_some());
    page.set_cache_disabled(start.cache_disabled);
    page.set_creator_insecure(start.insecure);
    page.set_embedder_policy(start.embedder_policy);
    page.set_embedder_reporting(start.embedder_reporting.clone());
    page.set_storage_namespaces(
        crate::storage::StorageNamespace::with_shared(
            start.indexed_db.clone(), start.caches.clone(), start.opfs.clone(), start.buckets.clone(),
        ),
        Default::default(),
    );
    page.set_request_context(crate::RequestContext { frame_id: start.frame_id.clone(), loader_id: None });
    let window = page.create_window_target(document);
    *page.net.borrow_mut() = Some(start.net.clone());
    page.set_hooks(Some(Rc::new(ScopeHooks { sched: event_loop.scheduler(), handle: event_loop.handle() })));
    // A synchronous request parks this thread: `terminate()` must still end it.
    let closing = start.shared.clone();
    crate::xhr::set_sync_watch(&page, Box::new(move || closing.closing.load(Ordering::Acquire)));
    let (script, response_url) = load_main_script(&page, &start);
    // HTML "run a worker" step: the worker global's URL (`location`, the base URL of its
    // `importScripts`, XHR and nested workers, the name of its script) is the main script
    // response's URL, so after a redirect the final one, not the one `new Worker` asked for.
    let url = response_url.clone().unwrap_or_else(|| start.url.clone());
    if url.as_str() != start.url.as_str() {
        let _ = page.dom.borrow_mut().set_document_url(document, url.as_str());
    }
    let worker_origin = if start.url.scheme() == "data" {
        Some(start.url.origin().ascii_serialization())
    } else {
        start.origin.clone()
    };
    let origin = worker_origin.clone().or_else(|| Some(start.url.origin().ascii_serialization()));
    let realm = engine.create_realm(RealmOptions { name: String::new(), global_class: Some(class), origin })?;
    let scope = ScopeData {
        target: EventTargetData::window(&page, window),
        id: start.id,
        shared: start.shared.clone(),
        poster: start.poster.clone(),
        url: url.clone(),
        secure: pyro_net::is_potentially_trustworthy_url(&url)
            && (start.creator_secure || start.shared_worker || start.service.is_some()),
        name: start.name.clone(),
        user_agent: crate::window::user_agent(&page),
        sched: event_loop.scheduler(),
        timers: RefCell::new(HashMap::new()),
        next_timer: Cell::new(1),
        module: start.module,
        started,
        origin_wall_ms,
        performance: Cell::new(None),
        // The URL its nested workers' requests are sent with as their referrer.
        response_url: RefCell::new(response_url),
    };
    if start.service.is_some() {
        realm.set_global_data::<ServiceWorkerGlobalScope>(scope)?;
    } else if start.shared_worker {
        realm.set_global_data::<SharedWorkerGlobalScope>(scope)?;
    } else {
        realm.set_global_data::<DedicatedWorkerGlobalScope>(scope)?;
    }
    // True while a module worker's top-level graph loads and evaluates.
    let starting = Rc::new(Cell::new(false));
    let graph = Rc::new(GraphPhase::default());
    // Static imports of a module worker and `import()` in any worker: the
    // worker's own fetches, like `importScripts`.
    realm.set_module_fetcher(Some(Rc::new(WorkerModuleFetcher {
        page: Rc::downgrade(&page),
        net: start.net.clone(),
        shared: start.shared.clone(),
        credentials: script_credentials(&start),
        origin: worker_origin,
        outside_origin: start.origin.clone(),
        url: url.to_string(),
        sched: event_loop.scheduler(),
        in_flight: Rc::default(),
        service_graph: start.service.is_some().then(|| starting.clone()),
        csp: start.csp.clone(),
        graph: graph.clone(),
        id: start.id,
        poster: start.poster.clone(),
        owner_policy: start.referrer.as_ref().map(|(_, policy)| *policy),
    })));
    {
        let s = realm.scope()?;
        let global = if start.service.is_some() {
            "ServiceWorkerGlobalScope"
        } else if start.shared_worker {
            "SharedWorkerGlobalScope"
        } else {
            "DedicatedWorkerGlobalScope"
        };
        install(&s, &page, global)?;
        if let Some(service) = &start.service {
            crate::service_worker_scope::install(&s, service, start.poster.clone(), start.shared.clone())?;
            crate::cookie_store::bind_environment(&s, &page)?;
            crate::cookie_store::exposure::install(&s, true)?;
        }
    }
    let state = WorkerLoop {
        engine,
        page,
        realm,
        events,
        shared: start.shared.clone(),
        id: start.id,
        poster: start.poster.clone(),
        in_error: false,
        starting,
        graph,
        held: Vec::new(),
    };
    if script.is_none() && start.service.is_none() && !start.shared.closing.load(Ordering::Acquire) {
        let id = start.id;
        (start.poster)(Box::new(move || deliver_up(id, Up::LoadFailed)));
    }
    Ok((state, script.map(|text| (text, url.to_string()))))
}

/// The main script request's referrer, in; its response's URL (after
/// redirects), out.
struct MainRequest {
    referrer: Option<(pyro_net::Url, pyro_net::ReferrerPolicy)>,
    response_url: RefCell<Option<pyro_net::Url>>,
    /// The worker keeps its owner's embedder policy whatever its script's response says (a worker of
    /// a document with a Document-Isolation-Policy shares the document's isolation).
    inherit_embedder: bool,
    /// The worker is not a secure context: HTML "obtain an embedder policy" gives its script response the
    /// default policy, whatever the response says.
    insecure: bool,
}

/// The script and, when it was fetched, its response's URL.
fn load_main_script(page: &Rc<PageDom>, start: &WorkerStart) -> (Option<String>, Option<pyro_net::Url>) {
    let main = MainRequest {
        referrer: start.referrer.clone(),
        response_url: RefCell::new(None),
        inherit_embedder: start.embedder_reporting.dip || start.embedder_reporting.report_only_dip,
        insecure: !(pyro_net::is_potentially_trustworthy_url(&start.url) && (start.creator_secure || start.shared_worker || start.service.is_some())),
    };
    let script = load_main_bytes(page, start, &main).map(|bytes| script_text(&bytes));
    (script, main.response_url.take())
}

fn load_main_bytes(page: &Rc<PageDom>, start: &WorkerStart, main: &MainRequest) -> Option<Bytes> {
    let bytes = match (&start.script, start.url.scheme()) {
        (Some(bytes), _) => bytes.clone(),
        // A revoked or unknown object URL.
        (None, "blob") => return None,
        _ => {
            let fetched = fetch_script(
                page,
                &start.net,
                &start.shared,
                &start.origin,
                start.url.clone(),
                // Sec-Fetch-Dest: the global's kind.
                if start.service.is_some() {
                    Destination::ServiceWorker
                } else if start.shared_worker {
                    Destination::SharedWorker
                } else {
                    Destination::Worker
                },
                RequestMode::SameOrigin,
                script_credentials(start),
                true,
                Some(main),
                start.csp.guard.clone(),
            );
            // A redirect (or a final response URL) the owner's CSP blocked or reported is
            // reported at the owner.
            if let Some(guard) = &start.csp.guard {
                post_violations(start.id, &start.poster, guard.take_violations());
            }
            let (bytes, mime, response_url) = fetched.ok()?;
            // HTML "run a worker": a dedicated worker whose script's embedder policy is not
            // compatible with its owner's is a failure to load (the owner's `require-corp` or
            // `credentialless` needs the worker's own), and the owner reports it.
            if let Some(owner) = start.owner_embedder.as_ref().filter(|_| start.service.is_none()) {
                if matches!(response_url.scheme(), "http" | "https") && !owner_accepts_worker(owner, &start.url, page.embedder_policy()) {
                    return None;
                }
            }
            // A module worker's script is a module script: strict MIME
            // checking (HTML "fetch a single module script").
            if start.module && crate::module_script::mime_error(pyro_js::ModuleType::JavaScript, &mime, "").is_some() {
                return None;
            }
            bytes
        }
    };
    Some(bytes)
}

/// HTML "check a global object's embedder policy" for a dedicated or shared worker whose script response has the
/// embedder policy `worker`: a policy that is not `unsafe-none` needs the worker's to be one too. The
/// owner reports a mismatch of its report-only policy (`reporting`) and one of its enforced policy
/// (`enforce`, which fails the worker); whether the worker may run is the second.
fn owner_accepts_worker(owner: &OwnerEmbedder, url: &pyro_net::Url, worker: pyro_net::EmbedderPolicy) -> bool {
    let compatible = |owner: pyro_net::EmbedderPolicy| owner == pyro_net::EmbedderPolicy::UnsafeNone || worker != pyro_net::EmbedderPolicy::UnsafeNone;
    let report = |report_only: bool| {
        if let Some(report) = &owner.report {
            (report.sink)(pyro_net::CorpViolation {
                kind: "worker initialization",
                blocked_url: url.as_str().to_owned(),
                destination: None,
                report_only,
            });
        }
    };
    if !compatible(owner.report_only) {
        report(true);
    }
    let accepted = compatible(owner.policy);
    if !accepted {
        report(false);
    }
    accepted
}

/// The credentials mode of a worker's main script and module fetches.
fn script_credentials(start: &WorkerStart) -> Credentials {
    if start.module {
        start.credentials
    } else {
        Credentials::SameOrigin
    }
}

fn script_text(bytes: &[u8]) -> String {
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    String::from_utf8_lossy(bytes).into_owned()
}

/// Before the worker's main script (classic, module or a shared worker's phased module graph):
/// eval and Function follow the worker's own CSP list (its script's response's, or its owner's
/// for a `data:` or `blob:` script, both recorded by `set_up`) and Trusted Types, as a window's
/// classic scripts do (`trusted_types::sync_compile_strings`, which installs the compile hooks
/// when a policy asks). Without this a worker's realm got the hooks only once it used
/// `trustedTypes`, so a worker's string compilation went unchecked.
/// It also arms WebAssembly's gate (`sync_compile_strings` calls `csp::sync_wasm_gate`).
fn sync_compile_gate(s: &Scope<'_>, state: &WorkerLoop) {
    crate::trusted_types::sync_compile_strings(s, &state.page);
}

/// Whether a classic script ran to its end (a module's evaluation is
/// asynchronous: its failure is reported, and this says true).
fn run_script(state: &WorkerLoop, script: &(String, String), module: bool) -> bool {
    let Ok(s) = state.realm.scope() else { return false };
    sync_compile_gate(&s, state);
    if !module {
        // HTML "run a classic script": the exception is reported before the clean-up's microtask
        // checkpoint, so the evaluation and the report share one script frame (the checkpoint
        // follows the report; WPT microtasks/evaluation-order-1-throw-importScripts).
        let (source, name) = (script.0.clone(), script.1.clone());
        let ok = s.in_script_frame(move |s| match s.eval(&source, Some(&name), 1) {
            Ok(_) => true,
            Err(e) => {
                s.report_exception(&e);
                false
            }
        });
        return match ok {
            Ok(ok) => ok,
            Err(_) => match s.eval(&script.0, Some(&script.1), 1) {
                Ok(_) => true,
                Err(e) => {
                    s.report_exception(&e);
                    false
                }
            },
        };
    }
    // A module's graph loads, then it evaluates: an import that fails or a
    // throw at its top level rejects the evaluation promise, which reports
    // the error (`error` at the Worker).
    // Until it settles, what the worker is sent waits (`WorkerLoop::held`).
    state.starting.set(true);
    // One engine step fetches and evaluates the graph: its requests are static imports or
    // `import()`s of the top level, which the engine does not tell apart (`GraphPhase::mixed`).
    state.graph.mixed.set(true);
    let (started, failed) = (state.starting.clone(), state.starting.clone());
    let (graph_done, graph_failed) = (state.graph.clone(), state.graph.clone());
    let evaluated = s.eval_module(&script.0, &script.1).and_then(|promise| {
        s.then(
            promise,
            move |_, _| {
                started.set(false);
                graph_done.mixed.set(false);
            },
            move |s, error| {
                failed.set(false);
                graph_failed.mixed.set(false);
                s.report_exception(&JsError::thrown(pyro_js::Persistent::new(s, error)))
            },
        )
    });
    if let Err(e) = evaluated {
        state.starting.set(false);
        state.graph.mixed.set(false);
        s.report_exception(&e);
        return false;
    }
    true
}

/// A module shared worker's script in HTML's two steps
/// ([`Scope::load_module_graph`], then [`Scope::link_and_evaluate_module`]).
/// A graph that fails to fetch, parse or link is a failure to load: the
/// document side hears `Up::LoadFailed` (`error` at every connected
/// `SharedWorker`) and ends the worker, and what waits in `held` is never
/// delivered (`starting` stays set). A throw of the evaluation is the
/// worker's own exception: it is reported, and what waits is delivered.
/// The constructor admits module shared workers only with the two steps.
fn start_module_graph(state: &WorkerLoop, script: &(String, String)) -> bool {
    let Ok(s) = state.realm.scope() else { return false };
    sync_compile_gate(&s, state);
    let id = state.id;
    state.starting.set(true);
    // The first step fetches the graph and runs nothing: every request until it settles is a
    // static import (`GraphPhase::loading`); after it, one is an `import()`.
    state.graph.loading.set(true);
    let loaded = match s.load_module_graph(&script.0, &script.1) {
        Ok(Some(loaded)) => loaded,
        Ok(None) | Err(_) => {
            state.graph.loading.set(false);
            load_failed(&state.poster, id);
            return false;
        }
    };
    let (url, starting) = (script.1.clone(), state.starting.clone());
    let (link_poster, fetch_poster) = (state.poster.clone(), state.poster.clone());
    let (graph_loaded, graph_failed) = (state.graph.clone(), state.graph.clone());
    let watched = s.then(
        loaded,
        move |s, _| {
            graph_loaded.loading.set(false);
            match s.link_and_evaluate_module(&url) {
                Ok(promise) => {
                    let (settled, failed) = (starting.clone(), starting.clone());
                    let reported = s.then(
                        promise,
                        move |_, _| settled.set(false),
                        move |s, error| {
                            failed.set(false);
                            s.report_exception(&JsError::thrown(Persistent::new(s, error)))
                        },
                    );
                    if let Err(e) = reported {
                        starting.set(false);
                        s.report_exception(&e);
                    }
                }
                // JavaScriptCore throws the evaluation error of a graph without top-level await
                // synchronously (as for paint worklets, 6f2c7a9ce): the script ran, so the
                // error is the worker's own, reported, and what waits is delivered.
                Err(pyro_js::JsError::Thrown(e)) if !crate::paint_worklet::is_link_error(s, e.get(s)) => {
                    starting.set(false);
                    s.note_module_evaluation_error(e.get(s));
                    s.report_exception(&pyro_js::JsError::Thrown(e));
                }
                // A link error (or a parse error JSC reports at link time).
                Err(_) => load_failed(&link_poster, id),
            }
        },
        // A fetch failed, a module's MIME type was refused, or it did not parse.
        move |_, _| {
            graph_failed.loading.set(false);
            load_failed(&fetch_poster, id)
        },
    );
    if watched.is_err() {
        state.graph.loading.set(false);
        load_failed(&state.poster, id);
        return false;
    }
    true
}

/// Tells the document side that worker `id` failed to load.
fn load_failed(poster: &RemotePoster, id: u64) {
    poster(Box::new(move || deliver_up(id, Up::LoadFailed)));
}

/// The ES module source of a worker's realm: each module is fetched in a task
/// of the worker's loop, as `importScripts` fetches (CORS mode, as module
/// scripts are).
struct WorkerModuleFetcher {
    page: std::rc::Weak<PageDom>,
    net: NetClient,
    shared: Arc<Shared>,
    credentials: Credentials,
    /// The worker's origin (opaque for a data: URL worker): an `import()` fetches as it.
    origin: Option<String>,
    /// Its creator's origin: the top-level graph's static imports fetch as it (HTML "fetch a
    /// module worker script graph" uses the outside settings; WPT
    /// workers/modules/dedicated-worker-import-data-url-cross-origin.html).
    outside_origin: Option<String>,
    url: String,
    sched: Scheduler<WorkerLoop>,
    /// The fetches in flight by URL and module type, with the requests that
    /// joined each (HTML module map: an entry being fetched collects
    /// callbacks). The engine asks for a module once per load until one
    /// settles, so concurrent loads would each fetch it. Nothing stays once a
    /// fetch settles: a failed one is fetched again by the next request.
    in_flight: Rc<RefCell<HashMap<(String, pyro_js::ModuleType), Vec<pyro_js::ModuleRequest>>>>,
    /// A service worker's flag of its top-level module graph loading and
    /// evaluating (never set for a classic one): any other module request is
    /// an `import()`.
    service_graph: Option<Rc<Cell<bool>>>,
    /// The owner's CSP, which the top-level graph's static imports answer to ([`GraphPhase`]);
    /// an `import()` answers to the worker's own.
    csp: crate::worker::WorkerCsp,
    graph: Rc<GraphPhase>,
    id: u64,
    poster: RemotePoster,
    /// The referrer policy of the worker's creator (its `new Worker` or `new SharedWorker` call's
    /// document): the top-level graph's static imports are fetched under it, an `import()`
    /// under the worker script's own `Referrer-Policy`.
    owner_policy: Option<pyro_net::ReferrerPolicy>,
}

/// Violations of the owner's CSP found on the worker's thread go to the owner's document.
fn post_violations(id: u64, poster: &RemotePoster, violations: Vec<pyro_net::csp::Violation>) {
    if !violations.is_empty() {
        poster(Box::new(move || deliver_up(id, Up::Violations(violations))));
    }
}

/// The CSP check of a module a worker fetches (`dest`: its module type's destination), by
/// what the request is ([`GraphRequest`]):
/// - a static import of the top-level graph: the owner's policies, for the worker's own
///   destination (`worker-src`), violations to the owner;
/// - an `import()`: the worker's own policies, for `dest`, violations at its global;
/// - either (the engine does one step): both, blocked when either blocks; the redirect guard
///   is the owner's.
///
/// No nonce is carried: a worker's module requests have no initiating element.
fn module_csp(
    page: &Rc<PageDom>,
    csp: &crate::worker::WorkerCsp,
    request: GraphRequest,
    id: u64,
    poster: &RemotePoster,
    url: &pyro_net::Url,
    dest: pyro_net::csp::RequestDest,
) -> Result<Option<Arc<pyro_net::csp::RequestCsp>>, crate::csp::Blocked> {
    use pyro_net::csp::{CspRequest, RequestCsp, RequestDest};
    let own = || crate::csp::check_request(page, CspRequest::new(url.clone(), dest), None);
    if request == GraphRequest::Dynamic {
        return own();
    }
    let owner = match &csp.owner {
        Some(list) => {
            let worker_dest = if dest == RequestDest::Script { csp.dest } else { dest };
            let check = CspRequest::new(url.clone(), worker_dest);
            let outcome = list.pre_request(&check);
            post_violations(id, poster, outcome.violations);
            if outcome.blocked {
                return Err(crate::csp::Blocked);
            }
            Some(RequestCsp::new(list.clone(), check))
        }
        None => None,
    };
    if request == GraphRequest::Mixed {
        let own = own()?;
        return Ok(owner.or(own));
    }
    Ok(owner)
}

impl pyro_js::ModuleFetcher for WorkerModuleFetcher {
    fn document_base_url(&self, _realm: &Realm) -> Option<String> {
        Some(self.url.clone())
    }

    fn fetch(&self, realm: &Realm, url: String, request: pyro_js::ModuleRequest) {
        // HTML HostLoadImportedModule: `import()` in a ServiceWorkerGlobalScope
        // fails with a TypeError, at once (no task, no microtask checkpoint).
        if self.service_graph.as_ref().is_some_and(|loading| !loading.get()) {
            if let Ok(s) = realm.scope() {
                let e = JsError::Type("import() is disallowed on ServiceWorkerGlobalScope".into());
                if let Err(e) = request.reject(&s, &e) {
                    s.report_exception(&e);
                }
            }
            return;
        }
        let key = (url.clone(), request.module_type());
        {
            let mut in_flight = self.in_flight.borrow_mut();
            if let Some(joined) = in_flight.get_mut(&key) {
                joined.push(request);
                return;
            }
            in_flight.insert(key.clone(), Vec::new());
        }
        // A blob: URL names the blob it named when the module was requested
        // (URL parsing takes the blob URL entry): one revoked right after
        // `import()` still loads.
        let blob =
            if url.starts_with("blob:") { self.page.upgrade().and_then(|page| page.blob_url(&url)) } else { None };
        let realm = realm.downgrade();
        // What the request is is read now, as the engine makes it.
        let graph_request = self.graph.request();
        // A static import of the top-level graph fetches as the creator; a dedicated worker's
        // graph (`GraphRequest::Mixed`) cannot tell its static imports from the top level's
        // `import()`s, which take the creator's origin too until the engine marks them.
        let origin = match graph_request {
            GraphRequest::Static | GraphRequest::Mixed => self.outside_origin.clone().or_else(|| self.origin.clone()),
            GraphRequest::Dynamic => self.origin.clone(),
        };
        let (page, net, shared, in_flight, credentials) =
            (self.page.clone(), self.net.clone(), self.shared.clone(), self.in_flight.clone(), self.credentials);
        let (csp, id, poster) = (self.csp.clone(), self.id, self.poster.clone());
        // HTML "fetch the descendants of a module script" / HostLoadImportedModule: the request's
        // referrer is the importing script's URL; its policy the creator's for a static import
        // and the worker script's own for an `import()`.
        let referrer_url = request.referrer_url().and_then(|u| pyro_net::Url::parse(&u).ok());
        let owner_policy = self.owner_policy;
        let request = RefCell::new(Some(request));
        self.sched.post(TaskSource::Networking, move |_| {
            // Every request of the fetch, taken out before any completes: one
            // the engine makes while they complete starts a fetch of its own.
            let take = || {
                let joined = in_flight.borrow_mut().remove(&key).unwrap_or_default();
                request.borrow_mut().take().into_iter().chain(joined).collect::<Vec<_>>()
            };
            let Some(realm) = realm.upgrade().filter(|r| !r.is_disposed()) else {
                drop(take());
                return;
            };
            let Ok(s) = realm.scope() else {
                drop(take());
                return;
            };
            let ty = key.1;
            // A worker has no CSSStyleSheet: a CSS module fails unfetched.
            let outcome = match crate::module_script::disallowed(ty, false) {
                Some(e) => Err(e),
                None => {
                    let source = match (page.upgrade(), pyro_net::Url::parse(&url)) {
                        (Some(page), Ok(parsed)) => {
                            // CSP, once per fetch (the requests that joined it share the outcome).
                            let dest = crate::csp::module_dest(ty);
                            match module_csp(&page, &csp, graph_request, id, &poster, &parsed, dest) {
                                Err(_) => Err(crate::csp::BLOCKED_BY_CSP.to_owned()),
                                Ok(guard) => {
                                    let fetched = match blob.clone().or_else(|| page.blob_url(&url)) {
                                        Some((bytes, mime)) => Ok((bytes, pyro_net::parse_content_type(&mime).0)),
                                        None => {
                                            // A JSON module is fetched with destination "json"
                                            // (CSS modules were rejected above).
                                            let destination = match ty {
                                                pyro_js::ModuleType::Json => Destination::Json,
                                                pyro_js::ModuleType::Text => Destination::Text,
                                                _ => Destination::Script,
                                            };
                                            let policy = match graph_request {
                                                GraphRequest::Dynamic => page.referrer_policy(),
                                                _ => owner_policy.unwrap_or_else(|| page.referrer_policy()),
                                            };
                                            let import = referrer_url
                                                .clone()
                                                .map(|url| MainRequest { referrer: Some((url, policy)), response_url: RefCell::new(None), inherit_embedder: false, insecure: false });
                                            fetch_script(
                                                &page,
                                                &net,
                                                &shared,
                                                &origin,
                                                parsed,
                                                destination,
                                                RequestMode::Cors,
                                                credentials,
                                                false,
                                                import.as_ref(),
                                                guard.clone(),
                                            )
                                            .map(|(bytes, mime, _)| (bytes, mime))
                                        }
                                    };
                                    // The guard's findings go where its policy is: the owner's
                                    // for a graph request, else the worker's own.
                                    if let Some(guard) = &guard {
                                        if graph_request == GraphRequest::Dynamic || csp.owner.is_none() {
                                            crate::csp::report_guard(&page, Some(guard), None);
                                        } else {
                                            post_violations(id, &poster, guard.take_violations());
                                        }
                                    }
                                    fetched
                                }
                            }
                        }
                        _ => Err("invalid URL".to_owned()),
                    };
                    match source {
                        Ok((bytes, mime)) => match crate::module_script::mime_error(ty, &mime, &url) {
                            None => Ok(script_text(&bytes)),
                            Some(e) => Err(JsError::Type(e.into())),
                        },
                        Err(_) => Err(JsError::Type(format!("Failed to fetch module: {url}").into())),
                    }
                }
            };
            for request in take() {
                let done = match &outcome {
                    Ok(text) => crate::module_script::complete(&s, request, text),
                    Err(e) => request.reject(&s, e),
                };
                if let Err(e) = done {
                    s.report_exception(&e);
                }
            }
        });
    }
}

/// The VM goes last: everything holding handles into it is released first.
fn tear_down(event_loop: EventLoop<WorkerLoop>, state: WorkerLoop) {
    state.events.release();
    crate::service_worker_scope::tear_down();
    // Ends the nested workers this worker started (`worker::dispose`).
    state.page.dispose();
    // Their threads were told to stop: wait for them, at most `NESTED_JOIN`
    // (a thread blocked outside script, in a fetch or a sync XHR, notices
    // within its poll interval). One still running is handed to the process
    // (`worker::join_nested`) and joined by `join_stopped`. Nothing they post
    // back runs now: this loop takes no more tasks.
    crate::worker::join_nested(NESTED_JOIN);
    drop(event_loop);
    state.engine.set_hooks(None);
    drop(state);
}

/// Fetches a script, blocking this worker's thread until it ends or the worker
/// is told to stop, with its MIME type essence. `main_request`: a main
/// script's referrer, and where its response URL goes. The `main` script's
/// `Referrer-Policy` header becomes the worker's policy, and its CSP headers
/// the worker's CSP list; an imported script's do not. `csp`: the request's
/// CSP guard, checked at each redirect and against the final response URL.
#[allow(clippy::too_many_arguments)]
fn fetch_script(
    page: &Rc<PageDom>,
    net: &NetClient,
    shared: &Shared,
    origin: &Option<String>,
    url: pyro_net::Url,
    destination: Destination,
    mode: RequestMode,
    credentials: Credentials,
    main: bool,
    main_request: Option<&MainRequest>,
    csp: Option<Arc<pyro_net::csp::RequestCsp>>,
) -> Result<(Bytes, String, pyro_net::Url), String> {
    let mut request = Request::get(url, destination);
    request.mode = mode;
    request.credentials = credentials;
    request.csp = csp;
    if let Some((referrer, policy)) = main_request.and_then(|m| m.referrer.clone()) {
        request.referrer = Some(referrer);
        request.referrer_policy = policy;
    }
    request.origin = origin.clone();
    request.frame_id = page.request_context().frame_id;
    request.max_body = Some(MAX_SCRIPT_BYTES);
    page.apply_request_defaults(&mut request);
    let (tx, rx) = mpsc::channel();
    let sink: pyro_net::ResponseSink = Box::new(move |event| {
        let _ = tx.send(event);
    });
    // An imported script is in the worker's timeline (the main one is in
    // its document's); the entry is added when the timeline is next read.
    let request_url = request.url.clone();
    let sink = if main {
        sink
    } else {
        let url = request.url.clone();
        crate::resource_timing::timed_sink(page, crate::resource_timing::Initiator::Other, &url, false, sink)
    };
    let id = net.fetch(request, sink);
    let mut body: Vec<u8> = Vec::new();
    let mut mime = String::new();
    // The response's URL after redirects (a script that ends cross-origin is "muted").
    let mut final_url: Option<pyro_net::Url> = None;
    loop {
        match rx.recv_timeout(CLOSING_POLL) {
            Ok(ResponseEvent::Redirect { .. }) => {}
            Ok(ResponseEvent::Response(head)) => {
                mime = head.mime.clone();
                final_url = Some(head.url.clone());
                if !head.status.is_success() && !matches!(head.kind, ResponseKind::Opaque) {
                    net.cancel(id);
                    return Err(format!("HTTP status {}", head.status.as_u16()));
                }
                // Fetch: a script-like response is blocked by nosniff or an
                // audio/image/video/CSV MIME type (a worker's script, importScripts).
                if destination.is_script_like()
                    && pyro_net::script_response_blocked(&head.mime, pyro_net::is_nosniff(&head.headers))
                {
                    net.cancel(id);
                    return Err(format!("MIME type ('{}') is not executable", head.mime));
                }
                if let Some(main_request) = main_request {
                    *main_request.response_url.borrow_mut() = Some(head.url.clone());
                }
                if main {
                    // The worker global's embedder policy is its script response's (HTML "run a
                    // worker"); a `data:` script's stays its owner's.
                    if matches!(head.url.scheme(), "http" | "https") && !main_request.is_some_and(|m| m.inherit_embedder || m.insecure) {
                        let (policy, reporting) = crate::coep_report::obtain_embedder(&head.headers, &head.url);
                        page.set_embedder_policy(policy);
                        page.set_embedder_reporting(reporting);
                    }
                    page.set_referrer_policy_header(pyro_net::ReferrerPolicy::from_header(
                        head.headers.get_all("referrer-policy").iter().filter_map(|v| v.to_str().ok()),
                    ));
                    // The worker's own CSP list is its script's response's (CSP3 §4.2.2).
                    let joined = |name: &str| {
                        head.headers.get_all(name).iter().filter_map(|v| v.to_str().ok()).collect::<Vec<_>>().join(",")
                    };
                    let (enforce, report_only) =
                        (joined("content-security-policy"), joined("content-security-policy-report-only"));
                    if !enforce.is_empty() || !report_only.is_empty() {
                        let _ = page.dom.borrow_mut().set_trusted_types_policies(page.document(), &enforce, &report_only);
                    }
                    // ... and its `Connection-Allowlist`, which governs the worker's own requests.
                    let header = |name: &str| {
                        let values: Vec<&str> = head.headers.get_all(name).iter().filter_map(|v| v.to_str().ok()).collect();
                        (!values.is_empty()).then(|| values.join(", "))
                    };
                    let (allow, allow_report_only) = (header("connection-allowlist"), header("connection-allowlist-report-only"));
                    if allow.is_some() || allow_report_only.is_some() {
                        page.set_connection_allowlist(pyro_net::connection_allowlist::ConnectionPolicy::from_headers(
                            allow.as_deref(),
                            allow_report_only.as_deref(),
                            &head.url,
                        ));
                    }
                }
            }
            Ok(ResponseEvent::Data(bytes)) => body.extend_from_slice(&bytes),
            Ok(ResponseEvent::Finished { .. }) => {
                let url = final_url.unwrap_or_else(|| request_url.clone());
                return Ok((Bytes::from(body), mime, url));
            }
            Ok(ResponseEvent::Failed(error)) => return Err(error.to_string()),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if shared.closing.load(Ordering::Acquire) {
                    net.cancel(id);
                    return Err("the worker is closing".into());
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return Err("the request ended".into()),
        }
    }
}

/// The global's state.
pub struct ScopeData {
    target: EventTargetData,
    id: u64,
    shared: Arc<Shared>,
    poster: RemotePoster,
    url: pyro_net::Url,
    secure: bool,
    name: String,
    user_agent: String,
    sched: Scheduler<WorkerLoop>,
    timers: RefCell<HashMap<u32, TimerId>>,
    next_timer: Cell<u32>,
    /// A module worker: `importScripts` throws.
    module: bool,
    /// The time origin (`performance`), and its wall-clock time.
    started: Instant,
    origin_wall_ms: f64,
    /// Identity of `performance` once made.
    performance: Cell<Option<IdentityKey>>,
    /// The main script response's URL, after redirects (none for a `data:`
    /// or `blob:` script): the referrer source of the worker's nested
    /// workers' script requests.
    response_url: RefCell<Option<pyro_net::Url>>,
}

fn set_timer<'s>(s: &Scope<'s>, g: &ScopeData, a: Args<'s>, repeat: bool) -> JsResult<Value<'s>> {
    let handler = a.get(0);
    let delay = a.get(1).to_number()?;
    let delay = if delay.is_finite() && delay > 0.0 { delay.min(i32::MAX as f64) as u64 } else { 0 };
    let callback = if handler.is_function() {
        Persistent::new(s, handler)
    } else {
        // Trusted Types, then CSP's string compilation check, when the timer is set (as Chrome):
        // blocked, it returns 0 and sets nothing.
        let sink = if repeat { "WorkerGlobalScope setInterval" } else { "WorkerGlobalScope setTimeout" };
        let Some(source) = crate::trusted_types::timer_handler_source(s, &g.target.page, handler, sink)? else {
            return Ok(s.number(0.0));
        };
        Persistent::new(s, s.eval(&format!("(function () {{\n{source}\n}})"), Some(g.url.as_str()), 1)?)
    };
    let args: Vec<Persistent> = a.iter().skip(2).map(|v| Persistent::new(s, v)).collect();
    let handle = g.next_timer.get();
    g.next_timer.set(handle.wrapping_add(1).max(1));
    let realm = s.realm().downgrade();
    let id = g.sched.set_timer(Duration::from_millis(delay), repeat, true, move |_| {
        let Some(realm) = realm.upgrade() else { return };
        let Ok(scope) = realm.scope() else { return };
        if !repeat {
            if let Some(g) = scope.global_data::<WorkerGlobalScope>() {
                g.timers.borrow_mut().remove(&handle);
            }
        }
        let Some(function) = callback.get(&scope).as_object() else { return };
        let values: Vec<Value<'_>> = args.iter().map(|p| p.get(&scope)).collect();
        if let Err(e) = function.call(None, &values) {
            scope.report_exception(&e);
        }
    });
    g.timers.borrow_mut().insert(handle, id);
    Ok(s.number(handle as f64))
}

impl crate::window::WindowHost for ScopeData {
    fn page(&self) -> &Rc<PageDom> {
        &self.target.page
    }
    fn event_target(&self) -> &EventTargetData {
        &self.target
    }
}

fn clear_timer<'s>(s: &Scope<'s>, g: &ScopeData, a: Args<'s>) -> JsResult<Value<'s>> {
    let handle = a.get(0).to_number()?;
    if let Some(id) = g.timers.borrow_mut().remove(&(handle as u32)) {
        g.sched.clear_timer(id);
    }
    Ok(s.undefined())
}

const KEY_LOCATION: u64 = (1 << 56) | 1;
const KEY_NAVIGATOR: u64 = (1 << 56) | 2;

pub struct WorkerGlobalScope;

impl JsClass for WorkerGlobalScope {
    const NAME: &'static str = "WorkerGlobalScope";
    type Data = ScopeData;
    type Parent = EventTarget;
    fn define(def: &mut ClassDef<Self>) {
        def.upcast(|g: &ScopeData| &g.target);
        def.getter("self", |s, _g| Ok(s.global().value()));
        def.getter("location", |s, g| {
            if let Some(o) = s.lookup_identity(IdentityKey(KEY_LOCATION, 0)) {
                return Ok(o.value());
            }
            Ok(s.wrap::<WorkerLocation>(g.url.clone(), Some(IdentityKey(KEY_LOCATION, 0)))?.value())
        });
        def.getter("navigator", navigator);
        def.global_value_accessor(
            "Notification",
            |s, g| {
                let value = crate::navigator_extras::member(s, &g.target.page, "Notification")?;
                crate::lazy_module::replace(s, &g.target.page, "Notification", value)?;
                Ok(value)
            },
            |s, g, v| crate::lazy_module::replace(s, &g.target.page, "Notification", v),
        );
        // `Permissions` and `PermissionStatus` are exposed to workers too.
        macro_rules! permission_interface {
            ($name:literal) => {
                def.global_value_accessor(
                    $name,
                    |s, g| {
                        let value = crate::navigator_extras::member(s, &g.target.page, $name)?;
                        crate::lazy_module::replace(s, &g.target.page, $name, value)?;
                        Ok(value)
                    },
                    |s, g, v| crate::lazy_module::replace(s, &g.target.page, $name, v),
                );
            };
        }
        permission_interface!("Permissions");
        permission_interface!("PermissionStatus");
        // [Replaceable] (WindowOrWorkerGlobalScope): assignment shadows it
        // with an own data property; the realm's origin is left as it is.
        def.accessor(
            "origin",
            |s, _g| Ok(s.string(s.realm().origin().as_deref().unwrap_or("null"))),
            |s, g, value| crate::window::replace_window_attribute(s, &g.target.page, "origin", value),
        );
        // The worker's URL stands for its owner's context: a same-origin
        // script, or a blob:/data: URL whose origin is the creator's.
        def.getter("isSecureContext", |s, g| Ok(s.boolean(g.secure)));
        // No COOP/COEP isolation here.
        def.getter("crossOriginIsolated", |s, _g| Ok(s.boolean(false)));
        def.method_this("reportError", 1, |s, _g, this, a| {
            crate::report_error::report_error(s, this, &a, "WorkerGlobalScope", false)
        });
        def.getter("crypto", |s, _g| crate::crypto::crypto_value(s));
        def.getter("performance", |s, g| Ok(performance_of(s, g)?.value()));
        // Performance Timeline observers, made on the realm's first use.
        // The accessors sit on the global's prototype here; a read settles the
        // name (and once the other, see `observer_interface`) as own data.
        def.global_value_accessor(
            "PerformanceObserver",
            |s, g| observer_interface(s, g, "PerformanceObserver"),
            |s, g, v| crate::lazy_module::replace(s, &g.target.page, "PerformanceObserver", v),
        );
        def.global_value_accessor(
            "PerformanceObserverEntryList",
            |s, g| observer_interface(s, g, "PerformanceObserverEntryList"),
            |s, g, v| crate::lazy_module::replace(s, &g.target.page, "PerformanceObserverEntryList", v),
        );
        crate::crypto::define_globals(def);
        crate::streams::define_globals(def);
        crate::observable::define_globals(def);
        crate::scheduler::define_global(def);
        crate::canvas::define_worker_globals(def);
        crate::media_capabilities::define_globals(def);
        // CSS Font Loading: `FontFace` and the worker's `FontFaceSet`.
        crate::font_loading::define_globals(def);
        crate::typed_om::define_globals(def);
        crate::trusted_types::define_globals(def);
        def.getter("fonts", |s, g| crate::font_loading::fonts(s, &g.target.page));
        crate::storage_manager::define_globals(def);
        crate::websocket::define_worker_globals(def);
        def.getter("indexedDB", |s, g| crate::indexeddb::window_factory(s, &g.target.page));
        def.getter("caches", |s, g| crate::cache_storage::caches(s, &g.target.page));
        crate::cache_storage::define_globals(def);
        def.method("setTimeout", 1, |s, g, a| set_timer(s, g, a, false));
        def.method("setInterval", 1, |s, g, a| set_timer(s, g, a, true));
        def.method("clearTimeout", 0, clear_timer);
        def.method("clearInterval", 0, clear_timer);
        def.method("queueMicrotask", 1, |s, _g, a| {
            let callback = a.get(0).as_object().filter(|f| f.is_function()).ok_or_else(|| {
                JsError::Type(
                    "Failed to execute 'queueMicrotask' on 'WorkerGlobalScope': parameter 1 is not of type 'Function'."
                        .into(),
                )
            })?;
            let callback = Persistent::new(s, callback.value());
            s.then(
                s.undefined(),
                move |s, _| {
                    if let Some(f) = callback.get(s).as_object() {
                        if let Err(e) = f.call(None, &[]) {
                            s.report_exception(&e);
                        }
                    }
                },
                |_, _| {},
            )?;
            Ok(s.undefined())
        });
        def.method("atob", 1, |s, _g, a| {
            Ok(s.string(&crate::encoding::atob(&arg_string(&a, 0, "WorkerGlobalScope.atob")?)?))
        });
        def.method("btoa", 1, |s, _g, a| {
            Ok(s.string(&crate::encoding::btoa(&arg_string(&a, 0, "WorkerGlobalScope.btoa")?)?))
        });
        // HTML: fetched in order first, then run in order, synchronously. The
        // thread blocks; the owner page keeps running.
        def.method("importScripts", 0, |s, g, a| {
            if g.module {
                return Err(JsError::Type(
                    "Failed to execute 'importScripts' on 'WorkerGlobalScope': Module scripts don't support importScripts()."
                        .into(),
                ));
            }
            let mut urls = Vec::with_capacity(a.len());
            for value in a.iter() {
                // Trusted Types (importScripts step 1): every argument is made a compliant TrustedScriptURL
                // string under the worker's own policies before anything is fetched.
                let input = crate::trusted_types::compliant_string(s, &g.target.page, crate::trusted_types::Kind::ScriptUrl, value, "WorkerGlobalScope importScripts", false)?;
                let url = g.url.join(&input).map_err(|_| {
                    JsError::dom(
                        "SyntaxError",
                        format!("Failed to execute 'importScripts': The URL '{input}' is invalid."),
                    )
                })?;
                urls.push(url);
            }
            let page = &g.target.page;
            let net = page.net.borrow().clone().ok_or(JsError::ContextDestroyed)?;
            let origin = s.realm().origin();
            let mut scripts = Vec::with_capacity(urls.len());
            for url in urls {
                let name = url.to_string();
                // CSP: the worker's own policies, `script-src-elem` (violations at the global).
                let request = pyro_net::csp::CspRequest::new(url.clone(), pyro_net::csp::RequestDest::Script);
                let guard = crate::csp::check_request(page, request, None).map_err(|_| {
                    JsError::dom(
                        "NetworkError",
                        format!("Failed to execute 'importScripts' on 'WorkerGlobalScope': The script at '{name}' failed to load."),
                    )
                })?;
                let fetched = match page.blob_url(&name) {
                    Some((bytes, _)) => Ok((bytes, None)),
                    None => fetch_script(
                        page,
                        &net,
                        &g.shared,
                        &origin,
                        url,
                        Destination::Script,
                        RequestMode::NoCors,
                        Credentials::SameOrigin,
                        false,
                        None,
                        guard.clone(),
                    )
                    .map(|(b, mime, final_url)| (b, Some((mime, final_url)))),
                };
                crate::csp::report_guard(page, guard.as_ref(), None);
                let (bytes, response) = fetched.map_err(|e| {
                    JsError::dom(
                        "NetworkError",
                        format!("Failed to execute 'importScripts': The script at '{name}' failed to load: {e}."),
                    )
                })?;
                // HTML issue 3255 (and Chrome): an imported script needs a JavaScript
                // MIME type; text/html, text/plain, application/octet-stream ... fail.
                // A response without a Content-Type is let through, as before.
                let mut muted = false;
                if let Some((mime, final_url)) = &response {
                    // A service worker's imports must have a JavaScript MIME type, an absent one fails too.
                    let sw = s.global_data::<ServiceWorkerGlobalScope>().is_some();
                    if (!mime.is_empty() || sw) && !pyro_net::is_javascript_mime(mime) {
                        return Err(JsError::dom(
                            "NetworkError",
                            format!("Failed to execute 'importScripts': The script at '{name}' failed to load: MIME type ('{mime}') is not executable."),
                        ));
                    }
                    // A script whose response ended cross-origin has muted errors.
                    // (Only a response from another http(s) origin: a `data:` or `blob:` script is
                    // CORS-same-origin, its errors are the script's own; WPT importScripts/004, 006.)
                    muted = matches!(final_url.scheme(), "http" | "https") && !pyro_net::cors::same_origin(final_url, &g.url);
                }
                scripts.push((script_text(&bytes), name, muted));
            }
            for (text, name, muted) in scripts {
                match s.eval(&text, Some(&name), 1) {
                    // HTML "run a classic script", rethrow errors: a muted script's
                    // error is a NetworkError, so nothing of it leaks (step 8.2).
                    Err(JsError::Terminated) => return Err(JsError::Terminated),
                    Err(_) if muted => {
                        return Err(JsError::dom(
                            "NetworkError",
                            format!("Failed to execute 'importScripts' on 'WorkerGlobalScope': The script at '{name}' failed to load."),
                        ));
                    }
                    Err(e) => return Err(e),
                    Ok(_) => {}
                }
            }
            Ok(s.undefined())
        });
        def.accessor(
            "onerror",
            |s, g| crate::target::event_handler(s, &g.target, "error"),
            |s, g, value| crate::target::set_event_handler(s, &g.target, "error", value),
        );
        // Made on first use, as in a window.
        def.accessor(
            "EventSource",
            |s, g| crate::event_source::global(s, &g.target.page, "EventSource"),
            |s, g, value| crate::event_source::replace(s, &g.target.page, "EventSource", value),
        );
    }
}

pub struct DedicatedWorkerGlobalScope;
impl JsClass for DedicatedWorkerGlobalScope {
    const NAME: &'static str = "DedicatedWorkerGlobalScope";
    type Data = ScopeData;
    type Parent = WorkerGlobalScope;
    fn define(def: &mut ClassDef<Self>) {
        crate::canvas::define_video_globals(def);
        // [Replaceable]: assignment shadows it; the worker's name stays.
        def.accessor(
            "name",
            |s, g| Ok(s.string(&g.name)),
            |s, g, value| crate::window::replace_window_attribute(s, &g.target.page, "name", value),
        );
        def.method("postMessage", 1, |s, g, a| {
            a.require(1, "DedicatedWorkerGlobalScope.postMessage")?;
            let message = encode_message(s, a.get(0), a.get(1))?;
            let size = message.size();
            // The page is behind: wait for it. One message may exceed the budget alone.
            loop {
                if g.shared.closing.load(Ordering::Acquire) {
                    return Ok(s.undefined());
                }
                let queued = g.shared.up_bytes.load(Ordering::Acquire);
                if queued == 0 || queued.saturating_add(size) <= MAX_QUEUED_BYTES {
                    break;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            g.shared.up_bytes.fetch_add(size, Ordering::AcqRel);
            let id = g.id;
            (g.poster)(Box::new(move || deliver_up(id, Up::Message(message))));
            Ok(s.undefined())
        });
        def.method("close", 0, |s, g, _a| {
            g.shared.closing.store(true, Ordering::Release);
            let id = g.id;
            (g.poster)(Box::new(move || deliver_up(id, Up::Closed)));
            Ok(s.undefined())
        });
        def.accessor(
            "onmessage",
            |s, g| crate::target::event_handler(s, &g.target, "message"),
            |s, g, value| crate::target::set_event_handler(s, &g.target, "message", value),
        );
        def.accessor(
            "onmessageerror",
            |s, g| crate::target::event_handler(s, &g.target, "messageerror"),
            |s, g, value| crate::target::set_event_handler(s, &g.target, "messageerror", value),
        );
    }
}

/// A shared worker's global (HTML `SharedWorkerGlobalScope`): each document
/// that connects fires `connect` with its port (`WorkerLoop::connect`).
pub struct SharedWorkerGlobalScope;
impl JsClass for SharedWorkerGlobalScope {
    const NAME: &'static str = "SharedWorkerGlobalScope";
    type Data = ScopeData;
    type Parent = WorkerGlobalScope;
    fn define(def: &mut ClassDef<Self>) {
        // [Replaceable]: assignment shadows it; the worker's name stays.
        def.accessor(
            "name",
            |s, g| Ok(s.string(&g.name)),
            |s, g, value| crate::window::replace_window_attribute(s, &g.target.page, "name", value),
        );
        def.method("close", 0, |s, g, _a| {
            g.shared.closing.store(true, Ordering::Release);
            let id = g.id;
            (g.poster)(Box::new(move || deliver_up(id, Up::Closed)));
            Ok(s.undefined())
        });
        def.accessor(
            "onconnect",
            |s, g| crate::target::event_handler(s, &g.target, "connect"),
            |s, g, value| crate::target::set_event_handler(s, &g.target, "connect", value),
        );
    }
}

/// A service worker's global (`service_worker_scope` makes what it returns).
pub struct ServiceWorkerGlobalScope;
impl JsClass for ServiceWorkerGlobalScope {
    const NAME: &'static str = "ServiceWorkerGlobalScope";
    type Data = ScopeData;
    type Parent = WorkerGlobalScope;
    fn define(def: &mut ClassDef<Self>) {
        def.getter("clients", |s, _g| crate::service_worker_scope::member(s, "clients"));
        def.getter("registration", |s, _g| crate::service_worker_scope::member(s, "registration"));
        def.getter("serviceWorker", |s, _g| crate::service_worker_scope::member(s, "serviceWorker"));
        def.method("skipWaiting", 0, |s, _g, _a| {
            let f = crate::service_worker_scope::member(s, "skipWaiting")?;
            f.as_object().ok_or(JsError::ContextDestroyed)?.call(None, &[])
        });
        macro_rules! handlers {
            ($($attr:literal => $name:literal),+) => {$(
                def.accessor(
                    $attr,
                    |s, g| crate::target::event_handler(s, &g.target, $name),
                    |s, g, value| crate::target::set_event_handler(s, &g.target, $name, value),
                );
            )+};
        }
        def.global_getter("cookieStore", |s, g| crate::cookie_store::member(s, &g.target.page, "cookieStore"));
        def.global_accessor("oncookiechange",
            |s, g| crate::target::event_handler(s, &g.target, "cookiechange"),
            |s, g, value| crate::target::set_event_handler(s, &g.target, "cookiechange", value));
        handlers!("oninstall" => "install", "onactivate" => "activate", "onfetch" => "fetch",
            "onmessage" => "message", "onmessageerror" => "messageerror");
    }
}

pub struct WorkerLocation;
impl JsClass for WorkerLocation {
    const NAME: &'static str = "WorkerLocation";
    type Data = pyro_net::Url;
    type Parent = pyro_js::NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.getter("href", |s, u| Ok(s.string(u.as_str())));
        def.method("toString", 0, |s, u, _a| Ok(s.string(u.as_str())));
        def.getter("origin", |s, u| Ok(s.string(&u.origin().ascii_serialization())));
        def.getter("protocol", |s, u| Ok(s.string(&format!("{}:", u.scheme()))));
        def.getter("host", |s, u| {
            let host = u.host_str().unwrap_or("");
            Ok(s.string(&match u.port() {
                Some(port) => format!("{host}:{port}"),
                None => host.to_owned(),
            }))
        });
        def.getter("hostname", |s, u| Ok(s.string(u.host_str().unwrap_or(""))));
        def.getter("port", |s, u| Ok(s.string(&u.port().map_or(String::new(), |p| p.to_string()))));
        def.getter("pathname", |s, u| Ok(s.string(u.path())));
        def.getter("search", |s, u| Ok(s.string(&u.query().map_or(String::new(), |q| format!("?{q}")))));
        def.getter("hash", |s, u| Ok(s.string(&u.fragment().map_or(String::new(), |f| format!("#{f}")))));
    }
}

pub struct WorkerNavigator;
impl JsClass for WorkerNavigator {
    const NAME: &'static str = "WorkerNavigator";
    type Data = String;
    type Parent = pyro_js::NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.getter("userAgent", |s, ua| Ok(s.string(ua)));
        def.getter("appVersion", |s, ua| Ok(s.string(ua.strip_prefix("Mozilla/").unwrap_or(ua))));
        // NavigatorID's other members (WorkerNavigator has no productSub,
        // vendor or vendorSub).
        def.getter("appCodeName", |s, _ua| Ok(s.string("Mozilla")));
        def.getter("appName", |s, _ua| Ok(s.string("Netscape")));
        def.getter("product", |s, _ua| Ok(s.string("Gecko")));
        def.getter("platform", |s, _ua| {
            let page = crate::target::page_of(s)?;
            Ok(s.string(&crate::window::platform(&page)))
        });
        def.getter("hardwareConcurrency", |s, _ua| Ok(s.number(4.0)));
        def.getter("onLine", |s, _ua| Ok(s.boolean(true)));
        def.getter("language", |s, _ua| Ok(s.string("en-US")));
        def.getter("locks", |s, _ua| crate::web_locks::locks(s, &crate::target::page_of(s)?));
        def.getter("storage", |s, _ua| crate::storage_manager::storage(s, &crate::target::page_of(s)?));
        def.getter("permissions", |s, _ua| crate::navigator_extras::member(s, &crate::target::page_of(s)?, "permissions"));
        def.getter("storageBuckets", |s, _ua| crate::storage_buckets::manager(s, &crate::target::page_of(s)?));
        def.getter("mediaCapabilities", |s, _ua| crate::media_capabilities::media_capabilities(s, &crate::target::page_of(s)?));
    }
}

fn performance_of<'s>(s: &Scope<'s>, g: &ScopeData) -> JsResult<pyro_js::Object<'s>> {
    crate::performance::worker_performance(s, &g.target.page, &g.performance, g.started, g.origin_wall_ms)
}

/// `PerformanceObserver` or `PerformanceObserverEntryList`, settled as an own
/// data property of the global (WebIDL interface objects); the first read of
/// either also settles the other unless the page defined or deleted it before
/// (`otherToSettle` in performance_observer.js).
fn observer_interface<'s>(s: &Scope<'s>, g: &ScopeData, name: &str) -> JsResult<Value<'s>> {
    let module = crate::performance::worker_observer_module(s, performance_of(s, g)?)?;
    if let Some(pick) = module.get("otherToSettle")?.as_object() {
        let other = pick.call(None, &[s.global().value(), s.string(name)])?;
        if !other.is_undefined() {
            let other = other.to_string()?;
            crate::lazy_module::replace(s, &g.target.page, &other, module.get(&other)?)?;
        }
    }
    let value = module.get(name)?;
    crate::lazy_module::replace(s, &g.target.page, name, value)?;
    Ok(value)
}

/// The worker realm's `performance`.
pub(crate) fn performance<'s>(s: &Scope<'s>) -> JsResult<pyro_js::Object<'s>> {
    let g = s.global_data::<WorkerGlobalScope>().ok_or(JsError::ContextDestroyed)?;
    performance_of(s, g)
}

fn navigator<'s>(s: &Scope<'s>, g: &ScopeData) -> JsResult<Value<'s>> {
    let key = IdentityKey(KEY_NAVIGATOR, 0);
    // The worker owns its navigator and the per-realm modules
    // stored on it even when script retains only a CryptoKey or a stream.
    crate::window::same_object(s, &g.target.page, key, || s.wrap::<WorkerNavigator>(g.user_agent.clone(), Some(key)))
}

/// The per-realm object the lazily made script modules (streams, Web Crypto,
/// StorageManager) hang from as hidden properties: the realm's `navigator`.
pub(crate) fn module_holder<'s>(s: &Scope<'s>, page: &Rc<PageDom>) -> JsResult<pyro_js::Object<'s>> {
    let unavailable = || JsError::Type("this realm has no navigator".into());
    let holder = match s.global_data::<WorkerGlobalScope>() {
        Some(g) => navigator(s, g)?,
        None => crate::window::navigator(s, page)?,
    };
    holder.as_object().ok_or_else(unavailable)
}

/// Whether `s` runs in a worker global scope rather than a Window: the XHR
/// steps conditioned on "the current global object is a Window object".
pub(crate) fn is_worker(s: &Scope<'_>) -> bool {
    s.global_data::<WorkerGlobalScope>().is_some()
}

pub(crate) fn secure_worker(s: &Scope<'_>) -> Option<bool> {
    s.global_data::<WorkerGlobalScope>().map(|g| g.secure)
}

/// The classes a worker's realm exposes; returns its global's class.
fn register(engine: &Engine, service: bool, shared_worker: bool) -> pyro_js::ClassId {
    engine.register::<EventTarget>();
    crate::trusted_types::register(engine);
    engine.register::<crate::scheduler::WebScheduler>();
    crate::event::register_for_worker(engine);
    crate::abort::register(engine);
    crate::message_channel::register(engine);
    crate::file::register(engine);
    engine.register::<crate::file_reader::FileReader>();
    crate::encoding::register(engine);
    crate::crypto::register(engine);
    crate::form_data::register(engine);
    engine.register::<crate::headers::Headers>();
    engine.register::<crate::url::URL>();
    engine.register::<crate::origin::Origin>();
    engine.register::<crate::url_pattern::URLPattern>();
    engine.register::<crate::url_search_params::URLSearchParams>();
    engine.register::<crate::url_search_params::ParamsIterator>();
    engine.register::<crate::response::Response>();
    engine.register::<crate::response::BodySource>();
    engine.register::<crate::xhr::WorkerXMLHttpRequest>();
    engine.register::<crate::xhr::XMLHttpRequestUpload>();
    engine.register::<crate::indexeddb::IdbFactory>();
    crate::storage_buckets::register(engine);
    crate::opfs::register(engine);
    engine.register::<crate::cache_storage::CacheHandle>();
    engine.register::<crate::cache_storage::StoredBody>();
    engine.register::<crate::request::Request>();
    engine.register::<WorkerLocation>();
    engine.register::<WorkerNavigator>();
    crate::performance::register_for_worker(engine);
    crate::canvas::register_worker(engine);
    engine.register::<WorkerGlobalScope>();
    if service {
        return engine.register::<ServiceWorkerGlobalScope>();
    }
    engine.register::<crate::file_reader::FileReaderSync>();
    if shared_worker {
        return engine.register::<SharedWorkerGlobalScope>();
    }
    // Nested dedicated workers (HTML: `Worker` is exposed in dedicated
    // workers). Not yet in shared workers: see docs/webapi/WORKER.md.
    crate::worker::register(engine);
    crate::audio_data::register(engine);
    crate::encoded_chunks::register(engine);
    engine.register::<DedicatedWorkerGlobalScope>()
}

/// These secure-context interface objects belong on the worker global itself.
fn install_lock_interfaces(s: &Scope<'_>) -> JsResult<()> {
    macro_rules! interface {
        ($name:ident) => {{
            struct $name;
            impl pyro_js::LazyGlobalInterface for $name {
                const NAME: &'static str = stringify!($name);
                fn value<'s>(s: &Scope<'s>) -> JsResult<Value<'s>> {
                    let page = crate::target::page_of(s)?;
                    crate::web_locks::MODULE.module(s, &page)?.get_atom(Self::NAME)
                }
            }
            s.define_lazy_global_interface::<$name>()?;
        }};
    }
    interface!(LockManager);
    interface!(Lock);
    Ok(())
}

/// The part of `crate::install` that applies without a window.
fn install(s: &Scope<'_>, page: &Rc<PageDom>, global: &str) -> JsResult<()> {
    let realm = s.realm();
    page.register_realm(&realm);
    realm.set_embedder(page.clone());
    realm.set_unhandled_rejection_handler(crate::event::dispatch_unhandled_rejection);
    realm.set_rejection_hooks(crate::event::REJECTION_HOOKS);
    // HTML "report the error": the global's `error` event fires in the task
    // that threw, its listeners before their microtasks.
    realm.set_exception_reporter(Some(crate::lifecycle::report_exception));
    let helpers = crate::install_helpers(s)?;
    crate::file::install(s, helpers.get_atom("blobParts")?)?;
    crate::encoding::install(s, helpers.get_atom("encoding")?)?;
    crate::headers::install(s, helpers.get_atom("headers")?)?;
    crate::url_search_params::install(s, helpers.get_atom("urlSearchParams")?)?;
    crate::form_data::install(s, helpers.get_atom("formData")?)?;
    crate::fetch::install(s)?;
    crate::fetch::install_wasm_streaming(s, helpers.get_atom("wasmStreaming")?)?;
    crate::xhr::install(s, page)?;
    crate::structured_clone::install(s, page, helpers.get_atom("structuredClone")?)?;
    // What the lazy globals (streams, CryptoKey) replace themselves with.
    if let Some(define) =
        s.global().get_atom("Object")?.as_object().map(|o| o.get_atom("defineProperty")).transpose()?
    {
        page.define_properties.borrow_mut().insert(s.realm_id(), Persistent::new(s, define));
    }
    crate::reporting::install_worker(s)?;
    if secure_worker(s) == Some(true) {
        crate::storage_manager::install_worker_file_system_globals(s)?;
        install_lock_interfaces(s)?;
        if s.global_data::<DedicatedWorkerGlobalScope>().is_some() {
            crate::audio_decoder::install_worker(s)?;
            crate::audio_encoder::install_worker(s)?;
            crate::video_encoder::install_worker(s)?;
            crate::image_decoder::install_worker(s)?;
        }
        crate::storage_buckets::install_worker(s)?;
        // `[Exposed=(DedicatedWorker,SharedWorker,Window)]`.
        if s.global_data::<DedicatedWorkerGlobalScope>().is_some() || s.global_data::<SharedWorkerGlobalScope>().is_some() {
            crate::pressure::install_worker(s)?;
        }
    } else if let Some(class) = s.global().get_atom("WorkerNavigator")?.as_object() {
        if let Some(proto) = class.get_atom("prototype")?.as_object() {
            proto.delete("locks")?;
            proto.delete("storageBuckets")?;
        }
    }
    if secure_worker(s) != Some(true) {
        s.global().delete("StorageBucketManager")?;
        s.global().delete("StorageBucket")?;
    }
    if s.global_data::<DedicatedWorkerGlobalScope>().is_some_and(|g| !g.secure) {
        s.global().delete("VideoDecoder")?;
    }
    // Before the worker's script can change the intrinsics they capture.
    crate::structured_clone::prepare_wire(s)?;
    crate::indexeddb::install_worker_globals(s, page)?;
    // The global is JSC's default one, whose prototype cannot be replaced
    // (docs/ARCHITECTURE.md), and libraries read `self.constructor.name`.
    s.eval(
        &format!("Object.defineProperty(self, 'constructor', {{value: {global}, writable: true, configurable: true}})"),
        Some("pyro://internal/worker-scope.js"),
        1,
    )?;
    // `origin` is WorkerGlobalScope's (not [Global]), so it is inherited, not
    // the global's own: assignment shadows it ([Replaceable]) and deleting
    // the shadow shows it again. The global's chain lacks the interface
    // prototype objects, so the copy moves to the global's own prototype
    // (one per realm) instead. That prototype is the realm's named properties
    // object, which refuses new properties under JSC patch 0041: `origin` then
    // stays the global's own (it was before; the worker still starts).
    s.eval(
        "(() => { const p = Object.getPrototypeOf(self), d = Object.getOwnPropertyDescriptor(self, 'origin');
           if (d && p && p !== Object.prototype && !Object.hasOwn(p, 'origin')) {
             try { Object.defineProperty(p, 'origin', d); } catch { return; }
             delete self.origin;
           } })()",
        Some("pyro://internal/worker-scope.js"),
        1,
    )?;
    Ok(())
}

/// The referrer source of a nested worker's script request when `s` is a
/// worker's realm: its main script's response URL, else its URL (a `data:`
/// or `blob:` one, which gives no referrer). None outside a worker.
pub(crate) fn nested_referrer_source(s: &Scope<'_>) -> Option<pyro_net::Url> {
    let g = s.global_data::<WorkerGlobalScope>()?;
    let url = g.response_url.borrow().clone();
    Some(url.unwrap_or_else(|| g.url.clone()))
}
