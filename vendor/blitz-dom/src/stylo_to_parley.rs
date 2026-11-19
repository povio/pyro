//! Conversion functions from Stylo types to Parley types
use blitz_traits::node_id::NodeId;
use std::borrow::Cow;

use style::values::computed::Length;

use crate::node::TextBrush;

// Module of type aliases so we can refer to stylo types with nicer names
pub(crate) mod stylo {
    pub(crate) use style::computed_values::direction::T as Direction;
    pub(crate) use style::computed_values::font_variant_caps::T as FontVariantCaps;
    pub(crate) use style::computed_values::font_variant_position::T as FontVariantPosition;
    pub(crate) use style::computed_values::text_wrap_mode::T as TextWrapMode;
    pub(crate) use style::computed_values::unicode_bidi::T as UnicodeBidi;
    pub(crate) use style::computed_values::white_space_collapse::T as WhiteSpaceCollapse;
    pub(crate) use style::properties::ComputedValues;
    pub(crate) use style::properties::style_structs::Font;
    pub(crate) use style::values::computed::AlignmentBaseline;
    pub(crate) use style::values::computed::BaselineShift;
    pub(crate) use style::values::computed::LineBreak;
    pub(crate) use style::values::computed::OverflowWrap;
    pub(crate) use style::values::computed::WordBreak;
    pub(crate) use style::values::computed::font::FontFamilyList;
    pub(crate) use style::values::computed::font::FontFeatureSettings;
    pub(crate) use style::values::computed::font::FontOpticalSizing;
    pub(crate) use style::values::computed::font::FontStyle;
    pub(crate) use style::values::computed::font::FontVariantEastAsian;
    pub(crate) use style::values::computed::font::FontVariantLigatures;
    pub(crate) use style::values::computed::font::FontVariantNumeric;
    pub(crate) use style::values::computed::font::FontWeight;
    pub(crate) use style::values::computed::font::FontWidth;
    pub(crate) use style::values::computed::font::GenericFontFamily;
    pub(crate) use style::values::computed::font::LineHeight;
    pub(crate) use style::values::computed::font::SingleFontFamily;
    pub(crate) use style::values::generics::box_::BaselineShiftKeyword;
    pub(crate) use style::values::specified::TextAlignKeyword;
    pub(crate) use style::values::specified::TextAlignLast;
}

pub(crate) mod parley {
    pub(crate) use parley::BaseDirection;
    pub(crate) use parley::FontFeature;
    pub(crate) use parley::FontVariation;
    pub(crate) use parley::fontique::QueryFamily;
    pub(crate) use parley::layout::Alignment;
    pub(crate) use parley::setting::*;
    pub(crate) use parley::style::*;
}

pub(crate) fn generic_font_family(input: stylo::GenericFontFamily) -> parley::GenericFamily {
    match input {
        stylo::GenericFontFamily::None => parley::GenericFamily::SansSerif,
        stylo::GenericFontFamily::Serif => parley::GenericFamily::Serif,
        stylo::GenericFontFamily::SansSerif => parley::GenericFamily::SansSerif,
        stylo::GenericFontFamily::Monospace => parley::GenericFamily::Monospace,
        stylo::GenericFontFamily::Cursive => parley::GenericFamily::Cursive,
        stylo::GenericFontFamily::Fantasy => parley::GenericFamily::Fantasy,
        stylo::GenericFontFamily::SystemUi => parley::GenericFamily::SystemUi,
    }
}

/// The family to end a `font-family` list with, if any. When none of the listed
/// families is available, browsers use their default font rather than the
/// system fallback for the script (CSS 2.1 § 15.5, step 4). Blitz's default font
/// is the initial value, `serif`. A list that names a generic family always
/// finds a font, so it needs none.
pub(crate) fn fallback_font_family(
    families: &stylo::FontFamilyList,
) -> Option<parley::GenericFamily> {
    let names_generic = families
        .iter()
        .any(|family| matches!(family, stylo::SingleFontFamily::Generic(_)));
    (!names_generic).then_some(parley::GenericFamily::Serif)
}

/// pyro: whether `family` names a hidden system font: macOS's own UI fonts go by names that start
/// with a dot (`.AppleSystemUIFont`, `.SF NS Mono`), which pages cannot reach (as in Safari and
/// Chrome; they have the `system-ui` and `ui-*` generics).
pub(crate) fn is_hidden_family(family: &stylo::SingleFontFamily) -> bool {
    matches!(family, stylo::SingleFontFamily::FamilyName(name) if name.name.starts_with('.'))
}

pub(crate) fn query_font_family(input: &stylo::SingleFontFamily) -> parley::QueryFamily<'_> {
    match input {
        stylo::SingleFontFamily::FamilyName(name) => {
            'ret: {
                let name = name.name.as_str();

                // Legacy web compatibility
                #[cfg(target_vendor = "apple")]
                if name == "-apple-system" {
                    break 'ret parley::QueryFamily::Generic(parley::GenericFamily::SystemUi);
                }
                #[cfg(target_os = "macos")]
                if name == "BlinkMacSystemFont" {
                    break 'ret parley::QueryFamily::Generic(parley::GenericFamily::SystemUi);
                }

                break 'ret parley::QueryFamily::Named(name);
            }
        }
        stylo::SingleFontFamily::Generic(generic) => {
            parley::QueryFamily::Generic(self::generic_font_family(*generic))
        }
    }
}

pub(crate) fn font_weight(input: stylo::FontWeight) -> parley::FontWeight {
    parley::FontWeight::new(input.value())
}

pub(crate) fn font_width(input: stylo::FontWidth) -> parley::FontWidth {
    parley::FontWidth::from_percentage(input.0.to_float())
}

pub(crate) fn font_style(input: stylo::FontStyle) -> parley::FontStyle {
    match input {
        stylo::FontStyle::NORMAL => parley::FontStyle::Normal,
        stylo::FontStyle::ITALIC => parley::FontStyle::Italic,
        val => parley::FontStyle::Oblique(Some(val.oblique_degrees())),
    }
}

/// Converts `font-variation-settings`, adding an `opsz` axis value equal to the font size in CSS
/// pixels when `font-optical-sizing` is `auto` (as browsers do), unless `opsz` is set explicitly.
pub(crate) fn font_variations(
    font_styles: &stylo::Font,
    font_size: f32,
) -> Vec<parley::FontVariation> {
    const OPSZ: parley::Tag = parley::Tag::new(b"opsz");
    let settings = &font_styles.font_variation_settings.0;
    let auto_opsz = auto_optical_size(font_styles);
    let mut variations = Vec::with_capacity(settings.len() + usize::from(auto_opsz));
    if auto_opsz {
        variations.push(parley::FontVariation {
            tag: OPSZ,
            value: font_size,
        });
    }
    variations.extend(settings.iter().map(|v| parley::FontVariation {
        tag: parley::Tag::from_bytes(v.tag.0.to_be_bytes()),
        value: v.value,
    }));
    variations
}

/// pyro: whether `font_variations` starts with the automatic `opsz` (`font-optical-sizing: auto`
/// and no `opsz` in `font-variation-settings`).
fn auto_optical_size(font_styles: &stylo::Font) -> bool {
    font_styles.font_optical_sizing == stylo::FontOpticalSizing::Auto
        && !font_styles.font_variation_settings.0.iter().any(|v| v.tag.0.to_be_bytes() == *b"opsz")
}

#[inline]
/// pyro: `features` with `tnum` on (tabular digits), unless it sets `tnum` or `pnum`.
pub(crate) fn with_tabular_nums(features: &parley::FontFeatures<'_>) -> Vec<parley::FontFeature> {
    let mut out: Vec<parley::FontFeature> = match features {
        parley::FontFeatures::List(list) => list.to_vec(),
        _ => Vec::new(),
    };
    if !out.iter().any(|f| f.tag == parley::Tag::from_bytes(*b"tnum") || f.tag == parley::Tag::from_bytes(*b"pnum")) {
        out.push(feature(b"tnum", 1));
    }
    out
}

fn feature(tag: &[u8; 4], value: u16) -> parley::FontFeature {
    parley::FontFeature {
        tag: parley::Tag::from_bytes(*tag),
        value,
    }
}

/// Convert the `font-feature-settings` property (low-level OpenType feature control)
/// into a list of Parley font features.
pub(crate) fn font_feature_settings(
    input: &stylo::FontFeatureSettings,
    out: &mut Vec<parley::FontFeature>,
) {
    out.extend(input.0.iter().map(|v| parley::FontFeature {
        tag: parley::Tag::from_bytes(v.tag.0.to_be_bytes()),
        value: v.value as u16,
    }));
}

/// Map the `font-variant-ligatures` property to OpenType features.
pub(crate) fn font_variant_ligatures(
    input: stylo::FontVariantLigatures,
    out: &mut Vec<parley::FontFeature>,
) {
    use stylo::FontVariantLigatures as L;

    // `none` disables all optional ligature and contextual features.
    if input.contains(L::NONE) {
        out.push(feature(b"liga", 0));
        out.push(feature(b"clig", 0));
        out.push(feature(b"dlig", 0));
        out.push(feature(b"hlig", 0));
        out.push(feature(b"calt", 0));
        return;
    }

    if input.contains(L::COMMON_LIGATURES) {
        out.push(feature(b"liga", 1));
        out.push(feature(b"clig", 1));
    } else if input.contains(L::NO_COMMON_LIGATURES) {
        out.push(feature(b"liga", 0));
        out.push(feature(b"clig", 0));
    }
    if input.contains(L::DISCRETIONARY_LIGATURES) {
        out.push(feature(b"dlig", 1));
    } else if input.contains(L::NO_DISCRETIONARY_LIGATURES) {
        out.push(feature(b"dlig", 0));
    }
    if input.contains(L::HISTORICAL_LIGATURES) {
        out.push(feature(b"hlig", 1));
    } else if input.contains(L::NO_HISTORICAL_LIGATURES) {
        out.push(feature(b"hlig", 0));
    }
    if input.contains(L::CONTEXTUAL) {
        out.push(feature(b"calt", 1));
    } else if input.contains(L::NO_CONTEXTUAL) {
        out.push(feature(b"calt", 0));
    }
}

/// Map the `font-variant-caps` property to OpenType features.
pub(crate) fn font_variant_caps(input: stylo::FontVariantCaps, out: &mut Vec<parley::FontFeature>) {
    match input {
        stylo::FontVariantCaps::Normal => {}
        stylo::FontVariantCaps::SmallCaps => out.push(feature(b"smcp", 1)),
        stylo::FontVariantCaps::AllSmallCaps => {
            out.push(feature(b"smcp", 1));
            out.push(feature(b"c2sc", 1));
        }
        stylo::FontVariantCaps::PetiteCaps => out.push(feature(b"pcap", 1)),
        stylo::FontVariantCaps::AllPetiteCaps => {
            out.push(feature(b"pcap", 1));
            out.push(feature(b"c2pc", 1));
        }
        stylo::FontVariantCaps::Unicase => out.push(feature(b"unic", 1)),
        stylo::FontVariantCaps::TitlingCaps => out.push(feature(b"titl", 1)),
    }
}

/// Map the `font-variant-position` property to OpenType features.
pub(crate) fn font_variant_position(
    input: stylo::FontVariantPosition,
    out: &mut Vec<parley::FontFeature>,
) {
    match input {
        stylo::FontVariantPosition::Normal => {}
        stylo::FontVariantPosition::Sub => out.push(feature(b"subs", 1)),
        stylo::FontVariantPosition::Super => out.push(feature(b"sups", 1)),
    }
}

/// Map the `font-variant-numeric` property to OpenType features.
pub(crate) fn font_variant_numeric(
    input: stylo::FontVariantNumeric,
    out: &mut Vec<parley::FontFeature>,
) {
    use stylo::FontVariantNumeric as N;

    if input.contains(N::LINING_NUMS) {
        out.push(feature(b"lnum", 1));
    }
    if input.contains(N::OLDSTYLE_NUMS) {
        out.push(feature(b"onum", 1));
    }
    if input.contains(N::PROPORTIONAL_NUMS) {
        out.push(feature(b"pnum", 1));
    }
    if input.contains(N::TABULAR_NUMS) {
        out.push(feature(b"tnum", 1));
    }
    if input.contains(N::DIAGONAL_FRACTIONS) {
        out.push(feature(b"frac", 1));
    }
    if input.contains(N::STACKED_FRACTIONS) {
        out.push(feature(b"afrc", 1));
    }
    if input.contains(N::ORDINAL) {
        out.push(feature(b"ordn", 1));
    }
    if input.contains(N::SLASHED_ZERO) {
        out.push(feature(b"zero", 1));
    }
}

/// Map the `font-variant-east-asian` property to OpenType features.
pub(crate) fn font_variant_east_asian(
    input: stylo::FontVariantEastAsian,
    out: &mut Vec<parley::FontFeature>,
) {
    use stylo::FontVariantEastAsian as E;

    if input.contains(E::JIS78) {
        out.push(feature(b"jp78", 1));
    }
    if input.contains(E::JIS83) {
        out.push(feature(b"jp83", 1));
    }
    if input.contains(E::JIS90) {
        out.push(feature(b"jp90", 1));
    }
    if input.contains(E::JIS04) {
        out.push(feature(b"jp04", 1));
    }
    if input.contains(E::SIMPLIFIED) {
        out.push(feature(b"smpl", 1));
    }
    if input.contains(E::TRADITIONAL) {
        out.push(feature(b"trad", 1));
    }
    if input.contains(E::FULL_WIDTH) {
        out.push(feature(b"fwid", 1));
    }
    if input.contains(E::PROPORTIONAL_WIDTH) {
        out.push(feature(b"pwid", 1));
    }
    if input.contains(E::RUBY) {
        out.push(feature(b"ruby", 1));
    }
}

/// Build the full list of OpenType font features for a computed font style.
///
/// Per the CSS Fonts feature precedence rules
/// (<https://drafts.csswg.org/css-fonts/#feature-precedence>), features implied by the
/// high-level `font-variant-*` properties are applied first, and the low-level
/// `font-feature-settings` property is applied last so that it takes precedence.
///
/// Parley stably sorts the feature list by tag and HarfBuzz applies the last of any
/// duplicate tags, so appending `font-feature-settings` after the `font-variant-*`
/// features gives it the higher precedence required by the spec.
pub(crate) fn font_features(font_styles: &stylo::Font) -> Vec<parley::FontFeature> {
    let mut features = Vec::new();

    // pyro: `font-kerning` and `font-variant-alternates` were not applied.
    self::font_kerning(font_styles.font_kerning, &mut features);
    self::font_variant_alternates(font_styles, &mut features);

    self::font_variant_ligatures(font_styles.font_variant_ligatures, &mut features);
    self::font_variant_caps(font_styles.font_variant_caps, &mut features);
    self::font_variant_numeric(font_styles.font_variant_numeric, &mut features);
    self::font_variant_east_asian(font_styles.font_variant_east_asian, &mut features);
    self::font_variant_position(font_styles.font_variant_position, &mut features);

    self::font_feature_settings(&font_styles.font_feature_settings, &mut features);

    // pyro: the last setting of a tag wins (`font-feature-settings` over `font-variant-*`);
    // `dedup_by_key` kept the first.
    features.sort_by_key(|feature| feature.tag);
    features.reverse();
    features.dedup_by_key(|feature| feature.tag);
    features.reverse();

    features
}

/// pyro: `font-kerning`: `none` turns the `kern` feature off, `normal` on (`auto` leaves it to
/// the shaper, which kerns).
pub(crate) fn font_kerning(
    input: style::values::computed::font::FontKerning,
    out: &mut Vec<parley::FontFeature>,
) {
    use style::values::computed::font::FontKerning;
    match input {
        FontKerning::Auto => {}
        FontKerning::None => out.push(feature(b"kern", 0)),
        FontKerning::Normal => out.push(feature(b"kern", 1)),
    }
}

thread_local! {
    /// pyro: for the layout running on this thread: the factor `font-size-adjust` scales the
    /// font size by, by the address of the font style struct of each style that has the
    /// property (`BaseDocument::publish_font_size_adjustments`). The conversions have neither
    /// the document nor its fonts (which the layout has locked).
    static FONT_SIZE_ADJUSTMENTS: std::cell::RefCell<std::collections::HashMap<usize, f32>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// pyro: see `FONT_SIZE_ADJUSTMENTS`.
pub(crate) fn set_font_size_adjustments(scales: std::collections::HashMap<usize, f32>) {
    FONT_SIZE_ADJUSTMENTS.with(|slot| *slot.borrow_mut() = scales);
}

/// pyro: the factor `font-size-adjust` scales this font style's size by (1 without it).
pub(crate) fn font_size_adjustment(font_styles: &stylo::Font) -> f32 {
    use style::values::computed::font::FontSizeAdjust;
    if font_styles.font_size_adjust == FontSizeAdjust::None {
        return 1.0;
    }
    let key = font_styles as *const stylo::Font as usize;
    FONT_SIZE_ADJUSTMENTS.with(|scales| scales.borrow().get(&key).copied().unwrap_or(1.0))
}

thread_local! {
    /// pyro: the document's `@font-feature-values` rules, in cascade order, for the layout
    /// running on this thread (`set_font_feature_values`): `font-variant-alternates` names
    /// its values through them.
    static FONT_FEATURE_VALUES: std::cell::RefCell<Vec<style::servo_arc::Arc<style::stylesheets::FontFeatureValuesRule>>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// pyro: gives the conversions on this thread the document's `@font-feature-values` rules.
pub(crate) fn set_font_feature_values(
    rules: Vec<style::servo_arc::Arc<style::stylesheets::FontFeatureValuesRule>>,
) {
    FONT_FEATURE_VALUES.with(|slot| *slot.borrow_mut() = rules);
}

/// pyro: `font-variant-alternates` as OpenType features (CSS Fonts 4 §6.8): `historical-forms`
/// is `hist`; the functions name values of the `@font-feature-values` rules for a family of the
/// element's `font-family` (`stylistic(x)` is `salt <n>`, `styleset(x)` `ssNN`,
/// `character-variant(x)` `cvNN <n>`, `swash(x)` `swsh <n>` and `cswh <n>`, `ornaments(x)`
/// `ornm <n>`, `annotation(x)` `nalt <n>`). A name no rule defines sets nothing.
pub(crate) fn font_variant_alternates(font_styles: &stylo::Font, out: &mut Vec<parley::FontFeature>) {
    use style::values::specified::font::VariantAlternates as V;
    let alternates = &font_styles.font_variant_alternates;
    if alternates.is_empty() {
        return;
    }
    let families: Vec<String> = font_styles
        .font_family
        .families
        .list
        .iter()
        .filter_map(|family| match family {
            stylo::SingleFontFamily::FamilyName(name) => Some(name.name.to_string().to_ascii_lowercase()),
            stylo::SingleFontFamily::Generic(_) => None,
        })
        .collect();
    FONT_FEATURE_VALUES.with(|rules| {
        let rules = rules.borrow();
        let rules: Vec<_> = rules
            .iter()
            .filter(|rule| {
                rule.family_names
                    .iter()
                    .any(|family| families.iter().any(|f| family.name.to_string().eq_ignore_ascii_case(f)))
            })
            .collect();
        // The last rule that defines a name wins.
        macro_rules! lookup {
            ($block:ident, $name:expr) => {
                rules
                    .iter()
                    .rev()
                    .find_map(|rule| rule.$block.iter().rev().find(|d| d.name == $name.0).map(|d| &d.value))
            };
        }
        let numbered = |prefix: &[u8; 2], number: u32, value: u16| {
            (1..=99).contains(&number).then(|| {
                let tag = [prefix[0], prefix[1], b'0' + (number / 10) as u8, b'0' + (number % 10) as u8];
                feature(&tag, value)
            })
        };
        let small = |value: u32| value.min(u16::MAX as u32) as u16;
        for alternate in alternates.iter() {
            match alternate {
                V::HistoricalForms => out.push(feature(b"hist", 1)),
                V::Stylistic(name) => {
                    if let Some(value) = lookup!(stylistic, name) {
                        out.push(feature(b"salt", small(value.0)));
                    }
                }
                V::Swash(name) => {
                    if let Some(value) = lookup!(swash, name) {
                        out.push(feature(b"swsh", small(value.0)));
                        out.push(feature(b"cswh", small(value.0)));
                    }
                }
                V::Ornaments(name) => {
                    if let Some(value) = lookup!(ornaments, name) {
                        out.push(feature(b"ornm", small(value.0)));
                    }
                }
                V::Annotation(name) => {
                    if let Some(value) = lookup!(annotation, name) {
                        out.push(feature(b"nalt", small(value.0)));
                    }
                }
                V::Styleset(names) => {
                    for name in names.iter() {
                        if let Some(values) = lookup!(styleset, name) {
                            out.extend(values.0.iter().filter_map(|n| numbered(b"ss", *n, 1)));
                        }
                    }
                }
                V::CharacterVariant(names) => {
                    for name in names.iter() {
                        if let Some(value) = lookup!(character_variant, name) {
                            out.extend(numbered(b"cv", value.0, small(value.1.unwrap_or(1))));
                        }
                    }
                }
            }
        }
    });
}

pub(crate) fn base_direction(
    direction: stylo::Direction,
    unicode_bidi: stylo::UnicodeBidi,
) -> parley::BaseDirection {
    if unicode_bidi == stylo::UnicodeBidi::Plaintext {
        return parley::BaseDirection::Auto;
    }
    match direction {
        stylo::Direction::Ltr => parley::BaseDirection::Ltr,
        stylo::Direction::Rtl => parley::BaseDirection::Rtl,
    }
}

pub(crate) fn text_align(input: stylo::TextAlignKeyword) -> parley::Alignment {
    match input {
        stylo::TextAlignKeyword::Start => parley::Alignment::Start,
        stylo::TextAlignKeyword::Left => parley::Alignment::Left,
        stylo::TextAlignKeyword::Right => parley::Alignment::Right,
        stylo::TextAlignKeyword::Center => parley::Alignment::Center,
        stylo::TextAlignKeyword::Justify | stylo::TextAlignKeyword::JustifyAll => parley::Alignment::Justify,
        stylo::TextAlignKeyword::End => parley::Alignment::End,
        stylo::TextAlignKeyword::MozCenter => parley::Alignment::Center,
        stylo::TextAlignKeyword::MozLeft => parley::Alignment::Left,
        stylo::TextAlignKeyword::MozRight => parley::Alignment::Right,
    }
}

/// Returns `None` for `text-align-last: auto`
pub(crate) fn text_align_last(input: stylo::TextAlignLast) -> Option<parley::Alignment> {
    match input {
        stylo::TextAlignLast::Auto => None,
        stylo::TextAlignLast::Start => Some(parley::Alignment::Start),
        stylo::TextAlignLast::End => Some(parley::Alignment::End),
        stylo::TextAlignLast::Left => Some(parley::Alignment::Left),
        stylo::TextAlignLast::Right => Some(parley::Alignment::Right),
        stylo::TextAlignLast::Center => Some(parley::Alignment::Center),
        stylo::TextAlignLast::Justify => Some(parley::Alignment::Justify),
    }
}

pub(crate) fn text_wrap_mode(input: stylo::TextWrapMode) -> parley::TextWrapMode {
    match input {
        stylo::TextWrapMode::Wrap => parley::TextWrapMode::Wrap,
        stylo::TextWrapMode::Nowrap => parley::TextWrapMode::NoWrap,
    }
}

pub(crate) fn white_space_collapse(input: stylo::WhiteSpaceCollapse) -> parley::WhiteSpaceCollapse {
    match input {
        stylo::WhiteSpaceCollapse::Collapse => parley::WhiteSpaceCollapse::Collapse,
        stylo::WhiteSpaceCollapse::Preserve => parley::WhiteSpaceCollapse::Preserve,

        stylo::WhiteSpaceCollapse::PreserveBreaks => parley::WhiteSpaceCollapse::PreserveBreaks,
        stylo::WhiteSpaceCollapse::BreakSpaces => parley::WhiteSpaceCollapse::BreakSpaces,
        // Parley has no mode that preserves spaces but converts segment breaks to spaces
        stylo::WhiteSpaceCollapse::PreserveSpaces => parley::WhiteSpaceCollapse::Preserve,
    }
}

/// Map the css-inline-3 `alignment-baseline` and `baseline-shift` longhands (which Stylo
/// stores in place of the `vertical-align` shorthand) to Parley's `VerticalAlign`.
///
/// Percentages are resolved against the element's own `line-height`.
pub(crate) fn vertical_align(style: &stylo::ComputedValues) -> parley::VerticalAlign {
    let box_styles = style.get_box();
    // pyro: `vertical-align`'s keywords align as CSS 2.1 has them; the `alignment-baseline`
    // longhand's (and, for `baseline`, the inherited `dominant-baseline`'s) align the span's
    // baselines of that name with its parent's, as Blink does (WPT css-inline
    // `alignment-baseline-*`, `dominant-baseline-*`).
    let alignment = match box_styles.slow_clone_alignment_baseline() {
        stylo::AlignmentBaseline::Baseline => {
            use style::values::computed::DominantBaseline;
            match style.get_inherited_box().slow_clone_dominant_baseline() {
                DominantBaseline::TextTop => parley::AlignmentBaseline::TextOver,
                DominantBaseline::TextBottom => parley::AlignmentBaseline::TextUnder,
                DominantBaseline::Middle => parley::AlignmentBaseline::XMiddle,
                _ => parley::AlignmentBaseline::Baseline,
            }
        }
        stylo::AlignmentBaseline::VerticalAlignTextTop => parley::AlignmentBaseline::TextTop,
        stylo::AlignmentBaseline::VerticalAlignTextBottom => parley::AlignmentBaseline::TextBottom,
        stylo::AlignmentBaseline::VerticalAlignMiddle => parley::AlignmentBaseline::Middle,
        stylo::AlignmentBaseline::TextTop => parley::AlignmentBaseline::TextOver,
        stylo::AlignmentBaseline::TextBottom => parley::AlignmentBaseline::TextUnder,
        stylo::AlignmentBaseline::Middle => parley::AlignmentBaseline::XMiddle,
    };
    let shift = match box_styles.get_baseline_shift() {
        stylo::BaselineShift::Keyword(stylo::BaselineShiftKeyword::Sub) => {
            parley::BaselineShift::Sub
        }
        stylo::BaselineShift::Keyword(stylo::BaselineShiftKeyword::Super) => {
            parley::BaselineShift::Super
        }
        stylo::BaselineShift::Keyword(stylo::BaselineShiftKeyword::Top) => {
            parley::BaselineShift::Top
        }
        stylo::BaselineShift::Keyword(stylo::BaselineShiftKeyword::Bottom) => {
            parley::BaselineShift::Bottom
        }
        // TODO: `center` (align the aligned subtree's centre with the line box's centre) is not
        // representable in Parley yet. Approximate it with `middle`.
        stylo::BaselineShift::Keyword(stylo::BaselineShiftKeyword::Center) => {
            return parley::VerticalAlign::MIDDLE;
        }
        stylo::BaselineShift::Length(lp) => {
            let shift = if lp.has_percentage() {
                let font_styles = style.get_font();
                let font_size = font_styles.font_size.used_size.0.px();
                let line_height = match font_styles.line_height {
                    // TODO: `normal` resolves against the first available font's metrics, which
                    // aren't available here; 1.2 is the usual approximation.
                    stylo::LineHeight::Normal => font_size * 1.2,
                    stylo::LineHeight::Number(num) => font_size * num.0,
                    stylo::LineHeight::Length(value) => value.0.px(),
                };
                return parley::VerticalAlign::new(
                    alignment,
                    parley::BaselineShift::LineHeight(lp.resolve(Length::new(line_height)).px()),
                );
            } else {
                lp.resolve(Length::new(0.0)).px()
            };
            parley::BaselineShift::Length(shift)
        }
    };
    parley::VerticalAlign::new(alignment, shift)
}

/// pyro: CSS Text 4 `text-spacing-trim`. Font features that set the widths of fullwidth
/// punctuation themselves (`halt`, `palt`, `font-variant-east-asian: proportional-width`, ...)
/// leave it all to the font, as in Blink.
/// pyro: CSS `text-fit` of a block container, `None` for `none`.
pub(crate) fn text_fit(style: &style::properties::ComputedValues) -> Option<parley::TextFit> {
    use style::values::generics::text::{TextFitKind, TextFitTarget};
    let fit = &style.get_inherited_text().text_fit;
    let kind = match fit.kind {
        TextFitKind::None => return None,
        TextFitKind::Grow => parley::TextFitKind::Grow,
        TextFitKind::Shrink => parley::TextFitKind::Shrink,
    };
    let target = match fit.target {
        TextFitTarget::Consistent => parley::TextFitTarget::Consistent,
        TextFitTarget::PerLine => parley::TextFitTarget::PerLine,
        TextFitTarget::PerLineAll => parley::TextFitTarget::PerLineAll,
    };
    Some(parley::TextFit { kind, target, limit: fit.limit.map(|limit| limit.0) })
}

fn text_spacing_trim(
    style: &stylo::ComputedValues,
    features: &[parley::FontFeature],
) -> parley::TextSpacingTrim {
    use style::values::computed::TextSpacingTrim as Trim;
    // Blink's `IsExclusiveFeature`.
    const WIDTH_FEATURES: [&[u8; 4]; 9] =
        [b"halt", b"hwid", b"palt", b"pwid", b"qwid", b"twid", b"valt", b"vhal", b"vpal"];
    if features
        .iter()
        .any(|feature| feature.value != 0 && WIDTH_FEATURES.contains(&&feature.tag.to_bytes()))
    {
        return parley::TextSpacingTrim::SpaceAll;
    }
    match style.get_inherited_text().text_spacing_trim {
        Trim::Normal | Trim::Auto => parley::TextSpacingTrim::Normal,
        Trim::SpaceAll => parley::TextSpacingTrim::SpaceAll,
        Trim::SpaceFirst => parley::TextSpacingTrim::SpaceFirst,
        Trim::TrimStart => parley::TextSpacingTrim::TrimStart,
        Trim::TrimBoth => parley::TextSpacingTrim::TrimBoth,
        Trim::TrimAll => parley::TextSpacingTrim::TrimAll,
    }
}

/// pyro: CSS Text 4 `text-autospace`. Upright text in a vertical writing mode (`text-orientation:
/// upright`, `text-combine-upright: all`) gets none, as in Blink.
fn text_autospace(style: &stylo::ComputedValues) -> parley::TextAutospace {
    use style::values::computed::TextAutospace as Autospace;
    let itext_styles = style.get_inherited_text();
    let value = itext_styles.text_autospace;
    if value.is_empty() {
        return parley::TextAutospace::NONE;
    }
    let wm = style.writing_mode;
    if wm.is_vertical()
        && !wm.is_sideways()
        && (wm.contains(style::logical_geometry::WritingMode::UPRIGHT)
            || itext_styles.text_combine_upright == style::values::computed::TextCombineUpright::All)
    {
        return parley::TextAutospace::NONE;
    }
    if value.intersects(Autospace::NORMAL | Autospace::AUTO) {
        return parley::TextAutospace::NORMAL;
    }
    parley::TextAutospace {
        ideograph_alpha: value.contains(Autospace::IDEOGRAPH_ALPHA),
        ideograph_numeric: value.contains(Autospace::IDEOGRAPH_NUMERIC),
    }
}

pub(crate) fn style(
    span_id: NodeId,
    style: &stylo::ComputedValues,
) -> parley::TextStyle<'static, 'static, TextBrush> {
    let font_styles = style.get_font();
    let itext_styles = style.get_inherited_text();

    // Convert font size and line height
    let font_size = font_styles.font_size.used_size.0.px();
    // pyro: `font-size-adjust` scales the used font size (`set_font_size_adjustments`), not the
    // computed one a `line-height` number multiplies.
    let adjust = font_size_adjustment(font_styles);
    let line_height = match font_styles.line_height {
        stylo::LineHeight::Normal => parley::LineHeight::NORMAL,
        stylo::LineHeight::Number(num) if adjust != 1.0 => parley::LineHeight::Absolute(num.0 * font_size),
        stylo::LineHeight::Number(num) => parley::LineHeight::FontSizeRelative(num.0),
        stylo::LineHeight::Length(value) => parley::LineHeight::Absolute(value.0.px()),
    };
    let font_size = font_size * adjust;
    let vertical_align = self::vertical_align(style);

    // pyro: combined text (`text-combine-upright: all`, the horizontal box Stylo makes of it in a
    // vertical line) is composed without letter spacing (css-writing-modes-4 §9.1.1).
    // (Stylo flags the box it made of it: the property does nothing to horizontal text.)
    let combined = style.flags.contains(style::computed_value_flags::ComputedValueFlags::IS_TEXT_COMBINED);
    let letter_spacing = if combined {
        0.0
    } else {
        itext_styles.letter_spacing.0.resolve(Length::new(font_size)).px()
    };

    let word_spacing = itext_styles
        .word_spacing
        .resolve(Length::new(font_size))
        .px();

    // Convert Bold/Italic
    let font_weight = self::font_weight(font_styles.font_weight);
    let font_style = self::font_style(font_styles.font_style);
    let font_width = self::font_width(font_styles.font_width);
    let font_variations = self::font_variations(font_styles, font_size);
    let font_features = self::font_features(font_styles);

    // Convert font family
    let mut families: Vec<_> = font_styles
        .font_family
        .families
        .list
        .iter()
        .filter(|family| !is_hidden_family(family))
        .map(|family| match family {
            stylo::SingleFontFamily::FamilyName(name) => {
                'ret: {
                    let name = name.name.as_str();

                    // Legacy web compatibility
                    #[cfg(target_vendor = "apple")]
                    if name == "-apple-system" {
                        break 'ret parley::FontFamilyName::Generic(
                            parley::GenericFamily::SystemUi,
                        );
                    }
                    #[cfg(target_os = "macos")]
                    if name == "BlinkMacSystemFont" {
                        break 'ret parley::FontFamilyName::Generic(
                            parley::GenericFamily::SystemUi,
                        );
                    }

                    break 'ret parley::FontFamilyName::Named(Cow::Owned(name.to_string()));
                }
            }
            stylo::SingleFontFamily::Generic(generic) => {
                parley::FontFamilyName::Generic(self::generic_font_family(*generic))
            }
        })
        .collect();
    families.extend(
        self::fallback_font_family(&font_styles.font_family.families)
            .map(parley::FontFamilyName::Generic),
    );

    // Wrapping and breaking
    let word_break = match itext_styles.word_break {
        stylo::WordBreak::Normal => parley::WordBreak::Normal,
        stylo::WordBreak::BreakAll => parley::WordBreak::BreakAll,
        stylo::WordBreak::KeepAll | stylo::WordBreak::AutoPhrase => parley::WordBreak::KeepAll,
        stylo::WordBreak::BreakWord | stylo::WordBreak::Manual => parley::WordBreak::Normal,
    };
    let dictionary_breaks = itext_styles.word_break != stylo::WordBreak::Manual;
    let line_break = match itext_styles.line_break {
        stylo::LineBreak::Loose => parley::LineBreak::Loose,
        stylo::LineBreak::Normal => parley::LineBreak::Normal,
        stylo::LineBreak::Auto | stylo::LineBreak::Strict => parley::LineBreak::Strict,
        stylo::LineBreak::Anywhere => parley::LineBreak::Anywhere,
    };
    // `word-break: break-word` implies `overflow-wrap: anywhere`
    let overflow_wrap = match itext_styles.overflow_wrap {
        _ if itext_styles.word_break == stylo::WordBreak::BreakWord => {
            parley::OverflowWrap::Anywhere
        }
        stylo::OverflowWrap::Normal => parley::OverflowWrap::Normal,
        stylo::OverflowWrap::BreakWord => parley::OverflowWrap::BreakWord,
        stylo::OverflowWrap::Anywhere => parley::OverflowWrap::Anywhere,
    };
    let text_wrap_mode = text_wrap_mode(itext_styles.text_wrap_mode);

    parley::TextStyle {
        // font_family: parley::FontFamily::Single(FontFamilyName::Generic(GenericFamily::SystemUi)),
        font_family: parley::FontFamily::List(Cow::Owned(families)),
        font_size,
        font_width,
        font_style,
        font_weight,
        font_variations: parley::FontVariations::List(Cow::Owned(font_variations)),
        font_features: parley::FontFeatures::List(Cow::Owned(font_features.clone())),
        locale: parley::Language::parse(&font_styles._x_lang.0).ok(),
        line_height,
        vertical_align,
        word_spacing,
        letter_spacing,
        // pyro: percentages scale with the text on lines `text-fit` scales.
        spacing_scales: parley::SpacingScales {
            letter: itext_styles.letter_spacing.0.has_percentage(),
            word: itext_styles.word_spacing.has_percentage(),
        },
        text_wrap_mode,
        white_space_collapse: white_space_collapse(itext_styles.white_space_collapse),
        dictionary_breaks,
        // (Set where an inline element's span is built, `inline::build`.)
        isolates: false,
        // (Set where an inline element's span is built, `inline::build`.)
        wrap_avoid_depth: 0,
        full_width_space: style
            .get_inherited_text()
            .text_transform
            .contains(style::values::specified::text::TextTransform::FULL_WIDTH),
        hanging_punctuation: {
            use style::values::computed::HangingPunctuation as H;
            let value = itext_styles.hanging_punctuation;
            (u8::from(value.contains(H::FORCE_END)) << 1)
                | (u8::from(value.contains(H::ALLOW_END)) << 2)
                | (u8::from(value.contains(H::LAST)) << 3)
        },
        phrase_breaks: itext_styles.word_break == stylo::WordBreak::AutoPhrase,
        // pyro: `font-size-adjust` sets the size (a face's `size-adjust` is then not applied).
        font_size_adjusted: font_styles.font_size_adjust != style::values::computed::font::FontSizeAdjust::None,
        // pyro: the first variation is the automatic `opsz` (`font_variations`).
        auto_optical_size: auto_optical_size(font_styles),
        // pyro: `font-language-override` names the OpenType language system to shape with.
        language_override: (font_styles.font_language_override.0 != 0)
            .then(|| font_styles.font_language_override.0.to_be_bytes()),
        soft_hyphen: match (itext_styles.hyphens, &itext_styles.hyphenate_character) {
            (style::computed_values::hyphens::T::None, _) => parley::SoftHyphen::NoBreak,
            (_, style::values::computed::HyphenateCharacter::Auto) => parley::SoftHyphen::Auto,
            (_, style::values::computed::HyphenateCharacter::String(string)) => {
                let mut chars = string.chars();
                match (chars.next(), chars.next()) {
                    (None, _) => parley::SoftHyphen::Hidden,
                    (Some(ch), None) => parley::SoftHyphen::Char(ch),
                    _ if string.chars().count() <= 4 => {
                        let mut chars = ['\0'; 4];
                        let mut len = 0;
                        for ch in string.chars() {
                            chars[len] = ch;
                            len += 1;
                        }
                        parley::SoftHyphen::Chars(chars, len as u8)
                    }
                    _ => parley::SoftHyphen::Auto,
                }
            }
        },
        tab_size: match itext_styles.tab_size {
            style::values::generics::length::LengthOrNumber::Number(number) => parley::TabSize::Spaces(number.0),
            style::values::generics::length::LengthOrNumber::Length(length) => parley::TabSize::Length(length.0.px()),
        },
        overflow_wrap,
        word_break,
        line_break,
        text_spacing_trim: text_spacing_trim(style, &font_features),
        text_autospace: text_autospace(style),
        upright: style.writing_mode.is_vertical()
            && !style.writing_mode.is_sideways()
            && style.writing_mode.contains(style::logical_geometry::WritingMode::UPRIGHT),
        vertical: style.writing_mode.is_vertical() && !style.writing_mode.is_sideways(),
        text_justify: match itext_styles.text_justify {
            style::values::computed::TextJustify::Auto => parley::TextJustify::Auto,
            style::values::computed::TextJustify::None => parley::TextJustify::None,
            style::values::computed::TextJustify::InterWord => parley::TextJustify::InterWord,
            style::values::computed::TextJustify::InterCharacter => parley::TextJustify::InterCharacter,
        },

        // Contains NodeId
        brush: TextBrush::from_id(span_id),

        // We avoid sending these styles through Parley because they don't affect layout
        // and handling them separately allows us to update them without rebuilding the Parley layout.
        //
        // Instead of setting them here we pass the NodeId in the `brush` field and use that to read these
        // styles lazily when rendering.
        has_underline: Default::default(),
        underline_offset: Default::default(),
        underline_size: Default::default(),
        underline_brush: Default::default(),
        has_strikethrough: Default::default(),
        strikethrough_offset: Default::default(),
        strikethrough_size: Default::default(),
        strikethrough_brush: Default::default(),
        // pyro: set by the inline build for spans with annotations (`TextAnnotation`).
        annotation_space: Default::default(),
        // pyro: set by the inline build for inline boxes (`layout::text_box`).
        text_box_trim: Default::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use style::values::generics::font::{FeatureTagValue, FontSettings, FontTag};

    /// Collect the (tag, value) pairs from a feature list for easy assertions.
    fn pairs(features: &[parley::FontFeature]) -> Vec<([u8; 4], u16)> {
        features
            .iter()
            .map(|f| (f.tag.to_bytes(), f.value))
            .collect()
    }

    fn feature_settings(tags: &[(&[u8; 4], i32)]) -> stylo::FontFeatureSettings {
        FontSettings(
            tags.iter()
                .map(|(tag, value)| FeatureTagValue {
                    tag: FontTag(u32::from_be_bytes(**tag)),
                    value: *value,
                })
                .collect(),
        )
    }

    #[test]
    fn ligatures_none_disables_all() {
        let mut out = Vec::new();
        font_variant_ligatures(stylo::FontVariantLigatures::NONE, &mut out);
        assert_eq!(
            pairs(&out),
            vec![
                (*b"liga", 0),
                (*b"clig", 0),
                (*b"dlig", 0),
                (*b"hlig", 0),
                (*b"calt", 0),
            ]
        );
    }

    #[test]
    fn ligatures_normal_emits_nothing() {
        let mut out = Vec::new();
        font_variant_ligatures(stylo::FontVariantLigatures::NORMAL, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn ligatures_mixed_toggles() {
        let mut out = Vec::new();
        let input = stylo::FontVariantLigatures::COMMON_LIGATURES
            | stylo::FontVariantLigatures::NO_DISCRETIONARY_LIGATURES
            | stylo::FontVariantLigatures::HISTORICAL_LIGATURES;
        font_variant_ligatures(input, &mut out);
        assert_eq!(
            pairs(&out),
            vec![(*b"liga", 1), (*b"clig", 1), (*b"dlig", 0), (*b"hlig", 1)]
        );
    }

    #[test]
    fn caps_small_caps() {
        let mut out = Vec::new();
        font_variant_caps(stylo::FontVariantCaps::SmallCaps, &mut out);
        assert_eq!(pairs(&out), vec![(*b"smcp", 1)]);

        let mut out = Vec::new();
        font_variant_caps(stylo::FontVariantCaps::Normal, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn position_sub_and_super() {
        let mut out = Vec::new();
        font_variant_position(stylo::FontVariantPosition::Sub, &mut out);
        assert_eq!(pairs(&out), vec![(*b"subs", 1)]);

        let mut out = Vec::new();
        font_variant_position(stylo::FontVariantPosition::Super, &mut out);
        assert_eq!(pairs(&out), vec![(*b"sups", 1)]);
    }

    #[test]
    fn numeric_features() {
        let mut out = Vec::new();
        let input = stylo::FontVariantNumeric::OLDSTYLE_NUMS
            | stylo::FontVariantNumeric::TABULAR_NUMS
            | stylo::FontVariantNumeric::SLASHED_ZERO;
        font_variant_numeric(input, &mut out);
        assert_eq!(
            pairs(&out),
            vec![(*b"onum", 1), (*b"tnum", 1), (*b"zero", 1)]
        );
    }

    #[test]
    fn east_asian_features() {
        let mut out = Vec::new();
        let input =
            stylo::FontVariantEastAsian::JIS04 | stylo::FontVariantEastAsian::PROPORTIONAL_WIDTH;
        font_variant_east_asian(input, &mut out);
        assert_eq!(pairs(&out), vec![(*b"jp04", 1), (*b"pwid", 1)]);
    }

    #[test]
    fn feature_settings_map_directly() {
        let mut out = Vec::new();
        font_feature_settings(&feature_settings(&[(b"smcp", 1), (b"tnum", 0)]), &mut out);
        assert_eq!(pairs(&out), vec![(*b"smcp", 1), (*b"tnum", 0)]);
    }

    /// `font-feature-settings` has a higher precedence than the `font-variant-*`
    /// properties, so its entries must be appended *after* the variant-derived ones.
    /// Parley stably sorts by tag and HarfBuzz applies the last of any duplicate tags,
    /// so the trailing `font-feature-settings` value wins.
    #[test]
    fn feature_settings_override_variant_features() {
        let mut out = Vec::new();
        // font-variant-numeric: tabular-nums => tnum=1
        font_variant_numeric(stylo::FontVariantNumeric::TABULAR_NUMS, &mut out);
        // font-feature-settings: "tnum" 0 (should override the variant)
        font_feature_settings(&feature_settings(&[(b"tnum", 0)]), &mut out);

        let flat = pairs(&out);
        // Both are present, and the feature-settings value comes last.
        assert_eq!(flat, vec![(*b"tnum", 1), (*b"tnum", 0)]);
        let last_tnum = flat.iter().rev().find(|(tag, _)| tag == b"tnum").unwrap();
        assert_eq!(last_tnum.1, 0);
    }
}
