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

    /// The language of element `id` (HTML "the language"): its nearest inclusive ancestor's
    /// `xml:lang` or `lang`, else the document's default language; `None` when unknown.
    pub fn language(&self, id: NodeId) -> DomResult<Option<String>> {
        let idx = self.check(id)?;
        Ok(self.language_idx(idx))
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

    /// Whether element `idx`'s directionality comes from its content (HTML "auto
    /// directionality"): `dir=auto`, or a `<bdi>` without a valid `dir`. The
    /// render tree carries these elements' directionality (`snapshot.rs`): the
    /// others' follows from their `dir` and their parent's.
    pub(crate) fn has_auto_directionality(&self, idx: u32) -> bool {
        let Some(e) = self.n(idx).element() else { return false };
        match self.dir_attr(idx) {
            Some("auto") => true,
            Some(_) => false,
            None => e.is_html() && e.name.local == local_name!("bdi"),
        }
    }

    pub(crate) fn directionality_idx(&self, idx: u32) -> Direction {
        let Some(e) = self.n(idx).element() else { return Direction::Ltr };
        match self.dir_attr(idx) {
            Some("ltr") => return Direction::Ltr,
            Some("rtl") => return Direction::Rtl,
            // HTML: `auto` (and a `<bdi>` without a valid `dir`) is left to right
            // when its content has no strong character, whatever its parent's.
            Some(_) => return self.auto_directionality(idx).unwrap_or(Direction::Ltr),
            None => {}
        }
        if e.is_html() {
            let local = &e.name.local;
            if *local == local_name!("bdi") {
                return self.auto_directionality(idx).unwrap_or(Direction::Ltr);
            }
            if *local == local_name!("input") {
                if let Ok(InputType::Tel) = self.input_type(self.id_of(idx)) {
                    return Direction::Ltr;
                }
            }
        }
        match self.dir_parent(idx) {
            Some(p) => self.directionality_idx(p),
            None => Direction::Ltr,
        }
    }

    /// Whether `idx` is an HTML "auto-directionality form-associated element",
    /// whose auto directionality is its value's.
    fn auto_directionality_control(&self, idx: u32) -> bool {
        let Some(e) = self.n(idx).element() else { return false };
        if !e.is_html() {
            return false;
        }
        match &*e.name.local {
            "textarea" => true,
            "input" => matches!(
                self.input_type(self.id_of(idx)),
                Ok(InputType::Hidden
                    | InputType::Text
                    | InputType::Search
                    | InputType::Tel
                    | InputType::Url
                    | InputType::Email
                    | InputType::Password
                    | InputType::Submit
                    | InputType::Reset
                    | InputType::Button)
            ),
            _ => false,
        }
    }

    /// HTML "auto directionality": a text control's value's, a slot's assigned
    /// nodes', or else the contained text's; `None` when nothing decides.
    fn auto_directionality(&self, idx: u32) -> Option<Direction> {
        if self.auto_directionality_control(idx) {
            let value = self.control_value_idx(idx);
            // Right to left when an R or AL character comes before any L one.
            return match text_direction(&value) {
                Some(d) => Some(d),
                None if value.is_empty() => None,
                None => Some(Direction::Ltr),
            };
        }
        if self.is_slot(idx) && self.in_shadow_tree(idx) {
            let assigned = self.slot_assigned(idx);
            if !assigned.is_empty() {
                for n in assigned {
                    let d = match &self.n(n).kind {
                        NodeKind::Text(t) | NodeKind::CData(t) => text_direction(t),
                        NodeKind::Element(_) => self.contained_text_direction(n, true),
                        _ => None,
                    };
                    if d.is_some() {
                        return d;
                    }
                }
                return None;
            }
        }
        self.contained_text_direction(idx, false)
    }

    /// Whether the contained text of an auto element skips element `n` and
    /// its subtree: `bdi`, `script`, `style` and `textarea` elements and
    /// elements with a valid `dir`.
    fn skipped_by_auto(&self, n: u32) -> bool {
        let Some(e) = self.n(n).element() else { return false };
        (e.is_html() && matches!(&*e.name.local, "bdi" | "script" | "style" | "textarea" | "template"))
            || self.dir_attr(n).is_some()
    }

    /// HTML "contained text auto directionality" of `root`: the first strong
    /// character of the text of its descendants in tree order, leaving out the
    /// subtrees `skipped_by_auto` names (and `root`'s whole content when
    /// `can_exclude_root` and `root` itself is one); a slot in a shadow tree
    /// gives its host's directionality.
    fn contained_text_direction(&self, root: u32, can_exclude_root: bool) -> Option<Direction> {
        if can_exclude_root && self.skipped_by_auto(root) {
            return None;
        }
        let mut stack: Vec<u32> = self.child_idx_iter(root).collect::<Vec<_>>();
        stack.reverse();
        while let Some(n) = stack.pop() {
            match &self.n(n).kind {
                NodeKind::Text(t) | NodeKind::CData(t) => {
                    if let Some(d) = text_direction(t) {
                        return Some(d);
                    }
                }
                NodeKind::Element(_) => {
                    if self.skipped_by_auto(n) {
                        continue;
                    }
                    if self.is_slot(n) && self.in_shadow_tree(n) {
                        let host = match &self.n(self.root_idx(n)).kind {
                            NodeKind::Fragment(f) => f.host,
                            _ => NIL,
                        };
                        if host != NIL {
                            return Some(self.directionality_idx(host));
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
        assert!(!lang_range_matches("fr-ninechars", "fr"), "a subtag over 8 characters");
        assert!(lang_range_matches("fr-eightchr", "fr"), "8 characters are allowed");
    }

    #[test]
    fn text_directions() {
        assert_eq!(text_direction("abc"), Some(Direction::Ltr));
        assert_eq!(text_direction("123 \u{5d0}"), Some(Direction::Rtl));
        assert_eq!(text_direction("123"), None);
    }
}
