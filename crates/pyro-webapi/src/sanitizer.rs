//! The HTML Sanitizer API's `Sanitizer` interface (https://wicg.github.io/sanitizer-api/), made in JS
//! (sanitizer.js) on a window's first use. The configuration lives in JS, as a canonical
//! dictionary; [`resolve`] hands the `sanitizer` option of a safe or unsafe parsing method to
//! the options conversion (sanitizer_options.rs), which compiles it into the parser's filter.
use std::rc::Rc;

use pyro_dom::fragment_filter::FragmentFilter;
use pyro_js::{ClassDef, JsClass, JsResult, Object, Scope, Value};
use serde_json::{json, Value as Json};

use crate::lazy_module::{lazy_globals, LazyModule};
use crate::page::PageDom;
use crate::window::WindowHost;

static MODULE: LazyModule = LazyModule {
    key: "lpSanitizer",
    source: include_str!("sanitizer.js"),
    url: "pyro://internal/sanitizer.js",
    natives: Some(natives),
};

/// `Sanitizer` is exposed on Window only.
pub(crate) fn define<W: JsClass>(def: &mut ClassDef<W>)
where
    W::Data: WindowHost,
{
    lazy_globals!(def, MODULE, "Sanitizer");
}

/// The canonical configuration (a fresh plain dictionary) of a `sanitizer` option's value: a
/// Sanitizer's, a SanitizerConfig dictionary's (`safe`: for a safe method, whose missing booleans
/// and processing instruction list default to false) or the `"default"` preset's. Throws the
/// constructor's TypeError for an invalid one.
pub(crate) fn resolve<'s>(s: &Scope<'s>, page: &Rc<PageDom>, value: Value<'s>, safe: bool) -> JsResult<Value<'s>> {
    MODULE.helper(s, page, "resolve", &[value, s.boolean(safe)])
}

/// A new `Sanitizer` object holding a copy of `config` (a canonical dictionary [`resolve`] made, or
/// the string `"default"`): the `sanitizer` member of a Trusted Types policy callback's input.
pub(crate) fn instance<'s>(s: &Scope<'s>, page: &Rc<PageDom>, config: Value<'s>) -> JsResult<Value<'s>> {
    MODULE.helper(s, page, "instance", &[config])
}

fn names_json<'a>(names: impl Iterator<Item = &'a (pyro_dom::Namespace, pyro_dom::LocalName)>) -> Json {
    let mut list: Vec<(Option<String>, String)> =
        names.map(|(ns, local)| ((!ns.is_empty()).then(|| ns.to_string()), local.to_string())).collect();
    list.sort();
    Json::Array(list.into_iter().map(|(namespace, name)| json!({ "name": name, "namespace": namespace })).collect())
}

/// The built-in safe default configuration as the canonical dictionary of its filter.
fn default_config(filter: &FragmentFilter) -> Json {
    let mut elements: Vec<_> = filter.elements.iter().flatten().collect();
    elements.sort_by(|a, b| (a.0 .0.to_string(), a.0 .1.to_string()).cmp(&(b.0 .0.to_string(), b.0 .1.to_string())));
    let elements: Vec<Json> = elements
        .into_iter()
        .map(|((ns, local), rule)| {
            let mut element = json!({ "name": local.to_string(), "namespace": ns.to_string() });
            if let Some(attributes) = &rule.attributes {
                element["attributes"] = names_json(attributes.iter());
            }
            element
        })
        .collect();
    let mut config = json!({ "elements": elements, "comments": filter.comments, "javascriptURLs": filter.javascript_urls });
    if let Some(attributes) = &filter.attributes {
        config["attributes"] = names_json(attributes.iter());
        config["dataAttributes"] = json!(filter.data_attributes);
    }
    let mut targets: Vec<&String> = filter.processing_instructions.iter().flatten().collect();
    targets.sort();
    config["processingInstructions"] = Json::Array(targets.into_iter().map(|target| json!({ "target": target })).collect());
    config
}

fn natives<'s>(s: &Scope<'s>) -> JsResult<Object<'s>> {
    let natives = s.object();
    let default = s.function("defaultConfig", |s, _this, _a| {
        let filter = pyro_dom::sanitizer_defaults::safe_default();
        Ok(s.string(&default_config(&filter).to_string()))
    })?;
    natives.set_atom("defaultConfig", default.value())?;
    let handlers = s.function("eventHandlers", |s, _this, _a| {
        let names: Vec<Value<'_>> = crate::handlers::event_handler_attributes().iter().map(|name| s.string(name)).collect();
        Ok(s.array(&names)?.value())
    })?;
    natives.set_atom("eventHandlers", handlers.value())?;
    Ok(natives)
}
