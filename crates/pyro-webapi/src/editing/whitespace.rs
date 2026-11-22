//! Canonical space sequences used by editable HTML. Alternate spaces and
//! NBSPs so adjacent typed spaces remain visible and can still wrap.

use crate::editing::{self, Point};
use crate::page::PageDom;
use pyro_dom::{Dom, NodeId, NodeType};

fn space(c: char) -> bool {
    matches!(c, ' ' | '\u{a0}')
}

fn collapses(page: &PageDom, dom: &Dom, node: NodeId) -> bool {
    let Some(el) = dom.parent_element(node).ok().flatten() else {
        return false;
    };
    let layout = page.layout();
    match layout.computed_style_value(dom, el, "white-space-collapse").as_str() {
        "collapse" | "preserve-breaks" => true,
        "" => matches!(layout.computed_style_value(dom, el, "white-space").as_str(), "normal" | "nowrap" | "pre-line"),
        _ => false,
    }
}

enum Next {
    Boundary,
    Break,
    Text(NodeId),
    PreservedText(NodeId),
    Content,
}

/// Walk across inline markup without crossing block, BR or editing-host
/// boundaries; replaced content occupies a position on the line.
fn adjacent(page: &PageDom, dom: &Dom, host: NodeId, mut n: NodeId, forward: bool) -> Next {
    loop {
        if n == host {
            return Next::Boundary;
        }
        let sibling = if forward { dom.next_sibling(n) } else { dom.prev_sibling(n) };
        if let Ok(Some(s)) = sibling {
            n = s;
            loop {
                if editing::is_text(dom, n) {
                    if !dom.char_data(n).unwrap_or("").is_empty() {
                        return if editing::editing_host(dom, n) != Some(host) {
                            Next::Content
                        } else if collapses(page, dom, n) {
                            Next::Text(n)
                        } else {
                            Next::PreservedText(n)
                        };
                    }
                    break;
                }
                if dom.node_type(n).ok() != Some(NodeType::Element) {
                    break;
                }
                let display = page.layout().computed_style_value(dom, n, "display");
                if display == "none" {
                    break;
                }
                if editing::local(dom, n) == "br" {
                    return Next::Break;
                }
                if !matches!(
                    display.as_str(),
                    "inline" | "contents" | "inline-block" | "inline-flex" | "inline-grid" | "inline-table" | "ruby"
                ) {
                    return Next::Boundary;
                }
                if editing::editing_host(dom, n) != Some(host)
                    || !editing::is_container(dom, n)
                    || display.starts_with("inline-")
                {
                    return Next::Content;
                }
                let child = if forward { dom.first_child(n) } else { dom.last_child(n) };
                if let Ok(Some(c)) = child {
                    n = c;
                } else {
                    break;
                }
            }
        } else {
            let Ok(Some(p)) = dom.parent(n) else {
                return Next::Boundary;
            };
            if p == host
                || !matches!(
                    page.layout().computed_style_value(dom, p, "display").as_str(),
                    "inline" | "contents" | "ruby"
                )
            {
                return Next::Boundary;
            }
            n = p;
        }
    }
}

/// Whether an extra preserved newline is needed to display the last empty
/// line. Text after an inline ancestor and an existing BR both supply it.
pub(super) fn at_line_end(page: &PageDom, dom: &Dom, host: NodeId, (node, offset): Point) -> bool {
    if offset < editing::len(dom, node) {
        return false;
    }
    matches!(adjacent(page, dom, host, node, true), Next::Boundary)
}

/// In a plaintext editor a terminal preserved LF can serve the same purpose
/// as a padding BR. Typing consumes it only on an otherwise empty last line.
pub(super) fn consume_padding_lf(page: &PageDom, dom: &mut Dom, host: NodeId, (node, offset): Point) {
    let Ok(text) = dom.char_data(node) else { return };
    if !text.ends_with('\n') || offset.checked_add(1) != Some(pyro_dom::dom::utf16_len(text)) {
        return;
    }
    let empty_line = text.ends_with("\n\n")
        || (text == "\n" && match adjacent(page, dom, host, node, false) {
            Next::Boundary | Next::Break => true,
            Next::Text(previous) | Next::PreservedText(previous) => {
                dom.char_data(previous).is_ok_and(|data| data.ends_with('\n'))
            }
            Next::Content => false,
        });
    if !empty_line || !editing::plaintext_at(dom, node) { return; }
    // An inline editing host keeps the final separator as inline content.
    let block = editing::block_of(dom, node, host);
    let display = page.layout().computed_style_value(dom, block, "display");
    if display != "inline" && display != "contents"
        && crate::edit_commands::newlines_significant_in(page, dom, node)
        && matches!(adjacent(page, dom, host, node, true), Next::Boundary)
    {
        let _ = dom.replace_data(node, offset, 1, "");
    }
}

type Segment = (NodeId, u32, u32);

/// Extend the local run through adjacent collapsing Text nodes.
fn extend(page: &PageDom, dom: &Dom, host: NodeId, mut node: NodeId, forward: bool, parts: &mut Vec<Segment>) -> bool {
    loop {
        let n = match adjacent(page, dom, host, node, forward) {
            Next::Boundary | Next::Break => return true,
            Next::Content | Next::PreservedText(_) => return false,
            Next::Text(n) => n,
        };
        let text = dom.char_data(n).unwrap_or("");
        let count = if forward {
            text.chars().take_while(|&c| space(c)).count()
        } else {
            text.chars().rev().take_while(|&c| space(c)).count()
        } as u32;
        let len = pyro_dom::dom::utf16_len(text);
        if count > 0 {
            parts.push((n, if forward { 0 } else { len - count }, count));
        }
        if count < len {
            return false;
        }
        node = n;
    }
}

fn canonical(mut n: usize, start: bool, end: bool) -> String {
    if n == 1 {
        return if start || end { "\u{a0}" } else { " " }.into();
    }
    let mut out = String::with_capacity(n);
    while n > 3 {
        out.push_str(if start { "\u{a0} " } else { " \u{a0}" });
        n -= 2;
    }
    out.push_str(match (n, start, end) {
        (3, false, false) => " \u{a0} ",
        (3, true, false) => "\u{a0}\u{a0} ",
        (3, false, true) => " \u{a0}\u{a0}",
        (3, true, true) => "\u{a0} \u{a0}",
        (2, true, true) => "\u{a0}\u{a0}",
        (2, false, true) => " \u{a0}",
        (2, _, _) => "\u{a0} ",
        _ => "",
    });
    out
}

/// Deleting one space from a previously visible run can turn its neighbors
/// into collapsing spaces. Re-canonicalize the remaining run without making
/// spaces visible that were originally collapsed, or rewriting spaces in
/// another formatting element.
fn deletes_visible_space(
    page: &PageDom,
    dom: &Dom,
    host: NodeId,
    node: NodeId,
    before: &str,
    after: &str,
    ch: char,
) -> bool {
    if !before.chars().next_back().is_some_and(space) && !after.chars().next().is_some_and(space) {
        return false;
    }
    if editing::editing_host(dom, node) != Some(host) || !collapses(page, dom, node) {
        return false;
    }
    let layout = page.layout();
    let mut parent = dom.parent_element(node).ok().flatten();
    if parent.is_none_or(|el| layout.computed_style_value(dom, el, "visibility") != "visible") {
        return false;
    }
    while let Some(el) = parent {
        if layout.computed_style_value(dom, el, "display") == "none" {
            return false;
        }
        parent = dom.parent_element(el).ok().flatten();
    }
    let left = before.trim_end_matches(space);
    let right = after.trim_start_matches(space);
    let prefix = &before[left.len()..];
    let suffix = &after[..after.len() - right.len()];
    // An existing pair of ASCII spaces already occupies at most one cell.
    if prefix.contains("  ")
        || suffix.contains("  ")
        || (ch == ' ' && (prefix.ends_with(' ') || suffix.starts_with(' ')))
    {
        return false;
    }
    let first = prefix.chars().next().unwrap_or(ch);
    let last = suffix.chars().next_back().unwrap_or(ch);
    let hard_edge = |inside: Option<char>, forward: bool| -> Option<bool> {
        let neighboring = match inside {
            Some(c) => c,
            None => match adjacent(page, dom, host, node, forward) {
                Next::Boundary | Next::Break => return Some(true),
                // Replaced or noneditable content might have no rendered width.
                Next::Content | Next::PreservedText(_) => return None,
                Next::Text(n) => {
                    let value = dom.char_data(n).ok()?;
                    if forward {
                        value.chars().next()?
                    } else {
                        value.chars().next_back()?
                    }
                }
            },
        };
        (!space(neighboring) && !matches!(neighboring, '\n' | '\r' | '\t' | '\x0c')).then_some(false)
    };
    let Some(begin_line) = hard_edge(left.chars().next_back(), false) else {
        return false;
    };
    let Some(end_line) = hard_edge(right.chars().next(), true) else {
        return false;
    };
    if (begin_line && first == ' ') || (end_line && last == ' ') {
        return false;
    }
    let loses_width = (prefix.ends_with(' ') && suffix.starts_with(' '))
        || (begin_line && prefix.is_empty() && suffix.starts_with(' '))
        || (end_line && suffix.is_empty() && prefix.ends_with(' '));
    if loses_width {
        return true;
    }
    wraps(page, dom, node)
}

fn wraps(page: &PageDom, dom: &Dom, node: NodeId) -> bool {
    let Some(el) = dom.parent_element(node).ok().flatten() else {
        return false;
    };
    let layout = page.layout();
    match layout.computed_style_value(dom, el, "text-wrap-mode").as_str() {
        "wrap" => true,
        "" => !layout
            .computed_style_value(dom, el, "white-space")
            .split_ascii_whitespace()
            .any(|keyword| keyword == "nowrap"),
        _ => false,
    }
}

/// Delete the remaining text after an ordinary space at the end of a line.
/// The run occupied one space before the edit, unless it began the line.
pub(super) fn delete_after_spaces(
    page: &PageDom, dom: &mut Dom, host: NodeId, from: Point, to: Point,
) -> Option<Point> {
    let node = from.0;
    if node != to.0 || from.1 >= to.1 || from.1 == 0 { return None; }
    let text = dom.char_data(node).ok()?;
    let (cut, end) = pyro_dom::dom::utf16_range(text, from.1, to.1 - from.1).ok()?;
    if end != text.len() || !text[..cut].ends_with(' ') || text[cut..].chars().any(char::is_whitespace) {
        return None;
    }
    if !collapses(page, dom, node) || !matches!(adjacent(page, dom, host, node, true), Next::Boundary) {
        return None;
    }
    let prefix = text[..cut].trim_end_matches(' ');
    let begin = pyro_dom::dom::utf16_len(prefix);
    if begin != 0 && !prefix.ends_with(char::is_whitespace) {
        dom.replace_data(node, begin, to.1 - begin, " ").ok()?;
        return Some((node, begin + 1));
    }
    if begin != 0 || !matches!(adjacent(page, dom, host, node, false), Next::Boundary | Next::Break) {
        return None;
    }
    // The line's only visible content was deleted. Put its placeholder at
    // that line's position, leaving neighboring block paragraphs untouched.
    let mut empty = node;
    let mut parent = dom.parent_element(empty).ok()??;
    while parent != host && dom.child_count(parent).ok()? == 1
        && matches!(editing::local(dom, parent).as_str(), "b" | "i" | "em" | "strong" | "u" | "s" | "span")
        && dom.attributes(parent).ok()?.is_empty()
        && page.layout().computed_style_value(dom, parent, "display") == "inline"
    {
        empty = parent;
        parent = dom.parent_element(empty).ok()??;
    }
    let offset = dom.index_in_parent(empty).ok()?;
    let br = dom.create_element(dom.node_document(parent).ok()?, "br").ok()?;
    dom.replace_child(parent, br, empty).ok()?;
    Some((parent, offset))
}

pub(super) enum Repair {
    Exposed,
    Deleted,
    Inserted,
    RemovedBox,
    Joined,
}

/// Whether the deletion needs whitespace repair at the resulting caret.
pub(super) fn deletion_repair(page: &PageDom, dom: &Dom, host: NodeId, point: Point, backward: bool) -> Option<Repair> {
    let (node, off) = point;
    if !editing::is_text(dom, node) {
        return None;
    }
    let Ok(text) = dom.char_data(node) else {
        return None;
    };
    let Ok((at, _)) = pyro_dom::dom::utf16_range(text, off, 0) else {
        return None;
    };
    let Some(ch) = (if backward { text[..at].chars().next_back() } else { text[at..].chars().next() }) else {
        return None;
    };
    if ch == '\n' {
        return None;
    }
    let cut = if backward { at - ch.len_utf8() } else { at };
    let before = &text[..cut];
    let after = &text[cut + ch.len_utf8()..];
    if space(ch) {
        // utf16_range maps a position inside a surrogate pair to that
        // scalar's start. Such a delete isn't targeting the preceding space.
        if text[at..].chars().next().is_some_and(|c| c.len_utf16() == 2) && pyro_dom::dom::utf16_len(&text[..at]) != off
        {
            return None;
        }
        return deletes_visible_space(page, dom, host, node, before, after, ch).then_some(Repair::Deleted);
    }
    let exposed = (after.is_empty()
        && before.chars().next_back().is_some_and(space)
        && (!before.trim_end_matches(space).is_empty()
            || !matches!(adjacent(page, dom, host, node, false), Next::Boundary | Next::Break))
        && matches!(adjacent(page, dom, host, node, true), Next::Boundary | Next::Break))
        || (before.is_empty()
            && after.chars().next().is_some_and(space)
            && (!after.trim_start_matches(space).is_empty()
                || !matches!(adjacent(page, dom, host, node, true), Next::Boundary | Next::Break))
            && matches!(adjacent(page, dom, host, node, false), Next::Boundary | Next::Break));
    exposed.then_some(Repair::Exposed)
}

fn has_sibling(dom: &Dom, host: NodeId, mut node: NodeId, forward: bool) -> bool {
    while node != host {
        let sibling = if forward { dom.next_sibling(node) } else { dom.prev_sibling(node) };
        if sibling.ok().flatten().is_some() {
            return true;
        }
        let Ok(Some(p)) = dom.parent(node) else {
            break;
        };
        node = p;
    }
    false
}

/// Removing an inline replaced box exposes the start of the following text.
/// At the end of its container the exposed text is instead on the left.
pub(super) fn after_replaced_element(page: &PageDom, dom: &mut Dom, host: NodeId, (parent, index): Point) {
    let point = if let Ok(Some(next)) = dom.child_at(parent, index) {
        (next, 0)
    } else if let Some(prev) = index.checked_sub(1).and_then(|i| dom.child_at(parent, i).ok().flatten()) {
        (prev, editing::len(dom, prev))
    } else {
        return;
    };
    at(page, dom, host, point, Repair::RemovedBox);
}

/// Preserve the visible width of the whitespace carried onto a new line.
pub(super) fn after_line_break(page: &PageDom, dom: &mut Dom, host: NodeId, point: Point, split_text: bool) {
    let (node, offset) = point;
    let Ok(text) = dom.char_data(node) else { return };
    let Ok((byte, _)) = pyro_dom::dom::utf16_range(text, offset, 0) else { return };
    let suffix = &text[byte..];
    let run = &suffix[..suffix.len() - suffix.trim_start_matches(space).len()];
    if run.is_empty() || editing::editing_host(dom, node) != Some(host) || !collapses(page, dom, node) {
        return;
    }
    let at_end = run.len() == suffix.len() && matches!(adjacent(page, dom, host, node, true), Next::Boundary | Next::Break);
    let mut count = 0;
    let mut ordinary = false;
    for ch in run.chars() {
        if !split_text || ch != ' ' || !ordinary {
            count += 1;
        }
        ordinary = ch == ' ';
    }
    if split_text && at_end && ordinary {
        count -= 1;
    }
    if count == 0 {
        return;
    }
    let replacement: String = (0..count)
        .map(|i| if i % 2 == 0 || (at_end && i + 1 == count) { '\u{a0}' } else { ' ' })
        .collect();
    if run != replacement {
        let units = pyro_dom::dom::utf16_len(run);
        let _ = dom.replace_data(node, offset, units, &replacement);
    }
}

/// Do not promote spaces that already collapsed before the edit into NBSPs.
pub(super) fn visible_before_deletion(dom: &Dom, from: Point, to: Point) -> bool {
    let Ok(text) = dom.char_data(from.0) else { return false };
    let Ok((a, b)) = pyro_dom::dom::utf16_range(text, from.1, to.1 - from.1) else { return false };
    let start = text[..a].trim_end_matches(space).len();
    let end = text.len() - text[b..].trim_start_matches(space).len();
    if text[start..end].contains("  ") { return false; }
    if end == text.len() {
        if let Some(right) = dom.next_sibling(from.0).ok().flatten().and_then(|n| dom.char_data(n).ok()) {
            let run = &right[..right.len() - right.trim_start_matches(space).len()];
            if run.contains("  ") || (text.ends_with(' ') && run.starts_with(' ')) { return false; }
        }
    }
    if start == 0 {
        if let Some(left) = dom.prev_sibling(from.0).ok().flatten().and_then(|n| dom.char_data(n).ok()) {
            let run = &left[left.trim_end_matches(space).len()..];
            if run.contains("  ") || (run.ends_with(' ') && text.starts_with(' ')) { return false; }
        }
    }
    true
}

/// Consecutive sibling text nodes form one visual text run. At a shared
/// boundary the selection stays upstream while the downstream text is
/// rebalanced. Preserve node identity on either side of the edit.
pub(super) fn after_text_deletion(page: &PageDom, dom: &mut Dom, host: NodeId, (node, offset): Point, rebalance: bool) -> Option<Point> {
    let prev = dom.prev_sibling(node).ok().flatten().filter(|&n| editing::is_text(dom, n));
    let next = dom.next_sibling(node).ok().flatten().filter(|&n| editing::is_text(dom, n));
    let len = editing::len(dom, node);
    let mut right = next;
    if offset == len {
        while let Some(n) = right {
            if editing::len(dom, n) != 0 { break; }
            right = dom.next_sibling(n).ok().flatten().filter(|&s| editing::is_text(dom, s));
            let _ = dom.remove_node(n);
        }
    }
    let mut left = prev;
    if offset == 0 {
        while let Some(n) = left {
            if editing::len(dom, n) != 0 { break; }
            left = dom.prev_sibling(n).ok().flatten().filter(|&s| editing::is_text(dom, s));
            let _ = dom.remove_node(n);
        }
    }
    let caret = if offset == 0 {
        left.map(|n| (n, editing::len(dom, n))).unwrap_or((node, offset))
    } else {
        (node, offset)
    };
    let point = if offset == len { right.map(|n| (n, 0)).unwrap_or(caret) } else { (node, offset) };
    if rebalance { at(page, dom, host, point, Repair::RemovedBox); }
    if len == 0 && (left.is_some() || right.is_some()) {
        let _ = dom.remove_node(node);
        return if left.is_some() { Some(caret) } else { right.map(|n| (n, 0)) };
    }
    Some(caret)
}

/// A paragraph join can expose a single visible run spanning both blocks.
pub(super) fn after_join(page: &PageDom, dom: &mut Dom, host: NodeId, caret: Point) {
    let left = editing::editable_edge(dom, caret, false);
    let point = if editing::is_text(dom, left.0) { left } else { editing::editable_edge(dom, caret, true) };
    at(page, dom, host, point, Repair::Joined);
}

/// Normalize only the run touching the edit, retaining UTF-16 offsets.
pub(super) fn at(page: &PageDom, dom: &mut Dom, host: NodeId, (node, offset): Point, repair: Repair) {
    if !editing::is_text(dom, node) || editing::editing_host(dom, node) != Some(host) {
        return;
    }
    let Ok(text) = dom.char_data(node) else {
        return;
    };
    let Ok((byte, _)) = pyro_dom::dom::utf16_range(text, offset, 0) else {
        return;
    };
    let left = text[..byte].chars().rev().take_while(|&c| space(c)).count() as u32;
    let right = text[byte..].chars().take_while(|&c| space(c)).count() as u32;
    // Most insertions touch no whitespace. Avoid style/layout queries when
    // neither side can continue the run into another text node.
    if left + right == 0
        && (byte > 0 || !has_sibling(dom, host, node, false))
        && (byte < text.len() || !has_sibling(dom, host, node, true))
    {
        return;
    }
    if !collapses(page, dom, node) {
        return;
    }
    // A removed box exposes only one text node. Keep that boundary even
    // when nowrap chooses the line-edge whitespace sequence.
    let removed_box = matches!(repair, Repair::RemovedBox);
    let repair = if matches!(repair, Repair::Inserted | Repair::RemovedBox) && !wraps(page, dom, node) {
        Repair::Exposed
    } else {
        repair
    };
    let left_char = text[..byte].chars().rev().nth(left as usize);
    let right_char = text[byte..].chars().nth(right as usize);
    let mut parts = Vec::new();
    // Insertion rebalances only the edited text node. Adjacent formatting
    // elements retain their own whitespace and text offsets.
    let local = removed_box || matches!(repair, Repair::Inserted);
    let mut first = left_char.is_none() && (local || extend(page, dom, host, node, false, &mut parts));
    parts.reverse();
    if left + right > 0 {
        parts.push((node, offset - left, left + right));
    }
    let mut last = right_char.is_none() && (local || extend(page, dom, host, node, true, &mut parts));
    if parts.is_empty() {
        return;
    }
    if (left_char == Some('\n') || right_char == Some('\n'))
        && crate::edit_commands::newlines_significant_in(page, dom, node)
    {
        first |= left_char == Some('\n');
        last |= right_char == Some('\n');
    }
    let count = parts.iter().map(|p| p.2 as usize).sum();
    if matches!(repair, Repair::Joined) {
        // Ordinary spaces at an old line edge may already have collapsed.
        // Only rewrite a nonbreaking run whose original width is known.
        if count < 2 || parts.iter().any(|&(n, start, count)| {
            !dom.substring_data(n, start, count).is_ok_and(|s| s.chars().all(|c| c == '\u{a0}'))
        }) {
            return;
        }
    }
    let local_end = right_char.is_none() && !matches!(repair, Repair::Joined);
    let sequence = match repair {
        Repair::Exposed => canonical(count, first, last),
        // Blink rebalances a visible run after editing from left to right.
        // Keep the last space visible at a line end and allow an isolated
        // internal space to wrap.
        Repair::Deleted | Repair::Inserted | Repair::RemovedBox | Repair::Joined => (0..count)
            .map(|i| {
                let nonbreaking = (i % 2 == 0 && (count > 1 || first)) || (i + 1 == count && (last || local_end));
                if nonbreaking { '\u{a0}' } else { ' ' }
            })
            .collect(),
    };
    let mut chars = sequence.chars();
    for (n, start, count) in parts {
        let replacement: String = chars.by_ref().take(count as usize).collect();
        if dom.substring_data(n, start, count).is_ok_and(|s| s != replacement) {
            let _ = dom.replace_data(n, start, count, &replacement);
        }
    }
}

pub(super) fn inserted(page: &PageDom, dom: &mut Dom, host: NodeId, end: Point, text: &str) {
    let start = end.1.saturating_sub(pyro_dom::dom::utf16_len(text));
    let mut offset = start;
    let mut in_space = false;
    for c in text.chars() {
        if offset == start || (space(c) && !in_space) {
            at(page, dom, host, (end.0, offset), Repair::Inserted);
        }
        in_space = space(c);
        offset += c.len_utf16() as u32;
    }
    if !in_space {
        at(page, dom, host, end, Repair::Inserted);
    }
}
