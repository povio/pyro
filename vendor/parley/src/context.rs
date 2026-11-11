// Copyright 2021 the Parley Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Context for layout.

use core::ops::Range;

use alloc::{vec, vec::Vec};

use parley_engine::break_overrides::LineBreakOverrideFn;
use parley_engine::{Analysis, AnalysisDataSources, Analyzer, LineBreakConfig, Shaper};

use super::FontContext;
use super::builder::{BuilderOptions, RangedBuilder, StyleRunBuilder};
use super::resolve::tree::TreeStyleBuilder;
use super::resolve::{RangedStyleBuilder, ResolveContext, ResolvedStyle, StyleRun};
use super::style::{Brush, TextStyle};

use crate::builder::TreeBuilder;
use crate::inline_box::LayoutInlineBox;

/// Shared scratch space used when constructing text layouts.
///
/// This type is designed to be a global resource with only one per-application (or per-thread).
pub struct LayoutContext<B: Brush = [u8; 4]> {
    pub(crate) rcx: ResolveContext,
    pub(crate) style_table: Vec<ResolvedStyle<B>>,
    pub(crate) style_runs: Vec<StyleRun>,
    pub(crate) inline_boxes: Vec<LayoutInlineBox>,

    // Reusable text analysis
    pub(crate) analyzer: Analyzer,
    pub(crate) analysis: Analysis,
    pub(crate) line_break: Vec<(Range<usize>, LineBreakConfig)>,
    pub(crate) break_spaces: Vec<Range<usize>>,

    // Reusable style builders (to amortise allocations)
    pub(crate) ranged_style_builder: RangedStyleBuilder<B>,
    pub(crate) tree_style_builder: TreeStyleBuilder<B>,

    /// Style index for each character, parallel to [`Analysis::char_info`].
    pub(crate) char_style_indices: Vec<u16>,
    pub(crate) scx: Shaper,
    /// Per style of the layout being built, the first available font the shaper's font selector
    /// found (lightningpanda; see `shape::FontSelector`), for the style metrics.
    pub(crate) style_first_fonts: Vec<Option<parley_engine::FontInstance>>,

    // Unicode analysis data sources (provided by icu)
    pub(crate) analysis_data_sources: AnalysisDataSources,

    /// Built layouts by their inputs (see [`crate::builder`]).
    pub(crate) build_cache: crate::build_cache::BuildCache<B>,

    /// Scratch for [`TreeBuilder::build_into_keeping_lines`]: the layout data being built
    /// again, while the new data is built and compared with it. Empty between builds.
    ///
    /// [`TreeBuilder::build_into_keeping_lines`]: crate::TreeBuilder::build_into_keeping_lines
    pub(crate) previous_layout: crate::layout::data::LayoutData<B>,

    /// The line-break override every builder of this context starts with
    /// (lightningpanda; see [`Self::set_line_break_override`]).
    pub(crate) line_break_override: Option<&'static LineBreakOverrideFn>,
}

impl<B: Brush> LayoutContext<B> {
    pub fn new() -> Self {
        Self {
            rcx: ResolveContext::default(),
            style_table: vec![],
            style_runs: vec![],
            inline_boxes: vec![],
            analyzer: Analyzer::new(),
            analysis: Analysis::new(),
            line_break: Vec::new(),
            break_spaces: Vec::new(),
            ranged_style_builder: RangedStyleBuilder::default(),
            tree_style_builder: TreeStyleBuilder::default(),
            char_style_indices: vec![],
            analysis_data_sources: AnalysisDataSources::new(),
            scx: Shaper::default(),
            style_first_fonts: vec![],
            build_cache: Default::default(),
            previous_layout: Default::default(),
            line_break_override: None,
        }
    }

    /// Sets the line-break override every builder made from this context starts with (a
    /// builder's own `set_line_break_override` still replaces it). Unlike an override given to
    /// a builder, this one keeps layouts in the build cache: a `'static` override is part of
    /// the cache key.
    pub fn set_line_break_override(&mut self, overrides: Option<&'static LineBreakOverrideFn>) {
        self.line_break_override = overrides;
    }

    /// The override set with [`Self::set_line_break_override`].
    pub fn line_break_override(&self) -> Option<&'static LineBreakOverrideFn> {
        self.line_break_override
    }

    /// Forgets the layouts built so far: the fonts they were shaped with
    /// changed (a font was registered).
    pub fn clear_build_cache(&mut self) {
        self.build_cache.clear();
    }

    fn resolve_style_set(
        &mut self,
        font_ctx: &mut FontContext,
        scale: f32,
        raw_style: &TextStyle<'_, '_, B>,
    ) -> ResolvedStyle<B> {
        self.rcx
            .resolve_entire_style_set(font_ctx, raw_style, scale)
    }

    /// Create a ranged style layout builder.
    ///
    /// Set `quantize` as `true` to have the layout coordinates aligned to pixel boundaries.
    /// That is the easiest way to avoid blurry text and to receive ready-to-paint layout metrics.
    ///
    /// For advanced rendering use cases you can set `quantize` as `false` and receive
    /// fractional coordinates. This ensures the most accurate results if you want to perform
    /// some post-processing on the coordinates before painting. To avoid blurry text you will
    /// still need to quantize the coordinates just before painting.
    ///
    /// Your should round at least the following:
    /// * Glyph run baseline
    /// * Inline box baseline
    ///   - `box.y = (box.y + box.height).round() - box.height`
    /// * Selection geometry's `y0` & `y1`
    /// * Cursor geometry's `y0` & `y1`
    ///
    /// Keep in mind that for the simple `f32::round` to be effective,
    /// you need to first ensure the coordinates are in physical pixel space.
    pub fn ranged_builder<'a>(
        &'a mut self,
        fcx: &'a mut FontContext,
        text: &'a str,
        scale: f32,
        quantize: bool,
    ) -> RangedBuilder<'a, B> {
        self.begin();

        let resolved_root_style = self.resolve_style_set(fcx, scale, &TextStyle::default());
        self.ranged_style_builder
            .begin(resolved_root_style, text.len());

        fcx.source_cache.prune(128, false);

        RangedBuilder {
            options: BuilderOptions::new(scale, quantize).with_line_break_override(self.line_break_override),
            lcx: self,
            fcx,
        }
    }

    /// Create a builder for constructing a layout from indexed style runs.
    ///
    /// Unlike [`Self::ranged_builder`], this builder expects callers to provide:
    /// - a style table of fully specified [`TextStyle`] values (via [`StyleRunBuilder::push_style`])
    /// - a complete sequence of **contiguous**, **non-overlapping** spans that cover
    ///   `0..text.len()` and reference style indices (via [`StyleRunBuilder::push_style_run`])
    ///
    /// Parley then skips its internal range-splitting logic.
    pub fn style_run_builder<'a>(
        &'a mut self,
        fcx: &'a mut FontContext,
        text: &'a str,
        scale: f32,
        quantize: bool,
    ) -> StyleRunBuilder<'a, B> {
        self.begin();

        fcx.source_cache.prune(128, false);

        StyleRunBuilder {
            options: BuilderOptions::new(scale, quantize).with_line_break_override(self.line_break_override),
            len: text.len(),
            lcx: self,
            fcx,
            cursor: 0,
        }
    }

    /// Create a tree style layout builder.
    ///
    /// Set `quantize` as `true` to have the layout coordinates aligned to pixel boundaries.
    /// That is the easiest way to avoid blurry text and to receive ready-to-paint layout metrics.
    ///
    /// For advanced rendering use cases you can set `quantize` as `false` and receive
    /// fractional coordinates. This ensures the most accurate results if you want to perform
    /// some post-processing on the coordinates before painting. To avoid blurry text you will
    /// still need to quantize the coordinates just before painting.
    ///
    /// Your should round at least the following:
    /// * Glyph run baseline
    /// * Inline box baseline
    ///   - `box.y = (box.y + box.height).round() - box.height`
    /// * Selection geometry's `y0` & `y1`
    /// * Cursor geometry's `y0` & `y1`
    ///
    /// Keep in mind that for the simple `f32::round` to be effective,
    /// you need to first ensure the coordinates are in physical pixel space.
    pub fn tree_builder<'a>(
        &'a mut self,
        fcx: &'a mut FontContext,
        scale: f32,
        quantize: bool,
        root_style: &TextStyle<'_, '_, B>,
    ) -> TreeBuilder<'a, B> {
        self.begin();

        let resolved_root_style = self.resolve_style_set(fcx, scale, root_style);
        self.tree_style_builder.begin(resolved_root_style);

        fcx.source_cache.prune(128, false);

        TreeBuilder {
            options: BuilderOptions::new(scale, quantize).with_line_break_override(self.line_break_override),
            lcx: self,
            fcx,
        }
    }

    fn begin(&mut self) {
        self.rcx.clear();
        self.style_table.clear();
        self.style_runs.clear();
        self.inline_boxes.clear();
    }
}

impl<B: Brush> Default for LayoutContext<B> {
    fn default() -> Self {
        Self::new()
    }
}

impl<B: Brush> Clone for LayoutContext<B> {
    fn clone(&self) -> Self {
        // None of the internal state is visible so just return a new instance.
        Self::new()
    }
}
