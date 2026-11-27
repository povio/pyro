//! pyro: the layout class of `munder`, `mover` and `munderover` (lane mathml-5), empty: `construct` returns `false`, so the elements are
//! built and laid out as they were before `layout::math` existed. The lane that fills it edits
//! this file only. See the interface at the top of `layout/math/mod.rs`: `construct` pushes the
//! layout children (`super::build_children`) and returns `true` to make the element a math
//! box, `layout` lays it out (and is only called for a box `construct` built).
use thin_vec::ThinVec;

use super::types::{ChildMetrics, MathContext, MathKind, MathLayout};
use crate::layout::LayoutPassState;
use crate::{BaseDocument, NodeId};

/// Construction (see the module's documentation): `false`, the element is built as it was.
pub(crate) fn construct(_doc: &mut BaseDocument, _node_id: NodeId, _kind: MathKind, _children: &mut ThinVec<NodeId>) -> bool {
    false
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
