// Copyright 2025 the Parley Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

use alloc::boxed::Box;
use hashbrown::Equivalent;
use parlance::FontVariation;

#[derive(PartialEq, Copy, Clone)]
pub(crate) struct ShapeDataKey {
    /// The font collection's blob ID.
    font_blob_id: u64,
    /// The font's index in the font collection.
    font_index: u32,
}

impl ShapeDataKey {
    pub(crate) const fn new(font_blob_id: u64, font_index: u32) -> Self {
        Self {
            font_blob_id,
            font_index,
        }
    }
}

impl Equivalent<Self> for ShapeDataKey {
    #[inline(always)]
    fn equivalent(&self, key: &Self) -> bool {
        self == key
    }
}

impl From<&Self> for ShapeDataKey {
    #[inline(always)]
    fn from(key: &Self) -> Self {
        *key
    }
}

pub(crate) struct ShapeInstanceId {
    /// The font collection's blob ID.
    font_blob_id: u64,
    /// The font's index in the font collection.
    font_index: u32,
    synthesis: fontique::Synthesis,
    variations: Option<Box<[FontVariation]>>,
}

pub(crate) struct ShapeInstanceKey<'a> {
    /// The font collection's blob ID.
    font_blob_id: u64,
    /// The font's index in the font collection.
    font_index: u32,
    synthesis: &'a fontique::Synthesis,
    variations: Option<&'a [FontVariation]>,
}

impl<'a> ShapeInstanceKey<'a> {
    pub(crate) const fn new(
        font_blob_id: u64,
        font_index: u32,
        synthesis: &'a fontique::Synthesis,
        variations: Option<&'a [FontVariation]>,
    ) -> Self {
        Self {
            font_blob_id,
            font_index,
            synthesis,
            variations,
        }
    }
}

impl<'a> Equivalent<ShapeInstanceId> for ShapeInstanceKey<'a> {
    #[inline(always)]
    fn equivalent(&self, key: &ShapeInstanceId) -> bool {
        self.font_blob_id == key.font_blob_id
            && self.font_index == key.font_index
            && *self.synthesis == key.synthesis
            && self.variations == key.variations.as_deref()
    }
}

impl<'a> From<ShapeInstanceKey<'a>> for ShapeInstanceId {
    #[inline(always)]
    fn from(key: ShapeInstanceKey<'a>) -> Self {
        Self {
            font_blob_id: key.font_blob_id,
            font_index: key.font_index,
            synthesis: *key.synthesis,
            variations: key.variations.map(|v| v.to_vec().into()),
        }
    }
}

pub(crate) struct ShapePlanId {
    /// The font collection's blob ID.
    font_blob_id: u64,
    /// The font's index in the font collection.
    font_index: u32,
    synthesis: fontique::Synthesis,
    direction: harfrust::Direction,
    script: harfrust::Script,
    language: Option<harfrust::Language>,
    features: Box<[harfrust::Feature]>,
    variations: Option<Box<[FontVariation]>>,
}

pub(crate) struct ShapePlanKey<'a> {
    /// The font collection's blob ID.
    font_blob_id: u64,
    /// The font's index in the font collection.
    font_index: u32,
    synthesis: &'a fontique::Synthesis,
    direction: harfrust::Direction,
    script: harfrust::Script,
    language: Option<harfrust::Language>,
    features: &'a [harfrust::Feature],
    variations: Option<&'a [FontVariation]>,
}

impl<'a> ShapePlanKey<'a> {
    pub(crate) const fn new(
        font_blob_id: u64,
        font_index: u32,
        synthesis: &'a fontique::Synthesis,
        direction: harfrust::Direction,
        script: harfrust::Script,
        language: Option<harfrust::Language>,
        features: &'a [harfrust::Feature],
        variations: Option<&'a [FontVariation]>,
    ) -> Self {
        Self {
            font_blob_id,
            font_index,
            synthesis,
            direction,
            script,
            language,
            features,
            variations,
        }
    }
}

impl<'a> Equivalent<ShapePlanId> for ShapePlanKey<'a> {
    #[inline(always)]
    fn equivalent(&self, key: &ShapePlanId) -> bool {
        self.font_blob_id == key.font_blob_id
            && self.font_index == key.font_index
            && *self.synthesis == key.synthesis
            && self.direction == key.direction
            && self.script == key.script
            && self.language == key.language
            && self.features.len() == key.features.len()
            && self.variations == key.variations.as_deref()
            && self
                .features
                .iter()
                .zip(key.features.iter())
                .all(|(a, b)| a == b)
    }
}

impl<'a> From<ShapePlanKey<'a>> for ShapePlanId {
    #[inline(always)]
    fn from(key: ShapePlanKey<'a>) -> Self {
        Self {
            font_blob_id: key.font_blob_id,
            font_index: key.font_index,
            synthesis: *key.synthesis,
            direction: key.direction,
            script: key.script,
            language: key.language,
            features: key.features.to_vec().into(),
            variations: key.variations.map(|v| v.to_vec().into()),
        }
    }
}

/// The inputs of shaping one run (see [`GlyphCache`]).
pub(crate) struct GlyphKey<'a> {
    pub font_blob_id: u64,
    pub font_index: u32,
    pub synthesis: &'a fontique::Synthesis,
    pub variations: &'a [FontVariation],
    pub features: &'a [harfrust::Feature],
    pub direction: harfrust::Direction,
    pub script: harfrust::Script,
    pub language: Option<&'a harfrust::Language>,
    pub font_size: f32,
    pub text: &'a str,
    /// The text around the run that shaping looks at.
    pub pre_context: &'a str,
    pub post_context: &'a str,
}

impl GlyphKey<'_> {
    fn hash(&self, hasher: &hashbrown::DefaultHashBuilder) -> u64 {
        use core::hash::{BuildHasher, Hash, Hasher};
        let mut h = hasher.build_hasher();
        self.font_blob_id.hash(&mut h);
        self.font_index.hash(&mut h);
        self.features.hash(&mut h);
        self.direction.hash(&mut h);
        self.script.hash(&mut h);
        self.language.hash(&mut h);
        self.font_size.to_bits().hash(&mut h);
        self.text.hash(&mut h);
        self.pre_context.hash(&mut h);
        self.post_context.hash(&mut h);
        h.finish()
    }
}

struct GlyphEntry {
    font_blob_id: u64,
    font_index: u32,
    synthesis: fontique::Synthesis,
    variations: Box<[FontVariation]>,
    features: Box<[harfrust::Feature]>,
    direction: harfrust::Direction,
    script: harfrust::Script,
    language: Option<harfrust::Language>,
    font_size: u32,
    text: Box<str>,
    pre_context: Box<str>,
    post_context: Box<str>,
    glyphs: harfrust::GlyphBuffer,
}

impl GlyphEntry {
    fn matches(&self, key: &GlyphKey<'_>) -> bool {
        self.font_blob_id == key.font_blob_id
            && self.font_index == key.font_index
            && self.synthesis == *key.synthesis
            && *self.variations == *key.variations
            && *self.features == *key.features
            && self.direction == key.direction
            && self.script == key.script
            && self.language.as_ref() == key.language
            && self.font_size == key.font_size.to_bits()
            && *self.text == *key.text
            && *self.pre_context == *key.pre_context
            && *self.post_context == *key.post_context
    }
}

/// Shaped runs by their shaping inputs. Bounded: emptied when it holds
/// [`GlyphCache::MAX_ENTRIES`] runs.
#[derive(Default)]
pub(crate) struct GlyphCache {
    entries: hashbrown::HashMap<u64, alloc::vec::Vec<GlyphEntry>>,
    len: usize,
    /// One hasher for every key (each `DefaultHashBuilder` is seeded anew).
    hasher: hashbrown::DefaultHashBuilder,
}

impl GlyphCache {
    const MAX_ENTRIES: usize = 4096;

    pub(crate) fn get(&self, key: &GlyphKey<'_>) -> Option<&harfrust::GlyphBuffer> {
        self.entries
            .get(&key.hash(&self.hasher))?
            .iter()
            .find(|e| e.matches(key))
            .map(|e| &e.glyphs)
    }

    pub(crate) fn insert(&mut self, key: &GlyphKey<'_>, glyphs: harfrust::GlyphBuffer) {
        if self.len >= Self::MAX_ENTRIES {
            self.entries.clear();
            self.len = 0;
        }
        self.len += 1;
        let hash = key.hash(&self.hasher);
        self.entries.entry(hash).or_default().push(GlyphEntry {
            font_blob_id: key.font_blob_id,
            font_index: key.font_index,
            synthesis: *key.synthesis,
            variations: key.variations.into(),
            features: key.features.into(),
            direction: key.direction,
            script: key.script,
            language: key.language.cloned(),
            font_size: key.font_size.to_bits(),
            text: key.text.into(),
            pre_context: key.pre_context.into(),
            post_context: key.post_context.into(),
            glyphs,
        });
    }
}
