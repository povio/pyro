//! Native contenteditable character deletion. Forward delete traverses an
//! extended grapheme; backward delete peels a scalar from combining text but
//! removes emoji sequences and base/variation-selector pairs together.

use icu_properties::{
    props::{
        BinaryProperty, CanonicalCombiningClass, EmojiModifier, EmojiModifierBase, ExtendedPictographic,
        RegionalIndicator, VariationSelector,
    },
    CodePointMapData,
};
use icu_segmenter::GraphemeClusterSegmenter;

/// Start byte of one pictographic component, optionally styled or modified.
/// Work backwards only inside the grapheme ICU has already identified.
fn emoji_component(s: &str) -> Option<usize> {
    let mut chars = s.char_indices().rev();
    let (_, last) = chars.next()?;
    let modified = EmojiModifier::for_char(last);
    let (mut at, mut base) = if modified { chars.next()? } else { (s.len() - last.len_utf8(), last) };
    if VariationSelector::for_char(base) {
        (at, base) = chars.next()?;
    }
    (if modified { EmojiModifierBase::for_char(base) } else { ExtendedPictographic::for_char(base) }).then_some(at)
}

fn backward_start(cluster: &str) -> usize {
    let mut chars = cluster.char_indices().rev();
    let (last_at, last) = chars.next().expect("nonempty grapheme");
    if last_at == 0 {
        return 0;
    }
    if cluster == "\r\n" {
        return 0;
    }
    if RegionalIndicator::for_char(last) {
        return chars.next().filter(|&(_, c)| RegionalIndicator::for_char(c)).map_or(last_at, |(i, _)| i);
    }
    if last == '\u{20e3}' {
        let mut prev = chars.next();
        if prev.is_some_and(|(_, c)| VariationSelector::for_char(c)) {
            prev = chars.next();
        }
        return prev.filter(|&(_, c)| matches!(c, '0'..='9' | '#' | '*')).map_or(last_at, |(i, _)| i);
    }
    if last == '\u{e007f}' {
        let mut end = last_at;
        for (i, c) in chars.by_ref() {
            if !('\u{e0020}'..='\u{e007e}').contains(&c) {
                break;
            }
            end = i;
        }
        if end < last_at {
            return emoji_component(&cluster[..end]).unwrap_or(end);
        }
        return last_at;
    }
    if let Some(mut at) = emoji_component(cluster) {
        while let Some(prefix) = cluster[..at].strip_suffix('\u{200d}') {
            let Some(previous) = emoji_component(prefix) else {
                break;
            };
            at = previous;
        }
        return at;
    }
    if VariationSelector::for_char(last) {
        if let Some((i, c)) = chars.next() {
            if !VariationSelector::for_char(c)
                && CodePointMapData::<CanonicalCombiningClass>::new().get(c) == CanonicalCombiningClass::NotReordered
            {
                return i;
            }
        }
    }
    last_at
}

/// Return the rendered caret's UTF-16 offset and the interval to delete.
/// Script can set a DOM caret inside a grapheme (even between surrogates);
/// native deletion starts from its trailing boundary in that case.
pub(super) fn step(text: &str, offset: u32, backward: bool) -> (u32, Option<(u32, u32)>) {
    if backward && offset == 0 {
        return (0, None);
    }
    let bytes = text.as_bytes();
    let i = offset as usize;
    // A wholly ASCII prefix through the following boundary needs no
    // Unicode traversal. In particular, deleting near the start of a long
    // paragraph need not inspect its suffix.
    if i <= bytes.len() && bytes[..i.saturating_add(3).min(bytes.len())].is_ascii() {
        let mid_crlf = i > 0 && bytes.get(i - 1) == Some(&b'\r') && bytes.get(i) == Some(&b'\n');
        let caret = i + usize::from(mid_crlf);
        let span = if backward {
            (caret > 0).then(|| {
                let width = if bytes[..caret].ends_with(b"\r\n") { 2 } else { 1 };
                ((caret - width) as u32, caret as u32)
            })
        } else {
            (caret < bytes.len()).then(|| {
                let width = if bytes[caret..].starts_with(b"\r\n") { 2 } else { 1 };
                (caret as u32, (caret + width) as u32)
            })
        };
        return (caret as u32, span);
    }
    let mut byte = 0;
    let mut units = 0;
    for next in GraphemeClusterSegmenter::new().segment_str(text).skip(1) {
        let cluster = &text[byte..next];
        let end = units + pyro_dom::dom::utf16_len(cluster);
        if !backward && offset <= units {
            return (units, Some((units, end)));
        }
        if backward && offset > units && offset <= end {
            let from = backward_start(cluster);
            return (end, Some((units + pyro_dom::dom::utf16_len(&cluster[..from]), end)));
        }
        byte = next;
        units = end;
    }
    (if offset == 0 { 0 } else { units }, None)
}

#[cfg(test)]
mod tests {
    use super::step;

    #[test]
    fn backward_keeps_the_base_letter_but_deletes_complete_emoji_sequences() {
        assert_eq!(step("e\u{301}", 2, true), (2, Some((1, 2))));
        for text in ["🇸🇮", "👋🏽", "👩‍👧", "1️⃣", "漢\u{e0100}"] {
            let end = text.encode_utf16().count() as u32;
            assert_eq!(step(text, end, true), (end, Some((0, end))));
        }
        assert_eq!(step("👋🏽\u{301}", 5, true), (5, Some((4, 5))));
    }

    #[test]
    fn forward_uses_the_whole_cluster_and_a_programmatic_caret_snaps_forward() {
        assert_eq!(step("क्षिY", 0, false), (0, Some((0, 4))));
        assert_eq!(step("X😀Y", 2, false), (3, Some((3, 4))));
        assert_eq!(step("X😀", 2, false), (3, None));
        assert_eq!(step("X😀", 2, true), (3, Some((1, 3))));
        assert_eq!(step("\r\nX", 0, false), (0, Some((0, 2))));
    }
}
