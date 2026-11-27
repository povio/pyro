//! Speculative preload scan (HTML "speculative HTML parsing", Chromium's
//! preload scanner): a tokenizer light enough to run over the buffered rest
//! of a document while the parser waits for a blocking script, finding the
//! start tags whose resources the document will request (`<script src>`,
//! `<link rel=stylesheet>`, `<link rel=preload>`, `<img src|srcset>` and the
//! `<picture><source srcset>` before it, `<video poster>`, SVG `<script href>`
//! and `<image href>`) and the tags that change how later ones are requested
//! (`<base href>`, `<meta name=referrer>`). It inserts nothing: the browser
//! starts the fetches, and the elements take the responses when the parser
//! reaches them.
//!
//! What it skips, as the real tokenizer never sees a tag there: comments and
//! bogus comments, the contents of raw text elements (`script`, `style`,
//! `textarea`, `title`, `xmp`, `iframe`, `noembed`, `noframes`, and
//! `noscript`, since the scan only runs with scripting on), `<template>`
//! contents (inert; a declarative shadow root, `<template shadowrootmode>`
//! outside an inert template, is not inert, but a `<base>` or `<meta>` in it
//! affects nothing outside), foreign content (`<svg>`, `<math>`, with CDATA
//! sections) except inside its integration points (`foreignObject`,
//! `mtext`, ...) and until a tag that breaks out of it (only SVG `<script>`
//! and `<image>` are requested there), script text in its escaped states
//! (`<!--<script>...</script>-->`), and everything after `<plaintext>`. An
//! image whose source depends on the viewport (a `w` descriptor, a `<source
//! media>`) is left to the element. A tag the chunk cuts off is dropped.

use std::borrow::Cow;

use markup5ever::data::NAMED_ENTITIES;

/// A resource (or a request-changing tag) the scan found, attribute values
/// as the tokenizer would produce them (character references decoded).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prescanned {
    /// `<script src>` the document will fetch: a classic script, or a
    /// module (`type=module`).
    /// (An SVG `<script>` with an `href` or `xlink:href` is a classic one.)
    /// `referrer_policy` is the element's `referrerpolicy` attribute as written.
    Script { src: String, module: bool, crossorigin: Option<String>, referrer_policy: Option<String> },
    /// `<link rel=stylesheet href>` (enabled, CSS type).
    Stylesheet { href: String, crossorigin: Option<String>, referrer_policy: Option<String> },
    /// `<link rel=preload href as>` (not a modulepreload).
    Preload { href: String, as_: Option<String>, crossorigin: Option<String>, referrer_policy: Option<String> },
    /// An eager image: the source an `<img>` (its `src`, `srcset` or the
    /// `<picture><source>` before it, at density 1) or a `<video poster>` requests.
    Image { src: String, referrer_policy: Option<String> },
    /// An SVG `<image href|xlink:href>`: requested as the element's own
    /// load-event fetch is (not as an `<img>`).
    SvgImage { href: String, crossorigin: Option<String> },
    /// The first `<base href>`: later URLs resolve against it.
    Base { href: String },
    /// `<meta name=referrer content>`: the `content` as written; later
    /// requests without a `referrerpolicy` carry the policy it names.
    ReferrerPolicy(String),
}

/// Scanner state carried across the chunks of one document's input (the
/// parser's remaining input may grow at the front with written markup).
#[derive(Default)]
pub struct Prescan {
    /// The open `<template>`s, innermost last: `true` for an inert one (its
    /// contents are no part of the document), `false` for a declarative shadow
    /// root (`shadowrootmode`) outside an inert template.
    templates: Vec<bool>,
    /// The open `<picture>`s, innermost last, with the source each selected.
    pictures: Vec<PictureSel>,
    /// The open elements from the outermost `<svg>`/`<math>` in: empty in
    /// HTML content. HTML elements inside an integration point are kept
    /// too, so their end tags do not close foreign ones.
    foreign: Vec<Open>,
    base_seen: bool,
    stopped: bool,
}

/// What a `<picture>`'s `<source>` children before the current position select.
#[derive(Default)]
struct PictureSel {
    /// The URL the first applicable `<source>` selected: its `<img>` requests it.
    selected: Option<String>,
    /// A `<source>` whose applicability (`media`) or candidate (a `w`
    /// descriptor) depends on the viewport: left to the element.
    unknown: bool,
}

impl PictureSel {
    /// A `<source>`, as `Dom::picture_candidate_auto` considers it (at device pixel ratio 1).
    fn consider(&mut self, attrs: &Attrs<'_>) {
        if self.selected.is_some() || self.unknown {
            return;
        }
        if let Some(t) = attr(attrs, "type") {
            let t = t.trim().to_ascii_lowercase();
            if !t.is_empty() && !t.starts_with("image/") {
                return;
            }
        }
        if attr(attrs, "media").is_some_and(|m| !m.trim().is_empty() && !m.trim().eq_ignore_ascii_case("all")) {
            self.unknown = true;
            return;
        }
        let Some(srcset) = attr(attrs, "srcset") else { return };
        if has_width_descriptor(srcset) {
            self.unknown = true;
            return;
        }
        let candidates = crate::picture::parse_srcset(srcset, 0.0);
        if let Some(c) = crate::picture::pick_candidate(&candidates, 1.0) {
            self.selected = Some(c.url.clone());
        }
    }
}

/// Whether a candidate of `srcset` has a width descriptor (its density then needs the `sizes` source size).
fn has_width_descriptor(srcset: &str) -> bool {
    srcset.split(',').any(|candidate| {
        candidate
            .split_ascii_whitespace()
            .skip(1)
            .any(|d| d.strip_suffix('w').is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())))
    })
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Ns {
    Html,
    Svg,
    MathMl,
}

/// An element open in (or inside) foreign content.
struct Open {
    /// Lowercase.
    name: Box<str>,
    ns: Ns,
    /// A MathML text integration point (`mi`, `mo`, `mn`, `ms`, `mtext`):
    /// start tags but `mglyph`/`malignmark` are HTML inside it.
    text_point: bool,
    /// An HTML integration point (SVG `foreignObject`, `desc`, `title`;
    /// MathML `annotation-xml` with an HTML `encoding`): start tags are
    /// HTML inside it.
    html_point: bool,
}

/// HTML elements without end tags (nothing to pop).
const VOID: &[&str] = &[
    "area", "base", "basefont", "bgsound", "br", "col", "embed", "frame", "hr", "image", "img", "input", "keygen",
    "link", "meta", "param", "source", "track", "wbr",
];

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
    /// Whether a `<plaintext>` ended the scan.
    pub fn stopped(&self) -> bool {
        self.stopped
    }

    /// Whether the current node is inside an inert `<template>`.
    fn inert(&self) -> bool {
        self.templates.last().copied().unwrap_or(false)
    }

    /// Whether the current node is an SVG or MathML element.
    fn in_foreign_element(&self) -> bool {
        self.foreign.last().is_some_and(|o| o.ns != Ns::Html)
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
                    } else if self.in_foreign_element() && b[i + 1..].starts_with(b"![CDATA[") {
                        // A CDATA section (the current node is not HTML).
                        match b[i + 9..].windows(3).position(|w| w == b"]]>") {
                            Some(e) => pos = i + 9 + e + 3,
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
        // HTML "tree construction dispatcher" and "in foreign content".
        loop {
            let Some(top) = self.foreign.last() else { break };
            let as_html = top.ns == Ns::Html
                || top.html_point
                || (top.text_point && !is("mglyph") && !is("malignmark"))
                || (top.ns == Ns::MathMl && &*top.name == "annotation-xml" && is("svg"));
            if as_html {
                if is("svg") || is("math") {
                    break;
                }
                let raw = self.html_start_tag(name, attrs, self_closing, emit);
                if raw.is_none() && !VOID.iter().any(|t| is(t)) {
                    self.foreign.push(Open {
                        name: name.to_ascii_lowercase().into(),
                        ns: Ns::Html,
                        text_point: false,
                        html_point: false,
                    });
                }
                return raw;
            }
            let breakout = BREAKOUT.iter().any(|t| is(t))
                || (is("font") && ["color", "face", "size"].iter().any(|a| attr(attrs, a).is_some()));
            if breakout {
                // Pop to an integration point or HTML element; reprocess.
                self.pop_to_html_context();
                continue;
            }
            // A foreign element, in the current node's namespace.
            let ns = top.ns;
            // SVG `<script>` and `<image>` request their `href` (else `xlink:href`).
            if ns == Ns::Svg && !self.inert() && (is("script") || is("image")) {
                if let Some(href) = attr(attrs, "href").or_else(|| attr(attrs, "xlink:href")).filter(|v| !v.trim().is_empty()) {
                    let crossorigin = attr(attrs, "crossorigin").map(str::to_owned);
                    if is("script") {
                        let referrer_policy = attr(attrs, "referrerpolicy").map(str::to_owned);
                        emit(Prescanned::Script { src: href.to_owned(), module: false, crossorigin, referrer_policy });
                    } else {
                        emit(Prescanned::SvgImage { href: href.to_owned(), crossorigin });
                    }
                }
            }
            if !self_closing {
                let lower = name.to_ascii_lowercase();
                let text_point = ns == Ns::MathMl && matches!(&*lower, "mi" | "mo" | "mn" | "ms" | "mtext");
                let html_point = match ns {
                    Ns::Svg => matches!(&*lower, "foreignobject" | "desc" | "title"),
                    _ => {
                        lower == "annotation-xml"
                            && attr(attrs, "encoding").is_some_and(|e| {
                                e.eq_ignore_ascii_case("text/html") || e.eq_ignore_ascii_case("application/xhtml+xml")
                            })
                    }
                };
                self.foreign.push(Open { name: lower.into(), ns, text_point, html_point });
            }
            return None;
        }
        self.html_start_tag(name, attrs, self_closing, emit)
    }

    /// Pops foreign elements up to an HTML integration point, a MathML text
    /// integration point or an HTML element (or out of foreign content).
    fn pop_to_html_context(&mut self) {
        while let Some(top) = self.foreign.last() {
            if top.ns == Ns::Html || top.html_point || top.text_point {
                break;
            }
            self.foreign.pop();
        }
    }

    /// A start tag in HTML content.
    fn html_start_tag(
        &mut self,
        name: &str,
        attrs: &Attrs<'_>,
        self_closing: bool,
        emit: &mut impl FnMut(Prescanned),
    ) -> Option<&'static str> {
        let is = |tag: &str| name.eq_ignore_ascii_case(tag);
        if let Some(raw) = RAW_TEXT.iter().find(|r| is(r)) {
            if is("script") && !self.inert() {
                self.script(attrs, emit);
            }
            return Some(raw);
        }
        if is("plaintext") {
            self.stopped = true;
        } else if is("template") {
            // A declarative shadow root (HTML "attach a shadow root" with `shadowrootmode`
            // open or closed) is part of the document, and its scripts, styles and images
            // are requested; any other template, and one inside an inert one, is inert.
            let shadow = !self.inert()
                && attr(attrs, "shadowrootmode").is_some_and(|m| m.eq_ignore_ascii_case("open") || m.eq_ignore_ascii_case("closed"));
            self.templates.push(!shadow);
        } else if is("svg") || is("math") {
            if !self_closing {
                let ns = if is("svg") { Ns::Svg } else { Ns::MathMl };
                self.foreign.push(Open {
                    name: name.to_ascii_lowercase().into(),
                    ns,
                    text_point: false,
                    html_point: false,
                });
            }
        } else if is("picture") {
            self.pictures.push(PictureSel::default());
        } else if self.inert() {
        } else if is("source") {
            if let Some(picture) = self.pictures.last_mut() {
                picture.consider(attrs);
            }
        } else if is("base") {
            // Not in a declarative shadow root: a <base> there affects no URL outside it.
            if !self.base_seen && self.templates.is_empty() {
                if let Some(href) = attr(attrs, "href") {
                    self.base_seen = true;
                    emit(Prescanned::Base { href: href.to_owned() });
                }
            }
        } else if is("meta") {
            // Not in a declarative shadow root: its policy is no part of the document's.
            if self.templates.is_empty() && attr(attrs, "name").is_some_and(|n| n.trim().eq_ignore_ascii_case("referrer")) {
                if let Some(content) = attr(attrs, "content") {
                    emit(Prescanned::ReferrerPolicy(content.to_owned()));
                }
            }
        } else if is("img") || is("image") {
            // The HTML tree builder reprocesses <image> as <img>.
            self.image(attrs, emit);
        } else if is("video") {
            // A poster image is requested at once, as an image (it has no `loading`).
            if let Some(poster) = attr(attrs, "poster").filter(|v| !v.trim().is_empty()) {
                emit(Prescanned::Image { src: poster.to_owned(), referrer_policy: None });
            }
        } else if is("link") {
            self.link(attrs, emit);
        }
        None
    }

    fn end_tag(&mut self, name: &str) {
        if !self.foreign.is_empty() {
            // The innermost open element of that name closes, with what is
            // inside it (an approximation of both insertion modes' rules).
            let lower = name.to_ascii_lowercase();
            let Some(at) = self.foreign.iter().rposition(|o| *o.name == *lower) else { return };
            let html = self.foreign[at].ns == Ns::Html;
            self.foreign.truncate(at);
            if !html {
                return;
            }
        }
        if name.eq_ignore_ascii_case("template") {
            self.templates.pop();
        } else if name.eq_ignore_ascii_case("picture") {
            self.pictures.pop();
        }
    }

    /// An `<img>`: the source its `<picture>` selected, else its `srcset` (the
    /// density-1 candidate, as `Dom::own_srcset_source` picks it), else its `src`.
    /// A lazy image waits for the element; so does one whose source needs the viewport.
    fn image(&self, attrs: &Attrs<'_>, emit: &mut impl FnMut(Prescanned)) {
        if attr(attrs, "loading").is_some_and(|value| value.eq_ignore_ascii_case("lazy")) {
            return;
        }
        let referrer_policy = attr(attrs, "referrerpolicy").map(str::to_owned);
        if let Some(picture) = self.pictures.last() {
            if picture.unknown {
                return;
            }
            if let Some(src) = &picture.selected {
                emit(Prescanned::Image { src: src.clone(), referrer_policy });
                return;
            }
        }
        let src = match attr(attrs, "srcset") {
            None => attr(attrs, "src").filter(|v| !v.trim().is_empty()).map(str::to_owned),
            Some(srcset) => {
                if has_width_descriptor(srcset) {
                    return;
                }
                let mut candidates = crate::picture::parse_srcset(srcset, 0.0);
                if let Some(src) = attr(attrs, "src").filter(|v| !v.is_empty()) {
                    if !candidates.iter().any(|c| c.density == 1.0) {
                        candidates.push(crate::picture::Candidate { url: src.to_owned(), density: 1.0 });
                    }
                }
                crate::picture::pick_candidate(&candidates, 1.0).map(|c| c.url.clone())
            }
        };
        if let Some(src) = src {
            emit(Prescanned::Image { src, referrer_policy });
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
                let raw = t.to_ascii_lowercase();
                if raw == "module" {
                    true
                } else if crate::script::is_javascript_mime(raw.trim_matches(|c: char| c.is_ascii_whitespace())) {
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
        let referrer_policy = attr(attrs, "referrerpolicy").map(str::to_owned);
        emit(Prescanned::Script { src: src.to_owned(), module, crossorigin, referrer_policy });
    }

    fn link(&self, attrs: &Attrs<'_>, emit: &mut impl FnMut(Prescanned)) {
        let Some(rel) = attr(attrs, "rel") else { return };
        let has = |token: &str| rel.split_ascii_whitespace().any(|t| t.eq_ignore_ascii_case(token));
        let Some(href) = attr(attrs, "href").filter(|v| !v.trim().is_empty()) else { return };
        let crossorigin = attr(attrs, "crossorigin").map(str::to_owned);
        let referrer_policy = attr(attrs, "referrerpolicy").map(str::to_owned);
        if has("stylesheet") {
            let enabled = attr(attrs, "disabled").is_none()
                && attr(attrs, "type").is_none_or(|v| v.is_empty() || v.eq_ignore_ascii_case("text/css"));
            if enabled {
                emit(Prescanned::Stylesheet { href: href.to_owned(), crossorigin, referrer_policy });
            }
        } else if has("preload") && !has("modulepreload") && attr(attrs, "imagesrcset").is_none() {
            // An image preload with a source set fetches a candidate of it, not `href`: the
            // element's own fetch decides.
            let as_ = attr(attrs, "as").map(str::to_owned);
            emit(Prescanned::Preload { href: href.to_owned(), as_, crossorigin, referrer_policy });
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

/// Whether `b[at..]` is `prefix` + `name` (any case) followed by
/// whitespace, `/` or `>` (or the end of input when `open_end`).
fn tag_at(b: &[u8], at: usize, prefix: &[u8], name: &str, open_end: bool) -> bool {
    let n = at + prefix.len();
    b[at..].starts_with(prefix)
        && b.get(n..n + name.len()).is_some_and(|t| t.eq_ignore_ascii_case(name.as_bytes()))
        && b.get(n + name.len()).map_or(open_end, |&c| is_space(c) || c == b'/' || c == b'>')
}

/// The position after `</name` (any case) followed by whitespace, `/` or
/// `>`, and then its `>`; `None` when the rest of the input is the element's
/// text.
fn raw_text_end(b: &[u8], from: usize, name: &str) -> Option<usize> {
    if name == "script" {
        return script_data_end(b, from);
    }
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

/// `raw_text_end` for script data, through its escaped (`<!--`) and
/// double-escaped (`<!--<script>`) states: in the latter, `</script>`
/// returns to escaped text instead of ending the element, and `-->` returns
/// to plain script data from either.
fn script_data_end(b: &[u8], from: usize) -> Option<usize> {
    #[derive(PartialEq)]
    enum State {
        Data,
        Escaped,
        DoubleEscaped,
    }
    let mut state = State::Data;
    let mut i = from;
    loop {
        // Only `<` (and in escaped text `-`) can change the state.
        let escaped = state != State::Data;
        i += b[i..].iter().position(|&c| c == b'<' || (escaped && c == b'-'))?;
        if escaped && b[i..].starts_with(b"-->") {
            state = State::Data;
            i += 3;
            continue;
        }
        if b[i] != b'<' {
            i += 1;
            continue;
        }
        match state {
            State::Data if b[i..].starts_with(b"<!--") => {
                // At the `--`: `<!-->` and `<!--->` return at once.
                state = State::Escaped;
                i += 2;
            }
            State::Data | State::Escaped if tag_at(b, i, b"</", "script", true) => {
                let at = i + 2;
                return b[at..].iter().position(|&c| c == b'>').map(|e| at + e + 1);
            }
            State::Escaped if tag_at(b, i, b"<", "script", false) => {
                state = State::DoubleEscaped;
                i += 7;
            }
            State::DoubleEscaped if tag_at(b, i, b"</", "script", false) => {
                state = State::Escaped;
                i += 8;
            }
            _ => i += 1,
        }
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
    decode_references(text, true)
}

/// Decode references in WebVTT text or tag annotations, using HTML data-state rules.
pub fn decode_webvtt_references(text: &str) -> Cow<'_, str> {
    decode_references(text, false)
}

fn decode_references(text: &str, in_attribute: bool) -> Cow<'_, str> {
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
                if in_attribute && !semicolon && next.is_some_and(|c| c == b'=' || c.is_ascii_alphanumeric()) {
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
        Prescanned::Script { src: src.into(), module: false, crossorigin: None, referrer_policy: None }
    }

    fn img(src: &str) -> Prescanned {
        Prescanned::Image { src: src.into(), referrer_policy: None }
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
            <img src="/lazy.png" loading="LaZy"><img loading="eager" src="/eager.png">
            <img src="/i.png"><img src="/set.png" srcset="/set2x.png 2x"><img src=" ">"#,
        );
        assert_eq!(
            items,
            [
                Prescanned::Stylesheet { href: "/a.css".into(), crossorigin: None, referrer_policy: None },
                Prescanned::Stylesheet { href: "b.css".into(), crossorigin: Some(String::new()), referrer_policy: None },
                Prescanned::Preload { href: "/p.js".into(), as_: Some("script".into()), crossorigin: None, referrer_policy: None },
                script("/s1.js"),
                Prescanned::Script {
                    src: "/mod.js".into(),
                    module: true,
                    crossorigin: Some("anonymous".into()),
                    referrer_policy: None
                },
                script("/lang.js"),
                img("/eager.png"),
                img("/i.png"),
                // `src` is the 1x candidate of a set without one.
                img("/set.png"),
            ]
        );
    }

    #[test]
    fn treats_image_as_img_only_in_html_content() {
        let items = scan(
            r#"<IMAGE src="/alias.png"><image loading="lazy" src="/lazy.png">
            <svg><image src="/not-href.png"/><image href="/foreign.png"/></svg>
            <picture><image src="/fallback.png"></picture>"#,
        );
        // An `<image>` in `<picture>` is an `<img>` there; an SVG one is requested by its `href`.
        assert_eq!(
            items,
            [
                img("/alias.png"),
                Prescanned::SvgImage { href: "/foreign.png".into(), crossorigin: None },
                img("/fallback.png"),
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
            <svg><script src="/svg-src.js"></script><image src="/svg-src.png"/><foreignObject><p>x</p></foreignObject></svg>
            <math><script href="/math.js"></script></math>
            <picture><source srcset="/pic.png" media="(min-width: 1px)"><img src="/fallback.png"></picture>
            <template><svg><script href="/tpl-svg.js"></script></svg><video poster="/tpl-poster.png"></video></template>
            <iframe src="/frame.html"><script src="/if.js"></script></iframe>
            <SCRIPT SRC="/last.js"></SCRIPT><plaintext><script src="/plain.js"></script>"#,
        );
        assert_eq!(items, [script("/after.js"), script("/last.js")]);
    }

    #[test]
    fn breakout_tags_end_foreign_content() {
        let items = scan(r#"<svg><circle/><img src="/in-svg.png"><script src="/html.js"></script>"#);
        assert_eq!(items, [img("/in-svg.png"), script("/html.js")]);
    }

    #[test]
    fn an_element_with_its_own_referrer_policy_is_speculated_with_it() {
        let found = scan(
            r#"<script src="/a.js" referrerpolicy="origin"></script><img src="/a.png" referrerpolicy=no-referrer>
            <link rel=stylesheet href="/a.css" referrerpolicy="unsafe-url"><script src="/b.js"></script>"#,
        );
        assert_eq!(
            found,
            [
                Prescanned::Script { src: "/a.js".into(), module: false, crossorigin: None, referrer_policy: Some("origin".into()) },
                Prescanned::Image { src: "/a.png".into(), referrer_policy: Some("no-referrer".into()) },
                Prescanned::Stylesheet { href: "/a.css".into(), crossorigin: None, referrer_policy: Some("unsafe-url".into()) },
                script("/b.js"),
            ]
        );
    }

    #[test]
    fn declarative_shadow_roots_are_scanned_and_plain_templates_are_not() {
        let found = scan(
            r#"<div><template shadowrootmode="closed"><script src="/dsd.js"></script><base href="/dsd-base/"><meta name=referrer content=origin></template></div>
            <template shadowrootmode="bogus"><img src="/bogus.png"></template>
            <template><div><template shadowrootmode=open><img src="/nested-1.png"></template></div></template>
            <template shadowrootmode=open><template><img src="/nested-2.png"></template><img src="/dsd.png"></template>
            <script src="/after.js"></script>"#,
        );
        assert_eq!(found, [script("/dsd.js"), img("/dsd.png"), script("/after.js")]);
    }

    #[test]
    fn picture_sources_srcset_posters_and_svg_references() {
        let found = scan(
            r#"<picture><source srcset="/br.png"><br><img></picture>
            <picture><source srcset="/text.png" type="text/plain"><source srcset="/second.png 1x, /second2.png 2x"><img src="/img.png"></picture>
            <picture><source srcset="/media.png" media="not all"><img src="/unknown.png"></picture>
            <picture><source srcset="/no-img.png"></picture>
            <picture><source srcset="/wide.png 100w"><img src="/wide-img.png"></picture>
            <img srcset="/only.png"><img srcset="/w.png 2w"><img srcset="/a.png 2x, /b.png 3x" src="/c.png"><img srcset="/d.png 1x" src="/e.png">
            <video poster="/poster.png"></video><video poster=" "></video>
            <svg><script xlink:href="/x.js"></script><script href="/h.js" xlink:href="/ignored.js"></script><image xlink:href="/x.png" crossorigin/></svg>"#,
        );
        assert_eq!(
            found,
            [
                img("/br.png"),
                img("/second.png"),
                img("/only.png"),
                img("/c.png"),
                img("/d.png"),
                img("/poster.png"),
                script("/x.js"),
                script("/h.js"),
                Prescanned::SvgImage { href: "/x.png".into(), crossorigin: Some(String::new()) },
            ]
        );
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
                Prescanned::ReferrerPolicy("no-referrer".into()),
                script("/late.js"),
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
    fn escaped_script_data_ends_where_the_tokenizer_ends_it() {
        // Double-escaped: the inner `</script>` does not end the element.
        let items = scan(
            r#"<script><!--<script></script><img src="/esc.png">--></script><img src="/after.png">
            <script><!--</script><img src="/escaped-end.png">
            <script><!-- x --><script src="/notnested.js"></script></script>
            <script><!--<script>--></script><img src="/dd.png">
            <script><!--><img src="/no.png"></script><img src="/short.png">"#,
        );
        assert_eq!(items, [img("/after.png"), img("/escaped-end.png"), img("/dd.png"), img("/short.png")]);
    }

    #[test]
    fn integration_points_keep_foreign_content() {
        // HTML inside a text integration point does not end the <math>.
        assert_eq!(
            scan(
                r#"<math><mtext><b>x</b></mtext><annotation-xml><script src="/mathx.js"></script></annotation-xml></math>
                <svg><foreignObject><div>x</div></foreignObject><script src="/svgx.js"></script></svg><img src="/out.png">"#
            ),
            [img("/out.png")]
        );
        // Inside an integration point, start tags are HTML elements.
        assert_eq!(
            scan(
                r#"<svg><foreignObject><img src="/fo.png"><script src="/fo.js"></script></foreignObject><image href="/x.png"/></svg>
                <math><mi><img src="/mi.png"></mi><annotation-xml encoding="text/html"><script src="/ax.js"></script></annotation-xml></math>"#
            ),
            [img("/fo.png"), script("/fo.js"), Prescanned::SvgImage { href: "/x.png".into(), crossorigin: None }, img("/mi.png"), script("/ax.js")]
        );
        // A breakout tag in plain foreign content ends it.
        assert_eq!(
            scan(r#"<svg><g><p><img src="/p.png"></svg><script src="/s.js"></script>"#),
            [img("/p.png"), script("/s.js")]
        );
    }

    #[test]
    fn cdata_in_foreign_content_is_skipped_to_its_end() {
        assert_eq!(
            scan(r#"<svg><![CDATA[ a > b <script src="/cd.js"></script> ]]></svg><script src="/after.js"></script>"#),
            [script("/after.js")]
        );
        // In HTML, a bogus comment up to the first `>`.
        assert_eq!(scan(r#"<![CDATA[ x ><script src="/html.js"></script>"#), [script("/html.js")]);
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
