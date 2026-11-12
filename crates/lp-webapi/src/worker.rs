//! Dedicated `Worker` (HTML §10.2): the page side. `worker_scope` is the side
//! that runs on the worker's thread.
//!
//! Each worker is a thread with its own JavaScriptCore VM, event loop and an
//! empty document that gives it the bindings a window has (events, fetch, URL).
//! The two sides share no JS or DOM state: messages cross as `WireMessage`s,
//! posted to the worker's `LoopHandle` one way and to the page's `RemotePoster`
//! the other, so each message is its own task and a navigation drops what is
//! still on its way to the replaced document.
//!
//! Lifetime: the wrapper is pinned from construction until the worker closes,
//! fails to load or is terminated, or its document goes away
//! (`PageDom::dispose`). A worker whose wrapper is otherwise unreachable keeps
//! running until then, as nothing tells it that no one can message it.
//!
//! Limits: `MAX_WORKERS_PER_DOCUMENT` and, for a connection's whole thread,
//! `MAX_WORKERS_PER_THREAD`. Past either, the worker fails like one whose script
//! did not load. Messages queued in either direction are bounded by
//! `MAX_QUEUED_BYTES`: a worker sending more waits for the page, and a page
//! sending more gets a DataCloneError.
//!
//! `serve --fork`: a template forks only with no other thread alive, so while
//! `hold_for_fork` is in force a constructed worker stays `Held` (messages and
//! inspector tasks queue on it) and its thread starts in the forked child that
//! takes a connection (`release_after_fork`). A page that waits for a worker's
//! reply during template boot waits until then.
//!
//! Gaps (explicit): `{type: 'module'}` throws NotSupportedError on the system
//! JavaScriptCore framework (module workers need the patched build); `SharedWorker`
//! and `ServiceWorker` are undefined so feature detection takes the no-worker
//! path; workers cannot start workers. ArrayBuffers and MessagePorts transfer. See
//! docs/webapi/WORKER.md.
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lp_js::{ClassDef, JsClass, JsError, JsResult, Scope, TerminateHandle, WeakRealm};
use lp_runtime::LoopHandle;
use rustc_hash::FxHashMap;

use crate::event::{EventCore, EventInterface};
use crate::page::PageDom;
use crate::structured_clone::{encode_message, WireMessage};
use crate::target::{fire, page_of, pin_target, unpin_target, EventTarget, EventTargetData};
use crate::util::dom_string;
use crate::worker_scope::{self, WorkerContext, WorkerLoop, WorkerStart};
use lp_dom::events::TargetKey;

/// Live workers of one document.
pub const MAX_WORKERS_PER_DOCUMENT: usize = 16;
/// Live workers of one browser thread, that is one CDP connection (iframes
/// have their own documents).
pub const MAX_WORKERS_PER_THREAD: usize = 64;
/// Message bytes that may wait for their receiver, per direction and worker.
pub(crate) const MAX_QUEUED_BYTES: usize = 64 * 1024 * 1024;

/// A template process: workers wait for the fork (see the module docs).
static HOLD: AtomicBool = AtomicBool::new(false);
static NEXT_WORKER: AtomicU64 = AtomicU64::new(1);

thread_local! {
    /// This browser thread's live workers, for tasks posted back to it.
    static LIVE: RefCell<FxHashMap<u64, Weak<WorkerInner>>> = RefCell::default();
    /// Workers waiting for `release_after_fork`.
    static HELD: RefCell<Vec<Weak<WorkerInner>>> = const { RefCell::new(Vec::new()) };
    /// Worker threads this browser thread started and has not joined.
    static THREADS: RefCell<Vec<std::thread::JoinHandle<()>>> = const { RefCell::new(Vec::new()) };
}

/// Whether this process is a template that must not start threads.
pub(crate) fn is_held() -> bool {
    HOLD.load(Ordering::Relaxed)
}

thread_local! {
    /// A worker this thread started has ended since [`take_ended`].
    static ENDED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// On the thread that started it: a worker's thread has torn down its VM.
pub(crate) fn note_ended() {
    ENDED.with(|ended| ended.set(true));
}

/// Whether a worker this thread started has ended and nobody has asked yet.
pub fn worker_ended() -> bool {
    ENDED.with(|ended| ended.get())
}

/// [`worker_ended`], forgetting it: the caller collects what the worker
/// kept alive on this thread (the page side of its ports and messages).
pub fn take_worker_ended() -> bool {
    ENDED.with(|ended| ended.replace(false))
}

/// Remembers a worker thread for [`join_stopped`].
pub(crate) fn track_thread(thread: std::thread::JoinHandle<()>) {
    THREADS.with(|threads| {
        let mut threads = threads.borrow_mut();
        threads.retain(|t| !t.is_finished());
        threads.push(thread);
    });
}

pub(crate) fn new_shared() -> Arc<Shared> {
    Arc::new(Shared {
        closing: AtomicBool::new(false),
        terminate: Mutex::new(None),
        down_bytes: AtomicUsize::new(0),
        up_bytes: AtomicUsize::new(0),
    })
}

/// Makes workers wait for the fork: call before a template creates any page.
pub fn hold_for_fork() {
    HOLD.store(true, Ordering::Relaxed);
}

/// In a forked child that serves a connection: starts the workers its snapshot
/// holds and every later one. Derived templates and waiting spares keep holding.
pub fn release_after_fork() {
    HOLD.store(false, Ordering::Relaxed);
    let held = HELD.with(|h| std::mem::take(&mut *h.borrow_mut()));
    for worker in held.iter().filter_map(Weak::upgrade) {
        worker.start();
    }
}

/// After the thread's pages were closed: waits for their worker threads to
/// finish (they were told to stop), so a process exit does not race a VM
/// teardown. Script that outlasts `timeout` is left to the exit. Whether every
/// thread finished.
pub fn join_stopped(timeout: Duration) -> bool {
    let threads = THREADS.with(|t| std::mem::take(&mut *t.borrow_mut()));
    let deadline = Instant::now() + timeout;
    let mut all = true;
    for thread in threads {
        while !thread.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        if thread.is_finished() {
            let _ = thread.join();
        } else {
            all = false;
        }
    }
    all
}

/// What both sides of a worker read and write.
pub(crate) struct Shared {
    pub(crate) closing: AtomicBool,
    /// Set by the worker thread once its VM exists.
    pub(crate) terminate: Mutex<Option<TerminateHandle>>,
    /// Message bytes on their way to the worker and to the page.
    down_bytes: AtomicUsize,
    pub(crate) up_bytes: AtomicUsize,
}

/// Work queued on a worker that has not started.
pub(crate) enum Early {
    Message(WireMessage),
    Inspect(InspectTask),
}

/// Runs on the worker's thread between its tasks.
pub type InspectTask = Box<dyn FnOnce(&WorkerContext<'_>) + Send>;

/// A worker's messages to the page side, delivered as tasks of the page's loop.
pub(crate) enum Up {
    Message(WireMessage),
    Error { message: String, filename: String, line: u32, column: u32 },
    LoadFailed,
    Closed,
}

enum State {
    /// Not started; `start` consumes this.
    Held(Box<WorkerStart>),
    Running(LoopHandle<WorkerLoop>),
    /// Will fire its load failure (`fail_later`).
    Failed,
    Closed,
}

pub(crate) struct WorkerInner {
    id: u64,
    url: String,
    target: EventTargetData,
    realm: WeakRealm,
    shared: Arc<Shared>,
    state: RefCell<State>,
    /// Whether the embedder was told (`WorkerEvent::Created`).
    announced: Cell<bool>,
}

/// The embedder's handle on a live worker.
#[derive(Clone)]
pub struct WorkerHandle(HandleKind);

#[derive(Clone)]
enum HandleKind {
    Dedicated(Rc<WorkerInner>),
    Service { id: u64, url: Rc<str>, thread: Option<LoopHandle<WorkerLoop>> },
}

impl WorkerHandle {
    pub(crate) fn service(id: u64, url: String, thread: Option<LoopHandle<WorkerLoop>>) -> WorkerHandle {
        WorkerHandle(HandleKind::Service { id, url: url.into(), thread })
    }
    pub fn id(&self) -> u64 {
        match &self.0 {
            HandleKind::Dedicated(worker) => worker.id,
            HandleKind::Service { id, .. } => *id,
        }
    }
    pub fn url(&self) -> &str {
        match &self.0 {
            HandleKind::Dedicated(worker) => &worker.url,
            HandleKind::Service { url, .. } => url,
        }
    }
    /// Runs `task` on the worker's thread, after the worker's initial script
    /// and whatever was queued before it. Dropped once the worker is gone.
    pub fn inspect(&self, task: InspectTask) {
        match &self.0 {
            HandleKind::Dedicated(worker) => match &mut *worker.state.borrow_mut() {
                State::Held(start) => start.early.push(Early::Inspect(task)),
                State::Running(handle) => {
                    let _ = handle.post(move |state| state.inspect(task));
                }
                State::Failed | State::Closed => {}
            },
            HandleKind::Service { thread, .. } => {
                if let Some(thread) = thread {
                    let _ = thread.post(move |state| state.inspect(task));
                }
            }
        }
    }
}

/// A worker's lifecycle, for the embedder (`PageHooks::worker_event`).
pub enum WorkerEvent {
    Created(WorkerHandle),
    /// A service worker version started; told to the hooks of the document
    /// that registered it, though it belongs to the browser context.
    ServiceCreated(WorkerHandle),
    Destroyed(u64),
}

impl WorkerInner {
    /// Starts a held worker's thread.
    fn start(self: &Rc<Self>) {
        let start = match std::mem::replace(&mut *self.state.borrow_mut(), State::Closed) {
            State::Held(start) => start,
            other => {
                *self.state.borrow_mut() = other;
                return;
            }
        };
        let page = self.target.page.clone();
        match worker_scope::spawn(*start) {
            Ok((handle, thread, early)) => {
                track_thread(thread);
                for item in early {
                    let _ = handle.post(move |state| match item {
                        Early::Message(message) => state.receive(message),
                        Early::Inspect(task) => state.inspect(task),
                    });
                }
                *self.state.borrow_mut() = State::Running(handle);
            }
            Err(_) => {
                *self.state.borrow_mut() = State::Failed;
                self.fail_later(&page);
            }
        }
    }

    /// Reports a load failure in a later task.
    fn fail_later(self: &Rc<Self>, page: &PageDom) {
        let worker = Rc::downgrade(self);
        page.post_task(Box::new(move || {
            if let Some(worker) = worker.upgrade() {
                worker.fail_to_load();
            }
        }));
    }

    fn is_closed(&self) -> bool {
        matches!(*self.state.borrow(), State::Closed)
    }

    /// A trusted, non-bubbling plain `error` Event, as a failed script load gives.
    fn fail_to_load(self: &Rc<Self>) {
        let page = &self.target.page;
        if self.is_closed() || page.is_disposed() {
            return;
        }
        let ev = EventCore::trusted(page, EventInterface::Event, "error", false, false);
        fire(page, self.target.key, &ev);
        self.finish();
    }

    /// Ends the worker: tells its thread to stop (a running script is
    /// terminated), drops the pin, and tells the embedder. Idempotent.
    fn finish(self: &Rc<Self>) {
        let state = std::mem::replace(&mut *self.state.borrow_mut(), State::Closed);
        if matches!(state, State::Closed) {
            return;
        }
        stop_thread(&self.shared);
        // An idle worker sleeps in its loop: the task wakes it to see `closing`.
        if let State::Running(handle) = &state {
            let _ = handle.post(|_| {});
        }
        LIVE.with(|live| live.borrow_mut().remove(&self.id));
        let page = &self.target.page;
        let removed: Vec<_> = {
            let mut workers = page.workers.borrow_mut();
            let (gone, kept) = std::mem::take(&mut *workers).into_iter().partition(|w| Rc::ptr_eq(w, self));
            *workers = kept;
            gone
        };
        drop(removed);
        if !page.is_disposed() {
            if let Some(realm) = self.realm.upgrade() {
                if let Ok(s) = realm.scope() {
                    unpin_target(&s, page, self.target.key);
                }
            }
        }
        if self.announced.get() {
            if let Some(hooks) = page.hooks() {
                hooks.worker_event(WorkerEvent::Destroyed(self.id));
            }
        }
    }

    fn receive(self: &Rc<Self>, up: Up) {
        let page = self.target.page.clone();
        if self.is_closed() || page.is_disposed() {
            return;
        }
        match up {
            Up::Message(message) => {
                self.shared.up_bytes.fetch_sub(message.size(), Ordering::AcqRel);
                let Some(realm) = self.realm.upgrade() else { return };
                let Ok(s) = realm.scope() else { return };
                fire_message(&s, &page, self.target.key, &message);
            }
            Up::Error { message, filename, line, column } => {
                let ev = EventCore::trusted(&page, EventInterface::ErrorEvent, "error", false, true);
                {
                    let mut fields = ev.fields.borrow_mut();
                    fields.message = message;
                    fields.filename = filename;
                    fields.lineno = line;
                    fields.colno = column;
                }
                fire(&page, self.target.key, &ev);
            }
            Up::LoadFailed => self.fail_to_load(),
            Up::Closed => self.finish(),
        }
    }
}

pub(crate) fn stop_thread(shared: &Shared) {
    shared.closing.store(true, Ordering::Release);
    if let Some(terminate) = shared.terminate.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        terminate.terminate();
    }
}

/// Fires `message` or, if it cannot be rebuilt, `messageerror` at `target`.
pub(crate) fn fire_message(s: &Scope<'_>, page: &Rc<PageDom>, target: TargetKey, message: &WireMessage) {
    crate::message_channel::fire_wire_message(s, page, target, message, String::new());
}

/// Runs on the page's thread, as a task of its loop.
pub(crate) fn deliver_up(id: u64, up: Up) {
    let worker = LIVE.with(|live| live.borrow().get(&id).and_then(Weak::upgrade));
    if let Some(worker) = worker {
        worker.receive(up);
    }
}

/// `PageDom::dispose`: the document's workers end with it.
pub(crate) fn dispose(page: &PageDom) {
    if page.workers.borrow().is_empty() {
        return;
    }
    let workers = page.workers.take();
    for worker in workers {
        worker.finish();
    }
}

fn origin_of(url: &lp_net::Url) -> String {
    url.origin().ascii_serialization()
}

/// HTML "run a worker" fetches same-origin scripts only; `data:` runs with an
/// opaque origin and a `blob:` URL carries its creator's.
fn check_script_url(page: &PageDom, s: &Scope<'_>, input: &str, url: &lp_net::Url) -> JsResult<()> {
    let document = page.dom.borrow().document_data(page.document()).map(|d| d.url.to_string()).unwrap_or_default();
    let document = lp_net::Url::parse(&document).ok();
    let scheme = url.scheme();
    let owner = s.realm().origin().or_else(|| document.as_ref().map(origin_of));
    let same = scheme == "data"
        || scheme == "blob"
        || (scheme == "file" && document.as_ref().is_some_and(|d| d.scheme() == "file"))
        || owner.as_deref() == Some(origin_of(url).as_str());
    if !same {
        return Err(JsError::dom(
            "SecurityError",
            format!("Failed to construct 'Worker': Script at '{input}' cannot be accessed from this origin."),
        ));
    }
    Ok(())
}

pub struct WorkerData(Rc<WorkerInner>);

pub struct Worker;
impl JsClass for Worker {
    const NAME: &'static str = "Worker";
    type Data = WorkerData;
    type Parent = EventTarget;
    fn define(def: &mut ClassDef<Self>) {
        def.upcast(|d: &WorkerData| &d.0.target);
        def.constructor(1, |s, nt, a| {
            a.require(1, "Worker")?;
            let page = page_of(s)?;
            if page.is_disposed() {
                return Err(JsError::ContextDestroyed);
            }
            let input = dom_string(a.get(0))?;
            let mut name = String::new();
            let mut module = false;
            let options = a.get(1);
            if let Some(options) = options.as_object() {
                let kind = options.get_atom("type")?;
                if !kind.is_undefined() {
                    match dom_string(kind)?.as_str() {
                        "classic" => {}
                        // Needs the engine's module loader.
                        "module" if lp_js::Capabilities::detect().modules => module = true,
                        "module" => {
                            return Err(JsError::dom(
                                "NotSupportedError",
                                "Failed to construct 'Worker': Module workers are not supported.",
                            ))
                        }
                        other => {
                            return Err(JsError::Type(format!(
                                "Failed to construct 'Worker': '{other}' is not a valid WorkerType."
                            ).into()))
                        }
                    }
                }
                let value = options.get_atom("name")?;
                if !value.is_undefined() {
                    name = dom_string(value)?;
                }
            } else if !options.is_undefined() && !options.is_null() {
                return Err(JsError::Type("Failed to construct 'Worker': options must be an object.".into()));
            }
            let resolved = crate::html::resolve_url(&page.dom.borrow(), page.document(), &input);
            let Ok(url) = lp_net::Url::parse(&resolved) else {
                return Err(JsError::dom(
                    "SyntaxError",
                    format!("Failed to construct 'Worker': The URL '{input}' is invalid."),
                ));
            };
            check_script_url(&page, s, &input, &url)?;
            // Compiled before any message, with the intrinsics of this moment.
            crate::structured_clone::prepare_wire(s)?;
            let target = EventTargetData::new(&page);
            let TargetKey::Other(k) = target.key else { unreachable!() };
            let key = page.other_identity(k);
            let id = NEXT_WORKER.fetch_add(1, Ordering::Relaxed);
            let shared = Arc::new(Shared {
                closing: AtomicBool::new(false),
                terminate: Mutex::new(None),
                down_bytes: AtomicUsize::new(0),
                up_bytes: AtomicUsize::new(0),
            });
            let start = WorkerStart::new(id, &page, s, url, name, shared.clone()).map(|mut start| {
                start.module = module;
                start
            });
            let full = page.workers.borrow().len() >= MAX_WORKERS_PER_DOCUMENT
                || LIVE.with(|live| live.borrow().len()) >= MAX_WORKERS_PER_THREAD;
            let inner = Rc::new(WorkerInner {
                id,
                url: resolved,
                target,
                realm: s.realm().downgrade(),
                shared,
                state: RefCell::new(start.map_or(State::Failed, |start| State::Held(Box::new(start)))),
                announced: Cell::new(false),
            });
            let object = s.wrap_new::<Worker>(WorkerData(inner.clone()), nt, Some(key))?;
            pin_target(s, &page, inner.target.key, object);
            LIVE.with(|live| live.borrow_mut().insert(id, Rc::downgrade(&inner)));
            page.workers.borrow_mut().push(inner.clone());
            // No network or no way back to this thread (an embedder without a
            // loop), or too many workers: the same failure a page must handle
            // for a script that did not load.
            let failed = matches!(*inner.state.borrow(), State::Failed);
            if full || failed {
                inner.fail_later(&page);
                return Ok(object);
            }
            if let Some(hooks) = page.hooks() {
                inner.announced.set(true);
                hooks.worker_event(WorkerEvent::Created(WorkerHandle(HandleKind::Dedicated(inner.clone()))));
            }
            if HOLD.load(Ordering::Relaxed) {
                HELD.with(|held| held.borrow_mut().push(Rc::downgrade(&inner)));
            } else {
                inner.start();
            }
            Ok(object)
        });
        def.method("postMessage", 1, |s, d, a| {
            a.require(1, "Worker.postMessage")?;
            // Clone even when the message will be dropped: errors are reported regardless.
            let message = encode_message(s, a.get(0), a.get(1))?;
            let size = message.size();
            let shared = &d.0.shared;
            match &mut *d.0.state.borrow_mut() {
                State::Held(start) => {
                    shared.down_bytes.fetch_add(size, Ordering::AcqRel);
                    start.early.push(Early::Message(message));
                }
                State::Running(handle) => {
                    if shared.down_bytes.load(Ordering::Acquire).saturating_add(size) > MAX_QUEUED_BYTES {
                        return Err(JsError::dom("DataCloneError", "The worker's message queue is full"));
                    }
                    shared.down_bytes.fetch_add(size, Ordering::AcqRel);
                    let _ = handle.post(move |state| state.receive(message));
                }
                State::Failed | State::Closed => {}
            }
            Ok(s.undefined())
        });
        def.method("terminate", 0, |s, d, _a| {
            d.0.finish();
            Ok(s.undefined())
        });
        def.accessor(
            "onmessage",
            |s, d| crate::target::event_handler(s, &d.0.target, "message"),
            |s, d, value| crate::target::set_event_handler(s, &d.0.target, "message", value),
        );
        def.accessor(
            "onmessageerror",
            |s, d| crate::target::event_handler(s, &d.0.target, "messageerror"),
            |s, d, value| crate::target::set_event_handler(s, &d.0.target, "messageerror", value),
        );
        def.accessor(
            "onerror",
            |s, d| crate::target::event_handler(s, &d.0.target, "error"),
            |s, d, value| crate::target::set_event_handler(s, &d.0.target, "error", value),
        );
    }
}

/// The worker has taken a message off its queue.
pub(crate) fn message_taken(shared: &Shared, size: usize) {
    shared.down_bytes.fetch_sub(size, Ordering::AcqRel);
}

pub(crate) fn register(engine: &lp_js::Engine) {
    engine.register::<Worker>();
}
