//! pyro: the layout class of `munder`, `mover` and `munderover` (MathML Core § 3.4.2, Chromium's
//! `MathMLUnderOverLayoutAlgorithm`), and the helpers the script and radical classes share: the
//! first available font of a box (`MathFace`), the MATH constants with the spec's fallbacks for a
//! font without a MATH table (`math_constant`), and the operator properties of a base
//! (`embellished_operator`).
//!
//! The base on the baseline, the underscript below it and the overscript above it, each centred
//! on the base (an overscript by its top accent attachment, the pair shifted by half the italic
//! correction of a large operator base). The gaps are the font's `UpperLimit*`/`LowerLimit*`
//! constants for a large operator, `StretchStack*` for a base that stretches horizontally, and
//! the over/underbar constants (`AccentBaseHeight` for an accent) for any other base. In compact
//! style a base that is an operator with `movablelimits` makes the element lay out as
//! `msub`/`msup`/`msubsup` (`scripts`). The horizontally stretchy children (`stretchy` operators
//! with the dictionary's `horizontal` axis) are measured again at the width of the widest other
//! child (§ 3.4.2.2).
//!
//! Not done: the ink extents are the children's line metrics (a glyph overflowing its box does
//! not count), the form of a base operator for the dictionary is the `form` attribute or the
//! dictionary's first form for its content, and the stretched operator's own glyph is its
//! class's (`operator`).
use markup5ever::{LocalName, ns};
use taffy::Size;
use thin_vec::ThinVec;

use super::types::{ChildMetrics, ChildPlacement, MathContext, MathKind, MathLayout};
use super::{MathConstant, row, scripts};
use crate::layout::LayoutPassState;
use crate::mathml_operator::{self, Form};
use crate::{BaseDocument, NodeId};

/// Construction (see the module's documentation): the children are the layout children.
pub(crate) fn construct(doc: &mut BaseDocument, node_id: NodeId, _kind: MathKind, children: &mut ThinVec<NodeId>) -> bool {
    super::build_children(doc, node_id, children)
}

// ---------------------------------------------------------------------------------------------
// The font of a box and the MATH constants.

/// The first available font of a box at its font size: what the math classes read glyphs and
/// metrics from that `MathContext::font` does not have.
pub(crate) struct MathFace {
    font: parley::fontique::QueryFont,
    size: f32,
}

/// What the spec's fallbacks for a font without a MATH table are made of (CSS px): the default
/// rule thickness (`post.underlineThickness`, else 0) and the OS/2 script offsets.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Fallbacks {
    pub(crate) rule_thickness: f32,
    pub(crate) subscript_shift_down: f32,
    pub(crate) superscript_shift_up: f32,
}

impl MathFace {
    /// The first font of `node_id`'s `font-family` list (at its weight, width and style) that
    /// has U+0020, as `BlitzFontMetricsProvider::math_parameters_for` finds it.
    pub(crate) fn of(state: &LayoutPassState<'_>, node_id: NodeId) -> Option<MathFace> {
        use crate::stylo_to_parley as to_parley;
        use parley::fontique::{Attributes, QueryFamily, QueryFont, QueryStatus};
        use skrifa::MetadataProvider as _;

        let node = &state.nodes[node_id];
        let style = node.primary_styles()?;
        let size = style.slow_clone_font_size().used_size().px();
        let font_styles = style.get_font();
        let mut font_ctx = state.font_ctx.lock().unwrap();
        let font_ctx = &mut *font_ctx;
        let mut query = font_ctx.collection.query(&mut font_ctx.source_cache);
        let families = &font_styles.font_family.families;
        let fallback = to_parley::fallback_font_family(families);
        query.set_families(
            families
                .iter()
                .filter(|family| !to_parley::is_hidden_family(family))
                .map(to_parley::query_font_family)
                .chain(fallback.map(QueryFamily::Generic)),
        );
        query.set_attributes(Attributes {
            width: to_parley::font_width(font_styles.font_width),
            weight: to_parley::font_weight(font_styles.font_weight),
            style: to_parley::font_style(font_styles.font_style),
        });
        let mut found = None;
        query.matches_with(|q_font: &QueryFont| {
            if !q_font.unicode_range.contains(' '.into()) {
                return QueryStatus::Continue;
            }
            let Ok(font_ref) = skrifa::FontRef::from_index(q_font.blob.as_ref(), q_font.index) else {
                return QueryStatus::Continue;
            };
            if font_ref.charmap().map(' ').is_none() {
                return QueryStatus::Continue;
            }
            found = Some(q_font.clone());
            QueryStatus::Stop
        });
        found.map(|font| MathFace { font, size })
    }

    pub(crate) fn font_ref(&self) -> Option<skrifa::FontRef<'_>> {
        skrifa::FontRef::from_index(self.font.blob.as_ref(), self.font.index).ok()
    }

    /// CSS px per font unit.
    pub(crate) fn scale(&self, font_ref: &skrifa::FontRef<'_>) -> Option<f32> {
        use skrifa::instance::{LocationRef, Size};
        let upem = skrifa::metrics::Metrics::new(font_ref, Size::unscaled(), LocationRef::default()).units_per_em;
        (upem > 0).then(|| self.size / f32::from(upem))
    }

    /// The glyph of `ch`.
    pub(crate) fn glyph(&self, font_ref: &skrifa::FontRef<'_>, ch: char) -> Option<u16> {
        use skrifa::MetadataProvider as _;
        font_ref.charmap().map(ch).map(|glyph| glyph.to_u32() as u16)
    }

    /// The italic correction of the glyph of `ch` from the MATH table, CSS px.
    pub(crate) fn italic_correction(&self, ch: char) -> Option<f32> {
        let font_ref = self.font_ref()?;
        let scale = self.scale(&font_ref)?;
        let glyph = self.glyph(&font_ref, ch)?;
        let math = font_ref.table_data(skrifa::Tag::new(b"MATH"))?;
        super::types::italic_correction(math.as_bytes(), glyph).map(|value| f32::from(value) * scale)
    }

    /// The top accent attachment of the glyph of `ch` from the MATH table, CSS px.
    pub(crate) fn top_accent_attachment(&self, ch: char) -> Option<f32> {
        let font_ref = self.font_ref()?;
        let scale = self.scale(&font_ref)?;
        let glyph = self.glyph(&font_ref, ch)?;
        let math = font_ref.table_data(skrifa::Tag::new(b"MATH"))?;
        super::types::top_accent_attachment(math.as_bytes(), glyph).map(|value| f32::from(value) * scale)
    }

    /// The fallbacks' parts of the font (`Fallbacks`).
    pub(crate) fn fallbacks(&self) -> Fallbacks {
        use skrifa::instance::{LocationRef, Size};
        use skrifa::raw::TableProvider as _;
        let Some(font_ref) = self.font_ref() else { return Fallbacks::default() };
        let rule_thickness = skrifa::metrics::Metrics::new(&font_ref, Size::new(self.size), LocationRef::default())
            .underline
            .map_or(0.0, |underline| underline.thickness.max(0.0));
        let scale = self.scale(&font_ref).unwrap_or(0.0);
        let (subscript_shift_down, superscript_shift_up) = match font_ref.os2() {
            Ok(os2) => (f32::from(os2.y_subscript_y_offset()) * scale, f32::from(os2.y_superscript_y_offset()) * scale),
            Err(_) => (0.0, 0.0),
        };
        Fallbacks { rule_thickness, subscript_shift_down, superscript_shift_up }
    }
}

impl Fallbacks {
    /// The fallbacks of the box `node_id`: nothing is read for a font with a MATH table (its
    /// constants are used as they are).
    pub(crate) fn load(state: &LayoutPassState<'_>, node_id: NodeId, ctx: &MathContext) -> Fallbacks {
        if ctx.font.constants.is_some() {
            return Fallbacks::default();
        }
        MathFace::of(state, node_id).map(|face| face.fallbacks()).unwrap_or_default()
    }
}

/// The value of a MATH constant in CSS px: the font's, or (a font without a MATH table) the
/// fallback of MathML Core § 5.1 (a percentage constant is the table's percentage as it is).
pub(crate) fn math_constant(ctx: &MathContext, fallbacks: &Fallbacks, constant: MathConstant) -> f32 {
    use MathConstant::*;
    if let Some(value) = ctx.font.constant(constant) {
        return value;
    }
    let x_height = ctx.font.x_height.unwrap_or(ctx.font_size * 0.5);
    let rule = fallbacks.rule_thickness;
    match constant {
        AxisHeight => x_height / 2.0,
        AccentBaseHeight => x_height,
        SubscriptShiftDown => fallbacks.subscript_shift_down,
        SuperscriptShiftUp => fallbacks.superscript_shift_up,
        SubscriptTopMax | SuperscriptBottomMaxWithSubscript => 0.8 * x_height,
        SuperscriptBottomMin => 0.25 * x_height,
        RadicalDisplayStyleVerticalGap => rule + 0.25 * x_height,
        FractionNumeratorGapMin | FractionDenominatorGapMin | FractionRuleThickness | OverbarExtraAscender | UnderbarExtraDescender
        | OverbarRuleThickness | UnderbarRuleThickness | RadicalRuleThickness | RadicalExtraAscender => rule,
        StackGapMin | FractionNumDisplayStyleGapMin | FractionDenomDisplayStyleGapMin | OverbarVerticalGap | UnderbarVerticalGap => 3.0 * rule,
        SubSuperscriptGapMin => 4.0 * rule,
        StackDisplayStyleGapMin => 7.0 * rule,
        RadicalVerticalGap => 1.25 * rule,
        SpaceAfterScript => ctx.font_size / 24.0,
        RadicalKernBeforeDegree => ctx.font_size * 5.0 / 18.0,
        RadicalKernAfterDegree => -ctx.font_size * 10.0 / 18.0,
        RadicalDegreeBottomRaisePercent => 60.0,
        ScriptPercentScaleDown => 71.0,
        ScriptScriptPercentScaleDown => 50.41,
        _ => 0.0,
    }
}

// ---------------------------------------------------------------------------------------------
// Operators.

/// The properties of an operator that the limits and scripts care about (MathML Core § 3.2.4):
/// the dictionary's, overridden by the `largeop`, `movablelimits` and `stretchy` attributes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct OperatorProperties {
    pub(crate) largeop: bool,
    pub(crate) movablelimits: bool,
    pub(crate) stretchy: bool,
    /// The dictionary stretches it along the inline axis (arrows, braces, bars).
    pub(crate) horizontal: bool,
}

impl OperatorProperties {
    /// A `stretchy` operator of the inline axis.
    pub(crate) fn stretches_inline(&self) -> bool {
        self.stretchy && self.horizontal
    }
}

/// The MathML elements that are an embellished operator through their first in-flow child
/// (`scripts`, `fraction` and the like), and the ones through their only child that is not
/// space-like (rows). (`mathml_operator` has the same rules for spacing; its helpers are private.)
fn first_child_embellishes(name: &str) -> bool {
    matches!(name, "msub" | "msup" | "msubsup" | "mmultiscripts" | "munder" | "mover" | "munderover" | "mfrac" | "semantics" | "maction")
}

fn row_embellishes(name: &str) -> bool {
    matches!(name, "mrow" | "mstyle" | "mpadded" | "mphantom" | "merror" | "menclose" | "a")
}

/// The in-flow element children of `node_id` (`display: none` and out-of-flow ones left out).
fn in_flow_children(doc: &BaseDocument, node_id: NodeId) -> Vec<NodeId> {
    doc.nodes[node_id]
        .children
        .iter()
        .copied()
        .filter(|&child| {
            let node = &doc.nodes[child];
            node.element_data().is_some() && !node.display_style().is_some_and(|display| display.is_none()) && !node.is_out_of_flow()
        })
        .collect()
}

/// The `<mo>` that is the core of the embellished operator `node_id` (MathML Core § 3.2.4.3),
/// if it is one.
pub(crate) fn core_operator(doc: &BaseDocument, node_id: NodeId) -> Option<NodeId> {
    let mut id = node_id;
    for _ in 0..64 {
        let element = doc.nodes[id].element_data()?;
        if element.name.ns != ns!(mathml) {
            return None;
        }
        let name: &str = &element.name.local;
        if name == "mo" {
            return Some(id);
        }
        let children = in_flow_children(doc, id);
        if first_child_embellishes(name) {
            id = *children.first()?;
        } else if row_embellishes(name) {
            let mut core = None;
            for child in children {
                let space_like = doc.nodes[child]
                    .element_data()
                    .is_some_and(|e| e.name.ns == ns!(mathml) && matches!(&*e.name.local, "mtext" | "mspace"));
                if space_like {
                    continue;
                }
                if core.is_some() {
                    return None;
                }
                core = Some(child);
            }
            id = core?;
        } else {
            return None;
        }
    }
    None
}

/// The properties of the operator that is the core of the embellished operator `node_id`.
pub(crate) fn embellished_operator(doc: &BaseDocument, node_id: NodeId) -> Option<OperatorProperties> {
    let mo = core_operator(doc, node_id)?;
    let node = &doc.nodes[mo];
    let element = node.element_data()?;
    let text = node.text_content();
    let form = match element.attr(LocalName::from("form")).map(str::to_ascii_lowercase).as_deref() {
        Some("prefix") => Form::Prefix,
        Some("postfix") => Form::Postfix,
        _ => Form::Infix,
    };
    let entry = mathml_operator::entry(text.trim_matches(|c: char| c.is_ascii_whitespace()), form);
    let flag = |name: &str, default: bool| match element.attr(LocalName::from(name)).map(str::to_ascii_lowercase).as_deref() {
        Some("true") => true,
        Some("false") => false,
        _ => default,
    };
    Some(OperatorProperties {
        largeop: flag("largeop", entry.largeop()),
        movablelimits: flag("movablelimits", entry.movablelimits()),
        stretchy: flag("stretchy", entry.stretchy()),
        horizontal: entry.horizontal(),
    })
}

/// The text of a token (`mi`, `mn`, `mo`, `mtext`, `ms`) child, white space trimmed, for the glyph
/// attributes of its last (or only) character; `None` for any other box.
fn token_text(doc: &BaseDocument, id: NodeId) -> Option<String> {
    let node = &doc.nodes[id];
    let element = node.element_data()?;
    if element.name.ns != ns!(mathml) || !matches!(&*element.name.local, "mi" | "mn" | "mo" | "mtext" | "ms") {
        return None;
    }
    Some(node.text_content().trim_matches(|c: char| c.is_ascii_whitespace()).to_string())
}

/// The italic correction of a child box (CSS px): the one its class reported, else, for a token,
/// the MATH table's for its last character's glyph, 0 for any other box.
pub(crate) fn child_italic_correction(state: &LayoutPassState<'_>, child: &ChildMetrics) -> f32 {
    if child.italic_correction != 0.0 {
        return child.italic_correction;
    }
    let Some(text) = token_text(state, child.id) else { return 0.0 };
    let Some(last) = text.chars().next_back() else { return 0.0 };
    MathFace::of(state, child.id).and_then(|face| face.italic_correction(last)).unwrap_or(0.0)
}

/// The inline position (CSS px, from the start of the margin box) the top accent of a child box
/// attaches at: the MATH table's for a one-character token, else the middle of the margin box.
fn top_accent_attachment(state: &LayoutPassState<'_>, child: &ChildMetrics) -> f32 {
    let middle = child.margin_box_width() / 2.0;
    let Some(text) = token_text(state, child.id) else { return middle };
    let mut chars = text.chars();
    let (Some(only), None) = (chars.next(), chars.next()) else { return middle };
    MathFace::of(state, child.id)
        .and_then(|face| face.top_accent_attachment(only))
        .map_or(middle, |attachment| child.margin.left + attachment)
}

fn boolean_attribute(state: &LayoutPassState<'_>, node_id: NodeId, name: &str) -> bool {
    state.nodes[node_id]
        .element_data()
        .and_then(|element| element.attr(LocalName::from(name)))
        .is_some_and(|value| value.eq_ignore_ascii_case("true"))
}

// ---------------------------------------------------------------------------------------------
// Layout.

/// Layout of a box `construct` built (see the module's documentation).
pub(crate) fn layout(state: &mut LayoutPassState<'_>, node_id: NodeId, kind: MathKind, ctx: &MathContext, children: &[ChildMetrics]) -> MathLayout {
    let expected = if kind == MathKind::UnderOver { 3 } else { 2 };
    if children.len() != expected {
        // (An out-of-flow child took the place of one: the markup is invalid.)
        return row::layout_row(ctx, children);
    }
    let base_properties = embellished_operator(state, children[0].id).unwrap_or_default();

    // § 3.4.2.1: in compact style, an operator with `movablelimits` makes limits scripts.
    if !ctx.display_style && base_properties.movablelimits {
        let as_scripts = match kind {
            MathKind::Under => MathKind::Sub,
            MathKind::Over => MathKind::Sup,
            _ => MathKind::SubSup,
        };
        return scripts::layout(state, node_id, as_scripts, ctx, children);
    }

    // § 3.4.2.2: the horizontally stretchy children take the width of the widest other child.
    let mut metrics: Vec<ChildMetrics> = children.to_vec();
    let stretchy: Vec<bool> = children
        .iter()
        .map(|child| embellished_operator(state, child.id).is_some_and(|properties| properties.stretches_inline()))
        .collect();
    if stretchy.iter().any(|&stretch| stretch) {
        let target = children
            .iter()
            .zip(&stretchy)
            .filter(|(_, stretch)| !**stretch)
            .fold(0.0f32, |widest, (child, _)| widest.max(child.margin_box_width()));
        for (slot, _) in metrics.iter_mut().zip(&stretchy).filter(|(_, stretch)| **stretch) {
            if target > slot.margin_box_width() {
                let width = (target - slot.margin.left - slot.margin.right).max(0.0);
                *slot = state.measure_math_child(slot.id, Size { width: Some(width), height: None });
            }
        }
    }

    let base = metrics[0];
    let (under, over) = match kind {
        MathKind::Under => (Some(metrics[1]), None),
        MathKind::Over => (None, Some(metrics[1])),
        _ => (Some(metrics[1]), Some(metrics[2])),
    };
    let fallbacks = Fallbacks::load(state, node_id, ctx);
    let constant = |c: MathConstant| math_constant(ctx, &fallbacks, c);
    let accent = boolean_attribute(state, node_id, "accent");
    let accent_under = boolean_attribute(state, node_id, "accentunder");
    let large_op_italic = if base_properties.largeop { child_italic_correction(state, &base) } else { 0.0 };

    let base_width = base.margin_box_width();
    // The horizontal extent in coordinates centred on the base (§ 3.4.2.3 to § 3.4.2.5).
    let mut max = base_width / 2.0;
    let mut min = -base_width / 2.0;
    let mut over_attachment = 0.0;
    if let Some(under) = &under {
        let width = under.margin_box_width();
        max = max.max(width / 2.0 - large_op_italic / 2.0);
        min = min.min(-width / 2.0 - large_op_italic / 2.0);
    }
    if let Some(over) = &over {
        over_attachment = top_accent_attachment(state, over);
        max = max.max(over.margin_box_width() - over_attachment + large_op_italic / 2.0);
        min = min.min(-over_attachment + large_op_italic / 2.0);
    }

    // The vertical placement: the baseline of the underscript below the base's by `under_offset`
    // and of the overscript above it by `over_offset`, and the ascent and descent of the box.
    let mut ascent = base.ascent;
    let mut descent = base.descent;
    let mut under_offset = 0.0;
    let mut over_offset = 0.0;
    if let Some(under) = &under {
        let (shift, extra_descender) = if base_properties.largeop {
            (constant(MathConstant::LowerLimitBaselineDropMin).max(constant(MathConstant::LowerLimitGapMin) + under.ascent), 0.0)
        } else if base_properties.stretches_inline() {
            (constant(MathConstant::StretchStackBottomShiftDown).max(constant(MathConstant::StretchStackGapAboveMin) + under.ascent), 0.0)
        } else {
            let gap = if accent_under { 0.0 } else { constant(MathConstant::UnderbarVerticalGap) };
            (gap + under.ascent, constant(MathConstant::UnderbarExtraDescender))
        };
        under_offset = base.descent + shift;
        ascent = ascent.max(under.ascent - under_offset);
        descent = descent.max(under.descent + under_offset + extra_descender);
    }
    if let Some(over) = &over {
        let (shift, extra_ascender) = if base_properties.largeop {
            (constant(MathConstant::UpperLimitBaselineRiseMin).max(constant(MathConstant::UpperLimitGapMin) + over.descent), 0.0)
        } else if base_properties.stretches_inline() {
            (constant(MathConstant::StretchStackTopShiftUp).max(constant(MathConstant::StretchStackGapBelowMin) + over.descent), 0.0)
        } else {
            let gap = if !accent {
                constant(MathConstant::OverbarVerticalGap)
            } else {
                (constant(MathConstant::AccentBaseHeight) - base.ascent).max(0.0)
            };
            (gap + over.descent, constant(MathConstant::OverbarExtraAscender))
        };
        over_offset = base.ascent + shift;
        ascent = ascent.max(over.ascent + over_offset + extra_ascender);
        descent = descent.max(over.descent - over_offset);
    }

    let mut placements = Vec::with_capacity(3);
    placements.push(ChildPlacement { metrics: base, inline_offset: -min - base_width / 2.0, block_offset: ascent - base.ascent });
    if let Some(under) = under {
        placements.push(ChildPlacement {
            metrics: under,
            inline_offset: -min - under.margin_box_width() / 2.0 - large_op_italic / 2.0,
            block_offset: ascent + under_offset - under.ascent,
        });
    }
    if let Some(over) = over {
        placements.push(ChildPlacement {
            metrics: over,
            inline_offset: -min - over_attachment + large_op_italic / 2.0,
            block_offset: ascent - over_offset - over.ascent,
        });
    }
    MathLayout { content_width: max - min, ascent, descent, children: placements, italic_correction: 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use taffy::Rect;

    fn context(constants: Option<[f32; super::super::MATH_CONSTANT_COUNT]>) -> MathContext {
        MathContext {
            font_size: 20.0,
            display_style: false,
            math_depth: 0,
            cramped: false,
            rtl: false,
            font: super::super::MathFontParameters { constants, x_height: Some(10.0) },
            padding: Rect::ZERO,
            border: Rect::ZERO,
            inset: Rect::ZERO,
        }
    }

    #[test]
    fn constants_fall_back_without_a_math_table() {
        let ctx = context(None);
        let fallbacks = Fallbacks { rule_thickness: 2.0, subscript_shift_down: 3.0, superscript_shift_up: 4.0 };
        let get = |c| math_constant(&ctx, &fallbacks, c);
        assert_eq!(get(MathConstant::AxisHeight), 5.0);
        assert_eq!(get(MathConstant::AccentBaseHeight), 10.0);
        assert_eq!(get(MathConstant::OverbarVerticalGap), 6.0);
        assert_eq!(get(MathConstant::SubSuperscriptGapMin), 8.0);
        assert_eq!(get(MathConstant::RadicalVerticalGap), 2.5);
        assert_eq!(get(MathConstant::RadicalDisplayStyleVerticalGap), 4.5);
        assert_eq!(get(MathConstant::SubscriptShiftDown), 3.0);
        assert_eq!(get(MathConstant::SuperscriptShiftUp), 4.0);
        assert_eq!(get(MathConstant::UpperLimitGapMin), 0.0);
        assert!((get(MathConstant::SpaceAfterScript) - 20.0 / 24.0).abs() < 1e-6);
        assert_eq!(get(MathConstant::RadicalDegreeBottomRaisePercent), 60.0);
    }

    #[test]
    fn constants_come_from_the_table_when_there_is_one() {
        let mut values = [0.0; super::super::MATH_CONSTANT_COUNT];
        values[MathConstant::UpperLimitGapMin as usize] = 7.0;
        let ctx = context(Some(values));
        let fallbacks = Fallbacks { rule_thickness: 2.0, ..Fallbacks::default() };
        assert_eq!(math_constant(&ctx, &fallbacks, MathConstant::UpperLimitGapMin), 7.0);
        // A constant the table has as 0 is 0: no fallback.
        assert_eq!(math_constant(&ctx, &fallbacks, MathConstant::OverbarVerticalGap), 0.0);
    }
}
