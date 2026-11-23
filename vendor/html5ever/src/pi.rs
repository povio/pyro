// pyro: processing instructions in HTML.

//! Processing instructions in HTML (`<?target data>`), as browsers parse them (WPT
//! html/syntax/parsing `processing-instructions.dat`; Chromium).
//!
//! The tokenizer reads `<?...>` as the bogus comment it was (its data starts with `?`) and marks
//! the comment token's data with a leading [`MARK`] when it started with `<?`. The tree builder
//! handles it as any comment, so it lands where a comment would; the tree sink's
//! `create_comment` then asks [`parse`] whether it is a processing instruction and makes that
//! node instead (or the comment, without the mark).

/// What the data of a comment token that started with `<?` begins with.
pub const MARK: char = '\0';

fn is_space(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\x0c' | '\r' | ' ')
}

/// The end of the target `text` starts with (a name of ASCII letters, digits, `-` and `_` that
/// starts with a letter or `_`, and not with "xml" in any case), if it has one.
fn target_end(text: &str) -> Option<usize> {
    let first = text.chars().next()?;
    if !(first.is_ascii_alphabetic() || first == '_') {
        return None;
    }
    let end = text.find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_')).unwrap_or(text.len());
    // Bytes: the first three need not end on a char boundary ("x\u{3c3e}", a UTF-16 "<?x>").
    if text.as_bytes().get(..3).is_some_and(|p| p.eq_ignore_ascii_case(b"xml")) {
        return None;
    }
    Some(end)
}

/// The target and data of the processing instruction `comment` is (the data of a comment token
/// without its [`MARK`]: `?` and what followed up to the `>`), or `None` when it stays a comment:
/// the target is followed by white space (skipped) and the data, by a `?` that starts the data,
/// or by nothing; one `?` at the end of the data is the instruction's own.
pub fn parse(comment: &str) -> Option<(&str, &str)> {
    let text = comment.strip_prefix('?')?;
    let end = target_end(text)?;
    let (target, rest) = text.split_at(end);
    let data = match rest.chars().next() {
        None => "",
        Some(c) if is_space(c) => rest.trim_start_matches(is_space),
        Some('?') => rest,
        Some(_) => return None,
    };
    Some((target, data.strip_suffix('?').unwrap_or(data)))
}

/// Whether the input ended inside a processing instruction: `comment` (as for [`parse`]) is `?`
/// alone or starts as an instruction does. Nothing is emitted for it then.
pub(crate) fn unfinished(comment: &str) -> bool {
    comment == "?" || parse(comment).is_some()
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn targets_and_data() {
        assert_eq!(parse("?something"), Some(("something", "")));
        assert_eq!(parse("?something else is good"), Some(("something", "else is good")));
        assert_eq!(parse("?good?"), Some(("good", "")));
        assert_eq!(parse("?hey   there?"), Some(("hey", "there")));
        assert_eq!(parse("?hey?there"), Some(("hey", "?there")));
        assert_eq!(parse("?something ? "), Some(("something", "? ")));
        assert_eq!(parse("?something ?\t   ??"), Some(("something", "?\t   ?")));
        assert_eq!(parse("?t d "), Some(("t", "d ")));
        assert_eq!(parse("?_-_ data"), Some(("_-_", "data")));
        assert_eq!(parse("?A----"), Some(("A----", "")));
    }

    #[test]
    fn comments() {
        for text in ["?x\u{3c3e}<meta", "?xm\u{e9}", "?a$", "?xml", "?XML-stylesheet", "?1st-place", "?-prefix", "?data.v1", "?namespace:tag", "? ", "?", "?\u{665}-star", "?fire-\u{1f525}", "no mark"] {
            assert_eq!(parse(text), None, "{text}");
        }
    }
}
