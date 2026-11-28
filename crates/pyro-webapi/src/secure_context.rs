//! `[SecureContext]` members added in JS modules (Web Locks, MediaDevices, Worklet): whether a window is a
//! secure context, and their removal from one that is not, so that `'locks' in navigator` and the
//! like are false there rather than present and undefined.

use pyro_js::{JsResult, Scope};

use crate::page::PageDom;

/// Interface objects on the global that exist only in a secure context.
const GLOBALS: &[&str] = &[
    "LockManager",
    "SubtleCrypto",
    "CryptoKey",
    "Lock",
    "MediaDevices", "MediaDeviceInfo", "InputDeviceInfo",
    "MediaKeySystemAccess", "MediaKeys", "MediaKeySession", "MediaKeyStatusMap", "MediaKeyMessageEvent",
    "Credential",
    "CredentialsContainer",
    "PasswordCredential",
    "FederatedCredential",
    "PublicKeyCredential",
    "AuthenticatorResponse",
    "AuthenticatorAttestationResponse",
    "AuthenticatorAssertionResponse",
    "Worklet",
    "AudioWorklet",
    "ImageDecoder",
    "ImageTrackList",
    "ImageTrack",
    "AudioDecoder",
    "AudioEncoder",
    "VideoDecoder",
    "VideoEncoder",
    "cookieStore",
    "StorageBucketManager",
    "StorageBucket",
    "AudioPlaybackStats",
    "FetchLaterResult",
    "ML",
    "MLContext",
    "MLGraph",
    "MLGraphBuilder",
    "MLOperand",
    "MLTensor",
    "PressureObserver",
    "PressureRecord",
    "Keyboard",
    "Sensor", "SensorErrorEvent", "Accelerometer", "LinearAccelerationSensor", "GravitySensor", "Gyroscope", "AmbientLightSensor", "OrientationSensor", "AbsoluteOrientationSensor", "RelativeOrientationSensor", "DeviceMotionEvent", "DeviceOrientationEvent", "DeviceMotionEventAcceleration", "DeviceMotionEventRotationRate",
    // `[SecureContext] readonly attribute CacheStorage caches`: absent rather than an accessor
    // reading undefined, so a page's own `var caches = ...` (fetch/api/request/request-init-001)
    // gets a plain global variable, as in Chrome.
    "caches",
];
/// `Navigator` members that exist only in a secure context.
const NAVIGATOR: &[&str] = &["serviceWorker", "locks", "storageBuckets", "mediaDevices", "credentials", "ml", "registerProtocolHandler", "unregisterProtocolHandler", "requestMediaKeySystemAccess", "keyboard"];

/// A secure context (HTML): the document and every ancestor document are potentially trustworthy,
/// as the embedder reports it (`PageHooks::is_secure_context`); without one, the realm's origin or the
/// document's URL decides (a sandboxed https document, whose origin is opaque, is still secure).
pub(crate) fn secure(s: &Scope<'_>, page: &PageDom) -> bool {
    if let Some(secure) = page.hooks_secure_context() {
        return secure;
    }
    let trustworthy = |url: &str| pyro_net::Url::parse(url).is_ok_and(|u| pyro_net::is_potentially_trustworthy_url(&u));
    s.realm().origin().is_some_and(|o| trustworthy(&o))
        || page.dom.borrow().document_data(page.document()).ok().is_some_and(|d| trustworthy(&d.url.to_string()))
}

/// [`secure`] for code that has no realm at hand (the input layer): the embedder's answer, else
/// whether the document's URL is potentially trustworthy.
pub(crate) fn secure_page(page: &PageDom) -> bool {
    if let Some(secure) = page.hooks_secure_context() {
        return secure;
    }
    page.dom.borrow().document_data(page.document()).ok().is_some_and(|d| {
        pyro_net::Url::parse(&d.url.to_string()).is_ok_and(|u| pyro_net::is_potentially_trustworthy_url(&u))
    })
}

/// Removes the `[SecureContext]` members above from a window that is not a secure context. Run
/// when the realm is set up.
pub(crate) fn hide_if_insecure(s: &Scope<'_>, page: &PageDom) -> JsResult<()> {
    if secure(s, page) {
        return Ok(());
    }
    crate::storage_manager::hide_file_system_globals(s)?;
    let global = s.global();
    for name in GLOBALS {
        global.delete(name)?;
    }
    // `caches` is a Window attribute: also off the interface prototype, wherever it is installed. So are the
    // `[SecureContext]` device event handlers (`ondevicemotion`, ...) and `onpointerrawupdate`: the Window's
    // handlers are on the global object itself (WPT pointerevents/pointerevent_pointerrawupdate.html).
    for name in ["ondevicemotion", "ondeviceorientation", "ondeviceorientationabsolute", "onpointerrawupdate"] {
        global.delete(name)?;
    }
    if let Some(window) = global.get_atom("Window")?.as_object() {
        if let Some(proto) = window.get_atom("prototype")?.as_object() {
            proto.delete("caches")?;
            for name in ["ondevicemotion", "ondeviceorientation", "ondeviceorientationabsolute"] {
                proto.delete(name)?;
            }
        }
    }
    // `PointerEvent.getCoalescedEvents()` and `onpointerrawupdate` are `[SecureContext]` too.
    for (interface, members) in [
        ("PointerEvent", &["getCoalescedEvents"][..]),
        ("HTMLMediaElement", &["mediaKeys", "setMediaKeys"][..]),
        ("Window", &["onpointerrawupdate"][..]),
        ("Document", &["onpointerrawupdate"][..]),
        ("HTMLElement", &["onpointerrawupdate"][..]),
        ("SVGElement", &["onpointerrawupdate"][..]),
        ("MathMLElement", &["onpointerrawupdate"][..]),
    ] {
        if let Some(proto) = global.get_atom(interface)?.as_object().map(|o| o.get_atom("prototype")).transpose()?.and_then(|p| p.as_object()) {
            for name in members {
                proto.delete(name)?;
            }
        }
    }
    if let Some(navigator) = global.get_atom("Navigator")?.as_object() {
        if let Some(proto) = navigator.get_atom("prototype")?.as_object() {
            for name in NAVIGATOR {
                proto.delete(name)?;
            }
        }
    }
    Ok(())
}
