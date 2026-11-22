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

pub(crate) mod alignment;
mod decoration;
mod indentation;
pub(crate) mod inline_value;

use std::rc::Rc;

use pyro_dom::{forms::ControlSelection, Dom, NodeId, NodeType};

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
        match range {
            Some(range) => dom.replace_text_in_range(el, range, text),
            None => dom.insert_text(el, text),
        }.unwrap_or(false)
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
        "strikethrough" => ("strike", &["strike", "s"], "formatStrikeThrough"),
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
    if requires_rich_selection(&cmd) && !rich_selection(page, host, &cmd) {
        return false;
    }
    if inline_value::format_spaced_run(page, host, &cmd, value) { return true; }
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
        "inserttext" => {
            // A visible caret can be positioned in a locked descendant.
            // Falling back to the focused host would type at a different point.
            if editing::selection_range(page).is_some_and(|(a, b)| a == b && {
                let dom = page.dom.borrow();
                dom.contains(host, a.0).unwrap_or(false) && !editing::is_editable(&dom, a.0)
            }) { return false; }
            editing::insert_text(page, host, value);
        }
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
                "acronym", "b", "bdo", "big", "cite", "code", "dfn", "em", "font", "i", "ins", "kbd", "mark",
                "nobr", "q", "s", "samp", "small", "span", "strike", "strong", "sub", "sup", "tt", "u", "var",
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
            if touching_inline_text(page, &page.dom.borrow(), start, end) {
                if matches!(tag, "u" | "strike") {
                    inline_value::toggle_decoration(page, (start, end), tag);
                } else {
                    let enabled = !format_state(page, &cmd).1;
                    inline_value::set_toggle(page, (start, end), tag, enabled);
                }
                return true;
            }
            if matches!(tag, "u" | "strike") {
                let pending = start == end || {
                    let dom = page.dom.borrow();
                    !accepts_html_formatting(&dom, start.0) && !accepts_html_formatting(&dom, end.0)
                        && (start.0 == end.0 || texts_in(&dom, host, start, end).into_iter()
                            .all(|t| !accepts_html_formatting(&dom, t)))
                };
                if pending {
                    inline_value::toggle_decoration(page, (start, end), tag);
                    return true;
                }
            }
            if start == end && matches!(tag, "b" | "i" | "sub" | "sup")
                || tag == "b" && empty_inline_caret(page, &page.dom.borrow(), start, end).is_some() {
                let enabled = !format_state(page, &cmd).1;
                inline_value::set_toggle(page, (start, end), tag, enabled);
                return true;
            }
            if start == end {
                return false;
            }
            if matches!(tag, "u" | "strike") && decoration::change_selected(page, host, start, end, tag, input_type) {
                return true;
            }
            if matches!(tag, "sub" | "sup") && inline_value::change_selected_script(page, host, start, end, tag, input_type) {
                return true;
            }
            if format_state(page, &cmd).1 || tag == "i" && starts_in_semantic_italic(page, start, end) {
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

fn rich_node(dom: &Dom, node: NodeId) -> bool {
    editing::is_editable(dom, node) && !editing::plaintext_at(dom, node)
}

/// A locked island is skipped, but a rich descendant can opt back into editing.
/// The caller restricts the walk to the selection's enclosing editing host.
fn inline_texts(page: &PageDom, dom: &Dom, host: NodeId, start: Point, end: Point) -> Vec<NodeId> {
    let mut texts = texts_in(dom, host, start, end);
    texts.retain(|&node| rich_node(dom, node) && !editing::invisible_formatting_space(page, dom, host, node));
    texts
}

/// A rendered BR in an intervening paragraph is a formatting leaf just like
/// its neighboring text. Keep the text endpoints for selection restoration.
fn include_paragraph_breaks(page: &PageDom, dom: &Dom, host: NodeId, texts: Vec<NodeId>) -> Vec<NodeId> {
    let (Some(&first), Some(&last)) = (texts.first(), texts.last()) else { return texts };
    if first == last || dom.parent(first).ok() == dom.parent(last).ok() { return texts; }
    let Ok(descendants) = dom.descendants(host) else { return texts };
    let mut result = Vec::with_capacity(texts.len());
    let mut selected = texts.iter().copied().peekable();
    for node in descendants.skip_while(|&node| node != first) {
        if selected.peek() == Some(&node) {
            result.push(node);
            selected.next();
            if node == last { break; }
        } else if dom.element(node).is_ok_and(|e| e.is_html() && e.local_name() == &markup5ever::local_name!("br"))
            && rich_node(dom, node) && editing::editing_host(dom, node) == Some(host)
            && ancestor_named(dom, node, host, &["p"]).is_some_and(|p|
                !dom.contains(p, first).unwrap_or(true) && !dom.contains(p, last).unwrap_or(true))
            && alignment::visible(page, dom, node)
        {
            result.push(node);
        }
    }
    result
}

/// Splits the text nodes at the range's ends so that whole text nodes make
/// it up; returns them in order.
fn split_texts(dom: &mut Dom, mut texts: Vec<NodeId>, start: Point, end: Point) -> Vec<NodeId> {
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

fn select_texts(page: &Rc<PageDom>, texts: &[NodeId], reversed: bool) {
    let (Some(&first), Some(&last)) = (texts.first(), texts.last()) else {
        return;
    };
    let len = editing::len(&page.dom.borrow(), last);
    let (start, end) = ((first, 0), (last, len));
    if reversed {
        editing::set_selection(page, end, start);
    } else {
        editing::set_selection(page, start, end);
    }
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

/// SVG renders character data under text and its inline text elements.
/// A foreignObject's HTML descendants use ordinary HTML editing.
fn accepts_html_formatting(dom: &Dom, text: NodeId) -> bool {
    let mut parent = dom.parent_element(text).ok().flatten();
    while let Some(node) = parent {
        let Ok(el) = dom.element(node) else { return false };
        if el.namespace() != &markup5ever::ns!(svg) { return true; }
        match el.local_name().as_ref() {
            "text" => return true,
            "tspan" | "textPath" | "a" => parent = dom.parent_element(node).ok().flatten(),
            _ => return false,
        }
    }
    false
}

/// Once HTML formatting wraps SVG text, that text no longer paints in the
/// SVG text run. Restore the range to a visible caret in the same text
/// element, or to the next editable text when its whole run disappeared.
fn restored_format_point(page: &PageDom, dom: &Dom, host: NodeId, point: Point) -> Point {
    let Some(wrapper) = dom.parent_element(point.0).ok().flatten() else { return point };
    if !dom.element(wrapper).is_ok_and(|el| el.is_html()) { return point; }
    let mut root = wrapper;
    loop {
        let Some(parent) = dom.parent_element(root).ok().flatten() else { return point };
        let Ok(el) = dom.element(parent) else { return point };
        if el.namespace() != &markup5ever::ns!(svg) { return point; }
        root = parent;
        match el.local_name().as_ref() {
            "text" => break,
            "tspan" | "textPath" | "a" => {}
            _ => return point,
        }
    }
    let Ok(nodes) = dom.descendants(host) else { return point };
    let mut previous = None;
    let mut previous_in_run = None;
    let mut past = false;
    for n in nodes {
        if n == point.0 { past = true; continue; }
        if !editing::is_text(dom, n) || editing::len(dom, n) == 0 || editing::editing_host(dom, n) != Some(host) {
            continue;
        }
        let in_run = dom.contains(root, n).unwrap_or(false);
        if in_run && !dom.parent_element(n).ok().flatten().is_some_and(|p|
            dom.element(p).is_ok_and(|el| el.namespace() == &markup5ever::ns!(svg)))
        { continue; }
        if !accepts_html_formatting(dom, n) || !editing::rendered(page, dom, n) { continue; }
        if past { return previous_in_run.unwrap_or((n, 0)); }
        previous = Some((n, editing::len(dom, n)));
        if in_run { previous_in_run = previous; }
    }
    previous.or_else(|| dom.parent(root).ok().flatten().zip(dom.index_in_parent(root).ok()).map(|(p, i)| (p, i+1)))
        .unwrap_or(point)
}

/// Drop the semantic weight while retaining attributes, including the ID
/// and unrelated presentation, on a neutral replacement.
fn neutralize_bold(dom: &mut Dom, element: NodeId, points: &mut [Point; 2]) -> Option<()> {
    if dom.attributes(element).ok()?.is_empty() {
        indentation::unwrap(dom, element, points)
    } else {
        indentation::rename(dom, element, "span", points).map(|_| ())
    }
}

/// A stylesheet reset can contain a selected bold run between ordinary text
/// siblings. When removing the enclosing semantic weight, apply it directly
/// to the unselected runs and keep the selected text in its reset span.
fn remove_reset_child_bold(page: &PageDom, dom: &mut Dom, host: NodeId, text: NodeId) -> Option<()> {
    use pyro_dom::stylo::declarations::DeclarationBlock;
    let inner = dom.parent_element(text).ok()??;
    let inner_style = DeclarationBlock::parse(dom.get_attribute(inner, "style").ok()?.unwrap_or(""));
    let direct = editing::local(dom, inner) == "span" && dom.has_attribute(inner, "class").ok()?
        && inner_style.value("font-weight") == "bold";
    let reset = if direct { inner } else { dom.parent_element(inner).ok()?? };
    let outer = dom.parent_element(reset).ok()??;
    if outer == host || !matches!(editing::local(dom, outer).as_str(), "b" | "strong")
        || editing::local(dom, reset) != "span" || !dom.has_attribute(reset, "class").ok()?
        || dom.child_count(inner).ok()? != 1 { return None; }
    if !direct {
        let inner_tag = editing::local(dom, inner);
        let simple = inner_tag == "b" && dom.attributes(inner).ok()?.is_empty()
            || inner_tag == "span" && dom.attributes(inner).ok()?.len() == 1
                && inner_style.names().len() == 1 && inner_style.value("font-weight") == "bold";
        if !simple || FontToggle::Bold.active(&page.layout().computed_style_value(dom, reset, "font-weight")) { return None; }
    }
    let selected = if direct { text } else { inner };
    let siblings: Vec<_> = dom.children(reset).ok()?.filter(|&n| n != selected).collect();
    if !siblings.iter().all(|&n| editing::is_text(dom, n)) { return None; }
    let outside: Vec<_> = dom.children(outer).ok()?.filter(|&n| n != reset).collect();
    if !outside.iter().all(|&n| editing::is_text(dom, n) || !editing::is_editable(dom, n)) { return None; }
    // Only the affected outer siblings lose inherited weight when B is
    // neutralized. Preserve a locked element and its children in place.
    let layout = page.layout();
    let affected: Vec<_> = outside.into_iter().filter(|&n| {
        let element = if editing::is_text(dom, n) { outer } else { n };
        FontToggle::Bold.active(&layout.computed_style_value(dom, element, "font-weight"))
    }).collect();
    let doc = dom.node_document(text).ok()?;
    let css = dom.document_data(doc).ok()?.style_with_css;
    let mut points = [(text, 0), (text, editing::len(dom, text))];
    neutralize_bold(dom, outer, &mut points)?;
    if !direct { indentation::unwrap(dom, inner, &mut points)?; }
    let mut retained = DeclarationBlock::parse(dom.get_attribute(reset, "style").ok()?.unwrap_or(""));
    retained.remove("font-weight");
    dom.set_attribute(reset, "style", &retained.css_text()).ok()?;
    for sibling in siblings.into_iter().chain(affected) {
        let parent = dom.parent_element(sibling).ok()??;
        if editing::is_text(dom, sibling) {
            let wrapper = dom.create_element(doc, if css { "span" } else { "b" }).ok()?;
            if css { dom.set_attribute(wrapper, "style", "font-weight: bold;").ok()?; }
            dom.insert_before(parent, wrapper, Some(sibling)).ok()?;
            dom.append_child(wrapper, sibling).ok()?;
        } else if !FontToggle::Bold.active(&layout.computed_style_value(dom, sibling, "font-weight")) {
            let mut style = DeclarationBlock::parse(dom.get_attribute(sibling, "style").ok()?.unwrap_or(""));
            style.set("font-weight", "bold", false)?;
            dom.set_attribute(sibling, "style", &style.css_text()).ok()?;
        }
    }
    Some(())
}

/// A semantic bold ancestor may be overridden closer to the selected text.
/// Remove the selected inline reset, or apply bold below a stylesheet reset.
fn prepare_overridden_bold(page: &PageDom, dom: &mut Dom, host: NodeId, text: NodeId, css: bool) -> Option<bool> {
    use pyro_dom::stylo::declarations::DeclarationBlock;
    let span = dom.parent_element(text).ok()??;
    if span == host || !dom.element(span).is_ok_and(|e| e.is_html() && &**e.local_name() == "span") {
        return None;
    }
    let layout = page.layout();
    if FontToggle::Bold.active(&layout.computed_style_value(dom, span, "font-weight")) { return None; }
    let mut style = DeclarationBlock::parse(dom.get_attribute(span, "style").ok()?.unwrap_or(""));
    // A class rule with higher priority can defeat a normal inline bold
    // declaration. Reapplying the same declaration leaves that cascade intact.
    if css && dom.child_count(span).ok()? == 1 && dom.has_attribute(span, "class").ok()?
        && style.value("font-weight") == "bold" { return Some(true); }
    ancestor_named(dom, span, host, &["b", "strong"])?;
    match style.value("font-weight").as_str() {
        "normal" => {
            let parent = dom.parent_element(span).ok()??;
            let middle = alignment::split_edge(dom, parent, text, false, &mut [])?;
            alignment::split_edge(dom, parent, text, true, &mut [])?;
            style.remove("font-weight");
            if style.is_empty() {
                dom.remove_attribute(middle, "style").ok()?;
                if dom.attributes(middle).ok()?.is_empty() {
                    indentation::unwrap(dom, middle, &mut [(text, 0), (text, editing::len(dom, text))])?;
                }
            } else {
                dom.set_attribute(middle, "style", &style.css_text()).ok()?;
            }
            let current = dom.parent_element(text).ok()??;
            Some(FontToggle::Bold.active(&layout.computed_style_value(dom, current, "font-weight")))
        }
        "" => {
            // A reset from a rule still applies after removing the outer B.
            // If the selected text fills both elements, keep the reset span
            // and reapply bold inside it without retaining a redundant B.
            if let Some(outer) = dom.parent_element(span).ok().flatten().filter(|&outer|
                outer != host && matches!(editing::local(dom, outer).as_str(), "b" | "strong")
                && dom.element(outer).is_ok_and(|el| el.is_html())
                && dom.child_count(outer).ok() == Some(1) && dom.child_count(span).ok() == Some(1))
            {
                let mut points = [(text, 0), (text, editing::len(dom, text))];
                neutralize_bold(dom, outer, &mut points)?;
            }
            if css && dom.child_count(span).ok()? == 1 {
                style.set("font-weight", "bold", false)?;
                dom.set_attribute(span, "style", &style.css_text()).ok()?;
                return Some(true);
            }
            Some(false)
        }
        _ => None,
    }
}

/// Remove a conflicting numeric weight from the selected portion before
/// applying bold. Other attributes and declarations stay on their span.
fn prepare_numeric_bold(page: &PageDom, dom: &mut Dom, host: NodeId, text: NodeId, use_css: bool) -> Option<bool> {
    use pyro_dom::stylo::declarations::DeclarationBlock;
    let span = dom.parent_element(text).ok()??;
    if span == host || !dom.element(span).is_ok_and(|el| el.is_html() && &**el.local_name() == "span") {
        return None;
    }
    let mut style = DeclarationBlock::parse(dom.get_attribute(span, "style").ok()??);
    let weight = style.value("font-weight").parse::<f64>().ok()?;
    if !(1.0..600.0).contains(&weight) { return None; }
    // split_texts has already isolated the selected text. If there are
    // unselected siblings, keep their carrier and apply bold inside it.
    if dom.child_count(span).ok()? != 1 { return Some(false); }
    style.remove("font-weight");
    if style.is_empty() && dom.attributes(span).ok()?.len() == 1 {
        hoist(dom, text, span);
        return None;
    }
    if style.is_empty() {
        dom.remove_attribute(span, "style").ok()?;
    } else {
        dom.set_attribute(span, "style", &style.css_text()).ok()?;
    }
    if FontToggle::Bold.active(&page.layout().computed_style_value(dom, span, "font-weight")) {
        return Some(true);
    }
    if use_css {
        style.set("font-weight", "bold", false)?;
        dom.set_attribute(span, "style", &style.css_text()).ok()?;
    }
    Some(use_css)
}

/// The UA styles B as bolder. Under a very light or already-heavy
/// numeric weight that can produce 400 or 900 instead of the command's
/// bold weight. Isolate the affected portion only when that happens.
fn resolve_relative_bold(page: &PageDom, dom: &mut Dom, span: NodeId, bold: NodeId) -> Option<()> {
    use pyro_dom::stylo::declarations::DeclarationBlock;
    if !dom.element(span).is_ok_and(|el| el.is_html() && &**el.local_name() == "span") { return None; }
    let mut style = DeclarationBlock::parse(dom.get_attribute(span, "style").ok()??);
    style.value("font-weight").parse::<f64>().ok()?;
    let layout = page.layout();
    if layout.computed_style_value(dom, bold, "font-weight") == "700" { return Some(()); }
    style.remove("font-weight");
    if style.is_empty() && dom.attributes(span).ok()?.len() == 1 {
        hoist(dom, bold, span);
    } else {
        let outer = dom.parent_element(span).ok()??;
        let middle = alignment::split_edge(dom, outer, bold, false, &mut [])?;
        alignment::split_edge(dom, outer, bold, true, &mut [])?;
        if style.is_empty() { dom.remove_attribute(middle, "style").ok()?; }
        else { dom.set_attribute(middle, "style", &style.css_text()).ok()?; }
    }
    if layout.computed_style_value(dom, bold, "font-weight") != "700" {
        dom.set_attribute(bold, "style", "font-weight: bold;").ok()?;
    }
    Some(())
}

/// The old anchor's authored presentation belongs to its original text
/// run. Convert its declarations into independent formatting around that run.
fn link_style_shell(dom: &mut Dom, doc: NodeId, mut child: NodeId, style: &str) -> Option<NodeId> {
    use pyro_dom::stylo::declarations::DeclarationBlock;
    let mut css = DeclarationBlock::parse(style);
    if !dom.document_data(doc).ok()?.style_with_css {
        let color = pyro_dom::color_well::editing_color(&css.value("color")).and_then(|(_, hex)| hex);
        for (property, value, tag) in [("font-style", "italic", "i"), ("font-weight", "bold", "b")] {
            if css.value(property) == value {
                css.remove(property);
                let el = dom.create_element(doc, tag).ok()?;
                dom.append_child(el, child).ok()?;
                child = el;
            }
        }
        if let Some(hex) = color {
            css.remove("color");
            let font = dom.create_element(doc, "font").ok()?;
            dom.set_attribute(font, "color", &hex).ok()?;
            dom.append_child(font, child).ok()?;
            child = font;
        }
    }
    if !css.is_empty() {
        let span = dom.create_element(doc, "span").ok()?;
        dom.set_attribute(span, "style", &css.css_text()).ok()?;
        dom.append_child(span, child).ok()?;
        child = span;
    }
    Some(child)
}

/// Strip link semantics from the selected fragment, preserving the anchor's
/// authored presentation around its original children.
fn unlink_styled_fragment(dom: &mut Dom, anchor: NodeId, text: NodeId, whole: bool, style: &str) -> Option<()> {
    let parent = dom.parent_element(anchor).ok()??;
    let selected = if whole { anchor } else {
        let selected = alignment::split_edge(dom, parent, text, false, &mut [])?;
        let following = dom.next_sibling(selected).ok()?;
        alignment::split_edge(dom, parent, text, true, &mut [])?;
        // split_edge keeps the ID on the first fragment. When that fragment
        // is the selected prefix, move the ID to the newly created suffix.
        if selected == anchor {
            if let Some(suffix) = dom.next_sibling(selected).ok()?.filter(|&n| Some(n) != following) {
                if let Some(id) = dom.get_attribute(selected, "id").ok()?.map(str::to_owned) {
                    dom.remove_attribute(selected, "id").ok()?;
                    dom.set_attribute(suffix, "id", &id).ok()?;
                }
            }
        }
        selected
    };
    let doc = dom.node_document(selected).ok()?;
    // Keep an existing inline child at the same depth when the link only
    // contributed inherited font/color declarations. Its own declarations
    // take precedence over inherited values, including their priorities.
    if dom.child_count(selected).ok()? == 1 {
        let child = dom.first_child(selected).ok()??;
        if dom.element(child).is_ok_and(|el| el.is_html() && matches!(&**el.local_name(), "span" | "i" | "em" | "b" | "strong" | "u"))
            && rich_node(dom, child) && !dom.has_attribute(child, "class").ok()?
        {
            use pyro_dom::stylo::declarations::DeclarationBlock;
            let mut inherited = DeclarationBlock::parse(style);
            if inherited.names().iter().all(|p| matches!(p.as_str(), "font-weight" | "font-style" | "color")) {
                let own = DeclarationBlock::parse(dom.get_attribute(child, "style").ok()?.unwrap_or(""));
                for name in own.names() {
                    inherited.set(&name, &own.value(&name), own.important(&name))?;
                }
                dom.set_attribute(child, "style", &inherited.css_text()).ok()?;
                dom.insert_before(parent, child, Some(selected)).ok()?;
                dom.remove_child(parent, selected).ok()?;
                return Some(());
            }
        }
    }
    let contents = dom.create_fragment(doc).ok()?;
    while let Some(child) = dom.first_child(selected).ok()? {
        dom.append_child(contents, child).ok()?;
    }
    let shell = link_style_shell(dom, doc, contents, style)?;
    dom.insert_before(parent, shell, Some(selected)).ok()?;
    dom.remove_child(parent, selected).ok()?;
    Some(())
}

/// Replace the selected part of an existing link. The original anchor
/// retains its suffix, while an unselected prefix keeps the ID.
fn relink_text(dom: &mut Dom, host: NodeId, text: NodeId, href: &str, entire_run: Option<NodeId>) -> Option<NodeId> {
    let anchor = ancestor_named(dom, text, host, &["a"])?;
    let parent = dom.parent_element(anchor).ok()??;
    let branch = alignment::split_edge(dom, anchor, text, false, &mut [])?;
    alignment::split_edge(dom, anchor, text, true, &mut [])?;
    let doc = dom.node_document(anchor).ok()?;
    let replacement = dom.create_element(doc, "a").ok()?;
    dom.set_attribute(replacement, "href", href).ok()?;
    let style = dom.get_attribute(anchor, "style").ok()?.map(str::to_owned);
    if dom.prev_sibling(branch).ok()?.is_some() {
        let prefix = dom.clone_node(anchor, false, None).ok()?;
        dom.insert_before(parent, prefix, Some(anchor)).ok()?;
        while let Some(child) = dom.first_child(anchor).ok()?.filter(|&n| n != branch) {
            dom.append_child(prefix, child).ok()?;
        }
    }
    dom.remove_attribute(anchor, "id").ok()?;
    let outside = if let Some(style) = style.as_deref().filter(|_| entire_run == Some(anchor)) {
        // A selection confined to this link inherits the old presentation
        // outside the new link. Its own link color can therefore take effect.
        dom.append_child(replacement, branch).ok()?;
        link_style_shell(dom, doc, replacement, style)?
    } else {
        let content = match style.as_deref() {
            Some(style) => link_style_shell(dom, doc, branch, style)?,
            None => branch,
        };
        dom.append_child(replacement, content).ok()?;
        replacement
    };
    dom.insert_before(parent, outside, Some(anchor)).ok()?;
    if dom.child_count(anchor).ok()? == 0 { dom.remove_node(anchor).ok()?; }
    Some(replacement)
}

/// Move the left link's children into a matching right link. Keep their
/// text nodes intact so the selection can still address its original text.
fn join_plain_links(dom: &mut Dom, left: NodeId, right: NodeId, href: &str) {
    let compatible = |n| dom.element(n).is_ok_and(|el| el.is_html() && &**el.local_name() == "a")
        && dom.attributes(n).is_ok_and(|attrs| attrs.len() == 1)
        && dom.get_attribute(n, "href").ok().flatten() == Some(href);
    if compatible(left) && compatible(right) { merge_inline_siblings(dom, left, right); }
}

/// Retain the right wrapper and all child nodes, including text selection
/// endpoints, as adjacent equivalent formatting elements become one.
fn merge_inline_siblings(dom: &mut Dom, left: NodeId, right: NodeId) {
    while let Ok(Some(child)) = dom.last_child(left) {
        let first = dom.first_child(right).ok().flatten();
        if dom.insert_before(right, child, first).is_err() { return; }
    }
    let _ = dom.remove_node(left);
}

/// A mixed selection beginning in an italic element toggles from the
/// first selected run in document order, including for a backward range.
fn starts_in_semantic_italic(page: &PageDom, start: Point, end: Point) -> bool {
    let dom = page.dom.borrow();
    let first = editing::deep_forward(&dom, start);
    if !editing::is_text(&dom, first.0) || first.1 == editing::len(&dom, first.0) { return false; }
    let Some(parent) = dom.parent_element(first.0).ok().flatten() else { return false };
    dom.element(parent).is_ok_and(|el| el.is_html() && matches!(&**el.local_name(), "address" | "cite" | "dfn" | "em" | "i" | "var"))
        && !dom.contains(parent, editing::deep_backward(&dom, end).0).unwrap_or(false)
        && FontToggle::Italic.active(&page.layout().computed_style_value(&dom, parent, "font-style"))
}

fn semantic_italic_tag(dom: &Dom, el: NodeId) -> bool {
    dom.element(el).is_ok_and(|e| e.is_html() && matches!(&**e.local_name(), "cite" | "dfn" | "var"))
}

/// Choose the unit that should share an italic wrapper with adjacent
/// selected text. A semantic element keeps its meaning; I and EM already
/// express the formatting that the new wrapper supplies.
fn mixed_italic_unit(page: &PageDom, dom: &mut Dom, host: NodeId, text: NodeId, wrapper: Option<NodeId>, next: Option<NodeId>) -> Option<NodeId> {
    use pyro_dom::stylo::declarations::DeclarationBlock;
    let parent = dom.parent_element(text).ok()??;
    let name = editing::local(dom, parent);
    if parent == host || !dom.element(parent).ok()?.is_html()
        || !matches!(name.as_str(), "address" | "cite" | "dfn" | "var" | "em" | "i") { return Some(text); }
    let whole = dom.child_count(parent).ok()? == 1;
    if whole {
        if let Some(style) = dom.get_attribute(parent, "style").ok()? {
            let mut declarations = DeclarationBlock::parse(style);
            if declarations.value("font-style") == "normal" && declarations.remove("font-style") {
                dom.set_attribute(parent, "style", &declarations.css_text()).ok()?;
            }
        }
    }
    let layout = page.layout();
    if !FontToggle::Italic.active(&layout.computed_style_value(dom, parent, "font-style")) { return Some(text); }
    if !whole || layout.computed_style_value(dom, parent, "display") != "inline" { return None; }
    let follows_wrapper = wrapper.is_some() && dom.prev_sibling(parent).ok()? == wrapper;
    if !follows_wrapper && !(next.is_some() && dom.next_sibling(parent).ok()? == next) { return None; }
    if matches!(name.as_str(), "em" | "i") {
        let mut points = [(text, 0), (text, editing::len(dom, text))];
        if dom.attributes(parent).ok()?.is_empty() {
            indentation::unwrap(dom, parent, &mut points)?;
            return Some(text);
        }
        return indentation::rename(dom, parent, "span", &mut points);
    }
    Some(parent)
}

/// Wraps the selected text in `tag` (with `href` for links), skipping text
/// that already has one of `carriers` around it.
fn format_inline(page: &Rc<PageDom>, host: NodeId, tag: &str, carriers: &[&str], input_type: &str, href: Option<&str>) {
    let (start, end) = editing::host_range(page, host);
    let reversed = start != end && editing::selection(page).is_some_and(|(anchor, _)| anchor == end);
    let collapsed = start == end;
    let selected = {
        let dom = page.dom.borrow();
        let mut texts = inline_texts(page, &dom, host, start, end);
        texts.retain(|&text| accepts_html_formatting(&dom, text));
        texts
    };
    if !collapsed && selected.is_empty() {
        return;
    }
    editing::apply_edit(page, host, input_type, href, None, |page| {
        let mut wrapped_svg = false;
        let texts = {
            let mut dom = page.dom.borrow_mut();
            let texts = if collapsed {
                // createLink at a caret inserts the URL as the link's text.
                let p = editing::insert_at(&mut dom, start, href.unwrap_or(""));
                if p.0 == start.0 && p.1 > start.1 {
                    let inserted = inline_texts(page, &dom, host, start, p);
                    split_texts(&mut dom, inserted, start, p)
                } else {
                    vec![p.0]
                }
            } else {
                split_texts(&mut dom, selected, start, end)
            };
            let texts = include_paragraph_breaks(page, &dom, host, texts);
            let Ok(doc) = dom.node_document(host) else {
                return;
            };
            let mut last_wrapper: Option<NodeId> = None;
            let use_css = dom.document_data(doc).is_ok_and(|d| d.style_with_css);
            let single_link = href.and_then(|_| {
                let link = ancestor_named(&dom, *texts.first()?, host, &["a"])?;
                dom.contains(link, *texts.last()?).ok()?.then_some(link)
            });
            for (index, &t) in texts.iter().enumerate() {
                if let Some(value) = href.filter(|_| !collapsed) {
                    if let Some(link) = relink_text(&mut dom, host, t, value, single_link) {
                        last_wrapper = Some(link);
                        continue;
                    }
                }
                let mixed_italic = tag == "i" && texts.len() > 1;
                let unit = if mixed_italic {
                    let Some(unit) = mixed_italic_unit(page, &mut dom, host, t, last_wrapper, texts.get(index + 1).copied()) else { continue };
                    unit
                } else { t };
                let prepared = if tag == "b" {
                    prepare_overridden_bold(page, &mut dom, host, t, use_css)
                        .or_else(|| prepare_numeric_bold(page, &mut dom, host, t, use_css))
                } else { None };
                let carried = prepared.unwrap_or_else(|| carriers.iter().any(|c| ancestor_named(&dom, t, host, &[c]).is_some()));
                if carried && (!mixed_italic || dom.parent_element(t).ok().flatten().is_some_and(|el|
                    FontToggle::Italic.active(&page.layout().computed_style_value(&dom, el, "font-style")))) {
                    continue;
                }
                let Ok(Some(parent)) = dom.parent(unit) else {
                    continue;
                };
                if let Some(w) = last_wrapper.filter(|&w| dom.parent(w).ok().flatten() == Some(parent)) {
                    // A fully selected plaintext island may inherit formatting
                    // from an outer wrapper. Do not insert markup inside it.
                    let mut next = dom.next_sibling(w).ok().flatten();
                    let mut islands = Vec::new();
                    while let Some(el) = next {
                        if el == unit || !dom.get_attribute(el, "contenteditable").ok().flatten()
                            .is_some_and(|mode| mode.eq_ignore_ascii_case("plaintext-only"))
                        {
                            break;
                        }
                        islands.push(el);
                        next = dom.next_sibling(el).ok().flatten();
                    }
                    if next == Some(unit) {
                        for island in islands {
                            let _ = dom.append_child(w, island);
                        }
                        let _ = dom.append_child(w, unit);
                        continue;
                    }
                }
                let style = if use_css { inline_css(tag) } else { None };
                let Ok(w) = dom.create_element(doc, if style.is_some() { "span" } else { tag }) else {
                    continue;
                };
                if let Some((property, value)) = style {
                    let _ = dom.set_attribute(w, "style", &format!("{property}: {value};"));
                }
                if let Some(href) = href {
                    let _ = dom.set_attribute(w, "href", href);
                }
                wrapped_svg |= dom.element(parent).is_ok_and(|el| el.namespace() == &markup5ever::ns!(svg));
                let _ = dom.insert_before(parent, w, Some(unit));
                let _ = dom.append_child(w, unit);
                if tag == "b" && !use_css { resolve_relative_bold(page, &mut dom, parent, w); }
                last_wrapper = Some(w);
            }
            if tag == "b" && !use_css && texts.len() == 1 {
                let plain_bold = |dom: &Dom, el| local_html(dom, el, "b") && dom.attributes(el).is_ok_and(|attrs| attrs.is_empty());
                if let Some(wrapper) = last_wrapper.filter(|&w| plain_bold(&dom, w)) {
                    if let Some(left) = dom.prev_sibling(wrapper).ok().flatten().filter(|&el| plain_bold(&dom, el)) {
                        merge_inline_siblings(&mut dom, left, wrapper);
                    }
                    if let Some(right) = dom.next_sibling(wrapper).ok().flatten().filter(|&el| plain_bold(&dom, el)) {
                        merge_inline_siblings(&mut dom, wrapper, right);
                    }
                }
            }
            if let Some(value) = href.filter(|_| !collapsed) {
                for &t in &texts {
                    let Some(link) = ancestor_named(&dom, t, host, &["a"]) else { continue };
                    if let Ok(Some(left)) = dom.prev_sibling(link) { join_plain_links(&mut dom, left, link, value); }
                    if let Ok(Some(right)) = dom.next_sibling(link) { join_plain_links(&mut dom, link, right, value); }
                }
            }
            texts
        };
        if wrapped_svg {
            let (Some(&first), Some(&last)) = (texts.first(), texts.last()) else { return };
            let (start, end) = {
                let dom = page.dom.borrow();
                (restored_format_point(page, &dom, host, (first, 0)),
                 restored_format_point(page, &dom, host, (last, editing::len(&dom, last))))
            };
            if start == end && !collapsed { editing::set_adjusted_caret(page, start, reversed); }
            else if reversed { editing::set_selection(page, end, start); }
            else { editing::set_selection(page, start, end); }
        } else {
            select_texts(page, &texts, reversed);
        }
    });
}

fn ancestor_named(dom: &Dom, node: NodeId, host: NodeId, names: &[&str]) -> Option<NodeId> {
    ancestor_format(dom, node, host, names, false)
}

fn numeric_bold_weight(value: &str) -> bool {
    value.parse::<f64>().is_ok_and(|weight| (600.0..=1000.0).contains(&weight))
}

fn ancestor_format(dom: &Dom, node: NodeId, host: NodeId, names: &[&str], removing: bool) -> Option<NodeId> {
    let mut cur = dom.parent_element(node).ok().flatten();
    while let Some(c) = cur {
        if c == host {
            return None;
        }
        let el = dom.element(c).ok()?;
        if !el.is_html() {
            cur = dom.parent_element(c).ok().flatten();
            continue;
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
                                || removing && property == "font-weight" && numeric_bold_weight(&declaration.value)
                                || removing && property == "font-style" && FontToggle::Italic.active(&declaration.value))
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
        if dom.prev_sibling(cur).ok().flatten().is_none() && dom.next_sibling(cur).ok().flatten().is_some() {
            // Keep the original right-hand wrapper (including listeners).
            // The ID belongs to the first fragment, not to both fragments.
            let moved = if p == anc {
                cur
            } else {
                let Ok(prefix) = dom.clone_node(p, false, None) else { return };
                if dom.append_child(prefix, cur).is_err() { return; }
                prefix
            };
            if dom.insert_before(gp, moved, Some(p)).is_err() { return; }
            let _ = dom.remove_attribute(p, "id");
            if p == anc { return; }
            cur = moved;
            continue;
        }
        let Ok(after) = dom.clone_node(p, false, None) else { return };
        let _ = dom.remove_attribute(after, "id");
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
            let _ = dom.remove_attribute(mid, "id");
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

#[derive(Clone, Copy)]
enum FontToggle {
    Bold,
    Italic,
}

impl FontToggle {
    fn property(self) -> &'static str {
        match self { Self::Bold => "font-weight", Self::Italic => "font-style" }
    }

    fn tag(self) -> &'static str {
        match self { Self::Bold => "b", Self::Italic => "i" }
    }

    fn active(self, value: &str) -> bool {
        match self {
            Self::Bold => value.eq_ignore_ascii_case("bold") || numeric_bold_weight(value),
            Self::Italic => value.eq_ignore_ascii_case("italic") || value.starts_with("oblique"),
        }
    }

    fn legacy_reproduces(self, authored: &str, inherited: &str) -> bool {
        match self {
            Self::Bold => matches!(authored, "bold" | "700") && inherited == "400",
            Self::Italic => authored == "italic" && inherited == "normal",
        }
    }

    fn transparent_inline(self, tag: &str) -> bool {
        matches!(tag, "span" | "u" | "s") || match self {
            Self::Bold => matches!(tag, "i" | "em"),
            Self::Italic => matches!(tag, "b" | "strong"),
        }
    }

    fn can_override_inline(self, tag: &str) -> bool {
        matches!(tag, "span" | "i" | "em" | "u" | "s" | "strong" | "b")
            || matches!(self, Self::Italic) && matches!(tag, "cite" | "var" | "dfn")
    }
}

/// Remove an explicitly authored paragraph font style while keeping its original
/// value and priority on the unselected text. More complex paragraphs can
/// retain the block declaration and override just the selected inline run.
fn distribute_block_font(page: &PageDom, dom: &mut Dom, font: FontToggle, block: NodeId, container: NodeId, first: Point, last: Point) -> Option<()> {
    use pyro_dom::stylo::declarations::DeclarationBlock;
    let mut style = DeclarationBlock::parse(dom.get_attribute(block, "style").ok()??);
    let property = font.property();
    let authored = style.value(property);
    if !font.active(&authored) { return None; }
    let important = style.important(property);
    // A chain of ordinary inline elements can carry the paragraph's font style
    // to its text. Leave those elements in place and preserve the weight on
    // either side inside the same innermost element.
    let mut inline = container;
    while inline != block {
        let parent = dom.parent_element(inline).ok()??;
        if dom.child_count(parent).ok()? != 1
            || !font.transparent_inline(&editing::local(dom, inline))
            || page.layout().computed_style_value(dom, inline, "display") != "inline"
            || !DeclarationBlock::parse(dom.get_attribute(inline, "style").ok()?.unwrap_or("")).value(property).is_empty()
            || dom.has_attribute(inline, "class").ok()?
        {
            return None;
        }
        inline = parent;
    }
    let children: Vec<_> = dom.children(container).ok()?.collect();
    if !children.iter().all(|&n| editing::is_text(dom, n)) { return None; }
    style.remove(property);
    if style.is_empty() { dom.remove_attribute(block, "style").ok()?; }
    else { dom.set_attribute(block, "style", &style.css_text()).ok()?; }
    let doc = dom.node_document(block).ok()?;
    let css = dom.document_data(doc).ok()?.style_with_css;
    let inherited = page.layout().computed_style_value(dom, container, property);
    for child in children {
        let before = crate::range::compare_boundary_points_dom(dom, (child, editing::len(dom, child)), first) <= 0;
        let after = crate::range::compare_boundary_points_dom(dom, last, (child, 0)) <= 0;
        if !before && !after { continue; }
        // The trailing run can use the legacy bold tag when it reproduces
        // the authored weight. Numeric weights such as 600 or 900 need CSS.
        let legacy = (after || container != block) && !css && !important && font.legacy_reproduces(&authored, &inherited);
        let span = dom.create_element(doc, if legacy { font.tag() } else { "span" }).ok()?;
        if !legacy {
            let mut retained = DeclarationBlock::parse("");
            retained.set(property, &authored, important)?;
            dom.set_attribute(span, "style", &retained.css_text()).ok()?;
        }
        dom.insert_before(container, span, Some(child)).ok()?;
        dom.append_child(span, child).ok()?;
    }
    Some(())
}

/// Keep the paragraph and unrelated styling when the selected text is still
/// styled after removable formatting tags have been taken out.
fn clear_remaining_font(page: &PageDom, dom: &mut Dom, font: FontToggle, host: NodeId, text: NodeId, first: Point, last: Point) -> Option<()> {
    use pyro_dom::stylo::declarations::DeclarationBlock;
    let property = font.property();
    let parent = dom.parent_element(text).ok()??;
    let layout = page.layout();
    if !font.active(&layout.computed_style_value(dom, parent, property)) {
        return Some(());
    }

    let block = editing::block_of(dom, text, host);
    if block != host {
        distribute_block_font(page, dom, font, block, parent, first, last);
        if !font.active(&layout.computed_style_value(dom, parent, property)) {
            return Some(());
        }
    }
    let mut parent = parent;
    if parent != host && editing::local(dom, parent) == "span" {
        let mut declarations = DeclarationBlock::parse(dom.get_attribute(parent, "style").ok()?.unwrap_or(""));
        if declarations.remove(property) {
            let outer = dom.parent_element(parent).ok()??;
            let mut edges = [(text, 0), (text, editing::len(dom, text))];
            let middle = alignment::split_edge(dom, outer, text, false, &mut edges)?;
            alignment::split_edge(dom, outer, text, true, &mut edges)?;
            if declarations.is_empty() {
                dom.remove_attribute(middle, "style").ok()?;
            } else {
                dom.set_attribute(middle, "style", &declarations.css_text()).ok()?;
            }
            parent = middle;
            if !font.active(&layout.computed_style_value(dom, parent, property)) {
                return Some(());
            }
        }
    }

    let carrier = if parent != host && dom.child_count(parent).ok()? == 1
        && layout.computed_style_value(dom, parent, "display") == "inline"
        && font.can_override_inline(&editing::local(dom, parent))
    {
        parent
    } else {
        let doc = dom.node_document(text).ok()?;
        let span = dom.create_element(doc, "span").ok()?;
        dom.insert_before(parent, span, Some(text)).ok()?;
        dom.append_child(span, text).ok()?;
        span
    };
    let mut style = DeclarationBlock::parse(dom.get_attribute(carrier, "style").ok()?.unwrap_or(""));
    style.set(property, "normal", false)?;
    dom.set_attribute(carrier, "style", &style.css_text()).ok()?;
    Some(())
}

/// Takes the selected text out of the elements named `names` (unlink,
/// removeFormat, toggling bold off).
fn remove_inline(page: &Rc<PageDom>, host: NodeId, names: &[&str], input_type: &str) {
    let (start, end) = editing::host_range(page, host);
    let reversed = start != end && editing::selection(page).is_some_and(|(anchor, _)| anchor == end);
    if start == end && !names.contains(&"a") {
        return;
    }
    let unlink_range = start != end && names == ["a"];
    let (start, end) = if start == end {
        let dom = page.dom.borrow();
        let Some(a) = ancestor_named(&dom, start.0, host, &["a"]) else { return };
        (editing::host_start(&dom, a), editing::host_end(&dom, a))
    } else {
        (start, end)
    };
    let selected = {
        let dom = page.dom.borrow();
        let mut texts = texts_in(&dom, host, start, end);
        texts.retain(|&node| {
            if !editing::is_editable(&dom, node) || !accepts_html_formatting(&dom, node) { return false; }
            // Clearing formatting across a fully enclosed plaintext island
            // can remove an existing carrier inside it. Keep the island itself.
            let Some(island) = editing::plaintext_root(&dom, node) else { return true };
            let (Ok(Some(parent)), Ok(index)) = (dom.parent(island), dom.index_in_parent(island)) else { return false };
            crate::range::compare_boundary_points_dom(&dom, start, (parent, index)) <= 0
                && crate::range::compare_boundary_points_dom(&dom, (parent, index + 1), end) <= 0
        });
        texts
    };
    if selected.is_empty() {
        return;
    }
    editing::apply_edit(page, host, input_type, None, None, |page| {
        let texts = {
            let mut dom = page.dom.borrow_mut();
            let texts = split_texts(&mut dom, selected, start, end);
            let texts = if names.is_empty() || input_type == "formatRemove" {
                texts
            } else {
                include_paragraph_breaks(page, &dom, host, texts)
            };
            let (Some(&first), Some(&last)) = (texts.first(), texts.last()) else { return };
            let first = (first, 0);
            let last = (last, editing::len(&dom, last));
            let mut leading_unit = None;
            if input_type == "formatItalic" && texts.len() > 1 {
                if let Some(el) = dom.parent_element(first.0).ok().flatten().filter(|&el| semantic_italic_tag(&dom, el)
                    && dom.child_count(el).ok() == Some(1) && dom.next_sibling(el).ok().flatten() == texts.get(1).copied()
                    && page.layout().computed_style_value(&dom, el, "display") == "inline") {
                    if let (Some(parent), Ok(doc)) = (dom.parent_element(el).ok().flatten(), dom.node_document(el)) {
                        // Keep the intrinsic style of a fully selected semantic
                        // element; apply normal to the containing run.
                        let already_normal = editing::local(&dom, parent) == "span" && dom.get_attribute(parent, "style")
                            .ok().flatten().is_some_and(|s| pyro_dom::stylo::declarations::DeclarationBlock::parse(s)
                                .value("font-style") == "normal");
                        if !already_normal {
                            if let Ok(span) = dom.create_element(doc, "span") {
                                let _ = dom.set_attribute(span, "style", "font-style: normal;");
                                let _ = dom.insert_before(parent, span, Some(el));
                                let _ = dom.append_child(span, el);
                                for &next in texts.iter().skip(1) {
                                    if dom.next_sibling(span).ok().flatten() != Some(next) { break; }
                                    let _ = dom.append_child(span, next);
                                }
                                leading_unit = Some(el);
                            }
                        }
                    }
                }
            }
            for &t in &texts {
                let Some(owner) = editing::plaintext_root(&dom, t).or_else(|| editing::editing_host(&dom, t)) else { continue };
                if input_type == "formatBold" && texts.len() == 1
                    && remove_reset_child_bold(page, &mut dom, owner, t).is_some()
                { continue; }
                let inner_bold = input_type == "formatBold" && dom.parent_element(t).ok().flatten()
                    .is_some_and(|el| matches!(editing::local(&dom, el).as_str(), "b" | "strong"));
                while let Some(mut a) = ancestor_format(&dom, t, owner, names, true) {
                    // Remove a completely selected carrier as a unit. Hoisting
                    // only its editable text would leave a formatting wrapper
                    // around its locked children or clone their ancestors.
                    let begins = editing::deep_forward(&dom, (a, 0));
                    let ends = editing::deep_backward(&dom, (a, editing::len(&dom, a)));
                    let mut whole = crate::range::compare_boundary_points_dom(&dom, first, begins) <= 0
                        && crate::range::compare_boundary_points_dom(&dom, ends, last) <= 0;
                    if !whole && t == first.0 && first.0 != last.0 && dom.contains(a, last.0).unwrap_or(false)
                        && editing::editing_host(&dom, last.0) == Some(owner)
                    {
                        // Isolate the whole selected middle before unwrapping
                        // it. Hoisting the two text edges independently leaves
                        // a styled clone around any locked node between them.
                        let Some(parent) = dom.parent(a).ok().flatten() else { break };
                        let mut points = [first, last];
                        let Some(middle) = alignment::split_edge(&mut dom, parent, first.0, false, &mut points) else { break };
                        if alignment::split_edge(&mut dom, parent, last.0, true, &mut points).is_none() { break; }
                        a = middle;
                        whole = true;
                    }
                    if unlink_range {
                        if let Some(style) = dom.get_attribute(a, "style").ok().flatten().filter(|s| !s.is_empty()).map(str::to_owned) {
                            if unlink_styled_fragment(&mut dom, a, t, whole, &style).is_none() { break; }
                            continue;
                        }
                    }
                    // Toggling strike removes the presentation but preserves
                    // independent attributes on the selected fragment.
                    let keep_attrs = input_type == "formatStrikeThrough"
                        && matches!(editing::local(&dom, a).as_str(), "s" | "strike")
                        && dom.attributes(a).is_ok_and(|attrs| !attrs.is_empty());
                    if whole && keep_attrs {
                        let mut points = [first, last];
                        if indentation::rename(&mut dom, a, "span", &mut points).is_none() { break; }
                    } else if whole {
                        let Some(parent) = dom.parent(a).ok().flatten() else { break };
                        while let Ok(Some(child)) = dom.first_child(a) {
                            if dom.insert_before(parent, child, Some(a)).is_err() { break; }
                        }
                        if dom.child_count(a).ok() == Some(0) {
                            let _ = dom.remove_child(parent, a);
                        }
                    } else {
                        let neutral = if keep_attrs {
                            let Ok(doc) = dom.node_document(a) else { break };
                            let Ok(span) = dom.create_element(doc, "span") else { break };
                            let at_start = editing::deep_forward(&dom, (a, 0)) == (t, 0);
                            for attr in dom.attributes(a).unwrap_or(&[]).to_vec() {
                                if attr.name.ns.is_empty() && attr.name.local == markup5ever::local_name!("id") && !at_start { continue; }
                                let ns = (!attr.name.ns.is_empty()).then_some(&*attr.name.ns);
                                let _ = dom.set_attribute_ns(span, ns, &attr.qualified_name(), &attr.value);
                            }
                            dom.attributes(span).is_ok_and(|attrs| !attrs.is_empty()).then_some(span)
                        } else { None };
                        hoist(&mut dom, t, a);
                        if let Some(span) = neutral {
                            if let Ok(Some(parent)) = dom.parent(t) {
                                let _ = dom.insert_before(parent, span, Some(t));
                                let _ = dom.append_child(span, t);
                            }
                        }
                    }
                    if ancestor_format(&dom, t, owner, names, true) == Some(a) {
                        break;
                    }
                }
                if inner_bold {
                    // With the bold child gone, a fully selected normal-weight
                    // wrapper is redundant when its parent already supplies 400.
                    if let Some(span) = dom.parent_element(t).ok().flatten().filter(|&span| span != owner
                        && editing::local(&dom, span) == "span" && dom.child_count(span).ok() == Some(1)
                        && dom.attributes(span).is_ok_and(|attrs| attrs.len() == 1)
                        && dom.get_attribute(span, "style").ok().flatten().is_some_and(|style| {
                            let declarations = crate::css::parse_declarations(style);
                            matches!(declarations.as_slice(), [d] if d.name == "font-weight" && d.value == "normal")
                        })
                        && dom.parent_element(span).ok().flatten().is_some_and(|outer|
                            page.layout().computed_style_value(&dom, outer, "font-weight") == "400"))
                    {
                        indentation::unwrap(&mut dom, span, &mut [first, last]);
                    }
                }
                let font = match input_type {
                    "formatBold" => Some(FontToggle::Bold),
                    "formatItalic" => Some(FontToggle::Italic),
                    _ => None,
                };
                if let Some(font) = font.filter(|_| rich_node(&dom, t)) {
                    if input_type == "formatItalic" && texts.len() > 1 {
                        if let Some(el) = dom.parent_element(t).ok().flatten().filter(|&el| semantic_italic_tag(&dom, el)
                            && dom.child_count(el).ok() == Some(1) && page.layout().computed_style_value(&dom, el, "display") == "inline") {
                            if leading_unit != Some(el) {
                                let mut style = pyro_dom::stylo::declarations::DeclarationBlock::parse(dom.get_attribute(el, "style").ok().flatten().unwrap_or(""));
                                style.remove("font-style");
                                let _ = dom.set_attribute(el, "style", &style.css_text());
                            }
                            continue;
                        }
                    }
                    clear_remaining_font(page, &mut dom, font, owner, t, first, last);
                }
            }
            texts
        };
        select_texts(page, &texts, reversed);
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

/// Resolve an element boundary to the text or leaf under its caret. Do not
/// walk outside the editing host when a boundary lies directly in the host.
fn format_block_endpoint(dom: &Dom, point: Point, host: NodeId) -> NodeId {
    if editing::is_text(dom, point.0) {
        return point.0;
    }
    // Empty paragraphs and whitespace-only blocks have no rendered text
    // position. An explicit BR, in contrast, supplies a caret position.
    let usable = |n| {
        if editing::editing_host(dom, n) != Some(host) { return false; }
        if editing::is_text(dom, n) {
            return dom.char_data(n).is_ok_and(|s| !s.is_empty());
        }
        !editing::is_container(dom, n) || dom.child_count(n).ok() != Some(0)
    };
    let count = editing::len(dom, point.0);
    for index in point.1..count {
        let next = editing::deep_forward(dom, (point.0, index));
        if next.0 == point.0 || editing::editing_host(dom, next.0) != Some(host) { break; }
        if usable(next.0) { return next.0; }
    }
    let previous = editing::deep_backward(dom, point);
    if usable(previous.0) { previous.0 } else { point.0 }
}

/// Normalize a structural query boundary past leaves with no visible caret
/// position. Keep the formatting operation's line selection independent.
fn format_query_endpoint(page: &PageDom, dom: &Dom, point: Point, host: NodeId) -> NodeId {
    let original = format_block_endpoint(dom, point, host);
    if editing::is_text(dom, point.0) { return original; }
    let rendered = |n| {
        editing::editing_host(dom, n) == Some(host) && alignment::visible(page, dom, n)
            && if editing::is_text(dom, n) {
                editing::len(dom, n) != 0 && !editing::invisible_formatting_space(page, dom, host, n)
            } else {
                local_html(dom, n, "br") || local_html(dom, n, "img") || local_html(dom, n, "hr")
            }
    };
    if rendered(original) { return original; }
    let mut previous = None;
    for leaf in dom.descendants(point.0).into_iter().flatten().filter(|&n| rendered(n)) {
        if crate::range::compare_boundary_points_dom(dom, point, (leaf, 0)) <= 0 { return leaf; }
        previous = Some(leaf);
    }
    previous.unwrap_or(point.0)
}

fn format_block_value(page: &PageDom) -> Option<String> {
    let (start, end) = editing::selection_range(page)?;
    let dom = page.dom.borrow();
    let host = editing::editing_host(&dom, start.0)?;
    if editing::editing_host(&dom, end.0) != Some(host) {
        return None;
    }
    let (a, b) = if start == end {
        let node = format_block_endpoint(&dom, start, host);
        (node, node)
    } else {
        (format_query_endpoint(page, &dom, start, host), format_query_endpoint(page, &dom, end, host))
    };
    let mut node = crate::range::common_ancestor(&dom, a, b).ok()?;
    while node != host {
        let tag = editing::local(&dom, node);
        if FORMAT_BLOCKS.contains(&tag.as_str()) && dom.element(node).is_ok_and(|el| el.is_html()) {
            return Some(tag);
        }
        node = dom.parent(node).ok()??;
    }
    None
}

/// A paragraph may be an explicit block or a contiguous run of inline
/// children of the host. Capture its boundaries before moving any nodes.
#[derive(PartialEq)]
enum FormatLine {
    Block(NodeId),
    Inline(Vec<NodeId>),
}

fn format_lines(page: &PageDom, dom: &Dom, host: NodeId, start: Point, end: Point) -> Vec<FormatLine> {
    let mut nodes = texts_in(dom, host, start, end);
    if nodes.is_empty() {
        nodes.push(format_block_endpoint(dom, start, host));
    }
    let mut lines = Vec::new();
    for node in nodes {
        if !rich_node(dom, node) { continue; }
        let mut child = node;
        let block = loop {
            if child == host { break None; }
            if dom.element(child).is_ok() {
                let display = page.layout().computed_style_value(dom, child, "display");
                if matches!(display.as_str(), "block" | "list-item" | "table-cell") { break Some(child); }
            }
            let Some(parent) = dom.parent(child).ok().flatten() else { break None };
            if parent == host { break None; }
            child = parent;
        };
        let line = if let Some(block) = block {
            FormatLine::Block(block)
        } else {
            if matches!(lines.last(), Some(FormatLine::Inline(kids)) if kids.contains(&child)) { continue; }
            let inline = |n| {
                if editing::is_text(dom, n) { return true; }
                let display = page.layout().computed_style_value(dom, n, "display");
                display == "inline" || display.starts_with("inline-") || display == "contents"
            };
            let mut first = child;
            while let Some(prev) = dom.prev_sibling(first).ok().flatten().filter(|&n| inline(n)) { first = prev; }
            let mut kids = Vec::new();
            if child != host {
                let mut next = Some(first);
                while let Some(n) = next {
                    kids.push(n);
                    next = dom.next_sibling(n).ok().flatten().filter(|&n| inline(n));
                }
            }
            FormatLine::Inline(kids)
        };
        if lines.last() != Some(&line) { lines.push(line); }
    }
    lines
}

/// formatBlock: the selected blocks become `tag` elements (content of a
/// host without blocks is wrapped in one).
/// The start of a whole-table selection can share a visible caret position
/// with the inline sibling immediately before the newly formatted block.
fn before_formatted_table(page: &PageDom, dom: &Dom, wrapper: NodeId, host: NodeId) -> Option<Point> {
    let mut previous = dom.prev_sibling(wrapper).ok()?;
    while let Some(node) = previous {
        previous = dom.prev_sibling(node).ok()?;
        if !alignment::visible(page, dom, node) { continue; }
        if editing::is_text(dom, node) {
            if editing::len(dom, node) == 0 { continue; }
            if editing::invisible_formatting_space(page, dom, host, node) { return None; }
            return Some((node, editing::len(dom, node)));
        }
        if page.layout().computed_style_value(dom, node, "display") != "inline"
            || local_html(dom, node, "br") || !editing::is_container(dom, node) { return None; }
        previous = dom.last_child(node).ok()?;
        if previous.is_none() { return Some((node, 0)); }
    }
    None
}

fn format_block(page: &Rc<PageDom>, host: NodeId, tag: &str) {
    let (start, end) = editing::host_range(page, host);
    editing::apply_edit(page, host, "formatBlock", None, None, |page| {
        let selection = {
            let mut dom = page.dom.borrow_mut();
            let Ok(doc) = dom.node_document(host) else { return };
            let mut first = editing::deep_forward(&dom, start);
            let mut last = editing::deep_backward(&dom, end);
            if start == end {
                first = if last.0 != start.0 { last } else { first };
                last = first;
            }
            let whole_child = (start.0 == end.0 && end.1.checked_sub(start.1) == Some(1))
                .then(|| dom.child_at(start.0, start.1).ok().flatten()).flatten();
            let table_contents = start.0 == end.0 && start.1 == 0 && end.1 > 0
                && local_html(&dom, start.0, "table") && dom.child_count(start.0).ok() == Some(end.1);
            let table = if table_contents && start.0 != host { Some(start.0) } else {
                whole_child.filter(|&n| local_html(&dom, n, "table"))
            };
            if let Some(table) = table {
                let Ok(Some(parent)) = dom.parent(table) else { return };
                let Ok(wrapper) = dom.create_element(doc, tag) else { return };
                if dom.insert_before(parent, wrapper, Some(table)).is_err() || dom.append_child(wrapper, table).is_err() { return; }
                let first = if table_contents { before_formatted_table(page, &dom, wrapper, host) } else { None }
                    .unwrap_or((wrapper, 0));
                let last = if editing::is_editable(&dom, first.0) { (wrapper, 1) } else { first };
                Some((first, last))
            } else {
                let lines = format_lines(page, &dom, host, start, end);
                let mut group = None;
                let mut first_content = None;
                for line in lines {
                    let (parent, before, children, remove) = match line {
                        FormatLine::Block(b) => {
                            let kids: Vec<_> = dom.children(b).map(|v| v.collect()).unwrap_or_default();
                            let old_tag = editing::local(&dom, b);
                            if FORMAT_BLOCKS.contains(&old_tag.as_str()) && start != end && kids.iter().all(|&n| {
                                dom.char_data(n).is_ok_and(|s| s.chars().all(|c| matches!(c, ' ' | '\t' | '\n' | '\r')))
                            }) { continue; }
                            if old_tag == tag {
                                first_content.get_or_insert_with(|| editing::deep_forward(&dom, (b, 0)));
                                group = None;
                                continue;
                            }
                            // Table cells and list items are structural containers.
                            // Format their contents without replacing the container.
                            if matches!(old_tag.as_str(), "td" | "th" | "li") {
                                (b, kids.first().copied(), kids, None)
                            } else {
                                let Some(p) = dom.parent(b).ok().flatten() else { continue };
                                // CSS display can create a paragraph boundary on
                                // a custom or preformatted element. Wrap that
                                // element intact unless its tag is replaceable.
                                if dom.element(b).is_ok_and(|el| el.is_html()) && FORMAT_BLOCKS.contains(&old_tag.as_str()) {
                                    (p, Some(b), kids, Some(b))
                                } else {
                                    (p, Some(b), vec![b], None)
                                }
                            }
                        }
                        FormatLine::Inline(kids) => (host, kids.first().copied(), kids, None),
                    };
                    if first_content.is_none() {
                        first_content = children.first().map(|&n| editing::deep_forward(&dom, (n, 0)));
                    }
                    let wrapper = if let Some(w) = group.filter(|&w| dom.parent(w).ok().flatten() == Some(parent)
                        && before.is_some() && dom.next_sibling(w).ok().flatten() == before)
                    {
                        let Ok(br) = dom.create_element(doc, "br") else { return };
                        let _ = dom.append_child(w, br);
                        w
                    } else {
                        let Ok(w) = dom.create_element(doc, tag) else { return };
                        if dom.insert_before(parent, w, before).is_err() { return; }
                        w
                    };
                    for n in children { let _ = dom.append_child(wrapper, n); }
                    if let Some(b) = remove { let _ = dom.remove_child(parent, b); }
                    group = Some(wrapper);
                }
                if !editing::is_text(&dom, first.0) || dom.char_data(first.0).is_ok_and(|s| s.trim().is_empty()) {
                    if let Some(p) = first_content { first = p; }
                }
                let live = |p: Point| dom.is_connected(p.0).unwrap_or(false);
                (live(first) && live(last)).then_some((first, last))
            }
        };
        let (s, e) = selection.unwrap_or_else(|| {
            let p = editing::host_start(&page.dom.borrow(), host);
            (p, p)
        });
        if start != end && s == e && !editing::is_editable(&page.dom.borrow(), s.0) {
            // Formatting normalizes a structural table range to forward,
            // even when its visible endpoints coincide on the locked edge.
            editing::set_adjusted_caret(page, s, false);
        } else {
            editing::set_selection(page, s, e);
        }
    });
}

/// Carry inherited text appearance before the items leave their source
/// list. A plain text run can carry its style on a span; for structured
/// items keep the existing children, including locked descendants, intact.
fn carry_list_text_style(page: &PageDom, dom: &mut Dom, source: NodeId, destination: NodeId, items: &[NodeId]) -> Option<()> {
    use pyro_dom::stylo::declarations::DeclarationBlock;
    let layout = page.layout();
    let authored = DeclarationBlock::parse(dom.get_attribute(source, "style").ok()?.unwrap_or(""));
    let mut changes = Vec::new();
    for property in ["color", "font-family", "font-size", "font-style", "font-weight"] {
        let old = layout.computed_style_value(dom, source, property);
        let new = layout.computed_style_value(dom, destination, property);
        if !old.is_empty() && !new.is_empty() && old != new {
            changes.push((property, old, new));
        }
    }
    if changes.is_empty() { return Some(()); }
    let doc = dom.node_document(source).ok()?;
    for &item in items {
        if dom.element(item).is_err() { continue; }
        let children: Vec<_> = dom.children(item).ok()?.collect();
        let text_only = !children.is_empty() && children.iter().all(|&n| editing::is_text(dom, n));
        let mut style = if text_only { DeclarationBlock::parse("") }
            else { DeclarationBlock::parse(dom.get_attribute(item, "style").ok()?.unwrap_or("")) };
        let mut changed = false;
        for (property, old, new) in &changes {
            let value = layout.computed_style_value(dom, item, property);
            if value.is_empty() || &value == new || !style.value(property).is_empty() { continue; }
            // Prefer the authored color spelling when it resolves to the
            // actual source color. Relative font sizes must use their
            // resolved value to avoid scaling a second time.
            let css = authored.value(property);
            let css = if *property == "color" && &value == old && !css.is_empty()
                && pyro_dom::color_well::editing_color(&css).is_some_and(|(c, _)| c == value)
            { css } else { value };
            style.set(property, &css, false)?;
            changed = true;
        }
        if !changed { continue; }
        if text_only {
            let span = dom.create_element(doc, "span").ok()?;
            dom.set_attribute(span, "style", &style.css_text()).ok()?;
            dom.insert_before(item, span, children.first().copied()).ok()?;
            for child in children { dom.append_child(span, child).ok()?; }
        } else {
            dom.set_attribute(item, "style", &style.css_text()).ok()?;
        }
    }
    Some(())
}

/// Convert a contiguous run of items in an opposite-type list at the same
/// nesting level. Keep the list items (and their subtrees) intact.
fn convert_list_items(page: &PageDom, dom: &mut Dom, host: NodeId, tag: &str, start: Point, end: Point) -> Option<(Point, Point)> {
    let first_point = editing::deep_forward(dom, start);
    let last_point = if start == end { first_point } else { editing::deep_backward(dom, end) };
    let first = ancestor_item(dom, first_point.0, host)?;
    let last = ancestor_item(dom, last_point.0, host)?;
    let source = dom.parent_element(first).ok()??;
    if source == host || !(local_html(dom, source, "ol") || local_html(dom, source, "ul")) || local_html(dom, source, tag) {
        return None;
    }

    // An outer paragraph and its subordinate list paragraphs each need
    // conversion, but an unselected subordinate list keeps its type.
    let nested = start != end && dom.contains(source, last).ok()? && dom.descendants(source).ok()?.skip(1).any(|n|
        local_html(dom, n, "ol") || local_html(dom, n, "ul"));
    if nested {
        let mut runs: Vec<(NodeId, NodeId, NodeId)> = Vec::new();
        for text in texts_in(dom, source, start, end) {
            if !rich_node(dom, text) { return None; }
            let item = ancestor_item(dom, text, host)?;
            let list = dom.parent_element(item).ok()??;
            if !(local_html(dom, list, "ol") || local_html(dom, list, "ul")) { return None; }
            if local_html(dom, list, tag) { continue; }
            if let Some(run) = runs.iter_mut().find(|r| r.0 == list) { run.2 = item; }
            else { runs.push((list, item, item)); }
        }
        if runs.is_empty() { return None; }
        for &(list, first, last) in &runs {
            let children: Vec<_> = dom.children(list).ok()?.collect();
            let run = children.get(dom.index_in_parent(first).ok()? as usize..=dom.index_in_parent(last).ok()? as usize)?;
            if !run.iter().all(|&n| editable_list_item(dom, host, n)) { return None; }
        }
        for (list, first, last) in runs.into_iter().rev() {
            convert_list_run(page, dom, host, tag, list, first, last, true)?;
        }
    } else {
        if dom.parent_element(last).ok()?? != source { return None; }
        convert_list_run(page, dom, host, tag, source, first, last, false)?;
    }
    Some((if start.0 == source { first_point } else { start }, if end.0 == source { last_point } else { end }))
}

fn ancestor_item(dom: &Dom, mut node: NodeId, host: NodeId) -> Option<NodeId> {
    while node != host {
        if local_html(dom, node, "li") { return Some(node); }
        node = dom.parent_element(node).ok()??;
    }
    None
}

fn editable_list_item(dom: &Dom, host: NodeId, node: NodeId) -> bool {
    local_html(dom, node, "li") && editing::editing_host(dom, node) == Some(host)
        || dom.char_data(node).is_ok_and(|s| s.trim().is_empty())
}

/// Preserve the inherited color on the text when moving a structured list
/// item; the descendant list itself has an independent numbering context.
fn distribute_nested_item_color(page: &PageDom, dom: &mut Dom, host: NodeId, item: NodeId) -> Option<()> {
    use pyro_dom::stylo::declarations::DeclarationBlock;
    let mut style = DeclarationBlock::parse(dom.get_attribute(item, "style").ok()??);
    let color = style.value("color");
    if color.is_empty() || !dom.descendants(item).ok()?.any(|n| local_html(dom,n,"ol") || local_html(dom,n,"ul")) { return None; }
    let layout = page.layout();
    let mut texts = Vec::new();
    for n in dom.descendants(item).ok()? {
        if editing::editing_host(dom, n) != Some(host) { return None; }
        if editing::is_text(dom, n) {
            let parent = dom.parent_element(n).ok()??;
            texts.push((n, parent, layout.computed_style_value(dom, parent, "color")));
        }
    }
    let priority = style.important("color");
    style.remove("color");
    if style.is_empty() { dom.remove_attribute(item, "style").ok()?; }
    else { dom.set_attribute(item, "style", &style.css_text()).ok()?; }
    let doc = dom.node_document(item).ok()?;
    for (text, parent, old) in texts {
        if old == layout.computed_style_value(dom, parent, "color") { continue; }
        let span = dom.create_element(doc, "span").ok()?;
        let mut retained = DeclarationBlock::parse("");
        let value = if pyro_dom::color_well::editing_color(&color).is_some_and(|(css,_)| css == old) { &color } else { &old };
        retained.set("color", value, priority)?;
        dom.set_attribute(span, "style", &retained.css_text()).ok()?;
        dom.insert_before(parent, span, Some(text)).ok()?;
        dom.append_child(span, text).ok()?;
    }
    Some(())
}

/// Join a selected bare host paragraph with the immediately following
/// selected list items, leaving the trailing host content outside the list.
/// Turn consecutive host lines separated by BR into distinct items, keeping
/// inline wrappers on their lines and both text selection endpoints alive.
fn listify_break_lines(dom: &mut Dom, host: NodeId, tag: &str, start: Point, end: Point) -> Option<()> {
    if !editing::is_text(dom,start.0) || !editing::is_text(dom,end.0) || start.0 == end.0 { return None; }
    let unit = |node| {
        let parent = dom.parent_element(node).ok()??;
        if parent == host { Some(node) } else if dom.parent(parent).ok()? == Some(host) && local_html(dom,parent,"span") {
            Some(parent)
        } else { None }
    };
    let first = unit(start.0)?;
    let last = unit(end.0)?;
    if first == last { return None; }
    let mut runs = vec![Vec::new()];
    let mut breaks = Vec::new();
    let mut cur = first;
    loop {
        if editing::editing_host(dom,cur) != Some(host) { return None; }
        if local_html(dom,cur,"br") {
            breaks.push(cur);
            runs.push(Vec::new());
        } else {
            if !editing::is_text(dom,cur) && !(local_html(dom,cur,"span") && dom.children(cur).ok()?.all(|n|
                editing::is_text(dom,n) || local_html(dom,n,"br") && dom.next_sibling(n).ok()==Some(None))) { return None; }
            runs.last_mut()?.push(cur);
            if cur != last && dom.last_child(cur).ok()?.is_some_and(|br| local_html(dom,br,"br")) {
                breaks.push(dom.last_child(cur).ok()??);
                runs.push(Vec::new());
            }
        }
        if cur == last { break; }
        cur = dom.next_sibling(cur).ok()??;
    }
    if runs.len() < 2 || runs.iter().any(Vec::is_empty) { return None; }
    // Limit this path to complete lines; the normal list logic handles
    // other unselected inline siblings sharing either boundary line.
    let boundary = |n| local_html(dom,n,"br") || local_html(dom,n,"ol") || local_html(dom,n,"ul");
    if dom.prev_sibling(first).ok()?.is_some_and(|n| !boundary(n)) || dom.next_sibling(last).ok()?.is_some_and(|n| !boundary(n)) { return None; }
    if let Some(br) = dom.next_sibling(last).ok()?.filter(|&n| local_html(dom,n,"br")) { breaks.push(br); }
    let doc = dom.node_document(host).ok()?;
    let list = dom.create_element(doc,tag).ok()?;
    dom.insert_before(host,list,Some(first)).ok()?;
    for run in runs {
        let item = dom.create_element(doc,"li").ok()?;
        dom.append_child(list,item).ok()?;
        for n in run { dom.append_child(item,n).ok()?; }
    }
    for br in breaks { dom.remove_node(br).ok()?; }
    Some(())
}

fn convert_text_and_list(page: &PageDom, dom: &mut Dom, host: NodeId, tag: &str, start: Point, end: Point) -> Option<()> {
    if !editing::is_text(dom, start.0) || dom.parent(start.0).ok()? != Some(host) || start == end { return None; }
    let source = dom.next_sibling(start.0).ok()??;
    if !(local_html(dom, source, "ol") || local_html(dom, source, "ul")) { return None; }
    let last = ancestor_item(dom, end.0, host)?;
    if dom.parent_element(last).ok()? != Some(source) { return None; }
    let first = dom.first_child(source).ok()??;
    if !local_html(dom, first, "li") { return None; }
    // Capture the whole preceding inline line, including styled siblings
    // before the selection's first text.
    let mut begin = start.0;
    while let Some(prev) = dom.prev_sibling(begin).ok()? {
        if editing::editing_host(dom, prev) != Some(host) || local_html(dom, prev, "br")
            || !editing::is_text(dom, prev) && page.layout().computed_style_value(dom, prev, "display") != "inline" { break; }
        begin = prev;
    }
    let mut line = Vec::new();
    let mut node = begin;
    loop {
        line.push(node);
        if node == start.0 { break; }
        node = dom.next_sibling(node).ok()??;
    }
    let convert = !local_html(dom, source, tag);
    let separate = convert && dom.next_sibling(last).ok()?.is_none()
        && crate::range::compare_boundary_points_dom(dom, end, editing::deep_backward(dom, (last, editing::len(dom,last)))) < 0;
    if convert { convert_list_run(page, dom, host, tag, source, first, last, false)?; }
    let doc = dom.node_document(host).ok()?;
    let item = dom.create_element(doc, "li").ok()?;
    if separate {
        let list = dom.create_element(doc, tag).ok()?;
        dom.insert_before(host, list, Some(begin)).ok()?;
        dom.append_child(list, item).ok()?;
    } else {
        let list = dom.parent_element(first).ok()??;
        dom.insert_before(list, item, Some(first)).ok()?;
    }
    for n in line { dom.append_child(item, n).ok()?; }
    Some(())
}

/// Extract selected levels of a nested list as lines. A nested list of
/// another type remains an intact block between the surrounding text.
fn unlist_nested_run(page: &PageDom, dom: &mut Dom, host: NodeId, tag: &str, start: Point, end: Point) -> Option<()> {
    use std::collections::HashSet;
    let first = ancestor_item(dom, start.0, host)?;
    let list = dom.parent_element(first).ok()??;
    if start == end || !local_html(dom, list, tag) || dom.parent(list).ok()? != Some(host)
        || !dom.contains(list, end.0).ok()? || !dom.descendants(list).ok()?.skip(1).any(|n| local_html(dom,n,"ol") || local_html(dom,n,"ul"))
        || dom.descendants(list).ok()?.any(|n| editing::editing_host(dom,n) != Some(host))
    { return None; }
    let selected: HashSet<_> = texts_in(dom, list, start, end).into_iter().filter_map(|t| ancestor_item(dom,t,host)).collect();
    fn lines(dom: &Dom, list: NodeId, tag: &str, selected: &HashSet<NodeId>, output: &mut Vec<(bool, Vec<NodeId>)>) -> Option<()> {
        let items: Vec<_> = dom.children(list).ok()?.collect();
        if items.is_empty() || !items.iter().all(|&n| local_html(dom,n,"li") && selected.contains(&n)) { return None; }
        for item in items {
            let mut run = Vec::new();
            for child in dom.children(item).ok()? {
                if local_html(dom,child,"ol") || local_html(dom,child,"ul") {
                    if !run.is_empty() { output.push((false,std::mem::take(&mut run))); }
                    if local_html(dom,child,tag) && dom.children(child).ok()?.all(|n| selected.contains(&n)) {
                        lines(dom,child,tag,selected,output)?;
                    } else { output.push((true,vec![child])); }
                } else { run.push(child); }
            }
            if !run.is_empty() { output.push((false,run)); }
        }
        Some(())
    }
    let mut output = Vec::new();
    lines(dom,list,tag,&selected,&mut output)?;
    let doc = dom.node_document(host).ok()?;
    let layout = page.layout();
    let mut previous = None;
    for (block, mut children) in output {
        if block && children.len() == 1 {
            let inner = children[0];
            if !local_html(dom, inner, tag) && dom.attributes(inner).ok()?.is_empty()
                && dom.children(inner).ok()?.all(|item| selected.contains(&item))
            {
                let replacement = dom.create_element(doc,tag).ok()?;
                while let Some(item) = dom.first_child(inner).ok()? { dom.append_child(replacement,item).ok()?; }
                children[0] = replacement;
            }
        }
        if !block {
            if let Some(prev) = previous {
                let br = dom.create_element(doc,"br").ok()?;
                if local_html(dom,prev,"span") { dom.append_child(prev,br).ok()?; }
                else { dom.insert_before(host,br,Some(list)).ok()?; }
            }
        }
        previous = None;
        for n in children {
            // A text child inherits from the LI and its list until it is
            // lifted. Keep that appearance on a span only when it changes.
            let mut retained = pyro_dom::stylo::declarations::DeclarationBlock::parse("");
            if editing::is_text(dom, n) {
                let source = dom.parent_element(n).ok()??;
                for prop in ["color", "font-family", "font-size", "font-weight", "font-style"] {
                    let value = layout.computed_style_value(dom,source,prop);
                    if value.is_empty() || value == layout.computed_style_value(dom,host,prop) { continue; }
                    let mut el = Some(source);
                    let mut literal = value.clone();
                    while let Some(node) = el.filter(|&el| el != host) {
                        let style = pyro_dom::stylo::declarations::DeclarationBlock::parse(dom.get_attribute(node,"style").ok()?.unwrap_or(""));
                        let authored = style.value(prop);
                        if !authored.is_empty() {
                            if authored == value || prop == "color" && pyro_dom::color_well::editing_color(&authored).is_some_and(|(c,_)| c == value)
                                || prop == "font-weight" && authored == "bold" && value == "700" { literal = authored; }
                            break;
                        }
                        el = dom.parent_element(node).ok()?;
                    }
                    retained.set(prop, &literal, false)?;
                }
            }
            let unit = if retained.is_empty() { n } else {
                let span = dom.create_element(doc,"span").ok()?;
                dom.set_attribute(span,"style",&retained.css_text()).ok()?;
                dom.append_child(span,n).ok()?;
                span
            };
            dom.insert_before(host,unit,Some(list)).ok()?;
            if !block { previous = Some(unit); }
        }
    }
    if previous.is_some() && dom.next_sibling(list).ok()?.is_some_and(|n| local_html(dom,n,"ul") || local_html(dom,n,"ol")) {
        let br = dom.create_element(doc,"br").ok()?;
        dom.insert_before(host,br,Some(list)).ok()?;
    }
    dom.remove_node(list).ok()
}

fn convert_list_run(page: &PageDom, dom: &mut Dom, host: NodeId, tag: &str, source: NodeId, first: NodeId, last: NodeId, nested: bool) -> Option<()> {
    let parent = dom.parent_element(source).ok()??;
    let begin = dom.index_in_parent(first).ok()? as usize;
    let finish = dom.index_in_parent(last).ok()? as usize + 1;
    let children: Vec<_> = dom.children(source).ok()?.collect();
    let run = children.get(begin..finish)?;
    if run.is_empty() || !run.iter().all(|&n| editable_list_item(dom, host, n)) { return None; }

    let list = dom.create_element(dom.node_document(host).ok()?, tag).ok()?;
    // Preserve authored presentation on the selected items. Numbering and
    // the source list's ID belong to the source list, not the converted run.
    for name in ["dir", "lang"] {
        if nested && name == "dir" { continue; }
        if let Some(value) = dom.get_attribute(source, name).ok()?.map(str::to_owned) {
            dom.set_attribute(list, name, &value).ok()?;
        }
    }
    let after = dom.next_sibling(source).ok()?;
    let before = if begin == 0 { Some(source) } else { after };
    dom.insert_before(parent, list, before).ok()?;
    carry_list_text_style(page, dom, source, list, run)?;
    if begin > 0 && finish < children.len() {
        let tail = dom.clone_node(source, false, None).ok()?;
        dom.remove_attribute(tail, "id").ok()?;
        dom.insert_before(parent, tail, after).ok()?;
        for &n in &children[finish..] { dom.append_child(tail, n).ok()?; }
    }
    for &n in run {
        dom.append_child(list, n).ok()?;
        if nested && local_html(dom, n, "li") { distribute_nested_item_color(page, dom, host, n); }
    }
    if dom.child_count(source).ok()? == 0 { dom.remove_node(source).ok()?; }

    // Adjacent simple lists of the requested type form a single list.
    let compatible = |dom: &Dom, n| dom.element(n).is_ok_and(|el| el.is_html() && &**el.local_name() == tag)
        && dom.attributes(n).is_ok_and(|attrs| attrs.is_empty());
    if compatible(dom, list) {
        let mut target = list;
        if let Some(left) = dom.prev_sibling(list).ok()?.filter(|&n| compatible(dom, n)) {
            while let Some(n) = dom.first_child(list).ok()? { dom.append_child(left, n).ok()?; }
            dom.remove_node(list).ok()?;
            target = left;
        }
        if let Some(right) = dom.next_sibling(target).ok()?.filter(|&n| compatible(dom, n)) {
            while let Some(n) = dom.first_child(right).ok()? { dom.append_child(target, n).ok()?; }
            dom.remove_node(right).ok()?;
        }
    }
    Some(())
}

/// Listify the anonymous paragraph containing a selected host text node.
/// Its adjacent lists are boundaries, not children of the new item.
fn listify_bare_text(
    page: &PageDom, dom: &mut Dom, host: NodeId, tag: &str, start: Point, end: Point,
) -> Option<(Point, Point)> {
    let (text, from) = editing::deep_forward(dom, start);
    let (last, to) = editing::deep_backward(dom, end);
    let container = dom.parent_element(text).ok()??;
    let quoted = local_html(dom, container, "blockquote") ||
        (local_html(dom, container, "p")
            && dom.parent_element(container).ok()?.is_some_and(|p| local_html(dom, p, "blockquote")));
    if !editing::is_text(dom, text) || !editing::is_text(dom, last)
        || dom.parent(last).ok()? != Some(container) || (container != host && !quoted)
        || (text != last && !quoted) || editing::editing_host(dom, text) != Some(host)
        || editing::editing_host(dom, last) != Some(host) { return None; }

    let line_boundary = |n| {
        if editing::is_text(dom, n) { return false; }
        let display = page.layout().computed_style_value(dom, n, "display");
        editing::local(dom, n) == "br" || !(display == "contents" || display.starts_with("inline"))
    };
    let children: Vec<_> = dom.children(container).ok()?.collect();
    let index = dom.index_in_parent(text).ok()? as usize;
    let last_index = dom.index_in_parent(last).ok()? as usize;
    if last_index < index || children[index..=last_index].iter().any(|&n|
        line_boundary(n) && !local_html(dom, n, "br")) { return None; }
    let begin = children[..index].iter().rposition(|&n| line_boundary(n)).map_or(0, |i| i + 1);
    let end = children[last_index + 1..].iter().position(|&n| line_boundary(n)).map_or(children.len(), |i| last_index + 1 + i);
    let left = begin.checked_sub(1).map(|i| children[i]);
    let right = children.get(end).copied();
    let list_tag = |n| dom.element(n).ok().filter(|el| el.is_html()).map(|el| el.local_name());
    let is_list = |n| list_tag(n).is_some_and(|name| matches!(&**name, "ol" | "ul"));
    let closing_break = right.filter(|&n| editing::local(dom, n) == "br"
        && (quoted || dom.next_sibling(n).ok().flatten().is_some_and(is_list)));
    if !quoted && ![left, right].into_iter().flatten().any(|n| is_list(n) || local_html(dom, n, "blockquote"))
        && closing_break.is_none() { return None; }

    // A terminal locked list supplies no editable position past the line.
    // The command still completes its input and history step, with the
    // original text, lists and forward-ordered endpoints in place.
    if right.is_some_and(|n| is_list(n) && editing::editing_host(dom, n) != Some(host)
        && dom.next_sibling(n).ok() == Some(None)) {
        return Some(((text, from), (text, to)));
    }
    let matches = |n| list_tag(n).is_some_and(|name| &**name == tag) && editing::editing_host(dom, n) == Some(host);
    let left = left.filter(|&n| matches(n));
    let right = right.filter(|&n| matches(n) && (left.is_none()
        || dom.first_child(n).ok().flatten().is_none_or(|first| editing::editing_host(dom, first) == Some(host))));
    let doc = dom.node_document(host).ok()?;
    let mut item = dom.create_element(doc, "li").ok()?;
    let mut points = [(text, from), (last, to)];
    if let Some(right) = right {
        let first = dom.first_child(right).ok()?;
        dom.insert_before(right, item, first).ok()?;
        if let Some(left) = left {
            while let Some(child) = dom.first_child(left).ok()? {
                dom.insert_before(right, child, Some(item)).ok()?;
            }
            dom.remove_node(left).ok()?;
        }
    } else if let Some(left) = left {
        dom.append_child(left, item).ok()?;
    } else {
        let list = dom.create_element(doc, tag).ok()?;
        dom.insert_before(container, list, Some(children[begin])).ok()?;
        dom.append_child(list, item).ok()?;
    }
    for &child in &children[begin..end] {
        if quoted && local_html(dom, child, "br") {
            let list = dom.parent_element(item).ok()??;
            let next = dom.next_sibling(item).ok()?;
            item = dom.create_element(doc, "li").ok()?;
            dom.insert_before(list, item, next).ok()?;
            dom.remove_node(child).ok()?;
        } else if child == text || child == last {
            let content = dom.clone_node(child, false, None).ok()?;
            dom.append_child(item, content).ok()?;
            for point in &mut points { if point.0 == child { point.0 = content; } }
            dom.remove_node(child).ok()?;
        } else {
            dom.append_child(item, child).ok()?;
        }
    }
    if let Some(br) = closing_break { dom.remove_node(br).ok()?; }
    Some((points[0], points[1]))
}

fn local_html(dom: &Dom, node: NodeId, name: &str) -> bool {
    dom.element(node).is_ok_and(|el| el.is_html() && &**el.local_name() == name)
}

/// A table selected by its outer boundaries is a single list item.
/// A selection inside the cells formats their contents independently:
/// the row must keep its cells even when the selection spans rows.
fn listify_table_selection(page: &PageDom, dom: &mut Dom, host: NodeId, tag: &str, start: Point, end: Point) -> Option<(Point, Point)> {
    let doc = dom.node_document(host).ok()?;
    if start.0 == end.0 && end.1.checked_sub(start.1) == Some(1) {
        if let Some(table) = dom.child_at(start.0, start.1).ok()?.filter(|&n|
            dom.element(n).is_ok_and(|el| el.is_html() && &**el.local_name() == "table")
                && editing::editing_host(dom, n) == Some(host)) {
            let item = start.0;
            if local_html(dom, item, "li") && dom.child_count(item).ok()? == 1 {
                let list = dom.parent_element(item).ok()??;
                if local_html(dom, list, tag) && list != host && dom.child_count(list).ok()? == 1 {
                    let mut points = [start, end];
                    indentation::unwrap(dom, item, &mut points)?;
                    indentation::unwrap(dom, list, &mut points)?;
                    return Some((points[0], points[1]));
                }
                return None;
            }
            let list = dom.create_element(doc, tag).ok()?;
            let item = dom.create_element(doc, "li").ok()?;
            dom.insert_before(start.0, list, Some(table)).ok()?;
            dom.append_child(list, item).ok()?;
            dom.append_child(item, table).ok()?;
            return Some(((item, 0), (item, 1)));
        }
    }

    let first = editing::deep_forward(dom, start);
    let last = if start == end { first } else { editing::deep_backward(dom, end) };
    let first_cell = editing::enclosing_html(dom, first.0, &["td", "th"])?;
    let last_cell = editing::enclosing_html(dom, last.0, &["td", "th"])?;
    let table = editing::enclosing_html(dom, first_cell, &["table"])?;
    if editing::enclosing_html(dom, last_cell, &["table"])? != table || !dom.contains(host, table).ok()? { return None; }
    // Include intervening cells even if they contain a BR or no text.
    // A nested table is a single piece of content in its outer cell.
    let mut containers = Vec::new();
    let mut selected = false;
    for cell in dom.descendants(table).ok()? {
        if !local_html(dom, cell, "td") && !local_html(dom, cell, "th") { continue; }
        if editing::enclosing_html(dom, cell, &["table"]) != Some(table) { continue; }
        selected |= cell == first_cell;
        if selected && editing::editing_host(dom, cell) == Some(host) && editing::rendered(page, dom, cell) {
            let child = dom.first_child(cell).ok()?;
            let container = child.filter(|&c| dom.child_count(cell).ok() == Some(1)
                && (local_html(dom, c, "p") || local_html(dom, c, "div"))).unwrap_or(cell);
            let list = dom.first_child(container).ok()?.filter(|&l| dom.child_count(container).ok() == Some(1)
                && (local_html(dom, l, "ol") || local_html(dom, l, "ul")));
            let list_item = list.and_then(|l| dom.first_child(l).ok().flatten().filter(|&item|
                dom.child_count(l).ok() == Some(1) && local_html(dom, item, "li")).map(|item| (l, item)));
            if list.is_some() && list_item.is_none() { return None; }
            containers.push((container, list_item));
        }
        if cell == last_cell { break; }
    }
    let remove = !containers.is_empty() && containers.iter().all(|(_, list)| list.is_some_and(|(l, _)| local_html(dom, l, tag)));
    let mut points = [first, last];
    for (container, existing) in containers {
        if let Some((list, item)) = existing {
            if remove {
                indentation::unwrap(dom, item, &mut points)?;
                indentation::unwrap(dom, list, &mut points)?;
            } else if !local_html(dom, list, tag) {
                indentation::rename(dom, list, tag, &mut points)?;
            }
            continue;
        }
        let list = dom.create_element(doc, tag).ok()?;
        let item = dom.create_element(doc, "li").ok()?;
        while let Some(child) = dom.first_child(container).ok()? {
            dom.append_child(item, child).ok()?;
        }
        if dom.child_count(item).ok()? == 0 {
            let br = dom.create_element(doc, "br").ok()?;
            dom.append_child(item, br).ok()?;
        }
        dom.append_child(list, item).ok()?;
        dom.append_child(container, list).ok()?;
    }
    Some((points[0], points[1]))
}

/// Keep a directional DIV as the list's containing block. Its direction
/// supplies the paragraph base direction and the list-marker placement.
fn listify_directional_blocks(
    page: &PageDom, dom: &mut Dom, host: NodeId, tag: &str, start: Point, end: Point,
) -> Option<(Point, Point)> {
    let first = editing::deep_forward(dom, start);
    let last = if start == end { first } else { editing::deep_backward(dom, end) };
    let first_block = editing::block_of(dom, first.0, host);
    let last_block = editing::block_of(dom, last.0, host);
    if first_block == host || !local_html(dom, first_block, "div") || !local_html(dom, last_block, "div") { return None; }
    let parent = dom.parent_element(first_block).ok()??;
    if dom.parent_element(last_block).ok()? != Some(parent) { return None; }
    let mut blocks = Vec::new();
    let mut current = first_block;
    let mut directional = false;
    loop {
        if local_html(dom, current, "div") && editing::editing_host(dom, current) == Some(host) {
            directional |= dom.get_attribute(current, "dir").ok()?.is_some()
                || page.layout().computed_style_value(dom, current, "direction") == "rtl";
            blocks.push(current);
        } else if editing::editing_host(dom, current) == Some(host)
            && !dom.char_data(current).is_ok_and(|data| data.chars().all(|c| c.is_ascii_whitespace())) {
            return None;
        }
        if current == last_block { break; }
        current = dom.next_sibling(current).ok()??;
    }
    if !directional { return None; }
    let doc = dom.node_document(first_block).ok()?;
    let list = dom.create_element(doc, tag).ok()?;
    let mut points = [first, last];
    for block in blocks {
        let item = dom.create_element(doc, "li").ok()?;
        while let Some(child) = dom.first_child(block).ok()? {
            if editing::is_text(dom, child) {
                let copy = dom.clone_node(child, false, None).ok()?;
                dom.append_child(item, copy).ok()?;
                for point in &mut points { if point.0 == child { point.0 = copy; } }
                dom.remove_node(child).ok()?;
            } else {
                dom.append_child(item, child).ok()?;
            }
        }
        if dom.child_count(item).ok()? == 0 { editing::placeholder_if_empty(dom, item); }
        dom.append_child(list, item).ok()?;
        if block != first_block { dom.remove_node(block).ok()?; }
    }
    dom.append_child(first_block, list).ok()?;
    Some((points[0], points[1]))
}

/// A list occupying a directional paragraph unwraps into lines of that
/// same paragraph. Keep the container's authored attributes on toggle-off.
fn unlist_paragraph_lines(
    page: &PageDom, dom: &mut Dom, host: NodeId, list: NodeId, start: Point, end: Point,
) -> Option<(Point, Point)> {
    let parent = dom.parent_element(list).ok()??;
    let next = dom.next_sibling(list).ok()?;
    let quoted = local_html(dom, parent, "blockquote") || (local_html(dom, parent, "p")
        && dom.parent_element(parent).ok()?.is_some_and(|n| local_html(dom, n, "blockquote")));
    let beside_quote = parent == host && [dom.prev_sibling(list).ok()?, next].into_iter().flatten().any(|n| local_html(dom, n, "blockquote"));
    let directional = parent != host && local_html(dom, parent, "div") && dom.child_count(parent).ok()? == 1
        && (dom.get_attribute(parent, "dir").ok()?.is_some() || page.layout().computed_style_value(dom, parent, "direction") == "rtl");
    if !quoted && !beside_quote && !directional { return None; }
    let first = editing::deep_forward(dom, start);
    let last = if start == end { first } else { editing::deep_backward(dom, end) };
    let items: Vec<_> = dom.children(list).ok()?.collect();
    if !items.iter().all(|&n| local_html(dom, n, "li") && editing::editing_host(dom, n) == Some(host))
        || !dom.contains(*items.first()?, first.0).ok()? || !dom.contains(*items.last()?, last.0).ok()? { return None; }
    let doc = dom.node_document(list).ok()?;
    for (index, item) in items.into_iter().enumerate() {
        if index != 0 {
            let br = dom.create_element(doc, "br").ok()?;
            dom.insert_before(parent, br, Some(list)).ok()?;
        }
        while let Some(child) = dom.first_child(item).ok()? {
            dom.insert_before(parent, child, Some(list)).ok()?;
        }
    }
    if next.is_some_and(|n| !local_html(dom, n, "br") && (quoted && !editing::is_block(dom, n)
        || beside_quote && local_html(dom, n, "blockquote"))) {
        let br = dom.create_element(doc, "br").ok()?;
        dom.insert_before(parent, br, Some(list)).ok()?;
    }
    dom.remove_node(list).ok()?;
    Some((first, last))
}

/// insertOrderedList / insertUnorderedList: the selected blocks become
/// items of a new list; inside such a list already, its items become `div`
/// lines again.
/// Listify the empty visual line represented by a paragraph's leading BR.
/// The paragraph survives unless its entire line joins an adjacent list.
fn listify_placeholder(page: &PageDom, dom: &mut Dom, host: NodeId, tag: &str, point: Point) -> Option<Point> {
    let (block, offset) = point;
    if block == host || !(local_html(dom, block, "p") || local_html(dom, block, "div"))
        || editing::editing_host(dom, block) != Some(host) || ancestor_named(dom, block, host, &["li"]).is_some()
    { return None; }
    let br = dom.first_child(block).ok()??;
    let whole = dom.child_count(block).ok()? == 1;
    if !local_html(dom, br, "br") || !(offset == 0 || whole && offset == 1)
        || editing::editing_host(dom, br) != Some(host) || !alignment::visible(page, dom, br) { return None; }
    let parent = dom.parent_element(block).ok()??;
    let matches = |n| whole && local_html(dom, n, tag) && editing::editing_host(dom, n) == Some(host);
    let left = dom.prev_sibling(block).ok()?.filter(|&n| matches(n));
    let right = dom.next_sibling(block).ok()?.filter(|&n| matches(n));
    let doc = dom.node_document(block).ok()?;
    let item = dom.create_element(doc, "li").ok()?;
    let placeholder = dom.clone_node(br, false, None).ok()?;
    dom.append_child(item, placeholder).ok()?;
    if let Some(list) = right {
        let first = dom.first_child(list).ok()?;
        dom.insert_before(list, item, first).ok()?;
        if let Some(left) = left {
            while let Some(child) = dom.first_child(left).ok()? {
                dom.insert_before(list, child, Some(item)).ok()?;
            }
            dom.remove_child(parent, left).ok()?;
        }
    } else if let Some(list) = left {
        dom.append_child(list, item).ok()?;
    } else {
        let list = dom.create_element(doc, tag).ok()?;
        dom.insert_before(block, list, Some(br)).ok()?;
        dom.append_child(list, item).ok()?;
    }
    dom.remove_child(block, br).ok()?;
    if left.is_some() || right.is_some() { dom.remove_child(parent, block).ok()?; }
    Some((item, 0))
}

/// Toggle out a single empty item without converting its siblings.
fn unlist_placeholder(dom: &mut Dom, host: NodeId, tag: &str, point: Point) -> Option<Point> {
    let (item, offset) = point;
    let list = dom.parent_element(item).ok()??;
    if offset != 0 || !local_html(dom, item, "li") || !local_html(dom, list, tag)
        || editing::editing_host(dom, item) != Some(host) || dom.child_count(item).ok()? != 1 { return None; }
    let br = dom.first_child(item).ok()??;
    if !local_html(dom, br, "br") || editing::editing_host(dom, br) != Some(host) { return None; }
    let parent = dom.parent_element(list).ok()??;
    let prev = dom.prev_sibling(item).ok()?;
    let next = dom.next_sibling(item).ok()?;
    let before = if prev.is_none() { Some(list) } else if let Some(next) = next {
        let tail = dom.clone_node(list, false, None).ok()?;
        dom.remove_attribute(tail, "id").ok()?;
        let after = dom.next_sibling(list).ok()?;
        dom.insert_before(parent, tail, after).ok()?;
        let mut child = Some(next);
        while let Some(n) = child {
            child = dom.next_sibling(n).ok()?;
            dom.append_child(tail, n).ok()?;
        }
        Some(tail)
    } else { dom.next_sibling(list).ok()? };
    dom.insert_before(parent, br, before).ok()?;
    dom.remove_child(list, item).ok()?;
    if dom.child_count(list).ok()? == 0 { dom.remove_child(parent, list).ok()?; }
    Some((parent, dom.index_in_parent(br).ok()?))
}

fn toggle_list(page: &Rc<PageDom>, host: NodeId, tag: &str, input_type: &str) {
    let (start, end) = editing::host_range(page, host);
    editing::apply_edit(page, host, input_type, None, None, |page| {
        {
            let mut dom = page.dom.borrow_mut();
            let Ok(doc) = dom.node_document(host) else { return };
            if unlist_nested_run(page, &mut dom, host, tag, start, end).is_some() {
                drop(dom);
                editing::set_selection(page, start, end);
                return;
            }
            if listify_break_lines(&mut dom, host, tag, start, end).is_some()
                || convert_text_and_list(page, &mut dom, host, tag, start, end).is_some() {
                drop(dom);
                editing::set_selection(page, start, end);
                return;
            }
            if let Some((a, b)) = convert_list_items(page, &mut dom, host, tag, start, end) {
                drop(dom);
                editing::set_selection(page, a, b);
                return;
            }
            if let Some((a, b)) = listify_bare_text(page, &mut dom, host, tag, start, end) {
                drop(dom);
                editing::set_selection(page, a, b);
                return;
            }
            if let Some((a, b)) = listify_table_selection(page, &mut dom, host, tag, start, end) {
                drop(dom);
                editing::set_selection(page, a, b);
                return;
            }
            if start == end {
                if let Some(caret) = unlist_placeholder(&mut dom, host, tag, start)
                    .or_else(|| listify_placeholder(page, &mut dom, host, tag, start)) {
                    drop(dom);
                    editing::set_selection(page, caret, caret);
                    return;
                }
            }
            let in_list = ancestor_named(&dom, start.0, host, &[tag]);
            if let Some(list) = in_list {
                if let Some((a, b)) = unlist_paragraph_lines(page, &mut dom, host, list, start, end) {
                    drop(dom);
                    editing::set_selection(page, a, b);
                    return;
                }
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
                if let Some((a, b)) = listify_directional_blocks(page, &mut dom, host, tag, start, end) {
                    drop(dom);
                    editing::set_selection(page, a, b);
                    return;
                }
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
/// Two DOM text endpoints can occupy the same insertion position across
/// adjoining inline elements. Stop at any intervening sibling or block.
pub(crate) fn touching_inline_text(page: &PageDom, dom: &Dom, start: Point, end: Point) -> bool {
    if start.0 == end.0 || end.1 != 0 || !editing::is_text(dom, start.0) || !editing::is_text(dom, end.0)
        || start.1 == 0 || start.1 != editing::len(dom, start.0) || editing::len(dom, end.0) == 0
    { return false; }
    let Some(host) = editing::editing_host(dom, start.0) else { return false };
    if editing::editing_host(dom, end.0) != Some(host) || !rich_node(dom, start.0) || !rich_node(dom, end.0)
        || !alignment::visible(page, dom, start.0) || !alignment::visible(page, dom, end.0) { return false; }
    let inline = |n| dom.element(n).is_ok_and(|e| e.is_html())
        && page.layout().computed_style_value(dom, n, "display") == "inline";
    let mut left = start.0;
    let mut right = loop {
        if let Ok(Some(next)) = dom.next_sibling(left) { break next; }
        let Some(parent) = dom.parent_element(left).ok().flatten() else { return false };
        if parent == host || !inline(parent) { return false; }
        left = parent;
    };
    while right != end.0 {
        if !inline(right) { return false; }
        let Some(child) = dom.first_child(right).ok().flatten() else { return false };
        right = child;
    }
    true
}

/// A child range can enclose markup without covering a rendered character.
/// Use the neighboring text to determine which visible caret it represents.
fn empty_inline_caret(page: &PageDom, dom: &Dom, start: Point, end: Point) -> Option<(Point, NodeId)> {
    if start.0 != end.0 || start.1 >= end.1 { return None; }
    let host = editing::editing_host(dom, start.0)?;
    let empty = |n| {
        dom.element(n).is_ok_and(|e| e.is_html() && matches!(&**e.local_name(), "b" | "strong" | "i" | "em" | "span"))
            && editing::editing_host(dom, n) == Some(host) && !editing::plaintext_at(dom, n)
            && page.layout().computed_style_value(dom, n, "display") == "inline"
    };
    for index in start.1..end.1 {
        let child = dom.child_at(start.0, index).ok()??;
        if !empty(child) || !dom.descendants(child).ok()?.all(|n| empty(n) || editing::is_text(dom, n) && editing::len(dom, n) == 0) {
            return None;
        }
    }
    let before = editing::deep_backward(dom, start);
    let after = editing::deep_forward(dom, end);
    [(start, before), (end, after)].into_iter().find_map(|(edge, (n, _))|
        (editing::is_text(dom, n) && editing::len(dom, n) != 0 && editing::editing_host(dom, n) == Some(host)
            && alignment::visible(page, dom, n)).then_some((edge, n)))
}

/// Insert at the visible edge rather than deleting the empty markup that
/// the original DOM range encloses.
pub(crate) fn empty_bold_typing_point(page: &PageDom, dom: &Dom, start: Point, end: Point) -> Option<Point> {
    let enabled = inline_value::toggle_state(page, (start, end), "b")?;
    let (edge, _) = empty_inline_caret(page, dom, start, end)?;
    if enabled && !dom.document_data(page.document()).ok()?.style_with_css {
        let before = edge.1.checked_sub(1).and_then(|i| dom.child_at(edge.0, i).ok().flatten());
        let after = dom.child_at(edge.0, edge.1).ok()?;
        for (node, at_end) in [(before, true), (after, false)] {
            if let Some(b) = node.filter(|&n| local_html(dom, n, "b") && dom.attributes(n).is_ok_and(|a| a.is_empty())
                && rich_node(dom, n) && numeric_bold_weight(&page.layout().computed_style_value(dom, n, "font-weight"))) {
                return Some((b, if at_end { editing::len(dom, b) } else { 0 }));
            }
        }
    }
    Some(edge)
}

fn format_state(page: &PageDom, command: &str) -> (bool, bool) {
    let Some((start, end)) = editing::selection_range(page) else { return (false, false) };
    if command == "bold" && start != end {
        if let Some((_, adjacent)) = empty_inline_caret(page, &page.dom.borrow(), start, end) {
            let active = inline_value::toggle_state(page, (start, end), "b").unwrap_or_else(|| {
                let dom = page.dom.borrow();
                dom.parent_element(adjacent).ok().flatten().is_some_and(|el|
                    numeric_bold_weight(&page.layout().computed_style_value(&dom, el, "font-weight")))
            });
            return (active, active);
        }
    }
    if let Some((tag @ ("u" | "strike"), _, _)) = format_of(command) {
        if let Some(active) = inline_value::decoration_state(page, (start, end), tag) {
            return (active, active);
        }
    }
    if start == end || touching_inline_text(page, &page.dom.borrow(), start, end) {
        if let Some((tag, _, _)) = format_of(command) {
            if start == end && matches!(tag, "sub" | "sup") {
                if let Some(active) = inline_value::caret_script_state(page, start, tag) {
                    return (active, active);
                }
            }
            if let Some(active) = inline_value::toggle_state(page, (start, end), tag) {
                return (active, active);
            }
        }
    }
    let dom = page.dom.borrow();
    let doc = page.document();
    let scope = dom.body(doc).ok().flatten().or_else(|| dom.document_element(doc).ok().flatten());
    let Some(scope) = scope else { return (false, false) };
    let mut elements: Vec<NodeId> = if start == end {
        vec![start.0]
    } else {
        let texts = match editing::editing_host(&dom, start.0) {
            Some(host) if editing::editing_host(&dom, end.0) == Some(host) => inline_texts(page, &dom, host, start, end),
            _ => texts_in(&dom, scope, start, end),
        };
        texts.into_iter()
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
            "bold" => numeric_bold_weight(&value("font-weight")),
            "italic" => {
                matches!(value("font-style").as_str(), s if s.starts_with("italic") || s.starts_with("oblique"))
            }
            "underline" => decorated("underline"),
            "strikethrough" => decorated("line-through"),
            "superscript" => inline_value::selected_script(page, &dom, n, scope) == Some("sup"),
            "subscript" => inline_value::selected_script(page, &dom, n, scope) == Some("sub"),
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
            let Some((start, end)) = editing::selection_range(page) else {
                return false;
            };
            let dom = page.dom.borrow();
            let Some(host) = editing::editing_host(&dom, start.0) else {
                return false;
            };
            if start != end {
                if let Some(list) = ancestor_named(&dom, start.0, host, &["ol", "ul"]) {
                    if let Some(end_list) = ancestor_named(&dom, end.0, host, &["ol", "ul"]) {
                        if list != end_list && dom.parent(list).ok() == dom.parent(end_list).ok() { return false; }
                    }
                    let other = if tag == "ol" { "ul" } else { "ol" };
                    if editing::selection(page).is_some_and(|(anchor, _)| editing::editing_host(&dom, anchor.0) == Some(host)) {
                        return local_html(&dom, list, tag) && ancestor_named(&dom, end.0, host, &["ol", "ul"]) == Some(list)
                            && !dom.descendants(list).is_ok_and(|mut nodes| nodes.any(|n| local_html(&dom, n, other)));
                    }
                }
            }
            let list = ancestor_named(&dom, start.0, host, &[tag]);
            if start != end && ancestor_named(&dom, start.0, host, &["td", "th"]).is_some() {
                return list.is_some() && list == ancestor_named(&dom, end.0, host, &[tag]);
            }
            list.is_some()
        }
        "italic" => format_state(page, &cmd).1 || editing::selection_range(page)
            .is_some_and(|(start, end)| start != end && starts_in_semantic_italic(page, start, end)),
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
        "formatblock" => format_block_value(page).unwrap_or_default(),
        "defaultparagraphseparator" => {
            page.dom.borrow().document_data(doc).map(|d| d.default_paragraph_separator.to_owned()).unwrap_or_default()
        }
        _ if format_of(&cmd).is_some() || cmd.starts_with("insert") && cmd.ends_with("list") => {
            query_state(page, doc, &cmd).to_string()
        }
        _ => String::new(),
    }
}

/// A DOM range around empty inline markup can have just one visible caret
/// position. Unlink requires selected content even when the range is not
/// collapsed in the DOM.
fn unlink_has_content(page: &PageDom, dom: &Dom, host: NodeId, start: Point, end: Point) -> bool {
    if start == end { return false; }
    if start.0 != end.0 || editing::is_text(dom, start.0) { return true; }
    let visible = |node| {
        if !alignment::visible(page, dom, node) { return false; }
        if editing::is_text(dom, node) {
            return editing::len(dom, node) > 0 && !editing::invisible_formatting_space(page, dom, host, node);
        }
        let Ok(el) = dom.element(node) else { return false };
        if !el.is_html() || !editing::is_editable(dom, node)
            || !matches!(&**el.local_name(), "a" | "span" | "b" | "strong" | "i" | "em" | "u" | "s" | "font" | "small" | "sub" | "sup") {
            return true;
        }
        match page.layout().computed_style_value(dom, node, "display").as_str() {
            "inline" | "contents" => false,
            "inline-block" => {
                let rect = page.layout().bounding_client_rect(dom, node).layout_units();
                rect.width > 0.0 && rect.height > 0.0
            }
            _ => true,
        }
    };
    (start.1..end.1).filter_map(|i| dom.child_at(start.0, i).ok().flatten()).any(|child|
        visible(child) || dom.descendants(child).is_ok_and(|mut nodes| nodes.any(visible)))
}

/// Rich editing commands require a range in this host. Plaintext-only can
/// be inherited or overridden within the same editable tree.
fn rich_selection(page: &PageDom, host: NodeId, command: &str) -> bool {
    let Some((start, end)) = editing::selection_range(page) else { return false };
    // Unlink acts on selected content. A caret inside an anchor does not
    // select that anchor, even when it is the only content in the host.
    let dom = page.dom.borrow();
    if command == "unlink" && !unlink_has_content(page, &dom, host, start, end) { return false; }
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
        return matches!(command_target(page), Some(Target::Host(h)) if rich_selection(page, h, &cmd));
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

/// Flat text offsets cannot distinguish selecting a table from starting
/// inside its first cell, or ending a cell from starting the next one.
struct TableSelection {
    path: Vec<u32>,
    cell: Option<&'static str>,
    anchor: u32,
    focus: u32,
}

impl TableSelection {
    fn capture(dom: &Dom, host: NodeId, anchor: Point, focus: Point) -> Option<Self> {
        if anchor.0 != focus.0 || anchor.1 == focus.1 { return None; }
        let mut node = anchor.0;
        let start = anchor.1.min(focus.1);
        let end = anchor.1.max(focus.1);
        let cell = if start == 0 && end == editing::len(dom, node) && local_html(dom, node, "td") { Some("td") }
            else if start == 0 && end == editing::len(dom, node) && local_html(dom, node, "th") { Some("th") }
            else if end - start == 1 && dom.child_at(node, start).ok()?.is_some_and(|n| local_html(dom, n, "table")) { None }
            else { return None };
        let mut path = Vec::new();
        while node != host {
            path.push(dom.index_in_parent(node).ok()?);
            node = dom.parent(node).ok()??;
        }
        path.reverse();
        Some(Self { path, cell, anchor: anchor.1, focus: focus.1 })
    }

    fn resolve(&self, dom: &Dom, host: NodeId) -> Option<(Point, Point)> {
        let mut node = host;
        for &index in &self.path { node = dom.child_at(node, index).ok()??; }
        if editing::editing_host(dom, node) != Some(host) { return None; }
        let start = self.anchor.min(self.focus);
        let end = self.anchor.max(self.focus);
        let count = dom.child_count(node).ok()?;
        if end > count { return None; }
        let (a, b) = if let Some(tag) = self.cell {
            if !local_html(dom, node, tag) || start != 0 || end != count { return None; }
            (editing::deep_forward(dom, (node, start)), editing::deep_backward(dom, (node, end)))
        } else {
            if end - start != 1 || !local_html(dom, dom.child_at(node, start).ok()??, "table") { return None; }
            ((node, start), (node, end))
        };
        if editing::editing_host(dom, a.0) != Some(host) || editing::editing_host(dom, b.0) != Some(host)
            || crate::range::compare_boundary_points_dom(dom, a, b) > 0 { return None; }
        Some(if self.anchor < self.focus { (a, b) } else { (b, a) })
    }
}

/// The HTML parser closes a P before a list. Retain the location and
/// actual markup of quoted list paragraphs and the paragraph owning a saved placeholder item.
struct ListParagraph {
    path: Vec<u32>,
    html: String,
    children: u32,
}

impl ListParagraph {
    fn capture(dom: &Dom, host: NodeId, html: &str, caret_paragraph: Option<NodeId>) -> Vec<Self> {
        if (caret_paragraph.is_none() && !html.contains("<blockquote")) || !html.contains("<p") || !(html.contains("<ol") || html.contains("<ul")) {
            return Vec::new();
        }
        dom.descendants(host).into_iter().flatten().filter_map(|node| {
            if !local_html(dom, node, "p") { return None; }
            let children = dom.child_count(node).ok()?;
            if Some(node) != caret_paragraph && !(children == 1 && local_html(dom, dom.parent_element(node).ok()??, "blockquote")) {
                return None;
            }
            let list = dom.first_child(node).ok()??;
            if !local_html(dom, list, "ol") && !local_html(dom, list, "ul") { return None; }
            let mut path = Vec::new();
            let mut current = node;
            while current != host {
                path.push(dom.index_in_parent(current).ok()?);
                current = dom.parent(current).ok()??;
            }
            path.reverse();
            Some(Self { path, html: dom.outer_html(node).ok()?, children })
        }).collect()
    }

    fn restore(self, dom: &mut Dom, host: NodeId) -> Option<()> {
        let mut p = host;
        for index in self.path { p = dom.child_at(p, index).ok()??; }
        if !local_html(dom, p, "p")
            || dom.child_count(p).ok()? != 0 { return None; }
        let list = dom.next_sibling(p).ok()??;
        let mut children = Vec::new();
        let mut next = Some(list);
        let mut serialized = String::new();
        for _ in 0..self.children {
            let child = next?;
            next = dom.next_sibling(child).ok()?;
            serialized.push_str(&dom.outer_html(child).ok()?);
            children.push(child);
        }
        let tail = next?;
        if !(local_html(dom, list, "ol") || local_html(dom, list, "ul")) || !local_html(dom, tail, "p")
            || dom.child_count(tail).ok()? != 0 || !dom.attributes(tail).ok()?.is_empty() { return None; }
        // Repair only the precise empty-P/list/empty-P sequence corresponding
        // to the saved element. Leave other parser normalizations untouched.
        let empty = dom.outer_html(p).ok()?;
        let contents = self.html.strip_prefix(empty.strip_suffix("</p>")?)?.strip_suffix("</p>")?;
        if contents != serialized { return None; }
        for child in children { dom.append_child(p, child).ok()?; }
        dom.remove_node(tail).ok()
    }
}

/// Flat offsets are ambiguous at the leading edge of an empty list item.
struct PlaceholderCaret {
    path: Vec<u32>,
    list_tag: &'static str,
}

impl PlaceholderCaret {
    fn capture(dom: &Dom, host: NodeId, point: Point) -> Option<Self> {
        let (mut node, offset) = point;
        if offset != 0 || !local_html(dom, node, "li") || dom.child_count(node).ok()? != 1
            || !local_html(dom, dom.first_child(node).ok()??, "br") { return None; }
        let list = dom.parent_element(node).ok()??;
        let list_tag = if local_html(dom, list, "ol") { "ol" } else if local_html(dom, list, "ul") { "ul" } else { return None };
        let mut path = Vec::new();
        while node != host { path.push(dom.index_in_parent(node).ok()?); node = dom.parent(node).ok()??; }
        path.reverse();
        Some(Self { path, list_tag })
    }

    fn resolve(self, dom: &Dom, host: NodeId) -> Option<Point> {
        let mut item = host;
        for index in self.path { item = dom.child_at(item, index).ok()??; }
        let parent = dom.parent_element(item).ok()??;
        let br = dom.first_child(item).ok()??;
        (local_html(dom, parent, self.list_tag) && local_html(dom, item, "li")
            && dom.child_count(item).ok()? == 1 && local_html(dom, br, "br")
            && editing::editing_host(dom, br) == Some(host)).then_some((item, 0))
    }
}

enum Saved {
    /// Invalid foreColor is an undoable selection-only command. Its inverse
    /// uses the same endpoints because the command makes no DOM mutation.
    UnchangedForeground { anchor: Point, focus: Point },
    Control { value: String, selection: ControlSelection, opposite: Option<ControlSelection> },
    /// Saved anchor and focus offsets, in selection direction.
    Host { html: String, selection: Option<(u32, u32)>, table_selection: Option<TableSelection>, list_paragraphs: Vec<ListParagraph>, placeholder: Option<PlaceholderCaret> },
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
            Saved::UnchangedForeground { .. } => 2 * size_of::<Point>(),
            Saved::Control { value, .. } => value.len(),
            Saved::Host { html, table_selection, list_paragraphs, placeholder, .. } => html.len()
                + table_selection.as_ref().map_or(0, |s| s.path.len() * size_of::<u32>())
                + placeholder.as_ref().map_or(0, |p| p.path.len() * size_of::<u32>())
                + list_paragraphs.iter().map(|q| q.html.len() + q.path.len() * size_of::<u32>()).sum::<usize>(),
        }
    }
}

impl UndoHistory {
    pub(crate) fn continues_host_typing(&self, host: NodeId, point: Point) -> bool {
        self.typing == Some((host, Caret::Host(point)))
    }

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
        return Some(Saved::Control {
            value: dom.value(target).ok()?,
            selection: dom.selection_snapshot(target).ok()?,
            opposite: None,
        });
    }
    let html = dom.inner_html(target).ok()?;
    let collapsed = editing::selection(page).and_then(|(a, f)| (a == f).then_some(a));
    let placeholder = collapsed.and_then(|p| PlaceholderCaret::capture(&dom, target, p));
    let caret_paragraph = collapsed.and_then(|p| {
        let item = if local_html(&dom, p.0, "li") { p.0 } else { dom.parent_element(p.0).ok()?? };
        if !local_html(&dom, item, "li") { return None; }
        let list = dom.parent_element(item).ok()??;
        dom.parent_element(list).ok()?
    });
    let list_paragraphs = ListParagraph::capture(&dom, target, &html, caret_paragraph);
    drop(dom);
    let mut table_selection = None;
    let selection = editing::selection(page).and_then(|(anchor, focus)| {
        let dom = page.dom.borrow();
        let inside =
            editing::editing_host(&dom, anchor.0) == Some(target) && editing::editing_host(&dom, focus.0) == Some(target);
        drop(dom);
        inside.then(|| {
            let flat = Flat::build(page, target);
            let dom = page.dom.borrow();
            table_selection = TableSelection::capture(&dom, target, anchor, focus);
            (flat.pos_of(&dom, anchor), flat.pos_of(&dom, focus))
        })
    });
    Some(Saved::Host { html, selection, table_selection, list_paragraphs, placeholder })
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
    let mut history = page.undo.borrow_mut();
    if history.replaying { return; }
    if let Some(Step { saved: Saved::Control { opposite, .. }, .. }) = history.undo.last_mut().filter(|s| s.target == el) {
        // Redo restores the edit's resulting caret even if script moves the
        // selection before undo. Coalesced typing updates the same step.
        *opposite = page.dom.borrow().selection_after_edit(el).ok();
    }
}

fn record_unchanged_foreground(page: &PageDom, host: NodeId) {
    let Some((anchor, focus)) = editing::selection(page) else { return };
    let mut history = page.undo.borrow_mut();
    if !history.replaying {
        history.typing = None;
        history.pending_typing = false;
        history.push(Step { target: host, saved: Saved::UnchangedForeground { anchor, focus } });
    }
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
        Saved::UnchangedForeground { anchor, focus } => {
            let dom = page.dom.borrow();
            let valid = |(node, offset)| dom.contains(target, node).unwrap_or(false)
                && editing::editing_host(&dom, node) == Some(target) && offset <= editing::len(&dom, node);
            if valid(anchor) && valid(focus) {
                drop(dom);
                editing::set_selection(page, anchor, focus);
            }
        }
        Saved::Control { value, selection, .. } => {
            {
                let mut dom = page.dom.borrow_mut();
                let len = dom.value(target).map(|v| pyro_dom::dom::utf16_len(&v)).unwrap_or(0);
                let _ = dom.set_selection_range(target, 0, len);
                let _ = dom.insert_text(target, &value);
                let _ = dom.set_selection_range(target, selection.range.0, selection.range.1);
                let _ = dom.set_selection_direction(target, selection.direction);
            }
            editing::queue_control_selectionchange(page, target);
        }
        Saved::Host { html, selection, table_selection, list_paragraphs, placeholder } => {
            let ok = pyro_dom::parse::set_inner_html(&mut page.dom.borrow_mut(), target, &html).is_ok();
            if !ok {
                return;
            }
            for paragraph in list_paragraphs { paragraph.restore(&mut page.dom.borrow_mut(), target); }
            if let Some(caret) = placeholder.and_then(|p| p.resolve(&page.dom.borrow(), target)) {
                editing::set_selection(page, caret, caret);
                return;
            }
            if let Some((anchor, focus)) = table_selection.and_then(|s| s.resolve(&page.dom.borrow(), target)) {
                editing::set_selection(page, anchor, focus);
                return;
            }
            let flat = Flat::build(page, target);
            let points = selection.and_then(|(anchor, focus)| {
                // Range starts prefer the following text at a node boundary,
                // ends the preceding text. Equal offsets restore one caret.
                let a = flat.point_at(anchor, anchor < focus)?;
                let f = if anchor == focus { a } else { flat.point_at(focus, focus < anchor)? };
                Some((a, f))
            });
            let (anchor, focus) = points.unwrap_or_else(|| {
                let p = editing::host_start(&page.dom.borrow(), target);
                (p, p)
            });
            editing::set_selection(page, anchor, focus);
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
    let mut current = match &step.saved {
        Saved::UnchangedForeground { anchor, focus } => Some(Saved::UnchangedForeground { anchor: *anchor, focus: *focus }),
        _ => save(page, step.target),
    };
    if let (Saved::Control { selection, opposite: Some(other), .. },
        Some(Saved::Control { selection: inverse, opposite, .. })) = (&step.saved, &mut current)
    {
        *inverse = *other;
        *opposite = Some(*selection);
    }
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
