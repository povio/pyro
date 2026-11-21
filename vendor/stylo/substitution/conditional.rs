/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Inline conditional substitution in CSS Values 5.

use super::{ArbitraryDeclarationValue, Substitution, SubstitutionParserContext, SubstitutionReplacementContext};
use crate::custom_properties::VariableValue;
use crate::derives::*;
use crate::queries::condition::IfCondition;
use cssparser::{Delimiter, Parser};
use style_traits::{ParseError, StyleParseErrorKind};

/// Arguments of an inline if(), kept as token ranges until substitution.
#[derive(Clone, Debug, MallocSizeOf, PartialEq, ToShmem)]
pub struct IfFunction {
    pub(crate) branches: Box<[Branch]>,
}

#[derive(Clone, Debug, MallocSizeOf, PartialEq, ToShmem)]
pub(crate) struct Branch {
    pub(crate) condition: ArbitraryDeclarationValue,
    pub(crate) value: ArbitraryDeclarationValue,
}

impl Branch {
    pub(crate) fn parsed_condition<'a>(
        &'a self,
        source: &'a VariableValue,
        context: &mut SubstitutionReplacementContext<'a, '_, '_, '_>,
    ) -> (Option<IfCondition>, bool) {
        let earlier = std::mem::take(&mut context.consulted_attr_taint);
        let substituted = context.substitute_declaration_value(&self.condition, source);
        let consulted = context.consulted_attr_taint;
        context.consulted_attr_taint |= earlier;
        let Ok(substituted) = substituted else { return (None, consulted) };
        let parser_context = context.parser_context(&source.url_data);
        let indices = crate::parser::EarlyRandomIndices::new(context.early_random_count, context.stylist);
        let parsed = parser_context.with_random_scope(context.property_id, Some(&indices), |cx| {
            Parser::new(&substituted.css).parse_entirely(|i| IfCondition::parse(cx, i)).ok()
        });
        if parsed.is_some() {
            context.early_random_count = indices.end();
        }
        (parsed, substituted.attr_tainted || consulted)
    }
}

impl IfFunction {
    pub(super) fn parse(input: &mut Parser, context: &mut SubstitutionParserContext) -> Result<Self, ParseError> {
        let mut branches = Vec::new();
        loop {
            input.skip_whitespace();
            let condition = if input.try_parse(|i| i.expect_curly_bracket_block()).is_ok() {
                input.parse_nested_block(|i| ArbitraryDeclarationValue::parse_internal(i, context, Delimiter::None))?
            } else {
                ArbitraryDeclarationValue::parse_internal_mode::<true>(input, context, Delimiter::None)?
            };
            if condition.is_empty() {
                return Err(ParseError::custom(StyleParseErrorKind::UnspecifiedError));
            }
            input.expect_colon()?;
            let (value, _) = ArbitraryDeclarationValue::parse_free_form(input, context, Delimiter::Semicolon, false)?;
            branches.push(Branch { condition, value });
            if input.is_exhausted() {
                break;
            }
            input.expect_semicolon()?;
            if input.is_exhausted() {
                break;
            }
        }
        Ok(Self { branches: branches.into_boxed_slice() })
    }

    pub(super) fn replace<'a>(
        &'a self,
        source: &'a VariableValue,
        context: &mut SubstitutionReplacementContext<'a, '_, '_, '_>,
    ) -> Result<Substitution<'a>, ()> {
        let mut attr_tainted = false;
        for branch in &self.branches {
            let (condition, tainted) = branch.parsed_condition(source, context);
            attr_tainted |= tainted;
            let Some(condition) = condition else { continue };
            let result = condition.evaluate(&mut |predicate| predicate.matches(source, context, &mut attr_tainted));
            context.consulted_attr_taint |= attr_tainted;
            if result.to_bool(false) {
                let mut result = context.substitute_declaration_value(&branch.value, source)?;
                result.attr_tainted |= attr_tainted;
                return Ok(result);
            }
        }
        Ok(Substitution { attr_tainted, ..Default::default() })
    }

    pub(super) fn for_each_unparsed_value(&self, mut f: impl FnMut(&ArbitraryDeclarationValue)) {
        for branch in &self.branches {
            f(&branch.condition);
            f(&branch.value);
        }
    }
}
