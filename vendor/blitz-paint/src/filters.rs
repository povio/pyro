use blitz_dom::util::ToColorColor as _;
use style::color::AbsoluteColor;
pub(crate) use style::computed_values::filter::single_value::T as StyloFilter;

use anyrender::filters::{Filter, FilterEffect, color_transformation::ColorMatrix};

/// pyro: the filter in device pixels: lengths are CSS px, and the layers the filter is applied
/// to are drawn in device pixels (`scale` of them per CSS px) without a scale in their transform.
/// `current_color` is the element's `color`, which a `drop-shadow()` without one has.
pub(crate) fn convert_filters(
    filters: &[StyloFilter],
    scale: f64,
    current_color: &AbsoluteColor,
) -> Option<Filter> {
    if filters.is_empty() {
        return None;
    }

    Some(Filter::linear_list(filters.iter().filter_map(|filter| {
        convert_single_filter(filter, scale as f32, current_color)
    })))
}

pub(crate) fn convert_single_filter(
    filter: &StyloFilter,
    scale: f32,
    current_color: &AbsoluteColor,
) -> Option<FilterEffect> {
    Some(match filter {
        StyloFilter::Blur(radius) => FilterEffect::blur(radius.px() * scale),
        StyloFilter::Brightness(amount) => color_matrix(brightness(amount.0)),
        StyloFilter::Contrast(amount) => color_matrix(contrast(amount.0)),
        StyloFilter::Grayscale(amount) => color_matrix(grayscale(1.0 - amount.0)),
        StyloFilter::HueRotate(angle) => color_matrix(hue_rotate(angle.degrees())),
        StyloFilter::Invert(amount) => color_matrix(invert(amount.0)),
        StyloFilter::Opacity(amount) => color_matrix(opacity(amount.0)),
        StyloFilter::Saturate(amount) => color_matrix(saturate(amount.0)),
        StyloFilter::Sepia(amount) => color_matrix(sepia(1.0 - amount.0)),
        // pyro: Chromium takes the third length as the blur's standard deviation (measured: the
        // edge of `drop-shadow(0 0 20px)` falls off as a Gaussian of 20px), where `text-shadow`
        // and `box-shadow` take half of it.
        StyloFilter::DropShadow(shadow) => FilterEffect::drop_shadow(
            shadow.horizontal.px() * scale,
            shadow.vertical.px() * scale,
            shadow.blur.px() * scale,
            shadow
                .color
                .resolve_to_absolute(current_color)
                .as_color_color(),
        ),
        StyloFilter::Url(_) => return None,
    })
}

// pyro: the colour functions' matrices as Chromium builds them (`cc/paint/render_surface_filters.cc`),
// f32 for f32: the filter effects spec's `feColorMatrix` equivalents, with grayscale's and
// saturate's last column of each row what the row leaves of 1, and grayscale and sepia given
// 1 - amount. Rows R, G, B, A; the fifth column is an offset in 0..1. They apply to sRGB values,
// as Chromium applies them (`color-interpolation-filters` is for SVG filter primitives).

fn color_matrix(matrix: [f32; 20]) -> FilterEffect {
    FilterEffect::ColorMatrix(ColorMatrix(matrix))
}

fn brightness(amount: f32) -> [f32; 20] {
    let mut m = [0.0; 20];
    (m[0], m[6], m[12], m[18]) = (amount, amount, amount, 1.0);
    m
}

fn contrast(amount: f32) -> [f32; 20] {
    let mut m = brightness(amount);
    let intercept = -0.5 * amount + 0.5;
    (m[4], m[9], m[14]) = (intercept, intercept, intercept);
    m
}

/// `amount` is 1 - the function's.
fn grayscale(amount: f32) -> [f32; 20] {
    let mut m = [0.0; 20];
    m[0] = 0.2126 + 0.7874 * amount;
    m[1] = 0.7152 - 0.7152 * amount;
    m[2] = 1.0 - (m[0] + m[1]);
    m[5] = 0.2126 - 0.2126 * amount;
    m[6] = 0.7152 + 0.2848 * amount;
    m[7] = 1.0 - (m[5] + m[6]);
    m[10] = 0.2126 - 0.2126 * amount;
    m[11] = 0.7152 - 0.7152 * amount;
    m[12] = 1.0 - (m[10] + m[11]);
    m[18] = 1.0;
    m
}

/// `amount` is 1 - the function's.
fn sepia(amount: f32) -> [f32; 20] {
    let mut m = [0.0; 20];
    m[0] = 0.393 + 0.607 * amount;
    m[1] = 0.769 - 0.769 * amount;
    m[2] = 0.189 - 0.189 * amount;
    m[5] = 0.349 - 0.349 * amount;
    m[6] = 0.686 + 0.314 * amount;
    m[7] = 0.168 - 0.168 * amount;
    m[10] = 0.272 - 0.272 * amount;
    m[11] = 0.534 - 0.534 * amount;
    m[12] = 0.131 + 0.869 * amount;
    m[18] = 1.0;
    m
}

fn saturate(amount: f32) -> [f32; 20] {
    let mut m = [0.0; 20];
    m[0] = 0.213 + 0.787 * amount;
    m[1] = 0.715 - 0.715 * amount;
    m[2] = 1.0 - (m[0] + m[1]);
    m[5] = 0.213 - 0.213 * amount;
    m[6] = 0.715 + 0.285 * amount;
    m[7] = 1.0 - (m[5] + m[6]);
    m[10] = 0.213 - 0.213 * amount;
    m[11] = 0.715 - 0.715 * amount;
    m[12] = 1.0 - (m[10] + m[11]);
    m[18] = 1.0;
    m
}

fn hue_rotate(degrees: f32) -> [f32; 20] {
    let radians = degrees * std::f32::consts::PI / 180.0;
    let (sin, cos) = (radians.sin(), radians.cos());
    let mut m = [0.0; 20];
    m[0] = 0.213 + cos * 0.787 - sin * 0.213;
    m[1] = 0.715 - cos * 0.715 - sin * 0.715;
    m[2] = 0.072 - cos * 0.072 + sin * 0.928;
    m[5] = 0.213 - cos * 0.213 + sin * 0.143;
    m[6] = 0.715 + cos * 0.285 + sin * 0.140;
    m[7] = 0.072 - cos * 0.072 - sin * 0.283;
    m[10] = 0.213 - cos * 0.213 - sin * 0.787;
    m[11] = 0.715 - cos * 0.715 + sin * 0.715;
    m[12] = 0.072 + cos * 0.928 + sin * 0.072;
    m[18] = 1.0;
    m
}

fn invert(amount: f32) -> [f32; 20] {
    let mut m = brightness(1.0 - 2.0 * amount);
    (m[4], m[9], m[14]) = (amount, amount, amount);
    m
}

fn opacity(amount: f32) -> [f32; 20] {
    let mut m = brightness(1.0);
    m[18] = amount;
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A colour (bytes) through a matrix, as Chromium rounds it.
    fn through(m: [f32; 20], [r, g, b]: [u8; 3]) -> [u8; 3] {
        let c = [r, g, b].map(|v| f32::from(v) / 255.0);
        std::array::from_fn(|row| {
            let m = &m[row * 5..row * 5 + 5];
            let v = (m[0] * c[0] + m[1] * c[1] + m[2] * c[2] + m[3] + m[4]).clamp(0.0, 1.0);
            (v * 255.0 + 0.5) as u8
        })
    }

    /// The colours WPT's references give (their values are Chromium's).
    #[test]
    fn the_colour_functions_give_the_colours_wpt_expects() {
        // filter-grayscale-001: blue at grayscale(100%) is #121212.
        assert_eq!(through(grayscale(0.0), [0, 0, 255]), [0x12; 3]);
        // filter-contrast-002: red at contrast(0%) is #808080.
        assert_eq!(through(contrast(0.0), [255, 0, 0]), [0x80; 3]);
        // filter-hue_rotate-002: red at hue-rotate(120deg) is #007100.
        assert_eq!(through(hue_rotate(120.0), [255, 0, 0]), [0, 0x71, 0]);
        // filter-saturate-001: yellow at saturate(0%) is #EDEDED.
        assert_eq!(through(saturate(0.0), [255, 255, 0]), [0xED; 3]);
        // composited-filters-under-opacity: yellow at invert(100%) is blue.
        assert_eq!(through(invert(1.0), [255, 255, 0]), [0, 0, 255]);
    }

    /// At the amounts that change nothing, the matrices are the identity to f32 rounding (which the
    /// renderer skips: no byte changes).
    #[test]
    fn neutral_amounts_are_the_identity() {
        let identity = opacity(1.0);
        for m in [
            brightness(1.0),
            contrast(1.0),
            grayscale(1.0),
            sepia(1.0),
            saturate(1.0),
            invert(0.0),
            hue_rotate(0.0),
        ] {
            assert!(
                m.iter().zip(identity).all(|(a, b)| (a - b).abs() <= 1e-6),
                "{m:?}"
            );
        }
    }
}
