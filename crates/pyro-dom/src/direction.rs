//! HTML directionality (`dir`, `dir=auto`, `<bdi>`) for `:dir()`, and the
//! language of an element for `:lang()`.

use markup5ever::local_name;

use crate::dom::Dom;
use crate::error::DomResult;
use crate::forms::InputType;
use crate::node::{NodeId, NodeKind, NIL};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Ltr,
    Rtl,
}

/// Bidi class of `c` reduced to strong left-to-right / right-to-left.
fn strong(c: char) -> Option<Direction> {
    use unicode_bidi::BidiClass as B;
    match unicode_bidi::bidi_class(c) {
        B::L => Some(Direction::Ltr),
        B::R | B::AL => Some(Direction::Rtl),
        _ => None,
    }
}

/// HTML "text directionality": the direction of the first strong character.
pub fn text_direction(text: &str) -> Option<Direction> {
    text.chars().find_map(strong)
}

impl Dom {
    /// The directionality of element `id` (HTML "the directionality").
    pub fn directionality(&self, id: NodeId) -> DomResult<Direction> {
        let idx = self.check(id)?;
        Ok(self.directionality_idx(idx))
    }

    fn dir_attr(&self, idx: u32) -> Option<&str> {
        let value = self.n(idx).element()?.attr_local(&local_name!("dir"))?;
        for k in ["ltr", "rtl", "auto"] {
            if value.eq_ignore_ascii_case(k) {
                return Some(k);
            }
        }
        None
    }

    /// Parent element in the flat tree (an assigned slot's host side, or the
    /// shadow host for a shadow root's children).
    fn dir_parent(&self, idx: u32) -> Option<u32> {
        let p = self.n(idx).parent;
        if p == NIL {
            return None;
        }
        match &self.n(p).kind {
            NodeKind::Element(_) => Some(p),
            NodeKind::Fragment(f) if f.shadow.is_some() => Some(f.host),
            _ => None,
        }
    }

    pub(crate) fn directionality_idx(&self, idx: u32) -> Direction {
        let Some(e) = self.n(idx).element() else { return Direction::Ltr };
        let dir = self.dir_attr(idx);
        match dir {
            Some("ltr") => return Direction::Ltr,
            Some("rtl") => return Direction::Rtl,
            _ => {}
        }
        let html = e.is_html();
        let local = &e.name.local;
        let is_bdi = html && *local == local_name!("bdi");
        if dir == Some("auto") || (is_bdi && dir.is_none()) {
            if let Some(d) = self.auto_directionality(idx) {
                return d;
            }
            if is_bdi && dir.is_none() {
                return Direction::Ltr;
            }
        }
        if html && *local == local_name!("input") && dir.is_none() {
            if let Ok(InputType::Tel) = self.input_type(self.id_of(idx)) {
                return Direction::Ltr;
            }
        }
        match self.dir_parent(idx) {
            Some(p) => self.directionality_idx(p),
            None => Direction::Ltr,
        }
    }

    /// HTML "auto directionality": from the value of a text control, or the
    /// first strong character of the contained text.
    fn auto_directionality(&self, idx: u32) -> Option<Direction> {
        let e = self.n(idx).element()?;
        if e.is_html() {
            let local = &e.name.local;
            if *local == local_name!("textarea") {
                return text_direction(&self.control_value_idx(idx));
            }
            if *local == local_name!("input") {
                return match self.input_type(self.id_of(idx)).ok()? {
                    InputType::Text | InputType::Search | InputType::Tel | InputType::Url | InputType::Email => {
                        text_direction(&self.control_value_idx(idx))
                    }
                    _ => None,
                };
            }
            if *local == local_name!("slot") {
                if let Some(d) = self.slot_assigned_direction(idx) {
                    return Some(d);
                }
            }
        }
        self.contained_text_direction(idx)
    }

    /// Direction of the first strong character among the nodes assigned to
    /// slot `idx`; `None` when none is assigned.
    fn slot_assigned_direction(&self, idx: u32) -> Option<Direction> {
        let assigned = self.slot_assigned(idx);
        if assigned.is_empty() {
            return None;
        }
        for n in assigned {
            let d = match &self.n(n).kind {
                NodeKind::Text(t) | NodeKind::CData(t) => text_direction(t),
                NodeKind::Element(_) => self.contained_text_direction(n),
                _ => None,
            };
            if d.is_some() {
                return d;
            }
        }
        None
    }

    /// HTML "contained text auto directionality": descendants' text in tree
    /// order, skipping `bdi`, `script`, `style`, `textarea` and elements
    /// with a `dir` attribute.
    fn contained_text_direction(&self, root: u32) -> Option<Direction> {
        let mut stack: Vec<u32> = self.child_idx_iter(root).collect::<Vec<_>>();
        stack.reverse();
        while let Some(n) = stack.pop() {
            match &self.n(n).kind {
                NodeKind::Text(t) | NodeKind::CData(t) => {
                    if let Some(d) = text_direction(t) {
                        return Some(d);
                    }
                }
                NodeKind::Element(e) => {
                    let skip = (e.is_html()
                        && matches!(
                            &*e.name.local,
                            "bdi" | "script" | "style" | "textarea" | "input" | "select" | "template"
                        ))
                        || self.dir_attr(n).is_some();
                    if skip {
                        continue;
                    }
                    if e.is_html() && e.name.local == local_name!("slot") {
                        if let Some(d) = self.slot_assigned_direction(n) {
                            return Some(d);
                        }
                        if !self.slot_assigned(n).is_empty() {
                            continue;
                        }
                    }
                    let children: Vec<u32> = self.child_idx_iter(n).collect();
                    stack.extend(children.into_iter().rev());
                }
                _ => {}
            }
        }
        None
    }

    /// The language of element `idx`: the nearest inclusive ancestor's
    /// `xml:lang` or `lang` attribute (the host's for a shadow tree), else
    /// the document's default language: a `<meta http-equiv=content-language>`
    /// pragma's, else the HTTP `Content-Language` (`Dom::set_content_language`),
    /// as blitz-dom's `lang.rs` resolves it for rendering.
    pub(crate) fn language_idx(&self, idx: u32) -> Option<String> {
        let mut cur = Some(idx);
        while let Some(i) = cur {
            if let Some(e) = self.n(i).element() {
                if let Some(a) = e.attrs.iter().find(|a| {
                    &*a.name.ns == "http://www.w3.org/XML/1998/namespace" && a.name.local == local_name!("lang")
                }) {
                    return Some(a.value.to_string());
                }
                if let Some(v) = e.attr_local(&local_name!("lang")).filter(|_| e.is_html() || e.name.ns.is_empty()) {
                    return Some(v.to_string());
                }
            }
            cur = self.dir_parent(i);
        }
        if !self.n(idx).is_connected() {
            return None;
        }
        let doc = self.n(idx).owner_doc;
        match &self.n(doc).kind {
            NodeKind::Document(d) => {
                self.pragma_language(d.head).or_else(|| d.content_language.as_deref()).map(str::to_owned)
            }
            _ => None,
        }
    }

    /// The pragma-set default language of the document whose `<head>` is
    /// `head` (NIL: none): the `content` of the last
    /// `<meta http-equiv="content-language">` child, up to the first space,
    /// unless it lists several (a comma).
    fn pragma_language(&self, head: u32) -> Option<&str> {
        if head == NIL {
            return None;
        }
        let mut c = self.n(head).last_child;
        while c != NIL {
            let lang =
                self.n(c).element().filter(|e| e.is_html() && e.name.local == local_name!("meta")).and_then(|e| {
                    e.attr_local(&local_name!("http-equiv"))
                        .filter(|v| v.trim().eq_ignore_ascii_case("content-language"))?;
                    e.attr_local(&local_name!("content")).filter(|v| !v.contains(','))?.split_ascii_whitespace().next()
                });
            if lang.is_some() {
                return lang;
            }
            c = self.n(c).prev;
        }
        None
    }

    /// `:lang(range)` for element `idx` (RFC 4647 extended filtering, with
    /// an implicit wildcard range).
    pub(crate) fn lang_matches(&self, idx: u32, ranges: &str) -> bool {
        let Some(lang) = self.language_idx(idx) else { return false };
        ranges.split(',').map(str::trim).any(|r| lang_range_matches(&lang, r))
    }
}

/// RFC 4647 §3.3.2 extended filtering of language tag `tag` against `range`,
/// with implicit wildcard semantics (`:lang(en)` matches `en-US`).
pub fn lang_range_matches(tag: &str, range: &str) -> bool {
    let range = range.trim_matches('"').trim_matches('\'');
    if range.is_empty() || tag.is_empty() {
        return false;
    }
    let mut tags = tag.split('-');
    let mut ranges = range.split('-');
    let (Some(t0), Some(r0)) = (tags.next(), ranges.next()) else { return false };
    if r0 != "*" && !t0.eq_ignore_ascii_case(r0) {
        return false;
    }
    let mut tags = tags.peekable();
    for r in ranges {
        if r == "*" {
            continue;
        }
        loop {
            let Some(t) = tags.next() else { return false };
            if t.eq_ignore_ascii_case(r) {
                break;
            }
            if t.len() == 1 {
                return false;
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extended_filtering() {
        assert!(lang_range_matches("en-US", "en"));
        assert!(lang_range_matches("en", "en"));
        assert!(!lang_range_matches("eng", "en"));
        assert!(lang_range_matches("fr-x", "fr-x"));
        assert!(lang_range_matches("fr-x-standard", "fr-x"));
        assert!(lang_range_matches("fr-standard", "fr-standard"));
        assert!(!lang_range_matches("fr-x-standard", "fr-standard"));
        assert!(lang_range_matches("de-CH-1996", "de-*-1996"));
        assert!(lang_range_matches("zh-Hant-TW", "*-Hant"));
    }

    #[test]
    fn text_directions() {
        assert_eq!(text_direction("abc"), Some(Direction::Ltr));
        assert_eq!(text_direction("123 \u{5d0}"), Some(Direction::Rtl));
        assert_eq!(text_direction("123"), None);
    }
}
