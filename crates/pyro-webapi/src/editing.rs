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

mod character;
mod paragraph_join;
mod target_range;
mod whitespace;

use std::rc::Rc;

use pyro_dom::{Dom, NodeId, NodeType};

use crate::event::{EventCore, EventInterface};
use crate::page::PageDom;
use crate::target::fire;

/// A boundary point: (node, offset in UTF-16 units or child index).
pub type Point = (NodeId, u32);

/// The document selection: its range (shared with the Range objects script
/// holds for it) and its direction.
pub(crate) struct DocSelection {
    pub(crate) range: Rc<crate::range::RangeOwner>,
    /// The focus is the range's start.
    backward: bool,
    /// The Range object each realm sees as `getRangeAt(0)`.
    pub(crate) wrappers: Vec<(pyro_js::RealmId, pyro_js::Persistent)>,
    /// The nodes a selection made by a gesture, by typing or by
    /// `collapse()`/`extend()` was set on. As measured in Chromium, such a
    /// selection is gone once one of them leaves the document; a range
    /// script added follows its live boundary points instead.
    origin: Option<(crate::collections::NodeHold, crate::collections::NodeHold)>,
    /// Separate live endpoints for shadow selections, which can span trees
    /// and follow removal of a shadow host. The exposed Range stays in one tree.
    composed: Option<Rc<crate::range::RangeOwner>>,
    cross_tree: bool,
}

// ----- selection state ----------------------------------------------------------

pub(crate) fn connected(page: &PageDom, node: NodeId) -> bool {
    let dom = page.dom.borrow();
    dom.is_connected(node).unwrap_or(false) && dom.node_document(node).ok() == Some(page.document())
}

/// (anchor, focus), or None when there is no selection.
pub fn selection(page: &PageDom) -> Option<(Point, Point)> {
    let sel = page.selection.borrow();
    let s = sel.as_ref()?;
    let (start, end) = (s.range.live.start(), s.range.live.end());
    let (a, f) = if s.backward { (end, start) } else { (start, end) };
    let origin = s.origin.as_ref().map(|(a, f)| (a.id, f.id));
    let cross_tree = s.composed.as_ref().filter(|_| s.cross_tree).map(|r| (r.live.start(), r.live.end()));
    drop(sel);
    if origin.is_some_and(|(a, f)| !connected(page, a) || !connected(page, f)) {
        return None;
    }
    if let Some(points) = cross_tree {
        return Some(points);
    }
    (connected(page, a.0) && connected(page, f.0)).then_some((a, f))
}

/// Composed snapshots follow live boundary adjustments even when a gesture
/// selection's original node was removed and its legacy getters are empty.
pub(crate) fn composed_points(page: &PageDom) -> Option<(Point, Point)> {
    let selection = page.selection.borrow();
    let sel = selection.as_ref()?;
    let range = &sel.range.live;
    let mut points = (range.start(), range.end());
    if let Some(shadow) = &sel.composed {
        if sel.cross_tree || !connected(page, points.0.0) || !connected(page, points.1.0) {
            points = (shadow.live.start(), shadow.live.end());
        }
    }
    (connected(page, points.0.0) && connected(page, points.1.0)).then_some(points)
}

/// (start, end) in document order.
pub fn selection_range(page: &PageDom) -> Option<(Point, Point)> {
    let (a, f) = selection(page)?;
    Some(if compare(page, a, f) <= 0 { (a, f) } else { (f, a) })
}

/// Selects from `anchor` to `focus` with a new range (as `collapse()` and
/// `extend()` do).
pub fn set_selection(page: &Rc<PageDom>, anchor: Point, focus: Point) {
    let changed = selection(page) != Some((anchor, focus));
    let same_tree = {
        let dom = page.dom.borrow();
        dom.root_node(anchor.0, false).ok() == dom.root_node(focus.0, false).ok()
    };
    let backward = same_tree && compare(page, anchor, focus) > 0;
    let (start, end) = match (same_tree, backward) {
        (false, _) => (focus, focus),
        (true, true) => (focus, anchor),
        (true, false) => (anchor, focus),
    };
    let cross_tree = !same_tree;
    let in_shadow = cross_tree || page.dom.borrow().root_node(anchor.0, false).ok() != Some(page.document());
    let composed = if in_shadow {
        let Ok(range) = crate::range::RangeOwner::new(page, anchor, focus) else { return };
        range.live.track_shadow_host_removal();
        Some(range)
    } else {
        None
    };
    let Ok(range) = crate::range::RangeOwner::new(page, start, end) else { return };
    let origin =
        match (crate::collections::NodeHold::new(page, anchor.0), crate::collections::NodeHold::new(page, focus.0)) {
            (Ok(a), Ok(f)) => Some((a, f)),
            _ => None,
        };
    let old = page.selection.replace(Some(DocSelection { range, backward, wrappers: Vec::new(), origin, composed, cross_tree }));
    drop(old);
    if changed {
        page.typing_values.take();
        queue_selectionchange(page);
    }
}

/// Retarget the selection after an edit while maintaining its associated
/// live Range and any Range objects already held by script.
pub(crate) fn retarget_selection(page:&Rc<PageDom>, anchor:Point, focus:Point) {
    let previous=selection(page);
    let backward=compare(page,anchor,focus)>0;
    let (start,end)=if backward{(focus,anchor)}else{(anchor,focus)};
    let mut slot=page.selection.borrow_mut();
    let Some(current)=slot.as_mut() else{return};
    if page.dom.borrow_mut().set_live_range(&current.range.live,start,end).is_err(){return;}
    current.backward=backward;
    current.cross_tree=false;
    current.composed=None;
    if current.origin.is_some() {
        current.origin=crate::collections::NodeHold::new(page,anchor.0).ok()
            .zip(crate::collections::NodeHold::new(page,focus.0).ok());
    }
    drop(slot);
    if previous!=Some((anchor,focus)){queue_selectionchange(page);}
}

/// `addRange()`: `range` becomes the selection's range, `wrapper` being the
/// Range object script passed in its realm.
pub(crate) fn set_selection_range(
    page: &Rc<PageDom>,
    range: Rc<crate::range::RangeOwner>,
    wrapper: Option<(pyro_js::RealmId, pyro_js::Persistent)>,
) {
    let selection = DocSelection {
        range,
        backward: false,
        wrappers: wrapper.into_iter().collect(),
        origin: None,
        cross_tree: false,
        composed: None,
    };
    page.typing_values.take();
    let old = page.selection.replace(Some(selection));
    drop(old);
    queue_selectionchange(page);
}

pub fn clear_selection(page: &Rc<PageDom>) {
    page.typing_values.take();
    let old = page.selection.take();
    if old.is_some() {
        drop(old);
        queue_selectionchange(page);
    }
}

/// `selectionchange` at the document, coalesced into one pending task.
pub(crate) fn queue_selectionchange(page: &Rc<PageDom>) {
    queue_selectionchange_at(page, None);
}

/// A text control's selectionchange bubbles from the control, even if
/// several controls changed during the same task.
pub(crate) fn queue_control_selectionchange(page: &Rc<PageDom>, control: NodeId) {
    // Chromium reports changes inside a shadow tree on the document.
    let in_shadow = {
        let dom = page.dom.borrow();
        dom.shadow_root_count() != 0 && dom.root_node(control, false).and_then(|root| dom.is_shadow_root(root)).unwrap_or(false)
    };
    queue_selectionchange_at(page, (!in_shadow).then_some(control));
}

fn queue_selectionchange_at(page: &Rc<PageDom>, target: Option<NodeId>) {
    let doc = page.document();
    let at = target.unwrap_or(doc);
    if !page.selectionchange_pending.borrow_mut().insert(at) {
        return;
    }
    let p = page.clone();
    page.post_task(Box::new(move || {
        p.selectionchange_pending.borrow_mut().remove(&at);
        if !p.is_disposed() && p.document() == doc {
            crate::input::fire_simple(&p, at, "selectionchange", target.is_some(), false);
        }
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
    in_design_mode_content(dom, node)
}

/// Whether `node` is content a designMode document makes editable: the document element and
/// what is in it, in the document's own tree (not a shadow tree).
pub fn in_design_mode_content(dom: &Dom, node: NodeId) -> bool {
    let Ok(doc) = dom.node_document(node) else { return false };
    if !dom.design_mode(doc) {
        return false;
    }
    dom.document_element(doc).ok().flatten().is_some_and(|root| dom.contains(root, node).unwrap_or(false))
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
        // In a designMode document the body is the editing host of its content, as in browsers
        // (HTML names the document element; Chromium and Gecko edit inside the body and never
        // remove it): `delete` after `selectAll` empties the body, it does not remove it.
        if editable_attr(dom, c).is_none()
            && dom.node_document(c).is_ok_and(|doc| dom.design_mode(doc) && dom.body(doc).ok().flatten() == Some(c))
        {
            break;
        }
        cur = dom.parent_element(c).ok().flatten();
    }
    host
}

pub(crate) fn local(dom: &Dom, n: NodeId) -> String {
    dom.element(n).map(|e| e.local_name().to_string()).unwrap_or_default()
}

pub(crate) fn is_text(dom: &Dom, n: NodeId) -> bool {
    matches!(dom.node_type(n), Ok(NodeType::Text | NodeType::CDataSection))
}

/// Elements a caret descends into (not void/replaced ones).
pub(crate) fn is_container(dom: &Dom, n: NodeId) -> bool {
    dom.element(n).is_ok_and(|el| {
        !(el.is_html() && pyro_dom::serialize::is_void(el.local_name()))
            && !matches!(el.local_name().as_ref(), "iframe" | "video" | "canvas" | "svg")
    })
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

pub(crate) fn is_block(dom: &Dom, n: NodeId) -> bool {
    BLOCKS.contains(&local(dom, n).as_str())
}

/// The block containing `node` inside `host` (the host itself when none).
pub(crate) fn block_of(dom: &Dom, node: NodeId, host: NodeId) -> NodeId {
    let mut cur = if is_text(dom, node) { dom.parent(node).ok().flatten() } else { Some(node) };
    while let Some(c) = cur {
        if c == host || is_block(dom, c) {
            return c;
        }
        cur = dom.parent(c).ok().flatten();
    }
    host
}

pub(crate) fn len(dom: &Dom, n: NodeId) -> u32 {
    if is_text(dom, n) {
        dom.char_length(n).unwrap_or(0)
    } else {
        dom.child_count(n).unwrap_or(0)
    }
}

/// A position moved forward into the first leaf at it (text start or
/// before a void element).
pub(crate) fn deep_forward(dom: &Dom, (mut n, mut off): Point) -> Point {
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
pub(crate) fn deep_backward(dom: &Dom, (mut n, mut off): Point) -> Point {
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

/// Descend into a nearby leaf only while each edge stays in the same
/// editing host. A non-editable child is an atomic boundary for typing.
fn editable_edge(dom: &Dom, mut point: Point, forward: bool) -> Point {
    let host = editing_host(dom, point.0);
    while !is_text(dom, point.0) {
        let index = if forward { Some(point.1) } else { point.1.checked_sub(1) };
        let Some(child) = index.and_then(|i| dom.child_at(point.0, i).ok().flatten()) else { break };
        if (!is_text(dom, child) && !is_container(dom, child)) || editing_host(dom, child) != host {
            break;
        }
        point = (child, if forward { 0 } else { len(dom, child) });
    }
    point
}

pub fn host_start(dom: &Dom, host: NodeId) -> Point {
    editable_edge(dom, (host, 0), true)
}

pub fn host_end(dom: &Dom, host: NodeId) -> Point {
    editable_edge(dom, (host, len(dom, host)), false)
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

pub(crate) fn input_event(
    page: &Rc<PageDom>,
    host: NodeId,
    ty: &str,
    input_type: &str,
    data: Option<&str>,
    cancelable: bool,
) -> bool {
    input_event_with(page, host, ty, input_type, data, None, cancelable)
}

/// An InputEvent, with `InputEvent.dataTransfer` (paste and insertHTML in
/// editable content).
pub(crate) fn input_event_with(
    page: &Rc<PageDom>,
    host: NodeId,
    ty: &str,
    input_type: &str,
    data: Option<&str>,
    transfer: Option<&crate::data_transfer::NativeTransfer>,
    cancelable: bool,
) -> bool {
    let Some(key) = page.dom.borrow().node_key(host) else { return true };
    let ev = EventCore::trusted(page, EventInterface::InputEvent, ty, true, cancelable);
    {
        let mut f = ev.fields.borrow_mut();
        if ty == "beforeinput" && (page.dom.borrow().listens_for(ty) || page.dom.borrow().has_pending_inline_handlers()) {
            f.editing_range = target_range::for_input(page, host, input_type).map(|range| Box::new((key, range)));
        }
        f.input_type = input_type.to_owned();
        f.data = data.map(str::to_owned);
        f.data_transfer = transfer.map(|t| t.reference.clone());
    }
    let proceed = fire(page, key, &ev);
    ev.fields.borrow_mut().editing_range.take();
    proceed
}

/// beforeinput, then the edit, then input (when the edit ran).
fn edit(page: &Rc<PageDom>, host: NodeId, input_type: &str, data: Option<&str>, op: impl FnOnce(&Rc<PageDom>)) {
    edit_with(page, host, input_type, data, None, op);
}

/// [`edit`] with `InputEvent.dataTransfer` on both events. The host's state
/// before the edit becomes an undo step (`crate::edit_commands`).
pub(crate) fn edit_with(
    page: &Rc<PageDom>,
    host: NodeId,
    input_type: &str,
    data: Option<&str>,
    transfer: Option<&crate::data_transfer::NativeTransfer>,
    op: impl FnOnce(&Rc<PageDom>),
) {
    if before_edit(page, host, input_type, data, transfer) {
        apply_edit(page, host, input_type, data, transfer, op);
    }
}

fn before_edit(
    page: &Rc<PageDom>,
    host: NodeId,
    input_type: &str,
    data: Option<&str>,
    transfer: Option<&crate::data_transfer::NativeTransfer>,
) -> bool {
    let doc = page.document();
    input_event_with(page, host, "beforeinput", input_type, data, transfer, true)
        && page.document() == doc
        && connected(page, host)
}

fn apply_edit(
    page: &Rc<PageDom>,
    host: NodeId,
    input_type: &str,
    data: Option<&str>,
    transfer: Option<&crate::data_transfer::NativeTransfer>,
    op: impl FnOnce(&Rc<PageDom>),
) {
    crate::edit_commands::record_host_edit(page, host, input_type);
    op(page);
    crate::edit_commands::note_host_edited(page, host);
    input_event_with(page, host, "input", input_type, data, transfer, false);
}

/// The current caret/selection inside `host`, defaulting to its start.
pub(crate) fn host_range(page: &PageDom, host: NodeId) -> (Point, Point) {
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
pub(crate) fn placeholder_if_empty(dom: &mut Dom, block: NodeId) -> Option<Point> {
    let has_content = dom.descendants(block).ok()?.any(|d| {
        d != block
            && (is_text(dom, d) && dom.char_length(d).unwrap_or(0) > 0
                || (!is_text(dom, d) && (!is_container(dom, d) || editable_attr(dom, d) == Some(false))))
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
pub(crate) fn delete_range(dom: &mut Dom, host: NodeId, start: Point, end: Point, placeholder: bool) -> Point {
    delete_range_with_merging(dom, host, start, end, placeholder, true)
}

fn delete_range_with_merging(
    dom: &mut Dom,
    host: NodeId,
    start: Point,
    end: Point,
    placeholder: bool,
    merge_blocks: bool,
) -> Point {
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
        if merge_blocks && start_block != end_block && dom.is_connected(end_block).unwrap_or(false) {
            if dom.contains(end_block, start_block).unwrap_or(false) {
                // The end lies in an anonymous line of an ancestor block
                // (possibly the editing host). Join that line to the start
                // block, leaving the ancestor and subsequent lines intact.
                let mut branch = start_block;
                while let Ok(Some(parent)) = dom.parent(branch) {
                    if parent == end_block { break; }
                    branch = parent;
                }
                let mut next = dom.next_sibling(branch).ok().flatten();
                while let Some(child) = next {
                    if is_block(dom, child) || !is_editable(dom, child) { break; }
                    if local(dom, child) == "br" {
                        let _ = dom.remove_child(end_block, child);
                        break;
                    }
                    next = dom.next_sibling(child).ok().flatten();
                    if dom.append_child(start_block, child).is_err() { break; }
                }
            } else {
                let kids: Vec<NodeId> = dom.children(end_block).map(|c| c.collect()).unwrap_or_default();
                for k in kids {
                    let _ = dom.append_child(start_block, k);
                }
                if let Ok(Some(p)) = dom.parent(end_block) {
                    let _ = dom.remove_child(p, end_block);
                }
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
pub(crate) fn insertion_point(dom: &Dom, caret: Point) -> Point {
    if is_text(dom, caret.0) {
        return caret;
    }
    let forward = editable_edge(dom, caret, true);
    if is_text(dom, forward.0) {
        return forward;
    }
    let backward = editable_edge(dom, caret, false);
    if is_text(dom, backward.0) { backward } else { forward }
}

pub(crate) fn insert_at(dom: &mut Dom, caret: Point, text: &str) -> Point {
    let point = insertion_point(dom, caret);
    insert_at_point(dom, point, text)
}

fn insert_at_point(dom: &mut Dom, (n, off): Point, text: &str) -> Point {
    let units = pyro_dom::dom::utf16_len(text);
    if is_text(dom, n) {
        let _ = dom.replace_data(n, off, 0, text);
        return (n, off + units);
    }
    let Ok(doc) = dom.node_document(n) else { return (n, off) };
    let Ok(t) = dom.create_text(doc, text) else { return (n, off) };
    let before = dom.child_at(n, off).ok().flatten();
    // Typing into an empty block replaces its <br> placeholder, as does
    // typing on the empty last line after a line break (`a<br>[]<br>`).
    let placeholder = before.filter(|&b| {
        local(dom, b) == "br"
            && dom.next_sibling(b).ok().flatten().is_none()
            && (dom.child_count(n).unwrap_or(0) == 1
                || (off > 0 && dom.child_at(n, off - 1).ok().flatten().is_some_and(|p| local(dom, p) == "br")))
    });
    let _ = dom.insert_before(n, t, before);
    if let Some(br) = placeholder {
        let _ = dom.remove_child(n, br);
    }
    (t, units)
}

/// A DOM selection can be inside a void element. Place typed text at its
/// editable parent boundary, retaining preceding inline formatting if possible.
fn typing_point(page: &PageDom, dom: &Dom, host: NodeId, caret: Point) -> Option<Point> {
    let Ok(element) = dom.element(caret.0) else { return Some(insertion_point(dom, caret)) };
    if !element.is_html() || !pyro_dom::serialize::is_void(element.local_name()) {
        return Some(insertion_point(dom, caret));
    }
    if caret.0 == host {
        return None;
    }
    let parent = dom.parent(caret.0).ok()??;
    let index = dom.index_in_parent(caret.0).ok()?;
    let boundary = (parent, index);
    let layout = page.layout();
    if layout.computed_style_value(dom, caret.0, "display") == "block" {
        return Some(boundary);
    }

    // Empty editable inline boxes between the caret and preceding text do
    // not interrupt its formatting. Stop before other hosts or inline boxes
    // that reserve space, such as inline-blocks.
    let mut previous = dom.prev_sibling(caret.0).ok().flatten();
    while let Some(node) = previous {
        if editing_host(dom, node) != Some(host) {
            break;
        }
        let end = editable_edge(dom, (node, len(dom, node)), false);
        if is_text(dom, end.0) {
            return Some(end);
        }
        if dom.child_count(node).ok() != Some(0) || !is_container(dom, node)
            || dom.shadow_root(node).ok().flatten().is_some()
            || layout.computed_style_value(dom, node, "display") != "inline"
        {
            break;
        }
        previous = dom.prev_sibling(node).ok().flatten();
    }
    Some(boundary)
}

/// insertText (typing, `Input.insertText`): replaces the selection.
pub fn insert_text(page: &Rc<PageDom>, host: NodeId, text: &str) {
    let (start, end) = host_range(page, host);
    edit(page, host, "insertText", Some(text), |page| {
        let caret = {
            let mut dom = page.dom.borrow_mut();
            let caret = if start != end { delete_range(&mut dom, host, start, end, false) } else { start };
            let mut caret = caret;
            if text.starts_with('\n') && dom.element(caret.0).is_ok_and(|el| el.is_html() && pyro_dom::serialize::is_void(el.local_name())) {
                let Some(point) = typing_point(page, &dom, host, caret) else { return };
                caret = point;
            }
            for (index, part) in text.split('\n').enumerate() {
                if index != 0 {
                    // An inserted linefeed performs insertParagraph within
                    // the same edit and undo step as the surrounding text.
                    caret = split_block(page, &mut dom, host, caret);
                }
                if !part.is_empty() {
                    let Some(point) = typing_point(page, &dom, host, caret) else { return };
                    let inserted_end = insert_at_point(&mut dom, point, part);
                    whitespace::inserted(page, &mut dom, host, inserted_end, part);
                    caret = crate::edit_commands::inline_value::format_typed(
                        page,
                        &mut dom,
                        (start, end),
                        inserted_end,
                        part,
                    )
                    .unwrap_or(inserted_end);
                }
            }
            caret
        };
        set_selection(page, caret, caret);
    });
}

/// `document.execCommand(command, showUI, value)`. Only `insertText` is
/// supported: it types `value` into the focused text control or the
/// contenteditable host of the focused element (else of the selection), with
/// `beforeinput`/`input` and no `textInput` (Chromium fires none for
/// execCommand, and uievents/textInput/api.html expects none). Returns
/// whether the command was handled.
pub(crate) fn exec_command(page: &Rc<PageDom>, command: &str, value: &str) -> bool {
    if !command.eq_ignore_ascii_case("inserttext") {
        return false;
    }
    let doc = page.document();
    let focused = page.dom.borrow().focused(doc).ok().flatten();
    // The focused element's host or text control comes first; otherwise the
    // document selection may still sit in a host (focus on a checkbox does
    // not clear it, as in Chromium).
    let (host, control) = {
        let dom = page.dom.borrow();
        match focused {
            Some(f) if is_text_entry_control(&dom, f) => (None, Some(f)),
            Some(f) if editing_host(&dom, f).is_some() => (editing_host(&dom, f), None),
            _ => (selection_range(page).and_then(|(start, _)| editing_host(&dom, start.0)), None),
        }
    };
    if let Some(host) = host {
        insert_text(page, host, value);
        return true;
    }
    let Some(control) = control else { return false };
    if !input_event(page, control, "beforeinput", "insertText", Some(value), true)
        || page.document() != doc
        || !connected(page, control)
    {
        return true;
    }
    crate::edit_commands::record_control_edit(page, control, "insertText");
    let changed = page.dom.borrow_mut().insert_text(control, value).unwrap_or(false);
    crate::edit_commands::note_control_edited(page, control);
    if changed {
        queue_control_selectionchange(page, control);
        input_event(page, control, "input", "insertText", Some(value), false);
    }
    true
}

/// A textarea or an input whose type takes typed text.
pub(crate) fn is_text_entry_control(dom: &Dom, el: NodeId) -> bool {
    use pyro_dom::forms::InputType;
    match local(dom, el).as_str() {
        "textarea" => true,
        "input" => !matches!(
            dom.input_type(el),
            Ok(InputType::Checkbox
                | InputType::Radio
                | InputType::File
                | InputType::Submit
                | InputType::Reset
                | InputType::Button
                | InputType::Image
                | InputType::Hidden)
        ),
        _ => false,
    }
}

/// Backspace / Delete: deletes the selection, else one character (merging
/// blocks at a block edge).
pub fn delete(page: &Rc<PageDom>, host: NodeId, backward: bool) {
    let input_type = if backward { "deleteContentBackward" } else { "deleteContentForward" };
    let original_selection = selection_range(page);
    let original_owner = original_selection.and_then(|(s, e)| selection_host(&page.dom.borrow(), s, e));
    if !before_edit(page, host, input_type, None, None) { return; }
    // A moved selection may choose another host. When the listeners leave
    // it alone, retain the command's editing host (including designMode's
    // document-body editing scope).
    let Some((start, end)) = selection_range(page) else { return };
    let current_owner = selection_host(&page.dom.borrow(), start, end);
    let (host, start, end) = if original_selection == Some((start, end)) && original_owner == current_owner {
        let (start, end) = host_range(page, host);
        (host, start, end)
    } else {
        let Some(host) = current_owner else { return };
        (host, start, end)
    };
    let (start, end, span) = if start == end {
        let (caret, span) = target_range::deletion_plan(page, &page.dom.borrow(), host, start, backward);
        (caret, caret, span)
    } else {
        (start, end, Some((start, end)))
    };
    let Some((from, to)) = span else {
        return;
    };
    if from == to {
        return;
    }
    apply_edit(page, host, input_type, None, None, |page| {
        let caret = {
            let mut dom = page.dom.borrow_mut();
            let repair = if start == end { whitespace::deletion_repair(page, &dom, host, start, backward) } else { None };
            let caret = if let Some(caret) = paragraph_join::at_boundary(page, &mut dom, host, start, end, backward) {
                caret
            } else {
                delete_content(page, &mut dom, host, start, end, backward, (from, to))
            };
            if let Some(repair) = repair {
                whitespace::at(page, &mut dom, host, caret, repair);
            }
            caret
        };
        set_selection(page, caret, caret);
    });
}

fn selection_host(dom: &Dom, start: Point, end: Point) -> Option<NodeId> {
    let host = editing_host(dom, start.0)?;
    (editing_host(dom, end.0) == Some(host)).then_some(host)
}

fn delete_content(
    page: &PageDom,
    dom: &mut Dom,
    host: NodeId,
    start: Point,
    end: Point,
    backward: bool,
    (from, to): (Point, Point),
) -> Point {
    if start == end
        && backward
        && dom.parent(to.0).ok().flatten() == Some(from.0)
        && dom.index_in_parent(to.0).ok() == Some(from.1)
        && to.1 == len(dom, to.0)
        && to.0 != host
        && placeholder_block(dom, to.0)
    {
        let _ = dom.remove_child(from.0, to.0);
        return from;
    }
    if start == end && from.0 == to.0 && is_text(dom, from.0) {
        return remove_character(dom, host, from, to);
    }
    if let Some(right_list) = adjacent_list_boundary(dom, host, from, to) {
        // Join the two list items across the paragraph boundary. Other
        // items keep their original list; remove an exhausted list shell.
        let caret = delete_range(dom, host, from, to, true);
        if dom.child_count(right_list).ok() == Some(0) {
            let _ = dom.remove_node(right_list);
        }
        return caret;
    }
    if start == end && from.0 == to.0 && !is_text(dom, from.0) && to.1 == from.1 + 1 {
        if let Ok(Some(child)) = dom.child_at(from.0, from.1) {
            if !is_container(dom, child) || !is_editable(dom, child) {
                let _ = dom.remove_child(from.0, child);
                return placeholder_if_empty(dom, block_of(dom, from.0, host)).unwrap_or(from);
            }
        }
    }
    if start != end {
        delete_range(dom, host, start, end, true)
    } else {
        merge_blocks(page, dom, host, from, to).unwrap_or(start)
    }
}

fn adjacent_list_boundary(dom: &Dom, host: NodeId, from: Point, to: Point) -> Option<NodeId> {
    let left = block_of(dom, from.0, host);
    let right = block_of(dom, to.0, host);
    if left == right || ![left, right].into_iter().all(|n| matches!(local(dom, n).as_str(), "dt" | "dd" | "li")) {
        return None;
    }
    let a = dom.parent(left).ok()??;
    let b = dom.parent(right).ok()??;
    if a == b || dom.next_sibling(a).ok()?? != b {
        return None;
    }
    [a, b].into_iter().all(|n| n != host && matches!(local(dom, n).as_str(), "dl" | "ul" | "ol")).then_some(b)
}

fn remove_character(dom: &mut Dom, host: NodeId, from: Point, to: Point) -> Point {
    let _ = dom.replace_data(from.0, from.1, to.1 - from.1, "");
    let block = block_of(dom, from.0, host);
    placeholder_if_empty(dom, block).unwrap_or(from)
}

fn placeholder_block(dom: &Dom, n: NodeId) -> bool {
    let Some(host) = editing_host(dom, n) else { return false };
    is_block(dom, n) && dom.child_count(n).ok() == Some(1)
        && dom.first_child(n).ok().flatten().is_some_and(|br| local(dom, br) == "br" && editing_host(dom, br) == Some(host))
}

/// Merge the blocks selected by the planner. In particular, retain any
/// hidden sibling that was skipped when resolving the visible target.
fn merge_blocks(page: &PageDom, dom: &mut Dom, host: NodeId, from: Point, to: Point) -> Option<Point> {
    let mut first = block_of(dom, from.0, host);
    let second = block_of(dom, to.0, host);
    if first == second {
        return None;
    }
    if second == host && dom.parent(first).ok()? == Some(host) {
        let mut source = to.0;
        while source != host && dom.parent(source).ok()?? != host {
            source = dom.parent(source).ok()??;
        }
        if source == host {
            return None;
        }
        let mut seam = from;
        let mut trailing_break = None;
        let inline = dom
            .descendants(first)
            .ok()?
            .skip(1)
            .filter(|&n| is_text(dom, n) || dom.node_type(n).ok() == Some(NodeType::Element) && !is_container(dom, n))
            .collect::<Vec<_>>();
        for n in inline.into_iter().rev() {
            if !target_range::rendered(page, dom, n) {
                continue;
            }
            if local(dom, n) == "br" && trailing_break.is_none() && editing_host(dom, n) == Some(host) {
                trailing_break = Some(n);
                continue;
            }
            if is_text(dom, n) && editing_host(dom, n) == Some(host) {
                seam = (n, len(dom, n));
            }
            break;
        }
        let moved = paragraph_join::join_anonymous_right(page, dom, host, first, source, seam)?;
        if let Some(br) = trailing_break {
            dom.remove_node(br).ok()?;
        }
        return Some(moved);
    }
    if first == host && dom.parent(second).ok()? == Some(host) {
        return paragraph_join::join_first_line(page, dom, host, second, host, Some(second), from);
    }
    if first == host || second == host || dom.contains(first, second).ok()? || dom.contains(second, first).ok()? {
        return None;
    }
    // A nested paragraph in the preceding list item remains a child of
    // that item; append the promoted text after it rather than inside it.
    if let Some(item) = dom.parent_element(first).ok()? {
        if matches!(local(dom, item).as_str(), "li" | "dt" | "dd")
            && matches!(local(dom, second).as_str(), "li" | "dt" | "dd")
            && dom.parent(item).ok()? == dom.parent(second).ok()?
            && item != host
        {
            first = item;
        }
    }
    if !is_block(dom, first)
        || !is_block(dom, second)
        || [first, second].into_iter().any(|n| editing_host(dom, n) != Some(host))
    {
        return None;
    }
    if placeholder_block(dom, first)
        && dom.parent(first).ok()? == dom.parent(second).ok()?
        && editable_edge(dom, (second, 0), true) == editable_edge(dom, to, true)
    {
        dom.remove_node(first).ok()?;
        return Some(to);
    }
    // An empty following paragraph contributes no line content.
    if placeholder_block(dom, second) || dom.child_count(second).ok() == Some(0) {
        dom.remove_node(second).ok()?;
        return Some(from);
    }
    let last = dom
        .descendants(first)
        .ok()?
        .filter(|&n| {
            n != first
                && target_range::rendered(page, dom, n)
                && (is_text(dom, n) && len(dom, n) > 0
                    || dom.node_type(n).ok() == Some(NodeType::Element) && !is_container(dom, n))
        })
        .last();
    let trailing = last.filter(|&n| {
        local(dom, n) == "br"
            && editing_host(dom, n) == Some(host)
            && (placeholder_block(dom, first)
                || dom
                    .parent(n)
                    .ok()
                    .flatten()
                    .zip(dom.index_in_parent(n).ok())
                    .is_some_and(|edge| crate::range::compare_boundary_points_dom(dom, from, edge) <= 0))
    });
    let has_later_line = dom.descendants(second).ok()?.skip(1).any(|n| {
        if is_text(dom, n) {
            dom.char_data(n).is_ok_and(|s| s.contains('\n'))
                && crate::edit_commands::newlines_significant_in(page, dom, n)
        } else {
            local(dom, n) == "br"
                || is_block(dom, n)
                || page.layout().computed_style_value(dom, n, "display") == "block"
        }
    });
    // Joining sibling items of the same list retains their contents and
    // internal line breaks under one item (and one list marker).
    let same_list = dom.parent_element(first).ok()?.is_some_and(|parent| {
        dom.parent_element(second).ok().flatten() == Some(parent)
            && [first, second, parent].iter().all(|&n| dom.element(n).is_ok_and(|el| el.is_html()))
            && match local(dom, parent).as_str() {
                "ol" | "ul" | "menu" => [first, second].iter().all(|&n| local(dom, n) == "li"),
                "dl" => [first, second].iter().all(|&n| matches!(local(dom, n).as_str(), "dt" | "dd")),
                _ => false,
            }
    });
    if has_later_line && !same_list {
        paragraph_join::join_first_line(page, dom, host, second, first, None, from)?;
    } else {
        // A single complete paragraph can move its existing nodes, keeping
        // their identities, attributes and formatting elements.
        while let Some(child) = dom.first_child(second).ok()? {
            dom.append_child(first, child).ok()?;
        }
        dom.remove_node(second).ok()?;
    }
    if let Some(br) = trailing {
        dom.remove_node(br).ok()?;
    }
    let seam =
        if dom.contains(first, from.0).ok()? && from.1 <= len(dom, from.0) { from } else { (first, len(dom, first)) };
    Some(placeholder_if_empty(dom, first).unwrap_or(seam))
}

/// Enter: splits the caret's block (insertParagraph).
pub fn insert_paragraph(page: &Rc<PageDom>, host: NodeId) {
    let (start, end) = host_range(page, host);
    edit(page, host, "insertParagraph", None, |page| {
        let caret = {
            let mut dom = page.dom.borrow_mut();
            let caret = if start != end { delete_range(&mut dom, host, start, end, true) } else { start };
            split_block(page, &mut dom, host, caret)
        };
        set_selection(page, caret, caret);
    });
}

/// Whether `node` is in (or is) an HTML element whose content is text only (raw text and
/// escapable raw text elements, `<textarea>`): a line break there is a linefeed, not a `<br>`.
fn in_text_only_element(dom: &Dom, node: NodeId) -> bool {
    let el = match dom.node_type(node).ok() {
        Some(NodeType::Element) => Some(node),
        _ => dom.parent_element(node).ok().flatten(),
    };
    el.and_then(|el| dom.element(el).ok().filter(|e| e.is_html()).map(|e| e.local_name().clone())).is_some_and(|name| {
        matches!(&*name, "script" | "style" | "title" | "textarea" | "xmp" | "iframe" | "noembed" | "noframes")
    })
}

fn split_block(page: &PageDom, dom: &mut Dom, host: NodeId, caret: Point) -> Point {
    let caret = insertion_point(dom, caret);
    // Content that is text only (a `<script>` or `<style>` in the head of a designMode
    // document) gets a linefeed, as in browsers: no element goes into it.
    if in_text_only_element(dom, caret.0) {
        return insert_at(dom, caret, "\n");
    }
    let Ok(doc) = dom.node_document(host) else {
        return caret;
    };
    // Address and preformatted paragraphs continue with a line break.
    let mut ancestor = Some(caret.0);
    while let Some(node) = ancestor {
        if matches!(local(dom, node).as_str(), "address" | "listing" | "pre") {
            return line_break_at(dom, host, caret);
        }
        if node == host || is_block(dom, node) { break; }
        ancestor = dom.parent(node).ok().flatten();
    }
    let mut block = block_of(dom, caret.0, host);
    let mut caret = caret;
    let display = page.layout().computed_style_value(dom, block, "display");
    let inline_box = display == "inline" || display.starts_with("inline-");
    if block == host {
        if inline_box {
            return if crate::edit_commands::newlines_significant_in(page, dom, caret.0) {
                preserved_break_at(page, dom, host, caret)
            } else {
                line_break_at(dom, host, caret)
            };
        }
        // An inline editing host, heading, term or paragraph can't acquire
        // child paragraphs. Its surrounding block may lie outside this host.
        if !is_block(dom, host) && local(dom, host) != "body"
            || matches!(local(dom, host).as_str(), "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "p" | "dt")
        {
            return line_break_at(dom, host, caret);
        }
        // Make the anonymous paragraph containing the caret explicit before
        // splitting it. Neighbouring block elements remain outside it.
        let separator = dom.document_data(doc).map(|d| d.default_paragraph_separator).unwrap_or("div");
        let Ok(wrapper) = dom.create_element(doc, separator) else {
            return caret;
        };
        let mut child = caret.0;
        while child != host && dom.parent(child).ok().flatten() != Some(host) {
            let Ok(Some(parent)) = dom.parent(child) else {
                return caret;
            };
            child = parent;
        }
        let children = dom.children(host).map(|c| c.collect::<Vec<_>>()).unwrap_or_default();
        let at = if child == host {
            caret.1 as usize
        } else {
            let Ok(idx) = dom.index_in_parent(child) else {
                return caret;
            };
            idx as usize
        };
        let mut begin = at.min(children.len());
        while begin > 0 && !is_block(dom, children[begin - 1]) {
            begin -= 1;
        }
        let mut end = at.min(children.len());
        while end < children.len() && !is_block(dom, children[end]) {
            end += 1;
        }
        if dom.insert_before(host, wrapper, children.get(begin).copied()).is_err() {
            return caret;
        }
        for &node in &children[begin..end] {
            let _ = dom.append_child(wrapper, node);
        }
        if child == host {
            caret = (wrapper, (at - begin) as u32);
        }
        block = wrapper;
    }
    let tag = local(dom, block);
    let Ok(new_block) = dom.clone_node(block, false, None) else {
        return caret;
    };
    let _ = dom.remove_attribute(new_block, "id");
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
    // Reconstruct the inline ancestor chain on the right side of the split.
    // The original nodes and their IDs stay in the first paragraph.
    let mut container = if is_text(dom, n) { dom.parent(n).ok().flatten() } else { Some(n) };
    let mut right_child = None;
    while let Some(parent) = container {
        let dest = if parent == block {
            new_block
        } else {
            let Ok(clone) = dom.clone_node(parent, false, None) else {
                return caret;
            };
            let _ = dom.remove_attribute(clone, "id");
            clone
        };
        if let Some(child) = right_child {
            let _ = dom.append_child(dest, child);
        }
        let mut next = split_from;
        while let Some(child) = next {
            next = dom.next_sibling(child).ok().flatten();
            let _ = dom.append_child(dest, child);
        }
        if parent == block {
            break;
        }
        right_child = (dom.child_count(dest).unwrap_or(0) > 0).then_some(dest);
        split_from = dom.next_sibling(parent).ok().flatten();
        container = dom.parent(parent).ok().flatten();
    }
    let parent = dom.parent(block).ok().flatten().unwrap_or(host);
    let next = dom.next_sibling(block).ok().flatten();
    let _ = dom.insert_before(parent, new_block, next);
    // Duplicating an inline-styled p or div alone does not start a new
    // visual paragraph. Keep its styles and put each half in a block.
    if inline_box && matches!(tag.as_str(), "p" | "div") {
        for child in [block, new_block] {
            let Ok(wrapper) = dom.create_element(doc, "div") else {
                break;
            };
            if dom.insert_before(parent, wrapper, Some(child)).is_ok() {
                let _ = dom.append_child(wrapper, child);
            }
        }
    }
    // A break at the end of a link is the new paragraph's placeholder.
    // Starting the next line inside that link would continue its href.
    let mut only_child = new_block;
    let mut inside_link = false;
    while dom.child_count(only_child).ok() == Some(1) {
        let Ok(Some(child)) = dom.first_child(only_child) else { break };
        inside_link |= local(dom, only_child) == "a";
        only_child = child;
    }
    if inside_link && local(dom, only_child) == "br" {
        let _ = dom.replace_all(new_block, Some(only_child));
    }
    placeholder_if_empty(dom, block);
    placeholder_if_empty(dom, new_block).unwrap_or_else(|| host_start(dom, new_block))
}

/// Caret keys inside a host: ArrowLeft/Right by one character, Home/End to
/// the block edges; Shift extends.
pub fn move_caret(page: &Rc<PageDom>, host: NodeId, key: &str, extend: bool) {
    let Some((anchor, focus)) = selection(page) else {
        let p = initial_caret(page, &page.dom.borrow(), host).unwrap_or((host, 0));
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

pub(crate) fn step(dom: &Dom, host: NodeId, p: Point, forward: bool) -> Point {
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
    // Into the adjacent text node (crossing a block edge or a <br> counts as
    // a step).
    match adjacent_text(dom, p.0, host, forward) {
        Some(t) if block_of(dom, t, host) == block_of(dom, p.0, host) && !br_between(dom, host, p.0, t) => {
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

/// Deletes [start, end) in `host` as one edit of `input_type` (word and
/// line deletion, cut).
pub(crate) fn delete_span(page: &Rc<PageDom>, host: NodeId, input_type: &str, start: Point, end: Point) {
    edit(page, host, input_type, None, |page| {
        let caret = delete_range(&mut page.dom.borrow_mut(), host, start, end, true);
        set_selection(page, caret, caret);
    });
}

/// Elements pasted or inserted markup never brings along.
const DROPPED_ON_INSERT: &[&str] = &["script", "style", "meta", "link", "title", "head", "base", "template"];

/// Moves `frag`'s children to the caret, splitting the text there; returns
/// the caret after them.
/// With `sanitize` (paste), event handler attributes and `javascript:` URLs
/// are dropped too, as Chromium sanitizes pasted markup: the clipboard may
/// hold markup another page wrote.
fn insert_fragment(dom: &mut Dom, host: NodeId, caret: Point, frag: NodeId, sanitize: bool) -> Point {
    let drop: Vec<NodeId> = dom
        .descendants(frag)
        .map(|d| d.filter(|&n| DROPPED_ON_INSERT.contains(&local(dom, n).as_str())).collect())
        .unwrap_or_default();
    for n in drop {
        if let Ok(Some(p)) = dom.parent(n) {
            let _ = dom.remove_child(p, n);
        }
    }
    if sanitize {
        let elements: Vec<NodeId> = dom
            .descendants(frag)
            .map(|d| d.filter(|&n| dom.node_type(n).ok() == Some(NodeType::Element)).collect())
            .unwrap_or_default();
        for el in elements {
            let unsafe_attrs: Vec<String> = dom
                .attributes(el)
                .map(|attrs| {
                    attrs
                        .iter()
                        .filter(|a| {
                            let name = &*a.name.local;
                            name.len() > 2 && name.get(..2).is_some_and(|p| p.eq_ignore_ascii_case("on"))
                                || matches!(name, "href" | "src" | "action" | "formaction" | "xlink:href")
                                    && a.value
                                        .trim_start_matches(|c: char| c.is_ascii_whitespace() || c.is_ascii_control())
                                        .get(..11)
                                        .is_some_and(|p| p.eq_ignore_ascii_case("javascript:"))
                        })
                        .map(|a| a.qualified_name())
                        .collect()
                })
                .unwrap_or_default();
            for name in unsafe_attrs {
                let _ = dom.remove_attribute(el, &name);
            }
        }
    }
    let kids: Vec<NodeId> = dom.children(frag).map(|c| c.collect()).unwrap_or_default();
    let Some(&last) = kids.last() else { return caret };
    let caret = insertion_point(dom, caret);
    let (parent, before) = if is_text(dom, caret.0) {
        let n = caret.0;
        let parent = dom.parent(n).ok().flatten().unwrap_or(host);
        let before = if caret.1 == 0 {
            Some(n)
        } else if caret.1 >= len(dom, n) {
            dom.next_sibling(n).ok().flatten()
        } else {
            dom.split_text(n, caret.1).ok()
        };
        (parent, before)
    } else {
        (caret.0, dom.child_at(caret.0, caret.1).ok().flatten())
    };
    // Content replaces an empty block's placeholder <br>.
    let placeholder =
        before.filter(|&b| local(dom, b) == "br" && dom.child_count(parent).unwrap_or(0) == 1 && parent != host);
    for k in kids {
        let _ = dom.insert_before(parent, k, before);
    }
    if let Some(br) = placeholder {
        let _ = dom.remove_child(parent, br);
    }
    if is_text(dom, last) {
        (last, len(dom, last))
    } else {
        deep_backward(dom, (parent, dom.index_in_parent(last).unwrap_or(0) + 1))
    }
}

/// Split containers which cannot contain an hr, keeping both sides and their
/// attributes. An id stays with the original element when it splits in two.
fn lift_horizontal_rule(dom: &mut Dom, host: NodeId, rule: NodeId) {
    let mut highest = None;
    let mut cur = dom.parent_element(rule).ok().flatten();
    while let Some(el) = cur {
        if el == host {
            break;
        }
        if matches!(
            local(dom, el).as_str(),
            "a" | "abbr"
                | "acronym"
                | "b"
                | "bdi"
                | "bdo"
                | "big"
                | "blink"
                | "cite"
                | "code"
                | "dfn"
                | "em"
                | "font"
                | "h1"
                | "h2"
                | "h3"
                | "h4"
                | "h5"
                | "h6"
                | "i"
                | "kbd"
                | "listing"
                | "mark"
                | "marquee"
                | "nobr"
                | "p"
                | "pre"
                | "q"
                | "rp"
                | "rt"
                | "ruby"
                | "s"
                | "samp"
                | "small"
                | "span"
                | "strike"
                | "strong"
                | "sub"
                | "sup"
                | "tt"
                | "u"
                | "var"
                | "xmp"
        ) {
            highest = Some(el);
        }
        cur = dom.parent_element(el).ok().flatten();
    }
    let Some(last) = highest else { return };
    while let Ok(Some(parent)) = dom.parent(rule) {
        let Ok(Some(grandparent)) = dom.parent(parent) else { return };
        let next = dom.next_sibling(parent).ok().flatten();
        if dom.prev_sibling(rule).ok().flatten().is_none() {
            if dom.insert_before(grandparent, rule, Some(parent)).is_err() {
                return;
            }
            if dom.child_count(parent).ok() == Some(0) {
                let _ = dom.remove_child(grandparent, parent);
            }
        } else {
            if dom.next_sibling(rule).ok().flatten().is_some() {
                let Ok(right) = dom.clone_node(parent, false, None) else { return };
                let _ = dom.remove_attribute(right, "id");
                while let Ok(Some(child)) = dom.next_sibling(rule) {
                    if dom.append_child(right, child).is_err() {
                        return;
                    }
                }
                let _ = dom.insert_before(grandparent, right, next);
            }
            let before = dom.next_sibling(parent).ok().flatten();
            if dom.insert_before(grandparent, rule, before).is_err() {
                return;
            }
        }
        if parent == last {
            break;
        }
    }
}

/// Insert an image or horizontal rule directly into the selected range
/// without reparsing the supplied URL as markup.
pub(crate) fn insert_replaced_element(page: &Rc<PageDom>, host: NodeId, image_src: Option<&str>) -> bool {
    let (start, end) = host_range(page, host);
    let mut dom = page.dom.borrow_mut();
    let (node, offset) = if start == end {
        start
    } else {
        delete_range_with_merging(&mut dom, host, start, end, false, image_src.is_some())
    };
    let (parent, before) = if is_text(&dom, node) {
        let Ok(Some(p)) = dom.parent(node) else { return false };
        let before = if offset == 0 {
            Some(node)
        } else if offset == len(&dom, node) {
            dom.next_sibling(node).ok().flatten()
        } else {
            let Ok(right) = dom.split_text(node, offset) else { return false };
            Some(right)
        };
        (p, before)
    } else if matches!(dom.node_type(node), Ok(NodeType::Comment | NodeType::ProcessingInstruction)) {
        // These boundaries have no visible caret positions. Editing resolves
        // them to the next position in their parent.
        let Ok(Some(parent)) = dom.parent(node) else { return false };
        (parent, dom.next_sibling(node).ok().flatten())
    } else {
        (node, dom.child_at(node, offset).ok().flatten())
    };
    let Ok(doc) = dom.node_document(host) else { return false };
    let tag = if image_src.is_some() { "img" } else { "hr" };
    let Ok(el) = dom.create_element(doc, tag) else { return false };
    if let Some(src) = image_src {
        let _ = dom.set_attribute(el, "src", src);
    }
    if dom.insert_before(parent, el, before).is_err() {
        return false;
    }
    if let Some(br) = before {
        if dom.child_count(parent).ok() == Some(2) && is_block(&dom, parent) && local(&dom, br) == "br" {
            let _ = dom.remove_child(parent, br);
        }
    }
    if image_src.is_none() {
        lift_horizontal_rule(&mut dom, host, el);
    }
    let Ok(Some(p)) = dom.parent(el) else { return false };
    let caret = (p, 1 + dom.index_in_parent(el).unwrap_or(0));
    drop(dom);
    set_selection(page, caret, caret);
    true
}

/// `insertHTML` (and rich paste): `html` parsed in the host's context
/// replaces the selection; the caret goes after it.
pub(crate) fn insert_html(page: &Rc<PageDom>, host: NodeId, html: &str, input_type: &str) {
    let (start, end) = host_range(page, host);
    let transfer = crate::data_transfer::NativeTransfer::new(
        page,
        &[("text/html".to_owned(), html.to_owned())],
        "uninitialized",
        crate::data_transfer::Mode::ReadOnly,
    );
    edit_with(page, host, input_type, None, transfer.as_ref(), |page| {
        let caret = {
            let mut dom = page.dom.borrow_mut();
            let caret = if start != end { delete_range(&mut dom, host, start, end, true) } else { start };
            let context = if is_text(&dom, caret.0) { dom.parent(caret.0).ok().flatten() } else { Some(caret.0) };
            let context = context.filter(|&c| dom.node_type(c).ok() == Some(NodeType::Element)).unwrap_or(host);
            match pyro_dom::parse::parse_fragment(&mut dom, context, html) {
                Ok(frag) => insert_fragment(&mut dom, host, caret, frag, false),
                Err(_) => caret,
            }
        };
        set_selection(page, caret, caret);
    });
    if let Some(t) = transfer {
        t.set_mode(crate::data_transfer::Mode::Disabled);
    }
}

/// Paste into a host (`insertFromPaste`): markup when the clipboard has it
/// (`html`), else the text with its linefeeds as line breaks.
pub(crate) fn insert_pasted(
    page: &Rc<PageDom>,
    host: NodeId,
    text: &str,
    html: Option<&str>,
    transfer: Option<&crate::data_transfer::NativeTransfer>,
) {
    let (start, end) = host_range(page, host);
    let newline = crate::edit_commands::newlines_significant(page, start.0);
    edit_with(page, host, "insertFromPaste", None, transfer, |page| {
        let caret = {
            let mut dom = page.dom.borrow_mut();
            let caret = if start != end { delete_range(&mut dom, host, start, end, true) } else { start };
            match html {
                Some(html) => {
                    let context =
                        if is_text(&dom, caret.0) { dom.parent(caret.0).ok().flatten() } else { Some(caret.0) };
                    let context = context.filter(|&c| dom.node_type(c).ok() == Some(NodeType::Element)).unwrap_or(host);
                    match pyro_dom::parse::parse_fragment(&mut dom, context, html) {
                        Ok(frag) => insert_fragment(&mut dom, host, caret, frag, true),
                        Err(_) => caret,
                    }
                }
                None if newline || !text.contains('\n') => insert_at(&mut dom, caret, &text.replace("\r\n", "\n")),
                None => {
                    let mut caret = caret;
                    for (i, line) in text.replace("\r\n", "\n").split('\n').enumerate() {
                        if i > 0 {
                            caret = line_break_at(&mut dom, host, caret);
                        }
                        if !line.is_empty() {
                            caret = insert_at(&mut dom, caret, line);
                        }
                    }
                    caret
                }
            }
        };
        set_selection(page, caret, caret);
    });
}

/// Whether a `<br>` lies between text nodes `a` and `b` (either order)
/// inside `within`.
fn br_between(dom: &Dom, within: NodeId, a: NodeId, b: NodeId) -> bool {
    let (first, last) =
        if crate::range::compare_boundary_points_dom(dom, (a, 0), (b, 0)) <= 0 { (a, b) } else { (b, a) };
    let Ok(all) = dom.descendants(within) else { return false };
    let mut inside = false;
    for n in all {
        if n == first {
            inside = true;
        } else if n == last {
            return false;
        } else if inside && local(dom, n) == "br" {
            return true;
        }
    }
    false
}

/// Shift+Enter / `insertLineBreak`: a `<br>` at the caret (a linefeed where
/// white space preserves newlines), with a second one when the break ends
/// its line so the new line shows, as Chromium does.
pub fn insert_line_break(page: &Rc<PageDom>, host: NodeId) {
    let (start, end) = host_range(page, host);
    let newline = crate::edit_commands::newlines_significant(page, start.0)
        || in_text_only_element(&page.dom.borrow(), start.0);
    edit(page, host, "insertLineBreak", None, |page| {
        let caret = {
            let mut dom = page.dom.borrow_mut();
            let caret = if start != end { delete_range(&mut dom, host, start, end, true) } else { start };
            if newline {
                preserved_break_at(page, &mut dom, host, caret)
            } else {
                line_break_at(&mut dom, host, caret)
            }
        };
        set_selection(page, caret, caret);
    });
}

fn preserved_break_at(page: &PageDom, dom: &mut Dom, host: NodeId, caret: Point) -> Point {
    let point = insertion_point(dom, caret);
    let at_end = whitespace::at_line_end(page, dom, host, point);
    let after = insert_at(dom, caret, if at_end { "\n\n" } else { "\n" });
    if at_end {
        (after.0, after.1.saturating_sub(1))
    } else {
        after
    }
}

fn next_in_block(dom: &Dom, host: NodeId, node: NodeId) -> Option<NodeId> {
    let block = block_of(dom, node, host);
    let mut current = node;
    loop {
        if let Some(next) = dom.next_sibling(current).ok()? { return Some(next); }
        current = dom.parent(current).ok()??;
        if current == host || current == block { return None; }
    }
}

fn line_break_at(dom: &mut Dom, host: NodeId, caret: Point) -> Point {
    let caret = insertion_point(dom, caret);
    let Ok(doc) = dom.node_document(host) else { return caret };
    let Ok(br) = dom.create_element(doc, "br") else { return caret };
    let (parent, before) = if is_text(dom, caret.0) {
        let n = caret.0;
        let parent = dom.parent(n).ok().flatten().unwrap_or(host);
        let before = if caret.1 == 0 {
            Some(n)
        } else if caret.1 >= len(dom, n) {
            dom.next_sibling(n).ok().flatten()
        } else {
            dom.split_text(n, caret.1).ok()
        };
        (parent, before)
    } else {
        (caret.0, dom.child_at(caret.0, caret.1).ok().flatten())
    };
    let _ = dom.insert_before(parent, br, before);
    // Nothing follows on the line inside its block: a placeholder <br> keeps
    // the new line visible.
    let next = next_in_block(dom, host, br);
    if next.is_none_or(|n| is_block(dom, n)) {
        if let Ok(extra) = dom.create_element(doc, "br") {
            let after = dom.next_sibling(br).ok().flatten();
            let _ = dom.insert_before(parent, extra, after);
        }
    }
    match dom.next_sibling(br).ok().flatten() {
        Some(t) if is_text(dom, t) => (t, 0),
        _ => (parent, dom.index_in_parent(br).unwrap_or(0) + 1),
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

/// Find the first visible caret position without descending into atomic or
/// non-editable content. Empty wrappers do not supply a text position.
fn initial_caret(page: &PageDom, dom: &Dom, host: NodeId) -> Option<Point> {
    let layout = page.layout();
    let mut stack = vec![(host, dom.children(host).ok()?.enumerate())];
    while let Some((parent, children)) = stack.last_mut() {
        let Some((index, child)) = children.next() else {
            stack.pop();
            continue;
        };
        if is_text(dom, child) {
            let text = dom.char_data(child).ok()?;
            if text.is_empty() || layout.computed_style_value(dom, *parent, "visibility") == "hidden" {
                continue;
            }
            let mode = layout.computed_style_value(dom, *parent, "white-space-collapse");
            let skip = text.chars().take_while(|c| match mode.as_str() {
                "preserve" | "break-spaces" => false,
                "preserve-breaks" => matches!(c, ' ' | '\t'),
                _ => matches!(c, ' ' | '\t' | '\n' | '\r'),
            }).count();
            // All skipped characters occupy one byte and one UTF-16 unit.
            if skip < text.len() {
                return Some((child, skip as u32));
            }
        } else if dom.node_type(child).ok() == Some(NodeType::Element) {
            if layout.computed_style_value(dom, child, "display") == "none" {
                continue;
            }
            let noneditable = editable_attr(dom, child) == Some(false);
            if noneditable
                || !is_container(dom, child)
                || matches!(local(dom, child).as_str(), "button" | "select" | "textarea")
            {
                if layout.computed_style_value(dom, child, "visibility") != "hidden" {
                    // At a non-editable island, preceding empty wrappers
                    // share the caret position at the start of this container.
                    return Some((*parent, if noneditable { 0 } else { index as u32 }));
                }
            } else {
                stack.push((child, dom.children(child).ok()?.enumerate()));
            }
        }
    }
    None
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
        let p = initial_caret(page, &page.dom.borrow(), host).unwrap_or((host, 0));
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

/// The caret a press at viewport (x, y) puts in editable content: the
/// character boundary nearest to the point in the pressed element's laid-out
/// text, else the end of its content.
pub(crate) fn caret_at_point(page: &Rc<PageDom>, target: NodeId, x: f64, y: f64) -> Option<Point> {
    let dom = page.dom.borrow();
    editing_host(&dom, target)?;
    let layout = page.layout();
    let hit = layout.caret_from_point(&dom, target, x, y, &|_| true);
    Some(match hit {
        Some((node, offset, _)) if editing_host(&dom, node).is_some() => (node, offset),
        _ => deep_backward(&dom, (target, len(&dom, target))),
    })
}
