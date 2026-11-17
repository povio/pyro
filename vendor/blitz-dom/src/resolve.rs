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
    document::DocumentFlags,
    layout::{
        construct::{
            ConstructionTask, ConstructionTaskData, ConstructionTaskResult,
            ConstructionTaskResultData, LayoutChildren, collect_layout_children,
        },
        damage::{ALL_DAMAGE, CONSTRUCT_BOX, CONSTRUCT_DESCENDENT, CONSTRUCT_FC},
        LayoutPassState,
        inline::build_inline_layout_into,
    },
    node::{NodeFlags, TextBrush, TextLayout, TransformContext},
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
        // pyro: sampled animations of elements styled anew resolve against that style.
        if self.resolve_sampled_animations() {
            self.resolve_stylist(current_time_for_animations);
        }
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
        let mut phase_t = phase_profile::start();

        // Apply any device changes (viewport resize, zoom, color-scheme, etc)
        // accumulated since the last resolve as a single device rebuild.
        self.flush_pending_device_changes();

        // Container conditions consume content-box sizes from layout. Re-match
        // only when an actually queried size changes; bound cyclic feedback.
        for pass in 0..32 {
            // we need to resolve stylist first since it will need to drive our layout bits
            self.resolve_stylist(current_time_for_animations);
            // pyro: sampled animations of elements styled anew resolve against that style.
            if self.resolve_sampled_animations() {
                self.resolve_stylist(current_time_for_animations);
            }
            // pyro: the scroll marker and scroll button boxes (`layout::scroll_markers`), before
            // counters (markers read them) and damage propagation.
            self.resolve_scroll_markers();
            // pyro: counters and quotes in generated content, before damage propagates.
            self.resolve_generated_content();
            timer.record_time("style");
            if !self.stylist.has_container_queries() {
                // pyro: container units (`cqw`, ...) query container sizes too: keep those of a
                // document that has styles using them.
                use style::computed_value_flags::ComputedValueFlags;
                let mut sizes = self.nodes.container_query_sizes.lock().unwrap();
                if !sizes.is_empty()
                    && !self.nodes.iter().any(|(_, node)| {
                        node.primary_styles()
                            .is_some_and(|style| style.flags.contains(ComputedValueFlags::USES_CONTAINER_UNITS))
                    })
                {
                    sizes.clear();
                }
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
            let container_sizes_changed = self.refresh_container_query_sizes();
            // pyro: the active scroll markers and the scroll buttons' states follow the layout
            // and the scroll positions (`layout::scroll_markers`): a change styles them again.
            let scroll_states_changed = !container_sizes_changed && self.update_target_current();
            if (!container_sizes_changed && !scroll_states_changed) || pass == 31 {
                break;
            }
            if !container_sizes_changed {
                continue;
            }
            self.nodes[root_node_id]
                .set_restyle_hint(style::invalidation::element::restyle_hints::RestyleHint::restyle_subtree());
            for (_, node) in self.nodes.iter_mut() {
                node.clear_layout_cache();
            }
            self.mark_all_damaged();
        }
        self.nodes.bump_geometry_generation();

        // Resolve transforms
        let resolve_all = self.flags.contains(DocumentFlags::RESOLVE_ALL_TRANSFORMS);
        self.flags.remove(DocumentFlags::RESOLVE_ALL_TRANSFORMS);
        self.resolve_transforms(root_node_id, resolve_all, ContainingTransform::default());
        for fo in self.foreign_object_roots() {
            self.resolve_transforms(fo, resolve_all, ContainingTransform::default());
        }
        timer.record_time("transform");
        phase_profile::mark(5, &mut phase_t);

        // Paint children / stacking contexts are rebuilt lazily, by the first
        // paint or hit test after this resolve (`ensure_paint_tree`), for the
        // nodes `clear_damage_and_dirty_flags` flags `PAINT_TREE_DIRTY` below.
        // Forced layouts (`getBoundingClientRect` and the like) never read
        // them, so a run of them builds the tree once, not once per resolve.
        self.paint_tree_dirty = true;
        // pyro: so are the hit bounds (`ensure_hit_overflow`).
        self.hit_overflow_dirty = true;
        timer.record_time("paint_tree");
        phase_profile::mark(6, &mut phase_t);

        // Clear all damage and dirty flags, walking only subtrees which are
        // marked as (potentially) containing damage.
        let doc_node_id = self.root_node().id;
        self.clear_damage_and_dirty_flags(doc_node_id);
        #[cfg(debug_assertions)]
        self.assert_no_damage_left();
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
                && !display.is_inline_flow()
                // pyro: an element inside an `<svg>` (a `<foreignObject>`, which has a CSS box of
                // its own since it renders its content) is no size query container (CSS
                // Conditional 5; WPT css-conditional `svg-foreignobject-no-size-container`).
                && !(node.element_data().is_some_and(|el| el.name.ns == markup5ever::ns!(svg))
                    && node.parent.and_then(|id| self.nodes.get(id)).is_some_and(|parent| {
                        parent.element_data().is_some_and(|el| el.name.ns == markup5ever::ns!(svg))
                    }));
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

    /// The layout `resolve_transforms` and `resolve_hit_overflow` place
    /// `node_id` by: its unrounded box, physical when it or its containing
    /// block is in a vertical writing mode.
    #[inline]
    fn transform_pass_layout(&self, node_id: NodeId) -> taffy::Layout {
        // CSS scroll extents must use the same unrounded boxes as client sizes.
        // Pixel-rounded paint widths can otherwise make fitting text overflow by 1px.
        // Only vertical placement needs the containing-block physicalisation walk.
        let node = &self.nodes[node_id];
        let vertical = node.layout_data().writing_mode.is_vertical()
            || node
                .containing_block()
                .is_some_and(|parent| self.nodes[parent].layout_data().writing_mode.is_vertical());
        if vertical { self.physical_unrounded_geometry(node_id).1 } else { *node.unrounded_layout() }
    }

    /// The transforms and overflow of `node_id`'s subtree: what it adds to its
    /// parent's scrollable overflow, in the parent's coordinates.
    ///
    /// pyro: each box this pass computes again is flagged
    /// `LayoutData::hit_stale`; the next hit test brings their hit bounds up
    /// to date (`ensure_hit_overflow`), so a run of forced layouts with no
    /// hit test in between pays nothing for them.
    ///
    /// pyro: `containing`: what `node_id`'s containing block gives the transforms of the
    /// boxes it contains (its perspective).
    fn resolve_transforms(&mut self, node_id: NodeId, resolve_all: bool, containing: ContainingTransform) -> Rect {
        if !self.nodes.contains_key(node_id) {
            return Rect::ZERO;
        }

        let scale = self.viewport.scale_f64();
        let layout = self.transform_pass_layout(node_id);

        // The cached overflow holds only while nothing in the subtree changed:
        // a descendant's relayout (its overflow shrinking) reaches its
        // ancestors as their damage, not as `RECALCULATE_OVERFLOW`.
        //
        // pyro: a 3D transform under its parent's perspective or in its parent's 3D rendering
        // context depends on where the box sits in the parent, and on the parent: both may have
        // changed with no damage here (`containing.refresh`). A `preserve-3d` box then passes
        // the change on to its own children.
        let unchanged = !resolve_all && {
            let node = &self.nodes[node_id];
            node.damage().is_some_and(|d| d.is_empty())
                && !node.has_damaged_descendants()
                && !node.is_anonymous()
                && !(containing.refresh && node.is_preserve_3d())
        };
        if unchanged {
            if containing.refresh {
                self.nodes[node_id].set_transform(scale as f32, &containing.context);
            }
            let node = &self.nodes[node_id];
            let location = layout.location.map(|v| v as f64 * scale);
            let overflow = self.overflow_contribution(node_id, *node.scrollable_overflow(), layout.size, scale);
            return placement((location.x, location.y), node.transform().as_deref())(overflow);
        }

        let (transform, accumulated) = self.nodes[node_id].set_transform(scale as f32, &containing.context);
        self.nodes[node_id].layout_data_mut().hit_stale = true;
        // pyro: what this box gives the transforms of its children: they are resolved again
        // while it gives them a perspective or a 3D rendering context, and once more after.
        // pyro: an anonymous box stands for the box it is in: the boxes it lays out are that
        // box's children in the DOM (an anonymous table's cells, an anonymous block's inline-blocks;
        // WPT css-transforms `transform-table-011`).
        let mut owner = node_id;
        while self.nodes[owner].is_anonymous()
            && let Some(parent) = self.nodes[owner].parent
        {
            owner = parent;
        }
        let contents = {
            let node = &mut self.nodes[node_id];
            let context = node.transform_context_for_children(accumulated);
            let refresh = context.gives() || node.flags.contains(NodeFlags::GIVES_3D_CONTEXT);
            node.flags.set(NodeFlags::GIVES_3D_CONTEXT, context.gives());
            ContainingTransform { context, refresh, parent: Some(owner) }
        };

        // pyro: the scrollable overflow starts at the padding box
        // (CSS Overflow 3 2.2), not the border box.
        let mut overflow = padding_box(&layout, scale);
        // pyro: a multi-column container's content lies in its strip: what it reaches is shown in
        // the columns (`layout::multicol`), added to the padding box below.
        let columns_base =
            self.nodes[node_id].column_fragments().is_some().then(|| std::mem::replace(&mut overflow, Rect::ZERO));

        let layout_children = std::mem::take(self.nodes[node_id].layout_children.get_mut());
        // pyro: a scroll container's padding surrounds its in-flow content (CSS Overflow 3 §2.2;
        // Blink's `ScrollableOverflowCalculator::Result`; Taffy's
        // `compute_scrollable_overflow_contribution`, whose overflow the local background's area
        // uses): each in-flow child's border box grown by the padding and the child's margin, so
        // that scrolled to the end, the end padding is in view after the content. It was not
        // here: `scrollWidth` and `scrollHeight` were short by the end padding, and a
        // `content-box` local background reached the scrollport's edge at the end (WPT
        // `local-attachment-content-box-scroll`).
        // The root's and a propagated `body`'s overflow is the viewport's.
        let scroll_padding = (self.nodes[node_id].primary_styles().is_some_and(|s| {
            use style::values::computed::Overflow;
            let scrolls = |o: Overflow| !matches!(o, Overflow::Visible | Overflow::Clip);
            scrolls(s.clone_overflow_x()) || scrolls(s.clone_overflow_y())
        }) && node_id != self.root_element().id
            && !self.body_overflow_goes_to_viewport(node_id))
        .then_some(layout.padding);

        // pyro: a table's cells, transformed again only where its grid placed them or
        // they changed since the last pass (`TableContext::placed`); the others' overflow stands.
        if let Some(cells_overflow) = self.table_cells_overflow(node_id, scale, resolve_all, contents) {
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
                let child = contents.for_child(&self.nodes, &self.nodes[child_id]);
                let child_rect_in_self =
                    crate::util::ensure_stack(|| self.resolve_transforms(child_id, resolve_all, child));
                overflow = unite_overflow(overflow, child_rect_in_self);
                if let Some(padding) = scroll_padding {
                    overflow = unite_overflow(overflow, self.padded_inflow_box(child_id, padding, scale));
                }
            }
        }
        let hoisted_children = std::mem::take(&mut *self.nodes[node_id].hoisted_children.borrow_mut());
        let viewport_contains_fixed = node_id == self.root_element().id;
        for &child_id in &hoisted_children {
            if !self.nodes.contains_key(child_id) {
                continue;
            }
            let child = contents.for_child(&self.nodes, &self.nodes[child_id]);
            let child_rect_in_self =
                crate::util::ensure_stack(|| self.resolve_transforms(child_id, resolve_all, child));
            // pyro: a box fixed to the viewport is not in the
            // document's scrollable overflow, whatever it reaches.
            if viewport_contains_fixed && self.is_fixed(child_id) {
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
            let child = contents.for_child(&self.nodes, &self.nodes[pseudo]);
            let child_rect_in_self = crate::util::ensure_stack(|| self.resolve_transforms(pseudo, resolve_all, child));
            overflow = unite_overflow(overflow, child_rect_in_self);
            if let Some(padding) = scroll_padding {
                overflow = unite_overflow(overflow, self.padded_inflow_box(pseudo, padding, scale));
            }
        }

        // pyro: the lines of an inline formatting context are
        // overflow too (a nowrap name in a 100px `truncate` cell scrolls), and
        // a scroll container adds its end padding after them.
        if let Some((lines, true)) = self.line_bounds(node_id, &layout, scale) {
            overflow = overflow.union(lines);
        }
        if let Some(base) = columns_base {
            overflow = base.union(self.columns_rect(node_id, overflow, scale));
        }

        *self.nodes[node_id].scrollable_overflow_mut() = overflow;
        *self.nodes[node_id].layout_children.get_mut() = layout_children;

        let scaled_x = layout.location.x as f64 * scale;
        let scaled_y = layout.location.y as f64 * scale;

        let contribution = self.overflow_contribution(node_id, overflow, layout.size, scale);
        placement((scaled_x, scaled_y), transform.as_ref())(contribution)
    }

    /// pyro: an in-flow child's border box in its scroll container (device px), grown on each
    /// side by the container's `padding` and the child's margin there (not below zero), the
    /// child's part of the container's scrollable overflow with the end padding after it. Its
    /// transform is left out, as Blink and Taffy leave it out of the in-flow bounds.
    fn padded_inflow_box(&self, child_id: NodeId, padding: taffy::Rect<f32>, scale: f64) -> Rect {
        let child = self.transform_pass_layout(child_id);
        let pad = |padding: f32, margin: f32| (padding + margin).max(0.0) as f64;
        let (x, y) = (child.location.x as f64, child.location.y as f64);
        let (w, h) = (child.size.width as f64, child.size.height as f64);
        if !(w > 0.0 && h > 0.0) {
            return Rect::ZERO;
        }
        Rect::new(
            (x - pad(padding.left, child.margin.left)) * scale,
            (y - pad(padding.top, child.margin.top)) * scale,
            (x + w + pad(padding.right, child.margin.right)) * scale,
            (y + h + pad(padding.bottom, child.margin.bottom)) * scale,
        )
    }

    /// pyro: whether `node_id` is `position: fixed`.
    fn is_fixed(&self, node_id: NodeId) -> bool {
        self.nodes[node_id]
            .primary_styles()
            .is_some_and(|s| s.clone_position() == style::computed_values::position::T::Fixed)
    }

    /// pyro: the box of the lines of `node_id`'s inline formatting
    /// context or text control (its own coordinates, device px; a scroll
    /// container's end padding added after them), and whether they reach
    /// anywhere (a width or a height). `None`: it lays out no lines.
    fn line_bounds(&self, node_id: NodeId, l: &taffy::Layout, scale: f64) -> Option<(Rect, bool)> {
        let element = self.nodes[node_id].element_data();
        let editor_layout = element.and_then(|e| e.text_input_data()).and_then(|i| i.editor.try_layout());
        let inline = element.and_then(|e| e.inline_layout_data.as_deref());
        let (text_layout, block_offset) = inline.map(|t| (&t.layout, t.block_offset)).or(editor_layout.map(|l| (l, 0.0)))?;
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
                // pyro: `sideways-lr` lines run up from the content box's bottom, their end padding
                // (a scroll container's) the top one.
                if stylo_taffy::WritingModeExt::line_left_is_bottom(self.nodes[node_id].layout_data().writing_mode) {
                    let bottom = (l.size.height - l.border.bottom - l.padding.bottom) as f64 * scale;
                    let end_top = if scrolls { l.padding.top as f64 * scale } else { 0.0 };
                    return Some((Rect::new(b0, bottom - width - end_top, b1, bottom), width > 0.0 || height > 0.0));
                }
                Rect::new(b0, top, b1, top + width + end_y)
            }
        };
        Some((lines, width > 0.0 || height > 0.0))
    }

    /// pyro: brings every box's hit bound (`LayoutData::hit_overflow`)
    /// up to date with the last resolve: the boxes the transform passes since
    /// the last call computed again (`LayoutData::hit_stale`) and their tables'
    /// per-row unions (`TableContext::row_hit_extents`). Hit testing calls
    /// this first; a resolve only marks the bounds stale.
    pub fn ensure_hit_overflow(&mut self) {
        if std::mem::take(&mut self.hit_overflow_dirty)
            && let Some(root_id) = self.try_root_element().map(|root| root.id)
        {
            let _ = self.resolve_hit_overflow(root_id);
        }
        for &node_id in &self.sub_document_nodes {
            if let Some(mut sub_doc) = self
                .nodes
                .get_mut(node_id)
                .and_then(|node| node.subdoc_mut())
                .map(|doc| doc.inner_mut())
            {
                sub_doc.ensure_hit_overflow();
            }
        }
    }

    /// pyro: whether a resolve ran since the hit bounds were last
    /// brought up to date (`ensure_hit_overflow`).
    pub fn hit_overflow_is_stale(&self) -> bool {
        self.hit_overflow_dirty
    }

    /// pyro: the hit bound of `node_id`'s subtree (see
    /// `LayoutData::hit_overflow`), computed again for the boxes flagged
    /// `hit_stale`, and what it adds to its parent's, in the parent's
    /// coordinates. It walks the tree `resolve_transforms` walks: a box is
    /// flagged only when that pass computed it again, and so were its
    /// ancestors.
    fn resolve_hit_overflow(&mut self, node_id: NodeId) -> Rect {
        if !self.nodes.contains_key(node_id) {
            return Rect::ZERO;
        }
        let scale = self.viewport.scale_f64();
        let layout = self.transform_pass_layout(node_id);
        let location = layout.location.map(|v| v as f64 * scale);
        if !self.nodes[node_id].layout_data().hit_stale {
            let node = &self.nodes[node_id];
            let hit = self.overflow_contribution(node_id, *node.hit_overflow(), layout.size, scale);
            return node.hit_bound_in_parent((location.x, location.y), scale, hit);
        }

        let mut hit = padding_box(&layout, scale);
        // pyro: as for the scrollable overflow (`resolve_transforms`).
        let columns_base =
            self.nodes[node_id].column_fragments().is_some().then(|| std::mem::replace(&mut hit, Rect::ZERO));
        let layout_children = std::mem::take(self.nodes[node_id].layout_children.get_mut());
        if let Some(cells_hit) = self.table_cells_hit(node_id, scale) {
            if let Some(cells_hit) = cells_hit {
                hit = hit.union(cells_hit);
            }
        } else if let Some(ref children) = layout_children {
            for &child_id in children {
                if self.nodes[child_id].is_out_of_flow() {
                    continue;
                }
                let child_hit = crate::util::ensure_stack(|| self.resolve_hit_overflow(child_id));
                hit = unite_hit(hit, child_hit);
            }
        }
        *self.nodes[node_id].layout_children.get_mut() = layout_children;
        let hoisted_children = std::mem::take(&mut *self.nodes[node_id].hoisted_children.borrow_mut());
        let viewport_contains_fixed = node_id == self.root_element().id;
        for &child_id in &hoisted_children {
            if !self.nodes.contains_key(child_id) {
                continue;
            }
            let child_hit = crate::util::ensure_stack(|| self.resolve_hit_overflow(child_id));
            if viewport_contains_fixed && self.is_fixed(child_id) {
                continue;
            }
            hit = unite_hit(hit, child_hit);
        }
        *self.nodes[node_id].hoisted_children.borrow_mut() = hoisted_children;
        for pseudo in [self.nodes[node_id].before(), self.nodes[node_id].after()] {
            let Some(pseudo) = pseudo else { continue };
            if self.nodes[pseudo].is_out_of_flow() {
                continue;
            }
            let child_hit = crate::util::ensure_stack(|| self.resolve_hit_overflow(pseudo));
            hit = unite_hit(hit, child_hit);
        }

        // pyro: a glyph run's content area (whole-pixel ascent and
        // descent around its baseline, its advance rounded up to a layout
        // unit) can reach outside the line boxes: a tight `line-height`,
        // hanging white space. Layout queries report an inline element's box
        // from those areas, so the hit bound holds the line boxes grown by the
        // largest such reach, measured once after each line breaking
        // (`TextLayout::hit_reach`). Text controls lay out no inline elements.
        if let Some((lines, reaches)) = self.line_bounds(node_id, &layout, scale) {
            if reaches {
                hit = hit.union(lines);
            }
            let text = self.nodes[node_id].element_data_mut().and_then(|e| e.inline_layout_data.as_deref_mut());
            if let Some(text) = text {
                let reach = *text.hit_reach.get_or_insert_with(|| glyph_reach(&text.layout));
                let reach = reach + scale / 64.0;
                hit = hit.union(lines.inflate(reach, reach));
            }
        }
        if let Some(base) = columns_base {
            hit = base.union(self.columns_rect(node_id, hit, scale));
        }

        let data = self.nodes[node_id].layout_data_mut();
        data.hit_overflow = hit;
        data.hit_stale = false;
        let node = &self.nodes[node_id];
        let hit = self.overflow_contribution(node_id, hit, layout.size, scale);
        node.hit_bound_in_parent((location.x, location.y), scale, hit)
    }

    /// pyro: what a box adds to its parent's scrollable overflow
    /// (its own coordinates): its border box, plus its own scrollable overflow
    /// on the axes it does not clip. A scroll container keeps its content's
    /// overflow to itself. (The same for its hit bound.)
    pub fn overflow_contribution(&self, node_id: NodeId, overflow: Rect, size: taffy::Size<f32>, scale: f64) -> Rect {
        use style::values::computed::Overflow;
        let border_box = Rect::new(0.0, 0.0, size.width as f64 * scale, size.height as f64 * scale);
        let mut out = overflow.union(border_box);
        if let Some(style) = self.nodes[node_id].primary_styles() {
            let (x, y) = (style.clone_overflow_x() != Overflow::Visible, style.clone_overflow_y() != Overflow::Visible);
            if (x || y) && !self.body_overflow_goes_to_viewport(node_id) {
                if x {
                    (out.x0, out.x1) = (border_box.x0, border_box.x1);
                }
                if y {
                    (out.y0, out.y1) = (border_box.y0, border_box.y1);
                }
            }
        }
        out
    }

    /// pyro: whether `node_id` is a `body` whose `overflow` applies
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
        // pyro: only the root's first `body` child (CSS Overflow 3 § 3.3; a `frameset` before it
        // does not count, WPT css/CSS2/visufx/overflow-propagation-001c), and only while it
        // generates a box (WPT css-overflow/overflow-body-propagation-014..016: a second `body`
        // keeps its `overflow`).
        let first = root
            .children
            .iter()
            .copied()
            .find(|&id| self.nodes[id].data.is_element_with_tag_name(&crate::local_name!("body")));
        if first != Some(node_id) {
            return false;
        }
        let (Some(body), Some(html)) = (node.primary_styles(), root.primary_styles()) else {
            return false;
        };
        let display = body.clone_display();
        if display.is_none() || display.is_contents() {
            return false;
        }
        html.clone_overflow_x() == Overflow::Visible
            && html.clone_overflow_y() == Overflow::Visible
            && body.clone_contain() == Contain::empty()
            && html.clone_contain() == Contain::empty()
    }

    /// pyro: flags the `<body>` whose `overflow` the viewport takes (`NodeFlags::
    /// OVERFLOW_TO_VIEWPORT`), so that layout reads its `overflow` as `visible` (CSS Overflow 3
    /// §3.3): `body { overflow: hidden }` made it a scroll container, a block formatting context
    /// root whose children's margins no longer collapsed through it. A body whose flag changes
    /// (the root's `overflow` or either's containment changed) is laid out again.
    fn mark_viewport_body(&mut self) {
        let Some(root) = self.try_root_element() else {
            return;
        };
        let root_id = root.id;
        let bodies: Vec<NodeId> = root
            .children
            .iter()
            .copied()
            .filter(|&child| self.nodes[child].data.is_element_with_tag_name(&crate::local_name!("body")))
            .collect();
        for body in bodies {
            let goes = self.body_overflow_goes_to_viewport(body);
            let node = &mut self.nodes[body];
            if node.flags.contains(NodeFlags::OVERFLOW_TO_VIEWPORT) != goes {
                node.flags.set(NodeFlags::OVERFLOW_TO_VIEWPORT, goes);
                node.clear_layout_cache();
                self.nodes[root_id].clear_layout_cache();
            }
        }
    }

    /// Ensure that the layout_children field is populated for all nodes
    /// pyro: the `@font-feature-values` rules in effect, for the style conversions of the
    /// layout about to run on this thread (`font-variant-alternates`).
    fn publish_font_feature_values(&self) {
        let mut rules = Vec::new();
        for (data, _origin) in self.stylist.iter_extra_data_origins() {
            rules.extend(data.font_feature_values.iter().map(|(rule, _layer)| rule.clone()));
        }
        crate::stylo_to_parley::set_font_feature_values(rules);
    }

    /// pyro: `font-size-adjust` (CSS Fonts 5 §3.6): the used font size is the font size times
    /// the property's value over the first available font's ratio of the named metric to its
    /// size (`ex-height`, `cap-height`, `ch-width`, `ic-width`, `ic-height`), so that fallback
    /// fonts keep the metric's size. Computed here for every style that has the property, for
    /// the style conversions of the layout about to run (which cannot ask for font metrics).
    fn publish_font_size_adjustments(&self) {
        use style::values::computed::font::FontSizeAdjust;
        let provider = crate::font_metrics::BlitzFontMetricsProvider { font_ctx: self.font_ctx.clone() };
        let mut scales = std::collections::HashMap::new();
        for (_, node) in self.nodes.iter() {
            let Some(style) = node.primary_styles() else { continue };
            let font = style.get_font();
            if font.font_size_adjust == FontSizeAdjust::None {
                continue;
            }
            let key = font as *const style::properties::style_structs::Font as usize;
            if scales.contains_key(&key) {
                continue;
            }
            let size = font.font_size.used_size.0;
            scales.insert(key, provider.font_size_adjust_scale(font, size));
        }
        crate::stylo_to_parley::set_font_size_adjustments(scales);
    }

    pub fn resolve_layout_children(&mut self) {
        self.publish_font_feature_values();
        self.publish_font_size_adjustments();
        // pyro: the boxes that left a container's list this pass and that no container
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

            // pyro: a table whose rows or cells changed (their damage reaches it) updates
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
                    if doc.nodes[child_id].is_clean_layout_child_of(node_id) {
                        continue;
                    }
                    crate::util::ensure_stack(|| resolve_layout_children_recursive(doc, child_id));
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

                // pyro: the previous layout children leave the list
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
                    crate::util::ensure_stack(|| resolve_layout_children_recursive(doc, child_id));
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
                    // pyro: a table whose cells it keeps visits the cells noted as changed
                    // (`table::cells_to_visit`), not all of them.
                    let noted = crate::layout::table::cells_to_visit(doc, node_id);
                    for child_id in noted.as_deref().unwrap_or(&layout_children[..]).iter().copied() {
                        // Anonymous blocks and pseudo-elements can be removed from the
                        // slab between render passes; skip stale IDs.
                        let Some(child) = doc.nodes.get(child_id) else { continue };
                        // pyro: only the children on a damaged path are visited: a child
                        // whose subtree has no damage (and is still listed here) would return at
                        // the skip above without changing anything. A cell edit in a long table
                        // visits that cell, not its thousands of siblings.
                        if child.is_clean_layout_child_of(node_id) {
                            continue;
                        }
                        crate::util::ensure_stack(|| resolve_layout_children_recursive(doc, child_id));
                        doc.nodes[child_id].layout_parent.set(Some(node_id));
                    }

                    *doc.nodes[node_id].layout_children.borrow_mut() = Some(layout_children);
                }

                // damage.remove(CONSTRUCT_DESCENDENT);
                // damage.insert(RestyleDamage::RELAYOUT | RestyleDamage::REPAINT);
            }

            // pyro: an inline `<svg>`'s `<foreignObject>`s are roots of their own, built here
            // (`BaseDocument::svg_foreign_objects`).
            if let Some(foreign_objects) = doc.svg_foreign_objects.get(&node_id).cloned() {
                for fo in foreign_objects {
                    if !doc.nodes.get(fo).is_some_and(crate::layout::construct::is_foreign_object) {
                        continue;
                    }
                    crate::util::ensure_stack(|| resolve_layout_children_recursive(doc, fo));
                    doc.nodes[fo].layout_parent.set(Some(node_id));
                    if let Some(mut data) = doc.nodes[fo].try_stylo_element_data_mut().and_then(|s| s.get_mut()) {
                        data.damage.remove(CONSTRUCT_DESCENDENT | CONSTRUCT_FC | CONSTRUCT_BOX);
                    }
                }
            }

            doc.nodes[node_id].set_damage(damage);
        }
    }

    /// pyro: `clear_damage_and_dirty_flags` reached every node with damage: the walks
    /// that follow damage skip a subtree whose root's `damaged_descendants` flag is clear
    /// (`Node::has_no_damage_below`), so damage set without its flag would outlive the pass.
    #[cfg(debug_assertions)]
    fn assert_no_damage_left(&self) {
        use crate::node::NodeFlags;
        for (id, node) in self.nodes.iter() {
            if !node.flags.contains(NodeFlags::IS_IN_DOCUMENT) {
                continue;
            }
            debug_assert!(
                !node.damage().is_some_and(|damage| !damage.is_empty()),
                "{id:?} ({:?}, anonymous: {}) keeps damage {:?} after the pass",
                node.data.downcast_element().map(|el| &el.name.local),
                node.is_anonymous(),
                node.damage(),
            );
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

        // pyro: the first letter each root's first line has (`layout::first_letter`), found on
        // the tree construction left, with the nodes holding their styles made before the builds.
        for task in deferred_construction_nodes.iter_mut() {
            let ConstructionTaskData::InlineLayout(layout) = &mut task.data;
            layout.first_letter = match self.first_letter_styles {
                true => crate::layout::first_letter::split(self, task.node_id),
                false => None,
            };
            // pyro: an initial letter box is laid out by the root's first line: it is the first
            // of the root's layout children, with its own inline layout.
            if let Some(split) = layout.first_letter.as_ref().filter(|split| split.initial.is_some() || split.floated) {
                crate::layout::first_letter::build_initial_letter_box(self, task.node_id, split.style_node);
            }
            layout.first_line = match self.first_line_styles {
                true => crate::layout::first_line::style_node_for(self, task.node_id),
                false => None,
            };
        }

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
                    layout_ctx.set_line_break_override(self.layout_ctx.line_break_override());
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

                    // Only the parley layout carries over (see `build_inline_layout_into`).
                    *layout = TextLayout {
                        layout: std::mem::take(&mut layout.layout),
                        first_letter: layout.first_letter.take(),
                        first_line: layout.first_line.take(),
                        ..TextLayout::new()
                    };
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

    /// pyro: the `<foreignObject>`s of the inline `<svg>`s in the document
    /// ([`BaseDocument::svg_foreign_objects`]) that are roots now: in the document, under an
    /// `<svg>` that has a box.
    pub(crate) fn foreign_object_roots(&self) -> Vec<NodeId> {
        if self.svg_foreign_objects.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for (&svg, foreign_objects) in &self.svg_foreign_objects {
            let Some(svg_node) = self.nodes.get(svg) else { continue };
            if !svg_node.flags.is_in_document() || svg_node.layout_parent.get().is_none() {
                continue;
            }
            for &fo in foreign_objects {
                if self.nodes.get(fo).is_some_and(|node| {
                    node.flags.is_in_document()
                        && crate::layout::construct::is_foreign_object(node)
                        && node.layout_parent.get() == Some(svg)
                }) {
                    out.push(fo);
                }
            }
        }
        out.sort_unstable_by_key(|id| id.as_u64());
        out
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

        // pyro: same viewport and nothing damaged, so the last pass's layout stands.
        let root = self.root_element();
        if self.last_layout_space == Some(available_space)
            && !root.has_damaged_descendants()
            && root.damage().is_some_and(|damage| damage.is_empty())
        {
            return;
        }
        self.last_layout_space = Some(available_space);
        phase_profile::laid_out();
        self.mark_viewport_body();

        let root_element_id = crate::taffy_node_id(self.root_element().id);

        // println!("\n\nRESOLVE LAYOUT\n===========\n");

        let foreign_objects = self.foreign_object_roots();
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
        // pyro: inline roots that a measuring run left unlike their final layout after this pass
        // last laid them out are laid out again, with the boxes they are in (once: what a second
        // run leaves so the next pass lays out; `LayoutPassState::mark_final_layout_stale`).
        if state.mark_stale_lines_up() {
            taffy::compute_root_layout_in(&mut state, root_element_id, available_space, initial_containing_block);
            state.mark_stale_lines_up();
        }
        #[cfg(feature = "writing-mode")]
        state.physicalise_and_round_layout(root_element_id);
        #[cfg(not(feature = "writing-mode"))]
        taffy::round_layout(&mut state, root_element_id);

        // pyro: each `<foreignObject>` is laid out as a root in a box of its `width` and
        // `height` (presentation attributes, `stylo.rs`), placed at its origin; blitz-paint draws
        // it in the `<svg>`'s user space (`draw_svg`).
        for fo in foreign_objects {
            let fo = crate::taffy_node_id(fo);
            #[cfg(feature = "writing-mode")]
            {
                state.layout_wm = state.layout_wm_of(crate::dom_node_id(fo));
            }
            taffy::compute_root_layout_in(&mut state, fo, initial_containing_block, initial_containing_block);
            #[cfg(feature = "writing-mode")]
            state.physicalise_and_round_layout(fo);
            #[cfg(not(feature = "writing-mode"))]
            taffy::round_layout(&mut state, fo);
        }

        // println!("\n\n");
        // taffy::print_tree(self, root_node_id)
    }
}

/// pyro: `overflow` with what a child adds to it. An empty rect (a
/// box of no width or no height, with nothing reaching out of it) adds nothing,
/// wherever it is: a 2000 x 0 px box does not make a page scroll.
impl BaseDocument {
    /// `table`'s `TableContext` when it is a table whose cells it keeps (`None`: its layout
    /// children are visited as any others').
    fn kept_cells_table(&self, table: NodeId) -> Option<std::sync::Arc<crate::layout::table::TableContext>> {
        use crate::node::SpecialElementData;
        let node = &self.nodes[table];
        if !node.flags.is_table_root() {
            return None;
        }
        let SpecialElementData::TableRoot(ctx) = &node.data.downcast_element()?.special_data else { return None };
        crate::layout::table::keeps_cells(ctx).then(|| std::sync::Arc::clone(ctx))
    }

    /// The union of the overflow of `table`'s cells in it (see `resolve_transforms`; `None` inside:
    /// none has any), or `None` if `table` is not a table whose cells it keeps (its layout children
    /// are visited as any others').
    fn table_cells_overflow(
        &mut self,
        table: NodeId,
        scale: f64,
        resolve_all: bool,
        contents: ContainingTransform,
    ) -> Option<Option<Rect>> {
        let ctx = self.kept_cells_table(table)?;
        let cells = ctx.cells.len();
        let mut placed = ctx.placed.borrow_mut();
        // pyro: under the table's perspective every cell's transform is resolved again.
        let again = placed.take_transform().filter(|_| {
            !resolve_all && !contents.refresh && placed.overflow.len() == cells && placed.overflow_scale == scale
        });
        // pyro: the next hit test brings these cells' hit contributions up to date
        // (`table_cells_hit`).
        placed.transformed(again.as_deref(), cells);
        let mut overflow = std::mem::take(&mut placed.overflow);
        drop(placed);
        match again {
            Some(again) => {
                for index in again {
                    let cell = contents.for_child(&self.nodes, &self.nodes[ctx.cells[index].node_id]);
                    overflow[index] = crate::util::ensure_stack(|| {
                        self.resolve_transforms(ctx.cells[index].node_id, resolve_all, cell)
                    });
                }
            }
            None => {
                overflow.clear();
                for cell in &ctx.cells {
                    let containing = contents.for_child(&self.nodes, &self.nodes[cell.node_id]);
                    overflow.push(crate::util::ensure_stack(|| {
                        self.resolve_transforms(cell.node_id, resolve_all, containing)
                    }));
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

    /// pyro: `table_cells_overflow` for the hit pass (`resolve_hit_overflow`): the union
    /// of `table`'s cells' hit contributions (`unite_hit`), computed again for the cells the
    /// transform passes since the last hit pass visited, and the per-row unions
    /// (`TableContext::row_hit_extents`) of their rows.
    fn table_cells_hit(&mut self, table: NodeId, scale: f64) -> Option<Option<Rect>> {
        let ctx = self.kept_cells_table(table)?;
        let cells = ctx.cells.len();
        let mut placed = ctx.placed.borrow_mut();
        let again = placed.take_hit().filter(|_| placed.hit.len() == cells && placed.hit_scale == scale);
        let mut hit = std::mem::take(&mut placed.hit);
        drop(placed);
        // The rows of the cells computed again (`None`: all).
        let mut changed_rows: Option<Vec<usize>> = None;
        match again {
            Some(again) => {
                let mut rows = Vec::with_capacity(again.len());
                for index in again {
                    hit[index] = crate::util::ensure_stack(|| self.resolve_hit_overflow(ctx.cells[index].node_id));
                    rows.push(ctx.cells[index].row as usize);
                }
                changed_rows = Some(rows);
            }
            None => {
                hit.clear();
                for cell in &ctx.cells {
                    hit.push(crate::util::ensure_stack(|| self.resolve_hit_overflow(cell.node_id)));
                }
            }
        }
        let hit_united = hit
            .iter()
            .filter(|cell| cell.width() > 0.0 || cell.height() > 0.0)
            .fold(None, |united: Option<Rect>, &cell| Some(united.map_or(cell, |united| united.union(cell))));
        let mut placed = ctx.placed.borrow_mut();
        // pyro: the union of the hit contributions per row (the
        // cells originating in it: a hit-test bound of the row,
        // `TableContext::row_hit_extents`), for the rows of the cells
        // computed again only.
        let rows = ctx.rows.len();
        let mut row_start = Vec::with_capacity(rows + 1);
        row_start.push(0usize);
        for row in &ctx.rows {
            row_start.push(row_start.last().unwrap() + row.cell_count as usize);
        }
        let row_union = |row: usize, hit: &[Rect]| {
            let (start, end) = (row_start[row], row_start[row + 1]);
            hit[start.min(hit.len())..end.min(hit.len())]
                .iter()
                .filter(|cell| cell.width() > 0.0 || cell.height() > 0.0)
                .fold(None, |united: Option<Rect>, &cell| Some(united.map_or(cell, |united| united.union(cell))))
                .unwrap_or(Rect::ZERO)
        };
        match changed_rows {
            Some(mut changed) if placed.row_overflow.len() == rows => {
                // The cells come in index order, which is row order.
                if !changed.is_sorted() {
                    changed.sort_unstable();
                }
                changed.dedup();
                for row in changed {
                    if row < rows {
                        placed.row_overflow[row] = row_union(row, &hit);
                    }
                }
            }
            _ => {
                placed.row_overflow = (0..rows).map(|row| row_union(row, &hit)).collect();
            }
        }
        placed.hit = hit;
        placed.hit_scale = scale;
        Some(hit_united)
    }
}

/// pyro: a box's padding box (its own coordinates, device px), where
/// its scrollable overflow and hit bound start (CSS Overflow 3 2.2).
fn padding_box(layout: &taffy::Layout, scale: f64) -> Rect {
    let w = layout.size.width as f64 * scale;
    let h = layout.size.height as f64 * scale;
    let b = layout.border;
    Rect::new(
        b.left as f64 * scale,
        b.top as f64 * scale,
        (w - b.right as f64 * scale).max(b.left as f64 * scale),
        (h - b.bottom as f64 * scale).max(b.top as f64 * scale),
    )
}

/// pyro: a box's rect in its parent: moved to `location`, through
/// its `transform` (the bounding box), as `transform_rect_bbox` of the
/// composed affine computes it (a translation alone adds the same values).
/// pyro: what a box gives the transforms of the boxes it lays out in the transform pass
/// (`resolve_transforms`).
#[derive(Clone, Copy, Default)]
struct ContainingTransform {
    /// What it gives its children's 3D transforms (its perspective, its 3D rendering context).
    context: TransformContext,
    /// Resolve their transforms again, whatever their own damage: it gives them a context
    /// (they may have moved in it, or it changed), or it stopped giving one.
    refresh: bool,
    /// The box, whose children (in the DOM) the context is for.
    parent: Option<NodeId>,
}

impl ContainingTransform {
    /// What `child` gets. Only the element's children in the DOM take its perspective and 3D
    /// rendering context (WPT `perspective-children-only-*`): an out-of-flow box it contains
    /// further down does not.
    ///
    /// pyro (lane H2): a parent without a box of its own passes it on. A `display: contents`
    /// element's children are the children of its parent's box here (WPT
    /// `transform3d-preserve3d-014`); a table's cells are laid out by the table, through rows and
    /// row groups that have no box in Blitz, which pass the table's 3D rendering context on when
    /// they have `transform-style: preserve-3d` themselves (WPT `transform-table-009`, `-010`) and
    /// end it otherwise (`-007`, `-008`).
    fn for_child(&self, nodes: &crate::NodeTree, child: &crate::Node) -> ContainingTransform {
        use style::values::specified::box_::Display;
        let mut parent = child.parent;
        while let Some(id) = parent
            && Some(id) != self.parent
            && let Some(node) = nodes.get(id)
            && let Some(style) = node.primary_styles()
        {
            let display = style.get_box().display;
            let boxless_table_part = [
                Display::TableRow,
                Display::TableRowGroup,
                Display::TableHeaderGroup,
                Display::TableFooterGroup,
            ]
            .contains(&display);
            if !(display.is_contents() || (boxless_table_part && node.is_preserve_3d())) {
                break;
            }
            parent = node.parent;
        }
        if self.parent.is_some() && parent == self.parent {
            *self
        } else {
            ContainingTransform {
                context: TransformContext { scroll: self.context.scroll, ..Default::default() },
                ..*self
            }
        }
    }
}

fn placement(location: (f64, f64), transform: Option<&Affine>) -> impl Fn(Rect) -> Rect {
    let full = transform.map(|t| Affine::translate(location) * *t);
    move |r: Rect| match &full {
        Some(full) => full.transform_rect_bbox(r),
        None => r + kurbo::Vec2::new(location.0, location.1),
    }
}

/// pyro: `hit` with what a child adds to it: as `unite_overflow`, but
/// a rect of no area that has a width or a height counts (text overflowing
/// sideways on lines of `line-height: 0`, a box of no width: hit testing finds
/// what such a box holds, or the box itself).
fn unite_hit(hit: Rect, child: Rect) -> Rect {
    if child.width() > 0.0 || child.height() > 0.0 {
        hit.union(child)
    } else {
        hit
    }
}

/// pyro: how far (device px) the content areas of `layout`'s glyph
/// runs (whole-pixel ascent and descent around the baseline) reach outside
/// their line's box, in any direction: before the line's start, past its end
/// less hanging white space, above and below its block extent. Aligning moves
/// a line with its runs, so it keeps it (see `TextLayout::hit_reach`; the rounding of
/// advances to a layout unit is added by the caller). The lines' union grown
/// by it holds every run.
fn glyph_reach(layout: &parley::Layout<crate::node::TextBrush>) -> f64 {
    let mut reach = 0.0f64;
    for line in layout.lines() {
        let m = line.metrics();
        let offset = m.offset as f64;
        let end = offset + (m.advance - m.hanging_advance) as f64;
        let (top, bottom) = (m.block_min_coord as f64, m.block_max_coord as f64);
        for item in line.items() {
            let parley::PositionedLayoutItem::GlyphRun(run) = item else { continue };
            let metrics = run.run().font_metrics();
            let (x, baseline) = (run.offset() as f64, run.baseline() as f64);
            let x1 = x + run.advance() as f64;
            let y0 = baseline - metrics.ascent.round() as f64;
            let y1 = baseline + metrics.descent.round() as f64;
            // Before the line's start, wherever aligning puts it from the
            // layout's start on (a line overflowing to the left stays put).
            let before = offset - x - offset.min(0.0);
            reach = reach.max(before).max(x1 - end).max(top - y0).max(y1 - bottom);
        }
    }
    reach
}

fn unite_overflow(overflow: Rect, child: Rect) -> Rect {
    if child.width() > 0.0 && child.height() > 0.0 {
        overflow.union(child)
    } else {
        overflow
    }
}

/// pyro: cumulative time per `resolve` phase (style, damage,
/// construct, pconstruct, layout, transform, paint_tree, c_damage, subdocs),
/// for profiling incremental relayout. The phases are timed only while
/// [`set_timing`] has turned timing on; the counters below always count.
pub mod phase_profile {
    use std::cell::Cell;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Instant;
    std::thread_local! {
        static NS: Cell<[u64; 9]> = const { Cell::new([0; 9]) };
    }
    static TIMING: AtomicBool = AtomicBool::new(false);
    /// Turns the phase timers on or off for the whole process.
    pub fn set_timing(on: bool) {
        TIMING.store(on, Ordering::Relaxed);
    }
    /// Whether the phase timers are on.
    pub fn timing() -> bool {
        TIMING.load(Ordering::Relaxed)
    }
    /// The start of a `resolve`'s first phase, when timing is on.
    pub(crate) fn start() -> Option<Instant> {
        timing().then(Instant::now)
    }
    pub(crate) fn mark(i: usize, t: &mut Option<Instant>) {
        let Some(t) = t else { return };
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
        static PERFORMED: Cell<(u64, u64)> = const { Cell::new((0, 0)) };
    }
    /// A box's layout algorithm ran to place its children (not a size
    /// measurement, not a cache hit); `root` for the root element.
    pub(crate) fn performed(root: bool) {
        PERFORMED.with(|p| p.set((p.get().0 + 1, p.get().1 + root as u64)));
    }
    /// Layout algorithm runs that placed children on this thread so far, and
    /// how many of them were the root element's.
    pub fn performed_layouts() -> (u64, u64) {
        PERFORMED.with(|p| p.get())
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
    std::thread_local! {
        static BREAKS: Cell<(u64, u64, u64)> = const { Cell::new((0, 0, 0)) };
    }
    /// An inline layout was broken into lines, continuing after `kept` lines of its last break
    /// and going over `bytes` of its text.
    pub(crate) fn broke(kept: usize, bytes: usize) {
        BREAKS.with(|b| {
            let (n, k, t) = b.get();
            b.set((n + 1, k + kept as u64, t + bytes as u64));
        });
    }
    /// Inline layouts broken into lines on this thread so far, the lines kept from their last
    /// break (resumed), and the text bytes the line breaker went over.
    pub fn line_breaks() -> (u64, u64, u64) {
        BREAKS.with(|b| b.get())
    }
    /// Milliseconds per phase on this thread so far.
    pub fn peek() -> [f64; 9] {
        NS.with(|ns| ns.get().map(|n| n as f64 / 1e6))
    }
}

#[cfg(test)]
mod layout_children_owner_tests {
    use crate::{Attribute, BaseDocument, DocumentConfig, Node, qual_name};
    use blitz_traits::shell::{ColorScheme, Viewport};

    /// Whether the node is in its `layout_parent`'s `layout_children` list, by the list owner it
    /// records (`Node::layout_child_of`).
    pub(super) fn is_layout_child(node: &Node) -> bool {
        node.layout_parent.get().is_some() && node.layout_child_of.get() == node.layout_parent.get()
    }

    /// `Node::layout_child_of` agrees with a scan of every node's layout
    /// parent's `layout_children` list.
    fn assert_owners_match_lists(doc: &BaseDocument, when: &str) {
        let mut members = 0;
        for (id, node) in doc.tree().iter() {
            let parent = node.layout_parent.get().and_then(|p| doc.get_node(p));
            let scan = parent.is_some_and(|p| p.layout_children.borrow().as_ref().is_some_and(|c| c.contains(&id)));
            assert_eq!(is_layout_child(node) && parent.is_some(), scan, "{when}: node {id:?}");
            members += scan as usize;
        }
        assert!(members > 3, "{when}: the document has layout children ({members})");
    }

    #[test]
    fn owner_tracks_list_membership_through_reconstruction() {
        let mut doc = BaseDocument::new(DocumentConfig {
            viewport: Some(Viewport::new(400, 300, 1.0, ColorScheme::Light)),
            ..Default::default()
        });
        let root_id = doc.root_node().id;
        let style = |value: &str| Attribute { name: qual_name!("style"), value: value.to_string() };
        let mut m = doc.mutate();
        let html = m.create_element(qual_name!("html"), vec![style("display:block")]);
        let body = m.create_element(qual_name!("body"), vec![style("display:block")]);
        let outer = m.create_element(qual_name!("div"), vec![style("display:block")]);
        let x = m.create_element(qual_name!("div"), vec![style("display:block")]);
        let x_text = m.create_text_node("x");
        let span = m.create_element(qual_name!("span"), vec![]);
        let inner = m.create_element(qual_name!("div"), vec![style("display:inline-block")]);
        let inner_text = m.create_text_node("inner");
        let y = m.create_element(qual_name!("div"), vec![style("display:block")]);
        let abs = m.create_element(qual_name!("div"), vec![style("display:block;position:absolute")]);
        m.append_children(x, &[x_text]);
        m.append_children(inner, &[inner_text]);
        m.append_children(span, &[inner]);
        m.append_children(outer, &[x, span, y, abs]);
        m.append_children(body, &[outer]);
        m.append_children(html, &[body]);
        m.append_children(root_id, &[html]);
        drop(m);
        doc.resolve(0.0);
        assert_owners_match_lists(&doc, "first layout");

        // Children leave `outer`'s list while keeping it as layout parent:
        // one is no longer displayed, the rest become its inline content.
        let mut m = doc.mutate();
        m.set_attribute(x, qual_name!("style"), "display:none");
        m.set_attribute(y, qual_name!("style"), "display:inline");
        m.set_attribute(abs, qual_name!("style"), "display:none");
        drop(m);
        doc.resolve(0.0);
        assert_owners_match_lists(&doc, "children leave the list");
        assert_eq!(doc.nodes[y].layout_parent.get(), Some(outer));
        assert!(!is_layout_child(&doc.nodes[y]));

        // A block turns inline: it leaves `outer`'s list for an anonymous
        // block's inline content, and the inline-block becomes a block.
        let mut m = doc.mutate();
        m.set_attribute(x, qual_name!("style"), "display:inline");
        m.set_attribute(inner, qual_name!("style"), "display:block");
        drop(m);
        doc.resolve(0.0);
        assert_owners_match_lists(&doc, "display changes");

        // A container turns inline with a block child left in its old list,
        // a child is dropped, and a new block arrives.
        let mut m = doc.mutate();
        m.set_attribute(outer, qual_name!("style"), "display:inline");
        m.remove_and_drop_node(y);
        let z = m.create_element(qual_name!("div"), vec![style("display:block")]);
        m.append_children(body, &[z]);
        drop(m);
        doc.resolve(0.0);
        assert_owners_match_lists(&doc, "container turns inline");

        // And back, so the old lists are replaced again.
        let mut m = doc.mutate();
        m.set_attribute(outer, qual_name!("style"), "display:block");
        m.set_attribute(x, qual_name!("style"), "display:block");
        drop(m);
        doc.resolve(0.0);
        assert_owners_match_lists(&doc, "restored");
    }
}

#[cfg(test)]
mod layout_children_owner_reparent_tests {
    use super::layout_children_owner_tests::is_layout_child;
    use crate::{Attribute, BaseDocument, DocumentConfig, qual_name};
    use blitz_traits::shell::{ColorScheme, Viewport};

    fn scan(doc: &BaseDocument, id: crate::NodeId) -> bool {
        let node = &doc.nodes[id];
        node.layout_parent
            .get()
            .and_then(|p| doc.get_node(p))
            .is_some_and(|p| p.layout_children.borrow().as_ref().is_some_and(|c| c.contains(&id)))
    }

    fn check(doc: &BaseDocument, when: &str) {
        for (id, node) in doc.tree().iter() {
            assert_eq!(is_layout_child(node), scan(doc, id), "{when}: node {id:?}");
        }
    }

    /// A node moved between layout containers, in both traversal orders (its
    /// new container visited before and after its old one), and a node
    /// detached but kept alive.
    #[test]
    fn owner_follows_nodes_moved_between_containers() {
        let mut doc = BaseDocument::new(DocumentConfig {
            viewport: Some(Viewport::new(400, 300, 1.0, ColorScheme::Light)),
            ..Default::default()
        });
        let root_id = doc.root_node().id;
        let block = || vec![Attribute { name: qual_name!("style"), value: "display:block".to_string() }];
        let mut m = doc.mutate();
        let html = m.create_element(qual_name!("html"), block());
        let body = m.create_element(qual_name!("body"), block());
        let a = m.create_element(qual_name!("div"), block());
        let b = m.create_element(qual_name!("div"), block());
        let x = m.create_element(qual_name!("div"), block());
        let y = m.create_element(qual_name!("div"), block());
        let z = m.create_element(qual_name!("div"), block());
        m.append_children(a, &[x, y]);
        m.append_children(b, &[z]);
        m.append_children(body, &[a, b]);
        m.append_children(html, &[body]);
        m.append_children(root_id, &[html]);
        drop(m);
        doc.resolve(0.0);
        check(&doc, "first layout");
        assert!(is_layout_child(&doc.nodes[x]));

        // Later sibling container: `a` (old) is visited before `b` (new).
        let mut m = doc.mutate();
        m.append_children(b, &[x]);
        drop(m);
        doc.resolve(0.0);
        check(&doc, "moved to a later container");
        assert_eq!(doc.nodes[x].layout_parent.get(), Some(b));
        assert!(is_layout_child(&doc.nodes[x]));

        // Back: `a` (new) is visited before `b` (old).
        let mut m = doc.mutate();
        m.append_children(a, &[x]);
        drop(m);
        doc.resolve(0.0);
        check(&doc, "moved to an earlier container");
        assert!(is_layout_child(&doc.nodes[x]));

        // Up to the ancestor, before and after its old container.
        let mut m = doc.mutate();
        m.insert_nodes_before(a, &[x]);
        drop(m);
        doc.resolve(0.0);
        check(&doc, "moved up before its old container");
        let mut m = doc.mutate();
        m.append_children(a, &[x]);
        drop(m);
        doc.resolve(0.0);
        let mut m = doc.mutate();
        m.append_children(body, &[x]);
        drop(m);
        doc.resolve(0.0);
        check(&doc, "moved up after its old container");
        assert_eq!(doc.nodes[x].layout_parent.get(), Some(body));

        // Detached but alive: in no list.
        let mut m = doc.mutate();
        m.remove_node(y);
        drop(m);
        doc.resolve(0.0);
        check(&doc, "detached");
        assert!(!is_layout_child(&doc.nodes[y]));
    }
}
