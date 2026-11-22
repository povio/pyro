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
    /// Elements of a document with policies whose `style` attribute the parser or a script set,
    /// or that joined such a document with one: their `style-src-attr` check waits for the
    /// embedder ([`Dom::take_pending_style_checks`]). CSSOM writes and clones record none.
    pending_style_attrs: Vec<NodeId>,
    /// HTML `<style>` elements of a document with policies whose style block was updated
    /// (connected, text changed): their `style-src-elem` check waits likewise.
    pending_style_elements: Vec<NodeId>,
    /// The `style` attribute text CSP blocked on an element: while the attribute still holds it,
    /// it declares nothing (as Chrome, the element's inline style stays empty).
    blocked_style_attrs: FxHashMap<NodeId, Box<str>>,
    /// Each checked `<style>`: the style sheet generation its check ran at, and whether CSP
    /// blocked it (it then has no sheet).
    style_element_checks: FxHashMap<NodeId, (u32, bool)>,
    /// A CSSOM write of an inline style is under way: it records no check.
    cssom_style_write: bool,
}

/// The CSP checks of inline styles waiting in a document ([`Dom::take_pending_style_checks`]).
#[derive(Default, Debug)]
pub struct PendingStyleChecks {
    /// Elements whose `style` attribute is to be checked (deduplicated, in order).
    pub attributes: Vec<NodeId>,
    /// HTML `<style>` elements whose sheet is to be checked (deduplicated, in order).
    pub elements: Vec<NodeId>,
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

    /// Gives `doc` the policies of the document it inherits from (a `data:` or `blob:`
    /// worker's owner), each with its delivery and disposition, replacing what it had.
    pub fn inherit_csp_policies(&mut self, doc: NodeId, policies: &[CspPolicy]) -> DomResult<()> {
        self.check(doc)?;
        self.nonces.trusted_types.remove(&doc);
        self.nonces.policies.remove(&doc);
        for policy in policies {
            self.add_csp_policies(doc, &policy.text, policy.report_only, policy.meta);
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

    /// Whether node `i`'s document has a CSP list (one map lookup; none without any policy).
    #[inline]
    fn in_csp_document(&self, i: u32) -> bool {
        !self.nonces.policies.is_empty() && self.nonces.policies.contains_key(&self.id_of(self.n(i).owner_doc))
    }

    /// The parser or a script set element `i`'s `style` attribute, or `i` joined another
    /// document: in a document with policies, its `style-src-attr` check is recorded (not for a
    /// CSSOM write).
    pub(crate) fn note_inline_style(&mut self, i: u32) {
        if self.nonces.policies.is_empty() || self.nonces.cssom_style_write {
            return;
        }
        let has = self
            .n(i)
            .element()
            .is_some_and(|e| e.attrs.iter().any(|a| a.name.local == markup5ever::local_name!("style") && a.name.ns == markup5ever::ns!()));
        if has && self.in_csp_document(i) {
            let id = self.id_of(i);
            self.nonces.pending_style_attrs.push(id);
        }
    }

    /// The HTML `<style>` `i`'s style block was updated: in a document with policies, its
    /// `style-src-elem` check is recorded.
    pub(crate) fn note_style_element(&mut self, i: u32) {
        if self.nonces.policies.is_empty() || !self.in_csp_document(i) {
            return;
        }
        if self.n(i).element().is_some_and(|e| e.is_html() && e.name.local == markup5ever::local_name!("style")) {
            let id = self.id_of(i);
            self.nonces.pending_style_elements.push(id);
        }
    }

    /// Runs `write`, a CSSOM write of an inline style (`el.style.x = …`, `cssText`): CSP checks
    /// none (as Chrome), and the element's inline style is what it wrote from now on.
    pub(crate) fn cssom_style_write<R>(&mut self, id: NodeId, write: impl FnOnce(&mut Dom) -> R) -> R {
        let was = std::mem::replace(&mut self.nonces.cssom_style_write, true);
        let r = write(self);
        self.nonces.cssom_style_write = was;
        if !self.nonces.blocked_style_attrs.is_empty() {
            self.nonces.blocked_style_attrs.remove(&id);
        }
        r
    }

    /// Whether inline style CSP checks wait for `doc` (or for another document of this DOM).
    #[inline]
    pub fn has_pending_style_checks(&self) -> bool {
        !self.nonces.pending_style_attrs.is_empty() || !self.nonces.pending_style_elements.is_empty()
    }

    /// Takes the inline style checks that wait for elements of `doc`; those of another document
    /// with policies keep waiting, the rest (collected nodes, elements now in a document without
    /// policies) go. A `<style>` whose text the parser has not finished keeps waiting.
    pub fn take_pending_style_checks(&mut self, doc: NodeId) -> PendingStyleChecks {
        let mut out = PendingStyleChecks::default();
        if !self.has_pending_style_checks() {
            return out;
        }
        let attrs = std::mem::take(&mut self.nonces.pending_style_attrs);
        let elements = std::mem::take(&mut self.nonces.pending_style_elements);
        let mut seen: FxHashSet<(NodeId, bool)> = FxHashSet::default();
        for (list, is_element) in [(attrs, false), (elements, true)] {
            for id in list {
                let Ok(i) = self.check(id) else { continue };
                let owner = self.id_of(self.n(i).owner_doc);
                if owner != doc {
                    if self.nonces.policies.contains_key(&owner) {
                        if is_element {
                            self.nonces.pending_style_elements.push(id);
                        } else {
                            self.nonces.pending_style_attrs.push(id);
                        }
                    }
                    continue;
                }
                if is_element {
                    let parsing = self
                        .n(i)
                        .element()
                        .is_some_and(|e| e.flags & crate::node::element_flags::STYLE_PARSING != 0);
                    if parsing {
                        self.nonces.pending_style_elements.push(id);
                    } else if seen.insert((id, true)) {
                        out.elements.push(id);
                    }
                } else if seen.insert((id, false)) {
                    out.attributes.push(id);
                }
            }
        }
        out
    }

    /// Records the outcome of `el`'s `style-src-attr` check: `blocked`, the attribute text CSP
    /// blocked (`None`: allowed). A change restyles the element.
    pub fn set_style_attribute_blocked(&mut self, el: NodeId, blocked: Option<&str>) {
        let Ok(i) = self.check(el) else { return };
        let changed = match blocked {
            Some(text) => self.nonces.blocked_style_attrs.insert(el, text.into()).is_none_or(|old| *old != *text),
            None => self.nonces.blocked_style_attrs.remove(&el).is_some(),
        };
        if self.nonces.blocked_style_attrs.len() >= 1024 {
            let stale: Vec<NodeId> =
                self.nonces.blocked_style_attrs.keys().filter(|id| self.check(**id).is_err()).copied().collect();
            for id in stale {
                self.nonces.blocked_style_attrs.remove(&id);
            }
        }
        if changed {
            let name = markup5ever::QualName::new(None, markup5ever::ns!(), markup5ever::local_name!("style"));
            self.bump();
            self.note(crate::dom::Change::attr(i, &name));
        }
    }

    /// A clone `to` of element `from` (same attributes): CSP blocked its `style` attribute too
    /// when it blocked `from`'s.
    pub(crate) fn copy_style_attribute_block(&mut self, from: u32, to: NodeId) {
        if self.nonces.blocked_style_attrs.is_empty() {
            return;
        }
        if let Some(text) = self.nonces.blocked_style_attrs.get(&self.id_of(from)).cloned() {
            self.nonces.blocked_style_attrs.insert(to, text);
        }
    }

    /// Whether CSP blocked the `style` attribute element `i` holds now ([`Dom::style_attribute_blocked`]).
    #[inline]
    pub(crate) fn style_attribute_blocked_idx(&self, i: u32) -> bool {
        if self.nonces.blocked_style_attrs.is_empty() {
            return false;
        }
        let Some(e) = self.n(i).element() else { return false };
        match self.nonces.blocked_style_attrs.get(&self.id_of(i)) {
            Some(text) => e.attr_local(&markup5ever::local_name!("style")).is_some_and(|v| v == &**text),
            None => false,
        }
    }

    /// Whether CSP blocked the `style` attribute `el` holds now: it declares nothing (layout,
    /// the renderer, `el.style`).
    pub fn style_attribute_blocked(&self, el: NodeId) -> bool {
        self.check(el).is_ok_and(|i| self.style_attribute_blocked_idx(i))
    }

    /// The style sheet generation `el`'s last `style-src-elem` check ran at, and whether it
    /// blocked the sheet.
    pub fn style_element_check(&self, el: NodeId) -> Option<(u32, bool)> {
        self.nonces.style_element_checks.get(&el).copied()
    }

    /// Records `el`'s `style-src-elem` check at its current style sheet generation. A change of
    /// the outcome changes its sheet (it is read again).
    pub fn set_style_element_blocked(&mut self, el: NodeId, blocked: bool) {
        let Ok(i) = self.check(el) else { return };
        let generation = self.style_sheet_generation(el);
        let before = self.nonces.style_element_checks.insert(el, (generation, blocked)).is_some_and(|(_, b)| b);
        if self.nonces.style_element_checks.len() >= 1024 {
            let stale: Vec<NodeId> =
                self.nonces.style_element_checks.keys().filter(|id| self.check(**id).is_err()).copied().collect();
            for id in stale {
                self.nonces.style_element_checks.remove(&id);
            }
        }
        if before != blocked {
            self.style_csp_changed(i);
        }
    }

    /// Whether CSP blocked the `<style>` `owner`'s sheet: it has none (no rules,
    /// not in `document.styleSheets`).
    #[inline]
    pub fn style_element_blocked(&self, owner: NodeId) -> bool {
        !self.nonces.style_element_checks.is_empty()
            && self.nonces.style_element_checks.get(&owner).is_some_and(|&(_, blocked)| blocked)
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

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use crate::dom::Dom;
    use crate::node::DocumentKind;

    fn provenance(dom: &Dom, doc: crate::node::NodeId) -> Vec<(String, bool, bool)> {
        dom.trusted_types_policies(doc).iter().map(|p| (p.text.to_string(), p.report_only, p.meta)).collect()
    }

    #[test]
    fn header_policies_are_not_meta() {
        let mut dom = Dom::new();
        let doc = dom.create_document(DocumentKind::Html, "https://a.test/");
        dom.set_trusted_types_policies(doc, "trusted-types a; report-uri /h", "require-trusted-types-for 'script'").unwrap();
        dom.add_csp_policies(doc, "trusted-types b", false, true);
        assert_eq!(
            provenance(&dom, doc),
            [
                ("trusted-types a; report-uri /h".to_owned(), false, false),
                ("require-trusted-types-for 'script'".to_owned(), true, false),
                ("trusted-types b".to_owned(), false, true),
            ]
        );
    }

    #[test]
    fn a_meta_in_head_is_meta() {
        let dom: crate::parse::DomRef = Rc::new(RefCell::new(Dom::new()));
        let doc = crate::parse::parse_document(
            &dom,
            "https://a.test/",
            r#"<head><meta http-equiv="Content-Security-Policy" content="trusted-types m; report-uri /r"></head>"#,
        );
        assert_eq!(provenance(&dom.borrow(), doc), [("trusted-types m; report-uri /r".to_owned(), false, true)]);
    }
}
