//! AbortController / AbortSignal.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use lp_dom::events::{ListenerHandle, TargetKey};
use lp_js::{ClassDef, JsClass, JsError, JsResult, NoParent, Object, Persistent, Scope, Value};

use crate::event::{EventCore, EventInterface};
use crate::page::PageDom;
use crate::target::{fire, page_of, EventTarget, EventTargetData};

type AbortHook = Box<dyn FnOnce(&PageDom)>;

pub struct AbortSignalData {
    pub target: EventTargetData,
    aborted: Cell<bool>,
    reason: RefCell<Option<Persistent>>,
    /// Listener registrations to drop on abort (`{signal}` option).
    remove_on_abort: RefCell<Vec<(TargetKey, String, ListenerHandle, bool)>>,
    /// Native abort steps (fetch/XHR cancellation).
    hooks: RefCell<Vec<AbortHook>>,
}

impl AbortSignalData {
    pub fn aborted(&self) -> bool {
        self.aborted.get()
    }
    pub fn reason<'s>(&self, s: &Scope<'s>) -> Value<'s> {
        self.reason.borrow().as_ref().map_or(s.undefined(), |p| p.get(s))
    }
    pub(crate) fn on_abort_remove(&self, key: TargetKey, ty: String, handle: ListenerHandle, capture: bool) {
        self.remove_on_abort.borrow_mut().push((key, ty, handle, capture));
    }
    /// Registers native abort steps (run once, on abort).
    pub fn add_abort_hook(&self, hook: AbortHook) {
        if self.aborted.get() {
            hook(&self.target.page);
        } else {
            self.hooks.borrow_mut().push(hook);
        }
    }

    /// "signal abort".
    pub fn abort(&self, s: &Scope<'_>, reason: Value<'_>) {
        if self.aborted.replace(true) {
            return;
        }
        let reason = if reason.is_undefined() {
            JsError::dom("AbortError", "signal is aborted without reason").to_value(s)
        } else {
            reason
        };
        *self.reason.borrow_mut() = Some(Persistent::new(s, reason));
        let page = self.target.page.clone();
        for (key, ty, handle, capture) in self.remove_on_abort.take() {
            page.dom.borrow_mut().remove_listener(key, &ty, handle, capture);
        }
        for hook in self.hooks.take() {
            hook(&page);
        }
        let ev = EventCore::new(&page, EventInterface::Event, "abort", false, false, false);
        ev.state.borrow_mut().is_trusted = true;
        fire(&page, self.target.key, &ev);
    }
}

pub struct AbortSignal;
impl JsClass for AbortSignal {
    const NAME: &'static str = "AbortSignal";
    type Data = AbortSignalData;
    type Parent = EventTarget;
    fn define(def: &mut ClassDef<Self>) {
        def.upcast(|d: &AbortSignalData| &d.target);
        def.getter("aborted", |s, d| Ok(s.boolean(d.aborted())));
        def.getter("reason", |s, d| Ok(d.reason(s)));
        def.method("throwIfAborted", 0, |s, d, _a| {
            if d.aborted() {
                let r = d.reason(s);
                return Err(JsError::Thrown(Persistent::new(s, r)));
            }
            Ok(s.undefined())
        });
        def.static_method("abort", 0, |s, a| {
            let sig = new_signal(s)?;
            if let Some(d) = s.unwrap::<AbortSignal>(sig.value()) {
                d.abort(s, a.get(0));
            }
            Ok(sig.value())
        });
        def.static_method("timeout", 1, |s, a| {
            let ms = a.get(0).to_number()?;
            let sig = new_signal(s)?;
            let keep = Persistent::new(s, sig.value());
            let cb = s.function("abortTimeout", move |s, _this, _args| {
                if let Some(d) = s.unwrap::<AbortSignal>(keep.get(s)) {
                    let reason = JsError::dom("TimeoutError", "signal timed out").to_value(s);
                    d.abort(s, reason);
                }
                Ok(s.undefined())
            })?;
            let set_timeout = s.global().get("setTimeout")?;
            if let Some(f) = set_timeout.as_object() {
                f.call(None, &[cb.value(), s.number(ms)])?;
            }
            Ok(sig.value())
        });
        def.static_method("any", 1, |s, a| {
            let sig = new_signal(s)?;
            let list =
                a.get(0).as_object().ok_or_else(|| JsError::Type("AbortSignal.any requires an iterable".into()))?;
            let len = list.get("length")?.to_number()? as u32;
            for i in 0..len {
                let v = list.get_index(i)?;
                let Some(src) = s.unwrap::<AbortSignal>(v) else { continue };
                if src.aborted() {
                    if let Some(d) = s.unwrap::<AbortSignal>(sig.value()) {
                        d.abort(s, src.reason(s));
                    }
                    break;
                }
                let target = Persistent::new(s, sig.value());
                let realm = s.realm().downgrade();
                let source = Persistent::new(s, v);
                src.add_abort_hook(Box::new(move |_page| {
                    let Some(realm) = realm.upgrade() else { return };
                    let Ok(s) = realm.scope() else { return };
                    let reason = s.unwrap::<AbortSignal>(source.get(&s)).map(|d| d.reason(&s)).unwrap_or(s.undefined());
                    let target_signal = s.unwrap::<AbortSignal>(target.get(&s));
                    if let Some(d) = target_signal {
                        d.abort(&s, reason);
                    }
                }));
            }
            Ok(sig.value())
        });
    }
}

pub fn new_signal<'s>(s: &Scope<'s>) -> JsResult<Object<'s>> {
    let page: Rc<PageDom> = page_of(s)?;
    let target = EventTargetData::new(&page);
    let TargetKey::Other(k) = target.key else { unreachable!() };
    let key = page.other_identity(k);
    let data = AbortSignalData {
        target,
        aborted: Cell::new(false),
        reason: RefCell::new(None),
        remove_on_abort: RefCell::new(Vec::new()),
        hooks: RefCell::new(Vec::new()),
    };
    s.wrap::<AbortSignal>(data, Some(key))
}

pub struct ControllerData {
    signal: Persistent,
}

pub struct AbortController;
impl JsClass for AbortController {
    const NAME: &'static str = "AbortController";
    type Data = ControllerData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(0, |s, nt, _a| {
            let sig = new_signal(s)?;
            s.wrap_new::<AbortController>(ControllerData { signal: Persistent::new(s, sig.value()) }, nt, None)
        });
        def.getter("signal", |s, c| Ok(c.signal.get(s)));
        def.method("abort", 0, |s, c, a| {
            if let Some(d) = s.unwrap::<AbortSignal>(c.signal.get(s)) {
                d.abort(s, a.get(0));
            }
            Ok(s.undefined())
        });
    }
}

pub(crate) fn register(engine: &lp_js::Engine) {
    engine.register::<AbortSignal>();
    engine.register::<AbortController>();
}
