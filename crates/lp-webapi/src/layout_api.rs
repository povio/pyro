//! Bindings over lp-dom's faux layout (`lp_dom::layout`): rects, metrics,
//! scrolling, hit testing, getComputedStyle, matchMedia and innerText.

use std::rc::Rc;

use lp_dom::layout::Metric;
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
    let r = page.layout().bounding_client_rect(&page.dom.borrow(), n.id);
    rect_value(s, r.layout_units())
}

pub(crate) fn client_rects<'s>(s: &Scope<'s>, n: &NodeData) -> JsResult<Value<'s>> {
    let page = n.page();
    let rects = page.layout().client_rects(&page.dom.borrow(), n.id);
    let mut vals = lp_js::RootedVec::new();
    for r in rects {
        vals.push(s, rect_value(s, r.layout_units())?);
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
        if let Some(r) = page.layout().text_rect(&page.dom.borrow(), t) {
            return rect_value(s, r.layout_units());
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
        Some(e) => page.layout().bounding_client_rect(&page.dom.borrow(), e),
        None => lp_dom::layout::Rect::default(),
    };
    rect_value(s, r.layout_units())
}

/// Queues the `scroll` event of a completed scroll of `target` (None: the
/// viewport), followed by its `scrollend`.
pub fn fire_scroll(page: &Rc<PageDom>, target: Option<NodeId>) {
    queue_scroll(page, target, true);
}

fn queue_scroll(page: &Rc<PageDom>, target: Option<NodeId>, scrollend: bool) {
    // HTML "run the scroll steps": a scrolled target is queued once and its
    // scroll event fires in the next rendering update before animation
    // callbacks. A 16 ms fallback also delivers it when no frame was requested.
    // Chromium dispatches it in its next frame: on a page that just rendered
    // (TOS react-aria autocomplete) that
    // was 23-29 ms after Playwright's scroll step, after its click; on an
    // idle page a frame can run within ~1 ms. Fired as an immediate task, it
    // let close-on-scroll handlers close the popover before the click.
    // Scrolls that complete (every scroll here is instant: programmatic,
    // reveal, wheel) are followed by `scrollend` in the same update, as
    // Chromium fires it after instant programmatic scrolls (Gap 35); layout
    // clamps fire `scroll` only.
    let doc = page.document();
    let token = {
        let mut pending = page.pending_scrolls.borrow_mut();
        if let Some((_, end, _)) = pending.iter_mut().find(|(key, _, _)| *key == (doc, target)) {
            *end |= scrollend;
            return;
        }
        let token = Rc::new(());
        pending.push(((doc, target), scrollend, token.clone()));
        token
    };
    let p = page.clone();
    page.post_delayed_task(
        std::time::Duration::from_millis(16),
        Box::new(move || {
            let pending = {
                let mut pending = p.pending_scrolls.borrow_mut();
                pending.iter().position(|(_, _, entry)| Rc::ptr_eq(entry, &token)).map(|i| pending.remove(i).1)
            };
            let Some(end) = pending else { return };
            dispatch_scroll(&p, doc, target, "scroll");
            if end {
                dispatch_scroll(&p, doc, target, "scrollend");
            }
        }),
    );
}

/// Runs queued scroll steps before animation-frame callbacks in a rendering update.
pub fn flush_pending_scrolls(page: &Rc<PageDom>) {
    let pending = std::mem::take(&mut *page.pending_scrolls.borrow_mut());
    for ((doc, target), end, _) in pending {
        dispatch_scroll(page, doc, target, "scroll");
        if end {
            dispatch_scroll(page, doc, target, "scrollend");
        }
    }
}

fn dispatch_scroll(p: &Rc<PageDom>, doc: NodeId, target: Option<NodeId>, kind: &str) {
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
                crate::input::fire_simple(p, el, kind, false, false);
            }
        }
        None => {
            crate::input::fire_simple(p, doc, kind, true, false);
        }
    }
}

/// Re-clamps scroll offsets that the current layout no longer reaches and
/// fires their `scroll` events: `now` (a rendering update) dispatches them
/// at once, otherwise (a script read) at the next frame like other scrolls.
pub(crate) fn clamp_scrolls(page: &Rc<PageDom>, now: bool) {
    if !page.layout.borrow().has_scroll_offsets() {
        return;
    }
    let moved = {
        let dom = page.dom.borrow();
        let moved = page.layout.borrow_mut().clamp_scroll_offsets(&dom);
        page.scroll_clamp.set((Some(dom.version()), page.scroll_clamp.get().1));
        moved
    };
    let doc = page.document();
    for target in moved {
        let node = match target {
            lp_dom::layout::ScrollTarget::Container(node) => Some(node),
            lp_dom::layout::ScrollTarget::Viewport => None,
        };
        if !now {
            queue_scroll(page, node, false);
        } else if !page.pending_scrolls.borrow().iter().any(|(key, _, _)| *key == (doc, node)) {
            dispatch_scroll(page, doc, node, "scroll");
        }
    }
}

/// After a task that changed the DOM while something is scrolled: re-clamp
/// at the next frame (16 ms, the scroll event cadence), when Chromium's
/// rendering update would lay out without any script read.
pub(crate) fn schedule_scroll_clamp(page: &Rc<PageDom>) {
    let (checked, queued) = page.scroll_clamp.get();
    if queued || page.is_disposed() || !page.layout.borrow().has_scroll_offsets() {
        return;
    }
    if checked == Some(page.dom.borrow().version()) {
        return;
    }
    page.scroll_clamp.set((checked, true));
    let p = page.clone();
    page.post_delayed_task(
        std::time::Duration::from_millis(16),
        Box::new(move || {
            p.scroll_clamp.set((p.scroll_clamp.get().0, false));
            if !p.is_disposed() {
                clamp_scrolls(&p, true);
            }
        }),
    );
}

pub(crate) fn scroll_into_view<'s>(s: &Scope<'s>, n: &NodeData) -> JsResult<Value<'s>> {
    let page = n.page();
    let moved = page.layout.borrow_mut().scroll_into_view_targets(&page.dom.borrow(), n.id);
    fire_scroll_targets(page, moved);
    Ok(s.undefined())
}

pub(crate) fn scroll_into_view_if_needed<'s>(s: &Scope<'s>, n: &NodeData, a: Args<'s>) -> JsResult<Value<'s>> {
    let center_if_needed = a.get(0).is_undefined() || a.get(0).to_bool();
    let page = n.page();
    let moved =
        page.layout.borrow_mut().scroll_into_view_if_needed_script_targets(&page.dom.borrow(), n.id, center_if_needed);
    fire_scroll_targets(page, moved);
    Ok(s.undefined())
}

fn fire_scroll_targets(page: &Rc<PageDom>, moved: Vec<lp_dom::layout::ScrollTarget>) {
    for target in moved {
        fire_scroll(
            page,
            match target {
                lp_dom::layout::ScrollTarget::Container(node) => Some(node),
                lp_dom::layout::ScrollTarget::Viewport => None,
            },
        );
    }
}

pub(crate) fn check_visibility<'s>(s: &Scope<'s>, n: &NodeData) -> JsResult<Value<'s>> {
    let page = n.page();
    let v = page.layout().check_visibility(&page.dom.borrow(), n.id, CheckVisibilityOptions::default());
    Ok(s.boolean(v))
}

fn scroll_args(a: &Args<'_>) -> JsResult<(Option<i32>, Option<i32>)> {
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
    // Reading a scroll offset sees the current layout (clamped to it).
    clamp_scrolls(n.page(), false);
    let layout = n.page().layout();
    Ok(s.number(if top { layout.scroll_top(n.id) } else { layout.scroll_left(n.id) } as f64))
}

pub(crate) fn scroll_set(_s: &Scope<'_>, n: &NodeData, v: Value<'_>, top: bool) -> JsResult<()> {
    let value = to_scroll_offset(v)?;
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
    let l = page.layout();
    let metric = match which {
        "scrollWidth" => Metric::ScrollWidth,
        "scrollHeight" => Metric::ScrollHeight,
        "clientWidth" => Metric::ClientWidth,
        "clientHeight" => Metric::ClientHeight,
        "offsetTop" => Metric::OffsetTop,
        "offsetLeft" => Metric::OffsetLeft,
        "offsetWidth" => Metric::OffsetWidth,
        "offsetHeight" => Metric::OffsetHeight,
        _ => return Ok(s.number(0.0)),
    };
    let v = l.metric(&dom, n.id, metric);
    Ok(s.number(v))
}

pub(crate) fn offset_parent<'s>(s: &Scope<'s>, n: &NodeData) -> JsResult<Value<'s>> {
    let page = n.page();
    let p = page.layout().offset_parent(&page.dom.borrow(), n.id);
    opt_node(s, page, p)
}

pub(crate) fn inner_text<'s>(s: &Scope<'s>, n: &NodeData) -> JsResult<Value<'s>> {
    let page = n.page();
    let t = page.dom.borrow().inner_text(page.layout().style(), n.id).js()?;
    Ok(s.string(&t))
}

pub(crate) fn set_inner_text(_s: &Scope<'_>, n: &NodeData, v: Value<'_>) -> JsResult<()> {
    let text = null_to_empty(v)?;
    n.page().dom.borrow_mut().set_inner_text(n.id, &text).js()
}

pub(crate) fn set_outer_text(_s: &Scope<'_>, n: &NodeData, v: Value<'_>) -> JsResult<()> {
    let text = null_to_empty(v)?;
    n.page().dom.borrow_mut().set_outer_text(n.id, &text).js()
}

/// Element hit-testing for CDP input (top-most element at the point).
pub fn hit_test(page: &PageDom, x: f64, y: f64) -> Option<NodeId> {
    page.layout().element_from_point(&page.dom.borrow(), x, y)
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

/// `ShadowRoot.elementFromPoint`: the document's hit, retargeted against
/// the shadow root (a host's for anything deeper, null outside its tree).
pub(crate) fn shadow_element_from_point<'s>(s: &Scope<'s>, n: &NodeData, a: Args<'s>) -> JsResult<Value<'s>> {
    a.require(2, "ShadowRoot.elementFromPoint")?;
    let (x, y) = (a.get(0).to_number()?, a.get(1).to_number()?);
    let page = n.page();
    let hit = hit_test(page, x, y);
    let retargeted = hit.and_then(|h| retarget_to_root(&page.dom.borrow(), h, n.id));
    opt_node(s, page, retargeted)
}

pub(crate) fn shadow_elements_from_point<'s>(s: &Scope<'s>, n: &NodeData, a: Args<'s>) -> JsResult<Value<'s>> {
    a.require(2, "ShadowRoot.elementsFromPoint")?;
    let (x, y) = (a.get(0).to_number()?, a.get(1).to_number()?);
    let page = n.page();
    let ids = page.layout().elements_from_point(&page.dom.borrow(), x, y);
    let mut out: Vec<NodeId> = Vec::new();
    {
        let dom = page.dom.borrow();
        for id in ids {
            if let Some(r) = retarget_to_root(&dom, id, n.id) {
                if out.last() != Some(&r) {
                    out.push(r);
                }
            }
        }
    }
    let mut vals = lp_js::RootedVec::new();
    for id in out {
        vals.push(s, wrap_node(s, page, id)?.value());
    }
    Ok(s.array(&vals.iter(s).collect::<Vec<_>>())?.value())
}

/// DOM "retarget" of `node` against the tree rooted at `root`: the node
/// itself when its root is `root`, else the nearest host whose root is;
/// `None` when `root` is not a shadow-including ancestor.
fn retarget_to_root(dom: &lp_dom::Dom, node: NodeId, root: NodeId) -> Option<NodeId> {
    let mut cur = node;
    loop {
        let r = dom.root_node(cur, false).ok()?;
        if r == root {
            return Some(cur);
        }
        if !dom.is_shadow_root(r).unwrap_or(false) {
            // Not in a shadow tree: nothing to retarget (the host itself).
            return Some(cur);
        }
        cur = dom.fragment_host(r).ok()??;
    }
}

pub(crate) fn elements_from_point<'s>(s: &Scope<'s>, n: &NodeData, a: Args<'s>) -> JsResult<Value<'s>> {
    a.require(2, "Document.elementsFromPoint")?;
    let (x, y) = (a.get(0).to_number()?, a.get(1).to_number()?);
    let page = n.page();
    let ids =
        if n.id == page.document() { page.layout().elements_from_point(&page.dom.borrow(), x, y) } else { Vec::new() };
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
            let v = page.layout().computed_style_value(&page.dom.borrow(), c.el.id, &name);
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
            let v = page.layout().computed_style_value(&page.dom.borrow(), c.el.id, &prop);
            Ok(Some(s.string(&v)))
        });
    }
}

/// `window.getComputedStyle(el)`.
pub fn get_computed_style<'s>(s: &Scope<'s>, page: &Rc<PageDom>, el: NodeId) -> JsResult<Object<'s>> {
    s.wrap::<ComputedStyle>(ComputedData { el: crate::collections::NodeHold::new(page, el)? }, None)
}

/// A live `MediaQueryList` (`PageDom::media_queries`): its target, query
/// text and the `matches` it last reported, so an emulation change fires
/// `change` only on the lists it flipped.
pub struct MediaQueryEntry {
    pub(crate) target: lp_dom::events::TargetKey,
    pub(crate) query: String,
    pub(crate) last: std::cell::Cell<bool>,
}

/// `MediaQueryList`: an EventTarget whose `change` fires when the embedder's
/// viewport or media emulation flips `matches`. `matchMedia` returns one
/// object per realm and query text, kept alive with the realm (as a Window
/// [SameObject]) so a listener added without keeping a reference still runs,
/// as in Chromium, where a list with listeners is not collected.
pub struct MqlData {
    target: crate::target::EventTargetData,
    query: String,
}

impl Drop for MqlData {
    fn drop(&mut self) {
        let key = self.target.key;
        if let Ok(mut lists) = self.target.page.media_queries.try_borrow_mut() {
            lists.retain(|e| e.target != key);
        }
    }
}

pub struct MediaQueryList;
impl JsClass for MediaQueryList {
    const NAME: &'static str = "MediaQueryList";
    type Data = MqlData;
    type Parent = crate::target::EventTarget;
    fn define(def: &mut ClassDef<Self>) {
        def.upcast(|d: &MqlData| &d.target);
        def.getter("media", |s, m| Ok(s.string(&m.query)));
        def.getter("matches", |s, m| Ok(s.boolean(m.target.page.layout().style().match_media(&m.query))));
        // Legacy aliases of addEventListener/removeEventListener('change').
        def.method("addListener", 1, |s, m, a| mql_listener(s, m, "addEventListener", a.get(0)));
        def.method("removeListener", 1, |s, m, a| mql_listener(s, m, "removeEventListener", a.get(0)));
        def.accessor(
            "onchange",
            |s, m| crate::target::event_handler(s, &m.target, "change"),
            |s, m, v| crate::target::set_event_handler(s, &m.target, "change", v),
        );
    }
}

fn mql_listener<'s>(s: &Scope<'s>, m: &MqlData, method: &str, listener: Value<'s>) -> JsResult<Value<'s>> {
    let object = crate::target::target_object(s, &m.target.page, m.target.key)?
        .ok_or_else(|| lp_js::JsError::Type("Illegal invocation".into()))?;
    let f = object.get(method)?.as_object().filter(|f| f.is_function());
    if let Some(f) = f {
        f.call(Some(object), &[s.string("change"), listener])?;
    }
    Ok(s.undefined())
}

fn media_query_identity(page: &PageDom, query: &str) -> lp_js::IdentityKey {
    use std::hash::{Hash, Hasher};
    let mut hasher = rustc_hash::FxHasher::default();
    query.hash(&mut hasher);
    lp_js::IdentityKey(page.id() | (1 << 54), hasher.finish())
}

/// `window.matchMedia(query)`.
pub fn match_media<'s>(s: &Scope<'s>, page: &Rc<PageDom>, query: String) -> JsResult<Object<'s>> {
    let key = media_query_identity(page, &query);
    let value = crate::window::same_object(s, page, key, || {
        let target = crate::target::EventTargetData::new(page);
        let last = page.layout().style().match_media(&query);
        page.media_queries.borrow_mut().push(MediaQueryEntry {
            target: target.key,
            query: query.clone(),
            last: std::cell::Cell::new(last),
        });
        s.wrap::<MediaQueryList>(MqlData { target, query: query.clone() }, Some(key))
    })?;
    Ok(value.as_object().expect("a MediaQueryList wrapper"))
}

/// This realm's `MediaQueryList` owning target `key`, if it is one.
pub(crate) fn media_query_object<'s>(
    s: &Scope<'s>,
    page: &PageDom,
    key: lp_dom::events::TargetKey,
) -> Option<Object<'s>> {
    let query = page.media_queries.borrow().iter().find(|e| e.target == key).map(|e| e.query.clone())?;
    let object = s.lookup_identity(media_query_identity(page, &query))?;
    let owns = s.unwrap::<MediaQueryList>(object.value()).map(|d| d.target.key);
    (owns == Some(key)).then_some(object)
}

pub(crate) fn register(engine: &lp_js::Engine) {
    engine.register::<DomRect>();
    engine.register::<ComputedStyle>();
    engine.register::<MediaQueryList>();
}
