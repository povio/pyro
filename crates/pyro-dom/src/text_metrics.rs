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
    pub weight: u16,
    pub size: f64,
    /// `font-family` as written (comma-separated list).
    pub family: String,
}

impl FontSpec {
    /// Whether the first available family is monospace.
    pub fn is_monospace(&self) -> bool {
        self.family.split(',').next().is_some_and(|f| {
            let f = f.trim().trim_matches(|c| c == '"' || c == '\'').to_ascii_lowercase();
            f == "monospace" || f.contains("mono") || f.contains("courier") || f == "menlo" || f == "consolas"
        })
    }

    /// Canvas `font` getter serialization (Chromium: style and weight only
    /// when not normal, then `<size>px <family>`).
    pub fn serialize(&self) -> String {
        let mut out = String::new();
        if self.style != "normal" {
            out.push_str(&self.style);
            out.push(' ');
        }
        if self.weight != 400 {
            out.push_str(&self.weight.to_string());
            out.push(' ');
        }
        out.push_str(&format!("{}px {}", crate::computed::fmt_num(self.size), self.family));
        out
    }
}

/// Parses a CSS `font` shorthand with absolute or relative sizes resolved
/// against `parent_size` (canvas: 10px default, `em` against it). Returns
/// None for invalid values, which canvas ignores.
pub fn parse_font(value: &str, parent_size: f64) -> Option<FontSpec> {
    let v = value.trim();
    let mut style = "normal".to_owned();
    let mut weight = 400u16;
    let mut rest = v;
    loop {
        let (word, tail) = match rest.find(char::is_whitespace) {
            Some(i) => (&rest[..i], rest[i..].trim_start()),
            None => (rest, ""),
        };
        let lower = word.to_ascii_lowercase();
        match lower.as_str() {
            "normal" | "small-caps" => {}
            "italic" | "oblique" => style = lower.clone(),
            "bold" => weight = 700,
            "bolder" => weight = 700,
            "lighter" => weight = 100,
            w if w.parse::<f64>().is_ok_and(|n| (1.0..=1000.0).contains(&n)) && !w.ends_with('%') => {
                weight = w.parse::<f64>().ok()? as u16
            }
            "ultra-condensed" | "extra-condensed" | "condensed" | "semi-condensed" | "semi-expanded" | "expanded"
            | "extra-expanded" | "ultra-expanded" => {}
            _ => break,
        }
        rest = tail;
        if rest.is_empty() {
            return None;
        }
    }
    // <size>[ / <line-height>] <family>
    let mut tokens = rest.split_whitespace().peekable();
    let first = tokens.next()?;
    let (size_text, lh_attached) = match first.split_once('/') {
        Some((size, lh)) => (size, !lh.is_empty()),
        None => (first, false),
    };
    if !lh_attached && (first.ends_with('/') || tokens.peek().is_some_and(|t| t.starts_with('/'))) {
        let t = if first.ends_with('/') { None } else { tokens.next() };
        if t.is_none_or(|t| t == "/") {
            tokens.next()?;
        }
    }
    let family = tokens.collect::<Vec<_>>().join(" ");
    if family.is_empty() {
        return None;
    }
    let size = font_size_px(size_text, parent_size)?;
    Some(FontSpec { style, weight, size, family })
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
    } else if let Some(n) = unit("pt") {
        n * 4.0 / 3.0
    } else {
        unit("%")? * parent / 100.0
    };
    (px.is_finite() && px >= 0.0).then_some(px)
}
