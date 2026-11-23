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
        let mut indices = crate::parser::EarlyRandomIndices::new(context.early_random_count, context.stylist);
        indices.function_rule = context.function_frame.as_ref().map(|f| f.rule.as_ref());
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
            use crate::properties::{LonghandId, PropertyIdRef};
            let computed = context.computed_context;
            let guards_viewport = computed.is_root_element() && matches!(context.property_id,
                Some(PropertyIdRef::NonCustom(id)) if [LonghandId::OverflowX, LonghandId::OverflowY,
                    LonghandId::ScrollbarWidth, LonghandId::ScrollbarGutter].iter().any(|axis| axis.is_or_is_longhand_of(id)));
            let ((condition, tainted), mut cyclic) = if guards_viewport {
                computed.track_viewport_use(|| branch.parsed_condition(source, context))
            } else {
                (branch.parsed_condition(source, context), false)
            };
            attr_tainted |= tainted;
            let Some(condition) = condition else { continue };
            let result = condition.evaluate(&mut |predicate| {
                if guards_viewport && matches!(predicate, crate::queries::condition::IfPredicate::Style(_)) {
                    predicate.for_each_property(&mut |name| {
                        let registration = context.stylist.get_custom_property_registration(&name);
                        cyclic |= context.substitution_functions.get_var(registration, &name).is_some_and(|v| v.viewport_dependent);
                    });
                    let (matched, used) = computed.track_viewport_use(|| predicate.matches(source, context, &mut attr_tainted));
                    cyclic |= used;
                    matched
                } else {
                    predicate.matches(source, context, &mut attr_tainted)
                }
            });
            if cyclic { return Err(()); }
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
