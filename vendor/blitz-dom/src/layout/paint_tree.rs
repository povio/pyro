//! Construction of the paint tree (`Node::paint_children` and
//! `Node::stacking_context`) from the laid-out box tree.
//!
//! The pass runs before the first paint or hit test after layout
//! (`BaseDocument::ensure_paint_tree`) and builds *topology* only: which
//! node ids each node paints itself and which z-indexed descendants are hoisted
//! to the nearest stacking-context root. Geometry (a hoisted box's offset from
//! its stacking-context root) is derived at use time from `final_layout()` and
//! memoised per geometry generation, so replaying a cached list for a clean
//! subtree can never paint or hit-test a box where layout no longer puts it.

use blitz_traits::node_id::NodeId;
use std::cell::Cell;
use std::ops::Range;
use stylo_taffy::CopyStyleGetters;
use thin_vec::ThinVec;

use crate::layout::damage::ALL_DAMAGE;
use crate::node::NodeFlags;
use crate::tree::NodeTree;
use crate::{BaseDocument, Node};
use style::properties::generated::longhands::position::computed_value::T as Position;
use style::selector_parser::RestyleDamage;
use style::values::computed::Float;
use style::values::specified::box_::DisplayInside;
use taffy::{Point, Rect};

/// Offset of the hoisted child `child_id` from the border box of its
/// stacking-context root `sc_root_id`, excluding the child's own
/// `final_layout().location`: the sum of `location - scroll_offset` of every
/// box strictly between the two on the child's positioning chain.
///
/// Walks `Node::containing_block` (the out-of-flow containing block for boxes
/// hoisted by Taffy, otherwise `layout_parent`, which is set for every layout
/// child by `resolve_layout_children`, anonymous blocks included).
pub fn hoisted_child_position(tree: &NodeTree, sc_root_id: NodeId, child_id: NodeId) -> Point<f32> {
    let child = &tree[child_id];
    let mut position = Point::ZERO;
    let mut current = child.containing_block();

    // A fixed-position box whose containing block is the root element is
    // positioned against the viewport and does not scroll with it: cancel the
    // scroll the walk below (or the stacking-context root itself) applies.
    // A fixed box contained by a transformed ancestor scrolls with it.
    if child.taffy_position() == taffy::Position::Fixed
        && let Some(cb) = current.and_then(|id| tree.get(id))
        && cb.is_root_element()
    {
        let scroll_offset = *cb.scroll_offset();
        let viewport_scroll = tree.viewport_scroll();
        position.x += scroll_offset.x as f32 + viewport_scroll.x as f32;
        position.y += scroll_offset.y as f32 + viewport_scroll.y as f32;
    }

    let mut reached_context = false;
    while let Some(id) = current {
        if id == sc_root_id {
            reached_context = true;
            break;
        }
        let Some(node) = tree.get(id) else {
            break;
        };
        let location = node.final_layout().location;
        let scroll_offset = *node.scroll_offset();
        position.x += location.x - scroll_offset.x as f32;
        position.y += location.y - scroll_offset.y as f32;
        current = node.containing_block();
    }
    if !reached_context {
        // A viewport-positioned descendant still belongs to an ancestor's
        // stacking context even when its containing block lies outside it.
        // Convert the containing-block coordinates into that context's frame.
        let mut ancestor = Some(sc_root_id);
        while let Some(id) = ancestor {
            let Some(node) = tree.get(id) else { break };
            let location = node.final_layout().location;
            let scroll = node.scroll_offset();
            position.x -= location.x - scroll.x as f32;
            position.y -= location.y - scroll.y as f32;
            let cb = node.containing_block();
            if node.taffy_position() == taffy::Position::Fixed
                && cb.and_then(|id| tree.get(id)).is_some_and(|n| n.is_root_element())
            {
                let root_scroll = tree[cb.unwrap()].scroll_offset();
                let viewport_scroll = tree.viewport_scroll();
                position.x -= root_scroll.x as f32 + viewport_scroll.x as f32;
                position.y -= root_scroll.y as f32 + viewport_scroll.y as f32;
            }
            ancestor = cb;
        }
    }

    position
}

/// A child with a z_index that is hoisted up to it's containing Stacking Context for paint purposes
#[derive(Debug, Clone)]
pub struct HoistedPaintChild {
    pub node_id: NodeId,
    pub z_index: i32,
    /// `(geometry generation, offset from the stacking-context root)`, see
    /// [`Self::position`]. Generation `0` is never current.
    position: Cell<(u64, Point<f32>)>,
}

impl HoistedPaintChild {
    fn new(node_id: NodeId, z_index: i32) -> Self {
        Self {
            node_id,
            z_index,
            position: Cell::new((0, Point::ZERO)),
        }
    }

    /// The child's offset from the border box of its stacking-context root
    /// (`sc_root_id`), excluding the child's own `final_layout().location`.
    /// Computed at most once per geometry generation.
    pub fn position(&self, tree: &NodeTree, sc_root_id: NodeId) -> Point<f32> {
        let generation = tree.geometry_generation();
        let (stamp, cached) = self.position.get();
        if stamp == generation {
            return cached;
        }
        let position = hoisted_child_position(tree, sc_root_id, self.node_id);
        self.position.set((generation, position));
        position
    }

    /// Whether [`Self::position`] is memoised for the current geometry generation.
    pub fn position_is_cached(&self, tree: &NodeTree) -> bool {
        self.position.get().0 == tree.geometry_generation()
    }
}

#[derive(Debug)]
pub struct StackingContext {
    pub children: ThinVec<HoistedPaintChild>,
    /// The number of hoisted point children with negative z_index
    pub negative_z_count: u32,
    /// `(geometry generation, bounding box of the hoisted children)`, see
    /// [`Self::hoisted_content_bbox`].
    hoisted_content_bbox: Cell<(u64, Rect<f32>)>,
    /// pyro: out-of-flow boxes without a `z-index` whose containing block lies outside this
    /// context, painted at its positioned level from its `paint_children`
    /// (`restore_out_of_flow_stacking_owners`): in [`Self::hoisted_content_bbox`] too.
    pub(crate) positioned_level: ThinVec<HoistedPaintChild>,
}

impl StackingContext {
    /// Build a sorted stacking context from the hoisted entries collected for it.
    fn from_children(children: ThinVec<HoistedPaintChild>) -> Self {
        let mut sc = Self {
            children,
            negative_z_count: 0,
            hoisted_content_bbox: Cell::new((0, Rect::ZERO)),
            positioned_level: ThinVec::new(),
        };
        sc.sort();
        sc
    }

    /// Bounding box (in CSS pixels relative to the stacking-context root
    /// `sc_root_id`, unscrolled) of the scrollable overflow of all hoisted
    /// children, including their transforms. `scale` is the device pixel
    /// ratio `scrollable_overflow` is stored in. Computed at most once per
    /// geometry generation.
    pub fn hoisted_content_bbox(
        &self,
        tree: &NodeTree,
        sc_root_id: NodeId,
        scale: f64,
    ) -> Rect<f32> {
        let generation = tree.geometry_generation();
        let (stamp, cached) = self.hoisted_content_bbox.get();
        if stamp == generation {
            return cached;
        }

        let child_rect = |child: &HoistedPaintChild| -> Option<Rect<f32>> {
            let node = tree.get(child.node_id)?;
            let position = child.position(tree, sc_root_id);
            let location = node.final_layout().location;
            let mut overflow = *node.scrollable_overflow();
            if let Some(transform) = node.transform().as_deref() {
                overflow = transform.transform_rect_bbox(overflow);
            }
            let left = position.x + location.x;
            let top = position.y + location.y;
            Some(Rect {
                left: left + (overflow.x0 / scale) as f32,
                top: top + (overflow.y0 / scale) as f32,
                right: left + (overflow.x1 / scale) as f32,
                bottom: top + (overflow.y1 / scale) as f32,
            })
        };

        // Children dropped by a DOM mutation since the last `resolve` yield
        // `None` and are left out, like hit testing skips them.
        let mut rects = self.children.iter().chain(&self.positioned_level).filter_map(child_rect);
        let mut area = rects.next().unwrap_or(Rect::ZERO);
        for pos in rects {
            area.left = area.left.min(pos.left);
            area.top = area.top.min(pos.top);
            area.right = area.right.max(pos.right);
            area.bottom = area.bottom.max(pos.bottom);
        }
        self.hoisted_content_bbox.set((generation, area));
        area
    }

    pub fn sort(&mut self) {
        self.children.sort_by_key(|c| c.z_index);
        self.negative_z_count = self.children.iter().take_while(|c| c.z_index < 0).count() as u32;
    }

    pub fn neg_z_range(&self) -> Range<usize> {
        0..(self.negative_z_count as usize)
    }

    pub fn pos_z_range(&self) -> Range<usize> {
        (self.negative_z_count as usize)..self.children.len()
    }

    pub fn neg_z_hoisted_children(
        &self,
    ) -> impl ExactSizeIterator<Item = &HoistedPaintChild> + DoubleEndedIterator {
        self.children[self.neg_z_range()].iter()
    }

    pub fn pos_z_hoisted_children(
        &self,
    ) -> impl ExactSizeIterator<Item = &HoistedPaintChild> + DoubleEndedIterator {
        self.children[self.pos_z_range()].iter()
    }
}

impl BaseDocument {
    /// Bring `paint_children` and `stacking_context` up to date with the last
    /// resolve, here and in every sub-document. Everything that reads them
    /// calls this first: the blitz-paint entry points and hit testing
    /// ([`hit`](Self::hit) and the methods built on it). Resolve only marks
    /// the tree stale, so a run of forced layouts with no paint or hit test
    /// in between builds it once.
    pub fn ensure_paint_tree(&mut self) {
        if std::mem::take(&mut self.paint_tree_dirty)
            && let Some(root_id) = self.try_root_element().map(|root| root.id)
        {
            self.build_paint_tree(root_id);
            debug_assert!(
                self.paint_tree_matches_fresh_build(),
                "the incrementally built paint tree differs from a fresh build"
            );
        }
        for &node_id in &self.sub_document_nodes {
            if let Some(mut sub_doc) = self
                .nodes
                .get_mut(node_id)
                .and_then(|node| node.subdoc_mut())
                .map(|doc| doc.inner_mut())
            {
                sub_doc.ensure_paint_tree();
            }
        }
    }

    /// Whether the paint tree equals one built from scratch: snapshots every
    /// node's paint lists, rebuilds the whole tree with every node dirty and
    /// compares. Leaves the fresh build in place. For tests and debug
    /// assertions; it costs a full build.
    #[doc(hidden)]
    pub fn paint_tree_matches_fresh_build(&mut self) -> bool {
        let Some(root_id) = self.try_root_element().map(|root| root.id) else {
            return true;
        };
        let incremental = self.paint_tree_snapshot();
        for (_, node) in self.nodes.iter_mut() {
            node.flags
                .insert(NodeFlags::PAINT_TREE_DIRTY | NodeFlags::PAINT_TREE_OWN_DAMAGE);
        }
        self.build_paint_tree(root_id);
        incremental == self.paint_tree_snapshot()
    }

    /// Every node's paint lists, in slab order.
    fn paint_tree_snapshot(&self) -> Vec<PaintTreeEntry> {
        let hoisted = |children: &[HoistedPaintChild]| -> Vec<(NodeId, i32)> {
            children.iter().map(|c| (c.node_id, c.z_index)).collect()
        };
        self.nodes
            .iter()
            .map(|(id, node)| PaintTreeEntry {
                id,
                paint_children: node.paint_children.borrow().as_ref().map(|c| c.to_vec()),
                // pyro: with the boxes moved into its positioned level (in any order: they only
                // widen its hoisted content's bounds).
                stacking_context: node.stacking_context.as_ref().map(|sc| {
                    let mut moved = hoisted(&sc.positioned_level);
                    moved.sort_unstable_by_key(|&(id, z)| (id.as_u64(), z));
                    (hoisted(&sc.children), sc.negative_z_count, moved)
                }),
                contribution: hoisted(&node.sc_contribution_cache.borrow()),
            })
            .collect()
    }

    /// Rebuild `paint_children` and `stacking_context` for the tree rooted at
    /// `root_id` from the laid-out box tree.
    ///
    /// Each node's layout children are placed either in its `paint_children`
    /// (sorted by paint order) or, when they have a non-zero `z-index` and are
    /// positioned or flex/grid items, in the nearest ancestor stacking context.
    ///
    /// Subtrees not flagged `PAINT_TREE_DIRTY` (no resolve since the last
    /// build stored `REBUILD_STACKING_CONTEXT` damage on them) are skipped: their
    /// own lists are still valid and the entries they contributed to the
    /// enclosing stacking context are replayed from `Node::sc_contribution_cache`.
    fn build_paint_tree(&mut self, root_id: NodeId) {
        self.build_paint_tree_impl(root_id, None);
        self.restore_out_of_flow_stacking_owners(root_id);
    }

    /// Layout ownership follows containing blocks; paint ownership follows
    /// ancestor stacking contexts. Fixed descendants must not escape their
    /// modal/opacity context merely because layout positions them at the root.
    fn restore_out_of_flow_stacking_owners(&mut self, root_id: NodeId) {
        let context_of = |doc: &Self, mut node: Option<NodeId>| {
            while let Some(id) = node {
                if doc.nodes[id].stacking_context.is_some() { return id; }
                node = doc.nodes[id].parent;
            }
            root_id
        };
        self.oof_hoisted.retain(|id| self.nodes.get(*id).is_some_and(|n| n.oof_containing_block.get().is_some()));
        let mut moves: Vec<_> = self.oof_hoisted.iter().filter_map(|&id| {
            let node = &self.nodes[id];
            if !node.flags.is_in_document() { return None; }
            let cb = node.oof_containing_block.get()?;
            let target = context_of(self, node.parent);
            (target != context_of(self, Some(cb))).then_some((id, target))
        }).collect();
        if moves.is_empty() { return; }
        // In node slot order, as when every node was scanned (equal z-indices
        // paint in insertion order).
        moves.sort_unstable_by_key(|(id, _)| id.as_u64() as u32);
        let ids: std::collections::HashSet<_> = moves.iter().map(|(id, _)| *id).collect();
        // Remove stale cache contributions too: a clean subtree can replay
        // its lists on the next frame, but it must not reintroduce a duplicate.
        for (_, node) in self.nodes.iter_mut() {
            if let Some(children) = node.paint_children.get_mut().as_mut() { children.retain(|id| !ids.contains(id)); }
            node.sc_contribution_cache.get_mut().retain(|child| !ids.contains(&child.node_id));
            if let Some(sc) = node.stacking_context.as_mut() {
                sc.children.retain(|child| !ids.contains(&child.node_id));
                sc.positioned_level.retain(|child| !ids.contains(&child.node_id));
                sc.sort();
                sc.hoisted_content_bbox.set((0, Rect::ZERO));
            }
        }
        for (id, target) in moves {
            let z = self.nodes[id].z_index();
            // pyro: one without a `z-index` joins the context's positioned level in its
            // `paint_children`, in tree order (CSS 2.1 Appendix E step 8), not after it: a blending
            // box there must see the earlier ones under it (WPT `mix-blend-mode-root-element-group`).
            if z == 0 && self.nodes[target].stacking_context.is_some() {
                let mut paint_children = self.nodes[target].paint_children.borrow_mut().take().unwrap_or_default();
                let flex_or_grid = self.nodes[target]
                    .display_style()
                    .is_some_and(|display| matches!(display.inside(), DisplayInside::Flex | DisplayInside::Grid));
                let mut at = paint_children.len();
                while at > 0 {
                    let other = &self.nodes[paint_children[at - 1]];
                    let positioned_level = other.primary_styles().is_some_and(|s| s.clone_position() != Position::Static)
                        || self.is_paint_stacking_context_root(other.id, flex_or_grid);
                    if !positioned_level || self.precedes_in_tree_order(other.id, id) {
                        break;
                    }
                    at -= 1;
                }
                paint_children.insert(at, id);
                *self.nodes[target].paint_children.borrow_mut() = Some(paint_children);
                if let Some(sc) = self.nodes[target].stacking_context.as_mut() {
                    sc.positioned_level.push(HoistedPaintChild::new(id, 0));
                    sc.hoisted_content_bbox.set((0, Rect::ZERO));
                }
                continue;
            }
            if let Some(sc) = self.nodes[target].stacking_context.as_mut() {
                sc.children.push(HoistedPaintChild::new(id, z));
                sc.sort();
                sc.hoisted_content_bbox.set((0, Rect::ZERO));
            }
        }
    }

    /// `stacking_context` is the entry list of the nearest ancestor stacking
    /// context, or `None` when `node_id` is itself a stacking-context root.
    fn build_paint_tree_impl(
        &mut self,
        node_id: NodeId,
        stacking_context: Option<&mut ThinVec<HoistedPaintChild>>,
    ) {
        {
            let node = &self.nodes[node_id];
            // `propagate_damage_flags` stores the union of a node's own and
            // its descendants' damage on the node, so the absence of
            // `REBUILD_STACKING_CONTEXT` (which every heavier level implies)
            // means nothing in the subtree can have changed a paint list.
            // Anonymous boxes are never skipped: damage marking walks the DOM
            // parent chain, which bypasses them.
            //
            // Whether a node is a stacking-context root also depends on its
            // parent (z-indexed flex/grid items), which does not damage the
            // node itself, so the role the caller resolved must match the one
            // this node was last built with.
            let role_unchanged = node.stacking_context.is_some() == stacking_context.is_none();
            // The build runs after the resolves it catches up with, whose
            // damage is gone by then: `PAINT_TREE_DIRTY` records it. Damage
            // still stored (none after a resolve, as every resolve clears it)
            // is honoured too, so a build can only be more thorough.
            let clean = role_unchanged
                && !node.flags.contains(NodeFlags::PAINT_TREE_DIRTY)
                && !node
                    .damage()
                    .unwrap_or(ALL_DAMAGE)
                    .contains(RestyleDamage::REBUILD_STACKING_CONTEXT)
                && !node.is_anonymous();
            if clean {
                if let Some(stacking_context) = stacking_context {
                    stacking_context.extend(node.sc_contribution_cache.borrow().iter().cloned());
                }
                return;
            }
        }

        let own_damage = self.nodes[node_id].flags.contains(NodeFlags::PAINT_TREE_OWN_DAMAGE);
        self.nodes[node_id]
            .flags
            .remove(NodeFlags::PAINT_TREE_DIRTY | NodeFlags::PAINT_TREE_OWN_DAMAGE);

        // pyro: a table whose cells are all plain boxes in it visits again only the cells
        // that changed (`TableContext::placed`).
        if !own_damage && self.table_paint_children_kept(node_id) {
            return;
        }

        let Some(display) = self.nodes[node_id].display_style() else {
            return;
        };

        // Entries hoisted out of this subtree are collected directly into the
        // storage that will hold them afterwards: the node's own stacking
        // context for a root, otherwise its contribution cache (from which
        // they are copied into the enclosing stacking context below).
        let mut entries = {
            let node = &mut self.nodes[node_id];
            let mut entries = match stacking_context {
                None => node
                    .stacking_context
                    .take()
                    .map(|sc| sc.children)
                    .unwrap_or_default(),
                Some(_) => std::mem::take(&mut *node.sc_contribution_cache.borrow_mut()),
            };
            entries.clear();
            entries
        };

        let children = self.nodes[node_id].layout_children.borrow_mut().take();
        if let Some(children) = children {
            let is_flex_or_grid =
                matches!(display.inside(), DisplayInside::Flex | DisplayInside::Grid);

            // Out-of-flow boxes are laid out (and painted) relative to their
            // containing block, as recorded by Taffy's out-of-flow pass in the
            // containing block's `hoisted_children`. A box whose containing
            // block is not its layout parent is owned by the containing block;
            // its layout parent neither visits nor lists it.
            let owned_here = |doc: &Self, child_id: NodeId| -> bool {
                doc.nodes[child_id]
                    .oof_containing_block
                    .get()
                    .is_none_or(|cb| cb == node_id)
            };
            // Boxes hoisted to this node from further down the tree, keyed by
            // the index of the layout child they descend from: they are visited
            // and listed in tree order, directly after that child (CSS 2.1
            // Appendix E). Boxes whose layout-parent chain does not lead through
            // a layout child of this node come after all of them.
            let mut hoisted_past_parent: Vec<(usize, NodeId)> = self.nodes[node_id]
                .hoisted_children
                .borrow()
                .iter()
                .copied()
                .filter(|&id| {
                    self.nodes
                        .get(id)
                        .is_some_and(|n| n.layout_parent.get() != Some(node_id))
                })
                .map(|id| {
                    let anchor = self
                        .layout_child_containing(node_id, id)
                        .and_then(|anchor| children.iter().position(|&c| c == anchor))
                        .unwrap_or(children.len());
                    (anchor, id)
                })
                .collect();
            hoisted_past_parent.sort_by_key(|&(anchor, _)| anchor);
            let mut next_hoisted = 0;

            let is_layer_root =
                stacking_context.is_none() || self.nodes[node_id].is_paint_layer_root();
            let keyed_base = self.scratch.keyed_children.len();

            for (index, &child_id) in children.iter().enumerate() {
                // pyro: where this child's group starts in both lists (see `place_hoisted`).
                let keyed_group = self.scratch.keyed_children.len();
                let entries_group = entries.len();
                if owned_here(self, child_id) {
                    let is_sc_root = self.is_paint_stacking_context_root(child_id, is_flex_or_grid);
                    let first_entry = entries.len();
                    crate::util::ensure_stack(|| self.build_paint_tree_impl(
                        child_id,
                        match is_sc_root {
                            true => None,
                            false => Some(&mut entries),
                        },
                    ));

                    // Only stacking contexts with a non-zero z-index are lifted
                    // out of tree order into the enclosing stacking context's
                    // z-sorted list (CSS 2.1 Appendix E).
                    let child = &self.nodes[child_id];
                    // pyro: `z-index` applies to positioned boxes and to flex and grid items
                    // only (CSS 2 9.9.1): an in-flow stacking context that is neither (a
                    // transform, an opacity) keeps its place in tree order whatever its value.
                    let positioned = child
                        .primary_styles()
                        .is_some_and(|style| style.clone_position() != Position::Static);
                    let z_index = if is_flex_or_grid || positioned {
                        child.z_index()
                    } else {
                        0
                    };
                    let paint_order = node_to_paint_order(child, is_flex_or_grid);
                    // pyro: a stacking context that is not positioned (a transform, an opacity,
                    // a filter) paints as a positioned box with `z-index: 0` does (CSS 2.1
                    // Appendix E step 8; CSS Transforms 1, CSS Color 4 `opacity`): above the
                    // in-flow content and floats, in tree order with the positioned boxes.
                    let static_stacking_context = is_sc_root && !positioned;
                    if is_sc_root && z_index != 0 {
                        entries.push(HoistedPaintChild::new(child_id, z_index))
                    } else if !is_layer_root && (positioned || static_stacking_context) {
                        // Positioned boxes paint above all in-flow content of
                        // the nearest positioned (or clipping) ancestor, not just
                        // of their parent: lift them out of this static box.
                        // pyro: out-of-flow ones too, which a box that is no layer
                        // root lays out for a positioned or filtered inline (an
                        // anonymous block).
                        entries.push(HoistedPaintChild::new(child_id, 0))
                    } else if static_stacking_context {
                        self.scratch
                            .keyed_children
                            .push(((2, paint_order.1), child_id));
                    } else {
                        self.scratch.keyed_children.push((paint_order, child_id));
                    }

                    // Positioned descendants lifted out of the static boxes of
                    // this child's subtree join this node's positioned paint
                    // level, in tree order after the child.
                    if is_layer_root && entries.len() > first_entry {
                        let subtree_entries: ThinVec<_> = entries.drain(first_entry..).collect();
                        for entry in subtree_entries {
                            if entry.z_index == 0 {
                                self.scratch
                                    .keyed_children
                                    .push(((2, paint_order.1), entry.node_id));
                            } else {
                                entries.push(entry);
                            }
                        }
                    }
                }

                while let Some(&(anchor, hoisted_id)) = hoisted_past_parent.get(next_hoisted)
                    && anchor == index
                {
                    let keyed_before = self.scratch.keyed_children.len();
                    self.attach_hoisted_past_parent(
                        hoisted_id,
                        is_flex_or_grid,
                        is_layer_root,
                        &mut entries,
                    );
                    self.place_hoisted(keyed_before, keyed_group, entries_group, &mut entries);
                    next_hoisted += 1;
                }
            }
            for &(_, hoisted_id) in &hoisted_past_parent[next_hoisted..] {
                self.attach_hoisted_past_parent(
                    hoisted_id,
                    is_flex_or_grid,
                    is_layer_root,
                    &mut entries,
                );
            }

            self.scratch.keyed_children[keyed_base..].sort_by_key(|&(paint_order, _)| paint_order);

            let mut paint_children = self.nodes[node_id]
                .paint_children
                .borrow_mut()
                .take()
                .unwrap_or_default();
            paint_children.clear();
            paint_children.extend(
                self.scratch
                    .keyed_children
                    .drain(keyed_base..)
                    .map(|(_, id)| id),
            );

            self.note_table_paint_children(node_id, &children, &paint_children, entries.is_empty());
            let node = &self.nodes[node_id];
            *node.paint_children.borrow_mut() = Some(paint_children);
            *node.layout_children.borrow_mut() = Some(children);
        }

        let node = &mut self.nodes[node_id];
        if let Some(stacking_context) = stacking_context {
            node.stacking_context = None;
            stacking_context.extend(entries.iter().cloned());
            *node.sc_contribution_cache.borrow_mut() = entries;
        } else {
            node.sc_contribution_cache.borrow_mut().clear();
            node.stacking_context = Some(Box::new(StackingContext::from_children(entries)));
        }
    }

    /// Whether `id` roots a stacking context for painting: by its style, or
    /// (pyro) because it is in the top layer, which always forms
    /// one (CSS Positioned Layout 4 §4), so its z-indexed and fixed
    /// descendants stay in its subtree and paint with it above the document.
    #[inline]
    fn is_paint_stacking_context_root(&self, id: NodeId, is_flex_or_grid_item: bool) -> bool {
        self.nodes[id].is_stacking_context_root(is_flex_or_grid_item) || self.is_in_top_layer(id)
    }

    /// The table `node_id` (with `TableContext::placed` holding what changed) whose cells were all
    /// plain boxes in it when the paint tree last visited it: no stacking context, nothing hoisted out
    /// of their subtrees, one paint order. The cells damaged or appended since are visited again, and
    /// if they still are plain boxes its paint children are the same, plus the appended cells, and it
    /// adds nothing to the stacking context. `false`: the table is to be visited as any box. (The
    /// caller asks only for a table without damage of its own since the last build,
    /// `PAINT_TREE_OWN_DAMAGE`.)
    fn table_paint_children_kept(&mut self, node_id: NodeId) -> bool {
        use crate::node::SpecialElementData;
        let node = &self.nodes[node_id];
        if !node.flags.is_table_root()
            || node.before().is_some()
            || node.after().is_some()
            || !node.hoisted_children.borrow().is_empty()
        {
            return false;
        }
        let Some(SpecialElementData::TableRoot(ctx)) = node.data.downcast_element().map(|e| &e.special_data) else {
            return false;
        };
        if !crate::layout::table::keeps_cells(ctx) {
            return false;
        }
        let ctx = std::sync::Arc::clone(ctx);
        let (cells, order) = {
            let mut placed = ctx.placed.borrow_mut();
            let order = placed.plain_paint_order;
            match (placed.take_paint(), order) {
                (Some(cells), Some(order)) => (cells, order),
                _ => return false,
            }
        };
        let Some(mut paint_children) = node.paint_children.borrow_mut().take() else { return false };
        let kept = paint_children.len() <= ctx.cells.len()
            && paint_children.iter().zip(&ctx.cells).all(|(&id, cell)| id == cell.node_id);
        let appended = paint_children.len()..ctx.cells.len();
        let mut entries = ThinVec::new();
        let plain = kept
            && cells.iter().copied().chain(appended.clone()).all(|index| {
                let Some(cell) = ctx.cells.get(index) else { return false };
                let child = &self.nodes[cell.node_id];
                if child.is_stacking_context_root(false) || node_to_paint_order(child, false) != order {
                    return false;
                }
                self.build_paint_tree_impl(cell.node_id, Some(&mut entries));
                entries.is_empty()
            });
        if plain {
            paint_children.extend(ctx.cells[appended].iter().map(|cell| cell.node_id));
        }
        *self.nodes[node_id].paint_children.borrow_mut() = Some(paint_children);
        if !plain {
            ctx.placed.borrow_mut().plain_paint_order = None;
        }
        plain
    }

    /// After the paint tree visited the table `node_id` as any box: whether its cells are plain boxes
    /// in it (see `table_paint_children_kept`), and the cells changed since are none.
    fn note_table_paint_children(
        &self,
        node_id: NodeId,
        children: &[NodeId],
        paint_children: &[NodeId],
        no_entries: bool,
    ) {
        use crate::node::SpecialElementData;
        let node = &self.nodes[node_id];
        if !node.flags.is_table_root() {
            return;
        }
        let Some(SpecialElementData::TableRoot(ctx)) = node.data.downcast_element().map(|e| &e.special_data) else {
            return;
        };
        let mut placed = ctx.placed.borrow_mut();
        placed.take_paint();
        let first = children.first().map(|&id| node_to_paint_order(&self.nodes[id], false));
        let plain = crate::layout::table::keeps_cells(ctx)
            && no_entries
            && node.before().is_none()
            && node.after().is_none()
            && node.hoisted_children.borrow().is_empty()
            && children.len() == ctx.cells.len()
            && paint_children.len() == children.len()
            && children.iter().zip(&ctx.cells).all(|(&id, cell)| id == cell.node_id)
            && paint_children.iter().zip(children).all(|(a, b)| a == b)
            && children.iter().all(|&id| {
                let child = &self.nodes[id];
                !child.is_stacking_context_root(false) && Some(node_to_paint_order(child, false)) == first
            });
        placed.plain_paint_order = if plain { first } else { None };
    }

    /// The layout child of `node_id` whose subtree contains `id`, following
    /// `layout_parent`.
    fn layout_child_containing(&self, node_id: NodeId, id: NodeId) -> Option<NodeId> {
        let mut current = id;
        loop {
            let parent = self.nodes.get(current)?.layout_parent.get()?;
            if parent == node_id {
                return Some(current);
            }
            current = parent;
        }
    }

    /// Visit an out-of-flow box hoisted to its containing block past its
    /// layout parent and place it in the paint lists: z-indexed boxes go to
    /// the enclosing stacking context like any other z-indexed positioned
    /// child, the rest join `paint_children` at the same paint level as other
    /// positioned boxes (the caller's stable sort keeps tree order).
    ///
    /// pyro: under a box that is no layer root (an anonymous block laying out
    /// a positioned or filtered inline's out-of-flow boxes), they are lifted
    /// to the nearest one's positioned level, as in-flow positioned boxes are.
    fn attach_hoisted_past_parent(
        &mut self,
        child_id: NodeId,
        is_flex_or_grid: bool,
        is_layer_root: bool,
        entries: &mut ThinVec<HoistedPaintChild>,
    ) {
        self.build_paint_tree_impl(
            child_id,
            match self.is_paint_stacking_context_root(child_id, false) {
                true => None,
                false => Some(entries),
            },
        );

        let z_index = self.nodes[child_id].z_index();
        if z_index != 0 {
            entries.push(HoistedPaintChild::new(child_id, z_index));
        } else if !is_layer_root {
            entries.push(HoistedPaintChild::new(child_id, 0));
        } else {
            let paint_order = node_to_paint_order(&self.nodes[child_id], is_flex_or_grid);
            self.scratch.keyed_children.push((paint_order, child_id));
        }
    }

    /// pyro: puts the out-of-flow box `attach_hoisted_past_parent` just placed last in one of the
    /// lists in tree order among what the same layout child's subtree placed in that list at the
    /// same paint level (`keyed_group..` and `entries_group..`): the positioned boxes and stacking
    /// contexts lifted out of it. Appended after them, an absolutely positioned box painted under
    /// a later sibling's `clip-path` or `opacity` box although it comes first (CSS 2.1 Appendix E
    /// step 8: "in tree order"; WPT `clip-path-reference-restore`).
    fn place_hoisted(
        &mut self,
        keyed_before: usize,
        keyed_group: usize,
        entries_group: usize,
        entries: &mut ThinVec<HoistedPaintChild>,
    ) {
        if self.scratch.keyed_children.len() > keyed_before {
            let mut keyed = std::mem::take(&mut self.scratch.keyed_children);
            let at = self.tree_order_place(&keyed[keyed_group..], |&(key, id)| (key, id));
            keyed[keyed_group + at..].rotate_right(1);
            self.scratch.keyed_children = keyed;
        } else if entries.len() > entries_group {
            let at = self.tree_order_place(&entries[entries_group..], |c| ((c.z_index, 0), c.node_id));
            entries[entries_group + at..].rotate_right(1);
        }
    }

    /// pyro: where the last item of `items` goes among the others: before the first of those at
    /// its paint level (`item` gives the level and the node) that comes after it in tree order.
    /// Those are in tree order already (each subtree lists them so), so a binary search finds it.
    /// Items of other levels stay where they are (the paint sort orders the levels).
    fn tree_order_place<T>(&self, items: &[T], item: impl Fn(&T) -> ((i32, i32), NodeId)) -> usize {
        let Some((last, rest)) = items.split_last() else { return 0 };
        let (level, id) = item(last);
        // The usual case first, without a search: it comes after every other of its level.
        match rest.iter().rposition(|other| item(other).0 == level) {
            None => return rest.len(),
            Some(index) if self.precedes_in_tree_order(item(&rest[index]).1, id) => return rest.len(),
            Some(_) => {}
        }
        let same: Vec<usize> = (0..rest.len()).filter(|&index| item(&rest[index]).0 == level).collect();
        let before = same.partition_point(|&index| self.precedes_in_tree_order(item(&rest[index]).1, id));
        same.get(before).copied().unwrap_or(rest.len())
    }

    /// pyro: whether `a` comes before `b` in tree order, `::before` first in its element and
    /// `::after` last. A box no DOM child list holds (an anonymous block) sorts after its
    /// parent's children.
    fn precedes_in_tree_order(&self, a: NodeId, b: NodeId) -> bool {
        let parent = |id: NodeId| self.nodes.get(id).and_then(|node| node.parent);
        let depth = |mut id: NodeId| {
            let mut depth = 0usize;
            while let Some(p) = parent(id) {
                depth += 1;
                id = p;
            }
            depth
        };
        let (mut a, mut b) = (a, b);
        let (mut depth_a, mut depth_b) = (depth(a), depth(b));
        // An ancestor comes before its descendants.
        while depth_a > depth_b {
            a = parent(a).unwrap();
            depth_a -= 1;
            if a == b {
                return false;
            }
        }
        while depth_b > depth_a {
            b = parent(b).unwrap();
            depth_b -= 1;
            if a == b {
                return true;
            }
        }
        if a == b {
            return false;
        }
        loop {
            match (parent(a), parent(b)) {
                (Some(pa), Some(pb)) if pa == pb => {
                    let node = &self.nodes[pa];
                    let index = |id: NodeId| {
                        if node.before() == Some(id) {
                            0
                        } else if node.after() == Some(id) {
                            usize::MAX
                        } else {
                            node.children.iter().position(|&c| c == id).map_or(usize::MAX - 1, |i| i + 1)
                        }
                    };
                    return index(a) < index(b);
                }
                (Some(pa), Some(pb)) => {
                    a = pa;
                    b = pb;
                }
                _ => return false,
            }
        }
    }
}

impl Node {
    /// Whether this node paints its positioned descendants itself (CSS 2.1
    /// Appendix E step 8) rather than leaving them to an ancestor: positioned
    /// boxes, and boxes that clip their contents. Stacking-context roots are
    /// handled by the caller.
    fn is_paint_layer_root(&self) -> bool {
        use style::values::computed::{Contain, Overflow};

        let Some(style) = self.primary_styles() else {
            return true;
        };
        let box_style = style.get_box();
        style.clone_position() != Position::Static
            || !matches!(box_style.overflow_x, Overflow::Visible)
            || !matches!(box_style.overflow_y, Overflow::Visible)
            || box_style.slow_clone_contain().contains(Contain::PAINT)
            || self.element_data().is_some_and(|e| {
                e.raster_image_data().is_some()
                    || e.sub_doc_data().is_some()
                    || e.text_input_data().is_some()
            })
    }

    /// The offset from the border box of `painter_id` at which this node is
    /// painted, excluding its own `final_layout().location`, when it is in the
    /// paint list of `painter_id` without being laid out relative to it (a
    /// positioned box lifted past static ancestors).
    pub fn lifted_paint_offset(&self, painter_id: NodeId) -> Option<Point<f32>> {
        match self.containing_block() {
            Some(id) if id == painter_id => None,
            None => None,
            Some(_) => Some(hoisted_child_position(self.tree(), painter_id, self.id)),
        }
    }
}

/// One node's paint lists, for [`BaseDocument::paint_tree_matches_fresh_build`].
#[derive(PartialEq)]
struct PaintTreeEntry {
    id: NodeId,
    paint_children: Option<Vec<NodeId>>,
    stacking_context: Option<(Vec<(NodeId, i32)>, u32, Vec<(NodeId, i32)>)>,
    contribution: Vec<(NodeId, i32)>,
}

#[inline(always)]
fn position_to_order(pos: Position) -> i32 {
    match pos {
        Position::Static => 0,
        // All positioned descendants with z-index: auto share one paint
        // level (CSS 2.1 Appendix E step 8); the stable sort keeps them in
        // tree order among themselves, above in-flow content and floats.
        Position::Relative | Position::Sticky | Position::Absolute | Position::Fixed => 2,
    }
}
#[inline(always)]
fn float_to_order(pos: Float) -> i32 {
    match pos {
        Float::None => 0,
        _ => 1,
    }
}

/// Paint sort key: (paint level, order-modified position). Positioned
/// (z-index: auto) descendants paint above in-flow content (CSS 2.1
/// Appendix E step 8); within a level the stable sort preserves
/// (order-modified) document order.
#[inline(always)]
fn node_to_paint_order(node: &Node, is_flex_or_grid: bool) -> (i32, i32) {
    let Some(style) = node.primary_styles() else {
        return (0, 0);
    };
    let position = style.clone_position();
    if is_flex_or_grid {
        match position {
            Position::Static => (0, style.clone_order()),
            Position::Relative | Position::Sticky => (2, style.clone_order()),
            // Out-of-flow children are not flex/grid items: `order` does
            // not apply; tree order does.
            Position::Absolute | Position::Fixed => (2, 0),
        }
    } else {
        (
            position_to_order(position) + float_to_order(style.clone_float()),
            0,
        )
    }
}


/// The rank of a `top-layer=N` token in a state attribute value.
fn top_layer_rank(value: &str) -> Option<u32> {
    value
        .split_ascii_whitespace()
        .find_map(|t| t.strip_prefix("top-layer="))
        .and_then(|n| n.parse().ok())
}

/// pyro: the top layer as the embedder marks it: elements by rank
/// (bottom first, ties in id order) and each element's rank. Membership
/// tests, entering, leaving and re-ranking are logarithmic or constant, so
/// a page with many showing popovers costs no quadratic work per snapshot,
/// drop or painted box.
#[derive(Default)]
pub(crate) struct TopLayer {
    by_rank: std::collections::BTreeMap<(u32, u64), NodeId>,
    rank_of: std::collections::HashMap<NodeId, u32>,
}

impl TopLayer {
    #[inline]
    pub(crate) fn is_empty(&self) -> bool {
        self.rank_of.is_empty()
    }

    #[inline]
    pub(crate) fn contains(&self, id: NodeId) -> bool {
        self.rank_of.contains_key(&id)
    }

    /// Takes `id` out of the top layer (if it is in it).
    pub(crate) fn remove(&mut self, id: NodeId) {
        if let Some(rank) = self.rank_of.remove(&id) {
            self.by_rank.remove(&(rank, id.as_u64()));
        }
    }

    /// Puts `id` in the top layer at `rank`, or takes it out (`None`).
    fn set(&mut self, id: NodeId, rank: Option<u32>) {
        if self.rank_of.get(&id).copied() == rank {
            return;
        }
        self.remove(id);
        if let Some(rank) = rank {
            self.rank_of.insert(id, rank);
            self.by_rank.insert((rank, id.as_u64()), id);
        }
    }

    /// The elements, bottom first.
    pub(crate) fn iter(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.by_rank.values().copied()
    }
}

impl BaseDocument {
    /// pyro: element `id`'s state attribute is now `value` (`None`:
    /// removed). Keeps [`Self::top_layer_nodes`] in step: an element with a
    /// `top-layer=N` token is in the top layer at rank `N`.
    pub(crate) fn note_lp_state(&mut self, id: NodeId, value: Option<&str>) {
        let rank = value.and_then(top_layer_rank);
        if self.top_layer.is_empty() && rank.is_none() {
            return;
        }
        self.top_layer.set(id, rank);
    }

    /// pyro: whether any element is in the top layer (one length
    /// check).
    #[inline]
    pub fn has_top_layer(&self) -> bool {
        !self.top_layer.is_empty()
    }

    /// pyro: whether `id` is in the top layer: painted above the
    /// whole document (see [`Self::top_layer_nodes`]), not in its place.
    #[inline]
    pub fn is_in_top_layer(&self, id: NodeId) -> bool {
        !self.top_layer.is_empty() && self.top_layer.contains(id)
    }

    /// pyro: the elements of the top layer that are in the
    /// document, bottom first.
    pub fn top_layer_nodes(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.top_layer.iter().filter(|&id| {
            self.nodes
                .get(id)
                .is_some_and(|n| n.flags.is_in_document() && n.is_element())
        })
    }

    /// pyro: the offset of top-layer element `id` from the root
    /// element's border box (with the root's scroll applied), excluding its
    /// own location: where it paints, hoisted to the root's stacking context.
    pub fn top_layer_position(&self, id: NodeId) -> Point<f32> {
        match self.try_root_element() {
            Some(root) => hoisted_child_position(self.tree(), root.id, id),
            None => Point::ZERO,
        }
    }
}
