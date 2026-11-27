//! Media Capabilities (`navigator.mediaCapabilities`, `MediaCapabilities`) for windows and workers:
//! made in JS (media_capabilities.js). The answers come from the codec tables this build really
//! has: `canPlayType`'s (`media_format::can_play_type`) and MediaSource's (`MediaType::parse`).

use std::rc::Rc;

use pyro_js::{ClassDef, JsClass, JsError, JsResult, Object, Scope, Value};

use crate::lazy_module::{lazy_globals, LazyModule};
use crate::page::PageDom;
use crate::window::WindowHost;

static MODULE: LazyModule = LazyModule {
    key: "lpMediaCapabilities",
    source: include_str!("media_capabilities.js"),
    url: "pyro://internal/media_capabilities.js",
    natives: Some(natives),
};

fn natives<'s>(s: &Scope<'s>) -> JsResult<Object<'s>> {
    let native = s.object();
    let can_play = s.function("canPlay", |s, _, a| {
        Ok(s.string(crate::media_format::can_play_type(&a.get(0).to_string()?).as_str()))
    })?;
    native.set_atom("canPlay", can_play.value())?;
    let mse = s.function("mseSupported", |s, _, a| {
        Ok(s.boolean(pyro_media::mse_buffer::MediaType::parse(&a.get(0).to_string()?).is_some()))
    })?;
    native.set_atom("mseSupported", mse.value())?;
    Ok(native)
}

pub(crate) fn define_globals<W: JsClass>(def: &mut ClassDef<W>)
where
    W::Data: WindowHost,
{
    lazy_globals!(def, MODULE, "MediaCapabilities");
}

/// `navigator.mediaCapabilities` of a window or worker realm ([SameObject]).
pub(crate) fn media_capabilities<'s>(s: &Scope<'s>, page: &Rc<PageDom>) -> JsResult<Value<'s>> {
    if page.is_disposed() {
        return Err(JsError::ContextDestroyed);
    }
    MODULE.module(s, page)?.get("mediaCapabilities")
}
