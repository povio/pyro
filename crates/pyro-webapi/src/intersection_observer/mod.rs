//! Task-delivered intersection observations over the native faux layout.
mod geometry;
mod options;
use crate::{target::page_of, util::DomResultExt, PageDom};
use pyro_dom::{layout::Rect, NodeId};
use pyro_js::{
    ClassDef, IdentityKey, JsClass, JsError, JsResult, NoParent, Persistent, RealmId, Scope, Value, WeakRealm,
};
use rustc_hash::FxHashMap;
use std::{
    cell::{Cell, RefCell},
    rc::{Rc, Weak},
    time::Instant,
};

const CALLBACK: &str = "__lpIntersectionCallback";
const THRESHOLDS: &str = "__lpIntersectionThresholds";
type Stamp = (NodeId, ((u64, u64, u32, u32), u64));
#[derive(Default)]
pub(crate) struct BindingState {
    owner: RefCell<Weak<PageDom>>,
    observers: RefCell<Vec<Weak<Observer>>>,
    // Active and pending observers must survive JS GC. Explicit page/realm
    // disposal releases callback captures, including callbacks capturing DOM.
    pins: RefCell<FxHashMap<u64, Persistent>>,
    helpers: RefCell<FxHashMap<RealmId, Persistent>>,
    stamp: Cell<Option<Stamp>>,
    queued: Cell<bool>,
    origin: Cell<Option<Instant>>,
}
impl BindingState {
    pub(crate) fn has_observers(&self) -> bool {
        !self.observers.borrow().is_empty()
    }
}
struct NodeLease {
    page: Weak<PageDom>,
    id: NodeId,
}
impl NodeLease {
    fn new(page: &Rc<PageDom>, id: NodeId) -> JsResult<Self> {
        page.dom.borrow_mut().add_ref(id).js()?;
        Ok(Self { page: Rc::downgrade(page), id })
    }
}
impl NodeLease {
    fn is(&self, page: &Rc<PageDom>, id: NodeId) -> bool {
        self.id == id && std::ptr::eq(self.page.as_ptr(), Rc::as_ptr(page))
    }
}
impl Drop for NodeLease {
    fn drop(&mut self) {
        if let Some(page) = self.page.upgrade() {
            page.release_node(self.id);
        }
    }
}
struct Observation {
    node: NodeLease,
    previous: Option<(usize, bool, bool)>,
}
#[derive(Clone, Copy)]
struct Geometry {
    bounds: Rect,
    target: Rect,
    intersection: Rect,
    intersecting: bool,
    ratio: f64,
    /// `isVisible` (IntersectionObserver v2): tracked, intersecting, and not affected by an
    /// opacity, filter or non-translation transform nor covered by another element.
    visible: bool,
}
struct PendingEntry {
    target: NodeLease,
    geometry: Geometry,
    time: f64,
}
pub struct Observer {
    page: Weak<PageDom>,
    realm: WeakRealm,
    realm_id: RealmId,
    identity: IdentityKey,
    id: u64,
    live: Cell<bool>,
    options: options::Options,
    targets: RefCell<Vec<Observation>>,
    records: RefCell<Vec<PendingEntry>>,
    /// When (ms since the page origin) the last entry of a tracking observer was queued, for `delay`.
    last_queued: Cell<Option<f64>>,
    /// A timer re-running the observation after `delay` is pending.
    retry: Cell<bool>,
}
impl Observer {
    fn page(&self) -> JsResult<Rc<PageDom>> {
        self.page.upgrade().filter(|p| !p.is_disposed() && self.live.get()).ok_or(JsError::ContextDestroyed)
    }
    fn reconcile(&self, page: &PageDom) -> JsResult<()> {
        if self.targets.borrow().is_empty() && self.records.borrow().is_empty() {
            let old = page.intersections.pins.borrow_mut().remove(&self.id);
            drop(old);
        } else if !page.intersections.pins.borrow().contains_key(&self.id) {
            let realm = self.realm.upgrade().ok_or(JsError::ContextDestroyed)?;
            let s = realm.scope()?;
            let wrapper = s.lookup_identity(self.identity).ok_or(JsError::ContextDestroyed)?;
            page.intersections.pins.borrow_mut().insert(self.id, Persistent::new(&s, wrapper.value()));
        }
        Ok(())
    }
    /// The target element and the document bindings it belongs to: any
    /// document's element can be observed (an iframe's, from its parent).
    fn node(&self, s: &Scope<'_>, value: Value<'_>) -> JsResult<(Rc<PageDom>, NodeId)> {
        self.page()?;
        let node = s
            .unwrap::<crate::node::Element>(value)
            .ok_or_else(|| JsError::Type("IntersectionObserver target must be an Element".into()))?;
        Ok((node.page().clone(), node.id))
    }
}
pub struct IntersectionObserver;
impl JsClass for IntersectionObserver {
    const NAME: &'static str = "IntersectionObserver";
    type Data = Rc<Observer>;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(1, |s, nt, args| {
            args.require(1, "IntersectionObserver")?;
            let callback = args
                .get(0)
                .as_object()
                .filter(|o| o.is_function())
                .ok_or_else(|| JsError::Type("IntersectionObserver callback must be a function".into()))?;
            let page = page_of(s)?;
            let options = options::parse(s, &page, args.get(1))?;
            if page.is_disposed() {
                return Err(JsError::ContextDestroyed);
            }
            let id = page.next_event_id();
            let identity = page.event_identity(id);
            let observer = Rc::new(Observer {
                page: Rc::downgrade(&page),
                realm: s.realm().downgrade(),
                realm_id: s.realm_id(),
                identity,
                id,
                live: Cell::new(true),
                options,
                targets: RefCell::new(Vec::new()),
                records: RefCell::new(Vec::new()),
                last_queued: Cell::new(None),
                retry: Cell::new(false),
            });
            let wrapper = s.wrap_new::<Self>(observer.clone(), nt, Some(identity))?;
            let _root = Persistent::new(s, wrapper.value());
            s.set_hidden(wrapper, CALLBACK, callback.value())?;
            let thresholds = s.array(&observer.options.thresholds.iter().map(|v| s.number(*v)).collect::<Vec<_>>())?;
            let helper = page
                .intersections
                .helpers
                .borrow()
                .get(&s.realm_id())
                .map(|p| p.get(s))
                .ok_or(JsError::ContextDestroyed)?;
            let frozen =
                helper.as_object().unwrap().get_index(1)?.as_object().unwrap().call(None, &[thresholds.value()])?;
            s.set_hidden(wrapper, THRESHOLDS, frozen)?;
            page.intersections.observers.borrow_mut().push(Rc::downgrade(&observer));
            page.intersections.stamp.set(None);
            Ok(wrapper)
        });
        def.getter("root", |s, o| {
            let page = o.page()?;
            // The root's own document's bindings wrap it (it may be an iframe's document).
            match o.options.root.as_ref() {
                Some(r) => {
                    let root_page = r.page.upgrade().unwrap_or(page);
                    crate::util::opt_node(s, &root_page, Some(r.id))
                }
                None => Ok(s.null()),
            }
        });
        def.getter("rootMargin", |s, o| Ok(s.string(&o.options.margin_text)));
        def.getter("scrollMargin", |s, o| Ok(s.string(&o.options.scroll_margin_text)));
        def.getter("thresholds", |s, o| {
            let realm = o.realm.upgrade().ok_or(JsError::ContextDestroyed)?;
            let owner = realm.scope()?;
            let wrapper = owner.lookup_identity(o.identity).ok_or(JsError::ContextDestroyed)?;
            // Both scopes share a VM; the getter receiver roots the wrapper.
            let wrapper = unsafe { pyro_js::Object::from_raw(s.ctx(), wrapper.as_raw()) };
            Ok(s.get_hidden(wrapper, THRESHOLDS)?.unwrap_or(s.undefined()))
        });
        def.getter("delay", |s, o| Ok(s.number(o.options.delay)));
        def.getter("trackVisibility", |s, o| Ok(s.boolean(o.options.track_visibility)));
        def.method("observe", 1, |s, o, args| {
            args.require(1, "IntersectionObserver.observe")?;
            let (target_page, id) = o.node(s, args.get(0))?;
            let page = o.page()?;
            if !o.targets.borrow().iter().any(|t| t.node.is(&target_page, id)) {
                let node = NodeLease::new(&target_page, id)?;
                o.targets.borrow_mut().push(Observation { node, previous: None });
                page.intersections.stamp.set(None);
                o.reconcile(&page)?;
            }
            Ok(s.undefined())
        });
        def.method("unobserve", 1, |s, o, args| {
            args.require(1, "IntersectionObserver.unobserve")?;
            let (target_page, id) = o.node(s, args.get(0))?;
            let page = o.page()?;
            o.targets.borrow_mut().retain(|t| !t.node.is(&target_page, id));
            o.reconcile(&page)?;
            Ok(s.undefined())
        });
        def.method("disconnect", 0, |s, o, _| {
            let page = o.page()?;
            o.targets.borrow_mut().clear();
            o.reconcile(&page)?;
            Ok(s.undefined())
        });
        def.method("takeRecords", 0, |s, o, _| {
            let page = o.page()?;
            let records = o.records.take();
            o.reconcile(&page)?;
            entries(s, records)
        });
    }
}

pub struct EntryData {
    target: crate::collections::NodeHold,
    geometry: Geometry,
    time: f64,
}
pub struct IntersectionObserverEntry;
impl JsClass for IntersectionObserverEntry {
    const NAME: &'static str = "IntersectionObserverEntry";
    type Data = EntryData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.getter("target", |s, d| Ok(crate::node::wrap_node(s, &d.target.page, d.target.id)?.value()));
        def.getter("time", |s, d| Ok(s.number(d.time)));
        def.getter("isIntersecting", |s, d| Ok(s.boolean(d.geometry.intersecting)));
        def.getter("intersectionRatio", |s, d| Ok(s.number(d.geometry.ratio)));
        def.getter("boundingClientRect", |s, d| crate::canvas::rect_value(s, &d.target.page, true, d.geometry.target));
        def.getter("intersectionRect", |s, d| crate::canvas::rect_value(s, &d.target.page, true, d.geometry.intersection));
        def.getter("isVisible", |s, d| Ok(s.boolean(d.geometry.visible)));
        def.getter("rootBounds", |s, d| crate::canvas::rect_value(s, &d.target.page, true, d.geometry.bounds));
    }
}
fn entries<'s>(s: &Scope<'s>, records: Vec<PendingEntry>) -> JsResult<Value<'s>> {
    let mut values = pyro_js::RootedVec::new();
    for record in records {
        // A target whose document went away is reported no more.
        let Some(page) = record.target.page.upgrade() else { continue };
        let target = crate::collections::NodeHold::new(&page, record.target.id)?;
        let entry = s.wrap::<IntersectionObserverEntry>(
            EntryData { target, geometry: record.geometry, time: record.time },
            None,
        )?;
        values.push(s, entry.value());
    }
    Ok(s.array(&values.iter(s).collect::<Vec<_>>())?.value())
}
/// HTML's lazy loading intersection: whether `target` of `page`'s document intersects the
/// top-level viewport expanded by the lazy load root margin, through each clipping
/// ancestor and frame (scroll containers expanded by the same margin). Not being rendered
/// or not being in a shown document intersects nothing. Syncs layout.
pub fn lazy_load_intersects(page: &PageDom, target: NodeId) -> bool {
    geometry::measure(page, &options::lazy_load(), page, target).intersecting
}

pub(crate) fn after_task(page: &PageDom) {
    if page.is_disposed() {
        return;
    }
    let Some(owner) = page.intersections.owner.borrow().upgrade() else { return };
    let stamp = (page.document(), page.layout().observation_stamp(&page.dom.borrow()));
    // Geometry that depends on another document's layout (a target in a
    // frame, or this document shown in one: the implicit root is the
    // top-level viewport) is measured after every task.
    let across = !matches!(page.container(), crate::page::FrameContainer::Top)
        || page.intersections.observers.borrow().iter().filter_map(Weak::upgrade).any(|o| {
            o.targets.borrow().iter().any(|t| !std::ptr::eq(t.node.page.as_ptr(), page))
        });
    if page.intersections.stamp.replace(Some(stamp)) != Some(stamp) || across {
        let observers: Vec<_> = page.intersections.observers.borrow().iter().filter_map(Weak::upgrade).collect();
        for observer in observers {
            if !observer.live.get() {
                continue;
            }
            // IntersectionObserver v2 `delay`: a tracking observer queues entries at most once per
            // `delay`; the observation is retried when it has passed (even with no frame).
            if observer.options.track_visibility {
                let now = page.intersections.origin.get().map_or(0.0, |t| t.elapsed().as_secs_f64() * 1000.0);
                if let Some(wait) = observer.last_queued.get().map(|last| last + observer.options.delay - now).filter(|w| *w > 0.0) {
                    page.intersections.stamp.set(None);
                    if !observer.retry.replace(true) {
                        let (weak, observer) = (Rc::downgrade(&owner), observer.clone());
                        page.post_delayed_task(
                            std::time::Duration::from_secs_f64(wait / 1000.0),
                            Box::new(move || {
                                observer.retry.set(false);
                                if let Some(page) = weak.upgrade() {
                                    after_task(&page);
                                }
                            }),
                        );
                    }
                    continue;
                }
            }
            for target in observer.targets.borrow_mut().iter_mut() {
                let Some(target_page) = target.node.page.upgrade() else { continue };
                let geometry = geometry::measure(page, &observer.options, &target_page, target.node.id);
                let index = observer.options.thresholds.partition_point(|threshold| *threshold <= geometry.ratio);
                let state = (index, geometry.intersecting, geometry.visible);
                if target.previous.replace(state) != Some(state) {
                    if let Ok(node) = NodeLease::new(&target_page, target.node.id) {
                        let time = page.intersections.origin.get().map_or(0.0, |t| t.elapsed().as_secs_f64() * 1000.0);
                        observer.records.borrow_mut().push(PendingEntry { target: node, geometry, time });
                        observer.last_queued.set(Some(time));
                    }
                }
            }
        }
        page.intersections.observers.borrow_mut().retain(|o| o.strong_count() > 0);
    }
    if !page.intersections.queued.get()
        && page
            .intersections
            .observers
            .borrow()
            .iter()
            .filter_map(Weak::upgrade)
            .any(|o| !o.records.borrow().is_empty())
    {
        if let Some(hooks) = page.hooks() {
            page.intersections.queued.set(true);
            let weak = Rc::downgrade(&owner);
            hooks.post_task(Box::new(move || {
                if let Some(page) = weak.upgrade() {
                    notify(&page)
                }
            }));
        }
    }
}
fn notify(page: &Rc<PageDom>) {
    page.intersections.queued.set(false);
    if page.is_disposed() {
        return;
    }
    let observers: Vec<_> = page.intersections.observers.borrow().iter().filter_map(Weak::upgrade).collect();
    for observer in observers {
        if !observer.live.get() {
            continue;
        }
        let records = observer.records.take();
        if records.is_empty() {
            continue;
        }
        let Some(realm) = observer.realm.upgrade() else {
            continue;
        };
        let Ok(s) = realm.scope() else {
            continue;
        };
        let result = (|| {
            let wrapper = s.lookup_identity(observer.identity).ok_or(JsError::ContextDestroyed)?;
            let _root = Persistent::new(&s, wrapper.value());
            observer.reconcile(page)?;
            let callback =
                s.get_hidden(wrapper, CALLBACK)?.and_then(|v| v.as_object()).ok_or(JsError::ContextDestroyed)?;
            let list = entries(&s, records)?;
            callback.call(Some(wrapper), &[list, wrapper.value()])?;
            Ok(())
        })();
        if let Err(error) = result {
            s.report_exception(&error);
        }
    }
}
pub(crate) fn cancel_realm(page: &PageDom, realm: RealmId) {
    let observers: Vec<_> = page
        .intersections
        .observers
        .borrow()
        .iter()
        .filter_map(Weak::upgrade)
        .filter(|o| o.realm_id == realm)
        .collect();
    for observer in observers {
        observer.live.set(false);
        observer.targets.borrow_mut().clear();
        observer.records.borrow_mut().clear();
        let old = page.intersections.pins.borrow_mut().remove(&observer.id);
        drop(old);
    }
    page.intersections.helpers.borrow_mut().remove(&realm);
}
pub(crate) fn dispose(page: &PageDom) {
    let realms: Vec<_> = page.intersections.helpers.borrow().keys().copied().collect();
    for realm in realms {
        cancel_realm(page, realm);
    }
}
/// `helper` is install.js `intersection`: [the threshold list of an init,
/// Object.freeze].
pub(crate) fn install(s: &Scope<'_>, page: &Rc<PageDom>, helper: pyro_js::Value<'_>) -> JsResult<()> {
    *page.intersections.owner.borrow_mut() = Rc::downgrade(page);
    if page.intersections.origin.get().is_none() {
        page.intersections.origin.set(Some(Instant::now()));
    }
    page.intersections.helpers.borrow_mut().insert(s.realm_id(), Persistent::new(s, helper));
    Ok(())
}
/// The realm of `old`'s window now serves `new` (Window reuse).
pub(crate) fn adopt_realm(old: &PageDom, new: &Rc<PageDom>, realm: RealmId) {
    *new.intersections.owner.borrow_mut() = Rc::downgrade(new);
    if new.intersections.origin.get().is_none() {
        new.intersections.origin.set(Some(Instant::now()));
    }
    let helper = old.intersections.helpers.borrow_mut().remove(&realm);
    if let Some(helper) = helper {
        new.intersections.helpers.borrow_mut().insert(realm, helper);
    }
}
pub(crate) fn register(engine: &pyro_js::Engine) {
    engine.register::<IntersectionObserver>();
    engine.register::<IntersectionObserverEntry>();
}
