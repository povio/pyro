//! CSS vertical writing modes (`writing-mode` feature).
//!
//! Taffy is a horizontal-tb engine: `width`/x is the inline axis and `height`/+y the block axis.
//! Blitz runs each box's layout algorithm in the axes of the box's own writing mode and translates at
//! the boundary (see [`stylo_taffy::writing_mode`] for the axis mapping):
//!
//! - Style getters: a box's own algorithm reads its style in its own writing mode
//!   (`get_core_container_style`); the algorithm laying it out reads it in *that* algorithm's writing mode
//!   (`get_*_child_style`, `get_oof_item_style`, [`BaseDocument::child_layout_style`]).
//!   [`LayoutPassState::layout_wm`] tracks the writing mode of the running algorithm.
//! - Orthogonal flows ([`compute_child_layout_in_own_wm`]): when a box's writing mode differs from its
//!   parent's, the `LayoutInput` is transposed into the box's axes, its auto inline size is
//!   resolved to fit-content (css-writing-modes-3 §7.3.1), and the `LayoutOutput` is transposed
//!   back. Percentage padding on such a box resolves against the containing block's inline size
//!   ([`LayoutPassState::orthogonal_percent_basis`]).
//! - Geometry: `unrounded_layout` is written by the placing algorithm in *its* writing mode.
//!   [`physicalise_and_round_layout`] converts every layout to physical coordinates (mirroring
//!   the block axis for `*-rl` modes) and rounds it, replacing `taffy::round_layout`.
//!
//! [`compute_child_layout_in_own_wm`]: LayoutPassState::compute_child_layout_in_own_wm
//! [`physicalise_and_round_layout`]: LayoutPassState::physicalise_and_round_layout

use super::{BlockContext, LayoutPassState, resolve_calc_value};
use crate::document::BaseDocument;
use crate::dom_node_id;
use crate::node::Node;
use stylo_taffy::WritingMode;
use stylo_taffy::WritingModeExt;
use taffy::{
    AvailableSpace, AxisStaticEdge, AxisStaticPosition, CoreStyle, Layout, LayoutInput,
    LayoutOutput, NodeId, Point, ResolveOrZero, RoundTree, RunMode, Size, TraversePartialTree,
};

impl BaseDocument {
    /// The writing mode in which the root element's layout algorithm runs: the HTML special case of
    /// css-writing-modes-3 §8 uses the `writing-mode` of its `<body>` child instead of its own.
    pub(crate) fn root_layout_wm(&self, root_id: crate::NodeId) -> WritingMode {
        let root = &self.nodes[root_id];
        let wm = root.writing_mode();
        if root.has_containment() {
            return wm;
        }
        root.children
            .iter()
            .map(|&child| &self.nodes[child])
            .find(|child| {
                child
                    .data
                    .downcast_element()
                    .is_some_and(|el| *el.name.local == *"body")
            })
            .filter(|body| !body.has_containment())
            .map(|body| body.writing_mode())
            .unwrap_or(wm)
    }
}

impl LayoutPassState<'_> {
    /// The writing mode in which `node_id`'s layout algorithm runs (the root's is resolved once per
    /// pass by [`BaseDocument::root_layout_wm`]).
    #[inline]
    pub(crate) fn layout_wm_of(&self, node_id: crate::NodeId) -> WritingMode {
        if Some(node_id) == self.root_id {
            self.root_wm
        } else {
            self.nodes[node_id].writing_mode()
        }
    }

    /// Lay out `node_id`, translating between the parent's writing mode and the node's own at an
    /// orthogonal-flow boundary.
    pub(crate) fn compute_child_layout_in_own_wm(
        &mut self,
        node_id: NodeId,
        inputs: LayoutInput,
        block_ctx: Option<&mut BlockContext<'_>>,
    ) -> LayoutOutput {
        let node_wm = self.layout_wm_of(dom_node_id(node_id));
        let parent_wm = core::mem::replace(&mut self.layout_wm, node_wm);
        // Set by `dispatch_child_layout` when the node turns out to be a column flex container.
        let parent_align_axis_is_inline =
            core::mem::replace(&mut self.current_align_axis_is_inline, false);

        let mut output = if node_wm.is_vertical() == parent_wm.is_vertical() {
            let mut output = self.compute_child_layout_in_current_wm(node_id, inputs, block_ctx);
            if node_wm.is_vertical() && node_wm.is_vertical_lr() != parent_wm.is_vertical_lr() {
                // Parallel writing modes with opposite block directions (`vertical-lr` in `vertical-rl`
                // or vice versa) share Taffy's axes but run the block axis the other way.
                let extent = output.size.height;
                for candidate in output.oof_candidates.as_mut_slice() {
                    candidate.static_position.y =
                        mirror_static_position(candidate.static_position.y, extent);
                }
            }
            output
        } else {
            self.compute_orthogonal_child_layout(node_id, inputs, parent_wm)
        };

        let node_block_reversed = node_wm.block_left_is_end();
        let parent_block_reversed = parent_wm.block_left_is_end();
        let mirror_x = node_block_reversed && !parent_wm.is_vertical();
        let mirror_y = (node_block_reversed && parent_wm.is_vertical()) ^ parent_block_reversed;
        if mirror_x || mirror_y {
            self.mirror_child_overflow(node_id, inputs, parent_wm, &mut output, mirror_x, mirror_y);
        }

        self.layout_wm = parent_wm;
        self.current_align_axis_is_inline = parent_align_axis_is_inline;
        output
    }

    /// lightningpanda: kept out of the frame of `compute_child_layout_in_own_wm`, which is on the
    /// stack once per tree level of a recursive layout.
    #[inline(never)]
    fn mirror_child_overflow(
        &self,
        node_id: NodeId,
        inputs: LayoutInput,
        parent_wm: WritingMode,
        output: &mut LayoutOutput,
        mirror_x: bool,
        mirror_y: bool,
    ) {
        let style = self.nodes[dom_node_id(node_id)].layout_style_in(parent_wm);
        let border = style
            .border()
            .resolve_or_zero(inputs.parent_size.width, resolve_calc_value);
        let client_size = output.size - border.sum_axes();
        let overflow = style.overflow();
        let is_scroll_container =
            overflow.x.is_scroll_container() || overflow.y.is_scroll_container();
        output.scrollable_overflow_rect = mirror_overflow(
            output.scrollable_overflow_rect,
            client_size,
            Point {
                x: mirror_x,
                y: mirror_y,
            },
            is_scroll_container,
        );
    }

    #[inline(never)]
    fn compute_orthogonal_child_layout(
        &mut self,
        node_id: NodeId,
        inputs: LayoutInput,
        parent_wm: WritingMode,
    ) -> LayoutOutput {
        let percent_basis = self.orthogonal_percent_basis.replace((
            dom_node_id(node_id),
            inputs.parent_size.width.unwrap_or(0.0),
        ));

        let mut inputs = inputs.transpose();

        // A block container stretches its in-flow children's inline size and passes it as a known
        // dimension. For an orthogonal child that is its block size, which is content-sized.
        // (An out-of-flow box's known dimensions come from its insets and are kept.)
        let node = &self.nodes[dom_node_id(node_id)];
        let placed_by_block_container = !self.is_out_of_flow(node_id)
            && node
                .layout_parent
                .get()
                .map(|parent| self.nodes[parent].taffy_display() == taffy::Display::Block)
                .unwrap_or(true);
        if placed_by_block_container
            && taffy::CoreStyle::size(&node.layout_style())
                .height
                .is_auto()
        {
            inputs.known_dimensions.height = None;
            inputs.known_dimensions_are_definite.height = false;
        }

        let is_intrinsic_inline_measurement = inputs.axis == taffy::RequestedAxis::Horizontal
            && matches!(
                inputs.available_space.width,
                AvailableSpace::MinContent | AvailableSpace::MaxContent
            );

        if inputs.run_mode != RunMode::PerformHiddenLayout
            && inputs.known_dimensions.width.is_none()
            && !is_intrinsic_inline_measurement
        {
            let inline_size_is_auto =
                taffy::CoreStyle::size(&self.nodes[dom_node_id(node_id)].layout_style())
                    .width
                    .is_auto();
            if inline_size_is_auto {
                // css-writing-modes-3 §7.3.1: an auto inline size in an orthogonal flow is
                // fit-content against the available space in that axis: the containing block's
                // block size if definite, else the initial containing block (nearest fixed-size
                // scroll container not implemented).
                let available = match (inputs.available_space.width, inputs.parent_size.width) {
                    (AvailableSpace::Definite(size), _) => size,
                    (_, Some(size)) => size,
                    _ => {
                        let viewport = self.stylist.device().au_viewport_size();
                        if parent_wm.is_vertical() {
                            viewport.width.to_f32_px()
                        } else {
                            viewport.height.to_f32_px()
                        }
                    }
                };
                let measure = |doc: &mut Self, available_space: AvailableSpace| {
                    let measure_inputs = LayoutInput {
                        run_mode: RunMode::ComputeSize,
                        axis: taffy::RequestedAxis::Horizontal,
                        known_dimensions: Size {
                            width: None,
                            height: inputs.known_dimensions.height,
                        },
                        available_space: Size {
                            width: available_space,
                            height: inputs.available_space.height,
                        },
                        ..inputs
                    };
                    doc.compute_child_layout_in_current_wm(node_id, measure_inputs, None)
                        .size
                        .width
                };
                let max_content = measure(self, AvailableSpace::MaxContent);
                let min_content = measure(self, AvailableSpace::MinContent);
                inputs.known_dimensions.width = Some(available.min(max_content).max(min_content));
                inputs.known_dimensions_are_definite.width = true;
            }
        }

        let mut output = self.compute_child_layout_in_current_wm(node_id, inputs, None);
        self.orthogonal_percent_basis = percent_basis;
        transpose_static_positions(&mut output, self.layout_wm, parent_wm);
        output.transpose()
    }

    /// Convert every `unrounded_layout` (written in the writing mode of the box that placed it) to
    /// physical coordinates and round it into `final_layout`, replacing `taffy::round_layout`.
    /// Subtrees whose rounding cannot have changed are skipped (see [`RoundTree::rounded_unchanged`]).
    pub(crate) fn physicalise_and_round_layout(&mut self, root: NodeId) {
        let viewport = self.stylist.device().au_viewport_size();
        let viewport = Size {
            width: viewport.width.to_f32_px(),
            height: viewport.height.to_f32_px(),
        };
        let root_wm = self.layout_wm_of(dom_node_id(root));
        self.round_counts = (0, 0);
        self.physicalise_and_round_inner(root, root_wm, viewport, Point::ZERO);
        crate::resolve::phase_profile::rounded(self.round_counts);
        #[cfg(debug_assertions)]
        self.assert_rounding_is_current(root, root_wm, viewport, Point::ZERO);
    }

    /// `parent_frac` is the fractional part (in `[0, 1)`) of the parent's unrounded absolute
    /// position: all the rounding of a box and its subtree depends on besides their own layouts
    /// (see [`rounded_layout`]).
    fn physicalise_and_round_inner(
        &mut self,
        node_id: NodeId,
        placer_wm: WritingMode,
        placer_size: Size<f32>,
        parent_frac: Point<f32>,
    ) {
        // lightningpanda: only a vertical placer reads it (`physical_layout`).
        let is_scroll_container = placer_wm.is_vertical() && {
            let overflow = self.doc.nodes[dom_node_id(node_id)].layout_style().overflow();
            overflow.x.is_scroll_container() || overflow.y.is_scroll_container()
        };
        let unrounded = physical_layout(
            self.get_unrounded_layout(node_id),
            placer_wm,
            placer_size,
            is_scroll_container,
        );
        self.round_counts.0 += 1;

        // Keyed on the physical layout: a `*-rl` box's x depends on its placer's size. That, the
        // parent's fractional offset and the subtree's unrounded layouts, whose changes are damage,
        // are all the rounding below reads: a subtree moved by whole pixels rounds as it did.
        if self.rounded_unchanged(node_id, &unrounded, parent_frac.x, parent_frac.y) {
            self.round_counts.1 += 1;
            return;
        }
        self.record_rounded(node_id, &unrounded, parent_frac.x, parent_frac.y);

        let (layout, frac) = rounded_layout(unrounded, parent_frac);
        let size = unrounded.size;

        // `unrounded_layout` is left in the placer's writing mode: Taffy's cache may skip re-placing
        // this node on a later layout, and only `final_layout` is read downstream.
        self.set_final_layout(node_id, &layout);

        let wm = self.layout_wm_of(dom_node_id(node_id));
        // lightningpanda: `content_box_subpixel_offset` (upstream #1382) stays zero: screenshots keep
        // text at the whole-pixel content box, and print moves text to its layout-unit x itself
        // (`fractional_text_x` in blitz-paint).
        self.node_from_id_mut(node_id).layout_data_mut().writing_mode = wm;
        // lightningpanda: a table where it was rounds again only the cells its grid placed since:
        // the others' layouts and offsets, all their rounding reads, are the same.
        let table_cells = (self.nodes[dom_node_id(node_id)].flags.is_table_root() && !wm.is_vertical())
            .then(|| crate::layout::table::table_cells_to_round(self, dom_node_id(node_id), frac))
            .flatten();
        if let Some(cells) = table_cells {
            let node = &self.nodes[dom_node_id(node_id)];
            let pseudos = [node.before(), node.after()];
            for child in cells.into_iter().chain(pseudos.into_iter().flatten()) {
                let child = crate::taffy_node_id(child);
                if !self.is_out_of_flow(child) {
                    self.physicalise_and_round_inner(child, wm, size, frac);
                }
            }
            for index in 0..self.hoisted_child_count(node_id) {
                let child = self.get_hoisted_child_id(node_id, index);
                self.physicalise_and_round_inner(child, wm, size, frac);
            }
            return;
        }
        for index in 0..self.child_count(node_id) {
            let child = self.get_child_id(node_id, index);
            if !self.is_out_of_flow(child) {
                self.physicalise_and_round_inner(child, wm, size, frac);
            }
        }
        for index in 0..self.hoisted_child_count(node_id) {
            let child = self.get_hoisted_child_id(node_id, index);
            self.physicalise_and_round_inner(child, wm, size, frac);
        }
    }

    /// Rounds the whole tree without the skip and without writing: what the skip left in place must
    /// be what this computes. Geometry that goes stale silently is the failure this guards against.
    #[cfg(debug_assertions)]
    fn assert_rounding_is_current(
        &self,
        node_id: NodeId,
        placer_wm: WritingMode,
        placer_size: Size<f32>,
        parent_frac: Point<f32>,
    ) {
        let is_scroll_container = placer_wm.is_vertical() && {
            let overflow = self.doc.nodes[dom_node_id(node_id)].layout_style().overflow();
            overflow.x.is_scroll_container() || overflow.y.is_scroll_container()
        };
        let unrounded = physical_layout(self.get_unrounded_layout(node_id), placer_wm, placer_size, is_scroll_container);
        let (layout, frac) = rounded_layout(unrounded, parent_frac);
        let node = self.node_from_id(node_id);
        // Formatted, as NaNs (never equal) must not fail this.
        let current = node.final_layout();
        assert!(
            *current == layout || format!("{current:?}") == format!("{layout:?}"),
            "incremental rounding left a stale layout on {:?} ({:?}): {current:?}, should be {layout:?}",
            node.id,
            node.data.downcast_element().map(|el| &el.name.local),
        );
        let wm = self.layout_wm_of(dom_node_id(node_id));
        assert_eq!(node.layout_data().writing_mode, wm, "stale writing mode on {:?}", node.id);
        for index in 0..self.child_count(node_id) {
            let child = self.get_child_id(node_id, index);
            if !self.is_out_of_flow(child) {
                self.assert_rounding_is_current(child, wm, unrounded.size, frac);
            }
        }
        for index in 0..self.hoisted_child_count(node_id) {
            let child = self.get_hoisted_child_id(node_id, index);
            self.assert_rounding_is_current(child, wm, unrounded.size, frac);
        }
    }
}

/// Rounds half up (`-2.5` to `-2`, as Blink's `LayoutUnit::Round`): the only rounding that moves
/// with the pixel grid, `round(k + v) == k + round(v)` for every integer `k`, which `f32::round`
/// (half away from zero) is not across zero. Exact for `f32` values.
#[inline]
fn round_half_up(v: f32) -> f32 {
    let rounded = v.round();
    if v - rounded == 0.5 { rounded + 1.0 } else { rounded }
}

/// `unrounded` (physical) rounded, as in `taffy::round_layout`: the box's outer edges are rounded in
/// absolute coordinates and every inner edge is rounded relative to them, so adjacent boxes never
/// overlap or gap.
///
/// lightningpanda: the absolute coordinates are kept modulo whole pixels. `parent_frac` is the
/// fractional part of the parent's absolute position; the box's position in the pixel grid is
/// `parent_frac + location`, rounded half up, and the parent's is `parent_frac` rounded. Moving the
/// parent by whole pixels changes nothing here (the arithmetic never sees the whole pixels), so
/// a subtree moved that way needs no rounding again (`RoundTree::rounded_unchanged`), and the
/// sums are taken near zero, where `f32` is finest. Also returns the box's fractional offset, for
/// its children.
fn rounded_layout(unrounded: Layout, parent_frac: Point<f32>) -> (Layout, Point<f32>) {
    let pos = Point {
        x: parent_frac.x + unrounded.location.x,
        y: parent_frac.y + unrounded.location.y,
    };
    let size = unrounded.size;
    let rounded_pos = Point {
        x: round_half_up(pos.x),
        y: round_half_up(pos.y),
    };
    let rounded_end = Point {
        x: round_half_up(pos.x + size.width),
        y: round_half_up(pos.y + size.height),
    };
    let from_left = |v: f32| round_half_up(pos.x + v) - rounded_pos.x;
    let from_top = |v: f32| round_half_up(pos.y + v) - rounded_pos.y;
    let from_right = |v: f32| rounded_end.x - round_half_up(pos.x + size.width - v);
    let from_bottom = |v: f32| rounded_end.y - round_half_up(pos.y + size.height - v);

    let mut layout = unrounded;
    layout.location = Point {
        x: rounded_pos.x - round_half_up(parent_frac.x),
        y: rounded_pos.y - round_half_up(parent_frac.y),
    };
    layout.size = Size {
        width: rounded_end.x - rounded_pos.x,
        height: rounded_end.y - rounded_pos.y,
    };
    layout.scrollbar_size = unrounded.scrollbar_size.map(f32::round);
    layout.border = taffy::Rect {
        left: from_left(unrounded.border.left),
        right: from_right(unrounded.border.right),
        top: from_top(unrounded.border.top),
        bottom: from_bottom(unrounded.border.bottom),
    };
    layout.padding = taffy::Rect {
        left: from_left(unrounded.padding.left),
        right: from_right(unrounded.padding.right),
        top: from_top(unrounded.padding.top),
        bottom: from_bottom(unrounded.padding.bottom),
    };
    let overflow = unrounded.scrollable_overflow_rect.rect();
    layout.scrollable_overflow_rect = taffy::ScrollableOverflow::new(
        taffy::Rect {
            left: from_left(overflow.left),
            right: from_left(overflow.right),
            top: from_top(overflow.top),
            bottom: from_top(overflow.bottom),
        },
        unrounded.scrollable_overflow_rect.is_reversed(),
    );
    // The box's fractional offset: `pos` less its whole pixels (exact in `f32`).
    let frac = Point {
        x: pos.x - pos.x.floor(),
        y: pos.y - pos.y.floor(),
    };
    (layout, frac)
}

/// Map the static positions of `output`'s out-of-flow candidates (relative to the border box of a
/// node laid out in `child_wm`) into `parent_wm`, ahead of [`LayoutOutput::transpose`],
/// which leaves them untouched. The block axis of `vertical-rl` runs against Taffy's
/// coordinate, so that coordinate is mirrored within the node's size.
fn transpose_static_positions(
    output: &mut LayoutOutput,
    child_wm: WritingMode,
    parent_wm: WritingMode,
) {
    if output.oof_candidates.is_empty() {
        return;
    }
    let vertical_wm = if child_wm.is_vertical() {
        child_wm
    } else {
        parent_wm
    };
    let mirror_block = !vertical_wm.is_vertical_lr();
    let size = output.size;
    for candidate in output.oof_candidates.as_mut_slice() {
        let Point { mut x, mut y } = candidate.static_position;
        if mirror_block {
            if child_wm.is_vertical() {
                y = mirror_static_position(y, size.height);
            } else {
                x = mirror_static_position(x, size.width);
            }
        }
        candidate.static_position = Point { x: y, y: x };
    }
}

fn mirror_static_position(position: AxisStaticPosition, extent: f32) -> AxisStaticPosition {
    let flip = |edge: AxisStaticEdge| match edge {
        AxisStaticEdge::Start => AxisStaticEdge::End,
        AxisStaticEdge::End => AxisStaticEdge::Start,
        AxisStaticEdge::Center => AxisStaticEdge::Center,
    };
    AxisStaticPosition {
        area: taffy::Line {
            start: extent - position.area.end,
            end: extent - position.area.start,
        },
        align: taffy::AxisStaticAlign {
            keyword: flip(position.align.keyword),
            fallback: flip(position.align.fallback),
            ..position.align
        },
    }
}

fn mirror_overflow(
    overflow: taffy::ScrollableOverflow,
    client_size: Size<f32>,
    axes: Point<bool>,
    is_scroll_container: bool,
) -> taffy::ScrollableOverflow {
    let mut rect = overflow.rect();
    let mut reversed = overflow.is_reversed();
    if axes.x {
        let left = client_size.width - rect.right;
        rect.right = (client_size.width - rect.left).max(0.0);
        rect.left = left.min(0.0);
        reversed.x = is_scroll_container && !reversed.x;
    }
    if axes.y {
        let top = client_size.height - rect.bottom;
        rect.bottom = (client_size.height - rect.top).max(0.0);
        rect.top = top.min(0.0);
        reversed.y = is_scroll_container && !reversed.y;
    }
    taffy::ScrollableOverflow::new(rect, reversed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mirrored_overflow_preserves_both_sides_and_scroll_container_flags() {
        for is_scroll_container in [false, true] {
            let original = taffy::ScrollableOverflow::new(
                taffy::Rect {
                    left: -96.0,
                    right: 204.0,
                    top: -20.0,
                    bottom: 150.0,
                },
                Point { x: false, y: false },
            );
            let size = Size {
                width: 100.0,
                height: 80.0,
            };
            let axes = Point { x: true, y: true };
            let mirrored = mirror_overflow(original, size, axes, is_scroll_container);
            assert_eq!(
                mirrored.rect(),
                taffy::Rect {
                    left: -104.0,
                    right: 196.0,
                    top: -70.0,
                    bottom: 100.0
                }
            );
            assert_eq!(
                mirrored.is_reversed(),
                Point {
                    x: is_scroll_container,
                    y: is_scroll_container
                }
            );
            assert_eq!(
                mirror_overflow(mirrored, size, axes, is_scroll_container),
                original
            );
        }
    }

    #[test]
    fn empty_overflow_remains_without_scrollable_extent_when_mirrored() {
        let size = Size {
            width: 100.0,
            height: 80.0,
        };
        let overflow = mirror_overflow(
            taffy::ScrollableOverflow::ZERO,
            size,
            Point { x: true, y: true },
            true,
        );
        let layout = Layout {
            size,
            scrollable_overflow_rect: overflow,
            ..Layout::new()
        };
        assert_eq!((layout.scroll_width(), layout.scroll_height()), (0.0, 0.0));
    }
}

/// `logical`, written by a placing algorithm running in `placer_wm` within a box of physical size
/// `placer_size`, in physical coordinates.
/// lightningpanda: whether `node` is a scroll container, for CSSOM geometry queries. These reach
/// boxes outside the layout tree too (below a `display: none` element, which have no styles), and
/// an unstyled box is not one.
fn is_styled_scroll_container(node: &Node) -> bool {
    node.primary_styles().is_some_and(|style| {
        let b = style.get_box();
        b.overflow_x.is_scrollable() || b.overflow_y.is_scrollable()
    })
}

fn physical_layout(
    logical: Layout,
    placer_wm: WritingMode,
    placer_size: Size<f32>,
    is_scroll_container: bool,
) -> Layout {
    if !placer_wm.is_vertical() {
        return logical;
    }
    let mut overflow = logical.scrollable_overflow_rect.transpose();
    if !placer_wm.is_vertical_lr() {
        let client_width = logical.size.height
            - logical.border.top
            - logical.border.bottom
            - logical.scrollbar_size.height;
        overflow = mirror_overflow(
            overflow,
            Size {
                width: client_width,
                height: 0.0,
            },
            Point { x: true, y: false },
            is_scroll_container,
        );
    }
    Layout {
        location: Point {
            x: if placer_wm.is_vertical_lr() {
                logical.location.y
            } else {
                placer_size.width - logical.location.y - logical.size.height
            },
            y: logical.location.x,
        },
        size: logical.size.transpose(),
        scrollbar_size: logical.scrollbar_size.transpose(),
        scrollable_overflow_rect: overflow,
        border: placer_wm.physical_rect(logical.border),
        padding: placer_wm.physical_rect(logical.padding),
        margin: placer_wm.physical_rect(logical.margin),
        ..logical
    }
}

impl BaseDocument {
    /// `node_id`'s unrounded layout in physical coordinates (`unrounded_layout` is in the writing
    /// mode of the box that placed it).
    pub fn physical_unrounded_layout(&self, node_id: crate::NodeId) -> Layout {
        let node = &self.nodes[node_id];
        let vertical = |node: &Node| node.layout_data().writing_mode.is_vertical();
        if vertical(node) || node.containing_block().is_some_and(|parent| vertical(&self.nodes[parent])) {
            self.physical_unrounded_geometry(node_id).1
        } else {
            *node.unrounded_layout()
        }
    }

    /// `node_id`'s document-relative position and unrounded layout in physical coordinates, for
    /// CSSOM geometry. `unrounded_layout` itself is left in the placing algorithm's writing mode (see
    /// [`LayoutPassState::physicalise_and_round_layout`]), so the containing-block chain is
    /// physicalised root-down in a single descent, each box in its placer's writing mode and size.
    pub fn physical_unrounded_geometry(
        &self,
        node_id: crate::NodeId,
    ) -> (crate::util::Point<f32>, Layout) {
        // Without a vertical box on the containing-block chain there is nothing to physicalise.
        let mut pos = crate::util::Point { x: 0.0, y: 0.0 };
        let mut has_vertical = false;
        let mut current = Some(node_id);
        while let Some(id) = current {
            let node = &self.nodes[id];
            pos.x += node.unrounded_layout().location.x - node.scroll_offset().x as f32;
            pos.y += node.unrounded_layout().location.y - node.scroll_offset().y as f32;
            has_vertical |= node.layout_data().writing_mode.is_vertical();
            current = node.containing_block();
        }
        if !has_vertical {
            return (pos, *self.nodes[node_id].unrounded_layout());
        }

        let root_id = self.try_root_element().map(|root| root.id);
        let node = &self.nodes[node_id];
        let (mut pos, placer_wm, placer_size) = self.placer_geometry(node, node_id, root_id);
        let layout = physical_layout(
            *node.unrounded_layout(),
            placer_wm,
            placer_size,
            is_styled_scroll_container(node),
        );
        pos.x += layout.location.x - node.scroll_offset().x as f32;
        pos.y += layout.location.y - node.scroll_offset().y as f32;
        (pos, layout)
    }

    /// Document-relative position, writing mode and physical size of the box that places `node`.
    fn placer_geometry(
        &self,
        node: &Node,
        node_id: crate::NodeId,
        root_id: Option<crate::NodeId>,
    ) -> (crate::util::Point<f32>, WritingMode, Size<f32>) {
        let Some(placer_id) = node.containing_block().filter(|_| Some(node_id) != root_id) else {
            let viewport = self.stylist.device().au_viewport_size();
            let viewport = Size {
                width: viewport.width.to_f32_px(),
                height: viewport.height.to_f32_px(),
            };
            let wm = root_id.map_or(WritingMode::empty(), |root| {
                self.nodes[root].layout_data().writing_mode
            });
            return (crate::util::Point { x: 0.0, y: 0.0 }, wm, viewport);
        };

        let placer = &self.nodes[placer_id];
        let (mut pos, grand_wm, grand_size) = self.placer_geometry(placer, placer_id, root_id);
        let logical = placer.unrounded_layout();
        let (location, size) = if grand_wm.is_vertical() {
            let physical = physical_layout(
                *logical,
                grand_wm,
                grand_size,
                is_styled_scroll_container(placer),
            );
            (physical.location, physical.size)
        } else {
            (logical.location, logical.size)
        };
        pos.x += location.x - placer.scroll_offset().x as f32;
        pos.y += location.y - placer.scroll_offset().y as f32;
        (pos, placer.layout_data().writing_mode, size)
    }
}
