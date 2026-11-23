//! pyro: the size of an `<mspace>` (MathML Core 3.2.4), as presentational hints.
//!
//! pyro lays no MathML out (MathML content flows as inline boxes), so an `<mspace>` is an
//! inline-block (by a user agent rule in `assets/default.css`, which the rule hiding `<maction>`'s
//! and `<semantics>`'s later children beats, where a hint would win over it) whose `width`
//! attribute sets its `width`, and whose `height` and `depth`
//! attributes set its `height` (height + depth), lowered by `depth` so that `height` lies above
//! the baseline and `depth` below it. Each attribute must be a non-negative CSS `<length>` (no
//! percentage, no unitless number): an invalid, negative or percentage value counts as absent,
//! which gives the same 0 as MathML Core's clamp. Author styles (`display`, `width`, ...) override
//! the hints, as for every presentational hint.

use markup5ever::{local_name, ns};
use style::properties::{PropertyDeclaration, PropertyId, SourcePropertyDeclaration, parse_one_declaration_into};
use style::stylesheets::{CssRuleType, Origin};
use style_traits::ParsingMode;

use crate::node::ElementData;

/// The declarations an `<mspace>`'s attributes map to, or none for any other element.
pub(crate) fn hints(elem: &ElementData) -> Vec<PropertyDeclaration> {
    if elem.name.ns != ns!(mathml) || elem.name.local != local_name!("mspace") {
        return Vec::new();
    }
    let length = |name: markup5ever::LocalName| elem.attr(name).map(str::trim).filter(|value| is_length(value));
    let width = length(local_name!("width"));
    let height = length(local_name!("height"));
    let depth = length(local_name!("depth"));

    let mut declarations = Vec::new();
    if let Some(width) = width {
        declarations.extend(parse("width", width));
    }
    let block_size = match (height, depth) {
        (Some(height), Some(depth)) => Some(format!("calc({height} + {depth})")),
        (Some(size), None) | (None, Some(size)) => Some(size.to_owned()),
        (None, None) => None,
    };
    if let Some(block_size) = block_size {
        declarations.extend(parse("height", &block_size));
    }
    // An inline-block without line boxes has its baseline at its bottom edge.
    if let Some(depth) = depth {
        declarations.extend(parse("vertical-align", &format!("calc(0px - {depth})")));
    }
    declarations
}

/// Whether `value` is a non-negative CSS `<length>`.
fn is_length(value: &str) -> bool {
    use style::parser::ParserContext;
    use style::values::specified::Length;

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
    parser.parse_entirely(|input| Length::parse_non_negative(&context, input)).is_ok()
}

/// `value` parsed as the CSS `property`'s value: its declarations, or none.
fn parse(property: &str, value: &str) -> Vec<PropertyDeclaration> {
    let Ok(property) = PropertyId::parse_enabled_for_all_content(property) else {
        return Vec::new();
    };
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
