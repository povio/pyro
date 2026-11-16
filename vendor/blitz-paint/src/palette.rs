//! pyro: `font-palette` (CSS Fonts 4 §9): the colours a colour font's (COLR) glyphs are drawn in.
//!
//! The glyph painter under the renderer draws a COLR glyph in palette 0 of its font's CPAL
//! table. A glyph run whose element selects another palette (`light`, `dark`, an
//! `@font-palette-values` rule's `base-palette` and `override-colors`, or a `palette-mix()` of
//! those) is drawn from a copy of its font whose palette 0 holds the selected colours.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use blitz_dom::{BaseDocument, NodeId};
use parley::FontData;
use peniko::Blob;
use skrifa::raw::tables::cpal::{Cpal, PaletteType};
use skrifa::raw::{FontRef, TableProvider, TopLevelTable};
use skrifa::{MetadataProvider, string::StringId};
use style::color::AbsoluteColor;
use style::color::mix::{ColorMixItem, mix_many};
use style::stylesheets::font_palette_values_rule::FontPaletteBase;
use style::values::computed::font::FontPalette;
use style::values::generics::color::ColorMixFlags;

use crate::color::ToColorColor as _;

/// The colours of a palette's entries, RGBA.
type Entries = Vec<[u8; 4]>;

/// The font a glyph run of `node_id` in `font` is drawn from: `None` for `font` itself (the
/// element's palette is the font's default one, or the font has no palettes).
pub(crate) fn palette_font(doc: &BaseDocument, node_id: NodeId, font: &FontData) -> Option<FontData> {
    let styles = doc.get_node(node_id)?.primary_styles()?;
    let palette = &styles.get_font().font_palette;
    if *palette == FontPalette::normal() {
        return None;
    }
    let data = font.data.data();
    let font_ref = FontRef::from_index(data, font.index).ok()?;
    let cpal = font_ref.cpal().ok()?;
    let fonts = PaletteFont { doc, font, font_ref: &font_ref, cpal: &cpal };
    let entries = fonts.entries(palette)?;
    if entries == fonts.palette(0)? {
        return None;
    }
    thread_local! {
        static FONTS: RefCell<HashMap<(u64, u32, Entries), FontData>> = RefCell::new(HashMap::new());
    }
    const MAX_FONTS: usize = 32;
    let key = (font.data.id(), font.index, entries);
    if let Some(found) = FONTS.with(|fonts| fonts.borrow().get(&key).cloned()) {
        return Some(found);
    }
    let patched = fonts.with_entries(&key.2)?;
    FONTS.with(|fonts| {
        let mut fonts = fonts.borrow_mut();
        if fonts.len() >= MAX_FONTS {
            fonts.clear();
        }
        fonts.insert(key, patched.clone());
    });
    Some(patched)
}

/// A colour font a run is drawn in, and the document whose rules select its palette.
struct PaletteFont<'a> {
    doc: &'a BaseDocument,
    font: &'a FontData,
    font_ref: &'a FontRef<'a>,
    cpal: &'a Cpal<'a>,
}

impl PaletteFont<'_> {
    /// The colours of CPAL palette `index`.
    fn palette(&self, index: u16) -> Option<Entries> {
        let records = self.cpal.color_records_array()?.ok()?;
        let first = usize::from(self.cpal.color_record_indices().get(usize::from(index))?.get());
        let records = records.get(first..first + usize::from(self.cpal.num_palette_entries()))?;
        Some(records.iter().map(|record| [record.red(), record.green(), record.blue(), record.alpha()]).collect())
    }

    /// The first palette with `flag` in its CPAL type (`light`, `dark`), else the default one.
    fn typed_palette(&self, flag: PaletteType) -> u16 {
        self.cpal
            .palette_types_array()
            .and_then(Result::ok)
            .and_then(|types| types.iter().position(|ty| ty.get().contains(flag)))
            .map_or(0, |index| index as u16)
    }

    /// The colours `palette` gives the font's entries.
    fn entries(&self, palette: &FontPalette) -> Option<Entries> {
        match palette {
            FontPalette::Mix(mix) => {
                let (left, right) = (self.entries(&mix.left.0)?, self.entries(&mix.right.0)?);
                // Weights that add up to zero: the mix is invalid at computed-value time, `normal`.
                let Some((p1, p2)) = mix.weights() else {
                    return self.palette(0);
                };
                let color = |[r, g, b, a]: [u8; 4]| AbsoluteColor::srgb_legacy(r, g, b, f32::from(a) / 255.0);
                Some(
                    left.into_iter()
                        .zip(right)
                        .map(|(l, r)| {
                            let mixed = mix_many(
                                mix.interpolation,
                                [ColorMixItem::new(color(l), p1), ColorMixItem::new(color(r), p2)],
                                ColorMixFlags::empty(),
                            );
                            mixed.as_srgb_color().to_rgba8().to_u8_array()
                        })
                        .collect(),
                )
            }
            FontPalette::Named(_) if *palette == FontPalette::light() => {
                self.palette(self.typed_palette(PaletteType::USABLE_WITH_LIGHT_BACKGROUND))
            }
            FontPalette::Named(_) if *palette == FontPalette::dark() => {
                self.palette(self.typed_palette(PaletteType::USABLE_WITH_DARK_BACKGROUND))
            }
            FontPalette::Named(name) => self.rule_entries(name).or_else(|| self.palette(0)),
        }
    }

    /// The colours of the last `@font-palette-values` rule named `name` whose `font-family` names a
    /// family of the font: `None` without one.
    fn rule_entries(&self, name: &style::Atom) -> Option<Entries> {
        // The family names the font goes by: that of the `@font-face` rule that loaded it, else
        // its own (an installed font).
        let mut families: Vec<String> = self.doc.font_face_families(self.font.data.id()).to_vec();
        if families.is_empty() {
            for id in [StringId::TYPOGRAPHIC_FAMILY_NAME, StringId::FAMILY_NAME] {
                if let Some(family) = self.font_ref.localized_strings(id).english_or_first() {
                    families.push(family.chars().collect());
                }
            }
        }
        let rule = self
            .doc
            .font_palette_values_rules()
            .filter(|rule| rule.name.0 == *name)
            .filter(|rule| {
                rule.family_names
                    .iter()
                    .any(|family| families.iter().any(|name| name.eq_ignore_ascii_case(&family.name)))
            })
            .last()?;
        let palettes = i32::from(self.cpal.num_palettes());
        let base = match &rule.base_palette {
            None => 0,
            Some(FontPaletteBase::Light) => self.typed_palette(PaletteType::USABLE_WITH_LIGHT_BACKGROUND),
            Some(FontPaletteBase::Dark) => self.typed_palette(PaletteType::USABLE_WITH_DARK_BACKGROUND),
            // A palette the font does not have is its default one.
            Some(base @ FontPaletteBase::Index(_)) => {
                base.resolved_index().filter(|&index| index >= 0 && index < palettes).unwrap_or(0) as u16
            }
        };
        let mut entries = self.palette(base)?;
        // Each override replaces its entry (a later one an earlier one); one past the entries is
        // ignored.
        for entry in &rule.override_colors {
            let Some(index) = entry.resolved_index().and_then(|index| usize::try_from(index).ok()) else {
                continue;
            };
            let Some(slot) = entries.get_mut(index) else {
                continue;
            };
            if let Some(color) = entry.color.to_computed_color(None).ok().and_then(|color| color.as_absolute().copied()) {
                *slot = color.as_srgb_color().to_rgba8().to_u8_array();
            }
        }
        Some(entries)
    }

    /// A copy of the font whose palette 0 has the colours `entries`.
    fn with_entries(&self, entries: &[[u8; 4]]) -> Option<FontData> {
        let data = self.font.data.data();
        let table = self.font_ref.table_data(Cpal::TAG)?;
        let table_start = (table.as_bytes().as_ptr() as usize).checked_sub(data.as_ptr() as usize)?;
        let records_start = table_start + self.cpal.color_records_array_offset().offset().to_u32() as usize;
        let mut bytes = data.to_vec();
        // Palette 0 of the copy starts at the first record, which it holds the colours in.
        let indices = table_start + self.cpal.color_record_indices_byte_range().start;
        bytes.get_mut(indices..indices + 2)?.copy_from_slice(&0u16.to_be_bytes());
        for (k, [r, g, b, a]) in entries.iter().enumerate() {
            // CPAL colour records are BGRA.
            let at = records_start + 4 * k;
            bytes.get_mut(at..at + 4)?.copy_from_slice(&[*b, *g, *r, *a]);
        }
        Some(FontData::new(Blob::new(Arc::new(bytes)), self.font.index))
    }
}
