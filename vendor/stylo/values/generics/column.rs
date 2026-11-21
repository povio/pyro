/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Generic types for the column properties.
use crate::derives::*;
use crate::parser::{Parse, ParserContext};
use crate::values::{
    animated::{Animate, Procedure},
    distance::{ComputeSquaredDistance, SquaredDistance},
    specified::Integer,
};
use cssparser::Parser;
use std::fmt::{self, Write};
use style_traits::{CssWriter, ParseError, ToCss};

#[allow(missing_docs)]
#[derive(
    Clone,
    Debug,
    PartialEq,
    MallocSizeOf,
    SpecifiedValueInfo,
    ToComputedValue,
    ToResolvedValue,
    ToAnimatedValue,
    ToShmem,
    ToTyped,
)]
#[typed(todo_derive_fields)]
pub enum RuleEntry<Value, Count> {
    Single(Value),
    Repeat(Option<Count>, Box<[Value]>),
}

#[allow(missing_docs)]
#[derive(
    Clone,
    Debug,
    PartialEq,
    MallocSizeOf,
    SpecifiedValueInfo,
    ToComputedValue,
    ToResolvedValue,
    ToAnimatedValue,
    ToShmem,
    ToTyped,
)]
#[typed(todo_derive_fields)]
pub struct RuleList<Value, Count>(pub Box<[RuleEntry<Value, Count>]>);

impl<Value, Count> From<Value> for RuleList<Value, Count> {
    fn from(value: Value) -> Self {
        Self(Box::new([RuleEntry::Single(value)]))
    }
}

impl<Value, Count> RuleList<Value, Count> {
    #[allow(missing_docs)]
    pub fn map<Out>(&self, mut map: impl FnMut(&Value) -> Out) -> RuleList<Out, Count>
    where
        Count: Clone,
    {
        RuleList(
            self.0
                .iter()
                .map(|entry| match entry {
                    RuleEntry::Single(value) => RuleEntry::Single(map(value)),
                    RuleEntry::Repeat(count, values) => {
                        RuleEntry::Repeat(count.clone(), values.iter().map(&mut map).collect())
                    }
                })
                .collect(),
        )
    }
}

impl<Value: Parse> Parse for RuleList<Value, Integer> {
    fn parse(context: &ParserContext, input: &mut Parser) -> Result<Self, ParseError> {
        let mut has_auto = false;
        input
            .parse_comma_separated(|input| {
                if input.try_parse(|input| input.expect_function_matching("repeat")).is_err() {
                    return Value::parse(context, input).map(RuleEntry::Single);
                }
                input.parse_nested_block(|input| {
                    let count = if input.try_parse(|input| input.expect_ident_matching("auto")).is_ok() {
                        if has_auto {
                            return Err(ParseError::custom(style_traits::StyleParseErrorKind::UnspecifiedError));
                        }
                        has_auto = true;
                        None
                    } else {
                        Some(Integer::parse_positive(context, input)?)
                    };
                    input.expect_comma()?;
                    let values = input.parse_comma_separated(|input| Value::parse(context, input))?;
                    Ok(RuleEntry::Repeat(count, values.into_boxed_slice()))
                })
            })
            .map(|items| Self(items.into_boxed_slice()))
    }
}

impl<Value: ToCss, Count: ToCss> ToCss for RuleList<Value, Count> {
    fn to_css<W: Write>(&self, dest: &mut CssWriter<W>) -> fmt::Result {
        for (index, entry) in self.0.iter().enumerate() {
            if index != 0 {
                dest.write_str(", ")?;
            }
            match entry {
                RuleEntry::Single(value) => value.to_css(dest)?,
                RuleEntry::Repeat(count, values) => {
                    dest.write_str("repeat(")?;
                    if let Some(count) = count {
                        count.to_css(dest)?;
                    } else {
                        dest.write_str("auto")?;
                    }
                    for value in values {
                        dest.write_str(", ")?;
                        value.to_css(dest)?;
                    }
                    dest.write_char(')')?;
                }
            }
        }
        Ok(())
    }
}

impl<Value> RuleList<Value, i32> {
    #[allow(missing_docs)]
    pub fn for_gap(&self, index: usize, total: usize) -> &Value {
        let count = |entry: &RuleEntry<Value, i32>| match entry {
            RuleEntry::Single(_) => 1,
            RuleEntry::Repeat(Some(n), values) => values.len().saturating_mul((*n).max(1) as usize),
            RuleEntry::Repeat(None, _) => 0,
        };
        let fixed = self.0.iter().map(count).fold(0usize, usize::saturating_add);
        let auto = self.0.iter().any(|entry| matches!(entry, RuleEntry::Repeat(None, _)));
        let mut offset = if auto { index } else { index % fixed };
        for entry in &self.0 {
            let length = match entry {
                RuleEntry::Repeat(None, _) => total.saturating_sub(fixed),
                _ => count(entry),
            };
            if offset < length {
                return match entry {
                    RuleEntry::Single(value) => value,
                    RuleEntry::Repeat(_, values) => &values[offset % values.len()],
                };
            }
            offset -= length;
        }
        unreachable!()
    }
    fn segments(&self) -> Result<(Vec<&Value>, Option<&[Value]>, Vec<&Value>), ()> {
        let (mut before, mut auto, mut after) = (Vec::new(), None, Vec::new());
        for entry in &self.0 {
            let segment = if auto.is_none() { &mut before } else { &mut after };
            match entry {
                RuleEntry::Single(value) => segment.push(value),
                RuleEntry::Repeat(None, values) => auto = Some(values.as_ref()),
                RuleEntry::Repeat(Some(count), values) => {
                    let length = values.len().checked_mul((*count).max(1) as usize).ok_or(())?;
                    if segment.len().saturating_add(length) > 65536 {
                        return Err(());
                    }
                    segment.extend(values.iter().cycle().take(length));
                }
            }
        }
        Ok((before, auto, after))
    }

    fn matched<Out>(
        &self,
        other: &Self,
        mut pair: impl FnMut(&Value, &Value) -> Result<Out, ()>,
    ) -> Result<RuleList<Out, i32>, ()> {
        use num_integer::Integer as _;
        let (left, left_auto, left_end) = self.segments()?;
        let (right, right_auto, right_end) = other.segments()?;
        let length = |a: usize, b: usize| {
            a.checked_div(a.gcd(&b)).and_then(|n| n.checked_mul(b)).filter(|n| *n <= 65536).ok_or(())
        };
        let mut items = Vec::new();
        match (left_auto, right_auto) {
            (None, None) => {
                for index in 0..length(left.len(), right.len())? {
                    items.push(RuleEntry::Single(pair(left[index % left.len()], right[index % right.len()])?));
                }
            }
            (Some(a), Some(b)) if left.len() == right.len() && left_end.len() == right_end.len() => {
                for (x, y) in left.into_iter().zip(right) {
                    items.push(RuleEntry::Single(pair(x, y)?));
                }
                let values = (0..length(a.len(), b.len())?)
                    .map(|n| pair(&a[n % a.len()], &b[n % b.len()]))
                    .collect::<Result<_, _>>()?;
                items.push(RuleEntry::Repeat(None, values));
                for (x, y) in left_end.into_iter().zip(right_end) {
                    items.push(RuleEntry::Single(pair(x, y)?));
                }
            }
            _ => return Err(()),
        }
        Ok(RuleList(items.into_boxed_slice()))
    }
}

impl<Value: Animate> Animate for RuleList<Value, i32> {
    fn animate(&self, other: &Self, procedure: Procedure) -> Result<Self, ()> {
        self.matched(other, |left, right| left.animate(right, procedure))
    }
}

impl<Value: crate::values::animated::ToAnimatedZero> crate::values::animated::ToAnimatedZero for RuleList<Value, i32> {
    fn to_animated_zero(&self) -> Result<Self, ()> {
        self.matched(self, |value, _| value.to_animated_zero())
    }
}

impl<Value: ComputeSquaredDistance> ComputeSquaredDistance for RuleList<Value, i32> {
    fn compute_squared_distance(&self, other: &Self) -> Result<SquaredDistance, ()> {
        let mut total = SquaredDistance::from_sqrt(0.);
        self.matched(other, |left, right| {
            total = total + left.compute_squared_distance(right)?;
            Ok(())
        })?;
        Ok(total)
    }
}

/// A generic type for `column-count` values.
#[derive(
    Animate,
    Clone,
    ComputeSquaredDistance,
    Copy,
    Debug,
    MallocSizeOf,
    Parse,
    PartialEq,
    SpecifiedValueInfo,
    ToAnimatedValue,
    ToAnimatedZero,
    ToComputedValue,
    ToCss,
    ToResolvedValue,
    ToShmem,
    ToTyped,
)]
#[repr(u8)]
#[typed(todo_derive_fields)]
pub enum GenericColumnCount<PositiveInteger> {
    /// A positive integer.
    Integer(PositiveInteger),
    /// The keyword `auto`.
    #[animation(error)]
    Auto,
}

pub use self::GenericColumnCount as ColumnCount;
impl<I> ColumnCount<I> {
    /// Returns whether this value is `auto`.
    #[inline]
    pub fn is_auto(&self) -> bool {
        matches!(self, ColumnCount::Auto)
    }
}
