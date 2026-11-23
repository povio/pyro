/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Computed color values.

use crate::color::AbsoluteColor;
use crate::derives::*;
use crate::values::specified::color::DynamicRangeLevel;
use crate::typed_om::{KeywordValue, ToTyped, TypedValue};
use crate::values::animated::ToAnimatedZero;
use crate::values::computed::percentage::Percentage;
use crate::values::generics::color::{
    GenericCaretColor, GenericColor, GenericColorMix, GenericColorOrAuto,
};
use std::fmt::{self, Write};
use style_traits::{CssString, CssWriter, ToCss};
use thin_vec::ThinVec;

pub use crate::values::specified::color::{ColorScheme, ForcedColorAdjust, PrintColorAdjust};

/// The computed value of the `color` property.
pub type ColorPropertyValue = AbsoluteColor;

/// A computed value for `<color>`.
pub type Color = GenericColor<Percentage>;

/// A computed color-mix().
pub type ColorMix = GenericColorMix<Color, Percentage>;

impl ToCss for Color {
    fn to_css<W>(&self, dest: &mut CssWriter<W>) -> fmt::Result
    where
        W: fmt::Write,
    {
        match *self {
            Self::Absolute(ref c) => c.to_css(dest),
            Self::ColorFunction(ref color_function) => color_function.to_css(dest),
            Self::CurrentColor => dest.write_str("currentcolor"),
            Self::ColorMix(ref m) => m.to_css(dest),
            Self::ContrastColor(ref c) => {
                dest.write_str("contrast-color(")?;
                c.to_css(dest)?;
                dest.write_char(')')
            },
        }
    }
}

impl ToTyped for Color {
    fn to_typed(&self, dest: &mut ThinVec<TypedValue>) -> Result<(), ()> {
        match *self {
            Self::CurrentColor => {
                dest.push(TypedValue::Keyword(KeywordValue(CssString::from(
                    "currentcolor",
                ))));
                Ok(())
            },
            _ => Err(()),
        }
    }
}

impl Color {
    /// A fully transparent color.
    pub const TRANSPARENT_BLACK: Self = Self::Absolute(AbsoluteColor::TRANSPARENT_BLACK);

    /// An opaque black color.
    pub const BLACK: Self = Self::Absolute(AbsoluteColor::BLACK);

    /// An opaque white color.
    pub const WHITE: Self = Self::Absolute(AbsoluteColor::WHITE);

    /// Create a new computed [`Color`] from a given color-mix, simplifying it to an absolute color
    /// if possible.
    pub fn from_color_mix(color_mix: ColorMix) -> Self {
        if let Some(absolute) = color_mix.mix_to_absolute() {
            Self::Absolute(absolute)
        } else {
            Self::ColorMix(Box::new(color_mix))
        }
    }

    /// Combine this complex color with the given foreground color into an absolute color.
    pub fn resolve_to_absolute(&self, current_color: &AbsoluteColor) -> AbsoluteColor {
        match *self {
            Self::Absolute(c) => c,
            Self::ColorFunction(ref color_function) => {
                color_function.resolve_to_absolute(current_color)
            },
            Self::CurrentColor => *current_color,
            Self::ColorMix(ref mix) => {
                use crate::color::mix;

                let fill = mix.omitted_weight().unwrap_or(0.0);

                mix::mix_many(
                    mix.interpolation,
                    mix.items.iter().map(|item| {
                        mix::ColorMixItem::new(
                            item.color.resolve_to_absolute(current_color),
                            item.percentage.as_ref().map_or(fill, |p| p.0),
                        )
                    }),
                    mix.flags,
                )
            },
            Self::ContrastColor(ref c) => {
                let bg_color = c.resolve_to_absolute(current_color);
                Self::resolve_contrast_color(&bg_color)
            },
        }
    }

    /// Performs the resolution of contrast-color given a background color.
    pub fn resolve_contrast_color(bg_color: &AbsoluteColor) -> AbsoluteColor {
        if Self::contrast_ratio(bg_color, &AbsoluteColor::BLACK)
            > Self::contrast_ratio(bg_color, &AbsoluteColor::WHITE)
        {
            AbsoluteColor::BLACK
        } else {
            AbsoluteColor::WHITE
        }
    }

    fn contrast_ratio(a: &AbsoluteColor, b: &AbsoluteColor) -> f32 {
        // TODO: This just implements the WCAG 2.1 algorithm,
        // https://www.w3.org/TR/WCAG21/#dfn-contrast-ratio
        // Consider using a more sophisticated contrast algorithm, e.g. see
        // https://apcacontrast.com
        let compute = |c| -> f32 {
            if c <= 0.04045 {
                c / 12.92
            } else {
                f32::powf((c + 0.055) / 1.055, 2.4)
            }
        };
        let luminance = |r, g, b| -> f32 { 0.2126 * r + 0.7152 * g + 0.0722 * b };
        let a = a.into_srgb_legacy();
        let b = b.into_srgb_legacy();
        let a = a.raw_components();
        let b = b.raw_components();
        let la = luminance(compute(a[0]), compute(a[1]), compute(a[2])) + 0.05;
        let lb = luminance(compute(b[0]), compute(b[1]), compute(b[2])) + 0.05;
        if la > lb { la / lb } else { lb / la }
    }
}

impl ToAnimatedZero for AbsoluteColor {
    fn to_animated_zero(&self) -> Result<Self, ()> {
        Ok(Self::TRANSPARENT_BLACK)
    }
}

/// auto | <color>
pub type ColorOrAuto = GenericColorOrAuto<Color>;

/// caret-color
pub type CaretColor = GenericCaretColor<Color>;

/// pyro: outline-color.
pub type OutlineColor = crate::values::generics::color::GenericOutlineColor<Color>;

/// A normalized mixture of the standard, constrained and unrestricted HDR limits.
/// The exposed computed value contains contributions, independent of display luminance.
#[derive(Clone, Copy, ComputeSquaredDistance, Debug, Deserialize, MallocSizeOf, PartialEq, Serialize, ToAnimatedValue, ToResolvedValue, ToShmem)]
pub struct DynamicRangeLimit {
    standard: f32,
    constrained: f32,
    unrestricted: f32,
}

impl DynamicRangeLimit {
    /// Initial value of the dynamic-range-limit property.
    pub fn no_limit() -> Self {
        Self { standard: 0.0, constrained: 0.0, unrestricted: 1.0 }
    }

    pub(crate) fn from_level(level: DynamicRangeLevel) -> Self {
        let mut weights = [0.0; 3];
        weights[level as usize] = 1.0;
        Self::from_weights(weights)
    }

    fn from_weights([standard, constrained, unrestricted]: [f32; 3]) -> Self {
        Self { standard, constrained, unrestricted }
    }

    pub(crate) fn normalized(weights: [f64; 3]) -> Self {
        let total: f64 = weights.iter().sum();
        if total <= 0.0 || !total.is_finite() {
            return Self::no_limit();
        }
        Self::from_weights(weights.map(|w| (w / total) as f32))
    }

    pub(crate) fn weights(&self) -> [f32; 3] {
        [self.standard, self.constrained, self.unrestricted]
    }

    pub(crate) fn single_level(&self) -> Option<DynamicRangeLevel> {
        let mut levels = DynamicRangeLevel::ORDER.into_iter().zip(self.weights()).filter(|(_, w)| *w > 0.0).map(|(level, _)| level);
        let first = levels.next()?;
        levels.next().is_none().then_some(first)
    }
}

impl ToCss for DynamicRangeLimit {
    fn to_css<W: Write>(&self, dest: &mut CssWriter<W>) -> fmt::Result {
        if let Some(level) = self.single_level() {
            return level.to_css(dest);
        }

        dest.write_str("dynamic-range-limit-mix(")?;
        let items = DynamicRangeLevel::ORDER.into_iter().zip(self.weights()).filter(|(_, weight)| *weight > 0.0);
        for (i, (level, weight)) in items.enumerate() {
            if i != 0 {
                dest.write_str(", ")?;
            }
            level.to_css(dest)?;
            dest.write_char(' ')?;
            crate::values::serialize_percentage(weight, dest)?;
        }
        dest.write_char(')')
    }
}

impl ToTyped for DynamicRangeLimit {
    fn to_typed(&self, dest: &mut ThinVec<TypedValue>) -> Result<(), ()> {
        self.single_level().ok_or(())?.to_typed(dest)
    }
}

impl crate::values::animated::Animate for DynamicRangeLimit {
    fn animate(&self, other: &Self, procedure: crate::values::animated::Procedure) -> Result<Self, ()> {
        let crate::values::animated::Procedure::Interpolate { progress } = procedure else { return Err(()) };
        if progress <= 0.0 { return Ok(*self); }
        if progress >= 1.0 { return Ok(*other); }
        let left = self.weights();
        let right = other.weights();
        Ok(Self::normalized(std::array::from_fn(|i| {
            (1.0 - progress) * f64::from(left[i]) + progress * f64::from(right[i])
        })))
    }
}

impl ToAnimatedZero for DynamicRangeLimit {
    fn to_animated_zero(&self) -> Result<Self, ()> { Err(()) }
}
