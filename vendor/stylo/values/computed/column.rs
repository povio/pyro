/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Computed types for the column properties.

use crate::values::computed::PositiveInteger;
#[allow(missing_docs)]
pub type RuleColor = super::super::generics::column::RuleList<super::Color, i32>;
#[allow(missing_docs)]
pub type RuleWidth = super::super::generics::column::RuleList<super::BorderSideWidth, i32>;
#[allow(missing_docs)]
pub type RuleStyle = super::super::generics::column::RuleList<super::BorderStyle, i32>;
use crate::values::generics::column::GenericColumnCount;
pub use crate::values::specified::column::{RuleBreak, RuleOverlap, RuleVisibilityItems};
pub use crate::values::specified::column::{ColumnFill, ColumnSpan, ColumnWrap};

/// A computed type for `column-count` values.
pub type ColumnCount = GenericColumnCount<PositiveInteger>;
