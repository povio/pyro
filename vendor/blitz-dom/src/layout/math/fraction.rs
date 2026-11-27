//! pyro: MathML fractions (`<mfrac>`, MathML Core § 3.3.2), the ordinary kind only: an `mfrac`
//! of `display: inline` (its default) with exactly two element children, each in flow, holding
//! no block box, with no `::before` or `::after` and no text of its own beside them. Its two
//! children are its layout children, stacked: the first (the numerator) above the fraction bar,
//! the second (the denominator) below it, each centred in the fraction's content box, the bar on
//! the math axis (Chromium's `MathMLFractionLayoutAlgorithm`). Any other `mfrac` is laid out as
//! it was, as an atomic inline (`replaced::is_inline_box`) holding its children's inline layout.
//!
//! The parameters (gaps, shifts, the bar's default thickness, the axis height) are the primary
//! font's OpenType MATH constants, or Chromium's fallbacks for a font without a MATH table: the
//! bar 0.05em, the gaps one bar (three in display style), the shifts zero, the axis half the
//! x-height. Not done: the `numalign`/`denomalign` and `bevelled` legacy attributes, `math-shift`,
//! vertical writing modes, right-to-left math, and stretchy operators or spacing in the children.
use markup5ever::{local_name, ns};
use style::values::specified::box_::{DisplayInside, DisplayOutside};
use taffy::{
    AxisStaticEdge, AxisStaticPosition, CoreStyle as _, LayoutInput, LayoutOutput, LayoutPartialTree as _, OofCandidate,
    OofPositioningArea, Point, ResolveOrZero as _, RunMode, Size,
};

use super::{MathConstant, MathFontParameters};
use crate::layout::{LayoutPassState, resolve_calc_value};
use crate::node::{NodeData, NodeFlags};
use crate::{BaseDocument, Node, NodeId, dom_node_id, taffy_node_id};

/// Whether `node` is an ordinary MathML fraction (see the module's documentation), from its
/// style, children and pseudo-elements as they are now.
pub(crate) fn is_ordinary_fraction(doc: &BaseDocument, node: &Node) -> bool {
    let Some(element) = node.element_data() else { return false };
    if element.name.ns != ns!(mathml) || element.name.local != local_name!("mfrac") {
        return false;
    }
    let Some(display) = node.display_style() else { return false };
    if display.outside() != DisplayOutside::Inline || display.inside() != DisplayInside::Flow {
        return false;
    }
    if node.before().is_some() || node.after().is_some() {
        return false;
    }
    fraction_children(doc, node).is_some()
}

/// The numerator and denominator of the fraction `node`: its two in-flow element children, when it
/// has exactly two, each holding no block box, and no other content (white space and comments
/// aside); and its out-of-flow children (positioned by the fraction's content box, MathML Core
/// § 3.1: they do not take part in its layout, as the `display: none` ones that are left out).
pub(crate) fn fraction_children(doc: &BaseDocument, node: &Node) -> Option<([NodeId; 2], Vec<NodeId>)> {
    let mut found = [NodeId::default(); 2];
    let mut out_of_flow = Vec::new();
    let mut count = 0;
    for &child_id in node.children.iter() {
        let child = &doc.nodes[child_id];
        match &child.data {
            NodeData::Comment { .. } => {}
            NodeData::Text(text) => {
                if !text.content.chars().all(|c| matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{c}')) {
                    return None;
                }
            }
            NodeData::Element(_) => {
                let display = child.display_style()?;
                if display.is_none() {
                    continue;
                }
                if display.is_contents() || child.is_or_contains_block() {
                    return None;
                }
                if child.primary_styles().is_none_or(|style| style.get_box().position.is_absolutely_positioned()) {
                    out_of_flow.push(child_id);
                    continue;
                }
                if count == 2 {
                    return None;
                }
                found[count] = child_id;
                count += 1;
            }
            _ => return None,
        }
    }
    (count == 2).then_some((found, out_of_flow))
}

/// The `linethickness` attribute of a fraction (MathML Core § 3.3.2.1): a `<length-percentage>`,
/// a percentage of the default thickness; `None` when it is absent or not one (negative, or with
/// a unit CSS has not), when the default is used. `em` and `ex` are of the fraction's font.
pub(crate) fn parse_line_thickness(value: &str, default: f32, font_size: f32, x_height: f32) -> Option<f32> {
    let value = value.trim_matches(|c: char| c.is_ascii_whitespace());
    // The number: a sign, digits with a decimal point, and an exponent when digits follow its
    // `e` (`1e2px`; the `e` of `em` and `ex` is the unit's).
    let bytes = value.as_bytes();
    let digits_from = |mut at: usize| {
        while at < bytes.len() && bytes[at].is_ascii_digit() {
            at += 1;
        }
        at
    };
    let mut split = if matches!(bytes.first(), Some(b'+' | b'-')) { 1 } else { 0 };
    split = digits_from(split);
    if bytes.get(split) == Some(&b'.') {
        split = digits_from(split + 1);
    }
    if matches!(bytes.get(split), Some(b'e' | b'E')) {
        let sign = usize::from(matches!(bytes.get(split + 1), Some(b'+' | b'-')));
        if bytes.get(split + 1 + sign).is_some_and(u8::is_ascii_digit) {
            split = digits_from(split + 1 + sign);
        }
    }
    let (number, unit) = value.split_at(split);
    let number: f32 = number.parse().ok()?;
    if !number.is_finite() || number < 0.0 {
        return None;
    }
    let unit = unit.to_ascii_lowercase();
    let px = match unit.as_str() {
        // Only zero is a length without a unit.
        "" if number == 0.0 => 0.0,
        "%" => number / 100.0 * default,
        "px" => number,
        "em" => number * font_size,
        "ex" => number * x_height,
        "in" => number * 96.0,
        "cm" => number * 96.0 / 2.54,
        "mm" => number * 96.0 / 25.4,
        "q" => number * 96.0 / 101.6,
        "pt" => number * 96.0 / 72.0,
        "pc" => number * 16.0,
        _ => return None,
    };
    Some(px)
}

/// What the fraction's parameters come to for a fraction (CSS px).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FractionParameters {
    pub(crate) axis_height: f32,
    pub(crate) thickness: f32,
    /// With a bar: the minimum gaps between it and the numerator and denominator, and the
    /// minimum shifts of their baselines up and down from the fraction's baseline.
    pub(crate) numerator_gap_min: f32,
    pub(crate) denominator_gap_min: f32,
    pub(crate) numerator_shift_up: f32,
    pub(crate) denominator_shift_down: f32,
    /// Without one (`linethickness` 0): the minimum gap between them, and their shifts.
    pub(crate) stack_gap_min: f32,
    pub(crate) stack_top_shift_up: f32,
    pub(crate) stack_bottom_shift_down: f32,
}

impl FractionParameters {
    /// Chromium's `GetFractionParameters` and `GetFractionStackParameters`.
    pub(crate) fn new(font: &MathFontParameters, font_size: f32, display_style: bool, line_thickness: Option<&str>) -> Self {
        use MathConstant::*;
        let rule_fallback = 0.05 * font_size;
        let x_height = font.x_height.unwrap_or(font_size * 0.5);
        let axis_height = font.constant(AxisHeight).unwrap_or(x_height / 2.0);
        let default_thickness = font.constant(FractionRuleThickness).unwrap_or(rule_fallback);
        // A negative value is zero (Chromium, WPT `frac-linethickness-002`: "Negative"); any other
        // invalid one is the default.
        let thickness = line_thickness
            .and_then(|value| {
                let value = value.trim_matches(|c: char| c.is_ascii_whitespace());
                match value.strip_prefix('-') {
                    Some(magnitude) => parse_line_thickness(magnitude, default_thickness, font_size, x_height).map(|_| 0.0),
                    None => parse_line_thickness(value, default_thickness, font_size, x_height),
                }
            })
            .unwrap_or(default_thickness)
            .max(0.0);
        let pick = |display: MathConstant, inline: MathConstant| font.constant(if display_style { display } else { inline });
        let gap_fallback = if display_style { 3.0 } else { 1.0 } * rule_fallback;
        FractionParameters {
            axis_height,
            thickness,
            numerator_gap_min: pick(FractionNumDisplayStyleGapMin, FractionNumeratorGapMin).unwrap_or(gap_fallback),
            denominator_gap_min: pick(FractionDenomDisplayStyleGapMin, FractionDenominatorGapMin).unwrap_or(gap_fallback),
            numerator_shift_up: pick(FractionNumeratorDisplayStyleShiftUp, FractionNumeratorShiftUp).unwrap_or(0.0),
            denominator_shift_down: pick(FractionDenominatorDisplayStyleShiftDown, FractionDenominatorShiftDown)
                .unwrap_or(0.0),
            stack_gap_min: pick(StackDisplayStyleGapMin, StackGapMin)
                .unwrap_or(if display_style { 7.0 } else { 3.0 } * rule_fallback),
            stack_top_shift_up: pick(StackTopDisplayStyleShiftUp, StackTopShiftUp).unwrap_or(0.0),
            stack_bottom_shift_down: pick(StackBottomDisplayStyleShiftDown, StackBottomShiftDown).unwrap_or(0.0),
        }
    }

    /// The shifts of the numerator's baseline up from the fraction's baseline and of the
    /// denominator's down, given each one's ascent and descent (from its baseline to its top and
    /// bottom margin edges).
    pub(crate) fn shifts(&self, numerator: (f32, f32), denominator: (f32, f32)) -> (f32, f32) {
        let (_, num_descent) = numerator;
        let (den_ascent, _) = denominator;
        if self.thickness > 0.0 {
            let up = self
                .numerator_shift_up
                .max(self.axis_height + self.thickness / 2.0 + self.numerator_gap_min + num_descent);
            let down = self
                .denominator_shift_down
                .max(self.thickness / 2.0 + self.denominator_gap_min + den_ascent - self.axis_height);
            (up, down)
        } else {
            let (mut up, mut down) = (self.stack_top_shift_up, self.stack_bottom_shift_down);
            let gap = up - num_descent + down - den_ascent;
            if gap < self.stack_gap_min {
                let delta = (self.stack_gap_min - gap) / 2.0;
                up += delta;
                down += delta;
            }
            (up, down)
        }
    }
}

impl LayoutPassState<'_> {
    /// The parameters the fraction `node_id` is laid out with.
    fn fraction_parameters(&self, node_id: NodeId) -> Option<FractionParameters> {
        let node = &self.nodes[node_id];
        let style = node.primary_styles()?;
        let font_size = style.slow_clone_font_size().used_size().px();
        let display_style = style.get_font().math_style == style::values::computed::MathStyle::Normal;
        let provider = crate::font_metrics::BlitzFontMetricsProvider { font_ctx: self.font_ctx.clone() };
        let font = provider.math_parameters_for(style.get_font(), font_size);
        let line_thickness = node.element_data().and_then(|el| el.attr(local_name!("linethickness")));
        Some(FractionParameters::new(&font, font_size, display_style, line_thickness))
    }

    /// Lays out the ordinary fraction `node_id` (Chromium's `MathMLFractionLayoutAlgorithm`):
    /// its children at their max-content widths, the numerator's baseline raised and the
    /// denominator's lowered from the fraction's by `FractionParameters::shifts`, its baseline its
    /// first and last. The box takes its `width`, `height`, `min-*`, `max-*` from the style
    /// around the content; out-of-flow children are positioned from the content box's start.
    /// Records the bar (`ElementData::math_fraction_bar`) for paint.
    #[inline(never)]
    pub(crate) fn compute_math_fraction_layout(&mut self, node_id: taffy::NodeId, inputs: LayoutInput) -> LayoutOutput {
        let dom_id = dom_node_id(node_id);
        let listed: Vec<NodeId> = self.nodes[dom_id].layout_children.borrow().as_ref().map(|c| c.to_vec()).unwrap_or_default();
        let Some(params) = self.fraction_parameters(dom_id) else { return LayoutOutput::HIDDEN };
        // The numerator and denominator, then the out-of-flow children (`construct_fraction`).
        let [numerator, denominator, out_of_flow @ ..] = &listed[..] else { return LayoutOutput::HIDDEN };
        let (numerator, denominator) = (*numerator, *denominator);
        let (padding, border) = {
            let style = self.nodes[dom_id].layout_style();
            let parent_width = inputs.parent_size.width;
            (
                style.padding().resolve_or_zero(parent_width, resolve_calc_value),
                style.border().resolve_or_zero(parent_width, resolve_calc_value),
            )
        };
        let inset = padding + border;

        // Each child at its max-content size, as an independent formatting context (an `mspace`'s
        // depth lowers its baseline: `measure_math_child`).
        let num = self.measure_math_child(numerator, Size::NONE);
        let den = self.measure_math_child(denominator, Size::NONE);
        let (up, down) = params.shifts((num.ascent, num.descent), (den.ascent, den.descent));
        let ascent = (up + num.ascent).max(den.ascent - down);
        let descent = (num.descent - up).max(down + den.descent);
        let content_width = num.margin_box_width().max(den.margin_box_width());

        // The box: `width`, `height`, `min-*`, `max-*` and `box-sizing` resolved against the content.
        let mut output = {
            let style = self.nodes[dom_id].layout_style();
            let content = Size { width: content_width, height: ascent + descent };
            taffy::compute_leaf_layout(inputs, &style, resolve_calc_value, |_, _| content)
        };
        let baseline = inset.top + ascent;
        output.baselines = taffy::Baselines::new(Some(baseline), Some(baseline));
        if inputs.run_mode != RunMode::PerformLayout {
            return output;
        }

        let place = [(numerator, num, baseline - up - num.ascent), (denominator, den, baseline + down - den.ascent)];
        for (order, (child, metrics, top)) in place.into_iter().enumerate() {
            let (child_padding, child_border) = {
                let style = self.nodes[child].layout_style();
                (
                    style.padding().resolve_or_zero(None, resolve_calc_value),
                    style.border().resolve_or_zero(None, resolve_calc_value),
                )
            };
            let layout = taffy::Layout {
                order: order as u32,
                location: Point {
                    x: inset.left + (content_width - metrics.margin_box_width()) / 2.0 + metrics.margin.left,
                    y: top + metrics.margin.top,
                },
                size: metrics.size,
                scrollable_overflow_rect: metrics.overflow,
                scrollbar_size: Size::ZERO,
                border: child_border,
                padding: child_padding,
                margin: metrics.margin,
            };
            self.set_unrounded_layout(taffy_node_id(child), &layout);
        }
        // The bar, centred on the math axis (Chromium's `MathMLPainter::PaintFractionBar`).
        let bar = (params.thickness > 0.0).then(|| [baseline - params.axis_height - params.thickness / 2.0, params.thickness]);
        if let Some(element) = self.nodes[dom_id].element_data_mut() {
            element.math_fraction_bar = bar;
        }

        // Out-of-flow children are positioned by their containing block: their static position is
        // the content box's start.
        if !out_of_flow.is_empty() {
            let static_position = Point { x: inset.left, y: inset.top }.map(|at| AxisStaticPosition::from_edge(at, AxisStaticEdge::Start));
            for (index, &child) in out_of_flow.iter().enumerate() {
                output.oof_candidates.push(OofCandidate {
                    containing_block: None,
                    node: taffy_node_id(child),
                    order: (2 + index) as u32,
                    position: self.nodes[child].taffy_position(),
                    static_position,
                });
            }
            output.oof_positioning_area = Some(OofPositioningArea {
                size: output.size - border.sum_axes(),
                offset: Point { x: border.left, y: border.top },
            });
        }
        output
    }
}

/// Construction of an ordinary fraction (`construct::collect_layout_children_with_wrap`): its
/// numerator and denominator are its layout children. Returns whether `node_id` is one (else
/// the caller builds it as any other box). A fraction built again has no bar until it is laid
/// out.
pub(crate) fn construct_fraction(doc: &mut BaseDocument, node_id: NodeId, children: &mut thin_vec::ThinVec<NodeId>) -> bool {
    let Some(element) = doc.nodes[node_id].element_data_mut() else { return false };
    if element.name.ns != ns!(mathml) || element.name.local != local_name!("mfrac") {
        return false;
    }
    element.math_fraction_bar = None;
    let node = &doc.nodes[node_id];
    if !is_ordinary_fraction(doc, node) {
        return false;
    }
    let (pair, out_of_flow) = fraction_children(doc, node).unwrap();
    doc.nodes[node_id].flags.insert(NodeFlags::IS_MATH_FRACTION);
    children.extend(pair);
    children.extend(out_of_flow);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_thickness_values() {
        let parse = |v: &str| parse_line_thickness(v, 2.0, 20.0, 10.0);
        assert_eq!(parse("0"), Some(0.0));
        assert_eq!(parse("0px"), Some(0.0));
        assert_eq!(parse("200%"), Some(4.0));
        assert_eq!(parse(" 3px "), Some(3.0));
        assert_eq!(parse("0.5em"), Some(10.0));
        assert_eq!(parse("1ex"), Some(10.0));
        assert_eq!(parse("1e1px"), Some(10.0));
        assert_eq!(parse("12pt"), Some(16.0));
        assert_eq!(parse("-1px"), None);
        assert_eq!(parse("3"), None);
        assert_eq!(parse("thick"), None);
        assert_eq!(parse("medium"), None);
        assert_eq!(parse(""), None);
    }

    #[test]
    fn shifts_without_a_math_table() {
        // A 20px font, x-height 10: the axis at 5, the bar 1px, the gaps 1px (inline style).
        let font = MathFontParameters { constants: None, x_height: Some(10.0) };
        let p = FractionParameters::new(&font, 20.0, false, None);
        assert_eq!((p.axis_height, p.thickness, p.numerator_gap_min), (5.0, 1.0, 1.0));
        // Numerator descent 4: its baseline 5 + 0.5 + 1 + 4 up; denominator ascent 15: 0.5 + 1 + 15 - 5 down.
        assert_eq!(p.shifts((15.0, 4.0), (15.0, 4.0)), (10.5, 11.5));
        // A negative thickness is zero, any other invalid one the default.
        assert_eq!(FractionParameters::new(&font, 20.0, false, Some("-1.5em")).thickness, 0.0);
        assert_eq!(FractionParameters::new(&font, 20.0, false, Some("thick")).thickness, 1.0);
        // Display style: three bars of gap.
        let p = FractionParameters::new(&font, 20.0, true, None);
        assert_eq!(p.numerator_gap_min, 3.0);
        // No bar: the stack gap (3 bars inline) split evenly.
        let p = FractionParameters::new(&font, 20.0, false, Some("0"));
        assert_eq!(p.thickness, 0.0);
        let (up, down) = p.shifts((15.0, 4.0), (15.0, 4.0));
        assert_eq!((up, down), (11.0, 11.0));
    }
}
