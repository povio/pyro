//! Window members owned by pyro-webapi (pyro-browser owns the Window class,
//! location/history and timers) plus Navigator and Screen.

use std::rc::Rc;

use pyro_js::{ClassDef, JsClass, JsError, JsResult, NoParent, Scope, Value};

use crate::page::PageDom;
use crate::target::EventTargetData;
use crate::util::*;

/// Implemented by pyro-browser's Window data.
pub trait WindowHost {
    fn page(&self) -> &Rc<PageDom>;
    fn event_target(&self) -> &EventTargetData;
}

/// Named access on the Window object: the names of the document's tree, behind
/// the global's own properties.
pub(crate) struct WindowNamedProperties(pub(crate) Rc<PageDom>);

impl pyro_js::NamedProperties for WindowNamedProperties {
    fn may_resolve(&self, name: pyro_js::NameUnits<'_>) -> bool {
        let page = &self.0;
        let hash = match name {
            pyro_js::NameUnits::Latin1(n) => pyro_dom::window_name_hash(n.iter().map(|&u| u16::from(u))),
            pyro_js::NameUnits::Utf16(n) => pyro_dom::window_name_hash(n.iter().copied()),
        };
        page.dom.try_borrow().is_ok_and(|dom| dom.window_name_may_exist(page.document(), hash))
    }

    fn resolve<'s>(&self, s: &Scope<'s>, name: &str) -> JsResult<Option<Value<'s>>> {
        let page = &self.0;
        let doc = page.document();
        // Navigable containers named by `name` (their child navigable's target name) stand for
        // their child window, which comes first: iframe, frame, and an object or embed showing a
        // document. An object or embed without one is a named element itself; an iframe or frame
        // without one names nothing.
        let named = page.dom.borrow().window_named_elements(doc, name).js()?;
        let kinds: Vec<Option<bool>> = {
            let dom = page.dom.borrow();
            named
                .iter()
                .map(|&el| {
                    dom.element(el).ok().and_then(|e| {
                        let local = &*e.name.local;
                        let container = e.is_html()
                            && matches!(local, "iframe" | "frame" | "object" | "embed")
                            && e.attr("name") == Some(name);
                        // Some(keeps itself without a window)
                        container.then(|| matches!(local, "object" | "embed"))
                    })
                })
                .collect()
        };
        for (&el, kind) in named.iter().zip(&kinds) {
            if kind.is_some() {
                if let Some(window) = page.frame_window(s, el)? {
                    // COOP access reporting: another window read a named property of this one.
                    crate::popup::note_window_access(page, "named");
                    return Ok(Some(window));
                }
            }
        }
        let found: Vec<_> =
            named.iter().zip(&kinds).filter(|(_, kind)| kind.unwrap_or(true)).map(|(&el, _)| el).collect();
        if !found.is_empty() {
            crate::popup::note_window_access(page, "named");
        }
        match found[..] {
            [] => Ok(None),
            [el] => Ok(Some(crate::node::wrap_node(s, page, el)?.value())),
            _ => {
                let kind = crate::collections::CollectionKind::WindowNamed(name.to_owned());
                Ok(Some(crate::collections::collection(s, page, doc, kind)?.value()))
            }
        }
    }
}

/// `Window.scroll()`, `scrollTo()` (`operation`) and `scrollBy()` (`by`): returns a scroll
/// promise (Chrome's `Promise<ScrollResult>`), settled when a smooth scroll ends.
fn window_scroll<'s>(
    s: &pyro_js::Scope<'s>,
    page: &Rc<PageDom>,
    a: &pyro_js::Args<'_>,
    operation: &str,
    by: bool,
) -> JsResult<pyro_js::Value<'s>> {
    let options = crate::layout_api::scroll_options(a, operation, "Window", to_scroll_offset)?;
    crate::layout_api::window_scroll(s, page, options, by)
}

/// Setter of a [Replaceable] attribute of a window or worker global:
/// assignment shadows it with an own data property.
pub fn replace_window_attribute<'s>(
    s: &pyro_js::Scope<'s>,
    page: &crate::page::PageDom,
    name: &str,
    value: pyro_js::Value<'s>,
) -> pyro_js::JsResult<()> {
    let define = page.define_properties.borrow().get(&s.realm_id()).map(|p| p.get(s));
    let define = define
        .and_then(|v| v.as_object())
        .ok_or_else(|| pyro_js::JsError::Type(format!("window.{name} cannot be replaced in this realm").into()))?;
    let desc = s.object();
    desc.set_atom("value", value)?;
    for k in ["writable", "enumerable", "configurable"] {
        desc.set(k, s.boolean(true))?;
    }
    define.call(None, &[s.global().value(), s.string(name), desc.value()])?;
    Ok(())
}

/// A lazily made interface object settling on the window: an own data
/// property, writable and configurable but not enumerable, as Web IDL
/// interface objects are (the getter of a `global_value_accessor` and its
/// assignment both end here).
pub fn replace_window_interface<'s>(
    s: &pyro_js::Scope<'s>,
    page: &crate::page::PageDom,
    name: &str,
    value: pyro_js::Value<'s>,
) -> pyro_js::JsResult<()> {
    let define = page.define_properties.borrow().get(&s.realm_id()).map(|p| p.get(s));
    let define = define
        .and_then(|v| v.as_object())
        .ok_or_else(|| pyro_js::JsError::Type(format!("window.{name} cannot be replaced in this realm").into()))?;
    let desc = s.object();
    desc.set_atom("value", value)?;
    desc.set_atom("writable", s.boolean(true))?;
    desc.set_atom("enumerable", s.boolean(false))?;
    desc.set_atom("configurable", s.boolean(true))?;
    define.call(None, &[s.global().value(), s.string(name), desc.value()])?;
    Ok(())
}

/// Makes `name` the global's own interface-object property (writable,
/// configurable, not enumerable) with `value`: what a lazily made
/// interface's accessor turns into on its first read or an assignment.
pub fn define_global_interface<'s>(
    s: &pyro_js::Scope<'s>,
    page: &crate::page::PageDom,
    name: &str,
    value: pyro_js::Value<'s>,
) -> pyro_js::JsResult<()> {
    let define = page.define_properties.borrow().get(&s.realm_id()).map(|p| p.get(s));
    let define = define
        .and_then(|v| v.as_object())
        .ok_or_else(|| pyro_js::JsError::Type(format!("{name} is unavailable in this realm").into()))?;
    let desc = s.object();
    desc.set_atom("value", value)?;
    desc.set_atom("writable", s.boolean(true))?;
    desc.set_atom("enumerable", s.boolean(false))?;
    desc.set_atom("configurable", s.boolean(true))?;
    define.call(None, &[s.global().value(), s.string(name), desc.value()])?;
    Ok(())
}

/// Whether [`crate::install`] makes `document` and `window` the global's own
/// constant attributes (patched JavaScriptCore). A Window class then leaves
/// them out of its prototype, as WebIDL does for [LegacyUnforgeable] members.
pub fn has_window_constants() -> bool {
    pyro_js::Capabilities::detect().global_constants
}

/// Adds pyro-webapi's Window members to pyro-browser's `Window` class.
pub fn define_members<W>(def: &mut ClassDef<W>)
where
    W: JsClass,
    W::Data: WindowHost,
{
    crate::handlers::define_window(def);
    macro_rules! bar_prop {
        ($property:literal, $slot:literal) => {
            def.accessor($property, |s, w| bar_prop(s, w.page(), $slot),
                |s, w, value| replace_window_attribute(s, w.page(), $property, value));
        };
    }
    bar_prop!("locationbar", 5);
    bar_prop!("menubar", 6);
    bar_prop!("personalbar", 7);
    bar_prop!("scrollbars", 8);
    bar_prop!("statusbar", 9);
    bar_prop!("toolbar", 10);
    // `install` makes `document` the global's own constant attribute where
    // the JavaScriptCore can (`has_window_constants`). Otherwise (the system
    // framework) this getter serves it: the lean dispatch, which finds the
    // window's node wrapper in the document's slot.
    if !has_window_constants() {
        def.pure_getter_typed("document", pyro_js::GetterResult::ObjectOrNull, |s, w| {
            Ok(crate::node::wrap_node(s, w.page(), w.page().document())?.value())
        });
    }
    // `window.event` (DOM "current event"), [Replaceable]: assignment shadows
    // it with an own data property.
    def.accessor(
        "event",
        |s, _w| crate::target::current_event(s),
        |s, w, value| replace_window_attribute(s, w.page(), "event", value),
    );
    def.global_getter("indexedDB", |s, w| crate::indexeddb::window_factory(s, w.page()));
    def.global_getter("cookieStore", |s, w| crate::cookie_store::member(s, w.page(), "cookieStore"));
    def.getter("caches", |s, w| crate::cache_storage::caches(s, w.page()));
    def.getter("localStorage", |s, w| crate::storage::window_storage(s, w.page(), false));
    def.getter("sessionStorage", |s, w| crate::storage::window_storage(s, w.page(), true));
    def.getter("customElements", |s, w| crate::custom::registry(s, w.page()));
    def.getter("navigator", |s, w| navigator(s, w.page()));
    def.getter("crypto", |s, _w| crate::crypto::crypto_value(s));
    // [Replaceable]: assignment shadows it with an own data property.
    def.accessor(
        "clientInformation",
        |s, w| navigator(s, w.page()),
        |s, w, value| replace_window_attribute(s, w.page(), "clientInformation", value),
    );
    def.getter("ClipboardItem", |s, w| crate::navigator_extras::member(s, w.page(), "ClipboardItem"));
    def.getter("Notification", |s, w| crate::navigator_extras::member(s, w.page(), "Notification"));
    def.getter("Keyboard", |s, w| crate::navigator_extras::member(s, w.page(), "Keyboard"));
    def.getter("Clipboard", |s, w| crate::navigator_extras::member(s, w.page(), "Clipboard"));
    def.getter("Permissions", |s, w| crate::navigator_extras::member(s, w.page(), "Permissions"));
    def.getter("PermissionStatus", |s, w| crate::navigator_extras::member(s, w.page(), "PermissionStatus"));
    crate::streams::define_globals(def);
    crate::scheduler::define_global(def);
    crate::observable::define_globals(def);
    crate::lazy_module::lazy_globals!(def, crate::view_transition::MODULE, "ViewTransition", "ViewTransitionTypeSet");
    crate::crypto::define_globals(def);
    crate::storage_manager::define_globals(def);
    crate::storage_manager::define_file_system_globals(def);
    crate::service_worker::define_globals(def);
    crate::cache_storage::define_globals(def);
    crate::event_source::define_globals(def);
    crate::websocket::define_globals(def);
    crate::indexeddb::define_globals(def);
    crate::editing_globals::define_globals(def);
    crate::font_loading::define_globals(def);
    crate::web_locks::define_globals(def);
    crate::media_devices::define_globals(def);
    crate::credentials::define_globals(def);
    crate::sensors::define_globals(def);
    crate::paint_worklet::define_globals(def);
    crate::audio_worklet::define_globals(def);
    crate::lazy_module::lazy_globals!(def, crate::pseudo_element::MODULE, "CSSPseudoElement");
    crate::typed_om::define_globals(def);
    crate::trusted_types::define_globals(def);
    crate::trusted_types::define_parser_options_global(def);
    // `Highlight` and `HighlightRegistry` (CSS Custom Highlight API), made on first use.
    def.global_value_accessor(
        "Highlight",
        |s, w| crate::highlights::global(s, w.page(), "Highlight"),
        |s, w, value| crate::highlights::replace_global(s, w.page(), "Highlight", value),
    );
    def.global_value_accessor(
        "HighlightRegistry",
        |s, w| crate::highlights::global(s, w.page(), "HighlightRegistry"),
        |s, w, value| crate::highlights::replace_global(s, w.page(), "HighlightRegistry", value),
    );
    // The `CSS` namespace object, made on its first read (most realms never
    // read it); assignment shadows it with a data property.
    def.accessor(
        "CSS",
        |s, w| {
            let value = css_namespace(s)?;
            crate::typed_om::add_factories(s, w.page(), value)?;
            css_namespace_property(s, w.page(), value)?;
            Ok(value)
        },
        |s, w, value| css_namespace_property(s, w.page(), value),
    );
    crate::animations::define_globals(def);
    crate::reporting::define(def);
    crate::audio_decoder::define(def);
    crate::sanitizer::define(def);
    crate::pressure::define(def);
    crate::audio_encoder::define(def);
    crate::video_encoder::define(def);
    crate::image_decoder::define(def);
    crate::canvas::define_globals(def);
    crate::media::define_globals(def);
    crate::media_source::define_globals(def);
    crate::webaudio::define_globals(def);
    crate::webnn::define_globals(def);
    def.getter("screen", |s, w| {
        let key = pyro_js::IdentityKey(w.page().id() | (1 << 57), 1);
        same_object(s, w.page(), key, || s.wrap::<Screen>(w.page().clone(), Some(key)))
    });
    def.getter("visualViewport", |s, w| {
        let key = viewport_identity(w.page(), ViewportTarget::VisualViewport);
        same_object(s, w.page(), key, || {
            s.wrap::<VisualViewport>(ViewportTargetData::new(w.page(), ViewportTarget::VisualViewport), Some(key))
        })
    });
    def.method("getComputedStyle", 1, |s, _, a| {
        let element = s.unwrap::<crate::node::Element>(a.get(0))
            .ok_or_else(|| JsError::Type("Window.getComputedStyle requires an Element".into()))?;
        Ok(crate::layout_api::get_computed_style(s, element.page(), element.id, a.get(1))?.value())
    });
    def.method("matchMedia", 1, |s, w, a| {
        let q = arg_string(&a, 0, "Window.matchMedia")?;
        Ok(crate::layout_api::match_media(s, w.page(), q)?.value())
    });
    def.method("getSelection", 0, |s, w, _a| crate::range::get_selection(s, w.page()));
    // pyro: the legacy `find(string, caseSensitive, backwards, wrapAround)` (not in a standard):
    // selects the next match within one text node after the selection (before it, backwards)
    // and answers whether there was one.
    def.method("find", 0, |s, w, a| {
        let needle = if a.len() == 0 { String::new() } else { crate::util::dom_string(a.get(0))? };
        let flag = |i: usize| a.len() > i && a.get(i).to_bool();
        Ok(s.boolean(find_text(w.page(), &needle, flag(1), flag(2), flag(3))))
    });
    def.method("open", 0, |s, w, a| window_open(s, w.page(), &a));
    def.getter("innerWidth", |s, w| Ok(s.number(window_size(w.page()).0 as f64)));
    def.getter("innerHeight", |s, w| Ok(s.number(window_size(w.page()).1 as f64)));
    // The device: the window itself under mobile emulation, as in Chrome.
    def.getter("outerWidth", |s, w| Ok(s.number(w.page().layout().device_viewport().width as f64)));
    def.getter("outerHeight", |s, w| Ok(s.number(w.page().layout().device_viewport().height as f64)));
    def.getter("devicePixelRatio", |s, w| Ok(s.number(w.page().layout().device_pixel_ratio())));
    def.getter("scrollX", |s, w| Ok(s.number(w.page().layout.borrow().scroll_x() as f64)));
    def.getter("scrollY", |s, w| Ok(s.number(w.page().layout.borrow().scroll_y() as f64)));
    def.getter("pageXOffset", |s, w| Ok(s.number(w.page().layout.borrow().scroll_x() as f64)));
    def.getter("pageYOffset", |s, w| Ok(s.number(w.page().layout.borrow().scroll_y() as f64)));
    def.getter("screenX", |s, w| Ok(s.number(f64::from(w.page().screen_position().0))));
    def.getter("screenY", |s, w| Ok(s.number(f64::from(w.page().screen_position().1))));
    def.getter("screenLeft", |s, w| Ok(s.number(f64::from(w.page().screen_position().0))));
    def.getter("screenTop", |s, w| Ok(s.number(f64::from(w.page().screen_position().1))));
    // The scroll methods return promises (Chrome's `Promise<ScrollResult>`): a bad argument
    // rejects.
    def.promise_method("scroll", 0, |s, w, a| window_scroll(s, w.page(), &a, "scroll", false));
    def.promise_method("scrollTo", 0, |s, w, a| window_scroll(s, w.page(), &a, "scrollTo", false));
    def.promise_method("scrollBy", 0, |s, w, a| window_scroll(s, w.page(), &a, "scrollBy", true));
    def.method("atob", 1, |s, _w, a| Ok(s.string(&crate::encoding::atob(&arg_string(&a, 0, "Window.atob")?)?)));
    def.method("btoa", 1, |s, _w, a| Ok(s.string(&crate::encoding::btoa(&arg_string(&a, 0, "Window.btoa")?)?)));
    def.getter("frameElement", |s, _w| Ok(s.null()));
    def.getter("opener", |s, _w| Ok(s.null()));
    def.getter("closed", |s, _w| Ok(s.boolean(false)));
    // `status`: the empty string until a script sets it (no status bar shows it); a set value
    // reads back, as an own data property of the window.
    def.accessor(
        "status",
        |s, _w| Ok(s.string("")),
        |s, w, v| {
            let v = s.string(&crate::util::dom_string(v)?);
            // The own property goes on the calling realm's global: only a window's own script
            // can store one (another window's assignment is dropped).
            if !crate::target::page_of(s).is_ok_and(|p| Rc::ptr_eq(&p, w.page())) {
                return Ok(());
            }
            replace_window_attribute(s, w.page(), "status", v)
        },
    );
    def.getter("length", |s, _w| Ok(s.number(0.0)));
    // [Replaceable] (WindowOrWorkerGlobalScope): assignment shadows it with
    // an own data property; the document's origin is left as it is.
    def.accessor(
        "origin",
        |s, w| {
            let url = w.page().dom.borrow().document_data(w.page().document()).map(|d| d.url.to_string()).unwrap_or_default();
            Ok(s.string(
                &pyro_net::Url::parse(&url).map(|u| u.origin().ascii_serialization()).unwrap_or_else(|_| "null".into()),
            ))
        },
        |s, w, value| replace_window_attribute(s, w.page(), "origin", value),
    );
    // The CrossOriginProperties methods tell COOP access reporting when another window calls them
    // (`note_window_access`: one thread-local read unless a COOP reporting endpoint exists).
    def.method("focus", 0, |s, w, _a| {
        crate::popup::note_window_access(w.page(), "focus");
        crate::input::focus_window(w.page());
        Ok(s.undefined())
    });
    def.method("blur", 0, |s, w, _a| {
        crate::popup::note_window_access(w.page(), "blur");
        Ok(s.undefined())
    });
    // CSSOM View: a popup's size and position follow the script; any other window ignores them.
    // (The window is as large as its viewport here: no chrome around it.)
    def.method("resizeTo", 2, |s, w, a| window_resize(s, w.page(), &a, "resizeTo", false));
    def.method("resizeBy", 2, |s, w, a| window_resize(s, w.page(), &a, "resizeBy", true));
    def.method("moveTo", 2, |s, w, a| window_move(s, w.page(), &a, "moveTo", false));
    def.method("moveBy", 2, |s, w, a| window_move(s, w.page(), &a, "moveBy", true));
    def.method("print", 0, |s, _w, _a| Ok(s.undefined()));
    // pyro: css-sizing-4 responsive iframes: a responsively embedded document (one that opted in with
    // `<meta name="responsive-embedded-sizing">`) in a child frame asks its iframe to take its
    // content's size again. That size is read whenever the embedder reads its layout or renders
    // (`PageDom::sync_responsive_frames`), so nothing is queued; anything else throws (WPT css-sizing
    // `responsive-iframe/responsive-iframe-request-resize-error`).
    def.method("requestResize", 0, |s, w, _a| {
        let page = w.page();
        let embedded = page.is_nested_frame() && page.dom.borrow().is_responsively_embedded(page.document());
        if !embedded {
            return Err(JsError::dom(
                "NotAllowedError",
                "Failed to execute 'requestResize' on 'Window': The document is not responsively embedded.",
            ));
        }
        Ok(s.undefined())
    });
    // The embedder may block until the dialog is answered (CDP with a
    // session that has Page enabled); otherwise each returns what a
    // dismissed one gives (Chrome's results when Playwright auto-dismisses)
    // and is reported (CDP Page.javascriptDialogOpening).
    def.method("alert", 0, |s, w, a| {
        dialog(w.page(), "alert", a.get(0), None)?;
        Ok(s.undefined())
    });
    def.method("confirm", 0, |s, w, a| {
        let answer = dialog(w.page(), "confirm", a.get(0), None)?;
        Ok(s.boolean(answer.is_some_and(|a| a.accept)))
    });
    def.method("prompt", 0, |s, w, a| {
        let default = a.get(1);
        let default = if default.is_undefined() { String::new() } else { default.to_string()? };
        match dialog(w.page(), "prompt", a.get(0), Some(default))? {
            Some(answer) if answer.accept => Ok(s.string(&answer.prompt_text)),
            _ => Ok(s.null()),
        }
    });
    // HTML `window.stop()`: "stop loading" the navigable. The embedder cancels its ongoing
    // navigation and aborts the document (no more parsing, no DOMContentLoaded, load or
    // pageshow); an aborted document's readiness is "complete" at once (Chromium, WPT
    // abort-document-load, window-stop-in-image-load-handler).
    def.method("stop", 0, |s, w, _a| {
        let page = w.page().clone();
        if let Some(hooks) = page.hooks() {
            hooks.stop_loading();
            let doc = page.document();
            if page.ready_state(doc) != "complete" {
                crate::lifecycle::set_ready_state(&page, doc, "complete");
            }
        }
        Ok(s.undefined())
    });
    // Closes a script-closable top-level browsing context (a popup); the
    // embedder decides and closes the page later. No-op elsewhere.
    def.method("close", 0, |s, w, _a| {
        crate::popup::note_window_access(w.page(), "close");
        w.page().close_window();
        Ok(s.undefined())
    });
    def.method("postMessage", 1, |s, w, a| post_message_to_self(s, w.page(), &a).map(|()| s.undefined()));
}

/// The two `long` arguments of `resizeTo`/`resizeBy`/`moveTo`/`moveBy` (WebIDL conversion).
fn window_long_pair(a: &pyro_js::Args<'_>, name: &str) -> JsResult<(i32, i32)> {
    if a.len() < 2 {
        return Err(JsError::Type(format!("Failed to execute '{name}' on 'Window': 2 arguments required, but only {} present.", a.len()).into()));
    }
    let long = |v: pyro_js::Value<'_>| -> JsResult<i32> {
        let n = v.to_number()?;
        Ok(if n.is_finite() { n.trunc().clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32 } else { 0 })
    };
    Ok((long(a.get(0))?, long(a.get(1))?))
}

fn window_resize<'s>(s: &Scope<'s>, page: &Rc<PageDom>, a: &pyro_js::Args<'s>, name: &str, by: bool) -> JsResult<Value<'s>> {
    let (x, y) = window_long_pair(a, name)?;
    if !page.is_script_sizable_window() {
        return Ok(s.undefined());
    }
    let device = page.device_viewport();
    let screen = (device.screen_width.unwrap_or(device.width), device.screen_height.unwrap_or(device.height));
    let (width, height) = if by { (i64::from(device.width) + i64::from(x), i64::from(device.height) + i64::from(y)) } else { (i64::from(x), i64::from(y)) };
    // (At least one CSS pixel, at most the screen.)
    let fit = |size: i64, screen: u32| size.clamp(1, i64::from(screen.max(1))) as u32;
    page.resize_window(fit(width, screen.0), fit(height, screen.1));
    Ok(s.undefined())
}

fn window_move<'s>(s: &Scope<'s>, page: &Rc<PageDom>, a: &pyro_js::Args<'s>, name: &str, by: bool) -> JsResult<Value<'s>> {
    let (x, y) = window_long_pair(a, name)?;
    if !page.is_script_sizable_window() {
        return Ok(s.undefined());
    }
    let device = page.device_viewport();
    let (is_popup, left, top) = page.window_chrome();
    let (left, top) = if by { (left.saturating_add(x), top.saturating_add(y)) } else { (x, y) };
    // (Kept on the screen.)
    let room = |screen: Option<u32>, size: u32| screen.unwrap_or(size).saturating_sub(size) as i32;
    page.set_window_chrome(is_popup, left.clamp(0, room(device.screen_width, device.width)), top.clamp(0, room(device.screen_height, device.height)));
    Ok(s.undefined())
}

/// `postMessage` of a window outside a frame tree, which only reaches
/// itself (the embedder's Window class replaces it with a frame-aware one).
pub fn post_message_to_self<'s>(s: &Scope<'s>, page: &Rc<PageDom>, a: &pyro_js::Args<'s>) -> JsResult<()> {
    let key = page.origin_key();
    let (message, target_origin, options) = crate::window_message::post_message_args(s, a, &key)?;
    let realm = page.main_world_realm().unwrap_or_else(|| s.realm());
    let deliver = target_origin.allows(&key);
    crate::window_message::send_window_message(s, message, options, page, &realm, Some(key), None, deliver)
}

/// A Window [SameObject] attribute value for this realm: looked up by
/// `key`, otherwise created by `make` and kept alive until the realm (or
/// page) is disposed, so expandos and `defineProperty` overrides persist.
pub(crate) fn same_object<'s>(
    s: &Scope<'s>,
    page: &PageDom,
    key: pyro_js::IdentityKey,
    make: impl FnOnce() -> JsResult<pyro_js::Object<'s>>,
) -> JsResult<Value<'s>> {
    if let Some(o) = s.lookup_identity(key) {
        return Ok(o.value());
    }
    let object = make()?;
    let realm = s.realm_id();
    // The holder is a private null-prototype object written by index, so no
    // page-visible method or prototype setter can intercept retention.
    let existing = page.same_objects.borrow().get(&realm).map(|(h, n)| (h.get(s), *n));
    let (holder, index) = match existing.and_then(|(h, n)| Some((h.as_object()?, n))) {
        Some(found) => found,
        None => {
            let holder = s.object();
            holder.set_prototype(s.null());
            page.same_objects.borrow_mut().insert(realm, (pyro_js::Persistent::new(s, holder.value()), 0));
            (holder, 0)
        }
    };
    holder.set_index(index, object.value())?;
    if let Some(entry) = page.same_objects.borrow_mut().get_mut(&realm) {
        entry.1 = index + 1;
    }
    Ok(object.value())
}

pub(crate) fn navigator<'s>(s: &Scope<'s>, page: &Rc<PageDom>) -> JsResult<Value<'s>> {
    let key = pyro_js::IdentityKey(page.id() | (1 << 57), 0);
    same_object(s, page, key, || s.wrap::<Navigator>(page.clone(), Some(key)))
}

pub(crate) fn user_agent(page: &PageDom) -> String {
    if let Some(ua) = page.identity.borrow().value.as_ref().map(|v| &v.user_agent).filter(|s| !s.is_empty()) {
        return ua.clone();
    }
    page.net.borrow().as_ref().map(|n| n.user_agent()).unwrap_or_else(|| pyro_net::NetConfig::default().user_agent)
}

/// `navigator.platform`: the override's, else the host's.
pub(crate) fn platform(page: &PageDom) -> String {
    let identity = page.identity.borrow();
    identity
        .value
        .as_ref()
        .and_then(|v| v.platform.clone())
        .unwrap_or_else(|| if cfg!(target_os = "macos") { "MacIntel" } else { "Linux x86_64" }.into())
}

pub struct Navigator;
impl JsClass for Navigator {
    const NAME: &'static str = "Navigator";
    type Data = Rc<PageDom>;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.getter("userAgent", |s, p| Ok(s.string(&user_agent(p))));
        def.getter("appVersion", |s, p| {
            let ua = user_agent(p);
            Ok(s.string(ua.strip_prefix("Mozilla/").unwrap_or(&ua)))
        });
        def.getter("appName", |s, _p| Ok(s.string("Netscape")));
        def.getter("appCodeName", |s, _p| Ok(s.string("Mozilla")));
        def.getter("product", |s, _p| Ok(s.string("Gecko")));
        def.getter("productSub", |s, _p| Ok(s.string("20030107")));
        def.getter("vendor", |s, _p| Ok(s.string("")));
        def.getter("vendorSub", |s, _p| Ok(s.string("")));
        def.getter("platform", |s, p| Ok(s.string(&platform(p))));
        def.getter("language", |s, p| Ok(s.string(&languages(p)[0])));
        def.getter("languages", |s, p| {
            let langs = languages(p);
            let mut vals = pyro_js::RootedVec::new();
            for lang in langs {
                vals.push(s, s.string(&lang));
            }
            Ok(s.array(&vals.iter(s).collect::<Vec<_>>())?.value())
        });
        def.getter("onLine", |s, p| Ok(s.boolean(!p.net.borrow().as_ref().is_some_and(|n| n.settings().offline))));
        def.getter("cookieEnabled", |s, _p| Ok(s.boolean(true)));
        def.getter("hardwareConcurrency", |s, _p| Ok(s.number(4.0)));
        def.getter("maxTouchPoints", |s, p| Ok(s.number(p.max_touch_points() as f64)));
        def.getter("userAgentData", |s, p| crate::navigator_extras::member(s, p, "userAgentData"));
        def.getter("geolocation", |s, p| crate::navigator_extras::member(s, p, "geolocation"));
        def.getter("permissions", |s, p| crate::navigator_extras::member(s, p, "permissions"));
        def.getter("clipboard", |s, p| crate::navigator_extras::member(s, p, "clipboard"));
        def.getter("keyboard", |s, p| crate::navigator_extras::member(s, p, "keyboard"));
        def.getter("storage", |s, p| crate::storage_manager::storage(s, p));
        def.getter("storageBuckets", |s, p| crate::storage_buckets::manager(s, p));
        def.getter("serviceWorker", |s, p| crate::service_worker::container(s, p));
        def.getter("locks", |s, p| crate::web_locks::locks(s, p));
        def.getter("mediaDevices", |s, p| crate::media_devices::media_devices(s, p));
        def.getter("credentials", |s, p| crate::credentials::credentials(s, p));
        def.getter("ml", |s, p| crate::webnn::ml(s, p));
        def.getter("userActivation", |s, p| crate::user_activation::user_activation(s, p));
        def.getter("webdriver", |s, _p| Ok(s.boolean(true)));
        def.getter("doNotTrack", |s, _p| Ok(s.null()));
        def.getter("pdfViewerEnabled", |s, _p| Ok(s.boolean(false)));
        // HTML: without a PDF viewer the plugin and MIME type lists are empty ([SameObject]).
        def.getter("plugins", |s, p| {
            let key = pyro_js::IdentityKey(p.id() | (1 << 57), 12);
            same_object(s, p, key, || s.wrap::<PluginArray>((), Some(key)))
        });
        def.getter("mimeTypes", |s, p| {
            let key = pyro_js::IdentityKey(p.id() | (1 << 57), 13);
            same_object(s, p, key, || s.wrap::<MimeTypeArray>((), Some(key)))
        });
        def.method("javaEnabled", 0, |s, _p, _a| Ok(s.boolean(false)));
        def.method("sendBeacon", 1, |s, p, a| {
            a.require(1, "Navigator.sendBeacon")?;
            let url = crate::util::dom_string(a.get(0))?;
            let url = crate::html::resolve_url(&p.dom.borrow(), p.document(), &url);
            let Some(url) = pyro_net::Url::parse(&url).ok().filter(|u| matches!(u.scheme(), "http" | "https")) else {
                return Err(JsError::Type(format!("Failed to execute 'sendBeacon': invalid URL '{url}'.").into()));
            };
            Ok(s.boolean(crate::fetch::send_beacon(s, p, url, a.get(1))?))
        });
        for name in ["registerProtocolHandler", "unregisterProtocolHandler"] {
            def.method(name, 2, protocol_handler);
        }
        def.method("vibrate", 0, |s, _p, _a| Ok(s.boolean(false)));
    }
}

/// HTML's normalize protocol handler parameters. This headless browser declines
/// registration requests, so a successful registration or removal needs no UI.
fn protocol_handler<'s>(s: &Scope<'s>, page: &Rc<PageDom>, args: pyro_js::Args<'s>) -> JsResult<Value<'s>> {
    args.require(2, "Navigator protocol handler")?;
    let scheme = dom_string(args.get(0))?.to_ascii_lowercase();
    let url = dom_string(args.get(1))?;
    let safelisted = matches!(
        scheme.as_str(),
        "bitcoin"
            | "ftp"
            | "ftps"
            | "geo"
            | "im"
            | "irc"
            | "ircs"
            | "magnet"
            | "mailto"
            | "matrix"
            | "mms"
            | "news"
            | "nntp"
            | "openpgp4fpr"
            | "sftp"
            | "sip"
            | "sms"
            | "smsto"
            | "ssh"
            | "tel"
            | "urn"
            | "webcal"
            | "wtai"
            | "xmpp"
            // Chromium's decentralized schemes (whatwg/html#5482, WPT protocol.tentative).
            | "cabal"
            | "dat"
            | "did"
            | "dweb"
            | "ethereum"
            | "hyper"
            | "ipfs"
            | "ipns"
            | "ssb"
    );
    let custom = scheme
        .strip_prefix("web+")
        .is_some_and(|name| !name.is_empty() && name.bytes().all(|b| b.is_ascii_lowercase()));
    if !safelisted && !custom {
        return Err(JsError::dom("SecurityError", "Unsupported protocol handler scheme."));
    }
    if !url.contains("%s") {
        return Err(JsError::dom("SyntaxError", "A protocol handler URL must contain %s."));
    }
    let resolved = crate::html::resolve_url(&page.dom.borrow(), page.document(), &url);
    let handler =
        pyro_net::Url::parse(&resolved).map_err(|_| JsError::dom("SyntaxError", "Invalid protocol handler URL."))?;
    if !matches!(handler.scheme(), "http" | "https") || handler.origin().ascii_serialization() != page.origin_key() {
        return Err(JsError::dom("SecurityError", "A protocol handler must use a same-origin HTTP(S) URL."));
    }
    Ok(s.undefined())
}

fn dialog(
    page: &PageDom,
    kind: &'static str,
    message: pyro_js::Value<'_>,
    default: Option<String>,
) -> pyro_js::JsResult<Option<crate::page::DialogAnswer>> {
    let message = if message.is_undefined() { String::new() } else { message.to_string()? };
    let Some(hooks) = page.hooks() else { return Ok(None) };
    let url = page.dom.borrow().document_data(page.document()).map(|d| d.url.to_string()).unwrap_or_default();
    Ok(hooks.javascript_dialog(kind, message, default, url))
}

fn languages(p: &PageDom) -> Vec<String> {
    let header = p
        .identity
        .borrow()
        .value
        .as_ref()
        .and_then(|v| v.accept_language.clone())
        .or_else(|| p.net.borrow().as_ref().and_then(|n| n.settings().accept_language))
        .unwrap_or_else(|| pyro_net::NetConfig::default().accept_language);
    let langs: Vec<String> = header
        .split(',')
        .map(|l| l.split(';').next().unwrap_or("").trim().to_owned())
        .filter(|l| !l.is_empty())
        .collect();
    if langs.is_empty() {
        vec!["en-US".into()]
    } else {
        langs
    }
}

pub struct Screen;
impl JsClass for Screen {
    const NAME: &'static str = "Screen";
    type Data = Rc<PageDom>;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.getter("width", |s, p| Ok(s.number(screen(p).0 as f64)));
        def.getter("height", |s, p| Ok(s.number(screen(p).1 as f64)));
        def.getter("availWidth", |s, p| Ok(s.number(screen(p).0 as f64)));
        def.getter("availHeight", |s, p| Ok(s.number(screen(p).1 as f64)));
        def.getter("availLeft", |s, _p| Ok(s.number(0.0)));
        def.getter("availTop", |s, _p| Ok(s.number(0.0)));
        def.getter("colorDepth", |s, _p| Ok(s.number(24.0)));
        def.getter("pixelDepth", |s, _p| Ok(s.number(24.0)));
        def.getter("orientation", |s, p| {
            let key = viewport_identity(p, ViewportTarget::Orientation);
            same_object(s, p, key, || {
                s.wrap::<ScreenOrientation>(ViewportTargetData::new(p, ViewportTarget::Orientation), Some(key))
            })
        });
    }
}

/// The screen's orientation: an emulated one wins (upstream
/// Screen.zig:105-120); otherwise the screen never rotates and follows its
/// dimensions. (type, angle)
pub fn screen_orientation(p: &PageDom) -> (String, f64) {
    match p.viewport().orientation {
        Some(o) => (o.kind.as_str().to_owned(), f64::from(o.angle)),
        None => {
            let (w, h) = screen(p);
            ((if w >= h { "landscape-primary" } else { "portrait-primary" }).to_owned(), 0.0)
        }
    }
}

/// Which viewport-related event target (`PageDom::viewport_targets`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewportTarget {
    VisualViewport,
    Orientation,
}

/// A realm's `visualViewport` or `screen.orientation` target, registered with
/// its page for embedder events while the wrapper lives.
pub struct ViewportTargetData {
    target: EventTargetData,
    kind: ViewportTarget,
}

impl ViewportTargetData {
    fn new(page: &Rc<PageDom>, kind: ViewportTarget) -> ViewportTargetData {
        let target = EventTargetData::new(page);
        page.viewport_targets.borrow_mut().push((kind, target.key));
        ViewportTargetData { target, kind }
    }
    fn page(&self) -> &Rc<PageDom> {
        &self.target.page
    }
}

impl Drop for ViewportTargetData {
    fn drop(&mut self) {
        let key = self.target.key;
        if let Ok(mut targets) = self.target.page.viewport_targets.try_borrow_mut() {
            targets.retain(|&(k, t)| !(k == self.kind && t == key));
        }
    }
}

fn viewport_identity(page: &PageDom, kind: ViewportTarget) -> pyro_js::IdentityKey {
    let slot = match kind {
        ViewportTarget::VisualViewport => 3,
        ViewportTarget::Orientation => 4,
    };
    pyro_js::IdentityKey(page.id() | (1 << 57), slot)
}

/// This realm's wrapper of viewport target `key`, if it is one.
pub(crate) fn viewport_target_object<'s>(
    s: &Scope<'s>,
    page: &PageDom,
    key: pyro_dom::events::TargetKey,
) -> Option<pyro_js::Object<'s>> {
    let kind = page.viewport_targets.borrow().iter().find(|(_, t)| *t == key).map(|(k, _)| *k)?;
    let object = s.lookup_identity(viewport_identity(page, kind))?;
    let owns = match kind {
        ViewportTarget::VisualViewport => s.unwrap::<VisualViewport>(object.value()).map(|d| d.target.key),
        ViewportTarget::Orientation => s.unwrap::<ScreenOrientation>(object.value()).map(|d| d.target.key),
    };
    (owns == Some(key)).then_some(object)
}

/// `screen.orientation` (ScreenOrientation): an EventTarget whose `change`
/// fires when its `type` or `angle` changes (an emulated orientation, or the
/// screen size, which follows the viewport unless emulated; headless
/// Chromium's screen does not follow viewport resizes). `type` and `angle`
/// are live; `lock()` resolves without rotating and `unlock()` is a no-op,
/// as in headless Chromium.
pub struct ScreenOrientation;
impl JsClass for ScreenOrientation {
    const NAME: &'static str = "ScreenOrientation";
    type Data = ViewportTargetData;
    type Parent = crate::target::EventTarget;
    fn define(def: &mut ClassDef<Self>) {
        def.upcast(|d: &ViewportTargetData| &d.target);
        def.getter("type", |s, d| Ok(s.string(&screen_orientation(d.page()).0)));
        def.getter("angle", |s, d| Ok(s.number(screen_orientation(d.page()).1)));
        // Headless Chromium resolves lock() without rotating.
        def.method("lock", 1, |s, _d, _a| {
            let deferred = s.deferred()?;
            deferred.resolve(s, s.undefined())?;
            Ok(deferred.promise.get(s))
        });
        def.method("unlock", 0, |s, _d, _a| Ok(s.undefined()));
        def.accessor(
            "onchange",
            |s, d| crate::target::event_handler(s, &d.target, "change"),
            |s, d, v| crate::target::set_event_handler(s, &d.target, "change", v),
        );
    }
}

/// `window.visualViewport`: there is no pinch zoom, so the visual viewport is
/// the layout viewport (headless Chromium hides scrollbars), or under mobile
/// emulation the device at the page scale (`width`, `height`, `scale`; a
/// page without `<meta name="viewport">` is 980 px at 390/980 on a 390 px
/// phone). react-aria bounds popover max-height by `offsetTop + height`;
/// without this object every popover got `max-height: 0`. An EventTarget:
/// the embedder fires `resize` when the viewport size or scale changes
/// (react-aria tracks the virtual keyboard with it). With no pinch zoom the
/// visual viewport never moves within the layout viewport, so
/// `scroll`/`scrollend` never fire.
pub struct VisualViewport;
impl JsClass for VisualViewport {
    const NAME: &'static str = "VisualViewport";
    type Data = ViewportTargetData;
    type Parent = crate::target::EventTarget;
    fn define(def: &mut ClassDef<Self>) {
        def.upcast(|d: &ViewportTargetData| &d.target);
        def.getter("width", |s, d| Ok(s.number(visual_viewport(d.page()).0)));
        def.getter("height", |s, d| Ok(s.number(visual_viewport(d.page()).1)));
        def.getter("offsetLeft", |s, _d| Ok(s.number(0.0)));
        def.getter("offsetTop", |s, _d| Ok(s.number(0.0)));
        def.getter("pageLeft", |s, d| Ok(s.number(d.page().layout.borrow().scroll_x() as f64)));
        def.getter("pageTop", |s, d| Ok(s.number(d.page().layout.borrow().scroll_y() as f64)));
        def.getter("scale", |s, d| Ok(s.number(visual_viewport(d.page()).2)));
        def.accessor(
            "onresize",
            |s, d| crate::target::event_handler(s, &d.target, "resize"),
            |s, d, v| crate::target::set_event_handler(s, &d.target, "resize", v),
        );
        def.accessor(
            "onscroll",
            |s, d| crate::target::event_handler(s, &d.target, "scroll"),
            |s, d, v| crate::target::set_event_handler(s, &d.target, "scroll", v),
        );
        def.accessor(
            "onscrollend",
            |s, d| crate::target::event_handler(s, &d.target, "scrollend"),
            |s, d, v| crate::target::set_event_handler(s, &d.target, "scrollend", v),
        );
    }
}

fn screen(p: &PageDom) -> (u32, u32) {
    let v = p.device_viewport();
    (v.screen_width.unwrap_or(v.width), v.screen_height.unwrap_or(v.height))
}

/// `innerWidth`/`innerHeight`: the layout viewport, or under mobile
/// emulation the frame, which the document's width can widen (laid out
/// first, as Chrome does).
fn window_size(p: &PageDom) -> (u32, u32) {
    p.sync_mobile_layout();
    p.layout().window_size()
}

/// `visualViewport`'s width, height and scale (see [`VisualViewport`]).
fn visual_viewport(p: &PageDom) -> (f64, f64, f64) {
    p.sync_mobile_layout();
    // The size excludes classic scrollbars: laid out first, noting a change for `resize`.
    p.note_visual_viewport();
    p.layout().visual_viewport()
}

/// Replaces the lazy `CSS` accessor with the namespace's own data property
/// (writable, configurable, not enumerable, as WebIDL defines a namespace).
fn css_namespace_property<'s>(s: &Scope<'s>, page: &crate::page::PageDom, value: Value<'s>) -> JsResult<()> {
    let define = page.define_properties.borrow().get(&s.realm_id()).map(|p| p.get(s));
    let define = define.and_then(|v| v.as_object()).ok_or_else(|| JsError::type_error("CSS is unavailable"))?;
    let desc = s.object();
    desc.set_atom("value", value)?;
    desc.set_atom("writable", s.boolean(true))?;
    desc.set_atom("enumerable", s.boolean(false))?;
    desc.set_atom("configurable", s.boolean(true))?;
    define.call(None, &[s.global().value(), s.string("CSS"), desc.value()])?;
    Ok(())
}

/// `CSS` namespace object (supports/escape/registerProperty).
pub fn css_namespace<'s>(s: &Scope<'s>) -> JsResult<Value<'s>> {
    let o = s.object();
    let supports = s.function("supports", |s, _this, a| {
        if a.len() == 0 {
            return Err(JsError::type_error(
                "Failed to execute 'supports' on 'CSS': 1 argument required, but only 0 present.",
            ));
        }
        if a.len() >= 2 {
            let property = dom_string(a.get(0))?;
            let value = dom_string(a.get(1))?;
            return Ok(s.boolean(pyro_dom::stylo::supports_declaration(&property, &value)));
        }
        let text = dom_string(a.get(0))?;
        Ok(s.boolean(pyro_dom::stylo::supports_condition(&text)))
    })?;
    o.set_atom("supports", supports.value())?;
    let escape = s.function("escape", |s, _this, a| {
        a.require(1, "CSS.escape")?;
        let input = a.get(0).to_js_string()?;
        let units = input.utf16();
        let mut escaped = Vec::with_capacity(units.len());
        for (index, &unit) in units.iter().enumerate() {
            match unit {
                0 => escaped.push(0xfffd),
                1..=31 | 127 => escaped.extend(format!("\\{unit:x} ").encode_utf16()),
                48..=57 if index == 0 || (index == 1 && units[0] == 45) => {
                    escaped.extend(format!("\\{unit:x} ").encode_utf16());
                },
                45 if units.len() == 1 => escaped.extend([92, unit]),
                45 | 48..=57 | 65..=90 | 95 | 97..=122 | 128.. => escaped.push(unit),
                _ => escaped.extend([92, unit]),
            }
        }
        Ok(s.string_utf16(&escaped))
    })?;
    o.set_atom("escape", escape.value())?;
    // escape(CSSOMString ident): length 1.
    let define = s.global().get_atom("Object")?.as_object().expect("Object").get_atom("defineProperty")?;
    let tag = s.global().get_atom("Symbol")?.as_object().expect("Symbol").get_atom("toStringTag")?;
    let tag_descriptor = s.object();
    tag_descriptor.set_atom("value", s.string("CSS"))?;
    tag_descriptor.set_atom("configurable", s.boolean(true))?;
    define.as_object().expect("Object.defineProperty").call(None, &[o.value(), tag, tag_descriptor.value()])?;
    let descriptor = s.object();
    descriptor.set_atom("value", s.number(1.0))?;
    define.as_object().expect("Object.defineProperty").call(None, &[escape.value(), s.string("length"), descriptor.value()])?;
    let register = s.function("registerProperty", |s, _this, a| {
        let page = crate::target::page_of(s)?;
        let def = a.get(0);
        let def = if def.is_undefined() || def.is_null() { None } else { Some(def) };
        let def = match def {
            Some(v) => Some(v.as_object().ok_or_else(|| {
                JsError::type_error(
                    "Failed to execute 'registerProperty' on 'CSS': The provided value is not of type 'PropertyDefinition'.",
                )
            })?),
            None => None,
        };
        // PropertyDefinition members, in WebIDL dictionary order.
        let member = |k: &str| def.map_or(Ok(s.undefined()), |d| d.get(k));
        let inherits = member("inherits")?;
        if inherits.is_undefined() {
            return Err(JsError::type_error(
                "Failed to execute 'registerProperty' on 'CSS': required member inherits is undefined.",
            ));
        }
        let inherits = inherits.to_bool();
        let initial = member("initialValue")?;
        let initial = if initial.is_undefined() { None } else { Some(dom_string(initial)?) };
        let name = member("name")?;
        if name.is_undefined() {
            return Err(JsError::type_error(
                "Failed to execute 'registerProperty' on 'CSS': required member name is undefined.",
            ));
        }
        let name = dom_string(name)?;
        let syntax = member("syntax")?;
        let syntax = if syntax.is_undefined() { "*".to_owned() } else { dom_string(syntax)? };
        let initial_value = pyro_dom::stylo::validate_property_registration(&name, &syntax, initial.as_deref())
            .map_err(|message| JsError::dom("SyntaxError", message))?;
        let property = pyro_dom::style::RegisteredProperty { name, syntax, inherits, initial_value };
        let fresh = page.layout.borrow_mut().style_mut().register_property(property);
        if !fresh {
            return Err(JsError::dom("InvalidModificationError", "The name provided has already been registered."));
        }
        Ok(s.undefined())
    })?;
    o.set_atom("registerProperty", register.value())?;
    let page = crate::target::page_of(s)?;
    o.set_atom("highlights", crate::highlights::member(s, &page, "highlights")?)?;
    // CSS Painting API: `paintWorklet` ([SecureContext]).
    if let Some(worklet) = crate::paint_worklet::paint_worklet(s, &page)? {
        o.set_atom("paintWorklet", worklet)?;
    }
    Ok(o.value())
}

/// `window.external` (HTML obsolete `External`): its search provider methods do nothing.
pub struct External;
impl JsClass for External {
    const NAME: &'static str = "External";
    type Data = ();
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.method("AddSearchProvider", 0, |s, _d, _a| Ok(s.undefined()));
        def.method("IsSearchProviderInstalled", 0, |s, _d, _a| Ok(s.undefined()));
    }
}

/// `window.external`, [SameObject].
pub fn external<'s>(s: &Scope<'s>, page: &Rc<PageDom>) -> JsResult<Value<'s>> {
    let key = pyro_js::IdentityKey(page.id() | (1 << 57), 14);
    same_object(s, page, key, || s.wrap::<External>((), Some(key)))
}

/// `navigator.plugins`: always empty (no PDF viewer).
pub struct PluginArray;
impl JsClass for PluginArray {
    const NAME: &'static str = "PluginArray";
    type Data = ();
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.getter("length", |s, _d| Ok(s.number(0.0)));
        def.method("item", 1, |s, _d, a| {
            a.require(1, "PluginArray.item")?;
            Ok(s.null())
        });
        def.method("namedItem", 1, |s, _d, a| {
            a.require(1, "PluginArray.namedItem")?;
            Ok(s.null())
        });
        def.method("refresh", 0, |s, _d, _a| Ok(s.undefined()));
    }
}

/// `navigator.mimeTypes`: always empty (no PDF viewer).
pub struct MimeTypeArray;
impl JsClass for MimeTypeArray {
    const NAME: &'static str = "MimeTypeArray";
    type Data = ();
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.getter("length", |s, _d| Ok(s.number(0.0)));
        def.method("item", 1, |s, _d, a| {
            a.require(1, "MimeTypeArray.item")?;
            Ok(s.null())
        });
        def.method("namedItem", 1, |s, _d, a| {
            a.require(1, "MimeTypeArray.namedItem")?;
            Ok(s.null())
        });
    }
}

struct BarProp;
impl JsClass for BarProp {
    const NAME: &'static str = "BarProp";
    type Data = std::rc::Weak<PageDom>;
    type Parent = NoParent;

    fn define(def: &mut ClassDef<Self>) {
        def.getter("visible", |s, page| Ok(s.boolean(page.upgrade().is_some_and(|p| p.bars_visible()))));
    }
}

fn bar_prop<'s>(s: &Scope<'s>, page: &Rc<PageDom>, slot: u64) -> JsResult<Value<'s>> {
    let key = pyro_js::IdentityKey(page.id() | (1 << 57), slot);
    same_object(s, page, key, || s.wrap::<BarProp>(Rc::downgrade(page), Some(key)))
}

pub(crate) fn register(engine: &pyro_js::Engine) {
    engine.register::<BarProp>();
    engine.register::<Navigator>();
    engine.register::<PluginArray>();
    engine.register::<External>();
    engine.register::<MimeTypeArray>();
    engine.register::<crate::user_activation::UserActivation>();
    engine.register::<Screen>();
    engine.register::<VisualViewport>();
    engine.register::<ScreenOrientation>();
}

/// `window.open(url, target, features)`. A new browsing context opens as a
/// popup page in the same browser context (the CDP layer announces it with
/// its opener). With `noopener`/`noreferrer` it returns null, as browsers do;
/// otherwise it returns this realm's [`crate::popup::PopupWindow`] for the
/// popup (see that module for what it reaches). A named popup this document
/// opened earlier and still open is reused: the same object comes back and
/// the popup navigates to `url` unless it is empty. An existing frame of the
/// page that the name chooses (`_parent`, `_top`, a frame's target name; the
/// embedder's `target_window` hook) comes back the same way. The popup's
/// session storage is copied from this document's now. `_self` navigates this
/// document.
pub(crate) fn window_open<'s>(s: &Scope<'s>, page: &Rc<PageDom>, a: &pyro_js::Args<'s>) -> JsResult<Value<'s>> {
    let url = crate::util::opt_dom_string(a.get(0))?.unwrap_or_default();
    let target = crate::util::opt_dom_string(a.get(1))?.filter(|t| !t.is_empty()).unwrap_or_else(|| "_blank".into());
    let features = crate::window_features::WindowFeatures::parse(&crate::util::opt_dom_string(a.get(2))?.unwrap_or_default());
    let no_opener = features.noopener;
    let url_given = !url.is_empty();
    let url = if url.is_empty() {
        "about:blank".to_owned()
    } else {
        let dom = page.dom.borrow();
        let url = crate::html::resolve_url(&dom, page.document(), &url);
        if pyro_net::Url::parse(&url).is_err() {
            return Err(JsError::dom("SyntaxError", format!("'{url}' is not a valid URL.")));
        }
        url
    };
    if pyro_net::Url::parse(&url).is_err() {
        return Err(JsError::dom("SyntaxError", format!("'{url}' is not a valid URL.")));
    }
    let lower = target.to_ascii_lowercase();
    let same_context = matches!(lower.as_str(), "_self" | "_top" | "_parent");
    let named = !same_context && lower != "_blank";
    // Cross-Origin-Opener-Policy: a window a frame of another origin than its top-level `same-origin`
    // document creates has no opener and no name (`PageHooks::coop_forces_noopener`).
    let coop_noopener = !same_context && page.hooks().is_some_and(|h| h.coop_forces_noopener());
    // An existing frame of this page the name chooses (`_parent`, `_top`, a frame's target
    // name): it navigates when there is a URL, and window.open returns its WindowProxy.
    if lower == "_parent" || lower == "_top" || (named && !no_opener) {
        if let Some(window) = page.hooks().and_then(|h| h.target_window(s, &target)) {
            if url_given {
                page.navigate(crate::page::NavigationRequest {
                    url,
                    target: Some(target.clone()),
                    method: "GET".into(),
                    body: None,
                    content_type: None,
                    opener: false,
                    popup: None,
                    window_features: features,
                    referrer_policy: features.noreferrer.then_some(pyro_net::ReferrerPolicy::NoReferrer),
                    source: None,
                    form_entries: None,
                    replace: false,
                    user_initiated: false,
                });
            }
            return Ok(window);
        }
    }
    if named && !no_opener {
        let existing =
            page.popups.borrow().iter().find(|(n, p)| *n == target && !p.is_closed()).map(|(_, p)| p.clone());
        if let Some(popup) = existing {
            if url_given {
                page.popup_command(popup.clone(), crate::popup::PopupCommand::OpenNamed {
                    url,
                    referrer: page.client_referrer().map(Into::into),
                    policy: page.referrer_policy(),
                });
            }
            return crate::popup::wrap_popup(s, page, &popup);
        }
    }
    // HTML "the rules for choosing a navigable": a document with the sandboxed auxiliary
    // navigation browsing context flag (a sandbox without `allow-popups`) gets no new window,
    // and window.open returns null. An existing frame the name chooses was handled above (with
    // noopener, by the embedder's navigate lookup, which window.open answers with null); without
    // the embedder's hook the navigation is still asked for (the embedder looks the name up first
    // and blocks only a new window), without a popup ticket.
    let sandboxed = !same_context && page.popups_sandboxed();
    let (ticket, popup) = if same_context || no_opener || coop_noopener || sandboxed {
        (None, None)
    } else {
        let name = if named { target.clone() } else { String::new() };
        let (ticket, popup) = crate::popup::PopupTicket::new(name, copy_session_storage(s, page));
        (Some(ticket), Some(popup))
    };
    let opener = ticket.is_some();
    page.navigate(crate::page::NavigationRequest {
        url,
        target: Some(target.clone()),
        method: "GET".into(),
        body: None,
        content_type: None,
        opener,
        popup: ticket,
        // The `noreferrer` feature: no referrer (the others: the document's policy).
        window_features: features,
        referrer_policy: features.noreferrer.then_some(pyro_net::ReferrerPolicy::NoReferrer),
        source: None,
        form_entries: None,
        replace: false,
        user_initiated: false,
    });
    if sandboxed {
        return Ok(s.null());
    }
    match popup {
        Some(popup) => {
            if named {
                let mut popups = page.popups.borrow_mut();
                popups.retain(|(n, p)| *n != target && !p.is_closed());
                popups.push((target, popup.clone()));
            }
            crate::popup::wrap_popup(s, page, &popup)
        }
        None if same_context => Ok(s.global().value()),
        None => Ok(s.null()),
    }
}

/// HTML "get an element's noopener" for a hyperlink or form that targets `target` with link
/// types `rel`: rel=noopener or noreferrer, or `_blank` without rel=opener.
pub(crate) fn element_noopener(rel: &str, target: Option<&str>) -> bool {
    let rel = rel.to_ascii_lowercase();
    let has = |t: &str| rel.split_ascii_whitespace().any(|r| r == t);
    if has("noopener") || has("noreferrer") {
        return true;
    }
    !has("opener") && target.is_some_and(|t| t.eq_ignore_ascii_case("_blank"))
}

/// The popup ticket for a hyperlink or form submission whose target may create a new
/// auxiliary browsing context (HTML "rules for choosing a navigable" with a new top-level
/// traversable): like `window.open`'s, it gives the popup its opener, its name ("" for
/// `_blank`) and a copy of this document's session storage. It goes unused when the name turns
/// out to be an existing frame. None for `_self`/`_top`/`_parent`, no target, or noopener (the
/// popup then has no opener and no name).
pub(crate) fn element_popup_ticket(
    page: &Rc<PageDom>,
    target: Option<&str>,
    noopener: bool,
) -> Option<Rc<crate::popup::PopupTicket>> {
    let target = target.filter(|t| !t.is_empty())?;
    let lower = target.to_ascii_lowercase();
    let noopener = noopener || page.hooks().is_some_and(|h| h.coop_forces_noopener());
    if noopener || matches!(lower.as_str(), "_self" | "_top" | "_parent") {
        return None;
    }
    let named = lower != "_blank";
    let mut session = crate::storage::StorageNamespace::default();
    if let Some(realm) = page.main_world_realm() {
        let scope = realm.scope();
        if let Ok(s) = &scope {
            session = copy_session_storage(s, page);
        }
    }
    let name = if named { target.to_owned() } else { String::new() };
    let (ticket, _popup) = crate::popup::PopupTicket::new(name, session);
    Some(ticket)
}

/// A new session storage namespace holding a copy of this document's
/// session storage area (HTML copies the opener's whole session storage
/// into a new top-level traversable). Only the document's own origin is
/// copied: the namespace does not expose other origins' areas, and a popup
/// starts there anyway. An opaque-origin document has no area to copy.
fn copy_session_storage(s: &Scope<'_>, page: &Rc<PageDom>) -> crate::storage::StorageNamespace {
    let copy = (|| -> JsResult<Option<crate::storage::StorageNamespace>> {
        let Some(origin) = crate::storage::tuple_origin(s, page) else { return Ok(None) };
        let storage = crate::storage::window_storage(s, page, true)?;
        let Some(storage) = storage.as_object() else { return Ok(None) };
        let key = storage.get_atom("key")?.as_object().ok_or(JsError::ContextDestroyed)?;
        let get = storage.get_atom("getItem")?.as_object().ok_or(JsError::ContextDestroyed)?;
        let length = storage.get_atom("length")?.to_number()? as u32;
        let mut entries = Vec::with_capacity(length as usize);
        for i in 0..length {
            let name = key.call(Some(storage), &[s.number(f64::from(i))])?;
            if name.is_null() {
                break;
            }
            let name = name.to_string()?;
            let value = get.call(Some(storage), &[s.string(&name)])?;
            if !value.is_null() {
                entries.push((name, value.to_string()?));
            }
        }
        let seed = crate::storage::StorageSeed::new(vec![(origin, entries)])
            .map_err(|e| JsError::Type(format!("cannot copy session storage: {e}").into()))?;
        Ok(Some(crate::storage::StorageNamespace::seeded(&seed)))
    })();
    match copy {
        Ok(Some(namespace)) => namespace,
        // SecurityError (opaque origin) or a disposed page: the popup starts empty.
        _ => crate::storage::StorageNamespace::default(),
    }
}

/// `window.find`: the next match of `needle` (ASCII case-insensitive unless `case_sensitive`)
/// within one text node of the document, after the selection's end (before its start when
/// `backwards`), wrapping around when `wrap`. The match becomes the selection.
fn find_text(page: &Rc<PageDom>, needle: &str, case_sensitive: bool, backwards: bool, wrap: bool) -> bool {
    if needle.is_empty() {
        return false;
    }
    let doc = page.document();
    let texts: Vec<(pyro_dom::NodeId, Vec<u16>)> = {
        let dom = page.dom.borrow();
        // Shadow trees are searched too (Chromium searches the flat tree: text in a shadow root is findable).
        let Ok(nodes) = dom.shadow_including_nodes(doc) else { return false };
        nodes
            .into_iter()
            .filter(|&n| dom.node_type(n).ok() == Some(pyro_dom::NodeType::Text))
            .filter_map(|n| Some((n, dom.text_content_ref(n).ok()??.encode_utf16().collect())))
            .collect()
    };
    let fold = |c: u16| if !case_sensitive && (b'A' as u16..=b'Z' as u16).contains(&c) { c + 32 } else { c };
    let needle: Vec<u16> = needle.encode_utf16().map(fold).collect();
    let matches = |text: &[u16], at: usize| text[at..at + needle.len()].iter().zip(&needle).all(|(&c, &n)| fold(c) == n);
    // Where the search starts: the text node of the selection's end (start), and the offset in it.
    let selection = crate::editing::selection_range(page);
    let from = selection.and_then(|(start, end)| {
        let point = if backwards { start } else { end };
        texts.iter().position(|(n, _)| *n == point.0).map(|i| (i, point.1 as usize))
    });
    let count = texts.len();
    let order: Vec<usize> = match (from, backwards) {
        (Some((i, _)), false) => (i..count).chain((0..=i).filter(|_| wrap)).collect(),
        (Some((i, _)), true) => (0..=i).rev().chain((i..count).rev().filter(|_| wrap)).collect(),
        (None, false) => (0..count).collect(),
        (None, true) => (0..count).rev().collect(),
    };
    for (step, &i) in order.iter().enumerate() {
        let (node, text) = &texts[i];
        if text.len() < needle.len() {
            continue;
        }
        let last = text.len() - needle.len();
        // Within the node the search starts from: only past (before) the selection.
        let (lo, hi) = match from {
            Some((at, offset)) if step == 0 && at == i => match backwards {
                false => (offset, last),
                true => (0, offset.saturating_sub(needle.len()).min(last)),
            },
            _ => (0, last),
        };
        if lo > hi {
            continue;
        }
        let found = match backwards {
            false => (lo..=hi).find(|&at| matches(text, at)),
            true => (lo..=hi).rev().find(|&at| matches(text, at)),
        };
        if let Some(at) = found {
            let searchable = {
                let dom = page.dom.borrow();
                dom.parent_element(*node).ok().flatten().is_some_and(|parent| {
                    page.layout().style().computed(&dom, parent).is_some_and(|s|
                        !s.inert && !s.in_display_none && !s.visibility_hidden)
                })
            };
            if !searchable { continue; }
            // As Chromium: selecting the match starts a selection (`selectstart` at its text, which
            // a listener may cancel).
            if !crate::input::fire_simple(page, *node, "selectstart", true, true) {
                return false;
            }
            crate::editing::set_selection(page, (*node, at as u32), (*node, (at + needle.len()) as u32));
            return true;
        }
    }
    false
}
