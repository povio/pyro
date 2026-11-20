//! pyro: relative positioning of inline boxes that are not atomic (CSS 2.1 § 9.4.3): such a box
//! has no layout box of its own, so the offset moves what its fragments hold where they are
//! painted (glyph runs, the boxes of its fragments) and the atomic inline boxes in it.

use blitz_traits::node_id::NodeId;
use style::computed_values::direction::T as Direction;
use style::computed_values::position::T as Position;
use style::properties::ComputedValues;
use style::values::computed::{Inset, Length};
use stylo_taffy::CopyStyleGetters;

use crate::{BaseDocument, NodeTree};

/// The relative offset `style` gives a box (CSS px), percentages against `cb`, the size of its
/// containing block: `left` wins over `right` where the containing block is left-to-right, and
/// `top` over `bottom`.
pub(crate) fn relative_offset(style: &ComputedValues, cb: taffy::Size<f32>, cb_rtl: bool) -> taffy::Point<f32> {
    let position = style.get_position();
    let resolve = |inset: &Inset, basis: f32| match inset {
        Inset::LengthPercentage(value) => Some(value.resolve(Length::new(basis)).px()),
        _ => None,
    };
    let left = resolve(&position.left, cb.width);
    let right = resolve(&position.right, cb.width).map(|right| -right);
    let top = resolve(&position.top, cb.height);
    let bottom = resolve(&position.bottom, cb.height).map(|bottom| -bottom);
    taffy::Point {
        x: if cb_rtl { right.or(left) } else { left.or(right) }.unwrap_or(0.0),
        y: top.or(bottom).unwrap_or(0.0),
    }
}

/// How far `position: relative` moves the inline content of `node_id`, laid out by the inline
/// root `root_id` (CSS px): the sum of the relative offsets of the inline boxes that are not
/// atomic from `node_id` up to the root. A split inline (`NodeFlags::IS_SPLIT_INLINE`) whose box
/// holds the root (an anonymous block between two of its blocks, the root's parent) is not
/// reached: its offset moves its box with everything in it. Its head and tail, in the anonymous
/// blocks of its container, are moved here.
pub(crate) fn inline_span_offset(
    nodes: &NodeTree,
    node_id: NodeId,
    root_id: NodeId,
    cb: taffy::Size<f32>,
) -> taffy::Point<f32> {
    inline_span_offset_with(nodes, node_id, root_id, cb, false)
}

/// [`inline_span_offset`], with the `position: sticky` offsets of the inline boxes on the way
/// when `sticky` (`Node::sticky_offset`: where scrolling holds them, known once laid out).
fn inline_span_offset_with(
    nodes: &NodeTree,
    node_id: NodeId,
    root_id: NodeId,
    cb: taffy::Size<f32>,
    sticky: bool,
) -> taffy::Point<f32> {
    let root = &nodes[root_id];
    // The spans of an anonymous inline root are descendants of its parent.
    let root_parent = root.is_anonymous().then_some(root.parent).flatten();
    let cb_rtl = root.primary_styles().is_some_and(|style| style.clone_direction() == Direction::Rtl);
    let mut offset = taffy::Point::ZERO;
    let mut current = Some(node_id);
    while let Some(id) = current {
        if id == root_id || Some(id) == root_parent {
            break;
        }
        let Some(node) = nodes.get(id) else {
            break;
        };
        if let Some(style) = node.primary_styles() {
            let display = style.clone_display();
            if !display.is_inline_flow() && !display.is_contents() {
                break;
            }
            if style.clone_position() == Position::Relative && display.is_inline_flow() {
                let own = relative_offset(&style, cb, cb_rtl);
                offset.x += own.x;
                offset.y += own.y;
            }
            if sticky && style.clone_position() == Position::Sticky && display.is_inline_flow() {
                let own = node.sticky_offset();
                offset.x += own.x as f32;
                offset.y += own.y as f32;
            }
        }
        current = node.parent;
    }
    offset
}

/// [`inline_span_offset`] in the axes the inline root `root_id` is laid out in (x along its
/// lines, y across them, towards its block end), with `cb` in those axes: the physical offset
/// turned for a vertical writing mode.
pub(crate) fn inline_span_flow_offset(
    nodes: &NodeTree,
    node_id: NodeId,
    root_id: NodeId,
    cb: taffy::Size<f32>,
) -> taffy::Point<f32> {
    let root = &nodes[root_id];
    let wm = root
        .primary_styles()
        .or_else(|| root.parent.and_then(|parent| nodes[parent].primary_styles()))
        .map(|style| style.writing_mode);
    match wm {
        Some(wm) if wm.is_vertical() => {
            let physical = inline_span_offset(nodes, node_id, root_id, taffy::Size { width: cb.height, height: cb.width });
            taffy::Point {
                x: if wm.contains(style::logical_geometry::WritingMode::WRITING_MODE_SIDEWAYS_LR) { -physical.y } else { physical.y },
                y: if wm.is_vertical_lr() { physical.x } else { -physical.x },
            }
        }
        _ => inline_span_offset(nodes, node_id, root_id, cb),
    }
}

impl BaseDocument {
    /// pyro: how far `position: relative` moves what the inline box (not atomic) `node_id` holds,
    /// laid out by the inline root `root_id` (CSS px): the relative offsets of it and of the
    /// inline boxes around it, up to the root (see [`inline_span_offset`]), percentages against
    /// the root's content box; and how far `position: sticky` holds them where they are painted.
    pub fn inline_span_relative_offset(&self, node_id: NodeId, root_id: NodeId) -> (f32, f32) {
        let layout = self.nodes[root_id].unrounded_layout();
        let cb = taffy::Size {
            width: layout.size.width - layout.border.left - layout.border.right - layout.padding.left - layout.padding.right,
            height: layout.size.height - layout.border.top - layout.border.bottom - layout.padding.top - layout.padding.bottom,
        };
        let offset = inline_span_offset_with(&self.nodes, node_id, root_id, cb, true);
        (offset.x, offset.y)
    }

    /// pyro: the positioned (relative or sticky) inline boxes that are not atomic from `node_id`
    /// up to the inline root `root_id`, innermost first: their content paints with them, after
    /// the in-flow content (CSS 2.1 Appendix E step 8), not in the root's text. An inline box faded
    /// or filtered (a stacking context, [`effect_entry`]) paints so too, at its stacking level.
    pub fn inline_span_positioned_chain(&self, node_id: NodeId, root_id: NodeId) -> Vec<NodeId> {
        let root = &self.nodes[root_id];
        let root_parent = root.is_anonymous().then_some(root.parent).flatten();
        let mut chain = Vec::new();
        let mut current = Some(node_id);
        while let Some(id) = current {
            if id == root_id || Some(id) == root_parent {
                break;
            }
            let Some(node) = self.nodes.get(id) else { break };
            if let Some(style) = node.primary_styles() {
                let display = style.clone_display();
                if !display.is_inline_flow() && !display.is_contents() {
                    break;
                }
                if display.is_inline_flow()
                    && (matches!(style.clone_position(), Position::Relative | Position::Sticky)
                        || (effect_entry(&style) && painted_ancestor(&self.nodes, id, root_id)))
                {
                    chain.push(id);
                }
            }
            current = node.parent;
        }
        chain
    }

    /// pyro: whether some inline box (not atomic) the inline root `root_id` lays out is
    /// relatively positioned, so its content is painted moved (see [`inline_span_offset`]).
    pub fn inline_root_has_relative_spans(&self, root_id: NodeId) -> bool {
        self.nodes[root_id]
            .element_data()
            .and_then(|element| element.inline_layout_data.as_deref())
            .is_some_and(|layout| layout.relative_spans)
    }
}

/// pyro: whether an inline box between `id` and its inline root `root_id` paints a box (a background,
/// a border or a shadow): the root's effect pass (blitz-paint `draw_inline_effects`), which paints a
/// faded or filtered inline box's area after the rest of the layout, would paint that box over it
/// again; such an inline box paints at its own entry instead ([`effect_entry`]).
pub(crate) fn painted_ancestor(nodes: &crate::NodeTree, id: NodeId, root_id: NodeId) -> bool {
    let root = &nodes[root_id];
    let container = root.is_anonymous().then_some(root.parent).flatten();
    let mut ancestor = nodes.get(id).and_then(|node| node.parent);
    while let Some(node) = ancestor.filter(|&a| a != root_id && Some(a) != container).and_then(|a| nodes.get(a)) {
        if let Some(style) = node.primary_styles() {
            let background = style.get_background();
            let border = style.get_border();
            let paints = background.background_color.resolve_to_absolute(&style.clone_color()).alpha > 0.0
                || background.background_image.0.iter().any(|image| !matches!(image, style::values::generics::image::GenericImage::None))
                || [
                    (border.border_top_style, &border.border_top_width),
                    (border.border_right_style, &border.border_right_width),
                    (border.border_bottom_style, &border.border_bottom_width),
                    (border.border_left_style, &border.border_left_width),
                ]
                .iter()
                .any(|(line, width)| !line.none_or_hidden() && width.0.to_f32_px() > 0.0)
                || !style.get_effects().box_shadow.0.is_empty();
            if paints {
                return true;
            }
        }
        ancestor = node.parent;
    }
    false
}

/// pyro: whether an inline box (not atomic) of style `style` is a stacking context by its opacity or
/// filter functions alone (no clip path, mask, SVG filter or backdrop filter, which its inline root
/// paints for it): its
/// content paints at its stacking level, as a positioned inline box's does, in its effect's group,
/// after the in-flow content and over its ancestors' boxes (CSS 2.1 Appendix E step 8; Chromium).
pub fn effect_entry(style: &style::properties::ComputedValues) -> bool {
    use style::values::computed::basic_shape::ClipPath;
    use style::values::generics::image::GenericImage;
    let effects = style.get_effects();
    (effects.opacity < 1.0 || !effects.filter.0.is_empty())
        // (An SVG filter, a graph its inline root paints in its filter region, is left to it.)
        && !effects.filter.0.iter().any(|filter| matches!(filter, style::values::computed::Filter::Url(_)))
        && effects.backdrop_filter.0.is_empty()
        && matches!(style.get_clip_path(), ClipPath::None)
        && style.get_svg().mask_image.0.iter().all(|image| matches!(image, GenericImage::None))
}
