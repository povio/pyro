//! Classic scrollbars under the mouse: dragging the thumb, clicking the track or an arrow button
//! (CSS Overflow 3 § 3: the UA's scrollbars; Chrome's `Scrollbar::HandleMousePressEvent`).
//!
//! The press is still dispatched to the scroll container as a `mousedown`, as Chrome does, but a
//! press on a scrollbar part takes no focus, starts no selection and no drag, and gives no
//! `click` ([`input`] says so). A thumb follows the pointer unsnapped (a snap container's snap
//! positions apply where the drag ends, to its end position), firing `scroll` per move and
//! `scrollend` at the release. A track click scrolls by a page (Chrome's page step: the larger of
//! 87.5% of the scrollport and the scrollport less 40 px), an arrow button by a line (40 px).

use std::rc::Rc;

use pyro_dom::layout::{ScrollbarHit, ScrollbarPart};
use pyro_dom::NodeId;

use crate::input::{MouseInput, MouseKind};
use crate::page::PageDom;

/// Chrome's `kPixelsPerLineStep`, the scroll of an arrow button.
const LINE_STEP: f64 = 40.0;
/// Chrome's `kMaxOverlapBetweenPages` and `kMinFractionToStepWhenPaging`, the track click's page.
const PAGE_OVERLAP: f64 = 40.0;
const PAGE_FRACTION: f64 = 0.875;

/// A thumb drag in progress.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Drag {
    /// The scrollbar as it was when the press grabbed its thumb.
    hit: ScrollbarHit,
    /// Where on the thumb the pointer took it (px from its start along the axis).
    grab: f64,
    /// Whether the drag moved the scroll offset.
    moved: bool,
}

/// The mouse input `m` of `kind` that hit `target`: whether it is the scrollbar's (a press on a
/// part of one of `target`'s scrollbars, a move or the release of the drag it started), which the
/// caller then gives no default action.
pub(crate) fn input(page: &Rc<PageDom>, kind: MouseKind, m: &MouseInput, target: NodeId) -> bool {
    match kind {
        // (Another button already down: a chord, not a press of the scrollbar.)
        MouseKind::Pressed if m.buttons & 1 != 0 && m.buttons & !1 == 0 => press(page, m, target),
        MouseKind::Moved => drag(page, m),
        MouseKind::Released if m.buttons & 1 == 0 => release(page),
        _ => false,
    }
}

fn along(hit: &ScrollbarHit, m: &MouseInput) -> f64 {
    if hit.axis == 1 { m.y } else { m.x }
}

fn press(page: &Rc<PageDom>, m: &MouseInput, target: NodeId) -> bool {
    // (A press that finds a drag still on, whose release was lost, ends it.)
    if page.scrollbar_drag.get().is_some() {
        release(page);
    }
    let hit = {
        let dom = page.dom.borrow();
        page.layout().scrollbar_hit(&dom, target, (m.x, m.y))
    };
    let Some(hit) = hit else { return false };
    // A user scroll interrupts a smooth scroll of the box.
    crate::smooth_scroll::cancel(page, hit.target);
    if hit.part == ScrollbarPart::Thumb {
        page.scrollbar_drag.set(Some(Drag { hit, grab: along(&hit, m) - hit.thumb_start, moved: false }));
        page.layout.borrow_mut().set_snap_suspended(true);
        return true;
    }
    let page_step = (hit.client * PAGE_FRACTION).max(hit.client - PAGE_OVERLAP).floor().max(1.0);
    let delta = match hit.part {
        ScrollbarPart::ButtonStart => -LINE_STEP,
        ScrollbarPart::ButtonEnd => LINE_STEP,
        ScrollbarPart::TrackBefore => -page_step,
        ScrollbarPart::TrackAfter => page_step,
        ScrollbarPart::Thumb => 0.0,
    } as i32;
    let (dx, dy) = if hit.axis == 1 { (None, Some(delta)) } else { (Some(delta), None) };
    let moved = {
        let dom = page.dom.borrow();
        let mut layout = page.layout.borrow_mut();
        match hit.target {
            Some(el) => layout.element_scroll_by(&dom, el, dx, dy),
            None => layout.window_scroll_by(&dom, dx, dy),
        }
    };
    if moved {
        crate::layout_api::fire_scroll(page, hit.target);
    }
    true
}

fn drag(page: &Rc<PageDom>, m: &MouseInput) -> bool {
    let Some(mut d) = page.scrollbar_drag.get() else { return false };
    // (The button was released where the page did not see it: the drag is over.)
    if m.buttons & 1 == 0 {
        release(page);
        return false;
    }
    let hit = d.hit;
    let travel = hit.room - hit.thumb_len;
    if travel <= 0.0 || hit.range <= 0.0 {
        return true;
    }
    // The thumb follows the pointer along its track: its start's share of the room it can move in
    // is the offset's share of the scroll range.
    let progress = ((along(&hit, m) - d.grab - hit.room_start) / travel).clamp(0.0, 1.0);
    let offset = (progress * hit.range - if hit.reversed { hit.range } else { 0.0 }).round() as i32;
    let (left, top) = if hit.axis == 1 { (None, Some(offset)) } else { (Some(offset), None) };
    let moved = {
        let dom = page.dom.borrow();
        let mut layout = page.layout.borrow_mut();
        match hit.target {
            Some(el) => layout.element_scroll_to(&dom, el, left, top),
            None => layout.window_scroll_to(&dom, left, top),
        }
    };
    if moved {
        d.moved = true;
        page.scrollbar_drag.set(Some(d));
        crate::layout_api::queue_scroll(page, hit.target, false);
    }
    true
}

/// Ends a thumb drag: the scroll snaps (to the position nearest where the thumb left it) and
/// `scrollend` follows. Whether there was one.
fn release(page: &Rc<PageDom>) -> bool {
    let Some(d) = page.scrollbar_drag.take() else { return false };
    let target = d.hit.target;
    let snapped = {
        let dom = page.dom.borrow();
        let mut layout = page.layout.borrow_mut();
        layout.set_snap_suspended(false);
        let (x, y) = layout.scroll_position(target);
        match target {
            Some(el) => layout.element_scroll_to(&dom, el, Some(x), Some(y)),
            None => layout.window_scroll_to(&dom, Some(x), Some(y)),
        }
    };
    if d.moved || snapped {
        crate::layout_api::fire_scroll(page, target);
    }
    true
}
