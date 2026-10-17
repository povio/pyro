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
        Ok(((!l.is_undefined()).then(|| to_i32(l)).transpose()?, (!t.is_undefined()).then(|| to_i32(t)).transpose()?))
    } else if a.len() >= 2 {
        Ok((Some(to_i32(a.get(0))?), Some(to_i32(a.get(1))?)))
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
        let p = page.clone();
        page.post_task(Box::new(move || {
            let doc = p.document();
            crate::input::fire_simple(&p, doc, "scroll", true, false);
        }));
    }
    Ok(())
}

/// Adds lp-webapi's Window members to lp-browser's `Window` class.
pub fn define_members<W>(def: &mut ClassDef<W>)
where
    W: JsClass,
    W::Data: WindowHost,
{
    def.getter("document", |s, w| Ok(crate::node::wrap_node(s, w.page(), w.page().document())?.value()));
    def.getter("indexedDB", |s, w| crate::indexeddb::window_factory(s, w.page()));
    def.getter("localStorage", |s, w| crate::storage::window_storage(s, w.page(), false));
    def.getter("sessionStorage", |s, w| crate::storage::window_storage(s, w.page(), true));
    def.getter("customElements", |s, w| crate::custom::registry(s, w.page()));
    def.getter("navigator", |s, w| navigator(s, w.page()));
    def.getter("clientInformation", |s, w| navigator(s, w.page()));
    def.getter("screen", |s, w| {
        let key = lp_js::IdentityKey(w.page().id() | (1 << 57), 1);
        if let Some(o) = s.lookup_identity(key) {
            return Ok(o.value());
        }
        Ok(s.wrap::<Screen>(w.page().clone(), Some(key))?.value())
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
    def.getter("innerWidth", |s, w| Ok(s.number(w.page().layout.borrow().inner_width() as f64)));
    def.getter("innerHeight", |s, w| Ok(s.number(w.page().layout.borrow().inner_height() as f64)));
    def.getter("outerWidth", |s, w| Ok(s.number(w.page().layout.borrow().inner_width() as f64)));
    def.getter("outerHeight", |s, w| Ok(s.number(w.page().layout.borrow().inner_height() as f64)));
    def.getter("devicePixelRatio", |s, w| Ok(s.number(w.page().layout.borrow().device_pixel_ratio())));
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
    def.method("focus", 0, |s, _w, _a| Ok(s.undefined()));
    def.method("blur", 0, |s, _w, _a| Ok(s.undefined()));
    def.method("print", 0, |s, _w, _a| Ok(s.undefined()));
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
                f.origin = origin;
            }
            crate::target::fire(&p2, key, &ev);
        }));
        Ok(s.undefined())
    });
}

fn navigator<'s>(s: &Scope<'s>, page: &Rc<PageDom>) -> JsResult<Value<'s>> {
    let key = lp_js::IdentityKey(page.id() | (1 << 57), 0);
    if let Some(o) = s.lookup_identity(key) {
        return Ok(o.value());
    }
    Ok(s.wrap::<Navigator>(page.clone(), Some(key))?.value())
}

fn user_agent(page: &PageDom) -> String {
    page.net.borrow().as_ref().map(|n| n.user_agent()).unwrap_or_else(|| lp_net::NetConfig::default().user_agent)
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
        def.getter("platform", |s, _p| {
            Ok(s.string(if cfg!(target_os = "macos") { "MacIntel" } else { "Linux x86_64" }))
        });
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
        def.getter("maxTouchPoints", |s, _p| Ok(s.number(0.0)));
        def.getter("webdriver", |s, _p| Ok(s.boolean(true)));
        def.getter("doNotTrack", |s, _p| Ok(s.null()));
        def.getter("pdfViewerEnabled", |s, _p| Ok(s.boolean(false)));
        def.method("javaEnabled", 0, |s, _p, _a| Ok(s.boolean(false)));
        def.method("sendBeacon", 1, |s, _p, _a| Ok(s.boolean(true)));
        def.method("vibrate", 0, |s, _p, _a| Ok(s.boolean(false)));
    }
}

fn languages(p: &PageDom) -> Vec<String> {
    let header = p
        .net
        .borrow()
        .as_ref()
        .and_then(|n| n.settings().accept_language)
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
            let (w, h) = screen(p);
            let o = s.object();
            o.set("type", s.string(if w >= h { "landscape-primary" } else { "portrait-primary" }))?;
            o.set("angle", s.number(0.0))?;
            Ok(o.value())
        });
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
}
