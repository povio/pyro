//! XPath 1.0 engine over the `Dom` store (backs `document.evaluate`,
//! `XPathEvaluator` and `XPathExpression` in pyro-webapi).
//!
//! Ported from upstream src/browser/xpath/{Tokenizer,Parser,Evaluator,
//! functions,result}.zig. Upstream follows a lenient JS polyfill; this port
//! keeps its HTML pragmatics but follows the XPath 1.0 spec and browser
//! (Blink/WebKit) behavior where they disagree:
//!
//! - Lexical/grammar errors, unknown axes, unknown functions and wrong
//!   argument counts are syntax errors (upstream skips or defers them).
//! - Name tests: in HTML documents an unprefixed name matches HTML elements
//!   ASCII case-insensitively and never matches non-HTML elements (so
//!   `//svg` is empty, `//*[local-name()='svg']` is not), as in browsers;
//!   upstream lowercases and matches any element. Prefixed names
//!   (`p:name`, `p:*`) match by namespace URI after the prefixes are
//!   resolved with [`XPathExpression::resolve_namespaces`].
//! - `namespace-uri()` and `lang()` are implemented (upstream stubs them);
//!   `name()`/`local-name()` follow the qualified/local name, so text nodes
//!   give `""`.
//! - Strings are measured in Unicode scalar values (`string-length`,
//!   `substring`), numbers parse with the strict XPath `Number` grammar.
//!
//! Attributes are not `Dom` nodes; the attribute axis yields
//! [`XPathNode::Attr`] (owner element + index into its attribute list),
//! ordered after the element and before its children. The namespace axis is
//! always empty. Variable references evaluate to the empty string (the DOM
//! API cannot bind variables; same as upstream).

mod eval;
mod parser;
mod tokenizer;

use crate::dom::Dom;
use crate::node::NodeId;

pub use eval::{number_to_string, string_to_number};

/// A node in the XPath data model.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum XPathNode {
    Node(NodeId),
    /// Attribute `index` of `element` (index into `ElementData::attrs`).
    Attr {
        element: NodeId,
        index: usize,
    },
}

/// Result of evaluating an expression. Node-sets are in document order
/// without duplicates.
#[derive(Clone, Debug, PartialEq)]
pub enum XPathValue {
    NodeSet(Vec<XPathNode>),
    Number(f64),
    String(String),
    Boolean(bool),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum XPathError {
    /// Invalid expression (DOMException `SyntaxError`).
    Syntax(String),
    /// Unresolvable namespace prefix (DOMException `NamespaceError`).
    Namespace(String),
    /// A value of the wrong type where a node-set is required (TypeError).
    Type(String),
    /// The context node is no longer alive.
    StaleNode,
}

impl std::fmt::Display for XPathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            XPathError::Syntax(m) | XPathError::Namespace(m) | XPathError::Type(m) => f.write_str(m),
            XPathError::StaleNode => f.write_str("The node is no longer available."),
        }
    }
}

impl std::error::Error for XPathError {}

/// A parsed (and optionally namespace-resolved) expression; reusable across
/// evaluations and documents.
#[derive(Clone, Debug)]
pub struct XPathExpression {
    expr: parser::Expr,
    source: String,
    /// Some prefixed name test has no namespace URI yet.
    unresolved: bool,
}

impl XPathExpression {
    pub fn parse(source: &str) -> Result<XPathExpression, XPathError> {
        let mut e = XPathExpression { expr: parser::parse(source)?, source: source.to_owned(), unresolved: false };
        e.unresolved = !e.prefixes().is_empty();
        Ok(e)
    }

    /// Distinct namespace prefixes used by name tests, in source order.
    pub fn prefixes(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        parser::visit(&self.expr, &mut |e| {
            if let parser::Expr::Path(_, steps) = e {
                for s in steps.iter() {
                    if let parser::NodeTest::Name { prefix: Some(p), .. } = &s.test {
                        if !out.contains(p) {
                            out.push(p.clone());
                        }
                    }
                }
            }
        });
        out
    }

    /// Binds every prefix through `lookup`, called once per distinct prefix
    /// (`None` or an empty URI means unresolvable: a `Namespace` error).
    pub fn resolve_namespaces(&mut self, mut lookup: impl FnMut(&str) -> Option<String>) -> Result<(), XPathError> {
        let mut err = None;
        // Each distinct prefix is looked up once.
        let mut seen: Vec<(String, Option<String>)> = Vec::new();
        let source = &self.source;
        parser::visit_mut(&mut self.expr, &mut |e| {
            if let parser::Expr::Path(_, steps) = e {
                for s in steps.iter_mut() {
                    if let parser::NodeTest::Name { prefix: Some(p), ns, .. } = &mut s.test {
                        let uri = match seen.iter().find(|(k, _)| k == p) {
                            Some((_, v)) => v.clone(),
                            None => {
                                let v = lookup(p).filter(|u| !u.is_empty());
                                seen.push((p.clone(), v.clone()));
                                v
                            }
                        };
                        match uri {
                            Some(uri) => *ns = Some(uri),
                            None => {
                                err.get_or_insert_with(|| unresolved(source));
                            }
                        }
                    }
                }
            }
        });
        self.unresolved = err.is_some();
        err.map_or(Ok(()), Err)
    }

    /// Evaluates against `context` (XPath §1 context: position 1, size 1).
    pub fn evaluate(&self, dom: &Dom, context: XPathNode) -> Result<XPathValue, XPathError> {
        if self.unresolved {
            return Err(unresolved(&self.source));
        }
        eval::evaluate(dom, &self.expr, &self.source, context)
    }

    pub fn source(&self) -> &str {
        &self.source
    }
}

fn unresolved(source: &str) -> XPathError {
    XPathError::Namespace(format!("The string '{source}' contains unresolvable namespaces."))
}

/// Parses and evaluates `source` without a namespace resolver.
pub fn evaluate(dom: &Dom, source: &str, context: XPathNode) -> Result<XPathValue, XPathError> {
    XPathExpression::parse(source)?.evaluate(dom, context)
}

impl XPathValue {
    pub fn to_boolean(&self) -> bool {
        match self {
            XPathValue::NodeSet(v) => !v.is_empty(),
            XPathValue::Number(n) => *n != 0.0 && !n.is_nan(),
            XPathValue::String(s) => !s.is_empty(),
            XPathValue::Boolean(b) => *b,
        }
    }

    /// XPath `string()` conversion (node-set: string-value of the first node).
    pub fn to_string(&self, dom: &Dom) -> String {
        match self {
            XPathValue::NodeSet(v) => v.first().map(|&n| dom.xpath_string_value(n)).unwrap_or_default(),
            XPathValue::Number(n) => number_to_string(*n),
            XPathValue::String(s) => s.clone(),
            XPathValue::Boolean(b) => b.to_string(),
        }
    }

    /// XPath `number()` conversion.
    pub fn to_number(&self, dom: &Dom) -> f64 {
        match self {
            XPathValue::Number(n) => *n,
            XPathValue::Boolean(b) => f64::from(u8::from(*b)),
            _ => string_to_number(&self.to_string(dom)),
        }
    }
}

impl Dom {
    /// XPath string-value of a node (XPath §5): concatenated descendant text
    /// for elements/documents/fragments, the value for attributes, the data
    /// for character data. Stale nodes give the empty string.
    pub fn xpath_string_value(&self, node: XPathNode) -> String {
        match eval::to_internal(self, node) {
            Ok(n) => eval::string_value(self, n).into_owned(),
            Err(_) => String::new(),
        }
    }
}
