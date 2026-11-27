//! Media Capture and Streams (`MediaStream`, `MediaStreamTrack`, `ImageCapture`, ...) and the capture
//! sources behind them, made in JS (media_stream.js) on a realm's first use. `canvas.captureStream()`
//! and `HTMLMediaElement.captureStream()` call the module's helpers (`html.rs`, `media.rs`);
//! `navigator.mediaDevices` (media_devices.js) reaches `getUserMedia` and the device list through
//! [`stream_helper`]. The fake devices are the embedder's explicit option (`media_devices.rs`).

use pyro_js::{ClassDef, JsClass, JsResult, Object, Scope, Value};

use crate::lazy_module::{lazy_globals, LazyModule};
use crate::page::PageDom;
use crate::window::WindowHost;
use std::rc::Rc;

pub(crate) static MODULE: LazyModule = LazyModule {
    key: "lpMediaStream",
    source: include_str!("media_stream.js"),
    url: "pyro://internal/media_stream.js",
    natives: Some(natives),
};

pub(crate) fn define_globals<W: JsClass>(def: &mut ClassDef<W>)
where
    W::Data: WindowHost,
{
    lazy_globals!(
        def,
        MODULE,
        "MediaStream",
        "MediaStreamTrack",
        "MediaStreamTrackEvent",
        "CanvasCaptureMediaStreamTrack",
        "OverconstrainedError",
        "MediaStreamTrackVideoStats",
        "MediaStreamTrackAudioStats",
        "ImageCapture",
    );
}

fn natives<'s>(s: &Scope<'s>) -> JsResult<Object<'s>> {
    let native = s.object();
    let active = s.function("active", |s, _, _| Ok(s.boolean(crate::target::page_of(s)?.is_fully_active())))?;
    native.set_atom("active", active.value())?;
    let fake = s.function("fakeDevices", |s, _, _| Ok(s.boolean(crate::media_devices::fake_devices())))?;
    native.set_atom("fakeDevices", fake.value())?;
    // policy(feature): whether the document's permissions policy lets it use `feature` (`camera`,
    // `microphone`); a refusal is also reported (`crate::permissions_policy::check_feature`).
    let policy = s.function("policy", |s, _, args| {
        let page = crate::target::page_of(s)?;
        Ok(s.boolean(crate::permissions_policy::check_feature(s, &page, &args.get(0).to_string()?)))
    })?;
    native.set_atom("policy", policy.value())?;
    let kinds = s.function("elementKinds", |s, _, a| crate::media::capture_kinds(s, a.get(0)))?;
    native.set_atom("elementKinds", kinds.value())?;
    Ok(native)
}

/// Calls helper `name` of this realm's media stream module.
pub(crate) fn stream_helper<'s>(s: &Scope<'s>, page: &Rc<PageDom>, name: &str, args: &[Value<'s>]) -> JsResult<Value<'s>> {
    MODULE.helper(s, page, name, args)
}
