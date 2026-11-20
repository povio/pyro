// Copyright 2026 the Parley Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Data structures to store the result of shaping.

use core::ops::Range;

use alloc::vec::Vec;
use parlance::{BidiLevel, NormalizedCoord};
use skrifa::MetadataProvider;

use crate::{
    CharInfo, FontInstance, Glyph,
    itemize::{Segment, TextRange},
};

use super::{
    Character, CharacterFlags, ShapedCluster, ShapedClusterFlags, Whitespace, atom::ShapedSlice,
    shaper::ShapeOptions,
};

/// Metrics that apply to the glyphs of a font.
///
/// These metrics are scaled by the font size.
///
/// The `underline_*` and `strikethrough_*` fields are derived if not set by the font.
///
// TODO: Perhaps it'd be nicer if we exposed unscaled metrics (design units) instead. This currently
// just follows what `parley` used to do in its `RunMetrics`. If we go unscaled, we should then
// either also store units per em, or em-normalize the values like CSS does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FontMetrics {
    /// Distance from the baseline to the top of the alignment box.
    pub ascent: f32,
    /// Distance from the baseline to the bottom of the alignment box.
    pub descent: f32,
    /// Recommended additional spacing between lines.
    pub leading: f32,
    /// Offset of the top of underline decoration from the baseline.
    pub underline_offset: f32,
    /// Thickness of the underline decoration.
    pub underline_size: f32,
    /// Offset of the top of strikethrough decoration from the baseline.
    pub strikethrough_offset: f32,
    /// Thickness of the strikethrough decoration.
    pub strikethrough_size: f32,
    /// Distance from the baseline to the top of a typical English capital.
    pub cap_height: Option<f32>,
    /// Distance from the baseline to the top of the lowercase "x" or
    /// similar character.
    pub x_height: Option<f32>,
}

impl FontMetrics {
    /// pyro: the metrics at `scale` times the font size (CSS `text-fit`).
    pub fn scaled(&self, scale: f32) -> Self {
        Self {
            ascent: self.ascent * scale,
            descent: self.descent * scale,
            leading: self.leading * scale,
            underline_offset: self.underline_offset * scale,
            underline_size: self.underline_size * scale,
            strikethrough_offset: self.strikethrough_offset * scale,
            strikethrough_size: self.strikethrough_size * scale,
            cap_height: self.cap_height.map(|v| v * scale),
            x_height: self.x_height.map(|v| v * scale),
        }
    }
}

/// pyro: the ideographic baselines of a font's `BASE` table (horizontal axis), scaled to a font
/// size, as coordinates above the alphabetic baseline: `ideo`, `idtp`, `icfb` and `icft`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct IdeographicBaselines {
    /// `ideo`: the bottom of the ideographic em box.
    pub ideographic_under: Option<f32>,
    /// `idtp`: the top of the ideographic em box.
    pub ideographic_over: Option<f32>,
    /// `icfb`: the bottom of the ideographic character face.
    pub face_under: Option<f32>,
    /// `icft`: the top of the ideographic character face.
    pub face_over: Option<f32>,
    /// pyro: `romn`: the alphabetic baseline (most fonts: 0, the glyph origin).
    pub alphabetic: Option<f32>,
    /// pyro: `hang`: the hanging baseline.
    pub hanging: Option<f32>,
    /// pyro: `math`: the mathematical baseline.
    pub math: Option<f32>,
}

impl IdeographicBaselines {
    /// Reads them from `font`; `None` for a font without a horizontal `BASE` axis.
    pub fn from_font_instance(font: &FontInstance, font_size: f32) -> Option<Self> {
        use skrifa::raw::TableProvider;
        use skrifa::raw::tables::base::BaseCoord;
        let font_ref =
            skrifa::FontRef::from_index(font.font.data.as_ref(), font.font.index).ok()?;
        let units_per_em = f32::from(font_ref.head().ok()?.units_per_em());
        let axis = font_ref.base().ok()?.horiz_axis()?.ok()?;
        let tags = axis.base_tag_list()?.ok()?;
        let scripts = axis.base_script_list().ok()?;
        let records = scripts.base_script_records();
        // The default script's values, else the first script's.
        let record = records
            .iter()
            .find(|record| record.base_script_tag() == skrifa::Tag::new(b"DFLT"))
            .or_else(|| records.first())?;
        let values = record.base_script(scripts.offset_data()).ok()?.base_values()?.ok()?;
        let coords = values.base_coords();
        let mut out = Self::default();
        for (index, tag) in tags.baseline_tags().iter().enumerate() {
            let Ok(coord) = coords.get(index) else { continue };
            let coord = match coord {
                BaseCoord::Format1(c) => c.coordinate(),
                BaseCoord::Format2(c) => c.coordinate(),
                BaseCoord::Format3(c) => c.coordinate(),
            };
            let value = Some(f32::from(coord) * font_size / units_per_em);
            match &tag.get().to_be_bytes() {
                b"ideo" => out.ideographic_under = value,
                b"idtp" => out.ideographic_over = value,
                b"icfb" => out.face_under = value,
                b"icft" => out.face_over = value,
                b"romn" => out.alphabetic = value,
                b"hang" => out.hanging = value,
                b"math" => out.math = value,
                _ => {}
            }
        }
        Some(out)
    }
}

impl FontMetrics {
    /// Metrics to use when no font is available at all.
    ///
    /// These roughly match common Latin fonts, so that a layout without fonts still has plausible
    /// line heights.
    pub const fn fallback(font_size: f32) -> Self {
        Self {
            ascent: font_size * 0.8,
            descent: font_size * 0.2,
            leading: 0.,
            underline_offset: 0.,
            underline_size: 0.,
            strikethrough_offset: 0.,
            strikethrough_size: 0.,
            cap_height: None,
            x_height: None,
        }
    }

    /// Compute the metrics of `font` at `font_size`, positioned at the variable font location
    /// given by the `synthesis` and `variations` axis settings.
    ///
    /// Returns `None` if the font data cannot be parsed.
    pub fn from_font_instance(
        font: &FontInstance,
        font_size: f32,
        variations: &[parlance::FontVariation],
    ) -> Option<Self> {
        let font_ref =
            skrifa::FontRef::from_index(font.font.data.as_ref(), font.font.index).ok()?;
        let location = font_ref.axes().location(
            font.synthesis
                .variation_settings()
                .iter()
                .map(|(tag, value)| (skrifa::Tag::new(&tag.to_be_bytes()), *value))
                .chain(
                    variations
                        .iter()
                        .map(|v| (skrifa::Tag::new(&v.tag.to_bytes()), v.value)),
                ),
        );
        // pyro: at the size the font is used at (`size-adjust`), with the metrics its `@font-face`
        // rule overrides.
        let font_size = font_size * font.synthesis.size_adjust();
        Some(
            Self::from_skrifa_metrics(
                &font_ref,
                &skrifa::metrics::Metrics::new(
                    &font_ref,
                    skrifa::prelude::Size::new(font_size),
                    &location,
                ),
                font_size,
            )
            .with_face_metrics(font.synthesis.face_metrics(), font_size),
        )
    }

    /// pyro: the metrics with those a `@font-face` rule overrides (`ascent-override`,
    /// `descent-override`, `line-gap-override`, fractions of `font_size`).
    pub fn with_face_metrics(mut self, face: &fontique::FaceMetrics, font_size: f32) -> Self {
        if let Some(ascent) = face.ascent {
            self.ascent = ascent * font_size;
        }
        if let Some(descent) = face.descent {
            self.descent = descent * font_size;
        }
        if let Some(line_gap) = face.line_gap {
            self.leading = line_gap * font_size;
        }
        self
    }

    fn from_skrifa_metrics(
        font_ref: &skrifa::FontRef<'_>,
        metrics: &skrifa::metrics::Metrics,
        font_size: f32,
    ) -> Self {
        // Default values from HarfBuzz: https://github.com/harfbuzz/harfbuzz/blob/12ee77ee00514c442bcd28f9aa9d56b708c37382/src/hb-ot-metrics.cc#L355-L372
        let fallback_decoration_size = font_size / 18.0;

        let (underline_offset, underline_size) = if let Some(underline) = metrics.underline {
            (underline.offset, underline.thickness)
        } else {
            (-fallback_decoration_size, fallback_decoration_size)
        };
        let (strikethrough_offset, strikethrough_size) = if let Some(strikeout) = metrics.strikeout
        {
            (strikeout.offset, strikeout.thickness)
        } else {
            (metrics.ascent / 2.0, fallback_decoration_size)
        };

        Self {
            ascent: metrics.ascent + chromium_ascent_adjustment(font_ref, metrics),
            descent: -metrics.descent,
            leading: metrics.leading,
            underline_offset,
            underline_size,
            strikethrough_offset,
            strikethrough_size,
            x_height: metrics.x_height,
            cap_height: metrics.cap_height,
        }
    }
}

/// pyro: WebKit's and Chromium's (macOS) extra ascent for Times,
/// Helvetica and Courier, "to closely match the vertical metrics of their
/// Microsoft counterparts": 15% of the rounded ascent plus descent, rounded
/// (16px Helvetica: 2px, a 18px normal line instead of 16px). These are the
/// macOS default `serif`, `sans-serif` and `monospace` fonts.
///
/// Decided from the font's own family name every time. There is deliberately no
/// per-font memo: one keyed by the font data's address gave a font allocated
/// where a freed one used to live the freed font's answer, so line heights
/// depended on what the thread had laid out before. The callers already run once
/// per font, size and variation location ([`RunMetrics`] lives in the glyph
/// cache), and the name lookup allocates nothing.
fn chromium_ascent_adjustment(font_ref: &skrifa::FontRef<'_>, metrics: &skrifa::metrics::Metrics) -> f32 {
    if !is_chromium_adjusted_family(font_ref) {
        return 0.0;
    }
    let height = metrics.ascent.round() + (-metrics.descent).round();
    (height * 0.15 + 0.5).floor()
}

/// Whether the font's (English, else first) family name is exactly Times,
/// Helvetica or Courier.
fn is_chromium_adjusted_family(font_ref: &skrifa::FontRef<'_>) -> bool {
    use skrifa::MetadataProvider;
    let Some(family) = font_ref
        .localized_strings(skrifa::string::StringId::FAMILY_NAME)
        .english_or_first()
    else {
        return false;
    };
    ["Times", "Helvetica", "Courier"]
        .iter()
        .any(|name| family.chars().eq(name.chars()))
}

/// pyro: the font metrics of a shaped run, which depend on the font instance, the font
/// size and the variation location only. Computed once per such combination in a
/// [`ShapedText`] (see [`ShapedText::push_run`]) and kept with the run's glyphs in the shaper's
/// glyph cache, so a run shaped before costs no font parse.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RunMetrics {
    /// The font's units per em, which scale glyph advances to the font size.
    pub(crate) units_per_em: f32,
    pub(crate) font_metrics: FontMetrics,
    /// pyro: the font's hyphen (U+2010, else U+002D): its glyph and advance at the font size.
    pub(crate) hyphen: Option<(u32, f32)>,
}

impl RunMetrics {
    /// The metrics of `font` at `font_size` and the normalized `coords`.
    ///
    /// # Panics
    ///
    /// Panics if the font data cannot be parsed (the shaper has parsed it already).
    pub(crate) fn new(
        font: &FontInstance,
        font_size: f32,
        coords: &[harfrust::NormalizedCoord],
    ) -> Self {
        let font_ref =
            skrifa::FontRef::from_index(font.font.data.as_ref(), font.font.index).unwrap();
        let metrics = skrifa::metrics::Metrics::new(
            &font_ref,
            skrifa::prelude::Size::new(font_size),
            coords,
        );
        let hyphen = {
            use skrifa::MetadataProvider as _;
            let charmap = font_ref.charmap();
            let glyph_metrics = skrifa::metrics::GlyphMetrics::new(
                &font_ref,
                skrifa::prelude::Size::unscaled(),
                coords,
            );
            charmap
                .map('\u{2010}')
                .or_else(|| charmap.map('-'))
                .and_then(|glyph| {
                    let advance = glyph_metrics.advance_width(glyph)?;
                    (metrics.units_per_em != 0).then(|| {
                        (glyph.to_u32(), advance * (font_size / metrics.units_per_em as f32))
                    })
                })
        };
        Self {
            units_per_em: metrics.units_per_em as f32,
            // pyro: `font_size` is the size the font is used at (`size-adjust` applied by the
            // shaper); its `@font-face` rule may override metrics.
            font_metrics: FontMetrics::from_skrifa_metrics(&font_ref, &metrics, font_size)
                .with_face_metrics(font.synthesis.face_metrics(), font_size),
            hyphen,
        }
    }
}

/// The metrics of the runs of one font at one size and variation location in a [`ShapedText`].
#[derive(Clone, Debug, PartialEq)]
struct RunMetricsEntry {
    /// Index into [`ShapedText::fonts`].
    font_index: usize,
    /// The font size's bits.
    font_size: u32,
    /// Range into [`ShapedText::normalized_coords`].
    normalized_coords_range: Range<usize>,
    metrics: RunMetrics,
}

/// pyro: a shaped cluster's glyphs before [`ShapedText::show_hyphen`] changed them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClusterGlyphs {
    glyph_offset: u32,
    flags: u16,
    advance: f32,
}

/// The result of shaping.
///
/// After [analyzing][crate::Analysis] your text, [shape the text][crate::Shaper::shape_text],
/// writing the result into this [`ShapedText`]. This then holds your shaped paragraph of text.
///
/// This shaped text holds spans of the source text's characters that were shaped into
/// [`ShapedCluster`]s. Note that the boundaries of shaped clusters and graphemes need not coincide;
/// see [`ShapedCluster`]s documentation for more information.
///
/// Use [`Self::run_slice`] to get a run's [`ShapedSlice`]. From there, you can interact with slices
/// of shaped text, such as walking [atoms](crate::Atom) (e.g., [`ShapedSlice::atoms_start`]) and
/// [graphemes](crate::Grapheme) (e.g., [`ShapedSlice::graphemes_start`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ShapedText {
    runs: Vec<ShapedRun>,
    characters: Vec<Character>,
    shaped_clusters: Vec<ShapedCluster>,
    glyphs: Vec<Glyph>,
    fonts: Vec<FontInstance>,
    normalized_coords: Vec<NormalizedCoord>,
    /// pyro: the metrics of the runs' fonts, one entry per font, size and variation
    /// location; a run of a combination seen before takes its entry.
    run_metrics: Vec<RunMetricsEntry>,
}

impl ShapedText {
    /// Create a reusable [`ShapedText`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Reserve capacity for at least `additional_chars` more characters.
    ///
    /// This speculatively reserves capacity for `additional_chars` more clusters and glyphs.
    #[inline]
    pub(crate) fn reserve(&mut self, additional_chars: usize) {
        self.characters.reserve(additional_chars);
        self.shaped_clusters.reserve(additional_chars);
        self.glyphs.reserve(additional_chars);
    }

    /// Clear the result while retaining capacity.
    #[inline]
    pub fn clear(&mut self) {
        self.runs.clear();
        self.characters.clear();
        self.shaped_clusters.clear();
        self.glyphs.clear();
        self.fonts.clear();
        self.normalized_coords.clear();
        self.run_metrics.clear();
    }

    /// Get a [`ShapedSlice`] of the run at `run_index`.
    pub fn run_slice(&self, run_index: u32) -> ShapedSlice<'_> {
        let run = &self.runs[run_index as usize];
        ShapedSlice {
            characters: &self.characters,

            shaped_clusters: &self.shaped_clusters,
            glyphs: &self.glyphs,

            clusters: (
                run.shaped_clusters_range.start,
                run.shaped_clusters_range.end,
            ),
        }
    }

    /// The shaped runs.
    ///
    /// Each run contains glyphs that can be rendered with a single font.
    #[inline(always)]
    pub fn runs(&self) -> &[ShapedRun] {
        &self.runs
    }

    /// Escape hatch for `parley` tests that remap style indices to check whether layouts are
    /// otherwise identical. This is not for public consumption!
    #[doc(hidden)]
    #[inline(always)]
    pub fn remap_styles(&mut self, remap: &[u16]) {
        for character in self.characters.iter_mut() {
            character.style_index = remap[character.style_index as usize];
        }
        for shaped_cluster in self.shaped_clusters.iter_mut() {
            shaped_cluster.style_index = remap[shaped_cluster.style_index as usize];
        }
    }

    /// pyro: sets the advance of the shaped cluster at `index` (a tab, once the line breaker knows
    /// the tab stop it reaches). A cluster whose glyphs are stored takes the difference on its
    /// last glyph; an inline glyph's advance is its cluster's.
    #[inline]
    /// pyro: the vertical advance of an upright glyph of the font at `font_index` without
    /// vertical metrics (no `vmtx` table) at `font_size`: its height, ascent + descent as the
    /// font gives them (HarfBuzz's default vertical advance; no `chromium_ascent_adjustment`).
    /// `None` for a font with vertical metrics.
    pub fn default_vertical_advance(&self, font_index: usize, font_size: f32) -> Option<f32> {
        use skrifa::raw::TableProvider;
        let font = self.fonts.get(font_index)?;
        let font_ref = skrifa::FontRef::from_index(font.font.data.as_ref(), font.font.index).ok()?;
        if font_ref.vmtx().is_ok() {
            return None;
        }
        let metrics = skrifa::metrics::Metrics::new(
            &font_ref,
            skrifa::instance::Size::new(font_size),
            skrifa::instance::LocationRef::default(),
        );
        Some(metrics.ascent - metrics.descent)
    }

    /// pyro: the vertical advance of the shaped cluster at `index` standing upright in the font
    /// at `font_index` at `font_size`: the sum of its glyphs' advance heights in the font's
    /// vertical metrics (`vmtx`), as HarfBuzz advances glyphs shaping top to bottom. `None` for a
    /// font without them.
    pub fn cluster_vertical_advance(&self, index: u32, font_index: usize, font_size: f32) -> Option<f32> {
        use skrifa::raw::TableProvider;
        let cluster = self.shaped_clusters.get(index as usize)?;
        let font = self.fonts.get(font_index)?;
        let font_ref = skrifa::FontRef::from_index(font.font.data.as_ref(), font.font.index).ok()?;
        let vmtx = font_ref.vmtx().ok()?;
        let units_per_em = f32::from(font_ref.head().ok()?.units_per_em());
        if units_per_em == 0.0 {
            return None;
        }
        let advance = |glyph: u32| vmtx.advance(skrifa::GlyphId::new(glyph)).map(f32::from);
        let units = if cluster.has_inline_glyph() {
            advance(cluster.glyph_offset)?
        } else {
            let start = cluster.glyph_offset as usize;
            let glyphs = self.glyphs.get(start..start + usize::from(cluster.glyph_len()))?;
            let mut sum = 0.0;
            for glyph in glyphs {
                sum += advance(glyph.id)?;
            }
            sum
        };
        Some(units * font_size / units_per_em)
    }

    /// pyro: makes the run at `run_index` upright with glyphs advancing by `advance`
    /// ([`ShapedRun::upright_advance`]), its hyphen too.
    pub fn set_run_upright_advance(&mut self, run_index: usize, advance: f32) {
        if let Some(run) = self.runs.get_mut(run_index) {
            run.upright_advance = Some(advance);
            run.hyphen = run.hyphen.map(|(glyph, _)| (glyph, advance));
        }
    }

    pub fn set_cluster_advance(&mut self, index: u32, advance: f32) {
        let Some(cluster) = self.shaped_clusters.get_mut(index as usize) else {
            return;
        };
        let delta = advance - cluster.advance;
        cluster.advance = advance;
        if !cluster.has_inline_glyph() && cluster.glyph_len() > 0 {
            let last = cluster.glyph_offset as usize + usize::from(cluster.glyph_len()) - 1;
            if let Some(glyph) = self.glyphs.get_mut(last) {
                glyph.advance += delta;
            }
        }
    }

    /// pyro: moves the one glyph of the shaped cluster at `index` by `dx` and changes its
    /// advance by `d_advance` (fullwidth punctuation set half-width or back at a line's edge,
    /// CSS `text-spacing-trim`), and returns the glyph's x offset and the cluster's advance before.
    /// `None`, changing nothing, for a cluster of other than one glyph, or whose glyph is stored
    /// inline when `dx` is not 0.
    pub fn shift_cluster(&mut self, index: u32, dx: f32, d_advance: f32) -> Option<(f32, f32)> {
        let cluster = self.shaped_clusters.get_mut(index as usize)?;
        if cluster.glyph_len() != 1 || (cluster.has_inline_glyph() && dx != 0.0) {
            return None;
        }
        let advance = cluster.advance;
        cluster.advance += d_advance;
        if cluster.has_inline_glyph() {
            return Some((0.0, advance));
        }
        let glyph = self.glyphs.get_mut(cluster.glyph_offset as usize)?;
        let x = glyph.x;
        glyph.x += dx;
        glyph.advance += d_advance;
        Some((x, advance))
    }

    /// pyro: sets the justification opportunities of the character at `index` (see
    /// [`CharacterFlags::with_justification`](super::CharacterFlags)).
    #[inline]
    pub fn set_character_justification(&mut self, index: usize, none: bool, before: bool, after: bool) {
        if let Some(character) = self.characters.get_mut(index) {
            character.flags = character.flags.with_justification(none, before, after);
        }
    }

    /// pyro: gives the one glyph of the shaped cluster at `index` back the x offset and the
    /// cluster back the advance [`Self::shift_cluster`] returned.
    pub fn unshift_cluster(&mut self, index: u32, x: f32, advance: f32) {
        let Some(cluster) = self.shaped_clusters.get_mut(index as usize) else {
            return;
        };
        let d_advance = advance - cluster.advance;
        cluster.advance = advance;
        if !cluster.has_inline_glyph()
            && cluster.glyph_len() == 1
            && let Some(glyph) = self.glyphs.get_mut(cluster.glyph_offset as usize)
        {
            glyph.x = x;
            glyph.advance += d_advance;
        }
    }

    /// pyro: the legacy `kern` pair between the shaped clusters `left` and `right` (visually
    /// adjacent, `left` first, in one run): HarfBuzz splits a pair's value between the two
    /// glyphs, `k1` on the first one's advance and `k2` on the second one's offset and advance.
    /// `Some((k1, k2))` for single-glyph clusters of a font with a `kern` table and no `GPOS`
    /// whose pair kerned (Blink shapes a line's edges again where shaping was not safe to break,
    /// so a line break leaves no pair across it).
    pub fn legacy_kern_across(&self, left: u32, right: u32) -> Option<(f32, f32)> {
        use skrifa::raw::TopLevelTable as _;
        let run = self.runs.iter().find(|run| {
            run.shaped_clusters_range.contains(&left) && run.shaped_clusters_range.contains(&right)
        })?;
        let glyph = |index: u32| -> Option<(u32, f32, f32)> {
            let cluster = self.shaped_clusters.get(index as usize)?;
            if cluster.glyph_len() != 1 {
                return None;
            }
            if cluster.has_inline_glyph() {
                return Some((cluster.glyph_offset, 0.0, cluster.advance));
            }
            let glyph = self.glyphs.get(cluster.glyph_offset as usize)?;
            Some((glyph.id, glyph.x, glyph.advance))
        };
        let (right_x, (left_id, left_x, left_advance)) = (glyph(right)?.1, glyph(left)?);
        if right_x == 0.0 {
            return None;
        }
        let font = self.fonts.get(run.font_index)?;
        let font_ref = skrifa::FontRef::from_index(font.font.data.as_ref(), font.font.index).ok()?;
        if font_ref.table_data(skrifa::raw::types::Tag::new(b"kern")).is_none()
            || font_ref.table_data(skrifa::raw::tables::gpos::Gpos::TAG).is_some()
        {
            return None;
        }
        let coords: alloc::vec::Vec<skrifa::instance::NormalizedCoord> = self.normalized_coords
            [run.normalized_coords_range.clone()]
        .iter()
        .map(|coord| skrifa::instance::NormalizedCoord::from_bits(coord.to_bits()))
        .collect();
        let metrics =
            skrifa::metrics::GlyphMetrics::new(&font_ref, skrifa::prelude::Size::unscaled(), coords.as_slice());
        let units_per_em = { use skrifa::raw::TableProvider as _; font_ref.head().ok()?.units_per_em() };
        (units_per_em != 0).then_some(())?;
        let base = metrics.advance_width(skrifa::GlyphId::new(left_id))? * (run.font_size / f32::from(units_per_em));
        Some((left_advance - base - left_x, right_x))
    }

    /// pyro: the bidi level of the run holding the shaped cluster at `index`.
    pub fn cluster_is_rtl(&self, index: u32) -> Option<bool> {
        self.runs
            .iter()
            .find(|run| run.shaped_clusters_range.contains(&index))
            .map(|run| run.bidi_level.is_rtl())
    }

    /// pyro: the glyph of `ch` in the font of the run at `run_index` and its advance at the run's
    /// size, if the font has the character (a `hyphenate-character` other than the font's hyphen).
    pub fn glyph_of(&self, run_index: usize, ch: char) -> Option<(u32, f32)> {
        use skrifa::MetadataProvider as _;
        let run = self.runs.get(run_index)?;
        let font = self.fonts.get(run.font_index)?;
        let font_ref = skrifa::FontRef::from_index(font.font.data.as_ref(), font.font.index).ok()?;
        let glyph = font_ref.charmap().map(ch)?;
        if let Some(advance) = run.upright_advance {
            return Some((glyph.to_u32(), advance));
        }
        // The same coords, in skrifa's type.
        let coords: alloc::vec::Vec<skrifa::instance::NormalizedCoord> = self.normalized_coords
            [run.normalized_coords_range.clone()]
        .iter()
        .map(|coord| skrifa::instance::NormalizedCoord::from_bits(coord.to_bits()))
        .collect();
        let metrics =
            skrifa::metrics::GlyphMetrics::new(&font_ref, skrifa::prelude::Size::unscaled(), coords.as_slice());
        let units_per_em = { use skrifa::raw::TableProvider as _; font_ref.head().ok()?.units_per_em() };
        (units_per_em != 0).then_some(())?;
        Some((glyph.to_u32(), metrics.advance_width(glyph)? * (run.font_size / f32::from(units_per_em))))
    }

    /// pyro: shows a hyphen on the shaped cluster at `index`: its glyphs become `hyphen`'s (glyph,
    /// advance) pairs, after its own glyphs when `keep` (a soft hyphen that shaping merged into the
    /// cluster before it), in visual order for `rtl` text. The new glyphs go to the end of the
    /// glyph array. Returns what the cluster had, for [`Self::restore_cluster`].
    pub fn show_hyphen(&mut self, index: u32, hyphen: &[(u32, f32)], keep: bool, rtl: bool) -> Option<ClusterGlyphs> {
        let cluster = *self.shaped_clusters.get(index as usize)?;
        let saved = ClusterGlyphs {
            glyph_offset: cluster.glyph_offset,
            flags: cluster.flags.bits(),
            advance: cluster.advance,
        };
        let mut own = alloc::vec::Vec::new();
        if keep {
            if cluster.has_inline_glyph() {
                own.push(Glyph { id: cluster.glyph_offset, x: 0., y: 0., advance: cluster.advance });
            } else {
                let start = cluster.glyph_offset as usize;
                own.extend_from_slice(self.glyphs.get(start..start + usize::from(cluster.glyph_len()))?);
            }
        }
        let added = hyphen.iter().map(|&(id, advance)| Glyph { id, x: 0., y: 0., advance });
        let start = self.glyphs.len();
        if rtl {
            self.glyphs.extend(added.rev());
            self.glyphs.extend(own);
        } else {
            self.glyphs.extend(own);
            self.glyphs.extend(added);
        }
        let len = self.glyphs.len() - start;
        if len == 0 || len > 255 {
            self.glyphs.truncate(start);
            return None;
        }
        let advance = if keep { cluster.advance } else { 0. } + hyphen.iter().map(|&(_, advance)| advance).sum::<f32>();
        let cluster = &mut self.shaped_clusters[index as usize];
        cluster.glyph_offset = start as u32;
        cluster.flags = cluster.flags.with_glyph_len(len as u8).with_inline_glyph(false);
        cluster.advance = advance;
        Some(saved)
    }

    /// pyro: gives the shaped cluster at `index` back what [`Self::show_hyphen`] returned.
    pub fn restore_cluster(&mut self, index: u32, saved: ClusterGlyphs) {
        if let Some(cluster) = self.shaped_clusters.get_mut(index as usize) {
            cluster.glyph_offset = saved.glyph_offset;
            cluster.flags = super::data::ShapedClusterFlags::from_bits(saved.flags);
            cluster.advance = saved.advance;
        }
    }

    /// Reserve the width of a separately shaped discretionary fragment.
    pub fn set_fragment_placeholder(&mut self, index: u32, advance: f32) -> ClusterGlyphs {
        let cluster = &mut self.shaped_clusters[index as usize];
        let old = ClusterGlyphs { glyph_offset: cluster.glyph_offset, flags: cluster.flags.bits(), advance: cluster.advance };
        cluster.flags = cluster.flags.with_inline_glyph(true).with_glyph_len(1);
        cluster.glyph_offset = 0;
        cluster.advance = advance;
        old
    }

    /// pyro: gives the one-glyph shaped cluster at `index` the glyph `id` with `advance`, and
    /// returns what it had (a soft hyphen at the end of a line shows a hyphen; one inside a line
    /// gets its own glyph back).
    #[inline]
    pub fn set_cluster_glyph(&mut self, index: u32, id: u32, advance: f32) -> Option<(u32, f32)> {
        let cluster = self.shaped_clusters.get_mut(index as usize)?;
        if cluster.glyph_len() != 1 {
            return None;
        }
        let before_advance = cluster.advance;
        cluster.advance = advance;
        if cluster.has_inline_glyph() {
            Some((core::mem::replace(&mut cluster.glyph_offset, id), before_advance))
        } else {
            let glyph = self.glyphs.get_mut(cluster.glyph_offset as usize)?;
            glyph.advance = advance;
            Some((core::mem::replace(&mut glyph.id, id), before_advance))
        }
    }

    /// The character data of the text, in logical order.
    ///
    /// [`ShapedRun::characters_range`] indexes into this. The nth [`Character`] corresponds exactly
    /// to the nth character of the source text (in the Unicode scalar value sense). This contains
    /// every character of the source text unless shaping was aborted due to
    /// [`FontSelector::select_font`][crate::FontSelector::select_font] failing to return a font.
    #[inline(always)]
    pub fn characters(&self) -> &[Character] {
        &self.characters
    }

    /// The shaped clusters.
    ///
    /// [`ShapedRun::shaped_clusters_range`] indexes into this.
    #[inline(always)]
    pub fn shaped_clusters(&self) -> &[ShapedCluster] {
        &self.shaped_clusters
    }

    /// The shaped glyphs.
    ///
    /// [`ShapedRun::glyphs_range`] indexes into this.
    #[inline(always)]
    pub fn glyphs(&self) -> &[Glyph] {
        &self.glyphs
    }

    /// The fonts.
    ///
    /// [`ShapedRun::font_index`] indexes into this.
    #[inline(always)]
    pub fn fonts(&self) -> &[FontInstance] {
        &self.fonts
    }

    /// The normalized font coordinates used by runs in this shaped text.
    ///
    /// [`ShapedRun::normalized_coords_range`] splits this into per-run slices.
    #[inline(always)]
    pub fn normalized_coords(&self) -> &[NormalizedCoord] {
        &self.normalized_coords
    }

    #[expect(clippy::cast_possible_truncation, reason = "Deferred")]
    pub(crate) fn push_run(
        &mut self,
        text: &str,
        range: TextRange,
        segment: &Segment,
        options: &ShapeOptions<'_>,
        char_info: &[CharInfo],
        char_style_indices: &[u16],
        font: &FontInstance,
        glyph_buffer: &harfrust::GlyphBuffer,
        normalized_coords: &[harfrust::NormalizedCoord],
        metrics: Option<RunMetrics>,
        trim: Option<&super::han_kerning::RunTrim>,
        // pyro: the glyphs' positions shaped top to bottom (`ShapeOptions::vertical_upright`),
        // in place of `glyph_buffer`'s.
        vertical: Option<&[harfrust::GlyphPosition]>,
    ) -> RunMetrics {
        debug_assert_eq!(
            range.char_range.len(),
            text[range.byte_range.clone()].chars().count(),
            "The claimed character range and the text's covered byte range should agree"
        );

        let glyph_infos = glyph_buffer.glyph_infos();

        let normalized_coords_range = {
            let start = self.normalized_coords.len();
            self.normalized_coords.extend(
                normalized_coords
                    .iter()
                    .map(|c| NormalizedCoord::from_bits(c.to_bits())),
            );
            start..self.normalized_coords.len()
        };

        let font_index = self
            .fonts
            .iter()
            .position(|f| f == font)
            .unwrap_or_else(|| {
                let index = self.fonts.len();
                self.fonts.push(font.clone());
                index
            });

        // pyro: the entry of this font, size and location in this text; the first run
        // of a combination enters the metrics the caller shaped it with before, or computes them.
        // The table is the same whatever the caller had cached.
        let metrics = {
            let font_size = options.font_size.to_bits();
            let coords = &self.normalized_coords[normalized_coords_range.clone()];
            let entry = self.run_metrics.iter().find(|e| {
                e.font_index == font_index
                    && e.font_size == font_size
                    && self.normalized_coords[e.normalized_coords_range.clone()] == *coords
            });
            match entry {
                Some(entry) => entry.metrics,
                None => {
                    let metrics = metrics.unwrap_or_else(|| {
                        // The same coords, in harfrust's type (bit-copied above).
                        RunMetrics::new(&self.fonts[font_index], options.font_size, normalized_coords)
                    });
                    self.run_metrics.push(RunMetricsEntry {
                        font_index,
                        font_size,
                        normalized_coords_range: normalized_coords_range.clone(),
                        metrics,
                    });
                    metrics
                }
            }
        };
        let units_per_em = metrics.units_per_em;
        let font_metrics = metrics.font_metrics;

        // `HarfRust` returns glyphs in visual order, so we need to process them as such while
        // maintaining logical ordering of clusters.

        let glyph_positions = vertical
            .filter(|positions| positions.len() == glyph_buffer.glyph_positions().len())
            .unwrap_or(glyph_buffer.glyph_positions());
        let scale_factor = options.font_size / units_per_em;
        let shaped_clusters_start = self.shaped_clusters.len();

        // Push all characters.
        let characters_start = self.characters.len();
        for (index, (((byte_offset, ch), info), style_index)) in text[range.byte_range.clone()]
            .char_indices()
            .zip(&char_info[range.char_range.clone()])
            .zip(&char_style_indices[range.char_range.clone()])
            .enumerate()
        {
            let flags = CharacterFlags::new(ch, info.is_word_boundary(), info.is_soft_wrap_opportunity())
                .with_phrase_fallback(info.is_phrase_fallback());
            self.characters.push(Character {
                text_byte_start: (range.byte_range.start + byte_offset) as u32,
                style_index: *style_index,
                // pyro: a line or paragraph separator breaks the line where `white-space`
                // preserves segment breaks (WPT CSS2 `bidi-breaking-003`); elsewhere it is a space.
                whitespace: if options.preserved_breaks && matches!(ch, '\u{2028}' | '\u{2029}') {
                    Whitespace::Newline
                } else {
                    Whitespace::from_char(ch)
                },
                // pyro: and how its fullwidth punctuation is trimmed (`han_kerning`).
                flags: match trim.and_then(|trim| trim.flags.get(index)) {
                    Some(&trim) if trim != 0 => flags.with_trim(trim),
                    _ => flags,
                },
                grapheme_start: info.is_grapheme_start(),
            });
        }
        // TODO: we force a grapheme break at the start of the run, as itemization could have split
        // a grapheme into two. Ideally, grapheme segmentation should be performed after
        // itemization, at which case this will always be true.
        self.characters[characters_start].grapheme_start = true;

        let glyphs_start = self.glyphs.len();
        if segment.bidi_level.is_ltr() {
            process_shaped_clusters(
                &mut self.shaped_clusters,
                &mut self.glyphs,
                scale_factor,
                glyph_infos.iter(),
                glyph_positions.iter(),
                &self.characters,
                characters_start,
            );
        } else {
            process_shaped_clusters(
                &mut self.shaped_clusters,
                &mut self.glyphs,
                scale_factor,
                glyph_infos.iter().rev(),
                glyph_positions.iter().rev(),
                &self.characters,
                characters_start,
            );
            // Reverse each cluster's glyphs, such that they are in paint order.
            //
            // TODO: could this become a single `reverse` over the range?
            for cluster in &self.shaped_clusters[shaped_clusters_start..] {
                if !cluster.has_inline_glyph() && cluster.glyph_len() > 1 {
                    let start = cluster.glyph_offset as usize;
                    self.glyphs[start..start + cluster.glyph_len() as usize].reverse();
                }
            }
        }

        let shaped_clusters_range = shaped_clusters_start as u32..self.shaped_clusters.len() as u32;
        let run_advance = self.shaped_clusters[shaped_clusters_start..]
            .iter()
            .map(|cluster| cluster.advance)
            .sum();

        self.runs.push(ShapedRun {
            range,
            font_size: options.font_size,
            font_index,
            characters_range: characters_start as u32..self.characters.len() as u32,
            shaped_clusters_range,
            glyphs_range: glyphs_start..self.glyphs.len(),
            normalized_coords_range,
            bidi_level: segment.bidi_level,
            advance: run_advance,
            font_metrics,
            hyphen: metrics.hyphen,
            upright_advance: None,
            vertical_shaped: vertical.is_some_and(|positions| positions.len() == glyph_buffer.glyph_positions().len()),
        });
        metrics
    }
}

/// One shaped run, belonging to a [`ShapedText`].
#[derive(Clone, Debug, PartialEq)]
pub struct ShapedRun {
    /// The range of text this run corresponds to.
    pub range: TextRange,
    /// Font size.
    pub font_size: f32,
    /// This run's font, as an index into [`ShapedText::fonts`].
    //
    // Note: We carry the font index instead of having `ShapedRun` carry `Arc<FontInstance>`,
    // meaning `ShapedRun` could become `Copy` once we have access to `core::range::Range`.
    pub font_index: usize,
    /// This run's characters, as a range into [`ShapedText::characters`].
    pub characters_range: Range<u32>,
    /// This run's shaped clusters, as a range into [`ShapedText::shaped_clusters`].
    pub shaped_clusters_range: Range<u32>,
    /// This run's glyphs, as a range into [`ShapedText::glyphs`].
    pub glyphs_range: Range<usize>,
    /// The normalized variation coords of this run, as a range into [`ShapedText::normalized_coords`].
    pub normalized_coords_range: Range<usize>,
    /// The bidi level of the run.
    pub bidi_level: BidiLevel,
    /// Total advance of the run.
    pub advance: f32,
    /// pyro: the hyphen of the run's font (U+2010, else U+002D): its glyph and its advance at
    /// the run's size. The line breaker shows it where a line ends at a soft hyphen.
    pub hyphen: Option<(u32, f32)>,
    /// pyro: the advance of every glyph of an upright run in a font without vertical metrics
    /// (parley's `shape::upright`): the hyphens the line breaker shows advance by it too.
    pub upright_advance: Option<f32>,
    /// pyro: its advances are the ones shaping it top to bottom gives (`ShapeOptions::vertical_upright`).
    pub vertical_shaped: bool,
    /// The font metrics of this run.
    pub font_metrics: FontMetrics,
}

/// Processes shaped glyphs from `HarfRust` and converts them into `ShapedCluster` and `Glyph`.
///
/// # Parameters
///
/// ## Output Parameters (mutated by this function):
/// * `shaped_clusters` - Vector where new `ShapedCluster` entries will be pushed.
/// * `glyphs` - Vector where new `Glyph` entries will be pushed. Note: single-glyph clusters
///   with zero offsets are inlined directly into `ShapedCluster`.
///
/// ## Input Parameters:
/// * `scale_factor` - Scaling factor used to convert font units to the target size.
/// * `glyph_infos` - `HarfRust` glyph information in logical order (i.e., reversed for RTL runs).
/// * `glyph_positions` - `HarfRust` glyph positioning data in logical order (i.e., reversed for RTL
///   runs).
/// * `characters` must contain the shaped characters whose clusters we're now processing, starting at
///   index `characters_start`.
/// * `characters_start` - See `characters`.
fn process_shaped_clusters<'a>(
    shaped_clusters: &mut Vec<ShapedCluster>,
    glyphs: &mut Vec<Glyph>,
    scale_factor: f32,
    glyph_infos: impl Iterator<Item = &'a harfrust::GlyphInfo>,
    glyph_positions: impl Iterator<Item = &'a harfrust::GlyphPosition>,
    characters: &[Character],
    characters_start: usize,
) {
    struct Cluster {
        id: u32,
        characters_start: usize,
        glyphs_start: usize,
        glyphs: u32,
        inline_glyph: Option<Glyph>,
        advance: f32,
    }

    /// Flush `cluster`, whose characters end at `char_end`, onto `shaped_clusters`.
    #[expect(clippy::cast_possible_truncation, reason = "Deferred")]
    fn flush(
        cluster: &mut Cluster,
        char_end: usize,
        characters: &[Character],
        shaped_clusters: &mut Vec<ShapedCluster>,
    ) {
        let first_character = &characters[cluster.characters_start];
        let is_newline = first_character.whitespace == Whitespace::Newline;
        let (glyph_offset, glyph_len, inline_glyph, advance) = if is_newline {
            // Elide glyphs of newlines.
            (cluster.glyphs_start as u32, 0, false, 0.)
        } else if let Some(inline_glyph) = cluster.inline_glyph.take() {
            (inline_glyph.id, 1, true, cluster.advance)
        } else {
            (
                cluster.glyphs_start as u32,
                cluster.glyphs as u8,
                false,
                cluster.advance,
            )
        };

        let mut breaks = (cluster.characters_start..char_end.saturating_sub(1))
            .filter(|&index| characters[index].flags.is_soft_hyphen())
            .map(|index| index + 1);
        let split = breaks.next();
        shaped_clusters.push(ShapedCluster {
            chars_range: (cluster.characters_start as u32, split.unwrap_or(char_end) as u32),
            style_index: first_character.style_index,
            flags: ShapedClusterFlags::new(glyph_len)
                .with_grapheme_start(first_character.grapheme_start)
                // TODO: fill with actual shaping data (`parley` currently just ignores this)
                .with_safe_to_break_before(false)
                .with_inline_glyph(inline_glyph)
                .with_first_char(
                    first_character.flags.is_soft_wrap_opportunity(),
                    first_character.whitespace,
                ),
            glyph_offset,
            advance,
        });
        let mut next = split;
        while let Some(start) = next {
            next = breaks.next();
            let first = characters[start];
            shaped_clusters.push(ShapedCluster {
                chars_range: (start as u32, next.unwrap_or(char_end) as u32),
                style_index: first.style_index,
                flags: ShapedClusterFlags::new(0)
                    .discretionary_continuation()
                    .with_grapheme_start(true)
                    .with_first_char(first.flags.is_soft_wrap_opportunity(), first.whitespace),
                glyph_offset: 0,
                advance: 0.,
            });
        }
    }

    let mut cluster = Cluster {
        id: 0,
        characters_start,
        glyphs_start: glyphs.len(),
        glyphs: 0,
        inline_glyph: None,
        advance: 0.,
    };

    for (glyph_info, glyph_pos) in glyph_infos.zip(glyph_positions) {
        if glyph_info.cluster != cluster.id {
            let char_end = characters_start + glyph_info.cluster as usize;
            flush(&mut cluster, char_end, characters, shaped_clusters);

            cluster = Cluster {
                id: glyph_info.cluster,
                characters_start: char_end,
                glyphs_start: glyphs.len(),
                glyphs: 0,
                inline_glyph: None,
                advance: 0.,
            };
        }

        let glyph = Glyph {
            id: glyph_info.glyph_id,
            x: (glyph_pos.x_offset as f32) * scale_factor,
            // Convert from font space (Y-up) to layout space (Y-down)
            y: -(glyph_pos.y_offset as f32) * scale_factor,
            advance: (glyph_pos.x_advance as f32) * scale_factor,
        };
        // pyro: an opening bracket that the line breaker may trim at a line's start keeps its
        // glyph stored, to take an offset then.
        if cluster.glyphs == 0
            && glyph.x == 0.
            && glyph.y == 0.
            && !characters
                .get(characters_start + glyph_info.cluster as usize)
                .is_some_and(|character| character.flags.is_trim_open())
        {
            // Defer this potential zero-offset, single glyph cluster
            cluster.inline_glyph = Some(glyph);
        } else {
            if let Some(pending_glyph) = cluster.inline_glyph.take() {
                glyphs.push(pending_glyph);
            }
            glyphs.push(glyph);
        }
        cluster.advance += glyph.advance;
        cluster.glyphs += 1;
    }

    // Flush the final cluster.
    flush(&mut cluster, characters.len(), characters, shaped_clusters);
}

#[cfg(test)]
mod tests {
    use alloc::{sync::Arc, vec, vec::Vec};

    use fontique::Synthesis;
    use linebender_resource_handle::{Blob, FontData};

    use crate::{
        Analysis, AnalysisOptions, Analyzer, FontInstance, FontInstanceRef, FontSelector,
        ShapeOptions, Shaper,
        itemize::{Item, Segment},
        shape::CharCluster,
    };

    use super::ShapedText;

    const ROBOTO: &[u8] =
        include_bytes!("../../../parley_dev/assets/fonts/roboto_fonts/Roboto-Regular.ttf");
    const NOTO_KUFI_ARABIC: &[u8] =
        include_bytes!("../../../parley_dev/assets/fonts/noto_fonts/NotoKufiArabic-Regular.otf");
    const NOTO_COLOR_EMOJI: &[u8] = include_bytes!(
        "../../../parley_dev/assets/fonts/noto_color_emoji/NotoColorEmoji-Subset.ttf"
    );

    /// A [`FontSelector`] shaping everything with a single font.
    struct SingleFont(FontInstance);

    impl FontSelector for SingleFont {
        fn select_font(
            &mut self,
            _item: &Segment,
            _options: &ShapeOptions<'_>,
            _cluster: &mut CharCluster,
        ) -> Option<FontInstanceRef<'_>> {
            Some(self.0.as_ref())
        }
    }

    /// A [`FontSelector`] shaping clusters starting with a particular character with one font, and
    /// all other clusters with another, to split the text into runs at that character.
    struct FontForChar {
        ch: char,
        font: FontInstance,
        other_font: FontInstance,
    }

    impl FontSelector for FontForChar {
        fn select_font(
            &mut self,
            _segment: &Segment,
            _options: &ShapeOptions<'_>,
            cluster: &mut CharCluster,
        ) -> Option<FontInstanceRef<'_>> {
            if cluster.chars()[0].ch == self.ch {
                Some(self.font.as_ref())
            } else {
                Some(self.other_font.as_ref())
            }
        }
    }

    fn analyze(text: &str) -> Analysis {
        let mut analysis = Analysis::new();
        Analyzer::new().analyze(
            text,
            &AnalysisOptions {
                line_break: &[],
                line_break_override: None,
                ..AnalysisOptions::default()
            },
            &mut analysis,
        );
        analysis
    }

    fn font_instance(font_data: &'static [u8]) -> FontInstance {
        FontInstance {
            font: FontData::new(Blob::new(Arc::new(font_data)), 0),
            synthesis: Synthesis::default(),
        }
    }

    fn shape_with_font(text: &str, font_data: &'static [u8]) -> ShapedText {
        shape_with_font_selector(text, SingleFont(font_instance(font_data)))
    }

    fn shape_with_font_selector(text: &str, select_font: impl FontSelector) -> ShapedText {
        let analysis = analyze(text);
        let mut shaper = Shaper::default();
        let mut shaped = ShapedText::new();

        let char_style_indices = vec![0; text.chars().count()];
        let items = [Item {
            char_end: text.chars().count().try_into().unwrap(),
            options: ShapeOptions {
                font_size: 32.0,
                point_size: 32.0,
                language: None,
                features: &[],
                variations: &[],
                face_size_adjust: true,
                auto_opsz: false,
                language_override: None,
                spacing_trim: Default::default(),
                vertical_upright: false,
                isolated_before: false,
                isolated_after: false,
                preserved_breaks: false,
            },
        }];
        shaper.shape_text(
            text,
            &analysis,
            &char_style_indices,
            items,
            select_font,
            &mut shaped,
        );
        shaped
    }

    /// A run can shape into no glyphs at all: `HarfRust` normally shapes `default-ignorable`
    /// characters (like a soft-hyphen) into a space glyph with zero advance, but if the space glyph
    /// is not available, it just deletes the character.
    ///
    /// This checks whether we handle that.
    #[test]
    fn run_without_glyphs_is_kept() {
        // The soft hyphen is `default-ignorable`, and is a grapheme of its own, so font selection
        // gives it a run of its own.
        let shaped = shape_with_font_selector(
            "a\u{00AD}b",
            FontForChar {
                ch: '\u{00AD}',
                // Our Noto Color Emoji subset has no space character.
                font: font_instance(NOTO_COLOR_EMOJI),
                other_font: font_instance(ROBOTO),
            },
        );

        assert_eq!(
            shaped.characters().iter().count(),
            3,
            "the characters of a run without glyphs are shaped"
        );
        let [a, soft_hyphen, b] = shaped.runs() else {
            panic!("expected three runs, got {}", shaped.runs().len());
        };
        assert_eq!(a.characters_range, 0..1);
        assert_eq!(soft_hyphen.characters_range, 1..2);
        assert_eq!(b.characters_range, 2..3);

        assert!(soft_hyphen.glyphs_range.is_empty());
        assert_eq!(soft_hyphen.advance, 0.);

        // The soft-hyphen's run should have a single cluster, and it must be without glyphs.
        let clusters = shaped.run_slice(1).shaped_clusters();
        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].glyph_len(), 0);
        assert_eq!(clusters[0].chars_range(), 1..2);
    }

    #[test]
    fn single_cluster_run_advance_matches_its_cluster() {
        let shaped = shape_with_font("A", ROBOTO);
        let run = &shaped.runs()[0];
        let cluster = &shaped.shaped_clusters()[run.shaped_clusters_range.start as usize];

        assert!(cluster.advance > 0.0);
        assert_eq!(run.advance, cluster.advance);
    }

    /// [U+0600 U+0623 U+064F] is a single grapheme in text-wide segmentation, but U+0600 has
    /// bidi class AN while the following characters resolve to a different bidi level, so
    /// itemization splits the grapheme into the items [U+0600] [U+0623 U+064F]. The item
    /// boundary forces a grapheme start on U+0623.
    #[test]
    fn one_grapheme_split_across_items() {
        let shaped = shape_with_font("\u{0600}\u{0623}\u{064F}", NOTO_KUFI_ARABIC);

        let grapheme_starts: Vec<bool> =
            shaped.characters.iter().map(|c| c.grapheme_start).collect();
        assert_eq!(grapheme_starts, [true, true, false]);

        let clusters: Vec<(u32, u32, bool)> = shaped
            .shaped_clusters
            .iter()
            .map(|c| {
                (
                    c.chars_range().start,
                    c.chars_range().end,
                    c.is_grapheme_start(),
                )
            })
            .collect();
        assert_eq!(clusters, [(0, 1, true), (1, 3, true)]);
    }

    /// "ffi" is three graphemes but shapes into one cluster.
    #[test]
    fn three_graphemes_one_cluster() {
        let shaped = shape_with_font("ffi", ROBOTO);

        let grapheme_starts: Vec<bool> =
            shaped.characters.iter().map(|c| c.grapheme_start).collect();
        assert_eq!(grapheme_starts, [true, true, true]);

        let clusters: Vec<(u32, u32, bool)> = shaped
            .shaped_clusters
            .iter()
            .map(|c| {
                (
                    c.chars_range().start,
                    c.chars_range().end,
                    c.is_grapheme_start(),
                )
            })
            .collect();
        assert_eq!(clusters, [(0, 3, true)]);
    }

    #[test]
    fn newlines_removed() {
        // pyro: not U+2028 or U+2029, which are shown as a space (`Whitespace::from_char`).
        for newline in ["\n", "\r"] {
            let shaped = shape_with_font(&alloc::format!("a{newline}b"), ROBOTO);

            let index = shaped
                .shaped_clusters
                .iter()
                .position(|cluster| cluster.chars_range().start == 1)
                .unwrap();
            let newline_cluster = &shaped.shaped_clusters[index];
            assert_eq!(newline_cluster.glyph_len(), 0, "newline glyphs are removed");
            assert_eq!(newline_cluster.advance, 0., "newline advance is set to 0");
        }
    }

    /// An itemization boundary forces a cluster break in shaping, and must force a grapheme start.
    #[test]
    fn item_boundaries_force_grapheme_start() {
        // Two regional indicators forming a single flag grapheme...
        let text = "\u{1F1E6}\u{1F1E7}";
        let char_style_indices = vec![0; text.chars().count()];
        let analysis = analyze(text);
        assert_eq!(
            analysis
                .char_info()
                .iter()
                .map(|info| info.is_grapheme_start())
                .collect::<Vec<_>>(),
            [true, false]
        );

        let font = font_instance(ROBOTO);
        let mut shaper = Shaper::default();
        let mut shaped = ShapedText::new();

        // ...split over two items.
        let items = [
            Item {
                char_end: 1,
                options: ShapeOptions {
                    font_size: 32.0,
                    point_size: 32.0,
                    language: None,
                    features: &[],
                    variations: &[],
                    face_size_adjust: true,
                    auto_opsz: false,
                    language_override: None,
                    spacing_trim: Default::default(),
                    vertical_upright: false,
                    isolated_before: false,
                    isolated_after: false,
                    preserved_breaks: false,
                },
            },
            Item {
                char_end: text.chars().count().try_into().unwrap(),
                options: ShapeOptions {
                    font_size: 32.0,
                    point_size: 32.0,
                    language: None,
                    features: &[],
                    variations: &[],
                    face_size_adjust: true,
                    auto_opsz: false,
                    language_override: None,
                    spacing_trim: Default::default(),
                    vertical_upright: false,
                    isolated_before: false,
                    isolated_after: false,
                    preserved_breaks: false,
                },
            },
        ];
        shaper.shape_text(
            text,
            &analysis,
            &char_style_indices,
            items,
            SingleFont(font),
            &mut shaped,
        );

        let grapheme_starts: Vec<bool> =
            shaped.characters.iter().map(|c| c.grapheme_start).collect();
        assert_eq!(grapheme_starts, [true, true]);

        // But note that, had we had three regional indicator symbols, like `[R0 R1 R2]` itemized as
        // `[R0] [R1 R2]`, the last pair should form a single grapheme. We don't do that currently.
    }
}
