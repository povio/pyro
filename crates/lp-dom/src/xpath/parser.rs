//! XPath 1.0 AST and recursive-descent parser, ported from upstream
//! src/browser/xpath/Parser.zig and ast.zig.
//!
//! Differences from upstream (which keeps polyfill leniency): unknown axes,
//! unknown functions and wrong argument counts are syntax errors at parse
//! time (as in browsers), and QName node tests keep their prefix so it can
//! be resolved against a namespace resolver before evaluation.

use super::tokenizer::{tokenize, Tok};
use super::XPathError;

/// Recursion cap so adversarial input (`((((...))))`, `------1`) cannot
/// overflow the native stack.
const MAX_DEPTH: u32 = 100;

#[derive(Clone, Debug)]
pub(crate) enum Expr {
    Or(Box<Expr>, Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Cmp(CmpOp, Box<Expr>, Box<Expr>),
    Arith(ArithOp, Box<Expr>, Box<Expr>),
    Neg(Box<Expr>),
    Union(Box<Expr>, Box<Expr>),
    /// Location path, optionally rooted at a filter expression.
    Path(PathStart, Vec<Step>),
    /// `primary[pred]...` (predicates use document order).
    Filter(Box<Expr>, Vec<Expr>),
    Literal(String),
    Number(f64),
    /// Variable reference. The DOM API cannot bind variables; upstream
    /// evaluates them to the empty string and so do we.
    Var,
    Call(Func, Vec<Expr>),
}

#[derive(Clone, Debug)]
pub(crate) enum PathStart {
    /// Relative to the context node.
    Context,
    /// `/...`: the root of the context node's tree.
    Root,
    Expr(Box<Expr>),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum CmpOp {
    Eq,
    Neq,
    Lt,
    Lte,
    Gt,
    Gte,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum ArithOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Axis {
    Child,
    Descendant,
    DescendantOrSelf,
    SelfOnly,
    Parent,
    Ancestor,
    AncestorOrSelf,
    FollowingSibling,
    PrecedingSibling,
    Following,
    Preceding,
    Attribute,
    Namespace,
}

impl Axis {
    /// Reverse axes number their nodes in reverse document order.
    pub(crate) fn is_reverse(self) -> bool {
        matches!(self, Axis::Ancestor | Axis::AncestorOrSelf | Axis::Preceding | Axis::PrecedingSibling)
    }
}

#[derive(Clone, Debug)]
pub(crate) enum NodeTest {
    /// `node()`
    Node,
    /// `text()`
    Text,
    /// `comment()`
    Comment,
    /// `processing-instruction(target?)`
    Pi(Option<String>),
    /// `*`, `prefix:*`, `name`, `prefix:name`. `ns` is filled in by
    /// namespace resolution for prefixed tests.
    Name { prefix: Option<String>, ns: Option<String>, local: Option<String> },
}

#[derive(Clone, Debug)]
pub(crate) struct Step {
    pub axis: Axis,
    pub test: NodeTest,
    pub preds: Vec<Expr>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Func {
    Last,
    Position,
    Count,
    Id,
    LocalName,
    NamespaceUri,
    Name,
    String,
    Concat,
    StartsWith,
    Contains,
    SubstringBefore,
    SubstringAfter,
    Substring,
    StringLength,
    NormalizeSpace,
    Translate,
    Boolean,
    Not,
    True,
    False,
    Lang,
    Number,
    Sum,
    Floor,
    Ceiling,
    Round,
}

impl Func {
    fn lookup(name: &str) -> Option<(Func, usize, usize)> {
        use Func::*;
        // (function, min args, max args)
        Some(match name {
            "last" => (Last, 0, 0),
            "position" => (Position, 0, 0),
            "count" => (Count, 1, 1),
            "id" => (Id, 1, 1),
            "local-name" => (LocalName, 0, 1),
            "namespace-uri" => (NamespaceUri, 0, 1),
            "name" => (Name, 0, 1),
            "string" => (String, 0, 1),
            "concat" => (Concat, 2, usize::MAX),
            "starts-with" => (StartsWith, 2, 2),
            "contains" => (Contains, 2, 2),
            "substring-before" => (SubstringBefore, 2, 2),
            "substring-after" => (SubstringAfter, 2, 2),
            "substring" => (Substring, 2, 3),
            "string-length" => (StringLength, 0, 1),
            "normalize-space" => (NormalizeSpace, 0, 1),
            "translate" => (Translate, 3, 3),
            "boolean" => (Boolean, 1, 1),
            "not" => (Not, 1, 1),
            "true" => (True, 0, 0),
            "false" => (False, 0, 0),
            "lang" => (Lang, 1, 1),
            "number" => (Number, 0, 1),
            "sum" => (Sum, 1, 1),
            "floor" => (Floor, 1, 1),
            "ceiling" => (Ceiling, 1, 1),
            "round" => (Round, 1, 1),
            _ => return None,
        })
    }

    fn returns_number(self) -> bool {
        use Func::*;
        matches!(self, Last | Position | Count | StringLength | Number | Sum | Floor | Ceiling | Round)
    }
}

fn axis_named(name: &str) -> Option<Axis> {
    Some(match name {
        "child" => Axis::Child,
        "descendant" => Axis::Descendant,
        "descendant-or-self" => Axis::DescendantOrSelf,
        "self" => Axis::SelfOnly,
        "parent" => Axis::Parent,
        "ancestor" => Axis::Ancestor,
        "ancestor-or-self" => Axis::AncestorOrSelf,
        "following-sibling" => Axis::FollowingSibling,
        "preceding-sibling" => Axis::PrecedingSibling,
        "following" => Axis::Following,
        "preceding" => Axis::Preceding,
        "attribute" => Axis::Attribute,
        "namespace" => Axis::Namespace,
        _ => return None,
    })
}

fn is_node_type(name: &str) -> bool {
    matches!(name, "node" | "text" | "comment" | "processing-instruction")
}

struct Parser<'a> {
    src: &'a str,
    toks: Vec<Tok>,
    pos: usize,
    depth: u32,
}

pub(crate) fn parse(src: &str) -> Result<Expr, XPathError> {
    let toks = tokenize(src)?;
    let mut p = Parser { src, toks, pos: 0, depth: 0 };
    let e = p.expr()?;
    if p.peek() != &Tok::Eof {
        return Err(p.err());
    }
    Ok(optimize(e))
}

impl Parser<'_> {
    fn err(&self) -> XPathError {
        XPathError::Syntax(format!("The string '{}' is not a valid XPath expression.", self.src))
    }
    fn peek(&self) -> &Tok {
        &self.toks[self.pos]
    }
    fn peek_at(&self, n: usize) -> &Tok {
        self.toks.get(self.pos + n).unwrap_or(&Tok::Eof)
    }
    fn bump(&mut self) -> Tok {
        let t = self.toks[self.pos].clone();
        if t != Tok::Eof {
            self.pos += 1;
        }
        t
    }
    fn eat(&mut self, t: &Tok) -> bool {
        if self.peek() == t {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    fn expect(&mut self, t: &Tok) -> Result<(), XPathError> {
        if self.eat(t) {
            Ok(())
        } else {
            Err(self.err())
        }
    }
    fn eat_keyword(&mut self, kw: &str) -> bool {
        if matches!(self.peek(), Tok::Name(n) if n == kw) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expr(&mut self) -> Result<Expr, XPathError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(self.err());
        }
        let r = self.or_expr();
        self.depth -= 1;
        r
    }

    fn or_expr(&mut self) -> Result<Expr, XPathError> {
        let mut l = self.and_expr()?;
        while self.eat_keyword("or") {
            l = Expr::Or(Box::new(l), Box::new(self.and_expr()?));
        }
        Ok(l)
    }

    fn and_expr(&mut self) -> Result<Expr, XPathError> {
        let mut l = self.equality()?;
        while self.eat_keyword("and") {
            l = Expr::And(Box::new(l), Box::new(self.equality()?));
        }
        Ok(l)
    }

    fn equality(&mut self) -> Result<Expr, XPathError> {
        let mut l = self.relational()?;
        loop {
            let op = match self.peek() {
                Tok::Eq => CmpOp::Eq,
                Tok::Neq => CmpOp::Neq,
                _ => return Ok(l),
            };
            self.pos += 1;
            l = Expr::Cmp(op, Box::new(l), Box::new(self.relational()?));
        }
    }

    fn relational(&mut self) -> Result<Expr, XPathError> {
        let mut l = self.additive()?;
        loop {
            let op = match self.peek() {
                Tok::Lt => CmpOp::Lt,
                Tok::Lte => CmpOp::Lte,
                Tok::Gt => CmpOp::Gt,
                Tok::Gte => CmpOp::Gte,
                _ => return Ok(l),
            };
            self.pos += 1;
            l = Expr::Cmp(op, Box::new(l), Box::new(self.additive()?));
        }
    }

    fn additive(&mut self) -> Result<Expr, XPathError> {
        let mut l = self.multiplicative()?;
        loop {
            let op = match self.peek() {
                Tok::Plus => ArithOp::Add,
                Tok::Minus => ArithOp::Sub,
                _ => return Ok(l),
            };
            self.pos += 1;
            l = Expr::Arith(op, Box::new(l), Box::new(self.multiplicative()?));
        }
    }

    /// After a complete operand, `*` is multiplication and `div`/`mod` are
    /// operator names (XPath §3.7 disambiguation).
    fn multiplicative(&mut self) -> Result<Expr, XPathError> {
        let mut l = self.unary()?;
        loop {
            let op = match self.peek() {
                Tok::Star => ArithOp::Mul,
                Tok::Name(n) if n == "div" => ArithOp::Div,
                Tok::Name(n) if n == "mod" => ArithOp::Mod,
                _ => return Ok(l),
            };
            self.pos += 1;
            l = Expr::Arith(op, Box::new(l), Box::new(self.unary()?));
        }
    }

    fn unary(&mut self) -> Result<Expr, XPathError> {
        if self.eat(&Tok::Minus) {
            self.depth += 1;
            if self.depth > MAX_DEPTH {
                return Err(self.err());
            }
            let e = self.unary();
            self.depth -= 1;
            return Ok(Expr::Neg(Box::new(e?)));
        }
        self.union()
    }

    fn union(&mut self) -> Result<Expr, XPathError> {
        let mut l = self.path_expr()?;
        while self.eat(&Tok::Pipe) {
            l = Expr::Union(Box::new(l), Box::new(self.path_expr()?));
        }
        Ok(l)
    }

    fn path_expr(&mut self) -> Result<Expr, XPathError> {
        let is_filter = match self.peek() {
            Tok::LParen | Tok::Literal(_) | Tok::Number(_) | Tok::Var(_) => true,
            Tok::Name(n) => self.peek_at(1) == &Tok::LParen && !is_node_type(n),
            _ => false,
        };
        if is_filter {
            let primary = self.primary()?;
            let mut preds = Vec::new();
            while self.eat(&Tok::LBracket) {
                preds.push(self.expr()?);
                self.expect(&Tok::RBracket)?;
            }
            let filter = if preds.is_empty() { primary } else { Expr::Filter(Box::new(primary), preds) };
            let mut steps = Vec::new();
            match self.peek() {
                Tok::Slash => {
                    self.pos += 1;
                    self.relative_into(&mut steps)?;
                }
                Tok::DoubleSlash => {
                    self.pos += 1;
                    steps.push(descendant_or_self());
                    self.relative_into(&mut steps)?;
                }
                _ => return Ok(filter),
            }
            return Ok(Expr::Path(PathStart::Expr(Box::new(filter)), steps));
        }
        let mut steps = Vec::new();
        match self.peek() {
            Tok::Slash => {
                self.pos += 1;
                if self.can_start_step() {
                    self.relative_into(&mut steps)?;
                }
                Ok(Expr::Path(PathStart::Root, steps))
            }
            Tok::DoubleSlash => {
                self.pos += 1;
                steps.push(descendant_or_self());
                self.relative_into(&mut steps)?;
                Ok(Expr::Path(PathStart::Root, steps))
            }
            _ => {
                self.relative_into(&mut steps)?;
                Ok(Expr::Path(PathStart::Context, steps))
            }
        }
    }

    fn can_start_step(&self) -> bool {
        matches!(self.peek(), Tok::Name(_) | Tok::PrefixStar(_) | Tok::Star | Tok::Dot | Tok::DoubleDot | Tok::At)
    }

    fn relative_into(&mut self, steps: &mut Vec<Step>) -> Result<(), XPathError> {
        steps.push(self.step()?);
        loop {
            match self.peek() {
                Tok::Slash => self.pos += 1,
                Tok::DoubleSlash => {
                    self.pos += 1;
                    steps.push(descendant_or_self());
                }
                _ => return Ok(()),
            }
            steps.push(self.step()?);
        }
    }

    fn step(&mut self) -> Result<Step, XPathError> {
        if self.eat(&Tok::Dot) {
            return Ok(Step { axis: Axis::SelfOnly, test: NodeTest::Node, preds: Vec::new() });
        }
        if self.eat(&Tok::DoubleDot) {
            return Ok(Step { axis: Axis::Parent, test: NodeTest::Node, preds: Vec::new() });
        }
        let mut axis = Axis::Child;
        if self.eat(&Tok::At) {
            axis = Axis::Attribute;
        } else if let (Tok::Name(n), Tok::DoubleColon) = (self.peek(), self.peek_at(1)) {
            axis = axis_named(n).ok_or_else(|| self.err())?;
            self.pos += 2;
        }
        let test = self.node_test()?;
        let mut preds = Vec::new();
        while self.eat(&Tok::LBracket) {
            preds.push(self.expr()?);
            self.expect(&Tok::RBracket)?;
        }
        Ok(Step { axis, test, preds })
    }

    fn node_test(&mut self) -> Result<NodeTest, XPathError> {
        match self.bump() {
            Tok::Star => Ok(NodeTest::Name { prefix: None, ns: None, local: None }),
            Tok::PrefixStar(p) => Ok(NodeTest::Name { prefix: Some(p), ns: None, local: None }),
            Tok::Name(n) => {
                if is_node_type(&n) && self.peek() == &Tok::LParen {
                    self.pos += 1;
                    let test = match n.as_str() {
                        "node" => NodeTest::Node,
                        "text" => NodeTest::Text,
                        "comment" => NodeTest::Comment,
                        _ => match self.peek().clone() {
                            Tok::Literal(target) => {
                                self.pos += 1;
                                NodeTest::Pi(Some(target))
                            }
                            _ => NodeTest::Pi(None),
                        },
                    };
                    self.expect(&Tok::RParen)?;
                    return Ok(test);
                }
                Ok(match n.split_once(':') {
                    Some((p, l)) => NodeTest::Name { prefix: Some(p.to_owned()), ns: None, local: Some(l.to_owned()) },
                    None => NodeTest::Name { prefix: None, ns: None, local: Some(n) },
                })
            }
            _ => Err(self.err()),
        }
    }

    fn primary(&mut self) -> Result<Expr, XPathError> {
        match self.bump() {
            Tok::Literal(s) => Ok(Expr::Literal(s)),
            Tok::Number(n) => Ok(Expr::Number(n)),
            Tok::Var(_) => Ok(Expr::Var),
            Tok::LParen => {
                let e = self.expr()?;
                self.expect(&Tok::RParen)?;
                Ok(e)
            }
            Tok::Name(name) => {
                self.expect(&Tok::LParen)?;
                let mut args = Vec::new();
                if self.peek() != &Tok::RParen {
                    args.push(self.expr()?);
                    while self.eat(&Tok::Comma) {
                        args.push(self.expr()?);
                    }
                }
                self.expect(&Tok::RParen)?;
                let (f, min, max) = Func::lookup(&name).ok_or_else(|| self.err())?;
                if args.len() < min || args.len() > max {
                    return Err(self.err());
                }
                Ok(Expr::Call(f, args))
            }
            _ => Err(self.err()),
        }
    }
}

fn descendant_or_self() -> Step {
    Step { axis: Axis::DescendantOrSelf, test: NodeTest::Node, preds: Vec::new() }
}

// ----- rewrites -----------------------------------------------------------------

/// `descendant-or-self::node()/child::T[p]` selects the same nodes as
/// `descendant::T[p]` when no predicate depends on the context position or
/// size. Rewriting `//T[p]` this way avoids materializing every node of the
/// tree as an intermediate context (upstream does the same with its "fused
/// descendant" fast path in Evaluator.zig).
fn optimize(mut e: Expr) -> Expr {
    visit_mut(&mut e, &mut |e| {
        if let Expr::Path(_, steps) = e {
            fuse_descendant_steps(steps);
        }
    });
    e
}

fn fuse_descendant_steps(steps: &mut Vec<Step>) {
    let mut i = 0;
    while i + 1 < steps.len() {
        let fusable = matches!(&steps[i], Step { axis: Axis::DescendantOrSelf, test: NodeTest::Node, preds } if preds.is_empty())
            && steps[i + 1].axis == Axis::Child
            && steps[i + 1].preds.iter().all(position_independent);
        if fusable {
            steps.remove(i);
            steps[i].axis = Axis::Descendant;
        }
        i += 1;
    }
}

/// True if a predicate's truth for a node does not depend on its position:
/// the predicate is not numeric and never reads `position()`/`last()` of
/// its own context.
fn position_independent(e: &Expr) -> bool {
    !returns_number(e) && !uses_context_position(e)
}

fn returns_number(e: &Expr) -> bool {
    match e {
        Expr::Number(_) | Expr::Arith(..) | Expr::Neg(_) => true,
        Expr::Call(f, _) => f.returns_number(),
        _ => false,
    }
}

fn uses_context_position(e: &Expr) -> bool {
    match e {
        Expr::Call(Func::Position | Func::Last, _) => true,
        Expr::Call(_, args) => args.iter().any(uses_context_position),
        Expr::Or(a, b) | Expr::And(a, b) | Expr::Cmp(_, a, b) | Expr::Arith(_, a, b) | Expr::Union(a, b) => {
            uses_context_position(a) || uses_context_position(b)
        }
        Expr::Neg(a) => uses_context_position(a),
        // Step and filter predicates have their own context.
        Expr::Path(PathStart::Expr(a), _) | Expr::Filter(a, _) => uses_context_position(a),
        Expr::Path(..) | Expr::Literal(_) | Expr::Number(_) | Expr::Var => false,
    }
}

/// Pre-order visit of every expression node (including predicates).
pub(crate) fn visit_mut(e: &mut Expr, f: &mut dyn FnMut(&mut Expr)) {
    f(e);
    match e {
        Expr::Or(a, b) | Expr::And(a, b) | Expr::Cmp(_, a, b) | Expr::Arith(_, a, b) | Expr::Union(a, b) => {
            visit_mut(a, f);
            visit_mut(b, f);
        }
        Expr::Neg(a) => visit_mut(a, f),
        Expr::Path(start, steps) => {
            if let PathStart::Expr(a) = start {
                visit_mut(a, f);
            }
            for s in steps {
                for p in &mut s.preds {
                    visit_mut(p, f);
                }
            }
        }
        Expr::Filter(a, preds) => {
            visit_mut(a, f);
            for p in preds {
                visit_mut(p, f);
            }
        }
        Expr::Call(_, args) => {
            for a in args {
                visit_mut(a, f);
            }
        }
        Expr::Literal(_) | Expr::Number(_) | Expr::Var => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(s: &str) -> Expr {
        parse(s).unwrap_or_else(|e| panic!("{s}: {e:?}"))
    }

    #[test]
    fn precedence() {
        assert!(
            matches!(ok("1 + 2 * 3"), Expr::Arith(ArithOp::Add, _, r) if matches!(*r, Expr::Arith(ArithOp::Mul, ..)))
        );
        assert!(
            matches!(ok("1 - 2 - 3"), Expr::Arith(ArithOp::Sub, l, _) if matches!(*l, Expr::Arith(ArithOp::Sub, ..)))
        );
        assert!(matches!(ok("a or b and c"), Expr::Or(_, r) if matches!(*r, Expr::And(..))));
        assert!(matches!(ok("div div div"), Expr::Arith(ArithOp::Div, ..)));
        assert!(matches!(ok("-1"), Expr::Neg(_)));
        assert!(matches!(ok("//a | //b"), Expr::Union(..)));
    }

    #[test]
    fn paths_and_axes() {
        match ok("/") {
            Expr::Path(PathStart::Root, steps) => assert!(steps.is_empty()),
            e => panic!("{e:?}"),
        }
        match ok("ancestor::div[contains(@class, 'h-[2.625rem]')][1]") {
            Expr::Path(PathStart::Context, steps) => {
                assert_eq!(steps.len(), 1);
                assert_eq!(steps[0].axis, Axis::Ancestor);
                assert_eq!(steps[0].preds.len(), 2);
            }
            e => panic!("{e:?}"),
        }
        for axis in [
            "child",
            "descendant",
            "descendant-or-self",
            "self",
            "parent",
            "ancestor",
            "ancestor-or-self",
            "following-sibling",
            "preceding-sibling",
            "following",
            "preceding",
            "attribute",
            "namespace",
        ] {
            ok(&format!("{axis}::node()"));
        }
        match ok("(//a)[1]/b") {
            Expr::Path(PathStart::Expr(f), steps) => {
                assert!(matches!(*f, Expr::Filter(..)));
                assert_eq!(steps.len(), 1);
            }
            e => panic!("{e:?}"),
        }
        ok("processing-instruction('x')");
        ok("svg:*/svg:rect");
        ok("$x + 1");
    }

    #[test]
    fn descendant_fusion() {
        let steps = |e: Expr| match e {
            Expr::Path(_, s) => s.iter().map(|s| s.axis).collect::<Vec<_>>(),
            e => panic!("{e:?}"),
        };
        assert_eq!(steps(ok("//div[@id='x']")), [Axis::Descendant]);
        assert_eq!(steps(ok(".//div")), [Axis::SelfOnly, Axis::Descendant]);
        // Positional predicates keep the child step.
        assert_eq!(steps(ok("//li[1]")), [Axis::DescendantOrSelf, Axis::Child]);
        assert_eq!(steps(ok("//li[last()]")), [Axis::DescendantOrSelf, Axis::Child]);
        assert_eq!(steps(ok("//li[count(a) = 1]")), [Axis::Descendant]);
        // Nested position() belongs to the nested step.
        assert_eq!(steps(ok("//li[a[position() = 1]]")), [Axis::Descendant]);
        assert_eq!(steps(ok("//@id")), [Axis::DescendantOrSelf, Axis::Attribute]);
    }

    #[test]
    fn syntax_errors() {
        for bad in [
            "",
            "(",
            "//",
            "a[",
            "a]",
            "foo::bar",
            "unknown()",
            "count()",
            "count(1, 2)",
            "concat('a')",
            "1 +",
            "@",
            "a/",
            "//*[",
            "text(",
            ". [1]",
        ] {
            assert!(parse(bad).is_err(), "{bad:?} should fail");
        }
        assert!(parse(&"(".repeat(1000)).is_err());
        assert!(parse(&format!("{}1", "-".repeat(1000))).is_err());
    }
}
