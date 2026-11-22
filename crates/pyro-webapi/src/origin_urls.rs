//! Extract the parsed URL of genuine URL and hyperlink objects. An author's
//! href getter or stringifier must not run during Origin.from().
use crate::{html, page::PageDom, target::page_of, url::URL};
use pyro_js::{Scope, Value};
use url::Url;

/// A live blob URL entry carries its creator's origin, including opaque
/// identity. Fragments are not part of the blob URL store key.
pub(crate) fn blob_origin(s: &Scope<'_>, url: &Url) -> Option<String> {
    if url.scheme() != "blob" {
        return None;
    }
    let page = page_of(s).ok()?;
    page_blob_origin(&page, url)
}

fn page_blob_origin(page: &PageDom, url: &Url) -> Option<String> {
    if url.scheme() != "blob" {
        return None;
    }
    let key = &url[..url::Position::AfterQuery];
    page.blob_urls.borrow().contains_key(key).then(|| page.origin_key())
}

pub(crate) fn object_url(s: &Scope<'_>, value: Value<'_>) -> Option<(Url, Option<String>)> {
    if let Some(data) = s.unwrap::<URL>(value) {
        return data.origin_url();
    }
    let (node, svg) = if let Some(node) = s.unwrap::<html::HtmlAnchorElement>(value) {
        (node, false)
    } else if let Some(node) = s.unwrap::<html::HtmlAreaElement>(value) {
        (node, false)
    } else {
        (s.unwrap::<html::SvgAElement>(value)?, true)
    };
    let url = {
        let dom = node.page().dom.borrow();
        html::hyperlink_url(&dom, &node, svg).ok()??
    };
    let origin = page_blob_origin(node.page(), &url);
    Some((url, origin))
}
