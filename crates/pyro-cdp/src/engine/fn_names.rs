//! V8's inferred names for anonymous functions (`ns.a = function () {}`
//! is `ns.a`), which `Profiler.takePreciseCoverage` reports as a
//! function's `functionName` when it has no name of its own
//! (`SharedFunctionInfo::DebugName`). JSC gives only the language's own
//! names, so the source is parsed here as V8's parser does it, with V8's
//! `FuncNameInferrer` (src/parsing/func-name-inferrer.{h,cc}) driven from
//! the same places in the grammar as in `parser-base.h`:
//!
//! - every assignment expression, variable declarator, object literal
//!   property, formal parameter and class member opens a name state; the
//!   names pushed inside it are dropped when it ends;
//! - identifiers push variable names, `.name` and `[key]` literal names
//!   (`<computed>` for a key that is not a string), object keys literal
//!   names; `prototype` is skipped;
//! - an anonymous function (any arrow) is added when it ends, while a state
//!   is open; `=` (and `||=`, `&&=`, `??=`), a declarator with an
//!   identifier and the end of an object property or class member name
//!   every function added so far, unless the value is a call (then, and
//!   after every call and other compound assignment, the last one added is
//!   dropped);
//! - V8 parses a function's body when it is first compiled, with a fresh
//!   inferrer that starts with the function's name when it is capitalised
//!   (a constructor), so names do not carry into a function's body, except
//!   for a function right after `(`, which V8 compiles with its parent.
//!
//! Anything this parser does not follow gives no names at all.

use std::collections::HashMap;

/// V8's name for functions in `source`, by their UTF-16 `(start, end)` (end
/// inclusive: the last character of the function): the inferred name of
/// each anonymous function (`false`), and the name of each getter and
/// setter (`get g`, `true`: it replaces the engine's own name).
pub(super) fn inferred_names(source: &str) -> HashMap<(u32, u32), (String, bool)> {
    anonymous_functions(source)
        .into_iter()
        .flatten()
        .filter_map(|f| match f.accessor {
            Some(name) => Some(((f.start, f.end), (name, true))),
            None => Some(((f.start, f.end), (f.name?, false))),
        })
        .collect()
}

/// Every anonymous function, with its inferred name if it has one; `None`
/// when the source does not parse.
fn anonymous_functions(source: &str) -> Option<Vec<Func>> {
    let tokens = lex(source)?;
    let mut p = Parser {
        t: tokens,
        i: 0,
        fni: Fni::default(),
        funcs: Vec::new(),
        depth: 0,
        no_in: false,
        in_async: false,
        in_generator: false,
        pife: false,
    };
    p.program().ok()?;
    Some(p.funcs.into_iter().filter(|f| f.anonymous || f.accessor.is_some()).collect())
}

// ---------------------------------------------------------------- lexer

#[derive(Clone, Debug, PartialEq)]
enum K {
    Ident(String),
    Private(String),
    Punct(&'static str),
    Str(String),
    Num(String),
    Regex,
    /// A template's text from its opening backtick (`head`) or a `}`, up to
    /// `${` or its closing backtick (`tail`).
    Template {
        head: bool,
        tail: bool,
    },
    Eof,
}

#[derive(Clone, Debug)]
struct Tok {
    k: K,
    start: u32,
    /// Exclusive, UTF-16.
    end: u32,
    /// A line terminator comes before it.
    nl: bool,
}

const PUNCTS: &[&str] = &[
    ">>>=", "...", "===", "!==", "**=", "<<=", ">>=", ">>>", "&&=", "||=", "??=", "=>", "==", "!=", "<=", ">=", "&&",
    "||", "??", "?.", "++", "--", "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=", "**", "<<", ">>", "{", "}", "(", ")",
    "[", "]", ";", ",", "<", ">", "+", "-", "*", "/", "%", "&", "|", "^", "!", "~", "?", ":", "=", ".", "@",
];

const REGEX_AFTER_WORD: &[&str] = &[
    "return",
    "typeof",
    "instanceof",
    "in",
    "of",
    "new",
    "delete",
    "void",
    "throw",
    "case",
    "do",
    "else",
    "yield",
    "await",
];

fn is_line_terminator(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

fn is_id_start(c: char) -> bool {
    c == '$' || c == '_' || c == '\\' || c.is_alphabetic()
}

fn is_id_part(c: char) -> bool {
    is_id_start(c) || c.is_alphanumeric() || c == '\u{200c}' || c == '\u{200d}'
}

fn lex(source: &str) -> Option<Vec<Tok>> {
    let chars: Vec<char> = source.chars().collect();
    // UTF-16 offset of each char (and of the end).
    let mut at = Vec::with_capacity(chars.len() + 1);
    let mut o = 0u32;
    for c in &chars {
        at.push(o);
        o += c.len_utf16() as u32;
    }
    at.push(o);
    let mut out: Vec<Tok> = Vec::new();
    // `true` for a `${` whose `}` resumes a template.
    let mut braces: Vec<bool> = Vec::new();
    let mut i = 0;
    let mut nl = false;
    // Scans template text from `i` (after a backtick or `}`): the index after
    // the closing backtick or `${`, and whether it was the backtick.
    let template = |mut i: usize| -> Option<(usize, bool)> {
        while i < chars.len() {
            match chars[i] {
                '\\' => i += 2,
                '`' => return Some((i + 1, true)),
                '$' if chars.get(i + 1) == Some(&'{') => return Some((i + 2, false)),
                _ => i += 1,
            }
        }
        None
    };
    while i < chars.len() {
        let c = chars[i];
        if is_line_terminator(c) {
            nl = true;
            i += 1;
            continue;
        }
        if c.is_whitespace() || c == '\u{feff}' {
            i += 1;
            continue;
        }
        let next = chars.get(i + 1).copied();
        if c == '/' && next == Some('/') {
            while i < chars.len() && !is_line_terminator(chars[i]) {
                i += 1;
            }
            continue;
        }
        if c == '/' && next == Some('*') {
            i += 2;
            while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                nl |= is_line_terminator(chars[i]);
                i += 1;
            }
            i = (i + 2).min(chars.len());
            continue;
        }
        if c == '#' && i == 0 && next == Some('!') {
            while i < chars.len() && !is_line_terminator(chars[i]) {
                i += 1;
            }
            continue;
        }
        let start = i;
        let k = if is_id_start(c) || (c == '#' && next.is_some_and(is_id_start)) {
            i += 1;
            while i < chars.len() && (is_id_part(chars[i]) || (chars[i - 1] == '\\')) {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            if c == '#' {
                K::Private(word)
            } else {
                K::Ident(word)
            }
        } else if c.is_ascii_digit() || (c == '.' && next.is_some_and(|n| n.is_ascii_digit())) {
            let hex = c == '0' && next.is_some_and(|n| n.is_ascii_alphabetic());
            i += 1;
            while i < chars.len() {
                let d = chars[i];
                let sign = (d == '+' || d == '-') && !hex && matches!(chars[i - 1], 'e' | 'E');
                if d.is_ascii_alphanumeric() || d == '_' || (d == '.' && !hex) || sign {
                    i += 1;
                } else {
                    break;
                }
            }
            K::Num(chars[start..i].iter().collect())
        } else if c == '"' || c == '\'' {
            i += 1;
            let mut value = String::new();
            loop {
                let d = *chars.get(i)?;
                i += 1;
                if d == c {
                    break;
                }
                if d == '\\' {
                    value.push(d);
                    value.push(*chars.get(i)?);
                    i += 1;
                } else {
                    value.push(d);
                }
            }
            K::Str(value)
        } else if c == '`' {
            let (end, tail) = template(i + 1)?;
            i = end;
            if !tail {
                braces.push(true);
            }
            K::Template { head: true, tail }
        } else if c == '}' && braces.last() == Some(&true) {
            braces.pop();
            let (end, tail) = template(i + 1)?;
            i = end;
            if !tail {
                braces.push(true);
            }
            K::Template { head: false, tail }
        } else if c == '/' && regex_allowed(out.last()) {
            i += 1;
            let mut class = false;
            loop {
                let d = *chars.get(i)?;
                if is_line_terminator(d) {
                    return None;
                }
                i += 1;
                match d {
                    '\\' => i += 1,
                    '[' => class = true,
                    ']' => class = false,
                    '/' if !class => break,
                    _ => {}
                }
            }
            while i < chars.len() && is_id_part(chars[i]) {
                i += 1;
            }
            K::Regex
        } else {
            let rest = &chars[i..];
            let p = PUNCTS.iter().find(|p| {
                p.len() <= rest.len()
                    && p.chars().zip(rest).all(|(a, b)| a == *b)
                    // `a?.5:b` is a conditional.
                    && !(**p == "?." && rest.get(2).is_some_and(|d| d.is_ascii_digit()))
            })?;
            i += p.len();
            match *p {
                "{" => braces.push(false),
                "}" => {
                    braces.pop();
                }
                _ => {}
            }
            K::Punct(p)
        };
        out.push(Tok { k, start: at[start], end: at[i.min(chars.len())], nl });
        nl = false;
    }
    out.push(Tok { k: K::Eof, start: o, end: o, nl: true });
    Some(out)
}

fn regex_allowed(prev: Option<&Tok>) -> bool {
    match prev.map(|t| &t.k) {
        None => true,
        Some(K::Punct(p)) => !matches!(*p, ")" | "]"),
        Some(K::Ident(w)) => REGEX_AFTER_WORD.contains(&w.as_str()),
        Some(K::Template { tail, .. }) => !tail,
        _ => false,
    }
}

// ------------------------------------------------------- name inferrer

#[derive(Clone, Copy, PartialEq)]
enum NameType {
    Enclosing,
    Literal,
    Variable,
}

/// V8's `FuncNameInferrer`.
#[derive(Default)]
struct Fni {
    names: Vec<(String, NameType)>,
    /// Indices into `Parser::funcs`.
    pending: Vec<usize>,
    depth: usize,
}

impl Fni {
    fn open(&mut self) -> usize {
        self.depth += 1;
        self.names.len()
    }
    fn close(&mut self, top: usize) {
        self.names.truncate(top);
        self.depth -= 1;
    }
    fn is_open(&self) -> bool {
        self.depth > 0
    }
    fn push_enclosing(&mut self, name: &str) {
        if name.chars().next().is_some_and(char::is_uppercase) {
            self.names.push((name.into(), NameType::Enclosing));
        }
    }
    fn push_literal(&mut self, name: &str) {
        if self.is_open() && name != "prototype" {
            self.names.push((name.into(), NameType::Literal));
        }
    }
    fn push_variable(&mut self, name: &str) {
        if self.is_open() {
            self.names.push((name.into(), NameType::Variable));
        }
    }
    fn add(&mut self, f: usize) {
        if self.is_open() {
            self.pending.push(f);
        }
    }
    fn remove_last(&mut self) {
        if self.is_open() {
            self.pending.pop();
        }
    }
    fn name(&self) -> String {
        let mut out = String::new();
        for (i, (name, ty)) in self.names.iter().enumerate() {
            if *ty == NameType::Variable && self.names.get(i + 1).is_some_and(|n| n.1 == NameType::Variable) {
                continue;
            }
            if !out.is_empty() {
                out.push('.');
            }
            out.push_str(name);
        }
        out
    }
}

// --------------------------------------------------------------- parser

struct Func {
    start: u32,
    end: u32,
    anonymous: bool,
    /// Compiled with its parent (right after `(`): V8 keeps no inferred
    /// name for it.
    eager: bool,
    name: Option<String>,
    /// A getter or setter's name, `get g` / `set g`.
    accessor: Option<String>,
}

/// What V8's checks need to know about a parsed expression.
#[derive(Clone, Copy, PartialEq)]
enum E {
    Ident,
    Call,
    /// An anonymous function (`funcs` index), parenthesized or not.
    Func(usize),
    Other,
}

type R<T = ()> = Result<T, ()>;

struct Parser {
    t: Vec<Tok>,
    i: usize,
    fni: Fni,
    funcs: Vec<Func>,
    depth: usize,
    no_in: bool,
    in_async: bool,
    in_generator: bool,
    /// The next function follows `(`: V8 compiles it with its parent.
    pife: bool,
}

const RESERVED: &[&str] = &[
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "export",
    "extends",
    "finally",
    "for",
    "function",
    "if",
    "import",
    "in",
    "instanceof",
    "new",
    "return",
    "super",
    "switch",
    "this",
    "throw",
    "try",
    "typeof",
    "var",
    "void",
    "while",
    "with",
    "null",
    "true",
    "false",
];

impl Parser {
    fn peek(&self) -> &K {
        &self.t[self.i].k
    }
    fn peek_at(&self, n: usize) -> &K {
        &self.t[(self.i + n).min(self.t.len() - 1)].k
    }
    fn is(&self, p: &str) -> bool {
        matches!(self.peek(), K::Punct(q) if *q == p)
    }
    fn is_word(&self, w: &str) -> bool {
        matches!(self.peek(), K::Ident(x) if x == w)
    }
    fn word_at(&self, n: usize, w: &str) -> bool {
        matches!(self.peek_at(n), K::Ident(x) if x == w)
    }
    fn punct_at(&self, n: usize, p: &str) -> bool {
        matches!(self.peek_at(n), K::Punct(q) if *q == p)
    }
    fn nl_before(&self) -> bool {
        self.t[self.i].nl
    }
    fn bump(&mut self) -> R<&Tok> {
        if self.peek() == &K::Eof {
            return Err(());
        }
        self.i += 1;
        Ok(&self.t[self.i - 1])
    }
    fn eat(&mut self, p: &str) -> bool {
        if self.is(p) {
            self.i += 1;
            true
        } else {
            false
        }
    }
    fn expect(&mut self, p: &str) -> R {
        if self.eat(p) {
            Ok(())
        } else {
            Err(())
        }
    }
    fn last_end(&self) -> u32 {
        self.t[self.i.saturating_sub(1)].end
    }
    fn ident(&self) -> Option<&str> {
        match self.peek() {
            K::Ident(w) if !RESERVED.contains(&w.as_str()) => Some(w),
            _ => None,
        }
    }
    fn enter(&mut self) -> R {
        self.depth += 1;
        if self.depth > 1500 {
            Err(())
        } else {
            Ok(())
        }
    }
    fn leave(&mut self) {
        self.depth -= 1;
    }
    /// Ends a statement: `;`, or a line break, `}` or the end before the
    /// next token.
    fn semicolon(&mut self) -> R {
        if self.eat(";") || self.nl_before() || self.is("}") || self.peek() == &K::Eof {
            Ok(())
        } else {
            Err(())
        }
    }

    fn program(&mut self) -> R {
        while self.peek() != &K::Eof {
            self.statement_list_item()?;
        }
        Ok(())
    }

    fn statement_list_item(&mut self) -> R {
        let before = self.i;
        self.enter()?;
        let r = self.statement_list_item_inner();
        self.leave();
        r?;
        if self.i == before {
            Err(())
        } else {
            Ok(())
        }
    }

    fn statement_list_item_inner(&mut self) -> R {
        if self.is_word("function") || (self.is_word("async") && self.word_at(1, "function") && !self.t[self.i + 1].nl)
        {
            return self.hoistable_declaration();
        }
        if self.is_word("class") {
            self.bump()?;
            return self.class_literal();
        }
        if self.is_word("const")
            || self.is_word("var")
            || (self.is_word("let")
                && (matches!(self.peek_at(1), K::Ident(_)) || self.punct_at(1, "[") || self.punct_at(1, "{")))
        {
            self.bump()?;
            self.variable_declarations()?;
            return self.semicolon();
        }
        if self.is_word("import") && !self.punct_at(1, "(") && !self.punct_at(1, ".") {
            // `import ... from '...'`: no functions in it.
            while !matches!(self.peek(), K::Str(_)) {
                self.bump()?;
            }
            self.bump()?;
            if self.is_word("with") || self.is_word("assert") {
                self.bump()?;
                self.balanced()?;
            }
            return self.semicolon();
        }
        if self.is_word("export") {
            self.bump()?;
            if self.is_word("default") {
                self.bump()?;
                if self.is_word("function")
                    || self.is_word("class")
                    || (self.is_word("async") && self.word_at(1, "function"))
                {
                    return self.statement_list_item_inner();
                }
                self.assignment_expression()?;
                return self.semicolon();
            }
            if self.is("*") || self.is("{") {
                while !self.is(";") && !self.nl_before_or_end() {
                    if self.is("{") {
                        self.balanced()?;
                    } else {
                        self.bump()?;
                    }
                }
                return self.semicolon();
            }
            return self.statement_list_item_inner();
        }
        self.statement()
    }

    fn nl_before_or_end(&self) -> bool {
        self.nl_before() || self.peek() == &K::Eof || self.is("}")
    }

    /// Skips a balanced `{...}`, `(...)` or `[...]`.
    fn balanced(&mut self) -> R {
        let mut level = 0usize;
        loop {
            match self.bump()?.k {
                K::Punct("{" | "(" | "[") => level += 1,
                K::Punct("}" | ")" | "]") => level -= 1,
                _ => {}
            }
            if level == 0 {
                return Ok(());
            }
        }
    }

    fn statement(&mut self) -> R {
        match self.peek().clone() {
            K::Punct("{") => self.block(),
            K::Punct(";") => {
                self.bump()?;
                Ok(())
            }
            K::Ident(w) => match w.as_str() {
                "if" => {
                    self.bump()?;
                    self.paren_expression()?;
                    self.statement_list_item()?;
                    if self.is_word("else") {
                        self.bump()?;
                        self.statement_list_item()?;
                    }
                    Ok(())
                }
                "while" | "with" => {
                    self.bump()?;
                    self.paren_expression()?;
                    self.statement_list_item()
                }
                "do" => {
                    self.bump()?;
                    self.statement_list_item()?;
                    if !self.is_word("while") {
                        return Err(());
                    }
                    self.bump()?;
                    self.paren_expression()?;
                    self.eat(";");
                    Ok(())
                }
                "for" => self.for_statement(),
                "return" | "throw" => {
                    self.bump()?;
                    if !self.is(";") && !self.nl_before_or_end() {
                        self.expression()?;
                    }
                    self.semicolon()
                }
                "break" | "continue" => {
                    self.bump()?;
                    if !self.nl_before() && self.ident().is_some() {
                        self.bump()?;
                    }
                    self.semicolon()
                }
                "debugger" => {
                    self.bump()?;
                    self.semicolon()
                }
                "try" => {
                    self.bump()?;
                    self.block()?;
                    if self.is_word("catch") {
                        self.bump()?;
                        if self.eat("(") {
                            let top = self.fni.open();
                            let r = self.binding_pattern();
                            self.fni.close(top);
                            r?;
                            self.expect(")")?;
                        }
                        self.block()?;
                    }
                    if self.is_word("finally") {
                        self.bump()?;
                        self.block()?;
                    }
                    Ok(())
                }
                "switch" => {
                    self.bump()?;
                    self.paren_expression()?;
                    self.expect("{")?;
                    while !self.eat("}") {
                        if self.is_word("case") {
                            self.bump()?;
                            self.expression()?;
                            self.expect(":")?;
                        } else if self.is_word("default") {
                            self.bump()?;
                            self.expect(":")?;
                        } else {
                            self.statement_list_item()?;
                        }
                    }
                    Ok(())
                }
                _ if self.ident().is_some() && self.punct_at(1, ":") => {
                    // A label.
                    self.bump()?;
                    self.bump()?;
                    self.statement_list_item()
                }
                _ => self.expression_statement(),
            },
            _ => self.expression_statement(),
        }
    }

    fn expression_statement(&mut self) -> R {
        self.expression()?;
        self.semicolon()
    }

    fn block(&mut self) -> R {
        self.expect("{")?;
        while !self.eat("}") {
            self.statement_list_item()?;
        }
        Ok(())
    }

    fn paren_expression(&mut self) -> R {
        self.expect("(")?;
        self.expression()?;
        self.expect(")")
    }

    fn for_statement(&mut self) -> R {
        self.bump()?;
        if self.is_word("await") {
            self.bump()?;
        }
        self.expect("(")?;
        let no_in = std::mem::replace(&mut self.no_in, true);
        let r = (|| {
            if self.is_word("var")
                || self.is_word("const")
                || (self.is_word("let")
                    && (matches!(self.peek_at(1), K::Ident(_)) || self.punct_at(1, "[") || self.punct_at(1, "{")))
            {
                self.bump()?;
                self.variable_declarations()?;
            } else if !self.is(";") {
                self.expression()?;
            }
            Ok(())
        })();
        self.no_in = no_in;
        r?;
        if self.is_word("in") || self.is_word("of") {
            self.bump()?;
            self.expression()?;
        } else {
            self.expect(";")?;
            if !self.is(";") {
                self.expression()?;
            }
            self.expect(";")?;
            if !self.is(")") {
                self.expression()?;
            }
        }
        self.expect(")")?;
        self.statement_list_item()
    }

    /// `ParseVariableDeclarations`, after `var`, `let` or `const`.
    fn variable_declarations(&mut self) -> R {
        loop {
            let top = self.fni.open();
            let r = (|| {
                let ident = self.binding_pattern()?;
                if self.eat("=") {
                    let value = self.assignment_expression()?;
                    if ident {
                        self.named(value);
                        if value == E::Call {
                            self.fni.remove_last();
                        } else {
                            self.infer();
                        }
                    }
                }
                Ok(())
            })();
            self.fni.close(top);
            r?;
            if !self.eat(",") {
                return Ok(());
            }
        }
    }

    /// A binding identifier or pattern: whether it was an identifier.
    fn binding_pattern(&mut self) -> R<bool> {
        if let Some(name) = self.ident() {
            let name = name.to_owned();
            self.bump()?;
            self.fni.push_variable(&name);
            return Ok(true);
        }
        // `{a, b: [c] = d}` / `[a, ...b]`: their defaults may hold functions.
        if self.is("{") || self.is("[") {
            self.primary()?;
            return Ok(false);
        }
        Err(())
    }

    /// A function given the name it is bound to (NamedEvaluation:
    /// `x = function () {}`, `{ k: () => 1 }`, `f(a = () => 1)`) has a name
    /// of its own: V8 reports that one.
    fn named(&mut self, value: E) {
        if let E::Func(f) = value {
            self.funcs[f].anonymous = false;
        }
    }

    fn infer(&mut self) {
        if !self.fni.pending.is_empty() {
            let name = self.fni.name();
            for f in std::mem::take(&mut self.fni.pending) {
                if !self.funcs[f].eager {
                    self.funcs[f].name = Some(name.clone());
                }
            }
        }
    }

    fn expression(&mut self) -> R<E> {
        let mut e = self.assignment_expression()?;
        while self.eat(",") {
            self.assignment_expression()?;
            e = E::Other;
        }
        Ok(e)
    }

    /// `ParseAssignmentExpressionCoverGrammar`.
    fn assignment_expression(&mut self) -> R<E> {
        self.enter()?;
        let r = self.assignment_expression_inner();
        self.leave();
        r
    }

    fn assignment_expression_inner(&mut self) -> R<E> {
        if self.in_generator && self.is_word("yield") {
            self.bump()?;
            if !self.nl_before() {
                self.eat("*");
                if !matches!(self.peek(), K::Punct(")" | "]" | "}" | "," | ";" | ":") | K::Eof)
                    && !self.is_word("in")
                    && !self.nl_before()
                {
                    self.assignment_expression()?;
                }
            }
            return Ok(E::Other);
        }
        let top = self.fni.open();
        let r = (|| {
            let lhs = self.conditional()?;
            let op = match self.peek() {
                K::Punct(p) if p.ends_with('=') && !matches!(*p, "==" | "===" | "!=" | "!==" | "<=" | ">=" | "=>") => {
                    *p
                }
                _ => return Ok(lhs),
            };
            self.bump()?;
            let right = self.assignment_expression()?;
            if lhs == E::Ident && matches!(op, "=" | "||=" | "&&=" | "??=") {
                self.named(right);
            }
            if matches!(op, "=" | "||=" | "&&=" | "??=") && right != E::Call {
                self.infer();
            } else {
                self.fni.remove_last();
            }
            Ok(E::Other)
        })();
        self.fni.close(top);
        r
    }

    fn conditional(&mut self) -> R<E> {
        let e = self.binary(0)?;
        if !self.eat("?") {
            return Ok(e);
        }
        let no_in = std::mem::replace(&mut self.no_in, false);
        let r = self.assignment_expression();
        self.no_in = no_in;
        r?;
        self.expect(":")?;
        self.assignment_expression()?;
        Ok(E::Other)
    }

    fn binary_precedence(&self) -> Option<u8> {
        Some(match self.peek() {
            K::Punct(p) => match *p {
                "??" => 1,
                "||" => 2,
                "&&" => 3,
                "|" => 4,
                "^" => 5,
                "&" => 6,
                "==" | "!=" | "===" | "!==" => 7,
                "<" | ">" | "<=" | ">=" => 8,
                "<<" | ">>" | ">>>" => 9,
                "+" | "-" => 10,
                "*" | "/" | "%" => 11,
                "**" => 12,
                _ => return None,
            },
            K::Ident(w) if w == "instanceof" || (w == "in" && !self.no_in) => 8,
            _ => return None,
        })
    }

    fn binary(&mut self, min: u8) -> R<E> {
        let mut e = if matches!(self.peek(), K::Private(_)) && self.word_at(1, "in") {
            // `#x in obj`
            self.bump()?;
            E::Other
        } else {
            self.unary()?
        };
        while let Some(prec) = self.binary_precedence() {
            if prec <= min {
                break;
            }
            self.bump()?;
            // `**` is right-associative.
            self.binary(if prec == 12 { prec - 1 } else { prec })?;
            e = E::Other;
        }
        Ok(e)
    }

    fn unary(&mut self) -> R<E> {
        self.enter()?;
        let r = self.unary_inner();
        self.leave();
        r
    }

    fn unary_inner(&mut self) -> R<E> {
        let prefix = match self.peek() {
            K::Punct(p) => matches!(*p, "!" | "~" | "+" | "-" | "++" | "--"),
            K::Ident(w) => matches!(w.as_str(), "typeof" | "void" | "delete") || (w == "await" && self.in_async),
            _ => false,
        };
        if prefix {
            self.bump()?;
            self.unary()?;
            return Ok(E::Other);
        }
        let e = self.lhs()?;
        if (self.is("++") || self.is("--")) && !self.nl_before() {
            self.bump()?;
            return Ok(E::Other);
        }
        Ok(e)
    }

    /// `ParseLeftHandSideExpression`: a member expression, then calls.
    fn lhs(&mut self) -> R<E> {
        let mut e = self.member()?;
        // `ParseLeftHandSideContinuation`: unlike in a member expression,
        // `[key]` pushes no name.
        let mut optional = false;
        loop {
            if self.is(".") || (self.is("?.") && !self.punct_at(1, "(") && !self.punct_at(1, "[")) {
                optional |= self.is("?.");
                self.bump()?;
                self.property_name_access()?;
                e = E::Other;
            } else if self.is("[") || (self.is("?.") && self.punct_at(1, "[")) {
                optional |= self.is("?.");
                if self.is("?.") {
                    self.bump()?;
                }
                self.bump()?;
                self.computed_key()?;
                self.expect("]")?;
                e = E::Other;
            } else if self.is("(") || (self.is("?.") && self.punct_at(1, "(")) {
                optional |= self.is("?.");
                if self.is("?.") {
                    self.bump()?;
                }
                self.arguments()?;
                self.fni.remove_last();
                e = if optional { E::Other } else { E::Call };
            } else if matches!(self.peek(), K::Template { head: true, .. }) {
                self.template()?;
                e = E::Other;
            } else {
                return Ok(e);
            }
        }
    }

    /// The name after `.`: pushed as a literal name.
    fn property_name_access(&mut self) -> R {
        let name = match self.peek() {
            K::Ident(w) | K::Private(w) => w.clone(),
            _ => return Err(()),
        };
        self.bump()?;
        self.fni.push_literal(&name);
        Ok(())
    }

    /// A `[key]`'s expression: its string when it is one.
    fn computed_key(&mut self) -> R<Option<String>> {
        let no_in = std::mem::replace(&mut self.no_in, false);
        let single = match (&self.peek().clone(), self.punct_at(1, "]")) {
            (K::Str(s), true) => Some(s.clone()),
            _ => None,
        };
        let r = self.expression();
        self.no_in = no_in;
        r?;
        Ok(single)
    }

    /// `PushPropertyName`: a string key that is not an array index, else
    /// `<computed>`.
    fn push_property_name(&mut self, key: Option<String>) {
        match key {
            Some(s) if !(s.parse::<u32>().is_ok_and(|n| n != u32::MAX && n.to_string() == s)) => {
                self.fni.push_literal(&s)
            }
            _ => self.fni.push_literal("<computed>"),
        }
    }

    fn arguments(&mut self) -> R {
        self.expect("(")?;
        let no_in = std::mem::replace(&mut self.no_in, false);
        let r = (|| {
            while !self.eat(")") {
                self.eat("...");
                self.assignment_expression()?;
                if !self.is(")") {
                    self.expect(",")?;
                }
            }
            Ok(())
        })();
        self.no_in = no_in;
        r
    }

    /// `ParseMemberExpression`: `new` forms, or a primary expression, then
    /// `.name`, `[key]` and templates.
    fn member(&mut self) -> R<E> {
        let mut e = if self.is_word("new") {
            self.bump()?;
            if self.eat(".") {
                // new.target
                self.bump()?;
                E::Other
            } else {
                self.member_no_call()?;
                if self.is("(") {
                    self.arguments()?;
                }
                E::Call
            }
        } else {
            self.primary()?
        };
        loop {
            if self.is(".") {
                self.bump()?;
                self.property_name_access()?;
                e = E::Other;
            } else if self.is("[") {
                self.bump()?;
                let key = self.computed_key()?;
                self.expect("]")?;
                self.push_property_name(key);
                e = E::Other;
            } else if matches!(self.peek(), K::Template { head: true, .. }) {
                self.template()?;
                e = E::Other;
            } else {
                return Ok(e);
            }
        }
    }

    /// The constructor of a `new`: a member expression without calls.
    fn member_no_call(&mut self) -> R {
        if self.is_word("new") {
            self.bump()?;
            self.member_no_call()?;
            if self.is("(") {
                self.arguments()?;
            }
            return Ok(());
        }
        self.primary()?;
        loop {
            if self.is(".") {
                self.bump()?;
                self.property_name_access()?;
            } else if self.is("[") {
                self.bump()?;
                let key = self.computed_key()?;
                self.expect("]")?;
                self.push_property_name(key);
            } else if matches!(self.peek(), K::Template { head: true, .. }) {
                self.template()?;
            } else {
                return Ok(());
            }
        }
    }

    fn template(&mut self) -> R {
        let K::Template { head: true, tail } = self.bump()?.k else { return Err(()) };
        if tail {
            return Ok(());
        }
        let no_in = std::mem::replace(&mut self.no_in, false);
        let r = (|| loop {
            self.expression()?;
            match self.bump()?.k {
                K::Template { head: false, tail: true } => return Ok(()),
                K::Template { head: false, tail: false } => {}
                _ => return Err(()),
            }
        })();
        self.no_in = no_in;
        r
    }

    /// Whether a function or arrow function starts at the cursor.
    fn function_starts_here(&mut self) -> bool {
        let at = |p: &mut Self, n: usize| {
            p.i += n;
            let r = p.is("(") && p.parens_then_arrow();
            p.i -= n;
            r
        };
        let ident = |k: &K| matches!(k, K::Ident(x) if !RESERVED.contains(&x.as_str()));
        self.is_word("function")
            || (ident(self.peek()) && self.punct_at(1, "=>"))
            || at(self, 0)
            || (self.is_word("async")
                && (self.word_at(1, "function") || (ident(self.peek_at(1)) && self.punct_at(2, "=>")) || at(self, 1)))
    }

    /// Whether the `(` at the cursor starts an arrow function's parameters.
    fn parens_then_arrow(&self) -> bool {
        let mut level = 0usize;
        let mut j = self.i;
        loop {
            match &self.t[j].k {
                K::Punct("(" | "[" | "{") | K::Template { head: true, tail: false } => level += 1,
                K::Template { head: false, tail: true } => level -= 1,
                K::Punct(")" | "]" | "}") => {
                    level -= 1;
                    if level == 0 {
                        let next = &self.t[j + 1];
                        return next.k == K::Punct("=>") && !next.nl;
                    }
                }
                K::Eof => return false,
                _ => {}
            }
            j += 1;
        }
    }

    fn primary(&mut self) -> R<E> {
        self.enter()?;
        let r = self.primary_inner();
        self.leave();
        r
    }

    fn primary_inner(&mut self) -> R<E> {
        let start = self.t[self.i].start;
        match self.peek().clone() {
            K::Ident(w) => {
                // Arrow functions.
                if self.ident().is_some() && self.punct_at(1, "=>") && !self.t[self.i + 1].nl {
                    self.bump()?;
                    let eager = std::mem::take(&mut self.pife);
                    return self.arrow_body(start, false, eager);
                }
                if w == "async" && !self.t[self.i + 1].nl {
                    if matches!(self.peek_at(1), K::Ident(x) if !RESERVED.contains(&x.as_str()))
                        && self.punct_at(2, "=>")
                    {
                        self.bump()?;
                        self.bump()?;
                        let eager = std::mem::take(&mut self.pife);
                        return self.arrow_body(start, true, eager);
                    }
                    if self.punct_at(1, "(") {
                        self.i += 1;
                        if self.parens_then_arrow() {
                            return self.arrow(start, true);
                        }
                        self.i -= 1;
                    }
                    if self.word_at(1, "function") {
                        self.bump()?;
                        return self.function_expression(start, true);
                    }
                }
                match w.as_str() {
                    "function" => self.function_expression(start, false),
                    "class" => {
                        self.bump()?;
                        self.class_literal()?;
                        Ok(E::Other)
                    }
                    "this" | "null" | "true" | "false" | "super" => {
                        self.bump()?;
                        Ok(E::Other)
                    }
                    "import" => {
                        // import(...) / import.meta
                        self.bump()?;
                        if self.eat(".") {
                            self.bump()?;
                            return Ok(E::Other);
                        }
                        self.arguments()?;
                        Ok(E::Other)
                    }
                    _ if RESERVED.contains(&w.as_str()) => Err(()),
                    _ => {
                        self.bump()?;
                        self.fni.push_variable(&w);
                        Ok(E::Ident)
                    }
                }
            }
            K::Punct("(") => {
                if self.parens_then_arrow() {
                    return self.arrow(start, false);
                }
                self.bump()?;
                let no_in = std::mem::replace(&mut self.no_in, false);
                // `(function` is compiled with its parent (V8's PIFE hint).
                self.pife = self.function_starts_here();
                let r = self.expression();
                self.no_in = no_in;
                let e = r?;
                self.expect(")")?;
                // V8 keeps a parenthesized expression's node: `(f())` is a call.
                Ok(if matches!(e, E::Call | E::Func(_)) { e } else { E::Other })
            }
            K::Punct("[") => {
                self.bump()?;
                let no_in = std::mem::replace(&mut self.no_in, false);
                let r = (|| {
                    while !self.eat("]") {
                        if self.eat(",") {
                            continue;
                        }
                        self.eat("...");
                        self.assignment_expression()?;
                        if !self.is("]") {
                            self.expect(",")?;
                        }
                    }
                    Ok(())
                })();
                self.no_in = no_in;
                r?;
                Ok(E::Other)
            }
            K::Punct("{") => {
                self.object_literal()?;
                Ok(E::Other)
            }
            K::Template { head: true, .. } => {
                self.template()?;
                Ok(E::Other)
            }
            K::Str(_) | K::Num(_) | K::Regex => {
                self.bump()?;
                Ok(E::Other)
            }
            K::Punct("@") => {
                // A decorator: `@expr` before a class.
                self.bump()?;
                self.lhs()?;
                self.primary()
            }
            _ => Err(()),
        }
    }

    fn object_literal(&mut self) -> R {
        self.expect("{")?;
        let no_in = std::mem::replace(&mut self.no_in, false);
        let r = (|| {
            while !self.eat("}") {
                let top = self.fni.open();
                let r = self.property_definition(false);
                if r.is_ok() && !self.is("}") && !self.eat(",") {
                    self.fni.close(top);
                    return Err(());
                }
                if r.is_ok() {
                    self.infer();
                }
                self.fni.close(top);
                r?;
            }
            Ok(())
        })();
        self.no_in = no_in;
        r
    }

    /// A property key (`ParseProperty`): pushes its name. `None` for a
    /// computed key.
    fn property_key(&mut self) -> R<Option<String>> {
        let name = match self.peek().clone() {
            K::Ident(w) | K::Private(w) => w,
            K::Str(s) => s,
            K::Num(n) => n,
            K::Punct("[") => {
                self.bump()?;
                self.computed_key()?;
                self.expect("]")?;
                return Ok(None);
            }
            _ => return Err(()),
        };
        self.bump()?;
        self.fni.push_literal(&name);
        Ok(Some(name))
    }

    /// An object literal property or class member, after any `static`.
    fn property_definition(&mut self, class: bool) -> R {
        if !class && self.eat("...") {
            self.assignment_expression()?;
            return Ok(());
        }
        let start = self.t[self.i].start;
        let mut is_async = false;
        let mut generator = false;
        let mut accessor = None;
        // `get`/`set`/`async` are modifiers when a key follows them.
        let key_follows = |p: &Parser, n: usize| {
            !matches!(p.peek_at(n), K::Punct("(" | ":" | "," | "}" | "=" | ";") | K::Eof)
                && !(class && p.t[p.i + n].nl && !matches!(p.peek_at(n), K::Punct("[" | "*")))
        };
        if (self.is_word("get") || self.is_word("set")) && key_follows(self, 1) {
            accessor = Some(if self.is_word("get") { "get" } else { "set" });
            self.bump()?;
        } else if self.is_word("async") && key_follows(self, 1) && !self.t[self.i + 1].nl {
            self.bump()?;
            is_async = true;
        }
        if self.eat("*") {
            generator = true;
        }
        let key = self.property_key()?;
        if self.is("(") {
            // A method: named, so not inferred, except in an object literal
            // with a computed key (V8 gives the function no name of its own
            // there, so it takes the object's).
            let f = self.function_rest(start, is_async, generator, key.as_deref(), false)?;
            match (&key, accessor) {
                (Some(key), Some(kind)) => self.funcs[f].accessor = Some(format!("{kind} {key}")),
                (None, _) if !class => {
                    self.funcs[f].anonymous = true;
                    self.fni.add(f);
                }
                _ => {}
            }
            return Ok(());
        }
        if class {
            if self.eat("=") {
                let value = self.assignment_expression()?;
                if key.is_some() {
                    self.named(value);
                }
            }
            return self.semicolon_class();
        }
        if self.eat(":") {
            let value = self.assignment_expression()?;
            if key.is_some() {
                self.named(value);
            }
            return Ok(());
        }
        // Shorthand `a` (a variable name) or `a = default` in a pattern.
        let Some(name) = key else { return Err(()) };
        self.fni.push_variable(&name);
        if self.eat("=") {
            let value = self.assignment_expression()?;
            self.named(value);
        }
        Ok(())
    }

    fn semicolon_class(&mut self) -> R {
        if self.eat(";") || self.nl_before() || self.is("}") {
            Ok(())
        } else {
            Err(())
        }
    }

    fn class_literal(&mut self) -> R {
        // After `class`: an optional name, `extends`, then the body.
        if self.ident().is_some() && !self.is_word("extends") {
            self.bump()?;
        }
        if self.is_word("extends") {
            self.bump()?;
            let top = self.fni.open();
            let r = self.lhs();
            self.fni.close(top);
            r?;
        }
        self.expect("{")?;
        let (a, g) = (self.in_async, self.in_generator);
        let r = (|| {
            while !self.eat("}") {
                if self.eat(";") {
                    continue;
                }
                let top = self.fni.open();
                let r = (|| {
                    while self.is("@") {
                        self.bump()?;
                        self.lhs()?;
                    }
                    if self.is_word("static") && self.punct_at(1, "{") {
                        self.bump()?;
                        self.in_async = false;
                        self.in_generator = false;
                        return self.function_body();
                    }
                    if self.is_word("static") && !matches!(self.peek_at(1), K::Punct("(" | "=" | ";" | "}")) {
                        self.bump()?;
                    }
                    if self.is_word("accessor") && !matches!(self.peek_at(1), K::Punct("(" | "=" | ";" | "}")) {
                        self.bump()?;
                    }
                    self.property_definition(true)
                })();
                if r.is_ok() {
                    self.infer();
                }
                self.fni.close(top);
                r?;
            }
            Ok(())
        })();
        self.in_async = a;
        self.in_generator = g;
        r
    }

    fn hoistable_declaration(&mut self) -> R {
        let start = self.t[self.i].start;
        let is_async = self.is_word("async");
        if is_async {
            self.bump()?;
        }
        self.bump()?; // function
        let generator = self.eat("*");
        let name = match self.peek().clone() {
            K::Ident(w) => {
                self.bump()?;
                w
            }
            _ => String::new(),
        };
        let top = self.fni.open();
        self.fni.push_enclosing(&name);
        let r = self.function_rest(start, is_async, generator, Some(&name), false);
        self.fni.close(top);
        r.map(|_| ())
    }

    /// `function [*] [name] (...) {...}` at `function`.
    fn function_expression(&mut self, start: u32, is_async: bool) -> R<E> {
        let eager = std::mem::take(&mut self.pife);
        self.bump()?; // function
        let generator = self.eat("*");
        let mut name = None;
        if let K::Ident(w) = self.peek().clone() {
            if !self.is("(") && (self.ident().is_some() || w == "yield" || w == "await") {
                self.bump()?;
                name = Some(w);
            }
        }
        let anonymous = name.is_none();
        let f = self.function_rest(start, is_async, generator, name.as_deref(), eager)?;
        self.funcs[f].eager = eager;
        if anonymous {
            self.funcs[f].anonymous = true;
            self.fni.add(f);
            return Ok(E::Func(f));
        }
        Ok(E::Other)
    }

    /// Parameters and body of a function whose name (if any) is parsed;
    /// its index in `funcs`. `name` is the function's own name; a body V8
    /// compiles lazily is parsed with a fresh inferrer.
    fn function_rest(
        &mut self,
        start: u32,
        is_async: bool,
        generator: bool,
        name: Option<&str>,
        eager: bool,
    ) -> R<usize> {
        let (a, g) = (self.in_async, self.in_generator);
        self.in_async = is_async;
        self.in_generator = generator;
        let saved = (!eager).then(|| std::mem::take(&mut self.fni));
        if let Some(n) = name.filter(|_| !eager) {
            self.fni.push_enclosing(n);
        }
        let r = (|| {
            self.formal_parameters()?;
            self.function_body()
        })();
        if let Some(s) = saved {
            self.fni = s;
        }
        self.in_async = a;
        self.in_generator = g;
        r?;
        self.funcs.push(Func {
            start,
            end: self.last_end() - 1,
            anonymous: false,
            eager: false,
            name: None,
            accessor: None,
        });
        Ok(self.funcs.len() - 1)
    }

    fn formal_parameters(&mut self) -> R {
        self.expect("(")?;
        let no_in = std::mem::replace(&mut self.no_in, false);
        let r = (|| {
            while !self.eat(")") {
                let top = self.fni.open();
                let r = (|| {
                    self.eat("...");
                    let ident = self.binding_pattern()?;
                    if self.eat("=") {
                        let value = self.assignment_expression()?;
                        if ident {
                            self.named(value);
                        }
                    }
                    Ok(())
                })();
                self.fni.close(top);
                r?;
                if !self.is(")") {
                    self.expect(",")?;
                }
            }
            Ok(())
        })();
        self.no_in = no_in;
        r
    }

    fn function_body(&mut self) -> R {
        let no_in = std::mem::replace(&mut self.no_in, false);
        let r = self.block();
        self.no_in = no_in;
        r
    }

    /// An arrow function whose `(` is at the cursor.
    fn arrow(&mut self, start: u32, is_async: bool) -> R<E> {
        let eager = std::mem::take(&mut self.pife);
        let (a, g) = (self.in_async, self.in_generator);
        self.in_async = is_async;
        self.in_generator = false;
        let saved = std::mem::take(&mut self.fni);
        let r = self.formal_parameters();
        self.fni = saved;
        self.in_async = a;
        self.in_generator = g;
        r?;
        self.arrow_body(start, is_async, eager)
    }

    /// At `=>`: the body, then the arrow is added for name inference.
    fn arrow_body(&mut self, start: u32, is_async: bool, eager: bool) -> R<E> {
        self.expect("=>")?;
        let (a, g) = (self.in_async, self.in_generator);
        self.in_async = is_async;
        self.in_generator = false;
        let saved = (!eager).then(|| std::mem::take(&mut self.fni));
        let r = if self.is("{") {
            self.function_body()
        } else {
            // A concise body: an assignment expression (`in` stays as the
            // context has it).
            self.assignment_expression().map(|_| ())
        };
        if let Some(s) = saved {
            self.fni = s;
        }
        self.in_async = a;
        self.in_generator = g;
        r?;
        self.funcs.push(Func { start, end: self.last_end() - 1, anonymous: true, eager, name: None, accessor: None });
        let f = self.funcs.len() - 1;
        self.fni.add(f);
        Ok(E::Func(f))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(src: &str) -> Vec<(String, String)> {
        let mut v: Vec<_> = inferred_names(src)
            .into_iter()
            .map(|((s, e), (n, _))| {
                let text: String =
                    src.encode_utf16().skip(s as usize).take((e - s + 1) as usize).map(|u| u as u8 as char).collect();
                (s, text, n)
            })
            .collect();
        v.sort();
        v.into_iter().map(|(_, t, n)| (t, n)).collect()
    }

    fn pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
        v.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
    }

    #[test]
    fn names_as_v8() {
        // Each case checked against Chrome's precise coverage.
        let src = r#"var ns = {};
ns.a = function () { return 1; };
ns.b = () => 2;
ns.deep.c = function () { return 3; };
ns["lit"] = function () { return 4; };
ns[0] = function () { return 5; };
ns[key] = function () { return 6; };
function Ctor() { this.m = function () { return 7; }; }
Ctor.prototype.p = function () { return 8; };
function lower() { this.q = function () { return 9; }; }
var cond = true ? function () { return 11; } : null;
const pr = new Promise(r => r(12));
const called = (function () { return () => 13; })();
var o2 = { g: [function () { return 15; }] };
let x1; x1 = [() => 16];
var o3 = (function () { var self = {}; self.h = function () { return 17; }; return self; })();
async function pending() { const v = await new Promise(r => { setTimeout(r, 0); }); return v; }
foo2(function () {}, function () { return 21; });
var after = 1;
window.run = () => { return 1 / 2; };
"#;
        assert_eq!(
            names(src),
            pairs(&[
                ("function () { return 1; }", "ns.a"),
                ("() => 2", "ns.b"),
                ("function () { return 3; }", "ns.deep.c"),
                ("function () { return 4; }", "ns.lit"),
                ("function () { return 5; }", "ns.<computed>"),
                ("function () { return 6; }", "ns.<computed>"),
                ("function () { return 7; }", "Ctor.m"),
                ("function () { return 8; }", "Ctor.p"),
                ("function () { return 9; }", "q"),
                ("function () { return 11; }", "cond"),
                ("function () { return 15; }", "o2.g"),
                ("() => 16", "x1"),
                ("function () { return 17; }", "self.h"),
                ("r => { setTimeout(r, 0); }", "v"),
                ("function () {}", "after"),
                ("() => { return 1 / 2; }", "window.run"),
            ])
        );
    }

    #[test]
    fn templates_regexes_classes_and_parenthesized_functions() {
        // Checked against Chrome: a function right after `(` (compiled with
        // its parent) keeps no inferred name, but its body's names carry the
        // parent's.
        let src = "var B = class {}; var x = 1;
const t = `a${(() => 1)()}b${`c${x}`}`; const re = /[/]x/g; class A extends B { static s = 1; #p = () => 2; get g() { return 3; } m() {} }
window.z = { k: async () => 4 }.k;
window.q = `${x}` && function () { return 5; };
window.r = x / 2 / 1 ? function () { return 6; } : 0;
label: for (const [k, v] of Object.entries({})) { window.s = function* () { yield 7; }; break label; }
ns.p1 = (function () { return 8; });
h((() => 9), (W9 = {}));
ns.y = (function () { ns2.w = function () { return 10; }; return 1; }());
ns.v = function () { ns2.u = function () { return 11; }; return 1; }();
";
        assert_eq!(
            names(src),
            pairs(&[
                ("get g() { return 3; }", "get g"),
                ("function () { return 5; }", "window.q"),
                ("function () { return 6; }", "window.r"),
                ("function* () { yield 7; }", "window.s"),
                ("function () { return 10; }", "ns.y.ns2.w"),
                ("function () { return 11; }", "ns2.u"),
            ])
        );
    }

    /// `FN_NAMES_SRC=file cargo test -p pyro-cdp --lib fn_names::tests::dump -- --ignored --nocapture`
    /// prints `end<TAB>name` for each inferred name (end exclusive, UTF-16).
    #[test]
    #[ignore]
    fn dump() {
        let src = std::fs::read_to_string(std::env::var("FN_NAMES_SRC").unwrap()).unwrap();
        let funcs = anonymous_functions(&src);
        println!("parsed={}", funcs.is_some());
        for f in funcs.into_iter().flatten() {
            println!("NAME\t{}\t{}\t{}", f.start, f.end + 1, f.accessor.or(f.name).unwrap_or_default());
        }
    }

    /// Checked against Chrome: getters and setters are `get k` / `set k`;
    /// an object literal's methods with a computed key take the object's
    /// name, a class's get none.
    #[test]
    fn accessors_and_computed_methods() {
        let src = "var o = { get g() { return 1; }, set 'q r'(v) {}, get 5() {}, [k]() {}, get [k]() {}, m() {} };
class K { static get s() {} set #p(v) {} [k]() {} get [k]() {} }";
        assert_eq!(
            names(src),
            pairs(&[
                ("get g() { return 1; }", "get g"),
                ("set 'q r'(v) {}", "set q r"),
                ("get 5() {}", "get 5"),
                ("[k]() {}", "o"),
                ("get [k]() {}", "o"),
                ("get s() {}", "get s"),
                ("set #p(v) {}", "set #p"),
            ])
        );
    }

    #[test]
    fn unparsable_source_gives_nothing() {
        assert!(inferred_names("a.b = function () {").is_empty());
    }
}
