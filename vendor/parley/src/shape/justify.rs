// Copyright 2026 the Parley Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! pyro: CSS Text 3 `text-justify`: where a justified line gets its extra space, as Blink
//! (`ShapeResultSpacing`) finds the opportunities. `auto` and `inter-word`: after each word
//! separator; `auto` also before and after each CJK character (once between two of them);
//! `inter-character` (`distribute`) between all characters but the letters of cursive scripts
//! (Arabic); `none` nowhere. Invisible formatting characters (bidi controls) are passed over. The
//! line breaker leaves out the opportunity before a line's first character and after its last.

use super::super::layout::Layout;
use super::super::resolve::ResolvedStyle;
use crate::style::{Brush, TextJustify};
use parley_engine::shape::{Whitespace, is_cursive, is_format};

/// Whether justification spaces out `ch` in `auto` (Blink's `IsCJKIdeographOrSymbol`, by block).
fn is_cjk(ch: char) -> bool {
    matches!(ch as u32, 0x2E80..=0x2FFF | 0x3000..=0x303F | 0x3040..=0x30FF | 0x31C0..=0x31FF
        | 0x3200..=0x33FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0xFE30..=0xFE4F
        | 0xFF00..=0xFFEF | 0x20000..=0x3FFFF)
}

pub(crate) fn apply<B: Brush>(
    layout: &mut Layout<B>,
    text: &str,
    styles: &[ResolvedStyle<B>],
    inline_boxes: &[crate::inline_box::LayoutInlineBox],
) {
    if styles.iter().all(|style| style.text_justify == TextJustify::InterWord)
        || styles.iter().all(|style| style.text_justify == TextJustify::Auto) && !text.chars().any(is_cjk)
    {
        return;
    }
    // The text positions of the atomic inlines: one between two characters is not text, so the
    // character after it gets the opportunity before it (as after the start of the line).
    let mut atomic: alloc::vec::Vec<u32> = inline_boxes
        .iter()
        .filter(|layout_box| layout_box.inline_box.kind == crate::InlineBoxKind::InFlow)
        .map(|layout_box| layout_box.inline_box.index as u32)
        .collect();
    atomic.sort_unstable();
    let mut atomic = atomic.into_iter().peekable();
    let shaped_text = &mut layout.data.shaped_text;
    let count = shaped_text.characters().len();
    // Whether an opportunity follows the previous character (Blink's `is_after_expansion`).
    let mut after_expansion = false;
    for index in 0..count {
        let character = shaped_text.characters()[index];
        let style = &styles[character.style_index as usize];
        while atomic.next_if(|&position| position <= character.text_byte_start as u32).is_some() {
            after_expansion = false;
        }
        let Some(ch) = text.get(character.text_byte_start as usize..).and_then(|rest| rest.chars().next()) else {
            continue;
        };
        // Invisible formatting characters (bidi controls) are passed over.
        if character.whitespace == Whitespace::None && is_format(ch) {
            continue;
        }
        let separator = matches!(character.whitespace, Whitespace::Space | Whitespace::NoBreakSpace);
        let (none, before, after) = match style.text_justify {
            TextJustify::None => (true, false, false),
            TextJustify::InterWord => (false, false, false),
            TextJustify::Auto if !separator && is_cjk(ch) => (false, !after_expansion, true),
            TextJustify::Auto => (false, false, false),
            TextJustify::InterCharacter
                if !separator && character.whitespace == Whitespace::None && !is_cursive(ch) =>
            {
                (false, !after_expansion, true)
            }
            TextJustify::InterCharacter => (false, false, false),
        };
        if none || before || after {
            shaped_text.set_character_justification(index, none, before, after);
        }
        after_expansion = after || (separator && !none);
    }
}
