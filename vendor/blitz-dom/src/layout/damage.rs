use blitz_traits::node_id::NodeId;
#[cfg(feature = "svg")]
use markup5ever::local_name;
use stylo_taffy::CopyStyleGetters;

use crate::net::ResourceHandler;
use crate::node::{LayoutStyles, Node, NodeFlags};
use crate::tree::NodeTree;
use crate::{
    BaseDocument, net::ImageHandler, node::ImageResourceData, node::Status, util::ImageLayerKind,
};
use style::matching::StyleChange;
use style::properties::ComputedValues;
use style::properties::generated::longhands::position::computed_value::T as Position;
use style::selector_parser::{PseudoElement, RestyleDamage};
use style::shared_lock::StylesheetGuards;
use style::url::ComputedUrl;
use style::values::generics::image::Image as StyloImage;
use style::values::computed::Display;
use style::values::specified::align::AlignFlags;
use style::values::specified::box_::DisplayInside;
use style::values::specified::box_::DisplayOutside;

use thin_vec::ThinVec;

// Blitz-specific damage bits, in increasing order of severity above Servo's
// `RestyleDamage::RELAYOUT` (0b1111).

pub(crate) const ONLY_RELAYOUT: RestyleDamage =
    RestyleDamage::from_bits_retain(0b_0000_0000_0000_1000);

/// The node's `order` changed: its flex/grid container must re-collect its
/// `layout_children` (which sorts them). Consumed by the container in
/// `propagate_damage_flags`, which marks itself `CONSTRUCT_FC` without
/// forwarding that further up. Deliberately not part of `ALL_DAMAGE`.
pub(crate) const REORDER_CHILDREN: RestyleDamage =
    RestyleDamage::from_bits_retain(0b_0000_0000_0001_0000);

/// The node's own box changed (its style, attributes, or it is new): it is
/// reconstructed, and so is the box that contains it, whose layout children
/// it is one of.
pub(crate) const CONSTRUCT_BOX: RestyleDamage =
    RestyleDamage::from_bits_retain(0b_0000_0000_0010_0000);
/// The node's children changed (inserted, removed, text edited, or a child's
/// own box changed): it re-collects its layout children. Unlike
/// `CONSTRUCT_BOX`, this stops at the node unless its children are laid out
/// by an ancestor (see `forwards_construction`); the mutator marks a parent
/// with it alone (`CHILDREN_CHANGED`).
pub(crate) const CONSTRUCT_FC: RestyleDamage =
    RestyleDamage::from_bits_retain(0b_0000_0000_0100_0000);
/// Something below the node is reconstructed (a traversal hint only).
pub(crate) const CONSTRUCT_DESCENDENT: RestyleDamage =
    RestyleDamage::from_bits_retain(0b_0000_0000_1000_0000);

/// The node's inline content changed: its text, the styles its text is shaped
/// with (`compute_layout_damage`), its children, or a box among them. An
/// inline root built again shapes its text again only when it or something
/// below it carries this bit (`collect_layout_children`); a box built again
/// for a change to its own box alone (`BOX_DAMAGE`: a table cell's padding,
/// a collapsed border's colour) keeps its inline layout. Part of `ALL_DAMAGE`
/// and `CHILDREN_CHANGED`; propagates to every ancestor with the rest.
pub(crate) const INLINE_CONTENT: RestyleDamage =
    RestyleDamage::from_bits_retain(0b_0000_0010_0000_0000);

/// What the mutator marks a parent with when its child list or a child's
/// text changes: re-collect this node's layout children, nothing above.
/// `propagate_damage_flags` adds the relayout it implies.
pub(crate) const CHILDREN_CHANGED: RestyleDamage =
    RestyleDamage::from_bits_retain(CONSTRUCT_FC.bits() | INLINE_CONTENT.bits());

pub(crate) const ALL_DAMAGE: RestyleDamage =
    RestyleDamage::from_bits_retain(0b_0000_0010_1110_1111);

/// `ALL_DAMAGE` without `INLINE_CONTENT`: the node's box is built again, its
/// inline content as it was.
pub(crate) const BOX_DAMAGE: RestyleDamage =
    RestyleDamage::from_bits_retain(0b_0000_0000_1110_1111);

/// Whether `damage` reaches layout: relayout, construction or reordering, as
/// opposed to Stylo's paint bits alone (`REPAINT`, `REBUILD_STACKING_CONTEXT`,
/// `RECALCULATE_OVERFLOW`), which change what paints and where the paint tree
/// is built, but no box.
#[inline]
pub(crate) fn reaches_layout(damage: RestyleDamage) -> bool {
    damage.intersects(ONLY_RELAYOUT | REORDER_CHILDREN | CONSTRUCT_BOX | CONSTRUCT_FC | CONSTRUCT_DESCENDENT)
}

/// An out-of-flow box below the node needs relayout, but nothing the node's
/// own layout reads changed: the box's static position (computed by its
/// parent from the box's style) and its existence are the same, and an
/// out-of-flow box takes no part in its ancestors' in-flow layout or
/// intrinsic sizes. It is what such a box returns to its parent in place of
/// `RELAYOUT` (keeping `RECALCULATE_OVERFLOW`), so the ancestors keep their
/// layout caches and the box is relaid by the root positioning pass, which
/// runs whether or not the root's own layout comes from its cache.
///
/// Only the initial containing block is such a boundary: every ancestor that
/// could be the box's containing block (`is_containing_block_for_some`) turns
/// it back into `RELAYOUT`, since its out-of-flow pass runs inside its own
/// cached layout. Never stored on a node. Deliberately not part of
/// `ALL_DAMAGE`.
pub(crate) const POS_CHILD_RELAYOUT: RestyleDamage =
    RestyleDamage::from_bits_retain(0b_0000_0001_0000_0000);

impl BaseDocument {
    /// Union `RestyleDamage` up the tree and clear the layout caches of every
    /// node whose subtree needs relayout.
    ///
    /// Damage is stored on (styled) DOM nodes, so this pass walks the DOM tree
    /// (`children` + `::before`/`::after`): `display:contents` nodes and the
    /// inline descendants of inline roots carry damage but are not layout
    /// children. Owned anonymous boxes are refreshed through `anonymous_blocks`,
    /// without revisiting their DOM children. Damage inside an anonymous box
    /// also invalidates its cache by walking `layout_parent` upward.
    pub(crate) fn propagate_damage_flags(&mut self, node_id: NodeId) -> RestyleDamage {
        let own_damage = if let Some(data) = self.nodes[node_id]
            .try_stylo_element_data_mut()
            .and_then(|s| s.get_mut())
        {
            // `RestyleDamage::reconstruct()` sets every bit: the
            // `POS_CHILD_RELAYOUT` flag only ever comes from children.
            data.damage - POS_CHILD_RELAYOUT
        } else {
            return RestyleDamage::empty();
        };
        let mut damage = own_damage;
        self.nodes[node_id].flags.set(NodeFlags::OWN_DAMAGE, !own_damage.is_empty());
        self.nodes[node_id].flags.set(NodeFlags::OWN_LAYOUT_DAMAGE, reaches_layout(own_damage));

        // Skip subtrees which contain no damage. Anonymous nodes are never
        // skipped themselves because damage marking walks the DOM parent
        // chain, which bypasses anonymous boxes: a damaged node's flagged
        // ancestors may reach it only through an unflagged anonymous wrapper.
        {
            let node = &self.nodes[node_id];
            if damage.is_empty() && !node.has_damaged_descendants() && !node.is_anonymous() {
                return RestyleDamage::empty();
            }
        }

        let mut damage_from_children = RestyleDamage::empty();
        let children = std::mem::take(&mut self.nodes[node_id].children);
        for child in children.iter() {
            // pyro: a child with no damage below it would return empty at the skip above
            // (`Node::has_no_damage_below`); it is not entered.
            if self.nodes[*child].has_no_damage_below() {
                continue;
            }
            damage_from_children |= crate::util::ensure_stack(|| self.propagate_damage_flags(*child));
        }
        if let Some(before_id) = self.nodes[node_id].before() {
            damage_from_children |= crate::util::ensure_stack(|| self.propagate_damage_flags(before_id));
        }
        if let Some(after_id) = self.nodes[node_id].after() {
            damage_from_children |= crate::util::ensure_stack(|| self.propagate_damage_flags(after_id));
        }

        // A child's `order` changed: this container re-collects (and so re-sorts)
        // its layout children. Only the direct parent consumes the flag; like
        // any other change to its children, it reaches ancestors only as
        // `RELAYOUT` (always set alongside `REORDER_CHILDREN`) plus
        // `CONSTRUCT_DESCENDENT`.
        let reorder = damage_from_children.contains(REORDER_CHILDREN)
            && matches!(
                self.nodes[node_id].display_constructed_as().inside(),
                DisplayInside::Flex | DisplayInside::Grid
            );
        damage_from_children.remove(REORDER_CHILDREN);

        // An out-of-flow descendant is relaid by its containing block: if this
        // node can be one, it relays (its out-of-flow pass runs inside its
        // cached layout); otherwise the flag passes on to the root.
        let mut pos_child = false;
        if damage_from_children.contains(POS_CHILD_RELAYOUT) {
            damage_from_children.remove(POS_CHILD_RELAYOUT);
            if self.is_containing_block_for_some(node_id) {
                damage_from_children.insert(RestyleDamage::RELAYOUT);
            } else {
                pos_child = true;
            }
        }
        damage |= damage_from_children;

        // Put children back
        self.nodes[node_id].children = children;

        if reorder {
            damage.insert(CONSTRUCT_FC);
        }

        let reconstructs = damage.intersects(CONSTRUCT_BOX | CONSTRUCT_FC);
        if reconstructs {
            damage.insert(RestyleDamage::RELAYOUT);
        }

        // Compute damage to propagate to parent. Reconstruction stops at the
        // box that owns it: the parent re-collects its layout children only
        // when this box itself changed, or when this node's children are
        // laid out by the parent (or further up).
        let mut damage_for_parent = damage - (CONSTRUCT_BOX | CONSTRUCT_FC);
        if reconstructs {
            damage_for_parent.insert(CONSTRUCT_DESCENDENT);
            if own_damage.contains(CONSTRUCT_BOX) || forwards_construction(&self.nodes[node_id]) {
                damage_for_parent.insert(CONSTRUCT_FC);
            }
        }

        // An inline `<svg>` is rendered from a snapshot of its subtree (with
        // `currentColor` resolved) taken during box construction, so it must be
        // rebuilt whenever the style of the `<svg>` or any descendant changes.
        #[cfg(feature = "svg")]
        if !damage.is_empty()
            && self.nodes[node_id]
                .data
                .is_element_with_tag_name(&local_name!("svg"))
        {
            damage.insert(CONSTRUCT_BOX);
        }

        // Reconstruction replaces the anonymous boxes with freshly styled ones.
        if !own_damage.is_empty() && !damage.intersects(CONSTRUCT_BOX | CONSTRUCT_FC) {
            let anonymous_damage = self.refresh_anonymous_block_styles(node_id) - POS_CHILD_RELAYOUT;
            damage |= anonymous_damage;
            damage_for_parent |= anonymous_damage;
        }

        // An out-of-flow box relaid only for what is inside it (its own style
        // and box are unchanged, so is its static position) is a relayout
        // boundary: its ancestors keep their layout (see `POS_CHILD_RELAYOUT`).
        if damage_for_parent.contains(ONLY_RELAYOUT)
            && !damage_for_parent.contains(CONSTRUCT_FC)
            && !own_damage.intersects(ONLY_RELAYOUT | CONSTRUCT_BOX)
            && self.is_relayout_boundary(node_id)
        {
            damage_for_parent.remove(ONLY_RELAYOUT);
            pos_child = true;
        }
        if pos_child {
            damage_for_parent.insert(POS_CHILD_RELAYOUT);
        }

        let node = &mut self.nodes[node_id];

        // If the node or any of it's children have been mutated or their layout styles
        // have changed, then we should clear it's layout cache.
        // (Read before `ONLY_RELAYOUT` is dropped below: a cell relaid out for a change inside
        // it, with no construction, is a changed cell too.)
        let damage_reaches_layout = reaches_layout(damage);
        let mut invalidate_anonymous_ancestors = false;
        if damage.intersects(ONLY_RELAYOUT | CONSTRUCT_BOX | CONSTRUCT_FC) {
            node.clear_layout_cache();
            invalidate_anonymous_ancestors = true;
            damage.remove(ONLY_RELAYOUT);
        }

        // Store damage for current node
        node.set_damage(damage);

        // pyro: a table cell whose subtree changed is measured and laid out again
        // (`TableContext::changed_cells`); one whose paint alone changed is transformed and
        // visited in the paint tree again.
        if !damage.is_empty() {
            self.note_changed_table_cell(node_id, damage_reaches_layout);
        }

        // Anonymous boxes between this node and its nearest element ancestor
        // are not visited by the DOM walk, so invalidate them here by walking
        // `layout_parent`. Stop at the first non-anonymous node: elements are
        // DOM ancestors and clear themselves when RELAYOUT reaches them.
        //
        // The chain is always usable: anonymous blocks are only freed by the
        // reconstruct branch of `resolve_layout_children` (which immediately
        // re-collects and resets `layout_parent` for every child it visits,
        // recursing through the fresh anonymous blocks which are born with
        // `ALL_DAMAGE`) and by node removal (the subtree leaves the document).
        // So when this pass runs, a node's `layout_parent` chain is either
        // current or owned by a container which reconstructs this frame. A
        // stale key for a freed block yields `None` (slab keys are versioned).
        if invalidate_anonymous_ancestors {
            let mut current = self.nodes[node_id].layout_parent.get();
            while let Some(ancestor) = current.and_then(|id| self.nodes.get_mut(id)) {
                if !ancestor.is_anonymous() {
                    break;
                }
                ancestor.clear_layout_cache();
                current = ancestor.layout_parent.get();
            }
        }

        // let _is_fc_root = node
        //     .primary_styles()
        //     .map(|s| is_fc_root(&s))
        //     .unwrap_or(false);

        // if damage.contains(CONSTRUCT_BOX) {
        //     // damage_for_parent.insert(CONSTRUCT_FC | CONSTRUCT_DESCENDENT);
        //     damage_for_parent.insert(CONSTRUCT_BOX);
        // }

        // if damage.contains(CONSTRUCT_FC) {
        //     damage_for_parent.insert(CONSTRUCT_DESCENDENT);
        //     // if !is_fc_root {
        //     damage_for_parent.insert(CONSTRUCT_FC);
        //     // }
        // }

        // Propagate damage to parent
        damage_for_parent
    }

    /// Notes `node_id` in its table if it is a table cell: to transform and visit in the paint
    /// tree again, and, when its damage reaches `layout`, as a changed cell to measure and lay
    /// out again.
    fn note_changed_table_cell(&self, node_id: NodeId, layout: bool) {
        let node = &self.nodes[node_id];
        let index = node.layout_data().table_cell_index;
        if index == u32::MAX {
            return;
        }
        let Some(table) = node.layout_parent.get().and_then(|id| self.nodes.get(id)) else { return };
        let Some(element) = table.data.downcast_element() else { return };
        if let crate::node::SpecialElementData::TableRoot(ctx) = &element.special_data {
            if ctx.cells.get(index as usize).is_some_and(|cell| cell.node_id == node_id) {
                if layout {
                    let mut changed = ctx.changed_cells.borrow_mut();
                    changed.push(index as usize);
                    // (A table that is not laid out keeps them: no more than one per cell.)
                    if changed.len() > 2 * ctx.cells.len() {
                        changed.sort_unstable();
                        changed.dedup();
                    }
                    drop(changed);
                    ctx.widths.borrow_mut().changed(index as usize);
                }
                ctx.placed.borrow_mut().damaged(index as usize, ctx.cells.len());
            }
        }
    }

    /// Whether `node_id` is an out-of-flow box (with a box of its own, below
    /// the root element) whose relayout need not relayout its ancestors.
    fn is_relayout_boundary(&self, node_id: NodeId) -> bool {
        let node = &self.nodes[node_id];
        let Some(parent) = node.parent else { return false };
        // The root element is laid out by the root pass itself.
        if self.nodes[parent].parent.is_none() {
            return false;
        }
        node.is_out_of_flow()
            && node
                .display_style()
                .is_some_and(|d| d.inside() != DisplayInside::Contents && d != Display::None)
    }

    /// Whether the node, or an anonymous box it owns, can be the containing
    /// block of an out-of-flow box below it. A superset of what any Taffy
    /// layout claims, not narrowed by position kind: its style claims
    /// (`style_claims_out_of_flow`), or it or an anonymous inline root it owns
    /// claims for a span (`inline_root_claims_out_of_flow`; the spans
    /// themselves are DOM ancestors of the boxes inside them and claim by
    /// style).
    fn is_containing_block_for_some(&self, node_id: NodeId) -> bool {
        fn owns_claiming_inline_root(doc: &BaseDocument, node: &Node) -> bool {
            inline_root_claims_out_of_flow(node)
                || node.anonymous_blocks.iter().any(|&id| {
                    // Owners can list anonymous boxes freed by construction.
                    doc.nodes.get(id).is_some_and(|anon| owns_claiming_inline_root(doc, anon))
                })
        }
        let node = &self.nodes[node_id];
        style_claims_out_of_flow(node) || owns_claiming_inline_root(self, node)
    }

    /// Refresh owned anonymous boxes without revisiting their DOM children.
    fn refresh_anonymous_block_styles(&mut self, node_id: NodeId) -> RestyleDamage {
        // Skip owners with no anonymous boxes to refresh.
        if self.nodes[node_id].anonymous_blocks.is_empty() {
            return RestyleDamage::empty();
        }

        // Cascade once from the owner, sharing the result across its anonymous boxes.
        let style = {
            let Some(parent_style) = self.nodes[node_id].primary_styles() else {
                return RestyleDamage::empty();
            };
            let read_guard = self.guard.read();
            let guards = StylesheetGuards::same(&read_guard);
            self.stylist.style_for_anonymous::<&Node>(
                &guards,
                &PseudoElement::ServoAnonymousBox,
                &parent_style,
            )
        };

        // Take the ownership list so we can mutate nodes without copying it.
        let anonymous_blocks = std::mem::take(&mut self.nodes[node_id].anonymous_blocks);
        let mut damage_for_parent = RestyleDamage::empty();
        for &anonymous_id in &anonymous_blocks {
            let (mut damage, changed, layout_changed) = {
                // Skip boxes whose node, style data, or previous style is unavailable.
                let Some(node) = self.nodes.get_mut(anonymous_id) else {
                    continue;
                };
                let Some(mut data) = node.try_stylo_element_data_mut().and_then(|s| s.get_mut())
                else {
                    continue;
                };
                let Some(old_style) = data.styles.primary.as_ref() else {
                    continue;
                };

                // Compute precise damage, retaining the old style allocation if unchanged.
                let diff = RestyleDamage::compute_style_difference::<&Node>(old_style, &style);
                let changed = matches!(diff.change, StyleChange::Changed { .. });
                if changed {
                    data.styles.primary = Some(style.clone());
                    data.set_restyled();
                }
                let layout_changed = changed && LayoutStyles::affected_by(diff.damage);
                (data.damage | diff.damage, changed, layout_changed)
            };
            // A paint-only change keeps the layout styles derived from the old style.
            if layout_changed {
                self.nodes[anonymous_id].update_layout_styles();
            } else if changed {
                #[cfg(debug_assertions)]
                self.nodes[anonymous_id].assert_layout_styles_are_current();
            }

            // Refresh nested owned boxes unless reconstruction will replace them.
            if changed && !damage.intersects(CONSTRUCT_BOX | CONSTRUCT_FC) {
                damage |= crate::util::ensure_stack(|| self.refresh_anonymous_block_styles(anonymous_id));
            }

            // Reconstruction also requires relayout; propagate all damage to the owner.
            if damage.contains(CONSTRUCT_BOX) {
                damage.insert(RestyleDamage::RELAYOUT);
            }
            damage_for_parent |= damage;

            // Invalidate cached layout and consume only the relayout bit.
            let node = &mut self.nodes[anonymous_id];
            if damage.intersects(ONLY_RELAYOUT | CONSTRUCT_BOX) {
                node.clear_layout_cache();
                damage.remove(ONLY_RELAYOUT);
            }

            // Store remaining damage and mark the ancestor path when needed.
            node.set_damage(damage);
            if !damage.is_empty() {
                node.mark_damaged();
            }
        }

        // Restore the ownership list and return the combined damage.
        self.nodes[node_id].anonymous_blocks = anonymous_blocks;
        damage_for_parent
    }

    /// Mark every node in the document with `ALL_DAMAGE` (non-incremental
    /// mode). Iterates the node slotmap directly rather than walking the tree,
    /// which is cheaper and also reaches pseudo-elements and anonymous boxes
    /// without special-casing them. Detached nodes get marked too, which is
    /// harmless: they are reconstructed on re-attachment anyway.
    pub(crate) fn mark_all_damaged(&mut self) {
        for (_, node) in self.nodes.iter_mut() {
            node.insert_damage(ALL_DAMAGE);
        }
    }

    /// Clear damage and the `damaged_descendants`/`dirty_descendants` flags
    /// on all nodes which may carry them, using the `damaged_descendants`
    /// flags to skip clean subtrees (mirroring `propagate_damage_flags`).
    ///
    /// Every node is visited exactly once: DOM nodes through their parent's
    /// `children`, pseudo-elements through their owner's `before`/`after`,
    /// anonymous boxes through their owner's `anonymous_blocks`. An anonymous
    /// box's `children` are DOM nodes already reached through its owner, so
    /// they are not recursed into.
    pub(crate) fn clear_damage_and_dirty_flags(&mut self, node_id: NodeId) {
        // Anonymous boxes can be freed during construction while still
        // listed in their owner's `anonymous_blocks`.
        let Some(node) = self.nodes.get_mut(node_id) else {
            return;
        };
        let is_anonymous = node.is_anonymous();
        let has_damage = node.damage().is_some_and(|d| !d.is_empty());
        if !has_damage && !node.has_damaged_descendants() && !is_anonymous {
            return;
        }
        let anonymous_blocks = std::mem::take(&mut node.anonymous_blocks);
        let children = std::mem::take(&mut node.children);
        let (before, after) = (node.before(), node.after());

        for anon_id in anonymous_blocks.iter() {
            crate::util::ensure_stack(|| self.clear_damage_and_dirty_flags(*anon_id));
        }
        if !is_anonymous {
            for child in children.iter() {
                // pyro: a child with no damage below it would return at the skip above
                // (`Node::has_no_damage_below`); it is not entered.
                if self.nodes.get(*child).is_none_or(|child| child.has_no_damage_below()) {
                    continue;
                }
                crate::util::ensure_stack(|| self.clear_damage_and_dirty_flags(*child));
            }
        }
        if let Some(before_id) = before {
            crate::util::ensure_stack(|| self.clear_damage_and_dirty_flags(before_id));
        }
        if let Some(after_id) = after {
            crate::util::ensure_stack(|| self.clear_damage_and_dirty_flags(after_id));
        }

        let node = &mut self.nodes[node_id];
        node.anonymous_blocks = anonymous_blocks;
        node.children = children;
        // The paint tree is built lazily (`ensure_paint_tree`), possibly after
        // several resolves: remember that this resolve's damage may have
        // changed the node's paint lists before the damage is dropped.
        if node
            .damage()
            .is_some_and(|d| d.contains(RestyleDamage::REBUILD_STACKING_CONTEXT))
        {
            node.flags.insert(NodeFlags::PAINT_TREE_DIRTY);
            // (A table keeps its paint children only across changes inside its
            // cells: whether it changed itself is kept with the flag.)
            if node.flags.contains(NodeFlags::OWN_DAMAGE) {
                node.flags.insert(NodeFlags::PAINT_TREE_OWN_DAMAGE);
            }
        }
        node.clear_damage_mut();
        node.flags.remove(NodeFlags::OWN_DAMAGE | NodeFlags::OWN_LAYOUT_DAMAGE);
        node.unset_damaged_descendants();
        node.unset_dirty_descendants();
    }
}

/// Whether the node's layout children are collected by an ancestor's
/// construction rather than (only) its own, so that construction damage on it
/// must reconstruct its parent too:
/// - `display: inline` boxes: a non-atomic one's content is part of the
///   inline root's inline layout (`find_inline_layout_embedded_boxes`), and a
///   block among any one's children (replaced elements included) changes how
///   the container partitions its children (`is_or_contains_block`);
/// - `display: contents`: its children are hoisted into the parent's box;
/// - table rows, row groups, columns and column groups: they are never layout
///   children; the table collects their cells into its `TableContext` (a
///   part whose own style change reaches layout carries `CONSTRUCT_BOX`
///   from `compute_layout_damage`, so it reaches the table this way too);
/// - list items: their markers are numbered by the list's construction
///   (`collect_list_item_children`).
///
/// Forwarding repeats at each such ancestor, so damage reaches the first box
/// that owns the content (the inline root, the table, the first non-contents
/// ancestor).
fn forwards_construction(node: &Node) -> bool {
    let Some(display) = node.display_style() else {
        return false;
    };
    if display.is_list_item() {
        return true;
    }
    match display.inside() {
        DisplayInside::Contents => true,
        // Replaced inline elements (`<canvas>`, `<svg>`, ...) included: they
        // are atomic in the inline layout, but `is_or_contains_block` looks
        // through any `display: inline` element at its children, so a block
        // among them changes how the parent partitions its children.
        DisplayInside::Flow => display.outside() == DisplayOutside::Inline,
        inside => is_table_part_inside(inside),
    }
}

/// Whether the node's style makes it the containing block of out-of-flow
/// boxes below it: positioned, or establishing one for `absolute` or `fixed`
/// boxes (transforms, filters, containment, `will-change`, size containers).
fn style_claims_out_of_flow(node: &Node) -> bool {
    node.primary_styles().is_some_and(|style| {
        let claims = stylo_taffy::convert::containing_block_claims(&style);
        claims.absolute || claims.fixed
    })
}

/// Whether the node is an inline root that claims the out-of-flow boxes of its
/// subtree for a positioned (or filtered) span with an inline box below it
/// (`TextLayout::span_cb_flags`, set by its last layout and added to its
/// layout style). The claim covers every out-of-flow box of the inline root,
/// also those outside the span, whose DOM ancestors may claim nothing.
fn inline_root_claims_out_of_flow(node: &Node) -> bool {
    node.element_data()
        .and_then(|element| element.inline_layout_data.as_deref())
        .is_some_and(|layout| !layout.span_cb_flags.is_empty())
}

/// Table-internal boxes which are not layout children: the table reads them
/// (and their cells) directly.
fn is_table_part_inside(inside: DisplayInside) -> bool {
    matches!(
        inside,
        DisplayInside::TableRow
            | DisplayInside::TableRowGroup
            | DisplayInside::TableHeaderGroup
            | DisplayInside::TableFooterGroup
            | DisplayInside::TableColumn
            | DisplayInside::TableColumnGroup
    )
}

// fn is_fc_root(style: &ComputedValues) -> bool {
//     let display = style.clone_display();
//     let display_inside = display.inside();

//     match display_inside {
//         DisplayInside::Flow => {
//             // Depends on parent context
//             false
//         }

//         DisplayInside::None => true,
//         DisplayInside::FlowRoot => true,
//         DisplayInside::Flex => true,
//         DisplayInside::Grid => true,
//         DisplayInside::Table => true,
//         DisplayInside::TableCell => true,

//         DisplayInside::Contents => false,
//         DisplayInside::TableRowGroup => false,
//         DisplayInside::TableColumn => false,
//         DisplayInside::TableColumnGroup => false,
//         DisplayInside::TableHeaderGroup => false,
//         DisplayInside::TableFooterGroup => false,
//         DisplayInside::TableRow => false,
//     }
// }

pub(crate) fn compute_layout_damage(old: &ComputedValues, new: &ComputedValues) -> RestyleDamage {
    let box_tree_needs_rebuild = || {
        let old_box = old.get_box();
        let new_box = new.get_box();

        // (`visibility` is paint: hit testing and paint read it from the style. Servo's box
        // tree needed it; Blitz's does not, and Stylo gives it `REPAINT` damage.)
        if old_box.display != new_box.display
            || old_box.float != new_box.float
            || old_box.position != new_box.position
            || old_box.contain != new_box.contain
        {
            return true;
        }

        if old.get_font() != new.get_font() {
            return true;
        }

        if new_box.display.outside() == DisplayOutside::Block
            && new_box.display.inside() == DisplayInside::Flow
        {
            let alignment_establishes_new_block_formatting_context = |style: &ComputedValues| {
                style.get_position().align_content.primary() != AlignFlags::NORMAL
            };

            let old_column = old.get_column();
            let new_column = new.get_column();
            if old_box.overflow_x.is_scrollable() != new_box.overflow_x.is_scrollable()
                || old_column.is_multicol() != new_column.is_multicol()
                || old_column.column_span != new_column.column_span
                || alignment_establishes_new_block_formatting_context(old)
                    != alignment_establishes_new_block_formatting_context(new)
            {
                return true;
            }
        }

        if old_box.display.is_list_item() {
            let old_list = old.get_list();
            let new_list = new.get_list();
            if old_list.list_style_position != new_list.list_style_position
                || old_list.list_style_image != new_list.list_style_image
                || (new_list.list_style_image == StyloImage::None
                    && old_list.list_style_type != new_list.list_style_type)
            {
                return true;
            }
        }

        if new.is_pseudo_style() && old.get_counters().content != new.get_counters().content {
            return true;
        }

        false
    };

    // Everything `stylo_to_parley::style` and the inline build read besides the font struct
    // (checked above): parley shapes the text with them.
    let text_shaping_needs_recollect = || {
        if old.clone_direction() != new.clone_direction()
            || old.clone_unicode_bidi() != new.clone_unicode_bidi()
        {
            return true;
        }

        let old_text = old.get_inherited_text();
        let new_text = new.get_inherited_text();
        if !std::ptr::eq(old_text, new_text)
            && (old_text.white_space_collapse != new_text.white_space_collapse
                || old_text.text_wrap_mode != new_text.text_wrap_mode
                || old_text.text_transform != new_text.text_transform
                || old_text.word_break != new_text.word_break
                || old_text.line_break != new_text.line_break
                || old_text.overflow_wrap != new_text.overflow_wrap
                || old_text.letter_spacing != new_text.letter_spacing
                || old_text.word_spacing != new_text.word_spacing
                || old_text.text_rendering != new_text.text_rendering)
        {
            return true;
        }

        // `vertical-align` (its longhands) is a span's vertical alignment in parley.
        let old_box = old.get_box();
        let new_box = new.get_box();
        if !std::ptr::eq(old_box, new_box)
            && (old_box.alignment_baseline != new_box.alignment_baseline
                || old_box.baseline_shift != new_box.baseline_shift
                || old_box.baseline_source != new_box.baseline_source)
        {
            return true;
        }

        false
    };

    // pyro: a table keeps the layout styles of its parts in its `TableContext`, built
    // with its box: a part's (or the table's) layout change builds it again, as a mutation does
    // (with the text in it shaped as it was: `BOX_DAMAGE`).
    let table_needs_rebuild = || {
        let display = new.get_box().display;
        display.outside() == DisplayOutside::InternalTable || display.inside() == DisplayInside::Table
    };

    #[allow(
        clippy::if_same_then_else,
        reason = "these branches will soon be different"
    )]
    if box_tree_needs_rebuild() {
        ALL_DAMAGE
    } else if text_shaping_needs_recollect() {
        ALL_DAMAGE
    } else if table_needs_rebuild() {
        BOX_DAMAGE
    } else if old.get_position().order != new.get_position().order
        // `position` is unchanged here (else the box tree would be rebuilt), so
        // checking the new style suffices. Out-of-flow children are not flex/grid
        // items: `Node::order()` keys them as 0 regardless of `order`.
        && !matches!(
            new.get_box().position,
            Position::Absolute | Position::Fixed
        )
    {
        RestyleDamage::RELAYOUT | REORDER_CHILDREN
    } else {
        // This element needs to be laid out again, but does not have any damage to
        // its box. In the future, we will distinguish between types of damage to the
        // fragment as well.
        RestyleDamage::RELAYOUT
    }
}

/// Stable-sort a flex/grid container's freshly constructed `layout_children`
/// (in document order) by `order`. Skips the sort when every child has the
/// default `order` (0).
pub(crate) fn sort_layout_children_by_order(
    nodes: &NodeTree,
    layout_children: &mut ThinVec<NodeId>,
) {
    if layout_children.iter().all(|id| nodes[*id].order() == 0) {
        return;
    }
    layout_children.sort_by_cached_key(|id| nodes[*id].order());
}

impl BaseDocument {
    pub(crate) fn invalidate_inline_contexts(&mut self) {
        let scale = self.viewport.scale();
        // Fonts or the scale changed: layouts built before are stale.
        self.layout_ctx.clear_build_cache();

        let font_ctx = &self.font_ctx;
        let layout_ctx = &mut self.layout_ctx;

        let mut anon_nodes = Vec::new();

        for (_, node) in self.nodes.iter_mut() {
            if !(node.flags.contains(NodeFlags::IS_IN_DOCUMENT)) {
                continue;
            }

            let Some(element) = node.data.downcast_element_mut() else {
                continue;
            };

            if element.inline_layout_data.is_some() {
                if node.is_anonymous() {
                    anon_nodes.push(node.id);
                } else {
                    node.insert_damage(ALL_DAMAGE);
                }
            } else if let Some(input) = element.text_input_data_mut() {
                input.editor.set_scale(scale);
                let mut font_ctx = font_ctx.lock().unwrap();
                input.editor.refresh_layout(&mut font_ctx, layout_ctx);
                // pyro: the placeholder is shaped again too (a textarea's is broken into
                // lines again by the relayout).
                if let Some(placeholder) = input.placeholder.as_deref_mut() {
                    placeholder.relayout(&mut font_ctx, layout_ctx, scale, None);
                }
                node.insert_damage(ONLY_RELAYOUT);
            }
        }

        for node_id in anon_nodes {
            if let Some(parent_id) = *(self.nodes[node_id].layout_parent.get_mut()) {
                self.nodes[parent_id].insert_damage(ALL_DAMAGE);
            }
        }
    }

    /// Flush the image layers of nodes whose style changed during the last
    /// style traversal (or whose pseudo-element boxes were (re)constructed).
    pub(crate) fn flush_pending_style_images(&mut self) {
        let mut pending = std::mem::take(&mut self.pending_style_image_nodes);
        pending.sort_unstable();
        pending.dedup();
        for node_id in pending {
            // Anonymous boxes (including pseudo-elements) can be removed from
            // the slab between queueing and flushing; skip stale IDs.
            if !self.nodes.contains_key(node_id) {
                continue;
            }
            self.flush_image_layers_from_style(node_id, ImageLayerKind::Background);
            self.flush_image_layers_from_style(node_id, ImageLayerKind::Mask);
        }
    }

    /// Flush a CSS image layer list (`background-image` or `mask-image`) from style
    /// to dedicated storage on the node, fetching any images which are not yet loaded.
    fn flush_image_layers_from_style(&mut self, node_id: NodeId, kind: ImageLayerKind) {
        let doc_id = self.id();
        let node = self.nodes.get_mut(node_id).unwrap();
        // Clone the primary style `Arc` into an owned value so the immutable
        // borrow of `node` (held by the stylo element data guard) is released
        // before we take a mutable borrow of `node.data` below.
        let style = {
            let stylo_element_data = node.try_stylo_element_data().and_then(|s| s.get());
            let primary_styles = stylo_element_data
                .as_ref()
                .and_then(|data| data.styles.get_primary());
            let Some(style) = primary_styles else {
                return;
            };
            style.clone()
        };
        let Some(elem) = node.data.downcast_element_mut() else {
            return;
        };

        let (style_images, elem_images) = match kind {
            ImageLayerKind::Background => (
                &style.get_background().background_image.0,
                &mut elem.background_images,
            ),
            ImageLayerKind::Mask => (&style.get_svg().mask_image.0, &mut elem.mask_images),
        };

        let len = style_images.len();
        elem_images.resize(len, None);

        for idx in 0..len {
            let style_image = &style_images[idx];
            let new_image = match style_image {
                StyloImage::Url(ComputedUrl::Valid(new_url)) => {
                    let old_image = elem_images[idx].as_ref();
                    let old_image_url = old_image.map(|data| &data.url);
                    if old_image_url.is_some_and(|old_url| **new_url == **old_url) {
                        continue;
                    }

                    // Check cache first
                    let url_str = new_url.as_str();
                    if let Some(cached_image) = self.image_cache.get(url_str) {
                        #[cfg(feature = "tracing")]
                        tracing::info!("Loading image {url_str} from cache");
                        Some(ImageResourceData {
                            url: new_url.clone(),
                            status: Status::Ok,
                            image: cached_image.clone(),
                        })
                    } else if let Some(waiting_list) = self.pending_images.get_mut(url_str) {
                        // Image is already being fetched, queue this node
                        #[cfg(feature = "tracing")]
                        tracing::info!("Image {url_str} already pending, queueing node {node_id}");
                        waiting_list.push((node_id, kind.image_type(idx)));
                        Some(ImageResourceData::new(new_url.clone()))
                    } else {
                        // Start fetch and track as pending
                        #[cfg(feature = "tracing")]
                        tracing::info!("Fetching image {url_str}");
                        self.pending_images
                            .insert(url_str.to_string(), vec![(node_id, kind.image_type(idx))]);

                        self.net_provider.fetch(
                            doc_id,
                            crate::net::stamped_request(
                                (**new_url).clone(),
                                self.abort_signal.as_ref(),
                            ),
                            ResourceHandler::boxed(
                                self.tx.clone(),
                                doc_id,
                                None, // Don't pass node_id, we'll handle via pending_images
                                self.shell_provider.clone(),
                                ImageHandler::new(kind.image_type(idx)),
                            ),
                        );

                        Some(ImageResourceData::new(new_url.clone()))
                    }
                }
                _ => None,
            };

            // Element will always exist due to resize_with above
            elem_images[idx] = new_image;
        }
    }
}
