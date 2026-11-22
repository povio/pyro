//! The popover API, dialogs and invokers (HTML "popover", "the dialog
//! element", "the button element" command steps):
//!
//! - `showPopover`/`hidePopover`/`togglePopover` with `beforetoggle`
//!   (cancelable when showing) and a coalesced, asynchronous `toggle`
//!   (`ToggleEvent`), the auto and hint popover stacks (showing one hides
//!   the others that are not its ancestors), popover focusing steps and
//!   focus restoration;
//! - light dismiss (a click outside the auto popovers, outside a
//!   `closedby=any` dialog) and close requests (Escape) through a close
//!   watcher list of open dialogs and auto/hint popovers;
//! - `dialog.show()`/`showModal()`/`close()`/`requestClose()`, `closedBy`,
//!   the dialog focusing steps, focus restoration and the `toggle`
//!   events Chrome fires for dialogs;
//! - `popovertarget`/`popovertargetaction` buttons and `command`/`commandfor`
//!   invokers with `CommandEvent`;
//! - `<details>` toggle events, queued by pyro-dom at mutation time.
//!
//! Every entry point is reached only when a page uses one of these
//! features: nothing here runs on page or realm creation, and the
//! per-event/per-mutation paths check an empty list first.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use pyro_dom::forms::InputType;
use pyro_dom::{Dom, NodeId, PopoverStack, PopoverType};
use pyro_js::{ClassDef, JsClass, JsError, JsResult};
use rustc_hash::{FxHashMap, FxHashSet};

use pyro_dom::dom::InvokerTarget;

use crate::event::{EventCore, EventInterface};
use crate::node::{node_value, NodeData};
use crate::page::PageDom;
use crate::util::*;

/// A `toggle` event task waiting to run (HTML "toggle task tracker").
struct PendingToggle {
    old_open: bool,
    new_open: bool,
    source: Option<NodeId>,
    /// The queued task that fires it (a later toggle cancels it and queues its own).
    seq: u64,
}

/// Per-page state of popovers, dialogs and invokers.
#[derive(Default)]
pub(crate) struct PopoverState {
    /// Pending `toggle` events by element and kind: a dialog's (`true`) are coalesced apart from
    /// its popover's (HTML gives each its own toggle task tracker).
    toggles: RefCell<FxHashMap<(NodeId, bool), PendingToggle>>,
    /// Numbers the queued toggle tasks (`PendingToggle::seq`).
    toggle_seq: Cell<u64>,
    /// The element focused when a dialog or an auto/hint popover was shown.
    previously_focused: RefCell<FxHashMap<NodeId, NodeId>>,
    /// Popovers being shown or hidden ("popover showing or hiding").
    busy: RefCell<FxHashSet<NodeId>>,
    /// The close watchers: dialogs opened with `show()`/`showModal()` and
    /// showing auto/hint popovers, in the order they opened.
    watchers: RefCell<Vec<NodeId>>,
    /// HTML "popover pointerdown target" and the dialog one.
    pointerdown_popover: Cell<Option<NodeId>>,
    pointerdown_dialog: Cell<Option<NodeId>>,
    /// An element the focusing steps focus although it is not focusable
    /// (a dialog without a focusable descendant).
    forced_focus: Cell<Option<NodeId>>,
}

impl PopoverState {
    /// Whether `el` is the element the dialog focusing steps focus now.
    pub(crate) fn forced_focus(&self, el: NodeId) -> bool {
        self.forced_focus.get() == Some(el)
    }
}

// ----- helpers ---------------------------------------------------------------

fn is_html(dom: &Dom, el: NodeId, local: &str) -> bool {
    dom.element(el).is_ok_and(|e| e.is_html() && &*e.name.local == local)
}

/// The `popover` attribute's state: missing is none, empty or `auto` is
/// auto, an invalid value is manual.
fn popover_type(dom: &Dom, el: NodeId) -> Option<PopoverType> {
    dom.get_attribute(el, "popover").ok().flatten().map(PopoverType::from_value)
}

fn stack_of(ty: Option<PopoverType>) -> Option<PopoverStack> {
    match ty {
        Some(PopoverType::Auto) => Some(PopoverStack::Auto),
        Some(PopoverType::Hint) => Some(PopoverStack::Hint),
        _ => None,
    }
}

fn is_open(dom: &Dom, el: NodeId) -> bool {
    dom.has_attribute(el, "open").unwrap_or(false)
}

/// The flat-tree parent, approximated: the parent node, or a shadow root's host.
fn parent_or_host(dom: &Dom, n: NodeId) -> Option<NodeId> {
    match dom.parent(n).ok().flatten() {
        Some(p) if dom.element(p).is_ok() => Some(p),
        Some(p) => dom.fragment_host(p).ok().flatten(),
        None => None,
    }
}

fn state_name(open: bool) -> &'static str {
    if open {
        "open"
    } else {
        "closed"
    }
}

/// Fires a `ToggleEvent` (`beforetoggle` or `toggle`) at `el`; false when
/// it was canceled.
fn fire_toggle(
    page: &Rc<PageDom>,
    el: NodeId,
    ty: &str,
    old: bool,
    new: bool,
    cancelable: bool,
    source: Option<NodeId>,
) -> bool {
    let (key, source) = {
        let dom = page.dom.borrow();
        (dom.node_key(el), source.and_then(|s| dom.node_key(s)))
    };
    let Some(key) = key else { return true };
    let ev = EventCore::trusted(page, EventInterface::ToggleEvent, ty, false, cancelable);
    {
        let mut f = ev.fields.borrow_mut();
        f.old_state = state_name(old).into();
        f.new_state = state_name(new).into();
        f.submitter = source;
    }
    crate::target::fire(page, key, &ev)
}

/// HTML "queue a popover toggle event task" (and the details and dialog
/// ones): a later change before the task ran keeps the first old state and
/// takes the new one.
pub(crate) fn queue_toggle(page: &Rc<PageDom>, el: NodeId, old_open: bool, new_open: bool, source: Option<NodeId>) {
    queue_toggle_of(page, el, false, old_open, new_open, source);
}

/// [`queue_toggle`] for a dialog's open state (its dialog toggle task tracker).
fn queue_dialog_toggle(page: &Rc<PageDom>, el: NodeId, old_open: bool, new_open: bool, source: Option<NodeId>) {
    queue_toggle_of(page, el, true, old_open, new_open, source);
}

fn queue_toggle_of(
    page: &Rc<PageDom>,
    el: NodeId,
    dialog: bool,
    old_open: bool,
    new_open: bool,
    source: Option<NodeId>,
) {
    // HTML "queue a ... toggle event task": a pending one is cancelled, its old state kept, and
    // the event queued again, after the tasks queued since.
    let seq = page.popover.toggle_seq.get().wrapping_add(1);
    page.popover.toggle_seq.set(seq);
    {
        let mut toggles = page.popover.toggles.borrow_mut();
        let old_open = toggles.get(&(el, dialog)).map_or(old_open, |t| t.old_open);
        toggles.insert((el, dialog), PendingToggle { old_open, new_open, source, seq });
    }
    let weak = Rc::downgrade(page);
    page.post_task(Box::new(move || {
        let Some(page) = weak.upgrade() else { return };
        let t = {
            let mut toggles = page.popover.toggles.borrow_mut();
            if toggles.get(&(el, dialog)).is_none_or(|t| t.seq != seq) {
                return;
            }
            toggles.remove(&(el, dialog))
        };
        let Some(t) = t else { return };
        fire_toggle(&page, el, "toggle", t.old_open, t.new_open, false, t.source);
    }));
}

/// Queues the `toggle` events of the details elements pyro-dom recorded.
pub(crate) fn flush_details_toggles(page: &Rc<PageDom>) {
    let toggles = match page.dom.try_borrow_mut() {
        Ok(mut dom) if dom.has_details_toggles() => dom.take_details_toggles(page.document()),
        _ => return,
    };
    for (el, was_open) in toggles {
        let open = is_open(&page.dom.borrow(), el);
        queue_toggle(page, el, was_open, open, None);
        // The pending task keeps the node by id; a node collected first
        // fires nothing.
        page.dom.borrow_mut().release(el);
    }
}

fn focused(page: &PageDom) -> Option<NodeId> {
    page.dom.borrow().focused(page.document()).ok().flatten()
}

/// Focuses `el` even when it is not a focusable area (the dialog focusing
/// steps' fallback to the dialog itself).
fn focus_forced(page: &Rc<PageDom>, el: NodeId) {
    let previous = page.popover.forced_focus.replace(Some(el));
    crate::input::focus_element(page, el);
    page.popover.forced_focus.set(previous);
}

/// A focusable area that is being rendered.
fn focusable_rendered(page: &PageDom, el: NodeId) -> bool {
    let dom = page.dom.borrow();
    dom.is_focusable(el).unwrap_or(false) && page.layout().is_visible(&dom, el)
}

/// The descendants of `root` in tree order, `root` excluded.
fn descendants(dom: &Dom, root: NodeId) -> Vec<NodeId> {
    dom.descendants(root).map(|d| d.skip(1).filter(|&n| dom.element(n).is_ok()).collect()).unwrap_or_default()
}

/// HTML "autofocus delegate": the first focusable descendant with
/// `autofocus` (`root` itself when it has it).
fn autofocus_delegate(page: &PageDom, root: NodeId) -> Option<NodeId> {
    let candidates = {
        let dom = page.dom.borrow();
        if dom.has_attribute(root, "autofocus").unwrap_or(false) {
            return Some(root);
        }
        descendants(&dom, root)
            .into_iter()
            .filter(|&d| dom.has_attribute(d, "autofocus").unwrap_or(false))
            .collect::<Vec<_>>()
    };
    candidates.into_iter().find(|&d| focusable_rendered(page, d))
}

// ----- popover validity, show and hide ----------------------------------------

/// HTML "check popover validity": whether showing (`expect_showing`
/// false) or hiding (true) `el` has anything to do; with `throw`, the
/// errors are exceptions, otherwise false.
pub(crate) fn check_popover_validity(page: &PageDom, el: NodeId, expect_showing: bool, throw: bool) -> JsResult<bool> {
    let dom = page.dom.borrow();
    let fail = |name: &'static str, message: &str| if throw { Err(JsError::dom(name, message)) } else { Ok(false) };
    if popover_type(&dom, el).is_none() {
        return fail(
            "NotSupportedError",
            "Not supported on elements that do not have a valid value for the 'popover' attribute.",
        );
    }
    if dom.is_popover_showing(el) != expect_showing {
        return Ok(false);
    }
    if !dom.is_connected(el).unwrap_or(false) || dom.node_document(el).ok() != Some(page.document()) {
        return fail("InvalidStateError", "Invalid on disconnected popover elements.");
    }
    // HTML "check popover validity": a modal dialog (one shown by show() or the `open`
    // attribute may also be a popover) and an element in fullscreen cannot be shown as popovers.
    if !expect_showing && is_html(&dom, el, "dialog") && dom.is_modal(el) {
        return fail(
            "InvalidStateError",
            "The dialog is already open as a modal dialog, and therefore cannot be opened as a popover.",
        );
    }
    if !expect_showing && page.fullscreen_stack.borrow().contains(&el) {
        return fail("InvalidStateError", "The element is fullscreen, and therefore cannot be opened as a popover.");
    }
    Ok(true)
}

/// HTML "nearest inclusive open popover": the closest flat-tree inclusive
/// ancestor that is a showing auto or hint popover.
fn nearest_open_popover(dom: &Dom, node: NodeId) -> Option<NodeId> {
    let mut cur = Some(node);
    while let Some(n) = cur {
        if dom.is_popover_showing(n) && matches!(popover_type(dom, n), Some(PopoverType::Auto | PopoverType::Hint)) {
            return Some(n);
        }
        // A select's open picker is an auto popover holding the select's contents.
        if dom.is_select_picker_open(n) {
            return Some(n);
        }
        cur = parent_or_host(dom, n);
    }
    None
}

/// HTML "nearest inclusive target popover for invoker": the showing
/// popover a flat-tree inclusive ancestor of `node` invokes.
fn nearest_invoked_popover(dom: &Dom, node: NodeId) -> Option<NodeId> {
    let mut cur = Some(node);
    while let Some(n) = cur {
        let target = popover_target_with(dom, n)
            // A disabled command button invokes nothing (as a disabled popovertarget one).
            .or_else(|| command_for_element(dom, n).filter(|_| !dom.is_disabled(n).unwrap_or(false)));
        if let Some(t) = target.filter(|&t| dom.is_popover_showing(t)) {
            if matches!(popover_type(dom, t), Some(PopoverType::Auto | PopoverType::Hint)) {
                return Some(t);
            }
        }
        cur = parent_or_host(dom, n);
    }
    None
}

/// Whether popover `inner` is nested in popover `outer`: `outer` is a flat-tree ancestor of it or
/// of one of its invokers, along the chain of invokers.
fn nested_in(dom: &Dom, inner: NodeId, outer: NodeId) -> bool {
    let mut current = Some(inner);
    let mut budget = 64;
    while let Some(popover) = current {
        let mut node = parent_or_host(dom, popover);
        while let Some(n) = node {
            if n == outer {
                return true;
            }
            node = parent_or_host(dom, n);
        }
        let invoker = dom.popover_invoker(popover);
        if invoker.is_some_and(|i| i == outer || dom.contains(outer, i).unwrap_or(false)) {
            return true;
        }
        budget -= 1;
        current = invoker.and_then(|i| nearest_open_popover(dom, i)).filter(|&p| p != popover && budget > 0);
    }
    false
}

/// The popover a dialog or fullscreen element entering the top layer keeps open, with those
/// below it (HTML "show a modal dialog", Fullscreen "requestFullscreen()"): its topmost popover
/// ancestor in the hint stack, else in the auto stack.
fn top_layer_popover_ancestor(dom: &Dom, el: NodeId) -> Option<NodeId> {
    topmost_popover_ancestor(dom, el, PopoverStack::Hint, None)
        .or_else(|| topmost_popover_ancestor(dom, el, PopoverStack::Auto, None))
}

/// HTML "topmost popover ancestor": the highest popover of `stack` that
/// contains `el` (flat tree) or its invoker.
fn topmost_popover_ancestor(dom: &Dom, el: NodeId, stack: PopoverStack, invoker: Option<NodeId>) -> Option<NodeId> {
    let list = dom.popover_stack(stack);
    let mut best: Option<(usize, NodeId)> = None;
    for candidate in [parent_or_host(dom, el), invoker].into_iter().flatten() {
        let Some(anc) = nearest_open_popover(dom, candidate) else { continue };
        if let Some(pos) = list.iter().position(|&p| p == anc) {
            if best.is_none_or(|(b, _)| b < pos) {
                best = Some((pos, anc));
            }
        }
    }
    best.map(|(_, p)| p)
}

/// Hides the popovers of `stack` above `endpoint` (all of them when it is
/// not in the stack), topmost first.
///
/// It ends whatever the page's script does (a page must never spin the browser):
/// - The popovers are hidden without moving focus; with `focus_previous`, focus goes back once,
///   after the last of them is hidden, to where it was before that one (the lowest) opened, if
///   it is in one of them. A focus handler that opens another popover each time focus comes
///   back can then open one, not keep this loop going.
/// - Events fire for the popovers above `endpoint` when it started; one an event handler opened
///   since is hidden without them (HTML's `repeatingHide`).
/// - Past a budget of hides, the rest are taken out without events.
/// - One that a handler kept showing, or that hide_popover does not act on (a node from another
///   document, one left on the stack), is taken out regardless.
fn close_stack_until(
    page: &Rc<PageDom>,
    stack: PopoverStack,
    endpoint: Option<NodeId>,
    focus_previous: bool,
    fire_events: bool,
) {
    let endpoint = endpoint.filter(|e| page.dom.borrow().popover_stack(stack).contains(e));
    let above = |page: &Rc<PageDom>| {
        let dom = page.dom.borrow();
        let list = dom.popover_stack(stack);
        let from = endpoint.and_then(|e| list.iter().position(|&p| p == e)).map_or(0, |i| i + 1);
        list[from..].to_vec()
    };
    let initial = above(page);
    if initial.is_empty() {
        return;
    }
    let mut budget = initial.len() * 2 + 16;
    // The popovers hidden, and their previously focused elements (the last hidden's is where
    // focus goes back).
    let mut hidden: Vec<NodeId> = Vec::new();
    let mut restore: Option<NodeId> = None;
    loop {
        let top = page.dom.borrow().popover_stack(stack).last().copied();
        let Some(top) = top.filter(|&t| Some(t) != endpoint) else { break };
        if budget == 0 {
            // A script that keeps opening popovers as these close: the rest go without events.
            for el in above(page).into_iter().rev() {
                page.popover.previously_focused.borrow_mut().remove(&el);
                page.dom.borrow_mut().drop_popover(el);
                remove_watcher(page, el);
                hidden.push(el);
            }
            break;
        }
        budget -= 1;
        if page.dom.borrow().is_select_picker_open(top) {
            hide_select_picker(page, top, false);
            hidden.push(top);
            continue;
        }
        let fire = fire_events && initial.contains(&top);
        let previous = page.popover.previously_focused.borrow().get(&top).copied();
        let _ = hide_popover(page, top, false, fire, false, None);
        if page.dom.borrow().popover_stack(stack).last() == Some(&top) {
            page.dom.borrow_mut().drop_popover(top);
            remove_watcher(page, top);
        }
        hidden.push(top);
        if previous.is_some() {
            restore = previous;
        }
    }
    // HTML "hide popover" step: focus goes back to the previously focused element when it is
    // in a popover hidden here (once, for the last one).
    if let (true, Some(prev)) = (focus_previous, restore) {
        let inside = focused(page).is_none_or(|f| {
            let dom = page.dom.borrow();
            hidden.iter().any(|&el| dom.contains(el, f).unwrap_or(false))
        });
        if inside {
            crate::input::focus_element_with_options(page, prev, true);
        }
    }
}

/// HTML "hide all popovers until": `endpoint` in the hint stack closes
/// the hints above it; anything else closes every hint and the autos
/// above it (all of them for the document, `None`).
fn hide_all_popovers_until(page: &Rc<PageDom>, endpoint: Option<NodeId>, focus_previous: bool, fire_events: bool) {
    let in_hint = endpoint.is_some_and(|e| page.dom.borrow().popover_stack(PopoverStack::Hint).contains(&e));
    close_stack_until(page, PopoverStack::Hint, endpoint.filter(|_| in_hint), focus_previous, fire_events);
    let auto_endpoint = match endpoint {
        // A hint endpoint keeps the auto popovers it is nested in (shown inside or invoked
        // from), and the ones below them; the autos above close (whatwg/html#12304).
        Some(hint) if in_hint => {
            let dom = page.dom.borrow();
            topmost_popover_ancestor(&dom, hint, PopoverStack::Auto, dom.popover_invoker(hint))
        }
        other => other,
    };
    close_stack_until(page, PopoverStack::Auto, auto_endpoint, focus_previous, fire_events);
}

/// Fullscreen "requestFullscreen()", as the element becomes fullscreen: the popovers that do
/// not contain it close (the topmost auto popover ancestor's stack above it, or all of them),
/// as for a modal dialog.
pub(crate) fn hide_popovers_for_fullscreen(page: &Rc<PageDom>, el: NodeId) {
    let until = top_layer_popover_ancestor(&page.dom.borrow(), el);
    hide_all_popovers_until(page, until, false, true);
}

fn remove_watcher(page: &PageDom, el: NodeId) {
    page.popover.watchers.borrow_mut().retain(|&w| w != el);
}

struct Busy<'a>(&'a PageDom, NodeId, bool);
impl Drop for Busy<'_> {
    fn drop(&mut self) {
        if self.2 {
            self.0.popover.busy.borrow_mut().remove(&self.1);
        }
    }
}

/// HTML "show popover".
pub(crate) fn show_popover(page: &Rc<PageDom>, el: NodeId, throw: bool, invoker: Option<NodeId>) -> JsResult<()> {
    if !check_popover_validity(page, el, false, throw)? {
        return Ok(());
    }
    let nested = !page.popover.busy.borrow_mut().insert(el);
    let _busy = Busy(page, el, !nested);
    let fire_events = !nested;
    // HTML "show popover": the document the popover was in before `beforetoggle` (its
    // "expected document").
    let original_document = page.dom.borrow().node_document(el).ok();
    if fire_events && !fire_toggle(page, el, "beforetoggle", false, true, true, invoker) {
        return Ok(());
    }
    // A `beforetoggle` handler that moved the popover to another document makes it invalid.
    if page.dom.borrow().node_document(el).ok() != original_document {
        if throw {
            return Err(JsError::dom("InvalidStateError", "The popover was moved to another document."));
        }
        return Ok(());
    }
    if !check_popover_validity(page, el, false, false)? {
        return Ok(());
    }
    let ty = popover_type(&page.dom.borrow(), el);
    // An auto popover inside a showing hint popover (or invoked from one) is shown as a hint: it
    // joins the hint stack above that ancestor (HTML: an auto popover nested in a hint is
    // downgraded to a hint).
    let mut downgraded = false;
    match ty {
        Some(PopoverType::Auto) => {
            let hint_ancestor = topmost_popover_ancestor(&page.dom.borrow(), el, PopoverStack::Hint, invoker);
            if hint_ancestor.is_some() {
                downgraded = true;
                close_stack_until(page, PopoverStack::Hint, hint_ancestor, false, fire_events);
            } else {
                let ancestor = topmost_popover_ancestor(&page.dom.borrow(), el, PopoverStack::Auto, invoker);
                hide_all_popovers_until(page, ancestor, false, fire_events);
            }
        }
        Some(PopoverType::Hint) => {
            let ancestor = topmost_popover_ancestor(&page.dom.borrow(), el, PopoverStack::Hint, invoker);
            close_stack_until(page, PopoverStack::Hint, ancestor, false, fire_events);
        }
        _ => {}
    }
    // The events may have changed the popover's type or state.
    if popover_type(&page.dom.borrow(), el) != ty {
        if throw {
            return Err(JsError::dom("InvalidStateError", "The popover type changed while showing the popover."));
        }
        return Ok(());
    }
    if !check_popover_validity(page, el, false, throw)? {
        return Ok(());
    }
    let previous = focused(page);
    let stack = if downgraded { Some(PopoverStack::Hint) } else { stack_of(ty) };
    overlay_style_flush(page, el);
    {
        let mut dom = page.dom.borrow_mut();
        dom.set_popover_showing(el, true).js()?;
        dom.set_popover_invoker(el, invoker);
        if let Some(stack) = stack {
            dom.push_popover_stack(el, stack);
        }
    }
    if stack.is_some() {
        // Once: an open dialog that is also shown as a popover is one watcher entry.
        let mut watchers = page.popover.watchers.borrow_mut();
        watchers.retain(|&w| w != el);
        watchers.push(el);
    }
    // Popover focusing steps.
    if let Some(control) = autofocus_delegate(page, el) {
        crate::input::focus_element(page, control);
    }
    if let Some(prev) = previous.filter(|_| stack.is_some() && page.dom.borrow().is_popover_showing(el)) {
        page.popover.previously_focused.borrow_mut().insert(el, prev);
    }
    queue_toggle(page, el, false, true, invoker);
    Ok(())
}

/// An element that may transition `overlay` (`animation_events::may_transition_overlay`) is
/// styled before it enters or leaves the top layer, as Chromium styles it there: that style is
/// the transition's before-change style. Whether it may.
fn overlay_style_flush(page: &Rc<PageDom>, el: NodeId) -> bool {
    let may = crate::animation_events::may_transition_overlay(page, el);
    if may {
        crate::animation_events::style_flush(page, Some(el));
    }
    may
}

/// HTML "hide popover".
pub(crate) fn hide_popover(
    page: &Rc<PageDom>,
    el: NodeId,
    focus_previous: bool,
    fire_events: bool,
    throw: bool,
    source: Option<NodeId>,
) -> JsResult<()> {
    if !check_popover_validity(page, el, true, throw)? {
        return Ok(());
    }
    hide_showing_popover(page, el, focus_previous, fire_events, source)
}

/// Run the closing steps even when the mutation that triggered them removed the attribute.
pub(crate) fn hide_showing_popover(
    page: &Rc<PageDom>,
    el: NodeId,
    focus_previous: bool,
    fire_events: bool,
    source: Option<NodeId>,
) -> JsResult<()> {
    let still_showing = || page.dom.borrow().is_popover_showing(el);
    if !still_showing() {
        return Ok(());
    }
    let nested = !page.popover.busy.borrow_mut().insert(el);
    let _busy = Busy(page, el, !nested);
    let fire_events = fire_events && !nested;
    // Stack membership records how the element opened, even after an attribute mutation.
    let stack = [PopoverStack::Auto, PopoverStack::Hint].into_iter()
        .find(|&stack| page.dom.borrow().popover_stack(stack).contains(&el));
    if let Some(stack) = stack {
        // The popovers above it in its stack close first.
        close_stack_until(page, stack, Some(el), focus_previous, fire_events);
        if !still_showing() {
            return Ok(());
        }
        // An auto popover's hidden with the hint popovers nested in it (shown inside it or
        // invoked from it); unrelated hints stay open.
        if stack == PopoverStack::Auto {
            let nested = {
                let dom = page.dom.borrow();
                let hints = dom.popover_stack(PopoverStack::Hint);
                hints.iter().position(|&h| nested_in(&dom, h, el)).map(|i| i.checked_sub(1).map(|j| hints[j]))
            };
            if let Some(endpoint) = nested {
                close_stack_until(page, PopoverStack::Hint, endpoint, focus_previous, fire_events);
                if !still_showing() {
                    return Ok(());
                }
            }
        }
    }
    page.dom.borrow_mut().set_popover_invoker(el, None);
    if fire_events {
        fire_toggle(page, el, "beforetoggle", true, false, false, source);
        if !still_showing() {
            return Ok(());
        }
        if let Some(stack) = stack.filter(|&stack| page.dom.borrow().popover_stack(stack).contains(&el)) {
            // A handler may have opened another popover above this one.
            close_stack_until(page, stack, Some(el), focus_previous, false);
        }
        if !still_showing() {
            return Ok(());
        }
        queue_toggle(page, el, true, false, source);
    }
    let keep = overlay_style_flush(page, el);
    {
        let mut dom = page.dom.borrow_mut();
        if keep {
            dom.keep_in_top_layer(el);
        }
        dom.set_popover_showing(el, false).js()?;
    }
    // A dialog still open (with show() or `open`) keeps its watcher.
    let open_dialog = {
        let dom = page.dom.borrow();
        is_html(&dom, el, "dialog") && is_open(&dom, el)
    };
    if !open_dialog {
        remove_watcher(page, el);
    }
    let previous = page.popover.previously_focused.borrow_mut().remove(&el);
    if let Some(prev) = previous.filter(|_| focus_previous) {
        let inside = focused(page).is_none_or(|f| page.dom.borrow().contains(el, f).unwrap_or(false));
        if inside {
            crate::input::focus_element_with_options(page, prev, true);
        }
    }
    Ok(())
}

/// `showPopover(options)` / `hidePopover()` / `togglePopover(options)`:
/// the `source` member of the options dictionary.
pub(crate) fn source_option<'s>(s: &pyro_js::Scope<'s>, v: pyro_js::Value<'s>) -> JsResult<Option<NodeId>> {
    let Some(o) = v.as_object() else { return Ok(None) };
    let source = o.get_atom("source")?;
    // Web IDL: `source` is a (non-nullable) HTMLElement: absent is none, null or anything that
    // is not an element throws.
    if source.is_undefined() {
        return Ok(None);
    }
    match s.unwrap::<crate::node::Node>(source) {
        Some(n) if n.page().dom.borrow().element(n.id).is_ok() => Ok(Some(n.id)),
        _ => Err(JsError::Type(
            "Failed to read the 'source' property: The provided value is not of type 'HTMLElement'.".into(),
        )),
    }
}

// ----- light dismiss and close requests --------------------------------------------

// ----- select pickers ------------------------------------------------------------------

/// Whether `select` shows a picker of its own: a customizable select (computed `appearance:
/// base-select`) rendering as a drop-down box (not `multiple`, display size 1), enabled and
/// connected.
pub(crate) fn has_select_picker(page: &PageDom, select: NodeId) -> bool {
    let dom = page.dom.borrow();
    if !is_html(&dom, select, "select")
        || !dom.is_connected(select).unwrap_or(false)
        || dom.is_disabled(select).unwrap_or(true)
    {
        return false;
    }
    if dom.has_attribute(select, "multiple").unwrap_or(false) {
        return false;
    }
    let size = dom.get_attribute(select, "size").ok().flatten().and_then(|s| s.trim().parse::<u32>().ok()).unwrap_or(1);
    size <= 1 && page.layout().computed_style_value(&dom, select, "appearance") == "base-select"
}

/// Opens a customizable select's picker. The picker is an auto popover: auto popovers that do
/// not hold the select close first, light dismiss, a close request, an auto popover or a dialog
/// shown later close it. The selected option takes the focus, as in Chromium.
pub(crate) fn show_select_picker(page: &Rc<PageDom>, select: NodeId) {
    if page.dom.borrow().is_select_picker_open(select) {
        return;
    }
    let ancestor = topmost_popover_ancestor(&page.dom.borrow(), select, PopoverStack::Auto, None);
    hide_all_popovers_until(page, ancestor, false, true);
    let previous = focused(page);
    if page.dom.borrow_mut().set_select_picker_open(select, true).is_err() {
        return;
    }
    page.popover.watchers.borrow_mut().retain(|&w| w != select);
    page.popover.watchers.borrow_mut().push(select);
    if let Some(prev) = previous {
        page.popover.previously_focused.borrow_mut().insert(select, prev);
    }
    let option = page.dom.borrow().selected_options(select).ok().and_then(|o| o.first().copied());
    if let Some(option) = option {
        focus_forced(page, option);
    }
}

/// Focuses an option of an open picker (one without a box of its own yet is focused too).
pub(crate) fn focus_picker_option(page: &Rc<PageDom>, option: NodeId) {
    focus_forced(page, option);
}

/// Closes a select's picker. With `focus_select`, focus inside the select goes back to it.
pub(crate) fn hide_select_picker(page: &Rc<PageDom>, select: NodeId, focus_select: bool) {
    if !page.dom.borrow().is_select_picker_open(select) {
        return;
    }
    let _ = page.dom.borrow_mut().set_select_picker_open(select, false);
    remove_watcher(page, select);
    page.popover.previously_focused.borrow_mut().remove(&select);
    let inside = focused(page).is_some_and(|f| f != select && page.dom.borrow().contains(select, f).unwrap_or(false));
    if focus_select || inside {
        crate::input::focus_element_with_options(page, select, true);
    }
}

/// A trusted left-button `mousedown` on `target` (not canceled): a press on a customizable
/// select's button (the select itself, or its first child button) opens its picker, or
/// closes it when open. A press inside the open picker does not.
pub(crate) fn select_picker_mousedown(page: &Rc<PageDom>, target: NodeId) {
    let select = {
        let dom = page.dom.borrow();
        let mut cur = Some(target);
        let mut found = None;
        while let Some(n) = cur {
            if is_html(&dom, n, "select") {
                found = Some(n);
                break;
            }
            cur = parent_or_host(&dom, n);
        }
        let Some(select) = found else { return };
        // The press is on the select's button: the select, or inside its first child button.
        let on_button = target == select || {
            let mut cur = Some(target);
            let mut button = false;
            while let Some(n) = cur.filter(|&n| n != select) {
                if is_html(&dom, n, "button") && dom.parent(n).ok().flatten() == Some(select) {
                    button = dom.element_children(select).ok().and_then(|mut c| c.next()) == Some(n);
                    break;
                }
                cur = parent_or_host(&dom, n);
            }
            button
        };
        if !on_button {
            return;
        }
        select
    };
    if page.dom.borrow().is_select_picker_open(select) {
        hide_select_picker(page, select, true);
    } else if has_select_picker(page, select) {
        show_select_picker(page, select);
    }
}

/// The auto/hint popover `node` is in or invokes that is highest in the
/// popover stacks ("topmost clicked popover").
fn topmost_clicked_popover(dom: &Dom, node: NodeId) -> Option<NodeId> {
    let position = |p: NodeId| {
        let auto = dom.popover_stack(PopoverStack::Auto);
        auto.iter()
            .position(|&e| e == p)
            .or_else(|| dom.popover_stack(PopoverStack::Hint).iter().position(|&e| e == p).map(|i| i + auto.len()))
    };
    let clicked = nearest_open_popover(dom, node);
    let invoked = nearest_invoked_popover(dom, node);
    match (clicked, invoked) {
        (Some(c), Some(i)) => Some(if position(c) >= position(i) { c } else { i }),
        (c, i) => c.or(i),
    }
}

/// The open dialog a pointer event at (`x`, `y`) on `target` is inside of
/// ("nearest clicked dialog"): none for a hit on a modal dialog's backdrop.
fn nearest_clicked_dialog(page: &PageDom, target: NodeId, x: f64, y: f64) -> Option<NodeId> {
    let dom = page.dom.borrow();
    let mut cur = Some(target);
    while let Some(n) = cur {
        if is_html(&dom, n, "dialog") && is_open(&dom, n) {
            if n == target {
                let r = page.layout().bounding_client_rect(&dom, n).layout_units();
                if x < r.left() || x > r.right() || y < r.top() || y > r.bottom() {
                    return None;
                }
            }
            return Some(n);
        }
        cur = parent_or_host(&dom, n);
    }
    None
}

/// HTML "light dismiss open popovers" and "light dismiss open dialogs"
/// for a trusted `pointerdown` (`down`) or `pointerup` at `target`.
pub(crate) fn light_dismiss(page: &Rc<PageDom>, target: NodeId, x: f64, y: f64, down: bool) {
    if page.popover.watchers.borrow().is_empty() {
        return;
    }
    forget_closed_watchers(page);
    let has_popovers = {
        let dom = page.dom.borrow();
        !dom.popover_stack(PopoverStack::Auto).is_empty() || !dom.popover_stack(PopoverStack::Hint).is_empty()
    };
    if has_popovers {
        let ancestor = topmost_clicked_popover(&page.dom.borrow(), target);
        if down {
            page.popover.pointerdown_popover.set(ancestor);
        } else if page.popover.pointerdown_popover.take() == ancestor {
            hide_all_popovers_until(page, ancestor, false, true);
        }
    }
    let topmost = {
        let dom = page.dom.borrow();
        let watchers = page.popover.watchers.borrow();
        watchers.iter().rev().copied().find(|&w| is_html(&dom, w, "dialog") && is_open(&dom, w))
    };
    let Some(topmost) = topmost.filter(|&d| closed_by(&page.dom.borrow(), d) == ClosedBy::Any) else {
        page.popover.pointerdown_dialog.set(None);
        return;
    };
    let ancestor = nearest_clicked_dialog(page, target, x, y);
    if down {
        page.popover.pointerdown_dialog.set(ancestor);
    } else if page.popover.pointerdown_dialog.take() == ancestor && ancestor != Some(topmost) {
        request_close_dialog(page, topmost, None, None);
    }
}

/// Forgets the close watchers whose element closed some other way (removed
/// from its document, `open` removed).
fn forget_closed_watchers(page: &PageDom) {
    let dom = page.dom.borrow();
    page.popover.watchers.borrow_mut().retain(|&w| {
        dom.is_alive(w)
            && dom.is_connected(w).unwrap_or(false)
            && if is_html(&dom, w, "dialog") && !dom.is_popover_showing(w) {
                is_open(&dom, w)
            } else {
                dom.is_popover_showing(w) || dom.is_select_picker_open(w)
            }
    });
}

/// A close request (Escape): the newest close watcher that listens acts —
/// an auto or hint popover hides, a dialog gets `cancel` and closes.
/// Returns whether a watcher handled it.
pub(crate) fn close_request(page: &Rc<PageDom>) -> bool {
    if page.popover.watchers.borrow().is_empty() {
        return false;
    }
    forget_closed_watchers(page);
    let candidate = {
        let dom = page.dom.borrow();
        let watchers = page.popover.watchers.borrow();
        watchers.iter().rev().copied().find(|&w| {
            dom.is_popover_showing(w) || dom.is_select_picker_open(w) || closed_by(&dom, w) != ClosedBy::None
        })
    };
    let Some(w) = candidate else { return false };
    if page.dom.borrow().is_select_picker_open(w) {
        hide_select_picker(page, w, true);
    } else if page.dom.borrow().is_popover_showing(w) {
        let _ = hide_popover(page, w, true, true, false, None);
        if page.dom.borrow().is_popover_showing(w) {
            remove_watcher(page, w);
        }
    } else {
        request_close_dialog(page, w, None, None);
    }
    true
}

// ----- dialogs -----------------------------------------------------------------------

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum ClosedBy {
    Any,
    CloseRequest,
    None,
}

/// The dialog's computed closed-by state: the attribute's keyword, else
/// close requests for a modal dialog and nothing for any other.
fn closed_by(dom: &Dom, el: NodeId) -> ClosedBy {
    match dom.get_attribute(el, "closedby").ok().flatten().map(str::to_ascii_lowercase).as_deref() {
        Some("any") => ClosedBy::Any,
        Some("closerequest") => ClosedBy::CloseRequest,
        Some("none") => ClosedBy::None,
        _ if dom.is_modal(el) => ClosedBy::CloseRequest,
        _ => ClosedBy::None,
    }
}

/// HTML "dialog focusing steps": the autofocus delegate, else the first
/// focusable descendant being rendered, else the dialog itself.
fn dialog_focusing_steps(page: &Rc<PageDom>, dialog: NodeId) {
    if page.dom.borrow().is_inert(dialog) {
        return;
    }
    if let Some(control) = autofocus_delegate(page, dialog) {
        if control != dialog {
            crate::input::focus_element(page, control);
            return;
        }
    }
    let first = {
        let list = descendants(&page.dom.borrow(), dialog);
        list.into_iter().find(|&d| {
            let tabbable = page
                .dom
                .borrow()
                .get_attribute(d, "tabindex")
                .ok()
                .flatten()
                .and_then(|t| t.trim().parse::<i32>().ok())
                .is_none_or(|t| t >= 0);
            tabbable && focusable_rendered(page, d)
        })
    };
    match first {
        Some(f) => crate::input::focus_element(page, f),
        None => focus_forced(page, dialog),
    }
}

fn after_dialog_open(page: &Rc<PageDom>, dialog: NodeId, previous: Option<NodeId>) {
    if let Some(prev) = previous {
        page.popover.previously_focused.borrow_mut().insert(dialog, prev);
    }
    {
        let mut watchers = page.popover.watchers.borrow_mut();
        watchers.retain(|&w| w != dialog);
        watchers.push(dialog);
    }
    dialog_focusing_steps(page, dialog);
}

/// `dialog.show()`.
fn show_dialog(page: &Rc<PageDom>, el: NodeId) -> JsResult<()> {
    {
        let dom = page.dom.borrow();
        if is_open(&dom, el) {
            if !dom.is_modal(el) {
                return Ok(());
            }
            return Err(JsError::dom(
                "InvalidStateError",
                "The dialog is already open as a modal dialog, and therefore cannot be opened as a non-modal dialog.",
            ));
        }
    }
    if !fire_toggle(page, el, "beforetoggle", false, true, true, None) || is_open(&page.dom.borrow(), el) {
        return Ok(());
    }
    queue_dialog_toggle(page, el, false, true, None);
    let previous = focused(page);
    page.dom.borrow_mut().set_attribute(el, "open", "").js()?;
    // Popovers that do not contain the dialog close, as for showModal().
    let until = top_layer_popover_ancestor(&page.dom.borrow(), el);
    hide_all_popovers_until(page, until, false, true);
    after_dialog_open(page, el, previous);
    Ok(())
}

/// `dialog.showModal()`; with `throw` false (the `show-modal` command),
/// the invalid states do nothing.
fn show_modal_dialog(page: &Rc<PageDom>, el: NodeId, throw: bool, source: Option<NodeId>) -> JsResult<()> {
    let fail = |name: &'static str, message: &str| if throw { Err(JsError::dom(name, message)) } else { Ok(()) };
    {
        let dom = page.dom.borrow();
        if is_open(&dom, el) {
            if dom.is_modal(el) {
                return Ok(());
            }
            return fail(
                "InvalidStateError",
                "The dialog is already open as a non-modal dialog, and therefore cannot be opened as a modal dialog.",
            );
        }
        if dom.is_popover_showing(el) {
            return fail(
                "InvalidStateError",
                "The dialog is already open as a Popover, and therefore cannot be opened as a modal dialog.",
            );
        }
        if !dom.is_connected(el).unwrap_or(false) {
            return fail("InvalidStateError", "The element is not in a Document.");
        }
        if !in_active_document(page, &dom, el) {
            return fail("InvalidStateError", "The element's document is not fully active.");
        }
    }
    if !fire_toggle(page, el, "beforetoggle", false, true, true, source) {
        return Ok(());
    }
    {
        let dom = page.dom.borrow();
        if is_open(&dom, el) || !dom.is_connected(el).unwrap_or(false) || dom.is_popover_showing(el) {
            return Ok(());
        }
    }
    queue_dialog_toggle(page, el, false, true, source);
    let previous = focused(page);
    overlay_style_flush(page, el);
    {
        let mut dom = page.dom.borrow_mut();
        dom.set_attribute(el, "open", "").js()?;
        // `:modal`, the top layer and the document's inertness until the
        // dialog closes or leaves its document.
        dom.set_modal(el, true).js()?;
    }
    // Popovers that do not contain the dialog close.
    let until = top_layer_popover_ancestor(&page.dom.borrow(), el);
    hide_all_popovers_until(page, until, false, true);
    after_dialog_open(page, el, previous);
    Ok(())
}

/// HTML "close the dialog" with `result` (None leaves `returnValue`).
pub(crate) fn close_dialog(page: &Rc<PageDom>, el: NodeId, result: Option<String>, source: Option<NodeId>) {
    if !is_open(&page.dom.borrow(), el) {
        return;
    }
    fire_toggle(page, el, "beforetoggle", true, false, false, source);
    if !is_open(&page.dom.borrow(), el) {
        return;
    }
    queue_dialog_toggle(page, el, true, false, source);
    let was_modal = page.dom.borrow().is_modal(el);
    let keep = overlay_style_flush(page, el);
    {
        let mut dom = page.dom.borrow_mut();
        if keep {
            dom.keep_in_top_layer(el);
        }
        let _ = dom.remove_attribute(el, "open");
        let _ = dom.set_modal(el, false);
    }
    if let Some(r) = result {
        page.set_dialog_return(el, r);
    }
    // A dialog still shown as a popover keeps its watcher.
    if !page.dom.borrow().is_popover_showing(el) {
        remove_watcher(page, el);
    }
    let previous = page.popover.previously_focused.borrow_mut().remove(&el);
    // The focused element, when it is in the dialog in the flat tree (a shadow tree inside it, an
    // element slotted into it).
    let focused_inside = focused(page).filter(|&f| {
        let dom = page.dom.borrow();
        let mut node = Some(f);
        while let Some(n) = node {
            if n == el {
                return true;
            }
            node = crate::input::flat_parent_element(&dom, n);
        }
        false
    });
    if let Some(prev) = previous {
        // HTML "close the dialog": focus goes back when the focused area is in the dialog, or
        // always for a dialog that was modal.
        if was_modal || focused(page).is_none() || focused_inside.is_some() {
            crate::input::focus_element_with_options(page, prev, true);
        }
    } else if let Some(f) = focused_inside {
        // No previously focused element: focus does not stay inside the closed dialog (the focus
        // fixup of its now unrendered element), it goes back to the body, as in Chrome.
        crate::input::blur_element(page, f);
    }
    crate::input::queue_simple(page, el, "close", false, false);
}

/// `dialog.requestClose(result)` and a close request: a cancelable
/// `cancel`, then the dialog closes unless it was canceled.
fn request_close_dialog(page: &Rc<PageDom>, el: NodeId, result: Option<String>, source: Option<NodeId>) {
    {
        let dom = page.dom.borrow();
        // Only a connected dialog of a fully active document has a close watcher.
        if !is_open(&dom, el) || !dom.is_connected(el).unwrap_or(false) || !in_active_document(page, &dom, el) {
            return;
        }
    }
    // HTML close watcher "is running cancel action": a request made from the dialog's own
    // `cancel` listener neither fires another `cancel` nor closes it.
    let key = (page.id(), el);
    if CANCELING.with(|c| c.borrow().contains(&key)) {
        return;
    }
    CANCELING.with(|c| c.borrow_mut().push(key));
    let proceed = crate::input::fire_simple(page, el, "cancel", false, true);
    CANCELING.with(|c| c.borrow_mut().retain(|k| *k != key));
    if !proceed {
        return;
    }
    close_dialog(page, el, result, source);
}

thread_local! {
    /// Dialogs whose close request is firing `cancel` (page id, element).
    static CANCELING: std::cell::RefCell<Vec<(u64, NodeId)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Whether `el` is in the page's own document, the one its browsing context shows, and that
/// document is fully active (a document made by `createHTMLDocument`, a removed iframe's or one
/// navigated away from is not).
fn in_active_document(page: &PageDom, dom: &Dom, el: NodeId) -> bool {
    page.is_fully_active() && dom.node_document(el).is_ok_and(|d| d == page.document())
}

/// `<form method=dialog>` submission: closes the form's nearest `dialog`
/// ancestor (not across a shadow root) with the submitter's result (an
/// image button's selected coordinate, else its value); no entry list, no
/// navigation.
pub(crate) fn submit_dialog(page: &Rc<PageDom>, form: NodeId, submitter: Option<NodeId>) {
    let dialog = {
        let dom = page.dom.borrow();
        let mut cur = dom.parent_element(form).ok().flatten();
        while let Some(n) = cur.filter(|&n| !is_html(&dom, n, "dialog")) {
            cur = dom.parent_element(n).ok().flatten();
        }
        cur
    };
    if let Some(d) = dialog.filter(|&d| is_open(&page.dom.borrow(), d)) {
        let result = crate::formdata::dialog_result(page, submitter);
        close_dialog(page, d, result, submitter);
    }
}

/// Whether `form`'s submission (with `submitter`) uses `method=dialog`.
pub(crate) fn is_dialog_method(dom: &Dom, form: NodeId, submitter: Option<NodeId>) -> bool {
    crate::formdata::is_dialog_method(dom, form, submitter)
}

/// The `HTMLDialogElement` members.
pub(crate) fn define_dialog<C: JsClass<Data = NodeData>>(def: &mut ClassDef<C>) {
    def.method("show", 0, |s, n, _a| {
        show_dialog(n.page(), n.id)?;
        Ok(s.undefined())
    });
    def.method("showModal", 0, |s, n, _a| {
        show_modal_dialog(n.page(), n.id, true, None)?;
        Ok(s.undefined())
    });
    def.method("close", 0, |s, n, a| {
        let result = if a.get(0).is_undefined() { None } else { Some(dom_string(a.get(0))?) };
        close_dialog(n.page(), n.id, result, None);
        Ok(s.undefined())
    });
    def.method("requestClose", 0, |s, n, a| {
        let result = if a.get(0).is_undefined() { None } else { Some(dom_string(a.get(0))?) };
        request_close_dialog(n.page(), n.id, result, None);
        Ok(s.undefined())
    });
    def.accessor(
        "closedBy",
        |s, n| {
            let dom = n.page().dom.borrow();
            Ok(s.string(match closed_by(&dom, n.id) {
                ClosedBy::Any => "any",
                ClosedBy::CloseRequest => "closerequest",
                ClosedBy::None => "none",
            }))
        },
        |_s, n, v| {
            let v = dom_string(v)?;
            n.page().dom.borrow_mut().set_attribute(n.id, "closedby", &v).js()
        },
    );
}

// ----- invokers ------------------------------------------------------------------------

/// An element of the PopoverInvokerElement mixin's types: a `button`, or an
/// `input` in the Submit, Reset, Image or Button state.
fn is_invoker_type(dom: &Dom, el: NodeId) -> bool {
    match dom.element(el) {
        Ok(e) if e.is_html() && &*e.name.local == "button" => true,
        Ok(e) if e.is_html() && &*e.name.local == "input" => {
            matches!(
                dom.input_type(el),
                Ok(InputType::Submit | InputType::Reset | InputType::Image | InputType::Button)
            )
        }
        _ => false,
    }
}

/// HTML "submit button" for a `button`: `type=submit`, or a missing or
/// invalid `type` without `command`/`commandfor` (the Auto state).
pub(crate) fn button_is_submit(dom: &Dom, el: NodeId) -> bool {
    dom.button_is_submit(el)
}

fn is_submit_button(dom: &Dom, el: NodeId) -> bool {
    if is_html(dom, el, "button") {
        return button_is_submit(dom, el);
    }
    matches!(dom.input_type(el), Ok(InputType::Submit | InputType::Image))
}

/// A reset button: `<input type=reset>` or `<button type=reset>`.
fn is_reset_button(dom: &Dom, el: NodeId) -> bool {
    if is_html(dom, el, "button") {
        return dom.get_attribute(el, "type").ok().flatten().is_some_and(|t| t.eq_ignore_ascii_case("reset"));
    }
    matches!(dom.input_type(el), Ok(InputType::Reset))
}

/// HTML "popover target element" of `node`.
fn popover_target_with(dom: &Dom, node: NodeId) -> Option<NodeId> {
    if !dom.has_attribute(node, "popovertarget").unwrap_or(false) || !is_invoker_type(dom, node) {
        return None;
    }
    if dom.is_disabled(node).unwrap_or(false) {
        return None;
    }
    // A submit or reset button with a form owner submits or resets the form instead (as
    // Chromium: a reset button of a form does not toggle its popover either).
    if dom.form_owner(node).ok().flatten().is_some() && (is_submit_button(dom, node) || is_reset_button(dom, node)) {
        return None;
    }
    dom.invoker_target(node, InvokerTarget::PopoverTarget).ok().flatten()
        .and_then(|target| dom.resolve_reference_target(target).ok().flatten())
        .filter(|&t| popover_type(dom, t).is_some())
}

fn command_for_element(dom: &Dom, node: NodeId) -> Option<NodeId> {
    if !is_html(dom, node, "button") || !dom.has_attribute(node, "commandfor").unwrap_or(false) {
        return None;
    }
    dom.invoker_target(node, InvokerTarget::CommandFor).ok().flatten()
        .and_then(|target| dom.resolve_reference_target(target).ok().flatten())
}

fn popover_target_of(page: &PageDom, node: NodeId) -> Option<NodeId> {
    let dom = page.dom.borrow();
    popover_target_with(&dom, node)
}

/// HTML "popover target attribute activation behavior" of `node` for a
/// click whose target is `event_target`.
pub(crate) fn popover_target_activation(page: &Rc<PageDom>, node: NodeId, event_target: NodeId) {
    let Some(popover) = popover_target_of(page, node) else { return };
    let (showing, action) = {
        let dom = page.dom.borrow();
        // A click inside a popover nested in its own invoker does nothing (a descendant, not the
        // invoker itself: a button that is its own popover target toggles itself).
        if popover != node
            && dom.contains(popover, event_target).unwrap_or(false)
            && dom.contains(node, popover).unwrap_or(false)
        {
            return;
        }
        let action = dom.get_attribute(node, "popovertargetaction").ok().flatten().map(str::to_ascii_lowercase);
        (dom.is_popover_showing(popover), action)
    };
    match action.as_deref() {
        Some("show") if showing => {}
        Some("hide") if !showing => {}
        _ if showing => {
            let _ = hide_popover(page, popover, true, true, false, Some(node));
        }
        _ => {
            if check_popover_validity(page, popover, false, false).unwrap_or(false) {
                let _ = show_popover(page, popover, false, Some(node));
            }
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Command {
    TogglePopover,
    ShowPopover,
    HidePopover,
    Close,
    RequestClose,
    ShowModal,
    Custom,
}

/// The `command` attribute's state; `None` is the Unknown state.
fn command_state(v: &str) -> Option<Command> {
    if v.starts_with("--") {
        return Some(Command::Custom);
    }
    Some(match v.to_ascii_lowercase().as_str() {
        "toggle-popover" => Command::TogglePopover,
        "show-popover" => Command::ShowPopover,
        "hide-popover" => Command::HidePopover,
        "close" => Command::Close,
        "request-close" => Command::RequestClose,
        "show-modal" => Command::ShowModal,
        _ => return None,
    })
}

/// The `button` activation behavior after its form steps: the
/// `commandfor` command, else the `popovertarget` action.
pub(crate) fn button_activation(page: &Rc<PageDom>, button: NodeId, event_target: NodeId) {
    let target = {
        let dom = page.dom.borrow();
        command_for_element(&dom, button)
    };
    let Some(target) = target else {
        popover_target_activation(page, button, event_target);
        return;
    };
    let (raw, command, html_target, is_dialog) = {
        let dom = page.dom.borrow();
        let raw = dom.get_attribute(button, "command").ok().flatten().unwrap_or("").to_owned();
        let command = command_state(&raw);
        (raw, command, dom.element(target).is_ok_and(|e| e.is_html()), is_html(&dom, target, "dialog"))
    };
    let Some(command) = command else { return };
    let popover_command = matches!(command, Command::TogglePopover | Command::ShowPopover | Command::HidePopover);
    let dialog_command = matches!(command, Command::Close | Command::RequestClose | Command::ShowModal);
    // Popover commands dispatch on any HTML element. A listener can
    // add or remove its popover attribute before the default action.
    if (popover_command && !html_target) || (dialog_command && !is_dialog) {
        return;
    }
    let (key, source) = {
        let dom = page.dom.borrow();
        (dom.node_key(target), dom.node_key(button))
    };
    let Some(key) = key else { return };
    let ev = EventCore::trusted(page, EventInterface::CommandEvent, "command", false, true);
    ev.state.borrow_mut().composed = true;
    {
        let mut f = ev.fields.borrow_mut();
        f.command = if command == Command::Custom { raw } else { raw.to_ascii_lowercase() };
        f.submitter = source;
    }
    if !crate::target::fire(page, key, &ev) || !page.dom.borrow().is_connected(target).unwrap_or(false) {
        return;
    }
    let showing = page.dom.borrow().is_popover_showing(target);
    match command {
        Command::Custom => {}
        Command::HidePopover | Command::TogglePopover if showing => {
            let _ = hide_popover(page, target, true, true, false, Some(button));
        }
        Command::ShowPopover | Command::TogglePopover => {
            if check_popover_validity(page, target, false, false).unwrap_or(false) {
                let _ = show_popover(page, target, false, Some(button));
            }
        }
        Command::HidePopover => {}
        Command::ShowModal | Command::Close | Command::RequestClose if showing => {}
        Command::ShowModal => {
            let _ = show_modal_dialog(page, target, false, Some(button));
        }
        Command::Close | Command::RequestClose => {
            let value = page.dom.borrow().get_attribute(button, "value").ok().flatten().map(str::to_owned);
            if command == Command::Close {
                close_dialog(page, target, value, Some(button));
            } else {
                request_close_dialog(page, target, value, Some(button));
            }
        }
    }
}

/// An element-reflecting IDL attribute (`popoverTargetElement`,
/// `commandForElement`) over the IDREF content attribute `$attr`.
macro_rules! reflect_element {
    ($def:expr, $prop:literal, $attr:literal, $which:expr) => {
        $def.accessor(
            $prop,
            |s, n| {
                let page = n.page();
                let target = {
                    let dom = page.dom.borrow();
                    dom.invoker_target(n.id, $which).js()?
                };
                node_value(s, page, target)
            },
            |s, n, v| {
                let target = if v.is_nullish() {
                    None
                } else {
                    let el = s.unwrap::<crate::node::Element>(v).ok_or_else(|| {
                        JsError::Type(concat!("Expected an Element for ", $prop).into())
                    })?;
                    if !Rc::ptr_eq(&el.page().dom, &n.page().dom) {
                        return n.page().dom.borrow_mut().set_attribute_ns(n.id, None, $attr, "").js();
                    }
                    Some(el.id)
                };
                n.page().dom.borrow_mut().set_invoker_target(n.id, $which, target).js()
            },
        );
    };
}

/// The PopoverInvokerElement mixin: `popoverTargetElement` and
/// `popoverTargetAction` (button and input).
pub(crate) fn define_popover_invoker<C: JsClass<Data = NodeData>>(def: &mut ClassDef<C>) {
    reflect_element!(def, "popoverTargetElement", "popovertarget", InvokerTarget::PopoverTarget);
    def.accessor(
        "popoverTargetAction",
        |s, n| {
            let dom = n.page().dom.borrow();
            let v = dom.get_attribute(n.id, "popovertargetaction").js()?.map(str::to_ascii_lowercase);
            Ok(s.string(match v.as_deref() {
                Some("show") => "show",
                Some("hide") => "hide",
                _ => "toggle",
            }))
        },
        |_s, n, v| {
            let v = dom_string(v)?;
            n.page().dom.borrow_mut().set_attribute(n.id, "popovertargetaction", &v).js()
        },
    );
}

/// `commandForElement` and `command` (button).
pub(crate) fn define_command_invoker<C: JsClass<Data = NodeData>>(def: &mut ClassDef<C>) {
    reflect_element!(def, "commandForElement", "commandfor", InvokerTarget::CommandFor);
    def.accessor(
        "command",
        |s, n| {
            let dom = n.page().dom.borrow();
            let v = dom.get_attribute(n.id, "command").js()?.unwrap_or("");
            Ok(match command_state(v) {
                Some(Command::Custom) => s.string(v),
                Some(_) => s.string(&v.to_ascii_lowercase()),
                None => s.string(""),
            })
        },
        |_s, n, v| {
            let v = dom_string(v)?;
            n.page().dom.borrow_mut().set_attribute(n.id, "command", &v).js()
        },
    );
}
