//! Constraint validation API (ValidityState, checkValidity/reportValidity).

use std::rc::Rc;

use lp_dom::forms::Validity;
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
    if v.type_mismatch {
        return "Please enter a valid value.".into();
    }
    if v.range_underflow || v.range_overflow {
        return "Value is out of range.".into();
    }
    if v.too_long || v.too_short {
        return "Please match the requested length.".into();
    }
    String::new()
}

pub(crate) fn set_custom(page: &PageDom, id: NodeId, msg: String) {
    let _ = page.dom.borrow_mut().set_custom_validity(id, &msg);
}

/// `checkValidity()` / `reportValidity()`: fires `invalid` when invalid.
pub(crate) fn check(page: &Rc<PageDom>, id: NodeId, _report: bool) -> bool {
    let valid = {
        let dom = page.dom.borrow();
        !dom.will_validate(id).unwrap_or(false) || dom.validity(id).map(|v| v.valid()).unwrap_or(true)
    };
    if !valid {
        crate::input::fire_simple(page, id, "invalid", false, true);
    }
    valid
}

/// Form-level check over its submittable controls.
pub(crate) fn check_form(page: &Rc<PageDom>, form: NodeId, report: bool) -> bool {
    let controls: Vec<NodeId> = {
        let dom = page.dom.borrow();
        let Ok(root) = dom.root_node(form, false) else { return true };
        dom.descendants(root)
            .map(|d| d.filter(|&n| dom.form_owner(n).ok().flatten() == Some(form)).collect())
            .unwrap_or_default()
    };
    let mut ok = true;
    for c in controls {
        ok &= check(page, c, report);
    }
    ok
}

pub(crate) fn register(engine: &lp_js::Engine) {
    engine.register::<ValidityState>();
}
