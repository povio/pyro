//! `navigator.credentials` (Credential Management: `Credential`, `CredentialsContainer`,
//! `PasswordCredential`, `FederatedCredential`) and Web Authentication (`PublicKeyCredential`,
//! `AuthenticatorResponse` and its two subclasses): made in JS (credentials.js), which does the
//! WebIDL argument conversion, the client steps of `create()` / `get()` (rpId, clientDataJSON,
//! timeouts, abort, extensions) and builds the credential objects. What needs the engine is native:
//! the document's frame facts (origin, permissions policy, user activation) and the authenticators
//! (`crate::webauthn`, the context's virtual authenticators). `[SecureContext]`
//! (`crate::secure_context`).

use std::rc::Rc;

use pyro_js::{JsError, JsResult, Object, Scope, Value};

use crate::lazy_module::LazyModule;
use crate::page::{FrameContainer, PageDom};

pub(crate) static MODULE: LazyModule = LazyModule {
    key: "lpCredentials",
    source: include_str!("credentials.js"),
    url: "pyro://internal/credentials.js",
    natives: Some(natives),
};

/// The interfaces on a window global (absent outside a secure context).
pub(crate) fn define_globals<W>(def: &mut pyro_js::ClassDef<W>)
where
    W: pyro_js::JsClass,
    W::Data: crate::window::WindowHost,
{
    macro_rules! interface {
        ($($name:literal),+) => {$(
            def.global_value_accessor(
                $name,
                |s, w| {
                    let page = crate::window::WindowHost::page(w);
                    if !crate::secure_context::secure(s, page) {
                        return Ok(s.undefined());
                    }
                    MODULE.global(s, page, $name)
                },
                |s, w, value| crate::lazy_module::replace(s, crate::window::WindowHost::page(w), $name, value),
            );
        )+};
    }
    interface!(
        "Credential",
        "CredentialsContainer",
        "PasswordCredential",
        "FederatedCredential",
        "PublicKeyCredential",
        "AuthenticatorResponse",
        "AuthenticatorAttestationResponse",
        "AuthenticatorAssertionResponse"
    );
}

/// `navigator.credentials`: the realm's `CredentialsContainer` in a secure context.
pub(crate) fn credentials<'s>(s: &Scope<'s>, page: &Rc<PageDom>) -> JsResult<Value<'s>> {
    if page.is_disposed() {
        return Err(JsError::ContextDestroyed);
    }
    if !crate::secure_context::secure(s, page) {
        return Ok(s.undefined());
    }
    MODULE.module(s, page)?.get("credentials")
}

/// A document's origin for a policy decision: the URL's, `None` when opaque.
fn document_origin(page: &PageDom) -> Option<String> {
    if page.has_opaque_origin() {
        return None;
    }
    let url = page.dom.borrow().document_data(page.document()).map(|d| d.url.to_string()).ok()?;
    let origin = url::Url::parse(&url).ok()?.origin();
    origin.is_tuple().then(|| origin.ascii_serialization())
}

/// Permissions Policy for `feature` in this document: every container up the frame tree admits its
/// child, by the container's `allow` attribute (`'src'`, `'self'`, `*`, origins, `'none'`) or, when
/// it does not mention the feature, the feature's default allowlist (`*`, or `'self'` when
/// `default_self`: a child at its parent's origin only).
fn policy_allows(page: &Rc<PageDom>, own_origin: Option<&str>, feature: &str, default_self: bool) -> bool {
    let mut current = page.clone();
    let mut child_origin = own_origin.map(str::to_owned);
    // (Bounded: a frame tree is never this deep.)
    for _ in 0..64 {
        let FrameContainer::Frame(parent, owner) = current.container() else { return true };
        let parent_origin = document_origin(&parent);
        let (allow, declared) = {
            let dom = parent.dom.borrow();
            let attr = |name: &str| dom.get_attribute(owner, name).ok().flatten().map(str::to_owned);
            let sandboxed = attr("sandbox").is_some_and(|v| !v.split_ascii_whitespace().any(|t| t.eq_ignore_ascii_case("allow-same-origin")));
            let declared = if sandboxed {
                None
            } else if attr("srcdoc").is_some() {
                parent_origin.clone()
            } else {
                match attr("src").filter(|v| !v.trim().is_empty()) {
                    Some(src) => url::Url::parse(&crate::html::resolve_url(&dom, owner, &src)).ok().and_then(|u| {
                        let origin = u.origin();
                        origin.is_tuple().then(|| origin.ascii_serialization())
                    }),
                    None => parent_origin.clone(),
                }
            };
            (attr("allow"), declared)
        };
        let same = child_origin.is_some() && child_origin == parent_origin;
        let admitted = crate::fullscreen::container_allows(feature, allow.as_deref(), parent_origin.as_deref(), declared.as_deref(), child_origin.as_deref())
            .unwrap_or(!default_self || same);
        if !admitted {
            return false;
        }
        child_origin = parent_origin;
        current = parent;
    }
    false
}

/// Whether the document and every ancestor are same-origin with it (the WebAuthn `crossOrigin`
/// flag of the client data is the opposite).
fn same_origin_with_ancestors(page: &Rc<PageDom>, own_origin: Option<&str>) -> bool {
    let Some(own) = own_origin else { return false };
    let mut current = page.clone();
    for _ in 0..64 {
        let FrameContainer::Frame(parent, _) = current.container() else { return true };
        if document_origin(&parent).as_deref() != Some(own) {
            return false;
        }
        current = parent;
    }
    false
}

/// The facts about the calling document the client steps need, as JSON:
/// `origin` (tuple origin or null), `host` (its host, null for an opaque origin or an IP address),
/// `same` (same-origin with ancestors), `policyGet` / `policyCreate`
/// (`publickey-credentials-get` / `-create`: both default to `'self'`, checked in headless Chrome:
/// a cross-origin frame without `allow` gets neither), `activation` (transient user activation).
fn frame_info(s: &Scope<'_>, page: &Rc<PageDom>) -> serde_json::Value {
    let origin = crate::storage::tuple_origin(s, page);
    let host = origin.as_deref().and_then(|o| url::Url::parse(o).ok()).and_then(|u| match u.host() {
        Some(url::Host::Domain(d)) => Some(d.to_owned()),
        _ => None,
    });
    serde_json::json!({
        "origin": origin,
        "host": host,
        "same": same_origin_with_ancestors(page, origin.as_deref()),
        "policyGet": policy_allows(page, origin.as_deref(), "publickey-credentials-get", true),
        "policyCreate": policy_allows(page, origin.as_deref(), "publickey-credentials-create", true),
        "activation": page.has_transient_activation(),
    })
}

/// The RP ID a request names (`candidate`, or the origin's effective domain when absent) if the
/// origin may use it ("is a registrable domain suffix of or is equal to the origin's effective
/// domain"): a domain without a port, not a public suffix unless it is the host itself.
fn valid_rp_id(host: &str, candidate: &str) -> Option<String> {
    // The candidate is compared with the origin's host as the URL parser holds it (IDNA ToASCII, so
    // `bücher.example` is `xn--bcher-kva.example`); an IP address, a forbidden host code point or a
    // port is no domain.
    let Ok(url::Host::Domain(candidate)) = url::Host::parse(candidate) else { return None };
    let host = host.to_ascii_lowercase();
    let label_ok = |label: &str| !label.is_empty() && !label.starts_with('-') && !label.ends_with('-') && label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if candidate.is_empty() || !candidate.split('.').all(label_ok) {
        return None;
    }
    if candidate == host {
        return Some(candidate);
    }
    let suffix = host.ends_with(&format!(".{candidate}"));
    // A public suffix ("com", "co.uk") is never an RP ID for a host below it.
    let public_suffix = pyro_psl::suffix_str(&candidate).is_some_and(|s| s == candidate);
    (suffix && !public_suffix).then_some(candidate)
}

fn natives<'s>(s: &Scope<'s>) -> JsResult<Object<'s>> {
    let native = s.object();
    // active() -> whether the realm's document is fully active.
    let active = s.function("active", |s, _, _| Ok(s.boolean(crate::target::page_of(s)?.is_fully_active())))?;
    native.set_atom("active", active.value())?;
    // frame() -> JSON of `frame_info`.
    let frame = s.function("frame", |s, _, _| {
        let page = crate::target::page_of(s)?;
        Ok(s.string(&frame_info(s, &page).to_string()))
    })?;
    native.set_atom("frame", frame.value())?;
    // rpId(candidate) -> the valid RP ID, or null.
    let rp_id = s.function("rpId", |s, _, a| {
        let page = crate::target::page_of(s)?;
        let host = crate::storage::tuple_origin(s, &page)
            .and_then(|o| url::Url::parse(&o).ok())
            .and_then(|u| match u.host() {
                Some(url::Host::Domain(d)) => Some(d.to_owned()),
                _ => None,
            });
        let candidate = a.get(0).to_string()?;
        Ok(match host.and_then(|h| valid_rp_id(&h, &candidate)) {
            Some(id) => s.string(&id),
            None => s.null(),
        })
    })?;
    native.set_atom("rpId", rp_id.value())?;
    // consumeActivation(): HTML "consume user activation".
    let consume = s.function("consumeActivation", |s, _, _| {
        crate::target::page_of(s)?.consume_transient_activation();
        Ok(s.undefined())
    })?;
    native.set_atom("consumeActivation", consume.value())?;
    // authenticate(requestJson) -> responseJSON: one authenticator operation (create or get).
    let authenticate = s.function("authenticate", |s, _, a| {
        let page = crate::target::page_of(s)?;
        let request: serde_json::Value = serde_json::from_str(&a.get(0).to_string()?).map_err(|e| JsError::Type(e.to_string().into()))?;
        Ok(s.string(&crate::webauthn::authenticate(&page, &request).to_string()))
    })?;
    native.set_atom("authenticate", authenticate.value())?;
    // platform() -> JSON {uvpaa, conditional}: what the context's authenticators offer a client.
    let platform = s.function("platform", |s, _, _| {
        let page = crate::target::page_of(s)?;
        Ok(s.string(&crate::webauthn::platform(&page).to_string()))
    })?;
    native.set_atom("platform", platform.value())?;
    // signal(requestJson) -> responseJSON: the Signal API calls on the context's authenticators.
    let signal = s.function("signal", |s, _, a| {
        let page = crate::target::page_of(s)?;
        let request: serde_json::Value = serde_json::from_str(&a.get(0).to_string()?).map_err(|e| JsError::Type(e.to_string().into()))?;
        Ok(s.string(&crate::webauthn::signal(&page, &request).to_string()))
    })?;
    native.set_atom("signal", signal.value())?;
    Ok(native)
}

#[cfg(test)]
mod tests {
    use super::valid_rp_id;

    #[test]
    fn rp_id_is_the_host_or_a_registrable_suffix_of_it() {
        assert_eq!(valid_rp_id("www.example.com", "example.com").as_deref(), Some("example.com"));
        assert_eq!(valid_rp_id("example.com", "example.com").as_deref(), Some("example.com"));
        assert_eq!(valid_rp_id("web-platform.test", "web-platform.test").as_deref(), Some("web-platform.test"));
        assert_eq!(valid_rp_id("localhost", "localhost").as_deref(), Some("localhost"));
        // A public suffix, another site, a port, a space, a leading dash or empty are not.
        assert_eq!(valid_rp_id("www.example.co.uk", "co.uk"), None);
        assert_eq!(valid_rp_id("www.example.com", "com"), None);
        assert_eq!(valid_rp_id("example.com", "other.com"), None);
        assert_eq!(valid_rp_id("example.com", "example.com:443"), None);
        assert_eq!(valid_rp_id("example.com", "invalid domain.com"), None);
        assert_eq!(valid_rp_id("example.com", "-example.com"), None);
        assert_eq!(valid_rp_id("example.com", ""), None);
        // IDNA: a Unicode candidate names its punycode host (and a domain below it).
        assert_eq!(valid_rp_id("xn--bcher-kva.example", "b\u{fc}cher.example").as_deref(), Some("xn--bcher-kva.example"));
        assert_eq!(valid_rp_id("www.xn--bcher-kva.example", "B\u{dc}CHER.example").as_deref(), Some("xn--bcher-kva.example"));
        assert_eq!(valid_rp_id("example.com", "127.0.0.1"), None);
        assert_eq!(valid_rp_id("example.com", "null"), None);
    }
}
