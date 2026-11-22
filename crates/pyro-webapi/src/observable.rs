//! WICG Observable: `Observable`, `Subscriber` and `EventTarget.when()`,
//! made in JS (observable.js) on a realm's first use. The script gets the
//! parts of the platform it cannot reach from JS as functions (`natives`):
//! HTML "report an exception", whether the document is fully active, the
//! AbortSignal brand check and DOM "add an abort algorithm".
use std::rc::Rc;

use pyro_js::{ClassDef, JsClass, JsError, JsException, JsResult, Object, Persistent, Scope, Value};

use crate::abort::{AbortHookId, AbortSignal};
use crate::page::PageDom;
use crate::target::page_of;
use crate::window::WindowHost;

/// Kept on the realm's `navigator` (a hidden property), itself kept per realm.
const KEY: &str = "lpObservable";

/// Adds the lazy interface names to Window and WorkerGlobalScope.
pub(crate) fn define_globals<W>(def: &mut ClassDef<W>)
where
    W: JsClass,
    W::Data: WindowHost,
{
    def.global_value_accessor(
        "Observable",
        |s, w| global(s, w.page(), "Observable"),
        |s, w, value| replace(s, w.page(), "Observable", value),
    );
    def.global_value_accessor(
        "Subscriber",
        |s, w| global(s, w.page(), "Subscriber"),
        |s, w, value| replace(s, w.page(), "Subscriber", value),
    );
}

fn global<'s>(s: &Scope<'s>, page: &Rc<PageDom>, name: &str) -> JsResult<Value<'s>> {
    let value = module(s, page)?.get(name)?;
    replace(s, page, name, value)?;
    Ok(value)
}

fn replace(s: &Scope<'_>, page: &Rc<PageDom>, name: &str, value: Value<'_>) -> JsResult<()> {
    let define = page.define_properties.borrow().get(&s.realm_id()).map(|p| p.get(s));
    let define = define.and_then(|v| v.as_object()).ok_or_else(unavailable)?;
    let desc = s.object();
    desc.set_atom("value", value)?;
    desc.set_atom("writable", s.boolean(true))?;
    desc.set_atom("enumerable", s.boolean(false))?;
    desc.set_atom("configurable", s.boolean(true))?;
    define.call(None, &[s.global().value(), s.string(name), desc.value()])?;
    Ok(())
}

fn unavailable() -> JsError {
    JsError::Type("Observable is unavailable".into())
}

/// This realm's module, made on first use.
pub(crate) fn module<'s>(s: &Scope<'s>, page: &Rc<PageDom>) -> JsResult<Object<'s>> {
    let navigator = crate::worker_scope::module_holder(s, page)?;
    if let Some(module) = s.get_hidden(navigator, KEY)?.and_then(|v| v.as_object()) {
        return Ok(module);
    }
    let make = s.eval(include_str!("observable.js"), Some("pyro://internal/observable.js"), 1)?;
    let module = make.as_object().ok_or_else(unavailable)?.call(None, &[natives(s)?.value()])?;
    s.set_hidden(navigator, KEY, module)?;
    module.as_object().ok_or_else(unavailable)
}

/// HTML "report an exception" with `value`. An Error carries the location it
/// was made at; any other value (`subscriber.error("text")`) is reported at
/// the script that is running, where a `throw` of it would have been.
pub(crate) fn report_exception<'s>(s: &Scope<'s>, value: Value<'s>) {
    let mut details = JsException::from_value(s, value);
    if details.line.is_none() {
        if let Some((url, line, column)) = crate::csp::running_script_location(s) {
            details.url = Some(url);
            details.line = Some(line);
            details.column = Some(column);
        }
    }
    // The `error` event at the Window first; the console unless a listener
    // canceled it.
    if !crate::lifecycle::report_exception(s, &details) {
        crate::lifecycle::report_diagnostic(s, &JsError::thrown(Persistent::new(s, value)));
    }
}

/// The engine's helpers of observable.js.
fn natives<'s>(s: &Scope<'s>) -> JsResult<Object<'s>> {
    let natives = s.object();
    // HTML "report an exception" for the realm's global: `error` at the
    // Window, then the console, as for any uncaught exception of a callback.
    let report = s.function("reportException", |s, _this, a| {
        report_exception(s, a.get(0));
        Ok(s.undefined())
    })?;
    natives.set_atom("reportException", report.value())?;
    // Whether the realm's document is fully active: a Subscriber does
    // nothing once its document is detached.
    let active =
        s.function("fullyActive", |s, _this, _a| Ok(s.boolean(page_of(s).is_ok_and(|p| p.is_fully_active()))))?;
    natives.set_atom("fullyActive", active.value())?;
    let is_signal =
        s.function("isAbortSignal", |s, _this, a| Ok(s.boolean(s.unwrap::<AbortSignal>(a.get(0)).is_some())))?;
    natives.set_atom("isAbortSignal", is_signal.value())?;
    // DOM "add an abort algorithm": when the signal aborts, the module runs
    // the steps it keeps under `key` for the signal. The steps stay in JS
    // (the GC sees what they reach); the signal only holds the key. Returns
    // the hook's identity for `removeAbortHook`.
    let add = s.function("addAbortHook", |s, _this, a| {
        let signal = s.unwrap::<AbortSignal>(a.get(0)).ok_or_else(|| JsError::Type("not an AbortSignal".into()))?;
        let key = a.get(1).to_number()?;
        let realm = s.realm().downgrade();
        let wrapper = signal.wrapper.clone();
        let hook = signal.add_abort_hook(Box::new(move |_page| {
            let Some(realm) = realm.upgrade() else { return };
            let Ok(s) = realm.scope() else { return };
            let run = || -> JsResult<()> {
                let module = module(&s, &page_of(&s)?)?;
                let run = module.get_atom("runAbortAlgorithm")?.as_object().ok_or_else(unavailable)?;
                run.call(None, &[wrapper.get(&s)?.value(), s.number(key)])?;
                Ok(())
            };
            // The abort goes on; the `abort()` call that started it rethrows.
            if let Err(error) = run() {
                crate::abort::algorithm_threw(error);
            }
        }));
        Ok(s.number(hook.map_or(0.0, AbortHookId::to_number)))
    })?;
    natives.set_atom("addAbortHook", add.value())?;
    // DOM "remove an abort algorithm", for steps that became moot.
    let remove = s.function("removeAbortHook", |s, _this, a| {
        if let Some(signal) = s.unwrap::<AbortSignal>(a.get(0)) {
            signal.remove_abort_hook(AbortHookId::from_number(a.get(1).to_number()?));
        }
        Ok(s.undefined())
    })?;
    natives.set_atom("removeAbortHook", remove.value())?;
    Ok(natives)
}

/// `EventTarget.when(type, options)`.
pub(crate) fn when<'s>(s: &Scope<'s>, target: Value<'s>, ty: Value<'s>, options: Value<'s>) -> JsResult<Value<'s>> {
    let page = crate::target::page_of(s)?;
    let module = module(s, &page)?;
    module.get_atom("when")?.as_object().ok_or_else(unavailable)?.call(None, &[target, ty, options])
}
