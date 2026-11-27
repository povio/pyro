//! The Sanitizer API's built-in configurations as [`FragmentFilter`]s: the "built-in safe default
//! configuration" (https://wicg.github.io/sanitizer-api/#built-in-safe-default-configuration, the
//! tables below are WPT `sanitizer-api/sanitizer-default-config.html`'s expected `get()`) and the
//! "built-in safe baseline configuration" that "remove unsafe" merges into any configuration.
use crate::fragment_filter::{ElementRule, FragmentFilter, Names};
use crate::{LocalName, Namespace};
use markup5ever::ns;
use rustc_hash::{FxHashMap, FxHashSet};

/// The built-in safe baseline's `removeElements`: script-executing and embedding elements.
fn baseline_elements() -> [(Namespace, &'static str); 8] {
    [
        (ns!(html), "base"),
        (ns!(html), "embed"),
        (ns!(html), "frame"),
        (ns!(html), "iframe"),
        (ns!(html), "object"),
        (ns!(html), "script"),
        (ns!(svg), "script"),
        (ns!(svg), "use"),
    ]
}

/// The built-in safe default configuration (`new Sanitizer()`, `"default"`): an element allow-list
/// with per-element attributes, a global attribute allow-list, no comments, no data attributes,
/// no processing instructions, no `javascript:` URLs.
pub fn safe_default() -> FragmentFilter {
    let mut elements: FxHashMap<(Namespace, LocalName), ElementRule> = FxHashMap::default();
    for (namespace, table) in [(ns!(html), HTML_ELEMENTS), (ns!(svg), SVG_ELEMENTS), (ns!(mathml), MATHML_ELEMENTS)] {
        for (name, attributes) in table {
            let attributes: Names = attributes.iter().map(|a| (ns!(), LocalName::from(*a))).collect();
            elements.insert(
                (namespace.clone(), LocalName::from(*name)),
                ElementRule { attributes: Some(attributes), remove_attributes: Names::default() },
            );
        }
    }
    FragmentFilter {
        elements: Some(elements),
        attributes: Some(ATTRIBUTES.iter().map(|a| (ns!(), LocalName::from(*a))).collect()),
        data_attributes: false,
        javascript_urls: false,
        comments: false,
        processing_instructions: Some(FxHashSet::default()),
        ..FragmentFilter::default()
    }
}

/// Sanitizer API "remove unsafe": merges the built-in safe baseline into `filter`. The baseline
/// elements are dropped from the element allow-list and the replace list, and added to
/// `remove_elements` when the filter has no allow-list; every event handler content attribute
/// (`event_handlers`: the names `on*`, no namespace) leaves the attribute allow-lists, or goes to
/// the deny-list when the filter has no global allow-list; `javascript:` URLs go.
pub fn remove_unsafe(filter: &mut FragmentFilter, event_handlers: &[&str]) {
    for (namespace, name) in baseline_elements() {
        let key = (namespace, LocalName::from(name));
        match &mut filter.elements {
            Some(elements) => {
                elements.remove(&key);
            }
            None => {
                filter.remove_elements.insert(key.clone());
            }
        }
        filter.replace_with_children.remove(&key);
    }
    for handler in event_handlers {
        let key = (ns!(), LocalName::from(*handler));
        match &mut filter.attributes {
            Some(attributes) => {
                attributes.remove(&key);
            }
            None => {
                filter.remove_attributes.insert(key.clone());
            }
        }
        if let Some(elements) = &mut filter.elements {
            for rule in elements.values_mut() {
                if let Some(attributes) = &mut rule.attributes {
                    attributes.remove(&key);
                }
            }
        }
    }
    filter.javascript_urls = false;
}

const HTML_ELEMENTS: &[(&str, &[&str])] = &[
    ("a", &["href", "hreflang", "type"]),
    ("abbr", &[]),
    ("address", &[]),
    ("article", &[]),
    ("aside", &[]),
    ("b", &[]),
    ("bdi", &[]),
    ("bdo", &[]),
    ("blockquote", &["cite"]),
    ("body", &[]),
    ("br", &[]),
    ("caption", &[]),
    ("cite", &[]),
    ("code", &[]),
    ("col", &["span"]),
    ("colgroup", &["span"]),
    ("data", &["value"]),
    ("dd", &[]),
    ("del", &["cite", "datetime"]),
    ("dfn", &[]),
    ("div", &[]),
    ("dl", &[]),
    ("dt", &[]),
    ("em", &[]),
    ("figcaption", &[]),
    ("figure", &[]),
    ("footer", &[]),
    ("h1", &[]),
    ("h2", &[]),
    ("h3", &[]),
    ("h4", &[]),
    ("h5", &[]),
    ("h6", &[]),
    ("head", &[]),
    ("header", &[]),
    ("hgroup", &[]),
    ("hr", &[]),
    ("html", &[]),
    ("i", &[]),
    ("ins", &["cite", "datetime"]),
    ("kbd", &[]),
    ("li", &["value"]),
    ("main", &[]),
    ("mark", &[]),
    ("menu", &[]),
    ("nav", &[]),
    ("ol", &["reversed", "start", "type"]),
    ("p", &[]),
    ("pre", &[]),
    ("q", &[]),
    ("rp", &[]),
    ("rt", &[]),
    ("ruby", &[]),
    ("s", &[]),
    ("samp", &[]),
    ("search", &[]),
    ("section", &[]),
    ("small", &[]),
    ("span", &[]),
    ("strong", &[]),
    ("sub", &[]),
    ("sup", &[]),
    ("table", &[]),
    ("tbody", &[]),
    ("td", &["colspan", "headers", "rowspan"]),
    ("tfoot", &[]),
    ("th", &["abbr", "colspan", "headers", "rowspan", "scope"]),
    ("thead", &[]),
    ("time", &["datetime"]),
    ("title", &[]),
    ("tr", &[]),
    ("u", &[]),
    ("ul", &[]),
    ("var", &[]),
    ("wbr", &[]),
];

const SVG_ELEMENTS: &[(&str, &[&str])] = &[
    ("a", &["href", "hreflang", "type"]),
    ("circle", &["cx", "cy", "pathLength", "r"]),
    ("defs", &[]),
    ("desc", &[]),
    ("ellipse", &["cx", "cy", "pathLength", "rx", "ry"]),
    ("foreignObject", &["height", "width", "x", "y"]),
    ("g", &[]),
    ("line", &["pathLength", "x1", "x2", "y1", "y2"]),
    ("marker", &["markerHeight", "markerUnits", "markerWidth", "orient", "preserveAspectRatio", "refX", "refY", "viewBox"]),
    ("metadata", &[]),
    ("path", &["d", "pathLength"]),
    ("polygon", &["pathLength", "points"]),
    ("polyline", &["pathLength", "points"]),
    ("rect", &["height", "pathLength", "rx", "ry", "width", "x", "y"]),
    ("svg", &["height", "preserveAspectRatio", "viewBox", "width", "x", "y"]),
    ("text", &["dx", "dy", "lengthAdjust", "rotate", "textLength", "x", "y"]),
    ("textPath", &["lengthAdjust", "method", "path", "side", "spacing", "startOffset", "textLength"]),
    ("title", &[]),
    ("tspan", &["dx", "dy", "lengthAdjust", "rotate", "textLength", "x", "y"]),
];

const MATHML_ELEMENTS: &[(&str, &[&str])] = &[
    ("a", &["href", "hreflang", "type"]),
    ("math", &[]),
    ("merror", &[]),
    ("mfrac", &[]),
    ("mi", &[]),
    ("mmultiscripts", &[]),
    ("mn", &[]),
    ("mo", &["fence", "form", "largeop", "lspace", "maxsize", "minsize", "movablelimits", "rspace", "separator", "stretchy", "symmetric"]),
    ("mover", &["accent"]),
    ("mpadded", &["depth", "height", "lspace", "voffset", "width"]),
    ("mphantom", &[]),
    ("mprescripts", &[]),
    ("mroot", &[]),
    ("mrow", &[]),
    ("ms", &[]),
    ("mspace", &["depth", "height", "width"]),
    ("msqrt", &[]),
    ("mstyle", &[]),
    ("msub", &[]),
    ("msubsup", &[]),
    ("msup", &[]),
    ("mtable", &[]),
    ("mtd", &["columnspan", "rowspan"]),
    ("mtext", &[]),
    ("mtr", &[]),
    ("munder", &["accentunder"]),
    ("munderover", &["accent", "accentunder"]),
    ("semantics", &[]),
];

const ATTRIBUTES: &[&str] = &[
    "alignment-baseline", "baseline-shift", "clip-path", "clip-rule", "color", "color-interpolation",
    "cursor", "dir", "direction", "display", "displaystyle", "dominant-baseline", "fill", "fill-opacity",
    "fill-rule", "font-family", "font-size", "font-size-adjust", "font-stretch", "font-style",
    "font-variant", "font-weight", "lang", "letter-spacing", "marker-end", "marker-mid", "marker-start",
    "mathbackground", "mathcolor", "mathsize", "opacity", "paint-order", "pointer-events", "scriptlevel",
    "shape-rendering", "stop-color", "stop-opacity", "stroke", "stroke-dasharray", "stroke-dashoffset",
    "stroke-linecap", "stroke-linejoin", "stroke-miterlimit", "stroke-opacity", "stroke-width",
    "text-anchor", "text-decoration", "text-overflow", "text-rendering", "title", "transform",
    "transform-origin", "unicode-bidi", "vector-effect", "visibility", "white-space", "word-spacing",
    "writing-mode",
];
