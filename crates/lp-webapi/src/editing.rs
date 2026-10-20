//! The document selection and editing in contenteditable hosts: caret
//! placement, insertText/delete/insertParagraph with beforeinput/input, caret
//! keys and selectionchange. Modelled on headless Chromium (Playwright
//! 1.59.1) for the operations Playwright and ProseMirror-based editors use.
//!
//! The selection is per document (shared by every world's `Selection`
//! object), as in browsers. Boundary points are not adjusted by unrelated DOM
//! mutations (no live-range updates); editing operations set the caret
//! explicitly, and a boundary node that leaves the document clears the
//! selection when read.

use std::rc::Rc;

use lp_dom::{Dom, NodeId, NodeType};

use crate::collections::NodeHold;
use crate::event::{EventCore, EventInterface};
use crate::page::PageDom;
use crate::target::fire;

/// A boundary point: (node, offset in UTF-16 units or child index).
pub type Point = (NodeId, u32);

/// The document selection: anchor and focus.
pub(crate) struct DocSelection {
    anchor: (NodeHold, u32),
    focus: (NodeHold, u32),
}

// ----- selection state ----------------------------------------------------------

fn connected(page: &PageDom, node: NodeId) -> bool {
    let dom = page.dom.borrow();
    dom.is_connected(node).unwrap_or(false) && dom.node_document(node).ok() == Some(page.document())
}

/// (anchor, focus), or None when there is no selection.
pub fn selection(page: &PageDom) -> Option<(Point, Point)> {
    let sel = page.selection.borrow();
    let s = sel.as_ref()?;
    let a = (s.anchor.0.id, s.anchor.1);
    let f = (s.focus.0.id, s.focus.1);
    drop(sel);
    (connected(page, a.0) && connected(page, f.0)).then_some((a, f))
}

/// (start, end) in document order.
pub fn selection_range(page: &PageDom) -> Option<(Point, Point)> {
    let (a, f) = selection(page)?;
    Some(if compare(page, a, f) <= 0 { (a, f) } else { (f, a) })
}

pub fn set_selection(page: &Rc<PageDom>, anchor: Point, focus: Point) {
    let (Ok(a), Ok(f)) = (NodeHold::new(page, anchor.0), NodeHold::new(page, focus.0)) else { return };
    let changed = selection(page) != Some((anchor, focus));
    *page.selection.borrow_mut() = Some(DocSelection { anchor: (a, anchor.1), focus: (f, focus.1) });
    if changed {
        queue_selectionchange(page);
    }
}

pub fn clear_selection(page: &Rc<PageDom>) {
    let had = page.selection.borrow_mut().take().is_some();
    if had {
        queue_selectionchange(page);
    }
}

/// `selectionchange` at the document, coalesced into one pending task.
fn queue_selectionchange(page: &Rc<PageDom>) {
    if page.selectionchange_pending.replace(true) {
        return;
    }
    let p = page.clone();
    let doc = page.document();
    page.post_task(Box::new(move || {
        p.selectionchange_pending.set(false);
        if p.is_disposed() || p.document() != doc {
            return;
        }
        crate::input::fire_simple(&p, doc, "selectionchange", false, false);
    }));
}

/// Boundary point comparison: -1, 0, 1.
pub fn compare(page: &PageDom, a: Point, b: Point) -> i8 {
    crate::range::compare_boundary_points(page, a, b).unwrap_or(0)
}

// ----- editing hosts -------------------------------------------------------------

fn editable_attr(dom: &Dom, el: NodeId) -> Option<bool> {
    let v = dom.get_attribute(el, "contenteditable").ok().flatten()?;
    Some(match v.to_ascii_lowercase().as_str() {
        "" | "true" | "plaintext-only" => true,
        "false" => false,
        _ => return None,
    })
}

/// Whether `node` is editable content (inside a contenteditable element).
pub fn is_editable(dom: &Dom, node: NodeId) -> bool {
    let mut cur = if dom.node_type(node).ok() == Some(NodeType::Element) {
        Some(node)
    } else {
        dom.parent_element(node).ok().flatten()
    };
    while let Some(c) = cur {
        if let Some(v) = editable_attr(dom, c) {
            return v;
        }
        cur = dom.parent_element(c).ok().flatten();
    }
    false
}

/// The editing host of `node`: the outermost editable ancestor-or-self.
pub fn editing_host(dom: &Dom, node: NodeId) -> Option<NodeId> {
    if !is_editable(dom, node) {
        return None;
    }
    let mut host = None;
    let mut cur = if dom.node_type(node).ok() == Some(NodeType::Element) {
        Some(node)
    } else {
        dom.parent_element(node).ok().flatten()
    };
    while let Some(c) = cur {
        if !is_editable(dom, c) {
            break;
        }
        host = Some(c);
        cur = dom.parent_element(c).ok().flatten();
    }
    host
}

fn local(dom: &Dom, n: NodeId) -> String {
    dom.element(n).map(|e| e.local_name().to_string()).unwrap_or_default()
}

fn is_text(dom: &Dom, n: NodeId) -> bool {
    dom.node_type(n).ok() == Some(NodeType::Text)
}

/// Elements a caret descends into (not void/replaced ones).
fn is_container(dom: &Dom, n: NodeId) -> bool {
    dom.node_type(n).ok() == Some(NodeType::Element)
        && !matches!(local(dom, n).as_str(), "br" | "img" | "input" | "hr" | "iframe" | "video" | "canvas" | "svg")
}

const BLOCKS: &[&str] = &[
    "p",
    "div",
    "li",
    "ul",
    "ol",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "blockquote",
    "pre",
    "section",
    "article",
    "header",
    "footer",
    "td",
    "th",
    "dd",
    "dt",
];

fn is_block(dom: &Dom, n: NodeId) -> bool {
    BLOCKS.contains(&local(dom, n).as_str())
}

/// The block containing `node` inside `host` (the host itself when none).
fn block_of(dom: &Dom, node: NodeId, host: NodeId) -> NodeId {
    let mut cur = if is_text(dom, node) { dom.parent(node).ok().flatten() } else { Some(node) };
    while let Some(c) = cur {
        if c == host || is_block(dom, c) {
            return c;
        }
        cur = dom.parent(c).ok().flatten();
    }
    host
}

fn len(dom: &Dom, n: NodeId) -> u32 {
    if is_text(dom, n) {
        dom.char_length(n).unwrap_or(0)
    } else {
        dom.child_count(n).unwrap_or(0)
    }
}

/// A position moved forward into the first leaf at it (text start or
/// before a void element).
fn deep_forward(dom: &Dom, (mut n, mut off): Point) -> Point {
    loop {
        if is_text(dom, n) || off >= dom.child_count(n).unwrap_or(0) {
            return (n, off);
        }
        let Some(c) = dom.child_at(n, off).ok().flatten() else { return (n, off) };
        if is_text(dom, c) {
            return (c, 0);
        }
        if !is_container(dom, c) {
            return (n, off);
        }
        (n, off) = (c, 0);
    }
}

/// A position moved backward into the last leaf before it.
fn deep_backward(dom: &Dom, (mut n, mut off): Point) -> Point {
    loop {
        if is_text(dom, n) || off == 0 {
            return (n, off);
        }
        let Some(c) = dom.child_at(n, off - 1).ok().flatten() else { return (n, off) };
        if is_text(dom, c) {
            return (c, len(dom, c));
        }
        if !is_container(dom, c) {
            return (n, off);
        }
        (n, off) = (c, len(dom, c));
    }
}

pub fn host_start(dom: &Dom, host: NodeId) -> Point {
    deep_forward(dom, (host, 0))
}

pub fn host_end(dom: &Dom, host: NodeId) -> Point {
    deep_backward(dom, (host, len(dom, host)))
}

/// Next/previous text node after/before `n` in tree order inside `within`.
fn adjacent_text(dom: &Dom, n: NodeId, within: NodeId, forward: bool) -> Option<NodeId> {
    let all: Vec<NodeId> = dom.descendants(within).ok()?.filter(|&d| is_text(dom, d)).collect();
    let i = all.iter().position(|&t| t == n);
    match (i, forward) {
        (Some(i), true) => all.get(i + 1).copied(),
        (Some(i), false) => i.checked_sub(1).map(|j| all[j]),
        (None, true) => all.into_iter().find(|&t| crate::range::compare_boundary_points_dom(dom, (t, 0), (n, 0)) > 0),
        (None, false) => {
            all.into_iter().rev().find(|&t| crate::range::compare_boundary_points_dom(dom, (t, 0), (n, 0)) < 0)
        }
    }
}

// ----- events ------------------------------------------------------------------------

fn input_event(
    page: &Rc<PageDom>,
    host: NodeId,
    ty: &str,
    input_type: &str,
    data: Option<&str>,
    cancelable: bool,
) -> bool {
    let Some(key) = page.dom.borrow().node_key(host) else { return true };
    let ev = EventCore::trusted(page, EventInterface::InputEvent, ty, true, cancelable);
    {
        let mut f = ev.fields.borrow_mut();
        f.input_type = input_type.to_owned();
        f.data = data.map(str::to_owned);
    }
    fire(page, key, &ev)
}

/// beforeinput, then the edit, then input (when the edit ran).
fn edit(page: &Rc<PageDom>, host: NodeId, input_type: &str, data: Option<&str>, op: impl FnOnce(&Rc<PageDom>)) {
    let doc = page.document();
    if !input_event(page, host, "beforeinput", input_type, data, true)
        || page.document() != doc
        || !connected(page, host)
    {
        return;
    }
    op(page);
    input_event(page, host, "input", input_type, data, false);
}

/// The current caret/selection inside `host`, defaulting to its start.
fn host_range(page: &PageDom, host: NodeId) -> (Point, Point) {
    let dom = page.dom.borrow();
    match selection_range(page) {
        Some((s, e)) if editing_host(&dom, s.0) == Some(host) && editing_host(&dom, e.0) == Some(host) => (s, e),
        _ => {
            let p = host_start(&dom, host);
            (p, p)
        }
    }
}

// ----- operations ---------------------------------------------------------------------

/// A block left without content gets Chrome's `<br>` placeholder; returns
/// the caret for it.
fn placeholder_if_empty(dom: &mut Dom, block: NodeId) -> Option<Point> {
    let has_content = dom.descendants(block).ok()?.any(|d| {
        d != block
            && (is_text(dom, d) && dom.char_length(d).unwrap_or(0) > 0 || (!is_text(dom, d) && !is_container(dom, d)))
    });
    if has_content {
        return None;
    }
    let kids: Vec<NodeId> = dom.children(block).ok()?.collect();
    for k in kids {
        let _ = dom.remove_child(block, k);
    }
    let doc = dom.node_document(block).ok()?;
    let br = dom.create_element(doc, "br").ok()?;
    dom.append_child(block, br).ok()?;
    Some((block, 0))
}

/// Deletes [start, end) inside `host`, merging the end block into the start
/// block, and returns the caret. With `placeholder`, a block left empty gets
/// `<br>`; text replacing the selection skips it and reuses the text node.
fn delete_range(dom: &mut Dom, host: NodeId, start: Point, end: Point, placeholder: bool) -> Point {
    let start = deep_forward(dom, start);
    let end = deep_backward(dom, end);
    if crate::range::compare_boundary_points_dom(dom, start, end) >= 0 {
        return start;
    }
    let start_block = block_of(dom, start.0, host);
    let end_block = block_of(dom, end.0, host);
    if start.0 == end.0 && is_text(dom, start.0) {
        let _ = dom.replace_data(start.0, start.1, end.1 - start.1, "");
    } else {
        if is_text(dom, start.0) {
            let _ = dom.replace_data(start.0, start.1, u32::MAX, "");
        }
        if is_text(dom, end.0) {
            let _ = dom.replace_data(end.0, 0, end.1, "");
        }
        // Remove everything strictly between the two points.
        let between: Vec<NodeId> = dom
            .descendants(host)
            .map(|d| d.collect::<Vec<_>>())
            .unwrap_or_default()
            .into_iter()
            .filter(|&n| {
                n != host
                    && crate::range::compare_boundary_points_dom(dom, (n, 0), start) > 0
                    && crate::range::compare_boundary_points_dom(dom, (n, len(dom, n)), end) < 0
                    && !dom.contains(n, start.0).unwrap_or(false)
                    && !dom.contains(n, end.0).unwrap_or(false)
            })
            .collect();
        for n in between {
            if let Ok(Some(p)) = dom.parent(n) {
                if dom.is_connected(n).unwrap_or(false) {
                    let _ = dom.remove_child(p, n);
                }
            }
        }
        if start_block != end_block && dom.is_connected(end_block).unwrap_or(false) {
            let kids: Vec<NodeId> = dom.children(end_block).map(|c| c.collect()).unwrap_or_default();
            for k in kids {
                let _ = dom.append_child(start_block, k);
            }
            if let Ok(Some(p)) = dom.parent(end_block) {
                let _ = dom.remove_child(p, end_block);
            }
        }
    }
    if placeholder {
        if let Some(caret) = placeholder_if_empty(dom, start_block) {
            return caret;
        }
    }
    if dom.is_connected(start.0).unwrap_or(false) {
        start
    } else {
        host_start(dom, host)
    }
}

/// Where inserted text goes: an existing text position, else an element
/// position.
fn insertion_point(dom: &Dom, caret: Point) -> Point {
    if is_text(dom, caret.0) {
        return caret;
    }
    let f = deep_forward(dom, caret);
    if is_text(dom, f.0) {
        return f;
    }
    let b = deep_backward(dom, caret);
    if is_text(dom, b.0) {
        return b;
    }
    f
}

fn insert_at(dom: &mut Dom, caret: Point, text: &str) -> Point {
    let (n, off) = insertion_point(dom, caret);
    let units = lp_dom::dom::utf16_len(text);
    if is_text(dom, n) {
        let _ = dom.replace_data(n, off, 0, text);
        return (n, off + units);
    }
    let Ok(doc) = dom.node_document(n) else { return caret };
    let Ok(t) = dom.create_text(doc, text) else { return caret };
    let before = dom.child_at(n, off).ok().flatten();
    // Typing into an empty block replaces its <br> placeholder.
    let placeholder = before.filter(|&b| local(dom, b) == "br" && dom.child_count(n).unwrap_or(0) == 1);
    let _ = dom.insert_before(n, t, before);
    if let Some(br) = placeholder {
        let _ = dom.remove_child(n, br);
    }
    (t, units)
}

/// insertText (typing, `Input.insertText`): replaces the selection.
pub fn insert_text(page: &Rc<PageDom>, host: NodeId, text: &str) {
    let (start, end) = host_range(page, host);
    edit(page, host, "insertText", Some(text), |page| {
        let caret = {
            let mut dom = page.dom.borrow_mut();
            let caret = if start != end { delete_range(&mut dom, host, start, end, false) } else { start };
            insert_at(&mut dom, caret, text)
        };
        set_selection(page, caret, caret);
    });
}

/// Backspace / Delete: deletes the selection, else one character (merging
/// blocks at a block edge).
pub fn delete(page: &Rc<PageDom>, host: NodeId, backward: bool) {
    let (start, end) = host_range(page, host);
    let input_type = if backward { "deleteContentBackward" } else { "deleteContentForward" };
    edit(page, host, input_type, None, |page| {
        let caret = {
            let mut dom = page.dom.borrow_mut();
            if start != end {
                delete_range(&mut dom, host, start, end, true)
            } else {
                delete_char(&mut dom, host, start, backward)
            }
        };
        set_selection(page, caret, caret);
    });
}

fn delete_char(dom: &mut Dom, host: NodeId, caret: Point, backward: bool) -> Point {
    let p = if backward { deep_backward(dom, caret) } else { deep_forward(dom, caret) };
    let p = insertion_point(dom, p);
    if is_text(dom, p.0) {
        let data = dom.char_data(p.0).unwrap_or("").to_owned();
        let units: Vec<u16> = data.encode_utf16().collect();
        let (from, to) = if backward && p.1 > 0 {
            let mut from = p.1 as usize - 1;
            if from > 0 && (0xDC00..0xE000).contains(&units[from]) {
                from -= 1;
            }
            (from as u32, p.1)
        } else if !backward && (p.1 as usize) < units.len() {
            let mut to = p.1 as usize + 1;
            if to < units.len() && (0xDC00..0xE000).contains(&units[to]) {
                to += 1;
            }
            (p.1, to as u32)
        } else {
            // At a text edge: the adjacent text in the same block, else merge blocks.
            let block = block_of(dom, p.0, host);
            if let Some(t) = adjacent_text(dom, p.0, block, !backward) {
                let edge = if backward { (t, len(dom, t)) } else { (t, 0) };
                return delete_char(dom, host, edge, backward);
            }
            return merge_blocks(dom, host, block, backward).unwrap_or(p);
        };
        let _ = dom.replace_data(p.0, from, to - from, "");
        let block = block_of(dom, p.0, host);
        if let Some(c) = placeholder_if_empty(dom, block) {
            return c;
        }
        return (p.0, from);
    }
    let block = block_of(dom, p.0, host);
    merge_blocks(dom, host, block, backward).unwrap_or(p)
}

/// Merges `block` into the previous block (backward) or the next block into
/// it (forward); returns the caret at the seam.
fn merge_blocks(dom: &mut Dom, host: NodeId, block: NodeId, backward: bool) -> Option<Point> {
    if block == host {
        return None;
    }
    let (first, second) = if backward {
        let prev = dom.parent(block).ok()??;
        let idx = dom.index_in_parent(block).ok()?;
        let p = (idx > 0).then(|| dom.child_at(prev, idx - 1).ok().flatten()).flatten()?;
        (p, block)
    } else {
        let parent = dom.parent(block).ok()??;
        let idx = dom.index_in_parent(block).ok()?;
        (block, dom.child_at(parent, idx + 1).ok()??)
    };
    if !is_block(dom, first) || !is_block(dom, second) {
        return None;
    }
    // A lone placeholder in the first block goes away.
    if dom.child_count(first).ok()? == 1 && dom.child_at(first, 0).ok()?.is_some_and(|c| local(dom, c) == "br") {
        let br = dom.child_at(first, 0).ok()??;
        let _ = dom.remove_child(first, br);
    }
    let seam = deep_backward(dom, (first, len(dom, first)));
    let kids: Vec<NodeId> = dom
        .children(second)
        .ok()?
        .filter(|&k| local(dom, k) != "br" || dom.child_count(second).unwrap_or(0) > 1)
        .collect();
    for k in kids {
        let _ = dom.append_child(first, k);
    }
    let parent = dom.parent(second).ok()??;
    let _ = dom.remove_child(parent, second);
    placeholder_if_empty(dom, first).or(Some(seam))
}

/// Enter: splits the caret's block (insertParagraph).
pub fn insert_paragraph(page: &Rc<PageDom>, host: NodeId) {
    let (start, end) = host_range(page, host);
    edit(page, host, "insertParagraph", None, |page| {
        let caret = {
            let mut dom = page.dom.borrow_mut();
            let caret = if start != end { delete_range(&mut dom, host, start, end, true) } else { start };
            split_block(&mut dom, host, caret)
        };
        set_selection(page, caret, caret);
    });
}

fn split_block(dom: &mut Dom, host: NodeId, caret: Point) -> Point {
    let caret = insertion_point(dom, caret);
    let block = block_of(dom, caret.0, host);
    let Ok(doc) = dom.node_document(host) else { return caret };
    if block == host {
        // No block to split: a line break.
        let Ok(br) = dom.create_element(doc, "br") else { return caret };
        let (n, off) = caret;
        if is_text(dom, n) {
            let Ok(after) = dom.split_text(n, off) else { return caret };
            if let Ok(Some(p)) = dom.parent(after) {
                let _ = dom.insert_before(p, br, Some(after));
            }
            return (after, 0);
        }
        let before = dom.child_at(n, off).ok().flatten();
        let _ = dom.insert_before(n, br, before);
        return (n, off + 1);
    }
    let tag = local(dom, block);
    let Ok(new_block) = dom.create_element(doc, &tag) else { return caret };
    // Move everything after the caret (within the block) into the new block.
    let (n, off) = caret;
    let mut split_from = if is_text(dom, n) {
        let n_len = len(dom, n);
        if off >= n_len {
            dom.next_sibling(n).ok().flatten()
        } else if off == 0 {
            Some(n)
        } else {
            dom.split_text(n, off).ok()
        }
    } else {
        dom.child_at(n, off).ok().flatten()
    };
    // Climb to the block's direct child, carrying following siblings.
    let mut anchor_parent = if is_text(dom, n) { dom.parent(n).ok().flatten() } else { Some(n) };
    let mut moved: Vec<NodeId> = Vec::new();
    while let Some(p) = anchor_parent {
        let mut cur = split_from;
        while let Some(c) = cur {
            let next = dom.next_sibling(c).ok().flatten();
            moved.push(c);
            cur = next;
        }
        if p == block {
            break;
        }
        split_from = dom.next_sibling(p).ok().flatten();
        anchor_parent = dom.parent(p).ok().flatten();
    }
    for m in moved {
        let _ = dom.append_child(new_block, m);
    }
    let parent = dom.parent(block).ok().flatten().unwrap_or(host);
    let next = dom.next_sibling(block).ok().flatten();
    let _ = dom.insert_before(parent, new_block, next);
    placeholder_if_empty(dom, block);
    placeholder_if_empty(dom, new_block).unwrap_or_else(|| host_start(dom, new_block))
}

/// Caret keys inside a host: ArrowLeft/Right by one character, Home/End to
/// the block edges; Shift extends.
pub fn move_caret(page: &Rc<PageDom>, host: NodeId, key: &str, extend: bool) {
    let Some((anchor, focus)) = selection(page) else {
        let p = host_start(&page.dom.borrow(), host);
        set_selection(page, p, p);
        return;
    };
    let (start, end) = selection_range(page).unwrap_or((anchor, focus));
    let target = {
        let dom = page.dom.borrow();
        match key {
            "ArrowLeft" if !extend && start != end => start,
            "ArrowRight" if !extend && start != end => end,
            "ArrowLeft" | "ArrowRight" => step(&dom, host, focus, key == "ArrowRight"),
            "Home" => {
                let b = block_of(&dom, focus.0, host);
                deep_forward(&dom, (b, 0))
            }
            _ => {
                let b = block_of(&dom, focus.0, host);
                deep_backward(&dom, (b, len(&dom, b)))
            }
        }
    };
    if extend {
        set_selection(page, anchor, target);
    } else {
        set_selection(page, target, target);
    }
}

fn step(dom: &Dom, host: NodeId, p: Point, forward: bool) -> Point {
    let p = insertion_point(dom, if forward { deep_forward(dom, p) } else { deep_backward(dom, p) });
    if !is_text(dom, p.0) {
        return p;
    }
    let data: Vec<u16> = dom.char_data(p.0).unwrap_or("").encode_utf16().collect();
    let n = data.len() as u32;
    if forward && p.1 < n {
        let mut o = p.1 + 1;
        if o < n && (0xDC00..0xE000).contains(&data[o as usize]) {
            o += 1;
        }
        return (p.0, o);
    }
    if !forward && p.1 > 0 {
        let mut o = p.1 - 1;
        if o > 0 && (0xDC00..0xE000).contains(&data[o as usize]) {
            o -= 1;
        }
        return (p.0, o);
    }
    // Into the adjacent text node (crossing a block edge counts as a step).
    match adjacent_text(dom, p.0, host, forward) {
        Some(t) if block_of(dom, t, host) == block_of(dom, p.0, host) => {
            step(dom, host, if forward { (t, 0) } else { (t, len(dom, t)) }, forward)
        }
        Some(t) => {
            if forward {
                (t, 0)
            } else {
                (t, len(dom, t))
            }
        }
        None => p,
    }
}

/// Select all inside the host (Ctrl/Cmd+A).
pub fn select_all(page: &Rc<PageDom>, host: NodeId) {
    let (s, e) = {
        let dom = page.dom.borrow();
        (host_start(&dom, host), host_end(&dom, host))
    };
    set_selection(page, s, e);
}

/// Focusing a host without the selection inside puts the caret at its start.
pub fn caret_on_focus(page: &Rc<PageDom>, el: NodeId) {
    let host = {
        let dom = page.dom.borrow();
        match editing_host(&dom, el) {
            Some(h) if h == el => h,
            _ => return,
        }
    };
    let inside = selection(page).is_some_and(|(a, _)| editing_host(&page.dom.borrow(), a.0) == Some(host));
    if !inside {
        let p = host_start(&page.dom.borrow(), host);
        set_selection(page, p, p);
    }
}

/// A primary press on editable content puts the caret at the end of the
/// pressed element's content (the click point within text is not modelled).
pub fn caret_on_press(page: &Rc<PageDom>, target: NodeId) {
    let p = {
        let dom = page.dom.borrow();
        if editing_host(&dom, target).is_none() {
            return;
        }
        deep_backward(&dom, (target, len(&dom, target)))
    };
    set_selection(page, p, p);
}
