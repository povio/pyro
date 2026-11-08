//! Speculative preload scan (HTML "speculative HTML parsing", Chromium's
//! preload scanner): a tokenizer light enough to run over the buffered rest
//! of a document while the parser waits for a blocking script, finding the
//! start tags whose resources the document will request (`<script src>`,
//! `<link rel=stylesheet>`, `<link rel=preload>`, `<img src>`) and the tags
//! that change how later ones are requested (`<base href>`, `<meta
//! name=referrer>`). It inserts nothing: the browser starts the fetches, and
//! the elements take the responses when the parser reaches them.
//!
//! What it skips, as the real tokenizer never sees a tag there: comments and
//! bogus comments, the contents of raw text elements (`script`, `style`,
//! `textarea`, `title`, `xmp`, `iframe`, `noembed`, `noframes`, and
//! `noscript`, since the scan only runs with scripting on), `<template>`
//! contents (inert), foreign content (`<svg>`, `<math>`) until a tag that
//! breaks out of it, and everything after `<plaintext>`. An `<img>` inside
//! `<picture>` or with a `srcset` is left to the element (its source
//! depends on the viewport). A tag the chunk cuts off is dropped.

use std::borrow::Cow;

use markup5ever::data::NAMED_ENTITIES;

/// A resource (or a request-changing tag) the scan found, attribute values
/// as the tokenizer would produce them (character references decoded).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prescanned {
    /// `<script src>` the document will fetch: a classic script, or a
    /// module (`type=module`).
    Script { src: String, module: bool, crossorigin: Option<String> },
    /// `<link rel=stylesheet href>` (enabled, CSS type).
    Stylesheet { href: String, crossorigin: Option<String> },
    /// `<link rel=preload href as>` (not a modulepreload).
    Preload { href: String, as_: Option<String>, crossorigin: Option<String> },
    /// `<img src>` without `srcset` and outside `<picture>`.
    Image { src: String },
    /// The first `<base href>`: later URLs resolve against it.
    Base { href: String },
    /// `<meta name=referrer>`: later requests may carry another referrer
    /// policy, so the scan stops here.
    ReferrerPolicy,
}

/// Scanner state carried across the chunks of one document's input (the
/// parser's remaining input may grow at the front with written markup).
#[derive(Default)]
pub struct Prescan {
    template_depth: usize,
    picture_depth: usize,
    foreign_depth: usize,
    base_seen: bool,
    stopped: bool,
}

/// Elements whose content is text up to their end tag (RAWTEXT, RCDATA,
/// script data; `noscript` with scripting enabled).
const RAW_TEXT: &[&str] = &["script", "style", "textarea", "title", "xmp", "iframe", "noembed", "noframes", "noscript"];

/// HTML start tags that end foreign content (HTML "in foreign content").
const BREAKOUT: &[&str] = &[
    "b",
    "big",
    "blockquote",
    "body",
    "br",
    "center",
    "code",
    "dd",
    "div",
    "dl",
    "dt",
    "em",
    "embed",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "head",
    "hr",
    "i",
    "img",
    "li",
    "listing",
    "menu",
    "meta",
    "nobr",
    "ol",
    "p",
    "pre",
    "ruby",
    "s",
    "small",
    "span",
    "strong",
    "strike",
    "sub",
    "sup",
    "table",
    "tt",
    "u",
    "ul",
    "var",
];

/// A tag's attributes as written (names in their case, values decoded).
type Attrs<'a> = Vec<(&'a str, Cow<'a, str>)>;

fn attr<'a>(attrs: &'a Attrs<'a>, name: &str) -> Option<&'a str> {
    attrs.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| &**v)
}

fn is_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\x0c' | b'\r')
}

fn ascii(b: &[u8]) -> &str {
    std::str::from_utf8(b).unwrap_or("")
}

impl Prescan {
    /// Whether a `<plaintext>` or `<meta name=referrer>` ended the scan.
    pub fn stopped(&self) -> bool {
        self.stopped
    }

    /// Scans one chunk of markup, reporting what it finds in document order.
    pub fn scan(&mut self, input: &str, mut emit: impl FnMut(Prescanned)) {
        let b = input.as_bytes();
        let mut pos = 0;
        while !self.stopped && pos < b.len() {
            let Some(off) = b[pos..].iter().position(|&c| c == b'<') else { break };
            let i = pos + off;
            match b.get(i + 1) {
                Some(b'!') => {
                    if b[i + 1..].starts_with(b"!--") {
                        // A comment; `<!-->` and `<!--->` close at once.
                        let body = i + 4;
                        let end = if b[body..].starts_with(b">") {
                            Some(body + 1)
                        } else if b[body..].starts_with(b"->") {
                            Some(body + 2)
                        } else {
                            comment_end(b, body)
                        };
                        match end {
                            Some(e) => pos = e,
                            None => return,
                        }
                    } else {
                        // DOCTYPE, or a bogus comment (`<![CDATA[` in HTML).
                        match b[i..].iter().position(|&c| c == b'>') {
                            Some(e) => pos = i + e + 1,
                            None => return,
                        }
                    }
                }
                Some(b'?') => match b[i..].iter().position(|&c| c == b'>') {
                    Some(e) => pos = i + e + 1,
                    None => return,
                },
                Some(b'/') => {
                    if b.get(i + 2).is_some_and(u8::is_ascii_alphabetic) {
                        let Some((name, _, _, end)) = parse_tag(input, i + 2) else { return };
                        pos = end;
                        self.end_tag(name);
                    } else {
                        match b[i..].iter().position(|&c| c == b'>') {
                            Some(e) => pos = i + e + 1,
                            None => return,
                        }
                    }
                }
                Some(c) if c.is_ascii_alphabetic() => {
                    let Some((name, attrs, self_closing, end)) = parse_tag(input, i + 1) else { return };
                    pos = end;
                    if let Some(skip_to) = self.start_tag(name, &attrs, self_closing, &mut emit) {
                        // Raw text: continue after the element's end tag.
                        match raw_text_end(b, pos, skip_to) {
                            Some(e) => pos = e,
                            None => return,
                        }
                    }
                }
                Some(_) => pos = i + 1,
                None => return,
            }
        }
    }

    /// Handles a start tag; returns the raw-text element name whose
    /// contents follow, if any.
    fn start_tag(
        &mut self,
        name: &str,
        attrs: &Attrs<'_>,
        self_closing: bool,
        emit: &mut impl FnMut(Prescanned),
    ) -> Option<&'static str> {
        let is = |tag: &str| name.eq_ignore_ascii_case(tag);
        if self.foreign_depth > 0 {
            let breakout = BREAKOUT.iter().any(|t| is(t))
                || (is("font") && ["color", "face", "size"].iter().any(|a| attr(attrs, a).is_some()));
            if !breakout {
                if !self_closing {
                    self.foreign_depth += 1;
                }
                return None;
            }
            self.foreign_depth = 0;
        }
        if let Some(raw) = RAW_TEXT.iter().find(|r| is(r)) {
            if is("script") && self.template_depth == 0 {
                self.script(attrs, emit);
            }
            return Some(raw);
        }
        if is("plaintext") {
            self.stopped = true;
        } else if is("template") {
            self.template_depth += 1;
        } else if is("svg") || is("math") {
            if !self_closing {
                self.foreign_depth = 1;
            }
        } else if is("picture") {
            self.picture_depth += 1;
        } else if self.template_depth > 0 {
        } else if is("base") {
            if !self.base_seen {
                if let Some(href) = attr(attrs, "href") {
                    self.base_seen = true;
                    emit(Prescanned::Base { href: href.to_owned() });
                }
            }
        } else if is("meta") {
            if attr(attrs, "name").is_some_and(|n| n.trim().eq_ignore_ascii_case("referrer")) {
                self.stopped = true;
                emit(Prescanned::ReferrerPolicy);
            }
        } else if is("img") {
            if self.picture_depth == 0 && attr(attrs, "srcset").is_none() {
                if let Some(src) = attr(attrs, "src").filter(|v| !v.trim().is_empty()) {
                    emit(Prescanned::Image { src: src.to_owned() });
                }
            }
        } else if is("link") {
            self.link(attrs, emit);
        }
        None
    }

    fn end_tag(&mut self, name: &str) {
        if self.foreign_depth > 0 {
            self.foreign_depth -= 1;
            return;
        }
        if name.eq_ignore_ascii_case("template") {
            self.template_depth = self.template_depth.saturating_sub(1);
        } else if name.eq_ignore_ascii_case("picture") {
            self.picture_depth = self.picture_depth.saturating_sub(1);
        }
    }

    /// `<script src>`, classified as `Dom::script_info` classifies the
    /// element: a JavaScript MIME type, no type, or a JavaScript `language`
    /// is classic (not fetched with `nomodule`); `module` is a module;
    /// anything else is a data block.
    fn script(&self, attrs: &Attrs<'_>, emit: &mut impl FnMut(Prescanned)) {
        let Some(src) = attr(attrs, "src").filter(|v| !v.is_empty()) else { return };
        let module = match attr(attrs, "type") {
            Some(t) if !t.is_empty() => {
                let t = t.trim().to_ascii_lowercase();
                if t == "module" {
                    true
                } else if crate::script::is_javascript_mime(&t) {
                    false
                } else {
                    return;
                }
            }
            Some(_) => false,
            None => match attr(attrs, "language") {
                Some(l) if !l.is_empty() => {
                    if !crate::script::is_javascript_mime(&format!("text/{}", l.to_ascii_lowercase())) {
                        return;
                    }
                    false
                }
                _ => false,
            },
        };
        if !module && attr(attrs, "nomodule").is_some() {
            return;
        }
        let crossorigin = attr(attrs, "crossorigin").map(str::to_owned);
        emit(Prescanned::Script { src: src.to_owned(), module, crossorigin });
    }

    fn link(&self, attrs: &Attrs<'_>, emit: &mut impl FnMut(Prescanned)) {
        let Some(rel) = attr(attrs, "rel") else { return };
        let has = |token: &str| rel.split_ascii_whitespace().any(|t| t.eq_ignore_ascii_case(token));
        let Some(href) = attr(attrs, "href").filter(|v| !v.trim().is_empty()) else { return };
        let crossorigin = attr(attrs, "crossorigin").map(str::to_owned);
        if has("stylesheet") {
            let enabled = attr(attrs, "disabled").is_none()
                && attr(attrs, "type").is_none_or(|v| v.is_empty() || v.eq_ignore_ascii_case("text/css"));
            if enabled {
                emit(Prescanned::Stylesheet { href: href.to_owned(), crossorigin });
            }
        } else if has("preload") && !has("modulepreload") {
            emit(Prescanned::Preload { href: href.to_owned(), as_: attr(attrs, "as").map(str::to_owned), crossorigin });
        }
    }
}

/// The end of a comment body starting at `from`: after `-->` or `--!>`.
fn comment_end(b: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    while let Some(off) = b[i..].windows(2).position(|w| w == b"--") {
        let at = i + off + 2;
        match b.get(at) {
            Some(b'>') => return Some(at + 1),
            Some(b'!') if b.get(at + 1) == Some(&b'>') => return Some(at + 2),
            _ => i = at - 1,
        }
    }
    None
}

/// The position after `</name` (any case) followed by whitespace, `/` or
/// `>`, and then its `>`; `None` when the rest of the input is the element's
/// text.
fn raw_text_end(b: &[u8], from: usize, name: &str) -> Option<usize> {
    let mut i = from;
    loop {
        let off = b[i..].windows(2).position(|w| w == b"</")?;
        let at = i + off + 2;
        let matched = b.get(at..at + name.len()).is_some_and(|t| t.eq_ignore_ascii_case(name.as_bytes()))
            && b.get(at + name.len()).is_none_or(|&c| is_space(c) || c == b'/' || c == b'>');
        if matched {
            return b[at..].iter().position(|&c| c == b'>').map(|e| at + e + 1);
        }
        i = at;
    }
}

/// Parses a tag from its name (at `from`): the name as written, the
/// attributes (first of a name wins, values decoded), the self-closing
/// flag and the position after `>`. `None` when the input ends inside the
/// tag.
fn parse_tag(input: &str, from: usize) -> Option<(&str, Attrs<'_>, bool, usize)> {
    let b = input.as_bytes();
    let mut i = from;
    while i < b.len() && !is_space(b[i]) && b[i] != b'/' && b[i] != b'>' {
        i += 1;
    }
    let name = ascii(&b[from..i]);
    let mut attrs: Attrs<'_> = Vec::new();
    let mut self_closing = false;
    loop {
        while i < b.len() && (is_space(b[i]) || b[i] == b'/') {
            self_closing = b[i] == b'/';
            i += 1;
        }
        match b.get(i) {
            None => return None,
            Some(b'>') => return Some((name, attrs, self_closing, i + 1)),
            Some(_) => {}
        }
        self_closing = false;
        let start = i;
        // An attribute name may start with `=` (a parse error, kept).
        if b[i] == b'=' {
            i += 1;
        }
        while i < b.len() && !is_space(b[i]) && b[i] != b'/' && b[i] != b'>' && b[i] != b'=' {
            i += 1;
        }
        let attr_name = ascii(&b[start..i]);
        while i < b.len() && is_space(b[i]) {
            i += 1;
        }
        let mut value = Cow::Borrowed("");
        if b.get(i) == Some(&b'=') {
            i += 1;
            while i < b.len() && is_space(b[i]) {
                i += 1;
            }
            match b.get(i) {
                None => return None,
                Some(&q) if q == b'"' || q == b'\'' => {
                    let end = i + 1 + b[i + 1..].iter().position(|&c| c == q)?;
                    value = decode_attr(ascii(&b[i + 1..end]));
                    i = end + 1;
                }
                Some(_) => {
                    let start = i;
                    while i < b.len() && !is_space(b[i]) && b[i] != b'>' {
                        i += 1;
                    }
                    value = decode_attr(ascii(&b[start..i]));
                }
            }
        }
        if !attrs.iter().any(|(n, _)| n.eq_ignore_ascii_case(attr_name)) {
            attrs.push((attr_name, value));
        }
    }
}

/// An attribute value with its character references decoded (HTML
/// "character reference state" in an attribute: a named reference without
/// its semicolon followed by `=` or an alphanumeric is left alone).
fn decode_attr(text: &str) -> Cow<'_, str> {
    if !text.contains('&') {
        return Cow::Borrowed(text);
    }
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'&' {
            let start = i;
            while i < b.len() && b[i] != b'&' {
                i += 1;
            }
            out.push_str(&text[start..i]);
            continue;
        }
        if b.get(i + 1) == Some(&b'#') {
            let (hex, mut j) = match b.get(i + 2) {
                Some(b'x' | b'X') => (true, i + 3),
                _ => (false, i + 2),
            };
            let digits = j;
            let mut n: u32 = 0;
            let mut overflow = false;
            while j < b.len() && (if hex { b[j].is_ascii_hexdigit() } else { b[j].is_ascii_digit() }) {
                let d = (b[j] as char).to_digit(if hex { 16 } else { 10 }).unwrap_or(0);
                n = n.saturating_mul(if hex { 16 } else { 10 }).saturating_add(d);
                overflow |= n > 0x10FFFF;
                j += 1;
            }
            if j == digits {
                out.push('&');
                i += 1;
                continue;
            }
            if b.get(j) == Some(&b';') {
                j += 1;
            }
            let c = match n {
                _ if overflow => '\u{fffd}',
                0 | 0xD800..=0xDFFF => '\u{fffd}',
                0x80..=0x9F => markup5ever::data::C1_REPLACEMENTS[(n - 0x80) as usize]
                    .unwrap_or_else(|| char::from_u32(n).unwrap_or('\u{fffd}')),
                n => char::from_u32(n).unwrap_or('\u{fffd}'),
            };
            out.push(c);
            i = j;
            continue;
        }
        // Named: the longest table entry that prefixes the name.
        let mut j = i + 1;
        while j < b.len() && b[j].is_ascii_alphanumeric() && j - i <= 32 {
            j += 1;
        }
        let with_semicolon = b.get(j) == Some(&b';');
        let mut matched = None;
        let longest = if with_semicolon { j + 1 } else { j };
        let mut end = longest;
        while end > i + 1 {
            if let Some(&(c1, c2)) = NAMED_ENTITIES.get(&text[i + 1..end]) {
                if c1 != 0 {
                    matched = Some((c1, c2, end));
                    break;
                }
            }
            end -= 1;
        }
        match matched {
            Some((c1, c2, end)) => {
                let next = b.get(end).copied();
                let semicolon = b[end - 1] == b';';
                if !semicolon && next.is_some_and(|c| c == b'=' || c.is_ascii_alphanumeric()) {
                    out.push_str(&text[i..end]);
                } else {
                    out.push(char::from_u32(c1).unwrap_or('\u{fffd}'));
                    if c2 != 0 {
                        out.push(char::from_u32(c2).unwrap_or('\u{fffd}'));
                    }
                }
                i = end;
            }
            None => {
                out.push('&');
                i += 1;
            }
        }
    }
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(html: &str) -> Vec<Prescanned> {
        let mut out = Vec::new();
        Prescan::default().scan(html, |item| out.push(item));
        out
    }

    fn script(src: &str) -> Prescanned {
        Prescanned::Script { src: src.into(), module: false, crossorigin: None }
    }

    #[test]
    fn finds_scripts_links_and_images() {
        let items = scan(
            r#"<link rel="stylesheet" href="/a.css"><LINK REL=StyleSheet HREF=b.css crossorigin>
            <link rel="preload" href="/p.js" as="script"><link rel="modulepreload" href="/m.js">
            <link rel="stylesheet" href="/off.css" disabled><link rel=stylesheet type="text/less" href="/x.less">
            <script src="/s1.js"></script><script type="module" src="/mod.js" crossorigin="anonymous"></script>
            <script type="text/template" src="/no.js"></script><script nomodule src="/nm.js"></script>
            <script language="vbscript" src="/vb.js"></script><script language="JavaScript" src="/lang.js"></script>
            <img src="/i.png"><img src="/set.png" srcset="/set2x.png 2x"><img src=" ">"#,
        );
        assert_eq!(
            items,
            [
                Prescanned::Stylesheet { href: "/a.css".into(), crossorigin: None },
                Prescanned::Stylesheet { href: "b.css".into(), crossorigin: Some(String::new()) },
                Prescanned::Preload { href: "/p.js".into(), as_: Some("script".into()), crossorigin: None },
                script("/s1.js"),
                Prescanned::Script { src: "/mod.js".into(), module: true, crossorigin: Some("anonymous".into()) },
                script("/lang.js"),
                Prescanned::Image { src: "/i.png".into() },
            ]
        );
    }

    #[test]
    fn skips_comments_raw_text_templates_noscript_and_foreign_content() {
        let items = scan(
            r#"<!-- <script src="/c.js"></script> --><!--><script src="/after.js"></script>
            <script>var s = '<script src="/inline.js"><\/script>';</script>
            <style>/* <script src="/style.js"></script> */</style>
            <textarea><script src="/ta.js"></script></textarea><title><img src="/t.png"></title>
            <noscript><img src="/ns.png"><link rel=stylesheet href="/ns.css"></noscript>
            <template><script src="/tpl.js"></script><template><img src="/tpl2.png"></template><img src="/tpl3.png"></template>
            <svg><script href="/svg.js"></script><image href="/svg.png"/><foreignObject><p>x</p></foreignObject></svg>
            <picture><source srcset="/pic.png"><img src="/fallback.png"></picture>
            <iframe src="/frame.html"><script src="/if.js"></script></iframe>
            <SCRIPT SRC="/last.js"></SCRIPT><plaintext><script src="/plain.js"></script>"#,
        );
        assert_eq!(items, [script("/after.js"), script("/last.js")]);
    }

    #[test]
    fn breakout_tags_end_foreign_content() {
        let items = scan(r#"<svg><circle/><img src="/in-svg.png"><script src="/html.js"></script>"#);
        assert_eq!(items, [Prescanned::Image { src: "/in-svg.png".into() }, script("/html.js")]);
    }

    #[test]
    fn base_meta_referrer_and_attribute_decoding() {
        let items = scan(
            r#"<base target=_blank><base href="/one/"><base href="/two/">
            <script src="/q?a=1&amp;b=2&lt;3&#x41;&#66;&copy=x&notin;"></script>
            <script src = '/sq.js' async defer></script><script src=/unq.js?x></script>
            <meta name="referrer" content="no-referrer"><script src="/late.js"></script>"#,
        );
        assert_eq!(
            items,
            [
                Prescanned::Base { href: "/one/".into() },
                script("/q?a=1&b=2<3AB&copy=x∉"),
                script("/sq.js"),
                script("/unq.js?x"),
                Prescanned::ReferrerPolicy,
            ]
        );
    }

    #[test]
    fn cut_off_tags_are_dropped() {
        assert_eq!(scan(r#"<script src="/a.js"></script><script src="/b"#), [script("/a.js")]);
        assert_eq!(scan(r#"<script src="/a.js"></script><!-- open"#), [script("/a.js")]);
        assert_eq!(scan(r#"<script src="/a.js"></script><script>no end"#), [script("/a.js")]);
    }

    #[test]
    fn state_carries_across_chunks() {
        let mut p = Prescan::default();
        let mut out = Vec::new();
        p.scan("<template><script src='/a.js'></script>", |i| out.push(i));
        p.scan("</template><script src='/b.js'></script>", |i| out.push(i));
        assert_eq!(out, [script("/b.js")]);
    }
}
