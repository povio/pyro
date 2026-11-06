// Copyright 2026 the Parley Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Resuming line breaking after the text of a layout changed.
//!
//! Breaking is greedy: a line's break depends only on the content up to the atom that overflowed
//! it. When a layout is built again from text that shares a prefix with the text it had (text
//! appended, truncated or inserted after the start), the lines its last break produced over that
//! prefix are still the lines a new break would produce, provided the shaped content the breaker
//! read for them is the same. [`adopt_line_prefix`] keeps those lines when the layout is built
//! again, and [`BreakLines::resume`](super::BreakLines::resume) continues breaking from the first
//! line after them, at the breaker state recorded at that line's start.

use alloc::vec::Vec;
use core::ops::Range;

use parlance::BidiLevel;

use crate::IndentOptions;
use crate::TextWrapMode;
use crate::layout::data::{
    AlignedSubtreeOffset, LayoutData, LayoutItemKind, LineData, LineItemData,
};
use crate::style::Brush;

/// The line breaker's state at the start of a line, recorded as the line is broken.
///
/// [`LayoutData::line_starts`] holds one per line of the last break, or none when the caller
/// adjusted the breaker in a way a resumed break cannot repeat.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LineStart {
    pub(crate) item_idx: usize,
    pub(crate) run_idx: usize,
    pub(crate) cluster_idx: u32,
    pub(crate) items: Range<usize>,
    pub(crate) clusters: Range<u32>,
    /// The `text-wrap-mode` the breaker lags by one atom.
    pub(crate) text_wrap_mode: TextWrapMode,
    pub(crate) line_x: f32,
    pub(crate) line_y: f64,
    pub(crate) layout_max_advance: f32,
    pub(crate) line_max_advance: f32,
    pub(crate) line_max_height: f32,
    /// The end of the shaped clusters the breaker had read before this line (exclusive): all
    /// earlier lines were decided by the clusters before it.
    pub(crate) read_end: u32,
}

/// The inputs of the break that produced adopted lines, besides the per-line geometry in their
/// [`LineStart`]s.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct BreakInputs {
    pub(crate) layout_max_advance: f32,
    pub(crate) indent_amount: f32,
    pub(crate) indent_options: IndentOptions,
}

/// Lines kept from the layout's previous build by [`adopt_line_prefix`], until the next break
/// continues after them ([`BreakLines::resume`](super::BreakLines::resume)) or drops them.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct AdoptedLines {
    /// `Some` iff lines were adopted.
    pub(crate) inputs: Option<BreakInputs>,
    pub(crate) lines: Vec<LineData>,
    pub(crate) line_items: Vec<LineItemData>,
    pub(crate) aligned_subtree_offsets: Vec<AlignedSubtreeOffset>,
    /// One more than `lines`: the last is the start of the first line to break again.
    pub(crate) line_starts: Vec<LineStart>,
}

impl AdoptedLines {
    pub(crate) fn clear(&mut self) {
        self.inputs = None;
        self.lines.clear();
        self.line_items.clear();
        self.aligned_subtree_offsets.clear();
        self.line_starts.clear();
    }
}

/// The number of leading shaped clusters of `new` that the line breaker reads exactly as it read
/// them in `old`, along with everything else it reads for them (characters, glyphs, runs, items,
/// styles). `0` when the layouts differ in anything that applies to every line.
fn equal_cluster_prefix<B: Brush>(old: &LayoutData<B>, new: &LayoutData<B>) -> u32 {
    // LTR text without inline boxes only: bidi text reorders lines, `dir=auto` resolves the base
    // level from the text, and inline boxes are sized by the caller.
    if old.base_level != BidiLevel::new(0)
        || new.base_level != BidiLevel::new(0)
        || old.scale != new.scale
        || old.quantize != new.quantize
        || !old.inline_boxes.is_empty()
        || !new.inline_boxes.is_empty()
        || old.styles != new.styles
        || old.style_metrics != new.style_metrics
    {
        return 0;
    }
    let (ot, nt) = (&old.shaped_text, &new.shaped_text);
    let (oc, nc) = (ot.shaped_clusters(), nt.shaped_clusters());
    let (ochars, nchars) = (ot.characters(), nt.characters());
    let (og, ng) = (ot.glyphs(), nt.glyphs());
    let mut equal = 0_u32;
    for (i, (orun, nrun)) in ot.runs().iter().zip(nt.runs()).enumerate() {
        // Everything about the run but where it ends (and so its total advance). Run-wide data
        // (`RunData`'s box metrics, font fallback) must be the same too.
        let same_run = orun.bidi_level == BidiLevel::new(0)
            && nrun.bidi_level == BidiLevel::new(0)
            && orun.range.byte_range.start == nrun.range.byte_range.start
            && orun.range.char_range.start == nrun.range.char_range.start
            && orun.font_size == nrun.font_size
            && orun.font_index == nrun.font_index
            && ot.fonts().get(orun.font_index) == nt.fonts().get(nrun.font_index)
            && orun.characters_range.start == nrun.characters_range.start
            && orun.shaped_clusters_range.start == nrun.shaped_clusters_range.start
            && orun.glyphs_range.start == nrun.glyphs_range.start
            && ot.normalized_coords().get(orun.normalized_coords_range.clone())
                == nt.normalized_coords().get(nrun.normalized_coords_range.clone())
            && orun.font_metrics == nrun.font_metrics
            && old.runs.get(i) == new.runs.get(i)
            && old.items.get(i) == new.items.get(i)
            && old
                .items
                .get(i)
                .is_some_and(|item| item.kind == LayoutItemKind::TextRun && item.index == i);
        if !same_run {
            return equal;
        }
        let end = orun
            .shaped_clusters_range
            .end
            .min(nrun.shaped_clusters_range.end);
        while equal < end {
            let (a, b) = (&oc[equal as usize], &nc[equal as usize]);
            if a != b {
                return equal;
            }
            let chars = a.chars_range();
            let chars = chars.start as usize..chars.end as usize;
            if ochars.get(chars.clone()) != nchars.get(chars) {
                return equal;
            }
            if !a.has_inline_glyph() {
                let glyphs =
                    a.glyph_offset as usize..a.glyph_offset as usize + a.glyph_len() as usize;
                if og.get(glyphs.clone()) != ng.get(glyphs) {
                    return equal;
                }
            }
            equal += 1;
        }
        if orun.shaped_clusters_range.end != nrun.shaped_clusters_range.end {
            return equal;
        }
    }
    equal
}

/// The end of `line`'s shaped clusters (its items are all text runs here).
#[cfg(debug_assertions)]
fn line_cluster_end(line: &LineData, line_items: &[LineItemData]) -> u32 {
    line_items[line.item_range.clone()]
        .iter()
        .map(|item| item.shaped_cluster_range.end)
        .max()
        .unwrap_or(0)
}

/// Keeps, in `new.adopted`, the leading lines of `old`'s last break that a break of `new` with
/// the same inputs would produce again, and returns how many.
///
/// The breaker decides a line's end once an atom overflowed it, and that atom can lie many lines
/// on (a long word broken at emergency opportunities over lines narrower than the one it did not
/// fit on). So each recorded line start says how far the breaker had read before it
/// ([`LineStart::read_end`]), and line `k` is resumed from when everything read before it is the
/// same in both layouts. The breaker state at the start of that line is kept with the lines
/// before it, so that breaking can continue from there.
pub(crate) fn adopt_line_prefix<B: Brush>(new: &mut LayoutData<B>, old: &LayoutData<B>) -> usize {
    new.adopted.clear();
    let line_count = old.lines.len();
    // `line_starts` is only complete when the old break can be repeated.
    if line_count < 2
        || old.line_starts.len() != line_count
        || old.text_len == 0
        || new.text_len == 0
        || !old.layout_max_advance.is_finite()
        || old.layout_max_advance >= f32::MAX
    {
        return 0;
    }
    let equal = equal_cluster_prefix(old, new);
    if equal == 0 {
        return 0;
    }
    // The line starts after the first that the breaker reached reading only the equal prefix
    // (`read_end` only grows from line to line).
    let kept = old.line_starts[1..]
        .iter()
        .take_while(|start| start.read_end <= equal)
        .count();
    if kept == 0 {
        return 0;
    }
    #[cfg(debug_assertions)]
    debug_assert!(
        line_cluster_end(&old.lines[kept - 1], &old.line_items) <= equal,
        "a kept line ends past what was read before the next one"
    );
    let last = &old.lines[kept - 1];
    let adopted = &mut new.adopted;
    adopted.inputs = Some(BreakInputs {
        layout_max_advance: old.layout_max_advance,
        indent_amount: old.indent_amount,
        indent_options: old.indent_options,
    });
    adopted.lines.extend_from_slice(&old.lines[..kept]);
    adopted
        .line_items
        .extend_from_slice(&old.line_items[..last.item_range.end]);
    adopted.aligned_subtree_offsets.extend_from_slice(
        &old.aligned_subtree_offsets[..last.aligned_subtree_offsets.end as usize],
    );
    adopted
        .line_starts
        .extend_from_slice(&old.line_starts[..=kept]);
    kept
}
