//! Connection-Allowlist embedded enforcement (<https://github.com/WICG/connection-allowlists>), the
//! sibling of CSP Embedded Enforcement ([`crate::csp_embedded`]), as Chrome implements it:
//!
//! - an HTML `<iframe connectionallowlist="...">` requires a `Connection-Allowlist` of the
//!   documents its frame loads. The attribute counts only when it is one Structured Field inner
//!   list (CR, LF and any other text never reach a header): a frame navigation to a network URL
//!   sends it as `Sec-Required-Connection-Allowlist` on every hop (`FetchRequest::
//!   required_connection_allowlist`), taken once as the navigation starts ([`requirement`]);
//! - the response must either opt in, with `Allow-Connection-Allowlist-From` naming the
//!   embedder's origin or `*` (its document then enforces the requirement, unless the list it
//!   delivers is as strict), or deliver an enforced list that subsumes the requirement
//!   ([`ConnectionPolicy::subsumes`]); otherwise the navigation fails with `net::ERR_BLOCKED_BY_CSP`
//!   and the frame commits Chrome's opaque error document ([`judge`]);
//! - a local-scheme document (about:blank, srcdoc, data:, blob:) commits: it inherits its
//!   embedder's policies and opens no connection (enforcement there is a follow-up in Chrome too).
//!
//! `Page.setBypassCSP`: the header is still sent, but nothing is judged or enforced.

use crate::frames::FrameKey;
use crate::loader::Resource;
use crate::Browser;
use pyro_net::connection_allowlist::ConnectionPolicy;

/// The error a navigation blocked by its frame's required allowlist fails with: the loader and
/// `Browser::document_fetch_failed` treat it as a `frame-src` block (the opaque error document).
pub(crate) const BLOCKED_ERROR: &str = pyro_net::csp::BLOCKED_BY_CSP;

/// The Connection-Allowlist a navigation of child frame `key` requires: its container's
/// `connectionallowlist` attribute when the container is an HTML `<iframe>` and the value is one
/// valid inner list (an `<object>`, `<embed>` or `<frame>` has none). Read as the navigation starts,
/// like [`crate::csp_embedded::requirement`] reads its container.
pub(crate) fn requirement(browser: &Browser, key: FrameKey) -> Option<String> {
    key.child?;
    let host = browser.page(key.page)?.frames.clone();
    let owner = browser.doc(key)?.main_frame.owner?;
    let dom = host.dom.borrow();
    let el = dom.element(owner).ok()?;
    if !el.is_html() || &*el.name.local != "iframe" {
        return None;
    }
    let value = el
        .attr("connectionallowlist")
        .map(|v| v.trim_matches(|c: char| matches!(c, ' ' | '\t')))
        .filter(|v| ConnectionPolicy::is_valid_value(v))
        .map(str::to_owned);
    value
}

/// The serialized origin of the document embedding child frame `key` (`null`: none, or opaque).
fn embedder_origin(browser: &Browser, key: FrameKey) -> String {
    let parent = browser.page(key.page).map(|p| p.frames.clone()).and_then(|host| {
        let id = browser.doc(key)?.main_frame.parent.clone()?;
        host.key(&id)
    });
    parent.and_then(|parent| browser.doc(parent).map(|d| d.main_frame.origin.clone())).unwrap_or_else(|| "null".to_owned())
}

/// `resource`, the response of a navigation of `key` that required `required` as it started, as
/// it may commit: unchanged, or with the requirement as the document's own enforced list when it
/// opted in, or `Err` ([`BLOCKED_ERROR`]) when it neither opted in nor delivered a list as strict.
pub(crate) fn judge(browser: &Browser, key: FrameKey, required: Option<&str>, mut resource: Resource) -> Result<Resource, String> {
    let Some(required) = required else { return Ok(resource) };
    if browser.page(key.page).is_some_and(|p| p.bypass_csp) {
        return Ok(resource);
    }
    let Ok(url) = url::Url::parse(&resource.url) else { return Ok(resource) };
    if !matches!(url.scheme(), "http" | "https" | "ws" | "wss" | "ftp") {
        return Ok(resource);
    }
    let delivered_subsumes = resource.connection_allowlist.as_ref().is_some_and(|policy| policy.subsumes(required, &url));
    if crate::csp_embedded::allows_csp_from(resource.allow_connection_allowlist_from.as_deref(), &embedder_origin(browser, key)) {
        if !delivered_subsumes {
            resource.connection_allowlist = ConnectionPolicy::with_required(resource.connection_allowlist.as_ref(), required, &url);
        }
        return Ok(resource);
    }
    if delivered_subsumes {
        Ok(resource)
    } else {
        Err(BLOCKED_ERROR.to_owned())
    }
}
