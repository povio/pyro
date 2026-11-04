//! Window members owned by lp-webapi (lp-browser owns the Window class,
//! location/history and timers) plus Navigator and Screen.

use std::rc::Rc;

use lp_js::{ClassDef, JsClass, JsError, JsResult, NoParent, Scope, Value};

use crate::page::PageDom;
use crate::target::EventTargetData;
use crate::util::*;

/// Implemented by lp-browser's Window data.
pub trait WindowHost {
    fn page(&self) -> &Rc<PageDom>;
    fn event_target(&self) -> &EventTargetData;
}

fn scroll_xy(a: &lp_js::Args<'_>) -> JsResult<(Option<i32>, Option<i32>)> {
    if let Some(o) = a.get(0).as_object() {
        let l = o.get("left")?;
        let t = o.get("top")?;
        Ok((
            (!l.is_undefined()).then(|| to_scroll_offset(l)).transpose()?,
            (!t.is_undefined()).then(|| to_scroll_offset(t)).transpose()?,
        ))
    } else if a.len() >= 2 {
        Ok((Some(to_scroll_offset(a.get(0))?), Some(to_scroll_offset(a.get(1))?)))
    } else {
        Ok((None, None))
    }
}

fn window_scroll(page: &Rc<PageDom>, a: &lp_js::Args<'_>, by: bool) -> JsResult<()> {
    let (l, t) = scroll_xy(a)?;
    let moved = {
        let dom = page.dom.borrow();
        let mut layout = page.layout.borrow_mut();
        if by {
            layout.window_scroll_by(&dom, l, t)
        } else {
            layout.window_scroll_to(&dom, l, t)
        }
    };
    if moved {
        crate::layout_api::fire_scroll(page, None);
    }
    Ok(())
}

/// Setter of a [Replaceable] Window attribute: assignment shadows it with an
/// own data property.
pub fn replace_window_attribute<'s>(
    s: &lp_js::Scope<'s>,
    page: &crate::page::PageDom,
    name: &str,
    value: lp_js::Value<'s>,
) -> lp_js::JsResult<()> {
    let define = page.define_properties.borrow().get(&s.realm_id()).map(|p| p.get(s));
    let define = define
        .and_then(|v| v.as_object())
        .ok_or_else(|| lp_js::JsError::Type(format!("window.{name} cannot be replaced in this realm")))?;
    let desc = s.object();
    desc.set("value", value)?;
    for k in ["writable", "enumerable", "configurable"] {
        desc.set(k, s.boolean(true))?;
    }
    define.call(None, &[s.global().value(), s.string(name), desc.value()])?;
    Ok(())
}

/// Adds lp-webapi's Window members to lp-browser's `Window` class.
pub fn define_members<W>(def: &mut ClassDef<W>)
where
    W: JsClass,
    W::Data: WindowHost,
{
    crate::handlers::define_window(def);
    def.getter("document", |s, w| Ok(crate::node::wrap_node(s, w.page(), w.page().document())?.value()));
    // `window.event` (DOM "current event"), [Replaceable]: assignment shadows
    // it with an own data property.
    def.accessor(
        "event",
        |s, _w| crate::target::current_event(s),
        |s, w, value| replace_window_attribute(s, w.page(), "event", value),
    );
    def.getter("indexedDB", |s, w| crate::indexeddb::window_factory(s, w.page()));
    def.getter("localStorage", |s, w| crate::storage::window_storage(s, w.page(), false));
    def.getter("sessionStorage", |s, w| crate::storage::window_storage(s, w.page(), true));
    def.getter("customElements", |s, w| crate::custom::registry(s, w.page()));
    def.getter("navigator", |s, w| navigator(s, w.page()));
    def.getter("crypto", |s, _w| crate::crypto::crypto_value(s));
    def.getter("clientInformation", |s, w| navigator(s, w.page()));
    def.getter("ClipboardItem", |s, w| crate::navigator_extras::member(s, w.page(), "ClipboardItem"));
    crate::streams::define_globals(def);
    def.getter("screen", |s, w| {
        let key = lp_js::IdentityKey(w.page().id() | (1 << 57), 1);
        same_object(s, w.page(), key, || s.wrap::<Screen>(w.page().clone(), Some(key)))
    });
    def.getter("visualViewport", |s, w| {
        let key = viewport_identity(w.page(), ViewportTarget::VisualViewport);
        same_object(s, w.page(), key, || {
            s.wrap::<VisualViewport>(ViewportTargetData::new(w.page(), ViewportTarget::VisualViewport), Some(key))
        })
    });
    def.method("getComputedStyle", 1, |s, w, a| {
        let el = crate::node::node_arg(s, a.get(0), "Window.getComputedStyle")?;
        Ok(crate::layout_api::get_computed_style(s, w.page(), el)?.value())
    });
    def.method("matchMedia", 1, |s, w, a| {
        let q = arg_string(&a, 0, "Window.matchMedia")?;
        Ok(crate::layout_api::match_media(s, w.page(), q)?.value())
    });
    def.method("getSelection", 0, |s, w, _a| crate::range::get_selection(s, w.page()));
    def.method("open", 0, |s, w, a| window_open(s, w.page(), &a));
    def.getter("innerWidth", |s, w| Ok(s.number(w.page().layout().inner_width() as f64)));
    def.getter("innerHeight", |s, w| Ok(s.number(w.page().layout().inner_height() as f64)));
    def.getter("outerWidth", |s, w| Ok(s.number(w.page().layout().inner_width() as f64)));
    def.getter("outerHeight", |s, w| Ok(s.number(w.page().layout().inner_height() as f64)));
    def.getter("devicePixelRatio", |s, w| Ok(s.number(w.page().layout().device_pixel_ratio())));
    def.getter("scrollX", |s, w| Ok(s.number(w.page().layout.borrow().scroll_x() as f64)));
    def.getter("scrollY", |s, w| Ok(s.number(w.page().layout.borrow().scroll_y() as f64)));
    def.getter("pageXOffset", |s, w| Ok(s.number(w.page().layout.borrow().scroll_x() as f64)));
    def.getter("pageYOffset", |s, w| Ok(s.number(w.page().layout.borrow().scroll_y() as f64)));
    def.getter("screenX", |s, _w| Ok(s.number(0.0)));
    def.getter("screenY", |s, _w| Ok(s.number(0.0)));
    def.getter("screenLeft", |s, _w| Ok(s.number(0.0)));
    def.getter("screenTop", |s, _w| Ok(s.number(0.0)));
    def.method("scroll", 0, |s, w, a| window_scroll(w.page(), &a, false).map(|_| s.undefined()));
    def.method("scrollTo", 0, |s, w, a| window_scroll(w.page(), &a, false).map(|_| s.undefined()));
    def.method("scrollBy", 0, |s, w, a| window_scroll(w.page(), &a, true).map(|_| s.undefined()));
    def.method("atob", 1, |s, _w, a| Ok(s.string(&crate::encoding::atob(&arg_string(&a, 0, "Window.atob")?)?)));
    def.method("btoa", 1, |s, _w, a| Ok(s.string(&crate::encoding::btoa(&arg_string(&a, 0, "Window.btoa")?)?)));
    def.getter("frameElement", |s, _w| Ok(s.null()));
    def.getter("opener", |s, _w| Ok(s.null()));
    def.getter("closed", |s, _w| Ok(s.boolean(false)));
    def.getter("length", |s, _w| Ok(s.number(0.0)));
    def.getter("origin", |s, w| {
        let url =
            w.page().dom.borrow().document_data(w.page().document()).map(|d| d.url.to_string()).unwrap_or_default();
        Ok(s.string(
            &lp_net::Url::parse(&url).map(|u| u.origin().ascii_serialization()).unwrap_or_else(|_| "null".into()),
        ))
    });
    def.method("focus", 0, |s, w, _a| {
        crate::input::focus_window(w.page());
        Ok(s.undefined())
    });
    def.method("blur", 0, |s, _w, _a| Ok(s.undefined()));
    def.method("print", 0, |s, _w, _a| Ok(s.undefined()));
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
    def.method("stop", 0, |s, _w, _a| Ok(s.undefined()));
    def.method("postMessage", 1, |s, w, a| {
        // Same-window postMessage: queue a MessageEvent on the window.
        let page = w.page().clone();
        let key = w.event_target().key;
        let data = lp_js::Persistent::new(s, a.get(0));
        let origin = {
            let url = page.dom.borrow().document_data(page.document()).map(|d| d.url.to_string()).unwrap_or_default();
            lp_net::Url::parse(&url).map(|u| u.origin().ascii_serialization()).unwrap_or_else(|_| "null".into())
        };
        let p2 = page.clone();
        page.post_task(Box::new(move || {
            let ev = crate::event::EventCore::trusted(
                &p2,
                crate::event::EventInterface::MessageEvent,
                "message",
                false,
                false,
            );
            {
                let mut f = ev.fields.borrow_mut();
                f.message_data = Some(data);
                f.message_source = Some(key);
                f.origin = origin;
            }
            crate::target::fire(&p2, key, &ev);
        }));
        Ok(s.undefined())
    });
}

/// A Window [SameObject] attribute value for this realm: looked up by
/// `key`, otherwise created by `make` and kept alive until the realm (or
/// page) is disposed, so expandos and `defineProperty` overrides persist.
pub(crate) fn same_object<'s>(
    s: &Scope<'s>,
    page: &PageDom,
    key: lp_js::IdentityKey,
    make: impl FnOnce() -> JsResult<lp_js::Object<'s>>,
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
            page.same_objects.borrow_mut().insert(realm, (lp_js::Persistent::new(s, holder.value()), 0));
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
    let key = lp_js::IdentityKey(page.id() | (1 << 57), 0);
    same_object(s, page, key, || s.wrap::<Navigator>(page.clone(), Some(key)))
}

pub(crate) fn user_agent(page: &PageDom) -> String {
    if let Some(ua) = page.identity.borrow().value.as_ref().map(|v| &v.user_agent).filter(|s| !s.is_empty()) {
        return ua.clone();
    }
    page.net.borrow().as_ref().map(|n| n.user_agent()).unwrap_or_else(|| lp_net::NetConfig::default().user_agent)
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
            let mut vals = lp_js::RootedVec::new();
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
        def.getter("webdriver", |s, _p| Ok(s.boolean(true)));
        def.getter("doNotTrack", |s, _p| Ok(s.null()));
        def.getter("pdfViewerEnabled", |s, _p| Ok(s.boolean(false)));
        def.method("javaEnabled", 0, |s, _p, _a| Ok(s.boolean(false)));
        def.method("sendBeacon", 1, |s, _p, _a| Ok(s.boolean(true)));
        def.method("vibrate", 0, |s, _p, _a| Ok(s.boolean(false)));
    }
}

fn dialog(
    page: &PageDom,
    kind: &'static str,
    message: lp_js::Value<'_>,
    default: Option<String>,
) -> lp_js::JsResult<Option<crate::page::DialogAnswer>> {
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
        .unwrap_or_else(|| lp_net::NetConfig::default().accept_language);
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

fn viewport_identity(page: &PageDom, kind: ViewportTarget) -> lp_js::IdentityKey {
    let slot = match kind {
        ViewportTarget::VisualViewport => 3,
        ViewportTarget::Orientation => 4,
    };
    lp_js::IdentityKey(page.id() | (1 << 57), slot)
}

/// This realm's wrapper of viewport target `key`, if it is one.
pub(crate) fn viewport_target_object<'s>(
    s: &Scope<'s>,
    page: &PageDom,
    key: lp_dom::events::TargetKey,
) -> Option<lp_js::Object<'s>> {
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
/// the layout viewport (headless Chromium hides scrollbars). react-aria
/// bounds popover max-height by `offsetTop + height`; without this object
/// every popover got `max-height: 0`. An EventTarget: the embedder fires
/// `resize` when the viewport size changes (react-aria tracks the virtual
/// keyboard with it). With no pinch zoom the visual viewport never moves
/// within the layout viewport, so `scroll`/`scrollend` never fire.
pub struct VisualViewport;
impl JsClass for VisualViewport {
    const NAME: &'static str = "VisualViewport";
    type Data = ViewportTargetData;
    type Parent = crate::target::EventTarget;
    fn define(def: &mut ClassDef<Self>) {
        def.upcast(|d: &ViewportTargetData| &d.target);
        def.getter("width", |s, d| Ok(s.number(d.page().layout().inner_width() as f64)));
        def.getter("height", |s, d| Ok(s.number(d.page().layout().inner_height() as f64)));
        def.getter("offsetLeft", |s, _d| Ok(s.number(0.0)));
        def.getter("offsetTop", |s, _d| Ok(s.number(0.0)));
        def.getter("pageLeft", |s, d| Ok(s.number(d.page().layout.borrow().scroll_x() as f64)));
        def.getter("pageTop", |s, d| Ok(s.number(d.page().layout.borrow().scroll_y() as f64)));
        def.getter("scale", |s, _d| Ok(s.number(1.0)));
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
    let v = p.viewport();
    (v.screen_width.unwrap_or(v.width), v.screen_height.unwrap_or(v.height))
}

/// `CSS` namespace object (supports/escape).
pub fn css_namespace<'s>(s: &Scope<'s>) -> JsResult<Value<'s>> {
    let o = s.object();
    let supports = s.function("supports", |s, _this, a| {
        // Syntax-level check only: property names and values are accepted.
        let ok = if a.len() >= 2 {
            !dom_string(a.get(0))?.is_empty()
        } else {
            let cond = dom_string(a.get(0))?;
            cond.contains(':') || cond.trim_start().starts_with("selector(")
        };
        Ok(s.boolean(ok))
    })?;
    o.set("supports", supports.value())?;
    let escape = s.function("escape", |s, _this, a| {
        let v = arg_string(&a, 0, "CSS.escape")?;
        let mut out = String::new();
        cssparser::serialize_identifier(&v, &mut out).map_err(|_| JsError::Type("CSS.escape failed".into()))?;
        Ok(s.string(&out))
    })?;
    o.set("escape", escape.value())?;
    Ok(o.value())
}

pub(crate) fn register(engine: &lp_js::Engine) {
    engine.register::<Navigator>();
    engine.register::<Screen>();
    engine.register::<VisualViewport>();
    engine.register::<ScreenOrientation>();
}

/// `window.open(url, target, features)`. A new browsing context opens as a
/// popup page in the same browser context (the CDP layer announces it with
/// its opener). With `noopener`/`noreferrer` it returns null, as browsers do;
/// otherwise it returns the minimal [`crate::popup::PopupWindow`], since there
/// is no cross-page WindowProxy. `_self`/`_top`/`_parent` navigate this page.
fn window_open<'s>(s: &Scope<'s>, page: &Rc<PageDom>, a: &lp_js::Args<'s>) -> JsResult<Value<'s>> {
    let url = crate::util::opt_dom_string(a.get(0))?.unwrap_or_default();
    let target = crate::util::opt_dom_string(a.get(1))?.filter(|t| !t.is_empty()).unwrap_or_else(|| "_blank".into());
    let features = crate::util::opt_dom_string(a.get(2))?.unwrap_or_default().to_ascii_lowercase();
    let no_opener = features
        .split(|c: char| c == ',' || c.is_ascii_whitespace())
        .filter_map(|f| f.split('=').next())
        .any(|f| f == "noopener" || f == "noreferrer");
    let url = if url.is_empty() {
        "about:blank".to_owned()
    } else {
        let dom = page.dom.borrow();
        crate::html::resolve_url(&dom, page.document(), &url)
    };
    if lp_net::Url::parse(&url).is_err() {
        return Err(JsError::dom("SyntaxError", format!("'{url}' is not a valid URL.")));
    }
    let same_context = matches!(target.to_ascii_lowercase().as_str(), "_self" | "_top" | "_parent");
    let (ticket, popup) = if same_context || no_opener {
        (None, None)
    } else {
        let (ticket, popup) = crate::popup::PopupTicket::new();
        (Some(ticket), Some(popup))
    };
    let opener = ticket.is_some();
    page.navigate(crate::page::NavigationRequest {
        url,
        target: Some(target),
        method: "GET".into(),
        body: None,
        content_type: None,
        opener,
        popup: ticket,
    });
    match popup {
        Some(popup) => {
            Ok(s.wrap::<crate::popup::PopupWindow>(crate::popup::PopupData::new(page, popup), None)?.value())
        }
        None if same_context => Ok(s.global().value()),
        None => Ok(s.null()),
    }
}
