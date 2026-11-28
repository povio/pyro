//! Document serialization for rendering (screenshots): the current tree as
//! another engine can lay out and paint it without running script, either
//! as HTML ([`Dom::render_html`]) or as a keyed node list
//! ([`Dom::render_tree`]) that a renderer can diff against the previous one.
//!
//! Differences from `outerHTML`:
//! - form state is written into markup: an input's current value and
//!   checkedness, a textarea's value, each option's selectedness;
//! - `<style>` text and `<link rel=stylesheet>` URLs come from the caller
//!   (the style engine holds the live text, CSSOM edits included);
//! - a `<script>`, `<noscript>` (scripting is on) and `<template>` are written without their
//!   content (the elements stay, `display: none`, so they count for selectors and have a computed
//!   style); comments and processing instructions are left out, and so is an `<iframe>`'s `src`
//!   (a frame shows the document [`Dom::set_frame_documents`] gave it, which the
//!   renderer takes from the frame table, [`Dom::frame_table`], by the index the frame's
//!   [`FRAME_ATTRIBUTE`] names; else nothing); a page attribute that would collide with
//!   one the engine writes is left out ([`ENGINE_ATTRIBUTES`]); HTML markup leaves out event handler
//!   (`on*`) attributes, which keyed trees keep (`content: attr(onclick)`);
//! - the tree is the flat tree: a shadow host is serialized with its shadow
//!   tree in place of its children (marked with the [`STATE_NS`]
//!   `shadow-host` attribute), and each `<slot>` of a shadow tree with the
//!   light children assigned to it in place of its own (marked
//!   `assigned-slot`), else its fallback content. Its styles, `<style>`,
//!   `<link>` and adopted sheets, stay in the shadow tree, which the renderer
//!   scopes to it (CSS Scoping);
//! - an element whose directionality comes from its content (`dir=auto`, a
//!   `<bdi>`) carries it as a `dir=ltr` or `dir=rtl` token of the
//!   [`STATE_NS`] `state` attribute: the renderer derives the others' from
//!   their `dir` attribute and their parent in the DOM;
//! - one element can be marked with an attribute (the clip target).


/// pyro: the attributes the snapshot itself writes and the renderer reads (beyond the state
/// namespace and the [`ENGINE_ATTRIBUTE_PREFIX`] family). A page attribute whose serialised name
/// (`prefix:local`, ASCII case-insensitively, in any namespace) is one of these, or one this
/// element got from the engine, or starts with the prefix, is not written: page content can
/// never stand in for an engine attribute, in markup or a keyed tree (security review cycle 3).
pub const ENGINE_ATTRIBUTES: &[&str] = &["srcdoc", FRAME_ATTRIBUTE, SHEET_POLICY_ATTRIBUTE, SHEET_ATTRIBUTE];

/// pyro: the attribute a sheet link carries the key of its text in the snapshot's sheets in
/// ([`sheet_key`]): the renderer serves a sheet by its link, never by its URL.
pub const SHEET_ATTRIBUTE: &str = "data-pyro-sheet";



/// pyro: the attribute a sheet link carries its response's `Referrer-Policy` in
/// ([`Dom::set_sheet_policy`]): the renderer records it for that sheet object.
pub const SHEET_POLICY_ATTRIBUTE: &str = "data-pyro-sheet-referrer-policy";

/// pyro: the attributes a frame element (`iframe`, `frame`, `object`, `embed`) loads a document
/// from: never written from another namespace, where markup would make them its own.
pub const FRAME_SOURCE_ATTRIBUTES: &[&str] = &["src", "srcdoc", "data", "type"];

/// pyro: the prefix of the snapshot's own attribute family (`data-pyro-key`, scroll offsets,
/// frame URL and content height, the clip mark): page attributes with it are never written.
pub const ENGINE_ATTRIBUTE_PREFIX: &str = "data-pyro-";

/// pyro: raw text (a `<style>`'s sheet) as markup can hold it so that parsing it back gives the
/// same tree: an end tag of its element (`</style`, any case, which would end it early and let
/// the rest of the text become elements, frames included) is written `<\/style`, which CSS reads
/// the same inside strings and comments and as nothing valid elsewhere. Other raw-text and RCDATA
/// elements' text is written escaped (entities), which no markup can come out of.
pub fn raw_text_safe<'a>(text: &'a str, tag: &str) -> std::borrow::Cow<'a, str> {
    let lower = text.to_ascii_lowercase();
    let end = format!("</{tag}");
    if !lower.contains(&end) {
        return std::borrow::Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len() + 8);
    let mut at = 0;
    while let Some(found) = lower[at..].find(&end) {
        let i = at + found;
        out.push_str(&text[at..i]);
        out.push_str("<\\/");
        at = i + 2;
    }
    out.push_str(&text[at..]);
    std::borrow::Cow::Owned(out)
}

/// pyro: the attribute a frame element carries its frame document in: `"<nonce>:<index>:<version>"`,
/// the index naming the entry of the snapshot's frame table ([`Dom::frame_table`]: the
/// document's markup, URL, scroll position, client and its own frames), the version changing
/// with the entry's content. The document and its client never travel in markup.
pub const FRAME_ATTRIBUTE: &str = "data-pyro-frame";

/// pyro: whether a page attribute may be written next to `engine` (the names, lowercase, of the
/// attributes the engine gave the element). A page attribute keeps its namespace and name
/// (`xml:lang`, `xlink:href`, `xmlns:*`, any author namespace); one is left out only if the
/// renderer could read it as an engine attribute: its local name or its serialised qualified name
/// (`prefix:local`), ASCII case-insensitively, is in [`ENGINE_ATTRIBUTES`], starts with
/// [`ENGINE_ATTRIBUTE_PREFIX`], or is one the engine gave this element. (The renderer looks
/// attributes up by local name, whatever their namespace: a namespaced or prefixed `srcdoc` is
/// left out too.)
/// pyro (security review cycle 8): the page's own `data-pyro-*` attributes are written like any
/// other (CSS matches them in the renderer); the engine's travel under names carrying the
/// document's nonce ([`engine_name`]), which page content cannot know, so none collides.
fn page_attribute_allowed(name: &QualName, engine: &[String], nonce: &str) -> bool {
    let local = name.local.to_string().to_ascii_lowercase();
    let serialised = match &name.prefix {
        Some(prefix) if !prefix.is_empty() => format!("{prefix}:{}", name.local).to_ascii_lowercase(),
        _ => local.clone(),
    };
    let own = (!nonce.is_empty()).then(|| format!("{ENGINE_ATTRIBUTE_PREFIX}{nonce}-"));
    let reserved = |n: &str| {
        n.eq_ignore_ascii_case("srcdoc")
            || own.as_deref().is_some_and(|own| n.starts_with(own))
            || engine.iter().any(|e| e == n)
    };
    !(reserved(&local) || reserved(&serialised))
}

/// pyro: the name an engine attribute (`canonical`, of the [`ENGINE_ATTRIBUTE_PREFIX`] family:
/// `data-pyro-sheet`, `data-pyro-frame`, ...) is written under in a document serialised with
/// `nonce`: `data-pyro-<nonce>-<field>`. Only the renderer, given the nonce, reads it as the
/// engine's (blitz-dom `engine_field`); page content can neither write nor match it (CSS has no
/// attribute-name wildcards), and the page's own `data-pyro-*` attributes stay its own. Without a
/// nonce (markup written by hand), the canonical name.
/// pyro: the namespace the engine's state attributes ([`STATE_NS`]: pseudo-class state, the top
/// layer, shadow and slot markers) travel in, in a keyed tree serialised with `nonce`:
/// `urn:x-pyro:state:<nonce>`. Only the renderer, given the nonce, takes them as state (blitz-dom
/// `engine_state`, into its engine store); a page attribute in any namespace, [`STATE_NS`] itself
/// included, is the page's own. Without a nonce, [`STATE_NS`].
pub fn state_ns(nonce: &str) -> String {
    if nonce.is_empty() {
        STATE_NS.to_owned()
    } else {
        format!("{STATE_NS}:{nonce}")
    }
}

/// pyro: [`engine_name`] for the attribute name of an engine attribute.
pub fn engine_name(nonce: &str, canonical: &str) -> String {
    match canonical.strip_prefix(ENGINE_ATTRIBUTE_PREFIX) {
        Some(field) if !nonce.is_empty() => format!("{ENGINE_ATTRIBUTE_PREFIX}{nonce}-{field}"),
        _ => canonical.to_owned(),
    }
}

use std::borrow::Cow;

use markup5ever::{ns, QualName};

use crate::dom::{Change, Dom};
use crate::error::DomResult;
use crate::forms::InputType;
use crate::node::{NodeId, NodeKind, NIL};
use crate::serialize::{escape, is_void};

/// Namespace of the `state` attribute render trees carry on form controls
/// (see `Dom::render_states`): page CSS cannot match it (`[state]` matches
/// the null namespace only), the renderer maps it to pseudo-classes.
pub const STATE_NS: &str = "urn:x-pyro:state";

/// Name ([`STATE_NS`]) of the attribute a shadow host carries in render
/// trees: its render children are its shadow tree's.
pub const SHADOW_HOST_ATTR: &str = "shadow-host";
/// Name ([`STATE_NS`]) of the attribute a slot of a shadow tree carries in
/// render trees when nodes are assigned to it: its render children are those
/// (light children of its host), not its own.
pub const ASSIGNED_SLOT_ATTR: &str = "assigned-slot";
/// Name ([`STATE_NS`]) of the attribute a light child element of a shadow host carries in render
/// trees: its zero-based DOM index and total element siblings, separated by '/'. Its render parent is its
/// slot, so this is how the renderer finds its light-tree siblings (sibling combinators match in
/// the node tree, not the flat tree).
pub const LIGHT_INDEX_ATTR: &str = "light-index";
/// Name ([`STATE_NS`]) of the attribute an `<svg>` panned or zoomed by script carries in render
/// trees (`currentTranslate`, `currentScale`): `tx ty scale`.
pub const SVG_PAN_ATTR: &str = "svg-pan";
/// pyro: name ([`STATE_NS`]) of the attribute that marks the slot a `<details>` element's content
/// renders in (the render tree gives a `<details>` the user-agent shadow tree Chromium gives it: a
/// slot for its first `<summary>`, then this one for the rest), which the renderer styles as its
/// `::details-content`.
pub const DETAILS_CONTENT_ATTR: &str = "details-content";
/// pyro: the bits a `<details>` element's render key carries for its summary slot, and with
/// [`DETAILS_CONTENT_SLOT_KEY`] for its content slot (which generations never reach).
pub const DETAILS_SUMMARY_SLOT_KEY: u64 = 1 << 61;
pub const DETAILS_CONTENT_SLOT_KEY: u64 = 1 << 60;
/// pyro: the render key flag of the synthetic assigned slot a shadow host's unassigned light
/// `<style>` children are rendered in (with the host's render key): their sheets apply in the
/// host's tree scope though no slot takes them (CSS Scoping; WPT css-cascade `scope-part`).
pub const UNASSIGNED_STYLE_SLOT_KEY: u64 = 1 << 59;

/// What the caller supplies.
pub struct RenderHtml<'a> {
    /// Sheet text replacing a `<style>` element's own text (CSSOM edits);
    /// `None`: its text content.
    pub style_text: &'a dyn Fn(NodeId) -> Option<String>,
    /// Absolute URL to keep on a `<link rel=stylesheet>`; `None` drops the
    /// link (sheet not loaded).
    pub stylesheet_href: &'a dyn Fn(NodeId) -> Option<String>,
    /// Element to mark with `mark.1` (a valueless attribute).
    pub mark: Option<(NodeId, &'a str)>,
    /// The constructed sheets (id and text) a document or shadow root adopts
    /// (`adoptedStyleSheets`), in order; `None` when nothing adopts any.
    /// They render as `<style>` elements after the document element's
    /// children and after a shadow tree's, each text once per tree (the
    /// renderer shares the parsed sheet between trees).
    pub adopted: Option<&'a dyn Fn(NodeId) -> Vec<(u64, std::rc::Rc<str>)>>,
}

/// Changes to a [`RenderTree`] ([`Dom::render_delta`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RenderDelta {
    pub ops: Vec<DeltaOp>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DeltaOp {
    /// The node's own content (an element's name and attributes, a text's
    /// data) is now `kind`; its children are unchanged.
    Node { key: u64, kind: RenderNodeKind },
    /// The render children of `parent` (`None`: the top level) are now
    /// `kids`; `nodes` are the subtrees of the inserted ones (parents index
    /// into `nodes`, [`TOP`] for a child of `parent`).
    Children { parent: Option<u64>, kids: Vec<u64>, nodes: Vec<RenderNode> },
}

/// The document as [`Dom::render_html`] would serialize it, as nodes in
/// document order. Keys identify nodes across snapshots of the same `Dom`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RenderTree {
    pub nodes: Vec<RenderNode>,
    /// Key of the marked element (`RenderHtml::mark`; its attribute is not
    /// written into the tree).
    pub mark: Option<u64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RenderNode {
    /// The node's [`render_key`]. Text a renderer gets in place of an
    /// element's children (a `<style>`'s sheet text, a textarea's value) has
    /// its element's key with [`SYNTHETIC_TEXT`] set.
    pub key: u64,
    /// Index of the parent in `RenderTree::nodes`, [`TOP`] at the top level.
    pub parent: u32,
    pub kind: RenderNodeKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RenderNodeKind {
    Element { name: QualName, attrs: Vec<(QualName, String)> },
    Text(String),
}

/// `RenderNode::parent` of a top-level node.
pub const TOP: u32 = u32::MAX;
/// Set in the key of text that stands for an element's children.
pub const SYNTHETIC_TEXT: u64 = 1 << 63;

/// A node's identity in render trees: stable while the node lives, never
/// reused for another node of the same `Dom`.
pub fn render_key(id: NodeId) -> u64 {
    ((id.generation() as u64) << 32) | id.index() as u64
}

/// pyro: the render key of `id`'s `::before` (`pseudo` 1) or `::after` (2) box: the element's
/// with the pseudo-element in its two top bits (which generations never reach).
pub fn pseudo_render_key(id: NodeId, pseudo: u8) -> u64 {
    render_key(id) | ((pseudo as u64 & 3) << 62)
}

/// Tree scope of a node being serialized: index into the shadow hosts
/// entered so far, `NONE` for the document.
type Scope = usize;
const NONE: Scope = usize::MAX;

enum Step {
    Enter(u32, Scope),
    /// An adopted sheet, as a `<style>` element of that key.
    Adopted(u64, std::rc::Rc<str>),
    Close,
    /// Text standing for element `.0`'s children.
    Synthetic(u32, String, TextKind),
    /// pyro: a slot of a `<details>` element's user-agent shadow tree (render key, whether it is
    /// the content slot, its assigned nodes and their scope).
    DetailsSlot(u64, bool, Vec<u32>, Scope),
}

#[derive(Clone, Copy, PartialEq)]
enum TextKind {
    /// A text node: escaped in HTML.
    Node,
    /// A `<style>`'s sheet text: raw in HTML.
    Style,
    /// A textarea's value: escaped, after the newline a parser drops.
    Textarea,
}

/// An attribute as written: `valueless` ones are serialized as a bare name.
struct RAttr<'a> {
    name: Cow<'a, QualName>,
    value: Cow<'a, str>,
    valueless: bool,
}

/// What the walk produces.
trait Sink {
    fn doctype(&mut self, name: &str);
    /// `tag`: the serialized tag name. `void`: no children and no `close`.
    fn open(&mut self, key: u64, tag: &str, name: &QualName, attrs: &[RAttr<'_>], marked: bool, void: bool);
    fn close(&mut self, tag: &str);
    fn text(&mut self, key: u64, text: &str, kind: TextKind);
}

struct HtmlSink<'a> {
    out: String,
    mark: Option<&'a str>,
    scroll: &'a [(u64, (f64, f64))],
    sticky: &'a [(u64, (f64, f64))],
    /// pyro: elements written with their render key (`data-pyro-key`): a frame's animated
    /// elements, whose sampled animations the renderer applies to its document by key.
    keyed: &'a [u64],
    /// pyro: the elements open in the markup, as written ([`Written`]).
    open: Vec<Written>,
    /// pyro: the document's snapshot nonce, which every element given engine attributes carries
    /// ([`NONCE_ATTRIBUTE`]).
    nonce: String,
}



/// pyro: how an open element was written, for its text and its end tag.
#[derive(Clone, Copy, PartialEq)]
enum Written {
    Plain,
    /// An HTML `<xmp>` or `<plaintext>`, written as `<pre>` with its name in [`TAG_ATTRIBUTE`]:
    /// a reparse would read its content as text up to `</xmp>` (`<plaintext>`: to the end of the
    /// markup, hiding every later element, frames and engine elements included), not as the DOM
    /// has it. `<pre>` parses as they do otherwise (it closes an open `<p>`); the renderer gives
    /// the element its name back (blitz-html `create_element`).
    AsPre,
    /// An HTML `<noembed>` or `<noframes>`: raw text in a reparse, so its text is written raw
    /// (an end tag of it neutralised, [`raw_text_safe`]), not as entities a reparse keeps as such.
    RawText(&'static str),
}

/// pyro: the attribute an element written as `<pre>` carries its own name in (see [`Written`]).
/// Page attributes with the [`ENGINE_ATTRIBUTE_PREFIX`] are never written, so only the engine
/// writes it.
pub const TAG_ATTRIBUTE: &str = "data-pyro-tag";

/// pyro: [`escape`], and a carriage return as a character reference: a reparse turns a raw CR
/// (and CR LF) into LF, a reference it keeps.
fn escape_render(out: &mut String, s: &str, attr: bool) {
    if !s.contains('\r') {
        return escape(out, s, attr);
    }
    let mut escaped = String::with_capacity(s.len() + 8);
    escape(&mut escaped, s, attr);
    out.push_str(&escaped.replace('\r', "&#13;"));
}

impl Sink for HtmlSink<'_> {
    fn doctype(&mut self, name: &str) {
        self.out.push_str("<!DOCTYPE ");
        self.out.push_str(name);
        self.out.push('>');
    }
    fn open(&mut self, key: u64, tag: &str, name: &QualName, attrs: &[RAttr<'_>], marked: bool, void: bool) {
        let written = match (name.ns == ns!(html), &*name.local) {
            (true, "xmp" | "plaintext") => Written::AsPre,
            (true, "noembed") => Written::RawText("noembed"),
            (true, "noframes") => Written::RawText("noframes"),
            _ => Written::Plain,
        };
        if !void {
            self.open.push(written);
        }
        // pyro: the engine's attributes under their nonce names ([`engine_name`]).
        let nonce = self.nonce.clone();
        let named = |canonical: &str| engine_name(&nonce, canonical);
        let out = &mut self.out;
        out.push('<');
        if written == Written::AsPre {
            out.push_str("pre ");
            out.push_str(&named(TAG_ATTRIBUTE));
            out.push_str("=\"");
            out.push_str(&name.local);
            out.push('"');
        } else {
            out.push_str(tag);
        }
        // The state attribute is for keyed trees: markup would lose its namespace. Markup, which
        // another engine may load, carries no event handler (`on*`) attributes; keyed trees keep
        // them for `content: attr()`.
        let html = name.ns == ns!(html);
        for a in attrs.iter().filter(|a| {
            *a.name.ns != *state_ns(&self.nonce) && !(html && a.name.ns == ns!() && a.name.local.starts_with("on"))
        }) {
            out.push(' ');
            match &a.name.prefix {
                Some(p) if !p.is_empty() => {
                    out.push_str(p);
                    out.push(':');
                    out.push_str(&a.name.local);
                }
                _ => out.push_str(&a.name.local),
            }
            if !a.valueless {
                out.push_str("=\"");
                escape_render(out, &a.value, true);
                out.push('"');
            }
        }
        if let (true, Some(name)) = (marked, self.mark) {
            out.push(' ');
            out.push_str(&named(name));
        }
        if let Some((_, (x, y))) = self.scroll.iter().find(|(id, _)| *id == key) {
            use std::fmt::Write;
            let _ = write!(out, " {}=\"{x}\" {}=\"{y}\"", named("data-pyro-scroll-left"), named("data-pyro-scroll-top"));
        }
        if let Some((_, (x, y))) = self.sticky.iter().find(|(id, _)| *id == key) {
            use std::fmt::Write;
            let _ = write!(out, " {}=\"{x} {y}\"", named("data-pyro-sticky"));
        }
        if self.keyed.contains(&key) {
            use std::fmt::Write;
            let _ = write!(out, " {}=\"{key}\"", named("data-pyro-key"));
        }
        out.push('>');
        if written == Written::AsPre {
            // A leading newline right after <pre> is dropped by parsers.
            out.push('\n');
        }
    }
    fn close(&mut self, tag: &str) {
        let tag = if self.open.pop() == Some(Written::AsPre) { "pre" } else { tag };
        self.out.push_str("</");
        self.out.push_str(tag);
        self.out.push('>');
    }
    fn text(&mut self, _key: u64, text: &str, kind: TextKind) {
        match kind {
            TextKind::Node => match self.open.last() {
                Some(Written::RawText(tag)) => self.out.push_str(&raw_text_safe(text, tag)),
                _ => escape_render(&mut self.out, text, false),
            },
            TextKind::Style => self.out.push_str(&raw_text_safe(text, "style")),
            TextKind::Textarea => {
                // A leading newline right after <textarea> is dropped by parsers.
                self.out.push('\n');
                escape_render(&mut self.out, text, false);
            }
        }
    }
}

#[derive(Default)]
struct TreeSink {
    /// pyro: as [`HtmlSink::nonce`].
    nonce: String,
    tree: RenderTree,
    /// Open elements (indices into `tree.nodes`).
    open: Vec<u32>,
}

impl TreeSink {
    fn parent(&self) -> u32 {
        self.open.last().copied().unwrap_or(TOP)
    }
}

impl Sink for TreeSink {
    fn doctype(&mut self, _name: &str) {}
    fn open(&mut self, key: u64, _tag: &str, name: &QualName, attrs: &[RAttr<'_>], marked: bool, void: bool) {
        // (The engine's attributes come under their nonce names from the walk.)
        let attrs: Vec<(QualName, String)> = attrs
            .iter()
            .map(|a| (a.name.clone().into_owned(), if a.valueless { String::new() } else { a.value.to_string() }))
            .collect();
        let parent = self.parent();
        self.tree.nodes.push(RenderNode { key, parent, kind: RenderNodeKind::Element { name: name.clone(), attrs } });
        if marked {
            self.tree.mark = Some(key);
        }
        if !void {
            self.open.push(self.tree.nodes.len() as u32 - 1);
        }
    }
    fn close(&mut self, _tag: &str) {
        self.open.pop();
    }
    fn text(&mut self, key: u64, text: &str, _kind: TextKind) {
        let parent = self.parent();
        self.tree.nodes.push(RenderNode { key, parent, kind: RenderNodeKind::Text(text.to_owned()) });
    }
}

impl Dom {
    /// Serializes document `doc` for rendering (see the module docs).
    pub fn render_html(&self, doc: NodeId, opts: &RenderHtml<'_>) -> DomResult<String> {
        self.render_html_scrolled(doc, opts, &[])
    }

    /// Writes scroll offsets into the render copy without modifying the live DOM.
    pub fn render_html_scrolled(
        &self,
        doc: NodeId,
        opts: &RenderHtml<'_>,
        scroll: &[(u64, (f64, f64))],
    ) -> DomResult<String> {
        self.render_html_positioned(doc, opts, scroll, &[])
    }

    /// [`Dom::render_html_scrolled`] that also marks how far each `position: sticky` element is
    /// moved (CSS px, by render key) in a `data-pyro-sticky="x y"` attribute.
    pub fn render_html_positioned(
        &self,
        doc: NodeId,
        opts: &RenderHtml<'_>,
        scroll: &[(u64, (f64, f64))],
        sticky: &[(u64, (f64, f64))],
    ) -> DomResult<String> {
        self.render_html_keyed(doc, opts, scroll, sticky, &[])
    }

    /// pyro: [`Dom::render_html_positioned`] with the elements of `keyed` (render keys) written
    /// with a `data-pyro-key` attribute (a frame's animated elements).
    pub fn render_html_keyed(
        &self,
        doc: NodeId,
        opts: &RenderHtml<'_>,
        scroll: &[(u64, (f64, f64))],
        sticky: &[(u64, (f64, f64))],
        keyed: &[u64],
    ) -> DomResult<String> {
        let nonce = self.snapshot_nonce(self.check(doc)?);
        let mut sink = HtmlSink {
            out: String::with_capacity(64 * 1024),
            mark: opts.mark.map(|m| m.1),
            scroll,
            sticky,
            keyed,
            open: Vec::new(),
            nonce,
        };
        self.render_walk(doc, opts, &mut sink)?;
        Ok(sink.out)
    }

    /// pyro: the `href` of an SVG `<image>`, `<feImage>` or `<use>` element `n` that names another
    /// resource (not an element of the document, not a `data:` URL), resolved against its
    /// document's base URL with the query in the document's encoding, as Chromium resolves it
    /// (HTML "encoding-parse a URL"; an unmappable character becomes an escaped `&#...;`). `None`
    /// when the document's output encoding is UTF-8: the renderer resolves the value as it is.
    fn svg_reference_in_encoding(&self, n: u32, value: &str) -> Option<String> {
        let value = value.trim();
        if value.is_empty() || value.starts_with('#') || value.get(..5).is_some_and(|s| s.eq_ignore_ascii_case("data:"))
        {
            return None;
        }
        let doc = self.id_of(self.n(n).owner_doc);
        let enc = encoding_rs::Encoding::for_label(self.document_data(doc).ok()?.charset.as_bytes())?.output_encoding();
        if enc == encoding_rs::UTF_8 {
            return None;
        }
        let base = url::Url::parse(self.document_base_url(doc).ok()?).ok()?;
        let encode: &dyn Fn(&str) -> Cow<'_, [u8]> = &|query| {
            let mut encoder = enc.new_encoder();
            let mut out = Vec::new();
            let mut rest = query;
            loop {
                let needed =
                    encoder.max_buffer_length_from_utf8_without_replacement(rest.len()).unwrap_or(rest.len() * 4 + 8);
                out.reserve(needed);
                let (result, read) = encoder.encode_from_utf8_to_vec_without_replacement(rest, &mut out, true);
                rest = &rest[read..];
                match result {
                    encoding_rs::EncoderResult::InputEmpty => return Cow::Owned(out),
                    encoding_rs::EncoderResult::OutputFull => {}
                    encoding_rs::EncoderResult::Unmappable(c) => {
                        out.extend_from_slice(format!("%26%23{}%3B", c as u32).as_bytes());
                    }
                }
            }
        };
        url::Url::options().base_url(Some(&base)).encoding_override(Some(encode)).parse(value).ok().map(String::from)
    }

    /// The nodes [`Dom::render_html`] would serialize, keyed (see
    /// [`RenderTree`]).
    pub fn render_tree(&self, doc: NodeId, opts: &RenderHtml<'_>) -> DomResult<RenderTree> {
        let mut sink = TreeSink { nonce: self.snapshot_nonce(self.check(doc)?), ..Default::default() };
        self.render_walk(doc, opts, &mut sink)?;
        Ok(sink.tree)
    }

    fn render_walk(&self, doc: NodeId, opts: &RenderHtml<'_>, sink: &mut dyn Sink) -> DomResult<()> {
        let d = self.check(doc)?;
        let root = self.child_idx_iter(d).find(|&c| self.n(c).is_element()).unwrap_or(NIL);
        self.render_walk_from(self.child_idx_iter(d).collect(), opts, sink, root, false);
        Ok(())
    }

    /// Pushes the `<style>` steps of the sheets `scope` adopts (rendered
    /// after what `stack` holds above them), one per distinct sheet, at the
    /// position of its last occurrence (Chrome: a sheet adopted twice applies
    /// once, last; two sheets with the same text are still two sheets);
    /// `emitted` counts the adopted sheets rendered so far (their keys).
    fn push_adopted(&self, stack: &mut Vec<Step>, opts: &RenderHtml<'_>, scope: u32, emitted: &mut usize) {
        let Some(adopted) = opts.adopted else { return };
        let mut sheets = Vec::new();
        let list = adopted(self.id_of(scope));
        for (i, (id, text)) in list.iter().enumerate() {
            if !text.is_empty() && !list[i + 1..].iter().any(|(later, _)| later == id) {
                *emitted += 1;
                sheets.push((adopted_key(*emitted), text.clone()));
            }
        }
        for (key, text) in sheets.into_iter().rev() {
            stack.push(Step::Adopted(key, text));
        }
    }

    /// The render walk over the subtrees of `roots` (in order), outside any
    /// shadow tree. `root` is the document element (whose rendered children
    /// end with the document's adopted sheets), `NIL` when not walked.
    /// `single`: stop after the first node written to `sink` (its own
    /// content, no subtree).
    fn render_walk_from(&self, roots: Vec<u32>, opts: &RenderHtml<'_>, sink: &mut dyn Sink, root: u32, single: bool) {
        // pyro: the nonce of the document walked (its engine attributes' names, [`state_ns`]).
        let walk_nonce = roots.first().map(|&r| self.snapshot_nonce(self.n(r).owner_doc)).unwrap_or_default();
        let mark = opts.mark.and_then(|(id, _)| self.check(id).ok());
        // Shadow hosts entered: (host, the host's own scope).
        let mut hosts: Vec<(u32, Scope)> = Vec::new();
        // Tags of the open elements, for `close`.
        let mut tags: Vec<String> = Vec::new();
        // Adopted sheets rendered so far.
        let mut emitted = 0usize;
        // The element index of each light child of a shadow host or `<details>` (`LIGHT_INDEX_ATTR`),
        // counted once per parent: one pass over its children, however many it has.
        let mut light_indices: rustc_hash::FxHashMap<u32, usize> = Default::default();
        let mut light_counts: rustc_hash::FxHashMap<u32, usize> = Default::default();
        let mut stack = Vec::new();
        push_children(&mut stack, roots, NONE);
        while let Some(step) = stack.pop() {
            let (n, scope) = match step {
                Step::Synthetic(e, text, kind) => {
                    sink.text(render_key(self.id_of(e)) | SYNTHETIC_TEXT, &text, kind);
                    continue;
                }
                Step::Close => {
                    let tag = tags.pop().unwrap_or_default();
                    sink.close(&tag);
                    continue;
                }
                Step::Adopted(key, text) => {
                    let name = QualName::new(None, ns!(html), markup5ever::local_name!("style"));
                    sink.open(key, "style", &name, &[], false, false);
                    sink.text(key | SYNTHETIC_TEXT, &text, TextKind::Style);
                    sink.close("style");
                    continue;
                }
                Step::DetailsSlot(key, content, kids, kids_scope) => {
                    let name = QualName::new(None, ns!(html), markup5ever::local_name!("slot"));
                    let marker = |name: &str| RAttr {
                        name: Cow::Owned(QualName::new(None, state_ns(&walk_nonce).into(), name.into())),
                        value: Cow::Borrowed(""),
                        valueless: true,
                    };
                    let mut attrs = vec![marker(ASSIGNED_SLOT_ATTR)];
                    if content {
                        attrs.push(marker(DETAILS_CONTENT_ATTR));
                    }
                    sink.open(key, "slot", &name, &attrs, false, false);
                    tags.push("slot".into());
                    stack.push(Step::Close);
                    push_children(&mut stack, kids, kids_scope);
                    continue;
                }
                Step::Enter(n, scope) => (n, scope),
            };
            match &self.n(n).kind {
                NodeKind::Doctype(dt) => sink.doctype(&dt.name),
                NodeKind::Text(s) | NodeKind::CData(s) => {
                    sink.text(render_key(self.id_of(n)), s, TextKind::Node);
                    if single {
                        return;
                    }
                }
                NodeKind::Comment(_) | NodeKind::ProcessingInstruction(_) | NodeKind::Attr(_) => {}
                NodeKind::Document(_) | NodeKind::Fragment(_) => {
                    push_children(&mut stack, self.child_idx_iter(n).collect(), scope);
                }
                NodeKind::Element(e) => {
                    let html = e.is_html();
                    let local: &str = &e.name.local;
                    // pyro: `<noscript>` and `<template>` are in the tree without their content, as a
                    // script is: they have a computed style (`getComputedStyle`, WPT
                    // html/rendering/bidi-rendering/unicode-bidi-ua-rules) and count for sibling and
                    // structural selectors; the UA sheet gives them `display: none`.
                    let childless = html && matches!(local, "noscript" | "template");
                    // pyro: a script element is in the tree, without its text: it counts for
                    // sibling and structural selectors, and `script { display: block }` boxes
                    // render (WPT CSS2 `content-072`, `-090`, `-138`).
                    let is_script = local == "script" && (html || e.name.ns == ns!(svg));
                    // A slot in a shadow tree: its children are the host's light children
                    // assigned to it, else its fallback content.
                    let slot_kids = (html && local == "slot" && scope != NONE).then(|| {
                        let (host, host_scope) = hosts[scope];
                        let name = e.attr("name").unwrap_or("");
                        // Only the slot the root assigns that name to: a second slot of the
                        // name rendered the same children again (two nodes of one key, which
                        // the renderer refuses: no screenshot).
                        let assigned = if self.assigns_manually(n) {
                            self.slot_assigned(n)
                        } else if self.takes_assignment(n, name) {
                            self.slotted(host, name)
                        } else {
                            Vec::new()
                        };
                        if assigned.is_empty() {
                            (self.child_idx_iter(n).collect(), scope, false)
                        } else {
                            (assigned, host_scope, true)
                        }
                    });
                    let id = self.id_of(n);
                    let is_sheet_link = html
                        && local == "link"
                        && e.attr("rel")
                            .is_some_and(|r| r.split_ascii_whitespace().any(|t| t.eq_ignore_ascii_case("stylesheet")));
                    let href = if is_sheet_link {
                        match (opts.stylesheet_href)(id) {
                            Some(href) => Some(href),
                            None => continue,
                        }
                    } else {
                        None
                    };
                    let input_type = (html && local == "input").then(|| self.input_type_idx(n));
                    // The source an `<img>` selected from `srcset` or `<picture>` is its `src`
                    // to the renderer, which reads no other.
                    // pyro: a lazy `<img>` whose fetch waits has no `src` there at all (the renderer
                    // would fetch it at its first layout).
                    let image_deferred = html && local == "img" && self.image_deferred(id);
                    let image_source = (html && local == "img" && !image_deferred).then(|| self.image_source(id)).flatten();
                    let mut attrs: Vec<RAttr<'_>> = Vec::with_capacity(e.attrs.len() + 1);
                    for a in &e.attrs {
                        let name: &str = &a.name.local;
                        // pyro: a page attribute in the state namespace is the page's own: the
                        // engine's state travels in the nonce namespace ([`state_ns`]).
                        // pyro: a frame element's source attributes in any other namespace would
                        // reparse as its own `src`/`srcdoc`/`data`: never written (the renderer
                        // loads only frame table entries anyway).
                        if html
                            && a.name.ns != ns!()
                            && matches!(local, "iframe" | "frame" | "object" | "embed")
                            && FRAME_SOURCE_ATTRIBUTES.iter().any(|n| n.eq_ignore_ascii_case(&a.name.local))
                        {
                            continue;
                        }
                        if a.name.ns == ns!() {
                            let skip = match (local, name) {
                                ("input", "value" | "checked") => input_type.is_some(),
                                ("option", "selected") => html,
                                ("link", "href") => href.is_some(),
                                ("iframe" | "frame", "src") => html,
                                // An `<object>` showing a frame document: its `data` is not an image.
                                ("object", "data" | "srcdoc") | ("embed", "src" | "srcdoc") => {
                                    html && self.frame_documents.contains_key(&n)
                                }
                                ("img", "src") => image_source.is_some() || image_deferred,
                                // pyro: a lazy `<video>` whose load waits has no `poster` there either: the
                                // renderer would fetch the poster at its first layout, far below the viewport.
                                ("video", "poster") => html && self.image_deferred(id),
                                // pyro: a script-made template's `shadowrootmode` would reparse as
                                // a declarative shadow root of its parent in the renderer.
                                ("template", "shadowrootmode") => html,
                                _ => false,
                            };
                            if skip {
                                continue;
                            }
                        }
                        // pyro: an SVG reference to another resource the renderer fetches, resolved
                        // with its query in the document's encoding (`svg_reference_in_encoding`).
                        let encoded = (e.name.ns == ns!(svg)
                            && matches!(local, "image" | "feImage" | "use")
                            && name == "href"
                            && (a.name.ns == ns!() || a.name.ns == ns!(xlink)))
                        .then(|| self.svg_reference_in_encoding(n, &a.value))
                        .flatten();
                        attrs.push(RAttr {
                            name: Cow::Borrowed(&a.name),
                            value: match encoded {
                                Some(url) => Cow::Owned(url),
                                None => Cow::Borrowed(&a.value),
                            },
                            valueless: false,
                        });
                    }
                    // pyro: what follows is the engine's; the page's attributes above are
                    // written after it, without any that would collide (`ENGINE_ATTRIBUTES`).
                    let page_end = attrs.len();
                    let plain = |local: &str| Cow::Owned(QualName::new(None, ns!(), local.into()));
                    // Keep the reflected attribute available to CSS attribute selectors.
                    // Only the declaration parser consumes this private snapshot value.
                    // pyro: a `style` attribute CSP blocked declares nothing in the renderer too.
                    let blocked_style = self.style_attribute_blocked_idx(n);
                    if let Some(style) = if blocked_style { Some("") } else { e.semantic_inline_style() } {
                        attrs.push(RAttr {
                            name: Cow::Owned(QualName::new(None, STATE_NS.into(), "inline-declarations".into())),
                            value: Cow::Borrowed(style),
                            valueless: false,
                        });
                    }
                    if let Some(href) = href {
                        attrs.push(RAttr { name: plain("href"), value: Cow::Owned(href), valueless: false });
                        attrs.push(RAttr { name: plain(SHEET_ATTRIBUTE), value: Cow::Owned(self.sheet_key(id)), valueless: false });
                        if let Some(policy) = self.sheet_policies.get(&n) {
                            attrs.push(RAttr {
                                name: plain(SHEET_POLICY_ATTRIBUTE),
                                value: Cow::Borrowed(policy),
                                valueless: false,
                            });
                        }
                    }
                    if let Some(source) = image_source {
                        attrs.push(RAttr { name: plain("src"), value: Cow::Borrowed(source), valueless: false });
                    }
                    if let Some(ty) = input_type {
                        if matches!(ty, InputType::Checkbox | InputType::Radio) {
                            if self.checked(id).unwrap_or(false) {
                                attrs.push(RAttr { name: plain("checked"), value: Cow::Borrowed(""), valueless: true });
                            }
                            // Their value is not displayed.
                            if let Some(v) = e.attr("value") {
                                attrs.push(RAttr { name: plain("value"), value: Cow::Borrowed(v), valueless: false });
                            }
                        } else if ty != InputType::File {
                            let v = self.value(id).unwrap_or_default();
                            let v = if ty == InputType::Password { "•".repeat(v.chars().count()) } else { v };
                            // pyro: and a button's empty label, which is still a line (blitz-dom's
                            // mutator), where the attribute says so.
                            let empty_label = matches!(ty, InputType::Button | InputType::Submit | InputType::Reset)
                                && e.attr("value").is_some();
                            if !v.is_empty() || empty_label {
                                attrs.push(RAttr { name: plain("value"), value: Cow::Owned(v), valueless: false });
                            }
                        }
                    }
                    if html && matches!(local, "iframe" | "frame" | "object" | "embed") {
                        if let Some(frame) = self.frame_documents.get(&n) {
                            // pyro: only the index of its entry in the frame table (and the
                            // entry's version, so a new document loads again).
                            attrs.push(RAttr {
                                name: plain(FRAME_ATTRIBUTE),
                                value: Cow::Owned(format!(
                                    "{}:{}:{:016x}",
                                    self.snapshot_nonce(self.n(n).owner_doc),
                                    render_key(id),
                                    frame.version()
                                )),
                                valueless: false,
                            });
                        }
                    }
                    if html && local == "iframe" {
                        if let Some((width, height)) = self.frame_content_sizes.get(&n) {
                            attrs.push(RAttr {
                                name: plain("data-pyro-frame-content-size"),
                                value: Cow::Owned(format!("{width},{height}")),
                                valueless: false,
                            });
                        }
                    }
                    if html && local == "option" && self.option_selected(id).unwrap_or(false) {
                        attrs.push(RAttr { name: plain("selected"), value: Cow::Borrowed(""), valueless: true });
                    }
                    if let Some(states) = self.render_states(n) {
                        attrs.push(RAttr {
                            name: Cow::Owned(QualName::new(None, STATE_NS.into(), "state".into())),
                            value: Cow::Owned(states),
                            valueless: false,
                        });
                    }
                    if let Some(states) = self.render_custom_states(n) {
                        attrs.push(RAttr {
                            name: Cow::Owned(QualName::new(None, STATE_NS.into(), "custom-states".into())),
                            value: Cow::Owned(states),
                            valueless: false,
                        });
                    }
                    let marker = |name: &str| RAttr {
                        name: Cow::Owned(QualName::new(None, state_ns(&walk_nonce).into(), name.into())),
                        value: Cow::Borrowed(""),
                        valueless: true,
                    };
                    if slot_kids.as_ref().is_some_and(|(_, _, assigned)| *assigned) {
                        attrs.push(marker(ASSIGNED_SLOT_ATTR));
                    }
                    if let Some(&(tx, ty, scale)) = self.svg_pans.get(&n).filter(|_| e.name.ns == ns!(svg) && local == "svg") {
                        attrs.push(RAttr {
                            name: Cow::Owned(QualName::new(None, STATE_NS.into(), SVG_PAN_ATTR.into())),
                            value: Cow::Owned(format!("{tx} {ty} {scale}")),
                            valueless: false,
                        });
                    }
                    // A light child of a shadow host (rendered under the slot it is assigned to).
                    // pyro: or of a `<details>` element (its user-agent shadow tree's slots).
                    let parent = self.n(n).parent;
                    if parent != NIL
                        && (self.n(parent).element().is_some_and(|p| p.shadow_root != NIL && !self.has_ua_shadow_idx(p.shadow_root))
                            || self.renders_details_slots(parent))
                    {
                        let count = *light_counts.entry(parent).or_insert_with(|| {
                            let mut count = 0;
                            for child in self.child_idx_iter(parent).filter(|&c| self.n(c).is_element()) {
                                light_indices.insert(child, count);
                                count += 1;
                            }
                            count
                        });
                        let index = light_indices.get(&n).copied().unwrap_or(0);
                        attrs.push(RAttr {
                            name: Cow::Owned(QualName::new(None, STATE_NS.into(), LIGHT_INDEX_ATTR.into())),
                            value: Cow::Owned(format!("{index}/{count}")),
                            valueless: false,
                        });
                    }
                    // A user-agent shadow root (`<use>` instance) renders as if absent.
                    let rendered_shadow = e.shadow_root != NIL && !self.has_ua_shadow_idx(e.shadow_root);
                    let details_slots = self.renders_details_slots(n);
                    if rendered_shadow || details_slots {
                        attrs.push(marker(SHADOW_HOST_ATTR));
                    }
                    // pyro: the engine's attributes first, then the page's that collide with none.
                    let page: Vec<RAttr<'_>> = attrs.drain(..page_end).collect();
                    // pyro: the engine's reserved ones under their nonce names ([`engine_name`]).
                    let nonce = self.snapshot_nonce(self.n(n).owner_doc);
                    for a in attrs.iter_mut() {
                        if a.name.ns == ns!() && a.name.local.starts_with(ENGINE_ATTRIBUTE_PREFIX) {
                            let renamed = engine_name(&nonce, &a.name.local);
                            a.name = Cow::Owned(QualName::new(None, ns!(), renamed.into()));
                        } else if &*a.name.ns == STATE_NS {
                            // pyro: and its state in the nonce namespace ([`state_ns`]).
                            a.name = Cow::Owned(QualName::new(None, state_ns(&nonce).into(), a.name.local.clone()));
                        }
                    }
                    // (The engine's state, in the nonce namespace, collides with no page attribute.)
                    let own_state_ns = state_ns(&nonce);
                    let mut engine: Vec<String> = attrs
                        .iter()
                        .filter(|a| *a.name.ns != *own_state_ns)
                        .map(|a| match &a.name.prefix {
                            Some(prefix) if !prefix.is_empty() => format!("{prefix}:{}", a.name.local),
                            _ => a.name.local.to_string(),
                        })
                        .map(|name| name.to_ascii_lowercase())
                        .collect();
                    if let Some((_, name)) = opts.mark {
                        engine.push(name.to_ascii_lowercase());
                    }
                    attrs.extend(page.into_iter().filter(|a| page_attribute_allowed(&a.name, &engine, &nonce)));
                    let tag = self.serialized_tag(n);
                    let void = html && is_void(local);
                    sink.open(render_key(id), &tag, &e.name, &attrs, mark == Some(n), void);
                    if single {
                        return;
                    }
                    if void {
                        continue;
                    }
                    tags.push(tag);
                    stack.push(Step::Close);
                    if is_script || childless {
                        continue;
                    }
                    if html && local == "style" {
                        let text = (opts.style_text)(id).unwrap_or_else(|| self.child_text_content(id).into_owned());
                        stack.push(Step::Synthetic(n, text, TextKind::Style));
                        continue;
                    }
                    // pyro: an SVG `<style>` whose type is not CSS has no sheet: the renderer gets it
                    // empty (an HTML one's text says so through `style_text`).
                    if local == "style" && e.name.ns == ns!(svg) && !Dom::style_type_is_css(e.attr("type")) {
                        continue;
                    }
                    if html && local == "textarea" {
                        stack.push(Step::Synthetic(n, self.value(id).unwrap_or_default(), TextKind::Textarea));
                        continue;
                    }
                    // pyro: a raw-text element's text is one stream to a parser: written whole (its
                    // text nodes joined), so an end tag split across nodes is seen and neutralised
                    // ([`raw_text_safe`]); element children would be text in a reparse anyway.
                    if html && matches!(local, "noembed" | "noframes") {
                        stack.push(Step::Synthetic(n, self.child_text_content(id).into_owned(), TextKind::Node));
                        continue;
                    }
                    if html && (matches!(local, "iframe" | "frame") || (local == "object" && self.frame_documents.contains_key(&n))) {
                        continue;
                    }
                    if let Some((kids, kids_scope, _)) = slot_kids {
                        push_children(&mut stack, kids, kids_scope);
                    } else if details_slots {
                        // pyro: its first `<summary>` in the summary slot, everything else in the
                        // content slot (Chromium's user-agent shadow tree for `<details>`).
                        let kids: Vec<u32> = self.child_idx_iter(n).collect();
                        let summary = kids.iter().copied().find(|&k| {
                            self.n(k).element().is_some_and(|c| c.is_html() && &*c.name.local == "summary")
                        });
                        let rest: Vec<u32> = kids.iter().copied().filter(|&k| Some(k) != summary).collect();
                        let key = render_key(id);
                        stack.push(Step::DetailsSlot(key | DETAILS_SUMMARY_SLOT_KEY | DETAILS_CONTENT_SLOT_KEY, true, rest, scope));
                        stack.push(Step::DetailsSlot(key | DETAILS_SUMMARY_SLOT_KEY, false, summary.into_iter().collect(), scope));
                    } else if rendered_shadow {
                        hosts.push((n, scope));
                        // pyro: its light `<style>` children no slot takes still style its tree scope:
                        // rendered after its shadow tree in a slot marked assigned (`<style>` boxes are
                        // `display: none`).
                        let styles: Vec<u32> = self
                            .child_idx_iter(n)
                            .filter(|&k| self.n(k).element().is_some_and(|c| c.is_html() && &*c.name.local == "style"))
                            .collect();
                        // (the shadow tree's slots looked up once for all of them)
                        let unassigned = if styles.is_empty() { styles } else { self.unassigned_children(n, styles) };
                        if !unassigned.is_empty() {
                            stack.push(Step::DetailsSlot(render_key(id) | UNASSIGNED_STYLE_SLOT_KEY, false, unassigned, scope));
                        }
                        self.push_adopted(&mut stack, opts, e.shadow_root, &mut emitted);
                        push_children(&mut stack, self.child_idx_iter(e.shadow_root).collect(), hosts.len() - 1);
                    } else {
                        if n == root && self.n(n).parent != NIL {
                            self.push_adopted(&mut stack, opts, self.n(n).parent, &mut emitted);
                        }
                        push_children(&mut stack, self.child_idx_iter(n).collect(), scope);
                    }
                }
            }
        }
    }

    /// What changed in [`Dom::render_tree`] of `doc` since DOM version
    /// `since`, from the change journal: element attributes and text that
    /// changed in place, and the new render children of every node whose
    /// children changed, with the subtrees of those inserted. `None` when the
    /// journal does not cover `since` or a change needs the whole tree
    /// (unknown changes, shadow trees, `<textarea>`/`<link>`/frames, whose
    /// render content is not their DOM content). A change of a `<style>`'s
    /// content gives its sheet text.
    pub fn render_delta(&self, doc: NodeId, since: u64, opts: &RenderHtml<'_>) -> Option<RenderDelta> {
        let d = self.check(doc).ok()?;
        // (User-agent shadow trees too: their nodes are not in the render tree.)
        if self.shadow_root_count() != 0 || opts.mark.is_some() || opts.adopted.is_some() {
            return None;
        }
        let mut changed: Vec<u32> = Vec::new();
        // Attribute-only textarea patches do not replace the synthetic value
        // child. A state change in the same interval must keep the fallback.
        let mut textarea_attributes = rustc_hash::FxHashMap::<u32, bool>::default();
        let is_textarea =
            |i| self.slot_live(i) && self.n(i).element().is_some_and(|e| e.is_html() && &*e.name.local == "textarea");
        let mut parents: Vec<u32> = Vec::new();
        let mut inserted: rustc_hash::FxHashSet<u32> = rustc_hash::FxHashSet::default();
        for c in self.changes_since(since)? {
            match c {
                Change::Attr(i, _, _) => {
                    if is_textarea(i) {
                        textarea_attributes.entry(i).or_insert(true);
                    }
                    changed.push(i);
                }
                Change::State(i) | Change::Text(i) => {
                    if is_textarea(i) {
                        textarea_attributes.insert(i, false);
                    }
                    changed.push(i);
                }
                Change::Inserted(p, c) => {
                    parents.push(p);
                    inserted.insert(c);
                }
                Change::Removed(p, n, _) => {
                    parents.push(p);
                    // A removed radio leaves its group (the radios still in
                    // the document revalidate).
                    if self.slot_live(n) && self.slot_live(p) {
                        let is_radio = |k: u32| {
                            self.n(k).element().is_some_and(|e| {
                                e.is_html()
                                    && &*e.name.local == "input"
                                    && e.attr("type").is_some_and(|t| t.eq_ignore_ascii_case("radio"))
                            })
                        };
                        // The removed node or anything inside it.
                        let mut radio = false;
                        let mut k = n;
                        while k != NIL && !radio {
                            radio = is_radio(k);
                            k = self.following_in(k, n);
                        }
                        if radio {
                            let mut first = Vec::new();
                            let root = self.root_idx(p);
                            let mut k = root;
                            while k != NIL {
                                if is_radio(k) {
                                    first.push(k);
                                }
                                k = self.following_in(k, root);
                            }
                            changed.extend(first);
                        }
                    }
                }
                Change::Pseudo => {}
                Change::Unknown(_) => return None,
            }
        }
        // Where a node is rendered: connected to `doc`, outside elements whose
        // children are not rendered. `Err`: in an element rendered specially.
        let rendered = |mut i: u32| -> Result<bool, ()> {
            if !self.slot_live(i) {
                return Ok(false);
            }
            loop {
                if i == d {
                    return Ok(true);
                }
                if let NodeKind::Element(e) = &self.n(i).kind {
                    let local: &str = &e.name.local;
                    if e.is_html() && matches!(local, "script" | "noscript" | "template" | "iframe" | "frame") {
                        return Ok(false);
                    }
                    if e.is_html() && matches!(local, "style" | "textarea" | "link" | "slot") {
                        return Err(());
                    }
                    if e.name.ns == ns!(svg) && local == "script" {
                        return Ok(false);
                    }
                }
                let parent = self.n(i).parent;
                if parent == NIL {
                    return Ok(false);
                }
                i = parent;
            }
        };
        // The `<style>` element `i` is or is in: its render content is its
        // sheet's text, its child text content, which only a change of its
        // own children replaces (nothing deeper in it is rendered).
        let style_owner = |mut i: u32| -> Option<u32> {
            while i != NIL && i != d {
                if let NodeKind::Element(e) = &self.n(i).kind {
                    if e.is_html() && &*e.name.local == "style" {
                        return Some(i);
                    }
                }
                i = self.n(i).parent;
            }
            None
        };
        let mut styles: Vec<u32> = Vec::new();
        // Nodes inside a subtree that is serialized whole (an inserted one).
        let covered = |mut i: u32| {
            i = self.n(i).parent;
            while i != NIL && i != d {
                if inserted.contains(&i) {
                    return true;
                }
                i = self.n(i).parent;
            }
            false
        };
        let mut ops = Vec::new();
        let mut seen = rustc_hash::FxHashSet::default();
        parents.sort_unstable();
        parents.dedup();
        for &p in &parents {
            // pyro: a `<details>` element whose children came or went renders them in its slots
            // again (its first `<summary>` may be another one): the whole tree. A change deeper in
            // it, or of its attributes (`open`), is a change of the nodes alone, which keep their
            // keys in the slots.
            if rendered(p) == Ok(true) && self.renders_details_slots(p) {
                return None;
            }
            match rendered(p) {
                Err(()) => {
                    // A child of the `<style>` came or went; deeper in it,
                    // nothing rendered changed.
                    let owner = style_owner(p)?;
                    if owner == p {
                        styles.push(owner);
                    }
                    continue;
                }
                Ok(false) => continue,
                Ok(true) => {}
            }
            // An inserted node's children are in its own serialized subtree.
            if covered(p) || inserted.contains(&p) || !seen.insert(p) {
                continue;
            }
            let kids: Vec<u32> = self.child_idx_iter(p).filter(|&k| self.render_child(k)).collect();
            let fresh: Vec<u32> = kids.iter().copied().filter(|k| inserted.contains(k)).collect();
            let mut sink = TreeSink { nonce: self.snapshot_nonce(d), ..Default::default() };
            self.render_walk_from(fresh, opts, &mut sink, NIL, false);
            ops.push(DeltaOp::Children {
                parent: (p != d).then(|| render_key(self.id_of(p))),
                kids: kids.iter().map(|&k| render_key(self.id_of(k))).collect(),
                nodes: sink.tree.nodes,
            });
        }
        // Validity is not local: a form or fieldset follows its controls,
        // and a required radio group spans its radios. The directionality of a
        // `dir=auto` element (or a `<bdi>`) follows its content.
        let mut dependents = Vec::new();
        for &i in changed.iter().chain(parents.iter()) {
            self.state_dependents(i, &mut dependents);
            self.auto_dir_ancestors(i, &mut dependents);
        }
        // `state_dependents` starts above the node: a form, fieldset or select that gained or lost
        // a child follows it too (WPT pseudo-classes/valid-invalid: an invalid control appended to
        // an empty form).
        for &p in parents.iter() {
            let follows_children = self.slot_live(p)
                && self
                    .n(p)
                    .element()
                    .is_some_and(|e| e.is_html() && matches!(&*e.name.local, "form" | "fieldset" | "select"));
            if follows_children {
                dependents.push(p);
            }
        }
        // A fieldset, optgroup or select whose own attributes changed (`disabled`) can disable or
        // enable the controls and options inside it.
        for &i in changed.iter() {
            self.disabled_dependents(i, &mut dependents);
        }
        changed.extend(dependents);
        changed.sort_unstable();
        changed.dedup();
        for i in changed {
            // Removed since: nothing to render.
            if !self.slot_live(i) {
                continue;
            }
            // A `<style>`'s own attributes: the element (its text stays its
            // sheet's).
            let is_style = style_owner(i) == Some(i);
            // A frame's own attributes (its children are not rendered, it is).
            let is_frame = matches!(&self.n(i).kind, NodeKind::Element(e)
                if e.is_html() && (matches!(&*e.name.local, "iframe" | "frame")
                    || (matches!(&*e.name.local, "object" | "embed") && self.frame_documents.contains_key(&i))));
            // A script's, noscript's or template's own attributes (`dir`, `class`): the element,
            // which is rendered though its content is not.
            let is_childless = matches!(&self.n(i).kind, NodeKind::Element(e)
                if (e.is_html() && matches!(&*e.name.local, "script" | "noscript" | "template"))
                    || (e.name.ns == ns!(svg) && &*e.name.local == "script"));
            let own_attributes = is_style || is_frame || is_childless || textarea_attributes.get(&i) == Some(&true);
            let place =
                if own_attributes && self.n(i).parent != NIL { rendered(self.n(i).parent) } else { rendered(i) };
            match place {
                // Inside a `<style>`: its sheet text.
                Err(()) => match style_owner(i).filter(|&owner| owner != i) {
                    Some(owner) => {
                        // A child's data: the sheet text. Deeper: nothing
                        // rendered.
                        if self.n(i).parent == owner {
                            styles.push(owner);
                        }
                        continue;
                    }
                    None if covered(i) || inserted.contains(&i) => continue,
                    None => return None,
                },
                Ok(false) => continue,
                Ok(true) => {}
            }
            if covered(i) || inserted.contains(&i) {
                continue;
            }
            // The node alone: the walk stops at the first node it writes
            // (the node's own content, not its subtree).
            let mut sink = TreeSink { nonce: self.snapshot_nonce(d), ..Default::default() };
            self.render_walk_from(vec![i], opts, &mut sink, NIL, true);
            let Some(first) = sink.tree.nodes.into_iter().next() else { continue };
            if first.key != render_key(self.id_of(i)) {
                return None;
            }
            ops.push(DeltaOp::Node { key: first.key, kind: first.kind });
            if is_style {
                styles.push(i);
            }
        }
        // A `<style>` whose content changed: its sheet text (unless the
        // element itself is new, and serialized whole above).
        styles.sort_unstable();
        styles.dedup();
        for owner in styles {
            if covered(owner) || inserted.contains(&owner) {
                continue;
            }
            let parent = self.n(owner).parent;
            if parent == NIL {
                continue;
            }
            match rendered(parent) {
                Ok(true) => {}
                Ok(false) => continue,
                Err(()) => return None,
            }
            let id = self.id_of(owner);
            let text = (opts.style_text)(id).unwrap_or_else(|| self.child_text_content(id).into_owned());
            ops.push(DeltaOp::Node { key: render_key(id) | SYNTHETIC_TEXT, kind: RenderNodeKind::Text(text) });
        }
        Some(RenderDelta { ops })
    }

    /// The `<style>` sheet text the render tree gives `owner` (see
    /// [`Dom::render_delta`]), as a delta op.
    pub fn render_style_text(&self, owner: NodeId, opts: &RenderHtml<'_>) -> Option<DeltaOp> {
        let e = self.node(owner).ok()?.element()?;
        if !(e.is_html() && &*e.name.local == "style") {
            return None;
        }
        let text = (opts.style_text)(owner).unwrap_or_else(|| self.child_text_content(owner).into_owned());
        Some(DeltaOp::Node { key: render_key(owner) | SYNTHETIC_TEXT, kind: RenderNodeKind::Text(text) })
    }

    /// The pseudo-class states of element `n` a renderer cannot derive from
    /// its markup (validity, placeholder shown, read-write, ...), as the
    /// space-separated value of the [`STATE_NS`] `state` attribute; `None`
    /// for elements without any.
    fn render_states(&self, n: u32) -> Option<String> {
        use crate::select::PseudoClass::*;
        let e = self.n(n).element()?;
        let local: &str = &e.name.local;
        let control = matches!(local, "input" | "textarea" | "select" | "button" | "output");
        let kind = e.is_html()
            && (control
                || matches!(local, "form" | "fieldset" | "option" | "optgroup" | "details" | "dialog" | "progress")
                || e.attr("popover").is_some()
                || e.flags & crate::node::element_flags::FORM_ASSOCIATED != 0);
        let editable = e.attr("contenteditable").is_some();
        // An image button (`<input type=image>`) is laid out like `<img>`:
        // its natural size travels the same way.
        // A `<video>` with known metadata reports its video size the same way.
        let is_image = local == "img"
            || local == "video"
            || (local == "input" && e.attr("type").is_some_and(|t| t.eq_ignore_ascii_case("image")));
        let natural = (e.is_html() && is_image).then(|| self.natural_size(self.id_of(n))).flatten();
        // The directionality of an element whose content decides it (`dir=auto`, `<bdi>`).
        let auto_dir = self.has_auto_directionality(n).then(|| self.directionality_idx(n));
        // A custom element not defined yet is not `:defined`.
        let undefined = !e.is_defined();
        // pyro: the scope of an active element-scoped view transition (`Dom::set_view_transition_scope`).
        let scope = self.view_transition_scope_idx(n);
        let fullscreen = self.is_fullscreen_idx(n);
        if !kind && !editable && natural.is_none() && auto_dir.is_none() && !undefined && scope.is_none() && !fullscreen {
            return None;
        }
        let mut out = String::new();
        if undefined {
            out.push_str("undefined");
        }
        if let Some((w, h)) = natural {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(&format!("natural={w},{h}"));
        }
        if let Some(dir) = auto_dir {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(match dir {
                crate::direction::Direction::Ltr => "dir=ltr",
                crate::direction::Direction::Rtl => "dir=rtl",
            });
        }
        for (pc, name) in [
            (Valid, "valid"),
            (Invalid, "invalid"),
            (Indeterminate, "indeterminate"),
            (Required, "required"),
            (Optional, "optional"),
            (ReadWrite, "read-write"),
            (PlaceholderShown, "placeholder-shown"),
            (Default, "default"),
            (Open, "open"),
            (Modal, "modal"),
            (PopoverOpen, "popover-open"),
            (InRange, "in-range"),
            (OutOfRange, "out-of-range"),
            (UserValid, "user-valid"),
            (UserInvalid, "user-invalid"),
            // `:enabled`/`:disabled` with what Blitz cannot derive from the element alone: options
            // and optgroups, fieldsets, and controls a disabled fieldset or optgroup disables.
            (Enabled, "enabled"),
            (Disabled, "disabled"),
        ] {
            if self.matches_state(n, &pc) {
                if !out.is_empty() {
                    out.push(' ');
                }
                out.push_str(name);
            }
        }
        // pyro: a dialog in the top layer, modal or kept there for its `overlay` transition: the
        // UA sheet lays it out as a modal dialog (`:-moz-topmost-modal`).
        if self.is_dialog_in_top_layer_idx(n) {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str("dialog-in-top-layer");
        }
        if fullscreen {
            if !out.is_empty() {
                out.push(' ');
            }
            // `:modal` matches an element in fullscreen too.
            out.push_str("fullscreen modal");
        }
        if let Some(contained) = scope {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(if contained { "view-transition-scope view-transition-contain" } else { "view-transition-scope" });
        }
        // The top layer's paint order: renderers paint these elements above
        // the document by rank (one empty check without a top layer).
        if let Some(rank) = self.top_layer_rank(self.id_of(n)) {
            if !out.is_empty() {
                out.push(' ');
            }
            let _ = std::fmt::Write::write_fmt(&mut out, format_args!("top-layer={rank}"));
        }
        (!out.is_empty()).then_some(out)
    }

    /// The elements inside a fieldset, optgroup or select `i` whose `:enabled`/`:disabled` state
    /// token follows its `disabled` attribute (the controls, options and nested fieldsets).
    fn disabled_dependents(&self, i: u32, out: &mut Vec<u32>) {
        if !self.slot_live(i) || !matches!(self.html_local(i).map(|l| &**l), Some("fieldset" | "optgroup" | "select")) {
            return;
        }
        let mut n = self.n(i).first_child;
        while n != NIL {
            if self.is_form_control_idx(n) || self.is_face_idx(n) {
                out.push(n);
            }
            n = self.following_in(n, i);
        }
    }

    /// Elements whose [`Dom::render_states`] a change at `i` can change
    /// besides `i`: the forms, fieldsets and selects around it, and every
    /// radio when `i` is one (a renamed radio leaves one group for another).
    fn state_dependents(&self, i: u32, out: &mut Vec<u32>) {
        if !self.slot_live(i) {
            return;
        }
        let mut cur = self.n(i).parent;
        while cur != NIL {
            if let Some(e) = self.n(cur).element() {
                if e.is_html() && matches!(&*e.name.local, "form" | "fieldset" | "select") {
                    out.push(cur);
                }
            }
            cur = self.n(cur).parent;
        }
        let is_radio = |e: &crate::node::ElementData| {
            e.is_html() && &*e.name.local == "input" && e.attr("type").is_some_and(|t| t.eq_ignore_ascii_case("radio"))
        };
        if self.n(i).element().is_some_and(is_radio) {
            let root = self.root_idx(i);
            let mut n = root;
            while n != NIL {
                if n != i && self.n(n).element().is_some_and(is_radio) {
                    out.push(n);
                }
                n = self.following_in(n, root);
            }
        }
    }

    /// The inclusive ancestors of `i` whose directionality comes from their
    /// content ([`Dom::has_auto_directionality`]): a change at `i` can change
    /// their `dir` state token.
    fn auto_dir_ancestors(&self, i: u32, out: &mut Vec<u32>) {
        if !self.slot_live(i) {
            return;
        }
        let mut k = i;
        while k != NIL {
            if self.has_auto_directionality(k) {
                out.push(k);
            }
            k = match &self.n(k).kind {
                NodeKind::Fragment(f) if f.shadow.is_some() => f.host,
                _ => self.n(k).parent,
            };
        }
    }

    /// Whether child `k` appears in the render tree (as itself).
    fn render_child(&self, k: u32) -> bool {
        match &self.n(k).kind {
            NodeKind::Text(_) | NodeKind::CData(_) => true,
            // pyro: every element, `<noscript>` and `<template>` included (without their content).
            NodeKind::Element(_) => true,
            _ => false,
        }
    }

    /// Whether `slot` is the slot its shadow root assigns the nodes named `name` to (DOM "find a
    /// slot"): the first slot of that name in tree order, in a root that assigns by name. A
    /// `slotAssignment: "manual"` root assigns none (`HTMLSlotElement.assign()` holds no state
    /// yet, as `Dom::find_slot`): its slots render their fallback content.
    fn takes_assignment(&self, slot: u32, name: &str) -> bool {
        let root = self.root_idx(slot);
        match &self.n(root).kind {
            NodeKind::Fragment(f) if f.shadow.is_some() && f.flags & crate::node::shadow_flags::MANUAL_SLOTS == 0 => {}
            _ => return false,
        }
        let mut cur = self.n(root).first_child;
        while cur != NIL {
            if self.is_slot(cur) && self.n(cur).element().and_then(|e| e.attr("name")).unwrap_or("") == name {
                return cur == slot;
            }
            cur = self.following_in(cur, root);
        }
        false
    }

    /// Whether `slot`'s shadow root assigns its slots manually (`HTMLSlotElement.assign()`).
    fn assigns_manually(&self, slot: u32) -> bool {
        matches!(&self.n(self.root_idx(slot)).kind,
            NodeKind::Fragment(f) if f.shadow.is_some() && f.flags & crate::node::shadow_flags::MANUAL_SLOTS != 0)
    }

    /// Light children of `host` assigned to the slot named `name` (the
    /// default slot takes text and children without a `slot` attribute).
    fn slotted(&self, host: u32, name: &str) -> Vec<u32> {
        self.child_idx_iter(host)
            .filter(|&c| match &self.n(c).kind {
                NodeKind::Text(_) => name.is_empty(),
                NodeKind::Element(e) => e.attr("slot").unwrap_or("") == name,
                _ => false,
            })
            .collect()
    }
}

/// The render key of the `n`th adopted sheet rendered: bit 62 marks it
/// apart from node keys. Keys follow the rendering order (not the sheet), so
/// a reordered list changes the texts of the same elements: the renderer
/// cascades its sheets in the order it made their elements.
fn adopted_key(n: usize) -> u64 {
    (1 << 62) | n as u64
}

impl Dom {
    /// pyro: whether element `n` is a `<details>` element the render tree gives its user-agent
    /// shadow tree (its slots: `Step::DetailsSlot`): an HTML one that is no shadow host itself.
    fn renders_details_slots(&self, n: u32) -> bool {
        self.n(n).element().is_some_and(|e| {
            e.is_html() && &*e.name.local == "details" && (e.shadow_root == NIL || self.has_ua_shadow_idx(e.shadow_root))
        })
    }
}

fn push_children(stack: &mut Vec<Step>, kids: Vec<u32>, scope: Scope) {
    stack.extend(kids.into_iter().rev().map(|k| Step::Enter(k, scope)));
}
