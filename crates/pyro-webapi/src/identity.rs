//! Page-local navigator identity and request defaults. Browser contexts may
//! share a NetClient without sharing these overrides.
use crate::PageDom;
use pyro_net::{http::header::InvalidHeaderValue, http::HeaderMap, http::HeaderValue, Request, RequestDefaults};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserAgentOverride {
    /// Empty resets only the UA to the context client's default.
    pub user_agent: String,
    pub accept_language: Option<String>,
    /// Navigator only; never sent as a request header.
    pub platform: Option<String>,
    /// Client hints (`navigator.userAgentData`); `None` derives them from
    /// the user agent string.
    pub metadata: Option<crate::navigator_extras::UserAgentMetadata>,
}
impl UserAgentOverride {
    /// Validate before changing any page/browser state. Cache the result in
    /// the loader and clone it into each request, including navigation.
    pub fn request_defaults(&self) -> Result<RequestDefaults, InvalidHeaderValue> {
        Ok(RequestDefaults {
            user_agent: if self.user_agent.is_empty() { None } else { Some(HeaderValue::from_str(&self.user_agent)?) },
            accept_language: self.accept_language.as_deref().map(HeaderValue::from_str).transpose()?,
            extra_headers: HeaderMap::new(),
            cache_disabled: false,
            client_hints: self.metadata.as_ref().map(|m| std::sync::Arc::new(client_hint_headers(m))),
            insecure: pyro_net::InsecurePolicy::default(),
            embedder_policy: pyro_net::EmbedderPolicy::default(),
            embedder_origin: None,
            cookie_blocking: None,
            corp_report: None,
            ephemeral_cookies: None,
            no_accept_ch: false,
            ch_denied: None,
        })
    }
}

/// Chrome's low-entropy client hint headers for `metadata` (no Accept-CH
/// opt-in to the high-entropy ones). `Sec-CH-UA` is left out when there
/// are no brands (metadata without brands, as Playwright sends, takes the
/// browser's own, which pyro's default agent has none of).
pub(crate) fn client_hint_headers(
    metadata: &crate::navigator_extras::UserAgentMetadata,
) -> Vec<(pyro_net::http::HeaderName, HeaderValue)> {
    use pyro_net::http::HeaderName;
    // Structured-header strings: quotes and backslashes escaped. Metadata
    // strings are printable ASCII (validated), so the values are valid.
    let item = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
    let mut out = Vec::with_capacity(3);
    let brands = metadata.brands.as_deref().unwrap_or_default();
    if !brands.is_empty() {
        let list: Vec<String> = brands.iter().map(|(b, v)| format!("{};v={}", item(b), item(v))).collect();
        if let Ok(v) = HeaderValue::from_str(&list.join(", ")) {
            out.push((HeaderName::from_static("sec-ch-ua"), v));
        }
    }
    out.push((
        HeaderName::from_static("sec-ch-ua-mobile"),
        HeaderValue::from_static(if metadata.mobile { "?1" } else { "?0" }),
    ));
    if let Ok(v) = HeaderValue::from_str(&item(&metadata.platform)) {
        out.push((HeaderName::from_static("sec-ch-ua-platform"), v));
    }
    out
}

#[derive(Default)]
pub(crate) struct IdentityState {
    pub value: Option<UserAgentOverride>,
    defaults: RequestDefaults,
    /// Page-scoped `Network.setExtraHTTPHeaders`; independent of the UA override.
    extra_headers: HeaderMap,
    /// Page-scoped `Network.setCacheDisabled`.
    cache_disabled: bool,
    /// Page-scoped `Network.setBypassServiceWorker`: fetch/XHR skip the
    /// controlling service worker.
    bypass_service_worker: bool,
    /// A worker's Mixed Content / Upgrade Insecure Requests state from its creator (its own
    /// document has no window origin to ask): `None` for a window.
    creator_insecure: Option<pyro_net::InsecurePolicy>,
    /// The document's cross-origin embedder policy (HTML § 7.1.4): from its response, or for a local-scheme
    /// document the one it inherits; a worker's is its owner's (or its script response's).
    embedder_policy: pyro_net::EmbedderPolicy,
    embedder_reporting: pyro_net::EmbedderReporting,
    /// A service worker's page: its requests carry no `Accept-CH` hints of their own.
    no_accept_ch: bool,
}
impl PageDom {
    /// Applies atomically to existing navigator wrappers/worlds and future
    /// fetch/XHR requests. The browser owns persistence across navigation.
    /// None resets all fields; this never mutates the shared network client.
    pub fn set_user_agent_override(&self, value: Option<UserAgentOverride>) -> Result<(), InvalidHeaderValue> {
        let defaults = value.as_ref().map(UserAgentOverride::request_defaults).transpose()?.unwrap_or_default();
        let mut identity = self.identity.borrow_mut();
        identity.value = value;
        identity.defaults = defaults;
        Ok(())
    }
    /// Page-scoped extra request headers for future fetch/XHR requests, as CDP
    /// `Network.setExtraHTTPHeaders` on this page's session: each call replaces
    /// the whole set and an empty map clears it. Sibling pages sharing the
    /// context's network client are unaffected, and the UA override is kept.
    /// Validation of names/values is the caller's (they are already typed).
    pub fn set_extra_http_headers(&self, headers: HeaderMap) {
        self.identity.borrow_mut().extra_headers = headers;
    }
    pub fn extra_http_headers(&self) -> HeaderMap {
        self.identity.borrow().extra_headers.clone()
    }
    /// Page-scoped HTTP cache bypass for future fetch/XHR requests, as CDP
    /// `Network.setCacheDisabled` on this page's session. Sibling pages sharing
    /// the context's network client keep using its cache.
    pub fn set_cache_disabled(&self, disabled: bool) {
        self.identity.borrow_mut().cache_disabled = disabled;
    }
    pub fn cache_disabled(&self) -> bool {
        self.identity.borrow().cache_disabled
    }
    pub fn set_bypass_service_worker(&self, bypass: bool) {
        self.identity.borrow_mut().bypass_service_worker = bypass;
    }
    pub fn bypass_service_worker(&self) -> bool {
        self.identity.borrow().bypass_service_worker
    }
    /// A worker's page takes its creator's Mixed Content / Upgrade Insecure Requests state.
    pub(crate) fn set_creator_insecure(&self, policy: pyro_net::InsecurePolicy) {
        self.identity.borrow_mut().creator_insecure = Some(policy);
    }
    /// Mixed Content / Upgrade Insecure Requests state of requests this context makes, for
    /// `RequestDefaults::insecure`. `upgrade`: an enforced CSP of the context (header, `<meta>`
    /// or inherited) has `upgrade-insecure-requests`; a worker also keeps its creator's.
    /// `prohibits_mixed`: an `https:` origin; for an opaque origin, a secure context whose
    /// URL is https/data/blob (the frame chain's own rule is the embedder's `is_secure_context`).
    pub fn insecure_policy(&self) -> pyro_net::InsecurePolicy {
        let creator = self.identity.borrow().creator_insecure;
        let own_upgrade = crate::csp::upgrades_insecure_requests(self);
        match creator {
            Some(c) => pyro_net::InsecurePolicy { prohibits_mixed: c.prohibits_mixed, upgrade: c.upgrade || own_upgrade },
            None => pyro_net::InsecurePolicy { prohibits_mixed: self.window_prohibits_mixed(), upgrade: own_upgrade },
        }
    }
    /// Sets the document's cross-origin embedder policy, as its navigation commits.
    pub fn set_embedder_policy(&self, policy: pyro_net::EmbedderPolicy) {
        self.identity.borrow_mut().embedder_policy = policy;
    }
    /// The cross-origin embedder policy requests this context makes are subject to
    /// (`RequestDefaults::embedder_policy`).
    pub fn embedder_policy(&self) -> pyro_net::EmbedderPolicy {
        self.identity.borrow().embedder_policy
    }
    /// Sets the reporting parts of the document's embedder policies (`report-to`, the report-only
    /// policy), as its navigation commits next to `set_embedder_policy`.
    pub fn set_embedder_reporting(&self, reporting: pyro_net::EmbedderReporting) {
        self.identity.borrow_mut().embedder_reporting = reporting;
    }
    /// Marks this page as a service worker's: the requests it makes get no `Accept-CH` hints added.
    pub(crate) fn set_no_accept_ch(&self, on: bool) {
        self.identity.borrow_mut().no_accept_ch = on;
    }
    /// The reporting parts of the embedder policies this context is subject to.
    pub fn embedder_reporting(&self) -> pyro_net::EmbedderReporting {
        self.identity.borrow().embedder_reporting.clone()
    }
    fn window_prohibits_mixed(&self) -> bool {
        if let Some(origin) = self.main_world_realm().and_then(|r| r.origin()) {
            if origin.starts_with("https:") {
                return true;
            }
            if origin.starts_with("http:") {
                return false;
            }
        }
        // An opaque origin (sandboxed frame, `data:` document).
        let scheme = self
            .dom
            .borrow()
            .document_data(self.document())
            .ok()
            .and_then(|d| pyro_net::Url::parse(&d.url.to_string()).ok())
            .map(|u| u.scheme().to_owned());
        self.hooks_secure_context() == Some(true) && scheme.is_some_and(|s| matches!(s.as_str(), "https" | "data" | "blob"))
    }
    pub(crate) fn apply_request_defaults(&self, request: &mut Request) {
        request.top_level_site = self.cookie_partition_key();
        request.cross_site_ancestor = self.cookie_site_context() == pyro_net::cookies::SiteContext::CrossSite;
        let cookie_blocking = self.cookie_blocking();
        let identity = self.identity.borrow();
        request.defaults = RequestDefaults {
            cookie_blocking,
            ephemeral_cookies: self.anonymous().map(|a| a.cookies),
            extra_headers: identity.extra_headers.clone(),
            cache_disabled: identity.cache_disabled,
            insecure: self.insecure_policy(),
            embedder_policy: identity.embedder_policy,
            no_accept_ch: identity.no_accept_ch,
            ch_denied: crate::permissions_policy::denied_client_hints(self),
            ..identity.defaults.clone()
        };
        drop(identity);
        self.apply_embedder_defaults(&mut request.defaults);
    }
}
