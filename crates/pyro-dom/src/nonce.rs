//! Nonce hiding (HTML "the nonce attribute"): when an element becomes
//! connected to a document that was delivered with a Content-Security-Policy
//! header, its `nonce` content attribute is emptied and the value stays in
//! the element's [[CryptographicNonce]] slot, so a selector or
//! `getAttribute` cannot read it. `moveBefore` connects nothing and leaves
//! the attribute alone.

use rustc_hash::{FxHashMap, FxHashSet};

use crate::dom::Dom;
use crate::error::DomResult;
use crate::node::{ElementData, NodeId};

/// CSP3 "Is element nonceable?" (Chrome's `ContentSecurityPolicy::IsNonceableElement`): an
/// element with an attribute whose name or value contains `<script`, `<style` or (Chrome's
/// addition) `<link`, ASCII case-insensitively, is not nonceable: markup injected into an
/// unclosed attribute (`<p title="<script nonce=...`) must not inherit a page's nonce. Its
/// nonce then matches no nonce-source. The spec's duplicate-attribute condition is not
/// checked: the parser does not record dropped duplicates.
pub(crate) fn is_nonceable(e: &ElementData) -> bool {
    const MARKERS: [&[u8]; 3] = [b"<script", b"<style", b"<link"];
    let suspect = |text: &str| {
        let text = text.as_bytes();
        MARKERS.iter().any(|m| text.windows(m.len()).any(|w| w.eq_ignore_ascii_case(m)))
    };
    !e.attrs.iter().any(|a| suspect(&a.name.local) || suspect(&a.value))
}

/// A Content Security Policy of a document (as delivered: its serialization), whether it is
/// report-only (`Content-Security-Policy-Report-Only`), and whether a `<meta http-equiv>`
/// delivered it (else a header).
#[derive(Clone, Debug)]
pub struct CspPolicy {
    pub text: Box<str>,
    pub report_only: bool,
    pub meta: bool,
}

#[derive(Default)]
pub(crate) struct NonceState {
    /// Documents whose response carried a Content-Security-Policy header.
    csp_documents: FxHashSet<NodeId>,
    /// The nonce of each element whose attribute was emptied.
    hidden: FxHashMap<NodeId, Box<str>>,
    /// The policies of a document that have a Trusted Types directive (`trusted-types`,
    /// `require-trusted-types-for`), with whether each is report-only. No other directive of
    /// them is enforced.
    trusted_types: FxHashMap<NodeId, Vec<CspPolicy>>,
    /// Every policy of a document (headers, then `<meta>` in insertion order): its CSP list,
    /// which the fetch, inline and eval checks read (`pyro_net::csp`).
    policies: FxHashMap<NodeId, Vec<CspPolicy>>,
    /// Documents of a page that bypasses CSP (`Page.setBypassCSP`): a `<meta>` adds nothing.
    bypassed: FxHashSet<NodeId>,
}

impl Dom {
    /// Records that `doc` has a header-delivered Content Security Policy.
    pub fn set_header_csp(&mut self, doc: NodeId, present: bool) -> DomResult<()> {
        self.check(doc)?;
        if present {
            self.nonces.csp_documents.insert(doc);
        } else {
            self.nonces.csp_documents.remove(&doc);
        }
        Ok(())
    }

    /// Records `doc`'s header-delivered policies: `enforce`, the `Content-Security-Policy`
    /// header values joined by `,`, and `report_only`, the `Content-Security-Policy-Report-Only`
    /// ones. All of them become the document's CSP list ([`Dom::csp_policies`]); those with
    /// Trusted Types directives are also [`Dom::trusted_types_policies`].
    pub fn set_trusted_types_policies(&mut self, doc: NodeId, enforce: &str, report_only: &str) -> DomResult<()> {
        self.check(doc)?;
        self.nonces.trusted_types.remove(&doc);
        self.nonces.policies.remove(&doc);
        self.add_csp_policies(doc, enforce, false, false);
        self.add_csp_policies(doc, report_only, true, false);
        Ok(())
    }

    /// Makes `doc` ignore its policies (`Page.setBypassCSP`): the ones recorded go, and a
    /// `<meta>` that joins its head later adds none.
    pub fn set_csp_bypassed(&mut self, doc: NodeId, bypassed: bool) -> DomResult<()> {
        self.check(doc)?;
        if bypassed {
            self.nonces.bypassed.insert(doc);
            self.nonces.policies.remove(&doc);
            self.nonces.trusted_types.remove(&doc);
        } else {
            self.nonces.bypassed.remove(&doc);
        }
        Ok(())
    }

    /// `doc`'s CSP list as delivered: header policies, then `<meta>` ones in the order they
    /// joined the head.
    pub fn csp_policies(&self, doc: NodeId) -> &[CspPolicy] {
        self.nonces.policies.get(&doc).map_or(&[], |v| v.as_slice())
    }

    /// Adds the policies in `csp` (a comma-separated list) to `doc`'s CSP list, and those with a
    /// Trusted Types directive to its Trusted Types policies (a header, or with `meta` a
    /// `<meta http-equiv="Content-Security-Policy">` that joined the head).
    pub(crate) fn add_csp_policies(&mut self, doc: NodeId, csp: &str, report_only: bool, meta: bool) {
        if self.nonces.bypassed.contains(&doc) {
            return;
        }
        let all: Vec<CspPolicy> = csp
            .split(',')
            .map(str::trim)
            .filter(|policy| !policy.is_empty())
            .map(|policy| CspPolicy { text: policy.into(), report_only, meta })
            .collect();
        if !all.is_empty() {
            self.nonces.policies.entry(doc).or_default().extend(all);
        }
        let policies: Vec<CspPolicy> = csp
            .split(',')
            .map(str::trim)
            .filter(|policy| {
                policy.split(';').any(|d| {
                    let name = d.split_ascii_whitespace().next().unwrap_or("");
                    name.eq_ignore_ascii_case("trusted-types") || name.eq_ignore_ascii_case("require-trusted-types-for")
                })
            })
            .map(|policy| CspPolicy { text: policy.into(), report_only, meta })
            .collect();
        if !policies.is_empty() {
            self.nonces.trusted_types.entry(doc).or_default().extend(policies);
        }
    }

    /// `doc`'s policies that have Trusted Types directives (headers and `<meta>`).
    pub fn trusted_types_policies(&self, doc: NodeId) -> &[CspPolicy] {
        self.nonces.trusted_types.get(&doc).map_or(&[], |v| v.as_slice())
    }

    /// The nonce `el` had when its attribute was emptied on connection.
    pub fn hidden_nonce(&self, el: NodeId) -> Option<&str> {
        self.nonces.hidden.get(&el).map(|v| &**v)
    }

    /// After `root` was inserted: empties the non-empty `nonce` attributes of
    /// its subtree when it is now connected to a document with a
    /// header-delivered policy.
    pub(crate) fn hide_nonces(&mut self, root: u32) {
        if self.nonces.csp_documents.is_empty() {
            return;
        }
        let doc = self.id_of(self.n(root).owner_doc);
        if !self.nonces.csp_documents.contains(&doc) || !self.is_connected_idx(root) {
            return;
        }
        let found: Vec<(NodeId, Box<str>)> = self
            .shadow_including_subtree(root)
            .into_iter()
            .filter_map(|i| {
                let nonce =
                    self.n(i).element()?.attr_local(&markup5ever::local_name!("nonce")).filter(|v| !v.is_empty())?;
                Some((self.id_of(i), nonce.into()))
            })
            .collect();
        if found.is_empty() {
            return;
        }
        // Entries of collected elements go when the map has grown.
        if self.nonces.hidden.len() >= 1024 {
            let stale: Vec<NodeId> =
                self.nonces.hidden.keys().filter(|id| self.check(**id).is_err()).copied().collect();
            for id in stale {
                self.nonces.hidden.remove(&id);
            }
        }
        for (el, nonce) in found {
            self.nonces.hidden.insert(el, nonce);
            let _ = self.set_attribute(el, "nonce", "");
        }
    }
}
