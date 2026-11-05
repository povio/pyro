//! WICG Observable: `Observable`, `Subscriber` and `EventTarget.when()`,
//! made in JS (observable.js) on a realm's first use.
use std::rc::Rc;

use lp_js::{ClassDef, JsClass, JsError, JsResult, Object, Scope, Value};

use crate::page::PageDom;
use crate::window::WindowHost;

/// Kept on the realm's `navigator` (a hidden property), itself kept per realm.
const KEY: &str = "lpObservable";

/// Adds the lazy interface names to lp-browser's `Window` class.
pub(crate) fn define_globals<W>(def: &mut ClassDef<W>)
where
    W: JsClass,
    W::Data: WindowHost,
{
    def.accessor(
        "Observable",
        |s, w| global(s, w.page(), "Observable"),
        |s, w, value| replace(s, w.page(), "Observable", value),
    );
    def.accessor(
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
    desc.set("value", value)?;
    desc.set("writable", s.boolean(true))?;
    desc.set("enumerable", s.boolean(false))?;
    desc.set("configurable", s.boolean(true))?;
    define.call(None, &[s.global().value(), s.string(name), desc.value()])?;
    Ok(())
}

fn unavailable() -> JsError {
    JsError::Type("Observable is unavailable".into())
}

/// This realm's module, made on first use.
pub(crate) fn module<'s>(s: &Scope<'s>, page: &Rc<PageDom>) -> JsResult<Object<'s>> {
    let navigator = crate::window::navigator(s, page)?.as_object().ok_or_else(unavailable)?;
    if let Some(module) = s.get_hidden(navigator, KEY)?.and_then(|v| v.as_object()) {
        return Ok(module);
    }
    let module = s.eval(include_str!("observable.js"), Some("lightningpanda://internal/observable.js"), 1)?;
    s.set_hidden(navigator, KEY, module)?;
    module.as_object().ok_or_else(unavailable)
}

/// `EventTarget.when(type, options)`.
pub(crate) fn when<'s>(s: &Scope<'s>, target: Value<'s>, ty: Value<'s>, options: Value<'s>) -> JsResult<Value<'s>> {
    let page = crate::target::page_of(s)?;
    let module = module(s, &page)?;
    module.get("when")?.as_object().ok_or_else(unavailable)?.call(None, &[target, ty, options])
}
