use std::sync::{Arc, Mutex};

use crate::stylo_to_parley;
use app_units::Au;
use parley::{FontContext, FontVariation};
use skrifa::MetadataProvider as _;
use skrifa::charmap::Charmap;
use style::properties::style_structs::Font as FontStyles;
use style::{
    device::servo::FontMetricsProvider,
    font_metrics::FontMetrics,
    values::computed::{CSSPixelLength, font::QueryFontMetricsFlags},
};

/// Query fontique for the fonts matching `font_styles` (family list and attributes).
fn query_for_font_styles<'a>(
    font_ctx: &'a mut FontContext,
    font_styles: &'a FontStyles,
) -> parley::fontique::Query<'a> {
    use parley::fontique::Attributes;

    let mut query = font_ctx.collection.query(&mut font_ctx.source_cache);
    let families = &font_styles.font_family.families;
    let fallback = stylo_to_parley::fallback_font_family(families);
    query.set_families(
        families
            .iter()
            .filter(|family| !stylo_to_parley::is_hidden_family(family))
            .map(stylo_to_parley::query_font_family)
            .chain(fallback.map(parley::fontique::QueryFamily::Generic)),
    );
    query.set_attributes(Attributes {
        width: stylo_to_parley::font_width(font_styles.font_width),
        weight: stylo_to_parley::font_weight(font_styles.font_weight),
        style: stylo_to_parley::font_style(font_styles.font_style),
    });
    query
}

#[derive(Clone)]
pub(crate) struct BlitzFontMetricsProvider {
    pub(crate) font_ctx: Arc<Mutex<FontContext>>,
}

impl core::fmt::Debug for BlitzFontMetricsProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "BlitzFontMetricsProvider")
    }
}

impl FontMetricsProvider for BlitzFontMetricsProvider {
    fn query_font_metrics(
        &self,
        vertical: bool,
        font_styles: &FontStyles,
        font_size: CSSPixelLength,
        _flags: QueryFontMetricsFlags,
    ) -> FontMetrics {
        // The first available font: the first of the list that has U+0020.
        let metrics = self.metrics_for_axis(font_styles, font_size, ' ', vertical);
        // pyro: under `font-size-adjust` the font is used at the adjusted size: `ex`, `ch`, `ic`
        // and `cap` measure it there (`em` stays the computed size).
        if font_styles.font_size_adjust == style::values::computed::font::FontSizeAdjust::None {
            return metrics;
        }
        let scale = self.font_size_adjust_scale(font_styles, font_size);
        if scale == 1.0 {
            return metrics;
        }
        let scaled = |length: CSSPixelLength| CSSPixelLength::new(length.px() * scale);
        FontMetrics {
            ascent: scaled(metrics.ascent),
            x_height: metrics.x_height.map(scaled),
            cap_height: metrics.cap_height.map(scaled),
            zero_advance_measure: metrics.zero_advance_measure.map(scaled),
            ic_width: metrics.ic_width.map(scaled),
            ..metrics
        }
    }

    fn base_size_for_generic(
        &self,
        generic: style::values::computed::font::GenericFontFamily,
    ) -> style::values::computed::Length {
        let size = match generic {
            style::values::computed::font::GenericFontFamily::Monospace => 13.0,
            _ => 16.0,
        };
        style::values::computed::Length::from(Au::from_f32_px(size))
    }

    /// pyro: `line-height: normal` as layout uses it (`layout::first_letter`): the first
    /// available font's ascent and descent in whole pixels, as Blink's, and its line gap.
    fn normal_line_height(&self, font_styles: &FontStyles, size: CSSPixelLength) -> Option<CSSPixelLength> {
        let (ascent, descent, gap, adjusted) = self.line_extents_for(font_styles, size.px(), ' ')?;
        let height = ascent.round() + descent.round();
        // (Chromium's taller ascent for Times, Helvetica and Courier, as parley's line boxes have
        // it: `parley_engine::shape::chromium_ascent_adjustment`.)
        let adjustment = if adjusted { (height * 0.15 + 0.5).floor() } else { 0.0 };
        Some(CSSPixelLength::new(height + adjustment + gap.max(0.0).round()))
    }
}

impl BlitzFontMetricsProvider {
    /// pyro: the ascent, descent (below the baseline, positive), line gap and cap height (CSS
    /// px, `font_size` big) of the first font of `font_styles`' list that draws `text_char` (in
    /// its `unicode-range` and character map), as parley measures them.
    pub(crate) fn extents_for(
        &self,
        font_styles: &FontStyles,
        font_size: f32,
        text_char: char,
    ) -> Option<(f32, f32, f32, f32)> {
        self.extents_and_family_for(font_styles, font_size, text_char).map(|(extents, _)| extents)
    }

    /// pyro: the ascent, descent and line gap of [`Self::extents_for`]'s font, and whether its
    /// family is Times, Helvetica or Courier (which Chromium gives a taller ascent).
    fn line_extents_for(&self, font_styles: &FontStyles, font_size: f32, text_char: char) -> Option<(f32, f32, f32, bool)> {
        self.extents_and_family_for(font_styles, font_size, text_char)
            .map(|((ascent, descent, gap, _), adjusted)| (ascent, descent, gap, adjusted))
    }

    fn extents_and_family_for(
        &self,
        font_styles: &FontStyles,
        font_size: f32,
        text_char: char,
    ) -> Option<((f32, f32, f32, f32), bool)> {
        use parley::fontique::{QueryFont, QueryStatus};
        use skrifa::instance::{LocationRef, Size};
        use skrifa::metrics::Metrics;

        let mut font_ctx = self.font_ctx.lock().unwrap();
        let font_ctx = &mut *font_ctx;
        let mut query = query_for_font_styles(font_ctx, font_styles);
        let variations = stylo_to_parley::font_variations(font_styles, font_size);
        let mut found = None;
        query.matches_with(|q_font: &QueryFont| {
            if !q_font.unicode_range.contains(text_char.into()) {
                return QueryStatus::Continue;
            }
            let Ok(font_ref) = skrifa::FontRef::from_index(q_font.blob.as_ref(), q_font.index) else {
                return QueryStatus::Continue;
            };
            if font_ref.charmap().map(text_char).is_none() {
                return QueryStatus::Continue;
            }
            found = Some(q_font.clone());
            QueryStatus::Stop
        });
        let font = found?;
        let font_ref = skrifa::FontRef::from_index(font.blob.as_ref(), font.index).ok()?;
        let location = font_ref.axes().location(
            variations
                .iter()
                .map(|v| (skrifa::Tag::from_be_bytes(v.tag.to_bytes()), v.value)),
        );
        let metrics = Metrics::new(&font_ref, Size::new(font_size), LocationRef::from(&location));
        let cap_height = metrics.cap_height.unwrap_or(metrics.ascent);
        let adjusted = font_ref
            .localized_strings(skrifa::string::StringId::FAMILY_NAME)
            .english_or_first()
            .is_some_and(|family| ["Times", "Helvetica", "Courier"].iter().any(|name| family.chars().eq(name.chars())));
        Some(((metrics.ascent, -metrics.descent, metrics.leading, cap_height), adjusted))
    }

    /// pyro: the metrics of the first font of `font_styles`' list that draws `text_char` (in its
    /// `unicode-range` and character map): `' '` is CSS's first available font, a letter the
    /// font a run of Latin text is drawn in (`font-size-adjust` scales by that one).
    /// pyro: `font-size-adjust` (CSS Fonts 5 §3.6): the factor of the font size that gives the
    /// font that draws `x` the named metric's ratio (`ex-height`, `cap-height`, `ch-width`,
    /// `ic-width`, `ic-height`); a font without the metric is taken to have the spec's fallback
    /// (0.5em for `ex` and `ch`, 1em for `ic`, the ascent for `cap`). 1 for none.
    pub(crate) fn font_size_adjust_scale(&self, font_styles: &FontStyles, font_size: CSSPixelLength) -> f32 {
        use style::values::computed::font::FontSizeAdjust;
        let size = font_size.px();
        if font_styles.font_size_adjust == FontSizeAdjust::None || !(size > 0.0) {
            return 1.0;
        }
        // The font that draws the text: the property keeps the metric of each font used (a
        // fallback one too); a run is taken to be drawn in the font that has `x`.
        let metrics = self.metrics_for(font_styles, font_size, 'x');
        let (wanted, metric) = match font_styles.font_size_adjust {
            FontSizeAdjust::None => return 1.0,
            FontSizeAdjust::ExHeight(v) => (v.0, metrics.x_height.map_or(size * 0.5, |m| m.px())),
            FontSizeAdjust::CapHeight(v) => (v.0, metrics.cap_height.unwrap_or(metrics.ascent).px()),
            FontSizeAdjust::ChWidth(v) => (v.0, metrics.zero_advance_measure.map_or(size * 0.5, |m| m.px())),
            FontSizeAdjust::IcWidth(v) | FontSizeAdjust::IcHeight(v) => (v.0, metrics.ic_width.map_or(size, |m| m.px())),
        };
        let ratio = metric / size;
        // (A ratio of 0 is left as it is.)
        if ratio > 0.0 && ratio.is_finite() && wanted.is_finite() { wanted / ratio } else { 1.0 }
    }

    pub(crate) fn metrics_for(
        &self,
        font_styles: &FontStyles,
        font_size: CSSPixelLength,
        text_char: char,
    ) -> FontMetrics {
        self.metrics_for_axis(font_styles, font_size, text_char, false)
    }

    /// pyro: [`Self::metrics_for`], with `ch` measured along the vertical axis when `vertical`
    /// (upright text in a vertical writing mode: the "0" glyph's vertical advance).
    pub(crate) fn metrics_for_axis(
        &self,
        font_styles: &FontStyles,
        font_size: CSSPixelLength,
        text_char: char,
        vertical: bool,
    ) -> FontMetrics {
        use parley::fontique::{Query, QueryFont, QueryStatus};
        use skrifa::instance::{LocationRef, Size};
        use skrifa::metrics::{GlyphMetrics, Metrics};

        // Lock font_ctx. Explicit reborrow required for borrow checker.
        let mut font_ctx = self.font_ctx.lock().unwrap();
        let font_ctx = &mut *font_ctx;

        // Query fontique for the font that matches the font styles
        let mut query = query_for_font_styles(font_ctx, font_styles);
        // let fb_script = crate::swash_convert::script_to_fontique(script);
        // let fb_language = locale.and_then(crate::swash_convert::locale_to_fontique);
        // query.set_fallbacks(fontique::FallbackKey::new(fb_script, fb_language.as_ref()));

        let variations = stylo_to_parley::font_variations(font_styles, font_size.px());
        // pyro: a face's `size-adjust` scales its metrics, but not where `font-size-adjust` sets
        // the size (the text is then drawn without it).
        let size_adjust = font_styles.font_size_adjust == style::values::computed::font::FontSizeAdjust::None;
        // let features = self.rcx.features(style.font_features).unwrap_or(&[]);

        // fn name_of(font_ref: &skrifa::FontRef) -> String {
        //     use skrifa::string::StringId;
        //     font_ref
        //         .localized_strings(StringId::POSTSCRIPT_NAME)
        //         .english_or_first()
        //         .unwrap()
        //         .chars()
        //         .collect()
        // }

        fn find_font_for(query: &mut Query, ch: char) -> Option<QueryFont> {
            let mut font = None;
            query.matches_with(|q_font: &QueryFont| {
                use skrifa::MetadataProvider;

                // A face is not used for code points outside its `unicode-range`.
                if !q_font.unicode_range.contains(ch.into()) {
                    return QueryStatus::Continue;
                }
                let Ok(font_ref) = skrifa::FontRef::from_index(q_font.blob.as_ref(), q_font.index)
                else {
                    return QueryStatus::Continue;
                };

                let charmap = font_ref.charmap();
                if charmap.map(ch).is_some() {
                    font = Some(q_font.clone());
                    QueryStatus::Stop
                } else {
                    QueryStatus::Continue
                }
            });
            font
        }

        /// Scales the advance the same way as Parley's shaped glyph advances (font units × size / upem)
        /// rather than with skrifa's FreeType-compatible fixed-point scale, so that `ch`/`ic`
        /// lengths exactly match the width of the corresponding laid-out text.
        fn advance_of(
            query: &mut Query,
            ch: char,
            font_size: f32,
            variations: &[FontVariation],
            size_adjust: bool,
        ) -> Option<f32> {
            let font = find_font_for(query, ch)?;
            // pyro: at the size the font is used at (`size-adjust`).
            let font_size = if size_adjust { font_size * font.synthesis.size_adjust() } else { font_size };
            let font_ref = skrifa::FontRef::from_index(font.blob.as_ref(), font.index).ok()?;
            let location = font_ref.axes().location(
                variations
                    .iter()
                    .map(|v| (skrifa::Tag::from_be_bytes(v.tag.to_bytes()), v.value)),
            );
            let location_ref = LocationRef::from(&location);
            let upem = Metrics::new(&font_ref, Size::unscaled(), location_ref).units_per_em;
            if upem == 0 {
                return None;
            }
            let glyph_metrics = GlyphMetrics::new(&font_ref, Size::unscaled(), location_ref);
            let char_map = Charmap::new(&font_ref);
            let glyph_id = char_map.map(ch)?;
            let advance = glyph_metrics.advance_width(glyph_id)?;
            Some(advance * (font_size / upem as f32))
        }

        fn metrics_of(
            query: &mut Query,
            ch: char,
            font_size: f32,
            variations: &[FontVariation],
            size_adjust: bool,
        ) -> Option<(f32, Option<f32>, Option<f32>)> {
            let font = find_font_for(query, ch)?;
            // pyro: at the size the font is used at (`size-adjust`), with the ascent its
            // `@font-face` rule may override.
            let font_size = if size_adjust { font_size * font.synthesis.size_adjust() } else { font_size };
            let font_ref = skrifa::FontRef::from_index(font.blob.as_ref(), font.index).ok()?;
            let location = font_ref.axes().location(
                variations
                    .iter()
                    .map(|v| (skrifa::Tag::from_be_bytes(v.tag.to_bytes()), v.value)),
            );
            let location_ref = LocationRef::from(&location);
            let metrics = Metrics::new(&font_ref, Size::new(font_size), location_ref);
            let ascent = font.synthesis.face_metrics().ascent.map_or(metrics.ascent, |a| a * font_size);
            Some((ascent, metrics.x_height, metrics.cap_height))
        }

        // pyro: `ch` is the "0" of the face that would render it by `unicode-range` (the first
        // whose range covers U+0030, WPT css-values `ch-unit-018`), 0.5em when that face has no
        // "0" glyph (CSS Values 4 § 6.1.1; `ch-unit-019`), not a fallback font's; in upright
        // vertical text its vertical advance (`vmtx`; `ch-unit-017`).
        let zero_advance = (|| {
            // (Whatever glyphs it has.)
            let mut first = None;
            query.matches_with(|q_font: &QueryFont| {
                if q_font.unicode_range.contains('0'.into()) {
                    first = Some(q_font.clone());
                    QueryStatus::Stop
                } else {
                    QueryStatus::Continue
                }
            });
            let font = first?;
            let font_ref = skrifa::FontRef::from_index(font.blob.as_ref(), font.index).ok()?;
            use skrifa::MetadataProvider;
            let glyph = font_ref.charmap().map('0')?;
            if vertical {
                use skrifa::raw::TableProvider;
                let metrics = Metrics::new(&font_ref, Size::unscaled(), LocationRef::default());
                let upem = metrics.units_per_em;
                let size = if size_adjust { font_size.px() * font.synthesis.size_adjust() } else { font_size.px() };
                if upem != 0 {
                    // (Without vertical metrics, the font's height: parley's `shape::upright`.)
                    let advance = match font_ref.vmtx() {
                        Ok(vmtx) => vmtx.advance(glyph).map(f32::from),
                        Err(_) => Some(metrics.ascent - metrics.descent),
                    };
                    if let Some(advance) = advance {
                        return Some(advance * size / f32::from(upem));
                    }
                }
            }
            let size = if size_adjust { font_size.px() * font.synthesis.size_adjust() } else { font_size.px() };
            let location = font_ref.axes().location(
                variations.iter().map(|v| (skrifa::Tag::from_be_bytes(v.tag.to_bytes()), v.value)),
            );
            let location_ref = LocationRef::from(&location);
            let upem = Metrics::new(&font_ref, Size::unscaled(), location_ref).units_per_em;
            if upem == 0 {
                return None;
            }
            let advance = GlyphMetrics::new(&font_ref, Size::unscaled(), location_ref).advance_width(glyph)?;
            Some(advance * (size / upem as f32))
        })();
        let ic_advance = advance_of(&mut query, '\u{6C34}', font_size.px(), &variations, size_adjust);
        let (ascent, x_height, cap_height) =
            metrics_of(&mut query, text_char, font_size.px(), &variations, size_adjust)
                .unwrap_or((0.0, None, None));

        FontMetrics {
            ascent: CSSPixelLength::new(ascent),
            x_height: x_height.filter(|xh| *xh != 0.0).map(CSSPixelLength::new),
            cap_height: cap_height.map(CSSPixelLength::new),
            zero_advance_measure: zero_advance.map(CSSPixelLength::new),
            ic_width: ic_advance.map(CSSPixelLength::new),
            script_percent_scale_down: None,
            script_script_percent_scale_down: None,
        }
    }
}
