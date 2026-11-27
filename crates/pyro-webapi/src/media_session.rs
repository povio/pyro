//! Media Session (`navigator.mediaSession`, `MediaSession`, `MediaMetadata`, `ChapterInformation`):
//! made in JS (media_session.js). A headless browser shows no media notification and receives no
//! media key, so the session is state: metadata, playback state, position state, action handlers.

use std::rc::Rc;

use pyro_js::{ClassDef, JsClass, JsError, JsResult, Value};

use crate::lazy_module::{lazy_globals, LazyModule};
use crate::page::PageDom;
use crate::window::WindowHost;

static MODULE: LazyModule = LazyModule {
    key: "lpMediaSession",
    source: include_str!("media_session.js"),
    url: "pyro://internal/media_session.js",
    natives: None,
};

pub(crate) fn define_globals<W: JsClass>(def: &mut ClassDef<W>)
where
    W::Data: WindowHost,
{
    lazy_globals!(def, MODULE, "MediaSession", "MediaMetadata", "ChapterInformation");
}

/// `navigator.mediaSession`: the realm's session ([SameObject]).
pub(crate) fn media_session<'s>(s: &pyro_js::Scope<'s>, page: &Rc<PageDom>) -> JsResult<Value<'s>> {
    if page.is_disposed() {
        return Err(JsError::ContextDestroyed);
    }
    MODULE.module(s, page)?.get("mediaSession")
}
