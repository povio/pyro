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
    crate::editing::caret_on_focus(page, el);
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
    Checkbox {
        el: NodeId,
        old: bool,
    },
    Radio {
        el: NodeId,
        previously_checked: Vec<NodeId>,
        was_checked: bool,
    },
    Submit {
        el: NodeId,
    },
    Reset {
        el: NodeId,
    },
    Link {
        el: NodeId,
    },
    Label {
        el: NodeId,
    },
    Summary {
        el: NodeId,
    },
    /// `<input type=file>`: shows the file picker.
    FilePicker {
        el: NodeId,
    },
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
            InputType::File => Some(Activation::FilePicker { el }),
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
                    let trusted = ev.state.borrow().is_trusted;
                    click_with_trust(page, c, trusted);
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
        Activation::FilePicker { el } => {
            if let Some(h) = page.hooks() {
                let multiple = page.dom.borrow().has_attribute(el, "multiple").unwrap_or(false);
                h.file_chooser(el, multiple);
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
    click_with_trust(page, el, false);
}

/// A click whose `isTrusted` follows the event that caused it (a label
/// forwarding a user's click to its control, as Chrome does).
fn click_with_trust(page: &Rc<PageDom>, el: NodeId, trusted: bool) {
    if page.dom.borrow().is_disabled(el).unwrap_or(false) {
        return;
    }
    if !page.begin_click(el) {
        return; // click in progress flag
    }
    let ev = EventCore::new(page, EventInterface::PointerEvent, "click", true, true, true);
    ev.fields.borrow_mut().pointer_id = -1;
    ev.state.borrow_mut().is_trusted = trusted;
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
    let (url, target, opener) = {
        let dom = page.dom.borrow();
        let Some(href) = dom.get_attribute(el, "href").ok().flatten() else { return };
        let url = crate::html::resolve_url(&dom, el, href);
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
                    match ::url::Url::parse(&url).ok().as_ref().and_then(lp_net::data_url::parse) {
                        Some((mime, bytes)) => Some((bytes, mime)),
                        None => return,
                    }
                } else {
                    None
                };
                drop(dom);
                page.download(crate::page::DownloadRequest { url, filename, data });
                return;
            }
        }
        // HTML: a link that targets a new browsing context is noopener unless rel=opener.
        let rel = dom.get_attribute(el, "rel").ok().flatten().unwrap_or_default().to_ascii_lowercase();
        let rel: Vec<&str> = rel.split_ascii_whitespace().collect();
        let opener = rel.contains(&"opener") && !rel.contains(&"noopener") && !rel.contains(&"noreferrer");
        (url, dom.get_attribute(el, "target").ok().flatten().map(str::to_owned), opener)
    };
    page.navigate(crate::page::NavigationRequest {
        url,
        target,
        method: "GET".into(),
        body: None,
        content_type: None,
        opener,
    });
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
    if let Some(host) = editing_host_of(page, target) {
        if k.text == "\r" || k.text == "\n" {
            crate::editing::insert_paragraph(page, host);
        } else {
            crate::editing::insert_text(page, host, &k.text);
        }
        return;
    }
    if k.text == "\r" || k.text == "\n" {
        enter_key(page, target);
    } else {
        insert_text_at(page, target, &k.text);
    }
}

/// The contenteditable host the focused element edits, if any.
fn editing_host_of(page: &PageDom, target: NodeId) -> Option<NodeId> {
    crate::editing::editing_host(&page.dom.borrow(), target)
}

/// Default actions for keys in a contenteditable host.
fn editing_key_action(page: &Rc<PageDom>, host: NodeId, k: &KeyInput) {
    use crate::editing;
    match k.key.as_str() {
        "Backspace" => editing::delete(page, host, true),
        "Delete" => editing::delete(page, host, false),
        "Enter" => editing::insert_paragraph(page, host),
        "Tab" => focus_next(page, host, k.modifiers.shift),
        "a" | "A" if k.modifiers.ctrl || k.modifiers.meta => editing::select_all(page, host),
        "ArrowLeft" | "ArrowRight" | "Home" | "End" => editing::move_caret(page, host, &k.key, k.modifiers.shift),
        _ => {}
    }
}

/// Default actions for non-text keys on the focused element.
fn default_key_action(page: &Rc<PageDom>, target: NodeId, k: &KeyInput) {
    if let Some(host) = editing_host_of(page, target) {
        editing_key_action(page, host, k);
        return;
    }
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
        // Outside editable content Home/End scroll the document (Chrome's
        // scrollToBeginningOfDocument / scrollToEndOfDocument).
        "Home" | "End" if !(k.modifiers.alt || k.modifiers.ctrl || k.modifiers.meta || k.modifiers.shift) => {
            let top = if k.key == "Home" { 0 } else { i32::MAX };
            let moved = page.layout.borrow_mut().window_scroll_to(&page.dom.borrow(), None, Some(top));
            if moved {
                crate::layout_api::fire_scroll(page, None);
            }
        }
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

/// Inclusive ancestors of `node` up to and including its document, innermost
/// first; empty when it is no longer in the page's document.
fn hover_chain(page: &PageDom, node: NodeId) -> Vec<NodeId> {
    let dom = page.dom.borrow();
    let mut chain = Vec::new();
    let mut cur = Some(node);
    while let Some(c) = cur {
        chain.push(c);
        cur = dom.parent(c).ok().flatten();
    }
    if chain.last() != Some(&page.document()) {
        chain.clear();
    }
    chain
}

/// Nodes that receive leave (innermost first) and enter (innermost first)
/// when the hovered node changes from `prev` to `next`: each chain up to,
/// excluding, their common ancestor.
fn boundary_chains(page: &PageDom, prev: Option<NodeId>, next: NodeId) -> (Vec<NodeId>, Vec<NodeId>) {
    let mut left = prev.map(|p| hover_chain(page, p)).unwrap_or_default();
    let mut entered = hover_chain(page, next);
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
fn dispatch_boundary_events(page: &Rc<PageDom>, prev: Option<NodeId>, target: NodeId, tkey: TargetKey, m: &MouseInput) {
    let prev_connected = prev.is_some_and(|p| page.dom.borrow().is_connected(p).unwrap_or(false));
    let (left, entered) = boundary_chains(page, prev, target);
    let prev_key = prev.and_then(|p| key_of(page, p));
    let related = |ev: EventData, key: Option<TargetKey>| {
        ev.fields.borrow_mut().related_target = key;
        ev
    };
    for (iface, prefix) in [(EventInterface::PointerEvent, "pointer"), (EventInterface::MouseEvent, "mouse")] {
        if let Some(p) = prev_key.filter(|_| prev_connected) {
            fire(page, p, &related(mouse_event(page, iface, &format!("{prefix}out"), m, true), Some(tkey)));
        }
        for &n in &left {
            if let Some(k) = key_of(page, n) {
                fire(page, k, &related(mouse_event(page, iface, &format!("{prefix}leave"), m, false), Some(tkey)));
            }
        }
        fire(page, tkey, &related(mouse_event(page, iface, &format!("{prefix}over"), m, true), prev_key));
        for &n in entered.iter().rev() {
            if let Some(k) = key_of(page, n) {
                fire(page, k, &related(mouse_event(page, iface, &format!("{prefix}enter"), m, false), prev_key));
            }
        }
    }
    let doc = page.document();
    let _ = page.dom.borrow_mut().set_hovered(doc, Some(target));
    page.set_hovered(Some(target));
}

/// Delay of the hover update after the hovered element is removed: Chromium
/// updates hover for content that changed under a stationary pointer on its
/// fake-mouse-move timer (~100 ms, measured 110 ms), not in the mutating task.
/// Frameworks rely on that: React detaches a removed node's fiber in passive
/// effects after the commit, and only then treats an over event whose
/// relatedTarget is that node as an enter (the @povio/ui QueryAutocomplete
/// static-to-live handoff in TOS needs this onMouseEnter).
const HOVER_UPDATE_DELAY: std::time::Duration = std::time::Duration::from_millis(100);

/// After a task removed the hovered element, the element then under the
/// stationary pointer gets boundary events (no out/leave to the removed
/// node, relatedTarget = the removed node, no move events) once
/// [`HOVER_UPDATE_DELAY`] has passed, as Chromium's hover update does.
/// Layout-only changes under the pointer are not tracked.
pub(crate) fn update_hover_after_task(page: &Rc<PageDom>) {
    if page.hover_update_queued.get() || !hovered_was_removed(page) || page.pointer.borrow().is_none() {
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

fn hovered_was_removed(page: &PageDom) -> bool {
    page.hovered_raw().is_some_and(|h| !page.dom.borrow().is_connected(h).unwrap_or(false))
}

fn update_hover_now(page: &Rc<PageDom>) {
    // A real mouse move in the meantime already updated hover.
    if !hovered_was_removed(page) {
        return;
    }
    let Some(prev) = page.hovered_raw() else { return };
    let Some(m) = page.pointer.borrow().clone() else { return };
    let Some(target) = pointer_target(page, m.x, m.y, true) else { return };
    let Some(tkey) = key_of(page, target) else { return };
    dispatch_boundary_events(page, Some(prev), target, tkey, &m);
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

/// Native text selection, as Chromium's mouse selection: a primary press
/// (not canceled, outside controls, editable content and drag sources) on
/// selectable text fires `selectstart` at that text and, unless canceled,
/// collapses the selection to the character boundary under the pointer. A
/// press beside the text of a focusable element only focuses it.
fn text_select_press(page: &Rc<PageDom>, target: NodeId, m: &MouseInput) {
    let caret = {
        let dom = page.dom.borrow();
        let layout = page.layout.borrow();
        if !press_selects_text(&dom, target) || layout.user_select_none(&dom, target) {
            return;
        }
        layout.caret_from_point(&dom, m.x, m.y, &|n| !layout.user_select_none(&dom, n))
    };
    let Some((node, offset, on_text)) = caret else { return };
    if !on_text && page.dom.borrow().is_focusable(target).unwrap_or(false) {
        return;
    }
    let started = fire_simple(page, node, "selectstart", true, true);
    if started {
        crate::editing::set_selection(page, (node, offset), (node, offset));
    }
    page.text_drag.set(Some(((node, offset), started)));
}

/// A move with the primary button down extends the selection to the
/// character boundary under the pointer; over `user-select: none` content
/// it stops at the edge of the selectable text before (or after) it.
fn text_select_move(page: &Rc<PageDom>, m: &MouseInput) {
    let Some((anchor, started)) = page.text_drag.get() else { return };
    if !started {
        if !fire_simple(page, anchor.0, "selectstart", true, true) {
            return;
        }
        crate::editing::set_selection(page, anchor, anchor);
        page.text_drag.set(Some((anchor, true)));
    }
    let focus = {
        let dom = page.dom.borrow();
        let layout = page.layout.borrow();
        match layout.caret_from_point(&dom, m.x, m.y, &|_| true) {
            Some((n, offset, _)) if !layout.user_select_none(&dom, n) => Some((n, offset)),
            Some((n, _, _)) => {
                let after = crate::editing::compare(page, (n, 0), anchor) > 0;
                let flow = layout.flow(&dom);
                let runs = flow.text_runs.iter().filter(|r| !layout.user_select_none(&dom, r.node));
                if after {
                    runs.filter(|r| crate::editing::compare(page, (r.node, r.end), (n, 0)) < 0)
                        .map(|r| (r.node, r.end))
                        .max_by(|a, b| crate::editing::compare(page, *a, *b).cmp(&0))
                } else {
                    runs.filter(|r| crate::editing::compare(page, (r.node, r.start), (n, 0)) > 0)
                        .map(|r| (r.node, r.start))
                        .min_by(|a, b| crate::editing::compare(page, *a, *b).cmp(&0))
                }
            }
            None => None,
        }
    };
    if let Some(f) = focus {
        crate::editing::set_selection(page, anchor, f);
    }
}

pub fn dispatch_mouse_intercepting_drags(
    page: &Rc<PageDom>,
    kind: MouseKind,
    m: &MouseInput,
    intercept_drags: bool,
) -> Result<Option<DragData>, String> {
    // Empty viewport space targets the root element, as Chromium hit-tests
    // it: for moves and wheels always, for presses and releases in flow
    // geometry (the synthetic layout's gaps are not real empty space).
    let flow = page.layout.borrow().geometry() == lp_dom::layout::Geometry::Flow;
    let target = pointer_target(page, m.x, m.y, flow || matches!(kind, MouseKind::Wheel | MouseKind::Moved))
        .ok_or_else(|| "no element at point".to_owned())?;
    let Some(tkey) = key_of(page, target) else { return Err("stale target".into()) };
    match kind {
        MouseKind::Moved => {
            *page.pointer.borrow_mut() = Some(m.clone());
            let prev = page.hovered();
            if prev != Some(target) {
                dispatch_boundary_events(page, prev, target, tkey, m);
            }
            fire(page, tkey, &mouse_event(page, EventInterface::PointerEvent, "pointermove", m, true));
            fire(page, tkey, &mouse_event(page, EventInterface::MouseEvent, "mousemove", m, true));
            if m.buttons & 1 != 0 {
                text_select_move(page, m);
            }
            if intercept_drags {
                return Ok(start_drag(page, target, m));
            }
        }
        MouseKind::Pressed => {
            let pd = fire(page, tkey, &mouse_event(page, EventInterface::PointerEvent, "pointerdown", m, true));
            let md = !pd || fire(page, tkey, &mouse_event(page, EventInterface::MouseEvent, "mousedown", m, true));
            page.set_press_target(Some(target));
            let place_caret = m.button == MouseButton::Left && pd && md;
            // A canceled mousedown prevents dragging, as in Chrome.
            let source =
                (m.button == MouseButton::Left && pd && md).then(|| drag_source(&page.dom.borrow(), target)).flatten();
            page.drag_candidate.set(source.map(|s| (s, m.x, m.y)));
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
                    // The pointer already hit visible content. Revealing its
                    // focusable ancestor can move that content between press
                    // and release, turning the click into an ancestor click.
                    // Explicit script focus keeps its own scroll policy.
                    Some(f) => focus_element_with_options(page, f, true),
                    None => {
                        let focused = page.dom.borrow().focused(page.document()).ok().flatten();
                        if let Some(f) = focused {
                            blur_element(page, f);
                        }
                    }
                }
            }
            if place_caret {
                crate::editing::caret_on_press(page, target);
            }
            page.text_drag.set(None);
            if place_caret && source.is_none() {
                text_select_press(page, target, m);
            }
        }
        MouseKind::Released => {
            page.drag_candidate.set(None);
            page.text_drag.set(None);
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
            // Default action of an uncanceled wheel (frame/user_input.zig:
            // 571-615): each axis scrolls the nearest container that can still
            // move, else the viewport, then `scroll` fires as a task.
            let doc = page.document();
            if !fire(page, tkey, &mouse_event(page, EventInterface::WheelEvent, "wheel", m, true))
                || page.is_disposed()
                || page.document() != doc
            {
                return Ok(None);
            }
            let moved = page.layout.borrow_mut().wheel_scroll(&page.dom.borrow(), target, m.delta_x, m.delta_y);
            for scrolled in moved {
                let node = match scrolled {
                    lp_dom::layout::ScrollTarget::Viewport => None,
                    lp_dom::layout::ScrollTarget::Container(node) => Some(node),
                };
                crate::layout_api::fire_scroll(page, node);
            }
        }
    }
    Ok(None)
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

/// Nearest ancestor-or-self that is draggable: `draggable=true`, or a link
/// with an href unless it says `draggable=false`. Images are not dragged:
/// Chrome drags only decoded images and the engine decodes none.
fn drag_source(dom: &Dom, target: NodeId) -> Option<NodeId> {
    let mut cur = Some(target);
    while let Some(c) = cur {
        if let Ok(e) = dom.element(c) {
            let draggable = e.attr("draggable");
            if draggable.is_some_and(|d| d.eq_ignore_ascii_case("true")) {
                return Some(c);
            }
            if e.is_html()
                && *e.local_name() == *"a"
                && e.attr("href").is_some()
                && !draggable.is_some_and(|d| d.eq_ignore_ascii_case("false"))
            {
                return Some(c);
            }
        }
        cur = dom.parent(c).ok().flatten();
    }
    None
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

/// The operation a target accepting the drag gets: its dropEffect when the
/// source allows it, else Chrome's default (copy, then move, then link).
fn accepted_operation(drop_effect: &str, mask: i64) -> Option<&'static str> {
    let ops = allowed_ops(mask);
    let named = [("copy", OP_COPY), ("link", OP_LINK), ("move", OP_MOVE)];
    if let Some(&(name, bit)) = named.iter().find(|(name, _)| *name == drop_effect) {
        return (ops & bit != 0).then_some(name);
    }
    [("copy", OP_COPY), ("move", OP_MOVE), ("link", OP_LINK)]
        .into_iter()
        .find(|(_, bit)| ops & bit != 0)
        .map(|(n, _)| n)
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
    let title = dom.inner_text(page.layout.borrow().style(), link).unwrap_or_default();
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
    let defaults = link_items(page, source);
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
fn update_drag(page: &Rc<PageDom>, m: &MouseInput, data: &DragData) -> Result<(), String> {
    let target = pointer_target(page, m.x, m.y, true);
    let (source, current, mut only_dragover) = {
        let drag = page.drag.borrow();
        let s = drag.as_ref().ok_or("no drag in progress")?;
        (s.source.clone(), s.target, s.only_dragover)
    };
    let transfer =
        NativeTransfer::new(page, &target_strings(data), effect_allowed_of(data.drag_operations_mask), Mode::Protected)
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
        if accepted { accepted_operation(&transfer.drop_effect(), data.drag_operations_mask) } else { None };
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
        DragKind::Enter | DragKind::Over => update_drag(page, &m, data),
        DragKind::Drop => {
            if page.drag.borrow().as_ref().is_some_and(|s| s.only_dragover) {
                update_drag(page, &m, data)?;
            }
            let Some(session) = page.drag.borrow_mut().take() else { return Ok(()) };
            match (session.operation, session.target) {
                (Some(operation), Some(target)) => {
                    let transfer = NativeTransfer::new(
                        page,
                        &target_strings(data),
                        effect_allowed_of(data.drag_operations_mask),
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
                        if let Some(transfer) = NativeTransfer::new(
                            page,
                            &target_strings(data),
                            effect_allowed_of(data.drag_operations_mask),
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
