//! The types of the MathML layout classes (see the interface in [`super`]'s documentation).
#![allow(dead_code)]
use taffy::{Rect, ScrollableOverflow, Size};

use super::MathFontParameters;
use crate::NodeId;

/// The kind of table part (`mtable` and its rows and cells), for [`MathKind::Table`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TableKind {
    /// `mtable`
    Table,
    /// `mtr`
    Row,
    /// `mlabeledtr`
    LabeledRow,
    /// `mtd`
    Cell,
}

/// The layout class of a MathML element (MathML Core § 3), as `math_kind` classifies it:
/// invalid markup (a wrong number of children) is already `Row` (§ 3.1: "an element whose
/// children are invalid is laid out as an `mrow`").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MathKind {
    /// `math`, `mrow`, `mstyle`, `merror`, `mphantom`, `maction`, `semantics`, `a`, unknown
    /// elements, and any element of another class with invalid children.
    Row,
    /// `mfrac` with two children. Laid out by `fraction` (its own box class, `IS_MATH_FRACTION`).
    Fraction,
    /// `munder` with two children.
    Under,
    /// `mover` with two children.
    Over,
    /// `munderover` with three children.
    UnderOver,
    /// `msub` with two children.
    Sub,
    /// `msup` with two children.
    Sup,
    /// `msubsup` with three children.
    SubSup,
    /// `mmultiscripts` with at least one child (its `mprescripts` structure is the class's to check).
    Multiscripts,
    /// `msqrt` (any number of children, an inferred `mrow`).
    Sqrt,
    /// `mroot` with two children.
    Root,
    /// `mpadded`.
    Padded,
    /// `mtable`, `mtr`, `mlabeledtr`, `mtd`.
    Table(TableKind),
    /// `mspace`.
    Space,
    /// `mo`.
    Operator,
    /// `mi`, `mn`, `mtext`, `ms`, `mglyph`, `annotation`, `annotation-xml`: text, laid out by the
    /// inline formatter, no layout children.
    Token,
}

/// What a class's layout function is told about the box (see [`super`]'s documentation).
#[derive(Clone, Debug)]
pub(crate) struct MathContext {
    /// The element's computed font size, CSS px.
    pub(crate) font_size: f32,
    /// `math-style: normal` (display style).
    pub(crate) display_style: bool,
    /// The computed `math-depth`.
    pub(crate) math_depth: i32,
    /// `math-shift: compact` (cramped style). Always `false` until the property is computed
    /// (`mathml-2`).
    pub(crate) cramped: bool,
    /// `direction: rtl`. The layout works in the inline axis (start edge first): the driver
    /// mirrors the positions.
    pub(crate) rtl: bool,
    /// The element's primary font's MATH constants (CSS px) and x-height. `Default` (no constants)
    /// for `MathKind::Row`, which uses none; use `axis_height()` and the constants' fallbacks
    /// below for any other class.
    pub(crate) font: MathFontParameters,
    /// The box's padding and border, CSS px (physical sides), and their sum (`inset`). Not part
    /// of the content a class lays out: the driver adds them.
    pub(crate) padding: Rect<f32>,
    pub(crate) border: Rect<f32>,
    pub(crate) inset: Rect<f32>,
}

impl MathContext {
    /// The math axis height: the MATH `AxisHeight` or, without one, half the x-height (Chromium).
    pub(crate) fn axis_height(&self) -> f32 {
        self.font
            .constant(super::MathConstant::AxisHeight)
            .unwrap_or(self.font.x_height.unwrap_or(self.font_size * 0.5) / 2.0)
    }
}

/// A child box, measured at its max-content size (`LayoutPassState::measure_math_child`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct ChildMetrics {
    pub(crate) id: NodeId,
    /// Its margins (physical; percentages resolved to zero).
    pub(crate) margin: Rect<f32>,
    /// Its border box.
    pub(crate) size: Size<f32>,
    /// Its scrollable overflow, as its layout returned it.
    pub(crate) overflow: ScrollableOverflow,
    /// From the top of its margin box to its baseline (a child without a baseline has one at its
    /// bottom border edge, as Chromium's `FirstBaselineOrSynthesize`).
    pub(crate) ascent: f32,
    /// From its baseline to the bottom of its margin box.
    pub(crate) descent: f32,
    /// Its italic correction (CSS px), 0 unless its class set one (`MathLayout::italic_correction`).
    pub(crate) italic_correction: f32,
    /// Its own class, if it is a MathML box the driver built.
    pub(crate) kind: Option<MathKind>,
}

impl ChildMetrics {
    /// The width of its margin box.
    pub(crate) fn margin_box_width(&self) -> f32 {
        self.size.width + self.margin.left + self.margin.right
    }

    /// The height of its margin box (`ascent + descent`).
    pub(crate) fn margin_box_height(&self) -> f32 {
        self.size.height + self.margin.top + self.margin.bottom
    }
}

/// Where a class puts a child: the top-left corner of the child's MARGIN box in the content box,
/// the inline offset from the content box's start edge (left for `ltr`, right for `rtl`: the
/// driver mirrors it for `rtl`, so a class never looks at `ctx.rtl` to place along the inline
/// axis, only to choose which child comes first).
#[derive(Clone, Copy, Debug)]
pub(crate) struct ChildPlacement {
    /// The child's metrics: the ones measured, or ones a class measured again (a stretched
    /// operator measured at a known size: `measure_math_child_with`).
    pub(crate) metrics: ChildMetrics,
    pub(crate) inline_offset: f32,
    pub(crate) block_offset: f32,
}

/// What a class's layout function returns: the content (inside padding and border), from which
/// the driver makes the box.
#[derive(Clone, Debug, Default)]
pub(crate) struct MathLayout {
    /// The width of the content (the box's content width unless `width` or a known width says
    /// otherwise).
    pub(crate) content_width: f32,
    /// From the top of the content to its baseline, and from its baseline to its bottom. The box's
    /// first and last baseline are `inset.top + ascent`.
    pub(crate) ascent: f32,
    pub(crate) descent: f32,
    /// Every in-flow child's placement (the driver sets their layouts: a child missing from the
    /// list gets none, so list them all), in layout-children order.
    pub(crate) children: Vec<ChildPlacement>,
    /// The box's italic correction (CSS px), kept for a parent that reads it from the child's
    /// `ChildMetrics`.
    pub(crate) italic_correction: f32,
}

// The MATH table's glyph tables (OpenType § MATH): italic corrections, top accent attachments and
// the size variants and assembly of a stretchy glyph. Reads of the table's bytes, in font units;
// every function is `None` for a glyph or table the font does not have (or a table that is cut).

/// A big-endian reader over a table's bytes.
#[derive(Clone, Copy)]
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn u16(self, at: usize) -> Option<u16> {
        self.0.get(at..at + 2).map(|b| u16::from_be_bytes([b[0], b[1]]))
    }

    fn i16(self, at: usize) -> Option<i16> {
        self.u16(at).map(|v| v as i16)
    }

    /// The table `offset` bytes into this one, where `offset` is the 16-bit offset stored at `at`
    /// (0 is no table).
    fn table_at(self, at: usize, base: usize) -> Option<Reader<'a>> {
        let offset = self.u16(at)? as usize;
        (offset != 0).then_some(())?;
        self.0.get(base + offset..).map(Reader)
    }
}

/// The index of `glyph` in the Coverage table `coverage` (formats 1 and 2).
fn coverage_index(coverage: Reader<'_>, glyph: u16) -> Option<usize> {
    match coverage.u16(0)? {
        1 => {
            let count = coverage.u16(2)? as usize;
            (0..count).find(|&index| coverage.u16(4 + 2 * index) == Some(glyph))
        }
        2 => {
            let count = coverage.u16(2)? as usize;
            (0..count).find_map(|index| {
                let record = 4 + 6 * index;
                let (start, end, first) = (coverage.u16(record)?, coverage.u16(record + 2)?, coverage.u16(record + 4)?);
                (start..=end).contains(&glyph).then(|| first as usize + (glyph - start) as usize)
            })
        }
        _ => None,
    }
}

/// The value of `glyph` in a table of a Coverage offset, a count and `MathValueRecord`s (the
/// italic corrections and the top accent attachments).
fn glyph_value_record(table: Reader<'_>, glyph: u16) -> Option<i16> {
    let coverage = table.table_at(0, 0)?;
    let index = coverage_index(coverage, glyph)?;
    let count = table.u16(2)? as usize;
    (index < count).then_some(())?;
    // Each record: a value and a device table offset.
    table.i16(4 + 4 * index)
}

/// The `MathGlyphInfo` table of the MATH table `math`.
fn glyph_info(math: Reader<'_>) -> Option<Reader<'_>> {
    math.table_at(6, 0)
}

/// The italic correction of `glyph`, font units (`MathItalicsCorrectionInfo`).
pub(crate) fn italic_correction(math: &[u8], glyph: u16) -> Option<i16> {
    let math = Reader(math);
    let info = glyph_info(math)?;
    glyph_value_record(info.table_at(0, 0)?, glyph)
}

/// The horizontal position of the top accent attachment point of `glyph`, font units
/// (`MathTopAccentAttachment`).
pub(crate) fn top_accent_attachment(math: &[u8], glyph: u16) -> Option<i16> {
    let math = Reader(math);
    let info = glyph_info(math)?;
    glyph_value_record(info.table_at(2, 0)?, glyph)
}

/// Whether `glyph` is an extended shape (`ExtendedShapeCoverage`).
pub(crate) fn is_extended_shape(math: &[u8], glyph: u16) -> bool {
    let math = Reader(math);
    glyph_info(math)
        .and_then(|info| info.table_at(4, 0))
        .is_some_and(|coverage| coverage_index(coverage, glyph).is_some())
}

/// A size variant of a glyph: the glyph, and its advance along the stretch axis, font units.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct GlyphVariant {
    pub(crate) glyph: u16,
    pub(crate) advance: u16,
}

/// A part of a glyph assembly (`GlyphPartRecord`), font units.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct GlyphPart {
    pub(crate) glyph: u16,
    pub(crate) start_connector_length: u16,
    pub(crate) end_connector_length: u16,
    pub(crate) full_advance: u16,
    /// `PartFlags` bit 0: an extender, repeatable.
    pub(crate) extender: bool,
}

/// The variants and the assembly of a glyph in one direction (`MathGlyphConstruction`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct GlyphConstruction {
    /// Increasing in size (the font's order), the glyph itself first by convention.
    pub(crate) variants: Vec<GlyphVariant>,
    /// The assembly's parts, from the start (bottom or left) to the end, and its italic
    /// correction; empty if the glyph has none.
    pub(crate) parts: Vec<GlyphPart>,
    pub(crate) assembly_italic_correction: i16,
}

/// The `MathVariants` table's minimum connector overlap, font units.
pub(crate) fn min_connector_overlap(math: &[u8]) -> Option<u16> {
    let math = Reader(math);
    math.table_at(8, 0)?.u16(0)
}

/// The size variants and glyph assembly of `glyph` (`MathVariants`): the vertical ones, or the
/// horizontal ones if `horizontal`.
pub(crate) fn glyph_construction(math: &[u8], glyph: u16, horizontal: bool) -> Option<GlyphConstruction> {
    let math = Reader(math);
    let variants = math.table_at(8, 0)?;
    // minConnectorOverlap, vertGlyphCoverage, horizGlyphCoverage, vertGlyphCount, horizGlyphCount,
    // then the vertical and the horizontal constructions' offsets.
    let (coverage_at, count_at) = if horizontal { (4, 8) } else { (2, 6) };
    let coverage = variants.table_at(coverage_at, 0)?;
    let index = coverage_index(coverage, glyph)?;
    let count = variants.u16(count_at)? as usize;
    (index < count).then_some(())?;
    let constructions_at = 10 + if horizontal { 2 * variants.u16(6)? as usize } else { 0 };
    let construction_offset = variants.u16(constructions_at + 2 * index)? as usize;
    (construction_offset != 0).then_some(())?;
    let construction = Reader(variants.0.get(construction_offset..)?);
    // glyphAssemblyOffset (from the construction), variantCount, then the variant records.
    let variant_count = construction.u16(2)? as usize;
    let mut result = GlyphConstruction::default();
    for variant in 0..variant_count {
        let at = 4 + 4 * variant;
        result.variants.push(GlyphVariant { glyph: construction.u16(at)?, advance: construction.u16(at + 2)? });
    }
    if let Some(assembly) = construction.table_at(0, 0) {
        // italicsCorrection (a MathValueRecord), partCount, then 10-byte part records.
        result.assembly_italic_correction = assembly.i16(0)?;
        let part_count = assembly.u16(4)? as usize;
        for part in 0..part_count {
            let at = 6 + 10 * part;
            result.parts.push(GlyphPart {
                glyph: assembly.u16(at)?,
                start_connector_length: assembly.u16(at + 2)?,
                end_connector_length: assembly.u16(at + 4)?,
                full_advance: assembly.u16(at + 6)?,
                extender: assembly.u16(at + 8)? & 1 != 0,
            });
        }
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u16s(values: &[u16]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_be_bytes()).collect()
    }

    #[test]
    fn coverage_formats() {
        // Format 1: glyphs 5, 9, 12.
        let format1 = u16s(&[1, 3, 5, 9, 12]);
        assert_eq!(coverage_index(Reader(&format1), 9), Some(1));
        assert_eq!(coverage_index(Reader(&format1), 10), None);
        // Format 2: ranges 3..=5 (indices 0..), 10..=11 (starting at index 3).
        let format2 = u16s(&[2, 2, 3, 5, 0, 10, 11, 3]);
        assert_eq!(coverage_index(Reader(&format2), 4), Some(1));
        assert_eq!(coverage_index(Reader(&format2), 11), Some(4));
        assert_eq!(coverage_index(Reader(&format2), 6), None);
    }

    #[test]
    fn italic_correction_lookup() {
        // MATH header (version 1.0, constants 0, glyph info at 10, variants 0), then MathGlyphInfo
        // at 10 (italics correction info at +8), whose table holds a coverage (at +8: glyph 7) and
        // one record, 33.
        let mut table = u16s(&[1, 0, 0, 10, 0]);
        table.extend(u16s(&[8, 0, 0, 0]));
        table.extend(u16s(&[8, 1]));
        table.extend(33i16.to_be_bytes());
        table.extend(u16s(&[0]));
        table.extend(u16s(&[1, 1, 7]));
        assert_eq!(italic_correction(&table, 7), Some(33));
        assert_eq!(italic_correction(&table, 8), None);
        assert_eq!(top_accent_attachment(&table, 7), None);
    }

    #[test]
    fn variants_and_assembly() {
        // MATH header with the variants at 10; MathVariants: overlap 4, vertical coverage at +12
        // (glyph 2), no horizontal coverage, one vertical construction (at +20), none horizontal.
        let mut table = u16s(&[1, 0, 0, 0, 10]);
        let mut variants = u16s(&[4, 12, 0, 1, 0, 20]);
        variants.extend(u16s(&[1, 1, 2]));
        variants.resize(20, 0);
        // The construction: assembly at +12, two variants (glyph 2 advance 10, glyph 40 advance 30).
        variants.extend(u16s(&[12, 2, 2, 10, 40, 30]));
        // The assembly: italics correction 5, one part: glyph 50, connectors 1 and 2, advance 9,
        // extender.
        variants.extend(u16s(&[5, 0, 1, 50, 1, 2, 9, 1]));
        table.extend(variants);
        assert_eq!(min_connector_overlap(&table), Some(4));
        let found = glyph_construction(&table, 2, false).unwrap();
        assert_eq!(found.variants, vec![GlyphVariant { glyph: 2, advance: 10 }, GlyphVariant { glyph: 40, advance: 30 }]);
        assert_eq!(found.assembly_italic_correction, 5);
        assert_eq!(
            found.parts,
            vec![GlyphPart { glyph: 50, start_connector_length: 1, end_connector_length: 2, full_advance: 9, extender: true }]
        );
        assert_eq!(glyph_construction(&table, 3, false), None);
        assert_eq!(glyph_construction(&table, 2, true), None);
    }
}
