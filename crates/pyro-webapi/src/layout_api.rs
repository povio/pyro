//! Bindings over pyro-dom's faux layout (`pyro_dom::layout`): rects, metrics,
//! scrolling, hit testing, getComputedStyle, matchMedia and innerText.

use std::rc::Rc;

use pyro_dom::layout::Metric;
use pyro_dom::style::CheckVisibilityOptions;
use pyro_dom::NodeId;
use pyro_js::{Args, ClassDef, JsClass, JsResult, NoParent, Object, Scope, Value};

use crate::node::{wrap_node, NodeData};
use crate::page::PageDom;
use crate::util::*;

/// `DOMRect` / `DOMRectReadOnly` values as a plain JS object with the
/// DOMRect shape (x, y, width, height, top, right, bottom, left, toJSON).
pub fn rect_value<'s>(s: &Scope<'s>, r: pyro_dom::layout::Rect) -> JsResult<Value<'s>> {
    let o = s.wrap::<DomRect>(r, None)?;
    Ok(o.value())
}

pub struct DomRect;
impl JsClass for DomRect {
    const NAME: &'static str = "DOMRect";
    type Data = pyro_dom::layout::Rect;
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
            let r = pyro_dom::layout::Rect { x: n(0)?, y: n(1)?, width: n(2)?, height: n(3)? };
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
    crate::animation_events::layout_flush(page, n.id);
    let r = page.layout().bounding_client_rect(&page.dom.borrow(), n.id);
    rect_value(s, r.layout_units())
}

pub(crate) fn client_rects<'s>(s: &Scope<'s>, n: &NodeData) -> JsResult<Value<'s>> {
    let page = n.page();
    crate::animation_events::layout_flush(page, n.id);
    let rects = page.layout().client_rects(&page.dom.borrow(), n.id);
    let mut vals = pyro_js::RootedVec::new();
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
        if dom.node_type(start).js()? == pyro_dom::NodeType::Text {
            Some(start)
        } else if start == end && end_offset == start_offset + 1 {
            dom.children(start)
                .js()?
                .nth(start_offset as usize)
                .filter(|&c| dom.node_type(c).ok() == Some(pyro_dom::NodeType::Text))
        } else {
            None
        }
    };
    if let Some(t) = text {
        if let Some(r) = page.layout().text_rect(&page.dom.borrow(), t) {
            return rect_value(s, r.layout_units());
        }
    }
    // A range over the children of one node (`selectNodeContents`,
    // `selectNode`): the union of their boxes, as Chromium unites the
    // selected nodes' rects (a `display: contents` child through its own
    // children; empty rects add nothing, none at all is an empty rect).
    if start == end && start_offset < end_offset {
        let dom = page.dom.borrow();
        if matches!(dom.node_type(start).js()?, pyro_dom::NodeType::Element | pyro_dom::NodeType::DocumentFragment) {
            let layout = page.layout();
            let children: Vec<NodeId> = dom
                .children(start)
                .js()?
                .skip(start_offset as usize)
                .take((end_offset - start_offset) as usize)
                .collect();
            let mut union = None;
            for c in children {
                unite_node_rects(&dom, &layout, c, &mut union);
            }
            return rect_value(s, union.unwrap_or_default().layout_units());
        }
    }
    let el = {
        let dom = page.dom.borrow();
        if dom.node_type(start).js()? == pyro_dom::NodeType::Element {
            Some(start)
        } else {
            dom.parent_element(start).js()?
        }
    };
    let r = match el {
        Some(e) => page.layout().bounding_client_rect(&page.dom.borrow(), e),
        None => pyro_dom::layout::Rect::default(),
    };
    rect_value(s, r.layout_units())
}

/// Adds `node`'s rect to `union`: a text node's line box, an element's
/// border box, a `display: contents` element's children's.
fn unite_node_rects(
    dom: &pyro_dom::Dom,
    layout: &pyro_dom::layout::Layout,
    node: NodeId,
    union: &mut Option<pyro_dom::layout::Rect>,
) {
    let rect = match dom.node_type(node) {
        Ok(pyro_dom::NodeType::Text) => layout.text_rect(dom, node),
        Ok(pyro_dom::NodeType::Element) => {
            if layout.computed_style_value(dom, node, "display") == "contents" {
                for c in dom.children(node).into_iter().flatten() {
                    unite_node_rects(dom, layout, c, union);
                }
                return;
            }
            layout.client_rect(dom, node)
        }
        _ => None,
    };
    // Blink's rect union skips empty rects.
    let Some(r) = rect.filter(|r| r.width > 0.0 && r.height > 0.0) else { return };
    *union = Some(match *union {
        None => r,
        Some(u) => {
            let (x0, y0) = (u.x.min(r.x), u.y.min(r.y));
            let (x1, y1) = ((u.x + u.width).max(r.x + r.width), (u.y + u.height).max(r.y + r.height));
            pyro_dom::layout::Rect { x: x0, y: y0, width: x1 - x0, height: y1 - y0 }
        }
    });
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

/// The scroll steps of a rendering update that runs ahead of the frame
/// interval ([`PageDom::request_rendering_update`]): the re-clamp a DOM
/// change owes, then the queued scroll events.
pub(crate) fn run_scroll_steps(page: &Rc<PageDom>) {
    if page.is_disposed() {
        return;
    }
    let (checked, queued) = page.scroll_clamp.get();
    if queued {
        // The delayed task that was to do it finds nothing owed.
        page.scroll_clamp.set((checked, false));
        clamp_scrolls(page, true);
    }
    flush_pending_scrolls(page);
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
            pyro_dom::layout::ScrollTarget::Container(node) => Some(node),
            pyro_dom::layout::ScrollTarget::Viewport => None,
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
            // Not owed any more when a rendering update ran ahead of it.
            let (checked, queued) = p.scroll_clamp.get();
            p.scroll_clamp.set((checked, false));
            if queued && !p.is_disposed() {
                clamp_scrolls(&p, true);
            }
        }),
    );
}

/// `scrollIntoView(arg)`: `(boolean or ScrollIntoViewOptions)`; `true`, a
/// missing argument and `{}` align block start / inline nearest, `false`
/// block end / inline nearest. `behavior` is accepted; scrolling is instant.
pub(crate) fn scroll_into_view<'s>(s: &Scope<'s>, n: &NodeData, a: Args<'s>) -> JsResult<Value<'s>> {
    use pyro_dom::layout::ScrollAlign;
    let arg = a.get(0);
    let (mut block, mut inline) = (ScrollAlign::Start, ScrollAlign::Nearest);
    if let Some(o) = arg.as_object() {
        // Dictionary members in lexicographic order: behavior, block, inline.
        let behavior = o.get("behavior")?;
        if !behavior.is_undefined() {
            let b = dom_string(behavior)?;
            if !matches!(b.as_str(), "auto" | "instant" | "smooth") {
                return Err(pyro_js::JsError::type_error(format!(
                    "Failed to execute 'scrollIntoView' on 'Element': The provided value '{b}' is not a valid enum value of type ScrollBehavior."
                )));
            }
        }
        let position = |k: &str, out: &mut ScrollAlign| -> JsResult<()> {
            let v = o.get(k)?;
            if v.is_undefined() {
                return Ok(());
            }
            let text = dom_string(v)?;
            *out = ScrollAlign::parse(&text).ok_or_else(|| {
                pyro_js::JsError::type_error(format!(
                    "Failed to execute 'scrollIntoView' on 'Element': The provided value '{text}' is not a valid enum value of type ScrollLogicalPosition."
                ))
            })?;
            Ok(())
        };
        position("block", &mut block)?;
        position("inline", &mut inline)?;
    } else if !arg.is_undefined() && !arg.is_null() && !arg.to_bool() {
        block = ScrollAlign::End;
    }
    let page = n.page();
    let moved =
        page.layout.borrow_mut().scroll_into_view_aligned_script_targets(&page.dom.borrow(), n.id, block, inline);
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

/// HTML "scroll to the fragment", after a same-document navigation to `fragment` (the URL's
/// fragment, percent-encoded): the indicated part of the document is scrolled into view (block
/// start, inline nearest), or the top of the document for an empty fragment or `top`. Nothing
/// moves when no element is indicated.
pub fn scroll_to_fragment(page: &Rc<PageDom>, fragment: &str) {
    use pyro_dom::layout::ScrollAlign;
    let doc = page.document();
    let decoded = percent_encoding::percent_decode_str(fragment).decode_utf8_lossy().into_owned();
    crate::animation_events::layout_flush(page, doc);
    let target = {
        let dom = page.dom.borrow();
        let by_id = |id: &str| (!id.is_empty()).then(|| dom.get_element_by_id(doc, id).ok().flatten()).flatten();
        by_id(fragment).or_else(|| by_id(&decoded)).or_else(|| {
            // The first `a` element whose `name` is the fragment.
            dom.descendants(doc).ok().into_iter().flatten().find(|&n| {
                dom.element(n).is_ok_and(|e| &*e.local_name() == "a")
                    && dom.get_attribute(n, "name").ok().flatten().is_some_and(|name| name == fragment || name == decoded)
            })
        })
    };
    match target {
        Some(el) => {
            let moved = page.layout.borrow_mut().scroll_into_view_aligned_script_targets(
                &page.dom.borrow(),
                el,
                ScrollAlign::Start,
                ScrollAlign::Nearest,
            );
            fire_scroll_targets(page, moved);
        }
        None if fragment.is_empty() || decoded.eq_ignore_ascii_case("top") => {
            let moved = page.layout.borrow_mut().window_scroll_to(&page.dom.borrow(), Some(0), Some(0));
            if moved {
                fire_scroll(page, None);
            }
        }
        None => {}
    }
}

fn fire_scroll_targets(page: &Rc<PageDom>, moved: Vec<pyro_dom::layout::ScrollTarget>) {
    for target in moved {
        fire_scroll(
            page,
            match target {
                pyro_dom::layout::ScrollTarget::Container(node) => Some(node),
                pyro_dom::layout::ScrollTarget::Viewport => None,
            },
        );
    }
}

/// `checkVisibility(options)`: `checkOpacity` / `opacityProperty` add the
/// opacity:0 test, `checkVisibilityCSS` / `visibilityProperty` the
/// visibility test (CSSOM View, CheckVisibilityOptions).
pub(crate) fn check_visibility<'s>(s: &Scope<'s>, n: &NodeData, a: Args<'s>) -> JsResult<Value<'s>> {
    let mut opts = CheckVisibilityOptions::default();
    let arg = a.get(0);
    if !arg.is_undefined() && !arg.is_null() {
        let o = arg.as_object().ok_or_else(|| {
            pyro_js::JsError::type_error("Failed to execute 'checkVisibility': options is not an object.")
        })?;
        // Dictionary members are read in lexicographic order.
        let flag = |k: &str| o.get(k).map(|v| v.to_bool());
        let check_opacity = flag("checkOpacity")?;
        let check_visibility_css = flag("checkVisibilityCSS")?;
        let opacity_property = flag("opacityProperty")?;
        let visibility_property = flag("visibilityProperty")?;
        opts.check_opacity = check_opacity || opacity_property;
        opts.check_visibility = check_visibility_css || visibility_property;
    }
    let page = n.page();
    let v = page.layout().check_visibility(&page.dom.borrow(), n.id, opts);
    Ok(s.boolean(v))
}

fn scroll_args(a: &Args<'_>) -> JsResult<(Option<i32>, Option<i32>)> {
    if let Some(o) = a.get(0).as_object() {
        let l = o.get_atom("left")?;
        let t = o.get_atom("top")?;
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

/// CSSOM View: whether `el`'s scroll offsets are the viewport's, so that its
/// `scrollTop`, `scrollLeft` and `scroll*()` read and move the window: the
/// page document's `scrollingElement` (the document element, or the body
/// element in quirks mode).
fn scrolls_viewport(page: &PageDom, el: NodeId) -> bool {
    let dom = page.dom.borrow();
    let doc = page.document();
    if dom.node_document(el).ok() != Some(doc) {
        return false;
    }
    let quirks = dom.document_data(doc).is_ok_and(|d| d.quirks == pyro_dom::QuirksMode::Quirks);
    let scrolling = if quirks { dom.body(doc) } else { dom.document_element(doc) };
    scrolling.ok().flatten() == Some(el)
}

pub(crate) fn scroll_to<'s>(s: &Scope<'s>, n: &NodeData, a: Args<'s>, by: bool) -> JsResult<Value<'s>> {
    let (l, t) = scroll_args(&a)?;
    let page = n.page();
    let viewport = scrolls_viewport(page, n.id);
    let moved = {
        let dom = page.dom.borrow();
        let mut layout = page.layout.borrow_mut();
        match (viewport, by) {
            (true, true) => layout.window_scroll_by(&dom, l, t),
            (true, false) => layout.window_scroll_to(&dom, l, t),
            (false, true) => layout.element_scroll_by(&dom, n.id, l, t),
            (false, false) => layout.element_scroll_to(&dom, n.id, l, t),
        }
    };
    if moved {
        fire_scroll(page, (!viewport).then_some(n.id));
    }
    Ok(s.undefined())
}

pub(crate) fn scroll_get<'s>(s: &Scope<'s>, n: &NodeData, top: bool) -> JsResult<Value<'s>> {
    // Reading a scroll offset sees the current layout (clamped to it).
    crate::animation_events::layout_flush(n.page(), n.id);
    clamp_scrolls(n.page(), false);
    let viewport = scrolls_viewport(n.page(), n.id);
    let layout = n.page().layout();
    Ok(s.number(match (viewport, top) {
        (true, true) => layout.scroll_y() as f64,
        (true, false) => layout.scroll_x() as f64,
        // Negative on an axis that scrolls from its end (right-to-left, reversed flex).
        (false, true) => layout.scroll_top(n.id) as f64,
        (false, false) => layout.scroll_left(n.id) as f64,
    }))
}

pub(crate) fn scroll_set(_s: &Scope<'_>, n: &NodeData, v: Value<'_>, top: bool) -> JsResult<()> {
    let value = to_scroll_offset(v)?;
    let page = n.page();
    let viewport = scrolls_viewport(page, n.id);
    let moved = {
        let dom = page.dom.borrow();
        let mut layout = page.layout.borrow_mut();
        match (viewport, top) {
            (true, true) => layout.window_scroll_to(&dom, None, Some(value)),
            (true, false) => layout.window_scroll_to(&dom, Some(value), None),
            (false, true) => layout.set_scroll_top(&dom, n.id, value),
            (false, false) => layout.set_scroll_left(&dom, n.id, value),
        }
    };
    if moved {
        fire_scroll(page, (!viewport).then_some(n.id));
    }
    Ok(())
}

pub(crate) fn metric<'s>(s: &Scope<'s>, n: &NodeData, which: &str) -> JsResult<Value<'s>> {
    let page = n.page();
    crate::animation_events::layout_flush(page, n.id);
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

pub(crate) fn set_inner_text(s: &Scope<'_>, n: &NodeData, v: Value<'_>) -> JsResult<()> {
    let text = if crate::trusted_types::is_html_script(n.page(), n.id) {
        crate::trusted_types::compliant_string(s, n.page(), crate::trusted_types::Kind::Script, v, "HTMLScriptElement innerText", true)?
    } else {
        null_to_empty(v)?
    };
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
    // Retargeted against the document: a hit inside a shadow tree is its
    // outermost host (CDP's `hit_test` stays un-retargeted, as Chrome's).
    let hit = hit_test(page, x, y).and_then(|h| page.dom.borrow().retarget_node(h, n.id));
    opt_node(s, page, hit)
}

/// `ShadowRoot.elementFromPoint`: the document's hit, retargeted against
/// the shadow root (a host's for anything deeper, null outside its tree).
pub(crate) fn shadow_element_from_point<'s>(s: &Scope<'s>, n: &NodeData, a: Args<'s>) -> JsResult<Value<'s>> {
    a.require(2, "ShadowRoot.elementFromPoint")?;
    let (x, y) = (a.get(0).to_number()?, a.get(1).to_number()?);
    let page = n.page();
    let hit = hit_test(page, x, y);
    let retargeted = hit.and_then(|h| page.dom.borrow().retarget_node(h, n.id));
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
            if let Some(r) = dom.retarget_node(id, n.id) {
                if out.last() != Some(&r) {
                    out.push(r);
                }
            }
        }
    }
    let mut vals = pyro_js::RootedVec::new();
    for id in out {
        vals.push(s, wrap_node(s, page, id)?.value());
    }
    Ok(s.array(&vals.iter(s).collect::<Vec<_>>())?.value())
}

pub(crate) fn elements_from_point<'s>(s: &Scope<'s>, n: &NodeData, a: Args<'s>) -> JsResult<Value<'s>> {
    a.require(2, "Document.elementsFromPoint")?;
    let (x, y) = (a.get(0).to_number()?, a.get(1).to_number()?);
    let page = n.page();
    let mut ids =
        if n.id == page.document() { page.layout().elements_from_point(&page.dom.borrow(), x, y) } else { Vec::new() };
    {
        // Retarget each hit against the document, dropping the repeats a
        // shadow tree's nodes collapse to (its host, once).
        let dom = page.dom.borrow();
        let mut out: Vec<NodeId> = Vec::with_capacity(ids.len());
        for id in ids.drain(..) {
            if let Some(r) = dom.retarget_node(id, n.id) {
                if !out.contains(&r) {
                    out.push(r);
                }
            }
        }
        ids = out;
    }
    let mut vals = pyro_js::RootedVec::new();
    for id in ids {
        vals.push(s, wrap_node(s, page, id)?.value());
    }
    Ok(s.array(&vals.iter(s).collect::<Vec<_>>())?.value())
}

/// Read-only computed style (`getComputedStyle`).
pub struct ComputedData {
    el: crate::collections::NodeHold,
    /// The `pseudoElt` argument, parsed.
    pseudo: pyro_dom::stylo::PseudoArg,
}

impl ComputedData {
    /// The computed value of `name` for the element or its pseudo-element.
    fn value(&self, name: &str) -> String {
        use pyro_dom::stylo::PseudoArg;
        let page = &self.el.page;
        match &self.pseudo {
            PseudoArg::Element => {
                crate::animation_events::observe(page, self.el.id);
                page.layout().computed_style_value(&page.dom.borrow(), self.el.id, name)
            }
            PseudoArg::Pseudo(pseudo) => {
                // A style flush too: the pseudo-element's transitions start at it.
                crate::animation_events::observe(page, self.el.id);
                page.layout().computed_pseudo_style_value(&page.dom.borrow(), self.el.id, pseudo, name)
            }
            PseudoArg::Invalid => String::new(),
        }
    }
}

/// A computed value as a JavaScript string: the realm's one string for a
/// keyword (`block`, `none`, `auto`: the same few dozen texts come back for
/// every element), a new string for anything else (lengths, colours, lists:
/// mostly distinct, where a shared string would cost more than it saves).
fn computed_string<'s>(s: &Scope<'s>, value: &str) -> Value<'s> {
    if !value.is_empty() && value.len() <= 24 && value.bytes().all(|b| b.is_ascii_lowercase() || b == b'-') {
        s.text_string(value)
    } else {
        s.string(value)
    }
}

/// A computed style's CSS properties, one getter each: Chrome's names
/// ([`crate::computed_properties`]), then its `epub*` aliases. Each entry is
/// the property whose value the getter reads.
fn computed_property_names() -> &'static [&'static str] {
    static NAMES: std::sync::OnceLock<Vec<&'static str>> = std::sync::OnceLock::new();
    NAMES.get_or_init(|| {
        let names = crate::computed_properties::COMPUTED_PROPERTIES.iter().copied();
        names.chain(crate::computed_properties::EPUB_ALIASES.iter().map(|&(_, property)| property)).collect()
    })
}

/// Getter of the property `computed_property_names()[I]` (under each of its
/// names): an attribute of `CSSStyleDeclaration.prototype`, read through the
/// inline cache like any other, with the property chosen when the class is
/// defined. The getters are made by index, [`MAX_PROPERTIES`] of them.
fn property_getter<'s, const I: usize>(s: &Scope<'s>, c: &ComputedData) -> JsResult<Value<'s>> {
    Ok(computed_string(s, &c.value(computed_property_names()[I])))
}

type ComputedGetter = for<'s> fn(&Scope<'s>, &ComputedData) -> JsResult<Value<'s>>;

macro_rules! property_getters_block {
    ($b:literal; $($i:literal),*) => { [ $( property_getter::<{ $b * 16 + $i }> as ComputedGetter ),* ] };
}
macro_rules! property_getters {
    ($($b:literal),*) => {
        [ $( property_getters_block!($b; 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15) ),* ]
    };
}

/// Room for the property list; the class definition asserts that it fits.
const MAX_PROPERTIES: usize = 768;
/// The getters by property index, in blocks of 16.
static PROPERTY_GETTERS: [[ComputedGetter; 16]; MAX_PROPERTIES / 16] = property_getters!(
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30,
    31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47
);

/// The attribute names of each entry of [`computed_property_names`], each
/// with whether `for..in` lists it, as Chrome has them: the camel-cased name
/// (CSSOM "camel-cased attribute": `background-color` -> `backgroundColor`,
/// `-webkit-text-security` -> `webkitTextSecurity`) is listed; the dashed
/// name and, for a `-webkit-` property, the "webkit-cased attribute"
/// (`WebkitTextSecurity`) are found by name only; `float` is also
/// `cssFloat`, an attribute of the interface; the `epub*` aliases are found
/// by name only.
fn property_attribute_names() -> &'static [Vec<(&'static str, bool)>] {
    static NAMES: std::sync::OnceLock<Vec<Vec<(&'static str, bool)>>> = std::sync::OnceLock::new();
    NAMES.get_or_init(|| {
        let camel = |name: &str, upper_first: bool| {
            let mut out = String::with_capacity(name.len());
            let mut upper = upper_first;
            for ch in name.chars() {
                if ch == '-' {
                    upper = upper_first || !out.is_empty();
                } else if upper {
                    out.push(ch.to_ascii_uppercase());
                    upper = false;
                } else {
                    out.push(ch);
                }
            }
            out
        };
        let leak = |n: String| -> &'static str { Box::leak(n.into_boxed_str()) };
        let mut all: Vec<Vec<(&'static str, bool)>> = crate::computed_properties::COMPUTED_PROPERTIES
            .iter()
            .map(|&name| {
                let mut names = Vec::with_capacity(3);
                let lower = camel(name, false);
                if lower == name {
                    names.push((name, true));
                } else {
                    names.push((leak(lower), true));
                    names.push((name, false));
                }
                if name == "float" {
                    names.push(("cssFloat", true));
                }
                if name.starts_with("-webkit-") {
                    names.push((leak(camel(name, true)), false));
                }
                names
            })
            .collect();
        all.extend(crate::computed_properties::EPUB_ALIASES.iter().map(|&(alias, _)| vec![(alias, false)]));
        all
    })
}

/// Every name of [`property_attribute_names`], with its property's index (a
/// name defined twice: the later, as on the prototype).
fn property_index_by_name() -> &'static rustc_hash::FxHashMap<&'static str, usize> {
    static INDEX: std::sync::OnceLock<rustc_hash::FxHashMap<&'static str, usize>> = std::sync::OnceLock::new();
    INDEX.get_or_init(|| {
        let mut index = rustc_hash::FxHashMap::default();
        for (i, names) in property_attribute_names().iter().enumerate() {
            for &(name, _) in names {
                index.insert(name, i);
            }
        }
        index
    })
}

/// Assigning to a computed style's property: Chrome's error, in sloppy code
/// too.
fn computed_read_only<'s>(_s: &Scope<'s>, _c: &ComputedData, _v: Value<'s>) -> JsResult<()> {
    Err(pyro_js::JsError::dom("NoModificationAllowedError", "These styles are computed, and therefore read-only."))
}

pub struct ComputedStyle;
impl JsClass for ComputedStyle {
    // CSSOM: a computed style is a `CSSStyleProperties`.
    const NAME: &'static str = "CSSStyleProperties";
    type Data = ComputedData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.not_exposed();
        // Every property, by its dashed name and its IDL attribute names, as
        // attributes of the prototype (CSSOM `CSSStyleDeclaration`). No named
        // property hook: a class with one sends every property read through
        // it, which defeats the inline cache.
        let names = property_attribute_names();
        assert!(names.len() <= MAX_PROPERTIES, "CSS properties beyond the getter table: raise MAX_PROPERTIES");
        let mut own = Vec::with_capacity(names.len() * 2);
        for (i, names) in names.iter().enumerate() {
            let get = PROPERTY_GETTERS[i / 16][i % 16];
            for &(name, enumerable) in names {
                def.pure_accessor_typed(name, pyro_js::GetterResult::String, get, computed_read_only);
                if !enumerable {
                    def.unenumerable_last_accessor();
                }
                // Chrome has the properties as named properties of the object
                // itself: the camel-cased names listed and enumerable, the
                // dashed and `Webkit` names own but unlisted, the `epub*`
                // aliases listed by getOwnPropertyNames only; `cssFloat` is
                // an attribute of the interface. The values stay with the
                // prototype's accessors, so reads stay inline-cached.
                let how = if name == "cssFloat" {
                    continue;
                } else if crate::computed_properties::EPUB_ALIASES.iter().any(|&(alias, _)| alias == name) {
                    pyro_js::OwnName::LISTED_HIDDEN
                } else if enumerable {
                    pyro_js::OwnName::ENUMERABLE
                } else {
                    pyro_js::OwnName::UNLISTED
                };
                own.push((name, how));
            }
        }
        // In Chrome's order: by code unit.
        own.sort_unstable_by_key(|&(name, _)| name);
        for (name, how) in own {
            def.own_property_name(name, how);
        }
        // An own name's value (a descriptor, spread, `Object.entries`) is
        // the property's, as Chrome's named property is, not whatever the
        // prototype chain holds after `Object.setPrototypeOf`.
        def.own_property_value(|s, c, name| {
            let Some(&i) = property_index_by_name().get(name) else { return Ok(None) };
            Ok(Some(PROPERTY_GETTERS[i / 16][i % 16](s, c)?))
        });
        def.method("getPropertyValue", 1, |s, c, a| {
            let name = arg_string(&a, 0, "CSSStyleDeclaration.getPropertyValue")?;
            Ok(computed_string(s, &c.value(&name)))
        });
        def.method("getPropertyPriority", 1, |s, _c, _a| Ok(s.string("")));
        // The longhands, as indexed properties; an element without computed
        // style (not rendered) lists nothing, as in Chrome.
        def.method("item", 1, |s, c, a| {
            let names = computed_names(c);
            Ok(s.string(names.get(to_u32(a.get(0))? as usize).copied().unwrap_or("")))
        });
        def.getter("length", |s, c| Ok(s.number(computed_names(c).len() as f64)));
        def.indexed(|s, c, i| Ok(computed_names(c).get(i as usize).map(|name| s.string(name))));
        def.indexed_iterator();
        def.getter("cssText", |s, _c| Ok(s.string("")));
        def.getter("parentRule", |s, _c| Ok(s.null()));
        def.method("setProperty", 2, |_s, _c, _a| {
            Err::<Value, _>(pyro_js::JsError::dom(
                "NoModificationAllowedError",
                "These styles are computed, and therefore read-only.",
            ))
        });
        def.method("removeProperty", 1, |_s, _c, a| {
            let name = arg_string(&a, 0, "CSSStyleDeclaration.removeProperty")?;
            Err::<Value, _>(pyro_js::JsError::dom(
                "NoModificationAllowedError",
                format!("These styles are computed, and therefore the '{name}' property is read-only."),
            ))
        });
    }
}

/// The property names a computed style lists.
fn computed_names(c: &ComputedData) -> &'static [&'static str] {
    if c.pseudo == pyro_dom::stylo::PseudoArg::Invalid {
        return &[];
    }
    let page = &c.el.page;
    let dom = page.dom.borrow();
    if page.layout().computed_style_value(&dom, c.el.id, "display").is_empty() {
        return &[];
    }
    pyro_dom::stylo::longhand_names()
}

/// `window.getComputedStyle(el, pseudoElt)`.
pub fn get_computed_style<'s>(
    s: &Scope<'s>,
    page: &Rc<PageDom>,
    el: NodeId,
    pseudo_elt: Value<'s>,
) -> JsResult<Object<'s>> {
    let pseudo = if pseudo_elt.is_undefined() || pseudo_elt.is_null() {
        pyro_dom::stylo::PseudoArg::Element
    } else {
        pyro_dom::stylo::parse_pseudo_arg(&dom_string(pseudo_elt)?)
    };
    s.wrap::<ComputedStyle>(ComputedData { el: crate::collections::NodeHold::new(page, el)?, pseudo }, None)
}

/// A live `MediaQueryList` (`PageDom::media_queries`): its target, query
/// text and the `matches` it last reported, so an emulation change fires
/// `change` only on the lists it flipped.
pub struct MediaQueryEntry {
    pub(crate) target: pyro_dom::events::TargetKey,
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
        .ok_or_else(|| pyro_js::JsError::Type("Illegal invocation".into()))?;
    let f = object.get(method)?.as_object().filter(|f| f.is_function());
    if let Some(f) = f {
        f.call(Some(object), &[s.string("change"), listener])?;
    }
    Ok(s.undefined())
}

fn media_query_identity(page: &PageDom, query: &str) -> pyro_js::IdentityKey {
    use std::hash::{Hash, Hasher};
    let mut hasher = rustc_hash::FxHasher::default();
    query.hash(&mut hasher);
    pyro_js::IdentityKey(page.id() | (1 << 54), hasher.finish())
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
    key: pyro_dom::events::TargetKey,
) -> Option<Object<'s>> {
    let query = page.media_queries.borrow().iter().find(|e| e.target == key).map(|e| e.query.clone())?;
    let object = s.lookup_identity(media_query_identity(page, &query))?;
    let owns = s.unwrap::<MediaQueryList>(object.value()).map(|d| d.target.key);
    (owns == Some(key)).then_some(object)
}

pub(crate) fn register(engine: &pyro_js::Engine) {
    engine.register::<DomRect>();
    engine.register::<ComputedStyle>();
    engine.register::<MediaQueryList>();
}
