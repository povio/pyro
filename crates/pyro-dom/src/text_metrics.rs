//! Text measurement outside flow layout: canvas `measureText` and SVG text
//! boxes. Proportional families use the system UI (SF Pro) advance table
//! that the flow layout measures with; monospace families and characters
//! outside the table use the engine's fixed-advance measure.

/// Width in px of `text` at `font_size` px and `weight`.
pub fn text_width(text: &str, font_size: f64, weight: u16) -> f64 {
    text.chars()
        .map(|c| match crate::flow_font::advance(c, font_size, weight) {
            Some(em) => em * font_size,
            None => crate::layout::text_width(c.encode_utf8(&mut [0; 4]), font_size),
        })
        .sum()
}

/// Width in px with the engine's fixed-advance (monospace) measure.
pub fn monospace_width(text: &str, font_size: f64) -> f64 {
    crate::layout::text_width(text, font_size)
}

/// (ascent, descent) in px of the system UI font at `font_size`.
pub fn vertical_metrics(font_size: f64) -> (f64, f64) {
    crate::flow_font::vertical_metrics(font_size)
}

/// A parsed CSS `font` shorthand (canvas `font`): the parts text
/// measurement uses.
#[derive(Clone, Debug, PartialEq)]
pub struct FontSpec {
    pub style: String,
    /// `font-variant-caps` (canvas `fontVariantCaps`; the shorthand sets
    /// only `small-caps` or `normal`).
    pub variant_caps: &'static str,
    pub weight: u16,
    /// `font-stretch` keyword (canvas `fontStretch`).
    pub stretch: &'static str,
    pub size: f64,
    /// `font-family` as CSS serializes it (comma-separated list).
    pub family: String,
}

/// The `fontVariantCaps` keywords of HTML canvas.
pub const VARIANT_CAPS: [&str; 7] =
    ["normal", "small-caps", "all-small-caps", "petite-caps", "all-petite-caps", "unicase", "titling-caps"];

/// The `fontStretch` keywords of HTML canvas.
pub const STRETCHES: [&str; 9] = [
    "ultra-condensed",
    "extra-condensed",
    "condensed",
    "semi-condensed",
    "normal",
    "semi-expanded",
    "expanded",
    "extra-expanded",
    "ultra-expanded",
];

impl FontSpec {
    /// Whether the first available family is monospace.
    pub fn is_monospace(&self) -> bool {
        self.family.split(',').next().is_some_and(|f| {
            let f = f.trim().trim_matches(|c| c == '"' || c == '\'').to_ascii_lowercase();
            f == "monospace" || f.contains("mono") || f.contains("courier") || f == "menlo" || f == "consolas"
        })
    }

    /// Canvas `font` getter serialization (Chromium: style, `small-caps`
    /// and weight only when not normal, then `<size>px <family>`).
    pub fn serialize(&self) -> String {
        let mut out = String::new();
        if self.style != "normal" {
            out.push_str(&self.style);
            out.push(' ');
        }
        if self.variant_caps == "small-caps" {
            out.push_str("small-caps ");
        }
        if self.weight != 400 {
            out.push_str(&self.weight.to_string());
            out.push(' ');
        }
        out.push_str(&format!("{}px {}", crate::computed::fmt_num(self.size), self.family));
        out
    }
}

/// Parses a CSS `font` shorthand with Stylo's parser (CSS Fonts 4 `font`
/// grammar; CSS-wide keywords, `var()` and anything after the family list
/// are invalid), sizes resolved against `parent_size` (canvas: the canvas
/// element's computed `font-size`, else 10px). System fonts compute to
/// explicit values. Returns None for invalid values, which canvas ignores.
pub fn parse_font(value: &str, parent_size: f64) -> Option<FontSpec> {
    use style::context::QuirksMode;
    use style::stylesheets::{CssRuleType, Origin};
    use style_traits::ToCss;
    crate::stylo::enable_features();
    let url = crate::stylo::url_data();
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
    let font = cssparser::Parser::new(value)
        .parse_entirely(|input| style::properties::shorthands::font::parse_value(&context, input))
        .ok()?;
    if let Some(system) = font.font_family.get_system() {
        use style::values::specified::font::SystemFont;
        // Chromium on macOS: the system UI font at 13px (11px small captions).
        let size = if matches!(system, SystemFont::SmallCaption) { 11.0 } else { 13.0 };
        return Some(FontSpec {
            style: "normal".to_owned(),
            variant_caps: "normal",
            weight: 400,
            stretch: "normal",
            size,
            family: "system-ui".to_owned(),
        });
    }
    let style = match font.font_style.to_css_string() {
        s if s.is_empty() => "normal".to_owned(),
        s => s,
    };
    let variant_caps = if font.font_variant_caps.to_css_string() == "small-caps" { "small-caps" } else { "normal" };
    let weight = match font.font_weight.to_css_string().as_str() {
        "normal" => 400,
        "bold" | "bolder" => 700,
        "lighter" => 100,
        w => w.parse::<f64>().ok().filter(|n| (1.0..=1000.0).contains(n)).map_or(400, |n| n.round() as u16),
    };
    let stretch_css = font.font_width.to_css_string();
    let stretch = STRETCHES.iter().copied().find(|k| *k == stretch_css).unwrap_or("normal");
    let size = font_size_px(&font.font_size.to_css_string(), parent_size)?;
    let family = font.font_family.to_css_string();
    if family.is_empty() {
        return None;
    }
    Some(FontSpec { style, variant_caps, weight, stretch, size, family })
}

fn font_size_px(v: &str, parent: f64) -> Option<f64> {
    let v = v.trim().to_ascii_lowercase();
    let keyword = match v.as_str() {
        "xx-small" => Some(9.0),
        "x-small" => Some(10.0),
        "small" => Some(13.0),
        "medium" => Some(16.0),
        "large" => Some(18.0),
        "x-large" => Some(24.0),
        "xx-large" => Some(32.0),
        "xxx-large" => Some(48.0),
        "larger" => Some(parent * 1.2),
        "smaller" => Some(parent / 1.2),
        _ => None,
    };
    if keyword.is_some() {
        return keyword;
    }
    let unit = |suffix: &str| v.strip_suffix(suffix).and_then(|n| n.parse::<f64>().ok());
    let px = if let Some(n) = unit("px") {
        n
    } else if let Some(n) = unit("rem") {
        n * 16.0
    } else if let Some(n) = unit("em") {
        n * parent
    } else if let Some(n) = unit("ex").or_else(|| unit("ch")) {
        n * parent / 2.0
    } else if let Some(n) = unit("pt") {
        n * 4.0 / 3.0
    } else if let Some(n) = unit("pc") {
        n * 16.0
    } else if let Some(n) = unit("in") {
        n * 96.0
    } else if let Some(n) = unit("cm") {
        n * 96.0 / 2.54
    } else if let Some(n) = unit("mm") {
        n * 96.0 / 25.4
    } else if let Some(n) = unit("q") {
        n * 96.0 / 101.6
    } else {
        unit("%")? * parent / 100.0
    };
    (px.is_finite() && px >= 0.0).then_some(px)
}
