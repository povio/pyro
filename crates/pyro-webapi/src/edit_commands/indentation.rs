//! Structural indent and outdent of the paragraphs intersecting a selection.
use super::alignment::{self, Paragraph};
use crate::{
    editing::{self, Point},
    page::PageDom,
};
use pyro_dom::stylo::declarations::DeclarationBlock;
use pyro_dom::{Dom, NodeId};
use std::rc::Rc;

fn property_of_indent(s: &str) -> bool {
    ["margin", "border", "padding"]
        .iter()
        .any(|p| s == *p || s.strip_prefix(*p).is_some_and(|rest| rest.starts_with('-')))
}

fn is_indent(dom: &Dom, el: NodeId) -> bool {
    if !dom.element(el).is_ok_and(|e| e.is_html()) {
        return false;
    }
    match editing::local(dom, el).as_str() {
        "blockquote" => true,
        "div" => dom.get_attribute(el, "style").ok().flatten().is_some_and(|s| {
            DeclarationBlock::parse(s).names().iter().any(|s| s == "margin" || s.starts_with("margin-"))
        }),
        _ => false,
    }
}

/// An indentation element can be joined if its only attributes are dir
/// and an inline style containing only indentation-related properties.
fn simple_indent(dom: &Dom, node: NodeId) -> bool {
    is_indent(dom, node)
        && dom.attributes(node).is_ok_and(|attrs| {
            attrs.iter().all(|a| {
                a.name.ns.is_empty()
                    && match &*a.name.local {
                        "dir" => true,
                        "style" => {
                            DeclarationBlock::parse(&a.value).names().iter().all(|name| property_of_indent(name))
                        }
                        _ => false,
                    }
            })
        })
}

fn list(dom: &Dom, n: NodeId) -> bool {
    matches!(editing::local(dom, n).as_str(), "ol" | "ul")
}

fn ancestor(dom: &Dom, mut n: NodeId, host: NodeId, predicate: impl Fn(&Dom, NodeId) -> bool) -> Option<NodeId> {
    while n != host {
        if editing::editing_host(dom, n) != Some(host) {
            break;
        }
        if predicate(dom, n) {
            return Some(n);
        }
        n = dom.parent_element(n).ok()??;
    }
    None
}

fn run(dom: &Dom, first: NodeId, last: NodeId) -> Vec<NodeId> {
    let mut nodes = vec![first];
    let mut n = first;
    while n != last {
        let Some(next) = dom.next_sibling(n).ok().flatten() else {
            return Vec::new();
        };
        n = next;
        nodes.push(n);
    }
    nodes
}

fn wrapper_for(dom: &Dom, node: NodeId, host: NodeId) -> Option<NodeId> {
    let parent = dom.parent_element(node).ok()??;
    ancestor(dom, parent, host, |d, n| is_indent(d, n) || list(d, n))
}

pub(super) fn unwrap(dom: &mut Dom, el: NodeId, points: &mut [Point; 2]) -> Option<()> {
    let p = dom.parent(el).ok()??;
    let at = dom.index_in_parent(el).ok()?;
    let count = dom.child_count(el).ok()?;
    for point in &mut *points {
        if point.0 == el {
            *point = (p, at + point.1);
        } else if point.0 == p && point.1 > at {
            point.1 = point.1 - 1 + count;
        }
    }
    while let Some(child) = dom.first_child(el).ok()? {
        dom.insert_before(p, child, Some(el)).ok()?;
    }
    dom.remove_child(p, el).ok()?;
    Some(())
}

pub(super) fn rename(dom: &mut Dom, node: NodeId, tag: &str, points: &mut [Point; 2]) -> Option<NodeId> {
    let parent = dom.parent(node).ok()??;
    let el = dom.create_element(dom.node_document(parent).ok()?, tag).ok()?;
    for a in dom.attributes(node).ok()?.to_vec() {
        let ns = (!a.name.ns.is_empty()).then_some(&*a.name.ns);
        dom.set_attribute_ns(el, ns, &a.qualified_name(), &a.value).ok()?;
    }
    while let Some(n) = dom.first_child(node).ok()? {
        dom.append_child(el, n).ok()?;
    }
    dom.replace_child(parent, el, node).ok()?;
    for p in points {
        if p.0 == node {
            p.0 = el;
        }
    }
    Some(el)
}

fn indent(dom: &mut Dom, nodes: &[NodeId], host: NodeId, points: &mut [Point; 2]) -> Option<()> {
    let first = *nodes.first()?;
    let last = *nodes.last()?;
    let parent = dom.parent(first).ok()??;
    let at = dom.index_in_parent(first).ok()?;
    let tag = if list(dom, parent) { editing::local(dom, parent) } else { "blockquote".into() };
    // Only join simple adjacent wrappers. Keep cited or independently
    // styled quotations as distinct elements.
    let simple = |n: NodeId| {
        editing::editing_host(dom, n) == Some(host)
            && if tag == "blockquote" { simple_indent(dom, n) } else { list(dom, n) && editing::local(dom, n) == tag }
    };
    let left = dom.prev_sibling(first).ok()?.filter(|&n| simple(n));
    let right = dom.next_sibling(last).ok()?.filter(|&n| simple(n));
    let created = left.is_none() && right.is_none();
    let wrapper = if let Some(el) = left.or(right) {
        el
    } else {
        let el = dom.create_element(dom.node_document(parent).ok()?, &tag).ok()?;
        dom.insert_before(parent, el, Some(first)).ok()?;
        el
    };
    let offset = if left.is_some() { dom.child_count(wrapper).ok()? } else { 0 };
    let before = if right == Some(wrapper) { dom.first_child(wrapper).ok()? } else { None };
    for &n in nodes {
        dom.insert_before(wrapper, n, before).ok()?;
    }
    let count = nodes.len() as u32;
    for point in &mut *points {
        if point.0 == parent && point.1 >= at {
            if point.1 <= at + count {
                *point = (wrapper, offset + point.1 - at);
            } else {
                point.1 = point.1 - count + u32::from(created);
            }
        } else if point.0 == wrapper && before.is_some() {
            point.1 += count;
        }
    }
    if left.is_some() {
        if let Some(next) = right {
            let existing = dom.child_count(wrapper).ok()?;
            let index = dom.index_in_parent(next).ok()?;
            for point in &mut *points {
                if point.0 == next {
                    *point = (wrapper, existing + point.1);
                } else if point.0 == parent && point.1 > index {
                    point.1 -= 1;
                }
            }
            while let Some(child) = dom.first_child(next).ok()? {
                dom.append_child(wrapper, child).ok()?;
            }
            dom.remove_child(parent, next).ok()?;
        }
    }
    Some(())
}

fn outdent(dom: &mut Dom, nodes: &[NodeId], host: NodeId, points: &mut [Point; 2]) -> Option<()> {
    let first = *nodes.first()?;
    let last = *nodes.last()?;
    let outer = wrapper_for(dom, first, host)?;
    let parent = dom.parent_element(outer).ok()??;
    // Split only the ancestor portions outside the selected paragraphs.
    // A boundary at the split can be expressed as either the end of
    // the left list or the beginning of its right fragment. Keep selected
    // element offsets attached to the selected run.
    let offsets = points.map(|(n, off)| {
        if n != outer || dom.parent(first).ok()? != Some(outer) || dom.parent(last).ok()? != Some(outer) {
            return None;
        }
        let start = dom.index_in_parent(first).ok()?;
        (off >= start && off <= dom.index_in_parent(last).ok()? + 1).then(|| off - start)
    });
    let right_item = if dom.parent(last).ok()? == Some(outer) { dom.next_sibling(last).ok()? } else { None };
    let middle = alignment::split_edge(dom, parent, first, false, points)?;
    alignment::split_edge(dom, parent, last, true, points)?;
    for (p, offset) in points.iter_mut().zip(offsets) {
        if let Some(offset) = offset {
            *p = (middle, offset);
        }
    }
    if list(dom, middle) {
        let items: Vec<_> = dom.children(middle).ok()?.collect();
        if parent != host
            && editing::local(dom, parent) == "li"
            && dom
                .parent_element(parent)
                .ok()?
                .is_some_and(|n| list(dom, n) && editing::editing_host(dom, n) == Some(host))
        {
            let outer_list = dom.parent_element(parent).ok()??;
            let before = dom.next_sibling(parent).ok()?;
            let at = dom.index_in_parent(parent).ok()? + 1;
            let count = items.len() as u32;
            // Only the remainder split from this list belongs after the
            // promoted items. Other children of the containing li (possibly
            // non-editable) retain their original parent and identity.
            let remainder = right_item
                .and_then(|n| dom.parent_element(n).ok().flatten())
                .filter(|&n| n != middle && dom.parent_element(n).ok().flatten() == Some(parent));
            for point in &mut *points {
                if point.0 == middle {
                    *point = (outer_list, at + point.1);
                } else if point.0 == outer_list && point.1 >= at {
                    point.1 += count + u32::from(remainder.is_some());
                }
            }
            for n in items {
                dom.insert_before(outer_list, n, before).ok()?;
            }
            if let Some(tail) = remainder {
                let index = dom.index_in_parent(tail).ok()?;
                for point in &mut *points {
                    if point.0 == parent && point.1 > index {
                        point.1 -= 1;
                    }
                }
                dom.insert_before(outer_list, tail, before).ok()?;
            }
            return unwrap(dom, middle, points);
        }
        if !list(dom, parent) {
            let separator = dom.document_data(dom.node_document(parent).ok()?).ok()?.default_paragraph_separator;
            for n in items {
                if editing::local(dom, n) == "li" {
                    rename(dom, n, separator, points)?;
                }
            }
        }
        unwrap(dom, middle, points)
    } else {
        dom.remove_attribute(middle, "dir").ok()?;
        if let Some(text) = dom.get_attribute(middle, "style").ok()? {
            let mut s = DeclarationBlock::parse(text);
            for name in s.names() {
                if property_of_indent(&name) {
                    s.remove(&name);
                }
            }
            if s.is_empty() {
                dom.remove_attribute(middle, "style").ok()?;
            } else {
                dom.set_attribute(middle, "style", &s.css_text()).ok()?;
            }
        }
        if dom.attributes(middle).ok()?.is_empty() {
            unwrap(dom, middle, points)
        } else {
            if editing::local(dom, middle) != "div" {
                rename(dom, middle, "div", points)?;
            }
            Some(())
        }
    }
}

pub(super) fn apply(page: &Rc<PageDom>, host: NodeId, increase: bool) {
    let Some((s, e)) = editing::selection_range(page) else {
        return;
    };
    let Some((a, f)) = editing::selection(page) else {
        return;
    };
    let mut points = [a, f];
    if s == e && s.1 > 0 {
        let dom = page.dom.borrow();
        let siblings = dom.child_at(s.0, s.1 - 1).ok().flatten().zip(dom.child_at(s.0, s.1).ok().flatten());
        // A boundary between two block siblings lies in neither paragraph.
        if siblings.is_some_and(|(left, right)| {
            !alignment::is_inline(page, &dom, left) && !alignment::is_inline(page, &dom, right)
        }) {
            return;
        }
    }
    let paragraphs = alignment::selected_paragraphs(page, &page.dom.borrow(), host, s, e);
    let input_type = if increase { "formatIndent" } else { "formatOutdent" };
    let saved = super::save(page, host);
    let mut dom = page.dom.borrow_mut();
    let mut groups: Vec<(NodeId, Vec<NodeId>)> = Vec::new();
    for para in paragraphs {
        let seed = match para {
            Paragraph::Block(n) => n,
            Paragraph::Line { first, .. } => first,
            Paragraph::Anonymous { first, host, .. } => first.unwrap_or(host),
        };
        let Some(owner) = editing::editing_host(&dom, seed) else {
            continue;
        };
        let li = ancestor(&dom, seed, owner, |d, n| {
            editing::local(d, n) == "li" && d.parent_element(n).ok().flatten().is_some_and(|p| list(d, p))
        });
        let nodes = if let Some(item) = li {
            vec![item]
        } else {
            if !increase && ancestor(&dom, seed, owner, |d, n| is_indent(d, n) || list(d, n)).is_none() {
                continue;
            }
            match para {
                Paragraph::Block(n)
                    if is_indent(&dom, n) || matches!(editing::local(&dom, n).as_str(), "td" | "th" | "caption") =>
                {
                    dom.children(n).map(|it| it.collect()).unwrap_or_default()
                }
                Paragraph::Block(n) => vec![n],
                Paragraph::Anonymous { first: Some(l), last: Some(r), .. } => run(&dom, l, r),
                Paragraph::Anonymous { host, .. } if increase => {
                    let Some(doc) = dom.node_document(host).ok() else {
                        continue;
                    };
                    let Some(br) = dom.create_element(doc, "br").ok() else {
                        continue;
                    };
                    if dom.append_child(host, br).is_err() {
                        continue;
                    }
                    vec![br]
                }
                Paragraph::Anonymous { .. } => continue,
                Paragraph::Line { host, block, first, last, trailing_break } => {
                    let Some(line) =
                        alignment::align_line(&mut dom, host, block, first, last, trailing_break, &mut points)
                    else {
                        continue;
                    };
                    vec![line]
                }
            }
        };
        for n in nodes {
            if editing::editing_host(&dom, n) != Some(owner) || n == owner {
                continue;
            }
            if !increase && wrapper_for(&dom, n, owner).is_none() {
                continue;
            }
            if let Some((prev_owner, g)) = groups.last_mut() {
                if g.contains(&n) {
                    continue;
                }
                if *prev_owner == owner && g.last().and_then(|&k| dom.next_sibling(k).ok().flatten()) == Some(n) {
                    g.push(n);
                    continue;
                }
            }
            groups.push((owner, vec![n]));
        }
    }
    let mut changed = false;
    for (owner, nodes) in groups {
        changed |= if increase {
            indent(&mut dom, &nodes, owner, &mut points)
        } else {
            outdent(&mut dom, &nodes, owner, &mut points)
        }
        .is_some();
    }
    drop(dom);
    if changed {
        super::inline_value::reselect(page, (s, e), points);
        if let Some(saved) = saved {
            let mut history = page.undo.borrow_mut();
            if !history.replaying {
                history.typing = None;
                history.push(super::Step { target: host, saved });
            }
        }
        editing::input_event(page, host, "input", input_type, None, false);
    }
}
