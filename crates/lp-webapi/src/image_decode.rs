//! `HTMLImageElement.decode()` (HTML §4.8.4.3): a promise per call that
//! fulfills once the image's current request is completely available and
//! rejects with `EncodingError` when it is broken, when the element's
//! selected source changes before it completes, or when the element has no
//! source or its document is not fully active.
//!
//! Pixels are never decoded: "decodable" is what the embedder reports when a
//! load finishes (`PageDom::image_loaded`), a recognized header or an
//! `image/*` format without a header parser. Promises wait in
//! `PageDom::image_decodes` only while a call is pending, so images whose
//! `decode()` is never called cost nothing here.
use std::rc::Rc;

use lp_dom::NodeId;
use lp_js::{Deferred, JsError, JsResult, Scope, Value, WeakRealm};

use crate::page::PageDom;

pub(crate) struct PendingDecode {
    realm: WeakRealm,
    deferred: Deferred,
    /// The selected source when `decode()` was called.
    source: String,
}

/// The source `<img>` loads: a `<picture>`'s matching `<source>` or its own
/// `srcset`, else `src` (what the loader selects).
fn selected_source(page: &PageDom, el: NodeId) -> Option<String> {
    let viewport = page.viewport();
    let dom = page.dom.borrow();
    dom.picture_source(el, &viewport)
        .or_else(|| dom.get_attribute(el, "src").ok().flatten().map(str::to_owned))
        .filter(|s| !s.trim().is_empty())
}

fn encoding_error<'s>(s: &Scope<'s>) -> Value<'s> {
    JsError::dom("EncodingError", "The source image cannot be decoded.").to_value(s)
}

pub(crate) fn decode<'s>(s: &Scope<'s>, page: &Rc<PageDom>, el: NodeId) -> JsResult<Value<'s>> {
    let deferred = s.deferred()?;
    let promise = deferred.promise.get(s);
    let active = page.is_fully_active() && page.dom.borrow().node_document(el).ok() == Some(page.document());
    let Some(source) = selected_source(page, el).filter(|_| active) else {
        deferred.reject(s, encoding_error(s))?;
        return Ok(promise);
    };
    // The current request already finished for this `src`.
    let src = page.dom.borrow().get_attribute(el, "src").ok().flatten().unwrap_or("").to_owned();
    let done = page.images.borrow().get(&el).filter(|(loading, pending, _)| !pending && *loading == src).map(|e| e.2);
    match done {
        Some(true) => deferred.resolve(s, s.undefined())?,
        Some(false) => deferred.reject(s, encoding_error(s))?,
        None => {
            let pending = PendingDecode { realm: s.realm().downgrade(), deferred, source };
            page.image_decodes.borrow_mut().entry(el).or_default().push(pending);
        }
    }
    Ok(promise)
}

fn settle(page: &PageDom, el: NodeId, outcome: impl Fn(&PendingDecode) -> bool) {
    let Some(pending) = page.image_decodes.borrow_mut().remove(&el) else { return };
    for p in pending {
        let Some(realm) = p.realm.upgrade().filter(|r| !r.is_disposed()) else { continue };
        let Ok(s) = realm.scope() else { continue };
        let _ =
            if outcome(&p) { p.deferred.resolve(&s, s.undefined()) } else { p.deferred.reject(&s, encoding_error(&s)) };
    }
}

/// The element lost its source: every pending `decode()` rejects.
pub(crate) fn reject_all(page: &PageDom, el: NodeId) {
    if page.image_decodes.borrow().contains_key(&el) {
        settle(page, el, |_| false);
    }
}

impl PageDom {
    /// Embedder: an `<img>` that is not in the document started (`keep`) or
    /// finished a request. Its wrappers, which hold its listeners, stay
    /// alive meanwhile, so `new Image()` that script dropped still fires
    /// `load`. A connected element's wrappers are kept by the connection.
    pub fn keep_loading_image(&self, el: NodeId, keep: bool) {
        if self.dom.borrow().is_in_active_document(el) {
            return;
        }
        if keep {
            self.keep_node(el.index());
        } else {
            self.unkeep_node(None, el.index());
        }
    }

    /// Embedder: the `<img>`'s current request (for the `src` value `src`)
    /// finished; `decodable` when it is an image (else it is broken).
    /// Pending `decode()` calls for the current source settle.
    pub fn image_loaded(&self, el: NodeId, src: &str, decodable: bool) {
        self.images.borrow_mut().insert(el, (src.to_owned(), false, decodable));
        self.settle_image_decodes(el);
    }

    /// Settles pending `decode()` calls with the finished request's result:
    /// those made for the current source fulfill when it is decodable, the
    /// others (made for a source since replaced) reject.
    pub fn settle_image_decodes(&self, el: NodeId) {
        if !self.image_decodes.borrow().contains_key(&el) {
            return;
        }
        let decodable = self.images.borrow().get(&el).is_some_and(|(_, pending, ok)| !pending && *ok);
        let current = selected_source(self, el);
        settle(self, el, |p| decodable && current.as_deref() == Some(p.source.as_str()));
    }
}
