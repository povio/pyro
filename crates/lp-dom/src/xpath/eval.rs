//! XPath 1.0 evaluator and core function library, ported from upstream
//! src/browser/xpath/Evaluator.zig, functions.zig and result.zig.
//!
//! Step results are produced per context node in axis order so positional
//! predicates on reverse axes count by proximity, then turned into document
//! order. Merging the results of several context nodes sorts and dedups
//! them; large sets use pre-order indexes built once per tree and
//! evaluation, so deep trees never pay per-comparison ancestor walks.

use std::borrow::Cow;
use std::cell::RefCell;
use std::cmp::Ordering;

use markup5ever::ns;
use rustc_hash::{FxHashMap, FxHashSet};

use super::parser::{ArithOp, Axis, CmpOp, Expr, Func, NodeTest, PathStart, Step};
use super::tokenizer::is_xml_space;
use super::{XPathError, XPathNode, XPathValue};
use crate::dom::Dom;
use crate::node::{DocumentKind, NodeKind, NIL};

/// Internal node reference: slot index, or (owner element slot, attribute
/// index).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum XN {
    N(u32),
    A(u32, u32),
}

impl XN {
    fn owner(self) -> u32 {
        match self {
            XN::N(i) | XN::A(i, _) => i,
        }
    }
}

enum Val {
    Nodes(Vec<XN>),
    Num(f64),
    Str(String),
    Bool(bool),
}

#[derive(Copy, Clone)]
struct Ctx {
    node: XN,
    pos: usize,
    size: usize,
}

/// Sets larger than this are sorted with pre-order indexes.
const PAIRWISE_SORT_MAX: usize = 32;

struct Eval<'d> {
    dom: &'d Dom,
    source: &'d str,
    /// slot -> (tree root, pre-order index), filled per tree on demand.
    order: RefCell<FxHashMap<u32, (u32, u32)>>,
    indexed_roots: RefCell<FxHashSet<u32>>,
}

pub(crate) fn to_internal(dom: &Dom, n: XPathNode) -> Result<XN, XPathError> {
    match n {
        XPathNode::Node(id) => dom.check(id).map(XN::N).map_err(|_| XPathError::StaleNode),
        XPathNode::Attr { element, index } => {
            let i = dom.check(element).map_err(|_| XPathError::StaleNode)?;
            let count = dom.n(i).element().map_or(0, |e| e.attrs.len());
            if index >= count {
                return Err(XPathError::StaleNode);
            }
            Ok(XN::A(i, index as u32))
        }
    }
}

fn to_public(dom: &Dom, n: XN) -> XPathNode {
    match n {
        XN::N(i) => XPathNode::Node(dom.id_of(i)),
        XN::A(e, k) => XPathNode::Attr { element: dom.id_of(e), index: k as usize },
    }
}

pub(crate) fn evaluate(dom: &Dom, expr: &Expr, source: &str, context: XPathNode) -> Result<XPathValue, XPathError> {
    let node = to_internal(dom, context)?;
    let ev = Eval { dom, source, order: Default::default(), indexed_roots: Default::default() };
    Ok(match ev.eval(expr, Ctx { node, pos: 1, size: 1 })? {
        Val::Nodes(v) => XPathValue::NodeSet(v.into_iter().map(|n| to_public(dom, n)).collect()),
        Val::Num(n) => XPathValue::Number(n),
        Val::Str(s) => XPathValue::String(s),
        Val::Bool(b) => XPathValue::Boolean(b),
    })
}

/// XPath §5 string-value.
pub(crate) fn string_value(dom: &Dom, n: XN) -> Cow<'_, str> {
    match n {
        XN::A(e, k) => Cow::Borrowed(dom.n(e).element().map_or("", |el| &el.attrs[k as usize].value)),
        XN::N(i) => match &dom.n(i).kind {
            NodeKind::Element(_) | NodeKind::Document(_) | NodeKind::Fragment(_) => {
                let mut out = String::new();
                dom.collect_text(i, &mut out);
                Cow::Owned(out)
            }
            NodeKind::Doctype(_) => Cow::Borrowed(""),
            k => Cow::Borrowed(k.char_data().unwrap_or("")),
        },
    }
}

/// XPath §4.4 `number()` of a string: optional whitespace, optional minus,
/// digits with an optional fraction; anything else is NaN.
pub fn string_to_number(s: &str) -> f64 {
    let t = s.trim_matches(is_xml_space);
    let body = t.strip_prefix('-').unwrap_or(t);
    let mut parts = body.splitn(2, '.');
    let int = parts.next().unwrap_or("");
    let frac = parts.next();
    let digits = |p: &str| p.bytes().all(|b| b.is_ascii_digit());
    let valid = digits(int) && frac.is_none_or(digits) && !(int.is_empty() && frac.is_none_or(str::is_empty));
    if !valid {
        return f64::NAN;
    }
    t.parse().unwrap_or(f64::NAN)
}

/// XPath §4.2 `string()` of a number: no exponent, integers without a
/// fraction, `NaN`/`Infinity`/`-Infinity`, both zeros as `0`.
pub fn number_to_string(n: f64) -> String {
    if n.is_nan() {
        "NaN".into()
    } else if n.is_infinite() {
        if n > 0.0 { "Infinity" } else { "-Infinity" }.into()
    } else if n == 0.0 {
        "0".into()
    } else {
        // Rust's shortest round-trip formatting never uses an exponent.
        format!("{n}")
    }
}

fn to_bool(v: &Val) -> bool {
    match v {
        Val::Nodes(n) => !n.is_empty(),
        Val::Num(n) => *n != 0.0 && !n.is_nan(),
        Val::Str(s) => !s.is_empty(),
        Val::Bool(b) => *b,
    }
}

/// Round half toward positive infinity, preserving -0 (XPath §4.4 `round`).
fn round(x: f64) -> f64 {
    if !x.is_finite() {
        return x;
    }
    let f = x.floor();
    let r = if x - f >= 0.5 { f + 1.0 } else { f };
    if r == 0.0 && x.is_sign_negative() {
        -0.0
    } else {
        r
    }
}

impl<'d> Eval<'d> {
    fn type_error(&self, what: &str) -> XPathError {
        XPathError::Type(format!("Failed to evaluate '{}': {what}", self.source))
    }

    fn eval(&self, e: &Expr, ctx: Ctx) -> Result<Val, XPathError> {
        Ok(match e {
            Expr::Number(n) => Val::Num(*n),
            Expr::Literal(s) => Val::Str(s.clone()),
            Expr::Var => Val::Str(String::new()),
            Expr::Neg(a) => Val::Num(-self.number(self.eval(a, ctx)?)),
            Expr::Or(a, b) => Val::Bool(to_bool(&self.eval(a, ctx)?) || to_bool(&self.eval(b, ctx)?)),
            Expr::And(a, b) => Val::Bool(to_bool(&self.eval(a, ctx)?) && to_bool(&self.eval(b, ctx)?)),
            Expr::Cmp(op, a, b) => {
                let l = self.eval(a, ctx)?;
                let r = self.eval(b, ctx)?;
                Val::Bool(self.compare(*op, &l, &r))
            }
            Expr::Arith(op, a, b) => {
                let l = self.number(self.eval(a, ctx)?);
                let r = self.number(self.eval(b, ctx)?);
                Val::Num(match op {
                    ArithOp::Add => l + r,
                    ArithOp::Sub => l - r,
                    ArithOp::Mul => l * r,
                    ArithOp::Div => l / r,
                    // Truncating remainder, like JS `%` (XPath §3.5).
                    ArithOp::Mod => l % r,
                })
            }
            Expr::Union(a, b) => {
                let mut l = self.nodes(self.eval(a, ctx)?, "the operands of '|' must be node-sets")?;
                let r = self.nodes(self.eval(b, ctx)?, "the operands of '|' must be node-sets")?;
                l.extend(r);
                self.sort_dedup(&mut l);
                Val::Nodes(l)
            }
            Expr::Filter(primary, preds) => {
                let mut nodes = self.nodes(self.eval(primary, ctx)?, "predicates apply only to node-sets")?;
                for p in preds {
                    nodes = self.filter(nodes, p)?;
                }
                Val::Nodes(nodes)
            }
            Expr::Path(start, steps) => {
                let mut nodes = match start {
                    PathStart::Context => vec![ctx.node],
                    PathStart::Root => vec![XN::N(self.dom.root_idx(ctx.node.owner()))],
                    PathStart::Expr(e) => self.nodes(self.eval(e, ctx)?, "a location step needs a node-set")?,
                };
                for step in steps {
                    nodes = self.step(&nodes, step)?;
                }
                Val::Nodes(nodes)
            }
            Expr::Call(f, args) => self.call(*f, args, ctx)?,
        })
    }

    fn nodes(&self, v: Val, what: &str) -> Result<Vec<XN>, XPathError> {
        match v {
            Val::Nodes(n) => Ok(n),
            _ => Err(self.type_error(what)),
        }
    }

    // ----- steps and predicates ------------------------------------------------

    fn step(&self, input: &[XN], step: &Step) -> Result<Vec<XN>, XPathError> {
        let mut out = Vec::new();
        for &c in input {
            let mut v = Vec::new();
            self.axis(c, step.axis, &step.test, &mut v);
            for p in &step.preds {
                v = self.filter(v, p)?;
            }
            if step.axis.is_reverse() {
                v.reverse();
            }
            out.extend(v);
        }
        if input.len() > 1 {
            self.sort_dedup(&mut out);
        }
        Ok(out)
    }

    /// Applies one predicate; positions follow the order of `nodes`.
    fn filter(&self, nodes: Vec<XN>, pred: &Expr) -> Result<Vec<XN>, XPathError> {
        let size = nodes.len();
        // `[n]` and `[last()]` select by index without evaluating per node.
        match pred {
            Expr::Number(k) => {
                let i = *k as usize;
                return Ok(if *k == i as f64 && i >= 1 && i <= size { vec![nodes[i - 1]] } else { Vec::new() });
            }
            Expr::Call(Func::Last, _) => return Ok(nodes.last().copied().into_iter().collect()),
            _ => {}
        }
        let mut out = Vec::new();
        for (i, n) in nodes.into_iter().enumerate() {
            let keep = match self.eval(pred, Ctx { node: n, pos: i + 1, size })? {
                Val::Num(x) => x == (i + 1) as f64,
                v => to_bool(&v),
            };
            if keep {
                out.push(n);
            }
        }
        Ok(out)
    }

    /// Pushes the nodes of `axis` from `c` that pass `test`, in axis order.
    fn axis(&self, c: XN, axis: Axis, test: &NodeTest, out: &mut Vec<XN>) {
        let dom = self.dom;
        let mut push = |i: u32| {
            if i != NIL && self.matches(i, test) {
                out.push(XN::N(i));
            }
        };
        match (c, axis) {
            (_, Axis::Namespace) => {}
            (XN::A(..), Axis::SelfOnly | Axis::DescendantOrSelf | Axis::AncestorOrSelf) => {
                // Name tests on these axes look for elements, never attributes.
                if matches!(test, NodeTest::Node) {
                    out.push(c);
                }
                if axis == Axis::AncestorOrSelf {
                    self.ancestors(c.owner(), test, out);
                }
            }
            (XN::A(e, _), Axis::Parent) => push(e),
            (XN::A(e, _), Axis::Ancestor) => self.ancestors(e, test, out),
            (XN::A(e, _), Axis::Following) => {
                // Attributes precede their element's children.
                let mut cur = dom.n(e).first_child;
                while cur != NIL {
                    push(cur);
                    cur = dom.following_in(cur, e);
                }
                self.following(e, test, out);
            }
            (XN::A(e, _), Axis::Preceding) => self.preceding(e, test, out),
            (XN::A(..), _) => {}
            (XN::N(i), Axis::SelfOnly) => push(i),
            (XN::N(i), Axis::Child) => {
                let mut cur = dom.n(i).first_child;
                while cur != NIL {
                    push(cur);
                    cur = dom.n(cur).next;
                }
            }
            (XN::N(i), Axis::Descendant | Axis::DescendantOrSelf) => {
                if axis == Axis::DescendantOrSelf {
                    push(i);
                }
                let mut cur = dom.n(i).first_child;
                while cur != NIL {
                    push(cur);
                    cur = dom.following_in(cur, i);
                }
            }
            (XN::N(i), Axis::Parent) => push(dom.n(i).parent),
            (XN::N(i), Axis::Ancestor) => self.ancestors(dom.n(i).parent, test, out),
            (XN::N(i), Axis::AncestorOrSelf) => self.ancestors(i, test, out),
            (XN::N(i), Axis::FollowingSibling) => {
                let mut cur = dom.n(i).next;
                while cur != NIL {
                    push(cur);
                    cur = dom.n(cur).next;
                }
            }
            (XN::N(i), Axis::PrecedingSibling) => {
                let mut cur = dom.n(i).prev;
                while cur != NIL {
                    push(cur);
                    cur = dom.n(cur).prev;
                }
            }
            (XN::N(i), Axis::Following) => self.following(i, test, out),
            (XN::N(i), Axis::Preceding) => self.preceding(i, test, out),
            (XN::N(i), Axis::Attribute) => {
                let Some(el) = dom.n(i).element() else { return };
                for k in 0..el.attrs.len() as u32 {
                    if self.attr_matches(XN::A(i, k), test) {
                        out.push(XN::A(i, k));
                    }
                }
            }
        }
    }

    /// `from` and its ancestors, nearest first (`from` may be NIL).
    fn ancestors(&self, from: u32, test: &NodeTest, out: &mut Vec<XN>) {
        let mut cur = from;
        while cur != NIL {
            if self.matches(cur, test) {
                out.push(XN::N(cur));
            }
            cur = self.dom.n(cur).parent;
        }
    }

    /// Nodes after `i` in document order, excluding its descendants.
    fn following(&self, i: u32, test: &NodeTest, out: &mut Vec<XN>) {
        let dom = self.dom;
        let mut cur = i;
        while cur != NIL {
            let mut sib = dom.n(cur).next;
            while sib != NIL {
                let mut d = sib;
                while d != NIL {
                    if self.matches(d, test) {
                        out.push(XN::N(d));
                    }
                    d = dom.following_in(d, sib);
                }
                sib = dom.n(sib).next;
            }
            cur = dom.n(cur).parent;
        }
    }

    /// Nodes before `i` in reverse document order, excluding its ancestors.
    fn preceding(&self, i: u32, test: &NodeTest, out: &mut Vec<XN>) {
        let dom = self.dom;
        let mut cur = i;
        while cur != NIL {
            let mut sib = dom.n(cur).prev;
            while sib != NIL {
                // Reverse pre-order of the subtree: deepest last descendant
                // first, ending at `sib` itself.
                let mut d = deepest_last(dom, sib);
                loop {
                    if self.matches(d, test) {
                        out.push(XN::N(d));
                    }
                    if d == sib {
                        break;
                    }
                    let p = dom.n(d).prev;
                    d = if p != NIL { deepest_last(dom, p) } else { dom.n(d).parent };
                }
                sib = dom.n(sib).prev;
            }
            cur = dom.n(cur).parent;
        }
    }

    // ----- node tests ------------------------------------------------------------------

    /// Node test against a tree node (principal node type: element).
    fn matches(&self, i: u32, test: &NodeTest) -> bool {
        let node = self.dom.n(i);
        match test {
            NodeTest::Node => !matches!(node.kind, NodeKind::Doctype(_)),
            NodeTest::Text => matches!(node.kind, NodeKind::Text(_) | NodeKind::CData(_)),
            NodeTest::Comment => matches!(node.kind, NodeKind::Comment(_)),
            NodeTest::Pi(target) => match &node.kind {
                NodeKind::ProcessingInstruction(pi) => target.as_deref().is_none_or(|t| *pi.target == *t),
                _ => false,
            },
            NodeTest::Name { prefix, ns, local } => {
                let Some(el) = node.element() else { return false };
                let html = el.is_html() && self.in_html_document(i);
                match (prefix, local) {
                    (None, None) => true,
                    (Some(_), None) => ns.as_deref() == Some(&*el.name.ns),
                    // Unprefixed names match HTML elements case-insensitively
                    // in HTML documents (HTML §"Interactions with XPath"); in
                    // XML documents they match null-namespace elements.
                    (None, Some(l)) => {
                        if html {
                            el.name.local.as_bytes().eq_ignore_ascii_case(l.as_bytes())
                        } else {
                            !self.in_html_document(i) && el.name.ns.is_empty() && *el.name.local == **l
                        }
                    }
                    (Some(_), Some(l)) => {
                        ns.as_deref() == Some(&*el.name.ns)
                            && if html {
                                el.name.local.as_bytes().eq_ignore_ascii_case(l.as_bytes())
                            } else {
                                *el.name.local == **l
                            }
                    }
                }
            }
        }
    }

    /// Node test against an attribute (principal node type: attribute).
    /// Namespace declarations (`xmlns`) are not attributes in XPath.
    fn attr_matches(&self, a: XN, test: &NodeTest) -> bool {
        let XN::A(e, k) = a else { return false };
        let Some(el) = self.dom.n(e).element() else { return false };
        let attr = &el.attrs[k as usize];
        if attr.name.ns == ns!(xmlns) {
            return false;
        }
        match test {
            NodeTest::Node => true,
            NodeTest::Text | NodeTest::Comment | NodeTest::Pi(_) => false,
            NodeTest::Name { prefix, ns, local } => match (prefix, local) {
                (None, None) => true,
                (Some(_), None) => ns.as_deref() == Some(&*attr.name.ns),
                (None, Some(l)) => {
                    attr.name.ns.is_empty()
                        && if el.is_html() && self.in_html_document(e) {
                            attr.name.local.as_bytes().eq_ignore_ascii_case(l.as_bytes())
                        } else {
                            *attr.name.local == **l
                        }
                }
                (Some(_), Some(l)) => ns.as_deref() == Some(&*attr.name.ns) && *attr.name.local == **l,
            },
        }
    }

    fn in_html_document(&self, i: u32) -> bool {
        let doc = self.dom.id_of(self.dom.n(i).owner_doc);
        self.dom.document_data(doc).is_ok_and(|d| d.kind == DocumentKind::Html)
    }

    // ----- document order ------------------------------------------------------------

    fn sort_dedup(&self, v: &mut Vec<XN>) {
        if v.len() <= 1 {
            return;
        }
        if v.len() <= PAIRWISE_SORT_MAX {
            v.sort_by(|&a, &b| self.cmp_order(a, b));
        } else {
            v.sort_by_cached_key(|&n| self.order_key(n));
        }
        v.dedup();
    }

    fn sub_index(n: XN) -> u32 {
        match n {
            XN::N(_) => 0,
            XN::A(_, k) => k + 1,
        }
    }

    fn cmp_order(&self, a: XN, b: XN) -> Ordering {
        let (oa, ob) = (a.owner(), b.owner());
        if oa == ob {
            return Self::sub_index(a).cmp(&Self::sub_index(b));
        }
        let (ra, rb) = (self.dom.root_idx(oa), self.dom.root_idx(ob));
        if ra != rb {
            return ra.cmp(&rb);
        }
        if self.dom.tree_order_before(oa, ob) {
            Ordering::Less
        } else {
            Ordering::Greater
        }
    }

    fn order_key(&self, n: XN) -> (u32, u32, u32) {
        let owner = n.owner();
        if let Some(&(root, pre)) = self.order.borrow().get(&owner) {
            return (root, pre, Self::sub_index(n));
        }
        let root = self.dom.root_idx(owner);
        if self.indexed_roots.borrow_mut().insert(root) {
            let mut order = self.order.borrow_mut();
            let mut cur = root;
            let mut k = 0;
            while cur != NIL {
                order.insert(cur, (root, k));
                k += 1;
                cur = self.dom.following_in(cur, root);
            }
        }
        let pre = self.order.borrow().get(&owner).map_or(0, |&(_, p)| p);
        (root, pre, Self::sub_index(n))
    }

    // ----- values ----------------------------------------------------------------------

    fn string_value(&self, n: XN) -> Cow<'d, str> {
        string_value(self.dom, n)
    }

    fn string(&self, v: Val) -> String {
        match v {
            Val::Str(s) => s,
            Val::Num(n) => number_to_string(n),
            Val::Bool(b) => b.to_string(),
            Val::Nodes(n) => n.first().map(|&n| self.string_value(n).into_owned()).unwrap_or_default(),
        }
    }

    fn number(&self, v: Val) -> f64 {
        match v {
            Val::Num(n) => n,
            Val::Bool(b) => f64::from(u8::from(b)),
            Val::Str(s) => string_to_number(&s),
            Val::Nodes(n) => n.first().map_or(f64::NAN, |&n| string_to_number(&self.string_value(n))),
        }
    }

    /// XPath §3.4 comparisons.
    fn compare(&self, op: CmpOp, l: &Val, r: &Val) -> bool {
        let eq_op = matches!(op, CmpOp::Eq | CmpOp::Neq);
        match (l, r) {
            (Val::Nodes(a), Val::Nodes(b)) => {
                let right: Vec<Cow<'_, str>> = b.iter().map(|&n| self.string_value(n)).collect();
                a.iter().any(|&n| {
                    let lv = self.string_value(n);
                    right.iter().any(|rv| {
                        if eq_op {
                            cmp_str(op, &lv, rv)
                        } else {
                            cmp_num(op, string_to_number(&lv), string_to_number(rv))
                        }
                    })
                })
            }
            (Val::Nodes(set), other) | (other, Val::Nodes(set)) => {
                let set_left = matches!(l, Val::Nodes(_));
                if let Val::Bool(b) = other {
                    let s = !set.is_empty();
                    let (x, y) = if set_left { (s, *b) } else { (*b, s) };
                    return cmp_num(op, f64::from(u8::from(x)), f64::from(u8::from(y)));
                }
                set.iter().any(|&n| {
                    let sv = self.string_value(n);
                    match other {
                        Val::Str(s) if eq_op => {
                            if set_left {
                                cmp_str(op, &sv, s)
                            } else {
                                cmp_str(op, s, &sv)
                            }
                        }
                        _ => {
                            let a = string_to_number(&sv);
                            let b = match other {
                                Val::Num(x) => *x,
                                Val::Str(s) => string_to_number(s),
                                _ => f64::NAN,
                            };
                            if set_left {
                                cmp_num(op, a, b)
                            } else {
                                cmp_num(op, b, a)
                            }
                        }
                    }
                })
            }
            _ if eq_op => {
                if matches!(l, Val::Bool(_)) || matches!(r, Val::Bool(_)) {
                    cmp_num(op, f64::from(u8::from(to_bool(l))), f64::from(u8::from(to_bool(r))))
                } else if matches!(l, Val::Num(_)) || matches!(r, Val::Num(_)) {
                    cmp_num(op, self.scalar_number(l), self.scalar_number(r))
                } else {
                    cmp_str(op, &self.scalar_string(l), &self.scalar_string(r))
                }
            }
            _ => cmp_num(op, self.scalar_number(l), self.scalar_number(r)),
        }
    }

    fn scalar_number(&self, v: &Val) -> f64 {
        match v {
            Val::Num(n) => *n,
            Val::Bool(b) => f64::from(u8::from(*b)),
            Val::Str(s) => string_to_number(s),
            Val::Nodes(_) => f64::NAN,
        }
    }

    fn scalar_string(&self, v: &Val) -> String {
        match v {
            Val::Str(s) => s.clone(),
            Val::Num(n) => number_to_string(*n),
            Val::Bool(b) => b.to_string(),
            Val::Nodes(_) => String::new(),
        }
    }

    // ----- core function library (XPath §4) ----------------------------------------

    fn call(&self, f: Func, args: &[Expr], ctx: Ctx) -> Result<Val, XPathError> {
        let arg = |i: usize| self.eval(&args[i], ctx);
        let str_arg = |i: usize| -> Result<String, XPathError> { Ok(self.string(arg(i)?)) };
        // Optional argument defaulting to the context node's string-value.
        let str_or_ctx = || -> Result<String, XPathError> {
            if args.is_empty() {
                Ok(self.string_value(ctx.node).into_owned())
            } else {
                str_arg(0)
            }
        };
        let node_arg = |what: &str| -> Result<Option<XN>, XPathError> {
            if args.is_empty() {
                return Ok(Some(ctx.node));
            }
            Ok(self.nodes(arg(0)?, what)?.first().copied())
        };
        Ok(match f {
            Func::Last => Val::Num(ctx.size as f64),
            Func::Position => Val::Num(ctx.pos as f64),
            Func::Count => Val::Num(self.nodes(arg(0)?, "count() needs a node-set")?.len() as f64),
            Func::Sum => {
                let nodes = self.nodes(arg(0)?, "sum() needs a node-set")?;
                Val::Num(nodes.iter().map(|&n| string_to_number(&self.string_value(n))).sum())
            }
            Func::Id => Val::Nodes(self.id(arg(0)?, ctx.node)),
            Func::LocalName => {
                Val::Str(node_arg("local-name() needs a node-set")?.map(|n| self.name_of(n, false)).unwrap_or_default())
            }
            Func::Name => {
                Val::Str(node_arg("name() needs a node-set")?.map(|n| self.name_of(n, true)).unwrap_or_default())
            }
            Func::NamespaceUri => {
                let n = node_arg("namespace-uri() needs a node-set")?;
                let dom = self.dom;
                Val::Str(match n {
                    Some(XN::N(i)) => dom.n(i).element().map(|e| e.name.ns.to_string()).unwrap_or_default(),
                    Some(XN::A(e, k)) => {
                        dom.n(e).element().map(|el| el.attrs[k as usize].name.ns.to_string()).unwrap_or_default()
                    }
                    None => String::new(),
                })
            }
            Func::String => Val::Str(str_or_ctx()?),
            Func::Concat => {
                let mut s = String::new();
                for i in 0..args.len() {
                    s.push_str(&str_arg(i)?);
                }
                Val::Str(s)
            }
            Func::StartsWith => Val::Bool(str_arg(0)?.starts_with(&str_arg(1)?)),
            Func::Contains => Val::Bool(str_arg(0)?.contains(&str_arg(1)?)),
            Func::SubstringBefore => {
                let (s, p) = (str_arg(0)?, str_arg(1)?);
                Val::Str(s.find(&p).map(|i| s[..i].to_owned()).unwrap_or_default())
            }
            Func::SubstringAfter => {
                let (s, p) = (str_arg(0)?, str_arg(1)?);
                Val::Str(s.find(&p).map(|i| s[i + p.len()..].to_owned()).unwrap_or_default())
            }
            Func::Substring => {
                let s = str_arg(0)?;
                let start = round(self.number(arg(1)?));
                let end = if args.len() > 2 { start + round(self.number(arg(2)?)) } else { f64::INFINITY };
                // Characters at 1-based position p with start <= p < end;
                // NaN bounds select nothing.
                let out = s
                    .chars()
                    .enumerate()
                    .filter(|&(i, _)| {
                        let p = (i + 1) as f64;
                        p >= start && p < end
                    })
                    .map(|(_, c)| c)
                    .collect();
                Val::Str(out)
            }
            Func::StringLength => Val::Num(str_or_ctx()?.chars().count() as f64),
            Func::NormalizeSpace => {
                Val::Str(str_or_ctx()?.split(is_xml_space).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" "))
            }
            Func::Translate => {
                let (s, from, to) = (str_arg(0)?, str_arg(1)?, str_arg(2)?);
                let from: Vec<char> = from.chars().collect();
                let to: Vec<char> = to.chars().collect();
                Val::Str(
                    s.chars()
                        .filter_map(|c| match from.iter().position(|&f| f == c) {
                            Some(k) => to.get(k).copied(),
                            None => Some(c),
                        })
                        .collect(),
                )
            }
            Func::Boolean => Val::Bool(to_bool(&arg(0)?)),
            Func::Not => Val::Bool(!to_bool(&arg(0)?)),
            Func::True => Val::Bool(true),
            Func::False => Val::Bool(false),
            Func::Lang => Val::Bool(self.lang(&str_arg(0)?, ctx.node)),
            Func::Number => Val::Num(if args.is_empty() {
                string_to_number(&self.string_value(ctx.node))
            } else {
                self.number(arg(0)?)
            }),
            Func::Floor => Val::Num(self.number(arg(0)?).floor()),
            Func::Ceiling => Val::Num(self.number(arg(0)?).ceil()),
            Func::Round => Val::Num(round(self.number(arg(0)?))),
        })
    }

    /// `id()`: whitespace-separated IDs (from every node's string-value for
    /// a node-set), looked up in the context node's tree.
    fn id(&self, v: Val, ctx: XN) -> Vec<XN> {
        let ids: Vec<String> = match v {
            Val::Nodes(n) => n.iter().map(|&n| self.string_value(n).into_owned()).collect(),
            v => vec![self.string(v)],
        };
        let root = self.dom.id_of(self.dom.root_idx(ctx.owner()));
        let mut out = Vec::new();
        for token in ids.iter().flat_map(|s| s.split(is_xml_space)).filter(|t| !t.is_empty()) {
            if let Ok(Some(el)) = self.dom.get_element_by_id(root, token) {
                out.push(XN::N(el.idx));
            }
        }
        self.sort_dedup(&mut out);
        out
    }

    /// `name()` (qualified) / `local-name()` of elements, attributes and
    /// processing instructions; empty for other nodes.
    fn name_of(&self, n: XN, qualified: bool) -> String {
        let q = |prefix: Option<&markup5ever::Prefix>, local: &str| match prefix {
            Some(p) if qualified && !p.is_empty() => format!("{p}:{local}"),
            _ => local.to_owned(),
        };
        match n {
            XN::A(e, k) => self.dom.n(e).element().map_or(String::new(), |el| {
                let a = &el.attrs[k as usize];
                q(a.name.prefix.as_ref(), &a.name.local)
            }),
            XN::N(i) => match &self.dom.n(i).kind {
                NodeKind::Element(el) => q(el.name.prefix.as_ref(), &el.name.local),
                NodeKind::ProcessingInstruction(pi) => pi.target.to_string(),
                _ => String::new(),
            },
        }
    }

    /// `lang()`: nearest `xml:lang` on the context node or an ancestor.
    fn lang(&self, want: &str, ctx: XN) -> bool {
        let mut cur = ctx.owner();
        while cur != NIL {
            if let Some(el) = self.dom.n(cur).element() {
                let found = el.attrs.iter().find(|a| {
                    (a.name.ns == ns!(xml) && &*a.name.local == "lang")
                        || (a.name.ns.is_empty() && &*a.name.local == "xml:lang")
                });
                if let Some(a) = found {
                    let lang = a.value.to_ascii_lowercase();
                    let want = want.to_ascii_lowercase();
                    return lang == want || lang.strip_prefix(&want).is_some_and(|r| r.starts_with('-'));
                }
            }
            cur = self.dom.n(cur).parent;
        }
        false
    }
}

fn deepest_last(dom: &Dom, mut i: u32) -> u32 {
    loop {
        let l = dom.n(i).last_child;
        if l == NIL {
            return i;
        }
        i = l;
    }
}

fn cmp_str(op: CmpOp, a: &str, b: &str) -> bool {
    match op {
        CmpOp::Eq => a == b,
        CmpOp::Neq => a != b,
        _ => cmp_num(op, string_to_number(a), string_to_number(b)),
    }
}

/// IEEE comparisons give XPath's NaN semantics (NaN equals nothing).
fn cmp_num(op: CmpOp, a: f64, b: f64) -> bool {
    match op {
        CmpOp::Eq => a == b,
        CmpOp::Neq => a != b,
        CmpOp::Lt => a < b,
        CmpOp::Lte => a <= b,
        CmpOp::Gt => a > b,
        CmpOp::Gte => a >= b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_conversions() {
        assert_eq!(string_to_number(" 42 "), 42.0);
        assert_eq!(string_to_number("-1.5"), -1.5);
        assert_eq!(string_to_number(".5"), 0.5);
        assert_eq!(string_to_number("5."), 5.0);
        for bad in ["", "  ", "abc", "+1", "1e3", "Infinity", "0x10", "-", ".", "1 2"] {
            assert!(string_to_number(bad).is_nan(), "{bad:?}");
        }
        assert_eq!(number_to_string(5.0), "5");
        assert_eq!(number_to_string(-0.0), "0");
        assert_eq!(number_to_string(0.5), "0.5");
        assert_eq!(number_to_string(1e21), "1000000000000000000000");
        assert_eq!(number_to_string(1e-7), "0.0000001");
        assert_eq!(number_to_string(f64::NAN), "NaN");
        assert_eq!(number_to_string(f64::NEG_INFINITY), "-Infinity");
    }

    #[test]
    fn rounding() {
        assert_eq!(round(0.5), 1.0);
        assert_eq!(round(-0.5), 0.0);
        assert!(round(-0.5).is_sign_negative());
        assert_eq!(round(1.5), 2.0);
        assert_eq!(round(-1.5), -1.0);
        assert_eq!(round(2.4), 2.0);
        assert!(round(f64::NAN).is_nan());
        assert_eq!(round(f64::INFINITY), f64::INFINITY);
    }
}
