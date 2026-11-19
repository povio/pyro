//! `navigator.mediaDevices` (`MediaDevices`, Media Capture and Streams): made in JS
//! (media_devices.js). pyro has no capture device, so `enumerateDevices()` lists none and
//! `getUserMedia()` rejects with NotFoundError. `[SecureContext]` (`crate::secure_context`).

use std::rc::Rc;

use pyro_js::{JsError, JsResult, Object, Scope, Value};

use crate::lazy_module::LazyModule;
use crate::page::PageDom;

pub(crate) static MODULE: LazyModule = LazyModule {
    key: "lpMediaDevices",
    source: include_str!("media_devices.js"),
    url: "pyro://internal/media_devices.js",
    natives: Some(natives),
};

fn natives<'s>(s: &Scope<'s>) -> JsResult<Object<'s>> {
    let native = s.object();
    let active = s.function("active", |s, _, _| Ok(s.boolean(crate::target::page_of(s)?.is_fully_active())))?;
    native.set_atom("active", active.value())?;
    Ok(native)
}

/// `MediaDevices` on a window global (absent outside a secure context).
pub(crate) fn define_globals<W>(def: &mut pyro_js::ClassDef<W>)
where
    W: pyro_js::JsClass,
    W::Data: crate::window::WindowHost,
{
    def.global_value_accessor(
        "MediaDevices",
        |s, w| {
            let page = crate::window::WindowHost::page(w);
            if !crate::secure_context::secure(s, page) {
                return Ok(s.undefined());
            }
            MODULE.global(s, page, "MediaDevices")
        },
        |s, w, value| crate::lazy_module::replace(s, crate::window::WindowHost::page(w), "MediaDevices", value),
    );
}

/// `navigator.mediaDevices`: the realm's `MediaDevices` in a secure context.
pub(crate) fn media_devices<'s>(s: &Scope<'s>, page: &Rc<PageDom>) -> JsResult<Value<'s>> {
    if page.is_disposed() {
        return Err(JsError::ContextDestroyed);
    }
    if !crate::secure_context::secure(s, page) {
        return Ok(s.undefined());
    }
    MODULE.module(s, page)?.get("mediaDevices")
}
