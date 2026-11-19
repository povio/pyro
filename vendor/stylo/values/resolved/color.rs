/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Resolved color values.

use super::{Context, ToResolvedValue};

use crate::properties::{PropertyId, ShorthandId};
use crate::values::computed::color as computed;
use crate::values::generics::color as generics;

impl ToResolvedValue for computed::Color {
    // A resolved color value is (almost always) a rgba color, with currentcolor resolved.
    type ResolvedValue = Self;

    #[inline]
    fn to_resolved_value(self, context: &Context) -> Self::ResolvedValue {
        if context.for_property == PropertyId::NonCustom(ShorthandId::TextDecoration.into())
            && matches!(self, Self::CurrentColor)
        {
            return self;
        }
        generics::Color::Absolute(context.style.resolve_color(&self))
    }

    #[inline]
    fn from_resolved_value(resolved: Self::ResolvedValue) -> Self {
        resolved
    }
}

/// pyro: a resolved caret-color: the caret's colour (`auto` resolved to currentcolor) and the
/// colour of the text under a block caret, when it has one.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedCaretColor(pub computed::Color, pub Option<computed::Color>);

impl style_traits::ToCss for ResolvedCaretColor {
    fn to_css<W>(&self, dest: &mut style_traits::CssWriter<W>) -> std::fmt::Result
    where
        W: std::fmt::Write,
    {
        self.0.to_css(dest)?;
        if let Some(text) = &self.1 {
            std::fmt::Write::write_str(dest, " ")?;
            text.to_css(dest)?;
        }
        Ok(())
    }
}

impl ToResolvedValue for computed::CaretColor {
    // A resolved caret-color value is an rgba color, with auto resolving to
    // currentcolor.
    type ResolvedValue = ResolvedCaretColor;

    #[inline]
    fn to_resolved_value(self, context: &Context) -> Self::ResolvedValue {
        let color = match self.0 {
            generics::ColorOrAuto::Color(color) => color,
            generics::ColorOrAuto::Auto => generics::Color::currentcolor(),
        };
        let text = match self.1 {
            generics::ColorOrAuto::Color(color) => Some(color.to_resolved_value(context)),
            generics::ColorOrAuto::Auto => None,
        };
        ResolvedCaretColor(color.to_resolved_value(context), text)
    }

    #[inline]
    fn from_resolved_value(resolved: Self::ResolvedValue) -> Self {
        generics::CaretColor(
            generics::ColorOrAuto::Color(computed::Color::from_resolved_value(resolved.0)),
            resolved.1.map_or(generics::ColorOrAuto::Auto, |text| {
                generics::ColorOrAuto::Color(computed::Color::from_resolved_value(text))
            }),
        )
    }
}
