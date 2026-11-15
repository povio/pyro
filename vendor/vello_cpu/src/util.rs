// Copyright 2025 the Vello Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

use crate::peniko::ImageQuality;
use vello_common::encode::EncodedImage;
use vello_common::fearless_simd::{f32x4, prelude::*, u8x32};
use vello_common::math::FloatExt;
use vello_common::util::{narrow, widen};

/// pyro: a division by 255 rounded to the nearest integer, for every product of two 8-bit values
/// the u8 pipeline composites with: as Skia's 8-bit pipeline divides on NEON (`div255` in
/// `SkRasterPipeline_opts.h`, `vrshrq_n_u16(vrsraq_n_u16(v, v, 8), 8)`, the same value) and as a
/// GPU's float blend rounds (Chromium on macOS). Skia's lowp `div255` on other CPUs is upstream's
/// `(v + 255) / 256`.
///
/// vello_common's `Div255Ext` (`(v + 255) >> 8`, `ceil(v / 256)`) is a level off for about 29% of
/// all products, nearly always high, so a translucent colour over another (`rgba(.., .5)` over
/// `rgba(.., .5)`, an opacity group over a background, a backdrop under a translucent box) came
/// out a level off the colour Chromium paints, and off the references that spell that colour out
/// as one opaque colour (WPT box-shadow-multiple-001, t32-opacity-zorder-c,
/// backdrop-filter-isolation-*). Both are exact for multiples of 255 (`div_255(v * 255) == v`), so
/// fully covered opaque pixels composite as before; about a fifth of the antialiased edge pixels
/// of opaque content over another colour move by a level.
///
/// Valid for `v <= 255 * 255 + 127`; every product of two `u8` is.
pub(crate) trait Div255Ext: private::Sealed {
    /// Divide by 255, rounding to the nearest integer (a quotient by 255 is never a half).
    fn div_255(self) -> Self;
}

mod private {
    use vello_common::fearless_simd::{Simd, u16x16, u16x32};

    pub(crate) trait Sealed {}

    impl<S: Simd> Sealed for u16x16<S> {}
    impl<S: Simd> Sealed for u16x32<S> {}
}

impl<T> Div255Ext for T
where
    T: private::Sealed
        + Copy
        + core::ops::Add<u16, Output = T>
        + core::ops::Add<T, Output = T>
        + core::ops::Shr<u32, Output = T>,
{
    #[inline(always)]
    fn div_255(self) -> Self {
        let t = self + 128_u16;
        (t + (t >> 8_u32)) >> 8_u32
    }
}

/// pyro: vello_common's `normalized_mul_u8` with the rounding division ([`Div255Ext`]).
#[inline(always)]
pub(crate) fn normalized_mul_u8<S, V>(a: V, b: V) -> <V::Widened as SimdCombine<S>>::Combined
where
    S: Simd,
    V: SimdWiden<S>,
    V::Widened: SimdCombine<S>,
    <V::Widened as SimdCombine<S>>::Combined:
        Div255Ext + core::ops::Mul<Output = <V::Widened as SimdCombine<S>>::Combined>,
{
    let a = widen(a);
    let b = widen(b);
    (a * b).div_255()
}

pub(crate) mod scalar {
    /// pyro: a division by 255 rounded to the nearest integer, as Skia's 8-bit pipeline divides on
    /// NEON (see [`Div255Ext`](super::Div255Ext)). Upstream divided by 256 rounding up
    /// (`(val + 255) >> 8`).
    ///
    /// Valid for `val <= 255 * 255 + 127`.
    #[inline(always)]
    pub(crate) const fn div_255(val: u16) -> u16 {
        debug_assert!(
            val <= 255 * 255 + 127,
            "the rounding division is exact up to 255 * 255 + 127"
        );
        let t = val + 128;
        (t + (t >> 8)) >> 8
    }

    #[cfg(test)]
    mod tests {
        use crate::util::scalar::div_255;

        #[test]
        fn div_255_rounds_to_nearest() {
            for i in 0_u16..=255 * 255 {
                let expected = (u32::from(i) * 2 + 255) / 510;
                assert_eq!(u32::from(div_255(i)), expected, "{i} / 255");
            }
        }
    }
}

pub(crate) trait NormalizedMulExt {
    fn normalized_mul(self, other: Self) -> Self;
}

impl<S: Simd> NormalizedMulExt for u8x32<S> {
    #[inline(always)]
    fn normalized_mul(self, other: Self) -> Self {
        narrow(normalized_mul_u8(self, other))
    }
}

pub(crate) trait EncodedImageExt {
    fn has_skew(&self) -> bool;
    fn nearest_neighbor(&self) -> bool;
}

impl EncodedImageExt for EncodedImage {
    fn has_skew(&self) -> bool {
        !(self.x_advance.y as f32).is_nearly_zero() || !(self.y_advance.x as f32).is_nearly_zero()
    }

    fn nearest_neighbor(&self) -> bool {
        self.sampler.quality == ImageQuality::Low
    }
}

pub(crate) trait Premultiply {
    fn premultiply(self, alphas: Self) -> Self;
    fn unpremultiply(self, alphas: Self) -> Self;
}

impl<S: Simd> Premultiply for f32x4<S> {
    #[inline(always)]
    fn premultiply(self, alphas: Self) -> Self {
        self * alphas
    }

    #[inline(always)]
    fn unpremultiply(self, alphas: Self) -> Self {
        let zero = Self::splat(alphas.simd, 0.0);
        let divided = self / alphas;

        self.simd
            .select_f32x4(self.simd.simd_eq_f32x4(alphas, zero), zero, divided)
    }
}
