//! Paragraph alignment for the four justify commands. A raw line of inline
//! content in the editing host has no block of its own, so it needs a wrapper.

use std::rc::Rc;

use pyro_dom::stylo::declarations::DeclarationBlock;
use pyro_dom::{Dom, NodeId, NodeType};

use crate::editing::{self, Point};
use crate::page::PageDom;

pub(super) fn value_for(command: &str) -> Option<&'static str> {
    Some(match command {
        "justifyleft" => "left",
        "justifyright" => "right",
        "justifycenter" => "center",
        "justifyfull" => "justify",
        _ => return None,
    })
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Paragraph {
    Block(NodeId),
    Line { host: NodeId, block: NodeId, first: NodeId, last: NodeId, trailing_break: Option<NodeId> },
    Anonymous { host: NodeId, first: Option<NodeId>, last: Option<NodeId> },
}

pub(super) fn is_inline(page: &PageDom, dom: &Dom, node: NodeId) -> bool {
    dom.node_type(node).ok() != Some(NodeType::Element)
        || matches!(
            page.layout().computed_style_value(dom, node, "display").as_str(),
            "inline" | "inline-block" | "inline-flex" | "inline-grid" | "inline-table" | "ruby" | "contents" | "none"
        )
}

fn paragraph(page: &PageDom, dom: &Dom, host: NodeId, mut node: NodeId) -> Paragraph {
    let seed = node;
    let mut child = None;
    while node != host {
        if !is_inline(page, dom, node) {
            return line_at_break(page, dom, host, node, seed).unwrap_or(Paragraph::Block(node));
        }
        child = Some(node);
        let Ok(Some(parent)) = dom.parent(node) else { break };
        node = parent;
    }
    if let Some(line) = line_at_break(page, dom, host, host, seed) {
        return line;
    }
    let mut first = child;
    let mut last = child;
    while let Some(prev) = first.and_then(|n| dom.prev_sibling(n).ok().flatten()) {
        if !is_inline(page, dom, prev) || editing::local(dom, prev) == "br" {
            break;
        }
        first = Some(prev);
    }
    while let Some(next) = last.and_then(|n| dom.next_sibling(n).ok().flatten()) {
        if !is_inline(page, dom, next) || last.is_some_and(|n| editing::local(dom, n) == "br") {
            break;
        }
        last = Some(next);
    }
    Paragraph::Anonymous { host, first, last }
}

/// BRs split one visual paragraph even inside nested phrasing elements.
fn line_at_break(page: &PageDom, dom: &Dom, host: NodeId, block: NodeId, seed: NodeId) -> Option<Paragraph> {
    if !dom.descendants(block).ok()?.any(|n| editing::local(dom, n) == "br") {
        return None;
    }
    let mut todo: Vec<_> = dom.children(block).ok()?.collect();
    todo.reverse();
    let mut first = None;
    let mut last = None;
    let mut target = false;
    let mut preceded_by_break = false;
    while let Some(n) = todo.pop() {
        let is_break = editing::local(dom, n) == "br";
        if is_break || !is_inline(page, dom, n) || editing::editing_host(dom, n) != Some(host) {
            if (target || n == seed && is_break) && (preceded_by_break || is_break) {
                return Some(Paragraph::Line {
                    host,
                    block,
                    first: first.unwrap_or(n),
                    last: last.unwrap_or(n),
                    trailing_break: is_break.then_some(n),
                });
            }
            if target {
                return None;
            }
            preceded_by_break = is_break;
            first = None;
            last = None;
            continue;
        }
        if dom.child_count(n).ok()? == 0 {
            first.get_or_insert(n);
            last = Some(n);
            target |= n == seed || dom.contains(seed, n).unwrap_or(false);
        } else {
            let index = todo.len();
            todo.extend(dom.children(n).ok()?);
            todo[index..].reverse();
        }
    }
    (target && preceded_by_break).then(|| Paragraph::Line {
        host,
        block,
        first: first.unwrap(),
        last: last.unwrap(),
        trailing_break: None,
    })
}

pub(super) fn visible(page: &PageDom, dom: &Dom, node: NodeId) -> bool {
    let mut cur = if dom.node_type(node).ok() == Some(NodeType::Element) {
        Some(node)
    } else {
        dom.parent_element(node).ok().flatten()
    };
    let layout = page.layout();
    if cur
        .is_some_and(|el| matches!(layout.computed_style_value(dom, el, "visibility").as_str(), "hidden" | "collapse"))
    {
        return false;
    }
    while let Some(el) = cur {
        if layout.computed_style_value(dom, el, "display") == "none" {
            return false;
        }
        cur = dom.parent_element(el).ok().flatten();
    }
    true
}

pub(super) fn selected_paragraphs(page: &PageDom, dom: &Dom, host: NodeId, start: Point, end: Point) -> Vec<Paragraph> {
    let mut paragraphs = Vec::new();
    if start != end {
        for n in dom.descendants(host).into_iter().flatten() {
            if dom.child_count(n).ok() != Some(0) {
                continue;
            }
            if !editing::is_text(dom, n) && dom.node_type(n).ok() != Some(NodeType::Element) {
                continue;
            }
            let (begin, finish) = if editing::is_text(dom, n) {
                if editing::len(dom, n) == 0 {
                    continue;
                }
                ((n, 0), (n, editing::len(dom, n)))
            } else {
                let (Ok(Some(parent)), Ok(index)) = (dom.parent(n), dom.index_in_parent(n)) else { continue };
                ((parent, index), (parent, index + 1))
            };
            if crate::range::compare_boundary_points_dom(dom, finish, start) <= 0
                || crate::range::compare_boundary_points_dom(dom, begin, end) >= 0
            {
                continue;
            }
            if !visible(page, dom, n) {
                continue;
            }
            // A range can cross non-editable content and reach a nested
            // editing host. Restrict each leaf's mutation to its own host.
            let Some(leaf_host) = editing::editing_host(dom, n) else {
                continue;
            };
            let p = paragraph(page, dom, leaf_host, n);
            if !paragraphs.contains(&p) {
                paragraphs.push(p);
            }
        }
    }
    // A range between the end of one inline node and the start of its
    // sibling has no selected characters but still occupies a paragraph.
    if paragraphs.is_empty() {
        let mut node = editing::deep_forward(dom, start).0;
        if node == start.0 && !editing::is_text(dom, node) {
            node = dom
                .child_at(start.0, start.1)
                .ok()
                .flatten()
                .or_else(|| start.1.checked_sub(1).and_then(|i| dom.child_at(start.0, i).ok().flatten()))
                .unwrap_or(node);
        }
        if visible(page, dom, node) && editing::editing_host(dom, node) == Some(host) {
            paragraphs.push(paragraph(page, dom, host, node));
        }
    }
    paragraphs
}

fn resolved(page: &PageDom, dom: &Dom, p: Paragraph) -> &'static str {
    let el = match p {
        Paragraph::Block(n) | Paragraph::Line { block: n, .. } => n,
        Paragraph::Anonymous { host, .. } => host,
    };
    let layout = page.layout();
    let align = layout.computed_style_value(dom, el, "text-align");
    match align.trim_start_matches("-webkit-").trim_start_matches("-moz-") {
        "center" => "center",
        "justify" => "justify",
        "right" => "right",
        "left" => "left",
        value => {
            let rtl = layout.computed_style_value(dom, el, "direction") == "rtl";
            if rtl != (value == "end") {
                "right"
            } else {
                "left"
            }
        }
    }
}

pub(super) fn selected_values(page: &PageDom) -> Vec<&'static str> {
    let Some((start, end)) = editing::selection_range(page) else { return Vec::new() };
    let dom = page.dom.borrow();
    let Some(host) = editing::editing_host(&dom, start.0) else { return Vec::new() };
    selected_paragraphs(page, &dom, host, start, end).into_iter().map(|p| resolved(page, &dom, p)).collect()
}

fn remap(point: &mut Point, host: NodeId, wrapper: NodeId, start: u32, count: u32) {
    if point.0 == host && point.1 >= start {
        if point.1 <= start + count {
            *point = (wrapper, point.1 - start);
        } else {
            point.1 -= count - 1;
        }
    }
}

/// Split the inline containers at an edge of the line. The original keeps
/// its ID and the cloned right-hand portion keeps the remaining attributes.
pub(super) fn split_edge(dom: &mut Dom, block: NodeId, mut edge: NodeId, after: bool, points: &mut [Point; 2]) -> Option<NodeId> {
    while let Some(parent) = dom.parent(edge).ok()? {
        if parent == block {
            return Some(edge);
        }
        let gp = dom.parent(parent).ok()??;
        let cut = dom.index_in_parent(edge).ok()? + u32::from(after);
        let count = dom.child_count(parent).ok()?;
        if cut > 0 && cut < count {
            let right = dom.clone_node(parent, false, None).ok()?;
            dom.remove_attribute(right, "id").ok()?;
            let next = dom.next_sibling(parent).ok()?;
            dom.insert_before(gp, right, next).ok()?;
            let mut cur = dom.child_at(parent, cut).ok()?;
            while let Some(child) = cur {
                cur = dom.next_sibling(child).ok()?;
                dom.append_child(right, child).ok()?;
            }
            for p in &mut *points {
                if p.0 == parent && p.1 > cut {
                    *p = (right, p.1 - cut);
                } else if p.0 == gp && p.1 > dom.index_in_parent(parent).ok()? {
                    p.1 += 1;
                }
            }
            edge = if after { parent } else { right };
        } else {
            edge = parent;
        }
    }
    None
}

pub(super) fn align_line(
    dom: &mut Dom,
    host: NodeId,
    block: NodeId,
    first: NodeId,
    last: NodeId,
    trailing_break: Option<NodeId>,
    points: &mut [Point; 2],
) -> Option<NodeId> {
    // A previous selected line may have split the original block. Locate
    // its current copy from this line's untouched leaf.
    let block = if dom.contains(block, first).ok()? { block } else { editing::block_of(dom, first, host) };
    let split_block =
        block != host && matches!(editing::local(dom, block).as_str(), "p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6");
    let parent = if split_block { dom.parent(block).ok()?? } else { block };
    let left = split_edge(dom, parent, first, false, points)?;
    let right = split_edge(dom, parent, last, true, points)?;
    let el = if split_block {
        left
    } else {
        let doc = dom.node_document(parent).ok()?;
        let el = dom.create_element(doc, "div").ok()?;
        let at = dom.index_in_parent(left).ok()?;
        dom.insert_before(parent, el, Some(left)).ok()?;
        let mut next = Some(left);
        let mut count = 0;
        while let Some(n) = next {
            next = dom.next_sibling(n).ok()?;
            dom.append_child(el, n).ok()?;
            count += 1;
            if n == right {
                break;
            }
        }
        for point in &mut *points {
            remap(point, parent, el, at, count);
        }
        el
    };
    // The new block supplies the newline. Keep a lone BR as the placeholder
    // when the selected line itself was empty.
    if let Some(br) = trailing_break.filter(|&br| br != first) {
        let mut child = br;
        while let Some(p) = dom.parent(child).ok()? {
            let ix = dom.index_in_parent(child).ok()?;
            for point in &mut *points {
                if point.0 == child {
                    *point = (p, ix);
                } else if point.0 == p && point.1 > ix {
                    point.1 -= 1;
                }
            }
            dom.remove_child(p, child).ok()?;
            if p == host || p == parent || p == el || dom.child_count(p).ok()? > 0 {
                break;
            }
            child = p;
        }
    }
    Some(el)
}

pub(super) fn apply(page: &Rc<PageDom>, host: NodeId, target: &'static str) {
    let Some((start, end)) = editing::selection_range(page) else { return };
    let Some((anchor, focus)) = editing::selection(page) else { return };
    let mut points = [anchor, focus];
    let changes: Vec<_> = {
        let dom = page.dom.borrow();
        selected_paragraphs(page, &dom, host, start, end)
            .into_iter()
            .filter(|&p| resolved(page, &dom, p) != target)
            .collect()
    };
    if changes.is_empty() {
        return;
    }
    let input_type = match target {
        "left" => "formatJustifyLeft",
        "right" => "formatJustifyRight",
        "center" => "formatJustifyCenter",
        _ => "formatJustifyFull",
    };
    super::record_host_edit(page, host, input_type);
    let mut dom = page.dom.borrow_mut();
    let Ok(doc) = dom.node_document(host) else { return };
    for p in changes {
        let el = match p {
            Paragraph::Block(el) => el,
            Paragraph::Line { host, block, first, last, trailing_break } => {
                let Some(el) = align_line(&mut dom, host, block, first, last, trailing_break, &mut points) else {
                    continue;
                };
                el
            }
            Paragraph::Anonymous { host, first, last } => {
                let Ok(wrapper) = dom.create_element(doc, "div") else { continue };
                let at = first.and_then(|n| dom.index_in_parent(n).ok()).unwrap_or(0);
                if dom.insert_before(host, wrapper, first).is_err() {
                    continue;
                }
                let mut next = first;
                let mut count = 0;
                while let Some(n) = next {
                    next = dom.next_sibling(n).ok().flatten();
                    if dom.append_child(wrapper, n).is_err() {
                        break;
                    }
                    count += 1;
                    if Some(n) == last {
                        break;
                    }
                }
                if count > 0 {
                    for point in &mut points {
                        remap(point, host, wrapper, at, count);
                    }
                } else if let Ok(br) = dom.create_element(doc, "br") {
                    let _ = dom.append_child(wrapper, br);
                    points = [(wrapper, 0); 2];
                }
                wrapper
            }
        };
        let mut style = DeclarationBlock::parse(dom.get_attribute(el, "style").ok().flatten().unwrap_or(""));
        style.set("text-align", target, false);
        let _ = dom.set_attribute(el, "style", &style.css_text());
        let _ = dom.remove_attribute(el, "align");
    }
    drop(dom);
    editing::set_selection(page, points[0], points[1]);
    super::note_host_edited(page, host);
    editing::input_event(page, host, "input", input_type, None, false);
}
