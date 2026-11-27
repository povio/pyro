//! NodeList, HTMLCollection, DOMTokenList, NamedNodeMap/Attr and
//! DOMStringMap. Live collections cache their member list against
//! `Dom::version()`; a later version keeps it when the change journal shows
//! nothing that can change its members (a change inside another subtree, a
//! text write under a child), so indexed loops stay O(1) per access while
//! the page mutates elsewhere. Member lists are also shared per page by
//! (root, kind): a second `getElementsByClassName('x')` reuses the first's.

use std::collections::HashMap;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use pyro_dom::{attr_name_hash, CollectionWatch, Dom, NodeId};
use pyro_js::{Args, ClassDef, IdentityKey, JsClass, JsError, JsResult, NoParent, Object, Scope, Value};

use crate::node::{wrap_node, NodeData};
use crate::page::PageDom;
use crate::util::*;

mod svg_aspect;
pub(crate) use svg_aspect::animated_aspect;

const COLL_NS: u64 = 1 << 61;

fn coll_identity(page: &PageDom, slot: u64, id: NodeId) -> IdentityKey {
    let base = page.node_identity(id);
    IdentityKey(base.0 | COLL_NS | (slot << 48), base.1)
}

/// Holds an external reference on a node for the lifetime of a wrapper.
pub struct NodeHold {
    pub page: Rc<PageDom>,
    pub id: NodeId,
}

impl NodeHold {
    pub fn new(page: &Rc<PageDom>, id: NodeId) -> JsResult<NodeHold> {
        page.dom.borrow_mut().add_ref(id).js()?;
        Ok(NodeHold { page: page.clone(), id })
    }
}

impl Drop for NodeHold {
    fn drop(&mut self) {
        self.page.release_node(self.id);
    }
}

/// Nodes of one page, each held as a [`NodeHold`] holds its node, with one
/// page reference and one DOM borrow for all of them.
pub struct NodeHolds {
    page: Rc<PageDom>,
    ids: Box<[NodeId]>,
}

impl NodeHolds {
    pub fn new(page: &Rc<PageDom>, ids: impl Into<Box<[NodeId]>>) -> JsResult<NodeHolds> {
        let ids = ids.into();
        page.dom.borrow_mut().add_refs(&ids).map_err(dom_err)?;
        Ok(NodeHolds { page: page.clone(), ids })
    }
}

impl Drop for NodeHolds {
    fn drop(&mut self) {
        self.page.release_nodes(&self.ids);
    }
}

fn page_rc(s: &Scope<'_>) -> JsResult<Rc<PageDom>> {
    crate::target::page_of(s)
}

// ----- NodeList -------------------------------------------------------------

pub enum NodeListSource {
    /// Static snapshot; every member is held (none for an empty list).
    Static(Option<NodeHolds>),
    /// Live `childNodes` of the held node.
    Children(NodeHold),
    /// Live controls of the held form with this id or name (`RadioNodeList`).
    FormNamed(NodeHold, String),
    /// Live elements of the held root's collection kind (`getElementsByName`).
    Members(NodeHold, CollectionKind),
}

pub struct NodeListData {
    source: NodeListSource,
    cache: RefCell<(u64, Vec<NodeId>)>,
}

/// `childNodes`: the root's children, text included.
fn child_nodes_watch() -> CollectionWatch {
    CollectionWatch { children_only: true, ..CollectionWatch::default() }
}

impl NodeListData {
    fn items(&self, dom: &Dom) -> Vec<NodeId> {
        match &self.source {
            NodeListSource::Static(v) => v.as_ref().map_or_else(Vec::new, |h| h.ids.to_vec()),
            NodeListSource::Children(h) => {
                let mut c = self.cache.borrow_mut();
                if c.0 != dom.version() {
                    if c.0 != u64::MAX && dom.collection_unchanged_since(h.id, c.0, &child_nodes_watch()) {
                        c.0 = dom.version();
                    } else {
                        c.1 = dom.children(h.id).map(|it| it.collect()).unwrap_or_default();
                        c.0 = dom.version();
                    }
                }
                c.1.clone()
            }
            NodeListSource::FormNamed(h, name) => form_candidates(&h.page, dom, h.id, name),
            NodeListSource::Members(h, kind) => shared_members(&h.page, dom, h.id, kind).to_vec(),
        }
    }
    fn len(&self) -> usize {
        match &self.source {
            NodeListSource::Static(v) => v.as_ref().map_or(0, |h| h.ids.len()),
            NodeListSource::Children(h) => h.page.dom.borrow().child_count(h.id).unwrap_or(0) as usize,
            NodeListSource::FormNamed(h, _) => self.items(&h.page.dom.borrow()).len(),
            NodeListSource::Members(h, kind) => shared_members(&h.page, &h.page.dom.borrow(), h.id, kind).len(),
        }
    }
    /// Member `i` and the page it belongs to (borrowed: an indexed read
    /// clones no `Rc`).
    fn get(&self, i: usize) -> Option<(&Rc<PageDom>, NodeId)> {
        match &self.source {
            NodeListSource::Static(v) => v.as_ref().and_then(|h| Some((&h.page, *h.ids.get(i)?))),
            NodeListSource::Children(h) => {
                let dom = h.page.dom.borrow();
                {
                    let c = self.cache.borrow();
                    if c.0 == dom.version() {
                        return c.1.get(i).map(|&id| (&h.page, id));
                    }
                }
                self.items(&dom);
                self.cache.borrow().1.get(i).map(|&id| (&h.page, id))
            }
            NodeListSource::FormNamed(h, _) => {
                let items = self.items(&h.page.dom.borrow());
                items.get(i).map(|&id| (&h.page, id))
            }
            NodeListSource::Members(h, kind) => {
                let id = *shared_members(&h.page, &h.page.dom.borrow(), h.id, kind).get(i)?;
                Some((&h.page, id))
            }
        }
    }
}

pub struct NodeList;
impl JsClass for NodeList {
    const NAME: &'static str = "NodeList";
    type Data = NodeListData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.pure_getter_typed("length", pyro_js::GetterResult::Number, |s, l| Ok(s.number(l.len() as f64)));
        def.pure_method("item", 1, |s, l, a| {
            a.require(1, "NodeList.item")?;
            let i = to_u32(a.get(0))? as usize;
            match l.get(i) {
                Some((p, id)) => Ok(wrap_node(s, p, id)?.value()),
                None => Ok(s.null()),
            }
        });
        def.legacy_platform_object();
        def.indexed(|s, l, i| match l.get(i as usize) {
            Some((p, id)) => Ok(Some(wrap_node(s, p, id)?.value())),
            None => Ok(None),
        });
        def.indexed_iterable();
    }
}

/// `form.name` when several controls share the name.
pub struct RadioNodeList;
impl JsClass for RadioNodeList {
    const NAME: &'static str = "RadioNodeList";
    type Data = NodeListData;
    type Parent = NodeList;
    fn define(def: &mut ClassDef<Self>) {
        def.accessor(
            "value",
            |s, l| {
                let NodeListSource::FormNamed(h, _) = &l.source else { return Ok(s.string("")) };
                let dom = h.page.dom.borrow();
                let checked = l.items(&dom).into_iter().find(|&n| is_radio(&dom, n) && dom.checked(n).unwrap_or(false));
                Ok(s.string(match checked {
                    Some(n) => dom.get_attribute(n, "value").js()?.unwrap_or("on"),
                    None => "",
                }))
            },
            |_s, l, v| {
                let value = dom_string(v)?;
                let NodeListSource::FormNamed(h, _) = &l.source else { return Ok(()) };
                let mut dom = h.page.dom.borrow_mut();
                let radio = l.items(&dom).into_iter().find(|&n| {
                    is_radio(&dom, n) && dom.get_attribute(n, "value").ok().flatten().unwrap_or("on") == value
                });
                match radio {
                    Some(n) => dom.set_checked(n, true).js(),
                    None => Ok(()),
                }
            },
        );
    }
}

fn is_radio(dom: &Dom, id: NodeId) -> bool {
    dom.element(id).is_ok_and(|e| {
        e.is_html() && &*e.name.local == "input" && e.attr("type").is_some_and(|t| t.eq_ignore_ascii_case("radio"))
    })
}

/// HTML "named elements" of a form: listed controls with this id or name, else
/// `<img>`s. Reuses the page's member lists, since a named getter that overrides
/// builtins runs on every property read of a form.
fn form_candidates(page: &PageDom, dom: &Dom, form: NodeId, name: &str) -> Vec<NodeId> {
    let named = |n: &NodeId| {
        dom.element(*n).is_ok_and(|e| e.id() == Some(name) || (e.is_html() && e.attr("name") == Some(name)))
    };
    let listed: Vec<NodeId> =
        shared_members(page, dom, form, &CollectionKind::FormElements).iter().copied().filter(named).collect();
    if !listed.is_empty() {
        return listed;
    }
    let images = shared_members(page, dom, form, &CollectionKind::TagName("img".to_owned()));
    images.iter().copied().filter(|n| dom.element(*n).is_ok_and(|e| e.is_html()) && named(n)).collect()
}

/// `HTMLFormElement`'s named getter: nothing, the one element, or a live
/// `RadioNodeList` (one object per form and name while it is alive).
pub(crate) fn form_named<'s>(s: &Scope<'s>, page: &PageDom, form: NodeId, name: &str) -> JsResult<Option<Value<'s>>> {
    if name.is_empty() {
        return Ok(None);
    }
    let candidates = form_candidates(page, &page.dom.borrow(), form, name);
    match candidates[..] {
        [] => Ok(None),
        [one] => Ok(Some(wrap_node(s, page, one)?.value())),
        _ => {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            std::hash::Hash::hash(name, &mut hasher);
            let base = coll_identity(page, 12, form);
            let key = IdentityKey(base.0, base.1 ^ std::hash::Hasher::finish(&hasher));
            if let Some(o) = s.lookup_identity(key) {
                return Ok(Some(o.value()));
            }
            let source = NodeListSource::FormNamed(NodeHold::new(&page_rc(s)?, form)?, name.to_owned());
            let data = NodeListData { source, cache: RefCell::new((u64::MAX, Vec::new())) };
            Ok(Some(s.wrap::<RadioNodeList>(data, Some(key))?.value()))
        }
    }
}

pub fn static_node_list<'s>(s: &Scope<'s>, page: &PageDom, ids: impl Into<Box<[NodeId]>>) -> JsResult<Object<'s>> {
    let page = page_rc(s).inspect(|p| {
        debug_assert_eq!(p.id(), page.id());
    })?;
    let ids = ids.into();
    // Empty static lists need no page reference.
    let holds = if ids.is_empty() { None } else { Some(NodeHolds::new(&page, ids)?) };
    s.wrap::<NodeList>(
        NodeListData { source: NodeListSource::Static(holds), cache: RefCell::new((0, Vec::new())) },
        None,
    )
}

/// `document.getElementsByName(name)`: a live `NodeList`.
pub fn elements_by_name<'s>(s: &Scope<'s>, page: &PageDom, root: NodeId, name: String) -> JsResult<Object<'s>> {
    let page = page_rc(s)?;
    let data = NodeListData {
        source: NodeListSource::Members(NodeHold::new(&page, root)?, CollectionKind::Name(name)),
        cache: RefCell::new((u64::MAX, Vec::new())),
    };
    s.wrap::<NodeList>(data, None)
}

/// A labelable element's `labels`: a live `NodeList`, the same object on each access.
pub fn labels<'s>(s: &Scope<'s>, page: &PageDom, id: NodeId) -> JsResult<Object<'s>> {
    let key = coll_identity(page, 10, id);
    if let Some(o) = s.lookup_identity(key) {
        return Ok(o);
    }
    let page = page_rc(s)?;
    let data = NodeListData {
        source: NodeListSource::Members(NodeHold::new(&page, id)?, CollectionKind::Labels),
        cache: RefCell::new((u64::MAX, Vec::new())),
    };
    s.wrap::<NodeList>(data, Some(key))
}

pub fn child_nodes<'s>(s: &Scope<'s>, page: &PageDom, id: NodeId) -> JsResult<Object<'s>> {
    let key = coll_identity(page, 1, id);
    if let Some(o) = s.lookup_identity(key) {
        return Ok(o);
    }
    let page = page_rc(s)?;
    let data = NodeListData {
        source: NodeListSource::Children(NodeHold::new(&page, id)?),
        cache: RefCell::new((u64::MAX, Vec::new())),
    };
    s.wrap::<NodeList>(data, Some(key))
}

// ----- HTMLCollection ---------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum CollectionKind {
    Children,
    /// `getElementsByTagName(name)`: elements whose qualified name is `name`
    /// (`*`: all). [`collection`] turns it into `TagNameHtml` where the DOM's
    /// "list of elements with qualified name" lowercases for HTML elements.
    TagName(String),
    /// `getElementsByTagName(name)` with ASCII uppercase letters in `name`,
    /// asked of a root whose node document was an HTML document at the time:
    /// HTML elements match `lower` (`name` in ASCII lowercase), elements of
    /// other namespaces `name` itself. The choice stays with the collection
    /// when its root later moves to an XML document.
    TagNameHtml {
        name: String,
        lower: String,
    },
    TagNameNs(String, String),
    ClassName(String),
    Name(String),
    /// The elements a Window property of this name collects (not iframes by
    /// name: their child window answers instead).
    WindowNamed(String),
    /// The elements a Document property of this name collects.
    DocumentNamed(String),
    Links,
    /// `document.anchors`: `<a>` elements with a `name`.
    Anchors,
    /// `document.applets`: always empty.
    Empty,
    /// `select.options` / `datalist.options`.
    Options,
    SelectedOptions,
    /// `form.elements` (listed elements owned by the form).
    FormElements,
    TableRows,
    TableBodies,
    RowCells,
    SectionRows,
    Labels,
}

pub struct CollectionData {
    root: NodeHold,
    kind: CollectionKind,
    cache: RefCell<(u64, Rc<Vec<NodeId>>)>,
}

/// Member lists of live collections by (root, kind) with the DOM version
/// they were computed at (`PageDom::collection_members`).
#[derive(Default)]
pub struct MemberCache(HashMap<(NodeId, CollectionKind), Members>);

/// A member list and the DOM version it was computed at.
type Members = (u64, Rc<Vec<NodeId>>);

/// Most (root, kind) member lists kept per page.
const MEMBER_CACHE_SIZE: usize = 256;

/// What decides membership, for kinds whose members the change journal can
/// vouch for; `None`: recomputed after any DOM change.
fn watch(kind: &CollectionKind) -> Option<CollectionWatch> {
    let subtree = |attrs: &[&str]| CollectionWatch {
        children_only: false,
        elements_only: true,
        attrs: attrs.iter().map(|a| attr_name_hash(a)).collect(),
        state: false,
    };
    Some(match kind {
        CollectionKind::Children => {
            CollectionWatch { children_only: true, elements_only: true, ..CollectionWatch::default() }
        }
        CollectionKind::TagName(_) | CollectionKind::TagNameHtml { .. } | CollectionKind::TagNameNs(..) => subtree(&[]),
        CollectionKind::ClassName(_) => subtree(&["class"]),
        CollectionKind::Name(_) => subtree(&["name"]),
        CollectionKind::WindowNamed(_) | CollectionKind::DocumentNamed(_) => subtree(&["id", "name"]),
        CollectionKind::Links => subtree(&["href"]),
        CollectionKind::Anchors => subtree(&["name"]),
        _ => return None,
    })
}

fn matches_kind(dom: &Dom, root: NodeId, id: NodeId, kind: &CollectionKind) -> bool {
    let Ok(e) = dom.element(id) else { return false };
    let html = e.is_html();
    let local = &*e.name.local;
    match kind {
        CollectionKind::TagName(t) => t == "*" || qualified_name_is(e, t),
        CollectionKind::TagNameHtml { name, lower } => qualified_name_is(e, if html { lower } else { name }),
        CollectionKind::TagNameNs(namespace, name) => {
            (namespace == "*" || namespace.as_str() == &*e.name.ns) && (name == "*" || name.as_str() == local)
        }
        CollectionKind::ClassName(names) => {
            // Quirks-mode documents compare class names ASCII
            // case-insensitively (DOM, "list of elements with class names").
            let quirks = dom
                .node_document(id)
                .ok()
                .and_then(|d| dom.document_data(d).ok())
                .is_some_and(|d| d.quirks == pyro_dom::QuirksMode::Quirks);
            let same = |a: &str, b: &str| if quirks { a.eq_ignore_ascii_case(b) } else { a == b };
            names.split_ascii_whitespace().next().is_some()
                && e.attr("class").is_some_and(|c| {
                    names.split_ascii_whitespace().all(|w| c.split_ascii_whitespace().any(|h| same(h, w)))
                })
        }
        // DOM getElementsByName: HTML elements only.
        CollectionKind::Name(n) => html && e.attr("name") == Some(n.as_str()),
        CollectionKind::WindowNamed(n) => {
            e.id() == Some(n.as_str())
                || (html && matches!(local, "embed" | "form" | "img" | "object") && e.attr("name") == Some(n.as_str()))
        }
        CollectionKind::DocumentNamed(n) => {
            (html
                && matches!(local, "embed" | "form" | "iframe" | "img" | "object")
                && e.attr("name") == Some(n.as_str()))
                || (html
                    && (local == "object" || (local == "img" && e.attr("name").is_some_and(|v| !v.is_empty())))
                    && e.id() == Some(n.as_str()))
        }
        CollectionKind::Links => html && matches!(local, "a" | "area") && e.attr("href").is_some(),
        CollectionKind::Anchors => html && local == "a" && e.attr("name").is_some(),
        CollectionKind::Empty => false,
        CollectionKind::FormElements => {
            let listed = html
                && matches!(local, "button" | "fieldset" | "input" | "object" | "output" | "select" | "textarea")
                // A form-associated custom element is a listed element once it is custom, not while
                // its constructor runs in the upgrade (HTML: custom element state "precustomized").
                || e.flags & (pyro_dom::node::element_flags::FORM_ASSOCIATED | pyro_dom::node::element_flags::CUSTOM_DEFINED)
                    == pyro_dom::node::element_flags::FORM_ASSOCIATED | pyro_dom::node::element_flags::CUSTOM_DEFINED;
            // HTML: a fieldset's `elements` are its listed descendants (whatever their form owner,
            // image buttons included); a form's are the listed elements it owns, but image buttons.
            if is_fieldset(dom, root) {
                listed
            } else {
                listed
                    && !(local == "input" && e.attr("type").is_some_and(|t| t.eq_ignore_ascii_case("image")))
                    && dom.form_owner(id).ok().flatten() == Some(root)
            }
        }
        _ => false,
    }
}

/// Whether the element's qualified name (`prefix:local`, else `local`) is
/// `wanted`, compared case-sensitively.
fn qualified_name_is(e: &pyro_dom::node::ElementData, wanted: &str) -> bool {
    let local = &*e.name.local;
    match e.name.prefix.as_deref() {
        Some(prefix) if !prefix.is_empty() => {
            wanted.strip_prefix(prefix).and_then(|rest| rest.strip_prefix(':')).is_some_and(|l| l == local)
        }
        _ => wanted == local,
    }
}

/// Whether `root` is an HTML `<fieldset>` (whose `elements` are its listed descendants).
fn is_fieldset(dom: &Dom, root: NodeId) -> bool {
    dom.element(root).is_ok_and(|e| e.is_html() && &*e.name.local == "fieldset")
}

fn compute(dom: &Dom, root: NodeId, kind: &CollectionKind) -> Vec<NodeId> {
    match kind {
        CollectionKind::Children => dom.element_children(root).map(|i| i.collect()).unwrap_or_default(),
        CollectionKind::Options => dom.select_options(root).unwrap_or_default(),
        CollectionKind::SelectedOptions => dom.selected_options(root).unwrap_or_default(),
        CollectionKind::TableRows
        | CollectionKind::TableBodies
        | CollectionKind::SectionRows
        | CollectionKind::RowCells => crate::html::table_collection(dom, root, kind),
        CollectionKind::Labels => crate::html::labels_for(dom, root),
        CollectionKind::FormElements if !is_fieldset(dom, root) => {
            dom.owned_form_elements(root).unwrap_or_default().into_iter().filter(|&n| matches_kind(dom, root, n, kind)).collect()
        }
        _ => dom
            .descendants(root)
            .map(|d| d.skip(1).filter(|&n| matches_kind(dom, root, n, kind)).collect())
            .unwrap_or_default(),
    }
}

/// The page's shared member list for (root, kind) at the DOM's version.
fn shared_members(page: &PageDom, dom: &Dom, root: NodeId, kind: &CollectionKind) -> Rc<Vec<NodeId>> {
    let version = dom.version();
    let watch = watch(kind);
    let still = |since: u64| {
        since != u64::MAX && watch.as_ref().is_some_and(|w| dom.collection_unchanged_since(root, since, w))
    };
    let mut shared = page.collection_members.borrow_mut();
    let key = (root, kind.clone());
    match shared.0.get_mut(&key) {
        Some(entry) if entry.0 == version || still(entry.0) => {
            entry.0 = version;
            entry.1.clone()
        }
        _ => {
            let members = Rc::new(compute(dom, root, kind));
            if shared.0.len() >= MEMBER_CACHE_SIZE {
                shared.0.clear();
            }
            shared.0.insert(key, (version, members.clone()));
            members
        }
    }
}

impl CollectionData {
    /// Brings the cached members up to the DOM's version: kept when the
    /// journal shows no change that can affect them, else taken from the
    /// page's shared list for (root, kind) or recomputed.
    fn refresh(&self, dom: &Dom) {
        let version = dom.version();
        let mut c = self.cache.borrow_mut();
        if c.0 == version {
            return;
        }
        let watch = watch(&self.kind);
        let still = |since: u64| {
            since != u64::MAX && watch.as_ref().is_some_and(|w| dom.collection_unchanged_since(self.root.id, since, w))
        };
        if still(c.0) {
            c.0 = version;
            return;
        }
        // Labels come from the DOM's own index (an ancestor walk and a map
        // lookup): cheaper than the shared list's lookup and eviction.
        let members = match self.kind {
            CollectionKind::Labels => Rc::new(compute(dom, self.root.id, &self.kind)),
            _ => shared_members(&self.root.page, dom, self.root.id, &self.kind),
        };
        *c = (version, members);
    }
    fn items(&self) -> Vec<NodeId> {
        let dom = self.root.page.dom.borrow();
        self.refresh(&dom);
        self.cache.borrow().1.to_vec()
    }
    fn with_items<R>(&self, f: impl FnOnce(&[NodeId]) -> R) -> R {
        let dom = self.root.page.dom.borrow();
        self.refresh(&dom);
        let members = self.cache.borrow().1.clone();
        f(&members)
    }
    fn named(&self, name: &str) -> Option<NodeId> {
        if name.is_empty() {
            return None;
        }
        let dom = self.root.page.dom.borrow();
        let items = {
            drop(dom);
            self.items()
        };
        let dom = self.root.page.dom.borrow();
        items.into_iter().find(|&n| {
            dom.element(n).is_ok_and(|e| e.id() == Some(name) || (e.is_html() && e.attr("name") == Some(name)))
        })
    }
    /// The supported property names (DOM, HTMLCollection): of each element
    /// in tree order its non-empty `id`, then its non-empty `name` when it is
    /// an HTML element, without duplicates.
    fn supported_names(&self) -> Vec<String> {
        let items = self.items();
        let dom = self.root.page.dom.borrow();
        let mut seen = std::collections::HashSet::new();
        let mut names = Vec::new();
        for id in items {
            let Ok(e) = dom.element(id) else { continue };
            let name = if e.is_html() { e.attr("name") } else { None };
            for n in [e.id(), name].into_iter().flatten() {
                if !n.is_empty() && seen.insert(n) {
                    names.push(n.to_owned());
                }
            }
        }
        names
    }
}

pub struct HtmlCollection;
impl JsClass for HtmlCollection {
    const NAME: &'static str = "HTMLCollection";
    type Data = CollectionData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.pure_getter_typed("length", pyro_js::GetterResult::Number, |s, c| {
            Ok(s.number(c.with_items(|v| v.len()) as f64))
        });
        def.pure_method("item", 1, |s, c, a| {
            a.require(1, "HTMLCollection.item")?;
            let i = to_u32(a.get(0))? as usize;
            let id = c.with_items(|v| v.get(i).copied());
            match id {
                Some(id) => Ok(wrap_node(s, &c.root.page, id)?.value()),
                None => Ok(s.null()),
            }
        });
        def.pure_method("namedItem", 1, |s, c, a| {
            a.require(1, "HTMLCollection.namedItem")?;
            let name = dom_string(a.get(0))?;
            match c.named(&name) {
                Some(id) => Ok(wrap_node(s, &c.root.page, id)?.value()),
                None => Ok(s.null()),
            }
        });
        def.indexed(|s, c, i| match c.with_items(|v| v.get(i as usize).copied()) {
            Some(id) => Ok(Some(wrap_node(s, &c.root.page, id)?.value())),
            None => Ok(None),
        });
        def.named(false, |s, c, name| match c.named(name) {
            Some(id) => Ok(Some(wrap_node(s, &c.root.page, id)?.value())),
            None => Ok(None),
        });
        def.unenumerable_property_names(|_s, c| Ok(c.supported_names()));
        // No `iterable<>` declaration: @@iterator alone.
        def.indexed_iterator();
        def.legacy_platform_object();
    }
}

/// `HTMLOptionsCollection.add(element, before)` and `HTMLSelectElement.add`
/// (HTML §2.7.3): `before` is an element, an index into the options or
/// null; `element` must not be an ancestor of the select, and an element
/// `before` must be a descendant of it.
pub(crate) fn options_add<'s>(
    s: &Scope<'s>,
    page: &Rc<PageDom>,
    select: NodeId,
    a: Args<'s>,
    iface: &str,
) -> JsResult<()> {
    let el = crate::node::node_arg(s, a.get(0), &format!("{iface}.add"))?;
    let before = a.get(1);
    let mut dom = page.dom.borrow_mut();
    let is_html = |dom: &pyro_dom::Dom, n: NodeId, names: &[&str]| {
        dom.element(n).is_ok_and(|e| e.is_html() && names.contains(&&*e.name.local))
    };
    if !is_html(&dom, el, &["option", "optgroup"]) {
        return Err(JsError::Type(format!(
            "Failed to execute 'add' on '{iface}': The provided value is not of type '(HTMLOptGroupElement or HTMLOptionElement)'."
        ).into()));
    }
    if dom.contains(el, select).js()? {
        return Err(JsError::dom("HierarchyRequestError", "The new child element contains the parent."));
    }
    let reference = if let Some(n) = s.unwrap::<crate::node::Node>(before) {
        if n.id == select || !dom.contains(select, n.id).js()? {
            return Err(JsError::dom(
                "NotFoundError",
                "The node before which the new node is to be inserted is not a child of this node.",
            ));
        }
        Some(n.id)
    } else if before.is_nullish() {
        None
    } else {
        let index = to_i32(before)?;
        let opts = dom.select_options(select).js()?;
        usize::try_from(index).ok().and_then(|i| opts.get(i).copied())
    };
    if reference == Some(el) {
        return Ok(());
    }
    let parent = match reference {
        Some(r) => dom.parent(r).js()?.unwrap_or(select),
        None => select,
    };
    dom.insert_before(parent, el, reference).js()?;
    Ok(())
}

/// Setting `HTMLOptionsCollection.length` and `HTMLSelectElement.length`
/// (HTML §2.7.3): a longer length appends options without attributes or
/// children to the select, a shorter one removes the last options of the
/// collection from their parents. As browsers, a length past 100,000 options
/// does nothing.
pub(crate) fn options_set_length(page: &Rc<PageDom>, select: NodeId, v: Value<'_>) -> JsResult<()> {
    const MAX_OPTIONS: usize = 100_000;
    let length = to_u32(v)? as usize;
    let mut dom = page.dom.borrow_mut();
    let options = dom.select_options(select).js()?;
    if length > options.len() {
        if length > MAX_OPTIONS {
            return Ok(());
        }
        let doc = dom.node_document(select).js()?;
        for _ in options.len()..length {
            let option = dom.create_element_ns(doc, Some("http://www.w3.org/1999/xhtml"), "option").js()?;
            dom.insert_before(select, option, None).js()?;
        }
    } else {
        for &option in options[length..].iter().rev() {
            dom.remove_node(option).js()?;
        }
    }
    Ok(())
}

/// `form.elements` / `fieldset.elements` (HTML §2.7.2): `namedItem` (and the named getter)
/// gives the one listed element with that id or name, or a live `RadioNodeList` of several.
pub struct HtmlFormControlsCollection;
impl JsClass for HtmlFormControlsCollection {
    const NAME: &'static str = "HTMLFormControlsCollection";
    type Data = CollectionData;
    type Parent = HtmlCollection;
    fn define(def: &mut ClassDef<Self>) {
        def.pure_method("namedItem", 1, |s, c, a| {
            a.require(1, "HTMLFormControlsCollection.namedItem")?;
            let name = dom_string(a.get(0))?;
            Ok(controls_named(s, c, &name)?.unwrap_or_else(|| s.null()))
        });
        def.named(false, |s, c, name| controls_named(s, c, name));
    }
}

fn controls_named<'s>(s: &Scope<'s>, c: &CollectionData, name: &str) -> JsResult<Option<Value<'s>>> {
    if name.is_empty() {
        return Ok(None);
    }
    let items = c.items();
    let matches: Vec<NodeId> = {
        let dom = c.root.page.dom.borrow();
        items
            .iter()
            .copied()
            .filter(|&n| dom.element(n).is_ok_and(|e| e.id() == Some(name) || (e.is_html() && e.attr("name") == Some(name))))
            .collect()
    };
    match matches[..] {
        [] => Ok(None),
        [one] => Ok(Some(wrap_node(s, &c.root.page, one)?.value())),
        // The listed elements are non-empty: `form_named` gives their RadioNodeList.
        _ => form_named(s, &c.root.page, c.root.id, name),
    }
}

pub struct HtmlOptionsCollection;
impl JsClass for HtmlOptionsCollection {
    const NAME: &'static str = "HTMLOptionsCollection";
    type Data = CollectionData;
    type Parent = HtmlCollection;
    fn define(def: &mut ClassDef<Self>) {
        def.accessor(
            "length",
            |s, c| Ok(s.number(c.root.page.dom.borrow().select_options(c.root.id).js()?.len() as f64)),
            |_s, c, v| options_set_length(&c.root.page, c.root.id, v),
        );
        def.accessor(
            "selectedIndex",
            |s, c| Ok(s.number(c.root.page.dom.borrow().selected_index(c.root.id).js()? as f64)),
            |_s, c, v| {
                let arg0 = to_i32(v)?;
                c.root.page.dom.borrow_mut().set_selected_index(c.root.id, arg0).js()
            },
        );
        def.method("add", 1, |s, c, a| {
            options_add(s, &c.root.page, c.root.id, a, "HTMLOptionsCollection")?;
            Ok(s.undefined())
        });
        def.method("remove", 1, |s, c, a| {
            let i = to_i32(a.get(0))?;
            let mut dom = c.root.page.dom.borrow_mut();
            let opts = dom.select_options(c.root.id).js()?;
            if let Some(&o) = usize::try_from(i).ok().and_then(|i| opts.get(i)) {
                dom.remove_node(o).js()?;
            }
            Ok(s.undefined())
        });
        // The indexed property setter (HTML, "set the value of a new indexed
        // property"); the class is the one collection whose indices can be
        // written. Other names: a visible named property stays read-only,
        // anything else becomes an own property.
        def.named_setter(|s, c, name, v| {
            let Some(index) = name.parse::<u32>().ok().filter(|i| *i != u32::MAX && i.to_string() == name) else {
                return Ok(c.named(name).is_some());
            };
            let index = index as usize;
            // Browsers refuse to grow a select past this many options.
            const MAX_OPTIONS: usize = 100_000;
            let mut dom = c.root.page.dom.borrow_mut();
            let options = dom.select_options(c.root.id).js()?;
            if index >= MAX_OPTIONS && index >= options.len() {
                return Ok(true);
            }
            if v.is_nullish() {
                if let Some(&option) = options.get(index) {
                    dom.remove_node(option).js()?;
                }
                return Ok(true);
            }
            let option = s
                .unwrap::<crate::node::Node>(v)
                .map(|n| n.id)
                .filter(|&id| dom.element(id).is_ok_and(|e| e.is_html() && &*e.name.local == "option"))
                .ok_or_else(|| {
                    JsError::Type(
                        "Failed to set an indexed property on 'HTMLOptionsCollection': not an HTMLOptionElement."
                            .into(),
                    )
                })?;
            match options.get(index) {
                Some(&old) => {
                    if old != option {
                        let parent = dom.parent(old).js()?.unwrap_or(c.root.id);
                        dom.replace_child(parent, option, old).js()?;
                    }
                }
                None => {
                    // Options without attributes or children fill the gap.
                    let doc = dom.node_document(c.root.id).js()?;
                    for _ in options.len()..index {
                        let filler = dom.create_element_ns(doc, Some("http://www.w3.org/1999/xhtml"), "option").js()?;
                        dom.insert_before(c.root.id, filler, None).js()?;
                    }
                    dom.insert_before(c.root.id, option, None).js()?;
                }
            }
            Ok(true)
        });
    }
}

fn kind_slot(kind: &CollectionKind) -> Option<u64> {
    Some(match kind {
        CollectionKind::Children => 2,
        CollectionKind::Options => 3,
        CollectionKind::SelectedOptions => 4,
        CollectionKind::FormElements => 5,
        CollectionKind::TableRows => 6,
        CollectionKind::TableBodies => 7,
        CollectionKind::RowCells => 8,
        CollectionKind::SectionRows => 9,
        CollectionKind::Labels => 10,
        CollectionKind::Links => 11,
        // pyro: `document.anchors` and `document.applets` are [SameObject].
        CollectionKind::Anchors => 62,
        CollectionKind::Empty => 63,
        _ => return None,
    })
}

/// Live collection over `root` (cached wrapper per element for fixed kinds).
pub fn collection<'s>(s: &Scope<'s>, page: &PageDom, root: NodeId, kind: CollectionKind) -> JsResult<Object<'s>> {
    // A named-property collection (`document.foo`, `window.foo` with several elements) is the
    // same object on each access while script holds it: its key folds in a hash of the name,
    // and a found wrapper is checked to be for this name (a collision gets an uncached one).
    // pyro: so are `getElementsByTagName(NS)` collections and the Document's [SameObject] ones
    // built from them (`document.embeds` and `document.plugins` are one object): by the name, the
    // namespace and the name for a namespaced one.
    let ns_name;
    let named = match &kind {
        CollectionKind::DocumentNamed(n) => Some((30, n)),
        CollectionKind::WindowNamed(n) => Some((31, n)),
        CollectionKind::TagName(n) => Some((60, n)),
        CollectionKind::TagNameNs(ns, n) => {
            ns_name = format!("{ns}\u{0}{n}");
            Some((61, &ns_name))
        }
        _ => None,
    };
    let mut key = match named {
        Some((slot, name)) => {
            let base = coll_identity(page, slot, root);
            let hash =
                name.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
            Some(IdentityKey(base.0, base.1 ^ hash))
        }
        None => kind_slot(&kind).map(|k| coll_identity(page, k, root)),
    };
    if let Some(k) = key {
        if let Some(o) = s.lookup_identity(k) {
            if named.is_none()
                || s.unwrap::<HtmlCollection>(o.value()).is_some_and(|d| d.kind == kind && d.root.id == root)
            {
                return Ok(o);
            }
            key = None;
        }
    }
    let page = page_rc(s)?;
    let options = kind == CollectionKind::Options;
    // HTML: form.elements is an HTMLFormControlsCollection, fieldset.elements a plain HTMLCollection.
    let controls = kind == CollectionKind::FormElements && !is_fieldset(&page.dom.borrow(), root);
    let kind = match kind {
        // DOM "list of elements with qualified name": whether HTML elements
        // are matched against the lowercased name is decided here, by the
        // root's node document.
        CollectionKind::TagName(name) if name.bytes().any(|b| b.is_ascii_uppercase()) => {
            let dom = page.dom.borrow();
            let html_document = dom
                .node_document(root)
                .ok()
                .and_then(|d| dom.document_data(d).ok())
                .is_some_and(|d| d.kind == pyro_dom::DocumentKind::Html);
            if html_document {
                let lower = name.to_ascii_lowercase();
                CollectionKind::TagNameHtml { name, lower }
            } else {
                CollectionKind::TagName(name)
            }
        }
        kind => kind,
    };
    let data =
        CollectionData { root: NodeHold::new(&page, root)?, kind, cache: RefCell::new((u64::MAX, Rc::default())) };
    if options {
        s.wrap::<HtmlOptionsCollection>(data, key)
    } else if controls {
        s.wrap::<HtmlFormControlsCollection>(data, key)
    } else {
        s.wrap::<HtmlCollection>(data, key)
    }
}

pub fn children<'s>(s: &Scope<'s>, page: &PageDom, id: NodeId) -> JsResult<Object<'s>> {
    collection(s, page, id, CollectionKind::Children)
}

// ----- DOMTokenList ---------------------------------------------------------

pub struct TokenListData {
    el: NodeHold,
    attr: &'static str,
}

impl TokenListData {
    /// The token set: the attribute's tokens in order, without duplicates,
    /// borrowed from `dom`. `class` has them as the element's atoms.
    fn tokens_in<'d>(&self, dom: &'d pyro_dom::Dom) -> Vec<&'d str> {
        let mut out: Vec<&'d str> = Vec::new();
        let mut add = |t: &'d str| {
            if !out.contains(&t) {
                out.push(t);
            }
        };
        if self.attr == "class" {
            if let Ok(e) = dom.element(self.el.id) {
                e.classes().iter().for_each(|c| add(c));
            }
        } else {
            dom.get_attribute_ns(self.el.id, None, self.attr)
                .ok()
                .flatten()
                .unwrap_or("")
                .split_ascii_whitespace()
                .for_each(add);
        }
        out
    }
    /// Token `i` of the set as a string: a `class` token is the element's
    /// atom, so its realm's one string for it.
    fn token<'s>(&self, s: &Scope<'s>, dom: &pyro_dom::Dom, i: usize) -> Option<Value<'s>> {
        if self.attr == "class" {
            let classes = dom.element(self.el.id).ok()?.classes();
            let mut set = classes.iter().enumerate().filter(|&(k, c)| !classes[..k].contains(c));
            return set.nth(i).map(|(_, c)| c.js(s));
        }
        self.tokens_in(dom).get(i).map(|x| s.string(x))
    }
    /// The attribute's value (`value`, `toString`): `class` as `className`.
    fn value<'s>(&self, s: &Scope<'s>) -> JsResult<Value<'s>> {
        let dom = self.el.page.dom.borrow();
        if self.attr == "class" {
            return Ok(crate::util::class_name(s, dom.element(self.el.id).js()?));
        }
        Ok(s.string(dom.get_attribute_ns(self.el.id, None, self.attr).js()?.unwrap_or("")))
    }
    fn contains(&self, token: &str) -> bool {
        let dom = self.el.page.dom.borrow();
        if self.attr == "class" {
            return dom.element(self.el.id).is_ok_and(|e| e.classes().iter().any(|c| &**c == token));
        }
        let value = dom.get_attribute_ns(self.el.id, None, self.attr).ok().flatten().unwrap_or("");
        value.split_ascii_whitespace().any(|t| t == token)
    }
    /// The "update steps" with `tokens` serialized: an absent attribute
    /// stays absent for an empty set. The token list's attribute is the one
    /// in no namespace, read and written (DOM "set an attribute value"): an
    /// attribute of that name in another namespace is left alone.
    fn write(&self, empty: bool, value: &str) -> JsResult<()> {
        let mut dom = self.el.page.dom.borrow_mut();
        if empty && dom.get_attribute_ns(self.el.id, None, self.attr).js()?.is_none() {
            return Ok(());
        }
        dom.set_attribute_ns(self.el.id, None, self.attr, value).js()
    }
}

/// Runs `$f` on the token set of `$t` (a `&mut Vec<&str>`) and writes the
/// result: the tokens stay borrowed from the DOM and the arguments.
macro_rules! update_tokens {
    ($t:expr, |$toks:ident| $f:expr) => {{
        let (empty, value) = {
            let dom = $t.el.page.dom.borrow();
            let mut $toks = $t.tokens_in(&dom);
            $f;
            ($toks.is_empty(), $toks.join(" "))
        };
        $t.write(empty, &value)
    }};
}

fn check_tokens<T: AsRef<str>>(tokens: &[T]) -> JsResult<()> {
    if let Some(t) = tokens.iter().map(AsRef::as_ref).find(|t| t.is_empty()) {
        return Err(JsError::dom("SyntaxError", format!("The token provided ('{t}') must not be empty.")));
    }
    tokens.iter().try_for_each(|t| check_token(t.as_ref()))
}

fn check_token(t: &str) -> JsResult<()> {
    if t.is_empty() {
        return Err(JsError::dom("SyntaxError", "The token provided must not be empty."));
    }
    if t.contains(|c: char| c.is_ascii_whitespace()) {
        return Err(JsError::dom(
            "InvalidCharacterError",
            format!("The token provided ('{t}') contains HTML space characters, which are not valid in tokens."),
        ));
    }
    Ok(())
}

pub struct DomTokenList;
impl JsClass for DomTokenList {
    const NAME: &'static str = "DOMTokenList";
    type Data = TokenListData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.pure_getter_typed("length", pyro_js::GetterResult::Number, |s, t| {
            Ok(s.number(t.tokens_in(&t.el.page.dom.borrow()).len() as f64))
        });
        def.accessor(
            "value",
            |s, t| t.value(s),
            |_s, t, v| {
                with_str(v, |value| t.el.page.dom.borrow_mut().set_attribute_ns(t.el.id, None, t.attr, value).js())
            },
        );
        def.pure_method("toString", 0, |s, t, _a| t.value(s));
        def.pure_method("item", 1, |s, t, a| {
            a.require(1, "DOMTokenList.item")?;
            let i = to_u32(a.get(0))? as usize;
            Ok(t.token(s, &t.el.page.dom.borrow(), i).unwrap_or_else(|| s.null()))
        });
        def.legacy_platform_object();
        def.indexed(|s, t, i| Ok(t.token(s, &t.el.page.dom.borrow(), i as usize)));

        def.pure_method("contains", 1, |s, t, a| {
            a.require(1, "DOMTokenList.contains")?;
            let v = a.get(0);
            Ok(s.boolean(t.contains(&str_of(&v)?)))
        });
        def.method("add", 0, |s, t, a| {
            let values: Vec<Value<'_>> = a.iter().collect();
            let args = values.iter().map(str_of).collect::<JsResult<Vec<_>>>()?;
            check_tokens(&args)?;
            update_tokens!(t, |toks| {
                for tok in &args {
                    if !toks.contains(&tok.as_ref()) {
                        toks.push(tok);
                    }
                }
            })?;
            Ok(s.undefined())
        });
        def.method("remove", 0, |s, t, a| {
            let values: Vec<Value<'_>> = a.iter().collect();
            let args = values.iter().map(str_of).collect::<JsResult<Vec<_>>>()?;
            check_tokens(&args)?;
            update_tokens!(t, |toks| toks.retain(|x| !args.iter().any(|a| a == x)))?;
            Ok(s.undefined())
        });
        def.method("toggle", 1, |s, t, a| {
            a.require(1, "DOMTokenList.toggle")?;
            let v = a.get(0);
            let tok = str_of(&v)?;
            check_token(&tok)?;
            let force = a.opt(1).filter(|v| !v.is_undefined()).map(|v| v.to_bool());
            let present = t.contains(&tok);
            let result = match (present, force) {
                (true, None | Some(false)) => {
                    update_tokens!(t, |toks| toks.retain(|x| *x != tok))?;
                    false
                }
                (false, None | Some(true)) => {
                    update_tokens!(t, |toks| toks.push(&tok))?;
                    true
                }
                (p, _) => p,
            };
            Ok(s.boolean(result))
        });
        def.method("replace", 2, |s, t, a| {
            a.require(2, "DOMTokenList.replace")?;
            let old = dom_string(a.get(0))?;
            let new = dom_string(a.get(1))?;
            check_tokens(&[old.as_str(), new.as_str()])?;
            if !t.contains(&old) {
                return Ok(s.boolean(false));
            }
            update_tokens!(t, |toks| {
                // Infra "replace" in an ordered set: the first of either token
                // becomes the new one, every other instance of both goes.
                if let Some(first) = toks.iter().position(|x| *x == old || *x == new) {
                    let mut at = 0;
                    toks.retain(|x| {
                        let keep = at == first || (*x != old && *x != new);
                        at += 1;
                        keep
                    });
                    if let Some(slot) = toks.iter_mut().find(|x| **x == old) {
                        *slot = &new;
                    }
                }
            })?;
            Ok(s.boolean(true))
        });
        def.pure_method("supports", 1, |s, t, a| {
            let tok = dom_string(a.get(0))?;
            // `rel` per element, as Chromium answers: a `<link>`'s fetch and resource types;
            // `<a>`, `<area>` and `<form>`'s opener and referrer types.
            let rel_of = |t: &TokenListData| {
                // (An SVG `<a>` has `<a>`'s: SVGAElement.relList is the same DOMTokenList.)
                t.el.page.dom.borrow().element(t.el.id).ok().filter(|e| e.is_html() || (e.namespace() == &markup5ever::ns!(svg) && &*e.name.local == "a")).map(|e| match &*e.name.local {
                    "link" => 0,
                    "a" | "area" | "form" => 1,
                    _ => 2,
                })
            };
            let supported: &[&str] = match t.attr {
                "rel" if rel_of(t) == Some(0) => &[
                    "modulepreload",
                    "preload",
                    "preconnect",
                    "dns-prefetch",
                    "stylesheet",
                    "icon",
                    "alternate",
                    "prefetch",
                    "prerender",
                    "next",
                    "manifest",
                    "apple-touch-icon",
                    "apple-touch-icon-precomposed",
                    "canonical",
                    "expect",
                    "compression-dictionary",
                ],
                "rel" if rel_of(t) == Some(1) => &["noreferrer", "noopener", "opener"],
                "rel" => &[
                    "alternate",
                    "canonical",
                    "author",
                    "bookmark",
                    "dns-prefetch",
                    "expect",
                    "external",
                    "help",
                    "icon",
                    "manifest",
                    "modulepreload",
                    "license",
                    "next",
                    "nofollow",
                    "noopener",
                    "noreferrer",
                    "opener",
                    "pingback",
                    "preconnect",
                    "prefetch",
                    "preload",
                    "prev",
                    "privacy-policy",
                    "search",
                    "stylesheet",
                    "tag",
                    "terms-of-service",
                ],
                "sandbox" => &[
                    "allow-downloads",
                    "allow-forms",
                    "allow-modals",
                    "allow-orientation-lock",
                    "allow-pointer-lock",
                    "allow-popups",
                    "allow-popups-to-escape-sandbox",
                    "allow-presentation",
                    "allow-same-origin",
                    "allow-scripts",
                    "allow-storage-access-by-user-activation",
                    "allow-top-navigation",
                    "allow-top-navigation-by-user-activation",
                    "allow-top-navigation-to-custom-protocols",
                ],
                "blocking" => &["render"],
                // Scoped focusgroup tokens (WPT focusgroup/tentative/idl-reflection.html).
                // Chromium's media controlsList (WICG): what `supports()` answers there.
                "controlslist" => &["nodownload", "nofullscreen", "noplaybackrate", "noremoteplayback"],
                "focusgroup" => &[
                    "toolbar",
                    "tablist",
                    "radiogroup",
                    "listbox",
                    "menu",
                    "menubar",
                    "wrap",
                    "nowrap",
                    "inline",
                    "block",
                    "nomemory",
                    "none",
                    "feed",
                    "grid",
                    "itemcontrols",
                    "noitemcontrols",
                    "manual",
                    "rowwrap",
                    "colwrap",
                    "flow",
                    "rowflow",
                    "colflow",
                ],
                _ => {
                    return Err(JsError::Type(format!(
                        "DOMTokenList has no supported tokens for the '{}' attribute.",
                        t.attr
                    ).into()))
                }
            };
            Ok(s.boolean(supported.iter().any(|k| k.eq_ignore_ascii_case(&tok))))
        });
        def.indexed_iterable();
    }
}

pub fn token_list<'s>(s: &Scope<'s>, page: &PageDom, id: NodeId, attr: &'static str) -> JsResult<Object<'s>> {
    let slot = match attr {
        "class" => 12,
        "part" => 13,
        "rel" => 14,
        "sandbox" => 15,
        // `link` has both `sizes` (16) and `blocking`.
        "blocking" => 17,
        "focusgroup" => 18,
        _ => 16,
    };
    let key = coll_identity(page, slot, id);
    if let Some(o) = s.lookup_identity(key) {
        return Ok(o);
    }
    let page = page_rc(s)?;
    s.wrap::<DomTokenList>(TokenListData { el: NodeHold::new(&page, id)?, attr }, Some(key))
}

// ----- NamedNodeMap / Attr ------------------------------------------------------

pub struct NamedNodeMapData {
    el: NodeHold,
}

/// Identity of an attribute on its element: (namespace, local name).
#[derive(Clone)]
pub(crate) struct AttrName {
    /// Empty for no namespace.
    pub ns: String,
    pub local: String,
}

impl AttrName {
    pub(crate) fn of(a: &pyro_dom::Attr) -> AttrName {
        AttrName { ns: a.name.ns.to_string(), local: a.name.local.to_string() }
    }
    pub(crate) fn ns(&self) -> Option<&str> {
        (!self.ns.is_empty()).then_some(self.ns.as_str())
    }
    /// Index of this attribute in `attrs`.
    pub(crate) fn find(&self, attrs: &[pyro_dom::Attr]) -> Option<usize> {
        attrs.iter().position(|a| *a.name.ns == *self.ns && *a.name.local == *self.local)
    }
}

/// Attribute of `el` with qualified name `name` (lowercased on an HTML
/// element of an HTML document, like `getAttribute`).
fn attr_named(page: &PageDom, el: NodeId, name: &str) -> JsResult<Option<AttrName>> {
    let dom = page.dom.borrow();
    let at = dom.attribute_index_qualified(el, name).js()?;
    Ok(at.map(|k| AttrName::of(&dom.attributes(el).unwrap_or_default()[k])))
}

pub(crate) fn attr_named_qualified(page: &PageDom, el: NodeId, name: &str) -> JsResult<Option<AttrName>> {
    attr_named(page, el, name)
}

/// Attribute of `el` by namespace and local name.
pub(crate) fn attr_named_ns(page: &PageDom, el: NodeId, ns: Option<&str>, local: &str) -> JsResult<Option<AttrName>> {
    let dom = page.dom.borrow();
    Ok(dom
        .attributes(el)
        .js()?
        .iter()
        .find(|x| &*x.name.ns == ns.unwrap_or("") && *x.name.local == *local)
        .map(AttrName::of))
}

pub struct NamedNodeMap;
impl JsClass for NamedNodeMap {
    const NAME: &'static str = "NamedNodeMap";
    type Data = NamedNodeMapData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.getter("length", |s, m| Ok(s.number(m.el.page.dom.borrow().attributes(m.el.id).js()?.len() as f64)));
        def.pure_method("item", 1, |s, m, a| {
            let i = to_u32(a.get(0))? as usize;
            attr_at(s, m, i).map(|o| o.unwrap_or(s.null()))
        });
        def.indexed(|s, m, i| attr_at(s, m, i as usize));
        def.method("getNamedItem", 1, |s, m, a| {
            let name = dom_string(a.get(0))?;
            match attr_named(&m.el.page, m.el.id, &name)? {
                Some(n) => make_attr(s, &m.el.page, m.el.id, n),
                None => Ok(s.null()),
            }
        });
        def.method("getNamedItemNS", 2, |s, m, a| {
            let ns = opt_dom_string(a.get(0))?;
            let local = dom_string(a.get(1))?;
            match attr_named_ns(&m.el.page, m.el.id, ns.as_deref(), &local)? {
                Some(n) => make_attr(s, &m.el.page, m.el.id, n),
                None => Ok(s.null()),
            }
        });
        def.method("setNamedItem", 1, |s, m, a| set_attribute_node(s, &m.el.page, m.el.id, a.get(0)));
        def.method("setNamedItemNS", 1, |s, m, a| set_attribute_node(s, &m.el.page, m.el.id, a.get(0)));
        def.method("removeNamedItem", 1, |s, m, a| {
            let name = dom_string(a.get(0))?;
            let Some(found) = attr_named(&m.el.page, m.el.id, &name)? else {
                return Err(JsError::dom("NotFoundError", format!("No item with name '{name}' was found.")));
            };
            let attr = make_attr(s, &m.el.page, m.el.id, found.clone())?;
            m.el.page.dom.borrow_mut().remove_attribute_ns(m.el.id, found.ns(), &found.local).js()?;
            Ok(attr)
        });
        def.method("removeNamedItemNS", 2, |s, m, a| {
            let ns = opt_dom_string(a.get(0))?;
            let local = dom_string(a.get(1))?;
            let Some(found) = attr_named_ns(&m.el.page, m.el.id, ns.as_deref(), &local)? else {
                return Err(JsError::dom("NotFoundError", format!("No item with name '{local}' was found.")));
            };
            let attr = make_attr(s, &m.el.page, m.el.id, found.clone())?;
            m.el.page.dom.borrow_mut().remove_attribute_ns(m.el.id, found.ns(), &found.local).js()?;
            Ok(attr)
        });
        def.named(false, |s, m, name| match attr_named(&m.el.page, m.el.id, name)? {
            Some(n) => Ok(Some(make_attr(s, &m.el.page, m.el.id, n)?)),
            None => Ok(None),
        });
        // The supported property names (DOM, NamedNodeMap): the attributes'
        // qualified names without duplicates; on an HTML element of an HTML
        // document not those with ASCII uppercase letters, which the named
        // getter (it lowercases) never finds.
        def.unenumerable_property_names(|_s, m| {
            let dom = m.el.page.dom.borrow();
            let lowercase_only = dom.element(m.el.id).js()?.is_html()
                && dom
                    .node_document(m.el.id)
                    .ok()
                    .and_then(|d| dom.document_data(d).ok())
                    .is_some_and(|d| d.kind == pyro_dom::DocumentKind::Html);
            let mut names: Vec<String> = Vec::new();
            for a in dom.attributes(m.el.id).js()? {
                let name = a.qualified_name();
                if !names.contains(&name) && !(lowercase_only && name.bytes().any(|b| b.is_ascii_uppercase())) {
                    names.push(name);
                }
            }
            Ok(names)
        });
        // No `iterable<>` declaration: @@iterator alone.

        def.indexed_iterator();
        def.legacy_platform_object();
    }
}

fn attr_at<'s>(s: &Scope<'s>, m: &NamedNodeMapData, i: usize) -> JsResult<Option<Value<'s>>> {
    let n = m.el.page.dom.borrow().attributes(m.el.id).js()?.get(i).map(AttrName::of);
    match n {
        Some(n) => Ok(Some(make_attr(s, &m.el.page, m.el.id, n)?)),
        None => Ok(None),
    }
}

/// Owner element and name of the `Attr` wrapped by `n` (`None`: not an Attr).
pub(crate) fn attr_of(n: &NodeData) -> Option<(Option<NodeId>, AttrName)> {
    let dom = n.page().dom.borrow();
    let a = dom.attr_node_data(n.id).ok()?;
    let name = AttrName { ns: a.name.ns.to_string(), local: a.name.local.to_string() };
    Some((dom.attr_owner_element(n.id).ok()?, name))
}

/// The Attr node of element `el`'s attribute `name`: the same object for as
/// long as the attribute stays on the element (null when it is not there).
pub(crate) fn make_attr<'s>(s: &Scope<'s>, page: &Rc<PageDom>, el: NodeId, name: AttrName) -> JsResult<Value<'s>> {
    let node = page.dom.borrow_mut().attribute_node(el, name.ns(), &name.local).js()?;
    match node {
        Some(id) => Ok(wrap_node(s, page, id)?.value()),
        None => Ok(s.null()),
    }
}

/// `document.createAttribute[NS]`: an Attr of `doc` without an element.
pub(crate) fn make_detached_attr<'s>(
    s: &Scope<'s>,
    page: &Rc<PageDom>,
    doc: NodeId,
    name: pyro_dom::QualName,
) -> JsResult<Value<'s>> {
    let id = page.dom.borrow_mut().create_attribute(doc, name).js()?;
    Ok(wrap_node(s, page, id)?.value())
}

/// `Element.setAttributeNode[NS]` / `NamedNodeMap.setNamedItem[NS]`:
/// returns the replaced Attr, or null.
pub(crate) fn set_attribute_node<'s>(
    s: &Scope<'s>,
    page: &Rc<PageDom>,
    el: NodeId,
    attr: Value<'s>,
) -> JsResult<Value<'s>> {
    let Some(a) = s.unwrap::<Attr>(attr) else {
        return Err(JsError::Type("parameter 1 is not of type 'Attr'.".into()));
    };
    // Trusted Types "set an attribute": the attribute's value is made compliant first, before the
    // in-use check and the same-attribute early return; a default policy's result becomes the
    // Attr's value. The policy can run script that gives the Attr an owner: the DOM's in-use
    // check below then sees it (`modify-attributes-in-callback`). No borrow is held across it.
    // Only a document that requires Trusted Types has anything to check (the cheap test
    // `attribute_value` makes first): nothing is copied out of the DOM otherwise.
    let requires = page.dom.borrow().node_document(el).is_ok_and(|doc| crate::trusted_types::requires_for_script_in(page, doc));
    if requires {
        let (local, ns, value) = {
            let dom = page.dom.borrow();
            let data = dom.attr_node_data(a.id).js()?;
            (data.name.local.to_string(), data.name.ns.to_string(), dom.attr_node_value(a.id).js()?.to_owned())
        };
        if let Some(compliant) = crate::trusted_types::attribute_value(s, page, el, &local, Some(ns.as_str()), s.string(&value))? {
            // An Attr some element owns by now is that element's attribute: left alone.
            let unowned = page.dom.borrow().attr_owner_element(a.id).js()?.is_none();
            if unowned && compliant != value {
                page.dom.borrow_mut().set_attr_node_value(a.id, &compliant).js()?;
            }
        }
    }
    let replaced = page.dom.borrow_mut().set_attribute_node(el, a.id).js()?;
    match replaced {
        Some(old) if old == a.id => Ok(attr),
        Some(old) => Ok(wrap_node(s, page, old)?.value()),
        None => Ok(s.null()),
    }
}

/// Setting the value of the Attr `n` (`Attr.value`, `nodeValue`, `textContent`): an Attr with an
/// owner element is a Trusted Types sink when the element's attribute is (`attribute_value`, the
/// element's node document's policies), else the text is set as it is. `text` is the converted
/// DOMString (a Trusted* object arrives as its string, so a default policy applies to it).
pub(crate) fn set_attr_value_checked(s: &Scope<'_>, n: &NodeData, text: &str) -> JsResult<()> {
    let page = n.page();
    let (owner, name) = attr_of(n).ok_or_else(|| JsError::Type("Not an Attr".into()))?;
    let mut text = std::borrow::Cow::Borrowed(text);
    if let Some(el) = owner {
        if let Some(compliant) = crate::trusted_types::attribute_value(s, page, el, &name.local, name.ns(), s.string(&text))? {
            text = std::borrow::Cow::Owned(compliant);
        }
    }
    page.dom.borrow_mut().set_attr_node_value(n.id, &text).js()
}

/// `Element.removeAttributeNode`.
pub(crate) fn remove_attribute_node<'s>(
    s: &Scope<'s>,
    page: &Rc<PageDom>,
    el: NodeId,
    attr: Value<'s>,
) -> JsResult<Value<'s>> {
    let Some(a) = s.unwrap::<Attr>(attr) else {
        return Err(JsError::Type("parameter 1 is not of type 'Attr'.".into()));
    };
    page.dom.borrow_mut().remove_attribute_node(el, a.id).js()?;
    Ok(attr)
}

/// `Attr`: a Node (https://dom.spec.whatwg.org/#interface-attr). The Node
/// members (`nodeValue`, `textContent`, `cloneNode`, `isEqualNode`..) come
/// from `Node`; the attribute's state lives in `pyro_dom` (`Dom::attribute_node`).
pub struct Attr;
impl JsClass for Attr {
    const NAME: &'static str = "Attr";
    type Data = NodeData;
    type Parent = crate::node::Node;
    fn define(def: &mut ClassDef<Self>) {
        def.getter("name", |s, a| {
            let dom = a.page().dom.borrow();
            let attr = dom.attr_node_data(a.id).js()?;
            Ok(match &attr.name.prefix {
                Some(p) if !p.is_empty() => s.string(&attr.qualified_name()),
                _ => attr.name.local.js(s),
            })
        });
        def.getter("localName", |s, a| Ok(a.page().dom.borrow().attr_node_data(a.id).js()?.name.local.js(s)));
        def.getter("prefix", |s, a| {
            let dom = a.page().dom.borrow();
            Ok(match &dom.attr_node_data(a.id).js()?.name.prefix {
                Some(p) if !p.is_empty() => p.js(s),
                _ => s.null(),
            })
        });
        def.getter("namespaceURI", |s, a| {
            let dom = a.page().dom.borrow();
            let ns = &dom.attr_node_data(a.id).js()?.name.ns;
            Ok(if ns.is_empty() { s.null() } else { ns.js(s) })
        });
        def.accessor(
            "value",
            |s, a| Ok(s.string(a.page().dom.borrow().attr_node_value(a.id).js()?)),
            |s, a, v| {
                let value = dom_string(v)?;
                set_attr_value_checked(s, a, &value)
            },
        );
        def.getter("ownerElement", |s, a| {
            let owner = a.page().dom.borrow().attr_owner_element(a.id).js()?;
            crate::node::node_value(s, a.page(), owner)
        });
        def.getter("specified", |s, _a| Ok(s.boolean(true)));
    }
}

pub fn named_node_map<'s>(s: &Scope<'s>, page: &PageDom, id: NodeId) -> JsResult<Object<'s>> {
    let key = coll_identity(page, 20, id);
    if let Some(o) = s.lookup_identity(key) {
        return Ok(o);
    }
    let page = page_rc(s)?;
    s.wrap::<NamedNodeMap>(NamedNodeMapData { el: NodeHold::new(&page, id)? }, Some(key))
}

// ----- DOMStringMap (dataset) -------------------------------------------------------

pub struct StringMapData {
    el: NodeHold,
}

fn camel_to_data_attr(name: &str) -> Option<String> {
    let mut out = String::with_capacity(name.len() + 5);
    out.push_str("data-");
    let mut chars = name.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '-' && chars.peek().is_some_and(|n| n.is_ascii_lowercase()) {
            return None; // SyntaxError per spec for setter; getter: absent
        }
        if c.is_ascii_uppercase() {
            out.push('-');
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    Some(out)
}

fn data_attr_to_camel(attr: &str) -> Option<String> {
    // HTML "supported property names" of DOMStringMap: names after `data-`
    // without ASCII upper alphas; each `-` followed by an ASCII lower alpha
    // is removed and that letter uppercased.
    let rest = attr.strip_prefix("data-")?;
    if rest.bytes().any(|b| b.is_ascii_uppercase()) {
        return None;
    }
    let mut out = String::with_capacity(rest.len());
    let mut chars = rest.chars().peekable();
    while let Some(c) = chars.next() {
        match chars.peek() {
            Some(&n) if c == '-' && n.is_ascii_lowercase() => {
                out.push(n.to_ascii_uppercase());
                chars.next();
            }
            _ => out.push(c),
        }
    }
    Some(out)
}

pub struct DomStringMap;
impl JsClass for DomStringMap {
    const NAME: &'static str = "DOMStringMap";
    type Data = StringMapData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.named(true, |s, m, name| {
            let Some(attr) = camel_to_data_attr(name) else { return Ok(None) };
            let dom = m.el.page.dom.borrow();
            Ok(dom.get_attribute(m.el.id, &attr).js()?.map(|v| s.string(v)))
        });
        def.named_setter(|_s, m, name, v| {
            let attr = camel_to_data_attr(name)
                .ok_or_else(|| JsError::dom("SyntaxError", format!("'{name}' is not a valid property name.")))?;
            let arg0 = dom_string(v)?;
            m.el.page.dom.borrow_mut().set_attribute(m.el.id, &attr, &arg0).js()?;
            Ok(true)
        });
        def.named_deleter(|_s, m, name| {
            let Some(attr) = camel_to_data_attr(name) else { return Ok(false) };
            m.el.page.dom.borrow_mut().remove_attribute(m.el.id, &attr).js()?;
            Ok(true)
        });
        def.property_names(|_s, m| {
            let dom = m.el.page.dom.borrow();
            Ok(dom.attributes(m.el.id).js()?.iter().filter_map(|a| data_attr_to_camel(&a.qualified_name())).collect())
        });
    }
}

pub fn dataset<'s>(s: &Scope<'s>, page: &PageDom, id: NodeId) -> JsResult<Object<'s>> {
    let key = coll_identity(page, 21, id);
    if let Some(o) = s.lookup_identity(key) {
        return Ok(o);
    }
    let page = page_rc(s)?;
    s.wrap::<DomStringMap>(StringMapData { el: NodeHold::new(&page, id)? }, Some(key))
}

// ----- SVGStringList -----------------------------------------------------------

use pyro_dom::svg_string_list::StringListAttribute;

pub struct StringListData {
    el: NodeHold,
    kind: StringListAttribute,
}

impl StringListData {
    fn with_items<R>(&self, f: impl FnOnce(&[String]) -> R) -> JsResult<R> {
        let mut dom = self.el.page.dom.borrow_mut();
        Ok(f(dom.svg_string_list(self.el.id, self.kind).js()?))
    }

    fn edit<R>(&self, f: impl FnOnce(&mut Vec<String>) -> JsResult<R>) -> JsResult<R> {
        let mut dom = self.el.page.dom.borrow_mut();
        let mut items = dom.svg_string_list(self.el.id, self.kind).js()?.to_vec();
        let result = f(&mut items)?;
        dom.set_svg_string_list(self.el.id, self.kind, items).js()?;
        Ok(result)
    }

    fn replace(&self, index: usize, value: String) -> JsResult<()> {
        self.edit(|items| {
            *items.get_mut(index).ok_or_else(string_list_index_error)? = value;
            Ok(())
        })
    }
}

/// Web IDL unsigned long conversion uses modulo after truncation, even for
/// doubles outside Rust's signed integer range. NaN and infinities map to zero.
fn svg_list_index(value: Value<'_>) -> JsResult<usize> {
    Ok(value.to_number()?.trunc().rem_euclid(4_294_967_296.0) as u32 as usize)
}

fn string_list_index_error() -> JsError {
    JsError::dom("IndexSizeError", "The index is outside the SVG string list.")
}

pub struct SvgStringList;
impl JsClass for SvgStringList {
    const NAME: &'static str = "SVGStringList";
    type Data = StringListData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        for name in ["length", "numberOfItems"] {
            def.getter(name, |s, l| l.with_items(|v| s.number(v.len() as f64)));
        }
        def.legacy_platform_object();
        def.indexed(|s, l, i| l.with_items(|v| v.get(i as usize).map(|item| s.string(item))));
        def.indexed_iterator();
        def.named_setter(|_, l, name, value| {
            let Some(i) = name.parse::<u32>().ok().filter(|i| *i != u32::MAX && i.to_string() == name) else {
                return Ok(false);
            };
            l.replace(i as usize, dom_string(value)?)?;
            Ok(true)
        });
        def.method("clear", 0, |s, l, _| {
            l.el.page.dom.borrow_mut().set_svg_string_list(l.el.id, l.kind, Vec::new()).js()?;
            Ok(s.undefined())
        });
        def.method("getItem", 1, |s, l, a| {
            a.require(1, "SVGStringList.getItem")?;
            let i = svg_list_index(a.get(0))?;
            l.with_items(|v| v.get(i).map(|item| s.string(item)))?.ok_or_else(string_list_index_error)
        });
        def.method("initialize", 1, |s, l, a| {
            a.require(1, "SVGStringList.initialize")?;
            let value = dom_string(a.get(0))?;
            let result = s.string(&value);
            l.el.page.dom.borrow_mut().set_svg_string_list(l.el.id, l.kind, vec![value]).js()?;
            Ok(result)
        });
        def.method("appendItem", 1, |s, l, a| {
            a.require(1, "SVGStringList.appendItem")?;
            let value = dom_string(a.get(0))?;
            let result = s.string(&value);
            l.edit(|items| { items.push(value); Ok(()) })?;
            Ok(result)
        });
        def.method("insertItemBefore", 2, |s, l, a| {
            a.require(2, "SVGStringList.insertItemBefore")?;
            let value = dom_string(a.get(0))?;
            let i = svg_list_index(a.get(1))?;
            let result = s.string(&value);
            l.edit(|items| { items.insert(i.min(items.len()), value); Ok(()) })?;
            Ok(result)
        });
        def.method("replaceItem", 2, |s, l, a| {
            a.require(2, "SVGStringList.replaceItem")?;
            let value = dom_string(a.get(0))?;
            let i = svg_list_index(a.get(1))?;
            let result = s.string(&value);
            l.replace(i, value)?;
            Ok(result)
        });
        def.method("removeItem", 1, |s, l, a| {
            a.require(1, "SVGStringList.removeItem")?;
            let i = svg_list_index(a.get(0))?;
            let value = l.edit(|items| {
                if i >= items.len() { return Err(string_list_index_error()); }
                Ok(items.remove(i))
            })?;
            Ok(s.string(&value))
        });
    }
}

pub fn svg_string_list<'s>(s: &Scope<'s>, n: &NodeData, kind: StringListAttribute) -> JsResult<Value<'s>> {
    let slot = match kind {
        StringListAttribute::RequiredExtensions => 440,
        StringListAttribute::SystemLanguage => 441,
    };
    let key = coll_identity(n.page(), slot, n.id);
    if let Some(obj) = s.lookup_identity(key) {
        return Ok(obj.value());
    }
    let el = NodeHold::new(n.page(), n.id)?;
    Ok(s.wrap::<SvgStringList>(StringListData { el, kind }, Some(key))?.value())
}

// ----- SVGAnimatedEnumeration ---------------------------------------------------

/// An SVG element's enumerated attribute (`clipPathUnits`, `spreadMethod`, ...):
/// the attribute name, its keywords in the order of their numeric values from 1
/// (0 is the unknown value), the value of an absent or unrecognized keyword (its
/// initial value), and the identity slot of the wrapper.
pub struct SvgEnum {
    pub attr: &'static str,
    pub keywords: &'static [&'static str],
    pub initial: u16,
    pub slot: u64,
}

pub struct AnimatedEnumData {
    el: NodeHold,
    e: &'static SvgEnum,
}

impl AnimatedEnumData {
    fn value(&self) -> u16 {
        let dom = self.el.page.dom.borrow();
        let keyword = dom.get_attribute_ns(self.el.id, None, self.e.attr).ok().flatten();
        keyword
            .and_then(|k| self.e.keywords.iter().position(|w| *w == k))
            .map_or(self.e.initial, |i| i as u16 + 1)
    }

    /// `animVal`: the keyword the animations in effect give, else `value`. An animated value that
    /// is no keyword leaves the base value (a marker's `orient` angle is its initial, angle, type).
    fn animated_value(&self) -> u16 {
        let contributions = attribute_contributions(&self.el.page, self.el.id, &[self.e.attr]);
        if contributions.is_empty() {
            return self.value();
        }
        let base = self.el.page.dom.borrow().get_attribute_ns(self.el.id, None, self.e.attr).ok().flatten().map(str::to_owned);
        let text = crate::smil::fold(&TextSpace, &contributions, base.unwrap_or_default());
        match self.e.keywords.iter().position(|w| *w == text) {
            Some(i) => i as u16 + 1,
            None if self.e.attr == "orient" => self.e.initial,
            None => self.value(),
        }
    }
}

/// `SVGAnimatedEnumeration`: `baseVal` reflects the attribute (setting it writes
/// the keyword, which re-renders the SVG as any attribute change does; 0 and
/// values without a keyword throw a TypeError, as in Chromium); `animVal` is
/// `baseVal` unless a SMIL animation is in effect.
pub struct SvgAnimatedEnumeration;
impl JsClass for SvgAnimatedEnumeration {
    const NAME: &'static str = "SVGAnimatedEnumeration";
    type Data = AnimatedEnumData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.accessor(
            "baseVal",
            |s, t| Ok(s.number(t.value() as f64)),
            |_s, t, v| {
                // WebIDL unsigned short.
                let n = (to_u32(v)? & 0xffff) as usize;
                let Some(keyword) = n.checked_sub(1).and_then(|i| t.e.keywords.get(i)) else {
                    return Err(JsError::Type(
                        format!("The enumeration value provided ({n}) is not a valid value for '{}'.", t.e.attr).into(),
                    ));
                };
                // Selecting ANGLE preserves a current angle and resets a keyword
                // or invalid attribute to the initial zero angle.
                if t.e.attr == "orient" && n == 2 {
                    let dom = t.el.page.dom.borrow();
                    if dom.get_attribute_ns(t.el.id, None, "orient").js()?.and_then(Angle::parse).is_some() {
                        return Ok(());
                    }
                }
                t.el.page.dom.borrow_mut().set_attribute_ns(t.el.id, None, t.e.attr, keyword).js()
            },
        );
        def.getter("animVal", |s, t| Ok(s.number(t.animated_value() as f64)));
    }
}

/// The element's `SVGAnimatedEnumeration` for `e`, the same object each time.
pub fn animated_enumeration<'s>(
    s: &Scope<'s>,
    page: &PageDom,
    id: NodeId,
    e: &'static SvgEnum,
) -> JsResult<Object<'s>> {
    let key = coll_identity(page, e.slot, id);
    if let Some(o) = s.lookup_identity(key) {
        return Ok(o);
    }
    let page = page_rc(s)?;
    s.wrap::<SvgAnimatedEnumeration>(AnimatedEnumData { el: NodeHold::new(&page, id)?, e }, Some(key))
}

// ----- SVGAnimatedString, SVGAnimatedNumber, SVGAnimatedInteger, SVGAnimatedBoolean -------------

/// Which number of an attribute an animated number or integer reflects: the whole attribute, or the
/// first or second of a pair (`stdDeviation`, `order`, `radius`, ...), the second defaulting to the
/// first when the attribute gives one number.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SvgPart {
    Whole,
    NumberOrPercentage,
    First,
    Second,
}

/// An SVG element's string, number, integer or boolean attribute: the attribute names (the first
/// present one is read and written; `href` falls back to `xlink:href`), which number of it, the
/// value of an absent or unparseable attribute, and the identity slot of the wrapper.
pub struct SvgAttr {
    pub attrs: &'static [&'static str],
    pub part: SvgPart,
    pub initial: f64,
    pub slot: u64,
}

pub struct AnimatedAttrData {
    el: NodeHold,
    a: &'static SvgAttr,
}

impl AnimatedAttrData {
    /// SVG 2 linking: href is reflected in no namespace unless only the
    /// legacy XLink attribute exists. Its DOM prefix is immaterial.
    fn name(&self, dom: &Dom) -> (Option<&'static str>, &'static str) {
        let primary = self.a.attrs[0];
        if self.a.attrs.len() == 1 || dom.get_attribute_ns(self.el.id, None, primary).ok().flatten().is_some() {
            return (None, primary);
        }
        for alias in &self.a.attrs[1..] {
            let (ns, local) = match alias.strip_prefix("xlink:") {
                Some(local) => (Some("http://www.w3.org/1999/xlink"), local),
                None => (None, *alias),
            };
            if dom.get_attribute_ns(self.el.id, ns, local).ok().flatten().is_some() {
                return (ns, local);
            }
        }
        (None, primary)
    }
    fn text(&self) -> Option<String> {
        let dom = self.el.page.dom.borrow();
        let (ns, name) = self.name(&dom);
        dom.get_attribute_ns(self.el.id, ns, name).ok().flatten().map(str::to_owned)
    }
    /// The text the SMIL animations in effect give the attribute (`None` without one).
    fn animated_text(&self) -> Option<String> {
        let contributions = attribute_contributions(&self.el.page, self.el.id, self.a.attrs);
        if contributions.is_empty() {
            return None;
        }
        Some(crate::smil::fold(&TextSpace, &contributions, self.text().unwrap_or_default()))
    }
    /// The numbers the SMIL animations in effect give the attribute (`None` without one).
    fn animated_numbers(&self) -> Option<Vec<f64>> {
        let contributions = attribute_contributions(&self.el.page, self.el.id, self.a.attrs);
        if contributions.is_empty() {
            return None;
        }
        let pair = matches!(self.a.part, SvgPart::First | SvgPart::Second);
        let mut base = self.numbers().unwrap_or_else(|| vec![self.a.initial]);
        // A pair given one number is that number twice.
        if pair && base.len() == 1 {
            base.push(base[0]);
        }
        Some(crate::smil::fold(&NumberSpace { pair }, &contributions, base))
    }
    /// The number of this part under animation (`None` without one).
    fn animated_part(&self) -> Option<f64> {
        let numbers = self.animated_numbers()?;
        let first = *numbers.first()?;
        Some(if self.a.part == SvgPart::Second { numbers.get(1).copied().unwrap_or(first) } else { first })
    }
    fn set(&self, value: &str) -> JsResult<()> {
        let mut dom = self.el.page.dom.borrow_mut();
        let (ns, name) = self.name(&dom);
        dom.set_attribute_ns(self.el.id, ns, name, value).js()
    }
    /// The attribute's numbers (separated by whitespace and/or a comma); `None` when it does not parse
    /// as one number (or, for a pair, one or two).
    fn numbers(&self) -> Option<Vec<f64>> {
        let text = self.text()?;
        let text = text.trim_matches(|c: char| c.is_ascii_whitespace());
        if self.a.part == SvgPart::NumberOrPercentage {
            let n = match text.strip_suffix('%') {
                Some(percent) => svg_number(percent)? / 100.0,
                None => svg_number(text)?,
            };
            return Some(vec![n]);
        }
        if self.a.part == SvgPart::Whole {
            return Some(vec![svg_number(text)?]);
        }
        let parts: Vec<_> = if let Some((x, y)) = text.split_once(',') {
            let whitespace = |c: char| c.is_ascii_whitespace();
            vec![x.trim_matches(whitespace), y.trim_matches(whitespace)]
        } else {
            text.split_ascii_whitespace().collect()
        };
        if parts.is_empty() || parts.len() > 2 {
            return None;
        }
        parts.into_iter().map(svg_number).collect()
    }
    fn number(&self) -> f64 {
        self.number_as(self.a.part)
    }
    /// Writes `v` as this part, keeping the other number of a pair.
    fn set_number(&self, v: f64) -> JsResult<()> {
        let text = match self.a.part {
            SvgPart::Whole | SvgPart::NumberOrPercentage => fmt_number(v),
            SvgPart::First => format!("{} {}", fmt_number(v), fmt_number(self.number_as(SvgPart::Second))),
            SvgPart::Second => format!("{} {}", fmt_number(self.number_as(SvgPart::First)), fmt_number(v)),
        };
        self.set(&text)
    }
    fn number_as(&self, part: SvgPart) -> f64 {
        match (self.numbers(), part) {
            (Some(n), SvgPart::Second) => n.get(1).copied().unwrap_or(n[0]),
            (Some(n), _) => n[0],
            (None, _) => self.a.initial,
        }
    }
}

/// An SVG `<number>`: a CSS number without units (`1e3`, `.5`, `-2`); `None` for anything else.
fn svg_number(text: &str) -> Option<f64> {
    if text.is_empty() || text.contains(|c: char| !(c.is_ascii_digit() || matches!(c, '.' | '-' | '+' | 'e' | 'E'))) {
        return None;
    }
    if text.split(['e', 'E']).next()?.ends_with('.') {
        return None;
    }
    text.parse::<f64>().ok().filter(|v| v.is_finite())
}

fn fmt_number(v: f64) -> String {
    if v == v.trunc() && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        // SVGAnimatedNumber is a WebIDL float. Serializing the widened
        // JS double would expose extra digits for values such as 0.2.
        (v as f32).to_string()
    }
}

fn finite_float(v: Value<'_>, what: &str) -> JsResult<f64> {
    let n = v.to_number()? as f32 as f64;
    if !n.is_finite() {
        return Err(JsError::Type(format!("Failed to set the 'baseVal' property on '{what}': The provided float value is non-finite.").into()));
    }
    Ok(n)
}

/// `SVGAnimatedString`: `baseVal` reflects the attribute; `animVal` is `baseVal` unless a SMIL animation is in effect.
pub struct SvgAnimatedString;
impl JsClass for SvgAnimatedString {
    const NAME: &'static str = "SVGAnimatedString";
    type Data = AnimatedAttrData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        // `baseVal` writes the attribute: SVGScriptElement's `href` is a Trusted Types sink (a
        // TrustedScriptURL or what the default policy makes of the string), as `setAttribute` is.
        def.accessor(
            "baseVal",
            |s, t| Ok(s.string(&t.text().unwrap_or_default())),
            |s, t, v| {
                let (ns, name) = {
                    let dom = t.el.page.dom.borrow();
                    let name = t.name(&dom);
                    name
                };
                match crate::trusted_types::attribute_value(s, &t.el.page, t.el.id, name, ns, v)? {
                    Some(compliant) => t.set(&compliant),
                    None => t.set(&dom_string(v)?),
                }
            },
        );
        def.getter("animVal", |s, t| Ok(s.string(&t.animated_text().or_else(|| t.text()).unwrap_or_default())));
    }
}

use pyro_dom::svg_point_list::{NumberItem, NumberLists};

pub struct SvgNumber;
impl JsClass for SvgNumber {
    const NAME: &'static str = "SVGNumber";
    type Data = Rc<NumberItem>;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.accessor("value", |s, n| Ok(s.number(f64::from(n.value()))), |_s, n, v| {
            n.set_value(finite_float(v, "SVGNumber")? as f32).js()
        });
    }
}

pub(crate) fn number_value<'s>(s: &Scope<'s>, item: Rc<NumberItem>) -> JsResult<Value<'s>> {
    let key = IdentityKey((1 << 59) | (2 << 56), Rc::as_ptr(&item) as u64);
    Ok(s.wrap::<SvgNumber>(item, Some(key))?.value())
}

fn number_argument(s: &Scope<'_>, value: Value<'_>) -> JsResult<Rc<NumberItem>> {
    let item = s.unwrap::<SvgNumber>(value).ok_or_else(|| JsError::Type("Expected an SVGNumber.".into()))?;
    Ok((*item).clone())
}

/// `SVGAnimatedNumber`: `baseVal` is the attribute's number (its initial value when absent or
/// invalid); setting it writes the number (a non-finite value throws a TypeError).
pub struct SvgAnimatedNumber;
impl JsClass for SvgAnimatedNumber {
    const NAME: &'static str = "SVGAnimatedNumber";
    type Data = AnimatedAttrData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.accessor(
            "baseVal",
            |s, t| Ok(s.number(t.number() as f32 as f64)),
            |_s, t, v| t.set_number(finite_float(v, "SVGAnimatedNumber")?),
        );
        def.getter("animVal", |s, t| Ok(s.number(t.animated_part().unwrap_or_else(|| t.number()) as f32 as f64)));
    }
}

/// `SVGAnimatedInteger`: as `SVGAnimatedNumber`, with WebIDL `long` values (a non-integer attribute
/// value is its initial value).
pub struct SvgAnimatedInteger;
impl JsClass for SvgAnimatedInteger {
    const NAME: &'static str = "SVGAnimatedInteger";
    type Data = AnimatedAttrData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        fn int(t: &AnimatedAttrData) -> f64 {
            let n = t.number();
            if n == n.trunc() { n } else { t.a.initial }
        }
        def.accessor("baseVal", |s, t| Ok(s.number(int(t))), |_s, t, v| t.set_number(to_i32(v)? as f64));
        // (An animated integer is rounded to the nearest one.)
        def.getter("animVal", |s, t| Ok(s.number(t.animated_part().map_or_else(|| int(t), f64::round))));
    }
}

/// `SVGAnimatedBoolean`: the attribute is `true` (or `false`, the value of anything else).
pub struct SvgAnimatedBoolean;
impl JsClass for SvgAnimatedBoolean {
    const NAME: &'static str = "SVGAnimatedBoolean";
    type Data = AnimatedAttrData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        fn get(t: &AnimatedAttrData) -> bool {
            t.text().is_some_and(|v| v.trim() == "true")
        }
        def.accessor(
            "baseVal",
            |s, t| Ok(s.boolean(get(t))),
            |_s, t, v| t.set(if v.to_bool() { "true" } else { "false" }),
        );
        def.getter("animVal", |s, t| Ok(s.boolean(t.animated_text().map_or_else(|| get(t), |v| v.trim() == "true"))));
    }
}

/// The element's animated value for `a`, of class `C`, the same object each time.
pub fn animated_attr<'s, C: JsClass<Data = AnimatedAttrData>>(
    s: &Scope<'s>,
    page: &PageDom,
    id: NodeId,
    a: &'static SvgAttr,
) -> JsResult<Object<'s>> {
    let key = coll_identity(page, a.slot, id);
    if let Some(o) = s.lookup_identity(key) {
        return Ok(o);
    }
    let page = page_rc(s)?;
    s.wrap::<C>(AnimatedAttrData { el: NodeHold::new(&page, id)?, a }, Some(key))
}

// SVG angles are stored in their specified units. The unitless form is in
// degrees, and a turn is accepted by CSS but has no SVGAngle unit constant.
#[derive(Clone, Copy)]
struct Angle {
    value: f32,
    unit: u16,
}
impl Angle {
    const ZERO: Self = Self { value: 0.0, unit: 1 };
    const UNITS: [&'static str; 6] = ["", "", "deg", "rad", "grad", "turn"];
    fn parse(text: &str) -> Option<Self> {
        let text = text.trim_matches(|c: char| c.is_ascii_whitespace());
        // grad precedes rad because the suffixes overlap.
        for unit in [4, 3, 2, 5, 1] {
            let suffix = Self::UNITS[unit];
            let Some(n) = text.len().checked_sub(suffix.len()) else { continue };
            if text.is_char_boundary(n) && text[n..].eq_ignore_ascii_case(suffix) {
                let value = svg_number(&text[..n])? as f32;
                return value.is_finite().then_some(Self { value, unit: unit as u16 });
            }
        }
        None
    }
    fn factor(unit: u16) -> f64 {
        match unit {
            3 => 180.0 / std::f64::consts::PI,
            4 => 0.9,
            5 => 360.0,
            _ => 1.0,
        }
    }
    fn degrees(self) -> f64 {
        f64::from(self.value) * Self::factor(self.unit)
    }
    fn clamp(value: f64) -> f32 {
        value.clamp(-f64::from(f32::MAX), f64::from(f32::MAX)) as f32
    }
    fn string(self) -> String {
        let value = if self.value == 0.0 { 0.0 } else { self.value };
        format!("{value}{}", Self::UNITS[self.unit as usize])
    }
}

pub enum SvgAngleData {
    Detached(Cell<(f32, u16)>),
    Reflected { el: NodeHold, anim: bool },
}
impl SvgAngleData {
    fn get(&self) -> Angle {
        match self {
            Self::Detached(cell) => {
                let (value, unit) = cell.get();
                Angle { value, unit }
            }
            Self::Reflected { el, anim } => {
                let base = el
                    .page
                    .dom
                    .borrow()
                    .get_attribute_ns(el.id, None, "orient")
                    .ok()
                    .flatten()
                    .and_then(Angle::parse)
                    .unwrap_or(Angle::ZERO);
                if !*anim {
                    return base;
                }
                let contributions = attribute_contributions(&el.page, el.id, &["orient"]);
                if contributions.is_empty() {
                    return base;
                }
                match crate::smil::fold(&AngleSpace, &contributions, AngleVal::Angle(base)) {
                    AngleVal::Angle(angle) => angle,
                    AngleVal::Keyword(_) => Angle::ZERO,
                }
            }
        }
    }
    fn writable(&self) -> JsResult<()> {
        if matches!(self, Self::Reflected { anim: true, .. }) {
            return Err(JsError::dom("NoModificationAllowedError", "The animated value is read-only."));
        }
        Ok(())
    }
    fn set(&self, angle: Angle) -> JsResult<()> {
        self.writable()?;
        match self {
            Self::Detached(cell) => {
                cell.set((angle.value, angle.unit));
                Ok(())
            }
            Self::Reflected { el, .. } => el.page.dom.borrow_mut().set_attribute_ns(el.id, None, "orient", &angle.string()).js(),
        }
    }
}

pub struct SvgAngle;
impl JsClass for SvgAngle {
    const NAME: &'static str = "SVGAngle";
    type Data = SvgAngleData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        for (i, key) in [
            "SVG_ANGLETYPE_UNKNOWN",
            "SVG_ANGLETYPE_UNSPECIFIED",
            "SVG_ANGLETYPE_DEG",
            "SVG_ANGLETYPE_RAD",
            "SVG_ANGLETYPE_GRAD",
        ]
        .into_iter()
        .enumerate()
        {
            def.constant(key, i as f64);
        }
        def.getter("unitType", |s, t| {
            let k = t.get().unit;
            Ok(s.number(if k == 5 { 0.0 } else { f64::from(k) }))
        });
        def.accessor(
            "value",
            |s, t| Ok(s.number(f64::from(Angle::clamp(t.get().degrees())))),
            |_s, t, v| {
                let n = finite_float(v, "SVGAngle")?;
                let unit = t.get().unit;
                t.set(Angle { value: Angle::clamp(n / Angle::factor(unit)), unit })
            },
        );
        def.accessor(
            "valueInSpecifiedUnits",
            |s, t| Ok(s.number(f64::from(t.get().value))),
            |_s, t, v| {
                let value = finite_float(v, "SVGAngle")? as f32;
                t.set(Angle { value, unit: t.get().unit })
            },
        );
        def.accessor(
            "valueAsString",
            |s, t| Ok(s.string(&t.get().string())),
            |_s, t, v| {
                let input = dom_string(v)?;
                t.writable()?;
                t.set(Angle::parse(&input).ok_or_else(|| JsError::dom("SyntaxError", "Invalid SVG angle."))?)
            },
        );
        def.method("newValueSpecifiedUnits", 2, |s, t, a| {
            a.require(2, "SVGAngle.newValueSpecifiedUnits")?;
            let unit = to_u32(a.get(0))? as u16;
            let value = finite_float(a.get(1), "SVGAngle")? as f32;
            t.writable()?;
            check_angle_unit(unit)?;
            t.set(Angle { value, unit })?;
            Ok(s.undefined())
        });
        def.method("convertToSpecifiedUnits", 1, |s, t, a| {
            a.require(1, "SVGAngle.convertToSpecifiedUnits")?;
            let unit = to_u32(a.get(0))? as u16;
            t.writable()?;
            check_angle_unit(unit)?;
            t.set(Angle { value: Angle::clamp(t.get().degrees() / Angle::factor(unit)), unit })?;
            Ok(s.undefined())
        });
    }
}
fn check_angle_unit(unit: u16) -> JsResult<()> {
    if (1..=4).contains(&unit) {
        Ok(())
    } else {
        Err(JsError::dom("NotSupportedError", "Unsupported SVG angle unit."))
    }
}
pub fn create_svg_angle<'s>(s: &Scope<'s>) -> JsResult<Object<'s>> {
    s.wrap::<SvgAngle>(SvgAngleData::Detached(Cell::new((0.0, 1))), None)
}
pub fn angle_string(s: &Scope<'_>, value: Value<'_>) -> JsResult<String> {
    let angle = s.unwrap::<SvgAngle>(value).ok_or_else(|| JsError::Type("Expected an SVGAngle.".into()))?;
    Ok(angle.get().string())
}
pub struct SvgAnimatedAngle;
impl JsClass for SvgAnimatedAngle {
    const NAME: &'static str = "SVGAnimatedAngle";
    type Data = NodeHold;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.getter("baseVal", |s, el| Ok(reflected_angle(s, el, false)?.value()));
        def.getter("animVal", |s, el| Ok(reflected_angle(s, el, true)?.value()));
    }
}
fn reflected_angle<'s>(s: &Scope<'s>, el: &NodeHold, anim: bool) -> JsResult<Object<'s>> {
    let key = coll_identity(&el.page, 429 + u64::from(anim), el.id);
    if let Some(o) = s.lookup_identity(key) {
        return Ok(o);
    }
    s.wrap::<SvgAngle>(SvgAngleData::Reflected { el: NodeHold::new(&el.page, el.id)?, anim }, Some(key))
}
pub fn animated_angle<'s>(s: &Scope<'s>, page: &PageDom, id: NodeId) -> JsResult<Object<'s>> {
    let key = coll_identity(page, 428, id);
    if let Some(o) = s.lookup_identity(key) {
        return Ok(o);
    }
    s.wrap::<SvgAnimatedAngle>(NodeHold::new(&page_rc(s)?, id)?, Some(key))
}

// ----- SVGAnimatedLength, SVGLength ------------------------------------------------------------

/// The direction a length attribute's percentages resolve in: the nearest `svg` viewport's width,
/// height, or its normalized diagonal (`r`).
#[derive(Clone, Copy)]
pub enum SvgLengthDir {
    Width,
    Height,
    Other,
}

/// An SVG element's `<length>` attribute: its name, its value when absent or invalid, the
/// direction of its percentages, and the identity slots of its wrappers (`slot` the
/// SVGAnimatedLength, `slot + 1` baseVal and `slot + 2` animVal).
pub struct SvgLengthAttr {
    pub attr: &'static str,
    pub initial: &'static str,
    pub dir: SvgLengthDir,
    pub slot: u64,
}

use pyro_dom::svg_point_list::{LengthItem, LengthLists, LENGTH_UNITS};

/// The unit types a script can name (`SVG_LENGTHTYPE_*`).
const NAMED_LENGTH_UNITS: usize = 11;

/// Parses an SVG `<length>`: a number and an optional unit, matched ASCII case-insensitively
/// (`None` when it is not one).
fn parse_svg_length(text: &str) -> Option<(f64, u16)> {
    let text = text.trim_matches(|c: char| c.is_ascii_whitespace());
    for (kind, unit) in LENGTH_UNITS.iter().enumerate().skip(2) {
        // (A value shorter than the unit, "3" against "em", is still a number.)
        let Some(n) = text.len().checked_sub(unit.len()) else { continue };
        if text.is_char_boundary(n) && text[n..].eq_ignore_ascii_case(unit) {
            if let Some(v) = svg_number(&text[..n]) {
                return Some((v, kind as u16));
            }
        }
    }
    svg_number(text).map(|v| (v, 1))
}

fn length_to_string(value: f64, kind: u16) -> String {
    // The float's shortest form, as Chromium and Firefox serialize SVG numbers.
    format!("{}{}", value as f32, LENGTH_UNITS[kind as usize])
}

pub enum SvgLengthData {
    Attribute { el: NodeHold, a: &'static SvgLengthAttr, anim: bool },
    /// A list item or a detached length from createSVGLength. Insertion
    /// attaches the same object to its new element and percentage direction.
    Item { page: Rc<PageDom>, item: Rc<LengthItem> },
}

impl SvgLengthData {
    fn get(&self) -> (f64, u16) {
        match self {
            Self::Item { item, .. } => item.value(),
            Self::Attribute { el, a, anim } => {
                let base = el.page.dom.borrow().get_attribute(el.id, a.attr)
                    .ok()
                    .flatten()
                    .and_then(parse_svg_length)
                    .or_else(|| parse_svg_length(a.initial))
                    .unwrap_or((0.0, 1));
                if !*anim {
                    return base;
                }
                // `animVal`: the animations in effect over the attribute (units convert in user units).
                let contributions = attribute_contributions(&el.page, el.id, &[a.attr]);
                if contributions.is_empty() {
                    return base;
                }
                let factor = |kind: u16| Self::attribute_factor(el, a.dir, kind);
                let space = LengthSpace { factor: &factor };
                crate::smil::fold(&space, &contributions, vec![base]).first().copied().unwrap_or(base)
            }
        }
    }

    fn writable(&self) -> JsResult<()> {
        let animated = match self {
            Self::Attribute { anim, .. } => *anim,
            Self::Item { item, .. } => item.is_read_only(),
        };
        if animated {
            return Err(JsError::dom("NoModificationAllowedError", "The object is read-only."));
        }
        Ok(())
    }

    fn write(&self, value: f64, kind: u16) -> JsResult<()> {
        match self {
            Self::Item { item, .. } => item.set_value((value, kind)).js(),
            Self::Attribute { el, a, .. } => {
                el.page.dom.borrow_mut().set_attribute(el.id, a.attr, &length_to_string(value, kind)).js()
            }
        }
    }

    fn factor(&self, kind: u16) -> f64 {
        match self {
            Self::Attribute { el, a, .. } => Self::attribute_factor(el, a.dir, kind),
            Self::Item { page, item } => item.unit_factor(kind).unwrap_or_else(|| Self::unit_factor(page, None, kind)),
        }
    }

    fn attribute_factor(el: &NodeHold, dir: SvgLengthDir, kind: u16) -> f64 {
        // An element may be adopted after its length wrapper was created.
        // The current document supplies the viewport and computed font.
        let home = &el.page;
        let doc = home.dom.borrow().node_document(el.id).ok();
        let other = doc.filter(|id| *id != home.document()).and_then(|id| {
            home.hooks()?.frame_tree().into_iter().find(|p| p.document() == id && Rc::ptr_eq(&p.dom, &home.dom))
        });
        Self::unit_factor(other.as_ref().unwrap_or(home), Some((el.id, dir)), kind)
    }

    /// User units per unit of `kind`, in the current attribute's coordinate system.
    fn unit_factor(page: &PageDom, context: Option<(NodeId, SvgLengthDir)>, kind: u16) -> f64 {
        match kind {
            1 | 5 => 1.0,
            2 => context.map_or(1.0, |(id, dir)| Self::viewport_percent(page, id, dir)),
            3 => context.map_or(16.0, |(id, _)| {
                page.layout().style().computed_font_size(&page.dom.borrow(), Some(id))
            }),
            4 | 17..=20 => page.layout().style().svg_relative_unit(
                &page.dom.borrow(), context.map(|(id, _)| id), LENGTH_UNITS[usize::from(kind)],
            ),
            6 => 96.0 / 2.54,
            7 => 96.0 / 25.4,
            8 => 96.0,
            9 => 4.0 / 3.0,
            10 => 16.0,
            11 if context.is_none() => 16.0,
            11 => {
                let dom = page.dom.borrow();
                let root = dom.document_element(page.document()).ok().flatten();
                page.layout().style().computed_font_size(&dom, root)
            }
            12..=15 => {
                let layout = page.layout();
                let viewport = layout.viewport();
                let (w, h) = (viewport.width as f64, viewport.height as f64);
                match kind {
                    12 => w / 100.0,
                    13 => h / 100.0,
                    14 => w.min(h) / 100.0,
                    _ => w.max(h) / 100.0,
                }
            }
            16 => 96.0 / 2.54 / 40.0,
            21..=25 => {
                let dom = page.dom.borrow();
                let root = context.and_then(|(id, _)| dom.node_document(id).ok())
                    .and_then(|doc| dom.document_element(doc).ok().flatten());
                // Root font-relative units use the corresponding metric of
                // the root element, or the initial font for a detached item.
                page.layout().style().svg_relative_unit(&dom, root, &LENGTH_UNITS[usize::from(kind)][1..])
            }
            _ => page.layout().style().svg_relative_unit(
                &page.dom.borrow(), context.map(|(id, _)| id), LENGTH_UNITS[usize::from(kind)],
            ),
        }
    }
    /// One percent of the nearest `svg` ancestor's viewport: its `viewBox`, else its absolute
    /// `width` and `height`; 0 without one.
    fn viewport_percent(page: &PageDom, id: NodeId, dir: SvgLengthDir) -> f64 {
        let dom = page.dom.borrow();
        let mut p = dom.parent_element(id).ok().flatten();
        while let Some(x) = p {
            if dom.element(x).is_ok_and(|e| &*e.name.local == "svg") {
                let attr = |n: &str| dom.get_attribute(x, n).ok().flatten().map(|v| v.to_string());
                let vb: Option<Vec<f64>> = attr("viewBox").and_then(|v| {
                    v.split(|c: char| c.is_ascii_whitespace() || c == ',')
                        .filter(|s| !s.is_empty())
                        .map(|s| s.parse::<f64>().ok())
                        .collect()
                });
                let (w, h) = match vb.as_deref() {
                    Some(&[_, _, w, h]) => (w, h),
                    _ => {
                        let abs = |n: &str| {
                            attr(n).and_then(|v| parse_svg_length(&v)).filter(|(_, k)| *k != 2 && *k != 3 && *k != 4)
                        };
                        match (abs("width"), abs("height")) {
                            (Some((w, kw)), Some((h, kh))) => (w * Self::unit_factor(page, Some((id, dir)), kw), h * Self::unit_factor(page, Some((id, dir)), kh)),
                            _ => return 0.0,
                        }
                    }
                };
                let base = match dir {
                    SvgLengthDir::Width => w,
                    SvgLengthDir::Height => h,
                    SvgLengthDir::Other => ((w * w + h * h) / 2.0).sqrt(),
                };
                return base / 100.0;
            }
            p = dom.parent_element(x).ok().flatten();
        }
        0.0
    }
}

fn length_kind(v: Value<'_>) -> JsResult<u16> {
    let kind = (to_u32(v)? & 0xffff) as u16;
    if kind == 0 || kind as usize >= NAMED_LENGTH_UNITS {
        return Err(JsError::dom("NotSupportedError", "The unit type provided is invalid."));
    }
    Ok(kind)
}

/// `SVGLength`, reflecting a length attribute (`baseVal`) or read-only (`animVal`, the attribute
/// under the SMIL animations in effect). `value` is in user units: percentages resolve against the nearest `svg` viewport.
pub struct SvgLength;
impl JsClass for SvgLength {
    const NAME: &'static str = "SVGLength";
    type Data = SvgLengthData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        for (i, name) in [
            "SVG_LENGTHTYPE_UNKNOWN",
            "SVG_LENGTHTYPE_NUMBER",
            "SVG_LENGTHTYPE_PERCENTAGE",
            "SVG_LENGTHTYPE_EMS",
            "SVG_LENGTHTYPE_EXS",
            "SVG_LENGTHTYPE_PX",
            "SVG_LENGTHTYPE_CM",
            "SVG_LENGTHTYPE_MM",
            "SVG_LENGTHTYPE_IN",
            "SVG_LENGTHTYPE_PT",
            "SVG_LENGTHTYPE_PC",
        ]
        .into_iter()
        .enumerate()
        {
            def.constant(name, i as f64);
        }
        def.getter("unitType", |s, t| {
            let kind = t.get().1 as usize;
            Ok(s.number(if kind < NAMED_LENGTH_UNITS { kind as f64 } else { 0.0 }))
        });
        def.accessor(
            "value",
            |s, t| {
                let (v, k) = t.get();
                Ok(s.number((v * t.factor(k)) as f32 as f64))
            },
            |_s, t, v| {
                t.writable()?;
                let user = finite_float(v, "SVGLength")?;
                let (_, k) = t.get();
                let f = t.factor(k);
                if f == 0.0 {
                    return t.write(user, 1);
                }
                t.write(user / f, k)
            },
        );
        def.accessor(
            "valueInSpecifiedUnits",
            |s, t| Ok(s.number(t.get().0 as f32 as f64)),
            |_s, t, v| {
                t.writable()?;
                let n = finite_float(v, "SVGLength")?;
                t.write(n, t.get().1)
            },
        );
        def.accessor(
            "valueAsString",
            |s, t| {
                let (v, k) = t.get();
                Ok(s.string(&length_to_string(v, k)))
            },
            |_s, t, v| {
                t.writable()?;
                let text = dom_string(v)?;
                let Some((n, k)) = parse_svg_length(&text) else {
                    return Err(JsError::dom("SyntaxError", "The value provided is invalid."));
                };
                t.write(n, k)
            },
        );
        def.method("newValueSpecifiedUnits", 2, |s, t, a| {
            t.writable()?;
            a.require(2, "SVGLength.newValueSpecifiedUnits")?;
            // (Both arguments convert before the unit is checked: Web IDL.)
            to_u32(a.get(0))?;
            let n = finite_float(a.get(1), "SVGLength")?;
            let k = length_kind(a.get(0))?;
            t.write(n, k)?;
            Ok(s.undefined())
        });
        def.method("convertToSpecifiedUnits", 1, |s, t, a| {
            t.writable()?;
            a.require(1, "SVGLength.convertToSpecifiedUnits")?;
            let k = length_kind(a.get(0))?;
            let (v, from) = t.get();
            let user = v * t.factor(from);
            let to = t.factor(k);
            if to == 0.0 {
                return Err(JsError::dom("NotSupportedError", "The length cannot be converted to that unit here."));
            }
            t.write(user / to, k)?;
            Ok(s.undefined())
        });
    }
}

pub struct AnimatedLengthData {
    el: NodeHold,
    a: &'static SvgLengthAttr,
}

fn svg_length<'s>(s: &Scope<'s>, el: &NodeHold, a: &'static SvgLengthAttr, anim: bool) -> JsResult<Object<'s>> {
    let key = coll_identity(&el.page, a.slot + 1 + anim as u64, el.id);
    if let Some(o) = s.lookup_identity(key) {
        return Ok(o);
    }
    s.wrap::<SvgLength>(SvgLengthData::Attribute { el: NodeHold::new(&el.page, el.id)?, a, anim }, Some(key))
}

fn length_item<'s>(s: &Scope<'s>, item: Rc<LengthItem>) -> JsResult<Object<'s>> {
    let key = IdentityKey((1 << 59) | (4 << 56), Rc::as_ptr(&item) as u64);
    s.wrap::<SvgLength>(SvgLengthData::Item { page: page_rc(s)?, item }, Some(key))
}

/// SVGSVGElement.createSVGLength returns a detached length of zero user units.
pub fn create_svg_length<'s>(s: &Scope<'s>, _svg: NodeId) -> JsResult<Object<'s>> {
    length_item(s, LengthItem::detached((0.0, 1)))
}

/// `SVGAnimatedLength`: `baseVal` reflects the attribute, `animVal` reads it as animated.
pub struct SvgAnimatedLength;
impl JsClass for SvgAnimatedLength {
    const NAME: &'static str = "SVGAnimatedLength";
    type Data = AnimatedLengthData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.getter("baseVal", |s, t| Ok(svg_length(s, &t.el, t.a, false)?.value()));
        def.getter("animVal", |s, t| Ok(svg_length(s, &t.el, t.a, true)?.value()));
    }
}

/// The element's `SVGAnimatedLength` for `a`, the same object each time.
pub fn animated_length<'s>(s: &Scope<'s>, page: &PageDom, id: NodeId, a: &'static SvgLengthAttr) -> JsResult<Object<'s>> {
    let key = coll_identity(page, a.slot, id);
    if let Some(o) = s.lookup_identity(key) {
        return Ok(o);
    }
    let page = page_rc(s)?;
    s.wrap::<SvgAnimatedLength>(AnimatedLengthData { el: NodeHold::new(&page, id)?, a }, Some(key))
}

// ----- SVGAnimatedTransformList ----------------------------------------------------

use pyro_dom::svg_point_list::{parse_transform_list, TransformItem, TransformLists, TransformValue};

/// The identity `SVGMatrix` as `[a b c d e f]`.
pub const IDENTITY_MATRIX: [f64; 6] = TransformValue::IDENTITY;

/// `SVGAnimatedTransformList`: `baseVal` is the list of the attribute, `animVal` that list under the
/// `<animateTransform>`s in effect. The CSS `transform` property is not in it (WPT css-transforms
/// `css-transforms-transformlist`).
pub struct SvgAnimatedTransformList;
impl JsClass for SvgAnimatedTransformList {
    const NAME: &'static str = "SVGAnimatedTransformList";
    type Data = Rc<TransformLists>;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.getter("baseVal", |s, lists| transform_list_view(s, lists, false));
        def.getter("animVal", |s, lists| transform_list_view(s, lists, true));
    }
}

fn transform_list_view<'s>(s: &Scope<'s>, lists: &Rc<TransformLists>, animated: bool) -> JsResult<Value<'s>> {
    let key = IdentityKey((1 << 59) | (6 << 56) | u64::from(animated), Rc::as_ptr(lists) as u64);
    Ok(s.wrap::<SvgTransformList>(SvgListData { lists: lists.clone(), animated }, Some(key))?.value())
}

/// `SVGTransformList`: the live list of the attribute's functions (`animVal`: read only, under the
/// animations in effect). An item inserted into a list is the object the script holds, so editing
/// it edits the attribute; one that is already in a list is copied (`ListItem::insertion_value`).
pub struct SvgTransformList;
impl JsClass for SvgTransformList {
    const NAME: &'static str = "SVGTransformList";
    type Data = SvgListData<TransformValue>;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        define_svg_list(def);
        def.method("createSVGTransformFromMatrix", 0, |s, _l, a| {
            let matrix = matrix_init_argument(s, a.get(0))?;
            detached_transform(s, matrix)
        });
        def.method("consolidate", 0, |s, l, _a| {
            let lists = l.writable()?;
            let count = lists.len(false);
            if count == 0 {
                return Ok(s.null());
            }
            // The single item is already the consolidation.
            if count == 1 {
                return transform_item(s, lists.item(0, false).ok_or_else(|| JsError::dom("IndexSizeError", "The index is outside the SVG list."))?);
            }
            let mut matrix = IDENTITY_MATRIX;
            for i in 0..count {
                if let Some(item) = lists.item(i, false) {
                    matrix = pyro_dom::svg_geometry::multiply(matrix, item.value().matrix);
                }
            }
            let item = TransformItem::detached(TransformValue::from_matrix(matrix));
            transform_item(s, lists.initialize(&item).js()?)
        });
    }
}

impl SvgItemList<TransformValue> for SvgTransformList {
    fn wrap_item<'s>(s: &Scope<'s>, item: Rc<TransformItem>) -> JsResult<Value<'s>> {
        transform_item(s, item)
    }
    fn unwrap_item(s: &Scope<'_>, value: Value<'_>) -> JsResult<Rc<TransformItem>> {
        let transform = s.unwrap::<SvgTransform>(value).ok_or_else(|| JsError::Type("Expected an SVGTransform.".into()))?;
        Ok(Rc::clone(&*transform))
    }
}

fn transform_item<'s>(s: &Scope<'s>, item: Rc<TransformItem>) -> JsResult<Value<'s>> {
    // As the points: one wrapper per item for as long as it is held.
    let key = IdentityKey((1 << 59) | (7 << 56), Rc::as_ptr(&item) as u64);
    Ok(s.wrap::<SvgTransform>(item, Some(key))?.value())
}

/// A detached `SVGTransform` of type SVG_TRANSFORM_MATRIX (`createSVGTransform` and
/// `createSVGTransformFromMatrix`).
pub fn detached_transform<'s>(s: &Scope<'s>, matrix: [f64; 6]) -> JsResult<Value<'s>> {
    transform_item(s, TransformItem::detached(TransformValue::from_matrix(matrix)))
}

/// A detached `SVGMatrix`.
pub fn detached_matrix<'s>(s: &Scope<'s>, matrix: [f64; 6]) -> JsResult<Value<'s>> {
    Ok(s.wrap::<SvgMatrix>(Cell::new(matrix), None)?.value())
}

/// The six numbers of an `SVGMatrix` argument (a TypeError for anything else).
pub fn matrix_argument(s: &Scope<'_>, value: Value<'_>) -> JsResult<[f64; 6]> {
    let matrix = s.unwrap::<SvgMatrix>(value).ok_or_else(|| JsError::Type("Expected an SVGMatrix.".into()))?;
    Ok(matrix.get())
}

/// `createSVGTransformFromMatrix`'s `optional DOMMatrix2DInit`: an `SVGMatrix` (or any object with
/// the members `a` .. `f`, the identity's by default; `undefined` is the identity).
pub fn matrix_init_argument(s: &Scope<'_>, value: Value<'_>) -> JsResult<[f64; 6]> {
    if let Some(matrix) = s.unwrap::<SvgMatrix>(value) {
        return Ok(matrix.get());
    }
    if value.is_undefined() {
        return Ok(IDENTITY_MATRIX);
    }
    let Some(init) = value.as_object() else {
        return Err(JsError::Type("The provided value is not of type 'DOMMatrix2DInit'.".into()));
    };
    let mut matrix = IDENTITY_MATRIX;
    for (slot, name) in matrix.iter_mut().zip(["a", "b", "c", "d", "e", "f"]) {
        let member = init.get_atom(name)?;
        if !member.is_undefined() {
            *slot = transform_number(member)?;
        }
    }
    Ok(matrix)
}

/// A Web IDL `float` argument: a TypeError for NaN and the infinities.
fn transform_number(v: Value<'_>) -> JsResult<f64> {
    let n = v.to_number()?;
    if !n.is_finite() {
        return Err(JsError::Type("The provided float value is non-finite.".into()));
    }
    Ok(n)
}

/// `SVGTransform`: its type, angle and the six numbers of its matrix (`matrix.a` .. `matrix.f`).
pub struct SvgTransform;
impl JsClass for SvgTransform {
    const NAME: &'static str = "SVGTransform";
    type Data = Rc<TransformItem>;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        for (name, value) in [
            ("SVG_TRANSFORM_UNKNOWN", 0.0),
            ("SVG_TRANSFORM_MATRIX", 1.0),
            ("SVG_TRANSFORM_TRANSLATE", 2.0),
            ("SVG_TRANSFORM_SCALE", 3.0),
            ("SVG_TRANSFORM_ROTATE", 4.0),
            ("SVG_TRANSFORM_SKEWX", 5.0),
            ("SVG_TRANSFORM_SKEWY", 6.0),
        ] {
            def.constant(name, value);
        }
        def.getter("type", |s, t| Ok(s.number(f64::from(t.value().kind))));
        def.getter("angle", |s, t| Ok(s.number(t.value().angle)));
        def.getter("matrix", |s, t| detached_matrix(s, t.value().matrix));
        def.method("setMatrix", 1, |s, t, a| {
            a.require(1, "SVGTransform.setMatrix")?;
            let matrix = matrix_argument(s, a.get(0))?;
            t.set_value(TransformValue::from_matrix(matrix)).js()?;
            Ok(s.undefined())
        });
        def.method("setTranslate", 2, |s, t, a| {
            a.require(2, "SVGTransform.setTranslate")?;
            let (x, y) = (transform_number(a.get(0))?, transform_number(a.get(1))?);
            t.set_value(TransformValue::translate(x, y)).js()?;
            Ok(s.undefined())
        });
        def.method("setScale", 2, |s, t, a| {
            a.require(2, "SVGTransform.setScale")?;
            let (x, y) = (transform_number(a.get(0))?, transform_number(a.get(1))?);
            t.set_value(TransformValue::scale(x, y)).js()?;
            Ok(s.undefined())
        });
        def.method("setRotate", 3, |s, t, a| {
            a.require(3, "SVGTransform.setRotate")?;
            let (angle, cx, cy) = (transform_number(a.get(0))?, transform_number(a.get(1))?, transform_number(a.get(2))?);
            t.set_value(TransformValue::rotate(angle, cx, cy)).js()?;
            Ok(s.undefined())
        });
        def.method("setSkewX", 1, |s, t, a| {
            a.require(1, "SVGTransform.setSkewX")?;
            t.set_value(TransformValue::skew_x(transform_number(a.get(0))?)).js()?;
            Ok(s.undefined())
        });
        def.method("setSkewY", 1, |s, t, a| {
            a.require(1, "SVGTransform.setSkewY")?;
            t.set_value(TransformValue::skew_y(transform_number(a.get(0))?)).js()?;
            Ok(s.undefined())
        });
    }
}

/// Sets component `index` of the matrix (`a` .. `f`).
fn set_matrix_component(m: &Cell<[f64; 6]>, index: usize, v: Value<'_>) -> JsResult<()> {
    let n = transform_number(v)?;
    let mut matrix = m.get();
    matrix[index] = n;
    m.set(matrix);
    Ok(())
}

/// A new `SVGMatrix`: `m` followed by `next` (the matrix the operation multiplies by).
fn matrix_then<'s>(s: &Scope<'s>, m: &Cell<[f64; 6]>, next: [f64; 6]) -> JsResult<Value<'s>> {
    detached_matrix(s, pyro_dom::svg_geometry::multiply(m.get(), next))
}

/// `SVGMatrix`: `a` .. `f` and the operations, each returning a new matrix (the receiver is not changed).
pub struct SvgMatrix;
impl JsClass for SvgMatrix {
    const NAME: &'static str = "SVGMatrix";
    type Data = Cell<[f64; 6]>;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.accessor("a", |s, m| Ok(s.number(m.get()[0])), |_s, m, v| set_matrix_component(m, 0, v));
        def.accessor("b", |s, m| Ok(s.number(m.get()[1])), |_s, m, v| set_matrix_component(m, 1, v));
        def.accessor("c", |s, m| Ok(s.number(m.get()[2])), |_s, m, v| set_matrix_component(m, 2, v));
        def.accessor("d", |s, m| Ok(s.number(m.get()[3])), |_s, m, v| set_matrix_component(m, 3, v));
        def.accessor("e", |s, m| Ok(s.number(m.get()[4])), |_s, m, v| set_matrix_component(m, 4, v));
        def.accessor("f", |s, m| Ok(s.number(m.get()[5])), |_s, m, v| set_matrix_component(m, 5, v));
        def.method("multiply", 1, |s, m, a| {
            a.require(1, "SVGMatrix.multiply")?;
            let other = matrix_argument(s, a.get(0))?;
            matrix_then(s, m, other)
        });
        def.method("inverse", 0, |s, m, _a| {
            let [a, b, c, d, e, f] = m.get();
            let det = a * d - b * c;
            if det == 0.0 || !det.is_finite() {
                return Err(JsError::dom("InvalidStateError", "The matrix is not invertible."));
            }
            detached_matrix(s, [d / det, -b / det, -c / det, a / det, (c * f - d * e) / det, (b * e - a * f) / det])
        });
        def.method("translate", 2, |s, m, a| {
            a.require(2, "SVGMatrix.translate")?;
            let (x, y) = (transform_number(a.get(0))?, transform_number(a.get(1))?);
            matrix_then(s, m, TransformValue::translate(x, y).matrix)
        });
        def.method("scale", 1, |s, m, a| {
            a.require(1, "SVGMatrix.scale")?;
            let factor = transform_number(a.get(0))?;
            matrix_then(s, m, TransformValue::scale(factor, factor).matrix)
        });
        def.method("scaleNonUniform", 2, |s, m, a| {
            a.require(2, "SVGMatrix.scaleNonUniform")?;
            let (x, y) = (transform_number(a.get(0))?, transform_number(a.get(1))?);
            matrix_then(s, m, TransformValue::scale(x, y).matrix)
        });
        def.method("rotate", 1, |s, m, a| {
            a.require(1, "SVGMatrix.rotate")?;
            let angle = transform_number(a.get(0))?;
            matrix_then(s, m, TransformValue::rotate(angle, 0.0, 0.0).matrix)
        });
        def.method("rotateFromVector", 2, |s, m, a| {
            a.require(2, "SVGMatrix.rotateFromVector")?;
            let (x, y) = (transform_number(a.get(0))?, transform_number(a.get(1))?);
            if x == 0.0 || y == 0.0 {
                return Err(JsError::dom("InvalidAccessError", "The vector has a zero component."));
            }
            matrix_then(s, m, TransformValue::rotate(y.atan2(x).to_degrees(), 0.0, 0.0).matrix)
        });
        def.method("flipX", 0, |s, m, _a| matrix_then(s, m, [-1.0, 0.0, 0.0, 1.0, 0.0, 0.0]));
        def.method("flipY", 0, |s, m, _a| matrix_then(s, m, [1.0, 0.0, 0.0, -1.0, 0.0, 0.0]));
        def.method("skewX", 1, |s, m, a| {
            a.require(1, "SVGMatrix.skewX")?;
            let angle = transform_number(a.get(0))?;
            matrix_then(s, m, TransformValue::skew_x(angle).matrix)
        });
        def.method("skewY", 1, |s, m, a| {
            a.require(1, "SVGMatrix.skewY")?;
            let angle = transform_number(a.get(0))?;
            matrix_then(s, m, TransformValue::skew_y(angle).matrix)
        });
    }
}

/// The element's `SVGAnimatedTransformList` of `attr` (`transform`, `gradientTransform`,
/// `patternTransform`), the same object each time.
pub fn animated_transform_list<'s>(s: &Scope<'s>, page: &PageDom, id: NodeId, attr: &'static str) -> JsResult<Object<'s>> {
    let key = coll_identity(page, 32, id);
    if let Some(o) = s.lookup_identity(key) {
        return Ok(o);
    }
    let page = page_rc(s)?;
    let hold = NodeHold::new(&page, id)?;
    let write = Box::new(move |text: &str| {
        hold.page.dom.borrow_mut().set_attribute_local(hold.id, &pyro_dom::LocalName::from(attr), text)
    });
    let lists = page.dom.borrow_mut().svg_transform_lists(id, attr, write).js()?;
    // `animVal` is the list under the SMIL animations in effect.
    let animated = NodeHold::new(&page, id)?;
    lists.set_animation(Box::new(move |base: &[TransformValue]| {
        let contributions = attribute_contributions(&animated.page, animated.id, &[attr]);
        if contributions.is_empty() {
            return None;
        }
        Some(animated_transform_items(&contributions, base))
    }));
    s.wrap::<SvgAnimatedTransformList>(lists, Some(key))
}

/// SMIL 3.3: the transform list `base` under the `<animateTransform>`s `contributions` (in sandwich
/// order): an additive animation adds its transform to the list below it; any other replaces the list.
fn animated_transform_items(contributions: &[crate::smil::Contribution], base: &[TransformValue]) -> Vec<TransformValue> {
    let mut items = base.to_vec();
    for c in contributions {
        let (Some(kind), Some(args)) = (c.transform_type(), crate::smil::evaluate(&TransformSpace, c, &Vec::new())) else {
            continue;
        };
        let args: Vec<String> = args.iter().map(|v| v.to_string()).collect();
        let animation = parse_transform_list(&format!("{kind}({})", args.join(" ")));
        if c.additive() && !items.is_empty() {
            items.extend(animation);
        } else {
            items = animation;
        }
    }
    items
}

/// The matrix of the `transform` attribute of `id` under the `<animateTransform>`s in effect on it (its
/// `animVal`), for `getBBox()` and `getCTM()`; `None` when no animation changes it.
pub(crate) fn animated_transform_matrix(page: &PageDom, id: NodeId) -> Option<[f64; 6]> {
    let contributions = attribute_contributions(page, id, &["transform"]);
    if contributions.is_empty() {
        return None;
    }
    let base = page.dom.borrow().get_attribute(id, "transform").ok().flatten().map(parse_transform_list).unwrap_or_default();
    let items = animated_transform_items(&contributions, &base);
    Some(items.iter().fold(IDENTITY_MATRIX, |matrix, item| pyro_dom::svg_geometry::multiply(matrix, item.matrix)))
}

// ----- SVGRect, SVGAnimatedRect -------------------------------------------------------

/// `SVGSVGElement.createSVGRect`: a detached rectangle of zeros.
pub fn detached_rect<'s>(s: &Scope<'s>) -> JsResult<Value<'s>> {
    let page = page_rc(s)?;
    crate::canvas::svg_rect_value(s, &page, pyro_dom::layout::Rect { x: 0.0, y: 0.0, width: 0.0, height: 0.0 })
}

/// The four numbers of an element's `viewBox` attribute; zeros when it is absent or not a valid
/// `<min-x> <min-y> <width> <height>` (a negative width or height is an error).
fn read_view_box(page: &PageDom, id: NodeId) -> [f64; 4] {
    let dom = page.dom.borrow();
    let Some(text) = dom.get_attribute(id, "viewBox").ok().flatten() else { return [0.0; 4] };
    let numbers: Option<Vec<f64>> = text
        .split(|c: char| c.is_ascii_whitespace() || c == ',')
        .filter(|p| !p.is_empty())
        .map(svg_number)
        .collect();
    match numbers.as_deref() {
        Some(&[x, y, w, h]) if w >= 0.0 && h >= 0.0 => [x, y, w, h],
        _ => [0.0; 4],
    }
}

/// `SVGAnimatedRect` of `viewBox`: `baseVal` and `animVal` are live `SVGRect`s of the attribute
/// (`animVal` read only), each the same object every time (kept on this wrapper).
pub struct SvgAnimatedRect;
impl JsClass for SvgAnimatedRect {
    const NAME: &'static str = "SVGAnimatedRect";
    type Data = NodeHold;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.getter_this("baseVal", |s, el, this| view_box_rect(s, el, this, false));
        def.getter_this("animVal", |s, el, this| view_box_rect(s, el, this, true));
    }
}

fn view_box_rect<'s>(s: &Scope<'s>, el: &NodeHold, this: Object<'s>, animated: bool) -> JsResult<Value<'s>> {
    let key = if animated { "__lpViewBoxAnim" } else { "__lpViewBoxBase" };
    if let Some(rect) = s.get_hidden(this, key)? {
        return Ok(rect);
    }
    let reader = Rc::new(NodeHold::new(&el.page, el.id)?);
    let writer = reader.clone();
    let read = s.function("read", move |s, _this, _a| {
        let numbers = if animated { animated_view_box(&reader.page, reader.id) } else { read_view_box(&reader.page, reader.id) };
        Ok(s.array(&numbers.map(|v| s.number(v)))?.value())
    })?;
    let write = s.function("write", move |s, _this, a| {
        let index = a.get(0).to_number()? as usize;
        let value = a.get(1).to_number()?;
        let mut numbers = read_view_box(&writer.page, writer.id);
        if let Some(slot) = numbers.get_mut(index) {
            *slot = value;
        }
        let text = format!("{} {} {} {}", numbers[0], numbers[1], numbers[2], numbers[3]);
        writer.page.dom.borrow_mut().set_attribute(writer.id, "viewBox", &text).js()?;
        Ok(s.undefined())
    })?;
    let page = page_rc(s)?;
    let rect = crate::canvas::view_box_rect(s, &page, read, write, animated)?;
    s.set_hidden(this, key, rect)?;
    Ok(rect)
}

/// The four numbers of `viewBox` under the SMIL animations in effect on it (`animVal`); the attribute's
/// own when none animates it or the animated value is not a valid rectangle.
fn animated_view_box(page: &PageDom, id: NodeId) -> [f64; 4] {
    let base = read_view_box(page, id);
    let contributions = attribute_contributions(page, id, &["viewBox"]);
    if contributions.is_empty() {
        return base;
    }
    match crate::smil::fold(&NumberSpace { pair: false }, &contributions, base.to_vec()).as_slice() {
        &[x, y, w, h] if w >= 0.0 && h >= 0.0 => [x, y, w, h],
        _ => base,
    }
}

/// The element's `SVGAnimatedRect` for `viewBox`, the same object each time.
pub fn animated_rect<'s>(s: &Scope<'s>, page: &PageDom, id: NodeId) -> JsResult<Object<'s>> {
    let key = coll_identity(page, 470, id);
    if let Some(o) = s.lookup_identity(key) {
        return Ok(o);
    }
    let page = page_rc(s)?;
    s.wrap::<SvgAnimatedRect>(NodeHold::new(&page, id)?, Some(key))
}

// ----- SMIL: the animated values of the SVG types --------------------------------------------------

use crate::smil::Space;

/// The SMIL animations in effect on the first of `names` that any animates, of `id`.
fn attribute_contributions(page: &PageDom, id: NodeId, names: &[&str]) -> Vec<crate::smil::Contribution> {
    names.iter().flat_map(|name| crate::smil::animated(page, id, name)).collect()
}

/// The animated value, in user units, of the geometry length attribute `name` of `id` (what its `animVal`
/// has: `x`, `width`, `cx`, `r`, ...) for `getBBox()`; `None` when no animation changes it, or the value
/// is in a unit that needs the layout (`%`, `em`: the attribute's own reading stays).
pub(crate) fn animated_geometry_number(page: &PageDom, id: NodeId, name: &str) -> Option<f64> {
    let contributions = attribute_contributions(page, id, &[name]);
    if contributions.is_empty() {
        return None;
    }
    let base = page.dom.borrow().get_attribute(id, name).ok().flatten().and_then(parse_svg_length).unwrap_or((0.0, 1));
    // (Absolute units only: this runs under the layout's borrow, which `unit_factor` takes again.)
    let factor = |kind: u16| match kind {
        1 | 5 => 1.0,
        6 => 96.0 / 2.54,
        7 => 96.0 / 25.4,
        8 => 96.0,
        9 => 4.0 / 3.0,
        10 => 16.0,
        _ => 0.0,
    };
    let space = LengthSpace { factor: &factor };
    let (value, kind) = crate::smil::fold(&space, &contributions, vec![base]).first().copied().unwrap_or(base);
    let scale = factor(kind);
    (scale != 0.0).then(|| value * scale)
}

/// What SMIL animation changes in the geometry of `page`'s elements at its current time, for
/// `getBBox()`, `getCTM()` and `getScreenCTM()` (`pyro_dom::svg_geometry::Animated`).
pub(crate) struct SmilGeometry<'a> {
    page: &'a PageDom,
    /// The `<animateMotion>`s' transforms, sampled once.
    motion: crate::smil::Geometry,
}

impl<'a> SmilGeometry<'a> {
    /// Call it before borrowing the layout.
    pub(crate) fn new(page: &'a PageDom) -> SmilGeometry<'a> {
        SmilGeometry { page, motion: crate::smil::geometry(page) }
    }
}

impl pyro_dom::svg_geometry::Animated for SmilGeometry<'_> {
    fn motion(&self, el: NodeId) -> Option<[f64; 6]> {
        pyro_dom::svg_geometry::Animated::motion(&self.motion, el)
    }

    fn number(&self, el: NodeId, name: &str) -> Option<f64> {
        animated_geometry_number(self.page, el, name)
    }

    fn transform(&self, el: NodeId) -> Option<[f64; 6]> {
        animated_transform_matrix(self.page, el)
    }
}

/// The numbers of a list separated by whitespace and/or commas.
fn split_values(text: &str) -> Vec<&str> {
    text.split(|c: char| c == ',' || c.is_ascii_whitespace()).filter(|t| !t.is_empty()).collect()
}

fn numbers_lerp(a: &[f64], b: &[f64], t: f64) -> Vec<f64> {
    if a.len() == b.len() {
        a.iter().zip(b).map(|(x, y)| x + (y - x) * t).collect()
    } else if t < 0.5 {
        a.to_vec()
    } else {
        b.to_vec()
    }
}

fn numbers_add(a: &[f64], b: &[f64]) -> Vec<f64> {
    if a.len() == b.len() {
        a.iter().zip(b).map(|(x, y)| x + y).collect()
    } else {
        b.to_vec()
    }
}

fn numbers_distance(a: &[f64], b: &[f64]) -> Option<f64> {
    (a.len() == b.len()).then(|| a.iter().zip(b).map(|(x, y)| (x - y) * (x - y)).sum::<f64>().sqrt())
}

/// Strings, keywords and booleans: they do not interpolate (the first half of a interval is the
/// first value) and do not add.
struct TextSpace;
impl Space for TextSpace {
    type V = String;
    fn parse(&self, text: &str, _kind: Option<&str>) -> Option<String> {
        Some(text.trim().to_owned())
    }
    fn lerp(&self, a: &String, b: &String, t: f64) -> String {
        if t < 0.5 { a.clone() } else { b.clone() }
    }
    fn add(&self, _a: &String, b: &String) -> String {
        b.clone()
    }
    fn scale(&self, a: &String, _k: f64) -> String {
        a.clone()
    }
    fn zero(&self, _like: &String) -> String {
        String::new()
    }
    fn additive(&self) -> bool {
        false
    }
    fn interpolates(&self) -> bool {
        false
    }
}

/// A `<number>`, a pair of them (`pair`: `<number-optional-number>`, one number being that number
/// twice), or a list.
struct NumberSpace {
    pair: bool,
}
impl Space for NumberSpace {
    type V = Vec<f64>;
    fn parse(&self, text: &str, _kind: Option<&str>) -> Option<Vec<f64>> {
        let mut numbers: Vec<f64> = split_values(text.trim()).into_iter().map(svg_number).collect::<Option<_>>()?;
        if numbers.is_empty() || (self.pair && numbers.len() > 2) {
            return None;
        }
        if self.pair && numbers.len() == 1 {
            numbers.push(numbers[0]);
        }
        Some(numbers)
    }
    fn lerp(&self, a: &Vec<f64>, b: &Vec<f64>, t: f64) -> Vec<f64> {
        numbers_lerp(a, b, t)
    }
    fn add(&self, a: &Vec<f64>, b: &Vec<f64>) -> Vec<f64> {
        numbers_add(a, b)
    }
    fn scale(&self, a: &Vec<f64>, k: f64) -> Vec<f64> {
        a.iter().map(|x| x * k).collect()
    }
    fn zero(&self, like: &Vec<f64>) -> Vec<f64> {
        vec![0.0; like.len()]
    }
    fn distance(&self, a: &Vec<f64>, b: &Vec<f64>) -> Option<f64> {
        numbers_distance(a, b)
    }
}

/// The arguments of an `<animateTransform>` value of `type` `kind`, filled in as the transform
/// function has them (`translate` y 0, `scale` y x, `rotate` about the origin).
fn transform_arguments(kind: &str, n: &[f64]) -> Option<Vec<f64>> {
    match (kind, n) {
        ("translate", [x]) => Some(vec![*x, 0.0]),
        ("translate", [x, y]) => Some(vec![*x, *y]),
        ("scale", [x]) => Some(vec![*x, *x]),
        ("scale", [x, y]) => Some(vec![*x, *y]),
        ("rotate", [a]) => Some(vec![*a, 0.0, 0.0]),
        ("rotate", [a, x, y]) => Some(vec![*a, *x, *y]),
        ("skewX", [a]) | ("skewY", [a]) => Some(vec![*a]),
        _ => None,
    }
}

/// The arguments of one transform function, interpolated one by one.
struct TransformSpace;
impl Space for TransformSpace {
    type V = Vec<f64>;
    fn parse(&self, text: &str, kind: Option<&str>) -> Option<Vec<f64>> {
        let numbers: Vec<f64> = split_values(text.trim()).into_iter().map(svg_number).collect::<Option<_>>()?;
        transform_arguments(kind?, &numbers)
    }
    fn lerp(&self, a: &Vec<f64>, b: &Vec<f64>, t: f64) -> Vec<f64> {
        numbers_lerp(a, b, t)
    }
    fn add(&self, a: &Vec<f64>, b: &Vec<f64>) -> Vec<f64> {
        numbers_add(a, b)
    }
    fn scale(&self, a: &Vec<f64>, k: f64) -> Vec<f64> {
        a.iter().map(|x| x * k).collect()
    }
    fn zero(&self, like: &Vec<f64>) -> Vec<f64> {
        vec![0.0; like.len()]
    }
    fn distance(&self, a: &Vec<f64>, b: &Vec<f64>) -> Option<f64> {
        numbers_distance(a, b)
    }
}

/// A list of `<length>`s. Lengths of different units interpolate in user units (the result in the
/// unit of the nearer end); `factor` is the user units of a unit.
struct LengthSpace<'a> {
    factor: &'a dyn Fn(u16) -> f64,
}
impl LengthSpace<'_> {
    fn lerp_one(&self, a: (f64, u16), b: (f64, u16), t: f64) -> (f64, u16) {
        if a.1 == b.1 {
            return (a.0 + (b.0 - a.0) * t, a.1);
        }
        let (fa, fb) = ((self.factor)(a.1), (self.factor)(b.1));
        if fa == 0.0 || fb == 0.0 {
            return if t < 0.5 { a } else { b };
        }
        let user = fa * a.0 + (fb * b.0 - fa * a.0) * t;
        if t < 0.5 { (user / fa, a.1) } else { (user / fb, b.1) }
    }
    fn add_one(&self, a: (f64, u16), b: (f64, u16)) -> (f64, u16) {
        if a.1 == b.1 {
            return (a.0 + b.0, a.1);
        }
        let (fa, fb) = ((self.factor)(a.1), (self.factor)(b.1));
        if fb == 0.0 {
            return b;
        }
        ((fa * a.0 + fb * b.0) / fb, b.1)
    }
    fn user(&self, a: (f64, u16)) -> f64 {
        a.0 * (self.factor)(a.1)
    }
}
impl Space for LengthSpace<'_> {
    type V = Vec<(f64, u16)>;
    fn parse(&self, text: &str, _kind: Option<&str>) -> Option<Vec<(f64, u16)>> {
        let lengths: Vec<(f64, u16)> = split_values(text.trim()).into_iter().map(parse_svg_length).collect::<Option<_>>()?;
        (!lengths.is_empty()).then_some(lengths)
    }
    fn lerp(&self, a: &Vec<(f64, u16)>, b: &Vec<(f64, u16)>, t: f64) -> Vec<(f64, u16)> {
        if a.len() != b.len() {
            return if t < 0.5 { a.clone() } else { b.clone() };
        }
        a.iter().zip(b).map(|(x, y)| self.lerp_one(*x, *y, t)).collect()
    }
    fn add(&self, a: &Vec<(f64, u16)>, b: &Vec<(f64, u16)>) -> Vec<(f64, u16)> {
        if a.len() != b.len() {
            return b.clone();
        }
        a.iter().zip(b).map(|(x, y)| self.add_one(*x, *y)).collect()
    }
    fn scale(&self, a: &Vec<(f64, u16)>, k: f64) -> Vec<(f64, u16)> {
        a.iter().map(|(v, unit)| (v * k, *unit)).collect()
    }
    fn zero(&self, like: &Vec<(f64, u16)>) -> Vec<(f64, u16)> {
        like.iter().map(|(_, unit)| (0.0, *unit)).collect()
    }
    fn distance(&self, a: &Vec<(f64, u16)>, b: &Vec<(f64, u16)>) -> Option<f64> {
        (a.len() == b.len()).then(|| a.iter().zip(b).map(|(x, y)| (self.user(*x) - self.user(*y)).powi(2)).sum::<f64>().sqrt())
    }
}

/// An `SVGAngle`, or a keyword where the attribute takes one (a marker's `orient="auto"`).
#[derive(Clone)]
enum AngleVal {
    Angle(Angle),
    Keyword(String),
}

/// An angle: units convert through degrees, the result in the unit of the nearer end.
struct AngleSpace;
impl Space for AngleSpace {
    type V = AngleVal;
    fn parse(&self, text: &str, _kind: Option<&str>) -> Option<AngleVal> {
        match Angle::parse(text) {
            Some(angle) => Some(AngleVal::Angle(angle)),
            None => Some(AngleVal::Keyword(text.trim().to_owned())).filter(|_| !text.trim().is_empty()),
        }
    }
    fn lerp(&self, a: &AngleVal, b: &AngleVal, t: f64) -> AngleVal {
        match (a, b) {
            (AngleVal::Angle(x), AngleVal::Angle(y)) => {
                let unit = if t < 0.5 { x.unit } else { y.unit };
                let degrees = x.degrees() + (y.degrees() - x.degrees()) * t;
                AngleVal::Angle(Angle { value: Angle::clamp(degrees / Angle::factor(unit)), unit })
            }
            _ if t < 0.5 => a.clone(),
            _ => b.clone(),
        }
    }
    fn add(&self, a: &AngleVal, b: &AngleVal) -> AngleVal {
        match (a, b) {
            (AngleVal::Angle(x), AngleVal::Angle(y)) => {
                AngleVal::Angle(Angle { value: Angle::clamp((x.degrees() + y.degrees()) / Angle::factor(x.unit)), unit: x.unit })
            }
            _ => b.clone(),
        }
    }
    fn scale(&self, a: &AngleVal, k: f64) -> AngleVal {
        match a {
            AngleVal::Angle(x) => AngleVal::Angle(Angle { value: Angle::clamp(f64::from(x.value) * k), unit: x.unit }),
            AngleVal::Keyword(_) => a.clone(),
        }
    }
    fn zero(&self, like: &AngleVal) -> AngleVal {
        match like {
            AngleVal::Angle(x) => AngleVal::Angle(Angle { value: 0.0, unit: x.unit }),
            AngleVal::Keyword(_) => AngleVal::Angle(Angle::ZERO),
        }
    }
    fn distance(&self, a: &AngleVal, b: &AngleVal) -> Option<f64> {
        match (a, b) {
            (AngleVal::Angle(x), AngleVal::Angle(y)) => Some((x.degrees() - y.degrees()).abs()),
            _ => None,
        }
    }
}

pub(crate) fn register(engine: &pyro_js::Engine) {
    engine.register::<SvgStringList>();
    engine.register::<SvgAnimatedTransformList>();
    engine.register::<SvgTransformList>();
    engine.register::<SvgTransform>();
    engine.register::<SvgMatrix>();
    engine.register::<SvgAnimatedRect>();
    engine.register::<SvgAnimatedEnumeration>();
    engine.register::<SvgAnimatedString>();
    engine.register::<SvgNumber>();
    engine.register::<SvgAnimatedNumberList>();
    engine.register::<SvgNumberList>();
    engine.register::<SvgAnimatedNumber>();
    engine.register::<SvgAnimatedInteger>();
    engine.register::<SvgAnimatedBoolean>();
    engine.register::<SvgAngle>();
    engine.register::<SvgAnimatedAngle>();
    svg_aspect::register(engine);
    engine.register::<SvgLengthList>();
    engine.register::<SvgAnimatedLengthList>();
    engine.register::<SvgAnimatedLength>();
    engine.register::<SvgLength>();
    engine.register::<SvgPoint>();
    engine.register::<SvgPointList>();
    engine.register::<NodeList>();
    engine.register::<RadioNodeList>();
    engine.register::<HtmlCollection>();
    engine.register::<HtmlOptionsCollection>();
    engine.register::<HtmlFormControlsCollection>();
    engine.register::<DomTokenList>();
    engine.register::<NamedNodeMap>();
    engine.register::<Attr>();
    engine.register::<DomStringMap>();
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dataset_names() {
        assert_eq!(camel_to_data_attr("fooBar").as_deref(), Some("data-foo-bar"));
        assert_eq!(data_attr_to_camel("data-foo-bar").as_deref(), Some("fooBar"));
        assert_eq!(data_attr_to_camel("data-x").as_deref(), Some("x"));
        assert_eq!(data_attr_to_camel("data-foo--bar").as_deref(), Some("foo-Bar"));
        assert_eq!(data_attr_to_camel("data--").as_deref(), Some("-"));
        assert_eq!(data_attr_to_camel("data-Foo"), None);
        assert_eq!(data_attr_to_camel("data-\u{e9}t\u{e9}").as_deref(), Some("\u{e9}t\u{e9}"));
        assert_eq!(camel_to_data_attr("\u{e0}").as_deref(), Some("data-\u{e0}"));
        assert_eq!(camel_to_data_attr("\u{37e}foo").as_deref(), Some("data-\u{37e}foo"));
        assert_eq!(camel_to_data_attr("a-b"), None);
        assert_eq!(camel_to_data_attr("a-\u{e9}").as_deref(), Some("data-a-\u{e9}"));
    }
}

/// A detached SVG point or the live pan of an SVG viewport.
pub enum SvgPointData {
    Item(Rc<pyro_dom::svg_point_list::PointItem>),
    Translate(NodeHold),
}

impl SvgPointData {
    fn coordinates(&self) -> (f64, f64) {
        match self {
            Self::Item(value) => {
                let (x, y) = value.coordinates();
                (f64::from(x), f64::from(y))
            }
            Self::Translate(node) => {
                let (x, y, _) = node.page.dom.borrow().svg_pan(node.id);
                (x, y)
            }
        }
    }

    fn set(&self, value: Value<'_>, horizontal: bool) -> JsResult<()> {
        let value = value.to_number()? as f32;
        if !value.is_finite() {
            return Err(JsError::Type("The provided float value is non-finite.".into()));
        }
        match self {
            Self::Item(point) => {
                let (x, y) = point.coordinates();
                point.set_coordinates(if horizontal { (value, y) } else { (x, value) }).js()?;
            }
            Self::Translate(node) => {
                let mut dom = node.page.dom.borrow_mut();
                let (x, y, scale) = dom.svg_pan(node.id);
                let position = if horizontal { (f64::from(value), y, scale) } else { (x, f64::from(value), scale) };
                dom.set_svg_pan(node.id, position);
            }
        }
        Ok(())
    }
}

pub fn detached_point<'s>(s: &Scope<'s>, x: f64, y: f64) -> JsResult<Value<'s>> {
    point_value(s, PointItem::detached((x as f32, y as f32)))
}

pub struct SvgPoint;
impl JsClass for SvgPoint {
    const NAME: &'static str = "SVGPoint";
    type Data = SvgPointData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.accessor("x", |s, p| Ok(s.number(p.coordinates().0)), |_s, p, v| p.set(v, true));
        def.accessor("y", |s, p| Ok(s.number(p.coordinates().1)), |_s, p, v| p.set(v, false));
        def.getter("w", |s, _| Ok(s.number(1.0)));
        def.getter("z", |s, _| Ok(s.number(0.0)));
        def.method("matrixTransform", 1, |s, p, args| {
            args.require(1, "SVGPoint.matrixTransform")?;
            let matrix = matrix_argument(s, args.get(0))?;
            let (x, y) = p.coordinates();
            detached_point(s, matrix[0] * x + matrix[2] * y + matrix[4], matrix[1] * x + matrix[3] * y + matrix[5])
        });
    }
}

use pyro_dom::svg_point_list::PointItem;

/// `SVGSVGElement.currentTranslate` (`[SameObject]`): one wrapper per element while it is held.
pub(crate) fn current_translate<'s>(s: &Scope<'s>, page: &Rc<PageDom>, id: NodeId) -> JsResult<Value<'s>> {
    let key = coll_identity(page, 480, id);
    Ok(s.wrap::<SvgPoint>(SvgPointData::Translate(NodeHold::new(page, id)?), Some(key))?.value())
}

fn point_value<'s>(s: &Scope<'s>, item: Rc<PointItem>) -> JsResult<Value<'s>> {
    // The wrapper owns this Rc and its weak identity entry ends at GC. Each
    // realm therefore gets one wrapper per point for as long as it is held.
    let key = IdentityKey((1 << 59) | (1 << 56), Rc::as_ptr(&item) as u64);
    Ok(s.wrap::<SvgPoint>(SvgPointData::Item(item), Some(key))?.value())
}

fn point_argument(s: &Scope<'_>, value: Value<'_>) -> JsResult<Rc<PointItem>> {
    let point = s.unwrap::<SvgPoint>(value).ok_or_else(|| JsError::Type("Expected an SVGPoint.".into()))?;
    Ok(match &*point {
        SvgPointData::Item(item) => item.clone(),
        SvgPointData::Translate(_) => {
            let (x, y) = point.coordinates();
            PointItem::detached((x as f32, y as f32))
        }
    })
}

use pyro_dom::svg_point_list::{ListItem, ListStorage};

pub struct SvgListData<T: Copy> {
    lists: Rc<ListStorage<T>>,
    animated: bool,
}

impl<T: Copy> SvgListData<T> {
    fn writable(&self) -> JsResult<&Rc<ListStorage<T>>> {
        if self.animated {
            return Err(dom_err(pyro_dom::svg_point_list::read_only()));
        }
        Ok(&self.lists)
    }
}

trait SvgItemList<T: Copy>: JsClass<Data = SvgListData<T>> {
    fn wrap_item<'s>(s: &Scope<'s>, item: Rc<ListItem<T>>) -> JsResult<Value<'s>>;
    fn unwrap_item(s: &Scope<'_>, value: Value<'_>) -> JsResult<Rc<ListItem<T>>>;
}

pub struct SvgPointList;
impl JsClass for SvgPointList {
    const NAME: &'static str = "SVGPointList";
    type Data = SvgListData<(f32, f32)>;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        define_svg_list(def);
    }
}

impl SvgItemList<(f32, f32)> for SvgPointList {
    fn wrap_item<'s>(s: &Scope<'s>, item: Rc<PointItem>) -> JsResult<Value<'s>> {
        point_value(s, item)
    }
    fn unwrap_item(s: &Scope<'_>, value: Value<'_>) -> JsResult<Rc<PointItem>> {
        point_argument(s, value)
    }
}

pub struct SvgNumberList;
impl JsClass for SvgNumberList {
    const NAME: &'static str = "SVGNumberList";
    type Data = SvgListData<f32>;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        define_svg_list(def);
    }
}

impl SvgItemList<f32> for SvgNumberList {
    fn wrap_item<'s>(s: &Scope<'s>, item: Rc<NumberItem>) -> JsResult<Value<'s>> {
        number_value(s, item)
    }
    fn unwrap_item(s: &Scope<'_>, value: Value<'_>) -> JsResult<Rc<NumberItem>> {
        number_argument(s, value)
    }
}

fn define_svg_list<T: Copy + 'static, C: SvgItemList<T>>(def: &mut ClassDef<C>) {
    for name in ["numberOfItems", "length"] {
        def.getter(name, |s, l| Ok(s.number(l.lists.len(l.animated) as f64)));
    }
    def.legacy_platform_object();
    def.indexed_iterator();
    def.indexed(|s, l, i| l.lists.item(i as usize, l.animated).map(|v| C::wrap_item(s, v)).transpose());
    def.named_setter(|s, l, name, value| {
        let Some(i) = name.parse::<u32>().ok().filter(|i| *i != u32::MAX && i.to_string() == name) else {
            return Ok(false);
        };
        let item = C::unwrap_item(s, value)?;
        l.writable()?.insert(i as usize, &item, true).js()?;
        Ok(true)
    });
    def.method("getItem", 1, |s, l, a| {
        a.require(1, C::NAME)?;
        let i = svg_list_index(a.get(0))?;
        let item = l.lists.item(i, l.animated).ok_or_else(|| JsError::dom("IndexSizeError", "The index is outside the SVG list."))?;
        C::wrap_item(s, item)
    });
    def.method("clear", 0, |s, l, _| {
        l.writable()?.clear().js()?;
        Ok(s.undefined())
    });
    def.method("initialize", 1, |s, l, a| {
        a.require(1, C::NAME)?;
        let item = C::unwrap_item(s, a.get(0))?;
        C::wrap_item(s, l.writable()?.initialize(&item).js()?)
    });
    def.method("appendItem", 1, |s, l, a| {
        a.require(1, C::NAME)?;
        let item = C::unwrap_item(s, a.get(0))?;
        C::wrap_item(s, l.writable()?.insert(usize::MAX, &item, false).js()?)
    });
    def.method("insertItemBefore", 2, |s, l, a| {
        a.require(2, C::NAME)?;
        let item = C::unwrap_item(s, a.get(0))?;
        let i = svg_list_index(a.get(1))?;
        C::wrap_item(s, l.writable()?.insert(i, &item, false).js()?)
    });
    def.method("replaceItem", 2, |s, l, a| {
        a.require(2, C::NAME)?;
        let item = C::unwrap_item(s, a.get(0))?;
        let i = svg_list_index(a.get(1))?;
        C::wrap_item(s, l.writable()?.insert(i, &item, true).js()?)
    });
    def.method("removeItem", 1, |s, l, a| {
        a.require(1, C::NAME)?;
        let i = svg_list_index(a.get(0))?;
        C::wrap_item(s, l.writable()?.remove(i).js()?)
    });
}

pub fn point_list<'s>(s: &Scope<'s>, n: &NodeData, animated: bool) -> JsResult<Value<'s>> {
    let key = coll_identity(n.page(), if animated { 443 } else { 442 }, n.id);
    if let Some(obj) = s.lookup_identity(key) { return Ok(obj.value()); }
    let hold = NodeHold::new(n.page(), n.id)?;
    let write = Box::new(move |text: Option<&str>| {
        let mut dom = hold.page.dom.borrow_mut();
        match text {
            Some(text) => dom.set_attribute_local(hold.id, &pyro_dom::LocalName::from("points"), text),
            None => dom.remove_attribute_ns(hold.id, None, "points").map(|_| ()),
        }
    });
    let lists = n.page().dom.borrow_mut().svg_point_lists(n.id, write).js()?;
    // `animatedPoints` (and `animVal`) is the list under the SMIL animations in effect: the points as one
    // list of numbers, as an animation's values parse.
    let animated_hold = NodeHold::new(n.page(), n.id)?;
    lists.set_animation(Box::new(move |base: &[(f32, f32)]| {
        let contributions = attribute_contributions(&animated_hold.page, animated_hold.id, &["points"]);
        if contributions.is_empty() {
            return None;
        }
        let flat: Vec<f64> = base.iter().flat_map(|(x, y)| [f64::from(*x), f64::from(*y)]).collect();
        let folded = crate::smil::fold(&NumberSpace { pair: false }, &contributions, flat);
        Some(folded.chunks_exact(2).map(|p| (p[0] as f32, p[1] as f32)).collect::<Vec<(f32, f32)>>())
    }));
    Ok(s.wrap::<SvgPointList>(SvgListData { lists, animated }, Some(key))?.value())
}

pub struct SvgAnimatedNumberList;

impl JsClass for SvgAnimatedNumberList {
    const NAME: &'static str = "SVGAnimatedNumberList";
    type Data = Rc<NumberLists>;
    type Parent = NoParent;

    fn define(def: &mut ClassDef<Self>) {
        def.getter("baseVal", |s, lists| number_list_view(s, lists, false));
        def.getter("animVal", |s, lists| number_list_view(s, lists, true));
    }
}

fn number_list_view<'s>(s: &Scope<'s>, lists: &Rc<NumberLists>, animated: bool) -> JsResult<Value<'s>> {
    let key = IdentityKey((1 << 59) | (3 << 56) | u64::from(animated), Rc::as_ptr(lists) as u64);
    Ok(s.wrap::<SvgNumberList>(SvgListData { lists: lists.clone(), animated }, Some(key))?.value())
}

pub fn animated_number_list<'s>(s: &Scope<'s>, n: &NodeData, attr: &'static str) -> JsResult<Value<'s>> {
    // All four exposing element interfaces have exactly one number list attribute.
    let key = coll_identity(n.page(), 447, n.id);
    if let Some(obj) = s.lookup_identity(key) {
        return Ok(obj.value());
    }
    let hold = NodeHold::new(n.page(), n.id)?;
    let write = Box::new(move |text: &str| {
        hold.page.dom.borrow_mut().set_attribute_local(hold.id, &pyro_dom::LocalName::from(attr), text)
    });
    let lists = n.page().dom.borrow_mut().svg_number_lists(n.id, attr, write).js()?;
    // `animVal` is the list under the SMIL animations in effect.
    let animated = NodeHold::new(n.page(), n.id)?;
    lists.set_animation(Box::new(move |base: &[f32]| {
        let contributions = attribute_contributions(&animated.page, animated.id, &[attr]);
        if contributions.is_empty() {
            return None;
        }
        let base: Vec<f64> = base.iter().map(|v| f64::from(*v)).collect();
        Some(crate::smil::fold(&NumberSpace { pair: false }, &contributions, base).into_iter().map(|v| v as f32).collect::<Vec<f32>>())
    }));
    Ok(s.wrap::<SvgAnimatedNumberList>(lists, Some(key))?.value())
}

pub struct SvgLengthList;
impl JsClass for SvgLengthList {
    const NAME: &'static str = "SVGLengthList";
    type Data = SvgListData<(f64, u16)>;
    type Parent = NoParent;

    fn define(def: &mut ClassDef<Self>) {
        define_svg_list(def);
    }
}

impl SvgItemList<(f64, u16)> for SvgLengthList {
    fn wrap_item<'s>(s: &Scope<'s>, item: Rc<LengthItem>) -> JsResult<Value<'s>> {
        Ok(length_item(s, item)?.value())
    }

    fn unwrap_item(s: &Scope<'_>, value: Value<'_>) -> JsResult<Rc<LengthItem>> {
        let length = s.unwrap::<SvgLength>(value).ok_or_else(|| JsError::Type("Expected an SVGLength.".into()))?;
        Ok(match &*length {
            SvgLengthData::Item { item, .. } => item.clone(),
            SvgLengthData::Attribute { .. } => LengthItem::detached(length.get()),
        })
    }
}

pub struct SvgAnimatedLengthList;
impl JsClass for SvgAnimatedLengthList {
    const NAME: &'static str = "SVGAnimatedLengthList";
    type Data = Rc<LengthLists>;
    type Parent = NoParent;

    fn define(def: &mut ClassDef<Self>) {
        def.getter("baseVal", |s, lists| length_list_view(s, lists, false));
        def.getter("animVal", |s, lists| length_list_view(s, lists, true));
    }
}

fn length_list_view<'s>(s: &Scope<'s>, lists: &Rc<LengthLists>, animated: bool) -> JsResult<Value<'s>> {
    let key = IdentityKey((1 << 59) | (5 << 56) | u64::from(animated), Rc::as_ptr(lists) as u64);
    Ok(s.wrap::<SvgLengthList>(SvgListData { lists: lists.clone(), animated }, Some(key))?.value())
}

pub fn animated_length_list<'s>(s: &Scope<'s>, n: &NodeData, attr: &'static str) -> JsResult<Value<'s>> {
    let slot = match attr {
        "x" => 450,
        "y" => 451,
        "dx" => 452,
        "dy" => 453,
        _ => unreachable!("unexpected SVG text positioning length list"),
    };
    let key = coll_identity(n.page(), slot, n.id);
    if let Some(obj) = s.lookup_identity(key) {
        return Ok(obj.value());
    }
    let el = Rc::new(NodeHold::new(n.page(), n.id)?);
    let writer = el.clone();
    let write = Box::new(move |text: &str| {
        writer.page.dom.borrow_mut().set_attribute_local(writer.id, &pyro_dom::LocalName::from(attr), text)
    });
    let dir = if matches!(attr, "y" | "dy") { SvgLengthDir::Height } else { SvgLengthDir::Width };
    let factor = Box::new(move |kind| SvgLengthData::attribute_factor(&el, dir, kind));
    let lists = n.page().dom.borrow_mut().svg_length_lists(n.id, attr, write, factor).js()?;
    // `animVal` is the list under the SMIL animations in effect.
    let animated = NodeHold::new(n.page(), n.id)?;
    lists.set_animation(Box::new(move |base: &[(f64, u16)]| {
        let contributions = attribute_contributions(&animated.page, animated.id, &[attr]);
        if contributions.is_empty() {
            return None;
        }
        let factor = |kind: u16| SvgLengthData::attribute_factor(&animated, dir, kind);
        let space = LengthSpace { factor: &factor };
        Some(crate::smil::fold(&space, &contributions, base.to_vec()))
    }));
    Ok(s.wrap::<SvgAnimatedLengthList>(lists, Some(key))?.value())
}
