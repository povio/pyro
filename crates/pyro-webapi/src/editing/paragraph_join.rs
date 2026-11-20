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

pub(super) fn at_boundary(
    page: &PageDom,
    dom: &mut Dom,
    host: NodeId,
    start: Point,
    end: Point,
    backward: bool,
) -> Option<Point> {
    let block = block_of(dom, start.0, host);
    if block == host {
        return None;
    }
    let right_to_left = start == end && backward;
    let block_edge =
        if right_to_left { deep_forward(dom, (block, 0)) } else { deep_backward(dom, (block, len(dom, block))) };
    let caret_edge = if right_to_left { deep_forward(dom, start) } else { deep_backward(dom, start) };
    if caret_edge != block_edge {
        return None;
    }
    let (other, separators) = adjacent(dom, block, !right_to_left)?;
    let (left, right) = if right_to_left { (other, block) } else { (block, other) };
    // These ordinary flow containers can join their inline contents directly.
    if ![left, right]
        .into_iter()
        .all(|n| matches!(local(dom, n).as_str(), "div" | "p") && editing_host(dom, n) == Some(host))
    {
        return None;
    }
    let seam = deep_backward(dom, (left, len(dom, left)));
    let edge = deep_forward(dom, (right, 0));
    if start != end && deep_forward(dom, end) != edge {
        return None;
    }
    if !is_text(dom, seam.0) || dom.parent(seam.0).ok()?? != left {
        return None;
    }
    let layout = page.layout();
    if ![left, right].into_iter().all(|n| layout.computed_style_value(dom, n, "display") == "block") {
        return None;
    }
    if !separators.is_empty() {
        let parent = dom.parent(left).ok()??;
        if !matches!(layout.computed_style_value(dom, parent, "white-space").as_str(), "normal" | "nowrap") {
            return None;
        }
    }
    let source_style = layout.computed_style_value(dom, right, "white-space");
    if source_style.is_empty() || source_style == layout.computed_style_value(dom, left, "white-space") {
        return None;
    }

    // Limit this path to a line of direct text. Other inline markup needs its
    // own computed style retained while ancestors are split.
    let text = dom.child_at(right, 0).ok()??;
    if !is_text(dom, text) {
        return None;
    }
    let value = dom.char_data(text).ok()?;
    let newline = if value.contains('\n') && crate::edit_commands::newlines_significant_in(page, dom, text) {
        value.find('\n').map(|i| value[..i].encode_utf16().count() as u32)
    } else {
        None
    };
    let next = dom.next_sibling(text).ok()?;
    if let Some(next) = next.filter(|_| newline.is_none()) {
        let display = layout.computed_style_value(dom, next, "display");
        let visible_break = local(dom, next) == "br"
            && display == "inline"
            && layout.computed_style_value(dom, next, "visibility") == "visible"
            && editing_host(dom, next) == Some(host);
        if !visible_break && (local(dom, next) == "br" || display != "block") {
            return None;
        }
    }

    let wrapper = dom.create_element(dom.node_document(right).ok()?, "span").ok()?;
    dom.set_attribute(wrapper, "style", &format!("white-space: {source_style};")).ok()?;
    if let Some(offset) = newline {
        let rest = dom.split_text(text, offset).ok()?;
        dom.replace_data(rest, 0, 1, "").ok()?;
        if len(dom, rest) == 0 {
            dom.remove_node(rest).ok()?;
        }
    } else if let Some(br) = next.filter(|&n| local(dom, n) == "br") {
        dom.remove_node(br).ok()?;
    }
    if len(dom, text) > 0 {
        dom.append_child(wrapper, text).ok()?;
        dom.append_child(left, wrapper).ok()?;
    } else {
        dom.remove_node(text).ok()?;
    }
    for separator in separators {
        dom.remove_node(separator).ok()?;
    }
    if len(dom, right) == 0 {
        dom.remove_node(right).ok()?;
    }
    Some(seam)
}
