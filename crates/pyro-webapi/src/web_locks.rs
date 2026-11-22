//! Web Locks API (https://w3c.github.io/web-locks/): `navigator.locks` (`LockManager`) and `Lock`.
//!
//! A context's lock service shares the storage-key queues across its
//! documents and workers. JS callbacks live only on their owning thread;
//! each grant or steal posts a notice to that agent's event loop.

mod broker;
pub(crate) use broker::LockService;
use broker::{Client, Mode, Notice, Request, StorageKey};

use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

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
                    if !crate::secure_context::secure(s, page) {
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
    if !crate::worker_scope::secure_worker(s).unwrap_or_else(|| crate::secure_context::secure(s, page)) {
        return Ok(s.undefined());
    }
    MODULE.module(s, page)?.get("locks")
}

struct Callbacks {
    realm: WeakRealm,
    grant: Option<Persistent>,
    abort_hook: Option<crate::abort::AbortRegistration>,
    stolen: Persistent,
    key: StorageKey,
    name: String,
}

struct LocalClient {
    page: Weak<PageDom>,
    route: Arc<Client>,
    requests: FxHashMap<u64, Callbacks>,
}

thread_local! {
    static CLIENTS: RefCell<FxHashMap<u64, LocalClient>> = RefCell::default();
}
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn manager_key(s: &Scope<'_>, page: &PageDom) -> Option<StorageKey> {
    Some(StorageKey {
        origin: crate::storage::tuple_origin(s, page)?,
        top_level_site: page.cookie_partition_key(),
        cross_site_ancestor: page.cookie_site_context() == pyro_net::cookies::SiteContext::CrossSite,
        local_thread: CLIENTS.with(|clients| {
            let remote = clients.borrow().get(&page.id()).map(|c| c.route.poster.is_some())
                .unwrap_or_else(|| page.remote_poster().is_some());
            (!remote).then(|| std::thread::current().id())
        }),
    })
}

fn forget(page_id: u64, id: u64) -> Option<Callbacks> {
    CLIENTS.with(|clients| {
        let mut clients = clients.borrow_mut();
        let client = clients.get_mut(&page_id)?;
        let callbacks = client.requests.remove(&id);
        if client.requests.is_empty() { clients.remove(&page_id); }
        callbacks
    })
}

fn deliver(route: &Client) {
    let Some(notice) = route.next_notice() else { return };
    let id = match notice { Notice::Grant(id) | Notice::Stolen(id) => id };
    let callback = match notice {
        Notice::Grant(id) => CLIENTS.with(|clients| {
            let mut clients = clients.borrow_mut();
            let req = clients.get_mut(&route.page)?.requests.get_mut(&id)?;
            drop(req.abort_hook.take());
            Some((req.realm.clone(), req.grant.take()?))
        }),
        Notice::Stolen(id) => forget(route.page, id).map(|req| (req.realm, req.stolen)),
    };
    let Some((realm, callback)) = callback else { return };
    let Some(realm) = realm.upgrade() else { return };
    let Ok(s) = realm.scope() else { return };
    if let Some(callback) = callback.get(&s).as_object() {
        if let Err(error) = callback.call(None, &[s.number(id as f64)]) {
            s.report_exception(&error);
        }
    }
}

fn dispatch(wake: Vec<Arc<Client>>) {
    for route in wake {
        if route.thread == std::thread::current().id() {
            let target = CLIENTS.with(|c| c.borrow().get(&route.page).and_then(|c| c.page.upgrade()));
            if let Some(page) = target {
                page.post_task(Box::new(move || deliver(&route)));
            }
        } else if let Some(post) = route.poster.clone() {
            post(Box::new(move || deliver(&route)));
        }
    }
}

fn remove_request(page: &PageDom, id: u64, abort: bool) -> bool {
    let info = CLIENTS.with(|clients| {
        let clients = clients.borrow();
        let request = clients.get(&page.id())?.requests.get(&id)?;
        // Abort only applies until the grant callback starts.
        if abort && request.grant.is_none() { return None }
        Some((request.key.clone(), request.name.clone()))
    });
    let Some((key, name)) = info else { return false };
    let (removed, wake) = page.context_services().web_locks.remove(&key, &name, id, abort);
    if removed {
        // A stolen request stays rooted until its stolen notice runs.
        drop(forget(page.id(), id));
    }
    dispatch(wake);
    removed
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
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let client_id = crate::service_worker::client_id(&page).unwrap_or_else(|| format!("{:032x}", page.id()));
        let callbacks = Callbacks {
            realm: s.realm().downgrade(),
            grant: Some(Persistent::new(s, a.get(4))),
            abort_hook: None,
            stolen: Persistent::new(s, a.get(5)),
            key: key.clone(),
            name: name.clone(),
        };
        let route = CLIENTS.with(|clients| {
            let mut clients = clients.borrow_mut();
            let client = clients.entry(page.id()).or_insert_with(|| LocalClient {
                page: Rc::downgrade(&page),
                route: Arc::new(Client::new(page.id(), page.remote_poster())),
                requests: FxHashMap::default(),
            });
            client.requests.insert(id, callbacks);
            client.route.clone()
        });
        let req = Request { id, mode, client_id, client: route };
        let (accepted, wake) = page.context_services().web_locks.acquire(key, name, req, steal, if_available);
        if !accepted {
            drop(forget(page.id(), id));
            return Ok(s.number(0.0));
        }
        dispatch(wake);
        Ok(s.number(id as f64))
    })?;
    native.set_atom("request", request.value())?;
    let release = s.function("release", |s, _, a| {
        let page = crate::target::page_of(s)?;
        remove_request(&page, a.get(0).to_number()? as u64, false);
        Ok(s.undefined())
    })?;
    native.set_atom("release", release.value())?;
    let abort = s.function("abort", |s, _, a| {
        let page = crate::target::page_of(s)?;
        Ok(s.boolean(remove_request(&page, a.get(0).to_number()? as u64, true)))
    })?;
    native.set_atom("abort", abort.value())?;
    // watchAbort(signal, id): DOM "add an abort algorithm" to `signal`: when it aborts (before its
    // `abort` event, which a listener could stop and a page could fake), the module's
    // abortRequest(id) runs. The signal holds only the id; the steps stay in JS.
    let watch = s.function("watchAbort", |s, _, a| {
        let signal = s
            .unwrap::<crate::abort::AbortSignal>(a.get(0))
            .ok_or_else(|| JsError::Type("not an AbortSignal".into()))?;
        let id = a.get(1).to_number()? as u64;
        let page_id = crate::target::page_of(s)?.id();
        let realm = s.realm().downgrade();
        let registration = signal.register_native(Box::new(move |_page| {
            let Some(realm) = realm.upgrade() else { return };
            let Ok(s) = realm.scope() else { return };
            let Ok(page) = crate::target::page_of(&s) else { return };
            let run = || -> JsResult<()> {
                let module = MODULE.module(&s, &page)?;
                if let Some(f) = module.get("abortRequest")?.as_object() {
                    f.call(None, &[s.number(id as f64)])?;
                }
                Ok(())
            };
            if let Err(e) = run() {
                s.report_exception(&e);
            }
        }));
        CLIENTS.with(|clients| {
            if let Some(req) = clients.borrow_mut().get_mut(&page_id).and_then(|c| c.requests.get_mut(&id)) {
                req.abort_hook = registration;
            }
        });
        Ok(s.undefined())
    })?;
    native.set_atom("watchAbort", watch.value())?;
    // query() -> [held, pending], each a list of [name, mode, clientId] of the realm's lock
    // manager, in each queue's order.
    let query = s.function("query", |s, _, _| {
        let page = crate::target::page_of(s)?;
        let rows = manager_key(s, &page)
            .map(|key| page.context_services().web_locks.query(&key))
            .unwrap_or_default();
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
    let local = CLIENTS.with(|clients| clients.borrow_mut().remove(&page.id()));
    if local.is_some() {
        let wake = page.context_services().web_locks.dispose(page.id());
        drop(local);
        dispatch(wake);
    }
}

pub(crate) fn thread_leftover() -> Option<&'static str> {
    CLIENTS.with(|clients| !clients.borrow().is_empty()).then_some("web_locks::CLIENTS")
}
