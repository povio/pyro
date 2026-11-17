//! Enable the dom to lay itself out using taffy
//!
//! In servo, style and layout happen together during traversal
//! However, in Blitz, we do a style pass then a layout pass.
//! This is slower, yes, but happens fast enough that it's not a huge issue.

use crate::node::{ComputedStyleRef, ImageData, NodeData, NodeFlags, SpecialElementData};
use crate::{document::BaseDocument, dom_node_id, node::Node, taffy_node_id};
use markup5ever::local_name;
use std::cell::Ref;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use style::Atom;
use style::values::computed::CSSPixelLength;
use style::values::computed::length_percentage::CalcLengthPercentage;
use stylo_taffy::CopyStyleGetters;
use stylo_taffy::TaffyStyloStyle;
use taffy::{
    AxisStaticEdge, AxisStaticPosition, BlockContext, CoreStyle as _, DetailedLayoutInfo,
    FlexDirection, LayoutContainingBlock, LayoutPartialTree, MaybeMath as _, NodeId, OofCandidate,
    ResolveOrZero, RoundTree, RunMode, TraversePartialTree, TraverseTree, compute_block_layout,
    compute_cached_layout, compute_flexbox_layout, compute_grid_layout, compute_leaf_layout,
    compute_oof_layout, prelude::*,
};

pub(crate) mod construct;
pub(crate) mod counters;
mod quotes_table;
pub(crate) mod damage;
pub(crate) mod first_letter;
pub(crate) mod first_line;
pub(crate) mod hyphenate;
pub(crate) mod inline;
pub(crate) mod line_clamp;
pub(crate) mod list;
pub mod multicol;
pub(crate) mod paint_tree;
pub(crate) mod replaced;
pub(crate) mod scroll_markers;
pub mod table;
pub(crate) mod table_fixup;
pub(crate) mod text_box;
pub(crate) mod text_transform;
#[cfg(feature = "writing-mode")]
pub(crate) mod writing_mode;

use self::replaced::{
    IntrinsicSizes, ReplacedContext, compute_replaced_layout, is_image_input, is_replaced,
};
use self::table::TableTreeWrapper;

/// The default object size for replaced elements
/// (https://drafts.csswg.org/css-images/#default-object-size).
const DEFAULT_OBJECT_SIZE: taffy::Size<f32> = taffy::Size {
    width: 300.0,
    height: 150.0,
};

/// The intrinsic dimensions and default object size for a replaced element
/// whose intrinsic dimensions are determined by its tag: an image (`<img>`,
/// or an `<input type=image>` image button) with no loaded resource has no
/// intrinsic dimensions and a zero default object size; a canvas has an
/// intrinsic size and aspect ratio given by its width/height attributes
/// (defaulting to 300x150); other replaced elements (video, iframe, embed)
/// have no intrinsic dimensions and the 300x150 default object size.
fn tag_intrinsic_sizes(
    element: &crate::node::ElementData,
    attr_size: taffy::Size<Option<f32>>,
) -> (IntrinsicSizes, taffy::Size<f32>) {
    let tag_name = &element.name.local;
    if *tag_name == local_name!("img")
        || *tag_name == local_name!("svg")
        || is_image_input(element)
    {
        return (IntrinsicSizes::default(), taffy::Size::ZERO);
    }
    if *tag_name == local_name!("canvas") {
        let width = attr_size.width.unwrap_or(300.0);
        let height = attr_size.height.unwrap_or(150.0);
        return (
            IntrinsicSizes {
                width: Some(width),
                height: Some(height),
                ratio: Some(width / height),
            },
            DEFAULT_OBJECT_SIZE,
        );
    }
    (IntrinsicSizes::default(), DEFAULT_OBJECT_SIZE)
}

/// pyro: whether `width` or `height` is `calc-size()` (`LayoutPassState::calc_size_inputs`).
#[inline]
fn has_calc_size(styles: &style::properties::ComputedValues) -> bool {
    use style::values::computed::Size;
    let is_calc_size = |size: &Size| {
        matches!(
            size,
            Size::CalcSizeAuto(_) | Size::CalcSizeFitContent(_) | Size::CalcSizeMinContent(_) | Size::CalcSizeMaxContent(_)
        )
    };
    let position = styles.get_position();
    is_calc_size(&position.width) || is_calc_size(&position.height)
}

pub(crate) fn resolve_calc_value(calc_ptr: *const (), parent_size: f32) -> f32 {
    let calc = unsafe { &*(calc_ptr as *const CalcLengthPercentage) };
    let result = calc.resolve(CSSPixelLength::new(parent_size));
    result.px()
}

/// Per-pass layout state: wraps the document for the duration of a layout pass and carries
/// the Taffy tree trait implementations. Derefs to [`BaseDocument`].
pub(crate) struct LayoutPassState<'doc> {
    doc: &'doc mut BaseDocument,
    /// The writing mode of the box whose layout algorithm is currently running (see
    /// `layout::writing_mode`). Child styles read during that algorithm are expressed in its axes.
    #[cfg(feature = "writing-mode")]
    pub(crate) layout_wm: stylo_taffy::WritingMode,
    /// The root element and the writing mode its algorithm runs in (see `BaseDocument::root_layout_wm`).
    #[cfg(feature = "writing-mode")]
    root_id: Option<crate::NodeId>,
    #[cfg(feature = "writing-mode")]
    root_wm: stylo_taffy::WritingMode,
    /// Whether the node whose layout algorithm is running aligns its children's `align-self` in its
    /// inline axis (a column flex container).
    #[cfg(feature = "writing-mode")]
    pub(crate) current_align_axis_is_inline: bool,
    /// The box currently being laid out in an orthogonal flow and its containing block's inline
    /// size, against which its percentage padding resolves (see `layout::writing_mode`).
    #[cfg(feature = "writing-mode")]
    pub(crate) orthogonal_percent_basis: Option<(crate::NodeId, f32)>,
    /// This rounding pass's nodes reached and subtrees skipped (see `layout::writing_mode`).
    #[cfg(feature = "writing-mode")]
    pub(crate) round_counts: (u64, u64),
    /// pyro: the multi-column container whose strip is being laid out and its content box width,
    /// while it has column spanners (`layout::multicol`).
    pub(crate) column_spanners: Option<(crate::NodeId, f32)>,
    /// pyro: the sizes `calc_size_inputs` measured for boxes this pass (box, axis 0 width or 1
    /// height, size): a parent that hands one back as a known size hands back the result.
    calc_size_results: Vec<(NodeId, u8, f32)>,
    /// pyro: the multi-column container whose strip is being laid out and the border-box height
    /// its content's percentage heights resolve against (its column height, `layout::multicol`).
    pub(crate) multicol_percent_height: Option<(crate::NodeId, f32)>,
    /// pyro: the inline roots this pass marked `FINAL_LAYOUT_STALE` (`mark_final_layout_stale`).
    pub(crate) stale_lines: Vec<crate::NodeId>,
}

impl<'doc> LayoutPassState<'doc> {
    /// The containing block's inline size when `dom_id` is the box currently laid out in an orthogonal
    /// flow: its percentage padding resolves against it (see `layout::writing_mode`).
    #[inline]
    fn orthogonal_percent_basis_of(&self, dom_id: crate::NodeId) -> Option<f32> {
        #[cfg(feature = "writing-mode")]
        {
            self.orthogonal_percent_basis
                .filter(|(node, _)| *node == dom_id)
                .map(|(_, basis)| basis)
        }
        #[cfg(not(feature = "writing-mode"))]
        {
            let _ = dom_id;
            None
        }
    }

    pub(crate) fn new(doc: &'doc mut BaseDocument) -> Self {
        #[cfg(feature = "writing-mode")]
        let root_id = doc.try_root_element().map(|root| root.id);
        Self {
            #[cfg(feature = "writing-mode")]
            root_wm: root_id.map_or(stylo_taffy::WritingMode::empty(), |id| {
                doc.root_layout_wm(id)
            }),
            doc,
            #[cfg(feature = "writing-mode")]
            root_id,
            #[cfg(feature = "writing-mode")]
            layout_wm: stylo_taffy::WritingMode::empty(),
            #[cfg(feature = "writing-mode")]
            current_align_axis_is_inline: false,
            #[cfg(feature = "writing-mode")]
            orthogonal_percent_basis: None,
            #[cfg(feature = "writing-mode")]
            round_counts: (0, 0),
            column_spanners: None,
            calc_size_results: Vec::new(),
            multicol_percent_height: None,
            stale_lines: Vec::new(),
        }
    }
}

impl Deref for LayoutPassState<'_> {
    type Target = BaseDocument;
    fn deref(&self) -> &BaseDocument {
        self.doc
    }
}

impl DerefMut for LayoutPassState<'_> {
    fn deref_mut(&mut self) -> &mut BaseDocument {
        self.doc
    }
}

impl LayoutPassState<'_> {
    fn node_from_id(&self, node_id: taffy::prelude::NodeId) -> &Node {
        &self.nodes[dom_node_id(node_id)]
    }
    fn node_from_id_mut(&mut self, node_id: taffy::prelude::NodeId) -> &mut Node {
        &mut self.nodes[dom_node_id(node_id)]
    }
}

impl LayoutPassState<'_> {
    /// Run the node's layout algorithm, then lay out the out-of-flow (absolute/fixed)
    /// boxes for which it is the containing block. Must be called inside the layout
    /// cache wrapper so that cache hits do not re-run the out-of-flow pass.
    fn compute_child_layout_internal(
        &mut self,
        node_id: NodeId,
        inputs: taffy::tree::LayoutInput,
        block_ctx: Option<&mut BlockContext<'_>>,
    ) -> taffy::tree::LayoutOutput {
        // pyro: every algorithm lays out its children through here (Taffy's
        // `compute_child_layout` and Blitz's block contexts alike), once per tree level, so a tree
        // of any depth lays out on a stack grown on demand instead of overflowing the thread's.
        // While it runs the box is `IN_LAYOUT` (see `mark_final_layout_stale`).
        let id = dom_node_id(node_id);
        let was_in_layout = self.nodes[id].flags.contains(NodeFlags::IN_LAYOUT);
        self.nodes[id].flags.insert(NodeFlags::IN_LAYOUT);
        let output = crate::util::ensure_stack(|| {
            #[cfg(feature = "writing-mode")]
            {
                self.compute_child_layout_in_own_wm(node_id, inputs, block_ctx)
            }
            #[cfg(not(feature = "writing-mode"))]
            {
                self.compute_child_layout_in_current_wm(node_id, inputs, block_ctx)
            }
        });
        self.nodes[id].flags.set(NodeFlags::IN_LAYOUT, was_in_layout);
        output
    }

    /// Lay out `node_id` in the writing mode of the currently running algorithm (its parent's)
    pub(crate) fn compute_child_layout_in_current_wm(
        &mut self,
        node_id: NodeId,
        inputs: taffy::tree::LayoutInput,
        block_ctx: Option<&mut BlockContext<'_>>,
    ) -> taffy::tree::LayoutOutput {
        // pyro: this runs once per tree level of a recursive layout; what is not part
        // of the recursion stays out of its frame (see `dispatch_child_layout`).
        let mut inputs = self.adjusted_child_inputs(node_id, inputs);
        // pyro: size containment (`Self::size_contained_size`) and inline-size containment
        // (`Self::inline_size_contained_width`)
        let (contain, calc_size) = match self.nodes[dom_node_id(node_id)].primary_styles() {
            Some(styles) => (Some(effective_size_containment(&styles)), has_calc_size(&styles)),
            None => (None, false),
        };
        if contain.is_some_and(|contain| contain.contains(style::values::computed::Contain::BLOCK_SIZE)) {
            if let Some(size) = self.size_contained_size(node_id, inputs) {
                if inputs.run_mode != RunMode::PerformLayout {
                    return taffy::tree::LayoutOutput::from_outer_size(size);
                }
                inputs.known_dimensions = size.map(Some);
            }
        } else if inputs.known_dimensions.width.is_none()
            && contain.is_some_and(|contain| contain.contains(style::values::computed::Contain::INLINE_SIZE))
            && let Some(width) = self.inline_size_contained_width(node_id, inputs)
        {
            if inputs.run_mode == RunMode::ComputeSize && inputs.axis == taffy::RequestedAxis::Horizontal {
                return taffy::tree::LayoutOutput::from_outer_size(taffy::Size { width, height: 0.0 });
            }
            inputs.known_dimensions.width = Some(width);
        }
        // pyro: `width`/`height: calc-size(<basis>, <calculation>)` (`Self::calc_size_inputs`)
        if calc_size {
            inputs = self.calc_size_inputs(node_id, inputs);
        }
        // pyro: a box that clamps its lines ends at the clamp line, which its laid-out
        // content names (`layout::line_clamp`): a measuring run lays it out too, and what
        // that leaves on its content is not the layout a cached final run stands for.
        let line_clamp = self.line_clamp_of(dom_node_id(node_id));
        let mut output = match line_clamp {
            Some(_) if inputs.run_mode == RunMode::ComputeSize => {
                let performed = taffy::tree::LayoutInput { run_mode: RunMode::PerformLayout, ..inputs };
                let output = self.dispatch_child_layout(node_id, performed, block_ctx);
                self.nodes[dom_node_id(node_id)].clear_layout_cache();
                output
            }
            _ => self.dispatch_child_layout(node_id, inputs, block_ctx),
        };
        match line_clamp {
            Some(max_lines) => self.clamp_lines(dom_node_id(node_id), max_lines, inputs, &mut output),
            None if self.nodes[dom_node_id(node_id)].flags.contains(NodeFlags::CLAMPS_LINES) => {
                self.unclamp_lines(dom_node_id(node_id))
            }
            None => {}
        }
        if inputs.run_mode == RunMode::PerformLayout {
            // pyro: a table cell aligned on its row's baseline has its content lower in its box.
            if self.nodes[dom_node_id(node_id)].flags.contains(crate::node::NodeFlags::HAS_TABLE_BASELINE_OFFSET) {
                table::shift_cell_content(self, dom_node_id(node_id), &mut output);
            }
            self.lay_out_out_of_flow_boxes(node_id, &mut output);
        }
        output
    }

    /// pyro: the inputs of a box whose `width` or `height` is `calc-size(<basis>, <calculation>)`
    /// (CSS Values 5 §6.1). Taffy lays the box out as its basis (`stylo_taffy::convert::dimension`);
    /// the size that gives (the size its parent resolved it to, else the box's own size measured
    /// with the basis) goes through the calculation, as the `box-sizing` box, and the result,
    /// between `min-*` and `max-*`, becomes a known size. In a horizontal writing mode only.
    #[inline(never)]
    fn calc_size_inputs(&mut self, node_id: NodeId, mut inputs: taffy::tree::LayoutInput) -> taffy::tree::LayoutInput {
        use style::values::computed::{LengthPercentage, Size as StyloSize};
        use taffy::{MaybeMath as _, MaybeResolve as _, ResolveOrZero as _};
        let node = &self.nodes[dom_node_id(node_id)];
        let Some(styles) = node.primary_styles() else { return inputs };
        if !styles.writing_mode.is_horizontal() {
            return inputs;
        }
        let calculation = |size: &StyloSize| -> Option<LengthPercentage> {
            match size {
                StyloSize::CalcSizeAuto(lp)
                | StyloSize::CalcSizeFitContent(lp)
                | StyloSize::CalcSizeMinContent(lp)
                | StyloSize::CalcSizeMaxContent(lp) => Some(lp.0.clone()),
                _ => None,
            }
        };
        // An `auto` or `fit-content` basis shrinks to fit when its parent measures it.
        let shrinks = matches!(styles.get_position().width, StyloSize::CalcSizeAuto(_) | StyloSize::CalcSizeFitContent(_));
        let position = styles.get_position();
        let mut functions = taffy::Size { width: calculation(&position.width), height: calculation(&position.height) };
        if functions.width.is_none() && functions.height.is_none() {
            return inputs;
        }
        let content_box = styles.get_position().box_sizing == style::computed_values::box_sizing::T::ContentBox;
        drop(styles);
        let style = node.layout_style();
        let parent_width = inputs.parent_size.width;
        let padding_border = style.padding().resolve_or_zero(parent_width, resolve_calc_value).sum_axes()
            + style.border().resolve_or_zero(parent_width, resolve_calc_value).sum_axes();
        let box_inset = if content_box { padding_border } else { taffy::Size::ZERO };
        let min = style.min_size().maybe_resolve(inputs.parent_size, resolve_calc_value).maybe_add(box_inset);
        let max = style.max_size().maybe_resolve(inputs.parent_size, resolve_calc_value).maybe_add(box_inset);
        drop(style);
        let missing = inputs.known_dimensions.width.is_none() && functions.width.is_some()
            || inputs.known_dimensions.height.is_none() && functions.height.is_some();
        let measured = missing.then(|| {
            let probe = taffy::tree::LayoutInput {
                run_mode: RunMode::ComputeSize,
                sizing_mode: taffy::SizingMode::InherentSize,
                axis: taffy::RequestedAxis::Both,
                ..inputs
            };
            self.dispatch_child_layout(node_id, probe, None).size
        });
        let apply = |function: &LengthPercentage, size: f32, inset: f32| -> f32 {
            let basis = (size - inset).max(0.0);
            function.resolve(CSSPixelLength::new(basis)).px().max(0.0) + inset
        };
        let given = |state: &Self, axis: u8, known: Option<f32>| {
            known.is_some_and(|known| state.calc_size_results.iter().any(|&(id, a, size)| id == node_id && a == axis && size == known))
        };
        let known_width = inputs.known_dimensions.width;
        if functions.width.is_some() && given(self, 0, known_width) {
            functions.width = None;
        }
        if functions.height.is_some() && given(self, 1, inputs.known_dimensions.height) {
            functions.height = None;
        }
        if let Some(function) = &functions.width {
            let size = inputs.known_dimensions.width.or(measured.map(|size| size.width)).unwrap_or(0.0);
            let mut width = apply(function, size, box_inset.width);
            // A size measured in definite space is the box's fit-content size: the space between
            // the calculation of its min-content and max-content sizes (as Blink), not the
            // calculation of the fit-content size of its basis.
            if let (true, None, AvailableSpace::Definite(space)) =
                (shrinks, inputs.known_dimensions.width, inputs.available_space.width)
            {
                let mut content = |available: AvailableSpace| {
                    let probe = taffy::tree::LayoutInput {
                        run_mode: RunMode::ComputeSize,
                        sizing_mode: taffy::SizingMode::InherentSize,
                        axis: taffy::RequestedAxis::Both,
                        available_space: taffy::Size { width: available, ..inputs.available_space },
                        ..inputs
                    };
                    apply(function, self.dispatch_child_layout(node_id, probe, None).size.width, box_inset.width)
                };
                let min_content = content(AvailableSpace::MinContent);
                let max_content = content(AvailableSpace::MaxContent).max(min_content);
                width = space.clamp(min_content, max_content);
            }
            let width = width.maybe_min(max.width).maybe_max(min.width);
            inputs.known_dimensions.width = Some(width.max(padding_border.width));
            if known_width.is_none() {
                self.calc_size_results.push((node_id, 0, width.max(padding_border.width)));
            }
        }
        if let Some(function) = &functions.height {
            let size = inputs.known_dimensions.height.or(measured.map(|size| size.height)).unwrap_or(0.0);
            let height = apply(function, size, box_inset.height).maybe_min(max.height).maybe_max(min.height);
            if inputs.known_dimensions.height.is_none() {
                self.calc_size_results.push((node_id, 1, height.max(padding_border.height)));
            }
            inputs.known_dimensions.height = Some(height.max(padding_border.height));
        }
        inputs
    }

    /// pyro: the size of a box with size containment (`contain: size`, css-contain-2 §3.1): the
    /// size it has as an empty box whose content is `contain-intrinsic-size` (css-sizing-4 §4.1;
    /// `none` is zero, `auto <length>` the length: no size is remembered). Its content is then
    /// laid out in a box of that size and never changes it. `None`: the box has no size
    /// containment (a non-atomic inline box, a table, an internal table box).
    #[inline(never)]
    fn size_contained_size(&self, node_id: NodeId, inputs: taffy::tree::LayoutInput) -> Option<taffy::Size<f32>> {
        use style::values::computed::Contain;
        use style::values::generics::box_::ContainIntrinsicSize;
        use style::values::specified::box_::{DisplayInside, DisplayOutside};
        let node = &self.nodes[dom_node_id(node_id)];
        let content = {
            let styles = node.primary_styles()?;
            if !effective_size_containment(&styles).contains(Contain::INLINE_SIZE | Contain::BLOCK_SIZE) {
                return None;
            }
            let display = styles.clone_display();
            let contained = match display.outside() {
                DisplayOutside::Inline => display.inside() != DisplayInside::Flow,
                DisplayOutside::Block => true,
                _ => false,
            } && display.inside() != DisplayInside::Table;
            if !contained {
                return None;
            }
            let length = |size: &style::values::computed::ContainIntrinsicSize| match size {
                ContainIntrinsicSize::None | ContainIntrinsicSize::AutoNone => 0.0,
                ContainIntrinsicSize::Length(length) | ContainIntrinsicSize::AutoLength(length) => length.0.px(),
            };
            let position = styles.get_position();
            let (width, height) = (length(&position.contain_intrinsic_width), length(&position.contain_intrinsic_height));
            #[cfg(feature = "writing-mode")]
            let vertical = self.layout_wm.is_vertical();
            #[cfg(not(feature = "writing-mode"))]
            let vertical = false;
            if vertical { taffy::Size { width: height, height: width } } else { taffy::Size { width, height } }
        };
        let style = self.child_layout_style(node);
        // A replaced element keeps its natural aspect ratio (its natural sizes are gone): a size
        // in one axis gives the other (WPT css-flexbox `canvas-contain-size`).
        let natural_ratio = node.data.downcast_element().and_then(replaced::natural_ratio).map(|ratio| {
            #[cfg(feature = "writing-mode")]
            let vertical = self.layout_wm.is_vertical();
            #[cfg(not(feature = "writing-mode"))]
            let vertical = false;
            if vertical { 1.0 / ratio } else { ratio }
        });
        let output = taffy::compute_leaf_layout(
            taffy::tree::LayoutInput { run_mode: RunMode::ComputeSize, ..inputs },
            &style,
            resolve_calc_value,
            |known_dimensions, _available_space| match (known_dimensions.width, known_dimensions.height, natural_ratio) {
                (Some(width), None, Some(ratio)) => taffy::Size { width, height: width / ratio },
                (None, Some(height), Some(ratio)) => taffy::Size { width: height * ratio, height },
                (width, height, _) => taffy::Size {
                    width: width.unwrap_or(content.width),
                    height: height.unwrap_or(content.height),
                },
            },
        );
        Some(output.size)
    }

    /// pyro: the width of a box with inline-size containment (`contain: inline-size`,
    /// css-contain-2 §3.2) that its parent leaves to it (a shrink-to-fit box, or one measured for
    /// its min-content or max-content contribution): its width as an empty box whose content is
    /// `contain-intrinsic-width` wide (`none` is zero). Its content is then laid out at that width,
    /// and its height follows from it. A truncating `display: block` line with `contain:
    /// inline-size` in a column flex container (Tailwind's `[contain:inline-size]`, Danex's list
    /// cells) gives the container none of its width, as in Chrome. `None`: no inline-size
    /// containment applies (as for `size_contained_size`), and replaced elements and vertical
    /// writing modes are left as they were.
    #[inline(never)]
    fn inline_size_contained_width(&self, node_id: NodeId, inputs: taffy::tree::LayoutInput) -> Option<f32> {
        use style::values::generics::box_::ContainIntrinsicSize;
        use style::values::specified::box_::{DisplayInside, DisplayOutside};
        #[cfg(feature = "writing-mode")]
        if self.layout_wm.is_vertical() {
            return None;
        }
        let node = &self.nodes[dom_node_id(node_id)];
        if node.data.downcast_element().is_some_and(replaced::is_replaced) {
            return None;
        }
        let content_width = {
            let styles = node.primary_styles()?;
            if styles.writing_mode.is_vertical() {
                return None;
            }
            let display = styles.clone_display();
            let contained = match display.outside() {
                DisplayOutside::Inline => display.inside() != DisplayInside::Flow,
                DisplayOutside::Block => true,
                _ => false,
            } && display.inside() != DisplayInside::Table;
            if !contained {
                return None;
            }
            match &styles.get_position().contain_intrinsic_width {
                ContainIntrinsicSize::None | ContainIntrinsicSize::AutoNone => 0.0,
                ContainIntrinsicSize::Length(length) | ContainIntrinsicSize::AutoLength(length) => length.0.px(),
            }
        };
        let style = self.child_layout_style(node);
        let output = taffy::compute_leaf_layout(
            taffy::tree::LayoutInput {
                run_mode: RunMode::ComputeSize,
                axis: taffy::RequestedAxis::Horizontal,
                ..inputs
            },
            &style,
            resolve_calc_value,
            |known_dimensions, _available_space| taffy::Size {
                width: known_dimensions.width.unwrap_or(content_width),
                height: known_dimensions.height.unwrap_or(0.0),
            },
        );
        Some(output.size.width)
    }

    #[inline(never)]
    fn lay_out_out_of_flow_boxes(&mut self, node_id: NodeId, output: &mut taffy::tree::LayoutOutput) {
        let dom_id = dom_node_id(node_id);
        // pyro: a positioned inline box split around block-level content (`NodeFlags::IS_SPLIT_INLINE`)
        // lays out no absolutely positioned box itself: their containing block is the box its
        // fragments bound, which the tail after its box ends, so its parent lays them out once both
        // are laid out (`split_inline_containing_block`).
        if self.nodes[dom_id].is_split_inline() && self.split_inline_contains_absolute(dom_id) {
            if output.oof_candidates.iter().all(|candidate| {
                candidate.containing_block.is_none() && candidate.position == taffy::Position::Absolute
            }) {
                self.clear_hoisted_children(node_id);
                return;
            }
        }
        if !output.oof_candidates.is_empty() && self.has_positioned_split_inline_child(dom_id) {
            for index in 0..output.oof_candidates.as_mut_slice().len() {
                let candidate = output.oof_candidates.as_mut_slice()[index];
                if candidate.containing_block.is_some() || candidate.position != taffy::Position::Absolute {
                    continue;
                }
                let Some(span) = self.split_inline_holding(dom_id, dom_node_id(candidate.node)) else { continue };
                if let Some(cb) = self.split_inline_containing_block(dom_id, span) {
                    output.oof_candidates.as_mut_slice()[index].containing_block = Some(cb);
                }
            }
        }
        compute_oof_layout(self, node_id, output);
    }

    /// The inputs `node_id` is laid out with (see [`Self::compute_child_layout_in_current_wm`]).
    #[inline(never)]
    fn adjusted_child_inputs(&self, node_id: NodeId, inputs: taffy::tree::LayoutInput) -> taffy::tree::LayoutInput {
        let mut inputs = inputs;
        // pyro: a column spanner is as wide as its multi-column container (`layout::multicol`).
        if self.column_spanners.is_some() {
            inputs = self.column_spanner_inputs(dom_node_id(node_id), inputs);
        }
        // stylo_taffy cannot represent intrinsic keywords in min_size(), so
        // min-width:max-content reaches Taffy as auto. Its flex minimum-size
        // query consequently requests min-content. Preserve the authored
        // keyword here before measuring the node's descendants.
        if inputs.sizing_mode == taffy::SizingMode::ContentSize
            && inputs.axis == taffy::RequestedAxis::Horizontal
            && inputs.available_space.width == AvailableSpace::MinContent
            && self.nodes[dom_node_id(node_id)].primary_styles().is_some_and(|style| {
                !style.writing_mode.is_vertical()
                    && matches!(style.get_min_width(), style::values::computed::Size::MaxContent)
            })
        {
            inputs.available_space.width = AvailableSpace::MaxContent;
        }
        // pyro: a list box (`<select multiple>` or `size` > 1) with
        // an auto height is `size` rows tall (4 for `multiple`), however many
        // options it has, as in Chromium: its options scroll inside.
        if inputs.known_dimensions.height.is_none() {
            if let Some(height) = self.listbox_height(dom_node_id(node_id), inputs.parent_size.width) {
                inputs.known_dimensions.height = Some(height);
            }
        }
        if inputs.run_mode == RunMode::PerformLayout {
            let node = &self.nodes[dom_node_id(node_id)];
            let is_root = node.parent.is_some_and(|parent| self.nodes[parent].parent.is_none());
            crate::resolve::phase_profile::performed(is_root);
        }
        inputs
    }

    /// The border-box height of a list-box `<select>` whose height is auto:
    /// `size` rows (4 for `multiple`) of its first option's height (a line
    /// of the select's font with nothing to measure), plus its padding and
    /// border. Chromium's `LayoutListBox` sizing; `min-`/`max-height` clamp.
    fn listbox_height(&self, id: blitz_traits::NodeId, parent_width: Option<f32>) -> Option<f32> {
        let node = &self.nodes[id];
        let el = node.data.downcast_element()?;
        if el.name.local != local_name!("select") {
            return None;
        }
        let size = el.attr(local_name!("size")).and_then(|s| s.trim().parse::<u32>().ok());
        let rows = match (el.attr(local_name!("multiple")).is_some(), size) {
            (_, Some(n)) if n > 1 => n,
            (true, None | Some(0)) => 4,
            (true, Some(_)) => 1,
            (false, _) => return None,
        };
        let style = node.primary_styles()?;
        if style.writing_mode.is_vertical()
            || stylo_taffy::convert::dimension(&style.slow_clone_height()).into_option().is_some()
        {
            return None;
        }
        let line = |s: &style::properties::ComputedValues| {
            use style::values::computed::font::LineHeight;
            let font_size = s.slow_clone_font_size().used_size().px();
            match s.clone_line_height() {
                // As the option's line box: whole pixels.
                LineHeight::Normal => (font_size * 1.2).round(),
                LineHeight::Number(num) => font_size * num.0,
                LineHeight::Length(value) => value.0.px(),
            }
        };
        let edges = |n: &Node| {
            let st = n.layout_style();
            let pb = st.padding().resolve_or_zero(parent_width, resolve_calc_value)
                + st.border().resolve_or_zero(parent_width, resolve_calc_value);
            pb.vertical_axis_sum()
        };
        // The first option that is displayed, in an optgroup too.
        let mut first = None;
        let mut stack: Vec<blitz_traits::NodeId> = node.children.iter().rev().copied().collect();
        while let Some(c) = stack.pop() {
            let n = &self.nodes[c];
            let Some(e) = n.data.downcast_element() else { continue };
            if n.primary_styles().is_none_or(|s| s.get_box().display.is_none()) {
                continue;
            }
            if e.name.local == local_name!("option") {
                first = Some(c);
                break;
            }
            if e.name.local == local_name!("optgroup") {
                stack.extend(n.children.iter().rev().copied());
            }
        }
        let item = match first {
            Some(o) => {
                let o = &self.nodes[o];
                let st = o.primary_styles()?;
                let min = o.layout_style().min_size().height.resolve_or_zero(parent_width, resolve_calc_value);
                line(&st).max(min) + edges(o)
            }
            None => line(&style),
        };
        let height = rows as f32 * item + edges(node);
        let st = node.layout_style();
        let min = st.min_size().height.resolve_or_zero(parent_width, resolve_calc_value);
        let max = st.max_size().height;
        let max = if max.is_auto() { f32::INFINITY } else { max.resolve_or_zero(parent_width, resolve_calc_value) };
        Some(height.max(min).min(max))
    }

    /// The style of a node being laid out *by* the node whose algorithm is currently running,
    /// expressed in that node's writing mode (see `layout::writing_mode`).
    #[inline]
    pub(crate) fn child_layout_style<'a>(
        &self,
        node: &'a Node,
    ) -> stylo_taffy::TaffyStyloStyle<ComputedStyleRef<'a>> {
        #[cfg(feature = "writing-mode")]
        {
            let mut style = node.layout_style_in(self.layout_wm);
            style.align_axis_is_inline = self.current_align_axis_is_inline;
            style
        }
        #[cfg(not(feature = "writing-mode"))]
        {
            node.layout_style()
        }
    }

    /// pyro: the layout algorithms recurse through here once per tree level, so this
    /// frame is kept small: the leaf boxes (form controls, replaced elements) and tables are
    /// laid out by non-inlined functions whose (large) frames are gone before a container's
    /// algorithm runs, so a deep tree costs only the algorithms' own frames per level.
    fn dispatch_child_layout(
        &mut self,
        node_id: NodeId,
        inputs: taffy::tree::LayoutInput,
        block_ctx: Option<&mut BlockContext<'_>>,
    ) -> taffy::tree::LayoutOutput {
        match self.leaf_layout_or_algorithm(node_id, inputs) {
            ChildLayout::Done(output) => output,
            ChildLayout::Block => compute_block_layout(self, node_id, inputs, block_ctx),
            ChildLayout::FlowRoot => compute_block_layout(self, node_id, inputs, None),
            ChildLayout::Inline => self.inline_layout(node_id, inputs, block_ctx),
            ChildLayout::Flex => self.flexbox_layout(node_id, inputs),
            ChildLayout::Grid => self.grid_layout(node_id, inputs),
            ChildLayout::Table => self.compute_table_root_layout(node_id, inputs),
            ChildLayout::Multicol { inline_root } => self.compute_multicol_layout(node_id, inputs, inline_root),
        }
    }

    // pyro: the algorithms other than block layout, not inlined into
    // `dispatch_child_layout` (whose frame every tree level pays).
    #[inline(never)]
    fn inline_layout(
        &mut self,
        node_id: NodeId,
        inputs: taffy::tree::LayoutInput,
        block_ctx: Option<&mut BlockContext<'_>>,
    ) -> taffy::tree::LayoutOutput {
        self.compute_inline_layout(dom_node_id(node_id), inputs, block_ctx)
    }

    #[inline(never)]
    fn flexbox_layout(&mut self, node_id: NodeId, inputs: taffy::tree::LayoutInput) -> taffy::tree::LayoutOutput {
        compute_flexbox_layout(self, node_id, inputs)
    }

    #[inline(never)]
    fn grid_layout(&mut self, node_id: NodeId, inputs: taffy::tree::LayoutInput) -> taffy::tree::LayoutOutput {
        compute_grid_layout(self, node_id, inputs)
    }

    /// The layout of a leaf box (text, form controls, replaced elements, hidden boxes), or the
    /// algorithm that lays out the container (see [`Self::dispatch_child_layout`]).
    #[inline(never)]
    fn leaf_layout_or_algorithm(&mut self, node_id: NodeId, inputs: taffy::tree::LayoutInput) -> ChildLayout {
        let orthogonal_percent_basis = self.orthogonal_percent_basis_of(dom_node_id(node_id));
        let scale = self.doc.viewport.scale();
        // pyro: the longest option label of a drop-down select (the
        // control's intrinsic width; its options are not laid out).
        let select_label_chars = {
            let n = &self.nodes[dom_node_id(node_id)];
            n.data.downcast_element().filter(|el| el.name.local == local_name!("select")).map(|el| {
                let listbox = el.attr(local_name!("multiple")).is_some()
                    || el.attr(local_name!("size")).and_then(|s| s.trim().parse::<u32>().ok()).is_some_and(|s| s > 1);
                let longest = n
                    .children
                    .iter()
                    .map(|&c| &self.nodes[c])
                    .filter(|c| c.data.downcast_element().is_some_and(|e| e.name.local == local_name!("option")))
                    .map(|c| c.text_content().trim().chars().count())
                    .max()
                    .unwrap_or(0);
                (listbox, longest)
            })
        };
        // A drop-down select's text line (its font at `line-height: normal`):
        // the control's baseline is the line's, centred in the content box.
        let select_line = if matches!(select_label_chars, Some((false, _))) {
            let doc = &mut *self.doc;
            let styles = doc.nodes[dom_node_id(node_id)].primary_styles();
            styles.and_then(|styles| {
                let parley_style = crate::stylo_to_parley::style(dom_node_id(node_id), &styles);
                let mut font_ctx = doc.font_ctx.lock().unwrap();
                let mut builder = doc.layout_ctx.tree_builder(&mut font_ctx, scale, true, &parley_style);
                builder.push_text("x");
                let mut layout = builder.build().0;
                layout.break_all_lines(None);
                layout.lines().next().map(|line| *line.metrics())
            })
        } else {
            None
        };
        let node = &mut self.nodes[dom_node_id(node_id)];

        let font_styles = node.primary_styles().map(|style| {
            use style::values::computed::font::LineHeight;

            let font_size = style.slow_clone_font_size().used_size().px();
            let line_height = match style.clone_line_height() {
                LineHeight::Normal => font_size * 1.2,
                LineHeight::Number(num) => font_size * num.0,
                LineHeight::Length(value) => value.0.px(),
            };

            (font_size, line_height)
        });
        let font_size = font_styles.map(|s| s.0);
        // In system-ui (SF Text on macOS) Chromium's width is
        // exactly ceil(0.525em x size) + round(0.4375em) (measured
        // at 10-16px; SF Display from 20px measures otherwise).
        let system_ui = cfg!(target_os = "macos")
            && node.primary_styles().is_some_and(|s| {
                use style::values::computed::font::{GenericFontFamily, SingleFontFamily};
                match s.get_font().font_family.families.iter().next() {
                    Some(SingleFontFamily::Generic(GenericFontFamily::SystemUi)) => true,
                    Some(SingleFontFamily::FamilyName(name)) => matches!(
                        name.name.as_ref(),
                        "ui-sans-serif" | "-apple-system" | "BlinkMacSystemFont"
                    ),
                    _ => false,
                }
            });
        let resolved_line_height = font_styles.map(|s| s.1);
        // A form control with a cyclic percentage width contributes zero to
        // min-content sizing (CSS Sizing 3 §5.2.2, compressible): text
        // inputs, textareas and selects shrink into a flex-wrap line. This is
        // the measured content contribution, not a preferred width:
        // ContentSize measurements ignore preferred-width styles. Auto and
        // max-content sizing keep the intrinsic width; known dimensions and
        // explicit minima remain the leaf/parent layout algorithm's.
        let compressible_min_content = inputs.available_space.width == AvailableSpace::MinContent
            && inputs.parent_size.width.is_none()
            && matches!(node.layout_style().size().width.expand(), taffy::style::ExpandedDimension::Percent(_));

        // pyro: the root `<svg>` of an SVG document keeps usvg's size (see the `Svg` arm below).
        let is_root_element = node.is_root_element();
        let content_sized = node.primary_styles().is_some_and(|style| {
            style.get_ui().field_sizing == style::values::computed::FieldSizing::Content
        });
        match &mut node.data {
            NodeData::Text(data) => {
                // With the new "inline context" architecture all text nodes should be wrapped in an "inline layout context"
                // and should therefore never be measured individually.
                #[cfg(feature = "tracing")]
                tracing::error!(
                    node_id = ?dom_node_id(node_id),
                    data = ?data,
                    "Tried to lay out text node individually",
                );

                #[cfg(not(feature = "tracing"))]
                let _ = data;

                ChildLayout::Done(taffy::LayoutOutput::HIDDEN)
                // unreachable!();

                // compute_leaf_layout(inputs, &node.style, |known_dimensions, available_space| {
                //     let context = TextContext {
                //         text_content: &data.content.trim(),
                //         writing_mode: WritingMode::Horizontal,
                //     };
                //     let font_metrics = FontMetrics {
                //         char_width: 8.0,
                //         char_height: 16.0,
                //     };
                //     text_measure_function(
                //         known_dimensions,
                //         available_space,
                //         &context,
                //         &font_metrics,
                //     )
                // })
            }
            NodeData::Element(element_data) | NodeData::AnonymousBlock(element_data) => {
                // TODO: deduplicate with single-line text input
                if *element_data.name.local == *"textarea" {
                    let rows = element_data
                        .attr(local_name!("rows"))
                        .and_then(|val| val.parse::<f32>().ok())
                        .unwrap_or(2.0);

                    let cols = element_data
                        .attr(local_name!("cols"))
                        .and_then(|val| val.parse::<f32>().ok());

                    // `field-sizing: content` measures the editor's shaped text (or its
                    // placeholder), not the HTML rows/cols hints. Use a private layout for
                    // intrinsic measurement: a min-content probe must not change the editor
                    // that paint and caret geometry share.
                    let mut content_layout = content_sized
                        .then(|| {
                            element_data.text_input_data().and_then(|input| {
                                input.shown_placeholder().map(|p| p.layout.clone())
                                    .or_else(|| input.editor.try_layout().cloned())
                            })
                        })
                        .flatten();
                    let output = compute_leaf_layout(
                        inputs,
                        &node.layout_style(),
                        resolve_calc_value,
                        |_known_size, available_space| {
                            if content_sized {
                                let line_height = resolved_line_height.unwrap_or(16.0);
                                if let Some(layout) = content_layout.as_mut() {
                                    let width = match available_space.width {
                                        taffy::AvailableSpace::Definite(width) => Some(width.max(0.0) * scale),
                                        taffy::AvailableSpace::MinContent => Some(0.0),
                                        taffy::AvailableSpace::MaxContent => None,
                                    };
                                    layout.break_all_lines(width);
                                    return taffy::Size {
                                        width: (layout.width() / scale).max(1.0),
                                        height: (layout.height() / scale).max(line_height),
                                    };
                                }
                                return taffy::Size { width: 1.0, height: line_height };
                            }
                            taffy::Size {
                                width: if compressible_min_content {
                                    0.0
                                } else {
                                    cols.map(|cols| cols * font_size.unwrap_or(16.0) * 0.6).unwrap_or(300.0)
                                },
                                height: resolved_line_height.unwrap_or(16.0) * rows,
                            }
                        },
                    );
                    if inputs.run_mode == RunMode::PerformLayout {
                        let pb = {
                            let style = node.layout_style();
                            style
                                .padding()
                                .resolve_or_zero(inputs.parent_size.width, resolve_calc_value)
                                + style
                                    .border()
                                    .resolve_or_zero(inputs.parent_size.width, resolve_calc_value)
                        };
                        let content_width = (output.size.width - pb.horizontal_axis_sum()).max(0.0);
                        let doc = &mut *self.doc;
                        let scale = doc.viewport.scale();
                        let node = &mut doc.nodes[dom_node_id(node_id)];
                        if let Some(input) = node
                            .data
                            .downcast_element_mut()
                            .and_then(|el| el.text_input_data_mut())
                        {
                            input.editor.set_width(Some(content_width * scale));
                            input.editor.refresh_layout(
                                &mut doc.font_ctx.lock().unwrap(),
                                &mut doc.layout_ctx,
                            );
                            // pyro: the placeholder wraps at the same width.
                            if let Some(placeholder) = input.placeholder.as_deref_mut() {
                                placeholder.break_lines(Some(content_width * scale));
                            }
                            // pyro: lines that do not wrap (`white-space: nowrap`, `wrap=off`)
                            // are the control's scrollable overflow: it scrolls to them.
                            let text_width = input.editor.try_layout().map_or(0.0, |layout| layout.width() / scale);
                            if text_width > content_width {
                                let mut output = output;
                                let overflow = output.scrollable_overflow_rect;
                                let mut rect = overflow.rect();
                                rect.right = rect.right.max(pb.left + text_width + pb.right);
                                output.scrollable_overflow_rect =
                                    taffy::ScrollableOverflow::new(rect, overflow.is_reversed());
                                return ChildLayout::Done(output);
                            }
                        }
                    }
                    return ChildLayout::Done(output);
                }

                // pyro: a drop-down select is an atomic box: its
                // longest option's label (about half an em per character)
                // plus the arrow, and, as Chromium's menu list, a line of
                // the control's font (`line-height: normal`) plus 2px tall,
                // the line half of that lower (rounded down), its baseline
                // the select's. Chromium's own control font (the UA's
                // `system-ui, sans-serif` here) is a pixel shorter than
                // `system-ui`: 1px there. Without this it had no box at all
                // (its options are hidden).
                if let Some((false, chars)) = select_label_chars {
                    let font_size = font_size.unwrap_or(13.333);
                    let style = node.layout_style();
                    let line_height = select_line.map(|line| line.line_height / scale);
                    let extra = if node.primary_styles().is_some_and(|st| {
                        use style::values::computed::font::{GenericFontFamily, SingleFontFamily};
                        let list = &st.get_font().font_family.families;
                        let names: Vec<_> = list.iter().collect();
                        matches!(
                            names.as_slice(),
                            [SingleFontFamily::Generic(GenericFontFamily::SystemUi), SingleFontFamily::Generic(GenericFontFamily::SansSerif)]
                        )
                    }) {
                        1.0
                    } else {
                        2.0
                    };
                    let mut output = compute_leaf_layout(
                        inputs,
                        &style,
                        resolve_calc_value,
                        |_known_size, _available_space| taffy::Size {
                            width: if compressible_min_content {
                                0.0
                            } else {
                                (chars as f32 * 0.5 * font_size + 21.0).ceil()
                            },
                            height: line_height.map_or((font_size * 1.25).ceil(), |h| h + extra),
                        },
                    );
                    if let Some(line) = select_line {
                        let parent = inputs.parent_size.width;
                        let padding = style.padding().resolve_or_zero(parent, resolve_calc_value);
                        let border = style.border().resolve_or_zero(parent, resolve_calc_value);
                        let content_height = output.size.height - padding.vertical_axis_sum() - border.vertical_axis_sum();
                        let offset = ((content_height - line.line_height / scale) / 2.0).floor().max(0.0);
                        let baseline = padding.top + border.top + offset + line.baseline / scale;
                        output.baselines = taffy::Baselines::new(Some(baseline), Some(baseline));
                    }
                    return ChildLayout::Done(output);
                }

                if *element_data.name.local == *"input" {
                    match construct::input_type_keyword(element_data.attr(local_name!("type"))) {
                        // if the input type is hidden, hide it
                        Some("hidden") => {
                            return ChildLayout::Done(taffy::LayoutOutput::HIDDEN);
                        }
                        // pyro: a checkbox or radio is 13x13 by nature (Blink's theme size), not by
                        // the UA sheet's `width`/`height`: a flex parent stretches it and a percentage
                        // sizes it (WPT css-flexbox `stretch-flex-item-checkbox-input`). The 13x13 is the
                        // border box: the content is what padding and border leave of it (the UA sheet's
                        // `input` padding and border would otherwise make a checkbox 21x19 and a radio 17x15).
                        Some("checkbox" | "radio") => {
                            let style = node.layout_style();
                            let parent = inputs.parent_size.width;
                            let padding = style.padding().resolve_or_zero(parent, resolve_calc_value);
                            let border = style.border().resolve_or_zero(parent, resolve_calc_value);
                            let natural = taffy::Size {
                                width: (13.0 - padding.horizontal_axis_sum() - border.horizontal_axis_sum()).max(0.0),
                                height: (13.0 - padding.vertical_axis_sum() - border.vertical_axis_sum()).max(0.0),
                            };
                            return ChildLayout::Done(compute_leaf_layout(
                                inputs,
                                &style,
                                resolve_calc_value,
                                |_known_size, _available_space| natural,
                            ));
                        }
                        None | Some("text" | "password" | "email" | "tel" | "url" | "search" | "time") => {
                            // Chromium's intrinsic width: `size` (default 20) average
                            // characters plus a max-char allowance, about
                            // (size / 2 + 0.4)em (166px for 16px Arial). It does not
                            // shrink to the available space: inputs keep it in flex rows
                            // unless given `min-width: 0`.
                            let size = element_data
                                .attr(local_name!("size"))
                                .and_then(|val| val.trim().parse::<u32>().ok())
                                .filter(|&size| size > 0)
                                .unwrap_or(20) as f32;
                            let font_size = font_size.unwrap_or(13.333);
                            let width = if system_ui && font_size < 20.0 {
                                (0.525 * font_size * size).ceil() + (0.4375 * font_size).round()
                            } else {
                                (size * 0.5 + 0.4) * font_size
                            };
                            let line = element_data
                                .text_input_data()
                                .and_then(|input| input.editor.try_layout())
                                .and_then(|layout| layout.lines().next())
                                .map(|line| *line.metrics());
                            let style = node.layout_style();
                            let contribution_width = if compressible_min_content { 0.0 } else { width };
                            // pyro: as tall as its text's line, which is `normal`
                            // for a line height of no more than the font size.
                            let line_height = line
                                .filter(|_| {
                                    node.primary_styles().is_some_and(|s| {
                                        !matches!(s.clone_line_height(), style::values::computed::font::LineHeight::Normal)
                                            && construct::input_line_is_normal(&s)
                                    })
                                })
                                .map(|line| line.line_height / scale)
                                .or(resolved_line_height);
                            let mut output = compute_leaf_layout(
                                inputs,
                                &style,
                                resolve_calc_value,
                                |_known_size, _available_space| taffy::Size {
                                    width: contribution_width,
                                    height: line_height.unwrap_or(16.0),
                                },
                            );
                            // Its baseline is its text's, the line centred in the content
                            // box (Chromium), not its bottom edge.
                            if let Some(line) = line {
                                let parent = inputs.parent_size.width;
                                let padding = style.padding().resolve_or_zero(parent, resolve_calc_value);
                                let border = style.border().resolve_or_zero(parent, resolve_calc_value);
                                let content_top = padding.top + border.top;
                                let content_height =
                                    output.size.height - content_top - padding.bottom - border.bottom;
                                let (line_height, baseline) = (line.line_height / scale, line.baseline / scale);
                                let baseline = content_top + (content_height - line_height) / 2.0 + baseline;
                                output.baselines = taffy::Baselines::new(Some(baseline), Some(baseline));
                            }
                            return ChildLayout::Done(output);
                        }
                        _ => {}
                    }
                }

                if is_replaced(element_data) {
                    // Width/height attributes are presentational hints mapped to the CSS
                    // width/height properties by
                    // `synthesize_presentational_hints_for_legacy_attributes`, so they
                    // are already part of the style. They are only read here for the
                    // elements whose attributes determine their *intrinsic* size
                    // (canvas, and custom widgets on canvas tags).
                    let attr_size = taffy::Size {
                        width: element_data
                            .attr(local_name!("width"))
                            .and_then(|val| val.parse::<f32>().ok()),
                        height: element_data
                            .attr(local_name!("height"))
                            .and_then(|val| val.parse::<f32>().ok()),
                    };

                    // Get the element's intrinsic dimensions and default object size
                    let (intrinsic_sizes, default_object_size) = match &element_data.special_data {
                        SpecialElementData::Image(image_data) => match &**image_data {
                            ImageData::Raster(image) => {
                                let (width, height) = (image.width as f32, image.height as f32);
                                (
                                    IntrinsicSizes {
                                        width: Some(width),
                                        height: Some(height),
                                        ratio: Some(width / height),
                                    },
                                    DEFAULT_OBJECT_SIZE,
                                )
                            }
                            #[cfg(feature = "svg")]
                            ImageData::Svg(svg) => {
                                // pyro: an SVG's natural width and height are its root's
                                // absolute `width` and `height`, and its ratio theirs or else its
                                // `viewBox`'s; what it does not declare it does not have, and the
                                // default object size (300x150) stands in, as in browsers (usvg's
                                // resolved size of an SVG without them stood in before). Not for
                                // the root `<svg>` of an SVG document, which fills the viewport in
                                // browsers: it keeps usvg's size until that is done.
                                let mut width = svg.intrinsic_width();
                                let mut height = svg.intrinsic_height();
                                let ratio = if is_root_element {
                                    if width.is_none() && height.is_none() && svg.viewbox_aspect_ratio().is_none() {
                                        let size = svg.tree.size();
                                        width = Some(size.width());
                                        height = Some(size.height());
                                    }
                                    Some(svg.aspect_ratio())
                                } else {
                                    match (width, height) {
                                        (Some(width), Some(height)) if width > 0.0 && height > 0.0 => {
                                            Some(width / height)
                                        }
                                        _ => svg.viewbox_aspect_ratio(),
                                    }
                                };
                                (IntrinsicSizes { width, height, ratio }, DEFAULT_OBJECT_SIZE)
                            }
                            // pyro: a `<video>` whose poster did not load
                            // keeps the default object size.
                            ImageData::None
                                if element_data.name.local == local_name!("video") =>
                            {
                                tag_intrinsic_sizes(element_data, attr_size)
                            }
                            ImageData::None => (IntrinsicSizes::default(), taffy::Size::ZERO),
                        },
                        SpecialElementData::Canvas(_)
                        | SpecialElementData::SubDocument(_)
                        | SpecialElementData::None => {
                            tag_intrinsic_sizes(element_data, attr_size)
                        }
                        #[cfg(feature = "custom-widget")]
                        SpecialElementData::CustomWidget(widget_data) => {
                            let (fallback, default_object_size) =
                                tag_intrinsic_sizes(element_data, attr_size);
                            // A canvas's content attributes determine its intrinsic size,
                            // overriding the widget-reported one; the widget-reported size
                            // in turn overrides the tag's fallback.
                            let attr_intrinsic =
                                if *element_data.name.local == local_name!("canvas") {
                                    attr_size
                                } else {
                                    taffy::Size::NONE
                                };
                            let attr_ratio = match (attr_intrinsic.width, attr_intrinsic.height) {
                                (Some(w), Some(h)) => Some(w / h),
                                _ => None,
                            };
                            let widget_sizes = widget_data.widget.intrinsic_sizes();
                            (
                                IntrinsicSizes {
                                    width: attr_intrinsic
                                        .width
                                        .or(widget_sizes.width)
                                        .or(fallback.width),
                                    height: attr_intrinsic
                                        .height
                                        .or(widget_sizes.height)
                                        .or(fallback.height),
                                    ratio: attr_ratio.or(widget_sizes.ratio).or(fallback.ratio),
                                },
                                default_object_size,
                            )
                        }
                        _ => unreachable!(),
                    };

                    // pyro: an image (`<img>` or `<input type=image>`)
                    // the embedder has loaded but Blitz has not (yet) reports
                    // its natural size through the render tree's state
                    // attribute (`natural=W,H`).
                    let loaded = matches!(&element_data.special_data, SpecialElementData::Image(d) if !matches!(&**d, ImageData::None));
                    let is_image = *element_data.name.local == *"img" || is_image_input(element_data);
                    // A `<video>` with known metadata is sized by its video
                    // track, whatever its poster's size.
                    let is_video = element_data.name.local == local_name!("video");
                    // The token is the density-corrected natural size (a `srcset` candidate's `2x`),
                    // which a loaded raster image's pixel size is not.
                    let raster = matches!(&element_data.special_data, SpecialElementData::Image(d) if matches!(&**d, ImageData::Raster(_)));
                    let (intrinsic_sizes, default_object_size) = match pyro_natural_size(element_data) {
                        Some((width, height)) if is_video || (is_image && (!loaded || raster)) => (
                            IntrinsicSizes {
                                width: Some(width),
                                height: Some(height),
                                ratio: (height > 0.0).then_some(width / height),
                            },
                            DEFAULT_OBJECT_SIZE,
                        ),
                        _ => (intrinsic_sizes, default_object_size),
                    };

                    // pyro: an image of generated content or of the element's `content` at the
                    // resolution of its `image-set()` option.
                    let intrinsic_sizes = match &element_data.content_image {
                        Some((_, density)) if raster && *density > 0.0 && *density != 1.0 => IntrinsicSizes {
                            width: intrinsic_sizes.width.map(|w| w / density),
                            height: intrinsic_sizes.height.map(|h| h / density),
                            ratio: intrinsic_sizes.ratio,
                        },
                        _ => intrinsic_sizes,
                    };

                    // pyro: `image-orientation: none`: a raster image whose EXIF orientation turned
                    // it a quarter has its sides as stored.
                    let quarter_turned = matches!(&element_data.special_data,
                        SpecialElementData::Image(image) if matches!(&**image,
                            ImageData::Raster(raster) if (5..=8).contains(&raster.orientation)));
                    let intrinsic_sizes = if quarter_turned
                        && node.primary_styles().is_some_and(|style| crate::util::image_unoriented(&style))
                    {
                        IntrinsicSizes {
                            width: intrinsic_sizes.height,
                            height: intrinsic_sizes.width,
                            ratio: intrinsic_sizes.ratio.map(|ratio| 1.0 / ratio),
                        }
                    } else {
                        intrinsic_sizes
                    };

                    // pyro: `object-view-box`: the view box is the content's natural size.
                    let intrinsic_sizes = match (intrinsic_sizes.width, intrinsic_sizes.height, node.primary_styles()) {
                        (Some(width), Some(height), Some(style)) => {
                            match crate::util::object_view_box(&style, width, height) {
                                Some((_, _, view_width, view_height)) => IntrinsicSizes {
                                    width: Some(view_width),
                                    height: Some(view_height),
                                    ratio: Some(view_width / view_height),
                                },
                                None => intrinsic_sizes,
                            }
                        }
                        _ => intrinsic_sizes,
                    };

                    #[cfg(feature = "writing-mode")]
                    let (intrinsic_sizes, default_object_size) =
                        if node.writing_mode().is_vertical() {
                            // Intrinsic sizes are physical; the element's algorithm runs in its own writing mode
                            (
                                crate::layout::replaced::IntrinsicSizes {
                                    width: intrinsic_sizes.height,
                                    height: intrinsic_sizes.width,
                                    ratio: intrinsic_sizes.ratio.map(|ratio| 1.0 / ratio),
                                },
                                default_object_size.transpose(),
                            )
                        } else {
                            (intrinsic_sizes, default_object_size)
                        };

                    let ratio_in_box_sizing_box = node
                        .primary_styles()
                        .is_some_and(|style| !style.get_position().aspect_ratio.auto);
                    let replaced_context = ReplacedContext {
                        intrinsic_sizes,
                        default_object_size,
                        ratio_in_box_sizing_box,
                    };

                    let mut style = node.layout_style();
                    style.set_percent_basis(orthogonal_percent_basis);
                    return ChildLayout::Done(compute_replaced_layout(
                        inputs,
                        &style,
                        resolve_calc_value,
                        &replaced_context,
                    ));
                }

                // pyro: a fixed-layout table with a percentage width asks to be as wide as possible:
                // its max-content width is Blink's `kTableMaxWidth` (flex items holding such tables
                // share the line evenly; WPT css-flexbox
                // `fixed-table-layout-with-percentage-width-in-flex-item`). Empty tables too, which
                // are laid out as blocks.
                if inputs.run_mode == RunMode::ComputeSize
                    && inputs.axis == taffy::RequestedAxis::Horizontal
                    && inputs.known_dimensions.width.is_none()
                    && inputs.available_space.width == AvailableSpace::MaxContent
                    && node.primary_styles().is_some_and(|style| {
                        style.clone_display().inside() == style::values::specified::box_::DisplayInside::Table
                            && style.get_table().table_layout
                                == style::computed_values::table_layout::T::Fixed
                            && matches!(
                                &style.get_position().width,
                                style::values::computed::Size::LengthPercentage(lp) if lp.0.has_percentage()
                            )
                    })
                {
                    return ChildLayout::Done(taffy::LayoutOutput::from_outer_size(taffy::Size {
                        width: 1_000_000.0,
                        height: 0.0,
                    }));
                }

                if node.flags.is_table_root() {
                    return ChildLayout::Table;
                }

                if node.flags.is_inline_root() {
                    return match multicol::is_multicol_node(node) {
                        true => ChildLayout::Multicol { inline_root: true },
                        false => ChildLayout::Inline,
                    };
                }

                // The default CSS file will set
                match node.taffy_display() {
                    // pyro: a multi-column container (`layout::multicol`).
                    Display::Block | Display::FlowRoot if multicol::is_multicol_node(node) => {
                        ChildLayout::Multicol { inline_root: false }
                    }
                    Display::Block => ChildLayout::Block,
                    Display::FlowRoot => ChildLayout::FlowRoot,
                    Display::Flex => {
                        #[cfg(feature = "writing-mode")]
                        {
                            self.current_align_axis_is_inline = node.is_column_flex_container();
                        }
                        ChildLayout::Flex
                    }
                    Display::Grid => ChildLayout::Grid,
                    Display::None => ChildLayout::Done(taffy::LayoutOutput::HIDDEN),
                }
            }
            NodeData::Document(_) => ChildLayout::FlowRoot,

            _ => ChildLayout::Done(taffy::LayoutOutput::HIDDEN),
        }
    }

    /// A table root's layout (see [`Self::dispatch_child_layout`]).
    #[inline(never)]
    fn compute_table_root_layout(&mut self, node_id: NodeId, inputs: taffy::tree::LayoutInput) -> taffy::LayoutOutput {
        // The space available to an absolutely positioned table never exceeds
        // the size of its containing block (less the table's margins), even
        // when negative insets would otherwise add to it
        // (https://drafts.csswg.org/css-tables-3/#abspos).
        let mut inputs = inputs;
        if self.nodes[dom_node_id(node_id)].is_out_of_flow() {
            let margin = self.nodes[dom_node_id(node_id)]
                .layout_style()
                .margin()
                .resolve_or_zero(inputs.parent_size.width, resolve_calc_value);
            let max_available_space = inputs
                .parent_size
                .maybe_sub(margin.sum_axes())
                .maybe_max(taffy::Size::ZERO);
            inputs.available_space =
                inputs.available_space.maybe_min(max_available_space);
        }

        let SpecialElementData::TableRoot(context) = &self.nodes[dom_node_id(node_id)]
            .data
            .downcast_element()
            .unwrap()
            .special_data
        else {
            panic!("Node marked as table root but doesn't have TableContext");
        };
        let context = Arc::clone(context);
        // pyro: a table is at least as wide as its captions' min-content widths.
        let caption_min = if context.captions.is_empty() {
            None
        } else {
            Some(self.table_captions_min_width(&context, inputs))
        };

        let mut table_wrapper = TableTreeWrapper {
            doc: self,
            ctx: context,
            style: None,
        };
        table_wrapper.style = table_wrapper.auto_column_widths(&inputs);
        // pyro: a table's content size (a flex item's flex base size) is the width its columns give
        // it (the grid's preferred width, which a content-size grid would drop: WPT css-flexbox
        // `table-as-item-percent-width-cell-001`) and the height of its rows, its own `height` and
        // `min-height` aside (Blink; WPT css-flexbox `table-as-item-min-height-1`)
        let mut inputs = inputs;
        if inputs.sizing_mode == taffy::SizingMode::ContentSize {
            let mut style = table_wrapper.style.take().unwrap_or_else(|| table_wrapper.ctx.style.clone());
            style.size.height = taffy::Dimension::auto();
            style.min_size.height = taffy::LengthPercentageAuto::auto();
            table_wrapper.style = Some(style);
            inputs.sizing_mode = taffy::SizingMode::InherentSize;
        }
        let mut output = compute_grid_layout(&mut table_wrapper, node_id, inputs);
        if let Some(min) = caption_min.filter(|min| *min > output.size.width) {
            if inputs.known_dimensions.width.is_none() {
                let wider = taffy::tree::LayoutInput {
                    known_dimensions: taffy::Size { width: Some(min), height: inputs.known_dimensions.height },
                    ..inputs
                };
                table_wrapper.style = table_wrapper.auto_column_widths(&wider);
                output = compute_grid_layout(&mut table_wrapper, node_id, wider);
            }
        }
        // pyro: nor does its `max-width` make it narrower than its min-content width (CSS 2.1
        // § 17.5.2: the used width is at least the minimum width; WPT css-tables
        // `table-intrinsic-size-002`): measured without the maximum and its `width`, the table
        // takes that width.
        if inputs.known_dimensions.width.is_none() {
            let base = table_wrapper.style.clone().unwrap_or_else(|| table_wrapper.ctx.style.clone());
            if !base.max_size.width.is_auto() {
                let mut unbounded = base.clone();
                unbounded.max_size.width = taffy::style_helpers::auto();
                unbounded.size.width = taffy::style_helpers::auto();
                let saved = table_wrapper.style.replace(unbounded);
                let measuring = taffy::tree::LayoutInput {
                    run_mode: RunMode::ComputeSize,
                    available_space: taffy::Size {
                        width: taffy::AvailableSpace::MinContent,
                        height: inputs.available_space.height,
                    },
                    ..inputs
                };
                let min = compute_grid_layout(&mut table_wrapper, node_id, measuring).size.width;
                table_wrapper.style = saved;
                if min > output.size.width + 0.01 {
                    let wider = taffy::tree::LayoutInput {
                        known_dimensions: taffy::Size { width: Some(min), height: inputs.known_dimensions.height },
                        ..inputs
                    };
                    table_wrapper.style = table_wrapper.auto_column_widths(&wider);
                    let mut style = table_wrapper.style.clone().unwrap_or_else(|| table_wrapper.ctx.style.clone());
                    style.max_size.width = taffy::style_helpers::auto();
                    table_wrapper.style = Some(style);
                    output = compute_grid_layout(&mut table_wrapper, node_id, wider);
                }
            }
        }

        // pyro: a height a flex or grid parent gives a table is its wrapper box's, captions included
        // (css-flexbox-1 §4.1): the grid gets what the captions leave. The table's own `height` is its
        // grid's, as in block flow (WPT `css/css-flexbox/table-as-item-stretch-cross-size*`).
        if let (Some(height), false) = (inputs.known_dimensions.height, table_wrapper.ctx.captions.is_empty()) {
            let own = {
                use taffy::MaybeResolve;
                let style = &table_wrapper.ctx.style;
                let parent = inputs.parent_size;
                style.size.height.maybe_resolve(parent.height, resolve_calc_value).map(|own| match style.box_sizing {
                    taffy::BoxSizing::BorderBox => own,
                    taffy::BoxSizing::ContentBox => {
                        own + style.padding.resolve_or_zero(parent.width, resolve_calc_value).vertical_axis_sum()
                            + style.border.resolve_or_zero(parent.width, resolve_calc_value).vertical_axis_sum()
                    }
                })
            };
            // An inflexible item of a column flex container has its definite flex basis as its height: the
            // table box's, as `height` is (WPT `table-as-item-inflexible-in-column-2`).
            let inflexible_in_column = {
                let node = &table_wrapper.doc.nodes[dom_node_id(node_id)];
                let column_parent = node.layout_parent.get().is_some_and(|parent| {
                    table_wrapper.doc.nodes[parent].primary_styles().is_some_and(|style| {
                        style.clone_display().inside() == style::values::specified::box_::DisplayInside::Flex
                            && matches!(
                                style.get_position().flex_direction,
                                style::computed_values::flex_direction::T::Column
                                    | style::computed_values::flex_direction::T::ColumnReverse
                            )
                    })
                });
                column_parent
                    && node.primary_styles().is_some_and(|style| {
                        style.get_position().flex_grow.0 == 0.0 && style.get_position().flex_shrink.0 == 0.0
                    })
            };
            if !inflexible_in_column && own.is_none_or(|own| (own - height).abs() > 0.01) {
                let context = Arc::clone(&table_wrapper.ctx);
                let mut scratch = output.clone();
                let grid = scratch.size.height;
                let measuring = taffy::tree::LayoutInput { run_mode: RunMode::ComputeSize, ..inputs };
                table_wrapper.doc.lay_out_table_captions(&context, measuring, &mut scratch);
                let captions = scratch.size.height - grid;
                if captions > 0.0 {
                    let inner = taffy::tree::LayoutInput {
                        known_dimensions: taffy::Size {
                            width: inputs.known_dimensions.width,
                            height: Some((height - captions).max(0.0)),
                        },
                        ..inputs
                    };
                    table_wrapper.style = table_wrapper.auto_column_widths(&inner);
                    output = compute_grid_layout(&mut table_wrapper, node_id, inner);
                }
            }
        }

        // Absolutely positioned children take no part in the table grid. Their
        // static position is the top-left of the table's content box.
        let context = table_wrapper.ctx;
        // pyro: the captions above and below the grid.
        let caption_top = if context.captions.is_empty() {
            0.0
        } else {
            self.lay_out_table_captions(&context, inputs, &mut output)
        };
        if inputs.run_mode == RunMode::PerformLayout && !context.oof_children.is_empty()
        {
            let style = &context.style;
            let parent_width = inputs.parent_size.width;
            let padding = style
                .padding
                .resolve_or_zero(parent_width, resolve_calc_value);
            let border = style
                .border
                .resolve_or_zero(parent_width, resolve_calc_value);
            let static_position = taffy::Point {
                x: padding.left + border.left,
                y: caption_top + padding.top + border.top,
            }
            .map(|pos| AxisStaticPosition::from_edge(pos, AxisStaticEdge::Start));
            let in_flow_count = context.cells.len();
            for (idx, (child_id, position)) in context.oof_children.iter().enumerate() {
                output.oof_candidates.push(OofCandidate { containing_block: None,
                    node: taffy_node_id(*child_id),
                    order: (in_flow_count + idx) as u32,
                    position: *position,
                    static_position,
                });
            }
        }

        // pyro: an absolutely positioned box whose containing block is a positioned row or row
        // group of this table (they have no boxes of their own) is laid out here, against the
        // part's box moved by its relative offset (`table_part_containing_block`).
        if inputs.run_mode == RunMode::PerformLayout && !output.oof_candidates.is_empty() {
            let table_id = dom_node_id(node_id);
            for candidate in output.oof_candidates.as_mut_slice() {
                if candidate.containing_block.is_some() {
                    continue;
                }
                candidate.containing_block = self.table_part_containing_block(
                    table_id,
                    &context,
                    dom_node_id(candidate.node),
                    candidate.position,
                );
            }
        }

        // HACK: Cap scrollable overflow at node size to prevent scrolling
        let overflow = output.scrollable_overflow_rect;
        output.scrollable_overflow_rect = taffy::ScrollableOverflow::new(
            taffy::Rect {
                left: 0.0,
                top: 0.0,
                right: overflow.right().min(output.size.width),
                bottom: overflow.bottom().min(output.size.height),
            },
            output.scrollable_overflow_rect.is_reversed(),
        );

        output
    }
}

impl LayoutPassState<'_> {
    /// pyro: the containing block of the absolutely positioned box `box_id` when it is a
    /// positioned row or row group of the table `table_id` (the nearest positioned ancestor of the
    /// box below the table): the part's box (rows span every column; a row group spans its rows),
    /// moved by the relative offsets of it and of its row group, as `(size, offset)` from the
    /// table's border box.
    #[inline(never)]
    fn table_part_containing_block(
        &self,
        table_id: crate::NodeId,
        context: &crate::layout::table::TableContext,
        box_id: crate::NodeId,
        position: taffy::Position,
    ) -> Option<(taffy::Size<f32>, taffy::Point<f32>)> {
        let doc: &BaseDocument = self;
        let part = doc.containing_block_below_table(table_id, box_id, position)?;
        let rows: Vec<usize> = context
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| row.node_id == part || row.group == Some(part))
            .map(|(index, _)| index)
            .collect();
        let (&first, &last) = (rows.first()?, rows.last()?);
        let grid = context.computed_grid_info.borrow();
        let grid = grid.as_ref()?;
        let top = context.grid_box.borrow().map_or(0.0, |(top, _)| top);
        let row_lines = &grid.rows.positions;
        let column_lines = &grid.columns.positions;
        let (Some(first_row), Some(last_row)) = (row_lines.get(first), row_lines.get(last)) else { return None };
        let x0 = column_lines.iter().map(|line| line.start.min(line.end)).fold(f32::INFINITY, f32::min);
        let x1 = column_lines.iter().map(|line| line.start.max(line.end)).fold(f32::NEG_INFINITY, f32::max);
        if !x0.is_finite() || !x1.is_finite() {
            return None;
        }
        let (y0, y1) = (first_row.start.min(last_row.start), first_row.end.max(last_row.end));
        // Its relative offset, and its row group's for a row.
        let table_parent = Some(table_id);
        let group = context.rows[first].group.filter(|&group| group != part);
        let mut offset = taffy::Point::ZERO;
        let own_parent = if group.is_some() { group } else { table_parent };
        for (id, parent) in [(Some(part), own_parent), (group, table_parent)] {
            if let Some(own) = id.and_then(|id| crate::layout::table::table_part_relative_offset(doc, id, parent)) {
                offset.x += own.x;
                offset.y += own.y;
            }
        }
        Some((
            taffy::Size { width: (x1 - x0).max(0.0), height: (y1 - y0).max(0.0) },
            taffy::Point { x: x0 + offset.x, y: top + y0 + offset.y },
        ))
    }

    /// pyro: the widest min-content width of a table's captions, with their margins.
    #[inline(never)]
    fn table_captions_min_width(
        &mut self,
        context: &crate::layout::table::TableContext,
        inputs: taffy::tree::LayoutInput,
    ) -> f32 {
        let mut widest = 0.0f32;
        for &caption in &context.captions {
            if self.nodes[caption].primary_styles().is_none() {
                continue;
            }
            let margin = self.nodes[caption].layout_style().margin().resolve_or_zero(None, resolve_calc_value);
            let measure = taffy::tree::LayoutInput {
                run_mode: RunMode::ComputeSize,
                sizing_mode: taffy::SizingMode::InherentSize,
                axis: taffy::RequestedAxis::Horizontal,
                known_dimensions: taffy::Size::NONE,
                known_dimensions_are_definite: taffy::Size { width: true, height: true },
                parent_size: taffy::Size { width: None, height: inputs.parent_size.height },
                available_space: taffy::Size {
                    width: taffy::AvailableSpace::MinContent,
                    height: taffy::AvailableSpace::MaxContent,
                },
                vertical_margins_are_collapsible: taffy::Line::FALSE,
            };
            let width = self.compute_child_layout(taffy_node_id(caption), measure).size.width;
            widest = widest.max(width + margin.left + margin.right);
        }
        widest
    }

    /// pyro: lays out a table's captions (`TableContext::captions`) above (`caption-side: top`)
    /// and below its grid, each a block as wide as the grid less its margins: the table's box is
    /// then its wrapper box, the grid (`output`, laid out) moved down by the captions above it, as
    /// its cells (whose positions the grid set). Returns the height of the captions above.
    #[inline(never)]
    fn lay_out_table_captions(
        &mut self,
        context: &crate::layout::table::TableContext,
        inputs: taffy::tree::LayoutInput,
        output: &mut taffy::LayoutOutput,
    ) -> f32 {
        use style::values::specified::table::CaptionSide;
        let width = output.size.width;
        let grid_height = output.size.height;
        // (caption, above the grid, its margins, its layout)
        let mut laid_out = Vec::with_capacity(context.captions.len());
        for &caption in &context.captions {
            let Some(style) = self.nodes[caption].primary_styles() else { continue };
            let above = !matches!(style.slow_clone_caption_side(), CaptionSide::Bottom);
            drop(style);
            let margin = self.nodes[caption]
                .layout_style()
                .margin()
                .resolve_or_zero(Some(width), resolve_calc_value);
            let inner = (width - margin.left - margin.right).max(0.0);
            let caption_inputs = taffy::tree::LayoutInput {
                run_mode: inputs.run_mode,
                sizing_mode: taffy::SizingMode::InherentSize,
                axis: taffy::RequestedAxis::Both,
                known_dimensions: taffy::Size { width: Some(inner), height: None },
                known_dimensions_are_definite: taffy::Size { width: true, height: true },
                parent_size: taffy::Size { width: Some(width), height: None },
                available_space: taffy::Size {
                    width: taffy::AvailableSpace::Definite(inner),
                    height: taffy::AvailableSpace::MaxContent,
                },
                vertical_margins_are_collapsible: taffy::Line::FALSE,
            };
            let caption_output = self.compute_child_layout(taffy_node_id(caption), caption_inputs);
            laid_out.push((caption, above, margin, caption_output));
        }
        let extent = |above: bool| -> f32 {
            laid_out
                .iter()
                .filter(|(_, a, _, _)| *a == above)
                .map(|(_, _, margin, out)| margin.top + out.size.height + margin.bottom)
                .sum()
        };
        let (top, bottom) = (extent(true), extent(false));
        if inputs.run_mode == RunMode::PerformLayout {
            // The grid moves down by the captions above it: so do its cells.
            if top != 0.0 {
                for cell in &context.cells {
                    self.nodes[cell.node_id].update_unrounded_layout(|layout| layout.location.y += top);
                }
            }
            let (mut y_above, mut y_below) = (0.0, top + grid_height);
            for (order, (caption, above, margin, out)) in laid_out.iter_mut().enumerate() {
                let y = if *above { &mut y_above } else { &mut y_below };
                // pyro: the out-of-flow boxes in a caption whose containing block is further up
                // (the table, or past it) are the table's to pass on, from where the caption is.
                if !out.oof_candidates.is_empty() {
                    let mut candidates = out.oof_candidates.take();
                    candidates.translate(taffy::Point { x: margin.left, y: *y + margin.top });
                    output.oof_candidates.append(&mut candidates);
                }
                // pyro: a relatively positioned caption is moved by its insets (CSS 2.1 § 9.4.3;
                // WPT `position-relative-table-caption`), percentages against the table's width
                // and given height.
                let relative = {
                    let style = self.nodes[*caption].layout_style();
                    if style.position() == taffy::Position::Relative {
                        let inset = style.inset();
                        let left = taffy::MaybeResolve::maybe_resolve(inset.left, Some(width), resolve_calc_value);
                        let right = taffy::MaybeResolve::maybe_resolve(inset.right, Some(width), resolve_calc_value);
                        let top = taffy::MaybeResolve::maybe_resolve(inset.top, inputs.known_dimensions.height, resolve_calc_value);
                        let bottom = taffy::MaybeResolve::maybe_resolve(inset.bottom, inputs.known_dimensions.height, resolve_calc_value);
                        let x = match context.style.direction {
                            taffy::Direction::Rtl => right.map(|right| -right).or(left),
                            taffy::Direction::Ltr => left.or(right.map(|right| -right)),
                        };
                        taffy::Point {
                            x: x.unwrap_or(0.0),
                            y: top.or(bottom.map(|bottom| -bottom)).unwrap_or(0.0),
                        }
                    } else {
                        taffy::Point::ZERO
                    }
                };
                let layout = taffy::Layout {
                    order: (context.cells.len() + context.oof_children.len() + order) as u32,
                    location: taffy::Point { x: margin.left + relative.x, y: *y + margin.top + relative.y },
                    size: out.size,
                    scrollable_overflow_rect: out.scrollable_overflow_rect,
                    scrollbar_size: taffy::Size::ZERO,
                    border: self.nodes[*caption]
                        .layout_style()
                        .border()
                        .resolve_or_zero(Some(width), resolve_calc_value),
                    padding: self.nodes[*caption]
                        .layout_style()
                        .padding()
                        .resolve_or_zero(Some(width), resolve_calc_value),
                    margin: *margin,
                };
                *y += margin.top + out.size.height + margin.bottom;
                self.set_unrounded_layout(taffy_node_id(*caption), &layout);
            }
            *context.grid_box.borrow_mut() = Some((top, grid_height));
        }
        output.size.height = top + grid_height + bottom;
        output.baselines.first = output.baselines.first.map(|baseline| baseline + top);
        output.baselines.last = output.baselines.last.map(|baseline| baseline + top);
        top
    }
}

/// pyro: what [`LayoutPassState::dispatch_child_layout`] runs for a box.
enum ChildLayout {
    /// A leaf box, laid out.
    Done(taffy::LayoutOutput),
    Block,
    /// A block container that establishes its own formatting context (or the document).
    FlowRoot,
    Inline,
    Flex,
    Grid,
    Table,
    /// pyro: a multi-column container, whose content is a block or (`inline_root`) an inline
    /// formatting context.
    Multicol { inline_root: bool },
}

impl TraversePartialTree for LayoutPassState<'_> {
    type ChildIter<'a>
        = RefCellChildIter<'a>
    where
        Self: 'a;

    fn child_ids(&self, node_id: NodeId) -> Self::ChildIter<'_> {
        let layout_children = self.node_from_id(node_id).layout_children.borrow(); //.unwrap().as_ref();
        RefCellChildIter::new(Ref::map(layout_children, |children| {
            children.as_ref().map(|c| c.as_slice()).unwrap_or(&[])
        }))
    }

    fn child_count(&self, node_id: NodeId) -> usize {
        self.node_from_id(node_id)
            .layout_children
            .borrow()
            .as_ref()
            .map(|c| c.len())
            .unwrap_or(0)
    }

    fn get_child_id(&self, node_id: NodeId, index: usize) -> NodeId {
        taffy_node_id(
            self.node_from_id(node_id)
                .layout_children
                .borrow()
                .as_ref()
                .unwrap()[index],
        )
    }
}
impl TraverseTree for LayoutPassState<'_> {}

impl LayoutPartialTree for LayoutPassState<'_> {
    type CoreContainerStyle<'a>
        = TaffyStyloStyle<ComputedStyleRef<'a>>
    where
        Self: 'a;

    type CustomIdent = Atom;

    fn get_core_container_style(&self, node_id: NodeId) -> Self::CoreContainerStyle<'_> {
        #[cfg(feature = "writing-mode")]
        {
            // Container styles are read by the node's own algorithm (or, for the containing block
            // of an out-of-flow box, by that box's algorithm) and are expressed in the node's writing mode.
            let dom_id = dom_node_id(node_id);
            let mut style = self.nodes[dom_id].layout_style_in(self.layout_wm_of(dom_id));
            style.set_percent_basis(self.orthogonal_percent_basis_of(dom_id));
            style
        }
        #[cfg(not(feature = "writing-mode"))]
        {
            self.node_from_id(node_id).layout_style()
        }
    }

    fn set_unrounded_layout(&mut self, node_id: NodeId, layout: &Layout) {
        // A parent's resize can relayout an otherwise style-clean child: this marks the existing
        // damage traversal flags (no restyle/reconstruction damage), so unchanged layouts continue to
        // reuse their cached overflow and rounding.
        self.node_from_id_mut(node_id)
            .update_unrounded_layout(|unrounded| *unrounded = *layout);
    }

    fn resolve_calc_value(&self, calc_ptr: *const (), parent_size: f32) -> f32 {
        resolve_calc_value(calc_ptr, parent_size)
    }

    #[inline(always)]
    fn compute_child_layout(
        &mut self,
        node_id: NodeId,
        inputs: taffy::LayoutInput,
    ) -> taffy::LayoutOutput {
        compute_cached_layout(self, node_id, inputs, |tree, node_id, inputs| {
            tree.compute_child_layout_internal(node_id, inputs, None)
        })
    }
}

impl LayoutContainingBlock for LayoutPassState<'_> {
    type OofItemStyle<'a>
        = TaffyStyloStyle<ComputedStyleRef<'a>>
    where
        Self: 'a;

    fn get_oof_item_style(&self, node_id: NodeId) -> Self::OofItemStyle<'_> {
        self.child_layout_style(self.node_from_id(node_id))
    }

    fn clear_hoisted_children(&mut self, node_id: NodeId) {
        let containing_block = dom_node_id(node_id);
        let hoisted = std::mem::take(&mut *self.node_from_id(node_id).hoisted_children.borrow_mut());
        for &hoisted_id in hoisted.iter() {
            // A box which has since been claimed by another containing block
            // must keep pointing at that one.
            if let Some(node) = self.nodes.get(hoisted_id) {
                if node.oof_containing_block.get() == Some(containing_block) {
                    node.oof_containing_block.set(None);
                    self.doc.oof_hoisted.remove(&hoisted_id);
                }
            }
        }
        let mut list = hoisted;
        list.clear();
        *self.node_from_id(node_id).hoisted_children.borrow_mut() = list;
    }

    /// pyro: an element in the top layer (a modal dialog, a showing popover) has the initial
    /// containing block as its static-position rectangle.
    fn static_position_is_containing_block(&self, node_id: NodeId) -> bool {
        self.doc.is_in_top_layer(dom_node_id(node_id))
    }

    fn add_hoisted_children(&mut self, node_id: NodeId, hoisted: &[NodeId]) {
        let containing_block = dom_node_id(node_id);
        let node = self.node_from_id(node_id);
        node.hoisted_children
            .borrow_mut()
            .extend(hoisted.iter().copied().map(dom_node_id));
        for &hoisted_id in hoisted {
            self.node_from_id(hoisted_id)
                .oof_containing_block
                .set(Some(containing_block));
            self.doc.oof_hoisted.insert(dom_node_id(hoisted_id));
        }
    }

    fn get_detailed_layout_info(&self, node_id: NodeId) -> &DetailedLayoutInfo<Atom> {
        self.node_from_id(node_id)
            .element_data()
            .map(|element| &element.detailed_layout_info)
            .unwrap_or(&DetailedLayoutInfo::None)
    }
}

impl LayoutPassState<'_> {
    #[cfg(feature = "floats")]
    fn clear_layout_cache_of(&mut self, node_id: NodeId) {
        use taffy::CacheTree as _;
        self.cache_clear(node_id);
    }

    /// pyro: a measuring run (`RunMode::ComputeSize`) left the lines of the inline root `node_id`
    /// elsewhere than its last final layout did (`TextLayout::final_lines`): its lines, their
    /// alignment or the places of its inline boxes are not those of that layout any more, and a
    /// cached final layout would leave them so. The root is marked `FINAL_LAYOUT_STALE`: its next
    /// final layout runs again (`cache_get`), and clears the mark (`cache_store`). So are the
    /// boxes around it that are being laid out (`IN_LAYOUT`, up to one that is not): this pass may
    /// ask one of them for a final layout it has cached, which would not lay the root out again (a
    /// table measures a cell for its columns after an earlier step laid the cell's `inline-flex`
    /// out). Boxes not being laid out are left alone: a box laid out alone (an absolutely
    /// positioned box of the root) would make them run again in the next pass. A column flex item
    /// measured at the available width and then stretched to its container's, in a table cell or a
    /// flex item that is measured before it is laid out, had its text aligned to the available
    /// width. A root still marked at the end of the pass is laid out again with the boxes it is in
    /// (`mark_stale_lines_up`, `BaseDocument::resolve_layout`).
    #[inline(never)]
    pub(crate) fn mark_final_layout_stale(&mut self, node_id: crate::NodeId) {
        let node = &mut self.nodes[node_id];
        if node.flags.contains(NodeFlags::FINAL_LAYOUT_STALE) {
            return;
        }
        node.flags.insert(NodeFlags::FINAL_LAYOUT_STALE);
        self.stale_lines.push(node_id);
        let mut id = self.nodes[node_id].layout_parent.get();
        while let Some(current) = id {
            let node = &mut self.nodes[current];
            if !node.flags.contains(NodeFlags::IN_LAYOUT) || node.flags.contains(NodeFlags::FINAL_LAYOUT_STALE) {
                break;
            }
            node.flags.insert(NodeFlags::FINAL_LAYOUT_STALE);
            id = node.layout_parent.get();
        }
    }

    /// pyro: marks the boxes that the inline roots this pass left `FINAL_LAYOUT_STALE` (see
    /// `mark_final_layout_stale`) are laid out in, up to one marked already, so that a layout
    /// from the root reaches those roots again; whether there were any.
    pub(crate) fn mark_stale_lines_up(&mut self) -> bool {
        let mut any = false;
        for root in std::mem::take(&mut self.stale_lines) {
            if !self.nodes[root].flags.contains(NodeFlags::FINAL_LAYOUT_STALE) {
                continue;
            }
            any = true;
            let mut id = self.nodes[root].layout_parent.get();
            while let Some(current) = id {
                let node = &mut self.nodes[current];
                if node.flags.contains(NodeFlags::FINAL_LAYOUT_STALE) {
                    break;
                }
                node.flags.insert(NodeFlags::FINAL_LAYOUT_STALE);
                id = node.layout_parent.get();
            }
        }
        any
    }
}

impl taffy::CacheTree for LayoutPassState<'_> {
    #[inline]
    fn cache_get(
        &mut self,
        node_id: NodeId,
        inputs: &taffy::LayoutInput,
    ) -> Option<taffy::LayoutOutput> {
        let node = self.node_from_id_mut(node_id);
        // pyro: see `mark_final_layout_stale`.
        if inputs.run_mode == RunMode::PerformLayout && node.flags.contains(NodeFlags::FINAL_LAYOUT_STALE) {
            return None;
        }
        node.try_layout_data_mut()?.cache.get(inputs)
    }

    #[inline]
    fn cache_store(
        &mut self,
        node_id: NodeId,
        inputs: &taffy::LayoutInput,
        layout_output: taffy::LayoutOutput,
    ) {
        let node = self.node_from_id_mut(node_id);
        if inputs.run_mode == RunMode::PerformLayout {
            node.flags.remove(NodeFlags::FINAL_LAYOUT_STALE);
        }
        node.cache_mut().store(inputs, layout_output);
    }

    #[inline]
    fn cache_clear(&mut self, node_id: NodeId) {
        self.node_from_id_mut(node_id).clear_layout_cache();
    }
}

impl taffy::LayoutBlockContainer for LayoutPassState<'_> {
    type BlockContainerStyle<'a>
        = TaffyStyloStyle<ComputedStyleRef<'a>>
    where
        Self: 'a;

    type BlockItemStyle<'a>
        = TaffyStyloStyle<ComputedStyleRef<'a>>
    where
        Self: 'a;

    fn get_block_container_style(&self, node_id: NodeId) -> Self::BlockContainerStyle<'_> {
        self.get_core_container_style(node_id)
    }

    fn get_block_child_style(&self, child_node_id: NodeId) -> Self::BlockItemStyle<'_> {
        self.child_layout_style(self.node_from_id(child_node_id))
    }

    fn block_child_width_fits_content(&self, child_node_id: NodeId) -> bool {
        // Form controls keep their intrinsic width as block-level boxes (Chromium).
        let Some(el) = self.node_from_id(child_node_id).data.downcast_element() else {
            return false;
        };
        match el.name.local {
            local_name!("select") | local_name!("textarea") | local_name!("button") => true,
            local_name!("input") => !matches!(el.attr(local_name!("type")), Some("hidden")),
            _ => false,
        }
    }

    fn block_child_establishes_formatting_context(&self, child_node_id: NodeId) -> bool {
        // pyro: a multi-column container is a block formatting context root (Multi-column
        // Layout 1 §2), which its parent keeps clear of floats.
        multicol::is_multicol_node(self.node_from_id(child_node_id))
    }

    fn block_container_passes_percentage_height(&self, node_id: NodeId) -> bool {
        let node = self.node_from_id(node_id);
        node.is_split_inline() || node.is_inline_with_only_block_children() || self.quirks_skips_for_percent_height(node)
    }

    fn block_container_percentage_height(&self, node_id: NodeId) -> Option<f32> {
        self.multicol_percent_height
            .filter(|(container, _)| *container == dom_node_id(node_id))
            .map(|(_, height)| height)
    }

    fn block_container_direction(&self, node_id: NodeId) -> Option<taffy::Direction> {
        use taffy::CoreStyle as _;
        let mut node = self.node_from_id(node_id);
        if !(node.is_split_inline() || node.is_inline_with_only_block_children()) {
            return None;
        }
        // The nearest box that is a block container.
        while node.is_split_inline() || node.is_inline_with_only_block_children() {
            node = &self.nodes[node.parent?];
        }
        node.primary_styles()?;
        Some(self.child_layout_style(node).direction())
    }

    fn block_child_flow_overlap(&self, child_node_id: NodeId) -> (bool, bool) {
        // pyro: a child that paints whole (a layer root, a float, a flex container) keeps its
        // content's order to itself.
        let child = self.node_from_id(child_node_id);
        let flags = child.flags;
        if !flags.intersects(NodeFlags::FLOW_BOXES_OVERLAP | NodeFlags::FLOW_CONTENT_OVERFLOWS)
            || !child.paints_in_ancestor_phases()
        {
            return (false, false);
        }
        (flags.contains(NodeFlags::FLOW_BOXES_OVERLAP), flags.contains(NodeFlags::FLOW_CONTENT_OVERFLOWS))
    }

    fn set_block_flow_overlap(&mut self, node_id: NodeId, overlap: bool, overflows: bool) {
        let flags = &mut self.node_from_id_mut(node_id).flags;
        flags.set(NodeFlags::FLOW_BOXES_OVERLAP, overlap);
        flags.set(NodeFlags::FLOW_CONTENT_OVERFLOWS, overflows);
    }

    #[inline(always)]
    fn compute_block_child_layout(
        &mut self,
        node_id: NodeId,
        inputs: taffy::LayoutInput,
        mut block_ctx: Option<&mut BlockContext<'_>>,
    ) -> taffy::LayoutOutput {
        // A block in a formatting context with floats depends on them, which
        // the cache doesn't key on: it is laid out afresh, and one that placed
        // floats is not replayed from the cache later (its floats would not
        // be placed again).
        #[cfg(feature = "floats")]
        if block_ctx.as_ref().is_some_and(|ctx| ctx.has_floats()) {
            self.clear_layout_cache_of(node_id);
            return self.compute_child_layout_internal(node_id, inputs, block_ctx);
        }
        let output = compute_cached_layout(self, node_id, inputs, |tree, node_id, inputs| {
            tree.compute_child_layout_internal(node_id, inputs, block_ctx.as_deref_mut())
        });
        #[cfg(feature = "floats")]
        if block_ctx.as_ref().is_some_and(|ctx| ctx.has_floats()) {
            self.clear_layout_cache_of(node_id);
        }
        output
    }
}

impl taffy::LayoutFlexboxContainer for LayoutPassState<'_> {
    type FlexboxContainerStyle<'a>
        = TaffyStyloStyle<ComputedStyleRef<'a>>
    where
        Self: 'a;

    type FlexboxItemStyle<'a>
        = TaffyStyloStyle<ComputedStyleRef<'a>>
    where
        Self: 'a;

    fn get_flexbox_container_style(&self, node_id: NodeId) -> Self::FlexboxContainerStyle<'_> {
        self.get_core_container_style(node_id)
    }

    fn get_flexbox_child_style(&self, child_node_id: NodeId) -> Self::FlexboxItemStyle<'_> {
        self.child_layout_style(self.node_from_id(child_node_id))
    }
}

impl taffy::LayoutGridContainer for LayoutPassState<'_> {
    type GridContainerStyle<'a>
        = TaffyStyloStyle<ComputedStyleRef<'a>>
    where
        Self: 'a;

    type GridItemStyle<'a>
        = TaffyStyloStyle<ComputedStyleRef<'a>>
    where
        Self: 'a;

    fn get_grid_container_style(&self, node_id: NodeId) -> Self::GridContainerStyle<'_> {
        self.get_core_container_style(node_id)
    }

    fn get_grid_child_style(&self, child_node_id: NodeId) -> Self::GridItemStyle<'_> {
        self.child_layout_style(self.node_from_id(child_node_id))
    }

    fn set_detailed_grid_info(
        &mut self,
        node_id: NodeId,
        detailed_grid_info: taffy::DetailedGridInfo<Atom>,
    ) {
        let node = self.node_from_id_mut(node_id);
        if let Some(element) = node.element_data_mut() {
            element.detailed_layout_info = DetailedLayoutInfo::Grid(Box::new(detailed_grid_info));
        }
    }

    /// pyro: the context of a subgrid (`LayoutData::subgrid`), while its layout parent is a grid (one
    /// left from when it was another grid's item is not read).
    fn subgrid_context(&self, node_id: NodeId) -> Option<taffy::SubgridContext<Atom>> {
        let node = self.node_from_id(node_id);
        let parent = node.layout_parent.get()?;
        let parent_is_grid = self.nodes[parent]
            .primary_styles()
            .is_some_and(|style| style.clone_display().inside() == style::values::specified::box_::DisplayInside::Grid);
        if !parent_is_grid {
            return None;
        }
        node.layout_data().subgrid.as_deref().cloned()
    }

    /// pyro: a subgrid's layout depends on its context as on its inputs: a new one drops its cached layouts.
    fn set_subgrid_context(&mut self, node_id: NodeId, context: Option<taffy::SubgridContext<Atom>>) {
        let node = self.node_from_id_mut(node_id);
        if node.layout_data().subgrid.as_deref() == context.as_ref() {
            return;
        }
        node.layout_data_mut().subgrid = context.map(Box::new);
        node.clear_layout_cache();
    }
}

impl RoundTree for LayoutPassState<'_> {
    fn get_unrounded_layout(&self, node_id: NodeId) -> Layout {
        *self.node_from_id(node_id).unrounded_layout()
    }

    fn set_final_layout(&mut self, node_id: NodeId, layout: &Layout) {
        let node = self.node_from_id_mut(node_id);
        let size_changed = node.final_layout().size != layout.size;
        *node.final_layout_mut() = *layout;
        // pyro: a transform resolves against the rounded size
        // (`Node::set_transform`), and rounding moves with the box's absolute
        // position: a box that only moved (or sits inside one) can change its
        // rounded size with no unrounded layout of its own changing. Mark it, so
        // `resolve_transforms` resolves its transform again instead of reusing
        // the one for the old size. The style decides, not `transform()`: one
        // that was the identity at the old size is stored as `None`.
        if size_changed && node.has_transform_style() {
            node.mark_layout_changed(Some(node.id));
        }
    }

    fn is_out_of_flow(&self, node_id: NodeId) -> bool {
        self.node_from_id(node_id)
            .out_of_flow
            .load(core::sync::atomic::Ordering::Relaxed)
    }

    fn hoisted_child_count(&self, node_id: NodeId) -> usize {
        self.node_from_id(node_id).hoisted_children.borrow().len()
    }

    fn get_hoisted_child_id(&self, node_id: NodeId, index: usize) -> NodeId {
        taffy_node_id(self.node_from_id(node_id).hoisted_children.borrow()[index])
    }

    // Incremental rounding: an undamaged subtree reached with the same
    // unrounded layout at the same offset rounds as it did last time. A subtree
    // is undamaged only if no unrounded layout in it changed
    // (`Node::update_unrounded_layout`). Anonymous boxes are always rounded
    // (damage marking walks the DOM parent chain, so their descendant flags are
    // not maintained), and so are nodes without style data, which have no damage.
    //
    // pyro: the offset is the fractional part of the parent's absolute
    // position (`writing_mode::rounded_layout` rounds relative to the pixel grid),
    // so a subtree moved by whole pixels, the rows below a table cell that grew,
    // is skipped as well.
    fn rounded_unchanged(&self, node_id: NodeId, unrounded: &Layout, parent_frac_x: f32, parent_frac_y: f32) -> bool {
        let node = self.node_from_id(node_id);
        !node.is_anonymous()
            && !node.has_damaged_descendants()
            && node.damage().is_some_and(|d| d.is_empty())
            && node.layout_data().rounded_from == Some((*unrounded, parent_frac_x, parent_frac_y))
    }

    fn record_rounded(&mut self, node_id: NodeId, unrounded: &Layout, parent_frac_x: f32, parent_frac_y: f32) {
        self.node_from_id_mut(node_id).layout_data_mut().rounded_from =
            Some((*unrounded, parent_frac_x, parent_frac_y));
    }
}

pub(crate) struct TaffyDebugTree<'doc>(pub(crate) &'doc BaseDocument);

impl TaffyDebugTree<'_> {
    fn node_from_id(&self, node_id: NodeId) -> &Node {
        &self.0.nodes[dom_node_id(node_id)]
    }
}

impl TraversePartialTree for TaffyDebugTree<'_> {
    type ChildIter<'a>
        = RefCellChildIter<'a>
    where
        Self: 'a;

    fn child_ids(&self, node_id: NodeId) -> Self::ChildIter<'_> {
        let layout_children = self.node_from_id(node_id).layout_children.borrow();
        RefCellChildIter::new(Ref::map(layout_children, |children| {
            children.as_ref().map(|c| c.as_slice()).unwrap_or(&[])
        }))
    }

    fn child_count(&self, node_id: NodeId) -> usize {
        self.node_from_id(node_id)
            .layout_children
            .borrow()
            .as_ref()
            .map(|c| c.len())
            .unwrap_or(0)
    }

    fn get_child_id(&self, node_id: NodeId, index: usize) -> NodeId {
        taffy_node_id(
            self.node_from_id(node_id)
                .layout_children
                .borrow()
                .as_ref()
                .unwrap()[index],
        )
    }
}

impl TraverseTree for TaffyDebugTree<'_> {}

impl PrintTree for TaffyDebugTree<'_> {
    fn get_debug_label(&self, node_id: NodeId) -> &'static str {
        let node = &self.node_from_id(node_id);

        match node.data {
            NodeData::Document(_) => "DOCUMENT",
            // NodeData::Doctype { .. } => return "DOCTYPE",
            NodeData::Text { .. } => node.node_debug_str().leak(),
            NodeData::Comment { .. } => "COMMENT",
            NodeData::AnonymousBlock(_) => "ANONYMOUS BLOCK",
            NodeData::Element(_) => {
                let style = node.layout_style();
                let display = match node.taffy_display() {
                    Display::Flex => match taffy::FlexboxContainerStyle::flex_direction(&style) {
                        FlexDirection::Row | FlexDirection::RowReverse => "FLEX ROW",
                        FlexDirection::Column | FlexDirection::ColumnReverse => "FLEX COL",
                    },
                    Display::Grid => "GRID",
                    Display::Block => "BLOCK",
                    Display::FlowRoot => "FLOW ROOT",
                    Display::None => "NONE",
                };
                format!("{} ({})", node.node_debug_str(), display).leak()
            } // NodeData::ProcessingInstruction { .. } => return "PROCESSING INSTRUCTION",
        }
    }

    fn get_final_layout(&self, node_id: NodeId) -> Layout {
        *self.node_from_id(node_id).final_layout()
    }
}

// pub struct ChildIter<'a>(std::slice::Iter<'a, usize>);
// impl<'a> Iterator for ChildIter<'a> {
//     type Item = NodeId;
//     fn next(&mut self) -> Option<Self::Item> {
//         self.0.next().copied().map(NodeId::from)
//     }
// }

pub struct RefCellChildIter<'a> {
    items: Ref<'a, [crate::NodeId]>,
    idx: usize,
}
impl<'a> RefCellChildIter<'a> {
    fn new(items: Ref<'a, [crate::NodeId]>) -> RefCellChildIter<'a> {
        RefCellChildIter { items, idx: 0 }
    }
}

impl Iterator for RefCellChildIter<'_> {
    type Item = NodeId;
    fn next(&mut self) -> Option<Self::Item> {
        self.items.get(self.idx).map(|id| {
            self.idx += 1;
            taffy_node_id(*id)
        })
    }
}

/// pyro: the `natural=W,H` token of the element's
/// `urn:x-pyro:state` attribute.
fn pyro_natural_size(element: &crate::node::ElementData) -> Option<(f32, f32)> {
    let attr =
        element.attrs.iter().find(|a| &*a.name.ns == "urn:x-pyro:state" && &*a.name.local == "state")?;
    let token = attr.value.split_ascii_whitespace().find_map(|t| t.strip_prefix("natural="))?;
    let (w, h) = token.split_once(',')?;
    Some((w.parse().ok()?, h.parse().ok()?))
}

impl BaseDocument {
    /// pyro: Quirks Mode §3.5, "The percentage height calculation quirk": in quirks mode a block
    /// container of `auto` height (the caller asks only then) that is no table cell and not out
    /// of flow hands its own percentage basis for heights on to its children, as Chromium's
    /// `UseParentPercentageResolutionBlockSizeForChildren` does (WPT css-sizing
    /// `intrinsic-percent-replaced-*` in quirks mode).
    pub(crate) fn quirks_skips_for_percent_height(&self, node: &Node) -> bool {
        use style::values::specified::box_::DisplayInside;
        if !self.tree().quirks_mode {
            return false;
        }
        let Some(style) = node.primary_styles() else { return false };
        let box_style = style.get_box();
        // A table cell's children keep their own basis (WPT css-tables
        // `percentages-grandchildren-quirks-mode-001`).
        let parent_is_cell = node
            .layout_parent
            .get()
            .and_then(|parent| self.nodes[parent].primary_styles())
            .is_some_and(|parent| parent.get_box().display.inside() == DisplayInside::TableCell);
        matches!(box_style.display.inside(), DisplayInside::Flow | DisplayInside::FlowRoot)
            && !box_style.position.is_absolutely_positioned()
            && !parent_is_cell
    }
}

/// pyro: `contain` with the size containment a size container has (`container-type: size` is
/// size containment, `inline-size` inline-size containment; css-contain-3 §2): its content does
/// not size it (WPT css-conditional `container-queries/flex-basis-with-container-type`).
pub(crate) fn effective_size_containment(styles: &style::properties::ComputedValues) -> style::values::computed::Contain {
    use style::values::computed::Contain;
    use style::values::specified::box_::ContainerType;
    let box_style = styles.get_box();
    let mut contain = box_style.contain;
    if box_style.container_type.intersects(ContainerType::SIZE) {
        contain |= Contain::INLINE_SIZE | Contain::BLOCK_SIZE;
    } else if box_style.container_type.intersects(ContainerType::INLINE_SIZE) {
        contain |= Contain::INLINE_SIZE;
    }
    contain
}
