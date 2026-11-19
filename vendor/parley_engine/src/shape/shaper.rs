// Copyright 2026 the Parley Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Shaping of text.

use alloc::vec::Vec;
use core::mem;
use harfrust::ShapeOptions as HarfShapeOptions;
use linebender_resource_handle::{Blob, FontData};
use parlance::{FontFeature, FontVariation, Language};

use crate::{
    Analysis, CharInfo, ShapedText,
    itemize::{Item, Segment, TextRange},
    lru_cache::LruCache,
    shape::{CharCluster, SpacingTrim, cache, han_kerning},
};

/// Runs `$f` with the glyph cache. With `std` it is one per thread, shared
/// by every `Shaper` (every document): its keys hold the font's blob id and
/// the run's exact shaping inputs, so a document reusing a font blob (the
/// system fonts; web fonts its embedder interns) reuses its glyphs, as
/// Chromium's shape caches serve every document.
#[cfg(feature = "std")]
macro_rules! with_glyph_cache {
    ($scx:expr, $f:expr) => {
        GLYPH_CACHE.with(|cache| ($f)(&mut *cache.borrow_mut()))
    };
}
#[cfg(not(feature = "std"))]
macro_rules! with_glyph_cache {
    ($scx:expr, $f:expr) => {
        ($f)(&mut $scx.glyph_cache)
    };
}

#[cfg(feature = "std")]
std::thread_local! {
    static GLYPH_CACHE: core::cell::RefCell<cache::GlyphCache> = Default::default();
}

/// Shaping options for one item.
///
/// These are styling options relevant for shaping. They're styling, in that they're not derived
/// from the underlying text. When you [shape the text][`Shaper::shape_text`], you should split the
/// text into [`Item`]s at the points where these options change.
#[derive(Debug)]
pub struct ShapeOptions<'a> {
    /// The font size to shape the item with.
    pub font_size: f32,
    /// pyro: the font size in points, for size-specific tracking (the AAT `trak` table, which
    /// harfrust applies; San Francisco has one). The same as `font_size` unless the layout is
    /// scaled: parley shapes a scaled layout at `font_size` times its scale, while tracking
    /// depends on the size the text is set in (CSS pixels), as in Core Text and Chromium.
    pub point_size: f32,
    /// The language to shape the item with.
    pub language: Option<Language>,
    /// The font features to shape the item with.
    ///
    /// If there are font features with duplicate [tags][`FontFeature::tag`], later values override
    /// earlier values.
    pub features: &'a [FontFeature],
    /// The font variations that are constant over an item.
    pub variations: &'a [FontVariation],
    /// pyro: whether a font is used at its `size-adjust` (`fontique::Synthesis::size_adjust`):
    /// not when CSS `font-size-adjust` sets the size, which fixes a metric of every font.
    pub face_size_adjust: bool,
    /// pyro: whether the first of `variations` is the automatic `opsz` (CSS
    /// `font-optical-sizing: auto`): it follows a face's `size-adjust`, and a face's
    /// `font-variation-settings` descriptor overrides it.
    pub auto_opsz: bool,
    /// pyro: the OpenType language system tag to shape with instead of `language`'s (CSS
    /// `font-language-override`).
    pub language_override: Option<[u8; 4]>,
    /// pyro: how the item's fullwidth punctuation is trimmed (CSS `text-spacing-trim`).
    pub spacing_trim: SpacingTrim,
    /// pyro: whether an atomic inline (or an inline box edge with width) sits right before the
    /// item: its first character has no punctuation before it to be trimmed against, and no text
    /// before it to join with.
    pub isolated_before: bool,
    /// pyro: whether an atomic inline sits right after the item.
    pub isolated_after: bool,
}

/// The font instance to shape an item with.
#[derive(Clone, Debug, PartialEq)]
pub struct FontInstance {
    /// The font.
    pub font: FontData,
    /// Font synthesis suggestions.
    // TODO: Synthesis carries more than we need, and ties us to `fontique`. We can likely change
    // this to opaque user data.
    pub synthesis: fontique::Synthesis,
}

impl FontInstance {
    /// Borrow this font instance.
    #[inline]
    pub fn as_ref(&self) -> FontInstanceRef<'_> {
        FontInstanceRef {
            data: &self.font.data,
            index: self.font.index,
            synthesis: &self.synthesis,
        }
    }
}

/// A borrowed [`FontInstance`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FontInstanceRef<'a> {
    /// Blob containing the content of the font file.
    pub data: &'a Blob<u8>,
    /// Index of the font in a collection, or 0 for a single font.
    pub index: u32,
    /// Font synthesis suggestions.
    pub synthesis: &'a fontique::Synthesis,
}

impl From<FontInstanceRef<'_>> for FontInstance {
    #[inline]
    fn from(font: FontInstanceRef<'_>) -> Self {
        Self {
            font: FontData {
                data: font.data.clone(),
                index: font.index,
            },
            synthesis: *font.synthesis,
        }
    }
}

/// Reusable scratch to shape text using [`Self::shape_text`].
pub struct Shaper {
    /// Shaped runs by their shaping inputs (see [`with_glyph_cache`]); only
    /// without `std`, which keeps one per thread.
    #[cfg(not(feature = "std"))]
    glyph_cache: cache::GlyphCache,
    shape_data_cache: LruCache<cache::ShapeDataKey, harfrust::ShaperData>,
    shape_instance_cache: LruCache<cache::ShapeInstanceId, harfrust::ShaperInstance>,
    shape_plan_cache: LruCache<cache::ShapePlanId, harfrust::ShapePlan>,
    unicode_buffer: Option<harfrust::UnicodeBuffer>,
    features: Vec<harfrust::Feature>,
    char_cluster: CharCluster,
    /// pyro: what fullwidth punctuation trimming knows about fonts (`han_kerning::FontData`), by
    /// font blob, index and language.
    han_fonts: Vec<(u64, u32, Option<harfrust::Language>, han_kerning::FontData)>,
}

impl Default for Shaper {
    fn default() -> Self {
        const MAX_ENTRIES: usize = 16;
        Self {
            #[cfg(not(feature = "std"))]
            glyph_cache: cache::GlyphCache::default(),
            shape_data_cache: LruCache::new(MAX_ENTRIES),
            shape_instance_cache: LruCache::new(MAX_ENTRIES),
            shape_plan_cache: LruCache::new(MAX_ENTRIES),
            unicode_buffer: Some(harfrust::UnicodeBuffer::new()),
            features: Vec::new(),
            char_cluster: CharCluster::default(),
            han_fonts: Vec::new(),
        }
    }
}

impl core::fmt::Debug for Shaper {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Shaper").finish_non_exhaustive()
    }
}

impl Shaper {
    /// Shape text into glyphs, overwriting `shaped_text`.
    ///
    /// The `text` passed in must be the same as used for producing `analysis`.
    ///
    /// This uses the items returned by `items`, and further itemizes the text into
    /// individually-shapeable segments of constant bidi level and script. `items` should be used to
    /// split on properties like shaping-relevant style changes (e.g., font size) or properties like
    /// language.
    ///
    /// `char_style_indices` holds per-character style indices; these are copied onto
    /// [`Character`][super::Character`] and [`ShapedCluster`][super::ShapedCluster]. A shaped
    /// cluster's style index is that of its logically first constituent character.
    ///
    /// Characters that don't have a particular script have their script resolved based on
    /// surrounding context (see [`Segment::script`]).
    ///
    /// Each segment is then broken into runs of maximal sequences of character clusters for which
    /// `select_font` returns the same font.
    ///
    /// # Panics
    ///
    /// Panics if `items` does not cover the entire source text, or the font returned by
    /// `select_font` is malformed.
    pub fn shape_text<'options>(
        &mut self,
        text: &str,
        analysis: &Analysis,
        // TODO: rename to something like `user_data` (s.t. we don't assume it's a style per se).
        char_style_indices: &[u16],
        items: impl IntoIterator<Item = Item<'options>>,
        mut select_font: impl FontSelector,
        shaped_text: &mut ShapedText,
    ) {
        shaped_text.clear();
        shaped_text.reserve(text.len());

        let char_count = analysis.char_info().len();
        debug_assert_eq!(
            char_style_indices.len(),
            char_count,
            "The number of character style indices must be equal to the character count"
        );

        let mut previous_item_end = 0;
        let mut itemizer = analysis.itemize(text);

        for item in items {
            assert!(
                item.char_end > previous_item_end,
                "item ends must be strictly increasing"
            );
            assert!(
                item.char_end as usize <= char_count,
                "items must not span past the text"
            );

            let mut first_segment = true;
            loop {
                let segment = itemizer
                    .next(
                        #[inline(always)]
                        |text_range| text_range.char_range.end == item.char_end as usize,
                    )
                    .expect("A segment must be yielded, given items tile the full text exactly");

                let item_edges = (
                    mem::take(&mut first_segment),
                    segment.range.char_range.end == item.char_end as usize,
                );
                if shape_segment(
                    self,
                    text,
                    &segment,
                    item_edges,
                    &item.options,
                    &mut select_font,
                    analysis.char_info(),
                    char_style_indices,
                    shaped_text,
                )
                .is_err()
                {
                    // Abort on error. This happens iff `FontSelector::select_font` failed to return a
                    // font. By aborting we ensure `ShapedText` covers the source text contiguously (as
                    // we need a font to construct `ShapedRun`).
                    return;
                };

                debug_assert!(
                    segment.range.char_range.end <= item.char_end as usize,
                    "Segments must not span past the item."
                );
                if segment.range.char_range.end == item.char_end as usize {
                    break;
                }
            }

            previous_item_end = item.char_end;
        }

        assert_eq!(
            previous_item_end as usize, char_count,
            "`items` does not cover the entire source text"
        );
        debug_assert_eq!(
            shaped_text.characters().len(),
            char_count,
            "All characters must have been processed"
        );
    }
}

/// Shape one segment.
///
/// Returns `Err(())` if shaping should be aborted, which happens iff [`FontSelector::select_font`]
/// returned `None`.
fn shape_segment(
    scx: &mut Shaper,
    text: &str,
    segment: &Segment,
    (at_item_start, at_item_end): (bool, bool),
    options: &ShapeOptions<'_>,
    select_font: &mut impl FontSelector,
    char_info: &[CharInfo],
    char_style_indices: &[u16],
    shaped_text: &mut ShapedText,
) -> Result<(), ()> {
    select_font.begin_segment(segment, options);

    let text_range = &segment.range.byte_range;
    let char_range = &segment.range.char_range;

    let segment_text = &text[text_range.clone()];

    // Only process current segment
    let segment_char_info = &char_info[char_range.start..char_range.end];
    let segment_char_style_indices = &char_style_indices[char_range.start..char_range.end];

    if segment_text.is_empty() {
        return Ok(()); // No clusters
    }

    let char_cluster = &mut scx.char_cluster;

    // Shaping inputs that are constant over the segment.
    let direction = if segment.bidi_level.is_rtl() {
        harfrust::Direction::RightToLeft
    } else {
        harfrust::Direction::LeftToRight
    };
    let hb_script = script_to_harfrust(segment.script);
    // pyro: `font-language-override` names the OpenType language system itself (harfrust takes
    // it as the private-use subtag `x-hbot<tag>`, upper-cased). Registered tags are upper case: one
    // with a lower-case letter (`"trk"`) matches none, which is the script's default system.
    let language = match options.language_override {
        Some(tag) if tag.iter().any(u8::is_ascii_lowercase) => None,
        Some(tag) => core::str::from_utf8(&tag).ok().and_then(|tag| {
            alloc::format!("x-hbot{}", tag.trim_end().to_ascii_lowercase())
                .parse::<harfrust::Language>()
                .ok()
        }),
        None => options
            .language
            .as_ref()
            .and_then(|lang| lang.as_str().parse::<harfrust::Language>().ok()),
    };
    scx.features.clear();
    scx.features.extend(options.features.iter().map(|feature| {
        harfrust::Feature::new(
            harfrust::Tag::new(&feature.tag.to_bytes()),
            feature.value as u32,
            ..,
        )
    }));

    // Shape a run of the segment with a single font, appending it to `shaped_text`.
    // pyro: `before`/`after`: the fonts of the runs around this one in the segment (whose
    // characters fullwidth punctuation trimming classes by their own font).
    let mut shape_run = |font: &FontInstance,
                         range: TextRange,
                         before_font: Option<&FontInstance>,
                         after_font: Option<&FontInstance>| {
        // pyro: a font its `@font-face` rule sizes (`size-adjust`) is shaped, measured and drawn
        // at the item's size times that factor.
        // pyro: and a face's `@font-face` features and variation settings
        // (`font-feature-settings`, `font-variation-settings` descriptors) apply under those of
        // the properties, over the automatic `opsz` (CSS Fonts 4 §7.2), which follows the
        // face's size.
        let face = font.synthesis.face_settings();
        let adjust = if options.face_size_adjust { font.synthesis.size_adjust() } else { 1.0 };
        let face_variations = face.as_ref().is_some_and(|face| !face.variations.is_empty());
        let auto_opsz_scaled = adjust != 1.0 && options.auto_opsz && !options.variations.is_empty();
        let variations: alloc::vec::Vec<FontVariation>;
        let adjusted;
        let options = if adjust != 1.0 || face_variations {
            let (auto, rest) = if options.auto_opsz && !options.variations.is_empty() {
                (&options.variations[..1], &options.variations[1..])
            } else {
                (&[][..], options.variations)
            };
            variations = if face_variations || auto_opsz_scaled {
                auto.iter()
                    .map(|v| FontVariation { tag: v.tag, value: v.value * adjust })
                    .chain(face.iter().flat_map(|face| {
                        face.variations.iter().map(|(tag, value)| FontVariation {
                            tag: parlance::Tag::from_bytes(*tag),
                            value: *value,
                        })
                    }))
                    .chain(rest.iter().copied())
                    .collect()
            } else {
                options.variations.to_vec()
            };
            adjusted = ShapeOptions {
                font_size: options.font_size * adjust,
                point_size: options.point_size * adjust,
                language: options.language.clone(),
                features: options.features,
                variations: &variations,
                face_size_adjust: options.face_size_adjust,
                auto_opsz: false,
                language_override: options.language_override,
                spacing_trim: options.spacing_trim,
                isolated_before: options.isolated_before,
                isolated_after: options.isolated_after,
            };
            &adjusted
        } else {
            options
        };
        let face_features: alloc::vec::Vec<harfrust::Feature>;
        let features: &[harfrust::Feature] = match &face {
            Some(face) if !face.features.is_empty() => {
                face_features = face
                    .features
                    .iter()
                    .map(|(tag, value)| harfrust::Feature::new(harfrust::Tag::new(tag), u32::from(*value), ..))
                    .chain(scx.features.iter().cloned())
                    .collect();
                &face_features
            }
            _ => &scx.features,
        };
        // pyro: the font is parsed only when something has to be built from it (shaper
        // data, instance, or the shaper of a run the glyph cache does not hold); a cached run needs
        // none of them, and its variation coords are the instance's.
        // TODO: How do we want to handle errors like this?
        let font_ref =
            || harfrust::FontRef::from_index(font.font.data.as_ref(), font.font.index).unwrap();

        // Create harfrust shaper
        let shaper_data = scx.shape_data_cache.entry(
            cache::ShapeDataKey::new(font.font.data.id(), font.font.index),
            || harfrust::ShaperData::new(&font_ref()),
        );
        let instance = scx.shape_instance_cache.entry(
            cache::ShapeInstanceKey::new(
                font.font.data.id(),
                font.font.index,
                &font.synthesis,
                Some(options.variations),
            ),
            || {
                harfrust::ShaperInstance::from_variations(
                    &font_ref(),
                    variations_iter(&font.synthesis, options.variations),
                )
            },
        );

        // Use the entire run text including newlines.
        let run_text = &text[range.byte_range.clone()];
        let pre_context = context_before(&text[..range.byte_range.start]);
        let post_context = context_after(&text[range.byte_range.end..]);

        // pyro: fullwidth punctuation next to punctuation is shaped with the font's `halt`
        // (CSS `text-spacing-trim`); the plan takes the feature without its ranges.
        let trim = (options.spacing_trim != SpacingTrim::None
            && !segment.bidi_level.is_rtl()
            && run_text.chars().any(han_kerning::is_candidate))
        .then(|| {
            let data = han_data(&mut scx.han_fonts, font, language.as_ref(), options).clone();
            // The characters around the run, classed by the font that draws them (Blink classes
            // them by the run's own font, which leaves fallback fonts' punctuation alone).
            let before = (!(range.byte_range.start == text_range.start && at_item_start && options.isolated_before))
                .then(|| text[..range.byte_range.start].chars().next_back())
                .flatten()
                .map(|ch| match before_font {
                    Some(other) if other != font => {
                        han_data(&mut scx.han_fonts, other, language.as_ref(), options).char_type(ch)
                    }
                    _ => data.char_type(ch),
                });
            let after = (!(range.byte_range.end == text_range.end && at_item_end && options.isolated_after))
                .then(|| text[range.byte_range.end..].chars().next())
                .flatten()
                .map(|ch| match after_font {
                    Some(other) if other != font => {
                        han_data(&mut scx.han_fonts, other, language.as_ref(), options).char_type(ch)
                    }
                    _ => data.char_type(ch),
                });
            han_kerning::RunTrim::new(options.spacing_trim, &data, run_text, before, after)
        })
        .flatten();
        let mut run_features;
        let mut plan_features;
        let (run_features, plan_features) = match &trim {
            Some(trim) if trim.trimmed().next().is_some() => {
                let halt = harfrust::Tag::new(b"halt");
                run_features = features.to_vec();
                plan_features = features.to_vec();
                // The fields directly: `Feature::new` stores an exclusive end minus one, which
                // `set_masks` then reads as exclusive (`i..i + 1` would select nothing).
                run_features.extend(trim.trimmed().map(|i| harfrust::Feature {
                    tag: halt,
                    value: 1,
                    start: i as u32,
                    end: i as u32 + 1,
                }));
                plan_features.push(harfrust::Feature { tag: halt, value: 1, start: 0, end: 1 });
                (&run_features[..], &plan_features[..])
            }
            _ => (features, features),
        };

        let glyph_key = cache::GlyphKey {
            font_blob_id: font.font.data.id(),
            font_index: font.font.index,
            synthesis: &font.synthesis,
            variations: options.variations,
            features: run_features,
            direction,
            script: hb_script,
            language: language.as_ref(),
            font_size: options.font_size,
            point_size: options.point_size,
            text: run_text,
            pre_context,
            post_context,
        };
        let cached = with_glyph_cache!(scx, |cache: &mut cache::GlyphCache| {
            let (glyphs, metrics) = cache.get(&glyph_key)?;
            shaped_text.push_run(
                text,
                range.clone(),
                segment,
                options,
                char_info,
                char_style_indices,
                font,
                glyphs,
                instance.coords(),
                Some(metrics),
                trim.as_ref(),
            );
            Some(())
        });
        if cached.is_some() {
            return;
        }
        let font_ref = font_ref();
        let harf_shaper = shaper_data
            .shaper(&font_ref)
            .instance(Some(instance))
            .build();
        let shaper_plan = scx.shape_plan_cache.entry(
            cache::ShapePlanKey::new(
                font.font.data.id(),
                font.font.index,
                &font.synthesis,
                direction,
                hb_script,
                language.clone(),
                plan_features,
                Some(options.variations),
            ),
            || {
                harfrust::ShapePlan::new(
                    &harf_shaper,
                    direction,
                    Some(hb_script),
                    language.as_ref(),
                    plan_features,
                )
            },
        );

        // Prepare harfrust buffer
        let mut buffer = mem::take(&mut scx.unicode_buffer).unwrap();
        buffer.clear();
        buffer.set_cluster_level(harfrust::BufferClusterLevel::MonotoneCharacters);

        buffer.reserve(run_text.len());
        #[expect(clippy::cast_possible_truncation, reason = "Deferred")]
        for (i, ch) in run_text.chars().enumerate() {
            // Ensure that each cluster's index matches the index into `infos`. This is required
            // for efficient cluster lookup within `data.rs`.
            //
            // In other words, instead of using `buffer.push_str`, which iterates `run_text`
            // with `char_indices`, push each char individually via `.chars` with a cluster index
            // that matches its `infos` counterpart. This allows us to lookup `infos` via cluster
            // index in `data.rs`.
            // pyro: a tab is shaped as a space, as in browsers: it draws nothing, and its advance
            // is the line breaker's to set (the tab stop it reaches).
            // pyro: a control character other than a tab or a line break is visible, as in
            // browsers (CSS Text 3 § 4.1.1): it is shaped as a code point no font maps, so it
            // draws the font's missing-glyph box even where the font maps it to an empty glyph
            // (TrueType fonts map U+0008 and U+001D to `.null`).
            // pyro: a no-break space is shaped as a space, as Blink normalizes it before shaping:
            // it kerns as the space does (macOS Times kerns space-T, not no-break-space-T).
            let ch = match ch {
                // pyro: and so are the line and paragraph separators, which CSS displays as a
                // space (WPT css-fonts `separators`): in the space's font, not a fallback's.
                '\t' | '\u{a0}' | '\u{2028}' | '\u{2029}' => ' ',
                '\u{0}'..='\u{8}' | '\u{b}' | '\u{c}' | '\u{e}'..='\u{1f}' | '\u{7f}'..='\u{84}' | '\u{86}'..='\u{9f}' => '\u{FFFF}',
                ch => ch,
            };
            buffer.add(ch, i as u32);
        }

        // pyro: no context across an isolating box at the item's edges (`ShapeOptions::isolated_before`).
        let pre = if range.byte_range.start == text_range.start && at_item_start && options.isolated_before {
            ""
        } else {
            &text[..range.byte_range.start]
        };
        let post = if range.byte_range.end == text_range.end && at_item_end && options.isolated_after {
            ""
        } else {
            &text[range.byte_range.end..]
        };
        buffer.set_pre_context(pre);
        buffer.set_post_context(post);
        buffer.set_direction(direction);
        buffer.set_script(hb_script);

        if let Some(lang) = &language {
            buffer.set_language(lang.clone());
        }

        let glyph_buffer = harf_shaper.shape(
            buffer,
            HarfShapeOptions::new()
                .plan(Some(shaper_plan))
                .features(run_features)
                .point_size(Some(options.point_size)),
        );

        let metrics = shaped_text.push_run(
            text,
            range,
            segment,
            options,
            char_info,
            char_style_indices,
            font,
            &glyph_buffer,
            harf_shaper.coords(),
            None,
            trim.as_ref(),
        );

        // The glyphs stay in the cache; the next run gets a new buffer.
        with_glyph_cache!(scx, |cache: &mut cache::GlyphCache| cache.insert(
            &glyph_key,
            glyph_buffer,
            metrics
        ));
        scx.unicode_buffer = Some(harfrust::UnicodeBuffer::new());
    };

    // This is the run being accumulated, i.e., consecutive graphemes for which `select_font`
    // returns the same font.
    let mut run: Option<(FontInstance, TextRange)> = None;
    // pyro: the font of the run shaped before it in the segment.
    let mut before_font: Option<FontInstance> = None;

    // The segment's characters with their analysis data and style indices.
    let mut chars = segment_text
        .chars()
        .zip(segment_char_info)
        .zip(segment_char_style_indices)
        .map(|((ch, &info), &style_index)| (ch, info, style_index));

    // The lengths of the segment's graphemes in characters.
    let grapheme_lengths = segment_char_info
        .chunk_by(|_, info| !info.is_grapheme_start())
        .map(<[_]>::len);

    // The end of the previous grapheme.
    let mut byte_end = text_range.start;
    let mut char_end = char_range.start;
    // pyro: per font of the segment, whether it shapes word by word (`shapes_word_by_word`).
    let mut word_fonts: Vec<(u64, u32, bool)> = Vec::new();

    for grapheme_length in grapheme_lengths {
        let (byte_start, char_start) = (byte_end, char_end);
        char_cluster.fill(chars.by_ref().take(grapheme_length), &mut byte_end);
        char_end += grapheme_length;
        let grapheme = TextRange {
            byte_range: byte_start..byte_end,
            char_range: char_start..char_end,
        };

        let font = select_font
            .select_font(segment, options, char_cluster)
            .ok_or(())?;

        // pyro: Blink shapes word by word (a space or no-break space alone, each word between
        // them alone) when the font's OpenType lookups never involve the space glyph, so a font
        // with only legacy `kern`/`morx` tables (Times, among the macOS fonts) never kerns
        // across a space. Such a run is broken at every space boundary.
        let is_space = |range: &TextRange| matches!(&text[range.byte_range.clone()], " " | "\u{a0}");
        let word_break = run.as_ref().is_some_and(|(run_font, run_range)| {
            run_font.as_ref() == font
                && {
                    let last = text[run_range.byte_range.clone()].chars().next_back();
                    matches!(last, Some(' ' | '\u{a0}')) || is_space(&grapheme)
                }
                && shapes_word_by_word(&mut word_fonts, run_font)
        });

        if let Some((run_font, run_range)) = &mut run
            && run_font.as_ref() == font
            && !word_break
        {
            run_range.byte_range.end = byte_end;
            run_range.char_range.end = char_end;
        } else {
            if let Some((run_font, run_range)) = run.take() {
                // The font changed: shape the previous run.
                let after_font: FontInstance = font.into();
                shape_run(&run_font, run_range, before_font.as_ref(), Some(&after_font));
                before_font = Some(run_font);
            }
            run = Some((font.into(), grapheme));
        }
    }
    if let Some((run_font, run_range)) = run {
        shape_run(&run_font, run_range, before_font.as_ref(), None);
    }

    Ok(())
}

/// pyro: whether Blink shapes text in `font` word by word (`Font::CanShapeWordByWord`): its
/// OpenType lookups never take the space glyph. Only fonts without `GSUB` and `GPOS` are
/// recognized (their lookups take nothing); `fonts` keeps the answers of the segment.
fn shapes_word_by_word(fonts: &mut Vec<(u64, u32, bool)>, font: &FontInstance) -> bool {
    let (id, index) = (font.font.data.id(), font.font.index);
    if let Some(&(.., known)) = fonts.iter().find(|(i, x, _)| *i == id && *x == index) {
        return known;
    }
    let answer = harfrust::FontRef::from_index(font.font.data.as_ref(), index).is_ok_and(|font_ref| {
        use skrifa::raw::TopLevelTable as _;
        font_ref.table_data(skrifa::raw::tables::gsub::Gsub::TAG).is_none()
            && font_ref.table_data(skrifa::raw::tables::gpos::Gpos::TAG).is_none()
            // A legacy `kern` table kerns across spaces in Chrome 147 (macOS Times and Helvetica:
            // lines measure as v0.0.41's), so those fonts keep shaping whole runs.
            && font_ref.table_data(skrifa::raw::types::Tag::new(b"kern")).is_none()
    });
    fonts.push((id, index, answer));
    answer
}

/// pyro: the trimming data of `font` in `language` (`han_font_data`), from `fonts` or probed
/// and kept there.
fn han_data<'a>(
    fonts: &'a mut Vec<(u64, u32, Option<harfrust::Language>, han_kerning::FontData)>,
    font: &FontInstance,
    language: Option<&harfrust::Language>,
    options: &ShapeOptions<'_>,
) -> &'a han_kerning::FontData {
    let found = fonts.iter().position(|(id, index, lang, _)| {
        *id == font.font.data.id() && *index == font.font.index && lang.as_ref() == language
    });
    let index = found.unwrap_or_else(|| {
        let data = match harfrust::FontRef::from_index(font.font.data.as_ref(), font.font.index) {
            Ok(font_ref) => {
                let instance = harfrust::ShaperInstance::from_variations(
                    &font_ref,
                    variations_iter(&font.synthesis, options.variations),
                );
                han_font_data(&font_ref, &instance, language, options.point_size)
            }
            Err(_) => han_kerning::FontData::NONE,
        };
        if fonts.len() >= 32 {
            fonts.clear();
        }
        fonts.push((font.font.data.id(), font.font.index, language.cloned(), data));
        fonts.len() - 1
    });
    &fonts[index].3
}

/// pyro: what fullwidth punctuation trimming needs to know about `font` in `language`: whether
/// it has `halt`, and what `halt` does to the punctuation whose kind depends on the glyph, each
/// shaped alone in Han script (for which CJK fonts' `locl` picks the language's form).
fn han_font_data(
    font: &harfrust::FontRef<'_>,
    instance: &harfrust::ShaperInstance,
    language: Option<&harfrust::Language>,
    point_size: f32,
) -> han_kerning::FontData {
    use skrifa::raw::TableProvider as _;
    let halt = skrifa::raw::types::Tag::new(b"halt");
    let has_halt = font
        .gpos()
        .ok()
        .and_then(|gpos| gpos.feature_list().ok())
        .is_some_and(|list| list.feature_records().iter().any(|record| record.feature_tag() == halt));
    let data = harfrust::ShaperData::new(font);
    let shaper = data.shaper(font).instance(Some(instance)).build();
    // Blink shapes them in the locale's script for Han, whatever the run's script.
    let script = harfrust::script::HAN;
    let mut buffer = Some(harfrust::UnicodeBuffer::new());
    han_kerning::FontData::new(has_halt, |ch, with_halt| {
        let mut unicode = buffer.take().unwrap_or_default();
        unicode.clear();
        unicode.add(ch, 0);
        unicode.set_direction(harfrust::Direction::LeftToRight);
        unicode.set_script(script);
        if let Some(language) = language {
            unicode.set_language(language.clone());
        }
        let features = [harfrust::Feature::new(harfrust::Tag::new(b"halt"), u32::from(with_halt), ..)];
        let glyphs = shaper.shape(
            unicode,
            HarfShapeOptions::new().features(if with_halt { &features } else { &[] }).point_size(Some(point_size)),
        );
        let found = match (glyphs.glyph_infos(), glyphs.glyph_positions()) {
            ([info], [position]) if info.glyph_id != 0 => {
                Some((position.x_advance as f32, position.x_offset as f32))
            }
            _ => None,
        };
        buffer = Some(glyphs.clear());
        found
    })
}

/// Up to harfrust's context length (5 characters) of `text`'s end.
fn context_before(text: &str) -> &str {
    let start = text.char_indices().rev().nth(4).map_or(0, |(i, _)| i);
    &text[start..]
}

/// Up to harfrust's context length (5 characters) of `text`'s start.
fn context_after(text: &str) -> &str {
    let end = text.char_indices().nth(5).map_or(text.len(), |(i, _)| i);
    &text[..end]
}

#[inline]
fn variations_iter<'a>(
    synthesis: &'a fontique::Synthesis,
    item: &'a [FontVariation],
) -> impl Iterator<Item = harfrust::Variation> + 'a {
    synthesis
        .variation_settings()
        .iter()
        .map(|(tag, value)| harfrust::Variation {
            tag: *tag,
            value: *value,
        })
        .chain(item.iter().map(|variation| harfrust::Variation {
            tag: harfrust::Tag::new(&variation.tag.to_bytes()),
            value: variation.value,
        }))
}

pub(crate) fn script_to_harfrust(script: fontique::Script) -> harfrust::Script {
    harfrust::Script::from_iso15924_tag(harfrust::Tag::new(&script.to_bytes()))
        .unwrap_or(harfrust::script::UNKNOWN)
}

/// Implements font selection for shaping.
pub trait FontSelector {
    /// Called when a new segment starts.
    ///
    /// This can be useful to inspect, e.g., the segment's script.
    fn begin_segment(&mut self, segment: &Segment, options: &ShapeOptions<'_>) {
        let _ = (segment, options);
    }

    /// Called once per character cluster within the current segment.
    ///
    /// A character cluster will usually be a grapheme, though if text direction or script changes
    /// mid-grapheme, it will be split over segments.
    ///
    /// Shaping is aborted if this returns `None`; [`ShapedText`] then contains a partial result.
    fn select_font(
        &mut self,
        segment: &Segment,
        options: &ShapeOptions<'_>,
        cluster: &mut CharCluster,
    ) -> Option<FontInstanceRef<'_>>;
}
