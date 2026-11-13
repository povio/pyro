//! `data:` URL processing (Fetch §4.1 "data: URL processor").

use base64::Engine;
use bytes::Bytes;
use url::Url;

/// Returns (mime type with parameters, body) or None if malformed.
pub fn parse(url: &Url) -> Option<(String, Bytes)> {
    if url.scheme() != "data" {
        return None;
    }
    parse_str(url.as_str())
}

/// [`parse`] of a URL already serialized (`Url::as_str`, or a string the
/// `url` crate produced): its text is all a `data:` URL holds, so a caller
/// with the string skips parsing it again (fonts inlined as `data:` URLs
/// run to megabytes).
pub fn parse_str(s: &str) -> Option<(String, Bytes)> {
    if !s.get(..5).is_some_and(|p| p.eq_ignore_ascii_case("data:")) {
        return None;
    }
    // Without fragment, strip "data:".
    let s = &s[5..s.find('#').unwrap_or(s.len())];
    let (meta, body) = s.split_once(',')?;
    let meta = meta.trim_matches(|c: char| c.is_ascii_whitespace());
    let (mime, is_b64) = match meta.rsplit_once(';') {
        Some((m, last)) if last.trim().eq_ignore_ascii_case("base64") => (m, true),
        _ => (meta, false),
    };
    let decoded: Vec<u8> = percent_encoding::percent_decode_str(body).collect();
    let bytes = if is_b64 {
        let cleaned: Vec<u8> = decoded.into_iter().filter(|b| !b.is_ascii_whitespace()).collect();
        let cleaned = trim_padding(&cleaned);
        base64::engine::general_purpose::STANDARD_NO_PAD.decode(cleaned).ok()?
    } else {
        decoded
    };
    let mut mime = mime.trim().to_owned();
    if mime.starts_with(';') {
        mime.insert_str(0, "text/plain");
    }
    if mime.is_empty() || !mime.contains('/') {
        mime = "text/plain;charset=US-ASCII".to_owned();
    }
    Some((mime, Bytes::from(bytes)))
}

fn trim_padding(b: &[u8]) -> &[u8] {
    let mut end = b.len();
    if b.len().is_multiple_of(4) {
        while end > 0 && b[end - 1] == b'=' && b.len() - end < 2 {
            end -= 1;
        }
    }
    &b[..end]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn data_urls() {
        let (m, b) = parse(&Url::parse("data:,Hello%2C%20World!").unwrap()).unwrap();
        assert_eq!(m, "text/plain;charset=US-ASCII");
        assert_eq!(&b[..], b"Hello, World!");
        let (m, b) = parse(&Url::parse("data:text/html;base64,PGI+aGk8L2I+").unwrap()).unwrap();
        assert_eq!(m, "text/html");
        assert_eq!(&b[..], b"<b>hi</b>");
        let (_, b) = parse(&Url::parse("data:;base64,YQ==").unwrap()).unwrap();
        assert_eq!(&b[..], b"a");
        assert!(parse(&Url::parse("data:text/plain;base64,%%%").unwrap()).is_none());
    }
}
