//! `Sec-Speculation-Tags` (https://wicg.github.io/nav-speculation/prefetch.html#sec-speculation-tags-header).
//!
//! The header of a speculative request lists the tags of every rule that asked for the URL: a
//! structured field list whose members are strings (a rule's tag, else its set's) or `null` (a
//! tagless rule). The list is deduplicated and sorted: `null` first, then the strings by code
//! unit (tags are printable ASCII, so bytewise). The prefetch driver sends it with one call,
//! [`tag_header`], over the effective tag of each candidate that asked for the request.
//!
//! # Invalid tags (WPT `speculation-tags/invalid-tags`, `deduped-and-sorted-tags`)
//! A tag is valid when it is a string of printable ASCII (0x20 to 0x7E). A rule whose `tag` is
//! present and invalid (a number, an object, `null`, a control character, non-ASCII) is an
//! invalid rule: the parser skips it, so it contributes no tag and no request (not even a `null`
//! one). A rule set whose own `tag` is invalid is invalid as a whole: its parse fails, the
//! script element fires `error`, and none of its rules applies. This is in `speculation_rules::parse`.
//!
//! A request that crosses to another site sends no tags at all (the caller leaves the header out).

#![allow(dead_code)]

/// The `Sec-Speculation-Tags` value for the effective tags (`None`: tagless) of the candidates
/// behind a request; `None` for no candidates (the header is left out).
pub fn tag_header(rule_tags: &[Option<String>]) -> Option<String> {
    if rule_tags.is_empty() {
        return None;
    }
    let tagless = rule_tags.iter().any(Option::is_none);
    let mut tags: Vec<&str> = rule_tags.iter().filter_map(|t| t.as_deref()).collect();
    tags.sort_unstable();
    tags.dedup();
    let mut members: Vec<String> = Vec::with_capacity(tags.len() + 1);
    if tagless {
        members.push("null".to_owned());
    }
    members.extend(tags.into_iter().map(sf_string));
    Some(members.join(", "))
}

/// RFC 8941 serialization of a string item: quoted, with `\` and `"` escaped.
fn sf_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        if c == '"' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn some(s: &str) -> Option<String> {
        Some(s.to_owned())
    }

    #[test]
    fn tagless_requests_send_null() {
        assert_eq!(tag_header(&[None]).as_deref(), Some("null"));
        assert_eq!(tag_header(&[]), None);
    }

    #[test]
    fn tags_are_deduped_sorted_and_null_first() {
        let tags = [some("def"), some("jkl"), some("def"), some("null"), None, some("abc"), some("ghi")];
        assert_eq!(tag_header(&tags).as_deref(), Some(r#"null, "abc", "def", "ghi", "jkl", "null""#));
    }

    #[test]
    fn strings_are_escaped() {
        assert_eq!(tag_header(&[some("\"\"\"")]).as_deref(), Some(r#""\"\"\"""#));
        assert_eq!(tag_header(&[some("\\")]).as_deref(), Some(r#""\\""#));
        assert_eq!(tag_header(&[some(""), some(" ")]).as_deref(), Some(r#""", " ""#));
    }
}
