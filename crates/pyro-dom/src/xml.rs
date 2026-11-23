//! Bounded XML 1.0 parsing for DOMParser, XHR and navigations. DOCTYPEs become nodes;
//! no DTD processing (internal subsets are ignored) and no external entities, except
//! that the XHTML and MathML public identifiers define HTML's named character
//! references, as the HTML spec's XML parser has it.
use std::sync::LazyLock;

use quick_xml::escape::resolve_predefined_entity;
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::{Namespace as XmlNamespace, NamespaceResolver, PrefixDeclaration, ResolveResult};
use quick_xml::{Reader, XmlVersion};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::dom::validate_name;
use crate::node::{element_flags, DoctypeData, ElementData, FragmentData, PiData, NIL};
use crate::{Attr, DocumentKind, Dom, NodeId, NodeKind, QualName};

pub const MAX_INPUT_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_NODES: usize = 100_000;
pub const MAX_DEPTH: usize = 256;
const MAX_ATTRIBUTES: usize = 256;
const XMLNS: &str = "http://www.w3.org/2000/xmlns/";
const PARSERERROR: &str = "http://www.mozilla.org/newlayout/xml/parsererror.xml";
type Result<T> = std::result::Result<T, &'static str>;

/// The DOCTYPE public identifiers for which the XML parser acts as if the document's DTD
/// defined HTML's named character references (HTML, "Parsing XML documents"; Chromium and
/// Firefox do the same): with one of them, `&nbsp;` is a no-break space instead of a fatal
/// error. Many XHTML documents use them (most of WPT's CSS 2.1 tests).
const HTML_ENTITY_PUBLIC_IDS: [&str; 11] = [
    "-//W3C//DTD XHTML 1.0 Transitional//EN",
    "-//W3C//DTD XHTML 1.1//EN",
    "-//W3C//DTD XHTML 1.0 Strict//EN",
    "-//W3C//DTD XHTML 1.0 Frameset//EN",
    "-//W3C//DTD XHTML Basic 1.0//EN",
    "-//W3C//DTD XHTML 1.1 plus MathML 2.0//EN",
    "-//W3C//DTD XHTML 1.1 plus MathML 2.0 plus SVG 1.1//EN",
    "-//W3C//DTD MathML 2.0//EN",
    "-//WAPFORUM//DTD XHTML Mobile 1.0//EN",
    "-//WAPFORUM//DTD XHTML Mobile 1.1//EN",
    "-//WAPFORUM//DTD XHTML Mobile 1.2//EN",
];

/// The text HTML's named character reference `&name;` stands for, one or two characters
/// (the entities with a semicolon of HTML's table: XML references always end with one).
fn html_entity(name: &str) -> Option<&'static str> {
    static ENTITIES: LazyLock<FxHashMap<&'static str, Box<str>>> = LazyLock::new(|| {
        markup5ever::data::NAMED_ENTITIES
            .entries()
            .filter_map(|(key, &(first, second))| {
                let name = key.strip_suffix(';')?;
                let text: String = [first, second].into_iter().filter(|&c| c != 0).filter_map(char::from_u32).collect();
                Some((name, text.into_boxed_str()))
            })
            .collect()
    });
    ENTITIES.get(name).map(|text| &**text)
}

/// The text of the entity reference `&name;`: a predefined XML entity, or with
/// `html_entities` (an [`HTML_ENTITY_PUBLIC_IDS`] doctype) one of HTML's.
fn entity(name: &str, html_entities: bool) -> Option<&'static str> {
    resolve_predefined_entity(name).or_else(|| html_entities.then(|| html_entity(name)).flatten())
}

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
fn text(nodes: &mut Vec<Prepared>, parent: Option<usize>, value: &str, fragment: bool) -> Result<()> {
    if parent.is_none() && !fragment {
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
/// Parses `<!DOCTYPE name [PUBLIC "pub" "sys" | SYSTEM "sys"] [[subset]]>`
/// (the text between `<!DOCTYPE` and `>`); the internal subset is ignored.
fn doctype(text: &str) -> Result<DoctypeData> {
    let text = text.trim();
    let end = text.find(|c: char| c.is_ascii_whitespace() || c == '[').unwrap_or(text.len());
    let name = &text[..end];
    validate_name(name).map_err(|_| "Invalid XML doctype name")?;
    let mut rest = text[end..].trim_start();
    let literal = |rest: &mut &str| -> Result<String> {
        let quote = rest.chars().next().filter(|c| matches!(c, '"' | '\'')).ok_or("Invalid XML doctype literal")?;
        let body = &rest[1..];
        let close = body.find(quote).ok_or("Unterminated XML doctype literal")?;
        let value = body[..close].to_owned();
        *rest = body[close + 1..].trim_start();
        Ok(value)
    };
    let (public_id, system_id) = if let Some(after) = rest.strip_prefix("PUBLIC") {
        rest = after.trim_start();
        let public = literal(&mut rest)?;
        let system = if rest.starts_with(['"', '\'']) { literal(&mut rest)? } else { String::new() };
        (public, system)
    } else if let Some(after) = rest.strip_prefix("SYSTEM") {
        rest = after.trim_start();
        (String::new(), literal(&mut rest)?)
    } else {
        (String::new(), String::new())
    };
    if !rest.is_empty() && !rest.starts_with('[') {
        return Err("Invalid XML doctype");
    }
    Ok(DoctypeData { name: name.into(), public_id: public_id.into(), system_id: system_id.into() })
}

fn prepare(source: &str) -> Result<Vec<Prepared>> {
    prepare_with(source, false)
}

/// `scripting`: elements are parser-inserted and their scripts may run
/// (a navigated XHTML document); otherwise they are inert.
fn prepare_with(source: &str, scripting: bool) -> Result<Vec<Prepared>> {
    read_nodes(source, scripting, None)
}

fn read_nodes(source: &str, scripting: bool, context: Option<NamespaceResolver>) -> Result<Vec<Prepared>> {
    let fragment = context.is_some();
    if source.len() > MAX_INPUT_BYTES {
        return Err("XML input limit exceeded");
    }
    if !source.chars().all(xml_char) {
        return Err("Invalid XML character");
    }
    let mut reader = Reader::from_str(source);
    reader.config_mut().check_comments = true;
    let mut namespaces = context.unwrap_or_default();
    namespaces.set_max_namespace_bindings(256);
    let mut nodes = Vec::new();
    let mut stack = Vec::new();
    let mut roots = 0;
    let mut first = true;
    // The doctype names a public identifier that defines HTML's entities.
    let mut html_entities = false;
    loop {
        let event = reader.read_event().map_err(|_| "Malformed XML")?;
        let parent = stack.last().copied();
        match event {
            Event::Start(ref start) | Event::Empty(ref start) => {
                let empty = matches!(event, Event::Empty(_));
                if stack.len() >= MAX_DEPTH {
                    return Err("XML depth limit exceeded");
                }
                if parent.is_none() && !fragment {
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
                        .normalized_value_with(XmlVersion::Implicit1_0, 1, |name| entity(name, html_entities))
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
                element.flags |=
                    if scripting { element_flags::PARSER_INSERTED } else { element_flags::SCRIPT_ALREADY_STARTED };
                // The XML parser's scripts are not force async either.
                element.script_force_async = false;
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
                    element.push_attr(Attr { name, value: value.into() });
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
                text(&mut nodes, parent, &value.xml10_content(), fragment)?;
            }
            Event::GeneralRef(value) => {
                if parent.is_none() && !fragment {
                    return Err("Reference outside XML root");
                }
                if let Some(c) = value.resolve_char_ref().map_err(|_| "Invalid XML character reference")? {
                    if !xml_char(c) {
                        return Err("Invalid XML character reference");
                    }
                    text(&mut nodes, parent, c.encode_utf8(&mut [0; 4]), fragment)?;
                } else {
                    let value = entity(value.as_ref(), html_entities).ok_or("Unknown XML entity")?;
                    text(&mut nodes, parent, value, fragment)?;
                }
            }
            Event::CData(value) => {
                if parent.is_none() && !fragment {
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
                if fragment || !first || value.version().map_err(|_| "Invalid XML declaration")? != "1.0" {
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
            Event::DocType(value) => {
                if fragment
                    || roots > 0
                    || parent.is_some()
                    || nodes.iter().any(|n| matches!(n.kind, NodeKind::Doctype(_)))
                {
                    return Err("Misplaced XML doctype");
                }
                let doctype = doctype(&value.xml10_content())?;
                html_entities = HTML_ENTITY_PUBLIC_IDS.contains(&&*doctype.public_id);
                push(&mut nodes, None, NodeKind::Doctype(Box::new(doctype)))?;
            }
            Event::Eof => break,
        }
        first = false;
    }
    if (!fragment && roots != 1) || !stack.is_empty() {
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
    let encoding = choose_encoding(bytes, charset);
    let (source, _, had_errors) = encoding.decode(bytes);
    if had_errors {
        return None;
    }
    let nodes = prepare(&source).ok()?;
    let document = new_document(dom, url, content_type);
    let registry = dom.registry_idx(document.index());
    attach(dom, document, nodes, registry);
    Some(document)
}

/// Chooses the encoding of an XML resource: BOM, transport charset, the
/// XML declaration, then UTF-8.
fn choose_encoding(bytes: &[u8], charset: Option<&str>) -> &'static encoding_rs::Encoding {
    encoding_rs::Encoding::for_bom(bytes)
        .map(|(encoding, _)| encoding)
        .or_else(|| charset.and_then(|label| encoding_rs::Encoding::for_label(label.as_bytes())))
        .or_else(|| declared_encoding(bytes))
        .unwrap_or(encoding_rs::UTF_8)
}

/// An element the tree builder of a navigated document has open.
struct Open {
    /// Its index among the prepared nodes.
    index: usize,
    node: u32,
    /// Where its children are linked (`create`).
    target: u32,
    /// An HTML or SVG `<script>` of the document: the builder stops at its
    /// end tag.
    script: bool,
    /// The parser holds a reference to it (it was open at a script).
    pinned: bool,
}

/// The tree builder of a navigated XML resource (XHTML, SVG, generic XML).
///
/// The whole resource is validated first: a malformed one becomes a
/// `parsererror` element and runs no script. Its nodes are then attached in
/// document order up to the end tag of each HTML or SVG `<script>` of the
/// document, where the builder stops until that script has run (HTML
/// "parsing XML documents"; SVG 2 runs a parser-inserted script there too, as
/// the HTML parser runs one in inline SVG). So a script does not find the
/// content after it, and nothing after it loads or fires events before it has
/// run: an `<iframe onload>` further down cannot call a function the scripts
/// above have yet to define.
pub(crate) struct Load {
    document: NodeId,
    /// The nodes not attached yet, in document order.
    nodes: std::iter::Peekable<std::vec::IntoIter<Prepared>>,
    /// How many are attached: the next one's index among the prepared nodes.
    attached: usize,
    /// The open elements, outermost first.
    open: Vec<Open>,
    /// What is attached has had its insertion steps: from the first stop at
    /// a script (or the end) on. Nothing can observe the tree before that,
    /// so it is linked as `attach` links it and the steps run once over all
    /// of it. Afterwards scripts may have run, and each node is inserted as
    /// a script would insert it.
    live: bool,
}

impl Load {
    /// Decodes and validates the resource for `document`, an empty document
    /// the browsing context created.
    pub(crate) fn new(
        dom: &mut Dom,
        document: NodeId,
        bytes: &[u8],
        charset: Option<&str>,
        content_type: &str,
    ) -> Self {
        let encoding = choose_encoding(bytes, charset);
        let (source, _, _) = encoding.decode(bytes);
        let _ = dom.set_document_kind(document, DocumentKind::Xml, content_type, encoding.name());
        let (nodes, live) = match prepare_with(&source, true) {
            Ok(nodes) => (nodes, false),
            Err(message) => {
                parser_error(dom, document, message);
                (Vec::new(), true)
            }
        };
        Load { document, nodes: nodes.into_iter().peekable(), attached: 0, open: Vec::new(), live }
    }

    /// Attaches the nodes up to the end tag of the next script and returns
    /// that script, for the caller to prepare before the next call; `None`
    /// once the rest is attached. The elements open at a script get a
    /// reference each, added to `pins` for the caller to release: the script
    /// may remove them, and the nodes that follow still go into them.
    pub(crate) fn step(&mut self, dom: &mut Dom, pins: &mut Vec<NodeId>) -> Option<NodeId> {
        let document = self.document.index();
        loop {
            // End tags: of every open element the next node is not a child of.
            let parent = self.nodes.peek().map(|node| node.parent);
            while let Some(open) = self.open.pop() {
                if parent == Some(Some(open.index)) {
                    self.open.push(open);
                    break;
                }
                if !open.script {
                    continue;
                }
                self.go_live(dom);
                for held in self.open.iter_mut().filter(|held| !held.pinned) {
                    held.pinned = pin(dom, held.node, pins);
                }
                if !open.pinned {
                    pin(dom, open.node, pins);
                }
                return Some(dom.id_of(open.node));
            }
            let Some(node) = self.nodes.next() else { break };
            let index = self.attached;
            self.attached += 1;
            // The parent as it is now: a script may have moved it to another
            // document, whose nodes its children then are.
            let (parent, owner) = match self.open.last() {
                Some(open) => (open.target, dom.n(open.target).owner_doc),
                None => (document, document),
            };
            let (is_element, script) = match &node.kind {
                NodeKind::Element(e) => (
                    true,
                    owner == document
                        && (e.is_html() || &*e.name.ns == "http://www.w3.org/2000/svg")
                        && &*e.name.local == "script",
                ),
                _ => (false, false),
            };
            let (id, target) = create(dom, node.kind, owner);
            if self.live {
                dom.note_inline_handlers(id);
                dom.note_inline_style(id);
                dom.append_parsed(id, parent);
            } else {
                dom.link_raw(id, parent, NIL);
            }
            if is_element {
                self.open.push(Open { index, node: id, target, script, pinned: false });
            }
        }
        self.go_live(dom);
        None
    }

    /// What the HTML parser does node by node, for the part of the document
    /// attached before any of its scripts ran: `on*` attributes are queued
    /// for compilation and the insertion steps run, so iframes get their
    /// browsing contexts and style sheets and images load.
    fn go_live(&mut self, dom: &mut Dom) {
        if std::mem::replace(&mut self.live, true) {
            return;
        }
        let elements: Vec<u32> =
            dom.descendants(self.document).map(|d| d.map(|n| n.index()).collect()).unwrap_or_default();
        for i in elements {
            dom.note_inline_handlers(i);
            dom.note_inline_style(i);
        }
        let top: Vec<u32> = dom.child_idx_iter(self.document.index()).collect();
        for node in top {
            dom.resources_after_insert(node);
        }
    }
}

/// References `node` for a parser: true when `pins` got it.
fn pin(dom: &mut Dom, node: u32, pins: &mut Vec<NodeId>) -> bool {
    let id = dom.id_of(node);
    let pinned = dom.add_ref(id).is_ok();
    if pinned {
        pins.push(id);
    }
    pinned
}

fn new_document(dom: &mut Dom, url: &str, content_type: &str) -> NodeId {
    let document = dom.create_document(DocumentKind::Xml, url);
    if let NodeKind::Document(data) = &mut dom.n_mut(document.index()).kind {
        data.content_type = content_type.into();
    }
    document
}

/// Allocates a prepared node for the document `owner`, unlinked. Returns it
/// and where its children are linked: those of an XHTML `<template>` go into
/// its template contents, a fragment in the inert document (HTML "parsing
/// XHTML documents"); templates in other namespaces keep their children.
fn create(dom: &mut Dom, kind: NodeKind, owner: u32) -> (u32, u32) {
    let is_template = match &kind {
        NodeKind::Element(e) if e.is_html() => {
            // `<base>` elements make `document_base_url` look for one.
            dom.base_seen |= &*e.name.local == "base";
            &*e.name.local == "template"
        }
        _ => false,
    };
    let id = dom.alloc_raw(kind, owner);
    if !is_template {
        return (id, id);
    }
    let inert = dom.inert_doc_for_pub(owner);
    let frag = dom.alloc_raw(NodeKind::Fragment(FragmentData::new(id, None, 0)), inert);
    dom.n_mut(id).element_mut().expect("template element").template_contents = frag;
    (id, frag)
}

/// XML fragment parsing inherits the context element's in-scope bindings.
pub(crate) fn parse_fragment(
    dom: &mut Dom,
    context: NodeId,
    source: &str,
    body_context: bool,
) -> crate::DomResult<NodeId> {
    use crate::{DomError, DomErrorKind};

    let syntax = |message| DomError::new(DomErrorKind::Syntax, message);
    let mut namespaces = NamespaceResolver::default();
    namespaces.set_max_namespace_bindings(256);
    let mut seen = FxHashSet::default();
    let mut bind = |prefix, uri: &str| -> crate::DomResult<()> {
        if seen.insert(prefix) {
            namespaces.add(prefix, XmlNamespace(uri)).map_err(|_| syntax("Invalid XML namespace context"))?;
        }
        Ok(())
    };
    if body_context {
        bind(PrefixDeclaration::Default, "http://www.w3.org/1999/xhtml")?;
    } else {
        // lookupNamespaceURI consults the element's own qualified name before
        // its declarations, then searches ancestors. An empty declaration
        // masks an ancestor's binding.
        let mut ancestor = Some(context);
        while let Some(id) = ancestor {
            let element = dom.element(id)?;
            if !element.name.ns.is_empty() {
                bind(
                    element.name.prefix.as_deref().map_or(PrefixDeclaration::Default, PrefixDeclaration::Named),
                    &element.name.ns,
                )?;
            }
            for attr in &element.attrs {
                if &*attr.name.ns == XMLNS {
                    let prefix = if attr.name.prefix.is_some() {
                        PrefixDeclaration::Named(&attr.name.local)
                    } else {
                        PrefixDeclaration::Default
                    };
                    bind(prefix, &attr.value)?;
                }
            }
            ancestor = dom.parent_element(id)?;
        }
    }
    let nodes = read_nodes(source, false, Some(namespaces)).map_err(syntax)?;
    let fragment = dom.create_fragment(dom.node_document(context)?)?;
    let registry = dom.registry_association(context)?;
    attach(dom, fragment, nodes, registry);
    dom.upgrades_after_fragment_parse(fragment.index());
    Ok(fragment)
}

fn attach(dom: &mut Dom, root: NodeId, nodes: Vec<Prepared>, registry: crate::RegistryAssociation) {
    let root = root.index();
    let owner = dom.n(root).owner_doc;
    // Per prepared node: where its children are linked and which document
    // owns them.
    let mut targets: Vec<(u32, u32)> = Vec::with_capacity(nodes.len());
    for node in nodes {
        let (parent, child_owner) = node.parent.map_or((root, owner), |index| targets[index]);
        let (id, target) = create(dom, node.kind, child_owner);
        if dom.n(id).element().is_some() {
            let registry = if parent == root { registry } else { dom.registry_idx(parent) };
            dom.set_registry_idx(id, registry);
        }
        dom.link_raw(id, parent, NIL);
        targets.push((target, dom.n(target).owner_doc));
    }
}

/// The document of a resource that is not well-formed XML.
fn parser_error(dom: &mut Dom, document: NodeId, message: &str) {
    let error = dom.create_element_ns(document, Some(PARSERERROR), "parsererror").expect("fixed parsererror name");
    let text = dom.create_text(document, message).expect("live document");
    dom.append_child(document, error).expect("new document");
    dom.append_child(error, text).expect("new element");
}

/// Returns an inert XML document, or a parsererror document on malformed input
/// or an explicit parser limit. Validation finishes before allocating DOM nodes,
/// so failed parses retain no partial arena trees. Never performs I/O.
pub fn parse_document(dom: &mut Dom, url: &str, source: &str, content_type: &str) -> NodeId {
    let prepared = prepare(source);
    let document = new_document(dom, url, content_type);
    match prepared {
        Ok(nodes) => {
            let registry = dom.registry_idx(document.index());
            attach(dom, document, nodes, registry);
        }
        Err(message) => parser_error(dom, document, message),
    }
    document
}

impl Dom {
    /// DOM "locate a namespace" (`lookupNamespaceURI`).
    pub fn lookup_namespace_uri(&self, node: NodeId, prefix: Option<&str>) -> crate::DomResult<Option<&str>> {
        let prefix = prefix.filter(|value| !value.is_empty());
        let mut current = match &self.node(node)?.kind {
            NodeKind::Document(_) => self.document_element(node)?,
            NodeKind::Element(_) => Some(node),
            NodeKind::Fragment(_) | NodeKind::Doctype(_) => None,
            NodeKind::Attr(a) => self.opt_id(a.owner),
            _ => self.parent_element(node)?,
        };
        if current.is_some() {
            if prefix == Some("xml") {
                return Ok(Some("http://www.w3.org/XML/1998/namespace"));
            }
            if prefix == Some("xmlns") {
                return Ok(Some(XMLNS));
            }
        }
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

    /// DOM "locate a namespace prefix" (`lookupPrefix`).
    pub fn lookup_prefix(&self, node: NodeId, namespace: Option<&str>) -> crate::DomResult<Option<&str>> {
        let Some(namespace) = namespace.filter(|value| !value.is_empty()) else { return Ok(None) };
        let mut current = match &self.node(node)?.kind {
            NodeKind::Document(_) => self.document_element(node)?,
            NodeKind::Element(_) => Some(node),
            NodeKind::Fragment(_) | NodeKind::Doctype(_) => None,
            NodeKind::Attr(a) => self.opt_id(a.owner),
            _ => self.parent_element(node)?,
        };
        while let Some(id) = current {
            let element = self.element(id)?;
            if &*element.name.ns == namespace {
                if let Some(prefix) = element.name.prefix.as_deref() {
                    return Ok(Some(prefix));
                }
            }
            for attr in &element.attrs {
                if &*attr.name.ns == XMLNS && attr.name.prefix.as_deref() == Some("xmlns") && &*attr.value == namespace
                {
                    return Ok(Some(&attr.name.local));
                }
            }
            current = self.parent_element(id)?;
        }
        Ok(None)
    }
}

#[cfg(test)]
mod fragment_tests {
    use super::*;
    use crate::parse::{parse_contextual_fragment, parse_insertion_fragment};
    use crate::DomErrorKind;

    #[test]
    fn fragment_uses_context_bindings_and_preserves_mixed_top_level_content() {
        let mut dom = Dom::new();
        let doc = parse_document(&mut dom, "about:blank", r#"<R xmlns="urn:a" xmlns:p="urn:p"><C/></R>"#, "text/xml");
        let root = dom.document_element(doc).unwrap().unwrap();
        let context = dom.first_child(root).unwrap().unwrap();
        let frag =
            parse_insertion_fragment(&mut dom, context, r#"a&amp;<p:X xmlns:p="urn:local"/><p:Y/><Z/><![CDATA[<t>]]>"#)
                .unwrap();
        let children: Vec<_> = dom.children(frag).unwrap().collect();
        assert_eq!(children.len(), 5);
        assert!(matches!(&dom.node(children[0]).unwrap().kind, NodeKind::Text(t) if t == "a&"));
        for (&node, (ns, local)) in children[1..4].iter().zip([("urn:local", "X"), ("urn:p", "Y"), ("urn:a", "Z")]) {
            let el = dom.element(node).unwrap();
            assert_eq!((&*el.name.ns, &*el.name.local), (ns, local));
            assert_eq!(dom.node_document(node).unwrap(), doc);
        }
        assert!(matches!(&dom.node(children[4]).unwrap().kind, NodeKind::CData(t) if t == "<t>"));
    }

    #[test]
    fn ill_formed_fragments_leave_existing_children_intact() {
        let mut dom = Dom::new();
        let doc = parse_document(&mut dom, "about:blank", "<r><old/></r>", "text/xml");
        let root = dom.document_element(doc).unwrap().unwrap();
        let old = dom.first_child(root).unwrap();
        for source in ["<new/><open>", "<p:x/>", "]]>", "<!DOCTYPE root>", "<?xml version='1.0'?>"] {
            assert_eq!(parse_insertion_fragment(&mut dom, root, source).unwrap_err().kind(), Some(DomErrorKind::Syntax));
            assert_eq!(dom.first_child(root).unwrap(), old);
        }
    }

    #[test]
    fn range_preserves_namespace_and_case_in_xml_documents() {
        let mut dom = Dom::new();
        let doc = parse_document(&mut dom, "about:blank", "<r/>", "text/xml");
        let root = dom.document_element(doc).unwrap().unwrap();
        let source = r#"<html xmlns="urn:foreign"><Body/></html>"#;
        let frag = parse_contextual_fragment(&mut dom, root, source).unwrap();
        let html = dom.first_child(frag).unwrap().unwrap();
        assert_eq!(&*dom.element(html).unwrap().name.ns, "urn:foreign");
        assert_eq!(dom.tag_name(dom.first_child(html).unwrap().unwrap()).unwrap(), "Body");
    }
}
