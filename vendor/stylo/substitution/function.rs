/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Evaluating CSS dashed functions. Each invocation has its own parameter and
//! local bindings. A missing local name looks outward through invoking frames
//! before reaching the custom properties on the element.

use super::{ArbitraryDeclarationValue, Substitution, SubstitutionParserContext, SubstitutionReplacementContext};
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
use style_traits::ParseError;

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
    rule: Arc<FunctionRule>,
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
        Some((result.to_variable_value(), tainted || result.attr_tainted))
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
        let v = nested.substitute_variable_value(source, None).ok().map(|v| owned(v, source));
        cx.consulted_attr_taint |= nested.consulted_attr_taint;
        v
    }

    fn outer(self: &Rc<Self>, name: &Name, cx: &mut SubstitutionReplacementContext) -> Option<Value> {
        let previous = std::mem::replace(&mut cx.function_frame, self.parent.clone());
        let value = super::var::VarFunction::resolve_custom_property(name, cx).map(|v| {
            let source = self.rule.result.as_deref().expect("only invoked functions have results");
            owned(v, source)
        });
        cx.function_frame = previous;
        value
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
            match CSSWideKeyword::parse(&mut Parser::new(&v.0.css)) {
                Ok(CSSWideKeyword::Initial) => parameter.and_then(|i| self.parameters.borrow()[i].clone()),
                Ok(CSSWideKeyword::Inherit) => self.outer(name, cx),
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
        return Some(value.map(|(v, tainted)| Substitution::from_value(v, tainted)));
    }
    None
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
                let (arg, _) = ArbitraryDeclarationValue::parse_free_form(input, cx, Delimiter::Comma, false)?;
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
        if self.arguments.len() > rule.parameters.len()
            || rule.parameters[self.arguments.len()..].iter().any(|p| p.default.is_none())
        {
            return Err(());
        }

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
            let supplied = self
                .arguments
                .get(index)
                .and_then(|arg| cx.substitute_declaration_value(arg, source).ok())
                .and_then(|v| frame.typed(Some(&param.name), owned(v, source), Some(&param.syntax), cx));
            frame.parameters.borrow_mut()[index] = supplied.or_else(|| {
                let v = frame.substitute(param.default.as_ref()?, cx)?;
                match CSSWideKeyword::parse(&mut Parser::new(&v.0.css)) {
                    Ok(CSSWideKeyword::Inherit) => frame.outer(&param.name, cx),
                    Ok(_) => None,
                    Err(_) => frame.typed(Some(&param.name), v, Some(&param.syntax), cx),
                }
            });
        }
        frame
            .locals
            .borrow_mut()
            .extend(frame.rule.locals.iter().map(|(n, v)| (n.clone(), Binding::Pending(v.clone()))));
        let v = frame.substitute(&result, cx).ok_or(())?;
        let (v, tainted) = frame.typed(None, v, Some(&frame.rule.returns), cx).ok_or(())?;
        Ok(Substitution::from_value(v, tainted))
    }
}
