// Copyright 2026 the Parley Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Text shaping utilities.

pub(crate) mod atom;
mod cache;
mod cluster;
mod data;
mod east_asian_spacing;
pub(crate) mod han_kerning;
pub(crate) mod shaped_text;
pub(crate) mod shaper;

pub use cluster::{Char, CharCluster, Coverage, SourceRange, Whitespace};
pub use data::{Character, CharacterFlags, ShapedCluster};
pub use east_asian_spacing::{EastAsianSpacing, east_asian_spacing, is_cursive, is_format, is_mark};
pub use han_kerning::SpacingTrim;

pub(crate) use data::ShapedClusterFlags;
