//! pyro: the layout class of `mtable`, `mtr`, `mlabeledtr` and `mtd` (lane mathml-9), empty: `construct` returns `false`, so the elements are
//! built and laid out as they were before `layout::math` existed. The lane that fills it edits
//! this file only. See the interface at the top of `layout/math/mod.rs`: `construct` pushes the
//! layout children (`super::build_children`) and returns `true` to make the element a math
//! box, `layout` lays it out (and is only called for a box `construct` built).
use thin_vec::ThinVec;

use super::types::{ChildMetrics, MathContext, MathKind, MathLayout};
use crate::layout::LayoutPassState;
use crate::{BaseDocument, NodeId};

/// Construction (see the module's documentation): a cell lays its children out as an `mrow` does
/// (MathML Core § 3.6.3: "the `mtd` ... children are laid out with the mrow algorithm"), so that
/// white space between them takes no width, as in the row it is compared with; the other elements
/// are built as they were (`false`).
pub(crate) fn construct(doc: &mut BaseDocument, node_id: NodeId, kind: MathKind, children: &mut ThinVec<NodeId>) -> bool {
    match kind {
        MathKind::Table(super::TableKind::Cell) => super::row::construct(doc, node_id, kind, children),
        _ => false,
    }
}

/// Layout of a box `construct` built (see the module's documentation): the row's until filled.
pub(crate) fn layout(
    _state: &mut LayoutPassState<'_>,
    _node_id: NodeId,
    _kind: MathKind,
    ctx: &MathContext,
    children: &[ChildMetrics],
) -> MathLayout {
    super::row::layout_row(ctx, children)
}
