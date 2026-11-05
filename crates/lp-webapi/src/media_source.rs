//! Minimal Media Source Extensions surface. There is no media pipeline, so a
//! `MediaSource` never opens: `readyState` stays "closed", `addSourceBuffer`
//! and `endOfStream` throw as they do on a closed source, and
//! `isTypeSupported` is false for every type. It exists so feature detection
//! and `URL.createObjectURL(mediaSource)` behave: the object URL is a
//! media-source entry, which fetch and XHR treat as a network error (Fetch
//! "scheme fetch" for `blob:` when the entry's object is not a Blob).

use lp_js::{ClassDef, JsClass, JsError, JsResult, Scope};

use crate::page::PageDom;
use crate::target::{page_of, EventTarget, EventTargetData};

pub struct MediaSourceData {
    pub target: EventTargetData,
}

pub struct MediaSource;

impl JsClass for MediaSource {
    const NAME: &'static str = "MediaSource";
    type Data = MediaSourceData;
    type Parent = EventTarget;

    fn define(def: &mut ClassDef<Self>) {
        def.upcast(|d: &MediaSourceData| &d.target);
        def.constructor(0, |s, nt, _a| {
            let page = page_of(s)?;
            let target = EventTargetData::new(&page);
            let lp_dom::events::TargetKey::Other(k) = target.key else { unreachable!() };
            let key = page.other_identity(k);
            s.wrap_new::<MediaSource>(MediaSourceData { target }, nt, Some(key))
        });
        def.getter("readyState", |s, _d| Ok(s.string("closed")));
        def.getter("duration", |s, _d| Ok(s.number(f64::NAN)));
        def.getter("sourceBuffers", |s, _d| Ok(s.array(&[])?.value()));
        def.getter("activeSourceBuffers", |s, _d| Ok(s.array(&[])?.value()));
        def.method("addSourceBuffer", 1, |_s, _d, a| {
            a.require(1, "MediaSource.addSourceBuffer")?;
            Err(JsError::dom("NotSupportedError", "MediaSource has no media pipeline: no type is supported."))
        });
        def.method("removeSourceBuffer", 1, |_s, _d, a| {
            a.require(1, "MediaSource.removeSourceBuffer")?;
            Err(JsError::dom("NotFoundError", "The SourceBuffer does not belong to this MediaSource."))
        });
        def.method("endOfStream", 0, |_s, _d, _a| {
            Err(JsError::dom("InvalidStateError", "The MediaSource is not open."))
        });
        def.method("setLiveSeekableRange", 2, |_s, _d, _a| {
            Err(JsError::dom("InvalidStateError", "The MediaSource is not open."))
        });
        def.method("clearLiveSeekableRange", 0, |_s, _d, _a| {
            Err(JsError::dom("InvalidStateError", "The MediaSource is not open."))
        });
        def.static_method("isTypeSupported", 1, |s, a| {
            a.require(1, "MediaSource.isTypeSupported")?;
            Ok(s.boolean(false))
        });
        def.accessor(
            "onsourceopen",
            |s, d| crate::target::event_handler(s, &d.target, "sourceopen"),
            |s, d, v| crate::target::set_event_handler(s, &d.target, "sourceopen", v),
        );
        def.accessor(
            "onsourceended",
            |s, d| crate::target::event_handler(s, &d.target, "sourceended"),
            |s, d, v| crate::target::set_event_handler(s, &d.target, "sourceended", v),
        );
        def.accessor(
            "onsourceclose",
            |s, d| crate::target::event_handler(s, &d.target, "sourceclose"),
            |s, d, v| crate::target::set_event_handler(s, &d.target, "sourceclose", v),
        );
    }
}

/// `URL.createObjectURL(mediaSource)`: a blob URL registered as a media-source
/// entry, which no fetch resolves.
pub(crate) fn object_url(s: &Scope<'_>, page: &PageDom) -> JsResult<String> {
    let origin = crate::storage::tuple_origin(s, page).unwrap_or_else(|| "null".into());
    Ok(format!("blob:{origin}/{}", crate::crypto::random_uuid()?))
}

pub(crate) fn register(engine: &lp_js::Engine) {
    engine.register::<MediaSource>();
}
