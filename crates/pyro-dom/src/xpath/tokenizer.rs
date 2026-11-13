//! XPath 1.0 lexer (XPath §3.7), ported from upstream
//! src/browser/xpath/Tokenizer.zig. Unlike upstream (which mirrors a lenient
//! JS polyfill), unknown characters and unterminated literals are syntax
//! errors, as in browsers. Operator names (`and`/`or`/`div`/`mod`) and `*`
//! are lexed as plain names/stars; the parser disambiguates them by position.

use super::XPathError;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Tok {
    /// String literal, quotes stripped (XPath literals have no escapes).
    Literal(String),
    Number(f64),
    /// NCName or QName (`prefix:local`).
    Name(String),
    /// `prefix:*`.
    PrefixStar(String),
    /// `$name`.
    Var(String),
    Slash,
    DoubleSlash,
    Dot,
    DoubleDot,
    At,
    LParen,
    RParen,
    LBracket,
    RBracket,
    Comma,
    Pipe,
    Eq,
    Neq,
    Lt,
    Lte,
    Gt,
    Gte,
    Plus,
    Minus,
    Star,
    DoubleColon,
    Eof,
}

fn is_name_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_' || !c.is_ascii()
}

fn is_name_char(c: char) -> bool {
    is_name_start(c) || c.is_ascii_digit() || c == '-' || c == '.'
}

/// XML whitespace (S production).
pub(crate) fn is_xml_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r')
}

fn syntax(src: &str) -> XPathError {
    XPathError::Syntax(format!("The string '{src}' is not a valid XPath expression."))
}

struct Lexer<'a> {
    src: &'a str,
    pos: usize,
}

impl Lexer<'_> {
    fn peek(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }
    fn peek_at(&self, n: usize) -> Option<char> {
        self.src[self.pos..].chars().nth(n)
    }
    fn ncname(&mut self) -> &str {
        let start = self.pos;
        while let Some(c) = self.peek().filter(|&c| is_name_char(c)) {
            self.pos += c.len_utf8();
        }
        &self.src[start..self.pos]
    }
}

pub(crate) fn tokenize(src: &str) -> Result<Vec<Tok>, XPathError> {
    let mut lx = Lexer { src, pos: 0 };
    let mut out = Vec::new();
    loop {
        while lx.peek().is_some_and(is_xml_space) {
            lx.pos += 1;
        }
        let Some(c) = lx.peek() else { break };
        let next = lx.peek_at(1);
        let two = |t: Tok, lx: &mut Lexer<'_>| {
            lx.pos += 2;
            t
        };
        let tok = match c {
            '"' | '\'' => {
                let start = lx.pos + 1;
                let end = src[start..].find(c).ok_or_else(|| syntax(src))? + start;
                lx.pos = end + 1;
                Tok::Literal(src[start..end].to_owned())
            }
            '0'..='9' => number(&mut lx),
            '.' if next.is_some_and(|n| n.is_ascii_digit()) => number(&mut lx),
            '.' if next == Some('.') => two(Tok::DoubleDot, &mut lx),
            '/' if next == Some('/') => two(Tok::DoubleSlash, &mut lx),
            ':' if next == Some(':') => two(Tok::DoubleColon, &mut lx),
            '!' if next == Some('=') => two(Tok::Neq, &mut lx),
            '<' if next == Some('=') => two(Tok::Lte, &mut lx),
            '>' if next == Some('=') => two(Tok::Gte, &mut lx),
            '$' => {
                lx.pos += 1;
                if !lx.peek().is_some_and(is_name_start) {
                    return Err(syntax(src));
                }
                let name = qname(&mut lx, src)?;
                match name {
                    Tok::Name(n) => Tok::Var(n),
                    _ => return Err(syntax(src)),
                }
            }
            c if is_name_start(c) => qname(&mut lx, src)?,
            _ => {
                let t = match c {
                    '.' => Tok::Dot,
                    '/' => Tok::Slash,
                    '@' => Tok::At,
                    '(' => Tok::LParen,
                    ')' => Tok::RParen,
                    '[' => Tok::LBracket,
                    ']' => Tok::RBracket,
                    ',' => Tok::Comma,
                    '|' => Tok::Pipe,
                    '=' => Tok::Eq,
                    '<' => Tok::Lt,
                    '>' => Tok::Gt,
                    '+' => Tok::Plus,
                    '-' => Tok::Minus,
                    '*' => Tok::Star,
                    _ => return Err(syntax(src)),
                };
                lx.pos += 1;
                t
            }
        };
        out.push(tok);
    }
    out.push(Tok::Eof);
    Ok(out)
}

fn number(lx: &mut Lexer<'_>) -> Tok {
    let start = lx.pos;
    while lx.peek().is_some_and(|c| c.is_ascii_digit()) {
        lx.pos += 1;
    }
    if lx.peek() == Some('.') {
        lx.pos += 1;
        while lx.peek().is_some_and(|c| c.is_ascii_digit()) {
            lx.pos += 1;
        }
    }
    // `\d+(\.\d*)?` or `\.\d+`: always valid for f64 parsing.
    Tok::Number(lx.src[start..lx.pos].parse().unwrap_or(f64::NAN))
}

/// NCName, `prefix:local` or `prefix:*`. A `::` after the name is the axis
/// separator and stays for the next token.
fn qname(lx: &mut Lexer<'_>, src: &str) -> Result<Tok, XPathError> {
    let first = lx.ncname().to_owned();
    if lx.peek() == Some(':') && lx.peek_at(1) != Some(':') {
        match lx.peek_at(1) {
            Some('*') => {
                lx.pos += 2;
                return Ok(Tok::PrefixStar(first));
            }
            Some(c) if is_name_start(c) => {
                lx.pos += 1;
                let local = lx.ncname();
                return Ok(Tok::Name(format!("{first}:{local}")));
            }
            _ => return Err(syntax(src)),
        }
    }
    Ok(Tok::Name(first))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(s: &str) -> Vec<Tok> {
        tokenize(s).unwrap()
    }

    #[test]
    fn operators_and_names() {
        assert_eq!(
            toks("//div[@class='x']/p[2]"),
            vec![
                Tok::DoubleSlash,
                Tok::Name("div".into()),
                Tok::LBracket,
                Tok::At,
                Tok::Name("class".into()),
                Tok::Eq,
                Tok::Literal("x".into()),
                Tok::RBracket,
                Tok::Slash,
                Tok::Name("p".into()),
                Tok::LBracket,
                Tok::Number(2.0),
                Tok::RBracket,
                Tok::Eof
            ]
        );
        assert_eq!(
            toks("child::a != .. <= >= ."),
            vec![
                Tok::Name("child".into()),
                Tok::DoubleColon,
                Tok::Name("a".into()),
                Tok::Neq,
                Tok::DoubleDot,
                Tok::Lte,
                Tok::Gte,
                Tok::Dot,
                Tok::Eof
            ]
        );
    }

    #[test]
    fn numbers_literals_qnames() {
        assert_eq!(toks(".5 5. 007")[..3], [Tok::Number(0.5), Tok::Number(5.0), Tok::Number(7.0)]);
        assert_eq!(toks("\"it's\"")[0], Tok::Literal("it's".into()));
        assert_eq!(toks("svg:rect")[0], Tok::Name("svg:rect".into()));
        assert_eq!(toks("svg:*")[0], Tok::PrefixStar("svg".into()));
        assert_eq!(toks("$x")[0], Tok::Var("x".into()));
        assert_eq!(toks("foo-bar.baz")[0], Tok::Name("foo-bar.baz".into()));
        assert_eq!(toks("h-[2.625rem]")[0], Tok::Name("h-".into()));
    }

    #[test]
    fn lexical_errors() {
        assert!(tokenize("'open").is_err());
        assert!(tokenize("a ? b").is_err());
        assert!(tokenize("a:").is_err());
        assert!(tokenize("$").is_err());
    }
}
