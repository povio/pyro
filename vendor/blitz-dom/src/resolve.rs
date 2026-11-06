//! Resolve style and layout

use stylo_taffy::CopyStyleGetters;
use blitz_traits::node_id::NodeId;
use std::cell::RefCell;

use debug_timer::debug_timer;
use kurbo::{Affine, Rect};
use parley::LayoutContext;
use selectors::Element as _;
use style::dom::TDocument;

#[cfg(feature = "parallel-construct")]
use rayon::prelude::*;

// FIXME: static thread_local FontCtx isn't necessarily correct in multi-document context.
// Should use thread_local crate with ThreadLocal value store in the Document.
thread_local! {
    pub(crate) static LAYOUT_CTX: RefCell<Option<Box<LayoutContext<TextBrush>>>> = const { RefCell::new(None) };
}

use taffy::AvailableSpace;

use crate::{
    layout::{
        construct::{
            ConstructionTask, ConstructionTaskData, ConstructionTaskResult,
            ConstructionTaskResultData, LayoutChildren, collect_layout_children,
        },
        damage::{ALL_DAMAGE, CONSTRUCT_BOX, CONSTRUCT_DESCENDENT, CONSTRUCT_FC},
        LayoutPassState,
        inline::build_inline_layout_into,
    },
    node::TextBrush,
    BaseDocument,
};

impl BaseDocument {
    /// Resolve styles, leaving construction, layout and their damage for the
    /// next [`Self::resolve`]. Size container queries are the exception: their
    /// computed values require feedback from the queried content boxes.
    pub fn resolve_style(&mut self, current_time_for_animations: f64) {
        if TDocument::as_node(&self.root_node()).first_element_child().is_none() {
            return;
        }
        self.handle_messages();
        if self.has_pending_critical_resources() {
            return;
        }
        // A viewport/device update can change percentage/auto content boxes
        // without changing their computed styles. Remember it before flushing.
        let device_changed = !self.pending_device_changes.is_empty();
        self.flush_pending_device_changes();
        self.resolve_stylist(current_time_for_animations);
        // Style reads must still discover background/mask resources. This
        // consumes only already-styled nodes, leaving construction/layout
        // damage intact for the next geometry query.
        self.flush_pending_style_images();
        let root = self.root_element();
        let geometry_dirty =
            device_changed || root.has_damaged_descendants() || root.damage().is_none_or(|damage| !damage.is_empty());
        // A size query is the exceptional style read which needs geometry.
        // Ordinary style reads leave construction/layout and damage untouched.
        if self.stylist.has_container_queries() {
            // Style-only queries also call the size adapter, including on normal
            // containers. Inspect final styles now that traversal has released
            // element data, and only lay out real size-container dependencies.
            self.nodes.container_query_sizes.lock().unwrap().retain(|id, _| {
                self.nodes.get(*id).is_some_and(|node| {
                    node.flags.is_in_document()
                        && node.primary_styles().is_some_and(|style| {
                            use style::values::specified::ContainerType;
                            style
                                .get_box()
                                .slow_clone_container_type()
                                .intersects(ContainerType::SIZE | ContainerType::INLINE_SIZE)
                        })
                })
            });
            let needs_layout = {
                let queried = self.nodes.container_query_sizes.lock().unwrap();
                !queried.is_empty()
                    && (geometry_dirty || queried.values().any(|entry| !entry.axes.width && !entry.axes.height))
            };
            // New registrations need their first content-box sample. Known
            // hidden containers have enabled axes with None sizes, so clean
            // reads of them must not keep requesting layout.
            if needs_layout {
                self.resolve(current_time_for_animations);
            }
        } else {
            self.nodes.container_query_sizes.lock().unwrap().clear();
        }
    }

    pub fn resolve(&mut self, current_time_for_animations: f64) {
        if TDocument::as_node(&self.root_node()).first_element_child().is_none() {
            #[cfg(feature = "tracing")]
            tracing::warn!("No DOM - not resolving");
            return;
        }

        // Process messages that have been sent to our message channel (e.g. loaded resource)
        self.handle_messages();

        // While render-blocking resources (e.g. stylesheets linked from the `<head>`) are
        // still loading, don't resolve styles or layout (matching how browsers block
        // rendering). Resolving styles before the document's stylesheets have loaded would
        // give elements computed styles based on an incomplete cascade, and a later restyle
        // (once the stylesheet loads) would treat those as genuine "before-change styles",
        // spuriously starting CSS transitions from unstyled values. See issue #689.
        //
        // `handle_messages` above must still run so that loaded resources are ingested and
        // this state can clear.
        if self.has_pending_critical_resources() {
            return;
        }

        phase_profile::resolved();
        self.resolve_scroll_animation();

        // Drop scrollbar-activity entries whose fade-out has finished (also
        // sheds entries for removed nodes).
        {
            use crate::node::scrollbar::{FADE_DELAY, FADE_DURATION};
            self.scrollbar_activity.retain(|_, last| last.elapsed() < FADE_DELAY + FADE_DURATION);
        }

        let root_node_id = self.root_element().id;
        debug_timer!(timer, feature = "log-phase-times");
        let mut phase_t = std::time::Instant::now();

        // Apply any device changes (viewport resize, zoom, color-scheme, etc)
        // accumulated since the last resolve as a single device rebuild.
        self.flush_pending_device_changes();

        // Container conditions consume content-box sizes from layout. Re-match
        // only when an actually queried size changes; bound cyclic feedback.
        for pass in 0..32 {
            // we need to resolve stylist first since it will need to drive our layout bits
            self.resolve_stylist(current_time_for_animations);
            timer.record_time("style");
            if !self.stylist.has_container_queries() {
                self.nodes.container_query_sizes.lock().unwrap().clear();
            }
            phase_profile::mark(0, &mut phase_t);

            // Non-incremental mode is "every node is damaged every frame": mark the
            // whole tree and let the same damage pipeline reconstruct and relayout
            // everything.
            if !self.incremental_layout {
                self.mark_all_damaged();
                timer.record_time("mark_all");
            }

            // Propagate damage flags (from mutation and restyles) up and down the tree
            self.propagate_damage_flags(root_node_id);
            timer.record_time("damage");
            phase_profile::mark(1, &mut phase_t);

            // Fix up tree for layout (insert anonymous blocks as necessary, etc)
            self.resolve_layout_children();
            #[cfg(debug_assertions)]
            self.assert_layout_parents_consistent();
            timer.record_time("construct");
            phase_profile::mark(2, &mut phase_t);

            self.resolve_deferred_tasks();
            // Flush background/mask images from style to dedicated storage on the
            // nodes whose style changed (queued by the style traversal and by
            // pseudo-element box construction), fetching any not-yet-loaded images.
            self.flush_pending_style_images();
            timer.record_time("pconstruct");
            phase_profile::mark(3, &mut phase_t);

            // Next we resolve layout with the data resolved by stlist
            self.resolve_layout();
            timer.record_time("layout");
            phase_profile::mark(4, &mut phase_t);
            if !self.refresh_container_query_sizes() || pass == 31 {
                break;
            }
            self.nodes[root_node_id]
                .set_restyle_hint(style::invalidation::element::restyle_hints::RestyleHint::restyle_subtree());
            for (_, node) in self.nodes.iter_mut() {
                node.invalidate_layout_cache();
            }
            self.mark_all_damaged();
        }
        self.nodes.bump_geometry_generation();

        // Resolve transforms
        self.resolve_transforms(root_node_id);
        timer.record_time("transform");
        phase_profile::mark(5, &mut phase_t);

        // Build paint children / stacking contexts for damaged subtrees. Runs
        // after layout and transforms so that stacking-context roots created by
        // this frame's transforms are picked up, and before damage is cleared.
        self.build_paint_tree(root_node_id);
        timer.record_time("paint_tree");
        phase_profile::mark(6, &mut phase_t);

        // Clear all damage and dirty flags, walking only subtrees which are
        // marked as (potentially) containing damage.
        let doc_node_id = self.root_node().id;
        self.clear_damage_and_dirty_flags(doc_node_id);
        timer.record_time("c_damage");
        phase_profile::mark(7, &mut phase_t);

        // Re-resolve the hover node from the pointer position against the fresh
        // layout. This must run *after* the damage/dirty flags are cleared
        // above, so that the restyle hint and ancestor `dirty_descendants`
        // flags set by any resulting hover change survive into the next resolve
        // pass (the clearing loop would otherwise wipe them). Any resulting
        // restyle is picked up on the next resolve pass; a redraw is requested
        // if the hovered node actually changes.
        self.refresh_hover();

        let mut subdoc_is_animating = false;
        for &node_id in &self.sub_document_nodes {
            let node = &mut self.nodes[node_id];
            let size = node.final_layout().size;
            if let Some(mut sub_doc) = node.subdoc_mut().map(|doc| doc.inner_mut()) {
                // Set viewport
                // viewport_mut handles change detection. So we just unconditionally set the values;
                let mut sub_viewport = sub_doc.viewport_mut();
                sub_viewport.hidpi_scale = self.viewport.hidpi_scale;
                sub_viewport.zoom = self.viewport.zoom;
                sub_viewport.color_scheme = self.viewport.color_scheme;

                let viewport_scale = self.viewport.scale();
                sub_viewport.window_size =
                    ((size.width * viewport_scale) as u32, (size.height * viewport_scale) as u32);
                drop(sub_viewport);

                sub_doc.resolve(current_time_for_animations);

                subdoc_is_animating |= sub_doc.is_animating();
            }
        }
        self.subdoc_is_animating = subdoc_is_animating;
        timer.record_time("subdocs");
        phase_profile::mark(8, &mut phase_t);

        timer.print_times(&format!("Resolve({}): ", self.id()));
    }

    fn refresh_container_query_sizes(&self) -> bool {
        use style::values::specified::ContainerType;
        let mut changed = false;
        let mut queried = self.nodes.container_query_sizes.lock().unwrap();
        queried.retain(|id, entry| {
            let Some(node) = self.nodes.get(*id) else { return false };
            if !node.flags.is_in_document() {
                return false;
            }
            let Some(values) = node.primary_styles() else { return false };
            let kind = values.get_box().slow_clone_container_type();
            let both = kind.intersects(ContainerType::SIZE);
            let inline = kind.intersects(ContainerType::INLINE_SIZE);
            if !both && !inline {
                changed |= entry.size.width.is_some() || entry.size.height.is_some();
                return false;
            }
            entry.axes = euclid::default::Size2D::new(
                both || (inline && values.writing_mode.is_horizontal()),
                both || (inline && !values.writing_mode.is_horizontal()),
            );
            let display = values.clone_display();
            let mut rendered = node.flags.is_in_document()
                && node.element_data().is_some_and(|element| element.layout_data.is_some())
                && !display.is_none()
                && !display.is_contents()
                && !display.is_inline_flow();
            let mut ancestor = node.parent;
            while rendered {
                let Some(id) = ancestor else { break };
                let Some(parent) = self.nodes.get(id) else { break };
                if parent.primary_styles().is_some_and(|style| style.clone_display().is_none()) {
                    rendered = false;
                    break;
                }
                ancestor = parent.parent;
            }
            let size = if rendered {
                let layout = node.final_layout();
                euclid::default::Size2D::new(
                    entry.axes.width.then(|| app_units::Au::from_f32_px(layout.content_box_width())),
                    entry.axes.height.then(|| app_units::Au::from_f32_px(layout.content_box_height())),
                )
            } else {
                Default::default()
            };
            if entry.size != size {
                entry.size = size;
                changed = true;
            }
            true
        });
        changed
    }

    fn resolve_transforms(&mut self, node_id: NodeId) -> Rect {
        if !self.nodes.contains_key(node_id) {
            return Rect::ZERO;
        }

        let scale = self.viewport.scale_f64();
        // CSS scroll extents must use the same unrounded boxes as client sizes.
        // Pixel-rounded paint widths can otherwise make fitting text overflow by 1px.
        // Only vertical placement needs the containing-block physicalisation walk.
        let node = &self.nodes[node_id];
        let vertical = node.layout_data().writing_mode.is_vertical()
            || node
                .containing_block()
                .is_some_and(|parent| self.nodes[parent].layout_data().writing_mode.is_vertical());
        let layout = if vertical { self.physical_unrounded_geometry(node_id).1 } else { *node.unrounded_layout() };

        // The cached overflow holds only while nothing in the subtree changed:
        // a descendant's relayout (its overflow shrinking) reaches its
        // ancestors as their damage, not as `RECALCULATE_OVERFLOW`.
        let unchanged = {
            let node = &self.nodes[node_id];
            node.damage().is_some_and(|d| d.is_empty()) && !node.has_damaged_descendants() && !node.is_anonymous()
        };
        if unchanged {
            let node = &self.nodes[node_id];
            let location = layout.location.map(|v| v as f64 * scale);

            let mut transform = Affine::translate((location.x, location.y));
            if let Some(t) = node.transform().as_deref() {
                transform *= *t
            }

            let overflow = self.overflow_contribution(node_id, *node.scrollable_overflow(), layout.size, scale);
            return transform.transform_rect_bbox(overflow);
        }

        let transform = self.nodes[node_id].set_transform(scale as f32);

        let w = layout.size.width as f64 * scale;
        let h = layout.size.height as f64 * scale;
        // lightningpanda: the scrollable overflow starts at the padding box
        // (CSS Overflow 3 2.2), not the border box.
        let b = layout.border;
        let mut overflow = Rect::new(
            b.left as f64 * scale,
            b.top as f64 * scale,
            (w - b.right as f64 * scale).max(b.left as f64 * scale),
            (h - b.bottom as f64 * scale).max(b.top as f64 * scale),
        );

        let layout_children = std::mem::take(self.nodes[node_id].layout_children.get_mut());

        // lightningpanda: a table's cells, transformed again only where its grid placed them or
        // they changed since the last pass (`TableContext::placed`); the others' overflow stands.
        if let Some(cells_overflow) = self.table_cells_overflow(node_id, scale) {
            if let Some(cells_overflow) = cells_overflow {
                overflow = overflow.union(cells_overflow);
            }
        } else if let Some(ref children) = layout_children {
            for &child_id in children {
                // Out-of-flow children are laid out relative to their containing
                // block, not their DOM parent: they are visited (and their overflow
                // accounted for) via the containing block's hoisted list below.
                if self.nodes[child_id].is_out_of_flow() {
                    continue;
                }
                let child_rect_in_self = self.resolve_transforms(child_id);
                overflow = unite_overflow(overflow, child_rect_in_self);
            }
        }
        let hoisted_children = std::mem::take(&mut *self.nodes[node_id].hoisted_children.borrow_mut());
        let viewport_contains_fixed = node_id == self.root_element().id;
        for &child_id in &hoisted_children {
            if !self.nodes.contains_key(child_id) {
                continue;
            }
            let child_rect_in_self = self.resolve_transforms(child_id);
            // lightningpanda: a box fixed to the viewport is not in the
            // document's scrollable overflow, whatever it reaches.
            let fixed = self.nodes[child_id]
                .primary_styles()
                .is_some_and(|s| s.clone_position() == style::computed_values::position::T::Fixed);
            if viewport_contains_fixed && fixed {
                continue;
            }
            overflow = unite_overflow(overflow, child_rect_in_self);
        }
        *self.nodes[node_id].hoisted_children.borrow_mut() = hoisted_children;
        for pseudo in [self.nodes[node_id].before(), self.nodes[node_id].after()] {
            let Some(pseudo) = pseudo else { continue };
            if self.nodes[pseudo].is_out_of_flow() {
                continue;
            }
            let child_rect_in_self = self.resolve_transforms(pseudo);
            overflow = unite_overflow(overflow, child_rect_in_self);
        }

        // lightningpanda: the lines of an inline formatting context are
        // overflow too (a nowrap name in a 100px `truncate` cell scrolls), and
        // a scroll container adds its end padding after them.
        let element = self.nodes[node_id].element_data();
        let editor_layout = element.and_then(|e| e.text_input_data()).and_then(|i| i.editor.try_layout());
        let inline = element.and_then(|e| e.inline_layout_data.as_deref());
        let lines = inline.map(|t| (&t.layout, t.block_offset)).or(editor_layout.map(|l| (l, 0.0)));
        if let Some((text_layout, block_offset)) = lines {
            let l = &layout;
            let width = text_layout
                .lines()
                .map(|line| {
                    let m = line.metrics();
                    (m.offset + m.advance - m.hanging_advance) as f64
                })
                .fold(0.0, f64::max);
            // The empty line parley adds after a final hard break (`<br>`)
            // is not content.
            let height = text_layout
                .lines()
                .filter(|line| !line.text_range().is_empty())
                .map(|line| line.metrics().block_max_coord as f64)
                .fold(0.0, f64::max);
            if width > 0.0 || height > 0.0 {
                let x0 = (l.border.left + l.padding.left) as f64 * scale;
                let y0 = (l.border.top + l.padding.top + block_offset) as f64 * scale;
                let style = self.nodes[node_id].primary_styles();
                // Text controls scroll their value whatever their `overflow`.
                let scrolls = editor_layout.is_some()
                    || style.as_ref().is_some_and(|s| {
                        s.clone_overflow_x() != style::values::computed::Overflow::Visible
                            || s.clone_overflow_y() != style::values::computed::Overflow::Visible
                    });
                let (end_x, end_y) = if scrolls {
                    (l.padding.right as f64 * scale, l.padding.bottom as f64 * scale)
                } else {
                    (0.0, 0.0)
                };
                // The lines of a vertical writing mode are logical: `width` runs along y, `height` along x.
                let lines = match self.nodes[node_id].vertical_block_axis() {
                    None => Rect::new(x0, y0, x0 + width + end_x, y0 + height + end_y),
                    Some(block_lr) => {
                        let offset = block_offset as f64 * scale;
                        let top = y0 - offset;
                        let (b0, b1) = if block_lr {
                            (x0 + offset, x0 + offset + height + end_x)
                        } else {
                            let start = (l.size.width - l.border.right - l.padding.right) as f64 * scale - offset;
                            (start - height - end_x, start)
                        };
                        Rect::new(b0, top, b1, top + width + end_y)
                    }
                };
                overflow = overflow.union(lines);
            }
        }

        *self.nodes[node_id].scrollable_overflow_mut() = overflow;
        *self.nodes[node_id].layout_children.get_mut() = layout_children;

        let scaled_x = layout.location.x as f64 * scale;
        let scaled_y = layout.location.y as f64 * scale;

        let full = if let Some(t) = transform {
            Affine::translate((scaled_x, scaled_y)) * t
        } else {
            Affine::translate((scaled_x, scaled_y))
        };

        let contribution = self.overflow_contribution(node_id, overflow, layout.size, scale);
        full.transform_rect_bbox(contribution)
    }

    /// lightningpanda: what a box adds to its parent's scrollable overflow
    /// (its own coordinates): its border box, plus its own scrollable overflow
    /// on the axes it does not clip. A scroll container keeps its content's
    /// overflow to itself.
    fn overflow_contribution(&self, node_id: NodeId, overflow: Rect, size: taffy::Size<f32>, scale: f64) -> Rect {
        use style::values::computed::Overflow;
        let node = &self.nodes[node_id];
        let border_box = Rect::new(0.0, 0.0, size.width as f64 * scale, size.height as f64 * scale);
        let mut out = overflow.union(border_box);
        if let Some(style) = node.primary_styles() {
            let (x, y) = (style.clone_overflow_x(), style.clone_overflow_y());
            if (x != Overflow::Visible || y != Overflow::Visible) && !self.body_overflow_goes_to_viewport(node_id) {
                if x != Overflow::Visible {
                    (out.x0, out.x1) = (border_box.x0, border_box.x1);
                }
                if y != Overflow::Visible {
                    (out.y0, out.y1) = (border_box.y0, border_box.y1);
                }
            }
        }
        out
    }

    /// lightningpanda: whether `node_id` is a `body` whose `overflow` applies
    /// to the viewport instead of to it (CSS Overflow 3 3.3): the root's is
    /// `visible`, and neither has containment.
    pub fn body_overflow_goes_to_viewport(&self, node_id: NodeId) -> bool {
        use style::values::computed::Overflow;
        use style::values::specified::box_::Contain;
        let node = &self.nodes[node_id];
        if !node.data.is_element_with_tag_name(&crate::local_name!("body")) {
            return false;
        }
        let root = self.root_element();
        if node.parent != Some(root.id) {
            return false;
        }
        let (Some(body), Some(html)) = (node.primary_styles(), root.primary_styles()) else {
            return false;
        };
        html.clone_overflow_x() == Overflow::Visible
            && html.clone_overflow_y() == Overflow::Visible
            && body.clone_contain() == Contain::empty()
            && html.clone_contain() == Contain::empty()
    }

    /// Ensure that the layout_children field is populated for all nodes
    pub fn resolve_layout_children(&mut self) {
        // lightningpanda: the boxes that left a container's list this pass and that no container
        // collected again are out of the layout tree, with what their subtrees built
        // (`BaseDocument::leave_layout_tree`).
        resolve_layout_children_recursive(self, self.root_node().id);
        for node_id in std::mem::take(&mut self.left_layout_lists) {
            if self.nodes.get(node_id).is_some_and(|node| node.layout_child_of.get().is_none()) {
                self.leave_layout_tree(node_id);
            }
        }

        fn resolve_layout_children_recursive(doc: &mut BaseDocument, node_id: NodeId) {
            // Anonymous blocks and pseudo-elements can be removed from the slab
            // between render passes. Bail out rather than panicking on a stale key.
            if doc.nodes.get(node_id).is_none() {
                return;
            }

            let mut damage = doc.nodes[node_id].damage().unwrap_or(ALL_DAMAGE);
            let _flags = doc.nodes[node_id].flags;

            // lightningpanda: a table whose rows or cells changed (their damage reaches it) updates
            // the rows that did and keeps its layout children; what it can't update is built again.
            let table_changed =
                damage.intersects(CONSTRUCT_FC | CONSTRUCT_BOX) || damage.contains(style::selector_parser::RestyleDamage::RELAYOUT);
            let updated = if table_changed && crate::layout::table::is_incremental(doc, node_id) {
                let updated = crate::layout::table::update_table_context(doc, node_id);
                if updated.is_none() {
                    damage.insert(CONSTRUCT_BOX);
                }
                updated
            } else {
                None
            };
            if let Some(changed_cells) = updated {
                let layout_children = doc.nodes[node_id].layout_children.borrow_mut().take().unwrap_or_default();
                // The cells that changed and the pseudo-elements, or all of them.
                let pseudos = [doc.nodes[node_id].before(), doc.nodes[node_id].after()];
                let visit: Vec<NodeId> = match changed_cells {
                    Some(cells) => cells.into_iter().chain(pseudos.into_iter().flatten()).collect(),
                    None => layout_children.to_vec(),
                };
                for child_id in visit {
                    let child = &doc.nodes[child_id];
                    if !child.has_damaged_descendants()
                        && !child.is_anonymous()
                        && child.layout_parent.get() == Some(node_id)
                    {
                        continue;
                    }
                    resolve_layout_children_recursive(doc, child_id);
                    doc.nodes[child_id].layout_parent.set(Some(node_id));
                    if let Some(mut data) = doc.nodes[child_id].try_stylo_element_data_mut().and_then(|s| s.get_mut()) {
                        data.damage.remove(CONSTRUCT_DESCENDENT | CONSTRUCT_FC | CONSTRUCT_BOX);
                    }
                }
                *doc.nodes[node_id].layout_children.borrow_mut() = Some(layout_children);
                damage.remove(CONSTRUCT_DESCENDENT | CONSTRUCT_FC | CONSTRUCT_BOX);
                doc.nodes[node_id].set_damage(damage);
                return;
            }

            if damage.intersects(CONSTRUCT_FC | CONSTRUCT_BOX) {
                //} || flags.contains(NodeFlags::IS_INLINE_ROOT) {

                // Deallocate the anonymous blocks created for this node in the
                // previous construction round. They live only in the slab, so
                // reconstructing without freeing them would leak a slab entry per
                // anonymous block per reconstruction.
                let old_anonymous_blocks = std::mem::take(&mut doc.nodes[node_id].anonymous_blocks);
                for anon_id in old_anonymous_blocks {
                    doc.deallocate_anonymous_block(anon_id);
                }

                let mut collected = LayoutChildren::default();
                phase_profile::collected();
                collect_layout_children(doc, node_id, &mut collected);
                let layout_children = collected.children;

                // lightningpanda: the previous layout children leave the list
                // (`Node::layout_child_of`); the new ones are entered below. A
                // child already collected by another container this pass points
                // there and is left alone. (Left after collecting: while it runs,
                // a child that is still listed tells one that enters the layout
                // tree again, see `table::enter_layout_tree`.)
                if let Some(old) = doc.nodes[node_id].layout_children.borrow().as_ref() {
                    for &child_id in old.iter() {
                        if let Some(child) = doc.nodes.get(child_id) {
                            if child.layout_child_of.get() == Some(node_id) {
                                child.layout_child_of.set(None);
                                doc.left_layout_lists.push(child_id);
                            }
                        }
                    }
                }
                doc.nodes[node_id].anonymous_blocks = collected.anonymous_blocks;

                // Recurse into newly collected layout children
                for child_id in layout_children.iter().copied() {
                    resolve_layout_children_recursive(doc, child_id);
                    doc.nodes[child_id].layout_parent.set(Some(node_id));
                    doc.nodes[child_id].layout_child_of.set(Some(node_id));
                    if let Some(mut data) = doc.nodes[child_id].try_stylo_element_data_mut().and_then(|s| s.get_mut()) {
                        data.damage.remove(CONSTRUCT_DESCENDENT | CONSTRUCT_FC | CONSTRUCT_BOX);
                    }
                }

                *doc.nodes[node_id].layout_children.borrow_mut() = Some(layout_children.clone());
                // *doc.nodes[node_id].paint_children.borrow_mut() = Some(layout_children);

                damage.remove(CONSTRUCT_DESCENDENT | CONSTRUCT_FC | CONSTRUCT_BOX);
                // damage.insert(RestyleDamage::RELAYOUT | RestyleDamage::REPAINT);
            } else {
                // A subtree with no damage has nothing to reconstruct: its
                // layout children and their `layout_parent` links are those
                // of the last pass. (The same skip as `propagate_damage_flags`;
                // anonymous boxes are always visited, as they are there.)
                {
                    let node = &doc.nodes[node_id];
                    if damage.is_empty() && !node.has_damaged_descendants() && !node.is_anonymous() {
                        return;
                    }
                }
                let layout_children = doc.nodes[node_id].layout_children.borrow_mut().take();
                if let Some(layout_children) = layout_children {
                    for child_id in layout_children.iter().copied() {
                        // Anonymous blocks and pseudo-elements can be removed from the
                        // slab between render passes; skip stale IDs.
                        if !doc.nodes.contains_key(child_id) {
                            continue;
                        }
                        resolve_layout_children_recursive(doc, child_id);
                        doc.nodes[child_id].layout_parent.set(Some(node_id));
                    }

                    *doc.nodes[node_id].layout_children.borrow_mut() = Some(layout_children);
                }

                // damage.remove(CONSTRUCT_DESCENDENT);
                // damage.insert(RestyleDamage::RELAYOUT | RestyleDamage::REPAINT);
            }

            doc.nodes[node_id].set_damage(damage);
        }
    }

    /// Every layout child must point back at its container via
    /// `layout_parent`, which `propagate_damage_flags` relies on to reach
    /// anonymous boxes.
    #[cfg(debug_assertions)]
    fn assert_layout_parents_consistent(&self) {
        use crate::node::NodeFlags;

        for (parent_id, node) in self.nodes.iter() {
            if !node.flags.contains(NodeFlags::IS_IN_DOCUMENT) {
                continue;
            }
            let Some(children) = node.layout_children.borrow().clone() else {
                continue;
            };
            for child_id in children {
                let Some(child) = self.nodes.get(child_id) else {
                    panic!("layout child {child_id:?} of {parent_id:?} is not in the slab");
                };
                debug_assert_eq!(
                    child.layout_parent.get(),
                    Some(parent_id),
                    "layout_parent of {child_id:?} does not point at its layout container {parent_id:?}"
                );
            }
        }
    }

    pub fn resolve_deferred_tasks(&mut self) {
        let mut deferred_construction_nodes = std::mem::take(&mut self.deferred_construction_nodes);

        // Deduplicate deferred tasks by node_id to avoid redundant work
        deferred_construction_nodes.sort_unstable_by_key(|task| task.node_id);
        deferred_construction_nodes.dedup_by_key(|task| task.node_id);

        #[cfg(feature = "parallel-construct")]
        let iter = deferred_construction_nodes.into_par_iter();
        #[cfg(not(feature = "parallel-construct"))]
        let iter = deferred_construction_nodes.into_iter();

        let results: Vec<ConstructionTaskResult> = iter
            .map(|task: ConstructionTask| match task.data {
                ConstructionTaskData::InlineLayout(mut layout) => {
                    #[cfg(feature = "parallel-construct")]
                    let mut layout_ctx = LAYOUT_CTX.take().unwrap_or_else(|| Box::new(LayoutContext::new()));
                    #[cfg(feature = "parallel-construct")]
                    let layout_ctx_mut = &mut layout_ctx;

                    #[cfg(feature = "parallel-construct")]
                    let mut font_ctx = self
                        .thread_font_contexts
                        .get_or(|| RefCell::new(Box::new(self.font_ctx.lock().unwrap().clone())))
                        .borrow_mut();
                    #[cfg(feature = "parallel-construct")]
                    let font_ctx_mut = &mut *font_ctx;

                    #[cfg(not(feature = "parallel-construct"))]
                    let layout_ctx_mut = &mut self.layout_ctx;
                    #[cfg(not(feature = "parallel-construct"))]
                    let font_ctx_mut = &mut *self.font_ctx.lock().unwrap();

                    layout.content_widths = None;
                    layout.last_break = None;
                    build_inline_layout_into(
                        &self.nodes,
                        layout_ctx_mut,
                        font_ctx_mut,
                        &mut layout,
                        self.viewport.scale(),
                        task.node_id,
                    );
                    phase_profile::rebuilt(layout.text.len());

                    #[cfg(feature = "parallel-construct")]
                    {
                        LAYOUT_CTX.set(Some(layout_ctx));
                    }

                    // If layout doesn't contain any inline boxes, then it is safe to populate the content_widths
                    // cache during this parallelized stage.
                    // if layout.layout.inline_boxes().is_empty() {
                    //     layout.content_widths();
                    // }

                    ConstructionTaskResult {
                        node_id: task.node_id,
                        data: ConstructionTaskResultData::InlineLayout(layout),
                    }
                }
            })
            .collect();

        for result in results {
            match result.data {
                ConstructionTaskResultData::InlineLayout(layout) => {
                    self.nodes[result.node_id].clear_layout_cache();
                    self.nodes[result.node_id].element_data_mut().unwrap().inline_layout_data = Some(layout);
                }
            }
        }

        self.deferred_construction_nodes.clear();
    }

    /// Walk the nodes now that they're properly styled and transfer their styles to the taffy style system
    ///
    /// TODO: update taffy to use an associated type instead of slab key
    /// TODO: update taffy to support traited styles so we don't even need to rely on taffy for storage
    pub fn resolve_layout(&mut self) {
        let size = self.stylist.device().au_viewport_size();

        let initial_containing_block = taffy::Size {
            width: AvailableSpace::Definite(size.width.to_f32_px()),
            height: AvailableSpace::Definite(size.height.to_f32_px()),
        };
        let available_space = match self.layout_available {
            Some((width, height)) => {
                taffy::Size { width: AvailableSpace::Definite(width), height: AvailableSpace::Definite(height) }
            }
            None => initial_containing_block,
        };

        // lightningpanda: same viewport and nothing damaged, so the last pass's layout stands.
        let root = self.root_element();
        if self.last_layout_space == Some(available_space)
            && !root.has_damaged_descendants()
            && root.damage().is_some_and(|damage| damage.is_empty())
        {
            return;
        }
        self.last_layout_space = Some(available_space);
        phase_profile::laid_out();

        let root_element_id = crate::taffy_node_id(self.root_element().id);

        // println!("\n\nRESOLVE LAYOUT\n===========\n");

        let mut state = LayoutPassState::new(self);
        #[cfg(feature = "writing-mode")]
        let (available_space, initial_containing_block) = {
            state.layout_wm = state.layout_wm_of(crate::dom_node_id(root_element_id));
            if state.layout_wm.is_vertical() {
                (available_space.transpose(), initial_containing_block.transpose())
            } else {
                (available_space, initial_containing_block)
            }
        };
        taffy::compute_root_layout_in(&mut state, root_element_id, available_space, initial_containing_block);
        #[cfg(feature = "writing-mode")]
        state.physicalise_and_round_layout(root_element_id);
        #[cfg(not(feature = "writing-mode"))]
        taffy::round_layout(&mut state, root_element_id);

        // println!("\n\n");
        // taffy::print_tree(self, root_node_id)
    }
}

/// lightningpanda: `overflow` with what a child adds to it. An empty rect (a
/// box of no width or no height, with nothing reaching out of it) adds nothing,
/// wherever it is: a 2000 x 0 px box does not make a page scroll.
impl BaseDocument {
    /// The union of the overflow of `table`'s cells in it (see `resolve_transforms`; `None` inside:
    /// none has any), or `None` if `table` is not a table whose cells it keeps (its layout children
    /// are visited as any others').
    fn table_cells_overflow(&mut self, table: NodeId, scale: f64) -> Option<Option<Rect>> {
        use crate::node::SpecialElementData;
        let node = &self.nodes[table];
        if !node.flags.is_table_root() {
            return None;
        }
        let SpecialElementData::TableRoot(ctx) = &node.data.downcast_element()?.special_data else { return None };
        if !crate::layout::table::keeps_cells(ctx) {
            return None;
        }
        let ctx = std::sync::Arc::clone(ctx);
        let cells = ctx.cells.len();
        let mut placed = ctx.placed.borrow_mut();
        let again = placed.take_transform().filter(|_| placed.overflow.len() == cells && placed.overflow_scale == scale);
        let mut overflow = std::mem::take(&mut placed.overflow);
        drop(placed);
        match again {
            Some(again) => {
                for index in again {
                    overflow[index] = self.resolve_transforms(ctx.cells[index].node_id);
                }
            }
            None => {
                overflow.clear();
                for cell in &ctx.cells {
                    overflow.push(self.resolve_transforms(cell.node_id));
                }
            }
        }
        // `unite_overflow` in any order: a union of the cells that are not empty.
        let united = overflow
            .iter()
            .filter(|cell| cell.width() > 0.0 && cell.height() > 0.0)
            .fold(None, |united: Option<Rect>, &cell| Some(united.map_or(cell, |united| united.union(cell))));
        let mut placed = ctx.placed.borrow_mut();
        placed.overflow = overflow;
        placed.overflow_scale = scale;
        Some(united)
    }
}

fn unite_overflow(overflow: Rect, child: Rect) -> Rect {
    if child.width() > 0.0 && child.height() > 0.0 {
        overflow.union(child)
    } else {
        overflow
    }
}

/// lightningpanda: cumulative time per `resolve` phase (style, damage,
/// construct, pconstruct, layout, transform, paint_tree, c_damage, subdocs),
/// for profiling incremental relayout.
pub mod phase_profile {
    use std::cell::Cell;
    use std::time::Instant;
    std::thread_local! {
        static NS: Cell<[u64; 9]> = const { Cell::new([0; 9]) };
    }
    pub(crate) fn mark(i: usize, t: &mut Instant) {
        let now = Instant::now();
        let d = (now - *t).as_nanos() as u64;
        *t = now;
        NS.with(|ns| {
            let mut v = ns.get();
            v[i] += d;
            ns.set(v);
        });
    }
    std::thread_local! {
        static REBUILDS: Cell<(u64, u64)> = const { Cell::new((0, 0)) };
    }
    /// An inline layout of `bytes` text was built (shaped) again.
    pub(crate) fn rebuilt(bytes: usize) {
        REBUILDS.with(|r| {
            let (n, b) = r.get();
            r.set((n + 1, b + bytes as u64));
        });
    }
    /// Inline layouts built on this thread so far, and their text bytes.
    pub fn rebuilds() -> (u64, u64) {
        REBUILDS.with(|r| r.get())
    }
    std::thread_local! {
        static COLLECTED: Cell<u64> = const { Cell::new(0) };
    }
    /// A container's layout children were collected again.
    pub(crate) fn collected() {
        COLLECTED.with(|c| c.set(c.get() + 1));
    }
    /// Layout-children collections and table context builds (from scratch: `tables().0`) on
    /// this thread so far.
    pub fn constructions() -> (u64, u64) {
        (COLLECTED.with(|c| c.get()), tables().0)
    }
    std::thread_local! {
        static RESTYLED: Cell<u64> = const { Cell::new(0) };
    }
    /// A node was visited by the style traversal.
    pub(crate) fn restyled() {
        RESTYLED.with(|r| r.set(r.get() + 1));
    }
    /// Nodes the style traversal visited on this thread so far.
    pub fn restyles() -> u64 {
        RESTYLED.with(|r| r.get())
    }
    std::thread_local! {
        static STYLE_STATS: Cell<[u64; 4]> = const { Cell::new([0; 4]) };
    }
    /// Adds an element's Stylo statistics: styled, matched, shared (style
    /// sharing cache hit) and reused (rule node match).
    pub(crate) fn style_stats(delta: [u32; 4]) {
        STYLE_STATS.with(|s| {
            let mut v = s.get();
            for (a, d) in v.iter_mut().zip(delta) {
                *a += d as u64;
            }
            s.set(v);
        });
    }
    /// Elements styled, matched, shared and reused on this thread so far.
    pub fn style_stats_peek() -> [u64; 4] {
        STYLE_STATS.with(|s| s.get())
    }
    std::thread_local! {
        static PASSES: Cell<(u64, u64)> = const { Cell::new((0, 0)) };
    }
    pub(crate) fn resolved() {
        PASSES.with(|p| p.set((p.get().0 + 1, p.get().1)));
    }
    pub(crate) fn laid_out() {
        PASSES.with(|p| p.set((p.get().0, p.get().1 + 1)));
    }
    /// `resolve` calls on this thread so far, and how many of their layout passes ran Taffy.
    pub fn passes() -> (u64, u64) {
        PASSES.with(|p| p.get())
    }
    std::thread_local! {
        static ROUNDING: Cell<(u64, u64)> = const { Cell::new((0, 0)) };
    }
    pub(crate) fn rounded((visited, skipped): (u64, u64)) {
        ROUNDING.with(|r| r.set((r.get().0 + visited, r.get().1 + skipped)));
    }
    /// Layout nodes rounding reached on this thread so far, and subtrees it skipped (each
    /// reached, the nodes under it not).
    pub fn rounding() -> (u64, u64) {
        ROUNDING.with(|r| r.get())
    }
    std::thread_local! {
        static TABLES: Cell<(u64, u64)> = const { Cell::new((0, 0)) };
    }
    pub(crate) fn table_built() {
        TABLES.with(|t| t.set((t.get().0 + 1, t.get().1)));
    }
    pub(crate) fn table_cell_collected() {
        TABLES.with(|t| t.set((t.get().0, t.get().1 + 1)));
    }
    /// Tables whose structure was built from scratch on this thread so far, and table cells read
    /// from their styles (by those builds and by the updates of the rows that changed).
    pub fn tables() -> (u64, u64) {
        TABLES.with(|t| t.get())
    }
    std::thread_local! {
        static TABLE_GRIDS: Cell<(u64, u64)> = const { Cell::new((0, 0)) };
    }
    pub(crate) fn table_grid_laid_out(again: bool) {
        TABLE_GRIDS.with(|t| {
            let (whole, again_count) = t.get();
            t.set(if again { (whole, again_count + 1) } else { (whole + 1, again_count) });
        });
    }
    /// Table grids laid out (and kept for the next layout) on this thread so far: whole, and again
    /// from their last layout in the rows that changed.
    pub fn table_grids() -> (u64, u64) {
        TABLE_GRIDS.with(|t| t.get())
    }
    /// Milliseconds per phase on this thread so far.
    pub fn peek() -> [f64; 9] {
        NS.with(|ns| ns.get().map(|n| n as f64 / 1e6))
    }
}
