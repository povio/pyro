use blitz_dom::util::ToColorColor as _;
use blitz_dom::{BaseDocument, Node};
use style::color::AbsoluteColor;
pub(crate) use style::computed_values::filter::single_value::T as StyloFilter;

use anyrender::filters::component_transfer::{
    ComponentTransferFilter, GammaTransferFunction, LinearTransferFunction, TransferFunction,
};
use anyrender::filters::{Filter, FilterEffect, color_transformation::ColorMatrix};

/// pyro: the filter in device pixels: lengths are CSS px, and the layers the filter is applied
/// to are drawn in device pixels (`scale` of them per CSS px) without a scale in their transform.
/// `current_color` is the element's `color`, which a `drop-shadow()` without one has. A `url()`
/// names an SVG `<filter>` of `dom` ([`reference_filter`]).
pub(crate) fn convert_filters(
    filters: &[StyloFilter],
    scale: f64,
    current_color: &AbsoluteColor,
    dom: &BaseDocument,
) -> Option<Filter> {
    if filters.is_empty() {
        return None;
    }

    Some(Filter::linear_list(filters.iter().flat_map(|filter| match filter {
        StyloFilter::Url(url) => reference_filter(dom, url).unwrap_or_default(),
        filter => convert_single_filter(filter, scale as f32, current_color).into_iter().collect(),
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

/// pyro: a `url(#id)` filter: the SVG `<filter>` of `dom` it names, when that is a chain of colour
/// primitives (`feColorMatrix`, `feComponentTransfer`; each one's input the one before it). Each
/// primitive works in its `color-interpolation-filters` space: linearRGB unless an ancestor's
/// attribute says sRGB, and sRGB for a filter in a `display: none` subtree, as in Chromium (a
/// `backdrop-filter: url()` matches `backdrop-filter: invert(1)` there). `None` for anything else.
fn reference_filter(dom: &BaseDocument, url: &style::url::ComputedUrl) -> Option<Vec<FilterEffect>> {
    let id = url.url()?.fragment()?;
    let filter = &dom.tree()[dom.get_element_by_id(id)?];
    if local_name(filter) != Some("filter") {
        return None;
    }
    // Stylo styles nothing in a `display: none` subtree.
    let unstyled = filter.primary_styles().is_none();
    let mut effects = Vec::new();
    let mut previous: Option<&str> = None;
    let primitives = filter.children.iter().map(|&id| &dom.tree()[id]).filter(|node| node.is_element());
    for (index, primitive) in primitives.enumerate() {
        match attr(primitive, "in") {
            None => {}
            Some("SourceGraphic") if index == 0 => {}
            Some(input) if Some(input) == previous => {}
            Some(_) => return None,
        }
        let effect = match local_name(primitive)? {
            "feColorMatrix" => color_matrix_primitive(primitive)?,
            "feComponentTransfer" => component_transfer_primitive(dom, primitive)?,
            _ => return None,
        };
        let linear = match interpolation(dom, primitive) {
            Some(space) => space.eq_ignore_ascii_case("linearRGB"),
            None => !unstyled,
        };
        if linear {
            effects.extend([to_linear(), effect, to_srgb()]);
        } else {
            effects.push(effect);
        }
        previous = attr(primitive, "result");
    }
    Some(effects)
}

fn local_name(node: &Node) -> Option<&str> {
    node.element_data().map(|element| &*element.name.local)
}

fn attr<'a>(node: &'a Node, name: &str) -> Option<&'a str> {
    node.attrs()?.iter().find(|attr| &*attr.name.local == name).map(|attr| attr.value.as_str())
}

/// The `color-interpolation-filters` of a primitive: its attribute, or the nearest ancestor's
/// (`auto` is sRGB).
fn interpolation<'a>(dom: &'a BaseDocument, node: &'a Node) -> Option<&'a str> {
    let mut at = Some(node);
    while let Some(node) = at {
        match attr(node, "color-interpolation-filters").map(str::trim) {
            Some("inherit") | None => {}
            Some("auto") => return Some("sRGB"),
            Some(space) => return Some(space),
        }
        at = node.parent.map(|parent| &dom.tree()[parent]);
    }
    None
}

fn numbers(value: &str) -> Vec<f32> {
    value
        .split(|c: char| c.is_ascii_whitespace() || c == ',')
        .filter(|n| !n.is_empty())
        .map_while(|n| n.parse().ok())
        .collect()
}

fn number(node: &Node, name: &str, default: f32) -> f32 {
    attr(node, name).and_then(|value| value.trim().parse().ok()).unwrap_or(default)
}

/// `feColorMatrix`, as the filter effects spec defines each `type` (a missing or malformed
/// `values` is the identity).
fn color_matrix_primitive(node: &Node) -> Option<FilterEffect> {
    let values = attr(node, "values").map(numbers);
    Some(color_matrix(match attr(node, "type").unwrap_or("matrix").trim() {
        "matrix" => match values {
            Some(values) if values.len() == 20 => values.try_into().ok()?,
            _ => opacity(1.0),
        },
        "saturate" => saturate_spec(values.and_then(|v| v.first().copied()).unwrap_or(1.0)),
        "hueRotate" => hue_rotate(values.and_then(|v| v.first().copied()).unwrap_or(0.0)),
        "luminanceToAlpha" => {
            let mut m = [0.0; 20];
            (m[15], m[16], m[17]) = (0.2125, 0.7154, 0.0721);
            m
        }
        _ => return None,
    }))
}

/// `feColorMatrix type="saturate"`: the spec's matrix (the blue column as written, not what the
/// row leaves of 1 as Chromium builds CSS `saturate()`).
fn saturate_spec(s: f32) -> [f32; 20] {
    let mut m = [0.0; 20];
    m[0] = 0.213 + 0.787 * s;
    m[1] = 0.715 - 0.715 * s;
    m[2] = 0.072 - 0.072 * s;
    m[5] = 0.213 - 0.213 * s;
    m[6] = 0.715 + 0.285 * s;
    m[7] = 0.072 - 0.072 * s;
    m[10] = 0.213 - 0.213 * s;
    m[11] = 0.715 - 0.715 * s;
    m[12] = 0.072 + 0.928 * s;
    m[18] = 1.0;
    m
}

/// `feComponentTransfer`: its `feFuncR`, `feFuncG`, `feFuncB` and `feFuncA` (the last of each).
fn component_transfer_primitive(dom: &BaseDocument, node: &Node) -> Option<FilterEffect> {
    let mut functions = [const { TransferFunction::Identity }; 4];
    for child in node.children.iter().map(|&id| &dom.tree()[id]) {
        let channel = match local_name(child) {
            Some("feFuncR") => 0,
            Some("feFuncG") => 1,
            Some("feFuncB") => 2,
            Some("feFuncA") => 3,
            _ => continue,
        };
        let values = attr(child, "tableValues").map(numbers).unwrap_or_default();
        functions[channel] = match attr(child, "type").unwrap_or("identity").trim() {
            "table" if !values.is_empty() => {
                TransferFunction::Table(if values.len() == 1 { vec![values[0]; 2] } else { values }.into())
            }
            "discrete" if !values.is_empty() => TransferFunction::Discrete(values),
            "linear" => TransferFunction::Linear(LinearTransferFunction {
                slope: number(child, "slope", 1.0),
                intercept: number(child, "intercept", 0.0),
            }),
            "gamma" => TransferFunction::Gamma(GammaTransferFunction {
                amplitude: number(child, "amplitude", 1.0),
                exponent: number(child, "exponent", 1.0),
                offset: number(child, "offset", 0.0),
            }),
            _ => TransferFunction::Identity,
        };
    }
    let [red_function, green_function, blue_function, alpha_function] = functions;
    Some(FilterEffect::ComponentTransfer(ComponentTransferFilter {
        red_function,
        green_function,
        blue_function,
        alpha_function,
    }))
}

/// sRGB values to linearRGB (the colour channels, as a table of 256 for the values of bytes).
pub(crate) fn to_linear() -> FilterEffect {
    srgb_table(256, |c| if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) })
}

/// linearRGB values back to sRGB (a table of 1024: the curve is steep near black).
pub(crate) fn to_srgb() -> FilterEffect {
    srgb_table(1024, |c| if c <= 0.0031308 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 })
}

fn srgb_table(len: usize, curve: impl Fn(f32) -> f32) -> FilterEffect {
    let table: Vec<f32> = (0..len).map(|i| curve(i as f32 / (len - 1) as f32)).collect();
    let function = TransferFunction::Table(table.into());
    FilterEffect::ComponentTransfer(ComponentTransferFilter {
        red_function: function.clone(),
        green_function: function.clone(),
        blue_function: function,
        alpha_function: TransferFunction::Identity,
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

/// pyro: a `filter()` image (CSS `<image>`): the image it filters, through nested `filter()`s,
/// and their filters, innermost first, in device pixels as [`convert_filters`] makes them.
/// `None` for an image that is not one.
pub(crate) fn image_filter<'a>(
    image: &'a style::values::computed::Image,
    scale: f64,
    current_color: &AbsoluteColor,
    dom: &BaseDocument,
) -> (
    &'a style::values::computed::Image,
    Option<std::sync::Arc<Filter>>,
) {
    use style::values::generics::image::GenericImage;
    let mut inner = image;
    let mut levels = Vec::new();
    while let GenericImage::Filter(filtered) = inner {
        levels.push(&filtered.filters);
        inner = &filtered.image;
    }
    if levels.is_empty() {
        return (image, None);
    }
    let filters: Vec<StyloFilter> = levels
        .iter()
        .rev()
        .flat_map(|level| level.iter().cloned())
        .collect();
    (
        inner,
        convert_filters(&filters, scale, current_color, dom).map(std::sync::Arc::new),
    )
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
