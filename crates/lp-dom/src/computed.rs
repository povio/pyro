//! Computed-value serialization for author-declared properties that the
//! renderless layout does not track (see `style.rs`). `getComputedStyle`
//! reports these from the cascade, the way Chromium serializes them:
//! colors as `rgb()`/`rgba()` (or normalized `oklch()`/`oklab()`), absolute
//! lengths and `calc()` of absolute lengths in px, other values as declared.
//!
//! This is value serialization, not layout: percentages, `auto` and other
//! used-value-dependent lengths stay as declared, and nothing here affects
//! geometry or hit testing.

use cssparser::{ParseError, Parser, Token};

/// Properties whose value inherits when nothing is declared on the element.
pub fn is_inherited(name: &str) -> bool {
    matches!(
        name,
        "color"
            | "caret-color"
            | "cursor"
            | "direction"
            | "fill"
            | "fill-opacity"
            | "font-family"
            | "font-feature-settings"
            | "font-stretch"
            | "font-style"
            | "font-variant"
            | "font-variant-numeric"
            | "font-weight"
            | "hyphens"
            | "letter-spacing"
            | "line-height"
            | "list-style-position"
            | "list-style-type"
            | "overflow-wrap"
            | "quotes"
            | "stroke"
            | "stroke-width"
            | "tab-size"
            | "text-align"
            | "text-indent"
            | "text-shadow"
            | "white-space"
            | "word-break"
            | "word-spacing"
            | "-webkit-text-fill-color"
    )
}

/// SVG presentation attributes mapped to the property of the same name.
pub fn is_presentation_attribute(name: &str) -> bool {
    matches!(
        name,
        "fill"
            | "fill-opacity"
            | "stroke"
            | "stroke-width"
            | "stroke-opacity"
            | "stroke-linecap"
            | "stroke-linejoin"
            | "stroke-dasharray"
            | "opacity"
            | "color"
            | "visibility"
            | "display"
            | "font-family"
            | "font-size"
            | "font-weight"
            | "text-anchor"
    )
}

/// Properties whose computed value is a color.
pub fn is_color_property(name: &str) -> bool {
    matches!(
        name,
        "color"
            | "background-color"
            | "border-color"
            | "border-top-color"
            | "border-right-color"
            | "border-bottom-color"
            | "border-left-color"
            | "caret-color"
            | "column-rule-color"
            | "fill"
            | "outline-color"
            | "stroke"
            | "text-decoration-color"
            | "accent-color"
            | "-webkit-text-fill-color"
    )
}

/// Properties whose absolute lengths (`rem`, `em`, `calc()`) compute to px.
pub fn is_length_property(name: &str) -> bool {
    matches!(
        name,
        "margin-top"
            | "margin-right"
            | "margin-bottom"
            | "margin-left"
            | "padding-top"
            | "padding-right"
            | "padding-bottom"
            | "padding-left"
            | "row-gap"
            | "column-gap"
            | "top"
            | "right"
            | "bottom"
            | "left"
            | "width"
            | "height"
            | "min-width"
            | "min-height"
            | "max-width"
            | "max-height"
            | "flex-basis"
            | "font-size"
            | "letter-spacing"
            | "word-spacing"
            | "text-indent"
            | "border-top-width"
            | "border-right-width"
            | "border-bottom-width"
            | "border-left-width"
            | "outline-width"
            | "outline-offset"
            | "border-top-left-radius"
            | "border-top-right-radius"
            | "border-bottom-right-radius"
            | "border-bottom-left-radius"
    )
}

/// Splits a value at top-level whitespace, keeping functions and blocks whole.
fn top_level_parts(value: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = 0u32;
    let mut quote: Option<char> = None;
    let mut cur = String::new();
    for c in value.chars() {
        if let Some(q) = quote {
            cur.push(c);
            if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '"' | '\'' => {
                quote = Some(c);
                cur.push(c);
            }
            '(' | '[' => {
                depth += 1;
                cur.push(c);
            }
            ')' | ']' => {
                depth = depth.saturating_sub(1);
                cur.push(c);
            }
            c if c.is_ascii_whitespace() && depth == 0 => {
                if !cur.is_empty() {
                    parts.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        parts.push(cur);
    }
    parts
}

fn sides(prefix: &str, suffix: &str, value: &str) -> Vec<(String, String)> {
    let parts = top_level_parts(value);
    let [t, r, b, l] = match parts.as_slice() {
        [a] => [a, a, a, a],
        [a, b] => [a, b, a, b],
        [a, b, c] => [a, b, c, b],
        [a, b, c, d] => [a, b, c, d],
        _ => return Vec::new(),
    };
    [("top", t), ("right", r), ("bottom", b), ("left", l)]
        .into_iter()
        .map(|(side, v)| (format!("{prefix}{side}{suffix}"), v.clone()))
        .collect()
}

fn is_number(s: &str) -> bool {
    s.parse::<f64>().is_ok()
}

/// The longhands a declaration sets. Unknown shorthands are kept as written
/// under their own name; `var()` in a multi-part shorthand cannot be split
/// before substitution and is assigned to each longhand whole.
pub fn expand(name: &str, value: &str) -> Vec<(String, String)> {
    let one = |n: &str| vec![(n.to_owned(), value.to_owned())];
    match name {
        "overflow" | "overscroll-behavior" => {
            let parts = top_level_parts(value);
            let (x, y) = match parts.as_slice() {
                [a] => (a.clone(), a.clone()),
                [a, b] => (a.clone(), b.clone()),
                _ => return one(name),
            };
            vec![(format!("{name}-x"), x), (format!("{name}-y"), y)]
        }
        "gap" | "grid-gap" => {
            let parts = top_level_parts(value);
            let (r, c) = match parts.as_slice() {
                [a] => (a.clone(), a.clone()),
                [a, b] => (a.clone(), b.clone()),
                _ => return one(name),
            };
            vec![("row-gap".into(), r), ("column-gap".into(), c)]
        }
        "margin" | "padding" => sides(&format!("{name}-"), "", value),
        "inset" => sides("", "", value),
        "border-color" | "border-width" | "border-style" => {
            let suffix = &name["border".len()..];
            sides("border-", suffix, value)
        }
        "flex" => {
            let v = value.trim();
            let (g, s, b) = match v.to_ascii_lowercase().as_str() {
                "none" => ("0".into(), "0".into(), "auto".into()),
                "auto" => ("1".into(), "1".into(), "auto".into()),
                "initial" => ("0".into(), "1".into(), "auto".into()),
                _ => {
                    let parts = top_level_parts(v);
                    match parts.as_slice() {
                        [a] if is_number(a) => (a.clone(), "1".into(), "0%".into()),
                        [a] => ("1".into(), "1".into(), a.clone()),
                        [a, b] if is_number(a) && is_number(b) => (a.clone(), b.clone(), "0%".into()),
                        [a, b] if is_number(a) => (a.clone(), "1".into(), b.clone()),
                        [a, b, c] => (a.clone(), b.clone(), c.clone()),
                        _ => return one(name),
                    }
                }
            };
            vec![("flex-grow".into(), g), ("flex-shrink".into(), s), ("flex-basis".into(), b)]
        }
        "background" => {
            // Only a lone color sets background-color; layered backgrounds are not modelled.
            if parse_color(value.trim()).is_some() || value.trim().to_ascii_lowercase().starts_with("var(") {
                vec![("background-color".into(), value.to_owned())]
            } else {
                Vec::new()
            }
        }
        _ => one(name),
    }
}

// ---------------------------------------------------------------------------
// var() substitution
// ---------------------------------------------------------------------------

/// Replaces every `var(--x[, fallback])` using `lookup`, recursively.
/// `None` means the value is invalid at computed-value time.
pub fn substitute_vars(value: &str, lookup: &mut dyn FnMut(&str) -> Option<String>, depth: u8) -> Option<String> {
    if depth > 32 {
        return None;
    }
    let lower = value.to_ascii_lowercase();
    let Some(start) = lower.find("var(") else { return Some(value.to_owned()) };
    let open = start + 4;
    let mut level = 1u32;
    let mut end = None;
    for (i, c) in value[open..].char_indices() {
        match c {
            '(' => level += 1,
            ')' => {
                level -= 1;
                if level == 0 {
                    end = Some(open + i);
                    break;
                }
            }
            _ => {}
        }
    }
    let end = end?;
    let inner = &value[open..end];
    let (name, fallback) = match split_top_level_comma(inner) {
        Some((n, f)) => (n.trim(), Some(f.trim())),
        None => (inner.trim(), None),
    };
    if !name.starts_with("--") {
        return None;
    }
    let replacement = match lookup(name) {
        Some(v) if !v.trim().eq_ignore_ascii_case("initial") => substitute_vars(&v, lookup, depth + 1),
        _ => None,
    };
    let replacement = match replacement {
        Some(v) => v,
        None => substitute_vars(fallback?, lookup, depth + 1)?,
    };
    let rest = substitute_vars(&value[end + 1..], lookup, depth + 1)?;
    Some(format!("{}{}{}", &value[..start], replacement.trim(), rest))
}

fn split_top_level_comma(s: &str) -> Option<(&str, &str)> {
    let mut depth = 0u32;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => return Some((&s[..i], &s[i + 1..])),
            _ => {}
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Numbers and lengths
// ---------------------------------------------------------------------------

/// Chromium-style number serialization: at most six significant digits,
/// no trailing zeros, no exponent for the ranges CSS values use.
pub fn fmt_num(x: f64) -> String {
    if !x.is_finite() {
        return "0".into();
    }
    if (x - x.round()).abs() < 1e-9 {
        return format!("{}", x.round() as i64);
    }
    let digits = 6 - (x.abs().log10().floor() as i32 + 1);
    let digits = digits.clamp(0, 10) as usize;
    let s = format!("{x:.digits$}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" {
        "0".into()
    } else {
        s.to_owned()
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
enum CalcValue {
    Number(f64),
    /// `px + percent%` (percent of the property's percentage basis).
    Linear(f64, f64),
}

/// What relative units resolve against.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct LengthContext {
    pub font_size: f64,
    pub root_font_size: f64,
    pub viewport_width: f64,
    pub viewport_height: f64,
}

impl LengthContext {
    pub fn fonts(font_size: f64, root_font_size: f64) -> Self {
        LengthContext { font_size, root_font_size, viewport_width: 0.0, viewport_height: 0.0 }
    }
}

fn calc_sum(p: &mut Parser<'_>, cx: &LengthContext) -> Option<CalcValue> {
    let mut acc = calc_product(p, cx)?;
    loop {
        let state = p.state();
        match p.next() {
            Ok(Token::Delim('+')) => acc = add(acc, calc_product(p, cx)?, 1.0)?,
            Ok(Token::Delim('-')) => acc = add(acc, calc_product(p, cx)?, -1.0)?,
            Ok(_) => return None,
            Err(_) => {
                p.reset(&state);
                return Some(acc);
            }
        }
    }
}

fn add(a: CalcValue, b: CalcValue, sign: f64) -> Option<CalcValue> {
    match (a, b) {
        (CalcValue::Number(x), CalcValue::Number(y)) => Some(CalcValue::Number(x + sign * y)),
        (CalcValue::Linear(x, xp), CalcValue::Linear(y, yp)) => Some(CalcValue::Linear(x + sign * y, xp + sign * yp)),
        // `0 + 1px` is invalid in CSS; zero lengths are written with units by Tailwind.
        _ => None,
    }
}

fn calc_product(p: &mut Parser<'_>, cx: &LengthContext) -> Option<CalcValue> {
    let mut acc = calc_factor(p, cx)?;
    loop {
        let state = p.state();
        match p.next() {
            Ok(Token::Delim('*')) => {
                let rhs = calc_factor(p, cx)?;
                acc = match (acc, rhs) {
                    (CalcValue::Number(x), CalcValue::Number(y)) => CalcValue::Number(x * y),
                    (CalcValue::Linear(x, xp), CalcValue::Number(y))
                    | (CalcValue::Number(y), CalcValue::Linear(x, xp)) => CalcValue::Linear(x * y, xp * y),
                    _ => return None,
                };
            }
            Ok(Token::Delim('/')) => {
                let CalcValue::Number(y) = calc_factor(p, cx)? else { return None };
                if y == 0.0 {
                    return None;
                }
                acc = match acc {
                    CalcValue::Number(x) => CalcValue::Number(x / y),
                    CalcValue::Linear(x, xp) => CalcValue::Linear(x / y, xp / y),
                };
            }
            _ => {
                p.reset(&state);
                return Some(acc);
            }
        }
    }
}

fn calc_factor(p: &mut Parser<'_>, cx: &LengthContext) -> Option<CalcValue> {
    let token = p.next().ok()?.clone();
    match token {
        Token::Number { value, .. } => Some(CalcValue::Number(value as f64)),
        Token::Percentage { unit_value, .. } => Some(CalcValue::Linear(0.0, unit_value as f64 * 100.0)),
        Token::Dimension { value, ref unit, .. } => Some(CalcValue::Linear(absolute_px(value as f64, unit, cx)?, 0.0)),
        Token::ParenthesisBlock => p.parse_nested_block(|p| nested(p, cx)).ok(),
        Token::Function(ref name) if name.eq_ignore_ascii_case("calc") => p.parse_nested_block(|p| nested(p, cx)).ok(),
        _ => None,
    }
}

fn nested(p: &mut Parser<'_>, cx: &LengthContext) -> Result<CalcValue, ParseError<()>> {
    let value = calc_sum(p, cx).ok_or_else(|| ParseError::custom(()))?;
    p.expect_exhausted().map_err(|_| ParseError::custom(()))?;
    Ok(value)
}

fn absolute_px(value: f64, unit: &str, cx: &LengthContext) -> Option<f64> {
    let unit = unit.to_ascii_lowercase();
    Some(match unit.as_str() {
        "px" => value,
        "rem" => value * cx.root_font_size,
        "em" => value * cx.font_size,
        "pt" => value * 96.0 / 72.0,
        "pc" => value * 16.0,
        "in" => value * 96.0,
        "cm" => value * 96.0 / 2.54,
        "mm" => value * 96.0 / 25.4,
        "q" => value * 96.0 / 101.6,
        "vw" | "dvw" | "svw" | "lvw" if cx.viewport_width > 0.0 => value * cx.viewport_width / 100.0,
        "vh" | "dvh" | "svh" | "lvh" if cx.viewport_height > 0.0 => value * cx.viewport_height / 100.0,
        _ => return None,
    })
}

/// A single length, percentage or `calc()` of them as `(px, percent)`.
/// `None` for keywords, lists and unsupported units.
pub fn resolve_len(value: &str, cx: &LengthContext) -> Option<(f64, f64)> {
    let mut p = Parser::new(value.trim());
    let token = p.next().ok()?.clone();
    let result = match token {
        Token::Dimension { value, ref unit, .. } => CalcValue::Linear(absolute_px(value as f64, unit, cx)?, 0.0),
        Token::Percentage { unit_value, .. } => CalcValue::Linear(0.0, unit_value as f64 * 100.0),
        Token::Number { value, .. } if value == 0.0 => CalcValue::Linear(0.0, 0.0),
        Token::Function(ref name) if name.eq_ignore_ascii_case("calc") => {
            p.parse_nested_block(|p| nested(p, cx)).ok()?
        }
        _ => return None,
    };
    p.expect_exhausted().ok()?;
    match result {
        CalcValue::Linear(px, pct) => Some((px, pct)),
        CalcValue::Number(_) => None,
    }
}

/// A single length or `calc()` of absolute lengths, as `Npx`. `None` keeps
/// the declared value (percentages, keywords, viewport units, lists).
pub fn resolve_length(value: &str, font_size: f64, root_font_size: f64) -> Option<String> {
    if value.contains('%') {
        return None;
    }
    let (px, _) = resolve_len(value, &LengthContext::fonts(font_size, root_font_size))?;
    Some(format!("{}px", fmt_num(px)))
}

/// A unitless number or `calc()` of numbers, else `None`.
pub fn resolve_number(value: &str) -> Option<f64> {
    let cx = LengthContext::fonts(16.0, 16.0);
    let mut p = Parser::new(value.trim());
    let token = p.next().ok()?.clone();
    let result = match token {
        Token::Number { value, .. } => CalcValue::Number(value as f64),
        Token::Function(ref name) if name.eq_ignore_ascii_case("calc") => {
            p.parse_nested_block(|p| nested(p, &cx)).ok()?
        }
        _ => return None,
    };
    p.expect_exhausted().ok()?;
    match result {
        CalcValue::Number(n) => Some(n),
        CalcValue::Linear(..) => None,
    }
}

// ---------------------------------------------------------------------------
// Colors
// ---------------------------------------------------------------------------

#[derive(Copy, Clone, Debug, PartialEq)]
enum Color {
    /// sRGB channels 0..=255 and alpha 0..=1.
    Rgb(f64, f64, f64, f64),
    /// Already-normalized Oklab/Oklch components and alpha.
    Oklab(f64, f64, f64, f64),
    Oklch(f64, f64, f64, f64),
    CurrentColor,
}

/// The computed color for `value`; `current` supplies `currentcolor`.
pub fn serialize_color(value: &str, current: &mut dyn FnMut() -> Option<String>) -> Option<String> {
    let color = parse_color(value.trim())?;
    Some(match color {
        Color::CurrentColor => return current(),
        Color::Rgb(r, g, b, a) => serialize_rgb(r, g, b, a),
        Color::Oklab(l, a, b, alpha) => {
            format!("oklab({} {} {}{})", fmt_num(l), fmt_num(a), fmt_num(b), alpha_suffix(alpha))
        }
        Color::Oklch(l, c, h, alpha) => {
            format!("oklch({} {} {}{})", fmt_num(l), fmt_num(c), fmt_num(h), alpha_suffix(alpha))
        }
    })
}

fn alpha_suffix(alpha: f64) -> String {
    if alpha >= 1.0 {
        String::new()
    } else {
        format!(" / {}", fmt_num(alpha))
    }
}

fn serialize_rgb(r: f64, g: f64, b: f64, a: f64) -> String {
    let c = |v: f64| v.round().clamp(0.0, 255.0) as u8;
    if a >= 1.0 {
        format!("rgb({}, {}, {})", c(r), c(g), c(b))
    } else {
        format!("rgba({}, {}, {}, {})", c(r), c(g), c(b), serialize_alpha(a))
    }
}

/// Chromium stores alpha in 8 bits and prints the shortest decimal that
/// round-trips (two places when possible, else three).
fn serialize_alpha(a: f64) -> String {
    let a8 = (a.clamp(0.0, 1.0) * 255.0).round();
    let two = (a8 / 255.0 * 100.0).round() / 100.0;
    let value = if (two * 255.0).round() == a8 { two } else { (a8 / 255.0 * 1000.0).round() / 1000.0 };
    fmt_num(value)
}

fn parse_color(value: &str) -> Option<Color> {
    let lower = value.to_ascii_lowercase();
    if lower == "currentcolor" {
        return Some(Color::CurrentColor);
    }
    if lower == "transparent" {
        return Some(Color::Rgb(0.0, 0.0, 0.0, 0.0));
    }
    if let Some(hex) = lower.strip_prefix('#') {
        return parse_hex(hex);
    }
    if let Some(rgb) = named_color(&lower) {
        return Some(Color::Rgb(((rgb >> 16) & 255) as f64, ((rgb >> 8) & 255) as f64, (rgb & 255) as f64, 1.0));
    }
    let open = lower.find('(')?;
    if !lower.ends_with(')') {
        return None;
    }
    let func = &lower[..open];
    let args = &lower[open + 1..lower.len() - 1];
    match func {
        "rgb" | "rgba" => parse_rgb_args(args),
        "hsl" | "hsla" => parse_hsl_args(args),
        "oklch" => {
            let (c, alpha) = component_args(args)?;
            let [l, ch, h] = c.as_slice() else { return None };
            Some(Color::Oklch(unit_or_percent(l, 1.0)?, unit_or_percent(ch, 0.4)?, hue(h)?, alpha))
        }
        "oklab" => {
            let (c, alpha) = component_args(args)?;
            let [l, a, b] = c.as_slice() else { return None };
            Some(Color::Oklab(unit_or_percent(l, 1.0)?, unit_or_percent(a, 0.4)?, unit_or_percent(b, 0.4)?, alpha))
        }
        "color-mix" => parse_color_mix(args),
        _ => None,
    }
}

fn parse_hex(hex: &str) -> Option<Color> {
    let digit = |i: usize, n: usize| u8::from_str_radix(&hex[i..i + n], 16).ok().map(|v| v as f64);
    let short = |i: usize| digit(i, 1).map(|v| v * 17.0);
    match hex.len() {
        3 => Some(Color::Rgb(short(0)?, short(1)?, short(2)?, 1.0)),
        4 => Some(Color::Rgb(short(0)?, short(1)?, short(2)?, short(3)? / 255.0)),
        6 => Some(Color::Rgb(digit(0, 2)?, digit(2, 2)?, digit(4, 2)?, 1.0)),
        8 => Some(Color::Rgb(digit(0, 2)?, digit(2, 2)?, digit(4, 2)?, digit(6, 2)? / 255.0)),
        _ => None,
    }
}

/// Space- or comma-separated components with an optional `/ alpha`.
fn component_args(args: &str) -> Option<(Vec<String>, f64)> {
    let (main, alpha) = match args.split_once('/') {
        Some((m, a)) => (m, Some(a.trim())),
        None => (args, None),
    };
    let parts: Vec<String> = main
        .split(|c: char| c == ',' || c.is_ascii_whitespace())
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    let alpha = match alpha {
        Some(a) => unit_or_percent(a, 1.0)?.clamp(0.0, 1.0),
        None => 1.0,
    };
    Some((parts, alpha))
}

/// A number, or a percentage of `full`.
fn unit_or_percent(s: &str, full: f64) -> Option<f64> {
    if s == "none" {
        return Some(0.0);
    }
    match s.strip_suffix('%') {
        Some(p) => p.parse::<f64>().ok().map(|v| v / 100.0 * full),
        None => s.parse::<f64>().ok(),
    }
}

fn hue(s: &str) -> Option<f64> {
    if s == "none" {
        return Some(0.0);
    }
    let (v, scale) = if let Some(v) = s.strip_suffix("deg") {
        (v, 1.0)
    } else if let Some(v) = s.strip_suffix("grad") {
        (v, 0.9)
    } else if let Some(v) = s.strip_suffix("rad") {
        (v, 180.0 / std::f64::consts::PI)
    } else if let Some(v) = s.strip_suffix("turn") {
        (v, 360.0)
    } else {
        (s, 1.0)
    };
    v.parse::<f64>().ok().map(|v| v * scale)
}

fn parse_rgb_args(args: &str) -> Option<Color> {
    let (parts, alpha) = component_args(args)?;
    let (rgb, alpha) = match parts.as_slice() {
        [r, g, b] => ([r, g, b], alpha),
        // Legacy `rgba(r, g, b, a)`.
        [r, g, b, a] if !args.contains('/') => ([r, g, b], unit_or_percent(a, 1.0)?.clamp(0.0, 1.0)),
        _ => return None,
    };
    let channel = |s: &String| unit_or_percent(s, 255.0);
    Some(Color::Rgb(channel(rgb[0])?, channel(rgb[1])?, channel(rgb[2])?, alpha))
}

fn parse_hsl_args(args: &str) -> Option<Color> {
    let (parts, alpha) = component_args(args)?;
    let (h, s, l, alpha) = match parts.as_slice() {
        [h, s, l] => (h, s, l, alpha),
        [h, s, l, a] if !args.contains('/') => (h, s, l, unit_or_percent(a, 1.0)?.clamp(0.0, 1.0)),
        _ => return None,
    };
    let h = hue(h)?.rem_euclid(360.0) / 360.0;
    let s = unit_or_percent(s, 1.0)?.clamp(0.0, 1.0);
    let l = unit_or_percent(l, 1.0)?.clamp(0.0, 1.0);
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let channel = |t: f64| {
        let t = t.rem_euclid(1.0);
        let v = if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        };
        v * 255.0
    };
    Some(Color::Rgb(channel(h + 1.0 / 3.0), channel(h), channel(h - 1.0 / 3.0), alpha))
}

/// `color-mix(in <space>, C [p%], transparent [q%])` as Tailwind emits for
/// opacity modifiers. Mixing with transparent keeps the color and scales
/// alpha (premultiplied interpolation); other mixes are not computed.
fn parse_color_mix(args: &str) -> Option<Color> {
    let (space, rest) = split_top_level_comma(args)?;
    let space = space.trim().strip_prefix("in ")?.trim();
    let (first, second) = split_top_level_comma(rest)?;
    let operand = |s: &str| -> Option<(Color, Option<f64>)> {
        let parts = top_level_parts(s.trim());
        match parts.as_slice() {
            [c] => Some((parse_color(c)?, None)),
            [c, p] if p.ends_with('%') => Some((parse_color(c)?, Some(unit_or_percent(p, 1.0)?))),
            [p, c] if p.ends_with('%') => Some((parse_color(c)?, Some(unit_or_percent(p, 1.0)?))),
            _ => None,
        }
    };
    let (a, pa) = operand(first)?;
    let (b, pb) = operand(second)?;
    let transparent = Color::Rgb(0.0, 0.0, 0.0, 0.0);
    let (color, weight) = match (a == transparent, b == transparent) {
        (false, true) => (a, pa.or(pb.map(|q| 1.0 - q)).unwrap_or(0.5)),
        (true, false) => (b, pb.or(pa.map(|q| 1.0 - q)).unwrap_or(0.5)),
        _ => return None,
    };
    let scaled = |alpha: f64| (alpha * weight).clamp(0.0, 1.0);
    Some(match (space, color) {
        ("oklab", Color::Oklab(l, a, b, alpha)) => Color::Oklab(l, a, b, scaled(alpha)),
        ("oklab", Color::Oklch(l, c, h, alpha)) => {
            let rad = h.to_radians();
            Color::Oklab(l, c * rad.cos(), c * rad.sin(), scaled(alpha))
        }
        ("oklab", Color::Rgb(r, g, b, alpha)) => {
            let (l, a, b) = srgb_to_oklab(r, g, b);
            Color::Oklab(l, a, b, scaled(alpha))
        }
        ("oklch", Color::Oklch(l, c, h, alpha)) => Color::Oklch(l, c, h, scaled(alpha)),
        ("srgb", Color::Rgb(r, g, b, alpha)) => Color::Rgb(r, g, b, scaled(alpha)),
        _ => return None,
    })
}

fn srgb_to_oklab(r: f64, g: f64, b: f64) -> (f64, f64, f64) {
    let lin = |c: f64| {
        let c = c / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let (r, g, b) = (lin(r), lin(g), lin(b));
    let l = (0.412_221_470_8 * r + 0.536_332_536_3 * g + 0.051_445_992_9 * b).cbrt();
    let m = (0.211_903_498_2 * r + 0.680_699_545_1 * g + 0.107_396_956_6 * b).cbrt();
    let s = (0.088_302_461_9 * r + 0.281_718_837_6 * g + 0.629_978_700_5 * b).cbrt();
    (
        0.210_454_255_3 * l + 0.793_617_785 * m - 0.004_072_046_8 * s,
        1.977_998_495_1 * l - 2.428_592_205 * m + 0.450_593_709_9 * s,
        0.025_904_037_1 * l + 0.782_771_766_2 * m - 0.808_675_766 * s,
    )
}

fn named_color(name: &str) -> Option<u32> {
    NAMED_COLORS.binary_search_by(|(n, _)| n.cmp(&name)).ok().map(|i| NAMED_COLORS[i].1)
}

/// CSS Color 4 named colors, sorted for binary search.
const NAMED_COLORS: &[(&str, u32)] = &[
    ("aliceblue", 0xf0f8ff),
    ("antiquewhite", 0xfaebd7),
    ("aqua", 0x00ffff),
    ("aquamarine", 0x7fffd4),
    ("azure", 0xf0ffff),
    ("beige", 0xf5f5dc),
    ("bisque", 0xffe4c4),
    ("black", 0x000000),
    ("blanchedalmond", 0xffebcd),
    ("blue", 0x0000ff),
    ("blueviolet", 0x8a2be2),
    ("brown", 0xa52a2a),
    ("burlywood", 0xdeb887),
    ("cadetblue", 0x5f9ea0),
    ("chartreuse", 0x7fff00),
    ("chocolate", 0xd2691e),
    ("coral", 0xff7f50),
    ("cornflowerblue", 0x6495ed),
    ("cornsilk", 0xfff8dc),
    ("crimson", 0xdc143c),
    ("cyan", 0x00ffff),
    ("darkblue", 0x00008b),
    ("darkcyan", 0x008b8b),
    ("darkgoldenrod", 0xb8860b),
    ("darkgray", 0xa9a9a9),
    ("darkgreen", 0x006400),
    ("darkgrey", 0xa9a9a9),
    ("darkkhaki", 0xbdb76b),
    ("darkmagenta", 0x8b008b),
    ("darkolivegreen", 0x556b2f),
    ("darkorange", 0xff8c00),
    ("darkorchid", 0x9932cc),
    ("darkred", 0x8b0000),
    ("darksalmon", 0xe9967a),
    ("darkseagreen", 0x8fbc8f),
    ("darkslateblue", 0x483d8b),
    ("darkslategray", 0x2f4f4f),
    ("darkslategrey", 0x2f4f4f),
    ("darkturquoise", 0x00ced1),
    ("darkviolet", 0x9400d3),
    ("deeppink", 0xff1493),
    ("deepskyblue", 0x00bfff),
    ("dimgray", 0x696969),
    ("dimgrey", 0x696969),
    ("dodgerblue", 0x1e90ff),
    ("firebrick", 0xb22222),
    ("floralwhite", 0xfffaf0),
    ("forestgreen", 0x228b22),
    ("fuchsia", 0xff00ff),
    ("gainsboro", 0xdcdcdc),
    ("ghostwhite", 0xf8f8ff),
    ("gold", 0xffd700),
    ("goldenrod", 0xdaa520),
    ("gray", 0x808080),
    ("green", 0x008000),
    ("greenyellow", 0xadff2f),
    ("grey", 0x808080),
    ("honeydew", 0xf0fff0),
    ("hotpink", 0xff69b4),
    ("indianred", 0xcd5c5c),
    ("indigo", 0x4b0082),
    ("ivory", 0xfffff0),
    ("khaki", 0xf0e68c),
    ("lavender", 0xe6e6fa),
    ("lavenderblush", 0xfff0f5),
    ("lawngreen", 0x7cfc00),
    ("lemonchiffon", 0xfffacd),
    ("lightblue", 0xadd8e6),
    ("lightcoral", 0xf08080),
    ("lightcyan", 0xe0ffff),
    ("lightgoldenrodyellow", 0xfafad2),
    ("lightgray", 0xd3d3d3),
    ("lightgreen", 0x90ee90),
    ("lightgrey", 0xd3d3d3),
    ("lightpink", 0xffb6c1),
    ("lightsalmon", 0xffa07a),
    ("lightseagreen", 0x20b2aa),
    ("lightskyblue", 0x87cefa),
    ("lightslategray", 0x778899),
    ("lightslategrey", 0x778899),
    ("lightsteelblue", 0xb0c4de),
    ("lightyellow", 0xffffe0),
    ("lime", 0x00ff00),
    ("limegreen", 0x32cd32),
    ("linen", 0xfaf0e6),
    ("magenta", 0xff00ff),
    ("maroon", 0x800000),
    ("mediumaquamarine", 0x66cdaa),
    ("mediumblue", 0x0000cd),
    ("mediumorchid", 0xba55d3),
    ("mediumpurple", 0x9370db),
    ("mediumseagreen", 0x3cb371),
    ("mediumslateblue", 0x7b68ee),
    ("mediumspringgreen", 0x00fa9a),
    ("mediumturquoise", 0x48d1cc),
    ("mediumvioletred", 0xc71585),
    ("midnightblue", 0x191970),
    ("mintcream", 0xf5fffa),
    ("mistyrose", 0xffe4e1),
    ("moccasin", 0xffe4b5),
    ("navajowhite", 0xffdead),
    ("navy", 0x000080),
    ("oldlace", 0xfdf5e6),
    ("olive", 0x808000),
    ("olivedrab", 0x6b8e23),
    ("orange", 0xffa500),
    ("orangered", 0xff4500),
    ("orchid", 0xda70d6),
    ("palegoldenrod", 0xeee8aa),
    ("palegreen", 0x98fb98),
    ("paleturquoise", 0xafeeee),
    ("palevioletred", 0xdb7093),
    ("papayawhip", 0xffefd5),
    ("peachpuff", 0xffdab9),
    ("peru", 0xcd853f),
    ("pink", 0xffc0cb),
    ("plum", 0xdda0dd),
    ("powderblue", 0xb0e0e6),
    ("purple", 0x800080),
    ("rebeccapurple", 0x663399),
    ("red", 0xff0000),
    ("rosybrown", 0xbc8f8f),
    ("royalblue", 0x4169e1),
    ("saddlebrown", 0x8b4513),
    ("salmon", 0xfa8072),
    ("sandybrown", 0xf4a460),
    ("seagreen", 0x2e8b57),
    ("seashell", 0xfff5ee),
    ("sienna", 0xa0522d),
    ("silver", 0xc0c0c0),
    ("skyblue", 0x87ceeb),
    ("slateblue", 0x6a5acd),
    ("slategray", 0x708090),
    ("slategrey", 0x708090),
    ("snow", 0xfffafa),
    ("springgreen", 0x00ff7f),
    ("steelblue", 0x4682b4),
    ("tan", 0xd2b48c),
    ("teal", 0x008080),
    ("thistle", 0xd8bfd8),
    ("tomato", 0xff6347),
    ("turquoise", 0x40e0d0),
    ("violet", 0xee82ee),
    ("wheat", 0xf5deb3),
    ("white", 0xffffff),
    ("whitesmoke", 0xf5f5f5),
    ("yellow", 0xffff00),
    ("yellowgreen", 0x9acd32),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn color(v: &str) -> Option<String> {
        serialize_color(v, &mut || Some("rgb(1, 2, 3)".into()))
    }

    #[test]
    fn named_colors_are_sorted() {
        assert!(NAMED_COLORS.windows(2).all(|w| w[0].0 < w[1].0));
    }

    #[test]
    fn colors_serialize_like_chromium() {
        assert_eq!(color("#7c3aed").as_deref(), Some("rgb(124, 58, 237)"));
        assert_eq!(color("#FFF").as_deref(), Some("rgb(255, 255, 255)"));
        assert_eq!(color("#ffffff80").as_deref(), Some("rgba(255, 255, 255, 0.5)"));
        assert_eq!(color("white").as_deref(), Some("rgb(255, 255, 255)"));
        assert_eq!(color("transparent").as_deref(), Some("rgba(0, 0, 0, 0)"));
        assert_eq!(color("rgb(1 2 3 / 50%)").as_deref(), Some("rgba(1, 2, 3, 0.5)"));
        assert_eq!(color("rgba(10, 20, 30, 0.25)").as_deref(), Some("rgba(10, 20, 30, 0.25)"));
        assert_eq!(color("hsl(0 100% 50%)").as_deref(), Some("rgb(255, 0, 0)"));
        assert_eq!(color("currentColor").as_deref(), Some("rgb(1, 2, 3)"));
        assert_eq!(color("oklch(54.1% 0.281 293.009)").as_deref(), Some("oklch(0.541 0.281 293.009)"));
        assert_eq!(color("oklch(0.5 0.1 20 / 0.5)").as_deref(), Some("oklch(0.5 0.1 20 / 0.5)"));
        assert_eq!(
            color("color-mix(in oklab, oklch(0.5 0 0) 50%, transparent)").as_deref(),
            Some("oklab(0.5 0 0 / 0.5)")
        );
        assert_eq!(color("not-a-color"), None);
    }

    #[test]
    fn lengths_and_calc_compute_to_px() {
        assert_eq!(resolve_length("calc(0.25rem * 2)", 16.0, 16.0).as_deref(), Some("8px"));
        assert_eq!(resolve_length("calc(0.25rem * 3.5)", 16.0, 16.0).as_deref(), Some("14px"));
        assert_eq!(resolve_length("1.5em", 10.0, 16.0).as_deref(), Some("15px"));
        assert_eq!(resolve_length("calc(100% - 2px)", 16.0, 16.0), None);
        assert_eq!(resolve_length("auto", 16.0, 16.0), None);
        assert_eq!(resolve_length("0", 16.0, 16.0).as_deref(), Some("0px"));
        assert_eq!(resolve_length("calc((1px + 2px) * 2)", 16.0, 16.0).as_deref(), Some("6px"));
        assert_eq!(resolve_length("13.333333px", 16.0, 16.0).as_deref(), Some("13.3333px"));
        assert_eq!(resolve_number("calc(1 + 1)"), Some(2.0));
        let cx =
            LengthContext { font_size: 10.0, root_font_size: 14.0, viewport_width: 1440.0, viewport_height: 900.0 };
        assert_eq!(resolve_len("calc(100% - 8px)", &cx), Some((-8.0, 100.0)));
        assert_eq!(resolve_len("50vw", &cx), Some((720.0, 0.0)));
        assert_eq!(resolve_len("2rem", &cx), Some((28.0, 0.0)));
        assert_eq!(resolve_len("auto", &cx), None);
    }

    #[test]
    fn var_substitution_uses_fallbacks_and_rejects_missing() {
        let mut lookup = |n: &str| match n {
            "--spacing" => Some("0.25rem".to_owned()),
            "--nested" => Some("var(--spacing)".to_owned()),
            _ => None,
        };
        assert_eq!(substitute_vars("calc(var(--spacing) * 2)", &mut lookup, 0).as_deref(), Some("calc(0.25rem * 2)"));
        assert_eq!(substitute_vars("var(--nested)", &mut lookup, 0).as_deref(), Some("0.25rem"));
        assert_eq!(substitute_vars("var(--missing, red)", &mut lookup, 0).as_deref(), Some("red"));
        assert_eq!(substitute_vars("var(--missing)", &mut lookup, 0), None);
    }

    #[test]
    fn shorthands_expand_to_longhands() {
        assert_eq!(
            expand("flex", "1"),
            vec![
                ("flex-grow".into(), "1".into()),
                ("flex-shrink".into(), "1".into()),
                ("flex-basis".into(), "0%".into())
            ]
        );
        assert_eq!(expand("overflow", "hidden auto")[1], ("overflow-y".into(), "auto".into()));
        assert_eq!(expand("gap", "calc(var(--spacing) * 2)")[1].0, "column-gap");
        assert_eq!(expand("margin", "1px 2px")[3], ("margin-left".into(), "2px".into()));
        assert_eq!(expand("background", "#fff"), vec![("background-color".into(), "#fff".into())]);
        assert!(expand("background", "url(x.png) no-repeat").is_empty());
    }
}
