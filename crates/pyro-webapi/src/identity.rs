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
    pub(crate) fn apply_request_defaults(&self, request: &mut Request) {
        request.top_level_site = self.cookie_partition_key();
        request.cross_site_ancestor = self.cookie_site_context() == pyro_net::cookies::SiteContext::CrossSite;
        let identity = self.identity.borrow();
        request.defaults = RequestDefaults {
            extra_headers: identity.extra_headers.clone(),
            cache_disabled: identity.cache_disabled,
            ..identity.defaults.clone()
        };
    }
}
