//! pyro: `hyphens: auto` (CSS Text 3 § 5.4): words of a language with hyphenation patterns get a
//! soft hyphen (U+00AD) at each hyphenation opportunity, which parley's line breaker shows as the
//! font's hyphen where a line ends there (as for authored soft hyphens). The patterns are TeX's
//! (`hypher`), from which Chromium's (Android's) dictionaries derive. `hyphenate-limit-chars`
//! bounds the word and the parts around a hyphen, `auto` as in Blink: 5, 2 and 2.

use std::borrow::Cow;

use style::properties::ComputedValues;
use style::values::computed::text::Hyphens;
use style::values::generics::text::NumberOrAuto;

/// The patterns for `lang` (a BCP 47 tag; its language subtag decides), if any.
fn language(lang: &str) -> Option<hypher::Lang> {
    let primary = lang.split(['-', '_']).next()?.to_ascii_lowercase();
    let code: [u8; 2] = primary.as_bytes().try_into().ok()?;
    match &code {
        // Norwegian Bokmål and Nynorsk use the Norwegian patterns.
        b"nb" | b"nn" => hypher::Lang::from_iso(*b"no"),
        _ => hypher::Lang::from_iso(code),
    }
}

/// Whether `c` belongs to a word: a letter, a combining mark or a soft hyphen.
pub(crate) fn is_word_char(c: char) -> bool {
    c.is_alphabetic() || is_mark(c) || c == '\u{ad}'
}

/// Whether `c` is invisible formatting (an empty inline box's strut, a bidi control) that a
/// word continues across.
pub(crate) fn is_invisible(c: char) -> bool {
    matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2069}' | '\u{FEFF}')
}

/// The byte offsets in `text` where `style` puts a soft hyphen: the hyphenation opportunities of
/// its words (`hyphens: auto` in a language with patterns), none for a word with a soft hyphen of
/// its own.
fn opportunities(text: &str, style: &ComputedValues) -> Vec<usize> {
    let itext = style.get_inherited_text();
    let mut points = Vec::new();
    if itext.hyphens != Hyphens::Auto || !text.chars().any(char::is_alphabetic) {
        return points;
    }
    let Some(lang) = language(&style.get_font()._x_lang.0) else {
        return points;
    };
    let limit = |value: &NumberOrAuto<i32>, auto: usize| match value {
        NumberOrAuto::Number(n) => (*n).max(1) as usize,
        NumberOrAuto::Auto => auto,
    };
    let limits = &itext.hyphenate_limit_chars;
    let word_min = limit(&limits.total_word_length, 5);
    let before_min = limit(&limits.pre_hyphen_length, 2);
    let after_min = limit(&limits.post_hyphen_length, 2);
    let mut offset = 0;
    while offset < text.len() {
        let rest = &text[offset..];
        let Some(start) = rest.find(|c: char| c.is_alphabetic()) else {
            break;
        };
        let word_start = offset + start;
        let word_len = text[word_start..]
            .char_indices()
            .find(|&(_, c)| !is_word_char(c))
            .map_or(text.len() - word_start, |(i, _)| i);
        let word = &text[word_start..word_start + word_len];
        offset = word_start + word_len;
        if word.chars().count() < word_min || word.contains('\u{ad}') {
            continue;
        }
        let mut at = word_start;
        let mut syllables = hypher::hyphenate_bounded(word, lang, before_min, after_min).peekable();
        while let Some(syllable) = syllables.next() {
            at += syllable.len();
            if syllables.peek().is_some() {
                points.push(at);
            }
        }
    }
    points
}

/// `text` with soft hyphens at the hyphenation opportunities that `style` gives it, with
/// `before` and `after` the parts of its first and last words outside it (text of other nodes):
/// an opportunity where `text` starts belongs to the text before it.
pub(crate) fn hyphenate<'a>(before: &str, text: &'a str, after: &str, style: &ComputedValues) -> Cow<'a, str> {
    if style.get_inherited_text().hyphens != Hyphens::Auto {
        return Cow::Borrowed(text);
    }
    let context = format!("{before}{text}{after}");
    let points: Vec<usize> = opportunities(&context, style)
        .into_iter()
        .filter(|&at| at > before.len() && at <= before.len() + text.len())
        .map(|at| at - before.len())
        .collect();
    if points.is_empty() {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len() + 2 * points.len());
    let mut last = 0;
    for at in points {
        out.push_str(&text[last..at]);
        out.push('\u{ad}');
        last = at;
    }
    out.push_str(&text[last..]);
    Cow::Owned(out)
}

fn is_mark(c: char) -> bool {
    matches!(c as u32, 0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE20..=0xFE2F)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_patterns() {
        let lang = language("en-US").unwrap();
        let joined: Vec<&str> = hypher::hyphenate_bounded("example", lang, 2, 2).collect();
        assert_eq!(joined, ["ex", "am", "ple"]);
        assert!(language("De-dE").is_some());
        assert!(language("x").is_none());
    }
}
