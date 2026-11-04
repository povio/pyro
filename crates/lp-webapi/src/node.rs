//! Node, CharacterData, Element, Document, DocumentFragment bindings.
//!
//! Every node wrapper holds one external reference on its node (released
//! when the wrapper is finalized), so detached subtrees reachable from JS
//! stay alive and unreachable ones are collected by `Dom::collect_garbage`.

use std::rc::Rc;

use lp_dom::events::TargetKey;
use lp_dom::{DocumentKind, NodeId, NodeKind, NodeType, ShadowRootMode};
use lp_js::{Args, ClassDef, JsClass, JsError, JsResult, Object, Scope, Value};

use crate::collections::{self, CollectionKind};
use crate::page::PageDom;
use crate::target::{page_of, EventTarget, EventTargetData};
use crate::util::*;

/// Payload of every Node wrapper.
pub struct NodeData {
    pub target: EventTargetData,
    pub id: NodeId,
}

impl NodeData {
    pub fn page(&self) -> &Rc<PageDom> {
        &self.target.page
    }
}

impl Drop for NodeData {
    fn drop(&mut self) {
        self.target.page.release_node(self.id);
    }
}

/// Wrapper of `id` for script in `s`'s realm: one per node per script world. A
/// frame tree shares one `Dom`, so a node that moved between its documents may
/// already have a wrapper in a sibling realm; a new one is made in `s`'s realm.
pub fn wrap_node<'s>(s: &Scope<'s>, page: &PageDom, id: NodeId) -> JsResult<Object<'s>> {
    let key = page.node_identity(id);
    if let Some(o) = s.lookup_identity(key) {
        return Ok(o);
    }
    let page_rc = page_of(s)?;
    if page_rc.id() != page.id() {
        if !Rc::ptr_eq(&page_rc.dom, &page.dom) {
            return Err(JsError::Type("node belongs to another page".into()));
        }
        return wrap_node(s, &page_rc, id);
    }
    if let Some(o) = page.sibling_wrapper(s, id) {
        return Ok(o);
    }
    let (ty, class) = {
        let mut dom = page.dom.borrow_mut();
        dom.add_ref(id).js()?;
        let node = dom.node(id).js()?;
        (node.kind.node_type(), crate::html::class_for(&dom, id))
    };
    let data = NodeData { target: EventTargetData { page: page_rc, key: TargetKey::Node(id.index()) }, id };
    let k = Some(key);
    match ty {
        NodeType::Element => crate::html::wrap_element(s, class, data, k),
        NodeType::Text => s.wrap::<Text>(data, k),
        NodeType::CDataSection => s.wrap::<CDataSection>(data, k),
        NodeType::Comment => s.wrap::<Comment>(data, k),
        NodeType::ProcessingInstruction => s.wrap::<ProcessingInstruction>(data, k),
        NodeType::Document => {
            let html = page.dom.borrow().document_data(id).map(|d| d.kind == DocumentKind::Html).unwrap_or(true);
            if html {
                s.wrap::<HtmlDocument>(data, k)
            } else {
                s.wrap::<XmlDocument>(data, k)
            }
        }
        NodeType::DocumentType => s.wrap::<DocumentType>(data, k),
        NodeType::DocumentFragment => {
            if page.dom.borrow().is_shadow_root(id).unwrap_or(false) {
                s.wrap::<ShadowRoot>(data, k)
            } else {
                s.wrap::<DocumentFragment>(data, k)
            }
        }
        NodeType::Attribute => Err(JsError::Type("unexpected attribute node".into())),
    }
}

pub fn node_value<'s>(s: &Scope<'s>, page: &PageDom, id: Option<NodeId>) -> JsResult<Value<'s>> {
    opt_node(s, page, id)
}

/// Extracts a Node argument (WebIDL `Node`), with a TypeError otherwise.
pub fn node_arg(s: &Scope<'_>, v: Value<'_>, what: &str) -> JsResult<NodeId> {
    s.unwrap::<Node>(v)
        .map(|n| n.id)
        .ok_or_else(|| JsError::Type(format!("Failed to execute '{what}': parameter is not of type 'Node'.")))
}

pub fn opt_node_arg(s: &Scope<'_>, v: Value<'_>, what: &str) -> JsResult<Option<NodeId>> {
    if v.is_nullish() {
        Ok(None)
    } else {
        node_arg(s, v, what).map(Some)
    }
}

/// `(Node or DOMString)...` -> single node (ParentNode/ChildNode helpers).
fn convert_nodes(s: &Scope<'_>, n: &NodeData, a: &Args<'_>) -> JsResult<Option<NodeId>> {
    NodeArgs::collect(s, a)?.into_node(n)
}

/// `(Node or DOMString)...` arguments with strings already converted, so the
/// ChildNode methods can find their viable sibling between IDL conversion
/// (which may run JS toString) and the DOM spec's "convert nodes into a node".
struct NodeArgs(Vec<NodeArg>);

enum NodeArg {
    Node(NodeId),
    Text(String),
}

impl NodeArgs {
    fn collect(s: &Scope<'_>, a: &Args<'_>) -> JsResult<Self> {
        let mut items = Vec::with_capacity(a.len());
        for v in a.iter() {
            items.push(match s.unwrap::<Node>(v) {
                Some(nd) => NodeArg::Node(nd.id),
                None => NodeArg::Text(dom_string(v)?),
            });
        }
        Ok(Self(items))
    }

    fn contains(&self, id: NodeId) -> bool {
        self.0.iter().any(|it| matches!(it, NodeArg::Node(n) if *n == id))
    }

    /// First sibling of `id` in the given direction that is not an argument.
    fn viable_sibling(&self, dom: &lp_dom::Dom, id: NodeId, forward: bool) -> JsResult<Option<NodeId>> {
        let mut cur = id;
        loop {
            let next = if forward { dom.next_sibling(cur).js()? } else { dom.prev_sibling(cur).js()? };
            match next {
                Some(c) if self.contains(c) => cur = c,
                other => return Ok(other),
            }
        }
    }

    fn into_node(self, n: &NodeData) -> JsResult<Option<NodeId>> {
        let mut dom = n.page().dom.borrow_mut();
        let doc = dom.node_document(n.id).js()?;
        let mut nodes = Vec::with_capacity(self.0.len());
        for it in self.0 {
            match it {
                NodeArg::Node(id) => nodes.push(id),
                NodeArg::Text(t) => nodes.push(dom.create_text(doc, &t).js()?),
            }
        }
        match nodes.len() {
            0 => Ok(None),
            1 => Ok(Some(nodes[0])),
            _ => {
                let frag = dom.create_fragment(doc).js()?;
                for c in nodes {
                    dom.append_child(frag, c).js()?;
                }
                Ok(Some(frag))
            }
        }
    }
}

/// Keeps `on*` IDL handlers in sync with `setAttribute`/`removeAttribute`
/// on HTML and SVG elements.
pub(crate) fn handler_attribute_changed(s: &Scope<'_>, n: &NodeData, name: &str, value: Option<&str>) -> JsResult<()> {
    let Some(ty) = crate::handlers::is_handler_attribute(&name.to_ascii_lowercase()) else { return Ok(()) };
    let (eligible, reflecting) = {
        let dom = n.page().dom.borrow();
        match dom.element(n.id) {
            Ok(e) => (
                e.is_html() || e.namespace() == &markup5ever::ns!(svg),
                e.is_html()
                    && matches!(&*e.name.local, "body" | "frameset")
                    && crate::handlers::reflects_to_window(ty)
                    && dom.node_document(n.id).ok() == Some(n.page().document()),
            ),
            Err(_) => (false, false),
        }
    };
    if !eligible {
        return Ok(());
    }
    match n.page().window_target().filter(|_| reflecting) {
        Some(window) => {
            let target = crate::target::EventTargetData::window(n.page(), window);
            crate::handlers::content_attribute_changed(s, &target, ty, value)
        }
        None => crate::handlers::content_attribute_changed(s, &n.target, ty, value),
    }
}

pub struct Node;
impl JsClass for Node {
    const NAME: &'static str = "Node";
    type Data = NodeData;
    type Parent = EventTarget;
    fn define(def: &mut ClassDef<Self>) {
        def.upcast(|n: &NodeData| &n.target);
        for (name, v) in [
            ("ELEMENT_NODE", 1.0),
            ("ATTRIBUTE_NODE", 2.0),
            ("TEXT_NODE", 3.0),
            ("CDATA_SECTION_NODE", 4.0),
            ("ENTITY_REFERENCE_NODE", 5.0),
            ("ENTITY_NODE", 6.0),
            ("PROCESSING_INSTRUCTION_NODE", 7.0),
            ("COMMENT_NODE", 8.0),
            ("DOCUMENT_NODE", 9.0),
            ("DOCUMENT_TYPE_NODE", 10.0),
            ("DOCUMENT_FRAGMENT_NODE", 11.0),
            ("NOTATION_NODE", 12.0),
            ("DOCUMENT_POSITION_DISCONNECTED", 1.0),
            ("DOCUMENT_POSITION_PRECEDING", 2.0),
            ("DOCUMENT_POSITION_FOLLOWING", 4.0),
            ("DOCUMENT_POSITION_CONTAINS", 8.0),
            ("DOCUMENT_POSITION_CONTAINED_BY", 16.0),
            ("DOCUMENT_POSITION_IMPLEMENTATION_SPECIFIC", 32.0),
        ] {
            def.constant(name, v);
        }
        def.getter("nodeType", |s, n| Ok(s.number(n.page().dom.borrow().node_type(n.id).js()? as u16 as f64)));
        def.getter("nodeName", |s, n| Ok(s.string(&n.page().dom.borrow().node_name(n.id).js()?)));
        def.accessor(
            "nodeValue",
            |s, n| {
                let dom = n.page().dom.borrow();
                Ok(match dom.node(n.id).js()?.kind.char_data() {
                    Some(d) => s.string(d),
                    None => s.null(),
                })
            },
            |_s, n, v| {
                let text = null_to_empty(v)?;
                let mut dom = n.page().dom.borrow_mut();
                if dom.node(n.id).js()?.kind.char_data().is_some() {
                    dom.set_char_data(n.id, &text).js()?;
                }
                Ok(())
            },
        );
        def.accessor(
            "textContent",
            |s, n| {
                let t = n.page().dom.borrow().text_content(n.id).js()?;
                Ok(t.map_or(s.null(), |t| s.string(&t)))
            },
            |_s, n, v| {
                let text = null_to_empty(v)?;
                crate::input::blur_before_removal(n.page(), n.id, true);
                n.page().dom.borrow_mut().set_text_content(n.id, &text).js()
            },
        );
        def.getter("parentNode", |s, n| {
            let p = n.page().dom.borrow().parent(n.id).js()?;
            node_value(s, n.page(), p)
        });
        def.getter("parentElement", |s, n| {
            let p = n.page().dom.borrow().parent_element(n.id).js()?;
            node_value(s, n.page(), p)
        });
        def.getter("childNodes", |s, n| Ok(collections::child_nodes(s, n.page(), n.id)?.value()));
        def.getter("firstChild", |s, n| {
            let c = n.page().dom.borrow().first_child(n.id).js()?;
            node_value(s, n.page(), c)
        });
        def.getter("lastChild", |s, n| {
            let c = n.page().dom.borrow().last_child(n.id).js()?;
            node_value(s, n.page(), c)
        });
        def.getter("previousSibling", |s, n| {
            let c = n.page().dom.borrow().prev_sibling(n.id).js()?;
            node_value(s, n.page(), c)
        });
        def.getter("nextSibling", |s, n| {
            let c = n.page().dom.borrow().next_sibling(n.id).js()?;
            node_value(s, n.page(), c)
        });
        def.getter("ownerDocument", |s, n| {
            let d = n.page().dom.borrow().owner_document(n.id).js()?;
            node_value(s, n.page(), d)
        });
        def.getter("isConnected", |s, n| Ok(s.boolean(n.page().dom.borrow().is_connected(n.id).js()?)));
        def.getter("baseURI", |s, n| {
            let dom = n.page().dom.borrow();
            let doc = dom.node_document(n.id).js()?;
            Ok(s.string(dom.document_base_url(doc).js()?))
        });
        def.method("hasChildNodes", 0, |s, n, _a| Ok(s.boolean(n.page().dom.borrow().child_count(n.id).js()? > 0)));
        def.method("getRootNode", 0, |s, n, a| {
            let composed = match a.get(0).as_object() {
                Some(o) => o.get("composed")?.to_bool(),
                None => false,
            };
            let r = n.page().dom.borrow().root_node(n.id, composed).js()?;
            Ok(wrap_node(s, n.page(), r)?.value())
        });
        def.method("appendChild", 1, |s, n, a| {
            a.require(1, "Node.appendChild")?;
            let c = node_arg(s, a.get(0), "appendChild")?;
            blur_moved(n.page(), c);
            n.page().dom.borrow_mut().append_child(n.id, c).js()?;
            Ok(a.get(0))
        });
        def.method("insertBefore", 2, |s, n, a| {
            a.require(2, "Node.insertBefore")?;
            let c = node_arg(s, a.get(0), "insertBefore")?;
            let r = opt_node_arg(s, a.get(1), "insertBefore")?;
            blur_moved(n.page(), c);
            n.page().dom.borrow_mut().insert_before(n.id, c, r).js()?;
            Ok(a.get(0))
        });
        def.method("removeChild", 1, |s, n, a| {
            a.require(1, "Node.removeChild")?;
            let c = node_arg(s, a.get(0), "removeChild")?;
            blur_removed_child(n.page(), n.id, c);
            n.page().dom.borrow_mut().remove_child(n.id, c).js()?;
            Ok(a.get(0))
        });
        def.method("replaceChild", 2, |s, n, a| {
            a.require(2, "Node.replaceChild")?;
            let new = node_arg(s, a.get(0), "replaceChild")?;
            let old = node_arg(s, a.get(1), "replaceChild")?;
            blur_removed_child(n.page(), n.id, old);
            n.page().dom.borrow_mut().replace_child(n.id, new, old).js()?;
            Ok(a.get(1))
        });
        def.method("cloneNode", 0, |s, n, a| {
            let deep = a.get(0).to_bool();
            let c = n.page().dom.borrow_mut().clone_node(n.id, deep, None).js()?;
            Ok(wrap_node(s, n.page(), c)?.value())
        });
        def.method("contains", 1, |s, n, a| {
            let Some(o) = opt_node_arg(s, a.get(0), "contains")? else { return Ok(s.boolean(false)) };
            Ok(s.boolean(n.page().dom.borrow().contains(n.id, o).js()?))
        });
        def.method("compareDocumentPosition", 1, |s, n, a| {
            a.require(1, "Node.compareDocumentPosition")?;
            let o = node_arg(s, a.get(0), "compareDocumentPosition")?;
            Ok(s.number(n.page().dom.borrow().compare_document_position(n.id, o).js()? as f64))
        });
        def.method("isSameNode", 1, |s, n, a| {
            let o = opt_node_arg(s, a.get(0), "isSameNode")?;
            Ok(s.boolean(o == Some(n.id)))
        });
        def.method("isEqualNode", 1, |s, n, a| {
            let Some(o) = opt_node_arg(s, a.get(0), "isEqualNode")? else { return Ok(s.boolean(false)) };
            let dom = n.page().dom.borrow();
            // Structural equality via serialization of both subtrees.
            let eq = dom.node_type(n.id).js()? == dom.node_type(o).js()?
                && dom.outer_html(n.id).js()? == dom.outer_html(o).js()?;
            Ok(s.boolean(eq))
        });
        def.method("normalize", 0, |s, n, _a| {
            n.page().dom.borrow_mut().normalize(n.id).js()?;
            Ok(s.undefined())
        });
        def.method("lookupNamespaceURI", 1, |s, n, a| {
            let prefix = opt_dom_string(a.get(0))?;
            let dom = n.page().dom.borrow();
            Ok(match dom.lookup_namespace_uri(n.id, prefix.as_deref()).js()? {
                Some(namespace) => s.string(namespace),
                None => s.null(),
            })
        });
        def.method("isDefaultNamespace", 1, |s, n, a| {
            let namespace = opt_dom_string(a.get(0))?;
            let dom = n.page().dom.borrow();
            Ok(s.boolean(dom.lookup_namespace_uri(n.id, None).js()? == namespace.as_deref().filter(|s| !s.is_empty())))
        });
    }
}

/// Chromium's blur before `parent` loses `child` (`removeChild`,
/// `replaceChild`, `remove`, `replaceWith`): only when it is a child, as a
/// non-child throws before anything happens.
fn blur_removed_child(page: &Rc<PageDom>, parent: NodeId, child: NodeId) {
    if page.dom.borrow().parent(child).ok().flatten() == Some(parent) {
        crate::input::blur_before_removal(page, child, false);
    }
}

/// Inserting a node that has a parent removes it from there first.
fn blur_moved(page: &Rc<PageDom>, node: NodeId) {
    let has_parent = page.dom.borrow().parent(node).ok().flatten().is_some();
    if has_parent {
        crate::input::blur_before_removal(page, node, false);
    }
}

// ----- mixins -------------------------------------------------------------

fn define_parent_node<C: JsClass<Data = NodeData>>(def: &mut ClassDef<C>) {
    def.getter("children", |s, n| Ok(collections::children(s, n.page(), n.id)?.value()));
    def.getter("firstElementChild", |s, n| {
        let c = n.page().dom.borrow().element_children(n.id).js()?.next();
        node_value(s, n.page(), c)
    });
    def.getter("lastElementChild", |s, n| {
        let c = n.page().dom.borrow().element_children(n.id).js()?.last();
        node_value(s, n.page(), c)
    });
    def.getter("childElementCount", |s, n| {
        Ok(s.number(n.page().dom.borrow().element_children(n.id).js()?.count() as f64))
    });
    def.method("append", 0, |s, n, a| {
        if let Some(c) = convert_nodes(s, n, &a)? {
            n.page().dom.borrow_mut().append_child(n.id, c).js()?;
        }
        Ok(s.undefined())
    });
    def.method("prepend", 0, |s, n, a| {
        if let Some(c) = convert_nodes(s, n, &a)? {
            let mut dom = n.page().dom.borrow_mut();
            let first = dom.first_child(n.id).js()?;
            dom.insert_before(n.id, c, first).js()?;
        }
        Ok(s.undefined())
    });
    def.method("replaceChildren", 0, |s, n, a| {
        let c = convert_nodes(s, n, &a)?;
        crate::input::blur_before_removal(n.page(), n.id, true);
        n.page().dom.borrow_mut().replace_all(n.id, c).js()?;
        Ok(s.undefined())
    });
    def.method("querySelector", 1, |s, n, a| {
        let page = n.page();
        let list = arg_selector(&a, 0, "querySelector", page)?;
        let r = page.dom.borrow().query_first(n.id, &list).js()?;
        node_value(s, page, r)
    });
    def.method("querySelectorAll", 1, |s, n, a| {
        let page = n.page();
        let list = arg_selector(&a, 0, "querySelectorAll", page)?;
        let r = page.dom.borrow().query_all(n.id, &list).js()?;
        Ok(collections::static_node_list(s, page, r)?.value())
    });
}

fn define_child_node<C: JsClass<Data = NodeData>>(def: &mut ClassDef<C>) {
    def.method("remove", 0, |s, n, _a| {
        crate::input::blur_before_removal(n.page(), n.id, false);
        n.page().dom.borrow_mut().remove_node(n.id).js()?;
        Ok(s.undefined())
    });
    // before/after/replaceWith insert relative to the nearest sibling that is not
    // itself an argument (the DOM spec's viable sibling), found before the
    // arguments move into a fragment, so the context node and its siblings can be
    // passed as arguments.
    def.method("before", 0, |s, n, a| {
        let args = NodeArgs::collect(s, &a)?;
        let (parent, viable_prev) = {
            let dom = n.page().dom.borrow();
            let Some(parent) = dom.parent(n.id).js()? else { return Ok(s.undefined()) };
            (parent, args.viable_sibling(&dom, n.id, false)?)
        };
        if let Some(c) = args.into_node(n)? {
            let mut dom = n.page().dom.borrow_mut();
            let child = match viable_prev {
                Some(p) => dom.next_sibling(p).js()?,
                None => dom.first_child(parent).js()?,
            };
            dom.insert_before(parent, c, child).js()?;
        }
        Ok(s.undefined())
    });
    def.method("after", 0, |s, n, a| {
        let args = NodeArgs::collect(s, &a)?;
        let (parent, viable_next) = {
            let dom = n.page().dom.borrow();
            let Some(parent) = dom.parent(n.id).js()? else { return Ok(s.undefined()) };
            (parent, args.viable_sibling(&dom, n.id, true)?)
        };
        if let Some(c) = args.into_node(n)? {
            n.page().dom.borrow_mut().insert_before(parent, c, viable_next).js()?;
        }
        Ok(s.undefined())
    });
    def.method("replaceWith", 0, |s, n, a| {
        let args = NodeArgs::collect(s, &a)?;
        let (parent, viable_next) = {
            let dom = n.page().dom.borrow();
            let Some(parent) = dom.parent(n.id).js()? else { return Ok(s.undefined()) };
            (parent, args.viable_sibling(&dom, n.id, true)?)
        };
        let c = args.into_node(n)?;
        if c != Some(n.id) {
            blur_removed_child(n.page(), parent, n.id);
        }
        let mut dom = n.page().dom.borrow_mut();
        // The context node itself may have moved into the fragment.
        let still_child = dom.parent(n.id).js()? == Some(parent);
        match c {
            Some(c) if c == n.id => {}
            Some(c) if still_child => {
                dom.replace_child(parent, c, n.id).js()?;
            }
            Some(c) => {
                dom.insert_before(parent, c, viable_next).js()?;
            }
            None if still_child => {
                dom.remove_child(parent, n.id).js()?;
            }
            None => {}
        }
        Ok(s.undefined())
    });
}

fn sibling_element(n: &NodeData, forward: bool) -> JsResult<Option<NodeId>> {
    let dom = n.page().dom.borrow();
    let mut cur = n.id;
    loop {
        let next = if forward { dom.next_sibling(cur).js()? } else { dom.prev_sibling(cur).js()? };
        match next {
            None => return Ok(None),
            Some(c) if dom.node_type(c).js()? == NodeType::Element => return Ok(Some(c)),
            Some(c) => cur = c,
        }
    }
}

fn define_non_doctype_child<C: JsClass<Data = NodeData>>(def: &mut ClassDef<C>) {
    def.getter("previousElementSibling", |s, n| node_value(s, n.page(), sibling_element(n, false)?));
    def.getter("nextElementSibling", |s, n| node_value(s, n.page(), sibling_element(n, true)?));
}

// ----- character data -----------------------------------------------------

pub struct CharacterData;
impl JsClass for CharacterData {
    const NAME: &'static str = "CharacterData";
    type Data = NodeData;
    type Parent = Node;
    fn define(def: &mut ClassDef<Self>) {
        define_child_node(def);
        define_non_doctype_child(def);
        def.accessor(
            "data",
            |s, n| Ok(s.string(n.page().dom.borrow().char_data(n.id).js()?)),
            |_s, n, v| {
                let t = null_to_empty(v)?;
                n.page().dom.borrow_mut().set_char_data(n.id, &t).js()
            },
        );
        def.getter("length", |s, n| Ok(s.number(n.page().dom.borrow().char_length(n.id).js()? as f64)));
        def.method("substringData", 2, |s, n, a| {
            a.require(2, "CharacterData.substringData")?;
            let (off, cnt) = (to_u32(a.get(0))?, to_u32(a.get(1))?);
            let r = n.page().dom.borrow().substring_data(n.id, off, cnt).js()?;
            Ok(s.string(&r))
        });
        def.method("appendData", 1, |s, n, a| {
            let d = arg_string(&a, 0, "CharacterData.appendData")?;
            let mut dom = n.page().dom.borrow_mut();
            let len = dom.char_length(n.id).js()?;
            dom.replace_data(n.id, len, 0, &d).js()?;
            Ok(s.undefined())
        });
        def.method("insertData", 2, |s, n, a| {
            a.require(2, "CharacterData.insertData")?;
            let d = dom_string(a.get(1))?;
            let arg0 = to_u32(a.get(0))?;
            n.page().dom.borrow_mut().replace_data(n.id, arg0, 0, &d).js()?;
            Ok(s.undefined())
        });
        def.method("deleteData", 2, |s, n, a| {
            a.require(2, "CharacterData.deleteData")?;
            let arg0 = to_u32(a.get(0))?;
            let arg1 = to_u32(a.get(1))?;
            n.page().dom.borrow_mut().replace_data(n.id, arg0, arg1, "").js()?;
            Ok(s.undefined())
        });
        def.method("replaceData", 3, |s, n, a| {
            a.require(3, "CharacterData.replaceData")?;
            let d = dom_string(a.get(2))?;
            let arg0 = to_u32(a.get(0))?;
            let arg1 = to_u32(a.get(1))?;
            n.page().dom.borrow_mut().replace_data(n.id, arg0, arg1, &d).js()?;
            Ok(s.undefined())
        });
    }
}

pub struct Text;
impl JsClass for Text {
    const NAME: &'static str = "Text";
    type Data = NodeData;
    type Parent = CharacterData;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(0, |s, nt, a| {
            let page = page_of(s)?;
            let data = if a.get(0).is_undefined() { String::new() } else { dom_string(a.get(0))? };
            let id = page.dom.borrow_mut().create_text(page.document(), &data).js()?;
            wrap_new_node::<Text>(s, &page, id, nt)
        });
        def.getter("assignedSlot", assigned_slot);
        def.method("splitText", 1, |s, n, a| {
            let off = to_u32(a.get(0))?;
            let t = n.page().dom.borrow_mut().split_text(n.id, off).js()?;
            Ok(wrap_node(s, n.page(), t)?.value())
        });
        def.getter("wholeText", |s, n| {
            let dom = n.page().dom.borrow();
            // Contiguous Text siblings.
            let mut start = n.id;
            while let Some(p) = dom.prev_sibling(start).js()? {
                if dom.node_type(p).js()? != NodeType::Text {
                    break;
                }
                start = p;
            }
            let mut out = String::new();
            let mut cur = Some(start);
            while let Some(c) = cur {
                if dom.node_type(c).js()? != NodeType::Text {
                    break;
                }
                out.push_str(dom.char_data(c).js()?);
                cur = dom.next_sibling(c).js()?;
            }
            Ok(s.string(&out))
        });
        def.getter("assignedSlot", assigned_slot);
        def.method("getHTML", 0, get_html);
        def.method("setHTMLUnsafe", 1, set_html_unsafe);
    }
}

pub struct CDataSection;
impl JsClass for CDataSection {
    const NAME: &'static str = "CDATASection";
    type Data = NodeData;
    type Parent = Text;
    fn define(_def: &mut ClassDef<Self>) {}
}

pub struct Comment;
impl JsClass for Comment {
    const NAME: &'static str = "Comment";
    type Data = NodeData;
    type Parent = CharacterData;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(0, |s, nt, a| {
            let page = page_of(s)?;
            let data = if a.get(0).is_undefined() { String::new() } else { dom_string(a.get(0))? };
            let id = page.dom.borrow_mut().create_comment(page.document(), &data).js()?;
            wrap_new_node::<Comment>(s, &page, id, nt)
        });
    }
}

pub struct ProcessingInstruction;
impl JsClass for ProcessingInstruction {
    const NAME: &'static str = "ProcessingInstruction";
    type Data = NodeData;
    type Parent = CharacterData;
    fn define(def: &mut ClassDef<Self>) {
        def.getter("target", |s, n| Ok(s.string(&n.page().dom.borrow().node_name(n.id).js()?)));
    }
}

pub struct DocumentType;
impl JsClass for DocumentType {
    const NAME: &'static str = "DocumentType";
    type Data = NodeData;
    type Parent = Node;
    fn define(def: &mut ClassDef<Self>) {
        define_child_node(def);
        def.getter("name", |s, n| doctype_field(s, n, 0));
        def.getter("publicId", |s, n| doctype_field(s, n, 1));
        def.getter("systemId", |s, n| doctype_field(s, n, 2));
    }
}

fn doctype_field<'s>(s: &Scope<'s>, n: &NodeData, which: u8) -> JsResult<Value<'s>> {
    let dom = n.page().dom.borrow();
    match &dom.node(n.id).js()?.kind {
        NodeKind::Doctype(d) => Ok(s.string(match which {
            0 => &d.name,
            1 => &d.public_id,
            _ => &d.system_id,
        })),
        _ => Err(JsError::Type("Illegal invocation".into())),
    }
}

/// Creates a wrapper for a freshly constructed node honoring `new.target`.
pub fn wrap_new_node<'s, C: JsClass<Data = NodeData>>(
    s: &Scope<'s>,
    page: &Rc<PageDom>,
    id: NodeId,
    new_target: Object<'s>,
) -> JsResult<Object<'s>> {
    page.dom.borrow_mut().add_ref(id).js()?;
    let data = NodeData { target: EventTargetData { page: page.clone(), key: TargetKey::Node(id.index()) }, id };
    s.wrap_new::<C>(data, new_target, Some(page.node_identity(id)))
}

// ----- fragments ------------------------------------------------------------

pub struct DocumentFragment;
impl JsClass for DocumentFragment {
    const NAME: &'static str = "DocumentFragment";
    type Data = NodeData;
    type Parent = Node;
    fn define(def: &mut ClassDef<Self>) {
        define_parent_node(def);
        def.constructor(0, |s, nt, _a| {
            let page = page_of(s)?;
            let id = page.dom.borrow_mut().create_fragment(page.document()).js()?;
            wrap_new_node::<DocumentFragment>(s, &page, id, nt)
        });
        def.method("getElementById", 1, |s, n, a| {
            let id = arg_string(&a, 0, "getElementById")?;
            let r = n.page().dom.borrow().get_element_by_id(n.id, &id).js()?;
            node_value(s, n.page(), r)
        });
    }
}

fn shadow_flag<'s>(s: &Scope<'s>, n: &NodeData, flag: u8) -> JsResult<Value<'s>> {
    Ok(s.boolean(n.page().dom.borrow().shadow_root_flags(n.id).js()? & flag != 0))
}

/// `getHTML(options)` on `Element` and `ShadowRoot`.
fn get_html<'s>(s: &Scope<'s>, n: &NodeData, a: Args<'s>) -> JsResult<Value<'s>> {
    let mut serializable = false;
    let mut roots = Vec::new();
    let options = a.get(0);
    if !options.is_undefined() && !options.is_null() {
        let o = options.as_object().ok_or_else(|| {
            JsError::Type("Failed to execute 'getHTML': The provided value is not of type 'GetHTMLOptions'.".into())
        })?;
        serializable = o.get("serializableShadowRoots")?.to_bool();
        let list = o.get("shadowRoots")?;
        if !list.is_undefined() {
            let list = list.as_object().ok_or_else(|| {
                JsError::Type(
                    "Failed to execute 'getHTML': The provided value cannot be converted to a sequence.".into(),
                )
            })?;
            let len = list.get("length")?.to_number()? as u32;
            for i in 0..len {
                let root = node_arg(s, list.get_index(i)?, "getHTML")?;
                if !n.page().dom.borrow().is_shadow_root(root).js()? {
                    return Err(JsError::Type(
                        "Failed to execute 'getHTML': The provided value is not of type 'ShadowRoot'.".into(),
                    ));
                }
                roots.push(root);
            }
        }
    }
    Ok(s.string(&n.page().dom.borrow().get_html(n.id, serializable, &roots).js()?))
}

/// `setHTMLUnsafe(html)` on `Element` and `ShadowRoot`.
fn set_html_unsafe<'s>(s: &Scope<'s>, n: &NodeData, a: Args<'s>) -> JsResult<Value<'s>> {
    a.require(1, "setHTMLUnsafe")?;
    let html = dom_string(a.get(0))?;
    crate::input::blur_before_removal(n.page(), n.id, true);
    lp_dom::parse::set_html_unsafe(&mut n.page().dom.borrow_mut(), n.id, &html).js()?;
    Ok(s.undefined())
}

pub struct ShadowRoot;
impl JsClass for ShadowRoot {
    const NAME: &'static str = "ShadowRoot";
    type Data = NodeData;
    type Parent = DocumentFragment;
    fn define(def: &mut ClassDef<Self>) {
        def.getter("host", |s, n| {
            let h = n.page().dom.borrow().fragment_host(n.id).js()?;
            node_value(s, n.page(), h)
        });
        def.getter("mode", |s, n| {
            let dom = n.page().dom.borrow();
            Ok(s.string(match &dom.node(n.id).js()?.kind {
                NodeKind::Fragment(_) if dom.is_shadow_root(n.id).js()? => {
                    let host = dom.fragment_host(n.id).js()?.unwrap();
                    if dom.shadow_root(host).js()? == Some(n.id) {
                        crate::html::shadow_mode(&dom, n.id)
                    } else {
                        "open"
                    }
                }
                _ => "open",
            }))
        });
        def.getter("delegatesFocus", |s, n| shadow_flag(s, n, lp_dom::shadow_flags::DELEGATES_FOCUS));
        def.getter("clonable", |s, n| shadow_flag(s, n, lp_dom::shadow_flags::CLONABLE));
        def.getter("serializable", |s, n| shadow_flag(s, n, lp_dom::shadow_flags::SERIALIZABLE));
        def.getter("slotAssignment", |s, n| {
            let flags = n.page().dom.borrow().shadow_root_flags(n.id).js()?;
            Ok(s.string(if flags & lp_dom::shadow_flags::MANUAL_SLOTS != 0 { "manual" } else { "named" }))
        });
        def.method("getHTML", 0, get_html);
        def.method("setHTMLUnsafe", 1, set_html_unsafe);
        def.getter("activeElement", |s, _n| Ok(s.null()));
        def.accessor(
            "innerHTML",
            |s, n| Ok(s.string(&n.page().dom.borrow().inner_html(n.id).js()?)),
            |_s, n, v| {
                let html = null_to_empty(v)?;
                let mut dom = n.page().dom.borrow_mut();
                let host = dom.fragment_host(n.id).js()?.ok_or(JsError::Type("detached shadow root".into()))?;
                let frag = lp_dom::parse::parse_fragment(&mut dom, host, &html).js()?;
                dom.replace_all(n.id, Some(frag)).js()
            },
        );
    }
}

// ----- element ----------------------------------------------------------

pub struct Element;
impl JsClass for Element {
    const NAME: &'static str = "Element";
    type Data = NodeData;
    type Parent = Node;
    fn define(def: &mut ClassDef<Self>) {
        define_parent_node(def);
        define_child_node(def);
        define_non_doctype_child(def);
        def.getter("tagName", |s, n| Ok(s.string(&n.page().dom.borrow().tag_name(n.id).js()?)));
        def.getter("localName", |s, n| Ok(s.string(n.page().dom.borrow().element(n.id).js()?.local_name())));
        def.getter("namespaceURI", |s, n| {
            let dom = n.page().dom.borrow();
            let ns = dom.element(n.id).js()?.namespace();
            Ok(if ns.is_empty() { s.null() } else { s.string(ns) })
        });
        def.getter("prefix", |s, n| {
            let dom = n.page().dom.borrow();
            Ok(match dom.element(n.id).js()?.prefix() {
                Some(p) if !p.is_empty() => s.string(p),
                _ => s.null(),
            })
        });
        def.accessor(
            "id",
            |s, n| Ok(s.string(n.page().dom.borrow().get_attribute(n.id, "id").js()?.unwrap_or(""))),
            |_s, n, v| {
                let arg0 = dom_string(v)?;
                n.page().dom.borrow_mut().set_attribute(n.id, "id", &arg0).js()
            },
        );
        def.accessor(
            "className",
            |s, n| Ok(s.string(n.page().dom.borrow().get_attribute(n.id, "class").js()?.unwrap_or(""))),
            |_s, n, v| {
                let arg0 = dom_string(v)?;
                n.page().dom.borrow_mut().set_attribute(n.id, "class", &arg0).js()
            },
        );
        def.accessor(
            "slot",
            |s, n| Ok(s.string(n.page().dom.borrow().get_attribute(n.id, "slot").js()?.unwrap_or(""))),
            |_s, n, v| {
                let arg0 = dom_string(v)?;
                n.page().dom.borrow_mut().set_attribute(n.id, "slot", &arg0).js()
            },
        );
        def.getter("classList", |s, n| Ok(collections::token_list(s, n.page(), n.id, "class")?.value()));
        def.getter("part", |s, n| Ok(collections::token_list(s, n.page(), n.id, "part")?.value()));
        def.getter("attributes", |s, n| Ok(collections::named_node_map(s, n.page(), n.id)?.value()));
        def.method("hasAttributes", 0, |s, n, _a| {
            Ok(s.boolean(!n.page().dom.borrow().attributes(n.id).js()?.is_empty()))
        });
        def.method("getAttributeNames", 0, |s, n, _a| {
            let names: Vec<String> =
                n.page().dom.borrow().attributes(n.id).js()?.iter().map(|a| a.qualified_name()).collect();
            let mut vals = lp_js::RootedVec::new();
            for name in names {
                vals.push(s, s.string(&name));
            }
            Ok(s.array(&vals.iter(s).collect::<Vec<_>>())?.value())
        });
        def.method("getAttribute", 1, |s, n, a| {
            let name = arg_string(&a, 0, "Element.getAttribute")?;
            let dom = n.page().dom.borrow();
            Ok(dom.get_attribute(n.id, &name).js()?.map_or(s.null(), |v| s.string(v)))
        });
        def.method("getAttributeNS", 2, |s, n, a| {
            a.require(2, "Element.getAttributeNS")?;
            let ns = opt_dom_string(a.get(0))?;
            let local = dom_string(a.get(1))?;
            let dom = n.page().dom.borrow();
            Ok(dom.get_attribute_ns(n.id, ns.as_deref(), &local).js()?.map_or(s.null(), |v| s.string(v)))
        });
        def.method("setAttribute", 2, |s, n, a| {
            a.require(2, "Element.setAttribute")?;
            let name = dom_string(a.get(0))?;
            let value = dom_string(a.get(1))?;
            n.page().dom.borrow_mut().set_attribute(n.id, &name, &value).js()?;
            handler_attribute_changed(s, n, &name, Some(&value))?;
            crate::input::blur_if_focus_disabled(n.page(), n.id, &name);
            Ok(s.undefined())
        });
        def.method("setAttributeNS", 3, |s, n, a| {
            a.require(3, "Element.setAttributeNS")?;
            let ns = opt_dom_string(a.get(0))?;
            let name = dom_string(a.get(1))?;
            let value = dom_string(a.get(2))?;
            n.page().dom.borrow_mut().set_attribute_ns(n.id, ns.as_deref(), &name, &value).js()?;
            if ns.is_none() {
                crate::input::blur_if_focus_disabled(n.page(), n.id, &name);
            }
            Ok(s.undefined())
        });
        def.method("removeAttribute", 1, |s, n, a| {
            let name = arg_string(&a, 0, "Element.removeAttribute")?;
            let removed = n.page().dom.borrow_mut().remove_attribute(n.id, &name).js()?;
            if removed {
                handler_attribute_changed(s, n, &name, None)?;
            }
            Ok(s.undefined())
        });
        def.method("removeAttributeNS", 2, |s, n, a| {
            a.require(2, "Element.removeAttributeNS")?;
            let ns = opt_dom_string(a.get(0))?;
            let local = dom_string(a.get(1))?;
            n.page().dom.borrow_mut().remove_attribute_ns(n.id, ns.as_deref(), &local).js()?;
            Ok(s.undefined())
        });
        def.method("hasAttribute", 1, |s, n, a| {
            let name = arg_string(&a, 0, "Element.hasAttribute")?;
            Ok(s.boolean(n.page().dom.borrow().has_attribute(n.id, &name).js()?))
        });
        def.method("hasAttributeNS", 2, |s, n, a| {
            let ns = opt_dom_string(a.get(0))?;
            let local = dom_string(a.get(1))?;
            Ok(s.boolean(n.page().dom.borrow().get_attribute_ns(n.id, ns.as_deref(), &local).js()?.is_some()))
        });
        def.method("toggleAttribute", 1, |s, n, a| {
            let name = arg_string(&a, 0, "Element.toggleAttribute")?;
            let force = a.opt(1).filter(|v| !v.is_undefined()).map(|v| v.to_bool());
            let present = n.page().dom.borrow_mut().toggle_attribute(n.id, &name, force).js()?;
            crate::input::blur_if_focus_disabled(n.page(), n.id, &name);
            Ok(s.boolean(present))
        });
        def.method("matches", 1, element_matches);
        def.method("webkitMatchesSelector", 1, element_matches);
        def.method("closest", 1, |s, n, a| {
            let page = n.page();
            let list = arg_selector(&a, 0, "Element.closest", page)?;
            let r = page.dom.borrow().element_closest(n.id, &list).js()?;
            node_value(s, page, r)
        });
        def.method("getElementsByTagName", 1, |s, n, a| {
            let name = arg_string(&a, 0, "getElementsByTagName")?;
            Ok(collections::collection(s, n.page(), n.id, CollectionKind::TagName(name))?.value())
        });
        def.method("getElementsByTagNameNS", 2, |s, n, a| {
            a.require(2, "getElementsByTagNameNS")?;
            let namespace = opt_dom_string(a.get(0))?.unwrap_or_default();
            let local = dom_string(a.get(1))?;
            Ok(collections::collection(s, n.page(), n.id, CollectionKind::TagNameNs(namespace, local))?.value())
        });
        def.method("getElementsByClassName", 1, |s, n, a| {
            let names = arg_string(&a, 0, "getElementsByClassName")?;
            Ok(collections::collection(s, n.page(), n.id, CollectionKind::ClassName(names))?.value())
        });
        def.accessor(
            "innerHTML",
            |s, n| Ok(s.string(&n.page().dom.borrow().inner_html(n.id).js()?)),
            |_s, n, v| {
                let html = null_to_empty(v)?;
                crate::input::blur_before_removal(n.page(), n.id, true);
                lp_dom::parse::set_inner_html(&mut n.page().dom.borrow_mut(), n.id, &html).js()
            },
        );
        def.accessor(
            "outerHTML",
            |s, n| Ok(s.string(&n.page().dom.borrow().outer_html(n.id).js()?)),
            |_s, n, v| {
                let html = null_to_empty(v)?;
                let has_parent = n.page().dom.borrow().parent(n.id).js()?.is_some();
                if has_parent {
                    crate::input::blur_before_removal(n.page(), n.id, false);
                }
                let mut dom = n.page().dom.borrow_mut();
                let Some(parent) = dom.parent(n.id).js()? else { return Ok(()) };
                if dom.node_type(parent).js()? == NodeType::Document {
                    return Err(JsError::dom(
                        "NoModificationAllowedError",
                        "Cannot set outerHTML on the document element.",
                    ));
                }
                let ctx = if dom.node_type(parent).js()? == NodeType::Element {
                    parent
                } else {
                    let doc = dom.node_document(n.id).js()?;
                    dom.create_element(doc, "body").js()?
                };
                let frag = lp_dom::parse::parse_fragment(&mut dom, ctx, &html).js()?;
                dom.replace_child(parent, frag, n.id).js()?;
                Ok(())
            },
        );
        def.method("insertAdjacentHTML", 2, |s, n, a| {
            a.require(2, "Element.insertAdjacentHTML")?;
            let pos = dom_string(a.get(0))?.to_ascii_lowercase();
            let html = dom_string(a.get(1))?;
            let mut dom = n.page().dom.borrow_mut();
            let parent = dom.parent(n.id).js()?;
            let ctx = match pos.as_str() {
                "beforebegin" | "afterend" => match parent {
                    Some(p) if dom.node_type(p).js()? == NodeType::Element => p,
                    _ => return Err(JsError::dom("NoModificationAllowedError", "The element has no parent.")),
                },
                "afterbegin" | "beforeend" => n.id,
                _ => {
                    return Err(JsError::dom(
                        "SyntaxError",
                        "The value provided is not one of 'beforeBegin', 'afterBegin', 'beforeEnd', or 'afterEnd'.",
                    ))
                }
            };
            let frag = lp_dom::parse::parse_fragment(&mut dom, ctx, &html).js()?;
            insert_adjacent(&mut dom, n.id, &pos, frag)?;
            Ok(s.undefined())
        });
        def.method("insertAdjacentElement", 2, |s, n, a| {
            a.require(2, "Element.insertAdjacentElement")?;
            let pos = dom_string(a.get(0))?.to_ascii_lowercase();
            let el = node_arg(s, a.get(1), "insertAdjacentElement")?;
            let ok = insert_adjacent(&mut n.page().dom.borrow_mut(), n.id, &pos, el)?;
            Ok(if ok { a.get(1) } else { s.null() })
        });
        def.method("insertAdjacentText", 2, |s, n, a| {
            a.require(2, "Element.insertAdjacentText")?;
            let pos = dom_string(a.get(0))?.to_ascii_lowercase();
            let text = dom_string(a.get(1))?;
            let mut dom = n.page().dom.borrow_mut();
            let doc = dom.node_document(n.id).js()?;
            let t = dom.create_text(doc, &text).js()?;
            insert_adjacent(&mut dom, n.id, &pos, t)?;
            Ok(s.undefined())
        });
        def.method("attachShadow", 1, |s, n, a| {
            let Some(o) = a.get(0).as_object() else {
                return Err(JsError::Type(
                    "Failed to execute 'attachShadow' on 'Element': 1 argument required, but only 0 present.".into(),
                ));
            };
            // Dictionary members convert in lexicographical order.
            let mut flags = 0;
            if o.get("clonable")?.to_bool() {
                flags |= lp_dom::shadow_flags::CLONABLE;
            }
            if o.get("delegatesFocus")?.to_bool() {
                flags |= lp_dom::shadow_flags::DELEGATES_FOCUS;
            }
            let mode = dom_string(o.get("mode")?)?;
            let mode = match mode.as_str() {
                "open" => ShadowRootMode::Open,
                "closed" => ShadowRootMode::Closed,
                _ => {
                    return Err(JsError::Type(format!(
                        "The provided value '{mode}' is not a valid enum value of type ShadowRootMode."
                    )))
                }
            };
            if o.get("serializable")?.to_bool() {
                flags |= lp_dom::shadow_flags::SERIALIZABLE;
            }
            let slots = o.get("slotAssignment")?;
            if !slots.is_undefined() {
                match dom_string(slots)?.as_str() {
                    "named" => {}
                    "manual" => flags |= lp_dom::shadow_flags::MANUAL_SLOTS,
                    other => {
                        return Err(JsError::Type(format!(
                            "The provided value '{other}' is not a valid enum value of type SlotAssignmentMode."
                        )))
                    }
                }
            }
            let sr = n.page().dom.borrow_mut().attach_shadow_with(n.id, mode, flags).js()?;
            Ok(wrap_node(s, n.page(), sr)?.value())
        });
        def.getter("shadowRoot", |s, n| {
            let dom = n.page().dom.borrow();
            let sr = dom.shadow_root(n.id).js()?;
            let open = sr.is_some_and(|r| crate::html::shadow_mode(&dom, r) == "open");
            drop(dom);
            node_value(s, n.page(), if open { sr } else { None })
        });
        def.getter("assignedSlot", assigned_slot);
        def.method("getHTML", 0, get_html);
        def.method("setHTMLUnsafe", 1, set_html_unsafe);
        crate::html::define_element_extras(def);
    }
}

fn assigned_slot<'s>(s: &Scope<'s>, n: &NodeData) -> JsResult<Value<'s>> {
    let slot = n.page().dom.borrow().assigned_slot(n.id).js()?;
    node_value(s, n.page(), slot)
}

fn element_matches<'s>(s: &Scope<'s>, n: &NodeData, a: Args<'s>) -> JsResult<Value<'s>> {
    let page = n.page();
    let list = arg_selector(&a, 0, "Element.matches", page)?;
    Ok(s.boolean(page.dom.borrow().element_matches(n.id, &list).js()?))
}

fn insert_adjacent(dom: &mut lp_dom::Dom, el: NodeId, pos: &str, node: NodeId) -> JsResult<bool> {
    match pos {
        "beforebegin" => match dom.parent(el).js()? {
            Some(p) => dom.insert_before(p, node, Some(el)).map(|_| true).js(),
            None => Ok(false),
        },
        "afterbegin" => {
            let first = dom.first_child(el).js()?;
            dom.insert_before(el, node, first).map(|_| true).js()
        }
        "beforeend" => dom.append_child(el, node).map(|_| true).js(),
        "afterend" => match dom.parent(el).js()? {
            Some(p) => {
                let next = dom.next_sibling(el).js()?;
                dom.insert_before(p, node, next).map(|_| true).js()
            }
            None => Ok(false),
        },
        _ => Err(JsError::dom(
            "SyntaxError",
            "The value provided is not one of 'beforeBegin', 'afterBegin', 'beforeEnd', or 'afterEnd'.",
        )),
    }
}

// ----- document -------------------------------------------------------------

pub struct Document;
impl JsClass for Document {
    const NAME: &'static str = "Document";
    type Data = NodeData;
    type Parent = Node;
    fn define(def: &mut ClassDef<Self>) {
        define_parent_node(def);
        crate::handlers::define_global(def);
        crate::handlers::define_document(def);
        crate::html::define_document_reflection(def);
        def.constructor(0, |s, nt, _a| {
            let page = page_of(s)?;
            let id = page.dom.borrow_mut().create_document(DocumentKind::Xml, "about:blank");
            wrap_new_node::<Document>(s, &page, id, nt)
        });
        def.static_method("parseHTMLUnsafe", 1, |s, a| {
            a.require(1, "Document.parseHTMLUnsafe")?;
            let html = dom_string(a.get(0))?;
            let page = page_of(s)?;
            let doc = lp_dom::parse::parse_html_unsafe(&page.dom, &html);
            Ok(wrap_node(s, &page, doc)?.value())
        });
        def.getter("documentElement", |s, n| {
            let e = n.page().dom.borrow().document_element(n.id).js()?;
            node_value(s, n.page(), e)
        });
        def.getter("doctype", |s, n| {
            let e = n.page().dom.borrow().doctype(n.id).js()?;
            node_value(s, n.page(), e)
        });
        def.getter("head", |s, n| {
            let e = n.page().dom.borrow().head(n.id).js()?;
            node_value(s, n.page(), e)
        });
        def.accessor(
            "body",
            |s, n| {
                let e = n.page().dom.borrow().body(n.id).js()?;
                node_value(s, n.page(), e)
            },
            |s, n, v| {
                let new = node_arg(s, v, "body")?;
                let mut dom = n.page().dom.borrow_mut();
                let html = dom
                    .document_element(n.id)
                    .js()?
                    .ok_or_else(|| JsError::dom("HierarchyRequestError", "No document element exists."))?;
                match dom.body(n.id).js()? {
                    Some(old) => dom.replace_child(html, new, old).map(|_| ()).js(),
                    None => dom.append_child(html, new).map(|_| ()).js(),
                }
            },
        );
        def.accessor(
            "title",
            |s, n| {
                let dom = n.page().dom.borrow();
                let sel = lp_dom::select::parse_selectors("title").js()?;
                let t = dom.query_selector(n.id, &sel).js()?;
                let text = match t {
                    Some(t) => dom.text_content(t).js()?.unwrap_or_default(),
                    None => String::new(),
                };
                Ok(s.string(&text.split_ascii_whitespace().collect::<Vec<_>>().join(" ")))
            },
            |_s, n, v| {
                let title = dom_string(v)?;
                let mut dom = n.page().dom.borrow_mut();
                let sel = lp_dom::select::parse_selectors("title").js()?;
                let t = match dom.query_selector(n.id, &sel).js()? {
                    Some(t) => t,
                    None => {
                        let Some(head) = dom.head(n.id).js()? else { return Ok(()) };
                        let t = dom.create_element(n.id, "title").js()?;
                        dom.append_child(head, t).js()?;
                        t
                    }
                };
                dom.set_text_content(t, &title).js()
            },
        );
        def.getter("URL", |s, n| Ok(s.string(&n.page().dom.borrow().document_data(n.id).js()?.url)));
        def.getter("documentURI", |s, n| Ok(s.string(&n.page().dom.borrow().document_data(n.id).js()?.url)));
        def.getter("compatMode", |s, n| {
            let q = n.page().dom.borrow().document_data(n.id).js()?.quirks;
            Ok(s.string(if q == lp_dom::QuirksMode::Quirks { "BackCompat" } else { "CSS1Compat" }))
        });
        def.getter("characterSet", |s, n| Ok(s.string(n.page().dom.borrow().document_data(n.id).js()?.charset)));
        def.getter("charset", |s, n| Ok(s.string(n.page().dom.borrow().document_data(n.id).js()?.charset)));
        def.getter("inputEncoding", |s, n| Ok(s.string(n.page().dom.borrow().document_data(n.id).js()?.charset)));
        def.getter("contentType", |s, n| Ok(s.string(&n.page().dom.borrow().document_data(n.id).js()?.content_type)));
        def.getter("readyState", |s, n| Ok(s.string(crate::html::ready_state(n.page(), n.id))));
        def.getter("visibilityState", |s, _n| Ok(s.string("visible")));
        def.getter("hidden", |s, _n| Ok(s.boolean(false)));
        def.getter("defaultView", |s, n| Ok(if n.page().document() == n.id { s.global().value() } else { s.null() }));
        def.getter("activeElement", |s, n| {
            let dom = n.page().dom.borrow();
            let f = dom.focused(n.id).js()?.or(dom.body(n.id).js()?);
            drop(dom);
            node_value(s, n.page(), f)
        });
        def.method("hasFocus", 0, |s, n, _a| Ok(s.boolean(n.page().document() == n.id)));
        def.getter("currentScript", |s, n| node_value(s, n.page(), n.page().current_script()));
        def.getter("implementation", |s, n| Ok(crate::html::dom_implementation(s, n)?.value()));
        def.getter("forms", |s, n| {
            Ok(collections::collection(s, n.page(), n.id, CollectionKind::TagName("form".into()))?.value())
        });
        def.getter("images", |s, n| {
            Ok(collections::collection(s, n.page(), n.id, CollectionKind::TagName("img".into()))?.value())
        });
        def.getter("scripts", |s, n| {
            Ok(collections::collection(s, n.page(), n.id, CollectionKind::TagName("script".into()))?.value())
        });
        def.getter("links", |s, n| Ok(collections::collection(s, n.page(), n.id, CollectionKind::Links)?.value()));
        def.accessor(
            "cookie",
            |s, n| {
                if s.realm().origin().as_deref() == Some("null") {
                    return Err(JsError::dom("SecurityError", "Cookies are unavailable for an opaque origin."));
                }
                Ok(s.string(&crate::html::document_cookie(n.page(), n.id)))
            },
            |s, n, v| {
                if s.realm().origin().as_deref() == Some("null") {
                    return Err(JsError::dom("SecurityError", "Cookies are unavailable for an opaque origin."));
                }
                crate::html::set_document_cookie(n.page(), n.id, &dom_string(v)?);
                Ok(())
            },
        );
        def.getter("referrer", |s, _n| Ok(s.string("")));
        def.getter("domain", |s, n| {
            if s.realm().origin().as_deref() == Some("null") {
                return Ok(s.string(""));
            }
            let url = n.page().dom.borrow().document_data(n.id).js()?.url.to_string();
            Ok(s.string(
                lp_net::Url::parse(&url).ok().and_then(|u| u.host_str().map(str::to_owned)).as_deref().unwrap_or(""),
            ))
        });
        def.getter("location", |s, n| {
            Ok(if n.page().document() == n.id { s.global().get("location")? } else { s.null() })
        });
        def.method("getElementById", 1, |s, n, a| {
            let id = arg_string(&a, 0, "getElementById")?;
            let r = n.page().dom.borrow().get_element_by_id(n.id, &id).js()?;
            node_value(s, n.page(), r)
        });
        def.method("getElementsByTagName", 1, |s, n, a| {
            let name = arg_string(&a, 0, "getElementsByTagName")?;
            Ok(collections::collection(s, n.page(), n.id, CollectionKind::TagName(name))?.value())
        });
        def.method("getElementsByTagNameNS", 2, |s, n, a| {
            a.require(2, "getElementsByTagNameNS")?;
            let namespace = opt_dom_string(a.get(0))?.unwrap_or_default();
            let local = dom_string(a.get(1))?;
            Ok(collections::collection(s, n.page(), n.id, CollectionKind::TagNameNs(namespace, local))?.value())
        });
        def.method("getElementsByClassName", 1, |s, n, a| {
            let names = arg_string(&a, 0, "getElementsByClassName")?;
            Ok(collections::collection(s, n.page(), n.id, CollectionKind::ClassName(names))?.value())
        });
        def.method("getElementsByName", 1, |s, n, a| {
            let name = arg_string(&a, 0, "getElementsByName")?;
            Ok(collections::collection(s, n.page(), n.id, CollectionKind::Name(name))?.value())
        });
        def.method("createElement", 1, |s, n, a| {
            let name = arg_string(&a, 0, "Document.createElement")?;
            let el = n.page().dom.borrow_mut().create_element(n.id, &name).js()?;
            if let Some(is) = a.get(1).as_object().map(|o| o.get("is")).transpose()?.filter(|v| !v.is_undefined()) {
                let arg0 = dom_string(is)?;
                n.page().dom.borrow_mut().set_attribute(el, "is", &arg0).js()?;
            }
            crate::html::upgrade_if_custom(s, n.page(), el)
        });
        def.method("createElementNS", 2, |s, n, a| {
            a.require(2, "Document.createElementNS")?;
            let ns = opt_dom_string(a.get(0))?;
            let q = dom_string(a.get(1))?;
            let el = n.page().dom.borrow_mut().create_element_ns(n.id, ns.as_deref(), &q).js()?;
            crate::html::upgrade_if_custom(s, n.page(), el)
        });
        def.method("createTextNode", 1, |s, n, a| {
            let d = arg_string(&a, 0, "Document.createTextNode")?;
            let t = n.page().dom.borrow_mut().create_text(n.id, &d).js()?;
            Ok(wrap_node(s, n.page(), t)?.value())
        });
        def.method("createComment", 1, |s, n, a| {
            let d = arg_string(&a, 0, "Document.createComment")?;
            let t = n.page().dom.borrow_mut().create_comment(n.id, &d).js()?;
            Ok(wrap_node(s, n.page(), t)?.value())
        });
        def.method("createCDATASection", 1, |s, n, a| {
            let d = arg_string(&a, 0, "Document.createCDATASection")?;
            let t = n.page().dom.borrow_mut().create_cdata(n.id, &d).js()?;
            Ok(wrap_node(s, n.page(), t)?.value())
        });
        def.method("createProcessingInstruction", 2, |s, n, a| {
            a.require(2, "Document.createProcessingInstruction")?;
            let t = dom_string(a.get(0))?;
            let d = dom_string(a.get(1))?;
            let pi = n.page().dom.borrow_mut().create_processing_instruction(n.id, &t, &d).js()?;
            Ok(wrap_node(s, n.page(), pi)?.value())
        });
        def.method("createDocumentFragment", 0, |s, n, _a| {
            let f = n.page().dom.borrow_mut().create_fragment(n.id).js()?;
            Ok(wrap_node(s, n.page(), f)?.value())
        });
        def.method("createEvent", 1, |s, n, a| {
            let iface = arg_string(&a, 0, "Document.createEvent")?;
            crate::html::create_event(s, n.page(), &iface)
        });
        def.method("createRange", 0, |s, n, _a| crate::html::create_range(s, n));
        def.method("createTreeWalker", 1, crate::traversal::create_tree_walker);
        def.method("createNodeIterator", 1, crate::traversal::create_node_iterator);
        def.method("evaluate", 2, crate::xpath::document_evaluate);
        def.method("createExpression", 1, crate::xpath::document_create_expression);
        def.method("createNSResolver", 1, crate::xpath::document_create_ns_resolver);
        def.method("importNode", 1, |s, n, a| {
            let src = node_arg(s, a.get(0), "importNode")?;
            let deep = match a.get(1).as_object() {
                Some(o) => !o.get("selfOnly")?.to_bool(),
                None => a.get(1).to_bool(),
            };
            let c = n.page().dom.borrow_mut().clone_node(src, deep, Some(n.id)).js()?;
            Ok(wrap_node(s, n.page(), c)?.value())
        });
        def.method("adoptNode", 1, |s, n, a| {
            let src = node_arg(s, a.get(0), "adoptNode")?;
            n.page().dom.borrow_mut().adopt_node(n.id, src).js()?;
            Ok(a.get(0))
        });
        def.method("elementFromPoint", 2, crate::html::element_from_point);
        def.method("elementsFromPoint", 2, crate::html::elements_from_point);
        def.method("getSelection", 0, |s, _n, _a| {
            s.global().get("getSelection")?.as_object().map_or(Ok(s.null()), |f| f.call(None, &[]))
        });
        def.method("execCommand", 0, |s, _n, _a| Ok(s.boolean(false)));
        def.method("queryCommandSupported", 0, |s, _n, _a| Ok(s.boolean(false)));
        def.method("open", 0, crate::html::document_open);
        def.method("close", 0, |s, n, _a| crate::html::document_close(s, n));
        def.method("write", 0, |s, n, a| crate::html::document_write(s, n, a, false));
        def.method("writeln", 0, |s, n, a| crate::html::document_write(s, n, a, true));
        def.getter("fonts", |s, _n| s.eval("({ready: Promise.resolve(), check(){return true}, load(){return Promise.resolve([])}, addEventListener(){}, removeEventListener(){}, status: 'loaded', forEach(){}, size: 0})", None, 1));
    }
}

pub struct HtmlDocument;
impl JsClass for HtmlDocument {
    const NAME: &'static str = "HTMLDocument";
    type Data = NodeData;
    type Parent = Document;
    fn define(_def: &mut ClassDef<Self>) {}
}

pub struct XmlDocument;
impl JsClass for XmlDocument {
    const NAME: &'static str = "XMLDocument";
    type Data = NodeData;
    type Parent = Document;
    fn define(_def: &mut ClassDef<Self>) {}
}

pub(crate) fn register(engine: &lp_js::Engine) {
    engine.register::<Node>();
    engine.register::<CharacterData>();
    engine.register::<Text>();
    engine.register::<CDataSection>();
    engine.register::<Comment>();
    engine.register::<ProcessingInstruction>();
    engine.register::<DocumentType>();
    engine.register::<DocumentFragment>();
    engine.register::<ShadowRoot>();
    engine.register::<Element>();
    engine.register::<Document>();
    engine.register::<HtmlDocument>();
    engine.register::<XmlDocument>();
}
