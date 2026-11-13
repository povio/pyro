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
//! `function (event) { body }` in the calling realm, with the document, form
//! owner and element on its scope chain, and installs it as the IDL handler;
//! `removeAttribute` clears it. Attributes from the HTML parser, `innerHTML`
//! and cloning are compiled in the main world before the next event dispatch,
//! listener registration or handler access (`drain_inline`), so pages without
//! them pay one emptiness check. Gap (documented): `onbeforeunload` return
//! semantics are not implemented.

use pyro_js::{ClassDef, JsClass, JsResult, Scope, Value};

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
            "change", "click", "close", "command", "contextlost", "contextmenu", "contextrestored", "copy", "cuechange", "cut",
            "dblclick", "drag", "dragend", "dragenter", "dragleave", "dragover", "dragstart", "drop",
            "durationchange", "emptied", "ended", "error", "focus", "formdata", "input", "invalid", "keydown",
            "keypress", "keyup", "load", "loadeddata", "loadedmetadata", "loadstart", "mousedown", "mouseenter",
            "mouseleave", "mousemove", "mouseout", "mouseover", "mouseup", "paste", "pause", "play", "playing",
            "progress", "ratechange", "reset", "resize", "scroll", "scrollend", "securitypolicyviolation", "seeked",
            "seeking", "select", "selectionchange", "selectstart", "slotchange", "stalled", "submit", "suspend",
            "timeupdate", "toggle", "volumechange", "waiting", "wheel", "pointerdown", "pointerup", "pointermove",
            "pointerover", "pointerout", "pointerenter", "pointerleave", "pointercancel", "gotpointercapture",
            "lostpointercapture", "animationstart", "animationend", "animationiteration", "animationcancel",
            "transitionstart", "transitionend", "transitionrun", "transitioncancel", "webkitanimationstart",
            "webkitanimationend", "webkitanimationiteration", "webkittransitionend"
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

/// The event type of an event handler content attribute named `local`
/// (`onclick`), compared ASCII case-insensitively.
pub(crate) fn is_handler_attribute(local: &str) -> Option<&'static str> {
    if local.len() < 3 || !local.as_bytes()[..2].eq_ignore_ascii_case(b"on") {
        return None;
    }
    // The first two bytes are ASCII: a char boundary follows them.
    let ty = &local[2..];
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
    if target.page.scripting_disabled() {
        return set_event_handler(s, target, ty, s.null());
    }
    let handler: Value<'_> = match body {
        Some(body) => {
            let ctor = s.global().get_atom("Function")?;
            // HTML "getting the current value of the event handler": the
            // document, form owner and element are on the scope chain.
            let scoped =
                format!("with(this.ownerDocument||document){{with(this.form||{{}}){{with(this){{{body}\n}}}}}}");
            let f =
                ctor.as_object().expect("Function constructor").construct(&[s.string("event"), s.string(&scoped)])?;
            f.value()
        }
        None => s.null(),
    };
    set_event_handler(s, target, ty, handler)
}

/// Whether `ty` on `<body>`/`<frameset>` targets the Window (HTML "Window-
/// reflecting body element event handler set").
pub(crate) fn reflects_to_window(ty: &str) -> bool {
    matches!(ty, "blur" | "error" | "focus" | "load" | "resize" | "scroll") || WINDOW.contains(&ty)
}

/// Compiles `on*` attributes of elements the parser created or cloning made,
/// in the main world. Elements outside the page's document (templates,
/// DOMParser documents) are skipped: they have no scripting.
pub(crate) fn drain_inline(page: &crate::page::PageDom) {
    if !page.dom.borrow().has_pending_inline_handlers() {
        return;
    }
    let Some(realm) = page.main_world_realm() else { return };
    let Ok(s) = realm.scope() else { return };
    let pending = page.dom.borrow_mut().take_pending_inline_handlers();
    for id in pending {
        let attrs: Vec<(String, String)> = {
            let dom = page.dom.borrow();
            if dom.node_document(id).ok() != Some(page.document()) {
                continue;
            }
            let Ok(attrs) = dom.attributes(id) else { continue };
            attrs
                .iter()
                .filter(|a| a.name.local.starts_with("on") && a.name.ns == markup5ever::ns!())
                .map(|a| (a.name.local.to_string(), a.value.to_string()))
                .collect()
        };
        let Ok(wrapper) = crate::node::wrap_node(&s, page, id) else { continue };
        let Some(node) = s.unwrap::<crate::node::Node>(wrapper.value()) else { continue };
        for (name, value) in attrs {
            if let Err(e) = crate::node::handler_attribute_changed(&s, &node, &name, Some(&value)) {
                s.report_exception(&e);
            }
        }
    }
}

/// `ontouch*` on the Window of a page with touch emulation, the way scripts
/// detect a touch device (`'ontouchstart' in window`). Desktop pages keep
/// Chrome's desktop shape, without them.
pub(crate) fn define_touch_handlers(s: &Scope<'_>) -> JsResult<()> {
    fn window_target(s: &Scope<'_>) -> JsResult<crate::target::EventTargetData> {
        let page = crate::target::page_of(s)?;
        let key = page.window_target().ok_or_else(|| pyro_js::JsError::Type("Illegal invocation".into()))?;
        Ok(crate::target::EventTargetData::window(&page, key))
    }
    let define = s.global().get_atom("Object")?.as_object().map(|o| o.get_atom("defineProperty")).transpose()?;
    let define =
        define.and_then(|d| d.as_object()).ok_or_else(|| pyro_js::JsError::Type("Object.defineProperty".into()))?;
    for ty in ["touchstart", "touchend", "touchmove", "touchcancel"] {
        let desc = s.object();
        let get = s.function("get", move |s, _, _| event_handler(s, &window_target(s)?, ty))?;
        let set = s.function("set", move |s, _, a| {
            set_event_handler(s, &window_target(s)?, ty, a.get(0))?;
            Ok(s.undefined())
        })?;
        desc.set_atom("get", get.value())?;
        desc.set_atom("set", set.value())?;
        desc.set_atom("enumerable", s.boolean(true))?;
        desc.set_atom("configurable", s.boolean(true))?;
        define.call(None, &[s.global().value(), s.string(&format!("on{ty}")), desc.value()])?;
    }
    Ok(())
}
