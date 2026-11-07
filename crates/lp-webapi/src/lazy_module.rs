//! Interfaces made in JS on a realm's first use (the pattern of streams.rs
//! and observable.rs, shared by the canvas and media helpers): the window
//! gets one accessor pair per interface name, the first read of a name makes
//! the realm's module and replaces the accessor with a plain data property,
//! and native code reaches the module's helpers through [`LazyModule::helper`].
use std::rc::Rc;

use lp_js::{JsError, JsResult, Object, Scope, Value};

use crate::page::PageDom;

/// One JS module: a factory script returning an object whose properties are
/// the interface objects and the helpers native code calls.
pub(crate) struct LazyModule {
    /// Hidden key on the realm's `navigator`, itself kept per realm.
    pub key: &'static str,
    pub source: &'static str,
    pub url: &'static str,
    /// What the factory gets as its argument: the engine's helpers it cannot
    /// reach from JS.
    pub natives: Option<for<'s> fn(&Scope<'s>) -> JsResult<Object<'s>>>,
}

/// Adds lazy interface names to lp-browser's `Window` class: one accessor
/// pair each, served by the `static` [`LazyModule`] `$module`.
macro_rules! lazy_globals {
    ($def:ident, $module:path, $($name:literal),+ $(,)?) => {
        $($def.global_value_accessor(
            $name,
            |s, w| $module.global(s, crate::window::WindowHost::page(w), $name),
            |s, w, value| crate::lazy_module::replace(s, crate::window::WindowHost::page(w), $name, value),
        );)+
    };
}
pub(crate) use lazy_globals;

impl LazyModule {
    pub(crate) fn global<'s>(&self, s: &Scope<'s>, page: &Rc<PageDom>, name: &str) -> JsResult<Value<'s>> {
        let value = self.module(s, page)?.get(name)?;
        replace(s, page, name, value)?;
        Ok(value)
    }

    /// This realm's module, made on first use.
    pub(crate) fn module<'s>(&self, s: &Scope<'s>, page: &Rc<PageDom>) -> JsResult<Object<'s>> {
        let navigator = crate::worker_scope::module_holder(s, page)?;
        if let Some(module) = s.get_hidden(navigator, self.key)?.and_then(|v| v.as_object()) {
            return Ok(module);
        }
        let factory = s.eval(self.source, Some(self.url), 1)?;
        let natives = match self.natives {
            Some(make) => make(s)?.value(),
            None => s.undefined(),
        };
        let module = factory.as_object().ok_or_else(unavailable)?.call(None, &[natives])?;
        s.set_hidden(navigator, self.key, module)?;
        module.as_object().ok_or_else(unavailable)
    }

    /// This realm's module if it was made.
    pub(crate) fn existing<'s>(&self, s: &Scope<'s>, page: &Rc<PageDom>) -> JsResult<Option<Object<'s>>> {
        let navigator = crate::worker_scope::module_holder(s, page)?;
        Ok(s.get_hidden(navigator, self.key)?.and_then(|v| v.as_object()))
    }

    /// Calls module helper `name`.
    pub(crate) fn helper<'s>(
        &self,
        s: &Scope<'s>,
        page: &Rc<PageDom>,
        name: &str,
        args: &[Value<'s>],
    ) -> JsResult<Value<'s>> {
        let module = self.module(s, page)?;
        module.get(name)?.as_object().ok_or_else(unavailable)?.call(None, args)
    }
}

/// Shadows the accessor with the interface object's usual data property; also
/// what assigning the name does, so page polyfills keep working.
pub(crate) fn replace(s: &Scope<'_>, page: &Rc<PageDom>, name: &str, value: Value<'_>) -> JsResult<()> {
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
    JsError::Type("interface is unavailable".into())
}
