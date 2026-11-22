//! The cookie subscription list belongs to the service worker registration,
//! so versions and client realms share it until unregistration.

use std::rc::Rc;
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use pyro_js::{JsError, JsResult, Object, Scope, Value};
use pyro_net::cookies::{CookieChange, SiteContext};
use pyro_net::{CookieJar, Url};
use pyro_runtime::LoopHandle;

use crate::page::PageDom;
use crate::worker_scope::WorkerLoop;

#[derive(Clone, PartialEq)]
struct Subscription {
    name: Option<String>,
    url: Url,
}

#[derive(Default)]
struct State {
    list: Vec<Subscription>,
    target: Option<(u64, LoopHandle<WorkerLoop>)>,
    activated: bool,
    pending: Vec<CookieChange>,
    scheduled: bool,
    removed: bool,
}

pub(crate) struct Subscriptions {
    scope: Url,
    partition: Option<String>,
    site: SiteContext,
    jar: Option<Weak<Mutex<CookieJar>>>,
    observer: Mutex<Option<u64>>,
    state: Mutex<State>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

impl Subscriptions {
    pub(crate) fn new(
        scope: Url,
        partition: Option<String>,
        site: SiteContext,
        jar: Option<&Arc<Mutex<CookieJar>>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            scope,
            partition,
            site,
            jar: jar.map(Arc::downgrade),
            observer: Mutex::default(),
            state: Mutex::default(),
        })
    }

    fn watch(self: &Arc<Self>) {
        let mut observer = lock(&self.observer);
        if observer.is_some() {
            return;
        }
        let state = lock(&self.state);
        if state.removed || state.list.is_empty() {
            return;
        }
        drop(state);
        let Some(jar) = self.jar.as_ref().and_then(Weak::upgrade) else { return };
        let weak = Arc::downgrade(self);
        *observer = Some(lock(&jar).observe(Arc::new(move |change| {
            if let Some(owner) = weak.upgrade() {
                owner.changed(change);
            }
        })));
    }

    fn changed(self: &Arc<Self>, change: CookieChange) {
        let cookie = match &change {
            CookieChange::Changed(c) | CookieChange::Deleted(c) => c,
        };
        let mut state = lock(&self.state);
        if state.removed
            || state.target.is_none()
            || state.list.is_empty()
            || !cookie.visible_in(&self.scope, self.partition.as_deref(), self.site)
            || !state.list.iter().any(|sub| {
                sub.name.as_ref().is_none_or(|n| n == &cookie.name)
                    && cookie.visible_in(&sub.url, self.partition.as_deref(), self.site)
            })
        {
            return;
        }
        state.pending.push(change);
        self.schedule(&mut state);
    }

    fn schedule(self: &Arc<Self>, state: &mut State) {
        if !state.activated || state.scheduled || state.pending.is_empty() {
            return;
        }
        let Some((id, handle)) = state.target.as_ref() else { return };
        let version = *id;
        let weak = Arc::downgrade(self);
        state.scheduled = handle
            .post(move |worker| {
                let Some(owner) = weak.upgrade() else { return };
                let changes = {
                    let mut state = lock(&owner.state);
                    if state.target.as_ref().is_none_or(|(id, _)| *id != version) {
                        return;
                    }
                    state.scheduled = false;
                    std::mem::take(&mut state.pending)
                };
                if !changes.is_empty() {
                    worker.service(|s, page| crate::service_worker_scope::cookie_change(s, page, &changes));
                }
            })
            .is_ok();
        if !state.scheduled {
            state.pending.clear();
        }
    }

    pub(crate) fn activating(&self, version: u64, handle: LoopHandle<WorkerLoop>) {
        let mut state = lock(&self.state);
        if state.removed {
            return;
        }
        state.target = Some((version, handle));
        state.activated = false;
        state.scheduled = false;
    }

    pub(crate) fn activated(self: &Arc<Self>) {
        let mut state = lock(&self.state);
        state.activated = true;
        self.schedule(&mut state);
    }

    pub(crate) fn clear(&self) {
        let mut observer = lock(&self.observer);
        {
            let mut state = lock(&self.state);
            if state.removed { return; }
            *state = State { removed: true, ..State::default() };
        }
        if let (Some(id), Some(jar)) = (observer.take(), self.jar.as_ref().and_then(Weak::upgrade)) {
            lock(&jar).unobserve(id);
        }
    }

    fn modify(self: &Arc<Self>, add: bool, base: &Url, input: Vec<(Option<String>, Option<String>)>) -> JsResult<()> {
        let mut state = lock(&self.state);
        // Methods operate on their manager's associated list. After
        // unregistration it is detached: watch() cannot restart delivery.
        for (name, input) in input {
            let url = match input {
                Some(text) => {
                    base.join(&text).ok().filter(|url| url.as_str().starts_with(self.scope.as_str())).ok_or_else(|| {
                        JsError::Type("The subscription URL is outside the registration's scope.".into())
                    })
                }
                None => Ok(self.scope.clone()),
            };
            let url = match url {
                Ok(url) => url,
                Err(error) => {
                    drop(state);
                    self.watch();
                    return Err(error);
                }
            };
            let sub = Subscription { name: name.map(|n| n.trim_matches([' ', '\t']).to_owned()), url };
            if add {
                if !state.list.contains(&sub) {
                    state.list.push(sub);
                }
            } else {
                state.list.retain(|existing| existing != &sub);
            }
        }
        drop(state);
        self.watch();
        Ok(())
    }
}

fn module<'s>(s: &Scope<'s>, page: &Rc<PageDom>) -> JsResult<Object<'s>> {
    let holder = crate::worker_scope::module_holder(s, page)?;
    const KEY: &str = "cookieStoreManagerModule";
    if let Some(object) = s.get_hidden(holder, KEY)?.and_then(|v| v.as_object()) {
        return Ok(object);
    }
    let factory = s.eval(include_str!("subscriptions.js"), Some("pyro://internal/cookie-store-manager.js"), 1)?;
    let slot = crate::streams::shared_slot(s, page, "CookieStoreManager.registration")?;
    let value = factory.as_object().ok_or(JsError::ContextDestroyed)?.call(None, &[slot])?;
    s.set_hidden(holder, KEY, value)?;
    value.as_object().ok_or(JsError::ContextDestroyed)
}

pub(crate) fn manager_constructor<'s>(s: &Scope<'s>, page: &Rc<PageDom>) -> JsResult<Value<'s>> {
    module(s, page)?.get_atom("CookieStoreManager")
}

pub(crate) fn manager<'s>(
    s: &Scope<'s>,
    page: &Rc<PageDom>,
    subscriptions: Arc<Subscriptions>,
    worker_url: Option<Url>,
) -> JsResult<Value<'s>> {
    let factory = module(s, page)?.get_atom("manager")?.as_object().ok_or(JsError::ContextDestroyed)?;
    let owner = Rc::downgrade(page);
    let operation = s.function("cookieSubscriptions", move |s, _, a| {
        let kind = a.get(0).to_string()?;
        if kind == "list" {
            let list = lock(&subscriptions.state).list.clone();
            let array = s.array(&[])?;
            for (index, sub) in list.iter().enumerate() {
                let object = s.object();
                if let Some(name) = &sub.name {
                    object.set_atom("name", s.string(name))?;
                }
                object.set_atom("url", s.string(sub.url.as_str()))?;
                array.set_index(index as u32, object.value())?;
            }
            return Ok(array.value());
        }
        let page = owner.upgrade().ok_or(JsError::ContextDestroyed)?;
        let base = worker_url
            .clone()
            .or_else(|| page.dom.borrow().document_base_url(page.document()).ok().and_then(|s| Url::parse(s).ok()))
            .ok_or_else(|| JsError::Type("Invalid subscription base URL.".into()))?;
        let input = a.get(1).as_object().ok_or(JsError::ContextDestroyed)?;
        let count = input.get_atom("length")?.to_number()? as u32;
        let mut entries = Vec::new();
        for i in 0..count {
            let entry = input.get_index(i)?.as_object().ok_or(JsError::ContextDestroyed)?;
            let (n, u) = (entry.get_index(0)?, entry.get_index(1)?);
            entries.push((
                (!n.is_null()).then(|| n.to_string()).transpose()?,
                (!u.is_null()).then(|| u.to_string()).transpose()?,
            ));
        }
        subscriptions.modify(kind == "subscribe", &base, entries)?;
        Ok(s.undefined())
    })?;
    factory.call(None, &[operation.value()])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subscription_list_preserves_scope_order_and_registration_lifetime() {
        let scope = Url::parse("https://app.test/work/").unwrap();
        let base = Url::parse("https://app.test/document").unwrap();
        let list = Subscriptions::new(scope.clone(), Some("https://app.test".into()), SiteContext::SameSite, None);
        list.modify(true, &base, vec![(Some("  id\t".into()), None), (None, Some("/work/child".into()))]).unwrap();
        list.modify(true, &base, vec![(Some("id".into()), Some("/work/".into()))]).unwrap();
        {
            let state = lock(&list.state);
            assert_eq!(state.list.len(), 2);
            assert_eq!(state.list[0].name.as_deref(), Some("id"));
            assert_eq!(state.list[0].url, scope);
            assert!(state.list[1].name.is_none());
            assert_eq!(state.list[1].url.path(), "/work/child");
        }
        assert!(list.modify(true, &base, vec![(None, Some("/different/".into()))]).is_err());
        list.modify(false, &base, vec![(Some("id".into()), None)]).unwrap();
        assert_eq!(lock(&list.state).list.len(), 1);
        list.clear();
        assert!(lock(&list.state).list.is_empty());
        list.modify(false, &base, vec![(Some("id".into()), None)]).unwrap();
        list.modify(true, &base, vec![(None, None)]).unwrap();
        assert!(lock(&list.state).removed);
        assert_eq!(lock(&list.state).list.len(), 1);
        assert!(lock(&list.observer).is_none());
    }
}
