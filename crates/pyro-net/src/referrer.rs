//! Referrer Policy (W3C Referrer Policy) and Fetch Metadata (`Sec-Fetch-*`)
//! request headers, computed per hop by the client.

use url::Url;

use crate::cookies::{is_trustworthy, site_of};
use crate::request::{Destination, RequestMode};

/// A referrer policy. The default is Chrome's (and the spec's)
/// `strict-origin-when-cross-origin`.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
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

    /// The state of a `referrerpolicy` content attribute (an enumerated
    /// attribute: a keyword, ASCII case-insensitive, nothing around it);
    /// `None` for the empty state (missing, empty or unknown value), in
    /// which the document's policy applies.
    pub fn from_attribute(value: Option<&str>) -> Option<Self> {
        let space = |c: char| c.is_ascii_whitespace();
        value.filter(|v| !v.starts_with(space) && !v.ends_with(space)).and_then(Self::parse)
    }

    /// The policy of a link or `window.open` feature list: `rel=noreferrer`
    /// sends none whatever `referrerpolicy` says.
    pub fn of_link(rel: Option<&str>, referrerpolicy: Option<&str>) -> Option<Self> {
        if rel.is_some_and(|rel| rel.split_ascii_whitespace().any(|t| t.eq_ignore_ascii_case("noreferrer"))) {
            return Some(ReferrerPolicy::NoReferrer);
        }
        Self::from_attribute(referrerpolicy)
    }

    /// The `Referrer-Policy` response header: the last token it names that
    /// is a known policy, across comma-separated values and header lines.
    ///
    /// An unknown token is skipped (a newer policy an older browser falls
    /// back from); an item that is not a token at all (`origin no-referrer`)
    /// makes the whole header give no policy, as in Chromium.
    pub fn from_header<'a>(values: impl IntoIterator<Item = &'a str>) -> Option<Self> {
        let is_tchar = |c: char| c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(c);
        let mut policy = None;
        for item in values.into_iter().flat_map(|v| v.split(',')) {
            match Self::parse(item) {
                Some(p) => policy = Some(p),
                None if item.trim_matches(|c: char| c.is_ascii_whitespace()).chars().all(is_tchar) => {}
                None => return None,
            }
        }
        policy
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

/// A request's referrer across its redirect chain. Fetch sets a request's
/// referrer to the value "determine request's referrer" gave (main fetch),
/// so each hop after a redirect starts from what the hop before it sent,
/// not from the original referrer: a referrer cut to its origin (or dropped)
/// on a cross-origin hop stays cut when a redirect leads back.
#[derive(Clone, Debug)]
pub struct ReferrerChain {
    /// The referrer source of the hop being made.
    pub referrer: Option<Url>,
    pub policy: ReferrerPolicy,
}

impl ReferrerChain {
    pub fn new(referrer: Option<Url>, policy: ReferrerPolicy) -> Self {
        ReferrerChain { referrer, policy }
    }

    /// The `Referer` value of a hop to `target`.
    pub fn header(&self, target: &Url) -> Option<String> {
        self.referrer.as_ref().and_then(|r| referrer_for(r, target, self.policy))
    }

    /// A redirect answered the hop to `from` with `headers`: the next hop's
    /// referrer is the one `from` was sent, and a `Referrer-Policy` header
    /// naming a known policy replaces the request's.
    pub fn redirect(&mut self, from: &Url, headers: &http::HeaderMap) {
        self.referrer = self.header(from).and_then(|r| Url::parse(&r).ok());
        if let Some(policy) =
            ReferrerPolicy::from_header(headers.get_all("referrer-policy").iter().filter_map(|v| v.to_str().ok()))
        {
            self.policy = policy;
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
                // A host's registrable domain keeps its trailing dot (URL "host public
                // suffix"): `example.com.` and `example.com` are different sites.
                (Some(a), Some(b)) => site_of(a) == site_of(b) && a.ends_with('.') == b.ends_with('.'),
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
        Destination::Worker => "worker",
        Destination::SharedWorker => "sharedworker",
        Destination::ServiceWorker => "serviceworker",
        Destination::Style => "style",
        Destination::Json => "json",
        Destination::Text => "text",
        Destination::Image => "image",
        Destination::Font => "font",
        Destination::Audio => "audio",
        Destination::Video => "video",
        Destination::Track => "track",
        Destination::WebSocket => "websocket",
        Destination::Xhr
        | Destination::Fetch
        | Destination::EventSource
        | Destination::Beacon
        | Destination::CompressionDictionary
        | Destination::Other => "empty",
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
    fn a_redirect_chain_keeps_what_the_hop_before_sent() {
        use ReferrerPolicy::*;
        let page = u("http://a.example/page?q");
        let same = u("http://a.example/r");
        let cross = u("http://b.example/r");
        let none = http::HeaderMap::new();
        // (policy, sent to a cross-origin hop, then sent after its redirect back to the page's origin)
        let cases = [
            (NoReferrer, None, None),
            (UnsafeUrl, Some("http://a.example/page?q"), Some("http://a.example/page?q")),
            (Origin, Some("http://a.example/"), Some("http://a.example/")),
            (SameOrigin, None, None),
            (StrictOrigin, Some("http://a.example/"), Some("http://a.example/")),
            (OriginWhenCrossOrigin, Some("http://a.example/"), Some("http://a.example/")),
            (StrictOriginWhenCrossOrigin, Some("http://a.example/"), Some("http://a.example/")),
            (NoReferrerWhenDowngrade, Some("http://a.example/page?q"), Some("http://a.example/page?q")),
        ];
        for (policy, first, back) in cases {
            let mut chain = ReferrerChain::new(Some(page.clone()), policy);
            assert_eq!(chain.header(&cross).as_deref(), first, "{policy:?} first hop");
            chain.redirect(&cross, &none);
            assert_eq!(chain.header(&same).as_deref(), back, "{policy:?} back to the origin");
        }
        // Same-origin first, then away: the full URL, then the policy again.
        let mut chain = ReferrerChain::new(Some(page.clone()), SameOrigin);
        assert_eq!(chain.header(&same).as_deref(), Some("http://a.example/page?q"));
        chain.redirect(&same, &none);
        assert_eq!(chain.header(&cross), None);
        // A redirect's own policy applies from the next hop on.
        let mut chain = ReferrerChain::new(Some(page), UnsafeUrl);
        let mut headers = http::HeaderMap::new();
        headers.insert("referrer-policy", "bogus, origin".parse().unwrap());
        chain.redirect(&same, &headers);
        assert_eq!(chain.header(&same).as_deref(), Some("http://a.example/"));
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
        assert_eq!(ReferrerPolicy::from_header(["origin, ,same-origin,"]), Some(ReferrerPolicy::SameOrigin));
        // Not a token: the header as a whole is dropped.
        assert_eq!(ReferrerPolicy::from_header(["origin", "origin no-referrer"]), None);
        assert_eq!(ReferrerPolicy::from_header(["origin no-referrer", "origin"]), None);
        assert_eq!(ReferrerPolicy::from_meta("never"), Some(ReferrerPolicy::NoReferrer));
        assert_eq!(ReferrerPolicy::from_meta("same-origin"), Some(ReferrerPolicy::SameOrigin));
        assert_eq!(ReferrerPolicy::from_attribute(Some("Unsafe-URL")), Some(ReferrerPolicy::UnsafeUrl));
        for empty_state in [None, Some(""), Some(" origin"), Some("never"), Some("bogus")] {
            assert_eq!(ReferrerPolicy::from_attribute(empty_state), None, "{empty_state:?}");
        }
        let noreferrer = ReferrerPolicy::of_link(Some("nofollow NoReferrer"), Some("unsafe-url"));
        assert_eq!(noreferrer, Some(ReferrerPolicy::NoReferrer));
        assert_eq!(ReferrerPolicy::of_link(Some("noopener"), Some("origin")), Some(ReferrerPolicy::Origin));
        assert_eq!(ReferrerPolicy::of_link(None, None), None);
    }

    #[test]
    fn fetch_site_relations() {
        let a = u("https://www.example.com/");
        assert_eq!(FetchSite::of(Some(&a), &u("https://www.example.com/x")), FetchSite::SameOrigin);
        assert_eq!(FetchSite::of(Some(&a), &u("https://api.example.com/")), FetchSite::SameSite);
        assert_eq!(FetchSite::of(Some(&a), &u("https://www.example.com./x")), FetchSite::CrossSite, "trailing dot");
        assert_eq!(FetchSite::of(Some(&a), &u("https://api.example.com./")), FetchSite::CrossSite, "trailing dot");
        assert_eq!(FetchSite::of(Some(&a), &u("http://www.example.com/")), FetchSite::CrossSite);
        assert_eq!(FetchSite::of(Some(&a), &u("https://other.test/")), FetchSite::CrossSite);
        assert_eq!(FetchSite::of(None, &a), FetchSite::CrossSite);
        assert!(FetchSite::SameOrigin.max(FetchSite::CrossSite) == FetchSite::CrossSite);
    }
}
