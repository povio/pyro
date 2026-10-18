//! Page-local navigator identity and request defaults. Browser contexts may
//! share a NetClient without sharing these overrides.
use crate::PageDom;
use lp_net::{http::header::InvalidHeaderValue, http::HeaderMap, http::HeaderValue, Request, RequestDefaults};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserAgentOverride {
    /// Empty resets only the UA to the context client's default.
    pub user_agent: String,
    pub accept_language: Option<String>,
    /// Navigator only; never sent as a request header.
    pub platform: Option<String>,
}
impl UserAgentOverride {
    /// Validate before changing any page/browser state. Cache the result in
    /// the loader and clone it into each request, including navigation.
    pub fn request_defaults(&self) -> Result<RequestDefaults, InvalidHeaderValue> {
        Ok(RequestDefaults {
            user_agent: if self.user_agent.is_empty() { None } else { Some(HeaderValue::from_str(&self.user_agent)?) },
            accept_language: self.accept_language.as_deref().map(HeaderValue::from_str).transpose()?,
            extra_headers: HeaderMap::new(),
        })
    }
}

#[derive(Default)]
pub(crate) struct IdentityState {
    pub value: Option<UserAgentOverride>,
    defaults: RequestDefaults,
    /// Page-scoped `Network.setExtraHTTPHeaders`; independent of the UA override.
    extra_headers: HeaderMap,
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
    pub(crate) fn apply_request_defaults(&self, request: &mut Request) {
        let identity = self.identity.borrow();
        request.defaults =
            RequestDefaults { extra_headers: identity.extra_headers.clone(), ..identity.defaults.clone() };
    }
}
