//! Prioritized Task Scheduling for window and worker event loops:
//! `postTask()` and `yield()`.
//!
//! A scheduler keeps the scheduling state (priority source and abort source)
//! of the task it is running: a `postTask()` callback, a `yield()`
//! continuation or a `requestIdleCallback()` callback (background), for the
//! callback and its microtask checkpoint. `yield()` inherits it; elsewhere
//! (timers, events, another frame's scheduler) it continues at user-visible
//! priority with no signal. JavaScriptCore has no hook that binds the state
//! to a promise reaction when it is created, so a reaction registered in
//! one task and run in another sees the running task's state, not its own.
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::Duration;

use crate::abort::{AbortRegistration, AbortSignal};
use crate::page::PageDom;
use crate::util::dom_string;
use pyro_js::{Args, ClassDef, Deferred, JsClass, JsError, JsResult, NoParent, Persistent, Scope, Value, WeakRealm};
use pyro_runtime::{WebPrioritySource, WebTaskPriority};
use rustc_hash::FxHashMap;

#[derive(Default)]
pub(crate) struct PendingTasks {
    next: Cell<u64>,
    records: RefCell<FxHashMap<u64, Rc<Task>>>,
    /// The scheduling state of the task this scheduler is running.
    current: RefCell<Option<Rc<SchedulingState>>>,
}

/// The spec's scheduling state. `priority` is `None` outside a scheduled
/// task's state (requestIdleCallback has a fixed background one).
struct SchedulingState {
    priority: Option<WebPrioritySource>,
    signal: Option<Persistent>,
}

impl PendingTasks {
    pub(crate) fn clear(&self) {
        // Drop JS roots and cancel delayed admissions before the realm detaches.
        drop(self.records.take());
        drop(self.current.take());
    }

    /// Runs `f` (a task's callback and its microtask checkpoint) with
    /// `state` as the current scheduling state, restoring the outer one.
    fn with_state<R>(&self, state: Option<Rc<SchedulingState>>, f: impl FnOnce() -> R) -> R {
        let outer = self.current.replace(state);
        let out = f();
        self.current.replace(outer);
        out
    }
    pub(crate) fn cancel_realm(&self, realm: pyro_js::RealmId) {
        let ids: Vec<_> =
            self.records.borrow().iter().filter_map(|(id, task)| (task.realm_id == realm).then_some(*id)).collect();
        for id in ids {
            self.remove(id);
        }
    }
    fn remove(&self, id: u64) -> Option<Rc<Task>> {
        self.records.borrow_mut().remove(&id)
    }
}

struct Task {
    /// `None` for a `yield()` continuation, which resolves with undefined.
    callback: Option<Persistent>,
    state: Rc<SchedulingState>,
    realm: WeakRealm,
    realm_id: pyro_js::RealmId,
    deferred: Deferred,
    cancel: RefCell<Option<Box<dyn FnOnce()>>>,
    abort: RefCell<Option<AbortRegistration>>,
}
impl Drop for Task {
    fn drop(&mut self) {
        if let Some(cancel) = self.cancel.get_mut().take() {
            cancel();
        }
    }
}

pub(crate) struct WebScheduler;
impl JsClass for WebScheduler {
    const NAME: &'static str = "Scheduler";
    type Data = Weak<PageDom>;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.promise_method("postTask", 1, post);
        def.promise_method("yield", 0, yield_);
    }
}

/// Runs a `requestIdleCallback()` callback: `yield()` from it continues at
/// background priority.
pub fn with_idle_scheduling_state<R>(page: &PageDom, f: impl FnOnce() -> R) -> R {
    let state = SchedulingState { priority: Some(WebPrioritySource::Fixed(WebTaskPriority::Background)), signal: None };
    page.web_tasks.with_state(Some(Rc::new(state)), f)
}

pub(crate) fn define_global<W: JsClass>(def: &mut ClassDef<W>)
where
    W::Data: crate::window::WindowHost,
{
    def.global_value_accessor(
        "scheduler",
        |s, w| {
            let page = crate::window::WindowHost::page(w);
            let holder = crate::worker_scope::module_holder(s, page)?;
            if let Some(value) = s.get_hidden(holder, "lpWebScheduler")? {
                return Ok(value);
            }
            let value = s.wrap::<WebScheduler>(Rc::downgrade(page), None)?.value();
            s.set_hidden(holder, "lpWebScheduler", value)?;
            Ok(value)
        },
        |s, w, value| crate::lazy_module::replace(s, crate::window::WindowHost::page(w), "scheduler", value),
    );
}

fn active_page(owner: &Weak<PageDom>) -> JsResult<Rc<PageDom>> {
    owner
        .upgrade()
        .filter(|p| p.is_fully_active())
        .ok_or_else(|| JsError::dom("NotSupportedError", "The scheduler is inactive."))
}

fn post<'s>(s: &Scope<'s>, owner: &Weak<PageDom>, a: Args<'s>) -> JsResult<Value<'s>> {
    a.require(1, "Scheduler.postTask")?;
    if !a.get(0).is_function() {
        return Err(JsError::Type("Expected a task callback".into()));
    }
    let options = a.get(1);
    let mut delay = Duration::ZERO;
    let mut priority = None;
    let mut signal = None;
    if !options.is_nullish() {
        let dict = options.as_object().ok_or_else(|| JsError::Type("Expected scheduler options".into()))?;
        let ms = dict.get_atom("delay")?;
        if !ms.is_undefined() {
            let ms = ms.to_number()?.trunc();
            if !(0.0..18_446_744_073_709_551_616.0).contains(&ms) {
                return Err(JsError::Type("delay is outside the unsigned long long range".into()));
            }
            delay = Duration::from_millis(ms as u64);
        }
        let explicit = dict.get_atom("priority")?;
        if !explicit.is_undefined() {
            priority = Some(WebPrioritySource::Fixed(match dom_string(explicit)?.as_str() {
                "user-blocking" => WebTaskPriority::UserBlocking,
                "user-visible" => WebTaskPriority::UserVisible,
                "background" => WebTaskPriority::Background,
                _ => return Err(JsError::Type("Invalid task priority".into())),
            }));
        }
        let sig = dict.get_atom("signal")?;
        if !sig.is_undefined() {
            if s.unwrap::<AbortSignal>(sig).is_none() {
                return Err(JsError::Type("Expected an AbortSignal".into()));
            }
            if priority.is_none() {
                priority = crate::task_signal::priority(s, sig).map(WebPrioritySource::Signal);
            }
            signal = Some(Persistent::new(s, sig));
        }
    }
    let page = active_page(owner)?;
    let priority = priority.unwrap_or(WebPrioritySource::Fixed(WebTaskPriority::UserVisible));
    let state = Rc::new(SchedulingState { priority: Some(priority.clone()), signal });
    enqueue(s, &page, Some(Persistent::new(s, a.get(0))), state, priority, Some(delay))
}

/// `scheduler.yield()`: a continuation with the inherited priority and
/// abort sources, which outranks tasks of that priority.
fn yield_<'s>(s: &Scope<'s>, owner: &Weak<PageDom>, _a: Args<'s>) -> JsResult<Value<'s>> {
    let page = active_page(owner)?;
    let inherited = page.web_tasks.current.borrow().clone();
    let state = inherited.unwrap_or_else(|| Rc::new(SchedulingState { priority: None, signal: None }));
    let priority = state.priority.clone().unwrap_or(WebPrioritySource::Fixed(WebTaskPriority::UserVisible));
    enqueue(s, &page, None, state, priority, None)
}

/// Queues a task (`delay` set) or a continuation (`callback` and `delay`
/// unset) whose promise the state's signal can reject.
fn enqueue<'s>(
    s: &Scope<'s>,
    page: &Rc<PageDom>,
    callback: Option<Persistent>,
    state: Rc<SchedulingState>,
    priority: WebPrioritySource,
    delay: Option<Duration>,
) -> JsResult<Value<'s>> {
    let deferred = s.deferred()?;
    let promise = deferred.promise.get(s);
    let signal = state.signal.as_ref().and_then(|p| s.unwrap::<AbortSignal>(p.get(s)));
    if let Some(aborted) = signal.as_ref().filter(|sig| sig.aborted()) {
        deferred.reject(s, aborted.reason(s))?;
        return Ok(promise);
    }
    let hooks = page.hooks().ok_or_else(|| JsError::dom("NotSupportedError", "No event loop for the scheduler."))?;
    let id = page.web_tasks.next.get();
    page.web_tasks.next.set(id.wrapping_add(1));
    let record = Rc::new(Task {
        realm: s.realm().downgrade(),
        realm_id: s.realm_id(),
        callback,
        deferred,
        state,
        cancel: RefCell::new(None),
        abort: RefCell::new(None),
    });
    page.web_tasks.records.borrow_mut().insert(id, record.clone());
    if let Some(sig) = signal {
        let weak = Rc::downgrade(page);
        let reg = sig.register_native(Box::new(move |_| {
            let Some(page) = weak.upgrade() else { return };
            let Some(record) = page.web_tasks.remove(id) else { return };
            let Some(realm) = record.realm.upgrade() else { return };
            let Ok(s) = realm.scope() else { return };
            if let Some(signal) = record.state.signal.as_ref().and_then(|v| s.unwrap::<AbortSignal>(v.get(&s))) {
                let _ = record.deferred.reject(&s, signal.reason(&s));
            };
        }));
        record.abort.replace(reg);
    }
    let weak = Rc::downgrade(page);
    let run_task: Box<dyn FnOnce()> = Box::new(move || {
        if let Some(page) = weak.upgrade() {
            run(&page, id)
        }
    });
    let cancel = match delay {
        Some(delay) => hooks.post_web_task(priority, delay, run_task),
        None => hooks.post_web_continuation(priority, run_task),
    };
    if let Some(cancel) = cancel {
        record.cancel.replace(Some(cancel));
    } else {
        page.web_tasks.remove(id);
        return Err(JsError::dom("NotSupportedError", "The event loop does not support scheduling."));
    }
    Ok(promise)
}

fn run(page: &Rc<PageDom>, id: u64) {
    let Some(task) = page.web_tasks.records.borrow().get(&id).cloned() else { return };
    let Some(realm) = task.realm.upgrade().filter(|r| !r.is_disposed() && page.is_fully_active()) else {
        page.web_tasks.remove(id);
        return;
    };
    let Ok(s) = realm.scope() else {
        page.web_tasks.remove(id);
        return;
    };
    let owner = page.clone();
    let state = task.state.clone();
    // Let an abort during the synchronous part of the callback reject its
    // promise; disconnect the abort algorithm before assimilating its result.
    // The scheduling state stays current through the microtask checkpoint
    // that ends the frame.
    let frame = page.web_tasks.with_state(Some(state), || {
        s.in_script_frame(move |s| {
            let value = match task.callback.as_ref() {
                Some(callback) => s.invoke_callback(callback.get(s).as_object().expect("task callback")),
                None => Ok(s.undefined()),
            };
            if owner.web_tasks.remove(id).is_some() {
                task.abort.take();
                match value {
                    Ok(v) => {
                        let _ = task.deferred.resolve(s, v);
                    }
                    Err(e) => {
                        let _ = task.deferred.reject(s, e.to_value(s));
                    }
                }
            }
        })
    });
    if let Err(error) = frame {
        if let Some(record) = page.web_tasks.remove(id) {
            let _ = record.deferred.reject(&s, error.to_value(&s));
        }
    }
}
