//! User-interaction semantics: focus, click activation behavior, form
//! submission/reset, keyboard text editing and CDP-style mouse/key input.
//!
//! Hit testing for mouse input uses the faux layout (`layout_api`); no
//! geometry is fabricated here.

use std::rc::Rc;

use lp_dom::events::TargetKey;
use lp_dom::forms::InputType;
use lp_dom::{Dom, NodeId};

use crate::event::{EventCore, EventData, EventInterface};
use crate::page::{FocusTarget, FocusTransition, PageDom};
use crate::target::fire;

fn key_of(page: &PageDom, id: NodeId) -> Option<TargetKey> {
    page.dom.borrow().node_key(id)
}

/// Fires a trusted simple `Event` at `node`. Returns false if canceled.
pub fn fire_simple(page: &Rc<PageDom>, node: NodeId, ty: &str, bubbles: bool, cancelable: bool) -> bool {
    let Some(key) = key_of(page, node) else { return true };
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

/// HTML focusing steps, coordinated with the browsing-context owner.
pub fn focus_element(page: &Rc<PageDom>, el: NodeId) {
    focus_element_with_options(page, el, false);
}

/// Focus with the already-converted preventScroll option.
pub fn focus_element_with_options(page: &Rc<PageDom>, el: NodeId, prevent_scroll: bool) {
    let doc = page.document();
    if !live_element(page, doc, el) || !page.dom.borrow().is_focusable(el).unwrap_or(false) {
        return;
    }
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
    if !live_element(page, doc, el) || !page.dom.borrow().is_focusable(el).unwrap_or(false) {
        return;
    }
    let prev = page.dom.borrow().focused(doc).ok().flatten();
    if prev == Some(el) {
        return;
    }
    if let Some(prev) = prev {
        run_blur(page, doc, prev, Some(el), token);
    }
    if !live_document(page, doc, token)
        || !live_element(page, doc, el)
        || !page.dom.borrow().is_focusable(el).unwrap_or(false)
    {
        return;
    }
    let _ = page.dom.borrow_mut().set_focused(doc, Some(el));
    if is_text_control(&page.dom.borrow(), el) {
        page.editing_target.set(Some(el));
    }
    page.remember_focus_value(el);
    let scrolled = if token.prevents_scroll() {
        Vec::new()
    } else {
        page.layout.borrow_mut().reveal_for_focus(&page.dom.borrow(), el)
    };
    fire_focus_event(page, el, "focus", false, prev);
    if !live_document(page, doc, token) || !live_element(page, doc, el) {
        return;
    }
    fire_focus_event(page, el, "focusin", true, prev);
    if live_document(page, doc, token) {
        for target in scrolled {
            let node = match target {
                lp_dom::layout::ScrollTarget::Viewport => None,
                lp_dom::layout::ScrollTarget::Container(node) => Some(node),
            };
            crate::layout_api::fire_scroll(page, node);
        }
    }
}

fn run_blur(page: &Rc<PageDom>, doc: NodeId, el: NodeId, related: Option<NodeId>, token: &FocusTransition) {
    let _ = page.dom.borrow_mut().set_focused(doc, None);
    page.space_target.set(None);
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

/// Explicit blur keeps the owner's selected frame and remembered edit target.
pub fn blur_element(page: &Rc<PageDom>, el: NodeId) {
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
    let current = page.dom.borrow().value(el).ok();
    if let (Some(cur), Some(old)) = (current, page.take_focus_value(el)) {
        if cur != old && is_text_control(&page.dom.borrow(), el) {
            fire_simple(page, el, "change", true, false);
        }
    }
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

#[derive(Debug)]
enum Activation {
    Checkbox { el: NodeId, old: bool },
    Radio { el: NodeId, previously_checked: Vec<NodeId>, was_checked: bool },
    Submit { el: NodeId },
    Reset { el: NodeId },
    Link { el: NodeId },
    Label { el: NodeId },
    Summary { el: NodeId },
}

fn local(dom: &Dom, id: NodeId) -> Option<String> {
    dom.element(id).ok().filter(|e| e.is_html()).map(|e| e.name.local.to_string())
}

fn activation_for(dom: &Dom, el: NodeId) -> Option<Activation> {
    let l = local(dom, el)?;
    if dom.is_disabled(el).unwrap_or(false) {
        return None;
    }
    match l.as_str() {
        "input" => match dom.input_type(el).ok()? {
            InputType::Checkbox => Some(Activation::Checkbox { el, old: dom.checked(el).unwrap_or(false) }),
            InputType::Radio => Some(Activation::Radio { el, previously_checked: Vec::new(), was_checked: false }),
            InputType::Submit | InputType::Image => Some(Activation::Submit { el }),
            InputType::Reset => Some(Activation::Reset { el }),
            _ => None,
        },
        "button" => match dom.get_attribute(el, "type").ok().flatten().map(|t| t.to_ascii_lowercase()).as_deref() {
            Some("reset") => Some(Activation::Reset { el }),
            Some("button") => None,
            _ => Some(Activation::Submit { el }),
        },
        "a" | "area" if dom.has_attribute(el, "href").unwrap_or(false) => Some(Activation::Link { el }),
        "label" => Some(Activation::Label { el }),
        "summary" => {
            let p = dom.parent(el).ok()??;
            let is_first = local(dom, p).as_deref() == Some("details")
                && dom.element_children(p).ok()?.find(|&c| local(dom, c).as_deref() == Some("summary")) == Some(el);
            is_first.then_some(Activation::Summary { el })
        }
        _ => None,
    }
}

/// Dispatches a click-like event with activation behavior (HTML "activation
/// behavior" + legacy pre-activation for checkboxes/radios). Used for both
/// trusted input and `dispatchEvent`/`click()`.
pub fn dispatch_with_activation(page: &Rc<PageDom>, target: NodeId, ev: &EventData) -> bool {
    let is_click = &*ev.state.borrow().ty == "click"
        && matches!(ev.iface, EventInterface::MouseEvent | EventInterface::PointerEvent);
    let Some(key) = key_of(page, target) else { return true };
    if !is_click {
        return fire(page, key, ev);
    }
    // Activation target: the target or its nearest inclusive ancestor with
    // activation behavior (click bubbles).
    let mut act = None;
    {
        let dom = page.dom.borrow();
        let mut cur = Some(target);
        while let Some(c) = cur {
            if let Some(a) = activation_for(&dom, c) {
                act = Some(a);
                break;
            }
            cur = dom.parent_element(c).ok().flatten();
        }
    }
    // Legacy pre-activation.
    if let Some(a) = act.as_mut() {
        let mut dom = page.dom.borrow_mut();
        match a {
            Activation::Checkbox { el, old } => {
                let _ = dom.set_checked(*el, !*old);
            }
            Activation::Radio { el, previously_checked, was_checked } => {
                *was_checked = dom.checked(*el).unwrap_or(false);
                *previously_checked = radio_group_checked(&dom, *el);
                let _ = dom.set_checked(*el, true);
            }
            _ => {}
        }
    }
    let not_canceled = fire(page, key, ev);
    let Some(a) = act else { return not_canceled };
    if !not_canceled {
        // Legacy-canceled-activation: restore.
        let mut dom = page.dom.borrow_mut();
        match a {
            Activation::Checkbox { el, old } => {
                let _ = dom.set_checked(el, old);
            }
            Activation::Radio { el, previously_checked, was_checked } => {
                let _ = dom.set_checked(el, was_checked);
                for p in previously_checked {
                    let _ = dom.set_checked(p, true);
                }
            }
            _ => {}
        }
        return false;
    }
    match a {
        Activation::Checkbox { el, .. } => {
            fire_input_event(page, el, "input", "", None, false);
            fire_simple(page, el, "change", true, false);
        }
        Activation::Radio { el, was_checked, .. } => {
            if !was_checked {
                fire_input_event(page, el, "input", "", None, false);
                fire_simple(page, el, "change", true, false);
            }
        }
        Activation::Submit { el } => {
            let form = page.dom.borrow().form_owner(el).ok().flatten();
            if let Some(f) = form {
                submit_form(page, f, Some(el), true);
            }
        }
        Activation::Reset { el } => {
            let form = page.dom.borrow().form_owner(el).ok().flatten();
            if let Some(f) = form {
                reset_form(page, f);
            }
        }
        Activation::Link { el } => follow_hyperlink(page, el),
        Activation::Label { el } => {
            let control = crate::html::labeled_control(&page.dom.borrow(), el);
            if let Some(c) = control {
                // Clicks on the control itself (or interactive content inside
                // the label) are not re-targeted.
                let inside_control = page.dom.borrow().contains(c, target).unwrap_or(false);
                if !inside_control && c != target {
                    element_click(page, c);
                    focus_element(page, c);
                }
            }
        }
        Activation::Summary { el } => {
            let details = page.dom.borrow().parent(el).ok().flatten();
            if let Some(d) = details {
                let _ = page.dom.borrow_mut().toggle_attribute(d, "open", None);
                queue_simple(page, d, "toggle", false, false);
            }
        }
    }
    true
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

/// `HTMLElement.click()`: synthetic (untrusted) PointerEvent click. As in
/// browsers (and upstream Html.zig), `detail` is 0, `pointerId` is -1 and
/// `pointerType` is empty; libraries such as React Aria rely on `detail == 0`
/// to recognise a virtual click.
pub fn element_click(page: &Rc<PageDom>, el: NodeId) {
    if page.dom.borrow().is_disabled(el).unwrap_or(false) {
        return;
    }
    if !page.begin_click(el) {
        return; // click in progress flag
    }
    let ev = EventCore::new(page, EventInterface::PointerEvent, "click", true, true, true);
    ev.fields.borrow_mut().pointer_id = -1;
    dispatch_with_activation(page, el, &ev);
    page.end_click(el);
}

/// Controls a Space keyup clicks: buttons and button-like/checkable inputs.
fn space_clickable(dom: &Dom, el: NodeId) -> bool {
    if dom.is_disabled(el).unwrap_or(false) {
        return false;
    }
    match local(dom, el).as_deref() {
        Some("button") => true,
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
    let (url, target) = {
        let dom = page.dom.borrow();
        let Some(href) = dom.get_attribute(el, "href").ok().flatten() else { return };
        let url = crate::html::resolve_url(&dom, el, href);
        // HTML "download the hyperlink": a `download` link to a blob:, data:
        // or same-origin URL downloads instead of navigating (browsers ignore
        // the attribute cross-origin). Downloads are not performed yet
        // (documented gap), but the page must not navigate away.
        if dom.get_attribute(el, "download").ok().flatten().is_some() {
            let doc_url = dom.document_data(page.document()).map(|d| d.url.to_string()).unwrap_or_default();
            let same_origin = |a: &str, b: &str| match (::url::Url::parse(a), ::url::Url::parse(b)) {
                (Ok(a), Ok(b)) => a.origin().is_tuple() && a.origin() == b.origin(),
                _ => false,
            };
            if url.starts_with("blob:") || url.starts_with("data:") || same_origin(&url, &doc_url) {
                return;
            }
        }
        (url, dom.get_attribute(el, "target").ok().flatten().map(str::to_owned))
    };
    page.navigate(crate::page::NavigationRequest { url, target, method: "GET".into(), body: None, content_type: None });
}

// ----- forms ------------------------------------------------------------------

/// Form submission algorithm (simplified HTML §4.10.21.3): `requestSubmit`
/// fires `submit` (with interactive validation), then navigates with the
/// form data set via the page hook.
pub fn submit_form(page: &Rc<PageDom>, form: NodeId, submitter: Option<NodeId>, request: bool) {
    if !page.begin_submit(form) {
        return;
    }
    let proceed = if request {
        let novalidate = {
            let dom = page.dom.borrow();
            dom.has_attribute(form, "novalidate").unwrap_or(false)
                || submitter.is_some_and(|s| dom.has_attribute(s, "formnovalidate").unwrap_or(false))
        };
        if !novalidate && !crate::validity::check_form(page, form, true) {
            false
        } else {
            let ev = EventCore::trusted(page, EventInterface::SubmitEvent, "submit", true, true);
            ev.fields.borrow_mut().submitter = submitter.and_then(|s| key_of(page, s));
            match key_of(page, form) {
                Some(k) => fire(page, k, &ev),
                None => false,
            }
        }
    } else {
        true
    };
    if proceed {
        let req = crate::formdata::form_submission(page, form, submitter);
        if let Some(req) = req {
            page.navigate(req);
        }
    }
    page.end_submit(form);
}

/// `form.reset()`: fires `reset`, then restores default values.
pub fn reset_form(page: &Rc<PageDom>, form: NodeId) {
    if !fire_simple(page, form, "reset", true, true) {
        return;
    }
    let controls: Vec<NodeId> = {
        let dom = page.dom.borrow();
        dom.descendants(form)
            .map(|d| d.filter(|&n| dom.form_owner(n).ok().flatten() == Some(form)).collect())
            .unwrap_or_default()
    };
    let mut dom = page.dom.borrow_mut();
    for c in controls {
        if let Ok(st) = dom.control_state_mut(c) {
            *st = Default::default();
        }
    }
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
    match kind {
        KeyKind::KeyDown | KeyKind::RawKeyDown => {
            let ev = key_event(page, "keydown", k, 0);
            let ok = fire(page, tkey, &ev);
            if !ok || page.document() != doc || !accepts_keyboard(page) {
                return;
            }
            let Some(target) = key_target(page) else { return };
            if k.key == " " && space_clickable(&page.dom.borrow(), target) {
                // Browsers mark the control active on Space keydown and click
                // it on the matching keyup (Blink KeyboardClickableInputTypeView).
                page.space_target.set(Some(target));
            }
            if kind == KeyKind::KeyDown && !k.text.is_empty() {
                press_char(page, target, k);
            } else {
                default_key_action(page, target, k);
            }
        }
        KeyKind::Char => press_char(page, target, k),
        KeyKind::KeyUp => {
            let ev = key_event(page, "keyup", k, 0);
            let ok = fire(page, tkey, &ev);
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

fn press_char(page: &Rc<PageDom>, target: NodeId, k: &KeyInput) {
    let code = k.text.chars().next().map_or(0, |c| c as u32);
    let ev = key_event(page, "keypress", k, code);
    let Some(tkey) = key_of(page, target) else { return };
    let doc = page.document();
    if !fire(page, tkey, &ev) || page.document() != doc || !accepts_keyboard(page) {
        return;
    }
    let Some(target) = key_target(page) else { return };
    if k.modifiers.ctrl || k.modifiers.meta {
        return;
    }
    if k.text == "\r" || k.text == "\n" {
        enter_key(page, target);
    } else {
        insert_text_at(page, target, &k.text);
    }
}

/// Default actions for non-text keys on the focused element.
fn default_key_action(page: &Rc<PageDom>, target: NodeId, k: &KeyInput) {
    let editable = is_text_control(&page.dom.borrow(), target);
    match k.key.as_str() {
        "Backspace" if editable => delete(page, target, "deleteContentBackward", true),
        "Delete" if editable => delete(page, target, "deleteContentForward", false),
        "Enter" => enter_key(page, target),
        "Tab" => focus_next(page, target, k.modifiers.shift),
        "a" | "A" if editable && (k.modifiers.ctrl || k.modifiers.meta) => {
            let mut dom = page.dom.borrow_mut();
            if let Ok(v) = dom.value(target) {
                let _ = dom.set_selection_range(target, 0, lp_dom::dom::utf16_len(&v));
            }
        }
        "ArrowLeft" | "ArrowRight" | "Home" | "End" if editable => move_caret(page, target, &k.key, k.modifiers.shift),
        "ArrowUp" | "ArrowDown"
            if !(k.modifiers.alt || k.modifiers.ctrl || k.modifiers.meta)
                && vertical_caret_control(&page.dom.borrow(), target) =>
        {
            move_caret_vertically(page, target, k.key == "ArrowUp", k.modifiers.shift)
        }
        _ => {}
    }
}

/// Text controls whose caret ArrowUp/ArrowDown move. Number, date/time, range
/// and color inputs step or pick values instead, which is not modelled.
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
}

fn move_caret(page: &Rc<PageDom>, el: NodeId, key: &str, extend: bool) {
    let mut dom = page.dom.borrow_mut();
    let Ok((s, e)) = dom.selection(el) else { return };
    let len = dom.value(el).map(|v| lp_dom::dom::utf16_len(&v)).unwrap_or(0);
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
    let changed = {
        let mut dom = page.dom.borrow_mut();
        if backward {
            dom.delete_backward(el).unwrap_or(false)
        } else {
            dom.delete_forward(el).unwrap_or(false)
        }
    };
    if changed {
        fire_input_event(page, el, "input", input_type, None, false);
    }
}

fn enter_key(page: &Rc<PageDom>, target: NodeId) {
    let l = local(&page.dom.borrow(), target);
    match l.as_deref() {
        Some("textarea") => insert_text_at(page, target, "\n"),
        Some("input") => {
            // Implicit submission: click the default button, else submit.
            let (form, default_button) = {
                let dom = page.dom.borrow();
                let form = dom.form_owner(target).ok().flatten();
                let button = form.and_then(|f| {
                    let root = dom.root_node(f, false).ok()?;
                    dom.descendants(root).ok()?.find(|&n| {
                        dom.form_owner(n).ok().flatten() == Some(f)
                            && matches!(activation_for(&dom, n), Some(Activation::Submit { .. }))
                    })
                });
                (form, button)
            };
            match (form, default_button) {
                (_, Some(b)) => element_click(page, b),
                (Some(f), None) => submit_form(page, f, None, true),
                _ => {}
            }
        }
        Some("a") | Some("button") => element_click(page, target),
        _ => {}
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
    let changed = page.dom.borrow_mut().insert_text(el, text).unwrap_or(false);
    if changed {
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

/// Sequential focus navigation in tree order (tabindex >= 0 or naturally
/// focusable; positive tabindex ordering is not modelled).
fn focus_next(page: &Rc<PageDom>, from: NodeId, backward: bool) {
    let next = {
        let dom = page.dom.borrow();
        let doc = page.document();
        let Ok(all) = dom.descendants(doc) else { return };
        let candidates: Vec<NodeId> = all
            .filter(|&n| {
                dom.is_focusable(n).unwrap_or(false)
                    && dom
                        .get_attribute(n, "tabindex")
                        .ok()
                        .flatten()
                        .and_then(|t| t.trim().parse::<i32>().ok())
                        .unwrap_or(0)
                        >= 0
            })
            .collect();
        let pos = candidates.iter().position(|&c| c == from);
        match (pos, backward) {
            (Some(p), false) => candidates.get(p + 1).copied(),
            (Some(p), true) => p.checked_sub(1).and_then(|i| candidates.get(i).copied()),
            (None, false) => candidates.first().copied(),
            (None, true) => candidates.last().copied(),
        }
    };
    if let Some(n) = next {
        focus_element(page, n);
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
}

fn mouse_event(page: &Rc<PageDom>, iface: EventInterface, ty: &str, m: &MouseInput, cancelable: bool) -> EventData {
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
        f.button =
            if ty.ends_with("move") || ty.ends_with("over") || ty.ends_with("out") { 0 } else { m.button.dom_button() };
        f.buttons = m.buttons;
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
        }
        if iface == EventInterface::WheelEvent {
            f.delta_x = m.delta_x;
            f.delta_y = m.delta_y;
        }
    }
    ev
}

/// CDP `Input.dispatchMouseEvent`. Returns an error when hit testing is not
/// possible (no element at the point).
pub fn dispatch_mouse(page: &Rc<PageDom>, kind: MouseKind, m: &MouseInput) -> Result<(), String> {
    let target = crate::layout_api::hit_test(page, m.x, m.y).ok_or_else(|| "no element at point".to_owned())?;
    let Some(tkey) = key_of(page, target) else { return Err("stale target".into()) };
    match kind {
        MouseKind::Moved => {
            let prev = page.hovered();
            if prev != Some(target) {
                if let Some(p) = prev.and_then(|p| key_of(page, p)) {
                    fire(page, p, &mouse_event(page, EventInterface::PointerEvent, "pointerout", m, true));
                    fire(page, p, &mouse_event(page, EventInterface::MouseEvent, "mouseout", m, true));
                }
                fire(page, tkey, &mouse_event(page, EventInterface::PointerEvent, "pointerover", m, true));
                fire(page, tkey, &mouse_event(page, EventInterface::MouseEvent, "mouseover", m, true));
                let doc = page.document();
                let _ = page.dom.borrow_mut().set_hovered(doc, Some(target));
                page.set_hovered(Some(target));
            }
            fire(page, tkey, &mouse_event(page, EventInterface::PointerEvent, "pointermove", m, true));
            fire(page, tkey, &mouse_event(page, EventInterface::MouseEvent, "mousemove", m, true));
        }
        MouseKind::Pressed => {
            let pd = fire(page, tkey, &mouse_event(page, EventInterface::PointerEvent, "pointerdown", m, true));
            let md = !pd || fire(page, tkey, &mouse_event(page, EventInterface::MouseEvent, "mousedown", m, true));
            page.set_press_target(Some(target));
            // Default action of mousedown: focus the nearest focusable ancestor.
            if pd && md {
                let focus_target = {
                    let dom = page.dom.borrow();
                    let mut cur = Some(target);
                    let mut found = None;
                    while let Some(c) = cur {
                        if dom.is_focusable(c).unwrap_or(false) {
                            found = Some(c);
                            break;
                        }
                        cur = dom.parent_element(c).ok().flatten();
                    }
                    found
                };
                match focus_target {
                    Some(f) => focus_element(page, f),
                    None => {
                        let focused = page.dom.borrow().focused(page.document()).ok().flatten();
                        if let Some(f) = focused {
                            blur_element(page, f);
                        }
                    }
                }
            }
        }
        MouseKind::Released => {
            let pu = fire(page, tkey, &mouse_event(page, EventInterface::PointerEvent, "pointerup", m, true));
            if pu {
                fire(page, tkey, &mouse_event(page, EventInterface::MouseEvent, "mouseup", m, true));
            }
            let pressed = page.take_press_target();
            // click targets the common ancestor of press and release targets.
            let click_target = match pressed {
                Some(p) if p == target => Some(target),
                Some(p) => common_ancestor(&page.dom.borrow(), p, target),
                None => None,
            };
            if let Some(ct) = click_target {
                let name = match m.button {
                    MouseButton::Left | MouseButton::None => "click",
                    MouseButton::Right => "contextmenu",
                    _ => "auxclick",
                };
                let ev = mouse_event(page, EventInterface::PointerEvent, name, m, true);
                ev.state.borrow_mut().composed = true;
                dispatch_with_activation(page, ct, &ev);
                if name == "click" && m.click_count == 2 {
                    if let Some(k) = key_of(page, ct) {
                        fire(page, k, &mouse_event(page, EventInterface::MouseEvent, "dblclick", m, true));
                    }
                }
            }
        }
        MouseKind::Wheel => {
            fire(page, tkey, &mouse_event(page, EventInterface::WheelEvent, "wheel", m, true));
        }
    }
    Ok(())
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
