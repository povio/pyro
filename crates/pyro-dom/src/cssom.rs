//! Stylesheet text to CSSOM rules, for `CSSStyleSheet.cssRules`. Runs only
//! when a script asks: the style engine parses sheets on its own with Stylo.

use cssparser::{ParseError, Parser, ToCss, Token};

use crate::select;

/// `(prefix, url)` of the `@namespace` rules in scope; `""` is the default.
pub type Namespaces = Vec<(String, String)>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedRule {
    /// A style rule: serialized selector and the block's raw text.
    Style { selector: String, block: String },
    /// `@media` / `@supports`: name, condition and child rules.
    Group { name: String, condition: String, rules: Vec<ParsedRule> },
    /// Any other known at-rule, kept as written. `rule_type` is `CSSRule.type`.
    Other { rule_type: u16, text: String },
}

/// The rules of a stylesheet. Rules a browser would drop are dropped: invalid
/// selectors, unknown at-rules and a rule cut off before its block.
pub fn parse_rules(text: &str, namespaces: &mut Namespaces) -> Vec<ParsedRule> {
    let mut parser = Parser::new(text);
    parse_list(&mut parser, namespaces, true)
}

/// The serialized form of a valid selector list. Valid means what
/// `querySelector` accepts, plus the given namespace prefixes.
pub fn normalize_selector(text: &str, namespaces: &Namespaces) -> Option<String> {
    if let Ok(list) = select::parse_selectors_in(text, namespaces) {
        return Some(list.to_css_string());
    }
    crate::stylo::select::parse(text).map(|list| list.to_css_string())
}

fn parse_list(p: &mut Parser<'_>, namespaces: &mut Namespaces, top: bool) -> Vec<ParsedRule> {
    let mut rules = Vec::new();
    loop {
        let start = p.position();
        let Ok(token) = p.next_including_whitespace_and_comments().cloned() else { return rules };
        let rule = match token {
            Token::WhiteSpace(_) | Token::Comment(_) => continue,
            Token::CDO | Token::CDC if top => continue,
            Token::AtKeyword(name) => parse_at_rule(p, &name, namespaces),
            token => parse_style_rule(p, start, &token, namespaces),
        };
        rules.extend(rule);
    }
}

/// Reads up to `;`, a `{}` block or the end. Returns the prelude text and, for
/// a block, runs `block` inside it.
fn parse_prelude_and_block<'i, R>(
    p: &mut Parser<'i>,
    start: cssparser::SourcePosition,
    block: impl FnOnce(&mut Parser<'i>) -> R,
) -> (String, Option<R>) {
    loop {
        let before = p.position();
        match p.next_including_whitespace_and_comments() {
            Err(_) => return (p.slice_from(start).to_owned(), None),
            Ok(Token::Semicolon) => return (p.slice(start..before).to_owned(), None),
            Ok(Token::CurlyBracketBlock) => {
                let prelude = p.slice(start..before).to_owned();
                let result: Result<R, ParseError<()>> = p.parse_nested_block(|n| Ok(block(n)));
                return (prelude, result.ok());
            }
            // Past the closing bracket, so `before` above is a token boundary.
            Ok(Token::Function(_) | Token::ParenthesisBlock | Token::SquareBracketBlock) => skip_block(p),
            Ok(_) => {}
        }
    }
}

fn skip_block(p: &mut Parser<'_>) {
    let _: Result<(), ParseError<()>> = p.parse_nested_block(|n| {
        while n.next_including_whitespace_and_comments().is_ok() {}
        Ok(())
    });
}

/// The rest of a block's text, leaving the parser at its end.
fn raw_block(n: &mut Parser<'_>) -> String {
    let start = n.position();
    while n.next_including_whitespace_and_comments().is_ok() {}
    n.slice_from(start).to_owned()
}

fn parse_style_rule(
    p: &mut Parser<'_>,
    start: cssparser::SourcePosition,
    first: &Token<'_>,
    namespaces: &Namespaces,
) -> Option<ParsedRule> {
    match first {
        // The prelude's first token is the block: `{}` has no selector.
        Token::CurlyBracketBlock => {
            skip_block(p);
            return None;
        }
        Token::Function(_) | Token::ParenthesisBlock | Token::SquareBracketBlock => skip_block(p),
        _ => {}
    }
    let (prelude, block) = parse_prelude_and_block(p, start, raw_block);
    let selector = normalize_selector(&prelude, namespaces)?;
    Some(ParsedRule::Style { selector, block: block? })
}

fn collapse(text: &str) -> String {
    text.split_ascii_whitespace().collect::<Vec<_>>().join(" ")
}

fn parse_at_rule(p: &mut Parser<'_>, name: &str, namespaces: &mut Namespaces) -> Option<ParsedRule> {
    let lower = name.to_ascii_lowercase();
    let start = p.position();
    if matches!(lower.as_str(), "media" | "supports") {
        let mut inner = namespaces.clone();
        let (condition, rules) = parse_prelude_and_block(p, start, |n| parse_list(n, &mut inner, false));
        return Some(ParsedRule::Group { name: lower, condition: collapse(&condition), rules: rules? });
    }
    let (prelude, block) = parse_prelude_and_block(p, start, raw_block);
    let rule_type = match lower.as_str() {
        "import" => 3,
        "font-face" => 5,
        "page" => 6,
        "keyframes" | "-webkit-keyframes" => 7,
        "namespace" => 10,
        "counter-style" => 11,
        "font-feature-values" => 14,
        "layer" | "container" | "property" | "font-palette-values" | "starting-style" | "scope" => 0,
        _ => return None,
    };
    if lower == "namespace" {
        namespaces.push(parse_namespace(&prelude)?);
    }
    let text = match block {
        Some(block) => format!("@{name} {} {{{block}}}", collapse(&prelude)),
        None => format!("@{name} {};", collapse(&prelude)),
    };
    Some(ParsedRule::Other { rule_type, text })
}

/// `@namespace [prefix] "url" | url("url")`.
fn parse_namespace(prelude: &str) -> Option<(String, String)> {
    let mut p = Parser::new(prelude);
    let prefix = p.try_parse(|p| p.expect_ident().cloned()).ok();
    let url = p.expect_url_or_string().ok()?;
    p.expect_exhausted().ok()?;
    Some((prefix.map_or_else(String::new, |n| n.to_string()), url.to_string()))
}

/// The URLs of the `@import` rules a style sheet starts with, as written
/// (to resolve against the sheet's URL), in order: what a browser fetches
/// for the sheet. As in CSS Cascade, an `@import` counts only before every
/// other rule except `@charset` and `@layer` statements, and one whose
/// `supports()` condition fails imports nothing; its media queries do not
/// matter (the sheet is fetched whatever they say). Reads no further than
/// the first other rule, so a sheet without imports costs a token or two.
pub fn leading_imports(text: &str) -> Vec<String> {
    let mut p = Parser::new(text);
    let mut urls = Vec::new();
    loop {
        let Ok(token) = p.next_including_whitespace_and_comments().cloned() else { return urls };
        match token {
            Token::WhiteSpace(_) | Token::Comment(_) | Token::CDO | Token::CDC => {}
            Token::AtKeyword(name) if name.eq_ignore_ascii_case("import") => {
                if let Some(url) = import_prelude(&mut p) {
                    urls.push(url);
                }
            }
            // Statements: `@layer a, b;` (a `@layer` block is a rule, after
            // which imports are invalid).
            Token::AtKeyword(name) if name.eq_ignore_ascii_case("charset") || name.eq_ignore_ascii_case("layer") => {
                if !statement_ends(&mut p) {
                    return urls;
                }
            }
            _ => return urls,
        }
    }
}

/// The rest of an `@import` rule: its URL, unless the rule is invalid (no
/// URL, or a block) or its `supports()` condition fails.
fn import_prelude(p: &mut Parser<'_>) -> Option<String> {
    let url = match p.next().cloned() {
        // The rule's end (its block is skipped by the next read).
        Err(_) | Ok(Token::Semicolon | Token::CurlyBracketBlock) => return None,
        Ok(Token::UnquotedUrl(url) | Token::QuotedString(url)) => Some(url.to_string()),
        Ok(Token::Function(name)) if name.eq_ignore_ascii_case("url") => {
            let url: Result<String, ParseError<()>> = p.parse_nested_block(|n| Ok(n.expect_string()?.to_string()));
            url.ok()
        }
        _ => None,
    };
    let mut supported = true;
    loop {
        match p.next().cloned() {
            Err(_) | Ok(Token::Semicolon) => break,
            Ok(Token::CurlyBracketBlock) => return None,
            Ok(Token::Function(name)) if name.eq_ignore_ascii_case("supports") => {
                let condition: Result<String, ParseError<()>> = p.parse_nested_block(|n| Ok(raw_block(n)));
                supported &= condition.is_ok_and(|c| crate::stylo::supports_condition(&c));
            }
            Ok(_) => {}
        }
    }
    url.filter(|_| supported)
}

/// Skips a statement at-rule's prelude: true at its `;` (or the end), false
/// at a block (the parser is then past it).
fn statement_ends(p: &mut Parser<'_>) -> bool {
    loop {
        match p.next() {
            Err(_) | Ok(Token::Semicolon) => return true,
            Ok(Token::CurlyBracketBlock) => return false,
            Ok(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn style(selector: &str, block: &str) -> ParsedRule {
        ParsedRule::Style { selector: selector.into(), block: block.into() }
    }

    #[test]
    fn rules_are_parsed_and_invalid_ones_dropped() {
        let rules = parse_rules(
            "<!-- a{color:red} /* c */ b,, c{} [x y]{} [z]:not(q){} @foo bar; @media  all  { d{} e!{} } {} f",
            &mut Vec::new(),
        );
        assert_eq!(
            rules,
            [
                style("a", "color:red"),
                style("[z]:not(q)", ""),
                ParsedRule::Group { name: "media".into(), condition: "all".into(), rules: vec![style("d", "")] },
            ]
        );
    }

    #[test]
    fn selectors_serialize_canonically() {
        let ns = Vec::new();
        for (input, expected) in [
            ("[foo='bar' /**/ I]", "[foo=\"bar\" i]"),
            ("[*|foo='bar' s]", "[*|foo=\"bar\" s]"),
            (":nth-child(1n+0)", ":nth-child(n)"),
            (":nth-child(+n+3)", ":nth-child(n+3)"),
            (":nth-child( 23n\n\n+\n\n123 )", ":nth-child(23n+123)"),
            (":NTH-LAST-OF-TYPE(-n\n- 1)", ":nth-last-of-type(-n-1)"),
        ] {
            assert_eq!(normalize_selector(input, &ns).as_deref(), Some(expected), "{input}");
        }
        for input in ["", ":nth-child(n- 1 2)", ":nth-child(-n+n)", "[foo='bar' i i]", "[foo='bar' *]"] {
            assert_eq!(normalize_selector(input, &ns), None, "{input}");
        }
    }

    #[test]
    fn leading_imports_are_the_ones_a_browser_fetches() {
        assert_eq!(
            leading_imports(
                "<!-- @charset 'utf-8'; /* x */ @import url(a.css); @IMPORT 'b.css' print; @layer x, y;\n\
                 @import url( \"c.css\" ) layer(x) supports(display: grid) screen and (min-width: 1px);\
                 @import url(d.css) supports(nonsense: 1); @import; @import e.css; @import 'f.css' {}\
                 @import \"g.css\"; p { color: red } @import 'h.css';"
            ),
            ["a.css", "b.css", "c.css", "g.css"]
        );
        // A rule, a `@layer` block or `@namespace` ends them; none at all.
        assert_eq!(leading_imports("@layer x {} @import 'a.css';"), Vec::<String>::new());
        assert_eq!(leading_imports("@namespace svg url(x); @import 'a.css';"), Vec::<String>::new());
        assert_eq!(leading_imports("body { color: red }"), Vec::<String>::new());
        assert_eq!(leading_imports(""), Vec::<String>::new());
        // The last rule may end with the sheet.
        assert_eq!(leading_imports("@import url(last.css)"), ["last.css"]);
    }

    #[test]
    fn namespace_rules_declare_prefixes() {
        let mut ns = Vec::new();
        let rules = parse_rules("@namespace x 'a'; [x|foo='' i]{} [y|foo]{}", &mut ns);
        assert_eq!(ns, [("x".to_owned(), "a".to_owned())]);
        assert_eq!(
            rules,
            [ParsedRule::Other { rule_type: 10, text: "@namespace x 'a';".into() }, style("[x|foo=\"\" i]", "")]
        );
    }
}
