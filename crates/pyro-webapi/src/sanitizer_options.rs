//! WebIDL conversion of the parser options of the HTML parsing methods: `SetHTMLUnsafeOptions`
//! (`runScripts`, `sanitizer`) for the unsafe methods and `SetHTMLOptions` (`sanitizer`) for the
//! safe ones. A `sanitizer` member (a `Sanitizer`, a `SanitizerConfig` dictionary or the
//! `"default"` preset) is canonicalised in JS (sanitizer.js, throwing the constructor's
//! TypeErrors) and compiled here into the parser's [`FragmentFilter`]; a safe method always adds
//! the built-in safe baseline (`remove_unsafe`) on top of whatever configuration it was given.
use crate::util::dom_string;
use pyro_dom::fragment_filter::{ElementRule, FragmentFilter, Names};
use pyro_dom::sanitizer_defaults::{remove_unsafe, safe_default};
use pyro_js::{JsError, JsResult, Object, Scope, Value};
use rustc_hash::{FxHashMap, FxHashSet};
use std::rc::Rc;

/// Which parsing method's options are converted.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParseMode {
    /// `setHTMLUnsafe` and the other unsafe fragment methods: `SetHTMLUnsafeOptions`.
    Unsafe,
    /// `setHTML` and the other safe methods: `SetHTMLOptions`.
    Safe,
    /// `Document.parseHTMLUnsafe`: a `sanitizer` that is not forced safe, and no `runScripts` (the
    /// new document has no browsing context, so its scripts never run and the member is not read).
    UnsafeDocument,
}

#[derive(Clone, Default)]
pub(crate) struct ParserOptions {
    pub run_scripts: bool,
    pub filter: Option<Rc<FragmentFilter>>,
    /// The options need vetting by a default policy's `createParserOptions` (Trusted Types):
    /// `runScripts` is true or the `sanitizer` member says something.
    pub needs_vetting: bool,
    /// The `sanitizer` member was given.
    pub has_sanitizer: bool,
}

impl ParserOptions {
    /// `SetHTMLUnsafeOptions`: the unsafe methods' options (`runScripts` and `sanitizer`).
    pub fn convert(s: &Scope<'_>, value: Value<'_>) -> JsResult<Self> {
        Self::convert_with(s, value, ParseMode::Unsafe)
    }

    /// `SetHTMLOptions` (`ParseMode::Safe`): the safe methods' options. `runScripts` is not read;
    /// the filter is never `None` (the `"default"` preset stands for a missing `sanitizer`) and
    /// always includes the built-in safe baseline.
    pub fn convert_with(s: &Scope<'_>, value: Value<'_>, mode: ParseMode) -> JsResult<Self> {
        let safe = mode == ParseMode::Safe;
        if value.is_nullish() {
            return Ok(Self::without_sanitizer(false, safe));
        }
        let obj = value.as_object().ok_or_else(|| JsError::Type("Expected HTML parser options".into()))?;
        let run_scripts = mode == ParseMode::Unsafe && obj.get_atom("runScripts")?.to_bool();
        let sanitizer = obj.get_atom("sanitizer")?;
        if sanitizer.is_undefined() {
            return Ok(Self::without_sanitizer(run_scripts, safe));
        }
        if sanitizer.is_string() && dom_string(sanitizer)? == "default" {
            return Ok(Self { run_scripts, filter: Some(Rc::new(safe_default())), needs_vetting: true, has_sanitizer: true });
        }
        let page = crate::target::page_of(s)?;
        let resolved = crate::sanitizer::resolve(s, &page, s.rehome(sanitizer), safe)?
            .as_object()
            .ok_or_else(|| JsError::Type("The sanitizer is unavailable".into()))?;
        let config = resolved.get_atom("config")?.as_object().ok_or_else(|| JsError::Type("The sanitizer is unavailable".into()))?;
        let nonempty = resolved.get_atom("nonempty")?.to_bool();
        let mut filter = compile(config)?;
        if safe {
            apply_baseline(&mut filter);
        }
        Ok(Self { run_scripts, filter: Some(Rc::new(filter)), needs_vetting: run_scripts || nonempty, has_sanitizer: true })
    }

    /// No `sanitizer` member: an unsafe method does not filter; a safe one applies the default configuration.
    fn without_sanitizer(run_scripts: bool, safe: bool) -> Self {
        Self { run_scripts, filter: safe.then(|| Rc::new(safe_default())), needs_vetting: run_scripts, has_sanitizer: false }
    }

}

/// "Remove unsafe": the built-in safe baseline configuration merged into `filter`.
fn apply_baseline(filter: &mut FragmentFilter) {
    let handlers: Vec<&str> = crate::handlers::event_handler_attributes().iter().map(String::as_str).collect();
    remove_unsafe(filter, &handlers);
}

/// Compiles a canonical `SanitizerConfig` (sanitizer.js: every name an object, `elements` or
/// `removeElements`, `attributes` or `removeAttributes`, ...) into the parser's filter.
fn compile(config: Object<'_>) -> JsResult<FragmentFilter> {
    let mut filter = FragmentFilter::default();
    let elements = config.get_atom("elements")?;
    if !elements.is_undefined() {
        let mut rules: FxHashMap<_, ElementRule> = FxHashMap::default();
        each(elements, |item| {
            let item = item.as_object().ok_or_else(invalid)?;
            let key = name_of(item)?;
            let attributes = item.get_atom("attributes")?;
            let remove_attributes = item.get_atom("removeAttributes")?;
            rules.insert(
                key,
                ElementRule {
                    attributes: if attributes.is_undefined() { None } else { Some(names(attributes)?) },
                    remove_attributes: if remove_attributes.is_undefined() { Names::default() } else { names(remove_attributes)? },
                },
            );
            Ok(())
        })?;
        filter.elements = Some(rules);
    }
    filter.remove_elements = optional_names(config.get_atom("removeElements")?)?;
    filter.replace_with_children = optional_names(config.get_atom("replaceWithChildrenElements")?)?;
    let attributes = config.get_atom("attributes")?;
    if !attributes.is_undefined() {
        filter.attributes = Some(names(attributes)?);
    }
    filter.remove_attributes = optional_names(config.get_atom("removeAttributes")?)?;
    filter.data_attributes = config.get_atom("dataAttributes")?.to_bool();
    filter.javascript_urls = config.get_atom("javascriptURLs")?.to_bool();
    filter.comments = config.get_atom("comments")?.to_bool();
    let allowed = config.get_atom("processingInstructions")?;
    if !allowed.is_undefined() {
        filter.processing_instructions = Some(targets(allowed)?);
    }
    let removed = config.get_atom("removeProcessingInstructions")?;
    if !removed.is_undefined() {
        filter.remove_processing_instructions = targets(removed)?;
    }
    Ok(filter)
}

fn invalid() -> JsError {
    JsError::Type("Invalid sanitizer configuration".into())
}

/// Calls `f` for every item of the canonical array `list`.
fn each<'s>(list: Value<'s>, mut f: impl FnMut(Value<'s>) -> JsResult<()>) -> JsResult<()> {
    let array = list.as_object().ok_or_else(invalid)?;
    let len = array.get_atom("length")?.to_number()? as u32;
    for i in 0..len {
        f(array.get_index(i)?)?;
    }
    Ok(())
}

/// An `{name, namespace}` object: `namespace: null` is no namespace.
fn name_of(item: Object<'_>) -> JsResult<(pyro_dom::Namespace, pyro_dom::LocalName)> {
    let local = dom_string(item.get_atom("name")?)?;
    let namespace = item.get_atom("namespace")?;
    let namespace = if namespace.is_nullish() { String::new() } else { dom_string(namespace)? };
    Ok((namespace.into(), local.into()))
}

fn names(list: Value<'_>) -> JsResult<Names> {
    let mut out = Names::default();
    each(list, |item| {
        out.insert(name_of(item.as_object().ok_or_else(invalid)?)?);
        Ok(())
    })?;
    Ok(out)
}

fn optional_names(list: Value<'_>) -> JsResult<Names> {
    if list.is_undefined() {
        Ok(Names::default())
    } else {
        names(list)
    }
}

fn targets(list: Value<'_>) -> JsResult<FxHashSet<String>> {
    let mut out = FxHashSet::default();
    each(list, |item| {
        out.insert(dom_string(item.as_object().ok_or_else(invalid)?.get_atom("target")?)?);
        Ok(())
    })?;
    Ok(out)
}
