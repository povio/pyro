//! GlobalEventHandlers / WindowEventHandlers / DocumentAndElementEventHandlers
//! IDL attributes (`onclick`, `oninput`, ...) on HTML, SVG and MathML elements, Document
//! and Window, plus `on*` content attributes set through `setAttribute`.
//!
//! IDL handlers reuse the ordinary listener slot of
//! `target::set_event_handler`, so they run in registration order with other
//! listeners. A handler returning `false` cancels the event, except the
//! Window's `onerror` for an ErrorEvent, which cancels by returning `true`
//! (HTML "process the event handler return value").
//!
//! Content attributes: `setAttribute('onx', body)` installs `body` as the
//! raw uncompiled handler, compiled as `function (event) { body }` (with the
//! document, form owner and element on its scope chain) when the handler is
//! first read or called; one that does not compile is reported then and
//! leaves the handler null in its listener position. `removeAttribute`
//! clears it. Attributes from the HTML parser, `innerHTML`
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
            "abort", "auxclick", "beforeinput", "beforematch", "beforetoggle", "blur", "cancel", "canplay", "canplaythrough",
            "change", "click", "close", "command", "contextlost", "contextmenu", "contextrestored", "copy", "cuechange", "cut",
            "dblclick", "drag", "dragend", "dragenter", "dragleave", "dragover", "dragstart", "drop",
            "durationchange", "emptied", "ended", "error", "focus", "formdata", "input", "installresult", "invalid", "keydown",
            "keypress", "keyup", "load", "loadeddata", "loadedmetadata", "loadstart", "mousedown", "mouseenter",
            "mouseleave", "mousemove", "mouseout", "mouseover", "mouseup", "paste", "pause", "play", "playing",
            "progress", "ratechange", "reset", "resize", "scroll", "scrollend", "scrollsnapchange", "scrollsnapchanging",
            "securitypolicyviolation", "seeked",
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

/// Fullscreen API handlers, on Document and Element.
macro_rules! fullscreen_events {
    ($callback:ident!($($args:tt)*)) => {
        $callback!($($args)* ["fullscreenchange", "fullscreenerror"])
    };
}

pub(crate) const GLOBAL: &[&str] = global_events!(event_list!());
pub(crate) const FULLSCREEN: &[&str] = fullscreen_events!(event_list!());
pub(crate) const WINDOW: &[&str] = window_events!(event_list!());
pub(crate) const DOCUMENT: &[&str] = document_events!(event_list!());
/// The permission elements' own handlers (WICG PEPC and install element, tentative); which
/// element has which is [`element_handler`].
pub(crate) const PERMISSION_ELEMENT: &[&str] =
    &["stream", "track", "location", "promptaction", "promptdismiss", "validationstatuschange"];
/// HTML-in-Canvas: `HTMLCanvasElement.onpaint` (see [`element_handler`]).
pub(crate) const CANVAS_ELEMENT: &[&str] = &["paint"];

/// Whether the HTML element named `local` has its own event handler for `ty` beyond
/// GlobalEventHandlers: the permission elements' (`HTMLUserMediaElement.onstream`, ...).
pub(crate) fn element_handler(local: &str, ty: &str) -> bool {
    match local {
        "usermedia" => ty == "stream",
        "camera" | "microphone" => ty == "track",
        "geolocation" => matches!(ty, "location" | "promptaction" | "promptdismiss" | "validationstatuschange"),
        "install" => ty == "validationstatuschange",
        "canvas" => ty == "paint",
        _ => false,
    }
}

/// The event type of an event handler content attribute named `local`
/// (`onclick`), compared ASCII case-insensitively.
pub(crate) fn is_handler_attribute(local: &str) -> Option<&'static str> {
    if local.len() < 3 || !local.as_bytes()[..2].eq_ignore_ascii_case(b"on") {
        return None;
    }
    // The first two bytes are ASCII: a char boundary follows them.
    let ty = &local[2..];
    GLOBAL
        .iter()
        .chain(WINDOW)
        .chain(DOCUMENT)
        .chain(FULLSCREEN)
        .chain(PERMISSION_ELEMENT)
        .chain(CANVAS_ELEMENT)
        .find(|t| t.eq_ignore_ascii_case(ty))
        .copied()
}

/// Elements and Document: GlobalEventHandlers.
pub(crate) fn define_global<C>(def: &mut ClassDef<C>)
where
    C: JsClass,
    C::Data: HasTarget,
{
    global_events!(handler_accessors!(def, <C::Data as HasTarget>::event_target_data,));
}

/// Fullscreen API: `onfullscreenchange` and `onfullscreenerror` (Document and Element).
pub(crate) fn define_fullscreen<C>(def: &mut ClassDef<C>)
where
    C: JsClass,
    C::Data: HasTarget,
{
    fullscreen_events!(handler_accessors!(def, <C::Data as HasTarget>::event_target_data,));
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

/// The Window's handlers for a `<body>`/`<frameset>` of the page's document;
/// `None` for one in a document without a browsing context.
fn body_window(n: &crate::node::NodeData) -> Option<EventTargetData> {
    let page = n.page();
    if page.dom.borrow().node_document(n.id).ok() != Some(page.document()) {
        return None;
    }
    page.window_target().map(|w| EventTargetData::window(page, w))
}

macro_rules! window_reflecting_accessors {
    ($def:expr, [$($ty:literal),* $(,)?]) => {
        $(
            $def.accessor(
                concat!("on", $ty),
                |s, n| match body_window(n) {
                    Some(target) => event_handler(s, &target, $ty),
                    None => Ok(s.null()),
                },
                |s, n, v| match body_window(n) {
                    Some(target) => set_event_handler(s, &target, $ty, v),
                    None => Ok(()),
                },
            );
        )*
    };
}

/// `<body>` and `<frameset>`: WindowEventHandlers and the Window-reflecting
/// body element event handler set (blur, error, focus, load, resize,
/// scroll) shadow the element's own handlers and forward to the Window; in a
/// document without one they are null and setting them does nothing.
pub(crate) fn define_window_reflecting<C>(def: &mut ClassDef<C>)
where
    C: JsClass<Data = crate::node::NodeData>,
{
    window_reflecting_accessors!(def, ["blur", "error", "focus", "load", "resize", "scroll"]);
    window_events!(window_reflecting_accessors!(def,));
}

/// `setAttribute('on<type>', body)` on an element: installs the handler as its raw body, compiled
/// when first read or called (`target::resolve_raw_handler`); `None` (removeAttribute) clears it.
pub(crate) fn content_attribute_changed(
    s: &Scope<'_>,
    target: &EventTargetData,
    ty: &'static str,
    body: Option<&str>,
) -> JsResult<()> {
    match body {
        Some(body) if !target.page.scripting_disabled() => crate::target::set_raw_event_handler(s, target, ty, body),
        _ => set_event_handler(s, target, ty, s.null()),
    }
}

/// HTML "getting the current value of the event handler": compiles a content attribute's handler
/// body as a function in `s`'s realm. An element's handler has the element, its form owner and its
/// document on its scope chain (innermost first); a Window's (`<body onerror>` reflected to the
/// Window) has none of them. Its source text is `function on<type>(<params>) {\n<body>\n}`, which
/// `Function.prototype.toString` gives back (WPT event-handler-sourcetext,
/// compile-event-handler-lexical-scopes{,-form-owner}).
pub(crate) fn compile_handler<'s>(
    s: &Scope<'s>,
    page: &crate::page::PageDom,
    key: pyro_dom::events::TargetKey,
    ty: &str,
    body: &str,
) -> JsResult<Value<'s>> {
    let ctor = s.global().get_atom("Function")?;
    let ctor = ctor.as_object().expect("Function constructor");
    let window = page.is_window_key(key);
    // The element (none for the Window), whether it is SVG, a `<body>`/`<frameset>`, and the ids
    // of its form owner (form-associated elements only) and document.
    let (element, svg, body_or_frameset, form, document) = {
        let dom = page.dom.borrow();
        let element = if window { None } else { dom.key_node(key).filter(|&n| dom.element(n).is_ok()) };
        let e = element.and_then(|n| dom.element(n).ok());
        let svg = e.is_some_and(|e| &*e.name.ns == "http://www.w3.org/2000/svg");
        let body_or_frameset = e.is_some_and(|e| e.is_html() && matches!(&*e.name.local, "body" | "frameset"));
        let form_associated = e.is_some_and(|e| {
            e.is_html()
                && matches!(
                    &*e.name.local,
                    "button" | "fieldset" | "input" | "object" | "output" | "select" | "textarea" | "img"
                )
        }) || element.is_some_and(|n| dom.is_form_associated(n));
        let form = element.filter(|_| form_associated).and_then(|n| dom.form_owner(n).ok().flatten());
        let document = element.and_then(|n| dom.node_document(n).ok());
        (element, svg, body_or_frameset, form, document)
    };
    // OnErrorEventHandler (the Window's, and `<body>`/`<frameset>`'s onerror) takes the five
    // ErrorEvent arguments; an SVG element's handler names its argument `evt` (SVG 2, as browsers
    // do).
    let params: &[&str] = if ty == "error" && (window || body_or_frameset) {
        &["event", "source", "lineno", "colno", "error"]
    } else if svg {
        &["evt"]
    } else {
        &["event"]
    };
    let mut args: Vec<Value<'_>> = params.iter().map(|p| s.string(p)).collect();
    args.push(s.string(body));
    // The body alone must parse as a function body (a body that closes the function early does
    // not); it passed the Trusted Types attribute check already.
    s.compile_exempt(|| ctor.construct(&args))?;
    let source = format!("function on{ty}({}) {{\n{body}\n}}", params.join(", "));
    let Some(element) = element else {
        let outer = s.compile_exempt(|| ctor.construct(&[s.string(&format!("return {source}"))]))?;
        return outer.call(None, &[]);
    };
    // The scopes are the outer function's: `with` on its arguments (an empty object for no form
    // owner), so the handler's own parameters and locals come first.
    let outer = s.compile_exempt(|| {
        ctor.construct(&[s.string(&format!(
            "with(arguments[0]){{with(arguments[1]){{with(arguments[2]){{return {source}}}}}}}"
        ))])
    })?;
    let wrap = |id: Option<pyro_dom::NodeId>| -> JsResult<Value<'s>> {
        match id {
            Some(id) => Ok(crate::node::wrap_node(s, page, id)?.value()),
            None => Ok(s.object().value()),
        }
    };
    let element = wrap(Some(element))?;
    outer.call(None, &[wrap(document)?, wrap(form)?, element])
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
