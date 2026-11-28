//! Connection-Allowlist embedded enforcement (<https://github.com/WICG/connection-allowlists>), the
//! sibling of CSP Embedded Enforcement ([`crate::csp_embedded`]), as Chrome implements it:
//!
//! - an HTML `<iframe connectionallowlist="...">` requires a `Connection-Allowlist` of the
//!   documents its frame loads. The attribute counts only when it is one Structured Field inner
//!   list (CR, LF and any other text never reach a header): a frame navigation to a network URL
//!   sends it as `Sec-Required-Connection-Allowlist` on every hop (`FetchRequest::
//!   required_connection_allowlist`), taken once as the navigation starts ([`requirement`]);
//! - the response must either opt in, with `Allow-Connection-Allowlist-From` naming the
//!   embedder's origin or `*`, or deliver an enforced list that subsumes the requirement
//!   ([`ConnectionPolicy::subsumes`]); otherwise the navigation fails with `net::ERR_BLOCKED_BY_CSP`
//!   and the frame commits Chrome's opaque error document ([`judge`]). An opted-in document
//!   enforces the requirement IN ADDITION to the list it delivered (a request must pass both: an
//!   embedder can narrow what a server restricted itself to, never widen it);
//! - a local-scheme document (about:blank, srcdoc, blob:, data:) is not refused but enforces the
//!   requirement too, besides what it inherited from its creator (`data:` inherits nothing; the
//!   requirement still applies to it): [`judge`] for the ones that arrive as a resource,
//!   [`enforce_on_blank`] for the initial about:blank document.
//!
//! Known follow-ups (not done here): a frame NESTED in a required frame does not inherit the
//! requirement, as the CSP sibling's `required_csp` does; and a navigation that targets an existing
//! named frame of the page is guarded by that frame's document, not the initiator's.
//!
//! `Page.setBypassCSP`: the header is still sent, but nothing is judged or enforced.

use crate::frames::FrameKey;
use crate::loader::Resource;
use crate::Browser;
use pyro_net::connection_allowlist::ConnectionPolicy;
use std::sync::Arc;

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

/// The frame holding child frame `key`'s document.
fn parent_key(browser: &Browser, key: FrameKey) -> Option<FrameKey> {
    let host = browser.page(key.page)?.frames.clone();
    let id = browser.doc(key)?.main_frame.parent.clone()?;
    host.key(&id)
}

/// The serialized origin of the document embedding child frame `key` (`null`: none, or opaque).
fn embedder_origin(browser: &Browser, key: FrameKey) -> String {
    parent_key(browser, key)
        .and_then(|parent| browser.doc(parent).map(|d| d.main_frame.origin.clone()))
        .unwrap_or_else(|| "null".to_owned())
}

/// What a local-scheme document of child frame `key` starts from: the allowlist of its creator
/// (`about:` and `blob:` inherit it; `data:` and anything else do not) and the creator's URL, which
/// is what `response-origin` means for it (a `data:` document's own URL: an opaque origin, so only
/// patterns match).
fn local_base(browser: &Browser, key: FrameKey, url: &url::Url) -> (Option<Arc<ConnectionPolicy>>, url::Url) {
    if !matches!(url.scheme(), "about" | "blob") {
        return (None, url.clone());
    }
    let creator = parent_key(browser, key).and_then(|parent| {
        let doc = browser.doc(parent)?;
        let dom = doc.main_frame.page_dom.clone()?;
        Some((dom.connection_allowlist(), url::Url::parse(&doc.main_frame.url).ok()?))
    });
    match creator {
        Some((allowlist, creator_url)) => (allowlist, creator_url),
        None => (None, url.clone()),
    }
}

/// The policy a network response's document ends up with under requirement `required`: `Ok` its own
/// (`delivered`), when that subsumes the requirement, or, when the response opted in
/// (`allow_from`), its own with the requirement added; `Err` ([`BLOCKED_ERROR`]) when it neither
/// delivered a list as strict nor opted in.
fn decide(
    required: &str,
    url: &url::Url,
    delivered: Option<&Arc<ConnectionPolicy>>,
    allow_from: Option<&str>,
    embedder_origin: &str,
) -> Result<Option<Arc<ConnectionPolicy>>, String> {
    if delivered.is_some_and(|policy| policy.subsumes(required, url)) {
        return Ok(delivered.cloned());
    }
    if crate::csp_embedded::allows_csp_from(allow_from, embedder_origin) {
        return ConnectionPolicy::with_required(delivered, required, url).map(Some).ok_or_else(|| BLOCKED_ERROR.to_owned());
    }
    Err(BLOCKED_ERROR.to_owned())
}

/// `resource`, the response of a navigation of `key` that required `required` as it started, as
/// it may commit: with the connection allowlist its document must have ([`decide`], or for a
/// local-scheme document its creator's with the requirement added), or `Err` ([`BLOCKED_ERROR`]).
/// A page that bypasses CSP (`Page.setBypassCSP`) judges nothing.
pub(crate) fn judge(browser: &Browser, key: FrameKey, required: Option<&str>, mut resource: Resource) -> Result<Resource, String> {
    let Some(required) = required else { return Ok(resource) };
    if browser.page(key.page).is_some_and(|p| p.bypass_csp) {
        return Ok(resource);
    }
    // A URL that does not parse cannot be told to be local: judged as a network one.
    let Ok(url) = url::Url::parse(&resource.url) else { return Err(BLOCKED_ERROR.to_owned()) };
    if matches!(url.scheme(), "http" | "https" | "ws" | "wss" | "ftp") {
        resource.connection_allowlist =
            decide(required, &url, resource.connection_allowlist.as_ref(), resource.allow_connection_allowlist_from.as_deref(), &embedder_origin(browser, key))?;
    } else {
        let (base, base_url) = local_base(browser, key, &url);
        resource.connection_allowlist = ConnectionPolicy::with_required(base.as_ref(), required, &base_url);
    }
    Ok(resource)
}

/// The initial about:blank document of child frame `key` (just committed) enforces `required`
/// besides the allowlist it inherited from its creator.
pub(crate) fn enforce_on_blank(browser: &Browser, key: FrameKey, required: &str) {
    if browser.page(key.page).is_some_and(|p| p.bypass_csp) {
        return;
    }
    let Some(dom) = browser.doc(key).and_then(|d| d.main_frame.page_dom.clone()) else { return };
    let Ok(url) = url::Url::parse("about:blank") else { return };
    let (base, base_url) = local_base(browser, key, &url);
    // What the document inherited is what it enforces already (the creator's, now), so the creator's
    // list is read from the document itself when it has one.
    let own = dom.connection_allowlist().or(base);
    if let Some(policy) = ConnectionPolicy::with_required(own.as_ref(), required, &base_url) {
        dom.set_connection_allowlist(Some(policy));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(value: &str) -> Arc<ConnectionPolicy> {
        ConnectionPolicy::from_headers(Some(value), None, &url::Url::parse("http://child.test:8000/frame").unwrap()).unwrap()
    }

    /// An opted-in framed document enforces what it delivered AND what its embedder requires: the
    /// requirement allows c.test, the document restricted itself to b.test, and neither is reachable.
    #[test]
    fn opt_in_adds_the_requirement_to_the_delivered_list() {
        let url = url::Url::parse("http://child.test:8000/frame").unwrap();
        let required = "(\"http://c.test:8000/*\")";
        let delivered = policy("(\"http://b.test:8000/*\")");
        let made = decide(required, &url, Some(&delivered), Some("*"), "http://embedder.test").unwrap().unwrap();
        for target in ["http://b.test:8000/x", "http://c.test:8000/x", "http://child.test:8000/x"] {
            let target = url::Url::parse(target).unwrap();
            assert!(made.check(&target, &target, false).blocked, "{target}");
        }
        // Nothing delivered: the requirement alone.
        let made = decide(required, &url, None, Some("http://embedder.test"), "http://embedder.test").unwrap().unwrap();
        let c = url::Url::parse("http://c.test:8000/x").unwrap();
        assert!(made.check(&c, &c, false).allowed());
        // Neither opted in nor subsuming: blocked; a subsuming delivered list needs no opt-in.
        assert!(decide(required, &url, Some(&delivered), None, "http://embedder.test").is_err());
        assert!(decide(required, &url, None, Some("http://other.test"), "http://embedder.test").is_err());
        let subsuming = policy("(\"http://c.test:8000/*\")");
        assert!(decide(required, &url, Some(&subsuming), None, "http://embedder.test").is_ok());
    }
}
