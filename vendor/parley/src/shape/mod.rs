// Copyright 2021 the Parley Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Text shaping implementation using `harfrust`for shaping
//! and `icu` for text analysis.

mod autospace;
mod upright;
mod justify;

use alloc::vec::Vec;
use parley_engine::shape::{CharCluster, Coverage};
use parley_engine::{
    Analysis, AnalysisDataSources, FontInstance, FontInstanceRef, ShapeOptions, Shaper,
};
use smallvec::SmallVec;

use super::layout::Layout;
use super::resolve::{ResolveContext, ResolvedStyle};
use super::style::{Brush, FontFeature};
use crate::inline_box::LayoutInlineBox;
use crate::InlineBoxKind;
use crate::util::{nearly_eq, nearly_zero};
use crate::{FontContext, FontData, Spacing};

use fontique::{self, Query, QueryFamily, QueryFont};
use parlance::{BidiLevel, GenericFamily, Tag};

/// If these font features are passed to the shaper, optional ligatures are not applied.
///
/// Required ligation such as is common in cursive scripts' fonts is still applied.
///
/// Following [CSS Text 3 § 7.2][css-letter-spacing], pass these features if you are going to apply
/// something like `letter-spacing`. Unfortunately, that specification doesn't say which font
/// features specifically should be disabled. Here, we disable the OpenType features for "common",
/// "discretionary" and "historic" ligatures as given by [CSS Fonts 4 § 6.4][css-fonts-ligatures].
///
/// As of writing, this list matches Gecko. Blink additionally disables contextual alternates
/// (`calt`), but that's perhaps not desirable: disabling contextual alternates potentially makes
/// text render wrong.
///
/// [css-letter-spacing]: https://www.w3.org/TR/css-text-3/#letter-spacing-property
/// [css-fonts-ligatures]: https://www.w3.org/TR/css-fonts-4/#font-variant-ligatures-prop
const OPTIONAL_LIGATURES_OFF: [FontFeature; 4] = [
    FontFeature::new(Tag::new(b"liga"), 0),
    FontFeature::new(Tag::new(b"clig"), 0),
    FontFeature::new(Tag::new(b"dlig"), 0),
    FontFeature::new(Tag::new(b"hlig"), 0),
];

pub(crate) fn shape_text<'a, B: Brush>(
    rcx: &'a ResolveContext,
    fcx: &'a mut FontContext,
    styles: &'a [ResolvedStyle<B>],
    inline_boxes: &[LayoutInlineBox],
    analysis: &Analysis,
    char_style_indices: &[u16],
    scx: &mut Shaper,
    mut text: &str,
    layout: &mut Layout<B>,
    analysis_data_sources: &AnalysisDataSources,
    first_fonts: &mut Vec<Option<FontInstance>>,
) {
    // pyro: one slot per style for the first available font the selector saw.
    first_fonts.clear();
    first_fonts.resize(styles.len(), None);

    // If we have both empty text and no inline boxes, shape with a fake space
    // to generate metrics that can be used to size a cursor.
    if text.is_empty() && inline_boxes.is_empty() {
        text = " ";
    }

    // Do nothing if there is no text or styles (there should always be a default style)
    if text.is_empty() || styles.is_empty() {
        // Process any remaining inline boxes whose index is greater than the length of the text
        for box_idx in 0..inline_boxes.len() {
            // Push the box to the list of items
            layout.data.push_inline_box(box_idx, BidiLevel::new(0));
        }
        return;
    }

    let mut fq = fcx.collection.query(&mut fcx.source_cache);

    let mut inline_box_iter = inline_boxes.iter().map(|b| &b.inline_box).peekable();

    // Merge font features with letter-spacing ligature suppression.
    //
    // TODO: This allocation is slightly unfortunate (though solvable). It's required currently,
    // because the iterator providing `ShapeOptions` has to provide a borrowed `&'a [FontFeature]`,
    // which cannot be tied to the lifetime of the call to `Iterator::next`. What we'd need is a
    // lending iterator (i.e., we probably just need to let `parley_engine` take some trait
    // providing the items).
    let style_features: &'_ Vec<SmallVec<[FontFeature; 8]>> = &styles
        .iter()
        .map(|style| {
            let style_features = rcx.features(style.font_features).unwrap_or(&[]);
            if !nearly_zero(style.letter_spacing) {
                // Later values override earlier values.
                OPTIONAL_LIGATURES_OFF
                    .into_iter()
                    .chain(style_features.iter().copied())
                    .collect()
            } else {
                style_features.iter().copied().collect()
            }
        })
        .collect();
    // pyro: upright glyphs in a vertical line take the font's vertical alternates (`vert`, as
    // HarfBuzz enables it shaping top to bottom, CSS Fonts 4 § 7.1 default features); the
    // style's own settings come after it, so `"vert" 0` turns it off (WPT css-fonts
    // `font-default-04`).
    let upright_features: &'_ Vec<SmallVec<[FontFeature; 8]>> = &style_features
        .iter()
        .map(|features| {
            core::iter::once(FontFeature::new(Tag::new(b"vert"), 1))
                .chain(features.iter().copied())
                .collect()
        })
        .collect();

    // pyro: the layout's scale (set before shaping), to shape with the unscaled size as the point size.
    let scale = layout.data.scale;

    // Split when shaping-relevant style properties change and at inline boxes.
    let items = {
        // TODO: we currently walk characters here, but we could instead just walk boundaries of
        // styles and inline boxes.
        let char_count = char_style_indices.len();

        // The first character of the item currently being built.
        let mut item_start_char = 0_usize;

        // Positioned at `item_start_char`, i.e., the first character not yet covered by an item.
        let mut chars = text.char_indices().enumerate().peekable();
        // pyro: whether the item being built follows an atomic inline box.
        let mut after_atomic_box = false;
        core::iter::from_fn(move || {
            if item_start_char == char_count {
                return None;
            }

            let item_style_index = char_style_indices[item_start_char];
            let item_style = &styles[usize::from(item_style_index)];

            // Items are at least one character long, therefore the item's own first character is
            // never a split point.
            // pyro: in a vertical line (not upright throughout) an item keeps one vertical
            // orientation, which paint draws a run by (`upright_in_vertical`).
            let item_upright = chars.next().map(|(_, (_, c))| crate::upright_in_vertical(c));

            let char_end = 'item: loop {
                let Some(&(char_index, (byte_index, _))) = chars.peek() else {
                    // End of text.
                    break (char_count, false);
                };

                // Split at inlines boxes, so each box falls on a shaping boundary.
                //
                // We loop because there may be multiple boxes at this index.
                let mut split = false;
                let mut atomic = false;
                while let Some(inline_box) = inline_box_iter.peek() {
                    if inline_box.index < byte_index {
                        // Inline boxes *before* this index are popped (this occurs if the itemizer
                        // split a run and we were not called, such as at a bidi boundary).
                        inline_box_iter.next();
                    } else if inline_box.index == byte_index {
                        // pyro: an atomic inline, or the edge of an inline box with a margin,
                        // border or padding, breaks the shaping context: no joining across it
                        // (CSS Text 3 § 7.3, WPT css-text `shaping-009`).
                        atomic |= inline_box.kind == InlineBoxKind::InFlow
                            || (matches!(inline_box.kind, InlineBoxKind::SpanStart | InlineBoxKind::SpanEnd)
                                && inline_box.width > 0.0);
                        inline_box_iter.next();
                        split = true;
                    } else {
                        break;
                    }
                }

                if split {
                    break 'item (char_index, atomic);
                }

                let style_index = char_style_indices[char_index];
                if style_index != item_style_index {
                    let style = &styles[usize::from(style_index)];
                    split = !nearly_eq(style.font_size, item_style.font_size)
                        || style.font_size_adjusted != item_style.font_size_adjusted
                        || style.auto_optical_size != item_style.auto_optical_size
                        || style.language_override != item_style.language_override
                        || style.locale != item_style.locale
                        || style.font_variations != item_style.font_variations
                        || style.font_features != item_style.font_features
                        || !nearly_eq(style.letter_spacing, item_style.letter_spacing)
                        || !nearly_eq(style.word_spacing, item_style.word_spacing)
                        || spacing_trim(style) != spacing_trim(item_style)
                        // pyro: and where a span shifted by `vertical-align` starts or ends
                        // (CSS Text 3 § 7.3, WPT css-text `boundary-shaping-002/006`).
                        || shifted_span(styles, style_index) != shifted_span(styles, item_style_index)
                        // pyro: and at the edges of a bidi isolate (CSS Text 3 § 7.3, WPT css-text
                        // `boundary-shaping-008`), where HarfBuzz would ligate across.
                        || isolating_span(styles, style_index) != isolating_span(styles, item_style_index);
                }

                // (Only at a grapheme cluster's start: a cluster takes its base's orientation,
                // UTR #50, so a ZWJ sequence or a variation selector stays with it.)
                if !split
                    && item_style.vertical
                    && !item_style.upright
                    && analysis.char_info().get(char_index).is_some_and(|info| info.is_grapheme_start())
                    && let Some(&(_, (_, c))) = chars.peek()
                {
                    split = Some(crate::upright_in_vertical(c)) != item_upright;
                }

                if split {
                    break 'item (char_index, false);
                }

                chars.next();
            };
            let (char_end, isolated_after) = char_end;

            item_start_char = char_end;
            let isolated_before = core::mem::replace(&mut after_atomic_box, isolated_after);
            Some(parley_engine::itemize::Item {
                char_end: char_end as u32,
                options: ShapeOptions {
                    language: item_style.locale,
                    font_size: item_style.font_size,
                    // pyro: tracking follows the unscaled size (styles hold sizes times the scale).
                    point_size: if scale > 0.0 { item_style.font_size / scale } else { item_style.font_size },
                    features: if item_style.vertical && (item_style.upright || item_upright == Some(true)) {
                        &upright_features[item_style_index as usize]
                    } else {
                        &style_features[item_style_index as usize]
                    },
                    variations: rcx.variations(item_style.font_variations).unwrap_or(&[]),
                    face_size_adjust: !item_style.font_size_adjusted,
                    auto_opsz: item_style.auto_optical_size,
                    language_override: item_style.language_override,
                    spacing_trim: spacing_trim(item_style),
                    isolated_before,
                    isolated_after,
                },
            })
        })
    };

    let font_selector =
        FontSelector::new(&mut fq, rcx, styles, analysis_data_sources, first_fonts);

    scx.shape_text(
        text,
        analysis,
        char_style_indices,
        items,
        font_selector,
        &mut layout.data.shaped_text,
    );

    let box_levels = inline_box_levels(text, analysis, inline_boxes);
    let mut inline_box_iter = inline_boxes
        .iter()
        .map(|b| &b.inline_box)
        .enumerate()
        .peekable();
    for shaped_run_idx in 0..layout.data.shaped_text.runs().len() {
        let shaped_run = &layout.data.shaped_text.runs()[shaped_run_idx];
        let run_text_byte_start = shaped_run.range.byte_range.start;
        let run_style_index = char_style_indices[shaped_run.range.char_range.start];
        let run_style = &styles[usize::from(run_style_index)];
        let run_chars = shaped_run.range.char_range.clone();

        // Push inline boxes positioned before the start of this item. pyro: each with its
        // own bidi level (`inline_box_levels`).
        while let Some((box_idx, inline_box)) = inline_box_iter.peek() {
            if inline_box.index <= run_text_byte_start {
                layout.data.push_inline_box(*box_idx, box_levels[*box_idx]);
                inline_box_iter.next();
            } else {
                break;
            }
        }

        layout.data.process_shaped_run(
            shaped_run_idx,
            // TODO: should we get the *item's* style here?
            //
            // Using the run's style for word and letter spacing is probably correct (and in fact,
            // we probably shouldn't itemize on them; we should just ensure we don't ligate if
            // they're non-zero).
            run_style,
            // pyro: no letter-spacing in a run of a cursive script (CSS Text 3 § 8.2: it would
            // break the letters' joins), as in Blink. A run is of one script; its own letters say
            // which (characters of no script take their neighbours').
            Spacing::new(
                run_style.word_spacing,
                match analysis.char_info()[run_chars].iter().find_map(|info| info.cursive_script()) {
                    Some(true) => 0.0,
                    _ => run_style.letter_spacing,
                },
            )
            .with_scales(run_style.spacing_scales),
            char_style_indices,
        );
    }

    // Process any remaining inline boxes whose index is greater than the length of the text
    for (box_idx, _inline_box) in inline_box_iter {
        layout.data.push_inline_box(box_idx, box_levels[box_idx]);
    }

    // pyro: CSS `text-orientation: upright`.
    upright::apply(layout, styles, text);
    // pyro: CSS `text-autospace`.
    autospace::apply(layout, text, styles, inline_boxes);
    // pyro: CSS `text-justify`.
    justify::apply(layout, text, styles, inline_boxes);
    // pyro: CSS `hanging-punctuation` (the line breaker's `end_hang`).
    layout.data.hang_classes.clear();
    if styles.iter().any(|style| style.hanging_punctuation != 0) {
        layout.data.hang_classes.extend(text.chars().map(|ch| {
            let stop = matches!(
                ch,
                ',' | '.' | '\u{060C}' | '\u{06D4}' | '\u{3001}' | '\u{3002}' | '\u{FF0C}' | '\u{FF0E}' | '\u{FE50}'
                    | '\u{FE51}' | '\u{FE52}' | '\u{FF61}' | '\u{FF64}'
            );
            u8::from(parley_engine::hangs_at_edge(ch, true))
                | (u8::from(parley_engine::hangs_at_edge(ch, false)) << 1)
                | (u8::from(stop) << 2)
        }));
        let clusters = layout.data.shaped_text.shaped_clusters();
        layout.data.last_content_cluster = clusters
            .iter()
            .rposition(|cluster| cluster.whitespace() == parley_engine::shape::Whitespace::None)
            .unwrap_or(0) as u32;
    }
}

/// pyro: how the shaper trims a style's fullwidth punctuation (CSS `text-spacing-trim`): the
/// line edges are the line breaker's.
fn spacing_trim<B: Brush>(style: &ResolvedStyle<B>) -> parley_engine::shape::SpacingTrim {
    use crate::style::TextSpacingTrim;
    use parley_engine::shape::SpacingTrim;
    match style.text_spacing_trim {
        TextSpacingTrim::SpaceAll => SpacingTrim::None,
        TextSpacingTrim::TrimAll => SpacingTrim::All,
        TextSpacingTrim::Normal
        | TextSpacingTrim::SpaceFirst
        | TextSpacingTrim::TrimStart
        | TextSpacingTrim::TrimBoth => SpacingTrim::Adjacent,
    }
}

/// pyro: the bidi level of each inline box. An atomic or out-of-flow box takes the level it
/// resolved to as U+FFFC (`Analysis::object_levels`); a span's start edge takes the level of what
/// follows it and its end edge that of what precedes it (the edges sit inside the span's bidi
/// formatting characters), characters that rule X9 removes and other edges aside. All at the
/// paragraph's level when the paragraph needed no resolution.
fn inline_box_levels(text: &str, analysis: &Analysis, inline_boxes: &[LayoutInlineBox]) -> Vec<BidiLevel> {
    let paragraph = analysis.paragraph_level();
    let char_levels = analysis.bidi_levels();
    let object_levels = analysis.object_levels();
    if char_levels.is_empty() {
        return alloc::vec![paragraph; inline_boxes.len()];
    }
    // The paragraph in logical order: characters, and the boxes before the character at their
    // index. `None`: a box whose level is still to find (an edge), or a removed character.
    enum Slot {
        Char(Option<BidiLevel>),
        Box(usize),
    }
    let removed = |c: char| {
        matches!(c, '\u{202A}'..='\u{202E}' | '\u{200B}'..='\u{200D}' | '\u{2060}'..='\u{2064}' | '\u{FEFF}')
    };
    let mut slots = Vec::with_capacity(char_levels.len() + inline_boxes.len());
    let mut levels: Vec<Option<BidiLevel>> = alloc::vec![None; inline_boxes.len()];
    let mut boxes = inline_boxes.iter().enumerate().peekable();
    let mut objects = object_levels.iter();
    let mut place_box = |slots: &mut Vec<Slot>, levels: &mut Vec<Option<BidiLevel>>, index: usize, kind: InlineBoxKind| {
        if !kind.is_span_edge() {
            levels[index] = Some(objects.next().copied().unwrap_or(paragraph));
        }
        slots.push(Slot::Box(index));
    };
    for ((byte, c), &level) in text.char_indices().zip(char_levels) {
        while let Some((index, b)) = boxes.next_if(|(_, b)| b.inline_box.index <= byte) {
            place_box(&mut slots, &mut levels, index, b.inline_box.kind);
        }
        slots.push(Slot::Char((!removed(c)).then_some(level)));
    }
    for (index, b) in boxes {
        place_box(&mut slots, &mut levels, index, b.inline_box.kind);
    }
    let level_of = |slot: &Slot, levels: &[Option<BidiLevel>]| match *slot {
        Slot::Char(level) => level,
        Slot::Box(index) => levels[index].filter(|_| !inline_boxes[index].inline_box.kind.is_span_edge()),
    };
    // End edges: what precedes them; start edges: what follows them.
    let mut last = None;
    for slot in &slots {
        if let Slot::Box(index) = *slot
            && inline_boxes[index].inline_box.kind == InlineBoxKind::SpanEnd
        {
            levels[index] = last;
        }
        last = level_of(slot, &levels).or(last);
    }
    let mut next = None;
    for slot in slots.iter().rev() {
        if let Slot::Box(index) = *slot
            && inline_boxes[index].inline_box.kind == InlineBoxKind::SpanStart
        {
            levels[index] = next;
        }
        next = level_of(slot, &levels).or(next);
    }
    // An edge with nothing on its side takes the other side's level.
    let (mut before, mut after) = (alloc::vec![None; inline_boxes.len()], None);
    let mut last = None;
    for slot in &slots {
        if let Slot::Box(index) = *slot {
            before[index] = last;
        }
        last = level_of(slot, &levels).or(last);
    }
    for slot in slots.iter().rev() {
        if let Slot::Box(index) = *slot
            && levels[index].is_none()
        {
            levels[index] = after.or(before[index]);
        }
        after = level_of(slot, &levels).or(after);
    }
    levels.into_iter().map(|level| level.unwrap_or(paragraph)).collect()
}

/// The font to use if a font query doesn't return any font candidates at all.
#[derive(Debug)]
enum LastResortFont {
    Unresolved,
    Resolved(FontInstance),
    Unavailable,
}

struct FontSelector<'a, 'b, B: Brush> {
    query: &'b mut Query<'a>,
    /// pyro: per style, the first font [`Self::query`] yields for the style's own
    /// families and attributes whose `unicode-range` includes U+0020 (its CSS first available
    /// font), recorded when the style's fonts are first queried; `None` where its families yield
    /// no such font or the style shaped no text.
    /// The style metrics take their font from here instead of querying again.
    first_fonts: &'b mut Vec<Option<FontInstance>>,
    /// The first available font of the current query, if it comes from the current style's own
    /// families.
    current_first: Option<FontInstance>,
    fonts_id: Option<usize>,
    rcx: &'a ResolveContext,
    styles: &'a [ResolvedStyle<B>],
    style_index: u16,
    attrs: fontique::Attributes,

    /// Whether [`Self::query`] yields any font, cached.
    query_yields_font: Option<bool>,

    /// The font to use if [`Self::query`] doesn't return any font.
    last_resort_font: LastResortFont,

    analysis_data_sources: &'a AnalysisDataSources,
}

impl<'a, 'b, B: Brush> FontSelector<'a, 'b, B> {
    /// Construct a new `FontSelector`.
    ///
    /// If `query` ends up not returning a font for a query, the `last_resort_font` is returned
    /// instead.
    fn new(
        query: &'b mut Query<'a>,
        rcx: &'a ResolveContext,
        styles: &'a [ResolvedStyle<B>],
        analysis_data_sources: &'a AnalysisDataSources,
        first_fonts: &'b mut Vec<Option<FontInstance>>,
    ) -> Self {
        let attrs = fontique::Attributes::default();

        Self {
            query,
            first_fonts,
            current_first: None,
            fonts_id: None,
            rcx,
            styles,
            style_index: 0,
            attrs,
            query_yields_font: None,
            last_resort_font: LastResortFont::Unresolved,

            analysis_data_sources,
        }
    }
}

impl<'a, 'b, B: Brush> parley_engine::FontSelector for FontSelector<'a, 'b, B> {
    fn begin_segment(
        &mut self,
        segment: &parley_engine::itemize::Segment,
        options: &ShapeOptions<'_>,
    ) {
        self.query.set_fallbacks(fontique::FallbackKey::new(
            segment.script,
            options.language.as_ref(),
        ));
        self.query_yields_font = None;
    }

    fn select_font(
        &mut self,
        _segment: &parley_engine::itemize::Segment,
        _options: &ShapeOptions<'_>,
        cluster: &mut CharCluster,
    ) -> Option<FontInstanceRef<'_>> {
        let style_index = cluster.style_index();
        // pyro: the presentation the cluster asks for: a variation selector's (U+FE0F emoji,
        // U+FE0E text), else `font-variant-emoji`'s, else its default (CSS Fonts 4 § 6.13).
        let presentation = emoji_presentation(cluster, self.styles[style_index as usize].font_variant_emoji);
        let is_emoji = cluster.is_emoji() || presentation == Some(true);

        if style_index != self.style_index || is_emoji || self.fonts_id.is_none() {
            self.style_index = style_index;
            let style = &self.styles[style_index as usize];

            let fonts_id = style.font_family.id();
            let fonts = self.rcx.stack(style.font_family).unwrap_or(&[]);
            let fonts = fonts.iter().copied().map(QueryFamily::Id);
            if is_emoji {
                use core::iter::once;
                let emoji_family = QueryFamily::Generic(GenericFamily::Emoji);
                self.query.set_families(fonts.chain(once(emoji_family)));
                self.fonts_id = None;
                self.query_yields_font = None;
            } else if self.fonts_id != Some(fonts_id) {
                self.query.set_families(fonts);
                self.fonts_id = Some(fonts_id);
                self.query_yields_font = None;
            }

            let attrs = fontique::Attributes {
                width: style.font_width,
                weight: style.font_weight,
                style: style.font_style,
            };
            if self.attrs != attrs {
                self.query.set_attributes(attrs);
                self.attrs = attrs;
                self.query_yields_font = None;
            }
        }

        // Fall back to the last-resort font if the query doesn't yield a font (with a charmap) at
        // all. This is checked before the actual query below due to lifetime issues. While this is
        // not *too* expensive, once the new borrow checker (Polonius) is stable, we could remove
        // this check and do the last-resort scan after the failed query:
        // <https://blog.rust-lang.org/2026/08/04/enabling-polonius-alpha-on-nightly/>.
        // pyro: this pass also finds the style's first available font, which the style
        // metrics need: the first font the query yields for the current families and attributes
        // whose `unicode-range` includes U+0020, if it comes from the style's own families (not
        // the script fallbacks or the emoji family, which the query yields after them). Fonts
        // without a charmap count here, as there.
        let families = self
            .rcx
            .stack(self.styles[style_index as usize].font_family)
            .unwrap_or(&[]);
        let query = &mut *self.query;
        let current_first = &mut self.current_first;
        let query_yields_font = *self.query_yields_font.get_or_insert_with(|| {
            let mut yields_font = false;
            let mut seeking_first = true;
            *current_first = None;
            query.matches_with(|font| {
                if seeking_first && !families.contains(&font.family.0) {
                    seeking_first = false;
                } else if seeking_first && font.unicode_range.contains(0x20) {
                    seeking_first = false;
                    *current_first = Some(FontInstance {
                        font: FontData {
                            data: font.blob.clone(),
                            index: font.index,
                        },
                        synthesis: font.synthesis,
                    });
                }
                yields_font |= font.charmap().is_some();
                if yields_font && !seeking_first {
                    fontique::QueryStatus::Stop
                } else {
                    fontique::QueryStatus::Continue
                }
            });
            yields_font
        });
        if let Some(font) = &self.current_first {
            let slot = &mut self.first_fonts[style_index as usize];
            if slot.is_none() {
                *slot = Some(font.clone());
            }
        }
        if !query_yields_font {
            if matches!(self.last_resort_font, LastResortFont::Unresolved) {
                if let Some(font) = any_font(self.query) {
                    self.last_resort_font = LastResortFont::Resolved(FontInstance {
                        font: FontData {
                            data: font.blob,
                            index: font.index,
                        },
                        synthesis: font.synthesis,
                    });
                } else {
                    self.last_resort_font = LastResortFont::Unavailable;
                }

                self.fonts_id = None;
            }

            return if let LastResortFont::Resolved(ref font) = self.last_resort_font {
                Some(font.as_ref())
            } else {
                None
            };
        }

        let mut selected_font = None;
        let mut best_coverage = Coverage::NONE;
        // pyro: the first font covering the cluster whose kind (colour or not) is the presentation's.
        let mut presented_font = None;
        // pyro: a variation sequence in the cluster: the first font that maps it wins (CSS Fonts 4
        // § 5.4 cluster matching), else its base character is matched as before.
        let sequence = {
            let chars = cluster.chars();
            chars.iter().position(|c| parley_engine::is_variation_selector(c.ch)).and_then(|i| {
                (i > 0).then(|| (chars[0].ch, chars[i].ch))
            })
        };

        self.query.matches_with(|font| {
            let Some(charmap) = font.charmap() else {
                return fontique::QueryStatus::Continue;
            };

            // Any non-zero value indicates the existence of a glyph.
            let has = |ch: char| charmap.map(ch).is_some_and(|g| g != 0);
            let coverage = cluster.calculate_coverage(
                |ch| {
                    // pyro: as HarfBuzz shapes them in a font without them (harfrust's normalizer and
                    // default-ignorable handling), so as Blink keeps them in the font: U+2011 drawn
                    // as the font's U+2010, and default-ignorable code points hidden (WPT css-text
                    // `line-break-anywhere-overrides-uax-behavior-011/013/014`).
                    // pyro: and a variation selector, which no nominal cmap maps: the sequence is
                    // looked up in the font's `cmap` format 14 below, else the base character
                    // is drawn alone.
                    has(ch)
                        || (ch == '\u{2011}' && has('\u{2010}'))
                        || hidden_default_ignorable(ch)
                        || parley_engine::is_variation_selector(ch)
                },
                self.analysis_data_sources,
            );
            if let Some((base, selector)) = sequence
                && coverage.is_complete()
                && parley_engine::maps_variation_sequence(font.blob.as_ref(), font.index, base, selector)
            {
                presented_font = Some(font);
                return fontique::QueryStatus::Stop;
            }
            if let Some(want_color) = presentation
                && coverage.is_complete()
            {
                if is_color_font(font) == want_color {
                    presented_font = Some(font);
                    return fontique::QueryStatus::Stop;
                }
                if best_coverage.is_complete() {
                    return fontique::QueryStatus::Continue;
                }
            }
            if coverage > best_coverage {
                selected_font = Some(font);
                best_coverage = coverage;

                if coverage.is_complete() && presentation.is_none() && sequence.is_none() {
                    fontique::QueryStatus::Stop
                } else {
                    fontique::QueryStatus::Continue
                }
            } else {
                if selected_font.is_none() {
                    selected_font = Some(font);
                }
                fontique::QueryStatus::Continue
            }
        });

        presented_font.or(selected_font).map(|font| FontInstanceRef {
            data: &font.blob,
            index: font.index,
            synthesis: &font.synthesis,
        })
    }
}

/// pyro: whether `cluster` asks for emoji presentation (`Some(true)`), text presentation
/// (`Some(false)`) or neither: a variation selector in it decides, else `font-variant-emoji`
/// (`text`, `emoji`, and `unicode`: the default presentation), else none (`normal`: each font's
/// own glyph, as before).
fn emoji_presentation(cluster: &CharCluster, variant: crate::style::FontVariantEmoji) -> Option<bool> {
    use crate::style::FontVariantEmoji;
    if cluster.chars().iter().any(|c| c.ch == '\u{FE0F}') {
        return Some(true);
    }
    if cluster.chars().iter().any(|c| c.ch == '\u{FE0E}') {
        return Some(false);
    }
    // Only for emoji: other text has no presentation to choose.
    let is_emoji_char = cluster.chars().first().is_some_and(|c| parley_engine::is_emoji_char(c.ch));
    match variant {
        FontVariantEmoji::Emoji if is_emoji_char => Some(true),
        FontVariantEmoji::Text if is_emoji_char => Some(false),
        // The Unicode default presentation (`Emoji_Presentation`).
        FontVariantEmoji::Unicode if is_emoji_char => {
            Some(cluster.chars().first().is_some_and(|c| parley_engine::is_emoji_presentation_char(c.ch)))
        }
        _ => None,
    }
}

/// pyro: whether `font` draws colour glyphs (`parley_engine::is_color_font`).
fn is_color_font(font: &QueryFont) -> bool {
    parley_engine::is_color_font(font.blob.as_ref(), font.index)
}

/// Just query for any generic family's font from the font collection.
///
/// This sets generic families on `query`, so callers need to make sure to set families back again.
/// This returns `None` if it doesn't find any fonts, but note this only checks generic families.
///
/// This can be used to have a font at hand for shaping, in case more sophisticated font querying
/// returns no fonts.
fn any_font(query: &mut Query<'_>) -> Option<QueryFont> {
    let mut found = None;

    // Try the system's generic text families.
    //
    // At this point, the font itself doesn't really matter: we mostly just need any font to work
    // with. This has a cost, as it extends our query's candidate font families with all the generic
    // families registered on the system. A better future may be to allow users to register some
    // last resort in `fontique`, or pass it directly to `parley`.
    query.set_families([
        QueryFamily::Generic(GenericFamily::SansSerif),
        QueryFamily::Generic(GenericFamily::Serif),
        QueryFamily::Generic(GenericFamily::Monospace),
    ]);
    query.matches_with(
        #[inline]
        |font: &QueryFont| {
            found = Some(font.clone());
            fontique::QueryStatus::Stop
        },
    );

    found
}

/// pyro: the nearest span around the text of style `index` (itself included) that `vertical-align`
/// shifts, if any: text on either side of the edge of such a span is shaped apart.
/// pyro: the nearest span at or above `index` that is a bidi isolate, if any.
fn isolating_span<B: Brush>(styles: &[ResolvedStyle<B>], mut index: u16) -> Option<u16> {
    loop {
        let style = &styles[usize::from(index)];
        if style.isolates {
            return Some(index);
        }
        if style.parent == index {
            return None;
        }
        index = style.parent;
    }
}

fn shifted_span<B: Brush>(styles: &[ResolvedStyle<B>], mut index: u16) -> Option<u16> {
    loop {
        let style = &styles[usize::from(index)];
        if style.vertical_align != crate::style::VerticalAlign::BASELINE {
            return Some(index);
        }
        if style.parent == index {
            return None;
        }
        index = style.parent;
    }
}

/// pyro: a default-ignorable code point (Unicode `Default_Ignorable_Code_Point`) that HarfBuzz
/// hides in a font without it, so that it does not choose the font. Variation selectors, the zero
/// width joiner and Mongolian variation selectors choose glyphs and fonts (emoji presentation,
/// sequences) and are left out, as is U+00AD (a soft hyphen draws a hyphen where a line breaks).
fn hidden_default_ignorable(ch: char) -> bool {
    matches!(
        ch as u32,
        0x034F
            | 0x061C
            | 0x115F..=0x1160
            | 0x17B4..=0x17B5
            | 0x200B..=0x200C
            | 0x200E..=0x200F
            | 0x202A..=0x202E
            | 0x2060..=0x206F
            | 0x3164
            | 0xFEFF
            | 0xFFA0
            | 0xFFF0..=0xFFF8
            | 0x1BCA0..=0x1BCA3
            | 0x1D173..=0x1D17A
            | 0xE0000..=0xE00FF
            | 0xE01F0..=0xE0FFF
    )
}
