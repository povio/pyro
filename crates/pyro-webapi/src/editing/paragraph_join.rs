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

fn editable_paragraph(page: &PageDom, dom: &Dom, node: NodeId, host: NodeId) -> bool {
    dom.element(node).is_ok_and(|el| el.is_html() && matches!(&**el.local_name(), "div" | "p"))
        && editing_host(dom, node) == Some(host)
        && page.layout().computed_style_value(dom, node, "display") == "block"
        && page.layout().computed_style_value(dom, node, "visibility") == "visible"
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

/// A nested paragraph beside an anonymous line in its containing paragraph.
/// Limit the join to single-leaf lines so moving it cannot consume later lines
/// or strip unrelated inline formatting.
pub(super) struct NestedJoin {
    outer: NodeId,
    inner: NodeId,
    left: NodeId,
    right: NodeId,
    inner_first: bool,
}

impl NestedJoin {
    pub(super) fn at(page: &PageDom, dom: &Dom, host: NodeId, caret: Point, backward: bool) -> Option<Self> {
        if is_text(dom, caret.0) && caret.1 != (if backward { 0 } else { len(dom, caret.0) }) {
            return None;
        }
        let block = block_of(dom, caret.0, host);
        for outer in [Some(block), dom.parent_element(block).ok().flatten()].into_iter().flatten() {
            if outer == host || dom.child_count(outer).ok()? != 2 || !editable_paragraph(page, dom, outer, host) {
                continue;
            }
            let a = dom.first_child(outer).ok()??;
            let b = dom.last_child(outer).ok()??;
            let inner_first = is_block(dom, a);
            let (inner, anonymous) = if inner_first { (a, b) } else { (b, a) };
            if !editable_paragraph(page, dom, inner, host) || dom.child_count(inner).ok()? != 1 {
                continue;
            }
            let child = dom.first_child(inner).ok()??;
            let (left, right) = if inner_first { (child, anonymous) } else { (anonymous, child) };
            if [left, right].iter().any(|&n| {
                !(is_text(dom, n) || local(dom, n) == "br")
                    || editing_host(dom, n) != Some(host) || !target_range::rendered(page, dom, n)
            }) { continue; }
            // A styled source needs a formatting carrier when unwrapped.
            // Keep the existing editing path for those joins.
            if !inner_first && is_text(dom, child) && (
                dom.attributes(inner).ok()?.len() != 0 ||
                ["white-space", "color", "font-weight", "font-style", "font-family", "font-size"].iter().any(|p|
                    page.layout().computed_style_value(dom, inner, p) != page.layout().computed_style_value(dom, outer, p))
            ) { continue; }
            let join = Self { outer, inner, left, right, inner_first };
            let (from, to) = join.range(dom)?;
            let edge = if backward { deep_forward(dom, caret) } else { deep_backward(dom, caret) };
            if caret == (if backward { to } else { from }) || edge == (if backward { to } else { from }) {
                return Some(join);
            }
        }
        None
    }

    pub(super) fn range(&self, dom: &Dom) -> Option<(Point, Point)> {
        let from = if is_text(dom, self.left) { (self.left, len(dom, self.left)) } else { before_node(dom, self.left)? };
        let to = if is_text(dom, self.right) { (self.right, 0) } else { before_node(dom, self.right)? };
        Some((from, to))
    }

    pub(super) fn apply(self, page: &PageDom, dom: &mut Dom, host: NodeId) -> Option<Point> {
        let (from, to) = self.range(dom)?;
        let outer_leaf = if self.inner_first { self.right } else { self.left };
        let caret = if local(dom, outer_leaf) == "br" {
            dom.remove_node(outer_leaf).ok()?;
            if self.inner_first { from } else { to }
        } else if local(dom, self.right) == "br" {
            dom.remove_node(self.inner).ok()?;
            from
        } else {
            let value = dom.char_data(self.right).ok()?.to_owned();
            let destination = if is_text(dom, self.left) {
                dom.replace_data(self.left, from.1, 0, &value).ok()?;
                from
            } else {
                let text = dom.create_text(dom.node_document(self.inner).ok()?, &value).ok()?;
                dom.replace_child(self.inner, text, self.left).ok()?;
                (text, 0)
            };
            dom.remove_node(if self.inner_first { self.right } else { self.inner }).ok()?;
            // Resolve the seam while its paragraph still has its computed style.
            whitespace::after_join(page, dom, host, destination);
            // At the host's sole anonymous paragraph the outer DIV no longer
            // separates any lines. Keep attributed or neighboring blocks.
            if local(dom, self.outer) == "div" && dom.attributes(self.outer).ok()?.is_empty()
                && dom.parent(self.outer).ok()? == Some(host) && dom.child_count(host).ok()? == 1
            {
                while let Some(child) = dom.first_child(self.outer).ok()? {
                    dom.insert_before(host, child, Some(self.outer)).ok()?;
                }
                dom.remove_node(self.outer).ok()?;
            }
            return Some(destination);
        };
        whitespace::after_join(page, dom, host, caret);
        Some(caret)
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
    // At the start of the first nested paragraph, backspace crosses the
    // outer paragraph's boundary. Stop as soon as any content precedes it.
    if right_to_left {
        while block != host && dom.prev_sibling(block).ok()?.is_none() {
            let parent = dom.parent_element(block).ok()??;
            if parent == host
                || !editable_paragraph(page, dom, block, host)
                || !editable_paragraph(page, dom, parent, host)
            {
                break;
            }
            block = parent;
        }
    }
    let (other, separators) = adjacent(dom, block, !right_to_left)?;
    let (left, right) = if right_to_left { (other, block) } else { (block, other) };
    let parent = dom.parent(left).ok()??;
    let seam = deep_backward(dom, (left, len(dom, left)));
    let layout = page.layout();
    let left_tag = local(dom, left);
    let right_tag = local(dom, right);
    let pre_join = matches!((left_tag.as_str(), right_tag.as_str()),
        ("pre", "div" | "p" | "blockquote") | ("div" | "p" | "blockquote", "pre"));
    let left_block = (pre_join || matches!(left_tag.as_str(), "div" | "p"))
        && layout.computed_style_value(dom, left, "display") == "block";
    if left_block && !pre_join && parent == host && !matches!(right_tag.as_str(), "div" | "p") && separators.is_empty() {
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
        if layout.computed_style_value(dom, left, "white-space")
            == layout.computed_style_value(dom, right_el, "white-space")
        {
            return None;
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
        || (!pre_join && !matches!(right_tag.as_str(), "div" | "p"))
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
    // The box containing the source line can be nested within the right
    // paragraph. Only descend through its leading editable block children.
    let mut source = right;
    while let Some(child) = dom.first_child(source).ok()? {
        if !editable_paragraph(page, dom, child, host) {
            break;
        }
        source = child;
    }
    let source_style = layout.computed_style_value(dom, source, "white-space");
    let destination = if left_block { left } else { parent };
    let dest_style = layout.computed_style_value(dom, destination, "white-space");
    let left_style =
        if is_text(dom, left) { parent_style } else { layout.computed_style_value(dom, left, "white-space") };
    if source_style.is_empty() || (!pre_join && source_style == left_style) {
        return None;
    }
    if !is_text(dom, deep_forward(dom, (right, 0)).0) {
        return None;
    }
    if source != right
        && (layout.computed_style_value(dom, right, "visibility") != "visible"
            || layout.computed_style_value(dom, destination, "visibility") != "visible"
            || layout.computed_style_value(dom, dom.parent_element(seam.0).ok()??, "visibility") != "visible")
    {
        return None;
    }
    let (mut carried, inherited) = if pre_join {
        pre_join_style(page, dom, source, destination, &source_style)
    } else {
        (vec![("white-space", source_style)], Vec::new())
    };
    carried.retain(|(name, value)| {
        if *name == "white-space" { *value != dest_style }
        else { *value != layout.computed_style_value(dom, destination, name) }
    });
    let line = first_line(page, dom, source, host)?;
    // A single inline element can carry the joined line's formatting itself.
    // Inspect its original context before extraction clones a partial line.
    let carrier = if !pre_join && source == right && carried.len() == 1 {
        dom.first_child(source).ok()?.filter(|&el| formatting_element(dom, el)).map(|el| {
            let explicit = ["white-space", "white-space-collapse", "text-wrap-mode", "text-wrap"]
                .iter().any(|name| layout.style().inline_declared_value(dom, el, name).is_some());
            (explicit, layout.computed_style_value(dom, el, "white-space"))
        })
    } else { None };
    let (fragment, styled) = extract_styled_line(page, dom, host, source, line, source != right)?;
    if len(dom, fragment) > 0 {
        let inline = carrier.filter(|_| len(dom, fragment) == 1);
        let content = if let Some((explicit, original)) = inline {
            let child = dom.first_child(fragment).ok()??;
            if !explicit && !original.is_empty() && original != dest_style {
                let existing = dom.get_attribute(child, "style").ok()?.unwrap_or("");
                let style = format!("white-space: {original}; {existing}");
                dom.set_attribute(child, "style", style.trim_end()).ok()?;
            }
            fragment
        } else if !carried.is_empty() {
            let span = dom.create_element(dom.node_document(right).ok()?, "span").ok()?;
            dom.set_attribute(span, "style", &declaration_text(&carried)).ok()?;
            dom.append_child(span, fragment).ok()?;
            span
        } else {
            fragment
        };
        dom.insert_before(destination, content, (!left_block).then_some(right)).ok()?;
        // An inherited declaration may contain a relative unit or a variable
        // scoped to the old ancestor. Keep the author's spelling when it
        // still resolves correctly, otherwise pin its original computed value.
        if !inherited.is_empty() && content != fragment {
            let mut changed = false;
            for (name, expected) in inherited {
                if layout.computed_style_value(dom, content, name) != expected {
                    if let Some((_, value)) = carried.iter_mut().find(|(key, _)| *key == name) {
                        *value = expected;
                        changed = true;
                    }
                }
            }
            if changed {
                dom.set_attribute(content, "style", &declaration_text(&carried)).ok()?;
            }
        }
    }
    for (node, values) in styled {
        preserve_style(page, dom, node, &values)?;
    }
    for separator in separators {
        dom.remove_node(separator).ok()?;
    }
    // Remove only emptied containers along the source line's original
    // ancestry. Siblings after a BR, preserved newline or block remain.
    let mut emptied = source;
    while len(dom, emptied) == 0 {
        let parent = dom.parent(emptied).ok()??;
        dom.remove_node(emptied).ok()?;
        if emptied == right {
            break;
        }
        emptied = parent;
    }
    Some(seam)
}

type Declarations = Vec<(&'static str, String)>;

fn declaration_text(declarations: &Declarations) -> String {
    declarations.iter().map(|(name, value)| format!("{name}: {value};")).collect::<Vec<_>>().join(" ")
}

fn pre_join_style(
    page: &PageDom, dom: &Dom, source: NodeId, destination: NodeId, source_white_space: &str,
) -> (Declarations, Declarations) {
    let layout = page.layout();
    let Some(engine) = layout.synced_geometry_source(dom) else { return (Vec::new(), Vec::new()) };
    let mut carried = engine.authored_editing_style(pyro_dom::snapshot::render_key(source), TEXT_STYLE, source_white_space);
    let mut inherited = Vec::new();
    let mut pending: Declarations = TEXT_STYLE.iter().filter_map(|&name| {
        if carried.iter().any(|(key, _)| *key == name) { return None; }
        let value = layout.computed_style_value(dom, source, name);
        (!value.is_empty() && value != layout.computed_style_value(dom, destination, name)).then_some((name, value))
    }).collect();
    // Walk only as long as the candidate property is inherited unchanged.
    // This prevents a pre's own UA font or whitespace from being copied.
    let mut parent = dom.parent_element(source).ok().flatten();
    while let Some(ancestor) = parent {
        pending.retain(|(name, original)| layout.computed_style_value(dom, ancestor, name) == *original);
        if pending.is_empty() { break; }
        let white_space = layout.computed_style_value(dom, ancestor, "white-space");
        for (name, specified) in engine.authored_editing_style(pyro_dom::snapshot::render_key(ancestor), TEXT_STYLE, &white_space) {
            if let Some(index) = pending.iter().position(|(key, _)| *key == name) {
                inherited.push(pending.remove(index));
                carried.push((name, specified));
            }
        }
        parent = dom.parent_element(ancestor).ok().flatten();
    }
    (carried, inherited)
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
    preserve_line_style(page, dom, node, source, None)
}

// A moved inline carrier lives in a new paragraph. Compare its line mode
// with the destination paragraph even if a matching stylesheet rule still
// applies to the carrier at its new location.
fn preserve_line_style(
    page: &PageDom, dom: &mut Dom, node: NodeId, source: &[String], paragraph_white_space: Option<&str>,
) -> Option<()> {
    let mut after = text_style(page, dom, node)?;
    if let Some(mode) = paragraph_white_space {
        after[0] = mode.to_owned();
    }
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
pub(super) fn join_anonymous_right(
    page: &PageDom,
    dom: &mut Dom,
    host: NodeId,
    left: NodeId,
    right: NodeId,
    seam: Point,
) -> Option<Point> {
    let start = (host, dom.index_in_parent(right).ok()?);
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
    let paragraph_mode = page.layout().computed_style_value(dom, left, "white-space");
    let mut authored_mode = None;
    for (child, mut values) in moved.into_iter().zip(styles) {
        let root = dom.parent_element(child).ok()? == Some(left);
        if root {
            let style = page.layout();
            let declared = ["white-space", "white-space-collapse", "text-wrap-mode", "text-wrap"]
                .iter().any(|name| style.style().inline_declared_value(dom, child, name).is_some());
            authored_mode = declared.then(|| values[0].clone());
        }
        // Relative declarations resolve in the new context. Keep dependent
        // descendants inheriting that value instead of freezing the old one.
        if authored_mode.as_ref() == values.first() {
            values[0].clear();
        }
        preserve_line_style(page, dom, child, &values, root.then_some(paragraph_mode.as_str()))?;
    }
    Some(seam)
}

/// Snapshot formatting on the inclusive preorder prefix which Range
/// extraction will move or clone out of the source paragraph.
fn extract_styled_line(
    page: &PageDom,
    dom: &mut Dom,
    host: NodeId,
    source: NodeId,
    line: LineEnd,
    keep_style: bool,
) -> Option<(NodeId, Vec<(NodeId, Vec<String>)>)> {
    if !keep_style {
        return Some((extract_line(dom, source, (source, 0), line)?, Vec::new()));
    }
    let mut styles = Vec::new();
    let stop = match line {
        LineEnd::Break(n) => Some((n, false)),
        LineEnd::Newline(n, i) => Some((n, i != 0)),
        LineEnd::Boundary((p, i)) => dom.child_at(p, i).ok()?.map(|n| (n, false)),
    };
    for n in dom.descendants(source).ok()?.skip(1) {
        if stop == Some((n, false)) {
            break;
        }
        if editing_host(dom, n) != Some(host) {
            return None;
        }
        styles.push(text_style(page, dom, n)?);
        if stop == Some((n, true)) {
            break;
        }
    }
    let fragment = extract_line(dom, source, (source, 0), line)?;
    let moved: Vec<_> = dom.descendants(fragment).ok()?.skip(1).collect();
    Some((fragment, moved.into_iter().zip(styles).collect()))
}

/// Move the first rendered line of the planned source paragraph without
/// reparenting or discarding its surrounding containers and following lines.
pub(super) fn join_first_line(
    page: &PageDom,
    dom: &mut Dom,
    host: NodeId,
    source: NodeId,
    destination: NodeId,
    before: Option<NodeId>,
    seam: Point,
) -> Option<Point> {
    let line = first_line(page, dom, source, host)?;
    let (fragment, styled) = extract_styled_line(page, dom, host, source, line, true)?;
    dom.insert_before(destination, fragment, before).ok()?;
    for (n, values) in styled {
        preserve_style(page, dom, n, &values)?;
    }
    if len(dom, source) == 0 {
        dom.remove_node(source).ok()?;
    }
    Some(seam)
}
