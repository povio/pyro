//! CSS Typed OM 1: `CSSStyleValue` and its subclasses, the `CSS.*` numeric factories and
//! `StylePropertyMap` (`element.attributeStyleMap`, `element.computedStyleMap()`), made in JS
//! (typed_om.js) on a realm's first use. Stylo parses and normalizes values
//! (`pyro_dom::stylo::declarations`); the JS side reifies the normalized text.

use std::rc::Rc;

use pyro_js::{JsResult, Object, Scope, Value};

use crate::lazy_module::LazyModule;
use crate::page::PageDom;

pub(crate) static MODULE: LazyModule = LazyModule {
    key: "lpTypedOm",
    source: include_str!("typed_om.js"),
    url: "pyro://internal/typed_om.js",
    natives: Some(natives),
};

/// The Typed OM interfaces on a window or worker global.
pub(crate) fn define_globals<W>(def: &mut pyro_js::ClassDef<W>)
where
    W: pyro_js::JsClass,
    W::Data: crate::window::WindowHost,
{
    crate::lazy_module::lazy_globals!(
        def,
        MODULE,
        "CSSStyleValue",
        "CSSUnparsedValue",
        "CSSVariableReferenceValue",
        "CSSKeywordValue",
        "CSSNumericValue",
        "CSSUnitValue",
        "CSSMathValue",
        "CSSMathSum",
        "CSSMathProduct",
        "CSSMathNegate",
        "CSSMathInvert",
        "CSSMathMin",
        "CSSMathMax",
        "CSSMathClamp",
        "CSSNumericArray",
        "StylePropertyMapReadOnly",
        "StylePropertyMap",
        "CSSTransformValue",
        "CSSTransformComponent",
        "CSSTranslate",
        "CSSRotate",
        "CSSScale",
        "CSSSkew",
        "CSSSkewX",
        "CSSSkewY",
        "CSSPerspective",
        "CSSMatrixComponent",
        "CSSImageValue",
        "CSSColorValue",
        "CSSRGB",
        "CSSHSL",
        "CSSHWB",
        "CSSLab",
        "CSSLCH",
        "CSSOKLab",
        "CSSOKLCH",
    );
}

/// Adds the `CSS.px()`, `CSS.number()`, ... factories to a realm's `CSS` namespace object.
pub(crate) fn add_factories(s: &Scope<'_>, page: &Rc<PageDom>, css: Value<'_>) -> JsResult<()> {
    let Some(css) = css.as_object() else { return Ok(()) };
    let factories = MODULE.module(s, page)?.get("factories")?;
    let assign = s.global().get_atom("Object")?.as_object().map(|o| o.get_atom("assign")).transpose()?;
    if let Some(assign) = assign.and_then(|a| a.as_object()) {
        assign.call(None, &[css.value(), factories])?;
    }
    Ok(())
}

/// `element.attributeStyleMap` (one map per element).
pub(crate) fn attribute_style_map<'s>(s: &Scope<'s>, page: &Rc<PageDom>, element: Value<'s>) -> JsResult<Value<'s>> {
    MODULE.helper(s, page, "attributeStyleMap", &[element])
}

/// `CSSStyleRule.styleMap`: the map of the rule's declaration block `style`.
pub(crate) fn declaration_map<'s>(s: &Scope<'s>, page: &Rc<PageDom>, style: Value<'s>) -> JsResult<Value<'s>> {
    MODULE.helper(s, page, "declarationMap", &[style])
}

/// `element.computedStyleMap()`.
pub(crate) fn computed_style_map<'s>(s: &Scope<'s>, page: &Rc<PageDom>, element: Value<'s>) -> JsResult<Value<'s>> {
    MODULE.helper(s, page, "computedStyleMap", &[element])
}

fn natives<'s>(s: &Scope<'s>) -> JsResult<Object<'s>> {
    let natives = s.object();
    // kind(property): [canonical name, 0 longhand | 1 shorthand | 2 custom], or null.
    let f = s.function("kind", |s, _this, a| {
        let name = crate::util::dom_string(a.get(0))?;
        Ok(match pyro_dom::stylo::declarations::property_kind(&name) {
            Some((name, kind)) => s.array(&[s.string(&name), s.number(f64::from(kind))])?.value(),
            None => s.null(),
        })
    })?;
    natives.set_atom("kind", f.value())?;
    // parse(property, text): the value normalized as Stylo serializes it, or null when invalid.
    let f = s.function("parse", |s, _this, a| {
        let name = crate::util::dom_string(a.get(0))?;
        let text = crate::util::dom_string(a.get(1))?;
        Ok(match pyro_dom::stylo::declarations::normalized_value(&name, &text) {
            Some(v) => s.string(&v),
            None => s.null(),
        })
    })?;
    natives.set_atom("parse", f.value())?;
    Ok(natives)
}
