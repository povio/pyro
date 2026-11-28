//! pyro: the layout class of `mpadded` (MathML Core § 3.3.6, Chromium's
//! `MathMLPaddedLayoutAlgorithm`).
//!
//! The children are laid out as an anonymous row (the inner box). The box's size is then the
//! requested one: `width`, `height` and `depth` where the attribute is a valid non-percentage
//! `<length>` (negative `height` and `depth` clamped to 0), the inner box's own where it is absent,
//! invalid or a percentage (a CSS `width`, which the driver resolves, applies after: the
//! attribute is only the author-level default). The legacy pseudo-units (`200%height`) are not
//! lengths: such an attribute is invalid. The content is moved `lspace` (clamped to 0, 0 for a
//! percentage) towards the end of the line and `voffset` upward (a negative one downward), neither
//! of which changes the box's size or preferred width. In an `rtl` box the driver mirrors the
//! `lspace` shift.
use thin_vec::ThinVec;

use super::fraction::parse_line_thickness;
use super::row;
use super::types::{ChildMetrics, ChildPlacement, MathContext, MathKind, MathLayout};
use crate::layout::LayoutPassState;
use crate::{BaseDocument, NodeId};

/// Construction: the children are the layout children.
pub(crate) fn construct(doc: &mut BaseDocument, node_id: NodeId, _kind: MathKind, children: &mut ThinVec<NodeId>) -> bool {
    super::build_children(doc, node_id, children)
}

/// The value of a `<length>` attribute in CSS px: `None` if it is absent, a percentage or
/// invalid. `signed` allows a negative value (`voffset`; the others clamp it themselves).
fn length(value: Option<&str>, ctx: &MathContext, signed: bool) -> Option<f32> {
    let value = value?.trim_matches(|c: char| c.is_ascii_whitespace());
    if value.ends_with('%') {
        return None;
    }
    let x_height = ctx.font.x_height.unwrap_or(ctx.font_size * 0.5);
    match value.strip_prefix('-') {
        Some(magnitude) if signed || magnitude.chars().next().is_some_and(|c| c.is_ascii_digit() || c == '.') => {
            // (`parse_line_thickness` has no negative values: the sign is applied here.)
            parse_line_thickness(magnitude, 0.0, ctx.font_size, x_height).map(|px| -px)
        }
        _ => parse_line_thickness(value, 0.0, ctx.font_size, x_height),
    }
}

/// The attribute `name` of `node_id`, if it has one.
fn attribute(state: &LayoutPassState<'_>, node_id: NodeId, name: &str) -> Option<String> {
    let element = state.nodes[node_id].element_data()?;
    element.attrs().iter().find(|a| a.name.ns == markup5ever::ns!() && &*a.name.local == name).map(|a| a.value.to_string())
}

/// Layout of a box `construct` built (see the module's documentation).
pub(crate) fn layout(state: &mut LayoutPassState<'_>, node_id: NodeId, _kind: MathKind, ctx: &MathContext, children: &[ChildMetrics]) -> MathLayout {
    let inner = row::layout_row(ctx, children);
    let get = |name: &str, signed: bool| length(attribute(state, node_id, name).as_deref(), ctx, signed);
    let width = get("width", false).unwrap_or(inner.content_width).max(0.0);
    let height = get("height", false).map_or(inner.ascent, |value| value.max(0.0));
    let depth = get("depth", false).map_or(inner.descent, |value| value.max(0.0));
    let lspace = get("lspace", false).map_or(0.0, |value| value.max(0.0));
    let voffset = get("voffset", true).unwrap_or(0.0);

    let placements = inner
        .children
        .iter()
        .map(|placement| ChildPlacement {
            metrics: placement.metrics,
            inline_offset: placement.inline_offset + lspace,
            block_offset: placement.block_offset + (height - inner.ascent) - voffset,
        })
        .collect();
    MathLayout { content_width: width, ascent: height, descent: depth, children: placements, italic_correction: 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use taffy::Rect;

    fn context() -> MathContext {
        MathContext {
            font_size: 10.0,
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
    fn lengths() {
        let ctx = context();
        assert_eq!(length(Some("25px"), &ctx, false), Some(25.0));
        assert_eq!(length(Some(" 2em "), &ctx, false), Some(20.0));
        assert_eq!(length(Some("50%"), &ctx, false), None);
        assert_eq!(length(Some("200%height"), &ctx, false), None);
        assert_eq!(length(Some("10"), &ctx, false), None);
        assert_eq!(length(Some("-5px"), &ctx, true), Some(-5.0));
        assert_eq!(length(Some("-5px"), &ctx, false), Some(-5.0));
        assert_eq!(length(None, &ctx, false), None);
    }
}
