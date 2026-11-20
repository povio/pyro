//! pyro: SVG sources whose clip path, mask or filter references form a cycle longer than usvg
//! breaks. usvg sets a reference that closes a cycle of one or two elements to `none`
//! (`fix_recursive_links`), but follows a longer one without end and overflows the stack
//! (WPT css/css-masking/clip-path-svg-content/clip-path-recursion-001.svg: clip0, clip, clip2),
//! which ended the browser. The source is rewritten first as usvg would treat it: every
//! reference that closes a cycle becomes `none`.
//!
//! pyro: patterns too. usvg sets a `fill` or `stroke` inside a pattern that refers back to the
//! pattern, directly or through one other element, to `none` (`fix_recursive_patterns`), but not
//! one that comes back through a pattern's `href` (a pattern without children draws the children
//! of the pattern its `href` names): WPT svg/crashtests/chrome-bug-565674533.html, where `p2`'s
//! line is stroked with `p1` and `p1` takes its content from `p2`, overflowed the stack. Here a
//! paint that closes a cycle among patterns loses its reference (its fallback colour stays, or it
//! becomes `none`), and an `href` that closes one stops naming an element.
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
    if !text.contains("url(") || !["clip-path", "mask", "filter", "pattern"].iter().any(|p| text.contains(p)) {
        return Cow::Borrowed(source);
    }
    let options = roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() };
    let Ok(doc) = roxmltree::Document::parse_with_options(text, options) else {
        return Cow::Borrowed(source);
    };
    // (source range, what replaces it)
    let mut cuts: Vec<(Range<usize>, &'static str)> = Vec::new();
    for (tag, property) in [("clipPath", "clip-path"), ("mask", "mask"), ("filter", "filter")] {
        cuts.extend(cycle_closing_references(&doc, tag, |element| content_references(element, property)));
    }
    cuts.extend(cycle_closing_references(&doc, "pattern", pattern_references));
    cuts.extend(missing_filter_references(&doc).into_iter().map(|range| (range, "none")));
    if cuts.is_empty() {
        return Cow::Borrowed(source);
    }
    cuts.sort_by_key(|(range, _)| range.start);
    cuts.dedup();
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for (cut, with) in cuts {
        if cut.start < at {
            continue;
        }
        out.push_str(&text[at..cut.start]);
        out.push_str(with);
        at = cut.end;
    }
    out.push_str(&text[at..]);
    Cow::Owned(out.into_bytes())
}

/// A reference to an element by id: the id, the source range to replace when the reference
/// closes a cycle, and what replaces it.
type Reference<'a> = (&'a str, Range<usize>, &'static str);

/// The references (`references_of` an element: those its content makes) that close a cycle among
/// the `tag` elements, as source ranges and their replacements: a depth-first search from each
/// element, whose back edges they are. Without them the references form no cycle.
fn cycle_closing_references<'a, 'input>(
    doc: &'a roxmltree::Document<'input>,
    tag: &str,
    references_of: impl Fn(roxmltree::Node<'a, 'input>) -> Vec<Reference<'a>>,
) -> Vec<(Range<usize>, &'static str)> {
    let elements: Vec<roxmltree::Node<'a, 'input>> = doc
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
    // The references each element's content (itself included) makes: (target, source range,
    // replacement).
    let edges: Vec<Vec<(usize, Range<usize>, &'static str)>> = elements
        .iter()
        .map(|element| {
            references_of(*element)
                .into_iter()
                .filter_map(|(id, range, with)| Some((*index.get(id)?, range, with)))
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
            let Some((target, range, with)) = edges[node].get(next) else {
                state[node] = 2;
                continue;
            };
            stack.push((node, next + 1));
            match state[*target] {
                0 => {
                    state[*target] = 1;
                    stack.push((*target, 0));
                }
                1 => cuts.push((range.clone(), *with)),
                _ => {}
            }
        }
    }
    cuts
}

/// The `property` references the content of `element` (itself included) makes, each replaced by
/// `none` when it closes a cycle.
fn content_references<'a>(element: roxmltree::Node<'a, '_>, property: &str) -> Vec<Reference<'a>> {
    element
        .descendants()
        .filter(|n| n.is_element())
        .flat_map(|n| references(n, property))
        .map(|(id, range)| (id, range, "none"))
        .collect()
}

/// pyro: the references a `<pattern>` makes to patterns its drawing needs: the `fill` and `stroke`
/// paints of its content (itself included: they inherit), and its own `href` (`xlink:href`), whose
/// pattern's children it draws when it has none. A paint that closes a cycle loses its `url()`
/// and keeps its fallback (`none` without one); an `href` becomes `none`, which names no element.
fn pattern_references<'a>(pattern: roxmltree::Node<'a, '_>) -> Vec<Reference<'a>> {
    let mut found = Vec::new();
    for node in pattern.descendants().filter(|n| n.is_element()) {
        for property in ["fill", "stroke"] {
            for (id, range, fallback) in paint_references(node, property) {
                found.push((id, range, if fallback { "" } else { "none" }));
            }
        }
    }
    for attr in pattern.attributes() {
        if attr.name() == "href" && matches!(attr.namespace(), None | Some("http://www.w3.org/1999/xlink")) {
            if let Some(id) = attr.value().trim().strip_prefix('#').filter(|id| !id.is_empty()) {
                found.push((id, attr.range_value(), "none"));
            }
        }
    }
    found
}

/// The `url(#id)` paint `node` has for `property` (`fill` or `stroke`), as an attribute or a
/// declaration of its `style` attribute: the id, and the source range of the value when the
/// paint is the reference alone (`false`), or of its `url()` when a fallback follows (`true`).
fn paint_references<'a>(node: roxmltree::Node<'a, '_>, property: &str) -> Vec<(&'a str, Range<usize>, bool)> {
    // `value` at `start` in the source.
    fn paint(value: &str, start: usize) -> Option<(&str, Range<usize>, bool)> {
        let lead = value.len() - value.trim_start().len();
        let rest = &value[lead..];
        let close = rest.find(')')?;
        let id = url_id(&rest[..=close])?;
        match rest[close + 1..].trim().is_empty() {
            true => Some((id, start..start + value.len(), false)),
            false => Some((id, start + lead..start + lead + close + 1, true)),
        }
    }
    let mut found = Vec::new();
    for attr in node.attributes() {
        if attr.namespace().is_some() {
            continue;
        }
        let range = attr.range_value();
        let raw = &node.document().input_text()[range.clone()];
        if attr.name() == property {
            found.extend(paint(raw, range.start));
        } else if attr.name() == "style" {
            let mut offset = range.start;
            for declaration in raw.split_inclusive(';') {
                let body = declaration.trim_end_matches(';');
                if let Some((name, value)) = body.split_once(':') {
                    if name.trim() == property {
                        found.extend(paint(value, offset + name.len() + 1));
                    }
                }
                offset += declaration.len();
            }
        }
    }
    found
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

/// The first position of `needle` in `haystack`.
pub(crate) fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// pyro: `source` with each `light-dark(a, b)` (which usvg does not parse: the paint fell back to
/// black) replaced by `a`, or by `b` when `dark`. `None` when there is none.
/// pyro: the `<style>` sheets of an SVG image as usvg can apply them (its CSS has neither media
/// queries nor `:root`): a `@media (prefers-color-scheme: …)` block is unwrapped when it matches the
/// preferred scheme (`dark` or light) and dropped when it does not, and `:root` is the root `svg`
/// (WPT css/mediaqueries `prefers-color-scheme-svg-as-image`). Other `@media` blocks stay as they are.
/// `None`: nothing to change.
pub(crate) fn resolve_scheme_media(source: &[u8], dark: bool) -> Option<Vec<u8>> {
    if find_bytes(source, b"prefers-color-scheme").is_none() && find_bytes(source, b":root").is_none() {
        return None;
    }
    let mut out = Vec::with_capacity(source.len());
    let mut rest = source;
    while let Some(at) = find_bytes(rest, b"@media") {
        out.extend_from_slice(&rest[..at]);
        let after = &rest[at + b"@media".len()..];
        let Some(open) = after.iter().position(|&b| b == b'{' || b == b';') else {
            out.extend_from_slice(&rest[at..]);
            rest = &[];
            break;
        };
        let condition: Vec<u8> =
            after[..open].iter().filter(|b| !b.is_ascii_whitespace()).map(|b| b.to_ascii_lowercase()).collect();
        let scheme = match condition.as_slice() {
            b"(prefers-color-scheme:dark)" => Some(true),
            b"(prefers-color-scheme:light)" => Some(false),
            _ => None,
        };
        let (Some(scheme), b'{') = (scheme, after[open]) else {
            out.extend_from_slice(&rest[at..at + b"@media".len()]);
            rest = after;
            continue;
        };
        // The block's closing brace.
        let body = &after[open + 1..];
        let mut depth = 0usize;
        let mut end = None;
        for (i, &b) in body.iter().enumerate() {
            match b {
                b'{' => depth += 1,
                b'}' if depth == 0 => {
                    end = Some(i);
                    break;
                }
                b'}' => depth -= 1,
                _ => {}
            }
        }
        let Some(end) = end else {
            out.extend_from_slice(&rest[at..]);
            rest = &[];
            break;
        };
        if scheme == dark {
            out.extend_from_slice(&body[..end]);
        }
        rest = &body[end + 1..];
    }
    out.extend_from_slice(rest);
    // `:root` is the root element, the outermost `svg`.
    let mut replaced = Vec::with_capacity(out.len());
    let mut rest = out.as_slice();
    while let Some(at) = find_bytes(rest, b":root") {
        replaced.extend_from_slice(&rest[..at]);
        replaced.extend_from_slice(b"svg");
        rest = &rest[at + b":root".len()..];
    }
    replaced.extend_from_slice(rest);
    Some(replaced)
}

pub(crate) fn resolve_light_dark(source: &[u8], dark: bool) -> Option<Vec<u8>> {
    const NAME: &[u8] = b"light-dark(";
    find_bytes(source, NAME)?;
    let mut out = Vec::with_capacity(source.len());
    let mut rest = source;
    while let Some(at) = find_bytes(rest, NAME) {
        out.extend_from_slice(&rest[..at]);
        let args = &rest[at + NAME.len()..];
        // The top-level comma and the closing parenthesis.
        let (mut depth, mut comma, mut end) = (0usize, None, None);
        for (i, &b) in args.iter().enumerate() {
            match b {
                b'(' => depth += 1,
                b')' if depth == 0 => {
                    end = Some(i);
                    break;
                }
                b')' => depth -= 1,
                b',' if depth == 0 && comma.is_none() => comma = Some(i),
                _ => {}
            }
        }
        let (Some(comma), Some(end)) = (comma, end) else {
            out.extend_from_slice(&rest[at..]);
            rest = &[];
            break;
        };
        let chosen = if dark { &args[comma + 1..end] } else { &args[..comma] };
        out.extend_from_slice(chosen.trim_ascii());
        rest = &args[end + 1..];
    }
    out.extend_from_slice(rest);
    Some(out)
}

/// pyro: whether an SVG image's own `color-scheme` (on its root, as a property) lets it be dark.
pub(crate) fn allows_dark_scheme(source: &[u8]) -> bool {
    let mut rest = source;
    while let Some(at) = find_bytes(rest, b"color-scheme") {
        let tail = &rest[at + 12..];
        let value = &tail[..tail.iter().position(|&b| matches!(b, b';' | b'"' | b'\'' | b'}')).unwrap_or(tail.len())];
        if find_bytes(value, b"dark").is_some() {
            return true;
        }
        rest = tail;
    }
    false
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
    #[test]
    fn light_dark_picks_one_colour() {
        let src = b"<rect fill='light-dark(rgb(1, 2, 3), blue)' style='stroke: light-dark(red,green)'/>";
        let light = super::resolve_light_dark(src, false).unwrap();
        assert_eq!(light, b"<rect fill='rgb(1, 2, 3)' style='stroke: red'/>".to_vec());
        let dark = super::resolve_light_dark(src, true).unwrap();
        assert_eq!(dark, b"<rect fill='blue' style='stroke: green'/>".to_vec());
        assert!(super::resolve_light_dark(b"<rect fill='red'/>", true).is_none());
        assert!(super::allows_dark_scheme(b"<svg style='color-scheme: light dark'>"));
        assert!(!super::allows_dark_scheme(b"<svg style='color-scheme: light'>"));
    }

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

    /// pyro: WPT svg/crashtests/chrome-bug-565674533.html: `p2`'s line is stroked with `p1`, and
    /// `p1`, which has no children, draws `p2`'s through its `href`. usvg followed that without
    /// end (a stack overflow that ended the browser).
    #[test]
    fn a_pattern_cycle_through_href_loses_the_reference_that_closes_it() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg">
<pattern id="p2" width="1" height="1"><line id="l" y2="1" x2="1" stroke="url(#p1) green"/></pattern>
<pattern id="p1" width="2" height="1" href="#p2"/>
<polygon points="0,0 100,0 100,100 0,100" fill="url(#p1) lime"/></svg>"##;
        let out = broken(svg);
        assert!(out.contains(r#"<pattern id="p1" width="2" height="1" href="none"/>"#), "{out}");
        assert!(out.contains(r#"stroke="url(#p1) green""#), "the paint that does not close the cycle stays: {out}");
        assert!(out.ends_with(r#"fill="url(#p1) lime"/></svg>"#), "{out}");
        usvg::Tree::from_str(&out, &usvg::Options::default()).unwrap();

        // Through `xlink:href`, with the paint in a `style` attribute and no fallback, from the
        // other end: the paint closes the cycle.
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink">
<pattern id="a" width="1" height="1" xlink:href="#b"/>
<pattern id="b" width="1" height="1"><rect width="1" height="1" style="fill: url(#c)"/></pattern>
<pattern id="c" width="1" height="1"><rect width="1" height="1" fill="url(#a) red"/></pattern>
<rect width="10" height="10" fill="url(#a)"/></svg>"##;
        let out = broken(svg);
        assert!(out.contains(r#"<rect width="1" height="1" fill=" red"/>"#), "the fallback colour stays: {out}");
        assert!(out.contains(r##"xlink:href="#b""##) && out.contains("fill: url(#c)"), "{out}");
        usvg::Tree::from_str(&out, &usvg::Options::default()).unwrap();

        // Patterns that only use each other are kept.
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg"><pattern id="a" width="1" height="1"><rect width="1" height="1" fill="red"/></pattern>
<pattern id="b" width="1" height="1" href="#a"/><pattern id="c" width="1" height="1"><rect fill="url(#b)" stroke="url(#a)"/></pattern>
<rect width="10" height="10" fill="url(#c)"/></svg>"##;
        assert!(matches!(break_reference_cycles(svg.as_bytes()), std::borrow::Cow::Borrowed(_)));
    }

    #[test]
    fn sources_without_a_cycle_are_kept() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg"><clipPath id="a"><rect/></clipPath>
<mask id="m"><rect clip-path="url(#a)"/></mask><rect mask="url(#m)" clip-path="url(#a)"/></svg>"#;
        assert!(matches!(break_reference_cycles(svg.as_bytes()), std::borrow::Cow::Borrowed(_)));
        assert!(matches!(break_reference_cycles(b"not xml url("), std::borrow::Cow::Borrowed(_)));
    }
}
