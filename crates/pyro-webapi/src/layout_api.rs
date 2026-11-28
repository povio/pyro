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

/// The page whose document `n` belongs to now: an element created by one document and adopted by
/// another (a frame's, or the page's) keeps the wrapper of the page it was created in, whose layout
/// does not hold it, so its geometry reads (`getBoundingClientRect`, `offsetWidth`, ...) are the
/// current owner's (WPT pointerevents/pointerevent_pointerrawupdate_after_adoption_from_child).
fn owner_page(n: &NodeData) -> Rc<PageDom> {
    crate::input::sibling_page_of(n.page(), n.id).unwrap_or_else(|| n.page().clone())
}

pub(crate) fn bounding_rect<'s>(s: &Scope<'s>, n: &NodeData) -> JsResult<Value<'s>> {
    let (page, owner) = (n.page(), &owner_page(n));
    crate::animation_events::layout_flush(owner, n.id);
    let r = owner.layout().bounding_client_rect(&owner.dom.borrow(), n.id);
    // HTML-in-Canvas: a drawable element's canvas transform places it (none: as laid out).
    let r = crate::html_in_canvas::transformed_rect(owner, n.id, r);
    crate::canvas::rect_value(s, page, false, r.layout_units())
}

pub(crate) fn client_rects<'s>(s: &Scope<'s>, n: &NodeData) -> JsResult<Value<'s>> {
    let (page, owner) = (n.page(), &owner_page(n));
    crate::animation_events::layout_flush(owner, n.id);
    let rects = owner.layout().client_rects(&owner.dom.borrow(), n.id);
    let mut vals = pyro_js::RootedVec::new();
    for r in rects {
        vals.push(s, crate::canvas::rect_value(s, page, false, r.layout_units())?);
    }
    crate::canvas::rect_list(s, page, &vals.iter(s).collect::<Vec<_>>())
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
        if t == start && start == end {
            if let Some(rect) = page.layout().text_range_rect(&page.dom.borrow(), t, start_offset, end_offset) {
                return crate::canvas::rect_value(s, page, false, rect.layout_units());
            }
        }
        if let Some(r) = page.layout().text_rect(&page.dom.borrow(), t) {
            return crate::canvas::rect_value(s, page, false, r.layout_units());
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
            return crate::canvas::rect_value(s, page, false, union.unwrap_or_default().layout_units());
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
    crate::canvas::rect_value(s, page, false, r.layout_units())
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
/// viewport), followed by its `scrollend`. The scroll interrupts a smooth scroll of the box.
pub fn fire_scroll(page: &Rc<PageDom>, target: Option<NodeId>) {
    crate::smooth_scroll::cancel(page, target);
    queue_scroll(page, target, true);
}

/// A ScrollBehavior (`ScrollOptions.behavior`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ScrollBehavior {
    /// The scrolling box's `scroll-behavior` decides.
    Auto,
    Instant,
    Smooth,
}

/// A scroll write of script (a scroll method, a `scrollTop`/`scrollLeft` setter,
/// `scrollIntoView`, a fragment scroll): `boxes` (`None`: the viewport) are the boxes it scrolls
/// whether or not they move, whose smooth scrolls it interrupts; `write` scrolls instantly and
/// returns the boxes it moved. A moved box that scrolls smoothly (`behavior`, else its
/// `scroll-behavior`) goes back where it was and smooth-scrolls to where the write put it
/// (snapped there in the write's direction, [`crate::smooth_scroll`]); another fires its scroll
/// events. Returns what waits for the smooth scrolls it started, if any.
pub(crate) fn perform_scroll(
    page: &Rc<PageDom>,
    behavior: ScrollBehavior,
    boxes: &[Option<NodeId>],
    write: impl FnOnce(&pyro_dom::Dom, &mut pyro_dom::layout::Layout) -> Vec<Option<NodeId>>,
) -> Option<Rc<crate::smooth_scroll::Waiter>> {
    // A smooth scroll may head where one already running on a box goes: that no-op leaves the
    // running one alone, so those boxes are not stopped before the scroll is known.
    let running: Vec<Option<NodeId>> = boxes
        .iter()
        .copied()
        .filter(|&t| crate::smooth_scroll::running_to(page, t).is_some())
        .filter(|&t| match behavior {
            ScrollBehavior::Smooth => true,
            ScrollBehavior::Instant => false,
            ScrollBehavior::Auto => crate::smooth_scroll::css_smooth(page, t),
        })
        .collect();
    for &target in boxes {
        if !running.contains(&target) {
            crate::smooth_scroll::cancel(page, target);
        }
    }
    let before = (behavior != ScrollBehavior::Instant).then(|| page.layout.borrow().scroll_positions());
    let moved = {
        let dom = page.dom.borrow();
        let mut layout = page.layout.borrow_mut();
        write(&*dom, &mut *layout)
    };
    let mut waiter: Option<Rc<crate::smooth_scroll::Waiter>> = None;
    let moved_targets = moved.clone();
    for target in moved {
        let smooth = match behavior {
            ScrollBehavior::Smooth => true,
            ScrollBehavior::Instant => false,
            ScrollBehavior::Auto => crate::smooth_scroll::css_smooth(page, target),
        };
        let from = before
            .as_ref()
            .filter(|_| smooth)
            .map(|before| before.iter().find(|(t, _)| *t == target).map_or((0, 0), |&(_, pos)| pos));
        let Some(from) = from else {
            fire_scroll(page, target);
            continue;
        };
        let to = {
            let dom = page.dom.borrow();
            let mut layout = page.layout.borrow_mut();
            let to = layout.scroll_position(target);
            layout.set_scroll_position(&dom, target, from);
            to
        };
        let w = waiter.get_or_insert_with(crate::smooth_scroll::Waiter::new);
        if running.contains(&target) && crate::smooth_scroll::running_to(page, target) == Some(to) && crate::smooth_scroll::join(page, target, w) {
            continue;
        }
        crate::smooth_scroll::start(page, target, from, to, w);
    }
    // A box this scroll did not move (or none) still stops the smooth scroll running on it.
    for &target in &running {
        if !moved_targets.contains(&target) {
            crate::smooth_scroll::cancel(page, target);
        }
    }
    waiter
}

/// The promise a scroll method returns (Chrome's `Promise<ScrollResult>`; WPT cssom-view
/// `*-scroll-promises`, `*-scroll-promise-interruption`), fulfilled with `{interrupted}`: at once
/// for an instant scroll, else when the smooth scrolls `waiter` waits for end, `interrupted: true`
/// when another scroll of one of their boxes cut it short.
fn scroll_promise<'s>(
    s: &Scope<'s>,
    page: &Rc<PageDom>,
    waiter: Option<Rc<crate::smooth_scroll::Waiter>>,
) -> JsResult<Value<'s>> {
    let deferred = s.deferred()?;
    let promise = deferred.promise.get(s);
    match waiter {
        Some(waiter) => crate::smooth_scroll::attach(page, &waiter, s.realm().downgrade(), deferred),
        None => {
            let o = s.object();
            o.set_atom("interrupted", s.boolean(false))?;
            deferred.resolve(s, o.value())?;
        }
    }
    Ok(promise)
}

/// The `behavior` member of a ScrollOptions dictionary `o` (a ScrollBehavior: "auto", "instant"
/// or "smooth"); another value is a TypeError.
fn scroll_behavior(o: &Object<'_>, operation: &str, interface: &str) -> JsResult<ScrollBehavior> {
    let behavior = o.get("behavior")?;
    if behavior.is_undefined() {
        return Ok(ScrollBehavior::Auto);
    }
    let b = dom_string(behavior)?;
    match b.as_str() {
        "smooth" => Ok(ScrollBehavior::Smooth),
        "instant" => Ok(ScrollBehavior::Instant),
        "auto" => Ok(ScrollBehavior::Auto),
        _ => Err(pyro_js::JsError::type_error(format!(
            "Failed to execute '{operation}' on '{interface}': The provided value '{b}' is not a valid enum value of type ScrollBehavior."
        ))),
    }
}

/// The arguments of `scroll()`, `scrollTo()` and `scrollBy()` (`optional ScrollToOptions` or
/// `unrestricted double x, unrestricted double y`): left, top and the scroll's behavior.
/// A single argument converts to the dictionary, so a number there is a TypeError.
pub(crate) fn scroll_options<T>(
    a: &Args<'_>,
    operation: &str,
    interface: &str,
    number: impl Fn(Value<'_>) -> JsResult<T>,
) -> JsResult<(Option<T>, Option<T>, ScrollBehavior)> {
    let arg = a.get(0);
    if let Some(o) = arg.as_object() {
        // Dictionary members in order: ScrollOptions' behavior, then left, top.
        let behavior = scroll_behavior(&o, operation, interface)?;
        let l = o.get_atom("left")?;
        let t = o.get_atom("top")?;
        Ok((
            (!l.is_undefined()).then(|| number(l)).transpose()?,
            (!t.is_undefined()).then(|| number(t)).transpose()?,
            behavior,
        ))
    } else if a.len() >= 2 {
        Ok((Some(number(a.get(0))?), Some(number(a.get(1))?), ScrollBehavior::Auto))
    } else if arg.is_undefined() || arg.is_null() {
        Ok((None, None, ScrollBehavior::Auto))
    } else {
        Err(pyro_js::JsError::type_error(format!(
            "Failed to execute '{operation}' on '{interface}': The provided value is not of type 'ScrollToOptions'."
        )))
    }
}

/// `Window.scroll()`, `scrollTo()` and `scrollBy()` (`by`) to `l`, `t`: returns their scroll
/// promise ([`scroll_promise`]).
pub(crate) fn window_scroll<'s>(
    s: &Scope<'s>,
    page: &Rc<PageDom>,
    (l, t, behavior): (Option<i32>, Option<i32>, ScrollBehavior),
    by: bool,
) -> JsResult<Value<'s>> {
    let waiter = perform_scroll(page, behavior, &[None], |dom, layout| {
        let moved = if by { layout.window_scroll_by(dom, l, t) } else { layout.window_scroll_to(dom, l, t) };
        moved.then_some(None).into_iter().collect()
    });
    scroll_promise(s, page, waiter)
}

pub(crate) fn queue_scroll(page: &Rc<PageDom>, target: Option<NodeId>, scrollend: bool) {
    // pyro: a scroll the page asked for (script, input, a smooth scroll's step) is a scroll to
    // report whatever offset it ends at: only a layout adjustment that undoes itself is none
    // (`scroll_back_where_reported`; `clamp_scrolls` queues those through `queue_layout_scroll`).
    page.scroll_reported.borrow_mut().remove(&(page.document(), target));
    queue_layout_scroll(page, target, scrollend);
}

/// [`queue_scroll`] for an adjustment layout made (a clamp, a re-snap, scroll anchoring): it keeps
/// the offset its last `scroll` event reported, so an adjustment brought back there fires nothing.
fn queue_layout_scroll(page: &Rc<PageDom>, target: Option<NodeId>, scrollend: bool) {
    // HTML "run the scroll steps": a scrolled target is queued once and its
    // scroll event fires in the next rendering update before animation
    // callbacks. A 16 ms fallback also delivers it when no frame was requested.
    // Chromium dispatches it in its next frame: on a page that just rendered
    // (TOS react-aria autocomplete) that
    // was 23-29 ms after Playwright's scroll step, after its click; on an
    // idle page a frame can run within ~1 ms. Fired as an immediate task, it
    // let close-on-scroll handlers close the popover before the click.
    // Scrolls that complete (instant ones: programmatic, reveal, wheel; a
    // smooth scroll's last step) are followed by `scrollend` in the same
    // update, as Chromium fires it after instant programmatic scrolls (Gap
    // 35); layout clamps and re-snaps end with `scrollend` too; a smooth scroll's other steps fire `scroll` only.
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
            // (A scroller back where its last `scroll` left it fires none, but a scroll that ended
            // there still ends: a smooth scroll interrupted and sent back to where it started
            // reports its last step at the same offset, then its `scrollend`.)
            if !scroll_back_where_reported(&p, doc, target) {
                dispatch_scroll(&p, doc, target, "scroll");
            }
            if end {
                fire_snap_events(&p);
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
    // pyro: a mandatory snap container starts at a snap position (CSS Scroll Snap 1 § 6.3), from
    // the first rendering update that lays it out (checked once per DOM version).
    if let (Ok(dom), Ok(mut layout)) = (page.dom.try_borrow(), page.layout.try_borrow_mut()) {
        layout.snap_initial_positions(&dom);
    }
    let (checked, queued) = page.scroll_clamp.get();
    if queued {
        // The delayed task that was to do it finds nothing owed.
        page.scroll_clamp.set((checked, false));
        // (The offsets script just wrote reach the engine before the clamp lays out: this runs
        // ahead of the frame, which would push them only after its callbacks.)
        push_scroll_offsets(page);
        clamp_scrolls(page, true);
    }
    flush_pending_scrolls(page);
    // pyro: snap targets a layout change or an initial snap changed, without a scroll to report
    // them before its `scrollend`.
    fire_snap_events(page);
}

/// Runs queued scroll steps before animation-frame callbacks in a rendering update.
/// pyro: a rendering frame of `page` starts: `content-visibility: auto` relevance is determined
/// anew (what queries forced to render since the last frame no longer is).
pub fn content_visibility_frame(page: &Rc<PageDom>) {
    if let Some(layout) = page.try_layout() {
        layout.content_visibility_frame();
    }
}

/// pyro: the `contentvisibilityautostatechange` events of a rendering update of `page` (CSS
/// Contain 2 § 4.1), right after relevance is determined at its start
/// ([`content_visibility_frame`]), before its animation frame callbacks: each
/// `content-visibility: auto` element whose skipped state was determined for the first time, or
/// changed since its last event, gets one (`skipped`), in tree order. Layouts that geometry reads
/// force between frames fire none (WPT css-contain `content-visibility-with-top-layer-008`); a
/// disconnected element none either (`-auto-state-changed-removed`). A page that listens for no
/// such event lays nothing out for it.
pub fn content_visibility_events(page: &Rc<PageDom>) {
    const TYPE: &str = "contentvisibilityautostatechange";
    if page.is_disposed() {
        return;
    }
    // (Handler content attributes the parser set are registered first, as a dispatch does.)
    if crate::target::unheard(page, TYPE) {
        return;
    }
    let states = {
        let (Ok(dom), Some(layout)) = (page.dom.try_borrow(), page.try_layout()) else { return };
        let states = layout.content_visibility_auto_states(&dom);
        // (What listeners change from here on is read again: `schedule_content_visibility_check`.)
        page.cv_auto_check.set((page.cv_auto_check.get().0, Some((dom.version(), layout.scroll_generation()))));
        states
    };
    let mut changed: Vec<(NodeId, bool)> = {
        let mut recorded = page.cv_auto_states.borrow_mut();
        let mut next = std::collections::HashMap::with_capacity(states.len());
        let mut changed = Vec::new();
        for (id, skipped) in states {
            if recorded.get(&id) != Some(&skipped) {
                changed.push((id, skipped));
            }
            next.insert(id, skipped);
        }
        // (Elements that left the document or `content-visibility: auto` are forgotten: one that
        // comes back is determined anew.)
        *recorded = next;
        changed
    };
    if changed.is_empty() {
        return;
    }
    {
        let dom = page.dom.borrow();
        let precedes = |a: NodeId, b: NodeId| dom.compare_document_position(b, a).is_ok_and(|p| p & 0x02 != 0);
        changed.sort_by(|(a, _), (b, _)| {
            if a == b {
                std::cmp::Ordering::Equal
            } else if precedes(*a, *b) {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Greater
            }
        });
    }
    for (id, skipped) in changed {
        // (A listener may have removed a later one.)
        if !page.dom.borrow().is_connected(id).unwrap_or(false) {
            page.cv_auto_states.borrow_mut().remove(&id);
            continue;
        }
        let ev = crate::event::EventCore::trusted(
            page,
            crate::event::EventInterface::ContentVisibilityAutoStateChangeEvent,
            TYPE,
            false,
            false,
        );
        ev.fields.borrow_mut().skipped = skipped;
        crate::target::fire_at_node(page, id, &ev);
    }
}

/// pyro: after a task of `page` that listens for `contentvisibilityautostatechange`: when the DOM
/// or a scroll position changed since the states were last read, they are read again at the
/// frame interval, as a rendering update without animation frame callbacks would (pyro runs
/// frames only for them): relevance is determined, then the events fire
/// ([`content_visibility_events`]). One lookup for a page without such a listener; none queued for
/// a page whose DOM and scroll positions stay as they were (a quiet page stays idle).
pub(crate) fn schedule_content_visibility_check(page: &Rc<PageDom>) {
    const TYPE: &str = "contentvisibilityautostatechange";
    let (pending, stamp) = page.cv_auto_check.get();
    if pending || page.is_disposed() {
        return;
    }
    {
        let (Ok(dom), Some(layout)) = (page.dom.try_borrow(), page.try_layout()) else { return };
        if !dom.listens_for(TYPE) && !dom.has_pending_inline_handlers() {
            return;
        }
        if stamp == Some((dom.version(), layout.scroll_generation())) {
            return;
        }
    }
    page.cv_auto_check.set((true, stamp));
    let p = page.clone();
    page.post_delayed_task(
        std::time::Duration::from_millis(16),
        Box::new(move || {
            p.cv_auto_check.set((false, p.cv_auto_check.get().1));
            // (A listener removed meanwhile: no frame to run for it.)
            if crate::target::unheard(&p, TYPE) {
                return;
            }
            content_visibility_frame(&p);
            content_visibility_events(&p);
        }),
    );
}

/// pyro: the ResizeObserver step of a rendering update of `page` (after its animation frame
/// callbacks, immediately before its ResizeObserver callbacks): elements with
/// `contain-intrinsic-size: auto` remember their sizes (CSS Sizing 4 § 5.1).
pub fn record_remembered_sizes(page: &PageDom) {
    if page.is_disposed() {
        return;
    }
    if let Some(layout) = page.try_layout() {
        let Ok(dom) = page.dom.try_borrow() else { return };
        layout.record_remembered_sizes(&dom);
    }
}

/// pyro: the scroll steps of an animation frame of `page` (HTML "update the rendering"): a
/// mandatory snap container not yet snapped snaps (CSS Scroll Snap 1 § 6.3: at its first layout,
/// before script that waits a frame scrolls it), running smooth scrolls advance a frame, the
/// queued scroll events fire, and so do the snap events of snap targets that changed without a
/// scroll to report them (the initial snap's, as Chromium; CSS Scroll Snap 2 § 5.1).
pub fn run_frame_scroll_steps(page: &Rc<PageDom>, deliver_scrolls: bool) {
    if page.is_disposed() {
        return;
    }
    // pyro: the frame's scroll offsets reach the layout engine before anything lays the document
    // out: the layout the steps below force (the visual viewport's scrollbars, the owed clamp)
    // samples the scroll-driven animations at the offsets it was told, and the animation stamp it
    // computes is kept for the rest of the task (a view timeline showed the previous frame's
    // progress; WPT scroll-animations `view-timeline-relative-ancestor`).
    push_scroll_offsets(page);
    // The resize steps for the visual viewport (its classic scrollbars changed).
    page.fire_visual_viewport_resize();
    // HTML "update the rendering": the scroll to the load's fragment is tried again.
    crate::highlights::try_fragment_scroll(page, false);
    if let (Ok(dom), Ok(mut layout)) = (page.dom.try_borrow(), page.layout.try_borrow_mut()) {
        layout.snap_initial_positions(&dom);
    }
    // pyro: what script changed since the offsets were last settled (a revert in a `scroll`
    // handler) is laid out and adjusted for (anchoring, clamping) before this frame's callbacks,
    // as Chromium's layout does; the delayed re-clamp would come after callbacks that wait two
    // frames (WPT scroll-anchoring `after-scrollable-range-shrinkage-*`).
    let owed = {
        let (checked, queued) = page.scroll_clamp.get();
        queued || (page.dom.try_borrow().is_ok_and(|dom| checked != Some(dom.version())) && page.layout.try_borrow().is_ok_and(|layout| layout.has_scroll_offsets()))
    };
    if owed {
        let (checked, _) = page.scroll_clamp.get();
        page.scroll_clamp.set((checked, false));
        // The frame's scroll offsets reach the layout engine first (scroll timelines, scroll
        // markers, `position-try` fallbacks that follow a scroller): the clamp lays the document
        // out, and a layout made with the offsets of before the script scroll left the first frame
        // after it showing the old fallback and active marker (WPT css-anchor-position
        // `anchor-scroll-position-try-003`, css-overflow `scroll-marker-controls-scroll-tracking-003`).
        if let Some(layout) = page.try_layout() {
            if layout.push_scroll() {
                layout.set_scroll_pushed_at(crate::event::page_now_ms(page));
            }
            if let Ok(dom) = page.dom.try_borrow() {
                layout.push_stuck(&dom);
            }
        }
        clamp_scrolls(page, true);
    }
    crate::smooth_scroll::step(page);
    if deliver_scrolls {
        flush_pending_scrolls(page);
    }
    fire_snap_events(page);
}

/// pyro: tells the layout engine the scroll offsets as they are now (scroll timelines, scroll
/// markers, `position-try` fallbacks that follow a scroller) before a layout a clamp or a frame
/// forces: that layout would choose with the offsets of the last push.
fn push_scroll_offsets(page: &Rc<PageDom>) {
    if let Some(layout) = page.try_layout() {
        if layout.push_scroll() {
            layout.set_scroll_pushed_at(crate::event::page_now_ms(page));
        }
        if let Ok(dom) = page.dom.try_borrow() {
            layout.push_stuck(&dom);
        }
    }
}

pub fn flush_pending_scrolls(page: &Rc<PageDom>) {
    let pending = std::mem::take(&mut *page.pending_scrolls.borrow_mut());
    for ((doc, target), end, _) in pending {
        if !scroll_back_where_reported(page, doc, target) {
            dispatch_scroll(page, doc, target, "scroll");
        }
        if end {
            fire_snap_events(page);
            dispatch_scroll(page, doc, target, "scrollend");
        }
    }
}

/// pyro: whether the scroller is where its last `scroll` event left it: an adjustment that was
/// made and undone before the rendering update (scroll anchoring over a toggled `display`, as
/// WPT scroll-anchoring `infinite-scroll-event`) is no scroll, and Chromium reports none; its
/// handler would otherwise toggle again, every frame.
fn scroll_back_where_reported(page: &Rc<PageDom>, doc: NodeId, target: Option<NodeId>) -> bool {
    let Some(reported) = page.scroll_reported.borrow().get(&(doc, target)).copied() else { return false };
    let Ok(layout) = page.layout.try_borrow() else { return false };
    let now = match target {
        Some(el) => (layout.scroll_left(el), layout.scroll_top(el)),
        None => (layout.scroll_x(), layout.scroll_y()),
    };
    now == reported
}

/// pyro: CSS Scroll Snap 2 § 5.1: `scrollsnapchanging`, then `scrollsnapchange`, at each scroller
/// whose snap targets changed since they were last reported (a scroll's, before its
/// `scrollend`, as Chromium; a layout change's in the rendering update that re-snaps it): at the
/// document for the viewport, bubbling, at the element otherwise, not; never cancelable. A
/// scroller that stopped snapping reports `(null, null)`.
pub(crate) fn fire_snap_events(page: &Rc<PageDom>) {
    if page.is_disposed() {
        return;
    }
    let changes = match (page.dom.try_borrow(), page.layout.try_borrow_mut()) {
        (Ok(dom), Ok(mut layout)) => layout.take_snap_changes(&dom),
        _ => return,
    };
    let doc = page.document();
    for (scroller, targets) in changes {
        for ty in ["scrollsnapchanging", "scrollsnapchange"] {
            if page.is_disposed() || crate::target::unheard(page, ty) {
                continue;
            }
            let Some(key) = page.dom.borrow().node_key(scroller.unwrap_or(doc)) else { continue };
            let iface = crate::event::EventInterface::SnapEvent;
            let ev = crate::event::EventCore::trusted(page, iface, ty, scroller.is_none(), false);
            crate::event::set_snap_targets(page, &mut ev.fields.borrow_mut(), targets);
            crate::target::fire(page, key, &ev);
        }
    }
}

/// pyro: queues `scroll` and `scrollend` at the originating element of each scroll marker group
/// that scrolled to keep its active marker in view, after the scroll that moved the marker
/// (Chromium scrolls the group smoothly; WPT css-overflow
/// `scroll-marker-group-keeps-active-in-view`).
fn deliver_scroll_group_moves(page: &Rc<PageDom>) {
    let moves = match (page.dom.try_borrow(), page.try_layout()) {
        (Ok(dom), Some(layout)) => layout.style().take_scroll_group_moves(&dom),
        _ => Vec::new(),
    };
    for owner in moves {
        let p = page.clone();
        page.post_delayed_task(std::time::Duration::from_millis(20), Box::new(move || queue_scroll(&p, Some(owner), true)));
    }
}

fn dispatch_scroll(p: &Rc<PageDom>, doc: NodeId, target: Option<NodeId>, kind: &str) {
    if p.is_disposed() || p.document() != doc {
        return;
    }
    if kind == "scroll" {
        if let Ok(layout) = p.layout.try_borrow() {
            let now = match target {
                Some(el) => (layout.scroll_left(el), layout.scroll_top(el)),
                None => (layout.scroll_x(), layout.scroll_y()),
            };
            p.scroll_reported.borrow_mut().insert((doc, target), now);
        }
    }
    match target {
        Some(el) => {
            let connected = {
                let dom = p.dom.borrow();
                dom.is_connected(el).unwrap_or(false) && dom.node_document(el).ok() == Some(doc)
            };
            if connected {
                // (The scroll targets and offsets are the engine's before the handlers read the
                // active scroll marker.)
                if kind == "scrollend" {
                    if let Ok(layout) = p.layout.try_borrow() {
                        layout.push_scroll();
                    }
                }
                crate::input::fire_simple(p, el, kind, false, false);
                // (A scroll can move the active scroll marker: the layout engine follows it.)
                if kind == "scrollend" {
                    if let Some(layout) = p.try_layout() {
                        layout.push_scroll();
                    }
                    crate::animation_events::layout_flush(p, el);
                    deliver_scroll_group_moves(p);
                }
            }
        }
        None => {
            if kind == "scrollend" {
                if let Ok(layout) = p.layout.try_borrow() {
                    layout.push_scroll();
                }
            }
            crate::input::fire_simple(p, doc, kind, true, false);
        }
    }
}

/// Re-clamps scroll offsets that the current layout no longer reaches and
/// fires their `scroll` events: `now` (a rendering update) dispatches them
/// at once, otherwise (a script read) at the next frame like other scrolls.
pub(crate) fn clamp_scrolls(page: &Rc<PageDom>, now: bool) {
    // pyro: a scroll marker group that scrolled to keep its active marker in view: its scroll
    // events go to its originating element, after the scroll that moved the marker (Chromium
    // scrolls the group smoothly; WPT css-overflow `scroll-marker-group-keeps-active-in-view`).
    deliver_scroll_group_moves(page);
    // pyro: a mandatory snap container starts at a snap position (CSS Scroll Snap 1 § 6.3).
    if let (Ok(dom), Ok(mut layout)) = (page.dom.try_borrow(), page.layout.try_borrow_mut()) {
        layout.snap_initial_positions(&dom);
    }
    // (A document with snap containers re-snaps unsnapped ones though nothing is scrolled.)
    if !page.layout.borrow().has_scroll_offsets() && !page.layout.borrow().snap_owed() {
        return;
    }
    let moved = {
        let dom = page.dom.borrow();
        let moved = page.layout.borrow_mut().clamp_scroll_offsets(&dom);
        page.scroll_clamp.set((Some(dom.version()), page.scroll_clamp.get().1));
        moved
    };
    let doc = page.document();
    // A clamp or re-snap is a scroll that finished: `scroll` is followed by `scrollend` (Chromium;
    // WPT `scrollend-event-fired-after-layout-clamping`, `scrollend-event-fired-for-mandatory-snap-
    // point-after-load`). `moved` holds only real offset changes.
    let mut ended = Vec::new();
    for target in moved {
        let node = match target {
            pyro_dom::layout::ScrollTarget::Container(node) => Some(node),
            pyro_dom::layout::ScrollTarget::Viewport => None,
        };
        if !now {
            queue_layout_scroll(page, node, true);
        } else if scroll_back_where_reported(page, doc, node) {
            // (A queued `scroll` finds the same at its flush.)
            continue;
        } else if page.pending_scrolls.borrow().iter().any(|(key, _, _)| *key == (doc, node)) {
            // Its queued `scroll` is still to come: let it carry the `scrollend`.
            queue_layout_scroll(page, node, true);
        } else {
            dispatch_scroll(page, doc, node, "scroll");
            ended.push(node);
        }
    }
    // pyro: a re-snap's snap events, after its `scroll` (a script read's at the next rendering
    // update).
    if now {
        fire_snap_events(page);
        for node in ended {
            dispatch_scroll(page, doc, node, "scrollend");
        }
    }
}

/// pyro: a script read of geometry (`getBoundingClientRect`, `offsetTop`, ...) lays out, which
/// adjusts the scroll offsets of the scrollers whose anchor moved (CSS Scroll Anchoring 1 § 3;
/// Chromium does it in layout, before the read returns; WPT `contain-paint-offscreen-container`
/// reads a rect after the change). The `scroll` events follow at the next frame.
pub(crate) fn apply_owed_scroll_anchors(page: &Rc<PageDom>) {
    let owed = match (page.dom.try_borrow(), page.layout.try_borrow()) {
        (Ok(dom), Ok(layout)) => layout.scroll_anchors_owed(&dom),
        _ => false,
    };
    // (A caller that still holds the layout cannot have it adjusted here: the next frame does.)
    if owed && page.layout.try_borrow_mut().is_ok() {
        clamp_scrolls(page, false);
    }
}

/// After a task that changed the DOM while something is scrolled: re-clamp
/// at the next frame (16 ms, the scroll event cadence), when Chromium's
/// rendering update would lay out without any script read.
pub(crate) fn schedule_scroll_clamp(page: &Rc<PageDom>) {
    let (checked, queued) = page.scroll_clamp.get();
    if queued || page.is_disposed() {
        return;
    }
    // (A document with snap containers re-snaps unsnapped ones though nothing is scrolled.)
    let owed = {
        let layout = page.layout.borrow();
        layout.has_scroll_offsets() || layout.snap_owed()
    };
    if !owed {
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
                push_scroll_offsets(&p);
                clamp_scrolls(&p, true);
            }
        }),
    );
}

/// `scrollIntoView(arg)`: `(boolean or ScrollIntoViewOptions)`; `true`, a
/// missing argument and `{}` align block start / inline nearest, `false`
/// block end / inline nearest. Each box it scrolls scrolls smoothly when `behavior` (else the
/// box's `scroll-behavior`) says so ([`perform_scroll`]). Returns a scroll promise
/// ([`scroll_promise`]).
pub(crate) fn scroll_into_view<'s>(s: &Scope<'s>, n: &NodeData, a: Args<'s>) -> JsResult<Value<'s>> {
    use pyro_dom::layout::ScrollAlign;
    let arg = a.get(0);
    let (mut block, mut inline) = (ScrollAlign::Start, ScrollAlign::Nearest);
    let mut behavior = ScrollBehavior::Auto;
    if let Some(o) = arg.as_object() {
        // Dictionary members in lexicographic order: behavior, block, inline.
        behavior = scroll_behavior(&o, "scrollIntoView", "Element")?;
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
    let waiter = perform_scroll(page, behavior, &[], |dom, layout| {
        let moved = layout.scroll_into_view_aligned_script_targets(dom, n.id, block, inline);
        moved.iter().map(scroll_target_node).collect()
    });
    scroll_promise(s, page, waiter)
}

fn scroll_target_node(target: &pyro_dom::layout::ScrollTarget) -> Option<NodeId> {
    match *target {
        pyro_dom::layout::ScrollTarget::Container(node) => Some(node),
        pyro_dom::layout::ScrollTarget::Viewport => None,
    }
}

pub(crate) fn scroll_into_view_if_needed<'s>(s: &Scope<'s>, n: &NodeData, a: Args<'s>) -> JsResult<Value<'s>> {
    let center_if_needed = a.get(0).is_undefined() || a.get(0).to_bool();
    let page = n.page();
    let moved =
        page.layout.borrow_mut().scroll_into_view_if_needed_script_targets(&page.dom.borrow(), n.id, center_if_needed);
    fire_scroll_targets(page, moved);
    Ok(s.undefined())
}

/// The viewport's scroll position (CSS px), for a history entry to persist.
pub fn viewport_scroll(page: &Rc<PageDom>) -> (i32, i32) {
    let layout = page.layout.borrow();
    (layout.scroll_x() as i32, layout.scroll_y() as i32)
}

/// Scrolls the viewport back to a history entry's persisted position: smoothly under the root's
/// `scroll-behavior: smooth`, as Chrome (WPT cssom-view `scroll-behavior-smooth-navigation`).
pub fn restore_viewport_scroll(page: &Rc<PageDom>, (x, y): (i32, i32)) {
    crate::animation_events::layout_flush(page, page.document());
    perform_scroll(page, ScrollBehavior::Auto, &[None], |dom, layout| {
        layout.window_scroll_to(dom, Some(x), Some(y)).then_some(None).into_iter().collect()
    });
}

/// HTML "scroll to the fragment", after a same-document navigation to `fragment` (the URL's
/// fragment, percent-encoded): the indicated part of the document is scrolled into view (block
/// start, inline nearest), or the top of the document for an empty fragment or `top`. Nothing
/// moves when no element is indicated.
pub fn scroll_to_fragment(page: &Rc<PageDom>, fragment: &str) {
    scroll_to_fragment_or(page, fragment, false);
}

/// [`scroll_to_fragment`] of a document load, which tries again until it can: true when there is
/// nothing more to do (the top of the document, or an element was scrolled to), false when no
/// element is indicated (yet).
pub fn try_scroll_to_fragment(page: &Rc<PageDom>, fragment: &str) -> bool {
    let decoded = percent_encoding::percent_decode_str(fragment).decode_utf8_lossy();
    if fragment.is_empty() || decoded.eq_ignore_ascii_case("top") {
        return true;
    }
    scroll_to_fragment_or(page, fragment, false)
}

/// URL Fragment Text Directives "scroll a range into view": the element holding the text a
/// directive matched (`node`, a text node) is centered in the viewport, as Chrome does, and so
/// is each same-origin ancestor frame's element showing it (a cross-origin parent is not
/// scrolled: it would learn where the text is).
pub fn scroll_to_text_match(page: &Rc<PageDom>, node: NodeId) {
    let el = {
        let dom = page.dom.borrow();
        if dom.node_type(node).ok() == Some(pyro_dom::NodeType::Element) { Some(node) } else { dom.parent_element(node).ok().flatten() }
    };
    let Some(el) = el else { return };
    scroll_centered(page, el);
    let mut current = page.clone();
    while let Some(crate::page::FrameContainer::Frame(parent, owner)) = current.hooks().map(|h| h.container()) {
        if parent.origin_key() != current.origin_key() {
            break;
        }
        scroll_centered(&parent, owner);
        current = parent;
    }
}

/// `el` scrolled into the center of every scrolling box around it, the viewport included.
fn scroll_centered(page: &Rc<PageDom>, el: NodeId) {
    use pyro_dom::layout::ScrollAlign;
    crate::animation_events::layout_flush(page, page.document());
    perform_scroll(page, ScrollBehavior::Auto, &[], |dom, layout| {
        layout
            .scroll_into_view_aligned_script_targets(dom, el, ScrollAlign::Center, ScrollAlign::Center)
            .iter()
            .map(scroll_target_node)
            .collect()
    });
}

/// [`scroll_to_fragment`] that, with `top_if_missing`, also scrolls to the top of the document
/// when no element is indicated (the Navigation API's "process scroll behavior", which Chrome
/// does for a missing fragment as for an empty one).
pub fn scroll_to_fragment_or_top(page: &Rc<PageDom>, fragment: &str) {
    scroll_to_fragment_or(page, fragment, true);
}

fn scroll_to_fragment_or(page: &Rc<PageDom>, fragment: &str, top_if_missing: bool) -> bool {
    use pyro_dom::layout::ScrollAlign;
    let doc = page.document();
    let decoded = percent_encoding::percent_decode_str(fragment).decode_utf8_lossy().into_owned();
    crate::animation_events::layout_flush(page, doc);
    let target = {
        let dom = page.dom.borrow();
        // HTML "find a potential indicated element": an element with the id, else the first `a`
        // with the name, for the fragment as it is, then percent-decoded.
        let find = |name: &str| {
            if name.is_empty() {
                return None;
            }
            dom.get_element_by_id(doc, name).ok().flatten().or_else(|| {
                dom.descendants(doc).ok().into_iter().flatten().find(|&n| {
                    dom.element(n).is_ok_and(|e| &*e.local_name() == "a")
                        && dom.get_attribute(n, "name").ok().flatten().is_some_and(|value| value == name)
                })
            })
        };
        find(fragment).or_else(|| find(&decoded))
    };
    // HTML "scroll to the fragment": the ancestor revealing algorithm runs on the target before
    // it is scrolled to; `beforematch` handlers may move, remove or hide it, so layout is
    // brought up to date again and a target no longer connected is not scrolled to.
    let target = target.filter(|&el| {
        reveal_ancestors(page, el);
        crate::animation_events::layout_flush(page, doc);
        page.dom.borrow().is_connected(el).unwrap_or(false)
    });
    // (Scrolled with behavior `auto`: the boxes' `scroll-behavior` decides.)
    match target {
        Some(el) => {
            perform_scroll(page, ScrollBehavior::Auto, &[], |dom, layout| {
                layout
                    .scroll_into_view_aligned_script_targets(dom, el, ScrollAlign::Start, ScrollAlign::Nearest)
                    .iter()
                    .map(scroll_target_node)
                    .collect()
            });
            // HTML: then "run the focusing steps for target, with the Document's viewport as the
            // fallback target": a focusable target takes focus (focus events fire), else the
            // viewport does, which leaves the document without a focused element (WPT
            // focus-changes-after-scroll-to-fragment).
            let focusable = {
                let layout = page.layout();
                let dom = page.dom.borrow();
                crate::input::is_focusable_area(&layout, &dom, el)
            };
            if focusable {
                crate::input::focus_element(page, el);
            } else {
                let focused = page.dom.borrow().focused(doc).ok().flatten();
                if let Some(focused) = focused {
                    crate::input::blur_element(page, focused);
                }
            }
            true
        }
        None if top_if_missing || fragment.is_empty() || decoded.eq_ignore_ascii_case("top") => {
            perform_scroll(page, ScrollBehavior::Auto, &[None], |dom, layout| {
                layout.window_scroll_to(dom, Some(0), Some(0)).then_some(None).into_iter().collect()
            });
            true
        }
        None => false,
    }
}

/// HTML "ancestor revealing algorithm" on `node`: walking up the flat tree (a shadow root's
/// host stands for the root), the `hidden=until-found` elements and the closed `<details>`
/// whose content holds the walk are collected first, innermost first; then each closed
/// `<details>` opens, and each `hidden=until-found` element gets a bubbling `beforematch` and
/// loses its `hidden` attribute. The run stops at an element no longer connected, a `<details>`
/// already open, or an element whose `hidden` left the until-found state before its turn
/// (WPT html/editing/the-hidden-attribute).
pub fn reveal_ancestors(page: &Rc<PageDom>, node: NodeId) {
    use markup5ever::local_name;
    enum Reveal {
        Details(NodeId),
        UntilFound(NodeId),
    }
    fn until_found(dom: &pyro_dom::Dom, id: NodeId) -> bool {
        dom.element(id)
            .is_ok_and(|e| e.is_html() && e.attr("hidden").is_some_and(|v| v.eq_ignore_ascii_case("until-found")))
    }
    // `child` is in the content slot of `parent`, a closed `<details>`: any child but its
    // first `<summary>`.
    fn in_closed_details(dom: &pyro_dom::Dom, child: NodeId, parent: NodeId) -> bool {
        let Ok(p) = dom.element(parent) else { return false };
        if !p.is_html() || p.name.local != local_name!("details") || p.attr_local(&local_name!("open")).is_some() {
            return false;
        }
        let is_summary =
            |id: &NodeId| dom.element(*id).is_ok_and(|c| c.is_html() && c.name.local == local_name!("summary"));
        dom.children(parent).ok().and_then(|mut c| c.find(is_summary)) != Some(child)
    }
    let reveal = {
        let dom = page.dom.borrow();
        let mut reveal = Vec::new();
        let mut current = node;
        while let Some(parent) = dom.parent(current).ok().flatten() {
            if until_found(&dom, current) {
                reveal.push(Reveal::UntilFound(current));
            }
            if in_closed_details(&dom, current, parent) {
                reveal.push(Reveal::Details(parent));
            }
            current = dom.fragment_host(parent).ok().flatten().unwrap_or(parent);
        }
        reveal
    };
    for step in reveal {
        match step {
            Reveal::Details(details) => {
                let mut dom = page.dom.borrow_mut();
                if !dom.is_connected(details).unwrap_or(false)
                    || dom.get_attribute(details, "open").ok().flatten().is_some()
                {
                    return;
                }
                let _ = dom.set_attribute(details, "open", "");
            }
            Reveal::UntilFound(el) => {
                {
                    let dom = page.dom.borrow();
                    if !dom.is_connected(el).unwrap_or(false) || !until_found(&dom, el) {
                        return;
                    }
                }
                crate::input::fire_simple(page, el, "beforematch", true, false);
                let _ = page.dom.borrow_mut().remove_attribute(el, "hidden");
            }
        }
    }
}

fn fire_scroll_targets(page: &Rc<PageDom>, moved: Vec<pyro_dom::layout::ScrollTarget>) {
    for target in moved {
        fire_scroll(page, scroll_target_node(&target));
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
    let page = &owner_page(n);
    let v = page.layout().check_visibility(&page.dom.borrow(), n.id, opts);
    Ok(s.boolean(v))
}

/// pyro: the effective `zoom` of the element `el` (1 without a style): its scroll offsets are
/// laid out in its zoomed px, and script reads and writes them in its own CSS px, as Chromium
/// does (WPT cssom-view `scrollTo-zoom`).
fn element_zoom(page: &PageDom, el: NodeId) -> f64 {
    let (Ok(dom), Some(layout)) = (page.dom.try_borrow(), page.try_layout()) else { return 1.0 };
    layout
        .style()
        .stylo_style(&dom, el)
        .map(|style| style.effective_zoom.value() as f64)
        .filter(|zoom| zoom.is_finite() && *zoom > 0.0)
        .unwrap_or(1.0)
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

/// `Element.scroll()`, `scrollTo()` (`operation`) and `scrollBy()` (`by`): returns a scroll
/// promise ([`scroll_promise`]).
pub(crate) fn scroll_to<'s>(
    s: &Scope<'s>,
    n: &NodeData,
    a: Args<'s>,
    operation: &str,
    by: bool,
) -> JsResult<Value<'s>> {
    let (l, t, behavior) = scroll_options(&a, operation, "Element", to_scroll_number)?;
    let page = n.page();
    let viewport = scrolls_viewport(page, n.id);
    let zoom = if viewport { 1.0 } else { element_zoom(page, n.id) };
    let (css_l, css_t) = (l, t);
    let (l, t) = (l.map(|v| round_scroll_offset(v * zoom)), t.map(|v| round_scroll_offset(v * zoom)));
    let target = (!viewport).then_some(n.id);
    let waiter = perform_scroll(page, behavior, &[target], |dom, layout| {
        let moved = match (viewport, by) {
            (true, true) => layout.window_scroll_by(dom, l, t),
            (true, false) => layout.window_scroll_to(dom, l, t),
            (false, true) => layout.element_scroll_by(dom, n.id, l, t),
            (false, false) => layout.element_scroll_to(dom, n.id, l, t),
        };
        // pyro: what script asked for reads back (a zoomed element's fractional offset).
        if !viewport && !by {
            layout.note_css_scroll(n.id, css_l, css_t, zoom);
        }
        moved.then_some(target).into_iter().collect()
    });
    scroll_promise(s, page, waiter)
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
        (false, true) => layout.css_scroll(n.id, true, element_zoom(n.page(), n.id)),
        (false, false) => layout.css_scroll(n.id, false, element_zoom(n.page(), n.id)),
    }))
}

pub(crate) fn scroll_set(_s: &Scope<'_>, n: &NodeData, v: Value<'_>, top: bool) -> JsResult<()> {
    let value = to_scroll_number(v)?;
    let page = n.page();
    let viewport = scrolls_viewport(page, n.id);
    let zoom = if viewport { 1.0 } else { element_zoom(page, n.id) };
    let css = value;
    let value = round_scroll_offset(value * zoom);
    let target = (!viewport).then_some(n.id);
    // (Behavior `auto`: smooth under the box's `scroll-behavior: smooth`.)
    perform_scroll(page, ScrollBehavior::Auto, &[target], |dom, layout| {
        let moved = match (viewport, top) {
            (true, true) => layout.window_scroll_to(dom, None, Some(value)),
            (true, false) => layout.window_scroll_to(dom, Some(value), None),
            (false, true) => layout.set_scroll_top(dom, n.id, value),
            (false, false) => layout.set_scroll_left(dom, n.id, value),
        };
        if !viewport {
            let (x, y) = if top { (None, Some(css)) } else { (Some(css), None) };
            layout.note_css_scroll(n.id, x, y, zoom);
        }
        moved.then_some(target).into_iter().collect()
    });
    Ok(())
}

pub(crate) fn metric<'s>(s: &Scope<'s>, n: &NodeData, which: &str) -> JsResult<Value<'s>> {
    let page = &owner_page(n);
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
    let v = l.metric_css(&dom, n.id, metric);
    Ok(s.number(v))
}

/// `Element.currentCSSZoom` (CSSOM View): the element's effective `zoom`, 1 where it is not rendered.
pub(crate) fn current_css_zoom<'s>(s: &Scope<'s>, n: &NodeData) -> JsResult<Value<'s>> {
    let page = &owner_page(n);
    crate::animation_events::layout_flush(page, n.id);
    let zoom = page.layout().effective_zoom(&page.dom.borrow(), n.id);
    Ok(s.number(zoom))
}

pub(crate) fn offset_parent<'s>(s: &Scope<'s>, n: &NodeData) -> JsResult<Value<'s>> {
    let owner = &owner_page(n);
    let p = owner.layout().offset_parent(&owner.dom.borrow(), n.id);
    opt_node(s, n.page(), p)
}

pub(crate) fn inner_text<'s>(s: &Scope<'s>, n: &NodeData) -> JsResult<Value<'s>> {
    let page = n.page();
    let t = page.dom.borrow().inner_text(page.layout().style(), n.id).js()?;
    Ok(s.string(&t))
}

/// `HTMLElement.innerText`'s setter: the text of a `<script>` set through it is not trusted
/// ([`set_script_inner_text`] is `HTMLScriptElement.innerText`).
pub(crate) fn set_inner_text(_s: &Scope<'_>, n: &NodeData, v: Value<'_>) -> JsResult<()> {
    let text = null_to_empty(v)?;
    n.page().dom.borrow_mut().set_inner_text(n.id, &text).js()
}

/// `HTMLScriptElement.innerText`'s setter, a Trusted Types sink ("HTMLScriptElement innerText"):
/// the script is trusted with the compliant text ([[ScriptText]]).
pub(crate) fn set_script_inner_text(s: &Scope<'_>, n: &NodeData, v: Value<'_>) -> JsResult<()> {
    let text = crate::trusted_types::compliant_string(s, n.page(), crate::trusted_types::Kind::Script, v, "HTMLScriptElement innerText", true)?;
    let mut dom = n.page().dom.borrow_mut();
    dom.set_script_text(n.id, &text);
    dom.set_inner_text(n.id, &text).js()
}

pub(crate) fn set_outer_text(_s: &Scope<'_>, n: &NodeData, v: Value<'_>) -> JsResult<()> {
    let text = null_to_empty(v)?;
    n.page().dom.borrow_mut().set_outer_text(n.id, &text).js()
}

/// Element hit-testing for CDP input (top-most element at the point).
pub fn hit_test(page: &PageDom, x: f64, y: f64) -> Option<NodeId> {
    page.layout().element_from_point(&page.dom.borrow(), x, y)
}

/// [`hit_test`] without image maps: the element whose box holds the point
/// (a usemap image, not its `area`).
pub fn hit_test_box(page: &PageDom, x: f64, y: f64) -> Option<NodeId> {
    page.layout().box_from_point(&page.dom.borrow(), x, y)
}

/// The `double x, double y` of `elementFromPoint`/`elementsFromPoint` (CSSOM View: plain `double`,
/// not `unrestricted`): both are required and a non-finite one (NaN included, which is what an
/// absent argument passed on as `undefined` converts to) is a TypeError (WPT `elementFromPoint-parameters`).
fn point_args(a: &Args<'_>, what: &str) -> JsResult<(f64, f64)> {
    a.require(2, what)?;
    let (x, y) = (a.get(0).to_number()?, a.get(1).to_number()?);
    if !x.is_finite() || !y.is_finite() {
        return Err(pyro_js::JsError::type_error(format!(
            "Failed to execute '{what}': The provided double value is non-finite."
        )));
    }
    Ok((x, y))
}

pub(crate) fn element_from_point<'s>(s: &Scope<'s>, n: &NodeData, a: Args<'s>) -> JsResult<Value<'s>> {
    let (x, y) = point_args(&a, "Document.elementFromPoint")?;
    let page = n.page();
    if n.id != page.document() {
        return Ok(s.null());
    }
    // Retargeted against the document: a hit inside a shadow tree is its
    // outermost host (CDP's `hit_test` stays un-retargeted, as Chrome's).
    let hit = if page.dom.borrow().drawable_canvas_seen() {
        // HTML-in-Canvas: the first of the hits a drawable canvas's drawn elements adjust.
        let hits = page.layout().elements_from_point(&page.dom.borrow(), x, y);
        crate::html_in_canvas::adjust_hits(page, x, y, hits).into_iter().next()
    } else {
        hit_test(page, x, y)
    };
    // CSSOM View "elementFromPoint": a point in the viewport that no box takes (the root and every
    // box `pointer-events: none`) is the root element's; only a point outside the viewport is null
    // (WPT pointerevents/pointerevent_root_hit_test).
    let hit = hit.or_else(|| {
        let (vw, vh) = {
            let layout = page.layout();
            (layout.viewport().width as f64, layout.viewport().height as f64)
        };
        if x >= 0.0 && y >= 0.0 && x < vw && y < vh {
            page.dom.borrow().document_element(page.document()).ok().flatten()
        } else {
            None
        }
    });
    let hit = hit.and_then(|h| page.dom.borrow().retarget_node(h, n.id));
    opt_node(s, page, hit)
}

/// `ShadowRoot.elementFromPoint`: the document's hit, retargeted against
/// the shadow root (a host's for anything deeper, null outside its tree).
pub(crate) fn shadow_element_from_point<'s>(s: &Scope<'s>, n: &NodeData, a: Args<'s>) -> JsResult<Value<'s>> {
    let (x, y) = point_args(&a, "ShadowRoot.elementFromPoint")?;
    let page = n.page();
    let hit = hit_test(page, x, y);
    let retargeted = hit.and_then(|h| page.dom.borrow().retarget_node(h, n.id));
    opt_node(s, page, retargeted)
}

pub(crate) fn shadow_elements_from_point<'s>(s: &Scope<'s>, n: &NodeData, a: Args<'s>) -> JsResult<Value<'s>> {
    let (x, y) = point_args(&a, "ShadowRoot.elementsFromPoint")?;
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
    let (x, y) = point_args(&a, "Document.elementsFromPoint")?;
    let page = n.page();
    let mut ids =
        if n.id == page.document() { page.layout().elements_from_point(&page.dom.borrow(), x, y) } else { Vec::new() };
    // HTML-in-Canvas: a drawable canvas's drawn elements, where they were drawn.
    let mut ids = crate::html_in_canvas::adjust_hits(page, x, y, std::mem::take(&mut ids));
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
    fn rendered_document(&self) -> Option<Rc<PageDom>> {
        let home = &self.el.page;
        let document = home.dom.borrow().owner_document(self.el.id).ok().flatten()?;
        let page = if home.document() == document {
            home.clone()
        } else {
            home.hooks()?.frame_tree().into_iter().find(|other| {
                other.document() == document && Rc::ptr_eq(&other.dom, &home.dom)
            })?
        };
        let mut frame = page.clone();
        while frame.is_nested_frame() {
            let crate::page::FrameContainer::Frame(parent, owner) = frame.container() else { return None };
            parent.layout().client_rect(&parent.dom.borrow(), owner)?;
            frame = parent;
        }
        Some(page)
    }

    /// The computed value of `name` for the element or its pseudo-element.
    fn value(&self, name: &str) -> String {
        use pyro_dom::stylo::PseudoArg;
        let Some(page) = self.rendered_document() else { return String::new() };
        match &self.pseudo {
            PseudoArg::Element => {
                crate::animation_events::observe(&page, self.el.id);
                page.layout().computed_style_value(&page.dom.borrow(), self.el.id, name)
            }
            PseudoArg::Pseudo(pseudo) => {
                // A style flush too: the pseudo-element's transitions start at it.
                crate::animation_events::observe(&page, self.el.id);
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
/// the property whose value the getter reads. The engine's longhands Chrome
/// lacks (`line-clamp`, `mask-border-*`, `font-width`, ...) are not among them:
/// feature detection (`'lineClamp' in style`) must not find them.
fn computed_property_names() -> &'static [&'static str] {
    static NAMES: std::sync::OnceLock<Vec<&'static str>> = std::sync::OnceLock::new();
    NAMES.get_or_init(|| {
        let mut names = crate::computed_properties::COMPUTED_PROPERTIES.to_vec();
        names.extend(crate::computed_properties::EPUB_ALIASES.iter().map(|&(_, property)| property));
        names
    })
}

/// Whether `name` (lowercase) is one of Chrome's computed properties.
fn chrome_computed_property(name: &str) -> bool {
    crate::computed_properties::COMPUTED_PROPERTIES.binary_search(&name).is_ok()
}

/// The engine's longhands a computed style lists (its indexed properties): those
/// Chrome has, under Chrome's name where the engine's is a newer one
/// (`font-width` is `font-stretch`), so every listed name also reads by name.
fn listed_longhands() -> &'static [&'static str] {
    const CHROME_NAMES: [(&str, &str); 1] = [("font-width", "font-stretch")];
    static NAMES: std::sync::OnceLock<Vec<&'static str>> = std::sync::OnceLock::new();
    NAMES.get_or_init(|| {
        let mut names: Vec<&'static str> = pyro_dom::stylo::longhand_names()
            .iter()
            .map(|&name| CHROME_NAMES.iter().find(|&&(engine, _)| engine == name).map_or(name, |&(_, chrome)| chrome))
            .filter(|name| chrome_computed_property(name))
            .collect();
        names.sort_unstable();
        names.dedup();
        names
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
const MAX_PROPERTIES: usize = 832;
/// The getters by property index, in blocks of 16.
static PROPERTY_GETTERS: [[ComputedGetter; 16]; MAX_PROPERTIES / 16] = property_getters!(
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30,
    31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51
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
        let properties = computed_property_names();
        let ordinary = properties.len() - crate::computed_properties::EPUB_ALIASES.len();
        let mut all: Vec<Vec<(&'static str, bool)>> = properties[..ordinary]
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
            // A property only the engine has (`line-clamp`, `mask-border`) is no property here,
            // as in Chrome: "".
            if !name.starts_with("--") {
                let lower = if name.bytes().any(|b| b.is_ascii_uppercase()) {
                    std::borrow::Cow::Owned(name.to_ascii_lowercase())
                } else {
                    std::borrow::Cow::Borrowed(name.as_str())
                };
                if !chrome_computed_property(&lower) && pyro_dom::stylo::is_property(&lower) {
                    return Ok(s.string(""));
                }
            }
            Ok(computed_string(s, &c.value(&name)))
        });
        def.method("getPropertyPriority", 1, |s, _c, _a| Ok(s.string("")));
        // The longhands, as indexed properties; an element without computed
        // style (not rendered) lists nothing, as in Chrome.
        def.method("item", 1, |s, c, a| {
            Ok(computed_item(s, c, to_u32(a.get(0))? as usize).unwrap_or_else(|| s.string("")))
        });
        def.getter("length", |s, c| {
            let names = computed_names(c);
            let count = if names.is_empty() { 0 } else { names.len() + computed_custom_names(c).len() };
            Ok(s.number(count as f64))
        });
        def.indexed(|s, c, i| Ok(computed_item(s, c, i as usize)));
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
fn computed_custom_names(c: &ComputedData) -> Vec<String> {
    let Some(page) = c.rendered_document() else { return Vec::new() };
    let dom = page.dom.borrow();
    let names = page.layout().style().computed_custom_names(&dom, c.el.id, &c.pseudo);
    names
}

fn computed_item<'s>(scope: &Scope<'s>, computed: &ComputedData, index: usize) -> Option<Value<'s>> {
    let names = computed_names(computed);
    if let Some(name) = names.get(index) { return Some(scope.string(name)); }
    if names.is_empty() { return None; }
    computed_custom_names(computed).get(index - names.len()).map(|name| scope.string(name))
}

fn computed_names(c: &ComputedData) -> &'static [&'static str] {
    if c.pseudo == pyro_dom::stylo::PseudoArg::Invalid {
        return &[];
    }
    let Some(page) = c.rendered_document() else { return &[] };
    let dom = page.dom.borrow();
    if page.layout().computed_style_value(&dom, c.el.id, "display").is_empty() {
        return &[];
    }
    listed_longhands()
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
    pub(crate) query: Rc<pyro_dom::cssom::MediaQueryList>,
    pub(crate) media: String,
    pub(crate) last: std::cell::Cell<bool>,
}

/// `MediaQueryList`: an EventTarget whose `change` fires when the embedder's
/// viewport or media emulation flips `matches`. Each call creates a fresh
/// object, kept alive while it has change listeners.
pub struct MqlData {
    target: crate::target::EventTargetData,
    query: Rc<pyro_dom::cssom::MediaQueryList>,
    media: String,
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
        def.getter("media", |s, m| Ok(s.string(&m.media)));
        def.getter("matches", |s, m| Ok(s.boolean(m.target.page.layout().style().matches_media_list(&m.query))));
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

/// `window.matchMedia(query)`.
pub fn match_media<'s>(s: &Scope<'s>, page: &Rc<PageDom>, query: String) -> JsResult<Object<'s>> {
    let target = crate::target::EventTargetData::new(page);
    let key = target.wrapper_identity();
    let query = Rc::new(pyro_dom::cssom::MediaQueryList::parse(&query));
    let media = query.media();
    let last = page.layout().style().matches_media_list(&query);
    page.media_queries.borrow_mut().push(MediaQueryEntry {
        target: target.key,
        query: query.clone(),
        media: media.clone(),
        last: std::cell::Cell::new(last),
    });
    s.wrap::<MediaQueryList>(MqlData { target, query, media }, key)
}

pub(crate) fn sync_media_listener(s: &Scope<'_>, object: Object<'_>) {
    let Some(list) = s.unwrap::<MediaQueryList>(object.value()) else { return };
    let target = &list.target;
    if target.page.dom.borrow().has_listener_for(target.key, "change") {
        crate::target::pin_target(s, &target.page, target.key, object);
    } else {
        crate::target::unpin_target(s, &target.page, target.key);
    }
}

pub(crate) fn register(engine: &pyro_js::Engine) {
    engine.register::<ComputedStyle>();
    engine.register::<MediaQueryList>();
}
