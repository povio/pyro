//! Deleting complete anonymous lines before a child paragraph leaves that
//! paragraph intact. The next insertion belongs inside the surviving block.
use super::*;

pub(super) struct Deletion {
    pub range: (Point, Point),
    child: NodeId,
    outer: NodeId,
    rebalance: bool,
}

fn before(dom: &Dom, node: NodeId) -> Option<Point> {
    Some((dom.parent(node).ok()??, dom.index_in_parent(node).ok()?))
}

fn lift_start(dom: &Dom, outer: NodeId, mut point: Point) -> Point {
    while point.0 != outer && point.1 == 0 {
        let Some(parent) = before(dom, point.0) else { break };
        point = parent;
    }
    point
}

fn preceding_break(page: &PageDom, dom: &Dom, host: NodeId, outer: NodeId, point: Point) -> Option<Point> {
    let p = deep_backward(dom, lift_start(dom, outer, point));
    let offset = p.1.checked_sub(1)?;
    let leaf = if is_text(dom, p.0) { p.0 } else { dom.child_at(p.0, offset).ok()?? };
    if block_of(dom, leaf, host) != outer
        || editing_host(dom, leaf) != Some(host)
        || !target_range::rendered(page, dom, leaf)
    {
        return None;
    }
    if is_text(dom, p.0) {
        return (dom.substring_data(p.0, offset, 1).ok()? == "\n"
            && crate::edit_commands::newlines_significant_in(page, dom, p.0))
        .then_some((p.0, offset));
    }
    (local(dom, leaf) == "br").then_some((p.0, offset))
}

/// Find the child block immediately following this break, possibly after
/// closing one or more inline ancestors.
fn following_block(dom: &Dom, outer: NodeId, point: Point) -> Option<NodeId> {
    let p = deep_forward(dom, point);
    let mut after = if is_text(dom, p.0) {
        if p.1.checked_add(1)? != len(dom, p.0) {
            return None;
        }
        (p.0, p.1 + 1)
    } else {
        let br = dom.child_at(p.0, p.1).ok()??;
        if local(dom, br) != "br" {
            return None;
        }
        (p.0, p.1 + 1)
    };
    while after.0 != outer {
        if after.1 != len(dom, after.0) {
            return None;
        }
        let (p, i) = before(dom, after.0)?;
        after = (p, i + 1);
    }
    dom.child_at(outer, after.1).ok()?
}

impl Deletion {
    /// For replacement the caller supplies the selected range. For a
    /// collapsed deletion the key direction identifies the intervening break.
    pub(super) fn plan(
        page: &PageDom,
        dom: &Dom,
        host: NodeId,
        start: Point,
        end: Point,
        backward: bool,
    ) -> Option<Self> {
        let collapsed = start == end;
        if collapsed && is_text(dom, start.0) {
            if backward && start.1 != 0 {
                return None;
            }
            if !backward && start.1.checked_add(1)? != len(dom, start.0) {
                return None;
            }
        }
        let (outer, child) = if collapsed && !backward {
            let outer = block_of(dom, start.0, host);
            (outer, following_block(dom, outer, start)?)
        } else {
            let child = block_of(dom, end.0, host);
            if child == host {
                return None;
            }
            let outer = dom.parent_element(child).ok()??;
            if !collapsed && block_of(dom, start.0, host) != outer {
                return None;
            }
            (outer, child)
        };
        if !is_block(dom, child) || editing_host(dom, child) != Some(host) || !target_range::rendered(page, dom, child)
        {
            return None;
        }
        let boundary = before(dom, child)?;
        let separator = preceding_break(page, dom, host, outer, boundary)?;
        if !target_range::rendered(page, dom, start.0) || !target_range::rendered(page, dom, end.0) {
            return None;
        }
        let child_start = editable_edge(dom, (child, 0), true);
        if collapsed && backward && deep_forward(dom, start) != child_start {
            return None;
        }
        let from = if collapsed { separator } else { start };
        if lift_start(dom, outer, from) != (outer, 0) && preceding_break(page, dom, host, outer, from).is_none() {
            return None;
        }
        let from = if collapsed { lift_start(dom, outer, from) } else { from };
        if crate::range::compare_boundary_points_dom(dom, from, boundary) >= 0 {
            return None;
        }
        let to = if collapsed {
            // When only the suffix of an inline is removed, its trailing
            // boundary stays inside that formatting element.
            let mut after = if is_text(dom, separator.0) {
                let (parent, index) = before(dom, separator.0)?;
                (parent, index + 1)
            } else {
                (separator.0, separator.1 + 1)
            };
            while after.0 != outer && !dom.contains(after.0, from.0).ok()? {
                let (parent, index) = before(dom, after.0)?;
                after = (parent, index + 1);
            }
            after
        } else if deep_forward(dom, end) == child_start {
            boundary
        } else {
            end
        };
        Some(Self { range: (from, to), child, outer, rebalance: collapsed })
    }

    pub(super) fn apply(self, page: &PageDom, dom: &mut Dom, host: NodeId) -> Point {
        let from = lift_start(dom, self.outer, self.range.0);
        if crate::range::delete_contents(dom, from, self.range.1).is_err() {
            return from;
        }
        let caret = editable_edge(dom, (self.child, 0), true);
        if self.rebalance {
            whitespace::after_join(page, dom, host, caret);
        }
        caret
    }
}
