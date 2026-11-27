//! `SharedWorker` (HTML §10.2.6): the document side. The worker runs on a
//! thread of its own as a dedicated worker does (`worker_scope`, with a
//! `SharedWorkerGlobalScope`), and is shared by every document of the browser
//! thread that constructs a `SharedWorker` with the same origin, script URL
//! and name in its context and top-level site. Each construction makes a `MessageChannel`: one port is the
//! object's `port`, the other moves to the worker, which fires `connect` with
//! it.
//!
//! Lifetime: the worker runs while a document that connected to it lives
//! (`dispose`), until it calls `close()`, or until its script fails to load
//! (an `error` event at every `SharedWorker` object connected to it).
//!
//! Module shared workers (`{type: 'module'}`) fetch their graph, then link
//! and evaluate it (`worker_scope::start_module_graph`): a graph that fails
//! to fetch, parse or link is a failure to load (`error` at every connected
//! object, no `connect`), a throw of its evaluation is the worker's own. They
//! need the engine's module loader with its two steps
//! (`pyro_js::Scope::has_module_phases`, JSC patch 0037); without them the
//! constructor throws NotSupportedError. `credentials` is the credentials mode
//! of the main script and of every module the worker fetches. A
//! construction that finds a live worker of another type or credentials mode
//! gets `error` instead of a connection (HTML "run a worker" matching, step
//! "worker global scope is not null").
//!
//! CSP: the constructing document's `worker-src` is checked at construction
//! (a blocked script fails to load, as a network error), and again for the
//! script's redirects and its module graph's static imports.
//!
//! Gaps: uncaught errors inside the worker are reported to its console only,
//! as in browsers; no inspector target is announced for it.
use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use pyro_dom::events::TargetKey;
use pyro_js::{ClassDef, JsClass, JsError, WeakRealm};
use pyro_net::Credentials;
use pyro_runtime::LoopHandle;
use rustc_hash::FxHashMap;

use crate::event::{EventCore, EventInterface};
use crate::page::PageDom;
use crate::structured_clone::{encode_message, WireMessage};
use crate::target::{fire, page_of, pin_target, unpin_target, EventTarget, EventTargetData};
use crate::util::dom_string;
use crate::worker::{Early, Shared, Up};
use crate::worker_scope::{self, WorkerLoop, WorkerStart};

enum State {
    Held(Box<WorkerStart>),
    Running(LoopHandle<WorkerLoop>),
    Failed,
    Closed,
}

/// One `SharedWorker` object: what connected to the worker.
struct Client {
    page: Weak<PageDom>,
    key: TargetKey,
    realm: WeakRealm,
}

#[derive(Clone, Eq, Hash, PartialEq)]
struct WorkerKey {
    network_context: usize,
    top_level_site: String,
    cross_site_ancestor: bool,
    origin: String,
    url: String,
    name: String,
}

struct SharedInner {
    id: u64,
    key: WorkerKey,
    /// What the worker was started with: a later construction with the same
    /// key and another type or credentials mode does not connect to it.
    module: bool,
    credentials: Credentials,
    shared: Arc<Shared>,
    state: RefCell<State>,
    clients: RefCell<Vec<Client>>,
    /// The worker as a service worker client.
    sw_client: Option<Rc<crate::service_worker::WorkerClient>>,
}

thread_local! {
    /// Live shared workers of this browser thread, keyed by storage partition, URL and name.
    static WORKERS: RefCell<FxHashMap<WorkerKey, Rc<SharedInner>>> = RefCell::default();
    static BY_ID: RefCell<FxHashMap<u64, Weak<SharedInner>>> = RefCell::default();
}

impl SharedInner {
    fn start(self: &Rc<Self>) {
        let state = std::mem::replace(&mut *self.state.borrow_mut(), State::Closed);
        let start = match state {
            State::Held(start) => start,
            other => {
                *self.state.borrow_mut() = other;
                return;
            }
        };
        match worker_scope::spawn(*start) {
            Ok((handle, thread, early)) => {
                crate::worker::track_thread(thread);
                for item in early {
                    let _ = handle.post(move |state| match item {
                        Early::Message(message) => state.receive(message),
                        Early::Inspect(task) => state.inspect(task),
                        Early::Connect(message) => state.connect(message),
                    });
                }
                *self.state.borrow_mut() = State::Running(handle);
            }
            Err(_) => {
                *self.state.borrow_mut() = State::Failed;
                self.fail_later();
            }
        }
    }

    fn connect(self: &Rc<Self>, message: WireMessage) {
        match &mut *self.state.borrow_mut() {
            State::Held(start) => start.early.push(Early::Connect(message)),
            State::Running(handle) => {
                let _ = handle.post(move |state| state.connect(message));
            }
            State::Failed | State::Closed => {}
        }
    }

    /// The script did not load: `error` at every connected object, in a later task.
    fn fail_later(self: &Rc<Self>) {
        let clients: Vec<_> = self.clients.borrow().iter().filter_map(|c| c.page.upgrade()).collect();
        let Some(page) = clients.first() else { return };
        let me = Rc::downgrade(self);
        page.post_task(Box::new(move || {
            if let Some(worker) = me.upgrade() {
                worker.fail_to_load();
            }
        }));
    }

    fn fail_to_load(self: &Rc<Self>) {
        let clients = std::mem::take(&mut *self.clients.borrow_mut());
        for client in &clients {
            let Some(page) = client.page.upgrade().filter(|p| !p.is_disposed()) else { continue };
            let ev = EventCore::trusted(&page, EventInterface::Event, "error", false, false);
            fire(&page, client.key, &ev);
        }
        self.unpin(&clients);
        self.finish();
    }

    fn unpin(&self, clients: &[Client]) {
        for client in clients {
            let Some(page) = client.page.upgrade().filter(|p| !p.is_disposed()) else { continue };
            if let Some(realm) = client.realm.upgrade() {
                if let Ok(s) = realm.scope() {
                    unpin_target(&s, &page, client.key);
                }
            }
        }
    }

    /// Ends the worker (idempotent): its thread stops and a later
    /// `SharedWorker` with the same key starts a new one.
    fn finish(self: &Rc<Self>) {
        let state = std::mem::replace(&mut *self.state.borrow_mut(), State::Closed);
        if matches!(state, State::Closed) {
            return;
        }
        crate::worker::stop_thread(&self.shared);
        if let State::Running(handle) = &state {
            let _ = handle.post(|_| {});
        }
        WORKERS.with(|w| {
            let mut w = w.borrow_mut();
            if w.get(&self.key).is_some_and(|i| Rc::ptr_eq(i, self)) {
                w.remove(&self.key);
            }
        });
        BY_ID.with(|b| b.borrow_mut().remove(&self.id));
        let clients = std::mem::take(&mut *self.clients.borrow_mut());
        self.unpin(&clients);
    }
}

/// A message from a shared worker's thread, as a task of the browser thread.
pub(crate) fn deliver_up(id: u64, up: Up) {
    let Some(worker) = BY_ID.with(|b| b.borrow().get(&id).and_then(Weak::upgrade)) else { return };
    match up {
        Up::LoadFailed => worker.fail_to_load(),
        Up::Closed => worker.finish(),
        // The owner's CSP, violated on the worker's thread: reported at a connected document.
        Up::Violations(violations) => {
            let page = worker.clients.borrow().iter().find_map(|c| c.page.upgrade().filter(|p| !p.is_disposed()));
            if let Some(page) = page {
                crate::csp::report(&page, violations, None);
            }
        }
        // Nothing posts to the document side directly; errors stay in the worker.
        Up::Message(_) | Up::Error { .. } => {}
    }
}

/// `page` goes away: it no longer keeps its shared workers running.
pub(crate) fn dispose(page: &PageDom) {
    let workers: Vec<Rc<SharedInner>> = WORKERS.with(|w| w.borrow().values().cloned().collect());
    for worker in workers {
        let empty = {
            let mut clients = worker.clients.borrow_mut();
            clients.retain(|c| c.page.upgrade().is_some_and(|p| !std::ptr::eq(Rc::as_ptr(&p), page) && !p.is_disposed()));
            clients.is_empty()
        };
        if empty {
            worker.finish();
        }
    }
}

/// A construction that matched a live worker of another type or credentials
/// mode: `error` at that one object, in a later task, and it is not kept alive.
fn reject_client_later(client: Client) {
    let Some(page) = client.page.upgrade() else { return };
    page.post_task(Box::new(move || {
        let Some(page) = client.page.upgrade().filter(|p| !p.is_disposed()) else { return };
        let ev = EventCore::trusted(&page, EventInterface::Event, "error", false, false);
        fire(&page, client.key, &ev);
        if let Some(realm) = client.realm.upgrade() {
            if let Ok(s) = realm.scope() {
                unpin_target(&s, &page, client.key);
            }
        }
    }));
}

const PORT: &str = "__lpSharedWorkerPort";

pub struct SharedWorkerData(EventTargetData);

pub struct SharedWorker;
impl JsClass for SharedWorker {
    const NAME: &'static str = "SharedWorker";
    type Data = SharedWorkerData;
    type Parent = EventTarget;
    fn define(def: &mut ClassDef<Self>) {
        def.upcast(|d: &SharedWorkerData| &d.0);
        def.constructor(1, |s, nt, a| {
            a.require(1, "SharedWorker")?;
            let page = page_of(s)?;
            if page.is_disposed() {
                return Err(JsError::ContextDestroyed);
            }
            // Trusted Types: the script URL is a TrustedScriptURL sink ("SharedWorker constructor").
            let input = crate::trusted_types::compliant_string(s, &page, crate::trusted_types::Kind::ScriptUrl, a.get(0), "SharedWorker constructor", false)?;
            let mut name = String::new();
            let mut module = false;
            let mut credentials = Credentials::SameOrigin;
            let options = a.get(1);
            if let Some(options) = options.as_object().filter(|_| !options.is_null()) {
                // `WorkerOptions` members in dictionary order: credentials, name, type.
                let value = options.get_atom("credentials")?;
                if !value.is_undefined() {
                    credentials = match dom_string(value)?.as_str() {
                        "omit" => Credentials::Omit,
                        "same-origin" => Credentials::SameOrigin,
                        "include" => Credentials::Include,
                        other => {
                            return Err(JsError::Type(
                                format!(
                                    "Failed to construct 'SharedWorker': '{other}' is not a valid RequestCredentials."
                                )
                                .into(),
                            ))
                        }
                    };
                }
                let value = options.get_atom("name")?;
                if !value.is_undefined() {
                    name = dom_string(value)?;
                }
                let kind = options.get_atom("type")?;
                if !kind.is_undefined() {
                    match dom_string(kind)?.as_str() {
                        "classic" => {}
                        // Needs the engine's module loader in two steps, so
                        // a graph that fails to load is told from a throw.
                        "module" if pyro_js::Capabilities::detect().modules && pyro_js::Scope::has_module_phases() => {
                            module = true
                        }
                        "module" => {
                            return Err(JsError::dom(
                                "NotSupportedError",
                                "Failed to construct 'SharedWorker': Module shared workers are not supported.",
                            ))
                        }
                        other => {
                            return Err(JsError::Type(
                                format!("Failed to construct 'SharedWorker': '{other}' is not a valid WorkerType.")
                                    .into(),
                            ))
                        }
                    }
                }
            } else if !options.is_undefined() {
                // `(USVString scriptURL, optional (DOMString or WorkerOptions) options)`.
                name = dom_string(options)?;
            }
            let resolved = crate::html::resolve_url(&page.dom.borrow(), page.document(), &input);
            let Ok(url) = pyro_net::Url::parse(&resolved) else {
                return Err(JsError::dom(
                    "SyntaxError",
                    format!("Failed to construct 'SharedWorker': The URL '{input}' is invalid."),
                ));
            };
            crate::worker::check_script_url(&page, s, &input, &url)?;
            // CSP `worker-src`: a blocked script fails to load, as a network error (below).
            let csp = crate::worker::csp_check(&page, &url, pyro_net::csp::RequestDest::SharedWorker);
            crate::structured_clone::prepare_wire(s)?;
            // The two ends: `port` here, the other moves to the worker.
            let channel = s
                .eval("new MessageChannel()", Some("pyro://internal/shared-worker.js"), 1)?
                .as_object()
                .ok_or(JsError::ContextDestroyed)?;
            let port1 = channel.get_atom("port1")?;
            let port2 = channel.get_atom("port2")?;
            let transfer = s.array(&[port2])?;
            let message = encode_message(s, s.undefined(), transfer.value())?;
            let target = EventTargetData::new(&page);
            let client_key = target.key;
            let TargetKey::Other(k) = target.key else { unreachable!() };
            let identity = page.other_identity(k);
            let object = s.wrap_new::<SharedWorker>(SharedWorkerData(target), nt, Some(identity))?;
            s.set_hidden(object, PORT, port1)?;
            pin_target(s, &page, client_key, object);
            let Ok(csp) = csp else {
                // Blocked: `error` at this object in a later task; no worker is started or
                // connected to.
                reject_client_later(Client { page: Rc::downgrade(&page), key: client_key, realm: s.realm().downgrade() });
                return Ok(object);
            };
            let origin = s.realm().origin().unwrap_or_else(|| url.origin().ascii_serialization());
            // A live worker retains its context's network client. A different
            // profile or top-level site must not reuse that worker's cookie jar.
            let network_context = page.net.borrow().as_ref().map_or(0, |net| Arc::as_ptr(net.cookies()) as usize);
            let key = WorkerKey {
                network_context,
                cross_site_ancestor: page.cookie_site_context() == pyro_net::cookies::SiteContext::CrossSite,
                top_level_site: page.storage_partition_key().unwrap_or_else(|| format!("opaque-page:{}", page.id())),
                origin,
                url: resolved,
                name: name.clone(),
            };
            let existing = WORKERS.with(|w| w.borrow().get(&key).cloned());
            let client = Client { page: Rc::downgrade(&page), key: client_key, realm: s.realm().downgrade() };
            let worker = match existing {
                Some(worker) if worker.module != module || worker.credentials != credentials => {
                    reject_client_later(client);
                    return Ok(object);
                }
                Some(worker) => worker,
                None => {
                    let id = crate::worker::NEXT_WORKER.fetch_add(1, Ordering::Relaxed);
                    let shared = Arc::new(Shared {
                        closing: AtomicBool::new(false),
                        report_end: AtomicBool::new(false),
                        terminate: Mutex::new(None),
                        down_bytes: AtomicUsize::new(0),
                        up_bytes: AtomicUsize::new(0),
                    });
                    // A service worker client, controlled by the registration its script URL matches.
                    let alive = shared.clone();
                    let sw_client = crate::service_worker::worker_client(
                        &page,
                        &url,
                        "sharedworker",
                        Box::new(move || !alive.closing.load(Ordering::Acquire)),
                    );
                    let control = sw_client.as_ref().map(|c| c.link());
                    let start = WorkerStart::new(id, &page, s, url, name, shared.clone()).map(|mut start| {
                        start.control = control;
                        start.shared_worker = true;
                        start.module = module;
                        start.credentials = credentials;
                        start.csp = csp;
                        // The script request's referrer: the creating document, under its policy
                        // (HTML "fetch a classic/module worker script graph" uses the outside settings).
                        start.referrer = page.client_referrer().map(|r| (r, page.referrer_policy()));
                        start
                    });
                    // A template process (`serve --fork`) starts no thread.
                    let state = match start {
                        Some(start) if !crate::worker::is_held() => State::Held(Box::new(start)),
                        _ => State::Failed,
                    };
                    let worker = Rc::new(SharedInner {
                        id,
                        key: key.clone(),
                        module,
                        credentials,
                        shared,
                        state: RefCell::new(state),
                        clients: RefCell::new(Vec::new()),
                        sw_client,
                    });
                    WORKERS.with(|w| w.borrow_mut().insert(key, worker.clone()));
                    BY_ID.with(|b| b.borrow_mut().insert(id, Rc::downgrade(&worker)));
                    worker
                }
            };
            worker.clients.borrow_mut().push(client);
            if matches!(*worker.state.borrow(), State::Failed) {
                worker.fail_later();
                return Ok(object);
            }
            worker.connect(message);
            worker.start();
            Ok(object)
        });
        def.getter("port", |s, d| {
            let TargetKey::Other(k) = d.0.key else { return Ok(s.undefined()) };
            let wrapper = s.lookup_identity(d.0.page.other_identity(k)).ok_or(JsError::ContextDestroyed)?;
            Ok(s.get_hidden(wrapper, PORT)?.unwrap_or(s.undefined()))
        });
        def.accessor(
            "onerror",
            |s, d| crate::target::event_handler(s, &d.0, "error"),
            |s, d, value| crate::target::set_event_handler(s, &d.0, "error", value),
        );
    }
}

/// This module's part of [`crate::thread_state::reset_thread`]: every page
/// is closed, so every shared worker ended with its last document.
pub(crate) fn thread_leftover() -> Option<&'static str> {
    if WORKERS.with(|w| !w.borrow().is_empty()) {
        return Some("shared_worker::WORKERS");
    }
    BY_ID.with(|b| b.borrow_mut().retain(|_, w| w.strong_count() > 0));
    BY_ID.with(|b| !b.borrow().is_empty()).then_some("shared_worker::BY_ID")
}

pub(crate) fn register(engine: &pyro_js::Engine) {
    engine.register::<SharedWorker>();
}
