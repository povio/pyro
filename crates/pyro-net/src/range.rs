//! `Range` request headers for locally served bodies (`blob:` URLs): Fetch
//! "parse a single range header value" and its application to a known length
//! (Fetch "scheme fetch", `blob`).

/// A parsed single byte range: `(start, end)` with the end inclusive, either
/// one absent for an open range (`bytes=5-`, `bytes=-5`), never both.
pub type SingleRange = (Option<u64>, Option<u64>);

fn skip_whitespace(bytes: &[u8], mut at: usize) -> usize {
    while at < bytes.len() && matches!(bytes[at], b' ' | b'\t') {
        at += 1;
    }
    at
}

fn digits(bytes: &[u8], at: usize) -> (Option<u64>, usize) {
    let end = at + bytes[at..].iter().take_while(|b| b.is_ascii_digit()).count();
    if end == at {
        return (None, at);
    }
    // Digits past u64 are no range anyone can satisfy: saturate rather than fail.
    let value = bytes[at..end].iter().fold(0u64, |n, d| n.saturating_mul(10).saturating_add((d - b'0') as u64));
    (Some(value), end)
}

/// Fetch "parse a single range header value", with whitespace allowed as the
/// `blob` scheme fetch asks: `None` is failure (no range unit, no `=`, no `-`,
/// a second range, a start past the end, anything else trailing).
pub fn parse_single_range(value: &[u8]) -> Option<SingleRange> {
    parse_single_range_with(value, true)
}

/// Fetch "parse a single range header value" with `allow_whitespace` (false
/// for the CORS safelist). The range unit is `bytes`, case-sensitively.
pub fn parse_single_range_with(value: &[u8], allow_whitespace: bool) -> Option<SingleRange> {
    let ws = |bytes: &[u8], at: usize| if allow_whitespace { skip_whitespace(bytes, at) } else { at };
    if !value.starts_with(b"bytes") {
        return None;
    }
    let mut at = ws(value, 5);
    if value.get(at) != Some(&b'=') {
        return None;
    }
    at = ws(value, at + 1);
    let (start, next) = digits(value, at);
    at = ws(value, next);
    if value.get(at) != Some(&b'-') {
        return None;
    }
    at = ws(value, at + 1);
    let (end, next) = digits(value, at);
    if next != value.len() {
        return None;
    }
    match (start, end) {
        (None, None) => None,
        (Some(s), Some(e)) if s > e => None,
        range => Some(range),
    }
}

/// The byte span `range` names in a body of `len` bytes, as `start..end`
/// (end exclusive); `None` when it is not satisfiable. A suffix longer than
/// the body is the whole body; a start at or past the end is unsatisfiable.
pub fn resolve_range(range: SingleRange, len: u64) -> Option<(u64, u64)> {
    let (start, end) = match range {
        (None, Some(suffix)) => (len.saturating_sub(suffix), len),
        (Some(start), end) => (start, end.map_or(len, |e| e.saturating_add(1).min(len))),
        (None, None) => return None,
    };
    (start < end).then_some((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_range_values() {
        assert_eq!(parse_single_range(b"bytes=9-21"), Some((Some(9), Some(21))));
        assert_eq!(parse_single_range(b"bytes=11-"), Some((Some(11), None)));
        assert_eq!(parse_single_range(b"bytes=-8"), Some((None, Some(8))));
        assert_eq!(parse_single_range(b"bytes= \t9-21"), Some((Some(9), Some(21))));
        assert_eq!(parse_single_range(b"bytes=5 - 10"), Some((Some(5), Some(10))));
        assert_eq!(parse_single_range(b"bytes=-\t 5"), Some((None, Some(5))));
        assert_eq!(parse_single_range(b"bytes \t =\t 6-"), Some((Some(6), None)));
        assert_eq!(parse_single_range(b"BYTES=0-0"), None, "the unit is case-sensitive");
        assert_eq!(parse_single_range_with(b"bytes=0-5", false), Some((Some(0), Some(5))));
        assert_eq!(parse_single_range_with(b"bytes= 0-5", false), None);
        assert_eq!(parse_single_range(b"bytes=4-100000000000"), Some((Some(4), Some(100_000_000_000))));
        for bad in [
            &b""[..],
            b"byte=0-",
            b"bytes",
            b"bytes\t \t",
            b"bytes=0-5,15-",
            b"bytes=0-5, 15-",
            b"bytes=0-5,",
            b"bytes=-",
            b"bytes=10-5",
            b"bytes=x-5",
            b"bytes=5",
            b"bytes=5-x",
            b"bytes=x",
            b"bytes 5-",
            b"5-",
            b"bytes=5-10 ",
        ] {
            assert_eq!(parse_single_range(bad), None, "{:?}", std::str::from_utf8(bad));
        }
    }

    #[test]
    fn resolved_spans() {
        assert_eq!(resolve_range((Some(9), Some(21)), 30), Some((9, 22)));
        assert_eq!(resolve_range((Some(11), None), 17), Some((11, 17)));
        assert_eq!(resolve_range((None, Some(8)), 19), Some((11, 19)));
        assert_eq!(resolve_range((Some(4), Some(100_000_000_000)), 13), Some((4, 13)));
        assert_eq!(resolve_range((Some(4), Some(13)), 13), Some((4, 13)));
        assert_eq!(resolve_range((None, Some(100)), 13), Some((0, 13)));
        assert_eq!(resolve_range((Some(13), None), 13), None);
        assert_eq!(resolve_range((Some(100_000), None), 13), None);
        assert_eq!(resolve_range((None, Some(0)), 13), None);
        assert_eq!(resolve_range((None, None), 13), None);
        assert_eq!(resolve_range((Some(0), None), 0), None);
    }
}
