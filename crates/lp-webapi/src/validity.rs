//! Constraint validation API (ValidityState, checkValidity/reportValidity).

use std::rc::Rc;

use lp_dom::forms::{InputType, Validity};
use lp_dom::NodeId;
use lp_js::{ClassDef, JsClass, JsResult, NoParent, Scope, Value};

use crate::node::NodeData;
use crate::page::PageDom;
use crate::util::*;

pub struct ValidityState;
impl JsClass for ValidityState {
    const NAME: &'static str = "ValidityState";
    type Data = Validity;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.getter("valueMissing", |s, v| Ok(s.boolean(v.value_missing)));
        def.getter("typeMismatch", |s, v| Ok(s.boolean(v.type_mismatch)));
        def.getter("patternMismatch", |s, v| Ok(s.boolean(v.pattern_mismatch)));
        def.getter("tooLong", |s, v| Ok(s.boolean(v.too_long)));
        def.getter("tooShort", |s, v| Ok(s.boolean(v.too_short)));
        def.getter("rangeUnderflow", |s, v| Ok(s.boolean(v.range_underflow)));
        def.getter("rangeOverflow", |s, v| Ok(s.boolean(v.range_overflow)));
        def.getter("stepMismatch", |s, v| Ok(s.boolean(v.step_mismatch)));
        def.getter("badInput", |s, v| Ok(s.boolean(v.bad_input)));
        def.getter("customError", |s, v| Ok(s.boolean(v.custom_error)));
        def.getter("valid", |s, v| Ok(s.boolean(v.valid())));
    }
}

/// Snapshot of the element's validity (attributes are not live).
pub(crate) fn validity_state<'s>(s: &Scope<'s>, n: &NodeData) -> JsResult<Value<'s>> {
    let v = n.page().dom.borrow().validity(n.id).js()?;
    Ok(s.wrap::<ValidityState>(v, None)?.value())
}

pub(crate) fn will_validate(page: &PageDom, id: NodeId) -> bool {
    page.dom.borrow().will_validate(id).unwrap_or(false)
}

/// Chrome-like validation message for the first failing constraint.
pub(crate) fn message(page: &PageDom, id: NodeId) -> String {
    let dom = page.dom.borrow();
    let Ok(v) = dom.validity(id) else { return String::new() };
    if v.custom_error {
        return dom.custom_validity(id).ok().flatten().unwrap_or("").to_owned();
    }
    if v.value_missing {
        return "Please fill out this field.".into();
    }
    let ty = dom.input_type(id).ok();
    if v.type_mismatch {
        let value = dom.value(id).unwrap_or_default();
        return match ty {
            Some(InputType::Email) => email_message(&value),
            Some(InputType::Url) => "Please enter a URL.".into(),
            _ => "Please enter a valid value.".into(),
        };
    }
    if v.bad_input {
        return match ty {
            Some(InputType::Number) => "Please enter a number.".into(),
            _ => "Please enter a valid value.".into(),
        };
    }
    if v.pattern_mismatch {
        return "Please match the requested format.".into();
    }
    let numeric = matches!(ty, Some(InputType::Number | InputType::Range));
    let attr = |name: &str| dom.get_attribute(id, name).ok().flatten().unwrap_or("").to_owned();
    if v.range_underflow {
        return if numeric {
            format!("Value must be greater than or equal to {}.", attr("min"))
        } else {
            format!("Value must be {} or later.", attr("min"))
        };
    }
    if v.range_overflow {
        return if numeric {
            format!("Value must be less than or equal to {}.", attr("max"))
        } else {
            format!("Value must be {} or earlier.", attr("max"))
        };
    }
    if v.step_mismatch {
        return "Please enter a valid value.".into();
    }
    if v.too_long || v.too_short {
        let len = lp_dom::dom::utf16_len(&dom.value(id).unwrap_or_default());
        return if v.too_short {
            format!(
                "Please lengthen this text to {} characters or more (you are currently using {len} characters).",
                attr("minlength").trim()
            )
        } else {
            format!(
                "Please shorten this text to {} characters or less (you are currently using {len} characters).",
                attr("maxlength").trim()
            )
        };
    }
    String::new()
}

/// Chrome's message for an invalid e-mail address.
fn email_message(value: &str) -> String {
    match value.split_once('@') {
        None => format!("Please include an '@' in the email address. '{value}' is missing an '@'."),
        Some(("", _)) => format!("Please enter a part followed by '@'. '{value}' is incomplete."),
        Some((_, "")) => format!("Please enter a part following '@'. '{value}' is incomplete."),
        Some(_) => "Please enter an email address.".into(),
    }
}

pub(crate) fn set_custom(page: &PageDom, id: NodeId, msg: String) {
    let _ = page.dom.borrow_mut().set_custom_validity(id, &msg);
}

/// Statically validates `id`: Some(handled) when invalid, after firing
/// `invalid` (handled: a listener canceled it).
fn check_one(page: &Rc<PageDom>, id: NodeId) -> Option<bool> {
    let valid = {
        let dom = page.dom.borrow();
        !dom.will_validate(id).unwrap_or(false) || dom.validity(id).map(|v| v.valid()).unwrap_or(true)
    };
    (!valid).then(|| !crate::input::fire_simple(page, id, "invalid", false, true))
}

/// `checkValidity()` / `reportValidity()`: fires `invalid` when invalid;
/// reporting focuses the control unless the event was canceled.
pub(crate) fn check(page: &Rc<PageDom>, id: NodeId, report: bool) -> bool {
    match check_one(page, id) {
        None => true,
        Some(handled) => {
            if report && !handled {
                crate::input::focus_element(page, id);
            }
            false
        }
    }
}

/// Form-level check over its submittable controls; reporting (interactive
/// validation) focuses the first invalid control whose `invalid` event was
/// not canceled, as Chrome does.
pub(crate) fn check_form(page: &Rc<PageDom>, form: NodeId, report: bool) -> bool {
    let controls: Vec<NodeId> = {
        let dom = page.dom.borrow();
        let Ok(root) = dom.root_node(form, false) else { return true };
        dom.descendants(root)
            .map(|d| d.filter(|&n| dom.form_owner(n).ok().flatten() == Some(form)).collect())
            .unwrap_or_default()
    };
    let mut ok = true;
    let mut first_unhandled = None;
    for c in controls {
        if let Some(handled) = check_one(page, c) {
            ok = false;
            if !handled && first_unhandled.is_none() {
                first_unhandled = Some(c);
            }
        }
    }
    if let Some(c) = first_unhandled.filter(|_| report) {
        crate::input::focus_element(page, c);
    }
    ok
}

pub(crate) fn register(engine: &lp_js::Engine) {
    engine.register::<ValidityState>();
}
