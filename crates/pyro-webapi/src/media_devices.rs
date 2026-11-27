//! `navigator.mediaDevices` (`MediaDevices`, Media Capture and Streams): made in JS
//! (media_devices.js). pyro has no capture device, so `enumerateDevices()` lists none and
//! `getUserMedia()` rejects with NotFoundError, as Chrome does on a machine without devices, unless
//! the embedder asked for the fake devices (`pyro serve --use-fake-device-for-media-stream`, Chrome's
//! switch of the same name): then a synthetic camera and microphone exist (media_stream.js) whose frames
//! and samples are computed, never a real capture. `[SecureContext]` (`crate::secure_context`).

use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

use pyro_js::{JsError, JsResult, Object, Scope, Value};

use crate::lazy_module::LazyModule;
use crate::page::PageDom;

pub(crate) static MODULE: LazyModule = LazyModule {
    key: "lpMediaDevices",
    source: include_str!("media_devices.js"),
    url: "pyro://internal/media_devices.js",
    natives: Some(natives),
};

static FAKE_DEVICES: AtomicBool = AtomicBool::new(false);

/// Embedder option: offer the fake camera and microphone (default off). Set before any page exists.
pub fn set_fake_devices(on: bool) {
    FAKE_DEVICES.store(on, Ordering::Relaxed);
}

pub(crate) fn fake_devices() -> bool {
    FAKE_DEVICES.load(Ordering::Relaxed)
}

fn natives<'s>(s: &Scope<'s>) -> JsResult<Object<'s>> {
    let native = s.object();
    let active = s.function("active", |s, _, _| Ok(s.boolean(crate::target::page_of(s)?.is_fully_active())))?;
    native.set_atom("active", active.value())?;
    // The media stream module's helpers (getUserMedia, the device list, ...), called as `stream(name, ...args)`.
    let stream = s.function("stream", |s, _, a| {
        let page = crate::target::page_of(s)?;
        let name = a.get(0).to_string()?;
        let rest: Vec<Value<'_>> = (1..a.len()).map(|i| a.get(i)).collect();
        crate::media_stream::stream_helper(s, &page, &name, &rest)
    })?;
    native.set_atom("stream", stream.value())?;
    Ok(native)
}

/// `MediaDevices` on a window global (absent outside a secure context).
pub(crate) fn define_globals<W>(def: &mut pyro_js::ClassDef<W>)
where
    W: pyro_js::JsClass,
    W::Data: crate::window::WindowHost,
{
    // `MediaDeviceInfo` and `InputDeviceInfo` are made by media_stream.js (they share its device model).
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
    def.global_value_accessor(
        "MediaDeviceInfo",
        |s, w| device_info_global(s, w, "MediaDeviceInfo"),
        |s, w, value| crate::lazy_module::replace(s, crate::window::WindowHost::page(w), "MediaDeviceInfo", value),
    );
    def.global_value_accessor(
        "InputDeviceInfo",
        |s, w| device_info_global(s, w, "InputDeviceInfo"),
        |s, w, value| crate::lazy_module::replace(s, crate::window::WindowHost::page(w), "InputDeviceInfo", value),
    );
}

/// `MediaDeviceInfo` or `InputDeviceInfo` of media_stream.js (`[SecureContext]`).
fn device_info_global<'s, D: crate::window::WindowHost>(s: &Scope<'s>, w: &D, name: &str) -> JsResult<Value<'s>> {
    let page = crate::window::WindowHost::page(w);
    if !crate::secure_context::secure(s, page) {
        return Ok(s.undefined());
    }
    let value = crate::media_stream::MODULE.module(s, page)?.get(name)?;
    crate::lazy_module::replace(s, page, name, value)?;
    Ok(value)
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
