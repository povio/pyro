//! pyro: the layout of an operator `mo` that stretches or is large (MathML Core § 3.2.4, "Layout of
//! operators", "Algorithm for stretching operators along the block axis"; Chromium's
//! `MathMLRowLayoutAlgorithm` and `MathMLOperatorElement`).
//!
//! An `mo` stays what it was built as, an atomic inline holding its text. What changes is its
//! measure (`LayoutPassState::measure_math_child` calls [`stretch`]):
//!
//! * A row-like box (`row`, `msqrt`, `mpadded`, a cell) measures its children, takes their greatest
//!   ascent and descent as the *target* (the stretchy operators count at their unstretched size),
//!   and measures again at that target each child that is, or is an embellished operator of, a
//!   vertically `stretchy` operator ([`stretch_children`]). The target travels in
//!   `LayoutPassState::math_stretch`, for the core `mo` alone, so a nested row or an `munder` over
//!   it measures its own children as usual.
//! * The operator's target is made symmetric about the math axis if it is `symmetric`, its size
//!   `T` clamped to `minsize` (default 100% of the unstretched size) and `maxsize` (default
//!   infinity; percentages of the unstretched size, negative values zero, `maxsize` below
//!   `minsize` is `minsize`). The first size variant of the MATH table's `MathVariants` at least
//!   that tall is the glyph, else a glyph assembly of that size, else the largest variant. The
//!   box is `H` tall (the variant's advance or the assembly's size), its ascent
//!   `max(0, Tascent - axis) * H / T + axis` (`H / 2 + axis` for `T` = 0, so for a symmetric
//!   target), its descent the rest.
//! * A `largeop` in display style takes the first size variant at least `DisplayOperatorMinHeight`
//!   tall (the largest if there is none), centred on the axis if it is `symmetric`, on the
//!   glyph's ink otherwise; with a target, the stretch above, no smaller than that.
//!
//! The box's width is the chosen glyph's advance (the widest part of an assembly), and what it
//! paints is recorded (`ElementData::math_stretch`: the variant glyph, or the placed parts of the
//! assembly; blitz-paint draws them in place of the text).
//!
//! Not done: horizontal stretching of an `mo` (the stretch along the inline axis is `underover`'s
//! measure at a known width, this class draws no glyph for it), an operator whose text is more than
//! one character, stretching of an `mo` of a font without a MATH table (no variants: it keeps the
//! text's size), and the rtlm/mirrored glyph of an `rtl` operator.
use markup5ever::{LocalName, ns};
use taffy::{ResolveOrZero as _, Size};
use thin_vec::ThinVec;

use super::fraction::parse_line_thickness;
use super::types::{ChildMetrics, GlyphPart, MathContext, MathKind, MathLayout, glyph_construction, min_connector_overlap};
use super::underover;
use super::MathConstant;
use crate::layout::{LayoutPassState, resolve_calc_value};
use crate::mathml_operator;
use crate::node::{MathStretch, MathStretchGlyph};
use crate::{BaseDocument, NodeId};

/// Construction (see the module's documentation): `false`, the element is built as it was.
pub(crate) fn construct(_doc: &mut BaseDocument, _node_id: NodeId, _kind: MathKind, _children: &mut ThinVec<NodeId>) -> bool {
    false
}

/// Layout of a box `construct` built (see the module's documentation): the row's (never called,
/// `construct` builds no box).
pub(crate) fn layout(
    _state: &mut LayoutPassState<'_>,
    _node_id: NodeId,
    _kind: MathKind,
    ctx: &MathContext,
    children: &[ChildMetrics],
) -> MathLayout {
    super::row::layout_row(ctx, children)
}

/// The block stretch target the operator `core` is measured at: the ascent and descent its row
/// asks for (CSS px, the largest of its children's).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct StretchTarget {
    pub(crate) core: NodeId,
    pub(crate) ascent: f32,
    pub(crate) descent: f32,
}

/// What the dictionary and the attributes make of an `mo` of a single character.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Properties {
    stretchy: bool,
    symmetric: bool,
    largeop: bool,
    /// The dictionary stretches it along the inline axis (it stretches no vertical target).
    horizontal: bool,
}

/// The properties of the `mo` `id`: its `stretchy`, `symmetric` and `largeop` attributes over the
/// dictionary entry of its text and form. `None` for another element or text of more or less than
/// one character (it neither stretches nor is large).
fn properties(doc: &BaseDocument, id: NodeId) -> Option<Properties> {
    let node = &doc.nodes[id];
    let element = node.element_data()?;
    if element.name.ns != ns!(mathml) || &*element.name.local != "mo" {
        return None;
    }
    let text = node.text_content();
    let text = text.trim_matches(|c: char| c.is_ascii_whitespace());
    let mut characters = text.chars();
    characters.next()?;
    if characters.next().is_some() {
        return None;
    }
    let entry = mathml_operator::entry(text, mathml_operator::operator_form(node));
    let flag = |name: &str, default: bool| match element.attr(LocalName::from(name)).map(str::to_ascii_lowercase).as_deref() {
        Some("true") => true,
        Some("false") => false,
        _ => default,
    };
    Some(Properties {
        stretchy: flag("stretchy", entry.stretchy()),
        symmetric: flag("symmetric", entry.symmetric()),
        largeop: flag("largeop", entry.largeop()),
        horizontal: entry.horizontal(),
    })
}

/// The glyph the operator `id`'s text was shaped to, in the font that shaped it (the first of the
/// `font-family` list that has the character, or the fallback font: the one the box was measured
/// with), at its size.
struct Shaped {
    font: parley::FontData,
    size: f32,
    glyph: u16,
}

impl Shaped {
    fn of(state: &LayoutPassState<'_>, id: NodeId) -> Option<Shaped> {
        let layout = &state.nodes[id].element_data()?.inline_layout_data.as_ref()?.layout;
        for line in layout.lines() {
            for item in line.items() {
                let parley::PositionedLayoutItem::GlyphRun(glyph_run) = item else { continue };
                let run = glyph_run.run();
                let glyph = glyph_run.positioned_glyphs().next()?.id;
                // (Glyph 0 is the font's .notdef: the character has no glyph in it.)
                if glyph == 0 {
                    return None;
                }
                return Some(Shaped { font: run.font().clone(), size: run.font_size(), glyph: u16::try_from(glyph).ok()? });
            }
        }
        None
    }

    fn font_ref(&self) -> Option<skrifa::FontRef<'_>> {
        skrifa::FontRef::from_index(self.font.data.as_ref(), self.font.index).ok()
    }

    /// CSS px per font unit.
    fn scale(&self, font_ref: &skrifa::FontRef<'_>) -> Option<f32> {
        use skrifa::instance::{LocationRef, Size};
        let upem = skrifa::metrics::Metrics::new(font_ref, Size::unscaled(), LocationRef::default()).units_per_em;
        (upem > 0).then(|| self.size / f32::from(upem))
    }

    /// The advance width and the ink's ascent and descent of `glyph`, CSS px.
    fn extents(font_ref: &skrifa::FontRef<'_>, scale: f32, glyph: u16) -> Option<(f32, f32, f32)> {
        use skrifa::MetadataProvider as _;
        use skrifa::instance::{LocationRef, Size};
        let metrics = font_ref.glyph_metrics(Size::unscaled(), LocationRef::default());
        let id = skrifa::GlyphId::new(u32::from(glyph));
        let advance = metrics.advance_width(id)? * scale;
        let (ascent, descent) = metrics.bounds(id).map_or((0.0, 0.0), |bounds| ((bounds.y_max * scale).max(0.0), (-bounds.y_min * scale).max(0.0)));
        Some((advance, ascent, descent))
    }
}

/// A `minsize` or `maxsize` value in CSS px: a length, a percentage of the `unstretched` size, or
/// `infinity`; a negative value is 0. `None` for any other value (the default applies).
fn size_attribute(value: &str, unstretched: f32, font_size: f32, x_height: f32) -> Option<f32> {
    let value = value.trim_matches(|c: char| c.is_ascii_whitespace());
    if value.eq_ignore_ascii_case("infinity") {
        return Some(f32::INFINITY);
    }
    match value.strip_prefix('-') {
        Some(magnitude) => parse_line_thickness(magnitude, unstretched, font_size, x_height).map(|_| 0.0),
        None => parse_line_thickness(value, unstretched, font_size, x_height),
    }
}

/// A part of a glyph assembly as placed: its glyph, the offset of its start from the assembly's
/// start edge and its advance along the axis, CSS px.
#[derive(Clone, Copy, Debug, PartialEq)]
#[allow(dead_code)]
pub(crate) struct PlacedPart {
    pub(crate) glyph: u16,
    pub(crate) start: f32,
    pub(crate) advance: f32,
}

/// The assembly of `parts` (font units times `scale`) for a glyph `target` long (OpenType MATH
/// § MathGlyphConstruction, Chromium's `GlyphAssembly`): every part once and the extenders
/// repeated, as few times as reach the target with the least overlap (`min_overlap`, between two
/// connected parts at most the shorter of their connectors); then the overlap is raised to make
/// the assembly as long as the target. Returns the parts and the assembly's length (the target
/// when the overlaps allow it, else the longest the parts make); `None` without parts.
pub(crate) fn assemble(parts: &[GlyphPart], scale: f32, min_overlap: f32, target: f32) -> Option<(Vec<PlacedPart>, f32)> {
    if parts.is_empty() {
        return None;
    }
    let advance = |part: &GlyphPart| f32::from(part.full_advance) * scale;
    let has_extender = parts.iter().any(|part| part.extender);
    let sequence = |repeats: usize| -> Vec<&GlyphPart> {
        parts.iter().flat_map(|part| std::iter::repeat(part).take(if part.extender { repeats } else { 1 })).collect()
    };
    // The fewest repeats whose assembly at the least overlap reaches the target.
    let mut repeats = usize::from(has_extender);
    let mut chosen = sequence(repeats);
    while has_extender && repeats < 4096 {
        let total: f32 = chosen.iter().map(|part| advance(*part)).sum();
        if total - (chosen.len() as f32 - 1.0) * min_overlap >= target {
            break;
        }
        repeats += 1;
        chosen = sequence(repeats);
    }
    let total: f32 = chosen.iter().map(|part| advance(*part)).sum();
    let connections = chosen.len() - 1;
    // The most two connected parts may overlap.
    let most = |from: &GlyphPart, to: &GlyphPart| (f32::from(from.end_connector_length.min(to.start_connector_length)) * scale).max(min_overlap);
    let overlap = if connections == 0 { 0.0 } else { ((total - target) / connections as f32).max(min_overlap) };
    let mut placed = Vec::with_capacity(chosen.len());
    let mut start = 0.0f32;
    for (index, part) in chosen.iter().enumerate() {
        placed.push(PlacedPart { glyph: part.glyph, start, advance: advance(*part) });
        if let Some(next) = chosen.get(index + 1) {
            start += advance(*part) - overlap.min(most(*part, *next));
        }
    }
    let length = placed.last().map(|last| last.start + last.advance)?;
    Some((placed, length))
}

/// Measured again at the target of its row, an operator's box and paint: what [`stretch`] makes of
/// the natural metrics of the `mo` `natural.id`.
struct Stretched {
    ascent: f32,
    descent: f32,
    /// The chosen glyph's advance width less the character's own: what the box grows by.
    width_delta: f32,
    paint: MathStretch,
}

/// The metrics of the `mo` `natural.id` as the operator layout makes them (see the module's
/// documentation), `natural` if it neither stretches nor is large. Records (or clears) what it
/// paints.
pub(crate) fn stretch(state: &mut LayoutPassState<'_>, natural: ChildMetrics) -> ChildMetrics {
    let id = natural.id;
    let is_mo = state.nodes[id].element_data().is_some_and(|element| element.name.ns == ns!(mathml) && &*element.name.local == "mo");
    if !is_mo {
        return natural;
    }
    let stretched = compute(state, &natural);
    if let Some(element) = state.nodes[id].element_data_mut() {
        element.math_stretch = stretched.as_ref().map(|stretched| stretched.paint.clone());
    }
    let Some(stretched) = stretched else { return natural };
    let (extra_top, extra_bottom) = vertical_extras(state, id);
    let mut metrics = natural;
    metrics.size.height = stretched.ascent + stretched.descent + extra_top + extra_bottom;
    metrics.size.width = (natural.size.width + stretched.width_delta).max(0.0);
    metrics.ascent = natural.margin.top + extra_top + stretched.ascent;
    metrics.descent = stretched.descent + extra_bottom + natural.margin.bottom;
    metrics
}

/// The top and bottom padding and border of the box `id`, CSS px.
fn vertical_extras(state: &LayoutPassState<'_>, id: NodeId) -> (f32, f32) {
    use taffy::CoreStyle as _;
    let style = state.nodes[id].layout_style();
    let padding = style.padding().resolve_or_zero(None, resolve_calc_value);
    let border = style.border().resolve_or_zero(None, resolve_calc_value);
    (padding.top + border.top, padding.bottom + border.bottom)
}

/// The stretched or large operator box of the `mo` `natural.id`, if it is one.
fn compute(state: &LayoutPassState<'_>, natural: &ChildMetrics) -> Option<Stretched> {
    let id = natural.id;
    let properties = properties(state, id)?;
    let target = state.math_stretch.filter(|target| target.core == id);
    let vertical = properties.stretchy && !properties.horizontal && target.is_some();
    let (font_size, display_style, font) = {
        let node = &state.nodes[id];
        let style = node.primary_styles()?;
        let font_size = style.slow_clone_font_size().used_size().px();
        let display_style = style.get_font().math_style == style::values::computed::MathStyle::Normal;
        let provider = crate::font_metrics::BlitzFontMetricsProvider { font_ctx: state.font_ctx.clone() };
        (font_size, display_style, provider.math_parameters_for(style.get_font(), font_size))
    };
    let large = properties.largeop && display_style;
    if !vertical && !large {
        return None;
    }

    let shaped = Shaped::of(state, id)?;
    let font_ref = shaped.font_ref()?;
    let scale = shaped.scale(&font_ref)?;
    let base = shaped.glyph;
    let math = font_ref.table_data(skrifa::Tag::new(b"MATH"))?;
    let construction = glyph_construction(math.as_bytes(), base, false)?;
    let extents = |glyph: u16| Shaped::extents(&font_ref, scale, glyph);
    let advance_of = |glyph: u16| extents(glyph).map_or(0.0, |(advance, ..)| advance);
    let base_advance = advance_of(base);
    let x_height = font.x_height.unwrap_or(font_size * 0.5);
    let axis = font.constant(MathConstant::AxisHeight).unwrap_or(x_height / 2.0);
    let (extra_top, extra_bottom) = vertical_extras(state, id);
    let unstretched = (natural.ascent - natural.margin.top - extra_top + natural.descent - natural.margin.bottom - extra_bottom).max(0.0);
    let display_min = if large { font.constant(MathConstant::DisplayOperatorMinHeight).unwrap_or(0.0) } else { 0.0 };

    // What to draw: (a variant's glyph) or (an assembly's parts), and its length along the axis.
    enum Choice {
        Variant(u16),
        Assembly(Vec<PlacedPart>),
    }
    let (ascent, descent, choice, length) = if let Some(target) = target.filter(|_| vertical) {
        let (mut target_ascent, mut target_descent) = (target.ascent, target.descent);
        if properties.symmetric {
            let half = (target_ascent - axis).max(target_descent + axis);
            target_ascent = half + axis;
            target_descent = half - axis;
        }
        let target_size = target_ascent + target_descent;
        let element = state.nodes[id].element_data()?;
        let attribute = |name: &str| element.attr(LocalName::from(name)).and_then(|value| size_attribute(value, unstretched, font_size, x_height));
        let min = attribute("minsize").unwrap_or(unstretched).max(0.0);
        let max = attribute("maxsize").unwrap_or(f32::INFINITY).max(min);
        let size = target_size.max(min).min(max).max(display_min);
        let (choice, length) = match construction.variants.iter().find(|variant| f32::from(variant.advance) * scale >= size) {
            Some(variant) => (Choice::Variant(variant.glyph), f32::from(variant.advance) * scale),
            None => {
                let overlap = f32::from(min_connector_overlap(math.as_bytes()).unwrap_or(0)) * scale;
                match assemble(&construction.parts, scale, overlap, size) {
                    Some((parts, length)) => (Choice::Assembly(parts), length),
                    None => {
                        let last = construction.variants.last()?;
                        (Choice::Variant(last.glyph), f32::from(last.advance) * scale)
                    }
                }
            }
        };
        let ascent = if target_size == 0.0 { length / 2.0 + axis } else { (target_ascent - axis).max(0.0) * length / target_size + axis };
        (ascent, length - ascent, choice, length)
    } else {
        let variant = construction
            .variants
            .iter()
            .find(|variant| f32::from(variant.advance) * scale >= display_min)
            .or(construction.variants.last())?;
        let length = f32::from(variant.advance) * scale;
        let (ascent, descent) = if properties.symmetric {
            (length / 2.0 + axis, length / 2.0 - axis)
        } else {
            let (_, ink_ascent, ink_descent) = extents(variant.glyph)?;
            (ink_ascent, ink_descent)
        };
        (ascent, descent, Choice::Variant(variant.glyph), length)
    };

    // The glyphs, placed in the border box: the variant's ink centred along the axis, the
    // assembly's parts from the bottom up, each centred across the width.
    let width = match &choice {
        Choice::Variant(glyph) => advance_of(*glyph),
        Choice::Assembly(parts) => parts.iter().map(|part| advance_of(part.glyph)).fold(0.0, f32::max),
    };
    let top = extra_top;
    let glyphs = match &choice {
        Choice::Variant(glyph) => {
            let (_, ink_ascent, ink_descent) = extents(*glyph)?;
            vec![MathStretchGlyph { glyph: *glyph, x: 0.0, baseline: top + (length - (ink_ascent + ink_descent)) / 2.0 + ink_ascent }]
        }
        Choice::Assembly(parts) => {
            let bottom = top + length;
            parts
                .iter()
                .filter_map(|part| {
                    let (advance, _, ink_descent) = extents(part.glyph)?;
                    Some(MathStretchGlyph { glyph: part.glyph, x: (width - advance) / 2.0, baseline: bottom - part.start - ink_descent })
                })
                .collect()
        }
    };
    Some(Stretched {
        ascent,
        descent,
        width_delta: width - base_advance,
        paint: MathStretch { font: shaped.font.clone(), size: shaped.size, glyphs },
    })
}

/// The core `mo` of the embellished operator `id` if it stretches along the block axis.
fn vertical_core(state: &LayoutPassState<'_>, id: NodeId) -> Option<NodeId> {
    let core = underover::core_operator(state, id)?;
    let properties = properties(state, core)?;
    (properties.stretchy && !properties.horizontal).then_some(core)
}

/// Forgets the layout cache of the boxes from the operator `core` up to its embellished operator
/// `from` (a stretched layout of them must not be taken for the unstretched one, nor the reverse).
fn forget_layouts(state: &mut LayoutPassState<'_>, from: NodeId, core: NodeId) {
    let mut at = core;
    loop {
        state.nodes[at].clear_layout_cache();
        if at == from {
            break;
        }
        match state.nodes[at].parent {
            Some(parent) => at = parent,
            None => break,
        }
    }
}

/// Stretches the children of a row-like box that stretch along the block axis (see the module's
/// documentation): `metrics` are the in-flow children measured at their natural size, those of
/// the stretchy operators are measured again at the target of the row.
pub(crate) fn stretch_children(state: &mut LayoutPassState<'_>, metrics: &mut [ChildMetrics]) {
    let stretchy: Vec<(usize, NodeId)> = metrics.iter().enumerate().filter_map(|(index, child)| vertical_core(state, child.id).map(|core| (index, core))).collect();
    if stretchy.is_empty() {
        return;
    }
    // An embellished operator that is not the `mo` itself may hold a layout stretched for an
    // earlier pass: measure it unstretched.
    for &(index, core) in &stretchy {
        let id = metrics[index].id;
        if core != id {
            forget_layouts(state, id, core);
            metrics[index] = state.measure_math_child(id, Size::NONE);
        }
    }
    let ascent = metrics.iter().fold(0.0f32, |ascent, child| ascent.max(child.ascent));
    let descent = metrics.iter().fold(0.0f32, |descent, child| descent.max(child.descent));
    for &(index, core) in &stretchy {
        let id = metrics[index].id;
        if core != id {
            forget_layouts(state, id, core);
        }
        let previous = state.math_stretch.replace(StretchTarget { core, ascent, descent });
        metrics[index] = state.measure_math_child(id, Size::NONE);
        state.math_stretch = previous;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(glyph: u16, start: u16, end: u16, advance: u16, extender: bool) -> GlyphPart {
        GlyphPart { glyph, start_connector_length: start, end_connector_length: end, full_advance: advance, extender }
    }

    #[test]
    fn size_attributes() {
        // 10px font, unstretched 20, x-height 5.
        assert_eq!(size_attribute("2em", 20.0, 10.0, 5.0), Some(20.0));
        assert_eq!(size_attribute("150%", 20.0, 10.0, 5.0), Some(30.0));
        assert_eq!(size_attribute("-3em", 20.0, 10.0, 5.0), Some(0.0));
        assert_eq!(size_attribute("infinity", 20.0, 10.0, 5.0), Some(f32::INFINITY));
        assert_eq!(size_attribute("3", 20.0, 10.0, 5.0), None);
        assert_eq!(size_attribute("big", 20.0, 10.0, 5.0), None);
    }

    #[test]
    fn assembly_reaches_the_target() {
        // A bottom part, a repeatable middle and a top, 10 long each, connectors 6, overlap at least 2.
        let parts = [part(1, 0, 6, 10, false), part(2, 6, 6, 10, true), part(3, 6, 0, 10, false)];
        let (placed, length) = assemble(&parts, 1.0, 2.0, 50.0).unwrap();
        // Four extenders are the fewest whose 6 parts, 4 overlaps of at least 2 apart, reach 50.
        let glyphs: Vec<u16> = placed.iter().map(|part| part.glyph).collect();
        assert_eq!(glyphs, vec![1, 2, 2, 2, 2, 3]);
        assert!((length - 50.0).abs() < 1e-3, "{length}");
        // The first part starts at 0 and each next one before the previous ends.
        assert_eq!(placed[0].start, 0.0);
        assert!(placed.windows(2).all(|pair| pair[1].start < pair[0].start + pair[0].advance));
    }

    #[test]
    fn assembly_without_room_is_as_long_as_it_can_be() {
        // Three parts without an extender: 30 long at no overlap, at most 6 + 6 of overlap.
        let parts = [part(1, 0, 6, 10, false), part(2, 6, 6, 10, false), part(3, 6, 0, 10, false)];
        let (_, length) = assemble(&parts, 1.0, 0.0, 100.0).unwrap();
        assert_eq!(length, 30.0);
        let (_, short) = assemble(&parts, 1.0, 0.0, 20.0).unwrap();
        assert!((short - 20.0).abs() < 1e-3, "{short}");
        assert_eq!(assemble(&[], 1.0, 0.0, 10.0), None);
    }
}
