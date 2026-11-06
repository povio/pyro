//! NodeList, HTMLCollection, DOMTokenList, NamedNodeMap/Attr and
//! DOMStringMap. Live collections cache their member list against
//! `Dom::version()`; a later version keeps it when the change journal shows
//! nothing that can change its members (a change inside another subtree, a
//! text write under a child), so indexed loops stay O(1) per access while
//! the page mutates elsewhere. Member lists are also shared per page by
//! (root, kind): a second `getElementsByClassName('x')` reuses the first's.

use std::collections::HashMap;

use std::cell::RefCell;
use std::rc::Rc;

use lp_dom::{attr_name_hash, CollectionWatch, Dom, NodeId};
use lp_js::{ClassDef, IdentityKey, JsClass, JsError, JsResult, NoParent, Object, Scope, Value};

use crate::node::{wrap_node, NodeData};
use crate::page::PageDom;
use crate::util::*;

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
        }
    }
    fn len(&self) -> usize {
        match &self.source {
            NodeListSource::Static(v) => v.as_ref().map_or(0, |h| h.ids.len()),
            NodeListSource::Children(h) => h.page.dom.borrow().child_count(h.id).unwrap_or(0) as usize,
            NodeListSource::FormNamed(h, _) => self.items(&h.page.dom.borrow()).len(),
        }
    }
    fn get(&self, i: usize) -> Option<(Rc<PageDom>, NodeId)> {
        match &self.source {
            NodeListSource::Static(v) => v.as_ref().and_then(|h| Some((h.page.clone(), *h.ids.get(i)?))),
            NodeListSource::Children(h) => {
                let dom = h.page.dom.borrow();
                {
                    let c = self.cache.borrow();
                    if c.0 == dom.version() {
                        return c.1.get(i).map(|&id| (h.page.clone(), id));
                    }
                }
                self.items(&dom);
                self.cache.borrow().1.get(i).map(|&id| (h.page.clone(), id))
            }
            NodeListSource::FormNamed(h, _) => {
                let items = self.items(&h.page.dom.borrow());
                items.get(i).map(|&id| (h.page.clone(), id))
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
        def.pure_getter_typed("length", lp_js::GetterResult::Number, |s, l| Ok(s.number(l.len() as f64)));
        def.method("item", 1, |s, l, a| {
            let i = to_u32(a.get(0))? as usize;
            match l.get(i) {
                Some((p, id)) => Ok(wrap_node(s, &p, id)?.value()),
                None => Ok(s.null()),
            }
        });
        def.legacy_platform_object();
        def.indexed(|s, l, i| match l.get(i as usize) {
            Some((p, id)) => Ok(Some(wrap_node(s, &p, id)?.value())),
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
                .is_some_and(|d| d.quirks == lp_dom::QuirksMode::Quirks);
            let same = |a: &str, b: &str| if quirks { a.eq_ignore_ascii_case(b) } else { a == b };
            names.split_ascii_whitespace().next().is_some()
                && e.attr("class").is_some_and(|c| {
                    names.split_ascii_whitespace().all(|w| c.split_ascii_whitespace().any(|h| same(h, w)))
                })
        }
        CollectionKind::Name(n) => e.attr("name") == Some(n.as_str()),
        CollectionKind::WindowNamed(n) => {
            e.id() == Some(n.as_str())
                || (html && matches!(local, "embed" | "form" | "img" | "object") && e.attr("name") == Some(n.as_str()))
        }
        CollectionKind::DocumentNamed(n) => {
            (html && matches!(local, "embed" | "form" | "iframe" | "img" | "object") && e.attr("name") == Some(n.as_str()))
                || (html && local == "img" && e.id() == Some(n.as_str()) && e.attr("name").is_some_and(|v| !v.is_empty()))
        }
        CollectionKind::Links => html && matches!(local, "a" | "area") && e.attr("href").is_some(),
        CollectionKind::Anchors => html && local == "a" && e.attr("name").is_some(),
        CollectionKind::Empty => false,
        CollectionKind::FormElements => {
            html && (matches!(local, "button" | "fieldset" | "input" | "object" | "output" | "select" | "textarea")
                || e.flags & lp_dom::node::element_flags::FORM_ASSOCIATED != 0)
                && !(local == "input" && e.attr("type").is_some_and(|t| t.eq_ignore_ascii_case("image")))
                && dom.form_owner(id).ok().flatten() == Some(root)
        }
        _ => false,
    }
}

/// Whether the element's qualified name (`prefix:local`, else `local`) is
/// `wanted`, compared case-sensitively.
fn qualified_name_is(e: &lp_dom::node::ElementData, wanted: &str) -> bool {
    let local = &*e.name.local;
    match e.name.prefix.as_deref() {
        Some(prefix) if !prefix.is_empty() => {
            wanted.strip_prefix(prefix).and_then(|rest| rest.strip_prefix(':')).is_some_and(|l| l == local)
        }
        _ => wanted == local,
    }
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
        CollectionKind::FormElements => {
            // Listed elements anywhere in the form's tree (form attribute
            // associations included), in tree order.
            let Ok(tree_root) = dom.root_node(root, false) else { return Vec::new() };
            dom.descendants(tree_root)
                .map(|d| d.skip(1).filter(|&n| matches_kind(dom, root, n, kind)).collect())
                .unwrap_or_default()
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
        *c = (version, shared_members(&self.root.page, dom, self.root.id, &self.kind));
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
        def.pure_getter_typed("length", lp_js::GetterResult::Number, |s, c| {
            Ok(s.number(c.with_items(|v| v.len()) as f64))
        });
        def.method("item", 1, |s, c, a| {
            let i = to_u32(a.get(0))? as usize;
            let id = c.with_items(|v| v.get(i).copied());
            match id {
                Some(id) => Ok(wrap_node(s, &c.root.page, id)?.value()),
                None => Ok(s.null()),
            }
        });
        def.method("namedItem", 1, |s, c, a| {
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

pub struct HtmlOptionsCollection;
impl JsClass for HtmlOptionsCollection {
    const NAME: &'static str = "HTMLOptionsCollection";
    type Data = CollectionData;
    type Parent = HtmlCollection;
    fn define(def: &mut ClassDef<Self>) {
        def.accessor(
            "selectedIndex",
            |s, c| Ok(s.number(c.root.page.dom.borrow().selected_index(c.root.id).js()? as f64)),
            |_s, c, v| {
                let arg0 = to_i32(v)?;
                c.root.page.dom.borrow_mut().set_selected_index(c.root.id, arg0).js()
            },
        );
        def.method("add", 1, |s, c, a| {
            let el = crate::node::node_arg(s, a.get(0), "HTMLOptionsCollection.add")?;
            let before = a.get(1);
            let mut dom = c.root.page.dom.borrow_mut();
            let reference = if let Some(n) = s.unwrap::<crate::node::Node>(before) {
                Some(n.id)
            } else if before.is_number() {
                let opts = dom.select_options(c.root.id).js()?;
                opts.get(to_u32(before)? as usize).copied()
            } else {
                None
            };
            let parent = match reference {
                Some(r) => dom.parent(r).js()?.unwrap_or(c.root.id),
                None => c.root.id,
            };
            dom.insert_before(parent, el, reference).js()?;
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
                    JsError::Type("Failed to set an indexed property on 'HTMLOptionsCollection': not an HTMLOptionElement.".into())
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
        _ => return None,
    })
}

/// Live collection over `root` (cached wrapper per element for fixed kinds).
pub fn collection<'s>(s: &Scope<'s>, page: &PageDom, root: NodeId, kind: CollectionKind) -> JsResult<Object<'s>> {
    let key = kind_slot(&kind).map(|k| coll_identity(page, k, root));
    if let Some(k) = key {
        if let Some(o) = s.lookup_identity(k) {
            return Ok(o);
        }
    }
    let page = page_rc(s)?;
    let options = kind == CollectionKind::Options;
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
                .is_some_and(|d| d.kind == lp_dom::DocumentKind::Html);
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
    fn tokens_in<'d>(&self, dom: &'d lp_dom::Dom) -> Vec<&'d str> {
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
            dom.get_attribute(self.el.id, self.attr).ok().flatten().unwrap_or("").split_ascii_whitespace().for_each(add);
        }
        out
    }
    /// Token `i` of the set as a string: a `class` token is the element's
    /// atom, so its realm's one string for it.
    fn token<'s>(&self, s: &Scope<'s>, dom: &lp_dom::Dom, i: usize) -> Option<Value<'s>> {
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
        Ok(s.string(dom.get_attribute(self.el.id, self.attr).js()?.unwrap_or("")))
    }
    fn contains(&self, token: &str) -> bool {
        let dom = self.el.page.dom.borrow();
        if self.attr == "class" {
            return dom.element(self.el.id).is_ok_and(|e| e.classes().iter().any(|c| &**c == token));
        }
        let value = dom.get_attribute(self.el.id, self.attr).ok().flatten().unwrap_or("");
        value.split_ascii_whitespace().any(|t| t == token)
    }
    /// The "update steps" with `tokens` serialized: an absent attribute
    /// stays absent for an empty set.
    fn write(&self, empty: bool, value: &str) -> JsResult<()> {
        let mut dom = self.el.page.dom.borrow_mut();
        if empty && !dom.has_attribute(self.el.id, self.attr).js()? {
            return Ok(());
        }
        dom.set_attribute(self.el.id, self.attr, value).js()
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
        def.pure_getter_typed("length", lp_js::GetterResult::Number, |s, t| {
            Ok(s.number(t.tokens_in(&t.el.page.dom.borrow()).len() as f64))
        });
        def.accessor(
            "value",
            |s, t| t.value(s),
            |_s, t, v| with_str(v, |value| t.el.page.dom.borrow_mut().set_attribute(t.el.id, t.attr, value).js()),
        );
        def.method("toString", 0, |s, t, _a| t.value(s));
        def.method("item", 1, |s, t, a| {
            let i = to_u32(a.get(0))? as usize;
            Ok(t.token(s, &t.el.page.dom.borrow(), i).unwrap_or_else(|| s.null()))
        });
        def.legacy_platform_object();
        def.indexed(|s, t, i| Ok(t.token(s, &t.el.page.dom.borrow(), i as usize)));

        def.method("contains", 1, |s, t, a| {
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
        def.method("supports", 1, |s, t, a| {
            let tok = dom_string(a.get(0))?;
            let supported: &[&str] = match t.attr {
                "rel" => &[
                    "alternate", "canonical", "author", "bookmark", "dns-prefetch", "expect", "external", "help",
                    "icon", "manifest", "modulepreload", "license", "next", "nofollow", "noopener", "noreferrer",
                    "opener", "pingback", "preconnect", "prefetch", "preload", "prev", "privacy-policy", "search",
                    "stylesheet", "tag", "terms-of-service",
                ],
                "sandbox" => &[
                    "allow-downloads", "allow-forms", "allow-modals", "allow-orientation-lock",
                    "allow-pointer-lock", "allow-popups", "allow-popups-to-escape-sandbox", "allow-presentation",
                    "allow-same-origin", "allow-scripts", "allow-top-navigation",
                    "allow-top-navigation-by-user-activation", "allow-top-navigation-to-custom-protocols",
                ],
                "blocking" => &["render"],
                _ => {
                    return Err(JsError::Type(format!(
                        "DOMTokenList has no supported tokens for the '{}' attribute.",
                        t.attr
                    )))
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
    pub(crate) fn of(a: &lp_dom::Attr) -> AttrName {
        AttrName { ns: a.name.ns.to_string(), local: a.name.local.to_string() }
    }
    pub(crate) fn ns(&self) -> Option<&str> {
        (!self.ns.is_empty()).then_some(self.ns.as_str())
    }
    /// Index of this attribute in `attrs`.
    pub(crate) fn find(&self, attrs: &[lp_dom::Attr]) -> Option<usize> {
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
        def.method("item", 1, |s, m, a| {
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
                    .is_some_and(|d| d.kind == lp_dom::DocumentKind::Html);
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
    name: lp_dom::QualName,
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
    let replaced = page.dom.borrow_mut().set_attribute_node(el, a.id).js()?;
    match replaced {
        Some(old) if old == a.id => Ok(attr),
        Some(old) => Ok(wrap_node(s, page, old)?.value()),
        None => Ok(s.null()),
    }
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
/// from `Node`; the attribute's state lives in `lp_dom` (`Dom::attribute_node`).
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
            |_s, a, v| {
                let value = dom_string(v)?;
                a.page().dom.borrow_mut().set_attr_node_value(a.id, &value).js()
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
    let mut out = String::from("data-");
    let b = name.as_bytes();
    for (i, &c) in b.iter().enumerate() {
        if c == b'-' && b.get(i + 1).is_some_and(|n| n.is_ascii_lowercase()) {
            return None; // SyntaxError per spec for setter; getter: absent
        }
        if c.is_ascii_uppercase() {
            out.push('-');
            out.push(c.to_ascii_lowercase() as char);
        } else {
            out.push(c as char);
        }
    }
    Some(out)
}

fn data_attr_to_camel(attr: &str) -> Option<String> {
    let rest = attr.strip_prefix("data-")?;
    let mut out = String::new();
    let mut up = false;
    for c in rest.chars() {
        if c == '-' {
            up = true;
            continue;
        }
        if up && c.is_ascii_lowercase() {
            out.push(c.to_ascii_uppercase());
        } else {
            if up {
                out.push('-');
            }
            out.push(c);
        }
        up = false;
    }
    if up {
        out.push('-');
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

pub(crate) fn register(engine: &lp_js::Engine) {
    engine.register::<NodeList>();
    engine.register::<RadioNodeList>();
    engine.register::<HtmlCollection>();
    engine.register::<HtmlOptionsCollection>();
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
    }
}
