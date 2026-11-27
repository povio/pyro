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
        Ok(s.boolean(s.unwrap::<crate::range::Range>(v).is_some() || s.unwrap::<crate::range::StaticRange>(v).is_some() || s.unwrap::<crate::range::OpaqueRange>(v).is_some()))
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
                // An OpaqueRange (a text control's value) is held by the Highlight but not painted.
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
/// and UTF-16 offset it came from (`None`: a block boundary, which no exact text crosses).
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

/// What an element's style says of its text for the search (Chrome: only rendered, visible text
/// is searched; a block-level box is a boundary).
#[derive(Clone, Copy, Default)]
struct SearchStyle {
    /// `display: none`: the subtree is not rendered, and is no boundary.
    none: bool,
    /// An inline-level box (`display: inline*`): no boundary.
    inline: bool,
    /// `display: contents` and the like: no box of its own.
    contents: bool,
    /// `visibility: hidden` (or `collapse`): its text is not searched, but a block still bounds.
    hidden: bool,
    /// The style is known (a styled element); else the element name decides.
    known: bool,
}

fn haystack(dom: &Dom, document: NodeId, style_of: &dyn Fn(NodeId) -> SearchStyle) -> Haystack {
    let mut hay = Haystack { chars: Vec::new(), at: Vec::new() };
    fn walk(dom: &Dom, node: NodeId, hay: &mut Haystack, style_of: &dyn Fn(NodeId) -> SearchStyle, hidden: bool) {
        let Ok(children) = dom.children(node) else { return };
        for child in children {
            if crate::editing::is_text(dom, child) {
                if hidden {
                    continue;
                }
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
            let style = style_of(child);
            if style.none {
                continue;
            }
            // pyro: by its computed display: an inline `div` is no boundary (WPT css-pseudo
            // `target-text-010`), a block-level `span` is one.
            let block = if style.known {
                name == "br" || (!style.inline && !style.contents)
            } else {
                BLOCKS.contains(&name.as_str())
            };
            if block {
                hay.chars.push('\0');
                hay.at.push(None);
            }
            walk(dom, child, hay, style_of, style.hidden);
            if block {
                hay.chars.push('\0');
                hay.at.push(None);
            }
        }
    }
    walk(dom, document, &mut hay, style_of, false);
    hay
}

fn needle(text: &str) -> Vec<char> {
    needle_of(text.trim())
}

/// pyro: [`needle`] keeping white space at its ends (Chromium: `text=PASS%20` matches only a
/// "PASS" followed by white space; WPT css-pseudo `target-text-007`).
fn needle_of(text: &str) -> Vec<char> {
    let mut out: Vec<char> = Vec::new();
    for c in text.chars() {
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

/// Whether `j` joins the alphanumerics `a` and `b` into one word (UAX #29 MidLetter, MidNum and
/// MidNumLet: "he's", "3.5", "1,000").
fn joins(a: char, j: char, b: char) -> bool {
    a.is_alphanumeric()
        && b.is_alphanumeric()
        && (matches!(j, '\'' | '\u{2019}' | '.') || (j == ',' && a.is_numeric() && b.is_numeric()))
}

impl Haystack {
    /// Whether the text has a word boundary between `chars[i - 1]` and `chars[i]` (the ends of the
    /// text and a block boundary are ones).
    fn boundary(&self, i: usize) -> bool {
        let c = &self.chars;
        if i == 0 || i >= c.len() {
            return true;
        }
        let joined = (c[i - 1].is_alphanumeric() && c[i].is_alphanumeric())
            || (i + 1 < c.len() && joins(c[i - 1], c[i], c[i + 1]))
            || (i >= 2 && joins(c[i - 2], c[i - 1], c[i]));
        !joined
    }

    /// The first occurrence of `needle` at or after `from`, starting on a word boundary when
    /// `start_bound` and ending on one when `end_bound`.
    fn find(&self, needle: &[char], from: usize, start_bound: bool, end_bound: bool) -> Option<usize> {
        if needle.is_empty() || needle.len() > self.chars.len() {
            return None;
        }
        (from..=self.chars.len() - needle.len()).find(|&i| {
            self.chars[i..i + needle.len()] == *needle
                && (!start_bound || self.boundary(i))
                && (!end_bound || self.boundary(i + needle.len()))
        })
    }

    /// Whether `needle` (a context term: it starts on a word boundary, and need not end on one)
    /// ends just before `at`, with white space and block boundaries between.
    fn ends_before(&self, needle: &[char], at: usize) -> bool {
        let mut end = at;
        while end > 0 && matches!(self.chars[end - 1], ' ' | '\0') {
            end -= 1;
        }
        end >= needle.len() && self.chars[end - needle.len()..end] == *needle && self.boundary(end - needle.len())
    }

    /// Whether `needle` (a context term: it ends on a word boundary, and need not start on one)
    /// starts just after `at`, with white space and block boundaries between.
    fn starts_after(&self, needle: &[char], at: usize) -> bool {
        let mut start = at;
        while start < self.chars.len() && matches!(self.chars[start], ' ' | '\0') {
            start += 1;
        }
        self.chars.len() - start >= needle.len()
            && self.chars[start..start + needle.len()] == *needle
            && self.boundary(start + needle.len())
    }

    fn matches(&self, directive: &TextDirective) -> Option<((NodeId, u32), (NodeId, u32))> {
        // pyro: a start with white space at its ends first as it is, then trimmed.
        let untrimmed = needle_of(&directive.start);
        if untrimmed != needle(&directive.start) && directive.end.is_none() {
            if let Some(found) = self.matches_start(directive, untrimmed) {
                return Some(found);
            }
        }
        self.matches_start(directive, needle(&directive.start))
    }

    /// "Find a range from a text directive": the first `start` (after the `prefix`, if any) that
    /// the rest of the directive matches. `start` begins on a word boundary unless a prefix leads
    /// it; it ends on one unless a suffix follows without an `end`; an `end` begins on one and
    /// ends on one unless a suffix follows it. The range may cross block boundaries when it has
    /// an `end`.
    fn matches_start(&self, directive: &TextDirective, start: Vec<char>) -> Option<((NodeId, u32), (NodeId, u32))> {
        let end = directive.end.as_deref().map(needle);
        let prefix = directive.prefix.as_deref().map(needle);
        let suffix = directive.suffix.as_deref().map(needle);
        let suffix_follows = |last: usize| suffix.as_ref().is_none_or(|s| self.starts_after(s, last));
        let mut from = 0;
        while let Some(i) = self.find(&start, from, prefix.is_none(), end.is_some() || suffix.is_none()) {
            from = i + 1;
            if prefix.as_ref().is_some_and(|p| !self.ends_before(p, i)) {
                continue;
            }
            let last = match &end {
                Some(end) => {
                    let mut end_from = i + start.len();
                    let mut found = None;
                    while let Some(j) = self.find(end, end_from, true, suffix.is_none()) {
                        end_from = j + 1;
                        if suffix_follows(j + end.len()) {
                            found = Some(j + end.len());
                            break;
                        }
                    }
                    match found {
                        Some(last) => last,
                        None => continue,
                    }
                }
                None => {
                    if !suffix_follows(i + start.len()) {
                        continue;
                    }
                    i + start.len()
                }
            };
            let (Some(first), Some(final_)) = (self.at[i], self.at[last - 1]) else { continue };
            return Some(((first.0, first.1), (final_.0, final_.2)));
        }
        None
    }
}

/// Finds the text of a fragment's directives (`directives`: what follows `:~:`) and makes them
/// the document's target text, painted as `::target-text`. Directives that match nothing are
/// ignored. Returns the text node the first matching directive (in URL order, as the spec's
/// "invoke text directives" scrolls to it) starts in, which the caller scrolls into view.
pub fn apply_fragment_directive(page: &Rc<PageDom>, directives: &str) -> Option<NodeId> {
    let parsed = parse_directives(directives);
    crate::animation_events::layout_flush(page, page.document());
    let found: Vec<_> = {
        let dom = page.dom.borrow();
        let layout = page.layout();
        let style = layout.style();
        let style_of = |id: NodeId| match style.computed_display(&dom, id) {
            Some(display) => SearchStyle {
                none: display == "none",
                inline: display.starts_with("inline"),
                contents: display == "contents",
                hidden: style.computed_value(&dom, id, "visibility").is_some_and(|v| v == "hidden" || v == "collapse"),
                known: true,
            },
            None => SearchStyle::default(),
        };
        let hay = haystack(&dom, page.document(), &style_of);
        parsed.iter().filter_map(|d| hay.matches(d)).collect()
    };
    // pyro: the ancestor revealing algorithm (HTML) runs on each match's ends: a
    // `hidden=until-found` element around the text gets `beforematch` and loses its `hidden`,
    // and a closed `<details>` around it opens, so the text shows (WPT css-pseudo
    // `target-text-010`).
    for &((start, _), (end, _)) in &found {
        crate::layout_api::reveal_ancestors(page, start);
        crate::layout_api::reveal_ancestors(page, end);
    }
    let first = found.first().map(|&((start, _), _)| start);
    let mut ranges = Vec::new();
    for (start, end) in found {
        if let Ok(owner) = RangeOwner::new(page, start, end) {
            ranges.push(owner);
        }
    }
    let old = std::mem::replace(&mut *page.target_text.borrow_mut(), ranges);
    drop(old);
    first
}

/// HTML "scroll to the fragment" for a fragment with text directives: invokes them
/// ([`apply_fragment_directive`]), scrolls the first match into the center of the viewport and
/// makes it the sequential focus navigation starting point (a Tab moves on from the text). False
/// when no directive matched: the caller falls back to the element the fragment names.
pub fn invoke_text_directives(page: &Rc<PageDom>, directives: &str) -> bool {
    let Some(node) = apply_fragment_directive(page, directives) else { return false };
    crate::layout_api::scroll_to_text_match(page, node);
    // (The element holding the text: the starting point is an element.)
    let holder = page.dom.borrow().parent_element(node).ok().flatten();
    page.sequential_start.set(holder.or(Some(node)));
    true
}

/// A cross-document load's fragment that is yet to be scrolled to (HTML "try to scroll to the
/// fragment", repeated at each rendering opportunity until the document has loaded).
pub(crate) struct PendingFragment {
    /// The URL's fragment without its text directives.
    fragment: String,
    /// The text directives (what follows `:~:`), when the navigation may invoke them.
    directives: Option<String>,
    /// DOMContentLoaded has fired: the document's content is there to search (Chrome waits for
    /// it too; the spec's "search timing" lets the UA).
    armed: bool,
    /// The DOM's `version` when the last attempt found nothing: until the DOM changes, another
    /// attempt (a layout flush and a scan) could not find more, so the rendering updates skip it.
    failed_at: Option<u64>,
}

/// Starts the scroll of a document committed by a cross-document navigation to `fragment`
/// (without its directives): it happens once the indicated element, or a text, exists. The
/// navigation may invoke `directives` only when allowed (a user activation, no opener, top level:
/// the caller decides). `force_load_at_top`: the response's `Document-Policy: force-load-at-top`
/// stops the document from scrolling on load, for any fragment.
pub fn begin_fragment_scroll(page: &Rc<PageDom>, fragment: Option<&str>, directives: Option<String>, force_load_at_top: bool) {
    let fragment = fragment.unwrap_or("");
    let nothing = fragment.is_empty() && directives.is_none();
    *page.pending_fragment.borrow_mut() =
        (!force_load_at_top && !nothing).then(|| PendingFragment { fragment: fragment.to_owned(), directives, armed: false, failed_at: None });
}

/// One attempt of the pending fragment scroll, at DOMContentLoaded (`arm`), at each rendering
/// update after it and after `load`: a text directive that matches is scrolled to; an element the
/// fragment names is scrolled to (and takes focus) as soon as it exists. Once the document has
/// completely loaded the attempt is the last: a text that is not found falls back to the element,
/// and a fragment that indicates nothing is dropped.
pub fn try_fragment_scroll(page: &Rc<PageDom>, arm: bool) {
    if page.is_disposed() {
        return;
    }
    let (fragment, directives, failed_at) = match page.pending_fragment.borrow_mut().as_mut() {
        Some(p) => {
            p.armed |= arm;
            if !p.armed {
                return;
            }
            (p.fragment.clone(), p.directives.clone(), p.failed_at)
        }
        None => return,
    };
    let last = page.is_completely_loaded();
    // The last attempt (the document has loaded) always runs: it falls back and drops the fragment.
    if !last && failed_at.is_some() && failed_at == page.dom.try_borrow().ok().map(|dom| dom.version()) {
        return;
    }
    let done = match directives.as_deref() {
        Some(directives) if invoke_text_directives(page, directives) => true,
        Some(_) if !last => false,
        _ => crate::layout_api::try_scroll_to_fragment(page, &fragment) || last,
    };
    if done {
        page.pending_fragment.borrow_mut().take();
    } else {
        let version = page.dom.try_borrow().ok().map(|dom| dom.version());
        if let Some(p) = page.pending_fragment.borrow_mut().as_mut() {
            p.failed_at = version;
        }
    }
}

/// The `FragmentDirective` interface (`document.fragmentDirective`): an object without members,
/// which scripts feature-detect text directives by.
pub struct FragmentDirective;
impl pyro_js::JsClass for FragmentDirective {
    const NAME: &'static str = "FragmentDirective";
    type Data = ();
    type Parent = pyro_js::NoParent;
    fn define(_def: &mut pyro_js::ClassDef<Self>) {}
}

pub(crate) fn register(engine: &pyro_js::Engine) {
    engine.register::<FragmentDirective>();
}

/// `document.fragmentDirective`: the document's one `FragmentDirective` ([SameObject]).
pub(crate) fn fragment_directive<'s>(s: &Scope<'s>, page: &Rc<PageDom>) -> JsResult<Value<'s>> {
    let key = pyro_js::IdentityKey(page.id() | (1 << 57), 0x7f01);
    crate::window::same_object(s, page, key, || s.wrap::<FragmentDirective>((), Some(key)))
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
