//! Enable the dom to lay itself out using taffy
//!
//! In servo, style and layout happen together during traversal
//! However, in Blitz, we do a style pass then a layout pass.
//! This is slower, yes, but happens fast enough that it's not a huge issue.

use crate::node::{ComputedStyleRef, ImageData, NodeData, SpecialElementData};
use crate::{document::BaseDocument, dom_node_id, node::Node, taffy_node_id};
use markup5ever::local_name;
use std::cell::Ref;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use style::Atom;
use style::values::computed::CSSPixelLength;
use style::values::computed::length_percentage::CalcLengthPercentage;
use stylo_taffy::TaffyStyloStyle;
use taffy::{
    AxisStaticEdge, AxisStaticPosition, BlockContext, CoreStyle as _, DetailedLayoutInfo,
    FlexDirection, LayoutContainingBlock, LayoutPartialTree, MaybeMath as _, NodeId, OofCandidate,
    ResolveOrZero, RoundTree, RunMode, TraversePartialTree, TraverseTree, compute_block_layout,
    compute_cached_layout, compute_flexbox_layout, compute_grid_layout, compute_leaf_layout,
    compute_oof_layout, prelude::*,
};

pub(crate) mod construct;
pub(crate) mod damage;
pub(crate) mod inline;
pub(crate) mod list;
pub(crate) mod paint_tree;
pub(crate) mod replaced;
pub mod table;
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
        #[cfg(feature = "writing-mode")]
        {
            self.compute_child_layout_in_own_wm(node_id, inputs, block_ctx)
        }
        #[cfg(not(feature = "writing-mode"))]
        {
            self.compute_child_layout_in_current_wm(node_id, inputs, block_ctx)
        }
    }

    /// Lay out `node_id` in the writing mode of the currently running algorithm (its parent's)
    pub(crate) fn compute_child_layout_in_current_wm(
        &mut self,
        node_id: NodeId,
        inputs: taffy::tree::LayoutInput,
        block_ctx: Option<&mut BlockContext<'_>>,
    ) -> taffy::tree::LayoutOutput {
        let mut inputs = inputs;
        // stylo_taffy cannot represent intrinsic keywords in min_size(), so
        // min-width:max-content reaches Taffy as auto. Its flex minimum-size
        // query consequently requests min-content. Preserve the authored
        // keyword here before measuring the node's descendants.
        if inputs.sizing_mode == taffy::SizingMode::ContentSize
            && inputs.axis == taffy::RequestedAxis::Horizontal
            && inputs.available_space.width == AvailableSpace::MinContent
            && self.nodes[dom_node_id(node_id)].primary_styles().is_some_and(|style| {
                !style.writing_mode.is_vertical()
                    && matches!(style.clone_min_width(), style::values::computed::Size::MaxContent)
            })
        {
            inputs.available_space.width = AvailableSpace::MaxContent;
        }
        let mut output = self.dispatch_child_layout(node_id, inputs, block_ctx);
        if inputs.run_mode == RunMode::PerformLayout {
            compute_oof_layout(self, node_id, &mut output);
        }
        output
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

    fn dispatch_child_layout(
        &mut self,
        node_id: NodeId,
        inputs: taffy::tree::LayoutInput,
        block_ctx: Option<&mut BlockContext<'_>>,
    ) -> taffy::tree::LayoutOutput {
        let orthogonal_percent_basis = self.orthogonal_percent_basis_of(dom_node_id(node_id));
        let scale = self.doc.viewport.scale();
        // lightningpanda: the longest option label of a drop-down select (the
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

            let font_size = style.clone_font_size().used_size().px();
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

                taffy::LayoutOutput::HIDDEN
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

                    let output = compute_leaf_layout(
                        inputs,
                        &node.layout_style(),
                        resolve_calc_value,
                        |_known_size, _available_space| taffy::Size {
                            width: if compressible_min_content {
                                0.0
                            } else {
                                cols.map(|cols| cols * font_size.unwrap_or(16.0) * 0.6).unwrap_or(300.0)
                            },
                            height: resolved_line_height.unwrap_or(16.0) * rows,
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
                        }
                    }
                    return output;
                }

                // lightningpanda: a drop-down select is an atomic box: its
                // longest option's label (about half an em per character)
                // plus the arrow, and a line of the control's font tall
                // (Chromium's menu list: `line-height` is `normal`). Without
                // this it had no box at all (its options are hidden).
                if let Some((false, chars)) = select_label_chars {
                    let font_size = font_size.unwrap_or(13.333);
                    let style = node.layout_style();
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
                            height: (font_size * 1.25).ceil(),
                        },
                    );
                    if let Some(line) = select_line {
                        let parent = inputs.parent_size.width;
                        let padding = style.padding().resolve_or_zero(parent, resolve_calc_value);
                        let border = style.border().resolve_or_zero(parent, resolve_calc_value);
                        let content_top = padding.top + border.top;
                        let content_height = output.size.height - content_top - padding.bottom - border.bottom;
                        let (line_height, baseline) = (line.line_height / scale, line.baseline / scale);
                        let baseline = content_top + (content_height - line_height) / 2.0 + baseline;
                        output.baselines = taffy::Baselines { first: Some(baseline), last: Some(baseline) };
                    }
                    return output;
                }

                if *element_data.name.local == *"input" {
                    match element_data.attr(local_name!("type")) {
                        // if the input type is hidden, hide it
                        Some("hidden") => {
                            return taffy::LayoutOutput::HIDDEN;
                        }
                        Some("checkbox") => {
                            return compute_leaf_layout(
                                inputs,
                                &node.layout_style(),
                                resolve_calc_value,
                                |_known_size, _available_space| {
                                    let size = node.layout_style().size();
                                    let width = size.width.resolve_or_zero(
                                        inputs.parent_size.width,
                                        resolve_calc_value,
                                    );
                                    let height = size.height.resolve_or_zero(
                                        inputs.parent_size.height,
                                        resolve_calc_value,
                                    );
                                    let min_size = width.min(height);
                                    taffy::Size {
                                        width: min_size,
                                        height: min_size,
                                    }
                                },
                            );
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
                            let mut output = compute_leaf_layout(
                                inputs,
                                &style,
                                resolve_calc_value,
                                |_known_size, _available_space| taffy::Size {
                                    width: contribution_width,
                                    height: resolved_line_height.unwrap_or(16.0),
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
                                output.baselines = taffy::Baselines { first: Some(baseline), last: Some(baseline) };
                            }
                            return output;
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
                                let mut width = svg.intrinsic_width();
                                let mut height = svg.intrinsic_height();
                                // An SVG with no declared dimensions and no viewBox has no
                                // intrinsic dimensions per CSS, but usvg still resolves a
                                // concrete size; use it in place of the default object size.
                                if width.is_none()
                                    && height.is_none()
                                    && svg.viewbox_aspect_ratio().is_none()
                                {
                                    let size = svg.tree.size();
                                    width = Some(size.width());
                                    height = Some(size.height());
                                }
                                (
                                    IntrinsicSizes {
                                        width,
                                        height,
                                        ratio: Some(svg.aspect_ratio()),
                                    },
                                    DEFAULT_OBJECT_SIZE,
                                )
                            }
                            // lightningpanda: a `<video>` whose poster did not load
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

                    // lightningpanda: an image (`<img>` or `<input type=image>`)
                    // the embedder has loaded but Blitz has not (yet) reports
                    // its natural size through the render tree's state
                    // attribute (`natural=W,H`).
                    let loaded = matches!(&element_data.special_data, SpecialElementData::Image(d) if !matches!(&**d, ImageData::None));
                    let is_image = *element_data.name.local == *"img" || is_image_input(element_data);
                    // A `<video>` with known metadata is sized by its video
                    // track, whatever its poster's size.
                    let is_video = element_data.name.local == local_name!("video");
                    let (intrinsic_sizes, default_object_size) = match lp_natural_size(element_data) {
                        Some((width, height)) if is_video || (!loaded && is_image) => (
                            IntrinsicSizes {
                                width: Some(width),
                                height: Some(height),
                                ratio: (height > 0.0).then_some(width / height),
                            },
                            DEFAULT_OBJECT_SIZE,
                        ),
                        _ => (intrinsic_sizes, default_object_size),
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

                    let replaced_context = ReplacedContext {
                        intrinsic_sizes,
                        default_object_size,
                    };

                    let mut style = node.layout_style();
                    style.set_percent_basis(orthogonal_percent_basis);
                    return compute_replaced_layout(
                        inputs,
                        &style,
                        resolve_calc_value,
                        &replaced_context,
                    );
                }

                if node.flags.is_table_root() {
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

                    let mut table_wrapper = TableTreeWrapper {
                        doc: self,
                        ctx: context,
                        style: None,
                    };
                    table_wrapper.style = table_wrapper.auto_column_widths(&inputs);
                    let mut output = compute_grid_layout(&mut table_wrapper, node_id, inputs);

                    // Absolutely positioned children take no part in the table grid. Their
                    // static position is the top-left of the table's content box.
                    let context = table_wrapper.ctx;
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
                            y: padding.top + border.top,
                        }
                        .map(|pos| AxisStaticPosition::from_edge(pos, AxisStaticEdge::Start));
                        let in_flow_count = context.cells.len();
                        for (idx, (child_id, position)) in context.oof_children.iter().enumerate() {
                            output.oof_candidates.push(OofCandidate {
                                node: taffy_node_id(*child_id),
                                order: (in_flow_count + idx) as u32,
                                position: *position,
                                static_position,
                            });
                        }
                    }

                    // HACK: Cap scrollable overflow at node size to prevent scrolling
                    output.scrollable_overflow_rect.left = 0.0;
                    output.scrollable_overflow_rect.top = 0.0;
                    output.scrollable_overflow_rect.right =
                        output.scrollable_overflow_rect.right.min(output.size.width);
                    output.scrollable_overflow_rect.bottom = output
                        .scrollable_overflow_rect
                        .bottom
                        .min(output.size.height);

                    return output;
                }

                if node.flags.is_inline_root() {
                    return self.compute_inline_layout(dom_node_id(node_id), inputs, block_ctx);
                }

                // The default CSS file will set
                match node.taffy_display() {
                    Display::Block => compute_block_layout(self, node_id, inputs, block_ctx),
                    Display::FlowRoot => compute_block_layout(self, node_id, inputs, None),
                    Display::Flex => {
                        #[cfg(feature = "writing-mode")]
                        {
                            self.current_align_axis_is_inline = node.is_column_flex_container();
                        }
                        compute_flexbox_layout(self, node_id, inputs)
                    }
                    Display::Grid => compute_grid_layout(self, node_id, inputs),
                    Display::None => taffy::LayoutOutput::HIDDEN,
                }
            }
            NodeData::Document(_) => compute_block_layout(self, node_id, inputs, None),

            _ => taffy::LayoutOutput::HIDDEN,
        }
    }
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
}

impl taffy::CacheTree for LayoutPassState<'_> {
    #[inline]
    fn cache_get(
        &mut self,
        node_id: NodeId,
        inputs: &taffy::LayoutInput,
    ) -> Option<taffy::LayoutOutput> {
        self.node_from_id_mut(node_id)
            .try_layout_data_mut()?
            .cache
            .get(inputs)
    }

    #[inline]
    fn cache_store(
        &mut self,
        node_id: NodeId,
        inputs: &taffy::LayoutInput,
        layout_output: taffy::LayoutOutput,
    ) {
        self.node_from_id_mut(node_id)
            .cache_mut()
            .store(inputs, layout_output);
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
}

impl RoundTree for LayoutPassState<'_> {
    fn get_unrounded_layout(&self, node_id: NodeId) -> Layout {
        *self.node_from_id(node_id).unrounded_layout()
    }

    fn set_final_layout(&mut self, node_id: NodeId, layout: &Layout) {
        *self.node_from_id_mut(node_id).final_layout_mut() = *layout;
    }

    fn is_out_of_flow(&self, node_id: NodeId) -> bool {
        self.node_from_id(node_id).is_out_of_flow()
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
    fn rounded_unchanged(&self, node_id: NodeId, unrounded: &Layout, absolute_x: f32, absolute_y: f32) -> bool {
        let node = self.node_from_id(node_id);
        !node.is_anonymous()
            && !node.has_damaged_descendants()
            && node.damage().is_some_and(|d| d.is_empty())
            && node.layout_data().rounded_from == Some((*unrounded, absolute_x, absolute_y))
    }

    fn record_rounded(&mut self, node_id: NodeId, unrounded: &Layout, absolute_x: f32, absolute_y: f32) {
        self.node_from_id_mut(node_id).layout_data_mut().rounded_from = Some((*unrounded, absolute_x, absolute_y));
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

/// lightningpanda: the `natural=W,H` token of the element's
/// `urn:x-lightningpanda:state` attribute.
fn lp_natural_size(element: &crate::node::ElementData) -> Option<(f32, f32)> {
    let attr =
        element.attrs.iter().find(|a| &*a.name.ns == "urn:x-lightningpanda:state" && &*a.name.local == "state")?;
    let token = attr.value.split_ascii_whitespace().find_map(|t| t.strip_prefix("natural="))?;
    let (w, h) = token.split_once(',')?;
    Some((w.parse().ok()?, h.parse().ok()?))
}
