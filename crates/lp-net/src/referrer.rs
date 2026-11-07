//! Referrer Policy (W3C Referrer Policy) and Fetch Metadata (`Sec-Fetch-*`)
//! request headers, computed per hop by the client.

use url::Url;

use crate::cookies::{is_trustworthy, site_of};
use crate::request::{Destination, RequestMode};

/// A referrer policy. The default is Chrome's (and the spec's)
/// `strict-origin-when-cross-origin`.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum ReferrerPolicy {
    NoReferrer,
    NoReferrerWhenDowngrade,
    SameOrigin,
    Origin,
    StrictOrigin,
    OriginWhenCrossOrigin,
    #[default]
    StrictOriginWhenCrossOrigin,
    UnsafeUrl,
}

impl ReferrerPolicy {
    /// The policy a token names (ASCII case-insensitive, as the header and
    /// `<meta name=referrer>` parse it); `None` for an empty or unknown one.
    pub fn parse(token: &str) -> Option<Self> {
        const ALL: [(&str, ReferrerPolicy); 8] = [
            ("no-referrer", ReferrerPolicy::NoReferrer),
            ("no-referrer-when-downgrade", ReferrerPolicy::NoReferrerWhenDowngrade),
            ("same-origin", ReferrerPolicy::SameOrigin),
            ("origin", ReferrerPolicy::Origin),
            ("strict-origin", ReferrerPolicy::StrictOrigin),
            ("origin-when-cross-origin", ReferrerPolicy::OriginWhenCrossOrigin),
            ("strict-origin-when-cross-origin", ReferrerPolicy::StrictOriginWhenCrossOrigin),
            ("unsafe-url", ReferrerPolicy::UnsafeUrl),
        ];
        let token = token.trim_matches(|c: char| c.is_ascii_whitespace());
        ALL.iter().find(|(name, _)| name.eq_ignore_ascii_case(token)).map(|(_, p)| *p)
    }

    /// The `Referrer-Policy` response header: the last token it names that
    /// is a known policy, across comma-separated values and header lines.
    pub fn from_header<'a>(values: impl IntoIterator<Item = &'a str>) -> Option<Self> {
        values.into_iter().flat_map(|v| v.split(',')).filter_map(Self::parse).last()
    }

    /// The `<meta name=referrer content>` value, which also accepts the
    /// legacy keywords `never`, `default`, `always` and `origin-when-crossorigin`.
    pub fn from_meta(content: &str) -> Option<Self> {
        let token = content.trim_matches(|c: char| c.is_ascii_whitespace());
        let legacy = [
            ("never", ReferrerPolicy::NoReferrer),
            ("default", ReferrerPolicy::StrictOriginWhenCrossOrigin),
            ("always", ReferrerPolicy::UnsafeUrl),
            ("origin-when-crossorigin", ReferrerPolicy::OriginWhenCrossOrigin),
        ];
        legacy.iter().find(|(name, _)| name.eq_ignore_ascii_case(token)).map(|(_, p)| *p).or_else(|| Self::parse(token))
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ReferrerPolicy::NoReferrer => "no-referrer",
            ReferrerPolicy::NoReferrerWhenDowngrade => "no-referrer-when-downgrade",
            ReferrerPolicy::SameOrigin => "same-origin",
            ReferrerPolicy::Origin => "origin",
            ReferrerPolicy::StrictOrigin => "strict-origin",
            ReferrerPolicy::OriginWhenCrossOrigin => "origin-when-cross-origin",
            ReferrerPolicy::StrictOriginWhenCrossOrigin => "strict-origin-when-cross-origin",
            ReferrerPolicy::UnsafeUrl => "unsafe-url",
        }
    }
}

/// Longest referrer sent in full; a longer one is reduced to its origin.
const MAX_REFERRER_LEN: usize = 4096;

/// Referrer Policy "determine request's referrer": the `Referer` value for a
/// request from `referrer` (a document or init URL) to `target`, or `None`.
/// Only http(s) referrers are sent; the fragment and credentials never are.
pub fn referrer_for(referrer: &Url, target: &Url, policy: ReferrerPolicy) -> Option<String> {
    if !matches!(referrer.scheme(), "http" | "https") {
        return None;
    }
    let origin = || referrer.origin().ascii_serialization() + "/";
    let full = || {
        let full = if referrer.fragment().is_none() && referrer.username().is_empty() && referrer.password().is_none() {
            // Nothing to strip (a document URL usually).
            referrer.as_str().to_owned()
        } else {
            let mut url = referrer.clone();
            url.set_fragment(None);
            let _ = url.set_username("");
            let _ = url.set_password(None);
            String::from(url)
        };
        if full.len() > MAX_REFERRER_LEN {
            origin()
        } else {
            full
        }
    };
    let same_origin = crate::cors::same_origin(referrer, target);
    let downgrade = is_trustworthy(referrer) && !is_trustworthy(target);
    match policy {
        ReferrerPolicy::NoReferrer => None,
        ReferrerPolicy::UnsafeUrl => Some(full()),
        ReferrerPolicy::Origin => Some(origin()),
        ReferrerPolicy::NoReferrerWhenDowngrade => (!downgrade).then(full),
        ReferrerPolicy::SameOrigin => same_origin.then(full),
        ReferrerPolicy::StrictOrigin => (!downgrade).then(origin),
        ReferrerPolicy::OriginWhenCrossOrigin => Some(if same_origin { full() } else { origin() }),
        ReferrerPolicy::StrictOriginWhenCrossOrigin => {
            if same_origin {
                Some(full())
            } else {
                (!downgrade).then(origin)
            }
        }
    }
}

/// `Sec-Fetch-Site`, ordered so a redirect chain keeps its most cross-site
/// value.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum FetchSite {
    /// A navigation the browser or user started (address bar, automation).
    None,
    SameOrigin,
    SameSite,
    CrossSite,
}

impl FetchSite {
    /// The relation of `target` to the request's initiator: `None` for an
    /// opaque (`null`) one.
    pub fn of(initiator: Option<&Url>, target: &Url) -> FetchSite {
        let Some(initiator) = initiator else { return FetchSite::CrossSite };
        if crate::cors::same_origin(initiator, target) {
            return FetchSite::SameOrigin;
        }
        let same_site = initiator.scheme() == target.scheme()
            && match (initiator.host_str(), target.host_str()) {
                (Some(a), Some(b)) => site_of(a) == site_of(b),
                _ => false,
            };
        if same_site {
            FetchSite::SameSite
        } else {
            FetchSite::CrossSite
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            FetchSite::None => "none",
            FetchSite::SameOrigin => "same-origin",
            FetchSite::SameSite => "same-site",
            FetchSite::CrossSite => "cross-site",
        }
    }
}

/// `Sec-Fetch-Mode`.
pub fn fetch_mode(mode: RequestMode, destination: Destination) -> &'static str {
    match (mode, destination) {
        (_, Destination::WebSocket) => "websocket",
        (RequestMode::Navigate, _) => "navigate",
        (RequestMode::SameOrigin, _) => "same-origin",
        (RequestMode::NoCors, _) => "no-cors",
        (RequestMode::Cors, _) => "cors",
    }
}

/// `Sec-Fetch-Dest`.
pub fn fetch_dest(destination: Destination) -> &'static str {
    match destination {
        Destination::Document => "document",
        Destination::Iframe => "iframe",
        Destination::Script => "script",
        Destination::Style => "style",
        Destination::Image => "image",
        Destination::Font => "font",
        Destination::WebSocket => "websocket",
        Destination::Xhr | Destination::Fetch | Destination::EventSource | Destination::Beacon | Destination::Other => {
            "empty"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn same_origin_matches_url_origins() {
        let urls = [
            "http://a.example/x",
            "http://A.example:80/y",
            "http://a.example:8080/",
            "https://a.example/",
            "https://a.example:443/z#f",
            "ws://a.example/",
            "wss://a.example:443/",
            "http://127.0.0.1/",
            "http://[::1]/",
            "ftp://a.example/",
            "file:///tmp/a",
            "data:text/plain,a",
            "blob:https://a.example/uuid",
            "about:blank",
        ];
        for a in urls {
            for b in urls {
                let (a, b) = (u(a), u(b));
                assert_eq!(crate::cors::same_origin(&a, &b), a.origin() == b.origin(), "{a} {b}");
            }
        }
        // Nothing to strip: the URL as it is.
        assert_eq!(
            referrer_for(&u("https://a.example/p?q"), &u("https://a.example/"), ReferrerPolicy::UnsafeUrl).as_deref(),
            Some("https://a.example/p?q")
        );
    }

    #[test]
    fn policies_strip_and_downgrade() {
        let from = u("https://user:pw@a.example/path?q=1#frag");
        let same = u("https://a.example/other");
        let cross = u("https://b.example/");
        let insecure = u("http://b.example/");
        use ReferrerPolicy::*;
        let full = Some("https://a.example/path?q=1".to_owned());
        let origin = Some("https://a.example/".to_owned());
        let cases = [
            (NoReferrer, None, None, None),
            (UnsafeUrl, full.clone(), full.clone(), full.clone()),
            (Origin, origin.clone(), origin.clone(), origin.clone()),
            (NoReferrerWhenDowngrade, full.clone(), full.clone(), None),
            (SameOrigin, full.clone(), None, None),
            (StrictOrigin, origin.clone(), origin.clone(), None),
            (OriginWhenCrossOrigin, full.clone(), origin.clone(), origin.clone()),
            (StrictOriginWhenCrossOrigin, full.clone(), origin.clone(), None),
        ];
        for (policy, to_same, to_cross, to_insecure) in cases {
            assert_eq!(referrer_for(&from, &same, policy), to_same, "{policy:?} same");
            assert_eq!(referrer_for(&from, &cross, policy), to_cross, "{policy:?} cross");
            assert_eq!(referrer_for(&from, &insecure, policy), to_insecure, "{policy:?} downgrade");
        }
        // http://localhost is trustworthy: no downgrade from https.
        assert!(referrer_for(&from, &u("http://localhost:3000/"), StrictOrigin).is_some());
        assert_eq!(referrer_for(&u("about:blank"), &same, UnsafeUrl), None);
        let long = u(&format!("https://a.example/{}", "x".repeat(5000)));
        assert_eq!(referrer_for(&long, &same, UnsafeUrl), origin);
    }

    #[test]
    fn policy_tokens() {
        assert_eq!(ReferrerPolicy::parse(" Origin "), Some(ReferrerPolicy::Origin));
        assert_eq!(ReferrerPolicy::parse("bogus"), None);
        assert_eq!(
            ReferrerPolicy::from_header(["no-referrer, bogus", "unsafe-url, nope"]),
            Some(ReferrerPolicy::UnsafeUrl)
        );
        assert_eq!(ReferrerPolicy::from_header(["bogus"]), None);
        assert_eq!(ReferrerPolicy::from_meta("never"), Some(ReferrerPolicy::NoReferrer));
        assert_eq!(ReferrerPolicy::from_meta("same-origin"), Some(ReferrerPolicy::SameOrigin));
    }

    #[test]
    fn fetch_site_relations() {
        let a = u("https://www.example.com/");
        assert_eq!(FetchSite::of(Some(&a), &u("https://www.example.com/x")), FetchSite::SameOrigin);
        assert_eq!(FetchSite::of(Some(&a), &u("https://api.example.com/")), FetchSite::SameSite);
        assert_eq!(FetchSite::of(Some(&a), &u("http://www.example.com/")), FetchSite::CrossSite);
        assert_eq!(FetchSite::of(Some(&a), &u("https://other.test/")), FetchSite::CrossSite);
        assert_eq!(FetchSite::of(None, &a), FetchSite::CrossSite);
        assert!(FetchSite::SameOrigin.max(FetchSite::CrossSite) == FetchSite::CrossSite);
    }
}
