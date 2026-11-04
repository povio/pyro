//! `Worker` (HTML §10.2) as an honest stub: no script is fetched or run.
//!
//! Lightningpanda has no worker threads. Without this class a page that builds
//! a Worker at startup dies on `ReferenceError`, so the constructor instead
//! behaves like a worker whose script failed to load, which pages already have
//! to handle: it validates the URL (SyntaxError when it cannot be parsed
//! against the document base), then a later task fires a trusted, non-bubbling
//! `error` Event at the object. Nothing pretends the worker ran: `postMessage`
//! drops its message and no `message` event ever arrives.
//!
//! `postMessage` does not structured-clone either. Nothing receives the value,
//! and `clone_value` rejects transfer lists, so cloning would turn the common
//! `postMessage(buffer, [buffer])` into a NotSupportedError.
//!
//! Lifetime: the wrapper is pinned until the error fires or `terminate()` is
//! called, so `new Worker(url).onerror = f` reports without a JS reference.
//!
//! Gaps (explicit): no script execution, no fetch of the URL, `options` is
//! ignored (not even converted as a dictionary), and `SharedWorker` and
//! `ServiceWorker` stay undefined. See docs/webapi/WORKER.md.
use std::cell::Cell;
use std::rc::Rc;

use lp_js::{ClassDef, JsClass, JsError, WeakRealm};

use crate::event::{EventCore, EventInterface};
use crate::target::{fire, page_of, pin_target, unpin_target, EventTarget, EventTargetData};
use crate::util::dom_string;
use lp_dom::events::TargetKey;

pub(crate) struct WorkerInner {
    target: EventTargetData,
    realm: WeakRealm,
    /// The load failure was reported or `terminate()` cancelled it; either way
    /// the pin is gone and no event can fire any more.
    settled: Cell<bool>,
}

impl WorkerInner {
    /// Marks the worker settled and drops the pin.
    fn settle(&self) {
        if self.settled.replace(true) {
            return;
        }
        if let Some(realm) = self.realm.upgrade() {
            if let Ok(s) = realm.scope() {
                unpin_target(&s, &self.target.page, self.target.key);
            }
        }
    }

    fn fail_to_load(&self) {
        let page = &self.target.page;
        if self.settled.get() || page.is_disposed() {
            return;
        }
        let ev = EventCore::trusted(page, EventInterface::Event, "error", false, false);
        fire(page, self.target.key, &ev);
        self.settle();
    }
}

pub struct WorkerData(Rc<WorkerInner>);

pub struct Worker;
impl JsClass for Worker {
    const NAME: &'static str = "Worker";
    type Data = WorkerData;
    type Parent = EventTarget;
    fn define(def: &mut ClassDef<Self>) {
        def.upcast(|d: &WorkerData| &d.0.target);
        def.constructor(1, |s, nt, a| {
            a.require(1, "Worker")?;
            let page = page_of(s)?;
            if page.is_disposed() {
                return Err(JsError::ContextDestroyed);
            }
            let input = dom_string(a.get(0))?;
            let resolved = crate::html::resolve_url(&page.dom.borrow(), page.document(), &input);
            if lp_net::Url::parse(&resolved).is_err() {
                return Err(JsError::dom(
                    "SyntaxError",
                    format!("Failed to construct 'Worker': The URL '{input}' is invalid."),
                ));
            }
            let target = EventTargetData::new(&page);
            let TargetKey::Other(k) = target.key else { unreachable!() };
            let key = page.other_identity(k);
            let inner = Rc::new(WorkerInner { target, realm: s.realm().downgrade(), settled: Cell::new(false) });
            let object = s.wrap_new::<Worker>(WorkerData(inner.clone()), nt, Some(key))?;
            pin_target(s, &page, inner.target.key, object);
            let worker = Rc::downgrade(&inner);
            page.post_task(Box::new(move || {
                if let Some(worker) = worker.upgrade() {
                    worker.fail_to_load();
                }
            }));
            Ok(object)
        });
        def.method("postMessage", 1, |s, _d, a| {
            a.require(1, "Worker.postMessage")?;
            Ok(s.undefined())
        });
        def.method("terminate", 0, |s, d, _a| {
            d.0.settle();
            Ok(s.undefined())
        });
        def.accessor(
            "onmessage",
            |s, d| crate::target::event_handler(s, &d.0.target, "message"),
            |s, d, value| crate::target::set_event_handler(s, &d.0.target, "message", value),
        );
        def.accessor(
            "onmessageerror",
            |s, d| crate::target::event_handler(s, &d.0.target, "messageerror"),
            |s, d, value| crate::target::set_event_handler(s, &d.0.target, "messageerror", value),
        );
        def.accessor(
            "onerror",
            |s, d| crate::target::event_handler(s, &d.0.target, "error"),
            |s, d, value| crate::target::set_event_handler(s, &d.0.target, "error", value),
        );
    }
}

pub(crate) fn register(engine: &lp_js::Engine) {
    engine.register::<Worker>();
}
