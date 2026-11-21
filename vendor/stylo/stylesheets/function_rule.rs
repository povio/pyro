/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Function definitions in CSS Functions and Mixins.

use crate::Atom;
use crate::custom_properties::{Name, VariableValue, parse_name};
use crate::derives::*;
use crate::parser::ParserContext;
use crate::properties_and_values::syntax::Descriptor;
use crate::properties_and_values::value::SpecifiedValue;
use crate::shared_lock::{SharedRwLockReadGuard, ToCssWithGuard};
use cssparser::{
    AtRuleParser, CowRcStr, DeclarationParser, Parser, ParserState, QualifiedRuleParser, RuleBodyItemParser,
    RuleBodyParser, Token,
};
use servo_arc::Arc;
use std::fmt::{self, Write};
use style_traits::{CssStringWriter, ParseError, StyleParseErrorKind};

/// A parsed function definition, including its signature and descriptors.
#[derive(Clone, Debug, MallocSizeOf, ToShmem)]
pub struct FunctionRule {
    /// The function name including its two leading dashes.
    pub name: Atom,
    pub(crate) parameters: Vec<Parameter>,
    pub(crate) returns: Descriptor,
    pub(crate) locals: Vec<(Name, Arc<VariableValue>)>,
    pub(crate) result: Option<Arc<VariableValue>>,
    text: String,
}

#[derive(Clone, Debug, ToShmem, MallocSizeOf)]
pub(crate) struct Parameter {
    pub name: Name,
    pub syntax: Descriptor,
    pub default: Option<Arc<VariableValue>>,
}

fn invalid() -> ParseError {
    ParseError::custom(StyleParseErrorKind::UnspecifiedError)
}

fn parse_type(input: &mut Parser) -> Result<Descriptor, ParseError> {
    if input.try_parse(|i| i.expect_function_matching("type")).is_ok() {
        return input.parse_nested_block(Descriptor::from_css_parser);
    }
    input.skip_whitespace();
    let start = input.position();
    match input.next()? {
        Token::Ident(_) => {}
        Token::Delim('<') => {
            input.expect_ident()?;
            input.expect_delim('>')?;
        }
        _ => return Err(invalid()),
    }
    let _ = input.try_parse(|i| match i.next_including_whitespace()? {
        Token::Delim('+' | '#') => Ok(()),
        _ => Err(invalid()),
    });
    Descriptor::from_css_parser(&mut Parser::new(input.slice_from(start)))
}

impl FunctionRule {
    /// Parse a function from a CSSOM prelude and block using the stylesheet grammar.
    pub fn from_css(context: &ParserContext, prelude: &str, body: &str) -> Result<Self, ParseError> {
        let signature = Parser::new(prelude).parse_entirely(|i| Self::prelude(context, i))?;
        Ok(signature.parse_body(context, &mut Parser::new(body)))
    }

    pub(crate) fn prelude(context: &ParserContext, input: &mut Parser) -> Result<Self, ParseError> {
        input.skip_whitespace();
        let start = input.position();
        let name = input.expect_function()?.clone();
        if parse_name(&name).is_err() {
            return Err(invalid());
        }
        let parameters = input.parse_nested_block(|i| {
            if i.is_exhausted() {
                return Ok(Vec::new());
            }
            i.parse_comma_separated(|i| {
                let name = Atom::from(parse_name(i.expect_ident()?).map_err(|_| invalid())?);
                let syntax = i.try_parse(parse_type).unwrap_or_else(|_| Descriptor::universal());
                let default = if i.try_parse(|i| i.expect_colon()).is_ok() {
                    i.skip_whitespace();
                    let v = if i.try_parse(|i| i.expect_curly_bracket_block()).is_ok() {
                        i.parse_nested_block(|i| {
                            i.skip_whitespace();
                            VariableValue::parse(context, i)
                        })?
                    } else {
                        VariableValue::parse(context, i)?
                    };
                    if !syntax.is_universal() && !v.has_references() {
                        // A non-substitution default must parse for the parameter grammar.
                        context.with_property_id(Some((&name).into()), |cx| {
                            SpecifiedValue::parse_with_context(cx, &mut Parser::new(&v.css), &syntax)
                        })?;
                    }
                    Some(Arc::new(v))
                } else {
                    None
                };
                Ok(Parameter { name, syntax, default })
            })
        })?;
        if parameters.iter().enumerate().any(|(n, p)| parameters[..n].iter().any(|q| q.name == p.name)) {
            return Err(invalid());
        }
        let returns = if input.try_parse(|i| i.expect_ident_matching("returns")).is_ok() {
            parse_type(input)?
        } else {
            Descriptor::universal()
        };
        Ok(Self {
            name: Atom::from(name.as_ref()),
            parameters,
            returns,
            result: None,
            locals: Vec::new(),
            text: input.slice_from(start).trim_end().to_owned(),
        })
    }

    pub(crate) fn parse_body(mut self, context: &ParserContext, input: &mut Parser) -> Self {
        let start = input.position();
        for _ in RuleBodyParser::new(input, &mut BodyParser { context, rule: &mut self }) {}
        self.text.push_str(" {");
        self.text.push_str(input.slice_from(start));
        self.text.push('}');
        self
    }
}

struct BodyParser<'a, 'b> {
    context: &'a ParserContext<'a>,
    rule: &'b mut FunctionRule,
}
impl<'i> AtRuleParser<'i> for BodyParser<'_, '_> {
    type Prelude = ();
    type AtRule = ();
    type Error = StyleParseErrorKind;
}
impl<'i> QualifiedRuleParser<'i> for BodyParser<'_, '_> {
    type Prelude = ();
    type QualifiedRule = ();
    type Error = StyleParseErrorKind;
}
impl<'i> RuleBodyItemParser<'i, (), StyleParseErrorKind> for BodyParser<'_, '_> {
    fn parse_declarations(&self) -> bool {
        true
    }
    fn parse_qualified(&self) -> bool {
        false
    }
}
impl<'i> DeclarationParser<'i> for BodyParser<'_, '_> {
    type Declaration = ();
    type Error = StyleParseErrorKind;
    fn parse_value(&mut self, name: CowRcStr<'i>, input: &mut Parser<'i>, _: &ParserState) -> Result<(), ParseError> {
        input.skip_whitespace();
        let value = Arc::new(input.parse_entirely(|i| VariableValue::parse(self.context, i))?);
        if name.eq_ignore_ascii_case("result") {
            self.rule.result = Some(value);
        } else {
            let name = Atom::from(parse_name(&name).map_err(|_| invalid())?);
            if let Some(local) = self.rule.locals.iter_mut().find(|(n, _)| n == &name) {
                local.1 = value;
            } else {
                self.rule.locals.push((name, value));
            }
        }
        Ok(())
    }
}

impl ToCssWithGuard for FunctionRule {
    fn to_css(&self, _: &SharedRwLockReadGuard, dest: &mut CssStringWriter) -> fmt::Result {
        write!(dest, "@function {}", self.text)
    }
}
