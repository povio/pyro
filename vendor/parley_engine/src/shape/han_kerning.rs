// Copyright 2026 the Parley Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! pyro: fullwidth punctuation set half-width while shaping (CSS Text 4 `text-spacing-trim`), as
//! Blink's `HanKerning` does it: a fullwidth opening bracket after an opening bracket, a closing
//! one or a middle dot, and a fullwidth closing bracket before a closing one or a middle dot, take
//! the font's `halt` feature. What kind of punctuation a full stop, a colon or a quotation mark is
//! depends on its glyph (CJK fonts draw `。` at the start of its em box for Japanese and Simplified
//! Chinese, centred for Traditional Chinese): it is read from what `halt` does to it in the run's
//! font and language.
//!
//! The line breaker does the line edges (a line's first opening bracket, its last closing one):
//! [`CharacterFlags::is_trim_open`](super::CharacterFlags::is_trim_open) and its siblings tell it
//! which characters it may set half-width there.

use alloc::vec::Vec;

/// How the fullwidth punctuation of a shaping item is trimmed while it is shaped.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SpacingTrim {
    /// Not at all (CSS `space-all`).
    #[default]
    None,
    /// Where punctuation meets punctuation (CSS `normal`, and the values that add line edges).
    Adjacent,
    /// All fullwidth punctuation (CSS `trim-all`).
    All,
}

/// The kind of punctuation a character is, for trimming.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CharType {
    /// Anything else.
    Other,
    /// Fullwidth opening punctuation: its glyph's ink is in the end half of its em box.
    Open,
    /// Proportional opening punctuation (`(`).
    OpenNarrow,
    /// Fullwidth closing punctuation (and full stops and commas drawn as such): its ink is in the
    /// start half.
    Close,
    /// Proportional closing punctuation (`)`).
    CloseNarrow,
    /// Fullwidth middle dots, the ideographic space, and colons drawn centred.
    Middle,
}

/// The characters whose kind depends on the font's glyph for them (in the run's language).
const PROBES: [char; 10] = ['、', '。', '，', '．', '：', '；', '‘', '“', '’', '”'];

/// What trimming needs to know about a font in a language.
#[derive(Clone, Debug)]
pub(crate) struct FontData {
    /// Whether the font has the `halt` feature at all.
    pub(crate) has_halt: bool,
    /// The kind of each of [`PROBES`] in this font and language.
    probes: [CharType; PROBES.len()],
}

impl FontData {
    /// A font without `halt`: nothing is trimmed.
    pub(crate) const NONE: Self = Self { has_halt: false, probes: [CharType::Other; PROBES.len()] };

    /// `shape(ch, halt)` shapes one character alone in the run's font, language and script (with
    /// `halt` on or off) and gives its glyph's advance and x offset, `None` for a missing glyph.
    pub(crate) fn new(has_halt: bool, mut shape: impl FnMut(char, bool) -> Option<(f32, f32)>) -> Self {
        if !has_halt {
            return Self::NONE;
        }
        let mut probes = [CharType::Other; PROBES.len()];
        for (slot, &ch) in probes.iter_mut().zip(PROBES.iter()) {
            let ink = match (shape(ch, false), shape(ch, true)) {
                (Some(full), Some(half)) => Ink::of(full, half),
                _ => Ink::Untrimmable,
            };
            *slot = match ch {
                '、' | '。' | '，' | '．' | '：' | '；' => match ink {
                    Ink::Start => CharType::Close,
                    Ink::Centre => CharType::Middle,
                    Ink::End | Ink::Untrimmable => CharType::Other,
                },
                '‘' | '“' => {
                    if ink == Ink::End { CharType::Open } else { CharType::OpenNarrow }
                }
                _ => {
                    if ink == Ink::Start { CharType::Close } else { CharType::CloseNarrow }
                }
            };
        }
        Self { has_halt, probes }
    }

    /// The kind of `ch` in this font.
    pub(crate) fn char_type(&self, ch: char) -> CharType {
        if let Some(index) = PROBES.iter().position(|&probe| probe == ch) {
            return self.probes[index];
        }
        base_char_type(ch)
    }
}

/// Where `halt` says a glyph's ink is, from its advance and x offset without and with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ink {
    /// `halt` leaves it alone (a proportional glyph).
    Untrimmable,
    /// In the start half: `halt` takes the end half off (closing punctuation).
    Start,
    /// In the middle: `halt` takes a quarter off each side.
    Centre,
    /// In the end half: `halt` moves it back by the half it takes off (opening punctuation).
    End,
}

impl Ink {
    fn of((advance, offset): (f32, f32), (half_advance, half_offset): (f32, f32)) -> Self {
        let removed = advance - half_advance;
        if advance <= 0.0 || removed < advance * 0.25 {
            return Self::Untrimmable;
        }
        let moved = (offset - half_offset) / removed;
        if moved < 0.25 {
            Self::Start
        } else if moved > 0.75 {
            Self::End
        } else {
            Self::Centre
        }
    }
}

/// The kind of a character whose kind does not depend on the font.
fn base_char_type(ch: char) -> CharType {
    match ch {
        '\u{3000}' | '\u{30FB}' => CharType::Middle,
        _ => match parley_data::Properties::get(ch).general_category() {
            icu_properties::props::GeneralCategory::OpenPunctuation => {
                if is_wide(ch) { CharType::Open } else { CharType::OpenNarrow }
            }
            icu_properties::props::GeneralCategory::ClosePunctuation => {
                if is_wide(ch) { CharType::Close } else { CharType::CloseNarrow }
            }
            _ => CharType::Other,
        },
    }
}

/// Whether `ch` is East Asian Wide or Fullwidth (UAX #11), for the punctuation that matters here.
fn is_wide(ch: char) -> bool {
    matches!(ch as u32, 0x2E80..=0x303E | 0x3041..=0x33FF | 0xFE10..=0xFE19 | 0xFE30..=0xFE6F | 0xFF01..=0xFF60 | 0xFFE0..=0xFFE6)
}

/// Whether `ch` can take part in trimming: runs without one are shaped as they are.
#[inline]
pub(crate) fn is_candidate(ch: char) -> bool {
    matches!(ch as u32, 0x2018..=0x201D | 0x3000..=0x301F | 0x30FB | 0xFE10..=0xFE19 | 0xFE30..=0xFE4F | 0xFF01..=0xFF65)
}

/// The trimming of one run: which of its characters `halt` applies to while shaping, and what
/// the line breaker may do at line edges.
#[derive(Clone, Debug, Default)]
pub(crate) struct RunTrim {
    /// Per character of the run: [`TRIM_OPEN`], [`TRIM_CLOSE`], [`TRIMMED`].
    pub(crate) flags: Vec<u8>,
}

/// The character is fullwidth opening punctuation the font can set half-width.
pub(crate) const TRIM_OPEN: u8 = 1 << 0;
/// The character is fullwidth closing punctuation the font can set half-width.
pub(crate) const TRIM_CLOSE: u8 = 1 << 1;
/// The character is shaped half-width (`halt` applies to it).
pub(crate) const TRIMMED: u8 = 1 << 2;

impl RunTrim {
    /// Trims `run` (the characters of a run) with `before` and `after` the kinds of the characters
    /// around it (`None` at the text's ends or at an atomic inline).
    pub(crate) fn new(
        mode: SpacingTrim,
        data: &FontData,
        run: &str,
        before: Option<CharType>,
        after: Option<CharType>,
    ) -> Option<Self> {
        if mode == SpacingTrim::None || !data.has_halt {
            return None;
        }
        let types: Vec<CharType> = run.chars().map(|ch| data.char_type(ch)).collect();
        // Whether each character is closing punctuation whatever the font.
        let chars: Vec<(char, bool)> =
            run.chars().map(|ch| (ch, base_char_type(ch) == CharType::Close)).collect();
        let type_at = |i: isize| -> CharType {
            if i < 0 {
                before.unwrap_or(CharType::Other)
            } else if i as usize >= types.len() {
                after.unwrap_or(CharType::Other)
            } else {
                types[i as usize]
            }
        };
        let mut flags = alloc::vec![0_u8; types.len()];
        let mut any = false;
        for (i, &ty) in types.iter().enumerate() {
            let at = i as isize;
            let trimmed = match mode {
                SpacingTrim::None => false,
                SpacingTrim::All => matches!(ty, CharType::Open | CharType::Close | CharType::Middle),
                SpacingTrim::Adjacent => match ty {
                    CharType::Open => matches!(
                        type_at(at - 1),
                        CharType::Open | CharType::OpenNarrow | CharType::Close | CharType::Middle
                    ),
                    CharType::Close => {
                        matches!(type_at(at + 1), CharType::Close | CharType::CloseNarrow | CharType::Middle)
                    }
                    _ => false,
                },
            };
            // A line's end trims closing brackets only, not full stops, commas, colons or quotation
            // marks that are closing punctuation by their glyph (as Chromium).
            let flag = match ty {
                CharType::Open => TRIM_OPEN,
                CharType::Close if chars[i].1 => TRIM_CLOSE,
                _ => 0,
            } | if trimmed { TRIMMED } else { 0 };
            any |= flag != 0;
            flags[i] = flag;
        }
        any.then_some(Self { flags })
    }

    /// The run's characters (indices in the run) that `halt` applies to.
    pub(crate) fn trimmed(&self) -> impl Iterator<Item = usize> + '_ {
        self.flags.iter().enumerate().filter(|(_, flags)| **flags & TRIMMED != 0).map(|(i, _)| i)
    }
}
