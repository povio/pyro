//! `HTMLInputElement.showPicker()` and `HTMLSelectElement.showPicker()` (HTML § 4.10.5.1.x,
//! § 4.10.7): the checks and the user-activation consumption of a same-origin call.
//!
//! The steps run in the spec's order: the mutability check (`InvalidStateError`, without
//! consuming activation), the transient-activation check (`NotAllowedError`), for a select the
//! being-rendered check (`NotSupportedError`), and then the activation is consumed.
//!
//! Not done here:
//! - The cross-origin `SecurityError` step (a document whose origin is not the top-level
//!   origin, except file and color inputs) is left out: the existing origin and access policy
//!   is kept as it was.
//! - "Show the picker, if applicable": this headless browser has no picker UI, so nothing is
//!   shown and no event, focus or value change follows. A successful call only consumes the
//!   activation.

use pyro_dom::forms::InputType;
use pyro_dom::NodeId;
use pyro_js::{JsError, JsResult, Scope, Value};

use crate::node::NodeData;
use crate::page::PageDom;
use crate::util::*;

/// `input.showPicker()`.
pub(crate) fn input<'s>(s: &Scope<'s>, n: &NodeData) -> JsResult<Value<'s>> {
    let page = n.page();
    let mutable = {
        let dom = page.dom.borrow();
        let ty = dom.input_type(n.id).js()?;
        let readonly = readonly_applies(ty) && dom.has_attribute(n.id, "readonly").unwrap_or(false);
        !dom.is_disabled(n.id).unwrap_or(false) && !readonly
    };
    if !mutable {
        return Err(JsError::dom(
            "InvalidStateError",
            "Failed to execute 'showPicker' on 'HTMLInputElement': HTMLInputElement::showPicker() cannot be used on immutable controls.",
        ));
    }
    require_activation(page, "HTMLInputElement")?;
    page.consume_transient_activation();
    Ok(s.undefined())
}

/// `select.showPicker()`.
pub(crate) fn select<'s>(s: &Scope<'s>, n: &NodeData) -> JsResult<Value<'s>> {
    let page = n.page();
    // `readonly` does not apply to a select: only a disabled one is immutable.
    if page.dom.borrow().is_disabled(n.id).unwrap_or(false) {
        return Err(JsError::dom(
            "InvalidStateError",
            "Failed to execute 'showPicker' on 'HTMLSelectElement': HTMLSelectElement::showPicker() cannot be used on immutable controls.",
        ));
    }
    require_activation(page, "HTMLSelectElement")?;
    if !being_rendered(page, n.id) {
        return Err(JsError::dom(
            "NotSupportedError",
            "Failed to execute 'showPicker' on 'HTMLSelectElement': HTMLSelectElement::showPicker() requires the select is rendered.",
        ));
    }
    page.consume_transient_activation();
    // A customizable select shows its own picker (a native one has none here).
    if crate::popover::has_select_picker(page, n.id) {
        crate::popover::show_select_picker(page, n.id);
    }
    Ok(s.undefined())
}

/// The `readonly` attribute applies to the text, search, URL, telephone, email, password,
/// date, month, week, time, local date and time, and number states.
fn readonly_applies(ty: InputType) -> bool {
    use InputType::*;
    matches!(ty, Text | Search | Url | Tel | Email | Password | Date | Month | Week | Time | DateTimeLocal | Number)
}

/// "If this's relevant global object does not have transient activation, throw a
/// `NotAllowedError`."
fn require_activation(page: &PageDom, interface: &str) -> JsResult<()> {
    if page.has_transient_activation() {
        return Ok(());
    }
    Err(JsError::dom(
        "NotAllowedError",
        format!("Failed to execute 'showPicker' on '{interface}': {interface}::showPicker() requires a user gesture."),
    ))
}

/// HTML "being rendered": the element has a box (not in a `display: none` subtree), and, as in
/// Chromium, no ancestor skips its contents with `content-visibility: hidden`.
fn being_rendered(page: &PageDom, el: NodeId) -> bool {
    let layout = page.layout();
    let dom = page.dom.borrow();
    if !dom.is_connected(el).unwrap_or(false) || !layout.is_visible(&dom, el) {
        return false;
    }
    let mut ancestor = dom.parent_element(el).ok().flatten();
    while let Some(a) = ancestor {
        if layout.computed_style_value(&dom, a, "content-visibility") == "hidden" {
            return false;
        }
        ancestor = dom.parent_element(a).ok().flatten();
    }
    true
}
