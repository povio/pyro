//! Range and Selection (minimal). Boundary points are not updated by DOM
//! mutations (live-range adjustment is not implemented); content
//! manipulation methods report NotSupportedError explicitly.

use std::cell::RefCell;
use std::rc::Rc;

use lp_dom::{position, NodeId, NodeType};
use lp_js::{Args, ClassDef, JsClass, JsError, JsResult, NoParent, Scope, Value};

use crate::collections::NodeHold;
use crate::node::{node_arg, wrap_node};
use crate::page::PageDom;
use crate::util::*;

pub struct Boundary {
    pub node: NodeHold,
    pub offset: u32,
}

pub struct RangeData {
    pub start: RefCell<Boundary>,
    pub end: RefCell<Boundary>,
}

impl RangeData {
    fn page(&self) -> Rc<PageDom> {
        self.start.borrow().node.page.clone()
    }
    pub fn new(page: &Rc<PageDom>, node: NodeId) -> JsResult<RangeData> {
        Ok(RangeData {
            start: RefCell::new(Boundary { node: NodeHold::new(page, node)?, offset: 0 }),
            end: RefCell::new(Boundary { node: NodeHold::new(page, node)?, offset: 0 }),
        })
    }
}

fn node_length(page: &PageDom, n: NodeId) -> JsResult<u32> {
    let dom = page.dom.borrow();
    Ok(match dom.node_type(n).js()? {
        NodeType::DocumentType => 0,
        NodeType::Text | NodeType::Comment | NodeType::CDataSection | NodeType::ProcessingInstruction => {
            dom.char_length(n).js()?
        }
        _ => dom.child_count(n).js()?,
    })
}

/// Compares boundary points (DOM "position of a boundary point"): -1, 0, 1.
fn compare_points(page: &PageDom, a: (NodeId, u32), b: (NodeId, u32)) -> JsResult<i8> {
    if a.0 == b.0 {
        return Ok(a.1.cmp(&b.1) as i8);
    }
    let dom = page.dom.borrow();
    let pos = dom.compare_document_position(a.0, b.0).js()?;
    if pos & position::FOLLOWING != 0 && pos & position::CONTAINED_BY == 0 {
        return Ok(-compare_points_rev(&dom, b, a)?);
    }
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
        drop(dom);
        return Ok(-compare_points(page, b, a)?);
    }
    Ok(if pos & position::PRECEDING != 0 { 1 } else { -1 })
}

fn compare_points_rev(dom: &lp_dom::Dom, a: (NodeId, u32), b: (NodeId, u32)) -> JsResult<i8> {
    // a follows b without containment => a after b.
    let _ = (dom, a, b);
    Ok(1)
}

fn set_boundary(r: &RangeData, s: &Scope<'_>, a: &Args<'_>, start: bool) -> JsResult<()> {
    a.require(2, if start { "Range.setStart" } else { "Range.setEnd" })?;
    let node = node_arg(s, a.get(0), "Range")?;
    let offset = to_u32(a.get(1))?;
    set_point(r, node, offset, start)
}

fn set_point(r: &RangeData, node: NodeId, offset: u32, start: bool) -> JsResult<()> {
    let page = r.page();
    if page.dom.borrow().node_type(node).js()? == NodeType::DocumentType {
        return Err(JsError::dom("InvalidNodeTypeError", "The node provided is a doctype."));
    }
    if offset > node_length(&page, node)? {
        return Err(JsError::dom("IndexSizeError", format!("There is no child at offset {offset}.")));
    }
    let b = Boundary { node: NodeHold::new(&page, node)?, offset };
    if start {
        *r.start.borrow_mut() = b;
        let (sn, so, en, eo) = points(r);
        if compare_points(&page, (sn, so), (en, eo))? > 0 {
            *r.end.borrow_mut() = Boundary { node: NodeHold::new(&page, sn)?, offset: so };
        }
    } else {
        *r.end.borrow_mut() = b;
        let (sn, so, en, eo) = points(r);
        if compare_points(&page, (sn, so), (en, eo))? > 0 {
            *r.start.borrow_mut() = Boundary { node: NodeHold::new(&page, en)?, offset: eo };
        }
    }
    Ok(())
}

fn points(r: &RangeData) -> (NodeId, u32, NodeId, u32) {
    let s = r.start.borrow();
    let e = r.end.borrow();
    (s.node.id, s.offset, e.node.id, e.offset)
}

fn before_after(r: &RangeData, s: &Scope<'_>, a: &Args<'_>, start: bool, after: bool) -> JsResult<()> {
    let node = node_arg(s, a.get(0), "Range")?;
    let page = r.page();
    let (parent, idx) = {
        let dom = page.dom.borrow();
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

fn unsupported<'s>(what: &str) -> JsResult<Value<'s>> {
    Err(JsError::dom("NotSupportedError", format!("Range.{what} is not supported by lightningpanda yet")))
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
            let dom = page.dom.borrow();
            let mut cur = Some(sn);
            while let Some(c) = cur {
                if dom.contains(c, en).js()? {
                    drop(dom);
                    return Ok(wrap_node(s, &page, c)?.value());
                }
                cur = dom.parent(c).js()?;
            }
            Ok(s.null())
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
            let node = node_arg(s, a.get(0), "Range.selectNodeContents")?;
            let len = node_length(&r.page(), node)?;
            set_point(r, node, 0, true)?;
            set_point(r, node, len, false)?;
            Ok(s.undefined())
        });
        def.method("collapse", 0, |s, r, a| {
            let (sn, so, en, eo) = points(r);
            let page = r.page();
            if a.get(0).to_bool() {
                *r.end.borrow_mut() = Boundary { node: NodeHold::new(&page, sn)?, offset: so };
            } else {
                *r.start.borrow_mut() = Boundary { node: NodeHold::new(&page, en)?, offset: eo };
            }
            Ok(s.undefined())
        });
        def.method("cloneRange", 0, |s, r, _a| {
            let page = r.page();
            let (sn, so, en, eo) = points(r);
            let d = RangeData {
                start: RefCell::new(Boundary { node: NodeHold::new(&page, sn)?, offset: so }),
                end: RefCell::new(Boundary { node: NodeHold::new(&page, en)?, offset: eo }),
            };
            Ok(s.wrap::<Range>(d, None)?.value())
        });
        def.method("toString", 0, |s, r, _a| Ok(s.string(&range_text(r)?)));
        def.method("detach", 0, |s, _r, _a| Ok(s.undefined()));
        def.method("compareBoundaryPoints", 2, |s, r, a| {
            let how = to_u32(a.get(0))?;
            let other = s
                .unwrap::<Range>(a.get(1))
                .ok_or_else(|| JsError::Type("parameter 2 is not of type 'Range'.".into()))?;
            let (sn, so, en, eo) = points(r);
            let (osn, oso, oen, oeo) = points(&other);
            let (x, y) = match how {
                0 => ((sn, so), (osn, oso)),
                1 => ((en, eo), (osn, oso)),
                2 => ((en, eo), (oen, oeo)),
                3 => ((sn, so), (oen, oeo)),
                _ => return Err(JsError::dom("NotSupportedError", "The comparison method provided is invalid.")),
            };
            Ok(s.number(compare_points(&r.page(), x, y)? as f64))
        });
        def.method("comparePoint", 2, |s, r, a| {
            let node = node_arg(s, a.get(0), "Range.comparePoint")?;
            let off = to_u32(a.get(1))?;
            let (sn, so, en, eo) = points(r);
            let page = r.page();
            if compare_points(&page, (node, off), (sn, so))? < 0 {
                return Ok(s.number(-1.0));
            }
            if compare_points(&page, (node, off), (en, eo))? > 0 {
                return Ok(s.number(1.0));
            }
            Ok(s.number(0.0))
        });
        def.method("isPointInRange", 2, |s, r, a| {
            let node = node_arg(s, a.get(0), "Range.isPointInRange")?;
            let off = to_u32(a.get(1))?;
            let (sn, so, en, eo) = points(r);
            let page = r.page();
            Ok(s.boolean(
                compare_points(&page, (node, off), (sn, so))? >= 0
                    && compare_points(&page, (node, off), (en, eo))? <= 0,
            ))
        });
        def.method("intersectsNode", 1, |s, r, a| {
            let node = node_arg(s, a.get(0), "Range.intersectsNode")?;
            let page = r.page();
            let parent_idx = {
                let dom = page.dom.borrow();
                match dom.parent(node).js()? {
                    Some(p) => Some((p, dom.index_in_parent(node).js()?)),
                    None => None,
                }
            };
            let Some((p, i)) = parent_idx else { return Ok(s.boolean(true)) };
            let (sn, so, en, eo) = points(r);
            Ok(s.boolean(
                compare_points(&page, (p, i), (en, eo))? < 0 && compare_points(&page, (p, i + 1), (sn, so))? > 0,
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
        def.method("getBoundingClientRect", 0, |s, r, _a| crate::layout_api::range_rect(s, &r.page(), points(r).0));
        def.method("getClientRects", 0, |s, r, _a| {
            let rect = crate::layout_api::range_rect(s, &r.page(), points(r).0)?;
            Ok(s.array(&[rect])?.value())
        });
        def.method("deleteContents", 0, |_s, _r, _a| unsupported("deleteContents"));
        def.method("extractContents", 0, |_s, _r, _a| unsupported("extractContents"));
        def.method("cloneContents", 0, |_s, _r, _a| unsupported("cloneContents"));
        def.method("insertNode", 1, |s, r, a| {
            let node = node_arg(s, a.get(0), "Range.insertNode")?;
            let page = r.page();
            let (sn, so, _, _) = points(r);
            let mut dom = page.dom.borrow_mut();
            match dom.node_type(sn).js()? {
                NodeType::Text => {
                    let tail = dom.split_text(sn, so).js()?;
                    let parent =
                        dom.parent(sn).js()?.ok_or_else(|| JsError::dom("HierarchyRequestError", "detached"))?;
                    dom.insert_before(parent, node, Some(tail)).js()?;
                }
                NodeType::Comment | NodeType::ProcessingInstruction => {
                    return Err(JsError::dom("HierarchyRequestError", "Cannot insert into this node."));
                }
                _ => {
                    let child = dom.child_at(sn, so).js()?;
                    dom.insert_before(sn, node, child).js()?;
                }
            }
            Ok(s.undefined())
        });
        def.method("surroundContents", 1, |_s, _r, _a| unsupported("surroundContents"));
    }
}

pub(crate) fn new_range<'s>(s: &Scope<'s>, page: &Rc<PageDom>, doc: NodeId) -> JsResult<Value<'s>> {
    Ok(s.wrap::<Range>(RangeData::new(page, doc)?, None)?.value())
}

// ----- Selection ------------------------------------------------------------------

pub struct SelectionData {
    page: Rc<PageDom>,
    range: RefCell<Option<lp_js::Persistent>>,
}

pub struct Selection;
impl JsClass for Selection {
    const NAME: &'static str = "Selection";
    type Data = SelectionData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.getter("rangeCount", |s, sel| Ok(s.number(if sel.range.borrow().is_some() { 1.0 } else { 0.0 })));
        def.getter("type", |s, sel| {
            let t = match current(s, sel) {
                None => "None",
                Some(r) => {
                    let (a, b, c, d) = points(&r);
                    if a == c && b == d {
                        "Caret"
                    } else {
                        "Range"
                    }
                }
            };
            Ok(s.string(t))
        });
        def.getter("isCollapsed", |s, sel| {
            Ok(s.boolean(current(s, sel).is_none_or(|r| {
                let (a, b, c, d) = points(&r);
                a == c && b == d
            })))
        });
        def.getter("anchorNode", |s, sel| match current(s, sel) {
            Some(r) => Ok(wrap_node(s, &sel.page, points(&r).0)?.value()),
            None => Ok(s.null()),
        });
        def.getter("anchorOffset", |s, sel| Ok(s.number(current(s, sel).map_or(0, |r| points(&r).1) as f64)));
        def.getter("focusNode", |s, sel| match current(s, sel) {
            Some(r) => Ok(wrap_node(s, &sel.page, points(&r).2)?.value()),
            None => Ok(s.null()),
        });
        def.getter("focusOffset", |s, sel| Ok(s.number(current(s, sel).map_or(0, |r| points(&r).3) as f64)));
        def.method("getRangeAt", 1, |s, sel, a| {
            if to_u32(a.get(0))? != 0 || sel.range.borrow().is_none() {
                return Err(JsError::dom("IndexSizeError", "The index provided is not a valid range index."));
            }
            Ok(sel.range.borrow().as_ref().unwrap().get(s))
        });
        def.method("addRange", 1, |s, sel, a| {
            if s.unwrap::<Range>(a.get(0)).is_none() {
                return Err(JsError::Type("parameter 1 is not of type 'Range'.".into()));
            }
            if sel.range.borrow().is_none() {
                *sel.range.borrow_mut() = Some(lp_js::Persistent::new(s, a.get(0)));
            }
            Ok(s.undefined())
        });
        def.method("removeAllRanges", 0, |s, sel, _a| {
            *sel.range.borrow_mut() = None;
            Ok(s.undefined())
        });
        def.method("empty", 0, |s, sel, _a| {
            *sel.range.borrow_mut() = None;
            Ok(s.undefined())
        });
        def.method("removeRange", 1, |s, sel, _a| {
            *sel.range.borrow_mut() = None;
            Ok(s.undefined())
        });
        def.method("collapse", 1, |s, sel, a| {
            if a.get(0).is_null() {
                *sel.range.borrow_mut() = None;
                return Ok(s.undefined());
            }
            let node = node_arg(s, a.get(0), "Selection.collapse")?;
            let off = to_u32(a.get(1))?;
            let r = RangeData::new(&sel.page, node)?;
            set_point(&r, node, off, true)?;
            set_point(&r, node, off, false)?;
            let obj = s.wrap::<Range>(r, None)?;
            *sel.range.borrow_mut() = Some(lp_js::Persistent::new(s, obj.value()));
            Ok(s.undefined())
        });
        def.method("selectAllChildren", 1, |s, sel, a| {
            let node = node_arg(s, a.get(0), "Selection.selectAllChildren")?;
            let len = node_length(&sel.page, node)?;
            let r = RangeData::new(&sel.page, node)?;
            set_point(&r, node, len, false)?;
            let obj = s.wrap::<Range>(r, None)?;
            *sel.range.borrow_mut() = Some(lp_js::Persistent::new(s, obj.value()));
            Ok(s.undefined())
        });
        def.method("setBaseAndExtent", 4, |s, sel, a| {
            let an = node_arg(s, a.get(0), "Selection.setBaseAndExtent")?;
            let ao = to_u32(a.get(1))?;
            let fnode = node_arg(s, a.get(2), "Selection.setBaseAndExtent")?;
            let fo = to_u32(a.get(3))?;
            let r = RangeData::new(&sel.page, an)?;
            set_point(&r, an, ao, true)?;
            set_point(&r, fnode, fo, false)?;
            let obj = s.wrap::<Range>(r, None)?;
            *sel.range.borrow_mut() = Some(lp_js::Persistent::new(s, obj.value()));
            Ok(s.undefined())
        });
        def.method("containsNode", 1, |s, sel, a| {
            let node = node_arg(s, a.get(0), "Selection.containsNode")?;
            let Some(r) = current(s, sel) else { return Ok(s.boolean(false)) };
            let (sn, _, en, _) = points(&r);
            let dom = sel.page.dom.borrow();
            Ok(s.boolean(dom.contains(sn, node).js()? || dom.contains(en, node).js()?))
        });
        def.method("toString", 0, |s, sel, _a| match current(s, sel) {
            Some(r) => Ok(s.string(&range_text(&r)?)),
            None => Ok(s.string("")),
        });
        def.method("deleteFromDocument", 0, |_s, _sel, _a| unsupported("deleteFromDocument"));
        def.method("modify", 0, |s, _sel, _a| Ok(s.undefined()));
        def.method("extend", 1, |s, sel, a| {
            let node = node_arg(s, a.get(0), "Selection.extend")?;
            let off = to_u32(a.get(1))?;
            if let Some(r) = current(s, sel) {
                set_point(&r, node, off, false)?;
            }
            Ok(s.undefined())
        });
    }
}

fn current<'s>(s: &Scope<'s>, sel: &SelectionData) -> Option<impl std::ops::Deref<Target = RangeData> + 's> {
    let v = sel.range.borrow().as_ref()?.get(s);
    s.unwrap::<Range>(v)
}

/// `getSelection()`: one Selection per page per realm.
pub fn get_selection<'s>(s: &Scope<'s>, page: &Rc<PageDom>) -> JsResult<Value<'s>> {
    let key = lp_js::IdentityKey(page.id() | (1 << 59), 0);
    if let Some(o) = s.lookup_identity(key) {
        return Ok(o.value());
    }
    Ok(s.wrap::<Selection>(SelectionData { page: page.clone(), range: RefCell::new(None) }, Some(key))?.value())
}

pub(crate) fn register(engine: &lp_js::Engine) {
    engine.register::<Range>();
    engine.register::<Selection>();
}
