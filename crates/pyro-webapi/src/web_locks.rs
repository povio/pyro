//! Web Locks API (https://w3c.github.io/web-locks/): `navigator.locks` (`LockManager`) and `Lock`.
//!
//! The interfaces and each request's promise plumbing are made in JS (web_locks.js, one module per
//! realm); the lock manager's state, which every same-origin document shares, is here: per
//! (browser context, origin) "lock manager", per resource name a queue of requests, the held
//! locks first. A request is granted in a task on its own document's page, which runs the
//! module's callback with the new `Lock`; the module releases the lock when the callback's
//! promise settles. A document that goes away (navigated, removed frame, closed popup) releases
//! its locks and drops its requests, as the spec's "agent terminated" steps do, and the next
//! requests in each queue are granted.
//!
//! Scope, as `BroadcastChannel`'s (message_channel.rs): the documents of the browser thread that
//! share a browser context (`ContextServices`) and origin. Workers have no `navigator.locks`.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::{Rc, Weak};

use pyro_js::{JsError, JsResult, Object, Persistent, Scope, Value, WeakRealm};
use rustc_hash::FxHashMap;

use crate::lazy_module::LazyModule;
use crate::page::PageDom;

pub(crate) static MODULE: LazyModule = LazyModule {
    key: "lpWebLocks",
    source: include_str!("web_locks.js"),
    url: "pyro://internal/web_locks.js",
    natives: Some(natives),
};

/// `LockManager` and `Lock` on a window global (`[SecureContext]`: absent elsewhere).
pub(crate) fn define_globals<W>(def: &mut pyro_js::ClassDef<W>)
where
    W: pyro_js::JsClass,
    W::Data: crate::window::WindowHost,
{
    macro_rules! global {
        ($($name:literal),+) => {$(
            def.global_value_accessor(
                $name,
                |s, w| {
                    let page = crate::window::WindowHost::page(w);
                    if !secure(s, page) {
                        return Ok(s.undefined());
                    }
                    MODULE.global(s, page, $name)
                },
                |s, w, value| crate::lazy_module::replace(s, crate::window::WindowHost::page(w), $name, value),
            );
        )+};
    }
    global!("LockManager", "Lock");
}

/// `navigator.locks`: the realm's `LockManager` in a secure context, undefined elsewhere.
pub(crate) fn locks<'s>(s: &Scope<'s>, page: &Rc<PageDom>) -> JsResult<Value<'s>> {
    if page.is_disposed() {
        return Err(JsError::ContextDestroyed);
    }
    if !secure(s, page) {
        return Ok(s.undefined());
    }
    MODULE.module(s, page)?.get("locks")
}

/// A secure context: the realm's origin or its document's URL is potentially trustworthy (a
/// sandboxed https document, whose origin is opaque, is still a secure context: there the API
/// is present and throws SecurityError).
fn secure(s: &Scope<'_>, page: &PageDom) -> bool {
    let trustworthy = |url: &str| pyro_net::Url::parse(url).is_ok_and(|u| pyro_net::is_potentially_trustworthy_url(&u));
    s.realm().origin().is_some_and(|o| trustworthy(&o))
        || page.dom.borrow().document_data(page.document()).ok().is_some_and(|d| trustworthy(&d.url.to_string()))
}

/// `[SecureContext]`: in a window that is not a secure context `navigator.locks`, `LockManager`
/// and `Lock` do not exist (`'locks' in navigator` is false). Run when the realm is set up.
pub(crate) fn hide_if_insecure(s: &Scope<'_>, page: &PageDom) -> JsResult<()> {
    if secure(s, page) {
        return Ok(());
    }
    let global = s.global();
    global.delete("LockManager")?;
    global.delete("Lock")?;
    if let Some(navigator) = global.get_atom("Navigator")?.as_object() {
        if let Some(proto) = navigator.get_atom("prototype")?.as_object() {
            proto.delete("locks")?;
        }
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Exclusive,
    Shared,
}

impl Mode {
    fn as_str(self) -> &'static str {
        match self {
            Mode::Exclusive => "exclusive",
            Mode::Shared => "shared",
        }
    }
}

struct Request {
    id: u64,
    mode: Mode,
    /// The query's `clientId`: the document's service worker client id.
    client_id: String,
    /// Its grant callback ran (the lock reached script). Until then an abort still removes it.
    delivered: Cell<bool>,
    /// The requesting document's page (`PageDom::id`, also the query's `clientId`).
    page_id: u64,
    page: Weak<PageDom>,
    realm: WeakRealm,
    /// Called (no arguments) in a task on the page when the lock is granted.
    grant: Persistent,
    /// Called (no arguments) in a task on the page when a `steal` request took the lock.
    stolen: Persistent,
}

#[derive(Default)]
struct Queue {
    held: Vec<Request>,
    pending: VecDeque<Request>,
}

impl Queue {
    /// Whether a request of `mode` at the front of the queue can be granted now.
    fn grantable(&self, mode: Mode) -> bool {
        match mode {
            Mode::Exclusive => self.held.is_empty(),
            Mode::Shared => self.held.iter().all(|r| r.mode == Mode::Shared),
        }
    }
}

/// One lock manager: a browser context's and origin's queues, by resource name.
type Manager = FxHashMap<String, Queue>;

thread_local! {
    static MANAGERS: RefCell<FxHashMap<(usize, String), Manager>> = RefCell::default();
    static NEXT_ID: Cell<u64> = const { Cell::new(1) };
    /// Granted requests an abort took back before their grant task ran.
    static CANCELLED: RefCell<rustc_hash::FxHashSet<u64>> = RefCell::default();
}

/// The lock manager key of the realm's document: its browser context and tuple origin, or
/// `None` for an opaque origin (the API then throws SecurityError).
fn manager_key(s: &Scope<'_>, page: &PageDom) -> Option<(usize, String)> {
    let origin = crate::storage::tuple_origin(s, page)?;
    Some((Rc::as_ptr(&page.context_services()) as usize, origin))
}

/// Calls `f` (a request's grant or stolen callback) in a task on its page, in its realm; a grant
/// (`grant` is its request) only if the request is still held when the task runs.
fn call_later(page: &Weak<PageDom>, realm: &WeakRealm, f: Persistent, grant: Option<u64>) {
    let Some(target) = page.upgrade() else { return };
    let realm = realm.clone();
    let weak = page.clone();
    target.post_task(Box::new(move || {
        let Some(page) = weak.upgrade() else { return };
        if page.is_disposed() || grant.is_some_and(|id| !deliver(id)) {
            return;
        }
        let Some(realm) = realm.upgrade() else { return };
        let Ok(s) = realm.scope() else { return };
        let callback = f.get(&s);
        if let Some(callback) = callback.as_object() {
            if let Err(e) = callback.call(None, &[]) {
                s.report_exception(&e);
            }
        }
    }));
}

/// Grants every request at the front of `queue` that can be granted ("process the lock request
/// queue"): the requests are granted in order, so a request waits behind the ones before it.
fn process(queue: &mut Queue) -> Vec<Call> {
    let mut granted = Vec::new();
    while let Some(front) = queue.pending.front() {
        if !queue.grantable(front.mode) {
            break;
        }
        let request = queue.pending.pop_front().expect("front");
        granted.push(Call::Grant(request.id, request.page.clone(), request.realm.clone(), request.grant.clone()));
        queue.held.push(request);
    }
    granted
}

/// A callback to run in a task on a request's page: a grant (only if the request is still held
/// then: a synchronous abort may have taken it back) or a steal.
enum Call {
    Grant(u64, Weak<PageDom>, WeakRealm, Persistent),
    Stolen(Weak<PageDom>, WeakRealm, Persistent),
}

/// Whether the grant of request `id` may run now: not when an abort took it back first. A request
/// whose lock was stolen before its grant ran still gets its callback (its promise is rejected),
/// as the spec's grant task runs whatever happened to the lock since. Marks it delivered.
fn deliver(id: u64) -> bool {
    if CANCELLED.with(|c| c.borrow_mut().remove(&id)) {
        return false;
    }
    MANAGERS.with(|m| {
        if let Some(r) = m.borrow().values().flat_map(|manager| manager.values()).flat_map(|q| q.held.iter()).find(|r| r.id == id) {
            r.delivered.set(true);
        }
    });
    true
}

fn run(call: Call) {
    match call {
        Call::Grant(id, page, realm, f) => call_later(&page, &realm, f, Some(id)),
        Call::Stolen(page, realm, f) => call_later(&page, &realm, f, None),
    }
}

/// Runs `f` on the realm's lock manager, then the grant callbacks it produced (outside the
/// registry's borrow: a callback runs in a later task anyway).
fn with_manager<R>(key: &(usize, String), f: impl FnOnce(&mut Manager) -> (R, Vec<Call>)) -> R {
    let (result, calls) = MANAGERS.with(|m| {
        let mut managers = m.borrow_mut();
        let manager = managers.entry(key.clone()).or_default();
        let out = f(manager);
        manager.retain(|_, q| !q.held.is_empty() || !q.pending.is_empty());
        if manager.is_empty() {
            managers.remove(key);
        }
        out
    });
    for call in calls {
        run(call);
    }
    result
}

fn natives<'s>(s: &Scope<'s>) -> JsResult<Object<'s>> {
    let native = s.object();
    // state() -> "ok" | "inactive" (the document is not fully active) | "opaque" (opaque origin).
    let state = s.function("state", |s, _, _| {
        let page = crate::target::page_of(s)?;
        Ok(s.string(if !page.is_fully_active() {
            "inactive"
        } else if manager_key(s, &page).is_none() {
            "opaque"
        } else {
            "ok"
        }))
    })?;
    native.set_atom("state", state.value())?;
    // request(name, exclusive, ifAvailable, steal, grant, stolen) -> id, or 0 when `ifAvailable`
    // and the lock cannot be granted now (nothing is queued; the module calls back with null).
    let request = s.function("request", |s, _, a| {
        let page = crate::target::page_of(s)?;
        let key = manager_key(s, &page).ok_or_else(|| JsError::dom("SecurityError", "Access to the Locks API is denied in this context."))?;
        let name = a.get(0).to_string()?;
        let mode = if a.get(1).to_bool() { Mode::Exclusive } else { Mode::Shared };
        let (if_available, steal) = (a.get(2).to_bool(), a.get(3).to_bool());
        let id = NEXT_ID.with(|n| n.replace(n.get() + 1));
        let client_id = crate::service_worker::client_id(&page).unwrap_or_else(|| format!("{:032x}", page.id()));
        let request = Request {
            id,
            mode,
            client_id,
            delivered: Cell::new(false),
            page_id: page.id(),
            page: Rc::downgrade(&page),
            realm: s.realm().downgrade(),
            grant: Persistent::new(s, a.get(4)),
            stolen: Persistent::new(s, a.get(5)),
        };
        let id = with_manager(&key, |manager| {
            let queue = manager.entry(name).or_default();
            let mut calls = Vec::new();
            if steal {
                // "steal": every held lock of the name is released, its holder's promise
                // rejected with AbortError; the request goes first and is granted.
                for held in queue.held.drain(..) {
                    calls.push(Call::Stolen(held.page.clone(), held.realm.clone(), held.stolen.clone()));
                }
                queue.pending.push_front(request);
            } else if if_available && !(queue.pending.is_empty() && queue.grantable(mode)) {
                return (0, calls);
            } else {
                queue.pending.push_back(request);
            }
            calls.extend(process(queue));
            (id, calls)
        });
        Ok(s.number(id as f64))
    })?;
    native.set_atom("request", request.value())?;
    // release(id): the held lock `id` goes (its callback's promise settled); later requests are
    // granted. Nothing when it went already (stolen, or its document went away).
    let release = s.function("release", |s, _, a| {
        let page = crate::target::page_of(s)?;
        let Some(key) = manager_key(s, &page) else { return Ok(s.undefined()) };
        let id = a.get(0).to_number()? as u64;
        with_manager(&key, |manager| {
            let mut calls = Vec::new();
            for queue in manager.values_mut() {
                if let Some(i) = queue.held.iter().position(|r| r.id == id) {
                    queue.held.remove(i);
                    calls.extend(process(queue));
                    break;
                }
            }
            ((), calls)
        });
        Ok(s.undefined())
    })?;
    native.set_atom("release", release.value())?;
    // abort(id) -> whether `id` was still waiting (its signal aborted it: it leaves the queue).
    let abort = s.function("abort", |s, _, a| {
        let page = crate::target::page_of(s)?;
        let Some(key) = manager_key(s, &page) else { return Ok(s.boolean(false)) };
        let id = a.get(0).to_number()? as u64;
        let removed = with_manager(&key, |manager| {
            let mut calls = Vec::new();
            let mut removed = false;
            for queue in manager.values_mut() {
                if let Some(i) = queue.pending.iter().position(|r| r.id == id) {
                    queue.pending.remove(i);
                    removed = true;
                } else if let Some(i) = queue.held.iter().position(|r| r.id == id && !r.delivered.get()) {
                    // Granted, but its grant has not reached script yet: the abort still wins.
                    queue.held.remove(i);
                    CANCELLED.with(|c| c.borrow_mut().insert(id));
                    removed = true;
                }
                if removed {
                    calls.extend(process(queue));
                    break;
                }
            }
            (removed, calls)
        });
        Ok(s.boolean(removed))
    })?;
    native.set_atom("abort", abort.value())?;
    // query() -> [held, pending], each a list of [name, mode, clientId] of the realm's lock
    // manager, in each queue's order.
    let query = s.function("query", |s, _, _| {
        let page = crate::target::page_of(s)?;
        let rows: (Vec<(String, &'static str, String)>, Vec<(String, &'static str, String)>) = match manager_key(s, &page) {
            None => Default::default(),
            Some(key) => MANAGERS.with(|m| {
                let managers = m.borrow();
                let mut out: (Vec<_>, Vec<_>) = Default::default();
                if let Some(manager) = managers.get(&key) {
                    let mut names: Vec<&String> = manager.keys().collect();
                    names.sort();
                    for name in names {
                        let queue = &manager[name];
                        out.0.extend(queue.held.iter().map(|r| (name.clone(), r.mode.as_str(), r.client_id.clone())));
                        out.1.extend(queue.pending.iter().map(|r| (name.clone(), r.mode.as_str(), r.client_id.clone())));
                    }
                }
                out
            }),
        };
        let list = |rows: &[(String, &'static str, String)]| -> JsResult<Value<'_>> {
            let mut items = pyro_js::RootedVec::new();
            for (name, mode, client) in rows {
                let row = [s.string(name), s.string(mode), s.string(client)];
                items.push(s, s.array(&row)?.value());
            }
            Ok(s.array(&items.iter(s).collect::<Vec<_>>())?.value())
        };
        let (held, pending) = (list(&rows.0)?, list(&rows.1)?);
        Ok(s.array(&[held, pending])?.value())
    })?;
    native.set_atom("query", query.value())?;
    Ok(native)
}

/// The document `page` goes away: its locks are released and its requests dropped (the spec's
/// steps when an agent or document is terminated), and the next requests are granted.
pub(crate) fn dispose(page: &PageDom) {
    let id = page.id();
    let mut calls = Vec::new();
    let mut dropped = Vec::new();
    MANAGERS.with(|m| {
        let mut managers = m.borrow_mut();
        for manager in managers.values_mut() {
            for queue in manager.values_mut() {
                let before = queue.held.len() + queue.pending.len();
                // Dropped outside the borrow: a Persistent's drop may run engine code.
                let (gone, kept): (Vec<_>, Vec<_>) = queue.held.drain(..).partition(|r| r.page_id == id);
                queue.held = kept;
                dropped.extend(gone);
                let (gone, kept): (Vec<_>, Vec<_>) = queue.pending.drain(..).partition(|r| r.page_id == id);
                queue.pending = kept.into();
                dropped.extend(gone);
                if queue.held.len() + queue.pending.len() != before {
                    calls.extend(process(queue));
                }
            }
            manager.retain(|_, q| !q.held.is_empty() || !q.pending.is_empty());
        }
        managers.retain(|_, m| !m.is_empty());
    });
    drop(dropped);
    for call in calls {
        run(call);
    }
}

/// This module's part of [`crate::thread_state::reset_thread`]: what of its per-thread state
/// still holds something, if any does.
pub(crate) fn thread_leftover() -> Option<&'static str> {
    // CANCELLED holds ids of aborted grants whose tasks went with their pages; the counter starts
    // again, as the other ids handed to script do.
    CANCELLED.with(|c| c.borrow_mut().clear());
    if MANAGERS.with(|m| m.borrow().is_empty()) {
        NEXT_ID.set(1);
    }
    MANAGERS.with(|m| !m.borrow().is_empty()).then_some("web_locks::MANAGERS")
}
