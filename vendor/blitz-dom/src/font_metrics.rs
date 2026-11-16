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
        _vertical: bool,
        font_styles: &FontStyles,
        font_size: CSSPixelLength,
        _flags: QueryFontMetricsFlags,
    ) -> FontMetrics {
        // The first available font: the first of the list that has U+0020.
        self.metrics_for(font_styles, font_size, ' ')
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
        Some((metrics.ascent, -metrics.descent, metrics.leading, cap_height))
    }

    /// pyro: the metrics of the first font of `font_styles`' list that draws `text_char` (in its
    /// `unicode-range` and character map): `' '` is CSS's first available font, a letter the
    /// font a run of Latin text is drawn in (`font-size-adjust` scales by that one).
    pub(crate) fn metrics_for(
        &self,
        font_styles: &FontStyles,
        font_size: CSSPixelLength,
        text_char: char,
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

        let zero_advance = advance_of(&mut query, '0', font_size.px(), &variations, size_adjust);
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
