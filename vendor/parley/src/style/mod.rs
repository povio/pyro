// Copyright 2021 the Parley Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Rich styling support.

mod brush;
mod font;
mod styleset;

use alloc::borrow::Cow;

#[cfg(feature = "libm")]
#[allow(unused_imports)]
use core_maths::CoreFloat;
use parley_engine::FontMetrics;

pub use brush::*;
pub use font::{
    FontFamily, FontFamilyName, FontFeature, FontFeatures, FontStyle, FontVariation,
    FontVariations, FontWeight, FontWidth, GenericFamily,
};
pub use fontique::Language;
pub use parlance::{LineBreak, OverflowWrap, TextWrapMode, WordBreak};
pub use styleset::StyleSet;

use crate::util::nearly_eq;

/// Whitespace collapsing and hanging behavior.
///
/// Note: Currently only the [`TreeBuilder`](crate::TreeBuilder) collapses white space. The
/// [`RangedBuilder`](crate::RangedBuilder) and [`StyleRunBuilder`](crate::StyleRunBuilder)
/// builders expect already-transformed text and use this property only to influence whitespace hanging behaviour.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum WhiteSpaceCollapse {
    /// Collapse whitespace, including ASCII segment breaks, to a space.
    Collapse,
    /// Preserve spaces, tabs, and segment breaks.
    #[default]
    Preserve,
    /// Collapse spaces and tabs while preserving segment breaks.
    PreserveBreaks,
    /// Preserve whitespace without hanging, allowing wrapping after each space or tab.
    BreakSpaces,
}

/// pyro: the distance between tab stops (CSS `tab-size`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TabSize {
    /// A number of spaces: the advance of a space in the layout's root style, with its letter
    /// and word spacing (see [`Layout::set_tab_stops`](crate::Layout::set_tab_stops)).
    Spaces(f32),
    /// A length.
    Length(f32),
}

impl Default for TabSize {
    fn default() -> Self {
        Self::Spaces(8.0)
    }
}

/// pyro: what a soft hyphen (U+00AD) does where a line could end at it (CSS `hyphens` and
/// `hyphenate-character`).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum SoftHyphen {
    /// The line may end there and then shows the hyphen of the font (U+2010, else U+002D).
    #[default]
    Auto,
    /// The line may end there and then shows this character, if the font has it.
    Char(char),
    /// The line may end there and then shows the first `.1` of these characters, if the font has
    /// them all (a `hyphenate-character` string of two to four characters).
    Chars([char; 4], u8),
    /// The line may end there and shows nothing.
    Hidden,
    /// No line ends there.
    NoBreak,
}

/// pyro: which fullwidth punctuation is set half-width, flush (CSS Text 4 `text-spacing-trim`).
/// The font's `halt` feature sets a character half-width; fonts without it are not trimmed.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum TextSpacingTrim {
    /// None (`space-all`, or font features that set those widths themselves: `halt`, `palt`, ...).
    SpaceAll,
    /// Punctuation next to punctuation collapses; opening punctuation at the start of a line keeps
    /// its width; closing punctuation at the end of a line that does not fit is trimmed (`normal`,
    /// `auto`).
    #[default]
    Normal,
    /// As [`Self::TrimStart`], except on a block's first line and after a forced break.
    SpaceFirst,
    /// As [`Self::Normal`], and opening punctuation at the start of every line is trimmed.
    TrimStart,
    /// As [`Self::TrimStart`], and closing punctuation at the end of every line is trimmed.
    TrimBoth,
    /// All fullwidth punctuation is trimmed.
    TrimAll,
}

/// pyro: how a layout's text is scaled to fill its lines (CSS Text 5 `text-fit`), as Blink does
/// it by default: after the lines are broken, a line's text is drawn scaled (glyphs, advances,
/// and the span boxes whose line height follows the font size); inline boxes and span edges keep
/// their size.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct TextFit {
    /// Whether lines grow, shrink or neither.
    pub kind: TextFitKind,
    /// Which lines are scaled, and by what.
    pub target: TextFitTarget,
    /// The limit on the scale factor (`1.0` for `100%`): the most a line grows by, or the least
    /// it shrinks to.
    pub limit: Option<f32>,
}

/// pyro: whether `text-fit` grows lines, shrinks them, or neither.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum TextFitKind {
    /// Lines keep their size.
    #[default]
    None,
    /// Lines narrower than their box grow to fill it.
    Grow,
    /// Lines wider than their box shrink to fit it.
    Shrink,
}

/// pyro: which lines `text-fit` scales.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum TextFitTarget {
    /// All lines by one factor, the one that makes the widest (or narrowest) fit
    /// ([`crate::Layout::text_fit_block_scale`]).
    #[default]
    Consistent,
    /// Each line by its own factor, except a block's last line and lines before a forced break.
    PerLine,
    /// Each line by its own factor.
    PerLineAll,
}

impl TextFit {
    /// `scale` within the limit (Blink's `RestrictScale`).
    pub(crate) fn restrict(&self, scale: f32) -> f32 {
        match self.limit {
            None => scale,
            Some(limit) if self.kind == TextFitKind::Grow => scale.min(limit.max(1.0)),
            Some(limit) => scale.max(limit.min(1.0)),
        }
    }
}

/// pyro: which of a style's letter and word spacing are fractions of its font size (CSS
/// percentages): `text-fit` scales those with the text and leaves lengths as they are.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SpacingScales {
    /// `letter-spacing` is a percentage.
    pub letter: bool,
    /// `word-spacing` is a percentage.
    pub word: bool,
}

/// pyro: where justification adds space (CSS `text-justify`).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum TextJustify {
    /// Between words, and between the characters of CJK text (as Blink).
    #[default]
    Auto,
    /// Nowhere.
    None,
    /// Between words.
    InterWord,
    /// Between all typographic character units (`distribute`).
    InterCharacter,
}

/// pyro: where `text-autospace` adds 1/8 ic between ideographs and other characters
/// (CSS Text 4 `text-autospace`). The default adds none.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TextAutospace {
    /// Between ideographs and non-ideographic letters.
    pub ideograph_alpha: bool,
    /// Between ideographs and non-ideographic decimal numerals.
    pub ideograph_numeric: bool,
}

impl TextAutospace {
    /// No space anywhere.
    pub const NONE: Self = Self { ideograph_alpha: false, ideograph_numeric: false };
    /// CSS `normal`: ideographs are spaced from letters and from numerals.
    pub const NORMAL: Self = Self { ideograph_alpha: true, ideograph_numeric: true };

    /// Whether no space is added.
    pub const fn is_none(self) -> bool {
        !self.ideograph_alpha && !self.ideograph_numeric
    }
}

/// The height that this text takes up. The default is `MetricsRelative(1.0)`, which is the given
/// font's preferred line height.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LineHeight {
    /// The line's height is a multiple of the "line height" defined by the font's metrics--the sum
    /// of the ascender height, descender height, and line gap / leading.
    MetricsRelative(f32),
    /// Line height specified as a multiple of the font size. This is how the CSS `line-height`
    /// property behaves if given a unitless number. Useful if you're using system-defined generic
    /// font families and want the line heights to be consistent across platforms.
    FontSizeRelative(f32),
    /// Line height specified in absolute units. This can be useful for ensuring all lines are
    /// spaced a whole number of pixels apart, or fitting lines into a given layout container
    /// height.
    Absolute(f32),
}

impl Default for LineHeight {
    fn default() -> Self {
        Self::NORMAL
    }
}

impl LineHeight {
    /// The recommended `LineHeight` for CSS's `line-height: normal`.
    pub const NORMAL: Self = Self::MetricsRelative(1.0);

    pub(crate) fn nearly_eq(self, other: Self) -> bool {
        match (self, other) {
            (Self::MetricsRelative(a), Self::MetricsRelative(b))
            | (Self::FontSizeRelative(a), Self::FontSizeRelative(b))
            | (Self::Absolute(a), Self::Absolute(b)) => nearly_eq(a, b),
            _ => false,
        }
    }

    pub(crate) fn scale(self, scale: f32) -> Self {
        match self {
            Self::Absolute(value) => Self::Absolute(value * scale),
            // The other variants are relative to the font size, so scaling here needn't do anything
            value => value,
        }
    }

    /// The line height in layout units for text set in a font with `metrics` at `font_size`.
    ///
    /// With a `quantum` (pyro: the layout units of a CSS pixel, the layout's scale; 0 for
    /// none), the font's ascent, descent and leading are each rounded to whole CSS pixels before
    /// being summed for [`MetricsRelative`](Self::MetricsRelative), as browsers do for
    /// `line-height: normal`, so that such lines are a whole number of CSS pixels tall at any
    /// device scale (Blink's `FontMetrics`).
    ///
    /// pyro: with a `quantum` the other two are in Blink's layout units (1/64 of a CSS pixel), as
    /// `ComputedStyle::ComputedLineHeightAsFixed` has them: a length rounded, a number times the
    /// font size (rounded to units) cut to units, in Blink's single-precision arithmetic. Lines of
    /// `19px/1.43` text are 27.15625px apart there, not 27.17px, which over three lines is another
    /// whole pixel for the text to be painted at.
    pub(crate) fn resolve(self, font_size: f32, metrics: &FontMetrics, quantum: f32) -> f32 {
        match self {
            Self::Absolute(value) if quantum > 0. => (value / quantum * 64.).round() / 64. * quantum,
            Self::FontSizeRelative(value) if quantum > 0. => {
                let font_size = (font_size / quantum * 64.).round() / 64.;
                // The number is a percentage in Blink.
                (font_size * (value * 100.) / 100. * 64.).trunc() / 64. * quantum
            }
            Self::Absolute(value) => value,
            Self::FontSizeRelative(value) => value * font_size,
            Self::MetricsRelative(value) if quantum > 0. => {
                (round_to(metrics.ascent, quantum)
                    + round_to(metrics.descent, quantum)
                    + round_to(metrics.leading, quantum))
                    * value
            }
            Self::MetricsRelative(value) => {
                (metrics.ascent + metrics.descent + metrics.leading) * value
            }
        }
    }
}

/// pyro: `value` rounded to a whole number of `quantum`s (a CSS pixel in layout units).
pub(crate) fn round_to(value: f32, quantum: f32) -> f32 {
    (value / quantum).round() * quantum
}

/// Which reference of a span (or [`InlineBox`]) is aligned with the same reference of its
/// *parent* span (the enclosing style span, or the root style for top-level content). Mirrors the
/// CSS `alignment-baseline` property.
///
/// [`InlineBox`]: crate::InlineBox
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AlignmentBaseline {
    /// Align the baseline of the box with the baseline of the parent span.
    #[default]
    Baseline,
    /// Align the top of the box with the top of the parent's content area (font ascent).
    TextTop,
    /// Align the bottom of the box with the bottom of the parent's content area (font descent).
    TextBottom,
    /// Align the vertical midpoint of the box with the parent span's baseline plus half the
    /// x-height of the parent.
    Middle,
    /// pyro: css-inline-3 `alignment-baseline: text-top`: align the span's text-over baseline
    /// (its font's ascent) with the parent's. A box aligns as with [`Self::TextTop`].
    TextOver,
    /// pyro: css-inline-3 `alignment-baseline: text-bottom`: align the text-under baselines
    /// (descents). A box aligns as with [`Self::TextBottom`].
    TextUnder,
    /// pyro: css-inline-3 `alignment-baseline: middle`: align the span's x-middle baseline (half
    /// its x-height above its baseline) with the parent's. A box stays on the baseline.
    XMiddle,
    /// pyro: css-inline-3 `alignment-baseline: alphabetic`.
    Alphabetic,
    /// pyro: css-inline-3 `alignment-baseline: ideographic` (the ideographic under baseline).
    Ideographic,
    /// pyro: css-inline-3 `alignment-baseline: central`.
    Central,
    /// pyro: css-inline-3 `alignment-baseline: mathematical`.
    Mathematical,
    /// pyro: css-inline-3 `alignment-baseline: hanging`.
    Hanging,
}

/// pyro: CSS `font-variant-emoji`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum FontVariantEmoji {
    /// Each emoji's default presentation (`Emoji_Presentation`).
    #[default]
    Normal,
    /// Text presentation.
    Text,
    /// Emoji presentation.
    Emoji,
    /// As `normal` (the Unicode default presentation).
    Unicode,
}

/// A shift applied to a span (or [`InlineBox`]) after [`AlignmentBaseline`] alignment. Mirrors
/// the CSS `baseline-shift` property.
///
/// [`Top`](Self::Top) and [`Bottom`](Self::Bottom) are *line-relative*: the box, together with
/// the descendants that are aligned relative to it, is placed against the line box instead of
/// its parent span, and the [`AlignmentBaseline`] is ignored. All other values are relative to
/// the parent span.
///
/// [`InlineBox`]: crate::InlineBox
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BaselineShift {
    /// Raise the box by the given distance (in layout units). Negative values lower it.
    Length(f32),
    /// pyro: like [`Length`](Self::Length), resolved from a percentage of the box's own line
    /// height (`vertical-align: <percentage>`): a line that CSS `text-fit` scales scales it with
    /// that line height.
    LineHeight(f32),
    /// Lower the box to the proper position for subscripts of the parent span.
    Sub,
    /// Raise the box to the proper position for superscripts of the parent span.
    Super,
    /// Align the top of the box (and its descendants) with the top of the line box.
    Top,
    /// Align the bottom of the box (and its descendants) with the bottom of the line box.
    Bottom,
}

impl Default for BaselineShift {
    fn default() -> Self {
        Self::Length(0.)
    }
}

/// Vertical alignment of a span (a styled range of text) or an [`InlineBox`] within its line.
/// This mirrors the CSS `vertical-align` shorthand, which sets [`AlignmentBaseline`] and
/// [`BaselineShift`]; the associated constants cover the CSS 2 keywords.
///
/// [`InlineBox`]: crate::InlineBox
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct VerticalAlign {
    /// The `alignment-baseline` component.
    pub alignment: AlignmentBaseline,
    /// The `baseline-shift` component.
    pub shift: BaselineShift,
}

impl VerticalAlign {
    /// `vertical-align: baseline`.
    pub const BASELINE: Self = Self::new(AlignmentBaseline::Baseline, BaselineShift::Length(0.));
    /// `vertical-align: sub`.
    pub const SUB: Self = Self::new(AlignmentBaseline::Baseline, BaselineShift::Sub);
    /// `vertical-align: super`.
    pub const SUPER: Self = Self::new(AlignmentBaseline::Baseline, BaselineShift::Super);
    /// `vertical-align: text-top`.
    pub const TEXT_TOP: Self = Self::new(AlignmentBaseline::TextTop, BaselineShift::Length(0.));
    /// `vertical-align: text-bottom`.
    pub const TEXT_BOTTOM: Self =
        Self::new(AlignmentBaseline::TextBottom, BaselineShift::Length(0.));
    /// `vertical-align: middle`.
    pub const MIDDLE: Self = Self::new(AlignmentBaseline::Middle, BaselineShift::Length(0.));
    /// `vertical-align: top`.
    pub const TOP: Self = Self::new(AlignmentBaseline::Baseline, BaselineShift::Top);
    /// `vertical-align: bottom`.
    pub const BOTTOM: Self = Self::new(AlignmentBaseline::Baseline, BaselineShift::Bottom);

    /// Combine an `alignment-baseline` with a `baseline-shift`.
    pub const fn new(alignment: AlignmentBaseline, shift: BaselineShift) -> Self {
        Self { alignment, shift }
    }

    /// `vertical-align: <length>`: raise the baseline of the box by `length` (in layout units)
    /// above the baseline of the parent span. Negative values lower the baseline.
    pub const fn length(length: f32) -> Self {
        Self::new(AlignmentBaseline::Baseline, BaselineShift::Length(length))
    }

    /// Whether this value is line-relative (`top` or `bottom`), i.e. aligns the box to the line
    /// box rather than to its parent span.
    pub fn is_line_relative(self) -> bool {
        matches!(self.shift, BaselineShift::Top | BaselineShift::Bottom)
    }

    pub(crate) fn nearly_eq(self, other: Self) -> bool {
        self.alignment == other.alignment
            && match (self.shift, other.shift) {
                (BaselineShift::Length(a), BaselineShift::Length(b))
                | (BaselineShift::LineHeight(a), BaselineShift::LineHeight(b)) => nearly_eq(a, b),
                (a, b) => a == b,
            }
    }

    pub(crate) fn scale(self, scale: f32) -> Self {
        match self.shift {
            BaselineShift::Length(value) => Self {
                shift: BaselineShift::Length(value * scale),
                ..self
            },
            BaselineShift::LineHeight(value) => Self {
                shift: BaselineShift::LineHeight(value * scale),
                ..self
            },
            _ => self,
        }
    }
}

/// Properties that define a style.
#[derive(Clone, PartialEq, Debug)]
pub enum StyleProperty<'a, B: Brush> {
    /// CSS `font-family` property value.
    FontFamily(FontFamily<'a>),
    /// Font size.
    FontSize(f32),
    /// Font width.
    FontWidth(FontWidth),
    /// Font style.
    FontStyle(FontStyle),
    /// Font weight.
    FontWeight(FontWeight),
    /// Font variation settings.
    FontVariations(FontVariations<'a>),
    /// Font feature settings.
    FontFeatures(FontFeatures<'a>),
    /// Locale.
    Locale(Option<Language>),
    /// Brush for rendering text.
    Brush(B),
    /// Underline decoration.
    Underline(bool),
    /// Offset of the underline decoration.
    UnderlineOffset(Option<f32>),
    /// Size of the underline decoration.
    UnderlineSize(Option<f32>),
    /// Brush for rendering the underline decoration.
    UnderlineBrush(Option<B>),
    /// Strikethrough decoration.
    Strikethrough(bool),
    /// Offset of the strikethrough decoration.
    StrikethroughOffset(Option<f32>),
    /// Size of the strikethrough decoration.
    StrikethroughSize(Option<f32>),
    /// Brush for rendering the strikethrough decoration.
    StrikethroughBrush(Option<B>),
    /// Line height.
    LineHeight(LineHeight),
    /// Vertical alignment within the line.
    VerticalAlign(VerticalAlign),
    /// Extra spacing between words.
    WordSpacing(f32),
    /// Extra spacing between letters.
    LetterSpacing(f32),
    /// Control over where words can wrap.
    WordBreak(WordBreak),
    /// Strictness of line-breaking rules.
    LineBreak(LineBreak),
    /// Control over "emergency" line-breaking.
    OverflowWrap(OverflowWrap),
    /// Control over non-"emergency" line-breaking.
    TextWrapMode(TextWrapMode),
    /// Whitespace collapsing and hanging behavior.
    WhiteSpaceCollapse(WhiteSpaceCollapse),
    /// pyro: the distance between tab stops.
    TabSize(TabSize),
    /// pyro: what a soft hyphen does at the end of a line.
    SoftHyphen(SoftHyphen),
    /// pyro: whether lines may break inside words where dictionaries find boundaries.
    DictionaryBreaks(bool),
    /// pyro: how many `wrap-inside: avoid` boxes hold the span ([`TextStyle::wrap_avoid_depth`]).
    WrapAvoidDepth(u8),
    /// pyro: CSS `hanging-punctuation` ([`TextStyle::hanging_punctuation`]).
    HangingPunctuation(u8),
    /// pyro: [`TextStyle::full_width_space`].
    FullWidthSpace(bool),
    /// pyro: whether the span is a bidi isolate ([`TextStyle::isolates`]).
    Isolates(bool),
    /// pyro: whether lines break between phrases (CSS `word-break: auto-phrase`).
    PhraseBreaks(bool),
    /// pyro: whether the font size is set by `font-size-adjust` (a face's `size-adjust` is then
    /// not applied).
    FontSizeAdjusted(bool),
    /// pyro: whether the first font variation is the automatic `opsz` (`font-optical-sizing`).
    AutoOpticalSize(bool),
    /// pyro: the OpenType language system to shape with (`font-language-override`).
    LanguageOverride(Option<[u8; 4]>),
    /// pyro: which fullwidth punctuation is set half-width.
    TextSpacingTrim(TextSpacingTrim),
    /// pyro: where space is added between ideographs and other characters.
    TextAutospace(TextAutospace),
    /// pyro: upright glyphs in a vertical line (`text-orientation: upright`).
    Upright(bool),
    /// pyro: text in a vertical line (`TextStyle::vertical`).
    Vertical(bool),
    /// pyro: emoji or text presentation (`TextStyle::font_variant_emoji`).
    FontVariantEmoji(FontVariantEmoji),
    /// pyro: where justification adds space.
    TextJustify(TextJustify),
    /// pyro: which of the letter and word spacing scale with the font size.
    SpacingScales(SpacingScales),
}

/// Unresolved styles.
#[derive(Clone, PartialEq, Debug)]
pub struct TextStyle<'family, 'settings, B: Brush> {
    /// CSS `font-family` property value.
    pub font_family: FontFamily<'family>,
    /// Font size.
    pub font_size: f32,
    /// Font width.
    pub font_width: FontWidth,
    /// Font style.
    pub font_style: FontStyle,
    /// Font weight.
    pub font_weight: FontWeight,
    /// Font variation settings.
    pub font_variations: FontVariations<'settings>,
    /// Font feature settings.
    pub font_features: FontFeatures<'settings>,
    /// Locale.
    pub locale: Option<Language>,
    /// Brush for rendering text.
    pub brush: B,
    /// Underline decoration.
    pub has_underline: bool,
    /// Offset of the underline decoration.
    pub underline_offset: Option<f32>,
    /// Size of the underline decoration.
    pub underline_size: Option<f32>,
    /// Brush for rendering the underline decoration.
    pub underline_brush: Option<B>,
    /// Strikethrough decoration.
    pub has_strikethrough: bool,
    /// Offset of the strikethrough decoration.
    pub strikethrough_offset: Option<f32>,
    /// Size of the strikethrough decoration.
    pub strikethrough_size: Option<f32>,
    /// Brush for rendering the strikethrough decoration.
    pub strikethrough_brush: Option<B>,
    /// Line height.
    pub line_height: LineHeight,
    /// Vertical alignment within the line.
    pub vertical_align: VerticalAlign,
    /// Extra spacing between words.
    pub word_spacing: f32,
    /// Extra spacing between letters.
    pub letter_spacing: f32,
    /// Control over where words can wrap.
    pub word_break: WordBreak,
    /// Strictness of line-breaking rules.
    pub line_break: LineBreak,
    /// Control over "emergency" line-breaking.
    pub overflow_wrap: OverflowWrap,
    /// Control over non-"emergency" line-breaking.
    pub text_wrap_mode: TextWrapMode,
    /// Whitespace collapsing and hanging behavior.
    pub white_space_collapse: WhiteSpaceCollapse,
    /// pyro: the distance between tab stops, for the preserved tabs of this style.
    pub tab_size: TabSize,
    /// pyro: what the soft hyphens of this style do at the end of a line.
    pub soft_hyphen: SoftHyphen,
    /// pyro: whether lines may break inside words where dictionaries find boundaries (Thai, Lao,
    /// Khmer, Burmese). `false` for CSS `word-break: manual`.
    pub dictionary_breaks: bool,
    /// pyro: how many inline boxes with `wrap-inside: avoid` hold the span (CSS Text 4): a line
    /// breaks inside fewer of them first.
    pub wrap_avoid_depth: u8,
    /// pyro: CSS `hanging-punctuation`: `FORCE_END` (2), `ALLOW_END` (4) and `LAST` (8) bits (the
    /// line breaker's; `first` is the caller's, a negative indent).
    pub hanging_punctuation: u8,
    /// pyro: a collapsed space of the span is U+3000 (CSS `text-transform: full-width`, which
    /// applies after white space collapsing).
    pub full_width_space: bool,
    /// pyro: whether the span is a bidi isolate (CSS `unicode-bidi: isolate`, `isolate-override`
    /// or `plaintext`): shaping breaks at its edges (CSS Text 3 § 7.3). Not inherited: set on the
    /// span's own style.
    pub isolates: bool,
    /// pyro: CSS `word-break: auto-phrase`: with `word_break` [`WordBreak::KeepAll`], lines also
    /// break between Japanese phrases, and a line whose phrase does not fit breaks as under
    /// [`WordBreak::Normal`].
    pub phrase_breaks: bool,
    /// pyro: whether the font size is set by CSS `font-size-adjust`: it fixes the size of the
    /// metric it names for every font, so a face's `size-adjust` (`@font-face`) is not applied.
    pub font_size_adjusted: bool,
    /// pyro: whether the first of `font_variations` is the `opsz` value CSS
    /// `font-optical-sizing: auto` gives (the font size): a face's `font-variation-settings`
    /// descriptor overrides it, and it follows the face's `size-adjust`.
    pub auto_optical_size: bool,
    /// pyro: the OpenType language system tag to shape with instead of the locale's (CSS
    /// `font-language-override`, padded with spaces).
    pub language_override: Option<[u8; 4]>,
    /// pyro: which fullwidth punctuation of this style is set half-width (CSS `text-spacing-trim`).
    pub text_spacing_trim: TextSpacingTrim,
    /// pyro: where this style's text spaces ideographs from letters and numerals (CSS
    /// `text-autospace`).
    pub text_autospace: TextAutospace,
    /// pyro: this style's glyphs stand upright in vertical lines (CSS `text-orientation:
    /// upright`): a glyph without a vertical advance (`vmtx`) advances by the font's height.
    pub upright: bool,
    /// pyro: this style's text is in a vertical line, not sideways (CSS `writing-mode: vertical-*`
    /// with `text-orientation: mixed` or `upright`): shaping runs split where a character's
    /// vertical orientation (UTR #50, [`crate::upright_in_vertical`]) changes.
    pub vertical: bool,
    /// pyro: CSS `font-variant-emoji`: the presentation emoji take where no variation selector
    /// chooses one (a colour font for emoji presentation, a text font for text presentation).
    pub font_variant_emoji: FontVariantEmoji,
    /// pyro: where justification adds space in this style's text (CSS `text-justify`).
    pub text_justify: TextJustify,
    /// pyro: which of this style's letter and word spacing scale with its font size (CSS
    /// percentages), which `text-fit` scales.
    pub spacing_scales: SpacingScales,
    /// pyro: room the span's text needs beyond its content area, over and under it, for what is
    /// drawn there (ruby annotations, emphasis marks). The line box holds the span's content area
    /// grown by it, where its line height leaves too little (CSS Ruby 1, 4.4).
    pub annotation_space: AnnotationSpace,
    /// pyro: the edges the span box's content area is trimmed to (CSS Inline 3 `text-box-trim` on
    /// an inline box): its backgrounds, borders and padding start there.
    pub text_box_trim: TextBoxTrim,
}

/// pyro: a font metric a span's content area can end at (CSS Inline 3 `<text-edge>`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TextEdge {
    /// The ascent or descent of the first available font.
    Text,
    /// The cap height (an over edge).
    Cap,
    /// The x-height (an over edge).
    Ex,
    /// The alphabetic baseline (an under edge).
    Alphabetic,
    /// The ideographic em box.
    Ideographic,
    /// The ideographic character face (ink) box.
    IdeographicInk,
}

/// pyro: the edges a span's content area is trimmed to ([`TextStyle::text_box_trim`]); `None`
/// leaves a side at the font's ascent or descent.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct TextBoxTrim {
    /// Over (block-start) edge.
    pub over: Option<TextEdge>,
    /// Under (block-end) edge.
    pub under: Option<TextEdge>,
}

/// pyro: room beyond a span's content area, over and under it ([`TextStyle::annotation_space`]).
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct AnnotationSpace {
    /// Over the content area.
    pub over: f32,
    /// Under the content area.
    pub under: f32,
}

impl AnnotationSpace {
    /// The room scaled by `scale`.
    pub(crate) fn scale(self, scale: f32) -> Self {
        Self { over: self.over * scale, under: self.under * scale }
    }
}

impl<B: Brush> Default for TextStyle<'static, 'static, B> {
    fn default() -> Self {
        TextStyle {
            font_family: FontFamily::Source(Cow::Borrowed("sans-serif")),
            font_size: 16.0,
            font_width: FontWidth::default(),
            font_style: FontStyle::default(),
            font_weight: FontWeight::default(),
            font_variations: FontVariations::empty(),
            font_features: FontFeatures::empty(),
            locale: None,
            brush: B::default(),
            has_underline: false,
            underline_offset: None,
            underline_size: None,
            underline_brush: None,
            has_strikethrough: false,
            strikethrough_offset: None,
            strikethrough_size: None,
            strikethrough_brush: None,
            line_height: LineHeight::default(),
            vertical_align: VerticalAlign::default(),
            word_spacing: 0.0,
            letter_spacing: 0.0,
            word_break: WordBreak::default(),
            line_break: LineBreak::default(),
            overflow_wrap: OverflowWrap::default(),
            text_wrap_mode: TextWrapMode::default(),
            white_space_collapse: WhiteSpaceCollapse::default(),
            tab_size: TabSize::default(),
            soft_hyphen: SoftHyphen::default(),
            dictionary_breaks: true,
            wrap_avoid_depth: 0,
            hanging_punctuation: 0,
            full_width_space: false,
            isolates: false,
            phrase_breaks: false,
            font_size_adjusted: false,
            auto_optical_size: false,
            language_override: None,
            text_spacing_trim: TextSpacingTrim::default(),
            text_autospace: TextAutospace::default(),
            upright: false,
            vertical: false,
            font_variant_emoji: FontVariantEmoji::Normal,
            text_justify: TextJustify::default(),
            spacing_scales: SpacingScales::default(),
            annotation_space: AnnotationSpace::default(),
            text_box_trim: TextBoxTrim::default(),
        }
    }
}

impl<'a, B: Brush> From<FontFamily<'a>> for StyleProperty<'a, B> {
    fn from(value: FontFamily<'a>) -> Self {
        StyleProperty::FontFamily(value)
    }
}

impl<'a, B: Brush> From<&'a [FontFamilyName<'a>]> for StyleProperty<'a, B> {
    fn from(value: &'a [FontFamilyName<'a>]) -> Self {
        StyleProperty::FontFamily(value.into())
    }
}

impl<'a, B: Brush> From<FontFamilyName<'a>> for StyleProperty<'a, B> {
    fn from(value: FontFamilyName<'a>) -> Self {
        StyleProperty::FontFamily(value.into())
    }
}

impl<'a, B: Brush> From<FontVariations<'a>> for StyleProperty<'a, B> {
    fn from(value: FontVariations<'a>) -> Self {
        StyleProperty::FontVariations(value)
    }
}

impl<'a, B: Brush> From<FontFeatures<'a>> for StyleProperty<'a, B> {
    fn from(value: FontFeatures<'a>) -> Self {
        StyleProperty::FontFeatures(value)
    }
}

impl<B: Brush> From<GenericFamily> for StyleProperty<'_, B> {
    fn from(f: GenericFamily) -> Self {
        StyleProperty::FontFamily(f.into())
    }
}

impl<B: Brush> From<LineHeight> for StyleProperty<'_, B> {
    fn from(value: LineHeight) -> Self {
        StyleProperty::LineHeight(value)
    }
}

impl<B: Brush> From<VerticalAlign> for StyleProperty<'_, B> {
    fn from(value: VerticalAlign) -> Self {
        StyleProperty::VerticalAlign(value)
    }
}

impl<B: Brush> From<FontWidth> for StyleProperty<'_, B> {
    fn from(value: FontWidth) -> Self {
        StyleProperty::FontWidth(value)
    }
}

impl<B: Brush> From<FontStyle> for StyleProperty<'_, B> {
    fn from(value: FontStyle) -> Self {
        StyleProperty::FontStyle(value)
    }
}

impl<B: Brush> From<FontWeight> for StyleProperty<'_, B> {
    fn from(value: FontWeight) -> Self {
        StyleProperty::FontWeight(value)
    }
}

impl<B: Brush> From<WordBreak> for StyleProperty<'_, B> {
    fn from(value: WordBreak) -> Self {
        StyleProperty::WordBreak(value)
    }
}

impl<B: Brush> From<LineBreak> for StyleProperty<'_, B> {
    fn from(value: LineBreak) -> Self {
        StyleProperty::LineBreak(value)
    }
}

impl<B: Brush> From<OverflowWrap> for StyleProperty<'_, B> {
    fn from(value: OverflowWrap) -> Self {
        StyleProperty::OverflowWrap(value)
    }
}

impl<B: Brush> From<TextWrapMode> for StyleProperty<'_, B> {
    fn from(value: TextWrapMode) -> Self {
        StyleProperty::TextWrapMode(value)
    }
}
