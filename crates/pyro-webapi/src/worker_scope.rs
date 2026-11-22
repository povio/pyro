//! `DedicatedWorkerGlobalScope`: the side of a `Worker` that runs on its thread.
//!
//! The thread owns one `Engine` (its own VM), an `EventLoop` and a `PageDom` over
//! an empty document at the worker's URL. The document is never exposed: it
//! gives the realm what the bindings look up through `page_of` (events,
//! `fetch`, `URL`, base URL and origin), so those run unchanged, each on this
//! thread with this thread's state. Only the classes in `register` exist here,
//! which is what makes `document` or `Worker` undefined in a worker.
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
    user_agent: Option<UserAgentOverride>,
    extra_headers: HeaderMap,
    cache_disabled: bool,
    /// The owner's context partition: the worker's IndexedDB shares its data.
    indexed_db: crate::indexeddb::Namespace,
    /// Its Cache Storage, likewise.
    caches: crate::cache_storage::Namespace,
    permissions: crate::navigator_extras::PermissionTable,
    /// `new Worker(url, {type: 'module'})`: the script is a module.
    pub(crate) module: bool,
    pub(crate) early: Vec<Early>,
    /// A service worker version: its global is a `ServiceWorkerGlobalScope`.
    pub(crate) service: Option<crate::service_worker_scope::ServiceStart>,
    /// A shared worker: its global is a `SharedWorkerGlobalScope`, and
    /// documents connect to it (`WorkerLoop::connect`).
    pub(crate) shared_worker: bool,
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
            creator_secure: s.global_data::<WorkerGlobalScope>()
                .map_or_else(|| crate::secure_context::secure(s, page), |parent| parent.secure),
            user_agent: page.identity.borrow().value.clone(),
            extra_headers: page.extra_http_headers(),
            cache_disabled: page.cache_disabled(),
            indexed_db: page.local_storage.borrow().indexed_db(),
            caches: page.local_storage.borrow().caches(),
            permissions: page.context_services().permissions.clone(),
            module: false,
            early: Vec::new(),
            service: None,
            shared_worker: false,
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
            creator_secure: true,
            user_agent: page.identity.borrow().value.clone(),
            extra_headers: page.extra_http_headers(),
            cache_disabled: page.cache_disabled(),
            indexed_db: page.local_storage.borrow().indexed_db(),
            caches: page.local_storage.borrow().caches(),
            permissions: page.context_services().permissions.clone(),
            module,
            early: Vec::new(),
            service: Some(service),
            shared_worker: false,
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
    held: Vec<Held>,
}

/// What a worker that is still starting was sent.
enum Held {
    Message(WireMessage),
    Connect(WireMessage),
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
                    Held::Connect(message) => self.connect(message),
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
    pub(crate) fn connect(&mut self, message: WireMessage) {
        if self.starting.get() {
            return self.held.push(Held::Connect(message));
        }
        if self.shared.closing.load(Ordering::Acquire) {
            return;
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

    fn post_delayed_task(&self, delay: Duration, task: Box<dyn FnOnce()>) {
        let task = RefCell::new(Some(task));
        self.sched.set_timer(delay, false, true, move |_| {
            if let Some(task) = task.borrow_mut().take() {
                task();
            }
        });
    }

    fn navigate(&self, _request: NavigationRequest) {}

    fn remote_poster(&self) -> Option<RemotePoster> {
        let handle = self.handle.clone();
        Some(Arc::new(move |task| {
            let _ = handle.post(move |_| task());
        }))
    }
}

fn run(start: WorkerStart, ready: mpsc::SyncSender<LoopHandle<WorkerLoop>>) {
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
                let ok = !shared.closing.load(Ordering::Acquire) && run_script(&state, &script, module);
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
    let page = PageDom::new(dom, document);
    // Dedicated workers, including data/blob workers and nested workers, use
    // their creator's cluster. Shared and service workers own a distinct cluster.
    if !start.shared_worker {
        if let Some(cluster) = &start.creator_cluster {
            page.inherit_agent_cluster(cluster.clone());
        }
    }
    page.set_context_services(Rc::new(crate::navigator_extras::ContextServices::for_worker(start.permissions.clone())));
    // The worker's time origin: `performance.now()`, `Event.timeStamp` and
    // its Resource Timing entries count from here.
    let started = Instant::now();
    let origin_wall_ms = epoch_ms();
    page.set_event_clock(Rc::new(move |at| crate::performance::time_since(started, at)));
    page.set_user_agent_override(start.user_agent.clone()).map_err(|e| JsError::Type(e.to_string().into()))?;
    page.set_extra_http_headers(start.extra_headers.clone());
    page.set_cache_disabled(start.cache_disabled);
    page.set_storage_namespaces(
        crate::storage::StorageNamespace::with_shared(start.indexed_db.clone(), start.caches.clone()),
        Default::default(),
    );
    page.set_request_context(crate::RequestContext { frame_id: start.frame_id.clone(), loader_id: None });
    let window = page.create_window_target(document);
    *page.net.borrow_mut() = Some(start.net.clone());
    page.set_hooks(Some(Rc::new(ScopeHooks { sched: event_loop.scheduler(), handle: event_loop.handle() })));
    let origin = start.origin.clone().or_else(|| Some(start.url.origin().ascii_serialization()));
    let realm = engine.create_realm(RealmOptions { name: String::new(), global_class: Some(class), origin })?;
    let scope = ScopeData {
        target: EventTargetData::window(&page, window),
        id: start.id,
        shared: start.shared.clone(),
        poster: start.poster.clone(),
        url: start.url.clone(),
        secure: pyro_net::is_potentially_trustworthy_url(&start.url)
            && (start.creator_secure || start.shared_worker || start.service.is_some()),
        name: start.name.clone(),
        user_agent: crate::window::user_agent(&page),
        sched: event_loop.scheduler(),
        timers: RefCell::new(HashMap::new()),
        next_timer: Cell::new(1),
        websocket_module: RefCell::new(None),
        module: start.module,
        started,
        origin_wall_ms,
        performance: Cell::new(None),
    };
    if start.service.is_some() {
        realm.set_global_data::<ServiceWorkerGlobalScope>(scope)?;
    } else if start.shared_worker {
        realm.set_global_data::<SharedWorkerGlobalScope>(scope)?;
    } else {
        realm.set_global_data::<DedicatedWorkerGlobalScope>(scope)?;
    }
    // Static imports of a module worker and `import()` in any worker: the
    // worker's own fetches, like `importScripts`.
    realm.set_module_fetcher(Some(Rc::new(WorkerModuleFetcher {
        page: Rc::downgrade(&page),
        net: start.net.clone(),
        shared: start.shared.clone(),
        origin: start.origin.clone(),
        url: start.url.to_string(),
        sched: event_loop.scheduler(),
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
        }
    }
    // A synchronous request parks this thread: `terminate()` must still end it.
    let closing = start.shared.clone();
    crate::xhr::set_sync_watch(&page, Box::new(move || closing.closing.load(Ordering::Acquire)));
    let script = load_main_script(&page, &start);
    let state = WorkerLoop {
        engine,
        page,
        realm,
        events,
        shared: start.shared.clone(),
        id: start.id,
        poster: start.poster.clone(),
        in_error: false,
        starting: Rc::new(Cell::new(false)),
        held: Vec::new(),
    };
    if script.is_none() && start.service.is_none() && !start.shared.closing.load(Ordering::Acquire) {
        let id = start.id;
        (start.poster)(Box::new(move || deliver_up(id, Up::LoadFailed)));
    }
    Ok((state, script.map(|text| (text, start.url.to_string()))))
}

fn load_main_script(page: &Rc<PageDom>, start: &WorkerStart) -> Option<String> {
    let bytes = match (&start.script, start.url.scheme()) {
        (Some(bytes), _) => bytes.clone(),
        // A revoked or unknown object URL.
        (None, "blob") => return None,
        _ => {
            let (bytes, mime) = fetch_script(
                page,
                &start.net,
                &start.shared,
                &start.origin,
                start.url.clone(),
                Destination::Script,
                RequestMode::SameOrigin,
                true,
            )
            .ok()?;
            // A module worker's script is a module script: strict MIME
            // checking (HTML "fetch a single module script").
            if start.module && crate::module_script::mime_error(pyro_js::ModuleType::JavaScript, &mime, "").is_some() {
                return None;
            }
            bytes
        }
    };
    Some(script_text(&bytes))
}

fn script_text(bytes: &[u8]) -> String {
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    String::from_utf8_lossy(bytes).into_owned()
}

/// Whether a classic script ran to its end (a module's evaluation is
/// asynchronous: its failure is reported, and this says true).
fn run_script(state: &WorkerLoop, script: &(String, String), module: bool) -> bool {
    let Ok(s) = state.realm.scope() else { return false };
    if !module {
        if let Err(e) = s.eval(&script.0, Some(&script.1), 1) {
            s.report_exception(&e);
            return false;
        }
        return true;
    }
    // A module's graph loads, then it evaluates: an import that fails or a
    // throw at its top level rejects the evaluation promise, which reports
    // the error (`error` at the Worker).
    // Until it settles, what the worker is sent waits (`WorkerLoop::held`).
    state.starting.set(true);
    let (started, failed) = (state.starting.clone(), state.starting.clone());
    let evaluated = s.eval_module(&script.0, &script.1).and_then(|promise| {
        s.then(
            promise,
            move |_, _| started.set(false),
            move |s, error| {
                failed.set(false);
                s.report_exception(&JsError::thrown(pyro_js::Persistent::new(s, error)))
            },
        )
    });
    if let Err(e) = evaluated {
        state.starting.set(false);
        s.report_exception(&e);
        return false;
    }
    true
}

/// The ES module source of a worker's realm: each module is fetched in a task
/// of the worker's loop, as `importScripts` fetches (CORS mode, as module
/// scripts are).
struct WorkerModuleFetcher {
    page: std::rc::Weak<PageDom>,
    net: NetClient,
    shared: Arc<Shared>,
    origin: Option<String>,
    url: String,
    sched: Scheduler<WorkerLoop>,
}

impl pyro_js::ModuleFetcher for WorkerModuleFetcher {
    fn document_base_url(&self, _realm: &Realm) -> Option<String> {
        Some(self.url.clone())
    }

    fn fetch(&self, realm: &Realm, url: String, request: pyro_js::ModuleRequest) {
        let realm = realm.downgrade();
        let (page, net, shared, origin) =
            (self.page.clone(), self.net.clone(), self.shared.clone(), self.origin.clone());
        let request = RefCell::new(Some(request));
        self.sched.post(TaskSource::Networking, move |_| {
            let Some(request) = request.borrow_mut().take() else { return };
            let Some(realm) = realm.upgrade().filter(|r| !r.is_disposed()) else { return };
            let Ok(s) = realm.scope() else { return };
            // A worker has no CSSStyleSheet: a CSS module fails unfetched.
            if let Some(e) = crate::module_script::disallowed(request.module_type(), false) {
                if let Err(e) = request.reject(&s, &e) {
                    s.report_exception(&e);
                }
                return;
            }
            let source = match (page.upgrade(), pyro_net::Url::parse(&url)) {
                (Some(page), Ok(parsed)) => match page.blob_url(&url) {
                    Some((bytes, mime)) => Ok((bytes, pyro_net::parse_content_type(&mime).0)),
                    None => {
                        // A JSON module is fetched with destination "json"
                        // (CSS modules were rejected above).
                        let destination = match request.module_type() {
                            pyro_js::ModuleType::Json => Destination::Json,
                            pyro_js::ModuleType::Text => Destination::Text,
                            _ => Destination::Script,
                        };
                        fetch_script(&page, &net, &shared, &origin, parsed, destination, RequestMode::Cors, false)
                    }
                },
                _ => Err("invalid URL".to_owned()),
            };
            let done = match source {
                Ok((bytes, mime)) => match crate::module_script::mime_error(request.module_type(), &mime, &url) {
                    None => crate::module_script::complete(&s, request, &script_text(&bytes)),
                    Some(e) => request.reject(&s, &JsError::Type(e.into())),
                },
                Err(_) => request.reject(&s, &JsError::Type(format!("Failed to fetch module: {url}").into())),
            };
            if let Err(e) = done {
                s.report_exception(&e);
            }
        });
    }
}

/// The VM goes last: everything holding handles into it is released first.
fn tear_down(event_loop: EventLoop<WorkerLoop>, state: WorkerLoop) {
    state.events.release();
    crate::service_worker_scope::tear_down();
    state.page.dispose();
    drop(event_loop);
    state.engine.set_hooks(None);
    drop(state);
}

/// Fetches a script, blocking this worker's thread until it ends or the worker
/// is told to stop, with its MIME type essence. The `main` script's
/// `Referrer-Policy` header becomes the worker's policy; an imported
/// script's does not.
fn fetch_script(
    page: &Rc<PageDom>,
    net: &NetClient,
    shared: &Shared,
    origin: &Option<String>,
    url: pyro_net::Url,
    destination: Destination,
    mode: RequestMode,
    main: bool,
) -> Result<(Bytes, String), String> {
    let mut request = Request::get(url, destination);
    request.mode = mode;
    request.credentials = Credentials::SameOrigin;
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
    let sink = if main {
        sink
    } else {
        let url = request.url.clone();
        crate::resource_timing::timed_sink(page, crate::resource_timing::Initiator::Other, &url, false, sink)
    };
    let id = net.fetch(request, sink);
    let mut body: Vec<u8> = Vec::new();
    let mut mime = String::new();
    loop {
        match rx.recv_timeout(CLOSING_POLL) {
            Ok(ResponseEvent::Redirect { .. }) => {}
            Ok(ResponseEvent::Response(head)) => {
                mime = head.mime.clone();
                if !head.status.is_success() && !matches!(head.kind, ResponseKind::Opaque) {
                    net.cancel(id);
                    return Err(format!("HTTP status {}", head.status.as_u16()));
                }
                if main {
                    page.set_referrer_policy_header(pyro_net::ReferrerPolicy::from_header(
                        head.headers.get_all("referrer-policy").iter().filter_map(|v| v.to_str().ok()),
                    ));
                }
            }
            Ok(ResponseEvent::Data(bytes)) => body.extend_from_slice(&bytes),
            Ok(ResponseEvent::Finished { .. }) => return Ok((Bytes::from(body), mime)),
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
    /// websocket.js's module, made on the scope's first use of `WebSocket`.
    pub(crate) websocket_module: RefCell<Option<Persistent>>,
    /// A module worker: `importScripts` throws.
    module: bool,
    /// The time origin (`performance`), and its wall-clock time.
    started: Instant,
    origin_wall_ms: f64,
    /// Identity of `performance` once made.
    performance: Cell<Option<IdentityKey>>,
}

fn set_timer<'s>(s: &Scope<'s>, g: &ScopeData, a: Args<'s>, repeat: bool) -> JsResult<Value<'s>> {
    let handler = a.get(0);
    let delay = a.get(1).to_number()?;
    let delay = if delay.is_finite() && delay > 0.0 { delay.min(i32::MAX as f64) as u64 } else { 0 };
    let callback = if handler.is_function() {
        Persistent::new(s, handler)
    } else {
        let source = handler.to_string()?;
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
        def.getter("origin", |s, _g| Ok(s.string(s.realm().origin().as_deref().unwrap_or("null"))));
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
        def.global_value_accessor(
            "PerformanceObserver",
            |s, g| crate::performance::worker_observer_module(s, performance_of(s, g)?)?.get("PerformanceObserver"),
            |s, g, v| crate::lazy_module::replace(s, &g.target.page, "PerformanceObserver", v),
        );
        def.global_value_accessor(
            "PerformanceObserverEntryList",
            |s, g| {
                crate::performance::worker_observer_module(s, performance_of(s, g)?)?
                    .get("PerformanceObserverEntryList")
            },
            |s, g, v| crate::lazy_module::replace(s, &g.target.page, "PerformanceObserverEntryList", v),
        );
        crate::crypto::define_globals(def);
        crate::streams::define_globals(def);
        crate::observable::define_globals(def);
        crate::scheduler::define_global(def);
        crate::canvas::define_worker_globals(def);
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
                let input = value.to_string()?;
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
                let bytes =
                    match page.blob_url(&name) {
                        Some((bytes, _)) => bytes,
                        None => fetch_script(page, &net, &g.shared, &origin, url, Destination::Script, RequestMode::NoCors, false).map(|(b, _)| b).map_err(
                            |e| {
                                JsError::dom(
                            "NetworkError",
                            format!("Failed to execute 'importScripts': The script at '{name}' failed to load: {e}."),
                        )
                            },
                        )?,
                    };
                scripts.push((script_text(&bytes), name));
            }
            for (text, name) in scripts {
                s.eval(&text, Some(&name), 1)?;
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
        crate::indexeddb::define_globals(def);
        def.getter("name", |s, g| Ok(s.string(&g.name)));
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
        crate::indexeddb::define_globals(def);
        def.getter("name", |s, g| Ok(s.string(&g.name)));
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
        crate::indexeddb::define_globals(def);
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
        def.getter("hardwareConcurrency", |s, _ua| Ok(s.number(4.0)));
        def.getter("onLine", |s, _ua| Ok(s.boolean(true)));
        def.getter("language", |s, _ua| Ok(s.string("en-US")));
        def.getter("storage", |s, _ua| crate::storage_manager::storage(s, &crate::target::page_of(s)?));
    }
}

fn performance_of<'s>(s: &Scope<'s>, g: &ScopeData) -> JsResult<pyro_js::Object<'s>> {
    crate::performance::worker_performance(s, &g.target.page, &g.performance, g.started, g.origin_wall_ms)
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
    engine.register::<crate::url_pattern::URLPattern>();
    engine.register::<crate::url_search_params::URLSearchParams>();
    engine.register::<crate::url_search_params::ParamsIterator>();
    engine.register::<crate::response::Response>();
    engine.register::<crate::response::BodySource>();
    engine.register::<crate::xhr::WorkerXMLHttpRequest>();
    engine.register::<crate::xhr::XMLHttpRequestUpload>();
    engine.register::<crate::indexeddb::IdbFactory>();
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
    crate::encoded_chunks::register(engine);
    engine.register::<DedicatedWorkerGlobalScope>()
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
    crate::xhr::install(s, page)?;
    crate::structured_clone::install(s, page, helpers.get_atom("structuredClone")?)?;
    // What the lazy globals (streams, CryptoKey) replace themselves with.
    if let Some(define) =
        s.global().get_atom("Object")?.as_object().map(|o| o.get_atom("defineProperty")).transpose()?
    {
        page.define_properties.borrow_mut().insert(s.realm_id(), Persistent::new(s, define));
    }
    if s.global_data::<DedicatedWorkerGlobalScope>().is_some_and(|g| !g.secure) {
        s.global().delete("VideoDecoder")?;
    }
    // Before the worker's script can change the intrinsics they capture.
    crate::structured_clone::prepare_wire(s)?;
    // The global is JSC's default one, whose prototype cannot be replaced
    // (docs/ARCHITECTURE.md), and libraries read `self.constructor.name`.
    s.eval(
        &format!("Object.defineProperty(self, 'constructor', {{value: {global}, writable: true, configurable: true}})"),
        Some("pyro://internal/worker-scope.js"),
        1,
    )?;
    Ok(())
}
