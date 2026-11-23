//! pyro: the language of an element, for `:lang()` (HTML, "The lang and xml:lang attributes"):
//! the nearest inclusive ancestor's `xml:lang`, or `lang` on an HTML, SVG or MathML element, else
//! the document's default language, which a `<meta http-equiv="content-language">` sets (else the
//! HTTP `Content-Language` of the document's response). A range
//! matches by RFC 4647 extended filtering, ASCII case-insensitively, as in Chromium.

use markup5ever::{LocalName, local_name, ns};

use crate::node::Node;

/// The `xml:lang`, else `lang`, attribute of `node` itself.
pub(crate) fn own_lang(node: &Node) -> Option<&str> {
    let elem = node.element_data()?;
    let accepts_lang =
        elem.name.ns == ns!(html) || elem.name.ns == ns!(svg) || elem.name.ns == ns!(mathml);
    let mut lang = None;
    for attr in elem.attrs() {
        if attr.name.local == local_name!("lang") {
            if attr.name.ns == ns!(xml) {
                return Some(attr.value.as_str());
            }
            if attr.name.ns == ns!() && accepts_lang {
                lang = lang.or(Some(attr.value.as_str()));
            }
        }
    }
    lang
}

/// The language of element `node` (`None`: unknown).
pub(crate) fn language(node: &Node) -> Option<&str> {
    let mut current = node;
    loop {
        if let Some(lang) = own_lang(current) {
            return Some(lang);
        }
        match current.parent {
            // pyro: a node assigned to a slot takes its language from its node tree (the host the
            // slot's shadow tree is attached to), not from the slot it renders in (WPT selectors
            // i18n/lang-pseudo-class-across-shadow-boundaries).
            Some(parent) if current.with(parent).is_assigned_slot() => {
                match current.with(parent).containing_shadow_host_id() {
                    Some(host) => current = current.with(host),
                    None => current = current.with(parent),
                }
            }
            Some(parent) => current = current.with(parent),
            // `current` is the document.
            None => return default_language(current),
        }
    }
}

/// The default language of `document`: the pragma-set one, else the protocol's (HTTP
/// `Content-Language`, [`crate::DocumentConfig::content_language`]).
fn default_language(document: &Node) -> Option<&str> {
    pragma_language(document).or_else(|| document.tree().content_language.as_deref())
}

/// The pragma-set default language of `document`: the `content` of the last
/// `<meta http-equiv="content-language">` of its `<head>`, up to the first space,
/// unless it lists several (a comma).
fn pragma_language(document: &Node) -> Option<&str> {
    fn child<'a>(node: &'a Node, name: &LocalName) -> Option<&'a Node> {
        node.children
            .iter()
            .map(|&id| node.with(id))
            .find(|n| n.data.is_element_with_tag_name(name))
    }
    let head = child(child(document, &local_name!("html"))?, &local_name!("head"))?;
    head.children.iter().rev().map(|&id| head.with(id)).find_map(|meta| {
        let elem = meta.element_data().filter(|e| e.name.local == local_name!("meta"))?;
        elem.attr(local_name!("http-equiv"))
            .filter(|v| v.trim().eq_ignore_ascii_case("content-language"))?;
        let content = elem.attr(local_name!("content")).filter(|c| !c.contains(','))?;
        content.split_ascii_whitespace().next()
    })
}

/// RFC 4647 §3.3.2 extended filtering of language tag `tag` by `range`, ASCII
/// case-insensitively, with the implicit wildcard of Selectors 4 (`:lang(en)`
/// matches `en-US`).
pub(crate) fn range_matches(tag: &str, range: &str) -> bool {
    if range.is_empty() || tag.is_empty() {
        return false;
    }
    // A subtag longer than 8 characters makes the tag ill-formed (BCP 47): Chrome matches no
    // range against it (WPT css/selectors/selectors-4 `lang-singleton-subtag-matching`).
    if tag.split('-').any(|t| t.len() > 8) {
        return false;
    }
    let mut tags = tag.split('-');
    let mut ranges = range.split('-');
    let (Some(t0), Some(r0)) = (tags.next(), ranges.next()) else { return false };
    if r0 != "*" && !t0.eq_ignore_ascii_case(r0) {
        return false;
    }
    for r in ranges {
        if r == "*" {
            continue;
        }
        loop {
            let Some(t) = tags.next() else { return false };
            if t.eq_ignore_ascii_case(r) {
                break;
            }
            // A singleton ends the subtags a range may skip.
            if t.len() == 1 {
                return false;
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::range_matches;

    #[test]
    fn ranges_match_by_extended_filtering_ascii_case_insensitively() {
        for (tag, range) in [
            ("es", "es"),
            ("es-MX", "es"),
            ("ES", "es"),
            ("en-GB-scouse", "en-GB"),
            ("en-us", "en"),
            ("En", "en"),
            ("de-Latn-DE", "de-DE"),
            ("de-DE", "*-DE"),
        ] {
            assert!(range_matches(tag, range), "{tag} {range}");
        }
        for (tag, range) in [
            ("MX-es", "es"),
            ("en-US", "en-GB"),
            ("en", "en-GB"),
            ("fi", "fİ"),
            ("fi", "Fı"),
            ("kl", "\u{212a}l"),
            ("de-x-DE", "de-DE"),
            ("", "en"),
            ("en", ""),
        ] {
            assert!(!range_matches(tag, range), "{tag} {range}");
        }
    }
}
