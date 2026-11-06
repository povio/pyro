//! Subresource Integrity for `fetch(url, {integrity})`: the metadata is
//! parsed when the request starts and checked against the whole body.

use base64::Engine as _;
use bytes::Bytes;
use sha2::{Digest, Sha256, Sha384, Sha512};

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Algorithm {
    Sha256,
    Sha384,
    Sha512,
}

/// The strongest algorithm of the metadata and the digests given for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Integrity {
    algorithm: Algorithm,
    digests: Vec<Vec<u8>>,
}

impl Integrity {
    /// SRI "parse metadata": whitespace-separated `alg-base64[?options]`
    /// tokens (base64 or base64url, padding optional). `None` when no token
    /// is valid, which matches any response.
    pub(crate) fn parse(metadata: &str) -> Option<Integrity> {
        let mut tokens: Vec<(Algorithm, Vec<u8>)> = Vec::new();
        for token in metadata.split_ascii_whitespace() {
            let token = token.split('?').next().unwrap_or("");
            let Some((alg, value)) = token.split_once('-') else { continue };
            let algorithm = if alg.eq_ignore_ascii_case("sha256") {
                Algorithm::Sha256
            } else if alg.eq_ignore_ascii_case("sha384") {
                Algorithm::Sha384
            } else if alg.eq_ignore_ascii_case("sha512") {
                Algorithm::Sha512
            } else {
                continue;
            };
            let normalized: String = value
                .trim_end_matches('=')
                .chars()
                .map(|c| match c {
                    '-' => '+',
                    '_' => '/',
                    c => c,
                })
                .collect();
            if let Ok(digest) = base64::engine::general_purpose::STANDARD_NO_PAD.decode(normalized) {
                tokens.push((algorithm, digest));
            }
        }
        let strongest = tokens.iter().map(|(a, _)| *a).max()?;
        let digests = tokens.into_iter().filter(|(a, _)| *a == strongest).map(|(_, d)| d).collect();
        Some(Integrity { algorithm: strongest, digests })
    }

    /// Whether the body, in its chunks, has one of the expected digests.
    pub(crate) fn matches(&self, chunks: &[Bytes]) -> bool {
        fn digest<D: Digest>(chunks: &[Bytes]) -> Vec<u8> {
            let mut d = D::new();
            for c in chunks {
                d.update(c);
            }
            d.finalize().to_vec()
        }
        let actual = match self.algorithm {
            Algorithm::Sha256 => digest::<Sha256>(chunks),
            Algorithm::Sha384 => digest::<Sha384>(chunks),
            Algorithm::Sha512 => digest::<Sha512>(chunks),
        };
        self.digests.iter().any(|d| *d == actual)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b64(alg: &str, data: &[u8]) -> String {
        let digest = match alg {
            "sha256" => Sha256::digest(data).to_vec(),
            "sha384" => Sha384::digest(data).to_vec(),
            _ => Sha512::digest(data).to_vec(),
        };
        format!("{alg}-{}", base64::engine::general_purpose::STANDARD.encode(digest))
    }

    #[test]
    fn strongest_algorithm_decides() {
        let body = [Bytes::from_static(b"top"), Bytes::from_static(b".txt\n")];
        let whole = b"top.txt\n";
        let good256 = b64("sha256", whole);
        let good512 = b64("sha512", whole);
        let bad512 = b64("sha512", b"other");
        assert!(Integrity::parse("").is_none());
        assert!(Integrity::parse("md5-abc bogus").is_none());
        assert!(Integrity::parse(&good256).unwrap().matches(&body));
        assert!(Integrity::parse(&format!("{good256}?opt=1")).unwrap().matches(&body));
        assert!(!Integrity::parse(&b64("sha256", b"x")).unwrap().matches(&body));
        assert!(Integrity::parse(&format!("{} {good512}", b64("sha256", b"x"))).unwrap().matches(&body));
        assert!(!Integrity::parse(&format!("{bad512} {good256}")).unwrap().matches(&body));
        assert!(Integrity::parse(&format!("{bad512} {good512}")).unwrap().matches(&body));
        let url_safe = good512.replace('+', "-").replace('/', "_");
        assert!(Integrity::parse(url_safe.trim_end_matches('=')).unwrap().matches(&body));
    }
}
