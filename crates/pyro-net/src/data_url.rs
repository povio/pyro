//! `data:` URL processing (Fetch §4.1 "data: URL processor").

use ::data_url::DataUrl;
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
    let url = DataUrl::process(s).ok()?;
    let (body, _fragment) = url.decode_to_vec().ok()?;
    Some((url.mime_type().to_string(), Bytes::from(body)))
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

    #[test]
    fn media_type_and_payload_follow_fetch_serialization() {
        let input = r#"data:TEXT/PLAIN; Foo="a;b";foo=ignored;BaR=x;,A%2b%23#fragment"#;
        let (mime, bytes) = parse_str(input).unwrap();
        assert_eq!(mime, r#"text/plain;foo="a;b";bar=x"#);
        assert_eq!(bytes.as_ref(), b"A+#");
        let (mime, bytes) = parse_str("data:not-a-type,one?two#three").unwrap();
        assert_eq!(mime, "text/plain;charset=US-ASCII");
        assert_eq!(bytes.as_ref(), b"one?two");
    }

    #[test]
    fn forgiving_base64_ignores_ascii_whitespace_and_unused_bits() {
        for input in ["data:;base64,YR==", "data:;BASE64,Y%20R", "data:;base64 ,Y%0CR=="] {
            assert_eq!(parse_str(input).unwrap().1.as_ref(), b"a");
        }
        for input in ["data:;base64,Y", "data:;base64,YQ===", "data:;base64,Y-Q="] {
            assert!(parse_str(input).is_none());
        }
    }
}
