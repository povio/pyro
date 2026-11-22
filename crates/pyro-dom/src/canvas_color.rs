//! Colors of the 2D canvas API (`fillStyle`, `strokeStyle`, `shadowColor`,
//! `addColorStop`): a string parsed as a CSS `<color>` with Stylo's parser,
//! and the color serialized as HTML's "serialization of a color" (as Chromium
//! prints it).

use style::color::{AbsoluteColor, ColorSpace};
use style::context::QuirksMode;
use style::servo_arc::Arc;
use style::stylesheets::{CssRuleType, Origin, UrlExtraData};

/// A parsed canvas color: its serialization and its components in the
/// canvas's color space (0..1, clipped to the gamut, `none` as 0) with alpha.
#[derive(Clone, Debug, PartialEq)]
pub struct CanvasColor {
    pub css: String,
    pub rgba: [f32; 4],
}

/// A canvas bitmap's or an `ImageData`'s color space (HTML
/// `PredefinedColorSpace`): the space its 8-bit RGB values are in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Space {
    #[default]
    Srgb,
    SrgbLinear,
    DisplayP3,
    DisplayP3Linear,
}

impl Space {
    /// The `PredefinedColorSpace` value `name` (case-sensitive), if it is one.
    pub fn parse(name: &str) -> Option<Space> {
        match name {
            "srgb" => Some(Space::Srgb),
            "srgb-linear" => Some(Space::SrgbLinear),
            "display-p3" => Some(Space::DisplayP3),
            "display-p3-linear" => Some(Space::DisplayP3Linear),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Space::Srgb => "srgb",
            Space::SrgbLinear => "srgb-linear",
            Space::DisplayP3 => "display-p3",
            Space::DisplayP3Linear => "display-p3-linear",
        }
    }

    fn stylo(self) -> ColorSpace {
        match self {
            Space::Srgb => ColorSpace::Srgb,
            Space::SrgbLinear => ColorSpace::SrgbLinear,
            Space::DisplayP3 => ColorSpace::DisplayP3,
            Space::DisplayP3Linear => ColorSpace::DisplayP3Linear,
        }
    }

    /// Whether values are encoded with the sRGB transfer function (Display P3
    /// uses the same curve).
    fn encoded(self) -> bool {
        matches!(self, Space::Srgb | Space::DisplayP3)
    }

    fn p3(self) -> bool {
        matches!(self, Space::DisplayP3 | Space::DisplayP3Linear)
    }
}

/// `raw` parsed as a CSS `<color>`: `None` when it is not one, so the caller
/// keeps its previous style. `currentcolor` resolves to `current` (the canvas
/// element's `color`, black when there is none). System colors use the light
/// scheme.
pub fn parse(raw: &str, current: [f32; 4]) -> Option<CanvasColor> {
    parse_in(raw, current, Space::Srgb)
}

/// [`parse`] with the components in `space` (a canvas's color space).
pub fn parse_in(raw: &str, current: [f32; 4], space: Space) -> Option<CanvasColor> {
    let color = absolute(raw.trim_matches(|c: char| c.is_ascii_whitespace()), current)?;
    Some(CanvasColor { css: serialize(&color), rgba: components(&color, space) })
}

/// Converts straight-alpha RGBA8 `pixels` from `from` to `to` (gamut-clipped,
/// rounded to nearest; alpha kept).
pub fn convert_pixels(pixels: &mut [u8], from: Space, to: Space) {
    if from == to {
        return;
    }
    // sRGB (and P3) decoding of every byte value.
    let decode: Vec<f32> = (0..=255u8)
        .map(|v| {
            let v = f32::from(v) / 255.0;
            if from.encoded() {
                to_linear(v)
            } else {
                v
            }
        })
        .collect();
    for p in pixels.chunks_exact_mut(4) {
        let mut rgb = [decode[p[0] as usize], decode[p[1] as usize], decode[p[2] as usize]];
        if from.p3() != to.p3() {
            let m = if from.p3() { &P3_TO_SRGB } else { &SRGB_TO_P3 };
            rgb = [0usize, 1, 2].map(|i| m[i][0] * rgb[0] + m[i][1] * rgb[1] + m[i][2] * rgb[2]);
        }
        for (out, v) in p.iter_mut().zip(rgb) {
            let v = v.clamp(0.0, 1.0);
            let v = if to.encoded() { from_linear(v) } else { v };
            *out = (v * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }
}

/// Linear-light sRGB to linear-light Display P3 (CSS Color 4 matrices, D65).
const SRGB_TO_P3: [[f32; 3]; 3] =
    [[0.822_462_1, 0.177_538, 0.0], [0.033_194_2, 0.966_805_8, 0.0], [0.017_082_7, 0.072_397_4, 0.910_519_9]];

const P3_TO_SRGB: [[f32; 3]; 3] = [
    [1.224_940_1, -0.224_940_4, 0.0],
    [-0.042_056_955, 1.042_057_1, 0.0],
    [-0.019_637_555, -0.078_636_05, 1.098_273_5],
];

fn to_linear(v: f32) -> f32 {
    if v <= 0.040_45 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

fn from_linear(v: f32) -> f32 {
    if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

fn absolute(raw: &str, current: [f32; 4]) -> Option<AbsoluteColor> {
    crate::stylo::enable_features();
    let url = UrlExtraData(Arc::new(url::Url::parse("about:blank").ok()?));
    let context = style::parser::ParserContext::new(
        Origin::Author,
        &url,
        Some(CssRuleType::Style),
        style_traits::ParsingMode::DEFAULT,
        QuirksMode::NoQuirks,
        Default::default(),
        None,
        None,
        Default::default(),
    );
    use style::values::specified::Color;
    let computed = match Color::parse_and_compute(&context, &mut cssparser::Parser::new(raw), None) {
        Ok(computed) => computed,
        // System colors compute only against a device.
        Err(()) => {
            let device = crate::picture::sizes_device(&crate::media::Viewport::default());
            Color::parse_and_compute(&context, &mut cssparser::Parser::new(raw), Some(&device)).ok()?
        }
    };
    let [r, g, b, a] = current;
    Some(computed.resolve_to_absolute(&AbsoluteColor::srgb_legacy((r * 255.0).round() as u8, (g * 255.0).round() as u8, (b * 255.0).round() as u8, a)))
}

fn srgb(color: &AbsoluteColor) -> [f32; 4] {
    components(color, Space::Srgb)
}

fn components(color: &AbsoluteColor, space: Space) -> [f32; 4] {
    let c = color.to_color_space(space.stylo());
    let unit = |v: Option<f32>| v.unwrap_or(0.0).clamp(0.0, 1.0);
    [unit(c.c0()), unit(c.c1()), unit(c.c2()), unit(color.alpha())]
}

/// Legacy sRGB syntaxes (hex, names, `rgb()`, `hsl()`, `hwb()`) print as
/// `#rrggbb` when opaque and `rgba(r, g, b, a)` otherwise; any other color
/// prints as CSS serializes it (`color(srgb ...)`, `lab(...)`).
fn serialize(color: &AbsoluteColor) -> String {
    use style_traits::ToCss;
    if !color.is_legacy_syntax() {
        return color.to_css_string();
    }
    let [r, g, b, a] = srgb(color).map(|v| (v * 255.0).round() as u8);
    if a == 255 {
        format!("#{r:02x}{g:02x}{b:02x}")
    } else {
        format!("rgba({r}, {g}, {b}, {})", alpha(a))
    }
}

/// Chromium keeps alpha in 8 bits and prints the shortest decimal that maps
/// back to it (two places when possible, else three).
fn alpha(a8: u8) -> String {
    let a8 = f64::from(a8);
    let two = (a8 / 255.0 * 100.0).round() / 100.0;
    let value = if (two * 255.0).round() == a8 { two } else { (a8 / 255.0 * 1000.0).round() / 1000.0 };
    format!("{value}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

    fn css(raw: &str) -> Option<String> {
        parse(raw, BLACK).map(|c| c.css)
    }

    #[test]
    fn serializes_as_canvas() {
        for (input, output) in [
            ("rgb(255, 0, 255)", "#ff00ff"),
            ("rgba(255, 0, 255, 1)", "#ff00ff"),
            ("#ff00ffff", "#ff00ff"),
            ("#ff00ff00", "rgba(255, 0, 255, 0)"),
            ("rgba(255, 0, 255,    0)", "rgba(255, 0, 255, 0)"),
            ("rgba(255, 255, 255, 0.45)", "rgba(255, 255, 255, 0.45)"),
            ("transparent", "rgba(0, 0, 0, 0)"),
            ("rgb(none none none)", "#000000"),
            ("hsl(120, 100%, 50%)", "#00ff00"),
            ("  lime\t", "#00ff00"),
            ("rgba(0, 255, 0, 1", "#00ff00"),
            ("color(srgb 1 0 1)", "color(srgb 1 0 1)"),
            ("color(srgb 1 0 1/0)", "color(srgb 1 0 1 / 0)"),
            ("lab(29% 39 20)", "lab(29 39 20)"),
            ("color-mix(in srgb, red, blue)", "color(srgb 0.5 0 0.5)"),
            ("rgb(from red g r b)", "color(srgb 0 1 0)"),
            ("color(from color(srgb 0.25 0.5 0.75 / 0.5) srgb r g b / alpha)", "color(srgb 0.25 0.5 0.75 / 0.5)"),
            ("currentcolor", "#000000"),
        ] {
            assert_eq!(css(input).as_deref(), Some(output), "{input}");
        }
    }

    #[test]
    fn rejects_invalid_colors() {
        for input in [
            "",
            "null",
            "undefined",
            "inherit",
            "rgb(NaN%, NaN%, NaN%)",
            "hsl(0, 100%, 50% / 1)",
            "rgb(255, 0, 0 / 1)",
            "hsl(0%, 100%, 50%)",
            "rgb(255.0, 0, 0,)",
            "rgb(100%, 0, 0)",
            "rgb(255 0 0,)",
        ] {
            assert_eq!(css(input), None, "{input}");
        }
    }

    #[test]
    fn system_colors_and_current_color() {
        let system = css("ThreeDDarkShadow").unwrap();
        assert!(system.starts_with('#') && system != "#ff0000", "{system}");
        let lime = parse("currentColor", [0.0, 1.0, 0.0, 1.0]).unwrap();
        assert_eq!(lime.css, "#00ff00");
        assert_eq!(lime.rgba, [0.0, 1.0, 0.0, 1.0]);
    }
}
