//! pyro: upright glyphs in vertical lines (CSS `text-orientation: upright`). A glyph of a font
//! without vertical metrics (`vmtx`) advances by the font's height (ascent + descent), as
//! HarfBuzz's default vertical advance in Blink (WPT css-values `ch-unit-002`, `-011`); a glyph of
//! a font with vertical metrics advances by its advance height there, as HarfBuzz shaping top to
//! bottom (WPT css-writing-modes `text-combine-upright-line-breaking-rules-001`).

use alloc::vec::Vec;

use crate::layout::Layout;
use crate::resolve::ResolvedStyle;
use crate::style::Brush;

pub(crate) fn apply<B: Brush>(layout: &mut Layout<B>, styles: &[ResolvedStyle<B>], text: &str) {
    if !styles.iter().any(|style| style.upright || style.vertical) {
        return;
    }
    let shaped_text = &layout.data.shaped_text;
    let characters = shaped_text.characters();
    let clusters = shaped_text.shaped_clusters();
    let mut changes: Vec<(u32, f32)> = Vec::new();
    let mut upright_runs: Vec<(usize, f32)> = Vec::new();
    for (run_index, run) in shaped_text.runs().iter().enumerate() {
        let Some(height) = shaped_text.default_vertical_advance(run.font_index, run.font_size) else {
            // Vertical metrics: each upright cluster's own advance height (unless shaping top to
            // bottom gave them already, with the features that change them).
            if run.vertical_shaped {
                continue;
            }
            for index in run.shaped_clusters_range.clone() {
                let cluster = &clusters[index as usize];
                let Some(character) = characters.get(cluster.chars_range().start as usize) else { continue };
                // (`text-orientation: upright`, or a character `mixed` stands upright.)
                let style = &styles[character.style_index as usize];
                let upright = style.upright || (style.vertical && text.get(character.text_byte_start as usize..).and_then(|rest| rest.chars().next()).is_some_and(crate::upright_in_vertical));
                if !upright || cluster.advance == 0.0 {
                    continue;
                }
                if let Some(advance) = shaped_text.cluster_vertical_advance(index, run.font_index, run.font_size) {
                    changes.push((index, advance));
                }
            }
            continue;
        };
        let mut upright = false;
        for index in run.shaped_clusters_range.clone() {
            let cluster = &clusters[index as usize];
            let Some(character) = characters.get(cluster.chars_range().start as usize) else { continue };
            if !styles[character.style_index as usize].upright {
                continue;
            }
            upright = true;
            if cluster.advance != 0.0 {
                changes.push((index, height));
            }
        }
        if upright {
            upright_runs.push((run_index, height));
        }
    }
    for (index, advance) in changes {
        layout.data.shaped_text.set_cluster_advance(index, advance);
    }
    for (run_index, advance) in upright_runs {
        layout.data.shaped_text.set_run_upright_advance(run_index, advance);
    }
}
