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
            deletion_span(&dom, host, start, input_type == "deleteContentBackward").unwrap_or((start, end))
        } else {
            (start, end)
        }
    };
    crate::range::StaticRangeData::from_points(page, start, end).ok()
}

pub(super) fn character(dom: &Dom, p: Point, backward: bool) -> Option<(Point, Point)> {
    let text = dom.char_data(p.0).ok()?;
    let index = if backward { p.1.checked_sub(1)? } else { p.1 };
    let unit = text.encode_utf16().nth(index as usize)?;
    let width = if backward {
        u32::from((0xdc00..=0xdfff).contains(&unit) && index > 0) + 1
    } else {
        u32::from((0xd800..=0xdbff).contains(&unit)) + 1
    };
    Some(if backward { ((p.0, p.1 - width), p) } else { (p, (p.0, p.1 + width)) })
}

pub(super) fn deletion_span(dom: &Dom, host: NodeId, caret: Point, backward: bool) -> Option<(Point, Point)> {
    let p = editable_edge(dom, caret, !backward);
    if is_text(dom, p.0) {
        if let Some(span) = character(dom, p, backward) {
            return Some(span);
        }
    }
    // Walk out of the inline ancestry at an edge, staying inside the host.
    let mut node = p.0;
    let mut sibling = if is_text(dom, node) {
        None
    } else {
        let index = if backward { p.1.checked_sub(1) } else { Some(p.1) };
        index.and_then(|i| dom.child_at(node, i).ok().flatten())
    };
    while sibling.is_none() && node != host {
        sibling = if backward { dom.prev_sibling(node) } else { dom.next_sibling(node) }.ok()?;
        node = dom.parent(node).ok()??;
    }
    let sibling = sibling?;
    if editing_host(dom, sibling) != Some(host) {
        return None;
    }
    let edge = editable_edge(dom, (sibling, if backward { len(dom, sibling) } else { 0 }), !backward);
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
                if let Some(next) = dom
                    .next_sibling(block)
                    .ok()
                    .flatten()
                    .filter(|&n| editing_host(dom, n) == Some(host) && is_block(dom, n))
                {
                    return Some((p, editable_edge(dom, (next, 0), true)));
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
