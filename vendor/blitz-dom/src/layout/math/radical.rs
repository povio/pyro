//! pyro: the layout class of `msqrt` and `mroot` (MathML Core § 3.3.3, Chromium's
//! `MathMLRadicalLayoutAlgorithm`).
//!
//! The radicand is a row (an `msqrt`'s children, an `mroot`'s first child) after the radical sign
//! (U+221A in the box's font, the smallest size variant of the font's MATH table that is at least
//! as tall as `RadicalRuleThickness + gap + the radicand's height`, the gap being
//! `RadicalDisplayStyleVerticalGap` in display style and `RadicalVerticalGap` otherwise), under an
//! overbar of `RadicalRuleThickness` that starts where the radicand does and `RadicalExtraAscender`
//! below the box's top. An `mroot`'s index precedes them: `RadicalKernBeforeDegree` (at least 0)
//! from the start, `RadicalKernAfterDegree` (at least minus the index's width) after it, its
//! baseline raised `RadicalDegreeBottomRaisePercent` of the radical's height above the
//! radical's bottom.
//!
//! What it records for paint (`ElementData::math_radical`, drawn by blitz-paint's
//! `draw_math_radical`): the bar and the sign's glyph, font and position.
//!
//! Not done: a glyph assembly (when no size variant is tall enough the box is sized as the
//! assembly would be, to the target, but the largest variant is the glyph painted), and the
//! `rtlm`/mirrored glyph of an `rtl` radical (the sign is the same glyph, on the end side).
use markup5ever::ns;
use thin_vec::ThinVec;

use super::types::{ChildMetrics, ChildPlacement, MathContext, MathKind, MathLayout};
use super::underover::{Fallbacks, MathFace, math_constant};
use super::{MathConstant, row};
use crate::layout::LayoutPassState;
use crate::node::{MathRadical, MathRadicalSign};
use crate::{BaseDocument, NodeId};

/// Construction: the children are the layout children; the radical paint of an earlier layout is
/// dropped.
pub(crate) fn construct(doc: &mut BaseDocument, node_id: NodeId, _kind: MathKind, children: &mut ThinVec<NodeId>) -> bool {
    if let Some(element) = doc.nodes[node_id].element_data_mut() {
        element.math_radical = None;
    }
    super::build_children(doc, node_id, children)
}

/// The radical sign for a target height: its metrics (CSS px) and, when the box's font draws one,
/// the glyph to paint.
#[derive(Clone, Debug, Default)]
struct Sign {
    advance: f32,
    ascent: f32,
    descent: f32,
    /// The font, the glyph to paint and the font size.
    paint: Option<(parley::FontData, u16, f32)>,
}

/// Chromium's "shape a stretchy glyph" for U+221A (§ 5.3): the first size variant at least
/// `target` tall (its advance along the vertical axis); the largest one if there is none, the box
/// then sized to `target` when the font has an assembly for it.
fn shape_sign(face: &MathFace, target: f32) -> Sign {
    let Some(font_ref) = face.font_ref() else { return Sign::default() };
    let Some(scale) = face.scale(&font_ref) else { return Sign::default() };
    let Some(base) = face.glyph(&font_ref, '\u{221A}') else { return Sign::default() };
    let mut chosen = base;
    let mut assembly_bottom = None;
    if let Some(math) = font_ref.table_data(skrifa::Tag::new(b"MATH")) {
        if let Some(construction) = super::types::glyph_construction(math.as_bytes(), base, false) {
            match construction.variants.iter().find(|variant| f32::from(variant.advance) * scale >= target) {
                Some(variant) => chosen = variant.glyph,
                None => {
                    if let Some(last) = construction.variants.last() {
                        chosen = last.glyph;
                    }
                    assembly_bottom = construction.parts.first().map(|part| part.glyph);
                }
            }
        }
    }
    let Some((advance, ascent, descent)) = face.glyph_extents(chosen) else { return Sign::default() };
    let paint = Some((face.font_data(), chosen, face.size()));
    match assembly_bottom {
        // An assembly reaches the target whatever the variants: the box is that tall, the
        // bottom part's depth under the baseline.
        Some(bottom) if ascent + descent < target => {
            let bottom_descent = face.glyph_extents(bottom).map_or(0.0, |(_, _, descent)| descent);
            Sign { advance, ascent: (target - bottom_descent).max(0.0), descent: bottom_descent, paint }
        }
        _ => Sign { advance, ascent, descent, paint },
    }
}

/// Layout of a box `construct` built (see the module's documentation).
pub(crate) fn layout(state: &mut LayoutPassState<'_>, node_id: NodeId, kind: MathKind, ctx: &MathContext, children: &[ChildMetrics]) -> MathLayout {
    let (radicand, index) = match kind {
        MathKind::Root if children.len() == 2 => (&children[..1], Some(children[1])),
        MathKind::Root => return row::layout_row(ctx, children),
        _ => (children, None),
    };
    let fallbacks = Fallbacks::load(state, node_id, ctx);
    let constant = |c: MathConstant| math_constant(ctx, &fallbacks, c);
    let rule = constant(MathConstant::RadicalRuleThickness);
    let extra = constant(MathConstant::RadicalExtraAscender);
    let gap = constant(if ctx.display_style { MathConstant::RadicalDisplayStyleVerticalGap } else { MathConstant::RadicalVerticalGap });

    // The radicand, a row.
    let base = row::layout_row(ctx, radicand);
    let base_height = base.ascent + base.descent;
    let sign = MathFace::of(state, node_id).map(|face| shape_sign(&face, rule + gap + base_height)).unwrap_or_default();

    // § 3.3.3.2: the radical (sign and radicand) `B`.
    let b_ascent = base.ascent.max(base.ascent + gap + rule + extra);
    let b_descent = base.descent.max(sign.ascent + sign.descent + extra - b_ascent);
    let b_width = sign.advance + base.content_width;

    // § 3.3.3.3: the index before it, its baseline raised.
    let (kern_before, kern_after, index_width) = match &index {
        Some(index) => {
            let width = index.margin_box_width();
            (constant(MathConstant::RadicalKernBeforeDegree).max(0.0), constant(MathConstant::RadicalKernAfterDegree).max(-width), width)
        }
        None => (0.0, 0.0, 0.0),
    };
    let radical_offset = if index.is_some() { kern_before + index_width + kern_after } else { 0.0 };
    let (mut ascent, mut descent) = (b_ascent, b_descent);
    let mut index_baseline_height = 0.0;
    if let Some(index) = &index {
        let raise = constant(MathConstant::RadicalDegreeBottomRaisePercent) / 100.0 * (b_ascent + b_descent);
        // The index's margin box bottom `raise` above the radical's bottom.
        index_baseline_height = -b_descent + raise + index.descent;
        ascent = ascent.max(index_baseline_height + index.ascent);
        descent = descent.max(b_descent - raise);
    }

    let radical_top = ascent - b_ascent;
    // Layout children are in document order: the radicand (the root's base) first, then the index.
    let mut placements = Vec::with_capacity(children.len());
    for placement in &base.children {
        placements.push(ChildPlacement {
            metrics: placement.metrics,
            inline_offset: radical_offset + sign.advance + placement.inline_offset,
            block_offset: radical_top + (b_ascent - base.ascent) + placement.block_offset,
        });
    }
    if let Some(index) = index {
        placements.push(ChildPlacement { metrics: index, inline_offset: kern_before, block_offset: ascent - index_baseline_height - index.ascent });
    }
    let content_width = radical_offset + b_width;

    // The paint: the overbar over the radicand and the sign at the radical's start, its top on the
    // bar's.
    let inset = ctx.inset;
    let bar_top = inset.top + radical_top + extra;
    let radical = MathRadical {
        rtl: ctx.rtl,
        bar_start: radical_offset + sign.advance,
        bar_width: base.content_width,
        bar_top,
        bar_thickness: rule,
        sign: sign.paint.clone().map(|(font, glyph, size)| MathRadicalSign {
            font,
            glyph,
            size,
            start: radical_offset,
            width: sign.advance,
            baseline: bar_top + sign.ascent,
        }),
    };
    if let Some(element) = state.nodes[node_id].element_data_mut() {
        if element.name.ns == ns!(mathml) {
            element.math_radical = Some(radical);
        }
    }
    MathLayout { content_width, ascent, descent, children: placements, italic_correction: 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NodeId;
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

    #[test]
    fn radicand_row_sizes_the_target() {
        // The sign is as tall as the rule, the gap and the radicand.
        let ctx = MathContext {
            font_size: 10.0,
            display_style: false,
            math_depth: 0,
            cramped: false,
            rtl: false,
            font: Default::default(),
            padding: Rect::ZERO,
            border: Rect::ZERO,
            inset: Rect::ZERO,
        };
        let base = row::layout_row(&ctx, &[child(1, 30.0, 10.0, 2.0), child(2, 5.0, 4.0, 6.0)]);
        assert_eq!((base.content_width, base.ascent, base.descent), (35.0, 10.0, 6.0));
    }
}
