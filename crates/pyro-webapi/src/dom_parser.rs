//! `DOMParser` (HTML §8.5.1; upstream src/browser/webapi/DOMParser.zig) and
//! `XMLSerializer` (DOM Parsing §2).
//!
//! HTML uses html5ever; XML types use the bounded inert XML 1.0 parser.
//! Malformed XML and unsupported DTDs return a parsererror document.

use pyro_js::{ClassDef, JsClass, JsError, NoParent};

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
            if mime != "text/html" && !XML_TYPES.contains(&mime.as_str()) {
                return Err(JsError::Type(format!(
                    "Failed to execute 'parseFromString' on 'DOMParser': The provided value '{mime}' is not a valid enum value of type DOMParserSupportedType."
                ).into()));
            }
            let page = page_of(s)?;
            let url = page.dom.borrow().document_data(page.document()).map(|d| d.url.to_string()).unwrap_or_default();
            let doc = if mime == "text/html" {
                pyro_dom::parse::parse_domparser_document(&page.dom, &url, &html)
            } else {
                pyro_dom::xml::parse_document(&mut page.dom.borrow_mut(), &url, &html, &mime)
            };
            Ok(wrap_node(s, &page, doc)?.value())
        });
    }
}

pub struct XmlSerializer;
impl JsClass for XmlSerializer {
    const NAME: &'static str = "XMLSerializer";
    type Data = ();
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(0, |s, nt, _a| s.wrap_new::<XmlSerializer>((), nt, None));
        def.method("serializeToString", 1, |s, _d, a| {
            let node = crate::node::node_arg(s, a.get(0), "XMLSerializer.serializeToString")?;
            let page = page_of(s)?;
            let xml = page.dom.borrow().xml_serialize(node).js()?;
            Ok(s.string(&xml))
        });
    }
}

pub(crate) fn register(engine: &pyro_js::Engine) {
    engine.register::<DomParser>();
    engine.register::<XmlSerializer>();
}
