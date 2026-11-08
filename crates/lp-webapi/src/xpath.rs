//! XPathResult, XPathExpression, XPathEvaluator and the Document
//! `evaluate`/`createExpression`/`createNSResolver` mixin (DOM §8 XPath),
//! over the `lp_dom::xpath` engine.
//!
//! Ported from upstream src/browser/webapi/{XPathResult,XPathExpression,
//! XPathEvaluator}.zig and Document.zig. Differences from upstream, which
//! follow browsers instead:
//! - Namespace resolvers are honored (null, a function, or an object with
//!   `lookupNamespaceURI`); prefixes are resolved once when the expression
//!   is created, and an unresolvable prefix throws `NamespaceError`.
//! - Type mismatches (wrong accessor, node-set type for a scalar) throw
//!   TypeError, not `InvalidStateError`.
//! - Iterator results become invalid once the DOM changes (any tree,
//!   attribute or character-data mutation in the page's `Dom`, tracked by
//!   `Dom::tree_version`; form-control state, focus and hover do not
//!   count): `invalidIteratorState` turns true and `iterateNext()` throws
//!   `InvalidStateError`. Snapshots are unaffected.
//!
//! Kept from upstream: a null/undefined `contextNode` of
//! `document.evaluate` means the document; the `result` argument is
//! ignored (a new XPathResult is always returned); `createNSResolver(node)`
//! returns `node`. Results hold a reference on every node (like static
//! NodeLists), so nodes stay alive while the result is reachable.
//! An attribute result is its element's `Attr` node.

use std::cell::Cell;
use std::rc::Rc;

use lp_dom::xpath::{XPathError, XPathExpression as Compiled, XPathNode, XPathValue};
use lp_dom::{Dom, NodeId};
use lp_js::{Args, ClassDef, JsClass, JsError, JsResult, NoParent, Scope, Value};

use crate::collections::{attr_of, make_attr, AttrName, NodeHold};
use crate::node::{wrap_node, NodeData};
use crate::page::PageDom;
use crate::util::*;

pub const ANY_TYPE: u16 = 0;
pub const NUMBER_TYPE: u16 = 1;
pub const STRING_TYPE: u16 = 2;
pub const BOOLEAN_TYPE: u16 = 3;
pub const UNORDERED_NODE_ITERATOR_TYPE: u16 = 4;
pub const ORDERED_NODE_ITERATOR_TYPE: u16 = 5;
pub const UNORDERED_NODE_SNAPSHOT_TYPE: u16 = 6;
pub const ORDERED_NODE_SNAPSHOT_TYPE: u16 = 7;
pub const ANY_UNORDERED_NODE_TYPE: u16 = 8;
pub const FIRST_ORDERED_NODE_TYPE: u16 = 9;

fn xpath_err(e: XPathError) -> JsError {
    match e {
        XPathError::Syntax(m) => JsError::dom("SyntaxError", m),
        XPathError::Namespace(m) => JsError::dom("NamespaceError", m),
        XPathError::Type(m) => JsError::Type(m.into()),
        XPathError::StaleNode => JsError::dom("InvalidStateError", "The node is no longer available."),
    }
}

// ----- XPathResult --------------------------------------------------------------

/// A node in a result, held for the result's lifetime.
enum Item {
    Node(NodeHold),
    /// Owner element and attribute name.
    Attr(NodeHold, AttrName),
}

enum ResultValue {
    Number(f64),
    String(String),
    Boolean(bool),
    Nodes(Vec<Item>),
}

pub struct XPathResultData {
    page: Rc<PageDom>,
    kind: u16,
    value: ResultValue,
    cursor: Cell<usize>,
    /// `Dom::tree_version` when the result was created (iterator validity).
    version: u64,
}

impl XPathResultData {
    fn is_iterator(&self) -> bool {
        matches!(self.kind, UNORDERED_NODE_ITERATOR_TYPE | ORDERED_NODE_ITERATOR_TYPE)
    }
    fn is_snapshot(&self) -> bool {
        matches!(self.kind, UNORDERED_NODE_SNAPSHOT_TYPE | ORDERED_NODE_SNAPSHOT_TYPE)
    }
    fn invalid(&self) -> bool {
        self.is_iterator() && self.page.dom.borrow().tree_version() != self.version
    }
    fn items(&self) -> &[Item] {
        match &self.value {
            ResultValue::Nodes(v) => v,
            _ => &[],
        }
    }
    fn item<'s>(&self, s: &Scope<'s>, i: usize) -> JsResult<Value<'s>> {
        match self.items().get(i) {
            Some(Item::Node(h)) => Ok(wrap_node(s, &self.page, h.id)?.value()),
            Some(Item::Attr(h, n)) => make_attr(s, &self.page, h.id, n.clone()),
            None => Ok(s.null()),
        }
    }
}

fn mismatch(what: &str) -> JsError {
    JsError::Type(format!("The result type is not {what}.").into())
}

pub struct XPathResult;
impl JsClass for XPathResult {
    const NAME: &'static str = "XPathResult";
    type Data = XPathResultData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        for (name, v) in [
            ("ANY_TYPE", ANY_TYPE),
            ("NUMBER_TYPE", NUMBER_TYPE),
            ("STRING_TYPE", STRING_TYPE),
            ("BOOLEAN_TYPE", BOOLEAN_TYPE),
            ("UNORDERED_NODE_ITERATOR_TYPE", UNORDERED_NODE_ITERATOR_TYPE),
            ("ORDERED_NODE_ITERATOR_TYPE", ORDERED_NODE_ITERATOR_TYPE),
            ("UNORDERED_NODE_SNAPSHOT_TYPE", UNORDERED_NODE_SNAPSHOT_TYPE),
            ("ORDERED_NODE_SNAPSHOT_TYPE", ORDERED_NODE_SNAPSHOT_TYPE),
            ("ANY_UNORDERED_NODE_TYPE", ANY_UNORDERED_NODE_TYPE),
            ("FIRST_ORDERED_NODE_TYPE", FIRST_ORDERED_NODE_TYPE),
        ] {
            def.constant(name, f64::from(v));
        }
        def.getter("resultType", |s, r| Ok(s.number(f64::from(r.kind))));
        def.getter("numberValue", |s, r| match r.value {
            ResultValue::Number(n) => Ok(s.number(n)),
            _ => Err(mismatch("a number")),
        });
        def.getter("stringValue", |s, r| match &r.value {
            ResultValue::String(v) => Ok(s.string(v)),
            _ => Err(mismatch("a string")),
        });
        def.getter("booleanValue", |s, r| match r.value {
            ResultValue::Boolean(b) => Ok(s.boolean(b)),
            _ => Err(mismatch("a boolean")),
        });
        def.getter("singleNodeValue", |s, r| {
            if !matches!(r.kind, ANY_UNORDERED_NODE_TYPE | FIRST_ORDERED_NODE_TYPE) {
                return Err(mismatch("a single node"));
            }
            r.item(s, 0)
        });
        def.getter("invalidIteratorState", |s, r| Ok(s.boolean(r.invalid())));
        def.getter("snapshotLength", |s, r| {
            if !r.is_snapshot() {
                return Err(mismatch("a snapshot"));
            }
            Ok(s.number(r.items().len() as f64))
        });
        def.method("iterateNext", 0, |s, r, _a| {
            if !r.is_iterator() {
                return Err(mismatch("an iterator"));
            }
            if r.invalid() {
                return Err(JsError::dom(
                    "InvalidStateError",
                    "The document has mutated since the result was returned.",
                ));
            }
            let i = r.cursor.get();
            if i < r.items().len() {
                r.cursor.set(i + 1);
            }
            r.item(s, i)
        });
        def.pure_method("snapshotItem", 1, |s, r, a| {
            a.require(1, "XPathResult.snapshotItem")?;
            if !r.is_snapshot() {
                return Err(mismatch("a snapshot"));
            }
            r.item(s, to_u32(a.get(0))? as usize)
        });
    }
}

/// Converts an evaluation value to the requested result type (DOM §8
/// `evaluate` step "convert") and wraps it in an XPathResult.
fn make_result<'s>(s: &Scope<'s>, page: &Rc<PageDom>, value: XPathValue, requested: u16) -> JsResult<Value<'s>> {
    let is_set = matches!(value, XPathValue::NodeSet(_));
    let kind = match requested {
        ANY_TYPE => match value {
            XPathValue::Number(_) => NUMBER_TYPE,
            XPathValue::String(_) => STRING_TYPE,
            XPathValue::Boolean(_) => BOOLEAN_TYPE,
            XPathValue::NodeSet(_) => UNORDERED_NODE_ITERATOR_TYPE,
        },
        NUMBER_TYPE | STRING_TYPE | BOOLEAN_TYPE => requested,
        UNORDERED_NODE_ITERATOR_TYPE..=FIRST_ORDERED_NODE_TYPE if is_set => requested,
        UNORDERED_NODE_ITERATOR_TYPE..=FIRST_ORDERED_NODE_TYPE => {
            return Err(JsError::Type(
                "The result is not a node set, and therefore cannot be converted to the desired type.".into(),
            ))
        }
        _ => return Err(JsError::dom("NotSupportedError", format!("The result type '{requested}' is not supported."))),
    };
    // Convert while the DOM is borrowed; take node holds afterwards.
    let (version, scalar, targets) = {
        let dom = page.dom.borrow();
        let scalar = match kind {
            NUMBER_TYPE => Some(ResultValue::Number(value.to_number(&dom))),
            STRING_TYPE => Some(ResultValue::String(value.to_string(&dom))),
            BOOLEAN_TYPE => Some(ResultValue::Boolean(value.to_boolean())),
            _ => None,
        };
        let mut targets = Vec::new();
        if let (None, XPathValue::NodeSet(v)) = (&scalar, &value) {
            let keep = if matches!(kind, ANY_UNORDERED_NODE_TYPE | FIRST_ORDERED_NODE_TYPE) { 1 } else { v.len() };
            for n in v.iter().take(keep) {
                targets.push(target(&dom, *n)?);
            }
        }
        (dom.tree_version(), scalar, targets)
    };
    let value = match scalar {
        Some(v) => v,
        None => ResultValue::Nodes(
            targets
                .into_iter()
                .map(|(id, attr)| {
                    let h = NodeHold::new(page, id)?;
                    Ok(match attr {
                        Some(n) => Item::Attr(h, n),
                        None => Item::Node(h),
                    })
                })
                .collect::<JsResult<_>>()?,
        ),
    };
    let data = XPathResultData { page: page.clone(), kind, value, cursor: Cell::new(0), version };
    Ok(s.wrap::<XPathResult>(data, None)?.value())
}

/// Node to hold for a result item (attributes: owner element + name).
fn target(dom: &Dom, n: XPathNode) -> JsResult<(NodeId, Option<AttrName>)> {
    Ok(match n {
        XPathNode::Node(id) => (id, None),
        XPathNode::Attr { element, index } => {
            let name = dom.attributes(element).js()?.get(index).map(AttrName::of);
            (element, Some(name.ok_or_else(|| JsError::dom("InvalidStateError", "The attribute was removed."))?))
        }
    })
}

// ----- evaluation helpers -------------------------------------------------------

/// `contextNode`: a Node or an Attr (`None` for null/undefined).
fn context_arg(s: &Scope<'_>, v: Value<'_>, what: &str) -> JsResult<Option<(Rc<PageDom>, XPathNode)>> {
    if let Some(n) = s.unwrap::<crate::node::Node>(v) {
        let page = n.page();
        // An Attr is a Node; XPath addresses it through its element.
        if let Some((element, name)) = attr_of(&n) {
            let Some(element) = element else { return Ok(None) };
            let index = name.find(page.dom.borrow().attributes(element).js()?);
            let index = index.ok_or_else(|| JsError::dom("InvalidStateError", "The attribute was removed."))?;
            return Ok(Some((page.clone(), XPathNode::Attr { element, index })));
        }
        return Ok(Some((page.clone(), XPathNode::Node(n.id))));
    }
    if v.is_nullish() {
        return Ok(None);
    }
    Err(JsError::Type(format!("Failed to execute '{what}': parameter is not of type 'Node'.").into()))
}

fn required_context(s: &Scope<'_>, v: Value<'_>, what: &str) -> JsResult<(Rc<PageDom>, XPathNode)> {
    context_arg(s, v, what)?
        .ok_or_else(|| JsError::Type(format!("Failed to execute '{what}': parameter is not of type 'Node'.").into()))
}

/// `unsigned short type` (undefined: ANY_TYPE).
fn result_type(v: Value<'_>) -> JsResult<u16> {
    if v.is_undefined() {
        return Ok(ANY_TYPE);
    }
    Ok((to_u32(v)? & 0xFFFF) as u16)
}

/// Parses `source` and binds its prefixes through `resolver`: null, a
/// function, or an object with `lookupNamespaceURI` (e.g. a Node).
fn compile(s: &Scope<'_>, source: &str, resolver: Value<'_>) -> JsResult<Compiled> {
    let mut expr = Compiled::parse(source).map_err(xpath_err)?;
    let prefixes = expr.prefixes();
    if prefixes.is_empty() {
        return Ok(expr);
    }
    let mut bound: Vec<(String, Option<String>)> = Vec::new();
    if !resolver.is_nullish() {
        let not_resolver = || JsError::Type("The resolver is not an XPathNSResolver.".into());
        let obj = resolver.as_object().ok_or_else(not_resolver)?;
        for p in prefixes {
            let arg = s.string(&p);
            let uri = if obj.is_function() {
                obj.call(None, &[arg])?
            } else {
                let f = obj.get_atom("lookupNamespaceURI")?;
                let f = f.as_object().filter(|f| f.is_function()).ok_or_else(not_resolver)?;
                f.call(Some(obj), &[arg])?
            };
            bound.push((p, opt_dom_string(uri)?));
        }
    }
    expr.resolve_namespaces(|p| bound.iter().find(|(k, _)| k == p).and_then(|(_, v)| v.clone())).map_err(xpath_err)?;
    Ok(expr)
}

fn run<'s>(
    s: &Scope<'s>,
    expr: &Compiled,
    (page, context): (Rc<PageDom>, XPathNode),
    requested: u16,
) -> JsResult<Value<'s>> {
    let value = expr.evaluate(&page.dom.borrow(), context).map_err(xpath_err)?;
    make_result(s, &page, value, requested)
}

fn create_expression<'s>(s: &Scope<'s>, a: &Args<'s>, what: &str) -> JsResult<Value<'s>> {
    let source = arg_string(a, 0, what)?;
    let expr = compile(s, &source, a.get(1))?;
    Ok(s.wrap::<XPathExpression>(XPathExpressionData { expr }, None)?.value())
}

fn create_ns_resolver<'s>(s: &Scope<'s>, a: &Args<'s>, what: &str) -> JsResult<Value<'s>> {
    crate::node::node_arg(s, a.get(0), what)?;
    Ok(a.get(0))
}

// ----- Document mixin ---------------------------------------------------------------

/// `document.evaluate(expression, contextNode, resolver, type, result)`.
pub(crate) fn document_evaluate<'s>(s: &Scope<'s>, n: &NodeData, a: Args<'s>) -> JsResult<Value<'s>> {
    // Argument conversions (which may run JS: resolver, valueOf) happen
    // before the context node is resolved to a DOM position.
    let source = arg_string(&a, 0, "Document.evaluate")?;
    let expr = compile(s, &source, a.get(2))?;
    let ty = result_type(a.get(3))?;
    let context =
        context_arg(s, a.get(1), "Document.evaluate")?.unwrap_or_else(|| (n.page().clone(), XPathNode::Node(n.id)));
    run(s, &expr, context, ty)
}

pub(crate) fn document_create_expression<'s>(s: &Scope<'s>, _n: &NodeData, a: Args<'s>) -> JsResult<Value<'s>> {
    create_expression(s, &a, "Document.createExpression")
}

pub(crate) fn document_create_ns_resolver<'s>(s: &Scope<'s>, _n: &NodeData, a: Args<'s>) -> JsResult<Value<'s>> {
    create_ns_resolver(s, &a, "Document.createNSResolver")
}

// ----- XPathExpression / XPathEvaluator ---------------------------------------------

pub struct XPathExpressionData {
    expr: Compiled,
}

pub struct XPathExpression;
impl JsClass for XPathExpression {
    const NAME: &'static str = "XPathExpression";
    type Data = XPathExpressionData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.method("evaluate", 1, |s, e, a| {
            let ty = result_type(a.get(1))?;
            let context = required_context(s, a.get(0), "XPathExpression.evaluate")?;
            run(s, &e.expr, context, ty)
        });
    }
}

pub struct XPathEvaluator;
impl JsClass for XPathEvaluator {
    const NAME: &'static str = "XPathEvaluator";
    type Data = ();
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(0, |s, nt, _a| s.wrap_new::<XPathEvaluator>((), nt, None));
        def.method("evaluate", 2, |s, _e, a| {
            let source = arg_string(&a, 0, "XPathEvaluator.evaluate")?;
            let expr = compile(s, &source, a.get(2))?;
            let ty = result_type(a.get(3))?;
            let context = required_context(s, a.get(1), "XPathEvaluator.evaluate")?;
            run(s, &expr, context, ty)
        });
        def.method("createExpression", 1, |s, _e, a| create_expression(s, &a, "XPathEvaluator.createExpression"));
        def.method("createNSResolver", 1, |s, _e, a| create_ns_resolver(s, &a, "XPathEvaluator.createNSResolver"));
    }
}

pub(crate) fn register(engine: &lp_js::Engine) {
    engine.register::<XPathResult>();
    engine.register::<XPathExpression>();
    engine.register::<XPathEvaluator>();
}
