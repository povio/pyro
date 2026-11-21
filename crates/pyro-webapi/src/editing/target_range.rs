//! Intended editing range captured before the beforeinput listeners run.
use super::*;

pub(super) fn for_input(page: &Rc<PageDom>, host: NodeId, input_type: &str) -> Option<crate::range::StaticRangeData> {
    if matches!(input_type, "historyUndo" | "historyRedo") {
        return None;
    }
    let (start, end) = selection_range(page)?;
    let (start, end) = {
        let dom = page.dom.borrow();
        if editing_host(&dom, start.0) != Some(host) || editing_host(&dom, end.0) != Some(host) {
            return None;
        }
        if start == end && matches!(input_type, "deleteContentBackward" | "deleteContentForward") {
            let (caret, span) = deletion_plan(page, &dom, host, start, input_type == "deleteContentBackward");
            span.unwrap_or((caret, caret))
        } else {
            (start, end)
        }
    };
    crate::range::StaticRangeData::from_points(page, start, end).ok()
}

fn character(dom: &Dom, p: Point, backward: bool) -> Option<(Point, Point)> {
    let (_, span) = super::character::step(dom.char_data(p.0).ok()?, p.1, backward);
    span.map(|(from, to)| ((p.0, from), (p.0, to)))
}

/// A hidden descendant cannot supply a visible editable character.
pub(super) fn rendered(page: &PageDom, dom: &Dom, node: NodeId) -> bool {
    let mut current = if dom.node_type(node).ok() == Some(NodeType::Element) {
        Some(node)
    } else {
        dom.parent_element(node).ok().flatten()
    };
    let layout = page.layout();
    if current.is_none_or(|el| layout.computed_style_value(dom, el, "visibility") != "visible") {
        return false;
    }
    while let Some(el) = current {
        if layout.computed_style_value(dom, el, "display") == "none" {
            return false;
        }
        current = dom.parent_element(el).ok().flatten();
    }
    true
}

pub(super) fn deletion_plan(
    page: &PageDom,
    dom: &Dom,
    host: NodeId,
    caret: Point,
    backward: bool,
) -> (Point, Option<(Point, Point)>) {
    let mut p = editable_edge(dom, caret, !backward);
    // Don't descend from a parent boundary into hidden content.
    if p != caret && !rendered(page, dom, p.0) {
        p = caret;
    }
    let mut normalized = caret;
    if is_text(dom, p.0) {
        if !rendered(page, dom, p.0) || editing_host(dom, p.0) != Some(host) {
            return (caret, Some((caret, caret)));
        }
        if let Ok(text) = dom.char_data(p.0) {
            let (offset, span) = super::character::step(text, p.1, backward);
            if p == caret {
                normalized = (p.0, offset);
            }
            p.1 = offset;
            if let Some((from, to)) = span {
                return (normalized, Some(((p.0, from), (p.0, to))));
            }
        }
    }
    let span = at_edge(page, dom, host, p, backward).map(|(from, to)| {
        // A paragraph boundary cannot merge through text-only elements or
        // across a table cell. Keep the advertised range collapsed there.
        if from.0 != to.0
            && (in_text_only_element(dom, from.0)
                || in_text_only_element(dom, to.0)
                || cell(dom, host, from.0) != cell(dom, host, to.0))
        {
            (p, p)
        } else {
            (from, to)
        }
    });
    (normalized, span.or_else(|| (normalized != caret).then_some((p, p))))
}

fn cell(dom: &Dom, host: NodeId, mut n: NodeId) -> Option<NodeId> {
    loop {
        if matches!(local(dom, n).as_str(), "td" | "th") {
            return Some(n);
        }
        if n == host {
            return None;
        }
        n = dom.parent_element(n).ok()??;
    }
}

fn at_edge(page: &PageDom, dom: &Dom, host: NodeId, p: Point, backward: bool) -> Option<(Point, Point)> {
    // Walk out of the inline ancestry at an edge, staying inside the host.
    let mut node = p.0;
    let mut sibling = if is_text(dom, node) {
        None
    } else {
        let index = if backward { p.1.checked_sub(1) } else { Some(p.1) };
        index.and_then(|i| dom.child_at(node, i).ok().flatten())
    };
    let mut skipped = false;
    let (sibling, edge) = loop {
        while sibling.is_none() && node != host {
            sibling = if backward { dom.prev_sibling(node) } else { dom.next_sibling(node) }.ok()?;
            node = dom.parent(node).ok()??;
        }
        let Some(candidate) = sibling else {
            return skipped.then_some((p, p));
        };
        if !rendered(page, dom, candidate) {
            // visibility can be overridden on descendants. Continue at a
            // visible leaf if this subtree has one; display:none cannot.
            let el = if dom.node_type(candidate).ok() == Some(NodeType::Element) {
                Some(candidate)
            } else {
                dom.parent_element(candidate).ok().flatten()
            };
            if el.is_some_and(|el| {
                page.layout().computed_style_value(dom, el, "display") != "none"
                    && page.layout().computed_style_value(dom, el, "visibility") != "visible"
            }) {
                let leaves = dom.descendants(candidate).ok()?.skip(1).filter(|&n| {
                    ((is_text(dom, n) && len(dom, n) > 0)
                        || (dom.node_type(n).ok() == Some(NodeType::Element) && !is_container(dom, n)))
                        && rendered(page, dom, n)
                });
                let visible = if backward { leaves.last() } else { leaves.into_iter().next() };
                if let Some(leaf) = visible {
                    if editing_host(dom, leaf) != Some(host) {
                        return Some((p, p));
                    }
                    sibling = Some(leaf);
                    node = dom.parent(leaf).ok()??;
                    continue;
                }
            }
            skipped = true;
            node = candidate;
            sibling = None;
            continue;
        }
        if editing_host(dom, candidate) != Some(host) {
            // Non-editable children are atomic to this editor. Remove their
            // element, if targeted, without ever modifying its descendants.
            let parent = dom.parent(candidate).ok()??;
            if editing_host(dom, parent) == Some(host)
                && !is_editable(dom, candidate)
                && dom.node_type(candidate).ok() == Some(NodeType::Element)
            {
                let block = block_of(dom, p.0, host);
                if backward
                    && is_block(dom, candidate)
                    && block != host
                    && dom.parent(block).ok()? == Some(parent)
                    && dom.next_sibling(candidate).ok()? == Some(block)
                    && placeholder_block(dom, block)
                {
                    return Some(((parent, dom.index_in_parent(block).ok()?), (block, len(dom, block))));
                }
                // A caret leaving editable text cannot cross a locked block
                // boundary to delete the whole neighboring paragraph.
                if is_text(dom, p.0) && len(dom, p.0) != 0
                    && page.layout().style().computed(dom, candidate).is_some_and(|s| s.box_kind == pyro_dom::style::BoxKind::Block)
                {
                    return Some((p, p));
                }
                let at = dom.index_in_parent(candidate).ok()?;
                return Some(((parent, at), (parent, at + 1)));
            }
            return Some((p, p));
        }
        let edge = editable_edge(dom, (candidate, if backward { len(dom, candidate) } else { 0 }), !backward);
        if !rendered(page, dom, edge.0) || (is_text(dom, edge.0) && len(dom, edge.0) == 0) {
            skipped = true;
            node = edge.0;
            sibling = None;
            continue;
        }
        if !is_text(dom, edge.0) && is_container(dom, edge.0) && block_of(dom, edge.0, host) == block_of(dom, p.0, host)
        {
            // An inline formatting shell can end at an empty child or at
            // a non-editable/replaced child. Continue from that boundary.
            let at = if backward { edge.1.checked_sub(1) } else { Some(edge.1) };
            sibling = at.and_then(|i| dom.child_at(edge.0, i).ok().flatten());
            node = edge.0;
            skipped = true;
            continue;
        }
        break (candidate, edge);
    };
    if block_of(dom, edge.0, host) != block_of(dom, p.0, host) {
        return Some(if backward { (edge, p) } else { (p, edge) });
    }
    if is_text(dom, edge.0) {
        return character(dom, edge, backward);
    }
    if !is_container(dom, sibling) {
        // A trailing break holding open its block is not a character. A
        // forward deletion can instead cross the next paragraph boundary.
        if local(dom, sibling) == "br" && next_in_block(dom, host, sibling).is_none_or(|n| is_block(dom, n)) {
            let block = block_of(dom, sibling, host);
            if !backward && block != host {
                let parent = dom.parent(block).ok()??;
                let after = (parent, dom.index_in_parent(block).ok()? + 1);
                if let Some((from, to)) = at_edge(page, dom, host, after, false) {
                    let next_block = block_of(dom, to.0, host);
                    if from != to && next_block != host && next_block != block {
                        return Some((p, to));
                    }
                }
            }
            return Some((p, p));
        }
        let parent = dom.parent(sibling).ok()??;
        let offset = dom.index_in_parent(sibling).ok()?;
        return Some(((parent, offset), (parent, offset + 1)));
    }
    None
}
