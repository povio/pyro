//! Extract the parsed URL of genuine URL and hyperlink objects. An author's
//! href getter or stringifier must not run during Origin.from().
use crate::{html, url::URL};
use pyro_js::{Scope, Value};
use url::Url;

pub(crate) fn object_url(s: &Scope<'_>, value: Value<'_>) -> Option<(Url, Option<String>)> {
    if let Some(data) = s.unwrap::<URL>(value) {
        return data.origin_url().map(|url| (url, None));
    }
    let (node, svg) = if let Some(node) = s.unwrap::<html::HtmlAnchorElement>(value) {
        (node, false)
    } else if let Some(node) = s.unwrap::<html::HtmlAreaElement>(value) {
        (node, false)
    } else {
        (s.unwrap::<html::SvgAElement>(value)?, true)
    };
    let dom = node.page().dom.borrow();
    if let Some((url, origin)) = dom.hyperlink_blob_record(node.id) {
        return Some((url, Some(origin)));
    }
    html::hyperlink_url(&dom, &node, svg).ok()?.map(|url| (url, None))
}
