// Copyright 2026 the Parley Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The code points a font is used for.

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::ops::RangeInclusive;

/// The code points a font is used for: the CSS `@font-face` [`unicode-range`]
/// descriptor.
///
/// A font is a candidate for a character only if its range includes the
/// character, whatever its character map covers. Web font services split a
/// family into one face per script subset (Google Fonts declares cyrillic,
/// greek, latin, ... faces of one family, weight and style), and each subset's
/// character map also covers some characters the other faces are for.
///
/// The default range includes every code point. Cloning is cheap.
///
/// [`unicode-range`]: https://drafts.csswg.org/css-fonts-4/#unicode-range-desc
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct UnicodeRange(Option<Arc<[RangeInclusive<u32>]>>);

impl UnicodeRange {
    /// The range made of `ranges`, which may overlap and come in any order.
    pub fn new(ranges: impl IntoIterator<Item = RangeInclusive<u32>>) -> Self {
        let mut ranges: Vec<RangeInclusive<u32>> = ranges
            .into_iter()
            .filter(|range| !range.is_empty())
            .collect();
        ranges.sort_unstable_by_key(|range| *range.start());
        let mut merged: Vec<RangeInclusive<u32>> = Vec::with_capacity(ranges.len());
        for range in ranges {
            match merged.last_mut() {
                Some(last) if *range.start() <= last.end().saturating_add(1) => {
                    if range.end() > last.end() {
                        *last = *last.start()..=*range.end();
                    }
                }
                _ => merged.push(range),
            }
        }
        Self(Some(merged.into()))
    }

    /// Whether the range includes `code_point`.
    pub fn contains(&self, code_point: u32) -> bool {
        let Some(ranges) = &self.0 else {
            return true;
        };
        // The ranges are ordered and disjoint: only the last one starting at or
        // before `code_point` can include it.
        let after = ranges.partition_point(|range| *range.start() <= code_point);
        after > 0 && code_point <= *ranges[after - 1].end()
    }

    /// Whether the range includes every code point.
    pub fn is_all(&self) -> bool {
        self.0.is_none()
    }

    /// The range's intervals, ordered and disjoint; `None` when it includes
    /// every code point.
    pub fn ranges(&self) -> Option<&[RangeInclusive<u32>]> {
        self.0.as_deref()
    }
}
