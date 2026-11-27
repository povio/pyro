//! pyro: the row (MathML Core § 3.1: `math`, `mrow`, `mstyle`, `merror`, `mphantom`, `maction`,
//! `semantics`, `a`, unknown elements, and any element with invalid children): Chromium's
//! `MathMLRowLayoutAlgorithm`. The children, each at its max-content size, one after the other
//! from the start edge (the left one first, the right one first in an `rtl` row: the driver
//! mirrors the offsets), their baselines on one line; the row's ascent is its highest child's and
//! its descent its deepest's, a child's margins counted in both.
use thin_vec::ThinVec;

use super::types::{ChildMetrics, ChildPlacement, MathContext, MathKind, MathLayout};
use crate::{BaseDocument, NodeId};

/// Construction of a row: its element children are its layout children (`super::build_children`).
pub(crate) fn construct(doc: &mut BaseDocument, node_id: NodeId, _kind: MathKind, children: &mut ThinVec<NodeId>) -> bool {
    super::build_children(doc, node_id, children)
}

/// The row's layout of `children`.
pub(crate) fn layout_row(_ctx: &MathContext, children: &[ChildMetrics]) -> MathLayout {
    let ascent = children.iter().fold(0.0f32, |ascent, child| ascent.max(child.ascent));
    let descent = children.iter().fold(0.0f32, |descent, child| descent.max(child.descent));
    let mut offset = 0.0f32;
    let mut placements = Vec::with_capacity(children.len());
    for child in children {
        placements.push(ChildPlacement { metrics: *child, inline_offset: offset, block_offset: ascent - child.ascent });
        offset += child.margin_box_width();
    }
    MathLayout { content_width: offset, ascent, descent, children: placements, italic_correction: 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use taffy::{Rect, ScrollableOverflow, Size};

    fn child(id: u64, width: f32, ascent: f32, descent: f32) -> ChildMetrics {
        ChildMetrics {
            id: NodeId::from_u64(id),
            margin: Rect::ZERO,
            size: Size { width, height: ascent + descent },
            overflow: ScrollableOverflow::ZERO,
            ascent,
            descent,
            italic_correction: 0.0,
            kind: None,
        }
    }

    fn context() -> MathContext {
        MathContext {
            font_size: 16.0,
            display_style: false,
            math_depth: 0,
            cramped: false,
            rtl: false,
            font: Default::default(),
            padding: Rect::ZERO,
            border: Rect::ZERO,
            inset: Rect::ZERO,
        }
    }

    #[test]
    fn baselines_line_up() {
        let layout = layout_row(&context(), &[child(1, 10.0, 8.0, 2.0), child(2, 5.0, 3.0, 6.0)]);
        assert_eq!((layout.content_width, layout.ascent, layout.descent), (15.0, 8.0, 6.0));
        assert_eq!((layout.children[0].inline_offset, layout.children[0].block_offset), (0.0, 0.0));
        assert_eq!((layout.children[1].inline_offset, layout.children[1].block_offset), (10.0, 5.0));
    }

    #[test]
    fn empty_row_has_no_size() {
        let layout = layout_row(&context(), &[]);
        assert_eq!((layout.content_width, layout.ascent, layout.descent), (0.0, 0.0, 0.0));
    }
}
