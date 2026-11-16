// Copyright 2024 the Parley Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Model for a font.

use crate::CharmapIndex;

use super::source::{SourceInfo, SourceKind};
use super::{Blob, source_cache::SourceCache};
use crate::{FontStyle, FontWeight, FontWidth, UnicodeRange};
use core::fmt;
use read_fonts::{FontRef, TableProvider as _, types::Tag};
use smallvec::SmallVec;

type AxisVec = SmallVec<[AxisInfo; 1]>;

/// Representation of a single font in a family.
#[derive(Clone, Debug)]
pub struct FontInfo {
    source: SourceInfo,
    index: u32,
    width: FontWidth,
    style: FontStyle,
    weight: FontWeight,
    axes: AxisVec,
    attr_axes: u8,
    charmap_index: CharmapIndex,
    unicode_range: UnicodeRange,
    precedence: u64,
    /// pyro: the slant angles (CSS degrees, clockwise positive) a `@font-face` rule's
    /// `font-style` descriptor allows the font: `(0, 0)` for `normal`, the range of `oblique
    /// <angle> <angle>`. `None` without a descriptor: the font's own `slnt` axis decides.
    slant_range: Option<(f32, f32)>,
    /// pyro: the size and metrics a `@font-face` rule's descriptors give the font.
    face_metrics: FaceMetrics,
    /// pyro: the features and variation settings a `@font-face` rule gives the font
    /// ([`FaceSettings`] id, 0 for none).
    face_settings: u32,
    /// pyro: the weights a `@font-face` rule's `font-weight` range gives the font (`min <= max`).
    weight_range: Option<(f32, f32)>,
    /// pyro: the widths (percentages) a `@font-face` rule's `font-stretch` range gives the font.
    width_range: Option<(f32, f32)>,
}

impl FontInfo {
    /// Creates a new font object from the given source and index.
    pub fn from_source(source: SourceInfo, index: u32) -> Option<Self> {
        match &source.kind {
            #[cfg(feature = "std")]
            SourceKind::Path(path) => {
                let file = std::fs::File::open(&**path).ok()?;
                let mapped = unsafe { memmap2::Mmap::map(&file).ok()? };
                let font = FontRef::from_index(&mapped, index).ok()?;
                Self::from_font_ref(&font, source.clone(), index)
            }
            SourceKind::Memory(memory) => {
                let font = FontRef::from_index(memory.as_ref(), index).ok()?;
                Self::from_font_ref(&font, source.clone(), index)
            }
        }
    }

    /// Returns an object describing how to locate the data containing this
    /// font.
    pub fn source(&self) -> &SourceInfo {
        &self.source
    }

    /// Returns the index of the font in a collection.
    pub fn index(&self) -> u32 {
        self.index
    }

    /// Attempts to load the font, optionally from a source cache.
    pub fn load(&self, source_cache: Option<&mut SourceCache>) -> Option<Blob<u8>> {
        if let Some(source_cache) = source_cache {
            source_cache.get(&self.source)
        } else {
            match &self.source.kind {
                SourceKind::Memory(blob) => Some(blob.clone()),
                #[cfg(feature = "std")]
                SourceKind::Path(path) => super::source_cache::load_blob(path),
            }
        }
    }

    /// Returns the visual width of the font-- a relative change from the normal
    /// aspect ratio, typically in the range `0.5` to `2.0`.
    pub fn width(&self) -> FontWidth {
        self.width
    }

    /// Returns the visual style or 'slope' of the font.
    pub fn style(&self) -> FontStyle {
        self.style
    }

    /// Returns the visual weight class of the font, typically on a scale
    /// from `1.0` to `1000.0`.
    pub fn weight(&self) -> FontWeight {
        self.weight
    }

    /// Returns synthesis suggestions for this font with the given attributes.
    pub fn synthesis(&self, width: FontWidth, style: FontStyle, weight: FontWeight) -> Synthesis {
        let mut synth = Synthesis::default();
        let mut len = 0_usize;
        // pyro: a `@font-face` range holds the axes to it (the face is used at the nearest value).
        let width = match self.width_range {
            Some((min, max)) => FontWidth::from_percentage(width.percentage().clamp(min, max)),
            None => width,
        };
        let wanted_weight = weight;
        let weight = match self.weight_range {
            Some((min, max)) => FontWeight::new(weight.value().clamp(min, max)),
            None => weight,
        };
        if self.has_width_axis() && (self.width != width || self.width_range.is_some()) {
            synth.vars[len] = (Tag::new(b"wdth"), width.percentage());
            len += 1;
        }
        // Imagine a caller requests weight 400, but the `wght` axis defaults to
        // 100. Unless we synthesize `wght=400`, shaping will use weight 100. So,
        // when the axis default and requested weight are not the same, synthesize
        // the requested weight for correct shaping.
        //
        // The requested weight and axis default can differ when:
        // 1. A `FontInfoOverride` overrides font weight.
        // 2. A non-conformant font specifies different weights in its OS/2 and `fvar` tables.
        //      - See: <https://learn.microsoft.com/en-us/typography/opentype/spec/dvaraxistag_wght#additional-information>
        if let Some(weight_axis) = self.axes.iter().find(|axis| axis.tag == Tag::new(b"wght")) {
            if weight_axis.default != weight.value() {
                synth.vars[len] = (Tag::new(b"wght"), weight.value());
                len += 1;
            }
            // pyro: a range without bold in it is faked bold, as a face without the axis.
            if self.weight_range.is_some_and(|(_, max)| max < SYNTHETIC_BOLD_WEIGHT)
                && wanted_weight.value() >= SYNTHETIC_BOLD_WEIGHT
            {
                synth.embolden = true;
            }
        } else if wanted_weight.value() >= SYNTHETIC_BOLD_WEIGHT
            && self.weight_range.map_or(self.weight.value(), |(_, max)| max) < SYNTHETIC_BOLD_WEIGHT
        {
            // pyro: as Chromium does (upstream: a requested weight more than 200 above the
            // font's, which left 600 on a regular face thin). See README.pyro.md.
            synth.embolden = true;
        }
        // pyro: a face whose `font-style` range is all oblique (`oblique 45deg 67.5deg`) is used
        // at the angle of its range nearest the one asked for (0deg for normal, 14deg for italic).
        let oblique_range = match (self.style, self.slant_range) {
            (FontStyle::Oblique(_), Some((a, b))) if a != b && self.has_slant_axis() => Some((a.min(b), a.max(b))),
            _ => None,
        };
        if let Some((min, max)) = oblique_range {
            let wanted = match style {
                FontStyle::Normal => 0.0,
                FontStyle::Italic => 14.0,
                FontStyle::Oblique(angle) => angle.unwrap_or(14.0),
            };
            synth.vars[len] = (Tag::new(b"slnt"), -wanted.clamp(min, max));
            len += 1;
        } else if style == FontStyle::Italic
            && self.style == FontStyle::Italic
            && self.has_italic_axis()
            && self.slant_range.is_none()
        {
            // pyro: a face a `@font-face` rule declares italic, of a variable font with an `ital`
            // axis, is used at `ital` 1 (its default instance is the upright one).
            synth.vars[len] = (Tag::new(b"ital"), 1.0);
            len += 1;
        } else if self.style != style {
            match style {
                FontStyle::Normal => {}
                // pyro: the `slnt` axis counts counter-clockwise, CSS angles clockwise: `oblique
                // 10deg` is `slnt -10` (upstream set `slnt 14` for italic, a backslant). And a
                // `@font-face` rule's `font-style` says how far the axis may go (`slant_range`):
                // `normal` not at all (the slant is then faked), a range clamps the angle.
                FontStyle::Italic => {
                    if self.style == FontStyle::Normal {
                        if self.has_italic_axis() && self.slant_range.is_none() {
                            synth.vars[len] = (Tag::new(b"ital"), 1.0);
                            len += 1;
                        } else if let Some(slnt) = self.slant_axis_value(14.0) {
                            synth.vars[len] = (Tag::new(b"slnt"), slnt);
                            len += 1;
                        } else {
                            synth.skew = 14;
                        }
                    }
                }
                FontStyle::Oblique(angle) => {
                    if self.style == FontStyle::Normal {
                        let degrees = angle.unwrap_or(14.0);
                        if let Some(slnt) = self.slant_axis_value(degrees) {
                            synth.vars[len] = (Tag::new(b"slnt"), slnt);
                            len += 1;
                            // pyro: an oblique face of a font with both axes is the upright
                            // design slanted: `ital` 0, whatever the font's default instance.
                            if self.has_italic_axis() {
                                synth.vars[len] = (Tag::new(b"ital"), 0.0);
                                len += 1;
                            }
                        } else if self.has_italic_axis() && degrees > 0. && self.slant_range.is_none() {
                            synth.vars[len] = (Tag::new(b"ital"), 1.0);
                            len += 1;
                        } else {
                            synth.skew = degrees as i8;
                        }
                    }
                }
            }
        }
        synth.len = len as u8;
        synth.face_metrics = self.face_metrics;
        synth.face_settings = self.face_settings;
        synth
    }

    /// Returns the variation [axes] for the font.
    ///
    /// [axes]: crate::AxisInfo
    pub fn axes(&self) -> &[AxisInfo] {
        &self.axes
    }

    /// Returns `true` if the font has a `wght` [axis].
    ///
    /// This is a quicker check than scanning the [axes].
    ///
    /// [axes]: Self::axes
    /// [axis]: crate::AxisInfo
    pub fn has_weight_axis(&self) -> bool {
        self.attr_axes & WEIGHT_AXIS != 0
    }

    /// Returns `true` if the font has a `wdth` [axis].
    ///
    /// This is a quicker check than scanning the [axes].
    ///
    /// [axes]: Self::axes
    /// [axis]: crate::AxisInfo
    pub fn has_width_axis(&self) -> bool {
        self.attr_axes & WIDTH_AXIS != 0
    }

    /// Returns `true` if the font has a `slnt` [axis].
    ///
    /// This is a quicker check than scanning the [axes].
    ///
    /// [axes]: Self::axes
    /// [axis]: crate::AxisInfo
    pub fn has_slant_axis(&self) -> bool {
        self.attr_axes & SLANT_AXIS != 0
    }

    /// Returns `true` if the font has an `ital` [axis].
    ///
    /// This is a quicker check than scanning the [axes].
    ///
    /// [axes]: Self::axes
    /// [axis]: crate::AxisInfo
    pub fn has_italic_axis(&self) -> bool {
        self.attr_axes & ITALIC_AXIS != 0
    }

    /// Returns `true` if the font as an `opsz` [axis].
    ///
    /// This is a quicker check than scanning the [axes].
    ///
    /// [axes]: Self::axes
    /// [axis]: crate::AxisInfo
    pub fn has_optical_size_axis(&self) -> bool {
        self.attr_axes & OPTICAL_SIZE_AXIS != 0
    }

    /// Returns the index used for constructing a [Charmap] for this font.
    ///
    /// [Charmap]: crate::Charmap
    pub fn charmap_index(&self) -> CharmapIndex {
        self.charmap_index
    }

    /// Returns the code points this font is used for (its CSS
    /// `unicode-range`): every code point unless
    /// [overridden](FontInfoOverride::unicode_range).
    pub fn unicode_range(&self) -> &UnicodeRange {
        &self.unicode_range
    }

    /// Returns the font's [precedence](FontInfoOverride::precedence) among the
    /// fonts of its family that match a query equally (0 unless overridden).
    pub fn precedence(&self) -> u64 {
        self.precedence
    }
}

impl FontInfo {
    pub(crate) fn from_font_ref(
        font: &FontRef<'_>,
        source: SourceInfo,
        index: u32,
    ) -> Option<Self> {
        // It's probably not useful to retain fonts that don't have
        // a valid cmap so just bail here if we fail.
        let charmap_index = CharmapIndex::new(font)?;
        let (width, style, weight) = read_attributes(font);
        let (axes, attr_axes) = if let Ok(fvar_axes) = font.fvar().and_then(|fvar| fvar.axes()) {
            let mut axes = SmallVec::<[AxisInfo; 1]>::with_capacity(fvar_axes.len());
            let mut attrs_axes = 0_u8;
            for fvar_axis in fvar_axes {
                let axis = AxisInfo {
                    tag: fvar_axis.axis_tag(),
                    min: fvar_axis.min_value().to_f32(),
                    max: fvar_axis.max_value().to_f32(),
                    default: fvar_axis.default_value().to_f32(),
                };
                axes.push(axis);
                match &axis.tag.to_be_bytes() {
                    b"wght" => attrs_axes |= WEIGHT_AXIS,
                    b"wdth" => attrs_axes |= WIDTH_AXIS,
                    b"slnt" => attrs_axes |= SLANT_AXIS,
                    b"ital" => attrs_axes |= ITALIC_AXIS,
                    b"opsz" => attrs_axes |= OPTICAL_SIZE_AXIS,
                    _ => {}
                }
            }
            (axes, attrs_axes)
        } else {
            (SmallVec::default(), 0)
        };
        Some(Self {
            source,
            index,
            width,
            style,
            weight,
            axes,
            attr_axes,
            charmap_index,
            unicode_range: UnicodeRange::default(),
            precedence: 0,
            slant_range: None,
            face_metrics: FaceMetrics::default(),
            face_settings: 0,
            weight_range: None,
            width_range: None,
        })
    }

    #[allow(unused)]
    pub(crate) fn maybe_override_attributes(
        &mut self,
        width: FontWidth,
        style: FontStyle,
        weight: FontWeight,
    ) {
        if self.width == FontWidth::default() {
            self.width = width;
        }
        if self.style == FontStyle::default() {
            self.style = style;
        }
        if self.weight == FontWeight::default() {
            self.weight = weight;
        }
    }

    /// pyro: the `slnt` axis value that slants the font `degrees` clockwise, when the font has
    /// the axis and its `@font-face` rule lets it be used (see `slant_range`).
    /// pyro: the weight font matching takes the face to have for a request for `weight`: the
    /// nearest of its `@font-face` range, else its own (CSS Fonts 4 §5.2).
    pub(crate) fn matched_weight(&self, weight: f32) -> f32 {
        match self.weight_range {
            Some((min, max)) => weight.clamp(min, max),
            None => self.weight.value(),
        }
    }

    /// pyro: the width (percentage) font matching takes the face to have, as `matched_weight`.
    pub(crate) fn matched_width(&self, width: f32) -> f32 {
        match self.width_range {
            Some((min, max)) => width.clamp(min, max),
            None => self.width.percentage(),
        }
    }

    /// pyro: the style font matching takes the face to have for a request for `style`: an
    /// oblique range (`font-style: oblique 20deg 40deg`) gives the angle of it nearest the one
    /// asked for (14deg for italic, 0deg for normal).
    pub(crate) fn matched_style(&self, style: FontStyle) -> FontStyle {
        let Some((a, b)) = self.slant_range.filter(|(a, b)| a != b) else {
            return self.style;
        };
        if self.style == FontStyle::Italic {
            return self.style;
        }
        let wanted = match style {
            FontStyle::Normal => 0.0,
            FontStyle::Italic => 14.0,
            FontStyle::Oblique(angle) => angle.unwrap_or(14.0),
        };
        let angle = wanted.clamp(a.min(b), a.max(b));
        if angle == 0.0 { FontStyle::Normal } else { FontStyle::Oblique(Some(angle)) }
    }

    fn slant_axis_value(&self, degrees: f32) -> Option<f32> {
        if !self.has_slant_axis() {
            return None;
        }
        match self.slant_range {
            None => Some(-degrees),
            Some((min, max)) if min == max => None,
            Some((min, max)) => Some(-degrees.clamp(min.min(max), min.max(max))),
        }
    }

    pub(crate) fn apply_override(&mut self, info_override: &FontInfoOverride<'_>) {
        if let Some(width) = info_override.width {
            self.width = width;
        }
        if let Some(style) = info_override.style {
            self.style = style;
        }
        if let Some(weight) = info_override.weight {
            self.weight = weight;
        }
        if let Some(axes) = info_override.axes {
            // This is O(n^2) but no font should have enough axes for that to
            // matter (read-fonts does the same thing)
            for (tag, value) in axes {
                if let Some(axis) = self.axes.iter_mut().find(|axis| axis.tag == *tag) {
                    axis.default = *value;
                }
            }
        }
        if let Some(unicode_range) = info_override.unicode_range {
            self.unicode_range = unicode_range.clone();
        }
        if let Some(precedence) = info_override.precedence {
            self.precedence = precedence;
        }
        if let Some(slant_range) = info_override.slant_range {
            self.slant_range = Some(slant_range);
        }
        if let Some(face_metrics) = info_override.face_metrics {
            self.face_metrics = face_metrics;
        }
        if let Some(face_settings) = info_override.face_settings {
            self.face_settings = face_settings;
        }
        if let Some((a, b)) = info_override.weight_range {
            self.weight_range = Some((a.min(b), a.max(b)));
        }
        if let Some((a, b)) = info_override.width_range {
            self.width_range = Some((a.min(b), a.max(b)));
        }
    }
}

/// pyro: the weight from which a request counts as bold and a face as having it. Bold is
/// synthesized for a request at or above it on a face (without a `wght` axis) below it, which
/// is Chromium's rule (`FontSelectionAlgorithm`'s bold threshold, 600).
const SYNTHETIC_BOLD_WEIGHT: f32 = 600.0;

const WEIGHT_AXIS: u8 = 0x01;
const WIDTH_AXIS: u8 = 0x02;
const SLANT_AXIS: u8 = 0x04;
const ITALIC_AXIS: u8 = 0x08;
const OPTICAL_SIZE_AXIS: u8 = 0x10;

/// An axis of variation for a variable font.
///
/// Instances of this can be obtained from [`FontInfo::axes`].
///
/// These give the [`Tag`] and range of valid values for a given font
/// variation. In `parley`, these values are used to create a
/// `FontVariation`.
///
/// OpenType defines some common axes:
///
/// * [Italic](https://fonts.google.com/knowledge/glossary/italic_axis) or `ital`
/// * [Optical Size](https://fonts.google.com/knowledge/glossary/optical_size_axis) or `opsz`
/// * [Slant](https://fonts.google.com/knowledge/glossary/slant_axis) or `slnt`
/// * [FontWeight](https://fonts.google.com/knowledge/glossary/weight_axis) or `wght`
/// * [Width](https://fonts.google.com/knowledge/glossary/width_axis) or `wdth`
///
/// For a broader explanation of this, see
/// [Axis in Variable Fonts](https://fonts.google.com/knowledge/glossary/axis_in_variable_fonts)
/// from Google Fonts.
#[derive(Copy, Clone, Default, Debug)]
pub struct AxisInfo {
    /// The tag that identifies the axis.
    pub tag: Tag,
    /// The inclusive minimum value of the axis.
    pub min: f32,
    /// The inclusive maximum value of the axis.
    pub max: f32,
    /// The default value of the axis.
    pub default: f32,
}

/// Suggestions for synthesizing a set of font attributes for a given
/// font.
///
/// Instances of this can be obtained from [`FontInfo::synthesis`]
/// as well as [`QueryFont::synthesis`].
///
/// [`QueryFont::synthesis`]: crate::QueryFont::synthesis
#[derive(Copy, Clone, Default, PartialEq)]
pub struct Synthesis {
    // pyro: four, for `wdth`, `wght`, `slnt` and `ital` together.
    vars: [(Tag, f32); 4],
    len: u8,
    embolden: bool,
    skew: i8,
    /// pyro: the font's `@font-face` size and metrics descriptors (they travel with the font
    /// a query selects, as its synthesis does).
    face_metrics: FaceMetrics,
    /// pyro: the font's `@font-face` features and variation settings ([`FaceSettings`] id).
    face_settings: u32,
}

/// pyro: what a `@font-face` rule's `font-feature-settings` and `font-variation-settings`
/// descriptors give a face: its own features and variation settings, which the properties of the
/// same names override (CSS Fonts 4 §7.2). Faces carry them as an interned id
/// ([`FaceSettings::intern`]), so that [`Synthesis`] stays `Copy`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FaceSettings {
    /// OpenType features (tags) and their values.
    pub features: alloc::vec::Vec<([u8; 4], u16)>,
    /// Variation axis values (by axis tag).
    pub variations: alloc::vec::Vec<([u8; 4], f32)>,
}

#[cfg(feature = "std")]
static FACE_SETTINGS: std::sync::Mutex<alloc::vec::Vec<alloc::sync::Arc<FaceSettings>>> =
    std::sync::Mutex::new(alloc::vec::Vec::new());

impl FaceSettings {
    /// The id of these settings (equal settings share one), 0 for none.
    pub fn intern(self) -> u32 {
        if self.features.is_empty() && self.variations.is_empty() {
            return 0;
        }
        #[cfg(feature = "std")]
        {
            let mut all = FACE_SETTINGS.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(index) = all.iter().position(|settings| **settings == self) {
                return index as u32 + 1;
            }
            all.push(alloc::sync::Arc::new(self));
            all.len() as u32
        }
        #[cfg(not(feature = "std"))]
        0
    }

    /// The settings of id `id`.
    pub fn get(id: u32) -> Option<alloc::sync::Arc<Self>> {
        if id == 0 {
            return None;
        }
        #[cfg(feature = "std")]
        {
            let all = FACE_SETTINGS.lock().unwrap_or_else(|e| e.into_inner());
            all.get(id as usize - 1).cloned()
        }
        #[cfg(not(feature = "std"))]
        None
    }
}

/// pyro: what a `@font-face` rule's `size-adjust`, `ascent-override`, `descent-override` and
/// `line-gap-override` descriptors (CSS Fonts 5 §4.9-§4.11) make of a font: a factor of the
/// size it is used at (glyphs, advances and metrics), and metrics as fractions of that
/// (adjusted) size. `None` is the font's own (`normal`).
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct FaceMetrics {
    /// `size-adjust` as a factor (`150%` is 1.5).
    pub size_adjust: Option<f32>,
    /// `ascent-override` as a fraction of the font size.
    pub ascent: Option<f32>,
    /// `descent-override` as a fraction of the font size.
    pub descent: Option<f32>,
    /// `line-gap-override` as a fraction of the font size.
    pub line_gap: Option<f32>,
}

impl Synthesis {
    /// pyro: the font's `@font-face` size and metrics descriptors.
    pub fn face_metrics(&self) -> &FaceMetrics {
        &self.face_metrics
    }

    /// pyro: the font's `@font-face` features and variation settings, if it has any.
    pub fn face_settings(&self) -> Option<alloc::sync::Arc<FaceSettings>> {
        FaceSettings::get(self.face_settings)
    }

    /// pyro: the factor of the font size the font is used at (`size-adjust`), 1 without one.
    pub fn size_adjust(&self) -> f32 {
        self.face_metrics.size_adjust.unwrap_or(1.0)
    }

    /// pyro: the same synthesis with the font used at the size asked for (`size-adjust` left
    /// out, as CSS `font-size-adjust` has it).
    pub fn without_size_adjust(mut self) -> Self {
        self.face_metrics.size_adjust = None;
        self
    }

    /// Returns `true` if any synthesis suggestions are available.
    pub fn any(&self) -> bool {
        self.len != 0 || self.embolden || self.skew != 0
    }

    /// Returns the variation settings that should be applied to match the
    /// requested attributes.
    ///
    /// When using `parley`, these can be used to create `FontVariation`
    /// settings.
    pub fn variation_settings(&self) -> &[(Tag, f32)] {
        &self.vars[..self.len as usize]
    }

    /// Returns `true` if the scaler should apply a faux bold.
    pub fn embolden(&self) -> bool {
        self.embolden
    }

    /// Returns a skew angle for faux italic/oblique, if requested.
    pub fn skew(&self) -> Option<f32> {
        if self.skew != 0 {
            Some(self.skew as f32)
        } else {
            None
        }
    }
}

#[allow(clippy::missing_fields_in_debug)]
impl fmt::Debug for Synthesis {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Synthesis")
            .field("vars", &self.variation_settings())
            .field("embolden", &self.embolden)
            .field("skew", &self.skew)
            .finish()
    }
}

fn read_attributes(font: &FontRef<'_>) -> (FontWidth, FontStyle, FontWeight) {
    use read_fonts::{
        TableProvider,
        tables::{
            head::{Head, MacStyle},
            os2::{Os2, SelectionFlags},
            post::Post,
        },
    };

    fn width_from_width_class(width_class: u16) -> FontWidth {
        FontWidth::from_ratio(match width_class {
            0..=1 => 0.5,
            2 => 0.625,
            3 => 0.75,
            4 => 0.875,
            5 => 1.0,
            6 => 1.125,
            7 => 1.25,
            8 => 1.5,
            _ => 2.0,
        })
    }

    fn from_os2_post(os2: Os2<'_>, post: Option<Post<'_>>) -> (FontWidth, FontStyle, FontWeight) {
        let width = width_from_width_class(os2.us_width_class());
        // Bits 1 and 9 of the fsSelection field signify italic and
        // oblique, respectively.
        // See: <https://learn.microsoft.com/en-us/typography/opentype/spec/os2#fsselection>
        let fs_selection = os2.fs_selection();
        let style = if fs_selection.contains(SelectionFlags::ITALIC) {
            FontStyle::Italic
        } else if fs_selection.contains(SelectionFlags::OBLIQUE) {
            let angle = post.map(|post| post.italic_angle().to_f64() as f32);
            FontStyle::Oblique(angle)
        } else {
            FontStyle::Normal
        };
        // The usFontWeightClass field is specified with a 1-1000 range, but
        // we don't clamp here because variable fonts could potentially
        // have a value outside of that range.
        // See <https://learn.microsoft.com/en-us/typography/opentype/spec/os2#usweightclass>
        let weight = FontWeight::new(os2.us_weight_class() as f32);
        (width, style, weight)
    }

    fn from_head(head: Head<'_>) -> (FontWidth, FontStyle, FontWeight) {
        let mac_style = head.mac_style();
        let style = if mac_style.contains(MacStyle::ITALIC) {
            FontStyle::Italic
        } else {
            FontStyle::default()
        };
        let weight = if mac_style.contains(MacStyle::BOLD) {
            FontWeight::BOLD
        } else {
            FontWeight::default()
        };
        (FontWidth::default(), style, weight)
    }

    if let Ok(os2) = font.os2() {
        // Prefer values from the OS/2 table if it exists. We also use
        // the post table to extract the angle for oblique styles.
        from_os2_post(os2, font.post().ok())
    } else if let Ok(head) = font.head() {
        // Otherwise, fall back to the macStyle field of the head table.
        from_head(head)
    } else {
        (
            FontWidth::default(),
            FontStyle::Normal,
            FontWeight::default(),
        )
    }
}

/// Helper for specifying aspects of a font's metadata to be overridden when the
/// font is registered. Helpful when implementing a `@font-face`-like API, which
/// allows those defining the fonts to specify certain font properties manually.
#[derive(Copy, Clone, Debug, Default)]
pub struct FontInfoOverride<'a> {
    /// Font family name to be used instead of the one specified in the font
    /// itself.
    pub family_name: Option<&'a str>,
    /// Font width to be used instead of the one specified in the font itself.
    pub width: Option<FontWidth>,
    /// Font's visual style / "slope" to be used instead of the one specified in
    /// the font itself.
    pub style: Option<FontStyle>,
    /// Font weight to be used instead of the one specified in the font itself.
    pub weight: Option<FontWeight>,
    /// Default values for the font's variation axes. Axes not included within
    /// the font will be ignored.
    pub axes: Option<&'a [(Tag, f32)]>,
    /// The code points the font is used for (CSS `unicode-range`) instead of
    /// every code point: queries yield the font's character map
    /// [limited](crate::Charmap::limited_to) to them.
    pub unicode_range: Option<&'a UnicodeRange>,
    /// The font's precedence among the fonts of its family that match a
    /// query's attributes equally, such as the faces of a CSS composite font
    /// (`@font-face` rules of one family, width, style and weight): a query
    /// yields those fonts highest precedence first, fonts of equal precedence
    /// in the order they were registered. CSS checks a composite font's faces
    /// last rule first, whichever face finished loading first, so a
    /// `@font-face` implementation gives each rule a precedence rising in rule
    /// order.
    pub precedence: Option<u64>,
    /// pyro: the slant angles the font is allowed (`FontInfo`'s `slant_range`): CSS's
    /// `font-style` descriptor, `(0.0, 0.0)` for `normal`.
    pub slant_range: Option<(f32, f32)>,
    /// pyro: the size and metrics `@font-face` descriptors give the font.
    pub face_metrics: Option<FaceMetrics>,
    /// pyro: the features and variation settings `@font-face` descriptors give the font, an id of
    /// [`FaceSettings::intern`].
    pub face_settings: Option<u32>,
    /// pyro: a `@font-face` rule's `font-weight` range (either order): font matching takes the
    /// weight of the range nearest the one asked for (CSS Fonts 4 §5.2), and a `wght` axis is
    /// held to it.
    pub weight_range: Option<(f32, f32)>,
    /// pyro: a `@font-face` rule's `font-stretch` range in percentages, as `weight_range`.
    pub width_range: Option<(f32, f32)>,
}

#[cfg(test)]
mod tests {
    use alloc::sync::Arc;

    use super::*;
    use crate::SourceId;

    const ROBOTO: &[u8] =
        include_bytes!("../../parley_dev/assets/fonts/roboto_fonts/Roboto-Regular.ttf");
    const ROBOTO_FLEX: &[u8] =
        include_bytes!("../../parley_dev/assets/fonts/roboto_fonts/RobotoFlex-VariableFont.ttf");

    fn font_info(data: &[u8]) -> FontInfo {
        let source = SourceInfo::new(
            SourceId::new(),
            SourceKind::Memory(Blob::new(Arc::new(data.to_vec()))),
        );
        FontInfo::from_source(source, 0).unwrap()
    }

    #[test]
    fn variable_weight_synthesis_uses_axis_default() {
        let mut font = font_info(ROBOTO_FLEX);
        let axes = [(Tag::new(b"wght"), FontWeight::THIN.value())];
        font.apply_override(&FontInfoOverride {
            axes: Some(&axes),
            ..Default::default()
        });

        // Model a face whose declared weight is 400 but whose `wght` axis
        // defaults to 100.
        assert_eq!(font.weight(), FontWeight::NORMAL);
        assert_eq!(
            font.axes()
                .iter()
                .find(|axis| axis.tag == Tag::new(b"wght"))
                .unwrap()
                .default,
            FontWeight::THIN.value()
        );

        let synthesis = font.synthesis(font.width(), font.style(), FontWeight::NORMAL);
        assert_eq!(
            synthesis.variation_settings(),
            &[(Tag::new(b"wght"), FontWeight::NORMAL.value())]
        );
        assert!(!synthesis.embolden());

        let synthesis = font.synthesis(font.width(), font.style(), FontWeight::THIN);
        assert!(!synthesis.any());
    }

    #[test]
    fn slant_synthesis_follows_the_axis_direction_and_the_allowed_range() {
        let slnt = Tag::new(b"slnt");
        let oblique = |font: &FontInfo, degrees: f32| {
            font.synthesis(font.width(), FontStyle::Oblique(Some(degrees)), font.weight())
        };
        let slant_of = |synthesis: &Synthesis| {
            synthesis.variation_settings().iter().find(|(tag, _)| *tag == slnt).map(|(_, value)| *value)
        };

        // CSS angles are clockwise, the `slnt` axis counter-clockwise.
        let mut font = font_info(ROBOTO_FLEX);
        assert!(font.has_slant_axis());
        assert_eq!(slant_of(&oblique(&font, 10.0)), Some(-10.0));

        // `font-style: oblique 0deg 5deg` clamps the angle.
        font.apply_override(&FontInfoOverride { slant_range: Some((0.0, 5.0)), ..Default::default() });
        assert_eq!(slant_of(&oblique(&font, 10.0)), Some(-5.0));

        // `font-style: normal` leaves the axis alone: the slant is faked.
        font.apply_override(&FontInfoOverride { slant_range: Some((0.0, 0.0)), ..Default::default() });
        let synthesis = oblique(&font, 10.0);
        assert_eq!(slant_of(&synthesis), None);
        assert_eq!(synthesis.skew(), Some(10.0));
    }

    #[test]
    fn static_weight_synthesis_is_for_a_bold_request_on_a_lighter_face() {
        let font = font_info(ROBOTO);
        assert!(font.weight().value() < SYNTHETIC_BOLD_WEIGHT);

        for weight in [font.weight().value() + 1.0, 500.0, 599.0] {
            let synthesis = font.synthesis(font.width(), font.style(), FontWeight::new(weight));
            assert!(!synthesis.any(), "{weight}");
        }
        for weight in [600.0, 700.0, 900.0] {
            let synthesis = font.synthesis(font.width(), font.style(), FontWeight::new(weight));
            assert!(synthesis.embolden(), "{weight}");
            assert!(synthesis.variation_settings().is_empty());
        }
    }
}
