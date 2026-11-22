//! The value of `<input type=color>` (HTML "color state"): any CSS color,
//! serialized in the control's `colorspace` (`limited-srgb` or `display-p3`)
//! with or without its alpha channel (the `alpha` attribute).

use style::color::{AbsoluteColor, ColorSpace};
use style::context::QuirksMode;
use style::servo_arc::Arc;
use style::stylesheets::{CssRuleType, Origin, UrlExtraData};

/// `raw` parsed as a CSS color: `None` when it is not one. `currentcolor`
/// is black; colors that need a style context (system colors) do not parse.
fn parse(raw: &str) -> Option<AbsoluteColor> {
    parse_color(raw, false)
}

fn parse_color(raw: &str, hashless: bool) -> Option<AbsoluteColor> {
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
    let mut parser = cssparser::Parser::new(raw.trim_matches(|c: char| c.is_ascii_whitespace()));
    use style::values::specified::{AllowQuirks, Color};
    let computed = if hashless {
        // The editing command accepts numeric CSS tokens as quirky colors in
        // both document modes. Keep CSS-wide keywords on the ordinary path.
        let start = parser.state();
        match parser.next().ok()? {
            cssparser::Token::Number { .. } | cssparser::Token::Dimension { .. } => {}
            cssparser::Token::Ident(name) if matches!(name.len(), 3 | 6) && name.bytes().all(|b| b.is_ascii_hexdigit()) => {}
            _ => return None,
        }
        parser.reset(&start);
        parser.parse_entirely(|p| Color::parse_quirky(&context, p, AllowQuirks::Always)).ok()?
            .to_computed_color(None).ok()?
    } else {
        Color::parse_and_compute(&context, &mut parser, None).ok()?
    };
    Some(computed.resolve_to_absolute(&AbsoluteColor::BLACK))
}

pub fn is_fully_transparent(raw: &str) -> bool {
    raw == "rgba(0, 0, 0, 0)" || raw == "transparent" || parse(raw).is_some_and(|color| color.alpha() == Some(0.0))
}

/// The resolved CSS color string and its opaque legacy HTML color (when one
/// exists). Editing commands reject currentColor and invalid inputs.
pub fn editing_color(raw: &str) -> Option<(String, Option<String>)> {
    if raw.trim().eq_ignore_ascii_case("currentcolor") {
        return None;
    }
    Some(editing_components(parse(raw)?))
}

/// Hashless editing colors use ordinary three- and six-digit hex first.
/// Otherwise Stylo handles CSS number/dimension tokenization and padding.
pub fn hashless_foreground(raw: &str) -> Option<(String, Option<String>)> {
    let raw = raw.trim_matches(|c: char| c.is_ascii_whitespace());
    if matches!(raw.len(), 3 | 6) && raw.bytes().all(|b| b.is_ascii_hexdigit()) {
        return editing_color(&format!("#{raw}"));
    }
    Some(editing_components(parse_color(raw, true)?))
}

fn editing_components(color: AbsoluteColor) -> (String, Option<String>) {
    use style_traits::ToCss;
    let legacy = if color.alpha() == Some(1.0) && color.is_legacy_syntax() {
        let rgb = color.to_color_space(ColorSpace::Srgb);
        let [r, g, b] =
            [rgb.c0(), rgb.c1(), rgb.c2()].map(|v| (v.unwrap_or(0.0).clamp(0.0, 1.0) * 255.0).round() as u8);
        Some(format!("#{r:02x}{g:02x}{b:02x}"))
    } else {
        None
    };
    (color.to_css_string(), legacy)
}

/// A component as CSS color serialization prints it: at most six decimals,
/// no trailing zeros.
fn num(v: f32) -> String {
    let s = format!("{:.6}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" {
        "0".into()
    } else {
        s.into()
    }
}

/// The color state's value sanitization: `raw` as a CSS color (black when it
/// is not one), converted to `display-p3` when `p3`, else to sRGB clipped to
/// its gamut and stored in 8 bits per channel. Without `alpha` the color is
/// opaque and sRGB serializes as `#rrggbb`; otherwise as `color(...)`.
pub fn sanitize(raw: &str, alpha: bool, p3: bool) -> String {
    let color = parse(raw).unwrap_or(AbsoluteColor::BLACK);
    let a = if alpha { (color.alpha().unwrap_or(0.0).clamp(0.0, 1.0) * 255.0).round() / 255.0 } else { 1.0 };
    let suffix = if a < 1.0 { format!(" / {}", num(a)) } else { String::new() };
    if p3 {
        let c = color.to_color_space(ColorSpace::DisplayP3);
        let [r, g, b] = [c.c0(), c.c1(), c.c2()].map(|v| num(v.unwrap_or(0.0)));
        return format!("color(display-p3 {r} {g} {b}{suffix})");
    }
    let c = color.to_color_space(ColorSpace::Srgb);
    let [r, g, b] = [c.c0(), c.c1(), c.c2()].map(|v| (v.unwrap_or(0.0).clamp(0.0, 1.0) * 255.0).round());
    if !alpha {
        return format!("#{:02x}{:02x}{:02x}", r as u8, g as u8, b as u8);
    }
    let [r, g, b] = [r, g, b].map(|v| num(v / 255.0));
    format!("color(srgb {r} {g} {b}{suffix})")
}

#[cfg(test)]
mod tests {
    use super::sanitize;

    #[test]
    fn color_well_values() {
        assert_eq!(sanitize("", false, false), "#000000");
        assert_eq!(sanitize(" #FFF ", false, false), "#ffffff");
        assert_eq!(sanitize("#ffffff;", false, false), "#000000");
        assert_eq!(sanitize("inherit", false, false), "#000000");
        assert_eq!(sanitize("currentColor", true, false), "color(srgb 0 0 0)");
        assert_eq!(sanitize("crimson", false, false), "#dc143c");
        assert_eq!(sanitize("hsl(150deg 100 53.5)", false, false), "#12ff88");
        assert_eq!(sanitize("color(display-p3 .5 0 0)", false, false), "#8c0000");
        assert_eq!(sanitize("transparent", true, false), "color(srgb 0 0 0 / 0)");
        assert_eq!(sanitize("#ffffff08", true, false), "color(srgb 1 1 1 / 0.031373)");
        assert_eq!(sanitize("rgb(1,1,1,0.5)", false, false), "#010101");
        assert_eq!(sanitize("white", false, true), "color(display-p3 1 1 1)");
        assert_eq!(sanitize("color(display-p3 3 none .2 / .6)", true, true), "color(display-p3 3 0 0.2 / 0.6)");
    }
}
