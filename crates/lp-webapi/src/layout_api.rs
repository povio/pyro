//! Bindings over lp-dom's faux layout (`lp_dom::layout`): rects, metrics,
//! scrolling, hit testing, getComputedStyle, matchMedia and innerText.

use std::rc::Rc;

use lp_dom::style::CheckVisibilityOptions;
use lp_dom::NodeId;
use lp_js::{Args, ClassDef, JsClass, JsResult, NoParent, Object, Scope, Value};

use crate::node::{wrap_node, NodeData};
use crate::page::PageDom;
use crate::util::*;

/// `DOMRect` / `DOMRectReadOnly` values as a plain JS object with the
/// DOMRect shape (x, y, width, height, top, right, bottom, left, toJSON).
pub fn rect_value<'s>(s: &Scope<'s>, r: lp_dom::layout::Rect) -> JsResult<Value<'s>> {
    let o = s.wrap::<DomRect>(r, None)?;
    Ok(o.value())
}

pub struct DomRect;
impl JsClass for DomRect {
    const NAME: &'static str = "DOMRect";
    type Data = lp_dom::layout::Rect;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(0, |s, nt, a| {
            let n = |i: usize| -> JsResult<f64> {
                if a.get(i).is_undefined() {
                    Ok(0.0)
                } else {
                    a.get(i).to_number()
                }
            };
            let r = lp_dom::layout::Rect { x: n(0)?, y: n(1)?, width: n(2)?, height: n(3)? };
            s.wrap_new::<DomRect>(r, nt, None)
        });
        def.getter("x", |s, r| Ok(s.number(r.x)));
        def.getter("y", |s, r| Ok(s.number(r.y)));
        def.getter("width", |s, r| Ok(s.number(r.width)));
        def.getter("height", |s, r| Ok(s.number(r.height)));
        def.getter("left", |s, r| Ok(s.number(r.left())));
        def.getter("top", |s, r| Ok(s.number(r.top())));
        def.getter("right", |s, r| Ok(s.number(r.right())));
        def.getter("bottom", |s, r| Ok(s.number(r.bottom())));
        def.method("toJSON", 0, |s, r, _a| {
            let o = s.object();
            for (k, v) in [
                ("x", r.x),
                ("y", r.y),
                ("width", r.width),
                ("height", r.height),
                ("top", r.top()),
                ("right", r.right()),
                ("bottom", r.bottom()),
                ("left", r.left()),
            ] {
                o.set(k, s.number(v))?;
            }
            Ok(o.value())
        });
    }
}

pub(crate) fn bounding_rect<'s>(s: &Scope<'s>, n: &NodeData) -> JsResult<Value<'s>> {
    let page = n.page();
    let r = page.layout.borrow().bounding_client_rect(&page.dom.borrow(), n.id);
    rect_value(s, r)
}

pub(crate) fn client_rects<'s>(s: &Scope<'s>, n: &NodeData) -> JsResult<Value<'s>> {
    let page = n.page();
    let rects = page.layout.borrow().client_rects(&page.dom.borrow(), n.id);
    let mut vals = lp_js::RootedVec::new();
    for r in rects {
        vals.push(s, rect_value(s, r)?);
    }
    Ok(s.array(&vals.iter(s).collect::<Vec<_>>())?.value())
}

/// Range geometry for (start container, start offset, end container, end
/// offset). A range in a text node, or selecting exactly one text node
/// (`selectNode(text)`), reports that text's line-box rect when the layout
/// has one (flow geometry); otherwise the start's element box.
pub(crate) fn range_rect<'s>(
    s: &Scope<'s>,
    page: &Rc<PageDom>,
    (start, start_offset, end, end_offset): (NodeId, u32, NodeId, u32),
) -> JsResult<Value<'s>> {
    let text = {
        let dom = page.dom.borrow();
        if dom.node_type(start).js()? == lp_dom::NodeType::Text {
            Some(start)
        } else if start == end && end_offset == start_offset + 1 {
            dom.children(start)
                .js()?
                .nth(start_offset as usize)
                .filter(|&c| dom.node_type(c).ok() == Some(lp_dom::NodeType::Text))
        } else {
            None
        }
    };
    if let Some(t) = text {
        if let Some(r) = page.layout.borrow().text_rect(&page.dom.borrow(), t) {
            return rect_value(s, r);
        }
    }
    let el = {
        let dom = page.dom.borrow();
        if dom.node_type(start).js()? == lp_dom::NodeType::Element {
            Some(start)
        } else {
            dom.parent_element(start).js()?
        }
    };
    let r = match el {
        Some(e) => page.layout.borrow().bounding_client_rect(&page.dom.borrow(), e),
        None => lp_dom::layout::Rect::default(),
    };
    rect_value(s, r)
}

pub(crate) fn fire_scroll(page: &Rc<PageDom>, target: Option<NodeId>) {
    // Scroll events are delivered as a separate task (not throttled).
    let p = page.clone();
    let doc = page.document();
    page.post_task(Box::new(move || {
        if p.is_disposed() || p.document() != doc {
            return;
        }
        match target {
            Some(el) => {
                let connected = {
                    let dom = p.dom.borrow();
                    dom.is_connected(el).unwrap_or(false) && dom.node_document(el).ok() == Some(doc)
                };
                if connected {
                    crate::input::fire_simple(&p, el, "scroll", false, false);
                }
            }
            None => {
                crate::input::fire_simple(&p, doc, "scroll", true, false);
            }
        }
    }));
}

pub(crate) fn scroll_into_view<'s>(s: &Scope<'s>, n: &NodeData) -> JsResult<Value<'s>> {
    let page = n.page();
    let moved = page.layout.borrow_mut().scroll_into_view_targets(&page.dom.borrow(), n.id);
    for target in moved {
        fire_scroll(
            page,
            match target {
                lp_dom::layout::ScrollTarget::Container(node) => Some(node),
                lp_dom::layout::ScrollTarget::Viewport => None,
            },
        );
    }
    Ok(s.undefined())
}

pub(crate) fn check_visibility<'s>(s: &Scope<'s>, n: &NodeData) -> JsResult<Value<'s>> {
    let page = n.page();
    let v = page.layout.borrow().check_visibility(&page.dom.borrow(), n.id, CheckVisibilityOptions::default());
    Ok(s.boolean(v))
}

fn scroll_args(a: &Args<'_>) -> JsResult<(Option<i32>, Option<i32>)> {
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

pub(crate) fn scroll_to<'s>(s: &Scope<'s>, n: &NodeData, a: Args<'s>, by: bool) -> JsResult<Value<'s>> {
    let (l, t) = scroll_args(&a)?;
    let page = n.page();
    let moved = {
        let dom = page.dom.borrow();
        let mut layout = page.layout.borrow_mut();
        if by {
            layout.element_scroll_by(&dom, n.id, l, t)
        } else {
            layout.element_scroll_to(&dom, n.id, l, t)
        }
    };
    if moved {
        fire_scroll(page, Some(n.id));
    }
    Ok(s.undefined())
}

pub(crate) fn scroll_get<'s>(s: &Scope<'s>, n: &NodeData, top: bool) -> JsResult<Value<'s>> {
    let layout = n.page().layout.borrow();
    Ok(s.number(if top { layout.scroll_top(n.id) } else { layout.scroll_left(n.id) } as f64))
}

pub(crate) fn scroll_set(_s: &Scope<'_>, n: &NodeData, v: Value<'_>, top: bool) -> JsResult<()> {
    let value = to_i32(v)?;
    let page = n.page();
    let moved = {
        let dom = page.dom.borrow();
        let mut layout = page.layout.borrow_mut();
        if top {
            layout.set_scroll_top(&dom, n.id, value)
        } else {
            layout.set_scroll_left(&dom, n.id, value)
        }
    };
    if moved {
        fire_scroll(page, Some(n.id));
    }
    Ok(())
}

pub(crate) fn metric<'s>(s: &Scope<'s>, n: &NodeData, which: &str) -> JsResult<Value<'s>> {
    let page = n.page();
    let dom = page.dom.borrow();
    let l = page.layout.borrow();
    let v = match which {
        "scrollWidth" => l.scroll_width(&dom, n.id),
        "scrollHeight" => l.scroll_height(&dom, n.id),
        "clientWidth" => l.client_width(&dom, n.id),
        "clientHeight" => l.client_height(&dom, n.id),
        "offsetTop" => l.offset_top(&dom, n.id),
        "offsetLeft" => l.offset_left(&dom, n.id),
        "offsetWidth" => l.offset_width(&dom, n.id),
        "offsetHeight" => l.offset_height(&dom, n.id),
        _ => 0.0,
    };
    Ok(s.number(v))
}

pub(crate) fn offset_parent<'s>(s: &Scope<'s>, n: &NodeData) -> JsResult<Value<'s>> {
    let page = n.page();
    let p = page.layout.borrow().offset_parent(&page.dom.borrow(), n.id);
    opt_node(s, page, p)
}

pub(crate) fn inner_text<'s>(s: &Scope<'s>, n: &NodeData) -> JsResult<Value<'s>> {
    let page = n.page();
    let t = page.dom.borrow().inner_text(page.layout.borrow().style(), n.id).js()?;
    Ok(s.string(&t))
}

pub(crate) fn set_inner_text(_s: &Scope<'_>, n: &NodeData, v: Value<'_>) -> JsResult<()> {
    let text = null_to_empty(v)?;
    n.page().dom.borrow_mut().set_inner_text(n.id, &text).js()
}

/// Element hit-testing for CDP input (top-most element at the point).
pub fn hit_test(page: &PageDom, x: f64, y: f64) -> Option<NodeId> {
    page.layout.borrow().element_from_point(&page.dom.borrow(), x, y)
}

pub(crate) fn element_from_point<'s>(s: &Scope<'s>, n: &NodeData, a: Args<'s>) -> JsResult<Value<'s>> {
    a.require(2, "Document.elementFromPoint")?;
    let (x, y) = (a.get(0).to_number()?, a.get(1).to_number()?);
    let page = n.page();
    if n.id != page.document() {
        return Ok(s.null());
    }
    let hit = hit_test(page, x, y);
    opt_node(s, page, hit)
}

pub(crate) fn elements_from_point<'s>(s: &Scope<'s>, n: &NodeData, a: Args<'s>) -> JsResult<Value<'s>> {
    a.require(2, "Document.elementsFromPoint")?;
    let (x, y) = (a.get(0).to_number()?, a.get(1).to_number()?);
    let page = n.page();
    let ids = if n.id == page.document() {
        page.layout.borrow().elements_from_point(&page.dom.borrow(), x, y)
    } else {
        Vec::new()
    };
    let mut vals = lp_js::RootedVec::new();
    for id in ids {
        vals.push(s, wrap_node(s, page, id)?.value());
    }
    Ok(s.array(&vals.iter(s).collect::<Vec<_>>())?.value())
}

/// Read-only computed style (`getComputedStyle`).
pub struct ComputedData {
    el: crate::collections::NodeHold,
}

pub struct ComputedStyle;
impl JsClass for ComputedStyle {
    const NAME: &'static str = "CSSStyleDeclaration";
    type Data = ComputedData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.not_exposed();
        def.method("getPropertyValue", 1, |s, c, a| {
            let name = arg_string(&a, 0, "CSSStyleDeclaration.getPropertyValue")?;
            let page = &c.el.page;
            let v = page.layout.borrow().computed_style_value(&page.dom.borrow(), c.el.id, &name);
            Ok(s.string(&v))
        });
        def.method("getPropertyPriority", 1, |s, _c, _a| Ok(s.string("")));
        def.method("item", 1, |s, _c, _a| Ok(s.string("")));
        def.getter("length", |s, _c| Ok(s.number(0.0)));
        def.getter("cssText", |s, _c| Ok(s.string("")));
        def.method("setProperty", 2, |_s, _c, _a| {
            Err::<Value, _>(lp_js::JsError::dom(
                "NoModificationAllowedError",
                "These styles are computed, and therefore read-only.",
            ))
        });
        def.named(false, |s, c, name| {
            if !name.chars().next().is_some_and(|ch| ch.is_ascii_lowercase())
                || !name.chars().all(|ch| ch.is_ascii_alphanumeric())
            {
                return Ok(None);
            }
            let prop = crate::css::camel_to_css(name);
            let page = &c.el.page;
            let v = page.layout.borrow().computed_style_value(&page.dom.borrow(), c.el.id, &prop);
            Ok(Some(s.string(&v)))
        });
    }
}

/// `window.getComputedStyle(el)`.
pub fn get_computed_style<'s>(s: &Scope<'s>, page: &Rc<PageDom>, el: NodeId) -> JsResult<Object<'s>> {
    s.wrap::<ComputedStyle>(ComputedData { el: crate::collections::NodeHold::new(page, el)? }, None)
}

/// `MediaQueryList` (static: `change` events are not dispatched).
pub struct MqlData {
    page: Rc<PageDom>,
    query: String,
}

pub struct MediaQueryList;
impl JsClass for MediaQueryList {
    const NAME: &'static str = "MediaQueryList";
    type Data = MqlData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.getter("media", |s, m| Ok(s.string(&m.query)));
        def.getter("matches", |s, m| Ok(s.boolean(m.page.layout.borrow().style().match_media(&m.query))));
        def.method("addListener", 1, |s, _m, _a| Ok(s.undefined()));
        def.method("removeListener", 1, |s, _m, _a| Ok(s.undefined()));
        def.method("addEventListener", 2, |s, _m, _a| Ok(s.undefined()));
        def.method("removeEventListener", 2, |s, _m, _a| Ok(s.undefined()));
        def.method("dispatchEvent", 1, |s, _m, _a| Ok(s.boolean(true)));
        def.accessor("onchange", |s, _m| Ok(s.null()), |_s, _m, _v| Ok(()));
    }
}

pub fn match_media<'s>(s: &Scope<'s>, page: &Rc<PageDom>, query: String) -> JsResult<Object<'s>> {
    s.wrap::<MediaQueryList>(MqlData { page: page.clone(), query }, None)
}

pub(crate) fn register(engine: &lp_js::Engine) {
    engine.register::<DomRect>();
    engine.register::<ComputedStyle>();
    engine.register::<MediaQueryList>();
}
