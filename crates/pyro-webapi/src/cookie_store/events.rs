//! Script-visible mutations enter a per-window inbox from the shared cookie jar.
//! Network threads enqueue only native cookie data. The owning realm constructs
//! the event and calls listeners in a document task after the jar lock is free.

use pyro_dom::events::TargetKey;
use pyro_js::{JsError, JsResult, Object, Persistent, Realm, RealmId, RootedVec, Scope, Value};
use pyro_net::cookies::{CookieChange, CookieJar};
use std::rc::Rc;
use std::sync::{Arc, Mutex, Weak};

use crate::event::Event;
use crate::page::PageDom;
use crate::target::{self, EventTarget};

const MODULE: &str = "lpCookieChangeEvents";

pub(crate) fn module<'s>(s: &Scope<'s>, page: &Rc<PageDom>, sw: bool) -> JsResult<Object<'s>> {
    let holder = crate::worker_scope::module_holder(s, page)?;
    if let Some(module) = s.get_hidden(holder, MODULE)?.and_then(|v| v.as_object()) {
        return Ok(module);
    }
    let (name, base) = if sw {
        ("ExtendableCookieChangeEvent", crate::service_worker_scope::extendable_event(s)?)
    } else {
        ("CookieChangeEvent", s.interface::<Event>()?.value())
    };
    let slots = crate::streams::shared_slot(s, page, name)?;
    let factory = s.eval(include_str!("events.js"), Some("pyro://internal/cookie-events.js"), 1)?;
    let module = factory.as_object().ok_or(JsError::ContextDestroyed)?.call(None, &[base, slots, s.boolean(sw)])?;
    s.set_hidden(holder, MODULE, module)?;
    module.as_object().ok_or(JsError::ContextDestroyed)
}

pub(crate) fn cookie_event<'s>(
    s: &Scope<'s>,
    page: &Rc<PageDom>,
    sw: bool,
    changes: &[CookieChange],
) -> JsResult<Value<'s>> {
    let mut changed = RootedVec::new();
    let mut deleted = RootedVec::new();
    for change in changes {
        let item = s.object();
        item.set_prototype(s.null());
        match change {
            CookieChange::Changed(c) => {
                item.set_atom("name", s.string(&c.name))?;
                item.set_atom("value", s.string(&c.value))?;
                changed.push(s, item.value());
            }
            CookieChange::Deleted(c) => {
                item.set_atom("name", s.string(&c.name))?;
                deleted.push(s, item.value());
            }
        }
    }
    let init = s.object();
    init.set_prototype(s.null());
    init.set_atom("changed", s.array(&changed.iter(s).collect::<Vec<_>>())?.value())?;
    init.set_atom("deleted", s.array(&deleted.iter(s).collect::<Vec<_>>())?.value())?;
    let make = module(s, page, sw)?.get_atom("make")?.as_object().ok_or(JsError::ContextDestroyed)?;
    make.call(None, &[s.string(if sw { "cookiechange" } else { "change" }), init.value()])
}

/// Trusted delivery, bypassing EventTarget.dispatchEvent's mandatory untrust step.
pub(crate) fn dispatch_trusted(s: &Scope<'_>, receiver: Value<'_>, event: Value<'_>) -> JsResult<()> {
    let page = target::page_of(s)?;
    let key = if receiver.strict_equals(s.global().value()) {
        page.window_target()
    } else {
        s.unwrap::<EventTarget>(receiver).map(|t| t.key)
    }
    .ok_or(JsError::ContextDestroyed)?;
    dispatch_at(s, &page, key, event)
}

fn dispatch_at(s: &Scope<'_>, page: &PageDom, key: TargetKey, event: Value<'_>) -> JsResult<()> {
    // A listener receives the actual subclass instance with its traced arrays.
    let root = Persistent::new(s, event);
    let data = s.unwrap::<Event>(root.get(s)).ok_or(JsError::ContextDestroyed)?.clone();
    data.state.borrow_mut().is_trusted = true;
    target::fire(page, key, &data);
    Ok(())
}

type Inbox = Arc<Mutex<Vec<CookieChange>>>;

pub(crate) struct WindowObserver {
    realm: RealmId,
    target: Option<TargetKey>,
    jar: Weak<Mutex<CookieJar>>,
    subscription: u64,
    inbox: Inbox,
    has_poster: bool,
}

impl Drop for WindowObserver {
    fn drop(&mut self) {
        if let Some(jar) = self.jar.upgrade() {
            jar.lock().unwrap_or_else(|e| e.into_inner()).unobserve(self.subscription);
        }
    }
}

/// Observe the creation URL before author script runs. The JS CookieStore and
/// its EventTarget are still made lazily: a listener first installed between
/// the mutation and the delivery task can receive the queued change.
pub(crate) fn install_window(s: &Scope<'_>, page: &Rc<PageDom>) {
    let realm = s.realm_id();
    if page.cookie_observers.borrow().iter().any(|o| o.realm == realm) || !crate::secure_context::secure(s, page) {
        return;
    }
    let Ok(url) = super::environment(s, page) else { return };
    let Some(jar) = page.net.borrow().as_ref().map(|n| page.cookie_jar(n)) else { return };
    let inbox: Inbox = Arc::default();
    let queue = inbox.clone();
    let page_id = page.id();
    let partition = page.cookie_partition_key();
    let site = page.cookie_site_context();
    let poster = page.remote_poster();
    let has_poster = poster.is_some();
    let callback = Arc::new(move |change: CookieChange| {
        let cookie = match &change {
            CookieChange::Changed(c) | CookieChange::Deleted(c) => c,
        };
        if !cookie.visible_in(&url, partition.as_deref(), site) {
            return;
        }
        // Each committed change gets its own delivery task. A later change
        // to the same cookie must not join the earlier event's changed list.
        if let Some(post) = &poster {
            let queue = queue.clone();
            post(Box::new(move || deliver(page_id, realm, &queue, change)));
        } else {
            queue.lock().unwrap_or_else(|e| e.into_inner()).push(change);
        }
    });
    let subscription = jar.lock().unwrap_or_else(|e| e.into_inner()).observe(callback);
    page.cookie_observers.borrow_mut().push(WindowObserver {
        realm,
        target: None,
        jar: Arc::downgrade(&jar),
        subscription,
        inbox,
        has_poster,
    });
}

pub(crate) fn bind_target(s: &Scope<'_>, page: &Rc<PageDom>, store: Object<'_>) -> JsResult<()> {
    // Some embedders supply their network client after installing the realm.
    install_window(s, page);
    let key = s.unwrap::<EventTarget>(store.value()).ok_or(JsError::ContextDestroyed)?.key;
    if let Some(observer) = page.cookie_observers.borrow_mut().iter_mut().find(|o| o.realm == s.realm_id()) {
        observer.target = Some(key);
    }
    Ok(())
}

fn deliver(page_id: u64, realm_id: RealmId, inbox: &Inbox, change: CookieChange) {
    let Some(realm) = Realm::by_id(realm_id) else { return };
    let Ok(s) = realm.scope() else { return };
    let Ok(page) = target::page_of(&s) else { return };
    if page.id() != page_id || !page.is_fully_active() {
        return;
    }
    let key = page
        .cookie_observers
        .borrow()
        .iter()
        .find(|o| o.realm == realm_id && Arc::ptr_eq(&o.inbox, inbox))
        .and_then(|o| o.target);
    let Some(key) = key else { return };
    if !page.dom.borrow().has_listener_for(key, "change") {
        return;
    }
    let result = cookie_event(&s, &page, false, std::slice::from_ref(&change))
        .and_then(|event| dispatch_at(&s, &page, key, event));
    if let Err(error) = result {
        s.report_exception(&error);
    }
}

/// Embedders without a remote poster drain their inbox at the next page
/// checkpoint, after any network or script cookie mutation has returned.
pub(crate) fn after_task(page: &PageDom) {
    let fallback: Vec<_> = {
        let observers = page.cookie_observers.borrow();
        if observers.is_empty() {
            return;
        }
        observers
            .iter()
            .filter(|o| !o.has_poster)
            .filter_map(|o| {
                let changes = std::mem::take(&mut *o.inbox.lock().unwrap_or_else(|e| e.into_inner()));
                (!changes.is_empty()).then(|| (o.realm, o.inbox.clone(), changes))
            })
            .collect()
    };
    let id = page.id();
    for (realm, inbox, changes) in fallback {
        for change in changes {
            let inbox = inbox.clone();
            page.post_task(Box::new(move || deliver(id, realm, &inbox, change)));
        }
    }
}

pub(crate) fn detach(page: &PageDom, realm: RealmId) {
    page.cookie_observers.borrow_mut().retain(|o| o.realm != realm);
}
