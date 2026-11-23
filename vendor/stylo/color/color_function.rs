/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Output of parsing a color function, e.g. rgb(..), hsl(..), color(..)

use std::fmt::Write;

use super::{
    AbsoluteColor, ColorFlags, ColorSpace,
    component::ColorComponent,
    convert::normalize_hue,
    parsing::{NumberOrAngleComponent, NumberOrPercentageComponent},
};
use crate::derives::*;
use crate::values::{
    computed, computed::color::Color as ComputedColor, generics::Optional, normalize,
    specified::color::Color as SpecifiedColor,
};
use cssparser::color::{OPAQUE, clamp_floor_256_f32};

/// Represents a specified color function.
#[derive(Clone, Debug, MallocSizeOf, PartialEq, ToAnimatedValue, ToShmem)]
#[repr(u8)]
pub enum ColorFunction<OriginColor> {
    /// <https://drafts.csswg.org/css-color-4/#rgb-functions>
    Rgb(
        Optional<OriginColor>,                       // origin
        ColorComponent<NumberOrPercentageComponent>, // red
        ColorComponent<NumberOrPercentageComponent>, // green
        ColorComponent<NumberOrPercentageComponent>, // blue
        ColorComponent<NumberOrPercentageComponent>, // alpha
    ),
    /// <https://drafts.csswg.org/css-color-4/#the-hsl-notation>
    Hsl(
        Optional<OriginColor>,                       // origin
        ColorComponent<NumberOrAngleComponent>,      // hue
        ColorComponent<NumberOrPercentageComponent>, // saturation
        ColorComponent<NumberOrPercentageComponent>, // lightness
        ColorComponent<NumberOrPercentageComponent>, // alpha
    ),
    /// <https://drafts.csswg.org/css-color-4/#the-hwb-notation>
    Hwb(
        Optional<OriginColor>,                       // origin
        ColorComponent<NumberOrAngleComponent>,      // hue
        ColorComponent<NumberOrPercentageComponent>, // whiteness
        ColorComponent<NumberOrPercentageComponent>, // blackness
        ColorComponent<NumberOrPercentageComponent>, // alpha
    ),
    /// <https://drafts.csswg.org/css-color-4/#specifying-lab-lch>
    Lab(
        Optional<OriginColor>,                       // origin
        ColorComponent<NumberOrPercentageComponent>, // lightness
        ColorComponent<NumberOrPercentageComponent>, // a
        ColorComponent<NumberOrPercentageComponent>, // b
        ColorComponent<NumberOrPercentageComponent>, // alpha
    ),
    /// <https://drafts.csswg.org/css-color-4/#specifying-lab-lch>
    Lch(
        Optional<OriginColor>,                       // origin
        ColorComponent<NumberOrPercentageComponent>, // lightness
        ColorComponent<NumberOrPercentageComponent>, // chroma
        ColorComponent<NumberOrAngleComponent>,      // hue
        ColorComponent<NumberOrPercentageComponent>, // alpha
    ),
    /// <https://drafts.csswg.org/css-color-4/#specifying-oklab-oklch>
    Oklab(
        Optional<OriginColor>,                       // origin
        ColorComponent<NumberOrPercentageComponent>, // lightness
        ColorComponent<NumberOrPercentageComponent>, // a
        ColorComponent<NumberOrPercentageComponent>, // b
        ColorComponent<NumberOrPercentageComponent>, // alpha
    ),
    /// <https://drafts.csswg.org/css-color-4/#specifying-oklab-oklch>
    Oklch(
        Optional<OriginColor>,                       // origin
        ColorComponent<NumberOrPercentageComponent>, // lightness
        ColorComponent<NumberOrPercentageComponent>, // chroma
        ColorComponent<NumberOrAngleComponent>,      // hue
        ColorComponent<NumberOrPercentageComponent>, // alpha
    ),
    /// <https://drafts.csswg.org/css-color-4/#color-function>
    Color(
        Optional<OriginColor>,                       // origin
        ColorComponent<NumberOrPercentageComponent>, // red / x
        ColorComponent<NumberOrPercentageComponent>, // green / y
        ColorComponent<NumberOrPercentageComponent>, // blue / z
        ColorComponent<NumberOrPercentageComponent>, // alpha
        ColorSpace,
    ),
    /// <https://drafts.csswg.org/css-color-5/#relative-alpha>
    Alpha(
        OriginColor,                                 // origin
        ColorComponent<NumberOrPercentageComponent>, // alpha
    ),
}

impl ColorFunction<SpecifiedColor> {
    /// Return true if the color function has an origin color specified.
    pub fn has_origin_color(&self) -> bool {
        self.origin_color().is_some()
    }

    /// pyro: whether a component of the function is a `calc()`.
    pub fn has_calc_component(&self) -> bool {
        fn calc<T>(component: &ColorComponent<T>) -> bool {
            matches!(component, ColorComponent::Calc(..))
        }
        match self {
            Self::Rgb(_, c0, c1, c2, alpha) => calc(c0) || calc(c1) || calc(c2) || calc(alpha),
            Self::Hsl(_, c0, c1, c2, alpha) => calc(c0) || calc(c1) || calc(c2) || calc(alpha),
            Self::Hwb(_, c0, c1, c2, alpha) => calc(c0) || calc(c1) || calc(c2) || calc(alpha),
            Self::Lab(_, c0, c1, c2, alpha) => calc(c0) || calc(c1) || calc(c2) || calc(alpha),
            Self::Lch(_, c0, c1, c2, alpha) => calc(c0) || calc(c1) || calc(c2) || calc(alpha),
            Self::Oklab(_, c0, c1, c2, alpha) => calc(c0) || calc(c1) || calc(c2) || calc(alpha),
            Self::Oklch(_, c0, c1, c2, alpha) => calc(c0) || calc(c1) || calc(c2) || calc(alpha),
            Self::Color(_, c0, c1, c2, alpha, _) => calc(c0) || calc(c1) || calc(c2) || calc(alpha),
            Self::Alpha(_, alpha) => calc(alpha),
        }
    }

    /// pyro: the specified value of a function without an origin color that keeps its function
    /// form (one with a `calc()` component: css-color-4 § 15, as Chromium serializes it; WPT
    /// css-color `parsing/color-valid-*`): each component that is no `calc()` becomes the number
    /// the function resolves it to (a percentage against its reference range, an angle in
    /// degrees, clamped as the function clamps it), so `lab(200 calc(50%) 50%)` is
    /// `lab(100 calc(50%) 62.5)`. The `calc()` components stay as they are.
    pub fn normalize_plain_components(&mut self) {
        fn number(
            component: &mut ColorComponent<NumberOrPercentageComponent>,
            basis: f32,
            clamp: Option<(f32, f32)>,
        ) {
            if let ColorComponent::Value(value) = component {
                let mut n = value.to_number(basis);
                if let Some((lo, hi)) = clamp {
                    n = n.clamp(lo, hi);
                }
                *component = ColorComponent::Value(NumberOrPercentageComponent::Number(n));
            }
        }
        fn hue(component: &mut ColorComponent<NumberOrAngleComponent>) {
            if let ColorComponent::Value(value) = component {
                *component = ColorComponent::Value(NumberOrAngleComponent::Number(normalize_hue(value.degrees())));
            }
        }
        let unbounded = Some((0.0, f32::INFINITY));
        match self {
            Self::Rgb(_, r, g, b, alpha) => {
                for c in [r, g, b] {
                    number(c, 255.0, Some((0.0, 255.0)));
                }
                number(alpha, 1.0, Some((0.0, 1.0)));
            },
            Self::Hsl(_, h, s, l, alpha) => {
                hue(h);
                // (As Chromium: a negative saturation is 0; nothing else is clamped here.)
                number(s, 100.0, unbounded);
                number(l, 100.0, None);
                number(alpha, 1.0, Some((0.0, 1.0)));
            },
            Self::Hwb(_, h, w, b, alpha) => {
                hue(h);
                number(w, 100.0, None);
                number(b, 100.0, None);
                number(alpha, 1.0, Some((0.0, 1.0)));
            },
            Self::Lab(_, l, a, b, alpha) => {
                number(l, 100.0, Some((0.0, 100.0)));
                number(a, 125.0, None);
                number(b, 125.0, None);
                number(alpha, 1.0, Some((0.0, 1.0)));
            },
            Self::Lch(_, l, c, h, alpha) => {
                number(l, 100.0, Some((0.0, 100.0)));
                number(c, 150.0, unbounded);
                hue(h);
                number(alpha, 1.0, Some((0.0, 1.0)));
            },
            Self::Oklab(_, l, a, b, alpha) => {
                number(l, 1.0, Some((0.0, 1.0)));
                number(a, 0.4, None);
                number(b, 0.4, None);
                number(alpha, 1.0, Some((0.0, 1.0)));
            },
            Self::Oklch(_, l, c, h, alpha) => {
                number(l, 1.0, Some((0.0, 1.0)));
                number(c, 0.4, unbounded);
                hue(h);
                number(alpha, 1.0, Some((0.0, 1.0)));
            },
            Self::Color(_, c0, c1, c2, alpha, _) => {
                for c in [c0, c1, c2] {
                    number(c, 1.0, None);
                }
                number(alpha, 1.0, Some((0.0, 1.0)));
            },
            Self::Alpha(..) => {},
        }
    }

    /// Try to compute the color function to a computed color.
    ///
    /// If the origin color (if any) is absolute, the function is resolved to an
    /// absolute color in a single pass over the components; otherwise it is
    /// computed and preserved so it can be resolved at use-value time.
    pub fn to_computed_color(
        &self,
        context: Option<&computed::Context>,
    ) -> Result<ComputedColor, ()> {
        // Compute the origin color (if any) once.
        let origin = match self.origin_color() {
            Some(o) => Some(o.to_computed_color(context)?),
            None => None,
        };
        // We can only resolve to an absolute color if there is no origin color or
        // it is already absolute.
        let resolvable = origin.as_ref().is_none_or(|o| o.is_absolute());

        let computed = self.to_computed_value(context, origin)?;
        if resolvable && let Ok(absolute) = computed.to_absolute_color() {
            return Ok(ComputedColor::Absolute(absolute));
        }

        Ok(ComputedColor::ColorFunction(Box::new(computed)))
    }
}

impl<Color> ColorFunction<Color> {
    /// Map the origin color to another type.
    pub fn map_origin_color<U>(
        &self,
        f: impl FnOnce(&Color) -> Result<U, ()>,
    ) -> Result<ColorFunction<U>, ()> {
        macro_rules! map {
            ($f:ident, $o:expr, $c0:expr, $c1:expr, $c2:expr, $alpha:expr) => {{
                ColorFunction::$f(
                    match $o.as_ref() {
                        Some(c) => Some(f(c)?),
                        None => None,
                    }
                    .into(),
                    $c0.clone(),
                    $c1.clone(),
                    $c2.clone(),
                    $alpha.clone(),
                )
            }};
        }
        Ok(match self {
            ColorFunction::Rgb(o, c0, c1, c2, alpha) => map!(Rgb, o, c0, c1, c2, alpha),
            ColorFunction::Hsl(o, c0, c1, c2, alpha) => map!(Hsl, o, c0, c1, c2, alpha),
            ColorFunction::Hwb(o, c0, c1, c2, alpha) => map!(Hwb, o, c0, c1, c2, alpha),
            ColorFunction::Lab(o, c0, c1, c2, alpha) => map!(Lab, o, c0, c1, c2, alpha),
            ColorFunction::Lch(o, c0, c1, c2, alpha) => map!(Lch, o, c0, c1, c2, alpha),
            ColorFunction::Oklab(o, c0, c1, c2, alpha) => map!(Oklab, o, c0, c1, c2, alpha),
            ColorFunction::Oklch(o, c0, c1, c2, alpha) => map!(Oklch, o, c0, c1, c2, alpha),
            ColorFunction::Color(o, c0, c1, c2, alpha, color_space) => ColorFunction::Color(
                match o.as_ref() {
                    Some(c) => Some(f(c)?),
                    None => None,
                }
                .into(),
                c0.clone(),
                c1.clone(),
                c2.clone(),
                alpha.clone(),
                *color_space,
            ),
            ColorFunction::Alpha(o, alpha) => ColorFunction::Alpha(f(o)?, alpha.clone()),
        })
    }

    /// Returns the origin color of this color function, if it has one.
    pub fn origin_color(&self) -> Option<&Color> {
        match self {
            Self::Rgb(o, ..)
            | Self::Hsl(o, ..)
            | Self::Hwb(o, ..)
            | Self::Lab(o, ..)
            | Self::Lch(o, ..)
            | Self::Oklab(o, ..)
            | Self::Oklch(o, ..)
            | Self::Color(o, ..) => o.as_ref(),
            Self::Alpha(o, ..) => Some(o),
        }
    }

    /// `origin` is the (already computed) origin color, if any.
    fn to_computed_value(
        &self,
        context: Option<&computed::Context>,
        origin: Option<ComputedColor>,
    ) -> Result<ColorFunction<ComputedColor>, ()> {
        // The absolute origin color, if available, used to substitute channels.
        let abs_origin = origin.as_ref().and_then(|o| o.as_absolute());
        // Builds a variant where the origin is converted to `$space` (the
        // function's color space) before its channels are read.
        macro_rules! convert {
            ($variant:ident, $space:expr, $c0:expr, $c1:expr, $c2:expr, $alpha:expr, legacy_hue) => {{
                // pyro: the hue a legacy sRGB origin (`rgb()`, a hex color or a keyword) has
                // when it is achromatic reads as 0 rather than missing. pyro's CSSOM keeps inline
                // styles as text, so an `hsl()`/`hwb()` origin comes back as `rgb()` and its
                // authored hue would otherwise be lost to the conversion.
                let converted = abs_origin.map(|o| {
                    let mut c = o.convert_with_missing($space);
                    if o.color_space == ColorSpace::Srgb
                        && o.flags.contains(ColorFlags::IS_LEGACY_SRGB)
                        && !o.flags.intersects(ColorFlags::NONE_FLAGS)
                        && c.c0().is_none()
                    {
                        c.components.0 = 0.0;
                        c.flags.remove(ColorFlags::C0_IS_NONE);
                    }
                    c
                });
                ColorFunction::$variant(
                    Optional::from(origin),
                    $c0.to_computed_value(context, converted.as_ref())?,
                    $c1.to_computed_value(context, converted.as_ref())?,
                    $c2.to_computed_value(context, converted.as_ref())?,
                    $alpha.to_computed_value(context, converted.as_ref())?,
                )
            }};
            ($variant:ident, $space:expr, $c0:expr, $c1:expr, $c2:expr, $alpha:expr) => {{
                let converted = abs_origin.map(|o| o.convert_with_missing($space));
                ColorFunction::$variant(
                    Optional::from(origin),
                    $c0.to_computed_value(context, converted.as_ref())?,
                    $c1.to_computed_value(context, converted.as_ref())?,
                    $c2.to_computed_value(context, converted.as_ref())?,
                    $alpha.to_computed_value(context, converted.as_ref())?,
                )
            }};
        }

        Ok(match self {
            ColorFunction::Rgb(_, r, g, b, alpha) => {
                // rgb(..) channels are in the [0..255] range, so map the origin's
                // components accordingly.
                let converted = abs_origin.map(|o| {
                    let o = o.convert_with_missing(ColorSpace::Srgb);
                    AbsoluteColor::new(
                        ColorSpace::Srgb,
                        o.c0().map(|v| v * 255.0),
                        o.c1().map(|v| v * 255.0),
                        o.c2().map(|v| v * 255.0),
                        o.alpha(),
                    )
                });
                ColorFunction::Rgb(
                    Optional::from(origin),
                    r.to_computed_value(context, converted.as_ref())?,
                    g.to_computed_value(context, converted.as_ref())?,
                    b.to_computed_value(context, converted.as_ref())?,
                    alpha.to_computed_value(context, converted.as_ref())?,
                )
            },
            ColorFunction::Hsl(_, c0, c1, c2, alpha) => {
                convert!(Hsl, ColorSpace::Hsl, c0, c1, c2, alpha, legacy_hue)
            },
            ColorFunction::Hwb(_, c0, c1, c2, alpha) => {
                convert!(Hwb, ColorSpace::Hwb, c0, c1, c2, alpha, legacy_hue)
            },
            ColorFunction::Lab(_, c0, c1, c2, alpha) => {
                convert!(Lab, ColorSpace::Lab, c0, c1, c2, alpha)
            },
            ColorFunction::Lch(_, c0, c1, c2, alpha) => {
                convert!(Lch, ColorSpace::Lch, c0, c1, c2, alpha)
            },
            ColorFunction::Oklab(_, c0, c1, c2, alpha) => {
                convert!(Oklab, ColorSpace::Oklab, c0, c1, c2, alpha)
            },
            ColorFunction::Oklch(_, c0, c1, c2, alpha) => {
                convert!(Oklch, ColorSpace::Oklch, c0, c1, c2, alpha)
            },
            ColorFunction::Color(_, c0, c1, c2, alpha, color_space) => {
                let converted = abs_origin.map(|o| {
                    let mut result = o.convert_with_missing(*color_space);
                    // Drop the legacy flag so the origin is read as a `color()`.
                    result.flags.set(ColorFlags::IS_LEGACY_SRGB, false);
                    result
                });
                ColorFunction::Color(
                    Optional::from(origin),
                    c0.to_computed_value(context, converted.as_ref())?,
                    c1.to_computed_value(context, converted.as_ref())?,
                    c2.to_computed_value(context, converted.as_ref())?,
                    alpha.to_computed_value(context, converted.as_ref())?,
                    *color_space,
                )
            },
            ColorFunction::Alpha(_, alpha) => {
                let alpha = alpha.to_computed_value(context, abs_origin)?;
                let stored = origin.expect("alpha() is always relative");
                ColorFunction::Alpha(stored, alpha)
            },
        })
    }
}

impl ColorFunction<ComputedColor> {
    fn to_absolute_color(&self) -> Result<AbsoluteColor, ()> {
        macro_rules! alpha {
            ($alpha:expr) => {{
                $alpha
                    .resolve()?
                    .map(|value| normalize(value.to_number(1.0)).clamp(0.0, OPAQUE))
            }};
        }

        Ok(match self {
            ColorFunction::Rgb(origin_color, r, g, b, alpha) => {
                // Use `color(srgb ...)` to serialize `rgb(...)` if an origin color is available;
                // this is the only reason for now.
                let use_color_syntax = origin_color.is_some();

                if use_color_syntax {
                    // The components have already been mapped into the [0..255]
                    // range against the origin, so map them back to [0..1).
                    AbsoluteColor::new(
                        ColorSpace::Srgb,
                        r.resolve()?.map(|c| c.to_number(255.0) / 255.0),
                        g.resolve()?.map(|c| c.to_number(255.0) / 255.0),
                        b.resolve()?.map(|c| c.to_number(255.0) / 255.0),
                        alpha!(alpha),
                    )
                } else {
                    // Resolve a component to its legacy sRGB value in [0..1], preserving
                    // `none` as `None` so the `*_IS_NONE` flags are kept on the color. Legacy
                    // syntax cannot serialize `none`, but the flags are still needed to carry
                    // missing components forward through interpolation (e.g. `color-mix`).
                    #[inline]
                    fn resolve(
                        component: &ColorComponent<NumberOrPercentageComponent>,
                    ) -> Result<Option<f32>, ()> {
                        Ok(component.resolve()?.map(|value| {
                            clamp_floor_256_f32(value.to_number(u8::MAX as f32)) as f32 / 255.0
                        }))
                    }

                    let mut result = AbsoluteColor::new(
                        ColorSpace::Srgb,
                        resolve(r)?,
                        resolve(g)?,
                        resolve(b)?,
                        alpha!(alpha),
                    );
                    result.flags.insert(ColorFlags::IS_LEGACY_SRGB);
                    result
                }
            },
            ColorFunction::Hsl(origin_color, h, s, l, alpha) => {
                // Percent reference range for S and L: 0% = 0.0, 100% = 100.0
                const LIGHTNESS_RANGE: f32 = 100.0;
                const SATURATION_RANGE: f32 = 100.0;

                // If the origin color was *NOT* specified, then we stick with the
                // old way of serializing the value to rgb(..). Otherwise we don't
                // use the rgb(..) syntax, because we should allow the color to be
                // out of gamut and not clamp.
                let use_rgb_sytax = origin_color.is_none();

                let mut result = AbsoluteColor::new(
                    ColorSpace::Hsl,
                    h.resolve()?.map(|angle| normalize_hue(angle.degrees())),
                    s.resolve()?.map(|s| {
                        if use_rgb_sytax {
                            s.to_number(SATURATION_RANGE).max(0.0)
                        } else {
                            s.to_number(SATURATION_RANGE)
                        }
                    }),
                    l.resolve()?.map(|l| l.to_number(LIGHTNESS_RANGE)),
                    alpha!(alpha),
                );

                if use_rgb_sytax {
                    result.flags.insert(ColorFlags::IS_LEGACY_SRGB);
                }

                result
            },
            ColorFunction::Hwb(origin_color, h, w, b, alpha) => {
                let use_rgb_sytax = origin_color.is_none();

                // Percent reference range for W and B: 0% = 0.0, 100% = 100.0
                const WHITENESS_RANGE: f32 = 100.0;
                const BLACKNESS_RANGE: f32 = 100.0;

                let mut result = AbsoluteColor::new(
                    ColorSpace::Hwb,
                    h.resolve()?.map(|angle| normalize_hue(angle.degrees())),
                    w.resolve()?.map(|w| w.to_number(WHITENESS_RANGE)),
                    b.resolve()?.map(|b| b.to_number(BLACKNESS_RANGE)),
                    alpha!(alpha),
                );

                if use_rgb_sytax {
                    result.flags.insert(ColorFlags::IS_LEGACY_SRGB);
                }

                result
            },
            ColorFunction::Lab(_, l, a, b, alpha) => {
                // for L: 0% = 0.0, 100% = 100.0
                // for a and b: -100% = -125, 100% = 125
                const LIGHTNESS_RANGE: f32 = 100.0;
                const A_B_RANGE: f32 = 125.0;

                AbsoluteColor::new(
                    ColorSpace::Lab,
                    l.resolve()?.map(|l| l.to_number(LIGHTNESS_RANGE)),
                    a.resolve()?.map(|a| a.to_number(A_B_RANGE)),
                    b.resolve()?.map(|b| b.to_number(A_B_RANGE)),
                    alpha!(alpha),
                )
            },
            ColorFunction::Lch(_, l, c, h, alpha) => {
                // for L: 0% = 0.0, 100% = 100.0
                // for C: 0% = 0, 100% = 150
                const LIGHTNESS_RANGE: f32 = 100.0;
                const CHROMA_RANGE: f32 = 150.0;

                AbsoluteColor::new(
                    ColorSpace::Lch,
                    l.resolve()?.map(|l| l.to_number(LIGHTNESS_RANGE)),
                    c.resolve()?.map(|c| c.to_number(CHROMA_RANGE)),
                    h.resolve()?.map(|angle| normalize_hue(angle.degrees())),
                    alpha!(alpha),
                )
            },
            ColorFunction::Oklab(_, l, a, b, alpha) => {
                // for L: 0% = 0.0, 100% = 1.0
                // for a and b: -100% = -0.4, 100% = 0.4
                const LIGHTNESS_RANGE: f32 = 1.0;
                const A_B_RANGE: f32 = 0.4;

                AbsoluteColor::new(
                    ColorSpace::Oklab,
                    l.resolve()?.map(|l| l.to_number(LIGHTNESS_RANGE)),
                    a.resolve()?.map(|a| a.to_number(A_B_RANGE)),
                    b.resolve()?.map(|b| b.to_number(A_B_RANGE)),
                    alpha!(alpha),
                )
            },
            ColorFunction::Oklch(_, l, c, h, alpha) => {
                // for L: 0% = 0.0, 100% = 1.0
                // for C: 0% = 0.0 100% = 0.4
                const LIGHTNESS_RANGE: f32 = 1.0;
                const CHROMA_RANGE: f32 = 0.4;

                AbsoluteColor::new(
                    ColorSpace::Oklch,
                    l.resolve()?.map(|l| l.to_number(LIGHTNESS_RANGE)),
                    c.resolve()?.map(|c| c.to_number(CHROMA_RANGE)),
                    h.resolve()?.map(|angle| normalize_hue(angle.degrees())),
                    alpha!(alpha),
                )
            },
            ColorFunction::Color(_, r, g, b, alpha, color_space) => AbsoluteColor::new(
                *color_space,
                r.resolve()?.map(|c| c.to_number(1.0)),
                g.resolve()?.map(|c| c.to_number(1.0)),
                b.resolve()?.map(|c| c.to_number(1.0)),
                alpha!(alpha),
            ),
            ColorFunction::Alpha(origin_color, alpha) => {
                let origin_color = origin_color.as_absolute().ok_or(())?;
                origin_color.with_alpha(alpha!(alpha))
            },
        })
    }

    /// Resolve a computed color function to an absolute computed color.
    pub fn resolve_to_absolute(&self, current_color: &AbsoluteColor) -> AbsoluteColor {
        // Resolve the origin color (e.g. currentcolor) to an absolute color, then
        // substitute and assemble.
        let origin = self
            .origin_color()
            .map(|o| ComputedColor::Absolute(o.resolve_to_absolute(current_color)));
        self.to_computed_value(None, origin)
            .and_then(|r| r.to_absolute_color())
            .unwrap_or_else(|_| {
                debug_assert!(
                    false,
                    "the color could not be resolved even with a currentcolor specified?"
                );
                AbsoluteColor::TRANSPARENT_BLACK
            })
    }
}

impl<C: style_traits::ToCss> style_traits::ToCss for ColorFunction<C> {
    fn to_css<W>(&self, dest: &mut style_traits::CssWriter<W>) -> std::fmt::Result
    where
        W: std::fmt::Write,
    {
        let (origin_color, alpha, trailing_space) = match self {
            Self::Rgb(origin_color, _, _, _, alpha) => {
                dest.write_str("rgb(")?;
                (origin_color.as_ref(), alpha, true)
            },
            Self::Hsl(origin_color, _, _, _, alpha) => {
                dest.write_str("hsl(")?;
                (origin_color.as_ref(), alpha, true)
            },
            Self::Hwb(origin_color, _, _, _, alpha) => {
                dest.write_str("hwb(")?;
                (origin_color.as_ref(), alpha, true)
            },
            Self::Lab(origin_color, _, _, _, alpha) => {
                dest.write_str("lab(")?;
                (origin_color.as_ref(), alpha, true)
            },
            Self::Lch(origin_color, _, _, _, alpha) => {
                dest.write_str("lch(")?;
                (origin_color.as_ref(), alpha, true)
            },
            Self::Oklab(origin_color, _, _, _, alpha) => {
                dest.write_str("oklab(")?;
                (origin_color.as_ref(), alpha, true)
            },
            Self::Oklch(origin_color, _, _, _, alpha) => {
                dest.write_str("oklch(")?;
                (origin_color.as_ref(), alpha, true)
            },
            Self::Color(origin_color, _, _, _, alpha, _) => {
                dest.write_str("color(")?;
                (origin_color.as_ref(), alpha, true)
            },
            Self::Alpha(origin_color, alpha) => {
                dest.write_str("alpha(")?;
                (Some(origin_color), alpha, false)
            },
        };

        if let Some(origin_color) = origin_color {
            dest.write_str("from ")?;
            origin_color.to_css(dest)?;
            if trailing_space {
                dest.write_str(" ")?;
            }
        }

        macro_rules! serialize_components {
            ($c0:expr, $c1:expr, $c2:expr) => {{
                debug_assert!(!matches!($c0, ColorComponent::AlphaOmitted));
                debug_assert!(!matches!($c1, ColorComponent::AlphaOmitted));
                debug_assert!(!matches!($c2, ColorComponent::AlphaOmitted));

                $c0.to_css(dest)?;
                dest.write_str(" ")?;
                $c1.to_css(dest)?;
                dest.write_str(" ")?;
                $c2.to_css(dest)?;
            }};
        }

        match self {
            Self::Rgb(_, c0, c1, c2, _) => {
                serialize_components!(c0, c1, c2);
            },
            Self::Hsl(_, c0, c1, c2, _) => {
                serialize_components!(c0, c1, c2);
            },
            Self::Hwb(_, c0, c1, c2, _) => {
                serialize_components!(c0, c1, c2);
            },
            Self::Lab(_, c0, c1, c2, _) => {
                serialize_components!(c0, c1, c2);
            },
            Self::Lch(_, c0, c1, c2, _) => {
                serialize_components!(c0, c1, c2);
            },
            Self::Oklab(_, c0, c1, c2, _) => {
                serialize_components!(c0, c1, c2);
            },
            Self::Oklch(_, c0, c1, c2, _) => {
                serialize_components!(c0, c1, c2);
            },
            Self::Color(_, c0, c1, c2, _, color_space) => {
                color_space.to_css(dest)?;
                dest.write_str(" ")?;
                serialize_components!(c0, c1, c2);
            },
            Self::Alpha(_, _) => {},
        }

        // We can avoid serializing the alpha if it's 1, but only if the color is not relative
        // (since otherwise it's different from the omitted alpha).
        let omit_alpha = match *alpha {
            ColorComponent::AlphaOmitted => true,
            ColorComponent::Value(ref v) => origin_color.is_none() && v.to_number(OPAQUE) == OPAQUE,
            _ => false,
        };

        if !omit_alpha {
            dest.write_str(" / ")?;
            alpha.to_css(dest)?;
        }

        dest.write_str(")")
    }
}
