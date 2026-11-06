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
//! `queueMicrotask`, `fetch` and its classes, `XMLHttpRequest` (without
//! `responseXML`; relative URLs resolve against the script URL), `console`,
//! `TextEncoder`/`Decoder`, `URL`, `atob`/`btoa`, `structuredClone`, `crypto`,
//! `performance.now()`.
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
use lp_dom::parse::DomRef;
use lp_dom::Dom;
use lp_js::{
    Args, ClassDef, ConsoleLevel, Engine, EngineHooks, IdentityKey, JsClass, JsError, JsException, JsResult,
    Persistent, Realm, RealmOptions, Scope, Value,
};
use lp_net::http::HeaderMap;
use lp_net::{Credentials, Destination, NetClient, Request, RequestMode, ResponseEvent, ResponseKind};
use lp_runtime::{EventLoop, LoopHandle, LoopState, Scheduler, TaskSource, TimerId};

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
    url: lp_net::Url,
    name: String,
    /// The bytes of a `blob:` URL, captured at construction like a download's:
    /// the store is the owner document's and the page may revoke the URL at once.
    script: Option<Bytes>,
    net: NetClient,
    poster: RemotePoster,
    shared: Arc<Shared>,
    frame_id: Option<String>,
    origin: Option<String>,
    user_agent: Option<UserAgentOverride>,
    extra_headers: HeaderMap,
    cache_disabled: bool,
    /// The owner's context partition: the worker's IndexedDB shares its data.
    indexed_db: crate::indexeddb::Namespace,
    pub(crate) early: Vec<Early>,
}

impl WorkerStart {
    /// `None` when the embedder gives no network or no way back to this thread.
    pub(crate) fn new(
        id: u64,
        page: &Rc<PageDom>,
        s: &Scope<'_>,
        url: lp_net::Url,
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
            user_agent: page.identity.borrow().value.clone(),
            extra_headers: page.extra_http_headers(),
            cache_disabled: page.cache_disabled(),
            indexed_db: page.local_storage.borrow().indexed_db(),
            early: Vec::new(),
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
        .name(format!("lp-worker-{}", start.id))
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
}

impl LoopState for WorkerLoop {
    fn after_task(&mut self) {
        self.engine.drain_finalized();
        self.page.after_task();
    }
}

impl WorkerLoop {
    /// A message from the page, as its own task.
    pub(crate) fn receive(&mut self, message: WireMessage) {
        message_taken(&self.shared, message.size());
        if self.shared.closing.load(Ordering::Acquire) {
            return;
        }
        let Ok(s) = self.realm.scope() else { return };
        let Some(target) = self.page.window_target() else { return };
        fire_message(&s, &self.page, target, &message);
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
        if !self.in_error {
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
        if !lp_js::sys::SOURCE_BUILD {
            event_loop.enable_native_runloop_service(Duration::from_millis(250));
        }
        event_loop
    };
    let mut event_loop = event_loop;
    if ready.send(event_loop.handle()).is_err() {
        return;
    }
    let (id, shared, poster) = (start.id, start.shared.clone(), start.poster.clone());
    match set_up(&event_loop, start) {
        Ok((mut state, script)) => {
            let closing = shared.clone();
            if let Some(script) = script {
                if !shared.closing.load(Ordering::Acquire) {
                    run_script(&state, &script);
                }
                event_loop.run_until(&mut state, None, false, move |_| closing.closing.load(Ordering::Acquire));
            }
            tear_down(event_loop, state);
        }
        Err(_) => poster(Box::new(move || deliver_up(id, Up::LoadFailed))),
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
    let class = register(&engine);
    let events = Rc::new(WorkerEvents {
        buffer: RefCell::new(VecDeque::new()),
        inspector: RefCell::new(None),
        state: RefCell::new(None),
        sched: event_loop.scheduler(),
    });
    engine.set_hooks(Some(events.clone()));
    let dom: DomRef = Rc::new(RefCell::new(Dom::new()));
    let document = dom.borrow_mut().create_document(lp_dom::DocumentKind::Html, start.url.as_str());
    dom.borrow_mut().set_document_active(document, true).map_err(|e| JsError::Type(format!("{e:?}")))?;
    let page = PageDom::new(dom, document);
    page.set_user_agent_override(start.user_agent.clone()).map_err(|e| JsError::Type(e.to_string()))?;
    page.set_extra_http_headers(start.extra_headers.clone());
    page.set_cache_disabled(start.cache_disabled);
    page.set_storage_namespaces(
        crate::storage::StorageNamespace::with_indexed_db(start.indexed_db.clone()),
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
        name: start.name.clone(),
        user_agent: crate::window::user_agent(&page),
        sched: event_loop.scheduler(),
        timers: RefCell::new(HashMap::new()),
        next_timer: Cell::new(1),
    };
    realm.set_global_data::<DedicatedWorkerGlobalScope>(scope)?;
    {
        let s = realm.scope()?;
        install(&s, &page)?;
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
    };
    if script.is_none() && !start.shared.closing.load(Ordering::Acquire) {
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
        _ => fetch_script(page, &start.net, &start.shared, &start.origin, start.url.clone(), RequestMode::SameOrigin)
            .ok()?,
    };
    Some(script_text(&bytes))
}

fn script_text(bytes: &[u8]) -> String {
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    String::from_utf8_lossy(bytes).into_owned()
}

fn run_script(state: &WorkerLoop, script: &(String, String)) {
    let Ok(s) = state.realm.scope() else { return };
    if let Err(e) = s.eval(&script.0, Some(&script.1), 1) {
        s.report_exception(&e);
    }
}

/// The VM goes last: everything holding handles into it is released first.
fn tear_down(event_loop: EventLoop<WorkerLoop>, state: WorkerLoop) {
    state.events.release();
    state.page.dispose();
    drop(event_loop);
    state.engine.set_hooks(None);
    drop(state);
}

/// Fetches a script, blocking this worker's thread until it ends or the worker
/// is told to stop.
fn fetch_script(
    page: &Rc<PageDom>,
    net: &NetClient,
    shared: &Shared,
    origin: &Option<String>,
    url: lp_net::Url,
    mode: RequestMode,
) -> Result<Bytes, String> {
    let mut request = Request::get(url, Destination::Script);
    request.mode = mode;
    request.credentials = Credentials::SameOrigin;
    request.origin = origin.clone();
    request.frame_id = page.request_context().frame_id;
    request.max_body = Some(MAX_SCRIPT_BYTES);
    page.apply_request_defaults(&mut request);
    let (tx, rx) = mpsc::channel();
    let id = net.fetch(
        request,
        Box::new(move |event| {
            let _ = tx.send(event);
        }),
    );
    let mut body: Vec<u8> = Vec::new();
    loop {
        match rx.recv_timeout(CLOSING_POLL) {
            Ok(ResponseEvent::Redirect { .. }) => {}
            Ok(ResponseEvent::Response(head)) => {
                if !head.status.is_success() && !matches!(head.kind, ResponseKind::Opaque) {
                    net.cancel(id);
                    return Err(format!("HTTP status {}", head.status.as_u16()));
                }
            }
            Ok(ResponseEvent::Data(bytes)) => body.extend_from_slice(&bytes),
            Ok(ResponseEvent::Finished { .. }) => return Ok(Bytes::from(body)),
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
    url: lp_net::Url,
    name: String,
    user_agent: String,
    sched: Scheduler<WorkerLoop>,
    timers: RefCell<HashMap<u32, TimerId>>,
    next_timer: Cell<u32>,
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
            if let Some(g) = scope.global_data::<DedicatedWorkerGlobalScope>() {
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
impl crate::indexeddb::PageOf for ScopeData {
    fn page_of(&self) -> &Rc<PageDom> {
        &self.target.page
    }
}

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
        def.getter("navigator", |s, g| {
            if let Some(o) = s.lookup_identity(IdentityKey(KEY_NAVIGATOR, 0)) {
                return Ok(o.value());
            }
            Ok(s.wrap::<WorkerNavigator>(g.user_agent.clone(), Some(IdentityKey(KEY_NAVIGATOR, 0)))?.value())
        });
        def.getter("origin", |s, _g| Ok(s.string(s.realm().origin().as_deref().unwrap_or("null"))));
        def.getter("isSecureContext", |s, g| {
            let secure = g.url.scheme() == "https" || matches!(g.url.host_str(), Some("localhost" | "127.0.0.1"));
            Ok(s.boolean(secure))
        });
        def.getter("crypto", |s, _g| crate::crypto::crypto_value(s));
        def.getter("indexedDB", |s, g| crate::indexeddb::window_factory(s, &g.target.page));
        crate::indexeddb::define_globals(def);
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
                let bytes = match page.blob_url(&name) {
                    Some((bytes, _)) => bytes,
                    None => fetch_script(page, &net, &g.shared, &origin, url, RequestMode::NoCors).map_err(|e| {
                        JsError::dom(
                            "NetworkError",
                            format!("Failed to execute 'importScripts': The script at '{name}' failed to load: {e}."),
                        )
                    })?,
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
    }
}

pub struct DedicatedWorkerGlobalScope;
impl JsClass for DedicatedWorkerGlobalScope {
    const NAME: &'static str = "DedicatedWorkerGlobalScope";
    type Data = ScopeData;
    type Parent = WorkerGlobalScope;
    fn define(def: &mut ClassDef<Self>) {
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

pub struct WorkerLocation;
impl JsClass for WorkerLocation {
    const NAME: &'static str = "WorkerLocation";
    type Data = lp_net::Url;
    type Parent = lp_js::NoParent;
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
    type Parent = lp_js::NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.getter("userAgent", |s, ua| Ok(s.string(ua)));
        def.getter("appVersion", |s, ua| Ok(s.string(ua.strip_prefix("Mozilla/").unwrap_or(ua))));
        def.getter("hardwareConcurrency", |s, _ua| Ok(s.number(4.0)));
        def.getter("onLine", |s, _ua| Ok(s.boolean(true)));
        def.getter("language", |s, _ua| Ok(s.string("en-US")));
    }
}

/// Whether `s` runs in a worker global scope rather than a Window: the XHR
/// steps conditioned on "the current global object is a Window object".
pub(crate) fn is_worker(s: &Scope<'_>) -> bool {
    s.global_data::<DedicatedWorkerGlobalScope>().is_some()
}

/// The classes a worker's realm exposes; returns its global's class.
fn register(engine: &Engine) -> lp_js::ClassId {
    engine.register::<EventTarget>();
    crate::event::register_for_worker(engine);
    crate::abort::register(engine);
    crate::message_channel::register(engine);
    crate::file::register(engine);
    crate::encoding::register(engine);
    crate::crypto::register(engine);
    crate::form_data::register(engine);
    engine.register::<crate::headers::Headers>();
    engine.register::<crate::url::URL>();
    engine.register::<crate::url_search_params::URLSearchParams>();
    engine.register::<crate::url_search_params::ParamsIterator>();
    engine.register::<crate::response::Response>();
    engine.register::<crate::response::BodySource>();
    engine.register::<crate::xhr::WorkerXMLHttpRequest>();
    engine.register::<crate::xhr::XMLHttpRequestUpload>();
    engine.register::<crate::indexeddb::IdbFactory>();
    engine.register::<WorkerLocation>();
    engine.register::<WorkerNavigator>();
    engine.register::<WorkerGlobalScope>();
    engine.register::<DedicatedWorkerGlobalScope>()
}

/// The part of `crate::install` that applies without a window.
fn install(s: &Scope<'_>, page: &Rc<PageDom>) -> JsResult<()> {
    let realm = s.realm();
    page.register_realm(&realm);
    realm.set_embedder(page.clone());
    realm.set_unhandled_rejection_handler(crate::event::dispatch_unhandled_rejection);
    crate::encoding::install(s)?;
    crate::headers::install(s)?;
    crate::url_search_params::install(s)?;
    crate::form_data::install(s)?;
    crate::fetch::install(s)?;
    crate::xhr::install(s, page)?;
    crate::structured_clone::install(s, page)?;
    // Before the worker's script can change the intrinsics they capture.
    crate::structured_clone::prepare_wire(s)?;
    // The global is JSC's default one, whose prototype cannot be replaced
    // (docs/ARCHITECTURE.md), and libraries read `self.constructor.name`.
    s.eval(
        "Object.defineProperty(self, 'constructor', {value: DedicatedWorkerGlobalScope, writable: true, configurable: true})",
        Some("lightningpanda://internal/worker-scope.js"),
        1,
    )?;
    let started = Instant::now();
    let performance = s.object();
    performance.set(
        "now",
        s.function("now", move |s, _, _| Ok(s.number(started.elapsed().as_secs_f64() * 1000.0)))?.value(),
    )?;
    performance.set_atom("timeOrigin", s.number(epoch_ms()))?;
    s.global().set_atom("performance", performance.value())
}
