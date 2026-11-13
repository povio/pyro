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
