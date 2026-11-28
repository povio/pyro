//! Scoped focusgroup (Open UI "scoped focusgroup" explainer, Chrome's
//! `FocusgroupController`): arrow-key navigation between the items of an
//! element with a `focusgroup` attribute.
//!
//! The attribute is a list of ASCII-case-insensitive tokens. The first
//! recognised behaviour token (`toolbar`, `tablist`, `radiogroup`,
//! `listbox`, `menu`, `menubar`, `grid`, `feed`) makes the element a focusgroup
//! owner; without one the attribute does nothing. `none` anywhere opts the
//! element and its subtree out of every ancestor focusgroup. The modifiers
//! `inline` / `block` restrict the axes (both together allow both), `wrap`
//! / `nowrap` override the behaviour's wrapping and `nomemory` turns off
//! the last-focused memory used for the group's tab stop.
//!
//! An owner's items are the focusable elements of its flat-tree subtree
//! (children of reading-flow containers in reading order),
//! not looking into opted-out subtrees nor into nested owners (a focusable
//! nested owner is itself an item of the outer group). Directional input
//! only moves between items that are sequentially focusable (tabindex not
//! negative); a focused tabindex=-1 item still navigates away. Top-layer
//! elements (showing popovers, modal dialogs) and their subtrees belong to
//! no ancestor focusgroup.


use crate::dom::Dom;
use crate::node::{NodeId, NodeKind, NIL};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FocusgroupBehavior {
    Toolbar,
    Tablist,
    Radiogroup,
    Listbox,
    Menu,
    Menubar,
    Grid,
    /// A feed: block-axis navigation with hard edges (`aria-role` feed).
    Feed,
}

/// A parsed `focusgroup` attribute.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Focusgroup {
    /// `none` among the tokens: the element and its subtree opt out.
    OptOut,
    Owner(FocusgroupFlags),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct FocusgroupFlags {
    pub behavior: FocusgroupBehavior,
    pub inline: bool,
    pub block: bool,
    pub wrap: bool,
    pub memory: bool,
    /// A grid's edges: `rowwrap`/`colwrap`/`wrap` wrap to the other end of
    /// the same row (column), `rowflow`/`colflow`/`flow` continue in the
    /// next (previous) row (column). A wrap and a flow, or `nowrap` and a
    /// flow, on one axis conflict and leave a hard edge on that axis. Always
    /// false for other behaviours.
    pub row_wrap: bool,
    pub col_wrap: bool,
    pub row_flow: bool,
    pub col_flow: bool,
}

/// Parses a `focusgroup` attribute value; `None` when it has no effect
/// (no behaviour token and no `none`).
pub fn parse_focusgroup(value: &str) -> Option<Focusgroup> {
    let mut behavior = None;
    let (mut inline, mut block, mut wrap, mut nowrap, mut memory) = (false, false, false, false, true);
    // Grid modifiers: (row, column) wrap and flow requests.
    let (mut row_wrap, mut col_wrap, mut row_flow, mut col_flow) = (false, false, false, false);
    for token in value.split(|c: char| c.is_ascii_whitespace()).filter(|t| !t.is_empty()) {
        let t = token.to_ascii_lowercase();
        let b = match t.as_str() {
            "none" => return Some(Focusgroup::OptOut),
            "toolbar" => Some(FocusgroupBehavior::Toolbar),
            "tablist" => Some(FocusgroupBehavior::Tablist),
            "radiogroup" => Some(FocusgroupBehavior::Radiogroup),
            "listbox" => Some(FocusgroupBehavior::Listbox),
            "menu" => Some(FocusgroupBehavior::Menu),
            "menubar" => Some(FocusgroupBehavior::Menubar),
            "grid" => Some(FocusgroupBehavior::Grid),
            "feed" => Some(FocusgroupBehavior::Feed),
            "inline" => { inline = true; None }
            "block" => { block = true; None }
            "wrap" => { wrap = true; row_wrap = true; col_wrap = true; None }
            "nowrap" => { nowrap = true; None }
            "rowwrap" => { row_wrap = true; None }
            "colwrap" => { col_wrap = true; None }
            "flow" => { row_flow = true; col_flow = true; None }
            "rowflow" => { row_flow = true; None }
            "colflow" => { col_flow = true; None }
            "nomemory" => { memory = false; None }
            _ => None,
        };
        if behavior.is_none() {
            behavior = b;
        }
    }
    let behavior = behavior?;
    use FocusgroupBehavior::*;
    // Behaviour defaults: (inline, block, wrap).
    let (di, db, dw) = match behavior {
        Toolbar => (true, false, false),
        Tablist | Menubar => (true, false, true),
        Radiogroup => (true, true, true),
        Listbox => (false, true, false),
        Menu => (false, true, true),
        Grid => (true, true, false),
        Feed => (false, true, false),
    };
    let (inline, block) = if inline || block { (inline, block) } else { (di, db) };
    let wrap = if nowrap { false } else { wrap || dw };
    // A wrap and a flow on one axis cannot both apply, and `nowrap` rules
    // out a flow too: that axis keeps a hard edge. The hyphenated spellings
    // (`row-wrap`) are not tokens and were ignored above.
    let is_grid = behavior == Grid;
    let axis = |wrap_axis: bool, flow_axis: bool| {
        (is_grid && wrap_axis && !flow_axis && !nowrap, is_grid && flow_axis && !wrap_axis && !nowrap)
    };
    let (row_wrap, row_flow) = axis(row_wrap, row_flow);
    let (col_wrap, col_flow) = axis(col_wrap, col_flow);
    Some(Focusgroup::Owner(FocusgroupFlags { behavior, inline, block, wrap, memory, row_wrap, col_wrap, row_flow, col_flow }))
}

/// A table cell placed on the grid: `rows` x `cols` slots from (`row`, `col`).
struct GridCell {
    el: u32,
    row: usize,
    col: usize,
    rows: usize,
    cols: usize,
}

/// A directional request, already mapped from the physical key to the
/// owner's logical axes.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FocusgroupMove {
    /// `inline`: along the inline axis, else the block axis.
    Next { inline: bool },
    Previous { inline: bool },
    First,
    Last,
}

impl Dom {
    fn focusgroup_idx(&self, i: u32) -> Option<Focusgroup> {
        let value = self.n(i).element()?.attr("focusgroup")?;
        parse_focusgroup(value)
    }

    /// The parsed `focusgroup` attribute of `el`.
    pub fn focusgroup(&self, el: NodeId) -> Option<Focusgroup> {
        self.focusgroup_idx(self.check(el).ok()?)
    }

    /// An element in the top layer: a showing popover or a modal dialog.
    fn focusgroup_top_layer(&self, i: u32) -> bool {
        self.is_popover_showing_idx(i) || self.is_modal_idx(i) || self.is_dialog_in_top_layer_idx(i)
    }

    /// The parent element in the flat tree: a slottable's slot, a shadow
    /// root's host, else the parent element.
    fn focusgroup_flat_parent(&self, i: u32) -> u32 {
        if let Some(slot) = self.find_slot(i, false) {
            return slot;
        }
        let p = self.n(i).parent;
        if p == NIL {
            return NIL;
        }
        match &self.n(p).kind {
            NodeKind::Fragment(f) if f.shadow.is_some() => f.host,
            NodeKind::Element(_) => p,
            _ => NIL,
        }
    }

    /// Flat-tree children: a host's shadow tree, a slot's assigned nodes
    /// (its fallback content without any), else the children.
    fn focusgroup_flat_children(&self, i: u32) -> Vec<u32> {
        if let Some(e) = self.n(i).element() {
            if e.shadow_root != NIL {
                return self.child_idx_iter(e.shadow_root).collect();
            }
        }
        if self.is_slot(i) && self.in_shadow_tree(i) {
            let assigned = self.slot_assigned(i);
            if !assigned.is_empty() {
                return assigned;
            }
        }
        self.child_idx_iter(i).collect()
    }

    /// The focusgroup owner `el` is an item of, with its flags: the nearest
    /// flat-tree ancestor with a behaviour token, unless an opt-out comes
    /// first (or `el` opts out itself).
    pub fn focusgroup_owner_of(&self, el: NodeId) -> Option<(NodeId, FocusgroupFlags)> {
        let i = self.check(el).ok()?;
        // An opted-out element, or a top-layer one (a showing popover, a
        // modal dialog), is no item of any ancestor focusgroup.
        if self.focusgroup_idx(i) == Some(Focusgroup::OptOut) || self.focusgroup_top_layer(i) {
            return None;
        }
        let mut a = self.focusgroup_flat_parent(i);
        while a != NIL {
            match self.focusgroup_idx(a) {
                Some(Focusgroup::OptOut) => return None,
                Some(Focusgroup::Owner(flags)) => return Some((self.id_of(a), flags)),
                None => {}
            }
            // A top-layer element (open popover, modal dialog) is cut off
            // from ancestor focusgroups.
            if self.focusgroup_top_layer(a) {
                return None;
            }
            a = self.focusgroup_flat_parent(a);
        }
        None
    }

    /// The candidate items of `owner` in flat-tree order: every element not
    /// in an opted-out subtree nor inside a nested owner (`area` decides
    /// focusability afterwards).
    pub fn focusgroup_items(&self, owner: NodeId, area: &dyn Fn(NodeId) -> bool) -> Vec<NodeId> {
        self.focusgroup_items_in_order(owner, area, None)
    }

    /// [`Dom::focusgroup_items`] in reading order: children of a
    /// reading-flow container (`reading-flow: flex-visual` and the like)
    /// come in the order sequential navigation uses for them.
    pub fn focusgroup_items_in_order(
        &self, owner: NodeId, area: &dyn Fn(NodeId) -> bool, layout: Option<&crate::layout::Layout>,
    ) -> Vec<NodeId> {
        let Ok(o) = self.check(owner) else { return Vec::new() };
        let order = layout.map(|l| crate::focus_nav::ReadingOrder::new(self, l));
        let children = |i: u32| {
            let mut kids = self.focusgroup_flat_children(i);
            if let Some(order) = &order {
                order.sort(i, &mut kids);
            }
            kids
        };
        let mut out = Vec::new();
        let mut stack: Vec<u32> = children(o).into_iter().rev().collect();
        while let Some(i) = stack.pop() {
            if !self.n(i).is_element() {
                continue;
            }
            let fg = self.focusgroup_idx(i);
            if fg == Some(Focusgroup::OptOut) || self.focusgroup_top_layer(i) {
                continue;
            }
            if area(self.id_of(i)) {
                out.push(self.id_of(i));
            }
            if matches!(fg, Some(Focusgroup::Owner(_))) {
                continue;
            }
            let at = stack.len();
            stack.extend(children(i));
            stack[at..].reverse();
        }
        out
    }

    /// Where directional input `mv` moves focus from the focused `current`
    /// (`area`: the focusable-area test). `None` when focus stays: `current`
    /// is not an item, the owner does not allow the axis, or there is no
    /// further item and the group does not wrap.
    pub fn focusgroup_move_target(
        &self, current: NodeId, mv: FocusgroupMove, area: &dyn Fn(NodeId) -> bool,
        layout: Option<&crate::layout::Layout>,
    ) -> Option<NodeId> {
        let (owner, flags) = self.focusgroup_owner_of(current)?;
        let mut items = self.focusgroup_items_in_order(owner, area, layout);
        if !items.contains(&current) {
            // `current` is focused, so it is an item even when `area` says
            // otherwise (a stale layout); keep its place in tree order.
            let pos = items.iter().position(|&n| self.compare_tree_order(current, n)).unwrap_or(items.len());
            items.insert(pos, current);
        }
        if flags.behavior == FocusgroupBehavior::Grid {
            // A grid moves by table rows and columns; on an element that is no table it is
            // neither a grid nor a linear group. Home and End stay linear.
            let table = layout.is_some_and(|l| self.focusgroup_is_table(l, owner));
            if !table {
                return None;
            }
            if let FocusgroupMove::Next { inline } | FocusgroupMove::Previous { inline } = mv {
                let forward = matches!(mv, FocusgroupMove::Next { .. });
                return self.grid_move_target(owner, current, &items, (inline, forward), flags, layout?);
            }
        }
        let pos = items.iter().position(|&n| n == current)?;
        let target = |n: NodeId| n != current && self.focusgroup_tabbable(n);
        // Home and End go to the first and last sequentially focusable item, which is `current`
        // itself on the first (last) item: focus stays (WPT focusgroup `home-end-keys.optional`).
        // A focused tabindex=-1 item is not one of them.
        let edge = |n: NodeId| self.focusgroup_tabbable(n);
        let found = match mv {
            FocusgroupMove::First => items.iter().copied().find(|&n| edge(n)),
            FocusgroupMove::Last => items.iter().rev().copied().find(|&n| edge(n)),
            FocusgroupMove::Next { inline } | FocusgroupMove::Previous { inline } => {
                if !(if inline { flags.inline } else { flags.block }) {
                    return None;
                }
                let forward = matches!(mv, FocusgroupMove::Next { .. });
                let ahead = if forward {
                    items[pos + 1..].iter().copied().find(|&n| target(n))
                } else {
                    items[..pos].iter().rev().copied().find(|&n| target(n))
                };
                ahead.or_else(|| {
                    if !flags.wrap {
                        return None;
                    }
                    if forward {
                        items[..pos].iter().copied().find(|&n| target(n))
                    } else {
                        items[pos + 1..].iter().rev().copied().find(|&n| target(n))
                    }
                })
            }
        };
        found.filter(|&n| n != current)
    }

    /// Whether directional input may move focus to `n`: a non-negative `tabindex`. A focusable
    /// `<video controls>` or `<audio controls>` is one too though its `tabIndex` reads -1 here (it is
    /// a tab stop, as in Chrome).
    fn focusgroup_tabbable(&self, n: NodeId) -> bool {
        if self.tab_index(n).is_ok_and(|t| t >= 0) {
            return true;
        }
        let Ok(i) = self.check(n) else { return false };
        matches!(self.html_local(i).map(|l| &**l), Some("video" | "audio"))
            && self.tabindex_attr(i).is_none()
            && self.is_focusable(n).unwrap_or(false)
    }

    /// Whether `el` lays out as a table (`<table>` or `display: table`), the only element a
    /// grid focusgroup can be on.
    fn focusgroup_is_table(&self, layout: &crate::layout::Layout, el: NodeId) -> bool {
        matches!(layout.style().computed_display(self, el).as_deref(), Some("table" | "inline-table"))
    }

    /// The table's cell grid, rebuilt from the DOM and the computed `display` values at each
    /// key press (rows removed or added by a keydown handler count): `(cells, slots)`, `slots[row][col]`
    /// the index into `cells` of the cell covering that slot, `None` for a hole. Rows are in visual
    /// order (header group, bodies, footer group); a row-spanning cell stops at the end of its row
    /// group and occupies its slots in the rows below (HTML "forming a table").
    fn grid_cells(&self, owner: u32, layout: &crate::layout::Layout) -> (Vec<GridCell>, Vec<Vec<Option<usize>>>) {
        let display = |i: u32| layout.style().computed_display(self, self.id_of(i)).unwrap_or_default();
        let elements = |i: u32| self.focusgroup_flat_children(i).into_iter().filter(move |&c| self.n(c).is_element());
        let (mut header, mut footer): (Option<Vec<u32>>, Option<Vec<u32>>) = (None, None);
        let mut bodies: Vec<Vec<u32>> = Vec::new();
        let mut last_loose = false;
        for c in elements(owner) {
            let d = display(c);
            let rows_of = |g: u32| elements(g).filter(|&r| display(r) == "table-row").collect::<Vec<u32>>();
            match d.as_str() {
                "table-row" => {
                    // Consecutive loose rows share the implicit row group.
                    if last_loose && !bodies.is_empty() {
                        bodies.last_mut().map(|g| g.push(c));
                    } else {
                        bodies.push(vec![c]);
                    }
                    last_loose = true;
                    continue;
                }
                "table-header-group" if header.is_none() => header = Some(rows_of(c)),
                "table-footer-group" if footer.is_none() => footer = Some(rows_of(c)),
                "table-row-group" | "table-header-group" | "table-footer-group" => bodies.push(rows_of(c)),
                _ => continue,
            }
            last_loose = false;
        }
        // (row element, index one past the last row of its group)
        let mut rows: Vec<(u32, usize)> = Vec::new();
        for group in header.into_iter().chain(bodies).chain(footer) {
            let end = rows.len() + group.len();
            rows.extend(group.into_iter().map(|r| (r, end)));
        }
        let mut budget: usize = 1 << 22;
        let mut cells: Vec<GridCell> = Vec::new();
        let mut slots: Vec<Vec<Option<usize>>> = vec![Vec::new(); rows.len()];
        for (r, &(row, group_end)) in rows.iter().enumerate() {
            let mut col = 0;
            for cell in elements(row).filter(|&c| display(c) == "table-cell") {
                while slots[r].get(col).is_some_and(|s| s.is_some()) {
                    col += 1;
                }
                // Only HTML cells take their spans from attributes.
                let span = |name: &str| {
                    let e = self.n(cell).element().filter(|e| e.is_html() && matches!(&**e.local_name(), "td" | "th"));
                    e.and_then(|e| e.attr(name)).and_then(|v| {
                        let v = v.trim_matches(|c: char| c.is_ascii_whitespace());
                        let digits = v.len() - v.trim_start_matches(|c: char| c.is_ascii_digit()).len();
                        v[..digits].parse::<usize>().ok()
                    })
                };
                let cols = span("colspan").filter(|&n| n > 0).unwrap_or(1).min(1000);
                // `rowspan=0` spans the rest of the row group.
                let room = group_end - r;
                let rowspan = match span("rowspan") {
                    Some(0) => room,
                    Some(n) => n.min(room),
                    None => 1,
                };
                let idx = cells.len();
                cells.push(GridCell { el: cell, row: r, col, rows: rowspan, cols });
                for rr in r..r + rowspan {
                    if slots[rr].len() < col + cols {
                        // A table of huge spans builds no grid (the key does nothing) rather than a huge one.
                        budget = match budget.checked_sub(col + cols - slots[rr].len()) {
                            Some(b) => b,
                            None => return (Vec::new(), Vec::new()),
                        };
                        slots[rr].resize(col + cols, None);
                    }
                    for s in &mut slots[rr][col..col + cols] {
                        if s.is_none() {
                            *s = Some(idx);
                        }
                    }
                }
                col += cols;
            }
        }
        let width = slots.iter().map(Vec::len).max().unwrap_or(0);
        for row in &mut slots {
            row.resize(width, None);
        }
        (cells, slots)
    }

    /// Where a grid focusgroup's arrow moves focus from `current`: the nearest cell with a
    /// sequentially focusable item in the movement's direction, from the current cell's first
    /// row and column (a spanning cell leaves past its span), skipping holes and cells without
    /// an item. At the edge the axis wraps within its row or column, flows to the next
    /// (previous) row or column, or stays (`flags`); flowing past the last cell goes to the
    /// first. `items`: the owner's focusable items, `(inline, forward)`: the logical direction
    /// (inline moves along a row).
    fn grid_move_target(
        &self, owner: NodeId, current: NodeId, items: &[NodeId], (inline, forward): (bool, bool),
        flags: FocusgroupFlags, layout: &crate::layout::Layout,
    ) -> Option<NodeId> {
        let o = self.check(owner).ok()?;
        let (cells, slots) = self.grid_cells(o, layout);
        let (nrows, ncols) = (slots.len(), slots.first().map_or(0, Vec::len));
        let cell_at: std::collections::HashMap<u32, usize> = cells.iter().enumerate().map(|(i, c)| (c.el, i)).collect();
        // The cell holding `n`: the nearest flat-tree ancestor-or-self that is one of the cells.
        let cell_of = |n: NodeId| {
            let mut i = self.check(n).ok()?;
            while i != NIL && i != o {
                if let Some(&c) = cell_at.get(&i) {
                    return Some(c);
                }
                i = self.focusgroup_flat_parent(i);
            }
            None
        };
        let cur = cell_of(current)?;
        // Each cell's first sequentially focusable item in tree order.
        let mut cell_item: Vec<Option<NodeId>> = vec![None; cells.len()];
        for &n in items {
            if self.focusgroup_tabbable(n) {
                if let Some(c) = cell_of(n) {
                    cell_item[c].get_or_insert(n);
                }
            }
        }
        let at = |r: usize, c: usize| slots[r][c].filter(|&ci| ci != cur).and_then(|ci| cell_item[ci]);
        // Work along the axis of the key (`a`: position along it, `b`: the row or column the
        // move stays on), whichever it is.
        let GridCell { row: r0, col: c0, rows: rs, cols: cs, .. } = cells[cur];
        let (nalong, nacross) = if inline { (ncols, nrows) } else { (nrows, ncols) };
        let (start, span, line) = if inline { (c0, cs, r0) } else { (r0, rs, c0) };
        let (wrap, flow) = if inline { (flags.row_wrap, flags.row_flow) } else { (flags.col_wrap, flags.col_flow) };
        let item_at = |a: usize, b: usize| if inline { at(b, a) } else { at(a, b) };
        let hit = |b: usize, along: &[usize]| along.iter().find_map(|&a| item_at(a, b));
        let ascending = |lo: usize, hi: usize| -> Vec<usize> { if forward { (lo..hi).collect() } else { (lo..hi).rev().collect() } };
        // Past the current cell's span, then (wrapping) from the far end of the same line, then
        // (flowing) the whole next line, cycling through every line.
        let ahead: Vec<usize> = if forward { (start + span..nalong).collect() } else { (0..start).rev().collect() };
        let far: Vec<usize> = if forward { (0..start).collect() } else { (start + span..nalong).rev().collect() };
        let full = ascending(0, nalong);
        hit(line, &ahead)
            .or_else(|| if wrap { hit(line, &far) } else { None })
            .or_else(|| {
                if !flow {
                    return None;
                }
                (1..=nacross).find_map(|k| hit(if forward { (line + k) % nacross } else { (line + nacross - k) % nacross }, &full))
            })
    }

    /// The element Tab or Shift+Tab lands on when entering a focusgroup
    /// segment (`segment`: its items, one tab stop for all; `order`: the
    /// owner's items in tree order, [`Dom::focusgroup_items_in_order`]): the
    /// owner's remembered item when it is in the segment and the owner keeps
    /// memory, else the first item in tree order with `focusgroupstart`, else
    /// the first item in tree order. Tree order, not the tab order the
    /// segment was collected in: they differ for positive tabindex values.
    pub fn focusgroup_entry(&self, segment: &[NodeId], order: &[NodeId], memory: Option<NodeId>) -> Option<NodeId> {
        if let Some(m) = memory.filter(|m| segment.contains(m)) {
            return Some(m);
        }
        // Items missing from `order` (a stale layout) keep their segment order, after the others.
        let mut ranked: Vec<(usize, usize, NodeId)> = segment
            .iter()
            .enumerate()
            .map(|(i, &n)| (order.iter().position(|&o| o == n).unwrap_or(usize::MAX), i, n))
            .collect();
        ranked.sort_unstable_by_key(|&(rank, i, _)| (rank, i));
        ranked
            .iter()
            .map(|&(_, _, n)| n)
            .find(|&n| self.element(n).is_ok_and(|e| e.attr("focusgroupstart").is_some()))
            .or_else(|| ranked.first().map(|&(_, _, n)| n))
    }

    /// `a` precedes `b` in shadow-including tree order.
    fn compare_tree_order(&self, a: NodeId, b: NodeId) -> bool {
        self.compare_document_position(b, a).is_ok_and(|p| p & 0x02 != 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_behaviour_defaults_and_modifiers() {
        let owner = |v: &str| match parse_focusgroup(v) {
            Some(Focusgroup::Owner(f)) => Some((f.inline, f.block, f.wrap)),
            _ => None,
        };
        assert_eq!(owner("toolbar"), Some((true, false, false)));
        assert_eq!(owner("tablist"), Some((true, false, true)));
        assert_eq!(owner("tablist block"), Some((false, true, true)));
        assert_eq!(owner("tablist nowrap"), Some((true, false, false)));
        assert_eq!(owner("radiogroup inline"), Some((true, false, true)));
        assert_eq!(owner("listbox wrap"), Some((false, true, true)));
        assert_eq!(owner("menu inline"), Some((true, false, true)));
        assert_eq!(owner("wrap toolbar"), Some((true, false, true)));
        assert_eq!(owner("invalid toolbar"), Some((true, false, false)));
        assert_eq!(owner("toolbar inline block"), Some((true, true, false)));
        // A feed navigates along the block axis and has hard edges; `noitemcontrols` is no token.
        assert_eq!(owner("feed"), Some((false, true, false)));
        assert_eq!(owner("feed noitemcontrols"), Some((false, true, false)));
        assert_eq!(parse_focusgroup(""), None);
        assert_eq!(parse_focusgroup("wrap block"), None);
        let grid = |v: &str| match parse_focusgroup(v) {
            Some(Focusgroup::Owner(f)) if f.behavior == FocusgroupBehavior::Grid => {
                Some((f.row_wrap, f.col_wrap, f.row_flow, f.col_flow))
            }
            _ => None,
        };
        assert_eq!(grid("grid"), Some((false, false, false, false)));
        assert_eq!(grid("grid wrap"), Some((true, true, false, false)));
        assert_eq!(grid("grid rowwrap colflow"), Some((true, false, false, true)));
        assert_eq!(grid("grid flow"), Some((false, false, true, true)));
        // A wrap and a flow on one axis conflict into a hard edge there; the other axis keeps its own.
        assert_eq!(grid("grid wrap rowflow"), Some((false, true, false, false)));
        assert_eq!(grid("grid nowrap flow"), Some((false, false, false, false)));
        assert_eq!(grid("grid row-wrap col-wrap"), Some((false, false, false, false)));
        assert_eq!(parse_focusgroup("toolbar wrap none block"), Some(Focusgroup::OptOut));
    }
}
