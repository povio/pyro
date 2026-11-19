//! AbortController / AbortSignal.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::util::WrapperRef;
use pyro_dom::events::TargetKey;
use pyro_js::{ClassDef, JsClass, JsError, JsResult, NoParent, Object, Persistent, Scope, Value};

use crate::event::{EventCore, EventInterface};
use crate::page::PageDom;
use crate::target::{fire, page_of, EventTarget, EventTargetData};

type AbortHook = Box<dyn FnOnce(&PageDom)>;

/// The abort reason, a hidden property of the signal's wrapper.
const REASON: &str = "__lpAbortReason";
/// The signal's "dependent signals" (`AbortSignal.any` results that follow
/// it), an array on the signal's wrapper.
const DEPENDENTS: &str = "__lpAbortDependents";
/// A dependent signal's "source signals", an array on its wrapper.
const SOURCES: &str = "__lpAbortSources";

thread_local! {
    /// The first exception an abort algorithm of script threw while a signal
    /// was aborting (an Observable closing an iterator whose `return()`
    /// throws). The abort steps go on; the exception leaves through the
    /// `abort()` call that started them.
    static ALGORITHM_EXCEPTION: RefCell<Option<JsError>> = const { RefCell::new(None) };
}

/// Records the exception of an abort algorithm for the `abort()` call that is
/// running it; only the first one is kept.
pub(crate) fn algorithm_threw(error: JsError) {
    ALGORITHM_EXCEPTION.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            *slot = Some(error);
        }
    });
}

/// Rethrows what an abort algorithm threw during the abort that just ran.
fn rethrow_algorithm_exception() -> JsResult<()> {
    match ALGORITHM_EXCEPTION.with(|slot| slot.borrow_mut().take()) {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

/// Identity of a native abort step; never reused within one signal.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct AbortHookId(u64);

impl AbortHookId {
    /// The identity as a number (never 0) for bookkeeping kept in script.
    pub(crate) fn to_number(self) -> f64 {
        self.0 as f64
    }
    pub(crate) fn from_number(id: f64) -> AbortHookId {
        AbortHookId(id as u64)
    }
}

pub struct AbortSignalData {
    pub target: EventTargetData,
    aborted: Cell<bool>,
    /// Made by `AbortSignal.any`: it follows its source signals and has no
    /// dependent signals of its own.
    dependent: Cell<bool>,
    /// The signal's own wrapper, found again through its realm's identity map.
    /// The abort reason is a hidden property of that wrapper, which the GC
    /// traces: a `Persistent` here could be rooted through the reason (an
    /// object that reaches the signal) and keep the realm alive after close.
    pub(crate) wrapper: WrapperRef,
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
    /// The reason stored on the signal wrapper behind `wrapper`, if the wrapper is
    /// still alive and the signal has aborted.
    pub(crate) fn stored_reason<'s>(wrapper: &WrapperRef, s: &Scope<'s>) -> Option<Value<'s>> {
        wrapper.hidden(s, REASON).ok().flatten()
    }
    pub(crate) fn register_native(&self, hook: AbortHook) -> Option<AbortRegistration> {
        self.add_abort_hook(hook).map(|id| AbortRegistration { hooks: Rc::downgrade(&self.hooks), id })
    }
    pub fn aborted(&self) -> bool {
        self.aborted.get()
    }
    pub fn reason<'s>(&self, s: &Scope<'s>) -> Value<'s> {
        Self::stored_reason(&self.wrapper, s).unwrap_or(s.undefined())
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

    /// "signal abort": the signal and its dependent signals take the reason,
    /// then the signal's abort steps run, then those of each dependent.
    pub fn abort(&self, s: &Scope<'_>, reason: Value<'_>) {
        if !self.set_aborted(s, reason) {
            return;
        }
        // The dependents stay on the wrapper, which is alive; only the ones
        // this abort reached run their steps.
        let dependents = self.wrapper.hidden(s, DEPENDENTS).ok().flatten().and_then(|v| v.as_object());
        let mut reached = Vec::new();
        if let Some(list) = dependents {
            let reason = self.reason(s);
            let len = list.get_atom("length").and_then(|v| v.to_number()).unwrap_or(0.0) as u32;
            for i in 0..len {
                let Some(dependent) = list.get_index(i).ok().and_then(|v| s.unwrap::<AbortSignal>(v)) else { continue };
                if dependent.set_aborted(s, reason) {
                    reached.push(i);
                }
            }
        }
        self.run_abort_steps();
        if let Some(list) = dependents {
            for i in reached {
                if let Some(dependent) = list.get_index(i).ok().and_then(|v| s.unwrap::<AbortSignal>(v)) {
                    dependent.run_abort_steps();
                }
            }
        }
    }

    /// Sets the abort reason; false when the signal was already aborted.
    fn set_aborted(&self, s: &Scope<'_>, reason: Value<'_>) -> bool {
        if self.aborted.replace(true) {
            return false;
        }
        let reason = if reason.is_undefined() {
            JsError::dom("AbortError", "signal is aborted without reason").to_value(s)
        } else {
            reason
        };
        // Written before any hook or listener runs, so they all observe it. A failed write (the
        // wrapper is already gone) leaves the reason undefined; the hooks still run.
        if let Ok(wrapper) = self.wrapper.get(s) {
            let _ = s.set_hidden(wrapper, REASON, reason);
        }
        true
    }

    /// "run the abort steps": the abort algorithms, then the `abort` event.
    fn run_abort_steps(&self) {
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
        def.accessor(
            "onabort",
            |s, d| crate::target::event_handler(s, &d.target, "abort"),
            |s, d, v| crate::target::set_event_handler(s, &d.target, "abort", v),
        );
        def.method("throwIfAborted", 0, |s, d, _a| {
            if d.aborted() {
                let r = d.reason(s);
                return Err(JsError::thrown(Persistent::new(s, r)));
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
            a.require(1, "AbortSignal.timeout")?;
            let ms = a.get(0).to_number()?;
            let sig = new_signal(s)?;
            let keep = Persistent::new(s, sig.value());
            let cb = s.function("abortTimeout", move |s, _this, _args| {
                if let Some(d) = s.unwrap::<AbortSignal>(keep.get(s)) {
                    let reason = JsError::dom("TimeoutError", "signal timed out").to_value(s);
                    d.abort(s, reason);
                }
                rethrow_algorithm_exception()?;
                Ok(s.undefined())
            })?;
            let set_timeout = s.global().get_atom("setTimeout")?;
            if let Some(f) = set_timeout.as_object() {
                f.call(None, &[cb.value(), s.number(ms)])?;
            }
            Ok(sig.value())
        });
        def.static_method("any", 1, |s, a| {
            let sig = new_signal(s)?;
            let list =
                a.get(0).as_object().ok_or_else(|| JsError::Type("AbortSignal.any requires an iterable".into()))?;
            let len = list.get_atom("length")?.to_number()? as u32;
            // "create a dependent abort signal": an aborted signal in the
            // list gives its reason; otherwise the result follows each
            // signal, or the sources of one that is itself dependent.
            let Some(result) = s.unwrap::<AbortSignal>(sig.value()) else { return Ok(sig.value()) };
            for i in 0..len {
                let Some(src) = s.unwrap::<AbortSignal>(list.get_index(i)?) else { continue };
                if src.aborted() {
                    result.set_aborted(s, src.reason(s));
                    return Ok(sig.value());
                }
            }
            result.dependent.set(true);
            for i in 0..len {
                let v = list.get_index(i)?;
                let Some(src) = s.unwrap::<AbortSignal>(v) else { continue };
                if !src.dependent.get() {
                    follow(s, v, sig)?;
                    continue;
                }
                let Some(sources) = s.get_hidden(src.wrapper.get(s)?, SOURCES)?.and_then(|v| v.as_object()) else {
                    continue;
                };
                let count = sources.get_atom("length")?.to_number()? as u32;
                for j in 0..count {
                    follow(s, sources.get_index(j)?, sig)?;
                }
            }
            Ok(sig.value())
        });
    }
}

/// Makes the new signal `dependent` a dependent signal of the signal `source`
/// (and `source` one of its source signals), once.
fn follow<'s>(s: &Scope<'s>, source: Value<'s>, dependent: Object<'s>) -> JsResult<()> {
    let Some(source) = source.as_object() else { return Ok(()) };
    let dependents = hidden_list(s, source, DEPENDENTS)?;
    let len = dependents.get_atom("length")?.to_number()? as u32;
    // A source reached twice: `dependent` was just made, so it can only be
    // the entry this `AbortSignal.any` call appended last.
    if len > 0 && dependents.get_index(len - 1)?.strict_equals(dependent.value()) {
        return Ok(());
    }
    dependents.set_index(len, dependent.value())?;
    let sources = hidden_list(s, dependent, SOURCES)?;
    let len = sources.get_atom("length")?.to_number()? as u32;
    sources.set_index(len, source.value())
}

/// The array kept as `owner`'s hidden property `key`, made on first use.
fn hidden_list<'s>(s: &Scope<'s>, owner: Object<'s>, key: &'static str) -> JsResult<Object<'s>> {
    if let Some(list) = s.get_hidden(owner, key)?.and_then(|v| v.as_object()) {
        return Ok(list);
    }
    let list = s.array(&[])?;
    s.set_hidden(owner, key, list.value())?;
    Ok(list)
}

pub fn new_signal<'s>(s: &Scope<'s>) -> JsResult<Object<'s>> {
    let page: Rc<PageDom> = page_of(s)?;
    let target = EventTargetData::new(&page);
    let TargetKey::Other(k) = target.key else { unreachable!() };
    let key = page.other_identity(k);
    let data = AbortSignalData {
        target,
        aborted: Cell::new(false),
        dependent: Cell::new(false),
        wrapper: WrapperRef::new(s, key),
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
            rethrow_algorithm_exception()?;
            Ok(s.undefined())
        });
    }
}

pub(crate) fn register(engine: &pyro_js::Engine) {
    engine.register::<AbortSignal>();
    engine.register::<AbortController>();
}

/// This module's part of [`crate::thread_state::reset_thread`]: what of its
/// per-thread state still holds something, if any does.
pub(crate) fn thread_leftover() -> Option<&'static str> {
    ALGORITHM_EXCEPTION.with(|e| e.borrow().is_some()).then_some("abort::ALGORITHM_EXCEPTION")
}
