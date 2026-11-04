//! Bounded, inert XML 1.0 parsing for DOMParser. No DTD or external entities.
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::{Namespace as XmlNamespace, NamespaceResolver, PrefixDeclaration, ResolveResult};
use quick_xml::{Reader, XmlVersion};
use rustc_hash::FxHashSet;

use crate::dom::validate_name;
use crate::node::{element_flags, ElementData, PiData, NIL};
use crate::{Attr, DocumentKind, Dom, NodeId, NodeKind, QualName};

pub const MAX_INPUT_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_NODES: usize = 100_000;
pub const MAX_DEPTH: usize = 256;
const MAX_ATTRIBUTES: usize = 256;
const XMLNS: &str = "http://www.w3.org/2000/xmlns/";
const PARSERERROR: &str = "http://www.mozilla.org/newlayout/xml/parsererror.xml";
type Result<T> = std::result::Result<T, &'static str>;

struct Prepared {
    parent: Option<usize>,
    kind: NodeKind,
}
fn xml_char(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r' | '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..='\u{10FFFF}')
}
fn qname(namespace: Option<&str>, qualified: &str) -> Result<QualName> {
    if qualified.len() > 1024 {
        return Err("XML name limit exceeded");
    }
    let (prefix, local) = match qualified.split_once(':') {
        Some((prefix, local)) if !local.contains(':') => {
            validate_name(prefix).map_err(|_| "Invalid XML prefix")?;
            (Some(prefix), local)
        }
        Some(_) => return Err("Invalid XML qualified name"),
        None => (None, qualified),
    };
    validate_name(local).map_err(|_| "Invalid XML local name")?;
    if prefix.is_some() && namespace.is_none() {
        return Err("Undeclared XML prefix");
    }
    Ok(QualName::new(prefix.map(Into::into), namespace.unwrap_or("").into(), local.into()))
}
fn namespace(result: ResolveResult<'_>) -> Result<Option<&str>> {
    match result {
        ResolveResult::Unbound => Ok(None),
        ResolveResult::Bound(ns) => Ok(Some(ns.into_inner())),
        ResolveResult::Unknown(_) => Err("Undeclared XML namespace prefix"),
    }
}
fn push(nodes: &mut Vec<Prepared>, parent: Option<usize>, kind: NodeKind) -> Result<usize> {
    if nodes.len() >= MAX_NODES {
        return Err("XML node limit exceeded");
    }
    let index = nodes.len();
    nodes.push(Prepared { parent, kind });
    Ok(index)
}
fn text(nodes: &mut Vec<Prepared>, parent: Option<usize>, value: &str) -> Result<()> {
    if parent.is_none() {
        return if value.chars().all(|c| matches!(c, ' ' | '\t' | '\r' | '\n')) {
            Ok(())
        } else {
            Err("Text outside XML root")
        };
    }
    if let Some(Prepared { parent: previous_parent, kind: NodeKind::Text(previous) }) = nodes.last_mut() {
        if *previous_parent == parent {
            previous.push_str(value);
            return Ok(());
        }
    }
    if !value.is_empty() {
        push(nodes, parent, NodeKind::Text(value.into()))?;
    }
    Ok(())
}
fn prepare(source: &str) -> Result<Vec<Prepared>> {
    if source.len() > MAX_INPUT_BYTES {
        return Err("XML input limit exceeded");
    }
    if !source.chars().all(xml_char) {
        return Err("Invalid XML character");
    }
    let mut reader = Reader::from_str(source);
    reader.config_mut().check_comments = true;
    let mut namespaces = NamespaceResolver::default();
    namespaces.set_max_namespace_bindings(256);
    let mut nodes = Vec::new();
    let mut stack = Vec::new();
    let mut roots = 0;
    let mut first = true;
    loop {
        let event = reader.read_event().map_err(|_| "Malformed XML")?;
        let parent = stack.last().copied();
        match event {
            Event::Start(ref start) | Event::Empty(ref start) => {
                let empty = matches!(event, Event::Empty(_));
                if stack.len() >= MAX_DEPTH {
                    return Err("XML depth limit exceeded");
                }
                if parent.is_none() {
                    roots += 1;
                    if roots > 1 {
                        return Err("Multiple XML roots");
                    }
                }
                namespaces.set_level((stack.len() + 1) as u16);
                // Decode declarations before namespace resolution (NsReader stores
                // raw declaration values). Keep all explicit xmlns attributes.
                let mut attributes = Vec::new();
                for attr in start.attributes() {
                    if attributes.len() >= MAX_ATTRIBUTES {
                        return Err("XML attribute limit exceeded");
                    }
                    let attr = attr.map_err(|_| "Invalid/duplicate XML attribute")?;
                    if attr.value.contains('<') {
                        return Err("Literal '<' in XML attribute");
                    }
                    let value = attr
                        .normalized_value(XmlVersion::Implicit1_0)
                        .map_err(|_| "Invalid XML attribute value")?
                        .into_owned();
                    if !value.chars().all(xml_char) {
                        return Err("Invalid XML character reference");
                    }
                    if let Some(prefix) = attr.key.as_namespace_binding() {
                        if matches!(prefix, PrefixDeclaration::Named(_)) && value.is_empty() {
                            return Err("XML 1.0 forbids empty prefixed namespace bindings");
                        }
                        namespaces.add(prefix, XmlNamespace(&value)).map_err(|_| "Invalid XML namespace binding")?;
                    }
                    attributes.push((attr.key, value));
                }
                let name = qname(namespace(namespaces.resolve_element(start.name()).0)?, start.name().as_ref())?;
                if name.prefix.as_ref().is_some_and(|prefix| &**prefix == "xmlns") {
                    return Err("Reserved xmlns prefix on XML element");
                }
                let mut element = ElementData::new(name);
                element.flags |= element_flags::SCRIPT_ALREADY_STARTED;
                let mut expanded = FxHashSet::default();
                for (key, value) in attributes {
                    let ns = if key.as_namespace_binding().is_some() {
                        Some(XMLNS)
                    } else {
                        namespace(namespaces.resolve_attribute(key).0)?
                    };
                    let name = qname(ns, key.as_ref())?;
                    if !expanded.insert((name.ns.clone(), name.local.clone())) {
                        return Err("Duplicate expanded XML attribute");
                    }
                    element.attrs.push(Attr { name, value: value.into() });
                }
                let index = push(&mut nodes, parent, NodeKind::Element(Box::new(element)))?;
                if empty {
                    namespaces.pop();
                } else {
                    stack.push(index);
                }
            }
            Event::End(_) => {
                stack.pop().ok_or("Unexpected XML end tag")?;
                namespaces.pop();
            }
            Event::Text(value) => {
                if value.as_ref().contains("]]>") {
                    return Err("Invalid XML text delimiter");
                }
                text(&mut nodes, parent, &value.xml10_content())?;
            }
            Event::GeneralRef(value) => {
                if parent.is_none() {
                    return Err("Reference outside XML root");
                }
                if let Some(c) = value.resolve_char_ref().map_err(|_| "Invalid XML character reference")? {
                    if !xml_char(c) {
                        return Err("Invalid XML character reference");
                    }
                    text(&mut nodes, parent, c.encode_utf8(&mut [0; 4]))?;
                } else {
                    let value =
                        quick_xml::escape::resolve_predefined_entity(value.as_ref()).ok_or("Unknown XML entity")?;
                    text(&mut nodes, parent, value)?;
                }
            }
            Event::CData(value) => {
                if parent.is_none() {
                    return Err("CDATA outside XML root");
                }
                push(&mut nodes, parent, NodeKind::CData(value.xml10_content().into_owned()))?;
            }
            Event::Comment(value) => {
                push(&mut nodes, parent, NodeKind::Comment(value.xml10_content().into_owned()))?;
            }
            Event::PI(value) => {
                validate_name(value.target()).map_err(|_| "Invalid XML processing instruction")?;
                if value.target().eq_ignore_ascii_case("xml") {
                    return Err("Reserved XML processing instruction");
                }
                push(
                    &mut nodes,
                    parent,
                    NodeKind::ProcessingInstruction(Box::new(PiData {
                        target: value.target().into(),
                        data: value
                            .content()
                            .trim_start_matches([' ', '\t', '\r', '\n'])
                            .replace("\r\n", "\n")
                            .replace('\r', "\n"),
                    })),
                )?;
            }
            Event::Decl(value) => {
                if !first || value.version().map_err(|_| "Invalid XML declaration")? != "1.0" {
                    return Err("Only XML 1.0 declarations are supported");
                }
                if let Some(standalone) = value.standalone() {
                    if !matches!(standalone.map_err(|_| "Invalid XML declaration")?.as_ref(), "yes" | "no") {
                        return Err("Invalid XML standalone declaration");
                    }
                }
                let declaration = BytesStart::from_content(value.as_ref(), 3);
                let mut previous = 0;
                for attribute in declaration.attributes() {
                    let attribute = attribute.map_err(|_| "Invalid XML declaration")?;
                    let order = match attribute.key.as_ref() {
                        "version" => 1,
                        "encoding" => {
                            let mut chars = attribute.value.chars();
                            if !chars.next().is_some_and(|c| c.is_ascii_alphabetic())
                                || !chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
                            {
                                return Err("Invalid XML encoding name");
                            }
                            2
                        }
                        "standalone" => 3,
                        _ => return Err("Unknown XML declaration attribute"),
                    };
                    if order <= previous {
                        return Err("Invalid XML declaration order");
                    }
                    previous = order;
                }
            }
            Event::DocType(_) => return Err("DTD declarations are not supported"),
            Event::Eof => break,
        }
        first = false;
    }
    if roots != 1 || !stack.is_empty() {
        return Err("Missing or unclosed XML root");
    }
    Ok(nodes)
}

/// `encoding="..."` of a leading XML declaration.
fn declared_encoding(bytes: &[u8]) -> Option<&'static encoding_rs::Encoding> {
    let head = bytes.strip_prefix(b"<?xml")?;
    let declaration = &head[..head.iter().position(|&b| b == b'>')?];
    let at = declaration.windows(8).position(|w| w == b"encoding")?;
    let value = &declaration[at + 8..];
    let open = value.iter().position(|&b| b == b'"' || b == b'\'')?;
    let rest = &value[open + 1..];
    let close = rest.iter().position(|&b| b == value[open])?;
    encoding_rs::Encoding::for_label(&rest[..close])
}

/// Parses response bytes (XHR "XML response"): BOM, then the transport charset,
/// then the XML declaration, then UTF-8. `None` for a document that is not
/// well-formed, which includes undecodable bytes.
pub fn parse_bytes(
    dom: &mut Dom,
    url: &str,
    bytes: &[u8],
    charset: Option<&str>,
    content_type: &str,
) -> Option<NodeId> {
    let encoding = encoding_rs::Encoding::for_bom(bytes)
        .map(|(encoding, _)| encoding)
        .or_else(|| charset.and_then(|label| encoding_rs::Encoding::for_label(label.as_bytes())))
        .or_else(|| declared_encoding(bytes))
        .unwrap_or(encoding_rs::UTF_8);
    let (source, _, had_errors) = encoding.decode(bytes);
    if had_errors {
        return None;
    }
    let nodes = prepare(&source).ok()?;
    let document = new_document(dom, url, content_type);
    attach(dom, document, nodes);
    Some(document)
}

fn new_document(dom: &mut Dom, url: &str, content_type: &str) -> NodeId {
    let document = dom.create_document(DocumentKind::Xml, url);
    if let NodeKind::Document(data) = &mut dom.n_mut(document.index()).kind {
        data.content_type = content_type.into();
    }
    document
}

fn attach(dom: &mut Dom, document: NodeId, nodes: Vec<Prepared>) {
    let owner = document.index();
    let mut ids = Vec::with_capacity(nodes.len());
    for node in nodes {
        let parent = node.parent.map_or(owner, |index| ids[index]);
        let id = dom.alloc_raw(node.kind, owner);
        dom.link_raw(id, parent, NIL);
        ids.push(id);
    }
}

/// Returns an inert XML document, or a parsererror document on malformed input
/// or an explicit parser limit. Validation finishes before allocating DOM nodes,
/// so failed parses retain no partial arena trees. Never performs I/O.
pub fn parse_document(dom: &mut Dom, url: &str, source: &str, content_type: &str) -> NodeId {
    let prepared = prepare(source);
    let document = new_document(dom, url, content_type);
    match prepared {
        Ok(nodes) => attach(dom, document, nodes),
        Err(message) => {
            let error =
                dom.create_element_ns(document, Some(PARSERERROR), "parsererror").expect("fixed parsererror name");
            let text = dom.create_text(document, message).expect("live document");
            dom.append_child(document, error).expect("new document");
            dom.append_child(error, text).expect("new element");
        }
    }
    document
}

impl Dom {
    /// DOM namespace lookup, including inherited declarations and default resets.
    pub fn lookup_namespace_uri(&self, node: NodeId, prefix: Option<&str>) -> crate::DomResult<Option<&str>> {
        let prefix = prefix.filter(|value| !value.is_empty());
        if prefix == Some("xml") {
            return Ok(Some("http://www.w3.org/XML/1998/namespace"));
        }
        if prefix == Some("xmlns") {
            return Ok(Some(XMLNS));
        }
        let mut current = match &self.node(node)?.kind {
            NodeKind::Document(_) => self.document_element(node)?,
            NodeKind::Element(_) => Some(node),
            NodeKind::Fragment(_) | NodeKind::Doctype(_) => None,
            _ => self.parent_element(node)?,
        };
        while let Some(id) = current {
            let element = self.element(id)?;
            if !element.name.ns.is_empty() && element.name.prefix.as_deref() == prefix {
                return Ok(Some(&element.name.ns));
            }
            for attr in &element.attrs {
                if &*attr.name.ns == XMLNS {
                    let declared = attr.name.prefix.as_ref().map(|_| &*attr.name.local);
                    if declared == prefix {
                        return Ok((!attr.value.is_empty()).then_some(&attr.value));
                    }
                }
            }
            current = self.parent_element(id)?;
        }
        Ok(None)
    }
}
