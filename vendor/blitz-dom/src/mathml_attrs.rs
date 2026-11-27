//! pyro: the MathML attributes that map to CSS (MathML Core § 2.1.3 and § 3.1, "legacy MathML style
//! attributes"), as presentational hints, so author styles override them: `mathcolor` to `color`,
//! `mathbackground` to `background-color`, `mathsize` to `font-size` and `scriptlevel` to `math-depth`.
//! (`displaystyle` is mapped in `stylo.rs`, `dir` and `mathvariant` by the UA sheet's attribute
//! selectors.) They apply to every MathML element. The deprecated MathML 3 attributes (`color`,
//! `background`, `fontsize`, ...) map to nothing.

use markup5ever::ns;
use style::properties::{PropertyDeclaration, PropertyId, SourcePropertyDeclaration, parse_one_declaration_into};
use style::stylesheets::{CssRuleType, Origin};
use style_traits::ParsingMode;

use crate::node::ElementData;

/// The declarations the MathML style attributes of `elem` map to (none for a non-MathML element or
/// an invalid value).
pub(crate) fn hints(elem: &ElementData) -> Vec<PropertyDeclaration> {
    if elem.name.ns != ns!(mathml) {
        return Vec::new();
    }
    let attr = |name: &str| elem.attrs().iter().find(|a| a.name.ns == ns!() && &*a.name.local == name).map(|a| &*a.value);
    let mut declarations = Vec::new();
    // A `<color>`; a value that is not one (or `inherit` and the like) leaves the property alone.
    if let Some(value) = attr("mathcolor") {
        declarations.extend(parse("color", value.trim()));
    }
    if let Some(value) = attr("mathbackground") {
        declarations.extend(parse("background-color", value.trim()));
    }
    // A non-negative `<length-percentage>`: not a font-size keyword (`large`, `smaller`) nor a
    // legacy MathML 3 value (`small`, `big`), which have no effect (Core 3.1).
    if let Some(value) = attr("mathsize").map(str::trim)
        && is_length_percentage(value)
    {
        declarations.extend(parse("font-size", value));
    }
    if let Some(math_depth) = attr("scriptlevel").and_then(scriptlevel) {
        declarations.extend(parse("math-depth", &math_depth));
    }
    declarations
}

/// The `math-depth` value for a `scriptlevel` attribute: `+<n>` and `-<n>` add to the inherited
/// depth, a bare `<n>` sets it (MathML Core § 3.1: an optional sign and ASCII digits only; anything
/// else, including surrounding spaces and fractions, is invalid).
fn scriptlevel(value: &str) -> Option<String> {
    let (sign, digits) = match value.as_bytes().first()? {
        b'+' => (Some(1), &value[1..]),
        b'-' => (Some(-1), &value[1..]),
        _ => (None, value),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let magnitude: i32 = digits.parse().ok()?;
    Some(match sign {
        Some(sign) => format!("add({})", sign * magnitude),
        None => magnitude.to_string(),
    })
}

/// Whether `value` is a non-negative CSS `<length-percentage>`.
fn is_length_percentage(value: &str) -> bool {
    use style::parser::ParserContext;
    use style::values::specified::LengthPercentage;

    let url_data = crate::url::DocumentUrl::default().url_extra_data();
    let context = ParserContext::new(
        Origin::Author,
        &url_data,
        Some(CssRuleType::Style),
        ParsingMode::DEFAULT,
        style::context::QuirksMode::NoQuirks,
        Default::default(),
        None,
        None,
        Default::default(),
    );
    let mut parser = cssparser::Parser::new(value);
    parser.parse_entirely(|input| LengthPercentage::parse_non_negative(&context, input)).is_ok()
}

/// `value` parsed as the CSS `property`'s value: its declarations, or none.
fn parse(property: &str, value: &str) -> Vec<PropertyDeclaration> {
    let Ok(property) = PropertyId::parse_enabled_for_all_content(property) else {
        return Vec::new();
    };
    // Only a value of its own: no CSS-wide keyword, no `var()`.
    let lower = value.to_ascii_lowercase();
    if matches!(lower.as_str(), "inherit" | "initial" | "unset" | "revert" | "revert-layer") || lower.contains("var(") {
        return Vec::new();
    }
    let mut declarations = SourcePropertyDeclaration::default();
    let parsed = parse_one_declaration_into(
        &mut declarations,
        property,
        value,
        Origin::Author,
        &crate::url::DocumentUrl::default().url_extra_data(),
        None,
        ParsingMode::DEFAULT,
        style::context::QuirksMode::NoQuirks,
        CssRuleType::Style,
    );
    match parsed {
        Ok(()) => declarations.drain().declarations.collect(),
        Err(_) => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::scriptlevel;

    #[test]
    fn scriptlevel_values() {
        assert_eq!(scriptlevel("2").as_deref(), Some("2"));
        assert_eq!(scriptlevel("+1").as_deref(), Some("add(1)"));
        assert_eq!(scriptlevel("-3").as_deref(), Some("add(-3)"));
        for invalid in ["", "+", "+-1", "--1", "+z1", "+ 1", "2.0", "-3\"", "200px", "add(2)", " 1"] {
            assert_eq!(scriptlevel(invalid), None, "{invalid:?}");
        }
    }
}
