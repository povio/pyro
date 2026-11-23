//! WebIDL conversion for the explicit SanitizerConfig rules the fragment parser implements.
use crate::util::dom_string;
use pyro_dom::fragment_filter::{FragmentFilter, Names};
use pyro_js::{JsError, JsResult, Scope, Value};
use rustc_hash::FxHashSet;
use std::rc::Rc;

#[derive(Clone, Default)]
pub(crate) struct ParserOptions {
    pub run_scripts: bool,
    pub filter: Option<Rc<FragmentFilter>>,
    pub needs_vetting: bool,
}

impl ParserOptions {
    pub fn convert(s: &Scope<'_>, value: Value<'_>) -> JsResult<Self> {
        if value.is_undefined() || value.is_null() {
            return Ok(Self::default());
        }
        let obj = value.as_object().ok_or_else(|| JsError::Type("Expected HTML parser options".into()))?;
        let run_scripts = obj.get_atom("runScripts")?.to_bool();
        let sanitizer = obj.get_atom("sanitizer")?;
        if sanitizer.is_undefined() {
            return Ok(Self { run_scripts, needs_vetting: run_scripts, filter: None });
        }
        let (filter, nonempty) = convert_config(s, sanitizer)?;
        Ok(Self { run_scripts, needs_vetting: run_scripts || nonempty, filter: Some(Rc::new(filter)) })
    }
}

fn convert_config(s: &Scope<'_>, value: Value<'_>) -> JsResult<(FragmentFilter, bool)> {
    let obj = value.as_object().ok_or_else(|| JsError::Type("Expected an explicit sanitizer configuration".into()))?;
    let mut filter = FragmentFilter::default();
    let mut nonempty = false;
    let mut duplicate = false;
    // WebIDL reads dictionary members in code point order. Unsupported rule
    // families must reject the whole configuration before the tree changes.
    for key in [
        "attributes",
        "comments",
        "dataAttributes",
        "elements",
        "javascriptURLs",
        "processingInstructions",
        "removeAttributes",
        "removeElements",
        "removeProcessingInstructions",
        "replaceWithChildrenElements",
    ] {
        let field = obj.get_atom(key)?;
        if field.is_undefined() {
            continue;
        }
        nonempty = true;
        match key {
            "comments" => filter.comments = field.to_bool(),
            "javascriptURLs" if field.to_bool() => {}
            "removeElements" => {
                filter.remove_elements = names(s, field, "http://www.w3.org/1999/xhtml", &mut duplicate)?
            }
            "removeAttributes" => filter.remove_attributes = names(s, field, "", &mut duplicate)?,
            "processingInstructions" => filter.processing_instructions = Some(targets(s, field, &mut duplicate)?),
            "removeProcessingInstructions" => {
                filter.remove_processing_instructions = targets(s, field, &mut duplicate)?;
                if filter.processing_instructions.is_some() {
                    return Err(JsError::Type("Processing instruction lists are mutually exclusive".into()));
                }
            }
            _ => return Err(JsError::Type(format!("Unsupported sanitizer rule: {key}").into())),
        }
    }
    if duplicate {
        return Err(JsError::Type("Duplicate sanitizer configuration entry".into()));
    }
    Ok((filter, nonempty))
}

fn names(s: &Scope<'_>, value: Value<'_>, default_ns: &'static str, duplicate: &mut bool) -> JsResult<Names> {
    let items = s.convert_sequence(value, "SanitizerConfig", move |_, item| {
        let (ns, local) = if let Some(dictionary) = item.as_object() {
            let name = dictionary.get_atom("name")?;
            if name.is_undefined() {
                return Err(JsError::Type("A sanitizer name is required".into()));
            }
            let local = dom_string(name)?;
            let ns = dictionary.get_atom("namespace")?;
            let ns = if ns.is_undefined() {
                default_ns.into()
            } else if ns.is_null() {
                String::new()
            } else {
                dom_string(ns)?
            };
            (ns, local)
        } else if item.is_nullish() {
            return Err(JsError::Type("A sanitizer name is required".into()));
        } else {
            (default_ns.into(), dom_string(item)?)
        };
        Ok((ns.into(), local.into()))
    })?;
    let len = items.len();
    let names: Names = items.into_iter().collect();
    *duplicate |= len != names.len();
    Ok(names)
}

fn targets(s: &Scope<'_>, value: Value<'_>, duplicate: &mut bool) -> JsResult<FxHashSet<String>> {
    let items = s.convert_sequence(value, "SanitizerConfig", |_, item| {
        let target = if let Some(dictionary) = item.as_object() {
            let target = dictionary.get_atom("target")?;
            if target.is_undefined() {
                return Err(JsError::Type("A processing instruction target is required".into()));
            }
            target
        } else if item.is_nullish() {
            return Err(JsError::Type("A processing instruction target is required".into()));
        } else {
            item
        };
        dom_string(target)
    })?;
    let len = items.len();
    let targets: FxHashSet<_> = items.into_iter().collect();
    *duplicate |= len != targets.len();
    Ok(targets)
}
