//! The compiled Sanitizer API configuration the HTML tree builder applies while it parses
//! (HTML "HTML sanitization" + Sanitizer API "sanitize core", done inside the tree builder
//! instead of on a finished tree, so adoption agency, the Noah's Ark clause, the script data
//! tokenizer state and the form owner behave as in the specification's parser integration).
//!
//! # Interface for the other lanes (S1-S4)
//!
//! A *canonical* `SanitizerConfig` (every name an object with a namespace, `elements` or
//! `removeElements`, `attributes` or `removeAttributes`, ...) compiles to one [`FragmentFilter`]
//! field by field. A key that is absent from the canonical dictionary is the field's `None` /
//! empty / default value:
//!
//! | canonical key                         | [`FragmentFilter`] field                                   |
//! |---------------------------------------|------------------------------------------------------------|
//! | `elements` (+ per element `attributes`, `removeAttributes`) | `elements: Some(map (ns, local) -> `[`ElementRule`]`)`; absent: `None` |
//! | `removeElements`                      | `remove_elements`                                          |
//! | `replaceWithChildrenElements`         | `replace_with_children`                                    |
//! | `attributes`                          | `attributes: Some(names)`; absent: `None`                  |
//! | `removeAttributes`                    | `remove_attributes`                                        |
//! | `dataAttributes`                      | `data_attributes` (only read when `attributes` is `Some`)  |
//! | `javascriptURLs`                      | `javascript_urls` (absent: `false`, as the spec's default for the unsafe baseline; `Default` is permissive: `true`) |
//! | `comments`                            | `comments` (absent: `false`; `Default`: `true`)            |
//! | `processingInstructions` (targets)    | `processing_instructions: Some(set)`                       |
//! | `removeProcessingInstructions`        | `remove_processing_instructions`                           |
//!
//! Attribute names are `(Namespace, LocalName)` with the empty namespace `ns!()` for "no
//! namespace" (`namespace: null`). Element names are `(Namespace, LocalName)` too (the HTML
//! namespace for plain strings). `FragmentFilter::default()` is the *empty* configuration: it
//! removes nothing.
//!
//! The built-in configurations are in [`crate::sanitizer_defaults`]: `safe_default()` is the
//! "built-in safe default configuration" (what `new Sanitizer()` / `"default"` is) and
//! `remove_unsafe(&mut filter, event_handler_names)` merges the "built-in safe baseline" into a
//! filter (what the safe methods always apply on top of the user's configuration, and what
//! `Sanitizer.removeUnsafe()` does). Wrap the finished filter in `Rc` and pass `Some(filter)`:
//!
//! * `parse::set_html_unsafe_with(dom, target, html, run_scripts, filter)` (setHTML / setHTMLUnsafe;
//!   the safe method passes `run_scripts = false`);
//! * `parse::parse_modern_html_fragment_with(dom, context, html, run_scripts, filter)` (positional
//!   `appendHTML`, `beforeHTML`, ... : the fragment is returned, the caller inserts it);
//! * `parse::parse_html_with(dom, html, filter)` (`Document.parseHTML`) and
//!   `parse::parse_html_unsafe_with(dom, html, filter)` (`Document.parseHTMLUnsafe`), both of which
//!   return a new document;
//! * `StreamingFragmentParser::with_filter(dom, context, target, before, run_scripts, filter)`
//!   (the six `stream*HTML[Unsafe]` methods).
//!
//! `None` is the unfiltered, zero-cost parser path.
use crate::{LocalName, Namespace, QualName};
use markup5ever::{local_name, ns};
use rustc_hash::{FxHashMap, FxHashSet};

pub type Names = FxHashSet<(Namespace, LocalName)>;

/// What an `elements` entry says about one element (`SanitizerElementNamespaceWithAttributes`).
#[derive(Clone, Debug, Default)]
pub struct ElementRule {
    /// The element's own attribute allow-list (`attributes`): `None` when the entry has none.
    pub attributes: Option<Names>,
    /// The element's own attribute deny-list (`removeAttributes`).
    pub remove_attributes: Names,
}

/// What the tree builder does with an element.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Disposition {
    Keep,
    /// The element and everything parsed under it is dropped.
    Remove,
    /// The element is dropped, what is parsed under it goes to its parent.
    ReplaceWithChildren,
}

#[derive(Clone, Debug)]
pub struct FragmentFilter {
    /// The element allow-list (`elements`); `None` allows every element not named otherwise.
    pub elements: Option<FxHashMap<(Namespace, LocalName), ElementRule>>,
    pub remove_elements: Names,
    pub replace_with_children: Names,
    /// The global attribute allow-list (`attributes`); `None` allows every attribute.
    pub attributes: Option<Names>,
    pub remove_attributes: Names,
    /// `data-*` attributes pass although `attributes` does not list them. Read only with `attributes`.
    pub data_attributes: bool,
    /// Keep attributes whose value navigates to a `javascript:` URL.
    pub javascript_urls: bool,
    pub comments: bool,
    pub processing_instructions: Option<FxHashSet<String>>,
    pub remove_processing_instructions: FxHashSet<String>,
}

impl Default for FragmentFilter {
    fn default() -> Self {
        Self {
            elements: None,
            remove_elements: Names::default(),
            replace_with_children: Names::default(),
            attributes: None,
            remove_attributes: Names::default(),
            data_attributes: false,
            javascript_urls: true,
            comments: true,
            processing_instructions: None,
            remove_processing_instructions: FxHashSet::default(),
        }
    }
}

impl FragmentFilter {
    /// Sanitizer API "sanitize core", the element step: what to do with an element named `name`.
    pub fn disposition(&self, name: &QualName) -> Disposition {
        let key = (name.ns.clone(), name.local.clone());
        if self.remove_elements.contains(&key) {
            return Disposition::Remove;
        }
        // Checked before the allow-list: a replaced element need not be in `elements`.
        if !self.replace_with_children.is_empty() && self.replace_with_children.contains(&key) && !non_replaceable(name) {
            return Disposition::ReplaceWithChildren;
        }
        match &self.elements {
            Some(elements) if !elements.contains_key(&key) => Disposition::Remove,
            _ => Disposition::Keep,
        }
    }

    /// Sanitizer API "sanitize core", the attribute step: whether attribute `attr` with `value`
    /// stays on element `el` (element rule, global rules, `data-*`, navigating `javascript:` URLs).
    pub fn allows_attribute(&self, el: &QualName, attr: &QualName, value: &str) -> bool {
        let key = (attr.ns.clone(), attr.local.clone());
        let rule = self.elements.as_ref().and_then(|elements| elements.get(&(el.ns.clone(), el.local.clone())));
        if rule.is_some_and(|rule| rule.remove_attributes.contains(&key)) {
            return false;
        }
        let local = rule.and_then(|rule| rule.attributes.as_ref());
        let allowed = match &self.attributes {
            Some(global) => {
                global.contains(&key)
                    || local.is_some_and(|local| local.contains(&key))
                    || (self.data_attributes && is_data_attribute(attr))
            }
            None => !self.remove_attributes.contains(&key) && local.is_none_or(|local| local.contains(&key)),
        };
        allowed && (self.javascript_urls || !is_javascript_url_attribute(el, attr, value))
    }

    pub fn allows_pi(&self, target: &str) -> bool {
        self.processing_instructions
            .as_ref()
            .map_or_else(|| !self.remove_processing_instructions.contains(target), |allowed| allowed.contains(target))
    }
}

/// The built-in non-replaceable elements list: `html`, `svg` and `math` stay.
fn non_replaceable(name: &QualName) -> bool {
    (name.ns == ns!(html) && name.local == local_name!("html"))
        || (name.ns == ns!(svg) && name.local == local_name!("svg"))
        || (name.ns == ns!(mathml) && name.local == local_name!("math"))
}

/// A custom data attribute: no namespace, `data-` prefix.
fn is_data_attribute(attr: &QualName) -> bool {
    attr.ns == ns!() && attr.local.starts_with("data-")
}

/// Whether `value` parses as a `javascript:` URL: leading and trailing C0 control or space
/// stripped, ASCII tab and newlines removed anywhere, the scheme compared ASCII case-insensitively.
pub fn is_javascript_url(value: &str) -> bool {
    let trimmed = value.trim_matches(|c: char| c <= ' ');
    let mut chars = trimmed.chars().filter(|c| !matches!(*c, '\t' | '\n' | '\r'));
    "javascript:".chars().all(|expected| chars.next().is_some_and(|c| c.to_ascii_lowercase() == expected))
}

/// Whether an `attributeName` value names the `href` attribute, bare or behind exactly one prefix.
fn names_href(value: &str) -> bool {
    match value.split_once(':') {
        None => value == "href",
        Some((prefix, local)) => !prefix.is_empty() && local == "href",
    }
}

/// The built-in navigating URL attributes list and the built-in animating URL attributes list,
/// restricted to a `javascript:` URL: whether `attr` (with `value`) on `el` is one of them.
fn is_javascript_url_attribute(el: &QualName, attr: &QualName, value: &str) -> bool {
    let href_like = attr.local == local_name!("href") && (attr.ns == ns!() || attr.ns == ns!(xlink));
    let no_ns = attr.ns == ns!();
    let local: &str = &attr.local;
    let element: &str = &el.local;
    if el.ns == ns!(html) {
        let navigates = no_ns
            && matches!((element, local), ("a", "href") | ("area", "href") | ("button", "formaction") | ("form", "action") | ("input", "formaction"));
        return navigates && is_javascript_url(value);
    }
    if el.ns == ns!(svg) {
        if element == "a" && href_like {
            return is_javascript_url(value);
        }
        // `<animate attributeName=href>` would animate a URL into existence: its value is not a URL.
        // The target may be named through any namespace prefix (`xlink:href`, and `zlink:href`
        // whatever `zlink` is bound to, or is not bound to at all): all of them are removed.
        return matches!(element, "animate" | "animateTransform" | "set")
            && no_ns
            && local == "attributeName"
            && names_href(value);
    }
    el.ns == ns!(mathml) && href_like && is_javascript_url(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sanitizer_defaults::{remove_unsafe, safe_default};

    fn html(name: &str) -> QualName {
        QualName::new(None, ns!(html), name.into())
    }
    fn svg(name: &str) -> QualName {
        QualName::new(None, ns!(svg), name.into())
    }
    fn attr(name: &str) -> QualName {
        QualName::new(None, ns!(), name.into())
    }

    #[test]
    fn empty_configuration_keeps_everything() {
        let f = FragmentFilter::default();
        assert_eq!(f.disposition(&html("script")), Disposition::Keep);
        assert!(f.allows_attribute(&html("a"), &attr("onclick"), "x"));
        assert!(f.allows_attribute(&html("a"), &attr("href"), "javascript:1"));
    }

    #[test]
    fn default_configuration_data() {
        let f = safe_default();
        assert_eq!(f.disposition(&html("div")), Disposition::Keep);
        assert_eq!(f.disposition(&html("script")), Disposition::Remove);
        assert_eq!(f.disposition(&html("template")), Disposition::Remove);
        assert_eq!(f.disposition(&svg("circle")), Disposition::Keep);
        assert_eq!(f.disposition(&svg("script")), Disposition::Remove);
        assert!(f.allows_attribute(&html("a"), &attr("href"), "https://example.com/"));
        assert!(!f.allows_attribute(&html("a"), &attr("href"), " JaVa\nScRiPt:alert(1)"));
        assert!(!f.allows_attribute(&html("div"), &attr("href"), "https://example.com/"));
        assert!(f.allows_attribute(&html("div"), &attr("title"), "t"));
        assert!(!f.allows_attribute(&html("div"), &attr("onclick"), "x"));
        assert!(!f.allows_attribute(&html("div"), &attr("data-x"), "x"));
        assert!(!f.comments);
        assert!(!f.allows_pi("xml"));
    }

    #[test]
    fn remove_unsafe_on_default_changes_nothing_and_on_empty_adds_baseline() {
        let before = safe_default();
        let mut after = safe_default();
        remove_unsafe(&mut after, &["onclick", "onload"]);
        assert_eq!(before.elements.as_ref().unwrap().len(), after.elements.as_ref().unwrap().len());
        assert_eq!(before.attributes.as_ref().unwrap().len(), after.attributes.as_ref().unwrap().len());
        assert!(after.remove_elements.is_empty() && after.remove_attributes.is_empty());

        let mut empty = FragmentFilter::default();
        remove_unsafe(&mut empty, &["onclick", "onload"]);
        assert_eq!(empty.remove_elements.len(), 8);
        assert_eq!(empty.disposition(&html("script")), Disposition::Remove);
        assert_eq!(empty.disposition(&svg("use")), Disposition::Remove);
        assert_eq!(empty.disposition(&html("use")), Disposition::Keep);
        assert!(!empty.allows_attribute(&html("p"), &attr("onclick"), ""));
        assert!(!empty.javascript_urls);
    }

    #[test]
    fn replace_and_per_element_rules() {
        let mut f = FragmentFilter::default();
        f.replace_with_children.insert((ns!(html), "b".into()));
        f.replace_with_children.insert((ns!(html), "html".into()));
        assert_eq!(f.disposition(&html("b")), Disposition::ReplaceWithChildren);
        assert_eq!(f.disposition(&html("html")), Disposition::Keep);
        let mut elements = FxHashMap::default();
        let mut rule = ElementRule::default();
        rule.attributes = Some([(ns!(), LocalName::from("style"))].into_iter().collect());
        elements.insert((ns!(html), LocalName::from("div")), rule);
        f.elements = Some(elements);
        assert_eq!(f.disposition(&html("b")), Disposition::ReplaceWithChildren);
        assert_eq!(f.disposition(&html("p")), Disposition::Remove);
        assert!(f.allows_attribute(&html("div"), &attr("style"), ""));
        assert!(!f.allows_attribute(&html("div"), &attr("class"), ""));
    }

    #[test]
    fn animated_href_through_any_prefix_is_removed() {
        let set = svg("set");
        let mut allowing = FragmentFilter::default();
        allowing.javascript_urls = false;
        for (value, removed) in [("href", true), ("xlink:href", true), ("ylink:href", true), ("zlink:href", true), ("xlink:href:x", false), ("hreflang", false), ("xref", false), (":href", false)] {
            assert_eq!(!allowing.allows_attribute(&set, &attr("attributeName"), value), removed, "{value}");
        }
    }

    #[test]
    fn javascript_url_normalisation() {
        assert!(is_javascript_url("javascript:alert(1)"));
        assert!(is_javascript_url("\u{1} java\tscript:x"));
        assert!(!is_javascript_url("javascript"));
        assert!(!is_javascript_url("https://javascript:x"));
    }
}
