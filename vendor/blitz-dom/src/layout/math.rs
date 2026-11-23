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
    AvailableSpace, CoreStyle as _, LayoutInput, LayoutOutput, LayoutPartialTree as _, Point, Rect, RequestedAxis,
    ResolveOrZero as _, RunMode, ScrollableOverflow, Size, SizingMode,
};

use super::{LayoutPassState, resolve_calc_value};
use crate::node::{NodeData, NodeFlags};
use crate::{BaseDocument, Node, NodeId, dom_node_id, taffy_node_id};

/// The MATH table's `MathConstants` records the fraction reads (OpenType § MATH, the index of
/// each `MathValueRecord` after the table's four leading 16-bit fields).
#[derive(Clone, Copy)]
pub(crate) enum MathConstant {
    AxisHeight = 1,
    StackTopShiftUp = 18,
    StackTopDisplayStyleShiftUp = 19,
    StackBottomShiftDown = 20,
    StackBottomDisplayStyleShiftDown = 21,
    StackGapMin = 22,
    StackDisplayStyleGapMin = 23,
    FractionNumeratorShiftUp = 28,
    FractionNumeratorDisplayStyleShiftUp = 29,
    FractionDenominatorShiftDown = 30,
    FractionDenominatorDisplayStyleShiftDown = 31,
    FractionNumeratorGapMin = 32,
    FractionNumDisplayStyleGapMin = 33,
    FractionRuleThickness = 34,
    FractionDenominatorGapMin = 35,
    FractionDenomDisplayStyleGapMin = 36,
}

/// The highest record index [`MathConstant`] names, plus one.
pub(crate) const MATH_CONSTANT_COUNT: usize = 37;

/// The math parameters of a font at a size (CSS px): its MATH constants, when it has a MATH
/// table, and its x-height.
#[derive(Clone, Debug, Default)]
pub(crate) struct MathFontParameters {
    pub(crate) constants: Option<[f32; MATH_CONSTANT_COUNT]>,
    pub(crate) x_height: Option<f32>,
}

impl MathFontParameters {
    pub(crate) fn constant(&self, constant: MathConstant) -> Option<f32> {
        self.constants.map(|values| values[constant as usize])
    }
}

/// Reads the `MathConstants` records of a MATH table (`data`), in font units.
pub(crate) fn read_math_constants(data: &[u8]) -> Option<[i16; MATH_CONSTANT_COUNT]> {
    let u16_at = |offset: usize| data.get(offset..offset + 2).map(|b| u16::from_be_bytes([b[0], b[1]]));
    // MATH header: majorVersion, minorVersion, mathConstantsOffset (from the table's start).
    if u16_at(0)? != 1 {
        return None;
    }
    let constants = u16_at(4)? as usize;
    if constants == 0 {
        return None;
    }
    let mut values = [0i16; MATH_CONSTANT_COUNT];
    for (index, value) in values.iter_mut().enumerate() {
        // Four 16-bit fields, then the 4-byte records (a value and a device table offset).
        *value = u16_at(constants + 8 + 4 * index)? as i16;
    }
    Some(values)
}

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

/// The numerator and denominator of the fraction `node`: its two element children, when it has
/// exactly two, each laid out in flow and holding no block box, and no other content (white
/// space and comments aside).
pub(crate) fn fraction_children(doc: &BaseDocument, node: &Node) -> Option<[NodeId; 2]> {
    let mut found = [NodeId::default(); 2];
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
                if display.is_none() || display.is_contents() || child.is_or_contains_block() {
                    return None;
                }
                let out_of_flow =
                    child.primary_styles().is_none_or(|style| style.get_box().position.is_absolutely_positioned());
                if out_of_flow || count == 2 {
                    return None;
                }
                found[count] = child_id;
                count += 1;
            }
            _ => return None,
        }
    }
    (count == 2).then_some(found)
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
        let thickness = line_thickness
            .and_then(|value| parse_line_thickness(value, default_thickness, font_size, x_height))
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
    /// first and last. Records the bar (`ElementData::math_fraction_bar`) for paint.
    #[inline(never)]
    pub(crate) fn compute_math_fraction_layout(&mut self, node_id: taffy::NodeId, inputs: LayoutInput) -> LayoutOutput {
        let dom_id = dom_node_id(node_id);
        let children: Vec<NodeId> = self.nodes[dom_id].layout_children.borrow().as_ref().map(|c| c.to_vec()).unwrap_or_default();
        let Some(params) = self.fraction_parameters(dom_id) else { return LayoutOutput::HIDDEN };
        let [numerator, denominator] = children[..] else { return LayoutOutput::HIDDEN };
        let (padding, border) = {
            let style = self.nodes[dom_id].layout_style();
            let parent_width = inputs.parent_size.width;
            (
                style.padding().resolve_or_zero(parent_width, resolve_calc_value),
                style.border().resolve_or_zero(parent_width, resolve_calc_value),
            )
        };
        let inset = padding + border;

        // Each child at its max-content size, as an independent formatting context.
        let child_inputs = LayoutInput {
            run_mode: RunMode::PerformLayout,
            sizing_mode: SizingMode::InherentSize,
            axis: RequestedAxis::Both,
            known_dimensions: Size::NONE,
            known_dimensions_are_definite: Size { width: false, height: false },
            parent_size: Size::NONE,
            available_space: Size { width: AvailableSpace::MaxContent, height: AvailableSpace::MaxContent },
            vertical_margins_are_collapsible: taffy::Line::FALSE,
        };
        let mut laid_out = [(Rect::<f32>::ZERO, Size::<f32>::ZERO, ScrollableOverflow::ZERO, 0.0f32, 0.0f32); 2];
        for (slot, child) in laid_out.iter_mut().zip([numerator, denominator]) {
            let margin = self.nodes[child].layout_style().margin().resolve_or_zero(None, resolve_calc_value);
            let output = self.compute_child_layout(taffy_node_id(child), child_inputs);
            // A child without a baseline has one at its bottom border edge (Chromium's
            // `FirstBaselineOrSynthesize`).
            let baseline = output.baselines.first.or(output.baselines.last).unwrap_or(output.size.height);
            let ascent = margin.top + baseline;
            let descent = output.size.height + margin.top + margin.bottom - ascent;
            *slot = (margin, output.size, output.scrollable_overflow_rect, ascent, descent);
        }
        let [(num_margin, num_size, num_overflow, num_ascent, num_descent), (den_margin, den_size, den_overflow, den_ascent, den_descent)] =
            laid_out;
        let (up, down) = params.shifts((num_ascent, num_descent), (den_ascent, den_descent));
        let ascent = (up + num_ascent).max(den_ascent - down) + inset.top;
        let descent = (num_descent - up).max(down + den_descent) + inset.bottom;
        let num_width = num_size.width + num_margin.left + num_margin.right;
        let den_width = den_size.width + den_margin.left + den_margin.right;
        let content_width = num_width.max(den_width);
        let size = Size {
            width: inputs.known_dimensions.width.unwrap_or(content_width + inset.left + inset.right),
            height: inputs.known_dimensions.height.unwrap_or(ascent + descent),
        };

        if inputs.run_mode == RunMode::PerformLayout {
            let place = [
                (numerator, num_margin, num_size, num_overflow, num_width, ascent - up - num_ascent),
                (denominator, den_margin, den_size, den_overflow, den_width, ascent + down - den_ascent),
            ];
            for (order, (child, margin, child_size, overflow, width, top)) in place.into_iter().enumerate() {
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
                        x: inset.left + (content_width - width) / 2.0 + margin.left,
                        y: top + margin.top,
                    },
                    size: child_size,
                    scrollable_overflow_rect: overflow,
                    scrollbar_size: Size::ZERO,
                    border: child_border,
                    padding: child_padding,
                    margin,
                };
                self.set_unrounded_layout(taffy_node_id(child), &layout);
            }
            // The bar, centred on the math axis (Chromium's `MathMLPainter::PaintFractionBar`).
            let bar = (params.thickness > 0.0).then(|| [ascent - params.axis_height - params.thickness / 2.0, params.thickness]);
            if let Some(element) = self.nodes[dom_id].element_data_mut() {
                element.math_fraction_bar = bar;
            }
        }

        let overflow = ScrollableOverflow::new(
            Rect {
                left: 0.0,
                top: 0.0,
                right: (size.width - border.left - border.right).max(0.0),
                bottom: (size.height - border.top - border.bottom).max(0.0),
            },
            Point { x: false, y: false },
        );
        LayoutOutput::from_sizes_and_baselines(size, overflow, taffy::Baselines::new(Some(ascent), Some(ascent)))
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
    let pair = fraction_children(doc, node).unwrap();
    doc.nodes[node_id].flags.insert(NodeFlags::IS_MATH_FRACTION);
    children.extend(pair);
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
        // Display style: three bars of gap.
        let p = FractionParameters::new(&font, 20.0, true, None);
        assert_eq!(p.numerator_gap_min, 3.0);
        // No bar: the stack gap (3 bars inline) split evenly.
        let p = FractionParameters::new(&font, 20.0, false, Some("0"));
        assert_eq!(p.thickness, 0.0);
        let (up, down) = p.shifts((15.0, 4.0), (15.0, 4.0));
        assert_eq!((up, down), (11.0, 11.0));
    }

    #[test]
    fn math_constants_header() {
        // Version 1.0, constants at offset 10, every record's value its index.
        let mut table = vec![0u8, 1, 0, 0, 0, 10, 0, 0, 0, 0];
        table.extend([0u8; 8]);
        for index in 0..MATH_CONSTANT_COUNT as i16 {
            table.extend(index.to_be_bytes());
            table.extend([0u8, 0]);
        }
        let values = read_math_constants(&table).unwrap();
        assert_eq!(values[MathConstant::AxisHeight as usize], 1);
        assert_eq!(values[MathConstant::FractionRuleThickness as usize], 34);
        assert_eq!(read_math_constants(&table[..20]), None);
    }
}
