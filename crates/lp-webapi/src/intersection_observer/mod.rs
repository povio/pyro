//! Task-delivered intersection observations over the native faux layout.
mod geometry;
mod options;
use crate::{target::page_of, util::DomResultExt, PageDom};
use lp_dom::{layout::Rect, NodeId};
use lp_js::{
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
impl Drop for NodeLease {
    fn drop(&mut self) {
        if let Some(page) = self.page.upgrade() {
            page.release_node(self.id);
        }
    }
}
struct Observation {
    node: NodeLease,
    previous: Option<(usize, bool)>,
}
#[derive(Clone, Copy)]
struct Geometry {
    bounds: Rect,
    target: Rect,
    intersection: Rect,
    intersecting: bool,
    ratio: f64,
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
    fn node(&self, s: &Scope<'_>, value: Value<'_>) -> JsResult<NodeId> {
        let page = self.page()?;
        let node = s
            .unwrap::<crate::node::Element>(value)
            .ok_or_else(|| JsError::Type("IntersectionObserver target must be an Element".into()))?;
        if node.page().id() != page.id() {
            return Err(JsError::dom("NotSupportedError", "Cross-PageDom observation is not implemented"));
        }
        Ok(node.id)
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
            crate::util::opt_node(s, &page, o.options.root.as_ref().map(|r| r.id))
        });
        def.getter("rootMargin", |s, o| Ok(s.string(&o.options.margin_text)));
        def.getter("scrollMargin", |s, _| Ok(s.string("0px 0px 0px 0px")));
        def.getter("thresholds", |s, o| {
            let realm = o.realm.upgrade().ok_or(JsError::ContextDestroyed)?;
            let owner = realm.scope()?;
            let wrapper = owner.lookup_identity(o.identity).ok_or(JsError::ContextDestroyed)?;
            // Both scopes share a VM; the getter receiver roots the wrapper.
            let wrapper = unsafe { lp_js::Object::from_raw(s.ctx(), wrapper.as_raw()) };
            Ok(s.get_hidden(wrapper, THRESHOLDS)?.unwrap_or(s.undefined()))
        });
        def.getter("delay", |s, _| Ok(s.number(0.0)));
        def.getter("trackVisibility", |s, _| Ok(s.boolean(false)));
        def.method("observe", 1, |s, o, args| {
            args.require(1, "IntersectionObserver.observe")?;
            let id = o.node(s, args.get(0))?;
            let page = o.page()?;
            if !o.targets.borrow().iter().any(|t| t.node.id == id) {
                let node = NodeLease::new(&page, id)?;
                o.targets.borrow_mut().push(Observation { node, previous: None });
                page.intersections.stamp.set(None);
                o.reconcile(&page)?;
            }
            Ok(s.undefined())
        });
        def.method("unobserve", 1, |s, o, args| {
            args.require(1, "IntersectionObserver.unobserve")?;
            let id = o.node(s, args.get(0))?;
            let page = o.page()?;
            o.targets.borrow_mut().retain(|t| t.node.id != id);
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
            entries(s, &page, records)
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
        def.getter("boundingClientRect", |s, d| crate::layout_api::rect_value(s, d.geometry.target));
        def.getter("intersectionRect", |s, d| crate::layout_api::rect_value(s, d.geometry.intersection));
        def.getter("rootBounds", |s, d| crate::layout_api::rect_value(s, d.geometry.bounds));
    }
}
fn entries<'s>(s: &Scope<'s>, page: &Rc<PageDom>, records: Vec<PendingEntry>) -> JsResult<Value<'s>> {
    let mut values = lp_js::RootedVec::new();
    for record in records {
        let target = crate::collections::NodeHold::new(page, record.target.id)?;
        let entry = s.wrap::<IntersectionObserverEntry>(
            EntryData { target, geometry: record.geometry, time: record.time },
            None,
        )?;
        values.push(s, entry.value());
    }
    Ok(s.array(&values.iter(s).collect::<Vec<_>>())?.value())
}
pub(crate) fn after_task(page: &PageDom) {
    if page.is_disposed() {
        return;
    }
    let Some(owner) = page.intersections.owner.borrow().upgrade() else { return };
    let stamp = (page.document(), page.layout.borrow().observation_stamp(&page.dom.borrow()));
    if page.intersections.stamp.replace(Some(stamp)) != Some(stamp) {
        let observers: Vec<_> = page.intersections.observers.borrow().iter().filter_map(Weak::upgrade).collect();
        for observer in observers {
            if !observer.live.get() {
                continue;
            }
            for target in observer.targets.borrow_mut().iter_mut() {
                let geometry = geometry::measure(page, &observer.options, target.node.id);
                let index = observer.options.thresholds.partition_point(|threshold| *threshold <= geometry.ratio);
                let state = (index, geometry.intersecting);
                if target.previous.replace(state) != Some(state) {
                    if let Ok(node) = NodeLease::new(&owner, target.node.id) {
                        let time = page.intersections.origin.get().map_or(0.0, |t| t.elapsed().as_secs_f64() * 1000.0);
                        observer.records.borrow_mut().push(PendingEntry { target: node, geometry, time });
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
            let list = entries(&s, page, records)?;
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
pub(crate) fn install(s: &Scope<'_>, page: &Rc<PageDom>) -> JsResult<()> {
    *page.intersections.owner.borrow_mut() = Rc::downgrade(page);
    if page.intersections.origin.get().is_none() {
        page.intersections.origin.set(Some(Instant::now()));
    }
    let helper=s.eval(r#"(()=>{const freeze=Object.freeze, sort=Array.prototype.sort, push=Array.prototype.push, apply=Reflect.apply, finite=Number.isFinite, Range=RangeError, Type=TypeError;
        return [value=>{let list=[];
            if(value===undefined)list=[0];
            else if(value!==null&&(typeof value==='object'||typeof value==='function')){for(const item of value)apply(push,list,[+item]);}
            else list=[+value];
            for(let i=0;i<list.length;i++){const n=list[i];if(!finite(n))throw new Type('threshold must be finite'); if(n<0||n>1)throw new Range('threshold must be between zero and one');}
            if(list.length===0)apply(push,list,[0]);apply(sort,list,[(a,b)=>a-b]);return freeze(list);
        },freeze];})()"#,None,1)?;
    page.intersections.helpers.borrow_mut().insert(s.realm_id(), Persistent::new(s, helper));
    Ok(())
}
pub(crate) fn register(engine: &lp_js::Engine) {
    engine.register::<IntersectionObserver>();
    engine.register::<IntersectionObserverEntry>();
}
