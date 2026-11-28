//! Secure Contexts "potentially trustworthy" origins and URLs
//! (<https://w3c.github.io/webappsec-secure-contexts/#potentially-trustworthy-origin>).

use std::net::Ipv6Addr;
use url::{Host, Origin, Url};

/// Whether `origin` is potentially trustworthy: `https`/`wss`, a loopback
/// host (127.0.0.0/8, `[::1]`) or a `localhost` name (`localhost` and
/// `*.localhost`, which browsers resolve to loopback) of any scheme. An
/// opaque origin is not; see [`is_potentially_trustworthy_url`] for `file:`.
pub fn is_potentially_trustworthy_origin(origin: &Origin) -> bool {
    let Origin::Tuple(scheme, host, _) = origin else { return false };
    if scheme == "https" || scheme == "wss" {
        return true;
    }
    match host {
        Host::Ipv4(ip) => ip.is_loopback(),
        Host::Ipv6(ip) => *ip == Ipv6Addr::LOCALHOST,
        Host::Domain(name) => {
            let name = name.strip_suffix('.').unwrap_or(name);
            name.eq_ignore_ascii_case("localhost")
                || name.len() > ".localhost".len()
                    && name[name.len() - ".localhost".len()..].eq_ignore_ascii_case(".localhost")
        }
    }
}

/// "Is url potentially trustworthy?": `about:blank`, `about:srcdoc` and
/// `data:` URLs are, `file:` URLs are (their origin is opaque here), and any
/// other URL is when its origin is ([`is_potentially_trustworthy_origin`];
/// a `blob:` URL's origin is its creator's).
pub fn is_potentially_trustworthy_url(url: &Url) -> bool {
    match url.scheme() {
        "about" => matches!(url.path(), "blank" | "srcdoc"),
        "data" | "file" => true,
        _ => is_potentially_trustworthy_origin(&url.origin()),
    }
}

/// What the client does with an `http:` request hop under an [`InsecurePolicy`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InsecureStep {
    Allow,
    /// Change the scheme to `https` (Upgrade Insecure Requests, or Mixed Content's image/audio/video autoupgrade).
    Upgrade,
    Block,
}

/// Upgrade Insecure Requests §3.1 and Mixed Content §4.4 / §4.1 for one hop whose URL is `http:`.
pub(crate) fn insecure_step(policy: crate::request::InsecurePolicy, destination: crate::request::Destination, url: &Url) -> InsecureStep {
    use crate::request::Destination;
    if url.scheme() != "http" || destination == Destination::Document {
        return InsecureStep::Allow;
    }
    if policy.upgrade {
        return InsecureStep::Upgrade;
    }
    if policy.prohibits_mixed && !is_potentially_trustworthy_url(url) {
        return match destination {
            Destination::Image | Destination::Audio | Destination::Video => InsecureStep::Upgrade,
            _ => InsecureStep::Block,
        };
    }
    InsecureStep::Allow
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trustworthy(url: &str) -> bool {
        is_potentially_trustworthy_url(&Url::parse(url).unwrap())
    }

    #[test]
    fn potentially_trustworthy_urls() {
        for url in [
            "https://example.com/",
            "wss://example.com/",
            "http://localhost:3000/",
            "http://LOCALHOST/",
            "http://app.localhost:3000/x",
            "http://a.b.localhost./",
            "http://127.0.0.1/",
            "http://127.0.0.2:8080/",
            "http://127.255.255.254/",
            "http://[::1]:4000/",
            "ws://localhost/",
            "file:///tmp/a.html",
            "about:blank",
            "about:srcdoc",
            "data:text/html,x",
            "blob:https://example.com/0f0e",
        ] {
            assert!(trustworthy(url), "{url}");
        }
        for url in [
            "http://example.com/",
            "http://localhost.evil.test/",
            "http://127.0.0.1.nip.io/",
            "http://notlocalhost/",
            "http://.localhost.example/",
            "http://128.0.0.1/",
            "http://[::2]/",
            "ws://example.com/",
            "about:config",
            "blob:http://example.com/0f0e",
        ] {
            assert!(!trustworthy(url), "{url}");
        }
    }

    #[test]
    fn insecure_steps() {
        use crate::request::{Destination, InsecurePolicy};
        let http = Url::parse("http://example.test/").unwrap();
        let local = Url::parse("http://localhost/").unwrap();
        let mixed = InsecurePolicy { prohibits_mixed: true, upgrade: false };
        let upgrade = InsecurePolicy { prohibits_mixed: false, upgrade: true };
        assert_eq!(insecure_step(InsecurePolicy::default(), Destination::Fetch, &http), InsecureStep::Allow);
        assert_eq!(insecure_step(mixed, Destination::Fetch, &http), InsecureStep::Block);
        assert_eq!(insecure_step(mixed, Destination::Image, &http), InsecureStep::Upgrade);
        assert_eq!(insecure_step(mixed, Destination::Fetch, &local), InsecureStep::Allow);
        assert_eq!(insecure_step(mixed, Destination::Document, &http), InsecureStep::Allow);
        assert_eq!(insecure_step(upgrade, Destination::Script, &http), InsecureStep::Upgrade);
        assert_eq!(insecure_step(upgrade, Destination::Document, &http), InsecureStep::Allow);
    }
}
