//! CSSOM declaration blocks (`CSSStyleDeclaration` of `element.style` and
//! of style rules) over Stylo's `PropertyDeclarationBlock`: values are
//! parsed and validated as the cascade parses them, shorthands expand into
//! their longhands, and values and blocks serialize as CSSOM says.

use style::context::QuirksMode;
use style::properties::{
    Importance, PropertyDeclarationBlock, PropertyId, SourcePropertyDeclaration, SourcePropertyDeclarationUpdate,
};
use style::stylesheets::{CssRuleType, Origin};
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
        let id = property_id(name)?;
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
            CssRuleType::Style,
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

/// `name: value` (with `!important`) as a block of that one declaration
/// serializes (`margin: 1px 2px;`, `color: rgb(255, 0, 0);`); `None` when
/// `name` is no property or `value` is invalid for it.
pub fn declaration_text(name: &str, value: &str, important: bool) -> Option<String> {
    let mut block = DeclarationBlock(PropertyDeclarationBlock::new());
    block.set(name, value, important)?;
    Some(block.css_text())
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

#[cfg(test)]
mod tests {
    use super::*;

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
