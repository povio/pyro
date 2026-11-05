//! Range, StaticRange and Selection (minimal). Range boundary points are live:
//! `lp_dom::live_range` adjusts them as the tree changes. A node from another
//! page's `Dom` can't be a boundary point here: comparison methods treat it as
//! a different root, the setters report NotSupportedError.

use std::rc::Rc;

use lp_dom::live_range::{LiveRange, Point};
use lp_dom::{position, Dom, NodeId, NodeType};
use lp_js::{Args, ClassDef, JsClass, JsError, JsResult, NoParent, Scope, Value};

use crate::collections::NodeHold;
use crate::node::{node_arg, wrap_node, Node};
use crate::page::PageDom;
use crate::util::*;

/// A live range and the node references its boundary points hold. Shared by
/// the Range wrappers over it and, while it is the selection's range, by the
/// document selection (`crate::editing`).
pub(crate) struct RangeOwner {
    pub(crate) page: Rc<PageDom>,
    pub(crate) live: Rc<LiveRange>,
}

impl RangeOwner {
    /// A range from `start` to `end` (in tree order).
    pub(crate) fn new(page: &Rc<PageDom>, start: Point, end: Point) -> JsResult<Rc<RangeOwner>> {
        let mut dom = page.dom.borrow_mut();
        let live = dom.new_live_range(start.0).js()?;
        let owner = Rc::new(RangeOwner { page: page.clone(), live });
        let set = dom.set_live_range(&owner.live, start, end).js();
        // Dropping the owner releases nodes: not while the DOM is borrowed.
        drop(dom);
        set.map(|()| owner)
    }
}

impl Drop for RangeOwner {
    fn drop(&mut self) {
        self.page.release_node(self.live.start().0);
        self.page.release_node(self.live.end().0);
    }
}

pub struct RangeData {
    page: Rc<PageDom>,
    live: Rc<LiveRange>,
    owner: Rc<RangeOwner>,
}

impl RangeData {
    fn page(&self) -> Rc<PageDom> {
        self.page.clone()
    }
    pub fn new(page: &Rc<PageDom>, node: NodeId) -> JsResult<RangeData> {
        Ok(RangeData::from_owner(RangeOwner::new(page, (node, 0), (node, 0))?))
    }
    pub(crate) fn from_owner(owner: Rc<RangeOwner>) -> RangeData {
        RangeData { page: owner.page.clone(), live: owner.live.clone(), owner }
    }
}

fn node_length_in(dom: &Dom, n: NodeId) -> JsResult<u32> {
    Ok(match dom.node_type(n).js()? {
        NodeType::DocumentType => 0,
        NodeType::Text | NodeType::Comment | NodeType::CDataSection | NodeType::ProcessingInstruction => {
            dom.char_length(n).js()?
        }
        _ => dom.child_count(n).js()?,
    })
}

fn node_length(page: &PageDom, n: NodeId) -> JsResult<u32> {
    node_length_in(&page.dom.borrow(), n)
}

fn is_char_data(dom: &Dom, n: NodeId) -> JsResult<bool> {
    Ok(matches!(
        dom.node_type(n).js()?,
        NodeType::Text | NodeType::Comment | NodeType::CDataSection | NodeType::ProcessingInstruction
    ))
}

/// Compares boundary points (DOM "position of a boundary point"): -1, 0, 1.
fn compare_points(page: &PageDom, a: Point, b: Point) -> JsResult<i8> {
    let dom = page.dom.borrow();
    compare_points_in(&dom, a, b)
}

fn compare_points_in(dom: &Dom, a: Point, b: Point) -> JsResult<i8> {
    if a.0 == b.0 {
        return Ok(a.1.cmp(&b.1) as i8);
    }
    let pos = dom.compare_document_position(a.0, b.0).js()?;
    if pos & position::CONTAINED_BY != 0 {
        // b inside a: find child of a containing b.
        let mut child = b.0;
        while dom.parent(child).js()? != Some(a.0) {
            child = dom.parent(child).js()?.ok_or_else(|| JsError::Type("broken tree".into()))?;
        }
        let idx = dom.index_in_parent(child).js()?;
        return Ok(if idx < a.1 { 1 } else { -1 });
    }
    if pos & position::CONTAINS != 0 {
        return Ok(-compare_points_in(dom, b, a)?);
    }
    // b follows a (without containment) => a before b.
    Ok(if pos & position::PRECEDING != 0 { 1 } else { -1 })
}

/// Boundary point comparison for the document selection (`crate::editing`).
pub(crate) fn compare_boundary_points(page: &PageDom, a: Point, b: Point) -> JsResult<i8> {
    compare_points(page, a, b)
}

pub(crate) fn compare_boundary_points_dom(dom: &Dom, a: Point, b: Point) -> i8 {
    compare_points_in(dom, a, b).unwrap_or(0)
}

/// A Node argument: `None` when it lives in another page's `Dom`.
fn local_node(s: &Scope<'_>, page: &PageDom, v: Value<'_>, what: &str) -> JsResult<Option<NodeId>> {
    let n = s
        .unwrap::<Node>(v)
        .ok_or_else(|| JsError::Type(format!("Failed to execute '{what}': parameter is not of type 'Node'.")))?;
    Ok(Rc::ptr_eq(&n.page().dom, &page.dom).then_some(n.id))
}

fn boundary_node(s: &Scope<'_>, page: &PageDom, v: Value<'_>, what: &str) -> JsResult<NodeId> {
    local_node(s, page, v, what)?
        .ok_or_else(|| JsError::dom("NotSupportedError", "The node provided belongs to another page."))
}

/// Doctype and offset checks every method taking a boundary point shares.
fn check_point(dom: &Dom, node: NodeId, offset: u32) -> JsResult<()> {
    if dom.node_type(node).js()? == NodeType::DocumentType {
        return Err(JsError::dom("InvalidNodeTypeError", "The node provided is a doctype."));
    }
    if offset > node_length_in(dom, node)? {
        return Err(JsError::dom("IndexSizeError", format!("There is no child at offset {offset}.")));
    }
    Ok(())
}

fn set_boundary(r: &RangeData, s: &Scope<'_>, a: &Args<'_>, start: bool) -> JsResult<()> {
    a.require(2, if start { "Range.setStart" } else { "Range.setEnd" })?;
    let node = boundary_node(s, &r.page, a.get(0), "Range")?;
    let offset = to_u32(a.get(1))?;
    set_point(r, node, offset, start)
}

fn set_point(r: &RangeData, node: NodeId, offset: u32, start: bool) -> JsResult<()> {
    let mut dom = r.page.dom.borrow_mut();
    check_point(&dom, node, offset)?;
    let (s, e) = (r.live.start(), r.live.end());
    let p = (node, offset);
    let other = if start { e } else { s };
    let same_root = dom.root_node(node, false).js()? == dom.root_node(other.0, false).js()?;
    // Moving one end past the other, or to another tree, collapses onto it.
    if start {
        let end = if !same_root || compare_points_in(&dom, p, e)? > 0 { p } else { e };
        dom.set_live_range(&r.live, p, end).js()
    } else {
        let begin = if !same_root || compare_points_in(&dom, p, s)? < 0 { p } else { s };
        dom.set_live_range(&r.live, begin, p).js()
    }
}

fn points(r: &RangeData) -> (NodeId, u32, NodeId, u32) {
    let (s, e) = (r.live.start(), r.live.end());
    (s.0, s.1, e.0, e.1)
}

fn before_after(r: &RangeData, s: &Scope<'_>, a: &Args<'_>, start: bool, after: bool) -> JsResult<()> {
    a.require(1, "Range")?;
    let node = boundary_node(s, &r.page, a.get(0), "Range")?;
    let (parent, idx) = {
        let dom = r.page.dom.borrow();
        let p =
            dom.parent(node).js()?.ok_or_else(|| JsError::dom("InvalidNodeTypeError", "The node has no parent."))?;
        (p, dom.index_in_parent(node).js()?)
    };
    set_point(r, parent, if after { idx + 1 } else { idx }, start)
}

/// Text content of the range (Range.toString()).
fn range_text(r: &RangeData) -> JsResult<String> {
    let page = r.page();
    let (sn, so, en, eo) = points(r);
    let dom = page.dom.borrow();
    let root = dom.root_node(sn, false).js()?;
    if root != dom.root_node(en, false).js()? {
        return Ok(String::new());
    }
    let mut out = String::new();
    for n in std::iter::once(root).chain(dom.descendants(root).js()?) {
        if !matches!(dom.node_type(n), Ok(NodeType::Text | NodeType::CDataSection)) {
            continue;
        }
        let len = dom.char_length(n).js()?;
        // A text node contributes only where its interval intersects the range;
        // compare_points handles offsets into ancestor containers at any depth.
        if compare_points(&page, (n, len), (sn, so))? <= 0 || compare_points(&page, (n, 0), (en, eo))? >= 0 {
            continue;
        }
        let start = if n == sn { so } else { 0 };
        let end = if n == en { eo } else { len };
        out.push_str(&dom.substring_data(n, start, end.saturating_sub(start)).js()?);
    }
    Ok(out)
}

// ----- content methods (DOM "clone the contents", "extract", "delete") -----

fn common_ancestor(dom: &Dom, a: NodeId, b: NodeId) -> JsResult<NodeId> {
    let mut cur = Some(a);
    while let Some(c) = cur {
        if dom.contains(c, b).js()? {
            return Ok(c);
        }
        cur = dom.parent(c).js()?;
    }
    Err(JsError::dom("InvalidStateError", "The range's boundary points are in different trees."))
}

/// How a range's common ancestor's children relate to it.
struct Parts {
    first_partial: Option<NodeId>,
    contained: Vec<NodeId>,
    last_partial: Option<NodeId>,
}

fn partition(dom: &Dom, s: Point, e: Point) -> JsResult<Parts> {
    let common = common_ancestor(dom, s.0, e.0)?;
    let child_of_common = |mut n: NodeId| -> JsResult<NodeId> {
        while let Some(p) = dom.parent(n).js()? {
            if p == common {
                break;
            }
            n = p;
        }
        Ok(n)
    };
    let first_partial = if dom.contains(s.0, e.0).js()? { None } else { Some(child_of_common(s.0)?) };
    let last_partial = if dom.contains(e.0, s.0).js()? { None } else { Some(child_of_common(e.0)?) };
    // Contained children are exactly those strictly between the partially
    // contained ones or, without one, from or up to the boundary offset.
    let mut cur = match first_partial {
        Some(f) => dom.next_sibling(f).js()?,
        None => dom.child_at(common, s.1).js()?,
    };
    let stop = match last_partial {
        Some(l) => Some(l),
        None => dom.child_at(common, e.1).js()?,
    };
    let mut contained = Vec::new();
    while let Some(c) = cur {
        if Some(c) == stop {
            break;
        }
        contained.push(c);
        cur = dom.next_sibling(c).js()?;
    }
    Ok(Parts { first_partial, contained, last_partial })
}

/// A copy of `node` holding only `from..to` of its character data.
fn clone_substring(dom: &mut Dom, node: NodeId, from: u32, to: u32) -> JsResult<NodeId> {
    let data = dom.substring_data(node, from, to - from).js()?;
    let clone = dom.clone_node(node, false, None).js()?;
    dom.set_char_data(clone, &data).js()?;
    Ok(clone)
}

/// "Clone the contents" or, with `extract`, "extract" of `s..e`, leaving
/// the boundary points to the caller.
fn contents(dom: &mut Dom, s: Point, e: Point, extract: bool) -> JsResult<NodeId> {
    let doc = dom.node_document(s.0).js()?;
    let frag = dom.create_fragment(doc).js()?;
    if s == e {
        return Ok(frag);
    }
    if s.0 == e.0 && is_char_data(dom, s.0)? {
        let clone = clone_substring(dom, s.0, s.1, e.1)?;
        dom.append_child(frag, clone).js()?;
        if extract {
            dom.replace_data(s.0, s.1, e.1 - s.1, "").js()?;
        }
        return Ok(frag);
    }
    let parts = partition(dom, s, e)?;
    for &c in &parts.contained {
        if dom.node_type(c).js()? == NodeType::DocumentType {
            return Err(JsError::dom("HierarchyRequestError", "The range contains a doctype."));
        }
    }
    if let Some(f) = parts.first_partial {
        let len = node_length_in(dom, f)?;
        if is_char_data(dom, f)? {
            let clone = clone_substring(dom, f, s.1, len)?;
            dom.append_child(frag, clone).js()?;
            if extract {
                dom.replace_data(f, s.1, len - s.1, "").js()?;
            }
        } else {
            let clone = dom.clone_node(f, false, None).js()?;
            dom.append_child(frag, clone).js()?;
            let sub = contents(dom, s, (f, len), extract)?;
            dom.append_child(clone, sub).js()?;
        }
    }
    for c in parts.contained {
        let moved = if extract { c } else { dom.clone_node(c, true, None).js()? };
        dom.append_child(frag, moved).js()?;
    }
    if let Some(l) = parts.last_partial {
        if is_char_data(dom, l)? {
            let clone = clone_substring(dom, l, 0, e.1)?;
            dom.append_child(frag, clone).js()?;
            if extract {
                dom.replace_data(l, 0, e.1, "").js()?;
            }
        } else {
            let clone = dom.clone_node(l, false, None).js()?;
            dom.append_child(frag, clone).js()?;
            let sub = contents(dom, (l, 0), e, extract)?;
            dom.append_child(clone, sub).js()?;
        }
    }
    Ok(frag)
}

/// Where extract and delete collapse the range to, computed up front.
fn collapse_point(dom: &Dom, s: Point, e: Point) -> JsResult<Point> {
    if dom.contains(s.0, e.0).js()? {
        return Ok(s);
    }
    let mut reference = s.0;
    while let Some(p) = dom.parent(reference).js()? {
        if dom.contains(p, e.0).js()? {
            return Ok((p, dom.index_in_parent(reference).js()? + 1));
        }
        reference = p;
    }
    Err(JsError::dom("InvalidStateError", "The range's boundary points are in different trees."))
}

/// Nodes contained in `s..e` whose parent is not, in tree order.
fn contained_roots(dom: &Dom, s: Point, e: Point, out: &mut Vec<NodeId>) -> JsResult<()> {
    let parts = partition(dom, s, e)?;
    if let Some(f) = parts.first_partial.filter(|&f| !is_char_data(dom, f).unwrap_or(true)) {
        contained_roots(dom, s, (f, node_length_in(dom, f)?), out)?;
    }
    out.extend(parts.contained);
    if let Some(l) = parts.last_partial.filter(|&l| !is_char_data(dom, l).unwrap_or(true)) {
        contained_roots(dom, (l, 0), e, out)?;
    }
    Ok(())
}

/// "Delete the contents" of `s..e`; returns the point to collapse to.
fn delete_contents(dom: &mut Dom, s: Point, e: Point) -> JsResult<Point> {
    if s == e {
        return Ok(s);
    }
    if s.0 == e.0 && is_char_data(dom, s.0)? {
        dom.replace_data(s.0, s.1, e.1 - s.1, "").js()?;
        return Ok(s);
    }
    let to = collapse_point(dom, s, e)?;
    let mut nodes = Vec::new();
    contained_roots(dom, s, e, &mut nodes)?;
    if is_char_data(dom, s.0)? {
        dom.replace_data(s.0, s.1, u32::MAX, "").js()?;
    }
    for n in nodes {
        dom.remove_node(n).js()?;
    }
    if is_char_data(dom, e.0)? {
        dom.replace_data(e.0, 0, e.1, "").js()?;
    }
    Ok(to)
}

fn extract(r: &RangeData, dom: &mut Dom) -> JsResult<NodeId> {
    let (s, e) = (r.live.start(), r.live.end());
    let to = collapse_point(dom, s, e)?;
    let frag = contents(dom, s, e, true)?;
    dom.set_live_range(&r.live, to, to).js()?;
    Ok(frag)
}

fn insert_node(r: &RangeData, dom: &mut Dom, node: NodeId) -> JsResult<()> {
    let start = r.live.start();
    let ty = dom.node_type(start.0).js()?;
    let is_text = matches!(ty, NodeType::Text | NodeType::CDataSection);
    if matches!(ty, NodeType::ProcessingInstruction | NodeType::Comment)
        || (is_text && dom.parent(start.0).js()?.is_none())
        || start.0 == node
    {
        return Err(JsError::dom("HierarchyRequestError", "The range's start can't take this node."));
    }
    let mut reference = if is_text { Some(start.0) } else { dom.child_at(start.0, start.1).js()? };
    let parent = match reference {
        Some(c) => dom.parent(c).js()?.ok_or_else(|| JsError::Type("broken tree".into()))?,
        None => start.0,
    };
    dom.check_pre_insert(parent, node, reference).js()?;
    if is_text {
        reference = Some(dom.split_text(start.0, start.1).js()?);
    }
    if reference == Some(node) {
        reference = dom.next_sibling(node).js()?;
    }
    dom.remove_node(node).js()?;
    let at = match reference {
        Some(c) => dom.index_in_parent(c).js()?,
        None => node_length_in(dom, parent)?,
    };
    let count = if dom.node_type(node).js()? == NodeType::DocumentFragment { dom.child_count(node).js()? } else { 1 };
    dom.insert_before(parent, node, reference).js()?;
    let (s, e) = (r.live.start(), r.live.end());
    if s == e {
        dom.set_live_range(&r.live, s, (parent, at + count)).js()?;
    }
    Ok(())
}

fn surround_contents(r: &RangeData, dom: &mut Dom, new_parent: NodeId) -> JsResult<()> {
    let (s, e) = (r.live.start(), r.live.end());
    for (from, other) in [(s.0, e.0), (e.0, s.0)] {
        let mut cur = Some(from);
        while let Some(c) = cur {
            if dom.contains(c, other).js()? {
                break;
            }
            if dom.node_type(c).js()? != NodeType::Text && dom.node_type(c).js()? != NodeType::CDataSection {
                return Err(JsError::dom("InvalidStateError", "The range partially contains a non-Text node."));
            }
            cur = dom.parent(c).js()?;
        }
    }
    if matches!(
        dom.node_type(new_parent).js()?,
        NodeType::Document | NodeType::DocumentType | NodeType::DocumentFragment
    ) {
        return Err(JsError::dom("InvalidNodeTypeError", "The node provided can't surround a range."));
    }
    let frag = extract(r, dom)?;
    if dom.child_count(new_parent).js()? > 0 {
        dom.replace_all(new_parent, None).js()?;
    }
    insert_node(r, dom, new_parent)?;
    dom.append_child(new_parent, frag).js()?;
    let parent =
        dom.parent(new_parent).js()?.ok_or_else(|| JsError::dom("InvalidNodeTypeError", "The node has no parent."))?;
    let i = dom.index_in_parent(new_parent).js()?;
    dom.set_live_range(&r.live, (parent, i), (parent, i + 1)).js()
}

fn wrong_document() -> JsError {
    JsError::dom("WrongDocumentError", "The node provided and this range have different roots.")
}

pub struct Range;
impl JsClass for Range {
    const NAME: &'static str = "Range";
    type Data = RangeData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(0, |s, nt, _a| {
            let page = crate::target::page_of(s)?;
            let doc = page.document();
            s.wrap_new::<Range>(RangeData::new(&page, doc)?, nt, None)
        });
        for (n, v) in [("START_TO_START", 0.0), ("START_TO_END", 1.0), ("END_TO_END", 2.0), ("END_TO_START", 3.0)] {
            def.constant(n, v);
        }
        def.getter("startContainer", |s, r| Ok(wrap_node(s, &r.page(), points(r).0)?.value()));
        def.getter("startOffset", |s, r| Ok(s.number(points(r).1 as f64)));
        def.getter("endContainer", |s, r| Ok(wrap_node(s, &r.page(), points(r).2)?.value()));
        def.getter("endOffset", |s, r| Ok(s.number(points(r).3 as f64)));
        def.getter("collapsed", |s, r| {
            let (a, b, c, d) = points(r);
            Ok(s.boolean(a == c && b == d))
        });
        def.getter("commonAncestorContainer", |s, r| {
            let page = r.page();
            let (sn, _, en, _) = points(r);
            let c = common_ancestor(&page.dom.borrow(), sn, en)?;
            Ok(wrap_node(s, &page, c)?.value())
        });
        def.method("setStart", 2, |s, r, a| set_boundary(r, s, &a, true).map(|_| s.undefined()));
        def.method("setEnd", 2, |s, r, a| set_boundary(r, s, &a, false).map(|_| s.undefined()));
        def.method("setStartBefore", 1, |s, r, a| before_after(r, s, &a, true, false).map(|_| s.undefined()));
        def.method("setStartAfter", 1, |s, r, a| before_after(r, s, &a, true, true).map(|_| s.undefined()));
        def.method("setEndBefore", 1, |s, r, a| before_after(r, s, &a, false, false).map(|_| s.undefined()));
        def.method("setEndAfter", 1, |s, r, a| before_after(r, s, &a, false, true).map(|_| s.undefined()));
        def.method("selectNode", 1, |s, r, a| {
            before_after(r, s, &a, true, false)?;
            before_after(r, s, &a, false, true)?;
            Ok(s.undefined())
        });
        def.method("selectNodeContents", 1, |s, r, a| {
            a.require(1, "Range.selectNodeContents")?;
            let node = boundary_node(s, &r.page, a.get(0), "Range.selectNodeContents")?;
            if r.page.dom.borrow().node_type(node).js()? == NodeType::DocumentType {
                return Err(JsError::dom("InvalidNodeTypeError", "The node provided is a doctype."));
            }
            let len = node_length(&r.page, node)?;
            r.page.dom.borrow_mut().set_live_range(&r.live, (node, 0), (node, len)).js()?;
            Ok(s.undefined())
        });
        def.method("collapse", 0, |s, r, a| {
            let (start, end) = (r.live.start(), r.live.end());
            let p = if a.get(0).to_bool() { start } else { end };
            r.page.dom.borrow_mut().set_live_range(&r.live, p, p).js()?;
            Ok(s.undefined())
        });
        def.method("cloneRange", 0, |s, r, _a| {
            let d = RangeData::new(&r.page, r.live.start().0)?;
            r.page.dom.borrow_mut().set_live_range(&d.live, r.live.start(), r.live.end()).js()?;
            Ok(s.wrap::<Range>(d, None)?.value())
        });
        def.method("toString", 0, |s, r, _a| Ok(s.string(&range_text(r)?)));
        def.method("detach", 0, |s, _r, _a| Ok(s.undefined()));
        def.method("compareBoundaryPoints", 2, |s, r, a| {
            a.require(2, "Range.compareBoundaryPoints")?;
            // WebIDL `unsigned short`.
            let how = to_u32(a.get(0))? & 0xffff;
            let other = s
                .unwrap::<Range>(a.get(1))
                .ok_or_else(|| JsError::Type("parameter 2 is not of type 'Range'.".into()))?;
            if how > 3 {
                return Err(JsError::dom("NotSupportedError", "The comparison method provided is invalid."));
            }
            let dom = r.page.dom.borrow();
            if other.page.id() != r.page.id()
                || dom.root_node(r.live.start().0, false).js()? != dom.root_node(other.live.start().0, false).js()?
            {
                return Err(JsError::dom("WrongDocumentError", "The two ranges have different roots."));
            }
            let (x, y) = match how {
                0 => (r.live.start(), other.live.start()),
                1 => (r.live.end(), other.live.start()),
                2 => (r.live.end(), other.live.end()),
                _ => (r.live.start(), other.live.end()),
            };
            Ok(s.number(compare_points_in(&dom, x, y)? as f64))
        });
        def.method("comparePoint", 2, |s, r, a| {
            a.require(2, "Range.comparePoint")?;
            let node = local_node(s, &r.page, a.get(0), "Range.comparePoint")?;
            let off = to_u32(a.get(1))?;
            let node = node.ok_or_else(wrong_document)?;
            let dom = r.page.dom.borrow();
            if dom.root_node(node, false).js()? != dom.root_node(r.live.start().0, false).js()? {
                return Err(wrong_document());
            }
            check_point(&dom, node, off)?;
            Ok(s.number(if compare_points_in(&dom, (node, off), r.live.start())? < 0 {
                -1.0
            } else if compare_points_in(&dom, (node, off), r.live.end())? > 0 {
                1.0
            } else {
                0.0
            }))
        });
        def.method("isPointInRange", 2, |s, r, a| {
            a.require(2, "Range.isPointInRange")?;
            let node = local_node(s, &r.page, a.get(0), "Range.isPointInRange")?;
            let off = to_u32(a.get(1))?;
            let Some(node) = node else { return Ok(s.boolean(false)) };
            let dom = r.page.dom.borrow();
            if dom.root_node(node, false).js()? != dom.root_node(r.live.start().0, false).js()? {
                return Ok(s.boolean(false));
            }
            check_point(&dom, node, off)?;
            Ok(s.boolean(
                compare_points_in(&dom, (node, off), r.live.start())? >= 0
                    && compare_points_in(&dom, (node, off), r.live.end())? <= 0,
            ))
        });
        def.method("intersectsNode", 1, |s, r, a| {
            a.require(1, "Range.intersectsNode")?;
            let Some(node) = local_node(s, &r.page, a.get(0), "Range.intersectsNode")? else {
                return Ok(s.boolean(false));
            };
            let dom = r.page.dom.borrow();
            if dom.root_node(node, false).js()? != dom.root_node(r.live.start().0, false).js()? {
                return Ok(s.boolean(false));
            }
            let Some(p) = dom.parent(node).js()? else { return Ok(s.boolean(true)) };
            let i = dom.index_in_parent(node).js()?;
            Ok(s.boolean(
                compare_points_in(&dom, (p, i), r.live.end())? < 0
                    && compare_points_in(&dom, (p, i + 1), r.live.start())? > 0,
            ))
        });
        def.method("createContextualFragment", 1, |s, r, a| {
            let html = arg_string(&a, 0, "Range.createContextualFragment")?;
            let page = r.page();
            let ctx = {
                let dom = page.dom.borrow();
                let sn = points(r).0;
                if dom.node_type(sn).js()? == NodeType::Element {
                    sn
                } else {
                    dom.parent_element(sn)
                        .js()?
                        .or(dom.body(page.document()).js()?)
                        .ok_or_else(|| JsError::Type("no context element".into()))?
                }
            };
            let frag = lp_dom::parse::parse_fragment(&mut page.dom.borrow_mut(), ctx, &html).js()?;
            Ok(wrap_node(s, &page, frag)?.value())
        });
        def.method("getBoundingClientRect", 0, |s, r, _a| crate::layout_api::range_rect(s, &r.page(), points(r)));
        def.method("getClientRects", 0, |s, r, _a| {
            let rect = crate::layout_api::range_rect(s, &r.page(), points(r))?;
            Ok(s.array(&[rect])?.value())
        });
        def.method("deleteContents", 0, |s, r, _a| {
            let mut dom = r.page.dom.borrow_mut();
            let to = delete_contents(&mut dom, r.live.start(), r.live.end())?;
            dom.set_live_range(&r.live, to, to).js()?;
            Ok(s.undefined())
        });
        def.method("extractContents", 0, |s, r, _a| {
            let frag = extract(r, &mut r.page.dom.borrow_mut())?;
            Ok(wrap_node(s, &r.page, frag)?.value())
        });
        def.method("cloneContents", 0, |s, r, _a| {
            let frag = contents(&mut r.page.dom.borrow_mut(), r.live.start(), r.live.end(), false)?;
            Ok(wrap_node(s, &r.page, frag)?.value())
        });
        def.method("insertNode", 1, |s, r, a| {
            a.require(1, "Range.insertNode")?;
            let node = boundary_node(s, &r.page, a.get(0), "Range.insertNode")?;
            insert_node(r, &mut r.page.dom.borrow_mut(), node)?;
            Ok(s.undefined())
        });
        def.method("surroundContents", 1, |s, r, a| {
            a.require(1, "Range.surroundContents")?;
            let node = boundary_node(s, &r.page, a.get(0), "Range.surroundContents")?;
            surround_contents(r, &mut r.page.dom.borrow_mut(), node)?;
            Ok(s.undefined())
        });
    }
}

// ----- StaticRange ----------------------------------------------------------------

pub struct StaticRangeData {
    start: NodeHold,
    start_offset: u32,
    end: NodeHold,
    end_offset: u32,
}

/// One required `StaticRangeInit` boundary point.
fn static_point(
    s: &Scope<'_>,
    page: &Rc<PageDom>,
    init: &lp_js::Object<'_>,
    node: &str,
    offset: &str,
) -> JsResult<(NodeHold, u32)> {
    let id = boundary_node(s, page, init.get(node)?, "StaticRange")?;
    let off = init.get(offset)?;
    if off.is_undefined() {
        return Err(JsError::Type(format!(
            "Failed to construct 'StaticRange': required member {offset} is undefined."
        )));
    }
    if matches!(page.dom.borrow().node_type(id).js()?, NodeType::DocumentType | NodeType::Attribute) {
        return Err(JsError::dom("InvalidNodeTypeError", "The node provided is a doctype or an attribute."));
    }
    Ok((NodeHold::new(page, id)?, to_u32(off)?))
}

pub struct StaticRange;
impl JsClass for StaticRange {
    const NAME: &'static str = "StaticRange";
    type Data = StaticRangeData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(1, |s, nt, a| {
            a.require(1, "StaticRange")?;
            let page = crate::target::page_of(s)?;
            let init = a.get(0).as_object().ok_or_else(|| {
                JsError::Type("Failed to construct 'StaticRange': parameter 1 is not an object.".into())
            })?;
            let (start, start_offset) = static_point(s, &page, &init, "startContainer", "startOffset")?;
            let (end, end_offset) = static_point(s, &page, &init, "endContainer", "endOffset")?;
            s.wrap_new::<StaticRange>(StaticRangeData { start, start_offset, end, end_offset }, nt, None)
        });
        def.getter("startContainer", |s, r| Ok(wrap_node(s, &r.start.page, r.start.id)?.value()));
        def.getter("startOffset", |s, r| Ok(s.number(r.start_offset as f64)));
        def.getter("endContainer", |s, r| Ok(wrap_node(s, &r.end.page, r.end.id)?.value()));
        def.getter("endOffset", |s, r| Ok(s.number(r.end_offset as f64)));
        def.getter("collapsed", |s, r| Ok(s.boolean(r.start.id == r.end.id && r.start_offset == r.end_offset)));
    }
}

pub(crate) fn new_range<'s>(s: &Scope<'s>, page: &Rc<PageDom>, doc: NodeId) -> JsResult<Value<'s>> {
    Ok(s.wrap::<Range>(RangeData::new(page, doc)?, None)?.value())
}

// ----- Selection ------------------------------------------------------------------

/// A realm's view of the document selection (`crate::editing`): every
/// world's Selection object reads and writes the same per-document state.
pub struct SelectionData {
    page: Rc<PageDom>,
}

/// The Range object of the selection for this realm: the one script added,
/// or one made over the selection's range on first use.
fn selection_range_object<'s>(s: &Scope<'s>, page: &Rc<PageDom>) -> JsResult<Value<'s>> {
    let realm = s.realm_id();
    let owner = {
        let sel = page.selection.borrow();
        let Some(doc_sel) = sel.as_ref() else { return Ok(s.null()) };
        if let Some((_, wrapper)) = doc_sel.wrappers.iter().find(|(r, _)| *r == realm) {
            return Ok(wrapper.get(s));
        }
        doc_sel.range.clone()
    };
    let value = s.wrap::<Range>(RangeData::from_owner(owner.clone()), None)?.value();
    if let Some(doc_sel) = page.selection.borrow_mut().as_mut().filter(|d| Rc::ptr_eq(&d.range, &owner)) {
        doc_sel.wrappers.push((realm, lp_js::Persistent::new(s, value)));
    }
    Ok(value)
}

/// Whether `node` is in the page's document (the selection ignores nodes
/// of other trees).
fn in_document(page: &PageDom, node: NodeId) -> bool {
    let dom = page.dom.borrow();
    dom.is_connected(node).unwrap_or(false) && dom.node_document(node).ok() == Some(page.document())
}

/// A boundary point argument: the node must be in this document and the
/// offset within its length.
fn point_arg(
    s: &Scope<'_>,
    page: &Rc<PageDom>,
    node: Value<'_>,
    offset: Value<'_>,
    what: &str,
) -> JsResult<(NodeId, u32)> {
    let node = node_arg(s, node, what)?;
    let off = if offset.is_undefined() { 0 } else { to_u32(offset)? };
    if page.dom.borrow().node_type(node).js()? == NodeType::DocumentType {
        return Err(JsError::dom("InvalidNodeTypeError", "The node provided is a DocumentType node."));
    }
    if off > node_length(page, node)? {
        return Err(JsError::dom("IndexSizeError", format!("There is no child at offset {off}.")));
    }
    Ok((node, off))
}

fn node_or_null<'s>(s: &Scope<'s>, page: &Rc<PageDom>, n: Option<NodeId>) -> JsResult<Value<'s>> {
    match n {
        Some(n) => Ok(wrap_node(s, page, n)?.value()),
        None => Ok(s.null()),
    }
}

pub struct Selection;
impl JsClass for Selection {
    const NAME: &'static str = "Selection";
    type Data = SelectionData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        use crate::editing;
        def.getter("rangeCount", |s, sel| {
            Ok(s.number(if editing::selection(&sel.page).is_some() { 1.0 } else { 0.0 }))
        });
        def.getter("type", |s, sel| {
            Ok(s.string(match editing::selection(&sel.page) {
                None => "None",
                Some((a, f)) if a == f => "Caret",
                Some(_) => "Range",
            }))
        });
        def.getter("isCollapsed", |s, sel| Ok(s.boolean(editing::selection(&sel.page).is_none_or(|(a, f)| a == f))));
        def.getter("anchorNode", |s, sel| node_or_null(s, &sel.page, editing::selection(&sel.page).map(|(a, _)| a.0)));
        def.getter("anchorOffset", |s, sel| Ok(s.number(editing::selection(&sel.page).map_or(0, |(a, _)| a.1) as f64)));
        def.getter("focusNode", |s, sel| node_or_null(s, &sel.page, editing::selection(&sel.page).map(|(_, f)| f.0)));
        def.getter("focusOffset", |s, sel| Ok(s.number(editing::selection(&sel.page).map_or(0, |(_, f)| f.1) as f64)));
        def.getter("direction", |s, sel| {
            Ok(s.string(match editing::selection(&sel.page) {
                None => "none",
                Some((a, f)) if a == f => "none",
                Some((a, f)) if editing::compare(&sel.page, a, f) <= 0 => "forward",
                Some(_) => "backward",
            }))
        });
        def.method("getRangeAt", 1, |s, sel, a| {
            if editing::selection(&sel.page).is_none() || to_u32(a.get(0)).ok() != Some(0) {
                return Err(JsError::dom("IndexSizeError", "The index provided is not a valid range index."));
            }
            selection_range_object(s, &sel.page)
        });
        def.method("addRange", 1, |s, sel, a| {
            let Some(r) = s.unwrap::<Range>(a.get(0)) else {
                return Err(JsError::Type("parameter 1 is not of type 'Range'.".into()));
            };
            // A range of another tree is ignored, and so is a second range.
            let (start, end) = (r.live.start(), r.live.end());
            if !Rc::ptr_eq(&r.page, &sel.page)
                || !in_document(&sel.page, start.0)
                || !in_document(&sel.page, end.0)
                || editing::selection(&sel.page).is_some()
            {
                return Ok(s.undefined());
            }
            let owner = r.owner.clone();
            drop(r);
            editing::set_selection_range(&sel.page, owner, Some((s.realm_id(), lp_js::Persistent::new(s, a.get(0)))));
            Ok(s.undefined())
        });
        def.method("removeAllRanges", 0, |s, sel, _a| {
            editing::clear_selection(&sel.page);
            Ok(s.undefined())
        });
        def.method("empty", 0, |s, sel, _a| {
            editing::clear_selection(&sel.page);
            Ok(s.undefined())
        });
        def.method("removeRange", 1, |s, sel, a| {
            let Some(r) = s.unwrap::<Range>(a.get(0)) else {
                return Err(JsError::Type("parameter 1 is not of type 'Range'.".into()));
            };
            let is_selected = sel.page.selection.borrow().as_ref().is_some_and(|d| Rc::ptr_eq(&d.range, &r.owner));
            drop(r);
            if !is_selected {
                return Err(JsError::dom("NotFoundError", "The given range isn't in the selection."));
            }
            editing::clear_selection(&sel.page);
            Ok(s.undefined())
        });
        def.method("collapse", 1, |s, sel, a| {
            if a.get(0).is_null() || a.get(0).is_undefined() {
                editing::clear_selection(&sel.page);
                return Ok(s.undefined());
            }
            let p = point_arg(s, &sel.page, a.get(0), a.get(1), "Selection.collapse")?;
            if in_document(&sel.page, p.0) {
                editing::set_selection(&sel.page, p, p);
            }
            Ok(s.undefined())
        });
        def.method("setPosition", 1, |s, sel, a| {
            if a.get(0).is_null() || a.get(0).is_undefined() {
                editing::clear_selection(&sel.page);
                return Ok(s.undefined());
            }
            let p = point_arg(s, &sel.page, a.get(0), a.get(1), "Selection.setPosition")?;
            if in_document(&sel.page, p.0) {
                editing::set_selection(&sel.page, p, p);
            }
            Ok(s.undefined())
        });
        def.method("collapseToStart", 0, |s, sel, _a| {
            let Some((start, _)) = editing::selection_range(&sel.page) else {
                return Err(JsError::dom("InvalidStateError", "There is no selection to collapse."));
            };
            editing::set_selection(&sel.page, start, start);
            Ok(s.undefined())
        });
        def.method("collapseToEnd", 0, |s, sel, _a| {
            let Some((_, end)) = editing::selection_range(&sel.page) else {
                return Err(JsError::dom("InvalidStateError", "There is no selection to collapse."));
            };
            editing::set_selection(&sel.page, end, end);
            Ok(s.undefined())
        });
        def.method("selectAllChildren", 1, |s, sel, a| {
            let node = node_arg(s, a.get(0), "Selection.selectAllChildren")?;
            let children = {
                let dom = sel.page.dom.borrow();
                if dom.node_type(node).js()? == NodeType::DocumentType {
                    return Err(JsError::dom("InvalidNodeTypeError", "The node provided is a DocumentType node."));
                }
                dom.child_count(node).js()? as u32
            };
            if in_document(&sel.page, node) {
                editing::set_selection(&sel.page, (node, 0), (node, children));
            }
            Ok(s.undefined())
        });
        def.method("setBaseAndExtent", 4, |s, sel, a| {
            a.require(4, "Selection.setBaseAndExtent")?;
            let anchor = point_arg(s, &sel.page, a.get(0), a.get(1), "Selection.setBaseAndExtent")?;
            let focus = point_arg(s, &sel.page, a.get(2), a.get(3), "Selection.setBaseAndExtent")?;
            if in_document(&sel.page, anchor.0) && in_document(&sel.page, focus.0) {
                editing::set_selection(&sel.page, anchor, focus);
            }
            Ok(s.undefined())
        });
        def.method("extend", 1, |s, sel, a| {
            let node = node_arg(s, a.get(0), "Selection.extend")?;
            if !in_document(&sel.page, node) {
                return Ok(s.undefined());
            }
            let Some((anchor, _)) = editing::selection(&sel.page) else {
                return Err(JsError::dom("InvalidStateError", "There is no selection to extend."));
            };
            let focus = point_arg(s, &sel.page, a.get(0), a.get(1), "Selection.extend")?;
            editing::set_selection(&sel.page, anchor, focus);
            Ok(s.undefined())
        });
        def.method("containsNode", 1, |s, sel, a| {
            let node = node_arg(s, a.get(0), "Selection.containsNode")?;
            let Some(((sn, _), (en, _))) = editing::selection_range(&sel.page) else { return Ok(s.boolean(false)) };
            let dom = sel.page.dom.borrow();
            Ok(s.boolean(dom.contains(sn, node).js()? || dom.contains(en, node).js()?))
        });
        def.method("toString", 0, |s, sel, _a| match editing::selection_range(&sel.page) {
            Some((start, end)) => {
                let r = RangeData::new(&sel.page, start.0)?;
                set_point(&r, start.0, start.1, true)?;
                set_point(&r, end.0, end.1, false)?;
                Ok(s.string(&range_text(&r)?))
            }
            None => Ok(s.string("")),
        });
        def.method("deleteFromDocument", 0, |s, sel, _a| {
            if let Some((start, end)) = editing::selection_range(&sel.page) {
                let to = delete_contents(&mut sel.page.dom.borrow_mut(), start, end)?;
                editing::set_selection(&sel.page, to, to);
            }
            Ok(s.undefined())
        });
        def.method("modify", 0, |s, _sel, _a| Ok(s.undefined()));
    }
}

/// `getSelection()`: one Selection per page per realm.
pub fn get_selection<'s>(s: &Scope<'s>, page: &Rc<PageDom>) -> JsResult<Value<'s>> {
    let key = lp_js::IdentityKey(page.id() | (1 << 59), 0);
    crate::window::same_object(s, page, key, || s.wrap::<Selection>(SelectionData { page: page.clone() }, Some(key)))
}

pub(crate) fn register(engine: &lp_js::Engine) {
    engine.register::<Range>();
    engine.register::<StaticRange>();
    engine.register::<Selection>();
}
