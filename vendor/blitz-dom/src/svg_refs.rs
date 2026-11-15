//! pyro: SVG sources whose clip path, mask or filter references form a cycle longer than usvg
//! breaks. usvg sets a reference that closes a cycle of one or two elements to `none`
//! (`fix_recursive_links`), but follows a longer one without end and overflows the stack
//! (WPT css/css-masking/clip-path-svg-content/clip-path-recursion-001.svg: clip0, clip, clip2),
//! which ended the browser. The source is rewritten first as usvg would treat it: every
//! reference that closes a cycle becomes `none`.
//!
//! pyro: a `filter` that references no element is `none` as well: Chromium draws the element
//! unfiltered (Filter Effects: the filter chain is ignored), where usvg leaves the element out (WPT
//! css/filter-effects/filter-invalid).

use std::borrow::Cow;
use std::collections::HashMap;
use std::ops::Range;

use usvg::roxmltree;

const SVG_NS: &str = "http://www.w3.org/2000/svg";

/// `source` with the references that close a cycle among clip paths, masks or filters set to
/// `none`; borrowed when there is none (or it is not XML usvg would read).
pub(crate) fn break_reference_cycles(source: &[u8]) -> Cow<'_, [u8]> {
    let Ok(text) = std::str::from_utf8(source) else { return Cow::Borrowed(source) };
    if !text.contains("url(") || !["clip-path", "mask", "filter"].iter().any(|p| text.contains(p)) {
        return Cow::Borrowed(source);
    }
    let options = roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() };
    let Ok(doc) = roxmltree::Document::parse_with_options(text, options) else {
        return Cow::Borrowed(source);
    };
    let mut cuts = Vec::new();
    for (tag, property) in [("clipPath", "clip-path"), ("mask", "mask"), ("filter", "filter")] {
        cuts.extend(cycle_closing_references(&doc, tag, property));
    }
    cuts.extend(missing_filter_references(&doc));
    if cuts.is_empty() {
        return Cow::Borrowed(source);
    }
    cuts.sort_by_key(|range| range.start);
    cuts.dedup();
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for cut in cuts {
        if cut.start < at {
            continue;
        }
        out.push_str(&text[at..cut.start]);
        out.push_str("none");
        at = cut.end;
    }
    out.push_str(&text[at..]);
    Cow::Owned(out.into_bytes())
}

/// The source ranges of the `property` references (attribute values, or values in a `style`
/// attribute) that close a cycle among the `tag` elements: a depth-first search from each,
/// whose back edges they are. Without them the references form no cycle.
fn cycle_closing_references(
    doc: &roxmltree::Document<'_>,
    tag: &str,
    property: &str,
) -> Vec<Range<usize>> {
    let elements: Vec<roxmltree::Node<'_, '_>> = doc
        .descendants()
        .filter(|n| {
            // As usvg reads them: in the SVG namespace or none (blitz serializes an inline
            // `<svg>` without `xmlns`).
            n.is_element()
                && n.tag_name().name() == tag
                && matches!(n.tag_name().namespace(), None | Some(SVG_NS))
        })
        .collect();
    if elements.is_empty() {
        return Vec::new();
    }
    let index: HashMap<&str, usize> =
        elements.iter().enumerate().filter_map(|(i, n)| Some((n.attribute("id")?, i))).collect();
    // The references each element's content (itself included) makes: (target, source range).
    let edges: Vec<Vec<(usize, Range<usize>)>> = elements
        .iter()
        .map(|element| {
            element
                .descendants()
                .filter(|n| n.is_element())
                .flat_map(|n| references(n, property))
                .filter_map(|(id, range)| Some((*index.get(id)?, range)))
                .collect()
        })
        .collect();
    // 0: unvisited, 1: on the search's path, 2: done.
    let mut state = vec![0u8; elements.len()];
    let mut cuts = Vec::new();
    for start in 0..elements.len() {
        if state[start] != 0 {
            continue;
        }
        // (element, next edge to follow)
        let mut stack = vec![(start, 0)];
        state[start] = 1;
        while let Some((node, next)) = stack.pop() {
            let Some((target, range)) = edges[node].get(next) else {
                state[node] = 2;
                continue;
            };
            stack.push((node, next + 1));
            match state[*target] {
                0 => {
                    state[*target] = 1;
                    stack.push((*target, 0));
                }
                1 => cuts.push(range.clone()),
                _ => {}
            }
        }
    }
    cuts
}

/// pyro: the source ranges of the `filter` values that are one reference, to an id no element has
/// (a list of several is left to usvg).
fn missing_filter_references(doc: &roxmltree::Document<'_>) -> Vec<Range<usize>> {
    let ids: std::collections::HashSet<&str> =
        doc.descendants().filter(|n| n.is_element()).filter_map(|n| n.attribute("id")).collect();
    let single = |id: &str| !id.contains(|c: char| c.is_whitespace() || c == '(' || c == ')');
    doc.descendants()
        .filter(|n| n.is_element())
        .flat_map(|n| references(n, "filter"))
        .filter(|(id, _)| single(id) && !ids.contains(id))
        .map(|(_, range)| range)
        .collect()
}

/// The `url(#id)` references `node` makes with `property`, as an attribute or a declaration of
/// its `style` attribute: the id and the source range of the value.
fn references<'a>(node: roxmltree::Node<'a, '_>, property: &str) -> Vec<(&'a str, Range<usize>)> {
    let mut found = Vec::new();
    for attr in node.attributes() {
        if attr.namespace().is_some() {
            continue;
        }
        if attr.name() == property {
            let range = attr.range_value();
            let raw = &node.document().input_text()[range.clone()];
            if let Some(id) = url_id(raw) {
                found.push((id, range));
            }
        } else if attr.name() == "style" {
            let range = attr.range_value();
            let raw = &node.document().input_text()[range.clone()];
            let mut offset = range.start;
            for declaration in raw.split_inclusive(';') {
                let body = declaration.trim_end_matches(';');
                if let Some((name, value)) = body.split_once(':') {
                    if name.trim() == property {
                        if let Some(id) = url_id(value) {
                            let start = offset + name.len() + 1;
                            found.push((id, start..start + value.len()));
                        }
                    }
                }
                offset += declaration.len();
            }
        }
    }
    found
}

/// pyro: `source` with the `divisor` of each `feConvolveMatrix` that is zero (an error, which
/// Chromium reads as the default) set to the default, the sum of the kernel's values (1 when that
/// is zero); usvg leaves such a primitive out of its filter. `None` when there is none.
pub(crate) fn default_zero_divisors(source: &[u8]) -> Option<Vec<u8>> {
    let text = std::str::from_utf8(source).ok()?;
    if !text.contains("divisor") {
        return None;
    }
    let options = roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() };
    let doc = roxmltree::Document::parse_with_options(text, options).ok()?;
    let mut cuts: Vec<(Range<usize>, String)> = Vec::new();
    for node in doc.descendants().filter(|n| n.is_element() && n.tag_name().name() == "feConvolveMatrix") {
        let Some(divisor) = node.attributes().find(|a| a.name() == "divisor" && a.namespace().is_none()) else {
            continue;
        };
        if divisor.value().trim().parse::<f32>().ok() != Some(0.0) {
            continue;
        }
        let sum: f32 = node
            .attribute("kernelMatrix")
            .unwrap_or("")
            .split(|c: char| c.is_ascii_whitespace() || c == ',')
            .filter_map(|v| v.parse::<f32>().ok())
            .sum();
        let default = if sum.abs() < 1e-6 { 1.0 } else { sum };
        cuts.push((divisor.range_value(), default.to_string()));
    }
    if cuts.is_empty() {
        return None;
    }
    cuts.sort_by_key(|(range, _)| range.start);
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for (range, value) in cuts {
        out.push_str(&text[at..range.start]);
        out.push_str(&value);
        at = range.end;
    }
    out.push_str(&text[at..]);
    Some(out.into_bytes())
}

/// The id of a `url(#id)` value (quoted or not).
fn url_id(value: &str) -> Option<&str> {
    let inner = value.trim().strip_prefix("url(")?.strip_suffix(')')?.trim();
    let inner = inner.trim_matches(|c| c == '"' || c == '\'');
    inner.strip_prefix('#').filter(|id| !id.is_empty())
}

#[cfg(test)]
mod tests {
    use super::break_reference_cycles;

    fn broken(svg: &str) -> String {
        String::from_utf8(break_reference_cycles(svg.as_bytes()).into_owned()).unwrap()
    }

    #[test]
    fn a_cycle_of_three_clip_paths_loses_the_reference_that_closes_it() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg">
<clipPath id="a"><rect clip-path="url(#b)"/></clipPath>
<clipPath id="b"><rect style="fill:red; clip-path: url('#c')"/></clipPath>
<clipPath id="c"><rect clip-path="url(#a)"/></clipPath>
<rect clip-path="url(#a)"/></svg>"#;
        let out = broken(svg);
        assert_eq!(out.matches("none").count(), 1, "{out}");
        assert!(out.contains(r#"<clipPath id="c"><rect clip-path="none"/>"#), "{out}");
        assert!(out.ends_with(r#"<rect clip-path="url(#a)"/></svg>"#));
        usvg::Tree::from_str(&out, &usvg::Options::default()).unwrap();
    }

    /// pyro: WPT filter-invalid: a filter that references nothing is `none` (the element is drawn
    /// unfiltered, where usvg left it out).
    #[test]
    fn a_filter_referencing_nothing_is_none() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg"><filter id="f"><feFlood/></filter>
<rect style="filter:url(#not-found)" width="10" height="10"/><rect filter="url(#f)" width="5" height="5"/></svg>"#;
        let out = broken(svg);
        assert!(out.contains(r#"<rect style="filter:none" width="10""#), "{out}");
        assert!(out.contains(r#"<rect filter="url(#f)" width="5""#), "{out}");
        let tree = usvg::Tree::from_str(&out, &usvg::Options::default()).unwrap();
        assert_eq!(tree.root().children().len(), 2, "both rects are drawn");
        // A list (WPT svg-multiple-filter-functions) is not one reference.
        let list = r#"<svg xmlns="http://www.w3.org/2000/svg"><filter id="a"><feFlood/></filter>
<rect filter="url(#a) hue-rotate(90deg)" width="5" height="5"/></svg>"#;
        assert!(matches!(break_reference_cycles(list.as_bytes()), std::borrow::Cow::Borrowed(_)));
    }

    /// pyro: WPT feconvolve-divisor: a zero divisor is the kernel's sum.
    #[test]
    fn a_zero_divisor_is_the_default() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg"><filter id="f"><feConvolveMatrix kernelMatrix="1 1 1  1 1 1  1 1 1" divisor="0"/></filter></svg>"#;
        let out = String::from_utf8(super::default_zero_divisors(svg).unwrap()).unwrap();
        assert!(out.contains(r#"divisor="9""#), "{out}");
        assert!(super::default_zero_divisors(br#"<svg><feConvolveMatrix divisor="2"/></svg>"#).is_none());
    }

    #[test]
    fn sources_without_a_cycle_are_kept() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg"><clipPath id="a"><rect/></clipPath>
<mask id="m"><rect clip-path="url(#a)"/></mask><rect mask="url(#m)" clip-path="url(#a)"/></svg>"#;
        assert!(matches!(break_reference_cycles(svg.as_bytes()), std::borrow::Cow::Borrowed(_)));
        assert!(matches!(break_reference_cycles(b"not xml url("), std::borrow::Cow::Borrowed(_)));
    }
}
