use blitz_dom::util::ToColorColor as _;
use style::color::AbsoluteColor;
pub(crate) use style::computed_values::filter::single_value::T as StyloFilter;

use anyrender::filters::{Filter, FilterEffect};

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
        StyloFilter::Brightness(amount) => FilterEffect::brightness(amount.0),
        StyloFilter::Contrast(amount) => FilterEffect::contrast(amount.0),
        StyloFilter::Grayscale(amount) => FilterEffect::grayscale(amount.0),
        StyloFilter::HueRotate(angle) => FilterEffect::hue_rotate(angle.radians()),
        StyloFilter::Invert(amount) => FilterEffect::invert(amount.0),
        StyloFilter::Opacity(amount) => FilterEffect::opacity(amount.0),
        StyloFilter::Saturate(amount) => FilterEffect::saturate(amount.0),
        StyloFilter::Sepia(amount) => FilterEffect::sepia(amount.0),
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
