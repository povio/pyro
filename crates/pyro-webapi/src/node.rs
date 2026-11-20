//! Node, CharacterData, Element, Document, DocumentFragment bindings.
//!
//! Every node wrapper holds one external reference on its node (released
//! when the wrapper is finalized), so detached subtrees reachable from JS
//! stay alive and unreachable ones are collected by `Dom::collect_garbage`.

use std::rc::Rc;

use pyro_dom::events::TargetKey;
use pyro_dom::{DocumentKind, NodeId, NodeKind, NodeType, ShadowRootMode};
use pyro_js::{Args, ClassDef, JsClass, JsError, JsResult, Object, Scope, Value};

use crate::collections::{self, CollectionKind};
use crate::names::NameArg;
use crate::page::PageDom;
use crate::target::{page_of, EventTarget, EventTargetData};
use crate::util::*;

/// Payload of every Node wrapper.
///
/// # What a wrapper holds on its node
///
/// The payload is the whole path from the JavaScript object to the node: the
/// object's private data is this payload, `target.page` is the page whose
/// store the node lives in, and `id` is the node's slot in that store. No
/// map is consulted and nothing is looked up by key.
///
/// The invariant that makes this sound whatever goes away first:
///
/// - **The store outlives the wrapper.** `target.page` is an `Rc`, and the
///   page owns its `Dom`. A closed page (`PageDom::dispose`) drops its roots
///   and hooks, not the store: a wrapper another realm kept still reads a
///   node that is there.
/// - **The slot is this node's for as long as the wrapper exists.** A wrapper
///   holds one reference on its node (`Dom::add_ref` in `wrap_node`, released
///   in `Drop` below). `Dom::collect_garbage` frees a detached tree only when
///   no node in it is referenced, and a slot is given to another node only
///   after it was freed. So the node is not freed by removal from its
///   document, by its document being dropped, or by adoption into another
///   document (which changes the node's owner, never its slot). The
///   generation in `id` is checked on access and is never what keeps an
///   access right: a mismatch would be a broken reference count, and it
///   surfaces as a `StaleNode` error, not as another node's data.
/// - **The realm is not reached through the node.** A call into a wrapper
///   whose realm is disposed fails in the binding layer (`Header::core`),
///   before this payload is read.
///
/// Tests: `a_wrapper_keeps_its_removed_node_through_collections_and_slot_reuse`,
/// `a_wrapper_follows_its_node_into_another_document_and_back` (`dom_js.rs`),
/// `a_node_wrapper_kept_after_its_page_closed_stays_sound`
/// (`payload_cycles.rs`).
pub struct NodeData {
    pub target: EventTargetData,
    pub id: NodeId,
    /// The wrapper reference the page holds in this node's slot
    /// (`PageDom::node_wrappers`), raw; zero when the wrapper is in the
    /// realm's identity map instead.
    pub(crate) wrapper: std::cell::Cell<usize>,
    /// The realm's strings for the node's `id` and one-token `class`
    /// values, by the atom each mirrors (see [`AtomString`]).
    pub(crate) id_string: AtomString,
    pub(crate) class_string: AtomString,
}

impl NodeData {
    pub fn page(&self) -> &Rc<PageDom> {
        &self.target.page
    }
}

impl Drop for NodeData {
    fn drop(&mut self) {
        let wrapper = self.wrapper.get();
        if wrapper != 0 {
            self.target.page.forget_wrapper(self.id.index(), wrapper);
        }
        self.target.page.release_node(self.id);
    }
}

/// Wrapper of `id` for script in `s`'s realm: one per node per script world. A
/// frame tree shares one `Dom`, so a node that moved between its documents may
/// already have a wrapper in a sibling realm. A new one is made in the realm of
/// the document the node was created in (DOM "create a node": a node keeps
/// that realm across adoption), or in `s`'s realm when that document has none.
pub fn wrap_node<'s>(s: &Scope<'s>, page: &PageDom, id: NodeId) -> JsResult<Object<'s>> {
    if let Some(o) = page.node_wrapper(s, id) {
        return Ok(o);
    }
    let page_rc = page_of(s)?;
    if page_rc.id() != page.id() {
        if !Rc::ptr_eq(&page_rc.dom, &page.dom) {
            return Err(JsError::Type("node belongs to another page".into()));
        }
        return wrap_node(s, &page_rc, id);
    }
    if let Some(o) = page.sibling_wrapper(s, id)? {
        return Ok(o);
    }
    let (ty, class, connected) = {
        let mut dom = page.dom.borrow_mut();
        dom.add_ref(id).js()?;
        let ty = dom.node(id).js()?.kind.node_type();
        let class = crate::html::class_for(&dom, id);
        let connected = dom.is_in_active_document(id);
        if connected {
            dom.mark_pin_on_connect(id);
        }
        (ty, class, connected)
    };
    make_node_wrapper(s, page, page_rc, id, ty, class, connected)
}

/// [`wrap_node`] for a node the calling DOM operation just made in `doc`
/// (`createElement`, `createTextNode`): no world has a wrapper of it yet,
/// and when `doc` is the document of `page`, the page of `s`, the node
/// belongs to `s`'s realm. Then the searches for an existing wrapper, in
/// this world and its siblings, and for the node's home realm are skipped.
pub fn wrap_created_node<'s>(s: &Scope<'s>, page: &Rc<PageDom>, doc: NodeId, id: NodeId) -> JsResult<Object<'s>> {
    let page_rc = page_of(s)?;
    if doc != page.document() || page_rc.id() != page.id() {
        return wrap_node(s, page, id);
    }
    let (ty, class) = {
        let mut dom = page.dom.borrow_mut();
        dom.add_ref(id).js()?;
        let ty = dom.node(id).js()?.kind.node_type();
        (ty, crate::html::class_for(&dom, id))
    };
    // Not in a document yet: nothing to pin.
    make_node_wrapper(s, page, page_rc, id, ty, class, false)
}

/// The wrapper of `id` in `s`'s realm, which has none: `page` holds the
/// node's new reference already.
fn make_node_wrapper<'s>(
    s: &Scope<'s>,
    page: &PageDom,
    page_rc: Rc<PageDom>,
    id: NodeId,
    ty: NodeType,
    class: crate::html::ElClass,
    connected: bool,
) -> JsResult<Object<'s>> {
    let data = NodeData {
        target: EventTargetData::for_key(page_rc, TargetKey::Node(id.index())),
        id,
        wrapper: std::cell::Cell::new(0),
        id_string: AtomString::default(),
        class_string: AtomString::default(),
    };
    // The node keeps its wrapper (`PageDom::node_wrappers`) when it has a
    // slot for this world; otherwise the realm's identity map does.
    let owned = s.owned_wrappers() && page.wrapper_slot_free(s.realm_id(), id.index());
    let k = Some(match (owned, connected) {
        (false, _) => page.node_identity(id),
        (true, true) => pyro_js::IdentityKey::OWNED_KEPT,
        (true, false) => pyro_js::IdentityKey::OWNED,
    });
    let wrapper = wrap_node_kind(s, page, id, ty, class, data, k)?;
    // A wrapper of a node in the active document lives as long as the node
    // is connected, as in other browsers (expandos survive, and a selector
    // engine that touches every element does not make the garbage collector
    // drop and recreate thousands of wrappers per query). `reconcile_pins`
    // stops keeping it when the node leaves the document.
    register_node_wrapper(s, page, id, wrapper, owned, connected);
    Ok(wrapper)
}

/// Records a new node wrapper where it will be found again: in the node's
/// slot (`owned`, made with `IdentityKey::OWNED*`), or pinned if the node is
/// connected and the wrapper is in the identity map.
fn register_node_wrapper(s: &Scope<'_>, page: &PageDom, id: NodeId, wrapper: Object<'_>, owned: bool, connected: bool) {
    if owned {
        if let Some(r) = s.wrapper_ref(wrapper) {
            page.remember_wrapper(s.realm_id(), id.index(), r);
            // SAFETY: the wrapper was made just now and nothing that could
            // collect it runs before the payload is written (no allocation
            // in between): no protect/unprotect pair is needed.
            if let Some(node) = unsafe { s.unwrap_rooted::<Node>(wrapper.value()) } {
                node.wrapper.set(r.to_raw());
            }
        }
    } else if connected {
        page.pinned
            .borrow_mut()
            .entry((s.realm_id(), TargetKey::Node(id.index())))
            .or_insert_with(|| pyro_js::Persistent::new(s, wrapper.value()));
    }
}

fn wrap_node_kind<'s>(
    s: &Scope<'s>,
    page: &PageDom,
    id: NodeId,
    ty: NodeType,
    class: crate::html::ElClass,
    data: NodeData,
    k: Option<pyro_js::IdentityKey>,
) -> JsResult<Object<'s>> {
    match ty {
        NodeType::Element => crate::html::wrap_element(s, class, data, k),
        NodeType::Text => s.wrap::<Text>(data, k),
        NodeType::CDataSection => s.wrap::<CDataSection>(data, k),
        NodeType::Comment => s.wrap::<Comment>(data, k),
        NodeType::ProcessingInstruction => s.wrap::<ProcessingInstruction>(data, k),
        NodeType::Document => {
            // A document without a browsing context is first wrapped by the
            // script that created it: its nodes belong to that window's realm.
            let _ = page.dom.borrow_mut().set_document_creator(id, page.document());
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
        NodeType::Attribute => s.wrap::<collections::Attr>(data, k),
    }
}

pub fn node_value<'s>(s: &Scope<'s>, page: &PageDom, id: Option<NodeId>) -> JsResult<Value<'s>> {
    opt_node(s, page, id)
}

/// Extracts a Node argument (WebIDL `Node`), with a TypeError otherwise.
pub fn node_arg(s: &Scope<'_>, v: Value<'_>, what: &str) -> JsResult<NodeId> {
    node_id_of(s, v)
        .ok_or_else(|| JsError::Type(format!("Failed to execute '{what}': parameter is not of type 'Node'.").into()))
}

/// The node of a Node wrapper value: its id is read at once, so no
/// protect/unprotect pair keeps the wrapper alive meanwhile (`Scope::unwrap`).
fn node_id_of(s: &Scope<'_>, v: Value<'_>) -> Option<NodeId> {
    // SAFETY: the payload is read before anything can run a collection.
    unsafe { s.unwrap_rooted::<Node>(v) }.map(|n| n.id)
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
            items.push(match node_id_of(s, v) {
                Some(id) => NodeArg::Node(id),
                None => NodeArg::Text(dom_string(v)?),
            });
        }
        Ok(Self(items))
    }

    fn contains(&self, id: NodeId) -> bool {
        self.0.iter().any(|it| matches!(it, NodeArg::Node(n) if *n == id))
    }

    /// First sibling of `id` in the given direction that is not an argument.
    fn viable_sibling(&self, dom: &pyro_dom::Dom, id: NodeId, forward: bool) -> JsResult<Option<NodeId>> {
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
    let Some(ty) = crate::handlers::is_handler_attribute(name) else { return Ok(()) };
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
        def.pure_getter_typed("nodeType", pyro_js::GetterResult::Number, |s, n| {
            Ok(s.number(n.page().dom.borrow().node_type(n.id).js()? as u16 as f64))
        });
        def.pure_getter_typed("nodeName", pyro_js::GetterResult::String, |s, n| {
            let dom = n.page().dom.borrow();
            Ok(match dom.tag_name_parts(n.id).js()? {
                Some((name, upper)) => tag_name(s, name, upper),
                None => s.shared_string(&dom.node_name_shared(n.id).js()?),
            })
        });
        def.accessor(
            "nodeValue",
            |s, n| {
                let dom = n.page().dom.borrow();
                let kind = &dom.node(n.id).js()?.kind;
                Ok(match kind.char_data() {
                    Some(d) => s.string(d),
                    // An attribute's node value is its value.
                    None if kind.node_type() == NodeType::Attribute => s.string(dom.attr_node_value(n.id).js()?),
                    None => s.null(),
                })
            },
            |_s, n, v| {
                let set = |text: &str| {
                    let mut dom = n.page().dom.borrow_mut();
                    let kind = &dom.node(n.id).js()?.kind;
                    if kind.char_data().is_some() {
                        dom.set_char_data(n.id, text).js()?;
                    } else if kind.node_type() == NodeType::Attribute {
                        dom.set_attr_node_value(n.id, text).js()?;
                    }
                    Ok(())
                };
                if v.is_null() {
                    set("")
                } else {
                    with_str(v, set)
                }
            },
        );
        def.pure_accessor_typed(
            "textContent",
            pyro_js::GetterResult::Any,
            |s, n| {
                let dom = n.page().dom.borrow();
                Ok(dom.text_content_ref(n.id).js()?.map_or(s.null(), |t| s.string(&t)))
            },
            |s, n, v| {
                // `DOMString?`: undefined converts to null, which sets "".
                // The value converts first (an object's toString runs before
                // the blur, as in the spec), then a focused element inside
                // loses focus, which runs script. A string's characters are
                // borrowed only after that: a handler that uses the string
                // as a property key can swap its buffer for an atom's, and a
                // GC can then free the old one.
                let blur = || crate::input::blur_before_removal(n.page(), n.id, true);
                let set = |text: &str| n.page().dom.borrow_mut().set_text_content(n.id, text).js();
                // A `<script>`'s text is a Trusted Types sink when the document requires them.
                if crate::trusted_types::requires_for_script(n.page()) && crate::trusted_types::is_html_script(n.page(), n.id) {
                    let text = crate::trusted_types::compliant_string(s, n.page(), crate::trusted_types::Kind::Script, v, "HTMLScriptElement textContent", true)?;
                    return set(&text);
                }
                // A string (the usual value) needs no nullish test first.
                if v.is_string() {
                    blur();
                    return with_str(v, set);
                }
                if v.is_nullish() {
                    blur();
                    return set("");
                }
                let text = dom_string(v)?;
                blur();
                set(&text)
            },
        );
        def.pure_getter_typed("parentNode", pyro_js::GetterResult::ObjectOrNull, |s, n| {
            let p = n.page().dom.borrow().parent(n.id).js()?;
            node_value(s, n.page(), p)
        });
        def.pure_getter_typed("parentElement", pyro_js::GetterResult::ObjectOrNull, |s, n| {
            let p = n.page().dom.borrow().parent_element(n.id).js()?;
            node_value(s, n.page(), p)
        });
        def.pure_getter_typed("childNodes", pyro_js::GetterResult::ObjectOrNull, |s, n| {
            Ok(collections::child_nodes(s, n.page(), n.id)?.value())
        });
        def.pure_getter_typed("firstChild", pyro_js::GetterResult::ObjectOrNull, |s, n| {
            let c = n.page().dom.borrow().first_child(n.id).js()?;
            node_value(s, n.page(), c)
        });
        def.pure_getter_typed("lastChild", pyro_js::GetterResult::ObjectOrNull, |s, n| {
            let c = n.page().dom.borrow().last_child(n.id).js()?;
            node_value(s, n.page(), c)
        });
        def.pure_getter_typed("previousSibling", pyro_js::GetterResult::ObjectOrNull, |s, n| {
            let c = n.page().dom.borrow().prev_sibling(n.id).js()?;
            node_value(s, n.page(), c)
        });
        def.pure_getter_typed("nextSibling", pyro_js::GetterResult::ObjectOrNull, |s, n| {
            let c = n.page().dom.borrow().next_sibling(n.id).js()?;
            node_value(s, n.page(), c)
        });
        def.pure_getter_typed("ownerDocument", pyro_js::GetterResult::ObjectOrNull, |s, n| {
            let d = n.page().dom.borrow().owner_document(n.id).js()?;
            node_value(s, n.page(), d)
        });
        def.pure_getter_typed("isConnected", pyro_js::GetterResult::Boolean, |s, n| {
            Ok(s.boolean(n.page().dom.borrow().is_connected(n.id).js()?))
        });
        def.getter("baseURI", |s, n| {
            let dom = n.page().dom.borrow();
            let doc = dom.node_document(n.id).js()?;
            Ok(s.string(dom.document_base_url(doc).js()?))
        });
        def.pure_method("hasChildNodes", 0, |s, n, _a| Ok(s.boolean(n.page().dom.borrow().child_count(n.id).js()? > 0)));
        def.method("getRootNode", 0, |s, n, a| {
            let composed = match a.get(0).as_object() {
                Some(o) => o.get_atom("composed")?.to_bool(),
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
            crate::custom::upgrade_clone(s, n.page(), c);
            Ok(wrap_node(s, n.page(), c)?.value())
        });
        def.pure_method("contains", 1, |s, n, a| {
            a.require(1, "Node.contains")?;
            let Some(o) = opt_node_arg(s, a.get(0), "contains")? else { return Ok(s.boolean(false)) };
            Ok(s.boolean(n.page().dom.borrow().contains(n.id, o).js()?))
        });
        def.pure_method("compareDocumentPosition", 1, |s, n, a| {
            a.require(1, "Node.compareDocumentPosition")?;
            let o = node_arg(s, a.get(0), "compareDocumentPosition")?;
            Ok(s.number(n.page().dom.borrow().compare_document_position(n.id, o).js()? as f64))
        });
        def.pure_method("isSameNode", 1, |s, n, a| {
            a.require(1, "Node.isSameNode")?;
            let o = opt_node_arg(s, a.get(0), "isSameNode")?;
            Ok(s.boolean(o == Some(n.id)))
        });
        def.pure_method("isEqualNode", 1, |s, n, a| {
            a.require(1, "Node.isEqualNode")?;
            let Some(o) = opt_node_arg(s, a.get(0), "isEqualNode")? else { return Ok(s.boolean(false)) };
            Ok(s.boolean(n.page().dom.borrow().is_equal_node(n.id, o).js()?))
        });
        def.method("normalize", 0, |s, n, _a| {
            n.page().dom.borrow_mut().normalize(n.id).js()?;
            Ok(s.undefined())
        });
        def.pure_method("lookupPrefix", 1, |s, n, a| {
            a.require(1, "Node.lookupPrefix")?;
            let namespace = opt_dom_string(a.get(0))?;
            let dom = n.page().dom.borrow();
            Ok(match dom.lookup_prefix(n.id, namespace.as_deref()).js()? {
                Some(prefix) => s.string(prefix),
                None => s.null(),
            })
        });
        def.pure_method("lookupNamespaceURI", 1, |s, n, a| {
            a.require(1, "Node.lookupNamespaceURI")?;
            let prefix = opt_dom_string(a.get(0))?;
            let dom = n.page().dom.borrow();
            Ok(match dom.lookup_namespace_uri(n.id, prefix.as_deref()).js()? {
                Some(namespace) => s.string(namespace),
                None => s.null(),
            })
        });
        def.pure_method("isDefaultNamespace", 1, |s, n, a| {
            a.require(1, "Node.isDefaultNamespace")?;
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
    def.pure_getter_typed("children", pyro_js::GetterResult::ObjectOrNull, |s, n| {
        Ok(collections::children(s, n.page(), n.id)?.value())
    });
    def.pure_getter_typed("firstElementChild", pyro_js::GetterResult::ObjectOrNull, |s, n| {
        let c = n.page().dom.borrow().element_children(n.id).js()?.next();
        node_value(s, n.page(), c)
    });
    def.pure_getter_typed("lastElementChild", pyro_js::GetterResult::ObjectOrNull, |s, n| {
        let c = n.page().dom.borrow().element_children(n.id).js()?.last();
        node_value(s, n.page(), c)
    });
    def.pure_getter_typed("childElementCount", pyro_js::GetterResult::Number, |s, n| {
        Ok(s.number(n.page().dom.borrow().element_children(n.id).js()?.count() as f64))
    });
    def.method("append", 0, |s, n, a| {
        if let Some(c) = convert_nodes(s, n, &a)? {
            n.page().dom.borrow_mut().append_child(n.id, c).js()?;
        }
        Ok(s.undefined())
    });
    def.method("moveBefore", 2, |s, n, a| {
        a.require(2, "Node.moveBefore")?;
        let c = node_arg(s, a.get(0), "moveBefore")?;
        let r = opt_node_arg(s, a.get(1), "moveBefore")?;
        // The focused element stays focused across a move, wherever it
        // lands; the focus fixup rule looks at it afterwards.
        let moves_focus = {
            let dom = n.page().dom.borrow();
            let focused = dom.focused(n.page().document()).ok().flatten();
            focused.is_some_and(|f| dom.contains(c, f).unwrap_or(false))
        };
        n.page().dom.borrow_mut().move_before(n.id, c, r).js()?;
        if moves_focus {
            crate::input::schedule_focus_fixup(n.page());
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
    def.pure_method("querySelector", 1, |s, n, a| {
        let page = n.page();
        let r = with_selector(&a, 0, "querySelector", page, |q| page.dom.borrow().query_first(n.id, q))?.js()?;
        node_value(s, page, r)
    });
    def.pure_method("querySelectorAll", 1, |s, n, a| {
        let page = n.page();
        // The matches go to a buffer kept for the next query, then once into
        // the list: no vector grown and shrunk per call.
        let mut found = FOUND.take();
        let list =
            with_selector(&a, 0, "querySelectorAll", page, |q| page.dom.borrow().query_all_into(n.id, q, &mut found))
                .and_then(|r| r.js())
                .and_then(|()| collections::static_node_list(s, page, &found[..]));
        found.clear();
        // A larger query's buffer is cut down, not kept whole for the
        // thread's life (and its forks).
        found.shrink_to(FOUND_KEPT);
        FOUND.set(found);
        Ok(list?.value())
    });
}

thread_local! {
    /// `querySelectorAll`'s match buffer (taken while a query runs).
    static FOUND: std::cell::Cell<Vec<NodeId>> = const { std::cell::Cell::new(Vec::new()) };
}
/// The most matches whose buffer is kept between queries (64 KB): a
/// `querySelectorAll('*')` of a document of some thousand elements.
const FOUND_KEPT: usize = 1 << 13;

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
    def.pure_getter_typed("previousElementSibling", pyro_js::GetterResult::ObjectOrNull, |s, n| {
        node_value(s, n.page(), sibling_element(n, false)?)
    });
    def.pure_getter_typed("nextElementSibling", pyro_js::GetterResult::ObjectOrNull, |s, n| {
        node_value(s, n.page(), sibling_element(n, true)?)
    });
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
        def.pure_method("substringData", 2, |s, n, a| {
            a.require(2, "CharacterData.substringData")?;
            let (off, cnt) = (to_u32(a.get(0))?, to_u32(a.get(1))?);
            let r = n.page().dom.borrow().substring_data(n.id, off, cnt).js()?;
            Ok(s.string(&r))
        });
        def.method("appendData", 1, |s, n, a| {
            with_arg_str(&a, 0, "CharacterData.appendData", |d| {
                let mut dom = n.page().dom.borrow_mut();
                let len = dom.char_length(n.id).js()?;
                dom.replace_data(n.id, len, 0, d).js()
            })?;
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
            a.require(1, "Text.splitText")?;
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
        // `new ProcessingInstruction(target, data = "")` (DOM): as `createProcessingInstruction`
        // on the current document.
        def.constructor(1, |s, nt, a| {
            a.require(1, "ProcessingInstruction")?;
            let page = page_of(s)?;
            let target = dom_string(a.get(0))?;
            let data = if a.get(1).is_undefined() { String::new() } else { dom_string(a.get(1))? };
            let id = page.dom.borrow_mut().create_processing_instruction(page.document(), &target, &data).js()?;
            wrap_new_node::<ProcessingInstruction>(s, &page, id, nt)
        });
        // Processing instruction attributes (WICG declarative partial updates, as Chromium): the
        // data read as quoted `name="value"` pairs; writes rewrite the data in place, so they are
        // characterData mutations.
        def.pure_method("hasAttributes", 0, |s, n, _a| Ok(s.boolean(!pi_attributes(n)?.is_empty())));
        def.pure_method("getAttributeNames", 0, |s, n, _a| {
            let names: Vec<String> = pi_attributes(n)?.into_iter().map(|(k, _)| k).collect();
            let items: Vec<Value<'_>> = names.iter().map(|k| s.string(k)).collect();
            Ok(s.array(&items)?.value())
        });
        def.pure_method("getAttribute", 1, |s, n, a| {
            a.require(1, "ProcessingInstruction.getAttribute")?;
            let name = dom_string(a.get(0))?;
            Ok(pi_attributes(n)?.into_iter().find(|(k, _)| *k == name).map_or_else(|| s.null(), |(_, v)| s.string(&v)))
        });
        def.pure_method("hasAttribute", 1, |s, n, a| {
            a.require(1, "ProcessingInstruction.hasAttribute")?;
            let name = dom_string(a.get(0))?;
            Ok(s.boolean(pi_attributes(n)?.iter().any(|(k, _)| *k == name)))
        });
        def.method("setAttribute", 2, |s, n, a| {
            a.require(2, "ProcessingInstruction.setAttribute")?;
            let name = pi_attribute_name(a.get(0))?;
            let value = dom_string(a.get(1))?;
            let mut attrs = pi_attributes(n)?;
            match attrs.iter_mut().find(|(k, _)| *k == name) {
                Some(slot) => slot.1 = value,
                None => attrs.push((name, value)),
            }
            pi_write(n, &attrs)?;
            Ok(s.undefined())
        });
        def.method("removeAttribute", 1, |s, n, a| {
            a.require(1, "ProcessingInstruction.removeAttribute")?;
            let name = dom_string(a.get(0))?;
            let mut attrs = pi_attributes(n)?;
            let before = attrs.len();
            attrs.retain(|(k, _)| *k != name);
            if attrs.len() != before {
                pi_write(n, &attrs)?;
            }
            Ok(s.undefined())
        });
        def.method("toggleAttribute", 1, |s, n, a| {
            a.require(1, "ProcessingInstruction.toggleAttribute")?;
            let name = pi_attribute_name(a.get(0))?;
            let force = (!a.get(1).is_undefined()).then(|| a.get(1).to_bool());
            let mut attrs = pi_attributes(n)?;
            let present = attrs.iter().any(|(k, _)| *k == name);
            let result = match (present, force) {
                (true, Some(true)) | (false, Some(false)) => present,
                (true, _) => {
                    attrs.retain(|(k, _)| *k != name);
                    pi_write(n, &attrs)?;
                    false
                }
                (false, _) => {
                    attrs.push((name, String::new()));
                    pi_write(n, &attrs)?;
                    true
                }
            };
            Ok(s.boolean(result))
        });
    }
}

/// A processing instruction's attributes: its data as whitespace-separated `name="value"` (or
/// single-quoted) pairs, character references decoded; anything else (an unquoted value) is no
/// attributes at all. A repeated name keeps its first value.
fn pi_attributes(n: &NodeData) -> JsResult<Vec<(String, String)>> {
    let data = n.page().dom.borrow().char_data(n.id).js()?.to_owned();
    Ok(parse_pi_attributes(&data).unwrap_or_default())
}

fn parse_pi_attributes(data: &str) -> Option<Vec<(String, String)>> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut rest = data.trim_start_matches(|c: char| c.is_ascii_whitespace());
    while !rest.is_empty() {
        let end = rest.find(|c: char| c.is_ascii_whitespace() || matches!(c, '=' | '>' | '/'))?;
        let (name, tail) = rest.split_at(end);
        if name.is_empty() {
            return None;
        }
        let tail = tail.trim_start_matches(|c: char| c.is_ascii_whitespace()).strip_prefix('=')?;
        let tail = tail.trim_start_matches(|c: char| c.is_ascii_whitespace());
        let quote = tail.chars().next().filter(|c| matches!(c, '"' | '\''))?;
        let close = tail[1..].find(quote)? + 1;
        let value = decode_char_refs(&tail[1..close]);
        if !out.iter().any(|(k, _)| k == name) {
            out.push((name.to_owned(), value));
        }
        rest = &tail[close + 1..];
        if !rest.is_empty() && !rest.starts_with(|c: char| c.is_ascii_whitespace()) {
            return None;
        }
        rest = rest.trim_start_matches(|c: char| c.is_ascii_whitespace());
    }
    Some(out)
}

/// Decodes the character references an attribute serialization writes (`&amp;`, `&quot;`,
/// `&lt;`, `&gt;`, `&nbsp;`, `&apos;`, numeric ones); others stay as written.
fn decode_char_refs(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(semi) = rest.find(';').filter(|&j| j <= 10) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..semi];
        let decoded = match entity {
            "amp" => Some('&'),
            "quot" => Some('"'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "nbsp" => Some('\u{a0}'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .map(|h| u32::from_str_radix(h, 16))
                .or_else(|| entity.strip_prefix('#').map(|d| d.parse::<u32>()))
                .and_then(|r| r.ok())
                .and_then(char::from_u32),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// A name `setAttribute`/`toggleAttribute` accept (DOM "valid attribute local name"): not empty,
/// no ASCII whitespace, NULL, `/`, `>` or `=`.
fn pi_attribute_name(v: Value<'_>) -> JsResult<String> {
    let name = dom_string(v)?;
    if name.is_empty() || name.contains(|c: char| c.is_ascii_whitespace() || matches!(c, '\0' | '/' | '>' | '=')) {
        return Err(JsError::dom("InvalidCharacterError", "The attribute name is not valid."));
    }
    Ok(name)
}

/// Rewrites the data from `attrs`, serialized as an element's attributes are.
fn pi_write(n: &NodeData, attrs: &[(String, String)]) -> JsResult<()> {
    let mut text = String::new();
    for (i, (k, v)) in attrs.iter().enumerate() {
        if i > 0 {
            text.push(' ');
        }
        text.push_str(k);
        text.push_str("=\"");
        for c in v.chars() {
            match c {
                '&' => text.push_str("&amp;"),
                '"' => text.push_str("&quot;"),
                '<' => text.push_str("&lt;"),
                '>' => text.push_str("&gt;"),
                '\u{a0}' => text.push_str("&nbsp;"),
                c => text.push(c),
            }
        }
        text.push('"');
    }
    n.page().dom.borrow_mut().set_char_data(n.id, &text).js()
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
    {
        let mut dom = page.dom.borrow_mut();
        dom.add_ref(id).js()?;
        // `new Document()`: created by the constructor's window.
        if matches!(dom.node(id).js()?.kind, NodeKind::Document(_)) {
            let _ = dom.set_document_creator(id, page.document());
        }
    }
    let data = NodeData {
        target: EventTargetData::for_key(page.clone(), TargetKey::Node(id.index())),
        id,
        wrapper: std::cell::Cell::new(0),
        id_string: AtomString::default(),
        class_string: AtomString::default(),
    };
    let owned = s.owned_wrappers() && page.wrapper_slot_free(s.realm_id(), id.index());
    let key = if owned { pyro_js::IdentityKey::OWNED } else { page.node_identity(id) };
    let wrapper = s.wrap_new::<C>(data, new_target, Some(key))?;
    register_node_wrapper(s, page, id, wrapper, owned, false);
    Ok(wrapper)
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
        def.pure_method("getElementById", 1, |s, n, a| {
            let r = with_arg_str(&a, 0, "getElementById", |id| n.page().dom.borrow().get_element_by_id(n.id, id).js())?;
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
        serializable = o.get_atom("serializableShadowRoots")?.to_bool();
        let list = o.get_atom("shadowRoots")?;
        if !list.is_undefined() {
            let list = list.as_object().ok_or_else(|| {
                JsError::Type(
                    "Failed to execute 'getHTML': The provided value cannot be converted to a sequence.".into(),
                )
            })?;
            let len = list.get_atom("length")?.to_number()? as u32;
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
    let shadow = n.page().dom.borrow().node(n.id).is_ok_and(|node| matches!(node.kind, pyro_dom::node::NodeKind::Fragment(_)));
    let sink = if shadow { "ShadowRoot setHTMLUnsafe" } else { "Element setHTMLUnsafe" };
    let html = crate::trusted_types::compliant_string(s, n.page(), crate::trusted_types::Kind::Html, a.get(0), sink, false)?;
    crate::input::blur_before_removal(n.page(), n.id, true);
    pyro_dom::parse::set_html_unsafe(&mut n.page().dom.borrow_mut(), n.id, &html).js()?;
    Ok(s.undefined())
}

pub struct ShadowRoot;
impl JsClass for ShadowRoot {
    const NAME: &'static str = "ShadowRoot";
    type Data = NodeData;
    type Parent = DocumentFragment;
    fn define(def: &mut ClassDef<Self>) {
        // Fullscreen API: the document's fullscreen element, retargeted against this shadow root.
        def.accessor("fullscreenElement", |s, n| crate::fullscreen::element_for(s, n.page(), n.id), |_s, _n, _v| Ok(()));
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
        def.getter("delegatesFocus", |s, n| shadow_flag(s, n, pyro_dom::shadow_flags::DELEGATES_FOCUS));
        def.getter("clonable", |s, n| shadow_flag(s, n, pyro_dom::shadow_flags::CLONABLE));
        def.getter("serializable", |s, n| shadow_flag(s, n, pyro_dom::shadow_flags::SERIALIZABLE));
        def.getter("slotAssignment", |s, n| {
            let flags = n.page().dom.borrow().shadow_root_flags(n.id).js()?;
            Ok(s.string(if flags & pyro_dom::shadow_flags::MANUAL_SLOTS != 0 { "manual" } else { "named" }))
        });
        def.method("getHTML", 0, get_html);
        def.method("setHTMLUnsafe", 1, set_html_unsafe);
        def.method("getAnimations", 0, |s, n, _a| {
            let root = wrap_node(s, n.page(), n.id)?.value();
            crate::animations::root_animations(s, n.page(), root)
        });
        def.accessor("adoptedStyleSheets", crate::css::adopted_style_sheets, crate::css::set_adopted_style_sheets);
        def.method("elementFromPoint", 2, crate::layout_api::shadow_element_from_point);
        def.method("elementsFromPoint", 2, crate::layout_api::shadow_elements_from_point);
        def.getter("activeElement", |s, n| {
            let f = n.page().dom.borrow().active_element(n.id).js()?;
            node_value(s, n.page(), f)
        });
        def.getter("styleSheets", crate::css::style_sheets);
        def.accessor(
            "onslotchange",
            |s, n| crate::target::event_handler(s, &n.target, "slotchange"),
            |s, n, v| crate::target::set_event_handler(s, &n.target, "slotchange", v),
        );
        def.accessor(
            "innerHTML",
            |s, n| Ok(s.string(&n.page().dom.borrow().inner_html(n.id).js()?)),
            |s, n, v| {
                let html = crate::trusted_types::compliant_string(s, n.page(), crate::trusted_types::Kind::Html, v, "ShadowRoot innerHTML", true)?;
                let mut dom = n.page().dom.borrow_mut();
                let host = dom.fragment_host(n.id).js()?.ok_or(JsError::Type("detached shadow root".into()))?;
                let frag = pyro_dom::parse::parse_fragment(&mut dom, host, &html).js()?;
                dom.replace_all(n.id, Some(frag)).js()
            },
        );
    }
}

// ----- element ----------------------------------------------------------

/// `tagName` from [`pyro_dom::Dom::tag_name_parts`]: the local name's string
/// (the same as `localName`'s), or its uppercased form, one per realm too.
#[inline]
fn tag_name<'s>(s: &Scope<'s>, local: &markup5ever::LocalName, upper: bool) -> Value<'s> {
    if upper {
        upper_local_name(s, local)
    } else {
        local.js(s)
    }
}

pub struct Element;
impl JsClass for Element {
    const NAME: &'static str = "Element";
    type Data = NodeData;
    type Parent = Node;
    fn define(def: &mut ClassDef<Self>) {
        define_parent_node(def);
        define_child_node(def);
        define_non_doctype_child(def);
        // Fullscreen API (`crate::fullscreen`): granted with transient activation, denied otherwise.
        crate::handlers::define_fullscreen(def);
        def.promise_method("requestFullscreen", 0, |s, n, _a| crate::fullscreen::request(s, n.page(), n.id));
        // CSS Typed OM: the element's computed values as a StylePropertyMapReadOnly.
        def.method("computedStyleMap", 0, |s, n, _a| {
            let el = crate::node::wrap_node(s, n.page(), n.id)?.value();
            // The computed (not resolved) value of a property, from the style engine.
            let (page, id) = (n.page().clone(), n.id);
            let get = s.function("computedValue", move |s, _this, a| {
                let name = crate::util::dom_string(a.get(0))?;
                let (Ok(layout), Ok(dom)) = (page.layout.try_borrow(), page.dom.try_borrow()) else { return Ok(s.null()) };
                Ok(match layout.style().computed_value(&dom, id, &name) {
                    Some(v) if !v.is_empty() => s.string(&v),
                    _ => s.null(),
                })
            })?;
            crate::typed_om::computed_style_map(s, n.page(), el, get.value())
        });
        def.pure_getter_typed("tagName", pyro_js::GetterResult::String, |s, n| {
            let dom = n.page().dom.borrow();
            Ok(match dom.tag_name_parts(n.id).js()? {
                Some((name, upper)) => tag_name(s, name, upper),
                None => s.shared_string(&dom.tag_name_shared(n.id).js()?),
            })
        });
        // Names and id are the element's atoms, class its attribute value:
        // one string each per realm (`AtomJs`, `attr_value`), not a new one
        // per read.
        def.pure_getter_typed("localName", pyro_js::GetterResult::String, |s, n| {
            Ok(n.page().dom.borrow().element(n.id).js()?.local_name().js(s))
        });
        def.getter("namespaceURI", |s, n| {
            let dom = n.page().dom.borrow();
            let ns = dom.element(n.id).js()?.namespace();
            Ok(if ns.is_empty() { s.null() } else { ns.js(s) })
        });
        def.getter("prefix", |s, n| {
            let dom = n.page().dom.borrow();
            Ok(match dom.element(n.id).js()?.prefix() {
                Some(p) if !p.is_empty() => p.js(s),
                _ => s.null(),
            })
        });
        def.pure_accessor_typed(
            "id",
            pyro_js::GetterResult::String,
            |s, n| {
                let dom = n.page().dom.borrow();
                Ok(match dom.element(n.id).js()?.id_atom() {
                    Some(atom) => n.id_string.get(s, atom),
                    None => atom_or_empty(s, None),
                })
            },
            |_s, n, v| {
                with_str(v, |id| {
                    n.page().dom.borrow_mut().set_attribute_local(n.id, &markup5ever::local_name!("id"), id).js()
                })
            },
        );
        def.pure_accessor_typed(
            "className",
            pyro_js::GetterResult::String,
            |s, n| {
                let dom = n.page().dom.borrow();
                Ok(match dom.element(n.id).js()?.class_value() {
                    Some((_, Some(atom))) => n.class_string.get(s, atom),
                    Some((value, None)) => attr_value(s, value, None),
                    None => atom_or_empty(s, None),
                })
            },
            |_s, n, v| {
                with_str(v, |class| {
                    n.page().dom.borrow_mut().set_attribute_local(n.id, &markup5ever::local_name!("class"), class).js()
                })
            },
        );
        def.pure_accessor_typed(
            "slot",
            pyro_js::GetterResult::String,
            |s, n| {
                let dom = n.page().dom.borrow();
                Ok(s.string(dom.element(n.id).js()?.attr_local(&markup5ever::local_name!("slot")).unwrap_or("")))
            },
            |_s, n, v| {
                with_str(v, |slot| {
                    n.page().dom.borrow_mut().set_attribute_local(n.id, &markup5ever::local_name!("slot"), slot).js()
                })
            },
        );
        def.pure_accessor_typed(
            "classList",
            pyro_js::GetterResult::ObjectOrNull,
            |s, n| Ok(collections::token_list(s, n.page(), n.id, "class")?.value()),
            // [PutForwards=value]
            |_s, n, v| {
                with_str(v, |value| {
                    n.page().dom.borrow_mut().set_attribute_local(n.id, &markup5ever::local_name!("class"), value).js()
                })
            },
        );
        def.getter("part", |s, n| Ok(collections::token_list(s, n.page(), n.id, "part")?.value()));
        def.getter("attributes", |s, n| Ok(collections::named_node_map(s, n.page(), n.id)?.value()));
        def.pure_method("hasAttributes", 0, |s, n, _a| {
            Ok(s.boolean(!n.page().dom.borrow().attributes(n.id).js()?.is_empty()))
        });
        def.method("getAttributeNode", 1, |s, n, a| {
            let name = arg_string(&a, 0, "Element.getAttributeNode")?;
            match collections::attr_named_qualified(n.page(), n.id, &name)? {
                Some(found) => collections::make_attr(s, n.page(), n.id, found),
                None => Ok(s.null()),
            }
        });
        def.method("getAttributeNodeNS", 2, |s, n, a| {
            a.require(2, "Element.getAttributeNodeNS")?;
            let ns = opt_dom_string(a.get(0))?;
            let local = dom_string(a.get(1))?;
            match collections::attr_named_ns(n.page(), n.id, ns.as_deref(), &local)? {
                Some(found) => collections::make_attr(s, n.page(), n.id, found),
                None => Ok(s.null()),
            }
        });
        def.method("setAttributeNode", 1, |s, n, a| {
            a.require(1, "Element.setAttributeNode")?;
            collections::set_attribute_node(s, n.page(), n.id, a.get(0))
        });
        def.method("setAttributeNodeNS", 1, |s, n, a| {
            a.require(1, "Element.setAttributeNodeNS")?;
            collections::set_attribute_node(s, n.page(), n.id, a.get(0))
        });
        def.method("removeAttributeNode", 1, |s, n, a| {
            a.require(1, "Element.removeAttributeNode")?;
            collections::remove_attribute_node(s, n.page(), n.id, a.get(0))
        });
        def.pure_method("getAttributeNames", 0, |s, n, _a| {
            let mut vals = pyro_js::RootedVec::new();
            {
                let dom = n.page().dom.borrow();
                for a in dom.attributes(n.id).js()? {
                    // Making strings runs no script: the DOM stays borrowed.
                    vals.push(
                        s,
                        match &a.name.prefix {
                            Some(p) if !p.is_empty() => s.string(&a.qualified_name()),
                            _ => a.name.local.js(s),
                        },
                    );
                }
            }
            Ok(s.array(&vals.iter(s).collect::<Vec<_>>())?.value())
        });
        def.pure_method("getAttribute", 1, |s, n, a| {
            a.require(1, "Element.getAttribute")?;
            let name = a.get(0);
            let name = n.page().names.arg(s, &name)?;
            let dom = n.page().dom.borrow();
            let found = match &name {
                NameArg::Name(name) => dom.get_attribute_named_atom(n.id, name),
                NameArg::Text(name) => dom.get_attribute_qualified_atom(n.id, name),
            };
            Ok(match found.js()? {
                Some((value, atom)) => attr_value(s, value, atom),
                None => s.null(),
            })
        });
        def.pure_method("getAttributeNS", 2, |s, n, a| {
            a.require(2, "Element.getAttributeNS")?;
            let ns = opt_dom_string(a.get(0))?;
            let local = dom_string(a.get(1))?;
            let dom = n.page().dom.borrow();
            Ok(dom.get_attribute_ns(n.id, ns.as_deref(), &local).js()?.map_or(s.null(), |v| s.string(v)))
        });
        def.method("setAttribute", 2, |s, n, a| {
            a.require(2, "Element.setAttribute")?;
            let (name, value_arg) = (a.get(0), a.get(1));
            let name = n.page().names.arg(s, &name)?;
            let value = match crate::trusted_types::attribute_value(s, n.page(), n.id, &setattribute_local(n, name.text()), None, value_arg)? {
                Some(trusted) => std::borrow::Cow::Owned(trusted),
                None => str_of(&value_arg)?,
            };
            match &name {
                NameArg::Name(name) => n.page().dom.borrow_mut().set_attribute_named(n.id, name, &value),
                NameArg::Text(name) => n.page().dom.borrow_mut().set_attribute(n.id, name, &value),
            }
            .js()?;
            handler_attribute_changed(s, n, name.text(), Some(&value))?;
            crate::input::blur_if_focus_disabled(n.page(), n.id, name.text());
            Ok(s.undefined())
        });
        def.method("setAttributeNS", 3, |s, n, a| {
            a.require(3, "Element.setAttributeNS")?;
            let ns = opt_dom_string(a.get(0))?;
            let name = dom_string(a.get(1))?;
            let local = name.rsplit(':').next().unwrap_or(&name).to_owned();
            let value = match crate::trusted_types::attribute_value(s, n.page(), n.id, &local, ns.as_deref(), a.get(2))? {
                Some(trusted) => trusted,
                None => dom_string(a.get(2))?,
            };
            n.page().dom.borrow_mut().set_attribute_ns(n.id, ns.as_deref(), &name, &value).js()?;
            if ns.is_none() {
                crate::input::blur_if_focus_disabled(n.page(), n.id, &name);
            }
            Ok(s.undefined())
        });
        def.method("removeAttribute", 1, |s, n, a| {
            a.require(1, "Element.removeAttribute")?;
            let name = a.get(0);
            let name = n.page().names.arg(s, &name)?;
            let removed = match &name {
                NameArg::Name(name) => n.page().dom.borrow_mut().remove_attribute_named(n.id, name),
                NameArg::Text(name) => n.page().dom.borrow_mut().remove_attribute(n.id, name),
            }
            .js()?;
            if removed {
                handler_attribute_changed(s, n, name.text(), None)?;
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
        def.pure_method("hasAttribute", 1, |s, n, a| {
            a.require(1, "Element.hasAttribute")?;
            let name = a.get(0);
            // Converted before the DOM is borrowed: an object's conversion runs script.
            let name = n.page().names.arg(s, &name)?;
            let dom = n.page().dom.borrow();
            Ok(s.boolean(
                match name {
                    NameArg::Name(name) => dom.has_attribute_named(n.id, &name),
                    NameArg::Text(name) => dom.has_attribute_qualified(n.id, &name),
                }
                .js()?,
            ))
        });
        def.pure_method("hasAttributeNS", 2, |s, n, a| {
            a.require(2, "Element.hasAttributeNS")?;
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
        def.pure_method("matches", 1, element_matches);
        def.pure_method("webkitMatchesSelector", 1, element_matches);
        def.pure_method("closest", 1, |s, n, a| {
            let page = n.page();
            let r =
                with_selector(&a, 0, "Element.closest", page, |q| page.dom.borrow().element_closest(n.id, q))?.js()?;
            node_value(s, page, r)
        });
        def.pure_method("getElementsByTagName", 1, |s, n, a| {
            let name = arg_string(&a, 0, "getElementsByTagName")?;
            Ok(collections::collection(s, n.page(), n.id, CollectionKind::TagName(name))?.value())
        });
        def.pure_method("getElementsByTagNameNS", 2, |s, n, a| {
            a.require(2, "getElementsByTagNameNS")?;
            let namespace = opt_dom_string(a.get(0))?.unwrap_or_default();
            let local = dom_string(a.get(1))?;
            Ok(collections::collection(s, n.page(), n.id, CollectionKind::TagNameNs(namespace, local))?.value())
        });
        def.pure_method("getElementsByClassName", 1, |s, n, a| {
            let names = arg_string(&a, 0, "getElementsByClassName")?;
            Ok(collections::collection(s, n.page(), n.id, CollectionKind::ClassName(names))?.value())
        });
        def.accessor(
            "innerHTML",
            |s, n| Ok(s.string(&n.page().dom.borrow().inner_html(n.id).js()?)),
            |s, n, v| {
                let html = crate::trusted_types::compliant_string(s, n.page(), crate::trusted_types::Kind::Html, v, "Element innerHTML", true)?;
                crate::input::blur_before_removal(n.page(), n.id, true);
                pyro_dom::parse::set_inner_html(&mut n.page().dom.borrow_mut(), n.id, &html).js()
            },
        );
        def.accessor(
            "outerHTML",
            // An element of an XML document serializes as XML (DOM Parsing "fragment serializing").
            |s, n| {
                let dom = n.page().dom.borrow();
                let xml = dom.node_document(n.id).is_ok_and(|d| dom.is_xml_document(d));
                Ok(s.string(&if xml { dom.xml_serialize(n.id) } else { dom.outer_html(n.id) }.js()?))
            },
            |s, n, v| {
                let html = crate::trusted_types::compliant_string(s, n.page(), crate::trusted_types::Kind::Html, v, "Element outerHTML", true)?;
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
                let frag = pyro_dom::parse::parse_fragment(&mut dom, ctx, &html).js()?;
                dom.replace_child(parent, frag, n.id).js()?;
                Ok(())
            },
        );
        def.method("insertAdjacentHTML", 2, |s, n, a| {
            a.require(2, "Element.insertAdjacentHTML")?;
            let pos = dom_string(a.get(0))?.to_ascii_lowercase();
            let html = crate::trusted_types::compliant_string(s, n.page(), crate::trusted_types::Kind::Html, a.get(1), "Element insertAdjacentHTML", false)?;
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
                    ));
                }
            };
            let frag = pyro_dom::parse::parse_fragment(&mut dom, ctx, &html).js()?;
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
            if o.get_atom("clonable")?.to_bool() {
                flags |= pyro_dom::shadow_flags::CLONABLE;
            }
            if o.get_atom("delegatesFocus")?.to_bool() {
                flags |= pyro_dom::shadow_flags::DELEGATES_FOCUS;
            }
            let mode = dom_string(o.get_atom("mode")?)?;
            let mode = match mode.as_str() {
                "open" => ShadowRootMode::Open,
                "closed" => ShadowRootMode::Closed,
                _ => {
                    return Err(JsError::Type(format!(
                        "The provided value '{mode}' is not a valid enum value of type ShadowRootMode."
                    ).into()))
                }
            };
            if o.get_atom("serializable")?.to_bool() {
                flags |= pyro_dom::shadow_flags::SERIALIZABLE;
            }
            let slots = o.get_atom("slotAssignment")?;
            if !slots.is_undefined() {
                match dom_string(slots)?.as_str() {
                    "named" => {}
                    "manual" => flags |= pyro_dom::shadow_flags::MANUAL_SLOTS,
                    other => {
                        return Err(JsError::Type(format!(
                            "The provided value '{other}' is not a valid enum value of type SlotAssignmentMode."
                        ).into()))
                    }
                }
            }
            // A host whose upgrade constructor runs ("precustomized") makes
            // the root available to its internals (a custom one does in pyro-dom).
            if crate::custom::is_being_constructed(n.page(), n.id) {
                flags |= pyro_dom::shadow_flags::AVAILABLE_TO_INTERNALS;
            }
            let sr = n.page().dom.borrow_mut().attach_shadow_with(n.id, mode, flags).js()?;
            Ok(wrap_node(s, n.page(), sr)?.value())
        });
        def.pure_getter_typed("shadowRoot", pyro_js::GetterResult::ObjectOrNull, |s, n| {
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
    let r = with_selector(&a, 0, "Element.matches", page, |q| page.dom.borrow().element_matches(n.id, q))?;
    Ok(s.boolean(r.js()?))
}

fn insert_adjacent(dom: &mut pyro_dom::Dom, el: NodeId, pos: &str, node: NodeId) -> JsResult<bool> {
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
        crate::handlers::define_fullscreen(def);
        // Fullscreen API (`crate::fullscreen`). These getters answer for the browsing context's
        // document; another document (a DOMParser or createHTMLDocument one) has no browsing
        // context, so fullscreen is not enabled there and nothing in it is fullscreen.
        // (`[LegacyLenientSetter]`: assigning to them does nothing, and does not throw in strict mode.)
        // Not enabled in a document that is no longer active (its frame navigated away).
        def.accessor("fullscreenEnabled", |s, n| Ok(s.boolean(n.id == n.page().document() && n.page().dom.borrow().is_in_active_document(n.id) && crate::fullscreen::enabled(n.page()))), |_s, _n, _v| Ok(()));
        def.accessor("fullscreen", |s, n| Ok(s.boolean(n.id == n.page().document() && crate::fullscreen::element(n.page()).is_some())), |_s, _n, _v| Ok(()));
        def.accessor("fullscreenElement", |s, n| if n.id == n.page().document() { crate::fullscreen::element_for(s, n.page(), n.id) } else { Ok(s.null()) }, |_s, _n, _v| Ok(()));
        def.promise_method("exitFullscreen", 0, |s, n, _a| {
            if n.id == n.page().document() {
                return crate::fullscreen::exit(s, n.page());
            }
            let d = s.deferred()?;
            let promise = d.promise.get(s);
            d.reject(s, JsError::Type("Document not active or no fullscreen element".into()).to_value(s))?;
            Ok(promise)
        });
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
            let doc = pyro_dom::parse::parse_html_unsafe(&page.dom, &html);
            Ok(wrap_node(s, &page, doc)?.value())
        });
        // `documentElement`, `head` and `body` are kept on the document's
        // data as its tree changes: one load each, on the lean dispatch.
        def.pure_getter_typed("documentElement", pyro_js::GetterResult::ObjectOrNull, |s, n| {
            let e = n.page().dom.borrow().document_element(n.id).js()?;
            node_value(s, n.page(), e)
        });
        // CSSOM View: the element whose scroll offsets are the viewport's:
        // the document element, or the body element in quirks mode.
        def.getter("scrollingElement", |s, n| {
            let e = {
                let dom = n.page().dom.borrow();
                if dom.document_data(n.id).js()?.quirks == pyro_dom::QuirksMode::Quirks {
                    dom.body(n.id).js()?
                } else {
                    dom.document_element(n.id).js()?
                }
            };
            node_value(s, n.page(), e)
        });
        // SVG 2: the document element when it is an SVG `<svg>`, else null.
        def.getter("rootElement", |s, n| {
            let e = {
                let dom = n.page().dom.borrow();
                dom.document_element(n.id).js()?.filter(|&root| {
                    dom.element(root).is_ok_and(|e| e.namespace() == &markup5ever::ns!(svg) && &*e.name.local == "svg")
                })
            };
            node_value(s, n.page(), e)
        });
        def.getter("styleSheets", crate::css::style_sheets);
        def.getter("timeline", |s, n| crate::animations::timeline(s, n.page()));
        def.method("getAnimations", 0, |s, n, _a| {
            let root = wrap_node(s, n.page(), n.id)?.value();
            crate::animations::root_animations(s, n.page(), root)
        });
        def.accessor("adoptedStyleSheets", crate::css::adopted_style_sheets, crate::css::set_adopted_style_sheets);
        def.getter("doctype", |s, n| {
            let e = n.page().dom.borrow().doctype(n.id).js()?;
            node_value(s, n.page(), e)
        });
        def.pure_getter_typed("head", pyro_js::GetterResult::ObjectOrNull, |s, n| {
            let e = n.page().dom.borrow().head(n.id).js()?;
            node_value(s, n.page(), e)
        });
        def.pure_accessor_typed(
            "body",
            pyro_js::GetterResult::ObjectOrNull,
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
                let sel = pyro_dom::select::parse_selectors("title").js()?;
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
                let sel = pyro_dom::select::parse_selectors("title").js()?;
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
            Ok(s.string(if q == pyro_dom::QuirksMode::Quirks { "BackCompat" } else { "CSS1Compat" }))
        });
        def.getter("characterSet", |s, n| Ok(s.string(n.page().dom.borrow().document_data(n.id).js()?.charset)));
        def.getter("charset", |s, n| Ok(s.string(n.page().dom.borrow().document_data(n.id).js()?.charset)));
        def.getter("inputEncoding", |s, n| Ok(s.string(n.page().dom.borrow().document_data(n.id).js()?.charset)));
        def.getter("contentType", |s, n| Ok(s.string(&n.page().dom.borrow().document_data(n.id).js()?.content_type)));
        def.getter("readyState", |s, n| Ok(s.string(crate::html::ready_state(n.page(), n.id))));
        def.getter("visibilityState", |s, n| Ok(s.string(if n.page().is_hidden() { "hidden" } else { "visible" })));
        def.getter("hidden", |s, n| Ok(s.boolean(n.page().is_hidden())));
        // A document its frame navigated away from has no browsing context.
        def.getter("defaultView", |s, n| {
            Ok(if n.page().document() == n.id && !n.page().is_detached() { s.global().value() } else { s.null() })
        });
        def.getter("activeElement", |s, n| {
            let f = n.page().dom.borrow().active_element(n.id).js()?;
            node_value(s, n.page(), f)
        });
        def.pure_method("hasFocus", 0, |s, n, _a| Ok(s.boolean(n.page().document() == n.id && !n.page().is_detached())));
        def.getter("currentScript", |s, n| node_value(s, n.page(), n.page().current_script()));
        def.getter("implementation", |s, n| Ok(crate::html::dom_implementation(s, n)?.value()));
        def.getter("forms", |s, n| {
            Ok(collections::collection(s, n.page(), n.id, CollectionKind::TagName("form".into()))?.value())
        });
        def.getter("embeds", |s, n| {
            Ok(collections::collection(s, n.page(), n.id, CollectionKind::TagName("embed".into()))?.value())
        });
        def.getter("plugins", |s, n| {
            Ok(collections::collection(s, n.page(), n.id, CollectionKind::TagName("embed".into()))?.value())
        });
        def.getter("anchors", |s, n| Ok(collections::collection(s, n.page(), n.id, CollectionKind::Anchors)?.value()));
        def.getter("applets", |s, n| Ok(collections::collection(s, n.page(), n.id, CollectionKind::Empty)?.value()));
        def.property_names(|_s, n| {
            let dom = n.page().dom.borrow();
            let mut names: Vec<String> = Vec::new();
            if let Ok(desc) = dom.descendants(n.id) {
                for id in desc {
                    let Ok(e) = dom.element(id) else { continue };
                    if !e.is_html() {
                        continue;
                    }
                    let local = &*e.name.local;
                    let name = e.attr("name").filter(|v| !v.is_empty());
                    if matches!(local, "embed" | "form" | "iframe" | "img" | "object") {
                        if let Some(v) = name {
                            if !names.iter().any(|x| x == v) {
                                names.push(v.to_owned());
                            }
                        }
                    }
                    if local == "img" && name.is_some() {
                        if let Some(id) = e.id().filter(|v| !v.is_empty()) {
                            if !names.iter().any(|x| x == id) {
                                names.push(id.to_owned());
                            }
                        }
                    }
                }
            }
            Ok(names)
        });
        // Named properties: `document.foo` for `<form name=foo>` and the like;
        // an iframe stands for its child window. pyro-dom announces every name
        // that may appear (`Dom::set_named_item_hook`, installed by
        // `pyro_webapi::install`), so reads of the document's other properties
        // stay inline-cacheable.
        def.named_properties_are_announced();
        def.named(false, |s, n, name| {
            let page = n.page();
            let found = page.dom.borrow().document_named_elements(n.id, name).js()?;
            match found[..] {
                [] => Ok(None),
                [el] => {
                    let is_frame = page.dom.borrow().element(el).is_ok_and(|e| &*e.name.local == "iframe");
                    if is_frame {
                        if let Some(window) = page.frame_window(s, el)? {
                            return Ok(Some(window));
                        }
                    }
                    Ok(Some(wrap_node(s, page, el)?.value()))
                }
                _ => {
                    let kind = CollectionKind::DocumentNamed(name.to_owned());
                    Ok(Some(collections::collection(s, page, n.id, kind)?.value()))
                }
            }
        });
        const HTML_NS: &str = "http://www.w3.org/1999/xhtml";
        def.getter("images", |s, n| {
            // HTML `img` elements only (not foreign elements of that name).
            let kind = CollectionKind::TagNameNs(HTML_NS.into(), "img".into());
            Ok(collections::collection(s, n.page(), n.id, kind)?.value())
        });
        def.getter("scripts", |s, n| {
            // HTML `script` elements only (not foreign elements of that name).
            let kind = CollectionKind::TagNameNs(HTML_NS.into(), "script".into());
            Ok(collections::collection(s, n.page(), n.id, kind)?.value())
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
        def.getter("referrer", |s, n| {
            let page = n.page();
            Ok(s.string(&if page.document() == n.id { page.document_referrer() } else { String::new() }))
        });
        // HTML `document.domain`. The getter is the origin's effective domain; the setter runs
        // the checks (no browsing context, no effective domain, not a registrable domain suffix:
        // SecurityError) and then has no effect, as every agent cluster is origin-keyed (Chromium's
        // default; `Origin-Agent-Cluster: ?0` is not modelled).
        def.accessor(
            "domain",
            |s, n| Ok(s.string(effective_domain(s, n).as_deref().unwrap_or(""))),
            |s, n, v| {
                let value = dom_string(v)?;
                let security = |m: &str| JsError::dom("SecurityError", format!("Failed to set the 'domain' property on 'Document': {m}"));
                if n.page().document() != n.id {
                    return Err(security("Assignment is forbidden for documents without a browsing context."));
                }
                let Some(effective) = effective_domain(s, n) else {
                    return Err(security("Assignment is forbidden for documents with an opaque origin."));
                };
                if !registrable_domain_suffix_or_equal(&value, &effective) {
                    return Err(security(&format!("'{value}' is not a suffix of '{effective}'.")));
                }
                // The surrounding agent's agent cluster is origin-keyed: nothing changes.
                Ok(())
            },
        );
        def.getter("location", |s, n| {
            Ok(if n.page().document() == n.id { s.global().get_atom("location")? } else { s.null() })
        });
        def.pure_method("getElementById", 1, |s, n, a| {
            let r = with_arg_str(&a, 0, "getElementById", |id| n.page().dom.borrow().get_element_by_id(n.id, id).js())?;
            node_value(s, n.page(), r)
        });
        def.pure_method("getElementsByTagName", 1, |s, n, a| {
            let name = arg_string(&a, 0, "getElementsByTagName")?;
            Ok(collections::collection(s, n.page(), n.id, CollectionKind::TagName(name))?.value())
        });
        def.pure_method("getElementsByTagNameNS", 2, |s, n, a| {
            a.require(2, "getElementsByTagNameNS")?;
            let namespace = opt_dom_string(a.get(0))?.unwrap_or_default();
            let local = dom_string(a.get(1))?;
            Ok(collections::collection(s, n.page(), n.id, CollectionKind::TagNameNs(namespace, local))?.value())
        });
        def.pure_method("getElementsByClassName", 1, |s, n, a| {
            let names = arg_string(&a, 0, "getElementsByClassName")?;
            Ok(collections::collection(s, n.page(), n.id, CollectionKind::ClassName(names))?.value())
        });
        def.pure_method("getElementsByName", 1, |s, n, a| {
            let name = arg_string(&a, 0, "getElementsByName")?;
            Ok(collections::collection(s, n.page(), n.id, CollectionKind::Name(name))?.value())
        });
        def.method("createElement", 1, |s, n, a| {
            a.require(1, "Document.createElement")?;
            let name = a.get(0);
            let el = match n.page().names.arg(s, &name)? {
                NameArg::Name(name) => n.page().dom.borrow_mut().create_element_named(n.id, &name),
                NameArg::Text(name) => n.page().dom.borrow_mut().create_element(n.id, &name),
            }
            .js()?;
            if let Some(is) = a
                .opt(1)
                .and_then(|o| o.as_object())
                .map(|o| o.get_atom("is"))
                .transpose()?
                .filter(|v| !v.is_undefined())
            {
                // DOM "create an element" with the options' `is`: the element's is value (no
                // `is` attribute), for an HTML element.
                let is = dom_string(is)?;
                let mut dom = n.page().dom.borrow_mut();
                if dom.element(el).js()?.is_html() {
                    dom.set_is_value(el, Some(pyro_dom::LocalName::from(is.as_str()))).js()?;
                }
            }
            crate::html::upgrade_if_custom(s, n.page(), n.id, el)
        });
        def.method("createElementNS", 2, |s, n, a| {
            a.require(2, "Document.createElementNS")?;
            let ns = opt_dom_string(a.get(0))?;
            let q = dom_string(a.get(1))?;
            let el = n.page().dom.borrow_mut().create_element_ns(n.id, ns.as_deref(), &q).js()?;
            if let Some(is) =
                a.opt(2).and_then(|o| o.as_object()).map(|o| o.get_atom("is")).transpose()?.filter(|v| !v.is_undefined())
            {
                let is = dom_string(is)?;
                let mut dom = n.page().dom.borrow_mut();
                if dom.element(el).js()?.is_html() {
                    dom.set_is_value(el, Some(pyro_dom::LocalName::from(is.as_str()))).js()?;
                }
            }
            crate::html::upgrade_if_custom(s, n.page(), n.id, el)
        });
        def.method("createTextNode", 1, |s, n, a| {
            let t = with_arg_str(&a, 0, "Document.createTextNode", |d| {
                n.page().dom.borrow_mut().create_text(n.id, d).js()
            })?;
            Ok(wrap_created_node(s, n.page(), n.id, t)?.value())
        });
        def.method("createComment", 1, |s, n, a| {
            let t = with_arg_str(&a, 0, "Document.createComment", |d| {
                n.page().dom.borrow_mut().create_comment(n.id, d).js()
            })?;
            Ok(wrap_created_node(s, n.page(), n.id, t)?.value())
        });
        def.method("createAttribute", 1, |s, n, a| {
            let name = arg_string(&a, 0, "Document.createAttribute")?;
            pyro_dom::dom::validate_attribute_name(&name).js()?;
            let html = n.page().dom.borrow().document_data(n.id).js()?.kind == DocumentKind::Html;
            let name = if html { name.to_ascii_lowercase() } else { name };
            // The whole name is the local name, colons included: no prefix.
            let name = pyro_dom::QualName::new(None, pyro_dom::Namespace::from(""), pyro_dom::LocalName::from(name));
            collections::make_detached_attr(s, n.page(), n.id, name)
        });
        def.method("createAttributeNS", 2, |s, n, a| {
            a.require(2, "Document.createAttributeNS")?;
            let ns = opt_dom_string(a.get(0))?;
            let qualified = dom_string(a.get(1))?;
            let qn = pyro_dom::dom::validate_and_extract(ns.as_deref(), &qualified, pyro_dom::dom::NameContext::Attribute)
                .js()?;
            collections::make_detached_attr(s, n.page(), n.id, qn)
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
            Ok(wrap_created_node(s, n.page(), n.id, f)?.value())
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
                Some(o) => !o.get_atom("selfOnly")?.to_bool(),
                None => a.get(1).to_bool(),
            };
            let c = n.page().dom.borrow_mut().clone_node(src, deep, Some(n.id)).js()?;
            crate::custom::upgrade_clone(s, n.page(), c);
            Ok(wrap_node(s, n.page(), c)?.value())
        });
        def.method("adoptNode", 1, |s, n, a| {
            let src = node_arg(s, a.get(0), "adoptNode")?;
            n.page().dom.borrow_mut().adopt_node(n.id, src).js()?;
            Ok(a.get(0))
        });
        def.method("elementFromPoint", 2, crate::html::element_from_point);
        def.method("elementsFromPoint", 2, crate::html::elements_from_point);
        // HTML `designMode`: "on" makes the whole document editable, and puts the selection at its
        // start (Chrome has a caret there in a focused document: WPT editing/crashtests
        // designMode-caret-change waits for the selectionchange of an edit made at it).
        def.accessor(
            "designMode",
            |s, n| Ok(s.string(if n.page().dom.borrow().design_mode(n.id) { "on" } else { "off" })),
            |_s, n, v| {
                let value = crate::util::dom_string(v)?.to_ascii_lowercase();
                let on = match value.as_str() {
                    "on" => true,
                    "off" => false,
                    _ => return Ok(()),
                };
                let was = n.page().dom.borrow().design_mode(n.id);
                n.page().dom.borrow_mut().set_design_mode(n.id, on).js()?;
                if on && !was && n.page().document() == n.id {
                    crate::editing::set_selection(n.page(), (n.id, 0), (n.id, 0));
                }
                Ok(())
            },
        );
        def.getter("activeViewTransition", crate::view_transition::active);
        def.method("startViewTransition", 0, |s, n, a| {
            let doc = wrap_node(s, n.page(), n.id)?.value();
            let root = n.page().dom.borrow().document_element(n.id).js()?;
            let root = node_value(s, n.page(), root)?;
            crate::view_transition::start(s, n.page(), doc, root, a.get(0))
        });
        def.method("getSelection", 0, |s, _n, _a| {
            s.global().get_atom("getSelection")?.as_object().map_or(Ok(s.null()), |f| f.call(None, &[]))
        });
        def.method("execCommand", 1, |s, n, a| {
            let command = arg_string(&a, 0, "Document.execCommand")?;
            let value = if a.get(2).is_undefined() { String::new() } else { dom_string(a.get(2))? };
            Ok(s.boolean(crate::edit_commands::exec_command(n.page(), &command, &value)))
        });
        def.method("queryCommandSupported", 1, |s, _n, a| {
            let command = arg_string(&a, 0, "Document.queryCommandSupported")?;
            Ok(s.boolean(crate::edit_commands::query_supported(&command)))
        });
        def.method("queryCommandEnabled", 1, |s, n, a| {
            let command = arg_string(&a, 0, "Document.queryCommandEnabled")?;
            Ok(s.boolean(crate::edit_commands::query_enabled(n.page(), &command)))
        });
        def.method("queryCommandState", 1, |s, n, a| {
            let command = arg_string(&a, 0, "Document.queryCommandState")?;
            Ok(s.boolean(crate::edit_commands::query_state(n.page(), &command)))
        });
        def.method("queryCommandIndeterm", 1, |s, n, a| {
            let command = arg_string(&a, 0, "Document.queryCommandIndeterm")?;
            Ok(s.boolean(crate::edit_commands::query_indeterm(n.page(), &command)))
        });
        def.method("queryCommandValue", 1, |s, n, a| {
            let command = arg_string(&a, 0, "Document.queryCommandValue")?;
            Ok(s.string(&crate::edit_commands::query_value(n.page(), &command)))
        });
        def.method("caretRangeFromPoint", 0, crate::editing_globals::caret_range_from_point);
        def.method("caretPositionFromPoint", 2, crate::editing_globals::caret_position_from_point);
        def.method("open", 0, crate::html::document_open);
        def.method("close", 0, |s, n, _a| crate::html::document_close(s, n));
        def.method("write", 0, |s, n, a| crate::html::document_write(s, n, a, false));
        def.method("writeln", 0, |s, n, a| crate::html::document_write(s, n, a, true));
        // CSS Font Loading: the document's `FontFaceSet` (font_loading.rs).
        def.getter("fonts", |s, n| crate::font_loading::fonts(s, n.page()));
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

pub(crate) fn register(engine: &pyro_js::Engine) {
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

/// This module's part of [`crate::thread_state::reset_thread`]: the match
/// buffer keeps its allocation and nothing else.
pub(crate) fn thread_leftover() -> Option<&'static str> {
    let mut found = FOUND.take();
    found.clear();
    FOUND.set(found);
    None
}


/// The effective domain of the document's origin (its host), `None` for an opaque origin. The
/// window's document has its realm's origin (an about:srcdoc or about:blank document inherits
/// its creator's); another document, its URL's.
fn effective_domain(s: &Scope<'_>, n: &NodeData) -> Option<String> {
    let origin = s.realm().origin();
    if origin.as_deref() == Some("null") {
        return None;
    }
    let source = match origin {
        Some(origin) if n.page().document() == n.id => origin.to_string(),
        _ => n.page().dom.borrow().document_data(n.id).ok()?.url.to_string(),
    };
    pyro_net::Url::parse(&source).ok()?.host_str().map(str::to_owned)
}

/// HTML "is a registrable domain suffix of or is equal to" (`host_suffix` against the
/// effective domain `original`).
fn registrable_domain_suffix_or_equal(host_suffix: &str, original: &str) -> bool {
    let Ok(parsed) = pyro_net::Url::parse(&format!("http://{host_suffix}/")) else { return false };
    let Some(suffix) = parsed.host_str().map(str::to_owned) else { return false };
    if suffix == original {
        return true;
    }
    let is_domain = |h: &str| !(h.starts_with('[') || h.parse::<std::net::Ipv4Addr>().is_ok());
    if !is_domain(&suffix) || !is_domain(original) {
        return false;
    }
    if !original.ends_with(&format!(".{suffix}")) {
        return false;
    }
    // A public suffix (`com`, `github.io`) cannot be the domain.
    if pyro_psl::suffix_str(&suffix) == Some(suffix.as_str()) {
        return false;
    }
    if pyro_psl::suffix_str(original).is_some_and(|ps| ps.ends_with(&suffix)) {
        return false;
    }
    true
}

/// The local name `setAttribute(name)` sets: `name` lowercased on an HTML element of an HTML
/// document (DOM), else as given.
fn setattribute_local<'a>(n: &NodeData, name: &'a str) -> std::borrow::Cow<'a, str> {
    let dom = n.page().dom.borrow();
    let html = dom.element(n.id).is_ok_and(|e| e.is_html())
        && dom.node_document(n.id).ok().and_then(|d| dom.document_data(d).ok()).is_some_and(|d| d.kind == pyro_dom::DocumentKind::Html);
    if html && name.bytes().any(|b| b.is_ascii_uppercase()) {
        std::borrow::Cow::Owned(name.to_ascii_lowercase())
    } else {
        std::borrow::Cow::Borrowed(name)
    }
}
