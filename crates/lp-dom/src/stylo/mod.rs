//! Stylo (Servo's style system) over lp-dom nodes, for selector matching
//! (the DOM's query APIs, [`select`]) and for reading computed values
//! ([`serialize`]). Computed styles themselves come from the layout
//! engine's cascade (Blitz), whose `ComputedValues` are Stylo's too.

mod hints;
mod node;
pub mod select;

use std::sync::LazyLock;

use style::context::QuirksMode;
use style::properties::ComputedValues;
use style::servo_arc::Arc;
use style::stylesheets::UrlExtraData;
use style::values::computed::CSSPixelLength;

/// The property `name` names, if Stylo knows it (parsed once per name).
pub(crate) fn property_id(name: &str) -> Option<style::properties::PropertyId> {
    use style::properties::PropertyId;
    thread_local! {
        /// Property names as parsed (the same few dozen, read per element).
        static IDS: std::cell::RefCell<rustc_hash::FxHashMap<Box<str>, Option<PropertyId>>> = Default::default();
    }
    IDS.with(|ids| {
        if let Some(id) = ids.borrow().get(name) {
            return id.clone();
        }
        let id = PropertyId::parse_enabled_for_all_content(name).ok();
        let mut ids = ids.borrow_mut();
        if ids.len() < 4096 {
            ids.insert(name.into(), id.clone());
        }
        id
    })
}

/// A property's computed value as `getComputedStyle` serializes it: a
/// longhand directly, a shorthand when all its longhands serialize alike, a
/// custom property as specified. `None` for unknown properties.
pub fn serialize(style: &ComputedValues, name: &str) -> Option<String> {
    use style::properties::PropertyDeclarationId;
    let id = property_id(name)?;
    match id.as_shorthand() {
        Ok(shorthand) => {
            // The value all its longhands share...
            let values: Vec<(&str, String)> = shorthand
                .longhands()
                .map(|l| (l.name(), style.computed_value_to_string(PropertyDeclarationId::Longhand(l))))
                .collect();
            if values.iter().all(|(_, v)| *v == values[0].1) {
                return values.into_iter().next().map(|(_, v)| v);
            }
            // ...else the shorthand serialization of a block declaring the
            // longhands' resolved values (CSSOM's).
            let mut css = String::new();
            for (name, value) in &values {
                css.push_str(name);
                css.push(':');
                css.push_str(value);
                css.push(';');
            }
            let url = url_data();
            let block = style::properties::parse_style_attribute(
                &css,
                &url,
                None,
                QuirksMode::NoQuirks,
                style::stylesheets::CssRuleType::Style,
            );
            let mut out = String::new();
            block.shorthand_to_css(shorthand, &mut out).ok()?;
            (!out.is_empty()).then_some(out)
        }
        Err(declaration) => {
            // Stylo resolves every length unzoomed but a line-height length.
            if name.eq_ignore_ascii_case("line-height") {
                if let style::values::generics::font::GenericLineHeight::Length(l) = style.get_font().line_height {
                    let mut out = String::new();
                    let px = style.effective_zoom.unzoom(l.0.px());
                    style_traits::ToCss::to_css(&CSSPixelLength::new(px), &mut style_traits::CssWriter::new(&mut out))
                        .ok()?;
                    return Some(out);
                }
            }
            let value = style.computed_value_to_string(declaration);
            // Gecko's prefixed keywords, as Chrome spells them.
            Some(match value.strip_prefix("-moz-") {
                Some(rest @ ("center" | "left" | "right")) if name.eq_ignore_ascii_case("text-align") => {
                    format!("-webkit-{rest}")
                }
                _ => value,
            })
        }
    }
}

/// The URL style values resolve against (computed styles keep relative
/// `url()`s as written; the loader resolves them).
pub(crate) fn url_data() -> UrlExtraData {
    static URL: LazyLock<UrlExtraData> =
        LazyLock::new(|| UrlExtraData(Arc::new(url::Url::parse("about:blank").expect("url"))));
    URL.clone()
}

/// The longhand properties a computed style lists (`getComputedStyle(el)`'s
/// indexed properties), in alphabetical order.
pub fn longhand_names() -> &'static [&'static str] {
    static NAMES: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
        use style::properties::ShorthandId;
        // `all` covers every longhand but these two.
        let mut names: Vec<&'static str> = ShorthandId::All.longhands().map(|l| l.name()).collect();
        names.extend(["direction", "unicode-bidi"]);
        names.retain(|name| property_id(name).is_some());
        names.sort_unstable();
        names.dedup();
        names
    });
    &NAMES
}

/// Whether `name` is a CSS property (longhand or shorthand) the engine knows.
pub fn is_property(name: &str) -> bool {
    property_id(name).is_some()
}

/// Turns on the Stylo features the page's cascade parses with (Blitz sets the
/// same process-wide prefs when it makes its first document): `CSS.supports`
/// can run before any document was laid out.
fn ensure_parse_prefs() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        style_config::set_pref!("layout.grid.enabled", true);
        style_config::set_pref!("layout.css.has-selector.enabled", true);
        style_config::set_pref!("layout.unimplemented", true);
        style_config::set_pref!("layout.columns.enabled", true);
        style_config::set_pref!("layout.css.basic-shape-shape.enabled", true);
        style_config::set_pref!("layout.css.tree-counting-functions.enabled", true);
        style_config::set_pref!("layout.css.progress-function.enabled", true);
        style_config::set_pref!("layout.variable_fonts.enabled", true);
        style_config::set_pref!("layout.container-queries.enabled", true);
        style_config::set_pref!("layout.css.nth-child-of.enabled", true);
        style_config::set_pref!("layout.css.starting-style-at-rules.enabled", true);
        style_config::set_pref!("layout.css.at-scope.enabled", true);
        style_config::set_pref!("layout.css.style-queries.enabled", true);
        style_config::set_pref!("layout.css.line-clamp.enabled", true);
        style_config::set_pref!("layout.css.fit-content-function.enabled", true);
        style_config::set_pref!("layout.css.supports.at-rule.enabled", true);
    });
}

/// `CSS.supports(property, value)` (CSS Conditional 3): whether `property`
/// names a property the engine supports (ASCII case-insensitively; a custom
/// property as written) and `value` parses as that property's value in an
/// author style rule. The few properties Stylo's servo build does not parse
/// but lp-dom computes ([`crate::gecko_only`]) count as supported.
pub fn supports_declaration(property: &str, value: &str) -> bool {
    use style::properties::{parse_one_declaration_into, SourcePropertyDeclaration};
    use style::stylesheets::{CssRuleType, Origin};
    ensure_parse_prefs();
    let Ok(id) = style::properties::PropertyId::parse_enabled_for_all_content(property) else {
        let lower = property.to_ascii_lowercase();
        return crate::gecko_only::handles(&lower) && !value.trim().is_empty();
    };
    let mut declarations = SourcePropertyDeclaration::default();
    parse_one_declaration_into(
        &mut declarations,
        id,
        value,
        Origin::Author,
        &url_data(),
        None,
        style_traits::ParsingMode::DEFAULT,
        QuirksMode::NoQuirks,
        CssRuleType::Style,
    )
    .is_ok()
}

/// `CSS.supports(conditionText)`: `text` parsed as a `<supports-condition>`,
/// else as one declaration in parentheses, and evaluated as `@supports`
/// would be (declarations, `selector()`, `not` / `and` / `or`; unknown
/// syntax is false).
pub fn supports_condition(text: &str) -> bool {
    use style::stylesheets::supports_rule::SupportsCondition;
    use style::stylesheets::{CssRuleType, Origin};
    ensure_parse_prefs();
    let url = url_data();
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
    let parse = |text: &str| {
        let mut parser = cssparser::Parser::new(text);
        parser.parse_entirely(|p| SupportsCondition::parse(p)).ok()
    };
    match parse(text) {
        Some(condition) => condition.eval(&context),
        None => parse(&format!("({text})")).is_some_and(|condition| condition.eval(&context)),
    }
}
