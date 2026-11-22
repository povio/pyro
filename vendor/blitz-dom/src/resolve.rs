//! Resolve style and layout

use stylo_taffy::CopyStyleGetters;
use blitz_traits::node_id::NodeId;
use std::cell::RefCell;

use debug_timer::debug_timer;
use kurbo::Rect;
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
        // pyro: the page's `<meta name=color-scheme>`.
        self.update_page_color_scheme();
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
        let has_queries = self.stylist.has_container_queries();
        if !has_queries {
            self.nodes.anchor_query_choices.lock().unwrap().clear();
        }
        if has_queries || self.has_queried_container_units() {
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
            if needs_layout || (geometry_dirty && !self.nodes.anchor_query_choices.lock().unwrap().is_empty()) {
                self.resolve(current_time_for_animations);
            }
        } else {
            self.nodes.container_query_sizes.lock().unwrap().clear();
        }
    }

    fn has_queried_container_units(&self) -> bool {
        use style::computed_value_flags::ComputedValueFlags;
        !self.nodes.container_query_sizes.lock().unwrap().is_empty()
            && self.nodes.iter().any(|(_, node)| {
                node.primary_styles().is_some_and(|style| style.flags.contains(ComputedValueFlags::USES_CONTAINER_UNITS))
            })
    }

    /// pyro: whether an element's restyle damage reaches layout (a box changed, not only what
    /// paints): the styles were resolved since the last layout ([`Self::resolve_style`]).
    pub(crate) fn has_layout_damage(&self) -> bool {
        self.root_element().has_damaged_descendants()
            && self.nodes.iter().any(|(_, node)| node.damage().is_some_and(crate::layout::damage::reaches_layout))
    }

    pub fn resolve(&mut self, current_time_for_animations: f64) {
        self.anchor_flush_choices.clear();
        self.resolve_in_flush(current_time_for_animations);
    }

    fn resolve_in_flush(&mut self, current_time_for_animations: f64) {
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
        // pyro: scroll-driven animations follow the scroll position.
        let _ = self.resample_scroll_driven();

        // Drop scrollbar-activity entries whose fade-out has finished (also
        // sheds entries for removed nodes).
        {
            use crate::node::scrollbar::{FADE_DELAY, FADE_DURATION};
            self.scrollbar_activity.retain(|_, last| last.elapsed() < FADE_DELAY + FADE_DURATION);
        }

        let root_node_id = self.root_element().id;
        debug_timer!(timer, feature = "log-phase-times");
        let mut phase_t = phase_profile::start();

        // pyro: the page's `<meta name=color-scheme>`.
        self.update_page_color_scheme();
        // Apply any device changes (viewport resize, zoom, color-scheme, etc)
        // accumulated since the last resolve as a single device rebuild.
        self.flush_pending_device_changes();

        // Container conditions consume content-box sizes from layout. Re-match
        // only when an actually queried size changes; bound cyclic feedback.
        self.container_float_floors.clear();
        for pass in 0..32 {
            // we need to resolve stylist first since it will need to drive our layout bits
            self.resolve_stylist(current_time_for_animations);
            // pyro: a new `content-visibility: auto` element is skipped until its relevance is
            // determined after a layout (as in Chromium).
            if pass == 0 {
                self.skip_new_content_visibility_auto();
            }
            // pyro: classic scrollbars: viewport units see the viewport less the root's
            // `scroll`/`stable` gutters (Chrome), known once the root is styled: a change styles
            // the document again.
            if self.update_viewport_unit_inset() {
                self.flush_pending_device_changes();
                self.resolve_stylist(current_time_for_animations);
            }
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
            if !self.stylist.has_container_queries() && !self.has_queried_container_units() {
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
            self.resolve_layout_in_flush();
            timer.record_time("layout");
            phase_profile::mark(4, &mut phase_t);
            let changed_containers = self.refresh_container_query_sizes();
            let choices_changed = pass == 0 && self.refresh_anchored_query_choices();
            let container_sizes_changed = !changed_containers.is_empty();
            // pyro: the active scroll markers and the scroll buttons' states follow the layout
            // and the scroll positions (`layout::scroll_markers`): a change styles them again.
            let scroll_states_changed = !container_sizes_changed && self.update_target_current();
            // pyro: a marker's content that reads counters was met: they are evaluated now.
            let marker_counters = std::mem::take(&mut self.marker_counters_pending);
            // pyro: scroll-driven animations at this layout (the first sample of a new document
            // had none: no scroll range yet); a change styles and lays out once more.
            let timelines_changed = !container_sizes_changed && pass < 2 && self.resample_scroll_driven();
            // pyro: `content-visibility: auto` elements skip or render their contents as they are
            // relevant to the user at this layout; a change lays out again.
            let visibility_changed = !container_sizes_changed && pass < 8 && self.update_content_visibility();
            if (!choices_changed && !container_sizes_changed && !scroll_states_changed && !marker_counters && !timelines_changed && !visibility_changed)
                || pass == 31
            {
                break;
            }
            if !container_sizes_changed {
                continue;
            }
            // pyro: only the subtrees of the containers whose size changed (or that stopped being
            // containers) can depend on it: container queries and container units resolve
            // against an ancestor. They restyle and lay out again; the rest of the document keeps
            // its styles and layout caches (a whole-document pass per change made each layout
            // read after a React commit on a page of container-query cards cost a full restyle).
            self.restyle_container_subtrees(&changed_containers);
        }
        self.nodes.bump_geometry_generation();

        // Resolve transforms
        let resolve_all = self.flags.contains(DocumentFlags::RESOLVE_ALL_TRANSFORMS);
        self.flags.remove(DocumentFlags::RESOLVE_ALL_TRANSFORMS);
        self.resolve_transforms(root_node_id, resolve_all, ContainingTransform::default());
        for fo in self.foreign_object_roots() {
            self.resolve_transforms(fo, resolve_all, ContainingTransform::default());
        }
        // pyro: classic scrollbars (`stylo_taffy::scrollbars`): an `overflow: auto` box's or the
        // viewport's scrollbar shows when its scrollable overflow (with transforms and
        // `overflow-clip-margin`, as resolved here) passes its scrollport, and stops showing when
        // it no longer does with the scrollbar. A change lays the document out again (as Chrome
        // does), a few times at most.
        // Only a layout checks them. The first check may add and remove scrollbars; the relayouts
        // a change makes may add more (the overflow a first bar causes: a 100px box with 95x200
        // content gets both bars, as in Chrome) but remove none, so the passes end.
        if self.nodes.classic_scrollbars
            && std::mem::take(&mut self.layout_ran)
        {
            for pass in 0..3 {
                let may_remove = pass == 0;
                let boxes_changed = self.update_auto_scrollbars(may_remove);
                let viewport_changed = self.update_viewport_scrollbars(may_remove);
                if !boxes_changed && !viewport_changed {
                    break;
                }
                self.last_layout_space = None;
                self.resolve_layout_in_flush();
                self.resolve_transforms(root_node_id, false, ContainingTransform::default());
                for fo in self.foreign_object_roots() {
                    self.resolve_transforms(fo, false, ContainingTransform::default());
                }
            }
            self.layout_ran = false;
        }
        // pyro: `content-visibility: auto` relevance with the transforms this pass placed (the
        // layout passes saw the last ones): a change resolves once more.
        if !self.cv_resolving && self.update_content_visibility() {
            self.cv_resolving = true;
            self.resolve_in_flush(current_time_for_animations);
            self.cv_resolving = false;
        }
        #[cfg(feature = "svg")]
        self.update_svg_image_documents();
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
        if self.transition_trees.is_some() { self.refresh_transition_trees(); }
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
            // pyro: the frame's viewport is its content box (Blitz took the border box), and a
            // frame document from the render snapshot keeps its scroll position.
            let layout = node.final_layout();
            let size = taffy::Size {
                width: (layout.size.width - layout.border.left - layout.border.right - layout.padding.left - layout.padding.right)
                    .max(0.0),
                height: (layout.size.height - layout.border.top - layout.border.bottom - layout.padding.top - layout.padding.bottom)
                    .max(0.0),
            };
            // pyro: `<iframe scrolling=no>` (HTML § 15.5.14): its viewport shows no scrollbars.
            let no_scrollbars = node
                .attr(markup5ever::LocalName::from("scrolling"))
                .is_some_and(|value| value.eq_ignore_ascii_case("no") || value.eq_ignore_ascii_case("noscroll"));
            // pyro: the frame document's animations (by its client), sampled into it.
            let frame_animations = self.frame_animations.clone();
            // (Its client and scroll position from its frame table entry, `DocumentConfig::frames`.)
            let frames = self.frames.clone();
            let entry = node
                .reserved_attr(crate::FRAME_ATTRIBUTE)
                .and_then(|value| frames.lookup(value));
            let frame_effects = entry.and_then(|frame| frame_animations.get(&frame.client).cloned());
            let frame_scroll = entry
                .map(|frame| frame.scroll)
                .filter(|&(x, y)| x.is_finite() && y.is_finite() && (x, y) != (0.0, 0.0));
            if let Some(mut sub_doc) = node.subdoc_mut().map(|doc| doc.inner_mut()) {
                sub_doc.set_media_environment(self.media_environment);
                if sub_doc.frame_no_scrollbars != no_scrollbars {
                    sub_doc.frame_no_scrollbars = no_scrollbars;
                    sub_doc.last_layout_space = None;
                    sub_doc.layout_ran = true;
                }
                // Set viewport
                // viewport_mut handles change detection. So we just unconditionally set the values;
                let mut sub_viewport = sub_doc.viewport_mut();
                sub_viewport.hidpi_scale = self.viewport.hidpi_scale;
                sub_viewport.zoom = self.viewport.zoom;
                sub_viewport.color_scheme = self.viewport.color_scheme;
                sub_viewport.input = self.viewport.input;

                let viewport_scale = self.viewport.scale();
                sub_viewport.window_size =
                    ((size.width * viewport_scale) as u32, (size.height * viewport_scale) as u32);
                drop(sub_viewport);

                // pyro: its scroll position first (its scroll timelines read it), and its
                // animations now.
                if let Some((x, y)) = frame_scroll {
                    sub_doc.set_viewport_scroll(crate::Point { x, y });
                }
                sub_doc.frame_animations = frame_animations.clone();
                sub_doc.sample_frame_animations(frame_effects.as_deref().map(|v| v.as_slice()));
                // pyro: its scroll timelines read the frame's scroll position as given (its viewport
                // clamps it until laid out): its resolve places them at it, styling and laying out
                // again within that one resolve (`resample_scroll_driven`), each frame resolved once.
                if let Some((x, y)) = frame_scroll {
                    sub_doc.set_timeline_scroll(std::collections::HashMap::new(), (x, y));
                }
                sub_doc.resolve(current_time_for_animations);
                if let Some((x, y)) = frame_scroll {
                    // pyro: the offset the frame can reach as laid out now (it may have shrunk in
                    // this resolve): its scroll timelines are placed at that one and styled again;
                    // when that changes a box (an animated width), it is laid out once more (its
                    // timelines stay at that offset: no further clamp).
                    let scale = sub_doc.viewport().scale_f64();
                    let (vw, vh) = (
                        sub_doc.viewport().window_size.0 as f64 / scale,
                        sub_doc.viewport().window_size.1 as f64 / scale,
                    );
                    let (dw, dh) = sub_doc.document_scroll_size((vw, vh));
                    let (mx, my) = ((dw - vw).max(0.0), (dh - vh).max(0.0));
                    let rx = if sub_doc.viewport_scrolls_leftward() { (-mx, 0.0) } else { (0.0, mx) };
                    let reachable = (x.clamp(rx.0, rx.1), y.clamp(0.0, my));
                    let mut relayout = false;
                    if reachable != (x, y) {
                        sub_doc.set_timeline_scroll(std::collections::HashMap::new(), reachable);
                        if sub_doc.resample_scroll_driven() {
                            sub_doc.resolve_style(current_time_for_animations);
                            relayout = sub_doc.has_layout_damage();
                        }
                    }
                    sub_doc.set_viewport_scroll(crate::Point { x: reachable.0, y: reachable.1 });
                    if relayout {
                        sub_doc.resolve(current_time_for_animations);
                    }
                }

                subdoc_is_animating |= sub_doc.is_animating();
            }
        }
        self.subdoc_is_animating = subdoc_is_animating;
        timer.record_time("subdocs");
        phase_profile::mark(8, &mut phase_t);

        timer.print_times(&format!("Resolve({}): ", self.id()));
    }

    /// pyro: whether `style` makes a `content-visibility: auto` element whose contents can be
    /// skipped (containment applies: not `display: contents`/`none`, a non-atomic inline, a table
    /// or an internal table box).
    fn is_cv_auto(node: &crate::Node) -> bool {
        use style::values::specified::box_::{ContentVisibility, DisplayInside, DisplayOutside};
        node.is_element()
            && node.primary_styles().is_some_and(|style| {
                let display = style.get_box().display;
                style.get_box().slow_clone_content_visibility() == ContentVisibility::Auto
                    && !display.is_contents()
                    && !display.is_none()
                    && !(display.outside() == DisplayOutside::Inline && display.inside() == DisplayInside::Flow)
                    && display.inside() != DisplayInside::Table
                    && display.outside() != DisplayOutside::InternalTable
            })
    }

    /// pyro: skips the contents of the `content-visibility: auto` elements whose relevance was not
    /// determined yet (`update_content_visibility` determines it after their first layout).
    fn skip_new_content_visibility_auto(&mut self) {
        use crate::node::NodeFlags;
        let new: Vec<NodeId> = self
            .nodes
            .iter()
            .filter(|(_, node)| !node.flags.contains(NodeFlags::CV_DETERMINED) && Self::is_cv_auto(node))
            .map(|(id, _)| id)
            .collect();
        for id in new {
            let node = &mut self.nodes[id];
            node.flags.insert(NodeFlags::CV_DETERMINED | NodeFlags::CV_SKIPPED);
            node.insert_damage(ALL_DAMAGE);
            self.cv_auto.insert(id);
        }
    }

    /// pyro: the page's scroll position: the one its embedder gives for scroll timelines, which
    /// this document's own viewport may not have yet.
    fn cv_page_scroll(&self) -> (f64, f64) {
        match &self.timeline_scroll {
            Some((_, v)) => *v,
            None => (self.viewport_scroll().x, self.viewport_scroll().y),
        }
    }

    /// pyro: the steps the `content-visibility: auto` relevance passes took so far (candidates
    /// looked up, chain steps walked): linear in the tracked elements per pass.
    pub fn content_visibility_work(&self) -> u64 {
        self.cv_work.get()
    }

    /// pyro: whether relevance was determined at another scroll position than the page's now
    /// (a frame determines it again: `content-visibility: auto` follows scrolling).
    pub fn content_visibility_stale(&self) -> bool {
        !self.cv_auto.is_empty() && self.cv_scroll != Some(self.cv_page_scroll())
    }

    /// pyro: `content-visibility: auto` (CSS Contain 2 § 4.1): each such element (outside skipped
    /// contents) is relevant to the user when its border box, as its transforms place it (its
    /// visual bounds, as an IntersectionObserver sees them), is near the viewport (within half
    /// the viewport's size of it, as Chromium's margin), it holds the focus or a top-layer
    /// element, or a query needs it (`force_content_visibility`); an irrelevant one skips its
    /// contents, sized by `contain-intrinsic-size` (its last remembered size for `auto`, which
    /// `record_remembered_sizes` keeps at each rendering update, not at each layout). Returns
    /// whether one changed, which the caller lays out again. Each node's placement and whether it is in skipped contents are
    /// computed once (memoized up the chains), the focus's and top layer's ancestors once.
    pub(crate) fn update_content_visibility(&mut self) -> bool {
        use crate::node::NodeFlags;
        use std::collections::{HashMap, HashSet};
        // (Removed elements leave the set: pruned in place, one lookup each.)
        let nodes = &self.nodes;
        let mut work = 0u64;
        self.cv_auto.retain(|&id| {
            work += 1;
            nodes.get(id).is_some_and(|n| n.flags.is_in_document())
        });
        let candidates: Vec<NodeId> = self.cv_auto.iter().copied().collect();
        self.cv_work.set(self.cv_work.get() + work);
        if candidates.is_empty() {
            return false;
        }
        let scale = self.viewport.scale_f64();
        let (w, h) = (self.viewport.window_size.0 as f64 / scale, self.viewport.window_size.1 as f64 / scale);
        let scroll = self.cv_page_scroll();
        self.cv_scroll = Some(scroll);
        let view = kurbo::Rect::new(scroll.0 - w / 2.0, scroll.1 - h / 2.0, scroll.0 + w * 1.5, scroll.1 + h * 1.5);
        // The ancestors (inclusive) of the focus and of the top layer's elements (whatever skips
        // them: not `top_layer_nodes`, which leaves out those in skipped contents).
        let mut holders: HashSet<NodeId> = HashSet::new();
        let mut roots: Vec<NodeId> = self.focus_node_id.into_iter().collect();
        roots.extend(self.top_layer.iter().filter(|&t| {
            self.nodes.get(t).is_some_and(|n| n.flags.is_in_document() && n.is_element()) && self.renders_in_top_layer(t)
        }));
        for root in roots {
            // (Inclusive: a focused or top-layer `auto` element holds itself.)
            let mut at = self.nodes.get(root).map(|_| root);
            while let Some(n) = at {
                if !holders.insert(n) {
                    break;
                }
                at = self.nodes[n].parent;
            }
        }
        // (Chain steps walked, for `cv_work`.)
        let steps = std::cell::Cell::new(0u64);
        // Whether a node is in skipped contents (a skipped ancestor), memoized up the parents.
        let mut skipped_memo: HashMap<NodeId, bool> = HashMap::new();
        let in_skipped = |nodes: &crate::NodeTree, id: NodeId, skipped_memo: &mut HashMap<NodeId, bool>| -> bool {
            let mut chain = Vec::new();
            let mut at = nodes.get(id).and_then(|n| n.parent);
            let mut result = false;
            while let Some(n) = at {
                if let Some(&known) = skipped_memo.get(&n) {
                    result = known;
                    break;
                }
                if nodes[n].flags.contains(NodeFlags::CV_SKIPPED) {
                    result = true;
                    break;
                }
                chain.push(n);
                steps.set(steps.get() + 1);
                at = nodes[n].parent;
            }
            for n in chain {
                skipped_memo.insert(n, result);
            }
            result
        };
        // A node's box to the document (CSS px) up its containing blocks as paint places it
        // (`Node::paint_placement`: sticky offsets and transforms), memoized: its matrix, whether
        // it is fixed to the viewport (a fixed box whose containing block is the root, not a
        // transformed ancestor), the clip of the clipping containing blocks around it (document
        // coordinates), and whether a 3D plane places it (counted as relevant: not mapped here).
        #[derive(Clone, Copy)]
        struct Placed {
            t: kurbo::Affine,
            fixed: bool,
            clip: Option<kurbo::Rect>,
            planar: bool,
        }
        let root = self.try_root_element().map(|r| r.id);
        let clips = |node: &crate::Node| {
            node.primary_styles().is_some_and(|s| {
                use style::values::computed::Overflow;
                let b = s.get_box();
                b.overflow_x != Overflow::Visible || b.overflow_y != Overflow::Visible
            })
        };
        let placement = |nodes: &crate::NodeTree, id: NodeId, placed: &mut HashMap<NodeId, Placed>| -> Placed {
            let mut chain = Vec::new();
            let mut at = Some(id);
            let mut base = Placed { t: kurbo::Affine::IDENTITY, fixed: false, clip: None, planar: false };
            while let Some(n) = at {
                if let Some(&known) = placed.get(&n) {
                    base = known;
                    break;
                }
                chain.push(n);
                steps.set(steps.get() + 1);
                at = nodes[n].containing_block();
            }
            let mut parent = at;
            for &n in chain.iter().rev() {
                let node = &nodes[n];
                let mut t = base.t;
                let mut clip = base.clip;
                if let Some(p) = parent {
                    let pn = &nodes[p];
                    // A clipping containing block cuts what it holds at its padding box.
                    if clips(pn) && Some(p) != root {
                        let l = pn.final_layout();
                        let padding = kurbo::Rect::new(
                            l.border.left as f64,
                            l.border.top as f64,
                            (l.size.width - l.border.right) as f64,
                            (l.size.height - l.border.bottom) as f64,
                        );
                        let r = base.t.transform_rect_bbox(padding);
                        clip = Some(clip.map_or(r, |c| c.intersect(r)));
                    }
                    let o = pn.scroll_offset();
                    t = t * kurbo::Affine::translate((-o.x, -o.y));
                }
                let fixed_box =
                    node.primary_styles().is_some_and(|s| s.clone_position() == style::computed_values::position::T::Fixed);
                let fixed = if fixed_box { parent.is_none() || parent == root } else { base.fixed };
                base = Placed {
                    t: t * node.paint_placement(scale),
                    fixed,
                    clip,
                    planar: base.planar || node.plane_3d().is_some(),
                };
                placed.insert(n, base);
                parent = Some(n);
            }
            base
        };
        let mut placed: HashMap<NodeId, Placed> = HashMap::new();
        let mut changed = false;
        for id in candidates {
            steps.set(steps.get() + 1);
            if !Self::is_cv_auto(&self.nodes[id]) {
                // No longer `auto`: shown as its style says.
                let node = &mut self.nodes[id];
                if node.flags.contains(NodeFlags::CV_SKIPPED) {
                    node.flags.remove(NodeFlags::CV_SKIPPED);
                    node.insert_damage(ALL_DAMAGE);
                    changed = true;
                }
                node.flags.remove(NodeFlags::CV_DETERMINED | NodeFlags::CV_FORCED);
                self.cv_auto.remove(&id);
                self.cv_forced.remove(&id);
                continue;
            }
            // Inside skipped contents it is not laid out and keeps its state.
            if in_skipped(&self.nodes, id, &mut skipped_memo) {
                continue;
            }
            let p = placement(&self.nodes, id, &mut placed);
            let l = *self.nodes[id].final_layout();
            // (What it may paint: its paint containment clips at its `overflow-clip-margin`, from
            // the margin's reference box, so overflow reaching the viewport through it is near
            // enough; WPT css-overflow `overflow-clip-margin-007`, `-012`.)
            let reference = {
                use style::values::generics::box_::OverflowClipMarginBox;
                let border = kurbo::Rect::new(0.0, 0.0, l.size.width as f64, l.size.height as f64);
                let (b, pd) = (l.border, l.padding);
                let margin = self.nodes[id].primary_styles().map(|s| s.get_margin().overflow_clip_margin.clone());
                match margin {
                    Some(m) => {
                        let inner = match m.visual_box {
                            OverflowClipMarginBox::BorderBox => border,
                            OverflowClipMarginBox::PaddingBox => {
                                kurbo::Rect::new(b.left as f64, b.top as f64, border.x1 - b.right as f64, border.y1 - b.bottom as f64)
                            }
                            OverflowClipMarginBox::ContentBox => kurbo::Rect::new(
                                (b.left + pd.left) as f64,
                                (b.top + pd.top) as f64,
                                border.x1 - (b.right + pd.right) as f64,
                                border.y1 - (b.bottom + pd.bottom) as f64,
                            ),
                        };
                        let o = m.offset.px().max(0.0) as f64;
                        // (Its border box at least: the margin only reaches further.)
                        border.union(inner.inflate(o, o))
                    }
                    None => border,
                }
            };
            // A transform that collapses it to no area shows nothing.
            let collapsed = p.t.determinant().abs() < 1e-12;
            let mut rect = p.t.transform_rect_bbox(reference);
            if let Some(clip) = p.clip {
                rect = rect.intersect(clip);
            }
            if p.fixed {
                rect = rect + kurbo::Vec2::new(scroll.0, scroll.1);
            }
            // (An empty box in view counts, as an IntersectionObserver's edge-adjacent target; one
            // its clips leave nothing of does not.)
            let visible = !collapsed && rect.x1 >= rect.x0 && rect.y1 >= rect.y0;
            let near = p.planar || (visible && rect.x0 < view.x1 && rect.x1 > view.x0 && rect.y0 < view.y1 && rect.y1 > view.y0);
            // (A query forced it: rendered until the next frame, `clear_content_visibility_forcing`.)
            let forced = self.nodes[id].flags.contains(NodeFlags::CV_FORCED);
            let relevant = forced || holders.contains(&id) || near;
            let node = &mut self.nodes[id];
            if node.flags.contains(NodeFlags::CV_SKIPPED) == relevant {
                node.flags.set(NodeFlags::CV_SKIPPED, !relevant);
                node.insert_damage(ALL_DAMAGE);
                if relevant {
                    self.unskip_contents(id);
                }
                // (The memos stand: no box moves until the next layout.)
                changed = true;
            }
        }
        self.cv_work.set(self.cv_work.get() + steps.get());
        changed
    }

    /// pyro: the contents of `id`, skipped until now, are styled and built again: every node of
    /// its subtree is damaged (boxes never built under it, an absolutely positioned box its
    /// containing block hoists).
    fn unskip_contents(&mut self, id: NodeId) {
        self.nodes[id].set_restyle_hint(style::invalidation::element::restyle_hints::RestyleHint::restyle_subtree());
        self.iter_subtree_mut(id, |node_id: NodeId, doc: &mut BaseDocument| {
            doc.nodes[node_id].insert_damage(ALL_DAMAGE);
        });
    }

    /// pyro: the `content-visibility: auto` elements whose relevance was determined: whether
    /// they skip their contents, and their last remembered size; then the other elements a size
    /// is remembered for (`contain-intrinsic-size: auto`), as not skipping.
    pub fn content_visibility_state(&self) -> Vec<(NodeId, bool, [Option<f32>; 2])> {
        use crate::node::NodeFlags;
        let cv = self
            .cv_auto
            .iter()
            .filter_map(|&id| self.nodes.get(id).map(|n| (id, n)))
            .map(|(id, n)| (id, n.flags.contains(NodeFlags::CV_SKIPPED), n.layout_data().remembered_size));
        let remembered = self
            .cis_auto
            .iter()
            .filter(|id| !self.cv_auto.contains(id))
            .filter_map(|&id| self.nodes.get(id).map(|n| (id, n)))
            .filter(|(_, n)| n.layout_data().remembered_size != [None; 2])
            .map(|(id, n)| (id, false, n.layout_data().remembered_size));
        cv.chain(remembered).collect()
    }

    /// pyro: takes the `content-visibility: auto` state another document of the page determined
    /// (`content_visibility_state`): its elements skip or show their contents as there, with its
    /// remembered sizes, until relevance is determined here again.
    pub fn set_content_visibility_state(&mut self, state: Vec<(NodeId, bool, [Option<f32>; 2])>) {
        use crate::node::NodeFlags;
        for (id, skipped, size) in state {
            let Some(node) = self.nodes.get_mut(id) else { continue };
            if node.layout_data().remembered_size != size {
                node.layout_data_mut().remembered_size = size;
                // (Its size, while it skips its contents, may be that one.)
                node.insert_damage(ALL_DAMAGE);
            }
            if size != [None; 2] {
                self.cis_auto.insert(id);
            }
            // (An element only a size is remembered for: no relevance to take. One not styled yet
            // is taken as there, as `update_content_visibility` drops it if it is no `auto` one.)
            let node = &self.nodes[id];
            if node.primary_styles().is_some() && !Self::is_cv_auto(node) {
                continue;
            }
            let node = &self.nodes[id];
            let determined = node.flags.contains(NodeFlags::CV_DETERMINED);
            self.cv_auto.insert(id);
            let node = &mut self.nodes[id];
            if determined && node.flags.contains(NodeFlags::CV_SKIPPED) == skipped {
                continue;
            }
            node.flags.insert(NodeFlags::CV_DETERMINED);
            node.flags.set(NodeFlags::CV_SKIPPED, skipped);
            node.insert_damage(ALL_DAMAGE);
            if !skipped {
                self.unskip_contents(id);
            }
        }
    }

    /// pyro: whether some element may remember a size (`contain-intrinsic-size: auto` in an axis,
    /// or `content-visibility: auto`), as of the last style traversal.
    pub fn has_remembered_size_candidates(&self) -> bool {
        !self.cis_auto.is_empty()
    }

    /// pyro: after a style traversal (`stylo.rs`), each element that may remember a size forgets
    /// it in each logical axis whose `contain-intrinsic-size` lost `auto`, at once (WPT css-sizing
    /// `contain-intrinsic-size/auto-006`, "Losing cis:auto removes last remembered size
    /// immediately"); one with `auto` in neither axis, or removed, leaves the set. One not styled
    /// (in a `display: none` subtree) keeps its sizes: they survive its box (`auto-006`).
    pub(crate) fn retain_contain_intrinsic_auto(&mut self) {
        if self.cis_auto.is_empty() {
            return;
        }
        // (As `sticky_nodes`: one lookup per element of the set.)
        let nodes = &mut self.nodes;
        self.cis_auto.retain(|&id| {
            let Some(node) = nodes.get_mut(id) else { return false };
            if !node.flags.is_in_document() {
                node.layout_data_mut().remembered_size = [None; 2];
                return false;
            }
            let Some(styles) = node.primary_styles() else { return true };
            let (width, height) = contain_intrinsic_auto(&styles);
            let vertical = styles.writing_mode.is_vertical();
            drop(styles);
            let (inline, block) = if vertical { (height, width) } else { (width, height) };
            let remembered = &mut node.layout_data_mut().remembered_size;
            if !inline {
                remembered[0] = None;
            }
            if !block {
                remembered[1] = None;
            }
            inline || block
        });
    }

    /// pyro: the last remembered sizes (CSS Sizing 4 § 5.1), at a rendering update, after its
    /// animation frame callbacks and immediately before its ResizeObserver callbacks (WPT
    /// css-sizing `contain-intrinsic-size/auto-008`), never at a layout alone (`auto-006`: sized
    /// normally, then skipping again in the same task, it keeps the earlier size). Each element
    /// with `contain-intrinsic-size: auto` in an axis that is laid out with its contents (it and
    /// its ancestors render, none skips its contents) remembers its content box size in the
    /// logical axes with `auto` and forgets the others (`auto-009`; size containment does not
    /// stop it: `auto-011`, `auto-012`). One that skips its contents or has no box keeps its
    /// sizes; a disconnected one forgets them (`forget-on-disconnect-in-iframe`). Nothing is
    /// laid out again: a size is used only while its element skips its contents.
    pub fn record_remembered_sizes(&mut self) {
        let ids: Vec<NodeId> = self.cis_auto.iter().copied().collect();
        let mut work = 0u64;
        for id in ids {
            work += 1;
            let Some(node) = self.nodes.get(id) else {
                self.cis_auto.remove(&id);
                continue;
            };
            if !node.flags.is_in_document() {
                self.nodes[id].layout_data_mut().remembered_size = [None; 2];
                self.cis_auto.remove(&id);
                continue;
            }
            // (Laid out with its contents: it and its ancestors have boxes, none skips them.)
            let mut laid_out = true;
            let mut at = Some(id);
            while let Some(n) = at {
                work += 1;
                let node = &self.nodes[n];
                if node.is_element() {
                    let Some(styles) = node.primary_styles() else {
                        laid_out = false;
                        break;
                    };
                    let display = styles.get_box().display;
                    if display.is_none() || (n == id && display.is_contents()) || skips_contents(node, &styles) {
                        laid_out = false;
                        break;
                    }
                }
                at = node.parent;
            }
            if !laid_out {
                continue;
            }
            let node = &self.nodes[id];
            let Some(styles) = node.primary_styles() else { continue };
            let (auto_width, auto_height) = contain_intrinsic_auto(&styles);
            let vertical = styles.writing_mode.is_vertical();
            drop(styles);
            let l = *node.final_layout();
            let width = (l.size.width - l.border.left - l.border.right - l.padding.left - l.padding.right).max(0.0);
            let height = (l.size.height - l.border.top - l.border.bottom - l.padding.top - l.padding.bottom).max(0.0);
            let (inline, block, auto_inline, auto_block) =
                if vertical { (height, width, auto_height, auto_width) } else { (width, height, auto_width, auto_height) };
            self.nodes[id].layout_data_mut().remembered_size = [auto_inline.then_some(inline), auto_block.then_some(block)];
            if !auto_inline && !auto_block {
                self.cis_auto.remove(&id);
            }
        }
        self.remembered_size_work.set(self.remembered_size_work.get() + work);
    }

    /// pyro: the steps `record_remembered_sizes` took so far (elements looked up, ancestors
    /// walked).
    pub fn remembered_size_work(&self) -> u64 {
        self.remembered_size_work.get()
    }

    /// pyro: whether a query forced some `content-visibility: auto` element to render
    /// (`force_content_visibility`) since the last frame.
    pub fn has_content_visibility_forcing(&self) -> bool {
        !self.cv_forced.is_empty()
    }

    /// pyro: the elements queries forced to render are rendered only as relevance gives them now:
    /// called at each frame (`update_content_visibility` determines them at its next layout).
    pub fn clear_content_visibility_forcing(&mut self) {
        use crate::node::NodeFlags;
        for id in std::mem::take(&mut self.cv_forced) {
            let Some(node) = self.nodes.get_mut(id) else { continue };
            node.flags.remove(NodeFlags::CV_FORCED);
            node.insert_damage(ALL_DAMAGE);
        }
    }

    /// pyro: renders the contents of the `content-visibility: auto` ancestors of `node` that skip
    /// them, for a query that needs its layout (a geometry read, `scrollIntoView`): they are
    /// relevant at the next determination (CSS Contain 2 § 4.1, as Chromium forces them).
    /// Returns whether one was skipped (the caller resolves again).
    pub fn force_content_visibility(&mut self, node: NodeId) -> bool {
        use crate::node::NodeFlags;
        let mut forced = false;
        let mut at = self.nodes.get(node).and_then(|n| n.parent);
        while let Some(id) = at {
            let n = &mut self.nodes[id];
            if n.flags.contains(NodeFlags::CV_SKIPPED) {
                n.flags.remove(NodeFlags::CV_SKIPPED);
                n.flags.insert(NodeFlags::CV_FORCED);
                n.insert_damage(ALL_DAMAGE);
                let parent = n.parent;
                self.cv_forced.insert(id);
                self.unskip_contents(id);
                forced = true;
                at = parent;
                continue;
            } else if n.flags.contains(NodeFlags::CV_DETERMINED) {
                // (Kept rendered until the next frame, as the query read it.)
                n.flags.insert(NodeFlags::CV_FORCED);
                self.cv_forced.insert(id);
            }
            at = n.parent;
        }
        forced
    }

    /// pyro: restyles and lays out again the subtrees of `containers` (see the caller in
    /// `resolve`): each one's own subtree gets a full restyle hint, every node under it (DOM,
    /// pseudo-element and anonymous boxes, by parent or layout parent) its layout cache cleared
    /// and full damage, and its ancestors their layout caches cleared (an `inline-size`
    /// container's block size still follows its content).
    fn restyle_container_subtrees(&mut self, containers: &[NodeId]) {
        let set: std::collections::HashSet<NodeId> = containers.iter().copied().collect();
        let under: Vec<NodeId> = self
            .nodes
            .iter()
            .filter(|(id, _)| {
                let mut cur = Some(*id);
                let mut steps = 0;
                while let Some(n) = cur {
                    if set.contains(&n) {
                        return true;
                    }
                    let Some(node) = self.nodes.get(n) else { return false };
                    cur = node.parent.or_else(|| node.layout_parent.get());
                    steps += 1;
                    if steps > 4096 {
                        return true;
                    }
                }
                false
            })
            .map(|(id, _)| id)
            .collect();
        for id in under {
            let node = &mut self.nodes[id];
            node.clear_layout_cache();
            node.insert_damage(ALL_DAMAGE);
            // Damage carries `damaged_descendants` up its ancestors (`Node::mark_damaged`).
            node.mark_damaged();
        }
        for &container in containers {
            let Some(node) = self.nodes.get(container) else { continue };
            node.mark_damaged();
            let mut ancestor = node.parent.or_else(|| node.layout_parent.get());
            self.nodes[container]
                .set_restyle_hint(style::invalidation::element::restyle_hints::RestyleHint::restyle_subtree());
            while let Some(id) = ancestor {
                let Some(node) = self.nodes.get_mut(id) else { break };
                node.clear_layout_cache();
                ancestor = node.parent.or_else(|| node.layout_parent.get());
            }
        }
    }

    /// The query containers whose queried size changed since the last call, and the ones that
    /// stopped being size containers.
    fn refresh_anchored_query_choices(&mut self) -> bool {
        let mut choices = self.nodes.anchor_query_choices.lock().unwrap();
        if choices.is_empty() { return false; }
        self.nodes.anchor_query_samples.fetch_add(choices.len() as u64, std::sync::atomic::Ordering::Relaxed);
        let mut changed = Vec::new();
        choices.retain(|id, previous| {
            let Some(node) = self.nodes.get(*id) else { return false; };
            let Some(style) = node.primary_styles() else { return false; };
            if !node.flags.is_in_document() || !style.get_box().slow_clone_container_type().contains(style::values::specified::ContainerType::ANCHORED) { return false; }
            let selected = self.anchor_last_option.get(id).and_then(|(index, _)| {
                let display = style.get_box().get_display();
                if display.is_none() || display.is_contents() || !style.clone_position().is_absolutely_positioned() { return None; }
                style.get_position().position_try_fallbacks.value.0.get(*index).cloned()
            });
            if selected != *previous { *previous = selected; changed.push(*id); }
            true
        });
        drop(choices);
        for id in &changed { self.nodes[*id].set_restyle_hint(style::invalidation::element::restyle_hints::RestyleHint::restyle_subtree()); }
        !changed.is_empty()
    }

    fn refresh_container_query_sizes(&self) -> Vec<NodeId> {
        use style::values::specified::ContainerType;
        let mut changed_ids = Vec::new();
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
                if entry.size.width.is_some() || entry.size.height.is_some() {
                    changed_ids.push(*id);
                }
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
                // pyro: less its scrollbar gutters (`Node::container_query_gutters`)
                let g = node.container_query_gutters();
                euclid::default::Size2D::new(
                    entry.axes.width.then(|| app_units::Au::from_f32_px((layout.content_box_width() - g.width).max(0.0))),
                    entry.axes.height.then(|| app_units::Au::from_f32_px((layout.content_box_height() - g.height).max(0.0))),
                )
            } else {
                Default::default()
            };
            if entry.size != size {
                entry.size = size;
                changed_ids.push(*id);
            }
            true
        });
        changed_ids
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
    pub(crate) fn resolve_transition_transforms(&mut self, root: NodeId) {
        self.resolve_transforms(root, true, ContainingTransform::default());
    }

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
        // pyro: the SVG shape an `offset-path: url()` names, for `set_transform`.
        let url_path = self.motion_url_path(node_id);
        let _url_path = MotionUrlPath::set(url_path);
        if unchanged {
            if containing.refresh {
                self.nodes[node_id].set_transform(scale as f32, &containing.context);
            }
            let node = &self.nodes[node_id];
            let location = layout.location.map(|v| v as f64 * scale);
            let overflow = self.overflow_contribution(node_id, *node.scrollable_overflow(), layout.size, scale);
            return place_overflow(node, (location.x, location.y), scale, overflow);
        }

        let (_, accumulated) = self.nodes[node_id].set_transform(scale as f32, &containing.context);
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
        let root = node_id == self.root_element().id;
        let viewport = (
            self.viewport.window_size.0 as f64 / self.viewport.scale_f64(),
            self.viewport.window_size.1 as f64 / self.viewport.scale_f64(),
        );
        let contents = {
            let node = &mut self.nodes[node_id];
            let mut context = node.transform_context_for_children(accumulated);
            // pyro: the boxes the root contains are in the initial containing block, the
            // viewport, for `offset-path` (`MotionContainingBlock`).
            if root {
                context.cb_size = viewport;
                context.cb_border = [0.0; 4];
                context.cb_padding = [0.0; 4];
                context.cb_radii = Default::default();
                context.cb_corner_exponents = [2.0; 4];
            }
            let refresh = context.gives() || node.flags.contains(NodeFlags::GIVES_3D_CONTEXT);
            node.flags.set(NodeFlags::GIVES_3D_CONTEXT, context.gives());
            ContainingTransform { context, refresh, parent: Some(owner) }
        };

        // pyro: the scrollable overflow starts at the padding box
        // (CSS Overflow 3 2.2), not the border box.
        // pyro: classic scrollbars: less its scrollbar gutters (the scrollport; Chrome's `scrollWidth`
        // of a box whose content fits is its `clientWidth`).
        let mut overflow = padding_box(&layout, scale);
        if let Some(bars) = self.nodes[node_id].classic_scrollbars() {
            let g = bars.gutters;
            overflow.x0 += g.left as f64 * scale;
            overflow.y0 += g.top as f64 * scale;
            // (Its padding stays in it when the gutters leave no room: Chrome's scrollHeight of a
            // 4px `overflow: scroll` box with 2px padding is 4, WPT cssom-view
            // `scrollWidthHeight-contain-layout`.)
            let p = layout.padding;
            overflow.x1 = (overflow.x1 - g.right as f64 * scale).max(overflow.x0 + (p.left + p.right) as f64 * scale);
            overflow.y1 = (overflow.y1 - g.bottom as f64 * scale).max(overflow.y0 + (p.top + p.bottom) as f64 * scale);
        }
        // pyro: a multi-column container's content lies in its strip: what it reaches is shown in
        // the columns (`layout::multicol`), added to the padding box below.
        let columns_base =
            self.nodes[node_id].column_fragments().is_some().then(|| std::mem::replace(&mut overflow, Rect::ZERO));
        // pyro: so does a vertical writing mode's (its columns shown by paint only).
        let vertical_columns_base = (columns_base.is_none()
            && self.nodes[node_id].vertical_column_fragments().is_some())
            .then(|| std::mem::replace(&mut overflow, Rect::ZERO));
        // pyro: each box (and line) of a multi-column container is moved into the columns that
        // show it before they are united, not their union: a spanner as wide as the container
        // reaches into a column's range only where its row starts, and the union mapped through
        // that column's offset made a scrollable overflow columns wide that is not there
        // (Chromium; WPT css-multicol `multicol-list-item-003`, `abspos-after-spanner`, ... with
        // classic scrollbars).
        let in_columns = columns_base.is_some();
        // (Its spanners, for the boxes they are in.)
        let spanners: Vec<NodeId> =
            if in_columns { self.column_spanners_of(node_id).into_iter().map(|(id, _)| id).collect() } else { Vec::new() };
        // (So with a vertical writing mode's.)
        let in_vertical_columns = vertical_columns_base.is_some();

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
            // pyro: a `line-clamp` container clips its floats at its block-end content edge: they
            // overflow no further (CSS Overflow 4 § 5.2; WPT css-overflow `line-clamp-with-floats-010`).
            let float_clip_bottom = self.nodes[node_id]
                .primary_styles()
                .is_some_and(|style| {
                    let clamp = &style.get_box().line_clamp;
                    !clamp.webkit_legacy && !clamp.max_lines.is_none()
                })
                .then(|| (layout.size.height - layout.border.bottom - layout.padding.bottom) as f64 * scale);
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
                // pyro: a box a line clamp discarded is no overflow (`layout::line_clamp`).
                if self.nodes[child_id].flags.contains(NodeFlags::HIDDEN_BY_CLAMP) {
                    continue;
                }
                let child_rect_in_self = match float_clip_bottom {
                    Some(bottom)
                        if self.nodes[child_id]
                            .primary_styles()
                            .is_some_and(|style| style.clone_float() != style::computed_values::float::T::None) =>
                    {
                        let mut rect = child_rect_in_self;
                        rect.y1 = rect.y1.min(bottom);
                        if rect.y1 <= rect.y0 {
                            continue;
                        }
                        rect
                    }
                    _ => child_rect_in_self,
                };
                // (A box carrying spanners: its own box, the spanners in their rows below; what
                // they make it reach in the strip is no column's.)
                // (A spanner inside it: `carries_spanners_of` also holds for a box whose out-of-flow
                // boxes the columns fragment, whose overflow counts as any other's.)
                let holds_spanner = spanners.iter().any(|&spanner| {
                    let mut at = self.nodes[spanner].parent;
                    while let Some(id) = at.filter(|&id| id != node_id) {
                        if id == child_id {
                            return true;
                        }
                        at = self.nodes[id].parent;
                    }
                    false
                });
                let child_rect_in_self = if in_columns && holds_spanner {
                    let l = self.nodes[child_id].final_layout();
                    let own = Rect::new(
                        l.location.x as f64 * scale,
                        l.location.y as f64 * scale,
                        (l.location.x + l.size.width) as f64 * scale,
                        (l.location.y + l.size.height) as f64 * scale,
                    );
                    self.columns_rect(node_id, own, scale)
                } else if in_columns {
                    self.nodes[node_id].column_fragments().unwrap().map_child_rect(Some(child_id), child_rect_in_self, scale)
                } else if in_vertical_columns {
                    self.vertical_columns_rect(node_id, child_id, child_rect_in_self, scale).unwrap_or(child_rect_in_self)
                } else {
                    child_rect_in_self
                };
                overflow = unite_overflow(overflow, child_rect_in_self);
                if let Some(padding) = scroll_padding {
                    let padded = self.padded_inflow_box(child_id, padding, scale);
                    let padded = if in_columns { self.nodes[node_id].column_fragments().unwrap().map_child_rect(Some(child_id), padded, scale) } else { padded };
                    overflow = unite_overflow(overflow, padded);
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
            let child_rect_in_self =
                if in_columns { self.columns_rect(node_id, child_rect_in_self, scale) } else { child_rect_in_self };
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
            let child_rect_in_self =
                if in_columns { self.columns_rect(node_id, child_rect_in_self, scale) } else { child_rect_in_self };
            overflow = unite_overflow(overflow, child_rect_in_self);
            if let Some(padding) = scroll_padding {
                let padded = self.padded_inflow_box(pseudo, padding, scale);
                let padded = if in_columns { self.columns_rect(node_id, padded, scale) } else { padded };
                overflow = unite_overflow(overflow, padded);
            }
        }

        // pyro: the lines of an inline formatting context are
        // overflow too (a nowrap name in a 100px `truncate` cell scrolls), and
        // a scroll container adds its end padding after them.
        if let Some((lines, true)) = self.line_bounds(node_id, &layout, scale) {
            overflow = overflow.union(if in_columns { self.columns_rect(node_id, lines, scale) } else { lines });
        }
        // pyro: a vertical writing mode's columns (shown by paint only) are its overflow too.
        if let Some(base) = vertical_columns_base {
            // (Its in-flow boxes already in their columns, above; the rest mapped here.)
            overflow = if overflow.width() > 0.0 || overflow.height() > 0.0 { base.union(overflow) } else { base };
        }
        // (And the spanners, in their rows: `column_spanners_of` gives their parents' places.)
        if in_columns && let Some(columns) = self.nodes[node_id].column_fragments() {
            for &child_id in columns.first_after_spanner.keys() {
                let child = &self.nodes[child_id];
                if child.layout_parent.get() == Some(node_id) { continue }
                let mut rect = self.overflow_contribution(child_id, *child.scrollable_overflow(), child.final_layout().size, scale);
                let mut current = child;
                loop {
                    let location = current.final_layout().location;
                    rect = place_overflow(current, (location.x as f64 * scale, location.y as f64 * scale), scale, rect);
                    let Some(parent) = current.layout_parent.get().filter(|&parent| parent != node_id) else { break };
                    current = &self.nodes[parent];
                }
                overflow = unite_overflow(overflow, columns.map_child_rect(Some(child_id), rect, scale));
            }
            let rows: Vec<_> = columns.fragments.iter().filter(|f| f.spanner).copied().collect();
            for ((spanner, at), row) in self.column_spanners_of(node_id).into_iter().zip(rows) {
                let l = self.nodes[spanner].final_layout();
                let (x, y) = (at.x + l.location.x + row.offset.x, at.y + l.location.y + row.offset.y);
                let rect = Rect::new(x as f64 * scale, y as f64 * scale, (x + l.size.width) as f64 * scale, (y + l.size.height) as f64 * scale);
                overflow = unite_overflow(overflow, rect);
            }
        }
        if let Some(base) = columns_base {
            // (Each already in its columns, above.)
            overflow = if overflow.width() > 0.0 || overflow.height() > 0.0 { base.union(overflow) } else { base };
            if let Some(columns) = self.nodes[node_id].column_fragments() {
                for f in columns.fragments.iter().filter(|f| !f.spanner) {
                    overflow.x0 = overflow.x0.min(f.x as f64 * scale);
                    overflow.x1 = overflow.x1.max((f.x + f.width) as f64 * scale);
                    if scroll_padding.is_some() {
                        overflow.y0 = overflow.y0.min(f.top as f64 * scale);
                        overflow.y1 = overflow.y1.max((f.top + f.height) as f64 * scale);
                    }
                }
            }
        }
        // pyro: a scroll container's grid reaches as far as its tracks and the padding after them,
        // empty tracks too (css-grid-2 § 5.3; Taffy's grid overflow, in the padding-box frame;
        // WPT css-grid `grid-overflow-001`).
        if scroll_padding.is_some() && self.nodes[node_id].taffy_display() == taffy::Display::Grid {
            let grid = layout.scrollable_overflow_rect.rect();
            // (Taffy's frame is the padding box: the start-edge classic gutters the grid moves its
            // content box in by are in it, as padding is. An rtl grid's gutter at its inline end's
            // physical start edge (the left one, the top one in a vertical writing mode) is not:
            // its frame starts after it. Chrome: WPT css-grid `grid-overflow-001`, and
            // `scrollbar-gutter: stable both-edges` grids, scrollWidth 150 for 150px of content.)
            let (gl, gt) = match self.nodes[node_id].primary_styles() {
                Some(s) if !s.writing_mode.is_bidi_ltr() => {
                    let g = self.nodes[node_id].gutter_insets();
                    if s.writing_mode.is_vertical() { (0.0, g.top) } else { (g.left, 0.0) }
                }
                _ => (0.0, 0.0),
            };
            let (bl, bt) = ((layout.border.left + gl) as f64, (layout.border.top + gt) as f64);
            overflow = overflow.union(Rect::new(
                (grid.left as f64 + bl) * scale,
                (grid.top as f64 + bt) * scale,
                (grid.right as f64 + bl) * scale,
                (grid.bottom as f64 + bt) * scale,
            ));
        }

        // pyro: a right-to-left item's outside marker, painted after its border box's right edge
        // (blitz-paint's outside markers), is its overflow there. Chrome: WPT css-lists
        // `list-style-type-string-003`, a long rtl string marker makes the document 1137px wide.
        if scroll_padding.is_none() {
            if let Some((width, height)) = self.outside_marker_after_right_edge(node_id) {
                let x = layout.size.width as f64;
                let top = (layout.border.top + layout.padding.top) as f64;
                overflow = overflow.union(Rect::new(x * scale, top * scale, (x + width) * scale, (top + height) * scale));
            }
        }
        *self.nodes[node_id].scrollable_overflow_mut() = overflow;
        *self.nodes[node_id].layout_children.get_mut() = layout_children;

        let scaled_x = layout.location.x as f64 * scale;
        let scaled_y = layout.location.y as f64 * scale;

        let contribution = self.overflow_contribution(node_id, overflow, layout.size, scale);
        place_overflow(&self.nodes[node_id], (scaled_x, scaled_y), scale, contribution)
    }

    /// pyro: an in-flow child's border box in its scroll container (device px), grown on each
    /// side by the container's `padding` and the child's margin there (not below zero), the
    /// child's part of the container's scrollable overflow with the end padding after it. Its
    /// transform is left out, as Blink and Taffy leave it out of the in-flow bounds.
    fn padded_inflow_box(&self, child_id: NodeId, padding: taffy::Rect<f32>, scale: f64) -> Rect {
        let child = self.transform_pass_layout(child_id);
        // pyro: the bottom margin as it collapsed with those of the child's last in-flow
        // descendants it collapses through (Chrome: WPT css-overflow
        // `overflow-negative-margin-dynamic`, a -20px margin through a 10px one: -10px).
        let mut child = child;
        child.margin.bottom = self.collapsed_end_margin(child_id, child.margin.bottom, 0);
        let pad = |padding: f32, margin: f32| (padding + margin).max(0.0) as f64;
        // pyro: at its place before its relative offset (Chrome's in-flow bounds: an item moved
        // 10px past its scroll container's content edge adds no padding after that; WPT
        // css-flexbox overflow-area-003, scrollWidth 80, not 90).
        let offset = self.nodes[child_id]
            .primary_styles()
            .filter(|s| s.clone_position() == style::computed_values::position::T::Relative && !s.writing_mode.is_vertical())
            .map(|s| {
                let parent = self.nodes[child_id].layout_parent.get().or(self.nodes[child_id].parent);
                let cb = parent.map_or(taffy::Size { width: 0.0, height: 0.0 }, |p| {
                    let l = self.nodes[p].final_layout();
                    taffy::Size { width: l.content_box_width(), height: l.content_box_height() }
                });
                let rtl = parent.and_then(|p| self.nodes[p].primary_styles()).is_some_and(|ps| !ps.writing_mode.is_bidi_ltr());
                crate::layout::inline::relative::relative_offset(&s, cb, rtl)
            })
            .unwrap_or(taffy::Point { x: 0.0, y: 0.0 });
        let (x, y) = ((child.location.x - offset.x) as f64, (child.location.y - offset.y) as f64);
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

    /// pyro: `margin` (`node_id`'s bottom margin) collapsed with the bottom margin of its last
    /// in-flow child when they adjoin (CSS 2.1 § 8.3.1: no bottom border or padding, an `auto`
    /// height, no formatting context of its own, horizontal writing mode), recursively.
    fn collapsed_end_margin(&self, node_id: NodeId, margin: f32, depth: usize) -> f32 {
        use style::values::computed::Overflow;
        let node = &self.nodes[node_id];
        let Some(style) = node.primary_styles() else { return margin };
        let layout = node.unrounded_layout();
        let display = style.clone_display();
        let adjoins = depth < 32
            && !style.writing_mode.is_vertical()
            && layout.border.bottom == 0.0
            && layout.padding.bottom == 0.0
            && style.get_position().height.is_auto()
            && display.inside() == style::values::specified::box_::DisplayInside::Flow
            && display.outside() == style::values::specified::box_::DisplayOutside::Block
            && style.clone_overflow_y() == Overflow::Visible
            && style.clone_overflow_x() == Overflow::Visible
            && style.clone_float() == style::values::computed::Float::None
            && !node.is_out_of_flow()
            && !node.flags.is_inline_root();
        if !adjoins {
            return margin;
        }
        let last = node.layout_children.borrow().as_ref().and_then(|children| {
            children.iter().rev().copied().find(|&c| {
                let child = &self.nodes[c];
                child.is_element() && !child.is_out_of_flow() && child.primary_styles().is_some_and(|s| {
                    s.clone_float() == style::values::computed::Float::None
                })
            })
        });
        let Some(last) = last else { return margin };
        let inner = self.collapsed_end_margin(last, self.nodes[last].unrounded_layout().margin.bottom, depth + 1);
        // The last child's margin box must end at the parent's content end for them to adjoin.
        let l = self.nodes[last].unrounded_layout();
        if (l.location.y + l.size.height - (layout.size.height - layout.border.bottom - layout.padding.bottom)).abs() > 0.5 {
            return margin;
        }
        let (pos, neg) = (margin.max(inner).max(0.0), margin.min(inner).min(0.0));
        pos + neg
    }

    /// pyro: the width and height (CSS px) of the outside marker of `node_id`, a list item in a
    /// horizontal right-to-left writing mode, which paint places after its border box's right
    /// edge, with its padding (8px) for a symbol.
    fn outside_marker_after_right_edge(&self, node_id: NodeId) -> Option<(f64, f64)> {
        let element = self.nodes[node_id].element_data()?;
        let item = element.list_item_data.as_deref()?;
        let crate::node::ListItemLayoutPosition::Outside(marker) = &item.position else { return None };
        let style = self.nodes[node_id].primary_styles()?;
        if style.writing_mode.is_bidi_ltr() || style.writing_mode.is_vertical() {
            return None;
        }
        let scale = marker.scale().max(f32::EPSILON) as f64;
        let padding = if matches!(item.marker, crate::node::Marker::Char(_)) { 8.0 } else { 0.0 };
        let width = marker.full_width() as f64 / scale + padding;
        (width > 0.0).then(|| (width, marker.height() as f64 / scale))
    }

    /// pyro: whether `node_id` is a box of combined text in a vertical line, whose text paint
    /// compresses into it (blitz-paint's `combined_text_compression`).
    fn is_compressed_combined_text(&self, node_id: NodeId) -> bool {
        let node = &self.nodes[node_id];
        let Some(style) = node.primary_styles() else { return false };
        if !style.flags.contains(style::computed_value_flags::ComputedValueFlags::IS_TEXT_COMBINED) || style.writing_mode.is_vertical() {
            return false;
        }
        node.layout_parent
            .get()
            .and_then(|parent| self.nodes[parent].primary_styles().map(|p| p.writing_mode))
            .is_some_and(|wm| wm.is_vertical() && !wm.is_sideways())
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
        // pyro: the lines a line clamp discarded are no overflow (`layout::line_clamp`).
        let kept = inline.and_then(|t| t.clamp.as_ref()).map_or(usize::MAX, |clamp| clamp.lines as usize);
        // pyro: the clamp line of a clamp that discarded lines ends at its block ellipsis, which
        // goes inside the line box: what it hides past it is no overflow (Chrome: WPT css-overflow
        // `line-clamp/block-ellipsis-013`, a `white-space: pre` line wider than the viewport).
        let clamp_line = (kept != usize::MAX && text_layout.len() > kept)
            .then(|| kept.saturating_sub(1));
        let content_width = (l.size.width - l.border.left - l.border.right - l.padding.left - l.padding.right).max(0.0) as f64 * scale;
        let width = text_layout
            .lines()
            .take(kept)
            .enumerate()
            .map(|(i, line)| {
                let m = line.metrics();
                let w = (m.offset + m.advance - m.hanging_advance) as f64;
                if Some(i) == clamp_line { w.min(content_width) } else { w }
            })
            .fold(0.0, f64::max);
        // pyro: combined text (`text-combine-upright` in a vertical line, a horizontal box one em
        // square) is compressed to fit its box (blitz-paint's `combined_text_compression`): it
        // reaches no further than the box (Chrome: WPT css-writing-modes
        // `text-combine-upright-compression-007`, nine 72px characters combined overflow nothing).
        let width = if self.is_compressed_combined_text(node_id) { width.min(content_width) } else { width };
        // pyro: where the lines start: an overflowing right-to-left line is right-aligned and
        // overflows its inline start (parley's negative offset; its hanging trailing whitespace
        // is on its left), which is scrollable overflow (Chrome: `direction: rtl` 4ch of Ahem in
        // a 35px `overflow: auto` box has scrollWidth 40).
        let start = text_layout
            .lines()
            .take(kept)
            .map(|line| {
                let m = line.metrics();
                (m.offset + if line.is_rtl() { m.hanging_advance } else { 0.0 }) as f64
            })
            .fold(0.0, f64::min);
        // The empty line parley adds after a final hard break (`<br>`)
        // is not content.
        let height = text_layout
            .lines()
            .take(kept)
            .filter(|line| !line.text_range().is_empty())
            .map(|line| line.metrics().block_max_coord as f64)
            .fold(0.0, f64::max);
        let g = self.nodes[node_id].gutter_insets();
        let x0 = (l.border.left + l.padding.left + g.left) as f64 * scale;
        let y0 = (l.border.top + l.padding.top + g.top + block_offset) as f64 * scale;
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
            None => Rect::new(x0 + start, y0, x0 + width + end_x, y0 + height + end_y),
            Some(block_lr) => {
                let offset = block_offset as f64 * scale;
                let top = y0 - offset;
                let (b0, b1) = if block_lr {
                    (x0 + offset, x0 + offset + height + end_x)
                } else {
                    let start = (l.size.width - l.border.right - l.padding.right - g.right) as f64 * scale - offset;
                    (start - height - end_x, start)
                };
                // pyro: `sideways-lr` lines run up from the content box's bottom, their end padding
                // (a scroll container's) the top one.
                if stylo_taffy::WritingModeExt::line_left_is_bottom(self.nodes[node_id].layout_data().writing_mode) {
                    let bottom = (l.size.height - l.border.bottom - l.padding.bottom - g.bottom) as f64 * scale;
                    let end_top = if scrolls { l.padding.top as f64 * scale } else { 0.0 };
                    return Some((Rect::new(b0, bottom - width - end_top, b1, bottom - start), width > 0.0 || height > 0.0));
                }
                Rect::new(b0, top + start, b1, top + width + end_y)
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
            // pyro: layout containment makes the box's overflowing contents ink overflow, and paint
            // containment clips them: neither reaches an ancestor's scrollable overflow (CSS Contain
            // 2 § 3.2, § 3.4; Taffy's `compute_scrollable_overflow_contribution` agrees). WPT
            // css-contain `contain-layout-ink-overflow-*`.
            use style::values::specified::box_::Contain;
            let contain = style.get_box().used_containment();
            // pyro: paint containment alone clips as `overflow: clip` does, at its
            // `overflow-clip-margin` (below; WPT css-overflow `overflow-clip-margin-005`).
            let inline_flow = {
                use style::values::specified::box_::{DisplayInside, DisplayOutside};
                let display = style.clone_display();
                display.outside() == DisplayOutside::Inline && display.inside() == DisplayInside::Flow
            };
            let paint_only = contain.contains(Contain::PAINT) && !contain.contains(Contain::LAYOUT) && !inline_flow;
            if contain.intersects(Contain::LAYOUT | Contain::PAINT) && !paint_only && !inline_flow {
                return border_box;
            }
            let (ox, oy) = (style.clone_overflow_x(), style.clone_overflow_y());
            let as_clip = |o: Overflow| if paint_only && o == Overflow::Visible { Overflow::Clip } else { o };
            let (ox, oy) = (as_clip(ox), as_clip(oy));
            let (x, y) = (ox != Overflow::Visible, oy != Overflow::Visible);
            if (x || y) && !self.body_overflow_goes_to_viewport(node_id) {
                // pyro: an `overflow: clip` axis shows its overflow up to its overflow clip edge
                // (`overflow-clip-margin`), which is part of what it contributes (Chrome:
                // WPT css-overflow `overflow-clip-margin-002`).
                let edge = (ox == Overflow::Clip || oy == Overflow::Clip).then(|| {
                    use style::values::generics::box_::OverflowClipMarginBox;
                    let layout = self.nodes[node_id].final_layout();
                    let clip_margin = &style.get_margin().overflow_clip_margin;
                    let (b, p) = (layout.border, layout.padding);
                    let inset = match clip_margin.visual_box {
                        OverflowClipMarginBox::BorderBox => taffy::Rect { left: 0.0, right: 0.0, top: 0.0, bottom: 0.0 },
                        OverflowClipMarginBox::PaddingBox => b,
                        OverflowClipMarginBox::ContentBox => b + p,
                    };
                    let m = clip_margin.offset.px();
                    Rect::new(
                        (inset.left - m) as f64 * scale,
                        (inset.top - m) as f64 * scale,
                        (size.width - inset.right + m) as f64 * scale,
                        (size.height - inset.bottom + m) as f64 * scale,
                    )
                });
                if x {
                    let (x0, x1) = match edge.filter(|_| ox == Overflow::Clip) {
                        Some(edge) => (overflow.x0.max(edge.x0).min(border_box.x0), overflow.x1.min(edge.x1).max(border_box.x1)),
                        None => (border_box.x0, border_box.x1),
                    };
                    (out.x0, out.x1) = (x0, x1);
                }
                if y {
                    let (y0, y1) = match edge.filter(|_| oy == Overflow::Clip) {
                        Some(edge) => (overflow.y0.max(edge.y0).min(border_box.y0), overflow.y1.min(edge.y1).max(border_box.y1)),
                        None => (border_box.y0, border_box.y1),
                    };
                    (out.y0, out.y1) = (y0, y1);
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
            // pyro: the boxes of the root's combined text (`layout::combined_text`).
            crate::layout::combined_text::prepare(self, task.node_id);
            // pyro: the first line's style node first: a first letter on it inherits from it.
            layout.first_line = match self.first_line_styles {
                true => crate::layout::first_line::style_node_for(self, task.node_id),
                false => None,
            };
            layout.first_line_styles = match layout.first_line {
                Some(first_line) => crate::layout::first_line::layout_styles(self, task.node_id, first_line),
                None => Vec::new(),
            };
            layout.first_letter = match self.first_letter_styles {
                true => crate::layout::first_letter::split(self, task.node_id, layout.first_line),
                false => None,
            };
            // pyro: an initial letter box is laid out by the root's first line: it is the first
            // of the root's layout children, with its own inline layout.
            if let Some(split) = layout.first_letter.as_ref().filter(|split| split.initial.is_some() || split.floated) {
                crate::layout::first_letter::build_initial_letter_box(self, task.node_id, split.style_node);
            }
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
                        first_line_styles: std::mem::take(&mut layout.first_line_styles),
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
        self.anchor_flush_choices.clear();
        self.resolve_layout_in_flush();
        // pyro: a layout outside `resolve` (which bumps once after all its passes) changes geometry.
        self.nodes.bump_geometry_generation();
    }

    fn resolve_layout_in_flush(&mut self) {
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
        self.anchor_index.get_mut().clear();
        self.layout_ran = true;
        phase_profile::laid_out();
        self.mark_viewport_body();

        let root_element_id = crate::taffy_node_id(self.root_element().id);

        // println!("\n\nRESOLVE LAYOUT\n===========\n");

        let foreign_objects = self.foreign_object_roots();
        // pyro: classic scrollbars (`stylo_taffy::scrollbars`): the viewport's scrollbar gutters
        // make the initial containing block smaller, and the root starts after the left and top
        // ones. A pass that turns an `overflow: auto` box's or the viewport's scrollbar on or off
        // lays the document out again (as Chrome does), a few times at most.
        let classic = self.nodes.classic_scrollbars;
        if classic {
            self.viewport_scrollbars = self.viewport_scrollbars_for(self.viewport_scrollbars.shown);
        } else {
            self.viewport_scrollbars = Default::default();
        }
        let (full_available_space, full_initial_containing_block) = (available_space, initial_containing_block);
        let mut passes = 0;
        let mut state = loop {
            passes += 1;
            let gutters = self.viewport_scrollbars.gutters;
            let shrink = |space: taffy::Size<AvailableSpace>| taffy::Size {
                width: match space.width {
                    AvailableSpace::Definite(w) => AvailableSpace::Definite((w - gutters.left - gutters.right).max(0.0)),
                    other => other,
                },
                height: match space.height {
                    AvailableSpace::Definite(h) => AvailableSpace::Definite((h - gutters.top - gutters.bottom).max(0.0)),
                    other => other,
                },
            };
            let (available_space, initial_containing_block) =
                (shrink(full_available_space), shrink(full_initial_containing_block));
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
            // (In the root's axes: a vertical root's inline axis is physical y, and its block
            // axis runs from the right edge in `vertical-rl`.)
            let root_wm = state.layout_wm_of(crate::dom_node_id(root_element_id));
            let (shift_x, shift_y) = if !root_wm.is_vertical() {
                (gutters.left, gutters.top)
            } else if root_wm.is_vertical_lr() {
                (gutters.top, gutters.left)
            } else {
                (gutters.top, gutters.right)
            };
            if shift_x != 0.0 || shift_y != 0.0 {
                state.nodes[crate::dom_node_id(root_element_id)].update_unrounded_layout(|layout| {
                    layout.location.x += shift_x;
                    layout.location.y += shift_y;
                });
            }
            #[cfg(feature = "writing-mode")]
            state.physicalise_and_round_layout(root_element_id);
            #[cfg(not(feature = "writing-mode"))]
            taffy::round_layout(&mut state, root_element_id);
            let _ = passes;
            break state;
        };
        // The foreign objects' fallback viewport, in the root's axes as before.
        #[cfg(feature = "writing-mode")]
        let initial_containing_block = if state.layout_wm_of(crate::dom_node_id(root_element_id)).is_vertical() {
            initial_containing_block.transpose()
        } else {
            initial_containing_block
        };

        // pyro: each `<foreignObject>` is laid out as a root in a box of its `width` and
        // `height` (presentation attributes, `stylo.rs`), placed at its origin; blitz-paint draws
        // it in the `<svg>`'s user space (`draw_svg`).
        for fo in foreign_objects {
            let fo = crate::taffy_node_id(fo);
            #[cfg(feature = "writing-mode")]
            {
                state.layout_wm = state.layout_wm_of(crate::dom_node_id(fo));
            }
            // pyro: its percentage `width`/`height` resolve against the nearest `<svg>` viewport (WPT
            // css-transforms `view-box-mutation-003`).
            let viewport = state.foreign_object_viewport(crate::dom_node_id(fo)).map_or(initial_containing_block, |(w, h)| {
                taffy::Size { width: taffy::AvailableSpace::Definite(w), height: taffy::AvailableSpace::Definite(h) }
            });
            taffy::compute_root_layout_in(&mut state, fo, viewport, viewport);
            #[cfg(feature = "writing-mode")]
            state.physicalise_and_round_layout(fo);
            #[cfg(not(feature = "writing-mode"))]
            taffy::round_layout(&mut state, fo);
        }

        // println!("\n\n");
        // taffy::print_tree(self, root_node_id)
        self.anchor_scroll_dirty.clear();
        // pyro: the anchor caches memoised during this pass are stale for the next one (another pass
        // of the same flush); the geometry generation itself advances once per resolve, not per pass.
        self.nodes.clear_anchor_caches();
    }

    /// pyro: the viewport's overflow (CSS Overflow 3 § 3.3): the root's, or the `<body>`'s when the
    /// root's is `visible` and the body's goes to the viewport; `visible` is used as `auto` and `clip`
    /// as `hidden` (physical: x is `overflow-x`).
    pub fn viewport_overflow(&self) -> taffy::Point<style::values::computed::Overflow> {
        use style::values::computed::Overflow;
        let auto = taffy::Point { x: Overflow::Auto, y: Overflow::Auto };
        // pyro: the viewport of an `<iframe scrolling=no>` scrolls only programmatically.
        if self.frame_no_scrollbars {
            return taffy::Point { x: Overflow::Hidden, y: Overflow::Hidden };
        }
        let Some(root) = self.try_root_element() else { return auto };
        let Some(html) = root.primary_styles() else { return auto };
        let mut overflow = (html.clone_overflow_x(), html.clone_overflow_y());
        if overflow == (Overflow::Visible, Overflow::Visible) {
            let body = root.children.iter().copied().find(|&child| self.body_overflow_goes_to_viewport(child));
            if let Some(body) = body.and_then(|body| self.nodes[body].primary_styles()) {
                overflow = (body.clone_overflow_x(), body.clone_overflow_y());
            }
        }
        let used = |overflow: Overflow| match overflow {
            Overflow::Visible => Overflow::Auto,
            Overflow::Clip => Overflow::Hidden,
            other => other,
        };
        taffy::Point { x: used(overflow.0), y: used(overflow.1) }
    }

    /// pyro: lays this document out with classic scrollbars (`true`) or overlay scrollbars, whatever
    /// the process's mode (`stylo_taffy::scrollbars`): for tests and for embedders.
    pub fn set_classic_scrollbars(&mut self, classic: bool) {
        if self.nodes.classic_scrollbars == classic {
            return;
        }
        self.nodes.classic_scrollbars = classic;
        for (_, node) in self.nodes.iter_mut() {
            node.flags.remove(NodeFlags::AUTO_SCROLLBAR_X | NodeFlags::AUTO_SCROLLBAR_Y);
            node.clear_layout_cache();
        }
        self.mark_all_damaged();
        self.last_layout_space = None;
    }

    /// pyro: classic scrollbars: sets `viewport_unit_inset` from the root's style (the gutters that
    /// show whatever the document's size); whether it changed (queued as a device change).
    fn update_viewport_unit_inset(&mut self) -> bool {
        let inset = if self.nodes.classic_scrollbars && self.try_root_element().is_none_or(|root| root.custom_scrollbar_size().is_none()) {
            let g = self.viewport_scrollbars_for(taffy::Point { x: false, y: false }).gutters;
            let total = stylo_taffy::scrollbars::Gutters { edges: g }.total();
            (total.width, total.height)
        } else {
            (0.0, 0.0)
        };
        if inset == self.viewport_unit_inset {
            return false;
        }
        self.viewport_unit_inset = inset;
        self.queue_device_changes(crate::stylo_device::DeviceChanges::VIEWPORT_SIZE);
        true
    }

    /// pyro: classic scrollbars: the viewport's gutters when the scrollbars `shown` show (the root's
    /// `scrollbar-gutter` and `scrollbar-width` are the viewport's).
    fn viewport_scrollbars_for(&self, shown: taffy::Point<bool>) -> crate::ViewportScrollbars {
        use style::values::computed::ScrollbarGutter;
        let Some(html) = self.try_root_element().and_then(|root| root.primary_styles()) else {
            return Default::default();
        };
        let thickness = if self.nodes.classic_scrollbars {
            stylo_taffy::scrollbars::thickness_in(stylo_taffy::scrollbars::ScrollbarMode::Classic, html.clone_scrollbar_width())
        } else {
            0.0
        };
        let overflow = self.viewport_overflow();
        use style::values::computed::Overflow;
        let shown = taffy::Point {
            x: overflow.x == Overflow::Scroll || (overflow.x == Overflow::Auto && shown.x),
            y: overflow.y == Overflow::Scroll || (overflow.y == Overflow::Auto && shown.y),
        };
        let gutter = html.get_box().scrollbar_gutter;
        let viewport = self.stylist.device().au_viewport_size();
        let thickness = self.root_element().custom_scrollbar_size().unwrap_or(taffy::Size { width: thickness, height: thickness });
        let mut inputs = stylo_taffy::scrollbars::GutterInputs {
            thickness,
            overflow,
            auto_shows: shown,
            stable: gutter.contains(ScrollbarGutter::STABLE),
            both_edges: gutter.contains(ScrollbarGutter::BOTH_EDGES),
            vertical: html.writing_mode.is_vertical(),
            // The viewport's vertical scrollbar is on the right whatever the direction (Chrome).
            rtl: false,
        };
        let thickness = inputs.bounded_thickness(taffy::Size { width: viewport.width.to_f32_px(), height: viewport.height.to_f32_px() });
        inputs.thickness = thickness;
        let shown = taffy::Point { x: shown.x && thickness.height > 0.0, y: shown.y && thickness.width > 0.0 };
        crate::ViewportScrollbars { gutters: inputs.gutters().edges, shown, thickness }
    }

    /// pyro: classic scrollbars: after a layout pass, the viewport's `overflow: auto` scrollbars show
    /// when the document overflows its scrollport (and stop showing when it no longer does with
    /// them). Whether the gutters changed (the document is laid out again).
    fn update_viewport_scrollbars(&mut self, may_remove: bool) -> bool {
        let Some(root) = self.try_root_element() else { return false };
        let size = self.stylist.device().au_viewport_size();
        let (width, height) = (size.width.to_f32_px(), size.height.to_f32_px());
        let port = self.viewport_scrollbars.scrollport(width, height);
        let layout = root.final_layout();
        // The document's extent: the root's margin box and its scrollable overflow (resolved,
        // device px).
        let scale = self.viewport.scale_f64();
        let overflow = root.scrollable_overflow().scale_from_origin(1.0 / scale);
        let right = ((layout.location.x + layout.size.width + layout.margin.right) as f64)
            .max(layout.location.x as f64 + overflow.x1);
        let bottom = ((layout.location.y + layout.size.height + layout.margin.bottom) as f64)
            .max(layout.location.y as f64 + overflow.y1);
        // pyro: where the document's scroll origin is on the right (`direction: rtl`, or blocks
        // flowing leftward in `vertical-rl`), what lies left of the viewport is scrolled to, and
        // what lies right of it is not (CSS Overflow 3 § 2.2; WPT css-writing-modes
        // `wm-propagation-svg-root-scrollbar`).
        let leftward = self.viewport_scrolls_leftward();
        let left = ((layout.location.x - layout.margin.left) as f64).min(layout.location.x as f64 + overflow.x0);
        let had = self.viewport_scrollbars.shown;
        let overflows_x = if leftward { left < port.x0 - 0.5 } else { right > port.x1 + 0.5 };
        let shown = taffy::Point {
            x: overflows_x || (!may_remove && had.x),
            y: bottom > port.y1 + 0.5 || (!may_remove && had.y),
        };
        let root_id = root.id;
        let next = self.viewport_scrollbars_for(shown);
        let changed = next.gutters != self.viewport_scrollbars.gutters;
        self.viewport_scrollbars = next;
        if changed {
            self.nodes[root_id].clear_layout_cache();
        }
        changed
    }

    /// pyro: classic scrollbars: after a layout pass, an `overflow: auto` box whose content overflows
    /// shows its scrollbar (`NodeFlags::AUTO_SCROLLBAR_X`/`_Y`), which takes a gutter; one whose content
    /// no longer overflows with it stops showing it. A box whose scrollbars changed is laid out again
    /// with its ancestors. Whether any changed.
    fn update_auto_scrollbars(&mut self, may_remove: bool) -> bool {
        use style::values::computed::Overflow;
        use style::values::computed::ui::ScrollbarWidth;
        let scale = self.viewport.scale_f64();
        let mut changed = Vec::new();
        for (id, node) in self.nodes.iter() {
            let had = node.flags & (NodeFlags::AUTO_SCROLLBAR_X | NodeFlags::AUTO_SCROLLBAR_Y);
            let Some(style) = node.primary_styles() else {
                if !had.is_empty() {
                    changed.push((id, NodeFlags::empty(), None));
                }
                continue;
            };
            let (x, y) = (style.clone_overflow_x(), style.clone_overflow_y());
            let old_limit = node.layout_data().scrollbar_size_limit;
            let limit = if let Some(inputs) = node.classic_gutter_inputs() {
                let layout = node.unrounded_layout();
                let available = (layout.size - layout.border.sum_axes()).map(|length| length.max(0.0));
                let bounded = inputs.bounded_thickness(available);
                (bounded != inputs.thickness).then_some(bounded)
            } else { None };
            if x != Overflow::Auto && y != Overflow::Auto && had.is_empty() && limit == old_limit {
                continue;
            }
            let eligible = style.clone_scrollbar_width() != ScrollbarWidth::None
                && !node.flags.contains(NodeFlags::OVERFLOW_TO_VIEWPORT)
                && !node.is_root_element()
                && node.is_element();
            let mut want = NodeFlags::empty();
            if eligible {
                // The scrollport (the padding box less the gutters) against the resolved
                // scrollable overflow, which starts at it (`resolve_transforms`), device px.
                let layout = node.final_layout();
                let o = *node.scrollable_overflow();
                let g = node.classic_scrollbars().map(|bars| bars.gutters);
                let gap = |f: fn(&taffy::Rect<f32>) -> f32| g.as_ref().map_or(0.0, f) as f64;
                let b = layout.border;
                let port = kurbo::Rect::new(
                    (b.left as f64 + gap(|g| g.left)) * scale,
                    (b.top as f64 + gap(|g| g.top)) * scale,
                    ((layout.size.width - b.right) as f64 - gap(|g| g.right)) * scale,
                    ((layout.size.height - b.bottom) as f64 - gap(|g| g.bottom)) * scale,
                );
                let slack = 0.5 * scale;
                // Only the end of the scroll origin's axis is reachable (the start in a reversed one).
                let reversed = layout.scrollable_overflow_rect.is_reversed();
                let past_x = if reversed.x { o.x0 < port.x0 - slack } else { o.x1 > port.x1 + slack };
                let past_y = if reversed.y { o.y0 < port.y0 - slack } else { o.y1 > port.y1 + slack };
                // A scrollbar needs room across it: a box of no height shows no horizontal one, of
                // no width no vertical one (Chrome: WPT css-overflow `scrollbar-empty-002`).
                let room_x = layout.size.height - b.top - b.bottom > 0.5;
                let room_y = layout.size.width - b.left - b.right > 0.5;
                if x == Overflow::Auto && past_x && room_x {
                    want |= NodeFlags::AUTO_SCROLLBAR_X;
                }
                if y == Overflow::Auto && past_y && room_y {
                    want |= NodeFlags::AUTO_SCROLLBAR_Y;
                }
            }
            if !may_remove {
                want |= had;
            }
            if want != had || limit != old_limit {
                changed.push((id, want, limit));
            }
        }
        // Each changed box and its ancestors are laid out again, each once: a walk up stops at an
        // ancestor an earlier walk cleared (nested scrollers that all change cost the tree's size,
        // not its depth squared).
        let mut cleared = std::collections::HashSet::new();
        for &(id, want, limit) in &changed {
            let node = &mut self.nodes[id];
            node.layout_data_mut().scrollbar_size_limit = limit;
            node.flags.remove(NodeFlags::AUTO_SCROLLBAR_X | NodeFlags::AUTO_SCROLLBAR_Y);
            node.flags.insert(want);
            let mut current = Some(id);
            while let Some(id) = current {
                if !cleared.insert(id) {
                    break;
                }
                // (A table measures a changed cell again: its column widths.)
                self.note_changed_table_cell(id, true);
                let node = &mut self.nodes[id];
                node.clear_layout_cache();
                // (Up the boxes that lay it out: an inline-block's anonymous block too, which its
                // DOM parent chain skips; css-flexbox overflow-auto-006 measures its scrollbar.)
                let (layout_parent, parent) = (node.layout_parent.get(), node.parent);
                current = layout_parent.filter(|&p| self.nodes.contains_key(p)).or(parent);
            }
        }
        !changed.is_empty()
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
                context: TransformContext {
                    scroll: self.context.scroll,
                    cb_size: self.context.cb_size,
                    cb_border: self.context.cb_border,
                    cb_padding: self.context.cb_padding,
                    cb_radii: self.context.cb_radii,
                    cb_corner_exponents: self.context.cb_corner_exponents,
                    ..Default::default()
                },
                ..*self
            }
        }
    }
}

/// pyro: sets `stylo_to_kurbo::MOTION_URL_PATH` while it lives.
struct MotionUrlPath(Option<kurbo::BezPath>);

impl MotionUrlPath {
    fn set(path: Option<kurbo::BezPath>) -> Self {
        MotionUrlPath(crate::stylo_to_kurbo::MOTION_URL_PATH.with(|p| p.replace(path)))
    }
}

impl Drop for MotionUrlPath {
    fn drop(&mut self) {
        let previous = self.0.take();
        crate::stylo_to_kurbo::MOTION_URL_PATH.with(|p| *p.borrow_mut() = previous);
    }
}

impl BaseDocument {
    /// pyro: the geometry, in its user units, of the SVG shape element (`path`, `rect`, `circle`,
    /// `ellipse`, `line`, `polyline`, `polygon`) that `node_id`'s `offset-path: url()` names in
    /// this document; `None` for no `url()` or no such element.
    fn motion_url_path(&self, node_id: NodeId) -> Option<kurbo::BezPath> {
        use kurbo::Shape as _;
        use style::values::generics::motion::{GenericOffsetPath, GenericOffsetPathFunction};
        let styles = self.nodes[node_id].primary_styles()?;
        let GenericOffsetPath::OffsetPath { path, .. } = &styles.get_box().offset_path else { return None };
        let GenericOffsetPathFunction::Url(url) = &**path else { return None };
        let fragment = url.url()?.fragment()?.to_owned();
        let el = self.nodes.get(self.get_element_by_id(&fragment)?)?.element_data()?;
        if el.name.ns != markup5ever::ns!(svg) {
            return None;
        }
        let num = |name: &str| {
            el.attr(markup5ever::LocalName::from(name))
                .and_then(|v| v.trim().trim_end_matches("px").parse::<f64>().ok())
                .unwrap_or(0.0)
        };
        let points = |close: bool| {
            let text = el.attr(markup5ever::local_name!("points"))?;
            let numbers: Vec<f64> = text
                .split(|c: char| c.is_whitespace() || c == ',')
                .filter(|t| !t.is_empty())
                .map_while(|t| t.parse().ok())
                .collect();
            let mut path = kurbo::BezPath::new();
            for (i, pair) in numbers.chunks_exact(2).enumerate() {
                let p = (pair[0], pair[1]);
                if i == 0 { path.move_to(p) } else { path.line_to(p) }
            }
            if close {
                path.close_path();
            }
            Some(path)
        };
        match &*el.name.local {
            "path" => kurbo::BezPath::from_svg(el.attr(markup5ever::local_name!("d"))?).ok(),
            "rect" => Some(kurbo::Rect::new(num("x"), num("y"), num("x") + num("width"), num("y") + num("height")).to_path(0.01)),
            "circle" => Some(kurbo::Circle::new((num("cx"), num("cy")), num("r").max(0.0)).to_path(1e-6)),
            "ellipse" => Some(kurbo::Ellipse::new((num("cx"), num("cy")), (num("rx").max(0.0), num("ry").max(0.0)), 0.0).to_path(1e-6)),
            "line" => {
                let mut path = kurbo::BezPath::new();
                path.move_to((num("x1"), num("y1")));
                path.line_to((num("x2"), num("y2")));
                Some(path)
            }
            "polyline" => points(false),
            "polygon" => points(true),
            _ => None,
        }
    }
}

fn place_overflow(node: &crate::Node, location: (f64, f64), scale: f64, rect: Rect) -> Rect {
    let offset = kurbo::Vec2::new(location.0, location.1);
    let plane = node.plane_3d();
    if plane.is_some_and(|plane| !plane.matrix.to_array().iter().all(|v| v.is_finite())) {
        return Rect::ZERO;
    }
    let bounds = if let Some(plane) = plane.filter(|plane| plane.places_by_local || plane.crosses_viewer) {
        let limit = f64::from(app_units::MAX_AU.to_f32_px());
        let Some(bounds) = crate::projection::layout_bounds(&plane.local, rect.scale_from_origin(1.0 / scale)) else {
            return Rect::ZERO;
        };
        Rect::new(
            bounds.x0.clamp(-limit, limit), bounds.y0.clamp(-limit, limit),
            bounds.x1.clamp(-limit, limit), bounds.y1.clamp(-limit, limit),
        ).scale_from_origin(scale) + offset
    } else {
        match node.transform().as_deref() {
            Some(transform) if transform.is_finite() => transform.transform_rect_bbox(rect) + offset,
            Some(_) => return Rect::ZERO,
            None => rect + offset,
        }
    };
    if bounds.is_finite() { bounds } else { Rect::ZERO }
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

impl BaseDocument {
    /// pyro: whether the viewport's horizontal scroll origin is on the right, by the principal
    /// writing mode (the root's, or its `<body>`'s: `root_layout_wm`; [`scrolls_leftward`]).
    pub fn viewport_scrolls_leftward(&self) -> bool {
        self.try_root_element().is_some_and(|root| scrolls_leftward(self.root_layout_wm(root.id)))
    }

    /// pyro: the document's scroll size (CSS px), at least `viewport` (the scrollport: the
    /// viewport less its shown classic scrollbars): a document scrolled from the right
    /// ([`Self::viewport_scrolls_leftward`]) reaches what overflows it leftward, and not what
    /// overflows rightward. (The embedder's `scrollWidth`/`scrollHeight` of the root and the
    /// viewport's scroll timelines.)
    pub fn document_scroll_size(&self, viewport: (f64, f64)) -> (f64, f64) {
        let (vw, vh) = viewport;
        // Without a document element the document is as large as the viewport.
        let Some(root) = self.try_root_element() else { return (vw, vh) };
        let (size, overflow) =
            (root.final_layout().size, root.scrollable_overflow().scale_from_origin(1.0 / self.viewport().scale_f64()));
        let width = if self.viewport_scrolls_leftward() {
            let location = root.final_layout().location.x as f64;
            ((size.width as f64) + location).max(vw) - (location + overflow.x0).min(0.0)
        } else {
            (size.width as f64).max(overflow.x1).max(vw)
        };
        (width, (size.height as f64).max(overflow.y1).max(vh))
    }
}

/// pyro: whether a document in writing mode `wm` has its horizontal scroll origin on the right:
/// its content overflows the viewport leftward (`direction: rtl` in horizontal writing, blocks
/// flowing right to left in `vertical-rl`).
pub fn scrolls_leftward(wm: style::logical_geometry::WritingMode) -> bool {
    if wm.is_vertical() { !wm.is_vertical_lr() } else { !wm.is_bidi_ltr() }
}

/// pyro: whether `node` skips its contents (CSS Contain 2 § 4): `content-visibility: hidden`, or
/// `auto` while not relevant to the user (`BaseDocument::update_content_visibility`).
pub(crate) fn skips_contents(node: &crate::Node, styles: &style::properties::ComputedValues) -> bool {
    use crate::node::NodeFlags;
    use style::values::specified::box_::ContentVisibility;
    node.flags.contains(NodeFlags::CV_SKIPPED)
        || (node.is_element() && styles.get_box().slow_clone_content_visibility() == ContentVisibility::Hidden)
}

/// pyro: whether `contain-intrinsic-width` and `-height` have `auto` (physical: width, height);
/// `content-visibility: auto` adds it to both (CSS Contain 2 § 4.1, csswg-drafts#8407).
pub(crate) fn contain_intrinsic_auto(styles: &style::properties::ComputedValues) -> (bool, bool) {
    use style::values::generics::box_::ContainIntrinsicSize;
    use style::values::specified::box_::ContentVisibility;
    let cv_auto = styles.get_box().slow_clone_content_visibility() == ContentVisibility::Auto;
    let position = styles.get_position();
    let auto = |size: &style::values::computed::ContainIntrinsicSize| {
        cv_auto || matches!(size, ContainIntrinsicSize::AutoNone | ContainIntrinsicSize::AutoLength(_))
    };
    (auto(&position.contain_intrinsic_width), auto(&position.contain_intrinsic_height))
}
