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
                        return if editing::editing_host(dom, n) == Some(host) && collapses(page, dom, n) {
                            Next::Text(n)
                        } else {
                            Next::Content
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

type Segment = (NodeId, u32, u32);

/// Extend the local run through adjacent collapsing Text nodes.
fn extend(page: &PageDom, dom: &Dom, host: NodeId, mut node: NodeId, forward: bool, parts: &mut Vec<Segment>) -> bool {
    loop {
        let n = match adjacent(page, dom, host, node, forward) {
            Next::Boundary | Next::Break => return true,
            Next::Content => return false,
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

/// Deleting across formatting or between characters leaves the author's
/// existing spaces alone. Protect a previously visible space when deleting
/// the non-space character that separated it from a line edge.
pub(super) fn exposes_space(page: &PageDom, dom: &Dom, host: NodeId, point: Point, backward: bool) -> bool {
    let (node, off) = point;
    if !editing::is_text(dom, node) {
        return false;
    }
    let Ok(text) = dom.char_data(node) else {
        return false;
    };
    let Ok((at, _)) = pyro_dom::dom::utf16_range(text, off, 0) else {
        return false;
    };
    let Some(ch) = (if backward { text[..at].chars().next_back() } else { text[at..].chars().next() }) else {
        return false;
    };
    if space(ch) || ch == '\n' {
        return false;
    }
    let cut = if backward { at - ch.len_utf8() } else { at };
    let before = &text[..cut];
    let after = &text[cut + ch.len_utf8()..];
    (after.is_empty()
        && before.chars().next_back().is_some_and(space)
        && (!before.trim_end_matches(space).is_empty()
            || !matches!(adjacent(page, dom, host, node, false), Next::Boundary | Next::Break))
        && matches!(adjacent(page, dom, host, node, true), Next::Boundary | Next::Break))
        || (before.is_empty()
            && after.chars().next().is_some_and(space)
            && (!after.trim_start_matches(space).is_empty()
                || !matches!(adjacent(page, dom, host, node, true), Next::Boundary | Next::Break))
            && matches!(adjacent(page, dom, host, node, false), Next::Boundary | Next::Break))
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

/// Normalize only the run touching the edit, retaining UTF-16 offsets.
pub(super) fn at(page: &PageDom, dom: &mut Dom, host: NodeId, (node, offset): Point) {
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
    let left_char = text[..byte].chars().rev().nth(left as usize);
    let right_char = text[byte..].chars().nth(right as usize);
    let mut parts = Vec::new();
    let mut first = left_char.is_none() && extend(page, dom, host, node, false, &mut parts);
    parts.reverse();
    if left + right > 0 {
        parts.push((node, offset - left, left + right));
    }
    let mut last = right_char.is_none() && extend(page, dom, host, node, true, &mut parts);
    if parts.is_empty() {
        return;
    }
    if (left_char == Some('\n') || right_char == Some('\n'))
        && crate::edit_commands::newlines_significant_in(page, dom, node)
    {
        first |= left_char == Some('\n');
        last |= right_char == Some('\n');
    }
    let sequence = canonical(parts.iter().map(|p| p.2 as usize).sum(), first, last);
    let mut chars = sequence.chars();
    for (n, start, count) in parts {
        let replacement: String = chars.by_ref().take(count as usize).collect();
        if dom.substring_data(n, start, count).is_ok_and(|s| s != replacement) {
            let _ = dom.replace_data(n, start, count, &replacement);
        }
    }
}

pub(super) fn inserted(page: &PageDom, dom: &mut Dom, host: NodeId, end: Point, text: &str) {
    let mut off = end.1.saturating_sub(pyro_dom::dom::utf16_len(text));
    at(page, dom, host, (end.0, off));
    let mut was_space = true; // The first boundary was already visited.
    for c in text.chars() {
        if space(c) && !was_space {
            at(page, dom, host, (end.0, off));
        }
        was_space = space(c);
        off += c.len_utf16() as u32;
    }
    at(page, dom, host, end);
}
