// Copyright 2026 the Parley Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! pyro: CSS Text 4 `text-autospace`: 1/8 of an ideographic advance between ideographs (and kana,
//! Hangul) and the letters and numerals of other scripts, as Blink does it (`TextAutoSpace`): the
//! characters' classes are the East_Asian_Spacing property of UTR #59 (Wide, Narrow, Conditional,
//! Other), marks take their base's class, and Conditional punctuation (`!`, `#`, `%`, ...) counts as
//! Narrow in Chinese text only. The space is added after the earlier character of the pair (its
//! cluster's advance); after right-to-left text it goes to the right edge of that text, before the
//! left-to-right text that follows. The line breaker drops the space after the last character of
//! a line ([`LayoutData::autospace`]).

use alloc::vec::Vec;

use super::super::layout::Layout;
use super::super::resolve::ResolvedStyle;
use crate::inline_box::LayoutInlineBox;
use crate::style::Brush;
use parley_engine::shape::{EastAsianSpacing as Class, east_asian_spacing as class, is_mark};

/// Whether a language is Chinese (the macrolanguage `zh` or one it covers).
fn is_chinese(language: &str) -> bool {
    let primary = language.split(['-', '_']).next().unwrap_or("");
    primary.eq_ignore_ascii_case("zh")
        || [
            "cdo", "cjy", "cmn", "cnp", "cpx", "csp", "czh", "czo", "gan", "hak", "hsn", "lzh",
            "mnp", "nan", "wuu", "yue",
        ]
        .iter()
        .any(|code| primary.eq_ignore_ascii_case(code))
}

/// Adds the spaces `text-autospace` asks for to the shaped clusters of `layout`, and records in
/// `layout.data.autospace` those after a cluster (dropped where a line ends there).
pub(crate) fn apply<B: Brush>(
    layout: &mut Layout<B>,
    text: &str,
    styles: &[ResolvedStyle<B>],
    inline_boxes: &[LayoutInlineBox],
) {
    layout.data.autospace.clear();
    if styles.iter().all(|style| style.text_autospace.is_none())
        || !text.chars().any(|ch| ch as u32 >= 0x02C7 && !(0x1200..=0x3004).contains(&(ch as u32)))
    {
        return;
    }
    let chinese = styles
        .first()
        .and_then(|style| style.locale.as_ref())
        .is_some_and(|locale| is_chinese(locale.as_str()));
    let resolve = |class: Class| match class {
        Class::Conditional if chinese => Class::Narrow,
        Class::Conditional => Class::Other,
        class => class,
    };
    // Atomic inlines, floats and positioned boxes are characters to Blink (U+FFFC, Other); span
    // edges are not.
    let separators: Vec<usize> = inline_boxes
        .iter()
        .filter(|b| !b.inline_box.kind.is_span_edge())
        .map(|b| b.inline_box.index)
        .collect();
    let shaped_text = &layout.data.shaped_text;
    let characters = shaped_text.characters();
    let clusters = shaped_text.shaped_clusters();
    // Each cluster in logical order with its run's direction.
    let mut order: Vec<(u32, bool)> = Vec::with_capacity(clusters.len());
    for run in shaped_text.runs() {
        let rtl = run.bidi_level.is_rtl();
        order.extend(run.shaped_clusters_range.clone().map(|index| (index, rtl)));
    }
    // (cluster, space, trailing): `trailing`: after the cluster's glyph, else at the right edge
    // of right-to-left text.
    let mut spaces: Vec<(u32, f32, bool)> = Vec::new();
    // The last cluster that is not only marks: its class, whether its style spaces, its index in
    // `order`.
    let mut last: Option<(Class, usize)> = None;
    for (position, &(index, rtl)) in order.iter().enumerate() {
        let cluster = &clusters[index as usize];
        let chars = cluster.chars_range();
        let Some(first) = characters.get(chars.start as usize) else {
            continue;
        };
        let byte = first.text_byte_start as usize;
        let Some(ch) = text[byte..].chars().next() else {
            continue;
        };
        if separators.contains(&byte) {
            last = None;
        }
        if is_mark(ch) {
            continue;
        }
        let style = &styles[first.style_index as usize];
        let class = if style.text_autospace.is_none() { Class::Other } else { resolve(class(ch)) };
        if let Some((last_class, last_position)) = last {
            let numeric = |a: Class, b: Class| a == Class::Numeric || b == Class::Numeric;
            let pair = match (last_class, class) {
                (Class::Wide, Class::Narrow | Class::Numeric) | (Class::Narrow | Class::Numeric, Class::Wide) => {
                    Some(numeric(last_class, class))
                }
                _ => None,
            };
            if let Some(numeric) = pair {
                let (last_index, _) = order[last_position];
                let last_style = &styles[characters[clusters[last_index as usize].chars_range().start as usize].style_index as usize];
                let allowed = |style: &ResolvedStyle<B>| {
                    if numeric { style.text_autospace.ideograph_numeric } else { style.text_autospace.ideograph_alpha }
                };
                // The space goes right before the cluster (after the marks of the one before).
                let (before_index, before_rtl) = order[position - 1];
                if allowed(style) && allowed(last_style) {
                    if !before_rtl {
                        let before_style = &styles[characters[clusters[before_index as usize].chars_range().start as usize].style_index as usize];
                        spaces.push((before_index, before_style.font_size / 8.0, true));
                    } else if !rtl {
                        // The right edge of the right-to-left text before: its first cluster.
                        let mut start = position - 1;
                        while start > 0 && order[start - 1].1 {
                            start -= 1;
                        }
                        spaces.push((order[start].0, style.font_size / 8.0, false));
                    }
                }
            }
        }
        last = Some((class, position));
    }
    for &(cluster, space, trailing) in &spaces {
        let advance = layout.data.shaped_text.shaped_clusters()[cluster as usize].advance;
        layout.data.shaped_text.set_cluster_advance(cluster, advance + space);
        if trailing {
            layout.data.autospace.push((cluster, space));
        }
    }
    layout.data.autospace.sort_by_key(|&(cluster, _)| cluster);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classes() {
        assert_eq!(class('国'), Class::Wide);
        assert_eq!(class('あ'), Class::Wide);
        assert_eq!(class('\u{20000}'), Class::Wide);
        assert_eq!(class('A'), Class::Narrow);
        assert_eq!(class('1'), Class::Numeric);
        assert_eq!(class('!'), Class::Conditional);
        assert_eq!(class('#'), Class::Conditional);
        assert_eq!(class('$'), Class::Other);
        assert_eq!(class('。'), Class::Other);
        assert_eq!(class('Ａ'), Class::Other);
        assert_eq!(class('\u{5d0}'), Class::Narrow);
        assert!(is_mark('\u{301}') && is_mark('\u{FE00}'));
        assert!(is_chinese("zh-Hant") && is_chinese("yue") && !is_chinese("ja"));
    }
}
