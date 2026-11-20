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

fn formatting_element(dom: &Dom, n: NodeId) -> bool {
    dom.element(n).is_ok_and(|el| el.is_html())
        && matches!(local(dom, n).as_str(), "b" | "i" | "em" | "strong" | "span" | "u" | "s" | "sub" | "sup")
}

fn before_node(dom: &Dom, n: NodeId) -> Option<Point> {
    Some((dom.parent(n).ok()??, dom.index_in_parent(n).ok()?))
}

/// Inspect a single node without crossing block and formatting boundaries.
fn line_end_at(page: &PageDom, dom: &Dom, node: NodeId, host: NodeId) -> Result<Option<LineEnd>, ()> {
    if is_text(dom, node) {
        let data = dom.char_data(node).map_err(|_| ())?;
        if let Some(at) = data.find('\n') {
            if crate::edit_commands::newlines_significant_in(page, dom, node) {
                return Ok(Some(LineEnd::Newline(node, data[..at].encode_utf16().count() as u32)));
            }
        }
        return Ok(None);
    }
    let layout = page.layout();
    let display = layout.computed_style_value(dom, node, "display");
    if display == "block" {
        return before_node(dom, node).map(LineEnd::Boundary).map(Some).ok_or(());
    }
    if display != "inline"
        || layout.computed_style_value(dom, node, "visibility") != "visible"
        || editing_host(dom, node) != Some(host)
    {
        return Err(());
    }
    if local(dom, node) == "br" {
        return Ok(Some(LineEnd::Break(node)));
    }
    if !formatting_element(dom, node) {
        return Err(());
    }
    Ok(None)
}

fn first_line(page: &PageDom, dom: &Dom, root: NodeId, host: NodeId) -> Option<LineEnd> {
    for node in dom.descendants(root).ok()?.skip(1) {
        if let Some(end) = line_end_at(page, dom, node, host).ok()? {
            return Some(end);
        }
        if !is_text(dom, node) && dom.get_attribute(node, "id").ok()?.is_some() {
            return None;
        }
    }
    Some(LineEnd::Boundary((root, len(dom, root))))
}

fn extract_line(dom: &mut Dom, root: NodeId, start: Point, end: LineEnd) -> Option<NodeId> {
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
    if point == start && matches!(end, LineEnd::Boundary(_)) {
        return None;
    }
    let fragment = crate::range::contents(dom, start, point, true).ok()?;
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
        if start.0 == host {
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
    let at_edge = caret_edge == block_edge;
    if !at_edge && right_to_left {
        return None;
    }
    let (other, separators) = adjacent(dom, block, !right_to_left)?;
    let (left, right) = if right_to_left { (other, block) } else { (block, other) };
    let parent = dom.parent(left).ok()??;
    let seam = deep_backward(dom, (left, len(dom, left)));
    let layout = page.layout();
    let left_block = matches!(local(dom, left).as_str(), "div" | "p")
        && layout.computed_style_value(dom, left, "display") == "block";
    if left_block && parent == host && !matches!(local(dom, right).as_str(), "div" | "p") && separators.is_empty() {
        if !is_text(dom, right)
            && (!formatting_element(dom, right) || layout.computed_style_value(dom, right, "display") != "inline")
        {
            return None;
        }
        if editing_host(dom, left) != Some(host)
            || layout.computed_style_value(dom, left, "visibility") != "visible"
            || !is_text(dom, deep_forward(dom, (right, 0)).0)
            || (start != end && deep_forward(dom, end) != deep_forward(dom, (right, 0)))
        {
            return None;
        }
        // Hidden trailing siblings have no caret position. Preserve them
        // and place the caret at the last rendered text in the left block.
        let mut children: Vec<_> = dom.children(left).ok()?.collect();
        let visible_seam = loop {
            let n = children.pop()?;
            if is_text(dom, n) {
                if !dom.char_data(n).ok()?.is_empty() {
                    break (n, len(dom, n));
                }
                continue;
            }
            let display = layout.computed_style_value(dom, n, "display");
            if display == "none" {
                continue;
            }
            if display != "inline"
                || !formatting_element(dom, n)
                || layout.computed_style_value(dom, n, "visibility") != "visible"
                || editing_host(dom, n) != Some(host)
            {
                return None;
            }
            children.extend(dom.children(n).ok()?);
        };
        if !right_to_left && caret_edge != visible_seam {
            return None;
        }
        let right_el = if is_text(dom, right) { host } else { right };
        if layout.computed_style_value(dom, right_el, "visibility") != "visible" {
            return Some(visible_seam);
        }
        return join_anonymous_right(page, dom, host, left, right, visible_seam);
    }
    if !at_edge {
        return None;
    }
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
    let fragment = extract_line(dom, right, (right, 0), line)?;
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

/// Properties of editable text whose resolved values can change when an
/// inline element leaves an ancestor that supplied its formatting.
const TEXT_STYLE: &[&str] = &[
    "white-space",
    "font-family",
    "font-size",
    "font-weight",
    "font-style",
    "color",
    "letter-spacing",
    "word-spacing",
    "text-transform",
    "direction",
];

fn text_style(page: &PageDom, dom: &Dom, node: NodeId) -> Option<Vec<String>> {
    let el = if is_text(dom, node) { dom.parent_element(node).ok()?? } else { node };
    let layout = page.layout();
    Some(TEXT_STYLE.iter().map(|name| layout.computed_style_value(dom, el, name)).collect())
}

/// Retain the author's declaration text (including custom and unsupported
/// properties). Add only the declarations affected by moving this node.
fn preserve_style(page: &PageDom, dom: &mut Dom, node: NodeId, source: &[String]) -> Option<()> {
    let after = text_style(page, dom, node)?;
    let differences: Vec<_> = TEXT_STYLE
        .iter()
        .zip(source)
        .zip(&after)
        .filter_map(|((&name, want), got)| (!want.is_empty() && want != got).then_some((name, want)))
        .collect();
    if differences.is_empty() {
        return Some(());
    }
    let el = if is_text(dom, node) {
        let parent = dom.parent_element(node).ok()??;
        let span = dom.create_element(dom.node_document(node).ok()?, "span").ok()?;
        dom.insert_before(parent, span, Some(node)).ok()?;
        dom.append_child(span, node).ok()?;
        span
    } else {
        node
    };
    let mut style = dom.get_attribute(el, "style").ok()?.unwrap_or("").to_owned();
    let append = |style: &mut String, name: &str, value: &str, important: bool| {
        if !style.is_empty() && !style.trim_end().ends_with(';') {
            style.push(';');
        }
        if !style.is_empty() {
            style.push(' ');
        }
        style.push_str(name);
        style.push_str(": ");
        style.push_str(value);
        if important {
            style.push_str(" !important");
        }
        style.push(';');
    };
    for &(name, value) in &differences {
        append(&mut style, name, value, false);
    }
    dom.set_attribute(el, "style", &style).ok()?;
    // A contextual important rule on the destination may take precedence.
    let mut overridden = false;
    for (name, value) in differences {
        if page.layout().computed_style_value(dom, el, name) != *value {
            append(&mut style, name, value, true);
            overridden = true;
        }
    }
    if overridden {
        dom.set_attribute(el, "style", &style).ok()?;
    }
    Some(())
}

/// Join just the first rendered line of inline siblings following a block.
/// Preflight the affected subtree and record its text styling in preorder
/// before changing the DOM. Range extraction moves fully contained nodes
/// and clones only partially contained formatting ancestors.
fn join_anonymous_right(
    page: &PageDom,
    dom: &mut Dom,
    host: NodeId,
    left: NodeId,
    right: NodeId,
    seam: Point,
) -> Option<Point> {
    let start = (host, dom.index_in_parent(right).ok()?);
    let left_style = page.layout().computed_style_value(dom, left, "white-space");
    let right_element = if is_text(dom, right) { host } else { right };
    let right_style = page.layout().computed_style_value(dom, right_element, "white-space");
    if right_style.is_empty() || left_style == right_style {
        return None;
    }
    let mut node = right;
    let mut styles = Vec::new();
    let end = 'siblings: loop {
        for descendant in dom.descendants(node).ok()? {
            let hidden_text = is_text(dom, descendant)
                && page.layout().computed_style_value(dom, dom.parent_element(descendant).ok()??, "visibility")
                    != "visible";
            let boundary = if editing_host(dom, descendant) != Some(host) || hidden_text {
                Some(LineEnd::Boundary(before_node(dom, descendant)?))
            } else {
                line_end_at(page, dom, descendant, host).ok()?
            };
            if let Some(end) = boundary {
                // IDs on partial ancestors must not be duplicated. Fully
                // contained nodes move with their identity and attributes.
                let mut ancestor = dom.parent_element(descendant).ok()?;
                while let Some(el) = ancestor {
                    if el == host {
                        break;
                    }
                    if dom.get_attribute(el, "id").ok()?.is_some() {
                        return None;
                    }
                    ancestor = dom.parent_element(el).ok()?;
                }
                if matches!(end,LineEnd::Newline(_,offset) if offset>0) {
                    styles.push(text_style(page, dom, descendant)?);
                }
                break 'siblings end;
            }
            styles.push(text_style(page, dom, descendant)?);
        }
        let Some(next) = dom.next_sibling(node).ok()? else {
            break LineEnd::Boundary((host, dom.index_in_parent(node).ok()? + 1));
        };
        node = next;
    };
    let fragment = extract_line(dom, host, start, end)?;
    let moved: Vec<_> = dom.descendants(fragment).ok()?.skip(1).collect();
    dom.append_child(left, fragment).ok()?;
    for (child, values) in moved.into_iter().zip(styles) {
        preserve_style(page, dom, child, &values)?;
    }
    Some(seam)
}
