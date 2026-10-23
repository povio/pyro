//! AbortController / AbortSignal.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::util::WrapperRef;
use lp_dom::events::TargetKey;
use lp_js::{ClassDef, JsClass, JsError, JsResult, NoParent, Object, Persistent, Scope, Value};

use crate::event::{EventCore, EventInterface};
use crate::page::PageDom;
use crate::target::{fire, page_of, EventTarget, EventTargetData};

type AbortHook = Box<dyn FnOnce(&PageDom)>;

/// Identity of a native abort step; never reused within one signal.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct AbortHookId(u64);

pub struct AbortSignalData {
    pub target: EventTargetData,
    aborted: Cell<bool>,
    reason: Rc<RefCell<Option<Persistent>>>,
    /// Listener registrations to drop on abort (`{signal}` option).
    remove_on_abort: RefCell<Vec<(TargetKey, u32)>>,
    /// Native abort steps (fetch/XHR cancellation).
    hooks: Rc<RefCell<Vec<(AbortHookId, AbortHook)>>>,
    next_hook: Cell<u64>,
}

pub(crate) struct AbortRegistration {
    hooks: std::rc::Weak<RefCell<Vec<(AbortHookId, AbortHook)>>>,
    id: AbortHookId,
}
impl Drop for AbortRegistration {
    fn drop(&mut self) {
        if let Some(hooks) = self.hooks.upgrade() {
            let removed = {
                let mut hooks = hooks.borrow_mut();
                hooks.iter().position(|(id, _)| *id == self.id).map(|i| hooks.remove(i))
            };
            drop(removed);
        }
    }
}

impl AbortSignalData {
    pub(crate) fn reason_weak(&self) -> std::rc::Weak<RefCell<Option<Persistent>>> {
        Rc::downgrade(&self.reason)
    }
    pub(crate) fn register_native(&self, hook: AbortHook) -> Option<AbortRegistration> {
        self.add_abort_hook(hook).map(|id| AbortRegistration { hooks: Rc::downgrade(&self.hooks), id })
    }
    pub fn aborted(&self) -> bool {
        self.aborted.get()
    }
    pub fn reason<'s>(&self, s: &Scope<'s>) -> Value<'s> {
        self.reason.borrow().as_ref().map_or(s.undefined(), |p| p.get(s))
    }
    pub(crate) fn on_abort_remove(&self, key: TargetKey, registration: u32) {
        self.remove_on_abort.borrow_mut().push((key, registration));
    }
    /// Registers native abort steps (run once, on abort).
    pub fn add_abort_hook(&self, hook: AbortHook) -> Option<AbortHookId> {
        if self.aborted.get() {
            hook(&self.target.page);
            None
        } else {
            let id = AbortHookId(self.next_hook.get());
            self.next_hook.set(id.0.checked_add(1).expect("abort hook identity exhausted"));
            self.hooks.borrow_mut().push((id, hook));
            Some(id)
        }
    }

    /// Releases completed work without waiting for the signal to abort.
    pub fn remove_abort_hook(&self, id: AbortHookId) {
        let removed = {
            let mut hooks = self.hooks.borrow_mut();
            hooks.iter().position(|(key, _)| *key == id).map(|index| hooks.remove(index))
        };
        // A payload destructor can reenter this signal.
        drop(removed);
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
        for (key, registration) in self.remove_on_abort.take() {
            page.dom.borrow_mut().abort_listener(key, registration);
        }
        for (_, hook) in self.hooks.take() {
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
                let source_reason = Rc::downgrade(&src.reason);
                src.add_abort_hook(Box::new(move |_page| {
                    let Some(realm) = realm.upgrade() else { return };
                    let Ok(s) = realm.scope() else { return };
                    // The caller keeps native source data alive while abort
                    // steps run, including when the source is from another realm.
                    // A weak native reference avoids rooting our own JS wrapper.
                    let source_reason = source_reason.upgrade();
                    let reason = source_reason
                        .as_ref()
                        .and_then(|r| r.borrow().as_ref().map(|r| r.get(&s)))
                        .unwrap_or(s.undefined());
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
        reason: Rc::new(RefCell::new(None)),
        remove_on_abort: RefCell::new(Vec::new()),
        hooks: Rc::new(RefCell::new(Vec::new())),
        next_hook: Cell::new(1),
    };
    s.wrap::<AbortSignal>(data, Some(key))
}

/// The controller's signal, a hidden property of the controller's wrapper.
const SIGNAL: &str = "__lpAbortControllerSignal";

pub struct ControllerData {
    wrapper: WrapperRef,
}

impl ControllerData {
    fn signal<'s>(&self, s: &Scope<'s>) -> JsResult<Value<'s>> {
        Ok(self.wrapper.hidden(s, SIGNAL)?.unwrap_or(s.undefined()))
    }
}

pub struct AbortController;
impl JsClass for AbortController {
    const NAME: &'static str = "AbortController";
    type Data = ControllerData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(0, |s, nt, _a| {
            let sig = new_signal(s)?;
            let page = page_of(s)?;
            let identity = page.event_identity(page.next_event_id());
            let data = ControllerData { wrapper: WrapperRef::new(s, identity) };
            let controller = s.wrap_new::<AbortController>(data, nt, Some(identity))?;
            s.set_hidden(controller, SIGNAL, sig.value())?;
            Ok(controller)
        });
        def.getter("signal", |s, c| c.signal(s));
        def.method("abort", 0, |s, c, a| {
            if let Some(d) = s.unwrap::<AbortSignal>(c.signal(s)?) {
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
