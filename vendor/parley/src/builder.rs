// Copyright 2021 the Parley Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Context for layout.

use super::FontContext;
use super::context::LayoutContext;
use super::style::{Brush, StyleProperty, TextStyle};

use super::layout::Layout;

use alloc::string::String;
use core::ops::{Bound, Range, RangeBounds};
use parlance::BaseDirection;
use parley_engine::break_overrides::LineBreakOverrideFn;

use crate::InlineBoxKind;
use crate::inline_box::{InlineBox, LayoutInlineBox};
use crate::resolve::{ResolvedStyle, StyleRun};

#[derive(Clone, Copy)]
pub(crate) struct BuilderOptions<'a> {
    scale: f32,
    quantize: bool,
    base_direction: BaseDirection,
    line_break_override: Option<&'a LineBreakOverrideFn>,
}

impl BuilderOptions<'_> {
    pub(crate) fn new(scale: f32, quantize: bool) -> Self {
        Self {
            scale,
            quantize,
            base_direction: BaseDirection::Auto,
            line_break_override: None,
        }
    }

    pub(crate) fn with_line_break_override(mut self, overrides: Option<&'static LineBreakOverrideFn>) -> Self {
        self.line_break_override = overrides;
        self
    }
}

/// Builder for constructing a text layout with ranged attributes.
///
/// This builder does not collapse whitespace. For whitespace collapsing, use the [`TreeBuilder`]
#[must_use]
pub struct RangedBuilder<'a, B: Brush> {
    pub(crate) options: BuilderOptions<'a>,
    pub(crate) lcx: &'a mut LayoutContext<B>,
    pub(crate) fcx: &'a mut FontContext,
}

impl<'b, B: Brush> RangedBuilder<'b, B> {
    pub fn push_default<'a>(&mut self, property: impl Into<StyleProperty<'a, B>>) {
        let resolved =
            self.lcx
                .rcx
                .resolve_property(self.fcx, &property.into(), self.options.scale);
        self.lcx.ranged_style_builder.push_default(resolved);
    }

    pub fn push<'a>(
        &mut self,
        property: impl Into<StyleProperty<'a, B>>,
        range: impl RangeBounds<usize>,
    ) {
        let resolved =
            self.lcx
                .rcx
                .resolve_property(self.fcx, &property.into(), self.options.scale);
        self.lcx.ranged_style_builder.push(resolved, range);
    }

    pub fn push_inline_box(&mut self, inline_box: InlineBox) {
        self.lcx.inline_boxes.push(LayoutInlineBox {
            inline_box,
            parent_style_index: 0,
            baseline_offset: 0.,
        });
    }

    /// Sets the paragraph's base direction.
    ///
    /// The default is [`BaseDirection::Auto`], which infers the direction from the text.
    pub fn set_base_direction(&mut self, base_direction: BaseDirection) {
        self.options.base_direction = base_direction;
    }

    /// Set the callback which will be called as a first provider of line breaking decisions.
    ///
    /// See [`LineBreakOverrideFn`] for more details.
    pub fn set_line_break_override(&mut self, overrides: Option<&'b LineBreakOverrideFn>) {
        self.options.line_break_override = overrides;
    }

    pub fn build_into(self, layout: &mut Layout<B>, text: impl AsRef<str>) {
        // Apply RangedStyleBuilder styles directly to style-table/style-run state.
        self.lcx
            .ranged_style_builder
            .finish(&mut self.lcx.style_table, &mut self.lcx.style_runs);

        // Call generic layout builder method
        build_into_layout(layout, text.as_ref(), self.lcx, self.fcx, self.options);
    }

    pub fn build(self, text: impl AsRef<str>) -> Layout<B> {
        let mut layout = Layout::default();
        self.build_into(&mut layout, text);
        layout
    }
}

/// Builder for constructing a text layout from a style table and
/// indexed style runs.
///
/// This builder does not collapse whitespace. For whitespace collapsing, use the [`TreeBuilder`]
#[must_use]
pub struct StyleRunBuilder<'a, B: Brush> {
    pub(crate) options: BuilderOptions<'a>,
    pub(crate) len: usize,
    pub(crate) lcx: &'a mut LayoutContext<B>,
    pub(crate) fcx: &'a mut FontContext,
    pub(crate) cursor: usize,
}

impl<'b, B: Brush> StyleRunBuilder<'b, B> {
    /// Reserves additional capacity for styles and runs.
    ///
    /// This is an optional optimization for callers that know counts
    /// up front; call it before pushing styles and runs to reduce
    /// reallocations.
    pub fn reserve(&mut self, additional_styles: usize, additional_runs: usize) {
        self.lcx.style_table.reserve(additional_styles);
        self.lcx.style_runs.reserve(additional_runs);
    }

    /// Adds a fully-specified style to the shared style table and
    /// returns its index.
    pub fn push_style<'family, 'settings>(
        &mut self,
        style: TextStyle<'family, 'settings, B>,
    ) -> u16 {
        let resolved = self
            .lcx
            .rcx
            .resolve_entire_style_set(self.fcx, &style, self.options.scale);
        let style_index = self.lcx.style_table.len();
        assert!(style_index <= u16::MAX as usize, "too many styles");
        self.lcx.style_table.push(resolved);
        style_index as u16
    }

    /// Adds a style run referencing an entry from the style table.
    ///
    /// Runs must be contiguous and non-overlapping, and must cover
    /// `0..text.len()` once all runs have been added.
    pub fn push_style_run(&mut self, style_index: u16, range: impl RangeBounds<usize>) {
        let range = resolve_range(range, self.len);
        assert!(
            range.start == self.cursor,
            "StyleRunBuilder expects contiguous non-overlapping runs"
        );
        assert!(
            range.start <= range.end,
            "StyleRunBuilder expects ordered ranges"
        );
        assert!(
            (style_index as usize) < self.lcx.style_table.len(),
            "StyleRunBuilder expects style indices that were previously added via push_style"
        );
        self.lcx.style_runs.push(StyleRun {
            style_index,
            range: range.clone(),
        });
        self.cursor = range.end;
    }

    pub fn push_inline_box(&mut self, inline_box: InlineBox) {
        self.lcx.inline_boxes.push(LayoutInlineBox {
            inline_box,
            parent_style_index: 0,
            baseline_offset: 0.,
        });
    }

    /// Sets the paragraph's base direction.
    ///
    /// The default is [`BaseDirection::Auto`], which infers the direction from the text.
    pub fn set_base_direction(&mut self, base_direction: BaseDirection) {
        self.options.base_direction = base_direction;
    }

    /// Set the callback which will be called as a first provider of line breaking decisions.
    ///
    /// See [`LineBreakOverrideFn`] for more details.
    pub fn set_line_break_override(&mut self, overrides: Option<&'b LineBreakOverrideFn>) {
        self.options.line_break_override = overrides;
    }

    pub fn build_into(self, layout: &mut Layout<B>, text: impl AsRef<str>) {
        assert!(
            self.cursor == self.len,
            "StyleRunBuilder requires runs that cover the full text"
        );
        if self.lcx.style_runs.is_empty() {
            // This is reachable for empty text. In this case, the layout still needs a style and an
            // empty style run, e.g. to size a cursor.
            if self.lcx.style_table.is_empty() {
                let style = self.lcx.rcx.resolve_entire_style_set(
                    self.fcx,
                    &TextStyle::default(),
                    self.options.scale,
                );
                self.lcx.style_table.push(style);
            }
            self.lcx.style_runs.push(StyleRun {
                style_index: 0,
                range: 0..0,
            });
        }
        build_into_layout(layout, text.as_ref(), self.lcx, self.fcx, self.options);
    }

    pub fn build(self, text: impl AsRef<str>) -> Layout<B> {
        let mut layout = Layout::default();
        self.build_into(&mut layout, text);
        layout
    }
}

/// Builder for constructing a text layout with a tree of attributes.
#[must_use]
pub struct TreeBuilder<'a, B: Brush> {
    pub(crate) options: BuilderOptions<'a>,
    pub(crate) lcx: &'a mut LayoutContext<B>,
    pub(crate) fcx: &'a mut FontContext,
}

impl<'b, B: Brush> TreeBuilder<'b, B> {
    pub fn push_style_span(&mut self, style: TextStyle<'_, '_, B>) {
        let resolved = self
            .lcx
            .rcx
            .resolve_entire_style_set(self.fcx, &style, self.options.scale);
        self.lcx.tree_style_builder.push_style_span(resolved);
    }

    pub fn push_style_modification_span<'s, 'iter>(
        &mut self,
        properties: impl IntoIterator<Item = &'iter StyleProperty<'s, B>>,
    ) where
        's: 'iter,
        B: 'iter,
    {
        self.lcx
            .tree_style_builder
            .push_style_modification_span(properties.into_iter().map(|p| {
                self.lcx
                    .rcx
                    .resolve_property(self.fcx, p, self.options.scale)
            }));
    }

    pub fn pop_style_span(&mut self) {
        self.lcx.tree_style_builder.pop_style_span();
    }

    pub fn push_text(&mut self, text: &str) {
        self.lcx.tree_style_builder.push_text(text);
    }

    /// pyro: pushes a bidi formatting character (U+202A to U+202E, U+2066 to U+2069) that the
    /// start or the end of an inline element's `unicode-bidi` stands for, in the current span. It
    /// is no content to white space processing: white space collapses across it, and pending
    /// white space that is kept goes before it.
    pub fn push_bidi_control(&mut self, control: char) {
        debug_assert!(crate::resolve::tree::is_bidi_control(control));
        self.lcx.tree_style_builder.push_bidi_control(control);
    }

    /// pyro: pushes the strut of an inline box that holds nothing (U+2061, an invisible
    /// zero-width character with the current span's style), so that the box's font and line
    /// height take part in its line's height. It is no content to white space processing.
    pub fn push_empty_box_strut(&mut self) {
        self.lcx.tree_style_builder.push_empty_box_strut();
    }

    /// Returns the pushed text so far, after whitespace processing, but excluding any trailing
    /// collapsible whitespace.
    ///
    /// This is a prefix of the text that [`Self::build`] will return. Whether that trailing
    /// collapsible whitespace ends up being kept (as a single space) depends on where it is and on
    /// what follows it. See [`Self::has_pending_whitespace`].
    pub fn text(&self) -> &str {
        self.lcx.tree_style_builder.text()
    }

    /// Returns whether [`Self::text`] is followed by collapsible whitespace.
    ///
    /// That white space is not yet part of [`Self::text`]: depending on where it is and what
    /// follows, it is either removed or kept as a single space.
    pub fn has_pending_whitespace(&self) -> bool {
        self.lcx.tree_style_builder.has_pending_whitespace()
    }

    /// pyro: the scale the tree is built at (layout units per CSS pixel).
    pub fn scale(&self) -> f32 {
        self.options.scale
    }

    pub fn push_inline_box(&mut self, mut inline_box: InlineBox) {
        let kind = inline_box.kind;
        match kind {
            InlineBoxKind::InFlow => {
                self.lcx.tree_style_builder.flush_pending_whitespace();
                self.lcx.tree_style_builder.set_last_item_is_inline_box();
            }
            // pyro: white space before a span's start edge is outside the span.
            InlineBoxKind::SpanStart => self.lcx.tree_style_builder.begin_span_start_edge(),
            _ => {}
        }

        // TODO: arrange type better here to factor out the index
        inline_box.index = self.lcx.tree_style_builder.text().len();
        let parent_style_index = self.lcx.tree_style_builder.resolve_current_style_id();
        self.lcx.inline_boxes.push(LayoutInlineBox {
            inline_box,
            parent_style_index,
            baseline_offset: 0.,
        });
        if kind == InlineBoxKind::SpanEnd {
            // pyro: white space pending before a span's end edge is inside the span.
            let box_index = self.lcx.inline_boxes.len() - 1;
            self.lcx.tree_style_builder.pushed_span_end_edge(box_index);
        }
    }

    /// pyro: finishes the tree's text and styles, and puts the span end edges that white space
    /// was kept before behind that white space.
    fn finish_tree(&mut self) -> String {
        let text = self
            .lcx
            .tree_style_builder
            .finish(&mut self.lcx.style_table, &mut self.lcx.style_runs);
        for (box_index, index) in self.lcx.tree_style_builder.take_edge_moves() {
            self.lcx.inline_boxes[box_index].inline_box.index = index;
        }
        text
    }

    /// Sets the paragraph's base direction.
    ///
    /// The default is [`BaseDirection::Auto`], which infers the direction from the text.
    pub fn set_base_direction(&mut self, base_direction: BaseDirection) {
        self.options.base_direction = base_direction;
    }

    /// Set the callback which will be called as a first provider of line breaking decisions.
    ///
    /// See [`LineBreakOverrideFn`] for more details.
    pub fn set_line_break_override(&mut self, overrides: Option<&'b LineBreakOverrideFn>) {
        self.options.line_break_override = overrides;
    }

    #[inline]
    pub fn build_into(mut self, layout: &mut Layout<B>) -> String {
        // Apply TreeStyleBuilder styles to LayoutContext.
        let text = self.finish_tree();

        // Call generic layout builder method
        build_into_layout(layout, &text, self.lcx, self.fcx, self.options);

        text
    }

    #[inline]
    pub fn build(self) -> (Layout<B>, String) {
        let mut layout = Layout::default();
        let text = self.build_into(&mut layout);
        (layout, text)
    }

    /// Like [`Self::build_into`], building `layout` again, but keeps the lines of its last
    /// break that the same break of the new content would produce again: those over the
    /// beginning the old and new content share (see [`crate::layout::resume`]).
    /// [`BreakLines::resume`](crate::layout::BreakLines::resume) continues breaking after them.
    pub fn build_into_keeping_lines(mut self, layout: &mut Layout<B>) -> String {
        let text = self.finish_tree();

        // Build into the scratch's buffers, keeping the old data to compare with.
        core::mem::swap(&mut layout.data, &mut self.lcx.previous_layout);
        build_into_layout(layout, &text, self.lcx, self.fcx, self.options);
        crate::layout::resume::adopt_line_prefix(&mut layout.data, &self.lcx.previous_layout);
        self.lcx.previous_layout.clear();

        text
    }
}

fn build_into_layout<B: Brush>(
    layout: &mut Layout<B>,
    text: &str,
    lcx: &mut LayoutContext<B>,
    fcx: &mut FontContext,
    options: BuilderOptions<'_>,
) {
    if text.is_empty() && lcx.style_runs.is_empty() {
        lcx.style_table.push(ResolvedStyle::default());
        lcx.style_runs.push(StyleRun {
            style_index: 0,
            range: 0..0,
        });
    }
    assert!(
        !lcx.style_runs.is_empty(),
        "at least one style run is required"
    );

    // A layout built before from the same text and styles (cloned content,
    // the same paragraph built again) is reused: only the styles' brushes,
    // which nothing before line breaking depends on, are its own.
    // A layout built with a line-break override is cached only for the context's own `'static`
    // override, whose address is part of the key; another one may not be the same function next
    // time.
    let line_break_override = match options.line_break_override {
        None => Some(0),
        Some(o) => lcx
            .line_break_override
            .filter(|d| core::ptr::addr_eq(*d, o))
            .map(|d| core::ptr::from_ref(d).cast::<()>() as usize),
    };
    let cache_key = line_break_override.filter(|_| lcx.inline_boxes.is_empty()).map(|line_break_override| {
        crate::build_cache::BuildKey::new(&lcx.rcx, &lcx.style_table, &lcx.style_runs, text, options.scale, options.quantize, options.base_direction, line_break_override)
    });
    if let Some(key) = &cache_key
        && let Some(data) = lcx.build_cache.get(key)
    {
        layout.data.clone_from(data);
        layout.data.styles.clear();
        layout
            .data
            .styles
            .extend(lcx.style_table.iter().map(|s| s.as_layout_style()));
        layout.data.inline_boxes.clear();
        return;
    }

    crate::analysis::analyze_text(
        lcx,
        text,
        options.base_direction,
        options.line_break_override,
    );

    layout.data.clear();
    layout.data.scale = options.scale;
    layout.data.quantize = options.quantize;
    layout.data.base_level = lcx.analysis.paragraph_level();
    layout.data.text_len = text.len();

    // Characters not covered by any style run (e.g. the space substituted for empty text during
    // analysis) use the root style.
    lcx.char_style_indices.clear();
    lcx.char_style_indices
        .resize(lcx.analysis.char_info().len(), 0);
    let mut char_index = 0;
    for style_run in &lcx.style_runs {
        for _ in text[style_run.range.clone()].chars() {
            lcx.char_style_indices[char_index] = style_run.style_index;
            char_index += 1;
        }
    }

    // Copy the visual styles into the layout
    layout
        .data
        .styles
        .extend(lcx.style_table.iter().map(|s| s.as_layout_style()));

    // Sort the inline boxes as subsequent code assumes that they are in text index order.
    // Note: It's important that this is a stable sort to allow users to control the order of contiguous inline boxes
    //
    // TODO: consider dropping the sort and requiring `push_inline_box` callers to push boxes in text index order
    // (as `TreeBuilder` already does).
    lcx.inline_boxes.sort_by_key(|b| b.inline_box.index);

    {
        super::shape::shape_text(
            &lcx.rcx,
            fcx,
            &lcx.style_table,
            &lcx.inline_boxes,
            &lcx.analysis,
            &lcx.char_style_indices,
            &mut lcx.scx,
            text,
            layout,
            &lcx.analysis_data_sources,
            &mut lcx.style_first_fonts,
        );
    }

    // After shaping, so that the metrics of fonts used for shaping can be reused.
    crate::layout::style_metrics::resolve_style_metrics(
        &lcx.rcx,
        fcx,
        &lcx.style_table,
        &layout.data.shaped_text,
        &lcx.style_first_fonts,
        if options.quantize { options.scale } else { 0. },
        &mut layout.data.style_metrics,
    );

    // Move inline boxes into the layout
    layout.data.inline_boxes.clear();
    core::mem::swap(&mut layout.data.inline_boxes, &mut lcx.inline_boxes);

    if let Some(key) = cache_key {
        lcx.build_cache.insert(key, &layout.data);
    }
}

fn resolve_range(range: impl RangeBounds<usize>, len: usize) -> Range<usize> {
    let start = match range.start_bound() {
        Bound::Unbounded => 0,
        Bound::Included(n) => *n,
        Bound::Excluded(n) => *n + 1,
    };
    let end = match range.end_bound() {
        Bound::Unbounded => len,
        Bound::Included(n) => *n + 1,
        Bound::Excluded(n) => *n,
    };
    start.min(len)..end.min(len)
}
