// Copyright 2026 the Parley Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Built layouts by their inputs.
//!
//! Building a layout (text analysis, shaping, style metrics) depends on the
//! text, the resolved styles and runs, the scale and the base direction. The
//! styles' brushes only matter when painting: a layout built from the same
//! inputs with other brushes is the same layout with those brushes swapped
//! in. Embedders building the same text again (content cloned into another
//! element, a paragraph rebuilt after a sibling changed) reuse it.

use alloc::string::String;
use alloc::vec::Vec;
use core::hash::{BuildHasher, Hash, Hasher};
use core::ops::Range;

use fontique::FamilyId;
use parlance::{BaseDirection, FontFeature, FontVariation};

use crate::layout::data::LayoutData;
use crate::resolve::{ResolveContext, ResolvedStyle, StyleRun};
use crate::style::Brush;

/// One style as layout building sees it: its brushes left out and its
/// resolved families, variations and features by value (their handles index
/// tables rebuilt for every layout).
#[derive(PartialEq)]
struct StyleKey<B: Brush> {
    style: ResolvedStyle<B>,
    families: Vec<FamilyId>,
    variations: Vec<FontVariation>,
    features: Vec<FontFeature>,
}

/// The inputs of building one layout.
pub(crate) struct BuildKey<B: Brush> {
    text: String,
    scale: u32,
    quantize: bool,
    base_direction: BaseDirection,
    /// The address of the context's `'static` line-break override, 0 without one.
    line_break_override: usize,
    runs: Vec<(u16, Range<usize>)>,
    styles: Vec<StyleKey<B>>,
}

impl<B: Brush> BuildKey<B> {
    pub(crate) fn new(
        rcx: &ResolveContext,
        style_table: &[ResolvedStyle<B>],
        style_runs: &[StyleRun],
        text: &str,
        scale: f32,
        quantize: bool,
        base_direction: BaseDirection,
        line_break_override: usize,
    ) -> Self {
        let styles = style_table
            .iter()
            .map(|s| {
                let mut style = s.clone();
                style.brush = B::default();
                style.underline.brush = None;
                style.strikethrough.brush = None;
                style.font_family = Default::default();
                style.font_variations = Default::default();
                style.font_features = Default::default();
                StyleKey {
                    families: rcx.stack(s.font_family).unwrap_or_default().to_vec(),
                    variations: rcx.variations(s.font_variations).unwrap_or_default().to_vec(),
                    features: rcx.features(s.font_features).unwrap_or_default().to_vec(),
                    style,
                }
            })
            .collect();
        Self {
            text: text.into(),
            scale: scale.to_bits(),
            quantize,
            base_direction,
            line_break_override,
            runs: style_runs.iter().map(|r| (r.style_index, r.range.clone())).collect(),
            styles,
        }
    }

    fn hash(&self, hasher: &hashbrown::DefaultHashBuilder) -> u64 {
        let mut h = hasher.build_hasher();
        self.text.hash(&mut h);
        self.scale.hash(&mut h);
        self.runs.hash(&mut h);
        self.styles.len().hash(&mut h);
        for s in &self.styles {
            s.style.font_size.to_bits().hash(&mut h);
            s.families.hash(&mut h);
        }
        h.finish()
    }

    fn matches(&self, other: &Self) -> bool {
        self.text == other.text
            && self.scale == other.scale
            && self.quantize == other.quantize
            && self.base_direction == other.base_direction
            && self.line_break_override == other.line_break_override
            && self.runs == other.runs
            && self.styles == other.styles
    }
}

/// Built layouts (before line breaking) by their [`BuildKey`]. Bounded:
/// emptied when it holds [`BuildCache::MAX_ENTRIES`] layouts.
pub(crate) struct BuildCache<B: Brush> {
    entries: hashbrown::HashMap<u64, Vec<(BuildKey<B>, LayoutData<B>)>>,
    len: usize,
    /// One hasher for every key (each `DefaultHashBuilder` is seeded anew).
    hasher: hashbrown::DefaultHashBuilder,
}

impl<B: Brush> Default for BuildCache<B> {
    fn default() -> Self {
        Self { entries: Default::default(), len: 0, hasher: Default::default() }
    }
}

impl<B: Brush> BuildCache<B> {
    const MAX_ENTRIES: usize = 4096;

    pub(crate) fn get(&self, key: &BuildKey<B>) -> Option<&LayoutData<B>> {
        self.entries
            .get(&key.hash(&self.hasher))?
            .iter()
            .find(|(k, _)| k.matches(key))
            .map(|(_, data)| data)
    }

    pub(crate) fn insert(&mut self, key: BuildKey<B>, data: &LayoutData<B>) {
        if self.len >= Self::MAX_ENTRIES {
            self.clear();
        }
        self.len += 1;
        let hash = key.hash(&self.hasher);
        self.entries.entry(hash).or_default().push((key, data.clone()));
    }

    pub(crate) fn clear(&mut self) {
        self.entries.clear();
        self.len = 0;
    }
}
