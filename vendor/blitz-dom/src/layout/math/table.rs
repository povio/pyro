//! pyro: the layout class of `mtable`, `mtr`, `mlabeledtr` and `mtd` (MathML Core § 3.5, lane
//! mathml-9). The table is CSS's: the user agent sheet makes an `mtable` an inline table, an `mtr`
//! a table row and an `mtd` a table cell (`assets/default.css`), laid out by the table layout. What
//! is MathML's is here:
//!
//! * a cell lays its children out as a row (`construct`; Core § 3.5.2: "the children of an `mtd`
//!   are laid out as an inferred `mrow`"), centred in the width its column gives it (the
//!   driver's `Table(Cell)` centring), so its operators stretch to its content;
//! * the table's baseline is the math axis: the middle of its box is `AxisHeight` above its
//!   baseline (`on_axis`, applied where a math box measures the table as a child; Chromium's
//!   `LayoutNGMathMLTable` baseline), whatever baseline the CSS table has.
//!
//! Not done: `mlabeledtr` (its label is not hidden or laid out apart), the `columnalign`,
//! `rowalign`, `columnspacing`, `rowspacing`, `frame`, `side` and similar legacy attributes (Core
//! drops them), and `align`.
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

/// The metrics of the child box `metrics.id` of a math box with a table's baseline: for an `mtable`
/// the middle of its border box on the math axis (`AxisHeight` of its font, else half its
/// x-height); any other box unchanged.
pub(crate) fn on_axis(state: &LayoutPassState<'_>, metrics: ChildMetrics) -> ChildMetrics {
    let node = &state.nodes[metrics.id];
    let is_table = node.element_data().is_some_and(|element| element.name.ns == markup5ever::ns!(mathml) && &*element.name.local == "mtable");
    if !is_table {
        return metrics;
    }
    let Some(style) = node.primary_styles() else { return metrics };
    let font_size = style.slow_clone_font_size().used_size().px();
    let provider = crate::font_metrics::BlitzFontMetricsProvider { font_ctx: state.font_ctx.clone() };
    let font = provider.math_parameters_for(style.get_font(), font_size);
    let axis = font.constant(super::MathConstant::AxisHeight).unwrap_or(font.x_height.unwrap_or(font_size * 0.5) / 2.0);
    let ascent = metrics.margin.top + metrics.size.height / 2.0 + axis;
    ChildMetrics { ascent, descent: metrics.margin_box_height() - ascent, ..metrics }
}
