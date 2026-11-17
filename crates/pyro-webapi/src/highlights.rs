//! CSS Custom Highlight API 1 (`Highlight`, `CSS.highlights`) and URL Fragment Text Directives
//! (`#:~:text=`): the ranges a renderer paints as `::highlight(<name>)` and `::target-text`.
//!
//! `Highlight` and `HighlightRegistry` are made in JS (highlights.js) on a realm's first use;
//! each change hands the registry's entries to [`PageDom::highlights`] here. A fragment
//! navigation with a directive finds its text ([`apply_fragment_directive`]) into
//! [`PageDom::target_text`].

use std::rc::Rc;

use pyro_dom::{Dom, NodeId};
use pyro_js::{JsError, JsResult, Object, Persistent, Scope, Value};

use crate::page::PageDom;
use crate::range::RangeOwner;

/// Kept on the realm's `navigator` (a hidden property), itself kept per realm.
const KEY: &str = "lpHighlights";

/// One range of a custom highlight: a live `Range`'s, or a `StaticRange`'s ends.
pub(crate) enum HighlightRange {
    Live(Rc<RangeOwner>),
    Static((crate::collections::NodeHold, u32), (crate::collections::NodeHold, u32)),
}

impl HighlightRange {
    pub(crate) fn points(&self) -> ((NodeId, u32), (NodeId, u32)) {
        match self {
            HighlightRange::Live(owner) => (owner.live.start(), owner.live.end()),
            HighlightRange::Static((start, from), (end, to)) => ((start.id, *from), (end.id, *to)),
        }
    }
}

/// A registered highlight: its name, priority and ranges.
pub(crate) struct CustomHighlight {
    pub(crate) name: String,
    pub(crate) priority: i32,
    pub(crate) ranges: Vec<HighlightRange>,
}

fn unavailable() -> JsError {
    JsError::Type("Highlight is unavailable".into())
}

/// This realm's module (`{Highlight, HighlightRegistry, highlights}`), made on first use.
pub(crate) fn module<'s>(s: &Scope<'s>, page: &Rc<PageDom>) -> JsResult<Object<'s>> {
    let navigator = crate::window::navigator(s, page)?.as_object().ok_or_else(unavailable)?;
    if let Some(module) = s.get_hidden(navigator, KEY)?.and_then(|v| v.as_object()) {
        return Ok(module);
    }
    let natives = s.object();
    let is_range = s.function("isRange", |s, _this, a| {
        let v = a.get(0);
        Ok(s.boolean(s.unwrap::<crate::range::Range>(v).is_some() || s.unwrap::<crate::range::StaticRange>(v).is_some()))
    })?;
    natives.set_atom("isRange", is_range.value())?;
    let sync = s.function("sync", |s, _this, a| {
        let page = crate::target::page_of(s)?;
        let entries = a.get(0).as_object().ok_or_else(unavailable)?;
        let count = entries.get("length")?.to_number()? as u32;
        let mut highlights = Vec::new();
        for i in 0..count {
            let entry = entries.get_index(i)?.as_object().ok_or_else(unavailable)?;
            let name = crate::util::dom_string(entry.get_index(0)?)?;
            let priority = entry.get_index(1)?.to_number()? as i32;
            let list = entry.get_index(2)?.as_object().ok_or_else(unavailable)?;
            let n = list.get("length")?.to_number()? as u32;
            let mut ranges = Vec::new();
            for j in 0..n {
                let v = list.get_index(j)?;
                if let Some(r) = s.unwrap::<crate::range::Range>(v) {
                    ranges.push(HighlightRange::Live(r.owner()));
                } else if let Some(r) = s.unwrap::<crate::range::StaticRange>(v) {
                    let ((start, from), (end, to)) = r.points();
                    ranges.push(HighlightRange::Static(
                        (crate::collections::NodeHold::new(&page, start)?, from),
                        (crate::collections::NodeHold::new(&page, end)?, to),
                    ));
                }
            }
            highlights.push(CustomHighlight { name, priority, ranges });
        }
        // Dropping old ranges releases nodes: not while the cell is borrowed.
        let old = std::mem::replace(&mut *page.highlights.borrow_mut(), highlights);
        drop(old);
        Ok(s.undefined())
    })?;
    natives.set_atom("sync", sync.value())?;
    let root = Persistent::new(s, natives.value());
    let make = s.eval(include_str!("highlights.js"), Some("pyro://internal/highlights.js"), 1)?;
    let module = make.as_object().ok_or_else(unavailable)?.call(None, &[root.get(s)])?;
    s.set_hidden(navigator, KEY, module)?;
    module.as_object().ok_or_else(unavailable)
}

/// `Highlight` / `HighlightRegistry` globals and `CSS.highlights`: the module's member `name`.
pub(crate) fn member<'s>(s: &Scope<'s>, page: &Rc<PageDom>, name: &str) -> JsResult<Value<'s>> {
    module(s, page)?.get(name)
}

/// A lazy global's first read: the module's member, which then replaces the accessor.
pub(crate) fn global<'s>(s: &Scope<'s>, page: &Rc<PageDom>, name: &str) -> JsResult<Value<'s>> {
    let value = member(s, page, name)?;
    replace_global(s, page, name, value)?;
    Ok(value)
}

/// Replaces a lazy global accessor with a data property holding `value`.
pub(crate) fn replace_global(s: &Scope<'_>, page: &Rc<PageDom>, name: &str, value: Value<'_>) -> JsResult<()> {
    let define = page.define_properties.borrow().get(&s.realm_id()).map(|p| p.get(s));
    let define = define.and_then(|v| v.as_object()).ok_or_else(unavailable)?;
    let desc = s.object();
    desc.set_atom("value", value)?;
    desc.set_atom("writable", s.boolean(true))?;
    desc.set_atom("enumerable", s.boolean(false))?;
    desc.set_atom("configurable", s.boolean(true))?;
    define.call(None, &[s.global().value(), s.string(name), desc.value()])?;
    Ok(())
}

/// The text directives of a URL fragment (`text=...` after `:~:`), and the fragment without them.
pub fn split_fragment_directive(fragment: &str) -> Option<(&str, &str)> {
    let at = fragment.find(":~:")?;
    Some((&fragment[..at], &fragment[at + 3..]))
}

/// A parsed `text=[prefix-,]start[,end][,-suffix]` directive.
#[derive(Debug, PartialEq)]
struct TextDirective {
    prefix: Option<String>,
    start: String,
    end: Option<String>,
    suffix: Option<String>,
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(v) = hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn parse_directives(directives: &str) -> Vec<TextDirective> {
    let mut out = Vec::new();
    for part in directives.split('&') {
        let Some(value) = part.strip_prefix("text=") else { continue };
        let mut terms: Vec<&str> = value.split(',').collect();
        let prefix = match terms.first().copied() {
            Some(t) if t.ends_with('-') && terms.len() > 1 => {
                terms.remove(0);
                Some(percent_decode(&t[..t.len() - 1]))
            }
            _ => None,
        };
        let suffix = match terms.last().copied() {
            Some(t) if t.starts_with('-') && terms.len() > 1 => {
                terms.pop();
                Some(percent_decode(&t[1..]))
            }
            _ => None,
        };
        let (start, end) = match terms.as_slice() {
            [start] => (percent_decode(start), None),
            [start, end] => (percent_decode(start), Some(percent_decode(end))),
            _ => continue,
        };
        if start.is_empty() {
            continue;
        }
        out.push(TextDirective { prefix, start, end, suffix });
    }
    out
}

/// The document's searchable text: whitespace collapsed, lowercased, each char with the text node
/// and UTF-16 offset it came from (`None`: a block boundary, which no match crosses).
struct Haystack {
    chars: Vec<char>,
    at: Vec<Option<(NodeId, u32, u32)>>,
}

const BLOCKS: &[&str] = &[
    "address", "article", "aside", "blockquote", "body", "br", "dd", "details", "dialog", "div", "dl", "dt",
    "fieldset", "figcaption", "figure", "footer", "form", "h1", "h2", "h3", "h4", "h5", "h6", "header", "hr",
    "html", "li", "main", "nav", "ol", "p", "pre", "section", "table", "tbody", "td", "tfoot", "th", "thead",
    "tr", "ul",
];
const SKIPPED: &[&str] = &["head", "script", "style", "noscript", "template", "iframe", "textarea", "select", "title"];

fn haystack(dom: &Dom, document: NodeId) -> Haystack {
    let mut hay = Haystack { chars: Vec::new(), at: Vec::new() };
    fn walk(dom: &Dom, node: NodeId, hay: &mut Haystack) {
        let Ok(children) = dom.children(node) else { return };
        for child in children {
            if crate::editing::is_text(dom, child) {
                let Ok(data) = dom.char_data(child) else { continue };
                let mut units = 0u32;
                for c in data.chars() {
                    let len = c.len_utf16() as u32;
                    if c.is_whitespace() {
                        if !matches!(hay.chars.last(), Some(' ') | None) && hay.at.last().is_some_and(|a| a.is_some()) {
                            hay.chars.push(' ');
                            hay.at.push(Some((child, units, units + len)));
                        }
                    } else {
                        for l in c.to_lowercase() {
                            hay.chars.push(l);
                            hay.at.push(Some((child, units, units + len)));
                        }
                    }
                    units += len;
                }
                continue;
            }
            let name = crate::editing::local(dom, child);
            if name.is_empty() || SKIPPED.contains(&name.as_str()) {
                continue;
            }
            let block = BLOCKS.contains(&name.as_str());
            if block {
                hay.chars.push('\0');
                hay.at.push(None);
            }
            walk(dom, child, hay);
            if block {
                hay.chars.push('\0');
                hay.at.push(None);
            }
        }
    }
    walk(dom, document, &mut hay);
    hay
}

fn needle(text: &str) -> Vec<char> {
    let mut out: Vec<char> = Vec::new();
    for c in text.trim().chars() {
        if c.is_whitespace() {
            if out.last() != Some(&' ') {
                out.push(' ');
            }
        } else {
            out.extend(c.to_lowercase());
        }
    }
    out
}

fn is_word(c: Option<&char>) -> bool {
    c.is_some_and(|c| c.is_alphanumeric())
}

impl Haystack {
    /// The first word-bounded occurrence of `needle` at or after `from`.
    fn find(&self, needle: &[char], from: usize) -> Option<usize> {
        if needle.is_empty() || needle.len() > self.chars.len() {
            return None;
        }
        (from..=self.chars.len() - needle.len()).find(|&i| {
            self.chars[i..i + needle.len()] == *needle
                && !(is_word(i.checked_sub(1).and_then(|p| self.chars.get(p))) && is_word(needle.first()))
                && !(is_word(self.chars.get(i + needle.len())) && is_word(needle.last()))
        })
    }

    /// Whether `needle` ends just before `at` (white space between allowed).
    fn ends_before(&self, needle: &[char], at: usize) -> bool {
        let mut end = at;
        while end > 0 && self.chars[end - 1] == ' ' {
            end -= 1;
        }
        end >= needle.len() && self.chars[end - needle.len()..end] == *needle
    }

    /// Whether `needle` starts just after `at` (white space between allowed).
    fn starts_after(&self, needle: &[char], at: usize) -> bool {
        let mut start = at;
        while start < self.chars.len() && self.chars[start] == ' ' {
            start += 1;
        }
        self.chars.len() - start >= needle.len() && self.chars[start..start + needle.len()] == *needle
    }

    fn matches(&self, directive: &TextDirective) -> Option<((NodeId, u32), (NodeId, u32))> {
        let start = needle(&directive.start);
        let end = directive.end.as_deref().map(needle);
        let prefix = directive.prefix.as_deref().map(needle);
        let suffix = directive.suffix.as_deref().map(needle);
        let mut from = 0;
        while let Some(i) = self.find(&start, from) {
            from = i + 1;
            if prefix.as_ref().is_some_and(|p| !self.ends_before(p, i)) {
                continue;
            }
            let last = match &end {
                Some(end) => {
                    let Some(j) = self.find(end, i + start.len()) else { return None };
                    if self.chars[i..j].contains(&'\0') {
                        continue;
                    }
                    j + end.len()
                }
                None => i + start.len(),
            };
            if suffix.as_ref().is_some_and(|s| !self.starts_after(s, last)) {
                continue;
            }
            let (Some(first), Some(final_)) = (self.at[i], self.at[last - 1]) else { continue };
            return Some(((first.0, first.1), (final_.0, final_.2)));
        }
        None
    }
}

/// Finds the text of a fragment's directives (`directives`: what follows `:~:`) and makes them
/// the document's target text, painted as `::target-text`. Directives that match nothing are
/// ignored.
pub fn apply_fragment_directive(page: &Rc<PageDom>, directives: &str) {
    let parsed = parse_directives(directives);
    let found: Vec<_> = {
        let dom = page.dom.borrow();
        let hay = haystack(&dom, page.document());
        parsed.iter().filter_map(|d| hay.matches(d)).collect()
    };
    let mut ranges = Vec::new();
    for (start, end) in found {
        if let Ok(owner) = RangeOwner::new(page, start, end) {
            ranges.push(owner);
        }
    }
    let old = std::mem::replace(&mut *page.target_text.borrow_mut(), ranges);
    drop(old);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directives_parse() {
        assert_eq!(
            parse_directives("text=match,me&text=me%20and%20me&foo=bar"),
            vec![
                TextDirective { prefix: None, start: "match".into(), end: Some("me".into()), suffix: None },
                TextDirective { prefix: None, start: "me and me".into(), end: None, suffix: None },
            ]
        );
        assert_eq!(
            parse_directives("text=a-,b,-c"),
            vec![TextDirective { prefix: Some("a".into()), start: "b".into(), end: None, suffix: Some("c".into()) }]
        );
        assert_eq!(percent_decode("match%20me"), "match me");
    }
}
