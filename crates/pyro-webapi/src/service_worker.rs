//! Service workers (Service Workers spec), the browser-thread side:
//! registrations, their worker versions and lifecycle, the clients they
//! control, and the routing of a controlled client's requests to its worker.
//! `service_worker_scope` is the side that runs on a worker's thread, and
//! `service_worker.js` the page's `navigator.serviceWorker`,
//! `ServiceWorkerRegistration` and `ServiceWorker`, made on a realm's first use
//! of one of them.
//!
//! State: each browser context ([`ContextServices`]) has a [`Registry`] of
//! registrations (in memory, at most `MAX_REGISTRATIONS` per context) and of
//! the documents that are its clients. A document is a client once it touched
//! the API or was created in scope of a registration with an active worker,
//! which then controls it. That check is the only work a navigation does, and
//! only in a context that has registrations.
//!
//! Lifecycle: `register()` validates the URLs (http(s), same origin, no
//! encoded slashes), fetches the script on the page's network (CDP sees it;
//! `Service-Worker: script`), checks its JavaScript MIME type and the scope
//! against the script's directory or its `Service-Worker-Allowed` header, and
//! starts a version: a thread with its own VM, as a dedicated worker's. Once
//! its script ran, the version is `installing` (`updatefound`, and the
//! `register()` promise resolves), gets `install`, becomes `installed`
//! (`waiting`), and is activated when nothing is controlled by the previous
//! active worker or it called `skipWaiting()`: `activating`, `activate`, then
//! `activated`. A redundant version's thread is stopped. `clients.claim()`
//! makes the in-scope clients controlled (`controllerchange`). `update()`
//! fetches the script again and starts a new version only if it changed.
//! `unregister()` removes the registration at once; its versions go when no
//! client is controlled by them any more. In a context that blocks service
//! workers (Playwright's `serviceWorkers: 'block'`), `register()` registers
//! nothing and resolves with undefined.
//!
//! Requests: [`route`] is what `fetch()` and asynchronous XHR call instead of
//! the network, and [`subresource_route`] what the loader asks for a
//! document's scripts, stylesheets, images and other loads. For a document
//! that no worker controls it costs one flag test. A controlled document's
//! http(s) requests become `fetch` events of its controller (see
//! `service_worker_scope`); synchronous XHR bypasses the worker, which may need
//! the blocked thread to answer. Navigations ([`navigation_route`]) go to the
//! active worker of the registration their URL matches, with the id of the
//! client they create reserved under the frame's id: the new document takes
//! it (`resultingClientId`).
//!
//! Messages: `ServiceWorker.postMessage` and `Client.postMessage` cross as
//! serialized messages in the cross-VM format of dedicated workers
//! (`structured_clone::encode_message`): CryptoKeys and Blobs cross, and the
//! transfer list takes ArrayBuffers, MessagePorts (sides of a `port_link`
//! pipe, as for dedicated workers) and streams (over such ports). At most
//! `MAX_QUEUED_BYTES` wait for a worker.
//!
//! Gaps (explicit): no persistence (registrations live as long as the
//! context); requests of workers a controlled page starts do not go through
//! the worker; no Push, Background Sync or
//! `Clients.openWindow`; a worker does not stop when idle;
//! `clients.matchAll` and `claim` see the context's documents that are
//! clients (above), not every document; templates of `serve --fork` refuse
//! registration (NotSupportedError).
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::fmt::Write as _;
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use pyro_js::{WeakRealm, JsError, JsResult, Object, Persistent, Scope, Value};
use pyro_net::cookies::SiteContext;
use pyro_net::{Destination, NetClient, Request, RequestId, RequestMode, ResponseEvent, ResponseSink};
use pyro_runtime::LoopHandle;
use rustc_hash::FxHashMap;

use crate::navigator_extras::{quote, ContextServices};
use crate::page::PageDom;
use crate::service_worker_scope::{FetchJob, ServiceStart};
use crate::structured_clone::WireMessage;
use crate::window::WindowHost;
use crate::worker::{Shared, MAX_QUEUED_BYTES};
use crate::worker_scope::{WorkerLoop, WorkerStart};

/// Registrations one browser context may hold.
pub(crate) const MAX_REGISTRATIONS: usize = 32;
/// Register jobs that may wait for one registration's update.
const MAX_QUEUED_JOBS: usize = 16;
/// Navigations whose reserved client id waits for its document.
const MAX_RESERVED: usize = 64;
/// Live service worker versions of one browser thread (each is a thread).
pub(crate) const MAX_VERSIONS: usize = 64;
/// Largest service worker script.
const MAX_SCRIPT_BYTES: usize = 16 * 1024 * 1024;
/// Hidden property of the realm's navigator that holds the page module.
const KEY: &str = "lpServiceWorker";

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

thread_local! {
    /// This browser thread's live versions, for tasks their threads post back.
    static VERSIONS: RefCell<FxHashMap<u64, Weak<Version>>> = RefCell::default();
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Lifecycle {
    Install,
    Activate,
}

/// A browser-thread request of a worker's script (`Clients`, `skipWaiting`, ...).
pub(crate) enum QueryKind {
    SkipWaiting,
    Claim,
    MatchAll { include_uncontrolled: bool, kind: String },
    Get(String),
    Unregister,
    Update,
    /// The registration's `NavigationPreloadManager`: `enable`, `disable`, `setHeaderValue` (with
    /// `value`) or `getState`.
    Preload { op: String, value: String },
}

/// What a worker's thread tells the browser thread.
pub(crate) enum SwUp {
    /// The script ran (`false`: it did not load or threw).
    Evaluated(bool),
    /// The `install`/`activate` event's lifetime promises settled.
    Done {
        kind: Lifecycle,
        ok: bool,
    },
    Query {
        id: u64,
        kind: QueryKind,
    },
    /// The page cancelled the response a `fetch` event streams (the worker's job key).
    CancelFetch(u64),
    /// The navigation preload request of the worker's job `key` ended.
    PreloadDone(u64),
    PostToClient {
        client: String,
        message: WireMessage,
        /// Counted against the worker's budget until delivered or dropped.
        queued: crate::service_worker_scope::Queued,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    Parsed,
    Installing,
    Installed,
    Activating,
    Activated,
    Redundant,
}

impl State {
    fn name(self) -> &'static str {
        match self {
            State::Parsed => "parsed",
            State::Installing => "installing",
            State::Installed => "installed",
            State::Activating => "activating",
            State::Activated => "activated",
            State::Redundant => "redundant",
        }
    }
}

/// A browser context's registrations and clients.
#[derive(Default)]
pub struct Registry {
    registrations: RefCell<Vec<Rc<Registration>>>,
    clients: RefCell<Vec<Weak<ClientInner>>>,
    /// Navigations a worker handled: (frame id, the id reserved for the
    /// client the navigation creates, its `resultingClientId`). Taken when
    /// the frame's next document is created.
    reserved: RefCell<Vec<(String, String)>>,
    /// Documents created while the context had no registration (or none that covers them), with
    /// their creation URL: every document is a client, made when a worker first asks for the
    /// clients (`live_clients`: `claim`, `matchAll`, `get`).
    unregistered: RefCell<Vec<(Weak<PageDom>, pyro_net::Url)>>,
    /// The dedicated and shared workers of this context's documents (browser thread): clients too.
    worker_clients: RefCell<Vec<Weak<WorkerClient>>>,
    /// In a worker's own context (its thread): the link its requests take to the service worker that
    /// controls it. The browser thread's registry has none.
    remote: RefCell<Option<Arc<ControlLink>>>,
    /// Registration is blocked in this context
    /// ([`ContextServices::set_service_workers_blocked`]).
    blocked: Cell<bool>,
}

impl Registry {
    pub(crate) fn has_registrations(&self) -> bool {
        !self.registrations.borrow().is_empty()
    }

    fn live_clients(&self) -> Vec<Rc<ClientInner>> {
        // The documents that were created before a registration covered them are clients too,
        // with the URL they were created with.
        for (page, url) in self.unregistered.take() {
            if let Some(page) = page.upgrade().filter(|p| !p.is_disposed()) {
                client_made(&page, None, Some(&url));
            }
        }
        let mut clients = self.clients.borrow_mut();
        clients.retain(|c| c.upgrade().and_then(|c| c.page.upgrade()).is_some_and(|p| !p.is_disposed()));
        clients.iter().filter_map(Weak::upgrade).collect()
    }

    fn live_worker_clients(&self) -> Vec<Rc<WorkerClient>> {
        let mut clients = self.worker_clients.borrow_mut();
        clients.retain(|c| c.upgrade().is_some_and(|c| (c.alive)()));
        clients.iter().filter_map(Weak::upgrade).collect()
    }

    /// "Match service worker registration": the longest scope that prefixes `url`.
    fn matching(&self, url: &str, partition: Option<&str>, site: SiteContext) -> Option<Rc<Registration>> {
        let url = url.split('#').next().unwrap_or(url);
        self.registrations
            .borrow()
            .iter()
            .filter(|r| !r.uninstalling.get() && r.partition.as_deref() == partition && r.site == site && url.starts_with(r.scope.as_str()))
            .max_by_key(|r| r.scope.len())
            .cloned()
    }

    /// Reserves the id of the client a navigation of `frame` creates.
    fn reserve(&self, frame: &str) -> String {
        let id = random_id();
        let mut reserved = self.reserved.borrow_mut();
        reserved.retain(|(f, _)| f != frame);
        // Navigations that never made a document (failed, downloads) leave
        // theirs behind; a frame's next navigation replaces its own.
        if reserved.len() >= MAX_RESERVED {
            reserved.remove(0);
        }
        reserved.push((frame.to_owned(), id.clone()));
        id
    }

    /// The client a navigation of `frame` creates has the id `id` (a redirect hop that the same
    /// origin's worker handles keeps the id the navigation's earlier hop reserved).
    fn reserve_as(&self, frame: &str, id: &str) {
        let mut reserved = self.reserved.borrow_mut();
        reserved.retain(|(f, _)| f != frame);
        reserved.push((frame.to_owned(), id.to_owned()));
    }

    fn take_reserved(&self, frame: &str) -> Option<String> {
        let mut reserved = self.reserved.borrow_mut();
        let i = reserved.iter().position(|(f, _)| f == frame)?;
        Some(reserved.swap_remove(i).1)
    }

    fn by_id(&self, id: u64) -> Option<Rc<Registration>> {
        self.registrations.borrow().iter().find(|r| r.id == id).cloned()
    }

    fn remove(&self, registration: &Rc<Registration>) {
        self.registrations.borrow_mut().retain(|r| !Rc::ptr_eq(r, registration));
        registration.cookies.clear();
    }
}

impl Registry {
    /// Ends every registration's workers. `report`: their threads tell this
    /// one when they have ended (`crate::worker::note_ended`).
    fn close(&self, report: bool) {
        for registration in self.registrations.take() {
            if report {
                registration.report_ends();
            }
            registration.clear();
        }
    }
}

impl Drop for Registry {
    fn drop(&mut self) {
        self.close(false);
    }
}

impl crate::navigator_extras::ContextServices {
    /// The browser context is closed: its service workers end now. The
    /// registry itself lives as long as a closed page's wrappers refer to
    /// it, which is until a later collection; measured after the close of a
    /// context with one controlled page, the worker's thread and its VM
    /// stayed for five seconds in an idle server.
    pub fn close(&self) {
        self.service_workers.close(true);
    }

    /// Playwright's `serviceWorkers: 'block'` for this context
    /// (`Target.createBrowserContext` `serviceWorkers`): while set,
    /// `navigator.serviceWorker.register()` (the prototype's method too)
    /// registers nothing and resolves with undefined after the console
    /// warning Playwright's init script gives in Chrome. With no registration
    /// there is no worker, so nothing intercepts the context's requests.
    /// Registrations made before stay.
    pub fn set_service_workers_blocked(&self, blocked: bool) {
        self.service_workers.blocked.set(blocked);
    }
}

/// A document that is a service worker client.
pub(crate) struct ClientInner {
    id: String,
    page: Weak<PageDom>,
    /// The creation URL (what scopes match).
    url: String,
    origin: String,
    partition: Option<String>,
    site: SiteContext,
    nested: bool,
    controller: RefCell<Option<Rc<Version>>>,
}

/// What a worker's thread needs to send its requests through the service worker that controls it
/// (HTML/Service Workers "control and use worker client"): the browser thread owns the versions
/// (`Rc`), so the worker names its controller by id and asks the browser thread to dispatch.
pub(crate) struct ControlLink {
    /// The controlling version's id; 0: none. The browser thread sets it (a claim, an activation).
    version: AtomicU64,
    /// The worker client's id: the `clientId` of its fetch events.
    client_id: String,
    /// Runs a task on the browser thread.
    poster: crate::page::RemotePoster,
}

/// A dedicated or shared worker as a service worker client (browser thread): `Clients` lists it
/// with type `worker` or `sharedworker`, and its requests go through its controller (`ControlLink`).
pub(crate) struct WorkerClient {
    id: String,
    /// The creation URL: the worker's script URL (a `blob:` URL stays one).
    url: String,
    /// The URL the registration is matched with: the script's, or for a `blob:` script its
    /// creator's (it inherits the creator's controller).
    scope_url: String,
    kind: &'static str,
    origin: String,
    partition: Option<String>,
    site: SiteContext,
    /// The worker still runs.
    alive: Box<dyn Fn() -> bool>,
    controller: RefCell<Option<Rc<Version>>>,
    link: Arc<ControlLink>,
}

impl WorkerClient {
    fn set_controller(&self, version: Option<Rc<Version>>) {
        self.link.version.store(version.as_ref().map_or(0, |v| v.id), Ordering::Release);
        *self.controller.borrow_mut() = version;
    }

    /// The link the worker's thread keeps.
    pub(crate) fn link(&self) -> Arc<ControlLink> {
        self.link.clone()
    }

    fn json(&self) -> String {
        format!(
            "{{\"id\":{},\"url\":{},\"type\":\"{}\",\"frameType\":\"none\"}}",
            quote(&self.id),
            quote(&self.url),
            self.kind,
        )
    }
}

/// A new dedicated (`"worker"`) or shared (`"sharedworker"`) worker of `page`, whose script is
/// `url`: a client, controlled by the active worker of the registration its script URL matches (a
/// `blob:` script inherits its creator's controller). `None` for what is never a client (an opaque
/// origin, a `data:` script, no way back to this thread). The caller keeps the `Rc` as long as the
/// worker lives.
pub(crate) fn worker_client(
    page: &Rc<PageDom>,
    url: &pyro_net::Url,
    kind: &'static str,
    alive: Box<dyn Fn() -> bool>,
) -> Option<Rc<WorkerClient>> {
    if !matches!(url.scheme(), "http" | "https" | "blob") {
        return None;
    }
    let creator = client_of(page)?;
    let poster = page.remote_poster()?;
    let context = page.context_services();
    let registry = &context.service_workers;
    let controller = if url.scheme() == "blob" {
        creator.controller.borrow().clone()
    } else {
        registry
            .matching(url.as_str(), creator.partition.as_deref(), creator.site)
            .and_then(|r| r.active.borrow().clone())
    };
    let controller = controller.filter(|v| v.state.get() != State::Redundant);
    let link = Arc::new(ControlLink {
        version: AtomicU64::new(controller.as_ref().map_or(0, |v| v.id)),
        client_id: random_id(),
        poster,
    });
    let client = Rc::new(WorkerClient {
        id: link.client_id.clone(),
        url: url.to_string(),
        scope_url: if url.scheme() == "blob" { creator.url.clone() } else { url.to_string() },
        kind,
        origin: creator.origin.clone(),
        partition: creator.partition.clone(),
        site: creator.site,
        alive,
        controller: RefCell::new(controller),
        link,
    });
    let mut clients = registry.worker_clients.borrow_mut();
    clients.retain(|c| c.upgrade().is_some_and(|c| (c.alive)()));
    clients.push(Rc::downgrade(&client));
    Some(client)
}

/// The link a worker created by a worker takes over from its creator (it shares its controller);
/// `None` when the creating page is a document.
pub(crate) fn inherited_control(page: &PageDom) -> Option<Arc<ControlLink>> {
    page.context_services().service_workers.remote.borrow().clone()
}

/// Gives a worker's own context its link (the worker's thread, before its script runs).
pub(crate) fn set_worker_control(page: &PageDom, link: Option<Arc<ControlLink>>) {
    *page.context_services().service_workers.remote.borrow_mut() = link;
}

pub(crate) struct Registration {
    id: u64,
    scope: String,
    origin: String,
    partition: Option<String>,
    site: SiteContext,
    cookies: Arc<crate::cookie_store::subscriptions::Subscriptions>,
    update_via_cache: String,
    module: Cell<bool>,
    installing: RefCell<Option<Rc<Version>>>,
    waiting: RefCell<Option<Rc<Version>>>,
    active: RefCell<Option<Rc<Version>>>,
    context: Weak<ContextServices>,
    uninstalling: Cell<bool>,
    /// Bumped on every change, so a page applies snapshots in order.
    seq: Cell<u64>,
    /// An update (script fetch, evaluation, install) is under way.
    updating: Cell<bool>,
    /// Register jobs waiting for it, oldest first.
    jobs: RefCell<VecDeque<(Weak<PageDom>, pyro_net::Url, Settle)>>,
    /// The version whose script runs for the first time (before `installing`).
    evaluating: RefCell<Option<Rc<Version>>>,
    /// Navigation preload (`NavigationPreloadManager`): whether a navigation the active worker
    /// handles starts a preload request, and the value of its `Service-Worker-Navigation-Preload`
    /// header. They belong to the registration, not to a version.
    preload_enabled: Cell<bool>,
    preload_header: RefCell<String>,
}

pub(crate) struct Version {
    id: u64,
    script_url: String,
    script: Bytes,
    state: Cell<State>,
    registration: Weak<Registration>,
    thread: RefCell<Option<LoopHandle<WorkerLoop>>>,
    shared: Arc<Shared>,
    skip_waiting: Cell<bool>,
    /// What runs once the script was evaluated (the update job's continuation).
    evaluated: RefCell<Option<Evaluated>>,
    /// Message bytes on their way to the worker.
    queued: Arc<AtomicUsize>,
    /// The hooks that announced its CDP target (they outlive the page that
    /// registered it, which may go first), until the end is told.
    announced: RefCell<Option<Rc<dyn crate::page::PageHooks>>>,
}

type Evaluated = Box<dyn FnOnce(bool)>;

impl Drop for Version {
    fn drop(&mut self) {
        self.stop();
    }
}

impl Version {
    fn post(&self, task: impl FnOnce(&mut WorkerLoop) + Send + 'static) {
        if let Some(thread) = self.thread.borrow().as_ref() {
            let _ = thread.post(task);
        }
    }

    /// Runs `f` with the worker's realm, as a task on its thread.
    fn run(&self, f: impl FnOnce(&Scope<'_>, &Rc<PageDom>) + Send + 'static) {
        self.post(move |state| state.service(f));
    }

    fn stop(&self) {
        crate::worker::stop_thread(&self.shared);
        if let Some(thread) = self.thread.take() {
            let _ = thread.post(|_| {});
        }
        let id = self.id;
        let _ = VERSIONS.try_with(|v| v.borrow_mut().remove(&id));
        if let Some(hooks) = self.announced.take() {
            hooks.worker_event(crate::worker::WorkerEvent::Destroyed(id));
        }
    }

    fn set_state(&self, state: State) {
        self.state.set(state);
        let name = state.name();
        self.run(move |s, _| crate::service_worker_scope::set_state(s, name));
    }

    fn json(&self) -> String {
        format!(
            "{{\"id\":{},\"scriptURL\":{},\"state\":\"{}\"}}",
            self.id,
            quote(&self.script_url),
            self.state.get().name()
        )
    }

    /// Ends the version; its `ServiceWorker` objects in client pages, which no
    /// registration slot reaches any more, learn it (`statechange`).
    fn redundant(&self) {
        if self.state.get() == State::Redundant {
            return;
        }
        self.state.set(State::Redundant);
        self.stop();
        let Some(registration) = self.registration.upgrade() else { return };
        let Some(context) = registration.context.upgrade() else { return };
        let json = self.json();
        for client in context.service_workers.live_clients() {
            let Some(page) = client.page.upgrade().filter(|_| registration.shares_storage(&client)) else { continue };
            let json = json.clone();
            post_to_module(&page, false, move |s, glue| call(s, glue, "worker", &[s.string(&json)]));
        }
    }

    fn controls_any(self: &Rc<Self>) -> bool {
        let Some(registration) = self.registration.upgrade() else { return false };
        let Some(context) = registration.context.upgrade() else { return false };
        let clients = context.service_workers.live_clients();
        clients.iter().any(|c| c.controller.borrow().as_ref().is_some_and(|v| Rc::ptr_eq(v, self)))
            || context
                .service_workers
                .live_worker_clients()
                .iter()
                .any(|c| c.controller.borrow().as_ref().is_some_and(|v| Rc::ptr_eq(v, self)))
    }
}

impl Registration {
    fn shares_storage(&self, client: &ClientInner) -> bool {
        self.origin == client.origin && self.partition == client.partition && self.site == client.site
    }

    fn newest(&self) -> Option<Rc<Version>> {
        self.installing
            .borrow()
            .clone()
            .or_else(|| self.waiting.borrow().clone())
            .or_else(|| self.active.borrow().clone())
    }

    fn bump(&self) {
        self.seq.set(self.seq.get() + 1);
    }

    fn json(&self, matches: bool) -> String {
        let slot =
            |v: &RefCell<Option<Rc<Version>>>| v.borrow().as_ref().map_or_else(|| "null".to_owned(), |v| v.json());
        format!(
            "{{\"id\":{},\"seq\":{},\"scope\":{},\"updateViaCache\":{},\"installing\":{},\"waiting\":{},\"active\":{},\"matches\":{}}}",
            self.id,
            self.seq.get(),
            quote(&self.scope),
            quote(&self.update_via_cache),
            slot(&self.installing),
            slot(&self.waiting),
            slot(&self.active),
            matches,
        )
    }

    /// Before [`Registration::clear`] for a closed browser context: each
    /// version's thread reports its end (`Shared::report_end`).
    fn report_ends(&self) {
        for slot in [&self.evaluating, &self.installing, &self.waiting, &self.active] {
            if let Some(version) = slot.borrow().as_ref() {
                version.shared.report_end.store(true, Ordering::Release);
            }
        }
    }

    /// Ends every version (the registration is gone).
    fn clear(&self) {
        self.cookies.clear();
        for slot in [&self.evaluating, &self.installing, &self.waiting, &self.active] {
            if let Some(version) = slot.take() {
                version.redundant();
            }
        }
    }

    /// Tells each client page that touched the API about the new state.
    fn notify(self: &Rc<Self>) {
        self.bump();
        self.notify_clients(false);
    }

    fn notify_clients(self: &Rc<Self>, update_found: bool) {
        let Some(context) = self.context.upgrade() else { return };
        for client in context.service_workers.live_clients() {
            let Some(page) = client.page.upgrade() else { continue };
            if !self.shares_storage(&client) {
                continue;
            }
            let matches = context.service_workers.matching(&client.url, client.partition.as_deref(), client.site).is_some_and(|r| Rc::ptr_eq(&r, self));
            let json = self.json(matches);
            post_to_module(&page, false, move |s, glue| {
                call(s, glue, "registration", &[s.string(&json), s.boolean(update_found)]);
            });
        }
    }
}

/// Runs `f` with the page's main-world module, in a task. `create` makes the
/// module if the realm has none (a message must reach it); otherwise a realm
/// without it is skipped.
fn post_to_module(page: &Rc<PageDom>, create: bool, f: impl FnOnce(&Scope<'_>, Object<'_>) + 'static) {
    let weak = Rc::downgrade(page);
    page.post_task(Box::new(move || {
        let Some(page) = weak.upgrade().filter(|p| !p.is_disposed()) else { return };
        let Some(realm) = page.main_world_realm() else { return };
        let Ok(s) = realm.scope() else { return };
        let module = if create { module(&s, &page).ok() } else { existing(&s, &page).ok().flatten() };
        if let Some(module) = module {
            f(&s, module);
        }
    }));
}

fn call(s: &Scope<'_>, glue: Object<'_>, name: &str, args: &[Value<'_>]) {
    let result = glue.get(name).and_then(|f| match f.as_object() {
        Some(f) => f.call(None, args),
        None => Ok(s.undefined()),
    });
    if let Err(error) = result {
        if !matches!(error, JsError::Terminated | JsError::ContextDestroyed) {
            s.report_exception(&error);
        }
    }
}

fn random_id() -> String {
    let mut bytes = [0u8; 16];
    let _ = getrandom::fill(&mut bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let mut out = String::with_capacity(36);
    for (i, b) in bytes.iter().enumerate() {
        if matches!(i, 4 | 6 | 8 | 10) {
            out.push('-');
        }
        let _ = write!(out, "{b:02x}");
    }
    out
}

fn tuple_origin(url: &pyro_net::Url) -> Option<String> {
    let origin = url.origin();
    origin.is_tuple().then(|| origin.ascii_serialization())
}

/// A document whose origin is opaque although its URL's is not (a sandboxed
/// frame without `allow-same-origin`): it is never a client, and its
/// `navigator.serviceWorker` throws (as in Chromium).
fn opaque_origin(page: &PageDom) -> bool {
    page.has_opaque_origin() || page.main_world_realm().and_then(|r| r.origin()).is_some_and(|o| o == "null")
}

/// The document's service worker client id (Web Locks' `clientId`), or `None` when it is never a
/// client (an opaque origin).
pub(crate) fn client_id(page: &Rc<PageDom>) -> Option<String> {
    client_of(page).map(|c| c.id.clone())
}

/// The page's client, made on first need.
fn client_of(page: &Rc<PageDom>) -> Option<Rc<ClientInner>> {
    client_with_id(page, None)
}

/// The page's client; a new one takes `reserved` as its id (the
/// `resultingClientId` of the navigation that made the document).
fn client_with_id(page: &Rc<PageDom>, reserved: Option<String>) -> Option<Rc<ClientInner>> {
    client_made(page, reserved, None)
}

/// [`client_with_id`], with the URL the document was created with when that is known (it is
/// the client's creation URL, which later `pushState`s and fragment navigations do not change).
fn client_made(page: &Rc<PageDom>, reserved: Option<String>, created: Option<&pyro_net::Url>) -> Option<Rc<ClientInner>> {
    if let Some(client) = page.sw_client.borrow().clone() {
        return Some(client);
    }
    if opaque_origin(page) {
        return None;
    }
    let url = match created {
        Some(url) => url.clone(),
        None => page.document_url_parsed()?,
    };
    let origin = tuple_origin(&url)?;
    let client = Rc::new(ClientInner {
        id: reserved.unwrap_or_else(random_id),
        page: Rc::downgrade(page),
        url: url.to_string(),
        origin,
        partition: page.cookie_partition_key(),
        site: page.cookie_site_context(),
        nested: page.is_nested_frame(),
        controller: RefCell::new(None),
    });
    *page.sw_client.borrow_mut() = Some(client.clone());
    let context = page.context_services();
    let mut clients = context.service_workers.clients.borrow_mut();
    clients.retain(|c| c.strong_count() > 0);
    clients.push(Rc::downgrade(&client));
    Some(client)
}

fn set_controller(client: &ClientInner, version: Option<Rc<Version>>) {
    let controlled = version.is_some();
    *client.controller.borrow_mut() = version;
    if let Some(page) = client.page.upgrade() {
        page.sw_controlled.set(controlled);
    }
}

/// A new document of a context with registrations: in scope of one with an
/// active worker, it is controlled by that worker. One emptiness test otherwise.
pub(crate) fn document_created(page: &Rc<PageDom>) {
    let context = page.context_services();
    let registry = &context.service_workers;
    let none_yet = registry.registrations.borrow().is_empty();
    let reserved = if none_yet { None } else { page.request_context().frame_id.and_then(|f| registry.take_reserved(&f)) };
    let Some(url) = page.document_url_parsed() else { return };
    if !matches!(url.scheme(), "http" | "https") {
        return;
    }
    let active = if none_yet {
        None
    } else {
        registry
            .matching(url.as_str(), page.cookie_partition_key().as_deref(), page.cookie_site_context())
            .and_then(|r| r.active.borrow().clone())
    };
    let Some(active) = active else {
        // Not controlled, but still a client: made when the clients are first asked for.
        let mut unregistered = registry.unregistered.borrow_mut();
        if unregistered.len() >= 256 {
            unregistered.retain(|(page, _)| page.strong_count() > 0);
        }
        unregistered.push((Rc::downgrade(page), url));
        return;
    };
    if let Some(client) = client_with_id(page, reserved) {
        set_controller(&client, Some(active));
    }
}

/// `PageDom::dispose`: the document stops being a client. A registration that
/// was unregistered goes once none of its versions controls a client.
pub(crate) fn client_gone(page: &PageDom) {
    let Some(client) = page.sw_client.take() else { return };
    page.sw_controlled.set(false);
    let controller = client.controller.take();
    drop(client);
    let Some(registration) = controller.and_then(|v| v.registration.upgrade()) else { return };
    let controls = [&registration.installing, &registration.waiting, &registration.active]
        .iter()
        .any(|slot| slot.borrow().as_ref().is_some_and(|v| v.controls_any()));
    if registration.uninstalling.get() {
        if !controls {
            registration.clear();
        }
    } else if registration.waiting.borrow().is_some() {
        // "Try activate": the waiting worker may have been waiting for this client.
        try_activate(&registration);
    }
}

/// `fetch()` and asynchronous XHR: the controller's `fetch` event, or the network.
pub(crate) fn route(page: &PageDom, net: &NetClient, id: RequestId, request: Request, sink: ResponseSink) {
    // A controlled worker's thread cannot hold the version: its controller's id and the browser
    // thread's poster take the request there.
    let link = page.context_services().service_workers.remote.borrow().clone();
    if let Some(link) = link {
        let version = link.version.load(Ordering::Acquire);
        if version != 0 && !request.skip_interception && matches!(request.url.scheme(), "http" | "https") && !is_embedded_content(&request) {
            let (client_id, net) = (link.client_id.clone(), net.clone());
            (link.poster)(Box::new(move || remote_route(version, client_id, net, id, request, sink)));
            return;
        }
        net.fetch_with_id(id, request, sink);
        return;
    }
    match subresource_route(page, &request) {
        Some(route) => route.dispatch(net, id, request, sink),
        None => net.fetch_with_id(id, request, sink),
    }
}

/// A controlled worker's request, on the browser thread: the controller's `fetch` event, or the
/// network when the version is gone.
fn remote_route(version: u64, client_id: String, net: NetClient, id: RequestId, request: Request, sink: ResponseSink) {
    let version = VERSIONS.with(|v| v.borrow().get(&version).and_then(Weak::upgrade)).filter(|v| v.state.get() != State::Redundant);
    match version {
        Some(version) => LoadRoute { version, client_id, resulting_client_id: String::new(), preload: None }
            .dispatch(&net, id, request, sink),
        None => net.fetch_with_id(id, request, sink),
    }
}

/// A request on its way to a service worker's `fetch` event (see
/// [`navigation_route`] and [`subresource_route`]).
pub struct LoadRoute {
    version: Rc<Version>,
    client_id: String,
    resulting_client_id: String,
    /// A navigation of a registration with navigation preload enabled: the value of the
    /// `Service-Worker-Navigation-Preload` header its preload request carries.
    preload: Option<String>,
}

impl LoadRoute {
    /// Fires the `fetch` event; without `respondWith` (or when the worker is
    /// gone before it ran) the request goes on to `net` with `sink`.
    pub fn dispatch(self, net: &NetClient, id: RequestId, request: Request, sink: ResponseSink) {
        // Resource and Navigation Timing: the response's `workerStart` is when the worker was asked.
        let worker_start = std::time::Instant::now();
        let sink: ResponseSink = {
            let mut inner = sink;
            Box::new(move |event| match event {
                ResponseEvent::Response(mut head) => {
                    head.timing.worker_start = Some(worker_start);
                    inner(ResponseEvent::Response(head));
                }
                other => inner(other),
            })
        };
        // CSP: the response, the worker's included (`respondWith` of another URL's response),
        // is checked against the request's policy by its URL.
        let mut sink = match &request.csp {
            Some(csp) => pyro_net::csp::guard_sink(csp.clone(), sink),
            None => sink,
        };
        // A request its initiator's check blocked never reaches the worker.
        if request.csp.as_ref().is_some_and(|csp| csp.blocks_request()) {
            sink(ResponseEvent::Failed(pyro_net::NetError::Blocked(pyro_net::csp::BLOCKED_BY_CSP.into())));
            return;
        }
        let job = FetchJob {
            id,
            request: Some(request),
            sink: Some(sink),
            net: net.clone(),
            client_id: self.client_id,
            resulting_client_id: self.resulting_client_id,
            dispatched: false,
            observed: false,
            head_sent: false,
            sent: 0,
            race: None,
            preload: self.preload,
        };
        // A worker that is gone drops the job, which then goes to the network.
        self.version.run(move |s, _| crate::service_worker_scope::fetch(s, job));
    }
}

impl LoadRoute {
    /// The `resultingClientId` of this navigation's `fetch` event.
    pub fn resulting_client(&self) -> &str {
        &self.resulting_client_id
    }

    /// A redirect hop of a navigation within the origin of the hop before it creates the same
    /// client: its `resultingClientId` is `id`, and the document that commits is that client.
    pub fn keep_resulting_client(&mut self, services: &ContextServices, id: &str, frame: Option<&str>) {
        self.resulting_client_id = id.to_owned();
        if let Some(frame) = frame {
            services.service_workers.reserve_as(frame, id);
        }
    }
}

impl LoadRoute {
    /// The route of a speculation-rules prefetch: its `fetch` event has the document that asked
    /// as `clientId` and no `resultingClientId` (the prefetch creates no client).
    pub fn for_prefetch(mut self, initiator: &Rc<PageDom>) -> Self {
        self.client_id = client_id(initiator).unwrap_or_default();
        self.resulting_client_id = String::new();
        self
    }
}

/// The scopes of the context's registrations with an active worker that a navigation to a URL
/// of their own site reaches (see `PrefetchRedirectStop`), for the redirects of a prefetch.
pub fn prefetch_redirect_stop(services: &ContextServices) -> pyro_net::PrefetchRedirectStop {
    let registrations = services.service_workers.registrations.borrow();
    let scopes = registrations
        .iter()
        .filter(|r| !r.uninstalling.get() && r.site == SiteContext::SameSite && r.active.borrow().is_some())
        .map(|r| (r.partition.clone(), r.scope.clone()))
        .collect();
    pyro_net::PrefetchRedirectStop { scopes, navigation: false }
}

/// The scopes of the context's registrations with an active worker, for the redirects of a
/// navigation that no worker handled so far: a hop into one of them goes back to the loader
/// (`PrefetchRedirectStop::navigation`), which asks that registration's worker for the next hop.
pub fn navigation_redirect_stop(services: &ContextServices) -> pyro_net::PrefetchRedirectStop {
    let registrations = services.service_workers.registrations.borrow();
    let scopes = registrations
        .iter()
        .filter(|r| !r.uninstalling.get() && r.active.borrow().is_some())
        .map(|r| (r.partition.clone(), r.scope.clone()))
        .collect();
    pyro_net::PrefetchRedirectStop { scopes, navigation: true }
}

/// A request of an `<embed>` or `<object>` (their content, a document included): the browser
/// loads it without the page's service worker (Chrome's "embed"/"object" request contexts are
/// not intercepted).
fn is_embedded_content(request: &Request) -> bool {
    matches!(request.destination, Destination::Embed | Destination::Object)
}

/// A request `document` makes (its `fetch()`, XHR, scripts, stylesheets,
/// images, ...): its controller's route, if any. One flag test for a
/// document that no worker controls.
pub fn subresource_route(document: &PageDom, request: &Request) -> Option<LoadRoute> {
    if !document.sw_controlled.get()
        || request.skip_interception
        || is_embedded_content(request)
        || document.bypass_service_worker()
        || !matches!(request.url.scheme(), "http" | "https")
    {
        return None;
    }
    let client = document.sw_client.borrow();
    let client = client.as_ref()?;
    let version = client.controller.borrow().clone()?;
    if version.state.get() == State::Redundant {
        return None;
    }
    Some(LoadRoute { version, client_id: client.id.clone(), resulting_client_id: String::new(), preload: None })
}

/// A navigation request of a frame of `services`' context (mode `navigate`,
/// destination `document` or `iframe`): the route through the active worker
/// of the registration its URL matches, if any. The id of the client the
/// navigation creates is reserved under the request's frame id, so the new
/// document is the client the worker saw as `resultingClientId`. Callers test
/// [`ContextServices::may_route_loads`] first.
pub fn navigation_route(services: &ContextServices, request: &Request) -> Option<LoadRoute> {
    if request.skip_interception || is_embedded_content(request) || !matches!(request.url.scheme(), "http" | "https") {
        return None;
    }
    let registry = &services.service_workers;
    let partition = if request.destination == Destination::Document {
        pyro_net::cookies::partition_key(&request.url)
    } else { request.top_level_site.clone() };
    let site = if request.destination == Destination::Document
        || (!request.cross_site_ancestor && partition.is_some() && partition == pyro_net::cookies::partition_key(&request.url)) {
        SiteContext::SameSite
    } else {
        SiteContext::CrossSite
    };
    let registration = registry.matching(request.url.as_str(), partition.as_deref(), site)?;
    let version = registration.active.borrow().clone()?;
    if version.state.get() == State::Redundant {
        return None;
    }
    // "Handle Fetch": a GET navigation of a registration with navigation preload enabled.
    let preload = (registration.preload_enabled.get() && request.method == pyro_net::http::Method::GET && request.mode == RequestMode::Navigate)
        .then(|| registration.preload_header.borrow().clone());
    let resulting_client_id = match request.frame_id.as_deref() {
        Some(frame) => registry.reserve(frame),
        None => random_id(),
    };
    Some(LoadRoute { version, client_id: String::new(), resulting_client_id, preload })
}

/// Runs on the browser thread, as a task of its loop.
pub(crate) fn deliver(version: u64, up: SwUp) {
    let Some(version) = VERSIONS.with(|v| v.borrow().get(&version).and_then(Weak::upgrade)) else { return };
    match up {
        SwUp::Evaluated(ok) => {
            if let Some(next) = version.evaluated.take() {
                next(ok);
            }
        }
        SwUp::Done { kind, ok } => lifecycle_done(&version, kind, ok),
        SwUp::Query { id, kind } => {
            let result = query(&version, kind);
            version.run(move |s, _| crate::service_worker_scope::reply(s, id, result));
        }
        SwUp::PostToClient { client, message, queued } => post_to_client(&version, &client, message, queued),
        SwUp::CancelFetch(key) => version.run(move |s, _| crate::service_worker_scope::fetch_cancelled(s, key)),
        SwUp::PreloadDone(key) => version.run(move |s, _| crate::service_worker_scope::preload_ready(s, key)),
    }
}

fn reply_error(name: &str, message: &str) -> Result<String, (String, String)> {
    Err((name.to_owned(), message.to_owned()))
}

fn client_json(client: &ClientInner) -> String {
    let page = client.page.upgrade();
    let hidden = page.as_ref().is_some_and(|p| p.is_hidden());
    // The document is the selected keyboard frame (`frame.focus()` selects it), in a visible window.
    let focused = !hidden && page.as_ref().is_some_and(|p| p.hooks().is_none_or(|h| h.is_focused()));
    format!(
        "{{\"id\":{},\"url\":{},\"type\":\"window\",\"frameType\":\"{}\",\"visibilityState\":\"{}\",\"focused\":{}}}",
        quote(&client.id),
        quote(&client.url),
        if client.nested { "nested" } else { "top-level" },
        if hidden { "hidden" } else { "visible" },
        focused,
    )
}

fn query(version: &Rc<Version>, kind: QueryKind) -> Result<String, (String, String)> {
    let Some(registration) = version.registration.upgrade() else {
        return reply_error("InvalidStateError", "The service worker is gone.");
    };
    let Some(context) = registration.context.upgrade() else {
        return reply_error("InvalidStateError", "The browser context is gone.");
    };
    let registry = &context.service_workers;
    match kind {
        QueryKind::SkipWaiting => {
            version.skip_waiting.set(true);
            if registration.waiting.borrow().as_ref().is_some_and(|w| Rc::ptr_eq(w, version)) {
                try_activate(&registration);
            }
            Ok(String::new())
        }
        QueryKind::Claim => {
            let is_active = registration.active.borrow().as_ref().is_some_and(|a| Rc::ptr_eq(a, version));
            if !is_active || !matches!(version.state.get(), State::Activating | State::Activated) {
                return reply_error("InvalidStateError", "Only the active worker can claim clients.");
            }
            let mut losers: Vec<Rc<Registration>> = Vec::new();
            for client in registry.live_clients() {
                if !registration.shares_storage(&client) {
                    continue;
                }
                if !registry.matching(&client.url, client.partition.as_deref(), client.site).is_some_and(|r| Rc::ptr_eq(&r, &registration)) {
                    continue;
                }
                if client.controller.borrow().as_ref().is_some_and(|c| Rc::ptr_eq(c, version)) {
                    continue;
                }
                // The registration that controlled it until now (Clients.claim step 3.2.3).
                if let Some(old) = client.controller.borrow().as_ref().and_then(|v| v.registration.upgrade()) {
                    if !losers.iter().any(|r| Rc::ptr_eq(r, &old)) {
                        losers.push(old);
                    }
                }
                set_controller(&client, Some(version.clone()));
                if let Some(page) = client.page.upgrade() {
                    let json = version.json();
                    post_to_module(&page, false, move |s, glue| {
                        call(s, glue, "controller", &[s.string(&json), s.boolean(true)]);
                    });
                }
            }
            // Its dedicated and shared workers whose script URL it covers, too.
            for worker in registry.live_worker_clients() {
                if worker.origin != registration.origin || worker.partition != registration.partition || worker.site != registration.site {
                    continue;
                }
                if !registry.matching(&worker.scope_url, worker.partition.as_deref(), worker.site).is_some_and(|r| Rc::ptr_eq(&r, &registration)) {
                    continue;
                }
                if worker.controller.borrow().as_ref().is_some_and(|c| Rc::ptr_eq(c, version)) {
                    continue;
                }
                if let Some(old) = worker.controller.borrow().as_ref().and_then(|v| v.registration.upgrade()) {
                    if !losers.iter().any(|r| Rc::ptr_eq(r, &old)) {
                        losers.push(old);
                    }
                }
                worker.set_controller(Some(version.clone()));
            }
            // A registration that lost a controllee may have a waiting worker that was waiting for it.
            for old in losers {
                if old.waiting.borrow().is_some() {
                    try_activate(&old);
                }
            }
            Ok(String::new())
        }
        QueryKind::MatchAll { include_uncontrolled, kind } => {
            let mut out = String::from("[");
            if matches!(kind.as_str(), "window" | "all") {
                for client in registry.live_clients() {
                    if !registration.shares_storage(&client) {
                        continue;
                    }
                    let controlled = client.controller.borrow().as_ref().is_some_and(|c| Rc::ptr_eq(c, version));
                    if !include_uncontrolled && !controlled {
                        continue;
                    }
                    if out.len() > 1 {
                        out.push(',');
                    }
                    out.push_str(&client_json(&client));
                }
            }
            if matches!(kind.as_str(), "worker" | "sharedworker" | "all") {
                for worker in registry.live_worker_clients() {
                    if (kind != "all" && worker.kind != kind)
                        || worker.origin != registration.origin
                        || worker.partition != registration.partition
                        || worker.site != registration.site
                    {
                        continue;
                    }
                    let controlled = worker.controller.borrow().as_ref().is_some_and(|c| Rc::ptr_eq(c, version));
                    if !include_uncontrolled && !controlled {
                        continue;
                    }
                    if out.len() > 1 {
                        out.push(',');
                    }
                    out.push_str(&worker.json());
                }
            }
            out.push(']');
            Ok(out)
        }
        QueryKind::Get(id) => Ok(registry
            .live_clients()
            .iter()
            .find(|c| c.id == id && registration.shares_storage(c))
            .map(|c| client_json(c))
            .or_else(|| {
                registry
                    .live_worker_clients()
                    .iter()
                    .find(|w| w.id == id && w.origin == registration.origin && w.partition == registration.partition && w.site == registration.site)
                    .map(|w| w.json())
            })
            .unwrap_or_else(|| "null".to_owned())),
        QueryKind::Unregister => Ok(unregister(&registration).to_string()),
        // A worker's own update() changes nothing: its script is what it runs.
        QueryKind::Update => Ok(String::new()),
        QueryKind::Preload { op, value } => preload_op(&registration, &op, &value),
    }
}

/// `NavigationPreloadManager` of `registration` (https://w3c.github.io/ServiceWorker/#navigation-preload-manager):
/// `getState` answers the state as JSON; `enable`, `disable` and `setHeaderValue` (`value`: a
/// ByteString, one char per byte) change it and need an active worker ("InvalidStateError").
fn preload_op(registration: &Registration, op: &str, value: &str) -> Result<String, (String, String)> {
    if op == "getState" {
        return Ok(format!(
            "{{\"enabled\":{},\"headerValue\":{}}}",
            registration.preload_enabled.get(),
            quote(&registration.preload_header.borrow())
        ));
    }
    if registration.active.borrow().is_none() {
        return reply_error("InvalidStateError", "The registration has no active worker.");
    }
    match op {
        "enable" => registration.preload_enabled.set(true),
        "disable" => registration.preload_enabled.set(false),
        "setHeaderValue" => *registration.preload_header.borrow_mut() = value.to_owned(),
        other => return reply_error("TypeError", &format!("unknown navigation preload operation {other}")),
    }
    Ok(String::new())
}

fn post_to_client(
    version: &Rc<Version>,
    client: &str,
    message: WireMessage,
    queued: crate::service_worker_scope::Queued,
) {
    let Some(registration) = version.registration.upgrade() else { return };
    let Some(context) = registration.context.upgrade() else { return };
    let Some(client) = context.service_workers.live_clients().into_iter().find(|c| c.id == client) else { return };
    if !registration.shares_storage(&client) {
        return;
    }
    let Some(page) = client.page.upgrade() else { return };
    let (json, origin) = (version.json(), registration.origin.clone());
    post_to_module(&page, true, move |s, glue| {
        drop(queued);
        let (ty, payload) = match crate::structured_clone::decode_with_ports(s, &message) {
            Ok(payload) => ("message", Some(payload)),
            Err(JsError::Terminated) => return,
            Err(_) => ("messageerror", None),
        };
        let Ok(page) = crate::target::page_of(s) else { return };
        let event = crate::event::EventCore::trusted(&page, crate::event::EventInterface::MessageEvent, ty, false, false);
        {
            let mut fields = event.fields.borrow_mut();
            fields.set_sender_origin(origin);
            if let Some((data, ports)) = payload {
                fields.message_data = Some(Persistent::new(s, data));
                fields.message_ports = ports.map(|p| Persistent::new(s, p.value()));
            }
        }
        match crate::event::wrap_event(s, &event) {
            Ok(object) => call(s, glue, "message", &[s.string(&json), object.value()]),
            Err(error) => s.report_exception(&error),
        }
    });
}

fn lifecycle_done(version: &Rc<Version>, kind: Lifecycle, ok: bool) {
    let Some(registration) = version.registration.upgrade() else { return };
    match kind {
        Lifecycle::Install => {
            if !registration.installing.borrow().as_ref().is_some_and(|v| Rc::ptr_eq(v, version)) {
                return;
            }
            registration.installing.take();
            if !ok {
                version.redundant();
                registration.notify();
                end_update(&registration);
                return;
            }
            if let Some(old) = registration.waiting.replace(Some(version.clone())) {
                old.redundant();
            }
            version.set_state(State::Installed);
            registration.notify();
            try_activate(&registration);
            end_update(&registration);
        }
        Lifecycle::Activate => {
            if !registration.active.borrow().as_ref().is_some_and(|v| Rc::ptr_eq(v, version)) {
                return;
            }
            version.set_state(State::Activated);
            registration.cookies.activated();
            registration.notify();
        }
    }
}

fn try_activate(registration: &Rc<Registration>) {
    let Some(waiting) = registration.waiting.borrow().clone() else { return };
    if let Some(active) = registration.active.borrow().clone() {
        if active.state.get() == State::Activating {
            return;
        }
        if !waiting.skip_waiting.get() && active.controls_any() {
            return;
        }
    }
    let Some(context) = registration.context.upgrade() else { return };
    registration.waiting.take();
    let old = registration.active.replace(Some(waiting.clone()));
    waiting.state.set(State::Activating);
    if let Some(handle) = waiting.thread.borrow().clone() { registration.cookies.activating(waiting.id, handle); }
    // The registrations' clients see the new active worker before `controllerchange` fires
    // (Activate: "set registration's active worker", then "Handle ... controllerchange").
    registration.notify();
    if let Some(old) = &old {
        // Its clients move to the new worker.
        for client in context.service_workers.live_clients() {
            if client.controller.borrow().as_ref().is_some_and(|c| Rc::ptr_eq(c, old)) {
                set_controller(&client, Some(waiting.clone()));
                if let Some(page) = client.page.upgrade() {
                    let json = waiting.json();
                    post_to_module(&page, false, move |s, glue| {
                        call(s, glue, "controller", &[s.string(&json), s.boolean(true)]);
                    });
                }
            }
        }
        for worker in context.service_workers.live_worker_clients() {
            if worker.controller.borrow().as_ref().is_some_and(|c| Rc::ptr_eq(c, old)) {
                worker.set_controller(Some(waiting.clone()));
            }
        }
        old.redundant();
        registration.notify();
    }
    waiting.run(|s, _| crate::service_worker_scope::lifecycle(s, Lifecycle::Activate));
}

/// The update job ended: the next queued register job runs; with none, a
/// registration that holds no version goes away.
fn end_update(registration: &Rc<Registration>) {
    registration.updating.set(false);
    loop {
        let Some((page, url, settle)) = registration.jobs.borrow_mut().pop_front() else { break };
        let Some(page) = page.upgrade().filter(|p| !p.is_disposed()) else { continue };
        if registration.newest().is_some_and(|v| v.script_url == url.as_str()) {
            settle.registration(registration);
            continue;
        }
        update(&page, registration, url, settle);
        if registration.updating.get() {
            return;
        }
    }
    forget_if_empty(registration);
}

/// A registration whose every version failed goes away.
fn forget_if_empty(registration: &Rc<Registration>) {
    if registration.newest().is_none() {
        if let Some(context) = registration.context.upgrade() {
            context.service_workers.remove(registration);
        }
    }
}

fn unregister(registration: &Rc<Registration>) -> bool {
    if registration.uninstalling.replace(true) {
        return false;
    }
    let Some(context) = registration.context.upgrade() else { return false };
    context.service_workers.remove(registration);
    let controls = [&registration.installing, &registration.waiting, &registration.active]
        .iter()
        .any(|slot| slot.borrow().as_ref().is_some_and(|v| v.controls_any()));
    if !controls {
        registration.clear();
        registration.notify();
    }
    true
}

// ---------------------------------------------------------------------------
// Register and update.

/// A registration's operation from a document that is no longer fully active (its frame was
/// removed or navigated) would settle in a task that never runs: it rejects at once instead, with
/// Chromium's InvalidStateError.
fn not_fully_active(page: &PageDom, what: &str) -> JsResult<()> {
    if page.is_fully_active() {
        return Ok(());
    }
    Err(JsError::dom("InvalidStateError", &format!("Failed to {what}: No associated provider is available.")))
}

/// Settles one page-side request (`register`, `update`, ...) in a task.
#[derive(Clone)]
struct Settle {
    page: Weak<PageDom>,
    id: u64,
    /// The realm whose module made the promise: an isolated world's calls
    /// settle there, not in the main world.
    realm: WeakRealm,
}

impl Settle {
    fn done(&self, result: Result<String, (String, String)>) {
        let Some(page) = self.page.upgrade() else { return };
        let id = self.id;
        let realm = self.realm.clone();
        let weak = Rc::downgrade(&page);
        page.post_task(Box::new(move || {
            let Some(page) = weak.upgrade().filter(|p| !p.is_disposed()) else { return };
            let Some(realm) = realm.upgrade() else { return };
            let Ok(s) = realm.scope() else { return };
            let Some(glue) = existing(&s, &page).ok().flatten() else { return };
            settle(&s, glue, id, result);
        }));
    }

    fn registration(&self, registration: &Registration) {
        let matches = self.page.upgrade().and_then(|p| p.sw_client.borrow().clone()).is_some_and(|c| {
            registration
                .context
                .upgrade()
                .and_then(|ctx| ctx.service_workers.matching(&c.url, c.partition.as_deref(), c.site))
                .is_some_and(|r| r.id == registration.id)
        });
        self.done(Ok(format!("{{\"reg\":{}}}", registration.json(matches))));
    }
}

/// Settles promise `id` of a realm's module (`settle` of its glue).
fn settle(s: &Scope<'_>, glue: Object<'_>, id: u64, result: Result<String, (String, String)>) {
    match result {
        Ok(json) => call(s, glue, "settle", &[s.number(id as f64), s.boolean(true), s.string(&json)]),
        Err((name, message)) => call(
            s,
            glue,
            "settle",
            &[s.number(id as f64), s.boolean(false), s.undefined(), s.string(&name), s.string(&message)],
        ),
    }
}

fn type_error(message: String) -> JsError {
    JsError::Type(message.into())
}

fn has_encoded_slash(url: &pyro_net::Url) -> bool {
    let path = url.path().to_ascii_lowercase();
    path.contains("%2f") || path.contains("%5c")
}

/// `register(scriptURL, options)`: validated now (a throw rejects the
/// promise), then the update job runs and settles request `id`.
fn register(
    s: &Scope<'_>,
    page: &Rc<PageDom>,
    id: u64,
    script: &str,
    scope: Option<String>,
    module: bool,
    update_via_cache: String,
) -> JsResult<()> {
    let prefix = "Failed to register a ServiceWorker";
    let client = client_of(page)
        .ok_or_else(|| JsError::dom("SecurityError", format!("{prefix}: The document is in an invalid state.")))?;
    let base = page.dom.borrow().document_base_url(page.document()).ok().map(str::to_owned);
    let base = base.and_then(|b| pyro_net::Url::parse(&b).ok());
    let parse = |input: &str, base: Option<&pyro_net::Url>| pyro_net::Url::options().base_url(base).parse(input);
    let mut script_url =
        parse(script, base.as_ref()).map_err(|_| type_error(format!("{prefix}: The URL '{script}' is invalid.")))?;
    // Register: the script URL's fragment is dropped (the worker's `scriptURL` has none).
    script_url.set_fragment(None);
    if !matches!(script_url.scheme(), "http" | "https") {
        return Err(type_error(format!("{prefix}: The URL protocol of the script ('{script_url}') is not supported.")));
    }
    if has_encoded_slash(&script_url) {
        return Err(type_error(format!(
            "{prefix}: The provided scriptURL ('{script_url}') includes a disallowed escape character."
        )));
    }
    let mut scope_url = match &scope {
        Some(scope) => {
            parse(scope, base.as_ref()).map_err(|_| type_error(format!("{prefix}: The URL '{scope}' is invalid.")))?
        }
        None => parse("./", Some(&script_url)).map_err(|_| type_error(format!("{prefix}: invalid scope")))?,
    };
    scope_url.set_fragment(None);
    if !matches!(scope_url.scheme(), "http" | "https") {
        return Err(type_error(format!("{prefix}: The URL protocol of the scope ('{scope_url}') is not supported.")));
    }
    if has_encoded_slash(&scope_url) {
        return Err(type_error(format!(
            "{prefix}: The provided scope ('{scope_url}') includes a disallowed escape character."
        )));
    }
    if tuple_origin(&script_url).as_deref() != Some(client.origin.as_str()) {
        return Err(JsError::dom(
            "SecurityError",
            format!(
                "{prefix}: The origin of the provided scriptURL ('{}') does not match the current origin ('{}').",
                script_url.origin().ascii_serialization(),
                client.origin
            ),
        ));
    }
    if tuple_origin(&scope_url).as_deref() != Some(client.origin.as_str()) {
        return Err(JsError::dom(
            "SecurityError",
            format!(
                "{prefix}: The origin of the provided scope ('{}') does not match the current origin ('{}').",
                scope_url.origin().ascii_serialization(),
                client.origin
            ),
        ));
    }
    if crate::worker::is_held() {
        return Err(JsError::dom(
            "NotSupportedError",
            format!("{prefix}: Service workers cannot start in a prepared template."),
        ));
    }
    let settle = Settle { page: Rc::downgrade(page), id, realm: s.realm().downgrade() };
    let context = page.context_services();
    let registry = &context.service_workers;
    let scope = scope_url.to_string();
    let existing = registry.registrations.borrow().iter()
        .find(|r| r.scope == scope && r.shares_storage(&client) && !r.uninstalling.get()).cloned();
    let registration = match existing {
        Some(registration) => {
            let same = registration.newest().is_some_and(|v| v.script_url == script_url.as_str())
                && registration.update_via_cache == update_via_cache;
            if registration.updating.get() {
                // Jobs on one registration run one after another.
                if registration.jobs.borrow().len() >= MAX_QUEUED_JOBS {
                    return Err(JsError::dom(
                        "QuotaExceededError",
                        format!("{prefix}: Too many pending registrations."),
                    ));
                }
                registration.jobs.borrow_mut().push_back((Rc::downgrade(page), script_url, settle));
                return Ok(());
            }
            if same {
                settle.registration(&registration);
                return Ok(());
            }
            registration
        }
        None => {
            if registry.registrations.borrow().len() >= MAX_REGISTRATIONS {
                return Err(JsError::dom("QuotaExceededError", format!("{prefix}: Too many registrations.")));
            }
            let registration = Rc::new(Registration {
                id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
                scope,
                origin: client.origin.clone(),
                partition: client.partition.clone(),
                site: client.site,
                cookies: crate::cookie_store::subscriptions::Subscriptions::new(
                    scope_url.clone(), client.partition.clone(), client.site, page.net.borrow().as_ref().map(|net| net.cookies()),
                ),
                update_via_cache,
                module: Cell::new(module),
                installing: RefCell::new(None),
                waiting: RefCell::new(None),
                active: RefCell::new(None),
                context: Rc::downgrade(&context),
                uninstalling: Cell::new(false),
                seq: Cell::new(0),
                updating: Cell::new(false),
                jobs: RefCell::new(VecDeque::new()),
                evaluating: RefCell::new(None),
                preload_enabled: Cell::new(false),
                preload_header: RefCell::new("true".to_owned()),
            });
            registry.registrations.borrow_mut().push(registration.clone());
            registration
        }
    };
    registration.module.set(module);
    update(page, &registration, script_url, settle);
    Ok(())
}

/// The Update algorithm: fetches the script on `page`'s network and, when it
/// is new, starts a version and installs it.
fn update(page: &Rc<PageDom>, registration: &Rc<Registration>, script_url: pyro_net::Url, settle: Settle) {
    let Some(net) = page.net.borrow().clone() else {
        settle.done(reply_error("InvalidStateError", "The page has no network."));
        end_update(registration);
        return;
    };
    // Not the page's poster: the job outlives a navigation of the page.
    let Some(poster) = page.thread_poster() else {
        settle.done(reply_error("InvalidStateError", "The page cannot receive tasks."));
        end_update(registration);
        return;
    };
    registration.updating.set(true);
    let mut request = Request::get(script_url.clone(), Destination::ServiceWorker);
    request.mode = RequestMode::SameOrigin;
    request.credentials = pyro_net::Credentials::SameOrigin;
    request.origin = Some(registration.origin.clone());
    request.referrer = page.document_url_parsed();
    request.referrer_policy = page.referrer_policy();
    request.frame_id = page.request_context().frame_id;
    request.max_body = Some(MAX_SCRIPT_BYTES);
    // The creator's `Connection-Allowlist` governs the script's fetch, as a navigation's.
    request.csp = crate::csp::allowlist_guard(page, &script_url);
    // A redirect would take the body from a URL the max scope was not computed for.
    request.redirect = pyro_net::RedirectMode::Error;
    request.cache =
        if registration.update_via_cache == "all" { pyro_net::CacheMode::Default } else { pyro_net::CacheMode::NoCache };
    request.headers.insert("service-worker", pyro_net::http::HeaderValue::from_static("script"));
    page.apply_request_defaults(&mut request);
    // The script is fetched by the browser, not a document: no UA client hints go with it (an empty
    // set is "overridden without metadata": the low-entropy ones are not added) unless the page has
    // its own metadata override. Its headers are what the update tests read back from the worker.
    if request.defaults.client_hints.is_none() {
        request.defaults.client_hints = Some(Arc::new(Vec::new()));
    }
    request.top_level_site = registration.partition.clone();
    request.cross_site_ancestor = registration.site == SiteContext::CrossSite;
    #[derive(Default)]
    struct Fetched {
        head: Option<pyro_net::ResponseHead>,
        body: Vec<u8>,
        done: bool,
    }
    let fetched = Arc::new(Mutex::new(Fetched::default()));
    let (page_weak, registration_weak) = (Rc::downgrade(page), Rc::downgrade(registration));
    // The continuation runs on this thread; the sink, on the I/O thread.
    let finish: Finisher = Rc::new(RefCell::new(Some(Box::new(move |result| {
        let Some(registration) = registration_weak.upgrade() else { return };
        match page_weak.upgrade().filter(|p| !p.is_disposed()) {
            Some(page) => fetched_script(&page, &registration, script_url, result, settle),
            // The page that started the job is gone: the job ends, and the next one runs.
            None => end_update(&registration),
        }
    }))));
    let token = FINISHERS.with(|f| {
        let mut f = f.borrow_mut();
        let token = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        f.insert(token, finish);
        token
    });
    let sink_state = fetched.clone();
    // Answers the job once; a sink dropped unanswered fails it.
    let mut report = Some(Report { poster, token, result: None });
    net.fetch(
        request,
        Box::new(move |event| {
            let result = {
                let mut state = sink_state.lock().unwrap_or_else(|e| e.into_inner());
                if state.done {
                    return;
                }
                match event {
                    ResponseEvent::Redirect { .. } => {
                        state.done = true;
                        Err("redirected".to_owned())
                    }
                    ResponseEvent::Response(head) => {
                        state.head = Some(head);
                        return;
                    }
                    ResponseEvent::Data(bytes) => {
                        state.body.extend_from_slice(&bytes);
                        return;
                    }
                    ResponseEvent::Finished { .. } => {
                        state.done = true;
                        match state.head.take() {
                            Some(head) => Ok((head, Bytes::from(std::mem::take(&mut state.body)))),
                            None => Err("no response".to_owned()),
                        }
                    }
                    ResponseEvent::Failed(error) => {
                        state.done = true;
                        // The redirect mode "error": the script is behind a redirect.
                        Err(if matches!(error, pyro_net::NetError::RedirectNotAllowed) { "redirected".to_owned() } else { error.to_string() })
                    }
                }
            };
            if let Some(mut report) = report.take() {
                report.result = Some(result);
            }
        }),
    );
}

/// The end of a script fetch, posted to the browser thread when dropped.
struct Report {
    poster: crate::page::RemotePoster,
    token: u64,
    result: Option<Result<(pyro_net::ResponseHead, Bytes), String>>,
}

impl Drop for Report {
    fn drop(&mut self) {
        let result = self.result.take().unwrap_or_else(|| Err("cancelled".to_owned()));
        let token = self.token;
        (self.poster)(Box::new(move || {
            let finish = FINISHERS.with(|f| f.borrow_mut().remove(&token));
            if let Some(finish) = finish.and_then(|f| f.take()) {
                finish(result);
            }
        }));
    }
}

type Finisher = Rc<RefCell<Option<Box<dyn FnOnce(Result<(pyro_net::ResponseHead, Bytes), String>)>>>>;

thread_local! {
    /// Script fetches waiting for their answer on this thread.
    static FINISHERS: RefCell<FxHashMap<u64, Finisher>> = RefCell::default();
}

fn is_javascript_mime(mime: &str) -> bool {
    matches!(
        mime,
        "text/javascript"
            | "application/javascript"
            | "application/x-javascript"
            | "application/ecmascript"
            | "application/x-ecmascript"
            | "text/ecmascript"
            | "text/jscript"
            | "text/livescript"
            | "text/x-ecmascript"
            | "text/x-javascript"
            | "text/javascript1.0"
            | "text/javascript1.1"
            | "text/javascript1.2"
            | "text/javascript1.3"
            | "text/javascript1.4"
            | "text/javascript1.5"
    )
}

/// The directory of `url`'s path: the default maximum scope.
fn directory(url: &pyro_net::Url) -> String {
    let path = url.path();
    match path.rfind('/') {
        Some(i) => path[..=i].to_owned(),
        None => "/".to_owned(),
    }
}

fn fetched_script(
    page: &Rc<PageDom>,
    registration: &Rc<Registration>,
    script_url: pyro_net::Url,
    result: Result<(pyro_net::ResponseHead, Bytes), String>,
    settle: Settle,
) {
    let prefix =
        format!("Failed to register a ServiceWorker for scope ('{}') with script ('{script_url}')", registration.scope);
    let fail = |error: Result<String, (String, String)>| {
        settle.done(error);
        end_update(registration);
    };
    let (head, body) = match result {
        Ok(found) => found,
        // Fetch with the redirect mode "error": no service worker script comes from a redirect.
        Err(error) if error == "redirected" => {
            return fail(reply_error(
                "SecurityError",
                &format!("{prefix}: The script resource is behind a redirect, which is disallowed."),
            ))
        }
        Err(_) => {
            return fail(reply_error(
                "TypeError",
                &format!("{prefix}: An unknown error occurred when fetching the script."),
            ))
        }
    };
    if !head.status.is_success() {
        return fail(reply_error(
            "TypeError",
            &format!(
                "{prefix}: A bad HTTP response code ({}) was received when fetching the script.",
                head.status.as_u16()
            ),
        ));
    }
    if !is_javascript_mime(&head.mime) {
        return fail(reply_error(
            "SecurityError",
            &format!("{prefix}: The script has an unsupported MIME type ('{}').", head.mime),
        ));
    }
    let max_scope = match head.headers.get("service-worker-allowed").and_then(|v| v.to_str().ok()) {
        Some(allowed) => match script_url.join(allowed) {
            // The header's value is parsed against the script's URL and must stay on its origin.
            Ok(url) if url.origin() != script_url.origin() => {
                return fail(reply_error(
                    "SecurityError",
                    &format!("{prefix}: The Service-Worker-Allowed header value ('{allowed}') is cross-origin to the script."),
                ))
            }
            Ok(url) => url.path().to_owned(),
            Err(_) => {
                return fail(reply_error(
                    "SecurityError",
                    &format!("{prefix}: The Service-Worker-Allowed header value ('{allowed}') is invalid."),
                ))
            }
        },
        None => directory(&script_url),
    };
    let scope_path = pyro_net::Url::parse(&registration.scope).map(|u| u.path().to_owned()).unwrap_or_default();
    if !scope_path.starts_with(&max_scope) {
        return fail(reply_error(
            "SecurityError",
            &format!("{prefix}: The path of the provided scope ('{scope_path}') is not under the max scope allowed ('{max_scope}'). Adjust the scope, move the Service Worker script, or use the Service-Worker-Allowed HTTP header to allow the scope."),
        ));
    }
    if let Some(newest) = registration.newest() {
        if newest.script_url == script_url.as_str() && newest.script == body {
            settle.registration(registration);
            end_update(registration);
            return;
        }
    }
    // The worker's own `Connection-Allowlist` is its script response's.
    let allowlist = {
        let header = |name: &str| {
            let values: Vec<&str> = head.headers.get_all(name).iter().filter_map(|v| v.to_str().ok()).collect();
            (!values.is_empty()).then(|| values.join(", "))
        };
        pyro_net::connection_allowlist::ConnectionPolicy::from_headers(
            header("connection-allowlist").as_deref(),
            header("connection-allowlist-report-only").as_deref(),
            &head.url,
        )
    };
    // The worker's own embedder policy is its script response's (HTML "run a worker"): its fetches
    // and the `Cache` it reads are subject to it.
    let embedder = crate::coep_report::obtain_embedder(&head.headers, &head.url);
    start_version(page, registration, script_url, body, allowlist, embedder, settle);
}

fn start_version(
    page: &Rc<PageDom>,
    registration: &Rc<Registration>,
    script_url: pyro_net::Url,
    script: Bytes,
    allowlist: Option<Arc<pyro_net::connection_allowlist::ConnectionPolicy>>,
    embedder: (pyro_net::EmbedderPolicy, pyro_net::EmbedderReporting),
    settle: Settle,
) {
    if VERSIONS.with(|v| v.borrow().len()) >= MAX_VERSIONS {
        settle.done(reply_error("QuotaExceededError", "Too many service workers are running."));
        end_update(registration);
        return;
    }
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let shared = crate::worker::new_shared();
    let service = ServiceStart {
        version: id,
        registration: registration.id,
        partition_key: registration.partition.clone(),
        cookie_site_context: registration.site,
        cookies: registration.cookies.clone(),
        scope: registration.scope.clone(),
        script_url: script_url.to_string(),
        update_via_cache: registration.update_via_cache.clone(),
    };
    let start = WorkerStart::for_service(
        id,
        page,
        script_url.clone(),
        registration.origin.clone(),
        shared.clone(),
        script.clone(),
        service,
        registration.module.get(),
    );
    let Some(mut start) = start else {
        settle.done(reply_error("InvalidStateError", "The page cannot start workers."));
        end_update(registration);
        return;
    };
    start.csp.inherited_allowlist = allowlist;
    (start.embedder_policy, start.embedder_reporting) = embedder;
    let version = Rc::new(Version {
        id,
        script_url: script_url.to_string(),
        script,
        state: Cell::new(State::Parsed),
        registration: Rc::downgrade(registration),
        thread: RefCell::new(None),
        shared,
        skip_waiting: Cell::new(false),
        evaluated: RefCell::new(None),
        queued: Arc::new(AtomicUsize::new(0)),
        announced: RefCell::new(None),
    });
    VERSIONS.with(|v| v.borrow_mut().insert(id, Rc::downgrade(&version)));
    // The registration holds it until the job decides, then its slots do.
    if let Some(old) = registration.evaluating.replace(Some(version.clone())) {
        old.redundant();
    }
    let (registration_weak, version_weak) = (Rc::downgrade(registration), Rc::downgrade(&version));
    *version.evaluated.borrow_mut() = Some(Box::new(move |ok| {
        let (Some(registration), Some(version)) = (registration_weak.upgrade(), version_weak.upgrade()) else {
            return;
        };
        if registration.evaluating.borrow().as_ref().is_some_and(|v| Rc::ptr_eq(v, &version)) {
            registration.evaluating.take();
        }
        if !ok {
            version.redundant();
            settle.done(reply_error(
                "TypeError",
                &format!("Failed to register a ServiceWorker for scope ('{}') with script ('{}'): ServiceWorker script evaluation failed", registration.scope, version.script_url),
            ));
            end_update(&registration);
            return;
        }
        install(&registration, &version, settle);
    }));
    match crate::worker_scope::spawn(start) {
        Ok((handle, thread, _early)) => {
            crate::worker::track_thread(thread);
            *version.thread.borrow_mut() = Some(handle);
            if let Some(hooks) = page.hooks() {
                *version.announced.borrow_mut() = Some(hooks.clone());
                hooks.worker_event(crate::worker::WorkerEvent::ServiceCreated(crate::worker::WorkerHandle::service(
                    id,
                    version.script_url.clone(),
                    handle_of(&version),
                )));
            }
        }
        Err(_) => {
            if let Some(next) = version.evaluated.take() {
                next(false);
            }
        }
    }
}

fn handle_of(version: &Version) -> Option<LoopHandle<WorkerLoop>> {
    version.thread.borrow().clone()
}

fn install(registration: &Rc<Registration>, version: &Rc<Version>, settle: Settle) {
    if let Some(old) = registration.installing.replace(Some(version.clone())) {
        old.redundant();
    }
    version.state.set(State::Installing);
    // Service Workers "Install": update the installing state, resolve the
    // register/update job, then queue updatefound on the client event loops.
    registration.bump();
    settle.registration(registration);
    registration.notify_clients(true);
    version.run(|s, _| crate::service_worker_scope::lifecycle(s, Lifecycle::Install));
}

// ---------------------------------------------------------------------------
// The page module (service_worker.js).

/// Registration `id` when it belongs to the page's origin.
fn same_origin_registration(page: &Rc<PageDom>, id: u64) -> Option<Rc<Registration>> {
    let client = client_of(page)?;
    page.context_services().service_workers.by_id(id).filter(|r| r.shares_storage(&client))
}

fn existing<'s>(s: &Scope<'s>, page: &Rc<PageDom>) -> JsResult<Option<Object<'s>>> {
    let holder = crate::worker_scope::module_holder(s, page)?;
    Ok(s.get_hidden(holder, KEY)?.and_then(|v| v.as_object()))
}

fn unavailable() -> JsError {
    JsError::Type("service workers are unavailable in this realm".into())
}

/// This realm's module, made on first use.
fn module<'s>(s: &Scope<'s>, page: &Rc<PageDom>) -> JsResult<Object<'s>> {
    if let Some(module) = existing(s, page)? {
        return Ok(module);
    }
    let holder = crate::worker_scope::module_holder(s, page)?;
    let client = client_of(page);
    let native = s.object();
    native.set_atom("deliverMessage", s.function("deliverMessage", |s, _, args| {
        crate::cookie_store::events::dispatch_trusted(s, args.get(0), args.get(1))?;
        Ok(s.undefined())
    })?.value())?;
    let f = s.function("cookieManager", |s, _, a| {
        let page = crate::target::page_of(s)?;
        let id = a.get(0).to_number()? as u64;
        let cookies = if let Some(registration) = same_origin_registration(&page, id) {
            registration.cookies.clone()
        } else {
            let scope = pyro_net::Url::parse(&a.get(1).to_string()?).map_err(|_| JsError::ContextDestroyed)?;
            let list = crate::cookie_store::subscriptions::Subscriptions::new(scope, page.cookie_partition_key(), page.cookie_site_context(), None);
            list.clear();
            list
        };
        crate::cookie_store::subscriptions::manager(s, &page, cookies, None)
    })?;
    native.set_atom("cookieManager", f.value())?;
    native.set_atom("registrationCookies", crate::streams::shared_slot(s, page, "ServiceWorkerRegistration.cookies")?)?;
    let f = s.function("register", |s, _, a| {
        let page = crate::target::page_of(s)?;
        let id = a.get(0).to_number()? as u64;
        let script = a.get(1).to_string()?;
        let scope = if a.get(2).is_undefined() { None } else { Some(a.get(2).to_string()?) };
        let module = a.get(3).to_string()? == "module";
        if module && !pyro_js::Capabilities::detect().modules {
            return Err(JsError::dom(
                "NotSupportedError",
                "Failed to register a ServiceWorker: Module service workers are not supported.",
            ));
        }
        register(s, &page, id, &script, scope, module, a.get(4).to_string()?)?;
        Ok(s.undefined())
    })?;
    native.set_atom("register", f.value())?;
    // Trusted Types: `register(scriptURL)` takes a TrustedScriptURL ("ServiceWorkerContainer register"); the JS wrapper
    // calls this inside its try block so a violation rejects the promise with the TypeError.
    let f = s.function("trustedScriptURL", |s, _, a| {
        let page = crate::target::page_of(s)?;
        let url = crate::trusted_types::compliant_string(s, &page, crate::trusted_types::Kind::ScriptUrl, a.get(0), "ServiceWorkerContainer register", false)?;
        Ok(s.string(&url))
    })?;
    native.set_atom("trustedScriptURL", f.value())?;
    let f = s.function("blocked", |s, _, _| {
        let page = crate::target::page_of(s)?;
        Ok(s.boolean(page.context_services().service_workers.blocked.get()))
    })?;
    native.set_atom("blocked", f.value())?;
    let f = s.function("getRegistration", |s, _, a| {
        let page = crate::target::page_of(s)?;
        let id = a.get(0).to_number()? as u64;
        let client = client_of(&page).ok_or_else(|| JsError::dom("SecurityError", "The document is in an invalid state."))?;
        let url = if a.get(1).is_undefined() {
            client.url.clone()
        } else {
            let input = a.get(1).to_string()?;
            let base = page.dom.borrow().document_base_url(page.document()).ok().map(str::to_owned);
            let base = base.and_then(|b| pyro_net::Url::parse(&b).ok());
            let url = pyro_net::Url::options()
                .base_url(base.as_ref())
                .parse(&input)
                .map_err(|_| JsError::Type(format!("Failed to execute 'getRegistration' on 'ServiceWorkerContainer': The URL provided ('{input}') is invalid.").into()))?;
            if tuple_origin(&url).as_deref() != Some(client.origin.as_str()) {
                return Err(JsError::dom("SecurityError", "Failed to execute 'getRegistration' on 'ServiceWorkerContainer': The origin of the provided documentURL does not match the current origin."));
            }
            url.to_string()
        };
        let settle = Settle { page: Rc::downgrade(&page), id, realm: s.realm().downgrade() };
        match page.context_services().service_workers.matching(&url, client.partition.as_deref(), client.site) {
            Some(registration) => settle.registration(&registration),
            None => settle.done(Ok("{\"reg\":null}".into())),
        }
        Ok(s.undefined())
    })?;
    native.set_atom("getRegistration", f.value())?;
    let f = s.function("getRegistrations", |s, _, a| {
        let page = crate::target::page_of(s)?;
        let id = a.get(0).to_number()? as u64;
        let client =
            client_of(&page).ok_or_else(|| JsError::dom("SecurityError", "The document is in an invalid state."))?;
        let context = page.context_services();
        let registry = &context.service_workers;
        let mine = registry.matching(&client.url, client.partition.as_deref(), client.site);
        let mut out = String::from("{\"regs\":[");
        for registration in
            registry.registrations.borrow().iter().filter(|r| r.shares_storage(&client) && !r.uninstalling.get())
        {
            if out.len() > 10 {
                out.push(',');
            }
            out.push_str(&registration.json(mine.as_ref().is_some_and(|m| Rc::ptr_eq(m, registration))));
        }
        out.push_str("]}");
        Settle { page: Rc::downgrade(&page), id, realm: s.realm().downgrade() }.done(Ok(out));
        Ok(s.undefined())
    })?;
    native.set_atom("getRegistrations", f.value())?;
    let f = s.function("unregister", |s, _, a| {
        let page = crate::target::page_of(s)?;
        not_fully_active(&page, "unregister a ServiceWorkerRegistration")?;
        let id = a.get(0).to_number()? as u64;
        let registration = same_origin_registration(&page, a.get(1).to_number()? as u64);
        let done = registration.is_some_and(|r| unregister(&r));
        Settle { page: Rc::downgrade(&page), id, realm: s.realm().downgrade() }.done(Ok(format!("{{\"value\":{done}}}")));
        Ok(s.undefined())
    })?;
    native.set_atom("unregister", f.value())?;
    let f = s.function("update", |s, _, a| {
        let page = crate::target::page_of(s)?;
        not_fully_active(&page, "update a ServiceWorkerRegistration")?;
        let id = a.get(0).to_number()? as u64;
        let settle = Settle { page: Rc::downgrade(&page), id, realm: s.realm().downgrade() };
        let registration = same_origin_registration(&page, a.get(1).to_number()? as u64);
        let Some(registration) = registration.filter(|r| !r.uninstalling.get()) else {
            settle.done(reply_error(
                "InvalidStateError",
                "Failed to update a ServiceWorker: The registration is not active.",
            ));
            return Ok(s.undefined());
        };
        match registration.newest() {
            Some(newest) if !registration.updating.get() => {
                let url = pyro_net::Url::parse(&newest.script_url).map_err(|_| unavailable())?;
                update(&page, &registration, url, settle);
            }
            _ => settle.registration(&registration),
        }
        Ok(s.undefined())
    })?;
    native.set_atom("update", f.value())?;
    let f = s.function("preload", |s, _, a| {
        let page = crate::target::page_of(s)?;
        let id = a.get(0).to_number()? as u64;
        let settle = Settle { page: Rc::downgrade(&page), id, realm: s.realm().downgrade() };
        let op = a.get(2).to_string()?;
        let value = if a.get(3).is_undefined() { String::new() } else { a.get(3).to_string()? };
        let registration = same_origin_registration(&page, a.get(1).to_number()? as u64).filter(|r| !r.uninstalling.get());
        let result = match registration {
            Some(registration) => preload_op(&registration, &op, &value).map(|json| {
                // A settled promise's value rides in `value` (see the page glue's `settle`).
                if op == "getState" { format!("{{\"value\":{json}}}") } else { "{}".to_owned() }
            }),
            None => reply_error("InvalidStateError", "The registration is gone."),
        };
        settle.done(result);
        Ok(s.undefined())
    })?;
    native.set_atom("preload", f.value())?;
    // Whether the document is detached (its frame was removed): it has no controller then, and
    // posts nothing.
    let f = s.function("detached", |s, _, _| {
        let page = crate::target::page_of(s)?;
        Ok(s.boolean(page.is_detached() || page.is_disposed()))
    })?;
    native.set_atom("detached", f.value())?;
    let f = s.function("postMessage", |s, _, a| {
        let page = crate::target::page_of(s)?;
        if page.is_detached() || page.is_disposed() {
            return Err(JsError::dom(
                "InvalidStateError",
                "Failed to execute 'postMessage' on 'ServiceWorker': Cannot post a message from a detached context.",
            ));
        }
        let version = a.get(0).to_number()? as u64;
        let options = crate::structured_clone::transfer_options(s, a.opt(2))?;
        // Serialized (and its errors thrown) even when the worker is gone.
        let message = crate::structured_clone::encode_message(s, a.get(1), options)?;
        let Some(version) = VERSIONS.with(|v| v.borrow().get(&version).and_then(Weak::upgrade)) else {
            return Ok(s.undefined());
        };
        if version.state.get() == State::Redundant {
            return Ok(s.undefined());
        }
        let Some(client) = client_of(&page) else { return Ok(s.undefined()) };
        if !version.registration.upgrade().is_some_and(|r| r.shares_storage(&client)) {
            return Ok(s.undefined());
        }
        let size = message.size();
        let queued = version.queued.load(Ordering::Acquire);
        if queued > 0 && queued.saturating_add(size) > MAX_QUEUED_BYTES {
            return Err(JsError::dom("DataCloneError", "The service worker's message queue is full"));
        }
        version.queued.fetch_add(size, Ordering::AcqRel);
        let (source, origin_key, counter) = (client_json(&client), page.origin_key(), version.queued.clone());
        version.run(move |s, _page| {
            counter.fetch_sub(size, Ordering::AcqRel);
            crate::service_worker_scope::message(s, &message, Some(&source), &origin_key);
        });
        Ok(s.undefined())
    })?;
    native.set_atom("postMessage", f.value())?;
    let initial = s.object();
    let controller = client.as_ref().and_then(|c| c.controller.borrow().clone());
    initial.set_atom("controller", controller.map_or_else(|| s.null(), |v| s.string(&v.json())))?;
    let ready = client.as_ref()
        .and_then(|c| page.context_services().service_workers.matching(&c.url, c.partition.as_deref(), c.site));
    initial.set_atom("ready", ready.map_or_else(|| s.null(), |r| s.string(&r.json(true))))?;
    initial.set_atom("clientId", client.as_ref().map_or_else(|| s.string(""), |c| s.string(&c.id)))?;
    initial.set_atom("url", client.as_ref().map_or_else(|| s.string(""), |c| s.string(&c.url)))?;
    let roots = (Persistent::new(s, native.value()), Persistent::new(s, initial.value()));
    let factory = s.eval(include_str!("service_worker.js"), Some("pyro://internal/service-worker.js"), 1)?;
    let module = factory.as_object().ok_or_else(unavailable)?.call(None, &[roots.0.get(s), roots.1.get(s)])?;
    s.set_hidden(holder, KEY, module)?;
    module.as_object().ok_or_else(unavailable)
}

/// `navigator.serviceWorker`: the realm's container in a secure context,
/// undefined elsewhere (as the interface is `[SecureContext]`).
pub(crate) fn container<'s>(s: &Scope<'s>, page: &Rc<PageDom>) -> JsResult<Value<'s>> {
    let secure = page.document_url_parsed().is_some_and(|u| pyro_net::is_potentially_trustworthy_url(&u));
    if !secure
        || !matches!(page.document_url_parsed().map(|u| u.scheme().to_owned()).as_deref(), Some("http" | "https"))
    {
        return Ok(s.undefined());
    }
    if opaque_origin(page) {
        return Err(JsError::dom(
            "SecurityError",
            "Failed to read the 'serviceWorker' property from 'Navigator': Service worker is disabled because the context is sandboxed and lacks the 'allow-same-origin' flag.",
        ));
    }
    module(s, page)?.get_atom("container")
}

/// Adds the lazy interface names to the window.
pub(crate) fn define_globals<W>(def: &mut pyro_js::ClassDef<W>)
where
    W: pyro_js::JsClass,
    W::Data: crate::window::WindowHost,
{
    macro_rules! lazy {
        ($($name:literal),+) => {$(
            def.global_value_accessor(
                $name,
                |s, w| {
                    let value = module(s, w.page())?.get($name)?;
                    replace(s, w.page(), $name, value)?;
                    Ok(value)
                },
                |s, w, value| replace(s, w.page(), $name, value),
            );
        )+};
    }
    lazy!("ServiceWorker", "ServiceWorkerContainer", "ServiceWorkerRegistration", "NavigationPreloadManager");
}

fn replace(s: &Scope<'_>, page: &Rc<PageDom>, name: &str, value: Value<'_>) -> JsResult<()> {
    let define = page.define_properties.borrow().get(&s.realm_id()).map(|p| p.get(s));
    let define = define.and_then(|v| v.as_object()).ok_or_else(unavailable)?;
    let desc = s.object();
    desc.set_atom("value", value)?;
    desc.set_atom("writable", s.boolean(true))?;
    desc.set_atom("enumerable", s.boolean(false))?;
    desc.set_atom("configurable", s.boolean(true))?;
    define.call(None, &[s.global().value(), s.string(name), desc.value()])?;
    Ok(())
}

/// This module's part of [`crate::thread_state::reset_thread`]: what of its
/// per-thread state still holds something, if any does.
pub(crate) fn thread_leftover() -> Option<&'static str> {
    VERSIONS.with(|v| v.borrow_mut().retain(|_, version| version.strong_count() > 0));
    if VERSIONS.with(|v| !v.borrow().is_empty()) {
        return Some("service_worker::VERSIONS");
    }
    FINISHERS.with(|f| !f.borrow().is_empty()).then_some("service_worker::FINISHERS")
}
