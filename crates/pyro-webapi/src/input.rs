//! User-interaction semantics: focus, click activation behavior, form
//! submission/reset, keyboard text editing and CDP-style mouse/key input.
//!
//! Hit testing for mouse input uses the faux layout (`layout_api`); no
//! geometry is fabricated here.

use std::rc::Rc;

use pyro_dom::events::TargetKey;
use pyro_dom::forms::InputType;
use pyro_dom::{Dom, NodeId};

use crate::event::{EventCore, EventData, EventInterface};
use crate::page::{FocusTarget, FocusTransition, PageDom};
use crate::target::fire;

pub(crate) fn key_of(page: &PageDom, id: NodeId) -> Option<TargetKey> {
    page.dom.borrow().node_key(id)
}

/// Fires a trusted simple `Event` at `node`. Returns false if canceled.
pub fn fire_simple(page: &Rc<PageDom>, node: NodeId, ty: &str, bubbles: bool, cancelable: bool) -> bool {
    let Some(key) = key_of(page, node) else { return true };
    // An event nobody listens for is not made (`crate::target::unheard`).
    if crate::target::unheard(page, ty) {
        return true;
    }
    let ev = EventCore::trusted(page, EventInterface::Event, ty, bubbles, cancelable);
    fire(page, key, &ev)
}

/// Queues a simple event as a separate task (resource load/error, dialog
/// close). Runs synchronously when no task hook is installed.
pub fn queue_simple(page: &Rc<PageDom>, node: NodeId, ty: &'static str, bubbles: bool, cancelable: bool) {
    let p = page.clone();
    page.post_task(Box::new(move || {
        fire_simple(&p, node, ty, bubbles, cancelable);
    }));
}

fn fire_input_event(
    page: &Rc<PageDom>,
    node: NodeId,
    ty: &str,
    input_type: &str,
    data: Option<&str>,
    cancelable: bool,
) -> bool {
    let Some(key) = key_of(page, node) else { return true };
    if crate::target::unheard_input(page, ty, key, cancelable, None) {
        return true;
    }
    let ev = EventCore::trusted(page, EventInterface::InputEvent, ty, true, cancelable);
    {
        let mut f = ev.fields.borrow_mut();
        f.input_type = input_type.to_owned();
        f.data = data.map(str::to_owned);
    }
    fire(page, key, &ev)
}

fn fire_focus_event(page: &Rc<PageDom>, node: NodeId, ty: &str, bubbles: bool, related: Option<NodeId>) {
    let Some(key) = key_of(page, node) else { return };
    if crate::target::unheard(page, ty) {
        return;
    }
    let ev = EventCore::trusted(page, EventInterface::FocusEvent, ty, bubbles, false);
    ev.fields.borrow_mut().related_target = related.and_then(|r| key_of(page, r));
    fire(page, key, &ev);
}

// ----- focus --------------------------------------------------------------

fn adopt_transition(page: &PageDom, token: &FocusTransition) {
    let previous = page.focus_transition.replace(Some(token.clone()));
    if let Some(previous) = previous {
        if !previous.ptr_eq(token) {
            previous.cancel();
        }
    }
}

fn live_document(page: &PageDom, doc: NodeId, token: &FocusTransition) -> bool {
    token.is_current() && focus_document_is_live(page) && page.document() == doc
}

fn focus_document_is_live(page: &PageDom) -> bool {
    !page.is_disposed() && page.hooks().is_none_or(|h| h.focus_document_is_live())
}

fn live_element(page: &PageDom, doc: NodeId, el: NodeId) -> bool {
    let dom = page.dom.borrow();
    dom.node_document(el).ok() == Some(doc) && dom.is_connected(el).unwrap_or(false)
}

/// The sibling page (an iframe's document sharing this `Dom`) whose document
/// `el` belongs to, when that is not `page`'s own. A node created by one
/// document and adopted into a frame's keeps its wrapper's page, so its
/// `focus()`/`blur()` must be applied by the page that owns it now.
pub(crate) fn sibling_page_of(page: &Rc<PageDom>, el: NodeId) -> Option<Rc<PageDom>> {
    let doc = page.dom.borrow().node_document(el).ok()?;
    if doc == page.document() {
        return None;
    }
    let realm = page.realms.borrow().values().find_map(|r| r.upgrade())?;
    let scope = realm.scope().ok()?;
    page.sibling_realms(&scope).iter().find_map(|sibling| {
        let s = sibling.scope().ok()?;
        let owner = crate::target::page_of(&s).ok()?;
        (!Rc::ptr_eq(&owner, page) && owner.document() == doc).then_some(owner)
    })
}

/// An element whose focus means the focus of another document (its child
/// browsing context). Focus events across documents carry no relatedTarget,
/// as in Chromium, where the parent never focuses the owner element itself.
fn is_frame_owner(dom: &Dom, el: NodeId) -> bool {
    dom.element(el).is_ok_and(|e| e.is_html() && matches!(&*e.name.local, "iframe" | "frame" | "object" | "embed"))
}

fn request_focus(page: &Rc<PageDom>, target: FocusTarget, prevent_scroll: bool) -> Option<FocusTransition> {
    if !focus_document_is_live(page) {
        return None;
    }
    let doc = page.document();
    let token = FocusTransition::with_prevent_scroll(prevent_scroll);
    adopt_transition(page, &token);
    let hooks = page.hooks();
    let apply = hooks.is_none_or(|h| h.request_focus(target, &token));
    (apply && live_document(page, doc, &token)).then_some(token)
}

/// Recheck effective inertness after focus listeners have had a chance to change styles.
fn can_focus(page: &PageDom, el: NodeId) -> bool {
    let dom = page.dom.borrow();
    let layout = page.layout();
    (node_focusable(&layout, &dom, el) || page.popover.forced_focus(el) || is_keyboard_focusable_scroller(&layout, &dom, el))
        && !layout.style().is_inert(&dom, el)
}

/// Chrome's keyboard-focusable scroller (`Element::IsScrollableContainerFocusable`, shipped with
/// KeyboardFocusableScrollers): a scroll container that has something to scroll and no focusable
/// descendant is focusable (`scroller.focus()` works, so the arrow keys, Space and PageUp/PageDown
/// scroll it) even with no `tabindex`. Pyro takes it for `focus()` only: a press on it does not focus
/// it, as in Chrome, and Tab navigation does not stop at it. Not the viewport's root or body.
fn is_keyboard_focusable_scroller(layout: &pyro_dom::layout::Layout, dom: &Dom, el: NodeId) -> bool {
    if !dom.element(el).is_ok() || !dom.is_connected(el).unwrap_or(false) || layout.is_root_container(dom, el) {
        return false;
    }
    let axes = layout.style().overflow_axes(dom, el);
    let overflows = (axes.y && layout.scroll_height(dom, el) > layout.client_height(dom, el))
        || (axes.x && layout.scroll_width(dom, el) > layout.client_width(dom, el));
    if !overflows || !layout.is_visible(dom, el) {
        return false;
    }
    let Ok(descendants) = dom.descendants(el) else { return false };
    !descendants.skip(1).any(|d| dom.is_focusable(d).unwrap_or(false) || dom.is_delegates_focus_host(d))
}

thread_local! {
    /// The `focusVisible` member of the `focus(options)` being run, if given.
    static FOCUS_VISIBLE_OPTION: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
    /// The last mouse press: its page (the `Rc` address) and the flat-tree chain from the pressed
    /// element up. Keyboard scrolling with nothing focused starts at the first node of the chain
    /// still in the document (Chrome's `last_clicked_node_` falls back to a removed node's parent).
    static LAST_PRESS: std::cell::RefCell<Option<(usize, Vec<NodeId>)>> = const { std::cell::RefCell::new(None) };
}

/// Remembers `target`'s flat-tree chain as the last mouse press (see [`LAST_PRESS`]).
fn remember_press(page: &Rc<PageDom>, target: NodeId) {
    let mut chain = Vec::new();
    {
        let dom = page.dom.borrow();
        let mut cur = Some(target);
        while let Some(e) = cur {
            chain.push(e);
            cur = flat_parent_element(&dom, e);
        }
    }
    LAST_PRESS.with(|c| *c.borrow_mut() = Some((Rc::as_ptr(page) as usize, chain)));
}

/// `element.focus(options)`: [`focus_element_with_options`] with the `focusVisible` member, which
/// when given decides whether the focused element matches `:focus-visible`.
pub(crate) fn focus_element_from_script(
    page: &Rc<PageDom>,
    el: NodeId,
    prevent_scroll: bool,
    focus_visible: Option<bool>,
) {
    let previous = FOCUS_VISIBLE_OPTION.with(|f| f.replace(focus_visible));
    focus_element_with_options(page, el, prevent_scroll);
    FOCUS_VISIBLE_OPTION.with(|f| f.set(previous));
}

/// HTML focusing steps, coordinated with the browsing-context owner.
pub fn focus_element(page: &Rc<PageDom>, el: NodeId) {
    focus_element_with_options(page, el, false);
}

/// Focus with the already-converted preventScroll option.
pub fn focus_element_with_options(page: &Rc<PageDom>, el: NodeId, prevent_scroll: bool) {
    if let Some(owner) = sibling_page_of(page, el) {
        return focus_element_with_options(&owner, el, prevent_scroll);
    }
    let doc = page.document();
    if !live_element(page, doc, el) {
        return;
    }
    // HTML focusing steps: a `delegatesFocus` host forwards focus to its
    // focus delegate (none when focus is already inside it), and only a
    // focusable area (not inert, being rendered) takes focus.
    let el = {
        let layout = page.layout();
        let dom = page.dom.borrow();
        if dom.is_delegates_focus_host(el) {
            let inside = dom.focused(doc).ok().flatten().is_some_and(|f| dom.shadow_including_contains(el, f));
            match (!inside).then(|| dom.focus_delegate(el, &|n| is_focusable_area(&layout, &dom, n))).flatten() {
                Some(d) => d,
                None => return,
            }
        } else if is_focusable_area(&layout, &dom, el) || page.popover.forced_focus(el) || is_keyboard_focusable_scroller(&layout, &dom, el) {
            el
        } else {
            return;
        }
    };
    if let Some(token) = request_focus(page, FocusTarget::Element(el), prevent_scroll) {
        focus_element_with_transition(page, el, &token);
    }
}

/// Owner application of focus without recursively requesting owner policy.
pub fn focus_element_with_transition(page: &Rc<PageDom>, el: NodeId, token: &FocusTransition) {
    if !token.is_current() || !focus_document_is_live(page) {
        return;
    }
    adopt_transition(page, token);
    let doc = page.document();
    if !live_element(page, doc, el) || !can_focus(page, el) {
        return;
    }
    let prev = page.dom.borrow().focused(doc).ok().flatten();
    if prev == Some(el) {
        return;
    }
    // Focus entering or leaving a child document reports no relatedTarget.
    let (blur_related, focus_related) = {
        let dom = page.dom.borrow();
        ((!is_frame_owner(&dom, el)).then_some(el), prev.filter(|&p| !is_frame_owner(&dom, p)))
    };
    if let Some(prev) = prev {
        run_blur(page, doc, prev, blur_related, token);
    }
    if !live_document(page, doc, token) || !live_element(page, doc, el) || !can_focus(page, el) {
        return;
    }
    {
        // `:focus-visible` as Chrome decides it: after keyboard input (and
        // for script focus before any pointer press) the focused element
        // shows its ring; after a pointer press only an element that takes
        // typed text does.
        let mut dom = page.dom.borrow_mut();
        // `focus({focusVisible})` decides it when given.
        let visible = FOCUS_VISIBLE_OPTION
            .with(std::cell::Cell::get)
            .unwrap_or_else(|| !page.pointer_modality.get() || takes_typed_text(&dom, el));
        let _ = dom.set_focus_visible(doc, visible);
        let _ = dom.set_focused(doc, Some(el));
        // The focusgroup's memory follows focus onto its items.
        if let Some((owner, flags)) = dom.focusgroup_owner_of(el) {
            if flags.memory {
                page.focusgroup_memory.borrow_mut().insert(owner, el);
            }
        }
    }
    if is_text_control(&page.dom.borrow(), el) {
        page.editing_target.set(Some(el));
    }
    crate::editing::caret_on_focus(page, el);
    page.remember_focus_value(el);
    let scrolled = if token.prevents_scroll() {
        Vec::new()
    } else {
        page.layout.borrow_mut().reveal_for_focus(&page.dom.borrow(), el)
    };
    // A text control's caret is the document selection: Chromium reports
    // the change on focus.
    if is_text_control(&page.dom.borrow(), el) {
        crate::editing::queue_control_selectionchange(page, el);
    }
    fire_focus_event(page, el, "focus", false, focus_related);
    if !live_document(page, doc, token) || !live_element(page, doc, el) {
        return;
    }
    fire_focus_event(page, el, "focusin", true, focus_related);
    if live_document(page, doc, token) {
        for target in scrolled {
            let node = match target {
                pyro_dom::layout::ScrollTarget::Viewport => None,
                pyro_dom::layout::ScrollTarget::Container(node) => Some(node),
            };
            crate::layout_api::fire_scroll(page, node);
        }
    }
}

/// Sets the element being activated (`:active`); restyles only when a sheet
/// depends on `:active` (none does unless the page's own CSS uses it).
fn set_active(page: &PageDom, doc: NodeId, el: Option<NodeId>) {
    let restyle = page.layout.try_borrow().map_or(true, |l| l.style().active_styled());
    let _ = page.dom.borrow_mut().set_activated(doc, el, restyle);
}

fn run_blur(page: &Rc<PageDom>, doc: NodeId, el: NodeId, related: Option<NodeId>, token: &FocusTransition) {
    let _ = page.dom.borrow_mut().set_focused(doc, None);
    // A button or checkable input that blurs stops being `:active`, pressed by Space or a pointer
    // (WPT checkable-active-onblur-with-click).
    // The element focus leaves for nothing is the sequential focus navigation starting point.
    if related.is_none() {
        page.sequential_start.set(Some(el));
    }
    let held = page.space_target.take().is_some();
    if page.dom.borrow().activated(doc) == Some(el) && (held || space_clickable(&page.dom.borrow(), el)) {
        set_active(page, doc, None);
    }
    if !live_element(page, doc, el) {
        return;
    }
    fire_change_if_dirty(page, el);
    if !live_document(page, doc, token) || !live_element(page, doc, el) {
        return;
    }
    fire_focus_event(page, el, "blur", false, related);
    if !live_document(page, doc, token) || !live_element(page, doc, el) {
        return;
    }
    fire_focus_event(page, el, "focusout", true, related);
}

/// After `owner`'s `disabled` content attribute changed: a connected focused
/// control that it disabled (itself, or a descendant of the fieldset) loses
/// focus at once, as in Chromium: trusted `blur` and `focusout` with no
/// related target, the body active, before the change's MutationObserver
/// callbacks (react-aria synthesizes its own blur otherwise, and a dialog's
/// FocusScope then sees two blurs). A `disabled` write elsewhere moves no
/// focus, nor does one while the focused element is detached.
pub(crate) fn blur_if_focus_disabled(page: &Rc<PageDom>, owner: NodeId, attribute: &str) {
    if !attribute.eq_ignore_ascii_case("disabled") {
        return;
    }
    let lost = {
        let dom = page.dom.borrow();
        let fieldset = dom.element(owner).is_ok_and(|e| e.is_html() && &*e.name.local == "fieldset");
        dom.focused(page.document()).ok().flatten().filter(|&f| {
            (f == owner || (fieldset && dom.contains(owner, f).unwrap_or(false)))
                && dom.is_connected(f).unwrap_or(false)
                && dom.is_disabled(f).unwrap_or(false)
        })
    };
    if let Some(f) = lost {
        blur_element(page, f);
    }
}

/// Before a script removes `node` (or, with `children_only`, its children):
/// when the focused element is inside what goes, it loses focus first, as in
/// Chromium (`Document::RemoveFocusedElementOfSubtree`): trusted `blur` and
/// `focusout` with no related target while it is still connected, so a
/// root-delegated listener (React's `onBlur`) sees them, and the body is
/// active. A handler that moves the node makes the removal fail as usual.
/// Firefox and the spec's focus fixup rule fire nothing; Chromium-based
/// browsers do (MDN, "Element: blur event").
pub fn blur_before_removal(page: &Rc<PageDom>, node: NodeId, children_only: bool) {
    let focused = {
        let dom = page.dom.borrow();
        let Some(f) = dom.focused(page.document()).ok().flatten() else { return };
        let inside = dom.is_connected(node).unwrap_or(false)
            && dom.contains(node, f).unwrap_or(false)
            && !(children_only && f == node);
        if !inside {
            return;
        }
        f
    };
    blur_element(page, focused);
}

/// HTML's focus fixup rule for a focused element that `moveBefore` moved:
/// it keeps focus through the move, and loses it at the next rendering
/// opportunity (here: a task of its own) when it is no longer a focusable
/// area, i.e. it landed in an inert or non-rendered subtree. As in Chromium
/// the element then gets `blur` and `focusout` and the body is active.
pub(crate) fn schedule_focus_fixup(page: &Rc<PageDom>) {
    let Some(hooks) = page.hooks() else { return };
    let weak = Rc::downgrade(page);
    hooks.post_task(Box::new(move || {
        if let Some(page) = weak.upgrade() {
            run_focus_fixup(&page);
        }
    }));
}

/// Elements that take typed text (text controls, editable content): mouse
/// focus on them still matches `:focus-visible`, as in Chrome.
fn takes_typed_text(dom: &Dom, el: NodeId) -> bool {
    is_text_control(dom, el) || crate::editing::editing_host(dom, el).is_some()
}

/// Whether `select` is a listbox (display size above 1) of base appearance (`appearance:
/// base-select`, a customizable select): its options take the focus, the select itself does not.
fn is_base_listbox(layout: &pyro_dom::layout::Layout, dom: &Dom, select: NodeId) -> bool {
    if !dom.element(select).is_ok_and(|e| e.is_html() && &*e.name.local == "select") {
        return false;
    }
    let multiple = dom.has_attribute(select, "multiple").unwrap_or(false);
    let size = dom.get_attribute(select, "size").ok().flatten().and_then(|s| s.trim().parse::<u32>().ok()).filter(|&s| s > 0);
    size.unwrap_or(if multiple { 4 } else { 1 }) > 1 && layout.computed_style_value(dom, select, "appearance") == "base-select"
}

/// The base-appearance listbox select that holds `option` (see [`is_base_listbox`]).
fn option_base_listbox(layout: &pyro_dom::layout::Layout, dom: &Dom, option: NodeId) -> Option<NodeId> {
    if !dom.element(option).is_ok_and(|e| e.is_html() && &*e.name.local == "option") {
        return None;
    }
    let select = dom.option_nearest_select(option).ok().flatten()?;
    is_base_listbox(layout, dom, select).then_some(select)
}

/// [`Dom::is_focusable`] with what depends on style: the options of a base-appearance listbox are
/// focusable areas (an enabled one in an enabled select) and the select itself is not (Chromium's
/// customizable listbox: focus lives on the options).
fn node_focusable(layout: &pyro_dom::layout::Layout, dom: &Dom, el: NodeId) -> bool {
    if dom.is_focusable(el).unwrap_or(false) {
        let select = dom.element(el).is_ok_and(|e| e.is_html() && &*e.name.local == "select");
        return !(select && !dom.has_attribute(el, "tabindex").unwrap_or(false) && is_base_listbox(layout, dom, el));
    }
    dom.is_connected(el).unwrap_or(false)
        && !dom.is_disabled(el).unwrap_or(true)
        && option_base_listbox(layout, dom, el).is_some_and(|s| !dom.is_disabled(s).unwrap_or(true))
}

/// HTML "focusable area": focusable, not inert, and being rendered (not in
/// a `display: none` subtree nor `visibility: hidden`, as Chrome's
/// focusable-style check). An `<area>` is rendered through its image map.
pub(crate) fn is_focusable_area(layout: &pyro_dom::layout::Layout, dom: &Dom, el: NodeId) -> bool {
    if !node_focusable(layout, dom, el) || layout.style().is_inert(dom, el) {
        return false;
    }
    if dom.element(el).is_ok_and(|e| e.is_html() && &*e.name.local == "area") {
        return true;
    }
    let opts = pyro_dom::style::CheckVisibilityOptions { check_visibility: true, check_opacity: false };
    layout.check_visibility(dom, el, opts)
}

/// Blurs the focused element when it is not a focusable area any more:
/// disconnected, disabled, inert, or not being rendered.
pub fn run_focus_fixup(page: &Rc<PageDom>) {
    if page.is_disposed() {
        return;
    }
    let lost = {
        let layout = page.layout();
        let dom = page.dom.borrow();
        let Some(f) = dom.focused(page.document()).ok().flatten() else { return };
        let focusable = (node_focusable(&layout, &dom, f) || is_keyboard_focusable_scroller(&layout, &dom, f))
            && !layout.style().is_inert(&dom, f)
            && layout.is_visible(&dom, f);
        (!focusable).then_some(f)
    };
    if let Some(f) = lost {
        blur_element(page, f);
    }
}

/// HTML "update the rendering": when the focused element is no longer being rendered (it or an
/// ancestor became `display: none`), it loses focus, as in Chrome, which blurs it after the style
/// update and fires `blur`/`focusout` (WPT css/selectors `focus-display-none-001`,
/// `focus-within-display-none-001`). Narrower than [`run_focus_fixup`]: focusability is not
/// rechecked here, so a dialog the dialog focusing steps focused keeps focus, and an `<area>`
/// (rendered through its image map, with no box of its own) is left alone.
pub fn run_rendering_focus_fixup(page: &Rc<PageDom>) {
    if page.is_disposed() {
        return;
    }
    if page.dom.borrow().focused(page.document()).ok().flatten().is_none() {
        return;
    }
    let lost = {
        let layout = page.layout();
        let dom = page.dom.borrow();
        let Some(f) = dom.focused(page.document()).ok().flatten() else { return };
        let area = dom.element(f).is_ok_and(|e| e.is_html() && &*e.name.local == "area");
        (!area && !page.popover.forced_focus(f) && !layout.is_visible(&dom, f)).then_some(f)
    };
    if let Some(f) = lost {
        blur_element(page, f);
    }
}

/// Explicit blur keeps the owner's selected frame and remembered edit target.
pub fn blur_element(page: &Rc<PageDom>, el: NodeId) {
    if let Some(owner) = sibling_page_of(page, el) {
        return blur_element(&owner, el);
    }
    if page.dom.borrow().focused(page.document()).ok().flatten() != Some(el) {
        return;
    }
    if let Some(token) = request_focus(page, FocusTarget::Blur(el), false) {
        // Owner callbacks may already have changed the active element.
        if page.dom.borrow().focused(page.document()).ok().flatten() == Some(el) {
            blur_document_with_transition(page, &token);
        }
    }
}

/// Runs `f` the way a DOM method called by page script runs: inside a native
/// function (named `name`) called from the page's main world. JavaScriptCore
/// runs microtasks when the outermost call into it returns. Dispatched from
/// Rust, each listener of an event is such a call, with a microtask
/// checkpoint of its own; here the listeners are nested calls, and the
/// microtasks they queue run once, after `f`. Without a live main world `f`
/// runs directly.
fn as_script(page: &Rc<PageDom>, name: &str, f: impl Fn(&Rc<PageDom>) + 'static) {
    let f = Rc::new(f);
    let ran = Rc::new(std::cell::Cell::new(false));
    let call = || -> pyro_js::JsResult<()> {
        let realm = page.main_world_realm().filter(|r| !r.is_disposed()).ok_or(pyro_js::JsError::ContextDestroyed)?;
        let s = realm.scope()?;
        // Weak: the function object lives until it is collected.
        let (weak, f, ran) = (Rc::downgrade(page), f.clone(), ran.clone());
        let native = s.function(name, move |s, _, _| {
            if let Some(page) = weak.upgrade() {
                ran.set(true);
                f(&page);
            }
            Ok(s.undefined())
        })?;
        native.call(None, &[]).map(|_| ())
    };
    let _ = call();
    if !ran.get() {
        f(page);
    }
}

/// A speculation rule set that could not be used (HTML "update speculation rules"): `error`
/// at its `<script>` and, for text that is no rule set (`message`), a `TypeError` reported at the
/// window, after the element's listeners. Both run as one script-like call, so a promise the
/// element's listener resolves continues only once the window has been told too (as in Chrome,
/// where both happen inside the script that inserted the element).
pub fn speculation_rules_error(page: &Rc<PageDom>, el: NodeId, message: Option<String>) {
    as_script(page, "speculationrules", move |page| {
        fire_simple(page, el, "error", false, false);
        let Some(message) = &message else { return };
        let Some(realm) = page.main_world_realm() else { return };
        let Ok(scope) = realm.scope() else { return };
        scope.report_exception(&pyro_js::JsError::Type(message.clone().into()));
    });
}

/// [`focus_element`] as a script's `el.focus()` runs it (see [`as_script`]):
/// what a client's bare focus is. A user's focus comes with a pointer or key
/// event, dispatched by the browser, whose listeners each get a microtask
/// checkpoint. A focus without one exists only in script (Playwright's
/// `focus`, and the focus its `fill`, `press` and `type` start with, call
/// `focus()` from an injected script), where the listeners of `focus` and
/// `focusin` run back to back. A page that renders in a microtask and
/// replaces the focused control (React) would otherwise do so between two
/// listeners of one `focusin`, whose later listeners then see a detached
/// target (react-aria's focus scope takes that for focus leaving the scope).
pub fn script_focus(page: &Rc<PageDom>, el: NodeId) {
    as_script(page, "focus", move |page| focus_element(page, el));
}

/// [`blur_element`] as a script's `el.blur()` runs it (see [`script_focus`]).
pub fn script_blur(page: &Rc<PageDom>, el: NodeId) {
    as_script(page, "blur", move |page| blur_element(page, el));
}

pub fn focus_window(page: &Rc<PageDom>) {
    if let Some(token) = request_focus(page, FocusTarget::Window, false) {
        focus_window_with_transition(page, &token);
    }
}

/// Window activation clears local activeElement, retaining editing selection.
pub fn focus_window_with_transition(page: &Rc<PageDom>, token: &FocusTransition) {
    blur_document_with_transition(page, token);
}

/// Owner blur of an old document, without recursively invoking the owner hook.
pub fn blur_document_with_transition(page: &Rc<PageDom>, token: &FocusTransition) {
    if !token.is_current() || !focus_document_is_live(page) {
        return;
    }
    adopt_transition(page, token);
    let doc = page.document();
    let prev = page.dom.borrow().focused(doc).ok().flatten();
    if let Some(prev) = prev {
        run_blur(page, doc, prev, None, token);
    }
}

/// Non-event teardown. Never resurrects selection after detach/navigation.
pub fn clear_document_focus(page: &PageDom) {
    if let Some(token) = page.focus_transition.take() {
        token.cancel();
    }
    let _ = page.dom.borrow_mut().set_focused(page.document(), None);
    page.focus_values.borrow_mut().clear();
    page.editing_target.set(None);
    page.space_target.set(None);
}

/// A remembered text control is distinct from document.activeElement.
/// The returned node must still belong to this connected document.
pub fn remembered_editing_target(page: &PageDom) -> Option<NodeId> {
    page.editing_target.get().filter(|&el| focus_document_is_live(page) && live_element(page, page.document(), el))
}

fn accepts_keyboard(page: &PageDom) -> bool {
    focus_document_is_live(page) && page.hooks().is_none_or(|h| h.is_focused())
}

/// Text controls fire `change` on commit when their value differs from the
/// value at focus time.
fn fire_change_if_dirty(page: &Rc<PageDom>, el: NodeId) {
    // Leaving a control the user edited commits the edit: it gets user
    // validity (`:user-valid` / `:user-invalid`) even when the value ends up
    // as it was at focus time.
    let edited = page.dom.borrow().control_state(el).ok().flatten().is_some_and(|c| c.user_edited);
    if edited {
        let _ = page.dom.borrow_mut().set_user_validity(el, true);
    }
    // As Chrome: a value only script changed since focus fires no change.
    let since_focus = page.dom.borrow().control_state(el).ok().flatten().is_some_and(|c| c.edited_since_focus);
    let current = page.dom.borrow().value(el).ok();
    if let (Some(cur), Some(old)) = (current, page.take_focus_value(el)) {
        if since_focus && cur != old && is_text_control(&page.dom.borrow(), el) {
            fire_user_change(page, el);
        }
    }
}

/// `change` for a value the user committed: the control gets user validity
/// first (HTML §4.10.18.6), so `change` listeners and `:user-invalid` agree.
pub fn fire_user_change(page: &Rc<PageDom>, el: NodeId) {
    let _ = page.dom.borrow_mut().set_user_validity(el, true);
    fire_simple(page, el, "change", true, false);
}

fn is_text_control(dom: &Dom, el: NodeId) -> bool {
    match dom.element(el) {
        Ok(e) if e.is_html() && &*e.name.local == "textarea" => true,
        Ok(e) if e.is_html() && &*e.name.local == "input" => !matches!(
            dom.input_type(el),
            Ok(InputType::Checkbox
                | InputType::Radio
                | InputType::File
                | InputType::Submit
                | InputType::Reset
                | InputType::Button
                | InputType::Image
                | InputType::Hidden)
        ),
        _ => false,
    }
}

// ----- click activation ------------------------------------------------------

/// What an activation target's legacy-pre-activation behavior changed, for
/// its legacy-canceled-activation behavior to undo.
#[derive(Debug)]
enum LegacyPreActivation {
    None,
    Checkbox { checked: bool, indeterminate: bool },
    Radio { was_checked: bool, previously_checked: Vec<NodeId> },
}

/// The one activation target of a click (DOM "dispatch" steps 6.5 and 6.9).
#[derive(Debug)]
struct ActivationTarget {
    el: NodeId,
    legacy: LegacyPreActivation,
    /// The elements of the event's path after `el` that the activation could have reached (a
    /// label without a labeled control passes the click on to the first with activation
    /// behavior).
    rest: Vec<NodeId>,
}

fn local(dom: &Dom, id: NodeId) -> Option<String> {
    dom.element(id).ok().filter(|e| e.is_html()).map(|e| e.name.local.to_string())
}

/// The `href` of an SVG `a` element (`href`, else `xlink:href`): a hyperlink
/// as an HTML `a` is (SVG 2 "Links out of SVG content").
fn svg_link_href(dom: &Dom, el: NodeId) -> Option<&str> {
    let e = dom.element(el).ok()?;
    if e.name.ns != markup5ever::ns!(svg) || &*e.name.local != "a" {
        return None;
    }
    dom.get_attribute(el, "href")
        .ok()
        .flatten()
        .or_else(|| dom.get_attribute_ns(el, Some("http://www.w3.org/1999/xlink"), "href").ok().flatten())
}

/// HTML elements that have activation behavior: every `input` but a hidden
/// one (as Chrome: a click on it inside a label activates the label) and
/// every `button` (whatever their state: a disabled control is still the
/// activation target and merely does nothing), `label`, and, as in
/// browsers, `a`/`area` with an `href` and a `details`' own `summary`.
fn has_activation_behavior(dom: &Dom, el: NodeId) -> bool {
    match local(dom, el).as_deref() {
        Some("input") => dom.input_type(el).ok() != Some(InputType::Hidden),
        // A label is the activation target whether or not it has a labeled control: which it
        // has is decided when its activation behavior runs, after the click's listeners, and
        // without one the click activates what is around it, such as a button or link it is in
        // (`run_activation_behavior`).
        Some("button" | "label") => true,
        Some("a" | "area") => dom.has_attribute(el, "href").unwrap_or(false),
        Some("summary") => dom.is_details_summary(el),
        _ => svg_link_href(dom, el).is_some(),
    }
}

/// An HTML submit button, enabled or not: `input` in the Submit or Image
/// state, or a `button` that is neither a reset nor a plain button.
fn is_submit_button(dom: &Dom, el: NodeId) -> bool {
    match local(dom, el).as_deref() {
        Some("input") => matches!(dom.input_type(el), Ok(InputType::Submit | InputType::Image)),
        Some("button") => crate::popover::button_is_submit(dom, el),
        _ => false,
    }
}

/// HTML interactive content that has no activation behavior of its own: a
/// click inside it does not activate a `label` around it.
fn swallows_label_click(dom: &Dom, el: NodeId) -> bool {
    match local(dom, el).as_deref() {
        // A nested label too: one without a labeled control passes the click on to the outer
        // label, which must not forward it.
        Some("select" | "textarea" | "details" | "iframe" | "embed" | "label") => true,
        Some("audio" | "video") => dom.has_attribute(el, "controls").unwrap_or(false),
        Some("img") => dom.has_attribute(el, "usemap").unwrap_or(false),
        _ => false,
    }
}

/// HTML legacy-pre-activation behavior (`input` checkboxes and radio
/// buttons): the control is already toggled when the click's listeners run.
fn legacy_pre_activation(page: &Rc<PageDom>, el: NodeId) -> LegacyPreActivation {
    let mut dom = page.dom.borrow_mut();
    if local(&dom, el).as_deref() != Some("input") {
        return LegacyPreActivation::None;
    }
    match dom.input_type(el) {
        Ok(InputType::Checkbox) => {
            let checked = dom.checked(el).unwrap_or(false);
            let indeterminate = dom.control_state(el).ok().flatten().is_some_and(|c| c.indeterminate);
            let _ = dom.set_checked(el, !checked);
            if indeterminate {
                if let Ok(state) = dom.control_state_mut(el) {
                    state.indeterminate = false;
                }
            }
            LegacyPreActivation::Checkbox { checked, indeterminate }
        }
        Ok(InputType::Radio) => {
            let was_checked = dom.checked(el).unwrap_or(false);
            let previously_checked = radio_group_checked(&dom, el);
            let _ = dom.set_checked(el, true);
            LegacyPreActivation::Radio { was_checked, previously_checked }
        }
        _ => LegacyPreActivation::None,
    }
}

/// HTML legacy-canceled-activation behavior: the canceled click leaves the
/// checkbox or radio group as it was.
fn legacy_canceled_activation(page: &Rc<PageDom>, el: NodeId, legacy: LegacyPreActivation) {
    let mut dom = page.dom.borrow_mut();
    match legacy {
        LegacyPreActivation::Checkbox { checked, indeterminate } => {
            let _ = dom.set_checked(el, checked);
            // Both as they were, whatever the listeners set meanwhile.
            if let Ok(state) = dom.control_state_mut(el) {
                state.indeterminate = indeterminate;
            }
        }
        LegacyPreActivation::Radio { was_checked, previously_checked } => {
            let _ = dom.set_checked(el, was_checked);
            for p in previously_checked {
                let _ = dom.set_checked(p, true);
            }
        }
        LegacyPreActivation::None => {}
    }
}

/// Dispatches a click-like event with activation behavior (HTML "activation
/// behavior" + legacy pre-activation for checkboxes/radios). Used for both
/// trusted input and `dispatchEvent`/`click()`.
pub fn dispatch_with_activation(page: &Rc<PageDom>, target: NodeId, ev: &EventData) -> bool {
    dispatch_click(page, target, ev, false)
}

/// [`dispatch_with_activation`], without any activation behavior when `handled` (pyro: a click on
/// an enabled `::scroll-button()` is handled by the button, so a link it is in is not followed:
/// WPT css-overflow `scroll-button-link-activation`).
fn dispatch_click(page: &Rc<PageDom>, target: NodeId, ev: &EventData, handled: bool) -> bool {
    let is_click = &*ev.state.borrow().ty == "click"
        && matches!(ev.iface, EventInterface::MouseEvent | EventInterface::PointerEvent);
    let Some(key) = key_of(page, target) else { return true };
    if !is_click {
        return fire(page, key, ev);
    }
    let (bubbles, trusted) = {
        let state = ev.state.borrow();
        (state.bubbles, state.is_trusted)
    };
    // DOM "dispatch": the activation target is the target, else the first
    // item of the event's path with activation behavior; the path follows
    // slots and shadow hosts, and an item that is not a target of the event
    // only counts when the event bubbles. Its legacy-pre-activation behavior
    // runs before the first listener.
    let mut activation: Option<ActivationTarget> = None;
    // A trusted click on a child of a disabled form control stops below the control.
    let stop = if trusted { disabled_control_of(page, target).and_then(|control| key_of(page, control)) } else { None };
    let not_canceled = crate::target::fire_with(page, key, stop, ev, &mut |path| {
        if handled {
            return;
        }
        let (el, rest) = {
            let dom = page.dom.borrow();
            let mut items = path
                .iter()
                .filter(|item| bubbles || item.at_target())
                .filter_map(|item| dom.key_node(item.invocation_target));
            let el = items.by_ref().find(|&el| {
                // A user's click is not dispatched at a disabled form control
                // (HTML): it does not activate one it reaches either.
                let skipped = trusted && dom.is_disabled(el).unwrap_or(false);
                !skipped && has_activation_behavior(&dom, el)
            });
            (el, items.collect::<Vec<_>>())
        };
        activation = el.map(|el| ActivationTarget { el, legacy: legacy_pre_activation(page, el), rest });
    });
    let Some(ActivationTarget { el, legacy, rest }) = activation else { return not_canceled };
    if !not_canceled {
        legacy_canceled_activation(page, el, legacy);
        return false;
    }
    // The navigation the activation behavior starts is user-initiated (`NavigateEvent.userInitiated`)
    // when the click is the user's (trusted), not script's (`a.click()`).
    let outer = TRUSTED_ACTIVATION.with(|flag| flag.replace(trusted));
    run_activation_behavior(page, el, legacy, target, ev, &rest);
    TRUSTED_ACTIVATION.with(|flag| flag.set(outer));
    true
}

thread_local! {
    /// Whether the activation behavior now running is that of a trusted click.
    static TRUSTED_ACTIVATION: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether a navigation requested now comes from a trusted click's activation behavior (a
/// followed link, a submitted form): HTML "user involvement" is "activation".
pub(crate) fn user_initiated_navigation() -> bool {
    TRUSTED_ACTIVATION.with(std::cell::Cell::get)
}

/// HTML activation behavior of `el`, the activation target of the click
/// `ev` dispatched at `target`.
fn run_activation_behavior(
    page: &Rc<PageDom>,
    el: NodeId,
    legacy: LegacyPreActivation,
    target: NodeId,
    ev: &EventData,
    rest: &[NodeId],
) {
    let Some(name) = local(&page.dom.borrow(), el) else {
        if svg_link_href(&page.dom.borrow(), el).is_some() {
            follow_hyperlink(page, el);
        }
        return;
    };
    match name.as_str() {
        "input" => {
            let (ty, disabled, connected) = {
                let dom = page.dom.borrow();
                let Ok(ty) = dom.input_type(el) else { return };
                (ty, dom.is_disabled(el).unwrap_or(false), dom.is_connected(el).unwrap_or(false))
            };
            match ty {
                // A checkbox or radio button need not be mutable; a detached
                // one fires nothing.
                InputType::Checkbox | InputType::Radio => {
                    // Clicking the radio button that is already checked changes nothing.
                    let unchanged = matches!(legacy, LegacyPreActivation::Radio { was_checked: true, .. });
                    if connected && !unchanged {
                        fire_input_event(page, el, "input", "", None, false);
                        fire_user_change(page, el);
                    }
                }
                _ if disabled => {}
                InputType::Submit | InputType::Image => {
                    let form = page.dom.borrow().form_owner(el).ok().flatten();
                    if let Some(f) = form {
                        match image_button_coordinate(page, el, ev) {
                            Some((x, y)) => crate::formdata::with_selected_coordinate(page, el, x, y, || {
                                submit_form(page, f, Some(el), true)
                            }),
                            None => submit_form(page, f, Some(el), true),
                        }
                    }
                }
                InputType::Reset => {
                    let form = page.dom.borrow().form_owner(el).ok().flatten();
                    if let Some(f) = form {
                        reset_form(page, f);
                    }
                }
                InputType::File => {
                    if let Some(h) = page.hooks() {
                        let multiple = page.dom.borrow().has_attribute(el, "multiple").unwrap_or(false);
                        h.file_chooser(el, multiple);
                    }
                }
                _ => {}
            }
            if !disabled && matches!(ty, InputType::Submit | InputType::Image | InputType::Reset | InputType::Button) {
                crate::popover::popover_target_activation(page, el, target);
            }
        }
        "button" => {
            let (disabled, form, ty) = {
                let dom = page.dom.borrow();
                (
                    dom.is_disabled(el).unwrap_or(false),
                    dom.form_owner(el).ok().flatten(),
                    dom.get_attribute(el, "type").ok().flatten().map(|t| t.to_ascii_lowercase()),
                )
            };
            if disabled {
                return;
            }
            // With a form owner, a submit or reset button does only that and
            // the Auto state (no valid `type`) does nothing; only
            // `type=button` goes on to its command.
            if let Some(f) = form {
                match ty.as_deref() {
                    Some("reset") => return reset_form(page, f),
                    Some("button") => {}
                    _ if crate::popover::button_is_submit(&page.dom.borrow(), el) => {
                        return submit_form(page, f, Some(el), true);
                    }
                    _ => return,
                }
            }
            // `commandfor`, else `popovertarget`.
            crate::popover::button_activation(page, el, target);
        }
        // As Chrome's `IsLiveLink`: an editable link does not navigate.
        "a" | "area" if crate::editing::is_editable(&page.dom.borrow(), el) => {}
        "a" | "area" => follow_hyperlink(page, el),
        "label" => {
            let control = crate::html::labeled_control(&page.dom.borrow(), el);
            if let Some(c) = control {
                // A click on the control itself, or inside interactive
                // content of the label, is not forwarded.
                let forwarded = {
                    let dom = page.dom.borrow();
                    let mut inside = false;
                    let mut cur = Some(target);
                    while let Some(n) = cur.filter(|&n| n != el) {
                        inside |= n == c || swallows_label_click(&dom, n);
                        cur = dom.parent(n).ok().flatten();
                    }
                    !inside
                };
                if forwarded {
                    forward_click(page, c, ev);
                    // Chrome focuses the labeled control with the mouse focus type: no
                    // `:focus-visible` on a checkbox (WPT css/selectors `focus-visible-026`).
                    page.pointer_modality.set(true);
                    focus_element(page, c);
                }
            } else {
                // No labeled control (now, after the listeners: one may have been given or taken
                // during the click): as Chromium, the click activates the nearest ancestor with
                // activation behavior instead (WPT the-button-element
                // controls-inside-link-activation.tentative, popovers/label-in-invoker).
                // The rest of the event's path as dispatched. A checkbox or radio (only a script
                // puts a label in one) was never pre-activated: the click stops there.
                let next = {
                    let dom = page.dom.borrow();
                    let trusted = ev.state.borrow().is_trusted;
                    rest.iter()
                        .position(|&n| {
                            let skipped = trusted && dom.is_disabled(n).unwrap_or(false);
                            !skipped && has_activation_behavior(&dom, n)
                        })
                        .filter(|&i| {
                            local(&dom, rest[i]).as_deref() != Some("input")
                                || !matches!(dom.input_type(rest[i]), Ok(InputType::Checkbox | InputType::Radio))
                        })
                };
                if let Some(i) = next {
                    run_activation_behavior(page, rest[i], LegacyPreActivation::None, target, ev, &rest[i + 1..]);
                }
            }
        }
        "summary" => {
            if !page.dom.borrow().is_details_summary(el) {
                return;
            }
            let details = page.dom.borrow().parent(el).ok().flatten();
            // pyro-dom queues the `toggle` event of the attribute change.
            if let Some(d) = details {
                let _ = page.dom.borrow_mut().toggle_attribute(d, "open", None);
            }
        }
        _ => {}
    }
}

fn radio_group_checked(dom: &Dom, el: NodeId) -> Vec<NodeId> {
    let Some(name) = dom.get_attribute(el, "name").ok().flatten().map(str::to_owned) else { return Vec::new() };
    let Ok(root) = dom.root_node(el, false) else { return Vec::new() };
    let form = dom.form_owner(el).ok().flatten();
    dom.descendants(root)
        .map(|d| {
            d.filter(|&n| {
                n != el
                    && local(dom, n).as_deref() == Some("input")
                    && dom.input_type(n).ok() == Some(InputType::Radio)
                    && dom.get_attribute(n, "name").ok().flatten() == Some(name.as_str())
                    && dom.form_owner(n).ok().flatten() == form
                    && dom.checked(n).unwrap_or(false)
            })
            .collect()
        })
        .unwrap_or_default()
}

/// The "selected coordinate" an activated `<input type=image>` submits
/// (HTML "constructing the entry list"): the click's position relative to the
/// image's border box, floored, as Chrome computes it; (0, 0) for a click
/// without a position (`click()` and keyboard activation, which set
/// `pointerId` -1). `None` for every other submitter.
fn image_button_coordinate(page: &Rc<PageDom>, el: NodeId, ev: &EventData) -> Option<(i64, i64)> {
    {
        let dom = page.dom.borrow();
        if local(&dom, el).as_deref() != Some("input") || dom.input_type(el).ok()? != InputType::Image {
            return None;
        }
    }
    let (client_x, client_y, positionless) = {
        let f = ev.fields.borrow();
        (f.client_x, f.client_y, f.pointer_id == -1)
    };
    if positionless || !matches!(ev.iface, EventInterface::MouseEvent | EventInterface::PointerEvent) {
        return Some((0, 0));
    }
    let rect = page.layout().bounding_client_rect(&page.dom.borrow(), el).layout_units();
    Some(((client_x - rect.left()).floor() as i64, (client_y - rect.top()).floor() as i64))
}

/// `HTMLElement.click()`: synthetic (untrusted) PointerEvent click. As in
/// browsers (and upstream Html.zig), `detail` is 0, `pointerId` is -1 and
/// `pointerType` is empty; libraries such as React Aria rely on `detail == 0`
/// to recognise a virtual click.
pub fn element_click(page: &Rc<PageDom>, el: NodeId) {
    click_with_trust(page, el, false);
}

/// A click whose `isTrusted` follows the event that caused it (a label
/// forwarding a user's click to its control, as Chrome does).
fn click_with_trust(page: &Rc<PageDom>, el: NodeId, trusted: bool) {
    click_like(page, el, trusted, None);
}

/// A label's click forwarded to its control: trusted when the user's was,
/// with its pointer type, id, primariness and click count (as Chrome).
fn forward_click(page: &Rc<PageDom>, el: NodeId, from: &EventData) {
    let trusted = from.state.borrow().is_trusted;
    click_like(page, el, trusted, Some(from));
}

fn click_like(page: &Rc<PageDom>, el: NodeId, trusted: bool, from: Option<&EventData>) {
    // HTML click(): a disabled form control does nothing. As in Chromium, a disabled
    // `<fieldset>` is not one here: its click is dispatched (WPT disabled-elements
    // fieldset-event-propagation).
    let disabled_control = {
        let dom = page.dom.borrow();
        dom.is_disabled(el).unwrap_or(false)
            && !dom.element(el).is_ok_and(|e| e.is_html() && &*e.name.local == "fieldset")
    };
    if disabled_control {
        return;
    }
    if !page.begin_click(el) {
        return; // click in progress flag
    }
    let ev = EventCore::new(page, EventInterface::PointerEvent, "click", true, true, true);
    {
        let mut f = ev.fields.borrow_mut();
        f.pointer_id = -1;
        if let Some(from) = from {
            let src = from.fields.borrow();
            f.pointer_id = src.pointer_id;
            f.pointer_type = src.pointer_type.clone();
            f.is_primary = src.is_primary;
            f.detail = src.detail;
        }
    }
    ev.state.borrow_mut().is_trusted = trusted;
    dispatch_with_activation(page, el, &ev);
    page.end_click(el);
}

/// Controls a Space keyup clicks: buttons, checkable inputs and details summaries.
fn space_clickable(dom: &Dom, el: NodeId) -> bool {
    if dom.is_disabled(el).unwrap_or(false) {
        return false;
    }
    match local(dom, el).as_deref() {
        Some("button") => true,
        Some("summary") => dom.is_details_summary(el),
        Some("input") => matches!(
            dom.input_type(el),
            Ok(InputType::Checkbox
                | InputType::Radio
                | InputType::Submit
                | InputType::Image
                | InputType::Reset
                | InputType::Button)
        ),
        _ => false,
    }
}

/// The simulated click a keyboard activation dispatches: trusted, `detail`
/// 0, `pointerId` -1, empty `pointerType`.
fn keyboard_click(page: &Rc<PageDom>, el: NodeId) {
    if !space_clickable(&page.dom.borrow(), el) || !page.begin_click(el) {
        return;
    }
    let ev = EventCore::trusted(page, EventInterface::PointerEvent, "click", true, true);
    ev.state.borrow_mut().composed = true;
    ev.fields.borrow_mut().pointer_id = -1;
    dispatch_with_activation(page, el, &ev);
    page.end_click(el);
}

fn follow_hyperlink(page: &Rc<PageDom>, el: NodeId) {
    let (url, target, noopener, referrer_policy, pings) = {
        let dom = page.dom.borrow();
        // An `<area>` that is not connected is not followed (its activation behavior needs it
        // in the document, as Chromium checks); an `<a>` is.
        let area = dom.element(el).is_ok_and(|e| e.is_html() && &*e.name.local == "area");
        if area && !dom.is_connected(el).unwrap_or(false) {
            return;
        }
        let Some(href) = dom.get_attribute(el, "href").ok().flatten().or_else(|| svg_link_href(&dom, el)) else {
            return;
        };
        let url = crate::html::resolve_url(&dom, el, href);
        let pings = hyperlink_pings(&dom, el);
        // HTML "download the hyperlink": a `download` link to a blob:, data:
        // or same-origin URL downloads instead of navigating (browsers ignore
        // the attribute cross-origin).
        if let Some(filename) = dom.get_attribute(el, "download").ok().flatten().map(str::to_owned) {
            let doc_url = dom.document_data(page.document()).map(|d| d.url.to_string()).unwrap_or_default();
            let same_origin = |a: &str, b: &str| match (::url::Url::parse(a), ::url::Url::parse(b)) {
                (Ok(a), Ok(b)) => a.origin().is_tuple() && a.origin() == b.origin(),
                _ => false,
            };
            if url.starts_with("blob:") || url.starts_with("data:") || same_origin(&url, &doc_url) {
                // Bytes are captured now: pages revoke blob URLs right after click().
                let data = if url.starts_with("blob:") {
                    match page.blob_url(&url) {
                        Some(entry) => Some(entry),
                        None => return,
                    }
                } else if url.starts_with("data:") {
                    match ::url::Url::parse(&url).ok().as_ref().and_then(pyro_net::data_url::parse) {
                        Some((mime, bytes)) => Some((bytes, mime)),
                        None => return,
                    }
                } else {
                    None
                };
                drop(dom);
                if let Some(pings) = pings {
                    send_pings(page, pings, &url, None);
                }
                page.download(crate::page::DownloadRequest { url, filename, data });
                return;
            }
        }
        // Without a target of its own (or an empty one, as browsers treat it), the first
        // `<base target>`'s.
        let own = dom.get_attribute(el, "target").ok().flatten().filter(|t| !t.is_empty()).map(str::to_owned);
        let target = own.or_else(|| dom.node_document(el).ok().and_then(|doc| dom.base_target(doc)));
        // HTML "get an element's noopener": rel=noopener/noreferrer, or `_blank` without
        // rel=opener. A link to a named target keeps its opener.
        let rel = dom.get_attribute(el, "rel").ok().flatten().unwrap_or_default();
        let noopener = crate::window::element_noopener(rel, target.as_deref());
        // `rel=noreferrer`, else the link's `referrerpolicy` state.
        let referrer_policy = pyro_net::ReferrerPolicy::of_link(
            dom.get_attribute(el, "rel").ok().flatten(),
            dom.get_attribute(el, "referrerpolicy").ok().flatten(),
        );
        (url, target, noopener, referrer_policy, pings)
    };
    if let Some(pings) = pings {
        send_pings(page, pings, &url, referrer_policy);
    }
    let popup = crate::window::element_popup_ticket(page, target.as_deref(), noopener);
    page.navigate(crate::page::NavigationRequest {
        url,
        target,
        method: "GET".into(),
        body: None,
        content_type: None,
        opener: popup.is_some(),
        popup,
        window_features: Default::default(),
        referrer_policy,
        source: Some(el),
        form_entries: None,
        replace: false,
        user_initiated: user_initiated_navigation(),
    });
}

/// An HTML `<a>` or `<area>`'s `ping` URLs (http(s) only, resolved against
/// its base URL) and its document's URL; `None` without any.
fn hyperlink_pings(dom: &Dom, el: NodeId) -> Option<(Vec<pyro_net::Url>, Option<pyro_net::Url>)> {
    let e = dom.element(el).ok()?;
    if !e.is_html() || !matches!(&*e.name.local, "a" | "area") {
        return None;
    }
    let urls: Vec<pyro_net::Url> = e
        .attr("ping")?
        .split_ascii_whitespace()
        .filter_map(|token| pyro_net::Url::parse(&crate::html::resolve_url(dom, el, token)).ok())
        .filter(|u| matches!(u.scheme(), "http" | "https"))
        .collect();
    if urls.is_empty() {
        return None;
    }
    let document = dom.node_document(el).ok().and_then(|d| dom.document_url_parsed(d).ok().flatten().cloned());
    Some((urls, document))
}

/// Hyperlink auditing for a followed link (`policy`: the link's own referrer
/// policy, else its document's).
fn send_pings(
    page: &Rc<PageDom>,
    (urls, document): (Vec<pyro_net::Url>, Option<pyro_net::Url>),
    target: &str,
    policy: Option<pyro_net::ReferrerPolicy>,
) {
    let policy = policy.unwrap_or_else(|| page.referrer_policy());
    crate::fetch::send_pings(page, urls, document, target, policy);
}

// ----- forms ------------------------------------------------------------------

/// Form submission algorithm (simplified HTML §4.10.21.3): `requestSubmit`
/// fires `submit` (with interactive validation), then navigates with the
/// form data set via the page hook.
pub fn submit_form(page: &Rc<PageDom>, form: NodeId, submitter: Option<NodeId>, request: bool) {
    // HTML "submit" step 1: a form that cannot navigate (it is not connected)
    // is not submitted, and gets no `submit` event.
    if !page.dom.borrow().is_connected(form).unwrap_or(false) {
        return;
    }
    // HTML "submit" step 3: a document with the sandboxed forms browsing context flag (a sandbox
    // without `allow-forms`) submits nothing, and fires no `submit` event.
    if page.forms_sandboxed() {
        return;
    }
    if !page.begin_submit(form) {
        return;
    }
    let proceed = if request {
        let novalidate = {
            let dom = page.dom.borrow();
            dom.has_attribute(form, "novalidate").unwrap_or(false)
                || submitter.is_some_and(|s| dom.has_attribute(s, "formnovalidate").unwrap_or(false))
        };
        // Validating the form gives its controls user validity, so the
        // invalid ones match `:user-invalid` from here on.
        if !novalidate {
            let _ = page.dom.borrow_mut().set_form_user_validity(form);
        }
        if !novalidate && !crate::validity::check_form(page, form, true) {
            false
        } else {
            let ev = EventCore::trusted(page, EventInterface::SubmitEvent, "submit", true, true);
            // Composed, as Chrome fires it (WPT SubmitEvent.window: submit from a shadow tree).
            ev.state.borrow_mut().composed = true;
            ev.fields.borrow_mut().submitter = submitter.and_then(|s| key_of(page, s));
            match key_of(page, form) {
                Some(k) => fire(page, k, &ev),
                None => false,
            }
        }
    } else {
        true
    };
    let dialog = proceed && crate::popover::is_dialog_method(&page.dom.borrow(), form, submitter);
    if dialog {
        crate::popover::submit_dialog(page, form, submitter);
    } else if proceed {
        let req = crate::formdata::form_submission(page, form, submitter);
        if let Some(req) = req {
            page.plan_form_navigation(form, req);
        }
    }
    page.end_submit(form);
}

/// `form.reset()`: fires `reset`, then restores default values.
pub fn reset_form(page: &Rc<PageDom>, form: NodeId) {
    if !fire_simple(page, form, "reset", true, true) {
        return;
    }
    // The form's resettable elements: those it owns anywhere in its tree
    // (`form=` associates controls outside it), and the options of its
    // selects.
    let controls: Vec<NodeId> = {
        let dom = page.dom.borrow();
        let mut out = Vec::new();
        for n in dom.owned_form_elements(form).unwrap_or_default() {
            if matches!(local(&dom, n).as_deref(), Some("input" | "select" | "textarea" | "output")) {
                out.push(n);
                if local(&dom, n).as_deref() == Some("select") {
                    out.extend(dom.select_options(n).unwrap_or_default());
                }
            }
        }
        out
    };
    let mut dom = page.dom.borrow_mut();
    for &c in &controls {
        // `<output>`'s reset: its text becomes its default value.
        if local(&dom, c).as_deref() == Some("output") {
            let _ = dom.reset_output(c);
            continue;
        }
        let _ = dom.reset_control(c);
        // A file input's selected files are emptied.
        if dom.input_type(c).ok() == Some(InputType::File) && local(&dom, c).as_deref() == Some("input") {
            page.clear_input_files(c);
        }
    }
    // A select's reset updates its selectedcontent elements.
    for &c in &controls {
        if local(&dom, c).as_deref() == Some("select") {
            let _ = dom.selectedcontent_after_reset(c);
        }
    }
    // Form-associated custom elements reset themselves (formResetCallback).
    let _ = dom.enqueue_form_reset(form);
}

// ----- keyboard ---------------------------------------------------------------------

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub alt: bool,
    pub ctrl: bool,
    pub meta: bool,
    pub shift: bool,
}

impl Modifiers {
    /// CDP bit field: Alt=1, Ctrl=2, Meta/Command=4, Shift=8.
    pub fn from_cdp(bits: u32) -> Self {
        Modifiers { alt: bits & 1 != 0, ctrl: bits & 2 != 0, meta: bits & 4 != 0, shift: bits & 8 != 0 }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum KeyKind {
    /// keydown (+ keypress and text insertion when `text` is non-empty).
    KeyDown,
    /// keydown only.
    RawKeyDown,
    /// keypress + text insertion.
    Char,
    KeyUp,
}

#[derive(Clone, Debug, Default)]
pub struct KeyInput {
    pub key: String,
    pub code: String,
    pub text: String,
    pub modifiers: Modifiers,
    pub location: u32,
    pub auto_repeat: bool,
    pub key_code: u32,
    /// CDP `commands`: the macOS editing commands of the key (Playwright
    /// sends them on macOS), run in place of its default action.
    pub commands: Vec<String>,
}

/// Element receiving key events: focused element, else body/document element.
pub fn key_target(page: &PageDom) -> Option<NodeId> {
    let dom = page.dom.borrow();
    let doc = page.document();
    dom.focused(doc)
        .ok()
        .flatten()
        .or_else(|| dom.body(doc).ok().flatten())
        .or_else(|| dom.document_element(doc).ok().flatten())
}

fn key_event(page: &Rc<PageDom>, ty: &str, k: &KeyInput, char_code: u32) -> EventData {
    let ev = EventCore::trusted(page, EventInterface::KeyboardEvent, ty, true, true);
    {
        let mut f = ev.fields.borrow_mut();
        f.key = k.key.clone();
        f.code = k.code.clone();
        f.location = k.location;
        f.repeat = k.auto_repeat;
        f.key_code = if ty == "keypress" { char_code } else { k.key_code };
        f.char_code = if ty == "keypress" { char_code } else { 0 };
        f.ctrl_key = k.modifiers.ctrl;
        f.shift_key = k.modifiers.shift;
        f.alt_key = k.modifiers.alt;
        f.meta_key = k.modifiers.meta;
    }
    ev
}

/// CDP `Input.dispatchKeyEvent`.
pub fn dispatch_key(page: &Rc<PageDom>, kind: KeyKind, k: &KeyInput) {
    if !accepts_keyboard(page) {
        return;
    }
    let doc = page.document();
    let Some(target) = key_target(page) else { return };
    let Some(tkey) = key_of(page, target) else { return };
    // HTML activation-triggering input: a keydown other than Escape.
    if matches!(kind, KeyKind::KeyDown | KeyKind::RawKeyDown) && k.key != "Escape" {
        page.notify_input_activation();
    }
    match kind {
        KeyKind::KeyDown | KeyKind::RawKeyDown => {
            if !(k.modifiers.ctrl || k.modifiers.meta || k.modifiers.alt) {
                // Keyboard use makes focus visible, the current focus too.
                page.pointer_modality.set(false);
                let _ = page.dom.borrow_mut().set_focus_visible(doc, true);
            }
            let ok = crate::target::unheard_input(page, "keydown", tkey, true, Some(k.code.as_str())) || fire(page, tkey, &key_event(page, "keydown", k, 0));
            if !ok || page.document() != doc || !accepts_keyboard(page) {
                return;
            }
            let Some(target) = key_target(page) else { return };
            if !(k.modifiers.ctrl || k.modifiers.meta || k.modifiers.alt)
                && (select_picker_key(page, target, &k.key) || base_listbox_key(page, target, &k.key))
            {
                return;
            }
            if k.key == " " && space_clickable(&page.dom.borrow(), target) {
                // Browsers mark the control active on Space keydown and click
                // it on the matching keyup (Blink KeyboardClickableInputTypeView).
                page.space_target.set(Some(target));
                // Only the control itself is `:active`, not its ancestors (a press makes the chain).
                let restyle = page.layout.try_borrow().map_or(true, |l| l.style().active_styled());
                let _ = page.dom.borrow_mut().set_activated_alone(doc, Some(target), restyle);
            }
            // A focusgroup's arrow, Home and End keys move focus before the editing commands the
            // key carries (`moveRight`, ...) are run: those only act in editable content, which
            // keeps its keys (`is_directional_input_handler`).
            if !(k.modifiers.alt || k.modifiers.ctrl || k.modifiers.meta || k.modifiers.shift)
                && focusgroup_key_action(page, target, &k.key)
            {
                return;
            }
            // So do the value-changing keys of form controls (radio buttons, range and number
            // inputs, selects): those commands would move a caret instead.
            if !(k.modifiers.alt || k.modifiers.ctrl || k.modifiers.meta) && control_key_action(page, target, &k.key) {
                return;
            }
            // Outside editable content a caret command (`moveDown`, `pageDown`, ... which Playwright
            // sends with the key on macOS) moves nothing: the key scrolls instead, as it does on the
            // platforms that send no commands. (Shift+arrow, which extends the selection, is no scroll key.)
            if k.text.is_empty() && !k.commands.is_empty() && space_scrolls(page, target) && keyboard_scroll(page, k) {
                return;
            }
            if k.text.is_empty()
                && !k.commands.is_empty()
                && crate::edit_commands::run_commands(page, target, &k.commands)
            {
                return;
            }
            if kind == KeyKind::KeyDown && !k.text.is_empty() && !shortcut(k) {
                press_char(page, target, k);
            } else {
                default_key_action(page, target, k);
            }
        }
        KeyKind::Char => press_char(page, target, k),
        KeyKind::KeyUp => {
            let ok = crate::target::unheard_input(page, "keyup", tkey, true, Some(k.code.as_str())) || fire(page, tkey, &key_event(page, "keyup", k, 0));
            if k.key == " " && page.space_target.get().is_some() {
                set_active(page, doc, None);
            }
            if k.key == " "
                && page.space_target.take() == Some(target)
                && ok
                && page.document() == doc
                && accepts_keyboard(page)
                && key_target(page) == Some(target)
            {
                keyboard_click(page, target);
            }
        }
    }
}

/// Ctrl or Meta (without Alt) makes a key a shortcut: no keypress and no
/// text, as in Chromium.
fn shortcut(k: &KeyInput) -> bool {
    (k.modifiers.ctrl || k.modifiers.meta) && !k.modifiers.alt
}

fn press_char(page: &Rc<PageDom>, target: NodeId, k: &KeyInput) {
    if shortcut(k) {
        return;
    }
    let code = k.text.chars().next().map_or(0, |c| c as u32);
    let Some(tkey) = key_of(page, target) else { return };
    let doc = page.document();
    let ok = crate::target::unheard(page, "keypress") || fire(page, tkey, &key_event(page, "keypress", k, code));
    if !ok || page.document() != doc || !accepts_keyboard(page) {
        return;
    }
    let Some(target) = key_target(page) else { return };
    if k.modifiers.ctrl || k.modifiers.meta {
        return;
    }
    if let Some(host) = editing_host_of(page, target) {
        if (k.text == "\r" || k.text == "\n") && k.modifiers.shift {
            crate::editing::insert_line_break(page, host);
        } else if k.text == "\r" || k.text == "\n" {
            crate::editing::enter(page, host);
        } else {
            crate::editing::insert_text(page, host, &k.text);
        }
        return;
    }
    if k.text == "\r" || k.text == "\n" {
        enter_key(page, target);
    } else if !select_typeahead(page, target, &k.text) && !(k.key == " " && space_scrolls(page, target) && keyboard_scroll(page, k)) {
        // (Space that activates nothing and is no text scrolls by a page: Chrome's
        // `DefaultSpaceEventHandler`, a keypress default action.)
        insert_text_at(page, target, &k.text);
    }
}

/// The contenteditable host the focused element edits, if any.
fn editing_host_of(page: &PageDom, target: NodeId) -> Option<NodeId> {
    // In a designMode document typing goes where the selection is (the focus stays on the body
    // or the document): the editing host of the selection's start, when it is in the document.
    let design = {
        let dom = page.dom.borrow();
        dom.node_document(target).ok().filter(|&doc| dom.design_mode(doc))
    };
    if let Some(doc) = design {
        let Some((start, _)) = crate::editing::selection_range(page) else {
            // pyro: no selection yet (none is made by turning designMode on, as in Blink): typing
            // inserts nothing, until a click or a script places one.
            return None;
        };
        let dom = page.dom.borrow();
        return (dom.node_document(start.0).ok() == Some(doc))
            .then(|| crate::editing::editing_host(&dom, start.0))
            .flatten();
    }
    crate::editing::editing_host(&page.dom.borrow(), target)
}

/// Default actions for keys in a contenteditable host.
fn editing_key_action(page: &Rc<PageDom>, host: NodeId, k: &KeyInput) {
    use crate::editing;
    match k.key.as_str() {
        "Backspace" => editing::delete(page, host, true),
        "Delete" => editing::delete(page, host, false),
        "Enter" => editing::enter(page, host),
        "Tab" => focus_next(page, host, k.modifiers.shift),
        "a" | "A" if k.modifiers.ctrl || k.modifiers.meta => editing::select_all(page, host),
        "ArrowLeft" | "ArrowRight" | "Home" | "End" => editing::move_caret(page, host, &k.key, k.modifiers.shift),
        _ => {}
    }
}

/// Chromium's `SearchInputType::HandleKeydownEvent`: Escape empties an
/// enabled, writable `<input type=search>` that has a value, fires `input`
/// (the field has focus, so `change` waits for the blur) and then `search`.
fn clear_search_field(page: &Rc<PageDom>, el: NodeId) -> bool {
    {
        let mut dom = page.dom.borrow_mut();
        let search = local(&dom, el).as_deref() == Some("input")
            && dom.input_type(el).ok() == Some(InputType::Search)
            && !dom.is_disabled(el).unwrap_or(true)
            && !dom.has_attribute(el, "readonly").unwrap_or(false);
        if !search || dom.value(el).map_or(true, |v| v.is_empty()) || dom.set_value(el, "").is_err() {
            return false;
        }
        if let Ok(c) = dom.control_state_mut(el) {
            c.edited_since_focus = true;
        }
    }
    fire_simple(page, el, "input", true, false);
    fire_simple(page, el, "search", true, false);
    true
}

/// Default actions for non-text keys on the focused element.
fn default_key_action(page: &Rc<PageDom>, target: NodeId, k: &KeyInput) {
    // Escape in a filled search field clears it first (Chromium's
    // SearchInputType: `input`, then `search`); no close request follows.
    if k.key == "Escape" && clear_search_field(page, target) {
        return;
    }
    // A close request: the newest open dialog or auto popover closes.
    if k.key == "Escape" && crate::popover::close_request(page) {
        return;
    }
    // Word/line movement and deletion, clipboard and undo shortcuts,
    // Shift+Enter and line keys in editable content.
    if crate::edit_commands::key_action(page, target, k) {
        return;
    }
    if let Some(host) = editing_host_of(page, target) {
        editing_key_action(page, host, k);
        return;
    }
    if !(k.modifiers.alt || k.modifiers.ctrl || k.modifiers.meta) && control_key_action(page, target, &k.key) {
        return;
    }
    if !(k.modifiers.alt || k.modifiers.ctrl || k.modifiers.meta || k.modifiers.shift)
        && focusgroup_key_action(page, target, &k.key)
    {
        return;
    }
    let editable = is_text_control(&page.dom.borrow(), target);
    match k.key.as_str() {
        "Backspace" if editable => delete(page, target, "deleteContentBackward", true),
        "Delete" if editable => delete(page, target, "deleteContentForward", false),
        "Enter" => enter_key(page, target),
        "Tab" => focus_next(page, target, k.modifiers.shift),
        "a" | "A" if editable && (k.modifiers.ctrl || k.modifiers.meta) => {
            {
                let mut dom = page.dom.borrow_mut();
                if let Ok(v) = dom.value(target) {
                    let _ = dom.set_selection_range(target, 0, pyro_dom::dom::utf16_len(&v));
                }
            }
            crate::editing::queue_control_selectionchange(page, target);
        }
        "ArrowLeft" | "ArrowRight" | "Home" | "End" if editable => move_caret(page, target, &k.key, k.modifiers.shift),
        // Outside editable content Home/End scroll the document (Chrome's
        // scrollToBeginningOfDocument / scrollToEndOfDocument).
        // The focused element's nearest scroller that can move, else the viewport.
        "Home" | "End" if !(k.modifiers.alt || k.modifiers.ctrl || k.modifiers.meta || k.modifiers.shift) => {
            keyboard_scroll(page, k);
        }
        "ArrowUp" | "ArrowDown"
            if !(k.modifiers.alt || k.modifiers.ctrl || k.modifiers.meta)
                && vertical_caret_control(&page.dom.borrow(), target) =>
        {
            move_caret_vertically(page, target, k.key == "ArrowUp", k.modifiers.shift)
        }
        "ArrowUp" | "ArrowDown" | "ArrowLeft" | "ArrowRight" | "PageUp" | "PageDown" | " " => {
            keyboard_scroll(page, k);
        }
        _ => {}
    }
}

/// Whether a scroll key on key target `target` may scroll: not when the target takes the key itself
/// (a text control, editable content, `designMode`: Space as text, the arrows as caret movement).
/// (The DOM borrow ends before the scroll runs.)
fn space_scrolls(page: &PageDom, target: NodeId) -> bool {
    !is_directional_input_handler(&page.dom.borrow(), target)
}

/// How far a keyboard scroll goes (Chrome's scroll granularities).
#[derive(Copy, Clone, PartialEq, Eq)]
enum KeyScrollStep {
    /// An arrow key: `kPixelsPerLineStep` (40 CSS px).
    Line,
    /// PageUp, PageDown and Space: most of the scrollport.
    Page,
    /// Home and End: to the start or the end.
    Edge,
}

/// The default action of the keys that scroll (Chrome's `KeyboardEventManager` default handlers
/// and `ScrollManager::BubblingScroll`): an arrow key scrolls by a line (40px), PageUp, PageDown
/// and Space (Shift reverses) by a page along the block axis, Home and End to the start or end
/// of it. The scroll starts at the focused element, else at the last mouse press (or the
/// nearest ancestor of it still in the document), else at the viewport, and goes to the nearest
/// scroll container on the way up that can move in that direction (the viewport last); one that
/// is at its limit passes the scroll on unless its `overscroll-behavior` contains it. A scroller
/// with `scroll-behavior: smooth` animates, as for script. Returns whether the key is a scroll
/// key this applies to (a control that keeps the key, or one Space activates, is not).
fn keyboard_scroll(page: &Rc<PageDom>, k: &KeyInput) -> bool {
    if k.modifiers.ctrl || k.modifiers.alt || k.modifiers.meta {
        return false;
    }
    let shift = k.modifiers.shift;
    // (granularity, physical direction of an arrow, direction along the block axis).
    let (step, physical, block): (KeyScrollStep, (i32, i32), i32) = match (k.key.as_str(), shift) {
        ("ArrowUp", false) => (KeyScrollStep::Line, (0, -1), 0),
        ("ArrowDown", false) => (KeyScrollStep::Line, (0, 1), 0),
        ("ArrowLeft", false) => (KeyScrollStep::Line, (-1, 0), 0),
        ("ArrowRight", false) => (KeyScrollStep::Line, (1, 0), 0),
        ("PageUp", false) => (KeyScrollStep::Page, (0, 0), -1),
        ("PageDown", false) => (KeyScrollStep::Page, (0, 0), 1),
        (" ", shift) => (KeyScrollStep::Page, (0, 0), if shift { -1 } else { 1 }),
        ("Home", false) => (KeyScrollStep::Edge, (0, 0), -1),
        ("End", false) => (KeyScrollStep::Edge, (0, 0), 1),
        _ => return false,
    };
    let (start, root) = {
        let dom = page.dom.borrow();
        let doc = page.document();
        let root = dom.document_element(doc).ok().flatten();
        let start = match dom.focused(doc).ok().flatten() {
            Some(f) => {
                // The control keeps its keys (caret, value, picker), a button, checkbox or summary
                // takes Space.
                if is_directional_input_handler(&dom, f) || (k.key == " " && space_clickable(&dom, f)) {
                    return false;
                }
                Some(f)
            }
            None => LAST_PRESS.with(|c| {
                let c = c.borrow();
                let (owner, chain) = (*c).as_ref()?;
                if *owner != Rc::as_ptr(page) as usize {
                    return None;
                }
                let found = chain.iter().copied().find(|&n| dom.is_connected(n).unwrap_or(false));
                found
            }),
        };
        (start, root)
    };
    // (No borrow is held here: a smooth scroll starts its animation and queues the events.)
    let _ = crate::layout_api::perform_scroll(page, crate::layout_api::ScrollBehavior::Auto, &[], |dom, layout| {
        keyboard_scroll_write(dom, layout, start, root, step, physical, block)
    });
    true
}

/// The write of [`keyboard_scroll`]: walks up from `start` (`None`: straight to the viewport) and
/// scrolls the first scroll container that moves, else the viewport, returning the box it moved
/// (`None`: the viewport).
fn keyboard_scroll_write(
    dom: &Dom,
    layout: &mut pyro_dom::layout::Layout,
    start: Option<NodeId>,
    root: Option<NodeId>,
    step: KeyScrollStep,
    physical: (i32, i32),
    block: i32,
) -> Vec<Option<NodeId>> {
    // The direction (dx, dy) of the scroll, each -1, 0 or 1, for a box of writing mode `wm`: a
    // block-axis key follows the box's block axis (horizontal in a vertical writing mode, toward
    // the left in `vertical-rl`).
    let direction = |wm: &str| -> (i32, i32) {
        if block == 0 {
            return physical;
        }
        if wm.starts_with("vertical") || wm.starts_with("sideways") {
            (if matches!(wm, "vertical-rl" | "sideways-rl") { -block } else { block }, 0)
        } else {
            (0, block)
        }
    };
    // Scrolls a box by `dir`, `size` being its scrollport on each axis: whether it moved.
    let scroll = |layout: &mut pyro_dom::layout::Layout, el: Option<NodeId>, dir: (i32, i32), size: (f64, f64)| -> bool {
        let amount = |d: i32, extent: f64| -> Option<i32> {
            if d == 0 {
                return None;
            }
            Some(match step {
                // To the start: 0; to the end: as far as the box goes in the direction.
                KeyScrollStep::Edge => {
                    if block < 0 {
                        0
                    } else {
                        d.saturating_mul(i32::MAX)
                    }
                }
                KeyScrollStep::Line => 40 * d,
                // Chromium pages by 87.5% of the scrollport along the axis (at least 1px).
                KeyScrollStep::Page => d * (extent * 0.875).round().max(1.0).min(i32::MAX as f64) as i32,
            })
        };
        let (l, t) = (amount(dir.0, size.0), amount(dir.1, size.1));
        let edge = step == KeyScrollStep::Edge;
        match el {
            Some(el) if edge => layout.element_scroll_to(dom, el, l, t),
            Some(el) => layout.element_scroll_by(dom, el, l, t),
            None if edge => layout.window_scroll_to(dom, l, t),
            None => layout.window_scroll_by(dom, l, t),
        }
    };
    let mut cur = start;
    while let Some(e) = cur {
        if layout.is_root_container(dom, e) {
            break;
        }
        let wm = layout.style().computed_value(dom, e, "writing-mode").unwrap_or_default();
        let dir = direction(wm.as_str());
        let axes = pyro_dom::style::ScrollAxes { x: dir.0 != 0, y: dir.1 != 0 };
        let scrolls = layout.style().overflow_axes(dom, e);
        if (axes.x && scrolls.x) || (axes.y && scrolls.y) {
            let size = (layout.client_width(dom, e), layout.client_height(dom, e));
            if scroll(&mut *layout, Some(e), dir, size) {
                return vec![Some(e)];
            }
            if layout.contains_overscroll(dom, e, axes) {
                return Vec::new();
            }
        }
        cur = flat_parent_element(dom, e);
    }
    let wm = root.and_then(|r| layout.style().computed_value(dom, r, "writing-mode")).unwrap_or_default();
    let dir = direction(wm.as_str());
    let (w, h) = layout.window_size();
    if scroll(&mut *layout, None, dir, (w as f64, h as f64)) {
        vec![None]
    } else {
        Vec::new()
    }
}

/// An element that uses arrow keys itself (Chrome's focusgroup "key
/// conflict" elements): text controls, editable content, value-stepping
/// inputs, radio buttons, selects and media with controls. A focusgroup can
/// move focus onto one, but arrows do not move it away and Tab leaves it to
/// the adjacent item.
fn is_directional_input_handler(dom: &Dom, el: NodeId) -> bool {
    if is_text_control(dom, el) || crate::editing::editing_host(dom, el).is_some() {
        return true;
    }
    match local(dom, el).as_deref() {
        Some("select") => true,
        Some("input") => matches!(dom.input_type(el), Ok(InputType::Radio | InputType::Range | InputType::Number)),
        Some("video" | "audio") => dom.has_attribute(el, "controls").unwrap_or(false),
        _ => false,
    }
}

/// Scoped focusgroup arrow keys (Chrome's `FocusgroupController`): on a
/// focused focusgroup item, an arrow along an axis the owner allows moves
/// focus to the next or previous item (wrapping when the owner wraps), Home
/// and End to the first and last item. Arrows map to logical axes through the
/// focused item's own `writing-mode` and `direction` (an RTL item in an LTR
/// owner moves forward with ArrowLeft: WPT focusgroup
/// `mixed-ltr-rtl-visual-order`). Text controls keep the keys for their
/// caret. Returns whether focus moved.
fn focusgroup_key_action(page: &Rc<PageDom>, target: NodeId, key: &str) -> bool {
    use pyro_dom::focus_group::FocusgroupMove;
    if !matches!(key, "ArrowUp" | "ArrowDown" | "ArrowLeft" | "ArrowRight" | "Home" | "End") {
        return false;
    }
    let next = {
        let layout = page.layout();
        let dom = page.dom.borrow();
        if is_directional_input_handler(&dom, target) {
            return false;
        }
        if dom.focusgroup_owner_of(target).is_none() {
            return false;
        }
        let (writing_mode, direction) = {
            let style = layout.style();
            (
                style.computed_value(&dom, target, "writing-mode").unwrap_or_default(),
                style.computed_value(&dom, target, "direction").unwrap_or_default(),
            )
        };
        let vertical = writing_mode.starts_with("vertical") || writing_mode.starts_with("sideways");
        let rtl = direction == "rtl";
        // A focused scroll container keeps the arrows of the axes it scrolls in (Chrome's focusgroup
        // treats it as a directional input handler there): overflow-x the left and right keys,
        // overflow-y the up and down ones.
        let scrolls = |property: &str| {
            matches!(layout.style().computed_value(&dom, target, property).as_deref(), Some("scroll" | "auto"))
        };
        if (matches!(key, "ArrowLeft" | "ArrowRight") && scrolls("overflow-x"))
            || (matches!(key, "ArrowUp" | "ArrowDown") && scrolls("overflow-y"))
        {
            return false;
        }
        // (inline axis, forward) for each physical arrow.
        let (inline, forward) = match (key, vertical) {
            ("ArrowLeft" | "ArrowRight", false) => (true, (key == "ArrowRight") != rtl),
            ("ArrowUp" | "ArrowDown", false) => (false, key == "ArrowDown"),
            ("ArrowUp" | "ArrowDown", true) => (true, (key == "ArrowDown") != (rtl != (writing_mode == "sideways-lr"))),
            ("ArrowLeft" | "ArrowRight", true) => {
                let lr = matches!(writing_mode.as_str(), "vertical-lr" | "sideways-lr");
                (false, (key == "ArrowRight") == lr)
            }
            _ => (false, false),
        };
        let mv = match key {
            "Home" => FocusgroupMove::First,
            "End" => FocusgroupMove::Last,
            _ if forward => FocusgroupMove::Next { inline },
            _ => FocusgroupMove::Previous { inline },
        };
        dom.focusgroup_move_target(target, mv, &|n| is_focusable_area(&layout, &dom, n), Some(&layout))
    };
    match next {
        Some(n) => {
            focus_element(page, n);
            true
        }
        None => false,
    }
}

/// The value-changing keys of form controls (Chrome's default handlers):
/// ArrowUp/ArrowDown step a number input; arrows, PageUp/PageDown, Home and
/// End move a range slider; arrows move the checked radio button (and
/// focus) within its group; arrows, Home and End change a single-selection
/// `<select>`'s option. A change fires `input` and `change`. Returns
/// whether the key was such an action (handled, even when nothing changed).
fn control_key_action(page: &Rc<PageDom>, el: NodeId, key: &str) -> bool {
    let (name, ty, mutable) = {
        let dom = page.dom.borrow();
        let Some(name) = local(&dom, el) else { return false };
        if !matches!(name.as_str(), "input" | "select") {
            return false;
        }
        let ty = dom.input_type(el).ok();
        // `readonly` does not apply to a select.
        let readonly = name == "input" && dom.has_attribute(el, "readonly").unwrap_or(false);
        let mutable = !dom.is_disabled(el).unwrap_or(true) && !readonly;
        (name, ty, mutable)
    };
    let changed = match (name.as_str(), ty) {
        ("input", Some(InputType::Number)) => {
            let up = match key {
                "ArrowUp" => true,
                "ArrowDown" => false,
                _ => return false,
            };
            // Canceled (or the document went away in the listener): nothing steps, no `input`/`change`.
            if !mutable {
                false
            } else {
                match number_step(page, el, up) {
                    Some(changed) => changed,
                    None => return true,
                }
            }
        }
        ("input", Some(InputType::Range)) => {
            if !matches!(
                key,
                "ArrowUp" | "ArrowRight" | "ArrowDown" | "ArrowLeft" | "PageUp" | "PageDown" | "Home" | "End"
            ) {
                return false;
            }
            mutable && step_by_key(page, el, |dom| range_key(dom, el, key))
        }
        ("input", Some(InputType::Radio)) => {
            let forward = match key {
                "ArrowDown" | "ArrowRight" => true,
                "ArrowUp" | "ArrowLeft" => false,
                _ => return false,
            };
            let next = next_radio(&page.dom.borrow(), el, forward);
            if let Some(next) = next {
                focus_element(page, next);
                keyboard_click(page, next);
            }
            // The click fired the events.
            false
        }
        ("select", _) => {
            if !matches!(key, "ArrowUp" | "ArrowDown" | "Home" | "End")
                || page.dom.borrow().has_attribute(el, "multiple").unwrap_or(true)
            {
                return false;
            }
            mutable
                && select_by_key(page, el, |options, current| {
                    let pos = current.and_then(|c| options.iter().position(|&o| o == c));
                    match (key, pos) {
                        ("ArrowDown", Some(p)) => options.get(p + 1).copied(),
                        ("ArrowUp", Some(p)) => p.checked_sub(1).map(|p| options[p]),
                        ("ArrowDown" | "Home", _) => options.first().copied(),
                        _ => options.last().copied(),
                    }
                })
        }
        _ => return false,
    };
    if changed {
        fire_control_change(page, el);
    }
    true
}

/// A trusted left-button release (`mouseup` not canceled) on an option of a customizable select's
/// open picker, or inside one: as Chromium, the option is picked (the only selected option, by the
/// user) unless it is disabled, `input` and `change` fire when the selection changed, and the
/// picker closes with the select focused.
fn select_picker_option_release(page: &Rc<PageDom>, target: NodeId) {
    let (select, index) = {
        let dom = page.dom.borrow();
        let mut cur = Some(target);
        let mut option = None;
        while let Some(n) = cur {
            match local(&dom, n).as_deref() {
                Some("option") => {
                    option = Some(n);
                    break;
                }
                Some("select") => return,
                _ => {}
            }
            cur = dom.parent(n).ok().flatten();
        }
        let Some(option) = option else { return };
        let Some(select) = dom.option_nearest_select(option).ok().flatten() else { return };
        if !dom.is_select_picker_open(select) || dom.is_disabled(option).unwrap_or(true) {
            return;
        }
        let Some(index) = dom.select_options(select).ok().and_then(|o| o.iter().position(|&x| x == option)) else {
            return;
        };
        (select, index)
    };
    let changed = {
        let mut dom = page.dom.borrow_mut();
        let before = dom.selected_index(select).ok();
        let _ = dom.set_selected_index(select, index as i32);
        dom.selected_index(select).ok() != before
    };
    crate::popover::hide_select_picker(page, select, true);
    if changed {
        fire_control_change(page, select);
    }
}

/// A trusted left-button `mousedown` (not canceled) on an option of a list box (a select with
/// `multiple` or a display size above 1), or inside one: as Chromium, the option becomes the only
/// selected option (selectedness set by the user, so dirty) unless it is disabled or the select
/// is; `input` and `change` fire when the selection changed.
fn listbox_option_mousedown(page: &Rc<PageDom>, target: NodeId) {
    let (select, index) = {
        let dom = page.dom.borrow();
        let mut cur = Some(target);
        let mut option = None;
        while let Some(n) = cur {
            match local(&dom, n).as_deref() {
                Some("option") => {
                    option = Some(n);
                    break;
                }
                Some("select") => return,
                _ => {}
            }
            cur = dom.parent(n).ok().flatten();
        }
        let Some(option) = option else { return };
        let Some(select) = dom.option_nearest_select(option).ok().flatten() else { return };
        let multiple = dom.has_attribute(select, "multiple").unwrap_or(false);
        let size = dom.get_attribute(select, "size").ok().flatten().and_then(|s| s.trim().parse::<u32>().ok());
        if !multiple && size.is_none_or(|s| s <= 1) {
            return;
        }
        if dom.is_disabled(select).unwrap_or(true) || dom.is_disabled(option).unwrap_or(true) {
            return;
        }
        let Some(index) = dom.select_options(select).ok().and_then(|o| o.iter().position(|&x| x == option)) else {
            return;
        };
        (select, index)
    };
    let changed = {
        let mut dom = page.dom.borrow_mut();
        let before = dom.selected_options(select).unwrap_or_default();
        let _ = dom.set_selected_index(select, index as i32);
        dom.selected_options(select).unwrap_or_default() != before
    };
    if changed {
        fire_control_change(page, select);
    }
}

/// The keys of a customizable select (`appearance: base-select`, Chromium's behaviour) on a
/// trusted keydown without Ctrl, Meta or Alt. Returns whether the key was handled (no keypress
/// or other default action follows).
/// - The select focused with its picker closed: Space and the arrows open the picker without
///   changing the value; Enter opens it except on a Mac platform, and never submits the form.
/// - An option of its open picker focused: the arrows of the select's block axis (Down/Up, and in
///   a vertical writing mode Right/Left or Left/Right as its lines go; Down/Up stay secondary
///   keys) move the focus to the next or previous option that is enabled and displayed, without
///   wrapping; Home/PageUp and End/PageDown to the first or last; Enter and Space pick the
///   focused option (`input` and `change` when the value changed), close the picker and focus
///   the select. Escape is the close request (`popover::close_request`).
fn select_picker_key(page: &Rc<PageDom>, target: NodeId, key: &str) -> bool {
    let (select, option) = {
        let dom = page.dom.borrow();
        match local(&dom, target).as_deref() {
            Some("select") => (target, None),
            Some("option") => match dom.option_nearest_select(target).ok().flatten() {
                Some(select) if dom.is_select_picker_open(select) => (select, Some(target)),
                _ => return false,
            },
            _ => return false,
        }
    };
    let open = page.dom.borrow().is_select_picker_open(select);
    // The select itself focused while its picker is open: the keys act as on its selected option.
    let option = option.or_else(|| {
        let dom = page.dom.borrow();
        open.then(|| dom.selected_options(select).ok().and_then(|o| o.first().copied())).flatten()
    });
    let Some(option) = option else {
        if open || !crate::popover::has_select_picker(page, select) {
            return false;
        }
        match key {
            " " | "ArrowUp" | "ArrowDown" | "ArrowLeft" | "ArrowRight" => {
                crate::popover::show_select_picker(page, select)
            }
            "Enter" => {
                if !crate::window::platform(page).starts_with("Mac") {
                    crate::popover::show_select_picker(page, select);
                }
            }
            _ => return false,
        }
        return true;
    };
    match key {
        "Enter" | " " => {
            let changed = {
                let mut dom = page.dom.borrow_mut();
                let before = dom.selected_index(select).ok();
                let index = dom.select_options(select).ok().and_then(|o| o.iter().position(|&x| x == option));
                if let Some(index) = index {
                    let _ = dom.set_selected_index(select, index as i32);
                }
                dom.selected_index(select).ok() != before
            };
            crate::popover::hide_select_picker(page, select, true);
            if changed {
                fire_control_change(page, select);
            }
            true
        }
        "ArrowUp" | "ArrowDown" | "ArrowLeft" | "ArrowRight" | "Home" | "End" | "PageUp" | "PageDown" => {
            move_option_focus(page, select, option, key);
            true
        }
        _ => false,
    }
}

/// The arrow, Home, End, PageUp and PageDown keys on the focused `option` of `select` (an open
/// picker or a base-appearance listbox): focus moves to the next or previous option that is enabled
/// and displayed, without wrapping, along the select's block axis (Down/Up, and in a vertical writing
/// mode Right/Left or Left/Right as its lines go; the other arrow keys do nothing); Home and End go to
/// the first or last, PageDown to the last option in view (the next page when it is the focused
/// one), PageUp to the first (the previous page). The selection does not change.
fn move_option_focus(page: &Rc<PageDom>, select: NodeId, option: NodeId, key: &str) {
    let mode = {
        let dom = page.dom.borrow();
        page.layout().computed_style_value(&dom, select, "writing-mode")
    };
    // +1: the next option, -1: the previous one, 2/-2: the last/first one, 3/-3: a page down/up.
    let step: i32 = match (key, mode.as_str()) {
        ("ArrowDown", _) => 1,
        ("ArrowUp", _) => -1,
        ("ArrowRight", "vertical-lr" | "sideways-lr") | ("ArrowLeft", "vertical-rl" | "sideways-rl") => 1,
        ("ArrowLeft", "vertical-lr" | "sideways-lr") | ("ArrowRight", "vertical-rl" | "sideways-rl") => -1,
        ("ArrowLeft" | "ArrowRight", _) => return,
        ("End", _) => 2,
        ("Home", _) => -2,
        ("PageDown", _) => 3,
        _ => -3,
    };
    let (candidates, visible): (Vec<NodeId>, Vec<usize>) = {
        let dom = page.dom.borrow();
        let layout = page.layout();
        let candidates: Vec<NodeId> = dom
            .select_options(select)
            .unwrap_or_default()
            .into_iter()
            .filter(|&o| {
                o == option
                    || (!dom.is_disabled(o).unwrap_or(true) && layout.computed_style_value(&dom, o, "display") != "none")
            })
            .collect();
        // The options in view of a listbox: those whose middle is inside the select's box.
        let visible = if step.abs() == 3 {
            let sel = layout.bounding_client_rect(&dom, select);
            candidates
                .iter()
                .enumerate()
                .filter(|&(_, &o)| {
                    let r = layout.bounding_client_rect(&dom, o);
                    let mid = r.y + r.height / 2.0;
                    r.height > 0.0 && mid >= sel.y && mid <= sel.y + sel.height
                })
                .map(|(i, _)| i)
                .collect()
        } else {
            Vec::new()
        };
        (candidates, visible)
    };
    let Some(at) = candidates.iter().position(|&o| o == option) else { return };
    let last = candidates.len() - 1;
    let (first_visible, last_visible) = match (visible.first(), visible.last()) {
        (Some(&f), Some(&l)) => (f, l),
        _ => (0, last),
    };
    let page_len = (last_visible - first_visible).max(1);
    let next = match step {
        1 => (at < last).then(|| at + 1),
        -1 => at.checked_sub(1),
        2 => Some(last),
        -2 => Some(0),
        3 => Some(if at < last_visible { last_visible } else { (at + page_len).min(last) }),
        _ => Some(if at > first_visible { first_visible } else { at.saturating_sub(page_len) }),
    };
    if let Some(next) = next.filter(|&n| n != at) {
        crate::popover::focus_picker_option(page, candidates[next]);
    }
}

/// The keys of the focused option of a base-appearance listbox (`<select multiple>` or `size` above 1
/// with `appearance: base-select`; Chromium's customizable listbox): the arrow, Home, End, PageUp and
/// PageDown keys move the focus ([`move_option_focus`]); Space and Enter toggle the focused option's
/// selectedness (`input` and `change` follow), another option of a single-selection listbox losing it.
/// Returns whether the key was handled.
fn base_listbox_key(page: &Rc<PageDom>, option: NodeId, key: &str) -> bool {
    if local(&page.dom.borrow(), option).as_deref() != Some("option") {
        return false;
    }
    let select = {
        let layout = page.layout();
        let dom = page.dom.borrow();
        match option_base_listbox(&layout, &dom, option) {
            Some(select) => select,
            None => return false,
        }
    };
    match key {
        "Enter" | " " => {
            if page.dom.borrow().is_disabled(option).unwrap_or(true) || page.dom.borrow().is_disabled(select).unwrap_or(true) {
                return true;
            }
            let selected = page.dom.borrow().option_selected(option).unwrap_or(false);
            let _ = page.dom.borrow_mut().set_option_selected(option, !selected);
            fire_control_change(page, select);
            true
        }
        "ArrowUp" | "ArrowDown" | "ArrowLeft" | "ArrowRight" | "Home" | "End" | "PageUp" | "PageDown" => {
            move_option_focus(page, select, option, key);
            true
        }
        _ => false,
    }
}

/// Typeahead on the focused `option` of an open picker or a base-appearance listbox: typed keys
/// (kept for a second) move the focus, without selecting, to the next enabled, displayed option whose
/// label starts with them (the focused one first when more than one key was typed). Returns whether
/// `option` is such an option.
fn option_typeahead(page: &Rc<PageDom>, option: NodeId, text: &str) -> bool {
    let select = {
        let layout = page.layout();
        let dom = page.dom.borrow();
        if !dom.element(option).is_ok_and(|e| e.is_html() && &*e.name.local == "option") {
            return false;
        }
        match dom.option_nearest_select(option).ok().flatten() {
            Some(s) if dom.is_select_picker_open(s) || is_base_listbox(&layout, &dom, s) => s,
            _ => return false,
        }
    };
    let typed = text.to_lowercase();
    if typed.trim().is_empty() {
        return true;
    }
    let now = crate::event::page_now_ms(page);
    let search = {
        let mut state = page.option_typeahead.borrow_mut();
        if now - state.1 > 1000.0 {
            state.0.clear();
        }
        state.0.push_str(&typed);
        state.1 = now;
        state.0.clone()
    };
    // The same key typed again cycles through the options starting with it.
    let first = search.chars().next().unwrap_or(' ');
    let repeated = search.chars().all(|c| c == first);
    let needle = if repeated { first.to_string() } else { search };
    let found = {
        let dom = page.dom.borrow();
        let layout = page.layout();
        let options: Vec<NodeId> = dom
            .select_options(select)
            .unwrap_or_default()
            .into_iter()
            .filter(|&o| {
                o == option
                    || (!dom.is_disabled(o).unwrap_or(true) && layout.computed_style_value(&dom, o, "display") != "none")
            })
            .collect();
        let at = options.iter().position(|&o| o == option).unwrap_or(0);
        let start = if needle.chars().count() == 1 { at + 1 } else { at };
        (0..options.len())
            .map(|k| options[(start + k) % options.len()])
            .find(|&o| dom.option_text(o).unwrap_or_default().to_lowercase().starts_with(&needle))
    };
    if let Some(found) = found.filter(|&f| f != option) {
        crate::popover::focus_picker_option(page, found);
    }
    true
}

/// `input` and `change` for a value the user committed at once (a key
/// stepped or picked it): blurring later fires no second `change`.
fn fire_control_change(page: &Rc<PageDom>, el: NodeId) {
    fire_input_event(page, el, "input", "", None, false);
    fire_user_change(page, el);
    if page.focus_values.borrow().contains_key(&el) {
        page.remember_focus_value(el);
    }
}

/// A user step of a number input (arrow key, spin button): Chromium fires a cancelable
/// `beforeinput` (empty `inputType`) before it changes the value. `None` when a listener canceled
/// it or removed the document or element; else whether the value changed.
fn number_step(page: &Rc<PageDom>, el: NodeId, up: bool) -> Option<bool> {
    let doc = page.document();
    if !fire_input_event(page, el, "beforeinput", "", None, true) || page.document() != doc || !live_element(page, doc, el) {
        return None;
    }
    Some(step_by_key(page, el, |dom| dom.step_value(el, 1.0, up)))
}

/// The width of a number input's spin button, from its right edge.
const SPIN_BUTTON_WIDTH: f64 = 15.0;

/// A left press (mousedown not canceled) on the spin button of an enabled, writable
/// `<input type=number>`: the upper half steps up, the lower half down, as the arrow keys do
/// (`beforeinput`, then `input` and `change`).
fn number_spin_press(page: &Rc<PageDom>, target: NodeId, m: &MouseInput) {
    let rect = {
        let dom = page.dom.borrow();
        let is_number = local(&dom, target).as_deref() == Some("input")
            && dom.input_type(target).ok() == Some(InputType::Number)
            && !dom.is_disabled(target).unwrap_or(true)
            && !dom.has_attribute(target, "readonly").unwrap_or(false);
        if !is_number {
            return;
        }
        page.layout().bounding_client_rect(&dom, target)
    };
    if m.x < rect.x + rect.width - SPIN_BUTTON_WIDTH || m.x >= rect.x + rect.width || m.y < rect.y || m.y >= rect.y + rect.height {
        return;
    }
    if number_step(page, target, m.y < rect.y + rect.height / 2.0) == Some(true) {
        fire_control_change(page, target);
    }
}

/// Runs a value change of `el` and tells whether its value changed.
fn step_by_key(page: &Rc<PageDom>, el: NodeId, f: impl FnOnce(&mut Dom) -> pyro_dom::DomResult<()>) -> bool {
    let mut dom = page.dom.borrow_mut();
    let before = dom.value(el).ok();
    let _ = f(&mut dom);
    dom.value(el).ok() != before
}

/// An enabled `<input type=range>`: `el` itself (pyro's slider has no inner parts to hit).
fn enabled_range(dom: &Dom, el: NodeId) -> bool {
    local(dom, el).as_deref() == Some("input")
        && dom.input_type(el).ok() == Some(InputType::Range)
        && !dom.is_disabled(el).unwrap_or(true)
}

/// Moves a range slider's value to the pointer (Chromium's slider: the fraction of the element's
/// extent along its inline axis, from the inline start, snapped to its step): x and the width in a
/// horizontal writing mode, y and the height in a vertical one; the start is the left or top edge,
/// the opposite one for `direction: rtl` (and for `sideways-lr`, whose inline axis runs bottom to
/// top). True when the value changed.
fn range_value_at(page: &Rc<PageDom>, el: NodeId, x: f64, y: f64) -> bool {
    let (rect, writing_mode, direction) = {
        let dom = page.dom.borrow();
        let layout = page.layout();
        let style = layout.style();
        (
            layout.bounding_client_rect(&dom, el),
            style.computed_value(&dom, el, "writing-mode").unwrap_or_default(),
            style.computed_value(&dom, el, "direction").unwrap_or_default(),
        )
    };
    let vertical = writing_mode.starts_with("vertical") || writing_mode.starts_with("sideways");
    let (pos, start, extent) = if vertical { (y, rect.y, rect.height) } else { (x, rect.x, rect.width) };
    if !(extent > 0.0) {
        return false;
    }
    let mut fraction = ((pos - start) / extent).clamp(0.0, 1.0);
    if (direction == "rtl") != (writing_mode == "sideways-lr") {
        fraction = 1.0 - fraction;
    }
    let mut dom = page.dom.borrow_mut();
    let attr = |dom: &Dom, name: &str| dom.get_attribute(el, name).ok().flatten().and_then(pyro_dom::forms::parse_float);
    let min = attr(&dom, "min").unwrap_or(0.0);
    let max = attr(&dom, "max").unwrap_or(100.0).max(min);
    let before = dom.value(el).ok();
    let _ = dom.set_value_as_number(el, min + fraction * (max - min));
    dom.value(el).ok() != before
}

/// A left press on an enabled range slider whose mousedown was not canceled: the thumb jumps to
/// the pointer (`input`) and follows it while the button is held (`range_drag_move`); the release
/// commits (`change` when the value moved since the press), as in Chromium.
fn range_press(page: &Rc<PageDom>, target: NodeId, m: &MouseInput) {
    if !enabled_range(&page.dom.borrow(), target) {
        return;
    }
    let start = page.dom.borrow().value(target).unwrap_or_default();
    *page.range_drag.borrow_mut() = Some((target, start));
    if range_value_at(page, target, m.x, m.y) {
        fire_simple(page, target, "input", true, false);
    }
}

fn range_drag_move(page: &Rc<PageDom>, m: &MouseInput) {
    let Some(el) = page.range_drag.borrow().as_ref().map(|(el, _)| *el) else { return };
    if !enabled_range(&page.dom.borrow(), el) {
        return;
    }
    if range_value_at(page, el, m.x, m.y) {
        fire_simple(page, el, "input", true, false);
    }
}

fn range_release(page: &Rc<PageDom>) {
    let Some((el, start)) = page.range_drag.borrow_mut().take() else { return };
    let now = page.dom.borrow().value(el).unwrap_or_default();
    if now != start && page.dom.borrow().is_connected(el).unwrap_or(false) {
        fire_simple(page, el, "change", true, false);
    }
}

/// A range slider key: one step for an arrow, a tenth of the range (at
/// least a step) for PageUp/PageDown, the ends for Home/End.
fn range_key(dom: &mut Dom, el: NodeId, key: &str) -> pyro_dom::DomResult<()> {
    let attr =
        |dom: &Dom, name: &str| dom.get_attribute(el, name).ok().flatten().and_then(pyro_dom::forms::parse_float);
    let min = attr(dom, "min").unwrap_or(0.0);
    let max = attr(dom, "max").unwrap_or(100.0).max(min);
    match key {
        "Home" => dom.set_value_as_number(el, min),
        "End" => dom.set_value_as_number(el, max),
        "PageUp" | "PageDown" => {
            let step = dom
                .get_attribute(el, "step")
                .ok()
                .flatten()
                .and_then(pyro_dom::forms::parse_float)
                .filter(|&s| s > 0.0)
                .unwrap_or(1.0);
            let big = step.max((max - min) / 10.0);
            let v = dom.value_as_number(el)?;
            dom.set_value_as_number(el, if key == "PageUp" { v + big } else { v - big })
        }
        _ => {
            let up = matches!(key, "ArrowUp" | "ArrowRight");
            match dom.step_value(el, 1.0, up) {
                // step=any: a hundredth of the range.
                Err(_) => {
                    let v = dom.value_as_number(el)?;
                    let delta = (max - min) / 100.0;
                    dom.set_value_as_number(el, if up { v + delta } else { v - delta })
                }
                ok => ok,
            }
        }
    }
}

/// The enabled radio button after (or before) `el` in its group, wrapping.
fn next_radio(dom: &Dom, el: NodeId, forward: bool) -> Option<NodeId> {
    let name = dom.get_attribute(el, "name").ok().flatten().filter(|n| !n.is_empty())?.to_owned();
    let root = dom.root_node(el, false).ok()?;
    let form = dom.form_owner(el).ok().flatten();
    let group: Vec<NodeId> = dom
        .descendants(root)
        .ok()?
        .filter(|&n| {
            (n == el || !dom.is_disabled(n).unwrap_or(true))
                && local(dom, n).as_deref() == Some("input")
                && dom.input_type(n).ok() == Some(InputType::Radio)
                && dom.get_attribute(n, "name").ok().flatten() == Some(name.as_str())
                && dom.form_owner(n).ok().flatten() == form
        })
        .collect();
    let pos = group.iter().position(|&n| n == el)?;
    let next = if forward { (pos + 1) % group.len() } else { (pos + group.len() - 1) % group.len() };
    (group[next] != el).then_some(group[next])
}

/// Selects the option `pick` chooses among a select's enabled options,
/// given the selected one; tells whether the selection changed.
fn select_by_key(
    page: &Rc<PageDom>,
    select: NodeId,
    pick: impl FnOnce(&[NodeId], Option<NodeId>) -> Option<NodeId>,
) -> bool {
    let mut dom = page.dom.borrow_mut();
    let Ok(options) = dom.select_options(select) else { return false };
    let enabled: Vec<NodeId> = options.into_iter().filter(|&o| !dom.is_disabled(o).unwrap_or(true)).collect();
    let current = dom.selected_options(select).ok().and_then(|s| s.first().copied());
    let current_enabled = current.filter(|c| enabled.contains(c));
    match pick(&enabled, current_enabled) {
        Some(next) if Some(next) != current => dom.set_option_selected(next, true).is_ok(),
        _ => false,
    }
}

/// Typeahead in a focused single-selection `<select>`: the next enabled
/// option (after the selected one, wrapping) whose label starts with the
/// typed character. Returns whether `el` is such a select.
fn select_typeahead(page: &Rc<PageDom>, el: NodeId, text: &str) -> bool {
    if option_typeahead(page, el, text) {
        return true;
    }
    {
        let dom = page.dom.borrow();
        if local(&dom, el).as_deref() != Some("select") || dom.has_attribute(el, "multiple").unwrap_or(true) {
            return false;
        }
        if dom.is_disabled(el).unwrap_or(true) {
            return true;
        }
    }
    let prefix = text.to_lowercase();
    if prefix.trim().is_empty() {
        return true;
    }
    let labels: Vec<(NodeId, String)> = {
        let dom = page.dom.borrow();
        dom.select_options(el)
            .unwrap_or_default()
            .into_iter()
            .map(|o| (o, dom.option_text(o).unwrap_or_default().to_lowercase()))
            .collect()
    };
    let changed = select_by_key(page, el, |options, current| {
        let start = current.and_then(|c| options.iter().position(|&o| o == c)).map_or(0, |p| p + 1);
        (0..options.len())
            .map(|k| options[(start + k) % options.len()])
            .find(|o| labels.iter().any(|(id, label)| id == o && label.starts_with(&prefix)))
    });
    if changed {
        fire_control_change(page, el);
    }
    true
}

/// Text controls whose caret ArrowUp/ArrowDown move.
fn vertical_caret_control(dom: &Dom, el: NodeId) -> bool {
    match local(dom, el).as_deref() {
        Some("textarea") => true,
        Some("input") => matches!(
            dom.input_type(el),
            Ok(InputType::Text
                | InputType::Search
                | InputType::Tel
                | InputType::Url
                | InputType::Email
                | InputType::Password)
        ),
        _ => false,
    }
}

/// ArrowUp/ArrowDown caret movement as on macOS (the behaviour Playwright's
/// `moveUp`/`moveDown` editing commands name). A single-line input moves to its
/// start/end. A textarea moves by logical line (soft wrapping is not modelled),
/// keeping the UTF-16 column clamped to the target line, and moves to the
/// start/end from the first/last line. With a selection, Up starts from its
/// start and Down from its end; Shift extends. The preferred column is not kept
/// across successive moves.
fn move_caret_vertically(page: &Rc<PageDom>, el: NodeId, up: bool, extend: bool) {
    let multiline = local(&page.dom.borrow(), el).as_deref() == Some("textarea");
    let mut dom = page.dom.borrow_mut();
    let Ok((s, e)) = dom.selection(el) else { return };
    let (s, e) = (s as usize, e as usize);
    let Ok(v) = dom.value(el) else { return };
    let units: Vec<u16> = v.encode_utf16().collect();
    let len = units.len();
    let caret = if !multiline {
        if up {
            0
        } else {
            len
        }
    } else {
        let newline = u16::from(b'\n');
        let line_start = |pos: usize| units[..pos].iter().rposition(|&u| u == newline).map_or(0, |i| i + 1);
        let line_end = |pos: usize| units[pos..].iter().position(|&u| u == newline).map_or(len, |i| pos + i);
        let from = (if up { s } else { e }).min(len);
        let col = from - line_start(from);
        let target = if up {
            let start = line_start(from);
            if start == 0 {
                0
            } else {
                let prev = line_start(start - 1);
                prev + col.min(start - 1 - prev)
            }
        } else {
            let end = line_end(from);
            if end == len {
                len
            } else {
                let next = end + 1;
                next + col.min(line_end(next) - next)
            }
        };
        // Never split a surrogate pair.
        if target > 0 && target < len && (0xDC00..0xE000).contains(&units[target]) {
            target - 1
        } else {
            target
        }
    };
    let (ns, ne) = match (extend, up) {
        (false, _) => (caret, caret),
        (true, true) => (caret, e),
        (true, false) => (s, caret),
    };
    let _ = dom.set_selection_range(el, ns as u32, ne as u32);
    drop(dom);
    crate::editing::queue_control_selectionchange(page, el);
}

fn move_caret(page: &Rc<PageDom>, el: NodeId, key: &str, extend: bool) {
    let mut dom = page.dom.borrow_mut();
    let Ok((s, e)) = dom.selection(el) else { return };
    let len = dom.value(el).map(|v| pyro_dom::dom::utf16_len(&v)).unwrap_or(0);
    let caret = match key {
        "ArrowLeft" => {
            if s != e && !extend {
                s
            } else {
                s.saturating_sub(1)
            }
        }
        "ArrowRight" => {
            if s != e && !extend {
                e
            } else {
                (e + 1).min(len)
            }
        }
        "Home" => 0,
        _ => len,
    };
    let (ns, ne) = if extend {
        match key {
            "ArrowLeft" | "Home" => (caret, e),
            _ => (s, caret),
        }
    } else {
        (caret, caret)
    };
    let _ = dom.set_selection_range(el, ns, ne);
    drop(dom);
    crate::editing::queue_control_selectionchange(page, el);
}

fn delete(page: &Rc<PageDom>, el: NodeId, input_type: &str, backward: bool) {
    let doc = page.document();
    if !fire_input_event(page, el, "beforeinput", input_type, None, true)
        || page.document() != doc
        || !accepts_keyboard(page)
        || !live_element(page, doc, el)
    {
        return;
    }
    crate::edit_commands::record_control_edit(page, el, input_type);
    let changed = {
        let mut dom = page.dom.borrow_mut();
        if backward {
            dom.delete_backward(el).unwrap_or(false)
        } else {
            dom.delete_forward(el).unwrap_or(false)
        }
    };
    crate::edit_commands::note_control_edited(page, el);
    if changed {
        fire_input_event(page, el, "input", input_type, None, false);
    }
}

fn enter_key(page: &Rc<PageDom>, target: NodeId) {
    let l = local(&page.dom.borrow(), target);
    match l.as_deref() {
        Some("textarea") => insert_text_at(page, target, "\n"),
        Some("input") => {
            let ty = page.dom.borrow().input_type(target).ok();
            // Enter activates a button input itself.
            if matches!(ty, Some(InputType::Submit | InputType::Image | InputType::Reset | InputType::Button)) {
                keyboard_click(page, target);
                return;
            }
            let action = implicit_submission(&page.dom.borrow(), target);
            match action {
                ImplicitSubmission::Click(b) => element_click(page, b),
                ImplicitSubmission::Submit(f) => submit_form(page, f, None, true),
                ImplicitSubmission::Nothing => {}
            }
        }
        Some("a") | Some("button") => element_click(page, target),
        Some("summary") => keyboard_click(page, target),
        _ => {}
    }
}

enum ImplicitSubmission {
    Click(NodeId),
    Submit(NodeId),
    Nothing,
}

/// A field that blocks implicit submission (HTML §4.10.21.2): Enter in one
/// submits a form that has no submit button only when it is the only one.
fn blocks_implicit_submission(ty: InputType) -> bool {
    use InputType::*;
    matches!(ty, Text | Search | Tel | Url | Email | Password | Date | Month | Week | Time | DateTimeLocal | Number)
}

/// What Enter in the input `field` does, as in Chrome
/// (HTMLFormElement::SubmitImplicitly): the form's default button (its
/// first submit button, disabled or not) is clicked when enabled, and a
/// disabled one blocks submission; without one, a field that blocks
/// implicit submission submits the form when it is the only such field.
/// Other inputs (checkboxes, radios, ...) click the first enabled submit
/// button and never submit without one.
fn implicit_submission(dom: &Dom, field: NodeId) -> ImplicitSubmission {
    let Some(form) = dom.form_owner(field).ok().flatten() else { return ImplicitSubmission::Nothing };
    let Ok(controls) = dom.owned_form_elements(form) else { return ImplicitSubmission::Nothing };
    let trigger = dom.input_type(field).is_ok_and(blocks_implicit_submission);
    let mut blocking = 0;
    for n in controls {
        let Some(name) = local(dom, n) else { continue };
        if !matches!(name.as_str(), "input" | "button") { continue; }
        if is_submit_button(dom, n) {
            if !dom.is_disabled(n).unwrap_or(false) {
                return ImplicitSubmission::Click(n);
            }
            if trigger {
                return ImplicitSubmission::Nothing;
            }
        } else if name == "input" && dom.input_type(n).is_ok_and(blocks_implicit_submission) {
            blocking += 1;
        }
    }
    if trigger && blocking == 1 {
        ImplicitSubmission::Submit(form)
    } else {
        ImplicitSubmission::Nothing
    }
}

fn insert_text_at(page: &Rc<PageDom>, el: NodeId, text: &str) {
    if !is_text_control(&page.dom.borrow(), el) {
        return;
    }
    let input_type = if text == "\n" { "insertLineBreak" } else { "insertText" };
    let data = (text != "\n").then_some(text);
    let doc = page.document();
    if !fire_input_event(page, el, "beforeinput", input_type, data, true)
        || page.document() != doc
        || !accepts_keyboard(page)
        || !live_element(page, doc, el)
    {
        return;
    }
    crate::edit_commands::record_control_edit(page, el, input_type);
    let changed = page.dom.borrow_mut().insert_text(el, text).unwrap_or(false);
    crate::edit_commands::note_control_edited(page, el);
    if changed {
        crate::editing::queue_control_selectionchange(page, el);
        fire_input_event(page, el, "input", input_type, data, false);
    }
}

/// CDP `Input.insertText`: inserts into the focused text control, or the live
/// remembered editing control after blur/window activation. Empty text deletes
/// the selection. Owner focus and document lifetime are checked after events.
pub fn insert_text(page: &Rc<PageDom>, text: &str) {
    if !accepts_keyboard(page) {
        return;
    }
    let target = page.dom.borrow().focused(page.document()).ok().flatten();
    if let Some(host) = target.and_then(|t| editing_host_of(page, t)) {
        if text.is_empty() {
            if crate::editing::selection_range(page).is_some_and(|(s, e)| s != e) {
                crate::editing::delete(page, host, true);
            }
        } else {
            crate::editing::insert_text(page, host, text);
        }
        return;
    }
    let Some(target) = target.or_else(|| remembered_editing_target(page)) else { return };
    if text.is_empty() {
        let has_selection = page.dom.borrow().selection(target).map(|(s, e)| s != e).unwrap_or(false);
        if has_selection {
            delete(page, target, "deleteContentBackward", true);
        }
        return;
    }
    insert_text_at(page, target, text);
}

/// Sequential focus navigation (Tab / Shift+Tab) from the focused element
/// through the flattened tabindex-ordered scopes of the document, its
/// shadow trees and slots (see `pyro_dom::focus_nav`), over focusable areas.
///
/// A focusgroup segment (a run of one owner's items with nothing else
/// between them in that order) is one tab stop: navigation passes over the
/// other items of the segment it leaves, and entering a segment lands on
/// its entry (`Dom::focusgroup_entry`) from either side.
fn focus_next(page: &Rc<PageDom>, _from: NodeId, backward: bool) {
    // Tab or Shift+Tab inside an open customizable select's picker closes it.
    let open_picker = {
        let dom = page.dom.borrow();
        let focused = dom.focused(page.document()).ok().flatten();
        focused.and_then(|f| match local(&dom, f).as_deref() {
            Some("select") => Some(f),
            Some("option") => dom.option_nearest_select(f).ok().flatten(),
            _ => None,
        })
    };
    if let Some(select) = open_picker.filter(|&s| page.dom.borrow().is_select_picker_open(s)) {
        crate::popover::hide_select_picker(page, select, false);
    }
    // Moving focus with Tab ends the `:active` state of a button or checkable input, or of the
    // label pressed for one, whose pointer is still down.
    {
        let doc = page.document();
        let dom = page.dom.borrow();
        let mut cur = dom.activated(doc);
        let mut clears = false;
        while let Some(n) = cur {
            if space_clickable(&dom, n) || (local(&dom, n).as_deref() == Some("label") && dom.labeled_control(n).ok().flatten().is_some_and(|c| space_clickable(&dom, c))) {
                clears = true;
                break;
            }
            cur = dom.parent(n).ok().flatten();
        }
        drop(dom);
        if clears {
            set_active(page, doc, None);
        }
    }
    let next = {
        let layout = page.layout();
        let dom = page.dom.borrow();
        let doc = page.document();
        let area = |n: NodeId| is_focusable_area(&layout, &dom, n);
        let step = |cur: Option<NodeId>| dom.sequential_focus_target_with_layout(doc, cur, backward, &area, &layout);
        // HTML "sequential focus navigation starting point": with nothing focused, the node a
        // text directive scrolled to (still in the document) is where the Tab starts from.
        let start = page.sequential_start.take().filter(|&n| dom.is_connected(n).unwrap_or(false));
        let current = dom.focused(doc).ok().flatten().or(start);
        let owner_of = |n: NodeId| dom.focusgroup_owner_of(n).map(|(o, _)| o);
        let leaving = current.and_then(owner_of);
        let mut next = step(current);
        // Tab from an item that keeps the arrow keys goes to the adjacent
        // item of its focusgroup, the way out of it. An element past the
        // group (another focusgroup's item included) is reached as from any
        // item: entering another group lands on its entry.
        let handler = current.is_some_and(|c| is_directional_input_handler(&dom, c));
        let next_item = next.filter(|&n| handler && leaving.is_some() && owner_of(n) == leaving);
        if next_item.is_some() {
            next = None;
        }
        let mut guard = 0usize;
        // Pass over the rest of the segment being left.
        while let (Some(n), Some(o)) = (next, leaving) {
            if owner_of(n) != Some(o) || guard > 100_000 {
                break;
            }
            guard += 1;
            next = step(Some(n));
        }
        match next {
            _ if next_item.is_some() => next_item,
            Some(n) => match owner_of(n) {
                Some(owner) => {
                    // The segment entered: `n` and the items of `owner`
                    // right after it in this direction.
                    let mut segment = vec![n];
                    let mut cur = n;
                    while let Some(m) = step(Some(cur)) {
                        if owner_of(m) != Some(owner) || segment.len() > 100_000 {
                            break;
                        }
                        segment.push(m);
                        cur = m;
                    }
                    if backward {
                        segment.reverse();
                    }
                    let memory = page.focusgroup_memory.borrow().get(&owner).copied();
                    let memory = memory.filter(|&m| dom.focusgroup_owner_of(m).is_some_and(|(o, f)| o == owner && f.memory));
                    let order = dom.focusgroup_items_in_order(owner, &area, Some(&layout));
                    dom.focusgroup_entry(&segment, &order, memory)
                }
                None => Some(n),
            },
            None => None,
        }
    };
    match next {
        Some(n) => focus_element(page, n),
        // Past the last (or first) focusable area focus moves to the
        // browser's UI (the page keeps its focused element): a Space held
        // on the control no longer activates it, as when it blurs.
        None => {
            if page.space_target.take().is_some() {
                let doc = page.document();
                let current = page.dom.borrow().focused(doc).ok().flatten();
                if current.is_some() && page.dom.borrow().activated(doc) == current {
                    set_active(page, doc, None);
                }
            }
        }
    }
}

/// The `<a>` or `<area>` with an `href` that is `el` or its nearest flat-tree ancestor.
fn closest_link(page: &PageDom, el: NodeId) -> Option<NodeId> {
    let dom = page.dom.borrow();
    let mut cur = Some(el);
    while let Some(n) = cur {
        let is_link = dom.element(n).is_ok_and(|e| e.is_html() && matches!(&*e.name.local, "a" | "area"));
        if is_link && dom.get_attribute(n, "href").ok().flatten().is_some() {
            return Some(n);
        }
        cur = flat_parent_element(&dom, n);
    }
    None
}

/// Tells the embedder a press went down on a link (speculation rules' `conservative` eagerness).
fn note_link_pointer_down(page: &PageDom, target: NodeId) {
    let Some(hooks) = page.hooks() else { return };
    if !hooks.wants_link_hover() {
        return;
    }
    if let Some(link) = closest_link(page, target) {
        hooks.link_pointer_down(link);
    }
}

/// The parent element in the flat tree: a slottable's slot, a shadow
/// root's host, else the parent element.
pub(crate) fn flat_parent_element(dom: &Dom, el: NodeId) -> Option<NodeId> {
    if let Some(slot) = dom.assigned_slot_any(el) {
        return Some(slot);
    }
    match dom.parent(el).ok().flatten() {
        Some(p) if dom.is_shadow_root(p).unwrap_or(false) => dom.fragment_host(p).ok().flatten(),
        Some(p) if dom.element(p).is_ok() => Some(p),
        _ => None,
    }
}

// ----- mouse ----------------------------------------------------------------------

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MouseKind {
    Pressed,
    Released,
    Moved,
    Wheel,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum MouseButton {
    #[default]
    None,
    Left,
    Middle,
    Right,
    Back,
    Forward,
}

impl MouseButton {
    /// The `MouseEvent.buttons` bit of this button.
    fn mask(self) -> u16 {
        match self {
            MouseButton::None => 0,
            MouseButton::Left => 1,
            MouseButton::Right => 2,
            MouseButton::Middle => 4,
            MouseButton::Back => 8,
            MouseButton::Forward => 16,
        }
    }

    fn dom_button(self) -> i16 {
        match self {
            MouseButton::Left | MouseButton::None => 0,
            MouseButton::Middle => 1,
            MouseButton::Right => 2,
            MouseButton::Back => 3,
            MouseButton::Forward => 4,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct MouseInput {
    pub x: f64,
    pub y: f64,
    pub button: MouseButton,
    pub buttons: u16,
    pub click_count: u32,
    pub modifiers: Modifiers,
    pub delta_x: f64,
    pub delta_y: f64,
    /// `movementX/Y` of a `pointermove`/`mousemove`: the move since the previous mouse input of
    /// the document. Set by the dispatcher, not by callers.
    pub movement: (f64, f64),
    /// A pen drives the pointer (`Input.dispatchMouseEvent` with `pointerType: "pen"`): its
    /// events are `pointerType` "pen" with its own `pointerId`. `None`: the mouse.
    pub pen: Option<PenState>,
}

/// A pen's state at one `Input.dispatchMouseEvent`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PenState {
    /// Degrees, -90 to 90.
    pub tilt_x: f64,
    pub tilt_y: f64,
    /// Degrees clockwise, 0 to 359.
    pub twist: f64,
    /// Normalized pressure while a button is down, 0 to 1.
    pub pressure: f64,
    /// -1 to 1.
    pub tangential_pressure: f64,
}

/// The `pointerId` of the pen (the mouse is 1; touches count up from 2 and skip this one, so no
/// touch pointer can share the pen's id: `setPointerCapture` and friends route by id).
pub const PEN_POINTER_ID: i32 = 1 << 30;

pub(crate) fn mouse_event(
    page: &Rc<PageDom>,
    iface: EventInterface,
    ty: &str,
    m: &MouseInput,
    cancelable: bool,
) -> EventData {
    let ev = EventCore::trusted(
        page,
        iface,
        ty,
        !matches!(ty, "mouseenter" | "mouseleave" | "pointerenter" | "pointerleave"),
        cancelable,
    );
    {
        let mut f = ev.fields.borrow_mut();
        f.client_x = m.x;
        f.client_y = m.y;
        f.screen_x = m.x;
        f.screen_y = m.y;
        // A physical mouse move or a boundary crossing changes no button.
        // MouseEvent uses 0 here; PointerEvent reserves -1 for this case.
        f.button = match ty {
            "pointermove" | "pointerrawupdate" | "pointerover" | "pointerout" | "pointerenter" | "pointerleave" => -1,
            "mousemove" | "mouseover" | "mouseout" | "mouseenter" | "mouseleave" | "dragover" => 0,
            _ => m.button.dom_button(),
        };
        f.buttons = m.buttons;
        if matches!(ty, "pointermove" | "pointerrawupdate" | "mousemove") {
            (f.movement_x, f.movement_y) = m.movement;
        }
        f.detail = m.click_count as f64;
        f.ctrl_key = m.modifiers.ctrl;
        f.shift_key = m.modifiers.shift;
        f.alt_key = m.modifiers.alt;
        f.meta_key = m.modifiers.meta;
        if iface == EventInterface::PointerEvent {
            f.pointer_id = 1;
            f.pointer_type = "mouse".into();
            f.is_primary = true;
            f.width = 1.0;
            f.height = 1.0;
            f.pressure = if m.buttons != 0 { 0.5 } else { 0.0 };
            if let Some(pen) = m.pen {
                f.pointer_id = PEN_POINTER_ID;
                f.pointer_type = "pen".into();
                f.pressure = if m.buttons != 0 { pen.pressure } else { 0.0 };
                f.tangential_pressure = pen.tangential_pressure;
                f.tilt_x = pen.tilt_x;
                f.tilt_y = pen.tilt_y;
                f.twist = pen.twist;
                let (altitude, azimuth) = crate::event::tilt_to_spherical(pen.tilt_x, pen.tilt_y);
                (f.altitude_angle, f.azimuth_angle) = (Some(altitude), azimuth);
            }
        }
        if iface == EventInterface::WheelEvent {
            f.delta_x = m.delta_x;
            f.delta_y = m.delta_y;
        }
    }
    ev
}

/// Fires the trusted mouse or pointer event `ty` at `target`; `false` when
/// it was canceled. An event nobody listens for (`crate::target::unheard`)
/// is not made: its dispatch would run nothing, and the default actions
/// around it are the caller's either way.
fn fire_mouse(
    page: &Rc<PageDom>,
    target: TargetKey,
    iface: EventInterface,
    ty: &str,
    m: &MouseInput,
    cancelable: bool,
    hit: Option<pyro_dom::geometry::GeneratedHit>,
) -> bool {
    if crate::target::unheard_input(page, ty, target, cancelable, None) { return true; }
    // A trusted mousedown, mouseup or dblclick is not dispatched at a disabled form control; at a
    // child of one it stops below the control.
    let mut stop = None;
    if iface == EventInterface::MouseEvent && matches!(ty, "mousedown" | "mouseup" | "dblclick") {
        let node = page.dom.borrow().key_node(target);
        if let Some(node) = node {
            if let Some(control) = disabled_control_of(page, node) {
                if control == node {
                    return true;
                }
                stop = key_of(page, control);
            }
        }
    }
    let ev = mouse_event(page, iface, ty, m, cancelable);
    ev.fields.borrow_mut().pseudo_hit = hit.map(|hit| (target, hit));
    crate::target::fire_with(page, target, stop, &ev, &mut |_| {})
}

/// The trusted `pointermove` of a mouse. `button` is `None` for a plain move (`button` -1) and
/// the changed button for a chorded press or release (Pointer Events: another button going down
/// or up while one stays down is a `pointermove` carrying that button, not a down or up).
fn fire_pointer_move(
    page: &Rc<PageDom>,
    target: TargetKey,
    m: &MouseInput,
    button: Option<i16>,
    hit: Option<pyro_dom::geometry::GeneratedHit>,
) {
    // `pointerrawupdate` (secure contexts only) comes first, unfiltered and not cancelable, and
    // is not held back by what the pointermove's listeners do.
    for (ty, cancelable) in [("pointerrawupdate", false), ("pointermove", true)] {
        if crate::target::unheard(page, ty) || (ty == "pointerrawupdate" && !crate::secure_context::secure_page(page)) {
            continue;
        }
        let ev = mouse_event(page, EventInterface::PointerEvent, ty, m, cancelable);
        {
            let mut f = ev.fields.borrow_mut();
            if let Some(b) = button {
                f.button = b;
            }
            f.pseudo_hit = hit.map(|hit| (target, hit));
        }
        fire(page, target, &ev);
    }
}

/// The disabled form control `node` is, or is inside (a button, input, select or textarea, or a
/// form-associated custom element; not a fieldset, option or optgroup), the nearest one: Chromium
/// dispatches no trusted `mousedown`, `mouseup`, `click`, `auxclick` or `dblclick` at the control,
/// and for a child of it the event stops below the control (whatwg/html#5886), while the pointer
/// events and `mousemove` still fire (WPT html/semantics/disabled-elements).
fn disabled_control_of(page: &PageDom, node: NodeId) -> Option<NodeId> {
    let dom = page.dom.borrow();
    let mut cur = Some(node);
    while let Some(n) = cur {
        let control = dom
            .element(n)
            .is_ok_and(|e| !(e.is_html() && matches!(&*e.name.local, "fieldset" | "option" | "optgroup")));
        if control && dom.is_disabled(n).unwrap_or(false) {
            return Some(n);
        }
        cur = flat_parent_element(&dom, n);
    }
    None
}

/// The hit-test target at a viewport point. With `root_fallback`, empty
/// viewport space targets the root element, as Chrome hit-tests it; the
/// synthetic layout leaves such gaps (e.g. at Playwright's initial mouse
/// position, 0,0).
fn pointer_target(page: &PageDom, x: f64, y: f64, root_fallback: bool) -> Option<NodeId> {
    let hit = crate::layout_api::hit_test(page, x, y);
    if hit.is_some() || !root_fallback {
        return hit;
    }
    let vp = page.viewport();
    if (0.0..vp.width as f64).contains(&x) && (0.0..vp.height as f64).contains(&y) {
        let doc = page.document();
        return page.dom.borrow().document_element(doc).ok().flatten();
    }
    None
}

/// Where the pointer last moved to (viewport coordinates), if it has.
pub fn pointer_position(page: &PageDom) -> Option<(f64, f64)> {
    page.pointer.borrow().as_ref().map(|m| (m.x, m.y))
}

/// The buttons held as of the last mouse input (`MouseEvent.buttons`), 0
/// before the pointer has moved.
pub fn pointer_buttons(page: &PageDom) -> u16 {
    page.pointer.borrow().as_ref().map_or(0, |m| m.buttons)
}

/// Records a press or release's `buttons` on the tracked pointer.
fn note_buttons(page: &PageDom, m: &MouseInput) {
    if let Some(p) = page.pointer.borrow_mut().as_mut() {
        p.buttons = m.buttons;
    }
}

/// Inclusive ancestors of `node` up to and including its document, innermost
/// first; empty when it is no longer in the page's document.
///
/// Along the flat tree (a shadow tree's elements go up through their host, a slotted one through
/// its slot), as Chrome's boundary events do: a shadow root receives none.
fn hover_chain(page: &PageDom, node: NodeId) -> Vec<NodeId> {
    let dom = page.dom.borrow();
    let mut chain = vec![node];
    let mut cur = node;
    while let Some(p) = flat_parent_element(&dom, cur) {
        chain.push(p);
        cur = p;
    }
    // The root element's parent is the document.
    if let Some(doc) = dom.parent(cur).ok().flatten() {
        chain.push(doc);
    }
    if chain.last() != Some(&page.document()) {
        chain.clear();
    }
    chain
}

/// What the page knows of the hovered node's ancestry, for boundary events after the node left
/// the document (Chrome's BoundaryEventDispatcher keeps the last known ancestor chain).
#[derive(Default)]
pub(crate) struct HoverTrail {
    /// The hovered node's ancestors (inclusive, innermost first, ending at the document) as they
    /// were when it became hovered; after a removal, only those that survived it.
    chain: Vec<NodeId>,
    /// `Dom::removal_count` when the trail was last brought up to date.
    mark: u64,
    /// The hovered node was removed from the document since it became hovered (also when it was
    /// inserted again, e.g. moved): its pointer is no longer over it until a boundary update.
    removed: bool,
}

/// Brings the hover trail up to date with the removals since it was last looked at. Whether the
/// hovered node was removed since it became hovered.
fn refresh_hover_trail(page: &PageDom) -> bool {
    let Some(prev) = page.hovered_raw() else { return false };
    let dom = page.dom.borrow();
    let mut trail = page.hover_trail.borrow_mut();
    let now = dom.removal_count();
    if now == trail.mark {
        return trail.removed;
    }
    let since = std::mem::replace(&mut trail.mark, now);
    let connected = |n: NodeId| dom.is_connected(n).unwrap_or(false);
    if trail.removed {
        trail.chain.retain(|&n| connected(n));
        return true;
    }
    let removed = dom.nodes_removed_since(since);
    let gone = !connected(prev) || removed.as_ref().is_some_and(|r| r.contains(&prev));
    if gone {
        trail.removed = true;
        match removed {
            Some(r) => trail.chain.retain(|n| connected(*n) && !r.contains(n)),
            None => trail.chain.retain(|&n| connected(n) && n != prev),
        }
    }
    gone
}

/// The target of a compatibility mouse event (or the next pointer event) whose preceding target
/// `target` left the document in a listener: the nearest ancestor still connected, as Chrome
/// retargets it. Only for a target that is the hovered one (its trail is known).
fn live_target(page: &Rc<PageDom>, target: NodeId, tkey: TargetKey) -> (NodeId, TargetKey) {
    if page.dom.borrow().is_connected(target).unwrap_or(false) || page.hovered_raw() != Some(target) {
        return (target, tkey);
    }
    refresh_hover_trail(page);
    let dom = page.dom.borrow();
    let trail = page.hover_trail.borrow();
    for &n in trail.chain.iter() {
        if dom.is_connected(n).unwrap_or(false) && dom.element(n).is_ok() {
            if let Some(k) = dom.node_key(n) {
                return (n, k);
            }
        }
    }
    (target, tkey)
}

/// Nodes that receive leave (innermost first) and enter (innermost first)
/// when the hovered node changes from the one whose ancestor chain is `left` to the one whose
/// chain is `entered`: each chain up to, excluding, their common ancestor.
fn boundary_chains(mut left: Vec<NodeId>, mut entered: Vec<NodeId>) -> (Vec<NodeId>, Vec<NodeId>) {
    while let (Some(a), Some(b)) = (left.last(), entered.last()) {
        if a != b {
            break;
        }
        left.pop();
        entered.pop();
    }
    (left, entered)
}

/// Chrome's boundary events from `prev` to `target`: out, leave (inner to
/// outer), over, enter (outer to inner), for pointer then mouse events. Out
/// and leave carry `target` as `relatedTarget`, over and enter carry `prev`.
/// A `prev` that is no longer connected gets no out or leave events
/// (BoundaryEventDispatcher::SendBoundaryEvents).
/// Compatibility mouse events of a touch tap (`touch`: its pointer id) fire
/// no mouse-type pointer events.
fn dispatch_boundary_events(
    page: &Rc<PageDom>,
    prev: Option<NodeId>,
    target: NodeId,
    tkey: TargetKey,
    m: &MouseInput,
    touch: Option<i32>,
) {
    // A `prev` removed from the document since it was hovered (also one inserted again) gets no
    // out or leave, and the ancestors that survived the removal count as already entered.
    let prev_removed = refresh_hover_trail(page);
    let prev_connected = !prev_removed && prev.is_some_and(|p| page.dom.borrow().is_connected(p).unwrap_or(false));
    let prev_chain = match prev {
        Some(_) if prev_removed => page.hover_trail.borrow().chain.clone(),
        Some(p) => hover_chain(page, p),
        None => Vec::new(),
    };
    let target_chain = hover_chain(page, target);
    *page.hover_trail.borrow_mut() = HoverTrail {
        chain: target_chain.clone(),
        mark: page.dom.borrow().removal_count(),
        removed: false,
    };
    let (left, entered) = boundary_chains(prev_chain, target_chain);
    let connected = |n: NodeId| page.dom.borrow().is_connected(n).unwrap_or(false);
    let prev_key = prev.and_then(|p| key_of(page, p));
    // The new `:hover` state comes first: the boundary events observe it, as in Chrome (WPT
    // css/selectors `hover-boundary-events`: `#a` no longer matches during its `mouseout`).
    let doc = page.document();
    let _ = page.dom.borrow_mut().set_hovered(doc, Some(target));
    page.set_hovered(Some(target));
    // pyro: speculation rules' hover eagerness (a hook that only posts a task).
    if let Some(hooks) = page.hooks().filter(|hooks| hooks.wants_link_hover()) {
        hooks.link_hover(closest_link(page, target));
    }
    // An event nobody listens for is not made (see `fire_mouse`).
    let boundary = |iface: EventInterface, ty: &str, at: TargetKey, cancelable: bool, related: Option<TargetKey>| {
        if crate::target::unheard_input(page, ty, at, cancelable, None) {
            return;
        }
        let ev = mouse_event(page, iface, ty, m, cancelable);
        ev.fields.borrow_mut().related_target = related;
        fire(page, at, &ev);
    };
    for (iface, [out, leave, over, enter]) in [
        (EventInterface::PointerEvent, ["pointerout", "pointerleave", "pointerover", "pointerenter"]),
        (EventInterface::MouseEvent, ["mouseout", "mouseleave", "mouseover", "mouseenter"]),
    ] {
        if touch.is_some() && iface == EventInterface::PointerEvent {
            continue;
        }
        if let Some(p) = prev_key.filter(|_| prev_connected) {
            boundary(iface, out, p, true, Some(tkey));
        }
        for &n in left.iter().filter(|&&n| connected(n)) {
            if let Some(k) = key_of(page, n) {
                boundary(iface, leave, k, false, Some(tkey));
            }
        }
        boundary(iface, over, tkey, true, prev_key);
        // A node a listener removed in the meantime hears no enter.
        for &n in entered.iter().rev().filter(|&&n| connected(n)) {
            if let Some(k) = key_of(page, n) {
                boundary(iface, enter, k, false, prev_key);
            }
        }
    }
}

/// The pointer is now over `target`, the node that captured it: boundary events from what was
/// hovered (pointer capture §4.1.1; no-op when it already is).
pub(crate) fn hover_capture_target(page: &Rc<PageDom>, target: NodeId, m: &MouseInput) {
    let prev = page.hovered();
    if prev == Some(target) && !refresh_hover_trail(page) {
        return;
    }
    if let Some(tkey) = key_of(page, target) {
        dispatch_boundary_events(page, prev, target, tkey, m, None);
    }
}

/// The pointer lost its capture: it is over whatever is under it again.
pub(crate) fn hover_under_pointer(page: &Rc<PageDom>, m: &MouseInput) {
    let Some(hit) = pointer_target(page, m.x, m.y, true) else { return };
    hover_capture_target(page, hit, m);
}

/// Delay of the hover update after the hovered element is removed: Chromium
/// updates hover for content that changed under a stationary pointer on its
/// fake-mouse-move timer (~100 ms, measured 110 ms), not in the mutating task.
/// Frameworks rely on that: React detaches a removed node's fiber in passive
/// effects after the commit, and only then treats an over event whose
/// relatedTarget is that node as an enter (the @povio/ui QueryAutocomplete
/// static-to-live handoff in TOS needs this onMouseEnter).
const HOVER_UPDATE_DELAY: std::time::Duration = std::time::Duration::from_millis(100);

/// After a task changes the document, re-hit-test the stationary pointer.
/// An overlay can cover a still-connected trigger just as removal can expose
/// another element. Boundary events run after passive effects, without a move.
pub(crate) fn update_hover_after_task(page: &Rc<PageDom>) {
    if page.hover_update_queued.get() || page.hovered_raw().is_none() || page.pointer.borrow().is_none() {
        return;
    }
    let Some(hooks) = page.hooks() else { return };
    page.hover_update_queued.set(true);
    let weak = Rc::downgrade(page);
    hooks.post_delayed_task(
        HOVER_UPDATE_DELAY,
        Box::new(move || {
            if let Some(page) = weak.upgrade() {
                page.hover_update_queued.set(false);
                update_hover_now(&page);
            }
        }),
    );
}

fn update_hover_now(page: &Rc<PageDom>) {
    // Pointer capture owns the target until release, even if layout changes.
    let capture = page.mouse_capture.get();
    if capture.current.is_some() || capture.pending.is_some() {
        return;
    }
    let Some(prev) = page.hovered_raw() else { return };
    let Some(m) = page.pointer.borrow().clone() else { return };
    let Some(target) = pointer_target(page, m.x, m.y, true) else { return };
    if target == prev && !refresh_hover_trail(page) {
        return;
    }
    let Some(tkey) = key_of(page, target) else { return };
    dispatch_boundary_events(page, Some(prev), target, tkey, &m, None);
}

/// CDP `Input.dispatchMouseEvent`. Returns an error when hit testing is not
/// possible (no element at the point).
pub fn dispatch_mouse(page: &Rc<PageDom>, kind: MouseKind, m: &MouseInput) -> Result<(), String> {
    dispatch_mouse_intercepting_drags(page, kind, m, false).map(|_| ())
}

/// `dispatch_mouse` under CDP `Input.setInterceptDrags`. With
/// `intercept_drags`, a left-button move past the drag threshold from a
/// draggable source starts a native drag: `dragstart` fires on the source
/// and, unless canceled, the drag data is returned for
/// `Input.dragIntercepted`; `dispatch_drag` then drives the drag. Without
/// interception no native drag starts (there is no OS drag loop).
/// Elements a press in which does not select document text: form controls
/// (with their own selection) and editable content.
fn press_selects_text(dom: &Dom, target: NodeId) -> bool {
    if crate::editing::editing_host(dom, target).is_some() {
        return false;
    }
    let mut cur = Some(target);
    while let Some(e) = cur {
        if dom.element(e).is_ok_and(|el| {
            el.is_html()
                && matches!(&*el.name.local, "input" | "textarea" | "select" | "button" | "option" | "optgroup")
        }) {
            return false;
        }
        cur = dom.parent_element(e).ok().flatten();
    }
    true
}

/// The anchor of a native text selection drag: the caret at the press, its
/// node's parent and index there (a removed anchor continues from that
/// position, as a live range would), and whether `selectstart` let the
/// selection start (a canceled one is fired again on each move).
#[derive(Copy, Clone, Debug)]
pub(crate) struct TextDrag {
    anchor: (NodeId, u32),
    parent: Option<NodeId>,
    index: u32,
    started: bool,
}

/// The caret under the pointer within `target` (the element there), and the
/// node `selectstart` targets: the text when the point is on it, else the
/// element. An element without visible text under it is its own caret
/// position (offset 0), as in Chromium.
fn caret_in(page: &Rc<PageDom>, target: NodeId, m: &MouseInput, selectable: bool) -> ((NodeId, u32), NodeId) {
    let dom = page.dom.borrow();
    let layout = page.layout();
    let filter = |n: NodeId| !selectable || !layout.user_select_none(&dom, n);
    match layout.caret_from_point(&dom, target, m.x, m.y, &filter) {
        Some((node, offset, on_text)) => ((node, offset), if on_text { node } else { target }),
        None => ((target, 0), target),
    }
}

/// Native text selection, as Chromium's mouse selection: a primary press
/// (not canceled, outside controls, editable content and drag sources) on
/// selectable content fires `selectstart` at the node under the pointer
/// and, unless canceled (or the handler removed the position), collapses
/// the selection to the caret there. A press beside the text of a
/// focusable element only focuses it.
fn text_select_press(page: &Rc<PageDom>, target: NodeId, m: &MouseInput) {
    {
        let dom = page.dom.borrow();
        if !press_selects_text(&dom, target) || page.layout().user_select_none(&dom, target) {
            return;
        }
    }
    let (caret, event_target) = caret_in(page, target, m, true);
    if event_target != caret.0 && caret.0 != target && page.dom.borrow().is_focusable(target).unwrap_or(false) {
        return;
    }
    let started = fire_simple(page, event_target, "selectstart", true, true);
    let (parent, index) = {
        let dom = page.dom.borrow();
        if !dom.is_connected(caret.0).unwrap_or(false) {
            return;
        }
        let parent = dom.parent_element(caret.0).ok().flatten();
        let index = parent
            .and_then(|p| dom.children(p).ok().and_then(|mut c| c.position(|n| n == caret.0)))
            .unwrap_or(0) as u32;
        (parent, index)
    };
    if started {
        crate::editing::set_selection(page, caret, caret);
    }
    page.text_drag.set(Some(TextDrag { anchor: caret, parent, index, started }));
}

/// The drag's anchor now: the press caret, or where its removed node was
/// (the text node now at that position, else the parent there).
fn live_anchor(page: &Rc<PageDom>, drag: &TextDrag) -> Option<(NodeId, u32)> {
    let dom = page.dom.borrow();
    if dom.is_connected(drag.anchor.0).unwrap_or(false) {
        return Some(drag.anchor);
    }
    let parent = drag.parent.filter(|&p| dom.is_connected(p).unwrap_or(false))?;
    let children: Vec<NodeId> = dom.children(parent).ok()?.collect();
    match children.get(drag.index as usize) {
        Some(&c) if dom.node_type(c).ok() == Some(pyro_dom::NodeType::Text) => Some((c, 0)),
        _ => Some((parent, (drag.index as usize).min(children.len()) as u32)),
    }
}

/// A move with the primary button down extends the selection to the caret
/// under the pointer; over `user-select: none` content it stops at the
/// edge of the selectable text before (or after) it.
fn text_select_move(page: &Rc<PageDom>, target: NodeId, m: &MouseInput) {
    let Some(mut drag) = page.text_drag.get() else { return };
    let Some(anchor) = live_anchor(page, &drag) else { return };
    if !drag.started {
        if !fire_simple(page, anchor.0, "selectstart", true, true) {
            return;
        }
        drag.started = true;
        page.text_drag.set(Some(drag));
    }
    let (hit, _) = caret_in(page, target, m, false);
    let focus = {
        let dom = page.dom.borrow();
        let layout = page.layout();
        if !layout.user_select_none(&dom, hit.0) {
            Some(hit)
        } else {
            let after = crate::editing::compare(page, (hit.0, 0), anchor) > 0;
            let result = layout.text_result(&dom);
            let runs = result
                .text_runs
                .iter()
                .filter(|r| dom.is_connected(r.node).unwrap_or(false) && !layout.user_select_none(&dom, r.node));
            if after {
                runs.filter(|r| crate::editing::compare(page, (r.node, r.end), (hit.0, 0)) < 0)
                    .map(|r| (r.node, r.end))
                    .max_by(|a, b| crate::editing::compare(page, *a, *b).cmp(&0))
            } else {
                runs.filter(|r| crate::editing::compare(page, (r.node, r.start), (hit.0, 0)) > 0)
                    .map(|r| (r.node, r.start))
                    .min_by(|a, b| crate::editing::compare(page, *a, *b).cmp(&0))
            }
        }
    };
    let live = |n: NodeId| page.dom.borrow().is_connected(n).unwrap_or(false);
    if let Some(f) = focus.filter(|f| live(f.0) && live(anchor.0)) {
        crate::editing::set_selection(page, anchor, f);
    }
}

pub fn dispatch_mouse_intercepting_drags(
    page: &Rc<PageDom>,
    kind: MouseKind,
    m: &MouseInput,
    intercept_drags: bool,
) -> Result<Option<DragData>, String> {
    // HTML activation-triggering input events: for the mouse, mousedown (pointerdown) only; pointerup
    // triggers activation for other pointer types, and this is the mouse. A release grants none, so one
    // click whose mousedown handler consumed the activation leaves none for its click handler.
    if kind == MouseKind::Pressed {
        page.notify_input_activation();
    }
    dispatch_mouse_inner(page, kind, m, intercept_drags, None)
}

/// Mouse input; `touch` (the touch's pointer id) makes it the compatibility
/// mouse events of a tap: no mouse-type pointer events, and a `click` whose
/// pointerType is "touch", as Chrome's gesture tap.
fn dispatch_mouse_inner(
    page: &Rc<PageDom>,
    kind: MouseKind,
    m: &MouseInput,
    intercept_drags: bool,
    touch: Option<i32>,
) -> Result<Option<DragData>, String> {
    // Empty viewport space targets the root element, as Chromium hit-tests it.
    let hit = pointer_target(page, m.x, m.y, true).ok_or_else(|| "no element at point".to_owned())?;
    // The pointer id `setPointerCapture` accepts for this mouse-like pointer: the mouse's or the pen's.
    if touch.is_none() {
        let id = if m.pen.is_some() { PEN_POINTER_ID } else { 1 };
        let mut c = page.mouse_capture.get();
        if c.pointer_id != id {
            c.pointer_id = id;
            page.mouse_capture.set(c);
        }
    }
    // Mouse pointer events (and their compatibility mouse events) go to the
    // capture target while there is one.
    let captured = match (touch, kind) {
        (None, MouseKind::Pressed | MouseKind::Moved | MouseKind::Released) => {
            crate::pointer_capture::capture_target(page, m)
        }
        _ => None,
    };
    let target = captured.unwrap_or(hit);
    // pyro: and, for a left release, the `::scroll-button()` of the target under the pointer (from
    // the same synced geometry: the click's default action needs no other).
    let release = matches!(kind, MouseKind::Released) && m.button == MouseButton::Left && touch.is_none();
    let (pseudo, scroll_button) = if captured.is_none() {
        let dom = page.dom.borrow();
        let key = pyro_dom::snapshot::render_key(target);
        match page.layout().synced_geometry_source(&dom) {
            Some(geometry) => (
                geometry.generated_hit(key, m.x, m.y),
                release.then(|| geometry.scroll_button_hit(key, m.x, m.y)).flatten(),
            ),
            None => (None, None),
        }
    } else { (None, None) };
    let Some(tkey) = key_of(page, target) else { return Err("stale target".into()) };
    // A press handler may open an overlay beneath a stationary pointer.
    // Reconcile boundaries before the next pointer event, including release:
    // otherwise the old trigger stays hovered and can open a stale tooltip.
    // `movementX/Y`: the screen delta from the previous mouse input of this document (0 for the
    // first one); only a move reports it, other events read 0.
    let moved_input;
    let m = if kind == MouseKind::Moved {
        let prev = page.pointer.borrow().as_ref().map(|p| (p.x, p.y));
        moved_input = MouseInput { movement: prev.map_or((0.0, 0.0), |(px, py)| (m.x - px, m.y - py)), ..m.clone() };
        &moved_input
    } else {
        m
    };
    // The hovered node was removed since it was hovered: Chrome's fake mouse move after the layout
    // update has re-targeted the pointer before the next real input arrives, so the boundary
    // events of the content now under the old position come first (the delayed task of
    // `update_hover_after_task` would run later than a prompt next input).
    if touch.is_none()
        && matches!(kind, MouseKind::Moved | MouseKind::Pressed | MouseKind::Released)
        && page.hovered_raw().is_some()
        && page.pointer.borrow().is_some()
        && refresh_hover_trail(page)
    {
        update_hover_now(page);
    }
    if matches!(kind, MouseKind::Moved | MouseKind::Pressed | MouseKind::Released) {
        *page.pointer.borrow_mut() = Some(m.clone());
        page.layout().set_pointer(Some((m.x, m.y)));
        let prev = page.hovered();
        // (A hovered node that was removed, even if inserted again, is entered anew.)
        if prev != Some(target) || refresh_hover_trail(page) {
            dispatch_boundary_events(page, prev, target, tkey, m, touch);
        }
    }
    // A boundary listener may have removed the target (WPT mousemove_after_mouseover_target_removed).
    let (target, tkey) = live_target(page, target, tkey);
    match kind {
        MouseKind::Moved => {
            if touch.is_none() {
                fire_pointer_move(page, tkey, m, None, pseudo);
            }
            // A listener may have removed the target: the compatibility event goes to its
            // nearest ancestor still in the document, as in Chrome.
            let (target, tkey) = live_target(page, target, tkey);
            if !(page.compat_mouse_suppressed.get() && m.buttons != 0) {
                fire_mouse(page, tkey, EventInterface::MouseEvent, "mousemove", m, true, pseudo);
            }
            if m.buttons & 1 != 0 {
                text_select_move(page, target, m);
                range_drag_move(page, m);
            }
            if intercept_drags {
                return Ok(start_drag(page, target, m));
            }
        }
        MouseKind::Pressed => {
            // A right press (the context menu) light-dismisses nothing, as in Chromium: a
            // popover its `contextmenu` handler shows stays open.
            if m.button != MouseButton::Right {
                crate::popover::light_dismiss(page, target, m.x, m.y, true);
            }
            // Another button pressed while one is already down (a chord) is a `pointermove`
            // carrying that button, not a `pointerdown`; the compatibility `mousedown` still fires
            // (Pointer Events "chorded button interactions").
            let chorded = touch.is_none() && m.buttons & !m.button.mask() != 0;
            if touch.is_none() && !chorded {
                let mut c = page.mouse_capture.get();
                c.buttons_down = true;
                page.mouse_capture.set(c);
            }
            // Any button's press activates `:active`, as in Chrome (WPT css/selectors
            // `active-observable`: right and middle presses too).
            set_active(page, page.document(), Some(target));
            note_link_pointer_down(page, target);
            let pd = if chorded {
                fire_pointer_move(page, tkey, m, Some(m.button.dom_button()), pseudo);
                // A pointermove cannot suppress the compatibility mouse events; a canceled
                // pointerdown of the first button keeps suppressing them for the whole chord.
                !page.compat_mouse_suppressed.get()
            } else {
                touch.is_some() || fire_mouse(page, tkey, EventInterface::PointerEvent, "pointerdown", m, true, pseudo)
            };
            let (target, tkey) = live_target(page, target, tkey);
            let md = !pd
                || fire_mouse(page, tkey, EventInterface::MouseEvent, "mousedown", m, true, pseudo);
            if !chorded {
                page.compat_mouse_suppressed.set(!pd);
            }
            if chorded {
                // The first button's press stays the one a click is measured from.
                note_buttons(page, m);
                if m.button == MouseButton::Right && !page.is_disposed() {
                    if let Some(k) = key_of(page, target) {
                        let ev = mouse_event(page, EventInterface::PointerEvent, "contextmenu", m, true);
                        ev.state.borrow_mut().composed = true;
                        fire(page, k, &ev);
                    }
                }
                return Ok(None);
            }
            page.set_press_target(Some(target));
            remember_press(page, target);
            page.pressed_pseudo.set(pseudo);
            note_buttons(page, m);
            let place_caret = m.button == MouseButton::Left && pd && md;
            // A canceled mousedown prevents dragging, as in Chrome.
            let source =
                (m.button == MouseButton::Left && pd && md).then(|| drag_source(&page.dom.borrow(), target)).flatten();
            page.drag_candidate.set(source.map(|s| (s, m.x, m.y)));
            // Default action of mousedown: focus the nearest focusable ancestor.
            if pd && md {
                let focus_target = {
                    let layout = page.layout();
                    let dom = page.dom.borrow();
                    let mut cur = Some(target);
                    let mut found = None;
                    while let Some(c) = cur {
                        // A host that delegates focus takes it for a press
                        // anywhere in it (its delegate, see focus()).
                        if node_focusable(&layout, &dom, c) || dom.is_delegates_focus_host(c) {
                            found = Some(c);
                            break;
                        }
                        cur = flat_parent_element(&dom, c);
                    }
                    found
                };
                match focus_target {
                    // The pointer already hit visible content. Revealing its
                    // focusable ancestor can move that content between press
                    // and release, turning the click into an ancestor click.
                    // Explicit script focus keeps its own scroll policy.
                    // Chrome's last focus type: a press that focuses an element makes it
                    // "mouse" (later script focus shows no ring); one that focuses nothing
                    // clears focus with no type, so later script focus shows the ring again
                    // (WPT css/selectors `focus-visible-script-focus-002`, `-006`, `-016`).
                    Some(f) => {
                        page.pointer_modality.set(true);
                        page.sequential_start.set(None);
                        focus_element_with_options(page, f, true)
                    }
                    None => {
                        page.pointer_modality.set(false);
                        let focused = page.dom.borrow().focused(page.document()).ok().flatten();
                        if let Some(f) = focused {
                            blur_element(page, f);
                        }
                        // HTML "sequential focus navigation starting point": a press on content
                        // that cannot take focus moves it there, so Tab goes on from the clicked
                        // node (after the blur, whose own starting point it replaces).
                        page.sequential_start.set(Some(target));
                        // The press still makes this document's frame the focused one (Chrome's
                        // `FocusController::SetFocusedFrame` on mouse down): an iframe's owner becomes
                        // the parent's activeElement and keys go to the frame (WPT css-overscroll-behavior
                        // `overscroll-behavior-keyboard-scroll-child-frame`).
                        // (Without scrolling the owner into view: a press reveals nothing.)
                        if !page.is_disposed() {
                            if let Some(token) = request_focus(page, FocusTarget::Window, true) {
                                focus_window_with_transition(page, &token);
                            }
                        }
                    }
                }
            }
            // A press on a customizable select's button opens or closes its picker; one on an
            // option of a list box picks it.
            if pd && md && m.button == MouseButton::Left && !page.is_disposed() {
                crate::popover::select_picker_mousedown(page, target);
                listbox_option_mousedown(page, target);
            }
            // Chromium on Linux and macOS opens the context menu on the
            // press, whether or not the page canceled it (Windows waits for
            // the release); the release still fires `mouseup` and `auxclick`.
            if m.button == MouseButton::Right && !page.is_disposed() {
                if let Some(k) = key_of(page, target) {
                    let ev = mouse_event(page, EventInterface::PointerEvent, "contextmenu", m, true);
                    ev.state.borrow_mut().composed = true;
                    fire(page, k, &ev);
                }
            }
            if place_caret {
                crate::editing::caret_on_press(page, target);
                range_press(page, target, m);
                number_spin_press(page, target, m);
            }
            page.text_drag.set(None);
            if place_caret && source.is_none() {
                text_select_press(page, target, m);
            }
            // A double press selects a word, a triple one a line or block.
            if place_caret && source.is_none() && m.click_count >= 2 && !page.is_disposed() {
                crate::edit_commands::multi_click(page, target, m.x, m.y, m.click_count);
            }
        }
        MouseKind::Released => {
            note_buttons(page, m);
            if m.button == MouseButton::Left {
                range_release(page);
            }
            page.drag_candidate.set(None);
            page.text_drag.set(None);
            // A release while another button stays down is a `pointermove` carrying the released
            // button: no `pointerup`, the pointer capture stays, and the click goes with the last
            // release.
            let chorded = touch.is_none() && m.buttons & !m.button.mask() != 0;
            if !chorded {
                set_active(page, page.document(), None);
            }
            if chorded {
                fire_pointer_move(page, tkey, m, Some(m.button.dom_button()), pseudo);
            } else if touch.is_none() {
                fire_mouse(page, tkey, EventInterface::PointerEvent, "pointerup", m, true, pseudo);
                // Implicit release right after pointerup: lostpointercapture
                // before the compatibility mouseup and the click.
                let mut c = page.mouse_capture.get();
                c.pending = None;
                c.buttons_down = m.buttons != 0;
                page.mouse_capture.set(c);
                crate::pointer_capture::process_pending_capture(page, m);
            }
            let (target, tkey) = live_target(page, target, tkey);
            if m.button != MouseButton::Right {
                crate::popover::light_dismiss(page, target, m.x, m.y, false);
            }
            // Only a canceled pointerdown holds back the mouseup (an app's
            // pointerup handler that calls preventDefault does not).
            // (No mouseup, no mouseup default handling: nor a pick.)
            let suppressed = if chorded { page.compat_mouse_suppressed.get() } else { page.compat_mouse_suppressed.replace(false) };
            let mouseup_ok = !suppressed && fire_mouse(page, tkey, EventInterface::MouseEvent, "mouseup", m, true, pseudo);
            // A release on an option of a customizable select's open picker picks it.
            if mouseup_ok && m.button == MouseButton::Left && !page.is_disposed() {
                select_picker_option_release(page, target);
            }
            let pressed = if chorded { None } else { page.take_press_target() };
            let pressed_pseudo = if chorded { None } else { page.pressed_pseudo.take() };
            // click targets the common ancestor of press and release targets;
            // under capture, the capture target (as in Chrome).
            let click_target = match pressed {
                Some(_) if captured.is_some() => captured,
                Some(p) if p == target => Some(target),
                Some(p) => common_ancestor(&page.dom.borrow(), p, target),
                None => None,
            };
            // No click (nor auxclick, dblclick) at a disabled form control; one inside it stops
            // below the control (`dispatch_click`, `fire_mouse`).
            let click_target = click_target.filter(|&ct| disabled_control_of(page, ct) != Some(ct));
            if let Some(ct) = click_target {
                let name = match m.button {
                    MouseButton::Left | MouseButton::None => "click",
                    _ => "auxclick",
                };
                let ev = mouse_event(page, EventInterface::PointerEvent, name, m, true);
                let click_pseudo = if pressed == Some(ct) && ct == target {
                    pressed_pseudo.and_then(|p| p.common(pseudo?))
                } else { None };
                ev.fields.borrow_mut().pseudo_hit = click_pseudo.map(|p| (tkey, p));
                ev.state.borrow_mut().composed = true;
                if let Some(id) = touch {
                    let mut f = ev.fields.borrow_mut();
                    f.pointer_id = id;
                    f.pointer_type = "touch".into();
                    f.is_primary = false;
                }
                // pyro: a click on an enabled `::scroll-button()` (hit on release, on the element
                // the click targets) scrolls that element by a page in the button's direction,
                // unless canceled, instead of any other activation behavior (CSS Overflow 5 § 2.2).
                // A disabled one does nothing: the click activates what it would without it.
                let button = scroll_button
                    .filter(|&(_, enabled)| enabled && name == "click" && ct == target)
                    .map(|(delta, _)| (target, delta));
                let proceed = dispatch_click(page, ct, &ev, button.is_some());
                // pyro: an uncanceled click on a `::scroll-marker` scrolls to its target.
                if proceed && name == "click" && !page.is_disposed() {
                    let moved = match button {
                        Some((origin, delta)) => {
                            page.layout.borrow_mut().activate_scroll_button(&page.dom.borrow(), origin, delta)
                        }
                        None => page.layout.borrow_mut().activate_scroll_marker_at(&page.dom.borrow(), (m.x, m.y)),
                    };
                    for scrolled in moved {
                        let node = match scrolled {
                            pyro_dom::layout::ScrollTarget::Viewport => None,
                            pyro_dom::layout::ScrollTarget::Container(node) => Some(node),
                        };
                        crate::layout_api::fire_scroll(page, node);
                    }
                }
                if name == "click" && m.click_count == 2 {
                    if let Some(k) = key_of(page, ct) {
                        fire_mouse(page, k, EventInterface::MouseEvent, "dblclick", m, true, click_pseudo);
                    }
                }
            }
            // Releasing capture can expose another hit target without a DOM
            // mutation, so the ordinary after-task dirty check may skip it.
            if captured.is_some() {
                update_hover_after_task(page);
            }
        }
        MouseKind::Wheel => {
            // Default action of an uncanceled wheel (frame/user_input.zig:
            // 571-615): each axis scrolls the nearest container that can still
            // move, else the viewport, then `scroll` fires as a task.
            let doc = page.document();
            if !fire_mouse(page, tkey, EventInterface::WheelEvent, "wheel", m, true, pseudo)
                || page.is_disposed()
                || page.document() != doc
            {
                return Ok(None);
            }
            let moved =
                page.layout.borrow_mut().wheel_scroll_at(&page.dom.borrow(), target, (m.x, m.y), m.delta_x, m.delta_y);
            for scrolled in moved {
                let node = match scrolled {
                    pyro_dom::layout::ScrollTarget::Viewport => None,
                    pyro_dom::layout::ScrollTarget::Container(node) => Some(node),
                };
                crate::layout_api::fire_scroll(page, node);
            }
        }
    }
    Ok(None)
}

// ----- touch ---------------------------------------------------------------------
//
// CDP `Input.dispatchTouchEvent`, checked against headless Chrome 156 driven
// by Playwright 1.59.1 (`hasTouch` + `tap()`, and raw touch sequences): per
// changed touch, pointer events of pointerType "touch" (implicit capture
// included), then trusted TouchEvents; a tap then fires the compatibility
// mouse events and a click.

/// Chrome's touch slop (`max_touch_move_in_pixels_for_click`), CSS px: a
/// single touch that stays within it is a tap and gets no touchmove; one that
/// leaves it starts a scroll unless the touchmove is canceled.
const TOUCH_SLOP: f64 = 15.0;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TouchKind {
    Start,
    Move,
    End,
    Cancel,
}

/// One CDP touch point, in the document's viewport coordinates.
#[derive(Clone, Debug)]
pub struct TouchInput {
    pub id: i64,
    pub x: f64,
    pub y: f64,
    pub radius_x: f64,
    pub radius_y: f64,
    pub rotation_angle: f64,
    pub force: f64,
}

struct ActiveTouch {
    input: TouchInput,
    pointer_id: i32,
    primary: bool,
    /// The touchstart target: the touch's events go there for the whole
    /// sequence (implicit pointer capture).
    target: NodeId,
    /// The node the touch is "over" for boundary events: the touchstart target until pointer
    /// capture moves it to the capturing node (and back when the capture is lost).
    over: std::cell::Cell<NodeId>,
    /// A scroll took the touch over: no more pointer events for it.
    pointer_cancelled: bool,
}

/// A page's touch input state across `dispatch_touch` calls: the touches
/// down, and whether the sequence (first press to last release) is a tap.
#[derive(Default)]
pub struct TouchSequence {
    doc: Option<NodeId>,
    active: Vec<ActiveTouch>,
    /// More than one touch was down at once.
    multi: bool,
    start: (f64, f64),
    left_slop: bool,
    /// A scroll took over: later touch events are not cancelable.
    scrolling: bool,
    /// touchstart or touchend was canceled: no tap.
    no_tap: bool,
    /// pointerdown was canceled: the tap fires no compatibility mouse events.
    mouse_suppressed: bool,
    next_pointer_id: i32,
    /// The first touch's `touch-action` pan restrictions (x, y blocked), read when it came down:
    /// later changes of the style do not matter (WPT pointerevents
    /// `pointerevent_touch-action-modified_touch`).
    pan_blocked: (bool, bool),
    /// The pan that scrolls for the sequence's single touch, once it began.
    pan: Option<TouchPan>,
}

/// A touch pan (Chrome's gesture scroll): the finger's last position, the scroller each axis
/// latched to, the scrollers that moved, and the recent finger positions a fling starts from.
#[derive(Default)]
struct TouchPan {
    last: (f64, f64),
    /// Per axis (x, y): `Some(Some(box))` latched to a box that took the scroll, `Some(None)` to
    /// a chain end that contains the overscroll; `None`: nothing took it yet, so it still chains.
    latch: [Option<Option<pyro_dom::layout::ScrollTarget>>; 2],
    scrolled: Vec<pyro_dom::layout::ScrollTarget>,
    samples: Vec<(std::time::Instant, f64, f64)>,
}

impl TouchSequence {
    /// Whether any touch is down (a new sequence starts with the next press).
    pub fn is_active(&self) -> bool {
        !self.active.is_empty()
    }

    /// The ids of the touches down.
    pub fn ids(&self) -> Vec<i64> {
        self.active.iter().map(|t| t.input.id).collect()
    }

    pub fn has(&self, id: i64) -> bool {
        self.active.iter().any(|t| t.input.id == id)
    }

    /// Forgets the touches down (their document went away).
    pub fn clear(&mut self) {
        self.reset();
    }

    fn reset(&mut self) {
        let next = self.next_pointer_id;
        *self = TouchSequence { next_pointer_id: next, ..Default::default() };
    }
}

/// Pointer fields of one touch pointer event.
struct PointerState {
    x: f64,
    y: f64,
    button: i16,
    buttons: u16,
    width: f64,
    height: f64,
    pressure: f64,
    /// `movementX/Y` of a pointermove: the move since the touch's previous position.
    movement: (f64, f64),
}

impl PointerState {
    /// Down and enter: the touch's contact geometry.
    fn pressed(t: &TouchInput) -> Self {
        PointerState {
            x: t.x,
            y: t.y,
            button: 0,
            buttons: 1,
            width: 2.0 * t.radius_x,
            height: 2.0 * t.radius_y,
            pressure: t.force,
            movement: (0.0, 0.0),
        }
    }
    fn moved(t: &TouchInput, movement: (f64, f64)) -> Self {
        PointerState { button: -1, movement, ..Self::pressed(t) }
    }
    /// Up and what follows it: Chrome reports a 1x1 contact without pressure.
    fn released(t: &TouchInput) -> Self {
        PointerState { x: t.x, y: t.y, button: 0, buttons: 0, width: 1.0, height: 1.0, pressure: 0.0, movement: (0.0, 0.0) }
    }
    /// touchCancel's pointercancel keeps the contact size.
    fn cancelled(t: &TouchInput) -> Self {
        PointerState { button: -1, buttons: 0, pressure: 0.0, ..Self::pressed(t) }
    }
    /// A scroll's pointercancel carries no position (Chrome: 0,0, 1x1).
    fn scrolled() -> Self {
        PointerState { x: 0.0, y: 0.0, button: 0, buttons: 0, width: 1.0, height: 1.0, pressure: 0.0, movement: (0.0, 0.0) }
    }
}

fn touch_pointer_event(
    page: &Rc<PageDom>,
    ty: &str,
    t: &ActiveTouch,
    p: &PointerState,
    modifiers: Modifiers,
    related: Option<TargetKey>,
) -> EventData {
    let boundary = matches!(ty, "pointerenter" | "pointerleave");
    let cancelable = !boundary && !matches!(ty, "pointercancel" | "pointerrawupdate" | "gotpointercapture" | "lostpointercapture");
    let ev = EventCore::trusted(page, EventInterface::PointerEvent, ty, !boundary, cancelable);
    {
        let mut f = ev.fields.borrow_mut();
        f.client_x = p.x;
        f.client_y = p.y;
        f.screen_x = p.x;
        f.screen_y = p.y;
        f.button = p.button;
        f.buttons = p.buttons;
        f.ctrl_key = modifiers.ctrl;
        f.shift_key = modifiers.shift;
        f.alt_key = modifiers.alt;
        f.meta_key = modifiers.meta;
        f.pointer_id = t.pointer_id;
        f.pointer_type = "touch".into();
        f.is_primary = t.primary;
        f.width = p.width;
        f.height = p.height;
        f.pressure = p.pressure;
        if matches!(ty, "pointermove" | "pointerrawupdate") {
            (f.movement_x, f.movement_y) = p.movement;
        }
        f.related_target = related;
    }
    ev
}

fn fire_touch_pointer(page: &Rc<PageDom>, ty: &str, t: &ActiveTouch, p: &PointerState, modifiers: Modifiers) -> bool {
    fire_touch_pointer_at(page, t.target, ty, t, p, modifiers)
}

fn fire_touch_pointer_at(page: &Rc<PageDom>, node: NodeId, ty: &str, t: &ActiveTouch, p: &PointerState, modifiers: Modifiers) -> bool {
    match key_of(page, node) {
        Some(key) => fire(page, key, &touch_pointer_event(page, ty, t, p, modifiers, None)),
        None => true,
    }
}

/// Where the touch's pointermove, pointerup and pointercancel go: its pointer capture target
/// (the touchstart target while implicit capture is pending); without capture (script released
/// it, or the target left the document) the element under the touch, else the touchstart target.
fn fire_touch_pointer_routed(page: &Rc<PageDom>, ty: &str, t: &ActiveTouch, p: &PointerState, modifiers: Modifiers) -> bool {
    let node = match crate::pointer_capture::touch_capture(page, t.pointer_id) {
        Some(c) if c.current.is_some() => c.current,
        Some(c) if c.pending.is_some() => Some(t.target),
        _ => None,
    };
    let node = node.or_else(|| pointer_target(page, t.input.x, t.input.y, true)).unwrap_or(t.target);
    fire_touch_pointer_at(page, node, ty, t, p, modifiers)
}

/// "Process pending pointer capture" before the touch's next pointer event: lostpointercapture
/// at the old capture target, then gotpointercapture at the new one (the implicit one after
/// pointerdown, or what script set).
fn capture(page: &Rc<PageDom>, t: &ActiveTouch, p: &PointerState, modifiers: Modifiers) {
    let (lost, got) = crate::pointer_capture::touch_process_pending(page, t.pointer_id);
    if let Some(node) = lost {
        fire_touch_pointer_at(page, node, "lostpointercapture", t, p, modifiers);
        // The touch is over whatever is under it again.
        if got.is_none() {
            let under = pointer_target(page, t.input.x, t.input.y, true).unwrap_or(t.target);
            touch_hover(page, t, p, modifiers, under);
        }
    }
    if let Some(node) = got {
        // The capturing node is entered before it hears `gotpointercapture`.
        touch_hover(page, t, p, modifiers, node);
        fire_touch_pointer_at(page, node, "gotpointercapture", t, p, modifiers);
    }
}

/// Boundary events of touch `t` moving from the node it is over to `to` (out and leave inner to
/// outer, then over and enter outer to inner), as the mouse's `dispatch_boundary_events`.
fn touch_hover(page: &Rc<PageDom>, t: &ActiveTouch, p: &PointerState, modifiers: Modifiers, to: NodeId) {
    let from = t.over.get();
    if from == to || !page.dom.borrow().is_connected(to).unwrap_or(false) {
        return;
    }
    t.over.set(to);
    let from_connected = page.dom.borrow().is_connected(from).unwrap_or(false);
    let (left, entered) = boundary_chains(if from_connected { hover_chain(page, from) } else { Vec::new() }, hover_chain(page, to));
    let (from_key, to_key) = (key_of(page, from), key_of(page, to));
    if from_connected {
        if let Some(k) = from_key {
            fire(page, k, &touch_pointer_event(page, "pointerout", t, p, modifiers, to_key));
        }
    }
    for n in left {
        if let Some(k) = key_of(page, n) {
            fire(page, k, &touch_pointer_event(page, "pointerleave", t, p, modifiers, to_key));
        }
    }
    if let Some(k) = to_key {
        fire(page, k, &touch_pointer_event(page, "pointerover", t, p, modifiers, from_key));
    }
    for n in entered.into_iter().rev() {
        if let Some(k) = key_of(page, n) {
            fire(page, k, &touch_pointer_event(page, "pointerenter", t, p, modifiers, from_key));
        }
    }
}

/// pointerover and pointerenter (outer to inner) for a touch coming down.
fn touch_enter(page: &Rc<PageDom>, t: &ActiveTouch, p: &PointerState, modifiers: Modifiers) {
    fire_touch_pointer(page, "pointerover", t, p, modifiers);
    for n in hover_chain(page, t.target).into_iter().rev() {
        if let Some(k) = key_of(page, n) {
            fire(page, k, &touch_pointer_event(page, "pointerenter", t, p, modifiers, None));
        }
    }
}

/// `last` (pointerup or pointercancel), then lostpointercapture,
/// pointerout and pointerleave (inner to outer) for a touch going away.
fn touch_leave(page: &Rc<PageDom>, last: &str, t: &mut ActiveTouch, p: &PointerState, modifiers: Modifiers) {
    capture(page, t, p, modifiers);
    fire_touch_pointer_routed(page, last, t, p, modifiers);
    // The implicit release: no capture is pending after the last event of the touch.
    if let Some(node) = crate::pointer_capture::touch_release_pending(page, t.pointer_id) {
        fire_touch_pointer_at(page, node, "lostpointercapture", t, p, modifiers);
    }
    crate::pointer_capture::touch_capture_end(page, t.pointer_id);
    let over = t.over.get();
    fire_touch_pointer_at(page, over, "pointerout", t, p, modifiers);
    for n in hover_chain(page, over) {
        if let Some(k) = key_of(page, n) {
            fire(page, k, &touch_pointer_event(page, "pointerleave", t, p, modifiers, None));
        }
    }
}

fn touch_point(page: &PageDom, t: &ActiveTouch) -> crate::event::TouchPoint {
    crate::event::TouchPoint {
        identifier: t.input.id,
        target: key_of(page, t.target),
        client_x: t.input.x,
        client_y: t.input.y,
        screen_x: t.input.x,
        screen_y: t.input.y,
        page_x: t.input.x + page.layout.borrow().scroll_x() as f64,
        page_y: t.input.y + page.layout.borrow().scroll_y() as f64,
        radius_x: t.input.radius_x,
        radius_y: t.input.radius_y,
        rotation_angle: t.input.rotation_angle,
        force: t.input.force,
    }
}

/// Fires `ty` at `target` with `changed` and the touches down (`active`).
/// Returns false if it was canceled.
fn fire_touch_event(
    page: &Rc<PageDom>,
    ty: &str,
    target: NodeId,
    active: &[ActiveTouch],
    changed: &[&ActiveTouch],
    cancelable: bool,
    modifiers: Modifiers,
) -> bool {
    let Some(key) = key_of(page, target) else { return true };
    let ev = EventCore::trusted(page, EventInterface::TouchEvent, ty, true, cancelable);
    {
        let mut f = ev.fields.borrow_mut();
        f.ctrl_key = modifiers.ctrl;
        f.shift_key = modifiers.shift;
        f.alt_key = modifiers.alt;
        f.meta_key = modifiers.meta;
        f.touch = Some(Rc::new(crate::event::TouchLists {
            touches: active.iter().map(|t| touch_point(page, t)).collect(),
            target_touches: active.iter().filter(|t| t.target == target).map(|t| touch_point(page, t)).collect(),
            changed_touches: changed.iter().map(|t| touch_point(page, t)).collect(),
        }));
    }
    fire(page, key, &ev)
}

/// The `touch-action` pan restrictions of `target` (x, y blocked).
fn touch_pan_blocked(page: &PageDom, target: NodeId) -> (bool, bool) {
    let style = page.layout().style().computed(&page.dom.borrow(), target);
    style.map_or((false, false), |c| (c.pan_x_blocked, c.pan_y_blocked))
}

/// Whether `touch-action` (`blocked`: x, y) lets a move by (dx, dy) scroll.
fn touch_pans(blocked: (bool, bool), dx: f64, dy: f64) -> bool {
    if dx.abs() > dy.abs() {
        !blocked.0
    } else {
        !blocked.1
    }
}

/// A fling's reach: how long the finger's release velocity goes on, in seconds (one jump, as
/// the momentum), and the velocity limit (px/s) and the least one that flings.
const FLING_SECONDS: f64 = 0.1;
const FLING_MAX_VELOCITY: f64 = 4000.0;
const FLING_MIN_VELOCITY: f64 = 100.0;

/// One step of a touch pan: the finger moved by `finger` (dx, dy), the content follows it. Each
/// axis goes to the scroller it latched to, else to the first scroll container from `start` up
/// (the viewport last) that can move that way; one at its limit chains to the next unless it
/// contains the overscroll, and the first that moves latches the axis for the rest of the
/// gesture (Chrome's scroll latching). Moves are neither snapped nor smooth; snapping is the
/// pan's end ([`finish_touch_pan`]). Queues `scroll` for each box that moved.
fn pan_scroll(page: &Rc<PageDom>, start: NodeId, pan: &mut TouchPan, finger: (f64, f64)) {
    use pyro_dom::layout::ScrollTarget;
    let mut moved: Vec<ScrollTarget> = Vec::new();
    {
        let dom = page.dom.borrow();
        let mut layout = page.layout.borrow_mut();
        // Moves `target` by `amount` along an axis (0: x), whether it moved.
        let step = |layout: &mut pyro_dom::layout::Layout, target: ScrollTarget, axis: usize, amount: i32| -> bool {
            let node = match target {
                ScrollTarget::Viewport => None,
                ScrollTarget::Container(n) => Some(n),
            };
            let cur = layout.scroll_position(node);
            let want = if axis == 0 { (cur.0.saturating_add(amount), cur.1) } else { (cur.0, cur.1.saturating_add(amount)) };
            let to = layout.clamp_scroll_position(&dom, node, want);
            layout.set_scroll_position(&dom, node, to)
        };
        for (axis, d) in [finger.0, finger.1].into_iter().enumerate() {
            // Content follows the finger: the scroll offset moves the other way.
            let amount = -(d.round().clamp(i32::MIN as f64 + 1.0, i32::MAX as f64) as i32);
            if amount == 0 {
                continue;
            }
            let latched = pan.latch[axis];
            let hit = match latched {
                Some(latched) => latched.filter(|&t| step(&mut *layout, t, axis, amount)),
                None => {
                    let axes = pyro_dom::style::ScrollAxes { x: axis == 0, y: axis == 1 };
                    let mut chosen = None;
                    let mut contained = false;
                    let mut cur = Some(start);
                    while let Some(el) = cur {
                        let c = match layout.scroll_container(&dom, el, axes) {
                            ScrollTarget::Container(c) => c,
                            ScrollTarget::Viewport => break,
                        };
                        if step(&mut *layout, ScrollTarget::Container(c), axis, amount) {
                            chosen = Some(ScrollTarget::Container(c));
                            break;
                        }
                        if layout.contains_overscroll(&dom, c, axes) {
                            contained = true;
                            break;
                        }
                        cur = flat_parent_element(&dom, c);
                    }
                    if chosen.is_none() && !contained && step(&mut *layout, ScrollTarget::Viewport, axis, amount) {
                        chosen = Some(ScrollTarget::Viewport);
                    }
                    if chosen.is_some() || contained {
                        pan.latch[axis] = Some(chosen);
                    }
                    chosen
                }
            };
            if let Some(t) = hit {
                if !moved.contains(&t) {
                    moved.push(t);
                }
                if !pan.scrolled.contains(&t) {
                    pan.scrolled.push(t);
                }
            }
        }
    }
    // (No borrow is held here: queueing a scroll posts a task.)
    for t in moved {
        let node = match t {
            ScrollTarget::Viewport => None,
            ScrollTarget::Container(n) => Some(n),
        };
        crate::smooth_scroll::cancel(page, node);
        crate::layout_api::queue_scroll(page, node, false);
    }
}

/// The finger's release velocity (px/s) from its last positions: none when it rested before
/// lifting (a pause between the last move and the release, or one move only).
fn touch_fling_velocity(pan: &TouchPan, now: std::time::Instant) -> Option<(f64, f64)> {
    let recent: Vec<&(std::time::Instant, f64, f64)> =
        pan.samples.iter().filter(|(at, _, _)| now.saturating_duration_since(*at) <= std::time::Duration::from_millis(100)).collect();
    let (first, last) = (recent.first()?, recent.last()?);
    if now.saturating_duration_since(last.0) > std::time::Duration::from_millis(50) {
        return None;
    }
    let dt = last.0.saturating_duration_since(first.0).as_secs_f64().max(0.001);
    if recent.len() < 2 {
        return None;
    }
    let v = |a: f64, b: f64| (b - a) / dt;
    let (vx, vy) = (v(first.1, last.1), v(first.2, last.2));
    let speed = vx.hypot(vy);
    if speed < FLING_MIN_VELOCITY {
        return None;
    }
    let k = if speed > FLING_MAX_VELOCITY { FLING_MAX_VELOCITY / speed } else { 1.0 };
    Some((vx * k, vy * k))
}

/// The end of a touch pan (touchend, touchcancel): a release with velocity flings (the content
/// goes on by `FLING_SECONDS` of it, snapped ahead like a relative scroll), any other release
/// snaps where it stopped; each box the gesture moved (or the end moved) then gets `scrollend`.
fn finish_touch_pan(page: &Rc<PageDom>, pan: TouchPan, fling: bool) {
    use pyro_dom::layout::ScrollTarget;
    let velocity = if fling { touch_fling_velocity(&pan, std::time::Instant::now()) } else { None };
    let mut ended: Vec<ScrollTarget> = Vec::new();
    for &t in &pan.scrolled {
        let node = match t {
            ScrollTarget::Viewport => None,
            ScrollTarget::Container(n) => Some(n),
        };
        {
            let dom = page.dom.borrow();
            let mut layout = page.layout.borrow_mut();
            let _moved = match velocity {
                Some((vx, vy)) => {
                    // Only the axes that latched to this box continue.
                    let by = |axis: usize, v: f64| {
                        (pan.latch[axis] == Some(Some(t))).then(|| (-v * FLING_SECONDS).round().clamp(i32::MIN as f64 + 1.0, i32::MAX as f64) as i32)
                    };
                    let (l, tp) = (by(0, vx), by(1, vy));
                    match node {
                        Some(el) => layout.element_scroll_by(&dom, el, l, tp),
                        None => layout.window_scroll_by(&dom, l, tp),
                    }
                }
                None => {
                    let (x, y) = layout.scroll_position(node);
                    match node {
                        Some(el) => layout.element_scroll_to(&dom, el, Some(x), Some(y)),
                        None => layout.window_scroll_to(&dom, Some(x), Some(y)),
                    }
                }
            };
        }
        ended.push(t);
    }
    for t in ended {
        let node = match t {
            ScrollTarget::Viewport => None,
            ScrollTarget::Container(n) => Some(n),
        };
        crate::smooth_scroll::cancel(page, node);
        crate::layout_api::queue_scroll(page, node, true);
    }
}

/// CDP `Input.dispatchTouchEvent` in this document. `points` are the
/// touches the command lists (ids resolved, coordinates in this document's
/// viewport): for touchStart and touchMove, new ids come down and known
/// ones move; touchEnd releases the listed ones (all when empty); touchCancel
/// cancels all. Moves fire first (one touchmove per target, listing every
/// moved touch), then presses and releases one touch at a time, as Chrome
/// does. Errors (no element under a new touch) change nothing.
pub fn dispatch_touch(
    page: &Rc<PageDom>,
    seq: &mut TouchSequence,
    kind: TouchKind,
    points: &[TouchInput],
    modifiers: Modifiers,
) -> Result<(), String> {
    // HTML activation-triggering input: touchend.
    if kind == TouchKind::End {
        page.notify_input_activation();
    }
    // A navigation since the sequence began leaves its touches behind.
    if seq.doc.is_some_and(|d| d != page.document()) {
        seq.reset();
    }
    let mut moved = Vec::new();
    let mut pressed = Vec::new();
    let mut released = Vec::new();
    match kind {
        TouchKind::Start | TouchKind::Move => {
            for p in points {
                match seq.active.iter().position(|t| t.input.id == p.id) {
                    Some(i) => {
                        let t = &seq.active[i].input;
                        if (t.x, t.y, t.radius_x, t.radius_y, t.force) != (p.x, p.y, p.radius_x, p.radius_y, p.force) {
                            moved.push((i, p.clone()));
                        }
                    }
                    None => {
                        let target =
                            pointer_target(page, p.x, p.y, true).ok_or_else(|| "no element at point".to_owned())?;
                        pressed.push((p.clone(), target));
                    }
                }
            }
        }
        TouchKind::End if points.is_empty() => released.extend(seq.active.iter().map(|t| t.input.id)),
        TouchKind::End => released.extend(points.iter().map(|p| p.id)),
        TouchKind::Cancel => {}
    }
    let pointer_moves: Vec<usize> = moved.iter().map(|(i, _)| *i).collect();
    let movements: Vec<(f64, f64)> = moved.iter().map(|(i, p)| (p.x - seq.active[*i].input.x, p.y - seq.active[*i].input.y)).collect();
    for (i, p) in moved {
        seq.active[i].input = p;
    }
    if !pointer_moves.is_empty() {
        for (&i, &movement) in pointer_moves.iter().zip(&movements) {
            let t = &mut seq.active[i];
            if t.pointer_cancelled {
                continue;
            }
            let state = PointerState::moved(&t.input, movement);
            capture(page, t, &state, modifiers);
            if !crate::target::unheard(page, "pointerrawupdate") && crate::secure_context::secure_page(page) {
                fire_touch_pointer_routed(page, "pointerrawupdate", t, &state, modifiers);
            }
            fire_touch_pointer_routed(page, "pointermove", t, &state, modifiers);
        }
        // A single touch within the slop region is still a tap: no
        // touchmove (Chrome's touchmove slop suppression).
        let first = &seq.active[0].input;
        let (dx, dy) = (first.x - seq.start.0, first.y - seq.start.1);
        if !seq.multi && !seq.left_slop && dx.hypot(dy) > TOUCH_SLOP {
            seq.left_slop = true;
        }
        if seq.multi || seq.left_slop {
            let mut targets: Vec<NodeId> = Vec::new();
            for &i in &pointer_moves {
                if !targets.contains(&seq.active[i].target) {
                    targets.push(seq.active[i].target);
                }
            }
            let changed: Vec<&ActiveTouch> = pointer_moves.iter().map(|&i| &seq.active[i]).collect();
            let cancelable = !seq.scrolling;
            let mut canceled = false;
            for target in targets {
                canceled |= !fire_touch_event(page, "touchmove", target, &seq.active, &changed, cancelable, modifiers);
            }
            // An uncanceled touchmove out of the slop region starts a scroll
            // (unless touch-action forbids it), which cancels the pointer.
            if !seq.multi && !seq.scrolling && !canceled && touch_pans(seq.pan_blocked, dx, dy) {
                seq.scrolling = true;
                seq.pan = Some(TouchPan {
                    last: seq.start,
                    samples: vec![(std::time::Instant::now(), seq.start.0, seq.start.1)],
                    ..Default::default()
                });
                let t = &mut seq.active[0];
                t.pointer_cancelled = true;
                touch_leave(page, "pointercancel", t, &PointerState::scrolled(), modifiers);
            }
            // The scroll follows the finger: by its movement since the last step (the first
            // one since the touch came down, as Chrome's gesture scroll begin carries it).
            if seq.scrolling && !seq.multi && !page.is_disposed() && seq.doc == Some(page.document()) {
                if let Some(first) = seq.active.first() {
                    let (x, y, start) = (first.input.x, first.input.y, first.target);
                    let blocked = seq.pan_blocked;
                    if let Some(pan) = seq.pan.as_mut() {
                        let finger = (if blocked.0 { 0.0 } else { x - pan.last.0 }, if blocked.1 { 0.0 } else { y - pan.last.1 });
                        pan.last = (if blocked.0 { x } else { pan.last.0 + finger.0.round() }, if blocked.1 { y } else { pan.last.1 + finger.1.round() });
                        pan.samples.push((std::time::Instant::now(), x, y));
                        if pan.samples.len() > 8 {
                            pan.samples.remove(0);
                        }
                        pan_scroll(page, start, pan, finger);
                    }
                }
            }
        }
    }
    for (input, target) in pressed {
        if seq.active.is_empty() {
            seq.reset();
            seq.doc = Some(page.document());
            seq.start = (input.x, input.y);
            // touch-action is what it is as the touch comes down (before pointerdown's
            // listeners run).
            seq.pan_blocked = touch_pan_blocked(page, target);
        } else {
            seq.multi = true;
        }
        if seq.next_pointer_id < 2 {
            seq.next_pointer_id = 2;
        }
        if seq.next_pointer_id == PEN_POINTER_ID {
            seq.next_pointer_id += 1;
        }
        let t = ActiveTouch {
            input,
            pointer_id: seq.next_pointer_id,
            primary: seq.active.is_empty(),
            target,
            over: std::cell::Cell::new(target),
            pointer_cancelled: false,
        };
        seq.next_pointer_id += 1;
        let state = PointerState::pressed(&t.input);
        note_link_pointer_down(page, target);
        // Implicit capture of the touchstart target is pending from the start of pointerdown
        // (Pointer Events §4.1.3.2); script may override or release it there.
        crate::pointer_capture::touch_capture_begin(page, t.pointer_id, target);
        touch_enter(page, &t, &state, modifiers);
        if !fire_touch_pointer(page, "pointerdown", &t, &state, modifiers) {
            seq.mouse_suppressed = true;
        }
        seq.active.push(t);
        let t = seq.active.last().expect("pushed");
        if !fire_touch_event(page, "touchstart", target, &seq.active, &[t], !seq.scrolling, modifiers) {
            seq.no_tap = true;
        }
    }
    let mut tap = None;
    for id in released {
        let Some(i) = seq.active.iter().position(|t| t.input.id == id) else { continue };
        let mut t = seq.active.remove(i);
        if !t.pointer_cancelled {
            let state = PointerState::released(&t.input);
            touch_leave(page, "pointerup", &mut t, &state, modifiers);
        }
        if !fire_touch_event(page, "touchend", t.target, &seq.active, &[&t], !seq.scrolling, modifiers) {
            seq.no_tap = true;
        }
        if seq.active.is_empty() && !seq.multi && !seq.left_slop && !seq.scrolling && !seq.no_tap {
            tap = Some((t.input.x, t.input.y, t.pointer_id, seq.mouse_suppressed));
        }
    }
    if kind == TouchKind::Cancel {
        for mut t in std::mem::take(&mut seq.active) {
            if !t.pointer_cancelled {
                let state = PointerState::cancelled(&t.input);
                touch_leave(page, "pointercancel", &mut t, &state, modifiers);
            }
            fire_touch_event(page, "touchcancel", t.target, &[], &[&t], false, modifiers);
        }
    }
    if seq.active.is_empty() {
        // The pan ends with the last touch: after touchend, a fling goes on from the release
        // (a cancel stops where it is).
        if let Some(pan) = seq.pan.take() {
            if !page.is_disposed() && seq.doc == Some(page.document()) {
                finish_touch_pan(page, pan, kind == TouchKind::End);
            }
        }
        seq.reset();
    }
    // A tap: Chrome's gesture tap at the release point (mousemove,
    // mousedown, mouseup, click; the mouse events not when pointerdown was
    // canceled).
    if let Some((x, y, pointer_id, suppressed)) = tap {
        if !page.is_disposed() {
            let m = MouseInput { x, y, modifiers, ..Default::default() };
            let down = MouseInput { button: MouseButton::Left, buttons: 1, click_count: 1, ..m.clone() };
            let up = MouseInput { buttons: 0, ..down.clone() };
            if suppressed {
                // Only the click: the release sees the press target and a
                // suppressed mouseup.
                page.set_press_target(pointer_target(page, x, y, true));
                page.compat_mouse_suppressed.set(true);
            } else {
                let _ = dispatch_mouse_inner(page, MouseKind::Moved, &m, false, Some(pointer_id));
                let _ = dispatch_mouse_inner(page, MouseKind::Pressed, &down, false, Some(pointer_id));
            }
            let _ = dispatch_mouse_inner(page, MouseKind::Released, &up, false, Some(pointer_id));
        }
    }
    Ok(())
}

// ----- native drag and drop ------------------------------------------------------
//
// Modelled on Blink (EventHandler::UpdateDragAndDrop / PerformDragAndDrop /
// CancelDragAndDrop) and checked against headless Chromium driven by
// Playwright 1.59.1: event order, targets, cancelability, DataTransfer modes,
// effectAllowed/dropEffect and the intercepted drag data.

/// Chrome's macOS drag threshold (Blink kDragThresholdX/Y), CSS px.
const DRAG_THRESHOLD: f64 = 3.0;
const OP_COPY: i64 = 1;
const OP_LINK: i64 = 2;
const OP_MOVE: i64 = 16;
const OP_ALL: i64 = OP_COPY | OP_LINK | OP_MOVE;

/// One string item of CDP `Input.DragData`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DragItem {
    pub mime_type: String,
    pub data: String,
    pub title: Option<String>,
    pub base_url: Option<String>,
}

/// CDP `Input.DragData` (string items only; file drags are not supported).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DragData {
    pub items: Vec<DragItem>,
    /// Copy = 1, Link = 2, Move = 16; -1 when every operation is allowed.
    pub drag_operations_mask: i64,
}

/// CDP `Input.dispatchDragEvent` types.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DragKind {
    Enter,
    Over,
    Drop,
    Cancel,
}

pub(crate) struct DragSession {
    /// The page's drag source and its DataTransfer; None for a drag entering
    /// from outside the page.
    source: Option<(NodeId, Rc<NativeTransfer>)>,
    /// Current drop target (Blink drag_target_).
    target: Option<NodeId>,
    /// Blink should_only_fire_drag_over_event_: a new target got dragenter, so
    /// the next update fires only dragover.
    only_dragover: bool,
    /// The operation the current target accepted by canceling the last
    /// dragenter/dragover.
    operation: Option<&'static str>,
}

use crate::data_transfer::{Mode, NativeTransfer};

/// Nearest ancestor-or-self that is draggable: `draggable=true`, or, unless
/// it says `draggable=false`, an `<img>` whose image has loaded (Chrome drags
/// only available images) or a link with an href. As in Blink, an image is
/// found before the link around it.
fn drag_source(dom: &Dom, target: NodeId) -> Option<NodeId> {
    let mut cur = Some(target);
    while let Some(c) = cur {
        if let Ok(e) = dom.element(c) {
            let draggable = e.attr("draggable");
            if draggable.is_some_and(|d| d.eq_ignore_ascii_case("true")) {
                return Some(c);
            }
            let auto = !draggable.is_some_and(|d| d.eq_ignore_ascii_case("false"));
            if auto && e.is_html() && *e.local_name() == *"img" && dom.natural_size(c).is_some() {
                return Some(c);
            }
            if auto && e.is_html() && *e.local_name() == *"a" && e.attr("href").is_some() {
                return Some(c);
            }
        }
        cur = dom.parent(c).ok().flatten();
    }
    None
}

/// Image drag data, as Chrome fills it before dragstart: the URL is the
/// enclosing link's when the image is inside one, else the image's.
fn image_items(page: &PageDom, img: NodeId) -> Vec<DragItem> {
    let dom = page.dom.borrow();
    let Ok(e) = dom.element(img) else { return Vec::new() };
    if !(e.is_html() && *e.local_name() == *"img") {
        return Vec::new();
    }
    let Some(src) = e.attr("src") else { return Vec::new() };
    let mut url = crate::html::resolve_url(&dom, img, src);
    let mut cur = dom.parent(img).ok().flatten();
    while let Some(c) = cur {
        if let Ok(a) = dom.element(c) {
            if a.is_html() && *a.local_name() == *"a" {
                if let Some(href) = a.attr("href") {
                    url = crate::html::resolve_url(&dom, c, href);
                    break;
                }
            }
        }
        cur = dom.parent(c).ok().flatten();
    }
    let html = dom.outer_html(img).unwrap_or_default();
    vec![
        DragItem { mime_type: "text/uri-list".into(), data: url.clone(), title: None, base_url: None },
        DragItem { mime_type: "text/html".into(), data: html, title: None, base_url: Some(url) },
    ]
}

fn mask_of(effect_allowed: &str) -> i64 {
    match effect_allowed {
        "none" => 0,
        "copy" => OP_COPY,
        "link" => OP_LINK,
        "move" => OP_MOVE,
        "copyLink" => OP_COPY | OP_LINK,
        "copyMove" => OP_COPY | OP_MOVE,
        "linkMove" => OP_LINK | OP_MOVE,
        _ => -1,
    }
}

fn allowed_ops(mask: i64) -> i64 {
    if mask < 0 {
        OP_ALL
    } else {
        mask & OP_ALL
    }
}

fn effect_allowed_of(mask: i64) -> &'static str {
    match allowed_ops(mask) {
        0 => "none",
        OP_COPY => "copy",
        OP_LINK => "link",
        OP_MOVE => "move",
        3 => "copyLink",
        17 => "copyMove",
        18 => "linkMove",
        _ => "all",
    }
}

/// The `effectAllowed` a drag target's DataTransfer reports. Chrome keeps
/// "uninitialized" there while the in-page source never assigned a valid
/// effectAllowed (the mask round trip alone cannot tell that from "all");
/// otherwise the mask decides.
fn target_effect_allowed(source: &Option<(NodeId, Rc<NativeTransfer>)>, mask: i64) -> String {
    match source {
        Some((_, transfer)) if mask < 0 && transfer.effect_allowed() == "uninitialized" => "uninitialized".to_string(),
        _ => effect_allowed_of(mask).to_string(),
    }
}

/// Chrome's default operation for what the source allows: copy, then move,
/// then link; None when it allows nothing.
fn default_operation(mask: i64) -> Option<&'static str> {
    let ops = allowed_ops(mask);
    [("copy", OP_COPY), ("move", OP_MOVE), ("link", OP_LINK)]
        .into_iter()
        .find(|(_, bit)| ops & bit != 0)
        .map(|(n, _)| n)
}

/// The operation a target accepting the drag gets. A listener that assigned
/// dropEffect gets that operation when the source allows it; an assigned
/// "none" means no operation (no drop event, dragend reports "none"). A
/// listener that only canceled the event leaves dropEffect unset (it still
/// reads "none", as in Chrome) and gets Chrome's default.
fn accepted_operation(transfer: &NativeTransfer, mask: i64) -> Option<&'static str> {
    if !transfer.drop_effect_assigned() {
        return default_operation(mask);
    }
    let ops = allowed_ops(mask);
    let drop_effect = transfer.drop_effect();
    let named = [("copy", OP_COPY), ("link", OP_LINK), ("move", OP_MOVE)];
    named.iter().find(|(name, _)| *name == drop_effect).and_then(|&(name, bit)| (ops & bit != 0).then_some(name))
}

fn drag_event(page: &Rc<PageDom>, ty: &str, m: &MouseInput, cancelable: bool, transfer: &NativeTransfer) -> EventData {
    let ev = mouse_event(page, EventInterface::DragEvent, ty, m, cancelable);
    ev.fields.borrow_mut().data_transfer = Some(transfer.reference.clone());
    ev
}

/// Fires `ty` on `target` with `transfer` in `mode`, disabling the store
/// afterwards. Returns false if the event was canceled.
fn fire_drag(
    page: &Rc<PageDom>,
    target: NodeId,
    ty: &str,
    m: &MouseInput,
    cancelable: bool,
    transfer: &NativeTransfer,
    mode: Mode,
) -> bool {
    let Some(key) = key_of(page, target) else { return true };
    transfer.set_mode(mode);
    let not_canceled = fire(page, key, &drag_event(page, ty, m, cancelable, transfer));
    transfer.set_mode(Mode::Disabled);
    not_canceled
}

/// Link drag data, as Chrome fills it before dragstart.
fn link_items(page: &PageDom, link: NodeId) -> Vec<DragItem> {
    let dom = page.dom.borrow();
    let Ok(e) = dom.element(link) else { return Vec::new() };
    if !(e.is_html() && *e.local_name() == *"a") {
        return Vec::new();
    }
    let Some(href) = e.attr("href") else { return Vec::new() };
    let url = crate::html::resolve_url(&dom, link, href);
    let title = dom.inner_text(page.layout().style(), link).unwrap_or_default();
    let html = dom.outer_html(link).unwrap_or_default();
    vec![
        DragItem { mime_type: "text/uri-list".into(), data: url.clone(), title: Some(title), base_url: None },
        DragItem { mime_type: "text/plain".into(), data: url.clone(), title: None, base_url: None },
        DragItem { mime_type: "text/html".into(), data: html, title: None, base_url: Some(url) },
    ]
}

fn start_drag(page: &Rc<PageDom>, pointer_target: NodeId, m: &MouseInput) -> Option<DragData> {
    if m.buttons & 1 == 0 || page.drag.borrow().is_some() {
        return None;
    }
    let (source, x0, y0) = page.drag_candidate.get()?;
    if (m.x - x0).abs() < DRAG_THRESHOLD && (m.y - y0).abs() < DRAG_THRESHOLD {
        return None;
    }
    page.drag_candidate.set(None);
    if !page.dom.borrow().is_connected(source).unwrap_or(false) {
        return None;
    }
    let mut defaults = link_items(page, source);
    if defaults.is_empty() {
        defaults = image_items(page, source);
    }
    let strings: Vec<_> = defaults.iter().map(|i| (i.mime_type.clone(), i.data.clone())).collect();
    let transfer = Rc::new(NativeTransfer::new(page, &strings, "uninitialized", Mode::ReadWrite)?);
    let doc = page.document();
    if !fire_drag(page, source, "dragstart", m, true, &transfer, Mode::ReadWrite)
        || page.is_disposed()
        || page.document() != doc
    {
        return None;
    }
    // The pointer stream ends: pointercancel, and no click on release.
    if let Some(key) = key_of(page, pointer_target) {
        let released = MouseInput { buttons: 0, ..m.clone() };
        fire(page, key, &mouse_event(page, EventInterface::PointerEvent, "pointercancel", &released, false));
    }
    page.take_press_target();
    let items = transfer
        .strings()
        .into_iter()
        .map(|(mime_type, data)| {
            // Prefilled link entries keep Chrome's title/baseURL metadata.
            let default = defaults.iter().find(|d| d.mime_type == mime_type && d.data == data);
            DragItem {
                title: default.and_then(|d| d.title.clone()),
                base_url: default.and_then(|d| d.base_url.clone()),
                mime_type,
                data,
            }
        })
        .collect();
    let data = DragData { items, drag_operations_mask: mask_of(&transfer.effect_allowed()) };
    *page.drag.borrow_mut() =
        Some(DragSession { source: Some((source, transfer)), target: None, only_dragover: false, operation: None });
    Some(data)
}

/// Target-side entries in Chrome's order: text, URL and HTML first, then
/// other formats as given.
fn target_strings(data: &DragData) -> Vec<(String, String)> {
    let rank = |t: &str| match t {
        "text/plain" => 0,
        "text/uri-list" => 1,
        "text/html" => 2,
        _ => 3,
    };
    let mut items: Vec<_> = data.items.iter().map(|i| (i.mime_type.to_ascii_lowercase(), i.data.clone())).collect();
    items.sort_by_key(|(t, _)| rank(t));
    items
}

fn fire_source_drag(page: &Rc<PageDom>, source: &Option<(NodeId, Rc<NativeTransfer>)>, m: &MouseInput) {
    if let Some((node, transfer)) = source {
        fire_drag(page, *node, "drag", m, true, transfer, Mode::Protected);
    }
}

/// Blink UpdateDragAndDrop: drag on the source, then dragenter on a new
/// target and dragleave on the old one, or dragover on the same target.
fn update_drag(
    page: &Rc<PageDom>,
    m: &MouseInput,
    data: &DragData,
    files: &[crate::file::FileEntry],
) -> Result<(), String> {
    let target = pointer_target(page, m.x, m.y, true);
    let (source, current, mut only_dragover) = {
        let drag = page.drag.borrow();
        let s = drag.as_ref().ok_or("no drag in progress")?;
        (s.source.clone(), s.target, s.only_dragover)
    };
    let transfer = NativeTransfer::with_files(
        page,
        &target_strings(data),
        files,
        &target_effect_allowed(&source, data.drag_operations_mask),
        Mode::Protected,
    )
    .ok_or("no script context for the drag")?;
    let mut accepted = false;
    if target != current {
        if let Some(t) = target {
            fire_source_drag(page, &source, m);
            accepted = !fire_drag(page, t, "dragenter", m, true, &transfer, Mode::Protected);
        }
        if let Some(c) = current {
            fire_drag(page, c, "dragleave", m, false, &transfer, Mode::Protected);
        }
        if target.is_some() {
            only_dragover = true;
        }
    } else if let Some(t) = target {
        if !only_dragover {
            fire_source_drag(page, &source, m);
        }
        accepted = !fire_drag(page, t, "dragover", m, true, &transfer, Mode::Protected);
        only_dragover = false;
    }
    let operation =
        if accepted { accepted_operation(&transfer, data.drag_operations_mask) } else { None };
    if let Some(s) = page.drag.borrow_mut().as_mut() {
        s.target = target;
        s.only_dragover = only_dragover;
        s.operation = operation;
    }
    Ok(())
}

fn end_drag(page: &Rc<PageDom>, source: &Option<(NodeId, Rc<NativeTransfer>)>, m: &MouseInput, effect: &str) {
    if let Some((node, transfer)) = source {
        transfer.set_drop_effect(effect);
        fire_drag(page, *node, "dragend", m, false, transfer, Mode::Protected);
    }
}

/// CDP `Input.dispatchDragEvent` for the page's drag session. dragEnter and
/// dragOver update the drop target; drop fires `drop` when the target
/// accepted the drag (else drag/dragleave), then `dragend` on the source;
/// dragCancel fires only `dragend`. A dragEnter without a session starts a
/// drag entering from outside the page.
pub fn dispatch_drag(
    page: &Rc<PageDom>,
    kind: DragKind,
    x: f64,
    y: f64,
    data: &DragData,
    modifiers: Modifiers,
) -> Result<(), String> {
    dispatch_drag_files(page, kind, x, y, data, &[], modifiers)
}

pub fn dispatch_drag_files(
    page: &Rc<PageDom>,
    kind: DragKind,
    x: f64,
    y: f64,
    data: &DragData,
    files: &[crate::file::FileEntry],
    modifiers: Modifiers,
) -> Result<(), String> {
    let m = MouseInput { x, y, modifiers, ..MouseInput::default() };
    if page.drag.borrow().is_none() {
        match kind {
            DragKind::Cancel => return Ok(()),
            DragKind::Drop => return Err("no drag in progress".into()),
            DragKind::Enter | DragKind::Over => {}
        }
        *page.drag.borrow_mut() =
            Some(DragSession { source: None, target: None, only_dragover: false, operation: None });
    }
    match kind {
        DragKind::Enter | DragKind::Over => update_drag(page, &m, data, files),
        DragKind::Drop => {
            if page.drag.borrow().as_ref().is_some_and(|s| s.only_dragover) {
                update_drag(page, &m, data, files)?;
            }
            let Some(session) = page.drag.borrow_mut().take() else { return Ok(()) };
            match (session.operation, session.target) {
                (Some(operation), Some(target)) => {
                    let transfer = NativeTransfer::with_files(
                        page,
                        &target_strings(data),
                        files,
                        &target_effect_allowed(&session.source, data.drag_operations_mask),
                        Mode::ReadOnly,
                    )
                    .ok_or("no script context for the drag")?;
                    transfer.set_drop_effect(operation);
                    fire_drag(page, target, "drop", &m, true, &transfer, Mode::ReadOnly);
                    end_drag(page, &session.source, &m, operation);
                }
                (_, target) => {
                    fire_source_drag(page, &session.source, &m);
                    if let Some(t) = target {
                        if let Some(transfer) = NativeTransfer::with_files(
                            page,
                            &target_strings(data),
                            files,
                            &target_effect_allowed(&session.source, data.drag_operations_mask),
                            Mode::Protected,
                        ) {
                            fire_drag(page, t, "dragleave", &m, false, &transfer, Mode::Protected);
                        }
                    }
                    end_drag(page, &session.source, &m, "none");
                }
            }
            Ok(())
        }
        DragKind::Cancel => {
            if let Some(session) = page.drag.borrow_mut().take() {
                end_drag(page, &session.source, &m, "none");
            }
            Ok(())
        }
    }
}

fn common_ancestor(dom: &Dom, a: NodeId, b: NodeId) -> Option<NodeId> {
    let mut cur = Some(a);
    while let Some(c) = cur {
        if dom.contains(c, b).unwrap_or(false) {
            return Some(c);
        }
        cur = dom.parent(c).ok().flatten();
    }
    None
}

/// [`crate::thread_state::reset_thread`]'s part: [`focus_element_from_script`] restores the
/// option when it returns, so outside of a call it is unset.
pub(crate) fn thread_leftover() -> Option<&'static str> {
    FOCUS_VISIBLE_OPTION.with(|f| f.get().is_some()).then_some("input::FOCUS_VISIBLE_OPTION")
}
