//! Subresource Integrity (W3C SRI §3.3, "do bytes match metadataList"): the
//! `integrity` attribute of `<script>`, `<link rel=stylesheet>` and
//! `<link rel=preload/modulepreload>`.
//!
//! - Tokens are separated by ASCII whitespace; each is `alg-base64[?opts]`.
//!   `sha256`, `sha384` and `sha512` are recognized (ASCII case-insensitive);
//!   unknown algorithms and malformed tokens are ignored.
//! - Metadata with no recognized token matches anything (the spec's "no
//!   metadata" case), as does empty metadata.
//! - Only the strongest algorithm present is compared; any of its digests
//!   matching is enough.
//! - The digest may be base64 or base64url, padded or not (Chromium
//!   accepts both alphabets).
use base64::Engine as _;
use sha2::{Digest, Sha256, Sha384, Sha512};

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Alg {
    Sha256,
    Sha384,
    Sha512,
}

fn parse(token: &str) -> Option<(Alg, &str)> {
    let expr = token.split('?').next()?;
    let (alg, value) = expr.split_once('-')?;
    let alg = if alg.eq_ignore_ascii_case("sha256") {
        Alg::Sha256
    } else if alg.eq_ignore_ascii_case("sha384") {
        Alg::Sha384
    } else if alg.eq_ignore_ascii_case("sha512") {
        Alg::Sha512
    } else {
        return None;
    };
    let value = value.trim_end_matches('=');
    let valid = !value.is_empty() && value.bytes().all(|b| b.is_ascii_alphanumeric() || b"+/-_".contains(&b));
    valid.then_some((alg, value))
}

fn digest(alg: Alg, bytes: &[u8]) -> String {
    let raw = match alg {
        Alg::Sha256 => Sha256::digest(bytes).to_vec(),
        Alg::Sha384 => Sha384::digest(bytes).to_vec(),
        Alg::Sha512 => Sha512::digest(bytes).to_vec(),
    };
    base64::engine::general_purpose::STANDARD_NO_PAD.encode(raw)
}

/// Whether `metadata` (an `integrity` attribute value) has no recognized
/// hash: such metadata imposes no check.
pub fn is_empty(metadata: &str) -> bool {
    !metadata.split_ascii_whitespace().any(|t| parse(t).is_some())
}

/// SRI "do bytes match metadataList".
pub fn matches(metadata: &str, bytes: &[u8]) -> bool {
    let tokens: Vec<(Alg, &str)> = metadata.split_ascii_whitespace().filter_map(parse).collect();
    let Some(strongest) = tokens.iter().map(|(a, _)| *a).max() else { return true };
    let actual = digest(strongest, bytes);
    tokens.iter().filter(|(a, _)| *a == strongest).any(|(_, expected)| {
        expected.len() == actual.len()
            && expected.bytes().zip(actual.bytes()).all(|(e, a)| {
                let e = match e {
                    b'-' => b'+',
                    b'_' => b'/',
                    e => e,
                };
                e == a
            })
    })
}

/// The `sha256-…` expression of `bytes`, for console messages.
pub fn sha256_expression(bytes: &[u8]) -> String {
    format!("sha256-{}", base64::engine::general_purpose::STANDARD.encode(Sha256::digest(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALERT: &[u8] = b"alert('Hello, world.');";
    // Digests of ALERT (from the SRI specification's examples).
    const SHA256: &str = "sha256-qznLcsROx4GACP2dm0UCKCzCG+HiZ1guq6ZZDob/Tng=";
    const SHA384: &str = "sha384-H8BRh8j48O9oYatfu5AZzq6A9RINhZO5H16dQZngK7T62em8MUt1FLm52t+eX6xO";

    #[test]
    fn empty_and_unknown_metadata_match() {
        assert!(matches("", b"x"));
        assert!(matches("   ", b"x"));
        assert!(matches("md5-abc foo", b"x"));
        assert!(is_empty("md5-abc"));
        assert!(!is_empty(SHA256));
    }

    #[test]
    fn digest_matches() {
        assert!(matches(SHA256, ALERT));
        assert!(matches(SHA384, ALERT));
        assert!(!matches(SHA256, b"alert('Hello, world!');"));
        // Options and unpadded digests.
        assert!(matches(&format!("{SHA256}?foo=bar"), ALERT));
        assert!(matches(SHA256.trim_end_matches('='), ALERT));
        assert!(matches(&SHA256.replace("sha256", "SHA256"), ALERT));
        // base64url alphabet.
        assert!(matches(&SHA256.replace('+', "-").replace('/', "_"), ALERT));
    }

    #[test]
    fn strongest_algorithm_wins() {
        // A wrong sha384 makes a right sha256 irrelevant.
        let bad384 = "sha384-AAAAh8j48O9oYatfu5AZzq6A9RINhZO5H16dQZngK7T62em8MUt1FLm52t+eX6xO";
        assert!(!matches(&format!("{SHA256} {bad384}"), ALERT));
        // Any digest of the strongest algorithm may match.
        assert!(matches(&format!("{bad384} {SHA384}"), ALERT));
    }

    #[test]
    fn expression() {
        assert_eq!(sha256_expression(ALERT), SHA256);
    }
}
