/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Evaluating CSS dashed functions. Each invocation has its own parameter and
//! local bindings. A missing local name looks outward through invoking frames
//! before reaching the custom properties on the element.

use super::{ArbitraryDeclarationValue, ReferenceFlags, Substitution, SubstitutionParserContext, SubstitutionReplacementContext};
use crate::Atom;
use crate::custom_properties::{Name, VariableValue};
use crate::derives::*;
use crate::properties::{CSSWideKeyword, PropertyIdRef};
use crate::properties_and_values::{syntax::Descriptor, value::SpecifiedValue};
use crate::stylesheets::FunctionRule;
use crate::values::computed::ToComputedValue;
use cssparser::{Delimiter, Parser};
use servo_arc::Arc;
use std::cell::RefCell;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::rc::Rc;
use style_traits::{ParseError, StyleParseErrorKind};

/// A positional invocation with each argument retained as a token range.
#[derive(Clone, Debug, MallocSizeOf, PartialEq, ToShmem)]
pub struct FunctionCall {
    name: Atom,
    position: usize,
    pub(crate) arguments: Vec<ArbitraryDeclarationValue>,
}

type Value = (VariableValue, bool);

#[derive(Clone)]
enum Binding {
    Pending(Arc<VariableValue>),
    Resolving,
    Ready(Option<Value>),
}

pub(crate) struct Frame {
    pub(crate) rule: Arc<FunctionRule>,
    scope_skip: usize,
    parent: Option<Rc<Frame>>,
    seed: u64,
    parameters: RefCell<Vec<Option<Value>>>,
    locals: RefCell<Vec<(Name, Binding)>>,
    resolving: RefCell<Vec<usize>>,
}

fn owned(v: Substitution, source: &VariableValue) -> Value {
    (VariableValue::new(v.css.into_owned(), &source.url_data, v.first_token_type, v.last_token_type), v.attr_tainted)
}

impl Frame {
    fn parameter(&self, name: &Name) -> Option<usize> {
        self.rule.parameters.iter().position(|p| &p.name == name)
    }

    fn typed(
        &self,
        slot: Option<&Name>,
        value: Value,
        syntax: Option<&Descriptor>,
        cx: &SubstitutionReplacementContext,
    ) -> Option<Value> {
        let Some(syntax) = syntax.filter(|s| !s.is_universal()) else { return Some(value) };
        let (v, tainted) = value;
        // The synthetic name supplies the function/slot pair to random()'s existing
        // property-scoped identity machinery. Source names include the leading dashes.
        let function = self.rule.name.to_string();
        let slot = slot.map_or_else(|| "result".to_owned(), |n| format!("--{n}"));
        let name = Atom::from(format!("{}-{slot}", &function[2..]));
        let base = cx.parser_context(&v.url_data);
        let result = base
            .with_property_id(Some(PropertyIdRef::Custom(&name)), |p| {
                p.with_function_invocation(self.seed, tainted, v.css.len(), |p| {
                    SpecifiedValue::parse_with_context(p, &mut Parser::new(&v.css), syntax)
                })
            })
            .ok()?
            .to_computed_value(cx.computed_context);
        let mut computed = result.to_variable_value();
        if cx.function_dependencies.is_some() {
            // The computed value may now be px, so retain the units consulted by this slot.
            computed.references.flags = v.references.flags | ReferenceFlags::font_units_in(&v.css);
        }
        Some((computed, tainted || result.attr_tainted))
    }

    fn substitute(self: &Rc<Self>, source: &VariableValue, cx: &mut SubstitutionReplacementContext) -> Option<Value> {
        let mut nested = SubstitutionReplacementContext::new(
            cx.property_id,
            cx.substitution_functions,
            cx.stylist,
            cx.computed_context,
            cx.attribute_tracker,
        );
        nested.seen = cx.seen.clone();
        nested.function_frame = Some(self.clone());
        nested.function_dependencies = cx.function_dependencies.clone();
        let v = nested.substitute_variable_value(source, None).ok().map(|v| {
            let (mut value, taint) = owned(v, source);
            value.references.flags |= nested.function_units;
            (value, taint)
        });
        cx.consulted_attr_taint |= nested.consulted_attr_taint;
        v
    }

    pub(crate) fn outer(self: &Rc<Self>, name: &Name, cx: &mut SubstitutionReplacementContext) -> Option<Value> {
        let previous = std::mem::replace(&mut cx.function_frame, self.parent.clone());
        let previous_units = std::mem::take(&mut cx.function_units);
        let value = super::var::VarFunction::resolve_custom_property(name, cx).map(|v| {
            let source = self.rule.result.as_deref().expect("only invoked functions have results");
            let (mut value, taint) = owned(v, source);
            value.references.flags |= cx.function_units;
            (value, taint)
        });
        cx.function_units = previous_units;
        cx.function_frame = previous;
        value
    }

    pub(crate) fn initial(&self, name: &Name) -> Option<Value> {
        self.parameter(name).and_then(|i| self.parameters.borrow()[i].clone())
    }

    pub(crate) fn guarded(&self, name: &Name) -> bool {
        self.locals.borrow().iter().any(|(n, val)| n == name && matches!(val, Binding::Resolving))
    }

    pub(crate) fn query_value(&self, name: &Name, cx: &mut SubstitutionReplacementContext) -> Option<Value> {
        let v = super::var::VarFunction::resolve_custom_property(name, cx)?;
        Some(owned(v, self.rule.result.as_ref()?))
    }

    fn local(self: &Rc<Self>, index: usize, name: &Name, cx: &mut SubstitutionReplacementContext) -> Option<Value> {
        let state = self.locals.borrow()[index].1.clone();
        let source = match state {
            Binding::Ready(result) => return result,
            Binding::Resolving => {
                // Invalidate exactly the bindings in the cycle, including when a reference has a fallback.
                let active = self.resolving.borrow();
                if let Some(start) = active.iter().position(|i| *i == index) {
                    let mut locals = self.locals.borrow_mut();
                    for &i in &active[start..] {
                        locals[i].1 = Binding::Ready(None);
                    }
                }
                return None;
            }
            Binding::Pending(v) => v,
        };
        self.locals.borrow_mut()[index].1 = Binding::Resolving;
        self.resolving.borrow_mut().push(index);
        let value = self.substitute(&source, cx).and_then(|v| {
            let parameter = self.parameter(name);
            let keyword = CSSWideKeyword::parse(&mut Parser::new(&v.0.css));
            match keyword {
                Ok(CSSWideKeyword::Initial) => parameter.and_then(|i| self.parameters.borrow()[i].clone()),
                Ok(CSSWideKeyword::Inherit) =>
                    self.outer(name, cx).and_then(|v| self.typed(Some(name), v, parameter.map(|i| &self.rule.parameters[i].syntax), cx)),
                Ok(_) => None,
                Err(_) => self.typed(Some(name), v, parameter.map(|i| &self.rule.parameters[i].syntax), cx),
            }
        });
        self.resolving.borrow_mut().pop();
        let state = &mut self.locals.borrow_mut()[index].1;
        if matches!(state, Binding::Resolving) {
            *state = Binding::Ready(value.clone());
            value
        } else {
            None
        }
    }
}

/// An outer None means the element is the source; an inner None is a shadowing invalid binding.
pub(super) fn local_value(
    name: &Name,
    cx: &mut SubstitutionReplacementContext,
) -> Option<Option<Substitution<'static>>> {
    let mut frame = cx.function_frame.clone();
    while let Some(f) = frame {
        let local = f.locals.borrow().iter().position(|(n, _)| n == name);
        let value = if let Some(i) = local {
            f.local(i, name, cx)
        } else if let Some(i) = f.parameter(name) {
            f.parameters.borrow()[i].clone()
        } else {
            frame = f.parent.clone();
            continue;
        };
        return Some(value.map(|(v, tainted)| {
            cx.function_units |= v.references.flags & ReferenceFlags::NON_CUSTOM;
            Substitution::from_value(v, tainted)
        }));
    }
    None
}

/// Whether an unbraced argument's text is a named argument without a value: a dashed identifier
/// and a colon only (`--name:`, whitespace between and around allowed).
fn unvalued_named_argument(text: &str) -> bool {
    let text = text.trim();
    let Some(name) = text.strip_suffix(':') else { return false };
    let name = name.trim_end();
    name.starts_with("--") && !name.contains(|c: char| c.is_whitespace() || c == ':')
}

impl FunctionCall {
    pub(super) fn parse(
        name: Atom,
        position: usize,
        input: &mut Parser,
        cx: &mut SubstitutionParserContext,
    ) -> Result<Self, ParseError> {
        let mut arguments = Vec::new();
        if !input.is_exhausted() {
            loop {
                let state = input.state();
                let braced = input.expect_curly_bracket_block().is_ok();
                input.reset(&state);
                let (arg, text) = ArbitraryDeclarationValue::parse_free_form(input, cx, Delimiter::Comma, false)?;
                // css-mixins-1 § 3.1: the arguments are `<declaration-value>`s, which cannot be
                // empty (`--f(,)`, `--f(a,,b)`, `--f(a,)`, `--f({})`; only `--f()` has none), and a
                // named argument (`--name: <value>`) has a value after its colon.
                if arg.is_empty() || (!braced && unvalued_named_argument(text)) {
                    return Err(ParseError::custom(StyleParseErrorKind::UnspecifiedError));
                }
                arguments.push(arg);
                if input.is_exhausted() {
                    break;
                }
                input.expect_comma()?;
            }
        }
        Ok(Self { name, position, arguments })
    }

    pub(crate) fn replace<'a>(
        &'a self,
        source: &'a VariableValue,
        cx: &mut SubstitutionReplacementContext<'a, '_, '_, '_>,
    ) -> Result<Substitution<'a>, ()> {
        let parent = cx.function_frame.clone();
        let skip = parent.as_ref().map_or(0, |p| p.scope_skip);
        let (rule, scope_skip) = cx.computed_context.lookup_function(cx.stylist, &self.name, skip).ok_or(())?;
        let result = rule.result.clone().ok_or(())?;
        if self.arguments.len() > rule.parameters.len() { return Err(()); }
        let mut ancestors = parent.clone();
        let mut depth = 0;
        while let Some(f) = ancestors {
            if Arc::ptr_eq(&f.rule, &rule) || depth >= 64 {
                return Err(());
            }
            ancestors = f.parent.clone();
            depth += 1;
        }
        let mut hasher = DefaultHasher::new();
        self.name.hash(&mut hasher);
        self.position.hash(&mut hasher);
        parent.as_ref().map(|f| f.seed).hash(&mut hasher);
        match cx.property_id {
            Some(PropertyIdRef::Custom(n)) => n.hash(&mut hasher),
            Some(PropertyIdRef::NonCustom(n)) => n.name().hash(&mut hasher),
            None => {}
        }
        let frame = Rc::new(Frame {
            parameters: RefCell::new(vec![None; rule.parameters.len()]),
            locals: Default::default(),
            resolving: Default::default(),
            seed: hasher.finish(),
            rule,
            scope_skip,
            parent,
        });
        for (index, param) in frame.rule.parameters.iter().enumerate() {
            let previous_units = std::mem::take(&mut cx.function_units);
            let supplied = self.arguments.get(index)
                .and_then(|arg| cx.substitute_declaration_value(arg, source).ok())
                .and_then(|substitution| {
                    let (mut value, taint) = owned(substitution, source);
                    value.references.flags |= cx.function_units;
                    frame.typed(Some(&param.name), (value, taint), Some(&param.syntax), cx)
                });
            cx.function_units = previous_units;
            let binding = match (supplied, &param.default) {
                (Some(v), _) => Binding::Ready(Some(v)),
                (None, Some(default)) => Binding::Pending(default.clone()),
                (None, None) => Binding::Ready(None),
            };
            frame.locals.borrow_mut().push((param.name.clone(), binding));
        }
        // The argument rule is cascaded as a set. Every parameter shadows its
        // outer namesake, and defaults can depend on either earlier or later parameters.
        for (index, param) in frame.rule.parameters.iter().enumerate() {
            let value = frame.local(index, &param.name, cx);
            frame.parameters.borrow_mut()[index] = value;
        }
        frame.locals.borrow_mut().clear();
        frame
            .locals
            .borrow_mut()
            .extend(frame.rule.locals.iter().map(|(n, v)| (n.clone(), Binding::Pending(v.clone()))));
        let v = frame.substitute(&result, cx).ok_or(())?;
        let (v, tainted) = frame.typed(None, v, Some(&frame.rule.returns), cx).ok_or(())?;
        cx.function_units |= v.references.flags & ReferenceFlags::NON_CUSTOM;
        Ok(Substitution::from_value(v, tainted))
    }
}
