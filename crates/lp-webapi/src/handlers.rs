//! GlobalEventHandlers / WindowEventHandlers / DocumentAndElementEventHandlers
//! IDL attributes (`onclick`, `oninput`, ...) on HTML/SVG elements, Document
//! and Window, plus `on*` content attributes set through `setAttribute`.
//!
//! IDL handlers reuse the ordinary listener slot of
//! `target::set_event_handler`, so they run in registration order with other
//! listeners. A handler returning `false` cancels the event (except
//! `mouseover`/`error` per HTML "process the event handler return value").
//!
//! Content attributes: `setAttribute('onx', body)` compiles `body` as
//! `function (event) { body }` in the calling realm and installs it as the
//! IDL handler; `removeAttribute` clears it. Gaps (documented): inline
//! handler attributes created by the HTML parser or `innerHTML` are not
//! compiled; the special `window.onerror` 5-argument signature and
//! `onbeforeunload` return semantics are not implemented; compiled handlers
//! do not get the element/form/document scope chain.

use lp_js::{ClassDef, JsClass, JsResult, Scope, Value};

use crate::node::NodeData;
use crate::target::{event_handler, set_event_handler, EventTargetData};
use crate::window::WindowHost;

/// Gives access to the event target of a binding's payload.
pub(crate) trait HasTarget {
    fn event_target_data(&self) -> &EventTargetData;
}
impl HasTarget for NodeData {
    fn event_target_data(&self) -> &EventTargetData {
        &self.target
    }
}

// Each event family is written once. The `*_events!` macros pass the list to
// a callback macro, which either builds the `&[&str]` used for content
// attributes or registers the IDL accessors, so both stay in sync.

/// GlobalEventHandlers (HTML §8.1.8.2) without touch events (desktop UA).
macro_rules! global_events {
    ($callback:ident!($($args:tt)*)) => {
        $callback!($($args)* [
            "abort", "auxclick", "beforeinput", "beforetoggle", "blur", "cancel", "canplay", "canplaythrough",
            "change", "click", "close", "contextlost", "contextmenu", "contextrestored", "copy", "cuechange", "cut",
            "dblclick", "drag", "dragend", "dragenter", "dragleave", "dragover", "dragstart", "drop",
            "durationchange", "emptied", "ended", "error", "focus", "formdata", "input", "invalid", "keydown",
            "keypress", "keyup", "load", "loadeddata", "loadedmetadata", "loadstart", "mousedown", "mouseenter",
            "mouseleave", "mousemove", "mouseout", "mouseover", "mouseup", "paste", "pause", "play", "playing",
            "progress", "ratechange", "reset", "resize", "scroll", "scrollend", "securitypolicyviolation", "seeked",
            "seeking", "select", "selectionchange", "selectstart", "slotchange", "stalled", "submit", "suspend",
            "timeupdate", "toggle", "volumechange", "waiting", "wheel", "pointerdown", "pointerup", "pointermove",
            "pointerover", "pointerout", "pointerenter", "pointerleave", "pointercancel", "gotpointercapture",
            "lostpointercapture", "animationstart", "animationend", "animationiteration", "animationcancel",
            "transitionstart", "transitionend", "transitionrun", "transitioncancel"
        ])
    };
}

/// WindowEventHandlers (HTML §8.1.8.2).
macro_rules! window_events {
    ($callback:ident!($($args:tt)*)) => {
        $callback!($($args)* [
            "afterprint", "beforeprint", "beforeunload", "hashchange", "languagechange", "message", "messageerror",
            "offline", "online", "pagehide", "pagereveal", "pageshow", "pageswap", "popstate", "rejectionhandled",
            "storage", "unhandledrejection", "unload"
        ])
    };
}

/// Document-only handlers.
macro_rules! document_events {
    ($callback:ident!($($args:tt)*)) => {
        $callback!($($args)* ["readystatechange", "visibilitychange"])
    };
}

macro_rules! event_list {
    ([$($ty:literal),* $(,)?]) => {
        &[$($ty),*]
    };
}

macro_rules! handler_accessors {
    ($def:expr, $get:expr, [$($ty:literal),* $(,)?]) => {
        $(
            $def.accessor(
                concat!("on", $ty),
                |s, d| event_handler(s, $get(d), $ty),
                |s, d, v| set_event_handler(s, $get(d), $ty, v),
            );
        )*
    };
}

pub(crate) const GLOBAL: &[&str] = global_events!(event_list!());
pub(crate) const WINDOW: &[&str] = window_events!(event_list!());
pub(crate) const DOCUMENT: &[&str] = document_events!(event_list!());

/// Whether `on<name>` is a known event handler content attribute.
pub(crate) fn is_handler_attribute(local: &str) -> Option<&'static str> {
    let ty = local.strip_prefix("on")?;
    GLOBAL.iter().chain(WINDOW).chain(DOCUMENT).find(|t| t.eq_ignore_ascii_case(ty)).copied()
}

/// Elements and Document: GlobalEventHandlers.
pub(crate) fn define_global<C>(def: &mut ClassDef<C>)
where
    C: JsClass,
    C::Data: HasTarget,
{
    global_events!(handler_accessors!(def, <C::Data as HasTarget>::event_target_data,));
}

/// Document-only handlers (plus GlobalEventHandlers via `define_global`).
pub(crate) fn define_document<C>(def: &mut ClassDef<C>)
where
    C: JsClass,
    C::Data: HasTarget,
{
    document_events!(handler_accessors!(def, <C::Data as HasTarget>::event_target_data,));
}

fn window_target<D: WindowHost>(d: &D) -> &EventTargetData {
    d.event_target()
}

/// Window: GlobalEventHandlers + WindowEventHandlers.
pub(crate) fn define_window<W>(def: &mut ClassDef<W>)
where
    W: JsClass,
    W::Data: WindowHost,
{
    global_events!(handler_accessors!(def, window_target::<W::Data>,));
    window_events!(handler_accessors!(def, window_target::<W::Data>,));
}

/// `setAttribute('on<type>', body)` on an element: compiles and installs the
/// handler; `None` (removeAttribute) clears it.
pub(crate) fn content_attribute_changed(
    s: &Scope<'_>,
    target: &EventTargetData,
    ty: &'static str,
    body: Option<&str>,
) -> JsResult<()> {
    let handler: Value<'_> = match body {
        Some(body) => {
            let ctor = s.global().get("Function")?;
            let f = ctor.as_object().expect("Function constructor").construct(&[s.string("event"), s.string(body)])?;
            f.value()
        }
        None => s.null(),
    };
    set_event_handler(s, target, ty, handler)
}
