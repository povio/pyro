//! `ElementInternals` (`attachInternals()`) and `CustomStateSet`: the form
//! participation of form-associated custom elements (submission value,
//! form owner, labels, constraint validation) and custom states for
//! `:state()`. Their state lives in the tree (`pyro_dom` internals), so the
//! form submission, `:state()` and `:invalid` read it where they read the
//! native controls'.
//!
//! Both interfaces are host classes: their prototypes are made on a
//! realm's first use, and nothing exists for an element until its
//! constructor calls `attachInternals()`.

use std::cell::RefCell;
use std::rc::Rc;

use pyro_dom::forms::Validity;
use pyro_dom::NodeId;
use pyro_js::{ClassDef, IdentityKey, JsClass, JsError, JsResult, NoParent, Object, Scope, Value};

use crate::collections::NodeHold;
use crate::file::{File, FileEntry};
use crate::form_data::FormData;
use crate::formdata::EntryValue;
use crate::node::node_value;
use crate::page::PageDom;
use crate::util::*;

/// A form-associated custom element's submission value (`setFormValue`),
/// kept in the tree with the element.
pub(crate) enum FaceValue {
    Text(String),
    File(FileEntry),
    Entries(Vec<(String, EntryValue)>),
}

/// The entries a form-associated custom element contributes to its form's
/// entry list (HTML "constructing the entry list"): its value's entries,
/// or its value under its `name`.
pub(crate) fn face_entries(dom: &pyro_dom::Dom, el: NodeId, out: &mut Vec<(String, EntryValue)>) {
    let Some(value) = dom.face_value(el).and_then(|v| v.downcast_ref::<FaceValue>()) else { return };
    match value {
        FaceValue::Entries(entries) => out.extend(entries.iter().cloned()),
        FaceValue::Text(t) => {
            if let Some(name) = dom.element(el).ok().and_then(|e| e.attr("name")).filter(|n| !n.is_empty()) {
                out.push((name.to_owned(), EntryValue::Text(t.clone())));
            }
        }
        FaceValue::File(f) => {
            if let Some(name) = dom.element(el).ok().and_then(|e| e.attr("name")).filter(|n| !n.is_empty()) {
                out.push((name.to_owned(), EntryValue::File(f.clone())));
            }
        }
    }
}

pub struct InternalsData {
    /// The target element, held strongly.
    el: NodeHold,
    /// ARIAMixin values set on the internals (default semantics; they are
    /// not reflected to content attributes).
    aria: RefCell<Vec<(&'static str, String)>>,
}

impl InternalsData {
    fn page(&self) -> &Rc<PageDom> {
        &self.el.page
    }
    fn id(&self) -> NodeId {
        self.el.id
    }
    /// The checks of the members only form-associated custom elements have.
    fn face(&self, what: &str) -> JsResult<NodeId> {
        if self.page().dom.borrow().is_form_associated(self.id()) {
            Ok(self.id())
        } else {
            Err(JsError::dom(
                "NotSupportedError",
                format!("Failed to execute '{what}' on 'ElementInternals': The target element is not a form-associated custom element."),
            ))
        }
    }
}

/// ARIAMixin string attributes (`role` and `aria*`), stored per internals.
const ARIA: &[&str] = &[
    "role",
    "ariaAtomic",
    "ariaAutoComplete",
    "ariaBrailleLabel",
    "ariaBrailleRoleDescription",
    "ariaBusy",
    "ariaChecked",
    "ariaColCount",
    "ariaColIndex",
    "ariaColIndexText",
    "ariaColSpan",
    "ariaCurrent",
    "ariaDescription",
    "ariaDisabled",
    "ariaExpanded",
    "ariaHasPopup",
    "ariaHidden",
    "ariaInvalid",
    "ariaKeyShortcuts",
    "ariaLabel",
    "ariaLevel",
    "ariaLive",
    "ariaModal",
    "ariaMultiLine",
    "ariaMultiSelectable",
    "ariaOrientation",
    "ariaPlaceholder",
    "ariaPosInSet",
    "ariaPressed",
    "ariaReadOnly",
    "ariaRelevant",
    "ariaRequired",
    "ariaRoleDescription",
    "ariaRowCount",
    "ariaRowIndex",
    "ariaRowIndexText",
    "ariaRowSpan",
    "ariaSelected",
    "ariaSetSize",
    "ariaSort",
    "ariaValueMax",
    "ariaValueMin",
    "ariaValueNow",
    "ariaValueText",
];

macro_rules! aria_accessors {
    ($def:expr, $($i:literal),*) => {
        $(
            $def.accessor(
                ARIA[$i],
                |s, d| {
                    let aria = d.aria.borrow();
                    Ok(aria.iter().find(|(k, _)| *k == ARIA[$i]).map_or(s.null(), |(_, v)| s.string(v)))
                },
                |_s, d, v| {
                    let value = opt_dom_string(v)?;
                    let mut aria = d.aria.borrow_mut();
                    aria.retain(|(k, _)| *k != ARIA[$i]);
                    if let Some(v) = value {
                        aria.push((ARIA[$i], v));
                    }
                    Ok(())
                },
            );
        )*
    };
}

pub struct ElementInternals;
impl JsClass for ElementInternals {
    const NAME: &'static str = "ElementInternals";
    type Data = InternalsData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.getter("shadowRoot", |s, d| {
            // Only a root attached while the host was custom (not one it had
            // before its upgrade, which may be closed).
            let root = {
                let dom = d.page().dom.borrow();
                dom.shadow_root(d.id()).unwrap_or(None).filter(|&r| {
                    dom.shadow_root_flags(r).is_ok_and(|f| f & pyro_dom::shadow_flags::AVAILABLE_TO_INTERNALS != 0)
                })
            };
            node_value(s, d.page(), root)
        });
        def.method("setFormValue", 1, |s, d, a| {
            let el = d.face("setFormValue")?;
            a.require(1, "ElementInternals.setFormValue")?;
            let value = form_value(s, a.get(0))?;
            d.page().dom.borrow_mut().set_face_value(el, value.map(|v| Box::new(v) as Box<dyn std::any::Any>)).js()?;
            Ok(s.undefined())
        });
        def.getter("form", |s, d| {
            let el = d.face("form")?;
            let dom = d.page().dom.borrow();
            let form = dom.face_form(el).and_then(|f| dom.retarget_node(f, dom.root_node(el, false).ok()?));
            drop(dom);
            node_value(s, d.page(), form)
        });
        def.method("setValidity", 0, |s, d, a| {
            let el = d.face("setValidity")?;
            let flags = validity_flags(a.get(0))?;
            let message = if a.get(1).is_undefined() { None } else { Some(dom_string(a.get(1))?) };
            let anchor = crate::node::opt_node_arg(s, a.get(2), "ElementInternals.setValidity")?;
            // `optional HTMLElement anchor`: another kind of node (or a non-HTML element) fails the conversion.
            if let Some(anchor) = anchor {
                if !d.page().dom.borrow().element(anchor).is_ok_and(|e| e.is_html()) {
                    return Err(JsError::Type("Failed to execute 'setValidity' on 'ElementInternals': parameter 3 is not of type 'HTMLElement'.".into()));
                }
            }
            if !flags.valid() && message.as_deref().unwrap_or("").is_empty() {
                return Err(JsError::Type(
                    "Failed to execute 'setValidity' on 'ElementInternals': The second argument should not be empty if one or more flags in the first argument are true.".into(),
                ));
            }
            if let Some(anchor) = anchor {
                let dom = d.page().dom.borrow();
                // Chrome accepts the element itself.
                let inside = anchor == el || dom.is_shadow_including_descendant(el, anchor);
                if !inside {
                    return Err(JsError::dom(
                        "NotFoundError",
                        "Failed to execute 'setValidity' on 'ElementInternals': The anchor element is not a shadow-including descendant of the target element.",
                    ));
                }
            }
            let (message, anchor) = if flags.valid() {
                (String::new(), None)
            } else {
                (message.unwrap_or_default().replace("\r\n", "\n").replace('\r', "\n"), anchor)
            };
            d.page().dom.borrow_mut().set_face_validity(el, flags, message, anchor).js()?;
            Ok(s.undefined())
        });
        def.getter("willValidate", |s, d| {
            let el = d.face("willValidate")?;
            Ok(s.boolean(crate::validity::will_validate(d.page(), el)))
        });
        def.getter("validity", |s, d| {
            let el = d.face("validity")?;
            Ok(s.wrap::<crate::validity::ValidityState>(NodeHold::new(d.page(), el)?, None)?.value())
        });
        def.getter("validationMessage", |s, d| {
            let el = d.face("validationMessage")?;
            let dom = d.page().dom.borrow();
            let invalid =
                dom.will_validate(el).unwrap_or(false) && !dom.validity(el).map(|v| v.valid()).unwrap_or(true);
            Ok(s.string(if invalid { dom.face_validation_message(el).unwrap_or("") } else { "" }))
        });
        def.method("checkValidity", 0, |s, d, _a| {
            let el = d.face("checkValidity")?;
            Ok(s.boolean(crate::validity::check(d.page(), el, false)))
        });
        def.method("reportValidity", 0, |s, d, _a| {
            let el = d.face("reportValidity")?;
            Ok(s.boolean(crate::validity::check(d.page(), el, true)))
        });
        def.getter("labels", |s, d| {
            let el = d.face("labels")?;
            // The custom element's labels: the same live NodeList as `element.labels`.
            Ok(crate::collections::labels(s, d.page(), el)?.value())
        });
        def.getter("states", |s, d| states(s, d.page(), d.id()));
        aria_accessors!(
            def, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27,
            28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43
        );
    }
}

/// `setFormValue`'s `(File or USVString or FormData)?` argument.
fn form_value(s: &Scope<'_>, v: Value<'_>) -> JsResult<Option<FaceValue>> {
    if v.is_nullish() {
        return Ok(None);
    }
    if let Some(f) = s.unwrap::<File>(v) {
        let (name, last_modified) = f.file.clone().unwrap_or_default();
        return Ok(Some(FaceValue::File(FileEntry {
            name,
            mime: f.mime.clone(),
            bytes: f.bytes.clone(),
            last_modified,
        })));
    }
    if let Some(data) = s.unwrap::<FormData>(v) {
        return Ok(Some(FaceValue::Entries(data.entry_values(s, v)?)));
    }
    Ok(Some(FaceValue::Text(dom_string(v)?)))
}

/// A `ValidityStateFlags` dictionary.
fn validity_flags(v: Value<'_>) -> JsResult<Validity> {
    let mut flags = Validity::default();
    if v.is_nullish() {
        return Ok(flags);
    }
    let Some(o) = v.as_object() else {
        return Err(JsError::Type("Failed to execute 'setValidity' on 'ElementInternals': The provided value is not of type 'ValidityStateFlags'.".into()));
    };
    let get = |name: &str| -> JsResult<bool> { Ok(o.get(name)?.to_bool()) };
    // Dictionary members in lexicographic order.
    flags.bad_input = get("badInput")?;
    flags.custom_error = get("customError")?;
    flags.pattern_mismatch = get("patternMismatch")?;
    flags.range_overflow = get("rangeOverflow")?;
    flags.range_underflow = get("rangeUnderflow")?;
    flags.step_mismatch = get("stepMismatch")?;
    flags.too_long = get("tooLong")?;
    flags.too_short = get("tooShort")?;
    flags.type_mismatch = get("typeMismatch")?;
    flags.value_missing = get("valueMissing")?;
    Ok(flags)
}

/// `HTMLElement.attachInternals()`.
pub(crate) fn attach<'s>(s: &Scope<'s>, page: &Rc<PageDom>, el: NodeId) -> JsResult<Value<'s>> {
    let not_supported = |why: &str| {
        Err(JsError::dom("NotSupportedError", format!("Failed to execute 'attachInternals' on 'HTMLElement': {why}")))
    };
    let (custom_name, upgrading) = {
        let dom = page.dom.borrow();
        let e = dom.element(el).js()?;
        let name = e.local_name().clone();
        let defined = e.flags & pyro_dom::node::element_flags::CUSTOM_DEFINED != 0;
        (name, defined)
    };
    // The definition of the element's name in the realm that defined it
    // (`is` customized built-ins are not supported).
    let Some(disabled) = crate::custom::internals_disabled(page, el, &custom_name) else {
        return not_supported("Unable to attach ElementInternals to non-custom elements.");
    };
    if disabled {
        return not_supported("ElementInternals is disabled by disabledFeature static field.");
    }
    // "precustomized" (in its constructor) or "custom".
    if !upgrading && !crate::custom::is_being_constructed(page, el) {
        return not_supported(
            "Unable to attach ElementInternals to a custom element that is not constructed or upgraded.",
        );
    }
    if !page.dom.borrow_mut().attach_internals(el).js()? {
        return not_supported("ElementInternals for the specified element was already attached.");
    }
    let data = InternalsData { el: NodeHold::new(page, el)?, aria: RefCell::new(Vec::new()) };
    Ok(s.wrap::<ElementInternals>(data, None)?.value())
}

// ----- CustomStateSet -------------------------------------------------------

pub struct CustomStateSet;
impl JsClass for CustomStateSet {
    const NAME: &'static str = "CustomStateSet";
    type Data = NodeHold;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.getter("size", |s, h| Ok(s.number(h.page.dom.borrow().custom_states(h.id).len() as f64)));
        def.method_this("add", 1, |_s, h, this, a| {
            a.require(1, "CustomStateSet.add")?;
            let state = dom_string(a.get(0))?;
            h.page.dom.borrow_mut().set_custom_state(h.id, &state, true).js()?;
            Ok(this.value())
        });
        def.method("delete", 1, |s, h, a| {
            a.require(1, "CustomStateSet.delete")?;
            let state = dom_string(a.get(0))?;
            Ok(s.boolean(h.page.dom.borrow_mut().set_custom_state(h.id, &state, false).js()?))
        });
        def.pure_method("has", 1, |s, h, a| {
            a.require(1, "CustomStateSet.has")?;
            let state = dom_string(a.get(0))?;
            Ok(s.boolean(h.page.dom.borrow().has_custom_state(h.id, &state)))
        });
        def.method("clear", 0, |s, h, _a| {
            h.page.dom.borrow_mut().clear_custom_states(h.id).js()?;
            Ok(s.undefined())
        });
    }
}

/// `ElementInternals.states` ([SameObject] while script holds it).
fn states<'s>(s: &Scope<'s>, page: &Rc<PageDom>, el: NodeId) -> JsResult<Value<'s>> {
    let base = page.node_identity(el);
    let key = IdentityKey(base.0 | (1 << 61) | (25 << 48), base.1);
    if let Some(o) = s.lookup_identity(key) {
        return Ok(o.value());
    }
    let set = s.wrap::<CustomStateSet>(NodeHold::new(page, el)?, Some(key))?;
    if crate::custom::first_state_set(page, s.realm_id()) {
        if let Some(proto) = set.prototype().as_object() {
            complete_prototype(s, proto)?;
        }
    }
    Ok(set.value())
}

/// The setlike members of `CustomStateSet.prototype`: iteration over a
/// `Set` that mirrors the tree's states, so iterators see later changes as
/// a `Set`'s do. Added in a realm when its first set is made (no set can be
/// iterated before), so a page that never uses custom states runs none of it.
fn complete_prototype(s: &Scope<'_>, proto: Object<'_>) -> JsResult<()> {
    let complete = s.eval(
        r#"((proto, states) => {
        const define = Object.defineProperty, apply = Reflect.apply, TypeError_ = TypeError, Set_ = Set;
        const setValues = Set.prototype.values, setEntries = Set.prototype.entries, setAdd = Set.prototype.add,
            setDelete = Set.prototype.delete, setClear = Set.prototype.clear, mapGet = WeakMap.prototype.get,
            mapSet = WeakMap.prototype.set;
        const nativeAdd = proto.add, nativeDelete = proto.delete, nativeClear = proto.clear, nativeHas = proto.has;
        const mirrors = new WeakMap();
        // A set's mirror starts with the element's states: a wrapper made
        // again after the last one was collected sees the states it had.
        const mirror = (o) => {
            let m = apply(mapGet, mirrors, [o]);
            if (m === undefined) {
                m = new Set_();
                const current = states(o);
                for (let i = 0; i < current.length; i++) apply(setAdd, m, [current[i]]);
                apply(mapSet, mirrors, [o, m]);
            }
            return m;
        };
        const method = (name, length, fn) => {
            define(fn, 'name', { __proto__: null, value: name, configurable: true });
            define(fn, 'length', { __proto__: null, value: length, configurable: true });
            define(proto, name, { __proto__: null, value: fn, writable: true, enumerable: true, configurable: true });
            return fn;
        };
        method('add', 1, function (value) {
            if (arguments.length < 1) return apply(nativeAdd, this, []);
            value = `${value}`;
            apply(nativeAdd, this, [value]);
            apply(setAdd, mirror(this), [value]);
            return this;
        });
        method('delete', 1, function (value) {
            if (arguments.length < 1) return apply(nativeDelete, this, []);
            value = `${value}`;
            const deleted = apply(nativeDelete, this, [value]);
            apply(setDelete, mirror(this), [value]);
            return deleted;
        });
        method('clear', 0, function () {
            apply(nativeClear, this, []);
            apply(setClear, mirror(this), []);
        });
        const brand = (o) => { apply(nativeHas, o, ['']); return mirror(o); };
        const values = method('values', 0, function () { return apply(setValues, brand(this), []); });
        define(proto, 'keys', { __proto__: null, value: values, writable: true, enumerable: true, configurable: true });
        define(proto, Symbol.iterator, { __proto__: null, value: values, writable: true, configurable: true });
        method('entries', 0, function () { return apply(setEntries, brand(this), []); });
        method('forEach', 1, function (callback, thisArg) {
            if (typeof callback !== 'function') throw new TypeError_('CustomStateSet.forEach: callback is not a function');
            for (const v of apply(setValues, brand(this), [])) apply(callback, thisArg, [v, v, this]);
        });
    })"#,
        Some("pyro://internal/custom-state-set.js"),
        1,
    )?;
    let complete = complete.as_object().ok_or_else(|| JsError::Type("CustomStateSet setlike".into()))?;
    // The element's states, in insertion order (kept private to the closure).
    let states = s.function("states", |s, _this, a| {
        let Some(set) = s.unwrap::<CustomStateSet>(a.get(0)) else {
            return Err(JsError::Type("Illegal invocation".into()));
        };
        let states = set.page.dom.borrow().custom_states(set.id);
        let items: Vec<Value<'_>> = states.iter().map(|st| s.string(st)).collect();
        Ok(s.array(&items)?.value())
    })?;
    complete.call(None, &[proto.value(), states.value()])?;
    Ok(())
}

pub(crate) fn register(engine: &pyro_js::Engine) {
    engine.register::<ElementInternals>();
    engine.register::<CustomStateSet>();
}
