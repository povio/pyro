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
    let mut rules: Vec<ParsedRule> = Vec::new();
    // `@import` and `@namespace` only count before every other rule (`@layer`
    // statements aside); later ones are dropped, as browsers drop them.
    let mut past_prologue = false;
    loop {
        let start = p.position();
        let Ok(token) = p.next_including_whitespace_and_comments().cloned() else { return rules };
        let rule = match token {
            Token::WhiteSpace(_) | Token::Comment(_) => continue,
            Token::CDO | Token::CDC if top => continue,
            Token::AtKeyword(name) => parse_at_rule(p, &name, namespaces),
            token => parse_style_rule(p, start, &token, namespaces),
        };
        let Some(rule) = rule else { continue };
        if top {
            let prologue = match &rule {
                ParsedRule::Other { rule_type: 3 | 10, .. } => true,
                ParsedRule::Other { rule_type: 0, text } => text.starts_with("@layer") && text.ends_with(';'),
                _ => false,
            };
            if prologue && past_prologue && matches!(rule, ParsedRule::Other { rule_type: 3 | 10, .. }) {
                continue;
            }
            past_prologue |= !prologue;
        }
        rules.push(rule);
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

/// An `@import` prelude with its URL serialized as CSSOM does: `url("...")`.
fn import_url_form(prelude: &str) -> String {
    let mut p = Parser::new(prelude);
    let url = match p.next() {
        Ok(Token::QuotedString(v)) | Ok(Token::UnquotedUrl(v)) => v.to_string(),
        Ok(Token::Function(f)) if f.eq_ignore_ascii_case("url") => {
            let inner: Result<String, ParseError<()>> =
                p.parse_nested_block(|n| n.expect_string().map(|v| v.to_string()).map_err(Into::into));
            match inner {
                Ok(v) => v,
                Err(_) => return prelude.to_owned(),
            }
        }
        _ => return prelude.to_owned(),
    };
    let mut out = String::from("url(");
    let _ = cssparser::serialize_string(&url, &mut out);
    out.push(')');
    // The layer and supports() parts as written, the media queries serialized.
    let (head, media) = import_split(prelude);
    let modifiers = head[head.len().min(p.position().byte_index())..].trim().to_owned();
    let modifiers = if head.len() >= p.position().byte_index() { modifiers } else { String::new() };
    for part in [modifiers, if media.is_empty() { String::new() } else { serialize_media_list(&media) }] {
        if !part.is_empty() {
            out.push(' ');
            out.push_str(&part);
        }
    }
    out
}

/// A `layer()` argument that names a layer: dot-separated identifiers.
pub fn valid_layer_name(inner: &str) -> bool {
    let inner = inner.trim();
    !inner.is_empty()
        && inner.split('.').all(|part| {
            !part.is_empty() && part.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_' || !c.is_ascii())
        })
}

/// An `@import` prelude split before its media queries: the URL, `layer`
/// and `supports()` parts, and the media query text.
pub fn import_split(prelude: &str) -> (String, String) {
    let mut p = cssparser::Parser::new(prelude);
    let mut end = p.position();
    let mut first = true;
    loop {
        let state = p.state();
        let Ok(token) = p.next().cloned() else { break };
        let keep = match &token {
            _ if first => true,
            cssparser::Token::Ident(i) => i.eq_ignore_ascii_case("layer"),
            cssparser::Token::Function(f) => f.eq_ignore_ascii_case("layer") || f.eq_ignore_ascii_case("supports"),
            _ => false,
        };
        if !keep {
            p.reset(&state);
            break;
        }
        if let cssparser::Token::Function(f) = &token {
            let inner_start = p.position();
            let _: Result<(), cssparser::ParseError<()>> = p.parse_nested_block(|n| {
                while n.next().is_ok() {}
                Ok(())
            });
            // An invalid `layer()` starts the media queries.
            let inner = p.slice_from(inner_start);
            if !first && f.eq_ignore_ascii_case("layer") && !valid_layer_name(inner.strip_suffix(')').unwrap_or(inner)) {
                p.reset(&state);
                break;
            }
        }
        first = false;
        end = p.position();
    }
    let head = prelude[..end.byte_index()].trim().to_owned();
    let media = prelude[end.byte_index()..].trim().to_owned();
    (head, media)
}



fn parse_at_rule(p: &mut Parser<'_>, name: &str, namespaces: &mut Namespaces) -> Option<ParsedRule> {
    let lower = name.to_ascii_lowercase();
    let start = p.position();
    if matches!(lower.as_str(), "media" | "supports") {
        let mut inner = namespaces.clone();
        let (condition, rules) = parse_prelude_and_block(p, start, |n| parse_list(n, &mut inner, false));
        let condition = if lower == "media" { serialize_media_list(&condition) } else { collapse(&condition) };
        return Some(ParsedRule::Group { name: lower, condition, rules: rules? });
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
        "layer" | "container" | "property" | "font-palette-values" | "starting-style" | "scope" | "position-try"
        | "view-transition" => 0,
        _ => return None,
    };
    if lower == "namespace" {
        namespaces.push(parse_namespace(&prelude)?);
    }
    // `@name prelude {block}`, without a second space when the prelude is empty.
    let prelude = collapse(&prelude);
    let prelude = if lower == "import" { import_url_form(&prelude) } else { prelude };
    let head = if prelude.is_empty() { format!("@{name}") } else { format!("@{name} {prelude}") };
    let text = match block {
        // An empty descriptor block serializes as `{ }`.
        Some(block)
            if block.trim().is_empty()
                && matches!(lower.as_str(), "page" | "font-face" | "property" | "counter-style" | "font-palette-values" | "position-try" | "view-transition") =>
        {
            format!("{head} {{ }}")
        }
        Some(block) => format!("{head} {{{block}}}"),
        None => format!("{head};"),
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

/// CSS Syntax's decode of a style sheet's bytes: a byte order mark decides;
/// else the protocol encoding (`protocol`, the `Content-Type` charset); else
/// an `@charset "label";` the bytes start with (UTF-16 meaning UTF-8); else
/// the environment encoding, the first of `environment` that names one (the
/// referrer's: a `<link charset>`, then its document's encoding; for an
/// imported sheet the importing one's); else UTF-8. Encodings are labels, and
/// one that names none counts as absent. Returns the text, without the mark,
/// and the name of the encoding it was decoded with (the environment of the
/// sheets it imports).
pub fn decode_stylesheet<'a>(
    bytes: &'a [u8],
    protocol: Option<&str>,
    environment: &[&str],
) -> (std::borrow::Cow<'a, str>, &'static str) {
    struct EncodingRs;
    impl cssparser::EncodingSupport for EncodingRs {
        type Encoding = &'static encoding_rs::Encoding;
        fn from_label(label: &[u8]) -> Option<Self::Encoding> {
            encoding_rs::Encoding::for_label(label)
        }
        fn utf8() -> Self::Encoding {
            encoding_rs::UTF_8
        }
        fn is_utf16_be_or_le(encoding: &Self::Encoding) -> bool {
            *encoding == encoding_rs::UTF_16BE || *encoding == encoding_rs::UTF_16LE
        }
    }
    let environment = environment.iter().find_map(|label| encoding_rs::Encoding::for_label(label.trim().as_bytes()));
    let fallback = cssparser::stylesheet_encoding::<EncodingRs>(bytes, protocol.map(str::as_bytes), environment);
    // Decoding sniffs the mark first.
    let (text, used, _) = fallback.decode(bytes);
    (text, used.name())
}

/// [`decode_stylesheet`] as bytes a style sheet parser decodes as they are:
/// UTF-8 behind a byte order mark, which outranks every other source of the
/// encoding (an `@charset` the text still starts with included).
pub fn stylesheet_utf8_bytes(text: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(text.len() + 3);
    bytes.extend_from_slice(b"\xEF\xBB\xBF");
    bytes.extend_from_slice(text.as_bytes());
    bytes
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

    /// CSS Syntax's order (WPT css/CSS2/syntax/at-charset-*): byte order mark, protocol
    /// encoding, `@charset`, environment encoding, UTF-8.
    #[test]
    fn style_sheets_decode_by_mark_protocol_charset_rule_and_environment() {
        const SHIFT_JIS: &str = "Shift_JIS";
        const UTF_8: &str = "UTF-8";
        const WINDOWS_1252: &str = "windows-1252";
        // `.平和` in Shift_JIS.
        let sjis = b".\x95\xbd\x98\x61{}";
        let with_rule = |label: &str, body: &[u8]| [format!("@charset \"{label}\";").as_bytes(), body].concat();
        let decode = |bytes: &[u8], protocol, environment: Option<&str>| {
            let (text, used) = decode_stylesheet(bytes, protocol, environment.as_slice());
            (text.into_owned(), used)
        };
        // The protocol encoding wins over `@charset` and the environment.
        let (text, used) = decode(&with_rule("windows-1252", sjis), Some("shift_jis"), Some(WINDOWS_1252));
        assert_eq!((text.ends_with(".平和{}"), used), (true, SHIFT_JIS));
        // `@charset` over the environment (a `<link charset>`).
        let (text, used) = decode(&with_rule("shift-JIS", sjis), None, Some(WINDOWS_1252));
        assert_eq!((text.ends_with(".平和{}"), used), (true, SHIFT_JIS));
        // ISO-8859-1 is windows-1252: `.t\xE9st`.
        let (text, _) = decode(&with_rule("iso-8859-1", b".t\xe9st{}"), Some(""), None);
        assert!(text.ends_with(".tést{}"), "{text}");
        // The environment, then UTF-8.
        assert_eq!(decode(sjis, None, Some(SHIFT_JIS)), (".平和{}".into(), SHIFT_JIS));
        assert_eq!(decode(".平和{}".as_bytes(), None, None), (".平和{}".into(), UTF_8));
        // `@charset "UTF-16..."` means UTF-8; a UTF-16 environment decodes as UTF-16.
        assert_eq!(decode(&with_rule("utf-16be", "a{}".as_bytes()), None, None).1, UTF_8);
        let utf16: Vec<u8> = "b{}".encode_utf16().flat_map(u16::to_be_bytes).collect();
        assert_eq!(decode(&utf16, None, Some("UTF-16BE")).0, "b{}");
        // A label that names no encoding is no environment: the next one is.
        assert_eq!(decode(".é{}".as_bytes(), None, Some("no-such-encoding")).1, UTF_8);
        assert_eq!(decode_stylesheet(sjis, None, &["", "x-unknown", "shift-jis"]).1, SHIFT_JIS);
        // A byte order mark outranks all of them, and goes.
        let marked = [b"\xEF\xBB\xBF".as_slice(), &with_rule("shift-JIS", ".é{}".as_bytes())].concat();
        let (text, used) = decode(&marked, Some("Shift_JIS"), Some(SHIFT_JIS));
        assert_eq!((text.as_str(), used), ("@charset \"shift-JIS\";.é{}", UTF_8));
        // As bytes for a parser that decodes them itself: UTF-8 behind a mark.
        assert_eq!(stylesheet_utf8_bytes("é"), b"\xEF\xBB\xBF\xC3\xA9");
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

    /// `@position-try` and `@view-transition` are known at-rules (WPT css-anchor-position
    /// chrome-377324991-crash inserts one with `insertRule`).
    #[test]
    fn position_try_and_view_transition_rules_are_kept() {
        let rules = parse_rules("@position-try --opt {} @view-transition { navigation: auto }", &mut Vec::new());
        assert_eq!(
            rules,
            [
                ParsedRule::Other { rule_type: 0, text: "@position-try --opt { }".into() },
                ParsedRule::Other { rule_type: 0, text: "@view-transition { navigation: auto }".into() },
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

/// A media query list as CSSOM serializes it (`MediaList.mediaText`): parsed
/// by the style engine, so keywords lowercase, whitespace collapses and an
/// invalid query becomes `not all`.
pub fn serialize_media_list(text: &str) -> String {
    use style::stylesheets::{CssRuleType, Origin, UrlExtraData};
    use style_traits::ToCss;
    crate::stylo::enable_features();
    let Ok(url) = url::Url::parse("about:blank") else { return String::new() };
    let url = UrlExtraData(style::servo_arc::Arc::new(url));
    let mut context = style::parser::ParserContext::new(
        Origin::Author,
        &url,
        Some(CssRuleType::Media),
        style_traits::ParsingMode::DEFAULT,
        style::context::QuirksMode::NoQuirks,
        Default::default(),
        None,
        None,
        Default::default(),
    );
    let mut parser = Parser::new(text);
    style::media_queries::MediaList::parse(&mut context, &mut parser).to_css_string()
}

#[cfg(test)]
mod media_list_tests {
    #[test]
    fn media_lists_serialize_as_cssom_does() {
        use super::serialize_media_list as m;
        assert_eq!(m("SCREEN"), "screen");
        assert_eq!(m("  screen  ,  print  "), "screen, print");
        assert_eq!(m("NOT  PRINT"), "not print");
        assert_eq!(m("screen, bad$"), "screen, not all");
        assert_eq!(m(""), "");
    }
}
