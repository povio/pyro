//! A hyperlink's fully parsed URL retains its blob entry even after revoke.
//! Only entries whose origin cannot be recovered from the serialized URL
//! need this metadata. It stores no file bytes and grants no fetch access.

use crate::{Dom, NodeId, node::ElementData};
use markup5ever::{local_name, ns};
use rustc_hash::FxHashMap;
use std::cell::RefCell;
use std::rc::{Rc, Weak};
use url::{Position, Url};

type Registered = RefCell<FxHashMap<String, Rc<str>>>;

#[derive(Default)]
pub(crate) struct BlobHyperlinks {
    active: Rc<Registered>,
    records: FxHashMap<u32, (Url, Rc<str>)>,
}

/// Owned by the real object URL entry. Revocation or document teardown
/// removes this entry from subsequent URL parses, leaving old link records
/// intact. No DOM node references are held.
pub struct BlobOriginRegistration {
    active: Weak<Registered>,
    url: String,
}

impl Drop for BlobOriginRegistration {
    fn drop(&mut self) {
        if let Some(active) = self.active.upgrade() {
            active.borrow_mut().remove(&self.url);
        }
    }
}

pub(crate) fn hyperlink_href(el: &ElementData) -> Option<&str> {
    let svg = el.name.ns == ns!(svg) && el.name.local == local_name!("a");
    if !svg && !(el.is_html() && matches!(&*el.name.local, "a" | "area")) {
        return None;
    }
    el.attr_local(&local_name!("href")).or_else(|| {
        if !svg {
            return None;
        }
        el.attrs.iter().find(|a| a.name.ns == ns!(xlink) && a.name.local == local_name!("href")).map(|a| &*a.value)
    })
}

impl Dom {
    pub fn register_blob_link_origin(&mut self, url: &str, origin: String) -> BlobOriginRegistration {
        let state = self.blob_hyperlinks.get_or_insert_with(Default::default);
        state.active.borrow_mut().insert(url.to_owned(), origin.into());
        BlobOriginRegistration { active: Rc::downgrade(&state.active), url: url.to_owned() }
    }

    pub fn hyperlink_blob_record(&self, id: NodeId) -> Option<(Url, String)> {
        let (url, origin) = self.blob_hyperlinks.as_ref()?.records.get(&self.check(id).ok()?)?;
        Some((url.clone(), origin.to_string()))
    }

    pub(crate) fn capture_hyperlink_blob(&mut self, i: u32) {
        let Some(state) = &self.blob_hyperlinks else { return };
        let record = (|| {
            let active = state.active.borrow();
            if active.is_empty() {
                return None;
            }
            let node = self.n(i);
            let href = hyperlink_href(node.element()?)?;
            let url = Url::parse(href).ok().or_else(|| {
                let base = Url::parse(self.document_base_url(self.id_of(node.owner_doc)).ok()?).ok()?;
                base.join(href).ok()
            })?;
            let origin = active.get(&url[..Position::AfterQuery])?.clone();
            Some((url, origin))
        })();
        let state = self.blob_hyperlinks.as_mut().unwrap();
        if let Some(record) = record {
            state.records.insert(i, record);
        } else {
            state.records.remove(&i);
        }
    }

    pub(crate) fn forget_hyperlink_blob(&mut self, i: u32) {
        if let Some(state) = &mut self.blob_hyperlinks {
            state.records.remove(&i);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DocumentKind;

    #[test]
    fn hyperlink_record_survives_revoke_but_new_parses_do_not_recover_it() {
        let mut dom = Dom::new();
        let doc = dom.create_document(DocumentKind::Html, "https://example.test/");
        let url = "blob:null/fixture";
        let entry = dom.register_blob_link_origin(url, "opaque:fixture".into());
        let anchor = dom.create_element(doc, "a").unwrap();
        dom.set_attribute(anchor, "href", "blob:null/fixture#part").unwrap();
        let copy = dom.clone_node(anchor, false, None).unwrap();
        drop(entry);

        for node in [anchor, copy] {
            let (record, origin) = dom.hyperlink_blob_record(node).unwrap();
            assert_eq!(record.as_str(), "blob:null/fixture#part");
            assert_eq!(origin, "opaque:fixture");
        }
        let new_copy = dom.clone_node(anchor, false, None).unwrap();
        assert!(dom.hyperlink_blob_record(new_copy).is_none());
        dom.set_attribute(anchor, "href", "blob:null/fixture#part").unwrap();
        assert!(dom.hyperlink_blob_record(anchor).is_some());
        dom.remove_attribute(anchor, "href").unwrap();
        dom.set_attribute(anchor, "href", url).unwrap();
        assert!(dom.hyperlink_blob_record(anchor).is_none());
    }

    #[test]
    fn svg_href_takes_precedence_over_xlink_and_other_namespaces() {
        let mut dom = Dom::new();
        let doc = dom.create_document(DocumentKind::Html, "https://example.test/");
        let url = "blob:null/fixture";
        let _entry = dom.register_blob_link_origin(url, "opaque:fixture".into());
        let svg = dom.create_element_ns(doc, Some("http://www.w3.org/2000/svg"), "a").unwrap();
        dom.set_attribute_ns(svg, Some("urn:unrelated"), "test:href", url).unwrap();
        assert!(dom.hyperlink_blob_record(svg).is_none());
        dom.set_attribute_ns(svg, Some("http://www.w3.org/1999/xlink"), "xlink:href", url).unwrap();
        assert!(dom.hyperlink_blob_record(svg).is_some());
        dom.set_attribute(svg, "href", "https://example.test/").unwrap();
        assert!(dom.hyperlink_blob_record(svg).is_none());
        dom.remove_attribute(svg, "href").unwrap();
        assert!(dom.hyperlink_blob_record(svg).is_some());
    }
}
