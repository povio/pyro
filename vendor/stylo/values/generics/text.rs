/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Generic types for text properties.

use crate::Zero;
use crate::derives::*;
use std::fmt::{self, Write};
use style_traits::{CssWriter, ToCss};

/// Text inflation control: a keyword or a non-negative percentage.
#[allow(missing_docs)]
#[derive(Animate, Clone, ComputeSquaredDistance, Copy, Debug, MallocSizeOf, Parse, PartialEq,
         SpecifiedValueInfo, ToAnimatedValue, ToAnimatedZero, ToComputedValue, ToCss,
         ToResolvedValue, ToShmem, ToTyped)]
#[repr(C, u8)]
pub enum GenericTextAdjustment<Percentage> {
    Auto,
    None,
    Percentage(Percentage),
}

/// A generic value that is either a number or `auto`.
#[derive(
    Animate,
    Clone,
    ComputeSquaredDistance,
    Copy,
    Debug,
    MallocSizeOf,
    Parse,
    PartialEq,
    SpecifiedValueInfo,
    ToAnimatedValue,
    ToAnimatedZero,
    ToComputedValue,
    ToCss,
    ToResolvedValue,
    ToShmem,
)]
#[repr(C, u8)]
pub enum NumberOrAuto<N> {
    /// `auto`
    Auto,
    /// `<number>`
    Number(N),
}

/// A generic value for the `hyphenate-limit-chars` property.
#[derive(
    Animate,
    Clone,
    ComputeSquaredDistance,
    Debug,
    MallocSizeOf,
    PartialEq,
    SpecifiedValueInfo,
    ToAnimatedValue,
    ToAnimatedZero,
    ToComputedValue,
    ToResolvedValue,
    ToShmem,
    ToTyped,
)]
#[repr(C)]
#[typed(todo_derive_fields)]
pub struct GenericHyphenateLimitChars<Integer> {
    /// Required minimum number of characters in a hyphenated word.
    pub total_word_length: NumberOrAuto<Integer>,
    /// Required minumum number of characters before the hyphen.
    pub pre_hyphen_length: NumberOrAuto<Integer>,
    /// Required minumum number of characters after the hyphen.
    pub post_hyphen_length: NumberOrAuto<Integer>,
}

impl<Integer: ToCss + PartialEq> ToCss for GenericHyphenateLimitChars<Integer> {
    fn to_css<W>(&self, dest: &mut CssWriter<W>) -> fmt::Result
    where
        W: Write,
    {
        self.total_word_length.to_css(dest)?;

        if self.pre_hyphen_length != NumberOrAuto::Auto
            || self.post_hyphen_length != self.pre_hyphen_length
        {
            dest.write_char(' ')?;
            self.pre_hyphen_length.to_css(dest)?;
            if self.post_hyphen_length != self.pre_hyphen_length {
                dest.write_char(' ')?;
                self.post_hyphen_length.to_css(dest)?;
            }
        }

        Ok(())
    }
}

/// pyro: how `text-fit` (CSS Text 5) scales a block's text: not at all, up or down.
#[derive(
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    MallocSizeOf,
    Parse,
    PartialEq,
    SpecifiedValueInfo,
    ToComputedValue,
    ToCss,
    ToResolvedValue,
    ToShmem,
    ToTyped,
)]
#[repr(u8)]
pub enum TextFitKind {
    /// `none`
    None,
    /// `grow`
    Grow,
    /// `shrink`
    Shrink,
}

/// pyro: which lines `text-fit` scales: all by one factor, or each line on its own (the last
/// one too with `per-line-all`).
#[derive(
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    MallocSizeOf,
    Parse,
    PartialEq,
    SpecifiedValueInfo,
    ToComputedValue,
    ToCss,
    ToResolvedValue,
    ToShmem,
    ToTyped,
)]
#[repr(u8)]
pub enum TextFitTarget {
    /// `consistent`
    Consistent,
    /// `per-line`
    PerLine,
    /// `per-line-all`
    PerLineAll,
}

/// pyro: a generic value for the `text-fit` property:
/// `[none | grow | shrink] [consistent | per-line | per-line-all]? <percentage>?`.
#[derive(
    Clone,
    Debug,
    MallocSizeOf,
    PartialEq,
    SpecifiedValueInfo,
    ToComputedValue,
    ToResolvedValue,
    ToShmem,
    ToTyped,
)]
#[repr(C)]
#[typed(todo_derive_fields)]
pub struct GenericTextFit<Percentage> {
    /// Whether text grows, shrinks or neither.
    pub kind: TextFitKind,
    /// Which lines are scaled.
    pub target: TextFitTarget,
    /// The limit on the scale factor (`100%` is 1).
    pub limit: Option<Percentage>,
}

impl<Percentage: ToCss> ToCss for GenericTextFit<Percentage> {
    fn to_css<W>(&self, dest: &mut CssWriter<W>) -> fmt::Result
    where
        W: Write,
    {
        self.kind.to_css(dest)?;
        if self.target != TextFitTarget::Consistent {
            dest.write_char(' ')?;
            self.target.to_css(dest)?;
        }
        if let Some(limit) = &self.limit {
            dest.write_char(' ')?;
            limit.to_css(dest)?;
        }
        Ok(())
    }
}

/// A generic value for the `initial-letter` property.
#[derive(
    Clone,
    Copy,
    Debug,
    MallocSizeOf,
    PartialEq,
    SpecifiedValueInfo,
    ToComputedValue,
    ToResolvedValue,
    ToShmem,
    ToTyped,
)]
#[repr(C)]
pub struct GenericInitialLetter<Number, Integer> {
    /// The size, >=1, or 0 if `normal`.
    pub size: Number,
    /// The sink, >=1, if specified, 0 otherwise.
    pub sink: Integer,
    /// pyro: the `drop` or `raise` keyword given instead of a sink (CSS Inline 3 § 7.2), kept
    /// as written: `sink` is 0 then.
    #[css(skip)]
    pub keyword: InitialLetterKeyword,
}

/// pyro: the keyword of `initial-letter` that stands for its sink: `drop` (the size rounded
/// down) or `raise` (1).
#[derive(
    Clone,
    Copy,
    Debug,
    Eq,
    MallocSizeOf,
    PartialEq,
    SpecifiedValueInfo,
    ToComputedValue,
    ToResolvedValue,
    ToShmem,
)]
#[repr(u8)]
pub enum InitialLetterKeyword {
    /// No keyword: the sink, if any, is a number.
    None,
    /// `drop`
    Drop,
    /// `raise`
    Raise,
}

pub use self::GenericInitialLetter as InitialLetter;
impl<N: Zero, I: Zero> InitialLetter<N, I> {
    /// Returns `normal`.
    #[inline]
    pub fn normal() -> Self {
        InitialLetter {
            size: N::zero(),
            sink: I::zero(),
            keyword: InitialLetterKeyword::None,
        }
    }
}

impl<N: ToCss + Zero, I: ToCss + Zero> ToCss for InitialLetter<N, I> {
    fn to_css<W>(&self, dest: &mut CssWriter<W>) -> fmt::Result
    where
        W: Write,
    {
        if self.size.is_zero() {
            return dest.write_str("normal");
        }
        self.size.to_css(dest)?;
        match self.keyword {
            InitialLetterKeyword::Drop => return dest.write_str(" drop"),
            InitialLetterKeyword::Raise => return dest.write_str(" raise"),
            InitialLetterKeyword::None => {},
        }
        if !self.sink.is_zero() {
            dest.write_char(' ')?;
            self.sink.to_css(dest)?;
        }
        Ok(())
    }
}

/// Implements type for text-decoration-thickness
/// which takes the grammar of auto | from-font | <length> | <percentage>
///
/// https://drafts.csswg.org/css-text-decor-4/
#[repr(C, u8)]
#[derive(
    Animate,
    Clone,
    Copy,
    ComputeSquaredDistance,
    Debug,
    Deserialize,
    Eq,
    MallocSizeOf,
    Parse,
    PartialEq,
    Serialize,
    SpecifiedValueInfo,
    ToAnimatedValue,
    ToAnimatedZero,
    ToComputedValue,
    ToCss,
    ToResolvedValue,
    ToShmem,
    ToTyped,
)]
#[allow(missing_docs)]
pub enum GenericTextDecorationLength<L> {
    LengthPercentage(L),
    Auto,
    FromFont,
}

/// Text decoration inset values.
///
/// https://drafts.csswg.org/css-text-decor-4/#text-decoration-skip-inset-property
#[repr(C, u8)]
#[derive(
    Clone,
    Debug,
    Eq,
    MallocSizeOf,
    PartialEq,
    SpecifiedValueInfo,
    ToComputedValue,
    ToResolvedValue,
    ToShmem,
    ToTyped,
)]
pub enum GenericTextDecorationInset<LP> {
    /// `auto` value
    Auto,
    /// Start and end length values.
    #[allow(missing_docs)]
    LengthPercentage { start: LP, end: LP },
}

impl<L: Zero> GenericTextDecorationInset<L> {
    /// Gets the initial value (zero)
    #[inline]
    pub fn get_initial_value() -> Self {
        GenericTextDecorationInset::LengthPercentage {
            start: L::zero(),
            end: L::zero(),
        }
    }
}

impl<L: ToCss + PartialEq> ToCss for GenericTextDecorationInset<L> {
    fn to_css<W>(&self, dst: &mut CssWriter<W>) -> fmt::Result
    where
        W: Write,
    {
        match self {
            GenericTextDecorationInset::Auto => dst.write_str("auto"),
            GenericTextDecorationInset::LengthPercentage { start, end } => {
                start.to_css(dst)?;
                if start != end {
                    dst.write_char(' ')?;
                    end.to_css(dst)?;
                }
                Ok(())
            },
        }
    }
}

/// Implements type for text-indent
/// which takes the grammar of [<length-percentage>] && hanging? && each-line?
///
/// https://drafts.csswg.org/css-text/#propdef-text-indent
#[repr(C)]
#[derive(
    Animate,
    Clone,
    ComputeSquaredDistance,
    Debug,
    Eq,
    MallocSizeOf,
    PartialEq,
    SpecifiedValueInfo,
    ToAnimatedValue,
    ToAnimatedZero,
    ToComputedValue,
    ToCss,
    ToResolvedValue,
    ToShmem,
    ToTyped,
)]
pub struct GenericTextIndent<LengthPercentage> {
    /// The amount of indent to be applied to the inline-start of the first line.
    pub length: LengthPercentage,
    /// Apply indent to non-first lines instead of first.
    #[animation(constant)]
    #[css(represents_keyword)]
    pub hanging: bool,
    /// Apply to each line after a hard break, not only first in block.
    #[animation(constant)]
    #[css(represents_keyword)]
    pub each_line: bool,
}

impl<LengthPercentage: Zero> GenericTextIndent<LengthPercentage> {
    /// Return the initial zero value.
    pub fn zero() -> Self {
        Self {
            length: LengthPercentage::zero(),
            hanging: false,
            each_line: false,
        }
    }
}
