// Copyright 2026 the Parley Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! pyro: the East_Asian_Spacing property of UTR #59 (Wide, Narrow, Conditional, Other), which
//! CSS `text-autospace` spaces by, as Blink reads it.

/// A character's East_Asian_Spacing class (UTR #59).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EastAsianSpacing {
    /// Ideographs, kana, Hangul, Bopomofo and the like.
    Wide,
    /// Letters and marks of other scripts.
    Narrow,
    /// Decimal numerals (Narrow; CSS `ideograph-numeric` spaces them).
    Numeric,
    /// Punctuation that is Narrow in Chinese text and Other elsewhere (`!`, `#`, `%`, ...).
    Conditional,
    /// Anything else.
    Other,
}

/// UTR #59's Wide characters below U+3400 and outside the Han blocks (from its data file).
const WIDE: &[(u32, u32)] = &[
    (0x02C7, 0x02C7),
    (0x02C9, 0x02CB),
    (0x02D9, 0x02D9),
    (0x02EA, 0x02EB),
    (0x0305, 0x0305),
    (0x0323, 0x0323),
    (0x1100, 0x11FF),
    (0x3005, 0x3007),
    (0x3013, 0x3013),
    (0x3021, 0x302F),
    (0x3031, 0x3035),
    (0x3038, 0x303C),
    (0x3041, 0x3096),
    (0x3099, 0x309F),
    (0x30A1, 0x30FA),
    (0x30FC, 0x30FF),
    (0x3105, 0x312F),
    (0x3131, 0x318E),
    (0x31A0, 0x31BF),
    (0x31F0, 0x31FF),
    (0x3400, 0x4DBF),
    (0x4E00, 0xA48C),
    (0xA960, 0xA97C),
    (0xAC00, 0xD7A3),
    (0xD7B0, 0xD7C6),
    (0xD7CB, 0xD7FB),
    (0xF900, 0xFA6D),
    (0xFA70, 0xFAD9),
    (0x16FE0, 0x16FE1),
    (0x16FE3, 0x16FE4),
    (0x16FF0, 0x16FF1),
    (0x17000, 0x187F7),
    (0x18800, 0x18CD5),
    (0x18CFF, 0x18D08),
    (0x1AFF0, 0x1AFF3),
    (0x1AFF5, 0x1AFFB),
    (0x1AFFD, 0x1AFFE),
    (0x1B000, 0x1B122),
    (0x1B132, 0x1B132),
    (0x1B150, 0x1B152),
    (0x1B155, 0x1B155),
    (0x1B164, 0x1B167),
    (0x1B170, 0x1B2FB),
    (0x20000, 0x2A6DF),
    (0x2A700, 0x2B739),
    (0x2B740, 0x2B81D),
    (0x2B820, 0x2CEA1),
    (0x2CEB0, 0x2EBE0),
    (0x2EBF0, 0x2EE5D),
    (0x2F800, 0x2FA1D),
    (0x30000, 0x3134A),
    (0x31350, 0x323AF),
];

/// Whether `ch` is East Asian Wide, Fullwidth or Halfwidth (UAX #11), roughly: the blocks that
/// hold such letters, numerals and punctuation. UTR #59 leaves them out of Narrow and Conditional.
fn is_east_asian_width_whf(ch: u32) -> bool {
    matches!(ch, 0x1100..=0x115F | 0x2E80..=0x303E | 0x3041..=0x33FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF
        | 0xA000..=0xA4CF | 0xA960..=0xA97F | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF | 0xFE10..=0xFE19
        | 0xFE30..=0xFE6F | 0xFF00..=0xFFEE | 0x16FE0..=0x1B2FF | 0x1F300..=0x1F64F | 0x1F900..=0x1F9FF
        | 0x20000..=0x3FFFD)
}

/// pyro: the East_Asian_Spacing class (UTR #59) of `ch`, for CSS `text-autospace`.
pub fn east_asian_spacing(ch: char) -> EastAsianSpacing {
    use icu_properties::props::GeneralCategory as Gc;
    let cp = ch as u32;
    if WIDE.binary_search_by(|&(start, end)| {
        if end < cp {
            core::cmp::Ordering::Less
        } else if start > cp {
            core::cmp::Ordering::Greater
        } else {
            core::cmp::Ordering::Equal
        }
    })
    .is_ok()
    {
        return EastAsianSpacing::Wide;
    }
    if is_east_asian_width_whf(cp) {
        return EastAsianSpacing::Other;
    }
    match parley_engine_properties(ch) {
        Gc::OtherPunctuation => {
            if matches!(cp, 0x22 | 0x27 | 0x2A | 0x2F | 0xB7 | 0x2020 | 0x2021 | 0x2026) {
                EastAsianSpacing::Other
            } else {
                EastAsianSpacing::Conditional
            }
        }
        Gc::DecimalNumber => EastAsianSpacing::Numeric,
        Gc::UppercaseLetter
        | Gc::LowercaseLetter
        | Gc::TitlecaseLetter
        | Gc::ModifierLetter
        | Gc::OtherLetter
        | Gc::NonspacingMark
        | Gc::SpacingMark
        | Gc::EnclosingMark => EastAsianSpacing::Narrow,
        _ => EastAsianSpacing::Other,
    }
}

#[inline]
fn parley_engine_properties(ch: char) -> icu_properties::props::GeneralCategory {
    parley_data::Properties::get(ch).general_category()
}

/// pyro: whether `ch` is a mark (Gc=M): `text-autospace` passes over marks, so they take their
/// base's class.
pub fn is_mark(ch: char) -> bool {
    use icu_properties::props::GeneralCategory as Gc;
    matches!(
        parley_engine_properties(ch),
        Gc::NonspacingMark | Gc::SpacingMark | Gc::EnclosingMark
    )
}

