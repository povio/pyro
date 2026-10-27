//! ResizeObserver over the faux layout, porting upstream semantics
//! (lightpanda-io/browser @ 2ad38773, src/browser/webapi/ResizeObserver.zig):
//! an observation's size is the element's faux-layout axis (`getElementAxis`)
//! while connected and rendered, else 0x0; an entry is delivered when the
//! size differs from the last reported one (initially 0x0, so observing a
//! rendered element delivers an initial entry). Every box option reports the
//! same size (upstream ignores `box`); `contentRect` is (0, 0, width, height).
//!
//! Scheduling mirrors IntersectionObserver: `PageDom.after_task` re-measures
//! only when the DOM/style/viewport/scroll stamp changed or a target was
//! (un)observed, and queues callbacks as a separate `PageHooks.post_task`
//! (no polling, no inline fallback). Active observers pin their wrapper until
//! targets and pending records drain; realm/page disposal releases them.
//! Gaps: no rendering-frame cadence, no loop-limit/"undelivered" error event,
//! no device-pixel rounding, cross-PageDom targets are NotSupported.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use lp_dom::layout::{Axis, Geometry, Rect};
use lp_dom::NodeId;
use lp_js::{
    ClassDef, IdentityKey, JsClass, JsError, JsResult, NoParent, Persistent, RealmId, Scope, Value, WeakRealm,
};
use rustc_hash::FxHashMap;

use crate::collections::NodeHold;
use crate::target::page_of;
use crate::util::DomResultExt;
use crate::PageDom;

const CALLBACK: &str = "__lpResizeCallback";
type Stamp = (NodeId, ((u64, u64, u32, u32), u64));

#[derive(Default)]
pub(crate) struct BindingState {
    owner: RefCell<Weak<PageDom>>,
    observers: RefCell<Vec<Weak<Observer>>>,
    pub(crate) pins: RefCell<FxHashMap<u64, Persistent>>,
    stamp: Cell<Option<Stamp>>,
    queued: Cell<bool>,
    /// Flow layout pacing: when the next gather may run, and whether a
    /// delayed wake for it is pending.
    next_gather: Cell<Option<Instant>>,
    wake_queued: Cell<bool>,
}
impl BindingState {
    pub(crate) fn has_observers(&self) -> bool {
        !self.observers.borrow().is_empty()
    }
}

/// Native node reference that does not keep the PageDom alive.
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
    last: (f64, f64),
}

struct Pending {
    target: NodeLease,
    size: (f64, f64),
}

pub struct Observer {
    page: Weak<PageDom>,
    realm: WeakRealm,
    realm_id: RealmId,
    identity: IdentityKey,
    id: u64,
    live: Cell<bool>,
    targets: RefCell<Vec<Observation>>,
    records: RefCell<Vec<Pending>>,
}

impl Observer {
    fn page(&self) -> JsResult<Rc<PageDom>> {
        self.page.upgrade().filter(|p| !p.is_disposed() && self.live.get()).ok_or(JsError::ContextDestroyed)
    }
    fn reconcile(&self, page: &PageDom) -> JsResult<()> {
        if self.targets.borrow().is_empty() && self.records.borrow().is_empty() {
            let old = page.resizes.pins.borrow_mut().remove(&self.id);
            drop(old);
        } else if !page.resizes.pins.borrow().contains_key(&self.id) {
            let realm = self.realm.upgrade().ok_or(JsError::ContextDestroyed)?;
            let s = realm.scope()?;
            let wrapper = s.lookup_identity(self.identity).ok_or(JsError::ContextDestroyed)?;
            page.resizes.pins.borrow_mut().insert(self.id, Persistent::new(&s, wrapper.value()));
        }
        Ok(())
    }
    fn element(&self, s: &Scope<'_>, value: Value<'_>) -> JsResult<NodeId> {
        let page = self.page()?;
        let node = s
            .unwrap::<crate::node::Element>(value)
            .ok_or_else(|| JsError::Type("ResizeObserver target must be an Element".into()))?;
        if node.page().id() != page.id() {
            return Err(JsError::dom("NotSupportedError", "Cross-PageDom observation is not implemented"));
        }
        Ok(node.id)
    }
}

/// Upstream `deliverEntries` size: faux axis when connected and visible.
/// Under the flow layout, the content box like Chromium's contentRect: the
/// padding box (clientWidth/Height) less padding. @povio/ui sizes its select
/// popovers from this, so the faux axis (5 px) clipped every option.
fn measure(page: &PageDom, id: NodeId) -> (f64, f64) {
    let dom = page.dom.borrow();
    let layout = page.layout.borrow();
    if !dom.is_connected(id).unwrap_or(false) || !layout.is_visible(&dom, id) {
        return (0.0, 0.0);
    }
    if layout.geometry() == Geometry::Flow {
        let pad = |side: &str| {
            layout
                .style()
                .used_value(&dom, id, &format!("padding-{side}"))
                .and_then(|v| v.trim().strip_suffix("px").and_then(|n| n.parse::<f64>().ok()))
                .unwrap_or(0.0)
        };
        let width = (layout.client_width(&dom, id) - pad("left") - pad("right")).max(0.0);
        let height = (layout.client_height(&dom, id) - pad("top") - pad("bottom")).max(0.0);
        return (width, height);
    }
    (layout.element_axis(&dom, id, Axis::Width).value, layout.element_axis(&dom, id, Axis::Height).value)
}

pub struct ResizeObserver;
impl JsClass for ResizeObserver {
    const NAME: &'static str = "ResizeObserver";
    type Data = Rc<Observer>;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(1, |s, nt, args| {
            args.require(1, "ResizeObserver")?;
            let callback = args
                .get(0)
                .as_object()
                .filter(|o| o.is_function())
                .ok_or_else(|| JsError::Type("ResizeObserver callback must be a function".into()))?;
            let page = page_of(s)?;
            if page.is_disposed() {
                return Err(JsError::ContextDestroyed);
            }
            *page.resizes.owner.borrow_mut() = Rc::downgrade(&page);
            let id = page.next_event_id();
            let identity = page.event_identity(id);
            let observer = Rc::new(Observer {
                page: Rc::downgrade(&page),
                realm: s.realm().downgrade(),
                realm_id: s.realm_id(),
                identity,
                id,
                live: Cell::new(true),
                targets: RefCell::new(Vec::new()),
                records: RefCell::new(Vec::new()),
            });
            let wrapper = s.wrap_new::<Self>(observer.clone(), nt, Some(identity))?;
            s.set_hidden(wrapper, CALLBACK, callback.value())?;
            page.resizes.observers.borrow_mut().push(Rc::downgrade(&observer));
            Ok(wrapper)
        });
        def.method("observe", 1, |s, o, args| {
            args.require(1, "ResizeObserver.observe")?;
            let id = o.element(s, args.get(0))?;
            // `box` is validated like browsers do but, as upstream, all boxes
            // report the same faux size.
            if let Some(opts) = args.get(1).as_object() {
                let b = opts.get("box")?;
                if !b.is_undefined() {
                    let b = crate::util::dom_string(b)?;
                    if !matches!(b.as_str(), "content-box" | "border-box" | "device-pixel-content-box") {
                        return Err(JsError::Type(format!(
                            "The provided value '{b}' is not a valid ResizeObserverBoxOptions"
                        )));
                    }
                }
            }
            let page = o.page()?;
            if !o.targets.borrow().iter().any(|t| t.node.id == id) {
                let node = NodeLease::new(&page, id)?;
                o.targets.borrow_mut().push(Observation { node, last: (0.0, 0.0) });
                page.resizes.stamp.set(None);
                o.reconcile(&page)?;
            }
            Ok(s.undefined())
        });
        def.method("unobserve", 1, |s, o, args| {
            args.require(1, "ResizeObserver.unobserve")?;
            let id = o.element(s, args.get(0))?;
            let page = o.page()?;
            o.targets.borrow_mut().retain(|t| t.node.id != id);
            o.reconcile(&page)?;
            Ok(s.undefined())
        });
        def.method("disconnect", 0, |s, o, _| {
            let page = o.page()?;
            o.targets.borrow_mut().clear();
            o.records.borrow_mut().clear();
            o.reconcile(&page)?;
            Ok(s.undefined())
        });
    }
}

pub struct EntryData {
    target: NodeHold,
    size: (f64, f64),
}

pub struct ResizeObserverEntry;
impl JsClass for ResizeObserverEntry {
    const NAME: &'static str = "ResizeObserverEntry";
    type Data = EntryData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.getter("target", |s, d| Ok(crate::node::wrap_node(s, &d.target.page, d.target.id)?.value()));
        def.getter("contentRect", |s, d| {
            crate::layout_api::rect_value(s, Rect { x: 0.0, y: 0.0, width: d.size.0, height: d.size.1 })
        });
        def.getter("borderBoxSize", |s, d| sizes(s, d.size));
        def.getter("contentBoxSize", |s, d| sizes(s, d.size));
        def.getter("devicePixelContentBoxSize", |s, d| sizes(s, d.size));
    }
}

pub struct ResizeObserverSize;
impl JsClass for ResizeObserverSize {
    const NAME: &'static str = "ResizeObserverSize";
    type Data = (f64, f64);
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.getter("inlineSize", |s, d| Ok(s.number(d.0)));
        def.getter("blockSize", |s, d| Ok(s.number(d.1)));
    }
}

fn sizes<'s>(s: &Scope<'s>, size: (f64, f64)) -> JsResult<Value<'s>> {
    let item = s.wrap::<ResizeObserverSize>(size, None)?;
    let list = s.array(&[item.value()])?;
    let freeze = s.global().get("Object")?.as_object().expect("Object").get("freeze")?;
    freeze.as_object().expect("Object.freeze").call(None, &[list.value()])
}

fn entries<'s>(s: &Scope<'s>, page: &Rc<PageDom>, records: Vec<Pending>) -> JsResult<Value<'s>> {
    let mut values = lp_js::RootedVec::new();
    for record in records {
        let target = NodeHold::new(page, record.target.id)?;
        let entry = s.wrap::<ResizeObserverEntry>(EntryData { target, size: record.size }, None)?;
        values.push(s, entry.value());
    }
    Ok(s.array(&values.iter(s).collect::<Vec<_>>())?.value())
}

/// Checkpoint from `PageDom::after_task`.
pub(crate) fn after_task(page: &PageDom) {
    if page.is_disposed() {
        return;
    }
    let Some(owner) = page.resizes.owner.borrow().upgrade() else { return };
    let stamp = (page.document(), page.layout.borrow().observation_stamp(&page.dom.borrow()));
    let flow = page.layout.borrow().geometry() == Geometry::Flow;
    if flow && page.resizes.stamp.get() != Some(stamp) && !page.resizes.observers.borrow().is_empty() {
        // Each gather lays out the whole document under the flow layout.
        // Chromium gathers once per rendering update, not per task: gather
        // at most every max(16 ms, twice the last gather's cost), and wake
        // once more so the final size is still delivered.
        let now = Instant::now();
        if let Some(next) = page.resizes.next_gather.get().filter(|&n| n > now) {
            if !page.resizes.wake_queued.replace(true) {
                if let Some(hooks) = page.hooks() {
                    let weak = Rc::downgrade(&owner);
                    hooks.post_delayed_task(
                        next - now,
                        Box::new(move || {
                            if let Some(page) = weak.upgrade() {
                                page.resizes.wake_queued.set(false);
                                after_task(&page);
                            }
                        }),
                    );
                } else {
                    page.resizes.wake_queued.set(false);
                }
            }
            return;
        }
    }
    let started = Instant::now();
    if page.resizes.stamp.replace(Some(stamp)) != Some(stamp) {
        let observers: Vec<_> = page.resizes.observers.borrow().iter().filter_map(Weak::upgrade).collect();
        for observer in observers {
            if !observer.live.get() {
                continue;
            }
            for target in observer.targets.borrow_mut().iter_mut() {
                let size = measure(page, target.node.id);
                if size != target.last {
                    target.last = size;
                    if let Ok(node) = NodeLease::new(&owner, target.node.id) {
                        let mut records = observer.records.borrow_mut();
                        // Only the latest size per target is reported.
                        records.retain(|r| r.target.id != target.node.id);
                        records.push(Pending { target: node, size });
                    }
                }
            }
        }
        page.resizes.observers.borrow_mut().retain(|o| o.strong_count() > 0);
        if flow {
            let cost = started.elapsed();
            page.resizes.next_gather.set(Some(Instant::now() + (cost * 2).max(Duration::from_millis(16))));
        }
    }
    let pending =
        page.resizes.observers.borrow().iter().filter_map(Weak::upgrade).any(|o| !o.records.borrow().is_empty());
    if pending && !page.resizes.queued.get() {
        if let Some(hooks) = page.hooks() {
            page.resizes.queued.set(true);
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
    page.resizes.queued.set(false);
    if page.is_disposed() {
        return;
    }
    let observers: Vec<_> = page.resizes.observers.borrow().iter().filter_map(Weak::upgrade).collect();
    for observer in observers {
        if !observer.live.get() {
            continue;
        }
        let records = observer.records.take();
        if records.is_empty() {
            continue;
        }
        let Some(realm) = observer.realm.upgrade() else { continue };
        let Ok(s) = realm.scope() else { continue };
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
    let observers: Vec<_> =
        page.resizes.observers.borrow().iter().filter_map(Weak::upgrade).filter(|o| o.realm_id == realm).collect();
    for observer in observers {
        observer.live.set(false);
        observer.targets.borrow_mut().clear();
        observer.records.borrow_mut().clear();
        let old = page.resizes.pins.borrow_mut().remove(&observer.id);
        drop(old);
    }
}

pub(crate) fn dispose(page: &PageDom) {
    let observers: Vec<_> = page.resizes.observers.borrow().iter().filter_map(Weak::upgrade).collect();
    for observer in observers {
        observer.live.set(false);
        observer.targets.borrow_mut().clear();
        observer.records.borrow_mut().clear();
    }
    let pins = std::mem::take(&mut *page.resizes.pins.borrow_mut());
    drop(pins);
}

pub(crate) fn register(engine: &lp_js::Engine) {
    engine.register::<ResizeObserver>();
    engine.register::<ResizeObserverEntry>();
    engine.register::<ResizeObserverSize>();
}
