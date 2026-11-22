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
mod child_line;
mod paragraph_join;
mod target_range;
pub(crate) use target_range::rendered;
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
    /// An edit can project both ends of a directed range onto one visible
    /// caret. Explicit collapse starts a fresh, directionless selection.
    pub(crate) retained_backward: Option<bool>,
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

/// Legacy Selection endpoint getters expose the single-tree live range.
pub(crate) fn legacy_endpoints(page: &PageDom) -> Option<(Point, Point)> {
    let points = selection(page)?;
    let state = page.selection.borrow();
    let selection = state.as_ref()?;
    if selection.cross_tree {
        return Some((selection.range.live.start(), selection.range.live.end()));
    }
    Some(points)
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

/// Keep a collapsed shadow caret available to composed snapshots after its
/// host is removed. The live Range held by script remains in its own tree.
pub(crate) fn track_composed_caret(page: &Rc<PageDom>) {
    let mut slot = page.selection.borrow_mut();
    let Some(sel) = slot.as_mut() else { return };
    let caret = sel.range.live.start();
    if sel.composed.is_some() || sel.range.live.end() != caret
        || page.dom.borrow().root_node(caret.0, false).ok() == Some(page.document())
    {
        return;
    }
    if let Ok(composed) = crate::range::RangeOwner::new(page, caret, caret) {
        composed.live.track_shadow_host_removal();
        sel.composed = Some(composed);
    }
}

/// (start, end) in document order.
pub fn selection_range(page: &PageDom) -> Option<(Point, Point)> {
    let (a, f) = selection(page)?;
    Some(if compare(page, a, f) <= 0 { (a, f) } else { (f, a) })
}

/// Selects from `anchor` to `focus` with a new range (as `collapse()` and
/// `extend()` do).
pub fn set_selection(page: &Rc<PageDom>, anchor: Point, focus: Point) {
    set_selection_points(page, anchor, focus, None);
}

pub(crate) fn set_adjusted_caret(page: &Rc<PageDom>, caret: Point, backward: bool) {
    set_selection_points(page, caret, caret, Some(backward));
}

fn set_selection_points(page: &Rc<PageDom>, anchor: Point, focus: Point, retained_backward: Option<bool>) {
    let changed = selection(page) != Some((anchor, focus));
    let same_tree = {
        let dom = page.dom.borrow();
        dom.root_node(anchor.0, false).ok() == dom.root_node(focus.0, false).ok()
    };
    let backward = if same_tree {
        compare(page, anchor, focus) > 0
    } else {
        crate::range::composed_order(&page.dom.borrow(), anchor, focus).is_ok_and(|order| order > 0)
    };
    let (start, end) = match (same_tree, backward) {
        (false, true) => (anchor, anchor),
        (false, false) => (focus, focus),
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
    let old = page.selection.replace(Some(DocSelection { range, backward, retained_backward, wrappers: Vec::new(), origin, composed, cross_tree }));
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
    current.retained_backward=None;
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
        retained_backward: None,
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

pub(crate) fn apply_edit(
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
                || (!is_text(dom, d) && (!is_container(dom, d) || html_select(dom, d) || editable_attr(dom, d) == Some(false))))
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
    let selected_child = (start.0 == end.0 && end.1.checked_sub(start.1) == Some(1))
        .then(|| dom.child_at(start.0, start.1).ok().flatten()).flatten();
    if let Some(child) = selected_child.filter(|&el| html_select(dom, el) && editing_host(dom, el) == Some(host)) {
        let _ = dom.remove_child(start.0, child);
        if let Some(previous) = text_before_control(dom, host, start) { return previous; }
        let next = deep_forward(dom, start);
        return if editing_host(dom, next.0) == Some(host) && is_text(dom, next.0) && !inside_select(dom, next.0) {
            next
        } else {
            start
        };
    }
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
    insert_at_point(dom, point, text, true)
}

fn insert_at_point(dom: &mut Dom, (n, off): Point, text: &str, replace_placeholder: bool) -> Point {
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
        replace_placeholder && local(dom, b) == "br"
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
    if dom.child_at(caret.0, caret.1).ok().flatten().is_some_and(|n| html_select(dom, n)) {
        return Some(text_before_control(dom, host, caret).unwrap_or(caret));
    }
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
        if is_text(dom, node) {
            return Some((node, len(dom, node)));
        }
        if !is_container(dom, node) || dom.shadow_root(node).ok().flatten().is_some()
            || layout.computed_style_value(dom, node, "display") != "inline"
            || layout.computed_style_value(dom, node, "visibility") == "hidden"
        {
            break;
        }
        if dom.child_count(node).ok() != Some(0) {
            let end = editable_edge(dom, (node, len(dom, node)), false);
            // A descendant can have a different display or visibility.
            if is_text(dom, end.0) && dom.parent(end.0).ok().flatten() == Some(node) {
                return Some(end);
            }
            break;
        }
        previous = dom.prev_sibling(node).ok().flatten();
    }
    Some(boundary)
}

fn enclosing_link(dom: &Dom, host: NodeId, mut node: NodeId) -> Option<NodeId> {
    while node != host {
        if dom.element(node).is_ok_and(|el| el.is_html() && el.local_name() == &markup5ever::local_name!("a") && el.attr("href").is_some()) {
            return Some(node);
        }
        node = dom.parent(node).ok()??;
    }
    None
}

/// Typing at the outside edge of a rendered inline link does not extend its
/// destination. An interior caret and an anchor serving as the editor itself
/// remain within their original text.
fn link_typing_point(page: &PageDom, dom: &Dom, host: NodeId, point: Point) -> Point {
    let Some(link) = enclosing_link(dom, host, point.0) else { return point };
    let mut first = point.1 == 0;
    let mut last = point.1 == len(dom, point.0);
    let mut node = point.0;
    while node != link {
        if !first && !last { return point; }
        first &= dom.prev_sibling(node).ok().flatten().is_none();
        last &= dom.next_sibling(node).ok().flatten().is_none();
        let Some(parent) = dom.parent(node).ok().flatten() else { return point };
        node = parent;
    }
    if first == last { return point; }
    let layout = page.layout();
    let display = layout.computed_style_value(dom, link, "display");
    if !(display == "inline" || display == "contents" || display.starts_with("inline-")) {
        return point;
    }
    let (Ok(Some(parent)), Ok(index)) = (dom.parent(link), dom.index_in_parent(link)) else { return point };
    if first {
        // Prefer the preceding editable text on the same line. At the
        // start of the line retain the link parent's formatting context.
        let mut edge = link;
        while edge != host {
            if let Ok(Some(previous)) = dom.prev_sibling(edge) {
                let end = editable_edge(dom, (previous, len(dom, previous)), false);
                if is_text(dom, end.0) && block_of(dom, end.0, host) == block_of(dom, link, host)
                    && editing_host(dom, end.0) == Some(host) && enclosing_link(dom, host, end.0).is_none()
                    && target_range::rendered(page, dom, end.0)
                {
                    return end;
                }
                break;
            }
            let Some(container) = dom.parent(edge).ok().flatten() else { break };
            if container == host || layout.computed_style_value(dom, container, "display") != "inline" { break; }
            edge = container;
        }
    } else if let Ok(Some(next)) = dom.next_sibling(link) {
        if is_text(dom, next) { return (next, 0); }
    }
    (parent, index + u32::from(last))
}

fn text_before_control(dom: &Dom, host: NodeId, edge: Point) -> Option<Point> {
    let previous = dom.child_at(edge.0, edge.1.checked_sub(1)?).ok()??;
    (is_text(dom, previous) && editing_host(dom, previous) == Some(host)).then(|| (previous, len(dom, previous)))
}

/// DOM endpoints below a select do not expose a rendered selection direction.
pub(crate) fn inside_select(dom: &Dom, mut node: NodeId) -> bool {
    while let Ok(Some(parent)) = dom.parent_element(node) {
        if html_select(dom, parent) { return true; }
        node = parent;
    }
    false
}

/// DOM selections can reach option text that has no caret position in the
/// rendered editor. Keep edits outside the control, and treat a selection
/// of its entire contents as a selection of the atomic box.
fn select_edit_range(page: &PageDom, host: NodeId, start: Point, end: Point) -> Option<(Point, Point)> {
    let dom = &*page.dom.borrow();
    let in_control = |mut node| loop {
        if dom.element(node).is_ok_and(|el| el.is_html()
            && matches!(el.local_name().as_ref(), "option" | "optgroup" | "select")) {
            return true;
        }
        if node == host { return false; }
        let Some(parent) = dom.parent(node).ok().flatten() else { return false };
        node = parent;
    };
    if start.0 == end.0 && html_select(dom, start.0) && start.0 != host
        && start.1 == 0 && end.1 != 0 && end.1 == len(dom, start.0)
    {
        let parent = dom.parent(start.0).ok()??;
        let index = dom.index_in_parent(start.0).ok()?;
        return Some(((parent, index), (parent, index + 1)));
    }
    if start.0 != host && html_select(dom, start.0) && start.1 == 0 && start.0 != end.0
        && selection(page).is_some_and(|(anchor, _)| anchor == start)
        && dom.contains(start.0, end.0).ok()?
    {
        let outside = (dom.parent(start.0).ok()??, dom.index_in_parent(start.0).ok()?);
        return Some((outside, outside));
    }
    if in_control(start.0) || selection(page).is_some_and(|(anchor, _)| inside_select(dom, anchor.0)) { return None; }
    Some((start, if in_control(end.0) { start } else { end }))
}

/// insertText (typing, `Input.insertText`): replaces the selection.
pub fn insert_text(page: &Rc<PageDom>, host: NodeId, text: &str) {
    let (start, end) = host_range(page, host);
    let Some((start, end)) = select_edit_range(page, host, start, end) else { return };
    // Replacement and continued typing form one edit. Keep typing inside
    // the link when that edit began on selected link text.
    let keep_link = if start == end {
        page.undo.borrow().continues_host_typing(host, start)
    } else {
        let dom = page.dom.borrow();
        enclosing_link(&dom, host, start.0).is_some_and(|link| dom.contains(link, end.0).unwrap_or(false))
    };
    edit(page, host, "insertText", Some(text), |page| {
        let caret = {
            let mut dom = page.dom.borrow_mut();
            let caret = if start == end {
                start
            } else if let Some(plan) = child_line::Deletion::plan(page, &dom, host, start, end, false) {
                plan.apply(page, &mut dom, host)
            } else {
                delete_range(&mut dom, host, start, end, false)
            };
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
                    let point = if index == 0 && !keep_link { link_typing_point(page, &dom, host, point) } else { point };
                    whitespace::consume_padding_lf(page, &mut dom, host, point);
                    let inserted_end = insert_at_point(&mut dom, point, part, true);
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
        if text.ends_with('\n') {
            crate::edit_commands::inline_value::set_break_caret(page, (start, end), caret);
        } else {
            set_selection(page, caret, caret);
        }
    });
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
    let Some((mut start, mut end)) = select_edit_range(page, host, start, end) else { return };
    let control = (start == end).then(|| page.dom.borrow().child_at(start.0, start.1).ok().flatten()).flatten()
        .filter(|&node| html_select(&page.dom.borrow(), node));
    if let Some(control) = control {
        if !backward {
            end.1 += 1;
        } else {
            let plan = paragraph_join::ControlLine::before_select(page, &page.dom.borrow(), host, control);
            if let Some(plan) = plan {
                apply_edit(page, host, input_type, None, None, |page| {
                    let caret = plan.apply(&mut page.dom.borrow_mut());
                    set_selection(page, caret, caret);
                });
                return;
            }
            // A locked predecessor or the beginning of the host has no
            // editable character to remove. Keep the control and its options.
            let Some(previous) = text_before_control(&page.dom.borrow(), host, start) else { return };
            start = previous;
            end = previous;
        }
    }
    let child_line = child_line::Deletion::plan(page, &page.dom.borrow(), host, start, end, backward);
    let (start, end, span) = if let Some(plan) = &child_line {
        (start, end, Some(plan.range))
    } else if start == end {
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
            let inline_space = (start == end).then(|| whitespace::delete_inline_space(page, &mut dom, host, from, to)).flatten();
            if let Some(caret) = inline_space.or_else(|| whitespace::delete_after_spaces(page, &mut dom, host, from, to)) {
                drop(dom);
                set_selection(page, caret, caret);
                return;
            }
            let adjacent_text = start == end && from.0 == to.0 && is_text(&dom, from.0)
                && [dom.prev_sibling(from.0), dom.next_sibling(from.0)].into_iter()
                    .any(|s| s.ok().flatten().is_some_and(|n| is_text(&dom, n)));
            let rebalance = adjacent_text && whitespace::visible_before_deletion(&dom, from, to);
            let repair = if start == end { whitespace::deletion_repair(page, &dom, host, start, backward) } else { None };
            let caret = if let Some(plan) = child_line {
                plan.apply(page, &mut dom, host)
            } else if let Some(caret) = paragraph_join::at_boundary(page, &mut dom, host, start, end, backward) {
                caret
            } else {
                delete_content(page, &mut dom, host, start, end, backward, (from, to))
            };
            let adjacent = (adjacent_text && caret == from)
                .then(|| whitespace::after_text_deletion(page, &mut dom, host, caret, rebalance)).flatten();
            adjacent.unwrap_or_else(|| {
                if let Some(repair) = repair {
                    whitespace::at(page, &mut dom, host, caret, repair);
                }
                caret
            })
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
    if start == end && from.0 != to.0 {
        if let Some(join) = paragraph_join::NestedJoin::at(page, dom, host, start, backward) {
            return join.apply(page, dom, host).unwrap_or(start);
        }
    }
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
                let replaced = matches!(local(dom, child).as_str(), "img" | "canvas" | "video" | "iframe")
                    && is_editable(dom, child)
                    && page.layout().computed_style_value(dom, child, "display").starts_with("inline");
                if dom.remove_child(from.0, child).is_ok() && replaced {
                    whitespace::after_replaced_element(page, dom, host, from);
                }
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
        whitespace::after_join(page, dom, host, to);
        return Some(to);
    }
    // An empty following paragraph contributes no line content.
    if placeholder_block(dom, second) || dom.child_count(second).ok() == Some(0) {
        dom.remove_node(second).ok()?;
        whitespace::after_join(page, dom, host, from);
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
    whitespace::after_join(page, dom, host, seam);
    Some(placeholder_if_empty(dom, first).unwrap_or(seam))
}

/// Plaintext-only editability is inherited until the nearest explicit
/// contenteditable mode, including a rich island inside a plaintext host.
pub(crate) fn plaintext_root(dom: &Dom, mut node: NodeId) -> Option<NodeId> {
    loop {
        if let Ok(Some(value)) = dom.get_attribute(node, "contenteditable") {
            if value.eq_ignore_ascii_case("plaintext-only") {
                return Some(node);
            }
            if value.is_empty() || value.eq_ignore_ascii_case("true") || value.eq_ignore_ascii_case("false") {
                return None;
            }
        }
        node = dom.parent_element(node).ok()??;
    }
}

pub(crate) fn plaintext_at(dom: &Dom, node: NodeId) -> bool {
    plaintext_root(dom, node).is_some()
}

/// The unmodified Enter key inserts a line break in a plaintext editor.
pub(crate) fn enter(page: &Rc<PageDom>, host: NodeId) {
    let (start, _) = host_range(page, host);
    let plain = {
        let dom = page.dom.borrow();
        dom.get_attribute(host, "contenteditable").ok().flatten().is_some_and(|v| v.eq_ignore_ascii_case("plaintext-only"))
            && plaintext_at(&dom, start.0)
    };
    if plain {
        insert_line_break(page, host);
    } else {
        insert_paragraph(page, host);
    }
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
        crate::edit_commands::inline_value::set_break_caret(page, (start, end), caret);
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

fn html_select(dom: &Dom, node: NodeId) -> bool {
    dom.element(node).is_ok_and(|el| el.is_html() && el.local_name() == &markup5ever::local_name!("select"))
}

/// A select is an atomic box in its containing editor. Option contents
/// have no editable paragraph position. A point directly on the select
/// maps to its outside edge; an internal option point falls back to the
/// start of its enclosing paragraph.
fn paragraph_point(dom: &Dom, host: NodeId, caret: Point) -> Option<Point> {
    let select_ancestor = |mut node| loop {
        if html_select(dom, node) { return Some(node); }
        if node == host { return None; }
        node = dom.parent(node).ok()??;
    };
    if let Some(select) = select_ancestor(caret.0) {
        if select == host { return None; }
        if caret.0 == select {
            return Some((dom.parent(select).ok()??, dom.index_in_parent(select).ok()? + u32::from(caret.1 != 0)));
        }
        return Some((block_of(dom, select, host), 0));
    }
    let point = insertion_point(dom, caret);
    if point != caret && select_ancestor(point.0).is_some() { Some(caret) } else { Some(point) }
}

/// These hosts must keep the edit inside themselves without creating a
/// child paragraph. Existing block descendants can still split normally.
fn cannot_contain_paragraph(dom: &Dom, host: NodeId) -> bool {
    let tag = local(dom, host);
    (!is_block(dom, host) && tag != "body")
        || matches!(tag.as_str(), "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "p" | "dt")
}

/// A semantic block host can split a descendant rendered as a paragraph,
/// including a block-styled span. A phrasing host such as span can instead
/// break the line inside that span without cloning it.
fn paragraph_for_split(page: &PageDom, dom: &Dom, host: NodeId, node: NodeId, constrained: bool) -> NodeId {
    let layout = page.layout();
    let block_host = is_block(dom, host) || local(dom, host) == "body";
    let mut el = if is_text(dom, node) { dom.parent_element(node).ok().flatten() } else { Some(node) };
    while let Some(n) = el {
        if n == host { break; }
        let semantic_block = is_block(dom, n);
        if !constrained && semantic_block {
            return n;
        }
        if (block_host || semantic_block) && is_container(dom, n)
            && !matches!(local(dom, n).as_str(), "button" | "select" | "textarea" | "object")
        {
            let display = layout.computed_style_value(dom, n, "display");
            if display == "block" || (semantic_block && display != "contents" && display != "inline" && !display.starts_with("inline-")) {
                return n;
            }
        }
        el = dom.parent_element(n).ok().flatten();
    }
    host
}

/// At a plaintext paragraph edge, a BR inside the inline formatting
/// keeps the empty line's typing style even after the selection moves away.
fn formatted_split_edge(dom: &Dom, block: NodeId, point: Point) -> Option<bool> {
    let (mut node, offset) = point;
    if !is_text(dom, node) || len(dom, node) == 0 { return None; }
    let trailing = offset == len(dom, node);
    if (!trailing && offset != 0) || dom.parent(node).ok()? == Some(block) { return None; }
    loop {
        let neighbor = if trailing { dom.next_sibling(node) } else { dom.prev_sibling(node) };
        if neighbor.ok()?.is_some() { return None; }
        node = dom.parent_element(node).ok()??;
        if node == block { return Some(trailing); }
        if !dom.element(node).ok()?.is_html() || !matches!(local(dom, node).as_str(),
            "b" | "strong" | "i" | "em" | "u" | "s" | "strike" | "sub" | "sup" | "font" | "span") {
            return None;
        }
    }
}

/// Enter on the placeholder of an empty list item exits one list level.
fn exit_empty_list_item(page: &PageDom, dom: &mut Dom, host: NodeId, item: NodeId) -> Option<Point> {
    if !dom.element(item).is_ok_and(|el| el.is_html() && el.local_name() == &markup5ever::local_name!("li"))
        || item == host || editing_host(dom, item) != Some(host)
    {
        return None;
    }
    match dom.child_count(item).ok()? {
        0 => {}
        1 if dom.first_child(item).ok()?.is_some_and(|child| local(dom, child) == "br" && editing_host(dom, child) == Some(host)) => {}
        _ => return None,
    }
    let list = dom.parent_element(item).ok()??;
    let list_element = |dom: &Dom, n| dom.element(n).is_ok_and(|el| el.is_html() && matches!(&**el.local_name(), "ol" | "ul"));
    if list == host || !list_element(dom, list) { return None; }
    let parent = dom.parent_element(list).ok()??;
    let nested_item = local(dom, parent) == "li";
    let destination = if nested_item {
        let outer = dom.parent_element(parent).ok()??;
        if parent == host || !list_element(dom, outer) || dom.next_sibling(list).ok()?.is_some() { return None; }
        outer
    } else { parent };
    if editing_host(dom, destination) != Some(host) { return None; }
    let doc = dom.node_document(item).ok()?;
    let tag = if list_element(dom, destination) { "li" } else { dom.document_data(doc).ok()?.default_paragraph_separator };
    let paragraph = dom.create_element(doc, tag).ok()?;
    let br = dom.create_element(doc, "br").ok()?;
    dom.append_child(paragraph, br).ok()?;

    // Preserve both runs of neighboring items. split_edge keeps the
    // original list and its ID on the left and moves the existing children.
    if nested_item {
        let next = dom.next_sibling(parent).ok()?;
        dom.insert_before(destination, list, next).ok()?;
    }
    // Source indentation between list items is not a separate list run.
    // Keep that text in the original list when a run survives.
    let has_content = |forward| -> Option<bool> {
        let mut node = item;
        loop {
            let sibling = if forward { dom.next_sibling(node) } else { dom.prev_sibling(node) }.ok()?;
            let Some(next) = sibling else { return Some(false) };
            if !dom.char_data(next).is_ok_and(|s| is_text(dom, next)
                && s.chars().all(|c| matches!(c, ' ' | '\t' | '\r' | '\n' | '\x0c'))) {
                return Some(true);
            }
            if !matches!(page.layout().computed_style_value(dom, list, "white-space-collapse").as_str(), "collapse") {
                return Some(true);
            }
            node = next;
        }
    };
    let preceding = has_content(false)?;
    let following = has_content(true)?;
    if preceding && following {
        crate::edit_commands::alignment::split_edge(dom, destination, item, true, &mut [])?;
    }
    let before = if preceding { dom.next_sibling(list).ok()? } else { Some(list) };
    dom.insert_before(destination, paragraph, before).ok()?;
    dom.remove_node(item).ok()?;
    if !preceding && !following { dom.remove_node(list).ok()?; }
    Some((paragraph, 0))
}

fn split_block(page: &PageDom, dom: &mut Dom, host: NodeId, caret: Point) -> Point {
    // A DOM caret inside an inline void has no child insertion position.
    // Split at the rendered object's edge so it travels with the right line.
    if let Some(point) = exit_empty_list_item(page, dom, host, caret.0) { return point; }
    let void = caret.0;
    let kind = dom.element(void).ok().filter(|el| el.is_html() && pyro_dom::serialize::is_void(el.local_name()))
        .map(|el| el.local_name().to_string());
    let mut terminal_break = false;
    let mut following_text = None;
    let point = kind.as_deref().and_then(|kind| {
        if void == host || !target_range::rendered(page, dom, void) { return None; }
        let parent = dom.parent_element(void).ok()??;
        let index = dom.index_in_parent(void).ok()?;
        match kind {
            "img" | "input" => Some((parent, index)),
            "br" | "wbr" | "embed" => {
                let next = dom.next_sibling(void).ok()?;
                if let Some(text) = next.filter(|&n| is_text(dom, n) && len(dom, n) != 0) {
                    if kind != "br" { following_text = Some(text); }
                    Some((parent, index))
                } else if next.is_none() {
                    terminal_break = true;
                    Some((parent, index + 1))
                } else { None }
            }
            _ => None,
        }
    });
    let Some(caret) = point.or_else(|| paragraph_point(dom, host, caret)) else { return caret };

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
    let constrained = cannot_contain_paragraph(dom, host);
    let mut block = paragraph_for_split(page, dom, host, caret.0, constrained);
    let mut caret = caret;
    let display = page.layout().computed_style_value(dom, block, "display");
    let inline_box = display == "inline" || display.starts_with("inline-");
    if block == host {
        if inline_box || constrained {
            return if crate::edit_commands::newlines_significant_in(page, dom, caret.0) {
                preserved_break_at(page, dom, host, caret)
            } else {
                line_break_at(dom, host, caret)
            };
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
    let new_block = if terminal_break && dom.parent(void).ok().flatten() == Some(block)
        && matches!(tag.as_str(), "h1" | "h2" | "h3" | "h4" | "h5" | "h6")
    {
        let separator = dom.document_data(doc).map(|d| d.default_paragraph_separator).unwrap_or("div").to_owned();
        dom.create_element(doc, &separator)
    } else {
        dom.clone_node(block, false, None)
    };
    let Ok(new_block) = new_block else {
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
    let formatted_edge = if plaintext_at(dom, host) { formatted_split_edge(dom, block, caret) } else { None };
    let text_parent = container;
    let mut empty_caret = None;
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
        if Some(parent) == text_parent {
            if let Some(trailing) = formatted_edge {
                let empty = if trailing { dest } else { parent };
                if dom.child_count(empty).ok() == Some(0) {
                    if let Ok(br) = dom.create_element(doc, "br") {
                        let _ = dom.append_child(empty, br);
                        if trailing { empty_caret = Some((empty, 0)); }
                    }
                }
            }
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
    if let Some(point) = empty_caret { return point; }
    if let Some(text) = following_text { return (text, 0); }
    placeholder_if_empty(dom, new_block).unwrap_or_else(|| paragraph_point(dom, new_block, (new_block, 0)).unwrap_or((new_block, 0)))
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
        let mut dom = page.dom.borrow_mut();
        let caret = whitespace::delete_after_spaces(page, &mut dom, host, start, end)
            .unwrap_or_else(|| delete_range(&mut dom, host, start, end, true));
        drop(dom);
        set_selection(page, caret, caret);
    });
}

/// Elements pasted or inserted markup never brings along.
const DROPPED_ON_INSERT: &[&str] = &["script", "style", "meta", "link", "title", "head", "base", "template"];

fn html_fragment_boundary(
    page: &PageDom, dom: &mut Dom, host: NodeId, parent: NodeId, before: Option<NodeId>,
) -> Option<(NodeId, Option<NodeId>)> {
    let mut outer = parent;
    while outer != host && dom.element(outer).ok()?.is_html()
        && page.layout().computed_style_value(dom, outer, "display") == "inline"
    {
        let formatting = match local(dom, outer).as_str() {
            "b" | "strong" | "i" | "em" | "s" | "strike" | "sub" | "sup" | "u" | "font" => true,
            "span" => dom.get_attribute(outer, "style").ok()?.is_some(),
            _ => false,
        };
        if !formatting { break; }
        outer = dom.parent_element(outer).ok()??;
    }
    let mut reference = before;
    if outer != parent {
        let edge = before.or_else(|| dom.last_child(parent).ok().flatten())?;
        let sibling = crate::edit_commands::alignment::split_edge(dom, outer, edge, before.is_none(), &mut [])?;
        reference = if before.is_some() { Some(sibling) } else { dom.next_sibling(sibling).ok()? };
    }
    // At either end of a link the inserted fragment belongs outside it.
    if outer != host && local(dom, outer) == "a" {
        let start = reference.is_some() && reference == dom.first_child(outer).ok()?;
        if start || reference.is_none() {
            reference = if start { Some(outer) } else { dom.next_sibling(outer).ok()? };
            outer = dom.parent_element(outer).ok()??;
        }
    }
    Some((outer, reference))
}

/// Moves `frag`'s children to the caret, splitting the text there; returns
/// the caret after them.
/// With `sanitize` (paste), event handler attributes and `javascript:` URLs
/// are dropped too, as Chromium sanitizes pasted markup: the clipboard may
/// hold markup another page wrote.
fn insert_fragment(page: &PageDom, dom: &mut Dom, host: NodeId, caret: Point, frag: NodeId, sanitize: bool) -> Point {
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
    let single_image = kids.len() == 1 && dom.element(last).is_ok_and(|el| el.is_html() && el.local_name() == &markup5ever::local_name!("img"));
    let split_text = is_text(dom, caret.0) && caret.1 > 0 && caret.1 < len(dom, caret.0);
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
    let (parent, before) = if !sanitize && kids.iter().all(|&n| is_text(dom, n) || !is_block(dom, n)) {
        html_fragment_boundary(page, dom, host, parent, before).unwrap_or((parent, before))
    } else { (parent, before) };
    // Content replaces an empty block's placeholder <br>.
    let placeholder =
        before.filter(|&b| local(dom, b) == "br" && dom.child_count(parent).unwrap_or(0) == 1 && parent != host);
    for k in kids {
        let _ = dom.insert_before(parent, k, before);
    }
    if single_image {
        whitespace::after_image_insertion(page, dom, host, last, split_text);
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
    let split_text = is_text(&dom, node) && offset > 0 && offset < len(&dom, node);
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
    if image_src.is_some() {
        whitespace::after_image_insertion(page, &mut dom, host, el, split_text);
    } else {
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
    // HTML conversion follows the receiving editor. In designMode the
    // body can own a selection inside an explicitly plaintext descendant.
    if plaintext_at(&page.dom.borrow(), host) {
        let Ok(text) = Dom::plaintext_from_html(html) else { return };
        let newline = crate::edit_commands::newlines_significant(page, start.0);
        edit(page, host, input_type, None, |page| {
            let mut dom = page.dom.borrow_mut();
            let mut caret = if start != end { delete_range(&mut dom, host, start, end, false) } else { start };
            for (index, line) in text.split('\n').enumerate() {
                if index != 0 {
                    caret = if newline { insert_at(&mut dom, caret, "\n") } else { line_break_at(&mut dom, host, caret) };
                }
                if !line.is_empty() {
                    caret = insert_at(&mut dom, caret, line);
                    whitespace::inserted(page, &mut dom, host, caret, line);
                }
            }
            drop(dom);
            set_selection(page, caret, caret);
        });
        return;
    }
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
                Ok(frag) => insert_fragment(page, &mut dom, host, caret, frag, false),
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
                        Ok(frag) => insert_fragment(page, &mut dom, host, caret, frag, true),
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
            let split_text = is_text(&dom, caret.0) && caret.1 > 0 && caret.1 < len(&dom, caret.0);
            let after = if newline {
                preserved_break_at(page, &mut dom, host, caret)
            } else {
                line_break_at(&mut dom, host, caret)
            };
            whitespace::after_line_break(page, &mut dom, host, after, split_text);
            after
        };
        crate::edit_commands::inline_value::set_break_caret(page, (start, end), caret);
    });
}

fn preserved_break_at(page: &PageDom, dom: &mut Dom, host: NodeId, caret: Point) -> Point {
    let Some(point) = paragraph_point(dom, host, caret) else { return caret };
    let at_end = whitespace::at_line_end(page, dom, host, point);
    let display = page.layout().computed_style_value(dom, host, "display");
    if !in_text_only_element(dom, point.0) && (cannot_contain_paragraph(dom, host) || display == "inline" || display.starts_with("inline-")) {
        // A preserved separator is a separate text node. Keep the caret at
        // the start of the following text or before the existing padding BR.
        let Ok(doc) = dom.node_document(host) else { return caret };
        let Ok(lf) = dom.create_text(doc, "\n") else { return caret };
        let Some(parent) = insert_break_node(dom, host, point, lf) else { return caret };
        if at_end {
            if let Ok(padding) = dom.create_text(doc, "\n") {
                let before = dom.next_sibling(lf).ok().flatten();
                let _ = dom.insert_before(parent, padding, before);
            }
        }
        return caret_after_break(dom, parent, lf);
    }
    // A padding BR in these editors already supplies the final empty line.
    let keep_padding = !is_text(dom, point.0) && plaintext_at(dom, point.0);
    let after = insert_at_point(dom, point, if at_end { "\n\n" } else { "\n" }, !keep_padding);
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
    let Some(caret) = paragraph_point(dom, host, caret) else { return caret };
    let Ok(doc) = dom.node_document(host) else { return caret };
    let Ok(br) = dom.create_element(doc, "br") else { return caret };
    let Some(parent) = insert_break_node(dom, host, caret, br) else { return caret };
    // Nothing follows on the line inside its block: a placeholder <br> keeps
    // the new line visible.
    let next = next_in_block(dom, host, br);
    if next.is_none_or(|n| is_block(dom, n)) {
        if let Ok(extra) = dom.create_element(doc, "br") {
            let after = dom.next_sibling(br).ok().flatten();
            let _ = dom.insert_before(parent, extra, after);
        }
    }
    caret_after_break(dom, parent, br)
}

fn insert_break_node(dom: &mut Dom, host: NodeId, caret: Point, node: NodeId) -> Option<NodeId> {
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
    dom.insert_before(parent, node, before).ok()?;
    Some(parent)
}

fn caret_after_break(dom: &Dom, parent: NodeId, node: NodeId) -> Point {
    match dom.next_sibling(node).ok().flatten() {
        Some(t) if is_text(dom, t) => (t, 0),
        _ => (parent, dom.index_in_parent(node).unwrap_or(0) + 1),
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
