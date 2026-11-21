//! Editing commands beyond typing: caret movement and deletion by word,
//! line, paragraph and document (modifier keys, Playwright's macOS
//! `commands`, `Selection.modify()`), double/triple-click selection,
//! `document.execCommand()` with `queryCommand*()`, and undo/redo.
//!
//! Text controls work on their value (UTF-16 units). Editable content and
//! the document selection work on a flattened view of the editing host (or
//! the body): its text with block boundaries and `<br>`s as line breaks
//! ([`Flat`]), built only when a command needs it. Lines are logical lines
//! (block boundaries, `<br>` and preserved newlines); soft wrapping is not
//! modelled, as for textarea's ArrowUp/ArrowDown.
//!
//! Nothing here runs unless an editing command does: keys without these
//! modifiers take the existing paths, and the undo history is touched only
//! by edits the user (or execCommand) makes.

mod alignment;
mod indentation;
pub(crate) mod inline_value;

use std::rc::Rc;

use pyro_dom::{Dom, NodeId, NodeType};

use crate::editing::{self, Point};
use crate::page::PageDom;

/// Whether the page runs as macOS Chrome (`navigator.platform`): it decides
/// which modifiers move by word and line and which one is the shortcut key.
pub(crate) fn is_mac(page: &PageDom) -> bool {
    crate::window::platform(page).starts_with("Mac")
}

// ----- granularities over UTF-16 text --------------------------------------------

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Gran {
    Character,
    Word,
    Sentence,
    Line,
    Paragraph,
    LineBoundary,
    SentenceBoundary,
    ParagraphBoundary,
    DocumentBoundary,
}

impl Gran {
    fn parse(s: &str) -> Option<Gran> {
        Some(match s.to_ascii_lowercase().as_str() {
            "character" => Gran::Character,
            "word" => Gran::Word,
            "sentence" => Gran::Sentence,
            "line" => Gran::Line,
            "paragraph" => Gran::Paragraph,
            "lineboundary" => Gran::LineBoundary,
            "sentenceboundary" => Gran::SentenceBoundary,
            "paragraphboundary" => Gran::ParagraphBoundary,
            "documentboundary" => Gran::DocumentBoundary,
            _ => return None,
        })
    }
}

/// A block boundary in flattened text (also a textarea's linefeed).
const PARA: u16 = b'\n' as u16;
/// A line break inside a paragraph: `<br>`, a preserved linefeed.
const LINE: u16 = 0x2028;
/// A replaced element (image, control) in flattened text.
const OBJECT: u16 = 0xFFFC;

fn is_space(u: u16) -> bool {
    matches!(u, 0x20 | 0x09 | 0x0A | 0x0C | 0x0D | 0xA0 | LINE)
}

fn word_unit(u: u16) -> bool {
    (0xD800..0xE000).contains(&u) || char::from_u32(u as u32).is_some_and(|c| c.is_alphanumeric() || c == '_')
}

/// Part of a word: a letter, digit or underscore, or an apostrophe or period
/// between two of them ("don't", "e.g").
fn in_word(units: &[u16], i: usize) -> bool {
    let u = units[i];
    word_unit(u)
        || (matches!(u, 0x27 | 0x2019 | 0x2E)
            && i > 0
            && i + 1 < units.len()
            && word_unit(units[i - 1])
            && word_unit(units[i + 1]))
}

/// The end of the next word (symbols and spaces before it are skipped), as
/// Chromium moves forward by word on macOS and Linux.
fn word_end(units: &[u16], pos: usize) -> usize {
    let mut i = pos.min(units.len());
    while i < units.len() && !in_word(units, i) {
        i += 1;
    }
    while i < units.len() && in_word(units, i) {
        i += 1;
    }
    i
}

/// The start of the previous word.
fn word_start(units: &[u16], pos: usize) -> usize {
    let mut i = pos.min(units.len());
    while i > 0 && !in_word(units, i - 1) {
        i -= 1;
    }
    while i > 0 && in_word(units, i - 1) {
        i -= 1;
    }
    i
}

/// The run of the same kind (word, space, other) around `pos`: what a
/// double-click selects.
fn word_around(units: &[u16], pos: usize) -> (usize, usize) {
    let n = units.len();
    if n == 0 {
        return (0, 0);
    }
    let kind = |i: usize| {
        if in_word(units, i) {
            0
        } else if is_space(units[i]) {
            1
        } else {
            2
        }
    };
    // The character after the caret decides, else the one before.
    let at = if pos < n && units[pos] != PARA && units[pos] != LINE { pos } else { pos.saturating_sub(1).min(n - 1) };
    if matches!(units[at], PARA | LINE) {
        return (pos, pos);
    }
    let k = kind(at);
    let (mut a, mut b) = (at, at + 1);
    while a > 0 && !matches!(units[a - 1], PARA | LINE) && kind(a - 1) == k && (k != 2 || units[a - 1] == units[at]) {
        a -= 1;
    }
    while b < n && !matches!(units[b], PARA | LINE) && kind(b) == k && (k != 2 || units[b] == units[at]) {
        b += 1;
    }
    (a, b)
}

fn is_break(u: u16, para_only: bool) -> bool {
    u == PARA || (!para_only && u == LINE)
}

fn line_start(units: &[u16], pos: usize, para_only: bool) -> usize {
    units[..pos.min(units.len())].iter().rposition(|&u| is_break(u, para_only)).map_or(0, |i| i + 1)
}

fn line_end(units: &[u16], pos: usize, para_only: bool) -> usize {
    let pos = pos.min(units.len());
    units[pos..].iter().position(|&u| is_break(u, para_only)).map_or(units.len(), |i| pos + i)
}

fn is_terminator(u: u16) -> bool {
    matches!(u, 0x2E | 0x21 | 0x3F)
}

fn sentence_end(units: &[u16], pos: usize) -> usize {
    let n = units.len();
    let mut i = pos.min(n);
    while i < n && is_space(units[i]) && units[i] != PARA {
        i += 1;
    }
    while i < n && units[i] != PARA {
        let u = units[i];
        i += 1;
        if is_terminator(u) && (i == n || is_space(units[i])) {
            break;
        }
    }
    i
}

fn sentence_start(units: &[u16], pos: usize) -> usize {
    let mut i = pos.min(units.len());
    while i > 0 && is_space(units[i - 1]) && units[i - 1] != PARA {
        i -= 1;
    }
    while i > 0 && units[i - 1] != PARA && !(is_terminator(units[i - 1]) && i < units.len() && is_space(units[i])) {
        i -= 1;
    }
    while i < pos && is_space(units[i]) && units[i] != PARA {
        i += 1;
    }
    i
}

/// The caret one line up or down, keeping its column (clamped to the
/// target line); from the first (last) line it goes to the start (end).
fn line_move(units: &[u16], pos: usize, up: bool) -> usize {
    let n = units.len();
    let pos = pos.min(n);
    let col = pos - line_start(units, pos, false);
    let target = if up {
        let start = line_start(units, pos, false);
        if start == 0 {
            return 0;
        }
        let prev = line_start(units, start - 1, false);
        prev + col.min(start - 1 - prev)
    } else {
        let end = line_end(units, pos, false);
        if end == n {
            return n;
        }
        let next = end + 1;
        next + col.min(line_end(units, next, false) - next)
    };
    snap(units, target)
}

/// Never between the halves of a surrogate pair.
fn snap(units: &[u16], i: usize) -> usize {
    if i > 0 && i < units.len() && (0xDC00..0xE000).contains(&units[i]) {
        i - 1
    } else {
        i
    }
}

fn char_step(units: &[u16], pos: usize, forward: bool) -> usize {
    let n = units.len();
    if forward {
        if pos >= n {
            return n;
        }
        let mut i = pos + 1;
        if i < n && (0xDC00..0xE000).contains(&units[i]) {
            i += 1;
        }
        i
    } else {
        if pos == 0 {
            return 0;
        }
        snap(units, pos - 1)
    }
}

/// Where a move by `gran` from `pos` lands.
fn target(units: &[u16], pos: usize, forward: bool, gran: Gran) -> usize {
    let n = units.len();
    let pos = pos.min(n);
    match (gran, forward) {
        (Gran::Character, _) => char_step(units, pos, forward),
        (Gran::Word, true) => word_end(units, pos),
        (Gran::Word, false) => word_start(units, pos),
        (Gran::Sentence | Gran::SentenceBoundary, true) => sentence_end(units, pos),
        (Gran::Sentence, false) => {
            let s = sentence_start(units, pos);
            if s == pos && pos > 0 {
                // From a sentence's start: the previous one's.
                let mut j = pos;
                while j > 0 && units[j - 1] != PARA && (is_space(units[j - 1]) || is_terminator(units[j - 1])) {
                    j -= 1;
                }
                sentence_start(units, j)
            } else {
                s
            }
        }
        (Gran::SentenceBoundary, false) => sentence_start(units, pos),
        (Gran::Line, up) => line_move(units, pos, !up),
        (Gran::LineBoundary, true) => line_end(units, pos, false),
        (Gran::LineBoundary, false) => line_start(units, pos, false),
        (Gran::Paragraph, true) => {
            let e = line_end(units, pos, true);
            if e == pos && pos < n {
                line_end(units, pos + 1, true)
            } else {
                e
            }
        }
        (Gran::Paragraph, false) => {
            let s = line_start(units, pos, true);
            if s == pos && pos > 0 {
                line_start(units, pos - 1, true)
            } else {
                s
            }
        }
        (Gran::ParagraphBoundary, true) => line_end(units, pos, true),
        (Gran::ParagraphBoundary, false) => line_start(units, pos, true),
        (Gran::DocumentBoundary, true) => n,
        (Gran::DocumentBoundary, false) => 0,
    }
}

/// The inputType of a deletion by `gran` (Blink's
/// DeletionInputTypeFromTextGranularity).
fn deletion_input_type(forward: bool, gran: Gran) -> &'static str {
    match (gran, forward) {
        (Gran::Word, false) => "deleteWordBackward",
        (Gran::Word, true) => "deleteWordForward",
        (Gran::Line | Gran::LineBoundary, false) => "deleteSoftLineBackward",
        (Gran::Line | Gran::LineBoundary, true) => "deleteSoftLineForward",
        (Gran::Paragraph | Gran::ParagraphBoundary, false) => "deleteHardLineBackward",
        (Gran::Paragraph | Gran::ParagraphBoundary, true) => "deleteHardLineForward",
        (_, false) => "deleteContentBackward",
        (_, true) => "deleteContentForward",
    }
}

// ----- flattened editable content ------------------------------------------------

/// The text of a subtree as editing sees it: text nodes' data, a [`PARA`]
/// at block boundaries, a [`LINE`] per `<br>` (and per linefeed where white
/// space preserves them), an [`OBJECT`] per replaced element. Formatting
/// white space at the start of a line is left out.
pub(crate) struct Flat {
    units: Vec<u16>,
    /// (text node, start in `units`, length)
    texts: Vec<(NodeId, u32, u32)>,
    /// Element positions at offsets of `units` (before a `<br>` or object,
    /// at a block's start).
    marks: Vec<(u32, Point)>,
}

impl Flat {
    pub(crate) fn build(page: &PageDom, scope: NodeId) -> Flat {
        let dom = page.dom.borrow();
        let mut flat = Flat { units: Vec::new(), texts: Vec::new(), marks: Vec::new() };
        flat.walk(page, &dom, scope);
        flat
    }

    fn sep(&mut self) {
        match self.units.last_mut() {
            None | Some(&mut PARA) => {}
            // A <br> ending a block adds no line of its own.
            Some(last @ &mut LINE) => *last = PARA,
            Some(_) => self.units.push(PARA),
        }
    }

    fn walk(&mut self, page: &PageDom, dom: &Dom, node: NodeId) {
        let Ok(children) = dom.children(node) else { return };
        let children: Vec<NodeId> = children.collect();
        for (index, child) in children.into_iter().enumerate() {
            match dom.node_type(child) {
                Ok(NodeType::Text | NodeType::CDataSection) => {
                    let data = dom.char_data(child).unwrap_or("");
                    if data.is_empty() {
                        continue;
                    }
                    let preserved = data.contains('\n') && newlines_significant_in(page, dom, child);
                    let line_start = self.units.last().is_none_or(|&u| u == PARA || u == LINE);
                    if !preserved && line_start && data.chars().all(char::is_whitespace) {
                        continue;
                    }
                    let start = self.units.len() as u32;
                    self.units.extend(data.encode_utf16().map(|u| match u {
                        PARA if preserved => LINE,
                        PARA => 0x20,
                        u => u,
                    }));
                    self.texts.push((child, start, self.units.len() as u32 - start));
                }
                Ok(NodeType::Element) => {
                    let name = editing::local(dom, child);
                    match name.as_str() {
                        "br" => {
                            self.marks.push((self.units.len() as u32, (node, index as u32)));
                            self.units.push(LINE);
                        }
                        "script" | "style" | "template" | "noscript" | "head" | "title" | "meta" | "link" => {}
                        _ if !editing::is_container(dom, child)
                            || matches!(name.as_str(), "textarea" | "select" | "button") =>
                        {
                            self.marks.push((self.units.len() as u32, (node, index as u32)));
                            self.units.push(OBJECT);
                        }
                        _ if editing::is_block(dom, child) => {
                            self.sep();
                            self.marks.push((self.units.len() as u32, (child, 0)));
                            self.walk(page, dom, child);
                            self.sep();
                        }
                        _ => self.walk(page, dom, child),
                    }
                }
                _ => {}
            }
        }
    }

    /// The position at flat offset `pos`; at a node boundary, the later
    /// node's start (`later`) or the earlier one's end.
    fn point_at(&self, pos: u32, later: bool) -> Option<Point> {
        let mut found = None;
        for &(n, start, len) in &self.texts {
            if start > pos {
                break;
            }
            if pos <= start + len {
                found = Some((n, pos - start));
                if !later {
                    break;
                }
            }
        }
        if found.is_some() {
            return found;
        }
        if let Some(&(_, p)) = self.marks.iter().find(|m| m.0 == pos) {
            return Some(p);
        }
        self.texts.iter().find(|t| t.1 >= pos).map(|t| (t.0, 0)).or_else(|| self.texts.last().map(|t| (t.0, t.2)))
    }

    /// The flat offset of a position (the next laid-out text after an
    /// element position).
    fn pos_of(&self, dom: &Dom, p: Point) -> u32 {
        if editing::is_text(dom, p.0) {
            if let Some(&(_, start, len)) = self.texts.iter().find(|t| t.0 == p.0) {
                return start + p.1.min(len);
            }
        }
        if let Some(&(pos, _)) = self.marks.iter().find(|m| m.1 == p) {
            return pos;
        }
        for &(n, start, len) in &self.texts {
            if crate::range::compare_boundary_points_dom(dom, (n, len), p) > 0 {
                return if crate::range::compare_boundary_points_dom(dom, (n, 0), p) >= 0 {
                    start
                } else {
                    start + len
                };
            }
        }
        self.units.len() as u32
    }

    /// The text from `a` to `b` as copied: line breaks as linefeeds, objects
    /// left out.
    fn text(&self, a: u32, b: u32) -> String {
        let b = (b as usize).min(self.units.len());
        let slice = &self.units[(a as usize).min(b)..b];
        let units: Vec<u16> =
            slice.iter().filter(|&&u| u != OBJECT).map(|&u| if u == LINE || u == PARA { PARA } else { u }).collect();
        String::from_utf16_lossy(&units)
    }
}

/// Whether linefeeds in `node`'s text render as line breaks (`white-space`
/// pre, pre-wrap, pre-line, break-spaces). Computed style is read only for
/// text that has linefeeds.
pub(crate) fn newlines_significant_in(page: &PageDom, dom: &Dom, node: NodeId) -> bool {
    let el = if editing::is_text(dom, node) { dom.parent_element(node).ok().flatten() } else { Some(node) };
    let Some(el) = el else { return false };
    let layout = page.layout();
    let collapse = layout.computed_style_value(dom, el, "white-space-collapse");
    if !collapse.is_empty() {
        return matches!(collapse.as_str(), "preserve" | "preserve-breaks" | "break-spaces");
    }
    let ws = layout.computed_style_value(dom, el, "white-space");
    ws.starts_with("pre") || ws == "break-spaces"
}

pub(crate) fn newlines_significant(page: &PageDom, node: NodeId) -> bool {
    newlines_significant_in(page, &page.dom.borrow(), node)
}

/// The scope a document selection moves in: the editing host of `node`,
/// else the body (else the document element).
fn scope_of(page: &PageDom, node: NodeId) -> Option<NodeId> {
    let dom = page.dom.borrow();
    editing::editing_host(&dom, node).or_else(|| {
        let doc = page.document();
        dom.body(doc).ok().flatten().or_else(|| dom.document_element(doc).ok().flatten())
    })
}

// ----- text controls ---------------------------------------------------------------

fn control_text(dom: &Dom, el: NodeId) -> Option<(Vec<u16>, bool)> {
    let v = dom.value(el).ok()?;
    Some((v.encode_utf16().collect(), editing::local(dom, el) == "textarea"))
}

/// A gran-wise target in a control: single-line inputs have one line.
fn control_target(units: &[u16], multiline: bool, pos: usize, forward: bool, gran: Gran) -> usize {
    match gran {
        Gran::Line | Gran::LineBoundary | Gran::Paragraph | Gran::ParagraphBoundary if !multiline => {
            if forward {
                units.len()
            } else {
                0
            }
        }
        _ => target(units, pos, forward, gran),
    }
}

/// Moves (or with `extend`, extends) a text control's selection by `gran`.
pub(crate) fn control_move(page: &Rc<PageDom>, el: NodeId, forward: bool, gran: Gran, extend: bool) {
    {
        let mut dom = page.dom.borrow_mut();
        let Some((units, multiline)) = control_text(&dom, el) else { return };
        let Ok((s, e)) = dom.selection(el) else { return };
        let backward = dom.selection_direction(el).ok().flatten() == Some(true);
        let (anchor, focus) = if backward { (e, s) } else { (s, e) };
        let caret = if !extend && s != e && gran == Gran::Character {
            if forward {
                e
            } else {
                s
            }
        } else {
            let from = if extend {
                focus
            } else if forward {
                e
            } else {
                s
            };
            control_target(&units, multiline, from as usize, forward, gran) as u32
        };
        if extend {
            let _ = dom.set_selection_range(el, anchor.min(caret), anchor.max(caret));
            let _ = dom.set_selection_direction(el, Some(caret < anchor));
        } else {
            let _ = dom.set_selection_range(el, caret, caret);
        }
    }
    editing::queue_control_selectionchange(page, el);
}

/// Replaces a text control's [from, to) with `text` as one edit:
/// beforeinput, the change (an undo step), selectionchange and input.
fn control_replace(
    page: &Rc<PageDom>,
    el: NodeId,
    range: Option<(u32, u32)>,
    text: &str,
    input_type: &str,
    data: Option<&str>,
) -> bool {
    let doc = page.document();
    if !editing::input_event(page, el, "beforeinput", input_type, data, true)
        || page.document() != doc
        || !editing::connected(page, el)
    {
        return false;
    }
    apply_control_replace(page, el, range, text, input_type, data)
}

/// Apply the edit after its user-input gate, or directly for a script command.
fn apply_control_replace(
    page: &Rc<PageDom>,
    el: NodeId,
    range: Option<(u32, u32)>,
    text: &str,
    input_type: &str,
    data: Option<&str>,
) -> bool {
    record_control_edit(page, el, input_type);
    let changed = {
        let mut dom = page.dom.borrow_mut();
        if let Some((a, b)) = range {
            let _ = dom.set_selection_range(el, a, b);
        }
        dom.insert_text(el, text).unwrap_or(false)
    };
    note_control_edited(page, el);
    if changed {
        editing::queue_control_selectionchange(page, el);
        editing::input_event(page, el, "input", input_type, data, false);
    }
    changed
}

/// Keyboard editing dispatches cancelable beforeinput. Scripted control
/// commands run their default action directly and still dispatch input.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum EditSource {
    User,
    Script,
}

/// Deletes the selection, else from the caret to the `gran` boundary.
pub(crate) fn control_delete(
    page: &Rc<PageDom>, el: NodeId, forward: bool, gran: Gran, input_type: Option<&str>, source: EditSource,
) {
    let range = {
        let dom = page.dom.borrow();
        let Some((units, multiline)) = control_text(&dom, el) else { return };
        let Ok((s, e)) = dom.selection(el) else { return };
        if s != e {
            (s, e)
        } else {
            let t = control_target(&units, multiline, s as usize, forward, gran) as u32;
            (s.min(t), s.max(t))
        }
    };
    let input_type = input_type.unwrap_or_else(|| deletion_input_type(forward, gran));
    match source {
        EditSource::User => control_replace(page, el, Some(range), "", input_type, None),
        EditSource::Script => apply_control_replace(page, el, Some(range), "", input_type, None),
    };
}

/// Inserts `text` at a text control's selection (paste, insertText).
pub(crate) fn control_insert(page: &Rc<PageDom>, el: NodeId, text: &str, input_type: &str) -> bool {
    control_replace(page, el, None, text, input_type, Some(text))
}

fn control_select(page: &Rc<PageDom>, el: NodeId, a: u32, b: u32) {
    {
        let mut dom = page.dom.borrow_mut();
        let _ = dom.set_selection_range(el, a, b);
        let _ = dom.set_selection_direction(el, None);
    }
    editing::queue_control_selectionchange(page, el);
}

fn control_select_all(page: &Rc<PageDom>, el: NodeId) {
    let len = page.dom.borrow().value(el).map(|v| pyro_dom::dom::utf16_len(&v)).unwrap_or(0);
    control_select(page, el, 0, len);
}

// ----- editable content and the document selection ------------------------------

/// The (anchor, focus) a move by `gran` gives the document selection in
/// `scope` (`None`: nothing to move).
fn moved_selection(
    page: &Rc<PageDom>,
    scope: NodeId,
    forward: bool,
    gran: Gran,
    extend: bool,
) -> Option<(Point, Point)> {
    let (anchor, focus) = editing::selection(page)?;
    let (start, end) = editing::selection_range(page)?;
    let collapsed_to = if forward { end } else { start };
    if gran == Gran::Character {
        if !extend && start != end {
            return Some((collapsed_to, collapsed_to));
        }
        let dom = page.dom.borrow();
        let p = editing::step(&dom, scope, focus, forward);
        return Some((if extend { anchor } else { p }, p));
    }
    let flat = Flat::build(page, scope);
    let from = if extend { focus } else { collapsed_to };
    let pos = flat.pos_of(&page.dom.borrow(), from) as usize;
    let t = target(&flat.units, pos, forward, gran) as u32;
    let p = flat.point_at(t, !forward)?;
    Some((if extend { anchor } else { p }, p))
}

/// Moves the caret in an editing host by `gran`.
pub(crate) fn host_move(page: &Rc<PageDom>, host: NodeId, forward: bool, gran: Gran, extend: bool) {
    let inside =
        editing::selection(page).is_some_and(|(a, _)| editing::editing_host(&page.dom.borrow(), a.0) == Some(host));
    if !inside {
        let p = editing::host_start(&page.dom.borrow(), host);
        editing::set_selection(page, p, p);
        return;
    }
    if let Some((a, f)) = moved_selection(page, host, forward, gran, extend) {
        editing::set_selection(page, a, f);
    }
}

/// Deletes the selection, else from the caret to the `gran` boundary, in an
/// editing host.
pub(crate) fn host_delete(page: &Rc<PageDom>, host: NodeId, forward: bool, gran: Gran) {
    if gran == Gran::Character {
        editing::delete(page, host, !forward);
        return;
    }
    let (start, end) = editing::host_range(page, host);
    let (a, b) = if start != end {
        (start, end)
    } else {
        let flat = Flat::build(page, host);
        let pos = flat.pos_of(&page.dom.borrow(), start) as usize;
        let t = target(&flat.units, pos, forward, gran) as u32;
        let Some(p) = flat.point_at(t, !forward) else { return };
        if editing::compare(page, p, start) < 0 {
            (p, start)
        } else {
            (start, p)
        }
    };
    editing::delete_span(page, host, deletion_input_type(forward, gran), a, b);
}

/// `Selection.modify(alter, direction, granularity)`.
pub(crate) fn selection_modify(page: &Rc<PageDom>, alter: &str, direction: &str, granularity: &str) {
    let extend = match alter.to_ascii_lowercase().as_str() {
        "move" => false,
        "extend" => true,
        _ => return,
    };
    let forward = match direction.to_ascii_lowercase().as_str() {
        "forward" | "right" => true,
        "backward" | "left" => false,
        _ => return,
    };
    let Some(gran) = Gran::parse(granularity) else { return };
    let Some((_, focus)) = editing::selection(page) else { return };
    let Some(scope) = scope_of(page, focus.0) else { return };
    if let Some((a, f)) = moved_selection(page, scope, forward, gran, extend) {
        editing::set_selection(page, a, f);
    }
}

/// The text of the document range [start, end) as copying gives it: block
/// boundaries and `<br>`s as linefeeds, formatting white space left out.
pub(crate) fn selection_text(page: &PageDom, start: Point, end: Point) -> String {
    let scope = {
        let dom = page.dom.borrow();
        match (editing::editing_host(&dom, start.0), editing::editing_host(&dom, end.0)) {
            (Some(a), Some(b)) if a == b => Some(a),
            _ => None,
        }
    };
    let Some(scope) = scope.or_else(|| scope_of(page, page.document())) else { return String::new() };
    let flat = Flat::build(page, scope);
    let dom = page.dom.borrow();
    flat.text(flat.pos_of(&dom, start), flat.pos_of(&dom, end))
}

/// Selects the document's content (Ctrl/Cmd+A outside editable content).
fn select_document(page: &Rc<PageDom>) {
    let Some(body) = scope_of(page, page.document()) else { return };
    let n = page.dom.borrow().child_count(body).unwrap_or(0);
    editing::set_selection(page, (body, 0), (body, n));
}

// ----- double and triple click ------------------------------------------------------

/// A second press selects the word (or run of spaces or symbols) at the
/// caret, a third the line of a text control or the block of document text,
/// as Chromium does on mousedown. In text controls the caret the press
/// starts from is the control's current one (glyph positions inside
/// controls are not laid out).
pub(crate) fn multi_click(page: &Rc<PageDom>, target: NodeId, x: f64, y: f64, count: u32) {
    let control = {
        let dom = page.dom.borrow();
        editing::is_text_entry_control(&dom, target) && dom.focused(page.document()).ok().flatten() == Some(target)
    };
    if control {
        let range = {
            let dom = page.dom.borrow();
            let Some((units, multiline)) = control_text(&dom, target) else { return };
            let Ok((s, _)) = dom.selection(target) else { return };
            if count == 2 {
                word_around(&units, s as usize)
            } else if multiline {
                (line_start(&units, s as usize, true), line_end(&units, s as usize, true))
            } else {
                (0, units.len())
            }
        };
        control_select(page, target, range.0 as u32, range.1 as u32);
        return;
    }
    let caret = match editing::caret_at_point(page, target, x, y) {
        Some(p) => p,
        None => match editing::selection(page) {
            Some((a, f)) if a == f => f,
            _ => return,
        },
    };
    let Some(scope) = scope_of(page, caret.0) else { return };
    let block = editing::block_of(&page.dom.borrow(), caret.0, scope);
    if count >= 3 {
        let n = page.dom.borrow().child_count(block).unwrap_or(0);
        editing::set_selection(page, (block, 0), (block, n));
        return;
    }
    let flat = Flat::build(page, block);
    let pos = flat.pos_of(&page.dom.borrow(), caret) as usize;
    let (a, b) = word_around(&flat.units, pos);
    if let (Some(a), Some(b)) = (flat.point_at(a as u32, true), flat.point_at(b as u32, false)) {
        editing::set_selection(page, a, b);
    }
}

// ----- keys and Playwright's macOS commands -----------------------------------------

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Op {
    Move(bool, Gran, bool),
    Delete(bool, Gran),
    SelectAll,
    Copy,
    Cut,
    Paste,
    PastePlain,
    Undo,
    Redo,
    LineBreak,
}

/// Where a key's editing operation applies.
enum Target {
    Host(NodeId),
    Control(NodeId),
    Document,
}

fn target_of(page: &PageDom, el: NodeId) -> Target {
    let dom = page.dom.borrow();
    if let Some(h) = editing::editing_host(&dom, el) {
        Target::Host(h)
    } else if editing::is_text_entry_control(&dom, el) {
        Target::Control(el)
    } else {
        Target::Document
    }
}

fn run_op(page: &Rc<PageDom>, el: NodeId, op: Op) {
    use crate::clipboard::Action;
    match (op, target_of(page, el)) {
        (Op::Move(f, g, x), Target::Host(h)) => host_move(page, h, f, g, x),
        (Op::Move(f, g, x), Target::Control(c)) => control_move(page, c, f, g, x),
        // Outside editable content only the moves that extend the
        // selection act (Chromium enables the others in editable text or
        // caret browsing only).
        (Op::Move(_, _, false), Target::Document) => {}
        (Op::Move(f, g, x), Target::Document) => {
            if let Some((_, focus)) = editing::selection(page) {
                if let Some(scope) = scope_of(page, focus.0) {
                    if let Some((a, p)) = moved_selection(page, scope, f, g, x) {
                        editing::set_selection(page, a, p);
                    }
                }
            }
        }
        (Op::Delete(f, g), Target::Host(h)) => host_delete(page, h, f, g),
        (Op::Delete(f, g), Target::Control(c)) => control_delete(page, c, f, g, None, EditSource::User),
        (Op::Delete(..), Target::Document) => {}
        (Op::SelectAll, Target::Host(h)) => editing::select_all(page, h),
        (Op::SelectAll, Target::Control(c)) => control_select_all(page, c),
        (Op::SelectAll, Target::Document) => select_document(page),
        (Op::LineBreak, Target::Host(h)) => editing::insert_line_break(page, h),
        (Op::LineBreak, _) => {}
        (Op::Copy, _) => {
            crate::clipboard::run(page, Action::Copy, EditSource::User);
        }
        (Op::Cut, _) => {
            crate::clipboard::run(page, Action::Cut, EditSource::User);
        }
        (Op::Paste, _) => {
            crate::clipboard::run(page, Action::Paste, EditSource::User);
        }
        (Op::PastePlain, _) => {
            crate::clipboard::run(page, Action::PastePlain, EditSource::User);
        }
        (Op::Undo, _) => {
            undo(page, true, EditSource::User);
        }
        (Op::Redo, _) => {
            undo(page, false, EditSource::User);
        }
    }
}

/// The editing operation of a macOS editing command (as Chromium names
/// them; Playwright sends them on macOS), if handled.
fn command_op(name: &str) -> Option<Op> {
    let lower = name.to_ascii_lowercase();
    let (base, extend) = match lower.strip_suffix("andmodifyselection") {
        Some(b) => (b, true),
        None => (lower.as_str(), false),
    };
    let mv = |f: bool, g: Gran| Some(Op::Move(f, g, extend));
    match base {
        "moveleft" | "movebackward" => mv(false, Gran::Character),
        "moveright" | "moveforward" => mv(true, Gran::Character),
        "moveup" => mv(false, Gran::Line),
        "movedown" => mv(true, Gran::Line),
        "movewordleft" | "movewordbackward" => mv(false, Gran::Word),
        "movewordright" | "movewordforward" => mv(true, Gran::Word),
        "movetoleftendofline" | "movetobeginningofline" => mv(false, Gran::LineBoundary),
        "movetorightendofline" | "movetoendofline" => mv(true, Gran::LineBoundary),
        "movetobeginningofparagraph" => mv(false, Gran::ParagraphBoundary),
        "movetoendofparagraph" => mv(true, Gran::ParagraphBoundary),
        "moveparagraphbackward" => mv(false, Gran::Paragraph),
        "moveparagraphforward" => mv(true, Gran::Paragraph),
        "movetobeginningofdocument" => mv(false, Gran::DocumentBoundary),
        "movetoendofdocument" => mv(true, Gran::DocumentBoundary),
        _ if extend => None,
        "deletebackward" | "deletebackwardbydecomposingpreviouscharacter" => Some(Op::Delete(false, Gran::Character)),
        "deleteforward" => Some(Op::Delete(true, Gran::Character)),
        "deletewordbackward" => Some(Op::Delete(false, Gran::Word)),
        "deletewordforward" => Some(Op::Delete(true, Gran::Word)),
        "deletetobeginningofline" => Some(Op::Delete(false, Gran::LineBoundary)),
        "deletetoendofline" => Some(Op::Delete(true, Gran::LineBoundary)),
        "deletetobeginningofparagraph" => Some(Op::Delete(false, Gran::ParagraphBoundary)),
        "deletetoendofparagraph" => Some(Op::Delete(true, Gran::ParagraphBoundary)),
        "selectall" => Some(Op::SelectAll),
        "copy" => Some(Op::Copy),
        "cut" => Some(Op::Cut),
        "paste" => Some(Op::Paste),
        "pasteandmatchstyle" => Some(Op::PastePlain),
        "undo" => Some(Op::Undo),
        "redo" => Some(Op::Redo),
        "insertlinebreak" => Some(Op::LineBreak),
        _ => None,
    }
}

/// CDP `Input.dispatchKeyEvent`'s `commands` on an uncanceled keydown: the
/// handled ones run in order in place of the key's default action. Returns
/// false when none is handled (scrolling, Escape's cancelOperation, ...), and
/// the key's own default action runs as before.
pub fn run_commands(page: &Rc<PageDom>, el: NodeId, commands: &[String]) -> bool {
    let ops: Vec<Op> = commands.iter().filter_map(|c| command_op(c)).collect();
    if ops.is_empty() {
        return false;
    }
    for op in ops {
        if !editing::connected(page, el) {
            break;
        }
        run_op(page, el, op);
    }
    true
}

/// The editing operation of a key with modifiers, as Chromium binds them on
/// the page's platform. Only keys these bindings name reach the platform
/// check; everything else returns `None` at once.
fn key_op(page: &PageDom, el: NodeId, key: &str, m: crate::input::Modifiers) -> Option<Op> {
    let any = m.ctrl || m.meta || m.alt;
    let target = || target_of(page, el);
    match key {
        "Enter" if m.shift && !any => matches!(target(), Target::Host(_)).then_some(Op::LineBreak),
        "ArrowUp" | "ArrowDown" | "ArrowLeft" | "ArrowRight" | "Home" | "End" | "Backspace" | "Delete" | "c" | "C"
        | "x" | "X" | "v" | "V" | "z" | "Z" | "y" | "Y" | "a" | "A" => {
            let mac = any.then(|| is_mac(page)).unwrap_or(false);
            let primary = if mac { m.meta && !m.ctrl } else { m.ctrl && !m.meta };
            let word = if mac { m.alt && !m.meta && !m.ctrl } else { m.ctrl && !m.alt && !m.meta };
            let line = mac && m.meta && !m.alt && !m.ctrl;
            let forward = matches!(key, "ArrowRight" | "ArrowDown" | "End" | "Delete");
            match key {
                "c" | "C" if primary && !m.alt => Some(Op::Copy),
                "x" | "X" if primary && !m.alt => Some(Op::Cut),
                "v" | "V" if primary && !m.alt => Some(if m.shift { Op::PastePlain } else { Op::Paste }),
                "z" | "Z" if primary && !m.alt => Some(if m.shift { Op::Redo } else { Op::Undo }),
                "y" | "Y" if primary && !m.alt && !mac && !m.shift => Some(Op::Redo),
                // Inside editable content the existing select-all runs.
                "a" | "A" if primary && !m.alt => matches!(target(), Target::Document).then_some(Op::SelectAll),
                "ArrowLeft" | "ArrowRight" if word => Some(Op::Move(forward, Gran::Word, m.shift)),
                "ArrowLeft" | "ArrowRight" if line => Some(Op::Move(forward, Gran::LineBoundary, m.shift)),
                "ArrowUp" | "ArrowDown" if line => Some(Op::Move(forward, Gran::DocumentBoundary, m.shift)),
                "ArrowUp" | "ArrowDown" if mac && m.alt && !m.meta && !m.ctrl => {
                    Some(Op::Move(forward, Gran::ParagraphBoundary, m.shift))
                }
                "ArrowUp" | "ArrowDown" if !any => {
                    matches!(target(), Target::Host(_)).then_some(Op::Move(forward, Gran::Line, m.shift))
                }
                "Home" | "End" if !mac && m.ctrl && !m.alt && !m.meta => {
                    Some(Op::Move(forward, Gran::DocumentBoundary, m.shift))
                }
                // Lines of editable content and textareas (an input's
                // Home/End keep their existing whole-value path).
                "Home" | "End" if !any => match target() {
                    Target::Host(_) => Some(Op::Move(forward, Gran::LineBoundary, m.shift)),
                    Target::Control(c) if editing::local(&page.dom.borrow(), c) == "textarea" => {
                        Some(Op::Move(forward, Gran::LineBoundary, m.shift))
                    }
                    _ => None,
                },
                "Backspace" | "Delete" if word => Some(Op::Delete(forward, Gran::Word)),
                "Backspace" if line => Some(Op::Delete(false, Gran::LineBoundary)),
                _ => None,
            }
        }
        _ => None,
    }
}

/// The default action of an uncanceled keydown for the bindings above;
/// false when the key has none here (its existing default action runs).
pub(crate) fn key_action(page: &Rc<PageDom>, el: NodeId, k: &crate::input::KeyInput) -> bool {
    match key_op(page, el, &k.key, k.modifiers) {
        Some(op) => {
            run_op(page, el, op);
            true
        }
        None => false,
    }
}

// ----- execCommand ------------------------------------------------------------------

/// The text control or editing host commands act on: the focused element's,
/// else the host of the selection (focus on a checkbox does not clear it, as
/// in Chromium).
fn command_target(page: &PageDom) -> Option<Target> {
    let dom = page.dom.borrow();
    let focused = dom.focused(page.document()).ok().flatten();
    if let Some(f) = focused {
        if editing::is_text_entry_control(&dom, f) {
            return Some(Target::Control(f));
        }
        if let Some(h) = editing::editing_host(&dom, f) {
            return Some(Target::Host(h));
        }
    }
    drop(dom);
    let (start, _) = editing::selection_range(page)?;
    editing::editing_host(&page.dom.borrow(), start.0).map(Target::Host)
}

/// Inline formatting commands: (element made, elements that carry it).
fn format_of(command: &str) -> Option<(&'static str, &'static [&'static str], &'static str)> {
    Some(match command {
        "bold" => ("b", &["b", "strong"], "formatBold"),
        "italic" => ("i", &["i", "em"], "formatItalic"),
        "underline" => ("u", &["u"], "formatUnderline"),
        "strikethrough" => ("strike", &["strike", "s", "del"], "formatStrikeThrough"),
        "superscript" => ("sup", &["sup"], "formatSuperscript"),
        "subscript" => ("sub", &["sub"], "formatSubscript"),
        _ => return None,
    })
}

const SUPPORTED: &[&str] = &[
    "inserttext",
    "forecolor",
    "backcolor",
    "hilitecolor",
    "fontname",
    "fontsize",
    "indent",
    "outdent",
    "justifycenter",
    "justifyfull",
    "justifyleft",
    "justifyright",
    "stylewithcss",
    "usecss",
    "defaultparagraphseparator",
    "copy",
    "cut",
    "selectall",
    "undo",
    "redo",
    "delete",
    "forwarddelete",
    "insertparagraph",
    "insertlinebreak",
    "inserthtml",
    "insertimage",
    "inserthorizontalrule",
    "bold",
    "italic",
    "underline",
    "strikethrough",
    "superscript",
    "subscript",
    "createlink",
    "unlink",
    "insertorderedlist",
    "insertunorderedlist",
    "formatblock",
    "removeformat",
];

const FORMAT_BLOCKS: &[&str] = &[
    "address",
    "article",
    "aside",
    "blockquote",
    "dd",
    "div",
    "dl",
    "dt",
    "footer",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hgroup",
    "main",
    "nav",
    "p",
    "pre",
    "section",
];

/// `document.queryCommandSupported(command)`.
pub(crate) fn query_supported(command: &str) -> bool {
    SUPPORTED.contains(&command.to_ascii_lowercase().as_str())
}

fn requires_rich_selection(cmd: &str) -> bool {
    format_of(cmd).is_some()
        || inline_value::Kind::command(cmd).is_some()
        || alignment::value_for(cmd).is_some()
        || matches!(
            cmd,
            "indent" | "outdent" | "insertimage" | "inserthorizontalrule" | "createlink" | "unlink"
                | "removeformat" | "insertorderedlist" | "insertunorderedlist" | "formatblock"
        )
}

/// `document.execCommand(command, showUI, value)`: whether it ran.
/// `paste` is not available to script (as in Chromium, where reading the
/// clipboard from the DOM is off for web pages). `copy` and `cut` run as
/// from the keyboard; Chromium additionally requires transient user
/// activation, which is not tracked.
pub(crate) fn exec_command(page: &Rc<PageDom>, doc: NodeId, command: &str, value: &str) -> bool {
    use crate::clipboard::Action;
    let cmd = command.to_ascii_lowercase();
    if doc != page.document() {
        return false;
    }
    match cmd.as_str() {
        "stylewithcss" | "usecss" => {
            let css = value.eq_ignore_ascii_case("false") == (cmd == "usecss");
            return page.dom.borrow_mut().set_style_with_css(doc, css).is_ok();
        }
        "defaultparagraphseparator" => {
            return page.dom.borrow_mut().set_paragraph_separator(doc, value).unwrap_or(false)
        }
        "copy" => return crate::clipboard::run(page, Action::Copy, EditSource::Script),
        "cut" => return crate::clipboard::run(page, Action::Cut, EditSource::Script),
        "undo" => return undo(page, true, EditSource::Script),
        "redo" => return undo(page, false, EditSource::Script),
        "selectall" => {
            let target = command_target(page);
            // Editing "the selectAll command": `selectstart` (bubbling, cancelable) at the editing
            // host, else the body, else the document element; canceled, nothing is selected.
            let at = match &target {
                Some(Target::Control(c)) => Some(*c),
                Some(Target::Host(h)) => Some(*h),
                _ => {
                    let dom = page.dom.borrow();
                    let doc = page.document();
                    dom.body(doc).ok().flatten().or_else(|| dom.document_element(doc).ok().flatten())
                }
            };
            if let Some(at) = at {
                if !crate::input::fire_simple(page, at, "selectstart", true, true) {
                    return true;
                }
            }
            match target {
                Some(Target::Control(c)) => control_select_all(page, c),
                Some(Target::Host(h)) => editing::select_all(page, h),
                _ => select_document(page),
            }
            return true;
        }
        _ => {}
    }
    let target = command_target(page);
    if let Some(Target::Control(c)) = target {
        return match cmd.as_str() {
            "inserttext" => {
                apply_control_replace(page, c, None, value, "insertText", Some(value));
                true
            }
            "inserthtml" => {
                if !page.dom.borrow().is_mutable_text(c).unwrap_or(false) {
                    return false;
                }
                let textarea = editing::local(&page.dom.borrow(), c) == "textarea";
                let text = if textarea { Dom::plaintext_from_html(value) } else { Dom::single_line_text_from_html(value) };
                let Ok(text) = text else { return false };
                apply_control_replace(page, c, None, &text, "insertText", Some(&text));
                true
            }
            "delete" => {
                control_delete(page, c, false, Gran::Character, None, EditSource::Script);
                true
            }
            "forwarddelete" => {
                control_delete(page, c, true, Gran::Character, None, EditSource::Script);
                true
            }
            "insertlinebreak" | "insertparagraph" => {
                let textarea = editing::local(&page.dom.borrow(), c) == "textarea";
                if textarea {
                    let ty = if cmd == "insertlinebreak" { "insertLineBreak" } else { "insertParagraph" };
                    apply_control_replace(page, c, None, "\n", ty, None);
                }
                textarea
            }
            _ => false,
        };
    }
    let Some(Target::Host(host)) = target else { return false };
    if requires_rich_selection(&cmd) && !rich_selection(page, host) {
        return false;
    }
    if let Some(kind) = inline_value::Kind::command(&cmd) {
        return inline_value::apply(page, host, kind, value);
    }
    if matches!(cmd.as_str(), "indent" | "outdent") {
        indentation::apply(page, host, cmd == "indent");
        return true;
    }
    if let Some(target) = alignment::value_for(&cmd) {
        alignment::apply(page, host, target);
        return true;
    }
    match cmd.as_str() {
        "inserttext" => editing::insert_text(page, host, value),
        "delete" => editing::delete(page, host, true),
        "forwarddelete" => editing::delete(page, host, false),
        "insertparagraph" => editing::insert_paragraph(page, host),
        "insertlinebreak" => editing::insert_line_break(page, host),
        "inserthtml" => editing::insert_html(page, host, value, "insertHTML"),
        "insertimage" | "inserthorizontalrule" => {
            if cmd == "insertimage" && value.is_empty() {
                return false;
            }
            // Record the original selection and content only if insertion
            // succeeds. An input listener may invoke undo synchronously.
            let saved = save(page, host);
            if !editing::insert_replaced_element(page, host, (cmd == "insertimage").then_some(value)) {
                return false;
            }
            if let Some(saved) = saved {
                let mut history = page.undo.borrow_mut();
                if !history.replaying {
                    history.typing = None;
                    history.push(Step { target: host, saved });
                }
            }
            let input_type = if cmd == "insertimage" { "" } else { "insertHorizontalRule" };
            editing::input_event(page, host, "input", input_type, None, false);
        }
        "createlink" => {
            if value.is_empty() {
                return false;
            }
            format_inline(page, host, "a", &[], "insertLink", Some(value));
        }
        "unlink" => remove_inline(page, host, &["a"], ""),
        "removeformat" => remove_inline(
            page,
            host,
            &[
                "b", "strong", "i", "em", "u", "s", "strike", "del", "sub", "sup", "font", "span", "code", "small",
                "big", "mark",
            ],
            "formatRemove",
        ),
        "insertorderedlist" => toggle_list(page, host, "ol", "insertOrderedList"),
        "insertunorderedlist" => toggle_list(page, host, "ul", "insertUnorderedList"),
        "formatblock" => {
            let tag = value.trim().trim_start_matches('<').trim_end_matches('>').to_ascii_lowercase();
            if !FORMAT_BLOCKS.contains(&tag.as_str()) {
                return false;
            }
            format_block(page, host, &tag);
        }
        _ => {
            let Some((tag, carriers, input_type)) = format_of(&cmd) else { return false };
            let (start, end) = editing::host_range(page, host);
            // A collapsed selection would only set the style of the next
            // typed text, which is not modelled.
            if start == end {
                return false;
            }
            if format_state(page, &cmd).1 {
                remove_inline(page, host, carriers, input_type);
            } else {
                format_inline(page, host, tag, carriers, input_type, None);
            }
        }
    }
    true
}

/// Text nodes with characters in [start, end) (the whole of each).
fn texts_in(dom: &Dom, scope: NodeId, start: Point, end: Point) -> Vec<NodeId> {
    let Ok(all) = dom.descendants(scope) else { return Vec::new() };
    all.filter(|&n| {
        editing::is_text(dom, n) && {
            let len = editing::len(dom, n);
            len > 0
                && crate::range::compare_boundary_points_dom(dom, (n, len), start) > 0
                && crate::range::compare_boundary_points_dom(dom, (n, 0), end) < 0
        }
    })
    .collect()
}

/// Splits the text nodes at the range's ends so that whole text nodes make
/// it up; returns them in order.
fn split_texts(dom: &mut Dom, scope: NodeId, start: Point, end: Point) -> Vec<NodeId> {
    let mut texts = texts_in(dom, scope, start, end);
    if let Some(&last) = texts.last() {
        if last == end.0 && end.1 < editing::len(dom, last) {
            let _ = dom.split_text(last, end.1);
        }
    }
    if let Some(first) = texts.first_mut() {
        if *first == start.0 && start.1 > 0 && start.1 < editing::len(dom, *first) {
            if let Ok(rest) = dom.split_text(*first, start.1) {
                *first = rest;
            }
        } else if *first == start.0 && start.1 >= editing::len(dom, *first) {
            texts.remove(0);
        }
    }
    texts
}

fn select_texts(page: &Rc<PageDom>, texts: &[NodeId]) {
    let (Some(&first), Some(&last)) = (texts.first(), texts.last()) else {
        return;
    };
    let len = editing::len(&page.dom.borrow(), last);
    editing::set_selection(page, (first, 0), (last, len));
}

// Subscript and superscript keep their elements: both set the font size
// as well as the vertical alignment.
fn inline_css(tag: &str) -> Option<(&'static str, &'static str)> {
    Some(match tag {
        "b" => ("font-weight", "bold"),
        "i" => ("font-style", "italic"),
        "u" => ("text-decoration", "underline"),
        "s" => ("text-decoration", "line-through"),
        _ => return None,
    })
}

/// Wraps the selected text in `tag` (with `href` for links), skipping text
/// that already has one of `carriers` around it.
fn format_inline(page: &Rc<PageDom>, host: NodeId, tag: &str, carriers: &[&str], input_type: &str, href: Option<&str>) {
    let (start, end) = editing::host_range(page, host);
    let collapsed = start == end;
    editing::apply_edit(page, host, input_type, href, None, |page| {
        let texts = {
            let mut dom = page.dom.borrow_mut();
            let texts = if collapsed {
                // createLink at a caret inserts the URL as the link's text.
                let p = editing::insert_at(&mut dom, start, href.unwrap_or(""));
                if p.0 == start.0 && p.1 > start.1 {
                    split_texts(&mut dom, host, start, p)
                } else {
                    vec![p.0]
                }
            } else {
                split_texts(&mut dom, host, start, end)
            };
            let Ok(doc) = dom.node_document(host) else {
                return;
            };
            let mut last_wrapper: Option<NodeId> = None;
            for &t in &texts {
                let carried = carriers.iter().any(|c| ancestor_named(&dom, t, host, &[c]).is_some());
                if carried {
                    continue;
                }
                let Ok(Some(parent)) = dom.parent(t) else {
                    continue;
                };
                let prev = dom.prev_sibling(t).ok().flatten();
                if let Some(w) = last_wrapper.filter(|&w| prev == Some(w)) {
                    let _ = dom.append_child(w, t);
                    continue;
                }
                let style = if dom.document_data(doc).is_ok_and(|d| d.style_with_css) { inline_css(tag) } else { None };
                let Ok(w) = dom.create_element(doc, if style.is_some() { "span" } else { tag }) else {
                    continue;
                };
                if let Some((property, value)) = style {
                    let _ = dom.set_attribute(w, "style", &format!("{property}: {value};"));
                }
                if let Some(href) = href {
                    let _ = dom.set_attribute(w, "href", href);
                }
                let _ = dom.insert_before(parent, w, Some(t));
                let _ = dom.append_child(w, t);
                last_wrapper = Some(w);
            }
            texts
        };
        select_texts(page, &texts);
    });
}

fn ancestor_named(dom: &Dom, node: NodeId, host: NodeId, names: &[&str]) -> Option<NodeId> {
    ancestor_format(dom, node, host, names, false)
}

fn ancestor_format(dom: &Dom, node: NodeId, host: NodeId, names: &[&str], removing: bool) -> Option<NodeId> {
    let mut cur = dom.parent_element(node).ok().flatten();
    while let Some(c) = cur {
        if c == host {
            return None;
        }
        let tag = editing::local(dom, c);
        let css_carrier = tag == "span"
            && dom.attributes(c).is_ok_and(|attrs| attrs.len() == 1)
            && dom.get_attribute(c, "style").ok().flatten().is_some_and(|text| {
                let declarations = crate::css::parse_declarations(text);
                let [declaration] = declarations.as_slice() else {
                    return false;
                };
                names.iter().filter_map(|tag| inline_css(tag)).any(|(property, value)| {
                    declaration.name.eq_ignore_ascii_case(property)
                            && (declaration.value.eq_ignore_ascii_case(value)
                                || removing && property == "font-style" && declaration.value.eq_ignore_ascii_case("oblique"))
                })
            });
        if names.contains(&tag.as_str()) || css_carrier {
            return Some(c);
        }
        cur = dom.parent_element(c).ok().flatten();
    }
    None
}

/// Moves `node` out of its ancestor `anc`, splitting `anc` and the elements
/// between them so that what comes before and after stays where it was.
fn hoist(dom: &mut Dom, node: NodeId, anc: NodeId) {
    let mut cur = node;
    loop {
        let Ok(Some(p)) = dom.parent(cur) else { return };
        let Ok(Some(gp)) = dom.parent(p) else { return };
        let Ok(after) = dom.clone_node(p, false, None) else { return };
        let mut next = dom.next_sibling(cur).ok().flatten();
        while let Some(n) = next {
            next = dom.next_sibling(n).ok().flatten();
            let _ = dom.append_child(after, n);
        }
        let p_next = dom.next_sibling(p).ok().flatten();
        let moved = if p == anc {
            cur
        } else {
            let Ok(mid) = dom.clone_node(p, false, None) else { return };
            let _ = dom.append_child(mid, cur);
            mid
        };
        let _ = dom.insert_before(gp, moved, p_next);
        if dom.child_count(after).unwrap_or(0) > 0 {
            let after_moved = dom.next_sibling(moved).ok().flatten();
            let _ = dom.insert_before(gp, after, after_moved);
        }
        if dom.child_count(p).unwrap_or(0) == 0 {
            let _ = dom.remove_child(gp, p);
        }
        if p == anc {
            return;
        }
        cur = moved;
    }
}

/// Takes the selected text out of the elements named `names` (unlink,
/// removeFormat, toggling bold off).
fn remove_inline(page: &Rc<PageDom>, host: NodeId, names: &[&str], input_type: &str) {
    let (start, end) = editing::host_range(page, host);
    if start == end && !names.contains(&"a") {
        return;
    }
    editing::apply_edit(page, host, input_type, None, None, |page| {
        let texts = {
            let mut dom = page.dom.borrow_mut();
            let (start, end) = if start == end {
                // unlink at a caret removes the whole link around it.
                let Some(a) = ancestor_named(&dom, start.0, host, &["a"]) else { return };
                (editing::host_start(&dom, a), editing::host_end(&dom, a))
            } else {
                (start, end)
            };
            let texts = split_texts(&mut dom, host, start, end);
            for &t in &texts {
                while let Some(a) = ancestor_format(&dom, t, host, names, true) {
                    hoist(&mut dom, t, a);
                    // A hoist that could not move the text would repeat.
                    if ancestor_format(&dom, t, host, names, true) == Some(a) {
                        break;
                    }
                }
            }
            texts
        };
        select_texts(page, &texts);
    });
}

/// The blocks of `host` the selection touches (the host itself when its
/// content has no blocks).
fn selected_blocks(dom: &Dom, host: NodeId, start: Point, end: Point) -> Vec<NodeId> {
    let mut out: Vec<NodeId> = Vec::new();
    let texts = texts_in(dom, host, start, end);
    let points: Vec<NodeId> = if texts.is_empty() { vec![start.0] } else { texts };
    for n in points {
        let b = editing::block_of(dom, n, host);
        if !out.contains(&b) {
            out.push(b);
        }
    }
    out
}

/// formatBlock: the selected blocks become `tag` elements (content of a
/// host without blocks is wrapped in one).
fn format_block(page: &Rc<PageDom>, host: NodeId, tag: &str) {
    let (start, end) = editing::host_range(page, host);
    editing::apply_edit(page, host, "formatBlock", None, None, |page| {
        let selection = {
            let mut dom = page.dom.borrow_mut();
            let Ok(doc) = dom.node_document(host) else { return };
            let blocks = selected_blocks(&dom, host, start, end);
            for b in blocks {
                let Ok(new) = dom.create_element(doc, tag) else { continue };
                let kids: Vec<NodeId> = dom.children(b).map(|c| c.collect()).unwrap_or_default();
                for k in kids {
                    let _ = dom.append_child(new, k);
                }
                if b == host {
                    let _ = dom.append_child(host, new);
                } else if let Ok(Some(p)) = dom.parent(b) {
                    let _ = dom.insert_before(p, new, Some(b));
                    let _ = dom.remove_child(p, b);
                }
            }
            let live = |p: Point| dom.is_connected(p.0).unwrap_or(false);
            (live(start) && live(end)).then_some((start, end))
        };
        let (s, e) = selection.unwrap_or_else(|| {
            let p = editing::host_start(&page.dom.borrow(), host);
            (p, p)
        });
        editing::set_selection(page, s, e);
    });
}

/// insertOrderedList / insertUnorderedList: the selected blocks become
/// items of a new list; inside such a list already, its items become `div`
/// lines again.
fn toggle_list(page: &Rc<PageDom>, host: NodeId, tag: &str, input_type: &str) {
    let (start, end) = editing::host_range(page, host);
    editing::apply_edit(page, host, input_type, None, None, |page| {
        {
            let mut dom = page.dom.borrow_mut();
            let Ok(doc) = dom.node_document(host) else { return };
            let in_list = ancestor_named(&dom, start.0, host, &[tag]);
            if let Some(list) = in_list {
                let Ok(Some(parent)) = dom.parent(list) else { return };
                let items: Vec<NodeId> = dom.children(list).map(|c| c.collect()).unwrap_or_default();
                for item in items {
                    let Ok(div) = dom.create_element(doc, "div") else { continue };
                    let kids: Vec<NodeId> = dom.children(item).map(|c| c.collect()).unwrap_or_default();
                    for k in kids {
                        let _ = dom.append_child(div, k);
                    }
                    let _ = dom.insert_before(parent, div, Some(list));
                }
                let _ = dom.remove_child(parent, list);
            } else {
                let blocks = selected_blocks(&dom, host, start, end);
                let Ok(list) = dom.create_element(doc, tag) else { return };
                let first = blocks[0];
                let (parent, before) = if first == host {
                    (host, dom.child_at(host, 0).ok().flatten())
                } else {
                    (dom.parent(first).ok().flatten().unwrap_or(host), Some(first))
                };
                let _ = dom.insert_before(parent, list, before);
                for b in blocks {
                    let Ok(li) = dom.create_element(doc, "li") else { continue };
                    let kids: Vec<NodeId> = dom.children(b).map(|c| c.collect()).unwrap_or_default();
                    for k in kids {
                        if k != list {
                            let _ = dom.append_child(li, k);
                        }
                    }
                    let _ = dom.append_child(list, li);
                    if b != host {
                        if let Ok(Some(p)) = dom.parent(b) {
                            let _ = dom.remove_child(p, b);
                        }
                    }
                }
            }
        }
        let live = {
            let dom = page.dom.borrow();
            dom.is_connected(start.0).unwrap_or(false) && dom.is_connected(end.0).unwrap_or(false)
        };
        if live {
            editing::set_selection(page, start, end);
        }
    });
}

/// (any, all) of the selected text (or the caret's position) has the
/// inline style of `command`, by computed style as Chromium reads it.
fn format_state(page: &PageDom, command: &str) -> (bool, bool) {
    let Some((start, end)) = editing::selection_range(page) else { return (false, false) };
    let dom = page.dom.borrow();
    let doc = page.document();
    let scope = dom.body(doc).ok().flatten().or_else(|| dom.document_element(doc).ok().flatten());
    let Some(scope) = scope else { return (false, false) };
    let mut elements: Vec<NodeId> = if start == end {
        vec![start.0]
    } else {
        texts_in(&dom, scope, start, end)
            .into_iter()
            .filter(|&t| !dom.char_data(t).unwrap_or("").trim().is_empty())
            .collect()
    };
    if elements.is_empty() {
        elements.push(start.0);
    }
    let layout = page.layout();
    let has = |n: NodeId| -> bool {
        let el = if dom.node_type(n).ok() == Some(NodeType::Element) {
            Some(n)
        } else {
            dom.parent_element(n).ok().flatten()
        };
        let Some(el) = el else { return false };
        let value = |name: &str| layout.computed_style_value(&dom, el, name);
        // Text decorations do not inherit: any ancestor's counts.
        let decorated = |line: &str| {
            let mut cur = Some(el);
            while let Some(c) = cur {
                if layout.computed_style_value(&dom, c, "text-decoration-line").contains(line) {
                    return true;
                }
                cur = dom.parent_element(c).ok().flatten();
            }
            false
        };
        match command {
            "bold" => value("font-weight").parse::<f64>().is_ok_and(|w| w >= 600.0),
            "italic" => {
                matches!(value("font-style").as_str(), s if s.starts_with("italic") || s.starts_with("oblique"))
            }
            "underline" => decorated("underline"),
            "strikethrough" => decorated("line-through"),
            "superscript" => ancestor_named(&dom, n, scope, &["sup"]).is_some() || value("vertical-align") == "super",
            "subscript" => ancestor_named(&dom, n, scope, &["sub"]).is_some() || value("vertical-align") == "sub",
            _ => false,
        }
    };
    let mut any = false;
    let mut all = true;
    for e in elements {
        if has(e) {
            any = true;
        } else {
            all = false;
        }
    }
    (any, all)
}

/// `document.queryCommandState(command)`.
pub(crate) fn query_state(page: &PageDom, doc: NodeId, command: &str) -> bool {
    let cmd = command.to_ascii_lowercase();
    if doc != page.document() {
        return false;
    }
    if let Some(target) = alignment::value_for(&cmd) {
        let values = alignment::selected_values(page);
        return !values.is_empty() && values.iter().all(|&v| v == target);
    }
    match cmd.as_str() {
        "stylewithcss" => page.dom.borrow().document_data(doc).is_ok_and(|d| d.style_with_css),
        "insertorderedlist" | "insertunorderedlist" => {
            let tag = if cmd == "insertorderedlist" { "ol" } else { "ul" };
            let Some((start, _)) = editing::selection_range(page) else {
                return false;
            };
            let dom = page.dom.borrow();
            let Some(host) = editing::editing_host(&dom, start.0) else {
                return false;
            };
            ancestor_named(&dom, start.0, host, &[tag]).is_some()
        }
        _ if format_of(&cmd).is_some() => format_state(page, &cmd).1,
        _ => false,
    }
}

/// `document.queryCommandIndeterm(command)`: part of the selection only.
pub(crate) fn query_indeterm(page: &PageDom, command: &str) -> bool {
    let cmd = command.to_ascii_lowercase();
    if let Some(kind) = inline_value::Kind::command(&cmd) {
        return inline_value::mixed(page, kind);
    }
    if let Some(target) = alignment::value_for(&cmd) {
        let values = alignment::selected_values(page);
        return values.iter().any(|&v| v == target) && values.iter().any(|&v| v != target);
    }
    if format_of(&cmd).is_none() {
        return false;
    }
    let (any, all) = format_state(page, &cmd);
    any && !all
}

/// `document.queryCommandValue(command)`.
pub(crate) fn query_value(page: &PageDom, doc: NodeId, command: &str) -> String {
    let cmd = command.to_ascii_lowercase();
    if doc != page.document() {
        return String::new();
    }
    if let Some(kind) = inline_value::Kind::command(&cmd) {
        return inline_value::query(page, kind);
    }
    match cmd.as_str() {
        cmd if alignment::value_for(cmd).is_some() => {
            if editing::selection_range(page).is_none() { return String::new(); }
            alignment::selected_values(page).first().copied().unwrap_or("left").to_owned()
        }
        "formatblock" => {
            let Some((start, _)) = editing::selection_range(page) else {
                return String::new();
            };
            let dom = page.dom.borrow();
            let Some(host) = editing::editing_host(&dom, start.0) else {
                return String::new();
            };
            ancestor_named(&dom, start.0, host, FORMAT_BLOCKS).map(|b| editing::local(&dom, b)).unwrap_or_default()
        }
        "defaultparagraphseparator" => {
            page.dom.borrow().document_data(doc).map(|d| d.default_paragraph_separator.to_owned()).unwrap_or_default()
        }
        _ if format_of(&cmd).is_some() || cmd.starts_with("insert") && cmd.ends_with("list") => {
            query_state(page, doc, &cmd).to_string()
        }
        _ => String::new(),
    }
}

/// Rich editing commands require a range in this host. Plaintext-only can
/// be inherited or overridden within the same editable tree.
fn rich_selection(page: &PageDom, host: NodeId) -> bool {
    let Some((start, end)) = editing::selection_range(page) else { return false };
    let dom = page.dom.borrow();
    [start.0, end.0].into_iter().all(|node| {
        editing::editing_host(&dom, node) == Some(host) && !editing::plaintext_at(&dom, node)
    })
}

/// `document.queryCommandEnabled(command)`.
pub(crate) fn query_enabled(page: &PageDom, doc: NodeId, command: &str) -> bool {
    let cmd = command.to_ascii_lowercase();
    if doc != page.document() || !query_supported(&cmd) {
        return false;
    }
    if requires_rich_selection(&cmd) {
        return matches!(command_target(page), Some(Target::Host(h)) if rich_selection(page, h));
    }
    let has_range = || match command_target(page) {
        Some(Target::Control(c)) => page.dom.borrow().selection(c).is_ok_and(|(s, e)| s != e),
        _ => editing::selection_range(page).is_some_and(|(s, e)| s != e),
    };
    match cmd.as_str() {
        "selectall" | "stylewithcss" | "usecss" | "defaultparagraphseparator" => true,
        "inserthtml" => match command_target(page) {
            Some(Target::Control(c)) => page.dom.borrow().is_mutable_text(c).unwrap_or(false),
            Some(Target::Host(_)) => true,
            _ => false,
        },
        "copy" => has_range(),
        "cut" => command_target(page).is_some() && has_range(),
        "undo" => !page.undo.borrow().undo.is_empty(),
        "redo" => !page.undo.borrow().redo.is_empty(),
        "inserttext" | "delete" | "forwarddelete" | "insertlinebreak" | "insertparagraph" => {
            command_target(page).is_some()
        }
        _ => matches!(command_target(page), Some(Target::Host(_))),
    }
}

// ----- undo -------------------------------------------------------------------------

/// The page's undo and redo steps (one history per document, as in
/// Chromium). A step holds what its target looked like before the edit: a
/// text control's value and selection, an editing host's markup and caret
/// (as flat offsets). Consecutive typing at the caret the last typing left
/// is one step. Nothing is recorded until something is edited.
#[derive(Default)]
pub(crate) struct UndoHistory {
    undo: Vec<Step>,
    redo: Vec<Step>,
    /// The typing group still open: its target and the caret it left.
    typing: Option<(NodeId, Caret)>,
    /// The edit being made is typing (it keeps the group open).
    pending_typing: bool,
    /// Undo or redo is restoring a step: edits are not recorded.
    replaying: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Caret {
    Control(u32, u32),
    Host(Point),
}

enum Saved {
    Control { value: String, selection: (u32, u32) },
    Host { html: String, selection: Option<(u32, u32)> },
}

struct Step {
    target: NodeId,
    saved: Saved,
}

const UNDO_LIMIT: usize = 100;
/// Bytes of saved values and markup the undo steps keep at most (the
/// oldest go first; the latest step is always kept).
const UNDO_BYTES: usize = 16 << 20;

impl Saved {
    fn bytes(&self) -> usize {
        match self {
            Saved::Control { value, .. } => value.len(),
            Saved::Host { html, .. } => html.len(),
        }
    }
}

impl UndoHistory {
    fn push(&mut self, step: Step) {
        self.undo.push(step);
        self.redo.clear();
        let mut drop = self.undo.len().saturating_sub(UNDO_LIMIT);
        let mut total: usize = self.undo[drop..].iter().map(|s| s.saved.bytes()).sum();
        while total > UNDO_BYTES && drop + 1 < self.undo.len() {
            total -= self.undo[drop].saved.bytes();
            drop += 1;
        }
        if drop > 0 {
            self.undo.drain(..drop);
        }
    }
}

fn save(page: &PageDom, target: NodeId) -> Option<Saved> {
    let dom = page.dom.borrow();
    if editing::is_text_entry_control(&dom, target) {
        return Some(Saved::Control { value: dom.value(target).ok()?, selection: dom.selection(target).ok()? });
    }
    let html = dom.inner_html(target).ok()?;
    drop(dom);
    let selection = editing::selection_range(page).and_then(|(s, e)| {
        let dom = page.dom.borrow();
        let inside =
            editing::editing_host(&dom, s.0) == Some(target) && editing::editing_host(&dom, e.0) == Some(target);
        drop(dom);
        inside.then(|| {
            let flat = Flat::build(page, target);
            let dom = page.dom.borrow();
            (flat.pos_of(&dom, s), flat.pos_of(&dom, e))
        })
    });
    Some(Saved::Host { html, selection })
}

fn caret(page: &PageDom, target: NodeId, control: bool) -> Option<Caret> {
    if control {
        let (s, e) = page.dom.borrow().selection(target).ok()?;
        return Some(Caret::Control(s, e));
    }
    let (a, f) = editing::selection(page)?;
    (a == f).then_some(Caret::Host(f))
}

fn record(page: &PageDom, target: NodeId, input_type: &str, control: bool) {
    let typing = input_type == "insertText";
    {
        let mut h = page.undo.borrow_mut();
        if h.replaying {
            return;
        }
        h.pending_typing = typing;
        let open = h.typing;
        drop(h);
        if typing && open.is_some_and(|(t, c)| t == target && caret(page, target, control) == Some(c)) {
            return;
        }
    }
    let Some(saved) = save(page, target) else { return };
    let mut h = page.undo.borrow_mut();
    h.typing = None;
    h.push(Step { target, saved });
}

fn note(page: &PageDom, target: NodeId, control: bool) {
    let typing = std::mem::take(&mut page.undo.borrow_mut().pending_typing);
    let open = if typing { caret(page, target, control).map(|c| (target, c)) } else { None };
    let mut h = page.undo.borrow_mut();
    if !h.replaying {
        h.typing = open;
    }
}

/// Before a text control's value changes by an edit.
pub(crate) fn record_control_edit(page: &PageDom, el: NodeId, input_type: &str) {
    record(page, el, input_type, true);
}

/// After that edit.
pub(crate) fn note_control_edited(page: &PageDom, el: NodeId) {
    note(page, el, true);
}

/// Before an editing host's content changes by an edit.
pub(crate) fn record_host_edit(page: &PageDom, host: NodeId, input_type: &str) {
    record(page, host, input_type, false);
}

/// After that edit.
pub(crate) fn note_host_edited(page: &PageDom, host: NodeId) {
    note(page, host, false);
}

fn restore(page: &Rc<PageDom>, target: NodeId, saved: Saved) {
    match saved {
        Saved::Control { value, selection } => {
            {
                let mut dom = page.dom.borrow_mut();
                let len = dom.value(target).map(|v| pyro_dom::dom::utf16_len(&v)).unwrap_or(0);
                let _ = dom.set_selection_range(target, 0, len);
                let _ = dom.insert_text(target, &value);
                let _ = dom.set_selection_range(target, selection.0, selection.1);
            }
            editing::queue_control_selectionchange(page, target);
        }
        Saved::Host { html, selection } => {
            let ok = pyro_dom::parse::set_inner_html(&mut page.dom.borrow_mut(), target, &html).is_ok();
            if !ok {
                return;
            }
            let flat = Flat::build(page, target);
            let points = selection.and_then(|(s, e)| Some((flat.point_at(s, true)?, flat.point_at(e, false)?)));
            let (s, e) = points.unwrap_or_else(|| {
                let p = editing::host_start(&page.dom.borrow(), target);
                (p, p)
            });
            editing::set_selection(page, s, e);
        }
    }
}

/// Undo (`backward`) or redo the last step: `beforeinput` historyUndo /
/// historyRedo (cancelable), the restore, then `input`. Returns whether a
/// step was there.
fn undo(page: &Rc<PageDom>, backward: bool, source: EditSource) -> bool {
    let step = {
        let mut h = page.undo.borrow_mut();
        h.typing = None;
        if backward {
            h.undo.pop()
        } else {
            h.redo.pop()
        }
    };
    let Some(step) = step else { return false };
    if !editing::connected(page, step.target) {
        return false;
    }
    let input_type = if backward { "historyUndo" } else { "historyRedo" };
    let doc = page.document();
    let beforeinput = source == EditSource::User || matches!(&step.saved, Saved::Host { .. });
    if beforeinput && !editing::input_event(page, step.target, "beforeinput", input_type, None, true)
        || page.document() != doc
        || !editing::connected(page, step.target)
    {
        let mut h = page.undo.borrow_mut();
        if backward {
            h.undo.push(step);
        } else {
            h.redo.push(step);
        }
        return true;
    }
    let current = save(page, step.target);
    page.undo.borrow_mut().replaying = true;
    restore(page, step.target, step.saved);
    {
        let mut h = page.undo.borrow_mut();
        h.replaying = false;
        if let Some(saved) = current {
            let s = Step { target: step.target, saved };
            if backward {
                h.redo.push(s);
            } else {
                h.undo.push(s);
            }
        }
    }
    editing::input_event(page, step.target, "input", input_type, None, false);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn words_skip_symbols_and_spaces() {
        let t = u("~>>>>>p+++");
        assert_eq!(word_start(&t, 6), 0);
        assert_eq!(word_end(&t, 7), 10);
        let t = u("hello brave  new");
        assert_eq!(word_end(&t, 0), 5);
        assert_eq!(word_end(&t, 5), 11);
        assert_eq!(word_start(&t, 16), 13);
        assert_eq!(word_start(&t, 13), 6);
        assert_eq!(word_end(&u("don't stop"), 0), 5);
    }

    #[test]
    fn double_click_runs() {
        let t = u("foo bar, baz");
        assert_eq!(word_around(&t, 5), (4, 7));
        assert_eq!(word_around(&t, 3), (3, 4));
        assert_eq!(word_around(&t, 7), (7, 8));
        assert_eq!(word_around(&t, 12), (9, 12));
    }

    #[test]
    fn lines_and_paragraphs() {
        let t: Vec<u16> = vec![b'a' as u16, b'b' as u16, LINE, b'c' as u16, PARA, b'd' as u16, b'e' as u16];
        assert_eq!(target(&t, 1, true, Gran::LineBoundary), 2);
        assert_eq!(target(&t, 3, false, Gran::LineBoundary), 3);
        assert_eq!(target(&t, 1, true, Gran::ParagraphBoundary), 4);
        assert_eq!(target(&t, 1, true, Gran::Line), 4);
        assert_eq!(target(&t, 6, false, Gran::Line), 4);
        assert_eq!(target(&t, 0, false, Gran::Line), 0);
        assert_eq!(target(&t, 3, true, Gran::DocumentBoundary), 7);
    }

    #[test]
    fn sentences() {
        let t = u("One two. Three four. Five");
        assert_eq!(target(&t, 0, true, Gran::Sentence), 8);
        assert_eq!(target(&t, 8, true, Gran::Sentence), 20);
        assert_eq!(target(&t, 15, false, Gran::Sentence), 9);
        assert_eq!(target(&t, 9, false, Gran::Sentence), 0);
    }

    #[test]
    fn commands_map_to_operations() {
        assert_eq!(command_op("deleteWordBackward"), Some(Op::Delete(false, Gran::Word)));
        assert_eq!(command_op("moveWordLeftAndModifySelection"), Some(Op::Move(false, Gran::Word, true)));
        assert_eq!(command_op("selectAll"), Some(Op::SelectAll));
        assert_eq!(command_op("scrollToBeginningOfDocument"), None);
        assert_eq!(command_op("deleteBackwardAndModifySelection"), None);
    }
}
