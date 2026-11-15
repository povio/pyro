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
fn relative_offset(style: &ComputedValues, cb: taffy::Size<f32>, cb_rtl: bool) -> taffy::Point<f32> {
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
    /// the root's content box.
    pub fn inline_span_relative_offset(&self, node_id: NodeId, root_id: NodeId) -> (f32, f32) {
        let layout = self.nodes[root_id].unrounded_layout();
        let cb = taffy::Size {
            width: layout.size.width - layout.border.left - layout.border.right - layout.padding.left - layout.padding.right,
            height: layout.size.height - layout.border.top - layout.border.bottom - layout.padding.top - layout.padding.bottom,
        };
        let offset = inline_span_offset(&self.nodes, node_id, root_id, cb);
        (offset.x, offset.y)
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
