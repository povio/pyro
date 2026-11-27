//! pyro: MathML layout (MathML Core § 3): a box class for the MathML elements that have a layout
//! algorithm of their own (`NodeFlags::IS_MATH_BOX`), a `MathKind` dispatch, and the OpenType
//! MATH constants the algorithms read. An element without one is built and laid out as it was
//! before this module existed: an inline box holding its children's inline layout (an `mo`'s
//! spacing is `mathml_operator`'s inline margins, an `mspace` is an inline-block of
//! `mathml_space`, text is text). The ordinary fraction (`fraction`) has a box class and layout of
//! its own (`NodeFlags::IS_MATH_FRACTION`, `compute_math_fraction_layout`), kept as it was.
//!
//! # The interface of a layout class
//!
//! An element's class is `math_kind(doc, node_id)`: its local name's class, or `Row` when it has
//! the wrong number of children for it (Core § 3.1: invalid markup is laid out as an `mrow`). The
//! classes after the row each have a module (`underover`, `scripts`, `radical`, `padded`, `table`,
//! `operator`), empty until their lane fills it, with two functions:
//!
//! * `construct(doc, node_id, kind, children) -> bool`, called while the box tree is built for
//!   the element (`construct_math`). The empty modules return `false`: the element is built
//!   as it was. A class returns `true` to be a math box, after pushing the element's layout
//!   children onto `children`: `build_children(doc, node_id, children)` does it for the usual
//!   case (every child element that generates a box, `display: none` ones left out, out-of-flow
//!   ones kept; `false`, nothing pushed, for text, `display: contents`, `::before` and `::after`,
//!   an inline child holding a block, or a `display` that is not a flow box). A class that finds
//!   its markup invalid calls `row::construct`. Returning `true` sets `NodeFlags::IS_MATH_BOX` and
//!   `ElementData::math_kind` (the driver reads the kind back at layout).
//! * `layout(state, node_id, kind, ctx, children) -> MathLayout`, run by the driver
//!   (`LayoutPassState::compute_math_layout`) for a box built as one (`row::layout_row(ctx,
//!   children)` is the row's, and the empty modules' `layout` is the row's too). Arguments:
//!   - `state: &mut LayoutPassState`: for what is not below: the element's attributes
//!     (`state.nodes[node_id]`), its children's styles, and `state.measure_math_child(child,
//!     known)` to measure a child again at a known width or height (a stretched operator).
//!   - `ctx: &MathContext`: the font size, `display_style` (`math-style: normal`), `math_depth`,
//!     `rtl` (`direction`), the `padding`, `border` and their sum `inset` (not part of the content
//!     the class lays out: the driver adds them and resolves `width`, `height`, `min-*`, `max-*`
//!     and `box-sizing`), and `font`: the first available font's MATH constants
//!     (`font.constant(MathConstant::..)` in CSS px, `None` without a MATH table: use Chromium's
//!     fallbacks as `fraction::FractionParameters` does) and x-height. `font` is empty for a row.
//!   - `children: &[ChildMetrics]`: the in-flow layout children, each measured at its max-content
//!     size: `ascent` (the top of its margin box to its baseline), `descent` (its baseline to the
//!     bottom of its margin box), border box `size`, `margin`, `italic_correction` (the child's
//!     `MathLayout::italic_correction`, 0 for a token) and its own `kind` if it is a math box.
//!   Returns `MathLayout`: the content's `content_width`, `ascent` and `descent` (the box's
//!   first and last baseline is `inset.top + ascent`), a `ChildPlacement` for every child
//!   (the `ChildMetrics` it is placed with, which a class that measured the child again
//!   replaces, and the offset of the child's MARGIN box from the content box's start edge,
//!   `inline_offset`, and top edge, `block_offset`; the driver mirrors `inline_offset` in an `rtl`
//!   box, a class only chooses which child comes first) and the box's `italic_correction`.
//!   Out-of-flow children are the driver's: positioned from the content box's start.
//!
//! The driver: `width`/`height` larger than the content keep the content at the start edge and
//! the baseline where it was (Chromium); the `math` of `display: block` centres its content.
use markup5ever::{LocalName, ns};
use style::values::specified::box_::{DisplayInside, DisplayOutside};
use taffy::{
    AvailableSpace, AxisStaticEdge, AxisStaticPosition, CoreStyle as _, LayoutInput, LayoutOutput, LayoutPartialTree as _,
    OofCandidate, OofPositioningArea, Point, ResolveOrZero as _, RunMode, Size, SizingMode,
};

use super::{LayoutPassState, resolve_calc_value};
use crate::node::{ElementData, NodeData, NodeFlags};
use crate::{BaseDocument, Node, NodeId, dom_node_id, taffy_node_id};

pub(crate) mod fraction;
pub(crate) mod operator;
pub(crate) mod padded;
pub(crate) mod radical;
pub(crate) mod row;
pub(crate) mod scripts;
pub(crate) mod table;
pub(crate) mod types;
pub(crate) mod underover;

pub(crate) use types::{ChildMetrics, ChildPlacement, MathContext, MathKind, MathLayout, TableKind};

/// The MATH table's `MathConstants` (OpenType § MATH). The discriminant indexes
/// `MathFontParameters::constants`: 0 to 51 are the table's records after its four leading 16-bit
/// fields, in order (51, the radical degree's raise, is a plain 16-bit percentage that follows
/// the last record); 52 to 55 are the four leading fields (the two percentages, the two minimum
/// heights).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MathConstant {
    MathLeading = 0,
    AxisHeight = 1,
    AccentBaseHeight = 2,
    FlattenedAccentBaseHeight = 3,
    SubscriptShiftDown = 4,
    SubscriptTopMax = 5,
    SubscriptBaselineDropMin = 6,
    SuperscriptShiftUp = 7,
    SuperscriptShiftUpCramped = 8,
    SuperscriptBottomMin = 9,
    SuperscriptBaselineDropMax = 10,
    SubSuperscriptGapMin = 11,
    SuperscriptBottomMaxWithSubscript = 12,
    SpaceAfterScript = 13,
    UpperLimitGapMin = 14,
    UpperLimitBaselineRiseMin = 15,
    LowerLimitGapMin = 16,
    LowerLimitBaselineDropMin = 17,
    StackTopShiftUp = 18,
    StackTopDisplayStyleShiftUp = 19,
    StackBottomShiftDown = 20,
    StackBottomDisplayStyleShiftDown = 21,
    StackGapMin = 22,
    StackDisplayStyleGapMin = 23,
    StretchStackTopShiftUp = 24,
    StretchStackBottomShiftDown = 25,
    StretchStackGapAboveMin = 26,
    StretchStackGapBelowMin = 27,
    FractionNumeratorShiftUp = 28,
    FractionNumeratorDisplayStyleShiftUp = 29,
    FractionDenominatorShiftDown = 30,
    FractionDenominatorDisplayStyleShiftDown = 31,
    FractionNumeratorGapMin = 32,
    FractionNumDisplayStyleGapMin = 33,
    FractionRuleThickness = 34,
    FractionDenominatorGapMin = 35,
    FractionDenomDisplayStyleGapMin = 36,
    SkewedFractionHorizontalGap = 37,
    SkewedFractionVerticalGap = 38,
    OverbarVerticalGap = 39,
    OverbarRuleThickness = 40,
    OverbarExtraAscender = 41,
    UnderbarVerticalGap = 42,
    UnderbarRuleThickness = 43,
    UnderbarExtraDescender = 44,
    RadicalVerticalGap = 45,
    RadicalDisplayStyleVerticalGap = 46,
    RadicalRuleThickness = 47,
    RadicalExtraAscender = 48,
    RadicalKernBeforeDegree = 49,
    RadicalKernAfterDegree = 50,
    /// A percentage (not a length): `constant` returns it unscaled.
    RadicalDegreeBottomRaisePercent = 51,
    /// A percentage (not a length).
    ScriptPercentScaleDown = 52,
    /// A percentage (not a length).
    ScriptScriptPercentScaleDown = 53,
    DelimitedSubFormulaMinHeight = 54,
    DisplayOperatorMinHeight = 55,
}

/// The number of values of [`MathConstant`].
pub(crate) const MATH_CONSTANT_COUNT: usize = 56;

/// Whether the constant at `index` is a percentage rather than a length in font units.
fn is_percentage_constant(index: usize) -> bool {
    matches!(index, 51..=53)
}

/// The math parameters of a font at a size (CSS px): its MATH constants, when it has a MATH
/// table, and its x-height.
#[derive(Clone, Debug, Default)]
pub(crate) struct MathFontParameters {
    /// Lengths in CSS px, percentages as they are in the table.
    pub(crate) constants: Option<[f32; MATH_CONSTANT_COUNT]>,
    pub(crate) x_height: Option<f32>,
}

impl MathFontParameters {
    pub(crate) fn constant(&self, constant: MathConstant) -> Option<f32> {
        self.constants.map(|values| values[constant as usize])
    }
}

/// Reads the `MathConstants` of a MATH table (`data`), in font units (percentages as they are).
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
        let offset = match index {
            // The records (and the percentage after them) follow the four leading 16-bit
            // fields; each record is a value and a device table offset.
            0..=51 => constants + 8 + 4 * index,
            // The leading fields: two percentages, two minimum heights.
            _ => constants + 2 * (index - 52),
        };
        *value = u16_at(offset)? as i16;
    }
    Some(values)
}

/// `values` (font units, [`read_math_constants`]) in CSS px at `font_size` for a font of
/// `units_per_em`; percentages stay.
pub(crate) fn scale_math_constants(values: [i16; MATH_CONSTANT_COUNT], font_size: f32, units_per_em: f32) -> [f32; MATH_CONSTANT_COUNT] {
    let mut scaled = [0.0f32; MATH_CONSTANT_COUNT];
    for (index, (out, value)) in scaled.iter_mut().zip(values).enumerate() {
        *out = if is_percentage_constant(index) { f32::from(value) } else { f32::from(value) * font_size / units_per_em };
    }
    scaled
}

/// The class of the MathML element with the local name `local`, as far as the name tells (its
/// children are not counted: `math_kind`). `None` for the elements that are never laid out as math
/// boxes of their own (`annotation`, `annotation-xml`, `mprescripts`, `none`, `mglyph`, the
/// alignment markers). An element with an unknown name is a row (Core § 3.1).
pub(crate) fn kind_by_name(local: &LocalName) -> Option<MathKind> {
    Some(match &**local {
        "mi" | "mn" | "mtext" | "ms" => MathKind::Token,
        "mo" => MathKind::Operator,
        "mspace" => MathKind::Space,
        "mfrac" => MathKind::Fraction,
        "msub" => MathKind::Sub,
        "msup" => MathKind::Sup,
        "msubsup" => MathKind::SubSup,
        "mmultiscripts" => MathKind::Multiscripts,
        "munder" => MathKind::Under,
        "mover" => MathKind::Over,
        "munderover" => MathKind::UnderOver,
        "msqrt" => MathKind::Sqrt,
        "mroot" => MathKind::Root,
        "mpadded" => MathKind::Padded,
        "mtable" => MathKind::Table(TableKind::Table),
        "mtr" => MathKind::Table(TableKind::Row),
        "mlabeledtr" => MathKind::Table(TableKind::LabeledRow),
        "mtd" => MathKind::Table(TableKind::Cell),
        "annotation" | "annotation-xml" | "mprescripts" | "none" | "mglyph" | "malignmark" | "maligngroup" => return None,
        _ => MathKind::Row,
    })
}

/// How many element children the class `kind` needs (`None`: any number), for the markup to be valid.
fn required_children(kind: MathKind) -> Option<std::ops::RangeInclusive<usize>> {
    match kind {
        MathKind::Fraction | MathKind::Under | MathKind::Over | MathKind::Sub | MathKind::Sup | MathKind::Root => Some(2..=2),
        MathKind::UnderOver | MathKind::SubSup => Some(3..=3),
        MathKind::Multiscripts => Some(1..=usize::MAX),
        _ => None,
    }
}

/// The class of the MathML element `node_id`: its name's class, a `Row` if its children are
/// invalid for it (the number of element children that generate a box: not `display: none`).
/// `None` for a node that is not a MathML element or has a class of no layout of its own.
pub(crate) fn math_kind(doc: &BaseDocument, node_id: NodeId) -> Option<MathKind> {
    let element = doc.nodes[node_id].element_data()?;
    if element.name.ns != ns!(mathml) {
        return None;
    }
    let kind = kind_by_name(&element.name.local)?;
    let Some(required) = required_children(kind) else { return Some(kind) };
    let in_flow: Vec<&Node> = doc.nodes[node_id]
        .children
        .iter()
        .map(|&child| &doc.nodes[child])
        .filter(|child| {
            matches!(child.data, NodeData::Element(_))
                && !child.display_style().is_some_and(|display| display.is_none())
                // An out-of-flow child takes no part in the layout (Core § 3.1): the markup is as
                // valid as without it (WPT `not-participating-to-parent-layout`).
                && !child.is_out_of_flow()
        })
        .collect();
    let valid = required.contains(&in_flow.len()) && (kind != MathKind::Multiscripts || valid_multiscripts(&in_flow));
    Some(if valid { kind } else { MathKind::Row })
}

/// Whether the in-flow element children of an `mmultiscripts` are valid (MathML Core § 3.4.7): a
/// base, then pairs of post-scripts, then optionally an `mprescripts` followed by pairs of
/// pre-scripts. A first child that is an `mprescripts`, a second `mprescripts`, or an odd number
/// of scripts on either side makes it an `mrow` (as Chromium).
fn valid_multiscripts(children: &[&Node]) -> bool {
    let is_prescripts = |node: &Node| {
        node.element_data().is_some_and(|e| e.name.ns == ns!(mathml) && &*e.name.local == "mprescripts")
    };
    if children.first().is_some_and(|first| is_prescripts(first)) {
        return false;
    }
    let mut prescripts = children.iter().enumerate().filter(|(_, child)| is_prescripts(child)).map(|(i, _)| i);
    let first = prescripts.next();
    if prescripts.next().is_some() {
        return false;
    }
    match first {
        // The base and the post-script pairs, then the pre-script pairs.
        Some(at) => (at - 1) % 2 == 0 && (children.len() - at - 1) % 2 == 0,
        None => (children.len() - 1) % 2 == 0,
    }
}

/// Whether `element` is an atomic inline when inline-level (`replaced::is_inline_box`): every
/// MathML element (Chromium lays them out as `display: math`, a block-level box that is atomic
/// inside a line; so `width`, `height`, `contain: size` and floats' containment apply to the
/// tokens and the elements without a layout class of their own as to a row). One that is not
/// built as a box, `construct` returning `false`, holds its children's inline layout, as an
/// inline-block would.
pub(crate) fn is_atomic_inline(element: &ElementData) -> bool {
    element.name.ns == ns!(mathml)
}

/// Construction of the box of a MathML element (`construct::collect_layout_children_with_wrap`):
/// returns whether `node_id` is built as a math box, its layout children pushed onto `children`.
/// Any other element is built as any other box. See the module's documentation.
pub(crate) fn construct_math(doc: &mut BaseDocument, node_id: NodeId, children: &mut thin_vec::ThinVec<NodeId>) -> bool {
    {
        let Some(element) = doc.nodes[node_id].element_data_mut() else { return false };
        if element.name.ns != ns!(mathml) {
            return false;
        }
        // A box built again has no bar, no kind and no italic correction until it is laid out.
        element.math_fraction_bar = None;
        element.math_stretch = None;
        element.math_kind = None;
        element.math_italic_correction = 0.0;
    }
    let Some(kind) = math_kind(doc, node_id) else { return false };
    let built = match kind {
        // The fraction keeps its own flag and layout (`ChildLayout::MathFraction`).
        MathKind::Fraction => return fraction::construct_fraction(doc, node_id, children),
        MathKind::Row => row::construct(doc, node_id, kind, children),
        MathKind::Under | MathKind::Over | MathKind::UnderOver => underover::construct(doc, node_id, kind, children),
        MathKind::Sub | MathKind::Sup | MathKind::SubSup | MathKind::Multiscripts => scripts::construct(doc, node_id, kind, children),
        MathKind::Sqrt | MathKind::Root => radical::construct(doc, node_id, kind, children),
        MathKind::Padded => padded::construct(doc, node_id, kind, children),
        MathKind::Table(_) => table::construct(doc, node_id, kind, children),
        MathKind::Operator => operator::construct(doc, node_id, kind, children),
        MathKind::Space | MathKind::Token => false,
    };
    if built {
        doc.nodes[node_id].flags.insert(NodeFlags::IS_MATH_BOX);
        if let Some(element) = doc.nodes[node_id].element_data_mut() {
            element.math_kind = Some(kind);
        }
    }
    built
}

/// The usual layout children of a math box: `node_id`'s element children that generate a box
/// (`display: none` ones do not; out-of-flow ones are kept, the driver positions them) pushed
/// onto `children`. Returns whether it is one: `false` (nothing pushed) for an element whose
/// `display` is not a flow box, with `::before` or `::after`, text besides white space (its
/// own content: a token is no box of this kind), a child of `display: contents` (it takes no
/// part in layout, its children would), or an inline child holding a block.
pub(crate) fn build_children(doc: &BaseDocument, node_id: NodeId, children: &mut thin_vec::ThinVec<NodeId>) -> bool {
    build_children_in(doc, node_id, children, false)
}

/// [`build_children`]; with `cell` an element whose `display` is a table cell (an `mtd`'s, by the user
/// agent sheet) is a box of this kind too: its children are laid out as an inferred `mrow`.
pub(crate) fn build_children_in(doc: &BaseDocument, node_id: NodeId, children: &mut thin_vec::ThinVec<NodeId>, cell: bool) -> bool {
    let node = &doc.nodes[node_id];
    let Some(display) = node.display_style() else { return false };
    let flow = display.inside() == DisplayInside::Flow && matches!(display.outside(), DisplayOutside::Inline | DisplayOutside::Block);
    if !flow && !(cell && display.inside() == DisplayInside::TableCell) {
        return false;
    }
    if node.before().is_some() || node.after().is_some() {
        return false;
    }
    let mut found: Vec<NodeId> = Vec::new();
    for &child_id in node.children.iter() {
        let child = &doc.nodes[child_id];
        match &child.data {
            NodeData::Comment { .. } => {}
            NodeData::Text(text) => {
                if !text.content.chars().all(|c| matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{c}')) {
                    return false;
                }
            }
            NodeData::Element(_) => {
                let Some(display) = child.display_style() else { return false };
                if display.is_none() {
                    continue;
                }
                if display.is_contents() {
                    return false;
                }
                if display.outside() == DisplayOutside::Inline && child.is_or_contains_block() {
                    return false;
                }
                found.push(child_id);
            }
            _ => return false,
        }
    }
    children.extend(found);
    true
}

/// The ascent of an `mspace` (MathML Core § 3.2.5: its `height` attribute, a non-negative length,
/// else 0; its `depth` only sizes the box), `None` for any other box. The box is an inline-block
/// without line boxes (`mathml_space`), whose size the author's `height` may override without
/// moving the baseline from the attribute (WPT `mspace-width-height-001`).
fn mspace_ascent(node: &Node) -> Option<f32> {
    let element = node.element_data()?;
    if !(element.name.ns == ns!(mathml) && *element.name.local == *"mspace") {
        return None;
    }
    let style = node.primary_styles()?;
    let font_size = style.slow_clone_font_size().used_size().px();
    Some(
        element
            .attr(LocalName::from("height"))
            .and_then(|value| fraction::parse_line_thickness(value, 0.0, font_size, font_size * 0.5))
            .unwrap_or(0.0),
    )
}

impl LayoutPassState<'_> {
    /// Measures the box `child` of a math box at its max-content size, or at the `known` width and
    /// height where given (a stretched operator), laid out as an independent formatting context.
    #[inline(never)]
    pub(crate) fn measure_math_child(&mut self, child: NodeId, known: Size<Option<f32>>) -> ChildMetrics {
        let margin = self.nodes[child].layout_style().margin().resolve_or_zero(None, resolve_calc_value);
        let inputs = LayoutInput {
            run_mode: RunMode::PerformLayout,
            sizing_mode: SizingMode::InherentSize,
            axis: taffy::RequestedAxis::Both,
            known_dimensions: known,
            known_dimensions_are_definite: Size { width: known.width.is_some(), height: known.height.is_some() },
            parent_size: Size::NONE,
            available_space: Size { width: AvailableSpace::MaxContent, height: AvailableSpace::MaxContent },
            vertical_margins_are_collapsible: taffy::Line::FALSE,
        };
        let output = self.compute_child_layout(taffy_node_id(child), inputs);
        // A child without a baseline has one at its bottom border edge (Chromium's
        // `FirstBaselineOrSynthesize`).
        let baseline = mspace_ascent(&self.nodes[child])
            .unwrap_or_else(|| output.baselines.first.or(output.baselines.last).unwrap_or(output.size.height));
        let ascent = margin.top + baseline;
        let descent = output.size.height + margin.top + margin.bottom - ascent;
        let element = self.nodes[child].element_data();
        let metrics = ChildMetrics {
            id: child,
            margin,
            size: output.size,
            overflow: output.scrollable_overflow_rect,
            ascent,
            descent,
            italic_correction: element.map_or(0.0, |element| element.math_italic_correction),
            kind: element.and_then(|element| element.math_kind),
        };
        // An operator stretched along the block axis or large is as `operator` makes it (one
        // measured at a known size, stretched along the inline axis by its parent, is not).
        let metrics = table::on_axis(self, metrics);
        if known.width.is_none() && known.height.is_none() { operator::stretch(self, metrics) } else { metrics }
    }

    /// What the layout classes are told about the box `dom_id` (`parent_width`: what its
    /// percentages resolve against).
    fn math_context(&self, dom_id: NodeId, kind: MathKind, parent_width: Option<f32>) -> Option<MathContext> {
        let node = &self.nodes[dom_id];
        let (font_size, display_style, math_depth, rtl, font) = {
            let style = node.primary_styles()?;
            let font_size = style.slow_clone_font_size().used_size().px();
            let display_style = style.get_font().math_style == style::values::computed::MathStyle::Normal;
            let math_depth = style.get_font().math_depth as i32;
            let rtl = style.slow_clone_direction() == style::computed_values::direction::T::Rtl;
            // The row reads no constant: the font is looked up for the classes that do.
            let font = if kind == MathKind::Row {
                MathFontParameters::default()
            } else {
                let provider = crate::font_metrics::BlitzFontMetricsProvider { font_ctx: self.font_ctx.clone() };
                provider.math_parameters_for(style.get_font(), font_size)
            };
            (font_size, display_style, math_depth, rtl, font)
        };
        let layout_style = node.layout_style();
        let padding = layout_style.padding().resolve_or_zero(parent_width, resolve_calc_value);
        let border = layout_style.border().resolve_or_zero(parent_width, resolve_calc_value);
        Some(MathContext { font_size, display_style, math_depth, cramped: false, rtl, font, padding, border, inset: padding + border })
    }

    /// Lays out the math box `node_id` built by `construct_math` (see the module's documentation):
    /// its in-flow children measured, its class's layout function run on them, the box resolved
    /// from the content it returns and its children placed (mirrored in an `rtl` box).
    #[inline(never)]
    pub(crate) fn compute_math_layout(&mut self, node_id: taffy::NodeId, inputs: LayoutInput) -> LayoutOutput {
        let dom_id = dom_node_id(node_id);
        if inputs.run_mode == RunMode::PerformHiddenLayout {
            return LayoutOutput::HIDDEN;
        }
        let Some(kind) = self.nodes[dom_id].element_data().and_then(|element| element.math_kind) else {
            return LayoutOutput::HIDDEN;
        };
        let listed: Vec<NodeId> = self.nodes[dom_id].layout_children.borrow().as_ref().map(|c| c.to_vec()).unwrap_or_default();
        let Some(ctx) = self.math_context(dom_id, kind, inputs.parent_size.width) else { return LayoutOutput::HIDDEN };
        let (flow, out_of_flow): (Vec<NodeId>, Vec<NodeId>) = listed.into_iter().partition(|&child| !self.nodes[child].is_out_of_flow());
        let mut metrics: Vec<ChildMetrics> = flow.iter().map(|&child| self.measure_math_child(child, Size::NONE)).collect();
        // The operators of a row stretch to the row's children (Core § 3.2.4.2): an `msqrt`, an
        // `mpadded` and a cell lay their children out as a row.
        if matches!(kind, MathKind::Row | MathKind::Sqrt | MathKind::Padded | MathKind::Table(TableKind::Cell)) {
            operator::stretch_children(self, &mut metrics);
        }

        let layout = match kind {
            MathKind::Under | MathKind::Over | MathKind::UnderOver => underover::layout(self, dom_id, kind, &ctx, &metrics),
            MathKind::Sub | MathKind::Sup | MathKind::SubSup | MathKind::Multiscripts => scripts::layout(self, dom_id, kind, &ctx, &metrics),
            MathKind::Sqrt | MathKind::Root => radical::layout(self, dom_id, kind, &ctx, &metrics),
            MathKind::Padded => padded::layout(self, dom_id, kind, &ctx, &metrics),
            MathKind::Table(_) => table::layout(self, dom_id, kind, &ctx, &metrics),
            MathKind::Operator => operator::layout(self, dom_id, kind, &ctx, &metrics),
            // (The fraction has its own `ChildLayout`; a token or space is no math box.)
            MathKind::Row | MathKind::Fraction | MathKind::Space | MathKind::Token => row::layout_row(&ctx, &metrics),
        };

        // The box: `width`, `height`, `min-*`, `max-*` and `box-sizing` resolved against the
        // content the class laid out.
        let mut output = {
            let style = self.nodes[dom_id].layout_style();
            let content = Size { width: layout.content_width, height: layout.ascent + layout.descent };
            taffy::compute_leaf_layout(inputs, &style, resolve_calc_value, |_, _| content)
        };
        let inset = ctx.inset;
        let baseline = inset.top + layout.ascent;
        output.baselines = taffy::Baselines::new(Some(baseline), Some(baseline));
        if inputs.run_mode != RunMode::PerformLayout {
            return output;
        }

        let content_box_width = (output.size.width - inset.left - inset.right).max(0.0);
        // `display: block math` centres its content in the box (Chromium's row algorithm).
        let centred = (self.nodes[dom_id].element_data().is_some_and(|element| *element.name.local == *"math")
            && self.nodes[dom_id].display_style().is_some_and(|display| display.outside() == DisplayOutside::Block))
            // A table cell centres its content in the width the column gives it (`text-align: center`).
            || kind == MathKind::Table(TableKind::Cell);
        let centre = if centred { ((content_box_width - layout.content_width) / 2.0).max(0.0) } else { 0.0 };
        for (order, placement) in layout.children.iter().enumerate() {
            let metrics = &placement.metrics;
            let inline = placement.inline_offset + centre;
            let left = if ctx.rtl { inset.left + content_box_width - inline - metrics.margin_box_width() } else { inset.left + inline };
            let top = inset.top + placement.block_offset;
            let (child_padding, child_border) = {
                let style = self.nodes[metrics.id].layout_style();
                (
                    style.padding().resolve_or_zero(None, resolve_calc_value),
                    style.border().resolve_or_zero(None, resolve_calc_value),
                )
            };
            let child_layout = taffy::Layout {
                order: order as u32,
                location: Point { x: left + metrics.margin.left, y: top + metrics.margin.top },
                size: metrics.size,
                scrollable_overflow_rect: metrics.overflow,
                scrollbar_size: Size::ZERO,
                border: child_border,
                padding: child_padding,
                margin: metrics.margin,
            };
            self.set_unrounded_layout(taffy_node_id(metrics.id), &child_layout);
        }

        // Out-of-flow children are positioned by their containing block: their static position is
        // the content box's start.
        if !out_of_flow.is_empty() {
            let static_position = Point { x: inset.left, y: inset.top }.map(|at| AxisStaticPosition::from_edge(at, AxisStaticEdge::Start));
            for (index, &child) in out_of_flow.iter().enumerate() {
                output.oof_candidates.push(OofCandidate {
                    containing_block: None,
                    node: taffy_node_id(child),
                    order: (flow.len() + index) as u32,
                    position: self.nodes[child].taffy_position(),
                    static_position,
                });
            }
            output.oof_positioning_area = Some(OofPositioningArea {
                size: output.size - ctx.border.sum_axes(),
                offset: Point { x: ctx.border.left, y: ctx.border.top },
            });
        }
        if let Some(element) = self.nodes[dom_id].element_data_mut() {
            element.math_italic_correction = layout.italic_correction;
        }
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn math_constants_header() {
        // Version 1.0, constants at offset 10; the four leading fields hold 1000 + their index,
        // every record's value its index, the percentage after them 51.
        let mut table = vec![0u8, 1, 0, 0, 0, 10, 0, 0, 0, 0];
        for lead in 0..4i16 {
            table.extend((1000 + lead).to_be_bytes());
        }
        for index in 0..51i16 {
            table.extend(index.to_be_bytes());
            table.extend([0u8, 0]);
        }
        table.extend(51i16.to_be_bytes());
        let values = read_math_constants(&table).unwrap();
        assert_eq!(values[MathConstant::AxisHeight as usize], 1);
        assert_eq!(values[MathConstant::FractionRuleThickness as usize], 34);
        assert_eq!(values[MathConstant::RadicalKernAfterDegree as usize], 50);
        assert_eq!(values[MathConstant::RadicalDegreeBottomRaisePercent as usize], 51);
        assert_eq!(values[MathConstant::ScriptPercentScaleDown as usize], 1000);
        assert_eq!(values[MathConstant::ScriptScriptPercentScaleDown as usize], 1001);
        assert_eq!(values[MathConstant::DelimitedSubFormulaMinHeight as usize], 1002);
        assert_eq!(values[MathConstant::DisplayOperatorMinHeight as usize], 1003);
        assert_eq!(read_math_constants(&table[..20]), None);
        // Lengths scale with the font size, percentages do not.
        let scaled = scale_math_constants(values, 20.0, 1000.0);
        assert!((scaled[MathConstant::DisplayOperatorMinHeight as usize] - 20.06).abs() < 1e-3);
        assert_eq!(scaled[MathConstant::ScriptPercentScaleDown as usize], 1000.0);
        assert_eq!(scaled[MathConstant::RadicalDegreeBottomRaisePercent as usize], 51.0);
    }

    #[test]
    fn classes_by_name() {
        let kind = |name: &str| kind_by_name(&LocalName::from(name));
        assert_eq!(kind("math"), Some(MathKind::Row));
        assert_eq!(kind("mrow"), Some(MathKind::Row));
        assert_eq!(kind("mfenced"), Some(MathKind::Row));
        assert_eq!(kind("mfrac"), Some(MathKind::Fraction));
        assert_eq!(kind("mtd"), Some(MathKind::Table(TableKind::Cell)));
        assert_eq!(kind("mi"), Some(MathKind::Token));
        assert_eq!(kind("annotation"), None);
        assert_eq!(required_children(MathKind::SubSup), Some(3..=3));
        assert_eq!(required_children(MathKind::Row), None);
    }
}
