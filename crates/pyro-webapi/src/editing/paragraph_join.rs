//! Preserve the first rendered line's inherited white-space when joining paragraphs.
use super::*;

fn adjacent(dom: &Dom, block: NodeId, forward: bool) -> Option<(NodeId, Vec<NodeId>)> {
    let mut node = block;
    let mut separators = Vec::new();
    loop {
        node = if forward { dom.next_sibling(node) } else { dom.prev_sibling(node) }.ok()??;
        if !is_text(dom, node)
            || !dom.char_data(node).ok()?.chars().all(|c| matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0c'))
        {
            return Some((node, separators));
        }
        separators.push(node);
    }
}

// An explicit break is consumed. A following block starts another paragraph
// and must stay in place.
enum LineEnd {
    Break(NodeId),
    Newline(NodeId, u32),
    Boundary(Point),
}

fn first_line(page: &PageDom, dom: &Dom, root: NodeId, host: NodeId) -> Option<LineEnd> {
    let layout = page.layout();
    for node in dom.descendants(root).ok()?.skip(1) {
        if is_text(dom, node) {
            let data = dom.char_data(node).ok()?;
            if let Some(at) = data.find('\n') {
                if crate::edit_commands::newlines_significant_in(page, dom, node) {
                    return Some(LineEnd::Newline(node, data[..at].encode_utf16().count() as u32));
                }
            }
            continue;
        }
        let display = layout.computed_style_value(dom, node, "display");
        if display == "block" {
            return Some(LineEnd::Boundary((dom.parent(node).ok()??, dom.index_in_parent(node).ok()?)));
        }
        if display != "inline"
            || layout.computed_style_value(dom, node, "visibility") != "visible"
            || editing_host(dom, node) != Some(host)
        {
            return None;
        }
        if local(dom, node) == "br" {
            return Some(LineEnd::Break(node));
        }
        // Splitting these formatting elements retains their own styles.
        // An element with an ID must not be cloned into both paragraphs.
        if !matches!(local(dom, node).as_str(), "b" | "i" | "em" | "strong" | "span" | "u" | "s" | "sub" | "sup")
            || dom.get_attribute(node, "id").ok()?.is_some()
        {
            return None;
        }
    }
    Some(LineEnd::Boundary((root, len(dom, root))))
}

fn extract_line(dom: &mut Dom, root: NodeId, end: LineEnd) -> Option<NodeId> {
    let mut point = match end {
        LineEnd::Break(br) => (dom.parent(br).ok()??, dom.index_in_parent(br).ok()?),
        LineEnd::Newline(node, offset) => (node, offset),
        LineEnd::Boundary(point) => point,
    };
    // Use the edge of the whole subtree when possible. This moves existing
    // formatting nodes and avoids creating empty clones before the boundary.
    while point.0 != root && point.1 == 0 {
        point = (dom.parent(point.0).ok()??, dom.index_in_parent(point.0).ok()?);
    }
    if point == (root, 0) && matches!(end, LineEnd::Boundary(_)) {
        return None;
    }
    let fragment = crate::range::contents(dom, (root, 0), point, true).ok()?;
    let mut emptied = match end {
        LineEnd::Break(br) => {
            let parent = dom.parent(br).ok()??;
            dom.remove_node(br).ok()?;
            parent
        }
        LineEnd::Newline(node, _) => {
            dom.replace_data(node, 0, 1, "").ok()?;
            node
        }
        LineEnd::Boundary(_) => return Some(fragment),
    };
    while emptied != root && len(dom, emptied) == 0 {
        let parent = dom.parent(emptied).ok()??;
        dom.remove_node(emptied).ok()?;
        emptied = parent;
    }
    Some(fragment)
}

pub(super) fn at_boundary(
    page: &PageDom,
    dom: &mut Dom,
    host: NodeId,
    start: Point,
    end: Point,
    backward: bool,
) -> Option<Point> {
    let right_to_left = start == end && backward;
    let mut block = block_of(dom, start.0, host);
    // The line to the left can be an anonymous block box. Its contents are
    // direct inline children of the host.
    if block == host {
        if right_to_left || start.0 == host {
            return None;
        }
        block = start.0;
        while dom.parent(block).ok()?? != host {
            block = dom.parent(block).ok()??;
        }
    }
    let block_edge =
        if right_to_left { deep_forward(dom, (block, 0)) } else { deep_backward(dom, (block, len(dom, block))) };
    let caret_edge = if right_to_left { deep_forward(dom, start) } else { deep_backward(dom, start) };
    if caret_edge != block_edge {
        return None;
    }
    let (other, separators) = adjacent(dom, block, !right_to_left)?;
    let (left, right) = if right_to_left { (other, block) } else { (block, other) };
    let parent = dom.parent(left).ok()??;
    let seam = deep_backward(dom, (left, len(dom, left)));
    let layout = page.layout();
    let left_block = matches!(local(dom, left).as_str(), "div" | "p")
        && layout.computed_style_value(dom, left, "display") == "block";
    let left_inline = parent == host
        && (is_text(dom, left)
            || (local(dom, left) == "span" && layout.computed_style_value(dom, left, "display") == "inline"));
    if !(left_block || left_inline)
        || !is_text(dom, seam.0)
        || (left != seam.0 && dom.parent(seam.0).ok()?? != left)
        || !matches!(local(dom, right).as_str(), "div" | "p")
        || layout.computed_style_value(dom, right, "display") != "block"
        || [left, right].into_iter().any(|n| editing_host(dom, n) != Some(host))
    {
        return None;
    }
    // Removing this paragraph must not join a second anonymous line on its
    // far side. Leave that layout to the general deletion path.
    if left_inline
        && dom
            .next_sibling(right)
            .ok()?
            .is_some_and(|next| is_text(dom, next) || layout.computed_style_value(dom, next, "display") != "block")
    {
        return None;
    }
    if start != end && deep_forward(dom, end) != deep_forward(dom, (right, 0)) {
        return None;
    }
    let parent_style = layout.computed_style_value(dom, parent, "white-space");
    if !separators.is_empty() && (!left_block || !matches!(parent_style.as_str(), "normal" | "nowrap")) {
        return None;
    }
    let source_style = layout.computed_style_value(dom, right, "white-space");
    let destination = if left_block { left } else { parent };
    let dest_style = layout.computed_style_value(dom, destination, "white-space");
    let left_style =
        if is_text(dom, left) { parent_style } else { layout.computed_style_value(dom, left, "white-space") };
    if source_style.is_empty() || source_style == left_style {
        return None;
    }
    if !is_text(dom, deep_forward(dom, (right, 0)).0) {
        return None;
    }
    let line = first_line(page, dom, right, host)?;
    let fragment = extract_line(dom, right, line)?;
    if len(dom, fragment) > 0 {
        let content = if source_style != dest_style {
            let span = dom.create_element(dom.node_document(right).ok()?, "span").ok()?;
            dom.set_attribute(span, "style", &format!("white-space: {source_style};")).ok()?;
            dom.append_child(span, fragment).ok()?;
            span
        } else {
            fragment
        };
        dom.insert_before(destination, content, (!left_block).then_some(right)).ok()?;
    }
    for separator in separators {
        dom.remove_node(separator).ok()?;
    }
    if len(dom, right) == 0 {
        dom.remove_node(right).ok()?;
    }
    Some(seam)
}
