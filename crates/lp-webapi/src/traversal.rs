//! NodeFilter, TreeWalker and NodeIterator (DOM §6).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use lp_dom::NodeId;
use lp_js::{Args, ClassDef, JsClass, JsError, JsResult, NoParent, Object, Scope, Value};

use crate::collections::NodeHold;
use crate::node::{node_arg, wrap_node, NodeData};
use crate::page::PageDom;
use crate::util::*;

pub const FILTER_ACCEPT: u16 = 1;
pub const FILTER_REJECT: u16 = 2;
pub const FILTER_SKIP: u16 = 3;

pub struct NodeFilter;
impl JsClass for NodeFilter {
    const NAME: &'static str = "NodeFilter";
    type Data = ();
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        // A callback interface: `NodeFilter` is neither constructible nor has
        // a `prototype`; it only carries the constants.
        def.legacy_callback_interface();
        for (n, v) in [
            ("FILTER_ACCEPT", 1.0),
            ("FILTER_REJECT", 2.0),
            ("FILTER_SKIP", 3.0),
            ("SHOW_ALL", 4294967295.0),
            ("SHOW_ELEMENT", 0x1 as f64),
            ("SHOW_ATTRIBUTE", 0x2 as f64),
            ("SHOW_TEXT", 0x4 as f64),
            ("SHOW_CDATA_SECTION", 0x8 as f64),
            ("SHOW_ENTITY_REFERENCE", 0x10 as f64),
            ("SHOW_ENTITY", 0x20 as f64),
            ("SHOW_PROCESSING_INSTRUCTION", 0x40 as f64),
            ("SHOW_COMMENT", 0x80 as f64),
            ("SHOW_DOCUMENT", 0x100 as f64),
            ("SHOW_DOCUMENT_TYPE", 0x200 as f64),
            ("SHOW_DOCUMENT_FRAGMENT", 0x400 as f64),
            ("SHOW_NOTATION", 0x800 as f64),
        ] {
            def.constant(n, v);
        }
    }
}

pub struct WalkerData {
    root: NodeHold,
    current: RefCell<NodeHold>,
    what_to_show: u32,
    /// The filter, a hidden property of the walker's wrapper (None: no filter).
    filter: Option<WrapperRef>,
    active: Cell<bool>,
    /// NodeIterator: pointer-before-reference flag.
    before_ref: Cell<bool>,
    /// NodeIterator: the reference the DOM moves when nodes are removed.
    live: Option<std::rc::Rc<lp_dom::live_iterator::LiveIterator>>,
}

impl Drop for WalkerData {
    fn drop(&mut self) {
        if let Some(live) = &self.live {
            self.root.page.release_node(live.reference());
        }
    }
}

impl WalkerData {
    fn page(&self) -> &Rc<PageDom> {
        &self.root.page
    }
    /// "filter" a node: whatToShow mask then the JS filter.
    fn accept(&self, s: &Scope<'_>, node: NodeId) -> JsResult<u16> {
        if self.active.get() {
            return Err(JsError::dom("InvalidStateError", "The filter is already running."));
        }
        let ty = self.page().dom.borrow().node_type(node).js()? as u32;
        if self.what_to_show & (1 << (ty - 1)) == 0 {
            return Ok(FILTER_SKIP);
        }
        let Some(f) = &self.filter else { return Ok(FILTER_ACCEPT) };
        let fv = f.hidden(s, FILTER)?.unwrap_or(s.null());
        let Some(fo) = fv.as_object() else { return Ok(FILTER_ACCEPT) };
        let node_v = wrap_node(s, self.page(), node)?.value();
        // "Set the active flag ... if this throws an exception, then unset
        // the active flag and rethrow the exception."
        self.active.set(true);
        let result = call_filter(s, fo, node_v);
        self.active.set(false);
        result
    }

    /// NodeIterator: filters `node`, the position a traversal has reached,
    /// and returns the result with where that position is afterwards.
    fn filter_at(&self, s: &Scope<'_>, node: NodeId, before: bool) -> JsResult<(u16, (NodeId, bool))> {
        let Some(live) = &self.live else { return Ok((self.accept(s, node)?, (node, before))) };
        self.page().dom.borrow_mut().set_iterator_candidate(live, Some((node, before))).js()?;
        let result = self.accept(s, node);
        let at = live.candidate().unwrap_or((node, before));
        self.page().dom.borrow_mut().set_iterator_candidate(live, None).js()?;
        Ok((result?, at))
    }
    fn cur(&self) -> NodeId {
        match &self.live {
            Some(live) => live.reference(),
            None => self.current.borrow().id,
        }
    }
    /// NodeIterator: the pointer-before-reference flag, as the DOM keeps it.
    fn before(&self) -> bool {
        match &self.live {
            Some(live) => live.before(),
            None => self.before_ref.get(),
        }
    }
    fn set_cur(&self, id: NodeId) -> JsResult<()> {
        if let Some(live) = &self.live {
            self.page().dom.borrow_mut().set_iterator_reference(live, id, self.before_ref.get()).js()?;
            return Ok(());
        }
        *self.current.borrow_mut() = NodeHold::new(self.page(), id)?;
        Ok(())
    }
    fn set_before(&self, before: bool) -> JsResult<()> {
        self.before_ref.set(before);
        if let Some(live) = &self.live {
            let id = live.reference();
            self.page().dom.borrow_mut().set_iterator_reference(live, id, before).js()?;
        }
        Ok(())
    }
}

const FILTER: &str = "__lpTraversalFilter";

/// Web IDL "call a user object's operation" for the `NodeFilter` callback
/// interface: `filter` is a function, or an object whose `acceptNode` is read
/// on every call. It runs in the filter's associated realm (the one it was
/// created in), so a TypeError of the conversion is that realm's.
fn call_filter(s: &Scope<'_>, filter: Object<'_>, node: Value<'_>) -> JsResult<u16> {
    let realm = s.object_realm(filter).filter(|r| r.id() != s.realm_id());
    let Some(realm) = realm else { return run_filter(s, filter, node) };
    let cs = realm.scope()?;
    // A callback of a document that left its browsing context (a removed
    // iframe) is no longer runnable: it throws without being called.
    let runnable = crate::target::page_of(&cs)
        .is_ok_and(|p| !p.is_disposed() && p.hooks().is_none_or(|h| h.focus_document_is_live()));
    if !runnable {
        return Err(JsError::Type(
            "Failed to execute 'acceptNode' on 'NodeFilter': The provided callback is no longer runnable.".into(),
        ));
    }
    let Some(filter) = cs.rehome(filter.value()).as_object() else {
        return Ok(FILTER_ACCEPT);
    };
    let node = cs.rehome(node);
    run_filter(&cs, filter, node)
}

/// A TypeError of `s`'s realm, whichever realm the native caller runs in.
fn type_error_of(s: &Scope<'_>, message: &str) -> JsError {
    JsError::thrown(lp_js::Persistent::new(s, JsError::Type(message.into()).to_value(s)))
}

/// Whether `o` is a revoked Proxy. Calling one throws a TypeError of the
/// realm the Proxy was made in, where Web IDL wants the callback's realm.
/// `Array.isArray` is the one operation that tells without running a trap.
fn is_revoked_proxy(s: &Scope<'_>, o: Object<'_>) -> bool {
    if !o.value().has_proxy_target().unwrap_or(false) {
        return false;
    }
    let is_array =
        s.global().get_atom("Array").ok().and_then(|a| a.as_object()).and_then(|a| a.get_atom("isArray").ok());
    is_array
        .and_then(|f| f.as_object())
        .filter(|f| f.is_function())
        .is_some_and(|f| f.call(None, &[o.value()]).is_err())
}

fn run_filter(s: &Scope<'_>, filter: Object<'_>, node: Value<'_>) -> JsResult<u16> {
    let revoked = |o: Object<'_>| {
        is_revoked_proxy(s, o).then(|| type_error_of(s, "NodeFilter callback is a revoked Proxy")).map_or(Ok(()), Err)
    };
    let result = if filter.is_function() {
        revoked(filter)?;
        filter.call(None, &[node])?
    } else {
        // Get(O, "acceptNode"): an exception of the getter is rethrown as is.
        let method = match filter.get_atom("acceptNode") {
            Ok(m) => m,
            Err(JsError::Thrown(e)) => return Err(JsError::Thrown(e)),
            Err(e) => return Err(type_error_of(s, &e.to_string())),
        };
        match method.as_object().filter(|m| m.is_function()) {
            Some(m) => {
                revoked(m)?;
                m.call(Some(filter), &[node])?
            }
            None => return Err(type_error_of(s, "NodeFilter has no acceptNode method")),
        }
    };
    // Web IDL `unsigned short`: ToNumber, then modulo 2^16.
    let n = result.to_number()?;
    if !n.is_finite() {
        return Ok(0);
    }
    Ok(n.trunc().rem_euclid(65536.0) as u16)
}

/// The walker data, with its identity key when it has a filter to keep on its wrapper.
fn new_walker(s: &Scope<'_>, n: &NodeData, a: &Args<'_>) -> JsResult<(WalkerData, Option<lp_js::IdentityKey>)> {
    let root = node_arg(s, a.get(0), "createTreeWalker")?;
    let what = if a.get(1).is_undefined() { 0xFFFF_FFFF } else { to_u32(a.get(1))? };
    // `NodeFilter?` is a callback interface: null/undefined, or any object.
    if !a.get(2).is_nullish() && !a.get(2).is_object() {
        return Err(JsError::Type("parameter 3 is not of type 'NodeFilter'.".into()));
    }
    let identity = (!a.get(2).is_nullish()).then(|| n.page().event_identity(n.page().next_event_id()));
    let filter = identity.map(|key| WrapperRef::new(s, key));
    Ok((
        WalkerData {
            root: NodeHold::new(n.page(), root)?,
            current: RefCell::new(NodeHold::new(n.page(), root)?),
            what_to_show: what,
            filter,
            active: Cell::new(false),
            before_ref: Cell::new(true),
            live: None,
        },
        identity,
    ))
}

/// Wraps walker data and stores its filter on the wrapper.
fn wrap_walker<'s, C: JsClass<Data = WalkerData>>(
    s: &Scope<'s>,
    a: &Args<'s>,
    d: (WalkerData, Option<lp_js::IdentityKey>),
) -> JsResult<Value<'s>> {
    let (data, identity) = d;
    let wrapper = s.wrap::<C>(data, identity)?;
    if identity.is_some() {
        s.set_hidden(wrapper, FILTER, a.get(2))?;
    }
    Ok(wrapper.value())
}

pub(crate) fn create_tree_walker<'s>(s: &Scope<'s>, n: &NodeData, a: Args<'s>) -> JsResult<Value<'s>> {
    a.require(1, "Document.createTreeWalker")?;
    let d = new_walker(s, n, &a)?;
    wrap_walker::<TreeWalker>(s, &a, d)
}

pub(crate) fn create_node_iterator<'s>(s: &Scope<'s>, n: &NodeData, a: Args<'s>) -> JsResult<Value<'s>> {
    a.require(1, "Document.createNodeIterator")?;
    let (mut data, identity) = new_walker(s, n, &a)?;
    data.live = Some(n.page().dom.borrow_mut().new_live_iterator(data.root.id).js()?);
    wrap_walker::<NodeIterator>(s, &a, (data, identity))
}

type Step = fn(&lp_dom::Dom, NodeId) -> Option<NodeId>;

fn rel(w: &WalkerData, id: NodeId, f: Step) -> Option<NodeId> {
    f(&w.page().dom.borrow(), id)
}

fn first_child(d: &lp_dom::Dom, id: NodeId) -> Option<NodeId> {
    d.first_child(id).ok().flatten()
}
fn last_child(d: &lp_dom::Dom, id: NodeId) -> Option<NodeId> {
    d.last_child(id).ok().flatten()
}
fn next_sib(d: &lp_dom::Dom, id: NodeId) -> Option<NodeId> {
    d.next_sibling(id).ok().flatten()
}
fn prev_sib(d: &lp_dom::Dom, id: NodeId) -> Option<NodeId> {
    d.prev_sibling(id).ok().flatten()
}
fn parent(d: &lp_dom::Dom, id: NodeId) -> Option<NodeId> {
    d.parent(id).ok().flatten()
}

/// TreeWalker "traverse children".
fn traverse_children(s: &Scope<'_>, w: &WalkerData, first: bool) -> JsResult<Option<NodeId>> {
    let (child_fn, sib_fn): (Step, Step) = if first { (first_child, next_sib) } else { (last_child, prev_sib) };
    let mut node = rel(w, w.cur(), child_fn);
    while let Some(n) = node {
        match w.accept(s, n)? {
            FILTER_ACCEPT => {
                w.set_cur(n)?;
                return Ok(Some(n));
            }
            FILTER_SKIP => {
                if let Some(c) = rel(w, n, child_fn) {
                    node = Some(c);
                    continue;
                }
            }
            _ => {}
        }
        let mut cur = n;
        loop {
            if let Some(sib) = rel(w, cur, sib_fn) {
                node = Some(sib);
                break;
            }
            match rel(w, cur, parent) {
                Some(p) if p != w.root.id && p != w.cur() => cur = p,
                _ => return Ok(None),
            }
        }
    }
    Ok(None)
}

fn traverse_siblings(s: &Scope<'_>, w: &WalkerData, next: bool) -> JsResult<Option<NodeId>> {
    let (sib_fn, child_fn): (Step, Step) = if next { (next_sib, first_child) } else { (prev_sib, last_child) };
    let mut node = w.cur();
    if node == w.root.id {
        return Ok(None);
    }
    loop {
        let mut sib = rel(w, node, sib_fn);
        while let Some(sb) = sib {
            node = sb;
            let r = w.accept(s, node)?;
            if r == FILTER_ACCEPT {
                w.set_cur(node)?;
                return Ok(Some(node));
            }
            sib = rel(w, node, child_fn);
            if r == FILTER_REJECT || sib.is_none() {
                sib = rel(w, node, sib_fn);
            }
        }
        match rel(w, node, parent) {
            None => return Ok(None),
            Some(p) if p == w.root.id => return Ok(None),
            Some(p) => {
                node = p;
                if w.accept(s, node)? == FILTER_ACCEPT {
                    return Ok(None);
                }
            }
        }
    }
}

fn opt<'s>(s: &Scope<'s>, w: &WalkerData, id: Option<NodeId>) -> JsResult<Value<'s>> {
    match id {
        Some(i) => Ok(wrap_node(s, w.page(), i)?.value()),
        None => Ok(s.null()),
    }
}

pub struct TreeWalker;
impl JsClass for TreeWalker {
    const NAME: &'static str = "TreeWalker";
    type Data = WalkerData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.getter("root", |s, w| Ok(wrap_node(s, w.page(), w.root.id)?.value()));
        def.getter("whatToShow", |s, w| Ok(s.number(w.what_to_show as f64)));
        def.getter("filter", |s, w| match &w.filter {
            Some(f) => Ok(f.hidden(s, FILTER)?.unwrap_or(s.null())),
            None => Ok(s.null()),
        });
        def.accessor(
            "currentNode",
            |s, w| Ok(wrap_node(s, w.page(), w.cur())?.value()),
            |s, w, v| w.set_cur(node_arg(s, v, "currentNode")?),
        );
        def.method("parentNode", 0, |s, w, _a| {
            let mut node = w.cur();
            while node != w.root.id {
                match rel(w, node, parent) {
                    Some(p) => {
                        node = p;
                        if w.accept(s, node)? == FILTER_ACCEPT {
                            w.set_cur(node)?;
                            return opt(s, w, Some(node));
                        }
                    }
                    None => break,
                }
            }
            Ok(s.null())
        });
        def.method("firstChild", 0, |s, w, _a| {
            let r = traverse_children(s, w, true)?;
            opt(s, w, r)
        });
        def.method("lastChild", 0, |s, w, _a| {
            let r = traverse_children(s, w, false)?;
            opt(s, w, r)
        });
        def.method("nextSibling", 0, |s, w, _a| {
            let r = traverse_siblings(s, w, true)?;
            opt(s, w, r)
        });
        def.method("previousSibling", 0, |s, w, _a| {
            let r = traverse_siblings(s, w, false)?;
            opt(s, w, r)
        });
        def.method("nextNode", 0, |s, w, _a| {
            let mut node = w.cur();
            let mut result = FILTER_ACCEPT;
            loop {
                while result != FILTER_REJECT {
                    let Some(c) = rel(w, node, first_child) else { break };
                    node = c;
                    result = w.accept(s, node)?;
                    if result == FILTER_ACCEPT {
                        w.set_cur(node)?;
                        return opt(s, w, Some(node));
                    }
                }
                let mut sibling = None;
                let mut temp = Some(node);
                while let Some(t) = temp {
                    if t == w.root.id {
                        return Ok(s.null());
                    }
                    sibling = rel(w, t, next_sib);
                    if sibling.is_some() {
                        break;
                    }
                    temp = rel(w, t, parent);
                }
                let Some(sb) = sibling else { return Ok(s.null()) };
                node = sb;
                result = w.accept(s, node)?;
                if result == FILTER_ACCEPT {
                    w.set_cur(node)?;
                    return opt(s, w, Some(node));
                }
            }
        });
        def.method("previousNode", 0, |s, w, _a| {
            let mut node = w.cur();
            while node != w.root.id {
                let mut sibling = rel(w, node, prev_sib);
                while let Some(sb) = sibling {
                    node = sb;
                    let mut result = w.accept(s, node)?;
                    while result != FILTER_REJECT {
                        let Some(lc) = rel(w, node, last_child) else { break };
                        node = lc;
                        result = w.accept(s, node)?;
                    }
                    if result == FILTER_ACCEPT {
                        w.set_cur(node)?;
                        return opt(s, w, Some(node));
                    }
                    sibling = rel(w, node, prev_sib);
                }
                if node == w.root.id {
                    break;
                }
                let Some(p) = rel(w, node, parent) else { break };
                node = p;
                if w.accept(s, node)? == FILTER_ACCEPT {
                    w.set_cur(node)?;
                    return opt(s, w, Some(node));
                }
            }
            Ok(s.null())
        });
    }
}

fn following(dom: &lp_dom::Dom, root: NodeId, n: NodeId) -> Option<NodeId> {
    if let Some(c) = first_child(dom, n) {
        return Some(c);
    }
    let mut cur = n;
    loop {
        if cur == root {
            return None;
        }
        if let Some(sb) = next_sib(dom, cur) {
            return Some(sb);
        }
        cur = parent(dom, cur)?;
    }
}

fn preceding(dom: &lp_dom::Dom, root: NodeId, n: NodeId) -> Option<NodeId> {
    if n == root {
        return None;
    }
    match prev_sib(dom, n) {
        Some(mut p) => {
            while let Some(l) = last_child(dom, p) {
                p = l;
            }
            Some(p)
        }
        None => parent(dom, n),
    }
}

fn iterate<'s>(s: &Scope<'s>, w: &WalkerData, forward: bool) -> JsResult<Value<'s>> {
    let mut node = w.cur();
    let mut before = w.before();
    loop {
        if forward {
            if before {
                before = false;
            } else {
                match following(&w.page().dom.borrow(), w.root.id, node) {
                    Some(n) => node = n,
                    None => return Ok(s.null()),
                }
            }
        } else if !before {
            before = true;
        } else {
            match preceding(&w.page().dom.borrow(), w.root.id, node) {
                Some(n) => node = n,
                None => return Ok(s.null()),
            }
        }
        // The position reached is live while the filter runs: a filter that
        // removes the node (or an ancestor) moves it like the reference, and
        // the traversal goes on, or ends, from where it was moved to.
        let position = w.filter_at(s, node, before)?;
        let (result, (at, at_before)) = position;
        if result == FILTER_ACCEPT {
            w.before_ref.set(at_before);
            w.set_cur(at)?;
            w.set_before(at_before)?;
            return opt(s, w, Some(node));
        }
        node = at;
        before = at_before;
    }
}

pub struct NodeIterator;
impl JsClass for NodeIterator {
    const NAME: &'static str = "NodeIterator";
    type Data = WalkerData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.getter("root", |s, w| Ok(wrap_node(s, w.page(), w.root.id)?.value()));
        def.getter("referenceNode", |s, w| Ok(wrap_node(s, w.page(), w.cur())?.value()));
        def.getter("pointerBeforeReferenceNode", |s, w| Ok(s.boolean(w.before())));
        def.getter("whatToShow", |s, w| Ok(s.number(w.what_to_show as f64)));
        def.getter("filter", |s, w| match &w.filter {
            Some(f) => Ok(f.hidden(s, FILTER)?.unwrap_or(s.null())),
            None => Ok(s.null()),
        });
        def.method("nextNode", 0, |s, w, _a| iterate(s, w, true));
        def.method("previousNode", 0, |s, w, _a| iterate(s, w, false));
        def.method("detach", 0, |s, _w, _a| Ok(s.undefined()));
    }
}

pub(crate) fn register(engine: &lp_js::Engine) {
    engine.register::<NodeFilter>();
    engine.register::<TreeWalker>();
    engine.register::<NodeIterator>();
}
