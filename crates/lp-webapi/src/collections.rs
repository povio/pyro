//! NodeList, HTMLCollection, DOMTokenList, NamedNodeMap/Attr and
//! DOMStringMap. Live collections cache their member list against
//! `Dom::version()` so indexed loops are O(1) per access between mutations.

use std::cell::RefCell;
use std::rc::Rc;

use lp_dom::{Dom, NodeId, NodeType};
use lp_js::{ClassDef, IdentityKey, JsClass, JsError, JsResult, NoParent, Object, Scope, Value};

use crate::node::wrap_node;
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

fn page_rc(s: &Scope<'_>) -> JsResult<Rc<PageDom>> {
    crate::target::page_of(s)
}

// ----- NodeList -------------------------------------------------------------

pub enum NodeListSource {
    /// Static snapshot; every member is held.
    Static(Vec<NodeHold>),
    /// Live `childNodes` of the held node.
    Children(NodeHold),
}

pub struct NodeListData {
    source: NodeListSource,
    cache: RefCell<(u64, Vec<NodeId>)>,
}

impl NodeListData {
    fn items(&self, dom: &Dom) -> Vec<NodeId> {
        match &self.source {
            NodeListSource::Static(v) => v.iter().map(|h| h.id).collect(),
            NodeListSource::Children(h) => {
                let mut c = self.cache.borrow_mut();
                if c.0 != dom.version() || c.1.is_empty() {
                    c.1 = dom.children(h.id).map(|it| it.collect()).unwrap_or_default();
                    c.0 = dom.version();
                }
                c.1.clone()
            }
        }
    }
    fn len(&self) -> usize {
        match &self.source {
            NodeListSource::Static(v) => v.len(),
            NodeListSource::Children(h) => h.page.dom.borrow().child_count(h.id).unwrap_or(0) as usize,
        }
    }
    fn get(&self, i: usize) -> Option<(Rc<PageDom>, NodeId)> {
        match &self.source {
            NodeListSource::Static(v) => v.get(i).map(|h| (h.page.clone(), h.id)),
            NodeListSource::Children(h) => {
                let dom = h.page.dom.borrow();
                let items = {
                    let c = self.cache.borrow();
                    if c.0 == dom.version() && !c.1.is_empty() {
                        return c.1.get(i).map(|&id| (h.page.clone(), id));
                    }
                    drop(c);
                    self.items(&dom)
                };
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
        def.getter("length", |s, l| Ok(s.number(l.len() as f64)));
        def.method("item", 1, |s, l, a| {
            let i = to_u32(a.get(0))? as usize;
            match l.get(i) {
                Some((p, id)) => Ok(wrap_node(s, &p, id)?.value()),
                None => Ok(s.null()),
            }
        });
        def.indexed(|s, l, i| match l.get(i as usize) {
            Some((p, id)) => Ok(Some(wrap_node(s, &p, id)?.value())),
            None => Ok(None),
        });
        def.indexed_iterable();
    }
}

pub fn static_node_list<'s>(s: &Scope<'s>, page: &PageDom, ids: Vec<NodeId>) -> JsResult<Object<'s>> {
    let page = page_rc(s).inspect(|p| {
        debug_assert_eq!(p.id(), page.id());
    })?;
    let mut holds = Vec::with_capacity(ids.len());
    for id in ids {
        holds.push(NodeHold::new(&page, id)?);
    }
    if holds.is_empty() {
        // Empty static lists need no page reference; use an empty children view.
        return s.wrap::<NodeList>(
            NodeListData { source: NodeListSource::Static(Vec::new()), cache: RefCell::new((0, Vec::new())) },
            None,
        );
    }
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CollectionKind {
    Children,
    TagName(String),
    ClassName(String),
    Name(String),
    Links,
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
    cache: RefCell<(u64, Vec<NodeId>)>,
}

fn matches_kind(dom: &Dom, root: NodeId, id: NodeId, kind: &CollectionKind) -> bool {
    let Ok(e) = dom.element(id) else { return false };
    let html = e.is_html();
    let local = &*e.name.local;
    match kind {
        CollectionKind::TagName(t) => {
            t == "*" || (html && local.eq_ignore_ascii_case(t)) || (!html && local == t.as_str())
        }
        CollectionKind::ClassName(names) => {
            let want: Vec<&str> = names.split_ascii_whitespace().collect();
            !want.is_empty()
                && e.attr("class").is_some_and(|c| {
                    let have: Vec<&str> = c.split_ascii_whitespace().collect();
                    want.iter().all(|w| have.contains(w))
                })
        }
        CollectionKind::Name(n) => e.attr("name") == Some(n.as_str()),
        CollectionKind::Links => html && matches!(local, "a" | "area") && e.attr("href").is_some(),
        CollectionKind::FormElements => {
            html && matches!(local, "button" | "fieldset" | "input" | "object" | "output" | "select" | "textarea")
                && !(local == "input" && e.attr("type").is_some_and(|t| t.eq_ignore_ascii_case("image")))
                && dom.form_owner(id).ok().flatten() == Some(root)
        }
        _ => false,
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

impl CollectionData {
    fn items(&self) -> Vec<NodeId> {
        let dom = self.root.page.dom.borrow();
        let mut c = self.cache.borrow_mut();
        if c.0 != dom.version() {
            c.1 = compute(&dom, self.root.id, &self.kind);
            c.0 = dom.version();
        }
        c.1.clone()
    }
    fn with_items<R>(&self, f: impl FnOnce(&[NodeId]) -> R) -> R {
        let dom = self.root.page.dom.borrow();
        let mut c = self.cache.borrow_mut();
        if c.0 != dom.version() {
            c.1 = compute(&dom, self.root.id, &self.kind);
            c.0 = dom.version();
        }
        f(&c.1)
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
}

pub struct HtmlCollection;
impl JsClass for HtmlCollection {
    const NAME: &'static str = "HTMLCollection";
    type Data = CollectionData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.getter("length", |s, c| Ok(s.number(c.with_items(|v| v.len()) as f64)));
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
        def.indexed_iterable();
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
            |_s, c, v| c.root.page.dom.borrow_mut().set_selected_index(c.root.id, to_i32(v)?).js(),
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
    let data = CollectionData { root: NodeHold::new(&page, root)?, kind, cache: RefCell::new((u64::MAX, Vec::new())) };
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
    fn tokens(&self) -> Vec<String> {
        let dom = self.el.page.dom.borrow();
        let v = dom.get_attribute(self.el.id, self.attr).ok().flatten().unwrap_or("");
        let mut out: Vec<String> = Vec::new();
        for t in v.split_ascii_whitespace() {
            if !out.iter().any(|x| x == t) {
                out.push(t.to_owned());
            }
        }
        out
    }
    fn write(&self, tokens: &[String]) -> JsResult<()> {
        self.el.page.dom.borrow_mut().set_attribute(self.el.id, self.attr, &tokens.join(" ")).js()
    }
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
        def.getter("length", |s, t| Ok(s.number(t.tokens().len() as f64)));
        def.accessor(
            "value",
            |s, t| {
                let dom = t.el.page.dom.borrow();
                Ok(s.string(dom.get_attribute(t.el.id, t.attr).js()?.unwrap_or("")))
            },
            |_s, t, v| t.el.page.dom.borrow_mut().set_attribute(t.el.id, t.attr, &dom_string(v)?).js(),
        );
        def.method("toString", 0, |s, t, _a| {
            let dom = t.el.page.dom.borrow();
            Ok(s.string(dom.get_attribute(t.el.id, t.attr).js()?.unwrap_or("")))
        });
        def.method("item", 1, |s, t, a| {
            let i = to_u32(a.get(0))? as usize;
            Ok(t.tokens().get(i).map_or(s.null(), |x| s.string(x)))
        });
        def.indexed(|s, t, i| Ok(t.tokens().get(i as usize).map(|x| s.string(x))));
        def.method("contains", 1, |s, t, a| {
            let tok = dom_string(a.get(0))?;
            Ok(s.boolean(t.tokens().contains(&tok)))
        });
        def.method("add", 0, |s, t, a| {
            let mut toks = t.tokens();
            for v in a.iter() {
                let tok = dom_string(v)?;
                check_token(&tok)?;
                if !toks.contains(&tok) {
                    toks.push(tok);
                }
            }
            t.write(&toks)?;
            Ok(s.undefined())
        });
        def.method("remove", 0, |s, t, a| {
            let mut toks = t.tokens();
            for v in a.iter() {
                let tok = dom_string(v)?;
                check_token(&tok)?;
                toks.retain(|x| *x != tok);
            }
            t.write(&toks)?;
            Ok(s.undefined())
        });
        def.method("toggle", 1, |s, t, a| {
            let tok = arg_string(&a, 0, "DOMTokenList.toggle")?;
            check_token(&tok)?;
            let force = a.opt(1).filter(|v| !v.is_undefined()).map(|v| v.to_bool());
            let mut toks = t.tokens();
            let present = toks.contains(&tok);
            let result = match (present, force) {
                (true, None | Some(false)) => {
                    toks.retain(|x| *x != tok);
                    t.write(&toks)?;
                    false
                }
                (false, None | Some(true)) => {
                    toks.push(tok);
                    t.write(&toks)?;
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
            check_token(&old)?;
            check_token(&new)?;
            let mut toks = t.tokens();
            let Some(pos) = toks.iter().position(|x| *x == old) else { return Ok(s.boolean(false)) };
            if toks.contains(&new) {
                toks.remove(pos);
            } else {
                toks[pos] = new;
            }
            t.write(&toks)?;
            Ok(s.boolean(true))
        });
        def.method("supports", 1, |s, _t, _a| Ok(s.boolean(true)));
        def.indexed_iterable();
    }
}

pub fn token_list<'s>(s: &Scope<'s>, page: &PageDom, id: NodeId, attr: &'static str) -> JsResult<Object<'s>> {
    let slot = match attr {
        "class" => 12,
        "part" => 13,
        "rel" => 14,
        "sandbox" => 15,
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

pub struct AttrData {
    el: NodeHold,
    qualified: String,
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
            let exists = m.el.page.dom.borrow().has_attribute(m.el.id, &name).js()?;
            if !exists {
                return Ok(s.null());
            }
            let q = {
                let dom = m.el.page.dom.borrow();
                let lower = if dom.element(m.el.id).js()?.is_html() { name.to_ascii_lowercase() } else { name };
                lower
            };
            make_attr(s, &m.el.page, m.el.id, q)
        });
        def.method("getNamedItemNS", 2, |s, m, a| {
            let ns = opt_dom_string(a.get(0))?;
            let local = dom_string(a.get(1))?;
            let dom = m.el.page.dom.borrow();
            let found = dom
                .attributes(m.el.id)
                .js()?
                .iter()
                .find(|x| &*x.name.ns == ns.as_deref().unwrap_or("") && *x.name.local == local)
                .map(|x| x.qualified_name());
            drop(dom);
            match found {
                Some(q) => make_attr(s, &m.el.page, m.el.id, q),
                None => Ok(s.null()),
            }
        });
        def.method("removeNamedItem", 1, |s, m, a| {
            let name = dom_string(a.get(0))?;
            let removed = m.el.page.dom.borrow_mut().remove_attribute(m.el.id, &name).js()?;
            if !removed {
                return Err(JsError::dom("NotFoundError", format!("No item with name '{name}' was found.")));
            }
            Ok(s.undefined())
        });
        def.named(false, |s, m, name| {
            let exists = m.el.page.dom.borrow().has_attribute(m.el.id, name).js()?;
            if exists {
                Ok(Some(make_attr(s, &m.el.page, m.el.id, name.to_owned())?))
            } else {
                Ok(None)
            }
        });
        def.indexed_iterable();
    }
}

fn attr_at<'s>(s: &Scope<'s>, m: &NamedNodeMapData, i: usize) -> JsResult<Option<Value<'s>>> {
    let q = m.el.page.dom.borrow().attributes(m.el.id).js()?.get(i).map(|a| a.qualified_name());
    match q {
        Some(q) => Ok(Some(make_attr(s, &m.el.page, m.el.id, q)?)),
        None => Ok(None),
    }
}

fn make_attr<'s>(s: &Scope<'s>, page: &Rc<PageDom>, el: NodeId, qualified: String) -> JsResult<Value<'s>> {
    Ok(s.wrap::<Attr>(AttrData { el: NodeHold::new(page, el)?, qualified }, None)?.value())
}

pub struct Attr;
impl JsClass for Attr {
    const NAME: &'static str = "Attr";
    type Data = AttrData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.getter("name", |s, a| Ok(s.string(&a.qualified)));
        def.getter("nodeName", |s, a| Ok(s.string(&a.qualified)));
        def.getter("nodeType", |s, _a| Ok(s.number(NodeType::Attribute as u16 as f64)));
        def.getter("localName", |s, a| Ok(s.string(a.qualified.rsplit(':').next().unwrap_or(""))));
        def.getter("prefix", |s, a| Ok(a.qualified.split_once(':').map_or(s.null(), |(p, _)| s.string(p))));
        def.getter("namespaceURI", |s, a| {
            let dom = a.el.page.dom.borrow();
            let ns = dom
                .attributes(a.el.id)
                .js()?
                .iter()
                .find(|x| x.qualified_name() == a.qualified)
                .map(|x| x.name.ns.to_string())
                .unwrap_or_default();
            Ok(if ns.is_empty() { s.null() } else { s.string(&ns) })
        });
        def.accessor(
            "value",
            |s, a| Ok(s.string(a.el.page.dom.borrow().get_attribute(a.el.id, &a.qualified).js()?.unwrap_or(""))),
            |_s, a, v| a.el.page.dom.borrow_mut().set_attribute(a.el.id, &a.qualified, &dom_string(v)?).js(),
        );
        def.accessor(
            "nodeValue",
            |s, a| Ok(s.string(a.el.page.dom.borrow().get_attribute(a.el.id, &a.qualified).js()?.unwrap_or(""))),
            |_s, a, v| a.el.page.dom.borrow_mut().set_attribute(a.el.id, &a.qualified, &dom_string(v)?).js(),
        );
        def.getter("textContent", |s, a| {
            Ok(s.string(a.el.page.dom.borrow().get_attribute(a.el.id, &a.qualified).js()?.unwrap_or("")))
        });
        def.getter("ownerElement", |s, a| Ok(wrap_node(s, &a.el.page, a.el.id)?.value()));
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
            m.el.page.dom.borrow_mut().set_attribute(m.el.id, &attr, &dom_string(v)?).js()?;
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
