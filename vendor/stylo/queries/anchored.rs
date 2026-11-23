/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! pyro: `anchored()` container queries.
//!
//! https://drafts.csswg.org/css-anchor-position-2/#anchored-container-queries

use super::condition::Operator;
use crate::derives::*;
use crate::logical_geometry::{LogicalAxis, WritingMode};
use crate::parser::{Parse, ParserContext};
use crate::values::specified::position::{
    DashedIdentAndOrTryTactic, PositionArea, PositionAreaAxis, PositionAreaKeyword,
    PositionAreaTrack, PositionAreaType, PositionTryFallbacksItem,
};
use cssparser::Parser;
use std::fmt::{self, Write};
use style_traits::{CssWriter, ParseError, StyleParseErrorKind, ToCss};

/// One `<position-area>` keyword slot of an `anchored(fallback: ...)` query: a keyword, or `any`,
/// which matches whatever the applied fallback selects on that axis.
#[derive(Clone, Copy, Debug, MallocSizeOf, PartialEq, ToShmem)]
pub enum AnchoredAreaSlot {
    /// `any`
    Any,
    /// A `<position-area>` keyword other than `none`.
    Keyword(PositionAreaKeyword),
}

impl AnchoredAreaSlot {
    fn parse(input: &mut Parser) -> Result<Self, ParseError> {
        if input.try_parse(|i| i.expect_ident_matching("any")).is_ok() {
            return Ok(Self::Any);
        }
        let keyword = PositionAreaKeyword::parse(input)?;
        if keyword.is_none() {
            return Err(ParseError::custom(StyleParseErrorKind::UnspecifiedError));
        }
        Ok(Self::Keyword(keyword))
    }
}

impl ToCss for AnchoredAreaSlot {
    fn to_css<W>(&self, dest: &mut CssWriter<W>) -> fmt::Result
    where
        W: Write,
    {
        match *self {
            Self::Any => dest.write_str("any"),
            Self::Keyword(keyword) => keyword.to_css(dest),
        }
    }
}

/// The value of a `fallback: <value>` feature.
#[derive(Clone, Debug, MallocSizeOf, PartialEq, ToShmem)]
pub enum AnchoredFallback {
    /// `none`: no fallback option is applied.
    None,
    /// `<dashed-ident> || <try-tactic>`
    Tactic(DashedIdentAndOrTryTactic),
    /// A `<position-area>` whose keywords may be `any`; a missing second keyword is `any` too,
    /// unless the first has no axis of its own.
    Area(AnchoredAreaSlot, Option<AnchoredAreaSlot>),
}

impl AnchoredFallback {
    fn parse(context: &ParserContext, input: &mut Parser) -> Result<Self, ParseError> {
        if input.try_parse(|i| i.expect_ident_matching("none")).is_ok() {
            return Ok(Self::None);
        }
        if let Ok(tactic) = input.try_parse(|i| DashedIdentAndOrTryTactic::parse(context, i)) {
            return Ok(Self::Tactic(tactic));
        }
        let first = AnchoredAreaSlot::parse(input)?;
        let second = input.try_parse(AnchoredAreaSlot::parse).ok();
        if let (AnchoredAreaSlot::Keyword(first), Some(AnchoredAreaSlot::Keyword(second))) =
            (first, second)
        {
            // Two keywords pair up as in `position-area`.
            if (PositionArea { first, second }).get_type() == PositionAreaType::None {
                return Err(ParseError::custom(StyleParseErrorKind::UnspecifiedError));
            }
        }
        Ok(Self::Area(first, second))
    }

    fn matches(&self, applied: &PositionTryFallbacksItem, self_wm: WritingMode, cb_wm: WritingMode) -> bool {
        match (self, applied) {
            // Chrome matches the ident and the tactic list as written: `flip-block flip-inline`
            // does not match an applied `flip-inline flip-block`, nor `flip-y` a `flip-block`
            // (WPT `container-queries/anchored-fallback-try-tactic`).
            (Self::Tactic(query), PositionTryFallbacksItem::IdentAndOrTactic(applied)) => query == applied,
            (Self::Area(first, second), PositionTryFallbacksItem::PositionArea(applied)) => {
                let second = second.unwrap_or_else(|| match *first {
                    AnchoredAreaSlot::Keyword(keyword) if has_axis(keyword) => AnchoredAreaSlot::Any,
                    other => other,
                });
                let query = Cells::resolve(*first, second, self_wm, cb_wm);
                let applied = Cells::of_area(applied, self_wm, cb_wm);
                Cells::axis_matches(query.x, applied.x) && Cells::axis_matches(query.y, applied.y)
            },
            _ => false,
        }
    }
}

impl ToCss for AnchoredFallback {
    fn to_css<W>(&self, dest: &mut CssWriter<W>) -> fmt::Result
    where
        W: Write,
    {
        match *self {
            Self::None => dest.write_str("none"),
            Self::Tactic(ref tactic) => tactic.to_css(dest),
            Self::Area(ref first, ref second) => {
                first.to_css(dest)?;
                if let Some(second) = second {
                    dest.write_char(' ')?;
                    second.to_css(dest)?;
                }
                Ok(())
            },
        }
    }
}

/// Whether the keyword names its own axis (`top`, `block-end`, `self-x-start`...), rather than
/// taking it from its place in the pair (`start`, `self-end`) or from its partner (`center`,
/// `span-all`).
fn has_axis(keyword: PositionAreaKeyword) -> bool {
    !matches!(keyword.axis(), PositionAreaAxis::Inferred | PositionAreaAxis::None)
}

/// A keyword slot resolved to the physical grid.
#[derive(Clone, Copy)]
enum Resolved {
    Any,
    /// An axis-less keyword's track (`center`, `span-all`): on the axis its partner leaves.
    Free(PositionAreaTrack),
    /// A track on the horizontal (`true`) or vertical axis, counted from the left or top.
    Fixed(bool, PositionAreaTrack),
}

/// The physical cells a `<position-area>` selects: tracks counted from the left (`x`) and the top
/// (`y`); None is `any`.
struct Cells {
    x: Option<PositionAreaTrack>,
    y: Option<PositionAreaTrack>,
}

impl Cells {
    /// The first keyword is on the block axis and the second on the inline one when their own
    /// keywords leave it open. Logical keywords resolve against the containing block's writing
    /// mode and `self-*` ones against the anchored element's; the querying element's does not
    /// matter.
    fn resolve(first: AnchoredAreaSlot, second: AnchoredAreaSlot, self_wm: WritingMode, cb_wm: WritingMode) -> Self {
        let resolve = |slot: AnchoredAreaSlot, axis: LogicalAxis| match slot {
            AnchoredAreaSlot::Any => Resolved::Any,
            AnchoredAreaSlot::Keyword(keyword) => match keyword.physical_track(cb_wm, self_wm, axis) {
                Some((horizontal, track)) => Resolved::Fixed(horizontal, track),
                None => keyword.track().map_or(Resolved::Any, Resolved::Free),
            },
        };
        let slot_horizontal = |axis: LogicalAxis| (axis == LogicalAxis::Inline) != cb_wm.is_vertical();
        let mut cells = Self { x: None, y: None };
        let mut place = |horizontal: bool, track: PositionAreaTrack| {
            if horizontal {
                cells.x = Some(track);
            } else {
                cells.y = Some(track);
            }
        };
        use Resolved::*;
        match (resolve(first, LogicalAxis::Block), resolve(second, LogicalAxis::Inline)) {
            (Fixed(h1, t1), Fixed(h2, t2)) => {
                place(h1, t1);
                place(h2, t2);
            },
            (Fixed(h, t), Free(u)) | (Free(u), Fixed(h, t)) => {
                place(h, t);
                place(!h, u);
            },
            (Fixed(h, t), Any) | (Any, Fixed(h, t)) => place(h, t),
            (Free(u), Free(v)) => {
                place(slot_horizontal(LogicalAxis::Block), u);
                place(slot_horizontal(LogicalAxis::Inline), v);
            },
            (Free(u), Any) => place(slot_horizontal(LogicalAxis::Block), u),
            (Any, Free(v)) => place(slot_horizontal(LogicalAxis::Inline), v),
            (Any, Any) => {},
        }
        cells
    }

    /// The cells of an applied `<position-area>` fallback, whose missing second keyword is
    /// `span-all` (or a copy of an axis-less or inferred first one).
    fn of_area(area: &PositionArea, self_wm: WritingMode, cb_wm: WritingMode) -> Self {
        let second = if !area.second.is_none() {
            area.second
        } else if has_axis(area.first) {
            PositionAreaKeyword::SpanAll
        } else {
            area.first
        };
        let mut cells = Self::resolve(
            AnchoredAreaSlot::Keyword(area.first),
            AnchoredAreaSlot::Keyword(second),
            self_wm,
            cb_wm,
        );
        // An applied area selects all three tracks on an axis it leaves open.
        cells.x.get_or_insert(PositionAreaTrack::SpanAll);
        cells.y.get_or_insert(PositionAreaTrack::SpanAll);
        cells
    }

    fn axis_matches(query: Option<PositionAreaTrack>, applied: Option<PositionAreaTrack>) -> bool {
        query.is_none() || query == applied
    }
}

/// An `<anchored-query>`: `fallback`, `fallback: <value>`, and `not`/`and`/`or` conditions of
/// them in parentheses.
#[derive(Clone, Debug, MallocSizeOf, PartialEq, ToShmem)]
pub enum AnchoredQuery {
    /// `fallback` (whether a fallback option is applied) or `fallback: <value>`.
    Fallback(Option<AnchoredFallback>),
    /// `not <anchored-in-parens>`
    Not(Box<AnchoredQuery>),
    /// `<anchored-in-parens> [and|or <anchored-in-parens>]+`
    Operation(Box<[AnchoredQuery]>, Operator),
    /// `( <anchored-query> )`
    InParens(Box<AnchoredQuery>),
}

impl AnchoredQuery {
    /// Parses the contents of `anchored( ... )`.
    pub fn parse(context: &ParserContext, input: &mut Parser) -> Result<Self, ParseError> {
        if let Ok(feature) = input.try_parse(|i| Self::parse_whole_feature(context, i)) {
            return Ok(feature);
        }
        Self::parse_condition(context, input)
    }

    fn parse_whole_feature(context: &ParserContext, input: &mut Parser) -> Result<Self, ParseError> {
        input.expect_ident_matching("fallback")?;
        if input.is_exhausted() {
            return Ok(Self::Fallback(None));
        }
        input.expect_colon()?;
        let value = AnchoredFallback::parse(context, input)?;
        input.expect_exhausted()?;
        Ok(Self::Fallback(Some(value)))
    }

    fn parse_condition(context: &ParserContext, input: &mut Parser) -> Result<Self, ParseError> {
        if input.try_parse(|i| i.expect_ident_matching("not")).is_ok() {
            return Ok(Self::Not(Box::new(Self::parse_in_parens(context, input)?)));
        }
        let first = Self::parse_in_parens(context, input)?;
        let operator = match input.try_parse(Operator::parse) {
            Ok(operator) => operator,
            Err(..) => return Ok(first),
        };
        let mut conditions = vec![first, Self::parse_in_parens(context, input)?];
        let delimiter = match operator {
            Operator::And => "and",
            Operator::Or => "or",
        };
        while input.try_parse(|i| i.expect_ident_matching(delimiter)).is_ok() {
            conditions.push(Self::parse_in_parens(context, input)?);
        }
        Ok(Self::Operation(conditions.into_boxed_slice(), operator))
    }

    fn parse_in_parens(context: &ParserContext, input: &mut Parser) -> Result<Self, ParseError> {
        input.expect_parenthesis_block()?;
        let inner = input.parse_nested_block(|i| Self::parse(context, i))?;
        Ok(Self::InParens(Box::new(inner)))
    }

    /// Whether the query matches an anchored query container whose applied fallback option is
    /// `applied`. `self_wm` is the container's writing mode, `cb_wm` its containing block's.
    /// Every leaf is known once there is a container, so the Kleene and/or/not of the caller
    /// reduce to plain booleans here.
    pub fn matches(
        &self,
        applied: Option<&PositionTryFallbacksItem>,
        self_wm: WritingMode,
        cb_wm: WritingMode,
    ) -> bool {
        match *self {
            Self::Fallback(None) => applied.is_some(),
            Self::Fallback(Some(AnchoredFallback::None)) => applied.is_none(),
            Self::Fallback(Some(ref value)) => applied.is_some_and(|applied| value.matches(applied, self_wm, cb_wm)),
            Self::Not(ref inner) => !inner.matches(applied, self_wm, cb_wm),
            Self::InParens(ref inner) => inner.matches(applied, self_wm, cb_wm),
            Self::Operation(ref conditions, Operator::And) => {
                conditions.iter().all(|c| c.matches(applied, self_wm, cb_wm))
            },
            Self::Operation(ref conditions, Operator::Or) => {
                conditions.iter().any(|c| c.matches(applied, self_wm, cb_wm))
            },
        }
    }
}

impl ToCss for AnchoredQuery {
    fn to_css<W>(&self, dest: &mut CssWriter<W>) -> fmt::Result
    where
        W: Write,
    {
        match *self {
            Self::Fallback(ref value) => {
                dest.write_str("fallback")?;
                if let Some(value) = value {
                    dest.write_str(": ")?;
                    value.to_css(dest)?;
                }
                Ok(())
            },
            Self::Not(ref inner) => {
                dest.write_str("not ")?;
                inner.to_css(dest)
            },
            Self::InParens(ref inner) => {
                dest.write_char('(')?;
                inner.to_css(dest)?;
                dest.write_char(')')
            },
            Self::Operation(ref conditions, operator) => {
                let mut iter = conditions.iter();
                iter.next().unwrap().to_css(dest)?;
                for condition in iter {
                    dest.write_char(' ')?;
                    operator.to_css(dest)?;
                    dest.write_char(' ')?;
                    condition.to_css(dest)?;
                }
                Ok(())
            },
        }
    }
}
