//! MutationObserver bindings over lp-dom's native registration/record queues.
//! Callbacks and observer-to-node links are GC-traced JS properties, never
//! native self-roots. Only connected observed nodes and pending deliveries pin
//! wrappers. Safe-point scans inspect registered targets, not whole documents.
mod record;
use crate::{target::page_of, util::DomResultExt, PageDom};
use lp_dom::{
    mutation::{ObserveInit, ObserverId},
    NodeId,
};
use lp_js::{
    ClassDef, IdentityKey, JsClass, JsError, JsResult, NoParent, Object, Persistent, RealmId, Scope, Value, WeakRealm,
};
use rustc_hash::FxHashMap;
use std::{
    cell::{Cell, RefCell},
    rc::{Rc, Weak},
};
const CALLBACK: &str = "__lpMutationCallback";
const LINKS: &str = "__lpMutationObservers";

#[derive(Default)]
pub(crate) struct BindingState {
    owner: RefCell<Weak<PageDom>>,
    observers: RefCell<FxHashMap<ObserverId, Weak<Observer>>>,
    pins: RefCell<FxHashMap<(RealmId, NodeId), Persistent>>,
    pending_roots: RefCell<FxHashMap<ObserverId, Persistent>>,
    pending_drops: RefCell<Vec<ObserverId>>,
    helpers: RefCell<FxHashMap<RealmId, Persistent>>,
    queued: Cell<Option<(RealmId, u64)>>,
    generation: Cell<u64>,
    rooted: Cell<usize>,
    version: Cell<Option<u64>>,
    /// Bumped at every change of this struct that [`hook_pending`] looks at;
    /// with `Dom::hook_epoch`, the pair the hook last found nothing at.
    epoch: Cell<u64>,
    quiet: Cell<Option<(u64, u64)>>,
}
impl BindingState {
    /// Something `hook_pending` reads changed (see `quiet`).
    pub(crate) fn touch(&self) {
        self.epoch.set(self.epoch.get().wrapping_add(1));
    }
    pub(crate) fn pending_drops_is_empty(&self) -> bool {
        self.pending_drops.borrow().is_empty()
    }
}
pub struct Observer {
    page: Weak<PageDom>,
    realm: WeakRealm,
    realm_id: RealmId,
    identity: IdentityKey,
    id: Cell<Option<ObserverId>>,
    targets: RefCell<Vec<NodeId>>,
}
impl Drop for Observer {
    fn drop(&mut self) {
        if let (Some(page), Some(id)) = (self.page.upgrade(), self.id.take()) {
            page.mutations.observers.borrow_mut().remove(&id);
            page.mutations.rooted.set(0);
            match page.dom.try_borrow_mut() {
                Ok(mut dom) => dom.drop_observer(id),
                Err(_) => page.mutations.pending_drops.borrow_mut().push(id),
            }
            page.mutations.touch();
        }
    }
}
impl Observer {
    fn page(&self) -> JsResult<Rc<PageDom>> {
        self.page.upgrade().filter(|p| !p.is_disposed() && self.id.get().is_some()).ok_or(JsError::ContextDestroyed)
    }
    fn wrapper<'s>(&self, s: &Scope<'s>) -> JsResult<Object<'s>> {
        s.lookup_identity(self.identity).ok_or(JsError::ContextDestroyed)
    }
    fn disconnect(&self) -> JsResult<()> {
        let page = self.page()?;
        page.dom.borrow_mut().disconnect_observer(self.id.get().unwrap());
        let targets = self.targets.take();
        if let Some(realm) = self.realm.upgrade() {
            let scope = realm.scope()?;
            for node in targets {
                unlink(&scope, &page, node, self.identity)?;
            }
        }
        page.mutations.version.set(None);
        Ok(())
    }
}
pub struct MutationObserver;
impl JsClass for MutationObserver {
    const NAME: &'static str = "MutationObserver";
    type Data = Rc<Observer>;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(1, |s, nt, args| {
            args.require(1, "MutationObserver")?;
            let callback = args
                .get(0)
                .as_object()
                .filter(|v| v.is_function())
                .ok_or_else(|| JsError::Type("MutationObserver callback must be a function".into()))?;
            let page = page_of(s)?;
            if page.is_disposed() {
                return Err(JsError::ContextDestroyed);
            }
            let id = page.dom.borrow_mut().create_observer();
            let identity = page.event_identity(page.next_event_id());
            let observer = Rc::new(Observer {
                page: Rc::downgrade(&page),
                realm: s.realm().downgrade(),
                realm_id: s.realm_id(),
                identity,
                id: Cell::new(Some(id)),
                targets: RefCell::new(Vec::new()),
            });
            page.mutations.observers.borrow_mut().insert(id, Rc::downgrade(&observer));
            page.mutations.touch();
            let object = s.wrap_new::<Self>(observer, nt, Some(identity))?;
            s.set_hidden(object, CALLBACK, callback.value())?;
            Ok(object)
        });
        def.method("observe", 1, |s, observer, args| {
            args.require(1, "MutationObserver.observe")?;
            let node = s
                .unwrap::<crate::node::Node>(args.get(0))
                .ok_or_else(|| JsError::Type("observe target must be a Node".into()))?;
            let page = observer.page()?;
            if node.page().id() != page.id() {
                return Err(JsError::dom("NotSupportedError", "Observing a different PageDom is not implemented"));
            }
            let options = parse_options(s, &page, args.get(1))?;
            // Options may reenter or dispose the document. No DOM/state borrow
            // crosses a user getter, iterable, or string conversion.
            let page = observer.page()?;
            page.dom.borrow_mut().observe(observer.id.get().unwrap(), node.id, options).js()?;
            let realm = observer.realm.upgrade().ok_or(JsError::ContextDestroyed)?;
            let owner = realm.scope()?;
            link(&owner, &page, node.id, observer.wrapper(&owner)?)?;
            if !observer.targets.borrow().contains(&node.id) {
                observer.targets.borrow_mut().push(node.id);
            }
            page.mutations.version.set(None);
            pin_connected(&owner, &page, node.id)?;
            Ok(s.undefined())
        });
        def.method("disconnect", 0, |s, observer, _| {
            observer.disconnect()?;
            Ok(s.undefined())
        });
        def.method("takeRecords", 0, |s, observer, _| {
            let page = observer.page()?;
            record::array(s, record::take(&page, observer.id.get().unwrap())?)
        });
    }
}
fn links<'s>(s: &Scope<'s>, object: Object<'s>, create: bool) -> JsResult<Option<Object<'s>>> {
    if let Some(array) = s.get_hidden(object, LINKS)?.and_then(|v| v.as_object()) {
        return Ok(Some(array));
    }
    if !create {
        return Ok(None);
    }
    let array = s.array(&[])?;
    array.set_prototype(s.null());
    s.set_hidden(object, LINKS, array.value())?;
    Ok(Some(array))
}
fn link(s: &Scope<'_>, page: &PageDom, node: NodeId, observer: Object<'_>) -> JsResult<()> {
    let node = crate::node::wrap_node(s, page, node)?;
    let array = links(s, node, true)?.unwrap();
    let len = array.get_atom("length")?.to_number()? as u32;
    let mut free = len;
    for i in 0..len {
        let value = array.get_index(i)?;
        if value.strict_equals(observer.value()) {
            return Ok(());
        }
        if value.is_undefined() {
            free = free.min(i);
        }
    }
    array.set_index(free, observer.value())
}
fn unlink(s: &Scope<'_>, page: &PageDom, node: NodeId, identity: IdentityKey) -> JsResult<()> {
    let Some(node) = page.node_wrapper(s, node) else { return Ok(()) };
    let Some(array) = links(s, node, false)? else { return Ok(()) };
    let Some(observer) = s.lookup_identity(identity) else { return Ok(()) };
    let len = array.get_atom("length")?.to_number()? as u32;
    for i in 0..len {
        if array.get_index(i)?.strict_equals(observer.value()) {
            array.set_index(i, s.undefined())?;
        }
    }
    Ok(())
}
fn pin_connected(s: &Scope<'_>, page: &PageDom, node: NodeId) -> JsResult<()> {
    if page.dom.borrow().is_in_active_document(node) {
        let object = crate::node::wrap_node(s, page, node)?;
        page.mutations
            .pins
            .borrow_mut()
            .entry((s.realm_id(), node))
            .or_insert_with(|| Persistent::new(s, object.value()));
        page.mutations.touch();
    }
    Ok(())
}
fn parse_options(s: &Scope<'_>, page: &PageDom, value: Value<'_>) -> JsResult<ObserveInit> {
    let helper = page.mutations.helpers.borrow().get(&s.realm_id()).cloned().ok_or(JsError::ContextDestroyed)?;
    let array = helper.get(s).as_object().unwrap().call(None, &[value])?.as_object().unwrap();
    let optional = |i| -> JsResult<Option<bool>> {
        let value = array.get_index(i)?;
        Ok((!value.is_undefined()).then(|| value.to_bool()))
    };
    let filter = array.get_index(0)?;
    let attribute_filter = if filter.is_undefined() {
        None
    } else {
        let list = filter.as_object().unwrap();
        let n = list.get_atom("length")?.to_number()? as u32;
        Some((0..n).map(|i| list.get_index(i)?.to_string()).collect::<JsResult<Vec<_>>>()?)
    };
    Ok(ObserveInit {
        attribute_filter,
        attribute_old_value: optional(1)?,
        attributes: optional(2)?,
        character_data: optional(3)?,
        character_data_old_value: optional(4)?,
        child_list: array.get_index(5)?.to_bool(),
        subtree: array.get_index(6)?.to_bool(),
    })
}

/// Called after a native callback (once its borrows have dropped), and from
/// PageDom.after_task for parser/native mutations. No observer callback runs
/// inline here: Promise reactions provide coalesced microtask delivery.
pub fn checkpoint(page: &Rc<PageDom>) {
    if page.is_disposed() {
        return;
    }
    let slots_signaled = page.dom.try_borrow().is_ok_and(|d| d.has_signaled_slots());
    // No observer, nothing pinned, nothing to drop: nothing to do (this runs
    // after every DOM call from script).
    if page.mutations.observers.borrow().is_empty()
        && page.mutations.pins.borrow().is_empty()
        && page.mutations.pending_drops.borrow().is_empty()
        && page.mutations.pending_roots.borrow().is_empty()
        && !slots_signaled
    {
        return;
    }
    let (new_pending, version) = {
        let Ok(mut dom) = page.dom.try_borrow_mut() else { return };
        for id in page.mutations.pending_drops.take() {
            dom.drop_observer(id);
            page.mutations.rooted.set(0);
        }
        (dom.pending_observers_from(page.mutations.rooted.get()), dom.version())
    };
    page.mutations.rooted.set(page.mutations.rooted.get() + new_pending.len());
    page.mutations.touch();
    let mut scheduling_realm = None;
    for id in new_pending {
        let observer = page.mutations.observers.borrow().get(&id).and_then(Weak::upgrade);
        let Some(observer) = observer else { continue };
        let Some(realm) = observer.realm.upgrade() else { continue };
        let Ok(s) = realm.scope() else { continue };
        if let Ok(wrapper) = observer.wrapper(&s) {
            page.mutations.pending_roots.borrow_mut().entry(id).or_insert_with(|| Persistent::new(&s, wrapper.value()));
            scheduling_realm = Some(realm.clone());
        }
    }
    if (!page.mutations.pending_roots.borrow().is_empty() || slots_signaled) && page.mutations.queued.get().is_none()
    {
        let realm = scheduling_realm
            .or_else(|| {
                page.mutations
                    .observers
                    .borrow()
                    .values()
                    .filter_map(Weak::upgrade)
                    .find_map(|observer| observer.realm.upgrade())
            })
            .or_else(|| page.main_world_realm());
        if let Some(realm) = realm {
            if let Ok(s) = realm.scope() {
                let generation = page.mutations.generation.get().wrapping_add(1);
                page.mutations.generation.set(generation);
                let ticket = (realm.id(), generation);
                page.mutations.queued.set(Some(ticket));
                page.mutations.touch();
                let weak = Rc::downgrade(page);
                if let Err(error) = s.then(
                    s.undefined(),
                    move |_, _| {
                        if let Some(page) = weak.upgrade() {
                            if page.mutations.queued.get() == Some(ticket) {
                                notify(&page)
                            }
                        }
                    },
                    |_, _| {},
                ) {
                    page.mutations.queued.set(None);
                    page.mutations.touch();
                    s.report_exception(&error);
                }
            }
        }
    }
    // Reconcile only observed targets when the native tree changes, and only
    // when one of them was connected or disconnected (checked first without
    // allocating: most mutations leave every target where it was).
    if page.mutations.version.replace(Some(version)) != Some(version) && pins_out_of_date(page) {
        let observers: Vec<_> = page.mutations.observers.borrow().values().filter_map(Weak::upgrade).collect();
        let mut needed = FxHashMap::default();
        for observer in observers {
            let Some(realm) = observer.realm.upgrade() else { continue };
            let Ok(s) = realm.scope() else { continue };
            let targets = observer.targets.borrow().clone();
            for node in targets {
                if page.dom.borrow().is_in_active_document(node) {
                    needed.insert((realm.id(), node), ());
                    let _ = pin_connected(&s, page, node);
                }
            }
        }
        let stale: Vec<_> = page.mutations.pins.borrow().keys().filter(|k| !needed.contains_key(k)).copied().collect();
        for key in stale {
            let old = page.mutations.pins.borrow_mut().remove(&key);
            drop(old);
        }
    }
}
/// Whether some observed target's pin no longer matches whether it is in
/// the active document (or a pin outlived its target), so `checkpoint` must
/// reconcile. Runs no script and allocates nothing.
fn pins_out_of_date(page: &PageDom) -> bool {
    let Ok(dom) = page.dom.try_borrow() else { return true };
    let pins = page.mutations.pins.borrow();
    let observers = page.mutations.observers.borrow();
    let mut pinned_targets = 0;
    for observer in observers.values() {
        let Some(observer) = observer.upgrade() else { return true };
        let Ok(targets) = observer.targets.try_borrow() else { return true };
        for &node in targets.iter() {
            let pinned = pins.contains_key(&(observer.realm_id, node));
            if pinned != dom.is_in_active_document(node) {
                return true;
            }
            pinned_targets += usize::from(pinned);
        }
    }
    // Pins of targets no observer has any more.
    pins.len() != pinned_targets
}

fn notify(page: &Rc<PageDom>) {
    if page.is_disposed() {
        return;
    }
    page.mutations.queued.set(None);
    page.mutations.rooted.set(0);
    page.mutations.touch();
    // "Signal a slot change": slotchange at each signaled slot first.
    let slots = page.dom.borrow_mut().take_signaled_slots();
    for slot in slots {
        crate::input::fire_simple(page, slot, "slotchange", true, false);
    }
    let pending = page.dom.borrow_mut().take_pending_observers();
    let roots = page.mutations.pending_roots.take();
    // Snapshot binding identities as well as native ids: a callback can dispose
    // another realm and create an observer that reuses its native slot.
    let observers: Vec<_> = pending
        .into_iter()
        .filter_map(|id| {
            page.mutations.observers.borrow().get(&id).and_then(Weak::upgrade).map(|observer| (id, observer))
        })
        .collect();
    for (id, observer) in observers {
        if observer.id.get() != Some(id) {
            continue;
        }
        page.dom.borrow_mut().clear_transient(id);
        let Some(realm) = observer.realm.upgrade() else { continue };
        let Ok(s) = realm.scope() else { continue };
        // Web IDL "invoke a callback function": an exception of the callback
        // is reported to the global of the callback's own realm, which is not
        // the observer's when the function came from another window.
        let mut callback_realm = None;
        let result = (|| {
            let records = record::take(page, id)?;
            if records.is_empty() {
                return Ok(());
            }
            let wrapper = roots
                .get(&id)
                .map(|p| p.get(&s))
                .and_then(|v| v.as_object())
                .or_else(|| s.lookup_identity(observer.identity))
                .ok_or(JsError::ContextDestroyed)?;
            let callback =
                s.get_hidden(wrapper, CALLBACK)?.and_then(|v| v.as_object()).ok_or(JsError::ContextDestroyed)?;
            let records = record::array(&s, records)?;
            callback_realm = s.object_realm(callback).filter(|r| r.id() != s.realm_id());
            callback.call(Some(wrapper), &[records, wrapper.value()])?;
            Ok(())
        })();
        if let Err(error) = result {
            match callback_realm.as_ref().and_then(|r| r.scope().ok()) {
                Some(owner) => owner.report_exception(&error),
                None => s.report_exception(&error),
            }
        }
    }
    drop(roots);
    // Native-only embedders can cause mutations inside a callback without a
    // runtime hook; make sure those also get a subsequent notification.
    checkpoint(page);
}
pub(crate) fn after_task(page: &PageDom) {
    let owner = page.mutations.owner.borrow().upgrade();
    if let Some(page) = owner {
        checkpoint(&page)
    }
}
pub(crate) fn install(s: &Scope<'_>, page: &Rc<PageDom>) -> JsResult<()> {
    *page.mutations.owner.borrow_mut() = Rc::downgrade(page);
    let helper=s.eval(r#"(()=>{const str=String, TypeError_=TypeError, create=Object.create;
        return options=>{
            if(options==null)options={};
            if(typeof options!=='object'&&typeof options!=='function')throw new TypeError_('Observer options must be a dictionary');
            let filter=options.attributeFilter;
            if(filter!==undefined){
                if(filter===null||(typeof filter!=='object'&&typeof filter!=='function'))throw new TypeError_('attributeFilter must be a sequence');
                const list=create(null);let n=0;for(const item of filter){if(typeof item==='symbol')throw new TypeError_('Cannot convert symbol to string');list[n++]=str(item);}list.length=n;filter=list;
            }
            const old=options.attributeOldValue, attributes=options.attributes, chars=options.characterData, charOld=options.characterDataOldValue;
            return [filter,old===undefined?undefined:!!old,attributes===undefined?undefined:!!attributes,chars===undefined?undefined:!!chars,charOld===undefined?undefined:!!charOld,!!options.childList,!!options.subtree];
        };})()"#,None,1)?;
    page.mutations.helpers.borrow_mut().insert(s.realm_id(), Persistent::new(s, helper));
    let weak = Rc::downgrade(page);
    s.realm().set_native_callback_hook(Some(Rc::new(move |_| {
        if let Some(page) = weak.upgrade() {
            page.flush_iframe_events();
            page.update_render_blocking();
            crate::custom::run_reactions(&page);
            crate::custom::run_upgrades(&page);
            checkpoint(&page);
        }
    })))?;
    let weak = Rc::downgrade(page);
    s.realm()
        .set_native_callback_pending(Some(Rc::new(move || weak.upgrade().is_some_and(|page| hook_pending(&page)))))?;
    Ok(())
}

/// Whether the native callback hook (iframe events, then [`checkpoint`]) has
/// anything to do. Conservative: true whenever either could act.
fn hook_pending(page: &PageDom) -> bool {
    if page.is_disposed() {
        return false;
    }
    let Ok(dom) = page.dom.try_borrow() else { return true };
    let m = &page.mutations;
    // Every condition below changes only with `Dom::hook_epoch` (an entry in
    // one of the queues the hook drains), `epoch` (this struct) or, for the
    // two passes that follow every mutation, `Dom::version`: a pair the hook
    // already found quiet needs no second look while neither pass looks at
    // the tree. Native callbacks come in bursts of tens of thousands, most
    // of them mutating nothing the hook drains.
    let now = (dom.hook_epoch(), m.epoch.get());
    if m.quiet.get() == Some(now) && !follows_version(page, &dom, m) {
        return false;
    }
    let pending = hook_pending_slow(page, &dom, m);
    if !pending {
        m.quiet.set(Some(now));
    }
    pending
}

/// Whether a pass of the hook that runs after any mutation would look at the
/// tree again: the render-blocking evaluation while the set can change, and
/// the reconcile pass of `checkpoint` while observers or pins exist.
fn follows_version(page: &PageDom, dom: &lp_dom::Dom, m: &BindingState) -> bool {
    page.render_blocking_follows_version(dom)
        || (m.version.get() != Some(dom.version()) && (!m.observers.borrow().is_empty() || !m.pins.borrow().is_empty()))
}

fn hook_pending_slow(page: &PageDom, dom: &lp_dom::Dom, m: &BindingState) -> bool {
    dom.has_iframe_events()
        || dom.has_details_toggles()
        || page.render_blocking_pending(dom)
        || dom.has_custom_reactions_for(page.document())
        || dom.has_upgrade_candidates()
        || (dom.has_signaled_slots() && m.queued.get().is_none())
        || !m.pending_drops.borrow().is_empty()
        || dom.has_pending_observers_from(m.rooted.get())
        || (!m.pending_roots.borrow().is_empty() && m.queued.get().is_none())
        // The reconcile pass of `checkpoint` only acts on observers or pins;
        // it is idempotent, so a version it skipped is reconciled later.
        || (m.version.get() != Some(dom.version()) && (!m.observers.borrow().is_empty() || !m.pins.borrow().is_empty()))
}
pub(crate) fn cancel_realm(page: &PageDom, realm: RealmId) {
    if page.mutations.queued.get().is_some_and(|(owner, _)| owner == realm) {
        page.mutations.queued.set(None);
    }
    page.mutations.touch();
    let observers: Vec<_> =
        page.mutations.observers.borrow().values().filter_map(Weak::upgrade).filter(|o| o.realm_id == realm).collect();
    for observer in observers {
        if let Some(id) = observer.id.take() {
            page.dom.borrow_mut().drop_observer(id);
            page.mutations.observers.borrow_mut().remove(&id);
            let old = page.mutations.pending_roots.borrow_mut().remove(&id);
            drop(old);
        }
        observer.targets.borrow_mut().clear();
    }
    page.mutations.rooted.set(0);
    let keys: Vec<_> = page.mutations.pins.borrow().keys().filter(|(r, _)| *r == realm).copied().collect();
    for key in keys {
        let old = page.mutations.pins.borrow_mut().remove(&key);
        drop(old);
    }
    page.mutations.helpers.borrow_mut().remove(&realm);
}
pub(crate) fn dispose(page: &PageDom) {
    let realms: Vec<_> = page.mutations.helpers.borrow().keys().copied().collect();
    for realm in realms {
        cancel_realm(page, realm)
    }
    drop(page.mutations.pending_roots.take());
    drop(page.mutations.pins.take());
}
pub(crate) fn register(engine: &lp_js::Engine) {
    engine.register::<MutationObserver>();
    engine.register::<record::MutationRecord>();
}
