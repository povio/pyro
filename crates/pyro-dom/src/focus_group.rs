//! Scoped focusgroup (Open UI "scoped focusgroup" explainer, Chrome's
//! `FocusgroupController`): arrow-key navigation between the items of an
//! element with a `focusgroup` attribute.
//!
//! The attribute is a list of ASCII-case-insensitive tokens. The first
//! recognised behaviour token (`toolbar`, `tablist`, `radiogroup`,
//! `listbox`, `menu`, `menubar`, `grid`) makes the element a focusgroup
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
}

/// Parses a `focusgroup` attribute value; `None` when it has no effect
/// (no behaviour token and no `none`).
pub fn parse_focusgroup(value: &str) -> Option<Focusgroup> {
    let mut behavior = None;
    let (mut inline, mut block, mut wrap, mut nowrap, mut memory) = (false, false, false, false, true);
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
            "inline" => { inline = true; None }
            "block" => { block = true; None }
            "wrap" => { wrap = true; None }
            "nowrap" => { nowrap = true; None }
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
    };
    let (inline, block) = if inline || block { (inline, block) } else { (di, db) };
    let wrap = if nowrap { false } else { wrap || dw };
    Some(Focusgroup::Owner(FocusgroupFlags { behavior, inline, block, wrap, memory }))
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
        if self.focusgroup_idx(i) == Some(Focusgroup::OptOut) {
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
        // A grid focusgroup moves by table rows and columns, which this
        // does not implement; it never navigates linearly (on a non-table
        // element it is not a focusgroup of either kind in Chrome either).
        if flags.behavior == FocusgroupBehavior::Grid {
            return None;
        }
        let mut items = self.focusgroup_items_in_order(owner, area, layout);
        if !items.contains(&current) {
            // `current` is focused, so it is an item even when `area` says
            // otherwise (a stale layout); keep its place in tree order.
            let pos = items.iter().position(|&n| self.compare_tree_order(current, n)).unwrap_or(items.len());
            items.insert(pos, current);
        }
        let pos = items.iter().position(|&n| n == current)?;
        let target = |n: NodeId| n != current && self.tab_index(n).is_ok_and(|t| t >= 0);
        let found = match mv {
            FocusgroupMove::First => items.iter().copied().find(|&n| target(n)),
            FocusgroupMove::Last => items.iter().rev().copied().find(|&n| target(n)),
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

    /// The element Tab or Shift+Tab lands on when entering a focusgroup
    /// segment (`segment`: its items in tree order, one tab stop for all):
    /// the owner's remembered item when it is in the segment and the owner
    /// keeps memory, else the first item with `focusgroupstart`, else the
    /// first item.
    pub fn focusgroup_entry(&self, segment: &[NodeId], memory: Option<NodeId>) -> Option<NodeId> {
        if let Some(m) = memory.filter(|m| segment.contains(m)) {
            return Some(m);
        }
        segment
            .iter()
            .copied()
            .find(|&n| self.element(n).is_ok_and(|e| e.attr("focusgroupstart").is_some()))
            .or_else(|| segment.first().copied())
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
        assert_eq!(parse_focusgroup(""), None);
        assert_eq!(parse_focusgroup("wrap block"), None);
        assert_eq!(parse_focusgroup("toolbar wrap none block"), Some(Focusgroup::OptOut));
    }
}
