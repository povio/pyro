//! CSSOM declaration blocks (`CSSStyleDeclaration` of `element.style` and
//! of style rules) over Stylo's `PropertyDeclarationBlock`: values are
//! parsed and validated as the cascade parses them, shorthands expand into
//! their longhands, and values and blocks serialize as CSSOM says.

use style::context::QuirksMode;
use style::properties::{
    Importance, PropertyDeclarationBlock, PropertyId, SourcePropertyDeclaration, SourcePropertyDeclarationUpdate,
};
pub use style::stylesheets::CssRuleType;
use style::stylesheets::Origin;
use style_traits::ParsingMode;

use super::{property_id, url_data};

/// The declarations of `text` as the cascade parses a `style` attribute
/// (invalid ones dropped).
pub(crate) fn parse_block(text: &str) -> PropertyDeclarationBlock {
    super::enable_features();
    style::properties::parse_style_attribute(text, &url_data(), None, QuirksMode::NoQuirks, CssRuleType::Style)
}

/// One declaration block, parsed from its text.
pub struct DeclarationBlock(PropertyDeclarationBlock);

impl DeclarationBlock {
    pub fn parse_in_rule(text: &str, rule: CssRuleType) -> Self {
        super::enable_features();
        Self(style::properties::parse_style_attribute(text, &url_data(), None, QuirksMode::NoQuirks, rule))
    }

    pub fn keyframe(text: &str) -> Self {
        Self::parse_in_rule(text, CssRuleType::Keyframe)
    }

    pub fn page(text: &str) -> Self {
        Self::parse_in_rule(text, CssRuleType::Page)
    }

    pub fn position_try(text: &str) -> Self {
        Self::parse_in_rule(text, CssRuleType::PositionTry)
    }

    pub fn allowed_in_rule(name: &str, rule: CssRuleType) -> bool {
        property_id(name).is_some_and(|id| id.allowed_in_rule(rule.into()))
    }

    pub fn keyframe_allows(name: &str) -> bool { Self::allowed_in_rule(name, CssRuleType::Keyframe) }

    pub fn page_allows(name: &str) -> bool { Self::allowed_in_rule(name, CssRuleType::Page) }

    pub fn position_try_allows(name: &str) -> bool { Self::allowed_in_rule(name, CssRuleType::PositionTry) }

    /// The declarations of `text` (a `style` attribute or a rule's block);
    /// invalid ones are dropped.
    pub fn parse(text: &str) -> DeclarationBlock {
        if text.trim().is_empty() {
            return DeclarationBlock(PropertyDeclarationBlock::new());
        }
        super::enable_features();
        let url = url_data();
        DeclarationBlock(style::properties::parse_style_attribute(
            text,
            &url,
            None,
            QuirksMode::NoQuirks,
            CssRuleType::Style,
        ))
    }

    /// The number of declarations: longhands (a shorthand counts as each of
    /// its longhands) and custom properties.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The property name of the `index`th declaration.
    pub fn item(&self, index: usize) -> Option<String> {
        self.0.declarations().get(index).map(|d| d.id().name().into_owned())
    }

    /// The property names, in order.
    pub fn names(&self) -> Vec<String> {
        self.0.declarations().iter().map(|d| d.id().name().into_owned()).collect()
    }

    /// `getPropertyValue(name)`: a longhand's or custom property's value, a
    /// shorthand's when its longhands make one; `""` otherwise.
    pub fn value(&self, name: &str) -> String {
        let Some(id) = property_id(name) else { return String::new() };
        let mut out = String::new();
        if self.0.property_value_to_css(&id, &mut out).is_err() {
            out.clear();
        }
        out
    }

    /// `getPropertyPriority(name) == "important"`.
    pub fn important(&self, name: &str) -> bool {
        property_id(name).is_some_and(|id| self.0.property_priority(&id).important())
    }

    /// `setProperty(name, value, important)` with a non-empty `value`.
    /// `None`: `name` is no property or `value` is invalid for it, and the
    /// block is unchanged; else whether the block changed.
    pub fn set(&mut self, name: &str, value: &str, important: bool) -> Option<bool> {
        self.set_in_rule(name, value, important, CssRuleType::Style)
    }

    pub fn set_page(&mut self, name: &str, value: &str, important: bool) -> Option<bool> {
        self.set_in_rule(name, value, important, CssRuleType::Page)
    }

    pub fn set_in_rule(&mut self, name: &str, value: &str, important: bool, rule: CssRuleType) -> Option<bool> {
        let id = property_id(name)?;
        if !id.allowed_in_rule(rule.into()) { return None; }
        super::enable_features();
        let mut source = SourcePropertyDeclaration::default();
        let url = url_data();
        style::properties::parse_one_declaration_into(
            &mut source,
            id,
            value,
            Origin::Author,
            &url,
            None,
            ParsingMode::DEFAULT,
            QuirksMode::NoQuirks,
            rule,
        )
        .ok()?;
        let importance = if important { Importance::Important } else { Importance::Normal };
        let mut updates = SourcePropertyDeclarationUpdate::default();
        if !self.0.prepare_for_update(&source, importance, &mut updates) {
            return Some(false);
        }
        self.0.update(source.drain(), importance, &mut updates);
        Some(true)
    }

    /// `removeProperty(name)`: whether a declaration was removed.
    pub fn remove(&mut self, name: &str) -> bool {
        let Some(id) = property_id(name) else { return false };
        match self.0.first_declaration_to_remove(&id) {
            Some(first) => {
                self.0.remove_property(&id, first);
                true
            }
            None => false,
        }
    }

    /// An alternate representation only when CSSOM shorthand or number serialization
    /// would lose a negative zero in the parsed longhands.
    pub fn css_for_reparse(&self) -> Option<String> {
        self.0.css_for_reparse().ok().flatten()
    }

    /// Text suitable for the stylesheet parser, including hidden signed-zero operands.
    pub fn source_text(&self) -> String {
        self.css_for_reparse().unwrap_or_else(|| self.css_text())
    }

    /// The block serialized (`cssText`), shorthands where the longhands
    /// make one.
    pub fn css_text(&self) -> String {
        let mut out = String::new();
        if self.0.to_css(&mut out).is_err() {
            out.clear();
        }
        out
    }
}

/// Whether raw CSS contains a numeric token already rounded to negative zero, or a
/// math function that could produce it. Strings, comments and URL payloads are opaque.
pub fn may_need_signed_zero_backing(text: &str) -> bool {
    use cssparser::{Parser, Token};
    use style::values::specified::calc::MathFunction;

    fn math(name: &str) -> bool {
        MathFunction::from_ident(name).is_ok() || name.eq_ignore_ascii_case("calc-size")
            || name.eq_ignore_ascii_case("calc-mix")
    }
    fn candidate(input: &mut Parser) -> bool {
        while let Ok(token) = input.next() {
            match token {
                Token::Number { value, .. } | Token::Dimension { value, .. }
                | Token::Percentage { unit_value: value, .. } if value.to_bits() == (-0.0_f32).to_bits() => return true,
                Token::Function(name) if math(name) => return true,
                Token::Function(_) | Token::ParenthesisBlock | Token::SquareBracketBlock | Token::CurlyBracketBlock => {
                    // An Err stops the walk as soon as a candidate is found (or the parser's depth limit is hit).
                    if input.parse_nested_block(|p| {
                        if candidate(p) { Err(cssparser::ParseError::<()>::custom(())) } else { Ok(()) }
                    }).is_err() { return true; }
                }
                _ => {}
            }
        }
        false
    }

    let bytes = text.as_bytes();
    let possible = memchr::memchr3_iter(b'(', b'-', b'\\', bytes).any(|i| match bytes[i] {
        b'-' => bytes.get(i + 1).is_some_and(|b| b.is_ascii_digit() || *b == b'.'),
        b'(' => {
            let start = bytes[..i].iter().rposition(|b| !(b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_')))
                .map_or(0, |at| at + 1);
            math(&text[start..i])
        }
        // The tokenizer decodes escaped identifiers (including their optional trailing whitespace).
        _ => true,
    });
    possible && candidate(&mut Parser::new(text))
}

/// `name: value` (with `!important`) as a block of that one declaration
/// serializes (`margin: 1px 2px;`, `color: rgb(255, 0, 0);`); `None` when
/// `name` is no property or `value` is invalid for it.
pub fn declaration_text(name: &str, value: &str, important: bool) -> Option<String> {
    let mut block = DeclarationBlock(PropertyDeclarationBlock::new());
    block.set(name, value, important)?;
    Some(block.css_text())
}

/// Whether editing this explicit longhand can change the shorthand serialization
/// or the relative order of another declaration (including logical/physical pairs).
pub fn serializes_independently<'a>(name: &str, others: impl Iterator<Item = &'a str>) -> bool {
    use style::properties::{PropertyDeclarationId as Id, ShorthandId};
    let Some(id) = property_id(name) else { return false };
    let Some(Id::Longhand(target)) = id.as_shorthand().err() else { return false };
    if target.name() != name { return false }
    let related = |other: style::properties::LonghandId| {
        other == target || (target.logical_group().is_some() && target.logical_group() == other.logical_group())
            || target.shorthands().any(|s| s != ShorthandId::All && other.is_longhand_of(s))
    };
    for other in others.filter(|&p| p != name) {
        let Some(id) = property_id(other) else { return false };
        match id.as_shorthand() {
            Ok(s) if s == ShorthandId::All || s.longhands().any(related) => return false,
            Err(Id::Longhand(id)) if related(id) => return false,
            _ => {}
        }
    }
    true
}

/// Whether properties `a` and `b` set a longhand in common (`margin` and
/// `margin-top`, `border` and `border-color`, a property and itself): in a
/// block, a declaration of one can change what the other reads.
pub fn overlaps(a: &str, b: &str) -> bool {
    use style::properties::PropertyDeclarationId as Id;
    if a == b {
        return true;
    }
    let (Some(x), Some(y)) = (property_id(a), property_id(b)) else { return false };
    match (x.as_shorthand(), y.as_shorthand()) {
        (Err(Id::Longhand(l)), Err(Id::Longhand(m))) => l == m,
        (Err(Id::Longhand(l)), Ok(s)) | (Ok(s), Err(Id::Longhand(l))) => l.is_longhand_of(s),
        (Ok(s), Ok(t)) => s == t || s.longhands().any(|l| l.is_longhand_of(t)),
        _ => false,
    }
}

/// The canonical name of the property `name` names (`float`, `--x`), if the
/// engine knows it.
pub fn property_name(name: &str) -> Option<String> {
    let id = property_id(name)?;
    Some(match id {
        PropertyId::NonCustom(id) => id.name().to_owned(),
        PropertyId::Custom(_) => name.to_owned(),
    })
}

/// What the property `name` names: its canonical name and whether it is a longhand (0), a
/// shorthand (1) or a custom property (2) (CSS Typed OM).
pub fn property_kind(name: &str) -> Option<(String, u8)> {
    let id = property_id(name)?;
    Some(match &id {
        PropertyId::NonCustom(n) => (n.name().to_owned(), if n.as_shorthand().is_some() { 1 } else { 0 }),
        PropertyId::Custom(_) => (name.to_owned(), 2),
    })
}

/// `value` parsed as the property `name` and serialized as a declaration block of it alone
/// serializes the property (CSS Typed OM `CSSStyleValue.parse`): `None` when `name` is no property
/// or `value` is invalid for it; `Some("")` when a shorthand's value has no serialization.
pub fn normalized_value(name: &str, value: &str) -> Option<String> {
    let mut block = DeclarationBlock(PropertyDeclarationBlock::new());
    block.set(name, value, false)?;
    Some(block.value(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_zero_screen_recognizes_math_and_numeric_underflow() {
        for css in [
            r#"background-image: url("calc(-0px)"); color: rgba(0,0,0,.3); transform: translate(-20px, 10px)"#,
            r#"--text: "min(-0px)"; width: var(--width, 10px); /* calc(-0px) */"#,
            "transform: translateX(1e-10px)",
            r#"transform: tr\61 nslate(20px)"#,
        ] {
            assert!(!may_need_signed_zero_backing(css), "{css}");
        }
        for css in [
            "margin: -0em", "margin: -1e-9999px", "width: -.00000000000000000000000000000000000000000000000000001%",
            "z-index: sIgN(1em - 1px)", r#"transform: translate(c\61 lc(1px / -infinity))"#,
        ] {
            assert!(may_need_signed_zero_backing(css), "{css}");
        }
    }

    #[test]
    fn shorthands_expand_and_values_are_validated_and_serialized() {
        let mut block = DeclarationBlock::parse("margin: 1px 2px; color: #f00; width: 10; --x: { a }");
        assert_eq!(block.value("margin-top"), "1px");
        assert_eq!(block.value("margin-left"), "2px");
        assert_eq!(block.value("margin"), "1px 2px");
        assert_eq!(block.value("width"), "");
        assert_eq!(block.len(), 6);
        assert_eq!(block.item(0).as_deref(), Some("margin-top"));
        assert_eq!(block.set("width", "10", false), None);
        assert_eq!(block.set("width", "10px", true), Some(true));
        assert!(block.important("width"));
        assert_eq!(block.set("nope", "1", false), None);
        assert!(block.remove("margin"));
        assert_eq!(block.value("margin-top"), "");
        let text = block.css_text();
        assert!(text.contains("width: 10px !important;"), "{text}");
        assert!(!text.contains("margin"), "{text}");
        assert_eq!(property_name("float").as_deref(), Some("float"));
        assert_eq!(property_name("-moz-appearance"), None);
    }
}
