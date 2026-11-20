//! TaskController, TaskSignal and their trusted priority change event.
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use pyro_js::{ClassDef, JsClass, JsError, JsResult, Object, Persistent, Scope, Value};
use pyro_runtime::WebTaskPriority;

use crate::abort::{self, AbortController, AbortSignal, AbortSignalData, ControllerData};
use crate::event::{self, Event, EventCore, EventData, EventFields, EventInterface};
use crate::target;
use crate::util::{WrapperRef, dom_string};

fn parse(value: Value<'_>) -> JsResult<WebTaskPriority> {
    match dom_string(value)?.as_str() {
        "background" => Ok(WebTaskPriority::Background),
        "user-visible" => Ok(WebTaskPriority::UserVisible),
        "user-blocking" => Ok(WebTaskPriority::UserBlocking),
        _ => Err(JsError::Type("Invalid TaskPriority".into())),
    }
}
fn name(p: WebTaskPriority) -> &'static str {
    match p {
        WebTaskPriority::Background => "background",
        WebTaskPriority::UserVisible => "user-visible",
        WebTaskPriority::UserBlocking => "user-blocking",
    }
}
pub(crate) fn priority(s: &Scope<'_>, value: Value<'_>) -> Option<Rc<Cell<WebTaskPriority>>> {
    Some(s.unwrap::<TaskSignal>(value)?.level.clone())
}

#[derive(Clone)]
struct Dependent {
    level: Weak<Cell<WebTaskPriority>>,
    wrapper: WrapperRef,
}
pub(crate) struct SignalData {
    abort: AbortSignalData,
    level: Rc<Cell<WebTaskPriority>>,
    changing: Cell<bool>,
    // A controller's signal is a root. Composites flatten directly to that
    // root, preserving creation order across intermediate composites.
    source: Option<WrapperRef>,
    fixed: bool,
    dependents: RefCell<Vec<Dependent>>,
}

fn new_signal<'s>(s: &Scope<'s>, p: WebTaskPriority, fixed: bool, source: Option<WrapperRef>) -> JsResult<Object<'s>> {
    let (abort, key) = abort::signal_data(s)?;
    let data = SignalData { abort, level: Rc::new(Cell::new(p)), changing: Cell::new(false), source, fixed,
        dependents: RefCell::new(Vec::new()) };
    s.wrap::<TaskSignal>(data, Some(key))
}

fn notify(d: &SignalData, previous: WebTaskPriority) {
    let t = &d.abort.target;
    let ev = EventCore::trusted(&t.page, EventInterface::TaskPriorityChangeEvent, "prioritychange", false, false);
    ev.fields.borrow_mut().previous_task_priority = Some(previous);
    target::fire(&t.page, t.key, &ev);
}

fn set_priority(s: &Scope<'_>, d: &SignalData, p: WebTaskPriority) -> JsResult<()> {
    if d.changing.get() {
        return Err(JsError::dom("NotAllowedError", "A priority change is already in progress."));
    }
    let previous = d.level.replace(p);
    if previous == p { return Ok(()); }
    d.changing.set(true);
    notify(d, previous);
    // Listener callbacks may have created further composites. Do not hold
    // a borrow while dispatching events into script.
    d.dependents.borrow_mut().retain(|dep| dep.level.strong_count() != 0);
    let mut index = 0;
    loop {
        let Some(dep) = d.dependents.borrow().get(index).cloned() else { break };
        index += 1;
        if let Some(level) = dep.level.upgrade() {
            let prev = level.replace(p);
            if prev != p {
                if let Some(signal) = dep.wrapper.get(s).ok().and_then(|o| s.unwrap::<TaskSignal>(o.value())) {
                    notify(&signal, prev);
                }
            }
        }
    }
    d.changing.set(false);
    Ok(())
}

fn priority_member<'s>(value: Value<'s>) -> JsResult<Option<Value<'s>>> {
    if value.is_nullish() { return Ok(None); }
    let obj = value.as_object().ok_or_else(|| JsError::Type("Expected a task priority options dictionary".into()))?;
    Ok(Some(obj.get_atom("priority")?).filter(|v| !v.is_undefined()))
}

/// A priority source traces its composite signals only while their event
/// listeners could observe a priority change. Queue entries retain their
/// own priority cells independently. No protected JS roots cross realms.
pub(crate) fn sync_listener(s: &Scope<'_>, wrapper: Object<'_>) -> JsResult<()> {
    let Some(signal) = s.unwrap::<TaskSignal>(wrapper.value()) else { return Ok(()) };
    let Some(root) = signal.source.as_ref().and_then(|r| r.get(s).ok()) else { return Ok(()) };
    let t = &signal.abort.target;
    let retain = t.page.dom.borrow().has_listener_for(t.key, "prioritychange");
    const LISTENERS: &str = "__lpTaskSignalPriorityListeners";
    let holders = match s.get_hidden(root, LISTENERS)?.and_then(|v| v.as_object()) {
        Some(h) => h,
        None if retain => {
            let h = s.object();
            h.set_prototype(s.null());
            s.set_hidden(root, LISTENERS, h.value())?;
            h
        }
        None => return Ok(()),
    };
    let key = t.wrapper_identity().expect("signal target");
    let entry = format!("{}:{}", key.0, key.1);
    if retain { holders.set(&entry, wrapper.value())?; } else { holders.delete(&entry)?; }
    Ok(())
}

pub(crate) struct TaskController;
impl JsClass for TaskController {
    const NAME: &'static str = "TaskController";
    type Parent = AbortController;
    type Data = ControllerData;
    fn define(def: &mut ClassDef<Self>) {
        def.upcast(|d| d);
        def.constructor(0, |s, nt, a| {
            let p = priority_member(a.get(0))?.map(parse).transpose()?.unwrap_or(WebTaskPriority::UserVisible);
            let signal = new_signal(s, p, false, None)?;
            let page = target::page_of(s)?;
            let key = page.event_identity(page.next_event_id());
            let data = ControllerData { wrapper: WrapperRef::new(s, key) };
            let controller = s.wrap_new::<Self>(data, nt, Some(key))?;
            s.set_hidden(controller, abort::SIGNAL, signal.value())?;
            Ok(controller)
        });
        def.method("setPriority", 1, |s, d, a| {
            a.require(1, "TaskController.setPriority")?;
            let p = parse(a.get(0))?;
            let sig = s.unwrap::<TaskSignal>(d.signal(s)?).expect("controller owns TaskSignal");
            set_priority(s, &sig, p)?;
            Ok(s.undefined())
        });
    }
}

pub(crate) struct TaskSignal;
impl JsClass for TaskSignal {
    const NAME: &'static str = "TaskSignal";
    type Parent = AbortSignal;
    type Data = SignalData;
    fn define(def: &mut ClassDef<Self>) {
        def.upcast(|d| &d.abort);
        def.getter("priority", |s, d| Ok(s.string(name(d.level.get()))));
        def.accessor("onprioritychange",
            |s,d| target::event_handler(s, &d.abort.target, "prioritychange"),
            |s,d,v| target::set_event_handler(s, &d.abort.target, "prioritychange", v));
        def.static_method("any", 1, |s,a| {
            a.require(1, "TaskSignal.any")?;
            let signals = s.convert_sequence(a.get(0), "TaskSignal.any", |s,v| {
                s.unwrap::<AbortSignal>(v).ok_or_else(|| JsError::Type("Expected AbortSignal".into()))?;
                Ok(Persistent::new(s,v))
            })?;
            let p = priority_member(a.get(1))?;
            let parent = p.and_then(|v| s.unwrap::<TaskSignal>(v));
            let level = match &parent {
                Some(parent) => parent.level.get(),
                None => p.map(parse).transpose()?.unwrap_or(WebTaskPriority::UserVisible),
            };
            let source = parent.as_ref().and_then(|d| if d.fixed { None } else { d.source.clone().or_else(|| Some(d.abort.wrapper.clone())) });
            let signal = new_signal(s, level, source.is_none(), source.clone())?;
            let list = s.array(&signals.iter().map(|v| v.get(s)).collect::<Vec<_>>())?;
            abort::link_dependents(s, signal, list, signals.len() as u32)?;
            if let Some(root) = source.and_then(|w| w.get(s).ok()).and_then(|o| s.unwrap::<TaskSignal>(o.value())) {
                let data = s.unwrap::<Self>(signal.value()).expect("new signal");
                let mut list = root.dependents.borrow_mut();
                if !root.changing.get() && list.len() == list.capacity() { list.retain(|dep| dep.level.strong_count() != 0); }
                list.push(Dependent {level: Rc::downgrade(&data.level), wrapper: data.abort.wrapper.clone()});
            }
            Ok(signal.value())
        });
    }
}

pub(crate) struct TaskPriorityChangeEvent;
impl JsClass for TaskPriorityChangeEvent {
    const NAME: &'static str = "TaskPriorityChangeEvent";
    type Parent = Event;
    type Data = EventData;
    fn define(def: &mut ClassDef<Self>) {
        def.upcast(|d| d);
        def.constructor(2, |s, nt, a| {
            a.require(2, "TaskPriorityChangeEvent")?;
            event::construct::<Self>(s, nt, a, EventInterface::TaskPriorityChangeEvent, fill_event)
        });
        def.getter("previousPriority", |s, e| Ok(s.string(name(e.fields.borrow().previous_task_priority.expect("event priority")))));
    }
}
fn fill_event(_s: &Scope<'_>, fields: &mut EventFields, init: Option<Object<'_>>) -> JsResult<()> {
    let value = init.map(|o| o.get_atom("previousPriority")).transpose()?.filter(|v| !v.is_undefined())
        .ok_or_else(|| JsError::Type("previousPriority is required".into()))?;
    fields.previous_task_priority = Some(parse(value)?);
    Ok(())
}

pub(crate) fn register(engine: &pyro_js::Engine) {
    engine.register::<TaskSignal>();
    engine.register::<TaskController>();
    engine.register::<TaskPriorityChangeEvent>();
}
