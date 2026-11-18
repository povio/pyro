//! Nonce hiding (HTML "the nonce attribute"): when an element becomes
//! connected to a document that was delivered with a Content-Security-Policy
//! header, its `nonce` content attribute is emptied and the value stays in
//! the element's [[CryptographicNonce]] slot, so a selector or
//! `getAttribute` cannot read it. `moveBefore` connects nothing and leaves
//! the attribute alone.

use rustc_hash::{FxHashMap, FxHashSet};

use crate::dom::Dom;
use crate::error::DomResult;
use crate::node::NodeId;

#[derive(Default)]
pub(crate) struct NonceState {
    /// Documents whose response carried a Content-Security-Policy header.
    csp_documents: FxHashSet<NodeId>,
    /// The nonce of each element whose attribute was emptied.
    hidden: FxHashMap<NodeId, Box<str>>,
    /// The Trusted Types directives (`trusted-types`, `require-trusted-types-for`) of each
    /// header-delivered policy of a document, one string per policy.
    trusted_types: FxHashMap<NodeId, Vec<Box<str>>>,
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

    /// Records the Trusted Types directives of `doc`'s header-delivered policies (`csp`: the
    /// `Content-Security-Policy` header values joined by `,`). No other directive is kept.
    pub fn set_trusted_types_policies(&mut self, doc: NodeId, csp: &str) -> DomResult<()> {
        self.check(doc)?;
        self.nonces.trusted_types.remove(&doc);
        self.add_csp_policies(doc, csp);
        Ok(())
    }

    /// Adds the Trusted Types directives of the policies in `csp` to `doc`'s (a header, or a
    /// `<meta http-equiv="Content-Security-Policy">` that joined the head).
    pub(crate) fn add_csp_policies(&mut self, doc: NodeId, csp: &str) {
        let policies: Vec<Box<str>> = csp
            .split(',')
            .filter_map(|policy| {
                let kept: Vec<&str> = policy
                    .split(';')
                    .map(str::trim)
                    .filter(|d| {
                        let name = d.split_ascii_whitespace().next().unwrap_or("");
                        name.eq_ignore_ascii_case("trusted-types") || name.eq_ignore_ascii_case("require-trusted-types-for")
                    })
                    .collect();
                (!kept.is_empty()).then(|| kept.join("; ").into())
            })
            .collect();
        if !policies.is_empty() {
            self.nonces.trusted_types.entry(doc).or_default().extend(policies);
        }
    }

    /// The Trusted Types directives of `doc`'s policies (headers and `<meta>`).
    pub fn trusted_types_policies(&self, doc: NodeId) -> &[Box<str>] {
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
