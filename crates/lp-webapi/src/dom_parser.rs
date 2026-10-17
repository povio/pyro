//! `DOMParser` (HTML §8.5.1; upstream src/browser/webapi/DOMParser.zig).
//!
//! `parseFromString(string, "text/html")` parses a complete HTML document with
//! scripting disabled; its URL is the parser's window document URL. The XML
//! types (`text/xml`, `application/xml`, `application/xhtml+xml`,
//! `image/svg+xml`) throw NotSupportedError because there is no XML parser yet
//! (documented gap; upstream parses them). Other types throw TypeError, as for
//! a Web IDL enum mismatch.

use lp_js::{ClassDef, JsClass, JsError, NoParent};

use crate::node::wrap_node;
use crate::target::page_of;
use crate::util::*;

const XML_TYPES: [&str; 4] = ["text/xml", "application/xml", "application/xhtml+xml", "image/svg+xml"];

pub struct DomParser;
impl JsClass for DomParser {
    const NAME: &'static str = "DOMParser";
    type Data = ();
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(0, |s, nt, _a| s.wrap_new::<DomParser>((), nt, None));
        def.method("parseFromString", 2, |s, _d, a| {
            a.require(2, "DOMParser.parseFromString")?;
            let html = dom_string(a.get(0))?;
            let mime = dom_string(a.get(1))?;
            if XML_TYPES.contains(&mime.as_str()) {
                return Err(JsError::dom(
                    "NotSupportedError",
                    format!("DOMParser does not support '{mime}' in lightningpanda yet (no XML parser)"),
                ));
            }
            if mime != "text/html" {
                return Err(JsError::Type(format!(
                    "Failed to execute 'parseFromString' on 'DOMParser': The provided value '{mime}' is not a valid enum value of type DOMParserSupportedType."
                )));
            }
            let page = page_of(s)?;
            let url = page.dom.borrow().document_data(page.document()).map(|d| d.url.to_string()).unwrap_or_default();
            let doc = lp_dom::parse::parse_document(&page.dom, &url, &html);
            Ok(wrap_node(s, &page, doc)?.value())
        });
    }
}

pub(crate) fn register(engine: &lp_js::Engine) {
    engine.register::<DomParser>();
}
